// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `OverviewModel` — a QObject that owns every page's tile state.
//!
//! `start()` (called from QML `Component.onCompleted`) loads the daemon's
//! sensor catalogue, subscribes, and spawns a worker thread. Each sample
//! gets format-converted and pushed into the qproperties via
//! `qt_thread.queue(...)` setter calls. QML bindings on `cpuText`,
//! `memText`, and `tilesJson` re-evaluate from the resulting NOTIFY
//! signals.
//!
//! `save_layout` / `load_layout` / `layout_path` back the canvas editor.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::thread;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use linsight_core::{Reading, SensorId, Unit};
use linsight_protocol::SensorInfo;
use serde::Serialize;

use super::workspace_handle::with_workspace;

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
        #[qproperty(QString, cpu_text)]
        #[qproperty(QString, mem_text)]
        #[qproperty(QString, cpu_temp_text)]
        #[qproperty(QString, cpu_freq_text)]
        #[qproperty(QString, tiles_json)]
        #[qproperty(bool, connected)]
        type OverviewModel = super::OverviewModelRust;

        #[qinvokable]
        fn start(self: Pin<&mut OverviewModel>);

        /// Persist a canvas-editor layout JSON to
        /// `~/.config/linsight/dashboard.json`. Phase 6b uses a minimal
        /// `[{id,x,y,w,h}]` shape stored under a single "Custom" page so the
        /// editor stays decoupled from the full `DashboardSpec` schema
        /// while still living in the same file.
        ///
        /// Returns the absolute path written on success, or an
        /// `error: …` prefixed message on failure. QML treats both as
        /// status strings.
        #[qinvokable]
        fn save_layout(self: Pin<&mut OverviewModel>, json: QString) -> QString;

        /// Read the canvas-editor layout JSON previously written by
        /// `save_layout`. Missing file → `"[]"`. Malformed → `"[]"`.
        #[qinvokable]
        fn load_layout(self: Pin<&mut OverviewModel>) -> QString;

        /// Absolute path of the dashboard JSON file the editor writes to.
        /// Surfaced in the editor's status strip.
        #[qinvokable]
        fn layout_path(self: Pin<&mut OverviewModel>) -> QString;

        /// Returns the bundled-at-build-time third-party credits markdown
        /// (`docs/third-party-notices.md`). Used by the Credits page —
        /// QML's `XMLHttpRequest` against `qrc:/` URLs does not reliably
        /// reach the `DONE` ready-state on local resources, so we plumb
        /// the same content through a `Q_INVOKABLE` instead.
        #[qinvokable]
        fn credits_text(self: &OverviewModel) -> QString;

        /// Returns the bundled-at-build-time LICENSE (GPL-3.0) text.
        /// Mirrors `credits_text` for the same XHR-on-qrc reason.
        #[qinvokable]
        fn gpl_text(self: &OverviewModel) -> QString;

        /// Returns the narrative attribution document (the curated
        /// CREDITS.md). The Acknowledgments tab on the Licenses page
        /// renders this verbatim.
        #[qinvokable]
        fn narrative_credits_text(self: &OverviewModel) -> QString;

        /// Returns the cargo-about-generated third-party listing
        /// parsed into a JSON array of
        /// `{name, version, license, url}` objects. The Credits page
        /// renders this as a sortable, filterable table.
        #[qinvokable]
        fn third_party_credits_json(self: &OverviewModel) -> QString;

        /// True if the named environment variable is set in this process.
        /// Used by the Settings page to flip the always-on indicators
        /// between "on" and "off" instead of showing a fixed checkbox
        /// regardless of the actual env state.
        ///
        /// Caveat: this reads the GUI process's env, not the daemon's.
        /// In the systemd-unit setup the two share an env, but a
        /// detached daemon could diverge; surfacing daemon-side env is
        /// tracked in the open-followups doc.
        #[qinvokable]
        fn env_is_set(self: &OverviewModel, name: &QString) -> bool;
    }

    impl cxx_qt::Threading for OverviewModel {}
}

#[derive(Clone, Debug, Serialize)]
struct TileJson {
    id: String,
    category: String,
    device: Option<String>,
    name: String,
    /// Resolved device label (nickname || model || disambiguated). Rendered
    /// as a secondary title line under `name`. Absent for non-device-scoped
    /// sensors (CPU, memory, …).
    #[serde(rename = "deviceLabel", skip_serializing_if = "Option::is_none")]
    device_label: Option<String>,
    #[serde(rename = "parentDevice", skip_serializing_if = "Option::is_none")]
    parent_device: Option<String>,
    value: String,
    kind: String,
    unit: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    sparkline: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    rows: Vec<Vec<serde_json::Value>>,
}

