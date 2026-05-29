<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Themes + Custom Dashboards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a curated theme system (7 palettes including a Plasma-aware default) and a multi-dashboard authoring + viewing flow to the LinSight GUI.

**Architecture:** Two new cxx-qt qobjects (`PreferencesModel`, `DashboardsModel`) own JSON-on-disk state under `~/.config/linsight/`. `DesignTokens.qml` reads every color role from the preferences model with a Kirigami fallback for the `system` theme. A new `DashboardViewPage.qml` renders saved layouts read-only; the existing `CanvasEditorPage.qml` becomes slug-aware. The sidebar in `Main.qml` gains a `DASHBOARDS` section between `WORKSPACE` and `SYSTEM`.

**Tech Stack:** Rust 1.95, cxx-qt 0.8, Qt 6 / Kirigami, `serde_json`, `chrono` (NEW — for timestamps).

**Spec:** `docs/superpowers/specs/2026-05-26-themes-and-custom-dashboards-design.md`

---

## File Structure

### New files

| Path | Responsibility |
|---|---|
| `apps/linsight-gui/src/qobjects/preferences_model.rs` | `PreferencesModel` qobject + theme registry + prefs.json I/O |
| `apps/linsight-gui/src/qobjects/dashboards_model.rs` | `DashboardsModel` qobject + per-file dashboard I/O + slug logic |
| `apps/linsight-gui/qml/ThemePicker.qml` | Mini-card swatch grid (Settings page content) |
| `apps/linsight-gui/qml/DashboardViewPage.qml` | Read-only canvas renderer |
| `apps/linsight-gui/qml/DashboardNavRow.qml` | Sidebar row with kebab menu |
| `apps/linsight-gui/qml/NewDashboardDialog.qml` | Kirigami.PromptDialog for naming |
| `apps/linsight-gui/qml/DeleteDashboardDialog.qml` | Kirigami.PromptDialog for delete confirm |

### Modified files

| Path | Change |
|---|---|
| `apps/linsight-gui/src/qobjects/mod.rs` | Register new qobjects |
| `apps/linsight-gui/qml/Main.qml` | Declare `preferences` + `dashboards`; add DASHBOARDS sidebar section; extend `goTo()` for slug-routed pages; boot routing |
| `apps/linsight-gui/qml/DesignTokens.qml` | Color roles read through `preferences.color()` |
| `apps/linsight-gui/qml/SettingsPage.qml` | Replace Appearance placeholder with `ThemePicker { }` |
| `apps/linsight-gui/qml/CanvasEditorPage.qml` | Add `editingSlug` / `editingName`; save/load via DashboardsModel |
| `apps/linsight-gui/Cargo.toml` | Add `chrono` dep |
| `Justfile` | Extend `i18n-extract` target |
| `CHANGELOG.md` | New `[Unreleased]` subsection |
| `AGENTS.md` / `CLAUDE.md` | Mention themes + dashboards |

---

## Task 1 — PreferencesModel skeleton + theme registry

**Files:**
- Create: `apps/linsight-gui/src/qobjects/preferences_model.rs`
- Modify: `apps/linsight-gui/src/qobjects/mod.rs`
- Modify: `apps/linsight-gui/Cargo.toml`

### Step 1.1: add `chrono` to GUI deps + workspace

- [ ] Edit `Cargo.toml` workspace deps to add `chrono = { version = "0.4", default-features = false, features = ["serde", "clock"] }`.
- [ ] Edit `apps/linsight-gui/Cargo.toml` to add `chrono = { workspace = true }` under `[dependencies]`.

### Step 1.2: create the theme registry table

Create `apps/linsight-gui/src/qobjects/preferences_model.rs` with the `Theme` struct + 7-entry `THEMES` table. Surface colors are intentionally dark; `system` uses empty strings for surfaces (the QML side reads empty as "fall back to Kirigami").

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `PreferencesModel` — user-controlled UI preferences (currently:
//! theme name + active dashboard slug). Owns `~/.config/linsight/
//! preferences.json` via atomic write.

use std::path::PathBuf;
use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

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

/// All available themes. `system` uses empty surface strings so QML
/// falls back to Kirigami.Theme; accent is the LinSight default for it.
const THEMES: &[Theme] = &[
    Theme {
        id: "system",
        display_name: "System (Plasma)",
        surface0: "", surface1: "", surface2: "", surface_sidebar: "",
        text_primary: "", separator_rgba: "",
        accent: "#6c8cff", accent_mute: "#6c8cff29", accent_text: "#ffffff",
    },
    Theme {
        id: "tokyo-night", display_name: "Tokyo Night",
        surface0: "#1a1b26", surface1: "#24283b", surface2: "#2f3549",
        surface_sidebar: "#16161e", text_primary: "#c0caf5",
        separator_rgba: "#c0caf51a",
        accent: "#7aa2f7", accent_mute: "#7aa2f72e", accent_text: "#1a1b26",
    },
    Theme {
        id: "catppuccin-mocha", display_name: "Catppuccin Mocha",
        surface0: "#1e1e2e", surface1: "#313244", surface2: "#45475a",
        surface_sidebar: "#181825", text_primary: "#cdd6f4",
        separator_rgba: "#cdd6f41a",
        accent: "#cba6f7", accent_mute: "#cba6f72e", accent_text: "#1e1e2e",
    },
    Theme {
        id: "gruvbox-dark", display_name: "Gruvbox Dark",
        surface0: "#282828", surface1: "#3c3836", surface2: "#504945",
        surface_sidebar: "#1d2021", text_primary: "#ebdbb2",
        separator_rgba: "#ebdbb21a",
        accent: "#fabd2f", accent_mute: "#fabd2f2e", accent_text: "#282828",
    },
    Theme {
        id: "solarized-dark", display_name: "Solarized Dark",
        surface0: "#002b36", surface1: "#073642", surface2: "#586e75",
        surface_sidebar: "#001f27", text_primary: "#93a1a1",
        separator_rgba: "#93a1a11a",
        accent: "#268bd2", accent_mute: "#268bd22e", accent_text: "#002b36",
    },
    Theme {
        id: "dracula", display_name: "Dracula",
        surface0: "#282a36", surface1: "#44475a", surface2: "#6272a4",
        surface_sidebar: "#21222c", text_primary: "#f8f8f2",
        separator_rgba: "#f8f8f21a",
        accent: "#bd93f9", accent_mute: "#bd93f92e", accent_text: "#282a36",
    },
    Theme {
        id: "nord", display_name: "Nord",
        surface0: "#2e3440", surface1: "#3b4252", surface2: "#434c5e",
        surface_sidebar: "#242933", text_primary: "#eceff4",
        separator_rgba: "#eceff41a",
        accent: "#88c0d0", accent_mute: "#88c0d02e", accent_text: "#2e3440",
    },
];

fn theme_by_id(id: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PreferencesFile {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default = "default_theme")]
    theme: String,
    #[serde(default)]
    active_dashboard: Option<String>,
}

fn default_schema_version() -> u32 { 1 }
fn default_theme() -> String { "system".into() }

impl Default for PreferencesFile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            theme: "system".into(),
            active_dashboard: None,
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("linsight"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("linsight"))
}

fn prefs_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("preferences.json"))
}

