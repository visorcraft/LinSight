// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StatError {
    #[error("missing aggregate cpu line")]
    MissingAggregate,
    #[error("aggregate cpu line too short (need ≥ 8 fields)")]
    TooShort,
    #[error("non-numeric field in cpu line: {0}")]
    BadNumber(String),
    #[error("io: {0}")]
    Io(String),
}

/// Parsed counters from the aggregate `cpu` line of `/proc/stat`.
/// All values are clock ticks since boot (USER_HZ).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl Stat {
    pub fn total(self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    pub fn busy(self) -> u64 {
        self.total() - self.idle - self.iowait
    }
}

/// Parsed counters from a per-core `cpuN` line of `/proc/stat`.
/// Fields are identical to [`Stat`]; kept as a separate type for type-safety
/// in the plugin's per-core state tracking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreStat {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CoreStat {
    pub fn total(self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    pub fn busy(self) -> u64 {
        self.total() - self.idle - self.iowait
    }
}

impl From<Stat> for CoreStat {
    fn from(s: Stat) -> Self {
        Self {
            user: s.user,
            nice: s.nice,
            system: s.system,
            idle: s.idle,
            iowait: s.iowait,
            irq: s.irq,
            softirq: s.softirq,
            steal: s.steal,
        }
    }
}

pub fn parse_proc_stat(s: &str) -> Result<Stat, StatError> {
    let line = s.lines().next().ok_or(StatError::MissingAggregate)?;
    let mut it = line.split_whitespace();
    let first = it.next().ok_or(StatError::MissingAggregate)?;
    if first != "cpu" {
        return Err(StatError::MissingAggregate);
    }
    let parse_field = |it: &mut std::str::SplitWhitespace<'_>| -> Result<u64, StatError> {
        let tok = it.next().ok_or(StatError::TooShort)?;
        tok.parse::<u64>().map_err(|_| StatError::BadNumber(tok.into()))
    };
    Ok(Stat {
        user: parse_field(&mut it)?,
        nice: parse_field(&mut it)?,
        system: parse_field(&mut it)?,
        idle: parse_field(&mut it)?,
        iowait: parse_field(&mut it)?,
        irq: parse_field(&mut it)?,
        softirq: parse_field(&mut it)?,
        steal: parse_field(&mut it)?,
        // We intentionally stop at field 8 (steal). Modern kernels
        // (>= 2.6.24) also emit `guest` and `guest_nice` here, but those
        // are already double-counted inside `user` and `nice` per
        // `Documentation/filesystems/proc.rst`. Summing them would
        // overstate busy time on hosts running VMs.
    })
}

/// Parse a per-core `cpuN` line. Expects the caller to have identified
/// the line as a per-core entry.
pub fn parse_core_stat(line: &str) -> Result<CoreStat, StatError> {
    let mut it = line.split_whitespace();
    // Skip the cpuN token — caller has already identified the line.
    let _first = it.next().ok_or(StatError::MissingAggregate)?;
    let parse_field = |it: &mut std::str::SplitWhitespace<'_>| -> Result<u64, StatError> {
        let tok = it.next().ok_or(StatError::TooShort)?;
        tok.parse::<u64>().map_err(|_| StatError::BadNumber(tok.into()))
    };
    Ok(CoreStat {
        user: parse_field(&mut it)?,
        nice: parse_field(&mut it)?,
        system: parse_field(&mut it)?,
        idle: parse_field(&mut it)?,
        iowait: parse_field(&mut it)?,
        irq: parse_field(&mut it)?,
        softirq: parse_field(&mut it)?,
        steal: parse_field(&mut it)?,
    })
}

/// Compute CPU utilization (0..=100) between two `/proc/stat` samples.
pub fn util_between(a: Stat, b: Stat) -> f64 {
    let dt = b.total().saturating_sub(a.total());
    if dt == 0 {
        return 0.0;
    }
    let db = b.busy().saturating_sub(a.busy());
    100.0 * (db as f64) / (dt as f64)
}

/// Compute per-core utilization (0..=100) between two samples.
pub fn core_util_between(a: CoreStat, b: CoreStat) -> f64 {
    let dt = b.total().saturating_sub(a.total());
    if dt == 0 {
        return 0.0;
    }
    let db = b.busy().saturating_sub(a.busy());
    100.0 * (db as f64) / (dt as f64)
}

