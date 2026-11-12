//! Shared bounded media inspection and preview data, independent of caches and playback.
use crate::{file_source::FileSource, icons::Icon};
use little_clip_core::ImageThumbnail;
use std::time::Duration;

pub(in crate::file_manager) use little_clip_core::media_stream as timing;
mod video_details;

#[derive(Default)]
pub(in crate::file_manager) struct MusicMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bit_rate: Option<u64>,
    pub bit_depth: Option<u16>,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub channel_layout: Option<String>,
    pub sample_format: Option<String>,
    pub encoder_settings: Option<String>,
    pub bitrate_mode: Option<String>,
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
    pub photo_rating: u8,
    pub photo_metadata: Option<little_clip_ops::metadata::ImageMetadataSummary>,
    pub photo: Option<std::sync::Arc<super::image_strip::Analysis>>,
    pub midi_document: Option<std::sync::Arc<super::midi_strip::Document>>,
    pub pdf_info: Option<std::sync::Arc<little_clip_core::pdf::PdfInfo>>,
    pub system: Option<std::sync::Arc<super::system::Document>>,
    pub subtitles: Option<std::sync::Arc<super::subtitle_strip::Summary>>,
    pub notebook: Option<std::sync::Arc<super::notebook::Summary>>,
    pub multisample: Option<std::sync::Arc<little_clip_core::multisample::Document>>,
    pub toolpath: Option<std::sync::Arc<little_clip_core::gcode::Document>>,
    pub geography: Option<std::sync::Arc<little_clip_core::geography::Document>>,
    pub database: Option<std::sync::Arc<super::database::Summary>>,
    pub calendar: Option<std::sync::Arc<super::calendar::Summary>>,
    pub mail: Option<std::sync::Arc<super::mail::Summary>>,
    pub book: Option<std::sync::Arc<super::ebook::Summary>>,
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
            book: None,
            mail: None,
            calendar: None,
            database: None,
            geography: None,
            toolpath: None,
            multisample: None,
            notebook: None,
            subtitles: None,
            system: None,
            pdf_info: None,
            midi_document: None,
            photo_rating: 0,
            photo_metadata: None,
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
    use little_clip_core::media_backend::{VideoFrame, VideoStream};
    let mut settings = VideoFrame::new(
        format!(
            "{},{}",
            little_clip_core::media_backend::VIDEO_FORMATS,
            little_clip_core::media_backend::AUDIO_FORMATS
        ),
        160,
        160,
    );
    settings.stream = VideoStream::Ordinal(stream);
    settings.seconds = time_ms as f64 / 1000.0;
    let result = little_clip_core::media_backend::video_frame_request(&settings)
        .ok()?
        .arg_content_file(path)
        .ok()?
        .timeout(std::time::Duration::from_secs(3))
        .max_output_bytes(256 * 1024)
        .run()
        .ok()?;
    let frame = little_clip_core::media_backend::decode_video_frame(&result, &settings).ok()??;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(4 * 1024 * 1024);
    limits.max_image_width = Some(160);
    limits.max_image_height = Some(160);
    little_clip_core::ImageThumbnail::generate_with_limits(
        frame.png,
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
    let probe = little_clip_core::media_backend::probe_request(
        little_clip_core::media_backend::VIDEO_FORMATS,
    )
    .arg_content_file(path)
    .map_err(|e| e.to_string())?
    .timeout(Duration::from_secs(3))
    .max_output_bytes(64 * 1024)
    .run()
    .map_err(|e| format!("Cannot inspect video: {e}"))?;
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
    if black
        && prepared.duration_ms > 0
        && let Some(middle) = media_frame(path, prepared.stream, prepared.duration_ms / 2)
    {
        image = middle;
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
    let duration_ms = timing::Track::from_stream(value, stream)
        .map_or(0, |track| ((track.start + track.duration) * 1000.0) as u64);
    Ok((index, width as u32, height as u32, duration_ms))
}

pub(in crate::file_manager) use little_clip_core::media_stream::selected_video_stream;

pub(super) use little_clip_core::item::raster_preview_limits;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_worker_distinguishes_real_video_from_attached_artwork() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/video/with-cover.mp4");
        let video = media_frame(&path, 0, 0).unwrap();
        let cover = media_frame(&path, 1, 0).unwrap();
        assert_eq!((video.width, video.height), (160, 90));
        assert_eq!((cover.width, cover.height), (160, 160));
        assert!(
            video
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[0] > 200 && pixel[2] < 20)
        );
        assert!(
            cover
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[2] > 200 && pixel[0] < 20)
        );
    }

    #[test]
    fn video_preview_and_extraction_share_source_relative_duration() {
        let mut value = serde_json::json!({
            "streams": [{"index":0,"codec_type":"video","width":80,"height":40,
                "start_time":"5","tags":{"DURATION":"00:00:06.000000000"}}],
            "format":{"start_time":"5","duration":"6"}
        });
        assert_eq!(stream_metadata(&value).unwrap().3, 1000);
        value["streams"][0]["duration"] = serde_json::json!("0.5");
        assert_eq!(stream_metadata(&value).unwrap().3, 500);
        value["streams"][0]["duration"] = serde_json::json!("NaN");
        value["streams"][0]["tags"]["DURATION"] = serde_json::json!("99:99:99");
        value["format"]["duration"] = serde_json::json!("NaN");
        assert_eq!(stream_metadata(&value).unwrap().3, 0);
    }
    #[test]
    fn selected_stream_skips_cover_art_and_enforces_the_played_pixel_budget() {
        let mut metadata = serde_json::json!({
            "streams": [
                {"index":0,"width":160, "height":160, "disposition":{"attached_pic":1}},
                {"index":1,"width":1920, "height":1080, "disposition":{"attached_pic":0}}
            ], "format":{"duration":"3.5"}
        });
        assert_eq!(stream_metadata(&metadata).unwrap(), (1, 1920, 1080, 3500));
        metadata["streams"][1]["width"] = serde_json::json!(8192);
        assert!(stream_metadata(&metadata).is_err());
        assert!(stream_metadata(&serde_json::json!({"streams":[]})).is_err());
    }
}
