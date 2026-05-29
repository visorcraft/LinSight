# LinSight v2 — Implementation Plan

> **Audit date:** 2026-05-27 (status updated 2026-05-29)
> **Current version:** v1.4.0 (322 tests passing)
> **Architecture:** daemon (`linsightd`) + Qt6/Kirigami GUI + CLI + mTLS tunnel
> **Plugins (15):** CPU, Memory, Network, NVMe, NVML (NVIDIA), Intel Xe,
>   AMDGPU, Intel i915, block-device I/O (disk), filesystem (fs), hwmon,
>   process table (proc), system, systemd units, ZRAM
> **Extras:** Prometheus exporter, SQLite history, evalexpr alerts, device nicknames
> **Dashboard:** Preset pages (Overview/GPUs/Storage/Network/Hardware) + custom canvas editor

This document inventoried what a highly-customizable system resource monitor
*should* have that LinSight did **not** yet implement, organized by theme.
**Status (2026-05-29): all phases are implemented except where explicitly
noted as deferred/partial below** — see the per-item ✅ marks, the Priority
Matrix, and the Summary. Each entry includes priority, rationale, and a
sketch of how it was built given the existing architecture.

---

## Phase A: Deepen Existing Sensors

### A1. Per-Core CPU Breakdown ✅

**What: Currently `cpu.util` is a single aggregate over all cores. Add per-core utilization sensors (`cpu.core0.util`, `cpu.core1.util`, …) and per-core frequency.

**Why:** Power users tuning processes or diagnosing thermal throttling need per-core visibility.

**How:** The existing `proc_stat.rs` parser already reads individual CPU lines (`cpu0`, `cpu1`, …). Split into one sensor per core + the existing aggregate. The `cpu.core0.device_key` should remain `cpu:0` (no separate hardware device per core). Register with `min=0`, `max=100`.

**Tests:** Extend `proc_stat.rs` test fixtures with multi-core samples.

### A2. System-Level Sensors: Load, Uptime, Procs, Entropy ✅

**What: Advertise sensors from `/proc/loadavg`, `/proc/uptime`, `/proc/stat` (processes, context switches, processes running).

| Sensor | Source | Kind |
|---|---|---|
| `system.load_1m` | `/proc/loadavg` field 1 | Scalar |
| `system.load_5m` | `/proc/loadavg` field 2 | Scalar |
| `system.load_15m` | `/proc/loadavg` field 3 | Scalar |
| `system.procs_running` | `/proc/loadavg` field 4 | Scalar |
| `system.procs_total` | `/proc/loadavg` field 5 | Scalar |
| `system.uptime_secs` | `/proc/uptime` field 1 | Counter |
| `system.ctxt_switches` | `/proc/stat` `ctxt` line | Counter |
| `system.procs_created` | `/proc/stat` `processes` line | Counter |
| `system.entropy_bits` | `/proc/sys/kernel/random/entropy_avail` | Scalar |

**Why:** Every monitoring tool shows these. Missing them makes LinSight feel incomplete.

**How:** Create a new `linsight-sensors-system` crate (or add to the CPU plugin). Read `/proc` files on each sample, cache prev values for rates where needed. Emit one `HardwareDevice` with `key="system:0"`, `category=Other`.

**Tests:** Synthetic `/proc` fixtures per existing sensor plugin pattern.

### A3. Swap & ZRAM Sensors ✅

**What: Currently `mem.used_bytes` and `mem.total_bytes` ignore swap entirely.

**Add:**
- `mem.swap_total_bytes` — `/proc/meminfo SwapTotal`
- `mem.swap_used_bytes` — `SwapTotal - SwapFree`
- `mem.swap_cached_bytes` — `/proc/meminfo SwapCached`
- `mem.zram_*` — compressed in-memory swap from `/sys/class/block/zramN/` (mm_stat: orig_data_size, compr_data_size, mem_used_total)

**Why:** Swap usage is critical for diagnosing OOM pressure. ZRAM is standard on modern Linux.

**How:** Extend `meminfo.rs` with swap fields. Create a lightweight `linsight-sensors-zram` plugin that enumerates `/sys/class/block/zram*`.

**Tests:** Extend `mem/src/meminfo.rs` tests. Synthetic zram sysfs for zram plugin.

### A4. Full Block Device I/O ✅

**What:** Currently only NVMe has block I/O sensors (via `nvmeNn1/stat`). SATA/SAS SSDs, USB drives, and MMC are invisible.

