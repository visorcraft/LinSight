<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# ADR-0002 — Plugin ABI v4: hardware manifest

**Status:** accepted, 2026-05-26
**Supersedes:** parts of ADR-0001 (v3 manifest layout)

## Context

LinSight's v0.3 sensor catalogue only knew about *sensors* — a flat list
of `SensorDescriptor { id, unit, kind, category, device_id, ... }`. The
upcoming Hardware page and per-device nicknames feature
(`docs/specs/2026-05-25-hardware-page.md`) needs first-class *device*
identity: which physical GPU/SSD/NIC each sensor belongs to, what its
model name is, where it sits in the PCI / NVME / NET namespace, and a
stable key the nickname store can index against (`pci:0000:06:00.0`,
`nvml:uuid:gpu-abc…`, etc.).

Two implementation options were considered:

1. **Daemon-side resolver.** Keep the plugin ABI unchanged; have the
   daemon synthesize devices by parsing each sensor's
   `device_id` and looking up sysfs/PCI metadata at registration time.
   *Rejected* because it spreads vendor-specific knowledge (xe vs
   nvml vs nvme) across the daemon, duplicating what each plugin
   already knows internally, and forces the daemon to re-walk sysfs
   for hardware the plugin already enumerated in `init`.

2. **Plugin-emits-devices.** Extend the manifest so each plugin
   reports its own `HardwareDevice` list and sensors carry an
   optional `device_key` pointing into it. *Chosen* — keeps
   hardware knowledge with the plugin that produced it, lets the
   daemon stay vendor-agnostic, and makes Phase D's plugin work
   self-contained per crate.

## Decision

Bump `LINSIGHT_PLUGIN_ABI_VERSION` 3 → 4. Rename the factory symbol
`linsight_plugin_v3` → `linsight_plugin_v4` so a v3 `.so` fails the
`StabbyLibrary::get_stabbied` lookup at load time rather than silently
exchanging incompatible manifest shapes.

### Manifest extensions

* `PluginManifest.devices: Vec<HardwareDevice>` — the plugin's
  hardware roster (vendor, model, location, plugin_device_id, key).
* `SensorDescriptor.device_key: Option<HardwareDeviceKey>` — optional
  back-reference into the manifest's `devices`. `None` is valid
  (e.g. system-wide sensors like `cpu.util` or `mem.total_bytes`
  that don't bind to a single device).

### R-mirror additions

`RHardwareCategoryKind` (`#[repr(u8)]` unit-only enum) and
`RHardwareDevice` (plain `#[repr(C)]` struct). Per ADR-0001's v3
lesson, **payload-bearing variants live on sibling structs, never on
stabby tagged enums**: the 36.2.2 `match_owned` release-mode misdispatch
makes that representation unsafe for any type whose variants carry
payloads, so we keep using the `(kind, payload_fields)` pattern that
v3 already established for RUnit/RReading/RCell.

### Host validation

`host_init` calls `validate_manifest(&r_manifest)` BEFORE the
From-conversion to std types. Three rules:

1. Every `RHardwareDevice::key` parses via `HardwareDeviceKey::try_new`
   (rejects empty strings, unknown schemes, bad chars, > 140 bytes).
2. Device keys are unique within the manifest's `devices` vector.
3. Every `RSensorDescriptor::device_key` (when `Some`) references a
   key that appears in `devices`.

Failures surface as a new `PluginError::Manifest(String)` variant.
This is host-side only; the matching `RPluginError` mirror is
unchanged because the plugin can't synthesize a `Manifest` error
across the FFI boundary — it never sees the validation step.

## Consequences

* Plugins self-report hardware identity in `init()`. No more
  hand-rolled sysfs resolvers in the daemon.
* Phase C introduces a small `PciIdDb` helper crate so plugins that
  enumerate PCI devices (xe, nvme, future amdgpu) can map vendor/device
  IDs → human-readable model strings without each pulling in a full
  hwids database.
* Phase D removes the `// TODO Phase D` placeholders the v3-compat
  scaffold left in every in-tree sensor's `init()` — each plugin
  emits its real `devices` vector and sets `device_key` on each
  sensor.
* Test count climbs by **5+** in `linsight-plugin-sdk` (one
  RHardwareCategoryKind round-trip, two RHardwareDevice round-trips,
  four validate_manifest tests) — the load-bearing safety net for
  the v3 lessons: every new R-mirror type must round-trip in BOTH
  debug AND release.
* v3 plugins are now incompatible. Operators rebuilding against
  the v0.4 SDK get the new manifest fields automatically; pre-built
  v3 `.so` files fail symbol lookup with `linsight_plugin_v4 not
  found`, which is louder and safer than the v2→v3 ABI-mismatch
  fallback (a numerically-different `linsight_plugin_abi_version`).
