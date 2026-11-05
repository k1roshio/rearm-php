use crate::error::ProtocolError;
use bytes::BytesMut;
use serde::{de::DeserializeOwned, Serialize};

/// Default max frame payload (64 MiB).
pub const MAX_FRAME_SIZE: u32 = 64 * 1024 * 1024;

/// Encode `value` as MessagePack and prepend a 4-byte big-endian length.
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let payload = rmp_serde::to_vec_named(value)?;
    if payload.len() > MAX_FRAME_SIZE as usize {
        return Err(ProtocolError::FrameTooLarge(payload.len() as u32));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decode a MessagePack payload (without the length prefix).
pub fn decode_frame<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtocolError> {
    Ok(rmp_serde::from_slice(payload)?)
}

/// Split a complete wire buffer into `(length, payload)`.
pub fn split_length_prefix(frame: &[u8]) -> Result<(u32, &[u8]), ProtocolError> {
    if frame.len() < 4 {
        return Err(ProtocolError::TruncatedHeader);
    }
    let len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
    let need = 4 + len as usize;
    if frame.len() < need {
        return Err(ProtocolError::Incomplete {
            have: frame.len(),
            need,
        });
    }
    Ok((len, &frame[4..need]))
}

/// Incremental reader that reassembles length-prefixed frames from a byte stream.
#[derive(Debug, Default)]
pub struct FrameReader {
    buf: BytesMut,
    max_frame_size: u32,
}

impl FrameReader {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::new(),
            max_frame_size: MAX_FRAME_SIZE,
        }
    }

    pub fn with_max_frame_size(max_frame_size: u32) -> Self {
        Self {
            buf: BytesMut::new(),
            max_frame_size,
        }
    }

    /// Append `data` and return a complete payload (without length prefix) if available.
    pub fn push(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, ProtocolError> {
        self.buf.extend_from_slice(data);
        self.try_next()
    }

    /// Try to extract one complete frame payload from the buffer.
    pub fn try_next(&mut self) -> Result<Option<Vec<u8>>, ProtocolError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        if len > self.max_frame_size {
            return Err(ProtocolError::FrameTooLarge(len));
        }
        let total = 4 + len as usize;
        if self.buf.len() < total {
            return Ok(None);
        }
        let _ = self.buf.split_to(4);
        let payload = self.buf.split_to(len as usize).to_vec();
        Ok(Some(payload))
    }

    /// Clear buffered data (e.g. after a protocol error).
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}
