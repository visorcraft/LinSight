// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `AlertModel` — wraps the daemon's alert RPCs for the AlertsPage.
//! Holds the rule list as a JSON string so QML can `JSON.parse` it.

use std::pin::Pin;
use std::time::Duration;

use cxx_qt_lib::QString;

use crate::qobjects::workspace_handle::with_workspace;

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
}

#[derive(Default)]
pub struct AlertModelRust {
    rules_json: QString,
    is_loading: bool,
    last_error: QString,
    test_result: QString,
}

impl ffi::AlertModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let client = with_workspace(|w| w.client());
        match client.list_alerts(Duration::from_secs(5)) {
            Ok(rules) => {
                let json = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                self.as_mut().set_rules_json(QString::from(json.as_str()));
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
        self.as_mut().set_is_loading(false);
    }

    pub fn upsert(
        mut self: Pin<&mut Self>,
        name: &QString,
        expr: &QString,
        notify: &QString,
        enabled_state: i32,
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
        let client = with_workspace(|w| w.client());
        match client.upsert_alert(&n, &e, None, not, enabled, Duration::from_secs(5)) {
            Ok(_) => {
                // Reload to reflect the change
                drop(client);
                let client2 = with_workspace(|w| w.client());
                match client2.list_alerts(Duration::from_secs(5)) {
                    Ok(rules) => {
                        let json = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                        self.as_mut().set_rules_json(QString::from(json.as_str()));
                    }
                    Err(e) => {
                        self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
                    }
                }
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
        self.as_mut().set_is_loading(false);
    }

    pub fn delete_rule(mut self: Pin<&mut Self>, name: &QString) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let n = name.to_string();
        let client = with_workspace(|w| w.client());
        match client.delete_alert(&n, Duration::from_secs(5)) {
            Ok(_) => {
                drop(client);
                let client2 = with_workspace(|w| w.client());
                match client2.list_alerts(Duration::from_secs(5)) {
                    Ok(rules) => {
                        let json = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                        self.as_mut().set_rules_json(QString::from(json.as_str()));
                    }
                    Err(e) => {
                        self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
                    }
                }
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
        self.as_mut().set_is_loading(false);
    }

    pub fn test_expr(mut self: Pin<&mut Self>, expr: &QString) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_test_result(QString::from("testing..."));
        let e = expr.to_string();
        let client = with_workspace(|w| w.client());
        match client.test_alert_expr(&e, Duration::from_secs(5)) {
            Ok((is_true, error)) => {
                let msg = match error {
                    Some(err) => format!("Error: {err}"),
                    None if is_true => "✓ Condition is TRUE with current values".into(),
                    None => "✗ Condition is FALSE with current values".into(),
                };
                self.as_mut().set_test_result(QString::from(msg.as_str()));
            }
            Err(e) => {
                self.as_mut().set_test_result(QString::from(format!("RPC error: {e}").as_str()));
            }
        }
        self.as_mut().set_is_loading(false);
    }
}
