// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use linsight_core::{Reading, SensorId, Unit};
use linsight_protocol::{ClientMsg, RequestOp, ResponsePayload, ServerMsg};

use crate::commands::{connect_and_hello, request_rpc};

pub fn run(socket: &Path, sensor: &str, count: Option<u64>) -> Result<()> {
    // `--count 0` is a no-op: print zero samples and exit cleanly without
    // even bothering the daemon. Previously this printed one sample
    // because the limit check happened after the print.
    if matches!(count, Some(0)) {
        return Ok(());
    }
    let sensor_id = SensorId::try_new(sensor)?;
    let mut session = connect_and_hello(socket)?;

    // Pre-fetch the sensor's unit so we can format values correctly. Also
    // serves as the existence check: an unknown sensor name used to fall
    // through to `Unit::Count` and subscribe blindly, which the daemon
    // would silently discard — the CLI then waited forever for samples
    // that would never arrive. Bail immediately instead.
    let unit = match request_rpc(
        &mut session,
        RequestOp::GetSensorInfo { sensor: sensor_id.to_string() },
    )? {
        ResponsePayload::SensorInfo { info } => info.unit,
        other => anyhow::bail!("expected SensorInfo, got {other:?}"),
    };

    session
        .writer
        .write_client(&ClientMsg::Subscribe { sensors: vec![sensor_id.clone()], rate_hz: None })?;
    let mut printed = 0u64;
    let exit_code = loop {
        match session.reader.read_server()? {
            ServerMsg::Sample(s) if s.sensor == sensor_id => {
                let line = format_sample(&s.reading, &unit);
                println!("{}\t{}", s.sensor, line);
                printed += 1;
                if let Some(max) = count
                    && printed >= max
                {
                    break Ok(());
                }
            }
            ServerMsg::SensorDegraded { sensor: id, reason } if id == sensor_id => {
                anyhow::bail!("{sensor_id} degraded: {reason}");
            }
            ServerMsg::Bye { reason } => {
                // Match explicitly so the user gets "daemon going away"
                // instead of the opaque I/O error that bubbled up before
                // when the daemon shut down mid-stream.
                eprintln!("daemon shutting down: {reason}");
                break Ok(());
            }
            // Other Sample / SensorDegraded for unrelated sensor IDs, or
            // a duplicate Welcome — uninteresting for this subscription.
            ServerMsg::Sample(_)
            | ServerMsg::SensorDegraded { .. }
            | ServerMsg::Welcome { .. }
            | ServerMsg::SensorList(_) => continue,
            // Phase H1 wires real handlers; for now just consume and
            // continue so v2 daemon broadcasts don't break `linsight read`.
            ServerMsg::Response { .. } | ServerMsg::SensorListBroadcast(_) => continue,
        }
    };
    session.writer.write_client(&ClientMsg::Unsubscribe { sensors: vec![sensor_id] }).ok();
    session.writer.write_client(&ClientMsg::Goodbye).ok();
    exit_code
}

fn format_sample(r: &Reading, unit: &Unit) -> String {
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