struct SampleState {
    tiles: HashMap<String, TileJson>,
    id_order: Vec<String>,
    units: HashMap<String, Unit>,
    /// Sensor ids tagged constant ([`linsight_core::STATIC_TAG`]). Byte
    /// values for these render as a rounded whole-GB capacity (e.g.
    /// "32 GB") rather than a fractional binary size ("31.84 GiB").
    static_ids: HashSet<String>,
    /// Rolling scalar window per sensor (max 30 points) for sparkline rendering.
    sparklines: HashMap<String, Vec<f64>>,
}

impl SampleState {
    fn push_sparkline(&mut self, sensor: &str, value: f64) {
        const MAX_POINTS: usize = 30;
        let buf = self.sparklines.entry(sensor.to_string()).or_default();
        buf.push(value);
        if buf.len() > MAX_POINTS {
            buf.remove(0);
        }
    }
}

/// Apply a single sample to the in-memory tile state: format its value,
/// update the matching tile's text + rows, and push its scalar onto the
/// rolling sparkline buffer.
///
/// Extracted so the pump thread can call it once per sample in a tight
/// drain loop before doing the once-per-batch JSON re-serialize.
fn apply_sample(state: &mut SampleState, sample: &linsight_core::Sample) {
    let sensor_id = sample.sensor.as_str().to_string();
    let unit = state.units.get(&sensor_id).cloned().unwrap_or(Unit::Count);
    let is_static = state.static_ids.contains(&sensor_id);
    let (value_str, table_rows) =
        format_reading_with_rows(&sample.sensor, &sample.reading, &unit, is_static);
    if let Some(tile) = state.tiles.get_mut(&sensor_id) {
        tile.value = value_str;
        tile.rows = table_rows;
    }
    if let linsight_core::Reading::Scalar(v) = &sample.reading {
        state.push_sparkline(&sensor_id, *v);
    }
}

/// Apply `first` plus every other sample currently buffered in `rx` to
/// `state`, then return the total number applied. Does not block: as
/// soon as `rx.try_recv()` returns Empty (or Disconnected), the drain
/// stops and the caller does its one-per-batch render.
///
/// Lifted out of the pump thread so the batch-drain optimization is
/// directly unit-testable without a Qt context.
fn drain_into_state(
    rx: &std::sync::mpsc::Receiver<linsight_core::Sample>,
    state: &mut SampleState,
    first: linsight_core::Sample,
) -> usize {
    apply_sample(state, &first);
    let mut count = 1;
    while let Ok(more) = rx.try_recv() {
        apply_sample(state, &more);
        count += 1;
    }
    count
}

pub struct OverviewModelRust {
    cpu_text: QString,
    mem_text: QString,
    /// CPU package temperature, pre-formatted (e.g. "77.0°C").
    /// Empty until the first cpu.temp_c sample lands; "?" if the
    /// daemon returns Unsupported (no coretemp / k10temp module).
    cpu_temp_text: QString,
    /// Average CPU frequency, pre-formatted (e.g. "2.53 GHz").
    /// Empty until the first cpu.freq_hz sample lands; "?" if the
    /// daemon returns Unsupported (no cpufreq subsystem).
    cpu_freq_text: QString,
    tiles_json: QString,
    /// True once the handshake + first subscribe have completed and the
    /// sample-pump thread is alive. Flips to false when the daemon closes
    /// the socket so QML can show a disconnected banner instead of
    /// freezing at the last value.
    connected: bool,
    started: bool,
}

impl Default for OverviewModelRust {
    fn default() -> Self {
        Self {
            cpu_text: QString::from("…"),
            mem_text: QString::from("…"),
            cpu_temp_text: QString::from("…"),
            cpu_freq_text: QString::from("…"),
            tiles_json: QString::from("[]"),
            connected: false,
            started: false,
        }
    }
}

