//! Native port of dadabots/dadaspec (MIT; see ../../LICENSE-dadaspec).
//!
//! Uses 32 kHz linear resampling, uncentred 2048-point periodic-Hann STFT,
//! 30 Slaney-normalized mel filters, per-channel dB contrast and independently
//! normalized red/bass, green/mid and blue/high tint. Mono mirrors exactly.
//! Analysis retains at most 65,536 stereo mel frames, coarsening adjacent
//! powers for unusually long inputs; PCM is never retained by the analyzer.

use crate::ImageThumbnail;
use fearless_simd::{Level, Simd, dispatch, prelude::*};
use fearless_simd_macros::simd;
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex32};
use std::path::Path;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

const FFT: usize = 2048;
const MELS: usize = 30;
const RATE: u32 = 32_000;
const MAX_COLUMNS: usize = 65_536;

#[cfg(test)]
mod profile;

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Dadaspec cancelled".into())
    } else {
        Ok(())
    }
}
fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > 4096 || height > 1024 {
        Err("Dadaspec dimensions must be within 1..=4096 by 1..=1024".into())
    } else {
        Ok(())
    }
}

/// Streaming stereo input resampled with upstream's align_corners=false rule.
/// Feed the first two channels; duplicate the first for mono.
pub struct DadaspecBuilder {
    analyzer: Analyzer,
    input_index: usize,
    output_index: usize,
    input_frames: usize,
    output_frames: usize,
    ratio: f32,
    previous: [f32; 2],
}
impl DadaspecBuilder {
    /// Creates an analyzer for a known source-frame count and sample rate.
    ///
    /// # Errors
    /// Returns an error for an empty signal or zero sample rate.
    pub fn new(input_frames: usize, sample_rate: u32) -> Result<Self, String> {
        if input_frames == 0 || sample_rate == 0 {
            return Err("Dadaspec needs audio frames and a sample rate".into());
        }
        let output_frames = (input_frames as f64 * RATE as f64 / sample_rate as f64)
            .round_ties_even()
            .max(1.0) as usize;
        Ok(Self {
            analyzer: Analyzer::new(),
            input_index: 0,
            output_index: 0,
            input_frames,
            output_frames,
            ratio: input_frames as f32 / output_frames as f32,
            previous: [0.0; 2],
        })
    }
    /// Creates a strip analyzer with overlapping windows for short clips.
    ///
    /// Targets the requested pixel columns, with a 64-sample minimum hop to
    /// bound work. Longer clips retain upstream's 2048-sample hop. The window,
    /// mel filters, normalization and colors are unchanged.
    ///
    /// # Errors
    /// Returns an error for empty input, zero rate or unsupported column counts.
    pub fn new_preview(
        input_frames: usize,
        sample_rate: u32,
        columns: u32,
    ) -> Result<Self, String> {
        check_size(columns, 60)?;
        let mut builder = Self::new(input_frames, sample_rate)?;
        builder.analyzer.hop = builder
            .output_frames
            .saturating_sub(FFT)
            .div_ceil(columns.saturating_sub(1).max(1) as usize)
            .clamp(64, FFT);
        Ok(builder)
    }
    /// Feeds the next source frame; duplicates of mono belong in both elements.
    pub fn push_frame(&mut self, frame: [f32; 2]) {
        if self.input_index >= self.input_frames {
            return;
        }
        if self.input_frames == self.output_frames {
            self.analyzer.push(frame);
            self.output_index += 1;
            self.input_index += 1;
            self.previous = frame;
            return;
        }
        while self.output_index < self.output_frames {
            let source = ((self.output_index as f32 + 0.5) * self.ratio - 0.5).max(0.0);
            let left = (source as usize).min(self.input_frames - 1);
            let right = (left + 1).min(self.input_frames - 1);
            if right > self.input_index {
                break;
            }
            let fraction = source - left as f32;
            let previous = if left == self.input_index {
                frame
            } else {
                self.previous
            };
            self.analyzer.push([
                (1.0 - fraction) * previous[0] + fraction * frame[0],
                (1.0 - fraction) * previous[1] + fraction * frame[1],
            ]);
            self.output_index += 1;
        }
        self.previous = frame;
        self.input_index += 1;
    }
    /// Feeds borrowed planar packets, retaining only their last source frame.
    ///
    /// This batches resampling into FFT windows instead of calling the sample
    /// admission path for every input frame. Omit `right` for mirrored mono.
    pub fn push_planar(&mut self, left: &[f32], right: Option<&[f32]>) {
        let right = right.unwrap_or(left);
        let count = left
            .len()
            .min(right.len())
            .min(self.input_frames - self.input_index);
        if count == 0 {
            return;
        }
        if self.input_frames == self.output_frames {
            self.analyzer.push_planar(&left[..count], &right[..count]);
            self.output_index += count;
        } else {
            self.resample_block(&left[..count], &right[..count]);
        }
        self.previous = [left[count - 1], right[count - 1]];
        self.input_index += count;
    }
    fn resample_block(&mut self, left: &[f32], right: &[f32]) {
        let end = self.input_index + left.len();
        while self.output_index < self.output_frames {
            let source = ((self.output_index as f32 + 0.5) * self.ratio - 0.5).max(0.0);
            let first = (source as usize).min(self.input_frames - 1);
            let last = (first + 1).min(self.input_frames - 1);
            if last >= end {
                break;
            }
            let fraction = source - first as f32;
            let earlier = if first < self.input_index {
                self.previous
            } else {
                [
                    left[first - self.input_index],
                    right[first - self.input_index],
                ]
            };
            let later = if last < self.input_index {
                self.previous
            } else {
                [
                    left[last - self.input_index],
                    right[last - self.input_index],
                ]
            };
            self.analyzer.samples[0][self.analyzer.filled] =
                (1.0 - fraction) * earlier[0] + fraction * later[0];
            self.analyzer.samples[1][self.analyzer.filled] =
                (1.0 - fraction) * earlier[1] + fraction * later[1];
            self.analyzer.filled += 1;
            self.output_index += 1;
            if self.analyzer.filled == FFT {
                self.analyzer.complete_window();
            }
        }
    }
    /// Normalizes accumulated energy and produces a bounded RGBA image.
    ///
    /// # Errors
    /// Returns an error on cancellation or unsupported output dimensions.
    pub fn finish(
        self,
        width: u32,
        height: u32,
        cancel: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancel(cancel)?;
        check_size(width, height)?;
        self.analyzer.finish(width, height, cancel)
    }
}

