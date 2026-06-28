// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Generic hwmon sensor backend.
//!
//! Enumerates all `/sys/class/hwmon/hwmonN/` devices and exposes:
//! * `hwmon.<name>.<label>_temp_c` — temperature sensors
//! * `hwmon.<name>.<label>_fan_rpm` — fan speed sensors
//! * `hwmon.<name>.<label>_volts` — voltage sensors
//! * `hwmon.<name>.power_w` — power sensors
//! * `hwmon.<name>.<label>_amps` — current sensors
//!
//! One HardwareDevice per hwmonN, keyed as `hwmon:<name>`.
//! Skips hwmon devices whose `name` is `coretemp`, `k10temp`, or `zenpower`
//! (handled by the CPU plugin with its dedicated label matching logic).

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

/// hwmon names handled by the CPU plugin — skip to avoid overlap.
const CPU_MONITORED_HWMONS: &[&str] = &["coretemp", "k10temp", "zenpower"];

#[derive(Default)]
pub struct HwmonPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    devices: Vec<HwmonDeviceInfo>,
}

#[derive(Clone, Debug)]
struct HwmonDeviceInfo {
    name: String,
    sensors: Vec<HwmonSensorInfo>,
}

#[derive(Clone, Debug)]
struct HwmonSensorInfo {
    id: String,
    display_name: String,
    unit: Unit,
    kind: SensorKind,
    path: PathBuf,
}

impl HwmonPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("HwmonPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.devices = enumerate_hwmon(ctx.sysroot());

        let mut sensors: Vec<SensorDescriptor> = Vec::new();
        let mut devices: Vec<HardwareDevice> = Vec::new();

        for dev in &inner.devices {
            let key = HardwareDeviceKey::try_new(format!("hwmon:{}", dev.name))
                .map_err(|e| PluginError::Io(format!("hwmon {}: {e}", dev.name)))?;

            let dev_sensor_ids: Vec<SensorId> =
                dev.sensors.iter().map(|s| SensorId::new(s.id.as_str())).collect();

            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Other,
                model: dev.name.clone(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: dev.name.clone(),
                sensor_ids: dev_sensor_ids.clone(),
            });

            for s in &dev.sensors {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(s.id.as_str()),
                    display_name: s.display_name.clone(),
                    unit: s.unit.clone(),
                    kind: s.kind,
                    category: Category::Custom,
                    native_rate_hz: 0.5,
                    min: None,
                    max: None,
                    device_id: Some(dev.name.clone()),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
            }
        }

        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.hwmon".into(),
            display_name: "Hardware Monitor".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let inner = self.inner.lock().expect("HwmonPlugin poisoned");
        let id = sensor.as_str();

        // Walk all devices and find matching sensor
        for dev in &inner.devices {
            for s in &dev.sensors {
                if s.id == id {
                    let raw = fs::read_to_string(&s.path)
                        .map_err(|e| PluginError::Io(format!("{}: {e}", s.path.display())))?;
                    let val: i64 = raw
                        .trim()
                        .parse()
                        .map_err(|e| PluginError::Parse(format!("{}: {e}", s.path.display())))?;
                    return match s.unit {
                        Unit::Celsius => Ok(Reading::Scalar(val as f64 / 1000.0)),
                        Unit::Volts => Ok(Reading::Scalar(val as f64 / 1000.0)),
                        Unit::Watts => Ok(Reading::Scalar(val as f64 / 1_000_000.0)),
                        Unit::Rpm | Unit::Count => Ok(Reading::Scalar(val as f64)),
                        _ => Ok(Reading::Scalar(val as f64)),
                    };
                }
            }
        }

        Err(PluginError::Unsupported(id.into()))
    }
}

