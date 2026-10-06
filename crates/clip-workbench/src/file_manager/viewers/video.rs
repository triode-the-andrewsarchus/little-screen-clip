//! An owned FFplay window provides synchronized playback without retaining decoded frames in Files.
use crate::file_source::FileSource;
use egui::Ui;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub(super) struct Prepared {
    source: FileSource,
    dimensions: (u32, u32),
    duration_ms: u64,
    stream: usize,
}

fn inspect(path: &std::path::Path) -> Result<Prepared, String> {
    let source = FileSource::capture(path)?;
    let probe = little_clip_core::process::CommandRequest::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v",
            "-show_entries",
            "stream=width,height:stream_disposition=attached_pic:format=duration",
            "-of",
            "json",
        ])
        .arg(path.as_os_str())
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
    Ok(Prepared {
        source,
        dimensions: (width, height),
        duration_ms,
        stream,
    })
}

pub(super) fn prepare(path: &std::path::Path) -> Result<Prepared, String> {
    let prepared = inspect(path)?;
    let version = little_clip_core::process::CommandRequest::new("ffplay")
        .arg("-version")
        .timeout(Duration::from_secs(3))
        .max_output_bytes(4096)
        .run()
        .map_err(|e| format!("Video playback requires FFplay: {e}"))?;
    if !version.status.success() {
        return Err("FFplay is unavailable; install the FFmpeg player package.".into());
    }
    if FileSource::capture(path)? != prepared.source {
        return Err("Video changed while opening; open it again.".into());
    }
    Ok(prepared)
}

pub(in crate::file_manager) fn preview(
    path: &std::path::Path,
) -> Option<little_clip_core::ImageThumbnail> {
    let prepared = inspect(path).ok()?;
    let image = super::super::previews::media_frame(path, prepared.stream)?;
    (FileSource::capture(path).ok()? == prepared.source).then_some(image)
}

// Core's preview metadata reader does not distinguish an attached cover from the
// stream played here. Inspect one selected stream and pass its index to FFplay.
fn stream_metadata(value: &serde_json::Value) -> Result<(usize, u32, u32, u64), String> {
    let (index, stream) = value["streams"]
        .as_array()
        .and_then(|streams| {
            streams.iter().enumerate().find(|(_, stream)| {
                stream["disposition"]["attached_pic"].as_u64().unwrap_or(0) == 0
            })
        })
        .ok_or("No ordinary video stream is available")?;
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

pub(super) struct Video {
    path: PathBuf,
    prepared: Prepared,
    child: Option<Child>,
    status: String,
}
impl Video {
    pub fn new(path: PathBuf, prepared: Prepared) -> Self {
        let mut video = Self {
            path,
            prepared,
            child: None,
            status: String::new(),
        };
        video.start();
        video
    }
    fn start(&mut self) {
        self.stop();
        let result = self.spawn();
        match result {
            Ok(child) => {
                self.child = Some(child);
                self.status = "Playing in the Files video window".into();
            }
            Err(error) => self.status = error,
        }
    }
    fn spawn(&self) -> Result<Child, String> {
        if FileSource::capture(&self.path)? != self.prepared.source {
            return Err("Video source changed. Return to the folder and open it again.".into());
        }
        let mut command = Command::new("ffplay");
        command
            .args([
                "-loglevel",
                "error",
                "-nostats",
                "-autoexit",
                "-framedrop",
                "-noinfbuf",
                "-threads",
                "2",
                "-filter_threads",
                "1",
                "-max_alloc",
                "268435456",
                "-x",
                "960",
                "-y",
                "540",
                "-window_title",
                "Files video",
            ])
            .arg("-vst")
            .arg(format!("v:{}", self.prepared.stream))
            .arg("-i")
            .arg(&self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Linux parent-death ownership also covers an abrupt Files crash.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            let parent = unsafe { libc::getpid() };
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
                    }
                    Ok(())
                });
            }
        }
        command
            .spawn()
            .map_err(|e| format!("Cannot start video playback: {e}"))
    }
    pub(super) fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.status = "Video stopped".into();
    }
    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool) {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.child = None;
                    self.status = if status.success() {
                        "Video finished or closed".into()
                    } else {
                        format!(
                            "Video playback failed ({status}). Check codec support or open externally."
                        )
                    };
                }
                Err(error) => {
                    self.stop();
                    self.status = format!("Cannot monitor video playback: {error}");
                }
                Ok(None) => ui.ctx().request_repaint_after(Duration::from_millis(100)),
            }
        }
        ui.heading("Video playback");
        ui.label(format!(
            "{}×{} · {:.1} seconds",
            self.prepared.dimensions.0,
            self.prepared.dimensions.1,
            self.prepared.duration_ms as f64 / 1000.0
        ));
        ui.label(&self.status);
        ui.label("In the video window: Space pauses/resumes, Left/Right seek 10 seconds, Up/Down seek one minute, right-click seeks to a point, F toggles fullscreen, Q closes playback.");
        ui.label("Music stays paused while this video session is open. Returning to the folder closes the video window.");
        ui.horizontal(|ui| {
            if ui.button("Replay video").clicked() {
                self.start();
            }
            if ui
                .add_enabled(self.child.is_some(), egui::Button::new("Stop video"))
                .clicked()
            {
                self.stop();
                self.status = "Video stopped".into();
            }
        });
        *close |= ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        self.stop();
    }
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

    #[test]
    fn returning_to_folder_terminates_and_reaps_owned_video() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.mp4");
        std::fs::write(&path, "fixture").unwrap();
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let video = Video {
            prepared: Prepared {
                source: FileSource::capture(&path).unwrap(),
                dimensions: (320, 180),
                duration_ms: 1000,
                stream: 0,
            },
            path,
            child: Some(child),
            status: String::new(),
        };
        drop(video);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn replay_rejects_changed_source_before_launch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.mp4");
        std::fs::write(&path, "original").unwrap();
        let video = Video {
            prepared: Prepared {
                source: FileSource::capture(&path).unwrap(),
                dimensions: (320, 180),
                duration_ms: 1000,
                stream: 0,
            },
            path: path.clone(),
            child: None,
            status: String::new(),
        };
        std::fs::write(path, "replacement file").unwrap();
        assert!(video.spawn().unwrap_err().contains("source changed"));
    }
}
