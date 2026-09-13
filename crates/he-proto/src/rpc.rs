//! The handshake and the request/response pairs (PLAN §9).
//!
//! Two stream shapes, because QUIC streams are cheap:
//!
//! - the **control stream**, one per connection, opened first and held open:
//!   [`Hello`] up, then [`ServerFrame`](crate::event::ServerFrame) down for as
//!   long as the session lasts;
//! - a **request stream** per RPC: write a [`Request`], read a [`Response`],
//!   close.
//!
//! Note what [`Hello`] does *not* contain: the client's `EndpointId`. iroh
//! proved that during the QUIC handshake, and the server reads it off the
//! connection. A self-declared identity in a message body is the classic way
//! to build an authentication bypass (PLAN §11).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::event::{Channel, DeviceInfo, InviteInfo, Member, Message};
use crate::limits::{self, ValidationError};
use crate::secret::Password;

/// First frame on the control stream, client to server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename = "hello")]
pub struct Hello {
    /// [`crate::PROTOCOL_VERSION`]. The ALPN already refuses an incompatible
    /// peer; this catches a same-ALPN skew during development.
    pub proto: u32,
    pub auth: Auth,
}

impl Hello {
    pub fn new(auth: Auth) -> Self {
        Self {
            proto: crate::PROTOCOL_VERSION,
            auth,
        }
    }
}

/// How this connection claims to be a person (PLAN §3).
///
/// The three variants are the whole login story: [`Auth::Register`] once per
/// server, [`Auth::Password`] once per machine, [`Auth::Device`] every time
/// after that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Auth {
    /// This device is already enrolled; iroh's key exchange is the whole
    /// proof. No password, ever again, on this machine.
    Device {
        /// Only needed when one device holds several accounts on one server —
        /// the client's `mirror.db` records which account it is using.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    /// Enrol this device against an existing account.
    Password {
        username: String,
        password: Password,
    },
    /// Create an account. Always invite-gated, without exception, except for
    /// the owner — who is created locally and never over the network.
    Register {
        invite: String,
        username: String,
        password: Password,
    },
}

/// The handshake succeeded: here is the space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// The space's display name, so a client can label the server rail without
    /// a second round trip.
    pub server_name: String,
    /// Who the server decided this connection is.
    pub user: Member,
    pub channels: Vec<Channel>,
    pub members: Vec<Member>,
    /// Ids of the members who have a live session right now.
    ///
    /// A snapshot, and then [`ServerFrame::Presence`](crate::event::ServerFrame::Presence)
    /// keeps it current. Without it a client that joins a quiet space shows
    /// everyone offline until somebody happens to reconnect.
    #[serde(default)]
    pub online: Vec<String>,
    /// True once this `EndpointId` is enrolled, which after a successful
    /// handshake it always is. Present so a client can tell a first join from
    /// a return visit and offer to name the device.
    pub enrolled: bool,
}

/// Why something was refused.
///
/// The codes are coarse on purpose. `BAD_CREDENTIALS` covers "no such user"
/// and "wrong password" with one answer, so a stranger cannot enumerate
/// accounts; `INVITE_INVALID` covers unknown, expired and exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Wrong username *or* wrong password. Deliberately indistinguishable.
    BadCredentials,
    /// Unknown, expired or exhausted invite code.
    InviteInvalid,
    /// Registered, but that name is taken. Safe to distinguish: the client
    /// just offered the name, so it learns nothing it did not supply.
    UsernameTaken,
    /// The owner kicked this device. Stop reconnecting.
    DeviceRevoked,
    /// This `EndpointId` has no enrolment here; log in with a password once.
    DeviceNotEnrolled,
    /// This device holds several accounts here; say which one.
    DeviceAmbiguous,
    /// Backing off. See [`ProtocolError::retry_after`].
    RateLimited,
    /// The request broke a rule in [`limits`].
    Invalid,
    /// No such channel, message, or user.
    NotFound,
    /// The account is who it says it is and still may not do that — editing
    /// somebody else's message, for instance. Distinct from
    /// [`ErrorCode::NotFound`] because the client can already see the author
    /// of every message it is looking at, so there is no oracle to protect.
    Forbidden,
    /// The owner banned this account. Stop reconnecting and say so; retrying
    /// cannot change the answer, and a client that retried forever would look
    /// like a network fault rather than a decision somebody made.
    Banned,
    /// The frame was unreadable, out of order, or the wrong protocol version.
    Protocol,
    /// The server broke. Never carries any detail — the detail is in the
    /// server's log, where it cannot become an oracle.
    Internal,
}

