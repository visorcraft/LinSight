// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::pciids::PciIdDb;
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

use crate::fdinfo::{self, DrmFdIndex, PdevSnapshot};
use crate::sysfs::{XeDevice, enumerate};

#[derive(Default)]
pub struct XePlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    devices: Vec<XeDevice>,
    /// Per-pdev last fdinfo snapshot. The util sampler diffs the latest
    /// global capture against this to compute per-class engine cycle
    /// deltas; one entry per xe pdev.
    util_state: HashMap<String, PdevSnapshot>,
    /// Cached global fdinfo snapshot + when it was taken. Sharing one
    /// /proc walk across all xe devices is the difference between
    /// O(devices × pids) and O(pids) syscalls per second.
    cached_snapshot: Option<(Instant, HashMap<String, PdevSnapshot>)>,
    /// Map of `(pid -> drm fd numbers)` from the last scan. On the
    /// fast path the plugin only reads those specific fdinfo files —
    /// for a Chrome process with 200 fds and 2 of them DRM, that's
    /// 2 reads instead of 200. Updated every scan (entries that
    /// disappear are dropped automatically).
    drm_fd_index: DrmFdIndex,
    /// When we last did a full /proc rescan. We do one periodically
    /// (every `FDINFO_FULL_RESCAN_INTERVAL`) so newly-spawned GPU
    /// clients get picked up; in between we trust `xe_pids`.
    last_full_rescan: Option<Instant>,
}

/// How long a global fdinfo capture stays fresh in
/// [`Inner::cached_snapshot`]. 400 ms keeps a 2 Hz (500 ms period)
/// sampler from EVER reading stale data, while still letting two
/// xe devices sampled back-to-back share one /proc walk.
const FDINFO_CACHE_TTL: std::time::Duration = std::time::Duration::from_millis(400);

/// How often the xe plugin does a full /proc walk to refresh its
/// `xe_pids` cache. Between rescans, we only read fdinfo from PIDs
/// already known to hold drm fds. Trade-off: a GPU app launched in
/// the last few seconds may not appear in utilization metrics until
/// the next rescan. 15 s is short enough that newly-spawned clients
/// register quickly and long enough that the rescan cost (one full
/// /proc walk, ~25 k syscalls on a typical desktop) amortizes to
/// under 2 k syscalls/sec — well below the cost of other sensors.
const FDINFO_FULL_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

