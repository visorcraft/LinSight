// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Block device I/O sensor backend.
//!
//! Per physical block device in `/sys/class/block/*`:
//! * `disk.<name>.bytes_read` — cumulative bytes read (Counter)
//! * `disk.<name>.bytes_written` — cumulative bytes written (Counter)
//! * `disk.<name>.iops_read` — cumulative read operations (Counter)
//! * `disk.<name>.iops_written` — cumulative write operations (Counter)
//! * `disk.<name>.io_util_ms` — cumulative I/O time in ms (Counter)
//!
//! Skips virtual devices: loop, dm-, md, zram, nvme (covered by nvme plugin),
//! and ram.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, STATIC_TAG, SensorDescriptor,
};

/// Virtual/software device prefixes to skip.
const VIRTUAL_PREFIXES: &[&str] = &["loop", "dm-", "md", "zram", "ram"];
const CACHE_TTL: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct DiskPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    devices: Vec<DiskDevice>,
    cache: Option<linsight_core::SnapshotCache<HashMap<String, BlockStat>>>,
}

#[derive(Clone, Debug)]
struct DiskDevice {
    name: String,
    stat_path: PathBuf,
    capacity_bytes: u64,
}

impl DiskPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("DiskPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        let extra_exclude = parse_string_array(ctx.config(), "exclude_devices");
        inner.devices = enumerate(ctx.sysroot(), &extra_exclude);

        let mut sensors = Vec::with_capacity(inner.devices.len() * 6);
        let mut hw_devices: Vec<HardwareDevice> = Vec::with_capacity(inner.devices.len());
        for dev in &inner.devices {
            let key = HardwareDeviceKey::try_new(format!("block:{}", dev.name))
                .map_err(|e| PluginError::Io(format!("block {} bad key: {e}", dev.name)))?;
            hw_devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Storage,
                model: dev.name.clone(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: dev.name.clone(),
                sensor_ids: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("disk.{}.capacity_bytes", dev.name)),
                display_name: "Disk capacity".into(),
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
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("disk.{}.bytes_read", dev.name)),
                display_name: "Disk bytes read".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Counter,
                category: Category::Storage,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("disk.{}.bytes_written", dev.name)),
                display_name: "Disk bytes written".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Counter,
                category: Category::Storage,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("disk.{}.iops_read", dev.name)),
                display_name: "Disk read operations".into(),
                unit: Unit::Count,
                kind: SensorKind::Counter,
                category: Category::Storage,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("disk.{}.iops_written", dev.name)),
                display_name: "Disk write operations".into(),
                unit: Unit::Count,
                kind: SensorKind::Counter,
                category: Category::Storage,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("disk.{}.io_util_ms", dev.name)),
                display_name: "Disk I/O time".into(),
                unit: Unit::Custom("ms".into()),
                kind: SensorKind::Counter,
                category: Category::Storage,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
        }

        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.disk".into(),
            display_name: "Disk I/O".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: hw_devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("DiskPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("disk.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (name, metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let dev = inner
            .devices
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| PluginError::Unsupported(id.into()))?;

        if metric == "capacity_bytes" {
            return Ok(Reading::Scalar(dev.capacity_bytes as f64));
        }

        let stats = Self::snapshot(&mut inner)?;
        let stat = stats
            .get(name)
            .ok_or_else(|| PluginError::Unsupported(format!("disk.{name} not in snapshot")))?;
        let value = match metric {
            "bytes_read" => Reading::Counter(stat.sectors_read.saturating_mul(512)),
            "bytes_written" => Reading::Counter(stat.sectors_written.saturating_mul(512)),
            "iops_read" => Reading::Counter(stat.reads_completed),
            "iops_written" => Reading::Counter(stat.writes_completed),
            "io_util_ms" => Reading::Counter(stat.io_ticks),
            _ => return Err(PluginError::Unsupported(id.into())),
        };
        Ok(value)
    }

    fn snapshot(inner: &mut Inner) -> Result<Arc<HashMap<String, BlockStat>>, PluginError> {
        if let Some(cache) = &inner.cache
            && let Some(stats) = cache.get(CACHE_TTL)
        {
            return Ok(stats);
        }

        let mut stats = HashMap::with_capacity(inner.devices.len());
        let mut files_read = 0usize;
        for dev in &inner.devices {
            if let Ok(stat) = read_block_stat(&dev.stat_path) {
                stats.insert(dev.name.clone(), stat);
                files_read += 1;
            }
        }
        tracing::debug!(target: "linsight_sensors::reads", plugin = "disk", files_read);
        let stats = Arc::new(stats);
        inner.cache = Some(linsight_core::SnapshotCache::new(Arc::clone(&stats)));
        Ok(stats)
    }
}

