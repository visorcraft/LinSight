// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{BTreeMap, HashMap};

use linsight_core::{Sample, SensorId};
use linsight_plugin_sdk::PluginError;
use thiserror::Error;
use tracing::warn;

use std::path::PathBuf;
use std::sync::Arc;

use crate::alerts::AlertEngineHandle;
use crate::history::HistoryWriter;
use crate::plugin_host::{PluginHost, PluginMeta};

#[derive(Debug, Error)]
pub enum SchedError {
    #[error("unknown sensor: {0}")]
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    sensor: SensorId,
    period_micros: u64,
}

impl Subscription {
    pub fn sensor(&self) -> &SensorId {
        &self.sensor
    }

    pub fn period_micros(&self) -> u64 {
        self.period_micros
    }
}

struct Entry {
    period_counts: BTreeMap<u64, u32>,
    period_micros: u64,
    next_due_at_micros: u64,
    last_sampled_at_micros: Option<u64>,
    /// Value is constant for the process lifetime (sensor tagged
    /// [`linsight_plugin_sdk::STATIC_TAG`]). Sampled once per subscription,
    /// then parked: after a successful sample `tick()` sets
    /// `next_due_at_micros = u64::MAX` so it never re-polls. A fresh
    /// `subscribe()` resets it to 0 so a newly-connected client still gets
    /// one reading.
    is_static: bool,
    /// Consecutive `PluginError::Unsupported` results, used to back off so
    /// a removed/hotplugged-out device doesn't log-spam every tick. Reset
    /// on any successful sample. Cap is applied in `tick()`.
    unsupported_strikes: u32,
    /// Consecutive `PluginError::Timeout` results, used to back off sensors
    /// whose blocking external calls (NFS `statvfs`, D-Bus, NVML, etc.)
    /// repeatedly miss their deadline. Reset on any successful sample.
    timeout_strikes: u32,
}

pub struct Scheduler {
    host: Arc<PluginHost>,
    entries: HashMap<SensorId, Entry>,
    history: Option<HistoryWriter>,
    history_db_path: Option<PathBuf>,
    alerts_config_path: Option<PathBuf>,
    alerts: Option<AlertEngineHandle>,
    prom_running: bool,
}

/// One item produced by [`Scheduler::tick_plan`] and consumed by
/// [`Scheduler::tick_commit`]. Currently just the sensor id; the commit phase
/// looks up the live entry to apply state updates.
#[derive(Debug)]
pub struct TickItem {
    pub id: SensorId,
}

impl Scheduler {
    pub fn new(host: PluginHost) -> Self {
        Self {
            host: Arc::new(host),
            entries: HashMap::new(),
            history: None,
            history_db_path: None,
            alerts_config_path: None,
            alerts: None,
            prom_running: false,
        }
    }

    /// Attach a history writer so every produced sample is also persisted.
    /// Pass `None` to detach (e.g. for tests).
    pub fn set_history_writer(&mut self, writer: Option<HistoryWriter>) {
        self.history = writer;
    }

    /// Attach the history DB path so the transport can issue read queries.
    pub fn set_history_db_path(&mut self, path: Option<PathBuf>) {
        self.history_db_path = path;
    }

    /// Return the history DB path for query access.
    pub fn history_db_path(&self) -> Option<&std::path::Path> {
        self.history_db_path.as_deref()
    }

    /// Attach an alert engine so every produced sample is also evaluated
    /// against the rule set.
    pub fn set_alert_engine(&mut self, engine: Option<AlertEngineHandle>) {
        self.alerts = engine;
    }

