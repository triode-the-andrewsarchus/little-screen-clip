//! Streaming native WAV rendering over admitted input and staging handles.

use std::{
    fs::File,
    io::BufWriter,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use symphonia::core::{
    audio::{Channels, Layout, SampleBuffer},
    codecs::{CODEC_TYPE_OPUS, CodecParameters, Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatReader, Packet, SeekMode, SeekTo},
    units::{Time, TimeBase},
};

/// Sample encoding for a native WAV output.
#[derive(Clone, Copy, Default)]
pub enum WavEncoding {
    /// Signed 16-bit PCM for ordinary exports.
    #[default]
    Pcm16,
    /// Floating-point PCM for intermediate processing without 16-bit quantization.
    Float32,
}

/// Selection, gain and encoding for streaming WAV rendering.
#[derive(Clone, Copy, Default)]
pub struct WavRenderOptions {
    /// Source-relative selection in seconds; `None` renders the whole track.
    pub range: Option<(f64, f64)>,
    /// Linear gain in decibels; negative infinity mutes the source.
    pub gain_db: f32,
    /// WAV sample encoding.
    pub encoding: WavEncoding,
}

impl WavRenderOptions {
    fn validate(self) -> Result<(), String> {
        if self.gain_db.is_nan()
            || self.gain_db == f32::INFINITY
            || self.range.is_some_and(|(start, end)| {
                !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start
            })
        {
            return Err("Invalid native audio render settings".into());
        }
        Ok(())
    }
}

/// Renders native audio into an owned, already-admitted WAV staging handle.
///
/// Processing retains only packet-sized PCM. The input extension is a probing
/// hint, not a pathname to reopen. Call this on a background worker and install
/// the staged result only after revalidating the source and output directory.
/// Containers requiring edit lists or non-sample clocks and non-conventional
/// channel layouts are left to the codec backend.
///
/// # Errors
/// Returns unsupported-format, decoding, seeking, cancellation or output errors.
/// Failed attempts clear and rewind staging before returning. Cleanup failures
/// are also returned as errors; successful output remains owned by the caller.
pub fn render_wav(
    input: File,
    hint: &Path,
    output: File,
    options: WavRenderOptions,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut reset = output.try_clone().map_err(|error| error.to_string())?;
    let result = render_inner(input, hint, output, options, cancelled);
    if result.is_err() {
        use std::io::Seek;
        reset.set_len(0).map_err(|error| error.to_string())?;
        reset.rewind().map_err(|error| error.to_string())?;
    }
    result
}

fn render_inner(
    input: File,
    hint: &Path,
    output: File,
    options: WavRenderOptions,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    check_cancelled(cancelled)?;
    options.validate()?;
    let mut format = super::source::format_reader(input, hint, cancelled, export_probe())?;
    let track = format.default_track().ok_or("No native audio track")?;
    let id = track.id;
    let params = track.codec_params.clone();
    let spec = native_wav_spec(&params, options.encoding)?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|error| error.to_string())?;
    let span = seek_span(&mut *format, &mut *decoder, id, &params, options.range)?;
    if let Some(frames) = params.n_frames {
        let frames = frames.min(span.last).saturating_sub(span.first);
        check_wav_size(
            frames.saturating_mul(u64::from(spec.channels)),
            spec.bits_per_sample,
        )?;
    }
    let output = WavOutput {
        writer: hound::WavWriter::new(BufWriter::new(output), spec)
            .map_err(|error| error.to_string())?,
        encoding: options.encoding,
        gain: super::apply_audio_gain(1.0, options.gain_db, false),
        written: 0,
    };
    write_packets(&mut *format, &mut *decoder, id, span, output, cancelled)
}

fn native_wav_spec(
    params: &CodecParameters,
    encoding: WavEncoding,
) -> Result<hound::WavSpec, String> {
    let rate = params
        .sample_rate
        .filter(|rate| *rate > 0)
        .ok_or("Unknown audio sample rate")?;
    if params.start_ts != 0
        || params
            .time_base
            .is_some_and(|base| u64::from(base.numer) * u64::from(rate) != u64::from(base.denom))
    {
        return Err("Container timing requires the codec backend for frame-accurate export".into());
    }
    let layout = params.channels.ok_or("Unknown audio channel layout")?;
    let channels = layout.count();
    // Hound's ordinary mono/stereo headers preserve their conventional order.
    // Other layouts need an explicit channel mask and use the library backend.
    if layout != Layout::Mono.into_channels()
        && layout != Channels::FRONT_CENTRE
        && layout != Layout::Stereo.into_channels()
    {
        return Err("Native WAV rendering requires mono or stereo audio".into());
    }
    let (bits_per_sample, sample_format) = match encoding {
        WavEncoding::Pcm16 => (16, hound::SampleFormat::Int),
        WavEncoding::Float32 => (32, hound::SampleFormat::Float),
    };
    Ok(hound::WavSpec {
        channels: channels as u16,
        sample_rate: rate,
        bits_per_sample,
        sample_format,
    })
}

