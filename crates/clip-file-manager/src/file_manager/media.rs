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
    pub genre: String,
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
    // Native block pixels are owned by the map texture once uploaded.
    pub minecraft_terrain: Option<ImageThumbnail>,
    pub rendered_diagram: bool,
    pub diagram_vector: Option<std::sync::Arc<clip_ui::vectors::svg::Scene>>,
    pub markdown: Option<std::sync::Arc<clip_studios::text_preview::MarkdownExcerptData>>,
    pub spectrum: Option<ImageThumbnail>,
    pub waveform: Vec<f32>,
    pub waveform_colors: Vec<[u8; 3]>,
    pub icon: Icon,
    pub music: Option<MusicMetadata>,
    pub photo_rating: u8,
    pub photo_metadata: Option<little_clip_ops::metadata::ImageMetadataSummary>,
    pub photo: Option<std::sync::Arc<super::image_strip::Analysis>>,
    pub animated_frames: Option<std::sync::Arc<AnimatedFrames>>,
    pub midi_document: Option<std::sync::Arc<super::midi_strip::Document>>,
    pub handwritten_info: Option<std::sync::Arc<little_clip_core::notebook::Info>>,
    pub pdf_info: Option<std::sync::Arc<little_clip_core::pdf::PdfInfo>>,
    pub system: Option<std::sync::Arc<super::system::Document>>,
    pub subtitles: Option<std::sync::Arc<super::subtitle_strip::Summary>>,
    pub notebook: Option<std::sync::Arc<super::notebook::Summary>>,
    pub multisample: Option<std::sync::Arc<little_clip_core::multisample::Document>>,
    pub embroidery: Option<little_clip_core::embroidery::TimelineLayout>,
    pub scope: Option<std::sync::Arc<super::scope_view::strip::Preview>>,
    pub touchstone: Option<std::sync::Arc<super::touchstone_view::strip::Preview>>,
    pub toolpath: Option<std::sync::Arc<little_clip_core::gcode::Document>>,
    pub geography: Option<std::sync::Arc<super::geography::Preview>>,
    pub solar_trajectory: bool,
    pub spreadsheet: Option<little_clip_core::spreadsheet::SpreadsheetSheet>,
    pub database: Option<std::sync::Arc<super::database::Summary>>,
    pub calendar: Option<std::sync::Arc<super::calendar::Summary>>,
    pub mail: Option<std::sync::Arc<super::mail::Summary>>,
    pub book: Option<std::sync::Arc<super::ebook::Summary>>,
    pub font: Option<std::sync::Arc<super::font_strip::Document>>,
    pub binary: Option<std::sync::Arc<super::binary_strip::Document>>,
    pub document: Option<std::sync::Arc<super::text_strip::Document>>,
    pub vcv: Option<std::sync::Arc<super::vcv_strip::Document>>,
    pub materials:
        Option<std::sync::Arc<Vec<std::sync::Arc<little_clip_core::model3d::materials::Material>>>>,
    pub brush: Option<std::sync::Arc<little_clip_core::brushes::BrushLibrary>>,
}

impl PreviewData {
    pub(in crate::file_manager) fn new(title: String, detail: String, icon: Icon) -> Self {
        Self {
            title,
            detail,
            icon,
            text: String::new(),
            image: None,
            minecraft_terrain: None,
            rendered_diagram: false,
            diagram_vector: None,
            markdown: None,
            spectrum: None,
            waveform: Vec::new(),
            waveform_colors: Vec::new(),
            music: None,
            document: None,
            vcv: None,
            binary: None,
            font: None,
            book: None,
            mail: None,
            calendar: None,
            spreadsheet: None,
            database: None,
            geography: None,
            solar_trajectory: false,
            scope: None,
            touchstone: None,
            toolpath: None,
            embroidery: None,
            multisample: None,
            notebook: None,
            subtitles: None,
            system: None,
            pdf_info: None,
            handwritten_info: None,
            midi_document: None,
            photo_rating: 0,
            photo_metadata: None,
            photo: None,
            animated_frames: None,
            materials: None,
            brush: None,
        }
    }
}

pub(in crate::file_manager) struct AnimatedFrame {
    pub image: ImageThumbnail,
    pub delay: Duration,
}