/// Renders decoded planar PCM on a worker; cancellation is checked every window.
pub fn render_planar(
    left: &[f32],
    right: Option<&[f32]>,
    rate: u32,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    check_cancel(cancel)?;
    check_size(width, height)?;
    let right = right.unwrap_or(left);
    let frames = left.len().min(right.len());
    let mut builder = DadaspecBuilder::new(frames, rate)?;
    for first in (0..frames).step_by(2048) {
        check_cancel(cancel)?;
        let end = (first + 2048).min(frames);
        builder.push_planar(&left[first..end], Some(&right[first..end]));
    }
    builder.finish(width, height, cancel)
}

fn hz_to_mel(hz: f64) -> f64 {
    if hz < 1000.0 {
        hz / (200.0 / 3.0)
    } else {
        15.0 + (hz / 1000.0).ln() / (6.4_f64.ln() / 27.0)
    }
}
fn mel_to_hz(mel: f64) -> f64 {
    if mel < 15.0 {
        mel * (200.0 / 3.0)
    } else {
        1000.0 * ((mel - 15.0) * (6.4_f64.ln() / 27.0)).exp()
    }
}

struct MelFilter {
    first: usize,
    weights: Vec<f32>,
}
// This immutable plan contains no source data or mutable decoder/FFT buffers.
// All preview workers share its coefficients and existing SIMD FFT plan.
struct AnalysisPlan {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    filters: Vec<MelFilter>,
    bands: [usize; MELS],
}
struct Analyzer {
    plan: &'static AnalysisPlan,
    input: Vec<f32>,
    output: Vec<Complex32>,
    scratch: Vec<Complex32>,
    samples: [[f32; FFT]; 2],
    filled: usize,
    hop: usize,
    powers: Vec<[[f32; MELS]; 2]>,
    merge: usize,
    pending: [[f32; MELS]; 2],
    pending_count: usize,
}
impl AnalysisPlan {
    fn shared() -> &'static Self {
        static PLAN: OnceLock<AnalysisPlan> = OnceLock::new();
        PLAN.get_or_init(Self::new)
    }
    fn new() -> Self {
        let fft = RealFftPlanner::new().plan_fft_forward(FFT);
        let points: Vec<_> = (0..MELS + 2)
            .map(|i| mel_to_hz(hz_to_mel(16000.0) * i as f64 / (MELS + 1) as f64))
            .collect();
        let filters = points
            .windows(3)
            .map(|p| {
                let weights: Vec<_> = (0..=FFT / 2)
                    .filter_map(|bin| {
                        let hz = bin as f64 * RATE as f64 / FFT as f64;
                        let weight = ((hz - p[0]) / (p[1] - p[0]))
                            .min((p[2] - hz) / (p[2] - p[1]))
                            .max(0.0) as f32
                            * (2.0 / (p[2] - p[0])) as f32;
                        (weight > 0.0).then_some((bin, weight))
                    })
                    .collect();
                MelFilter {
                    first: weights.first().map_or(0, |&(bin, _)| bin),
                    weights: weights.into_iter().map(|(_, weight)| weight).collect(),
                }
            })
            .collect();
        // Upstream deliberately uses endpoint-spaced mel frequencies for tint
        // classification, rather than the filterbank's interior centers.
        let bands = std::array::from_fn(|i| {
            let hz = mel_to_hz(hz_to_mel(16000.0) * i as f64 / (MELS - 1) as f64);
            if hz < 200.0 {
                0
            } else if hz < 1500.0 {
                1
            } else if hz < 16000.0 {
                2
            } else {
                3
            }
        });
        Self {
            fft,
            window: (0..FFT)
                .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FFT as f64).cos()) as f32)
                .collect(),
            filters,
            bands,
        }
    }
}
impl Analyzer {
    fn new() -> Self {
        let plan = AnalysisPlan::shared();
        Self {
            plan,
            input: plan.fft.make_input_vec(),
            output: plan.fft.make_output_vec(),
            scratch: plan.fft.make_scratch_vec(),
            samples: [[0.0; FFT]; 2],
            filled: 0,
            hop: FFT,
            powers: Vec::new(),
            merge: 1,
            pending: [[0.0; MELS]; 2],
            pending_count: 0,
        }
    }
    fn push(&mut self, frame: [f32; 2]) {
        self.samples[0][self.filled] = frame[0];
        self.samples[1][self.filled] = frame[1];
        self.filled += 1;
        if self.filled == FFT {
            self.complete_window();
        }
    }
    fn push_planar(&mut self, left: &[f32], right: &[f32]) {
        let mut first = 0;
        while first < left.len() {
            let count = (FFT - self.filled).min(left.len() - first);
            self.samples[0][self.filled..self.filled + count]
                .copy_from_slice(&left[first..first + count]);
            self.samples[1][self.filled..self.filled + count]
                .copy_from_slice(&right[first..first + count]);
            self.filled += count;
            first += count;
            if self.filled == FFT {
                self.complete_window();
            }
        }
    }
    fn complete_window(&mut self) {
        self.transform();
        for channel in &mut self.samples {
            channel.copy_within(self.hop.., 0);
        }
        self.filled = FFT - self.hop;
    }
    fn transform(&mut self) {
        for channel in 0..2 {
            dispatch!(Level::new(), simd => window_samples(simd, &self.samples[channel], &self.plan.window, &mut self.input));
            self.plan
                .fft
                .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
                .expect("fixed FFT buffers");
            dispatch!(Level::new(), simd => mel_spectrum(simd, &self.output, &self.plan.filters, &mut self.pending[channel], self.merge));
        }
        self.pending_count += 1;
        if self.pending_count == self.merge {
            self.powers.push(self.pending);
            self.pending = [[0.0; MELS]; 2];
            self.pending_count = 0;
        }
        if self.powers.len() == MAX_COLUMNS {
            for index in 0..MAX_COLUMNS / 2 {
                for channel in 0..2 {
                    for mel in 0..MELS {
                        self.powers[index][channel][mel] = (self.powers[index * 2][channel][mel]
                            + self.powers[index * 2 + 1][channel][mel])
                            * 0.5;
                    }
                }
            }
            self.powers.truncate(MAX_COLUMNS / 2);
            self.merge *= 2;
        }
    }
    fn finish(
        mut self,
        width: u32,
        height: u32,
        cancel: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        // Only sub-window inputs are padded. Ordinary final partial windows are
        // discarded to avoid the artificial broadband edges of centred STFTs.
        if self.powers.is_empty() && self.pending_count == 0 {
            self.transform();
        }
        if self.pending_count > 0 {
            for ch in &mut self.pending {
                for power in ch {
                    *power *= self.merge as f32 / self.pending_count as f32;
                }
            }
            self.powers.push(self.pending);
        }
        let columns = self.powers.len();
        let mut image = image::RgbaImage::new(columns as u32, (MELS * 2) as u32);
        for channel in 0..2 {
            self.composite_channel(channel, &mut image, cancel)?;
        }
        let mut resized =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Lanczos3);
        // Store energy as coverage, rather than an opaque black backdrop. Lift
        // the other components slightly so saturated blue is readable on dark
        // surfaces too. Convert after resizing to keep interpolated edges valid
        // premultiplied pixels (every color component is at most alpha).
        for pixel in resized.pixels_mut() {
            *pixel = image::Rgba(transparent_pixel(pixel.0));
        }
        check_cancel(cancel)?;
        Ok(ImageThumbnail {
            width,
            height,
            premultiplied_rgba: resized.into_raw(),
        })
    }
    fn composite_channel(
        &self,
        channel: usize,
        image: &mut image::RgbaImage,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let columns = self.powers.len();
        let peak = self
            .powers
            .iter()
            .flat_map(|frame| frame[channel])
            .fold(1e-10_f32, f32::max);
        let mut tints = Vec::with_capacity(columns);
        let mut maxima = [0.0_f32; 3];
        for frame in &self.powers {
            let mut bands = [0.0_f32; 3];
            for (mel, &power) in frame[channel].iter().enumerate() {
                if self.plan.bands[mel] < 3 {
                    bands[self.plan.bands[mel]] += power;
                }
            }
            let tint = bands.map(|power| (10.0 * (power + 1e-10).log10() + 20.0).max(0.0));
            for band in 0..3 {
                maxima[band] = maxima[band].max(tint[band]);
            }
            tints.push(tint);
        }
        for (column, (frame, tint)) in self.powers.iter().zip(tints).enumerate() {
            if column.is_multiple_of(256) {
                check_cancel(cancel)?;
            }
            for (mel, &power) in frame[channel].iter().enumerate() {
                let brightness = ((10.0 * (power.max(1e-10).log10() - peak.log10()) + 60.0) / 60.0)
                    .clamp(0.0, 1.0)
                    .powf(0.6);
                let rgb: [u8; 3] = std::array::from_fn(|band| {
                    (brightness * tint[band] / maxima[band].max(1e-10) * 255.0).clamp(0.0, 255.0)
                        as u8
                });
                let row = if channel == 0 {
                    MELS - 1 - mel
                } else {
                    MELS + mel
                };
                image.put_pixel(
                    column as u32,
                    row as u32,
                    image::Rgba([rgb[0], rgb[1], rgb[2], 255]),
                );
            }
        }
        Ok(())
    }
}

