//! The real microphone and speakers: cpal streams behind [`Input`] and
//! [`Output`], with WebRTC's echo canceller between them.
//!
//! - **The streams live on their own thread** (a cpal `Stream` is not `Send`
//!   everywhere). It holds them until both the [`Mic`] and the [`Speaker`]
//!   are dropped.
//! - **Everything inside is 48 kHz mono.** A device that will not open at
//!   48 kHz is resampled (linear — fine for speech) and downmixed or
//!   duplicated across its channels.
//! - **Clock drift is absorbed** ([`crate::drift`]): the mic's and the
//!   speakers' buffers are each held at two frames (40 ms) by nudging the
//!   resampler at the device's edge — the sound card's clock and the call's
//!   tick never quite agree.
//! - **Latency stays bounded** even past that: the mic keeps at most 80 ms
//!   waiting and the speaker at most 200 ms; older audio is dropped rather
//!   than delayed — and counted ([`Stats`]).
//! - **Echo cancellation** (with noise suppression and a high-pass filter)
//!   sees every frame that is played and cleans every frame captured, in
//!   10 ms pieces, delay-agnostic. It is what lets laptop speakers and a
//!   laptop mic work at all; with a headset it is harmless. **Not on
//!   Windows yet**: the canceller's C++ does not build under MSVC, so there
//!   a call is open without it (a headset is fine; speakers echo).

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

use cpal::{
    FromSample, SampleFormat, SizedSample, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use serde::Serialize;
#[cfg(not(windows))]
use webrtc_audio_processing as wap;

/// Windows: no canceller to make — a processor that cannot exist, so every
/// `echo` stays `None` and the code around it is the same everywhere.
#[cfg(windows)]
mod wap {
    #[derive(Clone)]
    pub enum Processor {}

    impl Processor {
        pub fn process_capture_frame(&mut self, _: &mut [f32]) -> Result<(), std::convert::Infallible> {
            match *self {}
        }

        pub fn process_render_frame(&mut self, _: &mut [f32]) -> Result<(), std::convert::Infallible> {
            match *self {}
        }
    }
}

use crate::{
    call::{Input, Output},
    codec::{FRAME, RATE},
    drift::Drift,
};

/// webrtc-audio-processing's piece: 10 ms at 48 kHz.
const PIECE: usize = 480;
/// The most the mic keeps waiting before the oldest is dropped.
const MIC_MAX: usize = FRAME * 4;
/// The most the speaker keeps queued.
const SPEAKER_MAX: usize = RATE as usize / 5;
/// Where drift compensation holds each buffer: two frames, 40 ms.
const LEVEL: usize = 2 * FRAME;

/// What went wrong at the device edges since the devices were opened. With
/// drift compensation working, all four stay at zero in a steady call; a
/// count that climbs is a skip or a click someone heard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    /// Mic samples dropped because the buffer was full (the mic ran ahead).
    pub mic_dropped: u64,
    /// Ticks with no full mic frame ready (the mic fell behind).
    pub mic_short: u64,
    /// Speaker samples dropped because the queue was full.
    pub speaker_dropped: u64,
    /// Speaker callbacks that ran out and played silence.
    pub speaker_underruns: u64,
}

#[derive(Default)]
struct Counters {
    mic_dropped: AtomicU64,
    mic_short: AtomicU64,
    speaker_dropped: AtomicU64,
    speaker_underruns: AtomicU64,
}

impl Counters {
    fn stats(&self) -> Stats {
        Stats {
            mic_dropped: self.mic_dropped.load(Ordering::Relaxed),
            mic_short: self.mic_short.load(Ordering::Relaxed),
            speaker_dropped: self.speaker_dropped.load(Ordering::Relaxed),
            speaker_underruns: self.speaker_underruns.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no such audio device: {0}")]
    NoDevice(String),
    #[error("{0}")]
    Audio(String),
    #[error("echo canceller: {0}")]
    Echo(String),
}

fn audio(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Audio(e.to_string())
}

/// One device to choose from: `id` is what [`open`] takes (on Linux the
/// ALSA name — `pipewire`, `pulse`, `hw:CARD=…`), `name` what a person reads;
/// names repeat, ids do not.
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// The devices to choose from, for the settings.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Devices {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
}

fn id_of(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|i| i.id().to_owned())
}

