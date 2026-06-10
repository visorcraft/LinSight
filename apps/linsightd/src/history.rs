// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! SQLite-backed history store (Phase 7).
//!
//! Opt-in: set `LINSIGHT_HISTORY=1` to enable (or use the systemd user unit at
//! `packaging/systemd/linsight.service` for always-on mode). The
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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use linsight_core::{Reading, Sample, SensorId, parse_duration_dhm};
use rusqlite::{Connection, OpenFlags, params};
use tracing::{error, info, warn};

const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const MAX_BATCH: usize = 4096;
const QUEUE_CAPACITY: usize = 16_384;

/// Async-write handle. Cloneable so multiple producers can `record(sample)`
/// without coordinating around a mutex. Dropping the last clone signals the
/// writer thread to flush + exit.
#[derive(Clone)]
pub struct HistoryWriter {
    tx: SyncSender<Sample>,
    dropped: Arc<AtomicU64>,
}

impl HistoryWriter {
    pub fn record(&self, sample: Sample) {
        // The scheduler hot path must never block on history I/O pressure.
        // Under sustained backlog we intentionally drop samples and let the
        // writer thread report aggregated pressure.
        match self.tx.try_send(sample) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {
                warn!("history channel send failed; writer disconnected");
            }
        }
    }
}

/// Delete all rows with `ts < cutoff_micros`. Returns the number of rows removed.
pub(crate) fn prune_older_than(conn: &Connection, cutoff_micros: u64) -> Result<usize> {
    let removed =
        conn.execute("DELETE FROM samples WHERE ts < ?1", params![cutoff_micros as i64])?;
    Ok(removed)
}

/// Open the history database (creating it if needed) and spawn the writer
/// thread. Returns a producer handle for the scheduler plus a join handle
/// the runtime should keep so it can detect a thread crash on shutdown.
/// The writer exits cleanly when every clone of the producer handle is
/// dropped.
///
/// `retention` controls automatic pruning inside the writer thread.
/// `None` means keep rows forever; `Some(d)` prunes rows older than `d`
/// at most once per hour (with a short initial delay so the first prune
/// happens shortly after startup rather than waiting a full hour).
pub fn spawn(
    db_path: PathBuf,
    retention: Option<Duration>,
) -> Result<(HistoryWriter, thread::JoinHandle<()>)> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
        // Make the data dir owner-only: this closes the brief window in which
        // the db and its SQLite-created `-wal`/`-shm` sidecars exist at the
        // umask default before the chmods below, since no other user can
        // traverse into a 0700 directory to open them.
        std::fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .with_context(|| format!("chmod dir {}", parent.display()))?;
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

    // Restrict the db AND the `-wal`/`-shm` sidecars (created by WAL mode in
    // the schema writes above) to owner-only — they hold the same sample data.
    // chmod-after rather than a umask clamp: umask is process-global and would
    // corrupt file modes in concurrent threads (e.g. the test harness). `""`
    // covers the db file itself.
    for suffix in ["", "-wal", "-shm"] {
        let mut p = db_path.clone().into_os_string();
        p.push(suffix);
        let p = PathBuf::from(p);
        if p.exists() {
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o600))
                .with_context(|| format!("chmod {}", p.display()))?;
        }
    }

    let dropped = Arc::new(AtomicU64::new(0));
    let writer_dropped = Arc::clone(&dropped);
    let (tx, rx) = sync_channel::<Sample>(QUEUE_CAPACITY);
    let handle = thread::spawn(move || {
        if let Err(e) = run_writer(conn, rx, writer_dropped, retention) {
            error!(error = ?e, "history writer thread crashed");
        }
    });
    info!(db = %db_path.display(), "history writer ready");
    Ok((HistoryWriter { tx, dropped }, handle))
}

pub(crate) const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);
// Prime last_prune so the first prune fires after this delay rather than
// waiting a full hour.
pub(crate) const PRUNE_INITIAL_DELAY: Duration = Duration::from_secs(300);

