//! The first frame of every `hit/nostr/1` session: the sender's device
//! binding (`docs/PROTOCOL.md`, "Session").
//!
//! `["HIT-BINDING", <kind 30436 event>]` — shaped like a NIP-01 message, so a
//! frame dump stays readable, but a name no Nostr relay uses. The dialler
//! sends it first; the listener checks it against the connection's
//! `EndpointId` and answers with its own, or closes. NIP-42 `AUTH` is not
//! used: a binding authenticates a *device*, not a challenge.

use nostr::prelude::Event;
use serde_json::Value;
use thiserror::Error;

/// The message name.
pub const HELLO: &str = "HIT-BINDING";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HelloError {
    #[error("the first frame is not a {HELLO} message")]
    NotAHello,
    #[error("the {HELLO} message does not carry an event")]
    NoEvent,
}

/// The body of a hello frame carrying `binding`.
pub fn encode(binding: &Event) -> Vec<u8> {
    // An array of a string and a struct of strings and numbers: encoding
    // cannot fail, and an empty body would be refused by the framing anyway.
    serde_json::to_vec(&(HELLO, binding)).unwrap_or_default()
}

/// The event a hello frame carries. Says nothing about whether it is a
/// valid binding — that is `hit_event::BindingBook::authenticate`'s job.
pub fn decode(body: &[u8]) -> Result<Event, HelloError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| HelloError::NotAHello)?;
    let Some([Value::String(name), event]) = value.as_array().map(Vec::as_slice) else {
        return Err(HelloError::NotAHello);
    };
    if name != HELLO {
        return Err(HelloError::NotAHello);
    }
    serde_json::from_value(event.clone()).map_err(|_| HelloError::NoEvent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{EventBuilder, FinalizeEvent, Keys, Kind};

    #[test]
    fn a_hello_round_trips() {
        let keys = Keys::generate();
        let ev = EventBuilder::new(Kind::Custom(30436), "")
            .finalize(&keys)
            .unwrap();
        let body = encode(&ev);
        assert!(body.starts_with(br#"["HIT-BINDING",{"#));
        assert_eq!(decode(&body), Ok(ev));
    }

    #[test]
    fn anything_else_is_not_a_hello() {
        assert_eq!(decode(br#"["EVENT",{}]"#), Err(HelloError::NotAHello));
        assert_eq!(decode(b"garbage"), Err(HelloError::NotAHello));
        assert_eq!(
            decode(br#"["HIT-BINDING",{"x":1}]"#),
            Err(HelloError::NoEvent)
        );
        assert_eq!(decode(br#"["HIT-BINDING"]"#), Err(HelloError::NotAHello));
    }
}
