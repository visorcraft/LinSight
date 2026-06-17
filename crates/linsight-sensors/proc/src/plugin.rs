// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Process/Top monitoring sensor backend.
//!
//! Emits a `Reading::Table` of processes sorted by CPU usage.
//! Columns: pid, name, cpu%, mem%, rss_bytes, threads, state.
//!
//! Sensor id: `proc.list` — one-shot table snapshot.
//! Default rate: 0.2 Hz (5-second gap) to avoid /proc pressure.
//!
//! CPU% is computed by diffing `/proc/<pid>/stat` utime+stime against a
//! cached snapshot from the previous sample.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use linsight_core::{Category, Cell, Reading, SensorId, SensorKind, TableRow, Unit};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

/// How many pids to read at most per sample. Prevents pathological
/// stalls on systems with 10K+ threads.
const MAX_PIDS_PER_SAMPLE: usize = 4096;

pub struct ProcPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    /// Previous tick's CPU totals (pid → (utime+stime, total_jiffies))
    prev: Option<CpuSnapshot>,
    /// Cumulative total boot jiffies from `/proc/stat` at prev snapshot.
    prev_total_jiffies: u64,
    /// Total number of online CPUs (for normalizing % util).
    num_cpus: u32,
}

struct CpuSnapshot {
    /// pid → (cpu_jiffies, total_jiffies_at_time)
    by_pid: HashMap<u32, (u64, u64)>,
}

impl Default for ProcPlugin {
    fn default() -> Self {
        Self { inner: Mutex::new(Inner::default()) }
    }
}

