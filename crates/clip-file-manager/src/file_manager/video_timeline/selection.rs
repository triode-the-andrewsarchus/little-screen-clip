//! Source-relative timeline selections and guarded, bounded frame reads.
use super::{VideoMetadata, texture};
use crate::file_manager::{preview_copy, preview_job::Task, search::PreviewContext};
use egui::{Context, Id, TextureHandle};
use little_clip_core::{BackgroundJobs, media_backend, process::CommandRequest};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static FRAMES: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 2));
const LARGE_CLIP: u64 = 1_000_000_000;
const AUDIO_RATE: u64 = 192_000;

pub(in crate::file_manager) fn focus_id() -> Id {
    Id::unique("files_video_timeline_focus")
}

#[derive(Clone)]
pub(in crate::file_manager) struct Source {
    pub path: PathBuf,
    pub metadata: Arc<VideoMetadata>,
    pub context: PreviewContext,
}

impl Source {
    fn read<T>(
        &self,
        cancel: &AtomicBool,
        work: impl FnOnce(File) -> Result<T, String>,
    ) -> Result<T, String> {
        preview_copy::read_range(
            &self.context,
            &self.path,
            &self.metadata.source,
            cancel,
            |file| work(file.try_clone().map_err(|error| error.to_string())?),
        )
    }

    fn frame(
        &self,
        seconds: f64,
        size: [u32; 2],
        cancel: &AtomicBool,
        at_or_before: bool,
    ) -> Result<Still, String> {
        let mut settings =
            media_backend::VideoFrame::new(media_backend::VIDEO_FORMATS, size[0], size[1]);
        settings.stream = media_backend::VideoStream::Ordinal(self.metadata.stream);
        settings.max_source_pixels = 4096 * 2160;
        settings.at_or_before = at_or_before;
        settings.seconds = seconds;
        self.read(cancel, |file| {
            let output = media_backend::video_frame_request(&settings)?
                .arg_owned_content_file(file)
                .map_err(|error| error.to_string())?
                .timeout(Duration::from_secs(8))
                .run_cancellable(cancel)
                .map_err(|error| error.to_string())?;
            let frame = media_backend::decode_video_frame(&output, &settings)?
                .ok_or("No frame at this position")?;
            Ok(Still {
                png: frame.png.to_vec(),
                time_ms: (frame.info.seconds * 1000.0).round() as u64,
            })
        })
    }
}

struct Still {
    png: Vec<u8>,
    time_ms: u64,
}

#[derive(Clone)]
pub(in crate::file_manager) enum FrameCopy {
    Displayed(Arc<egui::ColorImage>, u64),
    Position(Source, u64),
}

impl FrameCopy {
    pub fn copy(self, cancel: &AtomicBool) -> Result<String, String> {
        use image::ImageEncoder;
        let frame = match self {
            Self::Displayed(image, time_ms) => {
                let mut png = Vec::new();
                image::codecs::png::PngEncoder::new(&mut png)
                    .write_image(
                        image.as_raw(),
                        image.width() as u32,
                        image.height() as u32,
                        image::ExtendedColorType::Rgba8,
                    )
                    .map_err(|error| error.to_string())?;
                Still { png, time_ms }
            }
            Self::Position(source, time_ms) => {
                let (width, height) = source.metadata.dimensions;
                let edge = width.max(height).clamp(1, 1920);
                source.frame(time_ms as f64 / 1000.0, [edge, edge], cancel, true)?
            }
        };
        little_clip_core::copy_bytes_to_system_clipboard(&frame.png, "image/png")
            .map_err(|error| error.to_string())?;
        Ok(format!("Copied frame at {}", timestamp(frame.time_ms)))
    }
}

