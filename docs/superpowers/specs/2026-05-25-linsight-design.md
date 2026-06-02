<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# LinSight v1 — Design

**Status:** Draft for implementation planning
**Date:** 2026-05-25
**Author:** VisorCraft (with Claude Opus 4.7 as collaborator)
**Target ship horizon:** ~4–6 months (ambitious v1)

## Summary

LinSight ("Linux Insight") is a fast, beautiful, modular system-monitoring
dashboard for Linux. It targets KDE Plasma 6 as its primary desktop and
fills a gap left by Mission Center and the Plasma System Monitor: a
single cumulative dashboard view of CPU, RAM, swap, multiple GPUs (Intel
xe, NVIDIA, AMD), NVMe drives, and network — extensible at runtime via
dynamic-library plugins so contributors can add new hardware support
without waiting for an upstream release.

LinSight is structured as a thin Qt 6 / Kirigami GUI talking to a
lightweight Rust daemon (`linsightd`) over a Unix socket. The daemon is
spawned as a child of the GUI by default and exits when the GUI does —
zero idle cost when LinSight isn't running. An opt-in systemd user
service enables persistent operation for alerts, Prometheus export,
and long-term history.

## Goals

1. **Cumulative multi-device dashboard.** A single Overview page shows
   every CPU, GPU, NVMe, and network interface on the machine. No
   per-device tabs.
2. **Fast & low-resource.** Subscription-driven sampling: no sensor is
   sampled unless a client is watching it. Daemon idle RSS ≤ 7 MB; CPU
   < 0.05% when idle; < 0.5% with the Overview page visible.
3. **Beautiful.** Native Plasma 6 look via Kirigami 6; tasteful default
   theme; light/dark + accent color support; meaningful animations
   only (no decorative motion).
4. **Modular.** Hardware support is plugin-shaped end-to-end — built-in
   sensors and third-party `.so` plugins implement the same public
   `LinsightPlugin` trait via a stable ABI (`stabby`). Contributors
   can ship a plugin without touching LinSight source.
5. **Ambitious v1 surface.** History graphs (5 min in-memory + opt-in
   SQLite for long-term), alert rules with notifications, Prometheus
   exporter, remote dashboards over SSH, per-GPU process list.
6. **Honest about scope it doesn't have.** No plugin sandbox in v1
   (documented). No native GTK/GNOME frontend (Kirigami only).

## Non-goals

- Replacing `htop` / `btop` as a process explorer.
- Cross-platform support (macOS, Windows). Linux only.
- A web UI. Native Qt only; Prometheus exporter is the structured-data
  interop point.
- Editing system state (kill processes, adjust fan curves). Read-only.
- Container-aware metrics (cgroup-per-container drilldown). v2.

## Architecture

### Process model

Two run modes from one binary, plus an opt-in service:

| Invocation | Role |
|---|---|
| `linsight` | Launches the GUI; spawns `linsight --daemon` as a child if no daemon is already listening. Daemon exits when GUI exits. |
| `linsight --daemon` | Headless sensor host. Binds `$XDG_RUNTIME_DIR/linsight.sock`. |
| `linsight-cli …` | CLI that talks to the daemon, or spawns a one-shot daemon for a single read. |
| `systemctl --user enable --now linsight` | Opt-in always-on daemon via the shipped `linsight.service` user unit. Enables alerts, Prometheus, and SQLite-backed history. |

GUI startup logic:

1. Try connecting to `$XDG_RUNTIME_DIR/linsight.sock`.
2. If a daemon answers (always-on mode), attach as a client.
3. Else fork `linsight --daemon` as a child, wait for the socket, attach.
4. On clean exit, signal the daemon to shut down — only if the daemon
   is our child.

Remote dashboards talk to a remote `linsight.service` over an
SSH-forwarded socket — same wire protocol, no remote-specific code path
in the daemon.

### Workspace layout

