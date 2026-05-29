// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
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

use std::path::Path;

use crate::proc_stat::{
    CoreStat, Stat, core_util_between, read_proc_core_stats, read_proc_stat, util_between,
};

/// Read `/proc/cpuinfo` (rooted at `sysroot` if set) and return the first
/// `model name` value. None on any read/parse failure.
fn cpu_model_name(sysroot: Option<&Path>) -> Option<String> {
    let path = match sysroot {
        Some(root) => root.join("proc/cpuinfo"),
        None => PathBuf::from("/proc/cpuinfo"),
    };
    let text = std::fs::read_to_string(&path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("model name")
            && let Some(v) = rest.split_once(':').map(|(_, v)| v.trim())
            && !v.is_empty()
        {
            return Some(v.to_owned());
        }
    }
    None
}

/// Walk `/sys/class/hwmon/hwmon*` once to locate the CPU's package
/// temperature input file. Returns the resolved `temp*_input` path so
/// the sample path can subsequently `read_to_string` ONE file per
/// sample instead of re-walking the entire hwmon tree.
///
/// The canonical sources are:
///   * Intel: `name=coretemp`, with a `temp*_label` reading `Package id 0`.
///   * AMD Zen: `name=k10temp` or `zenpower`, with a label `Tctl` (or
///     `Tdie` as a fallback).
///
/// Returns `None` if no recognizable package temperature is present —
/// the caller maps that to `PluginError::Unsupported`.
fn find_package_temp_input(sysroot: Option<&Path>) -> Option<PathBuf> {
    let hwmon_root = match sysroot {
        Some(r) => r.join("sys/class/hwmon"),
        None => PathBuf::from("/sys/class/hwmon"),
    };
    let entries = std::fs::read_dir(&hwmon_root).ok()?;
    for entry in entries.flatten() {
        let hw = entry.path();
        let Ok(name) = std::fs::read_to_string(hw.join("name")) else { continue };
        let name = name.trim();
        let preferred_labels: &[&str] = match name {
            "coretemp" => &["Package id 0"],
            "k10temp" | "zenpower" => &["Tctl", "Tdie"],
            _ => continue,
        };
        let Ok(files) = std::fs::read_dir(&hw) else { continue };
        for f in files.flatten() {
            let p = f.path();
            let Some(stem) = p.file_name().and_then(|s| s.to_str()) else { continue };
            let Some(idx) = stem.strip_prefix("temp").and_then(|r| r.strip_suffix("_label")) else {
                continue;
            };
            let Ok(label) = std::fs::read_to_string(&p) else { continue };
            let label = label.trim();
            if preferred_labels.contains(&label) {
                let input = hw.join(format!("temp{idx}_input"));
                // Confirm the input file actually exists before
                // caching its path — a sensor with a label but no
                // _input would otherwise wedge the cache forever.
                if std::fs::metadata(&input).is_ok() {
                    return Some(input);
                }
            }
        }
    }
    None
}

/// Enumerate `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq`
/// paths once. The sample path averages reads from this cached list
/// instead of re-scanning the cpu* directory tree per sample.
/// Returns `None` if cpufreq isn't present on this host; returns
/// `Some(empty)` only if /sys/devices/system/cpu exists but no cpuN
/// has cpufreq (an unusual locked-freq config — still cache it).
fn enumerate_cpu_freq_paths(sysroot: Option<&Path>) -> Option<Vec<PathBuf>> {
    let cpu_root = match sysroot {
        Some(r) => r.join("sys/devices/system/cpu"),
        None => PathBuf::from("/sys/devices/system/cpu"),
    };
    let entries = std::fs::read_dir(&cpu_root).ok()?;
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else { continue };
        // Only `cpuN` directories where N is a number — skip "cpufreq"
        // and "cpuidle" siblings.
        let Some(rest) = name.strip_prefix("cpu") else { continue };
        if rest.parse::<u32>().is_err() {
            continue;
        }
        let freq_path = p.join("cpufreq/scaling_cur_freq");
        if std::fs::metadata(&freq_path).is_ok() {
            paths.push(freq_path);
        }
    }
    Some(paths)
}

/// Detect the number of online CPUs from `/proc/stat` by counting
/// lines matching `cpuN` (with a leading digit after "cpu").
fn count_cores_from_proc_stat(sysroot: Option<&Path>) -> usize {
    read_proc_core_stats(sysroot).map(|v| v.len()).unwrap_or(0)
}

