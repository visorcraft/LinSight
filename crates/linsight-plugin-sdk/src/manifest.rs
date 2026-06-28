// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use linsight_core::{Category, HardwareDevice, HardwareDeviceKey, SensorId, SensorKind, Unit};
use stabby::option::Option as SOption;
use stabby::string::String as SString;
use stabby::vec::Vec as SVec;

use crate::mirror::{RCategory, RHardwareDevice, RSensorId, RSensorKind, RUnit};
use crate::plugin::PluginError;

/// Lower bound of the scheduler's accepted sampling rate, in Hz. Below this
/// the scheduler's period math (`1e6 / rate`) starts producing unreasonably
/// large gaps; we floor here instead.
pub const MIN_RATE_HZ: f32 = 0.1;

/// Upper bound of the scheduler's accepted sampling rate, in Hz. Above this
/// most sensors saturate `/sys` read latency without producing useful new
/// data; we cap here to protect the daemon from misconfigured clients.
pub const MAX_RATE_HZ: f32 = 20.0;

// ---------------------------------------------------------------------------
// Host-facing (std) types — what the daemon stores in its registry after
// translating the R-mirror returned across the FFI boundary.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct PluginManifest {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub sensors: Vec<SensorDescriptor>,
    /// ABI v4: per-plugin hardware devices the host should integrate into
    /// its Hardware page + nickname store. The daemon validates each
    /// device key, ensures uniqueness within the manifest, and rejects
    /// dangling `SensorDescriptor::device_key` references before the
    /// manifest enters the registry. See ADR-0002.
    pub devices: Vec<HardwareDevice>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginMetadata {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq)]
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
    /// ABI v4: optional pointer to a `HardwareDevice` declared on the
    /// owning [`PluginManifest::devices`]. The host validates referential
    /// integrity in `host_init` — a sensor pointing at a key that's not
    /// in the manifest's `devices` list is a `PluginError::Manifest`.
    pub device_key: Option<HardwareDeviceKey>,
    /// NEW: sensor tags for filtering and grouping in the UI.
    pub tags: Vec<String>,
}

impl SensorDescriptor {
    /// Native rate hint, clamped into the scheduler's accepted range.
    pub fn clamped_rate_hz(&self) -> f32 {
        self.native_rate_hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ)
    }
}

// ---------------------------------------------------------------------------
// R-mirror types — what the stabbified trait actually returns. These are
// what crosses the FFI boundary; the host converts to the std types above.
// ---------------------------------------------------------------------------

#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RSensorDescriptor {
    pub id: RSensorId,
    pub display_name: SString,
    pub unit: RUnit,
    pub kind: RSensorKind,
    pub category: RCategory,
    pub native_rate_hz: f32,
    pub min: SOption<f64>,
    pub max: SOption<f64>,
    pub device_id: SOption<SString>,
    /// ABI v4: optional hardware-device key (raw string form). Validated
    /// by the host via `HardwareDeviceKey::try_new` in `host_init`.
    pub device_key: SOption<SString>,
    /// NEW: sensor tags for filtering and grouping in the UI.
    pub tags: SVec<SString>,
}

#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RPluginManifest {
    pub plugin_id: SString,
    pub display_name: SString,
    pub version: SString,
    pub sensors: SVec<RSensorDescriptor>,
    /// ABI v4: plugin-declared hardware devices. See ADR-0002.
    pub devices: SVec<RHardwareDevice>,
}

#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RPluginMetadata {
    pub plugin_id: SString,
    pub display_name: SString,
    pub version: SString,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<SensorDescriptor> for RSensorDescriptor {
    fn from(d: SensorDescriptor) -> Self {
        Self {
            id: d.id.into(),
            display_name: d.display_name.as_str().into(),
            unit: d.unit.into(),
            kind: d.kind.into(),
            category: d.category.into(),
            native_rate_hz: d.native_rate_hz,
            min: d.min.into(),
            max: d.max.into(),
            device_id: d.device_id.map(|s| SString::from(s.as_str())).into(),
            device_key: d.device_key.map(|k| SString::from(k.as_str())).into(),
            tags: d.tags.iter().map(|t| t.as_str().into()).collect::<SVec<_>>(),
        }
    }
}