pub fn read_proc_stat(sysroot: Option<&Path>) -> Result<Stat, StatError> {
    let path = match sysroot {
        Some(root) => root.join("proc/stat"),
        None => Path::new("/proc/stat").to_path_buf(),
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|e| StatError::Io(format!("reading {}: {e}", path.display())))?;
    parse_proc_stat(&content)
}

/// Read `/proc/stat` and return all per-core (`cpu0`, `cpu1`, …) entries.
/// The returned list is sorted by core index. The aggregate `cpu` line is skipped.
/// Parse `/proc/stat` content and return all per-core (`cpu0`, `cpu1`, …) entries.
/// The returned list is sorted by core index. The aggregate `cpu` line is skipped.
pub fn parse_core_stats(s: &str) -> Result<Vec<(u32, CoreStat)>, StatError> {
    let mut cores: Vec<(u32, CoreStat)> = Vec::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("cpu") {
            // Skip the aggregate `cpu` (no digit after "cpu").
            if let Some(digits) = rest.strip_prefix(|c: char| c.is_ascii_digit()) {
                // Must be followed by whitespace or end-of-line (skip things like "cpuidle")
                if !digits.is_empty() && !digits.starts_with(|c: char| c.is_ascii_whitespace()) {
                    continue;
                }
                let idx: u32 = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| StatError::BadNumber(trimmed.into()))?;
                let stat = parse_core_stat(trimmed)?;
                cores.push((idx, stat));
            }
        }
    }
    cores.sort_by_key(|&(idx, _)| idx);
    Ok(cores)
}

