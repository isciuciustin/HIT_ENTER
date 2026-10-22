//! A call with real devices on one side: Alice's microphone and speakers
//! (`$IN` / `$OUT`, cpal, echo-cancelled) against Bob's synthetic 660 Hz
//! tone. Prints what Bob hears from Alice's microphone; record `$OUT`'s
//! monitor meanwhile to check what Alice's speakers played.
//!
//! Against null sinks, never the real microphone (CLAUDE.md) — see
//! `docs/TESTING.md` §M9 for the recipe. Without `IN` and `OUT` it refuses.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../hit-node/tests/common/mod.rs"]
mod common;

use std::{
    f32::consts::TAU,
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{loopback, loopback_addr, mesh, start};
use hit_event::{
    MemoryStore, Person,
    group::{Channel, ChannelKind, Edit},
};
use hit_voice::{
    call::{Config, Input, Output, Voice},
    codec::{FRAME, RATE, level},
    device,
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
        self.0.lock().unwrap().extend(frame);
    }
}

fn power(v: &[f32], f: f32) -> f32 {
    let w = TAU * f / RATE as f32;
    let (mut s1, mut s2) = (0f32, 0f32);
    for x in v {
        let s = x + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2) / v.len().max(1) as f32
}

#[tokio::main]
async fn main() {
    let (Ok(input), Ok(output)) = (std::env::var("IN"), std::env::var("OUT")) else {
        eprintln!("set IN and OUT to device ids (null sinks) — never the defaults");
        std::process::exit(2);
    };
    let secs: u64 = std::env::var("SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob) = (Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    mesh(&[&a, &b]);
    let g = a
        .create_group("Devices", &MemoryStore::default())
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
    a.sync_with(loopback(&b)).await.unwrap();

    let (va, vb) = (
        Voice::new(a.clone(), Config::default()),
        Voice::new(b.clone(), Config::default()),
    );
    let (mic, speaker) = device::open(
        Some(input),
        Some(output),
        std::env::var("ECHO").as_deref() != Ok("off"),
    )
    .unwrap();
    let watch = mic.watch();
    va.join(g, "lounge", Box::new(mic), Box::new(speaker))
        .await
        .unwrap();
    let heard = Heard::default();
    vb.join(
        g,
        "lounge",
        Box::new(Tone(660.0, 0)),
        Box::new(heard.clone()),
    )
    .await
    .unwrap();
    for s in 0..secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let st = va.state().unwrap();
        let v = heard.0.lock().unwrap().clone();
        let last = &v[v.len().saturating_sub(RATE as usize)..];
        println!(
            "{s}s  Alice: {} peer(s) connected, speaking={}  |  Bob hears level {:.3}, 440 Hz {:.1}, 660 Hz {:.1}  |  {:?}",
            st.people.iter().filter(|p| p.connected).count(),
            st.me_speaking,
            level(last),
            power(last, 440.0),
            power(last, 660.0),
            watch.stats(),
        );
    }
    va.leave().await.unwrap();
    vb.leave().await.unwrap();
}
