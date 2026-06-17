// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use linsight_core::{Category, Sample, SensorId, SensorKind, Unit};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Wire-format stability note:
//
// postcard serializes enum variants by their declaration-order
// discriminant index, not by a hash or name. That means inserting a new
// variant in the MIDDLE of any of the enums below silently corrupts every
// in-flight or persisted message — the daemon's `Sample` becomes the
// client's `SensorList` because the indexes shift. New variants MUST be
// appended at the END only. If you genuinely need to remove or reorder,
// bump `PROTOCOL_VERSION` and gate the change behind a handshake check.

/// A client → daemon message.
///
/// Variant order is wire-format-stable; see the note above this enum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message after socket connect. The daemon replies with Welcome or
    /// disconnects on protocol mismatch.
    Hello { protocol_version: u32, client_name: String, auth_token: Option<String> },
    /// Request the daemon's full sensor list.
    ListSensors,
    /// Subscribe to a set of sensors. `rate_hz = None` means "use the
    /// sensor's native rate."
    Subscribe { sensors: Vec<SensorId>, rate_hz: Option<f32> },
    /// Stop receiving samples for the given sensors.
    Unsubscribe { sensors: Vec<SensorId> },
    /// Polite shutdown signal so the daemon can release subscriptions
    /// immediately rather than waiting for socket close.
    Goodbye,
    /// NEW in v2: correlated request/response. `req_id` is echoed back in
    /// the matching `ServerMsg::Response` so clients can multiplex.
    Request { req_id: u32, op: RequestOp },
}

/// Body of a v2 client `Request`. See `ResponsePayload` for the matching
/// reply variants and `ProtoError` for failures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RequestOp {
    /// Ask the daemon for its current hardware inventory.
    GetHardware,
    /// Set or clear the nickname for a hardware device. `value = None`
    /// clears the nickname; `Some` sets it after validation.
    SetNickname { device_key: String, value: Option<String> },
    /// Per-client pump-thread tick interval. Lower values reduce
    /// sample latency at the cost of more daemon wakeups; higher
    /// values save CPU at the cost of bursty sample arrival.
    /// Daemon clamps the value to `[PUMP_INTERVAL_MIN_MS,
    /// PUMP_INTERVAL_MAX_MS]` and replies with the actual value
    /// applied (so a client requesting 25 ms gets back 50 ms).
    SetPumpIntervalMs { ms: u32 },
    /// Query historical samples for a sensor within a time window.
    GetHistory { sensor: String, since_micros: u64, until_micros: u64, max_points: Option<u32> },
    /// List all configured alert rules.
    ListAlerts,
    /// Create or update an alert rule.
    UpsertAlert {
        name: String,
        expr: String,
        for_duration: Option<String>,
        cooldown: Option<String>,
        notify: Vec<String>,
        enabled: Option<bool>,
    },
    /// Delete an alert rule by name.
    DeleteAlert { name: String },
    /// Dry-run an expression against current sensor values.
    TestAlertExpr { expr: String },
    /// Fetch the most recent alert fire/clear events. `limit` caps the number
    /// of entries returned; `None` returns all (up to the engine's ring-buffer
    /// capacity). New variant — appended at end per wire-format stability rules.
    ListAlertEvents { limit: Option<u32> },
    /// Query current daemon settings (history, alerts, prom enabled state).
    /// New variant — appended at end per wire-format stability rules.
    GetDaemonSettings,
    /// Toggle daemon subsystems at runtime. `history`, `alerts`, and `prom`
    /// are tristated: `Some(true)` enables, `Some(false)` disables,
    /// `None` leaves unchanged. `prom_bind` may be set to change the
    /// Prometheus bind address (takes effect after daemon restart).
    /// New variant — appended at end per wire-format stability rules.
    SetDaemonSettings {
        history: Option<bool>,
        alerts: Option<bool>,
        prom: Option<bool>,
        prom_bind: Option<String>,
    },
    /// Look up a single sensor's metadata by id. Avoids fetching the
    /// entire catalogue just to validate an id and capture its unit.
    /// New variant — appended at end per wire-format stability rules.
    GetSensorInfo { sensor: String },
}

