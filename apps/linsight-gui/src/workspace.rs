// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use linsight_core::{Sample, SensorId};
use linsight_protocol::SensorInfo;

use crate::client::{Client, ClientHandle, RpcError};

type SampleBridge = (u64, Sample);
type CatalogueBridge = (u64, Vec<SensorInfo>);

/// Resolve `XDG_RUNTIME_DIR/linsight.sock` per the daemon's default.
pub fn default_socket_path() -> anyhow::Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|dir| dir.join("linsight.sock"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "$XDG_RUNTIME_DIR is not set; refusing to fall back to /tmp. \
                 Set XDG_RUNTIME_DIR or pass --socket explicitly."
            )
        })
}

/// Process-wide shared state for the GUI. Holds the daemon client and
/// maintains a stable sample-channel bridge so the `OverviewModel`'s
/// receiver survives client swaps (local ↔ remote host reconnect).
pub struct Workspace {
    client: Mutex<ClientHandle>,
    /// Bridge sender. The matching receiver is handed to `OverviewModel`
    /// once via `take_sample_rx`. Each underlying client forwards its
    /// samples into this sender; when a client is replaced, its forwarder
    /// exits and a new one is spawned for the next client.
    sample_tx: Sender<SampleBridge>,
    /// Receiver side of the bridge. `take()` returns `Some` exactly once.
    sample_rx: Mutex<Option<Receiver<SampleBridge>>>,
    /// Stable sender for catalogue broadcasts. A matching receiver is
    /// handed to `OverviewModel` once via `take_catalogue_rx`.
    catalogue_tx: Sender<CatalogueBridge>,
    catalogue_rx: Mutex<Option<Receiver<CatalogueBridge>>>,
    /// Sensors the GUI currently wants subscribed. Replayed against a
    /// new client after reconnect so tile streams resume automatically.
    subscriptions: Mutex<Vec<SensorId>>,
    /// Last pump-interval value successfully applied. Replayed on reconnect.
    pump_interval_ms: Mutex<u32>,
    /// The currently connected target (`"local"` or an `ssh://...` URL).
    /// Mirrored into `HostsModel.active_host` so the UI shows the right
    /// label after a CLI `--connect` launch or an in-app reconnect.
    active_target: Mutex<String>,
    /// Monotonically incremented on each reconnect so a forwarder from a
    /// defunct client does not clear the connection-alive flag of a newer
    /// connection.
    connection_generation: Arc<AtomicU64>,
    /// True while the current client is connected. The OverviewModel pump
    /// polls this to surface disconnect/reconnect state to QML.
    connection_alive: Arc<AtomicBool>,
    /// Serializes reconnect attempts so two overlapping reconnects cannot
    /// interleave generation bumps and client swaps.
    reconnect_lock: Mutex<()>,
}

impl Workspace {
    pub fn new(client: ClientHandle, initial_target: &str) -> anyhow::Result<Self> {
        let client_rx = client
            .take_sample_rx()
            .ok_or_else(|| anyhow::anyhow!("client sample receiver already taken"))?;
        let (sample_tx, sample_rx) = channel::<SampleBridge>();
        let (catalogue_tx, catalogue_rx) = channel::<CatalogueBridge>();
        let connection_generation = Arc::new(AtomicU64::new(1));
        let connection_alive = Arc::new(AtomicBool::new(true));
        spawn_sample_forwarder(
            client_rx,
            sample_tx.clone(),
            Arc::clone(&connection_generation),
            Arc::clone(&connection_alive),
            1,
        );
        spawn_catalogue_forwarder(client.subscribe_catalogue(), catalogue_tx.clone(), 1);

        Ok(Self {
            client: Mutex::new(client),
            sample_tx,
            sample_rx: Mutex::new(Some(sample_rx)),
            catalogue_tx,
            catalogue_rx: Mutex::new(Some(catalogue_rx)),
            subscriptions: Mutex::new(Vec::new()),
            pump_interval_ms: Mutex::new(linsight_protocol::PUMP_INTERVAL_DEFAULT_MS),
            active_target: Mutex::new(initial_target.to_string()),
            connection_generation,
            connection_alive,
            reconnect_lock: Mutex::new(()),
        })
    }

