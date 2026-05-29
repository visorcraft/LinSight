// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `HardwareRegistry` — the daemon's authoritative in-memory device list.
//!
//! Built once at startup from every plugin's v4 manifest, then mutated by
//! `SetNickname` RPCs through the lifetime of the daemon. The registry does
//! NOT probe hardware itself; it is a passive collector that the
//! [`crate::plugin_host::PluginHost`] hands a tuple of
//! `(plugin_id, devices, sensor_descriptors)` per loaded plugin.
//!
//! Three indices are maintained side-by-side:
//!
//!   * `devices` — canonical map `device_key -> HardwareDevice`
//!   * `by_plugin` — fast lookup `(plugin_id, plugin_device_id) -> key`
//!     used by the transport layer to decorate outgoing `SensorInfo` rows.
//!   * `nicknames` — overlay applied on top of `model` when computing
//!     a `device_label`. Persisted by the caller via
//!     [`crate::nickname_store::NicknameStore`].

use std::collections::HashMap;

use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey};
use linsight_plugin_sdk::SensorDescriptor;

/// Display order for `HardwareCategory` in `snapshot()`. Lower rank
/// sorts first. Cpu leads because there's exactly one and it makes a
/// clean header; Other trails because by definition we don't know
/// where to put it.
fn category_rank(c: HardwareCategory) -> u8 {
    match c {
        HardwareCategory::Cpu => 0,
        HardwareCategory::Gpu => 1,
        HardwareCategory::Storage => 2,
        HardwareCategory::Network => 3,
        HardwareCategory::Other => 4,
    }
}

use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error, PartialEq)]
pub enum RegistryError {
    #[error("unknown hardware device: {0}")]
    UnknownDevice(String),
    #[error("invalid nickname: {0}")]
    InvalidNickname(#[from] linsight_core::NicknameError),
}

pub struct HardwareRegistry {
    pub devices: HashMap<HardwareDeviceKey, HardwareDevice>,
    /// `(plugin_id, plugin_device_id) -> device_key`. The pair is needed
    /// because a `plugin_device_id` like `"gpu0"` is only unique within
    /// the plugin that produced it.
    by_plugin: HashMap<(String, String), HardwareDeviceKey>,
    nicknames: HashMap<HardwareDeviceKey, String>,
}

impl HardwareRegistry {
    /// Build the registry from a slice of `(plugin_id, devices, sensors)`
    /// tuples and a pre-loaded nickname map (raw string keys, as they
    /// come off disk).
    ///
    /// Behavior:
    ///   * The `plugin_id` field on each `HardwareDevice` is FILLED IN
    ///     here from the tuple — plugins leave that field empty and the
    ///     host is the authoritative source.
    ///   * Sensor descriptors are walked once to populate the
    ///     `HardwareDevice::sensor_ids` cross-reference; a sensor
    ///     pointing at a key whose device isn't declared on the same
    ///     plugin is dropped with a warning (host_init's referential
    ///     check should have caught it earlier; we keep the warn for
    ///     defense in depth).
    ///   * Duplicate device keys across plugins log WARN and keep the
    ///     first registration.
    ///   * Nicknames whose key doesn't parse as a `HardwareDeviceKey`
    ///     are dropped with a warning (file may have been hand-edited
    ///     with stale entries from a removed plugin).
    pub fn build(
        manifests: &[(&str, &[HardwareDevice], &[SensorDescriptor])],
        nicknames: HashMap<String, String>,
    ) -> Self {
        let mut devices: HashMap<HardwareDeviceKey, HardwareDevice> = HashMap::new();
        let mut by_plugin: HashMap<(String, String), HardwareDeviceKey> = HashMap::new();

        for (plugin_id, plugin_devices, sensors) in manifests {
            for dev in *plugin_devices {
                if devices.contains_key(&dev.key) {
                    warn!(
                        key = %dev.key,
                        plugin = plugin_id,
                        "duplicate hardware device key across plugins; keeping first",
                    );
                    continue;
                }
                // Plugins leave `plugin_id` empty (they don't know it
                // until host_init binds them); the host fills it here.
                let mut device = dev.clone();
                device.plugin_id = (*plugin_id).to_owned();
                // Wipe sensor_ids — we'll repopulate from the manifest's
                // sensor descriptors below so the cross-reference is
                // authoritative regardless of what the plugin emitted.
                device.sensor_ids.clear();
                by_plugin.insert(
                    ((*plugin_id).to_owned(), device.plugin_device_id.clone()),
                    device.key.clone(),
                );
                devices.insert(device.key.clone(), device);
            }

            for sensor in *sensors {
                let Some(key) = sensor.device_key.as_ref() else { continue };
                let Some(dev) = devices.get_mut(key) else {
                    warn!(
                        sensor = %sensor.id,
                        key = %key,
                        plugin = plugin_id,
                        "sensor references unknown device key; ignoring (host_init should have rejected)",
                    );
                    continue;
                };
                dev.sensor_ids.push(sensor.id.clone());
            }
        }

        let mut typed_nicknames: HashMap<HardwareDeviceKey, String> = HashMap::new();
        for (raw_key, value) in nicknames {
            match HardwareDeviceKey::try_new(raw_key.clone()) {
                Ok(k) => {
                    typed_nicknames.insert(k, value);
                }
                Err(e) => {
                    warn!(
                        key = %raw_key,
                        error = %e,
                        "nickname references invalid device key; dropping",
                    );
                }
            }
        }

        Self { devices, by_plugin, nicknames: typed_nicknames }
    }