impl ProcPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("ProcPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        inner.prev = None;
        inner.prev_total_jiffies = 0;
        inner.num_cpus = num_cpus(ctx.sysroot()).unwrap_or(1).max(1);

        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.proc".into(),
            display_name: "Process List".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors: vec![SensorDescriptor {
                id: SensorId::new("proc.list"),
                display_name: "Process list".to_string(),
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
            devices: vec![],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        if sensor.as_str() != "proc.list" {
            return Err(PluginError::Unsupported(sensor.to_string()));
        }
        let mut inner = self.inner.lock().expect("ProcPlugin poisoned");

        let total_jiffies = total_cpu_jiffies(inner.sysroot.as_deref())?;
        let proc_root = match &inner.sysroot {
            Some(r) => r.join("proc"),
            None => PathBuf::from("/proc"),
        };

        // Enumerate pids
        let pids = enumerate_pids(&proc_root, MAX_PIDS_PER_SAMPLE);
        let mut procs: Vec<ProcessInfo> = Vec::with_capacity(pids.len());
        let mut current: HashMap<u32, (u64, u64)> = HashMap::with_capacity(pids.len());

        // Read MemTotal once — it's constant across the loop
        let mem_total_kb = total_ram_kb(Some(&proc_root))?;

        for pid in &pids {
            let stat_path = proc_root.join(pid.to_string()).join("stat");
            let status_path = proc_root.join(pid.to_string()).join("status");
            match read_process_info(&stat_path, &status_path, mem_total_kb) {
                Ok(info) => {
                    let cpu_jiffies = info.cpu_jiffies;
                    let cpu_pct = inner
                        .prev
                        .as_ref()
                        .and_then(|prev| {
                            prev.by_pid.get(pid).map(|(prev_jiffies, prev_total)| {
                                let delta_jiffies = cpu_jiffies.saturating_sub(*prev_jiffies);
                                let delta_total = total_jiffies.saturating_sub(*prev_total);
                                if delta_total == 0 {
                                    0.0_f64
                                } else {
                                    (delta_jiffies as f64 / delta_total as f64)
                                        * 100.0
                                        * inner.num_cpus as f64
                                }
                            })
                        })
                        .unwrap_or(0.0);
                    current.insert(*pid, (cpu_jiffies, total_jiffies));
                    procs.push(ProcessInfo {
                        pid: *pid,
                        comm: info.comm,
                        cpu_pct,
                        mem_pct: info.mem_pct,
                        rss_bytes: info.rss_bytes,
                        threads: info.threads,
                        state: info.state,
                        cpu_jiffies,
                    });
                }
                Err(_) => {
                    // Process vanished between readdir and stat — skip
                }
            }
        }

        // Sort by CPU descending
        procs
            .sort_by(|a, b| b.cpu_pct.partial_cmp(&a.cpu_pct).unwrap_or(std::cmp::Ordering::Equal));

        inner.prev = Some(CpuSnapshot { by_pid: current });
        inner.prev_total_jiffies = total_jiffies;

        let rows: Vec<TableRow> = procs
            .into_iter()
            .map(|p| TableRow {
                cells: vec![
                    Cell::Number(p.pid as f64),
                    Cell::Text(p.comm),
                    Cell::Number(p.cpu_pct),
                    Cell::Number(p.mem_pct),
                    Cell::Bytes(p.rss_bytes),
                    Cell::Number(p.threads as f64),
                    Cell::Text(p.state),
                ],
            })
            .collect();

        Ok(Reading::Table(rows))
    }
}

impl LinsightPlugin for ProcPlugin {
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
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ProcessInfo {
    pid: u32,
    comm: String,
    cpu_pct: f64,
    mem_pct: f64,
    rss_bytes: u64,
    threads: u64,
    state: String,
    cpu_jiffies: u64,
}

/// Read /proc/stat's aggregate CPU line and sum the first 8 fields to get
/// total jiffies since boot.
fn total_cpu_jiffies(sysroot: Option<&Path>) -> Result<u64, PluginError> {
    let path = match sysroot {
        Some(r) => r.join("proc/stat"),
        None => PathBuf::from("/proc/stat"),
    };
    let s = fs::read_to_string(&path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            let total: u64 =
                rest.split_whitespace().take(8).filter_map(|f| f.parse::<u64>().ok()).sum();
            return Ok(total);
        }
    }
    Err(PluginError::Parse(format!("{}: no cpu line", path.display())))
}

/// Count online CPUs from /proc/stat's cpuN lines.
fn num_cpus(sysroot: Option<&Path>) -> Result<u32, PluginError> {
    let path = match sysroot {
        Some(r) => r.join("proc/stat"),
        None => PathBuf::from("/proc/stat"),
    };
    let s = fs::read_to_string(&path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    let count = s
        .lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(|c| c.is_ascii_digit()))
        .count() as u32;
    Ok(count.max(1))
}

fn enumerate_pids(proc_root: &Path, max: usize) -> Vec<u32> {
    let mut pids = Vec::new();
    let entries = match fs::read_dir(proc_root) {
        Ok(e) => e,
        Err(_) => return pids,
    };
    for entry in entries.flatten() {
        if pids.len() >= max {
            break;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(pid) = name_str.parse::<u32>() {
            if pid == 0 {
                continue;
            }
            pids.push(pid);
        }
    }
    pids.sort();
    pids
}

/// Read `/proc/<pid>/stat` fields and `/proc/<pid>/status` for RSS/threads.
/// Returns ProcessInfo or error if the process vanished.
fn read_process_info(
    stat_path: &Path,
    status_path: &Path,
    mem_total_kb: u64,
) -> Result<ProcessInfo, PluginError> {
    let stat_content =
        fs::read_to_string(stat_path).map_err(|_| PluginError::Transient("process gone".into()))?;

    // Parse /proc/<pid>/stat: comm is in parens; everything after is space-separated.
    // Format: pid (comm) state ppid ... utime stime ...
    let comm_end = stat_content
        .rfind(')')
        .ok_or_else(|| PluginError::Parse("no closing paren in stat".into()))?;
    let comm = &stat_content[stat_content
        .find('(')
        .ok_or_else(|| PluginError::Parse("no opening paren in stat".into()))?
        + 1..comm_end];
    let fields: Vec<&str> = stat_content[comm_end + 2..].split_whitespace().collect();
    if fields.len() < 15 {
        return Err(PluginError::Parse("stat: too few fields".into()));
    }
    let state = fields[0].to_string();
    let utime_jf: u64 = fields[11].parse().unwrap_or(0);
    let stime_jf: u64 = fields[12].parse().unwrap_or(0);

    // Read /proc/<pid>/status for VmRSS, Threads
    let status_content = fs::read_to_string(status_path)
        .map_err(|_| PluginError::Transient("process gone".into()))?;

    let mut rss_kb: u64 = 0;
    let mut threads: u64 = 0;
    for line in status_content.lines() {
        if let Some(val) = line.strip_prefix("VmRSS:") {
            rss_kb = val.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("Threads:") {
            threads = val.trim().parse::<u64>().unwrap_or(0);
        }
    }

    // MemTotal is provided by the caller
    let mem_pct =
        if mem_total_kb > 0 { (rss_kb as f64 / mem_total_kb as f64) * 100.0 } else { 0.0 };

    Ok(ProcessInfo {
        pid: extract_pid_from_stat(&stat_content)?,
        comm: comm.to_owned(),
        cpu_pct: 0.0, // computed later by caller
        mem_pct,
        rss_bytes: rss_kb * 1024,
        threads,
        state,
        cpu_jiffies: utime_jf + stime_jf,
    })
}

fn extract_pid_from_stat(content: &str) -> Result<u32, PluginError> {
    let first_space =
        content.find(' ').ok_or_else(|| PluginError::Parse("stat: no space".into()))?;
    content[..first_space].parse::<u32>().map_err(|_| PluginError::Parse("stat: bad pid".into()))
}

fn total_ram_kb(proc_dir: Option<&Path>) -> Result<u64, PluginError> {
    let path = match proc_dir {
        Some(r) => r.join("meminfo"),
        None => PathBuf::from("/proc/meminfo"),
    };
    let content = fs::read_to_string(&path)
        .map_err(|e| PluginError::Io(format!("{}: {e}", path.display())))?;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("MemTotal:") {
            return val
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| PluginError::Parse("MemTotal parse".into()));
        }
    }
    Err(PluginError::Parse("MemTotal not found".into()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use linsight_plugin_sdk::{host_init, host_sample};

    use super::*;

    fn fake_proc() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let proc_dir = dir.path().join("proc");
        fs::create_dir_all(&proc_dir).unwrap();

        // /proc/stat
        fs::write(
            proc_dir.join("stat"),
            "cpu  1000 200 300 400 0 0 0 0 0 0\n\
             cpu0 500 100 150 200 0 0 0 0 0 0\n\
             cpu1 500 100 150 200 0 0 0 0 0 0\n\
             intr 1234\n\
             ctxt 5678\n\
             btime 1700000000\n\
             processes 100\n",
        )
        .unwrap();

        // /proc/meminfo
        fs::write(
            proc_dir.join("meminfo"),
            "MemTotal:     16777216 kB\n\
             MemFree:      8388608 kB\n\
             MemAvailable: 4194304 kB\n",
        )
        .unwrap();

        // PID 1: init
        let pid1 = proc_dir.join("1");
        fs::create_dir_all(&pid1).unwrap();
        fs::write(pid1.join("stat"), "1 (systemd) S 0 1 1 0 -1 4194560 123 0 0 0 50 30 0 0 20 0 1 0 100 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n").unwrap();
        fs::write(
            pid1.join("status"),
            "Name:	systemd\n\
             State:	S (sleeping)\n\
             Threads:	1\n\
             VmRSS:	   4096 kB\n\
             VmSize:	160000 kB\n",
        )
        .unwrap();

        // PID 42: a busy process (firefox)
        let pid42 = proc_dir.join("42");
        fs::create_dir_all(&pid42).unwrap();
        fs::write(pid42.join("stat"), "42 (firefox) R 1 42 42 0 -1 4194304 456 0 0 0 200 100 0 0 20 0 8 0 200 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n").unwrap();
        fs::write(
            pid42.join("status"),
            "Name:	firefox\n\
             State:	R (running)\n\
             Threads:	8\n\
             VmRSS:	  512000 kB\n\
             VmSize:	4000000 kB\n",
        )
        .unwrap();

        dir
    }

    #[test]
    fn init_advertises_proc_list() {
        let dir = fake_proc();
        let plugin = ProcPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        let ids: Vec<&str> = manifest.sensors.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"proc.list"));
    }

    #[test]
    fn enumerate_pids_discovers_synthetic_proc() {
        let dir = fake_proc();
        let pids = enumerate_pids(&dir.path().join("proc"), 100);
        assert!(pids.contains(&1));
        assert!(pids.contains(&42));
    }

    #[test]
    fn sample_proc_list_returns_table() {
        let dir = fake_proc();
        let plugin = ProcPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        let r = host_sample(&plugin, &SensorId::new("proc.list")).unwrap();
        match r {
            Reading::Table(rows) => {
                // Should have at least 2 rows (pid 1 and 42)
                assert!(rows.len() >= 2, "expected ≥2 rows, got {}", rows.len());
                // First sample: all CPU% are 0 (no prev snapshot)
                for row in &rows {
                    assert_eq!(row.cells.len(), 7, "expected 7 columns, got {}", row.cells.len());
                }
                // Check pid 42 (firefox) has comm "firefox"
                let firefox = rows
                    .iter()
                    .find(|r| matches!(&r.cells[0], linsight_core::Cell::Number(v) if *v == 42.0));
                assert!(firefox.is_some(), "expected pid 42 row");
                if let Some(fx) = firefox {
                    assert!(matches!(&fx.cells[1], linsight_core::Cell::Text(t) if t == "firefox"));
                    // VmRSS 512000 kB → 512000 * 1024 bytes
                    assert!(
                        matches!(&fx.cells[4], linsight_core::Cell::Bytes(b) if *b == 512000 * 1024)
                    );
                }
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn second_sample_shows_cpu_diff() {
        let dir = fake_proc();
        let plugin = ProcPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();

        // First sample: populates prev
        let _ = host_sample(&plugin, &SensorId::new("proc.list")).unwrap();

        // We can't observe CPU% change in a static fixture, but the
        // second call should succeed and return a table.
        let r = host_sample(&plugin, &SensorId::new("proc.list")).unwrap();
        assert!(matches!(r, Reading::Table(_)));
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let dir = fake_proc();
        let plugin = ProcPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        host_init(&plugin, &ctx).unwrap();
        let err = host_sample(&plugin, &SensorId::new("nope.nope")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }
}
