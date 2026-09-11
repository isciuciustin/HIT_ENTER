//! Driving [`frame`](crate::frame) over an async stream.
//!
//! **This module is behind the off-by-default `io` feature**, so `he-proto`
//! keeps its PLAN §7 property — no I/O, no runtime — for anyone who wants only
//! the types. `he-server` and `he-client` both turn it on.
//!
//! It is here, rather than written twice, because a length-prefix loop is
//! protocol code: two copies is two chances to disagree about what a truncated
//! frame means, and that disagreement would show up as a hang rather than an
//! error. Nothing in this module knows about sockets, files, or iroh — it is
//! generic over [`tokio::io`] traits and is exercised in tests against a
//! `Vec<u8>`.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::frame::{self, FrameError, HEADER_BYTES};

/// Reads exactly `buf.len()` bytes, or as many as arrive before the end of the
/// stream. Returns how many were read so the caller can tell a clean hang-up
/// (zero) from a truncated frame (some, but not enough).
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

/// Reads one frame and parses it as `T`.
///
/// Returns [`FrameError::Closed`] when the stream ends on a frame boundary,
/// which is how a request stream ends normally and how a control stream
/// reports that the peer is gone.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
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

    // Checked against the 1 MiB cap before this allocates. A peer cannot spend
    // our memory by lying in four bytes of header.
    let expected = frame::body_len(&header)?;
    let mut body = vec![0u8; expected];
    let read = fill(reader, &mut body).await?;
    if read < expected {
        return Err(FrameError::Truncated { read, expected });
    }

    frame::decode(&body)
}

/// Writes one frame and flushes it.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let frame = frame::encode(value)?;
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
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Probe {
        n: u32,
    }

    #[tokio::test]
    async fn frames_round_trip_over_a_stream() {
        let mut wire = Vec::new();
        for n in 0..3 {
            write_frame(&mut wire, &Probe { n }).await.expect("write");
        }

        let mut reader = wire.as_slice();
        for n in 0..3 {
            let probe: Probe = read_frame(&mut reader).await.expect("read");
            assert_eq!(probe, Probe { n });
        }
        // Framing must survive being read back one frame at a time: a reader
        // that over-reads into the next frame would desynchronise the stream.
        assert_eq!(
            read_frame::<_, Probe>(&mut reader).await,
            Err(FrameError::Closed)
        );
    }

    #[tokio::test]
    async fn a_clean_hangup_is_not_an_error_shape() {
        let mut empty: &[u8] = &[];
        assert_eq!(
            read_frame::<_, Probe>(&mut empty).await,
            Err(FrameError::Closed),
            "a peer that hangs up between frames has not done anything wrong"
        );
    }

    #[tokio::test]
    async fn a_body_cut_short_is_truncated_not_closed() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &Probe { n: 42 })
            .await
            .expect("write");
        wire.truncate(wire.len() - 1);

        let mut reader = wire.as_slice();
        assert!(
            matches!(
                read_frame::<_, Probe>(&mut reader).await,
                Err(FrameError::Truncated { .. })
            ),
            "losing a connection mid-frame is a fault, not a hang-up"
        );
    }

    #[tokio::test]
    async fn an_oversized_header_stops_the_read() {
        let mut wire = u32::MAX.to_le_bytes().to_vec();
        wire.extend_from_slice(b"not a gigabyte");
        let mut reader = wire.as_slice();
        assert!(matches!(
            read_frame::<_, Probe>(&mut reader).await,
            Err(FrameError::TooLarge { .. })
        ));
    }
}
