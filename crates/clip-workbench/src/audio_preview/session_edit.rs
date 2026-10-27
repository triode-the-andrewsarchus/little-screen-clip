//! Guarded working-file transforms shared by channel edits and range deletion.
use super::*;
use little_clip_core::audio::build_peak_pyramid;

/// Bake existing effects before cutting, so region coordinates retain their
/// meaning on the original timeline and surviving audio keeps those effects.
pub(super) fn deletion_filter(
    start: f32,
    end: f32,
    duration: f32,
    prefix: Option<(String, String)>,
) -> String {
    let (mut graph, input) = prefix.unwrap_or_else(|| (String::new(), "[0:a]".into()));
    if !graph.is_empty() {
        graph.push(';');
    }
    if start <= 0.0 {
        graph.push_str(&format!(
            "{input}atrim=start={end:.6},asetpts=PTS-STARTPTS[out]"
        ));
    } else if end >= duration {
        graph.push_str(&format!(
            "{input}atrim=end={start:.6},asetpts=PTS-STARTPTS[out]"
        ));
    } else {
        graph.push_str(&format!(
            "{input}asplit=2[delete_left][delete_right];\
             [delete_left]atrim=end={start:.6},asetpts=PTS-STARTPTS[delete_before];\
             [delete_right]atrim=start={end:.6},asetpts=PTS-STARTPTS[delete_after];\
             [delete_before][delete_after]concat=n=2:v=0:a=1[out]"
        ));
    }
    graph
}

#[derive(Debug)]
pub(super) struct SessionEditOutcome {
    pub(super) path: PathBuf,
    pub(super) identity: crate::file_source::FileSource,
    pub(super) read_check: crate::file_source::ReadCheck,
    pub(super) source: Arc<FullAudioAnalysis>,
    pub(super) displayed: Option<Arc<FullAudioAnalysis>>,
    pub(super) derived: Option<FullAudioAnalysis>,
    pub(super) spectrum: egui::ColorImage,
    pub(super) notes: egui::ColorImage,
}

pub(super) fn build_channel_signals(source: &FullAudioAnalysis) -> FullAudioAnalysis {
    let left = &source.ch1_samples;
    let right = source.ch2_samples.as_deref().unwrap_or(&[]);
    let sum: Arc<[f32]> = left.iter().zip(right).map(|(l, r)| l + r).collect();
    let difference: Arc<[f32]> = left.iter().zip(right).map(|(l, r)| l - r).collect();
    let (s0, s1, s2) = build_peak_pyramid(&sum, source.sample_rate as f32);
    let (d0, d1, d2) = build_peak_pyramid(&difference, source.sample_rate as f32);
    FullAudioAnalysis {
        channels: 2,
        sample_rate: source.sample_rate,
        duration_ms: source.duration_ms,
        total_frames: sum.len(),
        ch1_samples: sum,
        ch2_samples: Some(difference),
        peaks_lvl0_ch1: s0,
        peaks_lvl1_ch1: s1,
        peaks_lvl2_ch1: s2,
        peaks_lvl0_ch2: Some(d0),
        peaks_lvl1_ch2: Some(d1),
        peaks_lvl2_ch2: Some(d2),
        ..Default::default()
    }
}

fn prepare_session_edit(
    path: &Path,
    edits: &SessionAudioEdits,
    cancelled: &AtomicBool,
) -> Result<SessionEditOutcome, String> {
    let identity = crate::file_source::FileSource::capture(path)?;
    let source = compute_audio_analysis_progressive_cancellable(path, 1200, cancelled, |_| {})
        .ok_or_else(|| "Could not analyze edited audio".to_owned())?;
    let displayed = if *edits == SessionAudioEdits::default() {
        None
    } else {
        Some(Arc::new(
            compute_audio_analysis_progressive_with_gain_and_edits_cancellable(
                path,
                1200,
                edits.track_gain_db,
                &edits.regions,
                cancelled,
                |_| {},
            )
            .ok_or_else(|| "Could not analyze audio effects".to_owned())?,
        ))
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("Session edit cancelled".into());
    }
    let source = Arc::new(source);
    let derived = (source.channels == 2)
        .then(|| build_channel_signals(displayed.as_deref().unwrap_or(&source)));
    if cancelled.load(Ordering::Acquire) {
        return Err("Session edit cancelled".into());
    }
    let visual = displayed.as_deref().unwrap_or(&source);
    let spectrum = egui::ColorImage::from_rgba_unmultiplied(
        [visual.spectrogram_width, visual.spectrogram_height],
        &visual.spectrogram_rgba,
    );
    let notes = egui::ColorImage::from_rgba_unmultiplied(
        [visual.note_strip_width, visual.note_strip_height],
        &visual.note_strip_rgba,
    );
    identity.check()?;
    Ok(SessionEditOutcome {
        path: identity.path().to_owned(),
        identity,
        read_check: Default::default(),
        source,
        displayed,
        derived,
        spectrum,
        notes,
    })
}

