// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

//! NVMe SSD sensor backend.
//!
//! Per controller (`nvme<N>`):
//! * `nvme.<id>.temp_c` — composite temperature from the first hwmon
//! * `nvme.<id>.bytes_read` — cumulative bytes read (Counter)
//! * `nvme.<id>.bytes_written` — cumulative bytes written (Counter)
//!
//! Bytes are derived from the namespace's `/sys/class/block/nvme<N>n1/stat`
//! file (sectors × 512).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, STATIC_TAG, SensorDescriptor,
};

#[derive(Default)]
pub struct NvmePlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    devices: Vec<NvmeDevice>,
}

#[derive(Clone, Debug)]
struct NvmeDevice {
    /// e.g. "nvme0"
    name: String,
    /// Human-readable model, e.g. "Samsung SSD 990 PRO 4TB".
    model: String,
    /// World-Wide Identifier — globally unique device identifier when
    /// the controller exposes one. Preferred payload for the device key.
    wwid: Option<String>,
    /// Vendor-assigned serial number — second-choice payload for the
    /// device key when no WWID is available.
    serial: Option<String>,
    /// /sys/class/nvme/nvmeN/hwmonM
    hwmon_root: Option<PathBuf>,
    /// /sys/class/block/nvmeNn1/stat — first namespace's I/O stats.
    block_stat: Option<PathBuf>,
    /// Capacity in bytes.
    capacity_bytes: u64,
}

/// Pick the most stable identifier available and normalize it to
/// lowercase so it satisfies the `HardwareDeviceKey` charset.
fn nvme_key_payload(dev: &NvmeDevice) -> String {
    dev.wwid
        .clone()
        .or_else(|| dev.serial.clone())
        .unwrap_or_else(|| dev.name.clone())
        .to_ascii_lowercase()
}

impl NvmePlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("NvmePlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.devices = enumerate(ctx.sysroot()).map_err(|e| PluginError::Io(e.to_string()))?;

        let mut sensors = Vec::with_capacity(inner.devices.len() * 4);
        let mut devices: Vec<HardwareDevice> = Vec::with_capacity(inner.devices.len());
        for dev in &inner.devices {
            // Device identity is carried via device_key → device_label and
            // shown as a second title line; keep display_name a bare metric.
            let key = HardwareDeviceKey::try_new(format!("nvme:{}", nvme_key_payload(dev)))
                .map_err(|e| PluginError::Io(format!("nvme {} bad key: {e}", dev.name)))?;
            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Storage,
                model: dev.model.clone(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: dev.name.clone(),
                sensor_ids: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("nvme.{}.capacity_bytes", dev.name)),
                display_name: "NVMe capacity".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 0.2,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![STATIC_TAG.into()],
            });
            if dev.hwmon_root.is_some() {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("nvme.{}.temp_c", dev.name)),
                    display_name: "NVMe temperature".into(),
                    unit: Unit::Celsius,
                    kind: SensorKind::Scalar,
                    category: Category::Storage,
                    native_rate_hz: 0.5,
                    min: None,
                    max: None,
                    device_id: Some(dev.name.clone()),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
            }
            if dev.block_stat.is_some() {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("nvme.{}.bytes_read", dev.name)),
                    display_name: "NVMe bytes read".into(),
                    unit: Unit::Bytes,
                    kind: SensorKind::Counter,
                    category: Category::Storage,
                    native_rate_hz: 1.0,
                    min: Some(0.0),
                    max: None,
                    device_id: Some(dev.name.clone()),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("nvme.{}.bytes_written", dev.name)),
                    display_name: "NVMe bytes written".into(),
                    unit: Unit::Bytes,
                    kind: SensorKind::Counter,
                    category: Category::Storage,
                    native_rate_hz: 1.0,
                    min: Some(0.0),
                    max: None,
                    device_id: Some(dev.name.clone()),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
            }
        }
        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.nvme".into(),
            display_name: "NVMe".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let inner = self.inner.lock().expect("NvmePlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("nvme.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (name, metric) =
            rest.split_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let dev = inner
            .devices
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| PluginError::Unsupported(id.into()))?;
        match metric {
            "capacity_bytes" => Ok(Reading::Scalar(dev.capacity_bytes as f64)),
            "temp_c" => {
                let hwmon =
                    dev.hwmon_root.as_ref().ok_or_else(|| PluginError::Unsupported(id.into()))?;
                let milli = read_i64(&hwmon.join("temp1_input"))?;
                Ok(Reading::Scalar(milli as f64 / 1000.0))
            }
            "bytes_read" | "bytes_written" => {
                let stat =
                    dev.block_stat.as_ref().ok_or_else(|| PluginError::Unsupported(id.into()))?;
                let parsed = read_block_stat(stat)?;
                let bytes = if metric == "bytes_read" {
                    parsed.sectors_read.saturating_mul(512)
                } else {
                    parsed.sectors_written.saturating_mul(512)
                };
                Ok(Reading::Counter(bytes))
            }
            _ => Err(PluginError::Unsupported(id.into())),
        }
    }
}

