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
    ClientMsg, FrameError, FrameReader, FrameWriter, PROTOCOL_VERSION, PluginInfo, ProtoError,
    ProtoErrorCode, RequestOp, ResponsePayload, ServerMsg, verify_hello,
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

/// Idle read timeout after a successful handshake. A client that has not sent
/// any command in this window is treated as stale / half-open and disconnected
/// so it cannot hold a session forever.
const CLIENT_IDLE_READ_TIMEOUT: Duration = Duration::from_secs(1800);

/// Per-message write timeout. A slow or frozen client cannot be allowed to
/// block the outbound pump thread indefinitely.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of concurrently-served client sessions. Excess connections
/// receive a `Bye` and are closed immediately so the scheduler mutex's
/// contention stays bounded and the daemon can't be DoS'd by a connection
/// burst. 64 leaves room for typical GUI + CLI usage plus several Prom/tunnel
/// peers while staying well under the typical 1024 fd soft limit.
const MAX_CLIENT_SESSIONS: usize = 64;

/// Per-client outbound queue capacity. A slow client (e.g. a GUI whose event
/// loop is backlogged) cannot be allowed to push an unbounded number of
/// `ServerMsg`s into memory. 1024 samples is ~150 s of backlog at the default
/// 150 ms tick with a single subscription, or several seconds with many
/// subscriptions — long enough to survive transient stalls, small enough to
/// cap RSS growth. When full, samples are dropped rather than disconnecting,
/// so a laggy client recovers live data once it catches up.
const OUTBOUND_QUEUE_CAP: usize = 1024;
/// Maximum outbound messages the pump thread drains per tick. Capping
/// this prevents a slow client from monopolizing the shared writer mutex
/// (which the read loop also needs for RPC responses). At 256 messages
/// per 150 ms tick, throughput is ~1 700 msg/s — far above any realistic
/// sample rate — while still yielding the lock between batches.
const PUMP_DRAIN_CAP: u32 = 256;

/// Sentinel returned by [`now_micros`] when the system clock is before
/// `UNIX_EPOCH`. Distinguishable from real timestamps for downstream code.
const TS_SENTINEL_BAD_CLOCK: u64 = u64::MAX;

/// How long the sampler waits for a single batch of due sensors before
/// marking the remaining in-flight samples as timed out. Chosen so a single
/// hung sensor cannot stall the whole tick indefinitely while still allowing
/// slow-but-finite operations (large D-Bus round trips, NVML init) to finish.
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Number of worker threads in the sampler pool. Caps the number of
/// concurrently-running plugin samples so repeated timeouts cannot grow
/// worker threads without bound.
const SAMPLER_POOL_SIZE: usize = 8;

/// Per-client subscription tokens, keyed by `(sensor, period)` so a duplicate
/// `Subscribe` for the same sensor and rate is idempotent instead of leaking
/// a second `Subscription` and inflating the scheduler's refcount.
type ClientSubscriptions = HashMap<(SensorId, u64), Subscription>;

/// Internal outbound message. Samples and catalogue broadcasts are shared
/// via `Arc` so each client pump receives a pointer copy instead of a deep
/// clone of the payload.
enum OutboundMsg {
    Sample(Arc<linsight_core::Sample>),
    SensorListBroadcast(Arc<Vec<linsight_protocol::SensorInfo>>),
}

