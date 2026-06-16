// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! SMART disk health sensor plugin.
//!
//! Reads ATA and NVMe SMART data via udisks2's D-Bus interface.
//! If udisks2 is not on the system bus, the plugin logs once and
//! registers zero sensors — never an error loop.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId,
};
use tracing::{info, warn};

use crate::udisks;

const CACHE_TTL: Duration = Duration::from_secs(30);
const UDISKS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONCURRENT_UDISKS: usize = 2;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(300);

#[derive(Default)]
pub struct SmartPlugin {
    inner: Mutex<Inner>,
}

type SmartDrives = HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>;

/// Fetch SMART drive data from udisks2 with a wall-clock timeout. The D-Bus
/// call can hang if udisks2 itself is wedged; without this, the daemon's
/// sampler thread stalls on every SMART sample. Repeated timeouts back off so
/// a stuck udisks2 cannot spawn an unbounded number of worker threads.
fn fetch_smart_drives_timeout(inner: &mut Inner) -> Result<SmartDrives, PluginError> {
    fetch_smart_drives_timeout_with(inner, crate::udisks::fetch_smart_drives, UDISKS_TIMEOUT)
}

fn fetch_smart_drives_timeout_with<F>(
    inner: &mut Inner,
    fetch: F,
    timeout: Duration,
) -> Result<SmartDrives, PluginError>
where
    F: FnOnce() -> Result<SmartDrives, String> + Send + 'static,
{
    if let Some(until) = inner.backoff_until
        && Instant::now() < until
    {
        return Err(PluginError::Unsupported("udisks2 backed off after repeated timeouts".into()));
    }

    let permit = inner.sem.acquire();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(fetch());
    });
    let result = rx.recv_timeout(timeout).map_err(|_| {
        inner.timeout_strikes = inner.timeout_strikes.saturating_add(1);
        let factor = 1u64 << inner.timeout_strikes.min(8);
        inner.backoff_until =
            Some(Instant::now() + BACKOFF_BASE.saturating_mul(factor as u32).min(BACKOFF_MAX));
        PluginError::Unsupported(format!("udisks2 fetch timed out after {timeout:?}"))
    });
    drop(permit);

    if result.is_ok() {
        inner.timeout_strikes = 0;
        inner.backoff_until = None;
    }
    result?.map_err(PluginError::Io)
}

struct Inner {
    /// Disk name → cached sensor readings.
    cache: HashMap<String, (Instant, Vec<(SensorId, Reading)>)>,
    /// Whether we already warned about missing udisks2 at init.
    warned: bool,
    /// Whether we already warned about a sample-time udisks2 failure.
    sample_warned: bool,
    /// Instant after which we may retry udisks2 after a timeout.
    backoff_until: Option<Instant>,
    /// Consecutive timeout strikes, used to escalate backoff.
    timeout_strikes: u32,
    /// Limits concurrent udisks2 worker threads.
    sem: Arc<Semaphore>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            warned: false,
            sample_warned: false,
            backoff_until: None,
            timeout_strikes: 0,
            sem: Arc::new(Semaphore::new(MAX_CONCURRENT_UDISKS)),
        }
    }
}

/// Counting semaphore implemented with std primitives. The permit is held by
/// the caller and released as soon as the timeout fires, so a stuck udisks2
/// call cannot permanently exhaust the concurrency budget.
struct Semaphore {
    permits: Mutex<usize>,
    cvar: Condvar,
    max: usize,
}

struct SemaphorePermit<'a> {
    sem: &'a Semaphore,
}

impl Semaphore {
    fn new(max: usize) -> Self {
        Self { permits: Mutex::new(max), cvar: Condvar::new(), max }
    }

    fn acquire(&self) -> SemaphorePermit<'_> {
        let mut permits = self.permits.lock().expect("smart semaphore poisoned");
        while *permits == 0 {
            permits = self.cvar.wait(permits).expect("smart semaphore poisoned");
        }
        *permits -= 1;
        SemaphorePermit { sem: self }
    }
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        let mut permits = self.sem.permits.lock().expect("smart semaphore poisoned");
        *permits = self.sem.max.min(*permits + 1);
        self.sem.cvar.notify_one();
    }
}

