// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
//
// R-mirror types — `#[stabby::stabby]`-annotated counterparts to the
// `linsight-core` value types. These cross the plugin FFI boundary
// (vtable parameter / return slots) because std `String`, `Vec` and
// `Option` are not `IStable` and therefore cannot appear in a
// stabbified trait's vtable.
//
// **ABI v3 note on enum encoding.** Earlier versions used
// `#[stabby::stabby] #[repr(stabby)] enum` for tagged unions (RUnit,
// RReading, RCell). stabby's tagged-enum representation works
// correctly for `#[repr(u8)]` unit-only enums (RSensorKind,
// RCategory), but for enums with both unit AND payload variants the
// generated `match_owned` / `match_ref` dispatchers misroute closures
// at `opt-level >= 1`. Confirmed bug-for-bug reproducible on stabby
// 36.2.2: a Percent value round-trips to Celsius, a Scalar to
// Counter, etc. — a one-off variant misdispatch in the recursive
// Result-tree the macro emits. Debug builds pass; release builds
// silently corrupt the wire data.
//
// To eliminate the dependency on stabby's enum matcher, every former
// tagged enum is now a `(kind, payload_fields)` struct: an explicit
// `#[repr(u8)]` discriminant plus payload fields that are only valid
// when the corresponding variant is active. The host translates via
// trivial Rust `match`-on-the-kind expressions that don't rely on
// stabby-generated dispatch.
//
// This is a wire-format break vs ABI v2 — hence
// `LINSIGHT_PLUGIN_ABI_VERSION = 3` and the renamed factory symbol
// (`linsight_plugin_v3`). The daemon's version-symbol check refuses
// any v2 .so at load time.
//
// **ABI v4 note.** v4 keeps the same R-mirror encoding scheme as v3.
// The break is in `RPluginManifest` and `RSensorDescriptor`: the
// former grows `devices: SVec<RHardwareDevice>`, the latter grows
// `device_key: SOption<SString>`. The factory symbol moved to
// `linsight_plugin_v4` so v3 plugins fail symbol lookup at load
// time. See ADR-0002 and `RHardwareCategoryKind` / `RHardwareDevice`
// below.

use linsight_core::{Category, Cell, Reading, SensorId, SensorKind, TableRow, Unit};
use stabby::option::Option as SOption;
use stabby::string::String as SString;
use stabby::vec::Vec as SVec;

// ---------------------------------------------------------------------------
// RSensorId — newtype wrapper carrying a stabby String.
// ---------------------------------------------------------------------------

/// FFI-safe mirror of [`SensorId`]. Carries an opaque UTF-8 string;
/// plugins should construct via `SensorId::new(...).into()` rather than
/// touching the `value` field directly.
///
/// **Validation note:** the host runs every `RSensorId` produced by a
/// plugin through `SensorId::try_new` in [`crate::host_init`] before it
/// is allowed into the daemon's registry. A plugin that emits an empty
/// or whitespace-bearing string here is rejected with
/// [`PluginError::Parse`](crate::PluginError::Parse).
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RSensorId {
    pub value: SString,
}

impl From<SensorId> for RSensorId {
    fn from(id: SensorId) -> Self {
        Self { value: id.as_str().into() }
    }
}

impl From<&SensorId> for RSensorId {
    fn from(id: &SensorId) -> Self {
        Self { value: id.as_str().into() }
    }
}

impl From<RSensorId> for SensorId {
    fn from(r: RSensorId) -> Self {
        SensorId::new(r.value.as_str())
    }
}

// ---------------------------------------------------------------------------
// RUnit (struct kind+payload)
// ---------------------------------------------------------------------------

/// Discriminant for [`RUnit`]. Unit-only enum, fixed `#[repr(u8)]`
/// layout; new variants are wire-format-breaking and require an ABI
/// bump.
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RUnitKind {
    Percent,
    Celsius,
    Bytes,
    BytesPerSec,
    Hertz,
    Watts,
    Volts,
    Rpm,
    Count,
    Custom,
}