```
linsight/
├── apps/
│   ├── linsight-gui/                 # Qt 6 / Kirigami binary (crate: linsight)
│   │   ├── Cargo.toml
│   │   ├── build.rs                  # CxxQtBuilder + QML module bundling
│   │   ├── src/
│   │   │   ├── main.rs               # boots QGuiApplication + spawns/attaches daemon
│   │   │   ├── client.rs             # postcard-over-socket client
│   │   │   ├── qobjects/             # cxx_qt bridges per view
│   │   │   └── workspace.rs          # shared stores + Fluent bundle
│   │   └── qml/                      # bundled at build time → qrc:/qt/qml/...
│   └── linsightd/                    # daemon binary (crate: linsightd)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs               # CLI entry + socket binding
│           ├── runtime.rs            # sync event loop (`polling` crate)
│           ├── scheduler.rs          # per-sensor rate, subscription bookkeeping
│           ├── plugin_host.rs        # stabby dylib loader + lifecycle
│           ├── history.rs            # SQLite TSDB (always-on only)
│           ├── alerts.rs             # expression engine (always-on only)
│           ├── prom.rs               # /metrics HTTP (always-on only)
│           └── transport/            # Unix socket + ssh-forwarded + mTLS
├── crates/
│   ├── linsight-core/                # shared types (SensorId, Sample, DashboardSpec)
│   ├── linsight-protocol/            # postcard wire types + version handshake
│   ├── linsight-plugin-sdk/          # stabby traits public to plugin authors
│   ├── linsight-sensors/             # built-in sensor backends (in-tree plugins)
│   │   ├── nvml/
│   │   ├── xe/
│   │   ├── cpu/
│   │   ├── mem/
│   │   ├── nvme/
│   │   └── net/
│   ├── linsight-cli/                 # CLI binary
│   └── linsight-i18n/                # Fluent bundle (en + de + ja day 1)
├── docs/
│   ├── architecture.md
│   ├── plugin-sdk.md                 # public contract for third-party authors
│   ├── wire-protocol.md
│   ├── build-and-test.md
│   ├── packaging.md
│   ├── security.md
│   └── perf-budgets.md
├── packaging/
│   ├── arch/                         # x86_64 + x86_64-v3 PKGBUILDs
│   ├── debian/
│   ├── fedora/
│   ├── opensuse/
│   ├── flatpak/
│   ├── appimage/
│   ├── icons/
│   ├── systemd/linsight.service      # user unit for always-on mode
│   ├── com.visorcraft.LinSight.desktop
│   └── com.visorcraft.LinSight.metainfo.xml
├── scripts/
├── tests/
├── AGENTS.md
├── CONTRIBUTING.md                   # plugin authoring + contribution flow
├── README.md
├── LICENSE                           # GPL-3.0-only
├── Cargo.toml                        # workspace manifest
├── Cargo.lock
├── rust-toolchain.toml               # pinned latest stable at project kickoff
├── rustfmt.toml
├── deny.toml
└── Justfile
```

### Lifecycle

```
Default (interactive) mode:
  user runs `linsight`
    → linsight (GUI) forks linsight --daemon
      → daemon binds $XDG_RUNTIME_DIR/linsight.sock
      → GUI attaches, subscribes to current page's sensors
  user closes window
    → GUI sends "Goodbye", disconnects
    → daemon detects no more clients, exits
    → 0 processes, 0 RAM, 0 CPU

Always-on mode:
  user runs `systemctl --user enable --now linsight`
    → systemd starts linsight.service = linsight --daemon
    → daemon listens on socket, ready for clients + Prometheus + alerts
  user later runs `linsight` (GUI)
    → GUI sees socket already bound, attaches as another client
    → daemon serves GUI subscriptions + alert eval + Prometheus scrapes
  user closes GUI
    → daemon keeps running; alerts + Prometheus continue
  user runs `systemctl --user disable --now linsight`
    → 0 processes, 0 cost again
```

### Crate boundaries

- **`linsight-core`** owns types and pure logic: `SensorId`, `Reading`,
  `DashboardSpec`, dashboard migration, alert AST, history schema.
  No I/O, no async, no Qt.
