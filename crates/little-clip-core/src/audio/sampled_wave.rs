//! Sample-accurate PCM WAVE windows for mixed-signal analysis.

use super::source;
use crate::engineering::scope::{Channel, MAX_SAMPLES_PER_READ};
use std::{
    fs::File,
    ops::Range,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use symphonia::core::{
    audio::{AudioBufferRef, SampleBuffer, Signal},
    codecs::{self, Decoder, DecoderOptions},
    formats::{FormatReader, SeekMode, SeekTo},
};

/// Retains the native sample grid and decoder for a guarded PCM audio file.
pub struct SampledWave {
    channels: Vec<Channel>,
    reader: Mutex<Reader>,
}

struct Reader {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    conversion: Option<SampleBuffer<f32>>,
}

impl SampledWave {
    pub(crate) fn open(file: File, path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let format = source::format_reader(file, path, cancel, symphonia::default::get_probe())?;
        let track = format
            .default_track()
            .ok_or("Audio recording has no track")?;
        let params = &track.codec_params;
        let count = params
            .channels
            .ok_or("Audio recording has no channel layout")?
            .count();
        let rate = params
            .sample_rate
            .filter(|&rate| rate > 0)
            .ok_or("Audio recording has no sample rate")?;
        let frames = params
            .n_frames
            .filter(|&frames| frames > 0)
            .ok_or("Audio recording has no sample extent")?;
        if !matches!(
            params.codec,
            codecs::CODEC_TYPE_PCM_U8
                | codecs::CODEC_TYPE_PCM_S16LE
                | codecs::CODEC_TYPE_PCM_S24LE
                | codecs::CODEC_TYPE_PCM_S32LE
                | codecs::CODEC_TYPE_PCM_F32LE
                | codecs::CODEC_TYPE_PCM_F64LE
        ) || params.start_ts != 0
            || params
                .time_base
                .is_some_and(|base| base.numer != 1 || base.denom != rate)
        {
            return Err("Signal analysis requires PCM WAVE with a sample-clock timebase".into());
        }
        if !(1..=16).contains(&count) {
            return Err("Signal analysis supports up to 16 WAVE channels".into());
        }
        let channels = (0..count)
            .map(|index| Channel {
                name: match count {
                    1 => "Mono".into(),
                    2 if index == 0 => "Left".into(),
                    2 => "Right".into(),
                    _ => format!("CH{}", index + 1),
                },
                unit: "FS".into(),
                sample_count: frames,
                start_seconds: 0.0,
                interval_seconds: f64::from(rate).recip(),
            })
            .collect();
        let decoder = symphonia::default::get_codecs()
            .make(params, &DecoderOptions::default())
            .map_err(|error| error.to_string())?;
        let track = track.id;
        Ok(Self {
            channels,
            reader: Mutex::new(Reader {
                format,
                decoder,
                track,
                conversion: None,
            }),
        })
    }

    pub(crate) fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub(crate) fn read_samples(
        &self,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        let frames = self
            .channels
            .get(channel)
            .ok_or("Audio channel is unavailable")?
            .sample_count;
        if range.start > range.end
            || range.end > frames
            || range.end - range.start > MAX_SAMPLES_PER_READ
        {
            return Err("Audio sample range exceeds the available read window".into());
        }
        if range.is_empty() {
            return Ok(Vec::new());
        }
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| "Audio reader was interrupted")?;
        reader.read(channel, range, cancel)
    }
}

impl Reader {
    fn read(
        &mut self,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        check_cancelled(cancel)?;
        let seek = SeekTo::TimeStamp {
            ts: range.start,
            track_id: self.track,
        };
        self.format
            .seek(SeekMode::Accurate, seek)
            .map_err(|error| error.to_string())?;
        self.decoder.reset();
        let mut samples = Vec::with_capacity((range.end - range.start) as usize);
        let mut next = range.start;
        while next < range.end {
            check_cancelled(cancel)?;
            let packet = self
                .format
                .next_packet()
                .map_err(|error| error.to_string())?;
            if packet.track_id() != self.track || packet.ts().saturating_add(packet.dur()) <= next {
                continue;
            }
            if packet.ts() > next {
                return Err("Audio sample window contains an unexpected gap".into());
            }
            let audio = self
                .decoder
                .decode(&packet)
                .map_err(|error| error.to_string())?;
            let start = (next - packet.ts()) as usize;
            let stop = ((range.end - packet.ts()) as usize).min(audio.frames());
            if start >= stop {
                return Err("Audio sample packet length does not match its timestamp".into());
            }
            append_plane(
                &mut self.conversion,
                &mut samples,
                audio,
                channel,
                start..stop,
            );
            next += (stop - start) as u64;
        }
        Ok(samples)
    }
}

fn append_plane(
    conversion: &mut Option<SampleBuffer<f32>>,
    output: &mut Vec<f32>,
    audio: AudioBufferRef<'_>,
    channel: usize,
    rows: Range<usize>,
) {
    if let AudioBufferRef::F32(planar) = audio {
        output.extend_from_slice(&planar.chan(channel)[rows]);
        return;
    }
    let frames = audio.frames();
    let capacity = audio.capacity();
    let spec = *audio.spec();
    if conversion
        .as_ref()
        .is_none_or(|buffer| buffer.capacity() < capacity * spec.channels.count())
    {
        *conversion = Some(SampleBuffer::new(capacity as u64, spec));
    }
    if let Some(buffer) = conversion {
        buffer.copy_planar_ref(audio);
        let base = channel * frames;
        output.extend_from_slice(&buffer.samples()[base + rows.start..base + rows.end]);
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Audio sample read cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufWriter;

    #[test]
    fn stereo_windows_seek_back_and_keep_native_sample_values() {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 12_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::new(BufWriter::new(file.reopen().unwrap()), spec).unwrap();
        for i in 0..12_001 {
            writer.write_sample(i * 20).unwrap();
            writer.write_sample(-i * 50).unwrap();
        }
        writer.finalize().unwrap();
        let cancel = AtomicBool::new(false);
        let audio = SampledWave::open(file.reopen().unwrap(), file.path(), &cancel).unwrap();
        assert_eq!(audio.channels()[0].interval_seconds, 1.0 / 12_000.0);
        assert_eq!(audio.channels()[1].sample_count, 12_001);
        for (channel, range, scale) in [(1, 8063..8194, -50), (0, 130..145, 20)] {
            let samples = audio.read_samples(channel, range.clone(), &cancel).unwrap();
            let expected: Vec<f32> = range
                .map(|index| (index as i32 * scale) as f32 / 8_388_608.0)
                .collect();
            assert_eq!(samples, expected);
        }
    }
}
