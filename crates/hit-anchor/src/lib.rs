//! The anchor: the peer that never sleeps (M6, `docs/PLAN.md`; `DESIGN.md`
//! §9).
//!
//! **The same `hit-node`, headless, with a pin policy** — one node, many
//! roles. An anchor is a *member* of each group it pins: it joins with an
//! invite like anyone, marks its profile as an anchor (NIP-24 `bot`), and
//! from then on syncs, stores and serves the group exactly as a member's
//! app does. Nothing here is a path a member's app also takes; what is here
//! is what only a headless always-on peer needs:
//!
//! - keys from systemd credentials ([`secrets`]), since a server has no
//!   keychain;
//! - compaction on a timer, when it holds a group key;
//! - every file pinned (M8): fetched, and never collected;
//! - the `ws://` bridge for NIP-29 clients ([`ws`], [`nip29`]).

pub mod nip29;
pub mod secrets;
pub mod ws;
