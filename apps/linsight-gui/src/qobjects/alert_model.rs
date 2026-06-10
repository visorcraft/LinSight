// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `AlertModel` — wraps the daemon's alert RPCs for the AlertsPage.
//! Holds the rule list as a JSON string so QML can `JSON.parse` it.

use std::pin::Pin;
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use linsight_protocol::AlertRuleJson;

use crate::client::ClientHandle;
use crate::qobjects::rpc_worker::{RequestGenerated, spawn_rpc};
use crate::qobjects::workspace_handle::with_workspace;

const RPC_TIMEOUT: Duration = Duration::from_secs(5);

enum AlertMutation {
    Upsert { name: String, expr: String, notify: Vec<String>, enabled: Option<bool>, cooldown: Option<String> },
    Delete { name: String },
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
        #[qproperty(QString, rules_json)]
        #[qproperty(bool, is_loading)]
        #[qproperty(QString, last_error)]
        #[qproperty(QString, test_result)]
        type AlertModel = super::AlertModelRust;

        /// Re-fetch all alert rules from the daemon.
        #[qinvokable]
        fn reload(self: Pin<&mut AlertModel>);

        /// Upsert (add or update) a rule. `enabled_state` is a tri-state:
        ///   0 = preserve current enabled flag (used by the edit dialog,
        ///       which doesn't surface enable/disable),
        ///   1 = force-enable,
        ///  -1 = force-disable.
        /// Using a bool collapsed "force-enable" into the preserve case,
        /// so toggling a disabled rule's Switch back ON silently no-op'd.
        #[qinvokable]
        fn upsert(
            self: Pin<&mut AlertModel>,
            name: &QString,
            expr: &QString,
            notify: &QString,
            enabled_state: i32,
            cooldown: &QString,
        );

        /// Delete a rule by name. Named `delete_rule` because `delete`
        /// is a reserved word in C++ and `auto_cxx_name` would emit a
        /// `void delete(...)` method declaration that the C++ compiler
        /// rejects.
        #[qinvokable]
        fn delete_rule(self: Pin<&mut AlertModel>, name: &QString);

        /// Test an expression against current sensor values.
        #[qinvokable]
        fn test_expr(self: Pin<&mut AlertModel>, expr: &QString);
    }

    impl cxx_qt::Threading for AlertModel {}
}

#[derive(Default)]
pub struct AlertModelRust {
    rules_json: QString,
    is_loading: bool,
    last_error: QString,
    test_result: QString,
    request_generation: u64,
}

impl RequestGenerated for AlertModelRust {
    fn request_generation(&self) -> u64 {
        self.request_generation
    }
    fn bump_request_generation(&mut self) -> u64 {
        self.request_generation += 1;
        self.request_generation
    }
}

fn alert_rules_json(rules: &[AlertRuleJson]) -> String {
    serde_json::to_string(rules).unwrap_or_else(|_| "[]".into())
}

fn alert_test_status(is_true: bool, error: Option<String>) -> String {
    match error {
        Some(err) => format!("Error: {err}"),
        None if is_true => "✓ Condition is TRUE with current values".into(),
        None => "✗ Condition is FALSE with current values".into(),
    }
}

fn load_alert_rules_json(client: &ClientHandle) -> Result<String, String> {
    client
        .list_alerts(RPC_TIMEOUT)
        .map(|rules| alert_rules_json(&rules))
        .map_err(|e| format!("{e}"))
}

fn apply_alert_mutation_and_reload(
    client: &ClientHandle,
    mutation: AlertMutation,
) -> Result<String, String> {
    match mutation {
        AlertMutation::Upsert { name, expr, notify, enabled, cooldown } => {
            client.upsert_alert(&name, &expr, None, cooldown, notify, enabled, RPC_TIMEOUT).map(|_| ())
        }
        AlertMutation::Delete { name } => client.delete_alert(&name, RPC_TIMEOUT).map(|_| ()),
    }
    .map_err(|e| format!("{e}"))?;
    load_alert_rules_json(client)
}

