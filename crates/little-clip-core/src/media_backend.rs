//! Requests to the separately linked FFmpeg library worker.

use crate::process::CommandRequest;
use std::path::PathBuf;

/// Common audio containers accepted by native playback and the format bridge.
pub const AUDIO_FORMATS: &str =
    "wav,aiff,flac,mp3,ogg,mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,caf,aac,asf,xwma";

/// Builds a metadata request. The caller supplies a pinned content input and
/// retains its existing timeout, cancellation and output-size policy.
pub fn probe_request(formats: &str) -> CommandRequest {
    CommandRequest::new(worker_path())
        .args(["probe", formats])
        .max_output_bytes(64 * 1024)
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