impl LinsightPlugin for DiskPlugin {
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

#[derive(Clone)]
struct BlockStat {
    reads_completed: u64,
    writes_completed: u64,
    sectors_read: u64,
    sectors_written: u64,
    io_ticks: u64,
}

/// Read a small sysfs file into a stack buffer. Avoids the `String`
/// allocation of `fs::read_to_string` for files that are typically a
/// single line. Reads up to 512 bytes; truncates anything larger.
fn read_small_file(path: &Path) -> Result<String, PluginError> {
    use std::io::Read;
    let mut file =
        fs::File::open(path).map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    let mut buf = [0u8; 512];
    let mut n = 0usize;
    loop {
        match file.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(m) => n += m,
            Err(e) => return Err(PluginError::Io(format!("{}: {e}", path.display()))),
        }
        if n == buf.len() {
            break;
        }
    }
    std::str::from_utf8(&buf[..n])
        .map(|s| s.to_owned())
        .map_err(|e| PluginError::Parse(format!("{}: {e}", path.display())))
}

fn read_block_stat(path: &Path) -> Result<BlockStat, PluginError> {
    let s = read_small_file(path)?;
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() < 11 {
        return Err(PluginError::Parse(format!(
            "{}: expected ≥11 fields, got {}",
            path.display(),
            fields.len()
        )));
    }
    let parse = |idx: usize| -> Result<u64, PluginError> {
        fields[idx]
            .parse::<u64>()
            .map_err(|e| PluginError::Parse(format!("{} field {}: {e}", path.display(), idx + 1)))
    };
    Ok(BlockStat {
        reads_completed: parse(0)?,
        writes_completed: parse(4)?,
        sectors_read: parse(2)?,
        sectors_written: parse(6)?,
        io_ticks: parse(9)?,
    })
}

