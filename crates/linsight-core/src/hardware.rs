// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::SensorId;

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

#[derive(Debug, Error, PartialEq)]
pub enum KeyError {
    #[error("hardware device key is empty")]
    Empty,
    #[error("hardware device key too long ({0} bytes, max 140)")]
    TooLong(usize),
    #[error(
        "hardware device key missing scheme prefix (expected one of: pci, nvml, nvme, net, cpu, plugin)"
    )]
    NoScheme,
    #[error("hardware device key has unknown scheme '{0}'")]
    UnknownScheme(String),
    #[error("hardware device key payload empty after '{0}:'")]
    EmptyPayload(String),
    #[error("hardware device key contains invalid character {0:?}")]
    BadChar(char),
}

const ALLOWED_SCHEMES: &[&str] = &[
    "pci", "nvml", "nvme", "net", "cpu", "system", "block", "hwmon", "fs", "amdgpu", "zram",
    "i915", "plugin",
];
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
            let ok = c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-');
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HardwareDevice {
    pub key: HardwareDeviceKey,
    pub category: HardwareCategory,
    pub model: String,
    pub vendor: Option<String>,
    pub location: Option<String>,
    pub plugin_id: String,
    pub plugin_device_id: String,
    pub sensor_ids: Vec<SensorId>,
}

#[derive(Debug, Error, PartialEq)]
pub enum NicknameError {
    #[error("nickname too long ({0} chars, max 64)")]
    TooLong(usize),
    #[error("nickname contains control char {0:?}")]
    ControlChar(char),
}

pub const NICKNAME_MAX_CHARS: usize = 64;

/// Validate and normalize a user-supplied nickname.
/// * `Ok(None)` if empty after trimming (delete intent),
/// * `Ok(Some(s))` with the trimmed value if valid,
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

/// Parse a sysfs PCI ID file (`/sys/.../device/vendor` or `device/device`).
/// These files contain a single line like `0x8086\n`. Returns `None` on
/// any read or parse failure — non-PCI interfaces (`lo`, `veth*`, bond
/// masters, WireGuard) have no such file and that's not an error.
///
/// Lives here so the xe and net plugins (and any future PCI-backed
/// hardware plugin) read PCI IDs through one helper instead of three
/// near-duplicate inline parses.
pub fn parse_sysfs_pci_id(path: &std::path::Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    u16::from_str_radix(trimmed, 16).ok()
}

/// Compute the display label for a single device given the full
/// device list + nickname map. The rule:
///
///   1. If a nickname is set for this key, the label IS the nickname.
///   2. Otherwise, if another un-nicknamed device shares this model
///      string, append a location-derived suffix ("(payload after the
///      key's scheme prefix)") so the user can tell them apart.
///   3. Otherwise, the bare `model` string.
///
/// Both the daemon (`SensorInfo.device_label` decoration) and the GUI
/// (`HardwareModel`'s page-title rendering) call this so they can
/// never disagree on what a device is named.
pub fn compute_device_label(
    target: &HardwareDevice,
    all_devices: &[HardwareDevice],
    nicknames: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(nick) = nicknames.get(target.key.as_str()) {
        return nick.clone();
    }
    let needs_disambig = all_devices.iter().any(|d| {
        d.key != target.key && d.model == target.model && !nicknames.contains_key(d.key.as_str())
    });
    if needs_disambig {
        let suffix =
            target.key.as_str().split_once(':').map(|(_, p)| p).unwrap_or(target.key.as_str());
        format!("{} ({})", target.model, suffix)
    } else {
        target.model.clone()
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
            "plugin:io.visorcraft.linsight.echo:demo",
        ] {
            assert!(HardwareDeviceKey::try_new(s).is_ok(), "should accept: {s}");
        }
    }

    #[test]
    fn key_rejects_invalid_forms() {
        let long = "x".repeat(200);
        for s in [
            "",
            "pci",
            "pci:",
            "FOO:bar",
            "pci:0000:06:00.0 ",
            "PCI:0000:06:00.0",
            "pci:0000:06:00.0/extra",
            long.as_str(),
        ] {
            assert!(HardwareDeviceKey::try_new(s).is_err(), "should reject: {s}");
        }
    }

    #[test]
    fn key_accepts_uppercase_in_payload() {
        // Real-world keys from sysfs and tmpfs frequently contain uppercase
        // letters (hwmon names like BAT0, ACAD; mount path tokens like
        // S3Drive...). The validator must accept them in the payload while
        // continuing to require a lowercase scheme prefix (enforced by the
        // ALLOWED_SCHEMES table).
        for s in
            ["hwmon:BAT0", "hwmon:ACAD", "fs:tmp_.mount_S3DrivweY1lv", "nvml:uuid:GPU-abc123-456"]
        {
            assert!(HardwareDeviceKey::try_new(s).is_ok(), "should accept: {s}");
        }
    }

    #[test]
    fn key_scheme_extraction() {
        let k = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        assert_eq!(k.scheme(), "pci");
        let k = HardwareDeviceKey::try_new("nvml:uuid:gpu-abc").unwrap();
        assert_eq!(k.scheme(), "nvml");
    }

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
        assert_eq!(validate_nickname("\t\t"), Ok(None));
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

    #[test]
    fn device_serde_round_trip() {
        let dev = HardwareDevice {
            key: HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap(),
            category: HardwareCategory::Gpu,
            model: "Intel Arc B-series".into(),
            vendor: Some("Intel Corporation".into()),
            location: Some("PCI 0000:06:00.0".into()),
            plugin_id: "io.visorcraft.linsight.xe".into(),
            plugin_device_id: "gpu0".into(),
            sensor_ids: vec![SensorId::new("xe.gpu0.util")],
        };
        let s = serde_json::to_string(&dev).unwrap();
        let back: HardwareDevice = serde_json::from_str(&s).unwrap();
        assert_eq!(back, dev);
    }
}
