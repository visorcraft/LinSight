// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! DRM fdinfo aggregator for xe-driver GPUs.
//!
//! Walks `/proc/<pid>/fdinfo/<fd>` and collects the kernel's
//! `drm-usage-stats` accounting (`Documentation/gpu/drm-usage-stats.rst`)
//! to compute true per-engine utilization. The xe driver exposes the
//! *cycles* variant:
//!
//! * `drm-cycles-<class>` — engine cycles spent running THIS client's work
//! * `drm-total-cycles-<class>` — the engine's own wall-clock-equivalent
//!   cycle counter (effectively shared per-pdev+class; each fd-read
//!   samples it at the read instant so values drift a few µs between
//!   reads — we take the max as the latest)
//!
//! `idle_residency_ms` was the previous source for utilization but it
//! only ticks while the GT is in the gt-c6 deep-idle power state, so a
//! GPU parked in gt-c0 looks 100% busy regardless of actual work. This
//! module replaces that signal entirely.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Per-pdev snapshot of all xe-client cycle counters at a moment in time.
#[derive(Default, Clone, Debug)]
pub struct PdevSnapshot {
    /// Per-client busy cycles, keyed by `(drm-client-id, engine_class)`.
    /// Aggregated across every fd a process holds open against the same
    /// drm client (they all carry the same drm-client-id and identical
    /// per-client busy counters, so re-reading is harmless).
    pub busy: HashMap<(u64, String), u64>,
    /// Engine's wall-clock-equivalent cycle counter per class. Same
    /// physical counter regardless of client, so we keep the highest
    /// value observed in this scan.
    pub total: HashMap<String, u64>,
}

/// Map of `pid -> list of fd numbers known to point at /dev/dri/*`.
/// Built at full-rescan time via readlink on every /proc/<pid>/fd/<n>;
/// the fast path then reads ONLY those specific fdinfos (typically 1-2
/// per pid) instead of every fdinfo in the directory. Saves ~95% of
/// the fdinfo file reads on a host with many-fd processes (browsers,
/// IDEs, compositors).
pub type DrmFdIndex = HashMap<u32, Vec<u32>>;

/// Two-phase fdinfo walk:
///
///   * `pid_index = None` (full rescan): readlink every /proc/<pid>/fd/<n>
///     to find DRM fds, parse their fdinfo, and return the index of
///     (pid, fd-list) entries to feed back into the fast path.
///   * `pid_index = Some(index)`: hot path. For every (pid, fd-list)
///     known to be DRM, read only those specific fdinfo files. A
///     pid that disappeared between scans is skipped (ENOENT); an
///     fd that closed reads as ENOENT and is silently dropped.
///
/// Returns `(snapshot, refreshed_index)`. The caller stashes
/// `refreshed_index` for the next fast pass — entries are recomputed
/// every scan so a pid that closed its DRM fd drops off the index
/// without waiting for the next full rescan.
pub fn capture_all_filtered(
    sysroot: Option<&Path>,
    pid_index: Option<&DrmFdIndex>,
) -> (HashMap<String, PdevSnapshot>, DrmFdIndex) {
    let proc_root = match sysroot {
        Some(r) => r.join("proc"),
        None => PathBuf::from("/proc"),
    };
    let mut snap: HashMap<String, PdevSnapshot> = HashMap::new();
    let mut next_index: DrmFdIndex = HashMap::new();

    match pid_index {
        Some(index) => {
            // Fast path: only read the SPECIFIC fdinfos already known
            // to be DRM. Saves the readlink loop and skips every fd
            // that isn't pointed at /dev/dri/*.
            for (&pid, fds) in index {
                read_known_drm_fdinfos(&proc_root, pid, fds, &mut snap, &mut next_index);
            }
        }
        None => {
            // Full rescan: enumerate every PID, readlink each fd to
            // discover which are DRM, read those fdinfos, build the
            // index. Expensive but throttled (FDINFO_FULL_RESCAN_INTERVAL).
            let Ok(pids) = fs::read_dir(&proc_root) else {
                return (snap, next_index);
            };
            for pid_ent in pids.flatten() {
                let name = pid_ent.file_name();
                let Some(n) = name.to_str() else { continue };
                let Ok(pid) = n.parse::<u32>() else { continue };
                discover_pid_drm_fds(&proc_root, pid, &mut snap, &mut next_index);
            }
        }
    }
    (snap, next_index)
}