/// Check whether a prune is due and, if so, delete stale rows.
///
/// `now` is passed in so tests can drive the gate with synthetic instants
/// without sleeping. `last_prune` is updated unconditionally after an attempt
/// so a failed prune backs off for a full interval rather than hammering the
/// disk on a persistent error.
pub(crate) fn maybe_prune(
    conn: &Connection,
    retention: Option<Duration>,
    last_prune: &mut Instant,
    now: Instant,
) {
    let Some(retention) = retention else { return };
    if now.duration_since(*last_prune) < PRUNE_INTERVAL {
        return;
    }
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let cutoff = now_micros.saturating_sub(retention.as_micros() as u64);
    match prune_older_than(conn, cutoff) {
        Ok(n) if n > 0 => info!(removed = n, "pruned history rows past retention window"),
        Ok(_) => {}
        Err(e) => warn!(error = ?e, "history prune failed"),
    }
    // Update unconditionally so a transient error backs off rather than retrying every tick.
    *last_prune = now;
}

fn run_writer(
    mut conn: Connection,
    rx: Receiver<Sample>,
    dropped: Arc<AtomicU64>,
    retention: Option<Duration>,
) -> Result<()> {
    let mut pending: Vec<Sample> = Vec::with_capacity(MAX_BATCH);
    let mut last_flush = Instant::now();
    let mut last_prune = Instant::now()
        .checked_sub(PRUNE_INTERVAL - PRUNE_INITIAL_DELAY)
        .unwrap_or_else(Instant::now);
    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(s) => pending.push(s),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // All producers gone — flush remaining + exit. Errors on this
            // final flush are surfaced so an operator notices if the last
            // batch was lost to e.g. disk-full.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                log_dropped_pressure(&dropped);
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
            log_dropped_pressure(&dropped);
            if let Err(e) = flush(&mut conn, &pending) {
                warn!(error = ?e, "history flush failed");
            }
            pending.clear();
            last_flush = Instant::now();
        }
        // Evaluate the prune gate on every iteration so an idle daemon (no
        // samples → no flushes) still rotates stale rows on schedule.
        maybe_prune(&conn, retention, &mut last_prune, Instant::now());
    }
}

