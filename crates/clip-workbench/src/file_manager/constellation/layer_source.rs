//! Shared layer rendering for live pads, auditions, and portable WAV exports.
use super::sample_pack::SlotSample;
use rodio::{Source, source::UniformSourceIterator};
use std::time::Duration;

/// Streams stereo balance and linear fades over a frame-aligned trimmed source.
/// Center preserves existing stereo level; mono is duplicated before balance.
/// Overlapping fades use the lower envelope, ending at zero on the final frame.
pub(super) struct LayerSource<S: Source> {
    source: UniformSourceIterator<S>,
    rate: rodio::SampleRate,
    frames: usize,
    position: usize,
    gains: [f32; 2],
    attack: f32,
    release: f32,
}
impl<S: Source> LayerSource<S> {
    pub fn new(source: S, sample: &SlotSample, frames: usize) -> Self {
        let rate = source.sample_rate();
        let pan = sample.pan.clamp(-1.0, 1.0);
        Self {
            source: UniformSourceIterator::new(source, 2.try_into().unwrap(), rate),
            rate,
            frames,
            position: 0,
            gains: [
                sample.volume * (1.0 - pan.max(0.0)),
                sample.volume * (1.0 + pan.min(0.0)),
            ],
            attack: sample.attack_ms * rate.get() as f32 / 1000.0,
            release: sample.release_ms * rate.get() as f32 / 1000.0,
        }
    }
}
impl<S: Source> Iterator for LayerSource<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let frame = self.position / 2;
        if frame >= self.frames {
            return None;
        }
        let attack = if self.attack > 0.0 {
            (frame as f32 / self.attack).min(1.0)
        } else {
            1.0
        };
        let release = if self.release > 0.0 {
            ((self.frames - 1 - frame) as f32 / self.release).min(1.0)
        } else {
            1.0
        };
        let value = self.source.next()? * self.gains[self.position % 2] * attack.min(release);
        self.position += 1;
        Some(value)
    }
}
impl<S: Source> Source for LayerSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.frames * 2 - self.position)
    }
    fn channels(&self) -> rodio::ChannelCount {
        2.try_into().unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.frames as f64 / self.rate.get() as f64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn balance_and_fades_are_frame_aligned_and_bound_even_short_hits() {
        let mut sample = SlotSample::new("hit.wav".into(), "hit.wav".into(), None, 1.0);
        sample.volume = 0.5;
        sample.pan = -1.0;
        sample.attack_ms = 2.0;
        sample.release_ms = 2.0;
        let source = rodio::buffer::SamplesBuffer::new(
            1.try_into().unwrap(),
            1000.try_into().unwrap(),
            vec![1.0; 6],
        );
        let values: Vec<_> = LayerSource::new(source, &sample, 6).collect();
        assert_eq!(
            values,
            vec![0.0, 0.0, 0.25, 0.0, 0.5, 0.0, 0.5, 0.0, 0.25, 0.0, 0.0, 0.0]
        );
        let source = rodio::buffer::SamplesBuffer::new(
            2.try_into().unwrap(),
            1000.try_into().unwrap(),
            vec![1.0; 2],
        );
        assert_eq!(
            LayerSource::new(source, &sample, 1).collect::<Vec<_>>(),
            vec![0.0, 0.0]
        );
    }
}