#[derive(Clone)]
pub(in crate::file_manager) struct Clip {
    pub source: Source,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Clip {
    pub fn new(source: Source, (start, end): (f32, f32)) -> Option<Self> {
        let duration = source.metadata.duration_ms;
        let interval = source.metadata.frame_interval_ms;
        let start_ms = ((duration as f64 * f64::from(start)).round() as u64)
            .min(duration.saturating_sub(interval));
        let end_ms = ((duration as f64 * f64::from(end)).round() as u64)
            .max(start_ms.saturating_add(interval))
            .min(duration);
        (end_ms > start_ms).then_some(Self {
            source,
            start_ms,
            end_ms,
        })
    }

    fn rate(&self) -> u64 {
        let metadata = &self.source.metadata;
        let source_rate = (u128::from(metadata.size_bytes) * 8000
            / u128::from(metadata.duration_ms.max(1)))
        .min(u128::from(u64::MAX)) as u64;
        source_rate
            .saturating_sub(self.audio_rate())
            .clamp(128_000, 500_000_000)
    }

    fn audio_rate(&self) -> u64 {
        AUDIO_RATE * u64::from(self.source.metadata.audio_tracks)
    }

    pub fn large_estimate(&self) -> Option<u64> {
        let total =
            u128::from(self.rate() + self.audio_rate()) * u128::from(self.end_ms - self.start_ms);
        // Allow for container overhead. Encoder rate control remains approximate.
        let bytes = (total / 8000 * 102 / 100).min(u128::from(u64::MAX)) as u64;
        (bytes > LARGE_CLIP).then_some(bytes)
    }

    pub fn export(&self, cancel: &AtomicBool) -> Result<String, String> {
        let start = self.start_ms;
        let end = self.end_ms;
        preview_copy::copy_file(
            &self.source.path,
            &format!("{start}-{end}ms"),
            Some("mp4"),
            |output| self.write(output, cancel),
        )?;
        Ok(format!(
            "Copied video clip {}–{}",
            timestamp(start),
            timestamp(end)
        ))
    }

    fn write(&self, output: &File, cancel: &AtomicBool) -> Result<(), String> {
        let ffmpeg = little_clip_core::command_available("ffmpeg")
            .ok_or("Install FFmpeg with H.264 encoding to copy video clips.")?;
        self.source.read(cancel, |input| {
            let request = CommandRequest::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-y",
                    "-filter_threads",
                    "2",
                    "-abort_on",
                    "empty_output",
                ])
                .args([
                    "-protocol_whitelist",
                    "file",
                    "-format_whitelist",
                    media_backend::VIDEO_FORMATS,
                ])
                .arg("-ss")
                .arg(ffmpeg_time(self.start_ms))
                .arg("-i")
                .arg_owned_content_file(input)
                .map_err(|error| error.to_string())?
                .arg("-t")
                .arg(ffmpeg_time(self.end_ms - self.start_ms))
                .arg("-map")
                .arg(format!("0:v:{}", self.source.metadata.stream))
                .args(["-map", "0:a?", "-map_metadata", "-1", "-map_chapters", "-1"])
                .args([
                    "-vf",
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                    "-pix_fmt",
                    "yuv420p",
                    // Keep boundary timestamps through the encoder instead of
                    // rounding to its nominal frame interval.
                    "-enc_time_base:v",
                    "1:1000000",
                    "-fps_mode",
                    "passthrough",
                ])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-threads:v",
                    "4",
                    "-b:v",
                ])
                .arg(self.rate().to_string())
                .args(["-c:a", "aac", "-b:a"])
                .arg(AUDIO_RATE.to_string())
                .args(["-f", "mp4"]);
            let (request, fd) = request
                .inherit_handle(output)
                .map_err(|error| error.to_string())?;
            let result = request
                .arg(format!("/proc/self/fd/{fd}"))
                .timeout(Duration::MAX)
                .die_with_parent()
                .max_output_bytes(16 * 1024)
                .run_cancellable(cancel)
                .map_err(|error| error.to_string())?;
            if !result.status.success() {
                return Err(format!(
                    "Cannot export video clip: {}",
                    result.stderr_lossy().trim()
                ));
            }
            if output.metadata().map_err(|error| error.to_string())?.len() == 0 {
                return Err("Video clip export produced no output".into());
            }
            Ok(())
        })
    }
}

