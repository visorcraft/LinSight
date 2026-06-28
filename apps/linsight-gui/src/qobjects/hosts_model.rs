// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `HostsModel` — persisted saved remote hosts + in-app connection
//! switching. Stores `~/.config/linsight/hosts.json` and exposes the
//! list to QML as a JSON array. Connection attempts run off the Qt
//! thread via the shared `spawn_rpc` helper.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

use crate::qobjects::preferences_model::config_dir_override;
use crate::qobjects::rpc_worker::{RequestGenerated, spawn_rpc};
use crate::qobjects::workspace_handle::with_workspace;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct HostsFile {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default)]
    hosts: Vec<Host>,
}

fn default_schema_version() -> u32 {
    1
}

fn hosts_path() -> Option<PathBuf> {
    config_dir_override().map(|d| d.join("hosts.json"))
}

pub(crate) fn load_hosts() -> Vec<Host> {
    let Some(path) = hosts_path() else { return Vec::new() };
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_str::<HostsFile>(&raw) {
        Ok(doc) => doc.hosts,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(),
                "malformed hosts.json; backing up and using defaults");
            let _ = std::fs::rename(&path, path.with_extension("json.bad"));
            Vec::new()
        }
    }
}

fn save_hosts(hosts: &[Host]) -> std::io::Result<()> {
    let Some(path) = hosts_path() else {
        return Err(std::io::Error::other("no config dir resolvable from HOME / XDG_CONFIG_HOME"));
    };
    let doc = HostsFile { schema_version: 1, hosts: hosts.to_vec() };
    linsight_core::atomic_write_json(&path, &doc)
}

fn hosts_json(hosts: &[Host]) -> String {
    serde_json::to_string(hosts).unwrap_or_else(|_| "[]".into())
}

/// Validate a host entry. Returns `Ok(())` or an error message suitable
/// for surfacing in QML.
fn validate_host(
    name: &str,
    url: &str,
    existing: &[Host],
    editing_name: Option<&str>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Host name is required.".into());
    }
    if name.chars().count() > 64 {
        return Err("Host name must be 64 characters or fewer.".into());
    }
    // editing_name allows renaming a host to its own current name.
    if existing.iter().any(|h| h.name == name && Some(h.name.as_str()) != editing_name) {
        return Err(format!("A host named '{name}' already exists."));
    }
    let url = url.trim();
    let target = url.strip_prefix("ssh://").ok_or("URL must use the ssh:// scheme.")?;
    crate::client::validate_ssh_target(target).map_err(|e| e.to_string())?;
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
        #[qproperty(QString, hosts_json)]
        #[qproperty(bool, is_connecting)]
        #[qproperty(QString, last_error)]
        #[qproperty(QString, active_host)]
        type HostsModel = super::HostsModelRust;

        /// Re-load the persisted host list from disk.
        #[qinvokable]
        fn reload(self: Pin<&mut HostsModel>);

        /// Add a new saved host. On success the list is persisted and
        /// reloaded; on failure `last_error` is set.
        #[qinvokable]
        fn add(self: Pin<&mut HostsModel>, name: &QString, url: &QString);

        /// Remove a saved host by name.
        #[qinvokable]
        fn remove(self: Pin<&mut HostsModel>, name: &QString);

        /// Rename a saved host.
        #[qinvokable]
        fn rename(self: Pin<&mut HostsModel>, old_name: &QString, new_name: &QString);

        /// Connect to a saved host by name.
        #[qinvokable]
        fn connect_to(self: Pin<&mut HostsModel>, name: &QString);

        /// Switch back to the local daemon.
        #[qinvokable]
        fn connect_local(self: Pin<&mut HostsModel>);

    }

    impl cxx_qt::Threading for HostsModel {}
}

#[derive(Default)]
pub struct HostsModelRust {
    hosts_json: QString,
    is_connecting: bool,
    last_error: QString,
    active_host: QString,
    request_generation: u64,
}

impl RequestGenerated for HostsModelRust {
    fn request_generation(&self) -> u64 {
        self.request_generation
    }
    fn bump_request_generation(&mut self) -> u64 {
        self.request_generation += 1;
        self.request_generation
    }
}

