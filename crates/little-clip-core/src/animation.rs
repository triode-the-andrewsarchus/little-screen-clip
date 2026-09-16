//! Animation and video preview frame extraction, JPEG thumbnail sequences, and timing calculations.
//!
//! Provides decoders for animated GIF, animated WebP, and common video formats (via `ffmpeg`/`ffprobe`),
//! compiling them into compact MJPEG frame sequences for display playback.

use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

/// Lightweight animation preview containing downsampled JPEG frames and frame delays.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnimationThumbnail {
    /// Rendered thumbnail width in pixels.
    pub width: u32,
    /// Rendered thumbnail height in pixels.
    pub height: u32,
    /// Number of frames extracted in the loop.
    pub frame_count: usize,
    /// Nominal frames per second of playback.
    pub fps: f32,
    /// JPEG-encoded frames (each ~2 - 6 KB), keeping memory and IPC tiny.
    pub frames_jpeg: Vec<Vec<u8>>,
    /// Delay per frame in milliseconds.
    pub frame_delays_ms: Vec<u32>,
    /// Total duration of animation loop in milliseconds.
    pub loop_duration_ms: u64,
}

impl AnimationThumbnail {
    /// Computes the exact frame index corresponding to a millisecond offset in the animation
    pub fn get_frame_index_at_time(&self, elapsed_ms: u64) -> usize {
        if self.frame_count == 0 {
            return 0;
        }
        if self.loop_duration_ms == 0 {
            return 0;
        }
        let pos_in_loop = elapsed_ms % self.loop_duration_ms;
        let mut accum = 0u64;
        for (i, &delay) in self.frame_delays_ms.iter().enumerate() {
            accum += delay as u64;
            if pos_in_loop < accum {
                return i;
            }
        }
        self.frame_count.saturating_sub(1)
    }

    /// Helper to get current frame bytes given a timestamp
    pub fn get_current_frame_bytes(&self, elapsed_ms: u64) -> Option<&[u8]> {
        let idx = self.get_frame_index_at_time(elapsed_ms);
        self.frames_jpeg.get(idx).map(|f| f.as_slice())
    }
}

/// Returns true if the file path has a common video extension
pub fn is_video_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    matches!(
        ext.as_deref(),
        Some("mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "flv" | "wmv" | "ogv" | "ts")
    )
}

/// Returns true if the file extension could be an animated image format
pub fn is_animated_image_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    matches!(ext.as_deref(), Some("gif" | "webp" | "apng"))
}

/// Reads video width, height, and duration (in milliseconds) via ffprobe
pub fn read_video_metadata(path: &Path) -> Option<(u32, u32, u64)> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1",
            path.to_str()?,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut width = 0u32;
    let mut height = 0u32;
    let mut duration_ms = 0u64;

    for line in text.lines() {
        let line = line.trim();
        if let Some(w) = line.strip_prefix("width=") {
            width = w.parse().unwrap_or(0);
        } else if let Some(h) = line.strip_prefix("height=") {
            height = h.parse().unwrap_or(0);
        } else if let Some(d) = line.strip_prefix("duration=") {
            if let Ok(sec) = d.parse::<f64>() {
                duration_ms = (sec * 1000.0) as u64;
            }
        }
    }

    if width > 0 && height > 0 {
        Some((width, height, duration_ms))
    } else {
        None
    }
}

/// Decodes animated GIF or animated WebP from file into an AnimationThumbnail
pub fn decode_animated_gif_or_webp(
    path: &Path,
    max_w: u32,
    max_h: u32,
    max_frames: usize,
) -> Option<AnimationThumbnail> {
    // Try ffmpeg first for fast downsampled extraction without massive RAM allocations on large GIFs
    if let Some(thumb) = extract_animated_frames_via_ffmpeg(path, max_w, max_h, max_frames) {
        return Some(thumb);
    }

    let bytes = std::fs::read(path).ok()?;
    decode_animated_bytes(&bytes, max_w, max_h, max_frames)
}