struct ClientSink {
    tx: std::sync::mpsc::SyncSender<OutboundMsg>,
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

/// Constant-time auth-token comparison. `expected = None` means no token
/// is required and any provided token is accepted.
fn auth_token_ok(expected: Option<&str>, provided: Option<&str>) -> bool {
    match (expected, provided) {
        (Some(expected), Some(provided)) => provided.as_bytes().ct_eq(expected.as_bytes()).into(),
        (Some(_), None) => false,
        (None, _) => true,
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
    while !shutdown.load(Ordering::Relaxed) {
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
                let sessions_for_guard = Arc::clone(&sessions);
                if let Err(e) =
                    std::thread::Builder::new().name("linsight-client".into()).spawn(move || {
                        let _guard = SessionGuard(sessions_for_guard);
                        if let Err(e) =
                            serve(s, sched, reg, clients_for_serve, store_path_for_serve)
                        {
                            warn!(error = ?e, "client session ended with error");
                        }
                    })
                {
                    // Spawn failed: the closure was dropped without creating
                    // the guard, so decrement the session counter here to
                    // prevent a leak.
                    sessions.fetch_sub(1, Ordering::AcqRel);
                    warn!(error = ?e, "failed to spawn client session thread");
                }
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

/// Bounded pool of worker threads that run individual plugin samples.
/// Isolates slow or hung sensors so one blocked plugin cannot stall the
/// entire scheduler tick, while capping the number of in-flight sampling
/// threads so repeated timeouts cannot grow without bound.
struct SamplingPool {
    job_tx: Option<std::sync::mpsc::Sender<SamplingJob>>,
    result_rx: std::sync::mpsc::Receiver<SamplingResult>,
    handles: Vec<std::thread::JoinHandle<()>>,
    next_call_id: AtomicU64,
}

struct SamplingJob {
    host: Arc<crate::plugin_host::PluginHost>,
    id: SensorId,
    ts_micros: u64,
    call_id: u64,
}

struct SamplingResult {
    call_id: u64,
    sensor: SensorId,
    result: Result<linsight_core::Sample, linsight_plugin_sdk::PluginError>,
}

impl SamplingPool {
    fn new(size: usize) -> Self {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<SamplingJob>();
        let job_rx = Arc::new(std::sync::Mutex::new(job_rx));
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let mut handles = Vec::with_capacity(size);
        for i in 0..size {
            let rx = Arc::clone(&job_rx);
            let tx = result_tx.clone();
            let name = format!("linsight-sampler-{i}");
            match std::thread::Builder::new().name(name).spawn(move || {
                loop {
                    let job = {
                        let rx = rx.lock().unwrap();
                        match rx.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    };
                    let result = job.host.sample_to(&job.id, job.ts_micros);
                    let _ =
                        tx.send(SamplingResult { call_id: job.call_id, sensor: job.id, result });
                }
            }) {
                Ok(handle) => handles.push(handle),
                Err(e) => {
                    tracing::error!(error = ?e, worker_index = i, "failed to spawn sampler worker");
                }
            }
        }
        Self { job_tx: Some(job_tx), result_rx, handles, next_call_id: AtomicU64::new(0) }
    }

    fn sample(
        &self,
        host: Arc<crate::plugin_host::PluginHost>,
        plan: Vec<crate::scheduler::TickItem>,
        now: u64,
        timeout: Duration,
    ) -> Vec<(
        crate::scheduler::TickItem,
        Result<linsight_core::Sample, linsight_plugin_sdk::PluginError>,
    )> {
        let call_id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        let mut results = Vec::with_capacity(plan.len());
        let mut in_flight: HashMap<SensorId, crate::scheduler::TickItem> =
            HashMap::with_capacity(plan.len().min(self.handles.len().max(1)));
        let mut pending: std::collections::VecDeque<crate::scheduler::TickItem> = plan.into();
        let job_tx = self.job_tx.as_ref().expect("sampling pool is shut down");
        let deadline = std::time::Instant::now() + timeout;

        // Seed the pool with up to pool_size jobs. Workers pull more as
        // results arrive, so the number of concurrently-running samples is
        // always bounded by the pool size.
        for _ in 0..self.handles.len().max(1) {
            let Some(item) = pending.pop_front() else { break };
            if job_tx
                .send(SamplingJob {
                    host: Arc::clone(&host),
                    id: item.id.clone(),
                    ts_micros: now,
                    call_id,
                })
                .is_ok()
            {
                in_flight.insert(item.id.clone(), item);
            } else {
                results.push((
                    item,
                    Err(linsight_plugin_sdk::PluginError::Timeout(
                        "sampling pool shut down".into(),
                    )),
                ));
            }
        }

        while !in_flight.is_empty() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.result_rx.recv_timeout(remaining) {
                Ok(SamplingResult { call_id: got_id, sensor, result }) => {
                    if got_id != call_id {
                        // Stale result from a previous tick; ignore.
                        continue;
                    }
                    if let Some(item) = in_flight.remove(&sensor) {
                        results.push((item, result));
                        if let Some(next) = pending.pop_front() {
                            if job_tx
                                .send(SamplingJob {
                                    host: Arc::clone(&host),
                                    id: next.id.clone(),
                                    ts_micros: now,
                                    call_id,
                                })
                                .is_ok()
                            {
                                in_flight.insert(next.id.clone(), next);
                            } else {
                                results.push((
                                    next,
                                    Err(linsight_plugin_sdk::PluginError::Timeout(
                                        "sampling pool shut down".into(),
                                    )),
                                ));
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        for (_id, item) in in_flight.drain() {
            results.push((
                item,
                Err(linsight_plugin_sdk::PluginError::Timeout(format!(
                    "sample did not complete within {timeout:?}"
                ))),
            ));
        }
        // Pending jobs never started this tick; leave them for their next
        // scheduled due time instead of marking them degraded.
        results
    }
}

impl Drop for SamplingPool {
    fn drop(&mut self) {
        // Close the job channel so workers unblock from recv() and exit.
        drop(self.job_tx.take());
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn spawn_sampler(
    sched: Arc<Mutex<Scheduler>>,
    clients: ClientMap,
    shutdown: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let pool = SamplingPool::new(SAMPLER_POOL_SIZE);
        while !shutdown.load(Ordering::Relaxed) {
            // One scheduler owner avoids per-client pumps racing to advance
            // global due times. Use the protocol minimum so clients that ask
            // for lower latency are not capped by the shared sampler.
            thread::sleep(Duration::from_millis(linsight_protocol::PUMP_INTERVAL_MIN_MS as u64));

            // Phase 1: plan under lock; phase 2: sample WITHOUT the lock so a
            // slow/hung plugin cannot block subscriptions, RPCs, or other
            // sensors. Phase 3: commit under lock.
            let now = now_micros();
            let (host, plan) = {
                let mut s = sched.lock().unwrap();
                (s.host(), s.tick_plan(now))
            };
            let results = pool.sample(host, plan, now, SAMPLE_TIMEOUT);
            let samples = {
                let mut s = sched.lock().unwrap();
                s.tick_commit(results, now)
            };
            if samples.is_empty() {
                continue;
            }

            // Snapshot client sinks under a brief lock, then fan out
            // without holding the global clients mutex so accept /
            // subscribe / RPC handlers are not blocked during per-client
            // delivery.
            let sinks: Vec<(
                u64,
                std::sync::mpsc::SyncSender<OutboundMsg>,
                Arc<Mutex<ClientSubscriptions>>,
            )> = {
                let map = clients.lock().unwrap();
                map.iter()
                    .map(|(id, sink)| (*id, sink.tx.clone(), Arc::clone(&sink.subscriptions)))
                    .collect()
            };
            let samples: Vec<Arc<linsight_core::Sample>> =
                samples.into_iter().map(Arc::new).collect();
            let mut disconnected: Vec<u64> = Vec::new();
            for (id, tx, subscriptions) in sinks {
                let interested: Vec<Arc<linsight_core::Sample>> = {
                    let subs = subscriptions.lock().unwrap();
                    samples
                        .iter()
                        .filter(|sample| {
                            subs.keys().any(|(sensor_id, _period)| sensor_id == &sample.sensor)
                        })
                        .cloned()
                        .collect()
                };
                for sample in interested {
                    match tx.try_send(OutboundMsg::Sample(sample)) {
                        Ok(()) => {}
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            tracing::debug!("outbound queue full; dropping sample for slow client");
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            disconnected.push(id);
                            break;
                        }
                    }
                }
            }
            if !disconnected.is_empty() {
                let mut map = clients.lock().unwrap();
                for id in &disconnected {
                    map.remove(id);
                }
            }
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

/// Build a single `SensorInfo` from a descriptor so `ListSensors`,
/// `SensorListBroadcast`, and `GetSensorInfo` share the same decoration
/// logic.
fn build_sensor_info(
    d: &linsight_plugin_sdk::SensorDescriptor,
    sched: &Scheduler,
    registry: &HardwareRegistry,
) -> linsight_protocol::SensorInfo {
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
    sched.descriptors().map(|d| build_sensor_info(d, sched, registry)).collect()
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
    // Hello. After handshake we switch to the idle read timeout and set a
    // per-message write timeout so a stuck peer cannot pin the pump thread
    // forever. The `read_clone` keeps a handle to the read end so we can
    // re-tune the timeout after handshake without poking at `FrameReader`
    // internals.
    let read_clone = stream.try_clone().map_err(FrameError::Io)?;
    read_clone.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(FrameError::Io)?;
    let mut reader = FrameReader::new(read_clone.try_clone().map_err(FrameError::Io)?);
    // The writer uses the original stream; set its write timeout before
    // wrapping it so RPC responses and pumped samples share the same bound.
    stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT)).map_err(FrameError::Io)?;
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
        && !auth_token_ok(Some(expected.as_str()), auth_token.as_deref())
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

    // Replace the handshake timeout with an idle read timeout so stale or
    // half-open clients are eventually evicted instead of holding a session
    // forever. Same FD as the reader (it's a clone of the same socket), so
    // the kernel-side timeout applies.
    read_clone.set_read_timeout(Some(CLIENT_IDLE_READ_TIMEOUT)).map_err(FrameError::Io)?;

    // Register ourselves as an outbound target so the shared sampler and
    // SetNickname RPCs can push messages for this client. The receiver is
    // drained by the pump thread below; the per-client subscription ledger
    // lets fan-out filter samples without teaching the scheduler about
    // connection identities.
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let subscriptions = Arc::new(Mutex::new(ClientSubscriptions::new()));
    let (outbound_tx, outbound_rx) =
        std::sync::mpsc::sync_channel::<OutboundMsg>(OUTBOUND_QUEUE_CAP);
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
            // Drain any pending outbound messages. The channel is bounded;
            // `try_recv` lets us pull multiple messages per tick
            // without blocking the sample drain when none are queued.
            // Cap the drain so a slow client can't monopolize the shared
            // writer lock — the read loop needs it to send RPC responses.
            let mut drained = 0u32;
            loop {
                match outbound_rx.try_recv() {
                    Ok(msg) => {
                        let server_msg = match msg {
                            OutboundMsg::Sample(s) => ServerMsg::Sample((*s).clone()),
                            OutboundMsg::SensorListBroadcast(infos) => {
                                ServerMsg::SensorListBroadcast((*infos).clone())
                            }
                        };
                        let mut w = pump_writer.lock().unwrap();
                        if w.write_server(&server_msg).is_err() {
                            return;
                        }
                        drained += 1;
                        if drained >= PUMP_DRAIN_CAP {
                            break;
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
                            // Idempotent subscribe: a client already holding
                            // this (sensor, period) token does not need a new
                            // scheduler refcount. This prevents duplicate GUI
                            // subscriptions from leaking memory.
                            let subscription = match s.subscribe(id, rate_hz) {
                                Ok(sub) => sub,
                                Err(e) => {
                                    warn!(error = ?e, "subscribe rejected");
                                    degraded.push((id.clone(), e.to_string()));
                                    continue;
                                }
                            };
                            let key = (id.clone(), subscription.period_micros());
                            match local.entry(key) {
                                std::collections::hash_map::Entry::Vacant(e) => {
                                    e.insert(subscription);
                                }
                                std::collections::hash_map::Entry::Occupied(_) => {
                                    // Duplicate: drop the scheduler-side refcount
                                    // we just acquired so it stays balanced.
                                    s.unsubscribe(&subscription);
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
                            // Remove every (sensor, period) token held by this
                            // client. `Unsubscribe` carries no rate, so the
                            // only sensible interpretation is "stop all".
                            let keys_to_remove: Vec<_> = local
                                .keys()
                                .filter(|(sensor_id, _period)| sensor_id == id)
                                .cloned()
                                .collect();
                            for key in keys_to_remove {
                                if let Some(subscription) = local.remove(&key) {
                                    removed.push(subscription);
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
        local.drain().map(|(_key, subscription)| subscription).collect()
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
        RequestOp::GetSensorInfo { sensor } => {
            let sensor_id = match linsight_core::SensorId::try_new(&sensor) {
                Ok(id) => id,
                Err(e) => {
                    return writer.lock().unwrap().write_server(&ServerMsg::Response {
                        req_id,
                        result: Err(ProtoError {
                            code: ProtoErrorCode::UnknownSensor,
                            message: format!("bad sensor id: {e}"),
                        }),
                    });
                }
            };
            let info = {
                let s = sched.lock().unwrap();
                let r = registry.read().unwrap();
                s.descriptors().find(|d| d.id == sensor_id).map(|d| build_sensor_info(d, &s, &r))
            };
            match info {
                Some(info) => writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Ok(ResponsePayload::SensorInfo { info }),
                }),
                None => writer.lock().unwrap().write_server(&ServerMsg::Response {
                    req_id,
                    result: Err(ProtoError {
                        code: ProtoErrorCode::UnknownSensor,
                        message: format!("sensor not found: {sensor_id}"),
                    }),
                }),
            }
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
        RequestOp::SetDaemonSettings { history, alerts, prom, prom_bind } => {
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
            drop(s);
            if let Some(bind) = prom_bind {
                if bind.trim().is_empty() {
                    unsafe { std::env::remove_var("LINSIGHT_PROM_BIND") };
                } else {
                    unsafe { std::env::set_var("LINSIGHT_PROM_BIND", bind) };
                }
            }
            let (history_enabled, alerts_enabled, _prom, prom_bind) =
                sched.lock().unwrap().daemon_settings();
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
    let infos = Arc::new(infos);
    let mut map = clients.lock().unwrap();
    map.retain(|_id, sink| {
        match sink.tx.try_send(OutboundMsg::SensorListBroadcast(Arc::clone(&infos))) {
            Ok(()) => true,
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                // Same backpressure policy as samples: drop rather than disconnect.
                tracing::debug!(
                    "outbound queue full; dropping sensor-list broadcast for slow client"
                );
                true
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use linsight_core::{Category, SensorId, SensorKind, Unit};

    use super::*;
    use crate::plugin_host::PluginHost;
    use crate::scheduler::{Scheduler, TickItem};

    #[test]
    fn session_guard_decrements_on_drop() {
        let count = Arc::new(AtomicUsize::new(5));
        let guard = SessionGuard(Arc::clone(&count));
        drop(guard);
        assert_eq!(count.load(Ordering::Acquire), 4);
    }

    #[test]
    fn auth_token_ok_with_no_required_token() {
        assert!(auth_token_ok(None, None));
        assert!(auth_token_ok(None, Some("anything")));
    }

    #[test]
    fn auth_token_ok_requires_exact_match() {
        assert!(auth_token_ok(Some("secret"), Some("secret")));
        assert!(!auth_token_ok(Some("secret"), Some("wrong")));
        assert!(!auth_token_ok(Some("secret"), None));
    }

    #[test]
    fn auth_token_ok_rejects_different_length() {
        // A different-length token must be rejected (ct_eq still returns
        // false for different lengths without short-circuiting).
        assert!(!auth_token_ok(Some("secret"), Some("secrets")));
        assert!(!auth_token_ok(Some("secret"), Some("secr")));
    }

    #[test]
    fn now_micros_is_monotonic_or_sentinel() {
        let a = now_micros();
        thread::sleep(Duration::from_millis(1));
        let b = now_micros();
        assert!(b >= a || a == TS_SENTINEL_BAD_CLOCK);
    }

    #[test]
    fn broadcast_sensor_list_reaches_subscribed_clients() {
        let clients: ClientMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let subscriptions = Arc::new(Mutex::new(ClientSubscriptions::new()));
        clients
            .lock()
            .unwrap()
            .insert(1, ClientSink { tx, subscriptions: Arc::clone(&subscriptions) });

        let info = linsight_protocol::SensorInfo {
            id: SensorId::new("cpu.util"),
            display_name: "CPU".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: None,
            plugin_id: "com.visorcraft.linsight.cpu".into(),
            device_key: None,
            device_label: None,
            tags: vec![],
        };
        broadcast_sensor_list(&clients, vec![info.clone()]);

        let msg = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(msg, OutboundMsg::SensorListBroadcast(list) if list.len() == 1));
    }

    #[test]
    fn broadcast_sensor_list_drops_disconnected_clients() {
        let clients: ClientMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = std::sync::mpsc::sync_channel::<OutboundMsg>(16);
        drop(rx);
        let subscriptions = Arc::new(Mutex::new(ClientSubscriptions::new()));
        clients
            .lock()
            .unwrap()
            .insert(1, ClientSink { tx, subscriptions: Arc::clone(&subscriptions) });

        broadcast_sensor_list(&clients, vec![]);
        assert!(clients.lock().unwrap().is_empty(), "dead receiver should be removed");
    }

    #[test]
    fn subscribe_is_idempotent_per_client() {
        // Mirrors the `Subscribe` handler's bookkeeping to ensure a duplicate
        // (sensor, period) does not inflate the scheduler refcount.
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        let mut local = ClientSubscriptions::new();

        let first = sched.subscribe(&id, None).unwrap();
        let key = (id.clone(), first.period_micros());
        local.insert(key, first);

        let duplicate = sched.subscribe(&id, None).unwrap();
        let duplicate_period = duplicate.period_micros();
        let key = (id.clone(), duplicate_period);
        match local.entry(key) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(duplicate);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                sched.unsubscribe(&duplicate);
            }
        }

        assert_eq!(local.len(), 1, "duplicate subscribe should not add a second token");

        // Removing the single client token should drop the scheduler entry.
        let sub = local.remove(&(id.clone(), duplicate_period)).unwrap();
        sched.unsubscribe(&sub);
        assert!(sched.tick_plan(1_000_000).is_empty());
    }

    #[test]
    fn sampling_pool_samples_due_sensors() {
        let host = PluginHost::with_builtins();
        let pool = SamplingPool::new(2);
        let plan = vec![TickItem { id: SensorId::new("cpu.util") }];
        let results = pool.sample(Arc::new(host), plan, 1_000_000, Duration::from_secs(5));
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok(), "pool should produce a sample: {:?}", results[0].1);
    }
}
