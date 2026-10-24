//! Portable kits rendered on workers and published with the reference-pack installer.
use super::sample_pack::{SamplePackBank, SlotSample, install_pack};
use rodio::Source;
use std::{
    fs::File,
    io::BufWriter,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::Duration,
};

/// Saves portable layers and a relative-path bank, retaining originals for edited layers.
///
/// Sources and the output directory stay pinned through staging and publication.
/// Float PCM preserves gain above unity without integer clipping. The bus compressor
/// is a live performance effect and is not baked into individual layer files.
///
/// # Errors
/// Returns an error for invalid banks, unavailable/protected or changed sources,
/// trims beyond decoded audio, RIFF size limits, or staging/installation failures.
pub(super) fn export(root: &Path, bank: &SamplePackBank) -> Result<PathBuf, String> {
    install_pack(root, bank, |staged| stage(staged, bank))
}

fn stage(staged: &Path, bank: &SamplePackBank) -> Result<Vec<File>, String> {
    let mut rendered = bank.clone();
    rendered.validate(staged)?;
    let mut sources = Vec::new();
    std::fs::create_dir(staged.join("originals")).map_err(|e| e.to_string())?;
    for (index, slot) in rendered.slots.iter_mut().enumerate() {
        for (layer, sample) in slot.samples.iter_mut().enumerate() {
            let source = little_clip_core::kernel_interface::open_content(&sample.file_path, true)
                .map_err(|e| e.to_string())?;
            let view = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
            let identity = crate::file_source::FileSource::capture(&view)?;
            let (name, duration) = if !sample.has_edits() {
                let extension = sample
                    .file_path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy();
                let name = format!("pad{:02}_layer{}.{}", index + 1, layer + 1, extension);
                copy_source(&source, &staged.join(&name))?;
                (name, sample.duration)
            } else {
                let name = format!("pad{:02}_layer{}.wav", index + 1, layer + 1);
                let duration = render(&source, sample, &staged.join(&name))?;
                collect_original(staged, index, layer, sample, &source)?;
                (name, duration)
            };
            identity.check()?;
            sources.push(source);
            sample.file_path = name.into();
            sample.start_sec = 0.0;
            sample.end_sec = duration;
            sample.duration = duration;
            sample.volume = 1.0;
            sample.pan = 0.0;
            sample.attack_ms = 0.0;
            sample.release_ms = 0.0;
            sample.pitch_semitones = 0.0;
            sample.prepared_pitch = None;
            sample.cached_audio = None;
        }
    }
    let mut manifest = File::create(staged.join("bank.json")).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut manifest, &rendered).map_err(|e| e.to_string())?;
    manifest.sync_all().map_err(|e| e.to_string())?;
    Ok(sources)
}

/// Copies the complete pinned input independently of decoder cursor state.
fn collect_original(
    staged: &Path,
    index: usize,
    layer: usize,
    sample: &SlotSample,
    source: &File,
) -> Result<(), String> {
    let extension = sample
        .file_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy();
    let name = format!("pad{:02}_layer{}.{}", index + 1, layer + 1, extension);
    copy_source(source, &staged.join("originals").join(name))
}

/// Copies a pinned input independently of the decoder's shared cursor.
fn copy_source(source: &File, target: &Path) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    let mut original = File::create_new(target).map_err(|error| error.to_string())?;
    let mut input = source.try_clone().map_err(|error| error.to_string())?;
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    std::io::copy(&mut input, &mut original).map_err(|error| error.to_string())?;
    original.sync_all().map_err(|error| error.to_string())
}

