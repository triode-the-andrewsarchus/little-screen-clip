use super::*;

impl MidiViewerState {
    pub(crate) fn export_pending(&self) -> bool {
        self.export_job.is_some()
    }
    pub fn copy_selection(&mut self, now: f64) -> bool {
        self.copy_selection_on(
            now,
            BackgroundJobs::global(),
            midi_export_dir(),
            handoff_midi_export,
        )
    }

    pub(super) fn copy_selection_on(
        &mut self,
        now: f64,
        jobs: &BackgroundJobs,
        export_dir: PathBuf,
        handoff: impl FnOnce(
            MidiExportArtifact,
            &AtomicBool,
            Instant,
        ) -> Result<MidiExportArtifact, String>
        + Send
        + 'static,
    ) -> bool {
        let (s, e, is_slice) = match self.selected_region {
            Some((a, b)) if (b - a).abs() > 0.0 => (a.min(b), a.max(b), true),
            _ => (0.0, self.data.duration_sec.max(0.1), false),
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.cancel_export_job();
        let data = Arc::clone(&self.data);
        let solo_track = self.solo_track;
        let cancelled = Arc::new(AtomicBool::new(false));
        let job_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::from_secs(60), move || {
            let started = Instant::now();
            let artifact = write_midi_export(
                &data,
                s,
                e,
                is_slice,
                solo_track,
                timestamp,
                export_dir,
                Arc::clone(&job_cancelled),
            )?;
            handoff(artifact, &job_cancelled, started)
        });
        self.export_job = Some(MidiExportJob {
            ticket,
            cancelled,
            receiver,
        });
        self.status_toast.set("Preparing MIDI export…", now);
        true
    }

    pub(super) fn cancel_export_job(&mut self) {
        if let Some(job) = self.export_job.take() {
            job.cancelled.store(true, Ordering::Release);
            job.ticket.cancel();
        }
    }

    pub(crate) fn poll_export(&mut self, now: f64, ctx: &egui::Context) {
        let Some(job) = self.export_job.as_ref() else {
            return;
        };
        match job.receiver.try_recv() {
            Ok(Ok(Ok(mut artifact))) => {
                let desc = if artifact.is_slice {
                    format!(
                        "Copied {:.1}s MIDI slice ({} notes{}) to Clipboard & Left Hand!",
                        artifact.duration_sec, artifact.total_notes, artifact.solo_info
                    )
                } else {
                    format!(
                        "Copied full MIDI ({} notes{}) to Clipboard & Left Hand!",
                        artifact.total_notes, artifact.solo_info
                    )
                };
                match artifact.delivery {
                    MidiExportDelivery::PushedToWorkbench => {}
                    MidiExportDelivery::ClipboardFallback => {
                        tracing::debug!("MIDI export delivered through system clipboard fallback");
                    }
                }
                artifact.retain();
                self.status_toast.set(desc, now);
                self.export_job = None;
            }
            Ok(Ok(Err(error))) => {
                if error == "MIDI export cancelled" {
                    self.export_job = None;
                    return;
                }
                self.status_toast.set(error, now);
                self.export_job = None;
            }
            Ok(Err(JobError::Cancelled)) => self.export_job = None,
            Ok(Err(error)) => {
                self.status_toast
                    .set(format!("Failed to export MIDI: {error}"), now);
                self.export_job = None;
            }
            Err(TryRecvError::Disconnected) => {
                self.status_toast
                    .set("MIDI export worker disconnected", now);
                self.export_job = None;
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }
}
pub(super) fn midi_export_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir)
            .join("little-screen-clip")
            .join("midi_slices")
    } else {
        PathBuf::from("/tmp/little-screen-clip/midi_slices")
    }
}

pub(super) fn write_unique_midi_export(
    slice_dir: &std::path::Path,
    filename: &str,
    bytes: &[u8],
) -> Result<(PathBuf, std::fs::File), String> {
    let directory = little_clip_core::kernel_interface::create_content_directory(slice_dir)
        .map_err(|error| format!("Failed to create MIDI export directory: {error}"))?;
    let path = write_unique_midi_export_in(&directory, filename, bytes)?;
    Ok((path, directory))
}

