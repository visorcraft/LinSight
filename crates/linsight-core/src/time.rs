// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Shared time/duration utilities.

use std::time::Duration;

/// Parse a duration string with a `d` (days), `h` (hours), or `m` (minutes)
/// integer suffix.
///
/// - Input is trimmed before parsing.
/// - The numeric value must be a positive integer (> 0).
/// - Arithmetic uses `checked_mul` so overflow returns `None`.
/// - Returns `None` for any input that doesn't match the grammar.
pub fn parse_duration_dhm(s: &str) -> Option<Duration> {
    let s = s.trim();
    let split_at = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(split_at);
    let n: u64 = digits.parse().ok().filter(|&v| v > 0)?;
    let secs = match unit {
        "d" => n.checked_mul(86_400)?,
        "h" => n.checked_mul(3_600)?,
        "m" => n.checked_mul(60)?,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_dhm_suffixes() {
        assert_eq!(parse_duration_dhm("30d"), Some(Duration::from_secs(30 * 86_400)));
        assert_eq!(parse_duration_dhm("12h"), Some(Duration::from_secs(12 * 3_600)));
        assert_eq!(parse_duration_dhm("45m"), Some(Duration::from_secs(45 * 60)));
        assert_eq!(parse_duration_dhm("1d"), Some(Duration::from_secs(86_400)));
        assert_eq!(parse_duration_dhm("2h"), Some(Duration::from_secs(7_200)));
        assert_eq!(parse_duration_dhm("30m"), Some(Duration::from_secs(1_800)));
    }

    #[test]
    fn parse_duration_dhm_rejects_garbage() {
        assert_eq!(parse_duration_dhm("garbage"), None);
        assert_eq!(parse_duration_dhm("5x"), None);
        assert_eq!(parse_duration_dhm(""), None);
    }

    #[test]
    fn parse_duration_dhm_rejects_zero() {
        assert_eq!(parse_duration_dhm("0d"), None);
        assert_eq!(parse_duration_dhm("0h"), None);
        assert_eq!(parse_duration_dhm("0m"), None);
    }

    #[test]
    fn parse_duration_dhm_rejects_overflow() {
        assert_eq!(parse_duration_dhm("99999999999999999999d"), None);
        assert_eq!(parse_duration_dhm("999999999999999999h"), None);
    }

    #[test]
    fn parse_duration_dhm_trims_whitespace() {
        assert_eq!(parse_duration_dhm("  7d  "), Some(Duration::from_secs(7 * 86_400)));
        assert_eq!(parse_duration_dhm(" 3h "), Some(Duration::from_secs(3 * 3_600)));
    }
}
