//! Opus at 48 kHz, mono, 20 ms frames (S3's `callme` numbers): speech at
//! ~32 kbit/s, in-band FEC on so one lost packet is rebuilt from the next.

use opus::{Application, Bitrate, Channels, Decoder, Encoder};

/// Samples per second.
pub const RATE: u32 = 48_000;
/// Samples in one 20 ms frame.
pub const FRAME: usize = 960;
/// The most an encoded frame may take; far more than 32 kbit/s needs.
pub const MAX_PACKET: usize = 1_000;

#[derive(Debug, thiserror::Error)]
#[error("opus: {0}")]
pub struct CodecError(String);

fn err(e: opus::Error) -> CodecError {
    CodecError(e.to_string())
}

pub struct Enc(Encoder);

impl Enc {
    pub fn new() -> Result<Self, CodecError> {
        let mut e = Encoder::new(RATE, Channels::Mono, Application::Voip).map_err(err)?;
        e.set_bitrate(Bitrate::Bits(32_000)).map_err(err)?;
        e.set_inband_fec(true).map_err(err)?;
        e.set_packet_loss_perc(10).map_err(err)?;
        Ok(Self(e))
    }

    /// One frame of `FRAME` samples in, one packet out.
    pub fn encode(&mut self, frame: &[f32]) -> Result<Vec<u8>, CodecError> {
        let mut out = vec![0u8; MAX_PACKET];
        let n = self.0.encode_float(frame, &mut out).map_err(err)?;
        out.truncate(n);
        Ok(out)
    }
}

pub struct Dec(Decoder);

impl Dec {
    pub fn new() -> Result<Self, CodecError> {
        Ok(Self(Decoder::new(RATE, Channels::Mono).map_err(err)?))
    }

    /// One packet in, one frame out.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<f32>, CodecError> {
        self.run(packet, false)
    }

    /// The frame before `next`, rebuilt from `next`'s FEC data — when a
    /// packet was lost and the following one has arrived.
    pub fn recover(&mut self, next: &[u8]) -> Result<Vec<f32>, CodecError> {
        self.run(next, true)
    }

    /// A frame of concealment, when nothing arrived at all.
    pub fn conceal(&mut self) -> Result<Vec<f32>, CodecError> {
        self.run(&[], false)
    }

    fn run(&mut self, packet: &[u8], fec: bool) -> Result<Vec<f32>, CodecError> {
        let mut out = vec![0f32; FRAME];
        let n = self.0.decode_float(packet, &mut out, fec).map_err(err)?;
        out.truncate(n);
        out.resize(FRAME, 0.0);
        Ok(out)
    }
}

/// The level of a frame, 0..1: what drives the speaking indicator and voice
/// activation.
pub fn level(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tone(freq: f32, frame_no: usize) -> Vec<f32> {
        (0..FRAME)
            .map(|i| {
                let t = (frame_no * FRAME + i) as f32 / RATE as f32;
                0.3 * (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    #[test]
    fn speech_sized_packets_and_a_tone_survives_the_round_trip() {
        let (mut e, mut d) = (Enc::new().unwrap(), Dec::new().unwrap());
        let mut last = Vec::new();
        for n in 0..50 {
            let p = e.encode(&tone(440.0, n)).unwrap();
            assert!(p.len() < 200, "{} bytes for 20 ms", p.len());
            last = d.decode(&p).unwrap();
        }
        assert!((level(&last) - level(&tone(440.0, 49))).abs() < 0.08);
        // Concealment and recovery give frames, not errors.
        assert_eq!(d.conceal().unwrap().len(), FRAME);
        let p = e.encode(&tone(440.0, 50)).unwrap();
        assert_eq!(d.recover(&p).unwrap().len(), FRAME);
    }
}
