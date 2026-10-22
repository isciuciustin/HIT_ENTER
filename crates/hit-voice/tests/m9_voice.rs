//! M9, on loopback: two members of a group meet in a voice channel and each
//! hears the other — a 440 Hz tone one way, 660 Hz the other, through Opus,
//! datagrams and the jitter buffer. A stranger who dials in is refused, and
//! a full channel refuses the next member.

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
use hit_event::{
    MemoryStore, Person,
    group::{Channel, ChannelKind, Edit},
};
use hit_voice::{
    call::{Config, Input, Output, Voice, VoiceError},
    codec::{FRAME, RATE},
    wire::{ALPN, Answer, Join},
};
use nostr::prelude::Timestamp;
use tokio::io::{AsyncBufReadExt, BufReader};

struct Tone {
    freq: f32,
    n: usize,
}

impl Input for Tone {
    fn frame(&mut self) -> Option<Vec<f32>> {
        let f = (0..FRAME)
            .map(|i| {
                let t = (self.n + i) as f32 / RATE as f32;
                0.3 * (TAU * self.freq * t).sin()
            })
            .collect();
        self.n += FRAME;
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
    /// The loudest of the given frequencies in what was heard, if anything
    /// was (Goertzel, one bin each).
    fn tone(&self, of: &[f32]) -> Option<f32> {
        let v = self.0.lock().unwrap().clone();
        if v.len() < RATE as usize / 2 {
            return None;
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
        let (f, p) = of
            .iter()
            .map(|f| (*f, power(*f)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        // A tone at 0.3 gives ~v.len()/16; silence gives ~0.
        (p > 1.0).then_some(f)
    }
}

const TONES: [f32; 3] = [440.0, 660.0, 1000.0];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_members_hear_each_other_a_stranger_and_a_third_are_refused() {
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let people: Vec<Person> = (0..4).map(|_| Person::generate()).collect();
    let (alice, bob, carol, dave) = (&people[0], &people[1], &people[2], &people[3]);
    let a = start(dirs[0].path(), alice).await;
    let b = start(dirs[1].path(), bob).await;
    let c = start(dirs[2].path(), carol).await;
    let d = start(dirs[3].path(), dave).await;
    mesh(&[&a, &b, &c, &d]);

    let g = a
        .create_group("Talk", &MemoryStore::default())
        .await
        .unwrap();
    a.edit(
        &g,
        &[Edit::Channel(Channel {
            id: "lounge".into(),
            name: "lounge".into(),
            kind: ChannelKind::Voice,
            slowmode: 0,
        })],
    )
    .await
    .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    d.join(&link).await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    a.sync_with(loopback(&d)).await.unwrap();
    b.sync_with(loopback(&d)).await.unwrap();

    let config = Config {
        max: 2,
        heartbeat: Duration::from_secs(1),
        ..Config::default()
    };
    let (va, vb, vc, vd) = (
        Voice::new(a.clone(), config),
        Voice::new(b.clone(), config),
        Voice::new(c.clone(), config),
        Voice::new(d.clone(), config),
    );

    // Carol is in no group: she may not join, and has no channel to join.
    assert!(matches!(
        vc.join(
            g,
            "lounge",
            Box::new(Tone { freq: 1000.0, n: 0 }),
            Box::new(Heard::default())
        )
        .await,
        Err(VoiceError::NotAllowed)
    ));
    // A text channel is no voice channel.
    assert!(matches!(
        va.join(
            g,
            "general",
            Box::new(Tone { freq: 1.0, n: 0 }),
            Box::new(Heard::default())
        )
        .await,
        Err(VoiceError::NotAllowed)
    ));

    let (heard_a, heard_b) = (Heard::default(), Heard::default());
    va.join(
        g,
        "lounge",
        Box::new(Tone { freq: 440.0, n: 0 }),
        Box::new(heard_a.clone()),
    )
    .await
    .unwrap();
    vb.join(
        g,
        "lounge",
        Box::new(Tone { freq: 660.0, n: 0 }),
        Box::new(heard_b.clone()),
    )
    .await
    .unwrap();

    assert!(
        eventually(Duration::from_secs(20), || async {
            heard_a.tone(&TONES) == Some(660.0) && heard_b.tone(&TONES) == Some(440.0)
        })
        .await,
        "Alice heard {:?}, Bob heard {:?}",
        heard_a.tone(&TONES),
        heard_b.tone(&TONES)
    );
    let state = va.state().unwrap();
    assert_eq!(state.people.len(), 1);
    assert!(state.people[0].connected && state.people[0].speaking);

    // The owner mutes Bob mid-call: within a couple of seconds his voice
    // stops reaching Alice, and his own call knows he cannot be heard.
    a.mute(&g, &bob.public_key(), Some(Timestamp::now() + 3600))
        .await
        .unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            heard_a.tone(&TONES).is_none() && !vb.state().unwrap().can_speak
        })
        .await,
        "a muted member is still heard"
    );
    // Alice is still heard by Bob.
    assert_eq!(heard_b.tone(&TONES), Some(440.0));
    a.mute(&g, &bob.public_key(), None).await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            heard_a.tone(&TONES) == Some(660.0)
        })
        .await,
        "an unmuted member is not heard again"
    );

    // Carol dials Alice's call directly: refused, she is no member.
    let conn = c.endpoint().connect(loopback(&a), ALPN).await.unwrap();
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let join = Join::Channel {
        group: g.to_hex(),
        channel: "lounge".into(),
    };
    let mut line = serde_json::to_vec(&join).unwrap();
    line.push(b'\n');
    send.write_all(&line).await.unwrap();
    let mut answer = String::new();
    BufReader::new(recv).read_line(&mut answer).await.unwrap();
    let answer: Answer = serde_json::from_str(answer.trim()).unwrap();
    assert!(!answer.ok, "a stranger was let into the call");

    // Dave is a member, but the call (max 2) is full.
    assert!(
        eventually(Duration::from_secs(10), || async {
            vd.participants(&g, "lounge").len() == 2
        })
        .await,
        "Dave never saw who is in the call"
    );
    assert!(matches!(
        vd.join(
            g,
            "lounge",
            Box::new(Tone { freq: 1000.0, n: 0 }),
            Box::new(Heard::default())
        )
        .await,
        Err(VoiceError::Full(2))
    ));

    // Bob leaves: Alice's call empties, and she hears nothing more.
    vb.leave().await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            va.state().unwrap().people.is_empty()
        })
        .await,
        "Alice still sees Bob in the call"
    );
    assert!(
        eventually(Duration::from_secs(5), || async {
            heard_a.tone(&TONES).is_none()
        })
        .await,
        "Alice still hears a tone"
    );
    va.leave().await.unwrap();
}
