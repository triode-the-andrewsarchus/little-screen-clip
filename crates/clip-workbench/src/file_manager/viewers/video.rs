//! An owned FFplay window provides synchronized playback without retaining decoded frames in Files.
use crate::file_source::FileSource;
use egui::Ui;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use super::super::media::{VideoMetadata as Prepared, inspect_video};

pub(super) fn prepare(path: &std::path::Path) -> Result<Prepared, String> {
    let prepared = inspect_video(path)?;
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

pub(super) struct Video {
    path: PathBuf,
    prepared: Prepared,
    child: Option<Child>,
    start_seconds: f64,
    status: String,
}
impl Video {
    pub fn new(path: PathBuf, prepared: Prepared, start_seconds: f64) -> Self {
        let start_seconds = if start_seconds.is_finite() {
            start_seconds.clamp(
                0.0,
                prepared
                    .duration_ms
                    .saturating_sub(prepared.frame_interval_ms) as f64
                    / 1000.0,
            )
        } else {
            0.0
        };
        let mut video = Self {
            path,
            prepared,
            child: None,
            start_seconds,
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
            .arg("-ss")
            .arg(format!("{:.3}", self.start_seconds))
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

    fn fixture(path: PathBuf, child: Option<Child>) -> Video {
        Video {
            prepared: Prepared {
                source: FileSource::capture(&path).unwrap(),
                dimensions: (320, 180),
                duration_ms: 1000,
                stream: 0,
                codec: "h264".into(),
                size_bytes: 0,
                frame_interval_ms: 40,
                description: String::new(),
                audio_description: String::new(),
            },
            path,
            child,
            start_seconds: 0.0,
            status: String::new(),
        }
    }

    #[test]
    fn returning_to_folder_terminates_and_reaps_owned_video() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.mp4");
        std::fs::write(&path, "fixture").unwrap();
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let video = fixture(path, Some(child));
        drop(video);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn replay_rejects_changed_source_before_launch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.mp4");
        std::fs::write(&path, "original").unwrap();
        let video = fixture(path.clone(), None);
        std::fs::write(path, "replacement file").unwrap();
        assert!(video.spawn().unwrap_err().contains("source changed"));
    }
}