pub fn devices() -> Devices {
    let host = cpal::default_host();
    let list = |all: Vec<cpal::Device>| -> Vec<Device> {
        all.iter()
            .filter_map(|d| {
                Some(Device {
                    id: id_of(d)?,
                    name: d.to_string(),
                })
            })
            .collect()
    };
    Devices {
        inputs: list(
            host.input_devices()
                .map(Iterator::collect)
                .unwrap_or_default(),
        ),
        outputs: list(
            host.output_devices()
                .map(Iterator::collect)
                .unwrap_or_default(),
        ),
    }
}

/// The microphone: 20 ms frames, echo-cancelled.
pub struct Mic {
    buf: Arc<Mutex<VecDeque<f32>>>,
    echo: Option<wap::Processor>,
    /// The capture side's drift correction, read by its callback (f64 bits).
    ratio: Arc<AtomicU64>,
    drift: Drift,
    counters: Arc<Counters>,
    /// The buffer has held a frame once: a short tick before that is the
    /// device starting, not falling behind.
    flowing: bool,
    _alive: mpsc::Sender<()>,
}

/// The speakers (or headphones).
pub struct Speaker {
    buf: Arc<Mutex<VecDeque<f32>>>,
    echo: Option<wap::Processor>,
    counters: Arc<Counters>,
    _alive: mpsc::Sender<()>,
}

/// Reads the devices' [`Stats`] after the [`Mic`] and [`Speaker`] have
/// been handed to a call.
#[derive(Clone)]
pub struct Watch(Arc<Counters>);

impl Watch {
    pub fn stats(&self) -> Stats {
        self.0.stats()
    }
}

impl Mic {
    /// Both devices' counts — the mic and speakers share them.
    pub fn watch(&self) -> Watch {
        Watch(self.counters.clone())
    }
}

impl Input for Mic {
    fn frame(&mut self) -> Option<Vec<f32>> {
        let mut frame: Vec<f32> = {
            let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
            // The level before taking a frame: what drift compensation holds.
            let r = self.drift.update(buf.len());
            self.ratio.store(r.to_bits(), Ordering::Relaxed);
            if buf.len() < FRAME {
                if self.flowing {
                    self.counters.mic_short.fetch_add(1, Ordering::Relaxed);
                }
                return None;
            }
            self.flowing = true;
            buf.drain(..FRAME).collect()
        };
        if let Some(p) = self.echo.as_mut() {
            for piece in frame.chunks_mut(PIECE) {
                if let Err(e) = p.process_capture_frame(piece) {
                    tracing::debug!("echo canceller (capture): {e}");
                }
            }
        }
        Some(frame)
    }
}

