// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::ffi::OsStr;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use anyhow::Context;
use linsight_core::history_db_path;
use tracing::{info, warn};

use crate::alerts::AlertEngine;
use crate::hardware::HardwareRegistry;
use crate::history;
use crate::nickname_store::NicknameStore;
use crate::plugin_host::PluginHost;
use crate::prom;
use crate::scheduler::Scheduler;
use crate::transport;

pub fn run(socket: PathBuf) -> anyhow::Result<()> {
    if socket.exists() {
        match UnixStream::connect(&socket) {
            Ok(_) => {
                anyhow::bail!(
                    "{} is already in use by a live listener; refusing to overwrite",
                    socket.display(),
                );
            }
            Err(_) => {
                std::fs::remove_file(&socket)
                    .with_context(|| format!("removing stale socket at {}", socket.display()))?;
            }
        }
    }

    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    // chmod after bind rather than clamping the umask: umask is process-global
    // and would corrupt file modes in any concurrent thread (notably the test
    // harness). The brief window between bind() and this chmod is not
    // exploitable in practice — the socket lives in $XDG_RUNTIME_DIR (mode
    // 0700 per the XDG spec, so no other user can traverse to it) and every
    // accepted connection is SO_PEERCRED-checked regardless of socket mode.
    std::fs::set_permissions(&socket, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .with_context(|| format!("setting permissions on {}", socket.display()))?;
    listener.set_nonblocking(true).context("setting listener non-blocking")?;
    info!(socket = %socket.display(), "linsightd listening");

    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
        .context("installing SIGTERM handler")?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
        .context("installing SIGINT handler")?;

    let plugin_configs = load_plugin_configs();

    let mut host = PluginHost::with_builtins_and_config(&plugin_configs);
    host.load_dynamic_plugins(&plugin_configs);

    // Build the HardwareRegistry from the loaded plugins' manifests and
    // the on-disk nickname overlay BEFORE moving `host` into Scheduler.
    // The registry is shared with the transport layer (decorating
    // outgoing SensorInfo with device_key/device_label) and is mutated
    // by SetNickname RPCs through the daemon's lifetime.
    let store_path = nickname_store_path();
    let store = NicknameStore::load(&store_path);
    let manifests: Vec<_> = host.devices_by_plugin().collect();
    let registry = HardwareRegistry::build(&manifests, store.nicknames);
    info!(
        device_count = registry.devices.len(),
        nickname_count = registry.nicknames_snapshot().len(),
        "hardware registry built",
    );
    let registry = Arc::new(RwLock::new(registry));
    drop(manifests);

    let mut scheduler = Scheduler::new(host);

    // Opt-in history (always-on mode subsystem 1 of 3). The join handle is
    // held until the runtime exits so a writer-thread crash is observable
    // on shutdown (the join will surface a panic).
    let mut _history_join: Option<std::thread::JoinHandle<()>> = None;
    let mut history_writer_clone: Option<crate::history::HistoryWriter> = None;
    if std::env::var_os("LINSIGHT_HISTORY").is_some() {
        let db_path = history_db_path();
        let retention = history::retention_from_env(
            std::env::var("LINSIGHT_HISTORY_RETENTION").ok().as_deref(),
        );
        match retention {
            Some(d) => info!(retention_secs = d.as_secs(), "history retention window"),
            None => info!("history retention: keep forever"),
        }
        match history::spawn(db_path.clone(), retention) {
            Ok((writer, join)) => {
                history_writer_clone = Some(writer.clone());
                scheduler.set_history_writer(Some(writer));
                scheduler.set_history_db_path(Some(db_path));
                _history_join = Some(join);
            }
            Err(e) => warn!(error = ?e, db = %db_path.display(), "history disabled (spawn failed)"),
        }
    }

    // Opt-in alerts (always-on mode subsystem 2 of 3).
    if std::env::var_os("LINSIGHT_ALERTS").is_some() {
        let toml_path = alerts_config_path();
        match AlertEngine::load(&toml_path) {
            Ok(engine) => {
                let handle = engine.into_handle();
                // If history is also enabled, wire the writer so alert
                // events survive daemon restarts.
                if let Some(ref writer) = history_writer_clone {
                    handle.set_event_writer(Some(writer.clone()));
                }
                scheduler.set_alert_engine(Some(handle));
                scheduler.set_alerts_config_path(Some(toml_path));
            }
            Err(e) => {
                warn!(error = ?e, path = %toml_path.display(), "alerts disabled (load failed)")
            }
        }
    }

    let scheduler = Arc::new(Mutex::new(scheduler));

    // Opt-in Prometheus exporter (always-on mode subsystem 3 of 3).
    let mut _prom_shutdown: Option<Arc<AtomicBool>> = None;
    if let Ok(bind) = std::env::var("LINSIGHT_PROM_BIND") {
        match prom::spawn(&bind, Arc::clone(&scheduler), Arc::clone(&registry)) {
            Ok(prom_shutdown) => {
                scheduler.lock().unwrap().set_prom_running(true);
                _prom_shutdown = Some(prom_shutdown);
            }
            Err(e) => warn!(error = ?e, "Prometheus exporter disabled"),
        }
    }

    let _guard = SocketGuard(socket.clone());
    transport::unix::accept_loop(listener, scheduler, registry, store_path, shutdown)
}

/// Resolve a config file path under `$XDG_CONFIG_HOME/linsight/` or
/// `~/.config/linsight/`, falling back to `fallback` when neither env var is
/// set. The three public helpers below are thin wrappers over this function.
fn config_path_from(
    xdg: Option<&OsStr>,
    home: Option<&OsStr>,
    relative: impl AsRef<Path>,
    fallback: impl AsRef<Path>,
) -> PathBuf {
    let relative = relative.as_ref();
    if let Some(d) = xdg {
        PathBuf::from(d).join("linsight").join(relative)
    } else if let Some(h) = home {
        PathBuf::from(h).join(".config").join("linsight").join(relative)
    } else {
        fallback.as_ref().to_path_buf()
    }
}

fn config_path(relative: impl AsRef<Path>, fallback: impl AsRef<Path>) -> PathBuf {
    config_path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        relative,
        fallback,
    )
}

