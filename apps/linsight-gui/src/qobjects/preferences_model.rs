// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `PreferencesModel` — user-controlled UI preferences (currently:
//! theme name + active dashboard slug). Owns `~/.config/linsight/
//! preferences.json` via atomic write (write-tmp + rename).

use std::path::PathBuf;
use std::pin::Pin;

use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

use crate::qobjects::workspace_handle::with_workspace;

#[derive(Clone, Copy)]
struct Theme {
    id: &'static str,
    display_name: &'static str,
    surface0: &'static str,
    surface1: &'static str,
    surface2: &'static str,
    surface_sidebar: &'static str,
    text_primary: &'static str,
    separator_rgba: &'static str,
    accent: &'static str,
    accent_mute: &'static str,
    accent_text: &'static str,
}

/// All available themes. Curated to match Grexa's catalog (same
/// names, same color stops, same display order) so users moving
/// between Grexa and LinSight have a consistent palette set.
///
/// `system` uses empty surface strings so QML falls back to
/// `Kirigami.Theme`; named themes specify every color. The
/// `accent_text` is `#ffffff` across all non-system themes — Grexa
/// uses the same constant for legibility on every palette.
///
/// The order here is the order shown to the user (OLED Black is
/// moved up to position 4 — Grexa lists it that way in its
/// SettingsPage dropdown even though its internal enum number is
/// higher).
const THEMES: &[Theme] = &[
    // Alpha-bearing colors use the Qt-style `#AARRGGBB` form, NOT the
    // CSS-style `#RRGGBBAA`. `0x2e` = ~18% alpha matches Grexa's
    // `Qt.rgba(..., 0.18)` accent-mute formula.
    Theme {
        id: "system",
        display_name: "Follow system",
        surface0: "",
        surface1: "",
        surface2: "",
        surface_sidebar: "",
        text_primary: "",
        separator_rgba: "",
        accent: "#2d7ff9",
        accent_mute: "#2e2d7ff9",
        accent_text: "#ffffff",
    },
    Theme {
        id: "light",
        display_name: "Light",
        // Grexa derives Light's elevated surfaces via Qt.tint over
        // the host. Pre-baked here so the model returns concrete
        // values to QML without needing a tint helper.
        surface0: "#f5f5f5",
        surface1: "#ebebeb",
        surface2: "#e5e5e5",
        surface_sidebar: "#ecf0f4",
        text_primary: "#1a1a1a",
        separator_rgba: "#171a1a1a",
        accent: "#2d7ff9",
        accent_mute: "#2e2d7ff9",
        accent_text: "#ffffff",
    },
    Theme {
        id: "dark",
        display_name: "Dark",
        surface0: "#181818",
        surface1: "#252525",
        surface2: "#2c2c2c",
        surface_sidebar: "#0e0e0e",
        text_primary: "#f5f5f5",
        separator_rgba: "#1ff5f5f5",
        accent: "#2d7ff9",
        accent_mute: "#2e2d7ff9",
        accent_text: "#ffffff",
    },
    Theme {
        id: "oled-black",
        display_name: "OLED Black",
        // Pure-black canvas. Grexa picks #050505/#111111 for the
        // secondary and tertiary surfaces so card edges remain
        // visible against the canvas.
        surface0: "#000000",
        surface1: "#050505",
        surface2: "#111111",
        surface_sidebar: "#050505",
        text_primary: "#f5f5f5",
        separator_rgba: "#1ff5f5f5",
        accent: "#2d7ff9",
        accent_mute: "#2e2d7ff9",
        accent_text: "#ffffff",
    },
    Theme {
        id: "gentle-gecko",
        display_name: "Gentle Gecko",
        surface0: "#000000",
        surface1: "#003322",
        surface2: "#00593d",
        surface_sidebar: "#003322",
        text_primary: "#ffffff",
        separator_rgba: "#1fffffff",
        accent: "#00b86b",
        accent_mute: "#2e00b86b",
        accent_text: "#ffffff",
    },
    Theme {
        id: "black-knight",
        display_name: "Black Knight",
        surface0: "#000000",
        surface1: "#003366",
        surface2: "#00478f",
        surface_sidebar: "#003366",
        text_primary: "#ffffff",
        separator_rgba: "#1fffffff",
        accent: "#0078d4",
        accent_mute: "#2e0078d4",
        accent_text: "#ffffff",
    },
    Theme {
        id: "diamond",
        display_name: "Diamond",
        surface0: "#2d5b67",
        surface1: "#4f7f8c",
        surface2: "#7ca2b1",
        surface_sidebar: "#4f7f8c",
        text_primary: "#b9dae9",
        separator_rgba: "#1fb9dae9",
        accent: "#a5c5d5",
        accent_mute: "#2ea5c5d5",
        accent_text: "#ffffff",
    },
    Theme {
        id: "dreams",
        display_name: "Dreams",
        surface0: "#210b4b",
        surface1: "#3f1c6d",
        surface2: "#6a2a98",
        surface_sidebar: "#3f1c6d",
        text_primary: "#ff3d94",
        separator_rgba: "#1fff3d94",
        accent: "#b5307e",
        accent_mute: "#2eb5307e",
        accent_text: "#ffffff",
    },
    Theme {
        id: "paranoid",
        display_name: "Paranoid",
        surface0: "#1d1d4e",
        surface1: "#3f3f88",
        surface2: "#5f5fbf",
        surface_sidebar: "#3f3f88",
        text_primary: "#d2d2f4",
        separator_rgba: "#1fd2d2f4",
        accent: "#9a9ae0",
        accent_mute: "#2e9a9ae0",
        accent_text: "#ffffff",
    },
    Theme {
        id: "red-velvet",
        display_name: "Red Velvet",
        surface0: "#1a0f0f",
        surface1: "#3c1414",
        surface2: "#8b2323",
        surface_sidebar: "#3c1414",
        text_primary: "#ffdcdc",
        separator_rgba: "#1fffdcdc",
        accent: "#dc3c3c",
        accent_mute: "#2edc3c3c",
        accent_text: "#ffffff",
    },
    Theme {
        id: "subspace",
        display_name: "Subspace",
        surface0: "#2e1a47",
        surface1: "#4a2a6a",
        surface2: "#794b8b",
        surface_sidebar: "#4a2a6a",
        text_primary: "#e2c7e6",
        separator_rgba: "#1fe2c7e6",
        accent: "#b77bb4",
        accent_mute: "#2eb77bb4",
        accent_text: "#ffffff",
    },
    Theme {
        id: "tiefling",
        display_name: "Tiefling",
        surface0: "#3a0a4d",
        surface1: "#711d9a",
        surface2: "#a42db4",
        surface_sidebar: "#711d9a",
        text_primary: "#f9c54e",
        separator_rgba: "#1ff9c54e",
        accent: "#ff5c8a",
        accent_mute: "#2eff5c8a",
        accent_text: "#ffffff",
    },
    Theme {
        id: "vibes",
        display_name: "Vibes",
        surface0: "#0f0f1e",
        surface1: "#1e1e3c",
        surface2: "#cc00ff",
        surface_sidebar: "#1e1e3c",
        text_primary: "#00ffcc",
        separator_rgba: "#1f00ffcc",
        accent: "#ffcc00",
        accent_mute: "#2effcc00",
        accent_text: "#ffffff",
    },
];