impl From<RSensorDescriptor> for SensorDescriptor {
    fn from(r: RSensorDescriptor) -> Self {
        let device_id: Option<SString> = r.device_id.into();
        let device_key: Option<HardwareDeviceKey> = Option::from(r.device_key).map(|s: SString| {
            HardwareDeviceKey::try_new(s.as_str().to_owned()).expect("validated by host_init")
        });
        Self {
            id: r.id.into(),
            display_name: r.display_name.as_str().to_owned(),
            unit: r.unit.into(),
            kind: r.kind.into(),
            category: r.category.into(),
            native_rate_hz: r.native_rate_hz,
            min: r.min.into(),
            max: r.max.into(),
            device_id: device_id.map(|s| s.as_str().to_owned()),
            device_key,
            tags: crate::mirror::svec_into_std(r.tags),
        }
    }
}

impl From<PluginManifest> for RPluginManifest {
    fn from(m: PluginManifest) -> Self {
        let mut sensors = SVec::with_capacity(m.sensors.len());
        for s in m.sensors {
            sensors.push(s.into());
        }
        let mut devices = SVec::with_capacity(m.devices.len());
        for d in m.devices {
            devices.push(d.into());
        }
        Self {
            plugin_id: m.plugin_id.as_str().into(),
            display_name: m.display_name.as_str().into(),
            version: m.version.as_str().into(),
            sensors,
            devices,
        }
    }
}

impl From<RPluginManifest> for PluginManifest {
    fn from(r: RPluginManifest) -> Self {
        // Use the shared `svec_into_std` helper rather than the
        // previous duplicated 2N-allocation reverse-then-reverse.
        let sensors: Vec<SensorDescriptor> = crate::mirror::svec_into_std(r.sensors);
        let devices: Vec<HardwareDevice> = crate::mirror::svec_into_std(r.devices);
        Self {
            plugin_id: r.plugin_id.as_str().to_owned(),
            display_name: r.display_name.as_str().to_owned(),
            version: r.version.as_str().to_owned(),
            sensors,
            devices,
        }
    }
}

impl From<PluginMetadata> for RPluginMetadata {
    fn from(m: PluginMetadata) -> Self {
        Self {
            plugin_id: m.plugin_id.as_str().into(),
            display_name: m.display_name.as_str().into(),
            version: m.version.as_str().into(),
        }
    }
}

