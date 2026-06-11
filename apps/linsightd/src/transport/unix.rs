// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use subtle::ConstantTimeEq;

use linsight_core::SensorId;
use linsight_protocol::{
    ClientMsg, FrameError, FrameReader, FrameWriter, PROTOCOL_VERSION, PluginInfo, ServerMsg,
    verify_hello,
};
use tracing::{info, warn};

use crate::hardware::HardwareRegistry;
use crate::scheduler::{Scheduler, Subscription};

/// Polling interval when the listener has no pending connections. Smaller =
/// more responsive shutdown, larger = lower idle CPU. 100ms is invisible to
/// users and effectively free CPU-wise.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Upper bound for the exponential backoff applied to persistent (non-WouldBlock)
/// `accept()` errors. Keeps a wedged listener from spinning the CPU + log.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// How long a freshly-connected peer has to send its `Hello` frame before we
/// close the connection. Without this a client that connects and never writes
/// would park a worker thread forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of concurrently-served client sessions. Excess connections
/// receive a `Bye` and are closed immediately so the scheduler mutex's
/// contention stays bounded and the daemon can't be DoS'd by a connection
/// burst. 64 leaves room for typical GUI + CLI usage plus several Prom/tunnel
/// peers while staying well under the typical 1024 fd soft limit.
const MAX_CLIENT_SESSIONS: usize = 64;

/// Sentinel returned by [`now_micros`] when the system clock is before
/// `UNIX_EPOCH`. Distinguishable from real timestamps for downstream code.
const TS_SENTINEL_BAD_CLOCK: u64 = u64::MAX;

/// Per-client subscription tokens, grouped by sensor for quick fan-out checks.
type ClientSubscriptions = HashMap<SensorId, Vec<Subscription>>;

struct ClientSink {
    tx: std::sync::mpsc::Sender<ServerMsg>,
    subscriptions: Arc<Mutex<ClientSubscriptions>>,
}

/// Shared map of per-client outbound senders, keyed by a monotonic
/// per-connection id. The sampler and SetNickname RPC both push
/// `ServerMsg`s onto every matching sender; each client thread reads
/// from its receiver and forwards the message to its socket.
///
/// Switched from `Vec<Sender>` to `HashMap<u64, Sender>` so a client
/// can proactively deregister its own sender when `serve()` returns
/// (the previous `Vec::retain`-on-send-error path was lazy: a dead
/// sender lingered until the *next* broadcast fired, which on a
/// long-running daemon with many GUI launches and few nickname
/// changes meant the list could grow unboundedly between renames).
///
/// `std::sync::Mutex` is fine here: the broadcast path is rare (a
/// user renaming hardware) and sample fan-out does only short
/// per-client interest checks while holding it.
type ClientMap = Arc<Mutex<HashMap<u64, ClientSink>>>;

/// Process-wide monotonic counter used to mint a unique id for each
/// accepted client. The id is only meaningful inside `ClientMap`
/// — it's the key under which a `serve()` thread registers (and on
/// exit deregisters) its outbound sender. `Relaxed` ordering is
/// fine: we only need uniqueness, not a happens-before relation
/// against any other variable.
static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(0);

/// RAII guard that decrements the live-session counter when a `serve()` call
/// returns or unwinds.
struct SessionGuard(Arc<AtomicUsize>);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Optional auth token checked against every incoming Hello.
/// Read once at daemon startup from `LINSIGHT_AUTH_TOKEN` env var.
static AUTH_TOKEN: LazyLock<Option<String>> =
    LazyLock::new(|| std::env::var("LINSIGHT_AUTH_TOKEN").ok());

/// Effective UID of the daemon process, read once at first use.
/// Used to reject connections from other users via `SO_PEERCRED`.
static DAEMON_UID: LazyLock<u32> = LazyLock::new(|| unsafe { libc::geteuid() });

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut _,
            &mut len,
        )
    };
    if ret == -1 { Err(io::Error::last_os_error()) } else { Ok(cred.uid) }
}

