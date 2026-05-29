<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Themes + Custom Dashboards — Design Spec

**Status:** approved 2026-05-26.
**Author:** Claude (paired with VisorCraft).
**Implementation plan:** `docs/superpowers/plans/2026-05-26-themes-and-custom-dashboards-roadmap.md` (forthcoming).

## Problem

Two product gaps in the v0.3.x GUI:

1. **No themeability.** `DesignTokens.qml` hardcodes `accent = #6c8cff` and pulls every surface color from `Kirigami.Theme.backgroundColor`. Users can't pick a curated palette the way Grexa and LinSync offer.
2. **No custom-dashboard story.** The Editor saves to a single `~/.config/linsight/dashboard.json`. There is no way to create, name, list, switch between, rename, duplicate, or delete user-built dashboards. The Editor is also the only place a dashboard is visible — there's no view-mode rendering of a finished dashboard for daily use.

Both are addressed in one spec because they share the same persistence
shape (`~/.config/linsight/preferences.json`, atomic JSON writes) and
the same QML wiring layer (new app-scope qobjects exposed through
`Main.qml`).

## Non-goals

- Light-mode variants of the named palettes. (Most distros default
  dark; ship dark-only, add a `-light` suffix variant scheme later if
  demand surfaces.)
- A theme editor (user creates their own palette). Out of scope; the
  registry is compile-time.
- Dashboard sharing / export / import beyond the per-file JSON
  format. Users can manually copy `~/.config/linsight/dashboards/*.json`.
- A dashboard "trash" with restore — deletion is permanent after
  confirm.
- Per-dashboard widget styles beyond the existing canvas-tile shape.
- Reordering dashboards in the sidebar by drag. (Sorted by
  `updated_at` descending; user can rename to influence order if
  needed.)

## Theme system

### Persistence

`~/.config/linsight/preferences.json` — atomic write (write-tmp +
rename). Owned by `PreferencesModel`. Schema:

```json
{
  "schema_version": 1,
  "theme": "tokyo-night",
  "active_dashboard": "production"
}
```

Missing fields fall back to defaults (`theme: "system"`,
`active_dashboard: null`). Malformed JSON is logged + a `.bad` backup
written; the GUI continues with defaults.

### Theme registry

A compile-time table in
`apps/linsight-gui/src/qobjects/preferences_model.rs`:

```rust
struct Theme {
    id:              &'static str, // wire identifier
    display_name:    &'static str, // shown in picker
    surface0:        &'static str, // page background
    surface1:        &'static str, // raised cards
    surface2:        &'static str, // hover / disconnected banner
    surface_sidebar: &'static str, // sidebar background
    text_primary:    &'static str, // body text
    separator_rgba:  &'static str, // 8-hex (with alpha)
    accent:          &'static str, // active nav, primary buttons, focus rings
    accent_mute:     &'static str, // active nav background, drag proxy
    accent_text:     &'static str, // text on accent (e.g. button label)
}
```

| id | name | accent | surface0 |
|---|---|---|---|
| `system` | System (Plasma) | `#6c8cff` | (Plasma `backgroundColor`) |
| `tokyo-night` | Tokyo Night | `#7aa2f7` | `#1a1b26` |
| `catppuccin-mocha` | Catppuccin Mocha | `#cba6f7` | `#1e1e2e` |
| `gruvbox-dark` | Gruvbox Dark | `#fabd2f` | `#282828` |
| `solarized-dark` | Solarized Dark | `#268bd2` | `#002b36` |
| `dracula` | Dracula | `#bd93f9` | `#282a36` |
| `nord` | Nord | `#88c0d0` | `#2e3440` |

Seven entries. `system` is special: surface colors fall back to
Kirigami.Theme, but `accent` / `accent_mute` / `accent_text` are
always specified (the LinSight cyan-indigo). All other themes specify
every color; for them Kirigami.Theme is ignored.

### Rust → QML surface

