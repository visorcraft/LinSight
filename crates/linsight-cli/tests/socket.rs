// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn default_socket_refuses_tmp_fallback_without_xdg_runtime_dir() {
    Command::cargo_bin("linsight-cli")
        .unwrap()
        .env_remove("XDG_RUNTIME_DIR")
        .arg("list")
        .assert()
        .failure()
        .stderr(contains("refusing to fall back to /tmp"));
}