fn theme_by_id(id: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PreferencesFile {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub active_dashboard: Option<String>,
    /// Page to open when LinSight launches. Either a workspace key
    /// (`overview`, `gpus`, `storage`, `network`) or
    /// `dashboard:<slug>`. Old preferences.json files without the
    /// field default to `overview` so a fresh upgrade lands somewhere
    /// sane.
    #[serde(default = "default_start_page")]
    pub start_page: String,
    /// Per-client daemon pump-thread tick interval in ms. Lower =
    /// smoother live updates, higher daemon idle CPU. Higher = bursty
    /// sample arrival, lower CPU. Clamped on the daemon side to
    /// `linsight_protocol::PUMP_INTERVAL_{MIN,MAX}_MS`.
    #[serde(default = "default_sample_interval_ms")]
    pub sample_interval_ms: u32,
    /// Show mini sparkline charts inside scalar sensor tiles.
    /// Defaults to true to preserve the behaviour from before the preference
    /// existed (sparklines were unconditionally rendered).
    #[serde(default = "default_sparklines")]
    pub sparklines: bool,
}
fn default_schema_version() -> u32 {
    1
}
fn default_theme() -> String {
    "system".into()
}
fn default_start_page() -> String {
    "overview".into()
}
fn default_sample_interval_ms() -> u32 {
    linsight_protocol::PUMP_INTERVAL_DEFAULT_MS
}
fn default_sparklines() -> bool {
    true
}
impl Default for PreferencesFile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            theme: "system".into(),
            active_dashboard: None,
            start_page: default_start_page(),
            sample_interval_ms: default_sample_interval_ms(),
            sparklines: default_sparklines(),
        }
    }
}