impl ffi::OverviewModel {
    /// Idempotent. Called from QML's `Component.onCompleted`.
    pub fn start(mut self: Pin<&mut Self>) {
        if self.as_mut().rust().started {
            return;
        }
        self.as_mut().rust_mut().started = true;

        let qt_thread = self.qt_thread();
        let client = with_workspace(|ws| ws.client());

        let sensor_infos: Vec<SensorInfo> = client.sensor_infos();
        if sensor_infos.is_empty() {
            tracing::warn!("daemon advertised zero sensors");
            return;
        }

        let mut tiles: HashMap<String, TileJson> = HashMap::new();
        let mut units: HashMap<String, Unit> = HashMap::new();
        let mut id_order: Vec<String> = Vec::new();
        let mut static_ids: HashSet<String> = HashSet::new();
        for info in &sensor_infos {
            let category = serialize_category(info.category);
            if info.tags.iter().any(|t| t == linsight_core::STATIC_TAG) {
                static_ids.insert(info.id.as_str().to_string());
            }
            let kind = serialize_kind(info.kind);
            let id = info.id.as_str().to_string();
            id_order.push(id.clone());
            units.insert(id.clone(), info.unit.clone());
            tiles.insert(
                id.clone(),
                TileJson {
                    id,
                    category,
                    device: info.device_id.clone(),
                    name: info.display_name.clone(),
                    device_label: device_label_for(info),
                    parent_device: parent_device_for(info),
                    value: "…".into(),
                    kind,
                    unit: "".into(),
                    sparkline: vec![],
                    rows: vec![],
                },
            );
        }

        // Initial render so the catalogue is visible before the first
        // sample arrives. Setter is called on the GUI thread here, so its
        // changed signal does fire correctly.
        let initial = serialize_tiles(&id_order, &tiles);
        let init_q = QString::from(initial.as_str());
        self.as_mut().set_tiles_json(init_q);

        if let Err(e) = client.subscribe(sensor_infos.iter().map(|s| s.id.clone()).collect()) {
            tracing::error!(error = ?e, "subscribe failed");
            return;
        }

        // Push the user's persisted sample-interval choice to the daemon.
        // Each client has its own pump-tick, so this only affects the
        // current LinSight process. Failure here isn't fatal — the
        // daemon falls back to its compiled-in default
        // (`PUMP_INTERVAL_DEFAULT_MS`).
        let persisted_ms = crate::qobjects::preferences_model::load_prefs().sample_interval_ms;
        if let Err(e) = client.set_pump_interval_ms(persisted_ms, std::time::Duration::from_secs(5))
        {
            tracing::warn!(error = %e, ms = persisted_ms,
                "set_pump_interval_ms at handshake failed; daemon will use its default tick");
        }

        let Some(rx) = client.take_sample_rx() else {
            tracing::warn!("sample receiver already taken; live updates will not appear");
            return;
        };

        // Subscribe succeeded and the rx is live; flip the UI to
        // "connected" so any disconnected banner stays hidden.
        self.as_mut().set_connected(true);

        // The sample-pump and the catalogue-refresh threads both mutate
        // the same `tiles` map (sample writes `value`, broadcast writes
        // `name`), so the state lives behind an `Arc<Mutex<...>>`. The
        // critical sections are tiny — a HashMap lookup or a per-info
        // rename loop — so contention with the 1 Hz sample rate is
        // negligible.
        let state = Arc::new(Mutex::new(SampleState {
            tiles,
            id_order,
            units,
            static_ids,
            sparklines: HashMap::new(),
        }));

        // Catalogue-refresh worker: when the daemon sends a
        // `SensorListBroadcast` (e.g. after a nickname change), rebuild
        // each tile's `name` from the fresh `SensorInfo` and push a new
        // tiles_json so QML re-renders titles in place. Spawned BEFORE
        // the sample pump so we don't miss any broadcast that races
        // with the first sample.
        let catalogue_rx = client.subscribe_catalogue();
        {
            let state = Arc::clone(&state);
            let qt_thread = qt_thread.clone();
            thread::spawn(move || {
                while let Ok(fresh) = catalogue_rx.recv() {
                    let json = {
                        let mut guard = state.lock().expect("tile state poisoned");
                        for info in &fresh {
                            let id = info.id.as_str();
                            if let Some(tile) = guard.tiles.get_mut(id) {
                                tile.name = info.display_name.clone();
                                tile.device_label = device_label_for(info);
                                tile.parent_device = parent_device_for(info);
                            }
                        }
                        serialize_tiles(&guard.id_order, &guard.tiles)
                    };
                    let qjson = QString::from(json.as_str());
                    let _ = qt_thread.queue(move |mut pin| {
                        pin.as_mut().set_tiles_json(qjson);
                    });
                }
                // catalogue_rx closed — the dispatcher exited. Sample
                // pump will surface the disconnected banner; nothing
                // else to do here.
            });
        }

        // Sample pump: drains the per-sample receiver, updates the tile
        // value strings, and emits a fresh tiles_json plus the
        // shortcut cpu_text/mem_text properties.
        //
        // Coalesces all samples available at the moment of wake-up into a
        // single render. The daemon emits one batch per pump tick (default
        // 150 ms); with ~230 active sensors this means ~1.5k samples/sec
        // would otherwise cause ~1.5k full re-serializes of the tiles JSON.
        // After this change we do one re-serialize per pump tick — the
        // visible refresh rate is unchanged (it's bounded by tick rate),
        // but the per-sample CPU work drops by ~N (= number of active
        // sensors) on a populated system.
        {
            let state = Arc::clone(&state);
            thread::spawn(move || {
                while let Ok(first) = rx.recv() {
                    let (json, cpu, mem, cpu_temp, cpu_freq) = {
                        let mut guard = state.lock().expect("tile state poisoned");
                        // Drain the first sample plus any others already
                        // queued (typically the rest of this pump tick's
                        // batch) into the in-memory state. Non-blocking;
                        // returns once the channel is empty.
                        drain_into_state(&rx, &mut guard, first);
                        // Now snapshot sparklines and re-apply to tiles. Only
                        // need to do this once per batch since all samples
                        // share the same destination state.
                        let sparklines_snapshot: HashMap<String, Vec<f64>> =
                            guard.sparklines.clone();
                        for tile in guard.tiles.values_mut() {
                            tile.sparkline =
                                sparklines_snapshot.get(&tile.id).cloned().unwrap_or_default();
                        }
                        let json = serialize_tiles(&guard.id_order, &guard.tiles);
                        let cpu = guard.tiles.get("cpu.util").map(|t| t.value.clone());
                        let mem = guard.tiles.get("mem.used_bytes").map(|t| t.value.clone());
                        let cpu_temp = guard.tiles.get("cpu.temp_c").map(|t| t.value.clone());
                        let cpu_freq = guard.tiles.get("cpu.freq_hz").map(|t| t.value.clone());
                        (json, cpu, mem, cpu_temp, cpu_freq)
                    };
                    let qjson = QString::from(json.as_str());
                    let qcpu = cpu.map(|s| QString::from(s.as_str()));
                    let qmem = mem.map(|s| QString::from(s.as_str()));
                    let qctemp = cpu_temp.map(|s| QString::from(s.as_str()));
                    let qcfreq = cpu_freq.map(|s| QString::from(s.as_str()));
                    // queue() returns Err only when the Qt thread has shut
                    // down (i.e. the app is exiting). Silently discarding is
                    // correct in that case.
                    let _ = qt_thread.queue(move |mut pin| {
                        pin.as_mut().set_tiles_json(qjson);
                        if let Some(q) = qcpu {
                            pin.as_mut().set_cpu_text(q);
                        }
                        if let Some(q) = qmem {
                            pin.as_mut().set_mem_text(q);
                        }
                        if let Some(q) = qctemp {
                            pin.as_mut().set_cpu_temp_text(q);
                        }
                        if let Some(q) = qcfreq {
                            pin.as_mut().set_cpu_freq_text(q);
                        }
                    });
                }
                // rx.recv() returned Err — the sender side dropped because
                // the daemon connection closed. Tell QML so it can show a
                // banner instead of leaving tiles frozen at last-known values.
                tracing::warn!("sample stream ended; surfacing disconnected state to QML");
                let _ = qt_thread.queue(|mut pin| {
                    pin.as_mut().set_connected(false);
                });
            });
        }
    }