- **`linsight-protocol`** is the wire-format crate. `postcard`-serializable
  messages + protocol version constant. Used by both `linsightd` and the
  GUI client.
- **`linsight-plugin-sdk`** is the public crate plugin authors compile
  against. One trait (`LinsightPlugin`), a few value types, and the
  `export_plugin!` macro. ABI is guarded by a `LINSIGHT_PLUGIN_ABI_VERSION`
  constant; mismatches refuse to load.
- **`linsight-sensors`** holds the in-tree sensor backends. Each
  sub-module (cpu, mem, nvml, xe, nvme, net) implements
  `LinsightPlugin`. Statically linked into `linsightd`; same trait,
  zero second code path.
- **`linsightd`** is the daemon binary. Hosts the scheduler, plugin
  host, transport, and the always-on subsystems (history, alerts,
  Prometheus). No GUI deps.
- **`linsight-cli`** is the CLI. Connects to a running daemon or spawns
  a one-shot. Subcommands: `list`, `read`, `subscribe` (stream),
  `plugin {new,install,remove,list}`, `dashboard {export,import}`,
  `tap` (human-readable wire log).
- **`linsight-i18n`** is the Fluent bundle (mirrors Grexa). en is
  source of truth; de + ja ship day 1.
- **`linsight`** (in `apps/linsight-gui`) is the Qt binary. Thin
  client; no algorithmic code. QML talks to cxx-qt-generated QObjects;
  the postcard client lives in a worker thread that hops signals back
  via `cxx_qt::Threading::queue`.

## Plugin SDK & ABI

### The trait

```rust
// crates/linsight-plugin-sdk/src/lib.rs

use stabby::prelude::*;

pub const LINSIGHT_PLUGIN_ABI_VERSION: u32 = 1;

#[stabby::stabby]
pub trait LinsightPlugin {
    fn init(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError>;
    fn sample(&self, sensor: SensorId) -> Result<Reading, PluginError>;
    fn shutdown(&self) {}
}

#[stabby::stabby]
pub struct PluginManifest {
    pub plugin_id: RString,
    pub display_name: RString,
    pub version: RString,
    pub sensors: RVec<SensorDescriptor>,
}

#[stabby::stabby]
pub struct SensorDescriptor {
    pub id: SensorId,
    pub display_name: RString,
    pub unit: Unit,
    pub kind: SensorKind,           // Scalar | Counter | Table | State
    pub category: Category,          // Cpu | Gpu | Memory | Storage | Network | Custom
    pub native_rate_hz: f32,         // hint; clamped to [0.1, 20.0]
    pub min: ROption<f64>,
    pub max: ROption<f64>,
    pub device_id: ROption<RString>,
}

#[stabby::stabby]
pub enum Reading {
    Scalar(f64),
    Counter(u64),
    Table(RVec<TableRow>),
    State(RString),
}
```

The `export_plugin!` macro generates the `extern "C"` boilerplate:

```rust
#[no_mangle] pub extern "C" fn linsight_plugin_abi_version() -> u32 { 1 }
#[no_mangle] pub extern "C" fn linsight_plugin_v1() -> DynPtr<dyn LinsightPlugin>
```

…plus `catch_unwind` wrappers on every method.

### Loading flow

1. On startup and on SIGHUP, the daemon scans (in order):
   `/usr/lib/linsight/plugins/`, `/usr/local/lib/linsight/plugins/`,
   `$XDG_DATA_HOME/linsight/plugins/`.
2. For each `*.so`: `dlopen`, call `linsight_plugin_abi_version()`. On
   mismatch: log a warning, `dlclose`, skip.
3. Call `linsight_plugin_v1()` → `DynPtr<dyn LinsightPlugin>`.
4. Call `plugin.init()` inside `catch_unwind`. On panic: log, `dlclose`,
   skip.
5. Register sensors. On `SensorId` collision: first wins, log conflict.
6. Hold the dylib handle for the life of the daemon or until SIGHUP
   reloads it.

### Crash handling

Every plugin call goes through `catch_unwind`. On a `sample()` panic:

- The faulting sensor is marked `Degraded` and unsubscribed.
- The wire protocol emits `SensorDegraded { sensor, reason }`; the GUI
  dims the tile.
- The plugin is not unloaded — other sensors keep working.
- 3 consecutive panics from any sensor in the same plugin within 60s
  unloads the whole plugin and reports it.

Repeated `init()` panics → the plugin is quarantined in
`$XDG_STATE_HOME/linsight/quarantine.json` until the user removes it.

### Plugin author UX

```
linsight-cli plugin new my-sensor
# scaffolds:
#   my-sensor/
#   ├── Cargo.toml          # cdylib target, depends on linsight-plugin-sdk = "1.x"
#   ├── src/lib.rs          # skeleton with LinsightPlugin impl + export_plugin!
#   └── README.md           # build / install / submit-PR instructions

cargo build --release
# produces target/release/libmy_sensor.so

linsight-cli plugin install ./target/release/libmy_sensor.so
# copies to $XDG_DATA_HOME/linsight/plugins/ and SIGHUPs the daemon
```

### Security model

`.so` plugins run in-process inside the daemon. **No sandbox in v1.**
Documented in `docs/security.md`:

- Plugins from `$XDG_DATA_HOME` are user-trust.
- Plugins from `/usr/lib/` are distro-trust.
- First-launch shows a one-time dialog listing detected **third-party**
  plugins (anything outside `/usr/lib/linsight/plugins/` is treated as
  third-party for the prompt; built-in and distro-shipped plugins are
  not enumerated). The user acknowledges per plugin; rejection
  quarantines that plugin.
- A sandbox (seccomp filter + restricted FS view) is roadmap for v2.

## Wire protocol & data flow

- **Transport:** Unix socket at `$XDG_RUNTIME_DIR/linsight.sock`.
  Length-prefixed `postcard` frames (4-byte LE u32 length + body).
  One persistent connection per client.
- **Handshake:** client sends `Hello { protocol_version, client_name }`;
  daemon replies `Welcome { protocol_version, daemon_version, plugins }`.
  Mismatched protocol → disconnect with reason.
- **Subscriptions:** `Subscribe { sensors, rate_hz: Option<f32> }` and
  `Unsubscribe { sensors }`. Daemon schedules each sensor at
  `min(plugin_native_rate, requested_or_native)`. Sampling stops when
  the last subscriber leaves.
- **Streams:** daemon pushes `Sample { sensor, ts_micros, reading }`
  frames. No request/response per sample.
- **Control plane:** `ListSensors`, `ListPlugins`, `ReloadPlugins`,
  `GetHistory { sensor, from, to }`, `SetAlertRule`, `GetAlertState`.
- **Remote transport:** identical protocol, two wrappers — SSH-forwarded
  (`ssh -L`) or `linsight-tunnel` (an in-tree mTLS proxy). Daemon
  itself never speaks TLS or SSH.

## Built-in sensor coverage (v1)

| Sub-module | Sensors |
|---|---|
| `cpu` | per-core util, per-core freq, package temp, load avg (1/5/15m), context switches, ctx/sec |
| `mem` | RAM used/total, swap used/total, cached, buffers, dirty, available |
| `nvml` | per-GPU util, mem used/total, temp, power, fan, P-state, per-process usage (Table) |
| `xe` | per-tile engine util (RCS/CCS/VCS/VECS), per-tile freq, hwmon temp/power/fan, vram used/total (where exposed), DRM clients (Table) |
| `nvme` | per-namespace temp (composite + per-sensor), bytes-read/written counters, host-write-cmds, percent-used, critical-warning (State) |
| `net` | per-interface rx/tx bytes counters, link speed, link state |

Each sub-module is its own crate; statically linked into `linsightd`
by default, can be split out as a real `.so` if a distro needs it.

## Dashboard model & persistence

- **`DashboardSpec`** is the in-memory + on-disk shape: ordered list of
  `Page`, each with `kind: Preset | Custom`, `widgets`, `title`.