    /// Display label for `key`. Delegates to
    /// [`linsight_core::compute_device_label`] so the daemon and the
    /// GUI (HardwareModel) always agree on what a device is named —
    /// the GUI's Hardware-page title and the SensorInfo.device_label
    /// stamped onto tiles use the same algorithm.
    pub fn device_label_for(&self, key: &HardwareDeviceKey) -> String {
        let Some(dev) = self.devices.get(key) else {
            return key.as_str().to_owned();
        };
        let all_devices: Vec<HardwareDevice> = self.devices.values().cloned().collect();
        let nicks_str: HashMap<String, String> =
            self.nicknames.iter().map(|(k, v)| (k.as_str().to_owned(), v.clone())).collect();
        linsight_core::compute_device_label(dev, &all_devices, &nicks_str)
    }

    /// Resolve a `(plugin_id, plugin_device_id)` pair from a sensor
    /// descriptor to the canonical device key. Returns `None` for
    /// sensors that don't bind to a device (memory has no per-DIMM
    /// identity; CPU pins to a single shared device_key).
    pub fn key_for(&self, plugin_id: &str, plugin_device_id: &str) -> Option<&HardwareDeviceKey> {
        self.by_plugin.get(&(plugin_id.to_owned(), plugin_device_id.to_owned()))
    }

    /// Set, update, or clear a nickname.
    ///
    ///   * `Some(s)` — store `s` as the nickname (caller must have
    ///     already pushed it through `linsight_core::validate_nickname`;
    ///     we re-validate here to defend against direct callers that
    ///     skip it).
    ///   * `None`    — remove the entry (this is the "user cleared the
    ///     field" outcome from `validate_nickname`).
    ///
    /// Returns `RegistryError::UnknownDevice` if `key` is not in the
    /// registry — the transport layer maps this to
    /// `ProtoErrorCode::UnknownDevice` on the wire.
    pub fn set_nickname(
        &mut self,
        key: &HardwareDeviceKey,
        value: Option<String>,
    ) -> Result<(), RegistryError> {
        if !self.devices.contains_key(key) {
            return Err(RegistryError::UnknownDevice(key.as_str().to_owned()));
        }
        match value {
            Some(raw) => {
                // Re-validate; we trust callers but defense in depth is
                // cheap here and the only sane place to enforce the
                // invariant is at the registry boundary.
                let normalized = linsight_core::validate_nickname(&raw)?;
                match normalized {
                    Some(n) => {
                        self.nicknames.insert(key.clone(), n);
                    }
                    None => {
                        // "Some(empty string)" collapses to a clear via
                        // the same path as `None` would have taken.
                        self.nicknames.remove(key);
                    }
                }
            }
            None => {
                self.nicknames.remove(key);
            }
        }
        Ok(())
    }

    /// Sorted snapshot of every device, ready to ship over the wire as
    /// the body of a `ResponsePayload::Hardware`. Primary sort key is
    /// `HardwareCategory` (Cpu, Gpu, Storage, Network, Other — the
    /// declaration order, which we pin via `category_rank`); ties break
    /// by key string. This groups by category in the Hardware page
    /// (GPUs together regardless of `pci:` vs `nvml:` scheme) while
    /// staying stable across daemon restarts.
    pub fn snapshot(&self) -> Vec<HardwareDevice> {
        let mut out: Vec<HardwareDevice> = self.devices.values().cloned().collect();
        out.sort_by(|a, b| {
            category_rank(a.category)
                .cmp(&category_rank(b.category))
                .then_with(|| a.key.as_str().cmp(b.key.as_str()))
        });
        out
    }

