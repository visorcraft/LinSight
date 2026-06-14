<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Build and test

## Prerequisites

| Component | Minimum | Notes |
|---|---|---|
| Rust | 1.95 (stable) | `rust-toolchain.toml` pins this; rustup honors it |
| Linux kernel | 6.11 | For mature `xe` driver + Battlemage perf counters |
| Qt 6 | 6.10 | GUI only; CLI + daemon build without Qt |
| Kirigami 6 | any | GUI only |
| clang | 16+ | cxx-qt drives `cc` to compile generated C++ |
| SQLite | system | bundled via the `rusqlite` feature `bundled`, but a system SQLite is fine too |
| libnvidia-ml | 560+ | Optional; absent = `linsight-sensors-nvml` produces zero sensors |
| `lupdate6`/`lrelease6` | qt6-tools | Only needed when changing QML strings |

Distro packages:

| Distro | Command |
|---|---|
| Arch / CachyOS | `pacman -S rust qt6-base qt6-declarative kirigami qt6-tools clang` |
| Debian / Ubuntu | `apt install rustc cargo qt6-base-dev qt6-declarative-dev qt6-tools-dev kirigami2-dev clang libsqlite3-dev` |
| Fedora | `dnf install rust cargo qt6-qtbase-devel qt6-qtdeclarative-devel kf6-kirigami-devel clang sqlite-devel` |
| openSUSE | `zypper install rust cargo qt6-base-devel qt6-declarative-devel kirigami6-devel clang sqlite3-devel` |

## Dev loop

```
just ci                  # fmt-check + clippy -D warnings + cargo test --workspace
just test                # cargo test --workspace
just test-release        # cargo test --workspace --release
just bench               # Criterion benchmarks for protocol/core hot paths
just build               # debug
just build-release       # release with lto=fat, codegen-units=1, strip
just build-release-v3    # x86_64-v3 tuned release (CachyOS-friendly)
just lint                # cargo clippy --workspace --all-targets -- -D warnings
just fmt                 # cargo fmt --all
just credits             # cargo about generate → docs/third-party-notices.md
just run-daemon          # cargo run -p linsightd
just run-cli ARGS        # cargo run -p linsight-cli -- ARGS
```

CI parity: `just ci` runs `fmt --check`, `clippy -D warnings`, and
`cargo test --workspace` in that order. A red local `just ci` is a
red CI run.

Release-mode tests (`just test-release`) are worth running before
shipping: the v0.3.0 ABI v2→v3 migration was caused by a stabby
`match_owned` misdispatch that only surfaced at `opt-level >= 1`.

## Workspace map

```
linsight/
├── apps/
│   ├── linsight-gui/           # Qt 6 / Kirigami GUI (crate name: linsight)
│   ├── linsightd/              # daemon
│   └── linsight-tunnel/        # mTLS bridge (rustls + tokio)
├── crates/
│   ├── linsight-core/          # types, no I/O
│   ├── linsight-protocol/      # wire format
│   ├── linsight-plugin-sdk/    # public plugin API (ABI v6, stabby R-mirror)
│   ├── linsight-sensors/       # one in-tree plugin per hardware family
│   │   ├── cpu/  mem/  net/  nvme/  nvml/
│   │   ├── xe/  i915/  amdgpu/         # GPUs
│   │   ├── disk/  fs/  hwmon/  zram/
│   │   └── proc/  system/  systemd/  containers/  sock/
│   └── linsight-cli/
├── examples/
│   └── echo-plugin/            # minimal third-party plugin cdylib;
│                               # exercised by the SDK's dynamic-load test
├── docs/                       # this directory
├── packaging/                  # PKGBUILD / spec / Flatpak / AppImage / systemd
├── scripts/                    # dev helpers (dev_screenshot.sh, gui_smoke.sh)
└── target/                     # cargo output
```

## Tests

```
cargo test --workspace   # run for the current count; a few hardware-gated tests are #[ignore]d
```

Sensor crates use `tempfile::TempDir` to build synthetic `/sys`
fixtures — no real hardware required for unit tests. Hardware-
dependent tests are `#[ignore]`d and only run with
`cargo test -- --ignored`. The daemon's integration test in
`apps/linsightd/tests/handshake.rs` exercises the full
subscribe-then-sample flow against the daemon binary.

The SDK has a real end-to-end integration test at
`crates/linsight-plugin-sdk/tests/dynamic_load.rs`. It builds
`examples/echo-plugin/` as a `.so`, dlopens it via the same
`StabbyLibrary::get_stabbied` path the daemon uses, and asserts
the full ABI v6 handshake — closing the "fabricated test claim"
gap flagged in the post-v0.3.0 peer review.

`linsight-tunnel` ships a paired mTLS smoke at
`apps/linsight-tunnel/tests/mtls_smoke.rs`: generates a self-
signed CA + server + client cert chain via `rcgen`, exercises the
tokio-rustls + ring + TLS 1.2 + 1.3 stack end-to-end, and
asserts both a successful round-trip and rogue-client cert
rejection.

A GUI boot-smoke test (`scripts/gui_smoke.sh`, invoked via
`just gui-smoke`) wraps `xvfb-run` and asserts the daemon
handshake log line within 12 s. It distinguishes timeout (exit
124) from clean exit, uses GNU automake's exit-77 skip
convention when `xvfb-run` is missing, and forces software OpenGL
with `LIBGL_ALWAYS_SOFTWARE=1` so it runs reliably on GPU-less
runners. It is **not** part of `just ci` — it needs a display
surface — but it does run as a separate `gui-smoke` job in the
GitHub Actions CI on `ubuntu-latest`. Run `just gui-smoke`
manually before shipping QML changes.

For visual iteration, `scripts/dev_screenshot.sh <page> [out.png]`
kills any running GUI, makes sure the daemon is up, launches
`linsight <page> --reduce-motion --screenshot <out>`, and exits. The
PNG is rendered via `QQuickWindow::grabWindow()` so the result is
independent of compositor focus — a Wayland-stale-frame trap that
external screenshot tools (spectacle, grim) fall into for unfocused
windows.

## Plugins (third-party `.so`)

See `docs/plugin-sdk.md` for the full API. Quick path:

```
linsight-cli plugin new my-sensor
cd my-sensor && cargo build --release
linsight-cli plugin install target/release/libmy_sensor.so
```

The daemon needs to be restarted to pick up new plugins. `systemctl
--user restart linsight` for always-on mode; otherwise just relaunch
the GUI.
