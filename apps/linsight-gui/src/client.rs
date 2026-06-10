// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Postcard client to `linsightd`. Connects to an existing daemon or
//! spawns one as a child process. Exposes:
//!
//! * `subscribe` to manage server-side sampling.
//! * `take_sample_rx` returning the channel receiver QObjects drain to
//!   get live samples (the reader thread runs in the background).
//! * `get_hardware` / `set_nickname` — correlated request/response RPCs
//!   (v2 protocol). Each call gets a unique `req_id`; the reader thread
//!   matches incoming `ServerMsg::Response { req_id, .. }` against an
//!   `inflight` table to wake the caller.
//! * `subscribe_catalogue` — receive `SensorListBroadcast` updates so
//!   QObjects can refresh tile labels after a nickname change without
//!   re-handshaking.
//!
//! The reader thread is the single point of demultiplexing — every
//! `ServerMsg` arm is handled there rather than dropped on the floor,
//! which was the v0.3 behavior.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use linsight_core::{HardwareDevice, Sample, SensorId};
use linsight_protocol::{
    AlertRuleJson, ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, ProtoError, RequestOp,
    ResponsePayload, SensorInfo, ServerMsg,
};
use tracing::{info, warn};

pub type ClientHandle = Arc<Client>;

/// Error type for the v2 request/response RPCs (`get_hardware`,
/// `set_nickname`). The reader thread parks the caller on an
/// `mpsc::channel`; the variants below cover every way that wait can
/// finish unsuccessfully — server-rejected, wrong payload (bug), timed
/// out, or the framing layer rejected our send.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("server: {0}")]
    Server(String),
    #[error("unexpected payload: {0}")]
    UnexpectedPayload(String),
    #[error("request timed out")]
    Timeout,
    #[error("send: {0}")]
    Send(String),
}

/// `mpsc` sender flavor parked in the inflight table. The reader thread
/// pushes either the daemon's `ResponsePayload` or a `ProtoError` when
/// the matching `req_id` arrives; the caller pulls it off via
/// `recv_timeout`.
type ResponseSender = std::sync::mpsc::Sender<Result<ResponsePayload, ProtoError>>;

pub struct Client {
    writer: Mutex<FrameWriter<UnixStream>>,
    sample_rx: Mutex<Option<Receiver<Sample>>>,
    /// Live snapshot of the daemon's sensor catalogue. Seeded at
    /// handshake from `ListSensors`; the reader thread replaces it
    /// wholesale on each `SensorListBroadcast` (e.g. after a nickname
    /// change).
    catalogue: Arc<Mutex<Vec<SensorInfo>>>,
    /// Correlation map for v2 `Request`/`Response`. The RPC method
    /// inserts a sender keyed by `req_id`, sends the `Request`, and
    /// waits on the matching receiver. The reader thread removes the
    /// entry and forwards the result when the `Response` arrives. On
    /// timeout the caller removes its own entry to avoid leaking
    /// senders for never-arriving responses. Mutated through
    /// `get_hardware` / `set_nickname`, which the HardwareModel calls.
    inflight: Arc<Mutex<HashMap<u32, ResponseSender>>>,
    /// Monotonically-increasing source of `req_id`s. Wrap-around at
    /// 2^32 is fine — the inflight map can't realistically have 4B
    /// entries, so collisions are not a concern.
    next_req_id: Arc<AtomicU32>,
    /// QObjects that want to learn about `SensorListBroadcast`s
    /// register an `mpsc::Sender` here via `subscribe_catalogue`. The
    /// reader thread broadcasts the new `Vec<SensorInfo>` to each;
    /// disconnected senders are pruned on the next push.
    catalogue_listeners: Arc<Mutex<Vec<Sender<Vec<SensorInfo>>>>>,
    // Held to keep the spawned daemon alive; dropped on Client::drop.
    _child: Mutex<Option<Child>>,
    /// SSH `ssh -L` child process (Some only for `connect_ssh`). Killed on
    /// Drop alongside the daemon child.
    ssh_child: Mutex<Option<Child>>,
    /// Temp socket path created for SSH port-forwarding, removed on Drop
    /// so repeated SSH sessions don't leak `/tmp/linsight-remote-*.sock`.
    ssh_socket_path: Mutex<Option<PathBuf>>,
}