    /// Flat `HashMap<String, String>` view of the nickname overlay, in
    /// the same shape `NicknameStore` serializes to disk. Used by the
    /// transport layer's SetNickname handler to persist after a mutation.
    pub fn nicknames_snapshot(&self) -> HashMap<String, String> {
        self.nicknames.iter().map(|(k, v)| (k.as_str().to_owned(), v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use linsight_core::{Category, HardwareCategory, SensorId, SensorKind, Unit};

    use super::*;

    fn dev(key: &str, model: &str, plugin_device_id: &str) -> HardwareDevice {
        HardwareDevice {
            key: HardwareDeviceKey::try_new(key).unwrap(),
            category: HardwareCategory::Gpu,
            model: model.into(),
            vendor: None,
            location: None,
            // Plugins leave this empty; the host fills it in build().
            plugin_id: String::new(),
            plugin_device_id: plugin_device_id.into(),
            sensor_ids: vec![],
        }
    }

    fn sensor(id: &str, device_key: Option<&str>) -> SensorDescriptor {
        SensorDescriptor {
            id: SensorId::new(id),
            display_name: id.into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Gpu,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: None,
            device_key: device_key.map(|k| HardwareDeviceKey::try_new(k).unwrap()),
            tags: vec![],
        }
    }

    #[test]
    fn build_fills_in_plugin_id_and_collects_sensors() {
        let d = [dev("pci:0000:06:00.0", "Arc B-series", "gpu0")];
        let s = [sensor("xe.gpu0.util", Some("pci:0000:06:00.0"))];
        let reg = HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &s)], HashMap::new());
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        let device = reg.devices.get(&key).unwrap();
        assert_eq!(device.plugin_id, "io.visorcraft.linsight.xe");
        assert_eq!(device.sensor_ids.len(), 1);
        assert_eq!(device.sensor_ids[0].as_str(), "xe.gpu0.util");
    }