/// A daemon → client message.
///
/// Variant order is wire-format-stable; see the note above `ClientMsg`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Reply to Hello.
    Welcome { protocol_version: u32, daemon_version: String, plugins: Vec<PluginInfo> },
    /// Reply to ListSensors.
    SensorList(Vec<SensorInfo>),
    /// Pushed continuously while subscribed.
    Sample(linsight_core::Sample),
    /// A sensor has been degraded (e.g., plugin panic).
    SensorDegraded { sensor: SensorId, reason: String },
    /// Daemon is going away (e.g., systemd stop).
    Bye { reason: String },
    /// NEW in v2: reply to a `ClientMsg::Request` carrying the same
    /// `req_id`. `result` is either a `ResponsePayload` or a `ProtoError`.
    Response { req_id: u32, result: Result<ResponsePayload, ProtoError> },
    /// NEW in v2: pushed when the daemon's sensor catalogue changes (e.g.,
    /// a nickname update relabels devices). Clients should refresh their
    /// cached `SensorList` from this broadcast. The daemon keeps the wire
    /// shape as `Vec<SensorInfo>`; the outbound channel uses `Arc` internally
    /// so the same allocation is fanned out to every client until the final
    /// per-socket serialization clone.
    SensorListBroadcast(Vec<SensorInfo>),
}

/// Successful payload for a v2 `ServerMsg::Response`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResponsePayload {
    /// Reply to `RequestOp::GetHardware`. Carries both the devices and a
    /// `device_key -> nickname` map so the GUI can pre-populate the
    /// nickname TextField without a separate round-trip. Keyed by the
    /// raw `HardwareDeviceKey` string to avoid a postcard back-validation
    /// per entry.
    Hardware { devices: Vec<linsight_core::HardwareDevice>, nicknames: HashMap<String, String> },
    /// Reply to `RequestOp::SetNickname`. Echoes the device_key and the
    /// new (or cleared) value so clients can confirm the persisted state.
    NicknameSet { device_key: String, value: Option<String> },
    /// Reply to `RequestOp::SetPumpIntervalMs`. Echoes the value the
    /// daemon actually applied — usually the requested value, but
    /// clamped if the client asked for something outside
    /// `[PUMP_INTERVAL_MIN_MS, PUMP_INTERVAL_MAX_MS]`.
    PumpIntervalSet { ms: u32 },
    /// Reply to `RequestOp::GetHistory`. Carries historical samples for a
    /// sensor within the requested time window.
    History { sensor: String, samples: Vec<Sample> },
    /// Reply to `RequestOp::ListAlerts`. Carries the full list of
    /// configured alert rules.
    AlertList { rules: Vec<AlertRuleJson> },
    /// Reply to `RequestOp::UpsertAlert`. Echoes the name of the rule
    /// that was created or updated.
    AlertUpserted { name: String },
    /// Reply to `RequestOp::DeleteAlert`. Echoes the name of the rule
    /// that was removed.
    AlertDeleted { name: String },
    /// Reply to `RequestOp::TestAlertExpr`. Reports whether the expression
    /// evaluated to true given current sensor values, or an error.
    AlertTestResult { is_true: bool, error: Option<String> },
    /// Reply to `RequestOp::ListAlertEvents`. JSON-encoded array of recent
    /// fire/clear events, newest first. New variant — appended at end per
    /// wire-format stability rules.
    AlertEventList { events_json: String },
    /// Reply to `RequestOp::GetDaemonSettings`. Current subsystem states.
    DaemonSettings {
        history_enabled: bool,
        alerts_enabled: bool,
        prom_enabled: bool,
        prom_bind: Option<String>,
    },
    /// Reply to `RequestOp::SetDaemonSettings`. Echoes the applied state.
    DaemonSettingsSet { history_enabled: bool, alerts_enabled: bool, prom_enabled: bool },
    /// Reply to `RequestOp::GetSensorInfo`. Carries the matching sensor
    /// metadata, or is returned as an `UnknownSensor` error if absent.
    SensorInfo { info: SensorInfo },
}

/// Failure payload for a v2 `ServerMsg::Response`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtoError {
    pub code: ProtoErrorCode,
    pub message: String,
}

/// Coarse error code for `ProtoError`; the human-readable detail lives in
/// `ProtoError::message`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProtoErrorCode {
    UnknownDevice,
    InvalidNickname,
    Io,
    Internal,
    UnknownSensor,
    AlertNotFound,
    AlertParse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub sensor_count: u32,
}

/// Wire-format description of a single alert rule, used in the
/// `AlertList` response payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRuleJson {
    pub name: String,
    pub expr: String,
    pub for_duration: Option<String>,
    #[serde(default)]
    pub cooldown: Option<String>,
    pub notify: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorInfo {
    pub id: SensorId,
    pub display_name: String,
    pub unit: Unit,
    pub kind: SensorKind,
    pub category: Category,
    pub native_rate_hz: f32,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub device_id: Option<String>,
    pub plugin_id: String,
    /// NEW in v2: stable HardwareDeviceKey if resolved. Raw string on the
    /// wire; the daemon and clients validate at their boundaries (using the
    /// typed form here would force back-validation on every postcard decode
    /// for no extra safety).
    pub device_key: Option<String>,
    /// NEW in v2: nickname || model || disambiguated string for display.
    pub device_label: Option<String>,
    /// Arbitrary tags the plugin can attach for grouping / filtering.
    pub tags: Vec<String>,
}

