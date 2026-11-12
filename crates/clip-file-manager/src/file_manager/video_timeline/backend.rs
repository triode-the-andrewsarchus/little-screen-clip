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

// Retain the private output directory across all decoder attempts. Library
// output installation must not follow a replaced temporary-directory path.
struct TimelineOutput {
    path: PathBuf,
    directory: std::fs::File,
}
impl TimelineOutput {
    fn install(&self, bytes: &[u8]) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        let parent = PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()));
        little_clip_core::kernel_interface::pin_content_directory(&parent)
            .map_err(|error| error.to_string())?;
        let target = parent.join(self.path.file_name().ok_or("Missing timeline filename")?);
        // Discard only an unpublished prior decoder attempt in this private
        // directory. The shared creator stages and installs without overwrite.
        match std::fs::remove_file(target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        little_clip_core::file_source::create_file_in(&self.path, bytes, &self.directory)?;
        Ok(())
    }
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
        Ok("5") => "video-timelines-five-slices-v2",
        Ok("1") => "video-timelines-center-column-v2",
        _ => "video-timelines-v4",
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
    let output = TimelineOutput {
        path: temporary.path().join("timeline.jpg"),
        directory: little_clip_core::kernel_interface::pin_content_directory(temporary.path())
            .map_err(|error| error.to_string())?,
    };
    generate(path, metadata, width, &output, policy, token, progress)?;

    metadata.source.check()?;
    let bytes = crate::file_source::read_file_bytes(&output.path, 4 * 1024 * 1024, true, false)
        .map_err(|e| e.to_string())?;
    let image = decode(&bytes, width, 64)?;
    if let Some(cache) = cached
        && let Some(parent) = cache.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(mut file) = tempfile::NamedTempFile::new_in(parent)
    {
        use std::io::Write;
        if file.write_all(&bytes).is_ok() {
            let _ = file.persist(cache);
        }
    }
    Ok(image)
}

fn generate(
    path: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    output: &TimelineOutput,
    policy: super::dispatch::Policy,
    token: &AtomicBool,
    progress: &Progress<'_>,
) -> Result<(), String> {
    if generate_indexed(path, metadata, width, &output.path, policy, token, progress) {
        return Ok(());
    }
    if token.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Video preview cancelled".into());
    }
    // Preserve the measured sequential keyframe path for long AV1 files;
    // thousands of per-column decoder flushes were substantially slower.
    if metadata.codec == "av1" && metadata.duration_ms > 600_000 {
        return generate_library_timeline(path, metadata, width, output, token);
    }
    let mut command = CommandRequest::new(helper().into_os_string())
        .arg_content_file(path)
        .map_err(|error| error.to_string())?
        .arg("--fast")
        .arg("--timeline")
        .arg(output.path.as_os_str())
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
    let preview = output.path.with_extension("progress.ppm");
    let result = run_progressive(
        command.timeout(Duration::from_secs(240)),
        &preview,
        metadata,
        width,
        token,
        progress,
        true,
    )
    .map_err(|error| {
        format!("Video preview unavailable: {error}. Build it with launch.sh build.")
    })?;
    if !result.status.success() {
        return Err(format!(
            "Video decoder failed: {}",
            result.stderr_lossy().trim()
        ));
    }
    Ok(())
}

fn generate_library_timeline(
    path: &Path,
    metadata: &media::VideoMetadata,
    width: u32,
    output: &TimelineOutput,
    token: &AtomicBool,
) -> Result<(), String> {
    use little_clip_core::media_backend::{VideoFrame, VideoStream, VideoTimeline};
    let mut frame = VideoFrame::new(little_clip_core::media_backend::VIDEO_FORMATS, 1, 64);
    frame.stream = VideoStream::Ordinal(metadata.stream);
    frame.max_source_pixels = 4096 * 2160;
    let settings = VideoTimeline {
        frame,
        columns: width,
        duration_ms: metadata.duration_ms,
    };
    let result = little_clip_core::media_backend::video_timeline_request(&settings)?
        .arg_content_file(path)
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(240))
        .run_cancellable(token)
        .map_err(|error| error.to_string())?;
    if !result.status.success() || result.stdout_truncated || result.stdout.is_empty() {
        return Err(format!(
            "Video timeline decode failed: {}",
            result.stderr_lossy().trim()
        ));
    }
    metadata.source.check()?;
    output.install(&result.stdout)
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
    fn library_timeline_preserves_keyframe_color_holds_and_column_resolution() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/video/timed-colors.gif");
        let metadata = media::inspect_video(&path).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let output = TimelineOutput {
            path: temporary.path().join("timeline.jpg"),
            directory: little_clip_core::kernel_interface::pin_content_directory(temporary.path())
                .unwrap(),
        };
        generate_library_timeline(&path, &metadata, 16, &output, &AtomicBool::new(false)).unwrap();
        let image = image::load_from_memory(&std::fs::read(&output.path).unwrap())
            .unwrap()
            .to_rgb8();
        assert_eq!((image.width(), image.height()), (16, 64));
        for (column, channel) in [(0, 0), (3, 1), (12, 2)] {
            assert!(image.get_pixel(column, 32)[channel] > 200);
        }
    }

    #[test]
    fn library_timeline_installation_retains_the_original_private_directory() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("output");
        std::fs::create_dir(&parent).unwrap();
        let output = TimelineOutput {
            path: parent.join("timeline.jpg"),
            directory: little_clip_core::kernel_interface::pin_content_directory(&parent).unwrap(),
        };
        let moved = root.path().join("moved");
        std::fs::rename(&parent, &moved).unwrap();
        std::os::unix::fs::symlink("/proc", &parent).unwrap();
        output.install(b"generated timeline").unwrap();
        assert_eq!(
            std::fs::read(moved.join("timeline.jpg")).unwrap(),
            b"generated timeline"
        );
    }

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