/// Full-rescan helper. Readlinks every /proc/<pid>/fd/<n>; for each
/// link target that points at /dev/dri/*, reads the matching fdinfo
/// and records (pid, fd) in `index` so the next fast scan reads just
/// that pair. Silent on unreadable / vanished entries.
fn discover_pid_drm_fds(
    proc_root: &Path,
    pid: u32,
    snap: &mut HashMap<String, PdevSnapshot>,
    index: &mut DrmFdIndex,
) {
    let fd_dir = proc_root.join(pid.to_string()).join("fd");
    let fdinfo_dir = proc_root.join(pid.to_string()).join("fdinfo");
    let Ok(fds) = fs::read_dir(&fd_dir) else { return };
    let mut drm_fds: Vec<u32> = Vec::new();
    for fd in fds.flatten() {
        let fname = fd.file_name();
        let Some(name) = fname.to_str() else { continue };
        let Ok(fd_num) = name.parse::<u32>() else { continue };
        // readlink to see where this fd points. /dev/dri/card* and
        // /dev/dri/renderD* are the DRM nodes we care about; every
        // other target type means this fd is irrelevant to GPU
        // metrics so we don't bother reading its fdinfo.
        let Ok(target) = fs::read_link(fd.path()) else { continue };
        let Some(target_str) = target.to_str() else { continue };
        if !target_str.starts_with("/dev/dri/") {
            continue;
        }
        let info_path = fdinfo_dir.join(name);
        let Ok(text) = fs::read_to_string(&info_path) else { continue };
        if parse_into_any(&text, snap) {
            drm_fds.push(fd_num);
        }
    }
    if !drm_fds.is_empty() {
        index.insert(pid, drm_fds);
    }
}

/// Fast-path helper. For a pid known from a prior rescan to hold DRM
/// fds at the given fd numbers, read only those specific fdinfo files.
/// Entries that survive (the fd is still there + still xe + still on
/// the same pdev) get echoed back into `next_index` so the next
/// fast scan stays current.
fn read_known_drm_fdinfos(
    proc_root: &Path,
    pid: u32,
    fds: &[u32],
    snap: &mut HashMap<String, PdevSnapshot>,
    next_index: &mut DrmFdIndex,
) {
    let fdinfo_dir = proc_root.join(pid.to_string()).join("fdinfo");
    let mut survivors: Vec<u32> = Vec::with_capacity(fds.len());
    for &fd_num in fds {
        let info_path = fdinfo_dir.join(fd_num.to_string());
        let Ok(text) = fs::read_to_string(&info_path) else { continue };
        if parse_into_any(&text, snap) {
            survivors.push(fd_num);
        }
    }
    if !survivors.is_empty() {
        next_index.insert(pid, survivors);
    }
}

/// Single-pdev convenience kept around for tests + the few external
/// callers in the historical xe smoke-test surface. Does a full /proc
/// rescan and pulls the requested pdev out of the global map. The
/// production xe plugin uses `capture_all_filtered` directly.
#[cfg(test)]
pub fn capture(sysroot: Option<&Path>, pdev: &str) -> PdevSnapshot {
    capture_all_filtered(sysroot, None).0.remove(pdev).unwrap_or_default()
}

/// Parse one fdinfo body and append into whichever pdev's snapshot
/// matches. Returns `true` if the entry was an xe fdinfo (so the
/// caller can stamp the parent PID into the "PIDs with xe fds" set
/// for cheaper future scans).
fn parse_into_any(text: &str, out: &mut HashMap<String, PdevSnapshot>) -> bool {
    let mut is_xe = false;
    let mut pdev: Option<String> = None;
    let mut client_id: Option<u64> = None;
    let mut busy: Vec<(String, u64)> = Vec::new();
    let mut total: Vec<(String, u64)> = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim();
        match k {
            "drm-driver" => is_xe = v == "xe",
            "drm-pdev" => pdev = Some(v.to_owned()),
            "drm-client-id" => client_id = v.parse().ok(),
            other => {
                if let Some(class) = other.strip_prefix("drm-cycles-") {
                    if let Ok(n) = v.parse::<u64>() {
                        busy.push((class.to_owned(), n));
                    }
                } else if let Some(class) = other.strip_prefix("drm-total-cycles-")
                    && let Ok(n) = v.parse::<u64>()
                {
                    total.push((class.to_owned(), n));
                }
            }
        }
    }
    if !is_xe {
        return false;
    }
    let Some(pdev) = pdev else { return false };
    let Some(client_id) = client_id else { return false };
    let snap = out.entry(pdev).or_default();
    for (class, n) in busy {
        snap.busy.insert((client_id, class), n);
    }
    for (class, n) in total {
        snap.total
            .entry(class)
            .and_modify(|cur| {
                if n > *cur {
                    *cur = n;
                }
            })
            .or_insert(n);
    }
    true
}

