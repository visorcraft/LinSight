// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

const CACHE_TTL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_u64(path: &Path) -> Result<u64, PluginError> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    s.trim().parse::<u64>().map_err(|e| PluginError::Parse(format!("{}: {e}", path.display())))
}

fn read_line(path: &Path) -> Result<String, PluginError> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    Ok(s.trim().to_owned())
}

/// Parse `/proc/loadavg`: 5 whitespace-separated fields.
/// Fields: 1m 5m 15m running/total last_pid
#[derive(Clone)]
struct LoadAvg {
    load_1m: f64,
    load_5m: f64,
    load_15m: f64,
    procs_running: u64,
    procs_total: u64,
}

fn read_loadavg(sysroot: Option<&Path>) -> Result<LoadAvg, PluginError> {
    let path = match sysroot {
        Some(r) => r.join("proc/loadavg"),
        None => PathBuf::from("/proc/loadavg"),
    };
    let s = read_line(&path)?;
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() < 5 {
        return Err(PluginError::Parse(format!(
            "{}: expected 5 whitespace fields, got {}",
            path.display(),
            fields.len()
        )));
    }
    let load_1m = fields[0]
        .parse::<f64>()
        .map_err(|e| PluginError::Parse(format!("{} field 0: {e}", path.display())))?;
    let load_5m = fields[1]
        .parse::<f64>()
        .map_err(|e| PluginError::Parse(format!("{} field 1: {e}", path.display())))?;
    let load_15m = fields[2]
        .parse::<f64>()
        .map_err(|e| PluginError::Parse(format!("{} field 2: {e}", path.display())))?;
    let procs_part = fields[3];
    let (running, total) = procs_part
        .split_once('/')
        .and_then(|(a, b)| {
            let r = a.parse::<u64>().ok();
            let t = b.parse::<u64>().ok();
            r.zip(t)
        })
        .ok_or_else(|| PluginError::Parse(format!("{} field 3: {procs_part}", path.display())))?;
    Ok(LoadAvg { load_1m, load_5m, load_15m, procs_running: running, procs_total: total })
}

/// Parse `/proc/uptime`: 2 whitespace fields, first is uptime in seconds as float.
fn read_uptime(sysroot: Option<&Path>) -> Result<f64, PluginError> {
    let path = match sysroot {
        Some(r) => r.join("proc/uptime"),
        None => PathBuf::from("/proc/uptime"),
    };
    let s = read_line(&path)?;
    let uptime_str = s
        .split_whitespace()
        .next()
        .ok_or_else(|| PluginError::Parse(format!("{}: empty", path.display())))?;
    uptime_str.parse::<f64>().map_err(|e| PluginError::Parse(format!("{}: {e}", path.display())))
}

/// Parse a PSI file and return the "some" avg values. Returns (avg10, avg60, avg300).
fn read_psi_some(sysroot: Option<&Path>, resource: &str) -> Result<(f64, f64, f64), PluginError> {
    let path = match sysroot {
        Some(r) => r.join(format!("proc/pressure/{resource}")),
        None => PathBuf::from(format!("/proc/pressure/{resource}")),
    };
    let s = read_line(&path)?;

    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("some ") {
            return parse_psi_avgs(rest)
                .ok_or_else(|| PluginError::Parse(format!("{resource} some malformed")));
        }
    }
    Err(PluginError::Parse(format!("{resource}: no 'some' line")))
}

/// Parse a PSI file and return the "full" avg values. Returns (avg10, avg60, avg300).
fn read_psi_full(sysroot: Option<&Path>, resource: &str) -> Result<(f64, f64, f64), PluginError> {
    let path = match sysroot {
        Some(r) => r.join(format!("proc/pressure/{resource}")),
        None => PathBuf::from(format!("/proc/pressure/{resource}")),
    };
    let s = read_line(&path)?;

    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("full ") {
            return parse_psi_avgs(rest)
                .ok_or_else(|| PluginError::Parse(format!("{resource} full malformed")));
        }
    }
    Err(PluginError::Parse(format!("{resource}: no 'full' line")))
}