#[derive(Debug, Error, PartialEq)]
pub enum HandshakeError {
    #[error("first message must be Hello")]
    NotHello,
    #[error("protocol version mismatch: client={client} daemon={daemon}")]
    VersionMismatch { client: u32, daemon: u32 },
    #[error("authentication failed")]
    AuthFailed,
}

pub fn verify_hello(msg: &ClientMsg) -> Result<(&str, Option<&str>), HandshakeError> {
    match msg {
        ClientMsg::Hello { protocol_version, client_name, auth_token } => {
            if *protocol_version != crate::PROTOCOL_VERSION {
                Err(HandshakeError::VersionMismatch {
                    client: *protocol_version,
                    daemon: crate::PROTOCOL_VERSION,
                })
            } else {
                Ok((client_name.as_str(), auth_token.as_deref()))
            }
        }
        _ => Err(HandshakeError::NotHello),
    }
}

#[cfg(test)]
mod tests {
    use linsight_core::{Reading, Sample};

    use super::*;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: T) {
        let bytes = postcard::to_allocvec(&v).unwrap();
        let back: T = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn hello_round_trips() {
        round_trip(ClientMsg::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            client_name: "test".into(),
            auth_token: None,
        });
    }

    #[test]
    fn welcome_round_trips() {
        round_trip(ServerMsg::Welcome {
            protocol_version: crate::PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            plugins: vec![],
        });
    }

    #[test]
    fn subscribe_round_trips() {
        round_trip(ClientMsg::Subscribe {
            sensors: vec![SensorId::new("cpu.util"), SensorId::new("mem.used")],
            rate_hz: Some(2.0),
        });
    }

    #[test]
    fn unsubscribe_round_trips() {
        round_trip(ClientMsg::Unsubscribe { sensors: vec![SensorId::new("cpu.util")] });
    }

    #[test]
    fn list_sensors_round_trips() {
        round_trip(ClientMsg::ListSensors);
    }

    #[test]
    fn goodbye_round_trips() {
        round_trip(ClientMsg::Goodbye);
    }

    #[test]
    fn sample_message_round_trips() {
        round_trip(ServerMsg::Sample(Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 1_700_000_000_000_000,
            reading: Reading::Scalar(42.0),
        }));
    }

    #[test]
    fn sensor_list_round_trips() {
        round_trip(ServerMsg::SensorList(vec![SensorInfo {
            id: SensorId::new("cpu.util"),
            display_name: "CPU utilization".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: Some(0.0),
            max: Some(100.0),
            device_id: None,
            plugin_id: "com.visorcraft.linsight.cpu".into(),
            device_key: Some("cpu:0".into()),
            device_label: Some("AMD Ryzen 9 8945HX".into()),
            tags: vec!["cpu".into()],
        }]));
    }

    #[test]
    fn degraded_round_trips() {
        round_trip(ServerMsg::SensorDegraded {
            sensor: SensorId::new("cpu.util"),
            reason: "panic in sample()".into(),
        });
    }

    #[test]
    fn bye_round_trips() {
        round_trip(ServerMsg::Bye { reason: "systemd stop".into() });
    }

    #[test]
    fn handshake_accepts_matching_version() {
        let hello = ClientMsg::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            client_name: "x".into(),
            auth_token: None,
        };
        assert!(verify_hello(&hello).is_ok());
    }

    #[test]
    fn handshake_rejects_mismatched_version() {
        let hello =
            ClientMsg::Hello { protocol_version: 999, client_name: "x".into(), auth_token: None };
        assert!(matches!(
            verify_hello(&hello),
            Err(HandshakeError::VersionMismatch { client: 999, daemon: 3 })
        ));
    }

    #[test]
    fn handshake_rejects_non_hello() {
        let bad = ClientMsg::ListSensors;
        assert!(matches!(verify_hello(&bad), Err(HandshakeError::NotHello)));
    }

    #[test]
    fn request_get_hardware_round_trips() {
        round_trip(ClientMsg::Request { req_id: 42, op: RequestOp::GetHardware });
    }

    #[test]
    fn request_set_nickname_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 7,
            op: RequestOp::SetNickname {
                device_key: "pci:0000:06:00.0".into(),
                value: Some("Battlemage".into()),
            },
        });
        round_trip(ClientMsg::Request {
            req_id: 8,
            op: RequestOp::SetNickname { device_key: "pci:0000:06:00.0".into(), value: None },
        });
    }

    #[test]
    fn request_set_pump_interval_round_trips() {
        round_trip(ClientMsg::Request { req_id: 9, op: RequestOp::SetPumpIntervalMs { ms: 150 } });
        round_trip(ServerMsg::Response {
            req_id: 9,
            result: Ok(ResponsePayload::PumpIntervalSet { ms: 150 }),
        });
    }

    #[test]
    fn response_hardware_round_trips() {
        let mut nicks = HashMap::new();
        nicks.insert("pci:0000:06:00.0".into(), "Battlemage".into());
        round_trip(ServerMsg::Response {
            req_id: 42,
            result: Ok(ResponsePayload::Hardware { devices: vec![], nicknames: nicks }),
        });
        round_trip(ServerMsg::Response {
            req_id: 43,
            result: Ok(ResponsePayload::Hardware { devices: vec![], nicknames: HashMap::new() }),
        });
    }

    #[test]
    fn response_nickname_set_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 7,
            result: Ok(ResponsePayload::NicknameSet {
                device_key: "pci:0000:06:00.0".into(),
                value: Some("Battlemage".into()),
            }),
        });
    }

    #[test]
    fn response_error_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 7,
            result: Err(ProtoError {
                code: ProtoErrorCode::UnknownDevice,
                message: "no such device".into(),
            }),
        });
    }

    #[test]
    fn sensor_list_broadcast_round_trips() {
        round_trip(ServerMsg::SensorListBroadcast(vec![]));
    }

    #[test]
    fn alert_rule_json_round_trips() {
        round_trip(AlertRuleJson {
            name: "high-temp".into(),
            expr: "cpu.temp_c > 85".into(),
            for_duration: Some("30s".into()),
            cooldown: Some("5m".into()),
            notify: vec!["desktop".into(), "exec:notify-send alert".into()],
            enabled: true,
        });
        round_trip(AlertRuleJson {
            name: "minimal".into(),
            expr: "mem.used > 80".into(),
            for_duration: None,
            cooldown: None,
            notify: vec![],
            enabled: true,
        });
        round_trip(AlertRuleJson {
            name: "disabled".into(),
            expr: "cpu.util > 50".into(),
            for_duration: None,
            cooldown: Some("1h".into()),
            notify: vec![],
            enabled: false,
        });
    }

    #[test]
    fn history_payload_round_trips() {
        round_trip(ResponsePayload::History {
            sensor: "cpu.util".into(),
            samples: vec![
                Sample {
                    sensor: SensorId::new("cpu.util"),
                    ts_micros: 1_700_000_000_000_000,
                    reading: Reading::Scalar(42.0),
                },
                Sample {
                    sensor: SensorId::new("cpu.util"),
                    ts_micros: 1_700_000_000_000_001,
                    reading: Reading::Scalar(43.0),
                },
            ],
        });
    }

    #[test]
    fn request_get_history_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 10,
            op: RequestOp::GetHistory {
                sensor: "cpu.util".into(),
                since_micros: 1_700_000_000_000_000,
                until_micros: 1_700_000_000_000_100,
                max_points: Some(1000),
            },
        });
        round_trip(ClientMsg::Request {
            req_id: 11,
            op: RequestOp::GetHistory {
                sensor: "cpu.util".into(),
                since_micros: 1_700_000_000_000_000,
                until_micros: 1_700_000_000_000_100,
                max_points: None,
            },
        });
    }

    #[test]
    fn request_list_alerts_round_trips() {
        round_trip(ClientMsg::Request { req_id: 12, op: RequestOp::ListAlerts });
    }

    #[test]
    fn request_upsert_alert_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 13,
            op: RequestOp::UpsertAlert {
                name: "my-rule".into(),
                expr: "cpu.util > 90".into(),
                for_duration: Some("5m".into()),
                cooldown: Some("10m".into()),
                notify: vec!["desktop".into()],
                enabled: None,
            },
        });
    }

    #[test]
    fn request_delete_alert_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 14,
            op: RequestOp::DeleteAlert { name: "my-rule".into() },
        });
    }

    #[test]
    fn request_test_alert_expr_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 15,
            op: RequestOp::TestAlertExpr { expr: "cpu.util > 50".into() },
        });
    }

    #[test]
    fn request_list_alert_events_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 16,
            op: RequestOp::ListAlertEvents { limit: Some(100) },
        });
        round_trip(ClientMsg::Request {
            req_id: 17,
            op: RequestOp::ListAlertEvents { limit: None },
        });
    }

    #[test]
    fn response_alert_event_list_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 16,
            result: Ok(ResponsePayload::AlertEventList {
                events_json:
                    r#"[{"rule":"high-cpu","ts_micros":1000,"kind":"fired","value":null}]"#.into(),
            }),
        });
        round_trip(ServerMsg::Response {
            req_id: 17,
            result: Ok(ResponsePayload::AlertEventList { events_json: "[]".into() }),
        });
    }

    #[test]
    fn request_get_daemon_settings_round_trips() {
        round_trip(ClientMsg::Request { req_id: 18, op: RequestOp::GetDaemonSettings });
    }

    #[test]
    fn request_set_daemon_settings_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 19,
            op: RequestOp::SetDaemonSettings {
                history: Some(true),
                alerts: Some(false),
                prom: None,
                prom_bind: None,
            },
        });
    }

    #[test]
    fn response_daemon_settings_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 18,
            result: Ok(ResponsePayload::DaemonSettings {
                history_enabled: true,
                alerts_enabled: false,
                prom_enabled: true,
                prom_bind: Some("127.0.0.1:9777".into()),
            }),
        });
    }

    #[test]
    fn response_daemon_settings_set_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 19,
            result: Ok(ResponsePayload::DaemonSettingsSet {
                history_enabled: true,
                alerts_enabled: true,
                prom_enabled: false,
            }),
        });
    }

    #[test]
    fn sample_wire_size_within_budget() {
        // The perf budget guarantees a serialized ServerMsg::Sample is <= 64 B.
        // This assertion is deterministic and runs in the normal test job.
        let sample = Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 1_700_000_000_000_000,
            reading: Reading::Scalar(42.0),
        };
        let bytes = postcard::to_allocvec(&ServerMsg::Sample(sample)).unwrap();
        assert!(
            bytes.len() <= 64,
            "ServerMsg::Sample serialized to {} bytes, budget is 64",
            bytes.len()
        );
    }
}