impl XePlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("XePlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.devices = enumerate(ctx.sysroot()).map_err(|e| PluginError::Io(e.to_string()))?;
        inner.util_state.clear();

        let pci_db = PciIdDb::shared();
        let mut devices = Vec::with_capacity(inner.devices.len());
        let mut device_keys = Vec::with_capacity(inner.devices.len());

        for (idx, dev) in inner.devices.iter().enumerate() {
            let key_str = format!("pci:{}", dev.pci_slot);
            let key = HardwareDeviceKey::try_new(key_str)
                .map_err(|e| PluginError::Io(format!("xe gpu{idx} bad key: {e}")))?;

            let model = match (dev.vendor_id, dev.device_id) {
                (Some(v), Some(d)) => {
                    pci_db.lookup(v, d).unwrap_or_else(|| format!("Intel GPU ({v:04x}:{d:04x})"))
                }
                _ => format!("Intel GPU (gpu{idx})"),
            };
            let vendor = dev.vendor_id.and_then(|v| pci_db.vendor_name(v));

            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Gpu,
                model,
                vendor,
                location: Some(format!("PCI {}", dev.pci_slot)),
                plugin_id: String::new(),
                plugin_device_id: format!("gpu{idx}"),
                sensor_ids: vec![],
            });
            device_keys.push(key);
        }

        let mut sensors = Vec::with_capacity(inner.devices.len() * 4);
        for (idx, dev) in inner.devices.iter().enumerate() {
            let device_id = format!("gpu{idx}");
            // Device identity is carried via device_key → device_label and
            // shown as a second title line; keep display_name a bare metric.
            let device_key = device_keys[idx].clone();
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("xe.gpu{idx}.util")),
                display_name: "GPU utilization".into(),
                unit: Unit::Percent,
                kind: SensorKind::Scalar,
                category: Category::Gpu,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: Some(100.0),
                device_id: Some(device_id.clone()),
                device_key: Some(device_key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                // Sensor ID, unit and emitted value were previously in
                // three-way disagreement: ID was `freq_mhz`, unit was
                // `Hertz`, value was MHz × 1e6. Renamed to `freq_hz` so
                // the ID matches the (Hz) unit and the multiplied value.
                id: SensorId::new(format!("xe.gpu{idx}.freq_hz")),
                display_name: "GPU frequency".into(),
                unit: Unit::Hertz,
                kind: SensorKind::Scalar,
                category: Category::Gpu,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: Some(device_id.clone()),
                device_key: Some(device_key.clone()),
                tags: vec![],
            });
            if dev.package_temp_milli_c().is_some() {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("xe.gpu{idx}.temp_c")),
                    display_name: "GPU temperature".into(),
                    unit: Unit::Celsius,
                    kind: SensorKind::Scalar,
                    category: Category::Gpu,
                    native_rate_hz: 1.0,
                    min: None,
                    max: None,
                    device_id: Some(device_id.clone()),
                    device_key: Some(device_key.clone()),
                    tags: vec![],
                });
            }
            if dev.fan_rpm().is_some() {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("xe.gpu{idx}.fan_rpm")),
                    display_name: "GPU fan".into(),
                    unit: Unit::Rpm,
                    kind: SensorKind::Scalar,
                    category: Category::Gpu,
                    native_rate_hz: 1.0,
                    min: Some(0.0),
                    max: None,
                    device_id: Some(device_id.clone()),
                    device_key: Some(device_key.clone()),
                    tags: vec![],
                });
            }
            if dev.vram_total_bytes().is_some() {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("xe.gpu{idx}.mem_total_bytes")),
                    display_name: "GPU VRAM total".into(),
                    unit: Unit::Bytes,
                    kind: SensorKind::Scalar,
                    category: Category::Gpu,
                    // Hardware-static: total VRAM never changes, so the
                    // STATIC_TAG below tells the scheduler to sample it once
                    // rather than re-poll on this cadence.
                    native_rate_hz: 0.1,
                    min: Some(0.0),
                    max: None,
                    device_id: Some(device_id),
                    device_key: Some(device_key),
                    tags: vec![linsight_plugin_sdk::STATIC_TAG.into()],
                });
            }
        }

        Ok(PluginManifest {
            plugin_id: "io.visorcraft.linsight.xe".into(),
            display_name: "Intel xe GPU".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("XePlugin poisoned");
        let (idx, metric) = parse_sensor_id(sensor.as_str())
            .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;
        let dev =
            inner.devices.get(idx).ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;

        match metric {
            "util" => {
                let pdev = dev.pci_slot.clone();
                let now = Instant::now();

                // Decide which scan path to use this call:
                // * Cache hit (snapshot < 400 ms old): no scan at all,
                //   reuse the cached snapshot. Sibling xe devices
                //   sampled in the same scheduler tick land here.
                // * Full rescan (snapshot stale + last full was > 5 s ago,
                //   or first call): walk all of /proc, rebuild xe_pids.
                // * Fast scan (snapshot stale, last full < 5 s ago):
                //   walk only PIDs in xe_pids. Skips ~95% of /proc on a
                //   typical desktop.
                let cache_fresh = inner
                    .cached_snapshot
                    .as_ref()
                    .is_some_and(|(taken, _)| now.duration_since(*taken) < FDINFO_CACHE_TTL);
                if !cache_fresh {
                    let full_due = inner
                        .last_full_rescan
                        .is_none_or(|t| now.duration_since(t) >= FDINFO_FULL_RESCAN_INTERVAL);
                    let sysroot = inner.sysroot.clone();
                    let (fresh_snap, fresh_index) = if full_due {
                        let r = fdinfo::capture_all_filtered(sysroot.as_deref(), None);
                        inner.last_full_rescan = Some(now);
                        r
                    } else {
                        // Fast path: read only the SPECIFIC fdinfo
                        // entries already known to be DRM. The returned
                        // index drops pids/fds that disappeared so the
                        // next fast scan stays accurate without waiting
                        // for the periodic full rescan.
                        fdinfo::capture_all_filtered(sysroot.as_deref(), Some(&inner.drm_fd_index))
                    };
                    inner.drm_fd_index = fresh_index;
                    inner.cached_snapshot = Some((now, fresh_snap));
                }

                let cur = inner
                    .cached_snapshot
                    .as_ref()
                    .and_then(|(_, m)| m.get(&pdev).cloned())
                    .unwrap_or_default();
                let util = match inner.util_state.get(&pdev) {
                    Some(prev) => fdinfo::max_util(prev, &cur) * 100.0,
                    None => 0.0,
                };
                inner.util_state.insert(pdev, cur);
                Ok(Reading::Scalar(util))
            }
            "freq_hz" => {
                let f = dev.act_freq_mhz().map_err(|e| PluginError::Io(e.to_string()))?;
                // sysfs gives us MHz; multiply to Hz to match the
                // sensor ID and the Unit::Hertz declaration.
                Ok(Reading::Scalar(f as f64 * 1_000_000.0))
            }
            "temp_c" => match dev.package_temp_milli_c() {
                Some(milli) => Ok(Reading::Scalar(milli as f64 / 1000.0)),
                None => Err(PluginError::Unsupported(sensor.to_string())),
            },
            "fan_rpm" => match dev.fan_rpm() {
                Some(rpm) => Ok(Reading::Scalar(rpm as f64)),
                None => Err(PluginError::Unsupported(sensor.to_string())),
            },
            "mem_total_bytes" => match dev.vram_total_bytes() {
                // Reading::Scalar(f64) is the convention used by NVML
                // for the same metric — keeps the consumer-side
                // formatting (`16.00 GiB`) consistent across vendors.
                Some(b) => Ok(Reading::Scalar(b as f64)),
                None => Err(PluginError::Unsupported(sensor.to_string())),
            },
            _ => Err(PluginError::Unsupported(sensor.to_string())),
        }
    }
}

