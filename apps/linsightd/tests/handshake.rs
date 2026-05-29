// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use linsight_protocol::{
    ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, RequestOp, ResponsePayload, ServerMsg,
};

mod harness;
use harness::DaemonHarness;

#[test]
fn daemon_accepts_hello_replies_welcome() {
    let harness = DaemonHarness::spawn();
    let stream = UnixStream::connect(&harness.socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    let mut writer = FrameWriter::new(stream.try_clone().unwrap());
    let mut reader = FrameReader::new(stream);

    writer
        .write_client(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
            auth_token: None,
        })
        .unwrap();

    let welcome = reader.read_server().expect("welcome");
    match welcome {
        ServerMsg::Welcome { protocol_version, .. } => {
            assert_eq!(protocol_version, PROTOCOL_VERSION);
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
}

#[test]
fn subscribe_receives_at_least_one_sample() {
    let harness = DaemonHarness::spawn();
    let stream = UnixStream::connect(&harness.socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

    let mut writer = FrameWriter::new(stream.try_clone().unwrap());
    let mut reader = FrameReader::new(stream);

    writer
        .write_client(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
            auth_token: None,
        })
        .unwrap();
    let _ = reader.read_server().unwrap();

    writer
        .write_client(&ClientMsg::Subscribe {
            sensors: vec![linsight_core::SensorId::new("cpu.util")],
            rate_hz: None,
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut got_sample = false;
    while std::time::Instant::now() < deadline {
        match reader.read_server() {
            Ok(ServerMsg::Sample(s)) if s.sensor.as_str() == "cpu.util" => {
                got_sample = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(got_sample, "expected a cpu.util sample within 3 seconds");

    writer.write_client(&ClientMsg::Goodbye).unwrap();
}

#[test]
fn list_sensors_decorates_with_device_label() {
    // F4 regression guard: at least one returned SensorInfo must carry
    // a non-empty `device_label` after the daemon collects v4 hardware
    // manifests into the HardwareRegistry. We check loosely (any
    // labeled row passes) so the test stays insensitive to whether
    // the build host has a discrete GPU, NVMe, etc — every machine
    // has at least a CPU device, which the cpu plugin declares.
    let harness = DaemonHarness::spawn();
    let stream = UnixStream::connect(&harness.socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

    let mut writer = FrameWriter::new(stream.try_clone().unwrap());
    let mut reader = FrameReader::new(stream);

    writer
        .write_client(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test-decorate".into(),
            auth_token: None,
        })
        .unwrap();
    let _ = reader.read_server().unwrap();

    writer.write_client(&ClientMsg::ListSensors).unwrap();
    let infos = match reader.read_server().expect("sensor list") {
        ServerMsg::SensorList(v) => v,
        other => panic!("expected SensorList, got {other:?}"),
    };
    let labeled =
        infos.iter().filter(|s| s.device_label.as_deref().is_some_and(|l| !l.is_empty())).count();
    assert!(
        labeled > 0,
        "expected at least one SensorInfo with a non-empty device_label; got {} sensors with none decorated",
        infos.len(),
    );

    writer.write_client(&ClientMsg::Goodbye).unwrap();
}

#[test]
fn set_nickname_round_trips_and_broadcasts() {
    // End-to-end coverage of the F6 SetNickname flow: a client's
    // RPC produces a `Response` to the caller AND a
    // `SensorListBroadcast` to every other connected client, with
    // the nickname applied to the SensorInfo `device_label`s in the
    // broadcast. Also checks the on-disk `hardware.json` save.
    //
    // Target device-key selection: we ListSensors on client A first
    // and pick any sensor with a `device_key`. The cpu plugin
    // declares `cpu:0` but its sensor descriptor has
    // `device_id: None`, which means `device_label` isn't populated
    // on the wire SensorInfo even after a nickname is applied —
    // making cpu:0 a poor witness for the broadcast-label
    // assertion. Picking dynamically from the live ListSensors
    // keeps the assertion meaningful on any host with at least one
    // GPU/NVMe/network device declared (which every Linux box has;
    // even `lo`-only hosts get a `net:lo` entry from the net
    // plugin).
    let harness = DaemonHarness::spawn();

    // Two client connections. Neither subscribes to any sensor, so
    // the only ServerMsgs they receive after Welcome are Response
    // (for the caller) and SensorListBroadcast (for both).
    let (mut a_writer, mut a_reader) = open_session(&harness, "test-a");
    let (mut b_writer, mut b_reader) = open_session(&harness, "test-b");

    // Pick a device_key by asking the daemon what's available.
    a_writer.write_client(&ClientMsg::ListSensors).unwrap();
    let device_key = loop {
        match a_reader.read_server().expect("sensor list") {
            ServerMsg::SensorList(infos) => {
                let key = infos
                    .iter()
                    .find_map(|s| s.device_key.clone())
                    .expect("at least one sensor with a device_key on the host");
                break key;
            }
            // ListSensors response only ever produces SensorList,
            // but in case a broadcast from elsewhere races in we
            // just skip and keep reading.
            _ => continue,
        }
    };

    a_writer
        .write_client(&ClientMsg::Request {
            req_id: 1,
            op: RequestOp::SetNickname {
                device_key: device_key.clone(),
                value: Some("Test Nick".into()),
            },
        })
        .unwrap();

    // Caller's Response must arrive within 2s. Drain unrelated
    // messages (in particular the broadcast that arrives to the
    // caller too) until we see it.
    let response_deadline = Instant::now() + Duration::from_secs(2);
    let mut a_saw_response = false;
    let mut a_saw_broadcast = false;
    while Instant::now() < response_deadline && (!a_saw_response || !a_saw_broadcast) {
        match a_reader.read_server() {
            Ok(ServerMsg::Response {
                req_id: 1,
                result: Ok(ResponsePayload::NicknameSet { value, .. }),
            }) => {
                assert_eq!(value.as_deref(), Some("Test Nick"));
                a_saw_response = true;
            }
            Ok(ServerMsg::Response { req_id, result }) => {
                panic!("unexpected Response req_id={req_id} result={result:?}");
            }
            Ok(ServerMsg::SensorListBroadcast(infos)) => {
                let labels: Vec<_> = infos.iter().filter_map(|s| s.device_label.clone()).collect();
                assert!(
                    infos.iter().any(|s| s.device_label.as_deref() == Some("Test Nick")),
                    "client A broadcast lacks any SensorInfo with device_label='Test Nick'; labels seen: {labels:?}",
                );
                a_saw_broadcast = true;
            }
            Ok(_) => continue,
            Err(e) => panic!("client A read failed before Response: {e:?}"),
        }
    }
    assert!(a_saw_response, "client A did not receive Response within 2s");
    assert!(a_saw_broadcast, "client A did not receive SensorListBroadcast within 2s");

    // Client B must see the broadcast too.
    let broadcast_deadline = Instant::now() + Duration::from_secs(2);
    let mut b_saw_broadcast = false;
    while Instant::now() < broadcast_deadline && !b_saw_broadcast {
        match b_reader.read_server() {
            Ok(ServerMsg::SensorListBroadcast(infos)) => {
                assert!(
                    infos.iter().any(|s| s.device_label.as_deref() == Some("Test Nick")),
                    "client B broadcast lacks any SensorInfo with device_label='Test Nick'",
                );
                b_saw_broadcast = true;
            }
            Ok(_) => continue,
            Err(e) => panic!("client B read failed before broadcast: {e:?}"),
        }
    }
    assert!(b_saw_broadcast, "client B did not receive SensorListBroadcast within 2s");

    // On-disk persistence: hardware.json should exist under the
    // isolated XDG_CONFIG_HOME and contain both the nickname and
    // the device_key under which it was stored.
    let hw_json = harness.xdg_config_home.join("linsight/hardware.json");
    assert!(hw_json.exists(), "expected hardware.json at {}", hw_json.display());
    let contents = std::fs::read_to_string(&hw_json).expect("read hardware.json");
    assert!(
        contents.contains("Test Nick"),
        "hardware.json does not contain the nickname: {contents}",
    );
    assert!(
        contents.contains(&device_key),
        "hardware.json does not contain device_key {device_key}: {contents}",
    );

    a_writer.write_client(&ClientMsg::Goodbye).unwrap();
    b_writer.write_client(&ClientMsg::Goodbye).unwrap();
}

/// Open a client session: connect, send Hello, drain the Welcome,
/// and return the framed writer/reader pair. Read timeout is set to
/// 2s so a wedged daemon surfaces a panic instead of hanging the
/// whole test.
fn open_session(
    harness: &DaemonHarness,
    client_name: &str,
) -> (FrameWriter<UnixStream>, FrameReader<UnixStream>) {
    let stream = UnixStream::connect(&harness.socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut writer = FrameWriter::new(stream.try_clone().unwrap());
    let mut reader = FrameReader::new(stream);
    writer
        .write_client(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: client_name.into(),
            auth_token: None,
        })
        .unwrap();
    let welcome = reader.read_server().expect("welcome");
    assert!(matches!(welcome, ServerMsg::Welcome { .. }), "expected Welcome, got {welcome:?}");
    (writer, reader)
}
