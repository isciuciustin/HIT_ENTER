//! Publishing what is public anyway to public Nostr relays (M13): today a
//! person's profile, so their name shows in Primal, Damus and the rest.
//!
//! Publish only — never a subscription, never chat. The relays see the
//! event (signed and public by design) and this machine's IP.

use std::time::Duration;

use futures_util::future::join_all;
use nostr::prelude::Event;

use crate::{Error, relay::Relay};

/// Where a profile goes by default: large, long-lived relays that most
/// Nostr apps read. Editable in the app.
pub const PUBLIC_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
];

/// How long one relay has to connect and answer `OK`.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(15);

/// Publishes `event` to every relay in `relays` at once: each one's
/// answer, in the same order.
pub async fn publish(event: &Event, relays: &[String]) -> Vec<Result<(), Error>> {
    join_all(relays.iter().map(|url| async move {
        let mut r = Relay::connect(url).await?;
        r.publish(event, PUBLISH_TIMEOUT).await
    }))
    .await
}