impl AnimatedFrame {
    fn from_gif_frame(frame: image::Frame, edge: u32) -> Self {
        let (numer, denom) = frame.delay().numer_denom_ms();
        let raw_delay = numer.checked_div(denom).unwrap_or(100);
        // Browsers normalize < 20ms GIF frame delays to 100ms to avoid runaway CPU usage.
        let delay_ms = if raw_delay < 20 {
            100
        } else {
            raw_delay.min(10_000)
        };
        let delay = Duration::from_millis(u64::from(delay_ms));

        let buffer = frame.into_buffer();
        let rgba = if buffer.width().max(buffer.height()) > edge && edge > 0 {
            image::DynamicImage::ImageRgba8(buffer)
                .thumbnail(edge, edge)
                .to_rgba8()
        } else {
            buffer
        };
        Self {
            image: ImageThumbnail::from_rgba(rgba),
            delay,
        }
    }
}

pub(in crate::file_manager) struct AnimatedFrames {
    pub frames: Vec<AnimatedFrame>,
    pub total_duration: Duration,
    cycles: Option<u32>,
}

impl AnimatedFrames {
    /// Returns `(frame_index, remaining_delay)` for the elapsed playback time.
    ///
    /// Finite playback holds the last frame with a zero delay after its final pass.
    pub fn frame_at(&self, elapsed: Duration) -> (usize, Duration) {
        if self.frames.len() <= 1 {
            return (0, Duration::ZERO);
        }
        let total_ms = self.total_duration.as_millis();
        if total_ms == 0 {
            return (0, Duration::ZERO);
        }
        let elapsed_ms = elapsed.as_millis();
        if self
            .cycles
            .is_some_and(|count| elapsed_ms / total_ms >= u128::from(count))
        {
            return (self.frames.len() - 1, Duration::ZERO);
        }
        let pos_ms = (elapsed_ms % total_ms) as u64;
        let mut accum = 0u64;
        for (idx, frame) in self.frames.iter().enumerate() {
            let delay_ms = frame.delay.as_millis() as u64;
            accum += delay_ms;
            if pos_ms < accum {
                let remaining = (accum - pos_ms).max(16);
                return (idx, Duration::from_millis(remaining));
            }
        }
        (self.frames.len() - 1, Duration::from_millis(16))
    }
}

pub(in crate::file_manager) fn is_gif_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gif"))
}

const MAX_ANIMATION_FRAMES: usize = 240;
const MAX_ANIMATION_BYTES: usize = 64 * 1024 * 1024;

/// Decodes a multi-frame GIF into scaled RGBA frames for hover playback.
///
/// Returns `None` for non-GIFs, single-frame still GIFs, cancelled jobs, or decode errors.
pub(in crate::file_manager) fn decode_animated_gif(
    bytes: &[u8],
    edge: u32,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Option<((u32, u32), AnimatedFrames)> {
    use image::{AnimationDecoder, ImageDecoder, codecs::gif::GifDecoder};
    use std::sync::atomic::Ordering;

    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let mut decoder = GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    decoder.set_limits(raster_preview_limits()).ok()?;
    let (orig_w, orig_h) = decoder.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    // Read the GIF repeat extension directly to distinguish an absent extension
    // (one pass) from an explicit infinite loop.
    let repeat = gif::DecodeOptions::new().read_info(bytes).ok()?.repeat();
    let cycles = match repeat {
        gif::Repeat::Infinite => None,
        gif::Repeat::Finite(repeats) => Some(1 + u32::from(repeats)),
    };

    let mut frames = Vec::new();
    let mut total_duration = Duration::ZERO;
    let mut total_bytes = 0usize;

    for frame_result in decoder.into_frames() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(frame) = frame_result else {
            break;
        };
        let frame = AnimatedFrame::from_gif_frame(frame, edge);
        total_bytes = total_bytes.saturating_add(frame.image.premultiplied_rgba.len());
        total_duration += frame.delay;
        frames.push(frame);

        if frames.len() >= MAX_ANIMATION_FRAMES || total_bytes >= MAX_ANIMATION_BYTES {
            break;
        }
    }

    if frames.len() <= 1 {
        return None;
    }
    Some((
        (orig_w, orig_h),
        AnimatedFrames {
            frames,
            total_duration,
            cycles,
        },
    ))
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
    pub(super) audio_tracks: u32,
}

pub(super) fn inspect_video(path: &std::path::Path) -> Result<VideoMetadata, String> {
    let metadata = inspect_video_header(path, &std::sync::atomic::AtomicBool::new(false))?;
    validate_video_dimensions(metadata.dimensions)?;
    Ok(metadata)
}