pub(crate) fn config_dir_override() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("linsight"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("linsight"))
}

fn prefs_path() -> Option<PathBuf> {
    config_dir_override().map(|d| d.join("preferences.json"))
}

pub(crate) fn load_prefs() -> PreferencesFile {
    let Some(path) = prefs_path() else { return PreferencesFile::default() };
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return PreferencesFile::default(),
    };
    match serde_json::from_str::<PreferencesFile>(&raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(),
                "malformed preferences.json; backing up and using defaults");
            let _ = std::fs::rename(&path, path.with_extension("json.bad"));
            PreferencesFile::default()
        }
    }
}

pub(crate) fn save_prefs(p: &PreferencesFile) -> std::io::Result<()> {
    let Some(path) = prefs_path() else {
        return Err(std::io::Error::other("no config dir resolvable from HOME / XDG_CONFIG_HOME"));
    };
    linsight_core::atomic_write_json(&path, p)
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
        #[qproperty(QString, theme)]
        #[qproperty(QString, active_dashboard)]
        // Page LinSight opens on launch. Persisted to disk and
        // validated on load (see `validate_start_page_against`).
        #[qproperty(QString, start_page)]
        // Per-client daemon pump-thread tick interval in ms. The
        // Settings page's dropdown writes here; the GUI client sends
        // a SetPumpIntervalMs request at handshake + on every change.
        #[qproperty(i32, sample_interval_ms)]
        // Whether mini sparkline charts are rendered inside scalar tiles.
        #[qproperty(bool, sparklines)]
        type PreferencesModel = super::PreferencesModelRust;

        #[qinvokable]
        fn apply_theme(self: Pin<&mut PreferencesModel>, id: &QString);

        #[qinvokable]
        fn apply_active_dashboard(self: Pin<&mut PreferencesModel>, slug: &QString);

        /// Sets the start-page preference. Accepts the four
        /// workspace keys (`overview`, `gpus`, `storage`, `network`)
        /// or `dashboard:<slug>`. Invalid values fall back to
        /// `overview` rather than persist a route the GUI can't
        /// resolve.
        #[qinvokable]
        fn apply_start_page(self: Pin<&mut PreferencesModel>, key: &QString);

        /// Persist a new pump-interval choice and push it to the
        /// daemon via `client.set_pump_interval`. Values outside the
        /// protocol's `[PUMP_INTERVAL_MIN_MS, PUMP_INTERVAL_MAX_MS]`
        /// range are clamped on the daemon side; we clamp here too
        /// to keep the GUI's display consistent with what the daemon
        /// actually applied.
        #[qinvokable]
        fn apply_sample_interval_ms(self: Pin<&mut PreferencesModel>, ms: i32);

        /// Persist the tile-sparklines toggle.
        #[qinvokable]
        fn apply_sparklines(self: Pin<&mut PreferencesModel>, enabled: bool);

        #[qinvokable]
        fn color(self: &PreferencesModel, role: &QString) -> QString;

        #[qinvokable]
        fn themes_json(self: &PreferencesModel) -> QString;

        /// Re-read `preferences.json` from disk. Useful when the
        /// user edits the file by hand or a sibling process touches
        /// it. The Settings page's Reload button invokes this.
        #[qinvokable]
        fn reload(self: Pin<&mut PreferencesModel>);
    }
}

pub struct PreferencesModelRust {
    theme: QString,
    active_dashboard: QString,
    start_page: QString,
    sample_interval_ms: i32,
    sparklines: bool,
}

