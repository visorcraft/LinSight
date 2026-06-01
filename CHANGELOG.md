<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Changelog

All notable changes to LinSight. Format roughly follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions use
[SemVer](https://semver.org/).

## [Unreleased]

## [1.7.0] — 2026-06-01

- **Plugin panic isolation now actually works (plugin ABI v5 → v6).** A
  panic inside a plugin's `init`/`sample` is caught by the daemon
  (`PluginError::Panic`) instead of taking the whole process down. This
  required two changes that bump the plugin ABI: the trait methods are now
  `extern "C-unwind"` (so a panic can unwind across the FFI boundary rather
  than force-aborting at it), and the release profile is now
  `panic = "unwind"` (so `catch_unwind` is not a no-op). **Out-of-tree
  plugins must be rebuilt against the v6 SDK** — the factory symbol is
  renamed `linsight_plugin_v5` → `_v6`, so a stale `.so` is rejected at load
  with an actionable "rebuild against v6" log (the daemon skips it and keeps
  running; it does not crash). Porting a plugin's source is mechanical:
  change each `extern "C" fn init/sample/shutdown` in your
  `impl LinsightPlugin` to `extern "C-unwind" fn` (the compiler flags any you
  miss) and rebuild. See `docs/plugin-sdk.md` for the migration note.
- **Security hardening (audit follow-ups).** Fixed a panic in webhook URL
  validation on a malformed IPv6 literal (it ran under the alert-engine lock
  and would poison it); a use-after-`dlclose` on the duplicate-plugin-ID
  rejection path; the tunnel's misnamed "idle timeout" that was actually a
  hard 5-minute cap on every connection (now a true idle timeout that resets
  on activity); a leaked Prometheus connection slot on a worker panic; and
  WAL/`-shm` sidecar files inheriting the umask. Webhook SSRF validation now
  also rejects `userinfo@`-masked hosts, obfuscated numeric-IP encodings, and
  no longer follows redirects. Alert-rule evaluation no longer spawns a
  worker thread per rule per sample tick (the timeout sandbox is reserved for
  the ad-hoc `TestAlertExpr` RPC).

## [1.6.0] — 2026-05-31

- **Storage page: mount points are nested inside their physical disk.** Each
  disk is a section (ordered by capacity, largest first) showing the drive's
  own sensors, with its filesystems rendered as **collapsible inset cards**
  (collapsed by default) — `btrfs (/home)` sits under the Samsung 990 Pro it
  lives on, etc. The daemon resolves each mount's backing block device to its
  physical disk (including the NVMe namespace→controller step), so grouping is
  robust across btrfs subvolumes and multi-disk setups. Mounts with no
  resolvable local disk (NFS/CIFS, zram, LVM) appear as their own top-level
  sections.
- **GPU page: sections ordered by total VRAM, with unified sensor naming.**
  NVIDIA/Intel `GPU memory {used,total}` are renamed to `GPU VRAM
  {used,total}` to match AMD, and each GPU's sensors are grouped under the
  device and ordered by capacity (largest first).
- **Inode sensors are skipped for filesystems that don't report them.** btrfs
  and FAT/vfat return no inode counts from `statvfs` (perpetual zeros), so the
  daemon no longer registers or samples `inodes_total`/`inodes_used` for those
  mounts — less work in `linsightd` and a cleaner Storage page. ext4/xfs still
  report real inode usage.

## [1.5.0] — 2026-05-30

- **Dynamically-loaded `.so` plugins now receive their per-plugin config.**
  Previously only in-tree sensors honored `plugins.toml`; dynamic plugins
  always got an empty config, because a plugin's id (the config key) is
  only known after `init` runs — and `init` is what consumes the config.
  The loader now runs a throwaway "probe" `init` to read the id, looks up
  the config, and re-instantiates the plugin once with it (a fresh
  instance, so no live plugin is double-initialized). No plugin ABI
  change. A plugin that cannot `init` at all without config still can't be
  auto-configured, but that is vanishingly rare for read-only sensors.
- **New: container monitoring (Docker + Podman).** A
  `linsight-sensors-containers` plugin emits a `containers.list` table of
  running containers discovered in the cgroup v2 hierarchy
  (`system.slice/docker-*.scope`, `machine.slice/libpod-*.scope`): short
  id, runtime, CPU delta, memory, and PID count. Pure cgroup-filesystem
  reads — no Docker/Podman socket or elevated privileges, and it degrades
  to an empty table when no container runtime is present. (The non-systemd
  `cgroupfs` cgroup-driver layout is not covered.)
- **New: socket statistics sensors.** A `linsight-sensors-sock` plugin
  adds `sock.tcp_established`, `sock.tcp_listen`, `sock.tcp_time_wait`,
  `sock.udp_inuse`, and `sock.tcp_mem_bytes`, derived from
  `/proc/net/tcp{,6}` (per-connection state tally) and `/proc/net/sockstat`.

## [1.4.1] — 2026-05-29

- **Fixed: company domain corrected to `visorcraft.com`.** The GUI's Qt
  `organizationDomain` was `visorcraft.io`; it is now `visorcraft.com`,
  matching the project website. Packaging maintainer/contact fields and
  the security contact were updated to match.
- **Packaging: distro recipes now build end-to-end from a clean
  checkout** (Arch `!lto` fix for the `ring` link, Fedora container gains
  mold + correct target/output handling). No runtime behavior change.

## [1.4.0] — 2026-05-29

- **Themed, theme-aware buttons and dropdowns across the app.** The flat
  accent-tinted button style from the About page is now a shared
  `ThemedButton` component, applied to the Settings header (Reload), the
  Alerts header (Add Rule, Reload), and the dashboard viewer header
  (Export, Edit, Gallery). Hover/press wash and border tint toward the
  active theme's accent (via `ColorUtils.tintWithAlpha`) so they read on
  light *and* dark palettes — the previous `Qt.darker()` wash was invisible
  on dark themes.
- **New `ThemedComboBox` for all Settings dropdowns** (theme picker, start
  page, sample interval): matching surface/border/accent styling and a
  themed popup.
- **Fixed: dropdown list items were unreadable on dark themes (e.g. OLED
  Black).** The custom row delegate resolved the display role to
  `undefined` for JS-array models, rendering every entry blank.
- **Fixed: a blank row appeared in dropdowns (the item at the current
  selection).** The popup list shared the ComboBox's `DelegateModel`, which
  can only incubate a delegate in one place at a time; it now renders from
  the raw model with hand-wired selection so every row shows.
- **About page rework** to match the VisorCraft house style: a "built for"
  callout with the app icon and a repo link, a Licenses & Credits card,
  `GPL v3` / `Linux · Qt 6` pills, and a centered attribution footer.
- **Fixed: sidebar hover briefly highlighted two rows at once.** The hover
  wash shared the active pill's fade animation, so moving the mouse between
  rows left the previous row highlighted while it faded out. Hover/press is
  now painted instantly on a separate layer; the accent pill still eases.

## [1.3.0] — 2026-05-28

- **Hardware nicknames now appear everywhere as a second title line.** A
  device-scoped sensor renders its metric on top (e.g. "GPU utilization")
  and the device's nickname — or model, when no nickname is set — beneath
  it, across dashboard tiles, the GPUs / Storage / Network pages, and the
  canvas editor + palette. Previously the raw model was baked into the
  metric and the nickname was merely appended, so both showed at once
  ("NVIDIA GPU utilization (NVIDIA GeForce RTX 5080 Laptop GPU) · RTX 5080
  Max-Q"). Every device-scoped plugin now emits a device-agnostic metric;
  the device identity rides on the resolved `device_label`.
- **Fixed: constant values (e.g. GPU memory total) no longer disappear.**
  A flat sparkline was collapsing the value label's layout space a few
  seconds after load. Trend charts are now drawn only for series that
  actually vary, so a permanent value keeps its number and shows no chart.
- **Static capacity sensors are sampled once, not polled.** Sensors tagged
  `STATIC_TAG` (total VRAM via NVML/xe/amdgpu, installed RAM) are read a
  single time per subscription and then parked, instead of re-sampling on
  their native cadence. They also render as a rounded whole-unit capacity
  ("32 GB") rather than a fractional binary size ("31.84 GiB"), matching how
  the hardware is marketed.

## [1.2.0] — 2026-05-28

Rolls up the v1.1.0 development sprint into a tagged release and adds
dependency hygiene.

- **New in-tree sensors:** Intel i915 (legacy iGPU), zram compressed
  swap, and systemd unit monitoring, each as its own plugin crate.
- **Alerts UI.** Settings-side alert rule management (add / edit /
  enable / disable / delete) backed by the env-gated alerts surface.
  Rule enablement is tri-state on the wire so an edit can preserve the
  existing on/off state instead of clobbering it.
- **Plugin ABI v5.** `RPluginCtx` gains a `config_json` field for
  per-plugin TOML configuration; the factory symbol is renamed
  `linsight_plugin_v4` → `linsight_plugin_v5` so a stale `.so` fails
  fast at symbol lookup with a clear error instead of at first sample.
- **i915 / amdgpu PCI addressing fixed.** Device BDF now comes from
  resolving the sysfs `device` symlink rather than being synthesized,
  matching the xe backend.
- **Dependency cleanup.** Removed ten unused crate dependencies flagged
  by `cargo-machete`; `cxx` (GUI) and `stabby` (echo-plugin) are
  retained with documented machete-ignore entries because they are
  required by macro expansion that textual analysis can't see.

## [1.0.0] — 2026-05-27 — First stable release

LinSight reaches 1.0 with the full feature set targeted for v1:
multi-GPU monitoring with Intel xe (Battlemage-class engine
accounting via drm-usage-stats fdinfo) and NVIDIA NVML, NVMe +
network + CPU sensors, a Qt 6 / Kirigami GUI with theme picker,
custom dashboards, a Hardware page with per-device nicknames,
configurable per-client sample rate, the mTLS tunnel for non-SSH
remote topologies, the env-gated history / alerts / Prometheus
exporter, and a stable v4 plugin ABI with the runtime `.so`
loader exercised by the bundled echo-plugin example.

Since the previous tagged release (v0.3.0):

- **Sample interval is now user-configurable.** Settings page gains
  a Sample interval dropdown (50, 100, 150, 200, 250, 350, 500, 750,
  1000 ms). Per-client. New protocol variant
  `RequestOp::SetPumpIntervalMs`; daemon clamps to
  `PUMP_INTERVAL_{MIN,MAX}_MS` (50–1000) and echoes the applied
  value. Default 150 ms (was hardcoded 50 ms; halves idle wakeups
  for negligible UX impact).
- **Overview is 2x2.** Top row CPU util + memory used; bottom row
  CPU temperature (`cpu.temp_c` via coretemp / k10temp) + CPU
  frequency (`cpu.freq_hz` averaging `scaling_cur_freq`). Both
  sysfs-only, no privileges required.
- **Daemon idle CPU dropped 18% → ~3%.** Three stacked
  optimizations: shared 400 ms-TTL cache for the xe `fdinfo`
  capture; per-PID DRM fd discovery + 15 s full-rescan throttle so
  a Chrome process with 200 fds gets 2 fdinfo reads on the hot
  path instead of 200; CPU plugin caches the resolved coretemp +
  per-CPU `scaling_cur_freq` paths.
- **Hardware page** (Ctrl+5). Lists every detected GPU / NVMe /
  NIC / CPU with vendor-resolved model strings ("Intel Arc B-
  series", "NVIDIA RTX 5080 Mobile") and inline nickname editing.
  Nicknames propagate to GUI tile labels, CLI output, and the
  Prometheus exporter's `linsight_hardware_info` gauge.
- **Plugin SDK at ABI v4.** `PluginManifest` carries a `devices`
  list; `SensorDescriptor` carries a `device_key` back-reference.
  ADR-0001 documents the v2→v3 stabby release-mode-matcher fix;
  ADR-0002 documents the v3→v4 hardware-manifest extension.
- **Protocol at v2.** `SensorInfo` carries `device_key` +
  `device_label`. Correlated request/response via `req_id`.
  `SensorListBroadcast` pushed to every connected client after a
  nickname change.
- **Nickname store** at `~/.config/linsight/hardware.json`. Atomic
  write (tmp + fsync + rename) via the shared
  `linsight_core::atomic_write_json` helper that also backs
  `preferences.json` and the per-dashboard files.
- **Codebase cleanup.** Lifted PCI ID parsing + atomic-write JSON
  + the GUI client's RPC pattern (`get_hardware` / `set_nickname`
  / `set_pump_interval_ms` consolidated through a closure-based
  `request_rpc<R, F>` helper). Removed dead `Client::unsubscribe`,
  dead `NvmeDevice.ctrl_root`, dead `Meminfo::swap_used_bytes`,
  stale Phase markers.
- **Build hygiene.** GCC 16's `-Wsfinae-incomplete` silenced on
  the Qt 6 `QChar`/libstdc++ interaction so cxx-qt-generated
  `.cxx.cpp` shims compile silently. Fedora 44 containerized
  build path (`just fedora-pkg`) produces an RPM that installs
  cleanly on hosts with Qt 6.9 even when the dev host runs
  Qt 6.11.
- **213 tests passing.** Up from 198 at the previous release line.

## [0.5.2] — 2026-05-26 — Overview 2x2 + daemon perf

- **Overview page is now 2x2.** Top row: CPU utilization + Memory
  used (unchanged). Bottom row: CPU temperature + CPU frequency
  (new). Both new sensors are sourced from sysfs and need no
  privileges.
- **CPU temperature sensor** (`cpu.temp_c`) reads
  `/sys/class/hwmon/*` for `coretemp` (Intel) or
  `k10temp`/`zenpower` (AMD), preferring the package-level label
  (`Package id 0` / `Tctl`). Falls back gracefully on hosts without
  the relevant module.
- **CPU frequency sensor** (`cpu.freq_hz`) averages
  `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` across
  online CPUs. Returns Unsupported on virt-host configs where
  cpufreq isn't exposed.
- **Daemon idle-CPU drop, 18 % → ~3 %.** Three optimizations:
  1. xe `fdinfo` capture moved from per-device-per-sample to a
     shared 400 ms-TTL cache; one /proc walk now serves every xe
     GPU instead of N walks per second.
  2. xe maintains a cached set of PIDs known to hold DRM fds; the
     hot path only reads those PIDs' fdinfos, with a periodic 15 s
     full /proc rescan to pick up newly-spawned GPU clients.
     Typical desktop: 6 000 fdinfo reads per scan → ~30.
  3. CPU plugin resolves the coretemp `temp*_input` path and the
     per-CPU `scaling_cur_freq` list once, caching them across
     samples. Eliminates ~100 syscalls/sec of repeated
     `/sys/class/hwmon` and `/sys/devices/system/cpu` walks.

## [0.5.1] — 2026-05-26 — Hardware page + per-device nicknames

- **New Hardware page** (Ctrl+5) lists every detected GPU / NVMe /
  NIC / CPU with vendor-resolved model strings ("Intel Arc B-
  series", "NVIDIA RTX 5080 Mobile") and inline nickname editing.
- **Nicknames propagate everywhere** — GUI tile labels become
  `<metric> · <device label>`; daemon's SensorListBroadcast pushes
  refreshes to every connected client; Prometheus exporter adds a
  stable `device_key` label and a new `linsight_hardware_info`
  gauge for joins.
- **Plugin SDK ABI bumps v3 → v4.** `PluginManifest` gains a
  `devices` list; `SensorDescriptor` gains a `device_key`
  back-reference. v3 plugins fail symbol lookup at load (see
  ADR-0002). No third-party `.so` plugins existed yet, so this is
  a clean break.
- **Protocol bumps v1 → v2.** `SensorInfo` gains `device_key` +
  `device_label`; new `Request`/`Response` with `req_id`
  correlation; new `SensorListBroadcast` for nickname refresh.
  `ResponsePayload::Hardware` ships the nickname map alongside the
  devices so the Hardware page can pre-populate its TextField
  without a separate round-trip.
- **Loopback dropped from the net plugin.** `lo` is the kernel's
  software interface and has never been useful in a system monitor;
  it no longer appears in the Hardware page, the sensor catalogue,
  or the Prometheus output.
- **Nickname store** at `~/.config/linsight/hardware.json` (atomic
  tmp+rename, schema-versioned). Tmp file is cleaned up if the
  rename fails (e.g. cross-filesystem path) so stale `.json.tmp`
  siblings don't accumulate over retries.
- **Disambiguator suffix for duplicate-model devices.** Two
  identical Samsung 990 PROs (or two identical 4070s) now render as
  "Samsung SSD 990 PRO 2TB (s7kgnj…)" vs "(s7kgnu…)" instead of
  the same bare model. The policy lives in
  `linsight_core::compute_device_label` so the daemon's
  `SensorInfo.device_label` decoration and the GUI's Hardware-page
  title use one algorithm and can never disagree.
- **Hardware-page polish.** Cards compacted (title + key + sensor
  count on one row, tighter padding) so 8 cards fit at the default
  window height vs. the original 5. Nickname TextField disables +
  dims while the SetNickname RPC is in flight so the user can't
  double-fire an unsettled edit.
- **Daemon broadcast bookkeeping.** Per-client `SensorListBroadcast`
  senders moved from `Vec<Sender>` (pruned lazily on the next
  broadcast) to `HashMap<u64, Sender>` so each `serve()` thread
  proactively deregisters its entry on disconnect; bounds the map
  size to live connection count regardless of nickname-change
  cadence.
- **Daemon decoration prefers `SensorDescriptor.device_key`.** A
  sensor that sets device_key directly (the cpu plugin shape) used
  to be ignored by the registry lookup; now the explicit field wins,
  with the `(plugin_id, device_id)` lookup as a fallback.
- **Socket-roundtrip integration test for SetNickname.** Spawns the
  daemon, opens two clients, sends a SetNickname, asserts both
  clients receive the Response and the SensorListBroadcast with
  the updated device_label, and verifies the persisted
  `hardware.json` on disk.
- Tests: 147 → 203.

## [0.4.0] — 2026-05-26

Sprint that landed themes (Grexa-aligned 13-palette catalog with
ComboBox dropdown), multi-dashboard authoring (per-file storage,
slug routing, rename/duplicate/delete, sidebar list with live
filter from the canvas), a Start Page preference, the
Licenses/Credits page rewrite to match Grexa's anatomy with real
cargo-about data, drag-mechanics fixes (native Qt drag.target +
repaint-during-drag pause), mold + sccache wiring, and the
Intel Arc VRAM-total sensor via PCI BAR2 size. Codex peer review
of the themes + dashboards sprint surfaced 11 findings — all
addressed under the entry below.

### Themes + dashboards hardening (post-review pass)

Eleven findings from a codex peer review of commit
`4265ead` — themes + multi-dashboard sprint — addressed.

Critical/High:

- **Slug path-traversal.** Every public method on `DashboardsModel`
  now resolves disk paths through `dashboard_path(slug)`, which
  validates against the `[a-z0-9-]{1,40}` slug grammar before
  joining `dashboards/`. Routing keys (`editor:<slug>`,
  `dashboard:<slug>`) are validated in `Main.qml` via the new
  `DashboardsModel.isValidSlug` invokable before navigation — a
  `../etc/passwd`-style URL fragment is rejected at the routing
  boundary instead of reaching `std::fs::remove_file`. Two new
  tests (`slug_validator_rejects_traversal`,
  `dashboard_path_refuses_traversal`) lock the invariant.
- **Error-sentinel string returns.** `create` / `rename` /
  `duplicate` / `save_layout` no longer encode errors as
  `"error: ..."`-prefixed QStrings. They now return an empty
  QString on failure and set a `lastError` qproperty; QML callers
  surface that via the page banner. This brings the dashboards
  surface into line with the "discriminated banner feedback" rule
  in CLAUDE.md.
- **Alpha colors in the wrong byte order.** Translucent values
  (`accent_mute`, `separator_rgba`) shipped as CSS `#RRGGBBAA`,
  but QML parses 8-digit hex as Qt-style `#AARRGGBB`. Every named
  theme rendered the wash as an opaque slab of the text color.
  Fixed and pinned by an `alpha_colors_use_qt_aarrggbb_form` test.
- **Empty dashboard could not be saved.** The Save button was
  gated on `canvasModel.count > 0`, so clearing the canvas and
  saving did nothing — reload resurrected the old tiles. Save is
  now enabled whenever the editor has a valid slug; persisting an
  empty layout `[]` is a legitimate user action.

Medium:

- **Typed tile schema.** `DashboardFile.layout` was
  `serde_json::Value`; any JSON would round-trip, but the viewer
  silently dropped non-array shapes to an empty dashboard. Now
  modeled as `Vec<DashboardTile { id, x, y, w, h }>` with
  boundary validation (`id` non-empty, dimensions positive,
  position non-negative) inside `save_layout`.
- **Stale editor header after rename.** `editingName` re-runs
  `DashboardsModel.nameOf` only when the slug changes; a rename
  that preserved the slug left the header showing the old name.
  Now subscribes to `summaryJsonChanged` and bumps an internal
  tick that all name-derived bindings depend on.
- **TOCTOU + atomic-write races.** `unique_slug` probed `.exists()`
  before write; `write_one` used a fixed `.json.tmp` path. Two
  GUI processes could race each other to allocate the same slug
  or clobber each other's temp. `allocate_unique_slug` now opens
  candidates with `O_CREAT | O_EXCL`; `write_one` uses a
  per-process temp basename (`<slug>.json.tmp.<pid>.<counter>`)
  with an explicit `sync_all` before rename.
- **Migration skipped on stray junk.** The legacy
  `dashboard.json` migration ran only when `read_dir(dashboards/)`
  was empty — any `.tmp.*` leftover would strand the user's
  legacy layout. Now keyed on `list_files().is_empty()` (which
  ignores files whose stem isn't a valid slug), so crashed-write
  remnants don't block the migration.

Low:

- **Non-Latin dashboard names.** `derive_slug` rejected names with
  no ASCII alphanumerics (any Japanese / Chinese / Arabic /
  emoji-only label). It now falls back to `dash-<hash>` for those
  cases — the routing key stays ASCII-safe while the display name
  preserves the original characters.
- **Theme picker keyboard accessibility.** Cards were mouse-only
  `Rectangle`s with a `MouseArea`. Now `Controls.AbstractButton`
  delegates with `Qt::StrongFocus`, `Accessible.role:
  RadioButton`, `Accessible.checked`, and a visible focus ring —
  Tab/Shift-Tab + Space/Enter activates a theme without a mouse.
- **Test env-var swap was not panic-safe.** A closure that
  panicked left `XDG_CONFIG_HOME` pointing at a dropped tempdir
  and poisoned the next test. The closure helper is now a
  `TempXdgConfig` RAII struct that restores via `Drop`.

Workspace test count: **117 → 136** (gained five GUI tests for the
new slug-safety and alpha-hex invariants).

### Themes + multi-dashboard authoring

LinSight gains both built-in color theming and first-class multi-
dashboard support. Two new QObjects expose persistent state to QML:

- **`PreferencesModel`** owns `~/.config/linsight/preferences.json`
  (atomic write, malformed-file backup as `.bad`). Ships seven
  themes: `system` (defers to Plasma), `tokyo-night`,
  `catppuccin-mocha`, `gruvbox-dark`, `solarized-dark`, `dracula`,
  `nord`. Each named theme pins every color role
  (surface0/1/2/sidebar, text, separator, accent +
  accent-mute/text); `system` leaves surface/text empty so
  `DesignTokens` falls back to the live `Kirigami.Theme` values.
- **`DashboardsModel`** owns `~/.config/linsight/dashboards/<slug>.json`
  (one file per dashboard, atomic write). Provides create / rename
  / duplicate / remove / save-layout / load-layout / name-of with
  collision-handling slug derivation. A first-run migration moves
  the legacy `~/.config/linsight/dashboard.json` into
  `dashboards/default.json` and renames the original to
  `.json.migrated` (skipped if a `dashboards/` already exists).

QML surfaces:

- **Settings → Appearance** ships a `ThemePicker` grid showing every
  theme as a swatch (accent dot + surface preview); the active
  theme is bordered and badged.
- **Sidebar** gains a **DASHBOARDS** section between Workspace and
  System: each saved dashboard is a nav row that opens it in
  read-only `DashboardViewPage`; a "New Dashboard" row at the
  bottom opens `NewDashboardDialog`.
- **CanvasEditorPage** is now slug-aware (`editingSlug`,
  auto-load on slug change, save through `DashboardsModel`) and
  carries an overflow menu in the header for **Rename / Duplicate /
  Delete** with confirmation.
- **Page routing** in `Main.qml` recognizes `dashboard:<slug>` and
  `editor:<slug>` keys; the bare `editor` key resolves to the
  active dashboard from preferences (or prompts to create one).
- **`DesignTokens`** now reads every color role through
  `app.preferences.color(role)` and falls back to Kirigami only
  when the model returns an empty hex (the `system` theme path).
- Pages explicitly anchor a `tokens.surface0` Rectangle behind
  their content; `SensorTile` reads `tokens.surface1` for the tile
  body and `tokens.separator` for the border so themed surfaces
  flood every page.

15 new unit tests cover the prefs theme table, the slug derivation
+ uniqueness logic, atomic write/read round-trips, the malformed-
file backup path, and the legacy-dashboard migration (both the
happy path and the skip-when-already-migrated path). A shared
`ENV_GUARD: Mutex<()>` serializes the env-var swaps that the prefs
+ dashboards tests use, since `cargo test` runs them on a thread
pool.

### Plugin ABI v3 — release-mode correctness fix

`LINSIGHT_PLUGIN_ABI_VERSION` bumped **v2 → v3**. The factory
symbol is now `linsight_plugin_v3` (renamed from
`linsight_plugin_v2`); a stale v2 `.so` fails the symbol lookup
at load time rather than silently exchanging incompatible
mirror-type shapes with the host.

**Why the bump:** a manual launch of the v2 release binary found
`cpu.util` rendering with unit `°C` instead of `%` (and NVIDIA
GPU utilization the same). Root cause: stabby 36.2.2'''s
`match_owned` / `match_ref` dispatchers on `#[repr(stabby)]`
tagged enums with mixed unit + payload variants misroute closures
at `opt-level >= 1`. Confirmed via the workspace round-trip tests:

```
cargo test            -p linsight-plugin-sdk unit_round_trips  # pass
cargo test --release  -p linsight-plugin-sdk unit_round_trips  # FAIL: left: Percent, right: Celsius
```

The bug only surfaces in release binaries (everything users
install). `cargo test` defaults to debug, so the original v0.3.0
audit and the 2026-05-25 hardening sprint both missed it.

**The v3 fix:** every former `#[repr(stabby)]` enum (`RUnit`,
`RReading`, `RCell`) restructured into a
`(kind: <Repr>Kind, payload_fields)` struct. The discriminant
moves into an explicit `#[repr(u8)]` unit-only enum (unaffected
by the bug); payloads become plain struct fields selected by
`kind`. `From`/`Into` impls dispatch via trivial Rust `match`
on `kind` — no stabby-generated matcher involved.

Full rationale, before/after wire shapes, and the upstream-reporting
plan: [`docs/adr/0001-plugin-abi-stabby-deferral.md`](docs/adr/0001-plugin-abi-stabby-deferral.md)
§ "What we learned at v3 — the stabby release-mode matcher bug".

Workspace tests: **117 pass** in both debug AND release after the
fix (was: 117 pass debug, 2 fail release — `unit_round_trips`,
`reading_scalar_round_trips`).

### Editor stability + UX fixes (post-v3 launch)

Surfaced by actually running the v3 release binary on the Editor page:

- **Crash on palette drag.** `PaletteRow` used `Drag.Automatic`
  with `Drag.active = true` set synchronously inside
  `MouseArea.onPressed`. That mode triggers a synchronous Qt
  MIME drag operation from inside the active mouse grab, which
  races the Wayland compositor'''s grab and crashes the
  process. Switched to `Drag.Internal` with `drag.target`: the
  drag stays entirely inside the Qt event loop, no compositor
  handoff. The `DropArea` reads the sensor ID from
  `drop.source.sensorId` since Internal mode doesn'''t carry
  MIME data.
- **Drag proxy positioned at top-left of page instead of cursor.**
  The proxy is parented to `page` (so it can float over the
  canvas during the drag without being clipped by the palette
  `ScrollView`), which means `drag.target` translates it in
  `page` coords, not in the `MouseArea`'''s local coords. Without
  a seed position the proxy starts at `(0, 0)` and drift-translates
  from there — visible as a pill stuck in the top-left of the
  window during drag. Fix: `onPressed` projects the press point
  through `mapToItem(page, …)` and seeds `dragProxy.x/y` so the
  proxy centers on the cursor.
- **Drop didn'''t fire on release.** `Drag.Internal` requires an
  explicit `Drag.drop()` call to trigger the
  `DropArea.dropped()` signal. Added to
  `MouseArea.onReleased`.
- **Palette scroll snapped back to top every sample tick.**
  `refreshSensors()` was rebuilding `page.sensors` as a fresh
  array on every `tilesJsonChanged` (effectively every sample),
  which invalidated the `ListView`'''s model reference and
  reseated all delegates, resetting `contentY` to 0. Split the
  catalogue (stable; rebuilt only when the ID-set changes) from
  the live values (`page.valueById` lookup updated every tick).
  Palette delegates bind their displayed value to
  `valueById[id]` so values refresh in place without invalidating
  the model. Added `cacheBuffer: 5×row` so a scroll re-entering
  a previously-realized row is instant.

### Audit-driven hardening (post-v0.3.0 peer review)

Two consecutive in-depth peer reviews followed by a hardening sprint:
a file-by-file audit and a commit-by-commit audit. Together they
raised the test count from **87 → 117** and closed every Critical and
High finding. Full punch lists:
- File-by-file: [`docs/superpowers/plans/2026-05-25-code-review-punch-list.md`](docs/superpowers/plans/2026-05-25-code-review-punch-list.md)
- Commit-by-commit: [`docs/superpowers/plans/2026-05-25-commit-review-punch-list.md`](docs/superpowers/plans/2026-05-25-commit-review-punch-list.md)

#### Security
- Removed the `shell:<cmd>` alert notify target that passed user-config
  TOML to `sh -c` unescaped (RCE for anyone able to write the alerts
  file). Replaced with an `exec:<argv>` target that uses argv-split +
  direct exec, never invoking a shell. (`apps/linsightd/src/alerts.rs`)

#### Correctness
- Daemon transport now reports the real plugin list and per-sensor
  `plugin_id` in `Welcome` / `SensorList`. Previously hardcoded
  `"io.visorcraft.linsight.cpu"` for every sensor, breaking any client
  that filtered by plugin. (`apps/linsightd/src/transport/unix.rs`)
- CLI `read` now bails with "sensor not found" instead of hanging
  forever when given an unknown sensor name. (`crates/linsight-cli/src/commands/read.rs`)
- CLI `plugin new` scaffold now emits a `path = "../linsight/..."`
  dependency instead of the unpublished `linsight-plugin-sdk = "0.3"`
  registry dep; generated plugins compile out of the box.
- Plugin SDK validates every plugin-supplied sensor ID at `host_init`
  via `SensorId::try_new`; a release-mode plugin returning an empty or
  whitespace ID is now rejected instead of silently entering the
  registry. (`crates/linsight-plugin-sdk/src/plugin.rs`)
- Dashboard `migrate()` rebuilt as a real registry walking v0 → v1 →
  …; was previously a stub that would have rejected every existing
  user config the moment `DASHBOARD_SCHEMA_VERSION` was bumped.
  `CoreError` gained `Io`, `Serialize`, `UnsupportedSchema` variants so
  `load()`/`save()` no longer tunnel everything through
  `InvalidSensorId`. (`crates/linsight-core/src/{dashboard,error}.rs`)
- `mem` sensor falls back to `MemFree` (with a warning) when
  `/proc/meminfo` lacks `MemAvailable`. Previously silently reported
  100% memory used in containers and old kernels.
  (`crates/linsight-sensors/mem/src/meminfo.rs`)
- NVML `processes` table now returns an error when **both** the
  compute- and graphics-process queries fail; previously returned an
  empty table indistinguishable from "no processes running." Logs a
  warning if only one of the two fails. (Phase 34 commit `03ab588` follow-up.)
- xe `freq_mhz` sensor renamed to `freq_hz` — value, unit, and ID were
  in three-way disagreement (Hz value + MHz name + Hz unit declaration).
- xe `enumerate()` now returns `Ok(vec![])` instead of an error when
  `/sys/class/drm` is absent — matches the no-hardware-degrades-gracefully
  contract that nvme and nvml already uphold.
- GUI client now verifies the daemon's `protocol_version` in
  `Welcome` (CLI already did; daemon checks the client's `Hello`).
- GUI no longer panics on non-UTF-8 `XDG_RUNTIME_DIR` paths
  (`socket.to_str().unwrap()` → pass `&Path` directly to `Command`).
- Daemon scheduler clamps non-finite / non-positive effective sampling
  rates; previously a plugin returning a 0 native rate parked the
  sensor at `u64::MAX` micros and silently stopped sampling.
- Daemon scheduler now backs off exponentially on consecutive
  `PluginError::Unsupported` results instead of per-tick log-spamming a
  removed device.
- Prometheus exporter takes its sensor snapshot under a single
  scheduler lock so all series in one `/metrics` response share a
  consistent instant. Previously re-acquired per sensor and interleaved
  with the pump thread, violating the Prometheus scrape contract.

#### Reliability
- Tunnel grew real graceful shutdown via Ctrl+C / SIGTERM with a
  `DRAIN_TIMEOUT`; in-flight TLS sessions are allowed to close cleanly
  with `close_notify` before abort. (Previously the `tokio` `signal`
  feature was enabled but never imported.) Client-mode also removes
  its Unix socket on the way out via a `Drop` guard.
- Tunnel grew a `--max-connections` cap (default 64) with a
  `Semaphore`. Excess connections are rejected before TLS auth so a
  burst can't pre-auth-DoS the tunneled daemon.
- Daemon transport caps concurrent client sessions (default 64),
  applies an accept-loop exponential backoff (100ms → 5s) on persistent
  errors, and gates the initial `Hello` read on a 5-second timeout so
  silent peers can't park a worker thread indefinitely.
- Daemon's Prometheus exporter thread now responds to a shutdown flag
  the runtime holds and uses a non-blocking accept loop with poll
  interval — previously a fire-and-forget thread that ran to process exit.
- Daemon history writer's `JoinHandle` is now retained by the runtime
  so a writer-thread crash is observable on shutdown; the final flush
  before exit now logs on failure instead of silently dropping the
  last batch.
- GUI now surfaces a "Disconnected from linsightd" banner when the
  sample stream ends. Tiles previously froze at last value with no
  indication anything was wrong. New `OverviewModel.connected: bool`
  qproperty.
- `linsight-cli plugin install`, `ls`, and `remove` now hard-error
  when neither `XDG_DATA_HOME` nor `HOME` is set instead of falling
  back to a CWD-relative `./linsight-plugins` directory that the
  daemon doesn't look at.
- `PluginHost::Drop` now invokes each plugin's `shutdown()` hook so
  plugins owning background threads or hardware handles get a chance
  to release them.

#### GUI & UX
- Wired the "New Window" trigger that was missing for the shipped
  multi-window feature: sidebar item + `Ctrl+N` shortcut, child window
  tracked via `Main.qml`'s `extraWindows` so it survives QML GC.
- Renamed `DashWindow.qml`'s `dashboardModel` property to `dashModel`
  so its child pages actually receive the model (previously every
  page inside a secondary window stayed stuck on "…").
- Canvas-editor palette drag rewritten: dropped the `MouseArea.drag.target`
  + `Drag.Automatic` mix that made the visual proxy "teleport" rather
  than follow the cursor. Default tile size + drop-centering offset
  extracted to `defaultTileW`/`defaultTileH` constants.
- Canvas-editor tile drag now clamps to canvas bounds during move,
  matching the snap-on-release behavior.
- Justfile `i18n-extract` target now covers every QML file that
  contains `qsTr()` (was 3 of 13+).
- Added the `just credits` recipe referenced by the Credits page.
- `CategoryPage`'s "-1 sentinel" filter narrowed from `indexOf("-1") === 0`
  (which hid legitimate negative readings like "-1.0°C") to exact
  match against the units the kernel actually writes -1 to.
- Renamed `dashModel` property to match across all child page declarations.

#### Wire format / ABI
- `linsight-protocol` `FrameWriter::write_frame` now checks size
  before the `usize → u32` cast; bodies exactly at 2^32 would otherwise
  truncate to 0 and slip past the oversized guard.
- Documented enum variant ordering as wire-format-stable in
  `linsight-protocol`; appending only.

#### Tests + dev infrastructure
- Test count up from **87** to **117** (one ignored hardware-gated
  test added per the AGENTS.md convention).
- Removed unmarked live-`/proc` reads from `cpu`/`mem` test suites
  (now `#[ignore]` per the AGENTS.md hardware-gated convention).
- Added end-to-end mTLS smoke for `linsight-tunnel`
  (`tests/mtls_smoke.rs`) — generates a self-signed CA + server +
  client cert pair with `rcgen`, exercises the rustls + ring + TLS 1.2
  + 1.3 stack, asserts both happy-path round-trip and rogue-client
  rejection. Closes the open follow-up.
- **Added `examples/echo-plugin/` + `crates/linsight-plugin-sdk/tests/dynamic_load.rs`.**
  Closes the "fabricated test claim" gap in commit `8c301d5`: the
  test builds the example cdylib via escargot, dlopens it, and
  exercises the same `StabbyLibrary::get_stabbied` load path the
  daemon uses. Three assertions: `linsight_plugin_abi_version`
  symbol, factory signature acceptance, full `host_init` /
  `host_sample` round-trip.
- Added `net` sensor sample-path tests (`rx_bytes`, `tx_bytes`,
  `link_state`, `speed_mbps`); previously only `enumerate()` was
  tested.
- Added `nvml` crate tests for `parse_sensor_id` and the
  `comm_for_pid` helper (the Phase 34 process-table path was untested).
- Added FFI validation tests in `linsight-plugin-sdk`:
  `host_init_rejects_plugin_with_invalid_sensor_id` proves a
  release-mode plugin emitting a whitespace-bearing sensor ID is
  rejected; `plugin_ctx_rejects_non_utf8_sysroot` proves the new
  `PluginCtx::new_with_sysroot` constructor refuses non-UTF-8 paths.
- Added `manifest_with_many_sensors_preserves_order` — regression
  guard for the pop-twice anti-pattern fix.
- CLI test helper `wait_for_socket` now polls accept-readiness via
  `UnixStream::connect`, not just file `exists()`, fixing intermittent
  connection-refused races on CI.
- History flush test no longer races on a 200ms `thread::sleep` —
  joins the writer thread deterministically via the new handle.
- `scripts/gui_smoke.sh` now distinguishes timeout (exit 124) from
  clean exit, uses GNU automake's skip convention (exit 77) when
  `xvfb-run` is missing, and reports the binary exit code in the
  pass message so a hung-but-handshook GUI isn't conflated with a
  clean run.

#### Commit-review follow-ups

A second peer review covered the last 10 pre-audit commits and
flagged a separate batch of issues. Resolved here:

- **GUI**: `showResult`'s fragile error/success string-prefix sniff
  replaced with discriminated `showSuccess(msg)` / `showError(msg)`
  + a centralized `isLayoutError()` helper. All six pages now
  declare `dashModel` as `property QtObject` (typed) rather than
  `property var`. Page header height extracted to
  `app.tokens.pageHeaderHeight` (was copy-pasted as `height: 76`
  across 6 files). AboutPage hero stopped hard-coding hex colors —
  uses new `markPanelDeep/Top/Bar` design tokens. The
  `github.com/visorcraft/linsight` label is now a real clickable
  link with `Accessible.role: Link`.
- **GUI a11y**: `NavItem` now focusable via `Tab`, actionable via
  Enter / Space, has `Accessible.role: MenuItem` + name, and shows
  a visible focus border. Settings/About/Licenses cards got
  `Accessible.*` markup where they were missing.
- **GUI translation**: `CategoryPage`'s `"%1 sensor%2"` plural
  pattern replaced with two separate `qsTr` strings for proper
  language-by-language translation.
- **Settings status fidelity**: the env-var indicators on the
  Settings page now reflect actual on/off state via a new
  `OverviewModel.envIsSet(name)` invokable, instead of always
  showing the same checkbox-symbolic regardless. Env-var names
  now read from `readonly property string` constants instead of
  string literals.
- **CLI surface**: `apps/linsight-gui/src/main.rs` now uses clap
  for argument parsing. `--reduce-motion` (+ alias
  `--no-animations`), `--screenshot`, `--screenshot-delay`,
  `--connect`, and the initial-page positional all appear in
  `--help` and shell completion. `--screenshot-delay` is `u32`
  clamped to `[0, 30000]` (was `i32` accepting silent negatives).
  Screenshot destination is path-validated before Qt boots.
- **Screenshot internals**: `screenshot.cpp` magic numbers
  (20 retries / 100ms poll / 50ms post-save settle) extracted to
  named `constexpr` constants with explanatory comments.
  `dev_screenshot.sh` now omits `--screenshot-delay` so the script
  and Rust default can't drift.
- **Tunnel hardening**: default bind changed from `0.0.0.0:9443`
  to `127.0.0.1:9443` (operators who want the public bind pass it
  explicitly). `tokio` / `rustls` / `tokio-rustls` /
  `rustls-pemfile` / `rustls-pki-types` moved to
  `[workspace.dependencies]` so future async crates inherit the
  same revisions. New `apps/linsight-tunnel/README.md` with
  topology diagram, openssl cert recipe, and trust-model caveats
  (the configured CA == full daemon access; no per-cert filter
  yet, documented as the next tightening).
- **Plugin-SDK polish**: doc-comments added to every public R-type
  in `mirror.rs`. `host_init` and `host_sample` gained `#[must_use]`
  with prose explaining the consequence of discarding the result.
  Sensor-shim style unified across all six in-tree sensor crates
  (was two competing styles). `PluginCtx::new()` removed (was
  redundant with `Default`). The pop-twice anti-pattern in
  `svec_into_std` rewritten as a single forward-drain; the
  duplicate inline impl in `manifest.rs` now calls the shared
  helper.
- **Wire format**: `linsight-protocol/src/messages.rs` gained an
  explicit comment block warning that variant order is postcard
  wire-format-stable — appending only.
- **Packaging**: version drift in Arch / Arch-v3 / AppImage /
  metainfo / debian / opensuse / fedora packaging files (all
  still at `0.1.0` after the v0.3.0 bump) corrected to `0.3.0`
  with appropriate release notes.
- **Codebase hygiene**: `about.toml`'s skip-list of workspace
  members had three phantom crates (`linsight-containers`,
  `linsight-ai`, `linsight-i18n`) that don't exist. Replaced with
  the real workspace list. Dead `linsight-16.png` /
  `linsight-48.png` (committed but never registered in the qrc
  bundle) removed. Misleading docs in `build-and-test.md`
  describing CI infrastructure that doesn't exist rewritten
  honestly. ADR-0001 got the missing "Consequences" section.

#### Removed
- `shell:<cmd>` alert notify target. Replaced with `exec:<argv>`
  (see Security above). The old `shell:` prefix logs a clear "use
  exec instead" warning if a stale config still references it.
- `PluginCtx::new()`. Use `PluginCtx::default()` (identical) or
  `PluginCtx::new_with_sysroot(path)` (validated).
- Dead PNG assets in `apps/linsight-gui/resources/`
  (`linsight-16.png`, `linsight-48.png`) that were committed but
  never registered.

## [0.3.0] — 2026-05-25

All 10 spec phases plus the post-v0.3.0 polish sprint. See
[`docs/superpowers/plans/2026-05-25-phases-roadmap.md`](docs/superpowers/plans/2026-05-25-phases-roadmap.md)
for the rollout history and
[`docs/superpowers/plans/2026-05-25-v0.3.0-followups-completion-notes.md`](docs/superpowers/plans/2026-05-25-v0.3.0-followups-completion-notes.md)
for what shipped and what was originally deferred.

Highlights:
- Daemon, CLI, Qt 6 / Kirigami GUI with sidebar navigation
- Preset pages: Overview, GPUs, Storage, Network
- Phase 6b custom-canvas editor
- Always-on mode (env-gated): SQLite history, Prometheus exporter, alert engine
- NVIDIA + Intel xe + NVMe + network sensors
- Runtime `.so` plugins; **ABI v2** via stabby R-mirror types
- `linsight-tunnel` mTLS bridge for non-SSH remote topologies