    #[test]
    fn device_label_falls_back_to_model_when_no_nickname() {
        let d = [dev("pci:0000:06:00.0", "Arc B-series", "gpu0")];
        let reg =
            HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &[])], HashMap::new());
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        assert_eq!(reg.device_label_for(&key), "Arc B-series");
    }

    #[test]
    fn nickname_overrides_model_in_label() {
        let d = [dev("pci:0000:06:00.0", "Arc B-series", "gpu0")];
        let mut nicks = HashMap::new();
        nicks.insert("pci:0000:06:00.0".into(), "Battlemage".into());
        let reg = HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &[])], nicks);
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        assert_eq!(reg.device_label_for(&key), "Battlemage");
    }

    #[test]
    fn duplicate_device_key_across_plugins_keeps_first() {
        let d1 = [dev("pci:0000:06:00.0", "First", "gpu0")];
        let d2 = [dev("pci:0000:06:00.0", "Second", "gpu0")];
        let reg = HardwareRegistry::build(
            &[("first.plugin", &d1, &[]), ("second.plugin", &d2, &[])],
            HashMap::new(),
        );
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        let device = reg.devices.get(&key).unwrap();
        assert_eq!(device.model, "First", "first-registration-wins on key conflict");
        assert_eq!(device.plugin_id, "first.plugin");
    }

    #[test]
    fn set_nickname_then_label_reflects_value() {
        let d = [dev("pci:0000:06:00.0", "Arc B-series", "gpu0")];
        let mut reg =
            HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &[])], HashMap::new());
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        reg.set_nickname(&key, Some("Battlemage".into())).unwrap();
        assert_eq!(reg.device_label_for(&key), "Battlemage");

        // Clear-by-None drops the nickname and falls back to model.
        reg.set_nickname(&key, None).unwrap();
        assert_eq!(reg.device_label_for(&key), "Arc B-series");

        // Clear-by-empty-string also drops.
        reg.set_nickname(&key, Some("Battlemage".into())).unwrap();
        reg.set_nickname(&key, Some("   ".into())).unwrap();
        assert_eq!(reg.device_label_for(&key), "Arc B-series");
    }

    #[test]
    fn set_nickname_rejects_unknown_device() {
        let mut reg = HardwareRegistry::build(&[], HashMap::new());
        let key = HardwareDeviceKey::try_new("pci:0000:99:00.0").unwrap();
        let err = reg.set_nickname(&key, Some("ghost".into())).unwrap_err();
        match err {
            RegistryError::UnknownDevice(k) => assert_eq!(k, "pci:0000:99:00.0"),
            other => panic!("expected UnknownDevice, got {other:?}"),
        }
    }

    #[test]
    fn set_nickname_then_save_then_reload_persists() {
        // F6 round-trip guard: a SetNickname through the registry +
        // NicknameStore::save → ::load cycle must restore the
        // nickname. We exercise the same call sequence the SetNickname
        // RPC handler in transport/unix.rs uses, minus the socket layer,
        // so this stays a fast unit test.
        use crate::nickname_store::NicknameStore;

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("hardware.json");

        let d = [dev("pci:0000:06:00.0", "Arc B-series", "gpu0")];
        let mut reg =
            HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &[])], HashMap::new());
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        reg.set_nickname(&key, Some("Battlemage".into())).unwrap();

        let store = NicknameStore { schema_version: 1, nicknames: reg.nicknames_snapshot() };
        store.save(&path).unwrap();

        let reloaded = NicknameStore::load(&path);
        assert_eq!(
            reloaded.nicknames.get("pci:0000:06:00.0").map(String::as_str),
            Some("Battlemage"),
        );

        // And a fresh registry rebuilt from the reloaded map should
        // surface the nickname through `device_label_for`.
        let reg2 =
            HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &[])], reloaded.nicknames);
        assert_eq!(reg2.device_label_for(&key), "Battlemage");
    }

    #[test]
    fn duplicate_model_no_nicknames_gets_disambig_suffix() {
        // Two NVMe drives that report the same model string need a
        // suffix so the Hardware page can tell them apart at a glance.
        let d = [
            HardwareDevice {
                key: HardwareDeviceKey::try_new("nvme:eui.001").unwrap(),
                category: HardwareCategory::Storage,
                model: "Samsung SSD 990 PRO 2TB".into(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: "nvme0".into(),
                sensor_ids: vec![],
            },
            HardwareDevice {
                key: HardwareDeviceKey::try_new("nvme:eui.002").unwrap(),
                category: HardwareCategory::Storage,
                model: "Samsung SSD 990 PRO 2TB".into(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: "nvme1".into(),
                sensor_ids: vec![],
            },
        ];
        let reg =
            HardwareRegistry::build(&[("io.visorcraft.linsight.nvme", &d, &[])], HashMap::new());

        let k0 = HardwareDeviceKey::try_new("nvme:eui.001").unwrap();
        let k1 = HardwareDeviceKey::try_new("nvme:eui.002").unwrap();
        assert_eq!(reg.device_label_for(&k0), "Samsung SSD 990 PRO 2TB (eui.001)");
        assert_eq!(reg.device_label_for(&k1), "Samsung SSD 990 PRO 2TB (eui.002)");
    }

    #[test]
    fn nickname_on_one_dup_disambiguates_the_pair() {
        // If one of two duplicate-model devices has a nickname, that one
        // renders as the nickname; the OTHER renders as bare model (no
        // suffix needed — the user can already tell them apart by name).
        let d = [
            HardwareDevice {
                key: HardwareDeviceKey::try_new("nvme:eui.001").unwrap(),
                category: HardwareCategory::Storage,
                model: "Samsung SSD 990 PRO 2TB".into(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: "nvme0".into(),
                sensor_ids: vec![],
            },
            HardwareDevice {
                key: HardwareDeviceKey::try_new("nvme:eui.002").unwrap(),
                category: HardwareCategory::Storage,
                model: "Samsung SSD 990 PRO 2TB".into(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: "nvme1".into(),
                sensor_ids: vec![],
            },
        ];
        let mut nicks = HashMap::new();
        nicks.insert("nvme:eui.001".into(), "OS drive".into());
        let reg = HardwareRegistry::build(&[("io.visorcraft.linsight.nvme", &d, &[])], nicks);

        let k0 = HardwareDeviceKey::try_new("nvme:eui.001").unwrap();
        let k1 = HardwareDeviceKey::try_new("nvme:eui.002").unwrap();
        assert_eq!(reg.device_label_for(&k0), "OS drive");
        assert_eq!(reg.device_label_for(&k1), "Samsung SSD 990 PRO 2TB");
    }

    #[test]
    fn unique_models_get_no_suffix() {
        let d = [
            dev("nvme:eui.001", "Samsung SSD 990 PRO 2TB", "nvme0"),
            dev("nvme:eui.002", "WD_BLACK SN850X 1TB", "nvme1"),
        ];
        let reg =
            HardwareRegistry::build(&[("io.visorcraft.linsight.nvme", &d, &[])], HashMap::new());
        let k = HardwareDeviceKey::try_new("nvme:eui.001").unwrap();
        assert_eq!(reg.device_label_for(&k), "Samsung SSD 990 PRO 2TB");
    }
}
