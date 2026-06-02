<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Hardware Page + Per-Device Nicknames Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an in-app Hardware page that lists detected devices with real model strings ("Intel Arc B-series", "NVIDIA RTX 5080 Mobile") and accepts user-editable nicknames; propagate nicknames to GUI tiles, CLI output, and Prometheus exporter.

**Architecture:** Plugin SDK extends v3 → v4 so each plugin emits a `HardwareDevice` list alongside its sensors. Daemon collects manifests into a `HardwareRegistry`, persists nicknames in `~/.config/linsight/hardware.json`, decorates outgoing `SensorInfo` with stable `device_key` + display `device_label`. Protocol bumps v1 → v2 with request/response (req_id correlation) and a `SensorListBroadcast` for label refresh.

**Tech Stack:** Rust edition 2024 / rustc 1.95, stabby 36.2.2 for the FFI vtable, postcard for wire encoding, cxx-qt 0.8 for the Qt 6 / Kirigami GUI, serde + serde_json for the on-disk schema.

**Spec:** [`docs/superpowers/specs/2026-05-26-hardware-page-and-nicknames-design.md`](../specs/2026-05-26-hardware-page-and-nicknames-design.md)

**Test baseline:** 147 at HEAD (post-xe-fdinfo-fix). Target after this plan: 175+.

---

## Phase A — Core types (foundation)

No dependencies on anything else. Builds the `HardwareDeviceKey`, `HardwareCategory`, `HardwareDevice`, and `validate_nickname` in `linsight-core`. Both the SDK mirror and the protocol consume these.

### Task A1: HardwareCategory enum

**Files:**
- Create: `crates/linsight-core/src/hardware.rs`
- Modify: `crates/linsight-core/src/lib.rs`

- [ ] **Step 1: Create the module skeleton with the enum and a failing test.**

```rust
// crates/linsight-core/src/hardware.rs
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum HardwareCategory {
    Gpu,
    Storage,
    Network,
    Cpu,
    Other,
}

impl HardwareCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gpu => "gpu",
            Self::Storage => "storage",
            Self::Network => "network",
            Self::Cpu => "cpu",
            Self::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_as_str_is_stable() {
        assert_eq!(HardwareCategory::Gpu.as_str(), "gpu");
        assert_eq!(HardwareCategory::Storage.as_str(), "storage");
        assert_eq!(HardwareCategory::Network.as_str(), "network");
        assert_eq!(HardwareCategory::Cpu.as_str(), "cpu");
        assert_eq!(HardwareCategory::Other.as_str(), "other");
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`.**

Edit `crates/linsight-core/src/lib.rs` to add the module:

```rust
pub mod dashboard;
pub mod error;
pub mod hardware;
pub mod types;

pub use error::{CoreError, CoreResult};
pub use hardware::{HardwareCategory};
pub use types::*;
```

- [ ] **Step 3: Run the test.**

```bash
cargo test -p linsight-core hardware::tests::category_as_str_is_stable
```
Expected: 1 passed.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-core/src/{hardware.rs,lib.rs}
git commit -m "feat(core): add HardwareCategory enum"
```

### Task A2: HardwareDeviceKey newtype

**Files:**
- Modify: `crates/linsight-core/src/hardware.rs`
- Modify: `crates/linsight-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests in `hardware.rs`.** Add to the existing `tests` module:

```rust
    #[test]
    fn key_accepts_valid_schemes() {
        for s in [
            "pci:0000:06:00.0",
            "nvml:uuid:gpu-abc123-456",
            "nvme:eui.001b448b41234567",
            "nvme:nvme0",
            "net:enp4s0",
            "net:wg0",
            "cpu:0",
            "plugin:com.visorcraft.linsight.echo:demo",
        ] {
            assert!(HardwareDeviceKey::try_new(s).is_ok(), "should accept: {s}");
        }
    }

    #[test]
    fn key_rejects_invalid_forms() {
        for s in [
            "",
            "pci",
            "pci:",
            "FOO:bar",                  // unknown scheme
            "pci:0000:06:00.0 ",        // trailing space
            "PCI:0000:06:00.0",         // uppercase
            "pci:0000:06:00.0/extra",   // slash
            "x".repeat(200).as_str(),   // too long
        ] {
            assert!(HardwareDeviceKey::try_new(s).is_err(), "should reject: {s}");
        }
    }

    #[test]
    fn key_scheme_extraction() {
        let k = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        assert_eq!(k.scheme(), "pci");
        let k = HardwareDeviceKey::try_new("nvml:uuid:gpu-abc").unwrap();
        assert_eq!(k.scheme(), "nvml");
    }
```

- [ ] **Step 2: Implement the type.** Add above the `tests` mod:

```rust
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum KeyError {
    #[error("hardware device key is empty")]
    Empty,
    #[error("hardware device key too long ({0} bytes, max 140)")]
    TooLong(usize),
    #[error("hardware device key missing scheme prefix (expected one of: pci, nvml, nvme, net, cpu, plugin)")]
    NoScheme,
    #[error("hardware device key has unknown scheme '{0}'")]
    UnknownScheme(String),
    #[error("hardware device key payload empty after '{0}:'")]
    EmptyPayload(String),
    #[error("hardware device key contains invalid character {0:?}")]
    BadChar(char),
}

const ALLOWED_SCHEMES: &[&str] = &["pci", "nvml", "nvme", "net", "cpu", "plugin"];
const KEY_MAX_LEN: usize = 140;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct HardwareDeviceKey(String);

impl HardwareDeviceKey {
    pub fn try_new(s: impl Into<String>) -> Result<Self, KeyError> {
        let s = s.into();
        if s.is_empty() {
            return Err(KeyError::Empty);
        }
        if s.len() > KEY_MAX_LEN {
            return Err(KeyError::TooLong(s.len()));
        }
        let (scheme, rest) = s.split_once(':').ok_or(KeyError::NoScheme)?;
        if !ALLOWED_SCHEMES.contains(&scheme) {
            return Err(KeyError::UnknownScheme(scheme.to_owned()));
        }
        if rest.is_empty() {
            return Err(KeyError::EmptyPayload(scheme.to_owned()));
        }
        for c in rest.chars() {
            let ok = c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, '_' | ':' | '.' | '-');
            if !ok {
                return Err(KeyError::BadChar(c));
            }
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn scheme(&self) -> &str {
        self.0.split(':').next().unwrap_or("")
    }
}

impl std::fmt::Display for HardwareDeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
```

- [ ] **Step 3: Re-export.** Edit `crates/linsight-core/src/lib.rs`:

```rust
pub use hardware::{HardwareCategory, HardwareDevice, HardwareDeviceKey, KeyError};
```

(Note: `HardwareDevice` is not yet defined; the next task adds it. Leave the export pre-declared and `cargo build` will fail until A3 lands. Alternative: re-export only what exists yet, then update on A3. Pick whichever feels lower-friction — both are fine. The plan assumes you'll do A3 immediately after.)

- [ ] **Step 4: Run tests.**

```bash
cargo test -p linsight-core hardware::tests::
```
Expected: 4 passed (key_accepts_valid_schemes, key_rejects_invalid_forms, key_scheme_extraction, category_as_str_is_stable).

- [ ] **Step 5: Commit.**

```bash
git add crates/linsight-core/src/{hardware.rs,lib.rs}
git commit -m "feat(core): add HardwareDeviceKey newtype with validation"
```

### Task A3: HardwareDevice struct

**Files:**
- Modify: `crates/linsight-core/src/hardware.rs`

- [ ] **Step 1: Write the failing round-trip test.** Add to the `tests` module:

```rust
    #[test]
    fn device_serde_round_trip() {
        let dev = HardwareDevice {
            key: HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap(),
            category: HardwareCategory::Gpu,
            model: "Intel Arc B-series".into(),
            vendor: Some("Intel Corporation".into()),
            location: Some("PCI 0000:06:00.0".into()),
            plugin_id: "com.visorcraft.linsight.xe".into(),
            plugin_device_id: "gpu0".into(),
            sensor_ids: vec![SensorId::new("xe.gpu0.util")],
        };
        let s = serde_json::to_string(&dev).unwrap();
        let back: HardwareDevice = serde_json::from_str(&s).unwrap();
        assert_eq!(back, dev);
    }
```

- [ ] **Step 2: Implement `HardwareDevice`.** Add above the `tests` mod:

```rust
use crate::SensorId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HardwareDevice {
    pub key: HardwareDeviceKey,
    pub category: HardwareCategory,
    /// Canonical model string, no nickname applied.
    pub model: String,
    /// Optional vendor name, e.g. "Intel Corporation", "NVIDIA".
    pub vendor: Option<String>,
    /// Optional physical location for display, e.g. "PCI 0000:06:00.0", "USB 2-1".
    pub location: Option<String>,
    /// The plugin that emits sensors for this device. Set by the daemon
    /// when collecting manifests; plugins themselves do not set this.
    pub plugin_id: String,
    /// Plugin-local device identifier, e.g. "gpu0", "nvme0".
    pub plugin_device_id: String,
    /// Sensors this device is associated with. Filled by the daemon after
    /// all plugins have registered; plugins themselves leave this empty
    /// when emitting their manifest.
    pub sensor_ids: Vec<SensorId>,
}
```

- [ ] **Step 3: Run the test.**

```bash
cargo test -p linsight-core hardware::tests::device_serde_round_trip
```
Expected: 1 passed. Other hardware tests still pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-core/src/hardware.rs
git commit -m "feat(core): add HardwareDevice struct"
```

### Task A4: validate_nickname function

**Files:**
- Modify: `crates/linsight-core/src/hardware.rs`

- [ ] **Step 1: Write the failing tests.** Add to `tests`:

```rust
    #[test]
    fn nickname_validation_accepts_normal_text() {
        assert_eq!(validate_nickname("Battlemage"), Ok(Some("Battlemage".into())));
        assert_eq!(validate_nickname("OS drive"), Ok(Some("OS drive".into())));
        assert_eq!(validate_nickname("RTX 5080 \u{1F525}"), Ok(Some("RTX 5080 \u{1F525}".into())));
    }

    #[test]
    fn nickname_empty_means_delete() {
        assert_eq!(validate_nickname(""), Ok(None));
        assert_eq!(validate_nickname("   "), Ok(None));
        assert_eq!(validate_nickname("\t\t"), Err(NicknameError::ControlChar('\t')));
        // ^ tabs are control chars and rejected; trim() doesn't touch them mid-string.
        // The trim-only-spaces semantics are intentional: a user trying to enter
        // a tab as part of the name probably made a mistake, surface it.
    }

    #[test]
    fn nickname_trims_outer_whitespace() {
        assert_eq!(validate_nickname("  hello  "), Ok(Some("hello".into())));
    }

    #[test]
    fn nickname_rejects_control_chars() {
        assert!(matches!(validate_nickname("ab\nc"), Err(NicknameError::ControlChar('\n'))));
        assert!(matches!(validate_nickname("ab\x00c"), Err(NicknameError::ControlChar('\x00'))));
        assert!(matches!(validate_nickname("ab\x7fc"), Err(NicknameError::ControlChar('\x7f'))));
    }

    #[test]
    fn nickname_rejects_too_long() {
        let s = "x".repeat(65);
        assert!(matches!(validate_nickname(&s), Err(NicknameError::TooLong(65))));
        let s = "x".repeat(64);
        assert!(validate_nickname(&s).is_ok());
    }
```

Wait — re-read the second test above. The behavior I want: `trim()` strips leading / trailing ASCII whitespace (spaces, tabs, newlines, etc.). Then if empty, return None. If anything else, validate.

So `"\t\t"` trims to `""`, returns `Ok(None)`. Let me fix the test:

```rust
    #[test]
    fn nickname_empty_means_delete() {
        assert_eq!(validate_nickname(""), Ok(None));
        assert_eq!(validate_nickname("   "), Ok(None));
        assert_eq!(validate_nickname("\t\t"), Ok(None));
        // Inner control chars are still rejected — see nickname_rejects_control_chars.
    }
```

- [ ] **Step 2: Implement.** Add above `tests`:

```rust
#[derive(Debug, Error, PartialEq)]
pub enum NicknameError {
    #[error("nickname too long ({0} chars, max 64)")]
    TooLong(usize),
    #[error("nickname contains control char {0:?}")]
    ControlChar(char),
}

pub const NICKNAME_MAX_CHARS: usize = 64;

/// Validate and normalize a user-supplied nickname. Returns:
/// * `Ok(None)` if the input is empty after trimming (delete intent),
/// * `Ok(Some(s))` with the trimmed value if it's valid,
/// * `Err(_)` if rejected (length / control char).
pub fn validate_nickname(input: &str) -> Result<Option<String>, NicknameError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let char_count = trimmed.chars().count();
    if char_count > NICKNAME_MAX_CHARS {
        return Err(NicknameError::TooLong(char_count));
    }
    if let Some(c) = trimmed.chars().find(|c| c.is_control()) {
        return Err(NicknameError::ControlChar(c));
    }
    Ok(Some(trimmed.to_owned()))
}
```

- [ ] **Step 3: Update lib.rs re-exports.**

```rust
pub use hardware::{
    HardwareCategory, HardwareDevice, HardwareDeviceKey, KeyError,
    NicknameError, validate_nickname, NICKNAME_MAX_CHARS,
};
```

- [ ] **Step 4: Run tests.**

```bash
cargo test -p linsight-core hardware::tests::
```
Expected: 9 passed (the 4 from before + the 5 nickname tests).

- [ ] **Step 5: Commit.**

```bash
git add crates/linsight-core/src/{hardware.rs,lib.rs}
git commit -m "feat(core): add nickname validation"
```

---

## Phase B — Plugin SDK v3 → v4

Adds R-mirror types for the new core types, extends `PluginManifest` and `SensorDescriptor`, bumps the version constant, renames the export symbol, and hardens the host_init validator. ABI break — all in-tree plugins recompile against v4 in Phase D.

### Task B1: RHardwareCategoryKind discriminant enum

**Files:**
- Modify: `crates/linsight-plugin-sdk/src/mirror.rs`

- [ ] **Step 1: Write the failing round-trip tests.** Add to the existing `tests` mod in `mirror.rs`:

```rust
    #[test]
    fn hardware_category_kind_round_trips() {
        use linsight_core::HardwareCategory;
        for c in [
            HardwareCategory::Gpu,
            HardwareCategory::Storage,
            HardwareCategory::Network,
            HardwareCategory::Cpu,
            HardwareCategory::Other,
        ] {
            let r: RHardwareCategoryKind = c.into();
            let back: HardwareCategory = r.into();
            assert_eq!(back, c, "round trip failed for {:?}", c);
        }
    }