fn parse_psi_avgs(input: &str) -> Option<(f64, f64, f64)> {
    let avg10 = input
        .split_whitespace()
        .find_map(|t| t.strip_prefix("avg10="))
        .and_then(|v| v.parse::<f64>().ok())?;
    let avg60 = input
        .split_whitespace()
        .find_map(|t| t.strip_prefix("avg60="))
        .and_then(|v| v.parse::<f64>().ok())?;
    let avg300 = input
        .split_whitespace()
        .find_map(|t| t.strip_prefix("avg300="))
        .and_then(|v| v.parse::<f64>().ok())?;
    Some((avg10, avg60, avg300))
}

fn read_ctxt(sysroot: Option<&Path>) -> Result<u64, PluginError> {
    let path = match sysroot {
        Some(r) => r.join("proc/stat"),
        None => PathBuf::from("/proc/stat"),
    };
    let s = std::fs::read_to_string(&path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("ctxt ") {
            return rest
                .trim()
                .parse::<u64>()
                .map_err(|e| PluginError::Parse(format!("ctxt: {e}")));
        }
    }
    Err(PluginError::Parse(format!("{}: missing ctxt line", path.display())))
}

fn read_processes(sysroot: Option<&Path>) -> Result<u64, PluginError> {
    let path = match sysroot {
        Some(r) => r.join("proc/stat"),
        None => PathBuf::from("/proc/stat"),
    };
    let s = std::fs::read_to_string(&path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("processes ") {
            return rest
                .trim()
                .parse::<u64>()
                .map_err(|e| PluginError::Parse(format!("processes: {e}")));
        }
    }
    Err(PluginError::Parse(format!("{}: missing processes line", path.display())))
}

/// Enumerate thermal zones at init, cache their type+path pairs.
#[derive(Clone)]
struct ThermalZone {
    label: String,
    safe_label: String,
    temp_path: PathBuf,
}

fn enumerate_thermal_zones(sysroot: Option<&Path>) -> Vec<ThermalZone> {
    let tz_root = match sysroot {
        Some(r) => r.join("sys/class/thermal"),
        None => PathBuf::from("/sys/class/thermal"),
    };
    let Ok(entries) = std::fs::read_dir(&tz_root) else {
        return vec![];
    };
    // First pass: enumerate raw zones with their numeric index so we can
    // produce a stable, deterministic order regardless of readdir ordering.
    struct Raw {
        zone_index: u32,
        label: String,
        base_safe_label: String,
        temp_path: PathBuf,
    }
    let mut raw = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(idx_str) = name.strip_prefix("thermal_zone") else {
            continue;
        };
        let Ok(zone_index) = idx_str.parse::<u32>() else {
            continue;
        };
        let label = match std::fs::read_to_string(p.join("type")) {
            Ok(s) => s.trim().to_owned(),
            Err(_) => continue,
        };
        let base_safe_label: String =
            label
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' }
                })
                .collect();
        let temp_path = p.join("temp");
        if !temp_path.exists() {
            continue;
        }
        raw.push(Raw { zone_index, label, base_safe_label, temp_path });
    }
    raw.sort_by_key(|r| r.zone_index);
    // Second pass: disambiguate duplicate base_safe_label values (common on
    // Intel laptops, which expose multiple thermal_zoneN with type="acpitz").
    // First occurrence of each base label keeps the bare name (backward
    // compatible for the single-zone-per-type common case); subsequent
    // occurrences get the zone's numeric index appended.
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut zones = Vec::with_capacity(raw.len());
    for r in raw {
        let safe_label = if taken.insert(r.base_safe_label.clone()) {
            r.base_safe_label
        } else {
            format!("{}_{}", r.base_safe_label, r.zone_index)
        };
        zones.push(ThermalZone { label: r.label, safe_label, temp_path: r.temp_path });
    }
    zones
}

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct SystemPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    thermal_zones: Option<Vec<ThermalZone>>,
    cache: Option<linsight_core::SnapshotCache<SystemSnapshot>>,
}

#[derive(Clone)]
struct SystemSnapshot {
    loadavg: LoadAvg,
    uptime: f64,
    ctxt: u64,
    processes: u64,
    entropy: u64,
    psi_cpu: (f64, f64, f64),
    psi_mem_some: (f64, f64, f64),
    psi_mem_full: (f64, f64, f64),
    psi_io_some: (f64, f64, f64),
    psi_io_full: (f64, f64, f64),
    thermal: HashMap<String, f64>,
}

