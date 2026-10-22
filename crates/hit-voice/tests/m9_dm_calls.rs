//! 1:1 calls from a DM, on loopback: Alice calls Bob, his app rings, he
//! answers and each hears the other; she hangs up and his call ends. Then
//! the ways a call does not happen — declined, unanswered, busy — each told
//! to the caller as such. And who may make an app ring at all: a stranger
//! who knows Bob's device and presents a valid binding is refused and Bob's
//! app never rings, until Bob has written to her.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../hit-node/tests/common/mod.rs"]
mod common;

use std::{
    f32::consts::TAU,
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{eventually, loopback, loopback_addr, mesh, start};
use hit_event::{MemoryStore, Person};
use hit_node::Node;
use hit_voice::{
    call::{Config, Input, Output, Voice},
    codec::{FRAME, RATE},
    wire::{ALPN, Answer, Join},
};
use tokio::io::{AsyncBufReadExt, BufReader};

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

/// Keeps the last second of what it was given.
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
    /// Whether `freq` dominates what was heard (Goertzel against 1 kHz).
    fn hears(&self, freq: f32) -> bool {
        let v = self.0.lock().unwrap().clone();
        if v.len() < RATE as usize / 2 {
            return false;
        }
        let power = |f: f32| {
            let w = TAU * f / RATE as f32;
            let (mut s1, mut s2) = (0f32, 0f32);
            for x in &v {
                let s = x + 2.0 * w.cos() * s1 - s2;
                s2 = s1;
                s1 = s;
            }
            (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2) / v.len() as f32
        };
        let p = power(freq);
        p > 1.0 && p > 10.0 * power(1000.0)
    }
}

fn io(freq: f32) -> (Box<dyn Input>, Box<dyn Output>, Heard) {
    let heard = Heard::default();
    (Box::new(Tone(freq, 0)), Box::new(heard.clone()), heard)
}

/// Dials `to` on `hit/voice/1` the way a 1:1 call does, presenting `from`'s
/// binding — a client that skips `Voice::call`'s own checks.
async fn knock(from: &Node, to: &Node) -> Answer {
    let conn = from.endpoint().connect(loopback(to), ALPN).await.unwrap();
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let mut line = serde_json::to_vec(&Join::Dm { dm: from.binding() }).unwrap();
    line.push(b'\n');
    send.write_all(&line).await.unwrap();
    let mut answer = String::new();
    BufReader::new(recv).read_line(&mut answer).await.unwrap();
    serde_json::from_str(answer.trim()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_rings_is_answered_heard_and_hung_up_and_strangers_cannot_ring() {
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let people: Vec<Person> = (0..4).map(|_| Person::generate()).collect();
    let (alice, bob, carol, dave) = (&people[0], &people[1], &people[2], &people[3]);
    let a = start(dirs[0].path(), alice).await;
    let b = start(dirs[1].path(), bob).await;
    let c = start(dirs[2].path(), carol).await;
    let d = start(dirs[3].path(), dave).await;
    mesh(&[&a, &b, &c, &d]);

    // Alice, Bob and Dave share a group, so each holds the others' device
    // bindings. Carol shares nothing with anyone.
    let g = a
        .create_group("Friends", &MemoryStore::default())
        .await
        .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    d.join(&link).await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    a.sync_with(loopback(&d)).await.unwrap();
    b.sync_with(loopback(&d)).await.unwrap();

    let config = Config {
        ring: Duration::from_secs(4),
        ..Config::default()
    };
    let (va, vb, vc, vd) = (
        Voice::new(a.clone(), config),
        Voice::new(b.clone(), config),
        Voice::new(c.clone(), config),
        Voice::new(d.clone(), config),
    );

    // --- Alice calls Bob; he answers; each hears the other. -------------
    let (i, o, heard_a) = io(440.0);
    va.call(bob.public_key(), i, o).await.unwrap();
    assert!(va.state().unwrap().calling);
    assert!(
        eventually(Duration::from_secs(10), || async {
            vb.incoming() == Some(alice.public_key())
        })
        .await,
        "Bob's app never rang"
    );
    let (i, o, heard_b) = io(660.0);
    vb.answer(i, o).await.unwrap();
    assert!(vb.incoming().is_none());
    assert!(
        eventually(Duration::from_secs(10), || async {
            heard_a.hears(660.0) && heard_b.hears(440.0)
        })
        .await,
        "they did not hear each other"
    );
    let s = va.state().unwrap();
    assert!(!s.calling && s.people.len() == 1 && s.people[0].connected);
    assert!(s.group.is_none() && s.with.is_some());

    // While they talk, Dave calls Bob: busy.
    let (i, o, _) = io(1.0);
    vd.call(bob.public_key(), i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            vd.state().is_none() && vd.ended().as_deref() == Some("busy")
        })
        .await,
        "Dave was not told Bob is busy: {:?}",
        vd.ended()
    );

    // Alice hangs up: Bob's call ends by itself.
    va.leave().await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async { vb.state().is_none() }).await,
        "Bob is still in a call with nobody"
    );
    assert_eq!(vb.ended().as_deref(), Some("they hung up"));

    // --- Declined. -------------------------------------------------------
    let (i, o, _) = io(440.0);
    va.call(bob.public_key(), i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            vb.incoming().is_some()
        })
        .await
    );
    vb.decline().unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            va.state().is_none() && va.ended().as_deref() == Some("declined")
        })
        .await,
        "Alice was not told: {:?}",
        va.ended()
    );

    // --- Unanswered: rings for `ring`, then stops on both sides. ---------
    let (i, o, _) = io(440.0);
    va.call(bob.public_key(), i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            vb.incoming().is_some()
        })
        .await
    );
    assert!(
        eventually(Duration::from_secs(15), || async {
            va.state().is_none() && vb.incoming().is_none()
        })
        .await,
        "still ringing"
    );
    assert_eq!(va.ended().as_deref(), Some("no answer"));

    // --- The caller gives up: the callee stops ringing. ------------------
    let (i, o, _) = io(440.0);
    va.call(bob.public_key(), i, o).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            vb.incoming().is_some()
        })
        .await
    );
    va.leave().await.unwrap();
    assert!(
        eventually(Duration::from_secs(5), || async { vb.incoming().is_none() }).await,
        "Bob's app rang on after Alice hung up"
    );

    // --- Who may ring. ---------------------------------------------------
    // Carol has no device of Bob's: `call` says so rather than ringing nobody.
    let (i, o, _) = io(440.0);
    assert!(vc.call(bob.public_key(), i, o).await.is_err());
    // She dials Bob's device anyway, with her own valid binding: refused,
    // and Bob's app never rang.
    let no = knock(&c, &b).await;
    assert!(!no.ok, "a stranger made Bob's app ring");
    assert!(vb.incoming().is_none());
    // Bob writes to her. Now she may ring him.
    let _ = b.send_dm(&carol.public_key(), "call me").await;
    assert!(vb.may_call(&carol.public_key()).await);
    let ring = tokio::spawn({
        let c = c.clone();
        let b = b.clone();
        async move { knock(&c, &b).await }
    });
    assert!(
        eventually(Duration::from_secs(10), || async {
            vb.incoming() == Some(carol.public_key())
        })
        .await,
        "Bob's app did not ring for someone he wrote to"
    );
    vb.decline().unwrap();
    let answer = ring.await.unwrap();
    assert_eq!(
        (answer.ok, answer.why.as_deref()),
        (false, Some("declined"))
    );
    // It is one-way: Carol received Bob's DM but wrote to nobody and shares
    // no group with him, so Bob may not make *her* app ring.
    assert!(!vc.may_call(&bob.public_key()).await);
}
