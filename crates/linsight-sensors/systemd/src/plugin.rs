// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Systemd unit monitoring sensor backend.
//!
//! Emits a `Reading::Table` of systemd service units found under the
//! cgroup v2 hierarchy at `/sys/fs/cgroup/system.slice/`.
//!
//! Columns: unit, state, cpu_delta_usec, memory_bytes, pids.
//!
//! Sensor id: `systemd.units` — one-shot table snapshot.
//! Default rate: 0.2 Hz (5-second gap) to keep cgroup walks cheap.
//!
//! No D-Bus dependency — pure cgroup v2 filesystem reads.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use linsight_core::{
    Category, Cell, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId,
    SensorKind, TableRow, Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

const MAX_UNITS: usize = 512;

pub struct SystemdPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    prev_cpu: HashMap<String, u64>,
}

impl Default for SystemdPlugin {
    fn default() -> Self {
        Self { inner: Mutex::new(Inner::default()) }
    }
}

impl SystemdPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("SystemdPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());

        let cgroup_root = cgroup_root(inner.sysroot.as_deref());
        if !cgroup_root.join("system.slice").is_dir() {
            return Ok(empty_manifest());
        }

        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.systemd".into(),
            display_name: "Systemd Units".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors: vec![SensorDescriptor {
                id: SensorId::new("systemd.units"),
                display_name: "Systemd service units".to_string(),
                unit: Unit::Count,
                kind: SensorKind::Table,
                category: Category::Custom,
                native_rate_hz: 0.2,
                min: None,
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            }],
            devices: vec![HardwareDevice {
                key: HardwareDeviceKey::try_new("system:systemd")
                    .map_err(|e| PluginError::Manifest(e.to_string()))?,
                category: HardwareCategory::Other,
                model: "Systemd Services".into(),
                vendor: None,
                location: None,
                plugin_id: "com.visorcraft.linsight.systemd".into(),
                plugin_device_id: "systemd".into(),
                sensor_ids: vec![SensorId::new("systemd.units")],
            }],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        if sensor.as_str() != "systemd.units" {
            return Err(PluginError::Unsupported(sensor.to_string()));
        }
        let mut inner = self.inner.lock().expect("SystemdPlugin poisoned");

        let cgroup_root = cgroup_root(inner.sysroot.as_deref());
        let slice_dir = cgroup_root.join("system.slice");
        if !slice_dir.is_dir() {
            return Ok(Reading::Table(vec![]));
        }

        let units = enumerate_units(&slice_dir, MAX_UNITS);
        let mut rows: Vec<TableRow> = Vec::with_capacity(units.len());
        let mut current_cpu: HashMap<String, u64> = HashMap::with_capacity(units.len());

        for unit in &units {
            let cgroup_path = slice_dir.join(&unit.cgroup_dir);
            let cpu = read_cpu_usage(&cgroup_path).unwrap_or(0);
            let mem = read_memory_current(&cgroup_path).unwrap_or(0);
            let pids = read_pids_current(&cgroup_path).unwrap_or(0);
            let has_procs = cgroup_has_procs(&cgroup_path);

            let delta = if inner.prev_cpu.is_empty() {
                0
            } else {
                cpu.saturating_sub(inner.prev_cpu.get(&unit.name).copied().unwrap_or(0))
            };
            current_cpu.insert(unit.name.clone(), cpu);

            let state = if has_procs { "running" } else { "inactive" };

            rows.push(TableRow {
                cells: vec![
                    Cell::Text(unit.name.clone()),
                    Cell::Text(state.to_string()),
                    Cell::Number(delta as f64),
                    Cell::Bytes(mem),
                    Cell::Number(pids as f64),
                ],
            });
        }

        rows.sort_by(|a, b| match (&a.cells[0], &b.cells[0]) {
            (Cell::Text(l), Cell::Text(r)) => l.cmp(r),
            _ => std::cmp::Ordering::Equal,
        });

        inner.prev_cpu = current_cpu;
        Ok(Reading::Table(rows))
    }
}

impl LinsightPlugin for SystemdPlugin {
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

struct UnitEntry {
    name: String,
    cgroup_dir: String,
}

fn cgroup_root(sysroot: Option<&Path>) -> PathBuf {
    match sysroot {
        Some(r) => r.join("sys/fs/cgroup"),
        None => PathBuf::from("/sys/fs/cgroup"),
    }
}

fn empty_manifest() -> PluginManifest {
    PluginManifest {
        plugin_id: "com.visorcraft.linsight.systemd".into(),
        display_name: "Systemd Units".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        sensors: vec![],
        devices: vec![],
    }
}

fn enumerate_units(slice_dir: &Path, max: usize) -> Vec<UnitEntry> {
    let mut units = Vec::new();
    let entries = match fs::read_dir(slice_dir) {
        Ok(e) => e,
        Err(_) => return units,
    };
    for entry in entries.flatten() {
        if units.len() >= max {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".service") {
            continue;
        }
        if !entry.path().is_dir() {
            continue;
        }
        units.push(UnitEntry { name, cgroup_dir: entry.file_name().to_string_lossy().to_string() });
    }
    units.sort_by(|a, b| a.name.cmp(&b.name));
    units
}

fn read_cpu_usage(cgroup_path: &Path) -> Result<u64, PluginError> {
    let content = fs::read_to_string(cgroup_path.join("cpu.stat"))
        .map_err(|e| PluginError::Io(format!("cpu.stat: {e}")))?;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("usage_usec ") {
            return val
                .trim()
                .parse::<u64>()
                .map_err(|e| PluginError::Parse(format!("cpu.stat usage_usec: {e}")));
        }
    }
    Ok(0)
}