fn log_dropped_pressure(dropped: &AtomicU64) {
    let count = dropped.swap(0, Ordering::Relaxed);
    if count > 0 {
        warn!(dropped = count, "history queue pressure; dropped samples");
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
/// The query downsampling runs in SQLite with window bucketing so Rust
/// decodes only rows that can be returned.
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
    let limit = max_points.unwrap_or(500).clamp(1, 10_000) as i64;

    let mut stmt = conn.prepare_cached(
        "WITH ranked AS (
            SELECT
                ts,
                scalar,
                counter,
                state,
                row_number() OVER (ORDER BY ts) AS rn,
                count(*) OVER () AS total
            FROM samples
            WHERE sensor_id = ?1 AND ts >= ?2 AND ts <= ?3
         ),
         bucketed AS (
            SELECT
                ts,
                scalar,
                counter,
                state,
                rn,
                ((rn - 1) * ?4) / total AS bucket
            FROM ranked
         ),
         chosen AS (
            SELECT
                ts,
                scalar,
                counter,
                state,
                row_number() OVER (PARTITION BY bucket ORDER BY rn) AS bucket_rank
            FROM bucketed
         )
         SELECT ts, scalar, counter, state
         FROM chosen
         WHERE bucket_rank = 1
         ORDER BY ts
         LIMIT ?4",
    )?;
    let rows =
        stmt.query_map(params![sensor_id.as_str(), since_micros, until_micros, limit], |row| {
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
    for row in rows {
        samples.push(row?);
    }
    Ok(samples)
}

/// Parse a retention string into a `Duration`.
///
/// Accepts integer values with a `d` (days), `h` (hours), or `m` (minutes)
/// suffix. A bare `"0"` returns `None` (keep forever). Any other input that
/// doesn't match the grammar returns `None`.
///
/// Grammar is shared with `linsight_core::parse_duration_dhm`; this wrapper
/// only adds the `"0"` sentinel (which the core parser rejects as zero-value).
pub(crate) fn parse_retention(s: &str) -> Option<Duration> {
    let trimmed = s.trim();
    if trimmed == "0" {
        return None;
    }
    parse_duration_dhm(trimmed)
}

/// Read `LINSIGHT_HISTORY_RETENTION` and return the parsed retention window
/// (unset → 30d default; `"0"` → `None` keep-forever; unparseable → warn + 30d default).
pub(crate) fn retention_from_env(raw: Option<&str>) -> Option<Duration> {
    const DEFAULT: Duration = Duration::from_secs(30 * 86_400);
    match raw {
        None => Some(DEFAULT),
        Some(s) => {
            if let Some(d) = parse_retention(s) {
                Some(d)
            } else if s.trim() == "0" {
                None
            } else {
                warn!(value = %s, "LINSIGHT_HISTORY_RETENTION unparseable; using default 30d");
                Some(DEFAULT)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{TryRecvError, channel, sync_channel};
    use std::time::Duration;

    use linsight_core::SensorId;

    use super::*;

    #[test]
    fn retention_zero_sentinel_is_none() {
        assert_eq!(parse_retention("0"), None);
    }

    #[test]
    fn retention_env_default_is_30_days() {
        assert_eq!(retention_from_env(None), Some(Duration::from_secs(30 * 86_400)));
    }

    #[test]
    fn retention_env_zero_is_keep_forever() {
        assert_eq!(retention_from_env(Some("0")), None);
    }

    #[test]
    fn retention_env_garbage_falls_back_to_default() {
        assert_eq!(retention_from_env(Some("garbage")), Some(Duration::from_secs(30 * 86_400)));
    }

    #[test]
    fn write_and_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("h.db");
        let (writer, handle) = spawn(db.clone(), None).unwrap();
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

    #[test]
    fn record_drops_when_queue_is_full_without_blocking() {
        let (tx, rx) = sync_channel::<Sample>(1);
        let writer = HistoryWriter { tx, dropped: Arc::new(AtomicU64::new(0)) };
        let sensor = SensorId::new("cpu.util");

        writer.record(Sample {
            sensor: sensor.clone(),
            ts_micros: 1,
            reading: Reading::Scalar(1.0),
        });

        let (done_tx, done_rx) = channel::<()>();
        let writer2 = writer.clone();
        let sensor2 = sensor.clone();
        let thread = thread::spawn(move || {
            writer2.record(Sample { sensor: sensor2, ts_micros: 2, reading: Reading::Scalar(2.0) });
            done_tx.send(()).unwrap();
        });

        done_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("record blocked while queue was full");
        thread.join().expect("pressure thread panicked");

        assert_eq!(writer.dropped.load(Ordering::Relaxed), 1);
        assert!(rx.try_recv().is_ok());
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn prune_removes_rows_older_than_cutoff() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("h.db");
        let (writer, handle) = spawn(db.clone(), None).unwrap();
        writer.record(Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 1_000,
            reading: Reading::Scalar(1.0),
        });
        writer.record(Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 2_000_000,
            reading: Reading::Scalar(2.0),
        });
        drop(writer);
        handle.join().expect("writer thread panicked");

        let conn = Connection::open(&db).unwrap();
        let removed = prune_older_than(&conn, 1_500_000).unwrap();
        assert_eq!(removed, 1);

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
        let ts: i64 = conn.query_row("SELECT ts FROM samples", [], |r| r.get(0)).unwrap();
        assert_eq!(ts, 2_000_000);
    }

    // ---------------------------------------------------------------------------
    // maybe_prune gate tests — driven with synthetic Instants, no sleeping.
    // ---------------------------------------------------------------------------

    fn open_test_db(dir: &tempfile::TempDir) -> (Connection, std::path::PathBuf) {
        let db = dir.path().join("h.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
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
        .unwrap();
        (conn, db)
    }

    fn row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0)).unwrap()
    }

    /// Insert a row at `ts_micros` directly so tests can control age without
    /// going through the writer thread.
    fn insert_row(conn: &Connection, ts_micros: i64) {
        conn.execute(
            "INSERT INTO samples (sensor_id, ts, scalar) VALUES ('cpu.util', ?1, 1.0)",
            rusqlite::params![ts_micros],
        )
        .unwrap();
    }

    #[test]
    fn maybe_prune_skips_when_retention_is_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let (conn, _) = open_test_db(&dir);
        insert_row(&conn, 1_000);

        // Way past PRUNE_INTERVAL — but retention=None means prune never fires.
        let long_ago = Instant::now().checked_sub(PRUNE_INTERVAL * 2).unwrap();
        let mut last_prune = long_ago;
        maybe_prune(&conn, None, &mut last_prune, Instant::now());

        // last_prune must NOT advance (gate never ran).
        assert!(last_prune.duration_since(long_ago) < Duration::from_millis(1));
        assert_eq!(row_count(&conn), 1);
    }

    #[test]
    fn maybe_prune_skips_when_interval_not_elapsed() {
        let dir = tempfile::TempDir::new().unwrap();
        let (conn, _) = open_test_db(&dir);
        insert_row(&conn, 1_000);

        // last_prune is "just now" — interval hasn't elapsed.
        let mut last_prune = Instant::now();
        let retention = Some(Duration::from_secs(1));
        maybe_prune(&conn, retention, &mut last_prune, Instant::now());

        assert_eq!(row_count(&conn), 1, "should not prune before interval elapses");
    }

    /// Core regression test: an old row is pruned on an idle daemon (no samples
    /// flowing, pending always empty) when the interval has elapsed.
    ///
    /// We bypass the writer loop entirely and drive `maybe_prune` directly with
    /// a synthetic `now` that is `PRUNE_INTERVAL` past `last_prune`, which
    /// exercises the full gate logic (retention check, interval check, SQL
    /// delete, last_prune update) without any wall-clock sleeping.
    #[test]
    fn maybe_prune_fires_on_idle_daemon_after_interval() {
        let dir = tempfile::TempDir::new().unwrap();
        let (conn, _) = open_test_db(&dir);

        // Insert one very old row (ts=1 µs since epoch — always before any
        // real retention window).
        insert_row(&conn, 1);
        // Insert one fresh row at "now" (a large epoch value so it survives).
        let now_micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i64;
        insert_row(&conn, now_micros);
        assert_eq!(row_count(&conn), 2);

        // Simulate: last_prune is PRUNE_INTERVAL in the past.
        let last_prune_base = Instant::now().checked_sub(PRUNE_INTERVAL).unwrap();
        let mut last_prune = last_prune_base;

        // Short retention — 1 second is more than enough to catch the row at ts=1.
        let retention = Some(Duration::from_secs(1));

        // `now` is the current instant (> last_prune by PRUNE_INTERVAL → gate opens).
        maybe_prune(&conn, retention, &mut last_prune, Instant::now());

        // Old row deleted, fresh row kept.
        assert_eq!(row_count(&conn), 1, "old row should have been pruned");

        // last_prune advanced (so the next call in < PRUNE_INTERVAL is blocked).
        assert!(last_prune > last_prune_base, "last_prune must advance after a prune attempt");
    }

    #[test]
    fn maybe_prune_last_prune_advances_even_on_empty_result() {
        let dir = tempfile::TempDir::new().unwrap();
        let (conn, _) = open_test_db(&dir);
        // No rows — prune will delete 0 rows but gate still fires.
        let last_prune_base = Instant::now().checked_sub(PRUNE_INTERVAL).unwrap();
        let mut last_prune = last_prune_base;
        let retention = Some(Duration::from_secs(1));
        maybe_prune(&conn, retention, &mut last_prune, Instant::now());
        assert!(last_prune > last_prune_base, "last_prune must advance even when 0 rows deleted");
    }

    #[test]
    fn query_downsamples_with_bounded_even_spread() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("h.db");
        let (writer, handle) = spawn(db.clone(), None).unwrap();

        for ts in 0..103_u64 {
            writer.record(Sample {
                sensor: SensorId::new("cpu.util"),
                ts_micros: ts,
                reading: Reading::Scalar(ts as f64),
            });
        }
        drop(writer);
        handle.join().expect("writer thread panicked");

        let samples = query(&db, "cpu.util", 0, 102, Some(10)).unwrap();
        assert_eq!(samples.len(), 10);

        let ts: Vec<u64> = samples.iter().map(|s| s.ts_micros).collect();
        assert!(ts.windows(2).all(|w| w[1] > w[0]));
        let gaps: Vec<u64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
        let min_gap = *gaps.iter().min().unwrap();
        let max_gap = *gaps.iter().max().unwrap();
        assert!(max_gap - min_gap <= 1, "downsample gaps not evenly spread: {:?}", gaps);
    }
}