impl SystemPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("SystemPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.thermal_zones = None;

        let sys_key = HardwareDeviceKey::try_new("system:0").expect("static key");
        let device = HardwareDevice {
            key: sys_key.clone(),
            category: HardwareCategory::Other,
            model: "System".into(),
            vendor: None,
            location: None,
            plugin_id: String::new(),
            plugin_device_id: "system".into(),
            sensor_ids: vec![],
        };

        let mut sensors: Vec<SensorDescriptor> = Vec::new();

        // --- A2: System load / uptime / processes / entropy ---
        let sys_info = |id: &str,
                        name: &str,
                        unit: Unit,
                        kind: SensorKind,
                        rate: f32,
                        min: Option<f64>,
                        max: Option<f64>| {
            SensorDescriptor {
                id: SensorId::new(id),
                display_name: name.into(),
                unit,
                kind,
                category: Category::Custom,
                native_rate_hz: rate,
                min,
                max,
                device_id: None,
                device_key: Some(sys_key.clone()),
                tags: vec![],
            }
        };

        sensors.push(sys_info(
            "system.load_1m",
            "System load (1 min)",
            Unit::Count,
            SensorKind::Scalar,
            0.5,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.load_5m",
            "System load (5 min)",
            Unit::Count,
            SensorKind::Scalar,
            0.5,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.load_15m",
            "System load (15 min)",
            Unit::Count,
            SensorKind::Scalar,
            0.5,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.procs_running",
            "Running processes",
            Unit::Count,
            SensorKind::Scalar,
            0.5,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.procs_total",
            "Total processes",
            Unit::Count,
            SensorKind::Scalar,
            0.5,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.uptime_secs",
            "System uptime",
            Unit::Custom("s".into()),
            SensorKind::Scalar,
            0.2,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.ctxt_switches",
            "Context switches",
            Unit::Count,
            SensorKind::Counter,
            1.0,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.procs_created",
            "Processes created",
            Unit::Count,
            SensorKind::Counter,
            1.0,
            Some(0.0),
            None,
        ));
        sensors.push(sys_info(
            "system.entropy_bits",
            "Entropy available",
            Unit::Custom("bits".into()),
            SensorKind::Scalar,
            0.5,
            Some(0.0),
            None,
        ));

        // --- B5: PSI sensors ---
        sensors.push(sys_info(
            "psi.cpu_some_10",
            "CPU pressure (some 10s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));
        sensors.push(sys_info(
            "psi.cpu_some_60",
            "CPU pressure (some 60s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));
        sensors.push(sys_info(
            "psi.cpu_some_300",
            "CPU pressure (some 300s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));
        sensors.push(sys_info(
            "psi.mem_some_10",
            "Memory pressure (some 10s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));
        sensors.push(sys_info(
            "psi.mem_full_10",
            "Memory pressure (full 10s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));
        sensors.push(sys_info(
            "psi.io_some_10",
            "I/O pressure (some 10s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));
        sensors.push(sys_info(
            "psi.io_full_10",
            "I/O pressure (full 10s)",
            Unit::Percent,
            SensorKind::Scalar,
            1.0,
            Some(0.0),
            Some(100.0),
        ));

        // --- A6: Thermal zones (enumerated at init) ---
        let zones = enumerate_thermal_zones(inner.sysroot.as_deref());
        inner.thermal_zones = Some(zones.clone());
        for zone in &zones {
            sensors.push(sys_info(
                &format!("thermal.{}.temp_c", zone.safe_label),
                &format!("Thermal zone: {}", zone.label),
                Unit::Celsius,
                SensorKind::Scalar,
                0.5,
                None,
                None,
            ));
        }

        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.system".into(),
            display_name: "System".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: vec![device],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("SystemPlugin poisoned");
        let id = sensor.as_str();
        let snap = Self::snapshot(&mut inner)?;

        match id {
            // --- A2: loadavg ---
            "system.load_1m" => Ok(Reading::Scalar(snap.loadavg.load_1m)),
            "system.load_5m" => Ok(Reading::Scalar(snap.loadavg.load_5m)),
            "system.load_15m" => Ok(Reading::Scalar(snap.loadavg.load_15m)),
            "system.procs_running" => Ok(Reading::Scalar(snap.loadavg.procs_running as f64)),
            "system.procs_total" => Ok(Reading::Scalar(snap.loadavg.procs_total as f64)),

            "system.uptime_secs" => Ok(Reading::Scalar(snap.uptime)),

            "system.ctxt_switches" => Ok(Reading::Counter(snap.ctxt)),

            "system.procs_created" => Ok(Reading::Counter(snap.processes)),

            "system.entropy_bits" => Ok(Reading::Scalar(snap.entropy as f64)),

            // --- B5: PSI ---
            "psi.cpu_some_10" => Ok(Reading::Scalar(snap.psi_cpu.0)),
            "psi.cpu_some_60" => Ok(Reading::Scalar(snap.psi_cpu.1)),
            "psi.cpu_some_300" => Ok(Reading::Scalar(snap.psi_cpu.2)),
            "psi.mem_some_10" => Ok(Reading::Scalar(snap.psi_mem_some.0)),
            "psi.mem_full_10" => Ok(Reading::Scalar(snap.psi_mem_full.0)),
            "psi.io_some_10" => Ok(Reading::Scalar(snap.psi_io_some.0)),
            "psi.io_full_10" => Ok(Reading::Scalar(snap.psi_io_full.0)),

            // --- A6: thermal zones ---
            _ if id.starts_with("thermal.") && id.ends_with(".temp_c") => {
                let label = &id["thermal.".len()..id.len() - ".temp_c".len()];
                let temp = snap
                    .thermal
                    .get(label)
                    .copied()
                    .ok_or_else(|| PluginError::Unsupported(id.to_string()))?;
                Ok(Reading::Scalar(temp))
            }

            _ => Err(PluginError::Unsupported(id.to_string())),
        }
    }

    fn snapshot(inner: &mut Inner) -> Result<SystemSnapshot, PluginError> {
        if let Some(cache) = &inner.cache
            && let Some(snap) = cache.get(CACHE_TTL)
        {
            return Ok(snap);
        }

        let sysroot = inner.sysroot.as_deref();
        let loadavg = read_loadavg(sysroot)?;
        let uptime = read_uptime(sysroot)?;
        let ctxt = read_ctxt(sysroot)?;
        let processes = read_processes(sysroot)?;

        let entropy_path = match &inner.sysroot {
            Some(r) => r.join("proc/sys/kernel/random/entropy_avail"),
            None => PathBuf::from("/proc/sys/kernel/random/entropy_avail"),
        };
        let entropy = read_u64(&entropy_path).unwrap_or(0);

        let psi_cpu = read_psi_some(sysroot, "cpu").unwrap_or((0.0, 0.0, 0.0));
        let psi_mem_some = read_psi_some(sysroot, "memory").unwrap_or((0.0, 0.0, 0.0));
        let psi_mem_full = read_psi_full(sysroot, "memory").unwrap_or((0.0, 0.0, 0.0));
        let psi_io_some = read_psi_some(sysroot, "io").unwrap_or((0.0, 0.0, 0.0));
        let psi_io_full = read_psi_full(sysroot, "io").unwrap_or((0.0, 0.0, 0.0));

        let mut thermal = HashMap::new();
        let mut files_read = 0usize;
        if let Some(zones) = &inner.thermal_zones {
            for zone in zones {
                if let Ok(milli) = read_u64(&zone.temp_path) {
                    thermal.insert(zone.safe_label.clone(), milli as f64 / 1000.0);
                    files_read += 1;
                }
            }
        }

        let snapshot = SystemSnapshot {
            loadavg,
            uptime,
            ctxt,
            processes,
            entropy,
            psi_cpu,
            psi_mem_some,
            psi_mem_full,
            psi_io_some,
            psi_io_full,
            thermal,
        };
        tracing::debug!(target: "linsight_sensors::reads", plugin = "system", files_read);
        inner.cache = Some(linsight_core::SnapshotCache::new(snapshot.clone()));
        Ok(snapshot)
    }
}

impl LinsightPlugin for SystemPlugin {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    /// Build a fake sysroot with /proc and /sys fixtures for the system plugin.
    fn fake_sysroot() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();

        // /proc/loadavg
        fs::create_dir_all(dir.path().join("proc")).unwrap();
        fs::write(dir.path().join("proc/loadavg"), "1.23 0.89 0.45 3/456 78901\n").unwrap();

        // /proc/uptime
        fs::write(dir.path().join("proc/uptime"), "123456.78 987654.32\n").unwrap();

        // /proc/stat with ctxt and processes
        fs::write(
            dir.path().join("proc/stat"),
            "cpu  100 0 50 1000 0 0 0 0 0 0\nintr 1234567\nctxt 420000\nbtime 1700000000\nprocesses 50000\nprocs_running 1\nprocs_blocked 0\n",
        )
        .unwrap();

        // /proc/sys/kernel/random/entropy_avail
        fs::create_dir_all(dir.path().join("proc/sys/kernel/random")).unwrap();
        fs::write(dir.path().join("proc/sys/kernel/random/entropy_avail"), "256\n").unwrap();

        // /proc/pressure/{cpu,memory,io}
        fs::create_dir_all(dir.path().join("proc/pressure")).unwrap();
        fs::write(
            dir.path().join("proc/pressure/cpu"),
            "some avg10=0.42 avg60=0.31 avg300=0.15 total=1234\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("proc/pressure/memory"),
            "some avg10=1.23 avg60=0.98 avg300=0.50 total=5000\nfull avg10=0.10 avg60=0.05 avg300=0.01 total=100\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("proc/pressure/io"),
            "some avg10=2.50 avg60=1.75 avg300=0.80 total=9999\nfull avg10=0.90 avg60=0.60 avg300=0.30 total=5555\n",
        )
        .unwrap();

        // /sys/class/thermal/thermal_zone0/
        fs::create_dir_all(dir.path().join("sys/class/thermal/thermal_zone0")).unwrap();
        fs::write(dir.path().join("sys/class/thermal/thermal_zone0/type"), "x86_pkg_temp\n")
            .unwrap();
        fs::write(dir.path().join("sys/class/thermal/thermal_zone0/temp"), "55000\n").unwrap();

        dir
    }

    #[test]
    fn manifest_has_system_device() {
        let p = SystemPlugin::default();
        let ctx = PluginCtx::default();
        let m = host_init(&p, &ctx).unwrap();
        let dev = m.devices.iter().find(|d| d.key.as_str() == "system:0").expect("system:0 device");
        assert_eq!(dev.category, linsight_core::HardwareCategory::Other);
    }

    #[test]
    fn manifest_advertises_system_sensors() {
        let p = SystemPlugin::default();
        let ctx = PluginCtx::default();
        let m = host_init(&p, &ctx).unwrap();
        let ids: Vec<_> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"system.load_1m"));
        assert!(ids.contains(&"system.uptime_secs"));
        assert!(ids.contains(&"system.ctxt_switches"));
        assert!(ids.contains(&"system.procs_created"));
        assert!(ids.contains(&"system.entropy_bits"));
    }

    #[test]
    fn manifest_advertises_psi_sensors() {
        let p = SystemPlugin::default();
        let ctx = PluginCtx::default();
        let m = host_init(&p, &ctx).unwrap();
        let ids: Vec<_> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"psi.cpu_some_10"));
        assert!(ids.contains(&"psi.mem_some_10"));
        assert!(ids.contains(&"psi.mem_full_10"));
        assert!(ids.contains(&"psi.io_some_10"));
        assert!(ids.contains(&"psi.io_full_10"));
    }

    #[test]
    fn manifest_advertises_thermal_zones() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        let ids: Vec<_> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"thermal.x86_pkg_temp.temp_c"));
    }

    #[test]
    fn thermal_zones_with_duplicate_types_get_distinct_ids() {
        // Intel laptops commonly expose multiple thermal zones with
        // type="acpitz". The plugin must surface all of them with unique
        // sensor IDs; otherwise the duplicate-sensor-id check in the
        // daemon drops them on the floor.
        let dir = fake_sysroot();
        for n in 1..=2 {
            let zone = dir.path().join(format!("sys/class/thermal/thermal_zone{n}"));
            fs::create_dir_all(&zone).unwrap();
            fs::write(zone.join("type"), "acpitz\n").unwrap();
            fs::write(zone.join("temp"), format!("{}\n", 40000 + n * 1000)).unwrap();
        }
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        let acpitz_ids: Vec<_> = m
            .sensors
            .iter()
            .map(|s| s.id.as_str().to_owned())
            .filter(|id| id.starts_with("thermal.acpitz"))
            .collect();
        assert_eq!(
            acpitz_ids.len(),
            2,
            "expected 2 distinct acpitz sensor ids, got: {acpitz_ids:?}"
        );
        assert_eq!(
            acpitz_ids.iter().collect::<std::collections::HashSet<_>>().len(),
            2,
            "ids must be unique: {acpitz_ids:?}"
        );
    }

    #[test]
    fn sample_load_1m() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.load_1m")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 1.23).abs() < 1e-6));
    }

    #[test]
    fn sample_load_5m() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.load_5m")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 0.89).abs() < 1e-6));
    }

    #[test]
    fn sample_procs_running_and_total() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.procs_running")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 3.0));
        let t = host_sample(&p, SensorId::new("system.procs_total")).unwrap();
        assert!(matches!(t, Reading::Scalar(v) if v == 456.0));
    }

    #[test]
    fn sample_uptime() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.uptime_secs")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 123456.78).abs() < 1e-6));
    }

    #[test]
    fn sample_ctxt() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.ctxt_switches")).unwrap();
        assert!(matches!(r, Reading::Counter(420000)));
    }

    #[test]
    fn sample_procs_created() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.procs_created")).unwrap();
        assert!(matches!(r, Reading::Counter(50000)));
    }

    #[test]
    fn sample_entropy() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("system.entropy_bits")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 256.0));
    }

    #[test]
    fn sample_psi_cpu_some_10() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("psi.cpu_some_10")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 0.42).abs() < 1e-6));
    }

    #[test]
    fn sample_psi_mem_some_10() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("psi.mem_some_10")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 1.23).abs() < 1e-6));
    }

    #[test]
    fn sample_psi_mem_full_10() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("psi.mem_full_10")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 0.10).abs() < 1e-6));
    }

    #[test]
    fn sample_psi_io_some_10() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("psi.io_some_10")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 2.50).abs() < 1e-6));
    }

    #[test]
    fn sample_thermal_temp() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let r = host_sample(&p, SensorId::new("thermal.x86_pkg_temp.temp_c")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 55.0).abs() < 1e-6));
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        let err = host_sample(&p, SensorId::new("nope.nope")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn cache_reuses_loadavg_snapshot_within_ttl() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();

        // First sample populates cache
        let r1 = host_sample(&p, SensorId::new("system.load_1m")).unwrap();
        assert!(matches!(r1, Reading::Scalar(v) if (v - 1.23).abs() < 1e-6));

        // Mutate loadavg
        fs::write(dir.path().join("proc/loadavg"), "9.99 9.99 9.99 99/999 99999\n").unwrap();

        // Second sample immediately should still see cached value
        let r2 = host_sample(&p, SensorId::new("system.load_5m")).unwrap();
        assert!(
            matches!(r2, Reading::Scalar(v) if (v - 0.89).abs() < 1e-6),
            "cache should serve stale value"
        );
    }

    #[test]
    fn cache_expires_after_ttl() {
        let dir = fake_sysroot();
        let p = SystemPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();

        // First sample
        let r1 = host_sample(&p, SensorId::new("system.load_1m")).unwrap();
        assert!(matches!(r1, Reading::Scalar(v) if (v - 1.23).abs() < 1e-6));

        // Mutate loadavg
        fs::write(dir.path().join("proc/loadavg"), "9.99 9.99 9.99 99/999 99999\n").unwrap();

        // Wait for cache expiry
        std::thread::sleep(std::time::Duration::from_millis(60));

        // Second sample should see new value
        let r2 = host_sample(&p, SensorId::new("system.load_1m")).unwrap();
        assert!(
            matches!(r2, Reading::Scalar(v) if (v - 9.99).abs() < 1e-6),
            "cache should reflect new value after expiry"
        );
    }
}
