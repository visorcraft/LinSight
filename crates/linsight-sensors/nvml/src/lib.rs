// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

//! NVML sensor backend.
//!
//! Initializes NVML lazily on `init()`. Returns an empty manifest when the
//! library or driver isn't available — the daemon hosts the plugin
//! unconditionally, so a no-NVIDIA system gets zero NVIDIA sensors rather
//! than a hard error.

use std::path::PathBuf;
use std::sync::Mutex;

use std::collections::HashMap;

use linsight_core::{
    Category, Cell, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId,
    SensorKind, TableRow, Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use tracing::{info, warn};

#[derive(Default)]
pub struct NvmlPlugin {
    state: Mutex<Option<NvmlState>>,
}

struct NvmlState {
    nvml: Nvml,
    /// Count of devices enumerated at init time.
    device_count: u32,
    /// Optional sysroot override threaded through from PluginCtx. Used
    /// (only) to locate `/proc/<pid>/comm` when populating the per-process
    /// table so synthetic-fixture tests don't have to mock the real /proc.
    sysroot: Option<PathBuf>,
}

impl NvmlPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut guard = self.state.lock().expect("NvmlPlugin poisoned");
        // Init-once / keep-alive: the nvml-wrapper docs explicitly warn
        // that `Nvml::init` is heavy (loads every NVML function symbol)
        // and shouldn't be repeated. If we already have a live handle,
        // just rebuild the descriptor list against the existing handle's
        // device_count (which won't change at runtime — hot-plug of
        // NVIDIA GPUs is not supported by the driver).
        let sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        let (nvml_ref, device_count) = if let Some(state) = guard.as_mut() {
            state.sysroot = sysroot.clone();
            (&state.nvml, state.device_count)
        } else {
            let nvml = match Nvml::init() {
                Ok(n) => n,
                Err(e) => {
                    info!(error = ?e, "NVML init failed — assuming no NVIDIA hardware");
                    return Ok(PluginManifest {
                        plugin_id: "io.visorcraft.linsight.nvml".into(),
                        display_name: "NVIDIA NVML".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        sensors: vec![],
                        devices: vec![],
                    });
                }
            };
            // Driver/library version skew check. NVML loads its function
            // pointers from a libnvidia-ml.so that can drift out of sync
            // with the loaded kernel driver after a partial upgrade; on
            // mismatch queries silently return stale or malformed values.
            // Log loud enough that a frustrated operator finds the cause.
            match (nvml.sys_driver_version(), nvml.sys_nvml_version()) {
                (Ok(driver), Ok(lib)) => {
                    info!(driver, lib, "NVML initialized");
                }
                (driver, lib) => {
                    warn!(?driver, ?lib, "NVML version query failed — proceeding anyway");
                }
            }
            let device_count = nvml.device_count().map_err(|e| PluginError::Io(e.to_string()))?;
            *guard = Some(NvmlState { nvml, device_count, sysroot: sysroot.clone() });
            let state = guard.as_ref().expect("just inserted");
            (&state.nvml, state.device_count)
        };

        let mut sensors = Vec::with_capacity((device_count as usize) * 5);
        let mut devices: Vec<HardwareDevice> = Vec::with_capacity(device_count as usize);
        let mut keys_by_idx: HashMap<u32, HardwareDeviceKey> = HashMap::new();
        let nvml = nvml_ref;
        for i in 0..device_count {
            // Build the HardwareDevice entry first so we know the key
            // before we stamp it into per-device SensorDescriptors. The
            // UUID is the only NVIDIA identifier stable across reboot,
            // driver upgrade, and PCI slot reassignment, so it's the
            // payload of choice for the `nvml:` scheme.
            match nvml.device_by_index(i).and_then(|d| d.uuid()) {
                Ok(uuid) => {
                    let uuid_lc = uuid.to_ascii_lowercase();
                    let key = HardwareDeviceKey::try_new(format!("nvml:uuid:{uuid_lc}"))
                        .map_err(|e| PluginError::Io(format!("nvml gpu{i} bad uuid: {e}")))?;
                    let model = nvml
                        .device_by_index(i)
                        .and_then(|d| d.name())
                        .unwrap_or_else(|_| format!("NVIDIA GPU (gpu{i})"));
                    let pci_bus =
                        nvml.device_by_index(i).and_then(|d| d.pci_info()).ok().map(|p| p.bus_id);
                    devices.push(HardwareDevice {
                        key: key.clone(),
                        category: HardwareCategory::Gpu,
                        model,
                        vendor: Some("NVIDIA".into()),
                        location: pci_bus.map(|s| format!("PCI {s}")),
                        plugin_id: String::new(),
                        plugin_device_id: format!("gpu{i}"),
                        sensor_ids: vec![],
                    });
                    keys_by_idx.insert(i, key);
                }
                Err(e) => {
                    // No HardwareDevice row for this GPU — but keep its
                    // sensors so existing dashboards don't go dark. The
                    // sensors get device_key: None and float in the
                    // "ungrouped" bucket on the Hardware page.
                    warn!(gpu_idx = i, error = ?e, "NVML device uuid() failed; emitting sensors without device_key");
                }
            }

            // Device identity (model/nickname) is carried separately via
            // `device_key` → resolved `device_label`, and the GUI renders it
            // as a second title line. Keep display_name a device-agnostic
            // metric so the two don't duplicate (e.g. "GPU utilization").
            let device_id = Some(format!("gpu{i}"));
            let device_key = keys_by_idx.get(&i).cloned();
            sensors.push(scalar(
                &format!("nvml.gpu{i}.util"),
                "GPU utilization",
                Unit::Percent,
                &device_id,
                &device_key,
                2.0,
                Some(0.0),
                Some(100.0),
            ));
            sensors.push(scalar(
                &format!("nvml.gpu{i}.mem_used_bytes"),
                "GPU memory used",
                Unit::Bytes,
                &device_id,
                &device_key,
                1.0,
                Some(0.0),
                None,
            ));
            sensors.push(scalar(
                &format!("nvml.gpu{i}.mem_total_bytes"),
                "GPU memory total",
                Unit::Bytes,
                &device_id,
                &device_key,
                0.1,
                Some(0.0),
                None,
            ));
            // Total VRAM is fixed — sample once, no trend chart.
            sensors.last_mut().unwrap().tags.push(linsight_plugin_sdk::STATIC_TAG.into());
            sensors.push(scalar(
                &format!("nvml.gpu{i}.temp_c"),
                "GPU temperature",
                Unit::Celsius,
                &device_id,
                &device_key,
                1.0,
                None,
                None,
            ));
            sensors.push(scalar(
                &format!("nvml.gpu{i}.power_w"),
                "GPU power",
                Unit::Watts,
                &device_id,
                &device_key,
                1.0,
                Some(0.0),
                None,
            ));
            // Per-process GPU memory table. One row per running compute
            // or graphics process: [pid, command, gpu_mem_used_bytes].
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("nvml.gpu{i}.processes")),
                display_name: "GPU processes".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Table,
                category: Category::Gpu,
                native_rate_hz: 0.5,
                min: None,
                max: None,
                device_id: device_id.clone(),
                device_key: device_key.clone(),
                tags: vec![],
            });
        }

        Ok(PluginManifest {
            plugin_id: "io.visorcraft.linsight.nvml".into(),
            display_name: "NVIDIA NVML".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let guard = self.state.lock().expect("NvmlPlugin poisoned");
        let state =
            guard.as_ref().ok_or_else(|| PluginError::Transient("NVML not initialized".into()))?;
        let (idx, metric) = parse_sensor_id(sensor.as_str())
            .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;
        if idx >= state.device_count {
            return Err(PluginError::Unsupported(sensor.to_string()));
        }
        let dev = state.nvml.device_by_index(idx).map_err(|e| PluginError::Io(e.to_string()))?;
        match metric {
            "util" => {
                let u = dev.utilization_rates().map_err(|e| PluginError::Io(e.to_string()))?;
                Ok(Reading::Scalar(u.gpu as f64))
            }
            "mem_used_bytes" => {
                let m = dev.memory_info().map_err(|e| PluginError::Io(e.to_string()))?;
                Ok(Reading::Scalar(m.used as f64))
            }
            "mem_total_bytes" => {
                let m = dev.memory_info().map_err(|e| PluginError::Io(e.to_string()))?;
                Ok(Reading::Scalar(m.total as f64))
            }
            "temp_c" => {
                let t = dev
                    .temperature(TemperatureSensor::Gpu)
                    .map_err(|e| PluginError::Io(e.to_string()))?;
                Ok(Reading::Scalar(t as f64))
            }
            "power_w" => match dev.power_usage() {
                Ok(mw) => Ok(Reading::Scalar(mw as f64 / 1000.0)),
                Err(e) => {
                    warn!(error = ?e, "power_usage failed");
                    Err(PluginError::Transient(format!("{e}")))
                }
            },
            "processes" => {
                // NVML splits this into compute and graphics buckets; we
                // dedup by pid so a process showing up in both is
                // reported once with the higher of the two memory
                // readings.
                let mut by_pid: HashMap<u32, u64> = HashMap::new();
                let compute_res = dev.running_compute_processes();
                let graphics_res = dev.running_graphics_processes();
                let mut any_ok = false;
                if let Ok(procs) = &compute_res {
                    any_ok = true;
                    for p in procs {
                        let mem = match p.used_gpu_memory {
                            nvml_wrapper::enums::device::UsedGpuMemory::Used(b) => b,
                            nvml_wrapper::enums::device::UsedGpuMemory::Unavailable => 0,
                        };
                        by_pid.entry(p.pid).and_modify(|m| *m = (*m).max(mem)).or_insert(mem);
                    }
                }
                if let Ok(procs) = &graphics_res {
                    any_ok = true;
                    for p in procs {
                        let mem = match p.used_gpu_memory {
                            nvml_wrapper::enums::device::UsedGpuMemory::Used(b) => b,
                            nvml_wrapper::enums::device::UsedGpuMemory::Unavailable => 0,
                        };
                        by_pid.entry(p.pid).and_modify(|m| *m = (*m).max(mem)).or_insert(mem);
                    }
                }
                // If BOTH calls failed, the operator gets an explicit
                // error rather than an empty table that's
                // indistinguishable from "no processes running". MIG
                // mode, exclusive-compute, or a driver that lacks the
                // graphics query are all real conditions where this
                // arm used to silently lie.
                if !any_ok {
                    let c = compute_res.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
                    let g = graphics_res.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
                    return Err(PluginError::Io(format!(
                        "NVML process enumeration failed: compute=[{c}] graphics=[{g}]",
                    )));
                }
                if let Err(e) = &compute_res {
                    warn!(error = %e, "NVML compute-process enumeration failed; reporting graphics-only");
                }
                if let Err(e) = &graphics_res {
                    warn!(error = %e, "NVML graphics-process enumeration failed; reporting compute-only");
                }
                let mut entries: Vec<(u32, u64)> = by_pid.into_iter().collect();
                // Sort by memory descending so the GUI shows the biggest
                // tenant first.
                entries.sort_by_key(|(_, mem)| std::cmp::Reverse(*mem));
                let mut rows: Vec<TableRow> = Vec::with_capacity(entries.len());
                for (pid, mem) in entries {
                    let comm = comm_for_pid(state.sysroot.as_deref(), pid);
                    rows.push(TableRow {
                        cells: vec![Cell::Number(pid as f64), Cell::Text(comm), Cell::Bytes(mem)],
                    });
                }
                Ok(Reading::Table(rows))
            }
            _ => Err(PluginError::Unsupported(sensor.to_string())),
        }
    }
}

