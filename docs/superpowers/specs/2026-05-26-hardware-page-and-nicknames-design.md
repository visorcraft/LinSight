<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Hardware Page + Per-Device Nicknames — Design Spec

**Status:** approved 2026-05-26 (revised post-Codex-review: SDK ABI
bump v3 → v4 explicitly chosen over daemon-side hardware
re-derivation, since no third-party `.so` plugins exist yet).
**Author:** Claude (paired with VisorCraft); independently reviewed by Codex (gpt-5.5).
**Implementation plan:** `docs/superpowers/plans/2026-05-26-hardware-page-and-nicknames-roadmap.md` (forthcoming).

## Problem

Sensor tiles currently display labels like
`INTEL GPU UTILIZATION (GPU1 0000:06:00.0)` — truncated on narrow
tiles, generic across vendors, and pdev-cryptic. A user with three
GPUs (Intel iGPU + Intel dGPU + NVIDIA dGPU) cannot tell them apart
at a glance, and there is no way to assign meaningful names like
"Battlemage" or "RTX 5080" or "OS drive."

Three concrete gaps:

1. **No real model strings for non-NVIDIA hardware.** NVML already
   returns `"NVIDIA GeForce RTX 5080 Laptop GPU"` via `dev.name()`,
   but the Intel xe plugin emits `"gpu1 0000:06:00.0"` and NVMe /
   net only label by raw device name.
2. **No central hardware view.** Sensors are grouped by category
   ("GPUs", "Storage", "Network") but there is no list of *the
   underlying devices* with their model, vendor, location, and the
   set of sensors each one produces.
3. **No user-editable nicknames.** Even with proper model strings,
   two identical 5080s or two Samsung drives are indistinguishable
   without user-assigned labels — and those labels need to propagate
   to CLI / Prometheus / alerts, not just the GUI.

## Non-goals

- Hot-plug. Hardware enumeration runs once at daemon startup; adding
  or removing a GPU / NVMe / NIC requires a daemon restart. Matches
  the current sensor catalogue lifecycle.
- DMI / `dmidecode`-style memory probing. RAM has no per-DIMM
  identity in LinSight at this depth; the CPU row is the only
  motherboard-level entry.
- Per-sensor nicknames. Nicknames attach to *devices*, not individual
  sensors. "GPU temperature · Battlemage" is fine; renaming
  `xe.gpu1.temp_c` itself is not in scope.
- Custom nickname schemes (emoji-only, internationalized scripts).
  Nicknames are UTF-8 with control-char rejection and a 64-char cap;
  beyond that the user is on their own.
- A separate "rename device" wizard. Editing happens inline on the
  Hardware page row.
- Sharing nicknames across machines. `~/.config/linsight/hardware.json`
  is per-machine; users can copy it manually if they want.
- Backward compatibility with v3 plugin `.so` binaries. The SDK
  bumps to v4 (see "Plugin SDK changes" below); we have no
  third-party plugins in the wild, so a hard break is cheaper than
  a parallel-load story.

## Architecture overview

Plugins already enumerate their hardware to produce sensors — the
xe plugin walks `/sys/class/drm/card*`, NVML lists `device_by_index`,
NVMe walks `/sys/class/nvme/*`, etc. Rather than duplicate that
probing in a daemon-side resolver, we **extend the plugin SDK
manifest** so each plugin reports its hardware identities alongside
its sensors. The daemon collects, validates, applies nicknames, and
decorates outgoing `SensorInfo`.

Codex's review preferred the daemon-resolver path to keep the v3
ABI stable. We override that recommendation: no third-party `.so`
plugins exist yet, so a one-time ABI bump v3→v4 is cheaper than
permanent kernel-interface duplication. ADR-0002 will document the
v4 bump alongside this work (placeholder slot — written in the
implementation plan).

Four new components:

1. **`crates/linsight-core/src/hardware.rs`** — the
   `HardwareDeviceKey` newtype, `HardwareDevice` struct, and
   `HardwareCategory` enum. Living in core means both the protocol
   (`linsight-protocol`) and the plugin SDK
   (`linsight-plugin-sdk::mirror`'s R-types) reference one
   canonical definition.
2. **`apps/linsightd/src/hardware.rs`** — `HardwareRegistry`.
   Collects `HardwareDevice` entries from every loaded plugin
   manifest, dedupes by key, validates, holds nicknames, exposes
   `device_label_for(key)` to the transport layer + Prometheus
   exporter. Wrapped in `Arc<RwLock<...>>`.
3. **`apps/linsightd/src/nickname_store.rs`** — disk persistence at
   `~/.config/linsight/hardware.json`. Atomic write (tmp + rename),
   schema-versioned, validated. Same pattern as `preferences.json`.
4. **`apps/linsight-gui/qml/HardwarePage.qml`** + a new
   `HardwareModel` qobject — the GUI page that lists devices and
   accepts nickname edits.

The daemon decorates outgoing `SensorInfo` with `device_key` (stable
identifier) and `device_label` (best human label — nickname || model
|| disambiguated fallback). Clients render `<display_name> ·
<device_label>` for tiles; Prometheus uses `device_key` as a stable
label and exposes nickname/model via a separate info metric.

## Data model

### `HardwareDeviceKey` newtype

Lives in `crates/linsight-core/src/hardware.rs`:

```rust
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct HardwareDeviceKey(String);

impl HardwareDeviceKey {
    pub fn try_new(s: impl Into<String>) -> Result<Self, KeyError>;
    pub fn as_str(&self) -> &str;
    pub fn scheme(&self) -> &str;          // "pci", "nvml", "nvme", "net", "cpu", "plugin"
}
```

Validation regex (informal): `^(pci|nvml|nvme|net|cpu|plugin):
[a-z0-9_:.\-]{1,128}$`. Length cap 140 total (scheme prefix +
payload). Lower-case enforced. Empty key rejected.

### `HardwareDevice` (daemon-side)

```rust
pub struct HardwareDevice {
    pub key:       HardwareDeviceKey,
    pub category:  HardwareCategory,    // Gpu / Storage / Network / Cpu / Other
    pub model:     String,              // canonical model string (no nickname)
    pub vendor:    Option<String>,      // "Intel Corporation", "NVIDIA", etc.
    pub location:  Option<String>,      // PCI slot for display, "USB-2-1", etc.
    pub plugin_id: String,              // the plugin that emits sensors for this device
    pub plugin_device_id: String,       // the plugin-local id ("gpu0", "nvme0")
    pub sensor_ids: Vec<SensorId>,      // sensors produced for this device
}
```

This struct is also the wire shape of `HardwareList` (see protocol).

### `hardware.json` schema

```json
{
  "schema_version": 1,
  "nicknames": {
    "pci:0000:06:00.0":        "Battlemage",
    "nvml:uuid:GPU-abc123...":  "Workstation 5080",
    "nvme:eui.001b448b41234567": "OS drive"
  }
}
```

Schema rules:
- `schema_version` is `1` for now. Bump on backward-incompatible
  changes; the loader falls back to defaults + renames the old file
  to `hardware.json.bad` (same pattern as `preferences.json`
  malformed handling).
- Nickname values: trimmed; empty value → entry deleted; max 64
  Unicode scalar values; rejected if `char::is_control()` returns
  true for any character (covers C0 `U+0000..=U+001F`, DEL
  `U+007F`, C1 `U+0080..=U+009F`).
- Unknown keys (devices not present on this boot) are preserved
  verbatim — moving an NVMe drive between machines should not
  forget its nickname.

## Plugin SDK changes (v3 → v4)

The plugin SDK ABI bumps from `LINSIGHT_PLUGIN_ABI_VERSION = 3` to
`4`. The export symbol renames `linsight_plugin_v3 →
linsight_plugin_v4` so a v3 `.so` fails the symbol lookup at load
time (the clean-error pattern established for v2→v3 in ADR-0001).

### New manifest fields

`PluginManifest` gains a `devices` list:

```rust
pub struct PluginManifest {
    pub plugin_id:    String,
    pub display_name: String,
    pub version:      String,
    pub sensors:      Vec<SensorDescriptor>,
    pub devices:      Vec<HardwareDevice>,   // NEW in v4
}
```

`SensorDescriptor` gains a back-reference to its device:

```rust
pub struct SensorDescriptor {
    // existing fields unchanged ...
    pub device_id:  Option<String>,           // plugin-local grouping (still v3 meaning)
    pub device_key: Option<HardwareDeviceKey>, // NEW in v4: which manifest device
}
```

`device_id` keeps its current meaning (plugin-local grouping). The
new `device_key` references one of the entries in
`manifest.devices` — same regex-validated `HardwareDeviceKey`
newtype. Validation: the daemon rejects a manifest where a
`SensorDescriptor.device_key` doesn't appear in `manifest.devices`.

### R-mirror additions

Following ADR-0001 v3's `(kind, payload)` pattern (NOT
stabby-enum-matcher-bug-prone tagged unions):