pub fn read_proc_core_stats(sysroot: Option<&Path>) -> Result<Vec<(u32, CoreStat)>, StatError> {
    let path = match sysroot {
        Some(root) => root.join("proc/stat"),
        None => Path::new("/proc/stat").to_path_buf(),
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|e| StatError::Io(format!("reading {}: {e}", path.display())))?;
    parse_core_stats(&content)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const SAMPLE: &str = "\
cpu  100 0 50 1000 0 0 0 0 0 0
cpu0 50 0 25 500 0 0 0 0 0 0
cpu1 50 0 25 500 0 0 0 0 0 0
intr 1234567
ctxt 8910
btime 1700000000
processes 5000
procs_running 1
procs_blocked 0
";

    #[test]
    fn parse_aggregate_line() {
        let s = parse_proc_stat(SAMPLE).unwrap();
        assert_eq!(s.user, 100);
        assert_eq!(s.system, 50);
        assert_eq!(s.idle, 1000);
        assert_eq!(s.total(), 1150);
        assert_eq!(s.busy(), 150);
    }

    #[test]
    fn parse_missing_aggregate_errors() {
        let s = "cpu0 50 0 25 500 0 0 0 0 0 0\n";
        assert!(parse_proc_stat(s).is_err());
    }

    #[test]
    fn parse_short_aggregate_errors() {
        let s = "cpu 100 0\n";
        assert!(parse_proc_stat(s).is_err());
    }

    #[test]
    fn util_returns_percent_busy() {
        let a = Stat { user: 100, idle: 100, ..Stat::default() };
        let b = Stat { user: 200, idle: 100, ..Stat::default() };
        assert_eq!(util_between(a, b), 100.0);
    }

    #[test]
    fn util_clamped_when_idle_only() {
        let a = Stat { idle: 100, ..Stat::default() };
        let b = Stat { idle: 200, ..Stat::default() };
        assert_eq!(util_between(a, b), 0.0);
    }

    #[test]
    fn util_zero_when_no_time_elapsed() {
        let a = Stat { user: 10, idle: 10, ..Stat::default() };
        assert_eq!(util_between(a, a), 0.0);
    }

    #[test]
    fn read_with_sysroot_uses_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let proc_dir = dir.path().join("proc");
        fs::create_dir(&proc_dir).unwrap();
        fs::write(proc_dir.join("stat"), "cpu 1 2 3 4 5 6 7 8\n").unwrap();

        let stat = read_proc_stat(Some(dir.path())).unwrap();
        assert_eq!(stat.user, 1);
        assert_eq!(stat.system, 3);
    }

    #[test]
    #[ignore = "requires a live /proc; gate per AGENTS.md convention"]
    fn read_with_no_sysroot_reads_real_proc() {
        let stat = read_proc_stat(None).unwrap();
        assert!(stat.total() > 0);
    }

    // -----------------------------------------------------------------------
    // CoreStat / per-core tests
    // -----------------------------------------------------------------------

    const MULTI_CORE_SAMPLE: &str = "\
cpu  200 0 100 2000 0 0 0 0 0 0
cpu0 100 0 50 1000 0 0 0 0 0 0
cpu1 60 0 30 800 0 0 0 0 0 0
cpu2 30 0 15 150 0 0 0 0 0 0
cpu3 10 0 5 50 0 0 0 0 0 0
intr 1234567
ctxt 8910
btime 1700000000
processes 5000
procs_running 1
procs_blocked 0
";

    #[test]
    fn parse_core_line_cpu0() {
        let c = parse_core_stat("cpu0 100 0 50 1000 0 0 0 0 0 0").unwrap();
        assert_eq!(c.user, 100);
        assert_eq!(c.system, 50);
        assert_eq!(c.idle, 1000);
        assert_eq!(c.total(), 1150);
        assert_eq!(c.busy(), 150);
    }

    #[test]
    fn parse_core_line_cpu3() {
        let c = parse_core_stat("cpu3 10 0 5 50 0 0 0 0 0 0").unwrap();
        assert_eq!(c.user, 10);
        assert_eq!(c.idle, 50);
        assert_eq!(c.total(), 65);
    }

    #[test]
    fn parse_core_non_numeric_errors() {
        assert!(parse_core_stat("cpu0 x 0 0 0 0 0 0 0").is_err());
    }

    #[test]
    fn parse_core_short_line_errors() {
        assert!(parse_core_stat("cpu0 100 0").is_err());
    }

    #[test]
    fn read_proc_core_stats_returns_all_cores() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("proc")).unwrap();
        fs::write(dir.path().join("proc/stat"), MULTI_CORE_SAMPLE).unwrap();

        let cores = read_proc_core_stats(Some(dir.path())).unwrap();
        assert_eq!(cores.len(), 4);
        assert_eq!(cores[0].0, 0);
        assert_eq!(cores[1].0, 1);
        assert_eq!(cores[2].0, 2);
        assert_eq!(cores[3].0, 3);
        assert_eq!(cores[0].1.user, 100);
        assert_eq!(cores[2].1.system, 15);
        assert_eq!(cores[3].1.idle, 50);
    }

    #[test]
    fn read_proc_core_stats_skips_aggregate_no_core_lines() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("proc")).unwrap();
        // Only aggregate cpu line — no per-core entries.
        fs::write(dir.path().join("proc/stat"), "cpu 1 2 3 4 5 6 7 8\n").unwrap();

        let cores = read_proc_core_stats(Some(dir.path())).unwrap();
        assert!(cores.is_empty());
    }

    #[test]
    fn parse_multi_core_lines() {
        let cores = parse_core_stats(MULTI_CORE_SAMPLE).unwrap();
        assert_eq!(cores.len(), 4);
        assert_eq!(cores[0].0, 0);
        assert_eq!(cores[1].0, 1);
        assert_eq!(cores[2].0, 2);
        assert_eq!(cores[3].0, 3);
        assert_eq!(cores[0].1.user, 100);
        assert_eq!(cores[0].1.system, 50);
        assert_eq!(cores[0].1.idle, 1000);
        assert_eq!(cores[3].1.user, 10);
        assert_eq!(cores[3].1.idle, 50);
    }

    #[test]
    fn parse_core_empty_when_no_per_cpu() {
        let s = "cpu 100 0 50 1000 0 0 0 0 0 0\n";
        let cores = parse_core_stats(s).unwrap();
        assert!(cores.is_empty());
    }

    #[test]
    fn core_util_returns_percent_busy() {
        let a = CoreStat { user: 100, idle: 100, ..CoreStat::default() };
        let b = CoreStat { user: 200, idle: 100, ..CoreStat::default() };
        assert_eq!(core_util_between(a, b), 100.0);
    }

    #[test]
    fn core_util_zero_when_no_time_elapsed() {
        let a = CoreStat { user: 10, idle: 10, ..CoreStat::default() };
        assert_eq!(core_util_between(a, a), 0.0);
    }

    #[test]
    fn core_stat_from_stat() {
        let s =
            Stat { user: 1, nice: 2, system: 3, idle: 4, iowait: 5, irq: 6, softirq: 7, steal: 8 };
        let c: CoreStat = s.into();
        assert_eq!(c.user, 1);
        assert_eq!(c.nice, 2);
        assert_eq!(c.system, 3);
        assert_eq!(c.idle, 4);
        assert_eq!(c.iowait, 5);
        assert_eq!(c.irq, 6);
        assert_eq!(c.softirq, 7);
        assert_eq!(c.steal, 8);
    }
}