```

- [ ] **Step 2: Implement `RHardwareCategoryKind`.** Add to `mirror.rs`:

```rust
/// FFI-stable discriminant for `linsight_core::HardwareCategory`. Per
/// ADR-0001 v3 lessons, ALL discriminants are `#[repr(u8)]` unit-only
/// enums; payload-bearing variants live on a sibling struct (see
/// `RHardwareDevice`). This avoids the stabby `match_owned` release-mode
/// bug entirely.
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RHardwareCategoryKind {
    Gpu,
    Storage,
    Network,
    Cpu,
    Other,
}

impl From<linsight_core::HardwareCategory> for RHardwareCategoryKind {
    fn from(c: linsight_core::HardwareCategory) -> Self {
        match c {
            linsight_core::HardwareCategory::Gpu => Self::Gpu,
            linsight_core::HardwareCategory::Storage => Self::Storage,
            linsight_core::HardwareCategory::Network => Self::Network,
            linsight_core::HardwareCategory::Cpu => Self::Cpu,
            linsight_core::HardwareCategory::Other => Self::Other,
        }
    }
}

impl From<RHardwareCategoryKind> for linsight_core::HardwareCategory {
    fn from(r: RHardwareCategoryKind) -> Self {
        match r {
            RHardwareCategoryKind::Gpu => Self::Gpu,
            RHardwareCategoryKind::Storage => Self::Storage,
            RHardwareCategoryKind::Network => Self::Network,
            RHardwareCategoryKind::Cpu => Self::Cpu,
            RHardwareCategoryKind::Other => Self::Other,
        }
    }
}
```

- [ ] **Step 3: Run tests in both debug AND release.**

```bash
cargo test -p linsight-plugin-sdk hardware_category_kind_round_trips
cargo test --release -p linsight-plugin-sdk hardware_category_kind_round_trips
```
Expected: 1 passed in each.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-plugin-sdk/src/mirror.rs
git commit -m "feat(sdk): add RHardwareCategoryKind mirror"
```

### Task B2: RHardwareDevice mirror struct

**Files:**
- Modify: `crates/linsight-plugin-sdk/src/mirror.rs`

- [ ] **Step 1: Write the failing round-trip test.** Add to `tests`:

```rust
    #[test]
    fn hardware_device_round_trips_minimal() {
        use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey};

        let dev = HardwareDevice {
            key: HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap(),
            category: HardwareCategory::Gpu,
            model: "Intel Arc B-series".into(),
            vendor: None,
            location: None,
            plugin_id: String::new(),  // filled by daemon, plugin leaves empty
            plugin_device_id: "gpu0".into(),
            sensor_ids: vec![],
        };
        let r: RHardwareDevice = dev.clone().into();
        let back: HardwareDevice = r.into();
        assert_eq!(back, dev);
    }

    #[test]
    fn hardware_device_round_trips_with_options() {
        use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey};

        let dev = HardwareDevice {
            key: HardwareDeviceKey::try_new("nvml:uuid:gpu-abc").unwrap(),
            category: HardwareCategory::Gpu,
            model: "NVIDIA RTX 5080 Mobile".into(),
            vendor: Some("NVIDIA".into()),
            location: Some("PCI 0000:01:00.0".into()),
            plugin_id: String::new(),
            plugin_device_id: "gpu0".into(),
            sensor_ids: vec![],
        };
        let r: RHardwareDevice = dev.clone().into();
        let back: HardwareDevice = r.into();
        assert_eq!(back, dev);
    }
```

- [ ] **Step 2: Implement.** Add to `mirror.rs` (note: `SString` / `SOption` / `SVec` are the stabby imports already used elsewhere in the file — check existing types):

```rust
use stabby::option::Option as SOption;
use stabby::string::String as SString;
// (already imported earlier in the file; merge with existing import line)

/// FFI-stable mirror of `linsight_core::HardwareDevice`. The plugin
/// emits these alongside its sensors; the daemon validates each one
/// before integrating into its registry.
///
/// Note: `plugin_id` and `sensor_ids` are NOT on the wire from plugin
/// to host — the daemon fills `plugin_id` from the loader's knowledge
/// and `sensor_ids` from the manifest's sensors list. Including them
/// in the FFI mirror would invite plugins to lie about either.
#[stabby::stabby]
#[repr(C)]
#[derive(Clone, Debug)]
pub struct RHardwareDevice {
    pub key: SString,
    pub category_kind: RHardwareCategoryKind,
    pub model: SString,
    pub vendor: SOption<SString>,
    pub location: SOption<SString>,
    pub plugin_device_id: SString,
}

impl From<linsight_core::HardwareDevice> for RHardwareDevice {
    fn from(d: linsight_core::HardwareDevice) -> Self {
        Self {
            key: SString::from(d.key.as_str()),
            category_kind: d.category.into(),
            model: SString::from(d.model.as_str()),
            vendor: d.vendor.map(|s| SString::from(s.as_str())).into(),
            location: d.location.map(|s| SString::from(s.as_str())).into(),
            plugin_device_id: SString::from(d.plugin_device_id.as_str()),
        }
    }
}

impl From<RHardwareDevice> for linsight_core::HardwareDevice {
    fn from(r: RHardwareDevice) -> Self {
        // NOTE: From<RHardwareDevice> is infallible by contract; the FFI seam
        // (host_init) is responsible for re-validating the key string with
        // HardwareDeviceKey::try_new BEFORE this conversion runs. A plugin
        // emitting an invalid key value would land here via the wrapping
        // SDK helper, which logs+rejects before this code path.
        let key_str: String = r.key.as_str().to_owned();
        Self {
            key: linsight_core::HardwareDeviceKey::try_new(key_str)
                .expect("RHardwareDevice key was validated by host_init"),
            category: r.category_kind.into(),
            model: r.model.as_str().to_owned(),
            vendor: Option::from(r.vendor).map(|s: SString| s.as_str().to_owned()),
            location: Option::from(r.location).map(|s: SString| s.as_str().to_owned()),
            plugin_id: String::new(),
            plugin_device_id: r.plugin_device_id.as_str().to_owned(),
            sensor_ids: vec![],
        }
    }
}
```

- [ ] **Step 3: Run tests in both debug AND release.**

```bash
cargo test -p linsight-plugin-sdk hardware_device_round_trips
cargo test --release -p linsight-plugin-sdk hardware_device_round_trips
```
Expected: 2 passed in each.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-plugin-sdk/src/mirror.rs
git commit -m "feat(sdk): add RHardwareDevice mirror"
```

### Task B3: Extend `RPluginManifest` and `RSensorDescriptor`

**Files:**
- Modify: `crates/linsight-plugin-sdk/src/manifest.rs`
- Modify: `crates/linsight-plugin-sdk/src/mirror.rs`

- [ ] **Step 1: Read the existing `RPluginManifest` and `RSensorDescriptor` definitions.**

```bash
sed -n '/RPluginManifest\|RSensorDescriptor/,/^}/p' crates/linsight-plugin-sdk/src/manifest.rs | head -80
```
You're looking for the stabby-derive structs that mirror `PluginManifest` and `SensorDescriptor`. Take note of their field order — the new fields will be **appended at the end** (stabby uses positional field layout like `#[repr(C)]`, so appending is the only ABI-stable form).

- [ ] **Step 2: Append the new fields to the structs.** In `manifest.rs`, find `RSensorDescriptor` and add `device_key`:

```rust
#[stabby::stabby]
#[repr(C)]
pub struct RSensorDescriptor {
    // existing fields unchanged ...
    pub device_id: SOption<SString>,
    pub device_key: SOption<SString>,    // NEW in v4 — references manifest.devices entry
}
```

And `RPluginManifest`:

```rust
#[stabby::stabby]
#[repr(C)]
pub struct RPluginManifest {
    // existing fields unchanged ...
    pub sensors: SVec<RSensorDescriptor>,
    pub devices: SVec<RHardwareDevice>,  // NEW in v4
}
```

- [ ] **Step 3: Update the `From<PluginManifest> for RPluginManifest` and reverse `From` impls.** Each is in `manifest.rs`. For the manifest:

```rust
impl From<PluginManifest> for RPluginManifest {
    fn from(m: PluginManifest) -> Self {
        Self {
            plugin_id: SString::from(m.plugin_id.as_str()),
            display_name: SString::from(m.display_name.as_str()),
            version: SString::from(m.version.as_str()),
            sensors: m.sensors.into_iter().map(Into::into).collect(),
            devices: m.devices.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<RPluginManifest> for PluginManifest {
    fn from(r: RPluginManifest) -> Self {
        Self {
            plugin_id: r.plugin_id.as_str().to_owned(),
            display_name: r.display_name.as_str().to_owned(),
            version: r.version.as_str().to_owned(),
            sensors: r.sensors.into_iter().map(Into::into).collect(),
            devices: r.devices.into_iter().map(Into::into).collect(),
        }
    }
}
```

For `SensorDescriptor` / `RSensorDescriptor` the new field:
```rust
// In: From<SensorDescriptor> for RSensorDescriptor
device_key: m.device_key.map(|k| SString::from(k.as_str())).into(),

// In: From<RSensorDescriptor> for SensorDescriptor
device_key: Option::from(r.device_key)
    .map(|s: SString| s.as_str().to_owned())
    .map(|s| linsight_core::HardwareDeviceKey::try_new(s)
        .expect("RSensorDescriptor.device_key was validated by host_init")),
```

- [ ] **Step 4: Add `devices: Vec<HardwareDevice>` and `device_key: Option<HardwareDeviceKey>` to the host-side types.** `PluginManifest` is defined in `manifest.rs`; add the field. Same for `SensorDescriptor`:

```rust
pub struct PluginManifest {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub sensors: Vec<SensorDescriptor>,
    pub devices: Vec<HardwareDevice>,
}

pub struct SensorDescriptor {
    pub id: SensorId,
    pub display_name: String,
    pub unit: Unit,
    pub kind: SensorKind,
    pub category: Category,
    pub native_rate_hz: f32,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub device_id: Option<String>,
    pub device_key: Option<HardwareDeviceKey>,
}
```

(Field order matters for the From impls; keep `device_key` last to match the stabby append.)

- [ ] **Step 5: Run the cross-crate compile.**