**Add via a new `linsight-sensors-disk` plugin:**
- `disk.<name>.bytes_read` / `disk.<name>.bytes_written` — from `/sys/class/block/<name>/stat`
- `disk.<name>.iops_read` / `disk.<name>.iops_write` — derivative: sectors per sec → 512-byte IOPS
- `disk.<name>.io_util_ms` — `io_ticks` field from stat (field 10, like iostat)
- `disk.<name>.temp_c` — hwmon if attached (many SATA drives expose temp via `smartctl`-accessible attributes; can also read from `nvme` (already covered))

**Device key scheme:** `block:<devname>` (e.g. `block:sda`, `block:nvme0n1`). Skip loop, dm-, md-, zram devices (controlled via prefix filter + a user-visible configuration option later).

**Why:** Disk I/O is the most common bottleneck after CPU/RAM. Having only NVMe means non-NVMe users get no storage I/O data at all.

**How:** New `linsight-sensors-disk` crate. On `init()`, enumerate `/sys/class/block/*` skipping virtual devices. On each sample, read the `stat` file (11-field kernel iostats). Counter-aware sensors for cumulative bytes; time-delta for IOPS in the GUI.

**Tests:** Synthetic `stat` files. Verify loop/snap/zd/etc are filtered.

### A5. General Hwmon Enumeration ✅

**What:** The CPU plugin currently hardcodes `coretemp`/`k10temp` detection. Dozens of other hwmon devices exist: motherboard Super I/O (NCT6795D), GPU fan controllers, PSU voltage rails.

**Create a `linsight-sensors-hwmon` plugin that enumerates all `/sys/class/hwmon/hwmonN/`:**
- Every `temp*_input` with its label → `hwmon.<name>.<label>_temp_c`
- Every `fan*_input` with its label → `hwmon.<name>.<label>_fan_rpm`
- Every `in*_input` with its label → `hwmon.<name>.<label>_volts`
- Every `power*_input` → `hwmon.<name>.power_w`
- Every `curr*_input` → `hwmon.<name>.<label>_amps`

**Device key scheme:** `hwmon:<name>` using the `name` file inside each hwmonN, falling back to the indexed hwmonN path.

**Why:** This single plugin covers motherboard sensors, PSU sensors, chassis fans — the most common "why can't I see my fan speed?" complaint.

**How:** On `init()`, walk `/sys/class/hwmon/*`, read the `name` file, enumerate all `temp*_input` / `fan*_input` / `in*_input` / `power*_input` / `curr*_input` files with their `*_label` companions. Register one device per hwmonN. On sample, read the cached path. Use `tracing::warn` if a cached path vanishes (hot-unloaded module).

**Tests:** Fake sysroot with synthetic hwmon directories containing `name`, `temp1_input`, `temp1_label`, `fan1_input`.

### A6. Thermal Zone Sensors ✅

**What:** `/sys/class/thermal/thermal_zoneN/` exposes kernel thermal zone temps (e.g. `acpitz`, `x86_pkg_temp`, `iwlwifi_1`).

**Add sensors like:**
- `thermal.<type>.temp_c` (e.g. `thermal.x86_pkg_temp.temp_c`)

**Why:** Complements hwmon. Some zones (e.g. ACPI thermal) are only visible here.

**How:** Create within the hwmon plugin or separately. Read `type` + `temp` from each zone. No device key needed — aggregate as `thermal:0` device.

**Tests:** Synthetic `thermal_zoneN/type` and `temp` files.

---

## Phase B: New Sensor Domains

### B1. Process/Top Monitoring (`linsight-sensors-proc`) ✅

**What:** A sensor that emits a `Reading::Table` of processes sorted by resource usage. Columns: pid, name, cpu%, mem%, rss_bytes, threads, state.

**Sensor id:** `proc.list`  
**Kind:** `Table`, rate ~0.5 Hz  
**Category:** `Custom`

**Why:** The single most-requested feature in any system monitor. Without it, users must keep htop open alongside LinSight.

**How:** Read `/proc/<pid>/stat` + `/proc/<pid>/status` per process. For CPU%, diff against a previous snapshot (like `proc_stat.rs` does per-core). Cache across samples so the daemon doesn't re-open every directory on each tick — `/proc` is expensive to enumerating 1000+ entries at 1 Hz. Use `readdir` with `getdents64` and a cached fd approach.