impl LinsightPlugin for NvmePlugin {
    extern "C-unwind" fn init(&self, ctx: &RPluginCtx) -> RInitResult {
        let host_ctx: PluginCtx = ctx.into();
        match self.init_inner(&host_ctx) {
            Ok(m) => SResult::Ok(<PluginManifest as Into<RPluginManifest>>::into(m)),
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

#[derive(Clone, Copy, Debug, Default)]
struct BlockStat {
    sectors_read: u64,
    sectors_written: u64,
}

fn read_block_stat(p: &Path) -> Result<BlockStat, PluginError> {
    let s = fs::read_to_string(p).map_err(|e| PluginError::Io(format!("{}: {e}", p.display())))?;
    let mut fields = s.split_whitespace();
    let _reads = fields.next();
    let _read_merged = fields.next();
    let sectors_read = fields
        .next()
        .and_then(|t| t.parse::<u64>().ok())
        .ok_or_else(|| PluginError::Parse("missing sectors_read".into()))?;
    let _read_ticks = fields.next();
    let _writes = fields.next();
    let _write_merged = fields.next();
    let sectors_written = fields
        .next()
        .and_then(|t| t.parse::<u64>().ok())
        .ok_or_else(|| PluginError::Parse("missing sectors_written".into()))?;
    Ok(BlockStat { sectors_read, sectors_written })
}

fn enumerate(sysroot: Option<&Path>) -> Result<Vec<NvmeDevice>, std::io::Error> {
    let nvme_root = match sysroot {
        Some(r) => r.join("sys/class/nvme"),
        None => PathBuf::from("/sys/class/nvme"),
    };
    let block_root = match sysroot {
        Some(r) => r.join("sys/class/block"),
        None => PathBuf::from("/sys/class/block"),
    };
    let entries = match fs::read_dir(&nvme_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_nvme_controller_name(&name) {
            continue;
        }
        let ctrl_root = entry.path();
        let model = match fs::read_to_string(ctrl_root.join("model")) {
            Ok(s) => s.trim().to_owned(),
            Err(_) => format!("unknown model ({name})"),
        };
        let hwmon_root = find_hwmon(&ctrl_root);
        let block_stat = {
            let candidate = block_root.join(format!("{name}n1/stat"));
            if candidate.exists() { Some(candidate) } else { None }
        };

        let capacity_bytes = if let Some(ref stat_path) = block_stat {
            let size_path = stat_path.parent().unwrap().join("size");
            if let Ok(s) = fs::read_to_string(&size_path) {
                s.trim().parse::<u64>().map(|sectors| sectors * 512).unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        };

        let wwid = read_trimmed_nonempty(&ctrl_root.join("wwid"));
        let serial = read_trimmed_nonempty(&ctrl_root.join("serial"));
        out.push(NvmeDevice { name, model, wwid, serial, hwmon_root, block_stat, capacity_bytes });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn is_nvme_controller_name(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("nvme") else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod controller_name_tests {
    use super::is_nvme_controller_name;

    #[test]
    fn accepts_simple_controllers() {
        assert!(is_nvme_controller_name("nvme0"));
        assert!(is_nvme_controller_name("nvme7"));
        assert!(is_nvme_controller_name("nvme123"));
    }

    #[test]
    fn rejects_subsystem_or_fabrics_entries() {
        assert!(!is_nvme_controller_name("nvme-subsystem0"));
        assert!(!is_nvme_controller_name("nvme-fabrics"));
        assert!(!is_nvme_controller_name("nvme"));
        assert!(!is_nvme_controller_name("nvme0n1"));
    }
}

fn read_trimmed_nonempty(p: &Path) -> Option<String> {
    let s = fs::read_to_string(p).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) }
}

fn find_hwmon(ctrl_root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(ctrl_root).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("hwmon") {
            return Some(e.path());
        }
    }
    None
}

fn read_i64(p: &Path) -> Result<i64, PluginError> {
    let s = fs::read_to_string(p).map_err(|e| PluginError::Io(format!("{}: {e}", p.display())))?;
    s.trim().parse::<i64>().map_err(|e| PluginError::Parse(format!("{}: {e}", p.display())))
}

#[cfg(test)]
mod tests {
    use linsight_plugin_sdk::host_init;

    use super::*;

    #[test]
    fn manifest_emits_nvme_devices_with_wwid_preference() {
        let dir = tempfile::TempDir::new().unwrap();

        // nvme0: wwid present — should be preferred over serial.
        let n0 = dir.path().join("sys/class/nvme/nvme0");
        fs::create_dir_all(n0.join("hwmon5")).unwrap();
        fs::write(n0.join("model"), "Samsung SSD 990 PRO 2TB\n").unwrap();
        fs::write(n0.join("wwid"), "eui.001b448b41234567\n").unwrap();
        fs::write(n0.join("hwmon5/temp1_input"), "42000\n").unwrap();
        let n0_block = dir.path().join("sys/class/block/nvme0n1");
        fs::create_dir_all(&n0_block).unwrap();
        fs::write(n0_block.join("stat"), "0 0 12345 0 0 0 678910 0 0 0 0\n").unwrap();
        fs::write(n0_block.join("size"), "500000000\n").unwrap();

        // nvme1: no wwid, serial only — falls back to serial.
        let n1 = dir.path().join("sys/class/nvme/nvme1");
        fs::create_dir_all(&n1).unwrap();
        fs::write(n1.join("model"), "WD_BLACK SN850X 1TB\n").unwrap();
        fs::write(n1.join("serial"), "WD-XYZ123\n").unwrap();
        let n1_block = dir.path().join("sys/class/block/nvme1n1");
        fs::create_dir_all(&n1_block).unwrap();
        fs::write(n1_block.join("stat"), "0 0 12345 0 0 0 678910 0 0 0 0\n").unwrap();
        fs::write(n1_block.join("size"), "250000000\n").unwrap();

        let plugin = NvmePlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 2);

        let n0_dev =
            manifest.devices.iter().find(|d| d.plugin_device_id == "nvme0").expect("nvme0 device");
        assert_eq!(n0_dev.key.as_str(), "nvme:eui.001b448b41234567");

        let n1_dev =
            manifest.devices.iter().find(|d| d.plugin_device_id == "nvme1").expect("nvme1 device");
        assert_eq!(n1_dev.key.as_str(), "nvme:wd-xyz123");

        // Every emitted sensor must reference a manifest device.
        let keys: std::collections::HashSet<_> =
            manifest.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        for s in &manifest.sensors {
            let k = s.device_key.as_ref().expect("nvme sensors must have device_key");
            assert!(keys.contains(k.as_str()), "sensor key {k} not in manifest devices");
        }

        // Check capacity sensor
        let cap_sensor =
            manifest.sensors.iter().find(|s| s.id.as_str() == "nvme.nvme0.capacity_bytes").unwrap();
        assert!(cap_sensor.tags.contains(&linsight_plugin_sdk::STATIC_TAG.to_string()));
    }

    #[test]
    fn enumerate_synthetic_nvme() {
        let dir = tempfile::TempDir::new().unwrap();
        let nvme0 = dir.path().join("sys/class/nvme/nvme0");
        fs::create_dir_all(nvme0.join("hwmon5")).unwrap();
        fs::write(nvme0.join("model"), "Synthetic SSD\n").unwrap();
        fs::write(nvme0.join("hwmon5/temp1_input"), "42000\n").unwrap();
        let block = dir.path().join("sys/class/block/nvme0n1");
        fs::create_dir_all(&block).unwrap();
        fs::write(block.join("stat"), "0 0 12345 0 0 0 678910 0 0 0 0\n").unwrap();

        let devices = enumerate(Some(dir.path())).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "nvme0");
        assert!(devices[0].hwmon_root.is_some());
        assert!(devices[0].block_stat.is_some());
    }
}
