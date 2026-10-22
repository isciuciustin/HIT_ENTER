//! Pictures in messages: decoded once, shrunk to display size, and kept in
//! a cache with a byte cap (PLAN.md M12, "pictures are where RAM goes").
//! A 12-megapixel photo is 48 MB as pixels; on screen it is at most
//! `MAX_W` × `MAX_H`, under half a megabyte — and only that is kept.

use std::collections::{HashMap, HashSet, VecDeque};

use image::{ImageReader, Limits, imageops::FilterType};
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// The largest a picture is shown, in pixels.
pub const MAX_W: u32 = 400;
pub const MAX_H: u32 = 300;

/// Shown without asking up to this size, as the web window did; above it,
/// a chip with *show*.
pub const AUTO_BYTES: u64 = 8_000_000;

/// Decoded pictures kept, in bytes of pixels: about 100 at full display
/// size. Past it the oldest not on screen go, to be decoded again if they
/// come back.
const CAP_BYTES: usize = 48 << 20;

/// The most a decoder may allocate for one file: a decompression bomb is
/// refused rather than obeyed.
const DECODE_LIMIT: u64 = 256 << 20;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// One decode at a time: each needs the full picture as pixels for a
/// moment (24 MB for a 6-megapixel photo), and fifty at once is a peak of
/// a gigabyte. Opening a channel of photos takes a little longer instead.
pub static DECODING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Bytes of a picture → its pixels at display size. Runs off the async
/// workers (`spawn_blocking`): decoding is CPU-bound.
pub fn decode(bytes: &[u8]) -> Result<Pixels, String> {
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(DECODE_LIMIT);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| format!("not a picture this app can show ({e})"))?;
    let img = if img.width() > MAX_W || img.height() > MAX_H {
        img.resize(MAX_W, MAX_H, FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(SharedPixelBuffer::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}

/// What the window knows of each picture, by key (a hash, or a DM's id).
#[derive(Default)]
pub struct Pictures {
    shown: HashMap<String, Pixels>,
    /// Oldest first, for eviction.
    order: VecDeque<String>,
    bytes: usize,
    /// "loading…", or why it could not be shown.
    notes: HashMap<String, String>,
    /// Asked for with *show* (above `AUTO_BYTES`).
    asked: HashSet<String>,
}

impl Pictures {
    pub fn get(&self, key: &str) -> Option<&Pixels> {
        self.shown.get(key)
    }

    pub fn note(&self, key: &str) -> Option<&str> {
        self.notes.get(key).map(String::as_str)
    }

    pub fn set_note(&mut self, key: &str, note: String) {
        self.notes.insert(key.to_owned(), note);
    }

    pub fn ask(&mut self, key: &str) {
        self.notes.remove(key);
        self.asked.insert(key.to_owned());
    }

    /// Whether `key` should be fetched now: wanted, and neither held,
    /// loading nor failed.
    pub fn wants(&self, key: &str, size: u64) -> bool {
        (size <= AUTO_BYTES || self.asked.contains(key))
            && !self.shown.contains_key(key)
            && !self.notes.contains_key(key)
    }

    /// Keeps `px` for `key`; drops the oldest not in `visible` while over
    /// the cap.
    pub fn insert(&mut self, key: &str, px: Pixels, visible: &HashSet<String>) {
        self.notes.remove(key);
        self.bytes += px.as_bytes().len();
        self.shown.insert(key.to_owned(), px);
        self.order.push_back(key.to_owned());
        let mut keep = VecDeque::new();
        while self.bytes > CAP_BYTES {
            let Some(k) = self.order.pop_front() else { break };
            if visible.contains(&k) {
                keep.push_back(k);
                continue;
            }
            if let Some(p) = self.shown.remove(&k) {
                self.bytes -= p.as_bytes().len();
            }
        }
        while let Some(k) = keep.pop_back() {
            self.order.push_front(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn shrinks_to_display_size_keeping_the_shape() {
        let px = decode(&png(1600, 600)).unwrap();
        assert_eq!((px.width(), px.height()), (400, 150));
        let small = decode(&png(40, 30)).unwrap();
        assert_eq!((small.width(), small.height()), (40, 30));
        assert!(decode(b"not a picture").is_err());
    }

    #[test]
    fn the_cap_spares_what_is_on_screen() {
        let mut p = Pictures::default();
        let big = || SharedPixelBuffer::<Rgba8Pixel>::new(4096, 2048); // 32 MB
        let on_screen: HashSet<String> = ["a".to_owned()].into();
        p.insert("a", big(), &on_screen);
        p.insert("b", big(), &on_screen);
        // Over the cap: b is the only one not on screen.
        assert!(p.get("a").is_some());
        assert!(p.get("b").is_none());
        assert!(p.wants("b", 10));
        assert!(!p.wants("c", AUTO_BYTES + 1));
        p.ask("c");
        assert!(p.wants("c", AUTO_BYTES + 1));
    }
}