    pub fn subscribe(
        &mut self,
        id: &SensorId,
        requested_rate_hz: Option<f32>,
    ) -> Result<Subscription, SchedError> {
        let descriptor = self
            .host
            .descriptors()
            .find(|d| &d.id == id)
            .ok_or_else(|| SchedError::Unknown(id.to_string()))?
            .clone();

        let native = descriptor.clamped_rate_hz();
        // `clamped_rate_hz` is supposed to enforce a floor of 0.1, but a
        // plugin that returns a non-finite native rate (NaN propagates
        // through `clamp` on stable) could still produce 0.0 or NaN here.
        // Treat anything non-positive as the floor instead of dividing by
        // zero (which would silently park the sensor at u64::MAX micros).
        let mut effective = match requested_rate_hz {
            Some(r) => native.min(r.clamp(linsight_plugin_sdk::MIN_RATE_HZ, native)),
            None => native,
        };
        if !effective.is_finite() || effective <= 0.0 {
            warn!(
                sensor = %id,
                requested = ?requested_rate_hz,
                native,
                "non-positive effective rate; clamping to MIN_RATE_HZ",
            );
            effective = linsight_plugin_sdk::MIN_RATE_HZ;
        }
        let period_micros = (1_000_000.0 / effective as f64) as u64;

        let is_static = descriptor.tags.iter().any(|t| t == linsight_plugin_sdk::STATIC_TAG);
        let subscription = Subscription { sensor: id.clone(), period_micros };

        self.entries
            .entry(id.clone())
            .and_modify(|e| {
                *e.period_counts.entry(period_micros).or_insert(0) += 1;
                e.period_micros = *e.period_counts.keys().next().expect("period_counts non-empty");
                // A newly-connected client should get a fresh first reading
                // rather than waiting behind an existing subscriber's cadence.
                e.next_due_at_micros = 0;
            })
            .or_insert(Entry {
                period_counts: BTreeMap::from([(period_micros, 1)]),
                period_micros,
                next_due_at_micros: 0,
                last_sampled_at_micros: None,
                is_static,
                unsupported_strikes: 0,
                timeout_strikes: 0,
            });
        Ok(subscription)
    }