#[derive(Debug)]
struct SessionArtifact {
    // The temporary pathname uses the retained directory descriptor. Drop the
    // temporary file before that descriptor, including on discarded completions.
    _staging: tempfile::NamedTempFile,
    _directory: std::fs::File,
    identity: crate::file_source::FileSource,
}

#[derive(Debug)]
pub(super) struct RenderedSessionFile {
    pub(super) staging: tempfile::NamedTempFile,
    pub(super) directory: std::fs::File,
    pub(super) observed: crate::file_source::FileSource,
}

pub(super) fn render_session_file(
    source: &Path,
    expected: Option<&crate::file_source::FileSource>,
    check: &crate::file_source::ReadCheck,
    filter: String,
    mode: export::AudioRenderMode,
    cancelled: &AtomicBool,
) -> Result<RenderedSessionFile, String> {
    use std::os::fd::AsRawFd;
    let (input, observed) = check.open(source, expected, cancelled)?;
    let directory = little_clip_core::kernel_interface::create_content_directory(
        &std::env::temp_dir().join("little_clip_audio_sessions"),
    )
    .map_err(|error| error.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let staging = tempfile::Builder::new()
        .prefix("audio-session-")
        .suffix(".wav")
        .tempfile_in(anchor)
        .map_err(|error| error.to_string())?;
    let region = AudioSaveRegion {
        start: 0.0,
        end: 0.0,
        filename: "session.wav".into(),
        filter: Some(filter),
    };
    let input_path = PathBuf::from(format!("/proc/self/fd/{}", input.as_raw_fd()));
    let mut command = export::audio_export_command(
        &input_path,
        &region,
        staging
            .as_file()
            .try_clone()
            .map_err(|error| error.to_string())?,
        mode,
    )?;
    let status = run_audio_command_cancellable(&mut command, cancelled)
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("Session edit failed: {status}"));
    }
    check.validate(cancelled)?;
    observed.check()?;
    staging
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    Ok(RenderedSessionFile {
        staging,
        directory,
        observed,
    })
}

pub(super) fn render_session_edit(
    source: &Path,
    expected: Option<&crate::file_source::FileSource>,
    check: &crate::file_source::ReadCheck,
    filter: String,
    mode: export::AudioRenderMode,
    edits: &SessionAudioEdits,
    cancelled: &AtomicBool,
) -> Result<SessionEditOutcome, String> {
    let rendered = render_session_file(source, expected, check, filter, mode, cancelled)?;
    let outcome = prepare_session_artifact(rendered.staging, rendered.directory, edits, cancelled)?;
    check.validate(cancelled)?;
    rendered.observed.check()?;
    Ok(outcome)
}

fn prepare_session_artifact(
    staging: tempfile::NamedTempFile,
    directory: std::fs::File,
    edits: &SessionAudioEdits,
    cancelled: &AtomicBool,
) -> Result<SessionEditOutcome, String> {
    use std::os::fd::AsRawFd;
    let prepared_path = PathBuf::from(format!("/proc/self/fd/{}", staging.as_file().as_raw_fd()));
    let mut outcome = prepare_session_edit(&prepared_path, edits, cancelled)?;
    let artifact = Arc::new(SessionArtifact {
        _staging: staging,
        _directory: directory,
        identity: outcome.identity.clone(),
    });
    outcome.read_check = crate::file_source::ReadCheck::new(move |cancel| {
        if cancel.load(Ordering::Acquire) {
            return Err("Audio operation cancelled".into());
        }
        artifact.identity.check()
    });
    Ok(outcome)
}

pub(super) struct SessionEditJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<SessionEditOutcome, String>, JobError>>,
    before: AudioEditSnapshot,
    action: SessionEditAction,
}

impl Drop for SessionEditJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