/// Compute fractional utilization (0.0..=1.0) per engine class for one
/// pdev, given consecutive snapshots. A client only present in `cur`
/// contributes its full busy value (it's new work since the client
/// started, against the engine cycles that elapsed in the same window).
/// A client only in `prev` contributes nothing (its work was already
/// accounted for in the previous tick).
pub fn per_class_util(prev: &PdevSnapshot, cur: &PdevSnapshot) -> HashMap<String, f64> {
    let mut busy_delta: HashMap<String, u64> = HashMap::new();
    for ((cid, class), busy_now) in &cur.busy {
        let prev_busy = prev.busy.get(&(*cid, class.clone())).copied().unwrap_or(0);
        let delta = busy_now.saturating_sub(prev_busy);
        *busy_delta.entry(class.clone()).or_insert(0) += delta;
    }
    let mut out: HashMap<String, f64> = HashMap::new();
    for (class, total_now) in &cur.total {
        // No prior reading of the engine clock means we can't form a
        // delta — first sample after subscribe reports 0% for this class
        // rather than fabricating one from absolute counters.
        let Some(prev_total) = prev.total.get(class).copied() else { continue };
        let total_delta = total_now.saturating_sub(prev_total);
        if total_delta == 0 {
            continue;
        }
        let busy = busy_delta.get(class).copied().unwrap_or(0);
        out.insert(class.clone(), (busy as f64 / total_delta as f64).clamp(0.0, 1.0));
    }
    out
}