/// Simple token-bucket rate limiter for the accept loop.
/// Refills one token per `interval` up to `capacity`. Enabled at 20/s
/// by default; configurable via `LINSIGHT_ACCEPT_RATE` env var.
struct AcceptRateLimiter {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last_refill: std::time::Instant,
}

impl AcceptRateLimiter {
    fn from_env() -> Self {
        let rate: f64 = std::env::var("LINSIGHT_ACCEPT_RATE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0_f64)
            .clamp(1.0, 200.0);
        Self {
            tokens: rate,
            capacity: rate,
            refill_per_sec: rate,
            last_refill: std::time::Instant::now(),
        }
    }

    fn acquire(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub fn accept_loop(
    listener: UnixListener,
    scheduler: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
    store_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let shared = scheduler;
    let sessions = Arc::new(AtomicUsize::new(0));
    // Registry of per-client outbound channels and subscription interests.
    // Each `serve()` thread removes its own entry on exit, so the map size
    // tracks the live connection count instead of growing until a later send
    // discovers a dead receiver.
    let clients: ClientMap = Arc::new(Mutex::new(HashMap::new()));
    let sampler = spawn_sampler(Arc::clone(&shared), Arc::clone(&clients), Arc::clone(&shutdown));
    let mut consecutive_err: u32 = 0;
    let mut rate_limiter = AcceptRateLimiter::from_env();
    while !shutdown.load(Ordering::Relaxed) {
        // Throttle accept rate: if tokens are exhausted, sleep ~50ms and retry.
        if !rate_limiter.acquire() {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        match listener.accept() {
            Ok((s, _addr)) => {
                consecutive_err = 0;
                match peer_uid(&s) {
                    Ok(uid) if uid == *DAEMON_UID => {}
                    Ok(uid) => {
                        warn!(
                            peer_uid = uid,
                            daemon_uid = *DAEMON_UID,
                            "rejecting client: peer UID does not match daemon UID",
                        );
                        continue;
                    }
                    Err(e) => {
                        warn!(error = ?e, "rejecting client: failed to read peer credentials");
                        continue;
                    }
                }
                let prior = sessions.fetch_add(1, Ordering::AcqRel);
                if prior >= MAX_CLIENT_SESSIONS {
                    // Reject without spawning a worker. Releasing the counter
                    // immediately is important so legitimate clients aren't
                    // permanently locked out by a burst.
                    sessions.fetch_sub(1, Ordering::AcqRel);
                    warn!(
                        live = prior,
                        cap = MAX_CLIENT_SESSIONS,
                        "rejecting client: session cap reached",
                    );
                    let mut writer = FrameWriter::new(s);
                    let _ = writer.write_server(&ServerMsg::Bye {
                        reason: "daemon at session capacity".into(),
                    });
                    continue;
                }
                let sched = Arc::clone(&shared);
                let reg = Arc::clone(&registry);
                let clients_for_serve = Arc::clone(&clients);
                let store_path_for_serve = store_path.clone();
                let guard = SessionGuard(Arc::clone(&sessions));
                thread::spawn(move || {
                    let _g = guard;
                    if let Err(e) = serve(s, sched, reg, clients_for_serve, store_path_for_serve) {
                        warn!(error = ?e, "client session ended with error");
                    }
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) => {
                consecutive_err = consecutive_err.saturating_add(1);
                // Exponential backoff: 100ms, 200ms, 400ms, ..., capped at
                // ACCEPT_BACKOFF_MAX. Clamping shift exponent to <=8 keeps
                // the math well within u32 range without ever hitting the
                // 1 << 32 UB.
                let exp = consecutive_err.saturating_sub(1).min(8);
                let backoff =
                    ACCEPT_POLL_INTERVAL.saturating_mul(1u32 << exp).min(ACCEPT_BACKOFF_MAX);
                warn!(error = ?e, consecutive_err, backoff_ms = backoff.as_millis() as u64, "accept failed; backing off");
                thread::sleep(backoff);
            }
        }
    }
    info!("shutdown signal received, exiting accept loop");
    let _ = sampler.join();
    Ok(())
}

fn spawn_sampler(
    sched: Arc<Mutex<Scheduler>>,
    clients: ClientMap,
    shutdown: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            // One scheduler owner avoids per-client pumps racing to advance
            // global due times. Use the protocol minimum so clients that ask
            // for lower latency are not capped by the shared sampler.
            thread::sleep(Duration::from_millis(linsight_protocol::PUMP_INTERVAL_MIN_MS as u64));
            let samples = {
                let mut s = sched.lock().unwrap();
                s.tick(now_micros())
            };
            if samples.is_empty() {
                continue;
            }

            let mut map = clients.lock().unwrap();
            map.retain(|_id, sink| {
                let subscriptions = sink.subscriptions.lock().unwrap();
                let interested: Vec<_> = samples
                    .iter()
                    .filter(|sample| subscriptions.contains_key(&sample.sensor))
                    .cloned()
                    .collect();
                drop(subscriptions);

                for sample in interested {
                    if sink.tx.send(ServerMsg::Sample(sample)).is_err() {
                        return false;
                    }
                }
                true
            });
        }
    })
}

fn now_micros() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_micros() as u64,
        Err(e) => {
            warn!(error = ?e, "system clock before UNIX_EPOCH; emitting sentinel timestamp");
            TS_SENTINEL_BAD_CLOCK
        }
    }
}

/// Build the wire-shape `SensorInfo` catalogue from the scheduler's
/// descriptors and the hardware registry's decoration data. Shared by
/// the `ListSensors` handler and the `SensorListBroadcast` emitted
/// on a nickname change, so the two paths can never disagree about
/// what a "current catalogue" looks like.
fn build_sensor_info_list(
    sched: &Scheduler,
    registry: &HardwareRegistry,
) -> Vec<linsight_protocol::SensorInfo> {
    sched
        .descriptors()
        .map(|d| {
            let plugin_id = sched.plugin_id_for(&d.id).unwrap_or("unknown").to_owned();
            // Prefer the descriptor's explicit `device_key` (v4 plugins
            // set this directly and host_init validated it lives in
            // `manifest.devices`). Fall back to the `(plugin_id,
            // device_id)` lookup for legacy / partial v4 manifests
            // that bind via device_id only. Memory sensors carry
            // neither and emit `device_key = None`.
            let device_key = d.device_key.clone().or_else(|| {
                d.device_id.as_ref().and_then(|did| registry.key_for(&plugin_id, did)).cloned()
            });
            let device_label = device_key.as_ref().map(|k| registry.device_label_for(k));
            linsight_protocol::SensorInfo {
                id: d.id.clone(),
                display_name: d.display_name.clone(),
                unit: d.unit.clone(),
                kind: d.kind,
                category: d.category,
                native_rate_hz: d.native_rate_hz,
                min: d.min,
                max: d.max,
                device_id: d.device_id.clone(),
                plugin_id,
                device_key: device_key.map(|k| k.as_str().to_owned()),
                device_label,
                tags: d.tags.clone(),
            }
        })
        .collect()
}

fn serve(
    stream: UnixStream,
    sched: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
    clients: ClientMap,
    store_path: PathBuf,
) -> Result<(), FrameError> {
    let peer = stream.peer_addr().ok();
    info!(?peer, "client connected");
    // Bound the time a quiet client can hold a worker thread before sending
    // Hello. We clear this once the handshake completes so steady-state
    // reads can block indefinitely waiting for client commands. The
    // `read_clone` keeps a handle to the read end so we can re-tune the
    // timeout after handshake without poking at `FrameReader` internals.
    let read_clone = stream.try_clone().map_err(FrameError::Io)?;
    read_clone.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(FrameError::Io)?;
    let mut reader = FrameReader::new(read_clone.try_clone().map_err(FrameError::Io)?);
    let writer = Arc::new(Mutex::new(FrameWriter::new(stream)));

    // 1) Handshake.
    let first = reader.read_client()?;
    let (client_name, auth_token) = match verify_hello(&first) {
        Ok((name, token)) => (name.to_string(), token.map(|s| s.to_owned())),
        Err(e) => {
            let _ = writer
                .lock()
                .unwrap()
                .write_server(&ServerMsg::Bye { reason: format!("handshake failed: {e}") });
            return Ok(());
        }
    };

    // Optional auth check: if LINSIGHT_AUTH_TOKEN is set, verify token match.
    if let Some(ref expected) = *AUTH_TOKEN
        && !auth_token.as_deref().is_some_and(|t| t.as_bytes().ct_eq(expected.as_bytes()).into())
    {
        let _ = writer
            .lock()
            .unwrap()
            .write_server(&ServerMsg::Bye { reason: "authentication failed".into() });
        return Ok(());
    }
    info!(client = %client_name, "client said hello");

    // Build the Welcome plugin list from the real scheduler state instead of
    // a hardcoded stub. Drops the lock before the `write_server` so a slow
    // client can't block sample-pumping.
    let plugins: Vec<PluginInfo> = {
        let s = sched.lock().unwrap();
        s.plugins()
            .map(|(meta, sensor_count)| PluginInfo {
                plugin_id: meta.plugin_id.clone(),
                display_name: meta.display_name.clone(),
                version: meta.version.clone(),
                sensor_count,
            })
            .collect()
    };
    writer.lock().unwrap().write_server(&ServerMsg::Welcome {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").into(),
        plugins,
    })?;

    // Clear the handshake timeout so subscribed clients can wait quietly for
    // pushed samples without being kicked. Same FD as the reader (it's a
    // clone of the same socket), so the kernel-side timeout applies.
    read_clone.set_read_timeout(None).map_err(FrameError::Io)?;

    // Register ourselves as an outbound target so the shared sampler and
    // SetNickname RPCs can push messages for this client. The receiver is
    // drained by the pump thread below; the per-client subscription ledger
    // lets fan-out filter samples without teaching the scheduler about
    // connection identities.
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let subscriptions = Arc::new(Mutex::new(ClientSubscriptions::new()));
    let (outbound_tx, outbound_rx) = std::sync::mpsc::channel::<ServerMsg>();
    clients.lock().unwrap().insert(
        client_id,
        ClientSink { tx: outbound_tx, subscriptions: Arc::clone(&subscriptions) },
    );

    // Per-client pump-thread tick, in ms. Defaults to
    // `PUMP_INTERVAL_DEFAULT_MS` (150 ms); the client can adjust via
    // `RequestOp::SetPumpIntervalMs`. Stored as AtomicU64 because the
    // owning thread (the read loop) writes while the pump thread reads
    // each iteration — atomic is the cheapest correct way to share.
    let pump_interval_ms =
        Arc::new(AtomicU64::new(linsight_protocol::PUMP_INTERVAL_DEFAULT_MS as u64));

    // 2) Outbound-pumping thread.
    let pump_writer = Arc::clone(&writer);
    let pump_interval_for_pump = Arc::clone(&pump_interval_ms);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let pump = thread::spawn(move || {
        loop {
            // Re-read every iteration so a `SetPumpIntervalMs` request
            // takes effect on the next tick without restarting the
            // thread. Relaxed ordering is fine: we just need to
            // eventually see the new value, never any happens-before
            // relation against another variable.
            let tick = Duration::from_millis(pump_interval_for_pump.load(Ordering::Relaxed));
            if stop_rx.recv_timeout(tick).is_ok() {
                break;
            }
            // Drain any pending outbound messages. The channel is unbounded;
            // `try_recv` lets us pull multiple messages per tick
            // without blocking the sample drain when none are queued.
            loop {
                match outbound_rx.try_recv() {
                    Ok(msg) => {
                        let mut w = pump_writer.lock().unwrap();
                        if w.write_server(&msg).is_err() {
                            return;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }
    });

    // 3) Read loop.
    let result = (|| -> Result<(), FrameError> {
        loop {
            let msg = reader.read_client()?;
            match msg {
                ClientMsg::ListSensors => {
                    let infos = {
                        let s = sched.lock().unwrap();
                        let r = registry.read().unwrap();
                        build_sensor_info_list(&s, &r)
                    };
                    writer.lock().unwrap().write_server(&ServerMsg::SensorList(infos))?;
                }
                ClientMsg::Subscribe { sensors, rate_hz } => {
                    let mut degraded = Vec::new();
                    {
                        let mut s = sched.lock().unwrap();
                        let mut local = subscriptions.lock().unwrap();
                        for id in &sensors {
                            match s.subscribe(id, rate_hz) {
                                Ok(subscription) => {
                                    local.entry(id.clone()).or_default().push(subscription);
                                }
                                Err(e) => {
                                    warn!(error = ?e, "subscribe rejected");
                                    degraded.push((id.clone(), e.to_string()));
                                }
                            }
                        }
                    }
                    // Tell the client about rejected subscribes so it can
                    // surface a real error instead of waiting forever for
                    // samples that will never arrive.
                    for (sensor, reason) in degraded {
                        let mut w = writer.lock().unwrap();
                        w.write_server(&ServerMsg::SensorDegraded { sensor, reason })?;
                    }
                }
                ClientMsg::Unsubscribe { sensors } => {
                    let mut removed = Vec::new();
                    {
                        let mut local = subscriptions.lock().unwrap();
                        for id in &sensors {
                            if let Some(entries) = local.get_mut(id) {
                                if let Some(subscription) = entries.pop() {
                                    removed.push(subscription);
                                }
                                if entries.is_empty() {
                                    local.remove(id);
                                }
                            }
                        }
                    }
                    if !removed.is_empty() {
                        let mut s = sched.lock().unwrap();
                        for subscription in &removed {
                            s.unsubscribe(subscription);
                        }
                    }
                }
                ClientMsg::Hello { .. } => {
                    let _ = writer
                        .lock()
                        .unwrap()
                        .write_server(&ServerMsg::Bye { reason: "duplicate Hello".into() });
                    return Ok(());
                }
                ClientMsg::Goodbye => return Ok(()),
                ClientMsg::Request { req_id, op } => {
                    handle_request(
                        req_id,
                        op,
                        &sched,
                        &registry,
                        &clients,
                        &store_path,
                        &writer,
                        &pump_interval_ms,
                    )?;
                }
            }
        }
    })();

    let _ = stop_tx.send(());
    let _ = pump.join();
    clients.lock().unwrap().remove(&client_id);
    let removed: Vec<_> = {
        let mut local = subscriptions.lock().unwrap();
        local.drain().flat_map(|(_sensor, entries)| entries).collect()
    };
    if !removed.is_empty() {
        let mut s = sched.lock().unwrap();
        for subscription in &removed {
            s.unsubscribe(subscription);
        }
    }
    result
}

/// Per-request dispatch for the v2 `ClientMsg::Request` envelope. Kept
/// out of `serve` so the read loop stays readable and so each branch
/// can produce its own `Response` without indenting deeper than is
/// already painful.
// 8 args because every request handler needs access to the same shared
// daemon state (scheduler, registry, clients, store path, writer,
// per-client pump interval). Splitting them into a bag struct just to
// satisfy clippy would add a layer of indirection without making the
// call site any clearer.
#[allow(clippy::too_many_arguments)]
fn handle_request(
    req_id: u32,
    op: linsight_protocol::RequestOp,
    sched: &Arc<Mutex<Scheduler>>,
    registry: &Arc<RwLock<HardwareRegistry>>,
    clients: &ClientMap,
    store_path: &std::path::Path,
    writer: &Arc<Mutex<FrameWriter<UnixStream>>>,
    pump_interval_ms: &Arc<AtomicU64>,
) -> Result<(), FrameError> {
    use linsight_core::HardwareDeviceKey;
    use linsight_protocol::{ProtoError, ProtoErrorCode, RequestOp, ResponsePayload};

    use crate::nickname_store::NicknameStore;

    match op {
        RequestOp::GetHardware => {
            let (devices, nicknames) = {
                let reg = registry.read().unwrap();
                (reg.snapshot(), reg.nicknames_snapshot())
            };
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::Hardware { devices, nicknames }),
            })
        }
        RequestOp::SetNickname { device_key, value } => {
            // Parse the key BEFORE touching the registry so we can
            // map a malformed key to InvalidNickname (closest existing
            // code; UnknownDevice would lie about what's wrong).
            let key = match HardwareDeviceKey::try_new(device_key.clone()) {
                Ok(k) => k,
                Err(e) => {
                    return writer.lock().unwrap().write_server(&ServerMsg::Response {
                        req_id,
                        result: Err(ProtoError {
                            code: ProtoErrorCode::InvalidNickname,
                            message: format!("bad device key: {e}"),
                        }),
                    });
                }
            };

            // Re-validate the value. The GUI is the primary caller
            // and will normalize before sending, but a CLI/tunnel
            // peer could skip the check; defense in depth.
            let normalized =
                match value.as_deref().map(linsight_core::validate_nickname).transpose() {
                    Ok(opt) => opt.flatten(),
                    Err(e) => {
                        return writer.lock().unwrap().write_server(&ServerMsg::Response {
                            req_id,
                            result: Err(ProtoError {
                                code: ProtoErrorCode::InvalidNickname,
                                message: e.to_string(),
                            }),
                        });
                    }
                };

            // Apply to the in-memory registry first; if the device
            // is unknown we fail fast without touching disk. The
            // write guard is scoped tightly so a slow client socket
            // (the write_server below) can't stall ListSensors /
            // GetHardware on other clients waiting for the read lock.
            let set_err = {
                let mut reg = registry.write().unwrap();
                reg.set_nickname(&key, normalized.clone()).err()
            };
            if let Some(e) = set_err {
                return writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::UnknownDevice,
                        message: e.to_string(),
                    }),
                });
            }

