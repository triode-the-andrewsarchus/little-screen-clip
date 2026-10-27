//! Read-only indexed/Timelens generation and frame extraction on workers.
use super::super::{filesystem, media};
use crate::file_source::FileSource;
use little_clip_core::{
    ImageThumbnail,
    process::{CommandError, CommandOutput, CommandRequest},
    staging::ArtifactStore,
};
use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

pub(super) struct Rendered {
    pub metadata: Arc<media::VideoMetadata>,
    pub timeline: Result<ImageThumbnail, String>,
}

/// Verify intermediate snapshots before they enter the UI's progress channel.
pub(super) struct Progress<'a> {
    pub sender: mpsc::SyncSender<ImageThumbnail>,
    pub context: &'a super::super::search::PreviewContext,
    pub path: &'a Path,
    pub cancel: &'a AtomicBool,
}
impl Progress<'_> {
    fn try_send(&self, image: ImageThumbnail) -> Result<(), String> {
        let image = self.context.read(self.path, self.cancel, || Ok(image))?;
        self.sender.try_send(image).map_err(|e| e.to_string())
    }
}

pub(super) fn load(
    path: &Path,
    revision: u64,
    width: u32,
    peers: usize,
    token: &AtomicBool,
    inspected: mpsc::SyncSender<Arc<media::VideoMetadata>>,
    progress: Progress<'_>,
) -> Result<Rendered, String> {
    filesystem::check_revision(path, revision)?;
    let metadata = Arc::new(
        progress
            .context
            .read(path, token, || media::inspect_video(path))?,
    );
    if metadata.duration_ms == 0 {
        return Err("Video duration is unavailable".into());
    }
    let _ = inspected.try_send(metadata.clone());
    let policy = super::dispatch::Policy::for_video(&metadata, peers);
    let timeline = timeline(path, &metadata, width, policy, token, &progress);
    metadata.source.check()?;
    filesystem::check_revision(path, revision)?;
    progress.context.read(path, token, || Ok(()))?;
    Ok(Rendered { metadata, timeline })
}

pub(super) fn helper() -> PathBuf {
    video_helper("LITTLE_CLIP_TIMELENS", "timelens").unwrap_or_else(|| "timelens".into())
}

fn video_helper(environment: &str, name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(environment) {
        return Some(path.into());
    }
    let bundled =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/tools/{name}/{name}"));
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join(name)))
        .filter(|p| p.is_file())
        .or_else(|| bundled.is_file().then_some(bundled))
}

fn cache_path(source: &FileSource, stream: usize, width: u32) -> Option<PathBuf> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    format!("{source:?}").hash(&mut hash);
    stream.hash(&mut hash);
    width.hash(&mut hash);
    // Sparse experimental previews must not replace full-frame cached strips.
    let namespace = match std::env::var("VIDEO_STRIP_GPU_SLICES").as_deref() {
        Ok("5") => "video-timelines-five-slices-v1",
        Ok("1") => "video-timelines-center-column-v1",
        _ => "video-timelines-v3",
    };
    Some(
        ArtifactStore::new()
            .ok()?
            .base_dir()
            .join(namespace)
            .join(format!("{:016x}.jpg", hash.finish())),
    )
}
fn decode(bytes: &[u8], width: u32, height: u32) -> Result<ImageThumbnail, String> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(16 * 1024 * 1024);
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    ImageThumbnail::generate_with_limits(bytes, None, width, height, limits)
        .ok_or_else(|| "Video preview pixels are unavailable".into())
}
fn timeline(
    path: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    policy: super::dispatch::Policy,
    token: &AtomicBool,
    progress: &Progress<'_>,
) -> Result<ImageThumbnail, String> {
    let cached = cache_path(&metadata.source, metadata.stream, width);
    if let Some(image) = cached
        .as_ref()
        .and_then(|p| crate::file_source::read_file_bytes(p, 4 * 1024 * 1024, true, false).ok())
        .and_then(|bytes| decode(&bytes, width, 64).ok())
    {
        return Ok(image);
    }
    let temporary = tempfile::tempdir().map_err(|e| e.to_string())?;
    let output = temporary.path().join("timeline.jpg");
    generate(path, metadata, width, &output, policy, token, progress)?;

    metadata.source.check()?;
    let bytes = crate::file_source::read_file_bytes(&output, 4 * 1024 * 1024, true, false)
        .map_err(|e| e.to_string())?;
    let image = decode(&bytes, width, 64)?;
    if let Some(cache) = cached {
        if let Some(parent) = cache.parent() {
            if std::fs::create_dir_all(parent).is_ok() {
                if let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) {
                    use std::io::Write;
                    if file.write_all(&bytes).is_ok() {
                        let _ = file.persist(cache);
                    }
                }
            }
        }
    }
    Ok(image)
}

