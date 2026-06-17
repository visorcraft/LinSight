// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `DashboardsModel` — CRUD over per-file dashboard layouts at
//! `~/.config/linsight/dashboards/<slug>.json`. One file per
//! dashboard so renames/deletes touch only one path and slug
//! collisions are easy to detect via filesystem probes.

use std::fs::{File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;

use chrono::Utc;
use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

use super::preferences_model::config_dir_override;

/// Tile entry in a dashboard layout. Validated at the Rust boundary
/// before write so a malformed shape never reaches disk.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct DashboardTile {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct DashboardFile {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub layout: Vec<DashboardTile>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}
fn default_schema_version() -> u32 {
    1
}

fn dashboards_dir() -> Option<PathBuf> {
    config_dir_override().map(|d| d.join("dashboards"))
}

/// True iff `s` is a safe dashboard-file basename: lowercase ASCII +
/// digits + internal `-`, length 1..=40, no leading/trailing/consecutive
/// dashes. Anything outside this set could escape `dashboards/` via
/// `../`, hidden-file the dashboard with a leading dot, or otherwise
/// reach a path the user didn't intend.
pub(crate) fn is_valid_slug(s: &str) -> bool {
    if s.is_empty() || s.len() > 40 {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') || s.contains("--") {
        return false;
    }
    s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Build the on-disk path for a slug, returning `None` if the slug
/// fails validation. Every public method that touches disk MUST
/// resolve paths through this function — never via `dir.join(slug)`
/// directly — so a slug like `../../foo` cannot escape the dashboards
/// directory.
fn dashboard_path(slug: &str) -> Option<PathBuf> {
    if !is_valid_slug(slug) {
        return None;
    }
    dashboards_dir().map(|d| d.join(format!("{slug}.json")))
}

/// Derive a filesystem-safe slug from a user-entered name. Lowercases
/// ASCII alphanumerics, replaces every other character (incl. CJK,
/// emoji, punctuation) with `-`, collapses runs, and trims dashes. If
/// nothing usable survives — common for non-Latin scripts — falls back
/// to a stable hash of the name so e.g. a Japanese dashboard is still
/// representable as a routing key. The display name is preserved
/// separately on `DashboardFile.name`.
pub(crate) fn derive_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true;
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > 40 {
        out.truncate(40);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        let mut h = DefaultHasher::new();
        name.hash(&mut h);
        out = format!("dash-{:08x}", h.finish() as u32);
    }
    debug_assert!(is_valid_slug(&out), "derive_slug produced unsafe value: {out:?}");
    out
}

struct ReservedDashboardFile {
    slug: String,
    path: PathBuf,
    file: File,
}

/// Race-free slug allocation: tries `base`, then `base-2`..`base-99`,
/// each time opening the candidate file with `O_CREAT | O_EXCL`. The
/// first that succeeds wins; if every probe loses to a concurrent
/// writer or already exists, returns `AlreadyExists`.
fn allocate_unique_slug(base: &str) -> std::io::Result<ReservedDashboardFile> {
    if !is_valid_slug(base) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid slug base"));
    }
    let dir = dashboards_dir().ok_or_else(|| std::io::Error::other("no config dir resolvable"))?;
    std::fs::create_dir_all(&dir)?;
    for i in 0..99 {
        let candidate = if i == 0 { base.to_string() } else { format!("{base}-{}", i + 1) };
        if !is_valid_slug(&candidate) {
            // base-NN may exceed length cap; skip rather than emit a
            // borderline path.
            continue;
        }
        let path = dir.join(format!("{candidate}.json"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok(ReservedDashboardFile { slug: candidate, path, file }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many similarly-named dashboards",
    ))
}

pub(crate) fn read_one(path: &Path) -> Option<DashboardFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<DashboardFile>(&raw) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(),
                "skipping malformed dashboard file");
            None
        }
    }
}

/// Atomic dashboard-file write. Delegates to
/// [`linsight_core::atomic_write_json`] for the tmp+fsync+rename dance
/// (lifted into core so this module, `PreferencesModel`, and
/// `NicknameStore` share one tested implementation).
fn write_one(d: &DashboardFile) -> std::io::Result<PathBuf> {
    let path = dashboard_path(&d.slug)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid slug"))?;
    linsight_core::atomic_write_json(&path, d)?;
    Ok(path)
}

