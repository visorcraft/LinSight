// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

pub mod alert;
pub mod db;
pub mod history;
pub mod list;
pub mod plugin;
pub mod read;
pub mod watch;

use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result};
use linsight_protocol::{
    ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, RequestOp, ResponsePayload,
    ResponseReadError, ServerMsg,
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

/// Escape a string for safe CSV output (RFC 4180): wraps in double quotes
/// when the value contains `,`, `"`, `\n`, or `\r`, and doubles any embedded
/// `"`. Shared by `db::export --format csv` and `history --format csv`.
pub(crate) fn csv_cell(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
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

    match session.reader.read_server_response(req_id) {
        Ok(payload) => Ok(payload),
        Err(ResponseReadError::Server(pe)) => {
            Err(anyhow::anyhow!("RPC error [{}]: {}", pe.code as u8, pe.message))
        }
        Err(ResponseReadError::Bye(reason)) => {
            Err(anyhow::anyhow!("daemon disconnected: {reason}"))
        }
        Err(ResponseReadError::Frame(e)) => Err(e).context("reading RPC response"),
    }
}
