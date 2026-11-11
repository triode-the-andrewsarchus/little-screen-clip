//! Requests to the separately linked FFmpeg library worker.

use crate::process::CommandRequest;
pub use clip_media_protocol::playback;
pub use clip_media_protocol::{
    AUDIO_FORMATS, AudioFilter, AudioInput, AudioRender, EncodedVideoFrame, VIDEO_FORMATS,
    VideoFrame, VideoPreview, VideoStream, VideoTimeline,
};
use std::path::PathBuf;

/// Builds a metadata request. The caller supplies a pinned content input and
/// retains its existing timeout, cancellation and output-size policy.
pub fn probe_request(formats: &str) -> CommandRequest {
    CommandRequest::new(worker_path())
        .args(["probe", formats])
        .die_with_parent()
        .max_output_bytes(64 * 1024)
}

/// Builds a bounded packed-float audio decoding request for native format bridges.
///
/// The caller supplies a pinned input. Output is little-endian interleaved
/// `f32` PCM at the source rate and channel count, capped at 256 MiB.
pub fn decode_audio_request(formats: &str) -> CommandRequest {
    CommandRequest::new(worker_path())
        .args(["decode-audio", formats])
        .die_with_parent()
        .max_output_bytes(256 * 1024 * 1024)
}

/// Builds a library audio render request without opening input or output paths.
///
/// Append owned content handles in input order and direct stdout to an owned
/// seekable staging file. Accepted-write callers use `build_command` and retain
/// their existing cancellation/reaping lifetime, without a preview timeout.
///
/// # Errors
/// Returns an error if settings cannot be serialized or exceed the wire budget.
pub fn render_audio_request(settings: &AudioRender) -> Result<CommandRequest, String> {
    settings_request("render-audio", settings)
}

/// Builds a render request retaining an explicit staging descriptor.
///
/// This variant leaves stdout available for bounded command supervision.
/// Append owned input descriptors after this request's output argument.
///
/// # Errors
/// Returns settings serialization or staging-handle inheritance errors.
pub fn render_audio_to_request(
    settings: &AudioRender,
    output: &std::fs::File,
) -> Result<CommandRequest, String> {
    let (request, descriptor) = render_audio_request(settings)?
        .inherit_handle(output)
        .map_err(|error| error.to_string())?;
    Ok(request.arg(format!("--output=/proc/self/fd/{descriptor}")))
}

/// Builds a bounded still-frame request using a pinned content input.
///
/// # Errors
/// Returns a serialization or request-budget error.
pub fn video_frame_request(settings: &VideoFrame) -> Result<CommandRequest, String> {
    Ok(settings_request("video-frame", settings)?.max_output_bytes(32 * 1024 * 1024 + 1028))
}

/// Builds a sampled MJPEG preview request retaining one source decoder.
///
/// # Errors
/// Returns settings serialization or request-budget errors.
pub fn video_preview_request(settings: &VideoPreview) -> Result<CommandRequest, String> {
    Ok(settings_request("video-preview", settings)?.max_output_bytes(32 * 1024 * 1024))
}

/// Builds a sequential keyframe-column timeline fallback request.
///
/// # Errors
/// Returns settings serialization or request-budget errors.
pub fn video_timeline_request(settings: &VideoTimeline) -> Result<CommandRequest, String> {
    Ok(settings_request("video-timeline", settings)?.max_output_bytes(4 * 1024 * 1024))
}

/// Checks process completion and parses the actual decoded frame record.
///
/// # Errors
/// Returns child-failure, truncation or malformed-response errors.
pub fn decode_video_frame<'a>(
    output: &'a crate::process::CommandOutput,
    settings: &VideoFrame,
) -> Result<Option<EncodedVideoFrame<'a>>, String> {
    if !output.status.success() || output.stdout_truncated {
        return Err("Video frame decode failed or exceeded its output budget".into());
    }
    clip_media_protocol::parse_video_frame(&output.stdout, settings.width, settings.height)
}

/// Builds an owned streaming playback request. The caller supplies a pinned input,
/// pumps the bounded protocol off the UI thread, and kills/reaps the child on stop.
///
/// # Errors
/// Returns serialization or settings-budget errors.
pub fn playback_request(settings: &playback::Settings) -> Result<CommandRequest, String> {
    settings_request("playback", settings)
}

fn settings_request(
    mode: &str,
    settings: &impl serde::Serialize,
) -> Result<CommandRequest, String> {
    let settings = serde_json::to_string(settings).map_err(|error| error.to_string())?;
    if settings.len() > clip_media_protocol::MAX_SETTINGS_BYTES {
        return Err("Media worker settings exceed their budget".into());
    }
    Ok(CommandRequest::new(worker_path())
        .args([mode, &settings])
        .die_with_parent())
}

fn worker_path() -> PathBuf {
    if let Some(path) = std::env::var_os("LITTLE_CLIP_MEDIA_WORKER") {
        return path.into();
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let sibling = parent.join("clip-media-worker");
        if sibling.is_file() {
            return sibling;
        }
        let companion = parent.join("media-helper/clip-media-worker");
        if companion.is_file() {
            return companion;
        }
        // Cargo test executables live one directory below application binaries.
        if parent.file_name().is_some_and(|name| name == "deps")
            && let Some(profile) = parent.parent()
        {
            let sibling = profile.join("clip-media-worker");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    "clip-media-worker".into()
}