```rust
// linsight-plugin-sdk::mirror

#[stabby::stabby]
#[repr(C)]
pub struct RHardwareDevice {
    pub key:               SString,    // already validated by sender; host re-validates
    pub category_kind:     RHardwareCategoryKind,
    pub model:             SString,
    pub vendor:            SOption<SString>,
    pub location:          SOption<SString>,
    pub plugin_device_id:  SString,
}

#[stabby::stabby]
#[repr(u8)]
pub enum RHardwareCategoryKind {
    Gpu, Storage, Network, Cpu, Other,
}
```

`RPluginManifest` extended with `SVec<RHardwareDevice> devices`;
`RSensorDescriptor` extended with `SOption<SString> device_key`.
Both fields appended to the struct's field order (stabby's struct
layout is positional like a `#[repr(C)]` struct, so appending is
the only safe additive form).

`host_init` validates the v4 manifest before it reaches the
daemon's registry:
1. Every `RHardwareDevice.key` round-trips through
   `HardwareDeviceKey::try_new` — release-mode plugins can't
   poison the registry with malformed keys (same hardening
   pattern as `SensorId` validation).
2. Every sensor's `device_key`, if `Some`, must match a manifest
   device's key. Mismatch returns `PluginError::Manifest`.
3. Duplicate device keys within a manifest return
   `PluginError::Manifest`.

### Cost summary

Per the v3 lessons:
- Adding new R-types means two new variants in
  `linsight-plugin-sdk::mirror` plus two `From` impls. Documented
  contract.
- Stabby's proc-macro chain re-expands; cold rebuild of the SDK
  crate gains ~2 s. Acceptable.
- All six in-tree sensor crates need a manifest-augmentation diff.
  The xe and nvml plugins already do the sysfs / NVML probing; the
  diff is mostly assembling the existing knowledge into the new
  `devices` field.
- The `linsight-cli plugin new` template gains a one-device
  example so scaffolded plugins start v4-correct.

## Hardware enumeration per plugin

Each in-tree plugin populates `manifest.devices` from its existing
hardware enumeration; the daemon never re-probes. The table below
is **what the plugin must emit**, not what the daemon does.

| plugin                              | key scheme                           | model source                                                            | fallback                            |
|-------------------------------------|--------------------------------------|-------------------------------------------------------------------------|-------------------------------------|
| `io.visorcraft.linsight.cpu`        | `cpu:0`                              | `/proc/cpuinfo` `model name`                                            | `"CPU"`                             |
| `io.visorcraft.linsight.xe`         | `pci:<slot>`                         | `/sys/.../{vendor,device}` → pci.ids                                    | `"Intel GPU (<vendor>:<device>)"`   |
| `io.visorcraft.linsight.nvml`       | `nvml:uuid:<u>`                      | NVML `dev.name()`                                                       | `"NVIDIA GPU (gpu<i>)"`             |
| `io.visorcraft.linsight.nvme`       | `nvme:<wwid>`                        | `/sys/class/nvme/<n>/model`                                             | `"NVMe SSD (<n>)"`                  |
| `io.visorcraft.linsight.net`        | `net:<ifname>`                       | If PCI-backed: `/sys/class/net/<if>/device` → pci.ids; else driver name | `"<ifname>"`                        |
| third-party `.so` (no `devices`)    | daemon synthesizes `plugin:<plugin_id>:<device_id>` from `SensorDescriptor.device_id` | uses `SensorDescriptor.display_name`            | the synthesized key itself          |

### Shared pci.ids helper

