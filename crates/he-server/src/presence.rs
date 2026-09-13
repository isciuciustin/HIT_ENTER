//! Who is connected right now, and who is typing.
//!
//! Live-session state, so it lives in the network layer and never in
//! [`Server`](crate::Server) or the database. Presence is not durable and must
//! not be: a row saying "online" that outlived the process would be a lie the
//! next time the host's machine lost power, and the truth is already in the
//! set of open connections.
//!
//! **Presence is counted per account, not per connection.** A member with a
//! laptop and a phone is online until the second of the two goes away, which
//! is what the dot next to their name is claiming.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use he_proto::limits;

/// The live sessions, by account.
#[derive(Debug, Default)]
pub(crate) struct Presence {
    /// `user_id` → how many connections that account currently holds.
    sessions: Mutex<HashMap<String, usize>>,
    /// `connection id` → when that connection last had a typing event fanned
    /// out for it. Dropped when the session ends.
    typing: Mutex<HashMap<u64, Instant>>,
}

impl Presence {
    /// Records a new session. Returns true if this account just came online,
    /// which is the only case worth telling everybody about.
    pub(crate) fn join(&self, user_id: &str) -> bool {
        let mut sessions = self.lock_sessions();
        let count = sessions.entry(user_id.to_owned()).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Drops a session. Returns true if that was the account's last one.
    pub(crate) fn leave(&self, user_id: &str, connection_id: u64) -> bool {
        self.lock_typing().remove(&connection_id);

        let mut sessions = self.lock_sessions();
        match sessions.get_mut(user_id) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                sessions.remove(user_id);
                true
            }
            // Never registered, so nothing to take away. Reporting it offline
            // would announce a state change that did not happen.
            None => false,
        }
    }

    /// Everyone with at least one live session, for `Ready.online`.
    pub(crate) fn online(&self) -> Vec<String> {
        self.lock_sessions().keys().cloned().collect()
    }

    /// Whether this connection may fan out a typing event right now.
    ///
    /// Throttled on the server as well as the client, because a client is the
    /// one thing that cannot be trusted to throttle itself — and a typing
    /// event is a broadcast to every member, so it is the cheapest frame to
    /// send and one of the more expensive ones to deliver.
    pub(crate) fn may_type(&self, connection_id: u64) -> bool {
        let throttle = Duration::from_secs(limits::TYPING_THROTTLE_SECS);
        let now = Instant::now();
        let mut typing = self.lock_typing();
        match typing.get(&connection_id) {
            Some(last) if now.duration_since(*last) < throttle => false,
            _ => {
                typing.insert(connection_id, now);
                true
            }
        }
    }

    /// A poisoned lock here means another thread panicked while holding it.
    /// The map is a counter, not an invariant anybody reasons about, so
    /// carrying on with it is better than taking the server down.
    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, usize>> {
        self.sessions.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn lock_typing(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Instant>> {
        self.typing.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_online_until_its_last_session_goes() {
        // A laptop and a phone are one member. Announcing "offline" when the
        // laptop closes would grey out somebody who is still reading on their
        // phone.
        let presence = Presence::default();
        assert!(presence.join("u1"), "the first session is the news");
        assert!(!presence.join("u1"), "the second is not");
        assert_eq!(presence.online(), vec!["u1".to_string()]);

        assert!(!presence.leave("u1", 1), "one of two is still one");
        assert!(presence.leave("u1", 2), "that was the last one");
        assert!(presence.online().is_empty());
    }

    #[test]
    fn leaving_without_joining_announces_nothing() {
        // A connection refused during the handshake never joined, and a
        // "went offline" for it would be a state change that never happened.
        let presence = Presence::default();
        assert!(!presence.leave("ghost", 1));
    }

    #[test]
    fn typing_is_throttled_per_connection() {
        let presence = Presence::default();
        assert!(presence.may_type(1));
        assert!(!presence.may_type(1), "twice in a row is the client's bug");
        // A different connection is a different person typing.
        assert!(presence.may_type(2));
    }

    #[test]
    fn a_finished_session_forgets_its_throttle() {
        // Connection ids are handed out in sequence and never reused within a
        // run, but the map must not grow for the life of the process either.
        let presence = Presence::default();
        presence.join("u1");
        assert!(presence.may_type(1));
        presence.leave("u1", 1);
        assert!(presence.may_type(1), "a fresh session starts un-throttled");
    }
}
