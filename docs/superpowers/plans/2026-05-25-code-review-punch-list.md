<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Code Review Punch List — 2026-05-25

> **Status: closed.** Every Critical and High item below was resolved
> in the 2026-05-25 audit-driven hardening sprint. Most Medium items
> were also addressed; the few that remain (mostly cosmetic GUI nits
> and `RPluginCtx.sysroot_set: u8` deferred to ABI v3) are tracked in
> the open-followups doc.
>
> See [`CHANGELOG.md`](../../../CHANGELOG.md) for the user-facing
> summary and `2026-05-25-commit-review-punch-list.md` (in this
> directory) for the second peer review that followed.

In-depth peer code review of the v0.3.x codebase. Six independent
auditors covered the workspace in parallel; this is the consolidated
findings list. Per-area raw reports archived under
`/tmp/linsight-review/0{1..6}-*.md` at audit time.

## Tally

| Area | Critical | High | Medium | Low | Investigate |
|------|---------:|-----:|-------:|----:|------------:|
| daemon (`linsightd`)                  | 2 | 8 | 6 | 3 | 4 |
| GUI (`linsight-gui`)                  | 0 | 5 | 8 | 4 | 3 |
| tunnel + CLI                          | 2 | 6 | 6 | 5 | 3 |
| core + protocol + plugin-sdk          | 2 | 3 | 4 | 4 | 2 |
| basic sensors (cpu/mem/net)           | 1 | 4 | 3 | 3 | 3 |
| HW sensors (xe/nvml/nvme)             | 2 | 4 | 4 | 4 | 3 |
| **total**                             | **9** | **30** | **31** | **23** | **18** |

Headline: the workspace is structurally competent — plugin loading,
stabby ABI plumbing, scheduler, history batching, alert debounce, and
the cxx-qt bridge are all correctly built. The damage is concentrated
in (a) the transport layer (`apps/linsightd/src/transport/unix.rs`),
(b) anything that "ships but was never run" (tunnel mTLS, multi-window
GUI, canvas editor, Phase 34 NVML process table), and (c) a project-wide
pattern of silently swallowing errors with `.ok()` / `let _ = ...` /
wildcard match arms.

---

## Cross-cutting patterns (fix workspace-wide)

These show up in multiple crates. Worth a single sweep rather than
nine separate PRs.

1. **Silent error swallowing** — `Result.ok()`, `let _ = result;`,
   `_ => continue`, empty `Err(_) => {}` arms. Appears in: daemon
   `transport/unix.rs`, `history.rs`, `alerts.rs`; GUI `client.rs`,
   `overview_model.rs`; CLI `read.rs`, `plugin.rs`; sensor `mem`,
   `net`; nvml `processes` arm. Each instance has at minimum one
   user-visible failure mode where the operator gets a stale value
   or empty list instead of an error.

2. **Wildcard `_ => {}` / `_ => continue` hides protocol messages.**
   GUI `client.rs:88` doesn't validate `ServerMsg::Welcome.protocol_version`
   (CLI does); CLI `read.rs:44` swallows `ServerMsg::Bye`. Daemon transport
   hardcodes plugin IDs instead of reading them from the host.

3. **Hardcoded values that should be named constants.** `tile0/gt0`
   (xe), `nvme0n1` (nvme), `io.visorcraft.linsight.cpu` (daemon
   transport), `0.1..=20.0` Hz (`SensorDescriptor::clamped_rate_hz`
   not shared with `scheduler.rs`), tile dimensions `200×120` and
   centering offset `100, 60` (`CanvasEditorPage.qml`), page header
   `height: 76` copy-pasted across 4 QML pages.

4. **Tests reading live `/proc` or `/sys` without `#[ignore]`.**
   `cpu/proc_stat.rs:166-169`, `mem/meminfo.rs:112-116`. Violates
   the convention stated in AGENTS.md and will fail in any hermetic
   CI sandbox.

5. **No graceful shutdown anywhere.** Tunnel declares the tokio
   `signal` feature but never imports it. Daemon's Prom exporter
   thread has no shutdown signal. `PluginHost` has no `Drop` or
   `shutdown_all()` — `LinsightPlugin::shutdown()` is dead code.
   Daemon client threads aren't joined or tracked.