    /// Take the one-shot sample receiver that feeds every live tile. Each
    /// sample is tagged with the connection generation so the pump can drop
    /// stale values after a reconnect. Returns `None` if called more than
    /// once.
    pub fn take_sample_rx(&self) -> Option<Receiver<SampleBridge>> {
        self.sample_rx.lock().expect("sample_rx poisoned").take()
    }

    /// Take the one-shot catalogue-broadcast receiver used by the
    /// OverviewModel to refresh tile labels after nickname changes. Each
    /// broadcast is tagged with the connection generation so the refresh
    /// thread can drop stale values after a reconnect. Returns `None` if
    /// called more than once.
    pub fn take_catalogue_rx(&self) -> Option<Receiver<CatalogueBridge>> {
        self.catalogue_rx.lock().expect("catalogue_rx poisoned").take()
    }

    /// Snapshot of the daemon's last-known sensor catalogue from the current
    /// client.
    pub fn sensor_infos(&self) -> Vec<SensorInfo> {
        self.client().sensor_infos()
    }

    /// Snapshot of the current client. RPC QObjects use this for one-off
    /// request/response calls. The returned `Arc` may outlive a reconnect
    /// briefly; in-flight RPCs will simply time out on the old connection.
    pub fn client(&self) -> ClientHandle {
        Arc::clone(&*self.client.lock().expect("client poisoned"))
    }