#[cfg(test)]
mod format_tests {
    use super::*;

    // These exercise the real bounded media path, not just extension recognition.
    #[test]
    fn added_video_formats_decode_and_produce_list_timeline_strips() {
        let temporary = tempfile::tempdir().unwrap();
        let formats: &[(&str, &str, &str, &[&str])] = &[
            ("mpg", "mpeg", "mpeg2video", &[]),
            ("mpeg", "mpeg", "mpeg1video", &[]),
            ("3gp", "3gp", "mpeg4", &[]),
            ("qt", "mov", "mpeg4", &[]),
            ("f4v", "f4v", "libx264", &[]),
            ("asf", "asf", "wmv2", &[]),
            ("vob", "vob", "mpeg2video", &[]),
            ("3g2", "3g2", "mpeg4", &[]),
            ("m2ts", "mpegts", "mpeg2video", &["-mpegts_m2ts_mode", "1"]),
            ("mts", "mpegts", "mpeg2video", &[]),
            ("MXF", "mxf", "mpeg2video", &[]),
        ];
        for &(extension, muxer, codec, options) in formats {
            let path = temporary.path().join(format!("fixture.{extension}"));
            let dimensions = if muxer == "mxf" {
                (1280, 720)
            } else {
                (160, 120)
            };
            let source = format!(
                "testsrc2=size={}x{}:rate=25:duration=1",
                dimensions.0, dimensions.1
            );
            let mut generation = CommandRequest::new("ffmpeg")
                .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", &source]);
            generation = if muxer == "mxf" {
                generation.args([
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=sample_rate=48000:duration=1",
                    "-c:a",
                    "pcm_s16le",
                    "-timecode",
                    "01:02:03:04",
                ])
            } else {
                generation.arg("-an")
            };
            let generated = generation
                .args(["-c:v", codec, "-threads", "1", "-f", muxer])
                .args(options.iter().copied())
                .arg(path.as_os_str())
                .timeout(Duration::from_secs(15))
                .run()
                .unwrap();
            assert!(generated.status.success(), "{}", generated.stderr_lossy());
            if extension == "m2ts" {
                let bytes = std::fs::read(&path).unwrap();
                assert_eq!(bytes.len() % 192, 0);
                assert_eq!(bytes[4], 0x47); // Four-byte M2TS timestamp precedes each TS packet.
            }
            let directory =
                filesystem::read_directory(temporary.path(), &AtomicBool::new(false)).unwrap();
            let entry = directory
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap();
            assert!(entry.is_video(), "{extension}");
            let metadata = media::inspect_video(&path).unwrap();
            assert_eq!(metadata.dimensions, dimensions);
            assert!(metadata.duration_ms > 0);
            if muxer == "mxf" {
                assert_eq!(metadata.duration_ms, 1000);
                assert_eq!(metadata.codec, "mpeg2video");
                assert!(metadata.audio_description.contains("PCM S16LE"));
                assert_eq!(metadata.frame_interval_ms, 40);
            }
            let ctx = egui::Context::default();
            let mut player = crate::file_manager::video_playback::Session::new(
                &metadata,
                500,
                true,
                96,
                ctx.clone(),
                Default::default(),
                Default::default(),
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while player.texture.is_none() {
                player.poll(&ctx);
                assert!(player.error.is_none(), "{extension}: {:?}", player.error);
                assert!(
                    std::time::Instant::now() < deadline,
                    "{extension}: playback timed out"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(player.frame_position_ms.unwrap() < metadata.duration_ms);
            let size = player.texture.as_ref().unwrap().size();
            assert!(size[0] <= 96 && size[1] <= 54);
            drop(player);
            assert!(media::video_preview(&path).is_some());
            let (inspected, _) = mpsc::sync_channel(1);
            let (progress, _) = mpsc::sync_channel(1);
            let rendered = load(
                &path,
                entry.revision,
                48,
                1,
                &AtomicBool::new(false),
                inspected,
                Progress {
                    sender: progress,
                    context: &super::super::super::search::PreviewContext::default(),
                    path: &path,
                    cancel: &AtomicBool::new(false),
                },
            )
            .unwrap();
            let strip = rendered.timeline.unwrap();
            assert_eq!((strip.width, strip.height), (48, 64));
            assert!(
                strip
                    .premultiplied_rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[0] != pixel[1])
            );
            metadata.source.check().unwrap();
        }
    }
}
