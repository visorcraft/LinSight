// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// Stable, human-readable sensor identifier.
///
/// Convention: dot-separated path of lowercase ASCII identifiers,
/// e.g., `"cpu.util"`, `"xe.gpu1.temp_c"`. The string never contains
/// whitespace and is never empty.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SensorId(String);

impl SensorId {
    /// Construct from a static-known good string.
    ///
    /// Panics in debug builds if the string violates the invariants
    /// (empty / contains whitespace). Use [`SensorId::try_new`] for
    /// runtime-derived strings.
    pub fn new(s: impl Into<String>) -> Self {
        let s = s.into();
        debug_assert!(
            !s.is_empty() && !s.chars().any(char::is_whitespace),
            "SensorId invariant violated: {s:?}"
        );
        Self(s)
    }

    /// Fallible constructor for runtime-derived strings.
    pub fn try_new(s: impl Into<String>) -> CoreResult<Self> {
        let s = s.into();
        if s.is_empty() {
            return Err(CoreError::InvalidSensorId("empty".into()));
        }
        if s.chars().any(char::is_whitespace) {
            return Err(CoreError::InvalidSensorId(format!("whitespace in {s:?}")));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SensorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SensorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SensorId({})", self.0)
    }
}

/// Measurement unit for a sensor value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    Percent,
    Celsius,
    Bytes,
    BytesPerSec,
    Hertz,
    Watts,
    Volts,
    Rpm,
    Count,
    Custom(String),
}

impl Unit {
    pub fn symbol(&self) -> &str {
        match self {
            Unit::Percent => "%",
            Unit::Celsius => "°C",
            Unit::Bytes => "B",
            Unit::BytesPerSec => "B/s",
            Unit::Hertz => "Hz",
            Unit::Watts => "W",
            Unit::Volts => "V",
            Unit::Rpm => "rpm",
            Unit::Count => "",
            Unit::Custom(s) => s,
        }
    }
}

/// High-level grouping for the dashboard UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Cpu,
    Gpu,
    Memory,
    Storage,
    Network,
    Custom,
}

/// Shape of values a sensor emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SensorKind {
    /// Continuous numeric value (utilization, temperature, etc.).
    Scalar,
    /// Monotonically increasing counter (bytes transferred, etc.).
    Counter,
    /// Tabular value (process list, etc.).
    Table,
    /// Discrete labeled state (power state, link status, etc.).
    State,
}

/// One sample value as emitted by a sensor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reading {
    Scalar(f64),
    Counter(u64),
    Table(Vec<TableRow>),
    State(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cell {
    Text(String),
    Number(f64),
    Bytes(u64),
}

/// One sensor reading at a point in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub sensor: SensorId,
    /// Microseconds since the Unix epoch (UTC).
    pub ts_micros: u64,
    pub reading: Reading,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_id_displays_as_its_string() {
        let id = SensorId::new("cpu.util");
        assert_eq!(id.to_string(), "cpu.util");
    }

    #[test]
    fn sensor_id_rejects_empty() {
        assert!(SensorId::try_new("").is_err());
    }

    #[test]
    fn sensor_id_rejects_whitespace() {
        assert!(SensorId::try_new("cpu util").is_err());
        assert!(SensorId::try_new("cpu\tutil").is_err());
    }

    #[test]
    fn sensor_id_orders_lexicographically() {
        let a = SensorId::new("cpu.util");
        let b = SensorId::new("mem.used");
        assert!(a < b);
    }

    #[test]
    fn unit_displays_with_symbol() {
        assert_eq!(Unit::Percent.symbol(), "%");
        assert_eq!(Unit::Celsius.symbol(), "°C");
        assert_eq!(Unit::Bytes.symbol(), "B");
        assert_eq!(Unit::BytesPerSec.symbol(), "B/s");
        assert_eq!(Unit::Hertz.symbol(), "Hz");
        assert_eq!(Unit::Watts.symbol(), "W");
        assert_eq!(Unit::Volts.symbol(), "V");
        assert_eq!(Unit::Rpm.symbol(), "rpm");
        assert_eq!(Unit::Count.symbol(), "");
        assert_eq!(Unit::Custom("foo".into()).symbol(), "foo");
    }

    #[test]
    fn category_round_trips_through_serde() {
        let original = Category::Gpu;
        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: Category = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn sensor_kind_round_trips_through_serde() {
        for kind in [SensorKind::Scalar, SensorKind::Counter, SensorKind::Table, SensorKind::State]
        {
            let encoded = serde_json::to_string(&kind).unwrap();
            let decoded: SensorKind = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, kind);
        }
    }

    #[test]
    fn reading_round_trips_scalar() {
        let r = Reading::Scalar(42.5);
        let encoded = serde_json::to_string(&r).unwrap();
        let decoded: Reading = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, r);
    }

    #[test]
    fn reading_round_trips_table() {
        let r = Reading::Table(vec![TableRow {
            cells: vec![
                Cell::Text("firefox".into()),
                Cell::Number(1234.0),
                Cell::Bytes(50_000_000),
            ],
        }]);
        let encoded = serde_json::to_string(&r).unwrap();
        let decoded: Reading = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, r);
    }

    #[test]
    fn reading_round_trips_state_and_counter() {
        for r in [Reading::Counter(999_999), Reading::State("P0".into())] {
            let encoded = serde_json::to_string(&r).unwrap();
            let decoded: Reading = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, r);
        }
    }

    #[test]
    fn sample_holds_id_ts_and_reading() {
        let s = Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 1_700_000_000_000_000,
            reading: Reading::Scalar(33.3),
        };
        assert_eq!(s.sensor.as_str(), "cpu.util");
        assert_eq!(s.ts_micros, 1_700_000_000_000_000);
        assert!(matches!(s.reading, Reading::Scalar(v) if v == 33.3));
    }
}