fn generate(
    path: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    output: &Path,
    policy: super::dispatch::Policy,
    token: &AtomicBool,
    progress: &Progress<'_>,
) -> Result<(), String> {
    let mut command = CommandRequest::new(helper().into_os_string())
        .arg_content_file(path)
        .map_err(|error| error.to_string())?
        .arg("--fast")
        .arg("--timeline")
        .arg(output.as_os_str())
        .args([
            "--width",
            &width.to_string(),
            "--height",
            "64",
            "--video-stream",
            &metadata.stream.to_string(),
            "--duration-ms",
            &metadata.duration_ms.to_string(),
            "--source-width",
            &metadata.dimensions.0.to_string(),
            "--source-height",
            &metadata.dimensions.1.to_string(),
            "--codec",
            &metadata.codec,
        ]);
    if metadata.codec == "hevc" {
        command = command.arg("--gpu");
    }
    // AV1 keyframe streaming avoids thousands of decoder flushes. Real Shows
    // measurements favored this path over per-column seeks by more than 10x.
    if metadata.codec == "av1" && metadata.duration_ms > 600_000 {
        let filter = format!(
            "scale=1:64:flags=area,tpad=stop_mode=clone:stop_duration={},fps={}000/{},tile={}x1",
            metadata.duration_ms as f64 / 1000.0,
            width,
            metadata.duration_ms,
            width,
        );
        command = CommandRequest::new("ffmpeg")
            .args(["-v", "error", "-skip_frame", "nokey", "-threads", "2", "-i"])
            .arg_content_file(path)
            .map_err(|error| error.to_string())?
            .args([
                "-map",
                &format!("0:v:{}", metadata.stream),
                "-an",
                "-sn",
                "-dn",
                "-vf",
                &filter,
                "-frames:v",
                "1",
                "-q:v",
                "2",
                "-y",
            ])
            .arg(output.as_os_str());
    }
    let generated = generate_indexed(path, metadata, width, output, policy, token, progress);
    if !generated {
        if token.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Video preview cancelled".into());
        }
        let preview = output.with_extension("progress.ppm");
        let snapshots = metadata.codec != "av1" || metadata.duration_ms <= 600_000;
        let result = run_progressive(
            command.timeout(Duration::from_secs(240)),
            &preview,
            metadata,
            width,
            token,
            progress,
            snapshots,
        )
        .map_err(|e| format!("Video preview unavailable: {e}. Build it with launch.sh build."))?;
        if !result.status.success() {
            return Err(format!(
                "Video decoder failed: {}",
                result.stderr_lossy().trim()
            ));
        }
    }
    Ok(())
}