/// Carries no `t` of its own: it is always a variant of
/// [`ServerFrame`](crate::event::ServerFrame) or [`Response`], and the
/// enclosing enum is what tags it `"error"` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    /// Seconds to wait, on [`ErrorCode::RateLimited`] only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            retry_after: None,
        }
    }

    pub fn rate_limited(retry_after_secs: u64) -> Self {
        Self {
            code: ErrorCode::RateLimited,
            retry_after: Some(retry_after_secs),
        }
    }
}

impl From<ValidationError> for ProtocolError {
    fn from(_: ValidationError) -> Self {
        // The *reason* stays on the side that has it: a client validated with
        // the same [`limits`] functions before sending, so a rejection here
        // means a buggy or hostile client, not a user who needs advice.
        Self::new(ErrorCode::Invalid)
    }
}

/// One RPC, on its own bi-stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    /// Post a message.
    Send {
        channel_id: String,
        content: String,
        /// Client-chosen, unique per composed message. The server does not
        /// store it; it echoes it on the resulting event so the sender can
        /// match its optimistic bubble to the authoritative row.
        nonce: String,
    },
    /// A page of history, newest first, ending just before `before`.
    Backfill {
        channel_id: String,
        /// A message id; `None` starts from the newest message. Cursors are
        /// just ids because UUIDv7 sorts chronologically.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        limit: u32,
    },
    /// Rewrite a message. Only its author may.
    Edit { id: String, content: String },
    /// Withdraw a message. Only its author may.
    ///
    /// A soft delete: the row survives so that every client can be *told*, and
    /// take a message that is already on somebody's screen back off it. The
    /// content does not survive — see `docs/PROTOCOL.md` §5.
    Delete { id: String },
    /// Catch up after a reconnect, instead of reloading (PLAN §9).
    Resume {
        /// The newest message id this client holds, per channel.
        cursors: BTreeMap<String, String>,
        /// Unix seconds of the last successful sync. Messages *older* than a
        /// cursor but edited or deleted since then come back too — otherwise
        /// a message withdrawn while this client was offline stays on its
        /// screen forever.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
    },
    /// Say that this account is composing in a channel. Fire and forget:
    /// nothing is stored and nothing is guaranteed to arrive.
    Typing { channel_id: String },
    /// Create a channel. Owner only.
    CreateChannel {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<String>,
    },
    /// Delete a channel **and every message in it**. Owner only.
    ///
    /// The server refuses to delete the last one: a space with no channel has
    /// nowhere to put a message, and the next person to speak would find a
    /// dead end.
    DeleteChannel { id: String },
    /// Log an account out of every machine it is enrolled on. Owner only.
    ///
    /// Not a ban: the password still works, so the member can come back by
    /// enrolling a device again. It is the proportionate answer to a laptop
    /// left in a pub, and to a member who should read the room.
    Kick { user_id: String },
    /// Ban or un-ban an account. Owner only.
    ///
    /// A ban revokes every device *and* refuses the password, so the account
    /// cannot get back in at all. Reversible, because an irreversible action
    /// one misclick away is worse than one that can be undone.
    SetBanned { user_id: String, banned: bool },
    /// Kick one machine off. The owner may revoke anybody's device; everyone
    /// else may revoke only their own.
    RevokeDevice {
        user_id: String,
        endpoint_id: String,
    },
    /// The devices enrolled for an account. `None` means this account's own,
    /// which is the only one a member may ask about.
    Devices {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
    /// Every invite that has been minted and not revoked. Owner only.
    Invites,
    /// Delete an invite. Owner only. Accounts already made with it stay.
    RevokeInvite { code: String },
    /// Mint an invite code.
    Invite {
        /// Seconds from now; `None` never expires.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_in: Option<u64>,
        /// `None` is unlimited.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_uses: Option<i64>,
    },
}