impl Client {
    /// Connect to a remote `linsightd` over SSH. The URL is
    /// `ssh://[user@]host[:port]`. The remote socket path is discovered by
    /// running `printenv XDG_RUNTIME_DIR` on the remote and appending
    /// `/linsight.sock`. A local Unix socket is allocated under `/tmp`,
    /// `ssh -N -L local:remote` is spawned, then we attach to the local
    /// socket exactly like a local connection.
    pub fn connect_ssh(url: &str) -> Result<ClientHandle> {
        let target = url
            .strip_prefix("ssh://")
            .ok_or_else(|| anyhow::anyhow!("expected ssh:// prefix, got {url}"))?;
        if target.starts_with('-') {
            anyhow::bail!(
                "invalid SSH target: {target:?} starts with '-'; possible option injection"
            );
        }
        let remote_socket = discover_remote_socket(target)?;
        let local_socket =
            std::env::temp_dir().join(format!("linsight-remote-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&local_socket);
        let ssh = Command::new("ssh")
            .args(["-N", "-L", &format!("{}:{}", local_socket.display(), remote_socket), target])
            .spawn()
            .context("spawning ssh -L")?;
        // Wait for the local socket to be ready.
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            if Instant::now() > deadline {
                anyhow::bail!("ssh -L to {target} did not establish the socket within 10s");
            }
            match UnixStream::connect(&local_socket) {
                Ok(s) => break s,
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        };
        info!(target, remote = %remote_socket, "attached to remote daemon over ssh");
        let client = Self::finish_handshake(stream, None)?;
        // Stash ssh child + socket path so Drop cleans both up.
        *client.ssh_child.lock().unwrap() = Some(ssh);
        *client.ssh_socket_path.lock().unwrap() = Some(local_socket);
        Ok(client)
    }

    pub fn connect_or_spawn(socket: &Path) -> Result<ClientHandle> {
        let (stream, child) = connect_or_spawn_inner(socket)?;
        Self::finish_handshake(stream, child)
    }

    fn finish_handshake(stream: UnixStream, child: Option<Child>) -> Result<ClientHandle> {
        let read_stream = stream.try_clone().context("clone stream")?;
        let mut reader = FrameReader::new(read_stream);
        let mut writer = FrameWriter::new(stream);

        writer.write_client(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "linsight-gui".into(),
            auth_token: None,
        })?;
        // Validate the daemon's protocol_version: the CLI does this, the
        // daemon verifies the client's Hello, but the GUI used to wildcard-
        // match the Welcome and would silently keep talking to a
        // version-skewed daemon.
        match reader.read_server()? {
            ServerMsg::Welcome { protocol_version, .. } if protocol_version == PROTOCOL_VERSION => {
            }
            ServerMsg::Welcome { protocol_version, .. } => {
                anyhow::bail!(
                    "protocol mismatch: daemon={protocol_version} gui={PROTOCOL_VERSION}",
                );
            }
            ServerMsg::Bye { reason } => anyhow::bail!("daemon refused: {reason}"),
            other => anyhow::bail!("unexpected handshake reply: {other:?}"),
        }

        // Cache the sensor catalogue once during the handshake so the GUI
        // can categorize tiles without round-tripping again per page.
        writer.write_client(&ClientMsg::ListSensors)?;
        let sensors = match reader.read_server()? {
            ServerMsg::SensorList(infos) => infos,
            other => anyhow::bail!("unexpected reply to ListSensors: {other:?}"),
        };
        info!(count = sensors.len(), "sensor catalogue cached");

        let (tx, rx) = channel::<Sample>();
        let catalogue = Arc::new(Mutex::new(sensors));
        let inflight: Arc<Mutex<HashMap<u32, ResponseSender>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let catalogue_listeners: Arc<Mutex<Vec<Sender<Vec<SensorInfo>>>>> =
            Arc::new(Mutex::new(Vec::new()));
        {
            let catalogue = Arc::clone(&catalogue);
            let inflight = Arc::clone(&inflight);
            let listeners = Arc::clone(&catalogue_listeners);
            thread::spawn(move || dispatch(reader, tx, catalogue, inflight, listeners));
        }

        Ok(Arc::new(Client {
            writer: Mutex::new(writer),
            sample_rx: Mutex::new(Some(rx)),
            catalogue,
            inflight,
            next_req_id: Arc::new(AtomicU32::new(1)),
            catalogue_listeners,
            _child: Mutex::new(child),
            ssh_child: Mutex::new(None),
            ssh_socket_path: Mutex::new(None),
        }))
    }

    /// Snapshot of the daemon's last-known sensor catalogue. Returns an
    /// owned `Vec` (not a borrowed slice) because the underlying storage
    /// may be replaced wholesale by the reader thread when a
    /// `SensorListBroadcast` arrives.
    pub fn sensor_infos(&self) -> Vec<SensorInfo> {
        self.catalogue.lock().expect("catalogue mutex poisoned").clone()
    }

    pub fn subscribe(&self, sensors: Vec<SensorId>) -> Result<()> {
        self.writer
            .lock()
            .unwrap()
            .write_client(&ClientMsg::Subscribe { sensors, rate_hz: None })?;
        Ok(())
    }

    /// Take ownership of the sample receiver. Called once by the
    /// QObject that owns the live UI.
    pub fn take_sample_rx(&self) -> Option<Receiver<Sample>> {
        self.sample_rx.lock().expect("Client sample_rx mutex poisoned").take()
    }

    /// Subscribe to `SensorListBroadcast`s. Each returned receiver gets
    /// the full fresh `Vec<SensorInfo>` from the daemon when a
    /// nickname change (or any other catalogue-altering event) lands.
    /// Drop the receiver to unsubscribe — the dispatcher prunes
    /// disconnected senders lazily on the next broadcast.
    pub fn subscribe_catalogue(&self) -> Receiver<Vec<SensorInfo>> {
        let (tx, rx) = channel();
        self.catalogue_listeners.lock().expect("listeners poisoned").push(tx);
        rx
    }

    /// v2 RPC: ask the daemon for the current hardware inventory.
    /// Blocks the caller up to `timeout` waiting for the matching
    /// `Response`. On timeout, the inflight entry is reaped so the
    /// reader thread doesn't accumulate dead senders.
    pub fn get_hardware(
        &self,
        timeout: Duration,
    ) -> Result<(Vec<HardwareDevice>, HashMap<String, String>), RpcError> {
        self.request_rpc(RequestOp::GetHardware, timeout, |payload| match payload {
            ResponsePayload::Hardware { devices, nicknames } => Ok((devices, nicknames)),
            other => Err(other),
        })
    }

    /// v2 RPC: set or clear a hardware device's nickname. `value = None`
    /// clears it. The daemon validates the nickname; an invalid one
    /// surfaces as `RpcError::Server`. On success, the daemon will also
    /// emit a `SensorListBroadcast` shortly after — subscribers via
    /// `subscribe_catalogue` will see the refreshed labels.
    pub fn set_nickname(
        &self,
        device_key: &str,
        value: Option<String>,
        timeout: Duration,
    ) -> Result<(), RpcError> {
        let op = RequestOp::SetNickname { device_key: device_key.to_owned(), value };
        self.request_rpc(op, timeout, |payload| match payload {
            ResponsePayload::NicknameSet { .. } => Ok(()),
            other => Err(other),
        })
    }

    /// v2 RPC: ask the daemon to use `ms` between pump-thread ticks
    /// for THIS client. The daemon clamps to
    /// `[PUMP_INTERVAL_MIN_MS, PUMP_INTERVAL_MAX_MS]` and replies with
    /// the value actually applied. Each client has its own pump-tick;
    /// other clients are unaffected.
    /// v2 RPC: query historical samples for a sensor within the given
    /// time window (microseconds since epoch). Returns up to `max_points`
    /// samples, evenly spaced via server-side downsampling.
    pub fn get_history(
        &self,
        sensor: &str,
        since_micros: u64,
        until_micros: u64,
        max_points: Option<u32>,
        timeout: Duration,
    ) -> Result<Vec<Sample>, RpcError> {
        let op = RequestOp::GetHistory {
            sensor: sensor.to_owned(),
            since_micros,
            until_micros,
            max_points,
        };
        self.request_rpc(op, timeout, |payload| match payload {
            ResponsePayload::History { sensor: _, samples } => Ok(samples),
            other => Err(other),
        })
    }

    /// v2 RPC: list all alert rules from the daemon.
    pub fn list_alerts(&self, timeout: Duration) -> Result<Vec<AlertRuleJson>, RpcError> {
        self.request_rpc(RequestOp::ListAlerts, timeout, |payload| match payload {
            ResponsePayload::AlertList { rules } => Ok(rules),
            other => Err(other),
        })
    }

    /// v2 RPC: upsert (add or update) an alert rule.
    pub fn upsert_alert(
        &self,
        name: &str,
        expr: &str,
        for_duration: Option<String>,
        cooldown: Option<String>,
        notify: Vec<String>,
        enabled: Option<bool>,
        timeout: Duration,
    ) -> Result<String, RpcError> {
        let op = RequestOp::UpsertAlert {
            name: name.to_owned(),
            expr: expr.to_owned(),
            for_duration,
            cooldown,
            notify,
            enabled,
        };
        self.request_rpc(op, timeout, |payload| match payload {
            ResponsePayload::AlertUpserted { name } => Ok(name),
            other => Err(other),
        })
    }

    /// v2 RPC: delete an alert rule by name.
    pub fn delete_alert(&self, name: &str, timeout: Duration) -> Result<String, RpcError> {
        let op = RequestOp::DeleteAlert { name: name.to_owned() };
        self.request_rpc(op, timeout, |payload| match payload {
            ResponsePayload::AlertDeleted { name } => Ok(name),
            other => Err(other),
        })
    }

    /// v2 RPC: test an alert expression against current sensor values.
    pub fn test_alert_expr(
        &self,
        expr: &str,
        timeout: Duration,
    ) -> Result<(bool, Option<String>), RpcError> {
        let op = RequestOp::TestAlertExpr { expr: expr.to_owned() };
        self.request_rpc(op, timeout, |payload| match payload {
            ResponsePayload::AlertTestResult { is_true, error } => Ok((is_true, error)),
            other => Err(other),
        })
    }

    pub fn set_pump_interval_ms(&self, ms: u32, timeout: Duration) -> Result<u32, RpcError> {
        self.request_rpc(RequestOp::SetPumpIntervalMs { ms }, timeout, |payload| match payload {
            ResponsePayload::PumpIntervalSet { ms: applied } => Ok(applied),
            other => Err(other),
        })
    }

    /// Send a v2 `Request` and wait for the matching `Response`.
    /// `extract` pattern-matches the success payload into the caller's
    /// return type, returning `Err(payload)` if the variant doesn't
    /// match what the caller expected (which `request_rpc` then maps
    /// to `RpcError::UnexpectedPayload` so each call site doesn't
    /// repeat the boilerplate).
    ///
    /// Lifted from three near-identical copies (`get_hardware`,
    /// `set_nickname`, `set_pump_interval_ms`) that each carried the
    /// same req_id-mint + inflight-insert + write + recv_timeout +
    /// match dance.
    fn request_rpc<R, F>(&self, op: RequestOp, timeout: Duration, extract: F) -> Result<R, RpcError>
    where
        F: FnOnce(ResponsePayload) -> Result<R, ResponsePayload>,
    {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel::<Result<ResponsePayload, ProtoError>>();
        self.inflight.lock().expect("inflight poisoned").insert(req_id, tx);
        if let Err(e) = self
            .writer
            .lock()
            .expect("writer poisoned")
            .write_client(&ClientMsg::Request { req_id, op })
        {
            self.inflight.lock().expect("inflight poisoned").remove(&req_id);
            return Err(RpcError::Send(e.to_string()));
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(payload)) => {
                extract(payload).map_err(|wrong| RpcError::UnexpectedPayload(format!("{wrong:?}")))
            }
            Ok(Err(e)) => Err(RpcError::Server(e.message)),
            Err(_) => {
                self.inflight.lock().expect("inflight poisoned").remove(&req_id);
                Err(RpcError::Timeout)
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self
            .writer
            .lock()
            .expect("Client writer mutex poisoned")
            .write_client(&ClientMsg::Goodbye);
        if let Some(mut child) = self._child.lock().unwrap().take() {
            // Give the daemon up to 500 ms to exit gracefully on Goodbye.
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        // Tear down the SSH tunnel (only set by connect_ssh) and remove
        // its temp socket so repeated SSH sessions don't leak files into
        // /tmp.
        if let Some(mut ssh) = self.ssh_child.lock().unwrap().take() {
            let _ = ssh.kill();
            let _ = ssh.wait();
        }
        if let Some(path) = self.ssh_socket_path.lock().unwrap().take() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn connect_or_spawn_inner(socket: &Path) -> Result<(UnixStream, Option<Child>)> {
    if let Ok(s) = UnixStream::connect(socket) {
        info!(socket = %socket.display(), "attached to running daemon");
        return Ok((s, None));
    }
    info!(socket = %socket.display(), "no daemon found, spawning child");
    let bin = locate_linsightd()?;
    // Pass the socket via OsStr rather than .to_str().unwrap() — Linux
    // paths can contain non-UTF-8 bytes (e.g. user-set XDG_RUNTIME_DIR
    // pointing at a path with arbitrary OS bytes) and unwrap would panic.
    let mut cmd = Command::new(&bin);
    cmd.arg("--socket").arg(socket);
    let child = cmd.spawn().with_context(|| format!("spawning {}", bin.display()))?;

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(socket) {
            return Ok((s, Some(child)));
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("daemon at {} did not bind socket within 3s", bin.display())
}

fn locate_linsightd() -> Result<PathBuf> {
    // Look for `linsightd` next to the current binary first (the cargo
    // target dir layout puts both binaries side by side). Fall back to
    // bare `linsightd` so $PATH lookup applies.
    let next_to_us = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("linsightd")))
        .filter(|p| p.exists());
    if let Some(p) = next_to_us {
        return Ok(p);
    }
    Ok(PathBuf::from("linsightd"))
}

/// Ask the remote host where to find its LinSight socket. Runs
/// `printenv XDG_RUNTIME_DIR` over SSH; if unset, falls back to `/run/user/$(id -u)`.
fn discover_remote_socket(target: &str) -> Result<String> {
    let out = Command::new("ssh")
        .args([
            target,
            "sh",
            "-c",
            "printf %s \"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}\"/linsight.sock",
        ])
        .output()
        .context("running ssh to discover remote socket path")?;
    if !out.status.success() {
        anyhow::bail!("ssh to {target} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let path = String::from_utf8(out.stdout)
        .context("non-UTF-8 socket path from remote")?
        .trim()
        .to_owned();
    if path.is_empty() {
        anyhow::bail!("remote returned empty socket path");
    }
    Ok(path)
}

/// Demultiplexer for everything the daemon pushes after the handshake.
///
/// v0.3 used a one-arm `pump_samples` that dropped every non-Sample
/// variant — that worked when the protocol only had Samples to push.
/// v2 added `Response` (correlated by `req_id`) and `SensorListBroadcast`
/// (catalogue refresh), so we now branch on every variant. Samples
/// continue to flow through the existing `Sender<Sample>`; everything
/// else is handed off via the shared `inflight` table or the
/// `catalogue_listeners` fan-out.
fn dispatch(
    mut reader: FrameReader<UnixStream>,
    sample_tx: Sender<Sample>,
    catalogue: Arc<Mutex<Vec<SensorInfo>>>,
    inflight: Arc<Mutex<HashMap<u32, ResponseSender>>>,
    catalogue_listeners: Arc<Mutex<Vec<Sender<Vec<SensorInfo>>>>>,
) {
    loop {
        match reader.read_server() {
            Ok(ServerMsg::Sample(s)) => {
                if sample_tx.send(s).is_err() {
                    // The OverviewModel dropped its receiver — typically
                    // because the app is shutting down. Bail rather than
                    // spinning on a dead channel.
                    break;
                }
            }
            Ok(ServerMsg::Response { req_id, result }) => {
                // Take ownership of the parked sender so we don't hold
                // the lock across send().
                let waiter = inflight.lock().expect("inflight poisoned").remove(&req_id);
                if let Some(tx) = waiter {
                    let _ = tx.send(result);
                } else {
                    warn!(req_id, "response for unknown req_id; daemon out of sync?");
                }
            }
            Ok(ServerMsg::SensorListBroadcast(infos)) => {
                *catalogue.lock().expect("catalogue poisoned") = infos.clone();
                let mut listeners = catalogue_listeners.lock().expect("listeners poisoned");
                // Send to every listener; drop those whose receivers
                // have been dropped.
                listeners.retain(|tx| tx.send(infos.clone()).is_ok());
            }
            Ok(ServerMsg::SensorDegraded { sensor, reason }) => {
                warn!(?sensor, %reason, "sensor degraded");
            }
            Ok(ServerMsg::Bye { reason }) => {
                info!(%reason, "daemon Bye; reader thread exiting");
                break;
            }
            Ok(ServerMsg::Welcome { .. }) | Ok(ServerMsg::SensorList(_)) => {
                // Only valid during handshake; ignore here.
            }
            Err(e) => {
                warn!(error = ?e, "daemon connection closed");
                break;
            }
        }
    }
}