/// Headline util: max across engine classes, clamped to 0..=1. Matches
/// the convention used by Mission Center, nvtop, and intel_gpu_top — a
/// GPU doing 80% render + 0% video reads as 80%, not as an average.
pub fn max_util(prev: &PdevSnapshot, cur: &PdevSnapshot) -> f64 {
    per_class_util(prev, cur).values().copied().fold(0.0_f64, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Writes a synthetic fdinfo entry *and* the matching
    /// `/proc/<pid>/fd/<fd>` symlink that points at `/dev/dri/card0`.
    /// The post-perf-refactor full rescan requires the symlink to
    /// exist (and to point at a DRM node) before it'll read the
    /// matching fdinfo, so tests must mirror the kernel's layout.
    fn write_fdinfo(root: &Path, pid: u32, fd: u32, body: &str) {
        let fdinfo_dir = root.join(format!("proc/{pid}/fdinfo"));
        let fd_dir = root.join(format!("proc/{pid}/fd"));
        fs::create_dir_all(&fdinfo_dir).unwrap();
        fs::create_dir_all(&fd_dir).unwrap();
        fs::write(fdinfo_dir.join(fd.to_string()), body).unwrap();
        // Absolute symlink target so the readlink in discover_pid_drm_fds
        // sees "/dev/dri/..." rather than a relative path. The link
        // doesn't need to resolve — we only inspect its target string.
        std::os::unix::fs::symlink("/dev/dri/card0", fd_dir.join(fd.to_string())).unwrap();
    }

    #[test]
    fn parser_extracts_busy_and_total_for_matching_pdev() {
        let mut out = HashMap::new();
        parse_into_any(
            "drm-driver:\txe\n\
             drm-client-id:\t42\n\
             drm-pdev:\t0000:06:00.0\n\
             drm-cycles-rcs:\t1000\n\
             drm-total-cycles-rcs:\t10000\n\
             drm-cycles-bcs:\t500\n\
             drm-total-cycles-bcs:\t10000\n",
            &mut out,
        );
        let snap = out.get("0000:06:00.0").expect("pdev should be present");
        assert_eq!(snap.busy.get(&(42, "rcs".into())), Some(&1000));
        assert_eq!(snap.busy.get(&(42, "bcs".into())), Some(&500));
        assert_eq!(snap.total.get("rcs"), Some(&10000));
    }

    #[test]
    fn parser_skips_non_xe_and_routes_by_pdev() {
        let mut out = HashMap::new();
        // amdgpu — must be ignored entirely.
        parse_into_any(
            "drm-driver:\tamdgpu\ndrm-client-id:\t1\ndrm-pdev:\t0000:06:00.0\n\
             drm-cycles-rcs:\t1000\ndrm-total-cycles-rcs:\t10000\n",
            &mut out,
        );
        // xe on a different pdev — must land in its OWN bucket.
        parse_into_any(
            "drm-driver:\txe\ndrm-client-id:\t2\ndrm-pdev:\t0000:00:02.0\n\
             drm-cycles-rcs:\t1000\ndrm-total-cycles-rcs:\t10000\n",
            &mut out,
        );
        assert!(!out.contains_key("0000:06:00.0"), "amdgpu must not land in any xe pdev");
        let igpu = out.get("0000:00:02.0").expect("xe iGPU pdev should be present");
        assert_eq!(igpu.busy.get(&(2, "rcs".into())), Some(&1000));
    }

    #[test]
    fn parser_keeps_max_total_across_fds() {
        let mut out = HashMap::new();
        for (cid, total) in [(1, 100_u64), (2, 200), (3, 150)] {
            parse_into_any(
                &format!(
                    "drm-driver:\txe\ndrm-client-id:\t{cid}\ndrm-pdev:\t0000:06:00.0\n\
                     drm-cycles-rcs:\t0\ndrm-total-cycles-rcs:\t{total}\n"
                ),
                &mut out,
            );
        }
        assert_eq!(out.get("0000:06:00.0").unwrap().total.get("rcs"), Some(&200));
    }

    #[test]
    fn util_is_max_class_clamped() {
        let mut prev = PdevSnapshot::default();
        prev.busy.insert((1, "rcs".into()), 0);
        prev.busy.insert((1, "vcs".into()), 0);
        prev.total.insert("rcs".into(), 1000);
        prev.total.insert("vcs".into(), 1000);

        let mut cur = PdevSnapshot::default();
        cur.busy.insert((1, "rcs".into()), 800);
        cur.busy.insert((1, "vcs".into()), 200);
        cur.total.insert("rcs".into(), 2000);
        cur.total.insert("vcs".into(), 2000);

        let per = per_class_util(&prev, &cur);
        assert!((per["rcs"] - 0.8).abs() < 1e-6);
        assert!((per["vcs"] - 0.2).abs() < 1e-6);
        assert!((max_util(&prev, &cur) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn util_handles_disappeared_client_without_negative_spike() {
        // Old snap: two clients running, one at heavy load. New snap:
        // the heavy client closed its fd, so its cycles vanished from
        // the per-client map. Naive (sum new - sum old) would go
        // negative; we should report 0 instead.
        let mut prev = PdevSnapshot::default();
        prev.busy.insert((1, "rcs".into()), 5000);
        prev.busy.insert((2, "rcs".into()), 100);
        prev.total.insert("rcs".into(), 10_000);

        let mut cur = PdevSnapshot::default();
        cur.busy.insert((2, "rcs".into()), 100); // client 1 gone
        cur.total.insert("rcs".into(), 11_000);

        // Client 2's busy didn't move (100 → 100), so per-class delta is 0.
        let per = per_class_util(&prev, &cur);
        assert_eq!(per["rcs"], 0.0);
    }

    #[test]
    fn util_credits_new_client_full_value() {
        let mut prev = PdevSnapshot::default();
        prev.total.insert("rcs".into(), 10_000);

        let mut cur = PdevSnapshot::default();
        cur.busy.insert((42, "rcs".into()), 5000); // brand-new client
        cur.total.insert("rcs".into(), 20_000);

        // Engine clock moved 10k cycles; new client used 5k of those.
        let per = per_class_util(&prev, &cur);
        assert!((per["rcs"] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn util_first_sample_returns_zero() {
        // No prev total_cycles for this class → can't form a delta yet.
        let prev = PdevSnapshot::default();
        let mut cur = PdevSnapshot::default();
        cur.busy.insert((1, "rcs".into()), 999);
        cur.total.insert("rcs".into(), 10_000);
        assert_eq!(max_util(&prev, &cur), 0.0);
    }

    #[test]
    fn capture_walks_synthetic_proc() {
        let dir = tempfile::TempDir::new().unwrap();
        // Two xe clients on the target pdev + one decoy on a different pdev.
        write_fdinfo(
            dir.path(),
            100,
            5,
            "drm-driver:\txe\ndrm-client-id:\t1\ndrm-pdev:\t0000:06:00.0\n\
             drm-cycles-rcs:\t111\ndrm-total-cycles-rcs:\t1000\n",
        );
        write_fdinfo(
            dir.path(),
            101,
            7,
            "drm-driver:\txe\ndrm-client-id:\t2\ndrm-pdev:\t0000:06:00.0\n\
             drm-cycles-rcs:\t222\ndrm-total-cycles-rcs:\t1100\n",
        );
        write_fdinfo(
            dir.path(),
            102,
            9,
            "drm-driver:\txe\ndrm-client-id:\t3\ndrm-pdev:\t0000:00:02.0\n\
             drm-cycles-rcs:\t999\ndrm-total-cycles-rcs:\t9000\n",
        );
        let snap = capture(Some(dir.path()), "0000:06:00.0");
        assert_eq!(snap.busy.get(&(1, "rcs".into())), Some(&111));
        assert_eq!(snap.busy.get(&(2, "rcs".into())), Some(&222));
        assert_eq!(snap.busy.get(&(3, "rcs".into())), None);
        assert_eq!(snap.total.get("rcs"), Some(&1100));
    }
}