impl Request {
    /// Checks the request against [`limits`].
    ///
    /// Called by the client for fast feedback and by the server because it can
    /// never trust a client. Both call *this* function, so the two cannot
    /// drift apart.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Send {
                channel_id,
                content,
                nonce,
            } => {
                limits::validate_id(channel_id)?;
                limits::validate_message_content(content)?;
                limits::validate_nonce(nonce)
            }
            Self::Backfill {
                channel_id,
                before,
                limit,
            } => {
                limits::validate_id(channel_id)?;
                if let Some(before) = before {
                    limits::validate_id(before)?;
                }
                limits::validate_backfill_limit(*limit)
            }
            Self::Edit { id, content } => {
                limits::validate_id(id)?;
                limits::validate_message_content(content)
            }
            Self::Delete { id } => limits::validate_id(id),
            Self::Resume { cursors, .. } => limits::validate_resume_cursors(
                cursors.iter().map(|(c, m)| (c.as_str(), m.as_str())),
            ),
            Self::Typing { channel_id } => limits::validate_id(channel_id),
            Self::CreateChannel { name, topic } => {
                limits::validate_channel_name(name)?;
                match topic {
                    Some(topic) => limits::validate_channel_topic(topic),
                    None => Ok(()),
                }
            }
            Self::DeleteChannel { id } => limits::validate_id(id),
            Self::Kick { user_id } => limits::validate_id(user_id),
            Self::SetBanned { user_id, .. } => limits::validate_id(user_id),
            Self::RevokeDevice {
                user_id,
                endpoint_id,
            } => {
                limits::validate_id(user_id)?;
                // An `EndpointId` is z-base-32, which `validate_id` already
                // accepts; the point of the check is the length bound.
                limits::validate_id(endpoint_id)
            }
            Self::Devices { user_id } => match user_id {
                Some(user_id) => limits::validate_id(user_id),
                None => Ok(()),
            },
            Self::Invites => Ok(()),
            Self::RevokeInvite { code } => limits::validate_invite_code(code),
            Self::Invite { max_uses, .. } => match max_uses {
                Some(n) if *n < 1 => Err(ValidationError::InviteUses),
                _ => Ok(()),
            },
        }
    }
}

