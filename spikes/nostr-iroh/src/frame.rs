//! Length-prefixed JSON frames: u32 big-endian length, then UTF-8 JSON.
//! 1 MiB cap, as in DESIGN.md §4.

use anyhow::{bail, Result};
use iroh::endpoint::{RecvStream, SendStream};

pub const MAX_FRAME: usize = 1024 * 1024;

pub async fn write_frame(send: &mut SendStream, json: &str) -> Result<usize> {
    let bytes = json.as_bytes();
    if bytes.len() > MAX_FRAME {
        bail!("frame of {} bytes exceeds the {} cap", bytes.len(), MAX_FRAME);
    }
    send.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    send.write_all(bytes).await?;
    Ok(4 + bytes.len())
}

/// `None` when the peer finished the stream cleanly between frames.
pub async fn read_frame(recv: &mut RecvStream) -> Result<Option<(String, usize)>> {
    let mut len = [0u8; 4];
    match recv.read_exact(&mut len).await {
        Ok(()) => {}
        Err(iroh::endpoint::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("peer announced a {len}-byte frame, over the {MAX_FRAME} cap");
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    Ok(Some((String::from_utf8(buf)?, 4 + len)))
}