impl Output for Speaker {
    fn play(&mut self, frame: Vec<f32>) {
        if let Some(p) = self.echo.as_mut() {
            // The canceller learns what is about to be played; the copy it
            // works on is not what we play.
            let mut seen = frame.clone();
            for piece in seen.chunks_mut(PIECE) {
                if let Err(e) = p.process_render_frame(piece) {
                    tracing::debug!("echo canceller (render): {e}");
                }
            }
        }
        let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        buf.extend(frame);
        let over = buf.len().saturating_sub(SPEAKER_MAX);
        buf.drain(..over);
        if over > 0 {
            self.counters
                .speaker_dropped
                .fetch_add(over as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(windows)]
fn echo_canceller() -> Result<wap::Processor, DeviceError> {
    Err(DeviceError::Echo("not built on Windows yet".into()))
}

#[cfg(not(windows))]
fn echo_canceller() -> Result<wap::Processor, DeviceError> {
    let mut p = wap::Processor::new(&wap::InitializationConfig {
        num_capture_channels: 1,
        num_render_channels: 1,
        ..wap::InitializationConfig::default()
    })
    .map_err(|e| DeviceError::Echo(e.to_string()))?;
    p.set_config(wap::Config {
        echo_cancellation: Some(wap::EchoCancellation {
            suppression_level: wap::EchoCancellationSuppressionLevel::High,
            stream_delay_ms: None,
            enable_delay_agnostic: true,
            enable_extended_filter: true,
        }),
        noise_suppression: Some(wap::NoiseSuppression {
            suppression_level: wap::NoiseSuppressionLevel::Moderate,
        }),
        enable_high_pass_filter: true,
        ..wap::Config::default()
    });
    Ok(p)
}

/// Opens the microphone `input` and speakers `output` (by [`Device::id`], `None` for
/// the system's default), with echo cancellation if `echo`.
pub fn open(
    input: Option<String>,
    output: Option<String>,
    echo: bool,
) -> Result<(Mic, Speaker), DeviceError> {
    // On Windows the setting is ignored rather than refusing the call.
    let processor = if echo && cfg!(not(windows)) { Some(echo_canceller()?) } else { None };
    let mic_buf = Arc::new(Mutex::new(VecDeque::new()));
    let spk_buf = Arc::new(Mutex::new(VecDeque::new()));
    let (alive, stop) = mpsc::channel::<()>();
    let (ready_tx, ready) = mpsc::channel::<Result<(), DeviceError>>();
    let (m, s) = (mic_buf.clone(), spk_buf.clone());
    let ratio = Arc::new(AtomicU64::new(1f64.to_bits()));
    let counters = Arc::new(Counters::default());
    let (r, c1, c2) = (ratio.clone(), counters.clone(), counters.clone());
    std::thread::Builder::new()
        .name("hit-voice-audio".into())
        .spawn(move || {
            let streams = (|| {
                let host = cpal::default_host();
                let i = pick(&host, input.as_deref(), true)?;
                let o = pick(&host, output.as_deref(), false)?;
                let is = capture(&i, m, r, c1)?;
                let os = playback(&o, s, c2)?;
                is.play().map_err(audio)?;
                os.play().map_err(audio)?;
                Ok((is, os))
            })();
            match streams {
                Ok(held) => {
                    let _ = ready_tx.send(Ok(()));
                    // Until both ends are dropped.
                    let _ = stop.recv();
                    drop(held);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(audio)?;
    ready
        .recv()
        .map_err(|_| DeviceError::Audio("the audio thread died".into()))??;
    Ok((
        Mic {
            buf: mic_buf,
            echo: processor.clone(),
            ratio,
            drift: Drift::new(LEVEL),
            counters: counters.clone(),
            flowing: false,
            _alive: alive.clone(),
        },
        Speaker {
            buf: spk_buf,
            echo: processor,
            counters,
            _alive: alive,
        },
    ))
}

fn pick(host: &cpal::Host, name: Option<&str>, input: bool) -> Result<cpal::Device, DeviceError> {
    let found = match name {
        None if input => host.default_input_device(),
        None => host.default_output_device(),
        Some(n) => {
            let mut all = if input {
                host.input_devices().map_err(audio)?
            } else {
                host.output_devices().map_err(audio)?
            };
            all.find(|d| id_of(d).as_deref() == Some(n))
        }
    };
    found.ok_or_else(|| DeviceError::NoDevice(name.unwrap_or("default").to_owned()))
}

/// A config for `d`: 48 kHz if it can, else its default rate; f32 or i16.
fn config(d: &cpal::Device, input: bool) -> Result<(StreamConfig, SampleFormat), DeviceError> {
    let ranges: Vec<_> = if input {
        d.supported_input_configs().map_err(audio)?.collect()
    } else {
        d.supported_output_configs().map_err(audio)?.collect()
    };
    let usable = |f: SampleFormat| matches!(f, SampleFormat::F32 | SampleFormat::I16);
    let at_48k = [SampleFormat::F32, SampleFormat::I16]
        .into_iter()
        .find_map(|f| {
            ranges
                .iter()
                .filter(|r| r.sample_format() == f)
                .min_by_key(|r| r.channels())
                .and_then(|r| r.try_with_sample_rate(RATE))
        });
    let chosen = match at_48k {
        Some(c) => c,
        None => {
            let c = if input {
                d.default_input_config().map_err(audio)?
            } else {
                d.default_output_config().map_err(audio)?
            };
            if !usable(c.sample_format()) {
                return Err(DeviceError::Audio(format!(
                    "{d}: sample format {} not supported",
                    c.sample_format()
                )));
            }
            c
        }
    };
    Ok((chosen.config(), chosen.sample_format()))
}

fn capture(
    d: &cpal::Device,
    buf: Arc<Mutex<VecDeque<f32>>>,
    ratio: Arc<AtomicU64>,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, DeviceError> {
    let (cfg, format) = config(d, true)?;
    match format {
        SampleFormat::I16 => capture_as::<i16>(d, cfg, buf, ratio, counters),
        _ => capture_as::<f32>(d, cfg, buf, ratio, counters),
    }
}

fn capture_as<T>(
    d: &cpal::Device,
    cfg: StreamConfig,
    buf: Arc<Mutex<VecDeque<f32>>>,
    ratio: Arc<AtomicU64>,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(cfg.channels.max(1));
    let mut resample = Linear::new(cfg.sample_rate, RATE);
    let mut mono = Vec::new();
    let mut out = Vec::new();
    d.build_input_stream(
        cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            mono.clear();
            mono.extend(
                data.chunks(channels)
                    .map(|c| c.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / c.len() as f32),
            );
            out.clear();
            resample.set_ratio(f64::from_bits(ratio.load(Ordering::Relaxed)));
            resample.run(&mono, &mut out);
            let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
            b.extend(out.iter().copied());
            let over = b.len().saturating_sub(MIC_MAX);
            b.drain(..over);
            if over > 0 {
                counters
                    .mic_dropped
                    .fetch_add(over as u64, Ordering::Relaxed);
            }
        },
        |e| tracing::warn!("microphone: {e}"),
        None,
    )
    .map_err(audio)
}

fn playback(
    d: &cpal::Device,
    buf: Arc<Mutex<VecDeque<f32>>>,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, DeviceError> {
    let (cfg, format) = config(d, false)?;
    match format {
        SampleFormat::I16 => playback_as::<i16>(d, cfg, buf, counters),
        _ => playback_as::<f32>(d, cfg, buf, counters),
    }
}

fn playback_as<T>(
    d: &cpal::Device,
    cfg: StreamConfig,
    buf: Arc<Mutex<VecDeque<f32>>>,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(cfg.channels.max(1));
    let mut resample = Linear::new(RATE, cfg.sample_rate);
    let mut drift = Drift::new(LEVEL);
    // Silence before the queue first fills is starting, not an underrun.
    let mut flowing = false;
    let mut pending: VecDeque<f32> = VecDeque::new();
    let mut taken = Vec::new();
    let mut out = Vec::new();
    d.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels;
            // The level, queued and already resampled: what drift
            // compensation holds. Above it, the device is slower than the
            // tick — consume the queue a touch faster.
            let level = buf.lock().unwrap_or_else(|e| e.into_inner()).len() + pending.len();
            resample.set_ratio(drift.update(level));
            flowing |= level >= LEVEL;
            // Enough 48 kHz audio for this callback, resampled to the device.
            while pending.len() < frames {
                taken.clear();
                {
                    let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                    let n = b.len().min(PIECE);
                    taken.extend(b.drain(..n));
                }
                if taken.is_empty() {
                    break;
                }
                out.clear();
                resample.run(&taken, &mut out);
                pending.extend(out.iter().copied());
            }
            if flowing && pending.len() < frames {
                counters.speaker_underruns.fetch_add(1, Ordering::Relaxed);
            }
            for frame in data.chunks_mut(channels) {
                let s = pending.pop_front().unwrap_or(0.0);
                for x in frame {
                    *x = T::from_sample(s);
                }
            }
        },
        |e| tracing::warn!("speakers: {e}"),
        None,
    )
    .map_err(audio)
}

/// Linear resampling, one stream at a time. Fine for speech; a plain copy
/// when the rates match and no drift correction is asked for.
pub(crate) struct Linear {
    /// `from / to`, the rates' own ratio.
    base: f64,
    /// `base`, times the drift correction ([`crate::drift`]).
    step: f64,
    pos: f64,
    prev: f32,
}

impl Linear {
    pub(crate) fn new(from: u32, to: u32) -> Self {
        let base = f64::from(from) / f64::from(to.max(1));
        Self {
            base,
            step: base,
            pos: 0.0,
            prev: 0.0,
        }
    }

    /// Consumes input `ratio` times faster than the rates alone say: above
    /// 1, fewer samples out — a buffer it feeds drains.
    pub(crate) fn set_ratio(&mut self, ratio: f64) {
        self.step = self.base * ratio;
    }

    pub(crate) fn run(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < f64::EPSILON {
            out.extend_from_slice(input);
            if let Some(&last) = input.last() {
                self.prev = last;
            }
            return;
        }
        for &x in input {
            while self.pos < 1.0 {
                out.push(self.prev + (x - self.prev) * self.pos as f32);
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.prev = x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Linear;

    #[test]
    fn resampling_keeps_the_rate_ratio() {
        for (from, to) in [(44_100, 48_000), (48_000, 16_000), (24_000, 48_000)] {
            let mut r = Linear::new(from, to);
            let mut out = Vec::new();
            let input = vec![0.5f32; from as usize];
            r.run(&input, &mut out);
            let want = to as i64;
            assert!(
                (out.len() as i64 - want).abs() <= 2,
                "{from}→{to}: {} samples",
                out.len()
            );
            assert!(out[10..].iter().all(|s| (s - 0.5).abs() < 1e-6));
        }
    }
}