impl ffi::HostsModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        let hosts = load_hosts();
        self.as_mut().set_hosts_json(QString::from(hosts_json(&hosts).as_str()));
        self.as_mut().set_last_error(QString::from(""));
        // Mirror the Workspace's current target so a CLI --connect launch
        // shows the right label in the host switcher from the first frame.
        let target = with_workspace(|w| w.active_target());
        self.as_mut().set_active_host(QString::from(target.as_str()));
    }

    pub fn add(mut self: Pin<&mut Self>, name: &QString, url: &QString) {
        let n = name.to_string();
        let u = url.to_string();
        let mut hosts = load_hosts();
        if let Err(e) = validate_host(&n, &u, &hosts, None) {
            self.as_mut().set_last_error(QString::from(e.as_str()));
            return;
        }
        hosts.push(Host { name: n.trim().to_string(), url: u.trim().to_string() });
        if let Err(e) = save_hosts(&hosts) {
            self.as_mut().set_last_error(QString::from(format!("save failed: {e}").as_str()));
            return;
        }
        self.as_mut().set_hosts_json(QString::from(hosts_json(&hosts).as_str()));
        self.as_mut().set_last_error(QString::from(""));
    }

    pub fn remove(mut self: Pin<&mut Self>, name: &QString) {
        let n = name.to_string();
        let mut hosts = load_hosts();
        let before = hosts.len();
        hosts.retain(|h| h.name != n);
        if hosts.len() == before {
            self.as_mut().set_last_error(QString::from(format!("host '{n}' not found").as_str()));
            return;
        }
        if let Err(e) = save_hosts(&hosts) {
            self.as_mut().set_last_error(QString::from(format!("save failed: {e}").as_str()));
            return;
        }
        self.as_mut().set_hosts_json(QString::from(hosts_json(&hosts).as_str()));
        self.as_mut().set_last_error(QString::from(""));
    }

    pub fn rename(mut self: Pin<&mut Self>, old_name: &QString, new_name: &QString) {
        let old = old_name.to_string();
        let new = new_name.to_string();
        let mut hosts = load_hosts();
        let Some(pos) = hosts.iter().position(|h| h.name == old) else {
            self.as_mut().set_last_error(QString::from(format!("host '{old}' not found").as_str()));
            return;
        };
        if let Err(e) = validate_host(&new, &hosts[pos].url, &hosts, Some(&old)) {
            self.as_mut().set_last_error(QString::from(e.as_str()));
            return;
        }
        hosts[pos].name = new.trim().to_string();
        if let Err(e) = save_hosts(&hosts) {
            self.as_mut().set_last_error(QString::from(format!("save failed: {e}").as_str()));
            return;
        }
        self.as_mut().set_hosts_json(QString::from(hosts_json(&hosts).as_str()));
        self.as_mut().set_last_error(QString::from(""));
    }

    pub fn connect_to(mut self: Pin<&mut Self>, name: &QString) {
        let n = name.to_string();
        let hosts = load_hosts();
        let Some(host) = hosts.iter().find(|h| h.name == n) else {
            self.as_mut().set_last_error(QString::from(format!("host '{n}' not found").as_str()));
            return;
        };
        self.spawn_reconnect(host.url.clone());
    }

    pub fn connect_local(self: Pin<&mut Self>) {
        self.spawn_reconnect("local".to_string());
    }

    fn spawn_reconnect(mut self: Pin<&mut Self>, target: String) {
        // Ignore overlapping reconnect requests. The Workspace serializes
        // reconnects internally, but each UI click still spawns a thread
        // that blocks on the lock; drop duplicates here to avoid unbounded
        // thread accumulation if the user hammers the switcher. Read and
        // set the flag in one mutable borrow so two rapid signals cannot
        // both observe false.
        let was_connecting = {
            let this = self.as_mut();
            let connecting = *this.is_connecting();
            if !connecting {
                this.set_is_connecting(true);
            }
            connecting
        };
        if was_connecting {
            return;
        }
        let active_target = target.clone();
        self.as_mut().set_last_error(QString::from(""));
        let generation = self.as_mut().rust_mut().bump_request_generation();
        let qt_thread = self.qt_thread();
        let workspace = with_workspace(|w| Arc::clone(&w));
        spawn_rpc(
            qt_thread,
            generation,
            move || workspace.reconnect(&target),
            move |mut pin, result| {
                pin.as_mut().set_is_connecting(false);
                match result {
                    Ok(()) => {
                        pin.as_mut().set_active_host(QString::from(active_target.as_str()));
                        pin.as_mut().set_last_error(QString::from(""));
                    }
                    Err(e) => {
                        pin.as_mut().set_last_error(QString::from(e.as_str()));
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qobjects::preferences_model::tests::TempXdgConfig;

    #[test]
    fn round_trip_preserves_hosts() {
        let _g = TempXdgConfig::new();
        let hosts = vec![
            Host { name: "htpc".into(), url: "ssh://thomas@htpc".into() },
            Host { name: "lab".into(), url: "ssh://lab.example.com:2222".into() },
        ];
        save_hosts(&hosts).unwrap();
        let loaded = load_hosts();
        assert_eq!(loaded, hosts);
    }

    #[test]
    fn load_missing_returns_empty() {
        let _g = TempXdgConfig::new();
        assert!(load_hosts().is_empty());
    }

    #[test]
    fn validation_rejects_empty_name() {
        let hosts = vec![Host { name: "htpc".into(), url: "ssh://htpc".into() }];
        assert!(validate_host("", "ssh://new", &hosts, None).is_err());
        assert!(validate_host("   ", "ssh://new", &hosts, None).is_err());
    }

    #[test]
    fn validation_rejects_duplicate_name() {
        let hosts = vec![Host { name: "htpc".into(), url: "ssh://htpc".into() }];
        assert!(validate_host("htpc", "ssh://other", &hosts, None).is_err());
        // Renaming a host to itself is allowed.
        assert!(validate_host("htpc", "ssh://htpc", &hosts, Some("htpc")).is_ok());
    }

    #[test]
    fn validation_rejects_non_ssh_url() {
        let hosts: Vec<Host> = Vec::new();
        assert!(validate_host("a", "http://host", &hosts, None).is_err());
        assert!(validate_host("a", "host", &hosts, None).is_err());
        assert!(validate_host("a", "", &hosts, None).is_err());
    }

    #[test]
    fn validation_rejects_dash_prefix_target() {
        let hosts: Vec<Host> = Vec::new();
        assert!(validate_host("a", "ssh://-oProxyCommand=evil", &hosts, None).is_err());
        assert!(validate_host("a", "ssh://user@-oProxyCommand=evil", &hosts, None).is_err());
    }

    #[test]
    fn hosts_json_serializes_array() {
        let hosts = vec![Host { name: "htpc".into(), url: "ssh://thomas@htpc".into() }];
        let json = hosts_json(&hosts);
        assert!(json.contains(r#""name":"htpc""#));
        assert!(json.contains(r#""url":"ssh://thomas@htpc""#));
    }

    #[test]
    fn validation_rejects_overly_long_name() {
        let hosts: Vec<Host> = Vec::new();
        let long = "a".repeat(65);
        assert!(validate_host(&long, "ssh://host", &hosts, None).is_err());
        let ok = "a".repeat(64);
        assert!(validate_host(&ok, "ssh://host", &hosts, None).is_ok());
        // Character count, not byte count.
        let ok_unicode = "😀".repeat(64);
        assert_eq!(ok_unicode.chars().count(), 64);
        assert!(validate_host(&ok_unicode, "ssh://host", &hosts, None).is_ok());
        assert!(validate_host(&(ok_unicode + "x"), "ssh://host", &hosts, None).is_err());
    }

    #[test]
    fn malformed_file_is_backed_up_and_defaults_empty() {
        let _g = TempXdgConfig::new();
        let path = hosts_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json {{{").unwrap();
        let loaded = load_hosts();
        assert!(loaded.is_empty());
        assert!(path.with_extension("json.bad").exists());
    }
}