- **Preset pages** shipped: `Overview`, `GPUs`, `Storage`, `Network`.
  Widget lists are computed at runtime from available sensors —
  hardware that isn't detected doesn't render an empty slot.
- **Custom pages** use a snap-to-grid canvas (24-col × N-row).
  `WidgetPlacement { kind, col, row, w, h, sensor_bindings, options }`.
  Widget kinds in v1: `Gauge`, `Sparkline`, `Bar`, `TextValue`,
  `Donut`, `Table`, `MultiSparkline`.
- **Persistence:** `~/.config/linsight/dashboard.json` (JSON for
  human-editability). Schema versioned; migrations live in
  `linsight-core::dashboard::migrate`.
- **First-launch flow:** Overview preset is shown. "Customize" reveals
  the canvas editor; "+ New page" creates a custom page seeded from
  the current Overview.
- **Multi-window:** the GUI supports multiple top-level windows in a
  single process (one `QQuickWindow` per dashboard view, sharing the
  same daemon connection and Fluent bundle). Useful for displaying
  the Overview on a secondary monitor while a custom GPU-focused page
  is on the primary. Each window opts into its own page; subscriptions
  are deduplicated at the client.

## Always-on features (gated behind `linsight.service`)

### History (SQLite WAL)

`~/.local/share/linsight/history.db`:

```sql
CREATE TABLE samples (
    sensor_id TEXT NOT NULL,
    ts        INTEGER NOT NULL,    -- microseconds since epoch
    scalar    REAL,
    counter   INTEGER,
    state     TEXT,
    PRIMARY KEY (sensor_id, ts)
) WITHOUT ROWID;

CREATE INDEX samples_ts ON samples(ts);
```

Retention is configured in `~/.config/linsight/retention.toml`. The
default policy is global (24h at full rate, then 1-min averages for 30d,
then drop), with per-sensor overrides by glob pattern:

```toml
default = { full = "24h", downsampled = "30d", bucket = "1m" }

[[override]]
match = "xe.gpu*.temp_c"
full = "7d"
downsampled = "180d"
bucket = "5m"
```

A downsampling worker thread inside `linsightd` runs once per minute.
Hot-path writes are batched (1s flush window) on a dedicated
low-priority thread; never blocks the sample loop.

### Alerts

TOML rules in `~/.config/linsight/alerts.toml`:

```toml
[[rule]]
name = "B70 too hot"
expr = "xe.gpu1.temp_c > 85"
for  = "30s"
notify = ["desktop", "shell:notify-send 'B70 hot: {{value}}°C'"]
```

Expression engine: the `evalexpr` crate. `desktop` notifications go
through `notify-rust`; `shell:` triggers a shell command with the
rule context interpolated.

### Prometheus exporter

HTTP `/metrics` on a configurable bind (default `127.0.0.1:9777`),
off unless explicitly enabled in
`~/.config/linsight/prometheus.toml`. Each registered sensor emits
`linsight_<sanitized_id>{device="...",unit="..."} <value>`.

**Interaction with subscription-driven sampling:** Prometheus pulls
don't naturally generate subscriptions, so the exporter registers a
synthetic always-on subscriber covering every sensor named in
`prometheus.toml` (`exported_sensors = ["xe.*.util", "cpu.package.temp_c", …]`).
That synthetic subscriber requests each sensor at its
`native_rate_hz / 4` (so a 4 Hz GPU util sensor samples at 1 Hz for
Prometheus). The exporter serves the most recent cached sample per
sensor on scrape, never blocking on a fresh read. Sensors not in
`exported_sensors` are not sampled by the exporter.

## Remote dashboards

- **Zero-config (SSH):** `linsight --connect ssh://user@host` spawns
  `ssh user@host -L /tmp/linsight-remote.sock:$XDG_RUNTIME_DIR/linsight.sock`
  and attaches the local GUI. Requires `linsight.service` running on
  the remote.
- **mTLS:** `linsight-tunnel` wraps the Unix socket in TLS for
  non-SSH topologies. Cert provisioning is one-shot
  (`linsight-tunnel init`, `linsight-tunnel issue-client`). May be
  cut to v1.1 if scope tightens.

