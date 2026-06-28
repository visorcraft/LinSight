// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Atomic JSON file writes shared by every on-disk config the project
//! maintains (`preferences.json`, `hardware.json`, per-dashboard files
//! under `dashboards/`).
//!
//! The pattern is the same every time:
//!
//!   1. `mkdir -p` the parent.
//!   2. Open a per-process-unique sibling `.tmp.<pid>.<counter>` with
//!      `O_CREAT|O_EXCL` so two writers can't trample each other.
//!   3. Write the serialized body, then `fsync(2)` so the bytes hit
//!      stable storage before the rename swaps the inode.
//!   4. `rename(2)` onto the final path. On rename failure clean up
//!      the tmp sibling so it doesn't accumulate.
//!
//! Living in `linsight-core` means the daemon's `NicknameStore`, the
//! GUI's `PreferencesModel`, and the dashboards storage all share one
//! tested implementation — diverging behavior (e.g., one of them
//! missing the fsync, another missing the cleanup) caused minor bugs
//! before this lift.

use std::fs::OpenOptions;
use std::io::{Result, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide monotonic counter for tmp-file suffixes. Combined with
/// `process::id()` it guarantees per-process uniqueness across threads
/// AND across overlapping processes (two GUI instances writing the
/// same dashboards dir on shared XDG_CONFIG_HOME).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Serialize `value` as pretty JSON and write it to `path` atomically.
/// Errors propagate as `std::io::Error` (serialization errors are
/// wrapped via `Error::other`). Use `?` to surface them; the caller
/// decides whether to log / retry / abort.
pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let suffix = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let tmp = path.with_extension(format!("json.tmp.{pid}.{suffix}"));
    let body = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Sample {
        name: String,
        value: u32,
    }

    #[test]
    fn writes_pretty_json_to_target_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("out.json");
        let s = Sample { name: "x".into(), value: 7 };
        atomic_write_json(&path, &s).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"name\""));
        assert!(body.contains("\"value\": 7"));
    }

    #[test]
    fn creates_parent_dir_if_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("a/b/c/out.json");
        let s = Sample { name: "x".into(), value: 1 };
        atomic_write_json(&nested, &s).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn leaves_no_tmp_sibling_after_success() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("out.json");
        let s = Sample { name: "x".into(), value: 1 };
        atomic_write_json(&path, &s).unwrap();
        let entries: Vec<_> =
            std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        // Only "out.json"; no "out.json.tmp.*" sibling.
        assert_eq!(entries.len(), 1, "stray tmp file: {entries:?}");
    }

    #[test]
    fn cleans_up_tmp_when_rename_fails() {
        // Force rename to fail by making `path` an existing directory.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("out.json");
        std::fs::create_dir(&path).unwrap();
        let s = Sample { name: "x".into(), value: 1 };
        let err = atomic_write_json(&path, &s).expect_err("rename onto a dir must fail");
        // No leftover `.tmp.<pid>.<counter>` siblings.
        let tmp_count = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(tmp_count, 0, "leaked tmp file after rename failure: {err}");
    }
}
