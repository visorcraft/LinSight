<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# LinSight

A fast, beautiful, modular Linux system-monitoring dashboard with
multi-GPU support, a runtime plugin system, and a remote mTLS
tunnel.

**Status:** v1.0.0 — Hardeningsprint + per-device nicknames + extensive plugin expansion.
**344+ tests passing.** Daemon, CLI, Qt 6 / Kirigami GUI with
sidebar navigation, preset pages (Overview / GPUs / Storage / Network /
Hardware), a custom-canvas editor with keyboard-accessible nav, alerts
(argv-exec only — no shell injection), Prometheus exporter with
single-snapshot scrapes and a stable `device_key` label, SQLite history,
NVIDIA + Intel xe + AMD + NVMe + network + disk + filesystem + hwmon + system (PSI, load, uptime) sensors, runtime `.so` plugins
(ABI v4 via R-mirror types — validated at the FFI boundary,
dynamically-loaded test coverage, kind+payload struct encoding so
release builds round-trip every variant correctly), and the
`linsight-tunnel` mTLS bridge for non-SSH remote topologies (graceful
shutdown, bounded connections, localhost default bind).

See [`docs/superpowers/specs/2026-05-25-linsight-design.md`](docs/superpowers/specs/2026-05-25-linsight-design.md)
for the v1 design,
[`docs/superpowers/plans/2026-05-25-phases-roadmap.md`](docs/superpowers/plans/2026-05-25-phases-roadmap.md)
for the 10-phase rollout history, and
[`CHANGELOG.md`](CHANGELOG.md) for the user-facing change log.

## Try it

GUI:

```bash
cargo run -p linsight
# Kirigami window with a left sidebar:
# Workspace → Overview / GPUs / Storage / Network / Hardware / Editor
# System    → Settings / About
# Auto-spawns linsightd as a child if no daemon is running.
```

Keyboard shortcuts: `Ctrl+1..5` for the workspace pages (Overview /
GPUs / Storage / Network / Hardware), `F1` for About,
`StandardKey.Preferences` for Settings.

CLI:

```bash
just run-daemon                     # or let the GUI spawn it
just run-cli list                   # sensor catalogue (52+ entries)
just run-cli read cpu.util --count 5
just run-cli read mem.used_bytes --count 3
just run-cli plugin new my-sensor   # scaffold a third-party plugin
```

Remote (mTLS, non-SSH topologies):

```bash
# On the remote machine running linsightd. Default bind is
# 127.0.0.1:9443; pass --bind 0.0.0.0:9443 to expose to the network.
linsight-tunnel server \
  --bind 0.0.0.0:9443 \
  --cert server.pem --key server.key --ca clients-ca.pem \
  --socket /run/user/1000/linsight.sock

# On your desktop:
linsight-tunnel client \
  --listen $XDG_RUNTIME_DIR/linsight-remote.sock \
  --server remote.host.example:9443 \
  --cert client.pem --key client.key --ca server-ca.pem

# Then any LinSight client (GUI/CLI) connects to the local socket
# as usual; bytes are piped over mTLS to the remote daemon.
```

See [`apps/linsight-tunnel/README.md`](apps/linsight-tunnel/README.md)
for a full topology diagram, an openssl cert-generation recipe, and
the trust-model caveats (the configured CA is a full-access trust
boundary — there's no per-cert CN/SAN filter yet).

For most remote use, an SSH-forwarded socket
(`ssh -L $XDG_RUNTIME_DIR/linsight.sock:remote-runtime/linsight.sock host`)
is simpler and equally secure.

## Build

```bash
just ci              # fmt-check + clippy -D warnings + tests (213 pass)
just build           # debug
just build-release   # release: lto=fat, codegen-units=1, strip
just build-release-v3   # x86_64-v3 tuned (CachyOS / modern systems)
```

Optional preflight (install with
`cargo install cargo-deny cargo-audit cargo-about`):

```bash
just preflight       # ci + deny + audit
just credits         # cargo about generate → docs/credits-third-party.md
```

## Always-on mode (opt-in)

`packaging/systemd/linsight.service` is a systemd user unit. Enable
once to keep the daemon resident; the GUI / CLI then attach to the
existing socket. Always-on mode also gates history (`LINSIGHT_HISTORY`)
+ alerts (`LINSIGHT_ALERTS`) + the Prometheus exporter
(`LINSIGHT_PROM_BIND`); see the Settings page for env-var status.

## Screenshots (dev iteration)

```bash
./scripts/dev_screenshot.sh overview /tmp/shot.png
```

Internally drives `linsight --screenshot <path> --reduce-motion`.
`--reduce-motion` (alias: `--no-animations`) zeroes out all QML
animation durations so the captured frame doesn't include a tween
midpoint. `--screenshot` calls `QQuickWindow::grabWindow()` to
render the QML scene to PNG independently of compositor focus —
this bypasses the Wayland stale-surface trap that `spectacle` /
`grim` fall into for unfocused windows.

## Architecture

- **`apps/linsightd/`** — daemon; hosts plugins, schedules
  subscription-driven sampling, serves clients over a
  postcard-framed Unix socket. Optional history (SQLite), alerts
  (evalexpr), Prometheus exporter.
- **`apps/linsight-gui/`** — Qt 6 / Kirigami GUI via cxx-qt 0.8.
  Sidebar shell, preset pages, canvas editor, multi-window. The
  GUI auto-spawns the daemon if none is listening.
- **`apps/linsight-tunnel/`** — mTLS bridge for the daemon socket.
  `server` / `client` subcommands; transparent byte pipe.
- **`crates/linsight-core/`** — shared types (no I/O, no async).
- **`crates/linsight-protocol/`** — postcard wire format + framing.
- **`crates/linsight-plugin-sdk/`** — public `LinsightPlugin`
  trait + `export_plugin!` macro. ABI v4 uses R-mirror types on
  the FFI boundary for cross-rustc safety, encoded as
  `(kind, payload)` structs over `#[repr(u8)]` discriminants. See
  [`docs/adr/0001-plugin-abi-stabby-deferral.md`](docs/adr/0001-plugin-abi-stabby-deferral.md)
  for the v2→v3 rationale.
- **`crates/linsight-sensors/{cpu,mem,xe,nvml,nvme,net}/`** — one
  in-tree plugin per hardware family.
- **`crates/linsight-cli/`** — `list` / `read` / `plugin {new,
  install, ls, remove}`.
- **`examples/echo-plugin/`** — minimal third-party plugin built as
  a `cdylib`. Exercised by the SDK's `tests/dynamic_load.rs` to
  guarantee the `export_plugin!` macro produces a `.so` the daemon
  can actually dlopen; also serves as a worked example for plugin
  authors.

See [`docs/architecture.md`](docs/architecture.md) for the full
process model and data flow.

## License

GPL-3.0-only. See `LICENSE`. Third-party license credits live in
[`docs/credits-third-party.md`](docs/credits-third-party.md).
