//! One stereo bus for the drum rack, using the application's shared hardware sink.
//!
//! Layers have 2 dB of headroom. The linked stereo compressor has no makeup gain:
//! a normal single layer stays below its threshold, while overlapping hits receive
//! gentle 2:1 compression (5 ms attack, 120 ms release). A linked peak ceiling catches
//! fast overloads. Bypass removes both stages; it retains the same input headroom.

use super::sample_pack::{SharedPcm, SlotSample};
use rodio::{
    Player, Source,
    mixer::{Mixer, MixerSource},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::time::Duration;

const RATE: u32 = 48_000;
const HEADROOM: f32 = 0.794_328_2;

pub(super) struct PadOutput {
    mixer: Mixer,
    player: Player,
    bypass: Arc<AtomicBool>,
    reduction: Arc<AtomicU32>,
}

impl PadOutput {
    pub fn new(bypass: bool) -> Option<Self> {
        Some(Self::with_player(
            crate::ui_ext::new_shared_audio_player()?,
            bypass,
        ))
    }

    /// Attaches the rack bus to an owned player, retaining the shared device lifetime.
    pub(super) fn with_player(player: Player, bypass: bool) -> Self {
        let (mixer, source) = rodio::mixer::mixer(2.try_into().unwrap(), RATE.try_into().unwrap());
        // Keep the bus attached while idle between pad hits. Its owner stops and
        // drops it when all voices finish, on navigation, or on Escape.
        mixer.add(rodio::source::Zero::new(
            2.try_into().unwrap(),
            RATE.try_into().unwrap(),
        ));
        let bypass = Arc::new(AtomicBool::new(bypass));
        let reduction = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        player.append(Glue::new(source, bypass.clone(), reduction.clone()));
        Self {
            mixer,
            player,
            bypass,
            reduction,
        }
    }

    pub fn set_bypass(&self, bypass: bool) {
        self.bypass.store(bypass, Ordering::Relaxed);
    }

    pub fn reduction_db(&self) -> f32 {
        f32::from_bits(self.reduction.load(Ordering::Relaxed))
    }

    /// Starts a ready layer without copying PCM or opening another device stream.
    pub fn play(&self, sample: &SlotSample, velocity: u8) -> Option<Player> {
        if velocity == 0 || !sample.volume.is_finite() {
            return None;
        }
        let source = SharedPcm::trimmed(sample)?;
        let player = Player::connect_new(&self.mixer);
        player.append(
            source.amplify(
                sample.volume.clamp(0.0, 2.0) * velocity.min(127) as f32 / 127.0 * HEADROOM,
            ),
        );
        Some(player)
    }
}

impl Drop for PadOutput {
    fn drop(&mut self) {
        self.player.stop();
    }
}

/// Stereo-linked dynamics; processes complete frames to preserve stereo balance.
struct Glue<S> {
    source: S,
    bypass: Arc<AtomicBool>,
    reduction: Arc<AtomicU32>,
    gain: f32,
    right: Option<f32>,
    meter_frames: u16,
}

impl<S> Glue<S> {
    fn new(source: S, bypass: Arc<AtomicBool>, reduction: Arc<AtomicU32>) -> Self {
        Self {
            source,
            bypass,
            reduction,
            gain: 1.0,
            right: None,
            meter_frames: 0,
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        let left = if left.is_finite() { left } else { 0.0 };
        let right = if right.is_finite() { right } else { 0.0 };
        let peak = left.abs().max(right.abs());
        let target = if peak > 0.85 {
            (0.85 / peak).sqrt()
        } else {
            1.0
        };
        // exp(-1 / (48000 * time)); fixed bus rate avoids per-frame exponentials.
        let coefficient = if target < self.gain {
            0.995_842
        } else {
            0.999_826_43
        };
        self.gain = target + coefficient * (self.gain - target);
        let bypass = self.bypass.load(Ordering::Relaxed);
        let gain = if bypass {
            1.0
        } else {
            self.gain.min(0.98 / peak.max(0.98))
        };
        if self.meter_frames == 0 {
            self.reduction.store(
                (-20.0 * gain.max(0.000001).log10()).max(0.0).to_bits(),
                Ordering::Relaxed,
            );
        }
        self.meter_frames = (self.meter_frames + 1) % 256;
        (left * gain, right * gain)
    }
}

impl<S: Iterator<Item = f32>> Iterator for Glue<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        let left = self.source.next()?;
        let right = self.source.next().unwrap_or(0.0);
        let (left, right) = self.frame(left, right);
        self.right = Some(right);
        Some(left)
    }
}

impl Source for Glue<MixerSource> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        2.try_into().unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        RATE.try_into().unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_output_scales_midi_velocity_without_touching_cached_pcm() {
        let mut sample = SlotSample::new("kick.wav".into(), "kick.wav".into(), None, 1.0);
        let pcm = Arc::new(vec![0.5; RATE as usize]);
        sample.cached_audio = Some(Arc::new(super::super::sample_pack::CachedAudio {
            channels: 1,
            sample_rate: RATE,
            samples: pcm.clone(),
            mono_samples: pcm.clone(),
            waveform_peaks: vec![],
        }));
        for velocity in [1, 64, 127] {
            let (player, mut samples) = Player::new();
            let bus = PadOutput::with_player(player, true);
            let voice = bus.play(&sample, velocity).unwrap();
            for _ in 0..4000 {
                samples.next();
            }
            let value = samples.next().unwrap();
            assert!((value - 0.5 * HEADROOM * velocity as f32 / 127.0).abs() < 0.00001);
            assert!(Arc::ptr_eq(
                &sample.cached_audio.as_ref().unwrap().samples,
                &pcm
            ));
            assert!(bus.play(&sample, 0).is_none());
            voice.stop();
        }
    }

    #[test]
    fn glue_preserves_single_hits_links_stereo_and_bounds_overlaps() {
        let mut glue = Glue::new(
            std::iter::empty::<f32>(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
        );
        assert_eq!(
            glue.frame(HEADROOM, HEADROOM / 2.0),
            (HEADROOM, HEADROOM / 2.0)
        );
        for _ in 0..48000 {
            let (left, right) = glue.frame(2.0, 1.0);
            assert!(left <= 0.980001 && right <= 0.490001);
            assert!((left - right * 2.0).abs() < 0.000001);
        }
        for _ in 0..96000 {
            glue.frame(0.0, 0.0);
        }
        assert!(glue.gain > 0.999);
        glue.bypass.store(true, Ordering::Relaxed);
        assert_eq!(glue.frame(2.0, 1.0), (2.0, 1.0));
        assert_eq!(glue.frame(f32::NAN, f32::INFINITY), (0.0, 0.0));
    }
}
