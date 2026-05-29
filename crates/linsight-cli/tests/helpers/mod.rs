// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Builds (or cached-locates) `linsightd` via `escargot` and returns the
/// absolute path to the binary. Cached so each test only triggers one
/// `cargo build`.
pub fn linsightd_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let bin = escargot::CargoBuild::new()
            .package("linsightd")
            .bin("linsightd")
            .current_release()
            .run()
            .expect("build linsightd");
        bin.path().to_path_buf()
    })
}

pub fn spawn_daemon(socket: &Path) -> Child {
    Command::new(linsightd_path())
        .args(["--socket", socket.to_str().unwrap()])
        .env("LINSIGHT_LOG", "warn")
        .spawn()
        .expect("spawn linsightd")
}

/// Wait until the daemon is actually accepting connections on `p`, not
/// just until the inode exists. The kernel creates the socket file before
/// the daemon enters its accept loop, so a naive `p.exists()` check can
/// race and produce intermittent `connection refused` errors in CI.
pub fn wait_for_socket(p: &Path) {
    use std::os::unix::net::UnixStream;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if p.exists() && UnixStream::connect(p).is_ok() {
            return;
        }
        sleep(Duration::from_millis(20));
    }
    panic!("daemon did not start accepting on {}", p.display());
}