fn read_memory_current(cgroup_path: &Path) -> Result<u64, PluginError> {
    let content = fs::read_to_string(cgroup_path.join("memory.current"))
        .map_err(|e| PluginError::Io(format!("memory.current: {e}")))?;
    content.trim().parse::<u64>().map_err(|e| PluginError::Parse(format!("memory.current: {e}")))
}

fn read_pids_current(cgroup_path: &Path) -> Result<u64, PluginError> {
    let content = fs::read_to_string(cgroup_path.join("pids.current"))
        .map_err(|e| PluginError::Io(format!("pids.current: {e}")))?;
    content.trim().parse::<u64>().map_err(|e| PluginError::Parse(format!("pids.current: {e}")))
}

fn cgroup_has_procs(cgroup_path: &Path) -> bool {
    fs::read_to_string(cgroup_path.join("cgroup.procs"))
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    fn fake_cgroup() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let cgroup = dir.path().join("sys/fs/cgroup");
        fs::create_dir_all(cgroup.join("system.slice")).unwrap();

        // sshd.service — active
        let sshd = cgroup.join("system.slice/sshd.service");
        fs::create_dir_all(&sshd).unwrap();
        fs::write(
            sshd.join("cpu.stat"),
            "usage_usec 1500000\nuser_usec 800000\nsystem_usec 700000\n",
        )
        .unwrap();
        fs::write(sshd.join("memory.current"), "8388608\n").unwrap();
        fs::write(sshd.join("pids.current"), "3\n").unwrap();
        fs::write(sshd.join("cgroup.procs"), "123\n456\n789\n").unwrap();

        // nginx.service — active
        let nginx = cgroup.join("system.slice/nginx.service");
        fs::create_dir_all(&nginx).unwrap();
        fs::write(
            nginx.join("cpu.stat"),
            "usage_usec 3200000\nuser_usec 2000000\nsystem_usec 1200000\n",
        )
        .unwrap();
        fs::write(nginx.join("memory.current"), "16777216\n").unwrap();
        fs::write(nginx.join("pids.current"), "5\n").unwrap();
        fs::write(nginx.join("cgroup.procs"), "200\n201\n").unwrap();

        // cronie.service — inactive (no procs)
        let cronie = cgroup.join("system.slice/cronie.service");
        fs::create_dir_all(&cronie).unwrap();
        fs::write(
            cronie.join("cpu.stat"),
            "usage_usec 500000\nuser_usec 250000\nsystem_usec 250000\n",
        )
        .unwrap();
        fs::write(cronie.join("memory.current"), "1048576\n").unwrap();
        fs::write(cronie.join("pids.current"), "0\n").unwrap();
        fs::write(cronie.join("cgroup.procs"), "").unwrap();

        // A non-service directory (should be skipped)
        fs::create_dir_all(cgroup.join("system.slice/session-1.scope")).unwrap();

        dir
    }

    #[test]
    fn init_advertises_systemd_units() {
        let dir = fake_cgroup();
        let plugin = SystemdPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"systemd.units"));
        assert_eq!(manifest.devices.len(), 1);
        assert_eq!(manifest.devices[0].key.as_str(), "system:systemd");
    }

    #[test]
    fn sample_returns_table_with_units() {
        let dir = fake_cgroup();
        let plugin = SystemdPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        let r = host_sample(&plugin, &SensorId::new("systemd.units")).unwrap();
        match r {
            Reading::Table(rows) => {
                assert_eq!(rows.len(), 3, "expected 3 service units, got {}", rows.len());

                // Sorted alphabetically: cronie, nginx, sshd
                assert!(matches!(&rows[0].cells[0], Cell::Text(t) if t == "cronie.service"));
                assert!(matches!(&rows[0].cells[1], Cell::Text(t) if t == "inactive"));
                assert!(matches!(&rows[1].cells[0], Cell::Text(t) if t == "nginx.service"));
                assert!(matches!(&rows[1].cells[1], Cell::Text(t) if t == "running"));
                assert!(matches!(&rows[2].cells[0], Cell::Text(t) if t == "sshd.service"));
                assert!(matches!(&rows[2].cells[1], Cell::Text(t) if t == "running"));

                // First sample: deltas are zero (no prev)
                assert!(matches!(&rows[2].cells[2], Cell::Number(v) if *v == 0.0));

                // Check memory and pids for sshd
                assert!(matches!(&rows[2].cells[3], Cell::Bytes(b) if *b == 8388608));
                assert!(matches!(&rows[2].cells[4], Cell::Number(v) if *v == 3.0));
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn second_sample_shows_cpu_delta() {
        let dir = fake_cgroup();
        let plugin = SystemdPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        let _ = host_sample(&plugin, &SensorId::new("systemd.units")).unwrap();

        // Bump sshd cpu usage
        let sshd = dir.path().join("sys/fs/cgroup/system.slice/sshd.service/cpu.stat");
        fs::write(sshd, "usage_usec 2500000\nuser_usec 1300000\nsystem_usec 1200000\n").unwrap();

        let r = host_sample(&plugin, &SensorId::new("systemd.units")).unwrap();
        match r {
            Reading::Table(rows) => {
                // sshd is rows[2] (alphabetical)
                match &rows[2].cells[2] {
                    Cell::Number(delta) => {
                        assert_eq!(*delta, 1000000.0, "expected 1M usec delta");
                    }
                    other => panic!("expected Number cell, got {other:?}"),
                }
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let dir = fake_cgroup();
        let plugin = SystemdPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, &SensorId::new("nope.nope")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn no_cgroup_v2_returns_empty_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        let plugin = SystemdPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert!(manifest.sensors.is_empty());
    }
}
