// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `NicknameStore` — on-disk persistence for per-device user nicknames.
//!
//! Lives at `$XDG_CONFIG_HOME/linsight/hardware.json` (computed in the
//! daemon startup path, not here). Schema-versioned; atomic write via
//! tmp + rename; malformed payloads are renamed to `.json.bad` and the
//! daemon falls back to defaults rather than refusing to start.
//!
//! Mirrors the pattern used by `PreferencesModel` in the GUI crate so a
//! reviewer who has read one already knows the other. Kept dependency-free
//! beyond `serde`/`serde_json` so the daemon's startup path stays simple.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Default schema version emitted by `save()` and assumed by `load()` when
/// the on-disk file omits the field. Bump only on a breaking shape change;
/// adding new optional fields to the struct keeps schema_version at 1.
const CURRENT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

/// On-disk shape of `hardware.json`. The `device_key -> nickname` map is
/// the only consumer-facing field today; `schema_version` lets us migrate
/// the file if we ever need to.
///
/// `#[serde(default)]` on both fields means partial / legacy files load
/// cleanly: a totally missing field becomes its `Default` value rather
/// than rejecting the whole file. This is the same forgiveness model
/// `PreferencesModel` uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NicknameStore {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub nicknames: HashMap<String, String>,
}

impl Default for NicknameStore {
    fn default() -> Self {
        // We can't derive Default and still seed `schema_version = 1` —
        // derive uses `0`, which would cause the round-trip + default
        // tests to disagree on the version a fresh store reports.
        Self { schema_version: CURRENT_SCHEMA_VERSION, nicknames: HashMap::new() }
    }
}

impl NicknameStore {
    /// Read the store from `path`. On any I/O error returns the default
    /// (empty) store. On a JSON parse error renames the bad file to
    /// `<path>.bad` (so the user can recover by hand) and returns the
    /// default. Logs the parse error via `tracing::warn!`.
    pub fn load(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        match serde_json::from_str::<NicknameStore>(&raw) {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "malformed hardware.json; renaming to .bad and using defaults",
                );
                let bad = path.with_extension("json.bad");
                let _ = std::fs::rename(path, bad);
                Self::default()
            }
        }
    }

    /// Persist the store to `path` atomically via
    /// [`linsight_core::atomic_write_json`]. That helper handles the
    /// `mkdir -p`, per-process-unique `.tmp.<pid>.<counter>` sibling,
    /// fsync, rename, and tmp cleanup on rename failure.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        linsight_core::atomic_write_json(path, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("hardware.json")
    }

    #[test]
    fn default_when_file_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = NicknameStore::load(&path_in(&tmp));
        assert_eq!(store.schema_version, 1);
        assert!(store.nicknames.is_empty());
    }

    #[test]
    fn round_trip_preserves_nicknames() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = path_in(&tmp);
        let mut original = NicknameStore::default();
        original.nicknames.insert("pci:0000:06:00.0".into(), "Battlemage".into());
        original.nicknames.insert("nvme:eui.001".into(), "OS drive".into());
        original.save(&path).unwrap();

        let loaded = NicknameStore::load(&path);
        assert_eq!(loaded, original);
    }

    #[test]
    fn malformed_file_renamed_to_bad() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = path_in(&tmp);
        std::fs::write(&path, "not json {{{").unwrap();

        let loaded = NicknameStore::load(&path);
        assert_eq!(loaded, NicknameStore::default());
        assert!(path.with_extension("json.bad").exists(), ".bad sidecar should exist");
        assert!(!path.exists(), "the bad file should have been renamed away");
    }

    #[test]
    fn save_cleans_up_tmp_on_rename_failure() {
        // Force rename to fail by making `path` a directory — rename onto a
        // directory returns IsADirectory (Linux). Verifies the tmp sibling
        // is removed and the original error surfaces to the caller.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = path_in(&tmp);
        std::fs::create_dir(&path).unwrap();
        let store = NicknameStore::default();
        let err = store.save(&path).expect_err("rename onto a directory must fail");
        // We don't pin the exact ErrorKind because it varies across
        // platforms; what matters is the tmp file is gone after the failure.
        let tmp_sibling = path.with_extension("json.tmp");
        assert!(!tmp_sibling.exists(), "tmp file should be cleaned up; got: {err}");
    }

    #[test]
    fn unknown_top_level_keys_are_dropped_but_known_fields_survive() {
        // A future version may write fields we don't yet know about; a
        // legacy field we no longer model becomes a no-op. Either way,
        // the nicknames we DO understand must round-trip through a
        // load.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = path_in(&tmp);
        std::fs::write(
            &path,
            r#"{
                "schema_version": 1,
                "nicknames": { "pci:0000:06:00.0": "Battlemage" },
                "legacy_field_that_does_not_exist_anymore": [1, 2, 3]
            }"#,
        )
        .unwrap();

        let loaded = NicknameStore::load(&path);
        assert_eq!(loaded.schema_version, 1);
        assert_eq!(
            loaded.nicknames.get("pci:0000:06:00.0").map(String::as_str),
            Some("Battlemage")
        );
    }
}