impl ffi::AlertModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let generation = self.as_mut().rust_mut().bump_request_generation();
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());
        spawn_rpc(
            qt_thread,
            generation,
            move || load_alert_rules_json(&client),
            |mut pin, result| {
                match result {
                    Ok(json) => pin.as_mut().set_rules_json(QString::from(json.as_str())),
                    Err(e) => pin.as_mut().set_last_error(QString::from(e.as_str())),
                }
                pin.as_mut().set_is_loading(false);
            },
        );
    }

    pub fn upsert(
        mut self: Pin<&mut Self>,
        name: &QString,
        expr: &QString,
        notify: &QString,
        enabled_state: i32,
        cooldown: &QString,
    ) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let n = name.to_string();
        let e = expr.to_string();
        let not: Vec<String> = notify
            .to_string()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let enabled = match enabled_state {
            1 => Some(true),
            -1 => Some(false),
            _ => None,
        };
        let cd = {
            let s = cooldown.to_string();
            if s.trim().is_empty() { None } else { Some(s) }
        };
        let generation = self.as_mut().rust_mut().bump_request_generation();
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());
        spawn_rpc(
            qt_thread,
            generation,
            move || {
                apply_alert_mutation_and_reload(
                    &client,
                    AlertMutation::Upsert { name: n, expr: e, notify: not, enabled, cooldown: cd },
                )
            },
            |mut pin, result| {
                match result {
                    Ok(json) => pin.as_mut().set_rules_json(QString::from(json.as_str())),
                    Err(e) => pin.as_mut().set_last_error(QString::from(e.as_str())),
                }
                pin.as_mut().set_is_loading(false);
            },
        );
    }

    pub fn delete_rule(mut self: Pin<&mut Self>, name: &QString) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let n = name.to_string();
        let generation = self.as_mut().rust_mut().bump_request_generation();
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());
        spawn_rpc(
            qt_thread,
            generation,
            move || apply_alert_mutation_and_reload(&client, AlertMutation::Delete { name: n }),
            |mut pin, result| {
                match result {
                    Ok(json) => pin.as_mut().set_rules_json(QString::from(json.as_str())),
                    Err(e) => pin.as_mut().set_last_error(QString::from(e.as_str())),
                }
                pin.as_mut().set_is_loading(false);
            },
        );
    }

    pub fn test_expr(mut self: Pin<&mut Self>, expr: &QString) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        self.as_mut().set_test_result(QString::from("testing..."));
        let e = expr.to_string();
        let generation = self.as_mut().rust_mut().bump_request_generation();
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());
        spawn_rpc(
            qt_thread,
            generation,
            move || {
                client
                    .test_alert_expr(&e, RPC_TIMEOUT)
                    .map(|(is_true, error)| alert_test_status(is_true, error))
                    .unwrap_or_else(|e| format!("RPC error: {e}"))
            },
            |mut pin, result| {
                pin.as_mut().set_test_result(QString::from(result.as_str()));
                pin.as_mut().set_is_loading(false);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_protocol::AlertRuleJson;
    use serde_json::Value;

    #[test]
    fn alert_rules_json_preserves_wire_shape() {
        let rules = vec![AlertRuleJson {
            name: "high-temp".into(),
            expr: "cpu.temp_c > 85".into(),
            for_duration: Some("30s".into()),
            cooldown: Some("5m".into()),
            notify: vec!["desktop".into(), "exec:notify-send alert".into()],
            enabled: false,
        }];

        let json = alert_rules_json(&rules);
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed[0]["name"], "high-temp");
        assert_eq!(parsed[0]["expr"], "cpu.temp_c > 85");
        assert_eq!(parsed[0]["for_duration"], "30s");
        assert_eq!(parsed[0]["cooldown"], "5m");
        assert_eq!(parsed[0]["notify"][1], "exec:notify-send alert");
        assert!(!parsed[0]["enabled"].as_bool().unwrap());
    }

    #[test]
    fn alert_test_status_messages_match_qml_copy() {
        assert_eq!(alert_test_status(true, None), "✓ Condition is TRUE with current values");
        assert_eq!(alert_test_status(false, None), "✗ Condition is FALSE with current values");
        assert_eq!(
            alert_test_status(false, Some("unknown sensor".into())),
            "Error: unknown sensor"
        );
    }
}
