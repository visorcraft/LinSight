// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `linsight-cli db {stats,prune}` — offline history-database maintenance.
//!
//! These commands open the SQLite history DB directly (without the daemon) for
//! read-only inspection (`stats`) and offline housekeeping (`prune`).
//! The daemon keeps the DB in WAL mode; we use a 5-second busy_timeout so the
//! two can coexist safely.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use linsight_core::{history_db_path, parse_duration_dhm};
use rusqlite::{Connection, OpenFlags};

// NOTE: The d/h/m integer-suffix grammar is shared with `apps/linsightd/src/history.rs`
// `parse_retention` — both now delegate to `linsight_core::parse_duration_dhm`.
// Two legacy float-grammar parsers remain local to their call sites:
// `apps/linsightd/src/alerts.rs` `parse_duration` (float, s/ms grammar) and
// `crates/linsight-cli/src/commands/history.rs` `parse_duration_to_micros` (float grammar).

/// Parse a duration string with a `d`, `h`, or `m` suffix into a `Duration`.
/// Returns an error on unknown suffixes, zero values, or integer overflow.
pub(crate) fn parse_duration(s: &str) -> Result<Duration> {
    parse_duration_dhm(s).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid duration {:?}; expected a positive integer with d/h/m suffix (e.g. 30d, 12h, 45m)",
            s.trim()
        )
    })
}

/// Resolve the default history DB path. Delegates to the shared resolver
/// in `linsight_core::paths` so the CLI and daemon always agree on the location.
pub(crate) fn default_db_path() -> PathBuf {
    history_db_path()
}

fn open_db(path: &std::path::Path, flags: OpenFlags) -> Result<Connection> {
    if !path.exists() {
        anyhow::bail!(
            "history database not found: {}\n\
             Enable history with LINSIGHT_HISTORY=1 (or via the systemd user unit at \
             packaging/systemd/linsight.service for always-on mode), or pass --db <path>.",
            path.display()
        );
    }
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5)).context("setting busy_timeout")?;
    Ok(conn)
}

