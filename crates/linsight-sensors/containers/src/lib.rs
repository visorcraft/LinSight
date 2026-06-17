// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

//! Container monitoring sensor backend (Docker + Podman).
//!
//! Emits a single `Reading::Table` (`containers.list`) of running
//! containers discovered in the cgroup v2 hierarchy. With the systemd
//! cgroup driver (the default on modern distros), Docker containers
//! appear as `system.slice/docker-<id>.scope` and Podman containers as
//! `machine.slice/libpod-<id>.scope`. We read each scope's `cpu.stat`,
//! `memory.current`, `pids.current`, and `cgroup.procs` — exactly the
//! way the systemd-units plugin reads service slices.
//!
//! Columns: id (short), runtime, cpu_delta_usec, memory_bytes, pids.
//!
//! No Docker/Podman socket or API dependency — pure cgroup v2 filesystem
//! reads, so it needs no elevated privileges and degrades to an empty
//! table when no container runtime is present. The `cgroupfs` (non-systemd)
//! cgroup driver, which nests containers under `/sys/fs/cgroup/docker/<id>`
//! instead of `.scope` units, is not covered.

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

const MAX_CONTAINERS: usize = 512;
const SHORT_ID_LEN: usize = 12;

/// The cgroup-v2 slices that hold container `.scope` units under the
/// systemd cgroup driver: Docker lands in `system.slice`, Podman in
/// `machine.slice`.
const CONTAINER_SLICES: &[&str] = &["system.slice", "machine.slice"];

pub struct ContainersPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    prev_cpu: HashMap<String, u64>,
}

impl Default for ContainersPlugin {
    fn default() -> Self {
        Self { inner: Mutex::new(Inner::default()) }
    }
}

struct ContainerEntry {
    runtime: &'static str,
    short_id: String,
    /// Unique scope directory name, used as the `prev_cpu` delta key.
    scope_name: String,
    cgroup_path: PathBuf,
}