fn write_reserved_dashboard_file(
    mut reservation: ReservedDashboardFile,
    d: &DashboardFile,
) -> std::io::Result<PathBuf> {
    if d.slug != reservation.slug {
        remove_reserved_dashboard_file(reservation);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "reserved slug does not match dashboard slug",
        ));
    }
    if let Err(e) = serde_json::to_writer_pretty(&mut reservation.file, d) {
        remove_reserved_dashboard_file(reservation);
        return Err(std::io::Error::other(e));
    }
    if let Err(e) = reservation.file.sync_all() {
        remove_reserved_dashboard_file(reservation);
        return Err(e);
    }
    Ok(reservation.path)
}

fn remove_reserved_dashboard_file(reservation: ReservedDashboardFile) {
    drop(reservation.file);
    let _ = std::fs::remove_file(reservation.path);
}

fn rename_dashboard_file(old_slug: &str, new_name: &str) -> std::io::Result<String> {
    rename_dashboard_file_with_observer(old_slug, new_name, |_| {})
}

fn rename_dashboard_file_with_observer(
    old_slug: &str,
    new_name: &str,
    observe_reservation: impl FnOnce(&ReservedDashboardFile),
) -> std::io::Result<String> {
    let old_path = dashboard_path(old_slug).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid slug `{old_slug}`"))
    })?;
    let mut d = read_one(&old_path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("dashboard `{old_slug}` not found"),
        )
    })?;
    let base = derive_slug(new_name);
    if base == old_slug {
        d.name = new_name.into();
        d.updated_at = Utc::now().to_rfc3339();
        write_one(&d)?;
        return Ok(old_slug.into());
    }

    let reservation = allocate_unique_slug(&base)?;
    let new_slug = reservation.slug.clone();
    d.name = new_name.into();
    d.slug = new_slug.clone();
    d.updated_at = Utc::now().to_rfc3339();
    observe_reservation(&reservation);
    write_reserved_dashboard_file(reservation, &d)?;
    let _ = std::fs::remove_file(&old_path);
    Ok(new_slug)
}

fn duplicate_dashboard_file(slug: &str) -> std::io::Result<String> {
    duplicate_dashboard_file_with_observer(slug, |_| {})
}

fn duplicate_dashboard_file_with_observer(
    slug: &str,
    observe_reservation: impl FnOnce(&ReservedDashboardFile),
) -> std::io::Result<String> {
    let src = dashboard_path(slug).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid slug `{slug}`"))
    })?;
    let d = read_one(&src).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, format!("dashboard `{slug}` not found"))
    })?;
    let name = format!("{} (copy)", d.name);
    let base = derive_slug(&name);
    let reservation = allocate_unique_slug(&base)?;
    let new_slug = reservation.slug.clone();
    let now = Utc::now().to_rfc3339();
    let copy = DashboardFile {
        schema_version: 1,
        name,
        slug: new_slug.clone(),
        layout: d.layout,
        created_at: now.clone(),
        updated_at: now,
    };
    observe_reservation(&reservation);
    write_reserved_dashboard_file(reservation, &copy)?;
    Ok(new_slug)
}