impl From<RPluginMetadata> for PluginMetadata {
    fn from(r: RPluginMetadata) -> Self {
        Self {
            plugin_id: r.plugin_id.as_str().to_owned(),
            display_name: r.display_name.as_str().to_owned(),
            version: r.version.as_str().to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Host-side v4 manifest validation.
//
// Walks the raw R-mirror (BEFORE conversion to std types so we can produce a
// `PluginError::Manifest` instead of crashing on the conversion's
// `expect("validated by host_init")` panic). Three rules:
//
//   1. Every `RHardwareDevice::key` must parse via `HardwareDeviceKey::try_new`.
//   2. Device keys are unique within the manifest's `devices` vector.
//   3. Every `RSensorDescriptor::device_key` (if present) names a key
//      that appears in `devices`.
//
// Called from `host_init`; see ADR-0002 for context.
// ---------------------------------------------------------------------------

pub(crate) fn validate_manifest(m: &RPluginManifest) -> Result<(), PluginError> {
    use std::collections::HashSet;
    let mut keys = HashSet::new();
    for dev in m.devices.iter() {
        let key_str = dev.key.as_str();
        if let Err(e) = HardwareDeviceKey::try_new(key_str.to_owned()) {
            return Err(PluginError::Manifest(format!("invalid device key {:?}: {}", key_str, e)));
        }
        if !keys.insert(key_str.to_owned()) {
            return Err(PluginError::Manifest(format!(
                "duplicate device key in manifest: {:?}",
                key_str
            )));
        }
    }
    for s in m.sensors.iter() {
        // stabby's `Option<T>` doesn't expose Rust-level `Some`/`None`
        // variants — `Some()` and `None()` are inherent associated
        // functions on the type, not enum variants — so a normal
        // pattern match is rejected as a function call in a pattern.
        // Convert through stabby's `as_ref()` -> `Option<&T>` shape,
        // which DOES support std-style pattern matching.
        let opt: Option<&SString> = s.device_key.as_ref();
        if let Some(key_s) = opt {
            let k = key_s.as_str();
            if !keys.contains(k) {
                return Err(PluginError::Manifest(format!(
                    "sensor {} references device_key {:?} not in manifest.devices",
                    s.id.value.as_str(),
                    k
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_descriptor_clamps_native_rate() {
        let d = SensorDescriptor {
            id: SensorId::new("foo"),
            display_name: "Foo".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 99.0,
            min: None,
            max: None,
            device_id: None,
            device_key: None,
            tags: vec![],
        };
        assert_eq!(d.clamped_rate_hz(), 20.0);
    }

    #[test]
    fn sensor_descriptor_clamps_low_rate() {
        let d = SensorDescriptor {
            id: SensorId::new("foo"),
            display_name: "Foo".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 0.001,
            min: None,
            max: None,
            device_id: None,
            device_key: None,
            tags: vec![],
        };
        assert_eq!(d.clamped_rate_hz(), 0.1);
    }

    #[test]
    fn descriptor_round_trips_through_r_mirror() {
        let d = SensorDescriptor {
            id: SensorId::new("net.eth0.rx_bytes"),
            display_name: "rx".into(),
            unit: Unit::Custom("Mb/s".into()),
            kind: SensorKind::Counter,
            category: Category::Network,
            native_rate_hz: 2.0,
            min: Some(0.0),
            max: None,
            device_id: Some("eth0".into()),
            device_key: None,
            tags: vec!["network".into()],
        };
        let r: RSensorDescriptor = d.clone().into();
        let back: SensorDescriptor = r.into();
        assert_eq!(d, back);
    }

    #[test]
    fn manifest_round_trips_through_r_mirror() {
        let m = PluginManifest {
            plugin_id: "io.example".into(),
            display_name: "Example".into(),
            version: "1.2.3".into(),
            sensors: vec![SensorDescriptor {
                id: SensorId::new("ex.s"),
                display_name: "s".into(),
                unit: Unit::Percent,
                kind: SensorKind::Scalar,
                category: Category::Custom,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: Some(100.0),
                device_id: None,
                device_key: None,
                tags: vec![],
            }],
            devices: vec![],
        };
        let r: RPluginManifest = m.clone().into();
        let back: PluginManifest = r.into();
        assert_eq!(m, back);
    }

    #[test]
    fn manifest_with_many_sensors_preserves_order() {
        // Regression guard for the previous pop-twice antipattern: a
        // multi-sensor manifest must round-trip with sensor descriptors
        // in their original order (the host registry uses iteration
        // order to assign plugin indices).
        let sensors: Vec<SensorDescriptor> = (0..10)
            .map(|i| SensorDescriptor {
                id: SensorId::new(format!("sensor.{i}")),
                display_name: format!("Sensor {i}"),
                unit: Unit::Count,
                kind: SensorKind::Scalar,
                category: Category::Custom,
                native_rate_hz: 1.0,
                min: None,
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            })
            .collect();
        let m = PluginManifest {
            plugin_id: "io.example".into(),
            display_name: "Example".into(),
            version: "1.2.3".into(),
            sensors: sensors.clone(),
            devices: vec![],
        };
        let r: RPluginManifest = m.into();
        let back: PluginManifest = r.into();
        let back_ids: Vec<String> = back.sensors.iter().map(|s| s.id.to_string()).collect();
        let expect_ids: Vec<String> = sensors.iter().map(|s| s.id.to_string()).collect();
        assert_eq!(back_ids, expect_ids, "sensor order must be preserved through FFI mirror");
    }

    // -----------------------------------------------------------------------
    // host_init manifest-validation tests (ABI v4).
    //
    // These exercise `validate_manifest` against synthetic R-mirror
    // manifests. Building the R-mirror by hand (rather than via the
    // std-typed `PluginManifest` and `.into()`) lets us inject invariants
    // the std API would reject up-front — duplicate keys, malformed keys,
    // and dangling `device_key` references all need the raw-FFI form.
    // -----------------------------------------------------------------------

    use crate::mirror::{RHardwareCategoryKind, RHardwareDevice};

    fn make_minimal_manifest() -> RPluginManifest {
        RPluginManifest {
            plugin_id: "test.minimal".into(),
            display_name: "Minimal".into(),
            version: "0.0.1".into(),
            sensors: SVec::new(),
            devices: SVec::new(),
        }
    }

    fn r_dev(key: &str, category: RHardwareCategoryKind, model: &str) -> RHardwareDevice {
        RHardwareDevice {
            key: key.into(),
            category_kind: category,
            model: model.into(),
            vendor: SOption::None(),
            location: SOption::None(),
            plugin_device_id: "dev0".into(),
        }
    }

    fn make_minimal_manifest_with_one_device() -> RPluginManifest {
        let mut m = make_minimal_manifest();
        let mut devs = SVec::new();
        devs.push(r_dev("pci:0000:06:00.0", RHardwareCategoryKind::Gpu, "Arc B-series"));
        m.devices = devs;
        m
    }

    #[test]
    fn host_init_rejects_invalid_device_key() {
        let mut m = make_minimal_manifest();
        let mut devs = SVec::new();
        devs.push(r_dev("BAD_KEY", RHardwareCategoryKind::Gpu, "bogus"));
        m.devices = devs;
        let err = validate_manifest(&m).unwrap_err();
        match err {
            PluginError::Manifest(msg) => {
                assert!(msg.contains("BAD_KEY"), "expected error to name the bad key; got: {msg}",);
            }
            other => panic!("expected PluginError::Manifest, got {other:?}"),
        }
    }

    #[test]
    fn host_init_rejects_sensor_pointing_at_absent_device() {
        let mut m = make_minimal_manifest_with_one_device();
        let mut sensors = SVec::new();
        sensors.push(RSensorDescriptor {
            id: RSensorId { value: "test.dangling".into() },
            display_name: "Dangling".into(),
            unit: crate::mirror::RUnit {
                kind: crate::mirror::RUnitKind::Count,
                custom: SOption::None(),
            },
            kind: RSensorKind::Scalar,
            category: RCategory::Custom,
            native_rate_hz: 1.0,
            min: SOption::None(),
            max: SOption::None(),
            device_id: SOption::None(),
            device_key: SOption::Some(SString::from("pci:0000:99:00.0")),
            tags: SVec::new(),
        });
        m.sensors = sensors;
        let err = validate_manifest(&m).unwrap_err();
        match err {
            PluginError::Manifest(msg) => {
                assert!(
                    msg.contains("test.dangling") && msg.contains("pci:0000:99:00.0"),
                    "expected error to name the sensor and the dangling key; got: {msg}",
                );
            }
            other => panic!("expected PluginError::Manifest, got {other:?}"),
        }
    }

    #[test]
    fn host_init_rejects_duplicate_device_keys_within_manifest() {
        let mut m = make_minimal_manifest();
        let mut devs = SVec::new();
        devs.push(r_dev("pci:0000:06:00.0", RHardwareCategoryKind::Gpu, "first"));
        devs.push(r_dev("pci:0000:06:00.0", RHardwareCategoryKind::Gpu, "duplicate"));
        m.devices = devs;
        let err = validate_manifest(&m).unwrap_err();
        match err {
            PluginError::Manifest(msg) => {
                assert!(
                    msg.contains("duplicate") && msg.contains("pci:0000:06:00.0"),
                    "expected error to flag duplicate and the key; got: {msg}",
                );
            }
            other => panic!("expected PluginError::Manifest, got {other:?}"),
        }
    }

    #[test]
    fn host_init_accepts_valid_v4_manifest_with_devices_and_sensor_reference() {
        // Positive smoke test: a well-formed v4 manifest with one device
        // and one sensor that points at it should validate cleanly.
        let mut m = make_minimal_manifest_with_one_device();
        let mut sensors = SVec::new();
        sensors.push(RSensorDescriptor {
            id: RSensorId { value: "test.linked".into() },
            display_name: "Linked".into(),
            unit: crate::mirror::RUnit {
                kind: crate::mirror::RUnitKind::Percent,
                custom: SOption::None(),
            },
            kind: RSensorKind::Scalar,
            category: RCategory::Gpu,
            native_rate_hz: 1.0,
            min: SOption::None(),
            max: SOption::None(),
            device_id: SOption::None(),
            device_key: SOption::Some(SString::from("pci:0000:06:00.0")),
            tags: SVec::new(),
        });
        m.sensors = sensors;
        assert!(validate_manifest(&m).is_ok());
    }
}
