//! One explicitly selected raw sample, decoded on a cancellable worker.
use little_clip_core::multisample::{Document, EmbeddedSample, Region};
use rodio::Source;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
pub(super) struct Audio {
    pub source: rodio::buffer::SamplesBuffer,
    pub peaks: Vec<f32>,
    pub seconds: f32,
}
pub(super) fn prepare(
    document: &Document,
    region: &Region,
    base: &Path,
    archive: Option<&super::archive::Archive>,
    token: &AtomicBool,
) -> Result<Audio, String> {
    if token.load(Ordering::Relaxed) {
        return Err("Sample audition cancelled".into());
    }
    if let Some(EmbeddedSample::Pcm16(sample)) = &region.embedded {
        let data = document
            .pcm
            .as_ref()
            .and_then(|pcm| pcm.get(sample.bytes.clone()))
            .ok_or("Embedded PCM is unavailable")?;
        let limit = (sample.rate as usize * 10).min(2_000_000);
        let values: Vec<f32> = data
            .chunks_exact(2)
            .take(limit)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        return finish(values, 1, sample.rate, token);
    }
    if let Some(EmbeddedSample::Archive { path }) = &region.embedded {
        let bytes = archive
            .ok_or("Instrument archive is unavailable")?
            .member(path, token)?;
        let source =
            crate::audio_preview::decode_audio_source(Box::new(std::io::Cursor::new(bytes)))
                .map_err(|e| format!("Embedded sample decode failed: {e}"))?;
        let (samples, channels, rate) = decode_region(source, region, token)?;
        return finish(samples, channels, rate, token);
    }
    external(region, base, token)
}
fn external(region: &Region, base: &Path, token: &AtomicBool) -> Result<Audio, String> {
    if region.sample.is_empty() || region.sample.starts_with(['*', '[']) {
        return Err("This region references an embedded, generated or library sample; raw external audition is unavailable".into());
    }
    let path = base.join(&region.sample);
    let file = crate::file_source::open_regular_file(&path, true).map_err(|e| e.to_string())?;
    let held = file.try_clone().map_err(|e| e.to_string())?;
    let identity = crate::file_source::FileSource::capture_file(&file)?;
    let input = little_clip_core::audio::audio_input_from_file(file, token)?;
    let source = crate::audio_preview::decode_audio_source(input.stream)
        .map_err(|e| format!("Sample decode failed: {e}"))?;
    let (samples, channels, rate) = decode_region(source, region, token)?;
    if crate::file_source::FileSource::capture_file(&held)? != identity {
        return Err("Sample changed while decoding".into());
    }
    identity.check()?;
    finish(samples, channels, rate, token)
}
fn decode_region(
    mut source: crate::audio_preview::AudioDecoder,
    region: &Region,
    token: &AtomicBool,
) -> Result<(Vec<f32>, u16, u32), String> {
    let rate = source.sample_rate().get();
    let channels = source.channels().get();
    if rate > 384000 || channels > 8 {
        return Err("Sample rate/channel count exceeds audition limits".into());
    }
    let start = Duration::new(
        region.start / u64::from(rate),
        ((region.start % u64::from(rate)) * 1_000_000_000 / u64::from(rate)) as u32,
    );
    source
        .try_seek(start)
        .map_err(|e| format!("Cannot seek to the sample region: {e}"))?;
    let frames = region
        .end
        .map_or(u64::MAX, |end| end.saturating_sub(region.start))
        .min(u64::from(rate) * 10);
    let limit = frames.saturating_mul(u64::from(channels)).min(2_000_000) as usize;
    let mut samples = Vec::with_capacity(limit.min(480000));
    for (index, sample) in source.take(limit).enumerate() {
        if index % 4096 == 0 && token.load(Ordering::Relaxed) {
            return Err("Sample audition cancelled".into());
        }
        samples.push(if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        });
    }
    samples.truncate(samples.len() / channels as usize * channels as usize);
    Ok((samples, channels, rate))
}
fn finish(
    samples: Vec<f32>,
    channels: u16,
    rate: u32,
    token: &AtomicBool,
) -> Result<Audio, String> {
    if token.load(Ordering::Relaxed) {
        return Err("Sample audition cancelled".into());
    }
    if samples.is_empty() {
        return Err("This sample region contains no readable frames".into());
    }
    let seconds = samples.len() as f32 / channels as f32 / rate as f32;
    let chunk = samples.len().div_ceil(256).max(1);
    let peaks = samples
        .chunks(chunk)
        .map(|chunk| chunk.iter().map(|s| s.abs()).fold(0.0, f32::max))
        .collect();
    Ok(Audio {
        source: rodio::buffer::SamplesBuffer::new(
            channels.try_into().map_err(|_| "No sample channels")?,
            rate.try_into().map_err(|_| "No sample rate")?,
            samples,
        ),
        peaks,
        seconds,
    })
}

/// Prepare all chord voices together on the existing cancellable decoder worker.
/// The shared rodio mixer handles rate/channel conversion; gain reserves headroom.
pub(super) fn prepare_chord(
    document: &Document,
    instrument: usize,
    indices: &[usize],
    strikes: &[super::surface::Strike],
    base: &Path,
    archive: Option<&super::archive::Archive>,
    token: &AtomicBool,
) -> Result<Audio, String> {
    if indices.len() != strikes.len() || !(2..=8).contains(&indices.len()) {
        return Err("Chord audition requires two to eight mapped voices".into());
    }
    let regions = &document
        .instruments
        .get(instrument)
        .ok_or("Missing instrument")?
        .regions;
    let (mixer, source) = rodio::mixer::mixer(2.try_into().unwrap(), 44100.try_into().unwrap());
    let mut decoded = std::collections::BTreeMap::new();
    for (&index, strike) in indices.iter().zip(strikes) {
        let region = regions.get(index).ok_or("Missing sample region")?;
        if let std::collections::btree_map::Entry::Vacant(entry) = decoded.entry(index) {
            entry.insert(prepare(document, region, base, archive, token)?);
        }
        let speed = 2.0_f32.powf((strike.key as f32 - region.root as f32) / 12.0);
        mixer.add(
            decoded[&index]
                .source
                .clone()
                .speed(speed)
                .amplify(0.8 * velocity_gain(strike.velocity) / indices.len() as f32)
                .take_duration(Duration::from_secs(10)),
        );
    }
    let mut samples = Vec::new();
    for (index, value) in source.take(44100 * 2 * 10).enumerate() {
        if index % 4096 == 0 && token.load(Ordering::Relaxed) {
            return Err("Chord audition cancelled".into());
        }
        samples.push(value.clamp(-1.0, 1.0));
    }
    finish(samples, 2, 44100, token)
}

/// A quadratic amplitude response makes velocity audible even in single-layer banks.
pub(super) fn velocity_gain(velocity: u8) -> f32 {
    (f32::from(velocity.min(127)) / 127.0).powi(2)
}
