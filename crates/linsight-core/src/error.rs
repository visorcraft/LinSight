// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

/// Library-wide error type. Each variant exists to let callers distinguish
/// failure modes; `InvalidSensorId` is no longer a catch-all (it was being
/// reused for unrelated I/O and serde failures in `dashboard::load/save`,
/// which is a documented review finding).
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid sensor id: {0}")]
    InvalidSensorId(String),

    #[error("dashboard schema migration failed: v{from} → v{to}: {reason}")]
    MigrationFailed { from: u32, to: u32, reason: String },

    #[error("dashboard schema version v{0} is from the future; daemon supports v{supported}", supported = crate::dashboard::DASHBOARD_SCHEMA_VERSION)]
    UnsupportedSchema(u32),

    #[error("i/o: {0}")]
    Io(String),

    #[error("serialize/deserialize: {0}")]
    Serialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_sensor_id_display() {
        let e = CoreError::InvalidSensorId("oops".into());
        assert_eq!(e.to_string(), "invalid sensor id: oops");
    }

    #[test]
    fn migration_error_display() {
        let e =
            CoreError::MigrationFailed { from: 1, to: 2, reason: "no migrator registered".into() };
        assert_eq!(
            e.to_string(),
            "dashboard schema migration failed: v1 → v2: no migrator registered",
        );
    }

    #[test]
    fn unsupported_schema_mentions_supported_version() {
        let e = CoreError::UnsupportedSchema(99);
        assert!(e.to_string().contains("v99"));
        assert!(e.to_string().contains("v1"));
    }
}
