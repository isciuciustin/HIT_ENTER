//! Length-prefixed JSON framing — the same shape as HIT_ENTER's `he-proto`:
//! a `u32` little-endian byte count, then that many bytes of UTF-8 JSON.
//! The length is checked against the cap *before* a body is allocated.

use anyhow::{Result, bail};
use iroh::endpoint::{ReadExactError, RecvStream, SendStream};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub async fn write_frame(send: &mut SendStream, body: &str) -> Result<usize> {
    let bytes = body.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        bail!("frame of {} bytes is outside 1..={MAX_FRAME_BYTES}", bytes.len());
    }
    send.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    send.write_all(bytes).await?;
    Ok(4 + bytes.len())
}

/// `Ok(None)` is a clean end of stream between frames.
pub async fn read_frame(recv: &mut RecvStream) -> Result<Option<(String, usize)>> {
    let mut header = [0u8; 4];
    match recv.read_exact(&mut header).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        bail!("peer announced a frame of {len} bytes; cap is {MAX_FRAME_BYTES}");
    }
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(Some((String::from_utf8(body)?, 4 + len)))
}