fn enumerate(sysroot: Option<&Path>, extra_exclude: &[String]) -> Vec<DiskDevice> {
    let root = match sysroot {
        Some(r) => r.join("sys/class/block"),
        None => PathBuf::from("/sys/class/block"),
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return vec![];
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        if extra_exclude.iter().any(|p| {
            let trimmed = p.trim_end_matches('*');
            !trimmed.is_empty() && name.starts_with(trimmed)
        }) {
            continue;
        }
        if name.starts_with("nvme") {
            continue;
        }
        let stat_path = entry.path().join("stat");
        if !stat_path.exists() {
            continue;
        }

        let size_path = entry.path().join("size");
        let capacity_bytes = if let Ok(s) = read_small_file(&size_path) {
            s.trim().parse::<u64>().map(|sectors| sectors.saturating_mul(512)).unwrap_or(0)
        } else {
            0
        };

        out.push(DiskDevice { name, stat_path, capacity_bytes });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn parse_string_array(config: &serde_json::Value, key: &str) -> Vec<String> {
    match config.get(key) {
        Some(serde_json::Value::Array(arr)) => {
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    fn fake_sysroot(devices: &[(&str, &str)]) -> tempfile::TempDir {
        let devices: Vec<(&str, &str, Option<&str>)> =
            devices.iter().map(|(name, stat_content)| (*name, *stat_content, None)).collect();
        fake_sysroot_with_sizes(&devices)
    }

    fn fake_sysroot_with_sizes(devices: &[(&str, &str, Option<&str>)]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        for (name, stat_content, size_content) in devices {
            let p = dir.path().join("sys/class/block").join(name);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("stat"), stat_content).unwrap();
            if let Some(size_content) = size_content {
                fs::write(p.join("size"), size_content).unwrap();
            }
        }
        dir
    }

    #[test]
    fn enumerate_skips_virtual() {
        let dir = fake_sysroot(&[
            ("sda", "1 2 3 4 5 6 7 8 9 10 11"),
            ("loop0", "0 0 0 0 0 0 0 0 0 0 0"),
            ("dm-0", "0 0 0 0 0 0 0 0 0 0 0"),
            ("md0", "0 0 0 0 0 0 0 0 0 0 0"),
            ("nvme0n1", "0 0 0 0 0 0 0 0 0 0 0"),
            ("zram0", "0 0 0 0 0 0 0 0 0 0 0"),
        ]);
        let devs = enumerate(Some(dir.path()), &[]);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "sda");
    }

    #[test]
    fn init_advertises_six_sensors_per_device() {
        let dir = fake_sysroot(&[("sda", "100 200 300 400 500 600 700 800 0 900 1000")]);
        let plugin = DiskPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        // sda + nvme0n1 filtered out → only sda
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 6);
        assert!(ids.contains(&"disk.sda.capacity_bytes"));
        assert!(ids.contains(&"disk.sda.bytes_read"));
        assert!(ids.contains(&"disk.sda.bytes_written"));
        assert!(ids.contains(&"disk.sda.iops_read"));
        assert!(ids.contains(&"disk.sda.iops_written"));
        assert!(ids.contains(&"disk.sda.io_util_ms"));
    }

    #[test]
    fn capacity_bytes_saturates_extreme_sector_count() {
        let max_sectors = u64::MAX.to_string();
        let dir = fake_sysroot_with_sizes(&[(
            "sda",
            "0 0 0 0 0 0 0 0 0 0 0",
            Some(max_sectors.as_str()),
        )]);

        let devs = enumerate(Some(dir.path()), &[]);

        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].capacity_bytes, u64::MAX);
    }

    #[test]
    fn sample_disk_sensors() {
        // stat fields: reads=100, read_merged=200, sectors_read=300 (3 sectors),
        // read_ticks=400, writes=500, write_merged=600, sectors_written=700,
        // write_ticks=800, in_progress=0, io_ticks=900, weighted_io_ticks=1000
        let dir = fake_sysroot(&[("sda", "100 200 300 400 500 600 700 800 0 900 1000")]);
        let plugin = DiskPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        // bytes_read = 300 sectors * 512 = 153600
        let r = host_sample(&plugin, &SensorId::new("disk.sda.bytes_read")).unwrap();
        assert!(matches!(r, Reading::Counter(153600)));

        // bytes_written = 700 * 512 = 358400
        let r = host_sample(&plugin, &SensorId::new("disk.sda.bytes_written")).unwrap();
        assert!(matches!(r, Reading::Counter(358400)));

        // iops_read = 100
        let r = host_sample(&plugin, &SensorId::new("disk.sda.iops_read")).unwrap();
        assert!(matches!(r, Reading::Counter(100)));

        // iops_written = 500
        let r = host_sample(&plugin, &SensorId::new("disk.sda.iops_written")).unwrap();
        assert!(matches!(r, Reading::Counter(500)));

        // io_util_ms = 900
        let r = host_sample(&plugin, &SensorId::new("disk.sda.io_util_ms")).unwrap();
        assert!(matches!(r, Reading::Counter(900)));
    }

    #[test]
    fn manifest_emits_block_devices() {
        let dir =
            fake_sysroot(&[("sda", "0 0 0 0 0 0 0 0 0 0 0"), ("sdb", "0 0 0 0 0 0 0 0 0 0 0")]);
        let plugin = DiskPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 2);
        for dev in &manifest.devices {
            assert!(dev.key.as_str().starts_with("block:"));
        }
    }

    #[test]
    fn enumerate_respects_exclude_devices() {
        let dir =
            fake_sysroot(&[("sda", "1 2 3 4 5 6 7 8 9 10 11"), ("sdb", "1 2 3 4 5 6 7 8 9 10 11")]);
        let devs = enumerate(Some(dir.path()), &["sd".into()]);
        assert!(devs.is_empty(), "all devices should be excluded");
    }

    #[test]
    fn cache_reuses_snapshot_within_ttl() {
        let dir = fake_sysroot(&[("sda", "100 200 300 400 500 600 700 800 0 900 1000")]);
        let plugin = DiskPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        // First sample populates cache
        let r1 = host_sample(&plugin, &SensorId::new("disk.sda.bytes_read")).unwrap();
        assert!(matches!(r1, Reading::Counter(153600)));

        // Mutate the stat file on disk
        let stat_path = dir.path().join("sys/class/block/sda/stat");
        fs::write(&stat_path, "999 200 999 400 500 600 700 800 0 900 1000").unwrap();

        // Second sample immediately should still see cached value
        let r2 = host_sample(&plugin, &SensorId::new("disk.sda.bytes_read")).unwrap();
        assert!(matches!(r2, Reading::Counter(153600)), "cache should serve stale value");
    }

    #[test]
    fn cache_expires_after_ttl() {
        let dir = fake_sysroot(&[("sda", "100 200 300 400 500 600 700 800 0 900 1000")]);
        let plugin = DiskPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        // First sample
        let r1 = host_sample(&plugin, &SensorId::new("disk.sda.bytes_read")).unwrap();
        assert!(matches!(r1, Reading::Counter(153600)));

        // Mutate the stat file
        let stat_path = dir.path().join("sys/class/block/sda/stat");
        fs::write(&stat_path, "999 200 999 400 500 600 700 800 0 900 1000").unwrap();

        // Wait for cache expiry
        std::thread::sleep(std::time::Duration::from_millis(60));

        // Second sample should see new value
        let r2 = host_sample(&plugin, &SensorId::new("disk.sda.bytes_read")).unwrap();
        assert!(
            matches!(r2, Reading::Counter(511488)),
            "cache should reflect new value after expiry"
        );
    }
}
