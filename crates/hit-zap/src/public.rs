//! What is public anyway, on public Nostr relays (M13): a person's
//! profile, so their name shows in Primal, Damus and the rest, and public
//! groups' announcements, so strangers find them.
//!
//! Never chat. Published: the person's profile and their public groups'
//! announcements. Read back: announcements, for *browse communities* —
//! stored events only, never a live subscription. The relays see what is
//! sent (signed and public by design), what is asked, and this machine's
//! IP.

use std::time::Duration;

use futures_util::future::join_all;
use nostr::prelude::{Event, Filter};

use crate::{
    Error,
    relay::{Relay, sub_id},
};

/// Where profiles and announcements go, and announcements are read from,
/// by default: large, long-lived relays that most Nostr apps read.
/// Editable in the app.
pub const PUBLIC_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
];

/// The most events read from one relay for one question.
const MAX_FETCHED: usize = 1000;

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

/// The stored events matching `filter` on every relay in `relays`, asked
/// at once, until each says `EOSE` or `timeout` passes: what arrived by
/// then, all relays together, duplicates included. **Unverified** — the
/// caller checks every one.
pub async fn fetch(relays: &[String], filter: &Filter, timeout: Duration) -> Vec<Event> {
    let each = relays.iter().map(|url| async move {
        let mut got = Vec::new();
        let ask = async {
            let mut r = Relay::connect(url).await?;
            let id = sub_id();
            r.subscribe(&id, filter.clone()).await?;
            while got.len() < MAX_FETCHED {
                match r.next_event(&id, true).await? {
                    Some(e) => got.push(e),
                    None => break,
                }
            }
            r.close(&id).await;
            Ok::<_, Error>(())
        };
        match tokio::time::timeout(timeout, ask).await {
            Ok(Err(e)) => tracing::info!("{e}"),
            Err(_) => tracing::info!("relay {url}: slow, {} events by then", got.len()),
            Ok(Ok(())) => {}
        }
        got
    });
    join_all(each).await.into_iter().flatten().collect()
}
