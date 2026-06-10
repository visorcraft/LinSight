// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `HistoryModel` — exposes the daemon's `get_history` RPC to QML.
//!
//! Properties: `sensorId`, `rangeMinutes`, `pointsJson`, `isLoading`,
//! `lastError`. Call `reload()` to fetch; when the history subsystem
//! is disabled the daemon error lands verbatim in `lastError`.
//!
//! **Stale points on error:** `pointsJson` is never cleared on failure so the
//! last successful fetch remains visible while an error banner is shown.
//!
//! **Property writes do not auto-fetch:** changing `sensorId` or
//! `rangeMinutes` does *not* trigger a network call. Call `reload()` explicitly
//! after mutating those properties.

use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use linsight_core::Reading;
use serde::Serialize;

use crate::qobjects::rpc_worker::{RequestGenerated, spawn_rpc};
use crate::qobjects::workspace_handle::with_workspace;

const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of points requested from the daemon per history fetch.
const MAX_POINTS: u32 = 500;

/// JSON point shape consumed by QML chart components.
#[derive(Serialize)]
struct Point {
    t: u64,
    v: f64,
}

/// Convert a `Vec<Sample>` to a JSON string of `[{"t":<micros>,"v":<f64>}, ...]`.
/// `Table` and `State` readings are silently skipped.
fn samples_to_points_json(samples: &[linsight_core::Sample]) -> String {
    let points: Vec<Point> = samples
        .iter()
        .filter_map(|s| match s.reading {
            Reading::Scalar(v) => Some(Point { t: s.ts_micros, v }),
            Reading::Counter(c) => Some(Point { t: s.ts_micros, v: c as f64 }),
            Reading::Table(_) | Reading::State(_) => None,
        })
        .collect();
    serde_json::to_string(&points).unwrap_or_else(|_| "[]".into())
}

/// Return `(since_micros, until_micros)` for a rolling window of `range_minutes`
/// ending at `now_micros`. Uses saturating arithmetic — a very large
/// `range_minutes` clamps `since` to 0 rather than wrapping.
fn range_to_window(now_micros: u64, range_minutes: u32) -> (u64, u64) {
    let window_micros = (range_minutes as u64).saturating_mul(60).saturating_mul(1_000_000);
    let since = now_micros.saturating_sub(window_micros);
    (since, now_micros)
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
        #[qproperty(QString, sensor_id)]
        #[qproperty(u32, range_minutes)]
        #[qproperty(QString, points_json)]
        #[qproperty(bool, is_loading)]
        #[qproperty(QString, last_error)]
        type HistoryModel = super::HistoryModelRust;

        /// Fetch history for the configured `sensorId` and `rangeMinutes`.
        /// Sets `isLoading` while in flight; on success updates `pointsJson`,
        /// on failure writes the daemon error verbatim into `lastError`.
        #[qinvokable]
        fn reload(self: Pin<&mut HistoryModel>);
    }

    impl cxx_qt::Threading for HistoryModel {}
}

pub struct HistoryModelRust {
    sensor_id: QString,
    range_minutes: u32,
    points_json: QString,
    is_loading: bool,
    last_error: QString,
    request_generation: u64,
}

impl Default for HistoryModelRust {
    fn default() -> Self {
        Self {
            sensor_id: QString::from(""),
            range_minutes: 60,
            points_json: QString::from("[]"),
            is_loading: false,
            last_error: QString::from(""),
            request_generation: 0,
        }
    }
}

impl RequestGenerated for HistoryModelRust {
    fn request_generation(&self) -> u64 {
        self.request_generation
    }
    fn bump_request_generation(&mut self) -> u64 {
        self.request_generation += 1;
        self.request_generation
    }
}

impl ffi::HistoryModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        let sensor = self.as_mut().rust().sensor_id.to_string();
        // No-op when sensorId is empty — nothing useful to fetch.
        if sensor.is_empty() {
            return;
        }
        let range_minutes = self.as_mut().rust().range_minutes;
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let generation = self.as_mut().rust_mut().bump_request_generation();
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());

        let now_micros =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64;
        let (since_micros, until_micros) = range_to_window(now_micros, range_minutes);

        spawn_rpc(
            qt_thread,
            generation,
            move || {
                client
                    .get_history(&sensor, since_micros, until_micros, Some(MAX_POINTS), RPC_TIMEOUT)
                    .map(|samples| samples_to_points_json(&samples))
                    .map_err(|e| format!("{e}"))
            },
            |mut pin, result| {
                match result {
                    Ok(json) => pin.as_mut().set_points_json(QString::from(json.as_str())),
                    Err(e) => pin.as_mut().set_last_error(QString::from(e.as_str())),
                }
                pin.as_mut().set_is_loading(false);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::{Reading, Sample, SensorId};
    use serde_json::Value;

    fn make_sample(ts: u64, reading: Reading) -> Sample {
        Sample { sensor: SensorId::new("test.sensor"), ts_micros: ts, reading }
    }

    #[test]
    fn samples_to_points_json_maps_scalar_and_counter() {
        let samples = vec![
            make_sample(1_000_000, Reading::Scalar(1.5)),
            make_sample(2_000_000, Reading::Counter(42)),
            make_sample(3_000_000, Reading::Table(vec![])),
            make_sample(4_000_000, Reading::State("idle".into())),
        ];
        let json = samples_to_points_json(&samples);
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 2, "Table and State should be skipped");
        assert_eq!(parsed[0]["t"], 1_000_000u64);
        assert!((parsed[0]["v"].as_f64().unwrap() - 1.5).abs() < 1e-10);
        assert_eq!(parsed[1]["t"], 2_000_000u64);
        assert_eq!(parsed[1]["v"].as_f64().unwrap(), 42.0);
    }

    #[test]
    fn samples_to_points_json_empty_returns_empty_array() {
        let json = samples_to_points_json(&[]);
        assert_eq!(json, "[]");
    }

    #[test]
    fn samples_to_points_json_skips_table_and_state() {
        let samples = vec![
            make_sample(1_000, Reading::Table(vec![])),
            make_sample(2_000, Reading::State("P0".into())),
        ];
        let json = samples_to_points_json(&samples);
        assert_eq!(json, "[]");
    }

    #[test]
    fn range_to_window_computes_since_until() {
        let now = 3_600_000_000u64; // 1 hour in micros
        let (since, until) = range_to_window(now, 60);
        assert_eq!(until, now);
        assert_eq!(since, 0, "exactly 60 minutes before 1-hour epoch offset");
    }

    #[test]
    fn range_to_window_normal_case() {
        let now = 1_000_000_000_000u64; // some large time
        let (since, until) = range_to_window(now, 60);
        assert_eq!(until, now);
        assert_eq!(since, now - 3_600_000_000);
    }

    #[test]
    fn range_to_window_saturates_at_zero() {
        // range larger than now → since clamps to 0
        let now = 1_000u64;
        let (since, until) = range_to_window(now, 60);
        assert_eq!(until, now);
        assert_eq!(since, 0);
    }

    #[test]
    fn range_to_window_zero_minutes() {
        let now = 5_000_000u64;
        let (since, until) = range_to_window(now, 0);
        assert_eq!(since, now);
        assert_eq!(until, now);
    }
}