fn transparent_pixel(mut pixel: [u8; 4]) -> [u8; 4] {
    let alpha = *pixel[..3].iter().max().unwrap();
    for component in &mut pixel[..3] {
        *component = ((u16::from(*component) * 56 + u16::from(alpha) * 44) / 100) as u8;
    }
    pixel[3] = alpha;
    pixel
}

/// Returns a multiplicative grayscale tint for a Dadaspec drawn on the given surface.
///
/// Apply this when painting, so cached thumbnails follow theme changes without
/// decoding again. Alpha remains unchanged. Light surfaces use darker ink;
/// dark surfaces retain the bright spectral colors.
pub fn surface_tint(background: [u8; 3]) -> u8 {
    let [r, g, b] = background;
    if crate::palette::relative_luminance(r, g, b) >= 0.4 {
        112
    } else {
        255
    }
}

#[simd]
fn mel_spectrum<S: Simd>(
    simd: S,
    spectrum: &[Complex32],
    filters: &[MelFilter],
    pending: &mut [f32; MELS],
    merge: usize,
) {
    let mut powers = [0.0; FFT / 2 + 1];
    // Compute each bin's magnitude once, shared by neighboring mel triangles.
    for (power, bin) in powers.iter_mut().zip(spectrum) {
        *power = bin.norm_sqr();
    }
    let lanes = S::f32s::LEN;
    for (mel, filter) in filters.iter().enumerate() {
        let values = &powers[filter.first..filter.first + filter.weights.len()];
        let mut values = values.chunks_exact(lanes);
        let mut weights = filter.weights.chunks_exact(lanes);
        let mut sum = S::f32s::splat(simd, 0.0);
        for (values, weights) in values.by_ref().zip(weights.by_ref()) {
            sum += S::f32s::from_slice(simd, values) * S::f32s::from_slice(simd, weights);
        }
        let tail: f32 = values
            .remainder()
            .iter()
            .zip(weights.remainder())
            .map(|(value, weight)| value * weight)
            .sum();
        pending[mel] += (sum.reduce_sum() + tail) / merge as f32;
    }
}