6. **Plugin-host `sysroot` test override doesn't reach FFI.**
   `plugin-sdk/src/plugin.rs:103` uses `to_string_lossy` on the
   `PathBuf`, silently corrupting non-UTF-8 paths. nvml hardcodes
   `/proc/{pid}/comm` instead of going through `sysroot`. Net effect:
   the synthetic-fixture test pattern that works for cpu/mem doesn't
   actually work for plugins that take sysroot through the public
   ABI.

---

## Critical (must-fix before next release)

### CR-1. Daemon transport hardcodes `plugin_id` for every sensor
**`apps/linsightd/src/transport/unix.rs:75-80` and `:130`** — Both the
`Welcome` plugin list and every `SensorInfo` from `ListSensors`
hardcode `plugin_id = "io.visorcraft.linsight.cpu"` regardless of which
plugin actually owns the sensor. Adding any built-in beyond CPU (mem,
net, xe, nvml, nvme — all of them) results in the daemon claiming they
belong to the CPU plugin. Any client that filters by plugin is broken.

### CR-2. CLI `read` hangs forever on unknown sensor name
**`crates/linsight-cli/src/commands/read.rs:18-21`** + daemon — Unknown
sensor falls through to `Unit::Count`, subscribes blindly, daemon
discards the subscribe with a `warn!`, CLI waits forever. With
`--count` the process never exits.

### CR-3. `plugin new` scaffold doesn't compile
**`crates/linsight-cli/src/commands/plugin.rs:49`** — Generated
`Cargo.toml` writes `linsight-plugin-sdk = "0.3"` as the active
dependency, but the crate isn't on crates.io. Every scaffolded plugin
fails on first `cargo build`. The right `path = ...` line is in a
comment that gets ignored.

### CR-4. Stabby mirror skips sensor-ID validation in release builds
**`crates/linsight-plugin-sdk/src/mirror.rs:35`** — `From<RSensorId>
for SensorId` calls `SensorId::new()`, which is `debug_assert!`-only.
In release builds a plugin returning an empty or whitespace ID crosses
FFI silently and is inserted into the registry at `plugin_host.rs:101`.
Fix: call `try_new()` and surface through `init()`.

### CR-5. Dashboard `migrate()` is a stub that will eat configs on next schema bump
**`crates/linsight-core/src/dashboard.rs:142-157`** — Returns
`Err(MigrationFailed)` immediately when `from < to`. Comment says
"future migrations would loop over `0 => migrate_0_to_1`" — that loop
doesn't exist. Combined with `load()` defaulting `from = 0` for files
without a `schema_version` field, the moment `DASHBOARD_SCHEMA_VERSION`
is bumped, every user's existing config is permanently rejected with
no recovery.

### CR-6. Mem sensor reports 100% used when `MemAvailable` is absent
**`crates/linsight-sensors/mem/src/meminfo.rs:55-58`** — If
`/proc/meminfo` doesn't include `MemAvailable` (containers scrubbing
proc, kernels < 3.14, sandboxes), `available_bytes` silently defaults
to `0` and `used_bytes()` returns `total_bytes`. The dashboard shows
100% memory used with no error indication.

### CR-7. NVML process-table silently returns empty on enumeration failure
**`crates/linsight-sensors/nvml/src/lib.rs:175-184`** (Phase 34 commit
`03ab588`) — Both `running_compute_processes()` and
`running_graphics_processes()` are wrapped in `if let Ok(...)`. If both
fail (MIG mode, exclusive-compute, driver mismatch), the method returns
`Ok(Reading::Table([]))` — indistinguishable from "no processes
running" to the operator.

### CR-8. xe `freq_mhz` sensor lies about its value
**`crates/linsight-sensors/xe/src/plugin.rs:129-133`** — Sensor ID is
`xe.gpu{idx}.freq_mhz`, declared `unit: Unit::Hertz`, and the emitted
value multiplies the sysfs MHz by `1_000_000`. A 1200 MHz GPU reports
~1.2 billion. The ID, the unit, and the value are in three-way
disagreement.