```bash
cargo check --workspace
```
Expected: builds fail across in-tree plugins (they don't yet emit `devices` or `device_key`). That's OK — we'll fix them in Phase D. For now, only fix the SDK + echo-plugin compile errors so the SDK itself builds clean. Add the trivial `devices: vec![]` and `device_key: None` to the in-tree plugins' manifests with a `// TODO Phase D` comment so the workspace compiles.

(Bend the no-placeholders rule for these TODO comments — they're 9 lines total, deliberately temporary, and the rest of the plan removes them.)

- [ ] **Step 6: Re-run the workspace build + SDK tests.**

```bash
cargo build --workspace
cargo test -p linsight-plugin-sdk
cargo test --release -p linsight-plugin-sdk
```
Expected: build succeeds. All SDK tests pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/linsight-plugin-sdk/src/{manifest.rs,mirror.rs} \
        crates/linsight-sensors/{cpu,mem,xe,nvml,nvme,net}/src/ \
        examples/echo-plugin/src/lib.rs \
        crates/linsight-cli/src/commands/plugin.rs
git commit -m "feat(sdk): extend manifest with devices + device_key (v4 wire)"
```

### Task B4: Bump ABI version + rename export symbol

**Files:**
- Modify: `crates/linsight-plugin-sdk/src/lib.rs`
- Modify: `crates/linsight-plugin-sdk/src/export.rs`
- Modify: `apps/linsightd/src/plugin_host.rs`

- [ ] **Step 1: Bump the constant.** In `lib.rs`:

```rust
/// * v4: PluginManifest gains `devices: Vec<HardwareDevice>` and
///   SensorDescriptor gains `device_key: Option<HardwareDeviceKey>`
///   so each plugin reports its hardware identities for the
///   daemon's Hardware page + nickname store. v3 plugins fail
///   the symbol lookup at load (`linsight_plugin_v3` → `_v4`).
pub const LINSIGHT_PLUGIN_ABI_VERSION: u32 = 4;
```

- [ ] **Step 2: Rename the export symbol.** Find the `linsight_plugin_v3` symbol in `export.rs` and rename:

```bash
grep -n 'linsight_plugin_v3' crates/linsight-plugin-sdk/src/export.rs
```

Replace **every** occurrence of `linsight_plugin_v3` with `linsight_plugin_v4` in that file. The macro embeds the name string, so the rename propagates to all v4 plugins via the SDK alone.

- [ ] **Step 3: Update the daemon loader to look up `linsight_plugin_v4`.** In `apps/linsightd/src/plugin_host.rs`, search for the `v3` lookup:

```bash
grep -n 'linsight_plugin_v3\|v3' apps/linsightd/src/plugin_host.rs
```

Rename to `v4`.

- [ ] **Step 4: Build the workspace.**

```bash
cargo build --workspace
```
Expected: clean build. The echo example + in-tree plugins all pick up the new symbol via the export macro.

- [ ] **Step 5: Run the dynamic-load test.**

```bash
cargo test -p linsight-plugin-sdk --test dynamic_load
```
Expected: PASS (the echo plugin builds with the new SDK, exports the v4 symbol, and the daemon loader picks it up).

- [ ] **Step 6: Commit.**

```bash
git add crates/linsight-plugin-sdk/src/{lib.rs,export.rs} \
        apps/linsightd/src/plugin_host.rs
git commit -m "feat(sdk): bump LINSIGHT_PLUGIN_ABI_VERSION v3 -> v4"
```

### Task B5: host_init validates v4 manifest

**Files:**
- Modify: `crates/linsight-plugin-sdk/src/manifest.rs`

- [ ] **Step 1: Write the failing tests.** Locate the existing `host_init` tests in `manifest.rs` and add:

```rust
    #[test]
    fn host_init_rejects_invalid_device_key() {
        // A plugin that emits a HardwareDevice with a malformed key string
        // (must be caught at the FFI seam, not allowed to reach the registry).
        // Build an RPluginManifest manually with a key that fails try_new.
        let mut rm = make_minimal_manifest();
        rm.devices = stabby::vec::Vec::from([RHardwareDevice {
            key: SString::from("BAD KEY"),
            category_kind: RHardwareCategoryKind::Gpu,
            model: SString::from("Test"),
            vendor: stabby::option::Option::from(None::<SString>),
            location: stabby::option::Option::from(None::<SString>),
            plugin_device_id: SString::from("gpu0"),
        }]);
        let err = host_init_from_r(rm).expect_err("malformed key must reject");
        assert!(err.to_string().contains("BAD KEY"));
    }

    #[test]
    fn host_init_rejects_sensor_pointing_at_absent_device() {
        let mut rm = make_minimal_manifest_with_one_device();
        // Sensor references "pci:0000:99:99.9" which is not in manifest.devices.
        rm.sensors[0].device_key = stabby::option::Option::from(Some(SString::from("pci:0000:99:99.9")));
        let err = host_init_from_r(rm).expect_err("dangling device_key must reject");
        assert!(err.to_string().contains("pci:0000:99:99.9"));
    }

    #[test]
    fn host_init_rejects_duplicate_device_keys_within_manifest() {
        let mut rm = make_minimal_manifest_with_one_device();
        let dup = rm.devices[0].clone();
        rm.devices.push(dup);
        let err = host_init_from_r(rm).expect_err("duplicate keys must reject");
        assert!(err.to_string().contains("duplicate"));
    }
```

(`make_minimal_manifest` and `make_minimal_manifest_with_one_device` are test helpers — write them at the top of the test module if not already present.)

- [ ] **Step 2: Implement the validations.** In `host_init` (or in a helper called from `host_init`), add:

```rust
fn validate_manifest(m: &RPluginManifest) -> Result<(), PluginError> {
    use std::collections::HashSet;

    // 1. Every device key must be a valid HardwareDeviceKey.
    let mut keys = HashSet::new();
    for dev in m.devices.iter() {
        let key_str = dev.key.as_str();
        if let Err(e) = linsight_core::HardwareDeviceKey::try_new(key_str.to_owned()) {
            return Err(PluginError::Manifest(format!(
                "invalid device key {:?}: {}", key_str, e
            )));
        }
        if !keys.insert(key_str.to_owned()) {
            return Err(PluginError::Manifest(format!(
                "duplicate device key in manifest: {:?}", key_str
            )));
        }
    }

    // 2. Every SensorDescriptor.device_key, if Some, must match a manifest device.
    for s in m.sensors.iter() {
        if let stabby::option::Option::Some(key_s) = s.device_key.as_ref() {
            let k = key_s.as_str();
            if !keys.contains(k) {
                return Err(PluginError::Manifest(format!(
                    "sensor {} references device_key {:?} not in manifest.devices",
                    s.id.as_str(), k
                )));
            }
        }
    }

    Ok(())
}
```

Then call it from the existing `host_init` BEFORE the From-conversion runs:

```rust
pub fn host_init<P: LinsightPlugin>(plugin: &P, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
    let r_ctx: RPluginCtx = ctx.into();
    let r_result = plugin.init(&r_ctx);
    match r_result {
        SResult::Ok(rm) => {
            validate_manifest(&rm)?;
            // existing per-sensor SensorId validation continues below ...
        }
        // ...
    }
}
```

You also need `PluginError::Manifest(String)` as a new variant — add it to the existing enum in `plugin.rs` (or wherever `PluginError` lives) with its `From` reverse. Skip the FFI mirror addition for `Manifest` and just route through the existing `Io` for cross-FFI; the manifest variant only applies host-side.

- [ ] **Step 3: Run the new tests in debug AND release.**

```bash
cargo test -p linsight-plugin-sdk host_init_rejects
cargo test --release -p linsight-plugin-sdk host_init_rejects
```
Expected: 3 passed in each.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-plugin-sdk/src/{manifest.rs,plugin.rs}
git commit -m "feat(sdk): host_init validates v4 manifest (key shape, dangling, dupes)"
```

### Task B6: ADR-0002 documenting v4

**Files:**
- Create: `docs/adr/0002-plugin-abi-v4-hardware-manifest.md`

- [ ] **Step 1: Write a short ADR. ~50 lines.** Mirror the style of `docs/adr/0001-plugin-abi-stabby-deferral.md` (read it briefly first).

```markdown
<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# ADR-0002 — Plugin ABI v4: hardware manifest

**Status:** accepted, 2026-05-26.

## Context

v0.5 ships a per-device nickname system tied to a stable hardware
identity (PCI slot, NVML UUID, NVMe WWID, NIC ifname). The daemon
needs that identity for every sensor it advertises so user
nicknames propagate to GUI tile labels, CLI output, and Prometheus.

Two options were on the table:

1. **Daemon re-derives identity.** Daemon walks sysfs / NVML
   itself, keyed by `(plugin_id, device_id)`. v3 plugins
   unaffected. Requires duplicating every plugin's kernel-interface
   knowledge in the daemon.
2. **Plugins emit identity.** Plugins already enumerate their
   hardware to produce sensors; extend the manifest so they report
   identity alongside. ABI break.

Codex (gpt-5.5) reviewed and preferred option 1 to preserve v3
compatibility. We chose option 2 because no third-party `.so`
plugins exist yet, and option 1 places kernel-interface knowledge
in the daemon that already lives in the plugin.

## Decision

`LINSIGHT_PLUGIN_ABI_VERSION` bumps `3 → 4`. Symbol
`linsight_plugin_v3` renames to `linsight_plugin_v4`. v3 plugins
fail symbol lookup at load (the clean-error pattern from
ADR-0001 v3).

## Manifest extensions

`PluginManifest` gains `devices: Vec<HardwareDevice>`.
`SensorDescriptor` gains `device_key: Option<HardwareDeviceKey>`.
Both fields are appended at the end of their R-mirror structs
(stabby uses positional layout; appending is the only safe form).

## R-mirror additions

`RHardwareDevice` is a `(kind, payload)` struct per the v3
lesson — never a tagged enum, because stabby's release-mode
matcher misroutes closures (ADR-0001 v3 section). The category
discriminant is a `#[repr(u8)]` unit-only enum
(`RHardwareCategoryKind`); inactive payload defaults are SOption /
empty SVec.

## Host validation

`host_init` validates the v4 manifest BEFORE the From-conversion
runs:
- Every `RHardwareDevice.key` must parse via
  `HardwareDeviceKey::try_new`.
- No two devices in one manifest may share a key.
- Every `SensorDescriptor.device_key` (if `Some`) must match a
  manifest device key.

Validation failure returns `PluginError::Manifest(String)`. The
host logs the rejection and skips the plugin.

## Consequences

- Plugins now self-report hardware identity. Adding a sensor for a
  new device family means emitting a new `HardwareDevice` entry
  alongside its sensors, plus optionally setting
  `device_key` on each sensor descriptor.
- The SDK ships a `pciids::PciIdDb` helper for the common
  vendor:device lookup. Plugins that don't need PCI lookup don't
  pay its cost.
- Test count climbs ~20 from this ADR alone (mirror round-trips,
  validation paths, per-plugin manifest tests).
```

- [ ] **Step 2: Commit.**

```bash
git add docs/adr/0002-plugin-abi-v4-hardware-manifest.md
git commit -m "docs(adr): ADR-0002 plugin ABI v4 hardware manifest"
```

---

## Phase C — Shared pci.ids helper in the SDK

Used by xe + net plugins (cpu / nvml / nvme don't need it).

### Task C1: PciIdDb parser

**Files:**
- Create: `crates/linsight-plugin-sdk/src/pciids.rs`
- Modify: `crates/linsight-plugin-sdk/src/lib.rs`
- Create: `crates/linsight-plugin-sdk/tests/fixtures/mini-pci.ids`

- [ ] **Step 1: Create a tiny fixture file** at `crates/linsight-plugin-sdk/tests/fixtures/mini-pci.ids`:

```
# A minimal pci.ids fragment for tests. Real file lives at
# /usr/share/hwdata/pci.ids and follows the same syntax.
8086  Intel Corporation
	e223  Battlemage [Arc B-series]
	b0a0  Arrow Lake-S iGPU
10de  NVIDIA Corporation
	2782  GeForce RTX 4070
```

- [ ] **Step 2: Write the failing parser tests.** Create `crates/linsight-plugin-sdk/src/pciids.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/mini-pci.ids");

    #[test]
    fn parses_fixture_into_lookup_table() {
        let db = PciIdDb::parse(FIXTURE);
        assert_eq!(db.lookup(0x8086, 0xe223).as_deref(), Some("Battlemage [Arc B-series]"));
        assert_eq!(db.lookup(0x8086, 0xb0a0).as_deref(), Some("Arrow Lake-S iGPU"));
        assert_eq!(db.lookup(0x10de, 0x2782).as_deref(), Some("GeForce RTX 4070"));
    }

    #[test]
    fn lookup_misses_return_none() {
        let db = PciIdDb::parse(FIXTURE);
        assert!(db.lookup(0x8086, 0x0000).is_none());
        assert!(db.lookup(0xdead, 0xbeef).is_none());
    }

    #[test]
    fn vendor_name_lookup() {
        let db = PciIdDb::parse(FIXTURE);
        assert_eq!(db.vendor_name(0x8086).as_deref(), Some("Intel Corporation"));
        assert_eq!(db.vendor_name(0x10de).as_deref(), Some("NVIDIA Corporation"));
    }

    #[test]
    fn parse_skips_subdevice_lines() {
        // Real pci.ids has \t\t subdevice lines; we ignore them at this depth.
        let s = "8086  Intel\n\te223  Battlemage\n\t\t1234 5678  Some subsystem\n";
        let db = PciIdDb::parse(s);
        assert_eq!(db.lookup(0x8086, 0xe223).as_deref(), Some("Battlemage"));
    }
}
```

- [ ] **Step 3: Implement.** Above the tests in the same file:

```rust
//! Parser for the kernel's `pci.ids` database (`/usr/share/hwdata/
//! pci.ids` on Arch/Debian/Fedora/SUSE; `/usr/share/misc/pci.ids` on
//! Debian as an alternative).
//!
//! Format:
//! ```text
//! vendor_hex  Vendor Name
//! \tdevice_hex  Device Name
//! \t\tsubvendor_hex subdevice_hex  Subsystem Name
//! ```
//! We only parse the vendor and device levels; subsystem lines are
//! skipped. Lines starting with `#` and blank lines are ignored.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Default)]
pub struct PciIdDb {
    vendors: HashMap<u16, String>,
    devices: HashMap<(u16, u16), String>,
}

impl PciIdDb {
    pub fn parse(text: &str) -> Self {
        let mut db = Self::default();
        let mut current_vendor: Option<u16> = None;
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if line.starts_with("\t\t") {
                // subsystem line, skip
                continue;
            }
            if let Some(rest) = line.strip_prefix('\t') {
                // device line: "<dev_hex>  <name>"
                let Some(vendor) = current_vendor else { continue };
                let mut parts = rest.splitn(2, char::is_whitespace);
                let Some(dev_hex) = parts.next() else { continue };
                let name = parts.next().map(str::trim).unwrap_or("");
                if let Ok(dev) = u16::from_str_radix(dev_hex, 16) {
                    db.devices.insert((vendor, dev), name.to_owned());
                }
                continue;
            }
            // vendor line: "<vendor_hex>  <name>"
            let mut parts = line.splitn(2, char::is_whitespace);
            let Some(vendor_hex) = parts.next() else { continue };
            let name = parts.next().map(str::trim).unwrap_or("");
            if let Ok(v) = u16::from_str_radix(vendor_hex, 16) {
                current_vendor = Some(v);
                db.vendors.insert(v, name.to_owned());
            } else {
                current_vendor = None;
            }
        }
        db
    }

    pub fn lookup(&self, vendor: u16, device: u16) -> Option<String> {
        self.devices.get(&(vendor, device)).cloned()
    }

    pub fn vendor_name(&self, vendor: u16) -> Option<String> {
        self.vendors.get(&vendor).cloned()
    }

    /// Load from the canonical path, falling back to the Debian-alternate
    /// path. Returns an empty DB if neither file exists — callers should
    /// treat that as "no PCI ID lookups available" and use raw hex
    /// fallback labels.
    pub fn load_default() -> Self {
        for p in ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"] {
            if let Ok(text) = std::fs::read_to_string(Path::new(p)) {
                return Self::parse(&text);
            }
        }
        Self::default()
    }

    /// Process-wide cached default DB. First call parses; subsequent
    /// calls return the cached reference. Plugins that don't need PCI
    /// lookup never trigger the parse.
    pub fn shared() -> &'static Self {
        static CELL: OnceLock<PciIdDb> = OnceLock::new();
        CELL.get_or_init(Self::load_default)
    }
}
```

- [ ] **Step 4: Re-export.** In `crates/linsight-plugin-sdk/src/lib.rs`:

```rust
pub mod pciids;
```

- [ ] **Step 5: Run the tests.**

```bash
cargo test -p linsight-plugin-sdk pciids::tests
```
Expected: 4 passed.

- [ ] **Step 6: Commit.**

```bash
git add crates/linsight-plugin-sdk/src/{pciids.rs,lib.rs} \
        crates/linsight-plugin-sdk/tests/fixtures/mini-pci.ids
