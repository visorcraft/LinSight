// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use linsight_protocol::{ClientMsg, ServerMsg};

use crate::commands::connect_and_hello;

pub fn run(socket: &Path) -> Result<()> {
    let mut session = connect_and_hello(socket)?;
    session.writer.write_client(&ClientMsg::ListSensors)?;
    match session.reader.read_server()? {
        ServerMsg::SensorList(infos) => {
            for s in infos {
                let kind = match s.kind {
                    linsight_core::SensorKind::Scalar => "scalar",
                    linsight_core::SensorKind::Counter => "counter",
                    linsight_core::SensorKind::Table => "table",
                    linsight_core::SensorKind::State => "state",
                };
                println!("{:<24}  {:<24}  {} {}", s.id, s.display_name, s.unit.symbol(), kind);
            }
        }
        other => anyhow::bail!("expected SensorList, got {other:?}"),
    }
    Ok(())
}