impl LinsightPlugin for NvmlPlugin {
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

#[allow(clippy::too_many_arguments)]
fn scalar(
    id: &str,
    name: &str,
    unit: Unit,
    device_id: &Option<String>,
    device_key: &Option<HardwareDeviceKey>,
    rate: f32,
    min: Option<f64>,
    max: Option<f64>,
) -> SensorDescriptor {
    SensorDescriptor {
        id: SensorId::new(id),
        display_name: name.into(),
        unit,
        kind: SensorKind::Scalar,
        category: Category::Gpu,
        native_rate_hz: rate,
        min,
        max,
        device_id: device_id.clone(),
        device_key: device_key.clone(),
        tags: vec![],
    }
}

fn parse_sensor_id(id: &str) -> Option<(u32, &str)> {
    let rest = id.strip_prefix("nvml.gpu")?;
    let (idx_str, metric) = rest.split_once('.')?;
    let idx = idx_str.parse::<u32>().ok()?;
    Some((idx, metric))
}

/// Read `/proc/<pid>/comm` rooted at `sysroot` if set, else `/proc`.
/// Returns `"?"` if the read fails — `comm` can race against the process
/// exiting between the NVML query and the read, and that's not a failure
/// we want to surface as an error per row.
fn comm_for_pid(sysroot: Option<&std::path::Path>, pid: u32) -> String {
    let path = match sysroot {
        Some(root) => root.join(format!("proc/{pid}/comm")),
        None => std::path::PathBuf::from(format!("/proc/{pid}/comm")),
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => s.trim().to_owned(),
        Err(_) => "?".into(),
    }
}

#[cfg(test)]
mod tests {
    use linsight_plugin_sdk::host_init;

