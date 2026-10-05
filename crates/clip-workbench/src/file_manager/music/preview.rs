//! Visible music cards retain small summaries, never a per-row Audio Studio or PCM buffer.
use super::super::shelf::preview::Data;
use crate::icons::Icon;
use little_clip_core::{ImageThumbnail, process::CommandRequest};
use rodio::Source;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(in crate::file_manager) struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f32,
    pub source: Option<crate::file_source::FileSource>,
}

pub(in crate::file_manager) fn load(path: &Path, cancelled: &AtomicBool) -> Data {
    let mut data = Data::new(
        super::super::display_path(path),
        "Music".into(),
        Icon::Audio,
    );
    let result = (|| {
        let source = crate::file_source::FileSource::capture(path)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Music preview cancelled".into());
        }
        let mut metadata = metadata(path);
        metadata.source = Some(source.clone());
        data.title = metadata.title.clone();
        if cancelled.load(Ordering::Relaxed) {
            return Err("Music preview cancelled".into());
        }
        data.image = cover(path);
        let duration = metadata.duration;
        data.music = Some(metadata);
        data.waveform = peaks(path, duration, cancelled)?;
        source.check()
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
        .arg(path.as_os_str())
        .timeout(Duration::from_secs(3))
        .max_output_bytes(64 * 1024)
        .run();
    if let Ok(probe) = probe
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
    #[test]
    fn music_waveforms_are_full_length_fixed_size_and_cancellable() {
        let (_dir, path) = crate::audio_preview::tests::generate_fixture_wav("preview", 2.0);
        let peaks = peaks(&path, 2.0, &AtomicBool::new(false)).unwrap();
        assert_eq!(peaks.len(), 512);
        assert!(
            peaks
                .iter()
                .all(|p| p.is_finite() && (0.0..=1.0).contains(p))
        );
        assert!(peaks[..256].iter().any(|p| *p > 0.0));
        assert!(peaks[256..].iter().any(|p| *p > 0.0));
        assert!(super::peaks(&path, 2.0, &AtomicBool::new(true)).is_err());
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

fn cover(path: &Path) -> Option<ImageThumbnail> {
    let result = CommandRequest::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path.as_os_str())
        .args([
            "-map",
            "0:v:0",
            "-vf",
            "scale=160:160:force_original_aspect_ratio=decrease",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "pipe:1",
        ])
        .timeout(Duration::from_secs(3))
        .max_output_bytes(256 * 1024)
        .run()
        .ok()?;
    if !result.status.success() || result.stdout_truncated {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(4 * 1024 * 1024);
    limits.max_image_width = Some(160);
    limits.max_image_height = Some(160);
    ImageThumbnail::generate_with_limits(&result.stdout, Some("image/png"), 160, 160, limits)
}

fn peaks(path: &Path, duration: f32, cancelled: &AtomicBool) -> Result<Vec<f32>, String> {
    let decoder = super::decode_file(path)?;
    let duration = decoder
        .total_duration()
        .map_or(duration, |d| d.as_secs_f32());
    if !duration.is_finite() || duration <= 0.0 {
        return Err("Unknown track duration".into());
    }
    let sample_count =
        (duration as f64 * decoder.sample_rate().get() as f64 * decoder.channels().get() as f64)
            .max(1.0);
    let mut peaks = vec![0.0_f32; 512];
    let started = Instant::now();
    for (index, sample) in decoder.enumerate() {
        if index % 65536 == 0 {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Music preview cancelled".into());
            }
            if started.elapsed() > Duration::from_secs(10) {
                return Err("Waveform exceeded the ten-second decoding budget".into());
            }
        }
        let bucket = ((index as f64 / sample_count) * peaks.len() as f64) as usize;
        let bucket = bucket.min(peaks.len() - 1);
        if sample.is_finite() {
            peaks[bucket] = peaks[bucket].max(sample.abs().min(1.0));
        }
    }
    Ok(peaks)
}
