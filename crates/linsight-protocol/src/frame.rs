// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::io::{self, Read, Write};

use serde::Serialize;
use thiserror::Error;

use crate::messages::{ClientMsg, ProtoError, ResponsePayload, ServerMsg};

/// Cap on a single frame's body. 1 MiB is far larger than any
/// realistic LinSight message; anything bigger is treated as
/// adversarial / corrupted.
pub const MAX_FRAME_BYTES: u32 = 1 << 20;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("decode: {0}")]
    Decode(#[from] postcard::Error),
    #[error("frame larger than MAX_FRAME_BYTES: {0} bytes")]
    Oversized(u64),
    #[error("connection closed")]
    Closed,
}

pub struct FrameReader<R: Read> {
    inner: R,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    pub fn read_client(&mut self) -> Result<ClientMsg, FrameError> {
        let bytes = self.read_frame()?;
        Ok(postcard::from_bytes(&bytes)?)
    }

    pub fn read_server(&mut self) -> Result<ServerMsg, FrameError> {
        let bytes = self.read_frame()?;
        Ok(postcard::from_bytes(&bytes)?)
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, FrameError> {
        let (len, first) = self.read_frame_header()?;
        let mut body = vec![0u8; len as usize];
        body[0] = first;
        if len > 1 {
            self.inner.read_exact(&mut body[1..])?;
        }
        Ok(body)
    }

    /// Read the length prefix and the first body byte without decoding
    /// the rest of the frame. Callers can use [`FrameReader::skip_frame_body`]
    /// to discard the remainder, or reconstruct the full body for the
    /// variants they care about.
    pub fn read_frame_header(&mut self) -> Result<(u32, u8), FrameError> {
        let mut len_buf = [0u8; 4];
        match self.inner.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
            Err(e) => return Err(FrameError::Io(e)),
        }
        let len = u32::from_le_bytes(len_buf);
        if len > MAX_FRAME_BYTES {
            return Err(FrameError::Oversized(len as u64));
        }
        let mut first = [0u8; 1];
        match self.inner.read_exact(&mut first) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
            Err(e) => return Err(FrameError::Io(e)),
        }
        Ok((len, first[0]))
    }

    /// Discard `remaining` bytes from the current frame body. Used by the
    /// CLI RPC path to skip pushed samples without fully decoding them.
    pub fn skip_frame_body(&mut self, remaining: u32) -> Result<(), FrameError> {
        let mut remaining = remaining as usize;
        if remaining == 0 {
            return Ok(());
        }
        let mut discard = [0u8; 1024];
        while remaining > 0 {
            let n = std::cmp::min(remaining, discard.len());
            self.inner.read_exact(&mut discard[..n])?;
            remaining -= n;
        }
        Ok(())
    }

    /// Wait for a matching `ServerMsg::Response` for `req_id`, skipping
    /// pushed samples and catalogue broadcasts without fully decoding them.
    /// A `ServerMsg::Bye` is decoded so its reason can be surfaced.
    pub fn read_server_response(
        &mut self,
        req_id: u32,
    ) -> Result<ResponsePayload, ResponseReadError> {
        // Variant indexes for `ServerMsg` in declaration order.
        const BYE: u8 = 4;
        const RESPONSE: u8 = 5;

        loop {
            let (len, first) = self.read_frame_header()?;
            match first {
                RESPONSE => {
                    let mut body = vec![0u8; len as usize];
                    body[0] = first;
                    if len > 1 {
                        self.inner.read_exact(&mut body[1..]).map_err(FrameError::Io)?;
                    }
                    let msg: ServerMsg = postcard::from_bytes(&body).map_err(FrameError::Decode)?;
                    if let ServerMsg::Response { req_id: rid, result } = msg
                        && rid == req_id
                    {
                        return result.map_err(ResponseReadError::Server);
                    }
                }
                BYE => {
                    let mut body = vec![0u8; len as usize];
                    body[0] = first;
                    if len > 1 {
                        self.inner.read_exact(&mut body[1..]).map_err(FrameError::Io)?;
                    }
                    let msg: ServerMsg = postcard::from_bytes(&body).map_err(FrameError::Decode)?;
                    if let ServerMsg::Bye { reason } = msg {
                        return Err(ResponseReadError::Bye(reason));
                    }
                }
                _ => {
                    if len > 1 {
                        self.skip_frame_body(len - 1)?;
                    }
                }
            }
        }
    }
}