/// Extracts animated frames from a GIF or WebP file using ffmpeg for high performance
pub fn extract_animated_frames_via_ffmpeg(
    path: &Path,
    max_w: u32,
    max_h: u32,
    target_frames: usize,
) -> Option<AnimationThumbnail> {
    let scale_filter = format!(
        "fps=8,scale={}:{}:force_original_aspect_ratio=decrease",
        max_w, max_h
    );

    let output = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            path.to_str()?,
            "-vf",
            &scale_filter,
            "-frames:v",
            &target_frames.to_string(),
            "-c:v",
            "mjpeg",
            "-q:v",
            "6",
            "-f",
            "image2pipe",
            "-",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let frames_jpeg = extract_mjpeg_frames_from_bytes(&output.stdout);
    if frames_jpeg.is_empty() {
        return None;
    }

    let (tw, th) = if let Ok(reader) =
        image::ImageReader::new(Cursor::new(&frames_jpeg[0])).with_guessed_format()
    {
        reader.into_dimensions().unwrap_or((max_w, max_h))
    } else {
        (max_w, max_h)
    };

    let frame_count = frames_jpeg.len();
    let frame_delay_ms = 125; // 8 fps
    let frame_delays_ms = vec![frame_delay_ms; frame_count];
    let loop_duration_ms = (frame_count as u64 * frame_delay_ms as u64).max(100);

    Some(AnimationThumbnail {
        width: tw,
        height: th,
        frame_count,
        fps: 8.0,
        frames_jpeg,
        frame_delays_ms,
        loop_duration_ms,
    })
}

/// Decodes animated GIF or animated WebP from in-memory bytes into an AnimationThumbnail
pub fn decode_animated_bytes(
    bytes: &[u8],
    max_w: u32,
    max_h: u32,
    max_frames: usize,
) -> Option<AnimationThumbnail> {
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;
    use image::codecs::webp::WebPDecoder;

    // 1. Try GIF decoder
    if let Ok(decoder) = GifDecoder::new(Cursor::new(bytes)) {
        let frames: Vec<_> = decoder
            .into_frames()
            .take(max_frames.saturating_mul(2))
            .filter_map(|r| r.ok())
            .collect();
        if frames.len() > 1 {
            return process_image_frames(frames, max_w, max_h, max_frames);
        }
    }

    // 2. Try WebP decoder
    if let Ok(decoder) = WebPDecoder::new(Cursor::new(bytes)) {
        if decoder.has_animation() {
            let frames: Vec<_> = decoder
                .into_frames()
                .take(max_frames.saturating_mul(2))
                .filter_map(|r| r.ok())
                .collect();
            if frames.len() > 1 {
                return process_image_frames(frames, max_w, max_h, max_frames);
            }
        }
    }

    None
}

