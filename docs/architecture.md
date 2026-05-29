<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Architecture

LinSight is a Cargo workspace with one daemon binary, one GUI
binary, one CLI binary, one mTLS-tunnel binary, a public plugin SDK,
a wire-protocol crate, a shared types crate, one sensor-backend
crate per hardware family, and a worked example plugin
(`examples/echo-plugin/`) that doubles as the SDK's dynamic-load
integration test.

## Process model

```
+---------------------+        socket          +-------------------+
|  linsight (GUI)     | <--------------------> | linsightd (daemon)|
|  Qt 6 + Kirigami    |  postcard-framed       | plugin host       |
|  via cxx-qt 0.8     |  Unix socket           | subscription      |
+---------------------+        @                | scheduler         |
                          $XDG_RUNTIME_DIR/    +-------------------+
                          linsight.sock                 |
                                                        | StabbyLibrary
+---------------------+                                 | ::get_stabbied
|  linsight-cli       |                                 v
|  list / read /      | <-------- same -----+-----------------------+
|  plugin {new,...}   |          socket     | in-tree sensors:      |
+---------------------+                     |   cpu, mem, xe, nvml, |
                                            |   nvme, net           |
                                            | runtime .so plugins   |
                                            | (ABI v3, R-mirror     |
                                            |  R-mirror types)      |
                                            +-----------------------+

+---------------------+   mTLS    +---------------------+
|  linsight-tunnel    | <-------> |  linsight-tunnel    |
|  (client mode)      |  TCP      |  (server mode)      |
|  listens on local   |   |       |  listens on TCP,    |
|  Unix socket,       |   v       |  pipes bytes to     |
|  pipes to remote    |  rustls   |  local Unix socket  |
+---------------------+           +---------------------+
```

The GUI auto-spawns `linsightd` as a child if no daemon is already
listening. When the GUI exits its `Client::drop` sends `Goodbye` and
reaps the child. An opt-in systemd user unit (`linsight.service`)
runs the daemon persistently when always-on mode is enabled —
attaching the GUI later just finds the existing socket.

For remote dashboards, SSH-forwarded sockets are the recommended
path. `linsight-tunnel` is the fallback for non-SSH topologies: a
transparent byte pipe over rustls-based mTLS between a TCP socket
and the daemon's Unix socket. See `apps/linsight-tunnel/` for the
CLI surface.

## Subscription-driven sampling

Sensors only run when at least one client is subscribed. `Scheduler`
maintains a refcount per sensor; reaching zero drops the entry. Each
entry tracks `next_due_at` so `tick(now)` produces only the samples
whose period has elapsed. The Prometheus exporter is the one
exception: it scrapes synchronously via `Scheduler::sample_now`
without subscribing.

## Crate boundaries

- `linsight-core` — pure types: `SensorId`, `Reading`, `Sample`,
  `Unit`, `SensorKind`, `Category`, plus the `DashboardSpec` schema.
  No I/O, no async, no Qt.
- `linsight-protocol` — postcard wire types + length-prefixed
  `FrameReader`/`FrameWriter` + version handshake helper.
- `linsight-plugin-sdk` — public `LinsightPlugin` trait, manifest
  types, and the `export_plugin!` macro. Plugins compile against
  this crate alone (plus a direct `stabby = "36"` dep for ABI v3's
  proc-macros). The `mirror` submodule holds the FFI-boundary
  R-types (`RUnit`, `RSensorKind`, `RCategory`, `RReading`, …).
  v3 encodes payload-bearing mirrors as `(kind, payload)` structs
  rather than stabby tagged enums; see ADR-0001 for the
  release-mode `match_owned` bug that drove the v2→v3 refactor.
  `host_init` validates every plugin-returned sensor ID with
  `SensorId::try_new` *before* the From-conversion runs the
  infallible `SensorId::new`, so a release-mode plugin emitting
  whitespace-bearing IDs is rejected with `PluginError::Parse`
  instead of poisoning the registry. `PluginCtx::new_with_sysroot`
  refuses non-UTF-8 paths up front so the FFI mirror's UTF-8
  contract holds.
- `linsight-example-echo-plugin` (in `examples/echo-plugin/`) —
  minimal `cdylib` that emits one sensor returning a constant.
  Built by the SDK's `tests/dynamic_load.rs` integration test,
  which dlopens the resulting `.so` via the same
  `StabbyLibrary::get_stabbied` path the daemon uses and asserts
  the full handshake works. Also the canonical reference for
  third-party plugin authors.
- `linsight-sensors/{cpu,mem,xe,nvml,nvme,net}` — one in-tree plugin
  per hardware family. All are statically linked into `linsightd`;
  same code path as a runtime-loaded `.so`.
- `linsight-cli` — thin CLI: `list`, `read`, `plugin {new,install,
  ls,remove}`. Uses the postcard client like the GUI.
- `linsight` (in `apps/linsight-gui/`) — Qt 6 / Kirigami binary.
  cxx-qt 0.8 bridges Rust to QML. Owns the postcard client, the
  `OverviewModel` QObject, the sidebar shell, preset pages
  (Overview / GPUs / Storage / Network), the Phase 6b canvas
  editor, and the multi-window scaffold. Includes an in-app
  screenshot path (`--screenshot <path>`) that calls
  `QQuickWindow::grabWindow()` to bypass Wayland compositor
  caching.
- `linsightd` — daemon. Hosts plugins, schedules sampling, serves
  clients. Optional subsystems: SQLite history, evalexpr alerts,
  Prometheus exporter.
- `linsight-tunnel` (in `apps/linsight-tunnel/`) — mTLS bridge for
  non-SSH remote topologies. `server` / `client` subcommands;
  `tokio::io::copy_bidirectional` as the pipe, no protocol
  parsing. Defaults to `127.0.0.1:9443` (pass `--bind 0.0.0.0:9443`
  for public bind). Caps concurrent connections via a `Semaphore`
  (default 64 on each side) and drains in-flight TLS sessions on
  Ctrl+C / SIGTERM with a 10-second budget before aborting.

## Data flow for a single sample

1. Daemon's `transport::unix::serve` thread reads a `Subscribe`
   message from a client; the scheduler refcount goes 0 → 1.
2. The same thread's sample-pump loop calls `scheduler.tick(now)`
   every 50 ms.
3. `tick()` finds the sensor's `next_due_at <= now`, calls
   `plugin.sample(id)` to produce a `Sample`, optionally records
   it to history + evaluates alert rules.
4. The pump thread frames the `ServerMsg::Sample` with `postcard`
   and writes it to the socket.
5. The client's reader thread decodes the frame and forwards the
   `Sample` to a channel.
6. The GUI's worker thread drains the channel, formats the value,
   and posts a property update via `cxx_qt::Threading::queue`.
7. QML's tile sees the property change and re-renders.

End-to-end latency on the dev machine: ~5–15 ms from subscribe to
first visible value.
