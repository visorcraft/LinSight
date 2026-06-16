// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Prometheus text-format exporter on a configurable HTTP bind (Phase 7).
//!
//! Opt-in: set `LINSIGHT_PROM_BIND` to a `host:port` (e.g.
//! `127.0.0.1:9777`). Off by default — keeps the daemon's idle posture
//! socket-only.
//!
//! The exporter is a hand-rolled HTTP/1.0 server with a single hot path:
//! `GET /metrics`. Everything else returns 404. We avoid pulling a full
//! HTTP framework (`hyper`, `axum`, etc.) because the Prometheus contract
//! is one URL, one method, plain-text response — a few hundred LOC of
//! `TcpListener` + manual parse beats a transitive megabyte of deps.
//!
//! **Trust boundary:** the exporter binds to `127.0.0.1` by default and
//! performs no authentication. Only hosts that should see all sensor data
//! may reach the bind address. A concurrent connection cap (`MAX_PROM_CONNECTIONS`)
//! limits slow-loris exposure but is not a substitute for network-level access
//! control.
//!
//! On each scrape we synchronously call into the [`Scheduler`] to grab a
//! fresh sample for every registered sensor. We keep one fixed scrape
//! timestamp for consistency, but sample under smaller lock acquisitions so
//! GUI/CLI subscription work is not blocked behind one full scrape.

use std::fmt::Write as FmtWrite;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use linsight_core::{HardwareDeviceKey, Reading, Sample};
use linsight_plugin_sdk::SensorDescriptor;
use tracing::{info, warn};

use crate::hardware::HardwareRegistry;
use crate::scheduler::Scheduler;

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_PROM_CONNECTIONS: usize = 10;
const PROM_READ_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REQUEST_LINE_BYTES: usize = 2048;
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Spawn the exporter accept loop. Returns the shutdown flag — flip it to
/// `true` to stop the accept loop on the next poll interval. The runtime
/// keeps the flag alive until process exit so a flipping the flag during
/// graceful shutdown drains the thread cleanly.
pub fn spawn(
    bind: &str,
    scheduler: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
) -> Result<Arc<AtomicBool>> {
    let listener = TcpListener::bind(bind)
        .with_context(|| format!("binding Prometheus exporter on {bind}"))?;
    listener.set_nonblocking(true).context("setting Prometheus listener non-blocking")?;
    info!(bind, "Prometheus /metrics exporter listening");

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_thread = Arc::clone(&shutdown);
    thread::spawn(move || {
        accept_loop(listener, scheduler, registry, shutdown_thread);
    });
    Ok(shutdown)
}