    /// Save the canvas-editor's placements JSON. We embed the editor's
    /// flat `[{id,x,y,w,h}]` array under a single `Custom` page so the
    /// on-disk file stays valid against `DashboardSpec` once the
    /// Custom-page renderer in `linsight-core` learns to translate
    /// pixel coords back to the 24-column grid. Until then the editor
    /// owns the file end-to-end and round-trips it through a
    /// `editor_layout` extension field that `DashboardSpec` ignores.
    pub fn save_layout(self: Pin<&mut Self>, json: QString) -> QString {
        let path = match layout_path_buf() {
            Ok(p) => p,
            Err(e) => return QString::from(format!("error: {e}").as_str()),
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return QString::from(format!("error: mkdir {}: {e}", parent.display()).as_str());
        }
        // Validate the payload is JSON; refuse silently-corrupted writes.
        let body = json.to_string();
        let parsed: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => return QString::from(format!("error: invalid JSON: {e}").as_str()),
        };
        let doc = serde_json::json!({
            "schema_version": linsight_core::dashboard::DASHBOARD_SCHEMA_VERSION,
            "pages": [],
            "editor_layout": parsed,
        });
        let pretty = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| body.clone());
        if let Err(e) = std::fs::write(&path, pretty) {
            return QString::from(format!("error: write {}: {e}", path.display()).as_str());
        }
        QString::from(path.to_string_lossy().as_ref())
    }

    /// Counterpart to [`save_layout`]. Returns the inner editor array.
    pub fn load_layout(self: Pin<&mut Self>) -> QString {
        let path = match layout_path_buf() {
            Ok(p) => p,
            Err(_) => return QString::from("[]"),
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return QString::from("[]"),
        };
        let doc: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return QString::from("[]"),
        };
        let inner = doc.get("editor_layout").cloned().unwrap_or_else(|| serde_json::json!([]));
        QString::from(inner.to_string().as_str())
    }

    /// User-visible path label.
    pub fn layout_path(self: Pin<&mut Self>) -> QString {
        match layout_path_buf() {
            Ok(p) => QString::from(p.to_string_lossy().as_ref()),
            Err(e) => QString::from(format!("error: {e}").as_str()),
        }
    }

    pub fn credits_text(&self) -> QString {
        QString::from(BUNDLED_CREDITS)
    }

    pub fn gpl_text(&self) -> QString {
        QString::from(BUNDLED_GPL)
    }

    pub fn narrative_credits_text(&self) -> QString {
        QString::from(BUNDLED_NARRATIVE_CREDITS)
    }

    pub fn third_party_credits_json(&self) -> QString {
        QString::from(third_party_credit_entries_json(BUNDLED_CREDITS).as_str())
    }

    pub fn env_is_set(&self, name: &QString) -> bool {
        let name = name.to_string();
        std::env::var_os(name).map(|v| !v.is_empty()).unwrap_or(false)
    }
}