git commit -m "feat(sdk): add PciIdDb helper for plugin vendor:device lookups"
```

---

## Phase D — Update each in-tree plugin to v4

Each plugin enumerates its hardware (already does, for sensor enumeration) and reports it in `manifest.devices`. Each sensor's descriptor sets `device_key` to the matching device's key. Each plugin can be a parallel subagent task — no cross-plugin dependencies.

### Task D1: cpu plugin emits cpu:0

**Files:**
- Modify: `crates/linsight-sensors/cpu/src/plugin.rs`

- [ ] **Step 1: Write the failing manifest test.** Add to the existing `tests` mod:

```rust
    #[test]
    fn manifest_emits_cpu_device() {
        let plugin = CpuPlugin::default();
        let ctx = PluginCtx::new();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 1);
        let dev = &manifest.devices[0];
        assert_eq!(dev.key.as_str(), "cpu:0");
        assert_eq!(dev.category, linsight_core::HardwareCategory::Cpu);
        assert!(!dev.model.is_empty());
        // Every sensor in the manifest must point at this device.
        for s in &manifest.sensors {
            assert_eq!(s.device_key.as_ref().map(|k| k.as_str()), Some("cpu:0"));
        }
    }
```

- [ ] **Step 2: Implement.** In `init_inner`, build a `HardwareDevice` from `/proc/cpuinfo` and reference it from each sensor:

```rust
use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey};

fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
    let cpu_key = HardwareDeviceKey::try_new("cpu:0").expect("cpu:0 is a valid key");

    let model = cpu_model_name(ctx.sysroot()).unwrap_or_else(|| "CPU".into());

    let device = HardwareDevice {
        key: cpu_key.clone(),
        category: HardwareCategory::Cpu,
        model,
        vendor: None,
        location: None,
        plugin_id: String::new(),       // daemon fills
        plugin_device_id: "cpu".into(),
        sensor_ids: vec![],             // daemon fills
    };

    let sensors = vec![SensorDescriptor {
        id: SensorId::new("cpu.util"),
        display_name: "CPU utilization".into(),
        // existing fields unchanged ...
        device_id: Some("cpu".into()),
        device_key: Some(cpu_key.clone()),
    }];

    Ok(PluginManifest {
        plugin_id: "com.visorcraft.linsight.cpu".into(),
        display_name: "CPU".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        sensors,
        devices: vec![device],
    })
}