#[simd]
fn window_samples<S: Simd>(simd: S, samples: &[f32], window: &[f32], output: &mut [f32]) {
    let lanes = S::f32s::LEN;
    let first = output.len() / lanes * lanes;
    let mut chunks = output.chunks_exact_mut(lanes);
    for ((out, samples), window) in chunks
        .by_ref()
        .zip(samples.chunks_exact(lanes))
        .zip(window.chunks_exact(lanes))
    {
        let values = S::f32s::from_slice(simd, samples);
        let finite = values.is_finite().select(values, S::f32s::splat(simd, 0.0));
        (finite * S::f32s::from_slice(simd, window)).store_slice(out);
    }
    for (index, out) in chunks.into_remainder().iter_mut().enumerate() {
        let value = samples[first + index];
        *out = if value.is_finite() {
            value * window[first + index]
        } else {
            0.0
        };
    }
}

/// Decodes a guarded input into a bounded native Dadaspec thumbnail on a worker.
/// `start_sec` and `duration_sec` select a source-relative excerpt; no PCM is retained.
/// Short strips use overlapping windows to approach the requested column count.
pub fn render_file(
    file: std::fs::File,
    path: &Path,
    start_sec: f64,
    duration_sec: f64,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    use symphonia::core::codecs::DecoderOptions;
    check_size(width, height)?;
    check_cancel(cancel)?;
    if !start_sec.is_finite() || start_sec < 0.0 || !duration_sec.is_finite() || duration_sec <= 0.0
    {
        return Err("Unknown Dadaspec duration".into());
    }
    let format = super::source::format_reader(file, path, cancel, symphonia::default::get_probe())?;
    let track = format.default_track().ok_or("No audio track")?.clone();
    let rate = track
        .codec_params
        .sample_rate
        .ok_or("Unknown sample rate")?;
    if rate == 0 {
        return Err("Invalid Dadaspec sample rate".into());
    }
    let decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let frames = (duration_sec * rate as f64).round().max(1.0) as usize;
    let mut stream = DadaspecStream {
        format,
        decoder,
        track_id: track.id,
        rate,
        channels: None,
        skip: 0,
        remaining: frames,
        builder: DadaspecBuilder::new_preview(frames, rate, width)?,
    };
    stream.seek(start_sec, &track.codec_params)?;
    stream.decode(cancel)?;
    stream.builder.finish(width, height, cancel)
}