/// Decrements the active-connection counter on drop so a slot is freed even
/// if the worker thread panics.
struct SlotGuard(Arc<AtomicUsize>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn accept_loop(
    listener: TcpListener,
    scheduler: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
    shutdown: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((s, _addr)) => {
                if let Err(e) = s.set_read_timeout(Some(PROM_READ_TIMEOUT)) {
                    warn!(error = ?e, "failed setting prom socket read timeout; dropping connection");
                    drop(s);
                    continue;
                }
                if active.load(Ordering::Relaxed) >= MAX_PROM_CONNECTIONS {
                    warn!("prom connection cap reached, dropping new connection");
                    let _ = s
                        .try_clone()
                        .and_then(|mut w| w.write_all(b"HTTP/1.0 503 Service Unavailable\r\n\r\n"));
                    drop(s);
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let sched = Arc::clone(&scheduler);
                let reg = Arc::clone(&registry);
                let conn_active = Arc::clone(&active);
                thread::spawn(move || {
                    // Release the slot on every exit path, including a panic in
                    // serve_one (e.g. a poisoned scheduler/registry lock), so a
                    // slot is never leaked and the cap can't wedge permanently.
                    let _slot = SlotGuard(conn_active);
                    if let Err(e) = serve_one(s, sched, reg) {
                        warn!(error = %e, "prom request failed");
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) => {
                warn!(error = ?e, "prom accept failed; backing off");
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
        }
    }
    info!("prom exporter accept loop exiting");
}

enum RequestReadError {
    TooLong,
    Timeout,
    Io(std::io::Error),
}

fn read_limited_line(
    reader: &mut BufReader<std::net::TcpStream>,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, RequestReadError> {
    let mut out = Vec::new();
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(RequestReadError::Timeout);
            }
            Err(e) => return Err(RequestReadError::Io(e)),
        };
        if chunk.is_empty() {
            return if out.is_empty() { Ok(None) } else { Ok(Some(out)) };
        }
        let mut consume = chunk.len();
        if let Some(pos) = chunk.iter().position(|b| *b == b'\n') {
            consume = pos + 1;
        }
        if out.len().saturating_add(consume) > max_bytes {
            return Err(RequestReadError::TooLong);
        }
        out.extend_from_slice(&chunk[..consume]);
        reader.consume(consume);
        if out.last().is_some_and(|b| *b == b'\n') {
            return Ok(Some(out));
        }
    }
}

fn write_static_response(writer: &mut std::net::TcpStream, response: &[u8]) {
    let _ = writer.write_all(response);
}

fn serve_one(
    stream: std::net::TcpStream,
    scheduler: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let request_line = match read_limited_line(&mut reader, MAX_REQUEST_LINE_BYTES) {
        Ok(Some(line)) => line,
        Ok(None) => return Ok(()),
        Err(RequestReadError::TooLong) => {
            write_static_response(&mut writer, b"HTTP/1.0 400 Bad Request\r\n\r\n");
            return Ok(());
        }
        Err(RequestReadError::Timeout) => {
            write_static_response(&mut writer, b"HTTP/1.0 408 Request Timeout\r\n\r\n");
            return Ok(());
        }
        Err(RequestReadError::Io(e)) => return Err(e.into()),
    };
    let mut header_bytes = 0usize;
    loop {
        let remaining = MAX_HEADER_BYTES.saturating_sub(header_bytes);
        if remaining == 0 {
            write_static_response(&mut writer, b"HTTP/1.0 400 Bad Request\r\n\r\n");
            return Ok(());
        }
        let header_line = match read_limited_line(&mut reader, remaining) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(RequestReadError::TooLong) => {
                write_static_response(&mut writer, b"HTTP/1.0 400 Bad Request\r\n\r\n");
                return Ok(());
            }
            Err(RequestReadError::Timeout) => {
                write_static_response(&mut writer, b"HTTP/1.0 408 Request Timeout\r\n\r\n");
                return Ok(());
            }
            Err(RequestReadError::Io(e)) => return Err(e.into()),
        };
        header_bytes += header_line.len();
        if header_line == b"\r\n" || header_line == b"\n" {
            break;
        }
    }
    let request_line = match std::str::from_utf8(&request_line) {
        Ok(line) => line,
        Err(_) => {
            write_static_response(&mut writer, b"HTTP/1.0 400 Bad Request\r\n\r\n");
            return Ok(());
        }
    };
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 || parts[0] != "GET" {
        writer.write_all(b"HTTP/1.0 405 Method Not Allowed\r\n\r\n")?;
        return Ok(());
    }
    let path = parts[1];
    if path != "/metrics" {
        writer.write_all(b"HTTP/1.0 404 Not Found\r\n\r\n")?;
        return Ok(());
    }

    let body = render_for_scrape(&scheduler, &registry);
    write!(
        writer,
        "HTTP/1.0 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )?;
    writer.write_all(body.as_bytes())?;
    Ok(())
}

/// One row of the per-scrape input: descriptor + (optional) sample + the
/// owning plugin id. The plugin_id is needed to resolve a sensor's
/// `device_key` via `HardwareRegistry::key_for` when the descriptor
/// doesn't carry one (older plugins predating ABI v4's `device_key` field
/// still ship through the same exporter path).
type ScrapeRow = (SensorDescriptor, Option<Sample>, String);
type ScrapeTarget = (SensorDescriptor, String);

/// Acquire the scheduler + registry locks, build the per-scrape input,
/// then hand off to the pure [`render`] helper. Split this way so tests
/// can drive `render` directly without a live Scheduler.
fn render_for_scrape(scheduler: &Mutex<Scheduler>, registry: &RwLock<HardwareRegistry>) -> String {
    let now =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
    // Keep the lock scope narrow: snapshot scrape targets up front, then
    // re-acquire only around each sample call so subscribe/unsubscribe work
    // can proceed between sensors.
    let targets: Vec<ScrapeTarget> = {
        let s = scheduler.lock().unwrap();
        s.scrape_targets()
    };
    let snapshot = collect_scrape_rows(scheduler, targets, now);
    let reg = registry.read().unwrap();
    render(&reg, &snapshot)
}

fn collect_scrape_rows(
    scheduler: &Mutex<Scheduler>,
    targets: Vec<ScrapeTarget>,
    now: u64,
) -> Vec<ScrapeRow> {
    collect_scrape_rows_with_after_sample(scheduler, targets, now, |_| {})
}

fn collect_scrape_rows_with_after_sample<F>(
    scheduler: &Mutex<Scheduler>,
    targets: Vec<ScrapeTarget>,
    now: u64,
    mut after_sample: F,
) -> Vec<ScrapeRow>
where
    F: FnMut(&Mutex<Scheduler>),
{
    let mut rows = Vec::with_capacity(targets.len());
    for (descriptor, plugin_id) in targets {
        // Sample through a cloned host Arc so a slow/hung plugin does not
        // hold the scheduler mutex during the scrape.
        let host = {
            let s = scheduler.lock().unwrap();
            s.host()
        };
        let sample = host.sample_to(&descriptor.id, now).ok();
        rows.push((descriptor, sample, plugin_id));
        after_sample(scheduler);
    }
    rows
}

/// Pure render: take a hardware registry snapshot + a vector of
/// `(descriptor, sample, plugin_id)` rows, emit the Prometheus text
/// exposition body. No locks, no IO — exercised directly by the unit
/// tests below.
fn render(registry: &HardwareRegistry, snapshot: &[ScrapeRow]) -> String {
    let mut out = String::new();
    out.push_str("# linsight Prometheus exporter\n");
    for (d, sample, plugin_id) in snapshot {
        let Some(sample) = sample else { continue };
        let metric = sanitize_metric_name(d.id.as_str());
        let unit = d.unit.symbol();

        // Resolve the device_key: prefer the descriptor's own value (set
        // by ABI v4 plugins via `SensorDescriptor::device_key`), else
        // fall back to a `(plugin_id, device_id)` lookup against the
        // registry. Sensors with no device binding emit unlabeled — we
        // do NOT add an empty `device_key=""` because Prometheus treats
        // `{}` and `{device_key=""}` as distinct time series.
        let device_key: Option<&HardwareDeviceKey> = d
            .device_key
            .as_ref()
            .or_else(|| d.device_id.as_ref().and_then(|did| registry.key_for(plugin_id, did)));

        match sample.reading {
            Reading::Scalar(v) => {
                let _ = writeln!(out, "# HELP {metric} {}", d.display_name);
                let _ = writeln!(out, "# TYPE {metric} gauge");
                emit_sample_line(&mut out, &metric, unit, device_key, &v.to_string());
            }
            Reading::Counter(v) => {
                let _ = writeln!(out, "# HELP {metric} {}", d.display_name);
                let _ = writeln!(out, "# TYPE {metric} counter");
                emit_sample_line(&mut out, &metric, unit, device_key, &v.to_string());
            }
            Reading::State(_) | Reading::Table(_) => {
                // State and Table are not Prometheus-native; skip for now.
            }
        }
    }

    // linsight_hardware_info: static metadata gauge so dashboards can
    // join per-sample metrics (which carry only `device_key`) against
    // model / vendor / nickname / plugin_id without re-fetching the
    // hardware catalogue over gRPC.
    let devices = registry.snapshot();
    let nicks = registry.nicknames_snapshot();
    out.push_str("# HELP linsight_hardware_info Static hardware metadata\n");
    out.push_str("# TYPE linsight_hardware_info gauge\n");
    for dev in devices {
        let nickname = nicks.get(dev.key.as_str()).cloned().unwrap_or_default();
        let vendor = dev.vendor.as_deref().unwrap_or("");
        let _ = write!(out, "linsight_hardware_info{{device_key=\"");
        push_escaped_label(&mut out, dev.key.as_str());
        let _ = write!(out, "\",category=\"{}\",model=\"", dev.category.as_str());
        push_escaped_label(&mut out, &dev.model);
        let _ = write!(out, "\",vendor=\"");
        push_escaped_label(&mut out, vendor);
        let _ = write!(out, "\",nickname=\"");
        push_escaped_label(&mut out, &nickname);
        let _ = write!(out, "\",plugin_id=\"");
        push_escaped_label(&mut out, &dev.plugin_id);
        let _ = writeln!(out, "\"}} 1");
    }
    out
}

/// Emit one Prometheus sample line, optionally including the
/// `device_key` label. Sensors without a bound device emit
/// `metric{unit="..."} value` (no empty `device_key=""`).
fn emit_sample_line(
    out: &mut String,
    metric: &str,
    unit: &str,
    device_key: Option<&HardwareDeviceKey>,
    value: &str,
) {
    let _ = write!(out, "{metric}{{unit=\"");
    push_escaped_label(out, unit);
    if let Some(k) = device_key {
        let _ = write!(out, "\",device_key=\"");
        push_escaped_label(out, k.as_str());
    }
    let _ = writeln!(out, "\"}} {value}");
}

/// Append an escaped Prometheus label value to `out`.
/// backslash → `\\`, double-quote → `\"`, newline → `\n`.
/// Other control chars (NUL, etc.) are dropped at the daemon's
/// validate_nickname seam, so we don't see them here.
fn push_escaped_label(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r"\n"),
            _ => out.push(c),
        }
    }
}