impl Default for PreferencesModelRust {
    fn default() -> Self {
        let p = load_prefs();
        Self {
            theme: QString::from(p.theme.as_str()),
            active_dashboard: QString::from(p.active_dashboard.as_deref().unwrap_or("")),
            start_page: QString::from(p.start_page.as_str()),
            sample_interval_ms: p.sample_interval_ms as i32,
            sparklines: p.sparklines,
        }
    }
}

/// Workspace keys that `start_page` may name. Anything outside this
/// set (or the `dashboard:<slug>` form) is treated as invalid by
/// `apply_start_page`.
const VALID_START_WORKSPACES: &[&str] = &["overview", "gpus", "storage", "network", "hardware"];

/// True iff `key` is a syntactically valid start_page value.
/// Whether the named dashboard actually exists on disk is the
/// caller's responsibility; this only enforces the wire shape.
pub(crate) fn is_valid_start_page(key: &str) -> bool {
    if VALID_START_WORKSPACES.contains(&key) {
        return true;
    }
    if let Some(slug) = key.strip_prefix("dashboard:") {
        return crate::qobjects::dashboards_model::is_valid_slug(slug);
    }
    false
}

impl ffi::PreferencesModel {
    pub fn apply_theme(mut self: Pin<&mut Self>, id: &QString) {
        let resolved = theme_by_id(&id.to_string()).id;
        self.as_mut().set_theme(QString::from(resolved));
        let _ = save_prefs(&self.snapshot());
    }

    pub fn apply_active_dashboard(mut self: Pin<&mut Self>, slug: &QString) {
        self.as_mut().set_active_dashboard(slug.clone());
        let _ = save_prefs(&self.snapshot());
    }

    pub fn apply_start_page(mut self: Pin<&mut Self>, key: &QString) {
        let k = key.to_string();
        let resolved = if is_valid_start_page(&k) { k } else { "overview".to_string() };
        self.as_mut().set_start_page(QString::from(resolved.as_str()));
        let _ = save_prefs(&self.snapshot());
    }

    pub fn apply_sample_interval_ms(mut self: Pin<&mut Self>, ms: i32) {
        // Clamp to the daemon's accepted range so the persisted
        // value matches what the daemon will use. `i32` is the cxx-qt
        // bridge type; cast through u32 after clamping non-negative.
        let clamped = (ms.max(0) as u32).clamp(
            linsight_protocol::PUMP_INTERVAL_MIN_MS,
            linsight_protocol::PUMP_INTERVAL_MAX_MS,
        );
        self.as_mut().set_sample_interval_ms(clamped as i32);
        let _ = save_prefs(&self.snapshot());
        // Push the change to the daemon on the same thread; the RPC
        // returns the actually-applied value but for a clamped local
        // input that's identical to what we just persisted.
        let client = with_workspace(|w| w.client());
        if let Err(e) = client.set_pump_interval_ms(clamped, std::time::Duration::from_secs(5)) {
            tracing::warn!(error = %e, ms = clamped, "set_pump_interval_ms RPC failed");
        }
    }

    pub fn color(&self, role: &QString) -> QString {
        let r = role.to_string();
        let t = theme_by_id(&self.theme().to_string());
        let v = match r.as_str() {
            "surface0" => t.surface0,
            "surface1" => t.surface1,
            "surface2" => t.surface2,
            "surface_sidebar" => t.surface_sidebar,
            "text_primary" => t.text_primary,
            "separator_rgba" => t.separator_rgba,
            "accent" => t.accent,
            "accent_mute" => t.accent_mute,
            "accent_text" => t.accent_text,
            _ => "",
        };
        QString::from(v)
    }

    pub fn apply_sparklines(mut self: Pin<&mut Self>, enabled: bool) {
        self.as_mut().set_sparklines(enabled);
        let _ = save_prefs(&self.snapshot());
    }

    pub fn reload(mut self: Pin<&mut Self>) {
        let p = load_prefs();
        self.as_mut().set_theme(QString::from(p.theme.as_str()));
        self.as_mut()
            .set_active_dashboard(QString::from(p.active_dashboard.as_deref().unwrap_or("")));
        self.as_mut().set_start_page(QString::from(p.start_page.as_str()));
        self.as_mut().set_sample_interval_ms(p.sample_interval_ms as i32);
        self.as_mut().set_sparklines(p.sparklines);
    }

