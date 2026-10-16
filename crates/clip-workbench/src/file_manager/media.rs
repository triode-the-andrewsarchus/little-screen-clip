//! Shared bounded media inspection and preview data, independent of caches and playback.
use crate::{file_source::FileSource, icons::Icon};
use little_clip_core::ImageThumbnail;
use std::time::Duration;

mod video_details;

#[derive(Default)]
pub(in crate::file_manager) struct MusicMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f32,
    pub source: Option<crate::file_source::FileSource>,
}

pub(in crate::file_manager) struct PreviewData {
    pub title: String,
    pub detail: String,
    pub text: String,
    pub image: Option<ImageThumbnail>,
    pub spectrum: Option<ImageThumbnail>,
    pub waveform: Vec<f32>,
    pub waveform_colors: Vec<[u8; 3]>,
    pub icon: Icon,
    pub music: Option<MusicMetadata>,
    pub photo: Option<std::sync::Arc<super::image_strip::Analysis>>,
    pub midi_document: Option<std::sync::Arc<super::midi_strip::Document>>,
    pub pdf_info: Option<std::sync::Arc<little_clip_core::pdf::PdfInfo>>,
    pub system: Option<std::sync::Arc<super::system::Document>>,
    pub font: Option<std::sync::Arc<super::font_strip::Document>>,
    pub binary: Option<std::sync::Arc<super::binary_strip::Document>>,
    pub document: Option<std::sync::Arc<super::text_strip::Document>>,
    pub materials:
        Option<std::sync::Arc<Vec<std::sync::Arc<little_clip_core::model3d::materials::Material>>>>,
}

impl PreviewData {
    pub(in crate::file_manager) fn new(title: String, detail: String, icon: Icon) -> Self {
        Self {
            title,
            detail,
            icon,
            text: String::new(),
            image: None,
            spectrum: None,
            waveform: Vec::new(),
            waveform_colors: Vec::new(),
            music: None,
            document: None,
            binary: None,
            font: None,
            system: None,
            pdf_info: None,
            midi_document: None,
            photo: None,
            materials: None,
        }
    }
}

