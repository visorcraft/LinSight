// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

pub mod alert;
pub mod history;
pub mod list;
pub mod plugin;
pub mod read;
pub mod watch;

use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result};
use linsight_protocol::{
    ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, RequestOp, ResponsePayload, ServerMsg,
};

pub(crate) struct Session {
    pub reader: FrameReader<UnixStream>,
    pub writer: FrameWriter<UnixStream>,
}

pub(crate) fn connect_and_hello(socket: &Path) -> Result<Session> {
    let stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let read_stream = stream.try_clone().context("cloning stream")?;
    let mut reader = FrameReader::new(read_stream);
    let mut writer = FrameWriter::new(stream);
    writer
        .write_client(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "linsight-cli".into(),
            auth_token: None,
        })
        .context("writing hello")?;
    match reader.read_server().context("reading welcome")? {
        ServerMsg::Welcome { protocol_version, .. } if protocol_version == PROTOCOL_VERSION => {}
        ServerMsg::Welcome { protocol_version, .. } => {
            anyhow::bail!("protocol mismatch: daemon={protocol_version} cli={PROTOCOL_VERSION}");
        }
        ServerMsg::Bye { reason } => anyhow::bail!("daemon refused: {reason}"),
        other => anyhow::bail!("unexpected first message from daemon: {other:?}"),
    }
    Ok(Session { reader, writer })
}

/// Send a RequestOp RPC and return the matching ResponsePayload.
pub(crate) fn request_rpc(
    session: &mut Session,
    op: RequestOp,
) -> Result<ResponsePayload, anyhow::Error> {
    thread_local! {
        static NEXT_REQ_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
    }
    let req_id = NEXT_REQ_ID.with(|cell| {
        let id = cell.get();
        cell.set(id.wrapping_add(1));
        id
    });

    session
        .writer
        .write_client(&ClientMsg::Request { req_id, op })
        .context("writing RPC request")?;

    loop {
        match session.reader.read_server().context("reading RPC response")? {
            ServerMsg::Response { req_id: rid, result } if rid == req_id => {
                return result.map_err(|pe| {
                    anyhow::anyhow!("RPC error [{}]: {}", pe.code as u8, pe.message)
                });
            }
            ServerMsg::Bye { reason } => {
                return Err(anyhow::anyhow!("daemon disconnected: {reason}"));
            }
            ServerMsg::Sample(_)
            | ServerMsg::Welcome { .. }
            | ServerMsg::SensorList(_)
            | ServerMsg::SensorListBroadcast(_)
            | ServerMsg::SensorDegraded { .. } => continue,
            ServerMsg::Response { req_id: rid, .. } => {
                tracing::warn!("stale RPC response for req_id {rid}");
            }
        }
    }
}
