// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `HardwareModel` — wraps the daemon's `get_hardware` / `set_nickname`
//! RPCs for the Hardware page. Holds the most recent device inventory
//! as a JSON string (so QML can `JSON.parse` it once and bind a
//! `Repeater` to the result without bridging a `QAbstractListModel`).
//!
//! Nickname edits trigger an eager refresh so the page reflects the
//! change immediately. The daemon's `SensorListBroadcast` will also
//! refresh `OverviewModel`'s tile labels via the catalogue listener
//! installed in Phase H — those two paths are independent.

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use linsight_core::HardwareDevice;
use serde::Serialize;

use crate::qobjects::rpc_worker::spawn_rpc;
use crate::qobjects::workspace_handle::with_workspace;

const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// JSON shape sent to QML. `HardwareDevice` is flattened so existing
/// `deviceJson.model` / `deviceJson.key` bindings keep working; the
/// optional `nickname` sibling lets the page pre-populate its TextField
/// from the daemon's stored value, and `label` carries the daemon-
/// agreed display name (nickname || disambiguated model || raw model)
/// so the page title matches what the tiles show.
#[derive(Serialize)]
struct DeviceWithNickname<'a> {
    #[serde(flatten)]
    inner: &'a HardwareDevice,
    #[serde(skip_serializing_if = "Option::is_none")]
    nickname: Option<&'a str>,
    label: String,
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
        #[qproperty(QString, devices_json)]
        #[qproperty(bool, is_loading)]
        #[qproperty(QString, last_error)]
        type HardwareModel = super::HardwareModelRust;

        /// Re-fetch the hardware inventory from the daemon. Sets
        /// `isLoading` while the RPC is in flight; populates either
        /// `devicesJson` (success) or `lastError` (failure).
        #[qinvokable]
        fn reload(self: Pin<&mut HardwareModel>);

        /// Send `set_nickname` for `key`. An empty / whitespace-only
        /// `value` clears the nickname. On success, eagerly refreshes
        /// the inventory so the UI updates without waiting for the
        /// daemon's `SensorListBroadcast` to arrive.
        #[qinvokable]
        fn apply_nickname(self: Pin<&mut HardwareModel>, key: &QString, value: &QString);
    }

    impl cxx_qt::Threading for HardwareModel {}
}

#[derive(Default)]
pub struct HardwareModelRust {
    devices_json: QString,
    is_loading: bool,
    last_error: QString,
    request_generation: u64,
}

fn devices_json(devices: &[HardwareDevice], nicknames: &HashMap<String, String>) -> String {
    let payload: Vec<DeviceWithNickname<'_>> = devices
        .iter()
        .map(|d| DeviceWithNickname {
            inner: d,
            nickname: nicknames.get(d.key.as_str()).map(String::as_str),
            label: linsight_core::compute_device_label(d, devices, nicknames),
        })
        .collect();
    serde_json::to_string(&payload).unwrap_or_else(|_| "[]".into())
}

impl ffi::HardwareModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let generation = {
            let mut rust = self.as_mut().rust_mut();
            rust.request_generation += 1;
            rust.request_generation
        };
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());
        spawn_rpc(
            qt_thread,
            generation,
            move || {
                client
                    .get_hardware(RPC_TIMEOUT)
                    .map(|(devices, nicknames)| devices_json(&devices, &nicknames))
                    .map_err(|e| format!("{e}"))
            },
            |mut pin, req_gen, result| {
                if pin.as_mut().rust().request_generation != req_gen {
                    return;
                }
                match result {
                    Ok(json) => pin.as_mut().set_devices_json(QString::from(json.as_str())),
                    Err(e) => pin.as_mut().set_last_error(QString::from(e.as_str())),
                }
                pin.as_mut().set_is_loading(false);
            },
        );
    }

    pub fn apply_nickname(mut self: Pin<&mut Self>, key: &QString, value: &QString) {
        let key_s = key.to_string();
        let value_s = value.to_string();
        let value_opt = if value_s.trim().is_empty() { None } else { Some(value_s) };
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let generation = {
            let mut rust = self.as_mut().rust_mut();
            rust.request_generation += 1;
            rust.request_generation
        };
        let qt_thread = self.qt_thread();
        let client = with_workspace(|w| w.client());
        spawn_rpc(
            qt_thread,
            generation,
            move || {
                client
                    .set_nickname(&key_s, value_opt, RPC_TIMEOUT)
                    .and_then(|()| client.get_hardware(RPC_TIMEOUT))
                    .map(|(devices, nicknames)| devices_json(&devices, &nicknames))
                    .map_err(|e| format!("{e}"))
            },
            |mut pin, req_gen, result| {
                if pin.as_mut().rust().request_generation != req_gen {
                    return;
                }
                match result {
                    Ok(json) => pin.as_mut().set_devices_json(QString::from(json.as_str())),
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
    use linsight_core::{HardwareCategory, HardwareDeviceKey};
    use serde_json::Value;
    use std::collections::HashMap;

    fn device(key: &str, model: &str, plugin_device_id: &str) -> HardwareDevice {
        HardwareDevice {
            key: HardwareDeviceKey::try_new(key).unwrap(),
            category: HardwareCategory::Gpu,
            model: model.into(),
            vendor: Some("VisorCraft".into()),
            location: Some("slot 1".into()),
            plugin_id: "test".into(),
            plugin_device_id: plugin_device_id.into(),
            sensor_ids: Vec::new(),
        }
    }

    #[test]
    fn devices_json_includes_nickname_and_display_label() {
        let devices = vec![
            device("pci:0000:01:00.0", "Arc GPU", "gpu0"),
            device("pci:0000:02:00.0", "Arc GPU", "gpu1"),
        ];
        let nicknames = HashMap::from([("pci:0000:02:00.0".into(), "Render GPU".into())]);

        let json = devices_json(&devices, &nicknames);
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed[0]["model"], "Arc GPU");
        assert!(parsed[0].get("nickname").is_none());
        assert_eq!(parsed[0]["label"], "Arc GPU");
        assert_eq!(parsed[1]["nickname"], "Render GPU");
        assert_eq!(parsed[1]["label"], "Render GPU");
    }
}
