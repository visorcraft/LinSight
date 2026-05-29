// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use assert_cmd::Command;
use predicates::str::is_match;

mod helpers;
use helpers::{spawn_daemon, wait_for_socket};

#[test]
fn watch_single_sensor_two_samples_then_exits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli")
        .unwrap()
        .args(["--socket", socket.to_str().unwrap(), "watch", "cpu.util", "--count", "2"])
        .timeout(Duration::from_secs(8))
        .assert()
        .success()
        .stdout(is_match(r"(?m)^cpu\.util\s+\d+(\.\d+)?%$").unwrap());

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn watch_multiple_sensors_two_samples_each() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    // Subscribe to both cpu.util and mem.used_bytes — we'll receive 2
    // samples total (the count is global, not per-sensor), so the output
    // should contain lines for both sensors.
    Command::cargo_bin("linsight-cli")
        .unwrap()
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "watch",
            "cpu.util",
            "mem.used_bytes",
            "--count",
            "2",
        ])
        .timeout(Duration::from_secs(8))
        .assert()
        .success()
        .stdout(is_match(r"(?m)^(cpu\.util|mem\.used_bytes)\s+").unwrap());

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn watch_json_format() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli")
        .unwrap()
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "watch",
            "cpu.util",
            "--format",
            "json",
            "--count",
            "1",
        ])
        .timeout(Duration::from_secs(8))
        .assert()
        .success()
        .stdout(
            is_match(
                r#"(?m)^\{"sensor":"cpu\.util","value":\d+(\.\d+)?,"unit":"%","kind":"scalar"\}$"#,
            )
            .unwrap(),
        );

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn watch_unknown_sensor_fails_fast() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli")
        .unwrap()
        .args(["--socket", socket.to_str().unwrap(), "watch", "nonexistent.sensor"])
        .timeout(Duration::from_secs(5))
        .assert()
        .failure()
        .stderr(predicates::str::contains("sensor not found"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}