fn list_files() -> Vec<DashboardFile> {
    let Some(dir) = dashboards_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // Only files whose stem is a valid slug. Skips stray `.tmp.*`
        // remnants from crashed writes and any other junk a user might
        // have dropped into the dir.
        let stem_ok = p.file_stem().and_then(|s| s.to_str()).map(is_valid_slug).unwrap_or(false);
        if !stem_ok {
            continue;
        }
        if let Some(d) = read_one(&p) {
            out.push(d);
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

fn migrate_legacy_dashboard() {
    let Some(config) = config_dir_override() else {
        return;
    };
    let legacy = config.join("dashboard.json");
    if !legacy.exists() {
        return;
    }
    // Skip migration only when the user already has at least one valid
    // dashboard file. A directory containing only `.tmp` leftovers or
    // unrelated junk should not strand the user's legacy layout.
    if !list_files().is_empty() {
        return;
    }
    let raw = match std::fs::read_to_string(&legacy) {
        Ok(s) => s,
        Err(_) => return,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };
    let layout_value = parsed.get("editor_layout").cloned().unwrap_or(serde_json::json!([]));
    let layout: Vec<DashboardTile> = match serde_json::from_value(layout_value) {
        Ok(v) => v,
        Err(_) => return,
    };
    if layout.is_empty() {
        return;
    }
    let now = Utc::now().to_rfc3339();
    let d = DashboardFile {
        schema_version: 1,
        name: "Default".into(),
        slug: "default".into(),
        layout,
        created_at: now.clone(),
        updated_at: now,
    };
    if write_one(&d).is_ok() {
        let _ = std::fs::rename(&legacy, legacy.with_extension("json.migrated"));
        tracing::info!("migrated legacy dashboard.json -> dashboards/default.json");
    }
}

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, summary_json)]
        // JSON array of dashboard slugs, newest-first. QML parses this
        // to enumerate user-created dashboards without probing
        // incremental names.
        #[qproperty(QString, slug_list_json)]
        // Last-error qproperty replaces the `"error: ..."`-prefixed
        // string sentinels the original draft returned for create /
        // rename / duplicate / save_layout. QML callers check for an
        // empty slug return; if empty, they read `lastError` for the
        // banner detail. This stays in lockstep with the
        // "discriminated banner feedback" rule in CLAUDE.md.
        #[qproperty(QString, last_error)]
        type DashboardsModel = super::DashboardsModelRust;

        #[qinvokable]
        fn create(self: Pin<&mut DashboardsModel>, name: &QString) -> QString;

        #[qinvokable]
        fn rename(self: Pin<&mut DashboardsModel>, slug: &QString, new_name: &QString) -> QString;

        #[qinvokable]
        fn duplicate(self: Pin<&mut DashboardsModel>, slug: &QString) -> QString;

        #[qinvokable]
        fn remove(self: Pin<&mut DashboardsModel>, slug: &QString) -> bool;

        /// Returns the on-disk path on success, empty on failure.
        /// On failure the cause is in `last_error`.
        #[qinvokable]
        fn save_layout(
            self: Pin<&mut DashboardsModel>,
            slug: &QString,
            layout_json: &QString,
        ) -> QString;

        #[qinvokable]
        fn load_layout(self: &DashboardsModel, slug: &QString) -> QString;

        #[qinvokable]
        fn name_of(self: &DashboardsModel, slug: &QString) -> QString;

        /// True iff `slug` matches the routing-safe slug grammar
        /// (`[a-z0-9-]{1,40}`, no leading/trailing/consecutive `-`).
        /// QML routing layer calls this before navigating to an
        /// `editor:<slug>` or `dashboard:<slug>` URL fragment so a
        /// path-traversal attempt never reaches a file operation.
        #[qinvokable]
        fn is_valid_slug(self: &DashboardsModel, slug: &QString) -> bool;
    }
}

pub struct DashboardsModelRust {
    summary_json: QString,
    slug_list_json: QString,
    last_error: QString,
}

impl Default for DashboardsModelRust {
    fn default() -> Self {
        migrate_legacy_dashboard();
        let arr = current_summary_json();
        let slugs = current_slug_list_json();
        Self {
            summary_json: QString::from(arr.as_str()),
            slug_list_json: QString::from(slugs.as_str()),
            last_error: QString::default(),
        }
    }
}

