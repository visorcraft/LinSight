// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! ZRAM compressed swap sensor backend.
//!
//! Enumerates `/sys/class/block/zramN/` devices and exposes:
//! * `zram.<N>.orig_data_bytes` — uncompressed data size (mm_stat field 1)
//! * `zram.<N>.compr_data_bytes` — compressed data size (mm_stat field 2)
//! * `zram.<N>.mem_used_total_bytes` — memory used for this zram (mm_stat field 3)
//!
//! Sensor rate: 0.5 Hz — ZRAM stats change slowly.
//! Device key scheme: `zram:<N>`.

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
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

#[derive(Default)]
pub struct ZramPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    devices: Vec<ZramDevice>,
}

#[derive(Clone, Debug)]
struct ZramDevice {
    index: u32,
    sysfs_path: PathBuf,
}

impl ZramPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("ZramPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.devices = enumerate(inner.sysroot.as_deref());

        let mut sensors = Vec::new();
        let mut devices = Vec::new();
        for dev in &inner.devices {
            let key = HardwareDeviceKey::try_new(format!("zram:{}", dev.index))
                .map_err(|e| PluginError::Io(format!("zram {}: {e}", dev.index)))?;
            let label = format!("ZRAM {}", dev.index);
            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Storage,
                model: label.clone(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: dev.index.to_string(),
                sensor_ids: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("zram.{}.orig_data_bytes", dev.index)),
                display_name: "Original data".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 0.5,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.index.to_string()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("zram.{}.compr_data_bytes", dev.index)),
                display_name: "Compressed data".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 0.5,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.index.to_string()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("zram.{}.mem_used_total_bytes", dev.index)),
                display_name: "Memory used".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 0.5,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.index.to_string()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
        }
        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.zram".into(),
            display_name: "ZRAM".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let inner = self.inner.lock().expect("ZramPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("zram.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (idx_str, metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let idx: u32 = idx_str.parse().map_err(|_| PluginError::Unsupported(id.into()))?;
        let dev = inner
            .devices
            .iter()
            .find(|d| d.index == idx)
            .ok_or_else(|| PluginError::Unsupported(id.into()))?;

        let mm_stat = read_mm_stat(&dev.sysfs_path)?;
        let val = match metric {
            "orig_data_bytes" => mm_stat.orig_data_size,
            "compr_data_bytes" => mm_stat.compr_data_size,
            "mem_used_total_bytes" => mm_stat.mem_used_total,
            _ => return Err(PluginError::Unsupported(id.into())),
        };
        Ok(Reading::Scalar(val as f64))
    }
}

impl LinsightPlugin for ZramPlugin {
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

struct MmStat {
    orig_data_size: u64,
    compr_data_size: u64,
    mem_used_total: u64,
}

fn read_mm_stat(sysfs_path: &Path) -> Result<MmStat, PluginError> {
    let path = sysfs_path.join("mm_stat");
    let s = fs::read_to_string(&path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() < 3 {
        return Err(PluginError::Parse(format!(
            "{}: expected ≥3 fields, got {}",
            path.display(),
            fields.len()
        )));
    }
    Ok(MmStat {
        orig_data_size: fields[0].parse().unwrap_or(0),
        compr_data_size: fields[1].parse().unwrap_or(0),
        mem_used_total: fields[2].parse().unwrap_or(0),
    })
}

fn enumerate(sysroot: Option<&Path>) -> Vec<ZramDevice> {
    let block_dir = match sysroot {
        Some(r) => r.join("sys/class/block"),
        None => PathBuf::from("/sys/class/block"),
    };
    let Ok(entries) = fs::read_dir(&block_dir) else {
        return vec![];
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let idx: &str = match name.strip_prefix("zram") {
            Some(s) => s,
            None => continue,
        };
        let Ok(index) = idx.parse::<u32>() else {
            continue;
        };
        if !entry.path().join("mm_stat").exists() {
            continue;
        }
        out.push(ZramDevice { index, sysfs_path: entry.path() });
    }
    out.sort_by_key(|d| d.index);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_plugin_sdk::{host_init, host_sample};
    use std::fs;

    fn fake_sysroot() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let block = dir.path().join("sys/class/block");
        let zram0 = block.join("zram0");
        fs::create_dir_all(&zram0).unwrap();
        fs::write(zram0.join("mm_stat"), "1073741824 536870912 134217728 0 0 0 0 0 0 0\n").unwrap();
        let zram1 = block.join("zram1");
        fs::create_dir_all(&zram1).unwrap();
        fs::write(zram1.join("mm_stat"), "2147483648 1073741824 268435456 0 0 0 0 0 0 0\n")
            .unwrap();
        dir
    }

    #[test]
    fn enumerate_finds_zram_devices() {
        let dir = fake_sysroot();
        let devs = enumerate(Some(dir.path()));
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].index, 0);
        assert_eq!(devs[1].index, 1);
    }

    #[test]
    fn manifest_advertises_zram_sensors() {
        let dir = fake_sysroot();
        let p = ZramPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        let ids: Vec<&str> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"zram.0.orig_data_bytes"));
        assert!(ids.contains(&"zram.0.compr_data_bytes"));
        assert!(ids.contains(&"zram.0.mem_used_total_bytes"));
        assert!(ids.contains(&"zram.1.orig_data_bytes"));
    }

    #[test]
    fn sample_zram_sensors() {
        let dir = fake_sysroot();
        let p = ZramPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, &SensorId::new("zram.0.orig_data_bytes")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 1073741824.0));
        let r = host_sample(&p, &SensorId::new("zram.0.compr_data_bytes")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 536870912.0));
        let r = host_sample(&p, &SensorId::new("zram.1.mem_used_total_bytes")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 268435456.0));
    }

    #[test]
    fn sample_unknown_errors() {
        let dir = fake_sysroot();
        let p = ZramPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let err = host_sample(&p, &SensorId::new("zram.99.orig_data_bytes")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn no_zram_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sys/class/block")).unwrap();
        let devs = enumerate(Some(dir.path()));
        assert!(devs.is_empty());
    }
}