impl LinsightPlugin for HwmonPlugin {
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

/// Enumerate all hwmon devices and their sensors.
fn enumerate_hwmon(sysroot: Option<&Path>) -> Vec<HwmonDeviceInfo> {
    let root = match sysroot {
        Some(r) => r.join("sys/class/hwmon"),
        None => PathBuf::from("/sys/class/hwmon"),
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return vec![];
    };
    // First pass: gather (hwmon_index, name, dir) so duplicate names from
    // identical drivers (e.g. one hwmon per nvme controller, all named
    // "nvme") can be disambiguated deterministically by hwmonN index.
    let mut raw: Vec<(u32, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(dname) = dir.file_name().and_then(|s| s.to_str()) else { continue };
        let Some(idx_str) = dname.strip_prefix("hwmon") else { continue };
        let Ok(hwmon_index) = idx_str.parse::<u32>() else { continue };
        let name = match fs::read_to_string(dir.join("name")) {
            Ok(n) => n.trim().to_owned(),
            Err(_) => continue,
        };
        // Skip hwmon devices handled by the CPU plugin
        if CPU_MONITORED_HWMONS.contains(&name.as_str()) {
            continue;
        }
        raw.push((hwmon_index, name, dir));
    }
    raw.sort_by_key(|(idx, _, _)| *idx);

    // Second pass: disambiguate duplicate names. First occurrence of each
    // name keeps the bare value (backward compatible for single-device
    // common case); subsequent occurrences get the hwmonN index appended.
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (hwmon_index, base_name, dir) in raw {
        let name = if taken.insert(base_name.clone()) {
            base_name
        } else {
            format!("{base_name}_{hwmon_index}")
        };
        let sensors = discover_sensors(&dir, &name);
        if sensors.is_empty() {
            continue;
        }
        out.push(HwmonDeviceInfo { name, sensors });
    }
    out
}

/// Discover all sensor inputs under a single hwmonN directory.
///
/// `hwmon_name` is the (possibly disambiguated) device name used to form
/// sensor IDs and human-readable labels. Passed in by the caller rather
/// than re-read from `name` so that collisions resolved in
/// [`enumerate_hwmon`] flow through to sensor IDs as well.
fn discover_sensors(hwmon_dir: &Path, hwmon_name: &str) -> Vec<HwmonSensorInfo> {
    let Ok(entries) = fs::read_dir(hwmon_dir) else {
        return vec![];
    };

    // Collect all _input files and their corresponding _label files
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(fname) = p.file_name().and_then(|s| s.to_str()) else { continue };

        // Only match *_input files
        let (prefix, _suffix) = match fname.rsplit_once('_') {
            Some((pre, "input")) => (pre, pre.split_once('_').unwrap_or((pre, pre))),
            _ => continue,
        };

        let stem = prefix; // e.g. "temp1" from "temp1_input"
        let type_prefix = stem.trim_end_matches(|c: char| c.is_ascii_digit()); // e.g. "temp"

        // Determine the sensor type from the prefix
        let (unit, kind, label_suffix) = match type_prefix {
            "temp" => (Unit::Celsius, SensorKind::Scalar, "_temp_c"),
            "fan" => (Unit::Rpm, SensorKind::Scalar, "_fan_rpm"),
            "in" => (Unit::Volts, SensorKind::Scalar, "_volts"),
            "power" => (Unit::Watts, SensorKind::Scalar, "_power_w"),
            "curr" => (Unit::Count, SensorKind::Scalar, "_amps"),
            _ => continue,
        };

        // Probe the input file once before advertising. Some kernel
        // drivers expose an _input file that always returns EIO (e.g.
        // acpi_fan on certain Razer Blade ACPI tables); skipping those
        // here keeps the scheduler from logging `sample failed` every
        // tick forever.
        if fs::read_to_string(&p).is_err() {
            continue;
        }

        // Read label file (e.g. temp1_label) for display
        let label_path = hwmon_dir.join(format!("{stem}_label"));
        let label = fs::read_to_string(&label_path)
            .ok()
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|| stem.to_owned());

