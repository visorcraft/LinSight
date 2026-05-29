// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! SQLite-backed history store (Phase 7).
//!
//! Opt-in: set `LINSIGHT_HISTORY=1` (or pass `--history`) to enable. The
//! daemon then writes every sample to `$XDG_DATA_HOME/linsight/history.db`
//! via a background thread that batches writes on a 1-second window so the
//! sample loop never blocks on disk I/O.
//!
//! Schema (single table, idempotent migration):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS samples (
//!     sensor_id TEXT NOT NULL,
//!     ts        INTEGER NOT NULL,   -- microseconds since epoch
//!     scalar    REAL,
//!     counter   INTEGER,
//!     state     TEXT,
//!     PRIMARY KEY (sensor_id, ts)
//! ) WITHOUT ROWID;
//! CREATE INDEX IF NOT EXISTS samples_ts ON samples(ts);
//! ```

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use linsight_core::{Reading, Sample, SensorId};
use rusqlite::{Connection, OpenFlags, params};
use tracing::{error, info, warn};

const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const MAX_BATCH: usize = 4096;

/// Async-write handle. Cloneable so multiple producers can `record(sample)`
/// without coordinating around a mutex. Dropping the last clone signals the
/// writer thread to flush + exit.
#[derive(Clone)]
pub struct HistoryWriter {
    tx: Sender<Sample>,
}

impl HistoryWriter {
    pub fn record(&self, sample: Sample) {
        // A full channel means the writer is way behind; dropping the
        // sample is preferable to blocking the sample loop. We log so the
        // operator sees pressure.
        if let Err(e) = self.tx.send(sample) {
            warn!(error = ?e, "history channel send failed");
        }
    }
}

/// Open the history database (creating it if needed) and spawn the writer
/// thread. Returns a producer handle for the scheduler plus a join handle
/// the runtime should keep so it can detect a thread crash on shutdown.
/// The writer exits cleanly when every clone of the producer handle is
/// dropped.
pub fn spawn(db_path: PathBuf) -> Result<(HistoryWriter, thread::JoinHandle<()>)> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let conn = Connection::open(&db_path).with_context(|| format!("open {}", db_path.display()))?;
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
         CREATE INDEX IF NOT EXISTS samples_ts ON samples(ts);",
    )
    .context("init schema")?;

    let (tx, rx) = channel::<Sample>();
    let handle = thread::spawn(move || {
        if let Err(e) = run_writer(conn, rx) {
            error!(error = ?e, "history writer thread crashed");
        }
    });
    info!(db = %db_path.display(), "history writer ready");
    Ok((HistoryWriter { tx }, handle))
}

fn run_writer(mut conn: Connection, rx: Receiver<Sample>) -> Result<()> {
    let mut pending: Vec<Sample> = Vec::with_capacity(MAX_BATCH);
    let mut last_flush = Instant::now();
    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(s) => pending.push(s),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // All producers gone — flush remaining + exit. Errors on this
            // final flush are surfaced so an operator notices if the last
            // batch was lost to e.g. disk-full.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if !pending.is_empty()
                    && let Err(e) = flush(&mut conn, &pending)
                {
                    warn!(error = ?e, count = pending.len(), "final history flush failed; samples lost");
                }
                return Ok(());
            }
        }
        if pending.len() >= MAX_BATCH
            || (!pending.is_empty() && last_flush.elapsed() >= FLUSH_INTERVAL)
        {
            if let Err(e) = flush(&mut conn, &pending) {
                warn!(error = ?e, "history flush failed");
            }
            pending.clear();
            last_flush = Instant::now();
        }
    }
}

fn flush(conn: &mut Connection, batch: &[Sample]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO samples (sensor_id, ts, scalar, counter, state)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for s in batch {
            let (scalar, counter, state): (Option<f64>, Option<i64>, Option<&str>) =
                match &s.reading {
                    Reading::Scalar(v) => (Some(*v), None, None),
                    Reading::Counter(v) => (None, Some(*v as i64), None),
                    Reading::State(v) => (None, None, Some(v.as_str())),
                    // Tables are intentionally skipped — Phase 7 only persists
                    // scalar / counter / state. Per-process GPU tables stay
                    // ephemeral.
                    Reading::Table(_) => continue,
                };
            stmt.execute(params![s.sensor.as_str(), s.ts_micros as i64, scalar, counter, state])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Query historical samples for a sensor within a time window.
/// When the result set exceeds `max_points`, rows are downsampled by
/// taking every Nth row (stride = total_rows / max_points) so the
/// returned points are spread evenly across the time range.
/// Opens a read-only connection to avoid blocking the writer thread.
pub fn query(
    db_path: &Path,
    sensor: &str,
    since_micros: i64,
    until_micros: i64,
    max_points: Option<u32>,
) -> Result<Vec<Sample>> {
    let sensor_id = SensorId::try_new(sensor)
        .with_context(|| format!("invalid sensor id for query: {sensor:?}"))?;
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {} for query", db_path.display()))?;
    let limit = max_points.unwrap_or(500).min(10_000) as i64;

    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM samples WHERE sensor_id = ?1 AND ts >= ?2 AND ts <= ?3",
            params![sensor, since_micros, until_micros],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let stride = if total > limit { (total as usize / limit as usize).max(1) } else { 1 };

    let mut stmt = conn.prepare_cached(
        "SELECT ts, scalar, counter, state FROM samples
         WHERE sensor_id = ?1 AND ts >= ?2 AND ts <= ?3
         ORDER BY ts",
    )?;
    let rows = stmt.query_map(params![sensor, since_micros, until_micros], |row| {
        let ts: i64 = row.get(0)?;
        let scalar: Option<f64> = row.get(1)?;
        let counter: Option<i64> = row.get(2)?;
        let state: Option<String> = row.get(3)?;
        let reading = match (scalar, counter, state) {
            (Some(v), _, _) => Reading::Scalar(v),
            (_, Some(v), _) => Reading::Counter(v as u64),
            (_, _, Some(v)) => Reading::State(v),
            (None, None, None) => Reading::Scalar(0.0),
        };
        Ok(linsight_core::Sample { sensor: sensor_id.clone(), ts_micros: ts as u64, reading })
    })?;

    let mut samples = Vec::new();
    for (i, row) in rows.enumerate() {
        if i % stride == 0 {
            samples.push(row?);
        } else {
            let _ = row?;
        }
        if samples.len() as i64 >= limit {
            break;
        }
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use linsight_core::SensorId;

    use super::*;

    #[test]
    fn write_and_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("h.db");
        let (writer, handle) = spawn(db.clone()).unwrap();
        for ts in 0..10 {
            writer.record(Sample {
                sensor: SensorId::new("cpu.util"),
                ts_micros: ts,
                reading: Reading::Scalar(ts as f64),
            });
        }
        // Dropping the writer signals shutdown; joining the handle waits
        // for the final flush deterministically (no time-based race).
        drop(writer);
        handle.join().expect("writer thread panicked");
        let conn = Connection::open(&db).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 10);
    }
}
