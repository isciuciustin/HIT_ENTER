//! A voice channel not everyone can view (M9): only the owner and the
//! moderators may see "staff". Alice (owner) and Bob (moderator) meet in it
//! and hear each other — their presence handed directly to the members who
//! may view it, never on the group's gossip. Dave, a plain member, never
//! learns who is in the room, and cannot join it.

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
    group::{Channel, ChannelKind, DEFAULT_ROLE, Edit, Override},
};
use hit_group::Permissions;
use hit_voice::{
    call::{Config, Input, Output, Voice, VoiceError},
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_voice_room_works_for_who_may_see_it_and_stays_invisible_to_the_rest() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let people: Vec<Person> = (0..3).map(|_| Person::generate()).collect();
    let (alice, bob, dave) = (&people[0], &people[1], &people[2]);
    let a = start(dirs[0].path(), alice).await;
    let b = start(dirs[1].path(), bob).await;
    let d = start(dirs[2].path(), dave).await;
    mesh(&[&a, &b, &d]);

    let g = a
        .create_group("Crew", &MemoryStore::default())
        .await
        .unwrap();
    let view = Permissions::VIEW_CHANNEL.bits();
    a.edit(
        &g,
        &[
            Edit::Channel(Channel {
                id: "staff".into(),
                name: "staff".into(),
                kind: ChannelKind::Voice,
                slowmode: 0,
            }),
            // Members may not see it; moderators may.
            Edit::Override(Override {
                channel: "staff".into(),
                role: DEFAULT_ROLE.into(),
                allow: 0,
                deny: view,
            }),
            Edit::Override(Override {
                channel: "staff".into(),
                role: "mod".into(),
                allow: view,
                deny: 0,
            }),
        ],
    )
    .await
    .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    d.join(&link).await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    a.put(&g, &bob.public_key(), &["mod"]).await.unwrap();
    for (x, y) in [(&a, &b), (&a, &d), (&b, &d)] {
        x.sync_with(loopback(y)).await.unwrap();
    }
    let staff = |n: &hit_node::Node| {
        n.group(&g)
            .unwrap()
            .effective(&n.person(), "staff")
            .contains(Permissions::VIEW_CHANNEL)
    };
    assert!(
        staff(&a) && staff(&b) && !staff(&d),
        "the overrides did not take"
    );
    assert!(!a.group(&g).unwrap().everyone_can_view("staff"));

    let config = Config {
        heartbeat: Duration::from_secs(1),
        ..Config::default()
    };
    let (va, vb, vd) = (
        Voice::new(a.clone(), config),
        Voice::new(b.clone(), config),
        Voice::new(d.clone(), config),
    );

    // Dave cannot join a room he cannot see.
    assert!(matches!(
        vd.join(
            g,
            "staff",
            Box::new(Tone(1000.0, 0)),
            Box::new(Heard::default())
        )
        .await,
        Err(VoiceError::NotAllowed)
    ));

    let (heard_a, heard_b) = (Heard::default(), Heard::default());
    va.join(
        g,
        "staff",
        Box::new(Tone(440.0, 0)),
        Box::new(heard_a.clone()),
    )
    .await
    .unwrap();
    vb.join(
        g,
        "staff",
        Box::new(Tone(660.0, 0)),
        Box::new(heard_b.clone()),
    )
    .await
    .unwrap();
    // Each learns the other is in — without gossip — and they connect.
    assert!(
        eventually(Duration::from_secs(20), || async {
            heard_a.hears(660.0) && heard_b.hears(440.0)
        })
        .await,
        "Alice and Bob did not find each other in the private room"
    );
    assert_eq!(va.participants(&g, "staff").len(), 2);

    // Several heartbeats later, Dave still knows nothing of who is in it.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        vd.participants(&g, "staff").is_empty(),
        "a member who cannot see the room learned who is in it"
    );

    // Bob leaves: Alice learns it directly too.
    vb.leave().await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            va.participants(&g, "staff").len() == 1
        })
        .await,
        "Alice never learned Bob left"
    );
    va.leave().await.unwrap();
}
