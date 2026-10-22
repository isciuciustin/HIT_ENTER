//! The jitter buffer: packets in, in any order and with gaps; one frame out
//! every 20 ms, on time.
//!
//! It waits `delay` frames before playing (60 ms by default), then pops one
//! frame per tick: the packet with the next `seq`, or — if that is missing —
//! a recovery from the one after it (Opus in-band FEC) when it is here, or
//! concealment when it is not. A packet that arrives after its turn is
//! dropped. If the buffer runs dry for a while it starts over, waiting
//! `delay` again, rather than playing a stream of concealment.
//!
//! **And it catches up**: if more than [`CATCH_UP`] frames beyond `delay`
//! pile up — the sender's clock a touch fast through a long speech, or this
//! machine stalled and its ticks were skipped — it skips ahead to `delay`
//! behind the newest, once, rather than carrying that latency for the rest
//! of the call. (A pause in speech resets it anyway: voice activation stops
//! sending, the buffer runs dry, and it starts over.)

use std::collections::BTreeMap;

/// What to play for one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// Not started (or starved and starting over): silence.
    Waiting,
    /// This packet.
    Play(Vec<u8>),
    /// Lost; rebuild it from this, the next packet's, FEC.
    Recover(Vec<u8>),
    /// Lost, and nothing to rebuild it from: conceal.
    Conceal,
}

pub struct Jitter {
    delay: usize,
    held: BTreeMap<u32, Vec<u8>>,
    /// The next sequence number to play, unwrapped; `None` until started.
    next: Option<u32>,
    /// The newest unwrapped seq seen, to unwrap the next one.
    newest: Option<u32>,
    /// Ticks in a row with nothing to play.
    dry: usize,
}

/// Concealment ticks in a row before starting over.
const MAX_DRY: usize = 10;
/// Most packets held: two seconds.
const MAX_HELD: usize = 100;
/// Frames beyond `delay` that make it skip ahead: 120 ms.
pub const CATCH_UP: usize = 6;

impl Jitter {
    pub fn new(delay: usize) -> Self {
        Self {
            delay: delay.max(1),
            held: BTreeMap::new(),
            next: None,
            newest: None,
            dry: 0,
        }
    }

    /// `seq` wraps at 65536; this keeps it monotonic, near the newest.
    fn unwrap(&self, seq: u16) -> u32 {
        let Some(n) = self.newest else {
            return u32::from(seq) + 65_536;
        };
        let base = n & !0xffff;
        let candidates = [base.wrapping_sub(65_536), base, base + 65_536];
        candidates
            .into_iter()
            .map(|b| b + u32::from(seq))
            .min_by_key(|c| c.abs_diff(n))
            .unwrap_or(n)
    }

    pub fn push(&mut self, seq: u16, packet: Vec<u8>) {
        let s = self.unwrap(seq);
        if self.next.is_some_and(|n| s < n) {
            return; // its turn has passed
        }
        self.newest = Some(self.newest.map_or(s, |n| n.max(s)));
        self.held.insert(s, packet);
        while self.held.len() > MAX_HELD {
            self.held.pop_first();
        }
    }

    /// One tick: what to play now.
    pub fn pop(&mut self) -> Slot {
        let Some(next) = self.next else {
            if self.held.len() < self.delay {
                return Slot::Waiting;
            }
            let first = self.held.keys().next().copied().unwrap_or_default();
            self.next = Some(first);
            return self.pop();
        };
        // Too far behind the newest: skip to `delay` behind it.
        let next = match self.newest {
            Some(newest) if newest + 1 > next + (self.delay + CATCH_UP) as u32 => {
                let to = newest + 1 - self.delay as u32;
                self.held.retain(|s, _| *s >= to);
                to
            }
            _ => next,
        };
        self.next = Some(next + 1);
        if let Some(p) = self.held.remove(&next) {
            self.dry = 0;
            return Slot::Play(p);
        }
        self.dry += 1;
        if self.dry > MAX_DRY || self.held.is_empty() && self.dry > 2 {
            // Starved: start over, waiting for `delay` packets again.
            self.next = None;
            self.dry = 0;
            return Slot::Waiting;
        }
        match self.held.get(&(next + 1)) {
            Some(p) => Slot::Recover(p.clone()),
            None => Slot::Conceal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pile_up_is_skipped_once_and_the_delay_comes_back() {
        let mut j = Jitter::new(3);
        for s in 0..3 {
            j.push(s, p(s as u8));
        }
        assert_eq!(j.pop(), Slot::Play(p(0)));
        // A stall: 20 frames arrive at once.
        for s in 3..23 {
            j.push(s, p(s as u8));
        }
        // It skips to 3 behind the newest (22) and plays on from there.
        assert_eq!(j.pop(), Slot::Play(p(20)));
        assert_eq!(j.pop(), Slot::Play(p(21)));
        assert_eq!(j.pop(), Slot::Play(p(22)));
        // A small surplus (within CATCH_UP) is played, not skipped.
        for s in 23..30 {
            j.push(s, p(s as u8));
        }
        assert_eq!(j.pop(), Slot::Play(p(23)));
    }

    fn p(n: u8) -> Vec<u8> {
        vec![n]
    }

    #[test]
    fn waits_then_plays_in_order_whatever_the_arrival_order() {
        let mut j = Jitter::new(3);
        j.push(2, p(2));
        assert_eq!(j.pop(), Slot::Waiting);
        j.push(0, p(0));
        j.push(1, p(1));
        assert_eq!(j.pop(), Slot::Play(p(0)));
        assert_eq!(j.pop(), Slot::Play(p(1)));
        assert_eq!(j.pop(), Slot::Play(p(2)));
    }

    #[test]
    fn a_lost_packet_is_recovered_from_the_next_or_concealed() {
        let mut j = Jitter::new(1);
        j.push(10, p(10));
        j.push(12, p(12));
        j.push(15, p(15));
        assert_eq!(j.pop(), Slot::Play(p(10)));
        assert_eq!(j.pop(), Slot::Recover(p(12)), "11 rebuilt from 12's FEC");
        assert_eq!(j.pop(), Slot::Play(p(12)));
        assert_eq!(j.pop(), Slot::Conceal, "13: nothing after it yet");
        // A packet for a turn that has passed is dropped.
        j.push(13, p(13));
        assert_eq!(j.pop(), Slot::Recover(p(15)), "14 rebuilt from 15");
        assert_eq!(j.pop(), Slot::Play(p(15)));
    }

    #[test]
    fn sequence_numbers_wrap() {
        let mut j = Jitter::new(2);
        j.push(65_534, p(1));
        j.push(65_535, p(2));
        j.push(0, p(3));
        j.push(1, p(4));
        let got: Vec<Slot> = (0..4).map(|_| j.pop()).collect();
        assert_eq!(
            got,
            vec![
                Slot::Play(p(1)),
                Slot::Play(p(2)),
                Slot::Play(p(3)),
                Slot::Play(p(4))
            ]
        );
    }

    #[test]
    fn a_starved_buffer_starts_over_instead_of_concealing_forever() {
        let mut j = Jitter::new(2);
        j.push(0, p(0));
        j.push(1, p(1));
        assert_eq!(j.pop(), Slot::Play(p(0)));
        assert_eq!(j.pop(), Slot::Play(p(1)));
        let rest: Vec<Slot> = (0..4).map(|_| j.pop()).collect();
        assert!(rest.contains(&Slot::Waiting), "{rest:?}");
        // And resumes once enough has arrived.
        j.push(9, p(9));
        j.push(10, p(10));
        assert_eq!(j.pop(), Slot::Play(p(9)));
    }
}