#[derive(Clone, Copy)]
pub(super) enum SessionEditAction {
    Channels,
    Delete { start: f32, deleted: f32 },
}
impl SessionEditAction {
    fn label(self) -> &'static str {
        match self {
            Self::Channels => "Channel edit",
            Self::Delete { .. } => "Range deletion",
        }
    }
    fn success_message(self) -> String {
        match self {
            Self::Channels => "Channel edit applied; undo with Ctrl+Z".into(),
            Self::Delete { deleted, .. } => format!("Deleted {deleted:.2}s; undo with Ctrl+Z"),
        }
    }
}

impl AudioPreviewState {
    pub(super) fn start_session_edit(
        &mut self,
        filter: String,
        mode: export::AudioRenderMode,
        edits: SessionAudioEdits,
        action: SessionEditAction,
    ) {
        if self.session_edit_job.is_some() {
            return;
        }
        let before = self.edit_snapshot();
        let source = self.path.clone();
        let check = self.read_check.clone();
        let expected = self.file_source.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (ticket, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            render_session_edit(
                &source,
                expected.as_ref(),
                &check,
                filter,
                mode,
                &edits,
                &worker_cancelled,
            )
        });
        self.session_edit_job = Some(SessionEditJob {
            ticket,
            cancelled,
            receiver,
            before,
            action,
        });
    }
    pub(super) fn poll_session_edit(&mut self, ctx: &egui::Context) {
        let Some(job) = self.session_edit_job.take() else {
            return;
        };
        let now = ctx.input(|input| input.time);
        match job.receiver.try_recv() {
            Ok(Ok(Ok(outcome))) => {
                if self.edit_snapshot() != job.before {
                    self.status_toast = Some((
                        format!(
                            "{} discarded because the session changed",
                            job.action.label()
                        )
                        .into(),
                        now,
                    ));
                    return;
                }
                let current = self.current_time_sec;
                let playing = self.is_playing;
                self.stop();
                self.cancel_session_analysis();
                self.cancel_edit_analysis();
                self.path = outcome.path;
                self.read_check = outcome.read_check;
                self.file_source = Some(outcome.identity);
                self.analysis = Some(outcome.source.clone());
                self.install_audio_textures(ctx, outcome.spectrum, outcome.notes);
                self.completed_source_analysis = Some(outcome.source);
                self.edited_waveform = outcome.displayed;
                self.session_transients = self
                    .analysis
                    .as_ref()
                    .map(|analysis| analysis.transients.clone().into());
                self.is_loading_analysis = false;
                match job.action {
                    SessionEditAction::Channels => {}
                    SessionEditAction::Delete { start, .. } => {
                        let source = self.analysis.as_ref().unwrap();
                        self.duration_sec = source.total_frames as f32 / source.sample_rate as f32;
                        if let Some(metadata) = self.metadata.as_mut() {
                            metadata.duration_ms = source.duration_ms;
                        }
                        self.view_start_sec = 0.0;
                        self.view_duration_sec = self.duration_sec;
                        self.selection = None;
                        self.track_gain_db = 0.0;
                        self.clear_multi_regions();
                        self.clear_single_region_effects();
                        self.current_time_sec = start.min(self.duration_sec);
                    }
                }
                let display = self
                    .edited_waveform
                    .as_ref()
                    .or(self.analysis.as_ref())
                    .unwrap();
                self.channel_tools
                    .install_analysis(display, outcome.derived);
                self.edit_analysis_observed = self.session_audio_edits();
                self.edit_analysis_inflight = self.edit_analysis_observed.clone();
                self.edit_analysis_displayed = self.edit_analysis_observed.clone();
                self.record_edit(job.before.clone());
                if matches!(job.action, SessionEditAction::Channels) {
                    self.current_time_sec = current;
                }
                if playing {
                    self.play();
                }
                self.status_toast = Some((job.action.success_message().into(), now));
            }
            Err(TryRecvError::Empty) => {
                self.session_edit_job = Some(job);
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            Ok(Ok(Err(error))) => {
                self.status_toast = Some((
                    format!(
                        "Could not apply {}: {error}",
                        job.action.label().to_lowercase()
                    )
                    .into(),
                    now,
                ));
            }
            Ok(Err(error)) => {
                self.status_toast = Some((
                    format!(
                        "Could not apply {}: {error}",
                        job.action.label().to_lowercase()
                    )
                    .into(),
                    now,
                ));
            }
            Err(TryRecvError::Disconnected) => {
                self.status_toast =
                    Some(("Session edit worker stopped without a result.".into(), now));
            }
        }
    }
}
