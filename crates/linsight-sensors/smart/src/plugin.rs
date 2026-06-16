// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! SMART disk health sensor plugin.
//!
//! Reads ATA and NVMe SMART data via udisks2's D-Bus interface.
//! If udisks2 is not on the system bus, the plugin logs once and
//! registers zero sensors — never an error loop.

use std::collections::HashMap;
use std::sync::Mutex;
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

#[derive(Default)]
pub struct SmartPlugin {
    inner: Mutex<Inner>,
}

/// Fetch SMART drive data from udisks2 with a wall-clock timeout. The D-Bus
/// call can hang if udisks2 itself is wedged; without this, the daemon's
/// sampler thread stalls on every SMART sample.
fn fetch_smart_drives_timeout() -> Result<
    std::collections::HashMap<
        String,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    >,
    PluginError,
> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::udisks::fetch_smart_drives());
    });
    rx.recv_timeout(UDISKS_TIMEOUT)
        .map_err(|_| PluginError::Io(format!("udisks2 fetch timed out after {UDISKS_TIMEOUT:?}")))?
        .map_err(PluginError::Io)
}

#[derive(Default)]
struct Inner {
    /// Disk name → cached sensor readings.
    cache: HashMap<String, (Instant, Vec<(SensorId, Reading)>)>,
    /// Whether we already warned about missing udisks2 at init.
    warned: bool,
    /// Whether we already warned about a sample-time udisks2 failure.
    sample_warned: bool,
}

impl SmartPlugin {
    fn init_inner(&self, _ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("SmartPlugin poisoned");
        inner.cache.clear();

        let drives = match fetch_smart_drives_timeout() {
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
        let drives = match fetch_smart_drives_timeout() {
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
}