#[cfg(test)]
mod proptest_tests {
    use linsight_core::{Reading, Sample, SensorId};
    use proptest::prelude::*;

    use super::*;

    #[allow(dead_code)]
    fn sensor_id_strategy() -> impl Strategy<Value = SensorId> {
        "[a-z0-9_][a-z0-9_.]*"
            .prop_filter("non-empty", |s: &String| !s.is_empty())
            .prop_map(SensorId::new)
    }

    #[allow(dead_code)]
    fn sensor_info_strategy() -> impl Strategy<Value = SensorInfo> {
        (
            sensor_id_strategy(),
            "[a-zA-Z0-9_ ]*",
            any::<f32>(),
            any::<Option<f64>>(),
            any::<Option<f64>>(),
            any::<Option<String>>(),
            "[a-z.]*",
            any::<Option<String>>(),
            any::<Option<String>>(),
            proptest::collection::vec("[a-z]*", 0..4),
        )
            .prop_map(
                |(
                    id,
                    display_name,
                    native_rate_hz,
                    min,
                    max,
                    device_id,
                    plugin_id,
                    device_key,
                    device_label,
                    tags,
                )| {
                    SensorInfo {
                        id,
                        display_name,
                        unit: Unit::Percent,
                        kind: SensorKind::Scalar,
                        category: Category::Cpu,
                        native_rate_hz,
                        min,
                        max,
                        device_id,
                        plugin_id,
                        device_key,
                        device_label,
                        tags,
                    }
                },
            )
    }

    #[allow(dead_code)]
    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let bytes = postcard::to_allocvec(v).unwrap();
        let back: T = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, *v);
    }

    proptest! {
        fn hello_round_trips(name in "[a-zA-Z0-9_ -]{1,40}", token in any::<Option<String>>()) {
            round_trip(&ClientMsg::Hello {
                protocol_version: crate::PROTOCOL_VERSION,
                client_name: name,
                auth_token: token,
            });
        }

        fn subscribe_round_trips(ids in proptest::collection::vec(sensor_id_strategy(), 0..8)) {
            round_trip(&ClientMsg::Subscribe { sensors: ids, rate_hz: Some(2.0) });
        }

        fn sample_round_trips(sensor in sensor_id_strategy(), ts in any::<u64>(), value in any::<f64>()) {
            round_trip(&ServerMsg::Sample(Sample {
                sensor,
                ts_micros: ts,
                reading: Reading::Scalar(value),
            }));
        }

        fn sensor_list_round_trips(infos in proptest::collection::vec(sensor_info_strategy(), 0..4)) {
            round_trip(&ServerMsg::SensorList(infos));
        }
    }
}
