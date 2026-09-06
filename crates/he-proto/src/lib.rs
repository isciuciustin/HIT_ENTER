//! Types and rules shared by both sides of a HIT_ENTER connection.
//!
//! This crate performs **no I/O** and depends on no runtime. It is the single
//! place a type that crosses the wire may be defined — see `docs/PLAN.md` §7.

pub mod limits;

/// ALPN identifying the HIT_ENTER protocol to iroh.
///
/// Bump this on any breaking protocol change so that incompatible peers fail
/// to connect rather than misinterpreting each other.
pub const ALPN: &[u8] = b"hit-enter/0";

/// Protocol revision carried in the handshake.
pub const PROTOCOL_VERSION: u32 = 0;
