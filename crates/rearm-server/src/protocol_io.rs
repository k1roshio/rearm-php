//! Async helpers bridging `rearm-protocol` frames and Tokio I/O.

use anyhow::{anyhow, Context, Result};
use rearm_protocol::{decode_frame, encode_frame, RequestFrame, ResponseFrame, MAX_FRAME_SIZE};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let encoded = encode_frame(value).context("encode frame")?;
    // UDS/TCP write_all is enough; an extra flush() syscall per request shows up in ping.
    writer.write_all(&encoded).await?;
    Ok(())
}

pub async fn write_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &RequestFrame,
) -> Result<()> {
    write_frame(writer, request).await
}

pub async fn read_response<R: AsyncRead + Unpin>(reader: &mut R) -> Result<ResponseFrame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(anyhow!("response frame too large: {len}"));
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;
    decode_frame(&payload).context("decode response frame")
}