impl SmartPlugin {
    fn init_inner(&self, _ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("SmartPlugin poisoned");
        inner.cache.clear();

        let drives = match fetch_smart_drives_timeout(&mut inner) {
            Ok(d) => d,
            Err(e) => {
                if !inner.warned {
                    warn!("udisks2 not available: {e}; SMART sensors disabled");
                    inner.warned = true;
                }
                return Ok(PluginManifest {
                    plugin_id: "com.visorcraft.linsight.smart".into(),
                    display_name: "SMART".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    sensors: vec![],
                    devices: vec![],
                });
            }
        };

        inner.warned = false;
        inner.sample_warned = false;

        let mut sensors = Vec::new();
        let mut devices = Vec::new();
        for (disk_name, props) in &drives {
            let sensor_list = udisks::sensors_from_drive(disk_name, props)?;
            if sensor_list.is_empty() {
                continue;
            }

            let key = HardwareDeviceKey::try_new(format!("block:{disk_name}"))
                .map_err(|e| PluginError::Io(format!("block {disk_name} bad key: {e}")))?;
            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Storage,
                model: disk_name.clone(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: disk_name.clone(),
                sensor_ids: sensor_list.iter().map(|(id, _, _)| id.clone()).collect(),
            });

            for (_id, desc, _) in &sensor_list {
                sensors.push(desc.clone());
            }

            let readings: Vec<(SensorId, Reading)> =
                sensor_list.into_iter().map(|(id, _, reading)| (id, reading)).collect();
            inner.cache.insert(disk_name.clone(), (Instant::now(), readings));
        }