            // Persist BEFORE broadcasting. On save failure the
            // in-memory state is still updated but the next daemon
            // restart will lose the change — we surface the I/O
            // error so the client can decide whether to retry.
            let store = NicknameStore {
                schema_version: 1,
                nicknames: registry.read().unwrap().nicknames_snapshot(),
            };
            if let Err(e) = store.save(store_path) {
                tracing::error!(error = ?e, path = %store_path.display(), "hardware.json save failed");
                return writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::Io,
                        message: format!("save failed: {e}"),
                    }),
                });
            }

            // Confirm to the caller with the normalized value (so
            // they see exactly what we persisted, not what they
            // typed).
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::NicknameSet {
                    device_key: device_key.clone(),
                    value: normalized,
                }),
            })?;

            // Broadcast the updated catalogue to every connected
            // client (including the caller — the GUI relies on
            // this rather than treating its own Response as the
            // refresh trigger, so its SensorListModel and its
            // Hardware page stay in sync via the same path).
            let infos = {
                let s = sched.lock().unwrap();
                let r = registry.read().unwrap();
                build_sensor_info_list(&s, &r)
            };
            broadcast_sensor_list(clients, infos);
            Ok(())
        }
        RequestOp::SetPumpIntervalMs { ms } => {
            // Clamp to the documented protocol range before storing.
            // A client asking for 0 ms (busy loop) or u32::MAX (no
            // wakeups ever) lands on the nearest allowed value; the
            // response echoes what we actually applied.
            let clamped = ms.clamp(
                linsight_protocol::PUMP_INTERVAL_MIN_MS,
                linsight_protocol::PUMP_INTERVAL_MAX_MS,
            );
            pump_interval_ms.store(clamped as u64, Ordering::Relaxed);
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::PumpIntervalSet { ms: clamped }),
            })
        }
        RequestOp::GetHistory { sensor: s, since_micros, until_micros, max_points } => {
            // Query the history database when available.
            match sched.lock().unwrap().history_db_path() {
                Some(db_path) => {
                    match crate::history::query(
                        db_path,
                        &s,
                        since_micros as i64,
                        until_micros as i64,
                        max_points,
                    ) {
                        Ok(samples) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                            req_id,
                            result: Ok(ResponsePayload::History { sensor: s.clone(), samples }),
                        }),
                        Err(e) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                            req_id,
                            result: Err(ProtoError {
                                code: ProtoErrorCode::Io,
                                message: format!("history query failed: {e}"),
                            }),
                        }),
                    }
                }
                None => writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::Internal,
                        message: "history not enabled on daemon".into(),
                    }),
                }),
            }
        }
        RequestOp::ListAlerts => {
            // Read-live from the alert engine when available.
            let rules = sched
                .lock()
                .unwrap()
                .alert_engine_handle()
                .map(|h| h.list_rules_json())
                .unwrap_or_default();
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::AlertList { rules }),
            })
        }
        RequestOp::UpsertAlert { name, expr, for_duration, cooldown, notify, enabled } => {
            let sched_guard = sched.lock().unwrap();
            if let Some(handle) = sched_guard.alert_engine_handle() {
                if let Some(cfg_path) = sched_guard.alerts_config_path().map(|p| p.to_path_buf()) {
                    match handle.upsert_rule(
                        &name,
                        &expr,
                        for_duration.as_deref(),
                        cooldown.as_deref(),
                        notify,
                        enabled,
                    ) {
                        Ok(()) => {
                            if let Err(e) = handle.save_config(&cfg_path) {
                                tracing::warn!(error = ?e, "alert config save failed");
                            }
                            writer.lock().unwrap().write_server(&ServerMsg::Response {
                                req_id,
                                result: Ok(ResponsePayload::AlertUpserted { name }),
                            })
                        }
                        Err(e) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                            req_id,
                            result: Err(ProtoError {
                                code: ProtoErrorCode::Internal,
                                message: format!("upsert failed: {e}"),
                            }),
                        }),
                    }
                } else {
                    writer.lock().unwrap().write_server(&ServerMsg::Response {
                        req_id,
                        result: Err(ProtoError {
                            code: ProtoErrorCode::Internal,
                            message: "alerts not configured".into(),
                        }),
                    })
                }
            } else {
                writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::Internal,
                        message: "alert engine not loaded".into(),
                    }),
                })
            }
        }
        RequestOp::DeleteAlert { name } => {
            let sched_guard = sched.lock().unwrap();
            if let Some(handle) = sched_guard.alert_engine_handle() {
                if let Some(cfg_path) = sched_guard.alerts_config_path().map(|p| p.to_path_buf()) {
                    match handle.delete_rule(&name) {
                        Ok(true) => {
                            if let Err(e) = handle.save_config(&cfg_path) {
                                tracing::warn!(error = ?e, "alert config save failed");
                            }
                            writer.lock().unwrap().write_server(&ServerMsg::Response {
                                req_id,
                                result: Ok(ResponsePayload::AlertDeleted { name }),
                            })
                        }
                        Ok(false) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                            req_id,
                            result: Err(ProtoError {
                                code: ProtoErrorCode::AlertNotFound,
                                message: format!("rule '{name}' not found"),
                            }),
                        }),
                        Err(e) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                            req_id,
                            result: Err(ProtoError {
                                code: ProtoErrorCode::Internal,
                                message: format!("delete failed: {e}"),
                            }),
                        }),
                    }
                } else {
                    writer.lock().unwrap().write_server(&ServerMsg::Response {
                        req_id,
                        result: Err(ProtoError {
                            code: ProtoErrorCode::Internal,
                            message: "alerts not configured".into(),
                        }),
                    })
                }
            } else {
                writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::Internal,
                        message: "alert engine not loaded".into(),
                    }),
                })
            }
        }
        RequestOp::TestAlertExpr { expr } => {
            use crate::alerts::{EvalOutcome, eval_limited};
            use evalexpr::{ContextWithMutableVariables, HashMapContext};
            let mut ctx = HashMapContext::new();
            // Populate context from all current sensor values in the
            // scheduler. Substitute `.` -> `__` for evalexpr compatibility.
            {
                let s = sched.lock().unwrap();
                for d in s.descriptors() {
                    if let Some(sample) = s.sample_now(&d.id, now_micros()) {
                        let var_name = d.id.as_str().replace('.', "__");
                        if let linsight_core::Reading::Scalar(v) = sample.reading {
                            let _ = ctx.set_value(var_name, evalexpr::Value::Float(v));
                        }
                    }
                }
            }
            let rewritten = expr.replace('.', "__");
            match eval_limited(&rewritten, &ctx) {
                EvalOutcome::Ok(is_true) => {
                    writer.lock().unwrap().write_server(&ServerMsg::Response {
                        req_id,
                        result: Ok(ResponsePayload::AlertTestResult { is_true, error: None }),
                    })
                }
                EvalOutcome::Err(e) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Ok(ResponsePayload::AlertTestResult {
                        is_true: false,
                        error: Some(format!("eval failed: {e}")),
                    }),
                }),
                EvalOutcome::Timeout => writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Ok(ResponsePayload::AlertTestResult {
                        is_true: false,
                        error: Some("expression evaluation timed out".into()),
                    }),
                }),
                EvalOutcome::Panic => writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Ok(ResponsePayload::AlertTestResult {
                        is_true: false,
                        error: Some("expression evaluation panicked".into()),
                    }),
                }),
            }
        }
        RequestOp::ListAlertEvents { limit } => {
            let events_json = sched
                .lock()
                .unwrap()
                .alert_engine_handle()
                .map(|h| h.list_events_json(limit))
                .unwrap_or_else(|| "[]".to_owned());
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::AlertEventList { events_json }),
            })
        }
        RequestOp::GetDaemonSettings => {
            let (history, alerts, _prom, prom_bind) = sched.lock().unwrap().daemon_settings();
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::DaemonSettings {
                    history_enabled: history,
                    alerts_enabled: alerts,
                    prom_enabled: prom_bind.is_some(),
                    prom_bind,
                }),
            })
        }
        RequestOp::SetDaemonSettings { history, alerts, prom } => {
            let mut s = sched.lock().unwrap();
            let mut errors: Vec<String> = Vec::new();
            if let Some(v) = history
                && let Err(e) = s.toggle_history(v)
            {
                errors.push(e);
            }
            if let Some(v) = alerts
                && let Err(e) = s.toggle_alerts(v)
            {
                errors.push(e);
            }
            if let Some(v) = prom
                && v
            {
                errors.push(
                    "Prometheus toggle-on is not supported via RPC; set LINSIGHT_PROM_BIND and restart".into(),
                );
            }
            if let Some(v) = prom
                && !v
            {
                errors.push(
                    "Prometheus toggle-off is not supported via RPC; unset LINSIGHT_PROM_BIND and restart".into(),
                );
            }
            let (history_enabled, alerts_enabled, _prom, prom_bind) = s.daemon_settings();
            drop(s);
            if !errors.is_empty() {
                return writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::Internal,
                        message: errors.join("; "),
                    }),
                });
            }
            writer.lock().unwrap().write_server(&ServerMsg::Response {
                req_id,
                result: Ok(ResponsePayload::DaemonSettingsSet {
                    history_enabled,
                    alerts_enabled,
                    prom_enabled: prom_bind.is_some(),
                }),
            })
        }
    }
}

/// Push a `SensorListBroadcast` to every connected client. The
/// per-client `serve()` thread deregisters its own entry on exit, so
/// the map is normally already free of dead senders by the time we
/// run; the `retain` here is a defensive second pass that handles the
/// unlikely window where a peer is in the process of disconnecting
/// (its pump dropped the receiver but its `serve()` hasn't yet
/// reached the `remove` call).
fn broadcast_sensor_list(clients: &ClientMap, infos: Vec<linsight_protocol::SensorInfo>) {
    let msg = ServerMsg::SensorListBroadcast(infos);
    let mut map = clients.lock().unwrap();
    map.retain(|_id, sink| sink.tx.send(msg.clone()).is_ok());
}
