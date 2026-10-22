//! Lists the audio devices; with `IN` and `OUT` set, also plays a 440 Hz tone for two seconds to
//! `$OUT` while listening on `$IN` — with echo cancellation off, then on —
//! and prints how loud the tone came back each time.
//!
//! Against a null sink, never the real microphone (CLAUDE.md):
//!
//! ```bash
//! MOD=$(pactl load-module module-null-sink sink_name=hit_spike)
//! PULSE_SINK=hit_spike PULSE_SOURCE=hit_spike.monitor IN=pulse OUT=pulse \
//!   cargo run -p hit-voice --example devices
//! pactl unload-module $MOD
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{f32::consts::TAU, time::Duration};

use hit_voice::{
    call::{Input, Output},
    codec::{FRAME, RATE, level},
    device,
};

fn main() {
    let d = device::devices();
    for (what, list) in [("in ", &d.inputs), ("out", &d.outputs)] {
        for x in list {
            println!("{what} {:<40} {}", x.id, x.name);
        }
    }
    // Never the system's defaults: those are the owner's real microphone
    // and speakers. Without both named, this only lists.
    let (Ok(input), Ok(output)) = (std::env::var("IN"), std::env::var("OUT")) else {
        println!("set IN and OUT (device ids above) to run the loop");
        return;
    };
    for echo in [false, true] {
        let (mut mic, mut spk) =
            device::open(Some(input.clone()), Some(output.clone()), echo).unwrap();
        let mut n = 0usize;
        let mut levels = Vec::new();
        for _ in 0..100 {
            let f: Vec<f32> = (0..FRAME)
                .map(|i| 0.3 * (TAU * 440.0 * (n + i) as f32 / RATE as f32).sin())
                .collect();
            n += FRAME;
            spk.play(f);
            std::thread::sleep(Duration::from_millis(20));
            while let Some(f) = mic.frame() {
                levels.push(level(&f));
            }
        }
        let tail = &levels[levels.len() / 2..];
        let avg = tail.iter().sum::<f32>() / tail.len().max(1) as f32;
        println!(
            "echo cancellation {}: {} frames back, second-half level {avg:.4} (sent 0.2121)",
            if echo { "on " } else { "off" },
            levels.len()
        );
    }
}