    use super::*;

    #[test]
    fn manifest_empty_when_nvml_missing() {
        let plugin = NvmlPlugin::default();
        let ctx = PluginCtx::default();
        let manifest = host_init(&plugin, &ctx).unwrap();
        // Either no sensors at all, or sensors with appropriate device_keys.
        // On a no-NVML host this should produce empty everything.
        if manifest.sensors.is_empty() {
            assert!(manifest.devices.is_empty());
        }
    }

    #[test]
    #[ignore = "requires NVIDIA hardware + libnvidia-ml.so"]
    fn manifest_emits_nvml_uuid_device_per_gpu() {
        let plugin = NvmlPlugin::default();
        let ctx = PluginCtx::default();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert!(!manifest.devices.is_empty());
        for d in &manifest.devices {
            assert!(d.key.as_str().starts_with("nvml:uuid:"));
            assert!(!d.model.is_empty());
        }
    }

    #[test]
    fn parse_sensor_id_extracts_index_and_metric() {
        assert_eq!(parse_sensor_id("nvml.gpu0.util"), Some((0, "util")));
        assert_eq!(parse_sensor_id("nvml.gpu7.mem_total_bytes"), Some((7, "mem_total_bytes")));
        assert_eq!(parse_sensor_id("nvml.gpu0.processes"), Some((0, "processes")));
    }

    #[test]
    fn parse_sensor_id_rejects_garbage() {
        assert!(parse_sensor_id("xe.gpu0.util").is_none());
        assert!(parse_sensor_id("nvml.gpu.util").is_none());
        assert!(parse_sensor_id("nvml.gpunope.util").is_none());
    }

    #[test]
    fn comm_for_pid_returns_question_mark_on_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        // pid 999_999 won't exist under our empty fake sysroot
        let comm = comm_for_pid(Some(dir.path()), 999_999);
        assert_eq!(comm, "?");
    }

    #[test]
    fn comm_for_pid_reads_from_sysroot_when_present() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("proc/4242");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("comm"), "my-process\n").unwrap();
        let comm = comm_for_pid(Some(dir.path()), 4242);
        assert_eq!(comm, "my-process");
    }
}
