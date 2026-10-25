//! The default public relays (M13), for real and read-only: each one
//! connects, answers a question with stored events and says `EOSE`.
//! Nothing is published. Needs the internet, so it is ignored by default:
//!
//! ```text
//! cargo test -p hit-zap --test real_relays -- --ignored --nocapture
//! ```

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use hit_zap::{PUBLIC_RELAYS, public::fetch};
use nostr::prelude::{Filter, Kind};

#[tokio::test]
#[ignore = "talks to the real public relays"]
async fn every_default_relay_answers_a_question() {
    for url in PUBLIC_RELAYS {
        let notes = fetch(
            &[(*url).to_owned()],
            &Filter::new().kind(Kind::TextNote).limit(3),
            Duration::from_secs(10),
        )
        .await;
        let ours = fetch(
            &[(*url).to_owned()],
            &Filter::new()
                .kind(hit_event::announce::ANNOUNCEMENT)
                .limit(20),
            Duration::from_secs(10),
        )
        .await;
        println!(
            "{url}: {} notes, {} HIT_ENTER announcements",
            notes.len(),
            ours.len()
        );
        assert!(!notes.is_empty(), "{url} answered nothing");
        assert!(notes.iter().all(|e| e.verify().is_ok()), "{url}: a bad signature");
    }
}