/// The answer on a request stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Response {
    /// Accepted, nothing to return. A `send` answers with this: the message
    /// itself arrives on the control stream as an event, the same one every
    /// other member gets, so there is one code path that renders a message.
    Ok,
    /// A page of history, newest first.
    Messages {
        messages: Vec<Message>,
    },
    /// Everything this client missed, in id order across every channel it
    /// named. Includes older messages that were edited or deleted since
    /// `since`, which is why it is not just a `backfill` per channel.
    Resumed {
        messages: Vec<Message>,
        /// Channels whose gap was bigger than one answer could carry. The
        /// client pages those with `backfill` instead of believing it is
        /// caught up.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        truncated: Vec<String>,
    },
    /// The channel that was just created, so the client can select it without
    /// hunting for it by name in the `channels` event that follows.
    Channel {
        channel: Channel,
    },
    /// The devices enrolled for an account.
    Devices {
        devices: Vec<DeviceInfo>,
    },
    /// Every invite on the space.
    Invites {
        invites: Vec<InviteInfo>,
    },
    /// A freshly minted invite code, in display form (`K7QP-2M4X-9WTZ`).
    Invite {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_uses: Option<i64>,
    },
    Error(ProtocolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_is_shaped_as_documented() {
        let hello = Hello::new(Auth::Device { username: None });
        let json = serde_json::to_value(&hello).expect("serialisable");
        assert_eq!(json["t"], "hello");
        assert_eq!(json["proto"], 0);
        assert_eq!(json["auth"]["k"], "device");
    }

    #[test]
    fn hello_never_carries_an_endpoint_id() {
        // Trusting a self-declared device identity would defeat the entire
        // point of iroh proving it (PLAN §11). The field must not exist for a
        // future handler to be tempted to read.
        let hello = Hello::new(Auth::Password {
            username: "justin".into(),
            password: Password::new("correct horse"),
        });
        let json = serde_json::to_string(&hello).expect("serialisable");
        assert!(!json.contains("endpoint"), "{json}");
    }

    #[test]
    fn a_password_survives_the_wire_but_not_a_log_line() {
        let hello = Hello::new(Auth::Password {
            username: "justin".into(),
            password: Password::new("correct horse"),
        });
        let json = serde_json::to_string(&hello).expect("serialisable");
        // It has to cross the wire — that is what TLS 1.3 is protecting.
        assert!(json.contains("correct horse"));
        // It must never cross into a log.
        assert!(!format!("{hello:?}").contains("correct horse"));

        let back: Hello = serde_json::from_str(&json).expect("parsable");
        assert_eq!(back, hello);
    }

    #[test]
    fn requests_round_trip() {
        for request in [
            Request::Send {
                channel_id: "c1".into(),
                content: "hi".into(),
                nonce: "n1".into(),
            },
            Request::Backfill {
                channel_id: "c1".into(),
                before: None,
                limit: 50,
            },
            Request::Invite {
                expires_in: Some(86_400),
                max_uses: Some(10),
            },
        ] {
            let json = serde_json::to_vec(&request).expect("serialisable");
            assert_eq!(
                serde_json::from_slice::<Request>(&json).expect("parsable"),
                request
            );
        }
    }

    #[test]
    fn requests_are_validated_against_the_shared_limits() {
        assert!(
            Request::Send {
                channel_id: "c1".into(),
                content: "   ".into(),
                nonce: "n1".into(),
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::Backfill {
                channel_id: "c1".into(),
                before: None,
                limit: 100_000,
            }
            .validate()
            .is_err(),
            "an unbounded page size is a request to read the whole database \
             into memory"
        );
        assert!(
            Request::Invite {
                expires_in: None,
                max_uses: Some(0),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn error_codes_are_stable_strings() {
        // These go on the wire and into PROTOCOL.md; renaming a variant must
        // not silently rename the code a deployed client matches on.
        for (code, expected) in [
            (ErrorCode::BadCredentials, "BAD_CREDENTIALS"),
            (ErrorCode::InviteInvalid, "INVITE_INVALID"),
            (ErrorCode::DeviceRevoked, "DEVICE_REVOKED"),
            (ErrorCode::DeviceNotEnrolled, "DEVICE_NOT_ENROLLED"),
            (ErrorCode::DeviceAmbiguous, "DEVICE_AMBIGUOUS"),
            (ErrorCode::UsernameTaken, "USERNAME_TAKEN"),
            (ErrorCode::RateLimited, "RATE_LIMITED"),
            (ErrorCode::Invalid, "INVALID"),
            (ErrorCode::NotFound, "NOT_FOUND"),
            (ErrorCode::Forbidden, "FORBIDDEN"),
            (ErrorCode::Banned, "BANNED"),
            (ErrorCode::Protocol, "PROTOCOL"),
            (ErrorCode::Internal, "INTERNAL"),
        ] {
            assert_eq!(
                serde_json::to_value(code).expect("serialisable"),
                serde_json::Value::String(expected.into())
            );
        }
    }
}
