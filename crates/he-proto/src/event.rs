//! The things a chat is made of, and what the server pushes about them.
//!
//! These structs are the wire shapes for rows in `server.db` — a client never
//! sees the database, only these. Fields a client cannot act on (a password
//! hash, an invite's creator) are simply absent, which is the cheapest kind of
//! access control there is.

use serde::{Deserialize, Serialize};

use crate::rpc::{ProtocolError, Ready};

/// One message, as everyone else sees it.
///
/// `content` is plaintext, here and at rest, by design (PLAN §10). It is
/// encrypted *in transit* by QUIC + TLS 1.3 for its entire journey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// UUIDv7, so this is also the creation time and also the sort key.
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    /// Denormalised so that rendering a backfilled page needs no second
    /// lookup, and so the client mirror can answer offline after an account
    /// has been renamed or removed.
    pub author_name: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<i64>,
    /// Soft delete. Clients are *told* rather than silently skipped, because a
    /// message already on someone's screen has to be taken back off it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub position: i64,
}

/// An account on this server, as other members see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub is_owner: bool,
}

/// Everything the server writes on the **control stream**, in order: exactly
/// one [`ServerFrame::Ready`] or [`ServerFrame::Error`] answering the `Hello`,
/// and then events until the connection ends (PLAN §9).
///
/// One enum rather than a handshake type and an event type, because it is one
/// stream and a reader has to be able to decode whatever arrives next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerFrame {
    /// The handshake succeeded. Carries enough to paint the whole app.
    Ready(Ready),
    /// The handshake failed, or the session is being torn down.
    Error(ProtocolError),
    /// A message was posted, by anyone — including by this connection.
    Message {
        message: Message,
        /// Echoed back **only** to the connection that sent it, so that client
        /// can swap its optimistic bubble for the authoritative row. Everyone
        /// else gets the same event without one (PLAN §9).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::ErrorCode;

    #[test]
    fn a_message_event_is_shaped_as_documented() {
        let frame = ServerFrame::Message {
            message: Message {
                id: "0199".into(),
                channel_id: "c1".into(),
                author_id: "u1".into(),
                author_name: "justin".into(),
                content: "hi".into(),
                edited_at: None,
                deleted_at: None,
            },
            nonce: Some("n1".into()),
        };
        let json = serde_json::to_value(&frame).expect("serialisable");
        assert_eq!(json["t"], "message");
        assert_eq!(json["message"]["content"], "hi");
        assert_eq!(json["nonce"], "n1");
        // Absent, not null: a client reading `edited_at` must see "never
        // edited", and PROTOCOL.md documents the field as optional.
        assert!(json["message"].get("edited_at").is_none());
    }

    #[test]
    fn an_error_frame_is_shaped_as_documented() {
        let json = serde_json::to_value(ServerFrame::Error(ProtocolError::new(
            ErrorCode::BadCredentials,
        )))
        .expect("serialisable");
        assert_eq!(json["t"], "error");
        assert_eq!(json["code"], "BAD_CREDENTIALS");
    }

    #[test]
    fn frames_round_trip() {
        let frame = ServerFrame::Error(ProtocolError::rate_limited(30));
        let bytes = serde_json::to_vec(&frame).expect("serialisable");
        assert_eq!(
            serde_json::from_slice::<ServerFrame>(&bytes).expect("parsable"),
            frame
        );
    }
}
