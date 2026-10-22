//! Calling someone with no group in common (M9): Bob gives Carol his
//! contact link out of band. She writes to him — the DM reaches his device
//! directly, carrying her device binding sealed inside. Her call is refused
//! until he writes back (an app rings only for someone its person wrote
//! to); then it rings, he answers, and each hears the other. What Bob
//! learned of Carol's devices survives his app restarting.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../hit-node/tests/common/mod.rs"]
mod common;

use std::{
    f32::consts::TAU,
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{eventually, loopback_addr, mesh, same_person, start};
use hit_event::Person;
use hit_voice::{
    call::{Config, Input, Output, Voice},
    codec::{FRAME, RATE},
};

struct Tone(f32, usize);

impl Input for Tone {
    fn frame(&mut self) -> Option<Vec<f32>> {
        let f = (0..FRAME)
            .map(|i| 0.3 * (TAU * self.0 * (self.1 + i) as f32 / RATE as f32).sin())
            .collect();
        self.1 += FRAME;
        Some(f)
    }
}

#[derive(Clone, Default)]
struct Heard(Arc<Mutex<Vec<f32>>>);

impl Output for Heard {
    fn play(&mut self, frame: Vec<f32>) {
        let mut v = self.0.lock().unwrap();
        v.extend(frame);
        let over = v.len().saturating_sub(RATE as usize);
        v.drain(..over);
    }
}

impl Heard {
    fn hears(&self, freq: f32) -> bool {
        let v = self.0.lock().unwrap().clone();
        if v.len() < RATE as usize / 2 {
            return false;
        }
        let w = TAU * freq / RATE as f32;
        let (mut s1, mut s2) = (0f32, 0f32);
        for x in &v {
            let s = x + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2) / v.len() as f32 > 1.0
    }
}

fn io(freq: f32) -> (Box<dyn Input>, Box<dyn Output>, Heard) {
    let heard = Heard::default();
    (Box::new(Tone(freq, 0)), Box::new(heard.clone()), heard)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_with_no_group_in_common_write_then_call() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (bob, carol) = (Person::generate(), Person::generate());
    let b = start(dirs[0].path(), &bob).await;
    let c = start(dirs[1].path(), &carol).await;
    // Addresses only (relays are off in tests): no bindings, no groups.
    mesh(&[&b, &c]);
    assert!(!b.shares_group(&carol.public_key()));
    assert!(c.devices_of(&bob.public_key()).is_empty());

    let config = Config {
        ring: Duration::from_secs(5),
        ..Config::default()
    };
    let (vb, vc) = (Voice::new(b.clone(), config), Voice::new(c.clone(), config));

    // Without a link, Carol has nobody to ring.
    let (i, o, _) = io(1.0);
    assert!(vc.call(bob.public_key(), i, o).await.is_err());

    // Bob's contact link, as text — the way it travels.
    let link = b.contact_link(Some(loopback_addr(&b))).to_string();
    assert!(link.starts_with("hit-enter:contact?p=npub1"));
    let person = c.add_contact(&link.parse().unwrap()).await.unwrap();
    assert_eq!(person, bob.public_key());
    assert_eq!(c.devices_of(&bob.public_key()), vec![b.id()]);

    // She writes; it reaches his device directly, with her binding inside.
    let first = c
        .send_dm(&bob.public_key(), "hi, it's Carol")
        .await
        .unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            b.dms().await.unwrap().iter().any(|d| d.id == first.id)
        })
        .await,
        "Carol's DM never reached Bob"
    );
    assert_eq!(b.devices_of(&carol.public_key()), vec![c.id()]);

    // She calls: refused — Bob has not written to her — and he never rang.
    let (i, o, _) = io(440.0);
    vc.call(bob.public_key(), i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async { vc.state().is_none() }).await,
        "the call was not refused"
    );
    assert_eq!(
        vc.ended().as_deref(),
        Some("not someone who may call this person")
    );
    assert!(vb.incoming().is_none());

    // He writes back — to her device, learned from her DM.
    let reply = b.send_dm(&carol.public_key(), "hi Carol").await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            c.dms().await.unwrap().iter().any(|d| d.id == reply.id)
        })
        .await,
        "Bob's reply never reached Carol"
    );

    // Now it rings, and they talk.
    let (i, o, heard_c) = io(440.0);
    vc.call(bob.public_key(), i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            vb.incoming() == Some(carol.public_key())
        })
        .await,
        "Bob's app did not ring"
    );
    let (i, o, heard_b) = io(660.0);
    vb.answer(i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            heard_b.hears(440.0) && heard_c.hears(660.0)
        })
        .await,
        "they did not hear each other"
    );
    vc.leave().await.unwrap();
    assert!(eventually(Duration::from_secs(10), || async { vb.state().is_none() }).await);

    // Bob's app restarts: Carol's device is still known.
    drop(vb);
    b.shutdown().await;
    let b = start(dirs[0].path(), &same_person(&bob)).await;
    assert_eq!(b.devices_of(&carol.public_key()), vec![c.id()]);
}