/// Read `/proc/cpuinfo` (rooted at `sysroot` if set) and return the first
/// `model name` line's value. Returns `None` on any read or parse failure.
fn cpu_model_name(sysroot: Option<&std::path::Path>) -> Option<String> {
    let path = match sysroot {
        Some(root) => root.join("proc/cpuinfo"),
        None => std::path::PathBuf::from("/proc/cpuinfo"),
    };
    let text = std::fs::read_to_string(&path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("model name") {
            if let Some(value) = rest.split_once(':').map(|(_, v)| v.trim()) {
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    None
}
```

- [ ] **Step 3: Run the test.**

```bash
cargo test -p linsight-sensors-cpu manifest_emits_cpu_device
```
Expected: 1 passed.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-sensors/cpu/src/plugin.rs
git commit -m "feat(sensors/cpu): emit cpu:0 hardware device in manifest"
```

### Task D2: mem plugin — no device

**Files:**
- Modify: `crates/linsight-sensors/mem/src/plugin.rs`

- [ ] **Step 1: Confirm via design.** RAM has no per-DIMM identity at LinSight's depth. The mem plugin emits a sensor with `device_key: None` — no `HardwareDevice` entry needed. This means the `devices: Vec<HardwareDevice>` field is `vec![]` and existing sensor descriptors set `device_key: None`.

- [ ] **Step 2: Write the manifest test.**

```rust
    #[test]
    fn manifest_emits_no_devices() {
        let plugin = MemPlugin::default();
        let ctx = PluginCtx::new();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert!(manifest.devices.is_empty());
        for s in &manifest.sensors {
            assert!(s.device_key.is_none());
        }
    }
```

- [ ] **Step 3: Update `init_inner`.** Set `devices: vec![]` and `device_key: None` on each sensor (replacing the `// TODO Phase D` placeholders left in Task B3).

- [ ] **Step 4: Run.**

```bash
cargo test -p linsight-sensors-mem manifest_emits_no_devices
```
Expected: pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/linsight-sensors/mem/src/plugin.rs
git commit -m "feat(sensors/mem): finalize empty devices list (memory has no per-DIMM identity)"
```

### Task D3: xe plugin emits pci: devices

**Files:**
- Modify: `crates/linsight-sensors/xe/src/plugin.rs`
- Modify: `crates/linsight-sensors/xe/Cargo.toml`

- [ ] **Step 1: Write the failing test.** Add to xe's plugin tests with a synthetic sysroot. The xe sensor crate already has sysfs-fixture helpers from earlier; reuse:

```rust
    #[test]
    fn manifest_emits_pci_devices_for_each_card() {
        let dir = tempfile::TempDir::new().unwrap();
        // Two xe cards with known PCI IDs.
        make_xe_card(dir.path(), 0, "0x8086", "0xb0a0");
        make_xe_card(dir.path(), 2, "0x8086", "0xe223");

        let plugin = XePlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();
        assert_eq!(manifest.devices.len(), 2);

        let keys: Vec<_> = manifest.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        assert!(keys.iter().any(|k| k.starts_with("pci:0000:")));

        // Every xe sensor should point at one of the manifest devices.
        let device_key_set: std::collections::HashSet<_> =
            manifest.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        for s in &manifest.sensors {
            let k = s.device_key.as_ref().expect("xe sensors must have device_key");
            assert!(device_key_set.contains(k.as_str()));
        }
    }

    fn make_xe_card(root: &std::path::Path, idx: u32, vendor: &str, device: &str) {
        let card = root.join(format!("sys/class/drm/card{idx}/device"));
        std::fs::create_dir_all(&card).unwrap();
        // Driver symlink whose basename is "xe":
        let driver_dir = root.join("sys/bus/pci/drivers/xe");
        std::fs::create_dir_all(&driver_dir).unwrap();
        std::os::unix::fs::symlink(&driver_dir, card.join("driver")).unwrap();
        std::fs::write(card.join("vendor"), format!("{vendor}\n")).unwrap();
        std::fs::write(card.join("device"), format!("{device}\n")).unwrap();
        // Minimal sysfs to keep enumerate() happy:
        std::fs::create_dir_all(card.join("tile0/gt0/freq0")).unwrap();
        std::fs::write(card.join("tile0/gt0/freq0/act_freq"), "1200\n").unwrap();
    }
```

- [ ] **Step 2: Add `linsight-plugin-sdk` re-export of `pciids` to the xe crate.** Edit `crates/linsight-sensors/xe/Cargo.toml`:

```toml
# Already depends on linsight-plugin-sdk; pciids is exposed through the SDK.
# No new dep needed.
```

- [ ] **Step 3: Implement device emission in `init_inner`.**

Modify `XeDevice` to retain vendor:device IDs read from sysfs (currently only PCI slot is kept). In `crates/linsight-sensors/xe/src/sysfs.rs`, extend the struct:

```rust
pub struct XeDevice {
    pub pci_slot: String,
    pub device_root: PathBuf,
    pub hwmon_root: Option<PathBuf>,
    pub vendor_id: Option<u16>,     // NEW: parsed from device/vendor
    pub device_id: Option<u16>,     // NEW: parsed from device/device
}
```

And in `enumerate()` parse `device/{vendor,device}` files (they contain `0x<hex>\n`). Add to `make_card` in the test helpers to keep the existing tests green.

In `plugin.rs::init_inner`, before building sensors:

```rust
use linsight_plugin_sdk::pciids::PciIdDb;
use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey};

let pci_db = PciIdDb::shared();

let mut devices = Vec::with_capacity(inner.devices.len());
let mut device_keys = Vec::with_capacity(inner.devices.len());

for (idx, dev) in inner.devices.iter().enumerate() {
    let key_str = format!("pci:{}", dev.pci_slot);
    let key = HardwareDeviceKey::try_new(key_str.clone())
        .map_err(|e| PluginError::Io(format!("xe gpu{idx} bad key: {e}")))?;

    let model = match (dev.vendor_id, dev.device_id) {
        (Some(v), Some(d)) => pci_db.lookup(v, d)
            .unwrap_or_else(|| format!("Intel GPU ({:04x}:{:04x})", v, d)),
        _ => format!("Intel GPU (gpu{idx})"),
    };
    let vendor = dev.vendor_id.and_then(|v| pci_db.vendor_name(v));

    devices.push(HardwareDevice {
        key: key.clone(),
        category: HardwareCategory::Gpu,
        model,
        vendor,
        location: Some(format!("PCI {}", dev.pci_slot)),
        plugin_id: String::new(),
        plugin_device_id: format!("gpu{idx}"),
        sensor_ids: vec![],
    });
    device_keys.push(key);
}
```

Then in each `SensorDescriptor`, set `device_key: Some(device_keys[idx].clone())`. Final manifest passes `devices` along.

- [ ] **Step 4: Run all xe tests.**

```bash
cargo test -p linsight-sensors-xe
```
Expected: existing 10 tests still pass + 1 new test = 11 pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/linsight-sensors/xe/src/{plugin.rs,sysfs.rs}
git commit -m "feat(sensors/xe): emit pci:<slot> hardware devices with pci.ids lookup"
```

### Task D4: nvml plugin emits nvml:uuid: devices

**Files:**
- Modify: `crates/linsight-sensors/nvml/src/lib.rs`

- [ ] **Step 1: Add a manifest test.** Note this test is hardware-dependent (requires NVIDIA + NVML), so mark `#[ignore]` like other live-NVML tests:

```rust
    #[test]
    #[ignore = "requires NVIDIA hardware + libnvidia-ml.so"]
    fn manifest_emits_nvml_uuid_device_per_gpu() {
        let plugin = NvmlPlugin::default();
        let ctx = PluginCtx::new();
        let manifest = host_init(&plugin, &ctx).unwrap();
        // A NVIDIA-equipped host has at least one device.
        assert!(!manifest.devices.is_empty());
        for d in &manifest.devices {
            assert!(d.key.as_str().starts_with("nvml:uuid:"));
            assert!(!d.model.is_empty());
        }
    }
```

Plus a no-hardware test that exercises the empty-manifest path:

```rust
    #[test]
    fn manifest_empty_when_nvml_missing() {
        // This test runs on hosts without libnvidia-ml.so; Nvml::init fails
        // and the plugin returns an empty manifest. Devices vec is also empty.
        let plugin = NvmlPlugin::default();
        let ctx = PluginCtx::new();
        let manifest = host_init(&plugin, &ctx).unwrap();
        if manifest.sensors.is_empty() {
            assert!(manifest.devices.is_empty());
        }
    }
```

- [ ] **Step 2: Implement.** Where the plugin currently builds sensors per device, also build a `HardwareDevice`. NVML's `dev.uuid()` returns "GPU-abc123-...". Key format: `nvml:uuid:<uuid-lowercased>`. Lowercase + strip the "GPU-" prefix? — no, the spec says use the full UUID. Pick one convention:

```rust
let uuid = dev.uuid().map_err(|e| PluginError::Io(e.to_string()))?;
// NVML uuid is like "GPU-abc123def-..."; normalize to lowercase for the key.
let key_payload = uuid.to_ascii_lowercase();
let key = HardwareDeviceKey::try_new(format!("nvml:uuid:{key_payload}"))
    .map_err(|e| PluginError::Io(format!("nvml gpu{i} bad uuid: {e}")))?;

let model = nvml.device_by_index(i)
    .and_then(|d| d.name())
    .unwrap_or_else(|_| format!("NVIDIA GPU (gpu{i})"));

let pci_slot = dev.pci_info().ok().and_then(|info| Some(info.bus_id));

devices.push(HardwareDevice {
    key: key.clone(),
    category: HardwareCategory::Gpu,
    model,
    vendor: Some("NVIDIA".into()),
    location: pci_slot.map(|s| format!("PCI {s}")),
    plugin_id: String::new(),
    plugin_device_id: format!("gpu{i}"),
    sensor_ids: vec![],
});
```

Set `device_key: Some(key.clone())` on each `nvml.gpuN.*` sensor.

(Note: the lowercase-uuid validation must succeed against the `HardwareDeviceKey` regex. NVML UUIDs are hex + hyphens — that matches `[a-z0-9_:.\-]`. Test with at least one fixture-derived UUID string in a no-hardware test to keep CI green without NVML.)

- [ ] **Step 3: Run.**

```bash
cargo test -p linsight-sensors-nvml manifest_
```
Expected: `manifest_empty_when_nvml_missing` passes; `manifest_emits_nvml_uuid_device_per_gpu` skipped.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-sensors/nvml/src/lib.rs
git commit -m "feat(sensors/nvml): emit nvml:uuid:<u> hardware devices"
```

### Task D5: nvme plugin emits nvme:<wwid> devices

**Files:**
- Modify: `crates/linsight-sensors/nvme/src/lib.rs`

- [ ] **Step 1: Add the failing test.** Build a synthetic sysroot with two nvme devices, one having `wwid`, one falling back to `serial`:

```rust
    #[test]
    fn manifest_emits_nvme_devices_with_wwid_preference() {
        let dir = tempfile::TempDir::new().unwrap();
        let n0 = dir.path().join("sys/class/nvme/nvme0");
        let n1 = dir.path().join("sys/class/nvme/nvme1");
        std::fs::create_dir_all(&n0).unwrap();
        std::fs::create_dir_all(&n1).unwrap();
        std::fs::write(n0.join("model"), "Samsung SSD 990 PRO 2TB\n").unwrap();
        std::fs::write(n0.join("wwid"), "eui.001b448b41234567\n").unwrap();
        std::fs::write(n0.join("serial"), "S6S2NJ0X123456\n").unwrap();
        // nvme1: no wwid, falls back to serial.
        std::fs::write(n1.join("model"), "WD_BLACK SN850X 1TB\n").unwrap();
        std::fs::write(n1.join("serial"), "WD-XYZ123\n").unwrap();
        // Plus the block stat fixtures the existing plugin enumerator needs.
        // (Reuse whatever helper already exists in nvme/src tests; if none,
        // add minimal block_stat fixtures.)

        let plugin = NvmePlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();

        let n0_dev = manifest.devices.iter()
            .find(|d| d.plugin_device_id == "nvme0").unwrap();
        assert_eq!(n0_dev.key.as_str(), "nvme:eui.001b448b41234567");
        assert!(n0_dev.model.contains("Samsung"));

        let n1_dev = manifest.devices.iter()
            .find(|d| d.plugin_device_id == "nvme1").unwrap();
        assert_eq!(n1_dev.key.as_str(), "nvme:wd-xyz123");  // lowercased + scheme
        // (Note: NVMe serials contain ASCII letters/digits; lowercase
        // normalization here matches the HardwareDeviceKey regex.)
    }
```

- [ ] **Step 2: Implement.** In `nvme/src/lib.rs::enumerate`, additionally read `wwid` and `serial`. In `init_inner`, build a `HardwareDevice` per controller:

```rust
let key_payload = wwid_or_serial(&dev)
    .map(|s| s.to_ascii_lowercase())
    .unwrap_or_else(|| dev.name.clone());
let key = HardwareDeviceKey::try_new(format!("nvme:{key_payload}"))
    .map_err(|e| PluginError::Io(format!("nvme {} bad key: {e}", dev.name)))?;

devices.push(HardwareDevice {
    key: key.clone(),
    category: HardwareCategory::Storage,
    model: dev.model.clone(),
    vendor: None,
    location: None,
    plugin_id: String::new(),
    plugin_device_id: dev.name.clone(),
    sensor_ids: vec![],
});
```

Each sensor's `device_key: Some(key.clone())`.

- [ ] **Step 3: Run.**

```bash
cargo test -p linsight-sensors-nvme
```
Expected: existing tests + 1 new = pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-sensors/nvme/src/lib.rs
git commit -m "feat(sensors/nvme): emit nvme:<wwid|serial> hardware devices"
```

### Task D6: net plugin emits net:<ifname> devices

**Files:**
- Modify: `crates/linsight-sensors/net/src/lib.rs`

- [ ] **Step 1: Add the failing test.** Use a synthetic `/sys/class/net` fixture with one PCI-backed (eth) interface and one virtual (wg, lo):

```rust
    #[test]
    fn manifest_emits_net_devices_with_pci_lookup_when_available() {
        let dir = tempfile::TempDir::new().unwrap();
        make_net_iface(dir.path(), "enp4s0", Some(("0x8086", "0x125c")));
        make_net_iface(dir.path(), "wg0", None);  // no PCI parent

        let plugin = NetPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = host_init(&plugin, &ctx).unwrap();

        let enp = manifest.devices.iter().find(|d| d.plugin_device_id == "enp4s0").unwrap();
        assert_eq!(enp.key.as_str(), "net:enp4s0");
        assert!(enp.vendor.is_some(), "PCI-backed NIC should have vendor");

        let wg = manifest.devices.iter().find(|d| d.plugin_device_id == "wg0").unwrap();
        assert_eq!(wg.key.as_str(), "net:wg0");
        assert!(wg.vendor.is_none(), "logical interface has no vendor");
    }
```

(`make_net_iface` helper: create `/sys/class/net/<if>` dir, and if a PCI parent is given, create `/sys/class/net/<if>/device/{vendor,device}` files.)

- [ ] **Step 2: Implement.** Per interface, build a `HardwareDevice`:

```rust
let key = HardwareDeviceKey::try_new(format!("net:{ifname}"))
    .map_err(|e| PluginError::Io(format!("net {ifname} bad key: {e}")))?;

let (vendor, model) = match read_pci_ids_for_iface(sysroot, &ifname) {
    Some((v, d)) => {
        let db = linsight_plugin_sdk::pciids::PciIdDb::shared();
        let model = db.lookup(v, d).unwrap_or_else(|| ifname.clone());
        (db.vendor_name(v), model)
    }
    None => (None, ifname.clone()),  // logical interface, no PCI parent
};

devices.push(HardwareDevice {
    key: key.clone(),
    category: HardwareCategory::Network,
    model,
    vendor,
    location: None,
    plugin_id: String::new(),
    plugin_device_id: ifname.clone(),
    sensor_ids: vec![],
});
```

Set `device_key: Some(key.clone())` on each net.<if>.* sensor.

- [ ] **Step 3: Run.**

```bash
cargo test -p linsight-sensors-net
```
Expected: pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-sensors/net/src/lib.rs
git commit -m "feat(sensors/net): emit net:<ifname> hardware devices"
```

### Task D7: echo example plugin

**Files:**
- Modify: `examples/echo-plugin/src/lib.rs`

- [ ] **Step 1: Add a single `HardwareDevice` to the example.**

```rust
let key = HardwareDeviceKey::try_new("plugin:com.visorcraft.linsight.echo:demo").unwrap();
Ok(PluginManifest {
    plugin_id: "com.visorcraft.linsight.echo".into(),
    display_name: "Echo example".into(),
    version: env!("CARGO_PKG_VERSION").into(),
    sensors: vec![SensorDescriptor {
        id: SensorId::new("example.echo.value"),
        display_name: "Echo value".into(),
        // existing fields ...
        device_id: Some("demo".into()),
        device_key: Some(key.clone()),
    }],
    devices: vec![HardwareDevice {
        key,
        category: HardwareCategory::Other,
        model: "Echo demo device".into(),
        vendor: None,
        location: None,
        plugin_id: String::new(),
        plugin_device_id: "demo".into(),
        sensor_ids: vec![],
    }],
})
```

- [ ] **Step 2: Update the dynamic-load test.** In `crates/linsight-plugin-sdk/tests/dynamic_load.rs`, assert the loaded plugin emits exactly one device with the expected key:

```rust
// Add to the existing test, after manifest is loaded:
assert_eq!(manifest.devices.len(), 1);
assert_eq!(manifest.devices[0].key.as_str(), "plugin:com.visorcraft.linsight.echo:demo");
```

- [ ] **Step 3: Run.**

```bash
cargo test -p linsight-plugin-sdk --test dynamic_load
```
Expected: pass.

- [ ] **Step 4: Commit.**

```bash
git add examples/echo-plugin/src/lib.rs crates/linsight-plugin-sdk/tests/dynamic_load.rs
git commit -m "feat(example/echo): emit demo HardwareDevice in manifest"
```

### Task D8: linsight-cli plugin new template

**Files:**
- Modify: `crates/linsight-cli/src/commands/plugin.rs`

- [ ] **Step 1: Update the scaffold template** (the inline string the command writes to `src/lib.rs` of a new plugin) to include the new manifest fields. Locate the template constant and add the `device_key` + `devices` lines, mirroring the echo plugin shape.

- [ ] **Step 2: Verify scaffolding builds.** The CLI `plugin new` command's tests usually call `cargo build` on the scaffolded output. If a smoke test exists, run it. Otherwise:

```bash
cargo run -p linsight-cli -- plugin new /tmp/scaffold-test
cd /tmp/scaffold-test && cargo build
cd - && rm -rf /tmp/scaffold-test
```
Expected: scaffold builds clean against v4 SDK.

- [ ] **Step 3: Commit.**

```bash
git add crates/linsight-cli/src/commands/plugin.rs
git commit -m "chore(cli): update plugin new template for v4 SDK"
```

---

## Phase E — Protocol v1 → v2

### Task E1: Extend `SensorInfo` with `device_key` + `device_label`

**Files:**
- Modify: `crates/linsight-protocol/src/messages.rs`

- [ ] **Step 1: Append the fields** to `SensorInfo` at the END (per the wire-stability comment in the file):

```rust
pub struct SensorInfo {
    // existing fields unchanged ...
    pub device_id: Option<String>,
    pub plugin_id: String,
    pub device_key: Option<String>,    // NEW in v2
    pub device_label: Option<String>,  // NEW in v2
}
```

- [ ] **Step 2: Update the round-trip test** that exercises `SensorInfo` to include the new fields with `None`/`Some` values.

- [ ] **Step 3: Run.**

```bash
cargo test -p linsight-protocol sensor_list_round_trips
```
Expected: pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-protocol/src/messages.rs
git commit -m "feat(protocol): add device_key + device_label to SensorInfo (v2)"
```

### Task E2: Bump PROTOCOL_VERSION = 2

**Files:**
- Modify: `crates/linsight-protocol/src/lib.rs`

- [ ] **Step 1: Bump.**

```rust
pub const PROTOCOL_VERSION: u32 = 2;
```

- [ ] **Step 2: Run handshake tests.**

```bash
cargo test -p linsight-protocol handshake
```
Expected: pass (the tests already use the constant so they all bump together).

- [ ] **Step 3: Commit.**

```bash
git add crates/linsight-protocol/src/lib.rs
git commit -m "feat(protocol): bump PROTOCOL_VERSION 1 -> 2"
```

### Task E3: Request/response with req_id correlation

**Files:**
- Modify: `crates/linsight-protocol/src/messages.rs`

- [ ] **Step 1: Write failing round-trip tests.**

```rust
    #[test]
    fn request_get_hardware_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 42,
            op: RequestOp::GetHardware,
        });
    }

    #[test]
    fn request_set_nickname_round_trips() {
        round_trip(ClientMsg::Request {
            req_id: 7,
            op: RequestOp::SetNickname {
                device_key: "pci:0000:06:00.0".into(),
                value: Some("Battlemage".into()),
            },
        });
        round_trip(ClientMsg::Request {
            req_id: 8,
            op: RequestOp::SetNickname {
                device_key: "pci:0000:06:00.0".into(),
                value: None,
            },
        });
    }

    #[test]
    fn response_hardware_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 42,
            result: Ok(ResponsePayload::Hardware(vec![])),
        });
    }

    #[test]
    fn response_error_round_trips() {
        round_trip(ServerMsg::Response {
            req_id: 7,
            result: Err(ProtoError {
                code: ProtoErrorCode::UnknownDevice,
                message: "no such device".into(),
            }),
        });
    }

    #[test]
    fn sensor_list_broadcast_round_trips() {
        round_trip(ServerMsg::SensorListBroadcast(vec![]));
    }
```

- [ ] **Step 2: Add the variants. Append at the END of each enum** (postcard wire-format positional).

In `ClientMsg`:

```rust
pub enum ClientMsg {
    // existing variants unchanged ...
    Goodbye,
    Request { req_id: u32, op: RequestOp },  // NEW
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RequestOp {
    GetHardware,
    SetNickname { device_key: String, value: Option<String> },
}
```

In `ServerMsg`:

```rust
pub enum ServerMsg {
    // existing variants unchanged ...
    Bye { reason: String },
    Response { req_id: u32, result: Result<ResponsePayload, ProtoError> },  // NEW
    SensorListBroadcast(Vec<SensorInfo>),  // NEW
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResponsePayload {
    Hardware(Vec<linsight_core::HardwareDevice>),
    NicknameSet { device_key: String, value: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtoError {
    pub code: ProtoErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProtoErrorCode {
    UnknownDevice,
    InvalidNickname,
    Io,
    Internal,
}
```

- [ ] **Step 3: Run.**

```bash
cargo test -p linsight-protocol
```
Expected: 5 new tests pass, all existing pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/linsight-protocol/src/messages.rs
git commit -m "feat(protocol): add Request/Response/SensorListBroadcast (v2)"
```

---

## Phase F — Daemon hardware registry

### Task F1: nickname_store.rs

**Files:**
- Create: `apps/linsightd/src/nickname_store.rs`
- Modify: `apps/linsightd/src/lib.rs` (or main.rs depending on layout)

- [ ] **Step 1: Write the failing tests.**

```rust
// nickname_store.rs
#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own XDG_CONFIG_HOME so they don't trample
    /// the user's real ~/.config. Mirrors the pattern in PreferencesModel.
    #[test]
    fn default_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hardware.json");
        let store = NicknameStore::load(&path);
        assert!(store.nicknames.is_empty());
    }

    #[test]
    fn round_trip_via_atomic_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hardware.json");
        let mut store = NicknameStore::default();
        store.nicknames.insert("pci:0000:06:00.0".into(), "Battlemage".into());
        store.save(&path).unwrap();

        let back = NicknameStore::load(&path);
        assert_eq!(back.nicknames.get("pci:0000:06:00.0").unwrap(), "Battlemage");
    }

    #[test]
    fn malformed_renames_to_bad_and_returns_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hardware.json");
        std::fs::write(&path, "not json {{{").unwrap();
        let store = NicknameStore::load(&path);
        assert!(store.nicknames.is_empty());
        assert!(path.with_extension("json.bad").exists());
    }

    #[test]
    fn unknown_keys_are_preserved_through_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hardware.json");
        // Pre-seed a file with a nickname for a device that isn't currently
        // present — the user may re-attach the drive later.
        std::fs::write(
            &path,
            r#"{"schema_version":1,"nicknames":{"nvme:eui.detached":"backup drive"}}"#,
        ).unwrap();
        let store = NicknameStore::load(&path);
        assert_eq!(store.nicknames.get("nvme:eui.detached").unwrap(), "backup drive");
    }
}
```

- [ ] **Step 2: Implement** following the `preferences.json` pattern (atomic write, .bad rename, schema_version):

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct NicknameStore {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub nicknames: HashMap<String, String>,
}

fn default_schema_version() -> u32 { 1 }

impl NicknameStore {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else { return Self::default() };
        match serde_json::from_str::<Self>(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = ?e, path = %path.display(), "malformed hardware.json; renaming to .bad");
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
```