### CR-9. Alert `shell:` target executes user TOML via `sh -c`
**`apps/linsightd/src/alerts.rs:188-191`** — `Command::new("sh").arg("-c").arg(cmd).status();`
where `cmd` is the user's raw `alerts.toml` value with no escaping.
RCE for anyone who can write the alerts config (malicious dotfile,
config sync gone wrong). Even without an attacker, accidental TOML
mis-quotes execute as shell.

---

## High (real bugs that bite in normal use)

### Daemon
- **`transport/unix.rs:34`** — Thread-per-client spawn, no tracking, no cap, no backpressure. Combine with the read-timeout swallow below for a trivial DoS.
- **`transport/unix.rs:43`** — Persistent `accept()` errors (EMFILE, ENOMEM) turn into hot-spin / log flood with no backoff.
- **`transport/unix.rs:57`** — `stream.set_read_timeout(None).ok()` discards the error; a client that connects but never sends `Hello` parks a thread forever.
- **`prom.rs:38-53`** — Prometheus accept-loop is a detached `thread::spawn` with no shutdown signal and no panic recovery.
- **`prom.rs:96-105`** — `render()` re-acquires the scheduler lock O(N) per scrape and interleaves with pump threads' `tick()` — samples in one scrape are taken at different instants, violating the Prometheus consistency rule.
- **`history.rs:99-101`** — Final flush before shutdown silently drops the last batch with `let _ = flush(...);`.
- **`scheduler.rs:68`** — Period computation `(1_000_000.0 / effective as f64) as u64` produces `u64::MAX` if `effective` is 0.0, silently parking the sensor.

### GUI
- **`qml/DashWindow.qml:45-48`** — Passes `dashboardModel` to children that declare `dashModel` — every child page in a secondary window stays blank at "…" forever.
- **`qml/DashWindow.qml` (whole file)** — Multi-window feature claimed shipped; no trigger UI exists. Sidebar / Main.qml never instantiates it.
- **`src/qobjects/overview_model.rs:181-204`** — Daemon disconnect silently exits the pump thread; UI tiles freeze at last value with no banner, no reconnect, no connection-state property.
- **`src/client.rs:163`** — `socket.to_str().unwrap()` panics on non-UTF-8 XDG_RUNTIME_DIR.
- **`qml/CreditsPage.qml:129`** — UI tells user to run `just credits`; that target doesn't exist in the Justfile.

### Tunnel + CLI
- **`apps/linsight-tunnel/Cargo.toml:17` + `src/main.rs`** — `tokio::signal` feature enabled, never imported. There is no graceful shutdown anywhere. Ctrl+C aborts every TLS session and leaks the client-mode Unix socket.
- **`apps/linsight-tunnel/src/main.rs:183-198, 276-292`** — Unbounded connection spawn (no semaphore, no `--max-connections`). DoS surface that triggers before TLS auth.
- **`apps/linsight-tunnel/src/main.rs:193, 287`** — `JoinHandle`s dropped; no graceful drain is possible even if signal handling existed.
- **`apps/linsight-tunnel/src/main.rs:259-263`** — TOCTOU between `args.listen.exists()` and `remove_file`; also no socket cleanup on exit/panic.
- **`crates/linsight-cli/src/commands/read.rs:44`** — `_ => continue` swallows `ServerMsg::Bye`; user sees opaque I/O error instead of "daemon going away".
- **`crates/linsight-cli/src/commands/plugin.rs:17`** — `user_plugin_dir()` falls back to `./linsight-plugins` (relative CWD) if `HOME` and `XDG_DATA_HOME` are both unset. Install / ls / remove operate on different directories than the daemon reads.

### Core / Protocol / SDK
- **`crates/linsight-core/src/error.rs` + `dashboard.rs:110-130`** — `CoreError::InvalidSensorId` is reused as a catch-all for every I/O and serde failure. Callers can't distinguish "config corrupt" from "ID empty". Add `Io` / `Serialize` variants.
- **`apps/linsight-gui/src/client.rs:88`** — GUI client accepts `ServerMsg::Welcome { .. }` with a wildcard — it ignores `protocol_version`. CLI checks it; daemon checks client's `Hello`. GUI is the only client that would silently keep talking to a mismatched daemon.
- **`crates/linsight-plugin-sdk/src/plugin.rs:103`** — `PluginCtx → RPluginCtx` uses `to_string_lossy`, silently corrupting non-UTF-8 sysroot paths. Breaks the synthetic-fixture test pattern for any plugin taking sysroot through the public ABI.