// Header inspection is independent of the playback decoder's pixel budget.
pub(super) fn inspect_video_header(
    path: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<VideoMetadata, String> {
    let (input, source) = crate::file_source::ReadCheck::default().open(path, None, cancel)?;
    let size_bytes = input.metadata().map_err(|error| error.to_string())?.len();
    let probe = little_clip_core::media_backend::probe_request(
        little_clip_core::media_backend::VIDEO_FORMATS,
    )
    .arg_owned_content_file(input)
    .map_err(|e| e.to_string())?
    .timeout(Duration::from_secs(3))
    .max_output_bytes(64 * 1024)
    .run_cancellable(cancel)
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
        size_bytes,
        codec: video["codec_name"].as_str().unwrap_or_default().into(),
        frame_interval_ms: video_details::frame_rate(video)
            .map(|fps| (1000.0 / fps).ceil().clamp(1.0, 60_000.0) as u64)
            .unwrap_or(40),
        description,
        audio_description,
        audio_tracks: value["streams"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|stream| stream["codec_type"].as_str() == Some("audio"))
            .count() as u32,
    })
}

pub(in crate::file_manager) fn video_preview(
    path: &std::path::Path,
) -> Option<little_clip_core::ImageThumbnail> {
    let prepared = inspect_video(path).ok()?;
    let mut image = media_frame(path, prepared.stream, 0)?;
    let black = image
        .premultiplied_rgba
        .as_chunks::<4>()
        .0
        .iter()
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
    if width == 0 || height == 0 || width > u64::from(u32::MAX) || height > u64::from(u32::MAX) {
        return Err("Video dimensions are invalid".into());
    }
    let duration_ms = timing::Track::from_stream(value, stream)
        .map_or(0, |track| ((track.start + track.duration) * 1000.0) as u64);
    Ok((index, width as u32, height as u32, duration_ms))
}

fn validate_video_dimensions((width, height): (u32, u32)) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 4096 * 2160
    {
        return Err("Video dimensions are invalid or exceed the 4096×2160 pixel budget; use Open externally.".into());
    }
    Ok(())
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
        let (_, width, height, _) = stream_metadata(&metadata).unwrap();
        assert_eq!((width, height), (8192, 1080));
        assert!(validate_video_dimensions((width, height)).is_err());
        assert!(stream_metadata(&serde_json::json!({"streams":[]})).is_err());
    }

    use image::codecs::gif::Repeat;

    fn encode_gif(frame_colors: &[[u8; 4]], delay_ms: u32, repeat: Repeat) -> Vec<u8> {
        use image::codecs::gif::GifEncoder;
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(repeat).unwrap();
            let frames = frame_colors.iter().map(|&rgba| {
                let img = image::RgbaImage::from_pixel(4, 4, image::Rgba(rgba));
                image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(delay_ms, 1))
            });
            encoder.encode_frames(frames).unwrap();
        }
        bytes
    }

    #[test]
    fn decode_animated_gif_distinguishes_still_and_animated_gifs() {
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let still = encode_gif(&[[255, 0, 0, 255]], 100, Repeat::Infinite);
        assert!(decode_animated_gif(&still, 128, &cancelled).is_none());

        let animated = encode_gif(
            &[[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]],
            100,
            Repeat::Infinite,
        );
        let ((w, h), frames) = decode_animated_gif(&animated, 128, &cancelled).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(frames.frames.len(), 3);
        assert_eq!(frames.frame_at(Duration::from_millis(0)).0, 0);
        assert_eq!(frames.frame_at(Duration::from_millis(120)).0, 1);
        assert_eq!(frames.frame_at(Duration::from_millis(250)).0, 2);
        assert_eq!(frames.frame_at(Duration::from_millis(320)).0, 0);
    }

    #[test]
    fn animated_gif_stops_after_its_configured_repeats() {
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let colors = [[255, 0, 0, 255], [0, 255, 0, 255]];
        for repeats in [0, 1, 2] {
            let bytes = encode_gif(&colors, 100, Repeat::Finite(repeats));
            let (_, frames) = decode_animated_gif(&bytes, 128, &cancelled).unwrap();
            let last_start = Duration::from_millis(200 * u64::from(repeats));
            assert_eq!(frames.frame_at(last_start), (0, Duration::from_millis(100)));
            assert_eq!(
                frames.frame_at(last_start + Duration::from_millis(200)),
                (1, Duration::ZERO)
            );
        }
    }
}
