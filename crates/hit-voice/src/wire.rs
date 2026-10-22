//! `hit/voice/1`: one QUIC connection per pair of people in a call (M9).
//!
//! **The join**, on the first bi-stream, as one JSON line each way. From the
//! dialler, either `{"group": <hex>, "channel": <id>}` — a voice channel —
//! or `{"dm": <binding>}` — a 1:1 call, carrying the dialling device's
//! binding (kind 30436), since the listener may share no group with them
//! and so hold none. Back, `{"ok": true}` or `{"ok": false, "why": "..."}`.
//! The peer is checked by the listener — its `EndpointId` from the
//! connection, mapped to a person by a binding — and only then are
//! datagrams read. A 1:1 call's answer waits for the person to pick up.
//!
//! **Audio**, as QUIC datagrams (unreliable, unordered — what voice wants):
//! `[1 version][2 seq, big-endian][1 flags][opus packet]`. `seq` counts
//! 20 ms frames and wraps; flag bit 0 is "speaking" (voice activity), for
//! the indicator.

use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"hit/voice/1";
pub const VERSION: u8 = 1;
pub const SPEAKING: u8 = 1;
/// The most one join line may be: room for a binding event.
pub const MAX_JOIN: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Join {
    /// Into voice channel `channel` of group `group` (hex).
    Channel { group: String, channel: String },
    /// A 1:1 call, with the caller's device binding.
    Dm { dm: nostr::prelude::Event },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub seq: u16,
    pub speaking: bool,
    pub opus: Vec<u8>,
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.opus.len());
        out.push(VERSION);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.push(if self.speaking { SPEAKING } else { 0 });
        out.extend_from_slice(&self.opus);
        out
    }

    /// `None` for anything that is not a version-1 packet with audio in it.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [VERSION, a, b, flags, opus @ ..] if !opus.is_empty() => Some(Self {
                seq: u16::from_be_bytes([*a, *b]),
                speaking: flags & SPEAKING != 0,
                opus: opus.to_vec(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_round_trips_and_junk_is_nothing() {
        let p = Packet {
            seq: 65_535,
            speaking: true,
            opus: vec![1, 2, 3],
        };
        assert_eq!(Packet::decode(&p.encode()), Some(p));
        assert_eq!(Packet::decode(&[2, 0, 0, 0, 9]), None);
        assert_eq!(Packet::decode(&[1, 0, 0, 0]), None);
        assert_eq!(Packet::decode(&[]), None);
    }
}