```rust
#[qproperty(QString, theme)]
#[qproperty(QString, active_dashboard)]
type PreferencesModel = …;

#[qinvokable]
fn set_theme(self: Pin<&mut Self>, id: &QString);
#[qinvokable]
fn set_active_dashboard(self: Pin<&mut Self>, slug: &QString);

/// Returns the active theme's color for `role`. If `role` is
/// `"accent" | "accent_mute" | "accent_text"`, returns the theme's
/// value for every theme including `system`. For surface/text/
/// separator roles, returns the theme's value for named themes, or
/// an empty string for `system` (signaling QML to fall back to
/// Kirigami.Theme).
#[qinvokable]
fn color(self: &Self, role: &QString) -> QString;

/// Returns a JSON array of `{id, display_name, accent, surface0}`
/// for the picker grid.
#[qinvokable]
fn themes_json(self: &Self) -> QString;
```

### DesignTokens binding

```qml
readonly property color accent: app.preferences.color("accent")
readonly property color accent_mute: app.preferences.color("accent_mute")
readonly property color surface0: {
    const c = app.preferences.color("surface0")
    return c.length > 0 ? c : Kirigami.Theme.backgroundColor
}
// …same fallback pattern for every surface/text/separator role
```

Every existing `app.tokens.<role>` consumer continues to work. The
binding chain re-evaluates on `themeChanged` so the entire UI
repaints when the user picks a new theme.

### Picker UI

`ThemePicker.qml` — used as the content of the Settings page's
Appearance card. Replaces the current placeholder.

Layout: `Flow` of mini-card tiles, ~140 × 80 px each:

```
┌──────────────────────────┐
│ ▌▌▌  ┌────┐              │
│ ▌▌▌  │ ●  │              │   ← swatch shows a stylised
│ ▌▌▌  └────┘              │     sidebar strip + a tile
└──────────────────────────┘     in the target theme's
   Tokyo Night                   surface/accent colors
```

- Active theme: 2 px accent-colored ring + a check mark in the
  top-right corner.
- Click → instant theme apply (no save button — autosaves on every
  change).