fn check_wav_size(samples: u64, bits: u16) -> Result<(), String> {
    // Reserve header space as well as data; Hound uses 32-bit RIFF lengths.
    if samples
        .checked_mul(u64::from(bits / 8))
        .is_none_or(|bytes| bytes > u64::from(u32::MAX) - 128)
    {
        Err("Native WAV output exceeds the RIFF size limit".into())
    } else {
        Ok(())
    }
}

fn export_probe() -> &'static symphonia::core::probe::Probe {
    use symphonia::core::probe::{Instantiate, Probe, QueryDescriptor};
    static PROBE: std::sync::OnceLock<Probe> = std::sync::OnceLock::new();
    PROBE.get_or_init(|| {
        let mut probe = Probe::default();
        // This demuxer parses MP4 edit lists but does not apply their timeline.
        // Recognize the container before probing its payload and defer exports
        // to FFmpeg, preserving priming and edits even with a misleading suffix.
        for descriptor in symphonia::default::formats::IsoMp4Reader::query() {
            let mut descriptor = *descriptor;
            descriptor.inst = Instantiate::Format(|_, _| {
                Err(Error::Unsupported(
                    "MP4 edit-list timing requires the codec backend",
                ))
            });
            probe.register(&descriptor);
        }
        symphonia::default::register_enabled_formats(&mut probe);
        probe
    })
}

struct RenderCursor {
    first: u64,
    last: u64,
    clock_delay: u64,
    pending: Option<Packet>,
}

fn header_delay(
    format: &mut dyn FormatReader,
    track: u32,
    params: &CodecParameters,
) -> Result<(u64, Option<Packet>), String> {
    if params.codec != CODEC_TYPE_OPUS || params.delay.unwrap_or(0) == 0 {
        return Ok((0, None));
    }
    let packet = loop {
        let packet = format.next_packet().map_err(|error| error.to_string())?;
        if packet.track_id() == track {
            break packet;
        }
    };
    // The Opus adapter also honors OpusHead pre-skip. Some demuxers have
    // already normalized their packet clock and supplied trim_start; others
    // leave header trimming to the decoder. Normalize only the latter clock.
    let delay = if packet.trim_start() == 0 {
        u64::from(params.delay.unwrap_or(0))
    } else {
        0
    };
    Ok((delay, Some(packet)))
}

fn seek_span(
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track: u32,
    params: &CodecParameters,
    range: Option<(f64, f64)>,
) -> Result<RenderCursor, String> {
    let rate = params.sample_rate.ok_or("Unknown audio sample rate")?;
    let (clock_delay, pending) = header_delay(format, track, params)?;
    let Some((start, end)) = range else {
        return Ok(RenderCursor {
            first: 0,
            last: u64::MAX,
            clock_delay,
            pending,
        });
    };
    let mut span = RenderCursor {
        first: (start * f64::from(rate)).round() as u64,
        last: (end * f64::from(rate)).round() as u64,
        clock_delay,
        pending,
    };
    // Warm decoder overlap/history before the selected frame. Near the start,
    // decode from the beginning so codec priming remains identical to playback.
    let seek_start = (start - 0.25_f64.max(8192.0 / f64::from(rate))).max(0.0);
    if seek_start > 0.0 {
        let seek_time = seek_start + clock_delay as f64 / f64::from(rate);
        let seek = format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::new(seek_time as u64, seek_time.fract()),
                    track_id: Some(track),
                },
            )
            .map_err(|error| error.to_string())?;
        let actual = params
            .time_base
            .unwrap_or(TimeBase::new(1, rate))
            .calc_time(seek.actual_ts);
        let frame = (((actual.seconds as f64 + actual.frac) * f64::from(rate)).round() as u64)
            .saturating_sub(clock_delay);
        if frame > span.first {
            return Err("Native decoder sought past the selected frame".into());
        }
        decoder.reset();
        span.pending = None;
    }
    Ok(span)
}