/// Ways a response-only read can fail without returning the payload.
#[derive(Debug, thiserror::Error)]
pub enum ResponseReadError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("server: {0:?}")]
    Server(ProtoError),
    #[error("daemon disconnected: {0}")]
    Bye(String),
}

pub struct FrameWriter<W: Write> {
    inner: W,
    /// Reusable buffer for the serialized message body.
    body_buf: Vec<u8>,
    /// Reusable buffer holding the length prefix + body for a single
    /// `write_all`. Coalescing the two into one syscall reduces per-sample
    /// overhead on the hot-path pump threads.
    frame_buf: Vec<u8>,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, body_buf: Vec::with_capacity(256), frame_buf: Vec::with_capacity(260) }
    }

    pub fn write_client(&mut self, msg: &ClientMsg) -> Result<(), FrameError> {
        self.write_frame(msg)
    }

    pub fn write_server(&mut self, msg: &ServerMsg) -> Result<(), FrameError> {
        self.write_frame(msg)
    }

    fn write_frame<T: Serialize>(&mut self, msg: &T) -> Result<(), FrameError> {
        let body_len = self.serialize_body(msg)?;
        // Compare on `usize` BEFORE narrowing to `u32`: a body of exactly
        // 2^32 bytes would otherwise truncate to `len = 0` and slip past
        // the guard, producing a malformed frame.
        if body_len > MAX_FRAME_BYTES as usize {
            return Err(FrameError::Oversized(body_len as u64));
        }
        let len = body_len as u32;
        self.frame_buf.clear();
        self.frame_buf.extend_from_slice(&len.to_le_bytes());
        self.frame_buf.extend_from_slice(&self.body_buf[..body_len]);
        self.inner.write_all(&self.frame_buf)?;
        Ok(())
    }

    fn serialize_body<T: Serialize>(&mut self, msg: &T) -> Result<usize, FrameError> {
        loop {
            let cap = self.body_buf.capacity();
            self.body_buf.resize(cap, 0);
            match postcard::to_slice(msg, &mut self.body_buf) {
                Ok(slice) => return Ok(slice.len()),
                Err(postcard::Error::SerializeBufferFull) => {
                    self.body_buf.reserve(cap.max(256));
                }
                Err(e) => return Err(FrameError::Decode(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::PROTOCOL_VERSION;

    #[test]
    fn write_then_read_client_msg() {
        let original = ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
            auth_token: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        FrameWriter::new(&mut buf).write_client(&original).unwrap();

        let mut reader = FrameReader::new(Cursor::new(buf));
        let back = reader.read_client().unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn write_then_read_server_msg() {
        let original = ServerMsg::Welcome {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            plugins: vec![],
        };
        let mut buf: Vec<u8> = Vec::new();
        FrameWriter::new(&mut buf).write_server(&original).unwrap();

        let mut reader = FrameReader::new(Cursor::new(buf));
        let back = reader.read_server().unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn read_rejects_oversized_frame() {
        let mut bad = vec![0xff, 0xff, 0xff, 0xff];
        bad.extend_from_slice(b"junk");
        let mut reader = FrameReader::new(Cursor::new(bad));
        let err = reader.read_client().unwrap_err();
        assert!(matches!(err, FrameError::Oversized(_)));
    }

    #[test]
    fn multiple_frames_in_one_buffer() {
        let m1 = ClientMsg::Goodbye;
        let m2 = ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "x".into(),
            auth_token: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        let mut w = FrameWriter::new(&mut buf);
        w.write_client(&m1).unwrap();
        w.write_client(&m2).unwrap();

        let mut r = FrameReader::new(Cursor::new(buf));
        assert_eq!(r.read_client().unwrap(), m1);
        assert_eq!(r.read_client().unwrap(), m2);
    }
}
