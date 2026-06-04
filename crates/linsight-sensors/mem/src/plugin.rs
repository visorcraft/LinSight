// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use linsight_core::{Category, Reading, SensorId, SensorKind, Unit};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

use crate::meminfo::{Meminfo, read_meminfo};

const MEMINFO_CACHE_TTL: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct MemPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    meminfo_cache: Option<MeminfoCache>,
}

struct MeminfoCache {
    captured_at: Instant,
    snapshot: Meminfo,
}

impl MemPlugin {
    fn meminfo_snapshot(inner: &mut Inner) -> Result<Meminfo, PluginError> {
        if let Some(cache) = &inner.meminfo_cache
            && cache.captured_at.elapsed() <= MEMINFO_CACHE_TTL
        {
            return Ok(cache.snapshot);
        }

        let snapshot =
            read_meminfo(inner.sysroot.as_deref()).map_err(|e| PluginError::Io(e.to_string()))?;
        inner.meminfo_cache = Some(MeminfoCache { captured_at: Instant::now(), snapshot });
        Ok(snapshot)
    }

    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("MemPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.meminfo_cache = None;
        // RAM has no per-DIMM identity at this depth; per-DIMM data would
        // require DMI/dmidecode and is out of scope. The manifest emits no
        // `HardwareDevice` entries and each sensor leaves `device_key`
        // unset so the GUI groups memory sensors under a synthetic row.
        let sensors = vec![
            SensorDescriptor {
                id: SensorId::new("mem.used_bytes"),
                display_name: "Memory used".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Memory,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            },
            SensorDescriptor {
                id: SensorId::new("mem.total_bytes"),
                display_name: "Memory total".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Memory,
                native_rate_hz: 0.1,
                min: Some(0.0),
                max: None,
                device_id: None,
                device_key: None,
                // Installed RAM is fixed — sample once, no trend chart.
                tags: vec![linsight_plugin_sdk::STATIC_TAG.into()],
            },
            SensorDescriptor {
                id: SensorId::new("mem.swap_total_bytes"),
                display_name: "Swap total".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Memory,
                native_rate_hz: 0.1,
                min: Some(0.0),
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            },
            SensorDescriptor {
                id: SensorId::new("mem.swap_used_bytes"),
                display_name: "Swap used".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Memory,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            },
            SensorDescriptor {
                id: SensorId::new("mem.swap_cached_bytes"),
                display_name: "Swap cached".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Memory,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            },
        ];
        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.mem".into(),
            display_name: "Memory".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: vec![],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("MemPlugin poisoned");
        let m = Self::meminfo_snapshot(&mut inner)?;
        let v = match sensor.as_str() {
            "mem.used_bytes" => m.used_bytes() as f64,
            "mem.total_bytes" => m.total_bytes as f64,
            "mem.swap_total_bytes" => m.swap_total as f64,
            "mem.swap_used_bytes" => m.swap_used_bytes() as f64,
            "mem.swap_cached_bytes" => m.swap_cached as f64,
            _ => return Err(PluginError::Unsupported(sensor.to_string())),
        };
        Ok(Reading::Scalar(v))
    }
}

