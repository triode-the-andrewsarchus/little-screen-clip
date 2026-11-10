//! One stereo bus for pads and all auditions, using the shared hardware sink.
//!
//! Layers have 2 dB of headroom. The linked stereo compressor has adjustable makeup gain:
//! a normal single layer stays below its threshold, while overlapping hits receive
//! adjustable compression (default 2:1, 5 ms attack, 120 ms release). A linked peak ceiling catches
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

/// Steady-state compression in dBFS, including a continuous quadratic soft knee.
pub(super) fn compressor_db(input: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let delta = input - threshold;
    let slope = ratio.recip() - 1.0;
    if knee > 0.0 && delta.abs() < knee * 0.5 {
        input + slope * (delta + knee * 0.5).powi(2) / (2.0 * knee)
    } else if delta > 0.0 {
        threshold + delta / ratio
    } else {
        input
    }
}

pub(super) struct PadOutput {
    mixer: Mixer,
    player: Player,
    bypass: Arc<AtomicBool>,
    reduction: Arc<AtomicU32>,
    threshold: Arc<AtomicU32>,
    exponent: Arc<AtomicU32>,
    knee: Arc<AtomicU32>,
    makeup: Arc<AtomicU32>,
    attack: Arc<AtomicU32>,
    release: Arc<AtomicU32>,
    levels: Arc<[AtomicU32; 2]>,
}

impl PadOutput {
    pub fn new(bypass: bool, history: Arc<super::meter_history::MeterHistory>) -> Option<Self> {
        let (player, source) = Player::new();
        let output = Self::with_player_history(player, bypass, history);
        // Populate the queue before the device mixer inspects its format.
        crate::ui_ext::connect_shared_audio_source(source)?;
        Some(output)
    }

    /// Attaches the rack bus to an owned player, retaining the shared device lifetime.
    #[cfg(test)]
    pub(super) fn with_player(player: Player, bypass: bool) -> Self {
        Self::with_player_history(player, bypass, Arc::new(Default::default()))
    }
    fn with_player_history(
        player: Player,
        bypass: bool,
        history: Arc<super::meter_history::MeterHistory>,
    ) -> Self {
        let (mixer, source) = rodio::mixer::mixer(2.try_into().unwrap(), RATE.try_into().unwrap());
        // Keep the bus attached while idle between pad hits. Its owner stops and
        // drops it when all voices finish, on navigation, or on Escape.
        mixer.add(rodio::source::Zero::new(
            2.try_into().unwrap(),
            RATE.try_into().unwrap(),
        ));
        let bypass = Arc::new(AtomicBool::new(bypass));
        let reduction = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let mut glue = Glue::new(source, bypass.clone(), reduction.clone());
        glue.history = history;
        let threshold = glue.threshold.clone();
        let exponent = glue.exponent.clone();
        let levels = glue.levels.clone();
        let knee = glue.knee.clone();
        let makeup = glue.makeup.clone();
        let attack = glue.attack.clone();
        let release = glue.release.clone();
        player.append(glue);
        Self {
            mixer,
            player,
            bypass,
            reduction,
            threshold,
            exponent,
            knee,
            makeup,
            attack,
            release,
            levels,
        }
    }

    /// Attaches a guarded streaming audition to the same headroom/dynamics bus.
    pub fn play_source(&self, source: Box<dyn Source<Item = f32> + Send>) -> Player {
        let (player, queued) = Player::new();
        player.pause();
        player.append(source.amplify(HEADROOM));
        self.mixer.add(queued);
        player
    }

