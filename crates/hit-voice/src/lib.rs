//! Voice channels (M9, `docs/PLAN.md`; `DESIGN.md` §10).
//!
//! S3's verdict, built on: Opus over QUIC between every pair of people in a
//! call — a mesh, capped at 8 — with a jitter buffer, Opus FEC and
//! concealment for loss, and WebRTC's echo canceller between the speakers
//! and the microphone. **Audio is native**; the window only shows state and
//! sends button presses (WebKitGTK's WebRTC is not something to build on).
//!
//! - [`codec`]: Opus, 48 kHz mono, 20 ms.
//! - [`wire`]: `hit/voice/1` — the join, and the datagram.
//! - [`jitter`]: packets in, one frame per tick out.
//! - [`call`]: a call — the mesh, the gate, the 20 ms loop.
//! - [`device`]: the real microphone and speakers, echo-cancelled.
//! - [`drift`]: keeping a sound card's clock and the call's tick in step.

pub mod call;
pub mod codec;
pub mod device;
pub mod drift;
pub mod jitter;
pub mod wire;