impl LinsightPlugin for MemPlugin {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;
    use std::time::Duration;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    fn fake_sysroot() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("proc")).unwrap();
        fs::write(
            dir.path().join("proc/meminfo"),
            "MemTotal: 1000 kB\nMemAvailable: 600 kB\nSwapTotal: 8000 kB\nSwapFree: 7500 kB\nSwapCached: 200 kB\n",
        )
        .unwrap();
        dir
    }

    fn write_meminfo(
        root: &std::path::Path,
        total_kb: u64,
        available_kb: u64,
        swap_total_kb: u64,
        swap_free_kb: u64,
        swap_cached_kb: u64,
    ) {
        fs::write(
            root.join("proc/meminfo"),
            format!(
                "MemTotal: {total_kb} kB\n\
                 MemAvailable: {available_kb} kB\n\
                 SwapTotal: {swap_total_kb} kB\n\
                 SwapFree: {swap_free_kb} kB\n\
                 SwapCached: {swap_cached_kb} kB\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn init_returns_five_sensors() {
        let p = MemPlugin::default();
        let dir = fake_sysroot();
        let m = host_init(&p, &PluginCtx::new_with_sysroot(dir.path().into()).unwrap()).unwrap();
        assert_eq!(m.sensors.len(), 5);
    }

    #[test]
    fn sample_used_bytes() {
        let p = MemPlugin::default();
        let dir = fake_sysroot();
        host_init(&p, &PluginCtx::new_with_sysroot(dir.path().into()).unwrap()).unwrap();
        let r = host_sample(&p, SensorId::new("mem.used_bytes")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == (1000 - 600) as f64 * 1024.0));
    }

    #[test]
    fn sample_swap_sensors() {
        let p = MemPlugin::default();
        let dir = fake_sysroot();
        host_init(&p, &PluginCtx::new_with_sysroot(dir.path().into()).unwrap()).unwrap();
        // SwapTotal: 8000 kB -> 8000 * 1024 bytes
        let total = host_sample(&p, SensorId::new("mem.swap_total_bytes")).unwrap();
        assert!(matches!(total, Reading::Scalar(v) if v == 8000.0 * 1024.0));
        // SwapFree: 7500 kB -> used = (8000 - 7500) * 1024 = 500 * 1024
        let used = host_sample(&p, SensorId::new("mem.swap_used_bytes")).unwrap();
        assert!(matches!(used, Reading::Scalar(v) if v == (8000.0 - 7500.0) * 1024.0));
        // SwapCached: 200 kB -> 200 * 1024 bytes
        let cached = host_sample(&p, SensorId::new("mem.swap_cached_bytes")).unwrap();
        assert!(matches!(cached, Reading::Scalar(v) if v == 200.0 * 1024.0));
    }

    #[test]
    fn samples_reuse_meminfo_snapshot_within_cache_window() {
        let p = MemPlugin::default();
        let dir = fake_sysroot();
        host_init(&p, &PluginCtx::new_with_sysroot(dir.path().into()).unwrap()).unwrap();

        let used = host_sample(&p, SensorId::new("mem.used_bytes")).unwrap();
        assert!(matches!(used, Reading::Scalar(v) if v == 400.0 * 1024.0));

        write_meminfo(dir.path(), 2000, 100, 9000, 7000, 300);

        let cached_total = host_sample(&p, SensorId::new("mem.total_bytes")).unwrap();
        assert!(matches!(cached_total, Reading::Scalar(v) if v == 1000.0 * 1024.0));

        thread::sleep(Duration::from_millis(75));
        let refreshed_total = host_sample(&p, SensorId::new("mem.total_bytes")).unwrap();
        assert!(matches!(refreshed_total, Reading::Scalar(v) if v == 2000.0 * 1024.0));
    }

    #[test]
    fn init_clears_cached_meminfo_snapshot() {
        let p = MemPlugin::default();
        let dir1 = fake_sysroot();
        host_init(&p, &PluginCtx::new_with_sysroot(dir1.path().into()).unwrap()).unwrap();
        let used = host_sample(&p, SensorId::new("mem.used_bytes")).unwrap();
        assert!(matches!(used, Reading::Scalar(v) if v == 400.0 * 1024.0));

        let dir2 = fake_sysroot();
        write_meminfo(dir2.path(), 2000, 100, 9000, 7000, 300);
        host_init(&p, &PluginCtx::new_with_sysroot(dir2.path().into()).unwrap()).unwrap();

        let used = host_sample(&p, SensorId::new("mem.used_bytes")).unwrap();
        assert!(matches!(used, Reading::Scalar(v) if v == 1900.0 * 1024.0));
    }

    #[test]
    fn manifest_emits_no_devices() {
        let plugin = MemPlugin::default();
        let ctx = PluginCtx::default();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert!(manifest.devices.is_empty());
        for s in &manifest.sensors {
            assert!(s.device_key.is_none());
        }
    }

    #[test]
    fn sample_unknown_errors() {
        let p = MemPlugin::default();
        let dir = fake_sysroot();
        host_init(&p, &PluginCtx::new_with_sysroot(dir.path().into()).unwrap()).unwrap();
        assert!(host_sample(&p, SensorId::new("nope")).is_err());
    }
}