### Basic sensors
- **`mem/src/meminfo.rs:45`** — Per-line parse failures silently swallowed via `.ok()`. Future kernel format drift (e.g. unit suffix change) goes completely unnoticed for `MemAvailable`, `SwapTotal`, `SwapFree`.
- **`cpu/src/proc_stat.rs:166-169`** — Test reads live `/proc/stat`, not `#[ignore]`d.
- **`mem/src/meminfo.rs:112-116`** — Same: reads live `/proc/meminfo` without `#[ignore]`.
- **`net/src/lib.rs` (test module)** — Only one test: enumeration. `sample_inner`, `init_inner`, every rx/tx counter, link state, speed — all completely untested.

### HW sensors
- **`xe/src/sysfs.rs:61-68`** — `enumerate()` returns `Err` when `/sys/class/drm` is absent. nvme and nvml handle this with `Ok(vec![])`. Inconsistent no-hardware contract — only saved by the daemon's outer plugin-init fallback.
- **`xe/src/sysfs.rs:36, 41`** — `act_freq` and `idle_residency_ms` hardcode `tile0/gt0/`. Doesn't exist on multi-tile GPUs (Ponte Vecchio, future Arc) — sensor advertises then errors at every sample.
- **`nvml/src/lib.rs:39-52`** — `init_inner` documented "idempotent" but nvml-wrapper docs explicitly say repeated `Nvml::init` is unwise: it re-loads function symbols and (on Drop) calls `nvmlShutdown` then re-`init`s. Init-once-keep-alive is the correct pattern.

---

## Medium (cleanup; see per-area report for full text)

(31 items total — see `/tmp/linsight-review/0{1..6}-*.md`. Highlights:)

- **GUI canvas editor** — `PaletteRow` mixes `Drag.Automatic` with
  `MouseArea.drag.target`; the visual proxy teleports rather than
  following the cursor. **This is almost certainly the "rough drag
  ergonomics" the open-followups doc has been waiting on a human to
  validate** (`CanvasEditorPage.qml:491-530`).
- **i18n coverage** — `Justfile:57-59` (`i18n-extract`) covers only 3
  of 13+ QML files. CanvasEditorPage, SettingsPage, AboutPage,
  DashWindow, LicensesPage, CreditsPage, CategoryPage, GplLicenseDialog
  all have `qsTr()` calls that never reach the `.ts`/`.qm` catalogs.
  Release-blocking for the claimed de/ja i18n support.
- **Daemon `runtime.rs:63`** — Comment says "subsystem 2 of 2"; should
  be "3 of 3". Copy-paste slop.
- **Daemon `prom.rs:143-147`** — Dead function `_doc_only_marker` that
  takes `&SensorId` and does nothing.
- **Plugin SDK `manifest.rs:40`** — `MIN_RATE_HZ` / `MAX_RATE_HZ` magic
  floats (`0.1`/`20.0`) duplicated between SDK clamp and daemon
  scheduler. Extract to a shared constant.
- **Plugin SDK `plugin.rs:138`** — `LinsightPlugin::shutdown()` default
  exists but is never called anywhere. Either wire `PluginHost::shutdown_all()`
  or document teardown as plugin `Drop` responsibility.
- **Plugin SDK `RPluginCtx.sysroot_set: u8`** — manual 0/1 boolean
  instead of stabby's `Option`. Hides a pairing invariant.
- **Dashboard module doc** (`linsight-core/src/dashboard.rs:18`) — says
  "Phase 6b: not yet implemented". Stale; Phase 6b shipped.
- **NVMe** (`nvme/src/lib.rs:215-219`) — Only tracks the first
  namespace (`nvme0n1`). Multi-namespace enterprise SSDs silently miss
  I/O counters.
- **Net sensor** (`net/src/lib.rs:123-127`) — `speed_mbps` error path
  silently returns `-1.0` for *any* I/O error (not just the expected
  EINVAL from virtual interfaces).
- **CLI plugin.rs** — Three `fs::write` calls in `plugin::new` missing
  `.with_context(...)`; `.flatten()` in `ls` silently drops permission
  errors per-entry.
