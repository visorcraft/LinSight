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
//! On each scrape we synchronously call into the [`Scheduler`] to grab a
//! fresh sample for every registered sensor under a SINGLE lock acquisition
//! so the whole scrape represents one consistent snapshot — Prometheus
//! requires every series in a single response to share a timestamp.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
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

fn accept_loop(
    listener: TcpListener,
    scheduler: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((s, _addr)) => {
                let sched = Arc::clone(&scheduler);
                let reg = Arc::clone(&registry);
                thread::spawn(move || {
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

fn serve_one(
    stream: std::net::TcpStream,
    scheduler: Arc<Mutex<Scheduler>>,
    registry: Arc<RwLock<HardwareRegistry>>,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // Drain headers (we don't care; skipping them keeps the parser one-shot).
    let mut hdr = String::new();
    loop {
        hdr.clear();
        let n = reader.read_line(&mut hdr)?;
        if n == 0 || hdr == "\r\n" || hdr == "\n" {
            break;
        }
    }
    let mut writer = stream;
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
    let header = format!(
        "HTTP/1.0 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    writer.write_all(header.as_bytes())?;
    writer.write_all(body.as_bytes())?;
    Ok(())
}

/// One row of the per-scrape input: descriptor + (optional) sample + the
/// owning plugin id. The plugin_id is needed to resolve a sensor's
/// `device_key` via `HardwareRegistry::key_for` when the descriptor
/// doesn't carry one (older plugins predating ABI v4's `device_key` field
/// still ship through the same exporter path).
type ScrapeRow = (SensorDescriptor, Option<Sample>, String);

/// Acquire the scheduler + registry locks, build the per-scrape input,
/// then hand off to the pure [`render`] helper. Split this way so tests
/// can drive `render` directly without a live Scheduler.
fn render_for_scrape(scheduler: &Mutex<Scheduler>, registry: &RwLock<HardwareRegistry>) -> String {
    let now =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
    // Single-snapshot scrape: hold the scheduler lock for the WHOLE pass so
    // every series in this response is from one consistent instant. Before
    // this, render() re-acquired the lock per sensor and interleaved with
    // the pump-thread tick(), producing scrapes whose timestamps spread
    // over the scrape duration.
    let snapshot: Vec<ScrapeRow> = {
        let s = scheduler.lock().unwrap();
        s.descriptors()
            .cloned()
            .map(|d| {
                let sample = s.sample_now(&d.id, now);
                let plugin_id = s.plugin_id_for(&d.id).unwrap_or("unknown").to_owned();
                (d, sample, plugin_id)
            })
            .collect()
    };
    let reg = registry.read().unwrap();
    render(&reg, &snapshot)
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
                out.push_str(&format!("# HELP {metric} {}\n", d.display_name));
                out.push_str(&format!("# TYPE {metric} gauge\n"));
                emit_sample_line(&mut out, &metric, unit, device_key, &v.to_string());
            }
            Reading::Counter(v) => {
                out.push_str(&format!("# HELP {metric} {}\n", d.display_name));
                out.push_str(&format!("# TYPE {metric} counter\n"));
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
        out.push_str(&format!(
            "linsight_hardware_info{{device_key=\"{}\",category=\"{}\",model=\"{}\",vendor=\"{}\",nickname=\"{}\",plugin_id=\"{}\"}} 1\n",
            escape_label(dev.key.as_str()),
            dev.category.as_str(),
            escape_label(&dev.model),
            escape_label(vendor),
            escape_label(&nickname),
            escape_label(&dev.plugin_id),
        ));
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
    match device_key {
        Some(k) => out.push_str(&format!(
            "{metric}{{unit=\"{}\",device_key=\"{}\"}} {value}\n",
            escape_label(unit),
            escape_label(k.as_str()),
        )),
        None => out.push_str(&format!("{metric}{{unit=\"{}\"}} {value}\n", escape_label(unit),)),
    }
}

/// Escape a Prometheus label value per the exposition format:
/// backslash → `\\`, double-quote → `\"`, newline → `\n`.
/// Other control chars (NUL, etc.) are dropped at the daemon's
/// validate_nickname seam, so we don't see them here.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r"\n"),
            _ => out.push(c),
        }
    }
    out
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

    use linsight_core::{
        Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, Sample, SensorId,
        SensorKind, Unit,
    };

    use super::*;

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

    /// One-device, one-sensor fixture used by most tests. The sensor
    /// carries an explicit `device_key`, exercising the descriptor's
    /// own field (the v4 plugin path) rather than the `(plugin_id,
    /// device_id) -> key_for()` fallback.
    fn test_registry() -> HardwareRegistry {
        let d = [dev("pci:0000:06:00.0", "Intel Arc B-series", Some("Intel Corporation"))];
        let s = [sensor("xe.gpu0.util", Some("pci:0000:06:00.0"), Some("gpu0"))];
        HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &s)], HashMap::new())
    }

    #[test]
    fn exporter_emits_device_key_label_when_set() {
        let reg = test_registry();
        let rows = vec![(
            sensor("xe.gpu0.util", Some("pci:0000:06:00.0"), Some("gpu0")),
            Some(sample_scalar("xe.gpu0.util", 27.6)),
            "io.visorcraft.linsight.xe".to_owned(),
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
        assert!(body.contains(r#"plugin_id="io.visorcraft.linsight.xe""#));
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
        let reg = HardwareRegistry::build(&[("io.visorcraft.linsight.xe", &d, &[])], raw_nicks);
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
            "io.visorcraft.linsight.mem".to_owned(),
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
            "io.visorcraft.linsight.xe".to_owned(),
        )];
        let body = render(&reg, &rows);
        assert!(
            body.contains(r#"device_key="pci:0000:06:00.0""#),
            "expected device_key resolved via key_for; body was:\n{body}",
        );
    }
}
