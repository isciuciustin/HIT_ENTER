//! Driving [`frame`](crate::frame) over an async stream, from `he-proto`.
//!
//! Here rather than written twice because a length-prefix loop is protocol
//! code: two copies is two chances to disagree about what a truncated frame
//! means, and that disagreement shows up as a hang rather than an error.
//! Generic over [`tokio::io`] traits; tested against a `Vec<u8>`.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::frame::{self, FrameError, HEADER_BYTES};

/// Reads up to `buf.len()` bytes, stopping early only at end of stream, and
/// returns how many arrived — so a clean hang-up (zero) can be told from a
/// truncated frame (some, but not enough).
async fn fill<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut [u8]) -> Result<usize, FrameError> {
    let mut read = 0;
    while read < buf.len() {
        match reader.read(&mut buf[read..]).await {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(err) => return Err(FrameError::Transport(err.to_string())),
        }
    }
    Ok(read)
}

/// Reads one frame's body. [`FrameError::Closed`] when the stream ends on a
/// frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; HEADER_BYTES];
    match fill(reader, &mut header).await? {
        0 => return Err(FrameError::Closed),
        read if read < HEADER_BYTES => {
            return Err(FrameError::Truncated {
                read,
                expected: HEADER_BYTES,
            });
        }
        _ => {}
    }
    // Checked against the cap before this allocates.
    let expected = frame::body_len(&header)?;
    let mut body = vec![0u8; expected];
    let read = fill(reader, &mut body).await?;
    if read < expected {
        return Err(FrameError::Truncated { read, expected });
    }
    Ok(body)
}

/// Writes one frame and flushes it.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
) -> Result<(), FrameError> {
    let frame = frame::encode(body)?;
    writer
        .write_all(&frame)
        .await
        .map_err(|err| FrameError::Transport(err.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|err| FrameError::Transport(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_end_cleanly() {
        let mut wire = Vec::new();
        for body in [&b"[1]"[..], b"[2]", b"[3]"] {
            write_frame(&mut wire, body).await.unwrap();
        }
        let mut reader = wire.as_slice();
        for body in [&b"[1]"[..], b"[2]", b"[3]"] {
            assert_eq!(read_frame(&mut reader).await.unwrap(), body);
        }
        assert_eq!(read_frame(&mut reader).await, Err(FrameError::Closed));
    }

    #[tokio::test]
    async fn a_body_cut_short_is_truncated_not_closed() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"[42]").await.unwrap();
        wire.truncate(wire.len() - 1);
        assert!(matches!(
            read_frame(&mut wire.as_slice()).await,
            Err(FrameError::Truncated { .. })
        ));
    }

    #[tokio::test]
    async fn an_oversized_header_stops_the_read() {
        let mut wire = u32::MAX.to_le_bytes().to_vec();
        wire.extend_from_slice(b"not a gigabyte");
        assert!(matches!(
            read_frame(&mut wire.as_slice()).await,
            Err(FrameError::TooLarge { .. })
        ));
    }
}