/// Streams frame-aligned IEEE float WAV data without allocating the full result.
fn render(source: &File, sample: &SlotSample, target: &Path) -> Result<f32, String> {
    let mut decoder = rodio::Decoder::try_from(source.try_clone().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let channels = decoder.channels().get();
    let rate = decoder.sample_rate().get();
    if channels > super::MAX_SAMPLE_CHANNELS || rate > super::MAX_SAMPLE_RATE {
        return Err("Unsupported audio header".into());
    }
    let start = (sample.start_sec as f64 * rate as f64).round() as u64;
    let end = (sample.end_sec as f64 * rate as f64).round() as u64;
    let frames = end.saturating_sub(start);
    let expected_frames = (frames as f64 / sample.speed() as f64).ceil() as u64;
    expected_frames
        .checked_mul(8)
        .filter(|bytes| *bytes <= u32::MAX as u64 - 128)
        .ok_or("WAV layer exceeds RIFF size limit")?;
    if frames == 0 {
        return Err("Trim contains no complete audio frames".into());
    }
    let input: Box<dyn Source<Item = f32> + Send> = if sample.pitch_semitones != 0.0 {
        // Match preview's bounded preparation, using this same pinned input.
        let limit = (rate as usize * 15 * channels as usize).min(2_000_000);
        let values: Vec<_> = decoder.by_ref().take(limit).collect();
        let mut prepared = sample.clone();
        prepared.cached_audio = Some(std::sync::Arc::new(super::sample_pack::CachedAudio {
            channels,
            sample_rate: rate,
            samples: std::sync::Arc::new(values),
            mono_samples: std::sync::Arc::new(vec![]),
            waveform_peaks: vec![],
            waveform_colors: vec![],
        }));
        prepared.prepared_pitch = None;
        prepared.prepare_pitch()?;
        Box::new(
            super::sample_pack::SharedPcm::trimmed(&prepared).ok_or("Pitch source is not ready")?,
        )
    } else {
        decoder
            .try_seek(Duration::from_secs_f64(start as f64 / rate as f64))
            .map_err(|e| e.to_string())?;
        Box::new(decoder)
    };
    let layer =
        super::layer_source::LayerSource::new(input, sample, frames as usize).speed(sample.speed());
    let mut rendered = rodio::source::UniformSourceIterator::new(
        layer,
        2.try_into().unwrap(),
        rate.try_into().unwrap(),
    );
    let file = File::create(target).map_err(|e| e.to_string())?;
    let mut writer = hound::WavWriter::new(
        BufWriter::new(&file),
        hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .map_err(|e| e.to_string())?;
    let mut written = 0_u64;
    for value in &mut rendered {
        if written >= (u32::MAX as u64 - 128) / 4 {
            return Err("WAV layer exceeds RIFF size limit".into());
        }
        if !value.is_finite() {
            return Err("Non-finite source sample".into());
        }
        writer.write_sample(value).map_err(|e| e.to_string())?;
        written += 1;
    }
    if (written / 2).abs_diff(expected_frames) > 2 || written == 0 {
        return Err("Source is shorter than the saved trim".into());
    }
    writer.finalize().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(written as f32 / 2.0 / rate as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_and_speed_exports_match_preview_timing_and_reset_rendered_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tone.wav");
        let mut wav = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..24000 {
            wav.write_sample(
                ((std::f32::consts::TAU * 440.0 * i as f32 / 48000.0).sin() * 8000.0) as i16,
            )
            .unwrap();
        }
        wav.finalize().unwrap();
        for mode in [
            super::super::sample_pack::PitchMode::Pitch,
            super::super::sample_pack::PitchMode::Time,
        ] {
            let mut bank = SamplePackBank::default();
            let mut sample = SlotSample::new(path.clone(), "tone.wav".into(), None, 0.5);
            sample.pitch_semitones = 12.0;
            sample.pitch_mode = mode;
            sample.ensure_audio_loaded();
            sample.prepare_pitch().unwrap();
            let pcm = super::super::sample_pack::SharedPcm::trimmed(&sample).unwrap();
            let live = super::super::layer_source::LayerSource::new(pcm, &sample, 24000)
                .speed(sample.speed());
            let expected: Vec<_> = rodio::source::UniformSourceIterator::new(
                live,
                2.try_into().unwrap(),
                48000.try_into().unwrap(),
            )
            .collect();
            bank.slots[0].samples.push(sample);
            let saved = export(root.path(), &bank).unwrap();
            let manifest = super::super::sample_pack::load_sample_pack(&saved).unwrap();
            let rendered = &manifest.slots[0].samples[0];
            assert_eq!(rendered.pitch_semitones, 0.0);
            assert_eq!(rendered.pitch_mode, mode);
            let samples: Vec<_> = hound::WavReader::open(&rendered.file_path)
                .unwrap()
                .samples::<f32>()
                .map(Result::unwrap)
                .collect();
            assert_eq!(samples.len(), expected.len());
            assert!(
                samples
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| (*a - b).abs() < 0.0001)
            );
            let duration = if mode == super::super::sample_pack::PitchMode::Pitch {
                0.5
            } else {
                0.25
            };
            assert!((rendered.duration - duration).abs() < 0.0001);
            assert!(saved.join("originals/pad01_layer1.wav").is_file());
        }
    }

    #[test]
    fn automatic_pack_save_copies_unedited_layers_once_and_keeps_edited_originals() {
        let root = if let Some(path) = std::env::var_os("LITTLE_CLIP_PACK_TEST_ROOT") {
            tempfile::Builder::new()
                .prefix(".little-clip-pack-test-")
                .tempdir_in(path)
                .unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };
        let path = root.path().join("source.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 48000, 1);
        buffer.push_samples(&vec![8192; 4800]);
        buffer.write_wav(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut bank = SamplePackBank::new("Automatic");
        bank.slots[0].samples.push(SlotSample::new(
            path.clone(),
            "source.wav".into(),
            None,
            0.1,
        ));
        let first = export(root.path(), &bank).unwrap();
        assert_eq!(
            std::fs::read(first.join("pad01_layer1.wav")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_dir(first.join("originals")).unwrap().count(),
            0
        );
        bank.slots[0].samples[0].volume = 0.5;
        let edited = export(root.path(), &bank).unwrap();
        assert_ne!(first, edited);
        assert_eq!(
            std::fs::read(edited.join("originals/pad01_layer1.wav")).unwrap(),
            original
        );
        let loaded = super::super::sample_pack::load_sample_pack(&edited).unwrap();
        assert_eq!(loaded.slots[0].samples[0].filename, "source.wav");
        let values: Vec<_> =
            rodio::Decoder::try_from(File::open(&loaded.slots[0].samples[0].file_path).unwrap())
                .unwrap()
                .collect();
        assert!(values.iter().all(|value| (*value - 0.125).abs() < 0.0001));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn sub_millisecond_hits_keep_their_actual_duration_through_cache_and_export() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("click.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 48000, 1);
        buffer.push_samples(&[8192; 16]);
        buffer.write_wav(&path).unwrap();
        let mut sample = SlotSample::new(path, "click.wav".into(), None, 16.0 / 48000.0);
        sample.ensure_audio_loaded();
        assert!((sample.duration - 16.0 / 48000.0).abs() < 0.000001);
        let mut bank = SamplePackBank::new("Short");
        bank.slots[0].samples.push(sample);
        let exported = export(root.path(), &bank).unwrap();
        let decoder =
            rodio::Decoder::try_from(File::open(exported.join("pad01_layer1.wav")).unwrap())
                .unwrap();
        assert_eq!(decoder.count(), 16);
    }

    #[test]
    fn exported_kit_survives_original_removal_and_preserves_trim_and_gain() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("hit.wav");
        // A real generated WAV fixture, independent of the export writer.
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 48000, 1);
        buffer.push_samples(&vec![8192; 48000]);
        buffer.write_wav(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        let mut bank = SamplePackBank::new("Portable");
        let mut layer = SlotSample::new(source.clone(), "hit.wav".into(), None, 1.0);
        layer.start_sec = 0.25;
        layer.end_sec = 0.75;
        layer.volume = 0.5;
        bank.slots[0].samples.push(layer);
        let path = export(root.path(), &bank).unwrap();
        assert_eq!(
            std::fs::read(path.join("originals/pad01_layer1.wav")).unwrap(),
            original
        );
        bank.slots[0].samples[0].pan = 1.0;
        bank.slots[0].samples[0].attack_ms = 2.0;
        bank.slots[0].samples[0].release_ms = 2.0;
        let second = export(root.path(), &bank).unwrap();
        let effects =
            rodio::Decoder::try_from(File::open(second.join("pad01_layer1.wav")).unwrap())
                .unwrap()
                .collect::<Vec<_>>();
        assert_eq!(&effects[..2], &[0.0, 0.0]);
        assert_eq!(&effects[effects.len() - 2..], &[0.0, 0.0]);
        assert_eq!(effects[10000], 0.0);
        assert!((effects[10001] - 0.125).abs() < 0.0001);
        assert_ne!(path, second);
        std::fs::remove_file(source).unwrap();
        let relocated = root.path().join("relocated");
        std::fs::rename(path, &relocated).unwrap();
        let loaded = super::super::sample_pack::load_sample_pack(&relocated).unwrap();
        let layer = &loaded.slots[0].samples[0];
        assert_eq!(layer.volume, 1.0);
        assert_eq!(layer.start_sec, 0.0);
        let decoder = rodio::Decoder::try_from(File::open(&layer.file_path).unwrap()).unwrap();
        let values: Vec<_> = decoder.collect();
        assert_eq!(values.len(), 48000);
        assert!(values.iter().all(|value| (*value - 0.125).abs() < 0.0001));
        assert!(export(root.path(), &bank).is_err());
        let entries = std::fs::read_dir(super::super::sample_pack::get_sample_packs_root(
            root.path(),
        ))
        .unwrap()
        .count();
        assert_eq!(entries, 1); // Only the second complete export remains.
    }
}