        // Sanitize label for sensor id: lowercase, replace non-alphanumeric with _
        let safe_label: String =
            label
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' }
                })
                .collect();

        let sensor_id = format!("hwmon.{}.{}{}", hwmon_name, safe_label, label_suffix);

        out.push(HwmonSensorInfo {
            id: sensor_id,
            // The chip (`hwmon_name`) is carried as the device label / second
            // title line; keep display_name the bare sensor label.
            display_name: label,
            unit,
            kind,
            path: p.clone(),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    fn fake_sysroot() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        // Create hwmon0: motherboard Super I/O
        let hw0 = dir.path().join("sys/class/hwmon/hwmon0");
        fs::create_dir_all(&hw0).unwrap();
        fs::write(hw0.join("name"), "nct6795\n").unwrap();
        fs::write(hw0.join("temp1_input"), "45000\n").unwrap();
        fs::write(hw0.join("temp1_label"), "CPU Socket\n").unwrap();
        fs::write(hw0.join("fan1_input"), "1200\n").unwrap();
        fs::write(hw0.join("fan1_label"), "CPU Fan\n").unwrap();
        fs::write(hw0.join("in0_input"), "12000\n").unwrap();
        fs::write(hw0.join("in0_label"), "+12V\n").unwrap();
        // Create hwmon1: PSU (no label files)
        let hw1 = dir.path().join("sys/class/hwmon/hwmon1");
        fs::create_dir_all(&hw1).unwrap();
        fs::write(hw1.join("name"), "acpi_power\n").unwrap();
        fs::write(hw1.join("power1_input"), "15000000\n").unwrap();
        // Create coretemp (should be skipped — handled by CPU plugin)
        let hw2 = dir.path().join("sys/class/hwmon/hwmon2");
        fs::create_dir_all(&hw2).unwrap();
        fs::write(hw2.join("name"), "coretemp\n").unwrap();
        fs::write(hw2.join("temp1_input"), "55000\n").unwrap();
        fs::write(hw2.join("temp1_label"), "Package id 0\n").unwrap();
        dir
    }

    #[test]
    fn enumerate_discovers_all_non_cpu_hwmon() {
        let dir = fake_sysroot();
        let devs = enumerate_hwmon(Some(dir.path()));
        // hwmon0 (nct6795), hwmon1 (acpi_power) — hwmon2 (coretemp) skipped.
        // Order is by hwmonN index (deterministic across readdir orderings).
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].name, "nct6795");
        assert_eq!(devs[1].name, "acpi_power");
    }

    #[test]
    fn sensors_that_fail_to_read_at_init_are_skipped() {
        // Some hwmon sensors expose an `_input` file that the kernel
        // returns EIO when read (e.g. acpi_fan's fan1_input on certain
        // Razer Blade ACPI tables). The plugin should probe at init
        // and silently drop those sensors so the scheduler doesn't
        // spam `sample failed` warnings every 2s forever.
        let dir = tempfile::TempDir::new().unwrap();
        let hw = dir.path().join("sys/class/hwmon/hwmon0");
        fs::create_dir_all(&hw).unwrap();
        fs::write(hw.join("name"), "acpi_fan\n").unwrap();
        // A working temp sensor.
        fs::write(hw.join("temp1_input"), "45000\n").unwrap();
        fs::write(hw.join("temp1_label"), "Chassis\n").unwrap();
        // A broken fan sensor: `fan1_input` is a dangling symlink, so any
        // read() attempt returns an io error (ENOENT in this case, but the
        // plugin must treat *any* read error at init the same way).
        std::os::unix::fs::symlink("/nonexistent/path/that/cannot/be/read", hw.join("fan1_input"))
            .unwrap();
        fs::write(hw.join("fan1_label"), "Chassis Fan\n").unwrap();
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.iter().any(|id| id == &"hwmon.acpi_fan.chassis_temp_c"),
            "the readable temp sensor must surface: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.ends_with("_fan_rpm")),
            "the unreadable fan sensor must be skipped: {ids:?}"
        );
    }

    #[test]
    fn hwmon_devices_with_same_name_get_distinct_keys() {
        // Multi-drive systems expose multiple hwmon devices that all set
        // name="nvme" (one per controller). The plugin must surface each
        // one with a unique device key and unique sensor IDs; otherwise
        // the SDK's manifest dedup rejects the whole plugin.
        let dir = tempfile::TempDir::new().unwrap();
        for idx in 0..2_u32 {
            let hw = dir.path().join(format!("sys/class/hwmon/hwmon{idx}"));
            fs::create_dir_all(&hw).unwrap();
            fs::write(hw.join("name"), "nvme\n").unwrap();
            fs::write(hw.join("temp1_input"), format!("{}\n", 40000 + idx * 1000)).unwrap();
            fs::write(hw.join("temp1_label"), "Composite\n").unwrap();
        }
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 2, "both hwmon nvme devices should be exposed");
        let keys: std::collections::HashSet<_> =
            manifest.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        assert_eq!(keys.len(), 2, "device keys must be unique, got: {keys:?}");
        let ids: std::collections::HashSet<_> =
            manifest.sensors.iter().map(|s| s.id.as_str().to_owned()).collect();
        assert_eq!(
            ids.len(),
            manifest.sensors.len(),
            "sensor IDs must be unique across all devices, got: {ids:?}"
        );
    }

    #[test]
    fn manifest_advertises_hwmon_devices_and_sensors() {
        let dir = fake_sysroot();
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        // 2 devices: nct6795 (3 sensors: temp, fan, in) + acpi_power (1: power)
        assert_eq!(manifest.devices.len(), 2);
        assert_eq!(manifest.sensors.len(), 4);
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"hwmon.nct6795.cpu_socket_temp_c"));
        assert!(ids.contains(&"hwmon.nct6795.cpu_fan_fan_rpm"));
        assert!(ids.contains(&"hwmon.nct6795._12v_volts"));
        assert!(ids.contains(&"hwmon.acpi_power.power1_power_w"));
    }

    #[test]
    fn sample_temp_c() {
        let dir = fake_sysroot();
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, &SensorId::new("hwmon.nct6795.cpu_socket_temp_c")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 45.0).abs() < 1e-6));
    }

    #[test]
    fn sample_fan_rpm() {
        let dir = fake_sysroot();
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, &SensorId::new("hwmon.nct6795.cpu_fan_fan_rpm")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 1200.0));
    }

    #[test]
    fn sample_voltage() {
        let dir = fake_sysroot();
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, &SensorId::new("hwmon.nct6795._12v_volts")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 12.0).abs() < 1e-6));
    }

    #[test]
    fn sample_power_w() {
        let dir = fake_sysroot();
        let plugin = HwmonPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let r = host_sample(&plugin, &SensorId::new("hwmon.acpi_power.power1_power_w")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 15.0).abs() < 1e-6));
    }
}
