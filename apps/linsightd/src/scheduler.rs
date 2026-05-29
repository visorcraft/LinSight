// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use linsight_core::{Sample, SensorId};
use linsight_plugin_sdk::PluginError;
use thiserror::Error;
use tracing::warn;

use std::path::PathBuf;

use crate::alerts::AlertEngineHandle;
use crate::history::HistoryWriter;
use crate::plugin_host::{PluginHost, PluginMeta};

#[derive(Debug, Error)]
pub enum SchedError {
    #[error("unknown sensor: {0}")]
    Unknown(String),
}

struct Entry {
    refcount: u32,
    period_micros: u64,
    next_due_at_micros: u64,
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
}

pub struct Scheduler {
    host: PluginHost,
    entries: HashMap<SensorId, Entry>,
    history: Option<HistoryWriter>,
    history_db_path: Option<PathBuf>,
    alerts_config_path: Option<PathBuf>,
    alerts: Option<AlertEngineHandle>,
}

impl Scheduler {
    pub fn new(host: PluginHost) -> Self {
        Self {
            host,
            entries: HashMap::new(),
            history: None,
            history_db_path: None,
            alerts_config_path: None,
            alerts: None,
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
    ) -> Result<(), SchedError> {
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

        self.entries
            .entry(id.clone())
            .and_modify(|e| {
                e.refcount += 1;
                // A newly-connected client subscribing to an already-parked
                // static sensor needs one fresh reading; un-park it.
                if e.is_static {
                    e.next_due_at_micros = 0;
                }
            })
            .or_insert(Entry {
                refcount: 1,
                period_micros,
                next_due_at_micros: 0,
                is_static,
                unsupported_strikes: 0,
            });
        Ok(())
    }

    pub fn unsubscribe(&mut self, id: &SensorId) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.refcount = entry.refcount.saturating_sub(1);
            if entry.refcount == 0 {
                self.entries.remove(id);
            }
        }
    }

    pub fn tick(&mut self, now_micros: u64) -> Vec<Sample> {
        let mut out = Vec::new();
        for (id, entry) in self.entries.iter_mut() {
            if now_micros < entry.next_due_at_micros {
                continue;
            }
            match self.host.sample_to(id, now_micros) {
                Ok(sample) => {
                    entry.unsupported_strikes = 0;
                    if let Some(history) = &self.history {
                        history.record(sample.clone());
                    }
                    if let Some(alerts) = &self.alerts {
                        alerts.on_sample(&sample);
                    }
                    out.push(sample);
                    // Static sensors (total capacity, etc.) never change —
                    // park indefinitely after the first reading instead of
                    // re-polling on the native cadence.
                    entry.next_due_at_micros =
                        if entry.is_static { u64::MAX } else { now_micros + entry.period_micros };
                }
                Err(PluginError::Unsupported(_)) => {
                    // Back off and quiet down. First strike logs; subsequent
                    // strikes extend the next-due interval exponentially so a
                    // hot-unplugged device doesn't fill the log per tick.
                    entry.unsupported_strikes = entry.unsupported_strikes.saturating_add(1);
                    if entry.unsupported_strikes == 1 {
                        warn!(sensor = %id, "plugin no longer supports sensor; backing off");
                    }
                    let backoff_factor = 1u64 << entry.unsupported_strikes.min(8);
                    entry.next_due_at_micros =
                        now_micros + entry.period_micros.saturating_mul(backoff_factor);
                }
                Err(e) => {
                    warn!(sensor = %id, error = ?e, "sample failed");
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
}

#[cfg(test)]
mod tests {
    use linsight_core::SensorId;

    use super::*;

    #[test]
    fn subscribe_once_then_tick_yields_sample() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        let samples = sched.tick(1_000);
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
        assert_eq!(sched.tick(1_000).len(), 1, "first tick should sample once");
        assert!(sched.tick(60_000_000).is_empty(), "must not re-poll at +60s");
        assert!(sched.tick(3_600_000_000).is_empty(), "must not re-poll at +1h");
    }

    #[test]
    fn fresh_subscribe_unparks_static_sensor() {
        // A second subscriber to an already-parked static sensor still gets
        // one fresh reading (the entry is un-parked on subscribe).
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("mem.total_bytes");
        sched.subscribe(&id, None).unwrap();
        assert_eq!(sched.tick(1_000).len(), 1);
        assert!(sched.tick(60_000_000).is_empty(), "parked after first sample");
        sched.subscribe(&id, None).unwrap(); // new client
        assert_eq!(sched.tick(70_000_000).len(), 1, "new subscriber gets one reading");
    }

    #[test]
    fn second_tick_within_period_yields_nothing() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        let _ = sched.tick(1_000_000);
        let samples = sched.tick(1_500_000);
        assert!(samples.is_empty());
    }

    #[test]
    fn unsubscribe_stops_sampling() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        sched.unsubscribe(&id);
        let samples = sched.tick(10_000_000);
        assert!(samples.is_empty());
    }

    #[test]
    fn two_subscribers_increment_refcount() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        sched.subscribe(&id, None).unwrap();
        sched.unsubscribe(&id);
        assert_eq!(sched.tick(10_000_000).len(), 1);
        sched.unsubscribe(&id);
        assert!(sched.tick(20_000_000).is_empty());
    }

    #[test]
    fn requested_rate_caps_at_native() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, Some(99.0)).unwrap();
        let _ = sched.tick(0);
        let samples_at_500ms = sched.tick(500_000);
        assert!(samples_at_500ms.is_empty(), "should still be once per second");
    }

    #[test]
    fn unknown_sensor_rejects_subscribe() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let err = sched.subscribe(&SensorId::new("ghost"), None).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }
}
