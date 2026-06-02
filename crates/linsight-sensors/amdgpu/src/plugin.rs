// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! AMD GPU sensor backend via sysfs (DRM + hwmon).
//!
//! Per AMD GPU found at `/sys/class/drm/cardN/` with vendor ID 0x1002:
//! * `amdgpu.<gpu>.util` — GPU busy percent (gpu_busy_percent)
//! * `amdgpu.<gpu>.mem_used_bytes` — VRAM used (mem_info_vram_used)
//! * `amdgpu.<gpu>.mem_total_bytes` — VRAM total (mem_info_vram_total)
//! * `amdgpu.<gpu>.temp_c` — GPU temperature via hwmon child
//! * `amdgpu.<gpu>.power_w` — GPU power draw via hwmon child
//! * `amdgpu.<gpu>.freq_hz` — GPU clock frequency via hwmon child

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

const AMD_VENDOR_ID: u16 = 0x1002;

#[derive(Default)]
pub struct AmdgpuPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    devices: Vec<AmdGpuDevice>,
}

#[derive(Clone, Debug)]
struct AmdGpuDevice {
    name: String,
    device_path: PathBuf,
    pci_addr: String,
    model: String,
    vendor: Option<String>,
    hwmon_dir: Option<PathBuf>,
}

impl AmdgpuPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("AmdgpuPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.devices = enumerate(inner.sysroot.as_deref());

        let mut sensors = Vec::new();
        let mut devices: Vec<HardwareDevice> = Vec::new();
        for dev in &inner.devices {
            let key = HardwareDeviceKey::try_new(format!("pci:{}", dev.pci_addr))
                .map_err(|e| PluginError::Io(format!("pci {}: {e}", dev.pci_addr)))?;
            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Gpu,
                model: dev.model.clone(),
                vendor: dev.vendor.clone(),
                location: Some(format!("PCI {}", dev.pci_addr)),
                plugin_id: String::new(),
                plugin_device_id: dev.name.clone(),
                sensor_ids: vec![],
            });
            // Device identity is carried via device_key → device_label and
            // shown as a second title line; keep display_name a bare metric.
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("amdgpu.{}.util", dev.name)),
                display_name: "GPU utilization".into(),
                unit: Unit::Percent,
                kind: SensorKind::Scalar,
                category: Category::Gpu,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: Some(100.0),
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("amdgpu.{}.mem_used_bytes", dev.name)),
                display_name: "GPU VRAM used".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Gpu,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("amdgpu.{}.mem_total_bytes", dev.name)),
                display_name: "GPU VRAM total".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Gpu,
                native_rate_hz: 0.1,
                min: Some(0.0),
                max: None,
                device_id: Some(dev.name.clone()),
                device_key: Some(key.clone()),
                // Total VRAM is fixed — sample once, no trend chart.
                tags: vec![linsight_plugin_sdk::STATIC_TAG.into()],
            });
            if dev.hwmon_dir.is_some() {
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("amdgpu.{}.temp_c", dev.name)),
                    display_name: "GPU temperature".into(),
                    unit: Unit::Celsius,
                    kind: SensorKind::Scalar,
                    category: Category::Gpu,
                    native_rate_hz: 1.0,
                    min: None,
                    max: None,
                    device_id: Some(dev.name.clone()),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("amdgpu.{}.power_w", dev.name)),
                    display_name: "GPU power".into(),
                    unit: Unit::Watts,
                    kind: SensorKind::Scalar,
                    category: Category::Gpu,
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
            plugin_id: "com.visorcraft.linsight.amdgpu".into(),
            display_name: "AMD GPU".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let inner = self.inner.lock().expect("AmdgpuPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("amdgpu.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (name, metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let dev = inner
            .devices
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| PluginError::Unsupported(id.into()))?;

        match metric {
            "util" => {
                let path = dev.device_path.join("gpu_busy_percent");
                let v = read_u64(&path)?;
                Ok(Reading::Scalar(v as f64))
            }
            "mem_used_bytes" => {
                let path = dev.device_path.join("mem_info_vram_used");
                let v = read_u64(&path)?;
                Ok(Reading::Scalar(v as f64))
            }
            "mem_total_bytes" => {
                let path = dev.device_path.join("mem_info_vram_total");
                let v = read_u64(&path)?;
                Ok(Reading::Scalar(v as f64))
            }
            "temp_c" => {
                let hwmon =
                    dev.hwmon_dir.as_ref().ok_or_else(|| PluginError::Unsupported(id.into()))?;
                // Find temp1_input
                let Ok(entries) = fs::read_dir(hwmon) else {
                    return Err(PluginError::Io("hwmon dir unreadable".into()));
                };
                let mut found = None;
                for e in entries.flatten() {
                    let fname = e.file_name().to_string_lossy().into_owned();
                    if fname.starts_with("temp") && fname.ends_with("_input") {
                        found = Some(e.path());
                        break;
                    }
                }
                let path = found.ok_or_else(|| PluginError::Unsupported("no temp_input".into()))?;
                let milli = read_u64(&path)?;
                Ok(Reading::Scalar(milli as f64 / 1000.0))
            }
            "power_w" => {
                let hwmon =
                    dev.hwmon_dir.as_ref().ok_or_else(|| PluginError::Unsupported(id.into()))?;
                let Ok(entries) = fs::read_dir(hwmon) else {
                    return Err(PluginError::Io("hwmon dir unreadable".into()));
                };
                let mut found = None;
                for e in entries.flatten() {
                    let fname = e.file_name().to_string_lossy().into_owned();
                    if fname.starts_with("power") && fname.ends_with("_input") {
                        found = Some(e.path());
                        break;
                    }
                }
                let path =
                    found.ok_or_else(|| PluginError::Unsupported("no power_input".into()))?;
                let uw = read_u64(&path)?;
                Ok(Reading::Scalar(uw as f64 / 1_000_000.0))
            }
            _ => Err(PluginError::Unsupported(id.into())),
        }
    }
}