fn process_image_frames(
    frames: Vec<image::Frame>,
    max_w: u32,
    max_h: u32,
    max_frames: usize,
) -> Option<AnimationThumbnail> {
    if frames.is_empty() {
        return None;
    }

    let orig_w = frames[0].buffer().width();
    let orig_h = frames[0].buffer().height();
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    let aspect = orig_w as f32 / orig_h as f32;
    let (tw, th) = if aspect >= (max_w as f32 / max_h as f32) {
        (max_w, ((max_w as f32 / aspect).round() as u32).max(1))
    } else {
        (((max_h as f32 * aspect).round() as u32).max(1), max_h)
    };

    // Subsample frames if greater than max_frames to preserve smooth playback without bloat
    let step = (frames.len() as f64 / max_frames as f64).max(1.0);
    let selected_indices: Vec<usize> = if frames.len() <= max_frames {
        (0..frames.len()).collect()
    } else {
        (0..max_frames)
            .map(|i| (i as f64 * step) as usize)
            .filter(|&i| i < frames.len())
            .collect()
    };

    let mut frames_jpeg = Vec::new();
    let mut frame_delays_ms = Vec::new();
    let mut total_dur_ms = 0u64;

    for &idx in &selected_indices {
        let frame = &frames[idx];
        let (numer, denom) = frame.delay().numer_denom_ms();
        let raw_delay = if denom > 0 { numer / denom } else { 100 };
        let delay_ms = raw_delay.clamp(20, 1000);
        let eff_delay = (delay_ms as f64 * step) as u32;

        let img = image::DynamicImage::ImageRgba8(frame.buffer().clone());
        let resized = img.resize_exact(tw, th, image::imageops::FilterType::Triangle);
        let rgb = resized.to_rgb8();

        let mut jpeg_buf = Cursor::new(Vec::new());
        if rgb
            .write_to(&mut jpeg_buf, image::ImageFormat::Jpeg)
            .is_ok()
        {
            frames_jpeg.push(jpeg_buf.into_inner());
            frame_delays_ms.push(eff_delay);
            total_dur_ms += eff_delay as u64;
        }
    }

    if frames_jpeg.is_empty() {
        return None;
    }

    let frame_count = frames_jpeg.len();
    let avg_delay_ms = if frame_count > 0 {
        total_dur_ms as f32 / frame_count as f32
    } else {
        100.0
    };
    let fps = if avg_delay_ms > 0.0 {
        1000.0 / avg_delay_ms
    } else {
        10.0
    };

    Some(AnimationThumbnail {
        width: tw,
        height: th,
        frame_count,
        fps,
        frames_jpeg,
        frame_delays_ms,
        loop_duration_ms: total_dur_ms.max(100),
    })
}

/// Extracts a looping animated preview from a video file using ffmpeg
pub fn extract_video_preview_frames(
    path: &Path,
    max_w: u32,
    max_h: u32,
    target_frames: usize,
    sample_duration_sec: f32,
    sample_from_middle: bool,
) -> Option<AnimationThumbnail> {
    let (_orig_w, _orig_h, duration_ms) = read_video_metadata(path).unwrap_or((640, 360, 0));
    let duration_sec = duration_ms as f64 / 1000.0;

    let start_sec = if sample_from_middle && duration_sec > (sample_duration_sec as f64 * 2.0) {
        (duration_sec - sample_duration_sec as f64) / 2.0
    } else {
        0.0
    };

    let target_fps = ((target_frames as f32 / sample_duration_sec).round() as u32).clamp(5, 20);
    let scale_filter = format!(
        "fps={},scale={}:{}:force_original_aspect_ratio=decrease",
        target_fps, max_w, max_h
    );

    let output = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-ss",
            &format!("{:.2}", start_sec),
            "-t",
            &format!("{:.2}", sample_duration_sec),
            "-i",
            path.to_str()?,
            "-vf",
            &scale_filter,
            "-c:v",
            "mjpeg",
            "-q:v",
            "6",
            "-f",
            "image2pipe",
            "-",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let frames_jpeg = extract_mjpeg_frames_from_bytes(&output.stdout);
    if frames_jpeg.is_empty() {
        return None;
    }

    // Determine actual thumbnail dimensions from first JPEG frame
    let (tw, th) = if let Ok(reader) =
        image::ImageReader::new(Cursor::new(&frames_jpeg[0])).with_guessed_format()
    {
        reader.into_dimensions().unwrap_or((max_w, max_h))
    } else {
        (max_w, max_h)
    };

    let frame_count = frames_jpeg.len();
    let frame_delay_ms = (1000.0 / target_fps as f32).round() as u32;
    let frame_delays_ms = vec![frame_delay_ms; frame_count];
    let loop_duration_ms = (frame_count as u64 * frame_delay_ms as u64).max(100);

    Some(AnimationThumbnail {
        width: tw,
        height: th,
        frame_count,
        fps: target_fps as f32,
        frames_jpeg,
        frame_delays_ms,
        loop_duration_ms,
    })
}