To avoid duplicating pci.ids parsing in every plugin, the SDK
gains an optional helper:
`linsight-plugin-sdk::pciids::PciIdDb::load_default()` reads
`/usr/share/hwdata/pci.ids` once per process and caches the table
in a `OnceLock`. Plugins call `db.lookup(vendor, device)` →
`Option<String>`. Memory cost ~1.5 MB strings, paid once. Plugins
that don't need PCI lookup (cpu, nvml) don't load it.

Fallback chain if hwdata is absent:
1. Try `/usr/share/misc/pci.ids` (Debian alternative).
2. Plugin formats `"<scheme> device 0x<vendor>:0x<device>"`.

### NVMe key choice

Prefer in this order:
1. `/sys/class/nvme/<n>/wwid` (kernel-canonical NGUID/EUI string,
   stable across enclosure moves).
2. `/sys/class/nvme/<n>/serial` (vendor-set, stable per drive).
3. `nvme:<n>` (the controller name; unstable on re-enumeration but
   non-empty).

### NIC key choice

Two NIC archetypes exist; LinSight does not try to unify them:
- **Physical PCI NIC** (`enp4s0`, `wlan0` with a real PCI parent):
  key is `net:<ifname>`, location includes PCI slot.
- **Logical interface** (bonds, VLANs, veth, WireGuard, USB
  hot-attached NICs): key is `net:<ifname>`, no location.

`ifname` is the user-visible identity (the kernel renames hot-plugged
NICs to systemd-predictable names anyway), so we accept it as the
key. A renamed interface gets a new entry; the old nickname stays in
`hardware.json` for re-attachment.

### Disambiguation for identical models

When two devices resolve to the same `model` string and neither has
a user nickname set, the daemon appends a short location suffix
derived from the key:

| key                          | suffix              |
|------------------------------|---------------------|
| `pci:0000:06:00.0`           | `(06:00.0)`         |
| `nvml:uuid:GPU-abc...`       | `(NVIDIA 1)`        |
| `nvme:<wwid>`                | `(nvme<n>)`         |
| `net:<ifname>`               | `(<ifname>)`        |

A user setting a nickname wins; suffixing stops once nicknames
disambiguate.

## Protocol changes

`PROTOCOL_VERSION` bumps from `1` to `2`. The wire-stability comment
in `messages.rs` already prescribes this for non-additive changes.
Clients at v1 fail handshake with the existing `VersionMismatch`
error — no graceful coexistence; users update the GUI/CLI alongside
the daemon. (User confirmed no external consumers exist yet.)

### Augmented `SensorInfo`

```rust
pub struct SensorInfo {
    // existing fields unchanged ...
    pub device_id:    Option<String>,
    pub plugin_id:    String,
    // new in v2:
    pub device_key:   Option<String>,    // stable HardwareDeviceKey if resolved
    pub device_label: Option<String>,    // nickname || model || disambiguated
}
```