    /// Updates live controls without restarting any voices or touching the device.
    ///
    /// Non-finite numeric edits retain the last valid setting for each control.
    pub fn set_dynamics(&self, bank: &super::sample_pack::SamplePackBank) {
        self.bypass.store(bank.compressor_bypass, Ordering::Relaxed);
        // Numeric text edits can temporarily contain NaN/Inf. Retain the last
        // valid controls so they cannot poison the compressor envelope.
        if bank.compressor_threshold_db.is_finite() {
            self.threshold.store(
                10.0_f32
                    .powf(bank.compressor_threshold_db.clamp(-48.0, 0.0) / 20.0)
                    .to_bits(),
                Ordering::Relaxed,
            );
        }
        if bank.compressor_ratio.is_finite() {
            self.exponent.store(
                (1.0 - bank.compressor_ratio.clamp(1.0, 12.0).recip()).to_bits(),
                Ordering::Relaxed,
            );
        }
        if bank.compressor_knee_db.is_finite() {
            self.knee.store(
                bank.compressor_knee_db.clamp(0.0, 24.0).to_bits(),
                Ordering::Relaxed,
            );
        }
        for (value, target, min, max) in [
            (bank.compressor_attack_ms, &self.attack, 0.1, 200.0),
            (bank.compressor_release_ms, &self.release, 10.0, 2000.0),
        ] {
            if value.is_finite() {
                target.store(
                    time_coefficient(value.clamp(min, max)).to_bits(),
                    Ordering::Relaxed,
                );
            }
        }
        if bank.compressor_makeup_db.is_finite() {
            self.makeup.store(
                10.0_f32
                    .powf(bank.compressor_makeup_db.clamp(-12.0, 24.0) / 20.0)
                    .to_bits(),
                Ordering::Relaxed,
            );
        }
    }

    pub fn levels_db(&self) -> [f32; 2] {
        std::array::from_fn(|index| {
            20.0 * f32::from_bits(self.levels[index].load(Ordering::Relaxed))
                .max(0.000001)
                .log10()
        })
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
        let frames = source.size_hint().0 / source.channels().get() as usize;
        let source = super::layer_source::LayerSource::new(source, sample, frames);
        let (player, queued) = Player::new();
        player.append(
            source
                .speed(sample.speed().abs())
                .amplify(velocity.min(127) as f32 / 127.0 * HEADROOM),
        );
        self.mixer.add(queued);
        Some(player)
    }
}

impl Drop for PadOutput {
    fn drop(&mut self) {
        self.player.stop();
    }
}

/// Computes a one-pole envelope coefficient once per control update at the fixed bus rate.
fn time_coefficient(milliseconds: f32) -> f32 {
    (-1000.0 / (RATE as f32 * milliseconds)).exp()
}

/// Stereo-linked dynamics; processes complete frames to preserve stereo balance.
struct Glue<S> {
    source: S,
    bypass: Arc<AtomicBool>,
    reduction: Arc<AtomicU32>,
    threshold: Arc<AtomicU32>,
    exponent: Arc<AtomicU32>,
    knee: Arc<AtomicU32>,
    makeup: Arc<AtomicU32>,
    attack: Arc<AtomicU32>,
    release: Arc<AtomicU32>,
    levels: Arc<[AtomicU32; 2]>,
    gain: f32,
    right: Option<f32>,
    meter_frames: u16,
    peaks: [f32; 2],
    history: Arc<super::meter_history::MeterHistory>,
    meter_reduction: f32,
}