- Keyboard nav: Tab focuses the next tile, Enter / Space selects
  (mirrors `NavItem`'s pattern from the audit).
- Accessible: each tile has `Accessible.role: RadioButton` +
  `Accessible.name: displayName + " theme"`.

## Custom dashboards

### Persistence

Per-dashboard file in `~/.config/linsight/dashboards/<slug>.json`:

```json
{
  "schema_version": 1,
  "name": "Production",
  "slug": "production",
  "layout": [
    {"id": "cpu.util", "x": 16, "y": 16, "w": 200, "h": 120},
    {"id": "nvml.gpu0.util", "x": 240, "y": 16, "w": 200, "h": 120}
  ],
  "created_at": "2026-05-26T03:00:00Z",
  "updated_at": "2026-05-26T03:15:42Z"
}
```

Atomic write per file. `created_at` / `updated_at` are ISO-8601 UTC
strings.

### Slugs

Auto-derived from name: lowercase, ASCII alphanumerics only,
non-alphanumerics → `-`, collapse repeated `-`, strip leading/
trailing `-`, truncate at 40 chars. Collisions get numeric suffix
`-2`, `-3`, … (probing on the filesystem; up to `-99`).

Empty slug (e.g. user enters all-emoji name) → reject with error.

### Rust → QML surface

```rust
#[qobject]
#[qml_element]
#[qproperty(QString, summary_json)]    // notifies on listChanged
type DashboardsModel = …;

#[qinvokable]
fn create(self: Pin<&mut Self>, name: &QString) -> QString;
    // Returns the new slug, or "error: …".

#[qinvokable]
fn rename(self: Pin<&mut Self>, slug: &QString, new_name: &QString) -> QString;
    // Returns the (possibly new) slug, or "error: …".

#[qinvokable]
fn duplicate(self: Pin<&mut Self>, slug: &QString) -> QString;
    // Returns the new slug, or "error: …".

#[qinvokable]
fn remove(self: Pin<&mut Self>, slug: &QString) -> bool;

#[qinvokable]
fn save_layout(self: Pin<&mut Self>, slug: &QString, layout_json: &QString) -> QString;
    // Returns the path written, or "error: …".

#[qinvokable]
fn load_layout(self: &Self, slug: &QString) -> QString;
    // Returns the layout JSON array (just the `layout` field), or "[]".

#[qinvokable]
fn name_of(self: &Self, slug: &QString) -> QString;
```

`summary_json` is the qproperty consumed by the sidebar repeater:

```json
[
  {"slug": "production", "name": "Production", "updated_at": "..."},
  ...
]
```

Sorted by `updated_at` descending so the most recently-touched
dashboard floats to the top. Emits change notification atomically on
every list-mutating call.

### Navigation

```
WORKSPACE          DASHBOARDS         SYSTEM
  Overview           Production         Settings
  GPUs               Trading Rig        About
  Storage            Music Box
  Network          + New dashboard
                   ✎ Edit Current      ← only when a custom dashboard is the active page
```

- `Main.qml` adds a `DASHBOARDS` section between WORKSPACE and SYSTEM.
- The section is a `Repeater` over `JSON.parse(app.dashboards.summary_json)`.
- Each entry is a `DashboardNavRow` (new component): same look as
  `NavItem` but adds a hover-only kebab button that opens a menu
  (Rename, Duplicate, Delete).
- `+ New dashboard` is a sidebar row that opens `NewDashboardDialog`.
- `Edit Current` is conditional on `app.currentPageKey.startsWith("dashboard:")`.

Page-key scheme:
- `overview`, `gpus`, `storage`, `network` (existing presets)
- `editor` (existing — opens canvas editor with NO active dashboard,
  prompts user to create or pick)
- `editor:<slug>` (new — opens canvas editor for that dashboard)
- `dashboard:<slug>` (new — opens view-mode for that dashboard)
- `settings`, `about`, `licenses`, `credits` (existing)

`Main.qml.goTo(key)` parses the prefix and routes accordingly.

### "+ New dashboard" flow

1. Click `+ New dashboard` → opens `NewDashboardDialog` (a
   `Kirigami.PromptDialog`).
2. User types a name. Validation rules below.
3. Click Create → `DashboardsModel.create(name)` → returns new slug.
4. `goTo("editor:" + slug)` opens the editor with an empty canvas
   for the new dashboard.

Name validation:
- 1-40 characters
- At least one ASCII alphanumeric
- No null bytes, no `/`, no leading `.`
- Dialog disables Create until name is valid; shows inline error
  text otherwise.

### Edit flow

`CanvasEditorPage` gains:

```qml
property string editingSlug: ""
property string editingName: ""

Component.onCompleted: {
    // If the page was opened via "editor:<slug>", load that
    // dashboard's layout; otherwise the canvas starts empty.
    if (page.editingSlug.length > 0 && app.dashboards) {
        page.loadFromJson(app.dashboards.loadLayout(page.editingSlug))
        page.editingName = app.dashboards.nameOf(page.editingSlug)
    }
}
```

Header changes from "Editor" to "Editing: <name>" when a slug is
active.

Save button:
- If `editingSlug` is empty → prompts user to create (opens
  NewDashboardDialog with the layout pre-armed).
- Else → `app.dashboards.saveLayout(slug, page.serialize())` →
  banner shows success.

### View flow

`DashboardViewPage.qml` is a new page that:

1. Receives `slug` property at construction time.
2. Loads `app.dashboards.loadLayout(slug)` once.
3. Renders each tile as a `SensorTile` instance positioned at
   `(t.x, t.y)` with size `(t.w, t.h)`.
4. Tile values come from `app.dashModel.tilesJson` (the live
   OverviewModel) — the view binds to the live `valueById[id]`
   lookup so values refresh on every sample tick without
   re-rendering the layout.
5. Header: `<name>` plus an `Edit` button on the right that calls
   `app.goTo("editor:" + slug)`.

No drag, resize, palette, or save UI. Read-only.

Empty layout: shows a centered "This dashboard has no tiles yet —
click Edit to add some."

### Active dashboard persistence

When the user opens any `dashboard:<slug>`, `PreferencesModel`
saves `active_dashboard = slug`. On next launch, if the
preference is set and the slug still exists, `Main.qml`'s
`Component.onCompleted` routes to `dashboard:<slug>` instead of
`overview`. If the slug was deleted between sessions, fall back to
`overview` silently.

### Delete confirmation

`Kirigami.PromptDialog` with title "Delete <name>?" and body
"This dashboard's layout will be permanently removed. The
underlying sensors continue to run." Buttons: `Cancel` (default),
`Delete` (destructive). On confirm:
1. `DashboardsModel.remove(slug)` deletes the file.
2. If the deleted slug was the active one, route back to `overview`.
3. Sidebar `summary_json` updates → row disappears.

## Components / files

### New Rust

- `apps/linsight-gui/src/qobjects/preferences_model.rs` (~250 LoC)
  - Theme table (~110 LoC of hex-string constants)
  - JSON load/save + atomic write
  - 6+ unit tests
- `apps/linsight-gui/src/qobjects/dashboards_model.rs` (~300 LoC)
  - Per-file IO, slug logic, list summary
  - 8+ unit tests
- `apps/linsight-gui/src/qobjects/mod.rs` — register new qobjects.

### New QML

- `apps/linsight-gui/qml/ThemePicker.qml`
- `apps/linsight-gui/qml/DashboardViewPage.qml`
- `apps/linsight-gui/qml/DashboardNavRow.qml`
- `apps/linsight-gui/qml/NewDashboardDialog.qml`

### Modified

- `apps/linsight-gui/qml/Main.qml`
  - Declare `app.preferences`, `app.dashboards`.
  - Add DASHBOARDS sidebar section with repeater + "+ New" + "Edit
    Current" rows.
  - Extend `goTo(key)` to parse `editor:<slug>` and
    `dashboard:<slug>`.
  - `Component.onCompleted` reads `app.preferences.activeDashboard`
    and routes there if set, else `overview`.
- `apps/linsight-gui/qml/DesignTokens.qml`
  - Every color role bound through `app.preferences.color(role)`
    with a Kirigami fallback for `system`.
- `apps/linsight-gui/qml/SettingsPage.qml`
  - Appearance card swapped from the current placeholder to
    `ThemePicker { }`.
- `apps/linsight-gui/qml/CanvasEditorPage.qml`
  - Add `editingSlug` + `editingName` properties.
  - Replace save/load calls from `OverviewModel.save_layout` /
    `load_layout` to `DashboardsModel.saveLayout` /
    `loadLayout`. (The OverviewModel invokables stay for backwards
    compatibility — the single-dashboard `dashboard.json` file is
    silently migrated on first launch into a `default` dashboard.)
  - Header label switches between "Editor" and "Editing: <name>".
- `Justfile`
  - Add `qml/ThemePicker.qml`, `qml/DashboardViewPage.qml`,
    `qml/DashboardNavRow.qml`, `qml/NewDashboardDialog.qml` to the
    `i18n-extract` target.

### Migration

On first launch after the upgrade, `DashboardsModel` checks for
`~/.config/linsight/dashboard.json` (the old single-dashboard
file). If present and its `editor_layout` array is non-empty:
1. Create `~/.config/linsight/dashboards/default.json` with the
   migrated layout.
2. Rename the old file to `dashboard.json.migrated` (don't delete —
   user might want it).
3. Set `preferences.active_dashboard = "default"` if not already set.
4. Log a one-shot info message.

Skip migration if the dashboards dir already has any `.json` (the
user has already migrated or this is a fresh install).

## Error handling

| Failure | Behavior |
|---|---|
| `~/.config/linsight/` not creatable | `PreferencesModel` and `DashboardsModel` both log error and fall back to in-memory defaults. GUI works; nothing is persisted. |
| `preferences.json` malformed JSON | Log warn, rename to `preferences.json.bad`, write fresh defaults, show one-shot warning toast on the Settings page. |
| `dashboards/<slug>.json` malformed | Log warn, exclude from list. Sibling dashboards remain functional. |
| Slug derivation produces empty string | `create()` returns `"error: name must contain at least one letter or digit"`. |
| Slug collision | Auto-suffix `-2`, `-3`, … up to `-99`. Past that return `"error: too many similarly-named dashboards"`. |
| Disk full on save | `save_layout` returns `"error: <io message>"`. Editor banner displays it. |

## Testing

### Unit tests

`preferences_model::tests`:
1. `default_when_file_missing` — returns `theme: "system"`,
   `active_dashboard: None`.
2. `round_trip` — set theme, save, reload, theme matches.
3. `malformed_falls_back_to_default_and_renames_bad` — writes bad
   JSON, load surfaces defaults and `preferences.json.bad` exists.
4. `unknown_theme_id_falls_back_to_system` — setting `"nonexistent"`
   resolves to system on next load.
5. `color_returns_empty_for_surface_when_system_theme` — verifies
   the QML fallback contract.
6. `themes_json_lists_every_registered_theme` — count + ids
   present.

`dashboards_model::tests`:
1. `slug_derivation_normalizes` — "Production Server #2" →
   `production-server-2`.
2. `slug_collision_appends_suffix` — create("X") twice, second is
   `x-2`.
3. `create_writes_file` — fresh dashboards dir, expected path
   exists with correct schema.
4. `save_load_round_trip` — save a layout, load returns identical
   JSON.
5. `rename_changes_slug_and_filename` — old file gone, new file
   present, summary updated.
6. `duplicate_creates_independent_copy` — modifying the copy
   doesn't affect the original.
7. `remove_purges_file_and_updates_summary` — file gone, summary
   omits it.
8. `malformed_file_excluded_from_summary` — write garbage file,
   summary still loads cleanly without it.
9. `migration_from_legacy_dashboard_json` — old file present →
   `default` slug created with migrated layout, old file renamed
   `.migrated`.

### Integration

- `just gui-smoke` keeps passing (no functional regression for
  Overview).
- Manual: launch GUI, exercise:
  1. Pick each theme → screenshot.
  2. Create "Test" dashboard → drag 3 tiles → save → verify file on
     disk.
  3. Switch to overview → switch back to "Test" → tiles render
     read-only with live values.
  4. Rename "Test" → "Renamed Test" → file renamed, sidebar
     updates.
  5. Duplicate → "Renamed Test (copy)" appears, layout copied.
  6. Delete original → confirmed, gone, view falls back to
     overview.
  7. Restart binary → "Renamed Test (copy)" reopens (active
     dashboard persisted).

### Visual artifacts

Post-implementation, capture:
- `/tmp/linsight-theme-tokyo-night.png`
- `/tmp/linsight-theme-gruvbox.png`
- `/tmp/linsight-dashboard-edit.png` (custom dashboard in editor)
- `/tmp/linsight-dashboard-view.png` (same dashboard in view mode)

## Open questions

None remaining at spec-write time. The two design forks resolved
via brainstorming (theme scope = Plasma-aware + named palettes;
nav placement = sidebar section + contextual Edit; delete UX =
simple confirm; picker style = mini-card preview).

## Implementation order

The plan doc will sequence the work; rough shape:

1. **PreferencesModel + theme registry + JSON I/O + tests.**
2. **DesignTokens binding through PreferencesModel.**
3. **ThemePicker QML + SettingsPage wiring.**
4. **DashboardsModel + JSON I/O + slug logic + tests.**
5. **NewDashboardDialog + sidebar section + nav routing.**
6. **CanvasEditorPage refactor to use slug-aware DashboardsModel.**
7. **DashboardViewPage + sidebar click routing.**
8. **Migration from legacy `dashboard.json`.**
9. **Active-dashboard persistence + boot routing.**
10. **`just ci` green; release build; live launch + screenshots.**

Each batch is independently shippable (tests + green build); the
plan will detail handoff points.
