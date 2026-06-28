// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

/// Library-wide error type.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid sensor id: {0}")]
    InvalidSensorId(String),

    #[error("i/o: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_sensor_id_display() {
        let e = CoreError::InvalidSensorId("oops".into());
        assert_eq!(e.to_string(), "invalid sensor id: oops");
    }
}