    pub fn unsubscribe(&mut self, subscription: &Subscription) {
        let id = subscription.sensor();
        let should_remove = if let Some(entry) = self.entries.get_mut(id) {
            if let Some(count) = entry.period_counts.get_mut(&subscription.period_micros) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    entry.period_counts.remove(&subscription.period_micros);
                }
            }
            if entry.period_counts.is_empty() {
                true
            } else {
                entry.period_micros =
                    *entry.period_counts.keys().next().expect("period_counts non-empty");
                if !entry.is_static {
                    entry.next_due_at_micros = entry
                        .last_sampled_at_micros
                        .map_or(0, |last| last.saturating_add(entry.period_micros));
                }
                false
            }
        } else {
            false
        };
        if should_remove {
            self.entries.remove(id);
        }
    }

    /// Borrow the plugin host. The sampler clones this `Arc` and samples
    /// outside the scheduler mutex so a slow/hung plugin cannot stall
    /// subscriptions, RPCs, or other sensors.
    pub fn host(&self) -> Arc<PluginHost> {
        Arc::clone(&self.host)
    }

    /// First phase of a scheduler tick: decide which sensors are due and
    /// optimistically advance their `next_due_at_micros` so a slow sampler
    /// does not double-sample. Returns a list that the caller samples while
    /// NOT holding the scheduler mutex.
    pub fn tick_plan(&mut self, now_micros: u64) -> Vec<TickItem> {
        let mut due = Vec::new();
        for (id, entry) in self.entries.iter_mut() {
            if now_micros < entry.next_due_at_micros {
                continue;
            }
            due.push(TickItem { id: id.clone() });
            // Static sensors (total capacity, etc.) never change — park
            // indefinitely after the first reading instead of re-polling on
            // the native cadence. On error, tick_commit overwrites this.
            entry.next_due_at_micros =
                if entry.is_static { u64::MAX } else { now_micros + entry.period_micros };
        }
        due
    }

    /// Second phase of a scheduler tick: apply sampling results produced by
    /// the caller while the scheduler mutex was released. Records history,
    /// feeds alerts, and returns the successfully produced samples.
    pub fn tick_commit(
        &mut self,
        results: Vec<(TickItem, Result<Sample, PluginError>)>,
        now_micros: u64,
    ) -> Vec<Sample> {
        let mut out = Vec::with_capacity(results.len());
        for (item, result) in results {
            let Some(entry) = self.entries.get_mut(&item.id) else {
                // Sensor was unsubscribed between plan and commit.
                continue;
            };
            match result {
                Ok(sample) => {
                    entry.unsupported_strikes = 0;
                    entry.timeout_strikes = 0;
                    entry.last_sampled_at_micros = Some(now_micros);
                    if let Some(history) = &self.history {
                        history.record(&sample);
                    }
                    if let Some(alerts) = &self.alerts {
                        alerts.on_sample(&sample);
                    }
                    out.push(sample);
                }
                Err(PluginError::Unsupported(_)) => {
                    // Back off and quiet down. First strike logs; subsequent
                    // strikes extend the next-due interval exponentially so a
                    // hot-unplugged device doesn't fill the log per tick.
                    entry.unsupported_strikes = entry.unsupported_strikes.saturating_add(1);
                    if entry.unsupported_strikes == 1 {
                        warn!(sensor = %item.id, "plugin no longer supports sensor; backing off");
                    }
                    let backoff_factor = 1u64 << entry.unsupported_strikes.min(8);
                    entry.next_due_at_micros =
                        now_micros + entry.period_micros.saturating_mul(backoff_factor);
                }
                Err(PluginError::Timeout(_)) => {
                    // A sample that missed its wall-clock deadline is treated
                    // like an unsupported sensor: back off exponentially so a
                    // stuck NFS / GPU / D-Bus call cannot stall every tick.
                    entry.timeout_strikes = entry.timeout_strikes.saturating_add(1);
                    if entry.timeout_strikes == 1 {
                        warn!(sensor = %item.id, "sample timed out; backing off");
                    }
                    let backoff_factor = 1u64 << entry.timeout_strikes.min(8);
                    entry.next_due_at_micros =
                        now_micros + entry.period_micros.saturating_mul(backoff_factor);
                }
                Err(e) => {
                    warn!(sensor = %item.id, error = ?e, "sample failed");
                    entry.next_due_at_micros = now_micros + entry.period_micros;
                }
            }
        }
        out
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &linsight_plugin_sdk::SensorDescriptor> {
        self.host.descriptors()
    }

    /// Plugins currently loaded, with their identity and how many sensors
    /// they each contributed. Used by transport to build the Welcome reply.
    pub fn plugins(&self) -> impl Iterator<Item = (&PluginMeta, u32)> {
        self.host.plugins()
    }

    /// Owning plugin id for a sensor, or `None` if the sensor is unknown.
    pub fn plugin_id_for(&self, id: &SensorId) -> Option<&str> {
        self.host.plugin_id_for(id)
    }

    /// Snapshot `(descriptor, plugin_id)` pairs for a scrape plan.
    pub fn scrape_targets(&self) -> Vec<(linsight_plugin_sdk::SensorDescriptor, String)> {
        self.host
            .descriptors()
            .map(|d| {
                let plugin_id = self.host.plugin_id_for(&d.id).unwrap_or("unknown").to_owned();
                (d.clone(), plugin_id)
            })
            .collect()
    }

    /// Take a single sample of `id` outside the subscribe/tick flow. Used
    /// by the Prometheus exporter, which scrapes on demand rather than
    /// subscribing. Returns `None` on plugin error.
    pub fn sample_now(&self, id: &SensorId, ts_micros: u64) -> Option<Sample> {
        self.host.sample_to(id, ts_micros).ok()
    }

    /// Attach the alerts config path for save-back from transport dispatch.
    pub fn set_alerts_config_path(&mut self, path: Option<PathBuf>) {
        self.alerts_config_path = path;
    }

    /// Return the alerts config path for save-back.
    pub fn alerts_config_path(&self) -> Option<&std::path::Path> {
        self.alerts_config_path.as_deref()
    }

    /// Expose the alert engine handle for RPC dispatch outside the tick path.
    pub fn alert_engine_handle(&self) -> Option<AlertEngineHandle> {
        self.alerts.clone()
    }

    /// Toggle the history subsystem at runtime. Returns Ok on success or an
    /// error message on failure. Keeps the alert engine's event writer in
    /// sync so fire/clear events persist when both subsystems are enabled.
    pub fn toggle_history(&mut self, enable: bool) -> Result<(), String> {
        if enable {
            if self.history.is_some() {
                return Ok(());
            }
            let db_path = linsight_core::history_db_path();
            let retention = crate::history::retention_from_env(
                std::env::var("LINSIGHT_HISTORY_RETENTION").ok().as_deref(),
            );
            match crate::history::spawn(db_path.clone(), retention) {
                Ok((writer, _join)) => {
                    if let Some(ref alerts) = self.alerts {
                        alerts.set_event_writer(Some(writer.clone()));
                    }
                    self.history = Some(writer);
                    self.history_db_path = Some(db_path);
                    Ok(())
                }
                Err(e) => Err(format!("history spawn failed: {e}")),
            }
        } else {
            if let Some(ref alerts) = self.alerts {
                alerts.set_event_writer(None);
            }
            self.history = None;
            self.history_db_path = None;
            Ok(())
        }
    }

    /// Toggle the alert subsystem at runtime. Returns Ok on success or an
    /// error message on failure. Wires the history writer into the alert
    /// engine when both subsystems are enabled.
    pub fn toggle_alerts(&mut self, enable: bool) -> Result<(), String> {
        if enable {
            if self.alerts.is_some() {
                return Ok(());
            }
            let toml_path = crate::runtime::alerts_config_path();
            match crate::alerts::AlertEngine::load(&toml_path) {
                Ok(engine) => {
                    let handle = engine.into_handle();
                    if let Some(ref writer) = self.history {
                        handle.set_event_writer(Some(writer.clone()));
                    }
                    self.alerts = Some(handle);
                    self.alerts_config_path = Some(toml_path);
                    Ok(())
                }
                Err(e) => Err(format!("alert load failed: {e}")),
            }
        } else {
            self.alerts = None;
            self.alerts_config_path = None;
            Ok(())
        }
    }

    /// Set whether the Prometheus exporter actually bound and is running.
    pub fn set_prom_running(&mut self, running: bool) {
        self.prom_running = running;
    }

    /// Current daemon settings snapshot.
    pub fn daemon_settings(&self) -> (bool, bool, bool, Option<String>) {
        (
            self.history.is_some(),
            self.alerts.is_some(),
            self.prom_running,
            std::env::var("LINSIGHT_PROM_BIND").ok(),
        )
    }
}