- [ ] **Step 3: Wire into `apps/linsightd/src/lib.rs`.**

```rust
pub mod hardware;
pub mod nickname_store;
```

- [ ] **Step 4: Run.**

```bash
cargo test -p linsightd nickname_store::tests
```
Expected: 4 passed.

- [ ] **Step 5: Commit.**

```bash
git add apps/linsightd/src/{nickname_store.rs,lib.rs}
git commit -m "feat(daemon): hardware.json store with atomic write"
```

### Task F2: HardwareRegistry::build

**Files:**
- Create: `apps/linsightd/src/hardware.rs`

- [ ] **Step 1: Write the failing test.**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey, SensorId};
    use linsight_plugin_sdk::SensorDescriptor;
    use linsight_plugin_sdk::PluginManifest;

    fn fake_xe_manifest() -> PluginManifest {
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        PluginManifest {
            plugin_id: "com.visorcraft.linsight.xe".into(),
            display_name: "Intel xe".into(),
            version: "0.4.0".into(),
            devices: vec![HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Gpu,
                model: "Intel Arc B-series".into(),
                vendor: Some("Intel".into()),
                location: Some("PCI 0000:06:00.0".into()),
                plugin_id: String::new(),
                plugin_device_id: "gpu0".into(),
                sensor_ids: vec![],
            }],
            sensors: vec![SensorDescriptor {
                id: SensorId::new("xe.gpu0.util"),
                display_name: "Intel GPU utilization".into(),
                unit: linsight_core::Unit::Percent,
                kind: linsight_core::SensorKind::Scalar,
                category: linsight_core::Category::Gpu,
                native_rate_hz: 2.0,
                min: Some(0.0),
                max: Some(100.0),
                device_id: Some("gpu0".into()),
                device_key: Some(key),
            }],
        }
    }

    #[test]
    fn build_collects_devices_per_plugin() {
        let manifests = vec![fake_xe_manifest()];
        let nicknames = std::collections::HashMap::new();
        let registry = HardwareRegistry::build(&manifests, nicknames);
        assert_eq!(registry.devices.len(), 1);
        let dev = registry.devices.values().next().unwrap();
        assert_eq!(dev.plugin_id, "com.visorcraft.linsight.xe");
        assert_eq!(dev.sensor_ids.len(), 1);
        assert_eq!(dev.sensor_ids[0].as_str(), "xe.gpu0.util");
    }

    #[test]
    fn build_applies_nickname_in_label() {
        let manifests = vec![fake_xe_manifest()];
        let mut nicknames = std::collections::HashMap::new();
        nicknames.insert("pci:0000:06:00.0".into(), "Battlemage".into());
        let registry = HardwareRegistry::build(&manifests, nicknames);
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        assert_eq!(registry.device_label_for(&key), "Battlemage");
    }

    #[test]
    fn label_falls_back_to_model_when_no_nickname() {
        let manifests = vec![fake_xe_manifest()];
        let registry = HardwareRegistry::build(&manifests, std::collections::HashMap::new());
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        assert_eq!(registry.device_label_for(&key), "Intel Arc B-series");
    }

    #[test]
    fn duplicate_keys_across_plugins_log_warning_and_keep_first() {
        let mut m1 = fake_xe_manifest();
        m1.plugin_id = "io.first".into();
        let mut m2 = fake_xe_manifest();
        m2.plugin_id = "io.second".into();
        let registry = HardwareRegistry::build(&[m1, m2], Default::default());
        assert_eq!(registry.devices.len(), 1);  // dedup
        let dev = registry.devices.values().next().unwrap();
        assert_eq!(dev.plugin_id, "io.first");  // first wins
    }

    #[test]
    fn set_nickname_then_get_label() {
        let mut registry = HardwareRegistry::build(&[fake_xe_manifest()], Default::default());
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        registry.set_nickname(&key, Some("Battlemage".into())).unwrap();
        assert_eq!(registry.device_label_for(&key), "Battlemage");
    }

    #[test]
    fn set_nickname_unknown_key_returns_err() {
        let mut registry = HardwareRegistry::build(&[fake_xe_manifest()], Default::default());
        let bad = HardwareDeviceKey::try_new("pci:0000:ff:ff.f").unwrap();
        assert!(registry.set_nickname(&bad, Some("X".into())).is_err());
    }
}
```

- [ ] **Step 2: Implement.** Above the tests:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use linsight_core::{HardwareDevice, HardwareDeviceKey, NicknameError};
use linsight_plugin_sdk::PluginManifest;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("device key not found: {0}")]
    UnknownDevice(String),
    #[error("invalid nickname: {0}")]
    InvalidNickname(#[from] NicknameError),
}

#[derive(Debug, Default)]
pub struct HardwareRegistry {
    pub devices: HashMap<HardwareDeviceKey, HardwareDevice>,
    by_plugin: HashMap<(String, String), HardwareDeviceKey>,
    nicknames: HashMap<HardwareDeviceKey, String>,
}

impl HardwareRegistry {
    /// Build from a slice of plugin manifests (each annotated with the
    /// plugin's own plugin_id; for the in-tree plugins that's already in
    /// the manifest) and a nickname map keyed by device-key string.
    pub fn build(
        manifests: &[PluginManifest],
        nicknames: HashMap<String, String>,
    ) -> Self {
        let mut registry = Self::default();
        // Sensors -> device association.
        // Per-plugin pass.
        for m in manifests {
            // Fold devices first so the sensor pass can attach sensor_ids.
            for d in &m.devices {
                let key = d.key.clone();
                if registry.devices.contains_key(&key) {
                    tracing::warn!(
                        plugin_id = %m.plugin_id,
                        device_key = %key,
                        "duplicate device key across plugins; keeping first occurrence",
                    );
                    continue;
                }
                let mut dev = d.clone();
                dev.plugin_id = m.plugin_id.clone();
                registry.by_plugin.insert(
                    (m.plugin_id.clone(), dev.plugin_device_id.clone()),
                    key.clone(),
                );
                registry.devices.insert(key, dev);
            }
            for s in &m.sensors {
                if let Some(dk) = &s.device_key {
                    if let Some(dev) = registry.devices.get_mut(dk) {
                        dev.sensor_ids.push(s.id.clone());
                    }
                }
            }
        }
        for (k_str, nickname) in nicknames {
            if let Ok(key) = HardwareDeviceKey::try_new(k_str.clone()) {
                // Unknown keys are preserved in the on-disk file (loaded
                // back into the next save), but we don't add them to the
                // active registry's lookup. They re-attach silently if
                // the device returns.
                if registry.devices.contains_key(&key) {
                    registry.nicknames.insert(key, nickname);
                }
            }
        }
        registry
    }

    pub fn device_label_for(&self, key: &HardwareDeviceKey) -> String {
        if let Some(n) = self.nicknames.get(key) {
            return n.clone();
        }
        if let Some(d) = self.devices.get(key) {
            return d.model.clone();
        }
        key.as_str().to_owned()
    }

    pub fn key_for(&self, plugin_id: &str, device_id: &str) -> Option<&HardwareDeviceKey> {
        self.by_plugin.get(&(plugin_id.to_owned(), device_id.to_owned()))
    }

    pub fn set_nickname(
        &mut self,
        key: &HardwareDeviceKey,
        value: Option<String>,
    ) -> Result<(), RegistryError> {
        if !self.devices.contains_key(key) {
            return Err(RegistryError::UnknownDevice(key.to_string()));
        }
        match value {
            Some(v) => {
                // Re-validate (defense in depth — caller should already have).
                let normalized = linsight_core::validate_nickname(&v)?;
                match normalized {
                    Some(n) => { self.nicknames.insert(key.clone(), n); }
                    None => { self.nicknames.remove(key); }
                }
            }
            None => { self.nicknames.remove(key); }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Vec<HardwareDevice> {
        let mut v: Vec<_> = self.devices.values().cloned().collect();
        v.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
        v
    }

    pub fn nicknames_snapshot(&self) -> HashMap<String, String> {
        self.nicknames.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }
}
```

- [ ] **Step 3: Run.**

```bash
cargo test -p linsightd hardware::tests
```
Expected: 6 passed.

- [ ] **Step 4: Commit.**

```bash
git add apps/linsightd/src/hardware.rs
git commit -m "feat(daemon): HardwareRegistry — collect, dedupe, nickname"
```

### Task F3: Wire registry into daemon startup

**Files:**
- Modify: `apps/linsightd/src/main.rs` (or wherever the daemon initializes plugins)
- Modify: `apps/linsightd/src/plugin_host.rs`

- [ ] **Step 1: Read existing daemon startup flow.**

```bash
grep -n 'register\|PluginManifest\|plugin_host\|HardwareRegistry' apps/linsightd/src/main.rs apps/linsightd/src/plugin_host.rs
```

- [ ] **Step 2: After all plugins have registered, collect manifests into a `HardwareRegistry`.** In `plugin_host.rs` (or main.rs):

```rust
use crate::{hardware::HardwareRegistry, nickname_store::NicknameStore};
use std::sync::{Arc, RwLock};

// After plugins are registered, gather their manifests:
let manifests: Vec<PluginManifest> = host.plugins().iter()
    .map(|p| p.manifest().clone())
    .collect();

let store_path = nickname_store_path();
let store = NicknameStore::load(&store_path);
let registry = HardwareRegistry::build(&manifests, store.nicknames.clone());
let registry = Arc::new(RwLock::new(registry));
```

Helper for the path:

```rust
fn nickname_store_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(home).join("linsight/hardware.json");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".config/linsight/hardware.json")
}
```

Pass `Arc<RwLock<HardwareRegistry>>` to the transport / scheduler / Prometheus exporter as needed.

- [ ] **Step 3: Run the daemon briefly to confirm no startup regression.**

```bash
pkill -x linsightd 2>/dev/null
sleep 1
LINSIGHT_LOG=info nohup ./target/debug/linsightd > /tmp/linsightd.log 2>&1 &
disown
sleep 2
pkill -x linsightd
grep -i 'hardware\|registry' /tmp/linsightd.log
```
Expected: registry-build INFO line present, no errors.

- [ ] **Step 4: Commit.**

```bash
git add apps/linsightd/src/{main.rs,plugin_host.rs}
git commit -m "feat(daemon): wire HardwareRegistry into startup"
```

### Task F4: Decorate `SensorInfo` on the wire

**Files:**
- Modify: `apps/linsightd/src/transport/unix.rs`

- [ ] **Step 1: Locate the existing `SensorInfo` construction.**

```bash
grep -n 'SensorInfo' apps/linsightd/src/transport/unix.rs
```

- [ ] **Step 2: Inject `device_key` + `device_label` for each outgoing SensorInfo.** Pass the registry as an `Arc<RwLock<...>>` to the transport handler. In the place that maps a `SensorDescriptor` to a `linsight_protocol::SensorInfo`:

```rust
let registry_read = registry.read().unwrap();
let device_key = d.device_key.as_ref().map(|k| k.to_string());
let device_label = d.device_key.as_ref()
    .map(|k| registry_read.device_label_for(k));
// ...
linsight_protocol::SensorInfo {
    // existing fields ...
    device_id: d.device_id.clone(),
    plugin_id: plugin_id.clone(),
    device_key,
    device_label,
}
```

- [ ] **Step 3: Verify via linsight-cli.**