/// Splits a concatenated MJPEG byte stream into individual JPEG frame byte vectors
pub fn extract_mjpeg_frames_from_bytes(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut idx = 0;
    while idx < data.len() {
        // Find JPEG SOI marker 0xFF, 0xD8
        let mut start_opt = None;
        for i in idx..data.len().saturating_sub(1) {
            if data[i] == 0xFF && data[i + 1] == 0xD8 {
                start_opt = Some(i);
                break;
            }
        }

        let start = match start_opt {
            Some(s) => s,
            None => break,
        };

        // Find JPEG EOI marker 0xFF, 0xD9
        let mut end_opt = None;
        for i in (start + 2)..data.len().saturating_sub(1) {
            if data[i] == 0xFF && data[i + 1] == 0xD9 {
                end_opt = Some(i + 2);
                break;
            }
        }

        let end = match end_opt {
            Some(e) => e,
            None => break,
        };

        frames.push(data[start..end].to_vec());
        idx = end;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_animation_thumbnail_timing() {
        let thumb = AnimationThumbnail {
            width: 100,
            height: 100,
            frame_count: 3,
            fps: 10.0,
            frames_jpeg: vec![vec![1], vec![2], vec![3]],
            frame_delays_ms: vec![100, 100, 100],
            loop_duration_ms: 300,
        };

        assert_eq!(thumb.get_frame_index_at_time(0), 0);
        assert_eq!(thumb.get_frame_index_at_time(50), 0);
        assert_eq!(thumb.get_frame_index_at_time(100), 1);
        assert_eq!(thumb.get_frame_index_at_time(199), 1);
        assert_eq!(thumb.get_frame_index_at_time(250), 2);
        assert_eq!(thumb.get_frame_index_at_time(300), 0); // Looped!
        assert_eq!(thumb.get_frame_index_at_time(350), 0);
    }

    #[test]
    fn test_video_detection() {
        assert!(is_video_path(Path::new("movie.mp4")));
        assert!(is_video_path(Path::new("/path/to/clip.mkv")));
        assert!(is_video_path(Path::new("web.webm")));
        assert!(!is_video_path(Path::new("song.mp3")));
        assert!(!is_video_path(Path::new("photo.png")));
    }

    #[test]
    fn test_animated_detection() {
        assert!(is_animated_image_path(Path::new("meme.gif")));
        assert!(is_animated_image_path(Path::new("sticker.webp")));
        assert!(!is_animated_image_path(Path::new("photo.jpg")));
    }

    #[test]
    fn test_mjpeg_splitting() {
        // Construct fake MJPEG stream with 2 frames
        let mut stream = Vec::new();
        stream.extend_from_slice(b"\xff\xd8FRAME1\xff\xd9");
        stream.extend_from_slice(b"\xff\xd8FRAME2\xff\xd9");

        let frames = extract_mjpeg_frames_from_bytes(&stream);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], b"\xff\xd8FRAME1\xff\xd9");
        assert_eq!(frames[1], b"\xff\xd8FRAME2\xff\xd9");
    }

    #[test]
    fn test_decode_animated_gif() {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, RgbaImage};

        let mut gif_bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut gif_bytes);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();

            let frame1 = Frame::from_parts(
                RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(100, 1),
            );
            let frame2 = Frame::from_parts(
                RgbaImage::from_pixel(10, 10, image::Rgba([0, 255, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(100, 1),
            );
            encoder.encode_frame(frame1).unwrap();
            encoder.encode_frame(frame2).unwrap();
        }

        let thumb = decode_animated_bytes(&gif_bytes, 100, 100, 10).expect("Failed to decode GIF");
        assert_eq!(thumb.frame_count, 2);
        assert_eq!(thumb.frames_jpeg.len(), 2);
        assert!(thumb.loop_duration_ms >= 200);
    }
}
