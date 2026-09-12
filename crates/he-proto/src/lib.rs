//! Types and rules shared by both sides of a HIT_ENTER connection.
//!
//! This crate performs **no I/O** and depends on no runtime, and it is the
//! single place a type that crosses the wire may be defined — see
//! `docs/PLAN.md` §7.
//!
//! Two modules are exceptions, both behind off-by-default features and both
//! for the same reason — writing the thing twice is how the two sides of a
//! protocol learn to disagree:
//!
//! - [`io`] (`io`) is the async driver for [`frame`]'s codec, generic over
//!   [`tokio::io`] traits and innocent of sockets;
//! - [`net`]'s `apply` (`net`) turns a [`net::NetworkConfig`] into a
//!   configured iroh endpoint builder, which a host needs for *both* of its
//!   endpoints.
//!
//! A consumer that wants only the types builds without either and gets no
//! runtime. [`ticket`] needs no feature: an address is not a connection.

pub mod event;
pub mod frame;
#[cfg(feature = "io")]
pub mod io;
pub mod limits;
pub mod net;
pub mod rpc;
pub mod secret;
pub mod ticket;

pub use event::{Channel, Member, Message, ServerFrame};
pub use frame::FrameError;
pub use net::{NetworkConfig, Relays};
pub use rpc::{Auth, ErrorCode, Hello, ProtocolError, Ready, Request, Response};
pub use secret::Password;
pub use ticket::{InviteLink, LinkError};

/// ALPN identifying the HIT_ENTER protocol to iroh.
///
/// Bump this on any breaking protocol change so that incompatible peers fail
/// to connect rather than misinterpreting each other.
pub const ALPN: &[u8] = b"hit-enter/0";

/// Protocol revision carried in the handshake.
pub const PROTOCOL_VERSION: u32 = 0;