const BUNDLED_CREDITS: &str = include_str!("../../../../docs/third-party-notices.md");
const BUNDLED_GPL: &str = include_str!("../../../../LICENSE");
const BUNDLED_NARRATIVE_CREDITS: &str = include_str!("../../../../CREDITS.md");

// --- cargo-about parser ----------------------------------------
//
// `docs/third-party-notices.md` follows cargo-about's standard
// layout: an outer "## License Texts" section, with one "###
// <Human-readable license name>" subsection per license group, each
// containing a "Used by:" markdown list of `- [`crate version`](url)`
// entries. We walk the file line-by-line, tracking the current
// license, and emit `{name, version, license, url}` rows for every
// entry — sorted alphabetically for stable rendering. The parser is
// lifted verbatim from Grexa's settings.rs so the two apps stay in
// sync on cargo-about output format changes.

#[derive(Debug, Eq, PartialEq)]
struct ThirdPartyCredit {
    name: String,
    version: String,
    license: String,
    url: String,
}

fn third_party_credit_entries_json(text: &str) -> String {
    let rows: Vec<_> = third_party_credit_entries(text)
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "name": entry.name,
                "version": entry.version,
                "license": entry.license,
                "url": entry.url,
            })
        })
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_owned())
}

fn third_party_credit_entries(text: &str) -> Vec<ThirdPartyCredit> {
    let mut current_license = String::new();
    let mut in_license_texts = false;
    let mut in_used_by = false;
    let mut entries = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "## License Texts" {
            in_license_texts = true;
            continue;
        }
        if !in_license_texts {
            continue;
        }
        if let Some(title) = trimmed.strip_prefix("### ") {
            current_license = license_section_to_spdx(title);
            in_used_by = false;
            continue;
        }
        if trimmed == "Used by:" {
            in_used_by = !current_license.is_empty();
            continue;
        }
        if !in_used_by {
            continue;
        }
        if trimmed.starts_with("```") || trimmed == "---" {
            in_used_by = false;
            continue;
        }
        if let Some(entry) = parse_used_by_line(trimmed, &current_license) {
            entries.push(entry);
        }
    }

    entries.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.license.cmp(&b.license))
    });
    entries
}

fn parse_used_by_line(line: &str, license: &str) -> Option<ThirdPartyCredit> {
    let body = line.strip_prefix("- [`")?;
    let (label, rest) = body.split_once("`](")?;
    let url = rest.strip_suffix(')')?;
    let (name, version) = label.rsplit_once(' ')?;
    Some(ThirdPartyCredit {
        name: name.to_owned(),
        version: version.to_owned(),
        license: license.to_owned(),
        url: url.to_owned(),
    })
}

fn license_section_to_spdx(title: &str) -> String {
    match title {
        "Apache License 2.0" => "Apache-2.0".to_owned(),
        "BSD 3-Clause &quot;New&quot; or &quot;Revised&quot; License" => "BSD-3-Clause".to_owned(),
        "BSD Zero Clause License" => "0BSD".to_owned(),
        "Community Data License Agreement Permissive 2.0" => "CDLA-Permissive-2.0".to_owned(),
        "GNU General Public License v3.0 only" => "GPL-3.0-only".to_owned(),
        "ISC License" => "ISC".to_owned(),
        "MIT License" => "MIT".to_owned(),
        "Unicode License v3" => "Unicode-3.0".to_owned(),
        "zlib License" => "Zlib".to_owned(),
        other => other
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">"),
    }
}

/// Secondary title line: the server-resolved device label (nickname ||
/// model || disambiguated string, Phase F4). `None`/empty for sensors with
/// no associated hardware device (CPU, memory, …), which render a single
/// title line. The metric itself lives in `SensorInfo.display_name`, which
/// every device-scoped plugin keeps device-agnostic (e.g. "GPU
/// utilization") so the two lines never duplicate the device identity.
fn device_label_for(info: &SensorInfo) -> Option<String> {
    match &info.device_label {
        Some(d) if !d.is_empty() => Some(d.clone()),
        _ => None,
    }
}

