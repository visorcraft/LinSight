// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use linsight_core::{Sample, SensorId};

use crate::client::{Client, ClientHandle, RpcError};

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
    sample_tx: Sender<Sample>,
    /// Receiver side of the bridge. `take()` returns `Some` exactly once.
    sample_rx: Mutex<Option<Receiver<Sample>>>,
    /// Sensors the GUI currently wants subscribed. Replayed against a
    /// new client after reconnect so tile streams resume automatically.
    subscriptions: Mutex<Vec<SensorId>>,
    /// Last pump-interval value successfully applied. Replayed on reconnect.
    pump_interval_ms: Mutex<u32>,
}

impl Workspace {
    pub fn new(client: ClientHandle) -> anyhow::Result<Self> {
        let client_rx = client
            .take_sample_rx()
            .ok_or_else(|| anyhow::anyhow!("client sample receiver already taken"))?;
        let (sample_tx, sample_rx) = channel::<Sample>();
        spawn_sample_forwarder(client_rx, sample_tx.clone());

        Ok(Self {
            client: Mutex::new(client),
            sample_tx,
            sample_rx: Mutex::new(Some(sample_rx)),
            subscriptions: Mutex::new(Vec::new()),
            pump_interval_ms: Mutex::new(linsight_protocol::PUMP_INTERVAL_DEFAULT_MS),
        })
    }

    /// Take the one-shot sample receiver that feeds every live tile. Returns
    /// `None` if called more than once.
    pub fn take_sample_rx(&self) -> Option<Receiver<Sample>> {
        self.sample_rx.lock().expect("sample_rx poisoned").take()
    }

    /// Snapshot of the current client. RPC QObjects use this for one-off
    /// request/response calls. The returned `Arc` may outlive a reconnect
    /// briefly; in-flight RPCs will simply time out on the old connection.
    pub fn client(&self) -> ClientHandle {
        Arc::clone(&*self.client.lock().expect("client poisoned"))
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

        self.reconnect_with_client(new_client)
    }

    fn reconnect_with_client(&self, new_client: ClientHandle) -> Result<(), String> {
        let new_rx = new_client
            .take_sample_rx()
            .ok_or_else(|| "new client's sample receiver already taken".to_string())?;

        // Apply stored state to the new client *before* swapping. If this
        // fails we can still return the error without losing the old
        // connection.
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

        // Swap in the new client. The old one drops, killing its dispatch
        // thread and therefore its sample forwarder.
        let old_client = {
            let mut guard = self.client.lock().expect("client poisoned");
            std::mem::replace(&mut *guard, new_client)
        };
        drop(old_client);

        // Bridge the new client's samples into the same stable receiver.
        spawn_sample_forwarder(new_rx, self.sample_tx.clone());
        Ok(())
    }

    /// Test-only entry point that connects to an explicit socket path.
    #[cfg(test)]
    pub fn reconnect_to_path(&self, path: &std::path::Path) -> Result<(), String> {
        let new_client = Client::connect_or_spawn(path).map_err(|e| e.to_string())?;
        self.reconnect_with_client(new_client)
    }
}

fn spawn_sample_forwarder(client_rx: Receiver<Sample>, bridge_tx: Sender<Sample>) {
    thread::spawn(move || {
        while let Ok(s) = client_rx.recv() {
            if bridge_tx.send(s).is_err() {
                // The OverviewModel dropped its receiver (app exiting).
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::{Category, Reading, SensorKind, Unit};
    use linsight_protocol::{ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, ServerMsg};
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
                    Ok(ClientMsg::Goodbye) | Err(_) => {
                        disconnected.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(_) => {}
                }
            }
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
        let workspace = Workspace::new(client).expect("workspace");
        (workspace, path, disconnected)
    }

    fn recv_sample(rx: &Receiver<Sample>, sensor_id: &str) -> Sample {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(100))
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
}