**Performance constraint:** Must be opt-in with a low default rate (0.2 Hz) and a warning when subscribed. Must not block other sensors during enumeration.

**Tests:** Synthetic `/proc` tree with known pid contents — 5 unit tests passing.

### B2. AMD GPU ✅ ✅

**What:** Only NVIDIA (NVML) and Intel Arc (Xe) are supported.

**Add:**
- **AMDGPU** (`linsight-sensors-amdgpu`): Read `/sys/class/drm/cardN/device/` for vendor/device ID, `gpu_busy_percent`, `mem_busy_percent`, `power1_average` (via hwmon child), temperature (via hwmon), VRAM info from `/sys/class/drm/cardN/device/mem_info_vram_total` / `mem_info_vram_used`. Optionally integrate `rocm-smi`-equivalent queries via sysfs.
- **Intel legacy (i915)** (`linsight-sensors-i915`): `/sys/class/drm/cardN/device/gt_cur_freq_mhz`, `gt_act_freq_mhz`, `gt_RP*`, hwmon temp. The Xe plugin only works on DG2+ / Battlemage.

**Device key scheme:** Existing `pci:0000:XX:XX.X` for all GPU types.

**Why:** NVIDIA-only GPU monitoring alienates ~70% of the GPU market.

**How:** Each is a new in-tree plugin. The AMDGPU plugin checks for AMD PCI vendor ID (0x1002) and `/sys/class/drm/cardN/device/vendor`. i915 plugin checks for Intel's vendor ID and `i915` driver string.