fn load_prefs() -> PreferencesFile {
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

fn save_prefs(p: &PreferencesFile) -> std::io::Result<()> {
    let Some(path) = prefs_path() else {
        return Err(std::io::Error::new(std::io::ErrorKind::Other,
            "no config dir resolvable from HOME/XDG_CONFIG_HOME"));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(p)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
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
        type PreferencesModel = super::PreferencesModelRust;

        /// Set the active theme (by id). Persists immediately.
        #[qinvokable]
        fn set_theme(self: Pin<&mut PreferencesModel>, id: &QString);

        /// Set the active dashboard slug (or empty for "none"). Persists.
        #[qinvokable]
        fn set_active_dashboard(self: Pin<&mut PreferencesModel>, slug: &QString);

        /// Color for `role` in the active theme. Returns empty for
        /// surface/text/separator roles when the theme is `system` —
        /// QML reads that as "fall back to Kirigami.Theme".
        #[qinvokable]
        fn color(self: &PreferencesModel, role: &QString) -> QString;

        /// JSON array `[{id, display_name, accent, surface0}]` for the
        /// picker grid.
        #[qinvokable]
        fn themes_json(self: &PreferencesModel) -> QString;
    }
}

pub struct PreferencesModelRust {
    theme: QString,
    active_dashboard: QString,
}

impl Default for PreferencesModelRust {
    fn default() -> Self {
        let p = load_prefs();
        Self {
            theme: QString::from(p.theme.as_str()),
            active_dashboard: QString::from(p.active_dashboard.as_deref().unwrap_or("")),
        }
    }
}

impl ffi::PreferencesModel {
    pub fn set_theme(mut self: Pin<&mut Self>, id: &QString) {
        let s = id.to_string();
        // Validate; unknown ids snap to "system".
        let id_resolved = theme_by_id(&s).id;
        self.as_mut().set_theme(QString::from(id_resolved));
        let _ = save_prefs(&self.snapshot());
    }

    pub fn set_active_dashboard(mut self: Pin<&mut Self>, slug: &QString) {
        self.as_mut().set_active_dashboard(slug.clone());
        let _ = save_prefs(&self.snapshot());
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_config<F: FnOnce()>(f: F) {
        let tmp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: tests run single-threaded under cargo test by default
        // when nextest isn't used; we restore the env var below.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()); }
        f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[test]
    fn default_when_file_missing() {
        with_temp_config(|| {
            let p = load_prefs();
            assert_eq!(p.theme, "system");
            assert!(p.active_dashboard.is_none());
        });
    }

    #[test]
    fn round_trip_preserves_fields() {
        with_temp_config(|| {
            let original = PreferencesFile {
                schema_version: 1,
                theme: "tokyo-night".into(),
                active_dashboard: Some("production".into()),
            };
            save_prefs(&original).unwrap();
            let loaded = load_prefs();
            assert_eq!(loaded, original);
        });
    }

    #[test]
    fn malformed_falls_back_and_renames_bad() {
        with_temp_config(|| {
            let path = prefs_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "not json {{{").unwrap();
            let loaded = load_prefs();
            assert_eq!(loaded.theme, "system");
            assert!(path.with_extension("json.bad").exists());
        });
    }

    #[test]
    fn unknown_theme_id_resolves_to_system() {
        assert_eq!(theme_by_id("nonexistent").id, "system");
        assert_eq!(theme_by_id("tokyo-night").id, "tokyo-night");
    }

    #[test]
    fn theme_table_has_all_expected_entries() {
        let ids: Vec<&str> = THEMES.iter().map(|t| t.id).collect();
        assert!(ids.contains(&"system"));
        assert!(ids.contains(&"tokyo-night"));
        assert!(ids.contains(&"catppuccin-mocha"));
        assert!(ids.contains(&"gruvbox-dark"));
        assert!(ids.contains(&"solarized-dark"));
        assert!(ids.contains(&"dracula"));
        assert!(ids.contains(&"nord"));
        assert_eq!(THEMES.len(), 7);
    }

    #[test]
    fn every_named_theme_specifies_every_color() {
        for t in THEMES.iter().filter(|t| t.id != "system") {
            assert!(!t.surface0.is_empty(), "{}: surface0 empty", t.id);
            assert!(!t.surface1.is_empty(), "{}: surface1 empty", t.id);
            assert!(!t.text_primary.is_empty(), "{}: text_primary empty", t.id);
            assert!(!t.accent.is_empty(), "{}: accent empty", t.id);
        }
    }
}
```

### Step 1.3: register in mod.rs

Modify `apps/linsight-gui/src/qobjects/mod.rs` to add `pub mod preferences_model;`.

### Step 1.4: run tests

```bash
cargo test -p linsight --lib qobjects::preferences_model
```

Expected: 6 pass.

### Step 1.5: commit

```bash
git add apps/linsight-gui/src/qobjects/{mod.rs,preferences_model.rs} apps/linsight-gui/Cargo.toml Cargo.toml
git commit -m "feat(gui): PreferencesModel qobject + 7-theme registry"
```

---

## Task 2 — Wire PreferencesModel into Main.qml + DesignTokens

**Files:**
- Modify: `apps/linsight-gui/qml/Main.qml`
- Modify: `apps/linsight-gui/qml/DesignTokens.qml`

### Step 2.1: declare app-scope PreferencesModel in Main.qml

Add inside `Kirigami.ApplicationWindow { id: app … }`, near the `theDashModel` declaration:

```qml
property var preferences: thePreferences
PreferencesModel {
    id: thePreferences
}
```

### Step 2.2: rewire DesignTokens color roles

Replace the surface/text/separator/accent properties in `DesignTokens.qml` (everything under the "Colors derived from the host Kirigami theme" block) with the dual-source pattern. Keep the Plasma fallback for `system`:

```qml
function _colorFor(role, plasmaFallback) {
    if (!app.preferences) return plasmaFallback
    const c = app.preferences.color(role)
    return c.length > 0 ? c : plasmaFallback
}

readonly property color surface0: _colorFor("surface0", Kirigami.Theme.backgroundColor)
readonly property color surface1: _colorFor("surface1", Qt.lighter(Kirigami.Theme.backgroundColor, 1.10))
readonly property color surface2: _colorFor("surface2", Qt.lighter(Kirigami.Theme.backgroundColor, 1.22))
readonly property color surfaceSidebar: _colorFor("surface_sidebar", Qt.darker(Kirigami.Theme.backgroundColor, 1.08))
readonly property color textPrimary: _colorFor("text_primary", Kirigami.Theme.textColor)
readonly property color separator: {
    const c = app.preferences ? app.preferences.color("separator_rgba") : ""
    return c.length > 0 ? c : Qt.rgba(Kirigami.Theme.textColor.r,
                                      Kirigami.Theme.textColor.g,
                                      Kirigami.Theme.textColor.b, 0.10)
}
readonly property color accent: app.preferences ? app.preferences.color("accent") : "#6c8cff"
readonly property color accentMute: app.preferences ? app.preferences.color("accent_mute")
                                                    : Qt.rgba(0x6c/255, 0x8c/255, 0xff/255, 0.16)
readonly property color accentText: app.preferences ? app.preferences.color("accent_text") : "white"
```

Note: cxx-qt invokables called from QML do NOT re-evaluate the binding when the qobject's qproperty changes UNLESS the binding reads a qproperty directly. The fix: also bind to `app.preferences.theme` so the change-notify fires:

```qml
readonly property string _activeTheme: app.preferences ? app.preferences.theme : ""
// _activeTheme is read once in _colorFor() to force binding deps; QML
// re-evaluates every color when the theme property changes.
function _colorFor(role, plasmaFallback) {
    if (!app.preferences) return plasmaFallback
    const _ = _activeTheme  // pull dep
    const c = app.preferences.color(role)
    return c.length > 0 ? c : plasmaFallback
}
```

### Step 2.3: smoke-build the GUI

```bash
cargo build -p linsight
```

Expected: clean compile.

### Step 2.4: commit

```bash
git add apps/linsight-gui/qml/Main.qml apps/linsight-gui/qml/DesignTokens.qml
git commit -m "feat(gui): wire DesignTokens through PreferencesModel"
```

---

## Task 3 — ThemePicker QML + Settings page

**Files:**
- Create: `apps/linsight-gui/qml/ThemePicker.qml`
- Modify: `apps/linsight-gui/qml/SettingsPage.qml`

### Step 3.1: ThemePicker.qml

```qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Item {
    id: picker
    implicitHeight: flow.implicitHeight + tokens.spaceL * 2

    readonly property var tokens: app.tokens
    readonly property var preferences: app.preferences

    readonly property var themes: {
        if (!preferences) return []
        try {
            return JSON.parse(preferences.themesJson())
        } catch (e) {
            return []
        }
    }

    Flow {
        id: flow
        anchors.fill: parent
        anchors.margins: tokens.spaceL
        spacing: tokens.spaceM

        Repeater {
            model: picker.themes
            delegate: Rectangle {
                width: 160
                height: 92
                radius: tokens.radiusCard
                color: modelData.surface0
                border.color: preferences && preferences.theme === modelData.id
                              ? tokens.accent
                              : tokens.separator
                border.width: preferences && preferences.theme === modelData.id ? 2 : 1
                Accessible.role: Accessible.RadioButton
                Accessible.name: modelData.display_name + " theme"
                Accessible.checked: preferences && preferences.theme === modelData.id
                activeFocusOnTab: true
                Keys.onReturnPressed: themeMouse.clicked(null)
                Keys.onEnterPressed:  themeMouse.clicked(null)
                Keys.onSpacePressed:  themeMouse.clicked(null)

                // Mini mock: sidebar strip + value card + accent dot
                Rectangle {
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: 12
                    color: Qt.darker(modelData.surface0, 1.15)
                    radius: tokens.radiusCard
                }
                Rectangle {
                    x: 22
                    y: 14
                    width: 90
                    height: 36
                    radius: 4
                    color: Qt.lighter(modelData.surface0, 1.15)
                    Rectangle {
                        anchors.right: parent.right
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        radius: 7
                        color: modelData.accent
                    }
                    Rectangle {
                        anchors.left: parent.left
                        anchors.leftMargin: 8
                        anchors.top: parent.top
                        anchors.topMargin: 8
                        width: 36
                        height: 4
                        radius: 2
                        color: modelData.text_primary
                        opacity: 0.6
                    }
                    Rectangle {
                        anchors.left: parent.left
                        anchors.leftMargin: 8
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: 8
                        width: 24
                        height: 4
                        radius: 2
                        color: modelData.text_primary
                        opacity: 0.4
                    }
                }
                Controls.Label {
                    anchors.left: parent.left
                    anchors.leftMargin: 22
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 8
                    text: modelData.display_name
                    color: modelData.text_primary
                    font.pixelSize: tokens.textCaption
                    font.family: tokens.sansFamily
                }
                MouseArea {
                    id: themeMouse
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: preferences.setTheme(modelData.id)
                }
            }
        }
    }
}
```

### Step 3.2: swap Settings page Appearance card

In `SettingsPage.qml`, replace the existing Appearance `SettingsCard` block with:

```qml
SettingsCard {
    title: qsTr("Appearance")
    subtitle: qsTr("Pick a theme. \"System (Plasma)\" follows your KDE color scheme; named themes pin a full palette.")
    content: ThemePicker { }
}
```

### Step 3.3: build + manual smoke

```bash
cargo build -p linsight
```

Then launch + screenshot:

```bash
target/debug/linsight settings &
sleep 3
# verify themes render in the Appearance card; clicking should switch
```

### Step 3.4: commit

```bash
git add apps/linsight-gui/qml/ThemePicker.qml apps/linsight-gui/qml/SettingsPage.qml
git commit -m "feat(gui): ThemePicker chip grid in Settings"
```

---

## Task 4 — DashboardsModel qobject + slug logic + per-file I/O

**Files:**
- Create: `apps/linsight-gui/src/qobjects/dashboards_model.rs`
- Modify: `apps/linsight-gui/src/qobjects/mod.rs`

### Step 4.1: dashboards_model.rs

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `DashboardsModel` — CRUD over per-file dashboard layouts at
//! `~/.config/linsight/dashboards/<slug>.json`.

use std::path::PathBuf;
use std::pin::Pin;

use chrono::Utc;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct DashboardFile {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub layout: serde_json::Value,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}
fn default_schema_version() -> u32 { 1 }

pub(crate) fn config_dir_override() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("linsight"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("linsight"))
}

fn dashboards_dir() -> Option<PathBuf> {
    config_dir_override().map(|d| d.join("dashboards"))
}

/// Derive a filesystem-safe slug. Returns empty if no usable chars.
pub(crate) fn derive_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true; // strip leading dashes
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
    while out.ends_with('-') { out.pop(); }
    if out.len() > 40 { out.truncate(40); while out.ends_with('-') { out.pop(); } }
    out
}

fn unique_slug(base: &str) -> std::io::Result<String> {
    let dir = dashboards_dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other,
        "no config dir resolvable"))?;
    if !dir.join(format!("{base}.json")).exists() {
        return Ok(base.to_string());
    }
    for i in 2..=99 {
        let candidate = format!("{base}-{i}");
        if !dir.join(format!("{candidate}.json")).exists() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists,
        "too many similarly-named dashboards"))
}

pub(crate) fn read_one(path: &std::path::Path) -> Option<DashboardFile> {
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

fn write_one(d: &DashboardFile) -> std::io::Result<PathBuf> {
    let dir = dashboards_dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other,
        "no config dir"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", d.slug));
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(d)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn list_files() -> Vec<DashboardFile> {
    let Some(dir) = dashboards_dir() else { return vec![] };
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
        if let Some(d) = read_one(&p) { out.push(d); }
    }
    // Sort by updated_at descending.
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
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
        type DashboardsModel = super::DashboardsModelRust;

        #[qinvokable]
        fn refresh(self: Pin<&mut DashboardsModel>);

        #[qinvokable]
        fn create(self: Pin<&mut DashboardsModel>, name: &QString) -> QString;

        #[qinvokable]
        fn rename(self: Pin<&mut DashboardsModel>, slug: &QString, new_name: &QString) -> QString;

        #[qinvokable]
        fn duplicate(self: Pin<&mut DashboardsModel>, slug: &QString) -> QString;

        #[qinvokable]
        fn remove(self: Pin<&mut DashboardsModel>, slug: &QString) -> bool;

        #[qinvokable]
        fn save_layout(self: Pin<&mut DashboardsModel>, slug: &QString, layout_json: &QString) -> QString;

        #[qinvokable]
        fn load_layout(self: &DashboardsModel, slug: &QString) -> QString;

        #[qinvokable]
        fn name_of(self: &DashboardsModel, slug: &QString) -> QString;
    }
}

pub struct DashboardsModelRust {
    summary_json: QString,
}

impl Default for DashboardsModelRust {
    fn default() -> Self {
        let mut s = Self { summary_json: QString::from("[]") };
        s.rebuild_summary();
        s
    }
}

impl DashboardsModelRust {
    fn rebuild_summary(&mut self) {
        let files = list_files();
        let arr: Vec<serde_json::Value> = files.iter().map(|d| serde_json::json!({
            "slug": d.slug,
            "name": d.name,
            "updated_at": d.updated_at,
        })).collect();
        self.summary_json = QString::from(serde_json::to_string(&arr)
            .unwrap_or_else(|_| "[]".into()).as_str());
    }
}

impl ffi::DashboardsModel {
    pub fn refresh(mut self: Pin<&mut Self>) {
        let mut new_json = QString::from("[]");
        let files = list_files();
        let arr: Vec<serde_json::Value> = files.iter().map(|d| serde_json::json!({
            "slug": d.slug, "name": d.name, "updated_at": d.updated_at,
        })).collect();
        if let Ok(s) = serde_json::to_string(&arr) {
            new_json = QString::from(s.as_str());
        }
        self.as_mut().set_summary_json(new_json);
    }

    pub fn create(mut self: Pin<&mut Self>, name: &QString) -> QString {
        let n = name.to_string();
        let base = derive_slug(&n);
        if base.is_empty() {
            return QString::from("error: name must contain at least one letter or digit");
        }
        let slug = match unique_slug(&base) {
            Ok(s) => s,
            Err(e) => return QString::from(format!("error: {e}").as_str()),
        };
        let now = Utc::now().to_rfc3339();
        let d = DashboardFile {
            schema_version: 1,
            name: n,
            slug: slug.clone(),
            layout: serde_json::json!([]),
            created_at: now.clone(),
            updated_at: now,
        };
        if let Err(e) = write_one(&d) {
            return QString::from(format!("error: {e}").as_str());
        }
        self.as_mut().refresh();
        QString::from(slug.as_str())
    }

    pub fn rename(mut self: Pin<&mut Self>, slug: &QString, new_name: &QString) -> QString {
        let old_slug = slug.to_string();
        let new_name_s = new_name.to_string();
        let base = derive_slug(&new_name_s);
        if base.is_empty() {
            return QString::from("error: name must contain at least one letter or digit");
        }
        // Load current
        let Some(dir) = dashboards_dir() else {
            return QString::from("error: no config dir");
        };
        let old_path = dir.join(format!("{old_slug}.json"));
        let Some(mut d) = read_one(&old_path) else {
            return QString::from("error: dashboard not found");
        };
        let new_slug = if base == old_slug {
            old_slug.clone()
        } else {
            match unique_slug(&base) {
                Ok(s) => s,
                Err(e) => return QString::from(format!("error: {e}").as_str()),
            }
        };
        d.name = new_name_s;
        d.slug = new_slug.clone();
        d.updated_at = Utc::now().to_rfc3339();
        if let Err(e) = write_one(&d) {
            return QString::from(format!("error: {e}").as_str());
        }
        if new_slug != old_slug {
            let _ = std::fs::remove_file(&old_path);
        }
        self.as_mut().refresh();
        QString::from(new_slug.as_str())
    }

    pub fn duplicate(mut self: Pin<&mut Self>, slug: &QString) -> QString {
        let s = slug.to_string();
        let Some(dir) = dashboards_dir() else {
            return QString::from("error: no config dir");
        };
        let Some(d) = read_one(&dir.join(format!("{s}.json"))) else {
            return QString::from("error: dashboard not found");
        };
        let new_name = format!("{} (copy)", d.name);
        // Reuse the create path so slug logic + timestamp are consistent,
        // then re-save with the original layout.
        let new_slug = self.as_mut().create(&QString::from(new_name.as_str())).to_string();
        if new_slug.starts_with("error:") {
            return QString::from(new_slug.as_str());
        }
        // Reload the freshly-created file, attach the layout, save.
        let new_path = dir.join(format!("{new_slug}.json"));
        let Some(mut new_d) = read_one(&new_path) else {
            return QString::from("error: duplicate created but cannot reload");
        };
        new_d.layout = d.layout;
        new_d.updated_at = Utc::now().to_rfc3339();
        if let Err(e) = write_one(&new_d) {
            return QString::from(format!("error: {e}").as_str());
        }
        self.as_mut().refresh();
        QString::from(new_slug.as_str())
    }

    pub fn remove(mut self: Pin<&mut Self>, slug: &QString) -> bool {
        let s = slug.to_string();
        let Some(dir) = dashboards_dir() else { return false; };
        let path = dir.join(format!("{s}.json"));
        let removed = std::fs::remove_file(&path).is_ok();
        if removed { self.as_mut().refresh(); }
        removed
    }

    pub fn save_layout(mut self: Pin<&mut Self>, slug: &QString, layout_json: &QString) -> QString {
        let s = slug.to_string();
        let Some(dir) = dashboards_dir() else {
            return QString::from("error: no config dir");
        };
        let path = dir.join(format!("{s}.json"));
        let Some(mut d) = read_one(&path) else {
            return QString::from(format!("error: dashboard `{s}` not found").as_str());
        };
        let raw = layout_json.to_string();
        let parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return QString::from(format!("error: invalid layout JSON: {e}").as_str()),
        };
        d.layout = parsed;
        d.updated_at = Utc::now().to_rfc3339();
        match write_one(&d) {
            Ok(written) => {
                self.as_mut().refresh();
                QString::from(written.to_string_lossy().as_ref())
            }
            Err(e) => QString::from(format!("error: {e}").as_str()),
        }
    }

    pub fn load_layout(&self, slug: &QString) -> QString {
        let s = slug.to_string();
        let Some(dir) = dashboards_dir() else { return QString::from("[]"); };
        match read_one(&dir.join(format!("{s}.json"))) {
            Some(d) => QString::from(d.layout.to_string().as_str()),
            None => QString::from("[]"),
        }
    }

    pub fn name_of(&self, slug: &QString) -> QString {
        let s = slug.to_string();
        let Some(dir) = dashboards_dir() else { return QString::default(); };
        match read_one(&dir.join(format!("{s}.json"))) {
            Some(d) => QString::from(d.name.as_str()),
            None => QString::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp<F: FnOnce()>(f: F) {
        let tmp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()); }
        f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
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
    fn slug_rejects_empty_after_normalization() {
        assert_eq!(derive_slug(""), "");
        assert_eq!(derive_slug("###"), "");
        assert_eq!(derive_slug("🎉🎉🎉"), "");
    }

    #[test]
    fn unique_slug_appends_suffix_on_collision() {
        with_temp(|| {
            let dir = dashboards_dir().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("x.json"), "{}").unwrap();
            assert_eq!(unique_slug("x").unwrap(), "x-2");
            std::fs::write(dir.join("x-2.json"), "{}").unwrap();
            assert_eq!(unique_slug("x").unwrap(), "x-3");
        });
    }

    #[test]
    fn write_then_read_one_round_trips() {
        with_temp(|| {
            let d = DashboardFile {
                schema_version: 1,
                name: "Test".into(),
                slug: "test".into(),
                layout: serde_json::json!([{"id":"cpu.util","x":0,"y":0,"w":200,"h":120}]),
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
                schema_version: 1, name: "Old".into(), slug: "old".into(),
                layout: serde_json::json!([]),
                created_at: "2026-05-25T00:00:00Z".into(),
                updated_at: "2026-05-25T00:00:00Z".into(),
            };
            let d2 = DashboardFile {
                schema_version: 1, name: "New".into(), slug: "new".into(),
                layout: serde_json::json!([]),
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
}
```

### Step 4.2: register in mod.rs

Add `pub mod dashboards_model;` next to `preferences_model`.

### Step 4.3: run tests

```bash
cargo test -p linsight --lib qobjects::dashboards_model
```

Expected: 6 pass.

### Step 4.4: commit

```bash
git add apps/linsight-gui/src/qobjects/{mod.rs,dashboards_model.rs}
git commit -m "feat(gui): DashboardsModel qobject with slug-based per-file storage"
```

---

## Task 5 — Sidebar wiring + dialogs + routing

**Files:**
- Create: `apps/linsight-gui/qml/NewDashboardDialog.qml`
- Create: `apps/linsight-gui/qml/DeleteDashboardDialog.qml`
- Create: `apps/linsight-gui/qml/DashboardNavRow.qml`
- Modify: `apps/linsight-gui/qml/Main.qml`

### Step 5.1: NewDashboardDialog.qml

```qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.PromptDialog {
    id: dlg
    title: qsTr("New dashboard")
    standardButtons: Kirigami.Dialog.NoButton
    customFooterActions: [
        Kirigami.Action { text: qsTr("Cancel"); onTriggered: dlg.close() },
        Kirigami.Action {
            text: qsTr("Create")
            enabled: dlg.isValid
            onTriggered: dlg.accept()
        }
    ]

    property string slug: ""           // emitted slug on accept
    readonly property bool isValid: nameField.text.trim().length > 0
                                     && nameField.text.length <= 40

    signal accepted(string slug)

    function open() {
        nameField.text = ""
        nameField.forceActiveFocus()
        visible = true
    }

    function accept() {
        const result = app.dashboards.create(nameField.text.trim()).toString()
        if (result.indexOf("error:") === 0) {
            errorLabel.text = result
            return
        }
        dlg.slug = result
        dlg.close()
        dlg.accepted(result)
    }

    ColumnLayout {
        spacing: app.tokens.spaceS
        width: 360
        Controls.Label {
            text: qsTr("Name your dashboard. The Editor opens with an empty canvas.")
            opacity: 0.8
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
        Controls.TextField {
            id: nameField
            Layout.fillWidth: true
            placeholderText: qsTr("e.g. Production")
            onAccepted: if (dlg.isValid) dlg.accept()
        }
        Controls.Label {
            id: errorLabel
            color: Kirigami.Theme.negativeTextColor
            visible: text.length > 0
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
    }
}
```

### Step 5.2: DeleteDashboardDialog.qml

```qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Kirigami.PromptDialog {
    id: dlg
    property string slug: ""
    property string displayName: ""
    signal confirmed(string slug)

    title: qsTr("Delete %1?").arg(displayName)
    subtitle: qsTr("This dashboard's layout will be permanently removed. " +
                   "The underlying sensors continue to run.")
    standardButtons: Kirigami.Dialog.NoButton
    customFooterActions: [
        Kirigami.Action { text: qsTr("Cancel"); onTriggered: dlg.close() },
        Kirigami.Action {
            text: qsTr("Delete")
            icon.name: "edit-delete-symbolic"
            onTriggered: {
                if (app.dashboards.remove(dlg.slug)) {
                    dlg.confirmed(dlg.slug)
                }
                dlg.close()
            }
        }
    ]

    function open(s, name) {
        dlg.slug = s
        dlg.displayName = name
        visible = true
    }
}
```

### Step 5.3: DashboardNavRow.qml

```qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Item {
    id: row
    height: app.tokens.navRowHeight
    property string slug: ""
    property string label: ""
    property bool active: false
    property bool compact: false
    signal triggered()
    signal renameRequested(string slug)
    signal deleteRequested(string slug, string name)
    signal duplicateRequested(string slug)

    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: app.tokens.spaceS
        anchors.rightMargin: app.tokens.spaceS
        anchors.topMargin: 1
        anchors.bottomMargin: 1
        radius: app.tokens.radiusInput
        color: row.active ? app.tokens.accentMute
             : mouseArea.containsPress ? app.tokens.surface2
             : mouseArea.containsMouse ? app.tokens.surface1
             : "transparent"
        border.color: row.active
            ? Qt.rgba(app.tokens.accent.r, app.tokens.accent.g, app.tokens.accent.b, 0.25)
            : "transparent"
        border.width: 1
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: app.tokens.spaceL
        anchors.rightMargin: app.tokens.spaceS
        spacing: app.tokens.spaceM

        Kirigami.Icon {
            source: "view-grid-tracking-symbolic"
            implicitWidth: 18
            implicitHeight: 18
            color: row.active ? app.tokens.accent : app.tokens.textPrimary
            opacity: row.active ? 1.0 : 0.75
            isMask: true
        }
        Controls.Label {
            text: row.label
            font.pixelSize: app.tokens.textBody
            font.family: app.tokens.sansFamily
            color: row.active ? app.tokens.accent : app.tokens.textPrimary
            opacity: row.active ? 1.0 : 0.88
            elide: Text.ElideRight
            Layout.fillWidth: true
            visible: !row.compact
        }
        Controls.ToolButton {
            id: kebab
            icon.name: "overflow-menu-symbolic"
            visible: !row.compact && (mouseArea.containsMouse || menu.visible)
            implicitWidth: 24
            implicitHeight: 24
            onClicked: menu.popup()
            Controls.Menu {
                id: menu
                Controls.MenuItem { text: qsTr("Rename"); onTriggered: row.renameRequested(row.slug) }
                Controls.MenuItem { text: qsTr("Duplicate"); onTriggered: row.duplicateRequested(row.slug) }
                Controls.MenuItem { text: qsTr("Delete"); onTriggered: row.deleteRequested(row.slug, row.label) }
            }
        }
    }
    MouseArea {
        id: mouseArea
        anchors.fill: parent
        anchors.rightMargin: 28
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: row.triggered()
    }
}
```

### Step 5.4: extend Main.qml

Three changes:
1. Declare app-scope `dashboards` qobject + dialogs.
2. Add `DASHBOARDS` section with Repeater between WORKSPACE and SYSTEM.
3. Extend `goTo()` to parse `editor:<slug>` and `dashboard:<slug>`.

Inside the `Kirigami.ApplicationWindow { id: app … }`:

```qml
property var dashboards: theDashboards
DashboardsModel { id: theDashboards }

NewDashboardDialog {
    id: newDashboardDialog
    parent: Controls.Overlay.overlay
    onAccepted: function(slug) { app.goTo("editor:" + slug) }
}
DeleteDashboardDialog {
    id: deleteDashboardDialog
    parent: Controls.Overlay.overlay
    onConfirmed: function(slug) {
        // If we deleted the currently-active dashboard, fall back.
        if (app.currentPageKey === "dashboard:" + slug || app.currentPageKey === "editor:" + slug) {
            app.goTo("overview")
        }
    }
}
```

Extend `goTo(key)`:

```qml
function goTo(key) {
    if (key === currentPageKey) return
    currentPageKey = key
    if (key.indexOf("dashboard:") === 0) {
        const slug = key.substring("dashboard:".length)
        app.pageStack.replace(dashboardViewPage, { "slug": slug, "displayName": app.dashboards.nameOf(slug) })
        app.preferences.setActiveDashboard(slug)
        return
    }
    if (key.indexOf("editor:") === 0) {
        const slug = key.substring("editor:".length)
        app.pageStack.replace(editorPage, { "editingSlug": slug, "editingName": app.dashboards.nameOf(slug) })
        app.preferences.setActiveDashboard(slug)
        return
    }
    switch (key) {
        case "overview": app.pageStack.replace(overviewPage); break
        case "gpus":     app.pageStack.replace(gpusPage); break
        case "storage":  app.pageStack.replace(storagePage); break
        case "network":  app.pageStack.replace(networkPage); break
        case "editor":   app.pageStack.replace(editorPage, { "editingSlug": "", "editingName": "" }); break
        case "settings": app.pageStack.replace(settingsPage); break
        case "about":    app.pageStack.replace(aboutPage); break
        case "licenses": app.pageStack.replace(licensesPage); break
        case "credits":  app.pageStack.replace(creditsPage); break
    }
}
```

Add the DASHBOARDS section in `globalDrawer.contentItem` between the Editor item and the SYSTEM section:

```qml
// DASHBOARDS section
Controls.Label {
    Layout.fillWidth: true
    Layout.leftMargin: tokens.spaceL
    Layout.rightMargin: tokens.spaceL
    Layout.topMargin: tokens.spaceL
    Layout.bottomMargin: tokens.spaceS
    text: qsTr("DASHBOARDS")
    font.pixelSize: 10
    font.weight: tokens.weightSemibold
    opacity: 0.5
    visible: !drawer.isCollapsed
    color: tokens.textPrimary
}
Repeater {
    model: {
        if (!app.dashboards) return []
        try { return JSON.parse(app.dashboards.summaryJson) } catch (e) { return [] }
    }
    delegate: DashboardNavRow {
        Layout.fillWidth: true
        slug: modelData.slug
        label: modelData.name
        active: app.currentPageKey === "dashboard:" + modelData.slug
             || app.currentPageKey === "editor:" + modelData.slug
        compact: drawer.isCollapsed
        onTriggered: app.goTo("dashboard:" + modelData.slug)
        onRenameRequested: function(s) {
            // Use a single shared rename via NewDashboardDialog reused
            // — simplest path is reusing the prompt with a different
            // submit handler. Keep it inline here.
            renamePrompt.slug = s
            renamePrompt.currentName = app.dashboards.nameOf(s)
            renamePrompt.open()
        }
        onDuplicateRequested: function(s) {
            const result = app.dashboards.duplicate(s).toString()
            if (result.indexOf("error:") !== 0) app.goTo("editor:" + result)
        }
        onDeleteRequested: function(s, name) {
            deleteDashboardDialog.open(s, name)
        }
    }
}
NavItem {
    Layout.fillWidth: true
    label: qsTr("+ New dashboard")
    iconName: "list-add-symbolic"
    active: false
    compact: drawer.isCollapsed
    onTriggered: newDashboardDialog.open()
}
NavItem {
    Layout.fillWidth: true
    label: qsTr("✎ Edit Current")
    iconName: "document-edit-symbolic"
    active: false
    compact: drawer.isCollapsed
    visible: app.currentPageKey.indexOf("dashboard:") === 0
    onTriggered: {
        const slug = app.currentPageKey.substring("dashboard:".length)
        app.goTo("editor:" + slug)
    }
}
```

Add the rename prompt (a tiny inline reusable dialog):

```qml
Kirigami.PromptDialog {
    id: renamePrompt
    parent: Controls.Overlay.overlay
    property string slug: ""
    property string currentName: ""
    title: qsTr("Rename dashboard")
    standardButtons: Kirigami.Dialog.NoButton
    customFooterActions: [
        Kirigami.Action { text: qsTr("Cancel"); onTriggered: renamePrompt.close() },
        Kirigami.Action {
            text: qsTr("Rename")
            enabled: renameField.text.trim().length > 0
            onTriggered: {
                const r = app.dashboards.rename(renamePrompt.slug, renameField.text.trim()).toString()
                if (r.indexOf("error:") === 0) {
                    renameErr.text = r
                    return
                }
                if (app.currentPageKey === "dashboard:" + renamePrompt.slug) {
                    app.currentPageKey = "dashboard:" + r
                } else if (app.currentPageKey === "editor:" + renamePrompt.slug) {
                    app.currentPageKey = "editor:" + r
                }
                renamePrompt.close()
            }
        }
    ]
    function open() {
        renameField.text = renamePrompt.currentName
        renameErr.text = ""
        visible = true
        renameField.forceActiveFocus()
        renameField.selectAll()
    }
    ColumnLayout {
        width: 360
        Controls.TextField { id: renameField; Layout.fillWidth: true }
        Controls.Label {
            id: renameErr
            color: Kirigami.Theme.negativeTextColor
            visible: text.length > 0
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
    }
}
```

### Step 5.5: build + manual smoke

```bash
cargo build -p linsight
target/debug/linsight &
# verify: sidebar shows DASHBOARDS section, "+ New dashboard" opens dialog,
# creating a dashboard adds a row + routes to editor
```

### Step 5.6: commit

```bash
git add apps/linsight-gui/qml/{NewDashboardDialog,DeleteDashboardDialog,DashboardNavRow,Main}.qml
git commit -m "feat(gui): DASHBOARDS sidebar section + CRUD dialogs"
```

---

## Task 6 — CanvasEditorPage slug-aware

**Files:**
- Modify: `apps/linsight-gui/qml/CanvasEditorPage.qml`

### Step 6.1: add editingSlug / editingName + replace header

Add properties at the top:

```qml
property string editingSlug: ""
property string editingName: ""
```

Replace the existing header text:

```qml
Controls.Label {
    text: page.editingSlug.length > 0
          ? qsTr("Editing: %1").arg(page.editingName)
          : qsTr("Editor")
    font.pixelSize: app.tokens.textHeading
    font.weight: app.tokens.weightBold
    font.family: app.tokens.sansFamily
    color: app.tokens.textPrimary
}
```

### Step 6.2: load dashboard layout on Component.onCompleted

Extend `Component.onCompleted`:

```qml
Component.onCompleted: {
    page.refreshSensors()
    if (page.editingSlug.length > 0 && app.dashboards) {
        page.loadFromJson(app.dashboards.loadLayout(page.editingSlug).toString())
        page.statusText = app.dashboards.nameOf(page.editingSlug).toString()
    } else if (page.dashModel) {
        const raw = page.dashModel.layoutPath()
        page.statusText = raw.indexOf("error:") === 0 ? "" : raw
    }
}
```

### Step 6.3: rewire Save + Load buttons

Replace the Save button's `onClicked`:

```qml
onClicked: {
    if (page.editingSlug.length === 0) {
        // No active dashboard — prompt for a name, then save.
        newDashboardDialogFromEditor.layoutJson = page.serialize()
        newDashboardDialogFromEditor.open()
        return
    }
    const result = app.dashboards.saveLayout(page.editingSlug, page.serialize()).toString()
    if (page.isLayoutError(result)) {
        page.showError(result)
    } else {
        page.showSuccess(qsTr("Saved %1").arg(page.editingName))
    }
}
```

Replace the Load button's `onClicked`:

```qml
onClicked: {
    if (page.editingSlug.length === 0) {
        page.showError(qsTr("No dashboard selected to load"))
        return
    }
    page.loadFromJson(app.dashboards.loadLayout(page.editingSlug).toString())
    page.showSuccess(qsTr("Reloaded %1 from disk").arg(page.editingName))
}
```

Add the side dialog for "save without active slug" (at the bottom of the page):

```qml
NewDashboardDialog {
    id: newDashboardDialogFromEditor
    parent: Controls.Overlay.overlay
    property string layoutJson: "[]"
    onAccepted: function(slug) {
        // Persist the layout into the freshly-created dashboard.
        const r = app.dashboards.saveLayout(slug, newDashboardDialogFromEditor.layoutJson).toString()
        if (page.isLayoutError(r)) {
            page.showError(r)
        } else {
            page.editingSlug = slug
            page.editingName = app.dashboards.nameOf(slug).toString()
            app.goTo("editor:" + slug)
            page.showSuccess(qsTr("Saved as %1").arg(page.editingName))
        }
    }
}
```

### Step 6.4: build + smoke

```bash
cargo build -p linsight
```

### Step 6.5: commit

```bash
git add apps/linsight-gui/qml/CanvasEditorPage.qml
git commit -m "feat(gui): canvas editor honors editingSlug; save goes through DashboardsModel"
```

---

## Task 7 — DashboardViewPage (read-only)

**Files:**
- Create: `apps/linsight-gui/qml/DashboardViewPage.qml`
- Modify: `apps/linsight-gui/qml/Main.qml`

### Step 7.1: DashboardViewPage.qml

```qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    padding: 0
    title: displayName
    Accessible.role: Accessible.Pane
    Accessible.name: displayName

    property string slug: ""
    property string displayName: ""
    property var layout: []
    property var valueById: ({})

    function loadLayout() {
        if (!app.dashboards) return
        try {
            page.layout = JSON.parse(app.dashboards.loadLayout(slug).toString())
        } catch (e) {
            page.layout = []
        }
    }

    function refreshValues() {
        if (!app.dashModel) return
        try {
            const raw = JSON.parse(app.dashModel.tilesJson || "[]")
            const map = {}
            for (let i = 0; i < raw.length; ++i) {
                map[raw[i].id] = { name: raw[i].name, value: raw[i].value }
            }
            page.valueById = map
        } catch (e) { /* malformed early-boot JSON */ }
    }

    Component.onCompleted: { page.loadLayout(); page.refreshValues() }
    Connections {
        target: app.dashModel
        function onTilesJsonChanged() { page.refreshValues() }
    }

    Rectangle {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: app.tokens.pageHeaderHeight
        color: app.tokens.surface0
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 1
            color: app.tokens.separator
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: app.tokens.spaceXL
            anchors.rightMargin: app.tokens.spaceXL
            spacing: app.tokens.spaceM
            Controls.Label {
                text: page.displayName
                font.pixelSize: app.tokens.textHeading
                font.weight: app.tokens.weightBold
                font.family: app.tokens.sansFamily
                color: app.tokens.textPrimary
                Layout.fillWidth: true
            }
            Controls.Button {
                text: qsTr("Edit")
                icon.name: "document-edit-symbolic"
                onClicked: app.goTo("editor:" + page.slug)
            }
        }
    }

    Rectangle {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        color: app.tokens.surface0

        Repeater {
            model: page.layout
            delegate: Rectangle {
                x: modelData.x
                y: modelData.y
                width: modelData.w
                height: modelData.h
                radius: app.tokens.radiusCard
                color: app.tokens.surface1
                border.color: app.tokens.separator
                border.width: 1
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: app.tokens.spaceM
                    Controls.Label {
                        text: page.valueById[modelData.id] ? page.valueById[modelData.id].name : modelData.id
                        font.pixelSize: app.tokens.textCaption
                        opacity: 0.6
                        color: app.tokens.textPrimary
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        text: page.valueById[modelData.id] ? page.valueById[modelData.id].value : "…"
                        font.pixelSize: app.tokens.textSubheading + 4
                        font.weight: app.tokens.weightMedium
                        color: app.tokens.textPrimary
                        horizontalAlignment: Text.AlignHCenter
                        Layout.alignment: Qt.AlignHCenter
                        Layout.fillWidth: true
                    }
                }
            }
        }

        Controls.Label {
            anchors.centerIn: parent
            visible: page.layout.length === 0
            text: qsTr("This dashboard has no tiles yet — click Edit to add some.")
            opacity: 0.55
            color: app.tokens.textPrimary
            font.pixelSize: app.tokens.textBody
        }
    }
}
```

### Step 7.2: register the component in Main.qml

Add next to the existing `Component { id: editorPage … }`:

```qml
Component { id: dashboardViewPage; DashboardViewPage { } }
```

### Step 7.3: build + smoke

```bash
cargo build -p linsight
```

### Step 7.4: commit

```bash
git add apps/linsight-gui/qml/DashboardViewPage.qml apps/linsight-gui/qml/Main.qml
git commit -m "feat(gui): read-only DashboardViewPage with live values"
```

---

## Task 8 — Migration + boot routing + active dashboard persistence

**Files:**
- Modify: `apps/linsight-gui/src/qobjects/dashboards_model.rs`
- Modify: `apps/linsight-gui/qml/Main.qml`

### Step 8.1: migrate legacy dashboard.json on first DashboardsModel construction

Add to `DashboardsModelRust::default()`:

```rust
impl Default for DashboardsModelRust {
    fn default() -> Self {
        migrate_legacy_dashboard();
        let mut s = Self { summary_json: QString::from("[]") };
        s.rebuild_summary();
        s
    }
}

fn migrate_legacy_dashboard() {
    let Some(config) = config_dir_override() else { return };
    let legacy = config.join("dashboard.json");
    if !legacy.exists() { return; }
    let Some(dir) = dashboards_dir() else { return };
    // Skip if any dashboards already exist.
    if let Ok(mut iter) = std::fs::read_dir(&dir) {
        if iter.next().is_some() { return; }
    }
    let raw = match std::fs::read_to_string(&legacy) {
        Ok(s) => s, Err(_) => return,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v, Err(_) => return,
    };
    // editor_layout is the inner array OverviewModel.save_layout wrote.
    let layout = parsed.get("editor_layout").cloned().unwrap_or(serde_json::json!([]));
    if layout.as_array().map(|a| a.is_empty()).unwrap_or(true) {
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
        tracing::info!("migrated legacy dashboard.json to dashboards/default.json");
    }
}
```

### Step 8.2: add migration test

Add to `tests`:

```rust
#[test]
fn migration_from_legacy_dashboard_json() {
    with_temp(|| {
        let cfg = config_dir_override().unwrap();
        std::fs::create_dir_all(&cfg).unwrap();
        let legacy = cfg.join("dashboard.json");
        std::fs::write(&legacy, r#"{
            "schema_version":1,
            "pages":[],
            "editor_layout":[{"id":"cpu.util","x":0,"y":0,"w":200,"h":120}]
        }"#).unwrap();
        migrate_legacy_dashboard();
        let migrated = dashboards_dir().unwrap().join("default.json");
        assert!(migrated.exists());
        assert!(legacy.with_extension("json.migrated").exists());
        assert!(!legacy.exists());
    });
}

#[test]
fn migration_skipped_when_dashboards_already_exist() {
    with_temp(|| {
        let cfg = config_dir_override().unwrap();
        std::fs::create_dir_all(cfg.join("dashboards")).unwrap();
        std::fs::write(cfg.join("dashboards/existing.json"),
            r#"{"schema_version":1,"name":"E","slug":"existing","layout":[],"created_at":"","updated_at":""}"#).unwrap();
        let legacy = cfg.join("dashboard.json");
        std::fs::write(&legacy, r#"{"editor_layout":[{"id":"a","x":0,"y":0,"w":1,"h":1}]}"#).unwrap();
        migrate_legacy_dashboard();
        assert!(legacy.exists());  // not consumed
        assert!(!cfg.join("dashboards/default.json").exists());
    });
}
```

### Step 8.3: boot routing in Main.qml

Replace the existing `Component.onCompleted` block in `Main.qml`:

```qml
Component.onCompleted: {
    app.raise()
    app.requestActivate()
    const initialArg = Qt.application.arguments && Qt.application.arguments.length > 1
        ? Qt.application.arguments[1] : ""
    const known = ["overview","gpus","storage","network","editor","settings","about","licenses","credits"]
    if (initialArg && known.indexOf(initialArg) !== -1) {
        goTo(initialArg)
        return
    }
    // Reopen the last active dashboard if any.
    const active = app.preferences ? app.preferences.activeDashboard.toString() : ""
    if (active.length > 0 && app.dashboards.nameOf(active).toString().length > 0) {
        goTo("dashboard:" + active)
        return
    }
    goTo("overview")
}
```

### Step 8.4: run tests + build

```bash
cargo test -p linsight --lib qobjects::dashboards_model
cargo build -p linsight
```

Expected: 8 pass.

### Step 8.5: commit

```bash
git add apps/linsight-gui/src/qobjects/dashboards_model.rs apps/linsight-gui/qml/Main.qml
git commit -m "feat(gui): migrate legacy dashboard.json + reopen active dashboard at launch"
```

---

## Task 9 — i18n-extract + docs

**Files:**
- Modify: `Justfile`
- Modify: `CHANGELOG.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`

### Step 9.1: extend i18n-extract

In `Justfile`, the `i18n-extract` block adds the new QML files:

```
qml/ThemePicker.qml \
qml/DashboardViewPage.qml \
qml/DashboardNavRow.qml \
qml/NewDashboardDialog.qml \
qml/DeleteDashboardDialog.qml \
```

### Step 9.2: CHANGELOG entry

Prepend to `[Unreleased]`:

```markdown
### Theme system + custom dashboards

- **7 curated themes** selectable from Settings: System (Plasma),
  Tokyo Night, Catppuccin Mocha, Gruvbox Dark, Solarized Dark,
  Dracula, Nord. Selection persists in
  `~/.config/linsight/preferences.json`; `system` follows the Plasma
  color scheme, named themes pin a full palette. Picker is a
  mini-card swatch grid in the Settings page's Appearance card.
- **Multiple custom dashboards.** New `DashboardsModel` qobject backs
  `~/.config/linsight/dashboards/<slug>.json` (one file per
  dashboard). Sidebar gains a DASHBOARDS section listing the user's
  saved dashboards with kebab-menu Rename / Duplicate / Delete.
  `+ New dashboard` opens a Kirigami dialog; on accept the canvas
  editor opens with an empty layout for the new slug.
- **Read-only view mode.** `DashboardViewPage.qml` renders a saved
  dashboard with live values for daily use; Edit button switches to
  the canvas editor for the same slug.
- **Contextual editor.** `CanvasEditorPage` honors an `editingSlug`
  property — header reads "Editing: <name>", Save writes back to
  the active dashboard. Editor-from-scratch (no slug) prompts the
  user for a name on first save.
- **Active dashboard persists across launches.** Preferences tracks
  the last-opened dashboard; relaunch reopens it.
- **Legacy migration.** The pre-v0.4 single-dashboard
  `~/.config/linsight/dashboard.json` is auto-migrated into
  `dashboards/default.json` on first launch; the old file is
  renamed `.migrated` for safety.
```

### Step 9.3: AGENTS.md / CLAUDE.md update

Add to AGENTS.md "GUI conventions" section:

```markdown
- **Theme system.** All color roles in `DesignTokens.qml` go through
  `app.preferences.color(role)`; the empty-string return for surface
  roles under the `system` theme triggers a Kirigami.Theme fallback.
  When adding a new color role, extend both the role match in
  `PreferencesModel::color()` and every theme entry in `THEMES`.
- **Custom dashboards.** `DashboardsModel` owns
  `~/.config/linsight/dashboards/<slug>.json`. The sidebar's
  DASHBOARDS section is data-driven (Repeater over
  `summaryJson`). Page-key routing supports `editor:<slug>` and
  `dashboard:<slug>`; `Main.qml.goTo()` parses both.
```

Similar add to CLAUDE.md.

### Step 9.4: commit

```bash
git add Justfile CHANGELOG.md AGENTS.md CLAUDE.md
git commit -m "docs: themes + custom dashboards"
```

---

## Task 10 — End-to-end verification

### Step 10.1: full workspace test (debug + release)

```bash
cargo test --workspace
cargo test --release --workspace
```

Expected: 131 pass debug AND release (was 117 + 6 prefs + 8 dashboards).

### Step 10.2: just ci

```bash
just ci
```

Expected: green.

### Step 10.3: release build + live launch

```bash
just build-release
pkill -TERM -x linsightd linsight 2>/dev/null
rm -f /run/user/1000/linsight.sock
target/release/linsightd --socket /run/user/1000/linsight.sock >/tmp/linsightd.log 2>&1 &
target/release/linsight settings >/tmp/linsight-gui.log 2>&1 &
sleep 4
# Use the in-app screenshot path to capture Settings → Appearance theme grid
target/release/linsight settings --reduce-motion --screenshot /tmp/linsight-themes.png >/tmp/shot.log 2>&1
```

Verify the screenshot shows 7 theme tiles.

### Step 10.4: dashboards round-trip manual smoke

```bash
# (with the GUI running) click + New dashboard → name it "Test" →
# drag 3 tiles into the editor → save → switch to overview → switch
# back via the new sidebar entry → tiles render with live values.
```

Capture `/tmp/linsight-dashboard-view.png`.

### Step 10.5: commit + tag

```bash
git add CHANGELOG.md
git commit -m "chore: themes + custom dashboards green at v0.3.x-tip"
```

---

## Self-review checklist

- [x] **Spec coverage:** every spec section maps to a task. Theme: T1-T3. Dashboards: T4-T8. Migration: T8. i18n + docs: T9. Verification: T10.
- [x] **Placeholder scan:** no TODOs/TBDs in code blocks; commands have expected output.
- [x] **Type consistency:** `summary_json` qproperty used consistently; `editingSlug`/`editingName` properties used consistently in editor; slug routing prefixes (`editor:`, `dashboard:`) used identically across tasks.
- [x] **Migration coverage:** Task 8 includes the migration code + 2 tests.
- [x] **Atomic writes:** prefs and dashboards both use tmp+rename.
- [x] **Test coverage:** 6 prefs + 8 dashboards = 14 new unit tests; bumps workspace from 117 → ~131.