/// Format a microsecond timestamp as a UTC human-readable string.
fn micros_to_utc(micros: u64) -> String {
    let secs = (micros / 1_000_000) as i64;
    let millis = ((micros % 1_000_000) / 1_000) as u32;
    // Manual UTC formatting without chrono to keep deps minimal.
    let total_secs = secs;
    let s = total_secs % 60;
    let total_mins = total_secs / 60;
    let m = total_mins % 60;
    let total_hours = total_mins / 60;
    let h = total_hours % 24;
    let days_since_epoch = total_hours / 24;
    // Gregorian calendar calculation from days since 1970-01-01.
    let (year, month, day) = days_to_ymd(days_since_epoch as u32);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// Convert days since 1970-01-01 to (year, month, day).
fn days_to_ymd(mut z: u32) -> (u32, u32, u32) {
    // Algorithm from Howard Hinnant: https://howardhinnant.github.io/date_algorithms.html
    z += 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub fn stats(db_path: Option<PathBuf>) -> Result<()> {
    let path = db_path.unwrap_or_else(default_db_path);
    let conn = open_db(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;

    let row_count: u64 = conn.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))?;
    let distinct_sensors: u64 =
        conn.query_row("SELECT COUNT(DISTINCT sensor_id) FROM samples", [], |r| r.get(0))?;

    let (min_ts, max_ts): (Option<i64>, Option<i64>) =
        conn.query_row("SELECT MIN(ts), MAX(ts) FROM samples", [], |r| Ok((r.get(0)?, r.get(1)?)))?;

    let file_size =
        std::fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?.len();

    println!("db:              {}", path.display());
    println!("file size:       {} bytes ({:.1} MiB)", file_size, file_size as f64 / 1_048_576.0);
    println!("rows:            {row_count}");
    println!("distinct sensors:{distinct_sensors}");

    match (min_ts, max_ts) {
        (Some(min), Some(max)) => {
            let min_u = min as u64;
            let max_u = max as u64;
            println!("oldest ts:       {} ({})", min_u, micros_to_utc(min_u));
            println!("newest ts:       {} ({})", max_u, micros_to_utc(max_u));
            let span_secs = (max_u.saturating_sub(min_u)) / 1_000_000;
            println!("span:            {}s ({:.1}h)", span_secs, span_secs as f64 / 3600.0);
        }
        _ => {
            println!("oldest ts:       (no rows)");
            println!("newest ts:       (no rows)");
        }
    }

    Ok(())
}

pub fn prune(db_path: Option<PathBuf>, older_than: &str, vacuum: bool) -> Result<()> {
    let path = db_path.unwrap_or_else(default_db_path);
    let conn = open_db(&path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;

    let dur = parse_duration(older_than)?;
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let cutoff_micros = now_micros.saturating_sub(dur.as_micros() as u64);

    let removed_samples =
        conn.execute("DELETE FROM samples WHERE ts < ?1", rusqlite::params![cutoff_micros as i64])?;
    let removed_events = conn.execute(
        "DELETE FROM alert_events WHERE ts < ?1",
        rusqlite::params![cutoff_micros as i64],
    )?;

    println!(
        "removed {removed_samples} sample rows and {removed_events} alert-event rows older than {older_than} (cutoff ts: {cutoff_micros})"
    );

    if vacuum {
        conn.execute_batch("VACUUM")?;
        println!("VACUUM complete");
    }

    Ok(())
}

/// Export historical samples to CSV or JSON.
pub fn export(
    db_path: Option<PathBuf>,
    sensor: Option<&str>,
    since: &str,
    format: &str,
    output: Option<&std::path::Path>,
) -> Result<()> {
    let path = db_path.unwrap_or_else(default_db_path);
    let conn = open_db(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;

    let dur = parse_duration(since)?;
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let cutoff_micros = now_micros.saturating_sub(dur.as_micros() as u64);

    let mut stmt = if let Some(_sid) = sensor {
        conn.prepare_cached(
            "SELECT sensor_id, ts, scalar, counter, state FROM samples WHERE sensor_id = ?1 AND ts >= ?2 ORDER BY ts"
        )?
    } else {
        conn.prepare_cached(
            "SELECT sensor_id, ts, scalar, counter, state FROM samples WHERE ts >= ?1 ORDER BY ts",
        )?
    };

    let rows: Vec<_> = if let Some(sid) = sensor {
        stmt.query_map(rusqlite::params![sid, cutoff_micros as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(rusqlite::params![cutoff_micros as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };

    let mut out: Box<dyn std::io::Write> = match output {
        Some(p) => Box::new(
            std::fs::File::create(p)
                .with_context(|| format!("creating output file {}", p.display()))?,
        ),
        None => Box::new(std::io::stdout()),
    };

    match format {
        "csv" => {
            writeln!(out, "sensor_id,ts,scalar,counter,state")?;
            for row in rows {
                let (sid, ts, scalar, counter, state) = row;
                let scalar_s = scalar.map(|v| v.to_string()).unwrap_or_default();
                let counter_s = counter.map(|v| v.to_string()).unwrap_or_default();
                let state_s = state.as_deref().unwrap_or("");
                writeln!(
                    out,
                    "{},{},{},{},{}",
                    escape_csv(&sid),
                    ts,
                    scalar_s,
                    counter_s,
                    escape_csv(state_s),
                )?;
            }
        }
        "json" => {
            let mut arr = Vec::new();
            for row in rows {
                let (sid, ts, scalar, counter, state) = row;
                arr.push(serde_json::json!({
                    "sensor_id": sid,
                    "ts": ts,
                    "scalar": scalar,
                    "counter": counter,
                    "state": state,
                }));
            }
            writeln!(out, "{}", serde_json::to_string_pretty(&arr)?)?;
        }
        other => {
            return Err(anyhow::anyhow!(
                "unknown export format; expected csv or json format (got: {other})"
            ));
        }
    }

    Ok(())
}

fn escape_csv(s: &str) -> String {
    if s.contains(",") || s.contains("\"") || s.contains("\n") {
        let escaped = s.replace("\"", "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    use super::*;

    /// Create an in-memory (or temp-file) DB with the exact daemon schema.
    fn make_test_db() -> (NamedTempFile, Connection) {
        let f = NamedTempFile::new().unwrap();
        let conn = Connection::open(f.path()).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS samples (
                 sensor_id TEXT NOT NULL,
                 ts        INTEGER NOT NULL,
                 scalar    REAL,
                 counter   INTEGER,
                 state     TEXT,
                 PRIMARY KEY (sensor_id, ts)
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS samples_ts ON samples(ts);
             CREATE TABLE IF NOT EXISTS alert_events (
                 rule      TEXT NOT NULL,
                 ts        INTEGER NOT NULL,
                 kind      TEXT NOT NULL,
                 PRIMARY KEY (rule, ts)
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS alert_events_ts ON alert_events(ts);",
        )
        .unwrap();
        (f, conn)
    }

    fn now_micros() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
    }

    fn insert_row(conn: &Connection, sensor_id: &str, ts: u64, scalar: f64) {
        conn.execute(
            "INSERT INTO samples (sensor_id, ts, scalar) VALUES (?1, ?2, ?3)",
            rusqlite::params![sensor_id, ts as i64, scalar],
        )
        .unwrap();
    }

    #[test]
    fn stats_reports_row_count_and_span() {
        let (f, conn) = make_test_db();

        let base = now_micros();
        // 3 rows: 2 distinct sensors
        insert_row(&conn, "cpu.util", base - 3_000_000, 10.0);
        insert_row(&conn, "cpu.util", base - 2_000_000, 20.0);
        insert_row(&conn, "mem.used", base - 1_000_000, 512.0);

        drop(conn); // close so stats can re-open

        // Run stats; it should not error and should print the row count
        stats(Some(f.path().to_path_buf())).unwrap();

        // Verify counts directly via a fresh connection
        let conn2 = Connection::open(f.path()).unwrap();
        let row_count: u64 =
            conn2.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0)).unwrap();
        let distinct: u64 = conn2
            .query_row("SELECT COUNT(DISTINCT sensor_id) FROM samples", [], |r| r.get(0))
            .unwrap();
        let min_ts: i64 = conn2.query_row("SELECT MIN(ts) FROM samples", [], |r| r.get(0)).unwrap();
        let max_ts: i64 = conn2.query_row("SELECT MAX(ts) FROM samples", [], |r| r.get(0)).unwrap();

        assert_eq!(row_count, 3);
        assert_eq!(distinct, 2);
        assert_eq!(min_ts, (base - 3_000_000) as i64);
        assert_eq!(max_ts, (base - 1_000_000) as i64);
    }

    #[test]
    fn prune_older_than_flag_deletes_and_reports() {
        let (f, conn) = make_test_db();

        let now = now_micros();
        // 2 old rows (2h + 90m ago), 1 recent row (30m ago)
        let two_hours_ago = now.saturating_sub(2 * 3_600 * 1_000_000);
        let ninety_min_ago = now.saturating_sub(90 * 60 * 1_000_000);
        let thirty_min_ago = now.saturating_sub(30 * 60 * 1_000_000);

        insert_row(&conn, "cpu.util", two_hours_ago, 5.0);
        insert_row(&conn, "cpu.util", ninety_min_ago, 10.0);
        insert_row(&conn, "cpu.util", thirty_min_ago, 15.0);
        conn.execute(
            "INSERT INTO alert_events (rule, ts, kind) VALUES (?1, ?2, ?3)",
            rusqlite::params!["high-cpu", two_hours_ago as i64, "fired"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO alert_events (rule, ts, kind) VALUES (?1, ?2, ?3)",
            rusqlite::params!["high-cpu", thirty_min_ago as i64, "cleared"],
        )
        .unwrap();

        drop(conn);

        // Prune rows older than 1h — should remove the 2 old sample rows and 1 old alert event
        prune(Some(f.path().to_path_buf()), "1h", false).unwrap();

        let conn2 = Connection::open(f.path()).unwrap();
        let remaining: u64 =
            conn2.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 1, "expected only the 30-min-old sample row to survive");
        let remaining_events: u64 =
            conn2.query_row("SELECT COUNT(*) FROM alert_events", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining_events, 1, "expected only the 30-min-old alert event to survive");
    }

    #[test]
    fn parse_duration_returns_err_on_invalid() {
        assert!(parse_duration("0d").is_err());
        assert!(parse_duration("garbage").is_err());
        assert!(parse_duration("5x").is_err());
    }

    #[test]
    fn micros_to_utc_sub_100ms_formats_correctly() {
        // 2024-01-01T00:00:00 UTC = 1704067200 seconds since epoch.
        // Add 5_000 microseconds (5 ms) to test the 3-digit millisecond field.
        let base_micros: u64 = 1_704_067_200 * 1_000_000 + 5_000;
        let s = super::micros_to_utc(base_micros);
        assert!(s.ends_with(".005Z"), "expected sub-100ms timestamp to end with '.005Z', got: {s}");
    }

    #[test]
    fn missing_db_returns_clear_error() {
        let result = stats(Some(PathBuf::from("/nonexistent/path/history.db")));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not found") || msg.contains("history database"),
            "error should mention missing db, got: {msg}"
        );
    }
}
