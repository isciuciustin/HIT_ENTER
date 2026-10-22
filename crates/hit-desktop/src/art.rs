//! Pictures the software renderer cannot draw well by itself: colour emoji
//! (it draws no colour font, so Twemoji's PNGs instead — CC-BY 4.0, credited
//! in THIRD_PARTY_NOTICES).

use std::{cell::RefCell, collections::HashMap};

use slint::{ComponentHandle, Image, ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::{
    twemoji::TWEMOJI,
    ui::{Emoji, MainWindow},
};

/// HIT_ENTER's own reactions, Kick-style: green, first in the picker, and
/// the five on a message's bar. Sent as NIP-30 shortcodes (`:hit_gg:`);
/// other Nostr apps show the shortcode. `docs/PROTOCOL.md`, kind 7.
pub const SPECIALS: &[(&str, &str)] = &[
    (":hit_gg:", "😎"),
    (":hit_lol:", "😂"),
    (":hit_hype:", "🤩"),
    (":hit_love:", "😍"),
    (":hit_fire:", "🔥"),
    (":hit_pog:", "😮"),
    (":hit_sad:", "😭"),
    (":hit_rage:", "😡"),
    (":hit_wow:", "🤯"),
    (":hit_clap:", "👏"),
];

thread_local! {
    static CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

fn png(emoji: &str) -> Option<&'static [u8]> {
    TWEMOJI.iter().find(|(e, _)| *e == emoji).map(|(_, b)| *b)
}

fn decode(bytes: &[u8], green: bool) -> Option<Image> {
    let mut img = image::load_from_memory(bytes).ok()?.to_rgba8();
    if green {
        // By luminance onto the accent's ramp: dark ink, #3ddc84, mint.
        for p in img.pixels_mut() {
            let [r, g, b, a] = p.0;
            let l = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) / 255.0;
            let (lo, hi, t) = if l < 0.6 {
                ([4.0, 19.0, 10.0], [61.0, 220.0, 132.0], l / 0.6)
            } else {
                ([61.0, 220.0, 132.0], [217.0, 255.0, 233.0], (l - 0.6) / 0.4)
            };
            let mix = |i: usize| (lo[i] + (hi[i] - lo[i]) * t).round().clamp(0.0, 255.0) as u8;
            p.0 = [mix(0), mix(1), mix(2), a];
        }
    }
    let (w, h) = img.dimensions();
    Some(Image::from_rgba8(SharedPixelBuffer::clone_from_slice(
        img.as_raw(),
        w,
        h,
    )))
}

/// The picture for a reaction, or an empty image (the text is drawn then).
fn image_for(emoji: &str) -> Image {
    let key = emoji.trim().replace('\u{fe0f}', "");
    CACHE.with(|c| {
        if let Some(i) = c.borrow().get(&key) {
            return i.clone();
        }
        let made = match SPECIALS.iter().find(|(code, _)| *code == key) {
            Some((_, base)) => png(base).and_then(|b| decode(b, true)),
            None => png(&key).and_then(|b| decode(b, false)),
        }
        .unwrap_or_default();
        c.borrow_mut().insert(key, made.clone());
        made
    })
}

pub fn install(ui: &MainWindow) {
    let emoji = ui.global::<Emoji>();
    emoji.on_image(|e| image_for(&e));
    let strings = |v: Vec<&str>| {
        ModelRc::new(VecModel::from(
            v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
        ))
    };
    emoji.set_specials(strings(SPECIALS.iter().map(|(c, _)| *c).collect()));
    emoji.set_all(strings(TWEMOJI.iter().map(|(e, _)| *e).collect()));
}