    pub fn themes_json(&self) -> QString {
        let entries: Vec<serde_json::Value> = THEMES.iter().map(|t| {
            serde_json::json!({
                "id": t.id,
                "display_name": t.display_name,
                "accent": t.accent,
                "surface0": if t.surface0.is_empty() { "#1f2128" } else { t.surface0 },
                "text_primary": if t.text_primary.is_empty() { "#dbdde1" } else { t.text_primary },
                "is_system": t.id == "system",
            })
        }).collect();
        QString::from(serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into()).as_str())
    }

    fn snapshot(&self) -> PreferencesFile {
        let slug = self.active_dashboard().to_string();
        PreferencesFile {
            schema_version: 1,
            theme: self.theme().to_string(),
            active_dashboard: if slug.is_empty() { None } else { Some(slug) },
            start_page: self.start_page().to_string(),
            sample_interval_ms: (*self.sample_interval_ms()).max(0) as u32,
            sparklines: *self.sparklines(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `cargo test` runs tests on a thread pool by default. The
    /// `XDG_CONFIG_HOME` swap is a process-global mutation; without
    /// serialization, parallel tests race each other to read the wrong
    /// tempdir. The Mutex is `pub(crate)` so the dashboards_model
    /// tests can share the same guard (they touch the same env var).
    pub(crate) static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard around a per-test `XDG_CONFIG_HOME` override. The
    /// previous helper restored the env var at the end of a closure;
    /// if the closure panicked, the next test inherited the dead
    /// tempdir and started failing for unrelated reasons. Using
    /// `Drop` here keeps the restore on the panic path too.
    pub(crate) struct TempXdgConfig {
        _tmp: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TempXdgConfig {
        pub(crate) fn new() -> Self {
            let guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::TempDir::new().unwrap();
            let prev = std::env::var_os("XDG_CONFIG_HOME");
            unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };
            Self { _tmp: tmp, prev, _guard: guard }
        }
    }

    impl Drop for TempXdgConfig {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                    None => std::env::remove_var("XDG_CONFIG_HOME"),
                }
            }
        }
    }

    #[test]
    fn default_when_file_missing() {
        let _g = TempXdgConfig::new();
        let p = load_prefs();
        assert_eq!(p.theme, "system");
        assert!(p.active_dashboard.is_none());
    }

    #[test]
    fn round_trip_preserves_fields() {
        let _g = TempXdgConfig::new();
        let original = PreferencesFile {
            schema_version: 1,
            theme: "dreams".into(),
            active_dashboard: Some("production".into()),
            start_page: "dashboard:production".into(),
            sample_interval_ms: 200,
            sparklines: true,
        };
        save_prefs(&original).unwrap();
        let loaded = load_prefs();
        assert_eq!(loaded, original);
    }

    #[test]
    fn sparklines_defaults_to_true() {
        let _g = TempXdgConfig::new();
        let path = prefs_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // An older preferences.json without the sparklines field — serde default must kick in.
        std::fs::write(&path, r#"{"schema_version":1,"theme":"dark","sample_interval_ms":150}"#)
            .unwrap();
        let loaded = load_prefs();
        assert!(loaded.sparklines);
    }

    #[test]
    fn start_page_validator_accepts_workspaces_and_dashboards() {
        assert!(is_valid_start_page("overview"));
        assert!(is_valid_start_page("gpus"));
        assert!(is_valid_start_page("storage"));
        assert!(is_valid_start_page("network"));
        assert!(is_valid_start_page("hardware"));
        assert!(is_valid_start_page("dashboard:default"));
        assert!(is_valid_start_page("dashboard:my-rig"));
    }

    #[test]
    fn start_page_validator_rejects_bogus_keys() {
        assert!(!is_valid_start_page(""));
        assert!(!is_valid_start_page("settings"));
        assert!(!is_valid_start_page("about"));
        assert!(!is_valid_start_page("dashboard:"));
        assert!(!is_valid_start_page("dashboard:../etc"));
        assert!(!is_valid_start_page("editor:default"));
    }

    #[test]
    fn legacy_preferences_default_start_page_to_overview() {
        let _g = TempXdgConfig::new();
        let path = prefs_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A preferences.json from before start_page existed — no
        // such field. The serde default must kick in instead of
        // rejecting the whole file.
        std::fs::write(&path, r#"{"schema_version":1,"theme":"dark"}"#).unwrap();
        let loaded = load_prefs();
        assert_eq!(loaded.start_page, "overview");
    }

    #[test]
    fn malformed_falls_back_and_renames_bad() {
        let _g = TempXdgConfig::new();
        let path = prefs_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json {{{").unwrap();
        let loaded = load_prefs();
        assert_eq!(loaded.theme, "system");
        assert!(path.with_extension("json.bad").exists());
    }

    #[test]
    fn unknown_theme_id_resolves_to_system() {
        assert_eq!(theme_by_id("nonexistent").id, "system");
        assert_eq!(theme_by_id("dreams").id, "dreams");
        assert_eq!(theme_by_id("oled-black").id, "oled-black");
    }

    #[test]
    fn theme_table_matches_grexa_order() {
        // Order matters: Grexa's SettingsPage dropdown lists themes
        // in this exact sequence (OLED Black is moved up to slot 4
        // even though its internal enum is higher). LinSight's
        // dropdown reads the slice in array order, so the ordering
        // must match Grexa's display order for parity.
        let expected: &[&str] = &[
            "system",
            "light",
            "dark",
            "oled-black",
            "gentle-gecko",
            "black-knight",
            "diamond",
            "dreams",
            "paranoid",
            "red-velvet",
            "subspace",
            "tiefling",
            "vibes",
        ];
        let ids: Vec<&str> = THEMES.iter().map(|t| t.id).collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn every_named_theme_specifies_every_color() {
        for t in THEMES.iter().filter(|t| t.id != "system") {
            assert!(!t.surface0.is_empty(), "{}: surface0 empty", t.id);
            assert!(!t.surface1.is_empty(), "{}: surface1 empty", t.id);
            assert!(!t.surface2.is_empty(), "{}: surface2 empty", t.id);
            assert!(!t.surface_sidebar.is_empty(), "{}: surface_sidebar empty", t.id);
            assert!(!t.text_primary.is_empty(), "{}: text_primary empty", t.id);
            assert!(!t.separator_rgba.is_empty(), "{}: separator_rgba empty", t.id);
            assert!(!t.accent.is_empty(), "{}: accent empty", t.id);
            assert!(!t.accent_mute.is_empty(), "{}: accent_mute empty", t.id);
            assert!(!t.accent_text.is_empty(), "{}: accent_text empty", t.id);
        }
    }

    /// Regression check for the codex review's High finding: alpha
    /// colors stored as CSS `#RRGGBBAA` render as opaque text-color
    /// slabs in QML, which parses 8-digit hex as Qt-style
    /// `#AARRGGBB`. Every alpha-bearing color string must start with
    /// `#` + 2 alpha hex digits (so length 9, first three chars are
    /// `#` + alpha pair), distinct from the opaque base color.
    #[test]
    fn alpha_colors_use_qt_aarrggbb_form() {
        fn alpha_pair(s: &str) -> &str {
            &s[1..3]
        }
        for t in THEMES {
            // accent_mute is always present, separator_rgba is empty
            // only for the system theme.
            assert_eq!(
                t.accent_mute.len(),
                9,
                "{}: accent_mute `{}` must be `#AARRGGBB`",
                t.id,
                t.accent_mute
            );
            assert_ne!(
                alpha_pair(t.accent_mute),
                "ff",
                "{}: accent_mute should be translucent, got `{}`",
                t.id,
                t.accent_mute
            );
            if !t.separator_rgba.is_empty() {
                assert_eq!(
                    t.separator_rgba.len(),
                    9,
                    "{}: separator_rgba `{}` must be `#AARRGGBB`",
                    t.id,
                    t.separator_rgba
                );
                assert_ne!(
                    alpha_pair(t.separator_rgba),
                    "ff",
                    "{}: separator_rgba should be translucent, got `{}`",
                    t.id,
                    t.separator_rgba
                );
            }
        }
    }
}
