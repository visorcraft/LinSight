// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use linsight_core::{Reading, SensorId};
use linsight_protocol::{RequestOp, ResponsePayload};

use crate::commands::{connect_and_hello, request_rpc};

/// Escape a string for safe JSON string interpolation.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Escape a string for safe CSV output (RFC 4180).
fn csv_cell(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Parse a simple duration string like "5m" or "1h" into microseconds.
fn parse_duration_to_micros(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num_str, unit) =
        s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len()));
    let num: f64 = num_str.parse()?;
    let micros = match unit {
        "" | "s" => num * 1_000_000.0,
        "ms" => num * 1_000.0,
        "m" => num * 60_000_000.0,
        "h" => num * 3_600_000_000.0,
        "d" => num * 86_400_000_000.0,
        other => anyhow::bail!("unknown duration unit: {other:?}"),
    };
    Ok(micros as u64)
}

pub fn run(
    socket: &Path,
    sensor: &str,
    last: &str,
    fmt: &str,
    max_points: Option<u32>,
) -> Result<()> {
    let _ = SensorId::try_new(sensor)
        .map_err(|e| anyhow::anyhow!("invalid sensor id '{sensor}': {e}"))?;

    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let window = parse_duration_to_micros(last)?;
    let since_micros = now_micros.saturating_sub(window);

    let mut session = connect_and_hello(socket)?;
    let payload = request_rpc(
        &mut session,
        RequestOp::GetHistory {
            sensor: sensor.to_owned(),
            since_micros,
            until_micros: now_micros,
            max_points,
        },
    )?;

    match payload {
        ResponsePayload::History { sensor: _, samples } => {
            if samples.is_empty() {
                println!("No history data for '{sensor}' in the requested window.");
                return Ok(());
            }

            match fmt {
                "json" => {
                    println!("[");
                    for (i, s) in samples.iter().enumerate() {
                        let comma = if i < samples.len() - 1 { "," } else { "" };
                        let (value_str, kind) = match &s.reading {
                            Reading::Scalar(v) => (format!("{v}"), "scalar"),
                            Reading::Counter(v) => (format!("{v}"), "counter"),
                            Reading::State(v) => (format!("\"{}\"", json_escape(v)), "state"),
                            Reading::Table(_) => continue,
                        };
                        println!(
                            r#"  {{"ts":{},"sensor":"{}","value":{},"kind":"{}"}}{}"#,
                            s.ts_micros,
                            json_escape(s.sensor.as_str()),
                            value_str,
                            kind,
                            comma
                        );
                    }
                    println!("]");
                }
                "csv" => {
                    println!("ts_micros,sensor,value,kind");
                    for s in &samples {
                        let (value_str, kind) = match &s.reading {
                            Reading::Scalar(v) => (format!("{v}"), "scalar"),
                            Reading::Counter(v) => (format!("{v}"), "counter"),
                            Reading::State(v) => (csv_cell(v).to_string(), "state"),
                            Reading::Table(_) => continue,
                        };
                        println!(
                            "{},{},{},{}",
                            csv_cell(&s.ts_micros.to_string()),
                            csv_cell(s.sensor.as_str()),
                            csv_cell(&value_str),
                            csv_cell(kind)
                        );
                    }
                }
                _ => {
                    for s in &samples {
                        let value_str = match &s.reading {
                            Reading::Scalar(v) => format!("{v:.2}"),
                            Reading::Counter(v) => format!("{v}"),
                            Reading::State(v) => v.clone(),
                            Reading::Table(_) => continue,
                        };
                        println!("{}\t{}", s.ts_micros, value_str);
                    }
                }
            }
        }
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
    Ok(())
}