/// FFI-safe mirror of [`Unit`]. Encoded as a `kind` discriminant plus a
/// `custom` payload that is `Some(label)` only when `kind == Custom`.
/// All other variants set `custom: None`.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RUnit {
    pub kind: RUnitKind,
    pub custom: SOption<SString>,
}

impl From<Unit> for RUnit {
    fn from(u: Unit) -> Self {
        match u {
            Unit::Percent => Self { kind: RUnitKind::Percent, custom: SOption::None() },
            Unit::Celsius => Self { kind: RUnitKind::Celsius, custom: SOption::None() },
            Unit::Bytes => Self { kind: RUnitKind::Bytes, custom: SOption::None() },
            Unit::BytesPerSec => Self { kind: RUnitKind::BytesPerSec, custom: SOption::None() },
            Unit::Hertz => Self { kind: RUnitKind::Hertz, custom: SOption::None() },
            Unit::Watts => Self { kind: RUnitKind::Watts, custom: SOption::None() },
            Unit::Volts => Self { kind: RUnitKind::Volts, custom: SOption::None() },
            Unit::Rpm => Self { kind: RUnitKind::Rpm, custom: SOption::None() },
            Unit::Count => Self { kind: RUnitKind::Count, custom: SOption::None() },
            Unit::Custom(s) => {
                Self { kind: RUnitKind::Custom, custom: SOption::Some(s.as_str().into()) }
            }
        }
    }
}