`device_id` keeps its original meaning (plugin-local grouping —
`SensorInfo::device_id == "gpu0"` says "this sensor belongs to the
device the plugin internally calls gpu0"). `device_key` is the new
globally-stable identity.

### Request / response with correlation IDs

New `ClientMsg` variants (appended to the end, per the postcard
stability rule):

```rust
pub enum ClientMsg {
    // existing variants unchanged ...
    Goodbye,
    Request { req_id: u32, op: RequestOp },
}

pub enum RequestOp {
    GetHardware,
    SetNickname { device_key: String, value: Option<String> },
}
```

New `ServerMsg` variants:

```rust
pub enum ServerMsg {
    // existing variants unchanged ...
    Bye { reason: String },
    Response { req_id: u32, result: Result<ResponsePayload, ProtoError> },
    SensorListBroadcast(Vec<SensorInfo>),
}

pub enum ResponsePayload {
    Hardware(Vec<HardwareDevice>),
    NicknameSet { device_key: String, value: Option<String> },
}

pub struct ProtoError {
    pub code: ProtoErrorCode,
    pub message: String,
}

pub enum ProtoErrorCode {
    UnknownDevice,
    InvalidNickname,
    Io,
    Internal,
}
```

`req_id` is a client-chosen `u32`. Each new request increments
client-side; the GUI client maintains a `HashMap<u32, oneshot::Sender>`
keyed by `req_id`. `SensorListBroadcast` carries no `req_id` — it's
unsolicited, sent to every connected client after a successful
nickname change.

Single-flight is **not** the chosen contract. We pay the small cost
of `req_id` correlation upfront because the GUI may legitimately
issue a `GetHardware` while a `SetNickname` is in flight.

### Flow for a nickname edit

1. GUI sends `Request { req_id: N, op: SetNickname { device_key, value } }`.
2. Daemon validates the key (must be in registry) and the value
   (length, control chars). On invalid, replies
   `Response { req_id: N, result: Err(ProtoError) }`.
3. Daemon writes `hardware.json` (atomic).
4. Daemon updates its in-memory registry and re-renders every
   `device_label`.
5. Daemon replies `Response { req_id: N, result: Ok(NicknameSet { … }) }`.
6. Daemon broadcasts `SensorListBroadcast(updated_sensor_infos)` to
   every connected client (including the originator). All clients
   replace their cached catalogue and re-emit changed tile labels.

Persist-before-broadcast ensures a crash between (4) and (6) doesn't
lose the user's edit; the next daemon start re-reads the file.

## Daemon-side changes

### `linsightd/src/hardware.rs`

Owns the `HardwareRegistry`. **Does not probe hardware** — that's
the plugin's job. The registry's role is collection, deduplication,
validation, and nickname application.

```rust
pub struct HardwareRegistry {
    /// device_key → canonical record (model, vendor, location, ...)
    devices: HashMap<HardwareDeviceKey, HardwareDevice>,
    /// (plugin_id, plugin_device_id) → device_key, for SensorInfo decoration.
    by_plugin: HashMap<(String, String), HardwareDeviceKey>,
    /// User-assigned nicknames, persisted to hardware.json.
    nicknames: HashMap<HardwareDeviceKey, String>,
}

impl HardwareRegistry {
    /// Build from every loaded plugin's manifest.devices, plus the
    /// persisted nickname map. Logs and rejects conflicting keys
    /// (same key emitted by two plugins).
    pub fn build(
        plugins: &[(PluginId, &PluginManifest)],
        nicknames: HashMap<HardwareDeviceKey, String>,
    ) -> Self;

    /// "nickname" (if set) else "model" (possibly disambiguated).
    pub fn device_label_for(&self, key: &HardwareDeviceKey) -> String;

    /// For each SensorInfo on the wire, given its (plugin_id, device_id).
    pub fn key_for(&self, plugin_id: &str, device_id: &str)
        -> Option<&HardwareDeviceKey>;

    /// Validate + persist + apply. Returns the updated devices vec
    /// for the SensorListBroadcast that follows.
    pub fn set_nickname(&mut self, key: &HardwareDeviceKey, value: Option<String>)
        -> Result<(), NicknameError>;

    pub fn snapshot(&self) -> Vec<HardwareDevice>;
}
```

Wrapped in `Arc<RwLock<...>>` shared between the transport layer,
Prometheus exporter, and alerts. Writes (nickname changes) take the
write lock briefly; reads (sample broadcasts, GetHardware) take the
read lock.

### Conflict handling

A plugin returning a `device_key` that another plugin also claims
is a manifest bug. The daemon logs at WARN and keeps the first
plugin's record (the registry's `HashMap::entry().or_insert`
semantics). The conflict surfaces in `linsight-cli plugin ls`
output and the daemon's structured log; it does not abort startup.

A plugin's `sensors` referencing a `device_key` not in the same
plugin's `manifest.devices` is a HARDER bug — that's a per-manifest
contract violation caught in `host_init`. Such a plugin is
rejected at load.

### `linsightd/src/transport/unix.rs`

Where `SensorInfo` is currently constructed for `SensorList`
responses (around line 208 today), the daemon now consults the
registry and fills in `device_key` + `device_label`. Same logic runs
for the new `SensorListBroadcast`.

A new dispatcher branch handles `ClientMsg::Request { req_id, op }`,
routes to the registry, and writes a `Response`.

### `linsightd/src/prom.rs`

The Prometheus exporter changes to:

```
# HELP linsight_xe_gpu_util Intel xe GPU utilization
# TYPE linsight_xe_gpu_util gauge
linsight_xe_gpu_util{device_key="pci:0000:06:00.0"} 27.6

# HELP linsight_hardware_info Static hardware metadata
# TYPE linsight_hardware_info gauge
linsight_hardware_info{device_key="pci:0000:06:00.0",category="gpu",model="Intel Arc B-series",nickname="Battlemage",plugin_id="io.visorcraft.linsight.xe"} 1
```

Two changes from today:
1. Per-sample metrics gain a stable `device_key` label
   (model/nickname stays *out* of these to keep time-series stable
   across renames).
2. New `linsight_hardware_info` static metric with the
   join-friendly metadata.

Label value escaping is mandatory: `\\`, `\"`, `\n` per the
Prometheus exposition spec.

## GUI client refactor

### Current shape (the blocker)

`apps/linsight-gui/src/client.rs::run_reader_thread` reads
`ServerMsg` in a loop and:
- forwards `Sample` to a `crossbeam::channel::Sender<Sample>`,
- caches `SensorList` once during handshake,
- discards everything else.

This means `GetHardware` replies, `SetNickname` acks, and
`SensorListBroadcast` would all be silently dropped. Refactor first,
protocol second.

### New shape

```rust
struct Client {
    sample_tx:    crossbeam::Sender<Sample>,
    inflight:     Arc<Mutex<HashMap<u32, oneshot::Sender<RpcResult>>>>,
    catalogue_tx: tokio::sync::watch::Sender<Vec<SensorInfo>>,  // or sync equivalent
    next_req_id:  AtomicU32,
}
```

Reader thread:
- `ServerMsg::Sample(s)` → `sample_tx.send(s)`.
- `ServerMsg::Response { req_id, result }` → drain `inflight[req_id]`.
- `ServerMsg::SensorListBroadcast(infos)` → update `catalogue_tx`
  watch; the `OverviewModel` listener wakes up and re-emits
  `tilesJsonChanged`.
- `ServerMsg::SensorDegraded { ... }` → keep existing behavior
  (logged warning).

API exposed to the GUI:

```rust
impl Client {
    pub fn get_hardware(&self) -> Result<Vec<HardwareDevice>, RpcError>;
    pub fn set_nickname(&self, key: &str, value: Option<String>) -> Result<(), RpcError>;
    pub fn subscribe_catalogue(&self) -> watch::Receiver<Vec<SensorInfo>>;
}
```

These are sync wrappers — they send the `Request` and block on
`oneshot::Receiver::recv()`. The reader thread does the dispatching.

## GUI Hardware page

New page under Workspace, after "Network", before "Editor":

```
Workspace
  Overview
  GPUs
  Storage
  Network
  Hardware       ← new
  Editor
```

Keyboard shortcut: `Ctrl+5` (existing pages shift, Editor becomes
`Ctrl+6` etc.).

### Layout

Card per device, grouped by category:

```
┌─ GPUs ─────────────────────────────────────────────────┐
│ ┌─ Intel Arc B-series ─────────────────────┬──────┐   │
│ │ pci:0000:06:00.0 · 4 sensors             │ Edit │   │
│ │ ─────────────────────────────────────────┴──────┘   │
│ │ Nickname: [Battlemage              ]                │
│ └──────────────────────────────────────────────────────┘
│
│ ┌─ NVIDIA GeForce RTX 5080 Laptop GPU ─────┬──────┐   │
│ │ nvml:uuid:GPU-abc... · 6 sensors         │ Edit │   │
│ │                                                      │
│ │ Nickname: [                        ]                │
│ └──────────────────────────────────────────────────────┘
└──────────────────────────────────────────────────────────┘

┌─ Storage ───────────────────────────────────────────────┐
│ ...                                                     │
```

Each card:
- Title row: model string (full, not truncated) + sensor count.
- Subtitle row: `device_key` (small, dim).
- Nickname row: text field, max 64 chars, "Save" enabled only when
  dirty. Save → `set_nickname` via RPC → field shows pending state
  until the broadcast confirms.
- Clearing the field + Save → removes the nickname (sends
  `value: None`).