## Packaging & distribution

| Target | Notes |
|---|---|
| `packaging/arch/linsight` | x86_64 generic PKGBUILD |
| `packaging/arch/linsight-v3` | **x86_64-v3 tuned variant** (CachyOS-friendly). Same source, `RUSTFLAGS="-C target-cpu=x86-64-v3"`. |
| `packaging/debian/` | dpkg recipe, installs the user systemd unit |
| `packaging/fedora/` | rpm spec |
| `packaging/opensuse/` | rpm spec |
| `packaging/flatpak/` | Flatpak — full sensor access requires `--filesystem=host:ro` + `--device=all`; ships with those overrides since they're necessary to read `/sys/class/hwmon`. Runtime plugin loading from `$XDG_DATA_HOME` works via the Flatpak host path. Less sandboxed than ideal; documented. |
| `packaging/appimage/` | appimage-builder; bundles Qt 6 + Kirigami |
| `packaging/systemd/linsight.service` | User unit installed, **not enabled by default** |
| `packaging/com.visorcraft.LinSight.desktop` | Desktop entry; launches `linsight` |

## Performance targets

Documented in `docs/perf-budgets.md`. CI runs a benchmark suite that
asserts these don't regress by more than +20%.

| Metric | Budget |
|---|---|
| Daemon RSS, idle (no subs) | ≤ 7 MB |
| Daemon RSS, full Overview + 1 plugin | ≤ 12 MB |
| Daemon RSS, always-on with alerts + Prometheus + SQLite | ≤ 20 MB |
| Daemon CPU, Overview visible | ≤ 0.5% of one core |
| Daemon CPU, idle | < 0.05% (epoll_wait) |
| GUI RSS, Overview visible (Qt + Kirigami baseline) | ≤ 140 MB |
| GUI cold start to interactive | ≤ 700 ms |
| Wire-protocol overhead per sample | ≤ 64 B serialized, ≤ 5 µs encode |
| Subscribe → first sample latency | ≤ 60 ms |

### Build flags

```toml
[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
opt-level = 3
```

The x86_64-v3 packaging variant additionally sets
`RUSTFLAGS="-C target-cpu=x86-64-v3"` via a `.cargo/config.toml`
fragment.

## Versioning & dependencies

- **Rust toolchain:** pinned in `rust-toolchain.toml` to the latest
  stable at project kickoff. Bumped on every stable release that
  doesn't break clippy/fmt.
- **`Cargo.lock`:** committed. Updated weekly/monthly via
  `cargo update`.
- **Cargo deps:** caret ranges (`anyhow = "1.0"` etc.) so patch +
  minor releases flow in automatically.
- **Qt / Kirigami:** system deps. Documented floor: Qt ≥ 6.10
  (cxx-qt 0.8 compatibility), Kirigami ≥ 6.
- **`cxx-qt`:** `0.8` caret; bump intentionally.
- **`stabby`:** latest 1.x at kickoff; the load-time ABI gate is
  `LINSIGHT_PLUGIN_ABI_VERSION`, not the stabby crate version.

Minimum platform documented in README:

- Kernel ≥ 6.11 for mature `xe` driver + Battlemage perf counters.
- NVIDIA driver ≥ 560 for stable RTX 50 NVML.
- Qt ≥ 6.10.
- libdrm ≥ 2.4.121 for xe ioctl support.

## Testing

- **`linsight-core`** and **`linsight-protocol`:** pure unit tests +
  proptest, no Qt, no `/sys` access.
- **`linsight-sensors/*`:** integration tests against synthetic sysfs
  fixtures (a `tempfile`-backed `/sys` mirror written by the test
  harness). NVML / xe-PMU tests are `#[ignore]` by default and run on
  hardware-equipped CI runners only.
- **`linsightd`:** end-to-end tests spawn the daemon against a
  `MockPluginHost`, connect a postcard client, assert subscribe /
  sample / unsubscribe flows.