impl LinsightPlugin for AmdgpuPlugin {
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

fn read_u64(path: &Path) -> Result<u64, PluginError> {
    let s = fs::read_to_string(path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    s.trim().parse::<u64>().map_err(|e| PluginError::Parse(format!("{}: {e}", path.display())))
}

/// Enumerate AMD GPUs via DRM sysfs at /sys/class/drm/cardN/
fn enumerate(sysroot: Option<&Path>) -> Vec<AmdGpuDevice> {
    let root = match sysroot {
        Some(r) => r.join("sys/class/drm"),
        None => PathBuf::from("/sys/class/drm"),
    };
    let Ok(entries) = fs::read_dir(&root) else { return vec![] };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue; // skip cardN-* render nodes
        }
        let dev_path = entry.path().join("device");
        // Check vendor ID
        let vendor_path = dev_path.join("vendor");
        let vendor_id = match fs::read_to_string(&vendor_path) {
            Ok(s) => {
                let t = s.trim().strip_prefix("0x").unwrap_or(s.trim());
                u16::from_str_radix(t, 16).unwrap_or(0)
            }
            Err(_) => continue,
        };
        if vendor_id != AMD_VENDOR_ID {
            continue;
        }
        let device_id = match fs::read_to_string(dev_path.join("device")) {
            Ok(s) => u16::from_str_radix(s.trim().strip_prefix("0x").unwrap_or(s.trim()), 16)
                .unwrap_or(0),
            Err(_) => 0,
        };
        let db = PciIdDb::shared();
        let model =
            db.lookup(AMD_VENDOR_ID, device_id).unwrap_or_else(|| format!("AMD GPU ({})", name));
        let vendor_name = db.vendor_name(AMD_VENDOR_ID);
        // Real PCI BDF lives at the end of the `cardN/device` symlink
        // target (e.g. "0000:03:00.0"). The prior code synthesized
        // "0000:0:00.0" from the card index, which passed
        // HardwareDeviceKey's shape check but didn't match the real BDF
        // used by lspci, hwmon-page nicknames, or anything else keyed
        // off the real address.
        let pci_addr = fs::read_link(&dev_path)
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "unknown".into());

        // Find hwmon child (amdgpu exposes temp/power via hwmon)
        let hwmon_dir = find_hwmon(&dev_path);

        out.push(AmdGpuDevice {
            name,
            device_path: dev_path,
            pci_addr,
            model,
            vendor: vendor_name,
            hwmon_dir,
        });
    }
    out
}

fn find_hwmon(device_path: &Path) -> Option<PathBuf> {
    let Ok(entries) = fs::read_dir(device_path) else { return None };
    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().into_owned();
        if fname.starts_with("hwmon") {
            return Some(entry.path());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_plugin_sdk::{host_init, host_sample};
    use std::fs;

    fn fake_sysroot() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let drm = dir.path().join("sys/class/drm");
        let card0 = drm.join("card0");
        fs::create_dir_all(&card0).unwrap();

        // Real sysfs lays `card0/device` out as a symlink to
        // `/sys/devices/pci.../<BDF>`. Mirror that so the enumerator's
        // BDF extraction is actually exercised.
        let pci_device = dir.path().join("sys/devices/pci0000:00/0000:03:00.0");
        fs::create_dir_all(&pci_device).unwrap();
        std::os::unix::fs::symlink(&pci_device, card0.join("device")).unwrap();

        // AMD vendor + device
        fs::write(pci_device.join("vendor"), "0x1002\n").unwrap();
        fs::write(pci_device.join("device"), "0x744c\n").unwrap();
        fs::write(pci_device.join("gpu_busy_percent"), "42\n").unwrap();
        fs::write(pci_device.join("mem_info_vram_used"), "2147483648\n").unwrap();
        fs::write(pci_device.join("mem_info_vram_total"), "8589934592\n").unwrap();

        // hwmon child
        let hwmon = pci_device.join("hwmon0");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(hwmon.join("name"), "amdgpu\n").unwrap();
        fs::write(hwmon.join("temp1_input"), "65000\n").unwrap();
        fs::write(hwmon.join("power1_input"), "150000000\n").unwrap();

        dir
    }

    #[test]
    fn enumerate_finds_amd_gpu() {
        let dir = fake_sysroot();
        let devs = enumerate(Some(dir.path()));
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "card0");
        assert_eq!(devs[0].pci_addr, "0000:03:00.0");
    }

    #[test]
    fn manifest_advertises_amd_sensors() {
        let dir = fake_sysroot();
        let p = AmdgpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        let ids: Vec<&str> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"amdgpu.card0.util"));
        assert!(ids.contains(&"amdgpu.card0.mem_used_bytes"));
        assert!(ids.contains(&"amdgpu.card0.mem_total_bytes"));
        assert!(ids.contains(&"amdgpu.card0.temp_c"));
        assert!(ids.contains(&"amdgpu.card0.power_w"));
    }

    #[test]
    fn sample_gpu_sensors() {
        let dir = fake_sysroot();
        let p = AmdgpuPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();

        let r = host_sample(&p, SensorId::new("amdgpu.card0.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 42.0));

        let r = host_sample(&p, SensorId::new("amdgpu.card0.temp_c")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 65.0).abs() < 1e-6));

        let r = host_sample(&p, SensorId::new("amdgpu.card0.power_w")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if (v - 150.0).abs() < 1e-6));
    }
}
