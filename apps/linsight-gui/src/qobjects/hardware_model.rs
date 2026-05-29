// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `HardwareModel` — wraps the daemon's `get_hardware` / `set_nickname`
//! RPCs for the Hardware page. Holds the most recent device inventory
//! as a JSON string (so QML can `JSON.parse` it once and bind a
//! `Repeater` to the result without bridging a `QAbstractListModel`).
//!
//! Nickname edits trigger an eager `reload()` so the page reflects the
//! change immediately. The daemon's `SensorListBroadcast` will also
//! refresh `OverviewModel`'s tile labels via the catalogue listener
//! installed in Phase H — those two paths are independent.

use std::pin::Pin;
use std::time::Duration;

use cxx_qt_lib::QString;
use linsight_core::HardwareDevice;
use serde::Serialize;

use crate::qobjects::workspace_handle::with_workspace;

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
        /// `value` clears the nickname. On success, eagerly calls
        /// `reload` so the UI updates without waiting for the
        /// daemon's `SensorListBroadcast` to arrive.
        #[qinvokable]
        fn apply_nickname(self: Pin<&mut HardwareModel>, key: &QString, value: &QString);
    }
}

#[derive(Default)]
pub struct HardwareModelRust {
    devices_json: QString,
    is_loading: bool,
    last_error: QString,
}

impl ffi::HardwareModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let client = with_workspace(|w| w.client());
        match client.get_hardware(Duration::from_secs(5)) {
            Ok((devices, nicknames)) => {
                let payload: Vec<DeviceWithNickname<'_>> = devices
                    .iter()
                    .map(|d| DeviceWithNickname {
                        inner: d,
                        nickname: nicknames.get(d.key.as_str()).map(String::as_str),
                        label: linsight_core::compute_device_label(d, &devices, &nicknames),
                    })
                    .collect();
                let json = serde_json::to_string(&payload).unwrap_or_else(|_| "[]".into());
                self.as_mut().set_devices_json(QString::from(json.as_str()));
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
        self.as_mut().set_is_loading(false);
    }

    pub fn apply_nickname(mut self: Pin<&mut Self>, key: &QString, value: &QString) {
        let key_s = key.to_string();
        let value_s = value.to_string();
        let value_opt = if value_s.trim().is_empty() { None } else { Some(value_s) };
        // Flip `isLoading` for the duration of the round-trip so the
        // QML page can dim / disable the TextField. Without this the
        // user has no feedback during the (sub-second on a local
        // socket, multi-second over the mTLS tunnel) RPC.
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let client = with_workspace(|w| w.client());
        let result = client.set_nickname(&key_s, value_opt, Duration::from_secs(5));
        // Always clear isLoading before returning, even on error —
        // otherwise a failed RPC would freeze the field permanently.
        self.as_mut().set_is_loading(false);
        match result {
            Ok(()) => {
                // Eagerly reload for snappy UI feedback. The daemon's
                // SensorListBroadcast will also refresh OverviewModel's
                // tile labels via the catalogue listener Phase H wired.
                // `reload` will set/clear isLoading on its own pass.
                self.as_mut().reload();
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
    }
}