pub(super) fn write_unique_midi_export_in(
    directory: &std::fs::File,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    use std::{os::fd::AsRawFd, path::Path};
    let base_path = Path::new(filename);
    if base_path.file_name() != Some(std::ffi::OsStr::new(filename)) {
        return Err("Enter one MIDI export filename".into());
    }
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let parent = std::fs::read_link(&anchor).map_err(|error| error.to_string())?;
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("midi_export");
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("mid");

    let mut temporary =
        tempfile::NamedTempFile::new_in(&anchor).map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    for suffix in 0u32.. {
        let name = if suffix == 0 {
            filename.to_owned()
        } else {
            format!("{stem}_{suffix}.{extension}")
        };
        match temporary.persist_noclobber(anchor.join(&name)) {
            Ok(_) => return Ok(std::fs::read_link(&anchor).unwrap_or(parent).join(name)),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.file;
            }
            Err(error) => return Err(format!("Failed to write MIDI file: {error}")),
        }
    }
    unreachable!("u32 collision suffix space exhausted")
}

pub(super) fn write_midi_export(
    data: &MidiData,
    start_sec: f32,
    end_sec: f32,
    is_slice: bool,
    solo_track: Option<usize>,
    timestamp: u128,
    slice_dir: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<MidiExportArtifact, String> {
    let sliced_data = data.slice(start_sec, end_sec, solo_track);
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    let midi_bytes = sliced_data
        .to_midi_bytes()
        .map_err(|error| format!("Failed to export MIDI: {error}"))?;
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }

    let track_suffix = solo_track.map_or_else(String::new, |track| {
        let name = data
            .track_names
            .get(track)
            .map(|name| name.replace(|c: char| !c.is_alphanumeric() && c != '_', "_"))
            .unwrap_or_else(|| format!("track_{}", track + 1));
        format!("_{name}")
    });
    let filename = if is_slice {
        format!("midi_slice_{timestamp}_{start_sec:.1}s-{end_sec:.1}s{track_suffix}.mid")
    } else {
        format!("midi_export_{timestamp}{track_suffix}.mid")
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    let (path, directory) = write_unique_midi_export(&slice_dir, &filename, &midi_bytes)?;

    let artifact = MidiExportArtifact {
        path,
        directory,
        total_notes: sliced_data.total_notes,
        duration_sec: end_sec - start_sec,
        is_slice,
        solo_info: solo_track.map_or_else(String::new, |track| {
            let name = data
                .track_names
                .get(track)
                .map(String::as_str)
                .unwrap_or("track");
            format!(" (solo '{name}')")
        }),
        delivery: MidiExportDelivery::PushedToWorkbench,
        retain_file: false,
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    Ok(artifact)
}

pub(super) fn handoff_midi_export(
    mut artifact: MidiExportArtifact,
    cancelled: &AtomicBool,
    started: Instant,
) -> Result<MidiExportArtifact, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    // Keep enough of the job deadline for bounded IPC and the clipboard fallback.
    if started.elapsed() >= Duration::from_secs(45) {
        return Err("MIDI export timed out before handoff".to_string());
    }
    let request = IpcRequest::PushFile {
        path: artifact.path.clone(),
        hand: HandSide::Left,
    };
    if crate::send_ipc_request_with_timeout(&request, Some(Duration::from_secs(2))).is_ok() {
        artifact.retain();
        return Ok(artifact);
    }

    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    little_clip_core::copy_file_uris_to_system_clipboard(std::slice::from_ref(&artifact.path))
        .map_err(|error| format!("Failed to copy MIDI file to system clipboard: {error}"))?;
    artifact.delivery = MidiExportDelivery::ClipboardFallback;
    artifact.retain();
    Ok(artifact)
}