impl ContainersPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("ContainersPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());

        let cgroup_root = cgroup_root(inner.sysroot.as_deref());
        let any_slice = CONTAINER_SLICES.iter().any(|s| cgroup_root.join(s).is_dir());
        if !any_slice {
            return Ok(empty_manifest());
        }

        Ok(PluginManifest {
            plugin_id: PLUGIN_ID.into(),
            display_name: "Containers".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors: vec![SensorDescriptor {
                id: SensorId::new("containers.list"),
                display_name: "Containers (Docker / Podman)".to_string(),
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
                key: HardwareDeviceKey::try_new("system:containers")
                    .map_err(|e| PluginError::Manifest(e.to_string()))?,
                category: HardwareCategory::Other,
                model: "Containers".into(),
                vendor: None,
                location: None,
                plugin_id: PLUGIN_ID.into(),
                plugin_device_id: "containers".into(),
                sensor_ids: vec![SensorId::new("containers.list")],
            }],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        if sensor.as_str() != "containers.list" {
            return Err(PluginError::Unsupported(sensor.to_string()));
        }
        let mut inner = self.inner.lock().expect("ContainersPlugin poisoned");

        let cgroup_root = cgroup_root(inner.sysroot.as_deref());
        let containers = enumerate_containers(&cgroup_root, MAX_CONTAINERS);

        let mut rows: Vec<TableRow> = Vec::with_capacity(containers.len());
        let mut current_cpu: HashMap<String, u64> = HashMap::with_capacity(containers.len());
        let first_pass = inner.prev_cpu.is_empty();

        for c in &containers {
            let cpu = read_cpu_usage(&c.cgroup_path).unwrap_or(0);
            let mem = read_memory_current(&c.cgroup_path).unwrap_or(0);
            let pids = read_pids_current(&c.cgroup_path).unwrap_or(0);

            let delta = if first_pass {
                0
            } else {
                cpu.saturating_sub(inner.prev_cpu.get(&c.scope_name).copied().unwrap_or(0))
            };
            current_cpu.insert(c.scope_name.clone(), cpu);

            rows.push(TableRow {
                cells: vec![
                    Cell::Text(c.short_id.clone()),
                    Cell::Text(c.runtime.to_string()),
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

impl LinsightPlugin for ContainersPlugin {
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

const PLUGIN_ID: &str = "com.visorcraft.linsight.containers";

fn cgroup_root(sysroot: Option<&Path>) -> PathBuf {
    match sysroot {
        Some(r) => r.join("sys/fs/cgroup"),
        None => PathBuf::from("/sys/fs/cgroup"),
    }
}

fn empty_manifest() -> PluginManifest {
    PluginManifest {
        plugin_id: PLUGIN_ID.into(),
        display_name: "Containers".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        sensors: vec![],
        devices: vec![],
    }
}

/// Classify a cgroup scope directory name as a container, returning
/// `(runtime, short_id)`. Docker scopes are `docker-<id>.scope`; Podman
/// scopes are `libpod-<id>.scope`. The Podman conmon monitor process gets
/// its own `libpod-conmon-<id>.scope` sibling — it is not a container, so
/// it is skipped.
fn parse_container_scope(dirname: &str) -> Option<(&'static str, String)> {
    let stem = dirname.strip_suffix(".scope")?;
    if let Some(id) = stem.strip_prefix("docker-") {
        return Some(("docker", short_id(id)));
    }
    if let Some(rest) = stem.strip_prefix("libpod-") {
        if rest.starts_with("conmon-") {
            return None;
        }
        return Some(("podman", short_id(rest)));
    }
    None
}

fn short_id(id: &str) -> String {
    id.chars().take(SHORT_ID_LEN).collect()
}

/// Walk the container-bearing slices and collect every `.scope` that
/// classifies as a Docker or Podman container.
fn enumerate_containers(cgroup_root: &Path, max: usize) -> Vec<ContainerEntry> {
    let mut out = Vec::new();
    for slice in CONTAINER_SLICES {
        let slice_dir = cgroup_root.join(slice);
        let entries = match fs::read_dir(&slice_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if out.len() >= max {
                return out;
            }
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some((runtime, short_id)) = parse_container_scope(&name) {
                out.push(ContainerEntry {
                    runtime,
                    short_id,
                    scope_name: name,
                    cgroup_path: entry.path(),
                });
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    use linsight_plugin_sdk::{host_init, host_sample};

    /// Build a synthetic cgroup-v2 tree with one Docker container (under
    /// `system.slice`) and one Podman container (under `machine.slice`),
    /// plus a conmon sibling and a plain service that must be ignored.
    fn fake_cgroup() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let cgroup = dir.path().join("sys/fs/cgroup");

        let docker = cgroup.join("system.slice/docker-abc123def4567890aaaa.scope");
        fs::create_dir_all(&docker).unwrap();
        fs::write(docker.join("cpu.stat"), "usage_usec 1000000\nuser_usec 600000\n").unwrap();
        fs::write(docker.join("memory.current"), "33554432\n").unwrap();
        fs::write(docker.join("pids.current"), "7\n").unwrap();

        let podman = cgroup.join("machine.slice/libpod-99887766554433221100.scope");
        fs::create_dir_all(&podman).unwrap();
        fs::write(podman.join("cpu.stat"), "usage_usec 2000000\nuser_usec 1500000\n").unwrap();
        fs::write(podman.join("memory.current"), "16777216\n").unwrap();
        fs::write(podman.join("pids.current"), "3\n").unwrap();

        // conmon monitor — must be skipped.
        let conmon = cgroup.join("machine.slice/libpod-conmon-99887766554433221100.scope");
        fs::create_dir_all(&conmon).unwrap();
        fs::write(conmon.join("cpu.stat"), "usage_usec 10\n").unwrap();

        // plain systemd service — not a container.
        let svc = cgroup.join("system.slice/sshd.service");
        fs::create_dir_all(&svc).unwrap();
        fs::write(svc.join("cpu.stat"), "usage_usec 500\n").unwrap();

        dir
    }

    fn ctx_for(dir: &tempfile::TempDir) -> PluginCtx {
        PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap()
    }

    #[test]
    fn parse_docker_scope_extracts_runtime_and_short_id() {
        let got = parse_container_scope("docker-abc123def4567890aaaa.scope");
        assert_eq!(got, Some(("docker", "abc123def456".to_string())));
    }

    #[test]
    fn parse_podman_libpod_scope() {
        let got = parse_container_scope("libpod-99887766554433221100.scope");
        assert_eq!(got, Some(("podman", "998877665544".to_string())));
    }

    #[test]
    fn parse_skips_conmon_and_non_containers() {
        assert_eq!(parse_container_scope("libpod-conmon-99887766554433221100.scope"), None);
        assert_eq!(parse_container_scope("sshd.service"), None);
        assert_eq!(parse_container_scope("session-1.scope"), None);
    }

    #[test]
    fn enumerate_finds_docker_and_podman_containers() {
        let dir = fake_cgroup();
        let found = enumerate_containers(&cgroup_root(Some(dir.path())), MAX_CONTAINERS);
        let mut runtimes: Vec<&str> = found.iter().map(|c| c.runtime).collect();
        runtimes.sort_unstable();
        assert_eq!(runtimes, vec!["docker", "podman"], "expected exactly one docker + one podman");
    }

    #[test]
    fn init_advertises_containers_sensor() {
        let dir = fake_cgroup();
        let m = host_init(&ContainersPlugin::default(), &ctx_for(&dir)).unwrap();
        let ids: Vec<&str> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["containers.list"]);
        assert_eq!(m.devices.len(), 1);
        assert_eq!(m.devices[0].key.as_str(), "system:containers");
    }

    #[test]
    fn sample_returns_table_rows_for_each_container() {
        let dir = fake_cgroup();
        let p = ContainersPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let r = host_sample(&p, &SensorId::new("containers.list")).unwrap();
        match r {
            Reading::Table(rows) => {
                // docker + podman, conmon and the service excluded.
                assert_eq!(rows.len(), 2, "expected 2 containers, got {}", rows.len());
                // Sorted by short id: "998877665544" > "abc123def456"? '9' < 'a'
                // in ASCII, so the podman row sorts first.
                assert!(matches!(&rows[0].cells[0], Cell::Text(t) if t == "998877665544"));
                assert!(matches!(&rows[0].cells[1], Cell::Text(t) if t == "podman"));
                assert!(matches!(&rows[1].cells[0], Cell::Text(t) if t == "abc123def456"));
                assert!(matches!(&rows[1].cells[1], Cell::Text(t) if t == "docker"));
                // docker memory + pids
                assert!(matches!(&rows[1].cells[3], Cell::Bytes(b) if *b == 33554432));
                assert!(matches!(&rows[1].cells[4], Cell::Number(v) if *v == 7.0));
                // First pass: deltas zero.
                assert!(matches!(&rows[1].cells[2], Cell::Number(v) if *v == 0.0));
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn second_sample_shows_cpu_delta() {
        let dir = fake_cgroup();
        let p = ContainersPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let _ = host_sample(&p, &SensorId::new("containers.list")).unwrap();

        // Bump the docker container's CPU usage by 250000 usec.
        let docker =
            dir.path().join("sys/fs/cgroup/system.slice/docker-abc123def4567890aaaa.scope");
        fs::write(docker.join("cpu.stat"), "usage_usec 1250000\nuser_usec 700000\n").unwrap();

        let r = host_sample(&p, &SensorId::new("containers.list")).unwrap();
        match r {
            Reading::Table(rows) => {
                let docker_row = rows
                    .iter()
                    .find(|row| matches!(&row.cells[1], Cell::Text(t) if t == "docker"))
                    .expect("docker row");
                assert!(
                    matches!(&docker_row.cells[2], Cell::Number(v) if *v == 250000.0),
                    "expected 250000 usec delta, got {:?}",
                    docker_row.cells[2]
                );
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn no_cgroup_returns_empty_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        let m = host_init(&ContainersPlugin::default(), &ctx_for(&dir)).unwrap();
        assert!(m.sensors.is_empty());
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let dir = fake_cgroup();
        let p = ContainersPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let err = host_sample(&p, &SensorId::new("containers.bogus")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }
}