fn current_summary_json() -> String {
    let files = list_files();
    let arr: Vec<serde_json::Value> = files
        .iter()
        .map(|d| {
            serde_json::json!({
                "slug": d.slug,
                "name": d.name,
                "updated_at": d.updated_at,
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
}

fn current_slug_list_json() -> String {
    let files = list_files();
    let slugs: Vec<&str> = files.iter().map(|d| d.slug.as_str()).collect();
    serde_json::to_string(&slugs).unwrap_or_else(|_| "[]".into())
}

fn refresh_model(mut model: Pin<&mut ffi::DashboardsModel>) {
    let s = current_summary_json();
    let slugs = current_slug_list_json();
    model.as_mut().set_summary_json(QString::from(s.as_str()));
    model.as_mut().set_slug_list_json(QString::from(slugs.as_str()));
}

impl ffi::DashboardsModel {
    pub fn create(mut self: Pin<&mut Self>, name: &QString) -> QString {
        let n = name.to_string();
        let base = derive_slug(&n);
        let reservation = match allocate_unique_slug(&base) {
            Ok(v) => v,
            Err(e) => return self.report_err(format!("create failed: {e}")),
        };
        let slug = reservation.slug.clone();
        let now = Utc::now().to_rfc3339();
        let d = DashboardFile {
            schema_version: 1,
            name: n,
            slug: slug.clone(),
            layout: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        if let Err(e) = write_reserved_dashboard_file(reservation, &d) {
            return self.report_err(format!("create failed: {e}"));
        }
        refresh_model(self.as_mut());
        QString::from(slug.as_str())
    }

    pub fn rename(mut self: Pin<&mut Self>, slug: &QString, new_name: &QString) -> QString {
        let old_slug = slug.to_string();
        let new_name_s = new_name.to_string();
        match rename_dashboard_file(&old_slug, &new_name_s) {
            Ok(new_slug) => {
                refresh_model(self.as_mut());
                QString::from(new_slug.as_str())
            }
            Err(e) => self.report_err(format!("rename failed: {e}")),
        }
    }

    pub fn duplicate(mut self: Pin<&mut Self>, slug: &QString) -> QString {
        let s = slug.to_string();
        match duplicate_dashboard_file(&s) {
            Ok(new_slug) => {
                refresh_model(self.as_mut());
                QString::from(new_slug.as_str())
            }
            Err(e) => self.report_err(format!("duplicate failed: {e}")),
        }
    }

    pub fn remove(mut self: Pin<&mut Self>, slug: &QString) -> bool {
        let s = slug.to_string();
        let Some(path) = dashboard_path(&s) else {
            let _ = self.as_mut().report_err(format!("remove rejected: invalid slug `{s}`"));
            return false;
        };
        let removed = std::fs::remove_file(&path).is_ok();
        if removed {
            refresh_model(self.as_mut());
        }
        removed
    }

    pub fn save_layout(mut self: Pin<&mut Self>, slug: &QString, layout_json: &QString) -> QString {
        let s = slug.to_string();
        let Some(path) = dashboard_path(&s) else {
            return self.report_err(format!("save failed: invalid slug `{s}`"));
        };
        let Some(mut d) = read_one(&path) else {
            return self.report_err(format!("save failed: dashboard `{s}` not found"));
        };
        let raw = layout_json.to_string();
        let layout: Vec<DashboardTile> = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return self.report_err(format!("save failed: invalid layout JSON: {e}"));
            }
        };
        // Reject obviously-bogus tile geometry at the boundary so a
        // corrupt layout cannot reach disk.
        for (i, t) in layout.iter().enumerate() {
            if t.id.is_empty() {
                return self.report_err(format!("save failed: tile {i} has empty id"));
            }
            if t.w <= 0 || t.h <= 0 || t.x < 0 || t.y < 0 {
                return self.report_err(format!(
                    "save failed: tile {i} `{}` has invalid geometry ({}x{} @ {},{})",
                    t.id, t.w, t.h, t.x, t.y,
                ));
            }
        }
        d.layout = layout;
        d.updated_at = Utc::now().to_rfc3339();
        match write_one(&d) {
            Ok(written) => {
                refresh_model(self.as_mut());
                QString::from(written.to_string_lossy().as_ref())
            }
            Err(e) => self.report_err(format!("save failed: {e}")),
        }
    }

    pub fn load_layout(&self, slug: &QString) -> QString {
        let s = slug.to_string();
        let Some(path) = dashboard_path(&s) else {
            return QString::from("[]");
        };
        match read_one(&path) {
            Some(d) => {
                let body = serde_json::to_string(&d.layout).unwrap_or_else(|_| "[]".into());
                QString::from(body.as_str())
            }
            None => QString::from("[]"),
        }
    }

    pub fn name_of(&self, slug: &QString) -> QString {
        let s = slug.to_string();
        let Some(path) = dashboard_path(&s) else {
            return QString::default();
        };
        match read_one(&path) {
            Some(d) => QString::from(d.name.as_str()),
            None => QString::default(),
        }
    }

    pub fn is_valid_slug(&self, slug: &QString) -> bool {
        crate::qobjects::dashboards_model::is_valid_slug(&slug.to_string())
    }

    /// Sets `lastError` and returns an empty QString — the
    /// canonical failure value for slug-returning invokables. Callers
    /// in QML treat an empty return as "operation failed; consult
    /// lastError for the banner."
    fn report_err(mut self: Pin<&mut Self>, msg: impl Into<String>) -> QString {
        let m = msg.into();
        tracing::warn!(error = %m, "DashboardsModel operation failed");
        self.as_mut().set_last_error(QString::from(m.as_str()));
        QString::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qobjects::preferences_model::tests::TempXdgConfig;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    fn with_temp<F: FnOnce()>(f: F) {
        let _g = TempXdgConfig::new();
        f();
    }

    #[test]
    fn slug_derivation_normalizes() {
        assert_eq!(derive_slug("Production Server #2"), "production-server-2");
        assert_eq!(derive_slug("  Hello   World  "), "hello-world");
        assert_eq!(derive_slug("CamelCase"), "camelcase");
        assert_eq!(derive_slug("a/b/c"), "a-b-c");
        assert_eq!(derive_slug("---trim---"), "trim");
    }

    #[test]
    fn slug_falls_back_when_no_ascii_alnums() {
        // Non-Latin scripts and emoji produce a stable hash-derived
        // ASCII slug rather than the empty string — the user's
        // dashboard is still routable, the display name preserves the
        // original characters.
        let a = derive_slug("\u{1f389}\u{1f389}");
        let b = derive_slug("\u{1f389}\u{1f389}");
        assert!(a.starts_with("dash-"));
        assert!(is_valid_slug(&a));
        assert_eq!(a, b, "fallback must be deterministic per name");

        let jp = derive_slug("ダッシュボード");
        assert!(jp.starts_with("dash-"));
        assert!(is_valid_slug(&jp));
    }

    #[test]
    fn slug_validator_rejects_traversal() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("../etc"));
        assert!(!is_valid_slug("foo/bar"));
        assert!(!is_valid_slug(".hidden"));
        assert!(!is_valid_slug("-leading"));
        assert!(!is_valid_slug("trailing-"));
        assert!(!is_valid_slug("double--dash"));
        assert!(!is_valid_slug("UPPER"));
        assert!(!is_valid_slug("space here"));
        assert!(!is_valid_slug(&"a".repeat(41)));
        assert!(is_valid_slug("ok"));
        assert!(is_valid_slug("dash-12ab"));
    }

    #[test]
    fn dashboard_path_refuses_traversal() {
        with_temp(|| {
            assert!(dashboard_path("../etc").is_none());
            assert!(dashboard_path("/etc/passwd").is_none());
            assert!(dashboard_path("foo/bar").is_none());
            let p = dashboard_path("ok").unwrap();
            assert!(p.starts_with(dashboards_dir().unwrap()));
            assert_eq!(p.file_name().unwrap(), "ok.json");
        });
    }

    #[test]
    fn unique_slug_appends_suffix_on_collision() {
        with_temp(|| {
            let dir = dashboards_dir().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("x.json"), "{}").unwrap();
            let reservation_a = allocate_unique_slug("x").unwrap();
            assert_eq!(reservation_a.slug, "x-2");
            let reservation_b = allocate_unique_slug("x").unwrap();
            assert_eq!(reservation_b.slug, "x-3");
        });
    }

    fn sample_tile() -> DashboardTile {
        DashboardTile {
            id: "cpu.util".into(),
            x: 0,
            y: 0,
            w: 200,
            h: 120,
            options: serde_json::Value::Null,
        }
    }

    fn sample_dashboard(name: &str, slug: &str) -> DashboardFile {
        DashboardFile {
            schema_version: 1,
            name: name.into(),
            slug: slug.into(),
            layout: vec![sample_tile()],
            created_at: "2026-05-26T00:00:00Z".into(),
            updated_at: "2026-05-26T00:00:00Z".into(),
        }
    }

    #[test]
    fn held_reservation_forces_next_allocator_to_use_suffix() {
        with_temp(|| {
            let first = allocate_unique_slug("race").unwrap();
            assert_eq!(first.slug, "race");

            let second = allocate_unique_slug("race").unwrap();
            assert_eq!(second.slug, "race-2");
        });
    }

    #[cfg(unix)]
    #[test]
    fn reserved_dashboard_write_keeps_reserved_file_as_final_path() {
        with_temp(|| {
            let reservation = allocate_unique_slug("reserved").unwrap();
            let reserved_path = reservation.path.clone();
            let reserved_ino = reservation.file.metadata().unwrap().ino();
            let dashboard = sample_dashboard("Reserved", "reserved");

            let written = write_reserved_dashboard_file(reservation, &dashboard).unwrap();

            assert_eq!(written, reserved_path);
            assert_eq!(std::fs::metadata(&written).unwrap().ino(), reserved_ino);
            assert_eq!(read_one(&written).unwrap(), dashboard);
        });
    }

    #[cfg(unix)]
    #[test]
    fn rename_to_new_slug_keeps_reserved_file_as_final_path() {
        with_temp(|| {
            write_one(&sample_dashboard("Old", "old")).unwrap();
            write_one(&sample_dashboard("Target", "target")).unwrap();
            let mut reserved_path = PathBuf::new();
            let mut reserved_ino = 0;

            let renamed = rename_dashboard_file_with_observer("old", "Target", |reservation| {
                assert_eq!(reservation.slug, "target-2");
                reserved_path = reservation.path.clone();
                reserved_ino = reservation.file.metadata().unwrap().ino();

                let raced = allocate_unique_slug("target").unwrap();
                assert_eq!(raced.slug, "target-3");
                let raced_path = raced.path.clone();
                drop(raced);
                std::fs::remove_file(raced_path).unwrap();
            })
            .unwrap();

            assert_eq!(renamed, "target-2");
            assert!(!dashboard_path("old").unwrap().exists());
            assert_eq!(reserved_path, dashboard_path("target-2").unwrap());
            assert_eq!(std::fs::metadata(&reserved_path).unwrap().ino(), reserved_ino);
            let back = read_one(&reserved_path).unwrap();
            assert_eq!(back.slug, "target-2");
            assert_eq!(back.name, "Target");
        });
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_keeps_reserved_file_as_final_path() {
        with_temp(|| {
            write_one(&sample_dashboard("Source", "source")).unwrap();
            write_one(&sample_dashboard("Source (copy)", "source-copy")).unwrap();
            let mut reserved_path = PathBuf::new();
            let mut reserved_ino = 0;

            let duplicate = duplicate_dashboard_file_with_observer("source", |reservation| {
                assert_eq!(reservation.slug, "source-copy-2");
                reserved_path = reservation.path.clone();
                reserved_ino = reservation.file.metadata().unwrap().ino();

                let raced = allocate_unique_slug("source-copy").unwrap();
                assert_eq!(raced.slug, "source-copy-3");
                let raced_path = raced.path.clone();
                drop(raced);
                std::fs::remove_file(raced_path).unwrap();
            })
            .unwrap();

            assert_eq!(duplicate, "source-copy-2");
            assert_eq!(reserved_path, dashboard_path("source-copy-2").unwrap());
            assert_eq!(std::fs::metadata(&reserved_path).unwrap().ino(), reserved_ino);
            let back = read_one(&reserved_path).unwrap();
            assert_eq!(back.slug, "source-copy-2");
            assert_eq!(back.name, "Source (copy)");
        });
    }

    #[test]
    fn write_then_read_one_round_trips() {
        with_temp(|| {
            let d = DashboardFile {
                schema_version: 1,
                name: "Test".into(),
                slug: "test".into(),
                layout: vec![sample_tile()],
                created_at: "2026-05-26T00:00:00Z".into(),
                updated_at: "2026-05-26T00:00:00Z".into(),
            };
            let written = write_one(&d).unwrap();
            let back = read_one(&written).unwrap();
            assert_eq!(back, d);
        });
    }

    #[test]
    fn malformed_file_returns_none() {
        with_temp(|| {
            let dir = dashboards_dir().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let bad = dir.join("bad.json");
            std::fs::write(&bad, "not json").unwrap();
            assert!(read_one(&bad).is_none());
        });
    }

    #[test]
    fn list_files_sorts_by_updated_at_desc() {
        with_temp(|| {
            let d1 = DashboardFile {
                schema_version: 1,
                name: "Old".into(),
                slug: "old".into(),
                layout: Vec::new(),
                created_at: "2026-05-25T00:00:00Z".into(),
                updated_at: "2026-05-25T00:00:00Z".into(),
            };
            let d2 = DashboardFile {
                schema_version: 1,
                name: "New".into(),
                slug: "new".into(),
                layout: Vec::new(),
                created_at: "2026-05-26T00:00:00Z".into(),
                updated_at: "2026-05-26T00:00:00Z".into(),
            };
            write_one(&d1).unwrap();
            write_one(&d2).unwrap();
            let listed = list_files();
            assert_eq!(listed[0].slug, "new");
            assert_eq!(listed[1].slug, "old");
        });
    }

    #[test]
    fn list_files_ignores_unsafe_basenames() {
        // Stray `.tmp.<pid>.<n>` leftovers from a crashed write must
        // not poison the summary list; only `.json` files whose stem
        // is a valid slug are surfaced.
        with_temp(|| {
            let dir = dashboards_dir().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("ok.json"), r#"{"schema_version":1,"name":"OK","slug":"ok","layout":[],"created_at":"","updated_at":""}"#).unwrap();
            std::fs::write(dir.join("ok.json.tmp.123.0"), "{}").unwrap();
            std::fs::write(dir.join(".hidden.json"), "{}").unwrap();
            std::fs::write(dir.join("UPPER.json"), "{}").unwrap();
            let listed = list_files();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].slug, "ok");
        });
    }

    #[test]
    fn migration_from_legacy_dashboard_json() {
        with_temp(|| {
            let cfg = config_dir_override().unwrap();
            std::fs::create_dir_all(&cfg).unwrap();
            let legacy = cfg.join("dashboard.json");
            std::fs::write(
                &legacy,
                r#"{
                    "schema_version":1,
                    "pages":[],
                    "editor_layout":[{"id":"cpu.util","x":0,"y":0,"w":200,"h":120}]
                }"#,
            )
            .unwrap();
            migrate_legacy_dashboard();
            let migrated = dashboards_dir().unwrap().join("default.json");
            assert!(migrated.exists(), "expected dashboards/default.json after migration");
            assert!(legacy.with_extension("json.migrated").exists());
            assert!(!legacy.exists());
        });
    }

    #[test]
    fn migration_skipped_when_valid_dashboard_already_exists() {
        with_temp(|| {
            let cfg = config_dir_override().unwrap();
            std::fs::create_dir_all(cfg.join("dashboards")).unwrap();
            std::fs::write(
                cfg.join("dashboards/existing.json"),
                r#"{"schema_version":1,"name":"E","slug":"existing","layout":[],"created_at":"","updated_at":""}"#,
            )
            .unwrap();
            let legacy = cfg.join("dashboard.json");
            std::fs::write(&legacy, r#"{"editor_layout":[{"id":"a","x":0,"y":0,"w":1,"h":1}]}"#)
                .unwrap();
            migrate_legacy_dashboard();
            assert!(legacy.exists(), "legacy file must remain untouched");
            assert!(!cfg.join("dashboards/default.json").exists());
        });
    }

    #[test]
    fn migration_proceeds_past_stray_tmp_files() {
        // A directory with only `.tmp.*` junk (e.g. crashed-mid-write
        // remnants) must not strand the user — the migration should
        // still run because no valid dashboard exists.
        with_temp(|| {
            let cfg = config_dir_override().unwrap();
            std::fs::create_dir_all(cfg.join("dashboards")).unwrap();
            std::fs::write(cfg.join("dashboards/x.json.tmp.123.0"), "garbage").unwrap();
            let legacy = cfg.join("dashboard.json");
            std::fs::write(&legacy, r#"{"editor_layout":[{"id":"a","x":0,"y":0,"w":1,"h":1}]}"#)
                .unwrap();
            migrate_legacy_dashboard();
            assert!(cfg.join("dashboards/default.json").exists());
        });
    }
}
