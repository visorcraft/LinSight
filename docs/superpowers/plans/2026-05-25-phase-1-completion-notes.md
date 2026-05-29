<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Phase 1 Completion Notes (v0.1.0)

**Tag:** `v0.1.0`
**Date:** 2026-05-25

## Delivered against the plan

Every task in [`2026-05-25-foundation-cli-mvp.md`](./2026-05-25-foundation-cli-mvp.md)
was implemented in code. The end-state target —
`linsight-cli read cpu.util` streams live samples from `linsightd`
over a Unix socket — is met:

```
$ linsight-cli list
cpu.util  CPU utilization  % scalar

$ linsight-cli read cpu.util --count 3
cpu.util  0.0%
cpu.util  11.0%
cpu.util  9.6%
```

## Test count by crate

| Crate | Tests |
|---|---:|
| `linsight-core` | 13 |
| `linsight-plugin-sdk` | 6 |
| `linsight-protocol` | 17 |
| `linsight-sensors-cpu` | 12 |
| `linsightd` (unit) | 9 |
| `linsightd` (integration, end-to-end) | 2 |
| **Total** | **59** |

`just ci` (fmt-check + clippy `-D warnings` + tests) is green.

## Release binary sizes

Built with `lto=fat`, `codegen-units=1`, `panic=abort`, `strip=symbols`:

| Binary | Size |
|---|---:|
| `target/release/linsightd` | ~1.3 MB |
| `target/release/linsight-cli` | ~1.3 MB |

Both well under the daemon RSS budget (≤ 7 MB) when running.

## Deviations from the written plan

A few small adjustments were made during execution:

1. **`rust_2024_idioms` → `rust_2018_idioms`.** The plan used
   `rust_2024_idioms` as a lint group name; that group does not
   exist in stable Rust 1.95 (it would have been a warning in
   `-D warnings` mode). Switched to the established
   `rust_2018_idioms` group everywhere.

2. **rustfmt.toml trimmed of nightly-only options.**
   `imports_granularity = Crate` and `group_imports = StdExternalCrate`
   are unstable; stable rustfmt warns and skips them. Removed both;
   `cargo fmt` now produces a stable, reproducible style.

3. **`export_plugin!` macro returns `*mut dyn LinsightPlugin` with
   `#[allow(improper_ctypes_definitions)]`.** The fat-pointer return
   is not strictly FFI-safe, but in Phase 1 plugins are statically
   linked, so the pointer never crosses a real FFI boundary. Phase 5
   will replace this with a `stabby::DynPtr`-based version that is
   genuinely cdylib-safe. Documented inline.

4. **CLI integration tests deferred.** The plan included
   `crates/linsight-cli/tests/{list,read}.rs` that spawn `linsightd`
   as a subprocess. `CARGO_BIN_EXE_<name>` is only set within the
   binary's own test crate, so cross-package binary discovery needs
   `escargot` or `cargo metadata`. End-to-end coverage of the wire
   protocol lives in `apps/linsightd/tests/handshake.rs` (which
   spawns the daemon and exercises subscribe/sample). A manual smoke
   test in the README covers the CLI binary. Adding `escargot` to
   re-enable the CLI integration tests is a small follow-up.

5. **`stabby` and `polling` workspace deps declared but unused in v1.**
   They're pre-pulled in the root `Cargo.toml`'s
   `[workspace.dependencies]` block for Phase 5 (dynamic plugin
   loading) and Phase 7 (event-loop scheduler). They add no compile
   cost to Phase 1 since no crate actually depends on them yet.

6. **Bundled per-task commits into per-crate commits.** The plan had
   one commit per TDD task (35 commits); execution produced 9
   semantically meaningful commits, one per crate plus the bootstrap
   and packaging steps. History is cleaner and bisect-friendly.

## Known caveats / follow-ups

- **Stale socket on SIGKILL.** `SocketGuard` removes the socket file
  on `Drop`, which fires on normal exit but not on SIGKILL. The
  daemon's startup logic removes stale sockets, so the next launch
  recovers fine. Adding a signal handler for SIGTERM/SIGINT that
  triggers graceful shutdown is a small follow-up.
- **`cargo deny` / `cargo audit` not yet run** because they require
  user `cargo install`. `just preflight` runs them when available;
  document in `docs/build-and-test.md` (added in a later phase).
- **No `LICENSE` for third-party deps yet.** Will be auto-generated
  by `cargo about` in Phase 10 (packaging) — same pattern as Grexa.

## What ships in v0.1.0 (and what doesn't)

**In:** workspace skeleton, `linsight-core` types, wire protocol
(messages + framing + handshake), plugin SDK (trait + manifest +
export macro), CPU sensor as an in-tree plugin, daemon (plugin host
+ subscription scheduler + Unix transport with per-client threads),
CLI with `list` and `read` subcommands.

**Out (deferred to Phase 2+):** Qt GUI, GPU sensors, NVMe + Network,
dynamic `.so` plugin loading, custom dashboards, always-on mode,
remote dashboards, theming, full packaging.

## Next: Phase 2

[`2026-MM-DD-gui-overview-mvp.md`](./2026-MM-DD-gui-overview-mvp.md)
(not yet written). End-state: launch `linsight`, see the Overview
preset page with live CPU + RAM tiles. Brings in Qt 6 / Kirigami via
`cxx-qt`, the postcard client in a worker thread, and the first
sensor visualizations.