// Own the decoder and excerpt counters together so packet iteration and
// temporal admission cannot disagree about which samples enter the analyzer.
struct DadaspecStream {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    rate: u32,
    channels: Option<usize>,
    skip: usize,
    remaining: usize,
    builder: DadaspecBuilder,
}
impl DadaspecStream {
    fn seek(
        &mut self,
        start_sec: f64,
        params: &symphonia::core::codecs::CodecParameters,
    ) -> Result<(), String> {
        use symphonia::core::{
            formats::{SeekMode, SeekTo},
            units::Time,
        };
        if start_sec == 0.0 {
            return Ok(());
        }
        let seek = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(start_sec),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|error| error.to_string())?;
        let base = params.time_base.ok_or("Unknown seek time base")?;
        let actual = base.calc_time(seek.actual_ts);
        self.skip = ((start_sec - (actual.seconds as f64 + actual.frac)).max(0.0)
            * self.rate as f64)
            .round() as usize;
        self.decoder.reset();
        Ok(())
    }
    fn decode(&mut self, cancel: &AtomicBool) -> Result<(), String> {
        use symphonia::core::audio::{AudioBufferRef, Signal};
        use symphonia::core::errors::Error;
        let frames = self.remaining;
        let mut converted = None;
        let started = std::time::Instant::now();
        while self.remaining > 0 {
            check_cancel(cancel)?;
            if started.elapsed() > std::time::Duration::from_secs(10) {
                return Err("Dadaspec exceeded the ten-second preview budget".into());
            }
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.to_string()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = self.decoder.decode(&packet).map_err(|e| e.to_string())?;
            if decoded.spec().rate != self.rate {
                return Err("Dadaspec sample rate changed".into());
            }
            let channels = decoded.spec().channels.count();
            if channels == 0 {
                return Err("No Dadaspec channels".into());
            }
            if self.channels.is_some_and(|previous| previous != channels) {
                return Err("Dadaspec channel layout changed".into());
            }
            self.channels = Some(channels);
            let count = decoded.frames();
            let first = self.skip.min(count);
            self.skip -= first;
            let end = first + (count - first).min(self.remaining);
            if let AudioBufferRef::F32(buffer) = decoded {
                self.builder.push_planar(
                    &buffer.chan(0)[first..end],
                    (channels > 1).then(|| &buffer.chan(1)[first..end]),
                );
            } else {
                let samples = super::decode::conversion_buffer(
                    &mut converted,
                    decoded.capacity(),
                    *decoded.spec(),
                );
                samples.copy_planar_ref(decoded);
                self.builder.push_planar(
                    &samples.samples()[first..end],
                    (channels > 1).then(|| &samples.samples()[count + first..count + end]),
                );
            }
            self.remaining -= end - first;
        }
        if self.remaining == frames {
            return Err("No decoded Dadaspec samples".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(frames: usize) -> (Vec<f32>, Vec<f32>) {
        let left = (0..frames)
            .map(|i| {
                if (i / 2048) % 3 == 1 {
                    0.0
                } else {
                    ((i * 97 % 1024) as f32 - 512.0) / 2048.0
                }
            })
            .collect();
        let right = (0..frames)
            .map(|i| {
                if (i / 2048) % 4 == 2 {
                    0.0
                } else {
                    ((i * 31 % 2048) as f32 - 1024.0) / 4096.0
                }
            })
            .collect();
        (left, right)
    }
    fn tone(hz: f32) -> Vec<f32> {
        (0..32000)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / 32000.0).sin() * 0.5)
            .collect()
    }
    #[test]
    fn matches_upstream_numpy_pixels_at_native_rate_and_after_resampling() {
        for (rate, frames, width, reference) in [
            (
                32000,
                14336,
                7,
                include_bytes!("fixtures/dadaspec_32000.rgba").as_slice(),
            ),
            (
                44100,
                44100,
                15,
                include_bytes!("fixtures/dadaspec_44100.rgba").as_slice(),
            ),
        ] {
            let (left, right) = fixture(frames);
            let image = render_planar(
                &left,
                Some(&right),
                rate,
                width,
                60,
                &AtomicBool::new(false),
            )
            .unwrap();
            let differences: Vec<_> = image
                .premultiplied_rgba
                .iter()
                .zip(
                    reference
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|p| transparent_pixel(*p)),
                )
                .map(|(a, b)| a.abs_diff(b))
                .collect();
            let mean =
                differences.iter().map(|&d| f64::from(d)).sum::<f64>() / differences.len() as f64;
            assert!(mean < 0.5, "rate {rate}: mean pixel error {mean}");
            assert!(
                *differences.iter().max().unwrap() <= 4,
                "rate {rate}: max pixel error {}",
                differences.iter().max().unwrap()
            );
        }
    }
    #[test]
    fn silence_is_transparent_and_mono_mirrors_while_stereo_preserves_both_channels() {
        let cancel = AtomicBool::new(false);
        let silence = render_planar(&[0.0; 4096], None, 32000, 2, 60, &cancel).unwrap();
        assert!(
            silence
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 0, 0, 0])
        );
        let bass = tone(80.0);
        let high = tone(5000.0);
        let mono = render_planar(&bass, None, 32000, 15, 60, &cancel).unwrap();
        for row in 0..30 {
            assert_eq!(
                &mono.premultiplied_rgba[row * 60..(row + 1) * 60],
                &mono.premultiplied_rgba[(59 - row) * 60..(60 - row) * 60]
            );
        }
        let stereo = render_planar(&bass, Some(&high), 32000, 15, 60, &cancel).unwrap();
        let energy = |rows: std::ops::Range<usize>, band: usize| {
            rows.flat_map(|row| (0..15).map(move |col| (row * 15 + col) * 4 + band))
                .map(|i| u32::from(stereo.premultiplied_rgba[i]))
                .sum::<u32>()
        };
        assert!(energy(0..30, 0) > energy(0..30, 2) * 2);
        assert!(energy(30..60, 2) > energy(30..60, 0) * 2);
        // Sub-window clips are still visible, without indexing empty signals.
        assert!(render_planar(&bass[..100], None, 32000, 20, 60, &cancel).is_ok());
        assert!(render_planar(&bass, Some(&[]), 32000, 20, 60, &cancel).is_err());
    }
    #[test]
    fn spectral_colors_have_contrast_on_every_light_and_dark_surface() {
        use crate::{config::WorkbenchAppearance, palette::BaseColors};
        for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
            let colors = BaseColors::for_appearance(appearance);
            for surface in [
                colors.canvas,
                colors.surface_panel,
                colors.surface_raised,
                colors.surface_recessed,
                colors.console_surface,
            ] {
                let (r, g, b, _) = crate::color::parse_hex_color_rgba_or_white(&surface);
                let tint = u16::from(surface_tint([r, g, b]));
                for red in [0, 64, 128, 192, 255] {
                    for green in [0, 64, 128, 192, 255] {
                        for blue in [0, 64, 128, 192, 255] {
                            if red != 255 && green != 255 && blue != 255 {
                                continue;
                            }
                            let [pr, pg, pb, alpha] = transparent_pixel([red, green, blue, 255]);
                            assert_eq!(alpha, 255);
                            let ink = [pr, pg, pb].map(|v| (u16::from(v) * tint / 255) as u8);
                            let contrast =
                                crate::palette::contrast_ratio((r, g, b), (ink[0], ink[1], ink[2]));
                            assert!(
                                contrast >= 3.0,
                                "{surface}: {red}/{green}/{blue} has contrast {contrast}"
                            );
                        }
                    }
                }
            }
        }
        for energy in 0..=255 {
            let pixel = transparent_pixel([energy, energy / 2, 0, 255]);
            assert_eq!(pixel[3], energy);
            assert!(pixel[..3].iter().all(|c| *c <= pixel[3]));
        }
        let tone = tone(5000.0);
        let rendered = render_planar(&tone, None, 32000, 537, 71, &AtomicBool::new(false)).unwrap();
        assert!(
            rendered
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[..3].iter().all(|c| *c <= p[3]))
        );
        assert!(
            rendered
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] == 0)
        );
        assert!(
            rendered
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0 && p[3] < 255)
        );
    }
    #[test]
    fn validates_dimensions_and_cancellation_before_work() {
        let cancel = AtomicBool::new(false);
        for (width, height) in [(0, 60), (300, 0), (4097, 60), (300, 1025)] {
            assert!(render_planar(&[0.5; 100], None, 32000, width, height, &cancel).is_err());
        }
        assert!(render_planar(&[], None, 32000, 300, 60, &cancel).is_err());
        assert!(render_planar(&[0.5; 100], None, 0, 300, 60, &cancel).is_err());
        assert!(
            render_planar(&[0.5; 100], None, 32000, 300, 60, &AtomicBool::new(true))
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn packet_partitioning_and_channel_phase_do_not_change_the_image() {
        let cancel = AtomicBool::new(false);
        let left = tone(800.0);
        let right: Vec<_> = left.iter().map(|v| -*v).collect();
        let expected = render_planar(&left, Some(&right), 44100, 300, 60, &cancel).unwrap();
        for size in [37, 1152, 4096] {
            let mut builder = DadaspecBuilder::new(left.len(), 44100).unwrap();
            for offset in (0..left.len()).step_by(size) {
                for i in offset..(offset + size).min(left.len()) {
                    builder.push_frame([left[i], right[i]]);
                }
            }
            assert_eq!(builder.finish(300, 60, &cancel).unwrap(), expected);
        }
    }
    #[test]
    fn planar_packets_match_sample_feeding_across_resampling_and_overlap_boundaries() {
        let (mut left, right) = fixture(17001);
        left[123] = f32::NAN;
        left[1024] = f32::INFINITY;
        let cancel = AtomicBool::new(false);
        for rate in [8000, 22050, 32000, 44100, 48000, 96000] {
            for preview in [false, true] {
                let make = || {
                    if preview {
                        DadaspecBuilder::new_preview(left.len(), rate, 512)
                    } else {
                        DadaspecBuilder::new(left.len(), rate)
                    }
                    .unwrap()
                };
                let mut reference = make();
                for (&left, &right) in left.iter().zip(&right) {
                    reference.push_frame([left, right]);
                }
                let reference = reference.finish(512, 60, &cancel).unwrap();
                for packet in [1, 37, 1152, 4096] {
                    let mut builder = make();
                    for (left, right) in left.chunks(packet).zip(right.chunks(packet)) {
                        builder.push_planar(left, Some(right));
                    }
                    assert_eq!(
                        builder.finish(512, 60, &cancel).unwrap(),
                        reference,
                        "{rate} Hz, {packet} frames, preview={preview}"
                    );
                }
            }
        }
    }
    #[test]
    fn preview_overlap_adds_short_clip_detail_without_increasing_long_track_fft_work() {
        let (left, right) = fixture(32000);
        let mut canonical = DadaspecBuilder::new(left.len(), 32000).unwrap();
        let mut preview = DadaspecBuilder::new_preview(left.len(), 32000, 1024).unwrap();
        canonical.push_planar(&left, Some(&right));
        preview.push_planar(&left, Some(&right));
        assert_eq!(canonical.analyzer.powers.len(), 15);
        assert_eq!(preview.analyzer.powers.len(), 469);
        assert_eq!(preview.analyzer.hop, 64);
        let long = DadaspecBuilder::new_preview(32000 * 300, 32000, 1024).unwrap();
        assert_eq!(long.analyzer.hop, FFT);
    }
    #[test]
    fn resampling_clamps_rounded_endpoints_of_long_clips() {
        for frames in [13_230_000, 1 << 27] {
            let mut builder = DadaspecBuilder::new(frames, 44100).unwrap();
            // Exercise the final packet without retaining minutes of test PCM.
            builder.input_index = frames - 4;
            builder.output_index = builder.output_frames - 2;
            builder.previous = [0.25, -0.25];
            builder.push_planar(&[0.5; 4], Some(&[-0.5; 4]));
            assert_eq!(builder.input_index, frames);
            assert_eq!(builder.output_index, builder.output_frames);
            assert!(builder.finish(512, 60, &AtomicBool::new(false)).is_ok());
        }
    }
    #[test]
    fn simd_window_and_mel_projection_match_portable_scalar_results() {
        let plan = AnalysisPlan::shared();
        let (mut samples, _) = fixture(FFT);
        samples[123] = f32::NAN;
        samples[125] = f32::INFINITY;
        let mut scalar = vec![0.0; FFT];
        for ((out, &sample), &window) in scalar.iter_mut().zip(&samples).zip(&plan.window) {
            *out = if sample.is_finite() {
                sample * window
            } else {
                0.0
            };
        }
        let mut native = vec![0.0; FFT];
        dispatch!(Level::new(), simd => window_samples(simd, &samples, &plan.window, &mut native));
        assert_eq!(native, scalar);
        let mut spectrum = plan.fft.make_output_vec();
        plan.fft.process(&mut scalar, &mut spectrum).unwrap();
        let mut expected = [0.0; MELS];
        let mut actual = [0.0; MELS];
        for (power, filter) in expected.iter_mut().zip(&plan.filters) {
            *power = filter
                .weights
                .iter()
                .enumerate()
                .map(|(index, weight)| spectrum[filter.first + index].norm_sqr() * weight)
                .sum();
        }
        dispatch!(Level::new(), simd => mel_spectrum(simd, &spectrum, &plan.filters, &mut actual, 1));
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() <= 1e-5 * expected.max(1.0));
        }
    }
    #[test]
    fn guarded_file_decode_matches_planar_pcm_and_source_relative_excerpts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stereo.wav");
        let (left, right) = fixture(32_000);
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 2,
                sample_rate: 32_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for (&left, &right) in left.iter().zip(&right) {
            writer.write_sample(left).unwrap();
            writer.write_sample(right).unwrap();
        }
        writer.finalize().unwrap();
        let cancel = AtomicBool::new(false);
        for (start, duration) in [(0.0, 1.0), (0.25, 0.5)] {
            let file = crate::kernel_interface::open_content(&path, true).unwrap();
            let actual = render_file(file, &path, start, duration, 300, 60, &cancel).unwrap();
            let first = (start * 32000.0) as usize;
            let end = first + (duration * 32000.0) as usize;
            let mut expected = DadaspecBuilder::new_preview(end - first, 32000, 300).unwrap();
            expected.push_planar(&left[first..end], Some(&right[first..end]));
            let expected = expected.finish(300, 60, &cancel).unwrap();
            assert_eq!(actual, expected);
        }
        let file = crate::kernel_interface::open_content(&path, true).unwrap();
        let moved = path.with_extension("original");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"replaced input path").unwrap();
        assert!(render_file(file, &path, 0.0, 1.0, 300, 60, &cancel).is_ok());
        let file = std::fs::File::open("/dev/zero").unwrap();
        assert!(render_file(file, Path::new("/dev/zero"), 0.0, 1.0, 300, 60, &cancel).is_err());
    }

    #[test]
    fn long_inputs_bound_mel_storage_and_preserve_pending_tail_weights() {
        let mut analyzer = Analyzer::new();
        analyzer.powers = vec![[[4.0; MELS]; 2]; MAX_COLUMNS - 1];
        analyzer.transform();
        assert_eq!(analyzer.powers.len(), MAX_COLUMNS / 2);
        assert_eq!(analyzer.merge, 2);
        assert_eq!(analyzer.powers[0][0][0], 4.0);
        analyzer.pending = [[2.0; MELS]; 2];
        analyzer.pending_count = 1;
        assert!(analyzer.finish(300, 60, &AtomicBool::new(false)).is_ok());
    }
}