fn ffmpeg_time(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

pub(in crate::file_manager) fn timestamp(ms: u64) -> String {
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

type Endpoints = [Result<(little_clip_core::ImageThumbnail, u64), String>; 2];
pub(in crate::file_manager) struct Selection {
    pub clip: Clip,
    pub revision: u64,
    pending: Option<Task<Endpoints>>,
    pub frames: Option<[Result<(TextureHandle, u64), String>; 2]>,
}

impl Selection {
    pub fn new(clip: Clip, revision: u64) -> Self {
        Self {
            clip,
            revision,
            pending: None,
            frames: None,
        }
    }

    pub fn matches(&self, path: &Path, revision: u64) -> bool {
        self.clip.source.path == path && self.revision == revision
    }

    pub fn prepare_frames(&mut self, ctx: &Context) {
        let clip = self.clip.clone();
        let ctx = ctx.clone();
        self.pending = Some(Task::start(
            &FRAMES,
            Duration::from_secs(17),
            move |cancel| {
                let start = clip.start_ms as f64 / 1000.0;
                let end = clip.end_ms as f64 / 1000.0 - 0.000001;
                let frames = [(start, false), (end, true)].map(|(time, at_or_before)| {
                    let frame = clip.source.frame(time, [192, 108], cancel, at_or_before)?;
                    let image = little_clip_core::ImageThumbnail::generate(&frame.png, 192, 108)
                        .ok_or("Cannot display frame")?;
                    Ok((image, frame.time_ms))
                });
                ctx.request_repaint();
                frames
            },
        ));
    }

    pub fn poll(&mut self, ctx: &Context) {
        let Some(result) = self.pending.as_ref().and_then(Task::poll) else {
            return;
        };
        self.pending = None;
        self.frames = Some(match result {
            Ok(frames) => frames.map(|frame| {
                frame.map(|(image, time_ms)| {
                    (texture(ctx, "video_clip_boundary".into(), image), time_ms)
                })
            }),
            Err(error) => [Err(error.to_string()), Err(error.to_string())],
        });
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::{FileManager, filesystem, media};

    #[test]
    fn one_gigabyte_warning_retains_clip_without_starting_export() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mp4");
        std::fs::write(&path, b"identity fixture").unwrap();
        let metadata = Arc::new(VideoMetadata {
            source: crate::file_source::FileSource::capture(&path).unwrap(),
            dimensions: (640, 360),
            duration_ms: 3_600_000,
            stream: 0,
            codec: "h264".into(),
            size_bytes: 4_000_000_000,
            frame_interval_ms: 40,
            audio_description: String::new(),
            description: String::new(),
            audio_tracks: 1,
        });
        let source = Source {
            path,
            metadata,
            context: PreviewContext::default(),
        };
        let short = Clip::new(source.clone(), (0.1, 0.2)).unwrap();
        let long = Clip::new(source, (0.25, 0.75)).unwrap();
        assert!(short.large_estimate().is_none());
        assert!(
            long.large_estimate()
                .is_some_and(|bytes| bytes > 1_000_000_000)
        );
        let mut browser = FileManager::new(dir.path().to_owned());
        browser.video_previews.selection = Some(Selection::new(long, 1));
        browser.copy_video_range(&Context::default());
        assert!(browser.large_video_copy.is_some());
        assert!(!browser.preview_copies.busy());
    }

    #[test]
    fn export_cuts_at_a_non_keyframe_boundary_and_keeps_video_and_audio() {
        let Some(ffmpeg) = little_clip_core::command_available("ffmpeg") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mp4");
        let fixture = CommandRequest::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:size=96x64:rate=25:duration=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:size=96x64:rate=25:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[colors]",
                "-map",
                "[colors]",
                "-map",
                "2:a",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-g",
                "100",
                "-sc_threshold",
                "0",
                "-c:a",
                "aac",
                "-f",
                "mp4",
            ])
            .arg_content_output(&path)
            .unwrap()
            .run()
            .unwrap();
        assert!(fixture.status.success(), "{}", fixture.stderr_lossy());
        let cancel = AtomicBool::new(false);
        let entry = filesystem::read_directory(dir.path(), &cancel)
            .unwrap()
            .entries
            .remove(0);
        let metadata = Arc::new(media::inspect_video(&entry.path).unwrap());
        let source = Source {
            path,
            metadata,
            context: PreviewContext::default(),
        };
        let clip = Clip::new(source, (0.25, 0.75)).unwrap();
        let begin = clip.source.frame(0.5, [96, 64], &cancel, false).unwrap();
        let end = clip
            .source
            .frame(1.499999, [96, 64], &cancel, true)
            .unwrap();
        assert_eq!(
            (begin.time_ms, end.time_ms),
            (500_u64.div_ceil(40) * 40, 1480)
        );
        let output = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        clip.write(output.as_file(), &cancel).unwrap();
        let rendered = Arc::new(media::inspect_video(output.path()).unwrap());
        assert!(
            (960..=1040).contains(&rendered.duration_ms),
            "{}",
            rendered.duration_ms
        );
        assert_eq!(rendered.audio_tracks, 1);
        for (time, expected) in [(0, 0), (800, 2)] {
            let clip = Source {
                path: output.path().to_owned(),
                metadata: rendered.clone(),
                context: PreviewContext::default(),
            };
            let frame = clip
                .frame(time as f64 / 1000.0, [96, 64], &cancel, false)
                .unwrap();
            let image = image::load_from_memory(&frame.png).unwrap().into_rgb8();
            let pixel = image.get_pixel(40, 30);
            assert!(
                pixel[expected] > 200 && pixel[2 - expected] < 40,
                "{pixel:?}"
            );
        }
    }
}
