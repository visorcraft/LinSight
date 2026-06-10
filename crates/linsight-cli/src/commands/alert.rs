// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use linsight_protocol::{RequestOp, ResponsePayload};

use crate::commands::{connect_and_hello, request_rpc};

pub fn list(socket: &Path) -> Result<()> {
    let mut session = connect_and_hello(socket)?;
    let payload = request_rpc(&mut session, RequestOp::ListAlerts)?;
    match payload {
        ResponsePayload::AlertList { rules } => {
            if rules.is_empty() {
                println!("No alert rules configured.");
            } else {
                println!(
                    "{:<24} {:<40} {:<12} {:<12} {:<8} Notify",
                    "Name", "Expression", "Debounce", "Cooldown", "Enabled"
                );
                println!("{}", "-".repeat(120));
                for rule in &rules {
                    let debounce = rule.for_duration.as_deref().unwrap_or("0s");
                    let cooldown = rule.cooldown.as_deref().unwrap_or("0s");
                    let enabled_str = if rule.enabled { "yes" } else { "no" };
                    let notify = rule.notify.join(", ");
                    println!(
                        "{:<24} {:<40} {:<12} {:<12} {:<8} {}",
                        rule.name, rule.expr, debounce, cooldown, enabled_str, notify
                    );
                }
            }
        }
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
    Ok(())
}

pub fn add(
    socket: &Path,
    name: &str,
    expr: &str,
    for_duration: Option<&str>,
    cooldown: Option<&str>,
    notify: &[String],
) -> Result<()> {
    let mut session = connect_and_hello(socket)?;
    let for_d = for_duration.map(|s| s.to_owned());
    let cd = cooldown.map(|s| s.to_owned());
    let payload = request_rpc(
        &mut session,
        RequestOp::UpsertAlert {
            name: name.to_owned(),
            expr: expr.to_owned(),
            for_duration: for_d,
            cooldown: cd,
            notify: notify.to_vec(),
            enabled: None,
        },
    )?;
    match payload {
        ResponsePayload::AlertUpserted { name: n } => {
            println!("Alert rule '{n}' saved.");
        }
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
    Ok(())
}

pub fn remove(socket: &Path, name: &str) -> Result<()> {
    let mut session = connect_and_hello(socket)?;
    let payload = request_rpc(&mut session, RequestOp::DeleteAlert { name: name.to_owned() })?;
    match payload {
        ResponsePayload::AlertDeleted { name: n } => {
            println!("Alert rule '{n}' removed.");
        }
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
    Ok(())
}