impl From<RUnit> for Unit {
    fn from(r: RUnit) -> Self {
        match r.kind {
            RUnitKind::Percent => Unit::Percent,
            RUnitKind::Celsius => Unit::Celsius,
            RUnitKind::Bytes => Unit::Bytes,
            RUnitKind::BytesPerSec => Unit::BytesPerSec,
            RUnitKind::Hertz => Unit::Hertz,
            RUnitKind::Watts => Unit::Watts,
            RUnitKind::Volts => Unit::Volts,
            RUnitKind::Rpm => Unit::Rpm,
            RUnitKind::Count => Unit::Count,
            RUnitKind::Custom => {
                // The wire format places the label in `custom`; an
                // empty payload here is a malformed message and we
                // surface it as an explicit fallback rather than
                // panicking at the FFI seam.
                let opt: Option<&SString> = r.custom.as_ref();
                let label = opt.map(|s| s.as_str().to_owned()).unwrap_or_default();
                Unit::Custom(label)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RSensorKind
// ---------------------------------------------------------------------------

/// FFI-safe mirror of [`SensorKind`]. Unit-only enum encoded as a single
/// byte — adding a new variant is a wire-format breaking change.
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum RSensorKind {
    Scalar,
    Counter,
    Table,
    State,
}

impl From<SensorKind> for RSensorKind {
    fn from(k: SensorKind) -> Self {
        match k {
            SensorKind::Scalar => RSensorKind::Scalar,
            SensorKind::Counter => RSensorKind::Counter,
            SensorKind::Table => RSensorKind::Table,
            SensorKind::State => RSensorKind::State,
        }
    }
}

impl From<RSensorKind> for SensorKind {
    fn from(r: RSensorKind) -> Self {
        match r {
            RSensorKind::Scalar => SensorKind::Scalar,
            RSensorKind::Counter => SensorKind::Counter,
            RSensorKind::Table => SensorKind::Table,
            RSensorKind::State => SensorKind::State,
        }
    }
}

// ---------------------------------------------------------------------------
// RCategory
// ---------------------------------------------------------------------------

/// FFI-safe mirror of [`Category`]. Same wire-format-stable contract as
/// [`RSensorKind`].
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum RCategory {
    Cpu,
    Gpu,
    Memory,
    Storage,
    Network,
    Custom,
}

impl From<Category> for RCategory {
    fn from(c: Category) -> Self {
        match c {
            Category::Cpu => RCategory::Cpu,
            Category::Gpu => RCategory::Gpu,
            Category::Memory => RCategory::Memory,
            Category::Storage => RCategory::Storage,
            Category::Network => RCategory::Network,
            Category::Custom => RCategory::Custom,
        }
    }
}

impl From<RCategory> for Category {
    fn from(r: RCategory) -> Self {
        match r {
            RCategory::Cpu => Category::Cpu,
            RCategory::Gpu => Category::Gpu,
            RCategory::Memory => Category::Memory,
            RCategory::Storage => Category::Storage,
            RCategory::Network => Category::Network,
            RCategory::Custom => Category::Custom,
        }
    }
}

// ---------------------------------------------------------------------------
// RHardwareCategoryKind
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// RHardwareDevice
// ---------------------------------------------------------------------------

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
        Self {
            key: linsight_core::HardwareDeviceKey::try_new(r.key.as_str().to_owned())
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

// ---------------------------------------------------------------------------
// RCell (struct kind+payload)
// ---------------------------------------------------------------------------

/// Discriminant for [`RCell`].
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RCellKind {
    Text,
    Number,
    Bytes,
}

/// FFI-safe mirror of [`Cell`] — a single cell in a Table reading.
/// Three parallel payload fields; the active one is selected by
/// `kind`. Inactive fields carry default values that are never read.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RCell {
    pub kind: RCellKind,
    pub text: SOption<SString>,
    pub number: f64,
    pub bytes: u64,
}

impl From<Cell> for RCell {
    fn from(c: Cell) -> Self {
        match c {
            Cell::Text(s) => Self {
                kind: RCellKind::Text,
                text: SOption::Some(s.as_str().into()),
                number: 0.0,
                bytes: 0,
            },
            Cell::Number(n) => {
                Self { kind: RCellKind::Number, text: SOption::None(), number: n, bytes: 0 }
            }
            Cell::Bytes(b) => {
                Self { kind: RCellKind::Bytes, text: SOption::None(), number: 0.0, bytes: b }
            }
        }
    }
}

impl From<RCell> for Cell {
    fn from(r: RCell) -> Self {
        match r.kind {
            RCellKind::Text => {
                let opt: Option<&SString> = r.text.as_ref();
                Cell::Text(opt.map(|s| s.as_str().to_owned()).unwrap_or_default())
            }
            RCellKind::Number => Cell::Number(r.number),
            RCellKind::Bytes => Cell::Bytes(r.bytes),
        }
    }
}

/// FFI-safe mirror of [`TableRow`] — one row of an arbitrary-width
/// [`RReading::Table`].
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RTableRow {
    pub cells: SVec<RCell>,
}

impl From<TableRow> for RTableRow {
    fn from(row: TableRow) -> Self {
        let mut cells = SVec::with_capacity(row.cells.len());
        for c in row.cells {
            cells.push(c.into());
        }
        Self { cells }
    }
}

impl From<RTableRow> for TableRow {
    fn from(r: RTableRow) -> Self {
        TableRow { cells: svec_into_std(r.cells) }
    }
}

// ---------------------------------------------------------------------------
// RReading (struct kind+payload)
// ---------------------------------------------------------------------------

/// Discriminant for [`RReading`].
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RReadingKind {
    Scalar,
    Counter,
    Table,
    State,
}

/// FFI-safe mirror of [`Reading`] — every sample a plugin returns is
/// one of these variants, selected by `kind`. The four payload fields
/// (scalar / counter / state / table) live alongside the discriminant
/// and are read individually based on `kind`. Inactive fields carry
/// defaults that callers must not read.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RReading {
    pub kind: RReadingKind,
    pub scalar: f64,
    pub counter: u64,
    pub state: SOption<SString>,
    pub table: SVec<RTableRow>,
}

impl From<Reading> for RReading {
    fn from(r: Reading) -> Self {
        match r {
            Reading::Scalar(v) => Self {
                kind: RReadingKind::Scalar,
                scalar: v,
                counter: 0,
                state: SOption::None(),
                table: SVec::new(),
            },
            Reading::Counter(v) => Self {
                kind: RReadingKind::Counter,
                scalar: 0.0,
                counter: v,
                state: SOption::None(),
                table: SVec::new(),
            },
            Reading::State(s) => Self {
                kind: RReadingKind::State,
                scalar: 0.0,
                counter: 0,
                state: SOption::Some(s.as_str().into()),
                table: SVec::new(),
            },
            Reading::Table(rows) => {
                let mut out = SVec::with_capacity(rows.len());
                for row in rows {
                    out.push(row.into());
                }
                Self {
                    kind: RReadingKind::Table,
                    scalar: 0.0,
                    counter: 0,
                    state: SOption::None(),
                    table: out,
                }
            }
        }
    }
}

impl From<RReading> for Reading {
    fn from(r: RReading) -> Self {
        match r.kind {
            RReadingKind::Scalar => Reading::Scalar(r.scalar),
            RReadingKind::Counter => Reading::Counter(r.counter),
            RReadingKind::State => {
                let opt: Option<&SString> = r.state.as_ref();
                Reading::State(opt.map(|s| s.as_str().to_owned()).unwrap_or_default())
            }
            RReadingKind::Table => Reading::Table(svec_into_std(r.table)),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Drain a `stabby::vec::Vec<R>` into a `std::vec::Vec<T>` using
/// the `From<R>` impl on `T`, preserving original order.
pub(crate) fn svec_into_std<R, T>(mut v: SVec<R>) -> Vec<T>
where
    T: From<R>,
    R: stabby::IStable + Clone,
{
    let len = v.len();
    let mut out: Vec<T> = Vec::with_capacity(len);
    while let Some(item) = v.pop() {
        out.push(T::from(item));
    }
    out.reverse();
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_round_trips() {
        for u in [
            Unit::Percent,
            Unit::Celsius,
            Unit::Bytes,
            Unit::BytesPerSec,
            Unit::Hertz,
            Unit::Watts,
            Unit::Volts,
            Unit::Rpm,
            Unit::Count,
            Unit::Custom("Mb/s".into()),
        ] {
            let r: RUnit = u.clone().into();
            let back: Unit = r.into();
            assert_eq!(u, back, "round trip failed for variant {u:?}");
        }
    }

    #[test]
    fn sensor_kind_round_trips() {
        for k in [SensorKind::Scalar, SensorKind::Counter, SensorKind::Table, SensorKind::State] {
            let r: RSensorKind = k.into();
            assert_eq!(SensorKind::from(r), k);
        }
    }

    #[test]
    fn category_round_trips() {
        for c in [
            Category::Cpu,
            Category::Gpu,
            Category::Memory,
            Category::Storage,
            Category::Network,
            Category::Custom,
        ] {
            let r: RCategory = c.into();
            assert_eq!(Category::from(r), c);
        }
    }

    #[test]
    fn reading_table_round_trips() {
        let r = Reading::Table(vec![TableRow {
            cells: vec![Cell::Text("p".into()), Cell::Number(1.0), Cell::Bytes(42)],
        }]);
        let rr: RReading = r.clone().into();
        let back: Reading = rr.into();
        assert_eq!(back, r);
    }

    #[test]
    fn reading_scalar_round_trips() {
        let r = Reading::Scalar(42.5);
        let rr: RReading = r.clone().into();
        assert_eq!(Reading::from(rr), r);
    }

    #[test]
    fn reading_counter_round_trips() {
        let r = Reading::Counter(123_456_789);
        let rr: RReading = r.clone().into();
        assert_eq!(Reading::from(rr), r);
    }

    #[test]
    fn reading_state_round_trips() {
        let r = Reading::State("up".into());
        let rr: RReading = r.clone().into();
        assert_eq!(Reading::from(rr), r);
    }

    #[test]
    fn sensor_id_round_trips() {
        let id = SensorId::new("cpu.util");
        let r: RSensorId = id.clone().into();
        assert_eq!(SensorId::from(r), id);
    }

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
            assert_eq!(back, c);
        }
    }

    #[test]
    fn hardware_device_round_trips_minimal() {
        use linsight_core::{HardwareCategory, HardwareDevice, HardwareDeviceKey};
        let dev = HardwareDevice {
            key: HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap(),
            category: HardwareCategory::Gpu,
            model: "Intel Arc B-series".into(),
            vendor: None,
            location: None,
            plugin_id: String::new(),
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
}
