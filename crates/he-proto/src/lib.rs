//! Types and rules shared by both sides of a HIT_ENTER connection.
//!
//! This crate performs **no I/O** and depends on no runtime, and it is the
//! single place a type that crosses the wire may be defined — see
//! `docs/PLAN.md` §7.
//!
//! The one exception is [`io`], behind the off-by-default `io` feature: the
//! async driver for [`frame`]'s codec, generic over [`tokio::io`] traits and
//! innocent of sockets. It lives here because writing a length-prefix loop
//! twice is how the two sides of a protocol learn to disagree. A consumer that
//! wants only the types builds without the feature and gets no runtime.

pub mod event;
pub mod frame;
#[cfg(feature = "io")]
pub mod io;
pub mod limits;
pub mod rpc;
pub mod secret;

pub use event::{Channel, Member, Message, ServerFrame};
pub use frame::FrameError;
pub use rpc::{Auth, ErrorCode, Hello, ProtocolError, Ready, Request, Response};
pub use secret::Password;

/// ALPN identifying the HIT_ENTER protocol to iroh.
///
/// Bump this on any breaking protocol change so that incompatible peers fail
/// to connect rather than misinterpreting each other.
pub const ALPN: &[u8] = b"hit-enter/0";

/// Protocol revision carried in the handshake.
pub const PROTOCOL_VERSION: u32 = 0;
