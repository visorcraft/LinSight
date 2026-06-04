// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Dashboard data model + on-disk persistence.
//!
//! `DashboardSpec` is the in-memory + on-disk representation of every page
//! the GUI shows. It's a versioned JSON document at
//! `~/.config/linsight/dashboard.json`.
//!
//! Pages are either:
//! * `Preset` — the GUI computes the widget list from available sensors at
//!   render time. `Overview`, `GPUs`, `Storage`, `Network` ship today.
//! * `Custom` — explicit widget placements on a 24-column snap-to-grid.
//!
//! The Phase 6b canvas editor that mutates `Custom` pages is wired and
//! shipping; this module owns the schema, atomic write, and the migration
//! framework that future schema bumps will plug into.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::types::SensorId;

/// Schema version embedded in every saved `DashboardSpec`. Bump on any
/// incompatible change; add a matching migration in [`migrate`].
pub const DASHBOARD_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardSpec {
    pub schema_version: u32,
    pub pages: Vec<Page>,
}

impl Default for DashboardSpec {
    fn default() -> Self {
        Self {
            schema_version: DASHBOARD_SCHEMA_VERSION,
            pages: vec![Page {
                title: "Overview".into(),
                kind: PageKind::Preset(PresetKind::Overview),
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub title: String,
    pub kind: PageKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PageKind {
    Preset(PresetKind),
    Custom { widgets: Vec<WidgetPlacement> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    Overview,
    Gpus,
    Storage,
    Network,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetPlacement {
    pub kind: WidgetKind,
    /// 0-indexed column on a 24-column grid.
    pub col: u32,
    /// 0-indexed row.
    pub row: u32,
    /// Width in columns (1..=24).
    pub w: u32,
    /// Height in rows.
    pub h: u32,
    /// Sensors bound to this widget. Most widgets use exactly one sensor;
    /// MultiSparkline / Bar can take several.
    pub sensors: Vec<SensorId>,
    /// Free-form per-widget tweaks: color, threshold, label override, etc.
    /// JSON object preserved as-is so future widget kinds can extend.
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    Gauge,
    Sparkline,
    Bar,
    TextValue,
    Donut,
    Table,
    MultiSparkline,
}

/// Load a `DashboardSpec` from a file. Missing file → default spec
/// (Overview preset only). Malformed file → `Err::Serialize`. I/O error
/// (permission, etc.) → `Err::Io`. Future-version file → `Err::UnsupportedSchema`.
pub fn load(path: &Path) -> CoreResult<DashboardSpec> {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DashboardSpec::default()),
        Err(e) => return Err(CoreError::Io(format!("{}: {e}", path.display()))),
    };
    let raw: serde_json::Value = serde_json::from_str(&s)
        .map_err(|e| CoreError::Serialize(format!("{}: {e}", path.display())))?;
    let from = raw.get("schema_version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let migrated = migrate(raw, from, DASHBOARD_SCHEMA_VERSION)?;
    let spec: DashboardSpec = serde_json::from_value(migrated)
        .map_err(|e| CoreError::Serialize(format!("{}: {e}", path.display())))?;
    Ok(spec)
}

/// Persist a spec with the shared atomic JSON writer.
pub fn save(path: &Path, spec: &DashboardSpec) -> CoreResult<()> {
    crate::atomic_write_json(path, spec)
        .map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))
}

/// One-step migration: bumps `raw` from version N to N+1. Each step is a
/// pure JSON transform so it can be unit-tested in isolation.
type MigrationStep = fn(&mut serde_json::Value) -> CoreResult<()>;

/// Migration registry: `MIGRATIONS[i]` (when present) takes a v(i) document
/// and produces a v(i+1) document. Add new steps by appending here AND
/// bumping [`DASHBOARD_SCHEMA_VERSION`] in lock-step. Callers in `load()`
/// walk the slice starting from the file's `schema_version` and stop at
/// the current version.
const MIGRATIONS: &[MigrationStep] = &[
    // index 0 = v0 → v1: a v0 file is one written before `schema_version`
    // was a field. The default in `load()` treats "no field" as `from = 0`,
    // and v1 is identical to that shape, so the migration is a no-op
    // beyond stamping the version field. Keep this entry even after future
    // bumps so legacy files keep upgrading correctly.
    migrate_v0_to_v1,
];

fn migrate_v0_to_v1(_: &mut serde_json::Value) -> CoreResult<()> {
    // v0 and v1 are structurally identical (the v0 "format" was just an
    // implicit shape with no `schema_version` field). Nothing to rewrite;
    // the caller stamps the new version. Documented in MIGRATIONS.
    Ok(())
}

/// Walk migrations from `from` to `to`, applying each step in order.
/// Stamps `schema_version` to `to` at the end so the upgraded JSON
/// round-trips cleanly through `serde_json::from_value`.
pub fn migrate(mut raw: serde_json::Value, from: u32, to: u32) -> CoreResult<serde_json::Value> {
    if from > to {
        // A file from a newer daemon. The forward path doesn't exist; we
        // don't attempt to downgrade because we don't know what fields the
        // future version added.
        return Err(CoreError::UnsupportedSchema(from));
    }
    let mut cur = from;
    while cur < to {
        let step_idx = cur as usize;
        let step = MIGRATIONS.get(step_idx).ok_or_else(|| CoreError::MigrationFailed {
            from: cur,
            to: cur + 1,
            reason: format!("no migration registered for v{cur} → v{}", cur + 1),
        })?;
        step(&mut raw)?;
        cur += 1;
    }
    // Stamp the current version so the resulting JSON round-trips cleanly.
    if let serde_json::Value::Object(ref mut m) = raw {
        m.insert("schema_version".into(), serde_json::json!(to));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_overview_preset() {
        let spec = DashboardSpec::default();
        assert_eq!(spec.pages.len(), 1);
        assert!(matches!(spec.pages[0].kind, PageKind::Preset(PresetKind::Overview)));
    }

    #[test]
    fn round_trip_through_json() {
        let original = DashboardSpec {
            schema_version: DASHBOARD_SCHEMA_VERSION,
            pages: vec![
                Page { title: "Overview".into(), kind: PageKind::Preset(PresetKind::Overview) },
                Page {
                    title: "My View".into(),
                    kind: PageKind::Custom {
                        widgets: vec![WidgetPlacement {
                            kind: WidgetKind::Gauge,
                            col: 0,
                            row: 0,
                            w: 6,
                            h: 4,
                            sensors: vec![SensorId::new("cpu.util")],
                            options: serde_json::json!({ "color": "blue" }),
                        }],
                    },
                },
            ],
        };
        let s = serde_json::to_string(&original).unwrap();
        let back: DashboardSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let spec = load(&dir.path().join("absent.json")).unwrap();
        assert_eq!(spec, DashboardSpec::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("dashboard.json");
        let spec = DashboardSpec::default();
        save(&path, &spec).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn save_creates_missing_parent_dirs() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested/config/dashboard.json");
        let spec = DashboardSpec::default();
        save(&path, &spec).unwrap();
        assert_eq!(load(&path).unwrap(), spec);
    }

    #[test]
    fn save_leaves_no_tmp_sibling_after_success() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("dashboard.json");
        let spec = DashboardSpec::default();
        save(&path, &spec).unwrap();

        let entries: Vec<_> =
            std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(
            !entries.iter().any(|name| name == "dashboard.json.tmp"),
            "stray legacy tmp file: {entries:?}"
        );
        assert!(
            !entries.iter().any(|name| name.to_string_lossy().starts_with("dashboard.json.tmp.")),
            "stray atomic tmp file: {entries:?}"
        );
    }

    #[test]
    fn load_legacy_unversioned_file_upgrades_to_current() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("legacy.json");
        // A v0 (pre-`schema_version`) document: structurally identical to
        // v1 but missing the field. Build it by serializing the default
        // and stripping `schema_version` so we test the migration path
        // without hard-coding serde's chosen tag layout.
        let default = DashboardSpec::default();
        let mut value = serde_json::to_value(&default).unwrap();
        value.as_object_mut().unwrap().remove("schema_version");
        std::fs::write(&path, value.to_string()).unwrap();
        let spec = load(&path).unwrap();
        assert_eq!(spec.schema_version, DASHBOARD_SCHEMA_VERSION);
        assert_eq!(spec.pages, default.pages);
    }

    #[test]
    fn load_future_version_rejected_with_unsupported_schema() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("future.json");
        let future = serde_json::json!({
            "schema_version": 999,
            "pages": []
        });
        std::fs::write(&path, future.to_string()).unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, CoreError::UnsupportedSchema(999)));
    }

    #[test]
    fn load_malformed_json_returns_serialize_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json {{{").unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, CoreError::Serialize(_)));
    }
}
