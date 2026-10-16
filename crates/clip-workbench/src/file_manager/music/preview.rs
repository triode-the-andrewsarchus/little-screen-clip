//! Visible music cards retain small summaries, never a per-row Audio Studio or PCM buffer.
use super::super::media::PreviewData as Data;
use crate::icons::Icon;
use little_clip_core::process::CommandRequest;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{path::Path, time::Duration};

pub(in crate::file_manager) use super::super::media::MusicMetadata as Metadata;

pub(in crate::file_manager) fn load(path: &Path, cancelled: &AtomicBool) -> Data {
    let mut data = Data::new(
        super::super::display_path(path),
        "Music".into(),
        Icon::Audio,
    );
    let result: Result<(), String> = (|| {
        let source = crate::file_source::FileSource::capture(path)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Music preview cancelled".into());
        }
        // Header probing is inexpensive; expensive tags/cover processes run
        // alongside audio decoding instead of delaying the waveform's start.
        let duration = little_clip_core::audio::read_audio_metadata(path)
            .map_or(0.0, |metadata| metadata.duration_ms as f32 / 1000.0);
        let ((mut metadata, image), result) = std::thread::scope(|scope| {
            let details =
                scope.spawn(|| (metadata(path), super::super::media::media_frame(path, 0, 0)));
            let result = waveform(path, duration, 512, None, cancelled);
            details
                .join()
                .map(|details| (details, result))
                .map_err(|_| "Music details worker failed".to_owned())
        })?;
        metadata.source = Some(source.clone());
        data.title = metadata.title.clone();
        data.image = image;
        data.music = Some(metadata);
        let (waveform, colors) = result?;
        data.waveform = waveform;
        data.waveform_colors = colors;
        source.check()?;
        if crate::file_source::FileSource::capture(path)? != source {
            return Err("Music path changed while preparing its preview".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        data.detail = format!("Music preview unavailable: {error}");
    }
    data
}

fn metadata(path: &Path) -> Metadata {
    let mut metadata = Metadata {
        title: path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        ..Default::default()
    };
    if let Some(audio) = little_clip_core::audio::read_audio_metadata(path) {
        metadata.duration = audio.duration_ms as f32 / 1000.0;
    }
    let probe = CommandRequest::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:format_tags=title,artist,album",
            "-of",
            "json",
        ])
        .arg_content_file(path)
        .ok()
        .and_then(|command| {
            command
                .timeout(Duration::from_secs(3))
                .max_output_bytes(64 * 1024)
                .run()
                .ok()
        });
    if let Some(probe) = probe
        && probe.status.success()
        && !probe.stdout_truncated
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&probe.stdout)
    {
        let format = &value["format"];
        if let Some(duration) = format["duration"]
            .as_str()
            .and_then(|n| n.parse::<f32>().ok())
            .filter(|n| n.is_finite() && *n > 0.0)
        {
            metadata.duration = duration;
        }
        for (key, output) in [
            ("title", &mut metadata.title),
            ("artist", &mut metadata.artist),
            ("album", &mut metadata.album),
        ] {
            if let Some(tag) = format["tags"]
                .as_object()
                .and_then(|tags| tags.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)))
                .and_then(|(_, value)| value.as_str())
            {
                *output = tag.chars().take(512).collect();
            }
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;
    use std::time::Instant;
    /// Manual release benchmark; the fixture folder is supplied outside the repository.
    #[test]
    #[ignore]
    fn benchmark_music_preview_album() {
        let directory = std::env::var_os("FILES_WAVEFORM_BENCH_DIR")
            .expect("set FILES_WAVEFORM_BENCH_DIR to an album folder");
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| little_clip_core::audio::is_audio_path(path))
            .collect();
        paths.sort();
        for round in 0..3 {
            let ctx = egui::Context::default();
            let mut cache = super::super::super::shelf::preview::Previews::default();
            let entries: Vec<_> = paths
                .iter()
                .map(|path| (uuid::Uuid::new_v4(), path))
                .collect();
            let started = Instant::now();
            loop {
                cache.poll(&ctx);
                for (id, path) in &entries {
                    if cache.needs(*id) {
                        cache.request(
                            *id,
                            super::super::super::shelf::preview::Source::Music((*path).clone()),
                        );
                    }
                }
                if entries.iter().all(|(id, _)| cache.get(*id).is_some()) {
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(120));
                std::thread::sleep(Duration::from_millis(16));
            }
            for (id, _) in &entries {
                assert!(!cache.get(*id).unwrap().data.waveform.is_empty());
            }
            println!(
                "CACHE round={round} album_ms={:.3}",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        for round in 0..3 {
            for path in &paths {
                let started = Instant::now();
                let info = metadata(path);
                let metadata_ms = started.elapsed().as_secs_f64() * 1000.0;
                let started = Instant::now();
                let _cover = super::super::super::media::media_frame(path, 0, 0);
                let cover_ms = started.elapsed().as_secs_f64() * 1000.0;
                let started = Instant::now();
                let result = waveform(path, info.duration, 512, None, &AtomicBool::new(false));
                let wave_ms = started.elapsed().as_secs_f64() * 1000.0;
                assert!(result.is_ok(), "{}: {:?}", path.display(), result.err());
                println!(
                    "BENCH round={round} track={} duration_s={:.3} metadata_ms={metadata_ms:.3} cover_ms={cover_ms:.3} waveform_ms={wave_ms:.3} total_ms={:.3}",
                    path.file_name().unwrap().to_string_lossy(),
                    info.duration,
                    metadata_ms + cover_ms + wave_ms
                );
            }
        }
    }

    #[test]
    fn common_compressed_music_formats_decode_to_bounded_waveforms() {
        let (directory, source) = crate::audio_preview::tests::generate_fixture_wav("formats", 0.3);
        for (extension, codec) in [("mp3", "libmp3lame"), ("ogg", "libvorbis"), ("m4a", "aac")] {
            let path = directory.path().join(format!("track.{extension}"));
            let output = CommandRequest::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(source.as_os_str())
                .args(["-ar", "44100", "-ac", "2", "-c:a", codec])
                .arg(path.as_os_str())
                .timeout(Duration::from_secs(5))
                .run()
                .unwrap();
            assert!(output.status.success(), "{}", output.stderr_lossy());
            let duration = metadata(&path).duration;
            let mut decoder = super::super::decode_file(&path).unwrap();
            decoder
                .try_seek(Duration::from_secs_f32(duration * 0.5))
                .unwrap();
            let wave = waveform(&path, duration, 512, None, &AtomicBool::new(false))
                .unwrap_or_else(|error| panic!("{extension}: {error}"))
                .0;
            assert_eq!(wave.len(), 512);
            assert!(
                wave.iter().any(|peak| *peak > 0.01),
                "{extension} must decode real samples"
            );
        }
    }
    #[test]
    fn music_waveforms_are_full_length_fixed_size_and_cancellable() {
        let (_dir, path) = crate::audio_preview::tests::generate_fixture_wav("preview", 2.0);
        let peaks = waveform(&path, 2.0, 512, None, &AtomicBool::new(false))
            .unwrap()
            .0;
        assert_eq!(peaks.len(), 512);
        assert!(
            peaks
                .iter()
                .all(|p| p.is_finite() && (0.0..=1.0).contains(p))
        );
        assert!(peaks[..256].iter().any(|p| *p > 0.0));
        assert!(peaks[256..].iter().any(|p| *p > 0.0));
        assert!(super::waveform(&path, 2.0, 512, None, &AtomicBool::new(true)).is_err());
    }
    #[test]
    fn music_metadata_and_missing_cover_keep_playable_waveform_data() {
        let (_dir, path) = crate::audio_preview::tests::generate_fixture_wav("metadata", 1.0);
        let data = load(&path, &AtomicBool::new(false));
        assert_eq!(data.waveform.len(), 512);
        let metadata = data.music.unwrap();
        assert!(metadata.title.contains("metadata"));
        assert!((metadata.duration - 1.0).abs() < 0.02);
        assert!(metadata.source.unwrap().check().is_ok());
        assert!(data.image.is_none());
    }
}

pub(in crate::file_manager) fn waveform(
    path: &Path,
    duration: f32,
    bars: usize,
    max_duration: Option<f32>,
    cancelled: &AtomicBool,
) -> Result<(Vec<f32>, Vec<[u8; 3]>), String> {
    little_clip_core::audio::compute_preview_waveform(
        super::audio_file(path)?,
        path,
        duration,
        bars,
        max_duration,
        cancelled,
    )
}
