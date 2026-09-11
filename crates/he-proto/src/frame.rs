//! Length-prefixed JSON framing.
//!
//! Every frame on every stream is a `u32` little-endian byte count followed by
//! that many bytes of JSON (PLAN §9). JSON is the choice *while the protocol
//! churns*: a frame can be read off the wire and pasted into a bug report. A
//! `postcard` codec goes behind a feature flag once M5 stops changing shapes.
//!
//! The length is checked against [`limits::MAX_FRAME_BYTES`] **before** a
//! buffer is allocated, so a peer cannot ask us to reserve a gigabyte by
//! sending four bytes.
//!
//! ## Why errors here are so vague
//!
//! [`FrameError::Malformed`] carries a `serde_json` error's *position and
//! category* but never its message. That message quotes the input it choked
//! on — `invalid type: string "hunter2", expected u32` — and the input is a
//! password on one frame in three (PLAN §11: never log a password). The line
//! and column are enough to debug with, and cannot leak.

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::limits::{self, MAX_FRAME_BYTES};

/// Bytes of length prefix in front of every frame body.
pub const HEADER_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FrameError {
    /// The peer announced a body larger than [`limits::MAX_FRAME_BYTES`].
    /// Rejected before allocating, which is the entire point of the check.
    #[error("frame of {announced} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    TooLarge { announced: u64 },

    /// A zero-length body. There is no valid empty frame.
    #[error("frame body is empty")]
    Empty,

    /// The body is not the JSON this stream expected. Deliberately does not
    /// quote the body — see the module docs.
    #[error("malformed frame body ({category}) at line {line}, column {column}")]
    Malformed {
        category: &'static str,
        line: usize,
        column: usize,
    },

    /// The value could not be turned into JSON. Only reachable from a
    /// `Serialize` impl that itself fails, which ours do not.
    #[error("frame body could not be encoded")]
    Unencodable,

    /// The stream ended cleanly on a frame boundary: the peer is done, or
    /// gone. Normal on a request stream, the end of the session on a control
    /// stream.
    #[error("stream closed")]
    Closed,

    /// The stream ended *mid-frame*. Distinct from [`Self::Closed`] because
    /// this one means something broke, not that someone hung up.
    #[error("stream ended mid-frame after {read} of {expected} bytes")]
    Truncated { read: usize, expected: usize },

    /// The transport failed. `String` rather than `io::Error` so that the type
    /// stays `Clone` and `PartialEq`; the text is a transport message and
    /// never contains frame content.
    #[error("transport error: {0}")]
    Transport(String),
}

impl FrameError {
    fn malformed(err: &serde_json::Error) -> Self {
        use serde_json::error::Category;
        Self::Malformed {
            category: match err.classify() {
                Category::Io => "io",
                Category::Syntax => "syntax",
                Category::Data => "data",
                Category::Eof => "eof",
            },
            line: err.line(),
            column: err.column(),
        }
    }
}

/// Serialises `value` into a complete frame: header followed by body.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(value).map_err(|_| FrameError::Unencodable)?;
    if body.len() > MAX_FRAME_BYTES {
        // Our own frame is over the limit. The peer would reject it, so fail
        // here where the backtrace still points at whoever built it.
        return Err(FrameError::TooLarge {
            announced: body.len() as u64,
        });
    }
    let mut frame = Vec::with_capacity(HEADER_BYTES + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Reads a header and returns the body length it announces, rejecting
/// anything over the limit **before** the caller allocates.
pub fn body_len(header: &[u8; HEADER_BYTES]) -> Result<usize, FrameError> {
    let announced = u32::from_le_bytes(*header) as u64;
    match announced {
        0 => Err(FrameError::Empty),
        n if n > MAX_FRAME_BYTES as u64 => Err(FrameError::TooLarge { announced: n }),
        n => Ok(n as usize),
    }
}

/// Parses a frame body. `body` must be exactly the bytes [`body_len`] asked for.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    serde_json::from_slice(body).map_err(|err| FrameError::malformed(&err))
}

/// Largest frame this build will accept, re-exported so a caller sizing a
/// buffer does not have to reach into [`limits`].
pub const fn max_frame_bytes() -> usize {
    limits::MAX_FRAME_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Probe {
        n: u32,
        s: String,
    }

    #[test]
    fn a_frame_round_trips() {
        let probe = Probe {
            n: 7,
            s: "hi".into(),
        };
        let frame = encode(&probe).expect("encodable");
        let header: [u8; HEADER_BYTES] = frame[..HEADER_BYTES].try_into().expect("header");
        let len = body_len(&header).expect("length");
        assert_eq!(len, frame.len() - HEADER_BYTES);
        assert_eq!(
            decode::<Probe>(&frame[HEADER_BYTES..]).expect("decodable"),
            probe
        );
    }

    #[test]
    fn the_header_is_little_endian() {
        // Fixed on the wire, not by the host. A big-endian peer must agree.
        let frame = encode(&Probe {
            n: 0,
            s: String::new(),
        })
        .expect("encodable");
        let body_bytes = frame.len() - HEADER_BYTES;
        assert_eq!(frame[..HEADER_BYTES], (body_bytes as u32).to_le_bytes());
    }

    #[test]
    fn an_oversized_announcement_is_refused_before_allocating() {
        // The attack is four bytes of header asking for a gigabyte of buffer.
        let header = u32::MAX.to_le_bytes();
        assert_eq!(
            body_len(&header),
            Err(FrameError::TooLarge {
                announced: u32::MAX as u64
            })
        );
    }

    #[test]
    fn an_empty_body_is_not_a_frame() {
        assert_eq!(body_len(&0u32.to_le_bytes()), Err(FrameError::Empty));
    }

    #[test]
    fn a_malformed_body_never_quotes_itself() {
        // serde_json would say: invalid type: string "hunter2", expected u32.
        let body = br#"{"n":"hunter2","s":"x"}"#;
        let err = decode::<Probe>(body).expect_err("must not parse");
        let printed = err.to_string();
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(matches!(err, FrameError::Malformed { .. }), "{err:?}");
    }
}
