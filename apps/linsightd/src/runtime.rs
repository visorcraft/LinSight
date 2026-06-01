// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use anyhow::Context;
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
    if std::env::var_os("LINSIGHT_HISTORY").is_some() {
        let db_path = history_db_path();
        match history::spawn(db_path.clone()) {
            Ok((writer, join)) => {
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
                scheduler.set_alert_engine(Some(engine.into_handle()));
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
            Ok(prom_shutdown) => _prom_shutdown = Some(prom_shutdown),
            Err(e) => warn!(error = ?e, "Prometheus exporter disabled"),
        }
    }

    let _guard = SocketGuard(socket.clone());
    transport::unix::accept_loop(listener, scheduler, registry, store_path, shutdown)
}

/// Resolve `$XDG_CONFIG_HOME/linsight/hardware.json`, falling back to
/// `~/.config/linsight/hardware.json` if XDG isn't set. Mirrors the path
/// helpers above (`history_db_path`, `alerts_config_path`); kept here
/// rather than in `nickname_store` so the store stays a pure file API.
pub fn nickname_store_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(d).join("linsight/hardware.json")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".config/linsight/hardware.json")
    } else {
        PathBuf::from("/tmp/linsight-hardware.json")
    }
}

fn history_db_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(d).join("linsight/history.db")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".local/share/linsight/history.db")
    } else {
        PathBuf::from("/tmp/linsight-history.db")
    }
}

fn alerts_config_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(d).join("linsight/alerts.toml")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".config/linsight/alerts.toml")
    } else {
        PathBuf::from("/etc/linsight/alerts.toml")
    }
}

fn plugins_config_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(d).join("linsight/plugins.toml")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".config/linsight/plugins.toml")
    } else {
        PathBuf::from("/etc/linsight/plugins.toml")
    }
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
