// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::io::{self, Read, Write};

use thiserror::Error;

use crate::messages::{ClientMsg, ServerMsg};

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
        let mut body = vec![0u8; len as usize];
        self.inner.read_exact(&mut body)?;
        Ok(body)
    }
}

pub struct FrameWriter<W: Write> {
    inner: W,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    pub fn write_client(&mut self, msg: &ClientMsg) -> Result<(), FrameError> {
        let bytes = postcard::to_allocvec(msg)?;
        self.write_frame(&bytes)
    }

    pub fn write_server(&mut self, msg: &ServerMsg) -> Result<(), FrameError> {
        let bytes = postcard::to_allocvec(msg)?;
        self.write_frame(&bytes)
    }

    fn write_frame(&mut self, body: &[u8]) -> Result<(), FrameError> {
        // Compare on `usize` BEFORE narrowing to `u32`: a body of exactly
        // 2^32 bytes would otherwise truncate to `len = 0` and slip past
        // the guard, producing a malformed frame.
        if body.len() > MAX_FRAME_BYTES as usize {
            return Err(FrameError::Oversized(body.len() as u64));
        }
        let len = body.len() as u32;
        self.inner.write_all(&len.to_le_bytes())?;
        self.inner.write_all(body)?;
        Ok(())
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