impl LinsightPlugin for XePlugin {
    extern "C" fn init(&self, ctx: &RPluginCtx) -> RInitResult {
        let host_ctx: PluginCtx = ctx.into();
        match self.init_inner(&host_ctx) {
            Ok(m) => SResult::Ok(<PluginManifest as Into<RPluginManifest>>::into(m)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }

    extern "C" fn sample(&self, sensor: RSensorId) -> RSampleResult {
        let id: SensorId = sensor.into();
        match self.sample_inner(id) {
            Ok(r) => SResult::Ok(<Reading as Into<RReading>>::into(r)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }
}

fn parse_sensor_id(id: &str) -> Option<(usize, &str)> {
    let rest = id.strip_prefix("xe.gpu")?;
    let (idx_str, metric) = rest.split_once('.')?;
    let idx = idx_str.parse::<usize>().ok()?;
    Some((idx, metric))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::host_init;

    use super::*;

    fn make_xe_card(root: &std::path::Path, idx: u32, slot: &str, dev_id: &str) {
        let card = root.join(format!("sys/class/drm/card{idx}"));
        fs::create_dir_all(card.join("device/tile0/gt0/freq0")).unwrap();
        // Driver symlink — name must be `xe`.
        let drv = root.join("sys/bus/pci/drivers/xe");
        fs::create_dir_all(&drv).unwrap();
        std::os::unix::fs::symlink(&drv, card.join("device/driver")).unwrap();
        // The pci_slot is derived from `read_link(card<N>/device)`, where
        // the last path component is the slot. Point it at a synthetic
        // sysfs path under `sys/devices/...` with the desired slot.
        let target = root.join(format!("sys/devices/pci0000:00/{slot}"));
        fs::create_dir_all(&target).unwrap();
        // The existing `device` directory was created above. We replace
        // it with a symlink to the slot target so `read_link` finds it.
        fs::remove_dir_all(card.join("device")).unwrap();
        std::os::unix::fs::symlink(&target, card.join("device")).unwrap();
        // Now populate the contents the plugin reads.
        fs::create_dir_all(target.join("tile0/gt0/freq0")).unwrap();
        std::os::unix::fs::symlink(&drv, target.join("driver")).unwrap();
        fs::write(target.join("tile0/gt0/freq0/act_freq"), "1200\n").unwrap();
        fs::write(target.join("vendor"), "0x8086\n").unwrap();
        fs::write(target.join("device"), format!("{dev_id}\n")).unwrap();
    }

    #[test]
    fn manifest_emits_pci_devices_per_card() {
        let dir = tempfile::TempDir::new().unwrap();
        make_xe_card(dir.path(), 0, "0000:01:00.0", "0xb0a0");
        make_xe_card(dir.path(), 2, "0000:03:00.0", "0xe223");

        let plugin = XePlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 2);

        let keys: std::collections::HashSet<_> =
            manifest.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        for s in &manifest.sensors {
            let k = s.device_key.as_ref().expect("xe sensors must have device_key");
            assert!(keys.contains(k.as_str()), "sensor key {k} not in manifest devices");
        }
        for d in &manifest.devices {
            assert!(d.key.as_str().starts_with("pci:"));
            assert_eq!(d.category, linsight_core::HardwareCategory::Gpu);
            assert!(!d.model.is_empty());
        }
    }
}