    /// Shared flag indicating whether the current client connection is alive.
    /// The OverviewModel sample pump polls this to update `connected`.
    pub fn connection_alive(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.connection_alive)
    }

    /// Shared generation counter. Incremented on each reconnect so the
    /// OverviewModel can ignore samples/catalogue broadcasts from a
    /// connection that was replaced.
    pub fn connection_generation(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.connection_generation)
    }

    /// The currently connected target (`"local"` or the `ssh://...` URL).
    pub fn active_target(&self) -> String {
        self.active_target.lock().expect("active_target poisoned").clone()
    }

    /// Subscribe `sensors` on the current client and remember them for
    /// reconnect replay.
    pub fn subscribe(&self, sensors: Vec<SensorId>) -> anyhow::Result<()> {
        if sensors.is_empty() {
            return Ok(());
        }
        {
            let mut subs = self.subscriptions.lock().expect("subscriptions poisoned");
            for s in &sensors {
                if !subs.contains(s) {
                    subs.push(s.clone());
                }
            }
        }
        self.client().subscribe(sensors)
    }

    /// Unsubscribe `sensors` on the current client and drop them from the
    /// reconnect replay set.
    pub fn unsubscribe(&self, sensors: Vec<SensorId>) -> anyhow::Result<()> {
        if sensors.is_empty() {
            return Ok(());
        }
        {
            let mut subs = self.subscriptions.lock().expect("subscriptions poisoned");
            subs.retain(|s| !sensors.contains(s));
        }
        self.client().unsubscribe(sensors)
    }

    /// Apply the pump interval to the current client and remember it for
    /// reconnect replay.
    pub fn set_pump_interval_ms(&self, ms: u32, timeout: Duration) -> Result<u32, RpcError> {
        let applied = self.client().set_pump_interval_ms(ms, timeout)?;
        *self.pump_interval_ms.lock().expect("pump_interval poisoned") = applied;
        Ok(applied)
    }

    /// Reconnect to a different daemon. `target` is either `"local"` for the
    /// default Unix socket or `"ssh://[user@]host[:port]"`. On failure the
    /// existing client is kept and an error string is returned.
    #[allow(dead_code)]
    pub fn reconnect(&self, target: &str) -> Result<(), String> {
        let new_client = if target == "local" {
            let path = default_socket_path().map_err(|e| e.to_string())?;
            Client::connect_or_spawn(&path)
        } else if let Some(url) = target.strip_prefix("ssh://") {
            Client::connect_ssh(&format!("ssh://{url}"))
        } else {
            return Err(format!("unsupported reconnect target: {target}"));
        }
        .map_err(|e| e.to_string())?;

        self.reconnect_with_client(new_client, target)
    }

    fn reconnect_with_client(&self, new_client: ClientHandle, target: &str) -> Result<(), String> {
        // Serialize the whole reconnect so two overlapping attempts cannot
        // advance the generation and swap the client out of order.
        let _guard = self.reconnect_lock.lock().expect("reconnect_lock poisoned");

        let new_rx = new_client
            .take_sample_rx()
            .ok_or_else(|| "new client's sample receiver already taken".to_string())?;
        let new_cat_rx = new_client.subscribe_catalogue();

        // Compute the generation we will use if the replay succeeds, but do
        // not publish it yet. If the subscription replay fails and we return
        // early, the new client is dropped and the generation never becomes
        // current, so the old connection keeps working unchanged.
        let new_generation = self.connection_generation.load(Ordering::SeqCst).saturating_add(1);

        // Bridge the new client's catalogue into the same stable receiver
        // *before* replaying subscriptions. The replay may cause the daemon
        // to emit a SensorListBroadcast (e.g. nickname refresh), so the
        // catalogue forwarder must already be listening. Broadcasts sent
        // before the generation is published are dropped by the refresh
        // thread because their generation is not yet current.
        spawn_catalogue_forwarder(new_cat_rx, self.catalogue_tx.clone(), new_generation);

        // Apply stored state to the new client *before* swapping. If this
        // fails we return the error without publishing the generation, so
        // the old connection stays alive.
        let subs = self.subscriptions.lock().expect("subscriptions poisoned").clone();
        let pump_ms = *self.pump_interval_ms.lock().expect("pump_interval poisoned");
        if !subs.is_empty() {
            new_client.subscribe(subs).map_err(|e| e.to_string())?;
        }
        if pump_ms != linsight_protocol::PUMP_INTERVAL_DEFAULT_MS {
            new_client
                .set_pump_interval_ms(pump_ms, Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
        }

        // Commit the new connection: publish the generation, mark it alive,
        // and swap out the old client. Only *after* the generation is
        // published do we start forwarding samples; otherwise a sample from
        // the new client could advance the pump's current generation before
        // the replay succeeds, causing the old connection's samples to be
        // dropped if the replay later fails.
        self.connection_generation.store(new_generation, Ordering::SeqCst);
        self.connection_alive.store(true, Ordering::SeqCst);

        let old_client = {
            let mut guard = self.client.lock().expect("client poisoned");
            std::mem::replace(&mut *guard, new_client)
        };
        drop(old_client);

        spawn_sample_forwarder(
            new_rx,
            self.sample_tx.clone(),
            Arc::clone(&self.connection_generation),
            Arc::clone(&self.connection_alive),
            new_generation,
        );

        *self.active_target.lock().expect("active_target poisoned") = target.to_string();
        Ok(())
    }

    /// Test-only entry point that connects to an explicit socket path.
    #[cfg(test)]
    pub fn reconnect_to_path(&self, path: &std::path::Path) -> Result<(), String> {
        let new_client = Client::connect_or_spawn(path).map_err(|e| e.to_string())?;
        let target = path.to_string_lossy().to_string();
        self.reconnect_with_client(new_client, &target)
    }
}

fn spawn_sample_forwarder(
    client_rx: Receiver<Sample>,
    bridge_tx: Sender<SampleBridge>,
    generation: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    my_generation: u64,
) {
    thread::spawn(move || {
        while let Ok(s) = client_rx.recv() {
            if bridge_tx.send((my_generation, s)).is_err() {
                // The OverviewModel dropped its receiver (app exiting).
                break;
            }
        }
        // The client dispatch thread exited. Clear the alive flag only if
        // this forwarder still belongs to the current generation.
        if generation.load(Ordering::SeqCst) == my_generation {
            alive.store(false, Ordering::SeqCst);
        }
    });
}