```bash
pkill -x linsightd 2>/dev/null; sleep 1
LINSIGHT_LOG=info nohup ./target/debug/linsightd > /tmp/linsightd.log 2>&1 &
disown
sleep 2
./target/debug/linsight-cli list | head -5
pkill -x linsightd
```
Expected: list output includes lines with model strings. (CLI may need a tiny update in Task E to display device_label; OK to follow-up.)

- [ ] **Step 4: Commit.**

```bash
git add apps/linsightd/src/transport/unix.rs
git commit -m "feat(daemon): decorate SensorInfo with device_key + device_label"
```

### Task F5: Dispatch `Request { GetHardware }`

**Files:**
- Modify: `apps/linsightd/src/transport/unix.rs`

- [ ] **Step 1: Add the handler arm.** In the message-dispatch loop:

```rust
ClientMsg::Request { req_id, op } => match op {
    RequestOp::GetHardware => {
        let snapshot = registry.read().unwrap().snapshot();
        writer.send(&ServerMsg::Response {
            req_id,
            result: Ok(ResponsePayload::Hardware(snapshot)),
        })?;
    }
    RequestOp::SetNickname { .. } => {
        // implemented in Task F6
        unimplemented!("F6")
    }
},
```

- [ ] **Step 2: Run.** No specific test yet (integration test lands in F7); just build+CI.

```bash
cargo build -p linsightd
```

- [ ] **Step 3: Commit.**

```bash
git add apps/linsightd/src/transport/unix.rs
git commit -m "feat(daemon): handle Request{GetHardware} → HardwareList"
```

### Task F6: Dispatch `Request { SetNickname }` + broadcast

**Files:**
- Modify: `apps/linsightd/src/transport/unix.rs`

- [ ] **Step 1: Replace the `unimplemented!()`** with full logic:

```rust
RequestOp::SetNickname { device_key, value } => {
    let key = match HardwareDeviceKey::try_new(device_key.clone()) {
        Ok(k) => k,
        Err(e) => {
            writer.send(&ServerMsg::Response {
                req_id,
                result: Err(ProtoError {
                    code: ProtoErrorCode::InvalidNickname,
                    message: format!("bad device key: {e}"),
                }),
            })?;
            continue;
        }
    };

    let normalized = match value.as_deref().map(linsight_core::validate_nickname).transpose() {
        Ok(opt) => opt.flatten(),  // Option<Option<String>> -> Option<String>
        Err(e) => {
            writer.send(&ServerMsg::Response {
                req_id,
                result: Err(ProtoError {
                    code: ProtoErrorCode::InvalidNickname,
                    message: format!("{e}"),
                }),
            })?;
            continue;
        }
    };

    let mut reg = registry.write().unwrap();
    if let Err(e) = reg.set_nickname(&key, value.clone()) {
        writer.send(&ServerMsg::Response {
            req_id,
            result: Err(ProtoError {
                code: ProtoErrorCode::UnknownDevice,
                message: format!("{e}"),
            }),
        })?;
        continue;
    }

    // Persist BEFORE broadcasting (crash-between-2-and-3 must not lose user's edit).
    let store_path = nickname_store_path();
    let store = NicknameStore { schema_version: 1, nicknames: reg.nicknames_snapshot() };
    if let Err(e) = store.save(&store_path) {
        tracing::error!(error = ?e, "hardware.json save failed");
        writer.send(&ServerMsg::Response {
            req_id,
            result: Err(ProtoError {
                code: ProtoErrorCode::Io,
                message: format!("save failed: {e}"),
            }),
        })?;
        continue;
    }
    drop(reg);

    // Confirm to caller, then broadcast.
    writer.send(&ServerMsg::Response {
        req_id,
        result: Ok(ResponsePayload::NicknameSet { device_key, value: normalized }),
    })?;
    broadcast_sensor_list_to_all_clients(&clients, &registry, &catalogue);
}
```

`broadcast_sensor_list_to_all_clients` is a helper that re-builds the v4-decorated SensorInfo list and sends a `SensorListBroadcast` to every connected client. It needs access to the per-client writer set; pass a `Arc<Mutex<Vec<Arc<Writer>>>>` or similar through the transport's accept loop.

- [ ] **Step 2: Add an in-process integration test.**

```rust
// apps/linsightd/src/transport/tests/set_nickname.rs (or a new file)
#[test]
fn set_nickname_round_trips_through_in_process_socket() {
    // Spawn a daemon on a tempfile UnixDatagram, connect two clients,
    // have one set a nickname, both receive SensorListBroadcast,
    // hardware.json contains the nickname on disk.
    todo!("write the test with two clients + tempdir XDG_CONFIG_HOME")
}
```

(This `todo!()` is the only allowed placeholder — the actual test should be filled in fully. Expand it before committing.)

- [ ] **Step 3: Run.**

```bash
cargo test -p linsightd set_nickname_round_trips
```
Expected: pass.

- [ ] **Step 4: Commit.**

```bash
git add apps/linsightd/src/transport/unix.rs apps/linsightd/src/transport/tests/
git commit -m "feat(daemon): SetNickname → persist + broadcast"
```

---

## Phase G — Prometheus changes

### Task G1: Add device_key label + linsight_hardware_info metric

**Files:**
- Modify: `apps/linsightd/src/prom.rs`

- [ ] **Step 1: Read the existing exporter to find the metric-emission loop.**

```bash
grep -n 'push_str\|metric\|gauge\|HELP\|TYPE' apps/linsightd/src/prom.rs | head -40
```

- [ ] **Step 2: Write the failing test.** Add to `prom.rs` tests:

```rust
    #[test]
    fn exporter_renders_device_key_label_when_set() {
        // Build a fake registry + one sample, call render(), assert output.
        let mut output = String::new();
        let registry = test_registry_with_xe_device();
        let samples = vec![Sample {
            sensor: SensorId::new("xe.gpu0.util"),
            ts_micros: 0,
            reading: Reading::Scalar(27.6),
        }];
        render(&mut output, &registry, &samples).unwrap();
        assert!(output.contains(r#"device_key="pci:0000:06:00.0""#));
        assert!(output.contains("27.6"));
        assert!(output.contains("linsight_hardware_info"));
        assert!(output.contains(r#"model="Intel Arc B-series""#));
    }

    #[test]
    fn exporter_escapes_special_chars_in_nickname() {
        let mut output = String::new();
        let mut registry = test_registry_with_xe_device();
        registry.set_nickname(
            &HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap(),
            Some("a\"b\\c\nd".into()),
        ).unwrap();
        render(&mut output, &registry, &[]).unwrap();
        // Escaped form per Prometheus exposition spec.
        assert!(output.contains(r#"nickname="a\"b\\c\nd""#));
    }
```

- [ ] **Step 3: Implement.** Add the escape helper + update the render path:

```rust
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r"\n"),
            _ => out.push(c),
        }
    }
    out
}

// In the per-sample emission, look up the device_key:
let device_label_attr = sensor_descriptor.device_key.as_ref()
    .map(|k| format!(r#"device_key="{}""#, escape_label(k.as_str())))
    .unwrap_or_default();
out.push_str(&format!("{metric}{{{device_label_attr}}} {value}\n"));

// After the per-sample emission, append the info metric ONCE per scrape:
out.push_str("# HELP linsight_hardware_info Static hardware metadata\n");
out.push_str("# TYPE linsight_hardware_info gauge\n");
for dev in registry.snapshot() {
    let nickname = registry.nicknames_snapshot()
        .get(&dev.key.to_string()).cloned().unwrap_or_default();
    out.push_str(&format!(
        r#"linsight_hardware_info{{device_key="{}",category="{}",model="{}",nickname="{}",plugin_id="{}"}} 1
"#,
        escape_label(dev.key.as_str()),
        dev.category.as_str(),
        escape_label(&dev.model),
        escape_label(&nickname),
        escape_label(&dev.plugin_id),
    ));
}
```

- [ ] **Step 4: Run.**

```bash
cargo test -p linsightd prom::tests
```
Expected: pass.

- [ ] **Step 5: Commit.**

```bash
git add apps/linsightd/src/prom.rs
git commit -m "feat(daemon/prom): add device_key label + linsight_hardware_info"
```

---

## Phase H — GUI client refactor

### Task H1: Reader thread becomes a dispatcher

**Files:**
- Modify: `apps/linsight-gui/src/client.rs`

- [ ] **Step 1: Read the current Client implementation.**

```bash
grep -n 'reader_thread\|sample_tx\|impl Client\|Sample' apps/linsight-gui/src/client.rs | head -30
```

- [ ] **Step 2: Refactor.** Introduce request correlation:

```rust
use std::sync::atomic::{AtomicU32, Ordering};

pub struct Client {
    // existing fields...
    sample_tx: crossbeam::channel::Sender<Sample>,
    // NEW:
    inflight: Arc<Mutex<HashMap<u32, mpsc::Sender<Result<ResponsePayload, ProtoError>>>>>,
    catalogue_tx: Arc<Mutex<Option<Vec<SensorInfo>>>>,
    // For now, the GUI polls `sensors()` after every broadcast. A watch channel
    // is cleaner but mpsc::Sender is good enough for v1.
    next_req_id: AtomicU32,
}

impl Client {
    fn run_reader_thread(...) {
        // existing handshake -> SensorList caching ...
        loop {
            match reader.recv()? {
                ServerMsg::Sample(s) => {
                    let _ = sample_tx.send(s);
                }
                ServerMsg::Response { req_id, result } => {
                    let mut guard = inflight.lock().unwrap();
                    if let Some(tx) = guard.remove(&req_id) {
                        let _ = tx.send(result);
                    } else {
                        tracing::warn!(req_id, "received response for unknown req_id");
                    }
                }
                ServerMsg::SensorListBroadcast(infos) => {
                    *catalogue_tx.lock().unwrap() = Some(infos);
                    // Notify the OverviewModel via the qt_thread queue;
                    // mechanism added in the next step.
                }
                ServerMsg::SensorDegraded { sensor, reason } => {
                    tracing::warn!(?sensor, %reason, "sensor degraded");
                }
                ServerMsg::Bye { reason } => {
                    tracing::info!(%reason, "daemon Bye");
                    break;
                }
                ServerMsg::Welcome { .. } | ServerMsg::SensorList(_) => {
                    // Should only appear at handshake. Re-ignore here.
                }
            }
        }
    }
}
```

- [ ] **Step 3: Add `get_hardware` and `set_nickname` RPC methods.**

```rust
impl Client {
    pub fn get_hardware(&self, timeout: Duration) -> Result<Vec<HardwareDevice>, RpcError> {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inflight.lock().unwrap().insert(req_id, tx);
        self.send(&ClientMsg::Request { req_id, op: RequestOp::GetHardware })?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(ResponsePayload::Hardware(devices))) => Ok(devices),
            Ok(Ok(other)) => Err(RpcError::UnexpectedPayload(format!("{:?}", other))),
            Ok(Err(e)) => Err(RpcError::Server(e.message)),
            Err(_) => Err(RpcError::Timeout),
        }
    }

    pub fn set_nickname(&self, key: &str, value: Option<String>, timeout: Duration)
        -> Result<(), RpcError>
    {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inflight.lock().unwrap().insert(req_id, tx);
        self.send(&ClientMsg::Request {
            req_id,
            op: RequestOp::SetNickname { device_key: key.to_owned(), value },
        })?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(ResponsePayload::NicknameSet { .. })) => Ok(()),
            Ok(Ok(other)) => Err(RpcError::UnexpectedPayload(format!("{:?}", other))),
            Ok(Err(e)) => Err(RpcError::Server(e.message)),
            Err(_) => Err(RpcError::Timeout),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("server: {0}")]
    Server(String),
    #[error("unexpected payload: {0}")]
    UnexpectedPayload(String),
    #[error("request timed out")]
    Timeout,
    #[error("send: {0}")]
    Send(String),
}
```

- [ ] **Step 4: Build + test.**

```bash
cargo build -p linsight
cargo test -p linsight
```
Expected: clean build, existing GUI tests pass.

- [ ] **Step 5: Commit.**

```bash
git add apps/linsight-gui/src/client.rs
git commit -m "refactor(gui/client): reader thread dispatches Response + Broadcast"
```

### Task H2: OverviewModel refreshes on broadcast

**Files:**
- Modify: `apps/linsight-gui/src/qobjects/overview_model.rs`