/// Extract the backing physical-disk id from a sensor's `parent:<id>` tag
/// (set by the fs plugin). `None` for sensors with no such tag.
fn parent_device_for(info: &linsight_protocol::SensorInfo) -> Option<String> {
    info.tags.iter().find_map(|t| t.strip_prefix("parent:").map(|s| s.to_owned()))
}

fn serialize_tiles(order: &[String], tiles: &HashMap<String, TileJson>) -> String {
    let ordered: Vec<&TileJson> = order.iter().filter_map(|id| tiles.get(id)).collect();
    serde_json::to_string(&ordered).unwrap_or_else(|_| "[]".to_string())
}

fn format_reading_with_rows(
    _sensor: &SensorId,
    r: &Reading,
    unit: &Unit,
    is_static: bool,
) -> (String, Vec<Vec<serde_json::Value>>) {
    // Static byte capacities (total VRAM / RAM) read better as a rounded
    // whole-GB figure ("32 GB") than a precise binary size ("31.84 GiB").
    let bytes_capacity = is_static && matches!(unit, Unit::Bytes);
    match r {
        Reading::Scalar(v) => {
            let s =
                if bytes_capacity { format_bytes_capacity(*v) } else { format_scalar(*v, unit) };
            (s, vec![])
        }
        Reading::Counter(v) => {
            let s = if bytes_capacity {
                format_bytes_capacity(*v as f64)
            } else {
                format_counter(*v, unit)
            };
            (s, vec![])
        }
        Reading::State(s) => (s.clone(), vec![]),
        Reading::Table(rows) => {
            let json_rows: Vec<Vec<serde_json::Value>> = rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .map(|cell| serde_json::to_value(cell).unwrap_or(serde_json::Value::Null))
                        .collect()
                })
                .collect();
            (format!("<{} rows>", json_rows.len()), json_rows)
        }
    }
}

fn format_scalar(v: f64, unit: &Unit) -> String {
    match unit {
        Unit::Percent => format!("{v:.1}%"),
        Unit::Celsius => format!("{v:.1}°C"),
        Unit::Bytes => format_bytes(v),
        Unit::BytesPerSec => format!("{} B/s", v as i64),
        Unit::Hertz => format_hertz(v),
        Unit::Watts => format!("{v:.1} W"),
        Unit::Volts => format!("{v:.3} V"),
        Unit::Rpm => format!("{v:.0} rpm"),
        Unit::Count => format!("{v}"),
        Unit::Custom(s) => format!("{v} {s}"),
    }
}

fn format_counter(v: u64, unit: &Unit) -> String {
    match unit {
        Unit::Bytes => format_bytes(v as f64),
        _ => format!("{v}"),
    }
}

/// Format a constant byte capacity (total VRAM / RAM) as a rounded
/// whole-unit figure, e.g. 34_190_917_632 → "32 GB". Rounds the binary
/// size to the nearest integer so a 32 GiB-class card reads "32 GB"
/// (matching how the hardware is marketed) rather than "31.84 GiB".
/// Sub-GiB capacities fall back to the precise binary formatter.
fn format_bytes_capacity(v: f64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let x = v.abs();
    if x >= TIB {
        format!("{} TB", (v / TIB).round() as i64)
    } else if x >= GIB {
        format!("{} GB", (v / GIB).round() as i64)
    } else {
        format_bytes(v)
    }
}

fn format_bytes(v: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    match v.abs() {
        x if x >= TB => format!("{:.2} TiB", v / TB),
        x if x >= GB => format!("{:.2} GiB", v / GB),
        x if x >= MB => format!("{:.2} MiB", v / MB),
        x if x >= KB => format!("{:.2} KiB", v / KB),
        _ => format!("{v} B"),
    }
}

fn format_hertz(v: f64) -> String {
    const KHZ: f64 = 1_000.0;
    const MHZ: f64 = 1_000_000.0;
    const GHZ: f64 = 1_000_000_000.0;
    match v.abs() {
        x if x >= GHZ => format!("{:.2} GHz", v / GHZ),
        x if x >= MHZ => format!("{:.0} MHz", v / MHZ),
        x if x >= KHZ => format!("{:.0} kHz", v / KHZ),
        _ => format!("{v:.0} Hz"),
    }
}

fn serialize_category(c: linsight_core::Category) -> String {
    match c {
        linsight_core::Category::Cpu => "cpu".into(),
        linsight_core::Category::Gpu => "gpu".into(),
        linsight_core::Category::Memory => "memory".into(),
        linsight_core::Category::Storage => "storage".into(),
        linsight_core::Category::Network => "network".into(),
        linsight_core::Category::Custom => "custom".into(),
    }
}