#[derive(Default)]
pub struct CpuPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    prev_stat: Option<Stat>,
    /// Previous per-core stats for differential utilization computation.
    prev_core_stats: Option<Vec<(u32, CoreStat)>>,
    /// Resolved path to the `temp*_input` file under
    /// `/sys/class/hwmon/hwmonN` whose `temp*_label` matches our
    /// package-temperature label. Cached at first successful read so
    /// the daemon stops walking ~13 hwmon directories per second.
    /// `None` means "not yet probed" — `Some(None)` would mean
    /// "probed and unsupported on this host", but we don't bother
    /// caching the negative (re-probing is cheap-ish and lets
    /// hot-loaded modules become visible without a daemon restart).
    cached_temp_input: Option<PathBuf>,
    /// Cached list of `scaling_cur_freq` paths, one per online CPU.
    /// `None` means "not yet probed". Same rationale as
    /// `cached_temp_input`: avoid re-enumerating /sys/devices/system/cpu
    /// on every sample. cpufreq topology doesn't change at runtime
    /// (CPU hot-plug is rare on desktops).
    cached_freq_paths: Option<Vec<PathBuf>>,
}

impl CpuPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("CpuPlugin inner poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.prev_stat = None;
        inner.prev_core_stats = None;
        // Drop the cached sysfs paths whenever init runs — sysroot may
        // have changed (test fixtures, plugin reload). Cheap to re-probe
        // on the next temp/freq sample.
        inner.cached_temp_input = None;
        inner.cached_freq_paths = None;

        let cpu_key = HardwareDeviceKey::try_new("cpu:0").expect("static key");
        let model = cpu_model_name(ctx.sysroot()).unwrap_or_else(|| "CPU".into());

        // Detect cores from /proc/stat for per-core sensor advertisement.
        let core_count = count_cores_from_proc_stat(ctx.sysroot());

        let mut sensor_ids: Vec<SensorId> = Vec::new();
        let mut sensors: Vec<SensorDescriptor> = Vec::new();

        // Aggregate utilization
        let util_id = SensorId::new("cpu.util");
        sensor_ids.push(util_id.clone());
        sensors.push(SensorDescriptor {
            id: util_id,
            display_name: "CPU utilization".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: Some(0.0),
            max: Some(100.0),
            device_id: None,
            device_key: Some(cpu_key.clone()),
            tags: vec![],
        });

        // Per-core utilization sensors
        for core_idx in 0..core_count {
            let sid = SensorId::new(format!("cpu.core{core_idx}.util"));
            sensor_ids.push(sid.clone());
            sensors.push(SensorDescriptor {
                id: sid,
                display_name: format!("CPU core {core_idx} utilization"),
                unit: Unit::Percent,
                kind: SensorKind::Scalar,
                category: Category::Cpu,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: Some(100.0),
                device_id: None,
                device_key: Some(cpu_key.clone()),
                tags: vec![],
            });
        }

        // Temperature (optional)
        let temp_id = SensorId::new("cpu.temp_c");
        sensor_ids.push(temp_id.clone());
        sensors.push(SensorDescriptor {
            id: temp_id,
            display_name: "CPU temperature".into(),
            unit: Unit::Celsius,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: None,
            device_key: Some(cpu_key.clone()),
            tags: vec![],
        });

        // Frequency (optional)
        let freq_id = SensorId::new("cpu.freq_hz");
        sensor_ids.push(freq_id.clone());
        sensors.push(SensorDescriptor {
            id: freq_id,
            display_name: "CPU frequency".into(),
            unit: Unit::Hertz,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: Some(0.0),
            max: None,
            device_id: None,
            device_key: Some(cpu_key.clone()),
            tags: vec![],
        });

        let device = HardwareDevice {
            key: cpu_key.clone(),
            category: HardwareCategory::Cpu,
            model,
            vendor: None,
            location: None,
            plugin_id: String::new(),
            plugin_device_id: "cpu".into(),
            sensor_ids,
        };

        Ok(PluginManifest {
            plugin_id: "io.visorcraft.linsight.cpu".into(),
            display_name: "CPU".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: vec![device],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("CpuPlugin inner poisoned");

        // Match aggregate cpu.util first, then per-core cpu.coreN.util
        match sensor.as_str() {
            "cpu.util" => {
                let current = read_proc_stat(inner.sysroot.as_deref())
                    .map_err(|e| PluginError::Io(e.to_string()))?;
                let util = match inner.prev_stat {
                    None => 0.0,
                    Some(prev) => util_between(prev, current),
                };
                inner.prev_stat = Some(current);
                Ok(Reading::Scalar(util))
            }
            "cpu.temp_c" => {
                // First sample probes /sys/class/hwmon and caches the
                // resolved temp*_input path; subsequent samples just
                // read that one file. ~13 hwmon dirs avoided per sample.
                if inner.cached_temp_input.is_none() {
                    inner.cached_temp_input = find_package_temp_input(inner.sysroot.as_deref());
                }
                let path = inner
                    .cached_temp_input
                    .as_ref()
                    .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;
                let raw = std::fs::read_to_string(path)
                    .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
                let milli: i32 = raw
                    .trim()
                    .parse()
                    .map_err(|e| PluginError::Parse(format!("{}: {e}", path.display())))?;
                Ok(Reading::Scalar(milli as f64 / 1000.0))
            }
            "cpu.freq_hz" => {
                // First sample enumerates /sys/devices/system/cpu/cpu*
                // and caches the per-CPU scaling_cur_freq paths;
                // subsequent samples just read those N files.
                if inner.cached_freq_paths.is_none() {
                    inner.cached_freq_paths = enumerate_cpu_freq_paths(inner.sysroot.as_deref());
                }
                let paths = inner
                    .cached_freq_paths
                    .as_ref()
                    .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;
                if paths.is_empty() {
                    return Err(PluginError::Unsupported(sensor.to_string()));
                }
                let mut sum: u64 = 0;
                let mut count: u64 = 0;
                for p in paths {
                    let Ok(raw) = std::fs::read_to_string(p) else { continue };
                    if let Ok(khz) = raw.trim().parse::<u64>() {
                        sum = sum.saturating_add(khz);
                        count += 1;
                    }
                }
                let khz = sum
                    .checked_div(count)
                    .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;
                // Multiply to Hz to match the Hertz unit (same as xe.gpuN.freq_hz).
                Ok(Reading::Scalar(khz as f64 * 1000.0))
            }
            // Per-core utilization: cpu.coreN.util
            _ => {
                if let Some(rest) = sensor.as_str().strip_prefix("cpu.core")
                    && let Some(idx_str) = rest.strip_suffix(".util")
                    && let Ok(core_idx) = idx_str.parse::<u32>()
                {
                    let current_list = read_proc_core_stats(inner.sysroot.as_deref())
                        .map_err(|e| PluginError::Io(e.to_string()))?;
                    let current = current_list
                        .iter()
                        .find(|&&(idx, _)| idx == core_idx)
                        .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?
                        .1;
                    let util = match &inner.prev_core_stats {
                        None => 0.0,
                        Some(prev_list) => {
                            match prev_list.iter().find(|&&(idx, _)| idx == core_idx) {
                                Some(&(_, prev)) => core_util_between(prev, current),
                                None => 0.0,
                            }
                        }
                    };
                    inner.prev_core_stats = Some(current_list);
                    return Ok(Reading::Scalar(util));
                }
                Err(PluginError::Unsupported(sensor.to_string()))
            }
        }
    }
}

impl LinsightPlugin for CpuPlugin {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    fn fake_sysroot(stat_content: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("proc")).unwrap();
        fs::write(dir.path().join("proc/stat"), stat_content).unwrap();
        dir
    }

    #[test]
    fn init_returns_three_sensors_with_no_cores() {
        // With only aggregate cpu line, no per-core sensors appear.
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["cpu.util", "cpu.temp_c", "cpu.freq_hz"]);
    }

    #[test]
    fn manifest_emits_per_core_sensors() {
        let stat = "cpu 200 0 100 2000 0 0 0 0 0 0\n\
                     cpu0 100 0 50 1000 0 0 0 0 0 0\n\
                     cpu1 60 0 30 800 0 0 0 0 0 0\n";
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot(stat);
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"cpu.core0.util"));
        assert!(ids.contains(&"cpu.core1.util"));
        // Still has aggregate
        assert!(ids.contains(&"cpu.util"));
        assert!(ids.contains(&"cpu.temp_c"));
        assert!(ids.contains(&"cpu.freq_hz"));
    }

    #[test]
    fn per_core_sensors_have_percent_unit_and_range() {
        let stat = "cpu 200 0 100 2000 0 0 0 0 0 0\n\
                     cpu0 100 0 50 1000 0 0 0 0 0 0\n\
                     cpu1 60 0 30 800 0 0 0 0 0 0\n";
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot(stat);
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        let core0 = manifest.sensors.iter().find(|s| s.id.as_str() == "cpu.core0.util").unwrap();
        assert_eq!(core0.unit, Unit::Percent);
        assert_eq!(core0.min, Some(0.0));
        assert_eq!(core0.max, Some(100.0));
        assert_eq!(core0.device_key.as_ref().map(|k| k.as_str()), Some("cpu:0"));

        // NOTE: sensor_ids on the device are not round-tripped through the ABI
        // mirror (RHardwareDevice -> HardwareDevice sets sensor_ids: vec![]).
        // Per-core sensor presence is verified above via manifest.sensors.
        // We verify the device count and key instead.
        assert_eq!(manifest.devices.len(), 1);
        assert_eq!(manifest.devices[0].key.as_str(), "cpu:0");
    }

    #[test]
    fn per_core_first_sample_returns_zero() {
        let stat = "cpu 200 0 100 2000 0 0 0 0 0 0\n\
                     cpu0 100 0 50 1000 0 0 0 0 0 0\n\
                     cpu1 60 0 30 800 0 0 0 0 0 0\n";
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot(stat);
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, SensorId::new("cpu.core0.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 0.0));
        let r = host_sample(&plugin, SensorId::new("cpu.core1.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 0.0));
    }

    #[test]
    fn per_core_second_sample_reflects_delta() {
        let stat1 = "cpu 200 0 100 2000 0 0 0 0 0 0\n\
                      cpu0 100 0 50 1000 0 0 0 0 0 0\n\
                      cpu1 60 0 30 800 0 0 0 0 0 0\n";
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot(stat1);
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        // First sample (caches the baseline)
        host_sample(&plugin, SensorId::new("cpu.core0.util")).unwrap();

        // Second sample: cpu0 user went from 100 -> 200, idle unchanged
        let stat2 = "cpu 300 0 100 2000 0 0 0 0 0 0\n\
                      cpu0 200 0 50 1000 0 0 0 0 0 0\n\
                      cpu1 60 0 30 800 0 0 0 0 0 0\n";
        fs::write(dir.path().join("proc/stat"), stat2).unwrap();

        let r = host_sample(&plugin, SensorId::new("cpu.core0.util")).unwrap();
        // cpu0: busy went from 150 to 250 (delta 100), total from 1150 to 1250 (delta 100)
        // utilization = 100/100 * 100 = 100%
        assert!(matches!(r, Reading::Scalar(v) if (v - 100.0).abs() < 1e-6));
    }

    #[test]
    fn per_core_and_aggregate_sensors_work_independently() {
        let stat1 = "cpu 400 0 200 4000 0 0 0 0 0 0\n\
                      cpu0 100 0 50 1000 0 0 0 0 0 0\n\
                      cpu1 60 0 30 800 0 0 0 0 0 0\n";
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot(stat1);
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        // Sample aggregate (sets prev_stat, not prev_core_stats)
        host_sample(&plugin, SensorId::new("cpu.util")).unwrap();
        // Sample per-core (sets prev_core_stats, not prev_stat)
        host_sample(&plugin, SensorId::new("cpu.core0.util")).unwrap();

        // Change only per-core stat (cpu0), leaving aggregate unchanged
        let stat2 = "cpu 400 0 200 4000 0 0 0 0 0 0\n\
                      cpu0 200 0 50 1000 0 0 0 0 0 0\n\
                      cpu1 280 0 30 800 0 0 0 0 0 0\n";
        fs::write(dir.path().join("proc/stat"), stat2).unwrap();

        // Per-core should see its own delta (cpu0: 150->250 busy, 1150->1250 total)
        let r = host_sample(&plugin, SensorId::new("cpu.core0.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 100.0).abs() < 1e-6));

        // Aggregate still sees its last cached prev_stat (from stat1).
        // Since aggregate itself hasn't changed, util is 0.
        let r = host_sample(&plugin, SensorId::new("cpu.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 0.0));
    }

    #[test]
    fn unknown_per_core_sensor_errors() {
        let stat = "cpu 200 0 100 2000 0 0 0 0 0 0\n\
                     cpu0 100 0 50 1000 0 0 0 0 0 0\n";
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot(stat);
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, SensorId::new("cpu.core5.util")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn unknown_sensor_errors() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, SensorId::new("not.cpu")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    /// Build a synthetic `coretemp` hwmon at `<sysroot>/sys/class/hwmon/hwmon0`
    /// with a single `Package id 0` temperature reading the caller's value.
    fn write_coretemp(root: &std::path::Path, milli_c: i32) {
        let hw = root.join("sys/class/hwmon/hwmon0");
        fs::create_dir_all(&hw).unwrap();
        fs::write(hw.join("name"), "coretemp\n").unwrap();
        fs::write(hw.join("temp1_label"), "Package id 0\n").unwrap();
        fs::write(hw.join("temp1_input"), format!("{milli_c}\n")).unwrap();
    }

    #[test]
    fn temp_sensor_reads_package_id_0() {
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        write_coretemp(dir.path(), 72500);

        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, SensorId::new("cpu.temp_c")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 72.5).abs() < 1e-6));
    }

    #[test]
    fn temp_sensor_unsupported_without_coretemp() {
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        // No hwmon at all. The sample path must surface Unsupported,
        // not panic and not return a stale value.
        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, SensorId::new("cpu.temp_c")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn temp_sensor_picks_amd_tctl_label() {
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let hw = dir.path().join("sys/class/hwmon/hwmon0");
        fs::create_dir_all(&hw).unwrap();
        fs::write(hw.join("name"), "k10temp\n").unwrap();
        fs::write(hw.join("temp1_label"), "Tctl\n").unwrap();
        fs::write(hw.join("temp1_input"), "55000\n").unwrap();

        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, SensorId::new("cpu.temp_c")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 55.0).abs() < 1e-6));
    }

    /// Build a synthetic cpufreq fixture: each cpu gets one
    /// `cpufreq/scaling_cur_freq` file with the given kHz value.
    fn write_cpufreq(root: &std::path::Path, per_cpu_khz: &[u64]) {
        for (i, &khz) in per_cpu_khz.iter().enumerate() {
            let p = root.join(format!("sys/devices/system/cpu/cpu{i}/cpufreq"));
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("scaling_cur_freq"), format!("{khz}\n")).unwrap();
        }
    }

    #[test]
    fn freq_sensor_averages_per_cpu_khz_to_hz() {
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        // 1,000,000 + 2,000,000 + 3,000,000 = 6,000,000 / 3 = 2_000_000 kHz = 2 GHz
        write_cpufreq(dir.path(), &[1_000_000, 2_000_000, 3_000_000]);

        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, SensorId::new("cpu.freq_hz")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 2_000_000_000.0).abs() < 1.0));
    }

    #[test]
    fn freq_sensor_unsupported_without_cpufreq() {
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, SensorId::new("cpu.freq_hz")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn first_sample_returns_zero() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 100 0 50 1000 0 0 0 0\n");
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, SensorId::new("cpu.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 0.0));
    }

    #[test]
    fn second_sample_reflects_busy_delta() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 100 0 50 1000 0 0 0 0\n");
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        host_sample(&plugin, SensorId::new("cpu.util")).unwrap();
        std::fs::write(dir.path().join("proc/stat"), "cpu 200 0 50 1000 0 0 0 0\n").unwrap();
        let r = host_sample(&plugin, SensorId::new("cpu.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 100.0));
    }

    #[test]
    fn manifest_emits_cpu_device() {
        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::default();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 1);
        let dev = &manifest.devices[0];
        assert_eq!(dev.key.as_str(), "cpu:0");
        assert_eq!(dev.category, linsight_core::HardwareCategory::Cpu);
        assert!(!dev.model.is_empty());
        for s in &manifest.sensors {
            assert_eq!(s.device_key.as_ref().map(|k| k.as_str()), Some("cpu:0"));
        }
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, SensorId::new("not.cpu")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }
}