- [ ] **Step 1: Connect the broadcast hook.** When the reader thread receives `SensorListBroadcast`, it pushes new `SensorInfo` into the shared catalogue mutex. OverviewModel needs to re-evaluate its tile cache (the model already keys tiles by sensor ID; just rebuild the tile entries' `name` from the new `device_label`).

Add a method:

```rust
impl ffi::OverviewModel {
    /// Called from the qt_thread queue when the reader thread observes a
    /// SensorListBroadcast.
    pub fn on_catalogue_refreshed(mut self: Pin<&mut Self>, fresh: Vec<SensorInfo>) {
        let mut state = self.as_mut().rust_mut().sample_state.lock().unwrap();
        for info in fresh {
            if let Some(tile) = state.tiles.get_mut(&info.id.as_str().to_string()) {
                let new_name = match (&info.display_name, &info.device_label) {
                    (m, Some(d)) => format!("{m} · {d}"),
                    (m, None) => m.clone(),
                };
                tile.name = new_name;
            }
        }
        let json = serialize_tiles(&state.id_order, &state.tiles);
        drop(state);
        self.as_mut().set_tiles_json(QString::from(json.as_str()));
    }
}
```

(Adapt the data-flow plumbing to match the existing model: this code assumes a `sample_state` mutex; if the project uses a different shape, mirror it.)

In the reader-thread arm:

```rust
ServerMsg::SensorListBroadcast(infos) => {
    let _ = qt_thread.queue(move |mut pin| {
        pin.as_mut().on_catalogue_refreshed(infos);
    });
}
```

- [ ] **Step 2: Build + smoke.**

```bash
cargo build -p linsight
./scripts/dev_screenshot.sh overview /tmp/shot.png
```
Expected: clean build. Screenshot still works (no broadcast yet without a nickname set, so behavior should be unchanged from baseline).

- [ ] **Step 3: Commit.**

```bash
git add apps/linsight-gui/src/qobjects/overview_model.rs apps/linsight-gui/src/client.rs
git commit -m "feat(gui/overview): refresh tile labels on SensorListBroadcast"
```

---

## Phase I — GUI Hardware page

### Task I1: HardwareModel qobject

**Files:**
- Create: `apps/linsight-gui/src/qobjects/hardware_model.rs`
- Modify: `apps/linsight-gui/src/qobjects/mod.rs`

- [ ] **Step 1: Implement the qobject** mirroring `PreferencesModel`'s shape (CXX bridge + Rust impl + qinvokables):

```rust
// hardware_model.rs
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::pin::Pin;
use std::time::Duration;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;

use crate::client::Client;
use crate::workspace_handle::with_workspace;

#[cxx_qt::bridge]
mod ffi {
    extern "RustQt" {
        #[qobject]
        #[qproperty(QString, devices_json)]
        #[qproperty(bool, is_loading)]
        #[qproperty(QString, last_error)]
        type HardwareModel = super::HardwareModelRust;

        #[qinvokable]
        fn reload(self: Pin<&mut HardwareModel>);

        #[qinvokable]
        fn apply_nickname(self: Pin<&mut HardwareModel>, key: &QString, value: &QString);
    }
}

pub struct HardwareModelRust {
    devices_json: QString,
    is_loading: bool,
    last_error: QString,
}

impl Default for HardwareModelRust {
    fn default() -> Self {
        Self {
            devices_json: QString::from("[]"),
            is_loading: false,
            last_error: QString::from(""),
        }
    }
}

impl ffi::HardwareModel {
    pub fn reload(mut self: Pin<&mut Self>) {
        self.as_mut().set_is_loading(true);
        self.as_mut().set_last_error(QString::from(""));
        let client = with_workspace(|w| w.client());
        match client.get_hardware(Duration::from_secs(5)) {
            Ok(devices) => {
                let json = serde_json::to_string(&devices).unwrap_or_else(|_| "[]".into());
                self.as_mut().set_devices_json(QString::from(json.as_str()));
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
        self.as_mut().set_is_loading(false);
    }

    pub fn apply_nickname(mut self: Pin<&mut Self>, key: &QString, value: &QString) {
        let key_s = key.to_string();
        let value_s = value.to_string();
        let value_opt = if value_s.trim().is_empty() { None } else { Some(value_s) };
        let client = with_workspace(|w| w.client());
        match client.set_nickname(&key_s, value_opt, Duration::from_secs(5)) {
            Ok(()) => {
                self.as_mut().set_last_error(QString::from(""));
                // Broadcast from daemon will trigger reload() via OverviewModel
                // observer; we can also reload eagerly for snappy feedback:
                self.as_mut().reload();
            }
            Err(e) => {
                self.as_mut().set_last_error(QString::from(format!("{e}").as_str()));
            }
        }
    }
}
```

- [ ] **Step 2: Register the qobject** in `mod.rs`:

```rust
pub mod dashboards_model;
pub mod hardware_model;  // NEW
pub mod overview_model;
pub mod preferences_model;
pub mod workspace_handle;
```

- [ ] **Step 3: Wire `app.hardware` injection** in `apps/linsight-gui/src/main.rs` next to `app.preferences`. Use the same QML context-property pattern.

- [ ] **Step 4: Build.**

```bash
cargo build -p linsight
```
Expected: clean build.

- [ ] **Step 5: Commit.**

```bash
git add apps/linsight-gui/src/{qobjects/hardware_model.rs,qobjects/mod.rs,main.rs}
git commit -m "feat(gui): HardwareModel qobject"
```

### Task I2: HardwarePage.qml

**Files:**
- Create: `apps/linsight-gui/qml/HardwarePage.qml`
- Modify: `apps/linsight-gui/qml/Main.qml`
- Modify: `apps/linsight-gui/qml/StartPagePicker.qml`
- Modify: `Justfile` (add HardwarePage.qml to i18n-extract list)

- [ ] **Step 1: Create the QML page.** ~130 lines, similar shape to `CategoryPage.qml` but with editable rows:

```qml
// HardwarePage.qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    title: qsTr("Hardware")
    padding: 0

    Accessible.role: Accessible.Pane
    Accessible.name: qsTr("Hardware")

    Component.onCompleted: app.hardware.reload()

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    readonly property var devices: {
        try { return JSON.parse(app.hardware.devices_json || "[]") }
        catch (e) { return [] }
    }

    Controls.ScrollView {
        anchors.fill: parent
        anchors.leftMargin: app.tokens.spaceXL
        anchors.rightMargin: app.tokens.spaceXL
        anchors.topMargin: app.tokens.spaceL
        anchors.bottomMargin: app.tokens.spaceL
        clip: true
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: app.tokens.spaceM

            Repeater {
                model: page.devices
                delegate: DeviceCard {
                    Layout.fillWidth: true
                    deviceJson: modelData
                }
            }

            Controls.Label {
                visible: page.devices.length === 0
                text: app.hardware.is_loading
                    ? qsTr("Detecting hardware…")
                    : qsTr("No hardware detected")
                opacity: 0.6
            }
        }
    }

    component DeviceCard: Rectangle {
        property var deviceJson
        height: cardContent.implicitHeight + 24
        color: app.tokens.surface1
        radius: 6

        ColumnLayout {
            id: cardContent
            anchors.fill: parent
            anchors.margins: 12
            spacing: 6

            Controls.Label {
                text: deviceJson.model || qsTr("Unknown device")
                font.pixelSize: app.tokens.textHeading
                font.weight: app.tokens.weightBold
            }
            Controls.Label {
                text: deviceJson.key + " · " + (deviceJson.sensor_ids?.length || 0) + " " + qsTr("sensors")
                opacity: 0.55
                font.pixelSize: app.tokens.textCaption
            }
            RowLayout {
                Layout.topMargin: 6
                Controls.Label { text: qsTr("Nickname:"); opacity: 0.7 }
                Controls.TextField {
                    id: nicknameField
                    Layout.fillWidth: true
                    text: ""
                    placeholderText: qsTr("(none)")
                    maximumLength: 64
                    onEditingFinished: app.hardware.apply_nickname(deviceJson.key, text)
                }
            }
            Controls.Label {
                visible: app.hardware.last_error.length > 0
                text: app.hardware.last_error
                color: "#ff8080"
                font.pixelSize: app.tokens.textCaption
            }
        }
    }
}
```

- [ ] **Step 2: Add the page to Main.qml.** Insert into known pages, sidebar nav, page-stack switcher, and Ctrl+5 shortcut (existing Editor moves to Ctrl+6):

```qml
// In `app.knownPages` array:
const known = ["overview","gpus","storage","network","hardware","editor","settings","about","licenses","credits"]

// In sidebar entries:
Kirigami.Action {
    text: qsTr("Hardware")
    icon.name: "preferences-other"
    onTriggered: app.goTo("hardware")
}

// In the page-switcher case:
case "hardware":  app.pageStack.replace(hardwarePage); break

// In the Component declarations:
Component { id: hardwarePage; HardwarePage {} }

// Add the shortcut:
Shortcut { sequence: "Ctrl+5"; context: Qt.ApplicationShortcut; onActivated: app.goTo("hardware") }
// And update Editor to Ctrl+6
```

- [ ] **Step 3: Add to start-page picker.** Edit `StartPagePicker.qml`:

```qml
{ key: "hardware", label: qsTr("Hardware") },
```

- [ ] **Step 4: Add to Justfile i18n-extract.**

```bash
sed -i '/qml\/SettingsPage.qml/i\        qml/HardwarePage.qml \\\\' Justfile
```

Verify:

```bash
grep HardwarePage Justfile
```

- [ ] **Step 5: Build + screenshot.**

```bash
cargo build -p linsight
./scripts/dev_screenshot.sh hardware /tmp/hw.png
ls -la /tmp/hw.png
```
Expected: PNG created, daemon shows expected devices in the page.

- [ ] **Step 6: Commit.**

```bash
git add apps/linsight-gui/qml/{HardwarePage.qml,Main.qml,StartPagePicker.qml} Justfile
git commit -m "feat(gui): add Hardware page with inline nickname editing"
```

---

## Phase J — Verification & ship

### Task J1: just ci + manual screenshot

**Files:**
- None.

- [ ] **Step 1: Run the full CI gate.**

```bash
just ci
```
Expected: ALL pass; sum of passing tests >= 175.

- [ ] **Step 2: Capture a screenshot of the new Hardware page.**

```bash
pkill -x linsightd 2>/dev/null; pkill -x linsight 2>/dev/null; sleep 1
./scripts/dev_screenshot.sh hardware /tmp/hw-final.png
```

- [ ] **Step 3: Read /tmp/hw-final.png** to visually confirm the user's two Intel GPUs + 5080 + NVMe drives + CPU all appear with sensible model strings.

- [ ] **Step 4: Set a nickname interactively** via `linsight-cli` (if a future CLI subcommand is added — for now do via daemon-protocol-level test or directly editing `~/.config/linsight/hardware.json` and bouncing the daemon):

```bash
cat > ~/.config/linsight/hardware.json <<'EOF'
{
  "schema_version": 1,
  "nicknames": {
    "pci:0000:06:00.0": "Battlemage"
  }
}
EOF
pkill -x linsightd; sleep 1
LINSIGHT_LOG=info nohup ./target/debug/linsightd > /tmp/linsightd.log 2>&1 &
disown
sleep 2
./target/debug/linsight-cli list | grep -i battlemage
```
Expected: at least one row labeled "... · Battlemage".

### Task J2: CHANGELOG + README

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md`

- [ ] **Step 1: Add a CHANGELOG entry** at the top:

```markdown
## v0.5.0 — Hardware page + per-device nicknames

- **New Hardware page** (Ctrl+5) lists every detected GPU / NVMe /
  NIC / CPU with vendor-resolved model strings ("Intel Arc B-
  series", "NVIDIA RTX 5080 Mobile") and inline nickname editing.
- **Nicknames propagate everywhere** — GUI tile labels become
  `<metric> · <device label>`; CLI `linsight-cli list` shows the
  nickname column; Prometheus exporter adds a stable `device_key`
  label and a new `linsight_hardware_info` gauge for joins.
- **Plugin SDK ABI bumps v3 → v4.** `PluginManifest` gains a
  `devices` list; `SensorDescriptor` gains a `device_key`
  back-reference. v3 plugins fail symbol lookup at load (see
  ADR-0002). No third-party `.so` plugins existed yet, so this is
  a clean break.
- **Protocol bumps v1 → v2.** `SensorInfo` gains `device_key` +
  `device_label`; new `Request`/`Response` with `req_id`
  correlation; new `SensorListBroadcast` for nickname refresh.
- **Nickname store** at `~/.config/linsight/hardware.json` (atomic
  tmp+rename, schema-versioned).
- Tests: 147 → 175+.
```

- [ ] **Step 2: Update README.md status line.** Bump version cite, add Hardware to the feature list.

- [ ] **Step 3: Commit.**

```bash
git add CHANGELOG.md README.md
git commit -m "docs: changelog entry for v0.5.0 hardware page"
```

### Task J3: Update CLAUDE.md / AGENTS.md test baseline

**Files:**
- Modify: `CLAUDE.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: Bump the test baseline number in both files.**

```bash
grep -n '147 passing' CLAUDE.md AGENTS.md
```
Replace `147` with the actual final count from `just ci`.

- [ ] **Step 2: Commit.**

```bash
git add CLAUDE.md AGENTS.md
git commit -m "docs: update test baseline after hardware page sprint"
```

---

## Self-review checklist

- [ ] Every spec section above maps to at least one task here:
  - HardwareDeviceKey, HardwareCategory, HardwareDevice, validate_nickname → Phase A.
  - SDK v4 + R-mirror types + host_init validation → Phase B.
  - pci.ids helper → Phase C.
  - In-tree plugin updates → Phase D.
  - Protocol v2 → Phase E.
  - Daemon registry + nickname store + dispatcher + broadcast → Phase F.
  - Prometheus changes → Phase G.
  - GUI client dispatcher refactor → Phase H.
  - GUI Hardware page → Phase I.
  - just ci, screenshot, docs → Phase J.
- [ ] No "TBD" or "TODO" placeholders. (One `todo!()` in F6 step 2 is explicitly marked and must be expanded before that task commits; one `// TODO Phase D` comment in B3 step 5 is also explicit and removed in Phase D.)
- [ ] Type names are consistent across tasks: `HardwareDeviceKey`, `HardwareDevice`, `HardwareCategory`, `RHardwareDevice`, `RHardwareCategoryKind`, `NicknameError`, `RegistryError`, `RpcError`, `ResponsePayload`, `RequestOp`, `ProtoError`, `ProtoErrorCode`.
- [ ] Frequent commits — every task ends with a commit.
- [ ] TDD: failing test first, then implementation, then verify.
