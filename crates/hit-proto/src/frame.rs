//! Length-prefixed framing, from HIT_ENTER's `he-proto`.
//!
//! Every frame is a `u32` little-endian byte count followed by that many
//! bytes of JSON. The length is checked against [`MAX_FRAME_BYTES`]
//! **before** a buffer is allocated, so a peer cannot ask us to reserve a
//! gigabyte by sending four bytes.
//!
//! Bodies are raw bytes here; [`crate::hello`] and the node parse them. The
//! JSON is NIP-01's own, so a frame can be read off the wire and pasted into
//! any Nostr tool.

use thiserror::Error;

pub use crate::limits::MAX_FRAME_BYTES;

/// Bytes of length prefix in front of every frame body.
pub const HEADER_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FrameError {
    /// The peer announced a body larger than [`MAX_FRAME_BYTES`].
    /// Rejected before allocating, which is the entire point of the check.
    #[error("frame of {announced} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    TooLarge { announced: u64 },
    /// A zero-length body. There is no valid empty frame.
    #[error("frame body is empty")]
    Empty,
    /// The stream ended cleanly on a frame boundary: the peer is done.
    #[error("stream closed")]
    Closed,
    /// The stream ended *mid-frame*. Distinct from [`Self::Closed`] because
    /// this one means something broke, not that someone hung up.
    #[error("stream ended mid-frame after {read} of {expected} bytes")]
    Truncated { read: usize, expected: usize },
    /// The transport failed. `String` so the type stays `Clone` and
    /// `PartialEq`; the text is a transport message, never frame content.
    #[error("transport error: {0}")]
    Transport(String),
}

/// A complete frame: header followed by `body`.
pub fn encode(body: &[u8]) -> Result<Vec<u8>, FrameError> {
    if body.is_empty() {
        return Err(FrameError::Empty);
    }
    if body.len() > MAX_FRAME_BYTES {
        // Our own frame is over the limit. The peer would reject it, so fail
        // here where the backtrace still points at whoever built it.
        return Err(FrameError::TooLarge {
            announced: body.len() as u64,
        });
    }
    let mut frame = Vec::with_capacity(HEADER_BYTES + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

/// The body length a header announces, refused if over the limit **before**
/// the caller allocates.
pub fn body_len(header: &[u8; HEADER_BYTES]) -> Result<usize, FrameError> {
    match u64::from(u32::from_le_bytes(*header)) {
        0 => Err(FrameError::Empty),
        n if n > MAX_FRAME_BYTES as u64 => Err(FrameError::TooLarge { announced: n }),
        n => Ok(n as usize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_and_the_header_is_little_endian() {
        let frame = encode(br#"["EOSE","x"]"#).unwrap();
        let header: [u8; HEADER_BYTES] = frame[..HEADER_BYTES].try_into().unwrap();
        assert_eq!(body_len(&header), Ok(frame.len() - HEADER_BYTES));
        assert_eq!(frame[..HEADER_BYTES], 12u32.to_le_bytes());
    }

    #[test]
    fn an_oversized_announcement_is_refused_before_allocating() {
        assert_eq!(
            body_len(&u32::MAX.to_le_bytes()),
            Err(FrameError::TooLarge {
                announced: u64::from(u32::MAX)
            })
        );
    }

    #[test]
    fn empty_and_oversized_bodies_are_not_frames() {
        assert_eq!(body_len(&0u32.to_le_bytes()), Err(FrameError::Empty));
        assert_eq!(encode(b""), Err(FrameError::Empty));
        assert!(matches!(
            encode(&vec![b'x'; MAX_FRAME_BYTES + 1]),
            Err(FrameError::TooLarge { .. })
        ));
    }
}