fn sanitize_metric_name(s: &str) -> String {
    // Prometheus allows [a-zA-Z_:][a-zA-Z0-9_:]*. Map dots and dashes to
    // underscores, drop anything else. Prepend `linsight_` to namespace.
    let mut out = String::from("linsight_");
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use linsight_core::{
        Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, Sample, SensorId,
        SensorKind, Unit,
    };

    use super::*;
    use crate::plugin_host::PluginHost;

    fn dev(key: &str, model: &str, vendor: Option<&str>) -> HardwareDevice {
        HardwareDevice {
            key: HardwareDeviceKey::try_new(key).unwrap(),
            category: HardwareCategory::Gpu,
            model: model.into(),
            vendor: vendor.map(|s| s.into()),
            location: None,
            // Plugins leave this empty; build() fills it in from the
            // manifest tuple. Mirrors hardware::tests::dev().
            plugin_id: String::new(),
            plugin_device_id: "gpu0".into(),
            sensor_ids: vec![],
        }
    }

    fn sensor(id: &str, device_key: Option<&str>, device_id: Option<&str>) -> SensorDescriptor {
        SensorDescriptor {
            id: SensorId::new(id),
            display_name: id.into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Gpu,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: device_id.map(|s| s.into()),
            device_key: device_key.map(|k| HardwareDeviceKey::try_new(k).unwrap()),
            tags: vec![],
        }
    }

    fn sample_scalar(id: &str, v: f64) -> Sample {
        Sample { sensor: SensorId::new(id), ts_micros: 1_000_000, reading: Reading::Scalar(v) }
    }

    fn test_scheduler() -> Arc<Mutex<Scheduler>> {
        Arc::new(Mutex::new(Scheduler::new(PluginHost::with_builtins())))
    }

    fn connected_stream_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    fn run_serve_one_with_request(request: &[u8]) -> String {
        let (mut client, server) = connected_stream_pair();
        server.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
        let scheduler = test_scheduler();
        let registry = Arc::new(RwLock::new(test_registry()));
        let handle = thread::spawn(move || serve_one(server, scheduler, registry).unwrap());
        client.write_all(request).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        handle.join().unwrap();
        response
    }

    /// One-device, one-sensor fixture used by most tests. The sensor
    /// carries an explicit `device_key`, exercising the descriptor's
    /// own field (the v4 plugin path) rather than the `(plugin_id,
    /// device_id) -> key_for()` fallback.
    fn test_registry() -> HardwareRegistry {
        let d = [dev("pci:0000:06:00.0", "Intel Arc B-series", Some("Intel Corporation"))];
        let s = [sensor("xe.gpu0.util", Some("pci:0000:06:00.0"), Some("gpu0"))];
        HardwareRegistry::build(&[("com.visorcraft.linsight.xe", &d, &s)], HashMap::new())
    }

    #[test]
    fn exporter_emits_device_key_label_when_set() {
        let reg = test_registry();
        let rows = vec![(
            sensor("xe.gpu0.util", Some("pci:0000:06:00.0"), Some("gpu0")),
            Some(sample_scalar("xe.gpu0.util", 27.6)),
            "com.visorcraft.linsight.xe".to_owned(),
        )];
        let body = render(&reg, &rows);
        assert!(
            body.contains(r#"device_key="pci:0000:06:00.0""#),
            "expected device_key label on per-sample line; body was:\n{body}",
        );
        // And the value rendered with the expected metric name.
        assert!(
            body.contains("linsight_xe_gpu0_util{"),
            "expected metric-name line present; body was:\n{body}",
        );
    }

    #[test]
    fn exporter_emits_hardware_info_block() {
        let reg = test_registry();
        let body = render(&reg, &[]);
        assert!(body.contains("# HELP linsight_hardware_info"));
        assert!(body.contains("# TYPE linsight_hardware_info gauge"));
        assert!(body.contains(r#"device_key="pci:0000:06:00.0""#));
        assert!(body.contains(r#"model="Intel Arc B-series""#));
        assert!(body.contains(r#"vendor="Intel Corporation""#));
        assert!(body.contains(r#"category="gpu""#));
        assert!(body.contains(r#"plugin_id="com.visorcraft.linsight.xe""#));
        // Empty nickname when none set — Prometheus tolerates the empty
        // value on info-metric labels (this is NOT the per-sample
        // empty-device_key concern; this is a *different* label).
        assert!(body.contains(r#"nickname="""#));
    }

    #[test]
    fn exporter_escapes_special_chars_in_labels() {
        let key = HardwareDeviceKey::try_new("pci:0000:06:00.0").unwrap();
        // validate_nickname rejects raw control chars, so we drive the
        // map directly via the build()-time nickname path: a hand-
        // edited hardware.json could smuggle a backslash / quote /
        // newline past validation. This is the only path that
        // exercises every branch of `escape_label`.
        let mut raw_nicks = HashMap::new();
        raw_nicks.insert(key.as_str().to_owned(), "a\"b\\c\nd".to_owned());
        let d = [dev("pci:0000:06:00.0", "Intel Arc B-series", Some("Intel Corporation"))];
        let reg = HardwareRegistry::build(&[("com.visorcraft.linsight.xe", &d, &[])], raw_nicks);
        let body = render(&reg, &[]);
        assert!(
            body.contains(r#"nickname="a\"b\\c\nd""#),
            "label escape failed; body had:\n{body}",
        );
    }

    #[test]
    fn exporter_renders_no_empty_device_key_label() {
        // A sensor with neither `device_key` nor a registry-resolvable
        // (plugin_id, device_id) must NOT emit `device_key=""` — that
        // creates a distinct Prometheus time series from the
        // unlabeled form and breaks PromQL joins.
        let reg = HardwareRegistry::build(&[], HashMap::new());
        let rows = vec![(
            sensor("mem.used", None, None),
            Some(sample_scalar("mem.used", 1.5)),
            "com.visorcraft.linsight.mem".to_owned(),
        )];
        let body = render(&reg, &rows);
        assert!(
            body.contains("linsight_mem_used{"),
            "expected the unlabeled metric line; body was:\n{body}",
        );
        // The per-sample metric line must not carry an empty
        // device_key. The hardware_info block doesn't emit because
        // there are no registered devices, so no `device_key=""` can
        // appear from there either; we assert globally.
        for line in body.lines() {
            assert!(
                !line.contains(r#"device_key="""#),
                "no metric line should have an empty device_key label; offending line:\n{line}",
            );
        }
    }

    #[test]
    fn exporter_falls_back_to_registry_lookup_when_descriptor_missing_device_key() {
        // ABI v3-style plugin: the descriptor carries `device_id` but
        // not `device_key`. The exporter must still label the sample
        // by resolving via `HardwareRegistry::key_for`.
        let reg = test_registry();
        let rows = vec![(
            // device_key=None forces the fallback path; device_id="gpu0"
            // matches the fixture's plugin_device_id.
            sensor("xe.gpu0.util", None, Some("gpu0")),
            Some(sample_scalar("xe.gpu0.util", 50.0)),
            "com.visorcraft.linsight.xe".to_owned(),
        )];
        let body = render(&reg, &rows);
        assert!(
            body.contains(r#"device_key="pci:0000:06:00.0""#),
            "expected device_key resolved via key_for; body was:\n{body}",
        );
    }

    #[test]
    fn exporter_rejects_oversized_request_line() {
        let path = "x".repeat(MAX_REQUEST_LINE_BYTES + 64);
        let req = format!("GET /{path} HTTP/1.0\r\n\r\n");
        let response = run_serve_one_with_request(req.as_bytes());
        assert!(
            response.starts_with("HTTP/1.0 400 Bad Request"),
            "expected 400 for oversized request line, got:\n{response}",
        );
    }

    #[test]
    fn exporter_rejects_oversized_headers() {
        let huge = "y".repeat(MAX_HEADER_BYTES + 64);
        let req = format!("GET /metrics HTTP/1.0\r\nX-Huge: {huge}\r\n\r\n");
        let response = run_serve_one_with_request(req.as_bytes());
        assert!(
            response.starts_with("HTTP/1.0 400 Bad Request"),
            "expected 400 for oversized headers, got:\n{response}",
        );
    }

    #[test]
    fn exporter_times_out_on_slow_header() {
        let (mut client, server) = connected_stream_pair();
        server.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let scheduler = test_scheduler();
        let registry = Arc::new(RwLock::new(test_registry()));
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = done_tx.send(serve_one(server, scheduler, registry));
        });
        client.write_all(b"GET /metrics HTTP/1.0\r\nX-Slow: partial").unwrap();

        let start = Instant::now();
        let result = done_rx.recv_timeout(Duration::from_secs(1)).expect("serve_one should return");
        assert!(result.is_ok(), "timeout path should be handled without bubbling an error");
        assert!(
            start.elapsed() < Duration::from_millis(700),
            "slow header should return promptly, elapsed={:?}",
            start.elapsed(),
        );
        client.shutdown(Shutdown::Write).unwrap();
        client.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let mut response = String::new();
        let _ = client.read_to_string(&mut response);
        assert!(
            response.is_empty() || response.starts_with("HTTP/1.0 408 Request Timeout"),
            "expected empty close or 408 timeout response; got:\n{response}",
        );
    }

    #[test]
    fn scrape_sampling_releases_scheduler_lock_between_sensors() {
        let scheduler = Mutex::new(Scheduler::new(PluginHost::with_builtins()));
        let targets = {
            let s = scheduler.lock().unwrap();
            s.scrape_targets()
        };
        assert!(targets.len() >= 2, "builtins should expose at least two scrape targets");
        let targets: Vec<ScrapeTarget> = targets.into_iter().take(2).collect();
        let mut lock_reacquired = 0usize;
        let rows = collect_scrape_rows_with_after_sample(&scheduler, targets, 42, |sched| {
            if let Ok(_guard) = sched.try_lock() {
                lock_reacquired += 1;
            }
        });
        assert_eq!(rows.len(), 2);
        assert_eq!(lock_reacquired, 2, "scheduler mutex should be released after each sample",);
    }
}