/// Resolve `$XDG_CONFIG_HOME/linsight/hardware.json`, falling back to
/// `~/.config/linsight/hardware.json` if XDG isn't set, or finally
/// `/tmp/linsight-hardware.json`. Kept here rather than in `nickname_store`
/// so the store stays a pure file API.
pub fn nickname_store_path() -> PathBuf {
    config_path("hardware.json", "/tmp/linsight-hardware.json")
}

pub(crate) fn alerts_config_path() -> PathBuf {
    config_path("alerts.toml", "/etc/linsight/alerts.toml")
}

fn plugins_config_path() -> PathBuf {
    config_path("plugins.toml", "/etc/linsight/plugins.toml")
}

const MAX_PLUGINS_CONFIG_BYTES: u64 = 1 << 20;

fn load_plugin_configs() -> std::collections::HashMap<String, serde_json::Value> {
    let path = plugins_config_path();
    let mut out = std::collections::HashMap::new();
    if !path.exists() {
        return out;
    }
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to stat plugins.toml");
            return out;
        }
    };
    if meta.len() > MAX_PLUGINS_CONFIG_BYTES {
        warn!(
            path = %path.display(),
            size = meta.len(),
            limit = MAX_PLUGINS_CONFIG_BYTES,
            "plugins.toml exceeds size limit, refusing to load"
        );
        return out;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read plugins.toml");
            return out;
        }
    };
    let parsed: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to parse plugins.toml");
            return out;
        }
    };
    let table = match parsed.as_table() {
        Some(t) => t,
        None => return out,
    };
    for (key, value) in table {
        out.insert(key.clone(), serde_json::to_value(value).unwrap_or(serde_json::Value::Null));
    }
    info!(path = %path.display(), count = out.len(), "plugin configs loaded");
    out
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_uses_xdg_when_set() {
        let got = config_path_from(
            Some(OsStr::new("/xdg")),
            Some(OsStr::new("/home/user")),
            "hardware.json",
            "/tmp/linsight-hardware.json",
        );
        assert_eq!(got, PathBuf::from("/xdg/linsight/hardware.json"));
    }

    #[test]
    fn config_path_falls_back_to_home_when_xdg_missing() {
        let got = config_path_from(
            None,
            Some(OsStr::new("/home/user")),
            "alerts.toml",
            "/etc/linsight/alerts.toml",
        );
        assert_eq!(got, PathBuf::from("/home/user/.config/linsight/alerts.toml"));
    }

    #[test]
    fn config_path_uses_fallback_when_both_missing() {
        let got = config_path_from(None, None, "plugins.toml", "/etc/linsight/plugins.toml");
        assert_eq!(got, PathBuf::from("/etc/linsight/plugins.toml"));
    }

    #[test]
    fn nickname_store_path_matches_xdg() {
        let got = nickname_store_path();
        assert!(got.as_path().ends_with("linsight/hardware.json"));
    }
}