/// Resolve `~/.config/linsight/dashboard.json` without pulling in `dirs`.
/// Honors `XDG_CONFIG_HOME` per the spec; falls back to `$HOME/.config`.
fn layout_path_buf() -> Result<std::path::PathBuf, String> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(s) if !s.is_empty() => std::path::PathBuf::from(s),
        _ => {
            let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
            std::path::PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("linsight").join("dashboard.json"))
}

fn serialize_kind(k: linsight_core::SensorKind) -> String {
    match k {
        linsight_core::SensorKind::Scalar => "scalar".into(),
        linsight_core::SensorKind::Counter => "counter".into(),
        linsight_core::SensorKind::Table => "table".into(),
        linsight_core::SensorKind::State => "state".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::{Category, SensorKind};
    use std::sync::mpsc;

    fn empty_state_with_tiles(ids: &[&str]) -> SampleState {
        let tiles: HashMap<String, TileJson> = ids
            .iter()
            .map(|id| {
                (
                    (*id).to_string(),
                    TileJson {
                        id: (*id).to_string(),
                        category: "custom".into(),
                        device: None,
                        name: (*id).to_string(),
                        device_label: None,
                        parent_device: None,
                        value: String::new(),
                        kind: "scalar".into(),
                        unit: "count".into(),
                        sparkline: vec![],
                        rows: vec![],
                    },
                )
            })
            .collect();
        let id_order: Vec<String> = ids.iter().map(|s| (*s).to_string()).collect();
        let mut units: HashMap<String, Unit> = HashMap::new();
        for id in ids {
            units.insert((*id).to_string(), Unit::Count);
        }
        SampleState {
            tiles,
            id_order,
            units,
            static_ids: HashSet::new(),
            sparklines: HashMap::new(),
        }
    }

    fn scalar_sample(id: &str, value: f64) -> linsight_core::Sample {
        linsight_core::Sample {
            sensor: SensorId::new(id),
            ts_micros: 0,
            reading: linsight_core::Reading::Scalar(value),
        }
    }

    #[test]
    fn tile_json_carries_device_label_as_separate_line_not_concatenated() {
        // Regression: a hardware nickname used to be *appended* to the
        // metric ("GPU utilization (NVIDIA …) · RTX 5080 Max-Q"). It must
        // instead ride on its own `deviceLabel` field so the GUI can render
        // it as a second title line, and the metric line must stay
        // device-agnostic.
        let gpu = TileJson {
            id: "nvml.gpu0.util".into(),
            category: "gpu".into(),
            device: Some("gpu0".into()),
            name: "GPU utilization".into(),
            device_label: Some("RTX 5080 Max-Q".into()),
            parent_device: None,
            value: "…".into(),
            kind: "scalar".into(),
            unit: String::new(),
            sparkline: vec![],
            rows: vec![],
        };
        let json = serde_json::to_string(&gpu).unwrap();
        assert!(json.contains(r#""name":"GPU utilization""#), "{json}");
        assert!(json.contains(r#""deviceLabel":"RTX 5080 Max-Q""#), "{json}");
        // The device identity must never be glued back into the metric line.
        assert!(!json.contains("RTX 5080 Max-Q utilization"), "{json}");
        assert!(!json.contains("NVIDIA"), "{json}");

        // A sensor with no device (CPU/memory) emits no second line.
        let cpu = TileJson { name: "CPU".into(), device_label: None, ..gpu };
        let cpu_json = serde_json::to_string(&cpu).unwrap();
        assert!(!cpu_json.contains("deviceLabel"), "{cpu_json}");
    }

    #[test]
    fn parent_device_extracted_from_parent_tag() {
        let info = linsight_protocol::SensorInfo {
            id: SensorId::new("fs.home.used_bytes"),
            display_name: "Filesystem used".into(),
            unit: Unit::Bytes,
            kind: SensorKind::Scalar,
            category: Category::Storage,
            native_rate_hz: 1.0,
            min: Some(0.0),
            max: None,
            device_id: Some("home".into()),
            plugin_id: "com.visorcraft.linsight.fs".into(),
            device_key: Some("fs:home".into()),
            device_label: Some("btrfs (/home)".into()),
            tags: vec!["parent:nvme0".into()],
        };
        assert_eq!(super::parent_device_for(&info), Some("nvme0".to_owned()));

        let mut no_parent = info.clone();
        no_parent.tags = vec!["static".into()];
        assert_eq!(super::parent_device_for(&no_parent), None);
    }

    #[test]
    fn device_label_for_blanks_become_none() {
        // Guard the helper that feeds `TileJson.device_label`: an empty
        // server-resolved label collapses to None (→ no second line),
        // matching the serialization test above.
        let mut info = SensorInfo {
            id: SensorId::new("nvml.gpu0.util"),
            display_name: "GPU utilization".into(),
            unit: Unit::Percent,
            kind: linsight_core::SensorKind::Scalar,
            category: linsight_core::Category::Gpu,
            native_rate_hz: 2.0,
            min: Some(0.0),
            max: Some(100.0),
            device_id: Some("gpu0".into()),
            plugin_id: "nvml".into(),
            device_key: Some("nvml:uuid:abc".into()),
            device_label: Some("RTX 5080 Max-Q".into()),
            tags: vec![],
        };
        assert_eq!(device_label_for(&info), Some("RTX 5080 Max-Q".to_string()));
        info.device_label = Some(String::new());
        assert_eq!(device_label_for(&info), None);
        info.device_label = None;
        assert_eq!(device_label_for(&info), None);
    }

    #[test]
    fn static_byte_capacity_rounds_to_whole_gb() {
        // RTX 5090 reports 34_190_917_632 bytes (31.84 GiB) → "32 GB".
        assert_eq!(format_bytes_capacity(34_190_917_632.0), "32 GB");
        // RTX 5080 reports 17_094_934_528 bytes (15.92 GiB) → "16 GB".
        assert_eq!(format_bytes_capacity(17_094_934_528.0), "16 GB");
        assert_eq!(format_bytes_capacity(64.0 * 1024.0 * 1024.0 * 1024.0), "64 GB");
        // Sub-GiB capacities keep precise binary formatting.
        assert_eq!(format_bytes_capacity(512.0 * 1024.0 * 1024.0), "512.00 MiB");
    }

    #[test]
    fn capacity_rounding_only_applies_to_static_byte_sensors() {
        let id = SensorId::new("nvml.gpu0.mem_total_bytes");
        // Static + Bytes → rounded whole GB.
        let (s, _) =
            format_reading_with_rows(&id, &Reading::Scalar(34_190_917_632.0), &Unit::Bytes, true);
        assert_eq!(s, "32 GB");
        // Same value, non-static → precise binary size (e.g. "memory used").
        let (s2, _) =
            format_reading_with_rows(&id, &Reading::Scalar(34_190_917_632.0), &Unit::Bytes, false);
        assert_eq!(s2, "31.84 GiB");
    }

    #[test]
    fn apply_sample_updates_tile_value_and_sparkline() {
        // Sanity check on the per-sample helper used by the pump loop.
        // Establishes a baseline before the batch-drain test below.
        let mut state = empty_state_with_tiles(&["sensor.a"]);
        apply_sample(&mut state, &scalar_sample("sensor.a", 42.0));
        assert_eq!(state.sparklines.get("sensor.a").map(|v| v.as_slice()), Some(&[42.0_f64][..]));
        assert!(!state.tiles.get("sensor.a").unwrap().value.is_empty());
    }

    #[test]
    fn drain_into_state_consumes_all_pending_samples_in_one_call() {
        // Regression guard for the batch-drain optimization in the pump
        // thread. Before this refactor the pump emitted one full
        // tiles_json re-serialize per sample; on a system with ~230
        // sensors at the default 150 ms tick that was ~1.5k emits/sec.
        // The optimization rests on `drain_into_state` greedily pulling
        // every sample already queued and updating state for each, so
        // the caller does the expensive emit once per batch instead of
        // once per sample.
        let mut state = empty_state_with_tiles(&["s.a", "s.b", "s.c", "s.d", "s.e"]);
        let (tx, rx) = mpsc::channel();
        tx.send(scalar_sample("s.a", 1.0)).unwrap();
        tx.send(scalar_sample("s.b", 2.0)).unwrap();
        tx.send(scalar_sample("s.c", 3.0)).unwrap();
        tx.send(scalar_sample("s.d", 4.0)).unwrap();
        tx.send(scalar_sample("s.e", 5.0)).unwrap();
        // Mirror the production pump-loop: one blocking recv for the
        // first sample, then drain_into_state takes over.
        let first = rx.recv().unwrap();
        let count = drain_into_state(&rx, &mut state, first);
        assert_eq!(count, 5, "all five queued samples must be drained in one call");
        for id in ["s.a", "s.b", "s.c", "s.d", "s.e"] {
            assert_eq!(
                state.sparklines.get(id).map(Vec::len),
                Some(1),
                "sparkline buffer for {id} must contain the one sample"
            );
        }
    }

    #[test]
    fn drain_into_state_returns_one_when_channel_empties_immediately() {
        // Single-sample case: drain returns 1 (only the initial sample
        // was processed). Confirms try_recv's Empty error doesn't get
        // treated as a real sample.
        let mut state = empty_state_with_tiles(&["s.only"]);
        let (_tx, rx) = mpsc::channel::<linsight_core::Sample>();
        let count = drain_into_state(&rx, &mut state, scalar_sample("s.only", 7.0));
        assert_eq!(count, 1);
        assert_eq!(state.sparklines.get("s.only").map(Vec::len), Some(1));
    }
}