impl RenderCursor {
    fn next_packet(
        &mut self,
        format: &mut dyn FormatReader,
        track: u32,
        cancelled: &AtomicBool,
    ) -> Result<Option<Packet>, String> {
        loop {
            check_cancelled(cancelled)?;
            let next = match self.pending.take() {
                Some(packet) => Ok(packet),
                None => format.next_packet(),
            };
            match next {
                Ok(packet) if packet.track_id() == track => return Ok(Some(packet)),
                Ok(_) => {}
                Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

// Encoding owns size, quantization and output policy independently of source
// clocks. Failed attempts are discarded by render_wav after this writer drops.
struct WavOutput {
    writer: hound::WavWriter<BufWriter<File>>,
    encoding: WavEncoding,
    gain: f32,
    written: u64,
}

impl WavOutput {
    fn append(&mut self, samples: &[f32], cancelled: &AtomicBool) -> Result<(), String> {
        check_wav_size(
            self.written.saturating_add(samples.len() as u64),
            self.writer.spec().bits_per_sample,
        )?;
        for (index, sample) in samples.iter().enumerate() {
            if index.is_multiple_of(4096) {
                check_cancelled(cancelled)?;
            }
            let sample = *sample * self.gain;
            if !sample.is_finite() {
                return Err("Non-finite sample during native rendering".into());
            }
            match self.encoding {
                WavEncoding::Float32 => self.writer.write_sample(sample),
                WavEncoding::Pcm16 => self.writer.write_sample(
                    (sample * 32768.0)
                        .round_ties_even()
                        .clamp(-32768.0, 32767.0) as i16,
                ),
            }
            .map_err(|error| error.to_string())?;
            self.written += 1;
        }
        Ok(())
    }

    fn finish(self) -> Result<(), String> {
        if self.written == 0 {
            return Err("No native audio samples in the selected range".into());
        }
        self.writer.finalize().map_err(|error| error.to_string())
    }
}

fn write_packets(
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track: u32,
    mut span: RenderCursor,
    mut output: WavOutput,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let spec = output.writer.spec();
    let channels = usize::from(spec.channels);
    let mut converted: Option<SampleBuffer<f32>> = None;
    while let Some(packet) = span.next_packet(format, track, cancelled)? {
        // The first packet after a seek may only prime a codec's overlap state
        // and produce no PCM. Its duration must not shift later decoded audio.
        // The accepted native time base is one sample per tick.
        let frame = packet.ts().saturating_sub(span.clock_delay);
        if frame >= span.last {
            break;
        }
        let decoded = decoder.decode(&packet).map_err(|error| error.to_string())?;
        if decoded.spec().rate != spec.sample_rate || decoded.spec().channels.count() != channels {
            return Err("Audio format changed during native rendering".into());
        }
        if decoded
            .capacity()
            .saturating_mul(channels)
            .saturating_mul(4)
            > 64 * 1024 * 1024
        {
            return Err("Native audio packet exceeds the conversion budget".into());
        }
        let buffer =
            super::decode::conversion_buffer(&mut converted, decoded.capacity(), *decoded.spec());
        buffer.copy_interleaved_ref(decoded);
        let samples = buffer.samples();
        let frames = (samples.len() / channels) as u64;
        let first = span.first.saturating_sub(frame).min(frames) as usize * channels;
        let last = span.last.saturating_sub(frame).min(frames) as usize * channels;
        output.append(&samples[first..last], cancelled)?;
        if frame
            .checked_add(frames)
            .ok_or("Audio frame count overflow")?
            >= span.last
        {
            break;
        }
    }
    check_cancelled(cancelled)?;
    output.finish()
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("Audio rendering cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    #[test]
    fn failed_encoding_discards_partial_wav_and_rewinds_for_retry() {
        let mut input = tempfile::tempfile().unwrap();
        let mut writer = hound::WavWriter::new(
            input.try_clone().unwrap(),
            hound::WavSpec {
                channels: 1,
                sample_rate: 44100,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for sample in [0.25, 0.5, f32::NAN, 0.75] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        input.rewind().unwrap();
        let mut output = tempfile::tempfile().unwrap();
        let error = render_wav(
            input,
            Path::new("input.wav"),
            output.try_clone().unwrap(),
            WavRenderOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("Non-finite"));
        assert_eq!(output.metadata().unwrap().len(), 0);
        assert_eq!(output.stream_position().unwrap(), 0);
    }

    #[test]
    fn native_writer_rejects_riff_overflow_without_large_allocations() {
        assert!(check_wav_size((u64::from(u32::MAX) - 128) / 4, 32).is_ok());
        assert!(check_wav_size(u64::from(u32::MAX) / 4 + 1, 32).is_err());
        assert!(check_wav_size(u64::MAX, 16).is_err());
    }

    #[test]
    fn mp4_timeline_handling_is_deferred_before_output_even_with_a_wav_hint() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/video/with-cover.mp4");
        let output = tempfile::tempfile().unwrap();
        let error = render_wav(
            File::open(path).unwrap(),
            Path::new("misleading.wav"),
            output.try_clone().unwrap(),
            WavRenderOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("MP4 edit-list"));
        assert_eq!(output.metadata().unwrap().len(), 0);
    }

    #[test]
    #[ignore = "requires FFmpeg CLI only to generate compressed reference fixtures"]
    fn common_codec_seeks_match_selections_from_the_full_native_decode() {
        let (directory, source) =
            super::super::test_support::generate_fixture_wav("native-seek", 2.0);
        for (extension, codec) in [
            ("flac", "flac"),
            ("mp3", "libmp3lame"),
            ("aac", "aac"),
            ("ogg", "libvorbis"),
            ("opus", "libopus"),
        ] {
            let path = directory.path().join(format!("encoded.{extension}"));
            assert!(
                std::process::Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&source)
                    .args(["-c:a", codec])
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            let decode = |range| {
                let mut output = tempfile::tempfile().unwrap();
                render_wav(
                    File::open(&path).unwrap(),
                    &path,
                    output.try_clone().unwrap(),
                    WavRenderOptions {
                        range,
                        encoding: WavEncoding::Float32,
                        ..Default::default()
                    },
                    &AtomicBool::new(false),
                )
                .unwrap_or_else(|error| panic!("{extension}: {error}"));
                output.rewind().unwrap();
                let mut reader = hound::WavReader::new(output).unwrap();
                let spec = reader.spec();
                let samples: Vec<_> = reader.samples::<f32>().map(Result::unwrap).collect();
                (spec, samples)
            };
            let (spec, full) = decode(None);
            for (start, end) in [(0.025, 0.075), (1.25, 1.5)] {
                let (_, selected) = decode(Some((start, end)));
                let first = (start * f64::from(spec.sample_rate)).round() as usize
                    * usize::from(spec.channels);
                let last = (end * f64::from(spec.sample_rate)).round() as usize
                    * usize::from(spec.channels);
                assert_eq!(
                    selected.len(),
                    last - first,
                    "{extension} selected sample count"
                );
                let error = selected
                    .iter()
                    .zip(&full[first..last])
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                let rms = (selected
                    .iter()
                    .zip(&full[first..last])
                    .map(|(a, b)| f64::from(a - b).powi(2))
                    .sum::<f64>()
                    / selected.len() as f64)
                    .sqrt();
                eprintln!("{extension} at {start}: max={error}, rms={rms}");
                // Lossy decoders can restart their perceptual-noise synthesis
                // state on seek. Bound that small difference while rejecting
                // frame shifts and missing codec overlap/history on these tones.
                assert!(
                    error < 0.001 && rms < 0.0001,
                    "{extension} at {start}: maximum seek error {error}, RMS {rms}"
                );
            }
        }
    }

    #[test]
    fn native_selection_and_gain_preserve_stereo_frame_boundaries() {
        let (_directory, path) =
            super::super::test_support::generate_fixture_wav("native-render", 0.2);
        let source: Vec<_> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        let mut output = tempfile::tempfile().unwrap();
        render_wav(
            File::open(&path).unwrap(),
            &path,
            output.try_clone().unwrap(),
            WavRenderOptions {
                range: Some((0.025, 0.075)),
                gain_db: -6.0206,
                encoding: WavEncoding::Float32,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        output.rewind().unwrap();
        let samples: Vec<_> = hound::WavReader::new(output)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        let first = (0.025_f64 * 44100.0).round() as usize * 2;
        let last = (0.075_f64 * 44100.0).round() as usize * 2;
        assert_eq!(samples.len(), last - first);
        assert!(
            samples
                .iter()
                .zip(&source[first..last])
                .all(
                    |(actual, original)| (*actual - f32::from(*original) / 65536.0).abs()
                        < 0.000001
                )
        );
    }

    #[test]
    fn native_pcm_copy_is_exact_and_cancellation_does_not_start_output() {
        let (_directory, path) =
            super::super::test_support::generate_fixture_wav("native-copy", 0.05);
        let original: Vec<_> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        let mut output = tempfile::tempfile().unwrap();
        render_wav(
            File::open(&path).unwrap(),
            &path,
            output.try_clone().unwrap(),
            WavRenderOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        output.seek(SeekFrom::Start(0)).unwrap();
        let rendered: Vec<_> = hound::WavReader::new(output)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rendered, original);
        let output = tempfile::tempfile().unwrap();
        assert!(
            render_wav(
                File::open(&path).unwrap(),
                &path,
                output.try_clone().unwrap(),
                WavRenderOptions::default(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert_eq!(output.metadata().unwrap().len(), 0);
    }
}
