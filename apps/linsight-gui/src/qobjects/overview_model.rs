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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
        #[qproperty(QString, network_json)]
        #[qproperty(QString, processes_json)]
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

        /// Enable or disable the proc.list sample stream. The process
        /// page calls this on activation / deactivation so the 5-second
        /// /proc sweep only runs while the page is visible.
        #[qinvokable]
        fn set_process_stream_enabled(self: Pin<&mut OverviewModel>, enabled: bool);

        /// Fetch daemon subsystem states (history, alerts, prom) via RPC.
        /// Result is a JSON string: {"history":true,"alerts":false,"prom":true,"promBind":"127.0.0.1:9777"}
        #[qinvokable]
        fn fetch_daemon_settings(self: Pin<&mut OverviewModel>) -> QString;

        /// Toggle a daemon subsystem. `subsystem` is "history", "alerts", or "prom".
        /// `enabled` is the desired state. Returns a JSON status string.
        #[qinvokable]
        fn set_daemon_setting(
            self: Pin<&mut OverviewModel>,
            subsystem: &QString,
            enabled: bool,
        ) -> QString;
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
    #[serde(skip)]
    last_raw_value: u64,
    #[serde(skip)]
    last_ts_micros: u64,
}

#[derive(Clone, Debug, Serialize, Default)]
struct NetworkInterfaceJson {
    iface: String,
    rx_bytes_per_sec: f64,
    tx_bytes_per_sec: f64,
    rx_packets_per_sec: f64,
    tx_packets_per_sec: f64,
    rx_errors_per_sec: f64,
    tx_errors_per_sec: f64,
    rx_dropped_per_sec: f64,
    tx_dropped_per_sec: f64,
    link_state: String,
    speed_mbps: f64,
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
    /// Previous raw counter sample per net sensor, keyed by sensor id.
    net_prev: HashMap<String, (u64, u64)>, // (ts_micros, value)
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
        tile.last_ts_micros = sample.ts_micros;
        if let linsight_core::Reading::Counter(v) = &sample.reading {
            tile.last_raw_value = *v;
        }
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
    rx: &std::sync::mpsc::Receiver<(u64, linsight_core::Sample)>,
    state: &mut SampleState,
    first: linsight_core::Sample,
    current_g: &mut u64,
) -> usize {
    apply_sample(state, &first);
    let mut count = 1;
    while let Ok((g, more)) = rx.try_recv() {
        if g < *current_g {
            continue;
        }
        if g > *current_g {
            *current_g = g;
        }
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
    /// JSON array of per-interface network throughput objects. Empty "[]"
    /// until the first net counter samples have been processed.
    network_json: QString,
    /// JSON array of process objects from proc.list. Empty "[]" until
    /// the process page is opened and the first sample arrives.
    processes_json: QString,
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
            network_json: QString::from("[]"),
            processes_json: QString::from("[]"),
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
        let ws = with_workspace(|ws| ws);

        let sensor_infos: Vec<SensorInfo> = ws.sensor_infos();
        if sensor_infos.is_empty() {
            tracing::warn!("daemon advertised zero sensors");
            return;
        }

        let initial_state = build_sample_state(&sensor_infos);

        // Initial render so the catalogue is visible before the first
        // sample arrives. Setter is called on the GUI thread here, so its
        // changed signal does fire correctly.
        let initial = serialize_tiles(&initial_state.id_order, &initial_state.tiles);
        let init_q = QString::from(initial.as_str());
        self.as_mut().set_tiles_json(init_q);

        // Subscribe to everything except proc.list; the process page will
        // opt in via set_process_stream_enabled so the 5-second /proc
        // sweep only runs while the page is visible.
        let auto_subs: Vec<SensorId> = sensor_infos
            .iter()
            .map(|s| s.id.clone())
            .filter(|id| id.as_str() != "proc.list")
            .collect();
        if let Err(e) = ws.subscribe(auto_subs) {
            tracing::error!(error = ?e, "subscribe failed");
            return;
        }

        // Push the user's persisted sample-interval choice to the daemon.
        // Each client has its own pump-tick, so this only affects the
        // current LinSight process. Failure here isn't fatal — the
        // daemon falls back to its compiled-in default
        // (`PUMP_INTERVAL_DEFAULT_MS`).
        let persisted_ms = crate::qobjects::preferences_model::load_prefs().sample_interval_ms;
        if let Err(e) = ws.set_pump_interval_ms(persisted_ms, std::time::Duration::from_secs(5)) {
            tracing::warn!(error = %e, ms = persisted_ms,
                "set_pump_interval_ms at handshake failed; daemon will use its default tick");
        }

        let Some(rx) = ws.take_sample_rx() else {
            tracing::warn!("sample receiver already taken; live updates will not appear");
            return;
        };
        let Some(catalogue_rx) = ws.take_catalogue_rx() else {
            tracing::warn!("catalogue receiver already taken");
            return;
        };
        let connection_alive = ws.connection_alive();
        let connection_generation = ws.connection_generation();
        let ws_for_pump = Arc::clone(&ws);

        // Subscribe succeeded and the rx is live; flip the UI to
        // "connected" so any disconnected banner stays hidden.
        self.as_mut().set_connected(true);

        // The sample-pump and the catalogue-refresh threads both mutate
        // the same `tiles` map (sample writes `value`, broadcast writes
        // `name`), so the state lives behind an `Arc<Mutex<...>>`. The
        // critical sections are tiny — a HashMap lookup or a per-info
        // rename loop — so contention with the 1 Hz sample rate is
        // negligible.
        let state = Arc::new(Mutex::new(initial_state));

        // Catalogue-refresh worker: when the daemon sends a
        // `SensorListBroadcast` (e.g. after a nickname change), rebuild
        // each tile's `name` from the fresh `SensorInfo` and push a new
        // tiles_json so QML re-renders titles in place. Spawned BEFORE
        // the sample pump so we don't miss any broadcast that races
        // with the first sample. The receiver is a stable bridge owned
        // by the Workspace, so it survives local↔remote reconnects.
        // Broadcasts are tagged with the connection generation so stale
        // updates from a replaced client are ignored.
        {
            let state = Arc::clone(&state);
            let qt_thread = qt_thread.clone();
            let generation = Arc::clone(&connection_generation);
            thread::spawn(move || {
                while let Ok((g, fresh)) = catalogue_rx.recv() {
                    if g < generation.load(Ordering::Relaxed) {
                        continue;
                    }
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
        //
        // The receiver is a stable Workspace bridge, so it does *not* close
        // when the underlying client is replaced. Instead we poll the
        // Workspace's connection-alive flag to surface disconnect/reconnect
        // state to QML.
        {
            let state = Arc::clone(&state);
            thread::spawn(move || {
                let mut last_alive = true;
                let mut current_g = connection_generation.load(Ordering::Relaxed);
                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok((g, first)) => {
                            if g < current_g {
                                continue;
                            }
                            if g > current_g {
                                // The Workspace reconnected to a different
                                // daemon. Rebuild the tile catalogue from the
                                // new host's sensor list and auto-subscribe to
                                // its sensors so pages repopulate correctly.
                                current_g = g;
                                let sensor_infos = ws_for_pump.sensor_infos();
                                if sensor_infos.is_empty() {
                                    tracing::warn!(
                                        "new daemon advertised zero sensors after reconnect"
                                    );
                                }
                                let auto_subs: Vec<SensorId> = sensor_infos
                                    .iter()
                                    .map(|s| s.id.clone())
                                    .filter(|id| id.as_str() != "proc.list")
                                    .collect();
                                if let Err(e) = ws_for_pump.subscribe(auto_subs) {
                                    tracing::warn!(
                                        error = ?e,
                                        "auto-subscribe after reconnect failed"
                                    );
                                }
                                let json = {
                                    let mut guard = state.lock().expect("tile state poisoned");
                                    *guard = build_sample_state(&sensor_infos);
                                    serialize_tiles(&guard.id_order, &guard.tiles)
                                };
                                let qjson = QString::from(json.as_str());
                                let _ = qt_thread.queue(move |mut pin| {
                                    pin.as_mut().set_tiles_json(qjson);
                                    pin.as_mut().set_network_json(QString::from("[]"));
                                    pin.as_mut().set_cpu_text(QString::from("…"));
                                    pin.as_mut().set_mem_text(QString::from("…"));
                                    pin.as_mut().set_cpu_temp_text(QString::from("…"));
                                    pin.as_mut().set_cpu_freq_text(QString::from("…"));
                                    pin.as_mut().set_processes_json(QString::from("[]"));
                                });
                            }
                            if !last_alive {
                                last_alive = true;
                                let _ = qt_thread.queue(|mut pin| {
                                    pin.as_mut().set_connected(true);
                                });
                            }
                            let (json, network_json, cpu, mem, cpu_temp, cpu_freq, proc_json) = {
                                let mut guard = state.lock().expect("tile state poisoned");
                                // Drain the first sample plus any others already
                                // queued (typically the rest of this pump tick's
                                // batch) into the in-memory state. Non-blocking;
                                // returns once the channel is empty.
                                drain_into_state(&rx, &mut guard, first, &mut current_g);
                                // Now snapshot sparklines and re-apply to tiles. Only
                                // need to do this once per batch since all samples
                                // share the same destination state.
                                let sparklines_snapshot: HashMap<String, Vec<f64>> =
                                    guard.sparklines.clone();
                                for tile in guard.tiles.values_mut() {
                                    tile.sparkline = sparklines_snapshot
                                        .get(&tile.id)
                                        .cloned()
                                        .unwrap_or_default();
                                }
                                let json = serialize_tiles(&guard.id_order, &guard.tiles);
                                let network_json = compute_network_rates(&mut guard);
                                let cpu = guard.tiles.get("cpu.util").map(|t| t.value.clone());
                                let mem =
                                    guard.tiles.get("mem.used_bytes").map(|t| t.value.clone());
                                let cpu_temp =
                                    guard.tiles.get("cpu.temp_c").map(|t| t.value.clone());
                                let cpu_freq =
                                    guard.tiles.get("cpu.freq_hz").map(|t| t.value.clone());
                                let proc_json = guard
                                    .tiles
                                    .get("proc.list")
                                    .map(|t| serialize_proc_rows(&t.rows))
                                    .unwrap_or_else(|| "[]".to_string());
                                (json, network_json, cpu, mem, cpu_temp, cpu_freq, proc_json)
                            };
                            let qjson = QString::from(json.as_str());
                            let qnetwork = QString::from(network_json.as_str());
                            let qcpu = cpu.map(|s| QString::from(s.as_str()));
                            let qmem = mem.map(|s| QString::from(s.as_str()));
                            let qctemp = cpu_temp.map(|s| QString::from(s.as_str()));
                            let qcfreq = cpu_freq.map(|s| QString::from(s.as_str()));
                            let qproc = QString::from(proc_json.as_str());
                            // queue() returns Err only when the Qt thread has shut
                            // down (i.e. the app is exiting). Silently discarding is
                            // correct in that case.
                            let _ = qt_thread.queue(move |mut pin| {
                                pin.as_mut().set_tiles_json(qjson);
                                pin.as_mut().set_network_json(qnetwork);
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
                                pin.as_mut().set_processes_json(qproc);
                            });
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            let alive_now = connection_alive.load(Ordering::Relaxed);
                            // Do NOT advance current_g to the published
                            // generation here. If a reconnect happened during
                            // the timeout window, the next sample from the new
                            // host must arrive with g > current_g so the pump
                            // rebuilds the tile catalogue from the new daemon's
                            // sensor list.
                            if alive_now != last_alive {
                                last_alive = alive_now;
                                let _ = qt_thread.queue(move |mut pin| {
                                    pin.as_mut().set_connected(alive_now);
                                });
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            // The stable bridge itself closed (app exiting).
                            let _ = qt_thread.queue(|mut pin| {
                                pin.as_mut().set_connected(false);
                            });
                            break;
                        }
                    }
                }
                tracing::warn!("sample stream ended; surfacing disconnected state to QML");
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

    pub fn fetch_daemon_settings(self: Pin<&mut Self>) -> QString {
        let client = with_workspace(|ws| ws.client());
        match client.get_daemon_settings(std::time::Duration::from_secs(5)) {
            Ok((history, alerts, prom, prom_bind)) => {
                let json = serde_json::json!({
                    "history": history,
                    "alerts": alerts,
                    "prom": prom,
                    "promBind": prom_bind,
                });
                QString::from(json.to_string().as_str())
            }
            Err(e) => {
                tracing::warn!(error = ?e, "fetch_daemon_settings failed");
                QString::from("{}")
            }
        }
    }

    pub fn set_daemon_setting(self: Pin<&mut Self>, subsystem: &QString, enabled: bool) -> QString {
        let client = with_workspace(|ws| ws.client());
        let s = subsystem.to_string();
        let (history, alerts, prom) = match s.as_str() {
            "history" => (Some(enabled), None, None),
            "alerts" => (None, Some(enabled), None),
            "prom" => (None, None, Some(enabled)),
            _ => return QString::from(format!("error: unknown subsystem {s}").as_str()),
        };
        match client.set_daemon_settings(history, alerts, prom, std::time::Duration::from_secs(5)) {
            Ok((h, a, p)) => {
                let json = serde_json::json!({"history": h, "alerts": a, "prom": p});
                QString::from(json.to_string().as_str())
            }
            Err(e) => QString::from(format!("error: {e}").as_str()),
        }
    }

    pub fn set_process_stream_enabled(self: Pin<&mut Self>, enabled: bool) {
        let ws = with_workspace(|ws| ws);
        let id = SensorId::new("proc.list");
        if enabled {
            if let Err(e) = ws.subscribe(vec![id]) {
                tracing::warn!(error = ?e, "proc.list subscribe failed");
            }
        } else {
            if let Err(e) = ws.unsubscribe(vec![id]) {
                tracing::warn!(error = ?e, "proc.list unsubscribe failed");
            }
        }
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

/// Build a fresh `SampleState` from a daemon catalogue. Used at first
/// connect and again after a reconnect to a different host, so pages
/// repopulate from the new daemon's sensor set instead of keeping stale
/// tile IDs from the previous host.
fn build_sample_state(sensor_infos: &[SensorInfo]) -> SampleState {
    let mut tiles: HashMap<String, TileJson> = HashMap::new();
    let mut units: HashMap<String, Unit> = HashMap::new();
    let mut id_order: Vec<String> = Vec::new();
    let mut static_ids: HashSet<String> = HashSet::new();
    for info in sensor_infos {
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
                unit: info.unit.symbol().to_string(),
                sparkline: vec![],
                rows: vec![],
                last_raw_value: 0,
                last_ts_micros: 0,
            },
        );
    }
    SampleState {
        tiles,
        id_order,
        units,
        static_ids,
        sparklines: HashMap::new(),
        net_prev: HashMap::new(),
    }
}

/// Compute per-interface network rates from the latest counter samples.
///
/// Uses `state.net_prev` as the previous sample and updates it to the
/// current sample so the next call sees a one-tick delta. Non-counter
/// tiles (`link_state`, `speed_mbps`) are read directly for metadata.
fn compute_network_rates(state: &mut SampleState) -> String {
    const METRICS: &[&str] = &[
        "rx_bytes",
        "tx_bytes",
        "rx_packets",
        "tx_packets",
        "rx_errors",
        "tx_errors",
        "rx_dropped",
        "tx_dropped",
    ];

    let mut by_iface: BTreeMap<String, NetworkInterfaceJson> = BTreeMap::new();
    let ids: Vec<String> =
        state.tiles.keys().filter(|id| id.starts_with("net.")).cloned().collect();

    for id in ids {
        let rest = match id.strip_prefix("net.") {
            Some(r) => r,
            None => continue,
        };
        let (iface, metric) = match rest.rsplit_once('.') {
            Some((iface, metric)) => (iface.to_string(), metric.to_string()),
            None => continue,
        };
        let tile = match state.tiles.get(&id) {
            Some(t) => t,
            None => continue,
        };
        let ts = tile.last_ts_micros;
        let raw = tile.last_raw_value;

        let entry = by_iface
            .entry(iface.clone())
            .or_insert_with(|| NetworkInterfaceJson { iface: iface.clone(), ..Default::default() });

        if METRICS.contains(&metric.as_str()) {
            let rate = if ts == 0 {
                0.0
            } else if let Some((prev_ts, prev_val)) = state.net_prev.get(&id) {
                let delta_val = raw.saturating_sub(*prev_val);
                let delta_us = ts.saturating_sub(*prev_ts);
                if delta_us > 0 {
                    (delta_val as f64) / (delta_us as f64 / 1_000_000.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };
            match metric.as_str() {
                "rx_bytes" => entry.rx_bytes_per_sec = rate,
                "tx_bytes" => entry.tx_bytes_per_sec = rate,
                "rx_packets" => entry.rx_packets_per_sec = rate,
                "tx_packets" => entry.tx_packets_per_sec = rate,
                "rx_errors" => entry.rx_errors_per_sec = rate,
                "tx_errors" => entry.tx_errors_per_sec = rate,
                "rx_dropped" => entry.rx_dropped_per_sec = rate,
                "tx_dropped" => entry.tx_dropped_per_sec = rate,
                _ => {}
            }
            state.net_prev.insert(id, (ts, raw));
        } else if metric == "link_state" {
            entry.link_state = tile.value.clone();
        } else if metric == "speed_mbps" {
            entry.speed_mbps = tile
                .value
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(-1.0);
        }
    }

    // Prune entries for network sensors that have disappeared (interface
    // removed/renamed) so net_prev doesn't grow unboundedly over long runs.
    let active_net_ids: std::collections::HashSet<String> =
        state.tiles.keys().filter(|id| id.starts_with("net.")).cloned().collect();
    state.net_prev.retain(|id, _| active_net_ids.contains(id));

    let ordered: Vec<&NetworkInterfaceJson> = by_iface.values().collect();
    serde_json::to_string(&ordered).unwrap_or_else(|_| "[]".to_string())
}

/// Convert the raw proc.list table rows into a JSON array of objects
/// with named keys. Columns: pid, name, cpu, mem, rss, threads, state.
fn serialize_proc_rows(rows: &[Vec<serde_json::Value>]) -> String {
    const KEYS: &[&str] = &["pid", "name", "cpu", "mem", "rss", "threads", "state"];
    let objects: Vec<serde_json::Map<String, serde_json::Value>> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, key) in KEYS.iter().enumerate() {
                let val = row.get(i).cloned().unwrap_or(serde_json::Value::Null);
                obj.insert(key.to_string(), val);
            }
            obj
        })
        .collect();
    serde_json::to_string(&objects).unwrap_or_else(|_| "[]".to_string())
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
                        last_raw_value: 0,
                        last_ts_micros: 0,
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
            net_prev: HashMap::new(),
        }
    }

    fn scalar_sample(id: &str, value: f64) -> linsight_core::Sample {
        linsight_core::Sample {
            sensor: SensorId::new(id),
            ts_micros: 0,
            reading: linsight_core::Reading::Scalar(value),
        }
    }

    fn counter_sample(id: &str, ts_micros: u64, value: u64) -> linsight_core::Sample {
        linsight_core::Sample {
            sensor: SensorId::new(id),
            ts_micros,
            reading: linsight_core::Reading::Counter(value),
        }
    }

    fn state_sample(id: &str, state: &str) -> linsight_core::Sample {
        linsight_core::Sample {
            sensor: SensorId::new(id),
            ts_micros: 0,
            reading: linsight_core::Reading::State(state.into()),
        }
    }

    fn net_array(state: &mut SampleState) -> Vec<serde_json::Value> {
        let json = compute_network_rates(state);
        serde_json::from_str::<serde_json::Value>(&json).unwrap().as_array().unwrap().clone()
    }

    fn net_entry<'a>(arr: &'a [serde_json::Value], iface: &str) -> &'a serde_json::Value {
        arr.iter().find(|v| v["iface"] == iface).expect("iface in network_json")
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
            last_raw_value: 0,
            last_ts_micros: 0,
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
    fn network_rate_computes_from_counter_delta() {
        // The Network page needs per-interface throughput rates computed
        // from cumulative counters. Apply two rx_bytes samples one second
        // apart and verify the rate equals the delta.
        let mut state = empty_state_with_tiles(&["net.eth0.rx_bytes"]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 0, 1_000_000));
        let _ = compute_network_rates(&mut state);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 2_500_000));
        let json = compute_network_rates(&mut state);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let arr = parsed.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        let iface = &arr[0];
        assert_eq!(iface["iface"], "eth0");
        assert_eq!(iface["rx_bytes_per_sec"], 1_500_000.0);
    }

    #[test]
    fn network_rate_first_sample_populates_prev_without_rate() {
        // No prior entry → rate must be 0.0 and net_prev must capture the
        // sample so the next tick can compute a delta.
        let mut state = empty_state_with_tiles(&["net.eth0.rx_bytes"]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 5_000_000));
        let arr = net_array(&mut state);
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["rx_bytes_per_sec"], 0.0);
        assert_eq!(state.net_prev.get("net.eth0.rx_bytes"), Some(&(1_000_000, 5_000_000)));
    }

    #[test]
    fn network_rate_zero_delta_returns_zero() {
        // A repeated timestamp must not divide by zero.
        let mut state = empty_state_with_tiles(&["net.eth0.rx_bytes"]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 1_000_000));
        let _ = compute_network_rates(&mut state);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 2_000_000));
        let arr = net_array(&mut state);
        assert_eq!(arr[0]["rx_bytes_per_sec"], 0.0);
    }

    #[test]
    fn network_rate_decreasing_counter_saturates_to_zero() {
        // Counter wrap/reset must not produce a negative rate.
        let mut state = empty_state_with_tiles(&["net.eth0.rx_bytes"]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 0, 1_000_000));
        let _ = compute_network_rates(&mut state);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 500_000));
        let arr = net_array(&mut state);
        assert_eq!(arr[0]["rx_bytes_per_sec"], 0.0);
    }

    #[test]
    fn network_rate_aggregates_multiple_interfaces_and_metrics() {
        let mut state = empty_state_with_tiles(&[
            "net.eth0.rx_bytes",
            "net.eth0.tx_bytes",
            "net.eth1.rx_bytes",
        ]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 0, 1_000_000));
        apply_sample(&mut state, &counter_sample("net.eth0.tx_bytes", 0, 2_000_000));
        apply_sample(&mut state, &counter_sample("net.eth1.rx_bytes", 0, 3_000_000));
        let _ = compute_network_rates(&mut state);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 4_000_000));
        apply_sample(&mut state, &counter_sample("net.eth0.tx_bytes", 1_000_000, 8_000_000));
        apply_sample(&mut state, &counter_sample("net.eth1.rx_bytes", 1_000_000, 9_000_000));
        let arr = net_array(&mut state);

        let eth0 = net_entry(&arr, "eth0");
        assert_eq!(eth0["rx_bytes_per_sec"], 3_000_000.0);
        assert_eq!(eth0["tx_bytes_per_sec"], 6_000_000.0);

        let eth1 = net_entry(&arr, "eth1");
        assert_eq!(eth1["rx_bytes_per_sec"], 6_000_000.0);
    }

    #[test]
    fn network_rate_handles_dotted_interface_name() {
        // Interface names can contain dots (e.g. VLAN subinterfaces). The
        // metric is the final segment; everything after "net." before the
        // last dot is the interface name.
        let mut state = empty_state_with_tiles(&["net.eth0.100.rx_bytes"]);
        apply_sample(&mut state, &counter_sample("net.eth0.100.rx_bytes", 0, 1_000_000));
        let _ = compute_network_rates(&mut state);
        apply_sample(&mut state, &counter_sample("net.eth0.100.rx_bytes", 1_000_000, 2_500_000));
        let arr = net_array(&mut state);
        let iface = net_entry(&arr, "eth0.100");
        assert_eq!(iface["rx_bytes_per_sec"], 1_500_000.0);
    }

    #[test]
    fn network_rate_reads_link_state_and_speed() {
        let mut state = empty_state_with_tiles(&[
            "net.eth0.rx_bytes",
            "net.eth0.link_state",
            "net.eth0.speed_mbps",
        ]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 0, 1_000_000));
        apply_sample(&mut state, &state_sample("net.eth0.link_state", "up"));
        apply_sample(&mut state, &scalar_sample("net.eth0.speed_mbps", 1000.0));
        let _ = compute_network_rates(&mut state);

        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 1_000_000, 2_000_000));
        let arr = net_array(&mut state);
        let iface = net_entry(&arr, "eth0");
        assert_eq!(iface["link_state"], "up");
        assert_eq!(iface["speed_mbps"], 1000.0);
        assert_eq!(iface["rx_bytes_per_sec"], 1_000_000.0);
    }

    #[test]
    fn network_rate_prunes_stale_prev_entries() {
        // When a network interface disappears, its net_prev entry must be
        // removed so the map doesn't grow unboundedly over a long session.
        let mut state = empty_state_with_tiles(&["net.eth0.rx_bytes", "net.eth1.rx_bytes"]);
        apply_sample(&mut state, &counter_sample("net.eth0.rx_bytes", 0, 1_000_000));
        apply_sample(&mut state, &counter_sample("net.eth1.rx_bytes", 0, 2_000_000));
        let _ = compute_network_rates(&mut state);
        assert!(state.net_prev.contains_key("net.eth0.rx_bytes"));
        assert!(state.net_prev.contains_key("net.eth1.rx_bytes"));

        // Simulate eth1 disappearing from the sensor catalogue.
        state.tiles.remove("net.eth1.rx_bytes");
        state.id_order.retain(|id| id != "net.eth1.rx_bytes");
        let _ = compute_network_rates(&mut state);

        assert!(state.net_prev.contains_key("net.eth0.rx_bytes"));
        assert!(!state.net_prev.contains_key("net.eth1.rx_bytes"));
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
        let (tx, rx) = mpsc::channel::<(u64, linsight_core::Sample)>();
        tx.send((1, scalar_sample("s.a", 1.0))).unwrap();
        tx.send((1, scalar_sample("s.b", 2.0))).unwrap();
        tx.send((1, scalar_sample("s.c", 3.0))).unwrap();
        tx.send((1, scalar_sample("s.d", 4.0))).unwrap();
        tx.send((1, scalar_sample("s.e", 5.0))).unwrap();
        // Mirror the production pump-loop: one blocking recv for the
        // first sample, then drain_into_state takes over.
        let (_g, first) = rx.recv().unwrap();
        let mut current_g = 1;
        let count = drain_into_state(&rx, &mut state, first, &mut current_g);
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
        let (_tx, rx) = mpsc::channel::<(u64, linsight_core::Sample)>();
        let mut current_g = 1;
        let count = drain_into_state(&rx, &mut state, scalar_sample("s.only", 7.0), &mut current_g);
        assert_eq!(count, 1);
        assert_eq!(state.sparklines.get("s.only").map(Vec::len), Some(1));
    }

    #[test]
    fn drain_into_state_skips_stale_generation_samples() {
        // After a reconnect, buffered samples from the previous connection
        // must not overwrite state for the new connection.
        let mut state = empty_state_with_tiles(&["s.a"]);
        let (tx, rx) = mpsc::channel::<(u64, linsight_core::Sample)>();
        tx.send((2, scalar_sample("s.a", 99.0))).unwrap();
        tx.send((1, scalar_sample("s.a", 1.0))).unwrap();
        tx.send((2, scalar_sample("s.a", 100.0))).unwrap();
        let (_g, first) = rx.recv().unwrap();
        let mut current_g = 2;
        let count = drain_into_state(&rx, &mut state, first, &mut current_g);
        assert_eq!(count, 2);
        assert_eq!(state.tiles["s.a"].value, "100");
    }

    #[test]
    fn proc_table_reading_serializes_to_row_objects() {
        // Reading::Table with 2 TableRows → JSON
        // [{"pid":1,"name":"init","cpu":0.0,"mem":0.25,"rss":4194304,"threads":1,"state":"S"}, ...]
        let rows: Vec<Vec<serde_json::Value>> = vec![
            vec![
                serde_json::json!(1.0),
                serde_json::json!("init"),
                serde_json::json!(0.0),
                serde_json::json!(0.25),
                serde_json::json!(4194304),
                serde_json::json!(1.0),
                serde_json::json!("S"),
            ],
            vec![
                serde_json::json!(42.0),
                serde_json::json!("firefox"),
                serde_json::json!(12.5),
                serde_json::json!(8.0),
                serde_json::json!(536870912),
                serde_json::json!(8.0),
                serde_json::json!("R"),
            ],
        ];
        let json = serialize_proc_rows(&rows);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let arr = parsed.as_array().expect("array");
        assert_eq!(arr.len(), 2);

        let first = &arr[0];
        assert_eq!(first["pid"], 1.0);
        assert_eq!(first["name"], "init");
        assert_eq!(first["cpu"], 0.0);
        assert_eq!(first["mem"], 0.25);
        assert_eq!(first["rss"], 4194304);
        assert_eq!(first["threads"], 1.0);
        assert_eq!(first["state"], "S");

        let second = &arr[1];
        assert_eq!(second["pid"], 42.0);
        assert_eq!(second["name"], "firefox");
        assert_eq!(second["cpu"], 12.5);
        assert_eq!(second["mem"], 8.0);
        assert_eq!(second["rss"], 536870912);
        assert_eq!(second["threads"], 8.0);
        assert_eq!(second["state"], "R");
    }

    #[test]
    fn proc_table_empty_rows_returns_empty_array() {
        let json = serialize_proc_rows(&[]);
        assert_eq!(json, "[]");
    }

    #[test]
    fn proc_table_short_row_fills_missing_with_null() {
        let rows: Vec<Vec<serde_json::Value>> =
            vec![vec![serde_json::json!(1.0), serde_json::json!("init")]];
        let json = serialize_proc_rows(&rows);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = parsed.as_array().unwrap()[0].as_object().unwrap();
        assert_eq!(obj["pid"], 1.0);
        assert_eq!(obj["name"], "init");
        assert!(obj["cpu"].is_null());
        assert!(obj["mem"].is_null());
        assert!(obj["rss"].is_null());
        assert!(obj["threads"].is_null());
        assert!(obj["state"].is_null());
    }
}
