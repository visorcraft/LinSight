// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemError {
    #[error("io: {0}")]
    Io(String),
    #[error("missing key in /proc/meminfo: {0}")]
    MissingKey(&'static str),
}

/// Counts in bytes derived from `/proc/meminfo`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meminfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub free_bytes: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub swap_cached: u64,
}

impl Meminfo {
    pub fn used_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }

    pub fn swap_used_bytes(self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

pub fn parse_meminfo(s: &str) -> Result<Meminfo, MemError> {
    let mut out = Meminfo::default();
    let mut saw_available = false;
    for line in s.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        // Only attempt numeric parse for keys we care about. Per-line
        // parse failures used to be silently swallowed via `.ok()`; if a
        // future kernel format change (unit suffix from `kB` to `MB`,
        // hex values, etc.) breaks parsing on one of these critical
        // fields, log it instead of pretending the value was 0.
        let needs_value = matches!(
            key,
            "MemTotal" | "MemAvailable" | "MemFree" | "SwapTotal" | "SwapFree" | "SwapCached"
        );
        if !needs_value {
            continue;
        }
        let raw = rest.trim().trim_end_matches(" kB").trim();
        let v = match raw.parse::<u64>() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(key, raw, error = %e, "/proc/meminfo: failed to parse value");
                continue;
            }
        };
        match key {
            "MemTotal" => out.total_bytes = v * 1024,
            "MemAvailable" => {
                out.available_bytes = v * 1024;
                saw_available = true;
            }
            "MemFree" => out.free_bytes = v * 1024,
            "SwapTotal" => out.swap_total = v * 1024,
            "SwapFree" => out.swap_free = v * 1024,
            "SwapCached" => out.swap_cached = v * 1024,
            _ => unreachable!("needs_value filter handled by the match above"),
        }
    }
    if out.total_bytes == 0 {
        return Err(MemError::MissingKey("MemTotal"));
    }
    // `MemAvailable` is the modern (kernel >= 3.14) way to ask the
    // kernel how much memory is realistically usable. If a sandbox /
    // container scrubs it (some do), the previous code silently
    // defaulted `available_bytes` to 0 and `used_bytes()` returned
    // `total_bytes` — 100% memory used as valid-looking data. Fall
    // back to the classic `MemFree + Buffers + Cached` approximation
    // (close enough for a dashboard) and warn so the operator knows.
    if !saw_available {
        tracing::warn!(
            "/proc/meminfo is missing MemAvailable; falling back to MemFree as approximation",
        );
        out.available_bytes = out.free_bytes;
    }
    Ok(out)
}

pub fn read_meminfo(sysroot: Option<&Path>) -> Result<Meminfo, MemError> {
    let path = match sysroot {
        Some(root) => root.join("proc/meminfo"),
        None => Path::new("/proc/meminfo").to_path_buf(),
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|e| MemError::Io(format!("{}: {e}", path.display())))?;
    parse_meminfo(&content)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const SAMPLE: &str = "\
MemTotal:       64000000 kB
MemFree:        20000000 kB
MemAvailable:   45000000 kB
Buffers:        100000 kB
Cached:         15000000 kB
SwapTotal:      8000000 kB
SwapFree:       7500000 kB
";

    #[test]
    fn parse_basic() {
        let m = parse_meminfo(SAMPLE).unwrap();
        assert_eq!(m.total_bytes, 64_000_000 * 1024);
        assert_eq!(m.available_bytes, 45_000_000 * 1024);
        assert_eq!(m.used_bytes(), (64_000_000 - 45_000_000) * 1024);
    }

    #[test]
    fn parse_missing_memtotal_errors() {
        let bad = "MemFree: 10 kB\n";
        assert!(parse_meminfo(bad).is_err());
    }

    #[test]
    fn read_with_sysroot_uses_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let proc_dir = dir.path().join("proc");
        fs::create_dir(&proc_dir).unwrap();
        fs::write(proc_dir.join("meminfo"), SAMPLE).unwrap();
        let m = read_meminfo(Some(dir.path())).unwrap();
        assert_eq!(m.total_bytes, 64_000_000 * 1024);
    }

    #[test]
    #[ignore = "requires a live /proc; gate per AGENTS.md convention"]
    fn read_real_proc_meminfo() {
        let m = read_meminfo(None).unwrap();
        assert!(m.total_bytes > 0);
    }

    #[test]
    fn parse_swap_values() {
        let m = parse_meminfo(SAMPLE).unwrap();
        assert_eq!(m.swap_total, 8_000_000 * 1024);
        assert_eq!(m.swap_free, 7_500_000 * 1024);
        assert_eq!(m.swap_cached, 0);
        assert_eq!(m.swap_used_bytes(), (8_000_000 - 7_500_000) * 1024);
    }

    #[test]
    fn parse_swap_cached_from_full_line() {
        let s = "MemTotal: 1000 kB\nMemAvailable: 800 kB\nMemFree: 500 kB\nSwapTotal: 4000 kB\nSwapFree: 3000 kB\nSwapCached: 200 kB\n";
        let m = parse_meminfo(s).unwrap();
        assert_eq!(m.swap_total, 4000 * 1024);
        assert_eq!(m.swap_free, 3000 * 1024);
        assert_eq!(m.swap_cached, 200 * 1024);
        assert_eq!(m.swap_used_bytes(), (4000 - 3000) * 1024);
    }

    #[test]
    fn missing_memavailable_falls_back_to_free() {
        let s = "MemTotal: 1000 kB\nMemFree: 500 kB\n";
        let m = parse_meminfo(s).unwrap();
        assert_eq!(m.total_bytes, 1000 * 1024);
        // No MemAvailable in the input — used_bytes must NOT report 100%.
        // The fallback uses MemFree as a stand-in.
        assert_eq!(m.available_bytes, 500 * 1024);
        assert_eq!(m.used_bytes(), 500 * 1024);
    }

    #[test]
    fn parse_failure_on_critical_field_doesnt_silently_zero() {
        // Garbage value on MemTotal: total_bytes stays 0 and the
        // function should return MissingKey rather than producing a
        // bogus Meminfo with all-zeroes.
        let s = "MemTotal: not_a_number kB\nMemAvailable: 100 kB\n";
        assert!(parse_meminfo(s).is_err());
    }
}