- **Tunnel** — `set_nodelay(true).ok()`; daemon-down on TLS side
  produces opaque RST instead of a readable error to the client.

---

## Low / nits (23 items)

Mostly: copy-pasted magic numbers, mutex `.unwrap()` instead of
`.expect()`, English-only pluralization, `--count 0` off-by-one,
hardcoded ports without env-var fallback, README scaffolds that don't
mention the path-dep requirement. See per-area reports.

---

## Investigate (18 items — couldn't prove but worth a closer look)

- **Plugin host drop ordering** (`plugin_host.rs:27-35`) — comment
  claims field-declaration-order guarantees vtable outlives `Library`.
  Reasoning is correct for `Box<dyn Trait>`, but here it's `Arc<dyn
  LinsightPlugin>`. Any external `Arc::clone` (e.g. a future sampling
  thread holding one) breaks the invariant silently. Document or lint.
- **Export macro `#[unsafe(no_mangle)]`** (`plugin-sdk/src/export.rs:21`)
  — verify the pinned toolchain (1.95) actually accepts this edition-2024
  attribute. If not, the symbol is mangled and the daemon's
  `library.get(b"linsight_plugin_abi_version\0")` fails silently at load.
- **Scheduler retry loop** (`scheduler.rs:103-108`) — `PluginError::Unsupported`
  in `tick()` keeps the entry forever and re-tries every tick. No
  backoff, no cap. Per-tick warn flood on a removed device.
- **Postcard variant-order stability** (`linsight-protocol/src/messages.rs`)
  — Postcard serializes enum variants by discriminant index. Nothing
  in the source warns future contributors not to insert variants in
  the middle. A docs-only fix but worth doing before the next change.
- **NVML driver/library version skew** — `Nvml::init` does not check
  `lib_version()` vs `sys_driver_version()`. Log at init; warn on
  mismatch.
- **GUI binding-tick performance** — `CanvasEditorPage.qml:556-561`
  uses a `_valueTick` counter to force every visible tile's `liveValue`
  binding to re-evaluate on every sample, regardless of which sensor
  actually changed. Profile once a real dashboard exists.

---

## Suggested fix order (smallest-blast-radius first)

1. **CR-1** (daemon transport hardcoded plugin IDs) — tiny diff, huge
   correctness win. Wire `PluginHost::plugins()` and `:: sensors()`
   into the responses.
2. **CR-9** (alert `sh -c`) — replace with `argv` exec or remove the
   `shell:` target entirely. Security.
3. **CR-2** (`read` hangs on unknown sensor) — one branch in
   `read.rs` after `ListSensors`. UX bug, immediate value.
4. **CR-3** (`plugin new` scaffold) — string-edit the embedded
   template. Unblocks third-party plugin authors.
5. **CR-6** (mem sensor `MemAvailable` fallthrough) — one `if`. Avoid
   shipping a sensor that reports 100% used in any container.
6. **CR-7** (NVML processes silent empty) — one match arm. Avoid
   shipping a feature that lies to its user.
7. **CR-8** (xe freq_mhz unit mismatch) — rename the ID or drop the
   multiplication. Pick one, document, ship.
8. **CR-4** (stabby `try_new` for sensor IDs) — surface the error
   through the SDK; one-line change at the FFI boundary.
9. **CR-5** (`migrate()` stub) — *or* document the schema version as
   load-bearing and add a release-checklist note that bumping it
   requires writing the migration. Dormant today; ticking time bomb
   the moment the schema changes.
10. **Cross-cutting #1 (silent error swallowing)** — workspace sweep:
    grep `\.ok();` and `let _ = .*?;` in non-test code, justify each
    remaining instance with an inline comment.
11. **GUI multi-window + canvas-editor drag fix** — knocks off two
    items on the open-followups list at once.
12. Everything else as bandwidth allows.

---

## What the agents agreed about the author

Three of six reports independently used the phrase "structurally
competent" and three of six independently flagged that the failure
pattern is "ran out of steam after wiring the happy path." This is
consistent with a junior who can build a system but doesn't yet
internalize the discipline of error paths, shutdown paths, and "make
the test prove the production parser actually parses." The codebase is
salvageable — the architecture is sound — but every subsystem needs a
second pass on error handling, lifecycle, and test coverage of the
non-happy-path arms.