fn generate_indexed(
    path: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    output: &Path,
    policy: super::dispatch::Policy,
    token: &AtomicBool,
    progress: &Progress<'_>,
) -> bool {
    if metadata.duration_ms <= 600_000 {
        return false;
    }
    let Some(helper) = video_helper("LITTLE_CLIP_VIDEO_STRIP", "video-strip") else {
        return false;
    };
    let preview = output.with_extension("progress.ppm");
    let prefer_gpu = policy.prefer_gpu;
    let timeout =
        if u64::from(metadata.dimensions.0) * u64::from(metadata.dimensions.1) >= 4_000_000 {
            60
        } else {
            30
        };
    for gpu in [true, false].into_iter().filter(|gpu| !gpu || prefer_gpu) {
        if token.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        let Ok(command) = CommandRequest::new(helper.as_os_str()).arg_content_file(path) else {
            return false;
        };
        let mut command = command.arg(output.as_os_str()).args([
            &width.to_string(),
            &metadata.stream.to_string(),
            &metadata.duration_ms.to_string(),
        ]);
        command = command.args(["--workers", &policy.workers(gpu).to_string()]);
        if std::env::var("LITTLE_CLIP_VIDEO_IO").is_ok_and(|mode| mode == "uring") {
            command = command.arg("--io-uring");
        }
        if gpu {
            command = command.arg("--gpu");
        }
        if run_progressive(
            command.timeout(Duration::from_secs(timeout)),
            &preview,
            metadata,
            width,
            token,
            progress,
            true,
        )
        .is_ok_and(|result| result.status.success())
        {
            return true;
        }
    }
    false
}

fn run_progressive(
    mut command: CommandRequest,
    preview: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    token: &AtomicBool,
    progress: &Progress<'_>,
    snapshots: bool,
) -> Result<CommandOutput, CommandError> {
    // Retries start a fresh snapshot sequence. The AV1 FFmpeg fallback has no snapshots.
    let _ = std::fs::remove_file(preview);
    if snapshots {
        command = command.arg("--progressive").arg(preview.as_os_str());
    }
    let mut modified = None;
    command
        .max_output_bytes(16 * 1024)
        .run_cancellable_with_progress(token, || {
            poll_progress(preview, metadata, width, &mut modified, progress);
        })
}

fn poll_progress(
    path: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    previous: &mut Option<std::time::SystemTime>,
    progress: &Progress<'_>,
) {
    let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) else {
        return;
    };
    if *previous == Some(modified) || metadata.source.check().is_err() {
        return;
    }
    let image = crate::file_source::read_file_bytes(path, 4 * 1024 * 1024, true, false)
        .map_err(|e| e.to_string())
        .and_then(|bytes| decode(&bytes, width, 64));
    if let Ok(image) = image {
        // A full channel coalesces updates. Retry this snapshot after the UI drains it.
        if progress.try_send(image).is_ok() {
            *previous = Some(modified);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossless_indexed_strip_uses_the_bounded_shared_image_reader() {
        let mut ppm = b"P6\n3200 64\n255\n".to_vec();
        ppm.extend(std::iter::repeat_n([12, 64, 192], 3200 * 64).flatten());
        let image = decode(&ppm, 3200, 64).unwrap();
        assert_eq!((image.width, image.height), (3200, 64));
        assert_eq!(&image.premultiplied_rgba[..4], &[12, 64, 192, 255]);
        assert!(decode(&ppm, 1600, 64).is_err());
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn progress_cannot_publish_a_frame_outside_its_result_context() {
        let context = super::super::super::search::PreviewContext::empty_results();
        let cancel = AtomicBool::new(false);
        let (sender, frames) = mpsc::sync_channel(1);
        let mut progress = Progress {
            sender,
            context: &context,
            path: Path::new("/unpublished"),
            cancel: &cancel,
        };
        let image = || ImageThumbnail {
            width: 1,
            height: 1,
            premultiplied_rgba: vec![0; 4],
        };
        assert!(progress.try_send(image()).is_err());
        assert!(frames.try_recv().is_err());
        let ordinary = super::super::super::search::PreviewContext::default();
        progress.context = &ordinary;
        progress.try_send(image()).unwrap();
        assert_eq!(frames.try_recv().unwrap().width, 1);
    }
}