impl<S> Glue<S> {
    fn new(source: S, bypass: Arc<AtomicBool>, reduction: Arc<AtomicU32>) -> Self {
        Self {
            source,
            bypass,
            reduction,
            threshold: Arc::new(AtomicU32::new(0.85_f32.to_bits())),
            exponent: Arc::new(AtomicU32::new(0.5_f32.to_bits())),
            knee: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            makeup: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            attack: Arc::new(AtomicU32::new(
                time_coefficient(super::sample_pack::default_compressor_attack()).to_bits(),
            )),
            release: Arc::new(AtomicU32::new(
                time_coefficient(super::sample_pack::default_compressor_release()).to_bits(),
            )),
            levels: Arc::new([AtomicU32::new(0), AtomicU32::new(0)]),
            gain: 1.0,
            right: None,
            meter_frames: 0,
            peaks: [0.0; 2],
            history: Arc::new(Default::default()),
            meter_reduction: 0.0,
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        let left = if left.is_finite() { left } else { 0.0 };
        let right = if right.is_finite() { right } else { 0.0 };
        let peak = left.abs().max(right.abs());
        let threshold = f32::from_bits(self.threshold.load(Ordering::Relaxed));
        let exponent = f32::from_bits(self.exponent.load(Ordering::Relaxed));
        let knee = f32::from_bits(self.knee.load(Ordering::Relaxed));
        let makeup = f32::from_bits(self.makeup.load(Ordering::Relaxed));
        let target = if knee > 0.0 {
            let input = 20.0 * peak.max(0.000001).log10();
            10.0_f32.powf(
                (compressor_db(
                    input,
                    20.0 * threshold.log10(),
                    (1.0 - exponent).recip(),
                    knee,
                ) - input)
                    / 20.0,
            )
        } else if peak > threshold {
            (threshold / peak).powf(exponent)
        } else {
            1.0
        };
        let coefficient = f32::from_bits(if target < self.gain {
            self.attack.load(Ordering::Relaxed)
        } else {
            self.release.load(Ordering::Relaxed)
        });
        self.gain = target + coefficient * (self.gain - target);
        let bypass = self.bypass.load(Ordering::Relaxed);
        let gain = if bypass {
            1.0
        } else {
            (self.gain * makeup).min(0.98 / peak.max(0.000001))
        };
        self.peaks[0] = self.peaks[0].max(peak);
        self.peaks[1] = self.peaks[1].max(peak * gain);
        let reduction = if bypass {
            0.0
        } else {
            (-20.0 * (gain / makeup).max(0.000001).log10()).max(0.0)
        };
        self.meter_reduction = self.meter_reduction.max(reduction);
        if self.meter_frames == 0 {
            self.history.push([
                20.0 * self.peaks[0].max(0.000001).log10(),
                20.0 * self.peaks[1].max(0.000001).log10(),
                self.meter_reduction,
            ]);
            self.meter_reduction = 0.0;
            for (level, peak) in self.levels.iter().zip(self.peaks) {
                level.store(peak.to_bits(), Ordering::Relaxed);
            }
            self.peaks = [0.0; 2];
            self.reduction.store(
                (if bypass {
                    0.0
                } else {
                    -20.0 * (gain / makeup).max(0.000001).log10()
                })
                .max(0.0)
                .to_bits(),
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
    fn panned_voices_keep_channel_balance_from_the_first_audible_frame() {
        for (channels, rate, streaming) in [
            (1, 44100, false),
            (2, 48000, false),
            (1, 48000, true),
            (2, 44100, true),
        ] {
            for pan in [-1.0_f32, -0.35, 0.35, 1.0] {
                let pcm = Arc::new(vec![0.5; rate as usize * channels as usize]);
                let mut sample = SlotSample::new("hit.wav".into(), "hit.wav".into(), None, 1.0);
                sample.pan = pan;
                sample.cached_audio = Some(Arc::new(super::super::sample_pack::CachedAudio {
                    channels,
                    sample_rate: rate,
                    samples: pcm.clone(),
                    mono_samples: pcm,
                    waveform_peaks: vec![],
                    waveform_colors: vec![],
                }));
                let (device, mut audio) =
                    rodio::mixer::mixer(2.try_into().unwrap(), RATE.try_into().unwrap());
                let (player, queued) = Player::new();
                let bus = PadOutput::with_player(player, true);
                device.add(queued); // The bus queue is ready before device attachment too.
                let voice = if streaming {
                    bus.play_source(Box::new(super::super::layer_source::LayerSource::new(
                        SharedPcm::trimmed(&sample).unwrap(),
                        &sample,
                        rate as usize,
                    )))
                } else {
                    bus.play(&sample, 127).unwrap()
                };
                voice.play();
                let expected = [
                    0.5 * HEADROOM * (1.0 - pan.max(0.0)),
                    0.5 * HEADROOM * (1.0 + pan.min(0.0)),
                ];
                let mut audible = 0;
                for frame in 0..20000 {
                    let actual = [audio.next().unwrap(), audio.next().unwrap()];
                    if actual != [0.0, 0.0] {
                        audible += 1;
                        assert!(
                            (actual[0] * expected[1] - actual[1] * expected[0]).abs() < 0.00001,
                            "Wrong stereo balance at frame {frame}, {rate} Hz, pan {pan}: {actual:?}"
                        );
                        // Resuming a paused stream can interpolate its first frame from silence.
                        if frame > 1024 {
                            assert!(
                                (actual[0] - expected[0]).abs() < 0.00001
                                    && (actual[1] - expected[1]).abs() < 0.00001
                            );
                        }
                    }
                }
                assert!(audible > 19000);
            }
        }
    }

    #[test]
    fn soft_knee_is_continuous_and_makeup_precedes_the_safety_ceiling() {
        for threshold in [-30.0, -6.0, 0.0] {
            let width = 12.0;
            let lower = threshold - width * 0.5;
            let upper = threshold + width * 0.5;
            assert!((compressor_db(lower, threshold, 4.0, width) - lower).abs() < 0.00001);
            assert!(
                (compressor_db(upper, threshold, 4.0, width) - (threshold + 1.5)).abs() < 0.00001
            );
            assert!(compressor_db(threshold, threshold, 4.0, width) < threshold);
            let mut previous = f32::NEG_INFINITY;
            for step in 0..200 {
                let value = compressor_db(lower - 1.0 + step as f32 * 0.1, threshold, 4.0, width);
                assert!(value >= previous);
                previous = value;
            }
        }
        let mut glue = Glue::new(
            std::iter::empty::<f32>(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
        );
        glue.makeup.store(2.0_f32.to_bits(), Ordering::Relaxed);
        assert_eq!(glue.frame(0.2, 0.1), (0.4, 0.2));
        let frame = glue.frame(0.8, 0.4);
        assert!(frame.0 <= 0.980001);
        assert_eq!(frame.0, 2.0 * frame.1);
        glue.bypass.store(true, Ordering::Relaxed);
        assert_eq!(glue.frame(0.2, 0.1), (0.2, 0.1));
    }

    #[test]
    fn streaming_and_cached_auditions_share_pad_headroom_and_protection() {
        for streaming in [true, false] {
            let (player, mut audio) = Player::new();
            let bus = PadOutput::with_player(player, false);
            let pcm = Arc::new(vec![0.5; RATE as usize]);
            let mut sample = SlotSample::new("hit.wav".into(), "hit.wav".into(), None, 1.0);
            sample.cached_audio = Some(Arc::new(super::super::sample_pack::CachedAudio {
                channels: 1,
                sample_rate: RATE,
                samples: pcm.clone(),
                mono_samples: pcm,
                waveform_peaks: vec![],
                waveform_colors: vec![],
            }));
            let voice = if streaming {
                bus.play_source(Box::new(rodio::buffer::SamplesBuffer::new(
                    1.try_into().unwrap(),
                    RATE.try_into().unwrap(),
                    vec![0.5; RATE as usize],
                )))
            } else {
                bus.play(&sample, 127).unwrap()
            };
            voice.play();
            for _ in 0..4000 {
                audio.next();
            }
            assert!((audio.next().unwrap() - 0.5 * HEADROOM).abs() < 0.00001);
        }
    }
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
            waveform_colors: vec![],
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
    fn live_threshold_and_ratio_change_linked_compression_without_restarting() {
        let (mixer, _source) = rodio::mixer::mixer(2.try_into().unwrap(), RATE.try_into().unwrap());
        let output = PadOutput::with_player(Player::connect_new(&mixer), false);
        let mut bank = super::super::sample_pack::SamplePackBank::default();
        bank.compressor_threshold_db = -20.0;
        bank.compressor_ratio = 4.0;
        output.set_dynamics(&bank);
        let mut glue = Glue::new(
            std::iter::empty::<f32>(),
            output.bypass.clone(),
            output.reduction.clone(),
        );
        glue.threshold = output.threshold.clone();
        glue.exponent = output.exponent.clone();
        for _ in 0..48000 {
            glue.frame(0.5, 0.25);
        }
        let (left, right) = glue.frame(0.5, 0.25);
        assert!((left - 0.5 * 0.2_f32.powf(0.75)).abs() < 0.00001);
        assert!((left - 2.0 * right).abs() < 0.00001);
        bank.compressor_threshold_db = f32::NAN;
        bank.compressor_ratio = f32::INFINITY;
        output.set_dynamics(&bank);
        assert!(
            (glue.frame(0.5, 0.25).0 - left).abs() < 0.00001,
            "invalid live edits retain the last valid envelope"
        );
        bank.compressor_ratio = 1.0;
        output.set_dynamics(&bank);
        for _ in 0..96000 {
            glue.frame(0.5, 0.25);
        }
        assert!((glue.frame(0.5, 0.25).0 - 0.5).abs() < 0.001);
        bank.compressor_bypass = true;
        output.set_dynamics(&bank);
        assert_eq!(glue.frame(2.0, 1.0), (2.0, 1.0));
    }

    #[test]
    fn live_timing_controls_change_attack_and_release_envelopes() {
        let (player, _) = Player::new();
        let output = PadOutput::with_player(player, false);
        let mut bank = super::super::sample_pack::SamplePackBank::default();
        bank.compressor_threshold_db = -20.0;
        bank.compressor_ratio = 4.0;
        bank.compressor_attack_ms = 100.0;
        bank.compressor_release_ms = 1000.0;
        output.set_dynamics(&bank);
        let mut glue = Glue::new(
            std::iter::empty::<f32>(),
            output.bypass.clone(),
            output.reduction.clone(),
        );
        glue.threshold = output.threshold.clone();
        glue.exponent = output.exponent.clone();
        glue.attack = output.attack.clone();
        glue.release = output.release.clone();
        let target = (0.1_f32 / 0.5).powf(0.75);
        for _ in 0..4800 {
            glue.frame(0.5, 0.25);
        }
        assert!((glue.gain - (target + (-1.0_f32).exp() * (1.0 - target))).abs() < 0.001);
        let compressed = glue.gain;
        for _ in 0..48000 {
            glue.frame(0.0, 0.0);
        }
        assert!((glue.gain - (1.0 + (-1.0_f32).exp() * (compressed - 1.0))).abs() < 0.002);
        bank.compressor_attack_ms = f32::NAN;
        bank.compressor_release_ms = f32::INFINITY;
        output.set_dynamics(&bank);
        assert_eq!(
            f32::from_bits(output.attack.load(Ordering::Relaxed)),
            time_coefficient(100.0)
        );
        assert_eq!(
            f32::from_bits(output.release.load(Ordering::Relaxed)),
            time_coefficient(1000.0)
        );
        bank.compressor_attack_ms = 0.1;
        bank.compressor_release_ms = 10.0;
        output.set_dynamics(&bank);
        for _ in 0..480 {
            glue.frame(0.5, 0.25);
        }
        assert!((glue.gain - target).abs() < 0.001);
        for _ in 0..4800 {
            glue.frame(0.0, 0.0);
        }
        assert!(glue.gain > 0.999);
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

/// Shared ready-to-play rack. MIDI callbacks touch only bounded metadata and PCM
/// references here; they never decode audio or wait for an egui frame.
pub(super) struct Rack {
    bank: super::sample_pack::SamplePackBank,
    pub output: Option<PadOutput>,
    pub voices: [Vec<Player>; super::sample_pack::NUM_SLOTS],
    pub auditions: Vec<Player>,
    pub map_playing: bool,
    pub show_midi_notes: bool,
    pub history: Arc<super::meter_history::MeterHistory>,
}
impl Default for Rack {
    fn default() -> Self {
        Self {
            bank: Default::default(),
            output: None,
            voices: std::array::from_fn(|_| Vec::new()),
            auditions: Vec::new(),
            map_playing: false,
            show_midi_notes: false,
            history: Arc::new(Default::default()),
        }
    }
}
impl Rack {
    pub fn sync(&mut self, bank: &super::sample_pack::SamplePackBank) {
        let cache_changed = self
            .bank
            .slots
            .iter()
            .flat_map(|slot| &slot.samples)
            .zip(bank.slots.iter().flat_map(|slot| &slot.samples))
            .any(|(a, b)| {
                a.cached_audio.as_ref().map(Arc::as_ptr) != b.cached_audio.as_ref().map(Arc::as_ptr)
                    || a.prepared_pitch
                        .as_ref()
                        .map(|(_, audio)| Arc::as_ptr(audio))
                        != b.prepared_pitch
                            .as_ref()
                            .map(|(_, audio)| Arc::as_ptr(audio))
            });
        if self.bank != *bank || cache_changed {
            self.bank = bank.clone();
        }
        let solo = bank.slots.iter().any(|slot| slot.solo);
        for (slot, voices) in bank.slots.iter().zip(&self.voices) {
            let gain = if slot.mute || (solo && !slot.solo) {
                0.0
            } else {
                1.0
            };
            voices.iter().for_each(|voice| voice.set_volume(gain));
        }
        if let Some(output) = &self.output {
            output.set_dynamics(bank);
        }
    }
    pub fn retire(&mut self) {
        self.voices
            .iter_mut()
            .for_each(|voices| voices.retain(|player| !player.empty()));
        self.auditions.retain(|player| !player.empty());
        if !self.map_playing && self.auditions.is_empty() && self.voices.iter().all(Vec::is_empty) {
            self.output = None;
        }
    }
    pub fn stop(&mut self) {
        for player in self.voices.iter_mut().flat_map(|voices| voices.drain(..)) {
            player.stop();
        }
        self.auditions.drain(..).for_each(|player| player.stop());
        self.map_playing = false;
        self.output = None;
    }
    pub fn audition(&mut self, samples: &[SlotSample]) {
        self.auditions.drain(..).for_each(|player| player.stop());
        if self.output.is_none() {
            self.output = PadOutput::new(self.bank.compressor_bypass, self.history.clone());
        }
        if let Some(output) = &self.output {
            output.set_dynamics(&self.bank);
            self.auditions
                .extend(samples.iter().filter_map(|sample| output.play(sample, 127)));
        }
    }
    pub fn trigger(&mut self, slot_idx: usize, velocity: u8) -> bool {
        if slot_idx >= self.bank.slots.len() || velocity == 0 {
            return true;
        }
        let slot = &self.bank.slots[slot_idx];
        let solo = self.bank.slots.iter().any(|slot| slot.solo);
        if slot.mute || (solo && !slot.solo) {
            return true;
        }
        let group = slot.choke_group;
        // Equal priority keeps the familiar latest-hit behavior. Finished or gated
        // voices cannot lock out the group, even before the next UI retirement tick.
        if group != 0
            && self
                .bank
                .slots
                .iter()
                .zip(&self.voices)
                .any(|(peer, voices)| {
                    peer.choke_group == group
                        && peer.choke_priority > slot.choke_priority
                        && !peer.mute
                        && (!solo || peer.solo)
                        && voices.iter().any(|voice| !voice.empty())
                })
        {
            return true;
        }
        for (index, voices) in self.voices.iter_mut().enumerate() {
            if (index == slot_idx && slot.retrigger)
                || (index != slot_idx && group != 0 && self.bank.slots[index].choke_group == group)
            {
                for player in voices.drain(..) {
                    player.stop();
                }
            }
        }
        // Bound overlapping hits, retiring completed players even between UI frames.
        self.voices[slot_idx].retain(|player| !player.empty());
        while self.voices[slot_idx].len() + slot.samples.len() > 128 {
            self.voices[slot_idx].remove(0).stop();
        }
        let slot = &self.bank.slots[slot_idx];
        if !slot
            .samples
            .iter()
            .any(|sample| sample.cached_audio.is_some())
        {
            return true;
        }
        if self.output.is_none() {
            self.output = PadOutput::new(self.bank.compressor_bypass, self.history.clone());
        }
        let Some(output) = &self.output else {
            return false;
        };
        output.set_dynamics(&self.bank);
        let velocity = if self.bank.override_velocity {
            127
        } else {
            velocity.min(127)
        };
        for sample in &slot.samples {
            if let Some(player) = output.play(sample, velocity) {
                self.voices[slot_idx].push(player);
            }
        }
        true
    }
}
