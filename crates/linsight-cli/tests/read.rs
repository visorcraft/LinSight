// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use assert_cmd::Command;
use predicates::str::is_match;

mod helpers;
use helpers::{spawn_daemon, wait_for_socket};

#[test]
fn read_streams_two_samples_then_exits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli")
        .unwrap()
        .args(["--socket", socket.to_str().unwrap(), "read", "cpu.util", "--count", "2"])
        .timeout(Duration::from_secs(8))
        .assert()
        .success()
        .stdout(is_match(r"(?m)^cpu\.util\s+\d+(\.\d+)?%$").unwrap());

    let _ = daemon.kill();
    let _ = daemon.wait();
}