Validation feedback lives on the same row using the existing
`showSuccess` / `showError` banner pattern from `CanvasEditorPage`
(per CLAUDE.md guidance — don't sniff message content for state).

### `HardwareModel` qobject

```rust
#[cxx_qt::qobject]
pub struct HardwareModel {
    devices_json:  QString,    // serialized snapshot for QML
    is_loading:    bool,
    last_error:    QString,
}

impl HardwareModel {
    pub fn reload(&mut self);                       // calls Client::get_hardware
    pub fn apply_nickname(&mut self, key: QString, value: QString);  // empty → None
}
```

QML imports it via the existing `app.hardware` injection pattern
(see how `app.preferences` is wired). Component lifecycle: load on
page first-show, refresh on `SensorListBroadcast`.

## Validation & edge cases

### Nickname validation (server-side authoritative)

The daemon is the trust boundary; the GUI does identical client-side
validation for fast feedback, but the daemon never trusts it.

Rules in `crates/linsight-core/src/hardware.rs::validate_nickname`:
1. Apply `trim()`. If empty after trim, treat as "remove" (None).
2. Reject if `chars().count() > 64`.
3. Reject if any `c.is_control()` returns true (covers C0, C1,
   `\n`, `\r`, `\t`, NUL, etc.).
4. Otherwise accept verbatim. UTF-8 emoji / non-ASCII OK.

### Unknown device key

`SetNickname { device_key }` for a key not in the registry returns
`ProtoError { code: UnknownDevice, ... }`. Daemon does NOT silently
persist unknown keys — that would let a malicious or buggy client
poison the file. Unknown keys *loaded from disk* are preserved
verbatim (the drive may be re-attached later), but new writes
require a present device.

### Multiple GUIs connected at once

`SensorListBroadcast` goes to every transport. Each `OverviewModel`
updates its tile cache independently. The originating GUI sees both
the `Response` (immediate confirmation) and the `Broadcast` (label
refresh). Idempotent — the second updates produces no UI change.

### Daemon offline / GUI auto-spawn

Existing GUI auto-spawn path is unchanged. On daemon start the
hardware registry is rebuilt; nicknames are loaded from
`hardware.json`. The Hardware page renders a loading state until
`get_hardware` returns.

### pci.ids parse failure

If `/usr/share/hwdata/pci.ids` exists but parses incorrectly
(corrupt download, kernel update mid-write), the resolver logs a
warning and falls back to the hex-string label. No hard error.

### Hot-swap and `xe` enumeration order

The `xe` plugin enumerates cards in `card<N>` order, so its
`device_id` is index-based (`gpu0`, `gpu1`). If the user disables a
GPU in BIOS, the surviving GPU may move from `gpu1` to `gpu0` →
its `device_id` changes. The plugin's emitted `HardwareDeviceKey`
*stays the same* (`pci:0000:06:00.0`) because it's derived from the
sysfs PCI slot, not the enumeration index. Nicknames persist
correctly across this scenario. Same invariant applies to NVME and
NVML: the key is identity-derived, not enumeration-derived.

## Test plan

- **Unit (core):** `HardwareDeviceKey::try_new` accepts valid,
  rejects 8 invalid forms (empty, no scheme, uppercase, too long,
  bad chars, etc.). `validate_nickname` accepts emoji, rejects
  control chars (`\n`, NUL, etc.), rejects > 64 chars, trims
  correctly.
- **Unit (mirror):** `RHardwareDevice` round-trips via
  `From`/`Into`. Same hardening pattern as ADR-0001 v3:
  `cargo test --release -p linsight-plugin-sdk
  hardware_round_trips` MUST pass (catches any future stabby
  release-mode regressions).
- **Unit (mirror):** `RHardwareCategoryKind` discriminant variants
  round-trip in both debug and release.
- **Unit (per plugin):** Each in-tree plugin emits a non-empty
  `manifest.devices` on its synthetic sysroot fixture, and every
  `SensorDescriptor.device_key` resolves to a manifest entry.
- **Unit (pci.ids):** `PciIdDb::load` against a small fixture
  parses to the expected `(0x8086, 0xe223) → "Battlemage GPU"`
  mapping.
- **Unit (host_init):** A plugin manifest with a sensor pointing
  at an absent device_key is rejected with
  `PluginError::Manifest`.
- **Integration:** `HardwareRegistry::build` collects from a
  vector of fake manifests, deduplicates by key, applies
  nicknames. Conflict (two plugins same key) logs WARN and
  preserves first.
- **Integration:** `HardwareRegistry::set_nickname` round-trips
  through atomic write + reload. Empty value deletes the entry.
- **Integration:** Daemon emits `SensorListBroadcast` after a
  successful nickname change; in-process client receives it with
  updated `device_label`. Two simultaneous client connections both
  receive the broadcast.
- **Integration:** Prometheus exporter renders the two metric
  families correctly, with proper label escaping for a nickname
  containing `"`, `\`, and `\n`.
- **Integration (existing):** `crates/linsight-plugin-sdk/tests/
  dynamic_load.rs` updated to assert the echo plugin emits a v4
  manifest with at least one device, and the dlopened sample path
  picks up that device's key.
- **GUI smoke:** existing `gui-smoke.sh` extended with a
  `Ctrl+5` keyboard navigation step that lands on the Hardware
  page and asserts the device count is `>= 1`.

Test count target: `147 → 175+` net additions across the test
plan (a conservative estimate, may rise depending on resolver
parameterization). The 147 baseline is HEAD after the xe fdinfo
fix landed earlier today.

## Migration

- `hardware.json` absent on first run → empty nickname map, default
  labels (resolved model strings from plugin manifests), no error.
- `preferences.json` is **not modified**; this feature ships
  alongside, not on top of it.
- Old client (`PROTOCOL_VERSION=1`) connecting to new daemon: fails
  handshake with the existing version-mismatch error. User updates
  the GUI / CLI binary.
- v3 `.so` plugin loaded by v4 daemon: symbol lookup for
  `linsight_plugin_v4` fails (the v3 plugin only exports
  `linsight_plugin_v3`). Daemon logs an error and skips the plugin.
  Per CLAUDE.md, no third-party plugins exist yet, so this is a
  paper concern.
- `LINSIGHT_PLUGIN_ABI_VERSION` in the daemon constant bumps `3 → 4`
  to match the export-symbol rename. `just ci` enforces the
  in-tree plugins are all v4-correct.

## Risks worth re-checking before implementation

- **GUI auto-spawn assumes same-version daemon.** If a stale
  `linsightd` binary remains in `$PATH` after `cargo install`, the
  GUI may spawn it and fail handshake. The error path already
  surfaces `protocol mismatch: daemon=... gui=...`; this is
  acceptable but worth a smoke test.
- **`HashMap<u32, oneshot>` leak on unsent responses.** If the daemon
  crashes mid-request, the GUI's `inflight` map leaks the oneshot.
  Mitigation: a deadline timer on each request (5s default).
- **Prometheus metric explosion.** The info metric is a single time
  series per device — bounded. Per-sample label cardinality grows
  by `device_key` (one extra label per metric family, bounded by
  device count). Both safe.
- **Codex flagged disambiguator complexity.** If runtime cost of
  re-disambiguating on every nickname change is non-trivial, cache
  the per-device label inside the registry and re-render only on
  registry changes (not per-broadcast).
- **Stabby release-mode bug regression.** Per ADR-0001 v3, payload-
  bearing enums went `(kind, payload)` struct because stabby's
  recursive `match_owned` misroutes closures in release. Any new
  R-mirror type follows the same pattern — never `#[repr(stabby)]`
  on a payload-bearing enum. The release-mode round-trip test is
  mandatory (`cargo test --release -p linsight-plugin-sdk
  hardware_round_trips`).
- **Plugins doing pci.ids lookup at init() add cold-start latency.**
  The SDK helper caches via `OnceLock`, but the first plugin that
  triggers it eats the ~10 ms parse cost. Acceptable; document in
  the SDK helper's rustdoc.

## Out of scope / future work

- Sensor-level renames (`xe.gpu1.util` → "Main GPU usage"). Maybe
  later; would need its own per-sensor config layer.
- Custom icons per device.
- Drag-to-reorder devices on the Hardware page (sorted by category
  then key today).
- Power / temperature / clock graphs on the Hardware page itself
  (today the page is identity + nickname only; the existing GPUs /
  Storage pages still own metric rendering).
- Multi-machine nickname sync.
