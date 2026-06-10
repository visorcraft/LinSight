// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! SMART disk health sensor plugin.
//!
//! Reads ATA and NVMe SMART data via udisks2's D-Bus interface.
//! If udisks2 is not on the session bus, the plugin logs once and
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

#[derive(Default)]
pub struct SmartPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<std::path::PathBuf>,
    /// Disk name → cached sensor readings.
    cache: HashMap<String, (Instant, Vec<(SensorId, Reading)>)>,
    /// Whether we already warned about missing udisks2.
    warned: bool,
}

impl SmartPlugin {
    fn init_inner(
        &self,
        ctx: &PluginCtx,
    ) -> Result<(PluginManifest, Vec<HardwareDevice>), PluginError> {
        let mut inner = self.inner.lock().expect("SmartPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.cache.clear();
        inner.warned = false;

        let drives = match crate::udisks::fetch_smart_drives() {
            Ok(d) => d,
            Err(e) => {
                if !inner.warned {
                    warn!("udisks2 not available: {e}; SMART sensors disabled");
                    inner.warned = true;
                }
                return Ok((
                    PluginManifest {
                        plugin_id: "com.visorcraft.linsight.smart".into(),
                        display_name: "SMART".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        sensors: vec![],
                        devices: vec![],
                    },
                    vec![],
                ));
            }
        };

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
        Ok((
            PluginManifest {
                plugin_id: "com.visorcraft.linsight.smart".into(),
                display_name: "SMART".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                sensors,
                devices: devices.clone(),
            },
            devices,
        ))
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

        // Cache miss or expiry — refresh all SMART data
        let drives = match crate::udisks::fetch_smart_drives() {
            Ok(d) => d,
            Err(e) => {
                return Err(PluginError::Io(format!("udisks2 fetch failed: {e}")));
            }
        };

        for (disk_name, props) in &drives {
            let sensor_list = udisks::sensors_from_drive(disk_name, props)?;
            let readings: Vec<(SensorId, Reading)> =
                sensor_list.into_iter().map(|(id, _, reading)| (id, reading)).collect();
            inner.cache.insert(disk_name.clone(), (Instant::now(), readings));
        }

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
            Ok((manifest, _devices)) => {
                SResult::Ok(<PluginManifest as Into<RPluginManifest>>::into(manifest))
            }
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
