// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `linsight-cli watch <sensors...> [--rate <hz>] [--format <plain|json>] [--count <n>]`
//!
//! Subscribe to one or more sensors and stream live formatted values
//! until Ctrl+C or `--count` samples are received.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use linsight_core::{Reading, SensorId, Unit};
use linsight_protocol::{ClientMsg, ServerMsg};

use crate::commands::connect_and_hello;

pub fn run(
    socket: &Path,
    sensors: &[String],
    rate_hz: Option<f64>,
    format: &str,
    count: Option<u64>,
) -> Result<()> {
    // `--count 0` is a no-op: exit immediately without contacting the daemon.
    if matches!(count, Some(0)) {
        return Ok(());
    }

    let sensor_ids: Vec<SensorId> =
        sensors.iter().map(SensorId::try_new).collect::<Result<Vec<_>, _>>()?;

    let mut session = connect_and_hello(socket)?;

    // Fetch the full sensor list so we can validate every requested sensor
    // exists *and* capture each sensor's unit for formatting.
    session.writer.write_client(&ClientMsg::ListSensors)?;
    let units: std::collections::HashMap<SensorId, Unit> = match session.reader.read_server()? {
        ServerMsg::SensorList(infos) => {
            let mut map = std::collections::HashMap::new();
            for id in &sensor_ids {
                match infos.iter().find(|i| i.id == *id) {
                    Some(i) => {
                        map.insert(id.clone(), i.unit.clone());
                    }
                    None => anyhow::bail!(
                        "sensor not found: {id} \
                         (run `linsight-cli list` for the available set)",
                    ),
                }
            }
            map
        }
        other => anyhow::bail!("expected SensorList, got {other:?}"),
    };

    // Register Ctrl+C handler so we can send a graceful Goodbye.
    let term = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&term))?;

    session.writer.write_client(&ClientMsg::Subscribe {
        sensors: sensor_ids.clone(),
        rate_hz: rate_hz.map(|h| h as f32),
    })?;

    let mut printed = 0u64;
    let exit_code = loop {
        // Check for Ctrl+C between samples so we don't have to wait for
        // the next server message before reacting.
        if term.load(Ordering::Relaxed) {
            eprintln!("interrupted — shutting down");
            break Ok(());
        }

        match session.reader.read_server()? {
            ServerMsg::Sample(s) if sensor_ids.contains(&s.sensor) => {
                let unit = match units.get(&s.sensor) {
                    Some(u) => u,
                    // Should never happen since we validated above, but
                    // be defensive rather than panic.
                    None => {
                        tracing::warn!("sample for unknown sensor: {}", s.sensor);
                        continue;
                    }
                };
                match format {
                    "json" => println!("{}", json_sample(&s.sensor, &s.reading, unit)),
                    _ => println!("{}\t{}", s.sensor, plain_sample(&s.reading, unit)),
                }
                printed += 1;
                if let Some(max) = count
                    && printed >= max
                {
                    break Ok(());
                }
            }
            ServerMsg::SensorDegraded { sensor: id, reason } if sensor_ids.contains(&id) => {
                anyhow::bail!("{id} degraded: {reason}");
            }
            ServerMsg::Bye { reason } => {
                eprintln!("daemon shutting down: {reason}");
                break Ok(());
            }
            // Ignore samples / degradation for sensors we didn't
            // subscribe to, duplicate Welcome, re-broadcast SensorList,
            // and protocol-internal Response frames.
            ServerMsg::Sample(_)
            | ServerMsg::SensorDegraded { .. }
            | ServerMsg::Welcome { .. }
            | ServerMsg::SensorList(_)
            | ServerMsg::Response { .. }
            | ServerMsg::SensorListBroadcast(_) => continue,
        }
    };

    // Best-effort cleanup — don't let a send failure mask the exit code.
    session.writer.write_client(&ClientMsg::Unsubscribe { sensors: sensor_ids.clone() }).ok();
    session.writer.write_client(&ClientMsg::Goodbye).ok();
    exit_code
}

// ---------------------------------------------------------------------------
// Plain-text formatting
// ---------------------------------------------------------------------------

fn plain_sample(r: &Reading, unit: &Unit) -> String {
    match r {
        Reading::Scalar(v) => format_scalar(*v, unit),
        Reading::Counter(v) => format!("{v} {}", unit.symbol()),
        Reading::State(s) => s.clone(),
        Reading::Table(rows) => format!("<{} rows>", rows.len()),
    }
}

fn format_scalar(v: f64, unit: &Unit) -> String {
    match unit {
        Unit::Percent => format!("{v:.1}%"),
        Unit::Celsius => format!("{v:.1}°C"),
        Unit::Bytes => format_bytes(v),
        Unit::BytesPerSec => format!("{} B/s", v as i64),
        Unit::Hertz => format!("{v:.0} Hz"),
        Unit::Watts => format!("{v:.1} W"),
        Unit::Volts => format!("{v:.3} V"),
        Unit::Rpm => format!("{v:.0} rpm"),
        Unit::Count => format!("{v}"),
        Unit::Custom(s) => format!("{v} {s}"),
    }
}

fn format_bytes(v: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    match v.abs() {
        x if x >= TB => format!("{:.2} TiB", v / TB),
        x if x >= GB => format!("{:.2} GiB", v / GB),
        x if x >= MB => format!("{:.2} MiB", v / MB),
        x if x >= KB => format!("{:.2} KiB", v / KB),
        _ => format!("{v} B"),
    }
}

// ---------------------------------------------------------------------------
// JSON formatting
// ---------------------------------------------------------------------------

fn json_sample(sensor: &SensorId, r: &Reading, unit: &Unit) -> String {
    let unit_sym = unit.symbol();
    match r {
        Reading::Scalar(v) => {
            format!(r#"{{"sensor":"{sensor}","value":{v},"unit":"{unit_sym}","kind":"scalar"}}"#)
        }
        Reading::Counter(v) => {
            format!(r#"{{"sensor":"{sensor}","value":{v},"unit":"{unit_sym}","kind":"counter"}}"#)
        }
        Reading::State(s) => {
            // JSON-escape the state string (simplified — handles
            // backslash and double-quote escapes only).
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!(r#"{{"sensor":"{sensor}","state":"{escaped}","kind":"state"}}"#)
        }
        Reading::Table(rows) => {
            format!(r#"{{"sensor":"{sensor}","rows":{},"kind":"table"}}"#, rows.len())
        }
    }
}
