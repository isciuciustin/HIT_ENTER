//! Size and shape limits, enforced by the side that receives.
//!
//! Each one bounds how much work, memory or bandwidth a single frame from a
//! peer can make us spend. A peer that breaks one is answered with a refusal,
//! never with partial service that hides the problem.

/// Largest single frame, enforced before allocating a read buffer.
/// From `he-proto`.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Most filters one `REQ` may carry. Each is a query.
pub const MAX_FILTERS_PER_REQ: usize = 8;

/// Most events one filter of one `REQ` is answered with. A filter asking for
/// more (or not saying) gets this many, newest first; the asker pages with
/// `until`.
pub const MAX_EVENTS_PER_FILTER: usize = 500;

/// Most ids a dialler asks for in one `REQ` while fetching what negentropy
/// said it lacks. Under [`MAX_EVENTS_PER_FILTER`], so a fetch is never cut.
pub const FETCH_CHUNK: usize = 200;

/// Negentropy's own message budget, in bytes before hex. Hex doubles it on
/// the wire, which leaves a wide margin under [`MAX_FRAME_BYTES`] (S1).
pub const NEG_FRAME_LIMIT: u64 = 128 * 1024;

/// Negentropy sessions one stream may hold open at once. Each holds a copy
/// of the matching id set in memory.
pub const MAX_NEG_SESSIONS: usize = 4;

/// Longest subscription id accepted, as most NIP-01 relays do.
pub const MAX_SUBSCRIPTION_ID_BYTES: usize = 64;
