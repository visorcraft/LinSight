// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::fmt;
use std::time::{Duration, Instant};

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
#[serde(rename_all = "lowercase")]
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
}

/// Generic time-bounded cache for sensor snapshot data.
///
/// Used by sensor plugins to avoid re-reading kernel files multiple
/// times per sample window.  Create with [`SnapshotCache::new`] and
/// call [`SnapshotCache::get`] to retrieve the data while it is fresh.
#[derive(Clone, Debug)]
pub struct SnapshotCache<T> {
    captured_at: Instant,
    data: T,
}

impl<T: Clone> SnapshotCache<T> {
    /// Store `data` with the current timestamp.
    pub fn new(data: T) -> Self {
        Self { captured_at: Instant::now(), data }
    }

    /// Return a clone of the data if it was captured within `ttl`.
    pub fn get(&self, ttl: Duration) -> Option<T> {
        if self.captured_at.elapsed() <= ttl { Some(self.data.clone()) } else { None }
    }
}

#[cfg(test)]
mod snapshot_cache_tests {
    use super::SnapshotCache;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn get_returns_data_within_ttl() {
        let cache = SnapshotCache::new(42);
        assert_eq!(cache.get(Duration::from_millis(50)), Some(42));
    }

    #[test]
    fn get_returns_none_after_ttl() {
        let cache = SnapshotCache::new(42);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(cache.get(Duration::from_millis(50)), None);
    }
}

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    #[allow(dead_code)]
    fn sensor_id_strategy() -> impl Strategy<Value = SensorId> {
        "[a-z0-9_][a-z0-9_.]*"
            .prop_filter("non-empty", |s: &String| !s.is_empty())
            .prop_map(SensorId::new)
    }

    #[allow(dead_code)]
    fn cell_strategy() -> impl Strategy<Value = Cell> {
        prop_oneof![
            "[a-zA-Z0-9_ ]*".prop_map(Cell::Text),
            any::<f64>().prop_map(Cell::Number),
            any::<u64>().prop_map(Cell::Bytes),
        ]
    }

    #[allow(dead_code)]
    fn reading_strategy() -> impl Strategy<Value = Reading> {
        prop_oneof![
            any::<f64>().prop_map(Reading::Scalar),
            any::<u64>().prop_map(Reading::Counter),
            "[a-zA-Z0-9_ ]*".prop_map(Reading::State),
            proptest::collection::vec(
                proptest::collection::vec(cell_strategy(), 0..8)
                    .prop_map(|cells| TableRow { cells }),
                0..8,
            )
            .prop_map(Reading::Table),
        ]
    }

    #[allow(dead_code)]
    fn sample_strategy() -> impl Strategy<Value = Sample> {
        (sensor_id_strategy(), any::<u64>(), reading_strategy())
            .prop_map(|(sensor, ts_micros, reading)| Sample { sensor, ts_micros, reading })
    }

    proptest! {
        fn sensor_id_try_new_accepts_valid_ids(id in sensor_id_strategy()) {
            prop_assert!(SensorId::try_new(id.as_str()).is_ok());
        }

        fn sensor_id_try_new_rejects_whitespace(
            prefix in "[a-z0-9.]*",
            ws in "[ \t\n]+",
            suffix in "[a-z0-9.]*",
        ) {
            let bad = format!("{prefix}{ws}{suffix}");
            prop_assert!(SensorId::try_new(bad).is_err());
        }

        fn reading_json_round_trips(reading in reading_strategy()) {
            let json = serde_json::to_string(&reading).unwrap();
            let back: Reading = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(reading, back);
        }

        fn sample_json_round_trips(sample in sample_strategy()) {
            let json = serde_json::to_string(&sample).unwrap();
            let back: Sample = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(sample, back);
        }
    }

    #[test]
    fn sensor_id_try_new_rejects_empty() {
        assert!(SensorId::try_new("").is_err());
    }
}
