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

/// Renders one WAV per layer and a relative-path bank, optionally collecting originals.
///
/// Sources and the output directory stay pinned through staging and publication.
/// Float PCM preserves gain above unity without integer clipping. The bus compressor
/// is a live performance effect and is not baked into individual layer files.
///
/// # Errors
/// Returns an error for invalid banks, unavailable/protected or changed sources,
/// trims beyond decoded audio, RIFF size limits, or staging/installation failures.
pub(super) fn export(
    root: &Path,
    bank: &SamplePackBank,
    originals: bool,
) -> Result<PathBuf, String> {
    install_pack(root, bank, |staged| stage(staged, bank, originals))
}

fn stage(staged: &Path, bank: &SamplePackBank, originals: bool) -> Result<Vec<File>, String> {
    let mut rendered = bank.clone();
    rendered.validate(staged)?;
    let mut sources = Vec::new();
    if originals {
        std::fs::create_dir(staged.join("originals")).map_err(|e| e.to_string())?;
    }
    for (index, slot) in rendered.slots.iter_mut().enumerate() {
        for (layer, sample) in slot.samples.iter_mut().enumerate() {
            let source = little_clip_core::kernel_interface::open_content(&sample.file_path, true)
                .map_err(|e| e.to_string())?;
            let view = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
            let identity = crate::file_source::FileSource::capture(&view)?;
            let name = format!("pad{:02}_layer{}.wav", index + 1, layer + 1);
            let duration = render(&source, sample, &staged.join(&name))?;
            if originals {
                collect_original(staged, index, layer, sample, &source)?;
            }
            identity.check()?;
            sources.push(source);
            sample.file_path = name.into();
            sample.filename = sample
                .file_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            sample.start_sec = 0.0;
            sample.end_sec = duration;
            sample.duration = duration;
            sample.volume = 1.0;
            sample.pan = 0.0;
            sample.attack_ms = 0.0;
            sample.release_ms = 0.0;
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
    let mut original =
        File::create(staged.join("originals").join(name)).map_err(|e| e.to_string())?;
    // try_clone shares the cursor with the decoder; rewind explicitly.
    use std::io::{Seek, SeekFrom};
    let mut input = source.try_clone().map_err(|e| e.to_string())?;
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    std::io::copy(&mut input, &mut original).map_err(|e| e.to_string())?;
    original.sync_all().map_err(|e| e.to_string())?;
    Ok(())
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
    let bytes = frames
        .checked_mul(2 * 4)
        .filter(|bytes| *bytes <= u32::MAX as u64 - 128)
        .ok_or("WAV layer exceeds RIFF size limit")? as u32;
    if frames == 0 {
        return Err("Trim contains no complete audio frames".into());
    }
    decoder
        .try_seek(Duration::from_secs_f64(start as f64 / rate as f64))
        .map_err(|e| e.to_string())?;
    let mut decoder = super::layer_source::LayerSource::new(decoder, sample, frames as usize);
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
    for _ in 0..bytes / 4 {
        let value = decoder
            .next()
            .ok_or("Source is shorter than the saved trim")?;
        if !value.is_finite() {
            return Err("Non-finite source sample".into());
        }
        writer.write_sample(value).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(frames as f32 / rate as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let exported = export(root.path(), &bank, false).unwrap();
        let decoder =
            rodio::Decoder::try_from(File::open(exported.join("pad01_layer1.wav")).unwrap())
                .unwrap();
        assert_eq!(decoder.count(), 32);
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
        let path = export(root.path(), &bank, true).unwrap();
        assert_eq!(
            std::fs::read(path.join("originals/pad01_layer1.wav")).unwrap(),
            original
        );
        bank.slots[0].samples[0].pan = 1.0;
        bank.slots[0].samples[0].attack_ms = 2.0;
        bank.slots[0].samples[0].release_ms = 2.0;
        let second = export(root.path(), &bank, false).unwrap();
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
        assert!(export(root.path(), &bank, false).is_err());
        let entries = std::fs::read_dir(super::super::sample_pack::get_sample_packs_root(
            root.path(),
        ))
        .unwrap()
        .count();
        assert_eq!(entries, 1); // Only the second complete export remains.
    }
}