pub(super) fn media_frame(
    path: &std::path::Path,
    stream: usize,
    time_ms: u64,
) -> Option<little_clip_core::ImageThumbnail> {
    let mut command = little_clip_core::process::CommandRequest::new("ffmpeg").args([
        "-v",
        "error",
        "-threads",
        "2",
        "-filter_threads",
        "1",
        "-max_alloc",
        "268435456",
    ]);
    if time_ms > 0 {
        // Input-side seeking decodes only the GOP near the chosen thumbnail.
        command = command.args(["-ss", &format!("{:.3}", time_ms as f64 / 1000.0)]);
    }
    let result = command
        .arg("-i")
        .arg_content_file(path)
        .ok()?
        .arg("-map")
        .arg(format!("0:v:{stream}"))
        .args([
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
        .timeout(std::time::Duration::from_secs(3))
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
    little_clip_core::ImageThumbnail::generate_with_limits(
        &result.stdout,
        Some("image/png"),
        160,
        160,
        limits,
    )
}

pub(super) struct VideoMetadata {
    pub(super) source: FileSource,
    pub(super) dimensions: (u32, u32),
    pub(super) duration_ms: u64,
    pub(super) stream: usize,
    pub(super) codec: String,
    pub(super) size_bytes: u64,
    pub(super) frame_interval_ms: u64,
    pub(super) description: String,
    pub(super) audio_description: String,
}

pub(super) fn inspect_video(path: &std::path::Path) -> Result<VideoMetadata, String> {
    let source = FileSource::capture(path)?;
    let probe = little_clip_core::process::CommandRequest::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,profile,width,height,avg_frame_rate,r_frame_rate,bit_rate,channels,channel_layout:stream_disposition=attached_pic,default:stream_tags=BPS,BPS-eng,bps,bps-eng,DATE,date,YEAR,year,release_year,RELEASE_YEAR,release_date,RELEASE_DATE,date_released,DATE_RELEASED,originaldate,ORIGINALDATE:format=duration,size:format_tags=DATE,date,YEAR,year,release_year,RELEASE_YEAR,release_date,RELEASE_DATE,date_released,DATE_RELEASED,originaldate,ORIGINALDATE",
            "-of",
            "json",
        ])
        .arg_content_file(path).map_err(|e|e.to_string())?
        .timeout(Duration::from_secs(3))
        .max_output_bytes(64 * 1024)
        .run()
        .map_err(|e| format!("Cannot inspect video; FFprobe is required: {e}"))?;
    if !probe.status.success() || probe.stdout_truncated {
        return Err("Cannot inspect video. Check the file or open externally.".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&probe.stdout).map_err(|e| e.to_string())?;
    let (stream, width, height, duration_ms) = stream_metadata(&value)?;
    if FileSource::capture(path)? != source {
        return Err("Video changed while opening; open it again.".into());
    }
    let (_, video) = selected_video_stream(&value)?;
    let (description, audio_description) =
        video_details::describe(&value, video, duration_ms, (width, height));
    Ok(VideoMetadata {
        source,
        dimensions: (width, height),
        duration_ms,
        stream,
        size_bytes: value["format"]["size"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        codec: video["codec_name"].as_str().unwrap_or_default().into(),
        frame_interval_ms: video_details::frame_rate(video)
            .map(|fps| (1000.0 / fps).ceil().clamp(1.0, 60_000.0) as u64)
            .unwrap_or(40),
        description,
        audio_description,
    })
}

pub(in crate::file_manager) fn video_preview(
    path: &std::path::Path,
) -> Option<little_clip_core::ImageThumbnail> {
    let prepared = inspect_video(path).ok()?;
    let mut image = media_frame(path, prepared.stream, 0)?;
    let black = image
        .premultiplied_rgba
        .chunks_exact(4)
        .all(|pixel| pixel[..3].iter().all(|component| *component <= 8));
    if black && prepared.duration_ms > 0 {
        if let Some(middle) = media_frame(path, prepared.stream, prepared.duration_ms / 2) {
            image = middle;
        }
    }
    (FileSource::capture(path).ok()? == prepared.source).then_some(image)
}

// Core's preview metadata reader does not distinguish an attached cover from the
// stream played here. Inspect one selected stream and pass its index to FFplay.
fn stream_metadata(value: &serde_json::Value) -> Result<(usize, u32, u32, u64), String> {
    let (index, stream) = selected_video_stream(value)?;
    let width = stream["width"].as_u64().unwrap_or(0);
    let height = stream["height"].as_u64().unwrap_or(0);
    if width == 0 || height == 0 || width > 4096 || height > 4096 || width * height > 4096 * 2160 {
        return Err("Video dimensions are invalid or exceed the 4096×2160 pixel budget; use Open externally.".into());
    }
    let duration_ms = value["format"]["duration"]
        .as_str()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map_or(0, |seconds| (seconds * 1000.0) as u64);
    Ok((index, width as u32, height as u32, duration_ms))
}

fn selected_video_stream(value: &serde_json::Value) -> Result<(usize, &serde_json::Value), String> {
    value["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stream| stream["codec_type"].as_str().unwrap_or("video") == "video")
        .enumerate()
        .find(|(_, stream)| stream["disposition"]["attached_pic"].as_u64().unwrap_or(0) == 0)
        .ok_or_else(|| "No ordinary video stream is available".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_stream_skips_cover_art_and_enforces_the_played_pixel_budget() {
        let mut metadata = serde_json::json!({
            "streams": [
                {"width":160, "height":160, "disposition":{"attached_pic":1}},
                {"width":1920, "height":1080, "disposition":{"attached_pic":0}}
            ], "format":{"duration":"3.5"}
        });
        assert_eq!(stream_metadata(&metadata).unwrap(), (1, 1920, 1080, 3500));
        metadata["streams"][1]["width"] = serde_json::json!(8192);
        assert!(stream_metadata(&metadata).is_err());
        assert!(stream_metadata(&serde_json::json!({"streams":[]})).is_err());
    }
}

/// Shared allocation and dimension limits for gallery and ordinary previews.
pub(super) fn raster_preview_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits
}