#[cfg(test)]
mod tests {
    use linsight_core::SensorId;

    use super::*;

    /// Test helper that runs a full tick synchronously under the lock.
    fn tick(sched: &mut Scheduler, now_micros: u64) -> Vec<Sample> {
        let plan = sched.tick_plan(now_micros);
        let host = sched.host();
        let results: Vec<_> = plan
            .into_iter()
            .map(|item| {
                let sample = host.sample_to(&item.id, now_micros);
                (item, sample)
            })
            .collect();
        sched.tick_commit(results, now_micros)
    }

    #[test]
    fn subscribe_once_then_tick_yields_sample() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        let samples = tick(&mut sched, 1_000);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].sensor, id);
    }

    #[test]
    fn static_sensor_samples_once_then_parks() {
        // A sensor tagged STATIC_TAG (total RAM) is read once, then never
        // re-polled — even far past its native 0.1 Hz (10 s) period.
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("mem.total_bytes");
        sched.subscribe(&id, None).unwrap();
        assert_eq!(tick(&mut sched, 1_000).len(), 1, "first tick should sample once");
        assert!(tick(&mut sched, 60_000_000).is_empty(), "must not re-poll at +60s");
        assert!(tick(&mut sched, 3_600_000_000).is_empty(), "must not re-poll at +1h");
    }

    #[test]
    fn fresh_subscribe_unparks_static_sensor() {
        // A second subscriber to an already-parked static sensor still gets
        // one fresh reading (the entry is un-parked on subscribe).
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("mem.total_bytes");
        sched.subscribe(&id, None).unwrap();
        assert_eq!(tick(&mut sched, 1_000).len(), 1);
        assert!(tick(&mut sched, 60_000_000).is_empty(), "parked after first sample");
        sched.subscribe(&id, None).unwrap(); // new client
        assert_eq!(tick(&mut sched, 70_000_000).len(), 1, "new subscriber gets one reading");
    }

    #[test]
    fn second_tick_within_period_yields_nothing() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        let _ = tick(&mut sched, 1_000_000);
        let samples = tick(&mut sched, 1_500_000);
        assert!(samples.is_empty());
    }

    #[test]
    fn unsubscribe_stops_sampling() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        let sub = sched.subscribe(&id, None).unwrap();
        sched.unsubscribe(&sub);
        let samples = tick(&mut sched, 10_000_000);
        assert!(samples.is_empty());
    }

    #[test]
    fn two_subscribers_increment_refcount() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        let first = sched.subscribe(&id, None).unwrap();
        let second = sched.subscribe(&id, None).unwrap();
        sched.unsubscribe(&first);
        assert_eq!(tick(&mut sched, 10_000_000).len(), 1);
        sched.unsubscribe(&second);
        assert!(tick(&mut sched, 20_000_000).is_empty());
    }

    #[test]
    fn requested_rate_caps_at_native() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, Some(99.0)).unwrap();
        let _ = tick(&mut sched, 0);
        let samples_at_500ms = tick(&mut sched, 500_000);
        assert!(samples_at_500ms.is_empty(), "should still be once per second");
    }

    #[test]
    fn fastest_period_recomputes_when_fast_subscription_leaves() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        let slow = sched.subscribe(&id, Some(0.5)).unwrap();
        let fast = sched.subscribe(&id, None).unwrap();

        assert_eq!(tick(&mut sched, 0).len(), 1);
        assert_eq!(tick(&mut sched, 1_000_000).len(), 1);

        sched.unsubscribe(&fast);
        assert!(
            tick(&mut sched, 2_000_000).is_empty(),
            "slow subscriber should not inherit fast cadence"
        );
        assert_eq!(tick(&mut sched, 3_000_000).len(), 1);

        sched.unsubscribe(&slow);
        assert!(tick(&mut sched, 5_000_000).is_empty());
    }

    #[test]
    fn unknown_sensor_rejects_subscribe() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let err = sched.subscribe(&SensorId::new("ghost"), None).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn timeout_backs_off_and_resets_on_success() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();

        // Simulate a timed-out sample.
        let plan = sched.tick_plan(1_000_000);
        assert_eq!(plan.len(), 1);
        let item = plan.into_iter().next().unwrap();
        let results = vec![(item, Err(PluginError::Timeout("hung".into())))];
        assert!(sched.tick_commit(results, 1_000_000).is_empty());

        // The next tick at the normal cadence must be skipped because the
        // timeout strike backed the sensor off.
        assert!(sched.tick_plan(2_000_000).is_empty(), "sensor should be backed off");

        // After the backoff window the sensor is due again; a successful
        // sample resets the strike counter so future ticks resume normally.
        let plan = sched.tick_plan(3_000_000);
        assert_eq!(plan.len(), 1);
        let item = plan.into_iter().next().unwrap();
        let host = sched.host();
        let sample = host.sample_to(&item.id, 3_000_000).unwrap();
        let samples = sched.tick_commit(vec![(item, Ok(sample))], 3_000_000);
        assert_eq!(samples.len(), 1);
        // Next normal period should produce a sample again.
        assert_eq!(sched.tick_plan(4_000_000).len(), 1);
    }
}
