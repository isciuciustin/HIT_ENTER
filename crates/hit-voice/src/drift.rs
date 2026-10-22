//! Drift compensation: two clocks that never quite agree.
//!
//! A sound card fills (or drains) a buffer at its own crystal's rate; the
//! call's 20 ms tick drains (or fills) it at the system clock's. They differ
//! by tens to thousands of parts per million — S3 measured a Bluetooth
//! headset drifting audibly within seconds. Left alone the buffer creeps
//! until it overflows (audio dropped: a skip) or runs dry (silence: a
//! click), and latency wanders in between.
//!
//! [`Drift`] watches the buffer's level, smoothed, and returns a resampling
//! **ratio** within ±0.5 % that holds it at a target: above the target, the
//! side feeding the buffer resamples to fewer samples (ratio > 1); below
//! it, to more. Proportional on the level, plus a slow integral that takes
//! out the steady offset a constant drift would otherwise leave. 0.5 % is
//! inaudible as pitch (a twentieth of a semitone) and covers any real
//! device by a factor of ten.
//!
//! It is pure arithmetic — the device module feeds it buffer levels and
//! hands its ratio to the resampler at each clock boundary.

/// The furthest from 1 the ratio goes.
pub const MAX_SKEW: f64 = 0.005;
/// Smoothing of the level: each reading moves the average this far.
const ALPHA: f64 = 0.02;
/// Ratio per unit of relative level error.
const GAIN: f64 = 0.02;
/// Integral gain, per reading.
const KI: f64 = 0.000_2;

#[derive(Debug, Clone)]
pub struct Drift {
    target: f64,
    avg: Option<f64>,
    integral: f64,
    ratio: f64,
}

impl Drift {
    /// A controller holding a buffer at `target` samples.
    pub fn new(target: usize) -> Self {
        Self {
            target: target.max(1) as f64,
            avg: None,
            integral: 0.0,
            ratio: 1.0,
        }
    }

    /// One reading of the buffer's level; returns the ratio to resample
    /// the buffer's input at.
    pub fn update(&mut self, level: usize) -> f64 {
        let l = level as f64;
        let avg = self.avg.map_or(l, |a| a + ALPHA * (l - a));
        self.avg = Some(avg);
        let err = (avg - self.target) / self.target;
        self.integral = (self.integral + KI * err).clamp(-MAX_SKEW, MAX_SKEW);
        self.ratio = (1.0 + GAIN * err + self.integral).clamp(1.0 - MAX_SKEW, 1.0 + MAX_SKEW);
        self.ratio
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codec::FRAME, device::Linear};

    struct Run {
        underruns: usize,
        overflows: usize,
        /// Lowest and highest level after the first minute.
        low: usize,
        high: usize,
        ratio: f64,
    }

    /// Ten simulated minutes of a microphone whose clock runs `skew` fast
    /// (10 ms callbacks), feeding a buffer the call's tick drains a frame
    /// at a time every 20 ms — with the controller, or without.
    fn simulate(skew: f64, controlled: bool) -> Run {
        let target = 2 * FRAME;
        let cap = FRAME * 4;
        let mut drift = Drift::new(target);
        let mut resample = Linear::new(48_000, 48_000);
        let mut level = target;
        let mut owed = 0.0f64;
        let mut out = Vec::new();
        let mut run = Run {
            underruns: 0,
            overflows: 0,
            low: usize::MAX,
            high: 0,
            ratio: 1.0,
        };
        let ticks = 10 * 60 * 50;
        for tick in 0..ticks {
            // Two device callbacks per tick, each 10 ms of *its* clock.
            for _ in 0..2 {
                owed += 480.0 * (1.0 + skew);
                let n = owed as usize;
                owed -= n as f64;
                out.clear();
                resample.run(&vec![0.0; n], &mut out);
                level += out.len();
                if level > cap {
                    run.overflows += 1;
                    level = cap;
                }
            }
            if controlled {
                resample.set_ratio(drift.update(level));
            }
            if level >= FRAME {
                level -= FRAME;
            } else {
                run.underruns += 1;
            }
            if tick > 50 * 60 {
                run.low = run.low.min(level);
                run.high = run.high.max(level);
            }
        }
        run.ratio = drift.ratio();
        run
    }

    #[test]
    fn a_fast_or_slow_clock_is_absorbed_without_a_drop_or_a_gap() {
        for skew in [0.003, -0.003, 0.0005, -0.0005, 0.0] {
            let r = simulate(skew, true);
            assert_eq!(
                (r.underruns, r.overflows),
                (0, 0),
                "skew {skew}: {} underruns, {} overflows",
                r.underruns,
                r.overflows
            );
            // Settled: within half a frame of target either way, and the
            // ratio has found the skew.
            assert!(
                r.high - r.low <= FRAME + FRAME / 2,
                "skew {skew}: level wandered {}..{}",
                r.low,
                r.high
            );
            assert!(
                (r.ratio - (1.0 + skew)).abs() < 0.0005,
                "skew {skew}: ratio {}",
                r.ratio
            );
        }
    }

    #[test]
    fn without_it_the_same_clock_drops_or_starves() {
        let fast = simulate(0.003, false);
        assert!(fast.overflows > 100, "{} overflows", fast.overflows);
        // 0.3 % short of 10 minutes is 1.8 s — about 90 frames.
        let slow = simulate(-0.003, false);
        assert!(slow.underruns > 80, "{} underruns", slow.underruns);
    }

    #[test]
    fn the_ratio_never_leaves_its_bounds() {
        let mut d = Drift::new(1000);
        for _ in 0..100_000 {
            assert!((d.update(1_000_000) - 1.0).abs() <= MAX_SKEW + 1e-12);
        }
        for _ in 0..100_000 {
            assert!((d.update(0) - 1.0).abs() <= MAX_SKEW + 1e-12);
        }
    }
}
