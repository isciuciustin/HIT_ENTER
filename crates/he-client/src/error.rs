//! One error type for the client.
//!
//! The interesting variant is [`ClientError::Refused`], which carries the
//! server's own [`ProtocolError`] unchanged. A client must be able to tell
//! "wrong password" from "this device was revoked" from "back off for 30
//! seconds", because those need three different things from the user — and a
//! client is the one place where showing the reason is the correct behaviour.

use he_proto::rpc::ProtocolError;
use he_proto::{ErrorCode, FrameError};
use thiserror::Error;

pub type Result<T, E = ClientError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ClientError {
    /// The server answered the handshake, or a request, with `error`.
    #[error("server refused the request: {:?}", .0.code)]
    Refused(ProtocolError),

    /// Could not reach the server at all: it is offline, the `EndpointId` is
    /// wrong, or nothing could be punched or relayed to it.
    #[error("could not connect: {0}")]
    Connect(String),

    #[error("connection failed: {0}")]
    Transport(String),

    #[error(transparent)]
    Frame(#[from] FrameError),

    /// The server said something legal but not what this point in the
    /// conversation allows — an event before `ready`, a `messages` answer to a
    /// `send`.
    #[error("unexpected {0} frame")]
    Unexpected(&'static str),

    #[error(transparent)]
    Validation(#[from] he_proto::limits::ValidationError),

    /// The device key file exists but is not a 32-byte key.
    #[error("device identity at {0} is corrupt")]
    CorruptIdentity(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl ClientError {
    /// The server's code, when there is one. Lets a caller match on
    /// [`ErrorCode::DeviceNotEnrolled`] to fall back to a password prompt
    /// without stringly-typing the error.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Refused(err) => Some(err.code),
            _ => None,
        }
    }

    /// Seconds to wait, when the server asked for a wait.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            Self::Refused(err) => err.retry_after.map(std::time::Duration::from_secs),
            _ => None,
        }
    }
}