        info!(count = sensors.len(), "SMART sensors registered");
        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.smart".into(),
            display_name: "SMART".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("SmartPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("disk.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (name, _metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;

        // Check cache first
        if let Some((cached_at, readings)) = inner.cache.get(name)
            && cached_at.elapsed() <= CACHE_TTL
            && let Some((_, reading)) = readings.iter().find(|(sid, _)| sid == &sensor)
        {
            return Ok(reading.clone());
        }

        // Cache miss or expiry — refresh all SMART data.
        let drives = match fetch_smart_drives_timeout(&mut inner) {
            Ok(d) => d,
            Err(e) => {
                if !inner.sample_warned {
                    warn!("udisks2 fetch failed: {e}; reusing stale cache if present");
                    inner.sample_warned = true;
                }
                // Serve stale cached data rather than erroring every SMART tile
                // when D-Bus is slow or briefly hung.
                if let Some((_, readings)) = inner.cache.get(name)
                    && let Some((_, reading)) = readings.iter().find(|(sid, _)| sid == &sensor)
                {
                    return Ok(reading.clone());
                }
                return Err(e);
            }
        };
        inner.sample_warned = false;

        // Rebuild the cache from the current drive set so removed/hot-unplugged
        // drives don't leak memory forever.
        let mut new_cache = HashMap::new();
        for (disk_name, props) in &drives {
            let sensor_list = udisks::sensors_from_drive(disk_name, props)?;
            let readings: Vec<(SensorId, Reading)> =
                sensor_list.into_iter().map(|(id, _, reading)| (id, reading)).collect();
            new_cache.insert(disk_name.clone(), (Instant::now(), readings));
        }
        inner.cache = new_cache;

        // Try again after refresh
        if let Some((_, readings)) = inner.cache.get(name)
            && let Some((_, reading)) = readings.iter().find(|(sid, _)| sid == &sensor)
        {
            return Ok(reading.clone());
        }

        Err(PluginError::Unsupported(id.into()))
    }
}

impl LinsightPlugin for SmartPlugin {
    extern "C-unwind" fn init(&self, ctx: &RPluginCtx) -> RInitResult {
        let host_ctx: PluginCtx = ctx.into();
        match self.init_inner(&host_ctx) {
            Ok(manifest) => SResult::Ok(<PluginManifest as Into<RPluginManifest>>::into(manifest)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }

    extern "C-unwind" fn sample(&self, sensor: RSensorId) -> RSampleResult {
        let id: SensorId = sensor.into();
        match self.sample_inner(id) {
            Ok(r) => SResult::Ok(<Reading as Into<RReading>>::into(r)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    #[test]
    fn init_returns_manifest_without_panic() {
        let plugin = SmartPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(std::path::PathBuf::from("/")).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        // Either udisks2 is present (sensors registered) or not (empty).
        // The only hard requirement is that it doesn't panic.
        assert_eq!(manifest.plugin_id, "com.visorcraft.linsight.smart");
    }

    #[test]
    fn sample_unknown_sensor_returns_err() {
        let plugin = SmartPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(std::path::PathBuf::from("/")).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, SensorId::new("disk.nvme0n1.smart_temp_c")).unwrap_err();
        // May be Unsupported or Io depending on whether udisks2 is present.
        assert!(
            err.to_string().contains("unsupported") || err.to_string().contains("udisks2"),
            "unexpected error: {err}"
        );
    }

    fn ok_udisks() -> Result<SmartDrives, String> {
        Ok(SmartDrives::new())
    }

    fn hang_udisks() -> Result<SmartDrives, String> {
        std::thread::sleep(Duration::from_secs(60));
        Ok(SmartDrives::new())
    }

    #[test]
    fn timeout_marks_udisks_backoff() {
        let mut inner = Inner::default();
        let err =
            fetch_smart_drives_timeout_with(&mut inner, hang_udisks, Duration::from_millis(50))
                .unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)), "unexpected error: {err}");
        assert!(inner.backoff_until.is_some(), "backoff should be set");
        assert_eq!(inner.timeout_strikes, 1);
    }

    #[test]
    fn success_clears_udisks_backoff() {
        let mut inner = Inner {
            timeout_strikes: 1,
            backoff_until: Some(Instant::now() - Duration::from_millis(1)),
            ..Default::default()
        };
        let drives =
            fetch_smart_drives_timeout_with(&mut inner, ok_udisks, Duration::from_millis(50))
                .unwrap();
        assert!(drives.is_empty());
        assert_eq!(inner.timeout_strikes, 0);
        assert!(inner.backoff_until.is_none());
    }

    #[test]
    fn active_backoff_returns_unsupported_immediately() {
        let mut inner = Inner {
            backoff_until: Some(Instant::now() + Duration::from_secs(60)),
            ..Default::default()
        };
        let err =
            fetch_smart_drives_timeout_with(&mut inner, hang_udisks, Duration::from_millis(50))
                .unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)), "unexpected error: {err}");
        assert_eq!(inner.timeout_strikes, 0, "no new strike when backed off");
    }

    #[test]
    fn backoff_escalates_with_each_timeout() {
        let mut inner = Inner::default();
        let _ = fetch_smart_drives_timeout_with(&mut inner, hang_udisks, Duration::from_millis(20))
            .unwrap_err();
        let first = inner.backoff_until.unwrap();
        // Expire the first backoff so the next call actually times out again
        // rather than returning immediately from the guard.
        inner.backoff_until = Some(Instant::now() - Duration::from_millis(1));
        let _ = fetch_smart_drives_timeout_with(&mut inner, hang_udisks, Duration::from_millis(20))
            .unwrap_err();
        let second = inner.backoff_until.unwrap();
        assert!(second > first, "backoff should escalate");
        assert_eq!(inner.timeout_strikes, 2);
    }

    #[test]
    fn semaphore_caps_udisks_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let sem = Arc::new(Semaphore::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(10);
        for _ in 0..10 {
            let sem = Arc::clone(&sem);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            handles.push(std::thread::spawn(move || {
                let _permit = sem.acquire();
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(max_active.load(Ordering::SeqCst), 2);
    }
}