**Tests:** Synthetic DRM sysfs trees (same pattern as Xe plugin's existing tests).

### B3. Filesystem/Disk Usage ✅

**What:** No filesystem-level monitoring at all.

**Add `linsight-sensors-fs`:**
- `fs.<mountpoint>.total_bytes`, `fs.<mountpoint>.used_bytes`, `fs.<mountpoint>.avail_bytes`
- `fs.<mountpoint>.inodes_total`, `fs.<mountpoint>.inodes_used`
- For ZFS: `fs.<pool>.arc_size_bytes`, `fs.<pool>.l2arc_size` via `/proc/spl/kstat/zfs/`
- For Btrfs: per-subvol usage from `/sys/fs/btrfs/`

**Device key scheme:** `fs:<mountpoint>` sanitized (no leading slash — use `fs:home` for `/home`). Or `fs:pool:<poolname>` for ZFS.

**Why:** "My disk is full" is the #1 sysadmin question. A monitoring app must answer it.

**How:** Read `statvfs` (or `/proc/mounts` + `statfs`) to enumerate mounted filesystems at init. Skip pseudo-fs (proc, sysfs, tmpfs, devtmpfs, cgroup, etc.) — user-configurable filter list. On each sample, call `statvfs` on each tracked path.

**Tests:** Synthetic tmpfs mounts. Verify pseudo-fs filtering.

### B4. Container/Systemd Unit Monitoring ✅ systemd · ⬜ containers (deferred)

**What:** No awareness of containers or services.

**Near-term (systemd) ✅:** `linsight-sensors-systemd` — reads cgroup v2 filesystem at `/sys/fs/cgroup/system.slice/` for per-service CPU usage, memory, and PID counts. Emits a `Reading::Table` (`systemd.units`) with columns: unit name, state (running/inactive), CPU delta (usec), memory (bytes), PIDs. No D-Bus dependency — pure cgroup v2 filesystem reads. Device key: `system:systemd`. Gracefully returns empty manifest when cgroup v2 or systemd is absent.

**Long-term (containers) ⬜ not implemented:** `linsight-sensors-docker` or `linsight-sensors-podman` — list containers via socket, emit per-container CPU/mem/net as Table sensors. Deferred; no such crate exists yet.

**Why:** Most production Linux systems run containers or systemd services. A "top for containers" view is table-stakes.

**How:** The systemd plugin uses the `zbus` crate (already a transitive dep via D-Bus notifications in the daemon) to query `org.freedesktop.systemd1.Manager`. Falls back gracefully when D-Bus is unavailable.

**Tests:** Mock D-Bus with synthetic responses (hard; start with integration-level tests).

### B5. Pressure Stall Information (PSI) ✅

**What:** `/proc/pressure/cpu`, `/proc/pressure/memory`, `/proc/pressure/io` expose `some avg10 avg60 avg300 total` lines — the gold standard for resource contention measurement.

**Add sensors:**
- `psi.cpu_some_10` / `psi.cpu_some_60` / `psi.cpu_some_300`
- `psi.mem_some_10` / `psi.io_some_10` (and `full` variants)
- `psi.mem_full_total` / `psi.io_full_total` — cumulative stalled time (Counter)

**Why:** PSI is strictly better than raw CPU% for detecting contention — it measures how long tasks are *waiting* rather than how busy the resource is.

**How:** Read `/proc/pressure/*` files, parse the standard format. One sensor per line + window. Device key: `psi:0` (system-level).

**Tests:** Synthetic `/proc/pressure/*` files.

### B6. Network Detail: Errors, Drops, Packets ✅ net stats · ⬜ socket stats (deferred)

**What:** Current network sensors only cover aggregate bytes + link state/speed.

**Add to the net plugin:**
- `net.<iface>.rx_packets` / `tx_packets` — from `statistics/rx_packets`
- `net.<iface>.rx_errors` / `tx_errors`
- `net.<iface>.rx_dropped` / `tx_dropped`
- `net.<iface>.rx_multicast`
- `net.<iface>.tx_carrier` / `tx_colls` (if available)

**Socket-level (`linsight-sensors-sock`) ⬜ not implemented:** (deferred — no `linsight-sensors-sock` crate exists yet)
- `sock.tcp_established` / `tcp_time_wait` / `tcp_listen` — from `/proc/net/sockstat` or `ss`-like parsing
- `sock.udp_inuse`
- `sock.mem_bytes` — TCP memory pressure

**Why:** Packet errors/drops are the first sign of a bad cable or driver bug. Socket stats indicate service health.

**How:** Extend the existing net plugin's sensors list. New fields are cheap read of one additional `statistics/*` counter file. Socket sensors use a new plugin parsing `/proc/net/sockstat`.

**Tests:** Extend net plugin test fixtures with additional stat files.

---

## Phase C: Custom Dashboard & Visualization Improvements

### C1. History-Backed Charts (Sparklines over Time) ✅

**What:** Current dashboard tiles show only the live scalar value. No sparkline, no history.

**Done:**
- `RequestOp::GetHistory` / `ResponsePayload::History` added to the protocol (v2 append-only extension)
- Daemon history module (`apps/linsightd/src/history.rs`) has a `pub fn query()` that reads the SQLite DB: accepts sensor ID, time window (`since_micros`/`until_micros`), and `max_points` for server-side downsampling
- Transport dispatch in `unix.rs` wired: `GetHistory` queries the scheduler's `history_db_path` and returns `ResponsePayload::History`
- GUI client (`apps/linsight-gui/src/client.rs`) exposes `get_history()` RPC method
- Scheduler stores `history_db_path: Option<PathBuf>` and exposes it via `history_db_path()`; runtime sets it via `set_history_db_path()` when LINSIGHT_HISTORY is enabled
- Rolling sparkline buffer (30 points per sensor) maintained in `SampleState` inside the sample-pump thread; embedded in `TileJson.sparkline` on every tick
- Sparkline mini-chart rendered via `Canvas2D` on each `SensorTile` (overview page) and each `DashboardViewPage` tile delegate — normalized to min/max with accent color stroke
- QML plumbing: `tileSparkline` property on `SensorTile`, `_sparklines` map parsed from `tilesJson` in both OverviewPage and DashboardViewPage

### C2. Multi-Page Custom Dashboards (Beyond One Layout) ✅

**What:** Currently the editor saves one flat layout. Users should have multiple named dashboards, each with its own canvas.

**Why already works?** The `DashboardsModel` CRUD exists. The `DashboardViewPage` and `CanvasEditorPage` are wired for per-slug routing.

**Added:**
- **Dashboard gallery / overview**: `DashboardViewPage.qml` shows a card grid when `viewingSlug` is empty, using `app.dashboards.summaryJson` to list all saved dashboards as clickable cards with name and "Open"/"Edit" buttons
- **Export**: "Export" button in the dashboard view header copies the layout JSON to clipboard as a shareable `.dashboard.json` blob
- **Import**: `FileDialog` import button in the gallery toolbar reads a previously-exported JSON file, validates it, and creates a new dashboard with a unique slug
- **Works without protocol extensions**: export/import handled entirely client-side using existing `DashboardsModel` CRUD

### C3. Widget Type: Table Renderer ✅

**What:** The `Reading::Table` type exists (NVML processes), but the canvas editor only shows a big text value. Table-type tiles should render as a multi-column sortable table.

**Done:**
- **Rust side**: `TileJson` extended with `rows: Vec<Vec<serde_json::Value>>` field (skip-serialized when empty). `format_reading_with_rows()` replaces `format_reading()` — serializes `Cell` enums to JSON alongside the `"<N rows>"` placeholder string. The `rows` field flows through `tilesJson` to all QML consumers.
- **SensorTile.qml**: Already had `tileRows` property and `tableRenderer` component with `ListView` + per-cell formatting (text, number, bytes). Now wired through `CategoryPage.qml` which passes `tileKind` and `tileRows` from parsed tilesJson.
- **DashboardViewPage.qml**: `refreshSensors()` now extracts `rows` and `kind` per tile into `rowsById`/`kindById` maps. The tile delegate uses a `Loader` that switches between `dashScalarComp` (plain value label) and `dashTableComp` (scrollable `ListView` with per-cell rendering) based on `kindById`.
- **CanvasEditorPage.qml**: Same pattern — `rowsById`/`kindById` maps extracted in `refreshSensors()`. The `CanvasTile` body replaces its static `Controls.Label` with a `Loader` switching between `canvasScalarComp` and `canvasTableComp`.

### C4. Widget Options UI ✅

**What:** `WidgetPlacement.options` is a free-form JSON field in the data model. No GUI to set it.

**Minimum:** 
- Color picker for tile background / text accent
- Threshold: configurable `(ok, warn, crit)` values → tile border color changes
- Label override: user sets a custom display name for the tile
- Min/max range override for gauge widgets

**Done:**
- **Options round-trip**: `canvasModel` ListModel now has an `options` role. `serialize()` emits `{id, x, y, w, h, options}`, `loadFromJson()` reads `e.options`. `addTile()` initializes `options: {}`. The Rust `DashboardTile` already had `#[serde(default)] pub options: serde_json::Value` — so options persist to disk and reload correctly.
- **Options editor**: `Kirigami.OverlayDrawer` (right-edge, modal) opened via a gear icon (`configure-symbolic`) in each `CanvasTile` header. Fields: Label Override, Text Accent Color, Visibility Condition, Threshold Enable checkbox with OK/Warning threshold inputs. Apply/Cancel buttons write back to `canvasModel`.
- **Dashboard viewer rendering**: `DashboardViewPage.qml` tile delegate now reads `modelData.options` — threshold colors change border width/color based on live sensor value, label override replaces the tile name, text accent color applies to the header label.
- **SensorTile**: Already had `tileOptions` property with threshold color logic and label override support — wired and ready for callers that pass options.

### C5. Conditional Tile Visibility ✅

**What:** Show/hide a tile based on a sensor value expression (e.g., "only show GPU process table when nvml.gpu0.util > 0").

**Done:**
- `options: serde_json::Value` field added to `DashboardTile` struct (serde(default) for backward compatibility with existing dashboard files)
- `DashboardViewPage.qml` tile delegates evaluate `modelData.options.condition` against the live `valueById` map via `evalCondition()` — supports `>`, `<`, `>=`, `<=`, `==`, `!=` comparisons on numeric sensor values
- Unparseable conditions default to visible (fail-open). No condition = always visible

---

## Phase D: Alerting & Notification Overhaul

### D1. Alert Rule UI ✅

**What:** Alerts are configured via hand-editing `$XDG_CONFIG_HOME/linsight/alerts.toml`. No UI.

**Done:**
- **Daemon**: `AlertEngineHandle` now has `upsert_rule()`, `delete_rule()`, and `save_config()` methods. `AlertsConfig` and `RuleConfig` structs now derive `Serialize` so they can be re-serialized to TOML. Transport dispatch for `UpsertAlert` and `DeleteAlert` is fully wired — mutates the in-memory engine and persists to `alerts.toml`.
- **Protocol**: `ListAlerts`, `UpsertAlert`, `DeleteAlert`, `TestAlertExpr` already existed as protocol ops. `AlertRuleJson` struct already existed.
- **GUI client**: `list_alerts()`, `upsert_alert()`, `delete_alert()`, `test_alert_expr()` RPC methods added to `Client`.
- **GUI model**: New `AlertModel` Rust QObject (`apps/linsight-gui/src/qobjects/alert_model.rs`) with `reload()`, `upsert()`, `delete()`, `test_expr()` invokables. Exposes `rulesJson`, `isLoading`, `lastError`, `testResult` properties.
- **QML page**: `AlertsPage.qml` with rule list (name, expression, notify targets), Add/Edit dialog (name, expression, comma-separated notify targets), inline "Test" button per rule, "Delete" button with confirmation, and a separate test-result dialog. Parses `rulesJson` from the model.
- **Navigation**: "Alerts" nav item added to the sidebar (System section), `app.goTo("alerts")` wired, `Component` registered in Main.qml, `AlertModel` instantiated as `app.alerts`.
- **Scheduler**: `alerts_config_path: Option<PathBuf>` added to `Scheduler` struct with getter/setter. Runtime sets it when `LINSIGHT_ALERTS` is enabled.

- **Alert UI polish**: Sensor picker dropdown (populated from live sensor catalogue), per-rule enable/disable toggle (Switch in delegate, `enabled` field wired through protocol → daemon engine → GUI), expression builder with operator hints and sensor-id insertion, `enabled: bool` on `AlertRuleJson` / `CompiledRule` / `RuleConfig`, disabled rules skipped in engine evaluation.

### D2. Webhook Notification Target ✅

**What:** Only `desktop` (libnotify) and `exec:<argv>` were supported.

**Added a `webhook:<url>` target that POSTs the alert payload as JSON. Enables Slack, Discord, PagerDuty, etc.**

**How:** Uses `ureq` v3 (lightweight sync HTTP client with rustls TLS backend) to POST JSON with `{name, expr, source: "linsight"}` to the configured URL. Supports both `http://` and `https://` targets. The `fire_webhook` function already existed with a raw TCP implementation that only supported HTTP; replaced `do_webhook_post` with ureq-based version that handles HTTPS and reads response status codes. Error handling maps `ureq::Error::StatusCode(code)` to a descriptive `io::Error`. Tests cover payload format validation and fire-and-forget thread spawn behavior.

**Done:**
- Added `ureq = { version = "3", features = ["rustls"] }` to workspace deps
- Replaced raw TCP `do_webhook_post` with ureq-based implementation supporting HTTPS
- Added `webhook_payload_format` test verifying JSON payload structure
- Added `fire_webhook_returns_ok_for_any_url` test verifying thread spawn

### D3. Multi-Condition Rules (AND/OR/NOT) ✅

**What:** Currently rules are a single `evalexpr` boolean expression. Users can write compound expressions in the expr itself, but there's no structured AND/OR grouping with multiple conditions firing independently.

**How:** This is mostly already solved — `evalexpr` supports `&&`, `||`, `!` natively. Document the expression language in the UI and provide an expression-builder widget.

**Done:**
- **Expression-builder widget**: Added operator buttons (`&&`, `||`, `!`, `>`, `<`, `>=`, `<=`, `(`, `)`) to the alert edit dialog. Each button inserts the operator at the cursor position in the expression field.
- **Sensor picker**: Retained and improved — now inserts sensor ID at cursor position using `exprField.insert()` instead of manual string slicing.
- **Syntax help panel**: Collapsible `Kirigami.InlineMessage` toggled by a "Show Syntax Help" button. Documents sensor IDs as variables, comparison operators, logical operators, parentheses for grouping, and provides multi-condition examples (e.g., `cpu.util > 80 && mem.used_bytes > 8e9`, `(xe.gpu0.temp_c > 85 || xe.gpu1.temp_c > 85)`, `!(cpu.util > 10) && system.load_1m > 4`).
- **Placeholder text**: Updated to show a compound example (`e.g. cpu.util > 90 && mem.used_bytes > 8e9`).

---

## Phase E: CLI & Developer Experience

### E1. `linsight-cli watch` ✅

**What:** `linsight-cli read` fetches once. Add `linsight-cli watch cpu.util --rate 1` that subscribes and prints live updates.

**How:** Connect to the daemon, subscribe, enter the pump loop printing formatted samples to stdout. Support `--format json` for machine consumption.

### E2. `linsight-cli alert` Subcommands ✅

**What:** `linsight-cli alert list`, `linsight-cli alert add`, `linsight-cli alert rm`. Manage alert rules from the terminal.

**How:** `RequestOp::ListAlerts` etc. implemented in the protocol, CLI subcommands wired against them.

### E3. `linsight-cli history` ✅

**What:** `linsight-cli history cpu.util --last 5m --format csv` — query the SQLite history database through the daemon's protocol.

**How:** `RequestOp::GetHistory` implemented in protocol. CLI renders as table or CSV.

---

## Phase G: Configuration & Extensibility

### G1. Plugin Configuration (Per-Plugin Settings) ✅

**What:** Plugins have no user-facing configuration. You can't say "monitor only these network interfaces" or "exclude this disk."

**Add an optional `config` field to `PluginCtx`** that the daemon loads from a plugin-name-keyed file (`plugins.toml`):

```toml
[linsight-sensors-net]
exclude_interfaces = ["docker*", "veth*"]

[linsight-sensors-disk]
exclude_devices = ["loop*", "dm-*", "md*"]
```

**How:** The daemon reads `plugins.toml` at startup, passes a `serde_json::Value` into `PluginCtx` via the new `with_config()` builder. `RPluginCtx` carries it as `config_json: SString` across the FFI boundary. ABI bumped to v5 (factory symbol renamed `linsight_plugin_v4` → `linsight_plugin_v5` so mismatched plugins fail at symbol lookup). Net and disk plugins read `exclude_interfaces` / `exclude_devices` arrays from their config and skip matching devices during enumeration. Glob patterns use `prefix*` matching (trailing `*` = prefix match).

**Done:**
- **ABI v5**: `LINSIGHT_PLUGIN_ABI_VERSION` bumped to 5. `export_plugin!` macro emits `linsight_plugin_v5` factory. Dynamic load test updated.
- **PluginCtx**: Added `config: serde_json::Value` field, `config()` accessor, `with_config()` builder.
- **RPluginCtx**: Added `config_json: SString` field — JSON-serialized config passed across FFI. `From` conversions updated in both directions.
- **Daemon config**: `plugins.toml` loaded from `$XDG_CONFIG_HOME/linsight/plugins.toml` at startup. Top-level keys are plugin IDs (e.g. `linsight-sensors-net`), values are arbitrary TOML tables converted to `serde_json::Value`.
- **PluginHost**: `with_builtins_and_config(&HashMap)` passes per-plugin config. `register_with_config()` builds `PluginCtx` with config before calling `host_init`.
- **Net plugin**: Reads `exclude_interfaces` from config. Glob pattern matching (`docker*` matches `docker0`).
- **Disk plugin**: Reads `exclude_devices` from config, applied in addition to existing hardcoded `VIRTUAL_PREFIXES`.
- **Tests**: new tests added — `enumerate_respects_exclude_patterns`, `matches_exclude_handles_star_suffix`, `enumerate_respects_exclude_devices`, `with_builtins_and_config_passes_config_to_plugins`.

**Known limitation:** per-plugin config is wired through for in-tree plugins only. Dynamic `.so` plugins loaded from `plugin_dirs()` currently always receive an empty config — `PluginHost::load_from_dir` does not look up the plugin id in `plugin_configs` before calling `host_init`, because the plugin id is only known after init returns the manifest. A follow-up needs either an SDK-level `plugin_id()` accessor separate from `init`, or a documented "init is idempotent" contract so init can be re-run with the looked-up config. Until then, dynamic plugins must work with empty configuration. Tracked in `apps/linsightd/src/plugin_host.rs::load_from_dir`.

### G2. Sensor Tagging / Groups ✅

**What:** Users should be able to tag sensors (e.g. `tag:production`, `tag:gaming`) and filter the dashboard catalogue by tag.

**How:** `tags: Vec<String>` added to `SensorDescriptor` (plugin-sdk) + `SensorInfo` (protocol). Plugin-sdk manifest, FFI R-struct, From conversions all updated. Daemon stubs assign default category-based tags.

---

## Phase H: Security & Production Readiness

### H1. mTLS CN/SAN Filtering ✅

**What:** The tunnel's CA is a full-access trust boundary — any cert signed by the configured CA controls the daemon.

**Add optional CN/SAN allowlist:** `--allow-cn 'myserver' --allow-san '*.example.com'` so a compromised client cert doesn't have full daemon access.

### H2. Socket-Auth Handshake ✅

**What:** Any process that can reach the Unix socket can connect to `linsightd`. On a multi-user system, that's all users with access to `$XDG_RUNTIME_DIR`.

**Add optional `LINSIGHT_AUTH_TOKEN` — the daemon checks a shared token after the Hello handshake. The GUI reads the token from env (same variable populated by systemd user unit).

**Implementation:** `auth_token: Option<String>` added to `ClientMsg::Hello`. Daemon reads `LINSIGHT_AUTH_TOKEN` env var at serve time and validates the token before allowing the connection.

### H3. Rate Limits on the Accept Loop ✅

**What:** `MAX_CLIENT_SESSIONS=64` exists, but there's no per-IP/per-second rate cap for TCP server paths (Prometheus exporter, tunnel). Add connection rate limiting via a token bucket.

**Implementation:** `AcceptRateLimiter` token bucket in `unix.rs` (20/s default, configurable via `LINSIGHT_ACCEPT_RATE` env var). Applied in the accept loop at connection granularity.

---

## Priority Matrix

| Item | Priority | Effort | Impact |
|---|---|---|---|
| A2 — System load/uptime/entropy ✅ | P0 | Small | Fills basic gaps |
| A4 — Full block device I/O ✅ | P0 | Medium | Covers non-NVMe users |
| A1 — Per-core CPU ✅ | P1 | Small | Power users |
| A3 — Swap ✅ | P1 | Small | OOM debugging |
| B5 — PSI sensors ✅ | P1 | Small | Gold-standard contention metric |
| B1 — Process list table ✅ | P1 | Large | Most requested feature |
| C1 — History-backed sparklines | P1 | Large | Transforms dashboards from static to live |
| A5 — Generic hwmon ✅ | P1 | Medium | Fans/voltage/motherboard |
| B4 — Container monitoring ✅ | P2 | Large | Production essential |
| B2 — AMDGPU ✅ | P2 | Medium | GPU coverage parity |
| C4 — Widget options UI ✅ | P2 | Medium | User customization |
| C2 — Multi-page dashboards | P2 | Medium | Already partly built |
| C3 — Table widget rendering ✅ | P2 | Small | Table sensors already exist |
| B6 — Network errors/drops ✅ | P2 | Small | Quick win |
| D1 — Alert rule UI | P2 | Large | Usability |
| B3 — Filesystem usage ✅ | P2 | Medium | Essential metric |
| H1 — mTLS CN/SAN filtering ✅ | P2 | Small | Production security |
| E1 — CLI watch ✅ | P2 | Small | Developer UX |
| D2 — Webhook alerts ✅ | P2 | Small | Integration |
| G1 — Plugin configuration | P2 | Medium | Extensibility |
| A6 — Thermal zones ✅ | P3 | Small | Completeness |
| C5 — Conditional tiles ✅ | P3 | Small | Dashboard polish |
| Multi-host GUI ⬜ | P3 | Huge | Architecture change — deferred, not implemented |
| E2 — CLI alert mgmt ✅ | P3 | Medium | Parity |
| E3 — CLI history ✅ | P3 | Medium | Parity |
| H2 — Socket auth ✅ | P3 | Small | Security |
| G2 — Sensor tagging ✅ | P3 | Small | Organization |
| H3 — Rate limits ✅ | P3 | Small | Security |

## Summary of Changes

All Phase A through H items are implemented except where marked deferred:
the container sub-item of B4 (systemd shipped; Docker/Podman not), the
socket-stats sub-item of B6 (`linsight-sensors-sock` not built), and the
Multi-host GUI (architecture change, never started). Full workspace compiles
and `cargo test --workspace` passes **322** at the time of writing — run it
for the current number.

**The plugin ABI is now v5** (`LINSIGHT_PLUGIN_ABI_VERSION = 5`). Most of the
work above was additive and ABI-neutral; the one exception was G1 (per-plugin
config), which added `config_json` to `RPluginCtx` and bumped the factory
symbol `linsight_plugin_v4` → `_v5`. The additive patterns were:

1. **New sensors** are new `linsight-sensors-*` crates implementing the existing `LinsightPlugin` trait.
2. **New protocol messages** extend the `ClientMsg`/`ServerMsg` enums with new `RequestOp` / `ResponsePayload` variants — append-only to maintain wire stability.
3. **Dashboard/graph features** are QML + `OverviewModel` changes only.
4. **Alert UI** requires new request types but no daemon-side engine changes.
5. **Multi-host** and **Prometheus data source** are the only items that touch the client-architecture.

Each item added its own unit tests; see the live count via `cargo test --workspace`.