- **`linsight-gui`:** Rust-side QObject backing-struct tests (Grexa
  pattern). QML tested separately with `qmltestrunner`.
- **Benchmark suite:** `cargo bench` with `criterion`; CI fails on
  >20% regression vs main.

## Dev loop (`Justfile`, mirrors Grexa)

```
just ci             # fmt --check + clippy -D warnings + tests
just bench          # criterion benches
just run            # builds + spawns linsight (GUI auto-spawns daemon)
just run-daemon     # `linsight --daemon` standalone
just run-cli ARGS   # CLI passthrough
just flatpak        # vendor + flatpak-builder
just arch           # builds packaging/arch/linsight
just arch-v3        # builds packaging/arch/linsight-v3
just preflight      # ci + deny + audit + bench (release gate)
```

## XDG paths

| Path | Default | Purpose |
|---|---|---|
| `paths.config_dir` | `$XDG_CONFIG_HOME/linsight` (`~/.config/linsight`) | `dashboard.json`, `alerts.toml`, `prometheus.toml` |
| `paths.data_dir`   | `$XDG_DATA_HOME/linsight` (`~/.local/share/linsight`) | `plugins/*.so`, `history.db` |
| `paths.cache_dir`  | `$XDG_CACHE_HOME/linsight` (`~/.cache/linsight`) | QML pre-compile cache |
| `paths.state_dir`  | `$XDG_STATE_HOME/linsight` (`~/.local/state/linsight`) | `linsight-daemon.log`, `quarantine.json` |
| `paths.runtime_dir` | `$XDG_RUNTIME_DIR/linsight.sock` | Unix socket |

## Open questions to revisit during implementation

- **mTLS remote support** — keep in v1 or punt to v1.1? Decide once
  the postcard transport is built; if SSH-forwarded handles all
  realistic cases, defer.
- **Plugin sandbox (seccomp + namespaces)** — out of v1, but the
  exact phased plan for v2 is worth sketching before plugin API
  stabilization.
- **Native NVIDIA Blackwell (RTX 50) NVML coverage** — depends on
  `nvml-wrapper` keeping pace; if it lags, direct `bindgen` FFI in
  `linsight-sensors::nvml`.

## Decisions log

| Decision | Choice | Rationale |
|---|---|---|
| Process model | Demand-driven daemon spawned by GUI; opt-in systemd user unit | Mission Center's lifecycle proves the model; opt-in covers ambitious-v1 features |
| Plugin ABI library | `stabby` | User preference over `abi_stable`; smaller runtime, modern Rust patterns |
| Sampling cadence | Per-sensor native rate, subscription-driven | Avoids polling what's not visible; user-preferred |
| Wire protocol | `postcard` over Unix socket | Compact, fast; `linsight-cli tap` for debuggability |
| History storage | SQLite WAL | Boring + durable + queryable |
| Alerts engine | `evalexpr` crate | Self-contained, no external deps |
| Dashboard model | Hybrid: preset pages + custom canvas | User-preferred over preset-only or canvas-only |
| Language | Rust | User-confirmed; matches Grexa stack |
| GUI framework | Qt 6 + Kirigami via cxx-qt 0.8 | Mirrors Grexa; native Plasma 6 look |
| License | GPL-3.0-only | Mirrors Grexa |
| Packaging | Flatpak + AppImage + Arch (x86_64 + v3) + Debian + Fedora + openSUSE | Mirrors Grexa; v3 variant for CachyOS users |
| Build profile | `lto=fat`, `codegen-units=1`, `panic=abort`, stripped | Standard Rust release-perf playbook |

## What v2 looks like

Cataloged here so v1 stays focused:

- Plugin sandbox (seccomp filter + restricted FS view per plugin).
- Container-aware metrics (cgroup-per-container drilldown).
- Native GTK/libadwaita frontend (would share the daemon + protocol).
- Web UI on top of the daemon (Prometheus already covers
  structured-data interop).
- Anomaly detection / smart alerting over historical data.
- Mobile companion app over remote socket.
