// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use assert_cmd::Command;
use predicates::str::contains;

mod helpers;
use helpers::{spawn_daemon, wait_for_socket};

#[test]
fn list_prints_cpu_sensor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli")
        .unwrap()
        .args(["--socket", socket.to_str().unwrap(), "list"])
        .assert()
        .success()
        .stdout(contains("cpu.util"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}
