// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct DaemonHarness {
    pub socket: PathBuf,
    /// Isolated `XDG_CONFIG_HOME` handed to the daemon. Tests that
    /// trigger `SetNickname` need a sandboxed config dir so the
    /// daemon's `hardware.json` save doesn't trample the developer's
    /// real config. Exposed so a test can assert the on-disk file.
    pub xdg_config_home: PathBuf,
    child: Child,
    _tmp: tempfile::TempDir,
}

impl DaemonHarness {
    pub fn spawn() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket = tmp.path().join("linsight.sock");
        let xdg_config_home = tmp.path().join("xdg");
        std::fs::create_dir_all(&xdg_config_home).unwrap();
        let bin = env!("CARGO_BIN_EXE_linsightd");
        let child = Command::new(bin)
            .args(["--socket", socket.to_str().unwrap()])
            .env("LINSIGHT_LOG", "warn")
            .env("XDG_CONFIG_HOME", &xdg_config_home)
            .spawn()
            .expect("spawn linsightd");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if socket.exists() {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        Self { socket, xdg_config_home, child, _tmp: tmp }
    }
}

impl Drop for DaemonHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