fn spawn_catalogue_forwarder(
    client_rx: Receiver<Vec<SensorInfo>>,
    bridge_tx: Sender<CatalogueBridge>,
    my_generation: u64,
) {
    thread::spawn(move || {
        while let Ok(infos) = client_rx.recv() {
            if bridge_tx.send((my_generation, infos)).is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::{Category, Reading, SensorKind, Unit};
    use linsight_protocol::{
        ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, RequestOp, ResponsePayload,
        ServerMsg,
    };
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn fake_sensor(id: &str) -> linsight_protocol::SensorInfo {
        linsight_protocol::SensorInfo {
            id: SensorId::new(id),
            display_name: id.to_string(),
            unit: Unit::Count,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: None,
            plugin_id: "test".into(),
            device_key: None,
            device_label: None,
            tags: vec![],
        }
    }

    fn spawn_fake_daemon(
        listener: UnixListener,
        sensor_id: &'static str,
        disconnected: Arc<AtomicBool>,
    ) {
        thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut reader = FrameReader::new(stream.try_clone().expect("clone"));
            let mut writer = FrameWriter::new(stream);

            let Ok(ClientMsg::Hello { .. }) = reader.read_client() else {
                panic!("expected Hello");
            };
            writer
                .write_server(&ServerMsg::Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    plugins: vec![],
                })
                .expect("write welcome");

            let Ok(ClientMsg::ListSensors) = reader.read_client() else {
                panic!("expected ListSensors");
            };
            writer
                .write_server(&ServerMsg::SensorList(vec![fake_sensor(sensor_id)]))
                .expect("write sensor list");

            loop {
                match reader.read_client() {
                    Ok(ClientMsg::Subscribe { sensors, .. }) => {
                        writer
                            .write_server(&ServerMsg::SensorListBroadcast(vec![fake_sensor(
                                sensor_id,
                            )]))
                            .expect("write broadcast on subscribe");
                        for s in sensors {
                            writer
                                .write_server(&ServerMsg::Sample(Sample {
                                    sensor: s,
                                    ts_micros: 1,
                                    reading: Reading::Scalar(42.0),
                                }))
                                .expect("write sample");
                        }
                    }
                    Ok(ClientMsg::Request { req_id, op: RequestOp::SetPumpIntervalMs { ms } }) => {
                        writer
                            .write_server(&ServerMsg::Response {
                                req_id,
                                result: Ok(ResponsePayload::PumpIntervalSet { ms }),
                            })
                            .expect("write pump interval response");
                    }
                    Ok(ClientMsg::Goodbye) | Err(_) => {
                        disconnected.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(_) => {}
                }
            }
        });
    }

    /// A fake daemon that accepts a connection and handshake, then closes the
    /// socket as soon as it receives a Subscribe. Used to verify that a failed
    /// reconnect leaves the previous connection intact.
    fn spawn_flaky_daemon(listener: UnixListener, sensor_id: &'static str) {
        thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut reader = FrameReader::new(stream.try_clone().expect("clone"));
            let mut writer = FrameWriter::new(stream);

            let Ok(ClientMsg::Hello { .. }) = reader.read_client() else {
                panic!("expected Hello");
            };
            writer
                .write_server(&ServerMsg::Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    plugins: vec![],
                })
                .expect("write welcome");

            let Ok(ClientMsg::ListSensors) = reader.read_client() else {
                panic!("expected ListSensors");
            };
            writer
                .write_server(&ServerMsg::SensorList(vec![fake_sensor(sensor_id)]))
                .expect("write sensor list");

            // Wait for the reconnect replay, send one sample, then close.
            // The sample must not advance the pump's generation because the
            // replay is about to fail on the next RPC.
            match reader.read_client() {
                Ok(ClientMsg::Subscribe { sensors, .. }) => {
                    writer
                        .write_server(&ServerMsg::SensorListBroadcast(vec![fake_sensor(sensor_id)]))
                        .expect("write broadcast");
                    for s in sensors {
                        writer
                            .write_server(&ServerMsg::Sample(Sample {
                                sensor: s,
                                ts_micros: 1,
                                reading: Reading::Scalar(42.0),
                            }))
                            .expect("write sample");
                    }
                }
                other => panic!("expected Subscribe, got {other:?}"),
            }
            // Socket closes on thread exit, causing the next RPC to fail.
        });
    }

    fn make_workspace(sensor_id: &'static str) -> (Workspace, PathBuf, Arc<AtomicBool>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("linsight.sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let disconnected = Arc::new(AtomicBool::new(false));
        spawn_fake_daemon(listener, sensor_id, Arc::clone(&disconnected));

        // Give the listener thread a moment to start accepting.
        std::thread::sleep(Duration::from_millis(50));
        let client = Client::connect_or_spawn(&path).expect("connect");
        let workspace = Workspace::new(client, "local").expect("workspace");
        (workspace, path, disconnected)
    }

    fn recv_sample(rx: &Receiver<SampleBridge>, sensor_id: &str) -> Sample {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok((_gen, s)) = rx.recv_timeout(Duration::from_millis(100))
                && s.sensor.as_str() == sensor_id
            {
                return s;
            }
        }
        panic!("timed out waiting for sample from {sensor_id}");
    }

    #[test]
    fn workspace_forwards_samples_from_client() {
        let (workspace, _, _disconnected) = make_workspace("cpu.util");
        let rx = workspace.take_sample_rx().expect("take sample rx");
        workspace.subscribe(vec![SensorId::new("cpu.util")]).expect("subscribe");
        let sample = recv_sample(&rx, "cpu.util");
        assert!(matches!(sample.reading, Reading::Scalar(42.0)));
    }

    #[test]
    fn reconnect_swaps_client_and_keeps_receiver() {
        let (workspace, path_a, disconnected_a) = make_workspace("cpu.util");
        let rx = workspace.take_sample_rx().expect("take sample rx");

        workspace.subscribe(vec![SensorId::new("cpu.util")]).expect("subscribe");
        let first = recv_sample(&rx, "cpu.util");
        assert_eq!(first.ts_micros, 1);

        // Stand up a second fake daemon on a different socket.
        let dir_b = tempfile::tempdir().expect("tempdir");
        let path_b = dir_b.path().join("linsight.sock");
        let listener_b = UnixListener::bind(&path_b).expect("bind");
        let disconnected_b = Arc::new(AtomicBool::new(false));
        spawn_fake_daemon(listener_b, "mem.used_bytes", Arc::clone(&disconnected_b));
        std::thread::sleep(Duration::from_millis(50));

        workspace.reconnect_to_path(&path_b).expect("reconnect");

        // The new client should answer with the new sensor; the old one is gone.
        workspace.subscribe(vec![SensorId::new("mem.used_bytes")]).expect("subscribe");
        let second = recv_sample(&rx, "mem.used_bytes");
        assert!(matches!(second.reading, Reading::Scalar(42.0)));

        // Old daemon should have seen the connection close.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && !disconnected_a.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(disconnected_a.load(Ordering::SeqCst), "old client was not dropped");
        assert!(!disconnected_b.load(Ordering::SeqCst), "new client dropped unexpectedly");

        // Keep path_a alive until after the assertion so the listener stays
        // bound for the duration of the test.
        drop(path_a);
    }

    #[test]
    fn reconnect_replays_subscriptions() {
        let (workspace, _path_a, _disconnected_a) = make_workspace("cpu.util");
        let rx = workspace.take_sample_rx().expect("take sample rx");

        workspace.subscribe(vec![SensorId::new("cpu.util")]).expect("subscribe");
        let _ = recv_sample(&rx, "cpu.util");

        let dir_b = tempfile::tempdir().expect("tempdir");
        let path_b = dir_b.path().join("linsight.sock");
        let listener_b = UnixListener::bind(&path_b).expect("bind");
        let disconnected_b = Arc::new(AtomicBool::new(false));
        spawn_fake_daemon(listener_b, "cpu.util", Arc::clone(&disconnected_b));
        std::thread::sleep(Duration::from_millis(50));

        workspace.reconnect_to_path(&path_b).expect("reconnect");

        // The subscription was replayed, so we should get a sample without
        // explicitly subscribing again.
        let replayed = recv_sample(&rx, "cpu.util");
        assert!(matches!(replayed.reading, Reading::Scalar(42.0)));
    }

    #[test]
    fn connection_alive_clears_when_client_drops() {
        let (workspace, _path, _disconnected) = make_workspace("cpu.util");
        let alive = workspace.connection_alive();
        assert!(alive.load(Ordering::SeqCst));

        drop(workspace);

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && alive.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive.load(Ordering::SeqCst), "connection_alive should clear when client drops");
    }

    #[test]
    fn catalogue_receiver_survives_reconnect() {
        let (workspace, _path_a, _disconnected_a) = make_workspace("cpu.util");
        let sample_rx = workspace.take_sample_rx().expect("take sample rx");
        let catalogue_rx = workspace.take_catalogue_rx().expect("take catalogue rx");

        workspace.subscribe(vec![SensorId::new("cpu.util")]).expect("subscribe");
        let _ = recv_sample(&sample_rx, "cpu.util");
        let (gen_before, _) = catalogue_rx.recv().expect("initial broadcast");

        let dir_b = tempfile::tempdir().expect("tempdir");
        let path_b = dir_b.path().join("linsight.sock");
        let listener_b = UnixListener::bind(&path_b).expect("bind");
        let disconnected_b = Arc::new(AtomicBool::new(false));
        spawn_fake_daemon(listener_b, "mem.used_bytes", Arc::clone(&disconnected_b));
        std::thread::sleep(Duration::from_millis(50));

        workspace.reconnect_to_path(&path_b).expect("reconnect");

        // The stable catalogue receiver should survive and see a broadcast
        // from the new daemon with a higher generation.
        let (gen_after, _) = catalogue_rx.recv().expect("post-reconnect broadcast");
        assert!(
            workspace.connection_alive().load(Ordering::SeqCst),
            "catalogue receiver should survive reconnect"
        );
        assert!(gen_after > gen_before, "generation should advance after reconnect");
    }

    #[test]
    fn reconnect_advances_generation() {
        let (workspace, _path_a, _disconnected_a) = make_workspace("cpu.util");
        let gen_before = workspace.connection_generation().load(Ordering::SeqCst);

        let dir_b = tempfile::tempdir().expect("tempdir");
        let path_b = dir_b.path().join("linsight.sock");
        let listener_b = UnixListener::bind(&path_b).expect("bind");
        let disconnected_b = Arc::new(AtomicBool::new(false));
        spawn_fake_daemon(listener_b, "mem.used_bytes", Arc::clone(&disconnected_b));
        std::thread::sleep(Duration::from_millis(50));

        workspace.reconnect_to_path(&path_b).expect("reconnect");

        let gen_after = workspace.connection_generation().load(Ordering::SeqCst);
        assert_eq!(gen_after, gen_before + 1, "generation should increment by one");
    }

    #[test]
    fn failed_reconnect_keeps_old_connection_alive() {
        let (workspace, path_a, disconnected_a) = make_workspace("cpu.util");
        let rx = workspace.take_sample_rx().expect("take sample rx");

        workspace.subscribe(vec![SensorId::new("cpu.util")]).expect("subscribe");
        let _ = recv_sample(&rx, "cpu.util");

        // Use a non-default pump interval so the reconnect replay has a
        // second RPC that will fail when the flaky daemon closes.
        workspace
            .set_pump_interval_ms(200, Duration::from_secs(3))
            .expect("set pump interval on old connection");

        let dir_b = tempfile::tempdir().expect("tempdir");
        let path_b = dir_b.path().join("linsight.sock");
        let listener_b = UnixListener::bind(&path_b).expect("bind");
        spawn_flaky_daemon(listener_b, "mem.used_bytes");
        std::thread::sleep(Duration::from_millis(50));

        let result = workspace.reconnect_to_path(&path_b);
        assert!(result.is_err(), "reconnect should fail after daemon closes");

        // The original connection must still be usable.
        workspace.subscribe(vec![SensorId::new("cpu.util")]).expect("resubscribe old");
        let sample = recv_sample(&rx, "cpu.util");
        assert!(matches!(sample.reading, Reading::Scalar(42.0)));
        assert!(!disconnected_a.load(Ordering::SeqCst), "old daemon should still be connected");

        drop(path_a);
    }
}
