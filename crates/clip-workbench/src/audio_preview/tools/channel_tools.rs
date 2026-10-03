//! Stereo routing edits and synchronized signal inspection.
use super::super::*;
use little_clip_core::audio::build_peak_pyramid;
use little_clip_ops::hf_audio::HfSpectrogramConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelEdit {
    Sum,
    Swap,
    InvertLeft,
    InvertRight,
    LeftToRight,
    RightToLeft,
}

impl ChannelEdit {
    const ALL: [Self; 6] = [
        Self::Sum,
        Self::Swap,
        Self::InvertLeft,
        Self::InvertRight,
        Self::LeftToRight,
        Self::RightToLeft,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Sum => "[1] Sum → both",
            Self::Swap => "[2] Swap L/R",
            Self::InvertLeft => "[3] Invert L",
            Self::InvertRight => "[4] Invert R",
            Self::LeftToRight => "[5] Copy L → R",
            Self::RightToLeft => "[6] Copy R → L",
        }
    }

    fn expressions(self) -> [&'static str; 2] {
        match self {
            Self::Sum => ["val(0)+val(1)", "val(0)+val(1)"],
            Self::Swap => ["val(1)", "val(0)"],
            Self::InvertLeft => ["-val(0)", "val(1)"],
            Self::InvertRight => ["val(0)", "-val(1)"],
            Self::LeftToRight => ["val(0)", "val(0)"],
            Self::RightToLeft => ["val(1)", "val(1)"],
        }
    }

    fn filter(self, regions: &[(f32, f32)]) -> String {
        let gate = regions
            .iter()
            .map(|&(start, end)| {
                let (start, end) = AudioSelectionModel::normalize((start, end));
                format!("gte(t,{start:.9})*lt(t,{end:.9})")
            })
            .collect::<Vec<_>>()
            .join("+");
        let expressions = self
            .expressions()
            .into_iter()
            .enumerate()
            .map(|(channel, wet)| {
                if gate.is_empty() {
                    wet.to_owned()
                } else {
                    format!("if({gate},{wet},val({channel}))")
                }
            })
            .collect::<Vec<_>>()
            .join("|");
        format!("aeval='{expressions}':c=stereo")
    }
}

#[derive(Debug)]
struct ChannelEditOutcome {
    path: PathBuf,
    source: Arc<FullAudioAnalysis>,
    displayed: Option<Arc<FullAudioAnalysis>>,
    derived: FullAudioAnalysis,
    spectrum: egui::ColorImage,
    notes: egui::ColorImage,
}

fn build_channel_signals(source: &FullAudioAnalysis) -> FullAudioAnalysis {
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

fn prepare_channel_edit(
    path: &Path,
    edits: &SessionAudioEdits,
    cancelled: &AtomicBool,
) -> Result<ChannelEditOutcome, String> {
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
        return Err("Channel edit cancelled".into());
    }
    let source = Arc::new(source);
    let derived = build_channel_signals(displayed.as_deref().unwrap_or(&source));
    if cancelled.load(Ordering::Acquire) {
        return Err("Channel edit cancelled".into());
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
    Ok(ChannelEditOutcome {
        path: path.to_owned(),
        source,
        displayed,
        derived,
        spectrum,
        notes,
    })
}

struct ChannelEditJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<ChannelEditOutcome, String>, JobError>>,
    before: AudioEditSnapshot,
}

impl Drop for ChannelEditJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

struct DerivedSignalsJob {
    ticket: JobTicket,
    receiver: Receiver<Result<FullAudioAnalysis, JobError>>,
}

impl Drop for DerivedSignalsJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

#[derive(Default)]
pub(in crate::audio_preview) struct ChannelToolsState {
    entire_track: bool,
    reverse_difference: bool,
    solo: u64,
    pub(in crate::audio_preview) monitor: Arc<AtomicU64>,
    edit_job: Option<ChannelEditJob>,
    source_samples: Option<[Arc<[f32]>; 2]>,
    derived: Option<FullAudioAnalysis>,
    derived_job: Option<DerivedSignalsJob>,
    spectra: spectra::ChannelSpectra,
}

impl AudioPreviewState {
    pub(in crate::audio_preview) fn is_stereo(&self) -> bool {
        self.analysis
            .as_ref()
            .map(|analysis| analysis.channels == 2)
            .or_else(|| {
                self.metadata
                    .as_ref()
                    .map(|metadata| metadata.channels == 2)
            })
            .unwrap_or(false)
    }

    pub(in crate::audio_preview) fn sync_channel_monitor(&self) {
        let mode = if self.active_mode != AudioToolMode::ChannelTools {
            0
        } else if self.channel_tools.solo == 2 && self.channel_tools.reverse_difference {
            3
        } else {
            self.channel_tools.solo
        };
        self.channel_tools.monitor.store(mode, Ordering::Release);
    }

    pub(in crate::audio_preview) fn render_channel_workspace(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        layout::AudioWorkspace::control_bar(ui, palette, |ui| {
            let (icon, label) = if self.is_playing {
                (crate::icons::Icon::Pause, "Pause [Space]")
            } else {
                (crate::icons::Icon::Play, "Play [Space]")
            };
            if ui
                .add(layout::AudioWorkspace::primary_button_icon(
                    Some(icon),
                    label,
                    hex_to_c32(&palette.diff_add),
                    palette,
                ))
                .clicked()
            {
                self.toggle_play();
            }
            if ui
                .add(layout::AudioWorkspace::secondary_button(
                    "Start",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                ))
                .clicked()
            {
                self.seek_to(0.0);
            }
            if ui
                .add(layout::AudioWorkspace::toggle_button(
                    "Loop [L]",
                    self.loop_selection,
                    palette,
                ))
                .clicked()
            {
                self.loop_selection = !self.loop_selection;
            }
            if ui
                .add(layout::AudioWorkspace::secondary_button(
                    "Fit",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                ))
                .clicked()
            {
                self.fit_to_window();
            }
            if ui
                .add(layout::AudioWorkspace::secondary_button(
                    "Copy [Ctrl+C]",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                ))
                .clicked()
            {
                let now = ctx.input(|input| input.time);
                match self.selection_mode {
                    SelectionMode::Single => {
                        self.copy_selected_segment(now, ChannelCopyMode::Native)
                    }
                    SelectionMode::Multi => {
                        self.copy_all_multi_segments(now, ChannelCopyMode::Native)
                    }
                }
            }
            if ui
                .add(layout::AudioWorkspace::secondary_button(
                    "Save [Ctrl+S]",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                ))
                .clicked()
            {
                self.open_save_dialog();
            }
        });
        let available = ui.available_rect_before_wrap();
        egui::SidePanel::left(ui.make_persistent_id("channel_controls"))
            .exact_width(276.0)
            .resizable(false)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| self.render_channel_controls(ui, palette));
            });
        let canvas = ui.available_rect_before_wrap();
        self.render_waveform_canvas(
            ui,
            ctx,
            palette,
            canvas.width(),
            available.height().max(0.0),
        );
        self.render_save_dialog(ctx, palette, ctx.input(|input| input.time));
    }

    fn render_channel_controls(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.label("Apply to");
        ui.horizontal(|ui| {
            for (label, whole) in [("Regions", false), ("Track", true)] {
                if ui
                    .add(layout::AudioWorkspace::toggle_button(
                        label,
                        self.channel_tools.entire_track == whole,
                        palette,
                    ))
                    .clicked()
                {
                    self.channel_tools.entire_track = whole;
                }
            }
        });
        ui.add_space(4.0);
        let enabled = self.channel_edit_enabled();
        egui::Grid::new("channel_operations")
            .num_columns(2)
            .show(ui, |ui| {
                for (index, operation) in ChannelEdit::ALL.into_iter().enumerate() {
                    if ui
                        .add_enabled(
                            enabled,
                            layout::AudioWorkspace::secondary_button(
                                operation.label(),
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            ),
                        )
                        .clicked()
                    {
                        self.start_channel_edit(operation);
                    }
                    if index % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        if self.channel_tools.edit_job.is_some() {
            ui.label("Applying channel edit…");
        }
        self.render_channel_selection_controls(ui, palette);
    }

    fn render_channel_selection_controls(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.separator();
        ui.label("Selection & snapping");
        if ui
            .add(layout::AudioWorkspace::toggle_button(
                "Multi-region [M]",
                self.selection_mode == SelectionMode::Multi,
                palette,
            ))
            .clicked()
        {
            self.set_selection_mode(if self.selection_mode == SelectionMode::Single {
                SelectionMode::Multi
            } else {
                SelectionMode::Single
            });
        }
        if ui
            .add(layout::AudioWorkspace::toggle_button(
                "Zero-crossings [Z]",
                self.snap_zero_crossings,
                palette,
            ))
            .clicked()
        {
            self.snap_zero_crossings = !self.snap_zero_crossings;
            if self.snap_zero_crossings {
                self.snap_active_selection_to_zero_crossings(ui.input(|input| input.time));
            }
        }
        let ready = self
            .analysis
            .as_ref()
            .is_some_and(|analysis| analysis.is_transients_ready);
        if ui
            .add_enabled(
                ready,
                layout::AudioWorkspace::toggle_button(
                    "Transients [T]",
                    self.show_transients,
                    palette,
                ),
            )
            .clicked()
        {
            self.show_transients = !self.show_transients;
        }
        if self.show_transients {
            let mut sensitivity = transient_sensitivity_to_slider(self.transient_sensitivity);
            if ui
                .add(
                    egui::Slider::new(&mut sensitivity, 0.0..=1.0)
                        .text("Sensitivity")
                        .show_value(false),
                )
                .changed()
            {
                self.set_transient_sensitivity(transient_sensitivity_from_slider(sensitivity));
            }
        }
        if let Some((start, end)) = self.active_selection_range() {
            ui.label(format!("{start:.3}–{end:.3}s"));
        } else {
            ui.label("Drag any plot to select a region.");
        }
        ui.small("Wheel: zoom · Middle drag: pan");
        ui.small("Sum = L+R · Difference = L−R / R−L");
        ui.small("Sum/difference waveforms use a ±2 scale.");
        self.sync_channel_monitor();
    }

    fn channel_edit_enabled(&self) -> bool {
        let selected = match self.selection_mode {
            SelectionMode::Single => self.selection.is_some(),
            SelectionMode::Multi => !self.multi_selections.is_empty(),
        };
        self.is_stereo()
            && self.channel_tools.edit_job.is_none()
            && !self.is_loading_analysis
            && (self.channel_tools.entire_track || selected)
    }

    pub(in crate::audio_preview) fn handle_channel_shortcuts(&mut self, ctx: &egui::Context) {
        if self.active_mode != AudioToolMode::ChannelTools {
            return;
        }
        let key = ctx.input(|input| {
            if input.modifiers.any() {
                return None;
            }
            AUDIO_NUMBER_KEYS
                .into_iter()
                .position(|key| input.key_pressed(key))
        });
        match key {
            Some(index @ 1..=6) if self.channel_edit_enabled() => {
                self.start_channel_edit(ChannelEdit::ALL[index - 1])
            }
            Some(0) => self.channel_tools.solo = 0,
            Some(7) => self.channel_tools.solo = 1,
            Some(8) => self.channel_tools.solo = 2,
            Some(9) => {
                self.channel_tools.reverse_difference = !self.channel_tools.reverse_difference
            }
            _ => {}
        }
        self.sync_channel_monitor();
    }

    fn start_channel_edit(&mut self, operation: ChannelEdit) {
        if !self.is_stereo() || self.channel_tools.edit_job.is_some() {
            return;
        }
        let regions = if self.channel_tools.entire_track {
            Vec::new()
        } else {
            match self.selection_mode {
                SelectionMode::Single => self.selection.into_iter().collect(),
                SelectionMode::Multi => self.multi_selections.clone(),
            }
        };
        if !self.channel_tools.entire_track && regions.is_empty() {
            return;
        }
        let filter = operation.filter(&regions);
        let before = self.edit_snapshot();
        let source = self.path.clone();
        let edits = self.session_audio_edits();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            let output = std::env::temp_dir()
                .join("little_clip_audio_sessions")
                .join(format!("channels-{}.wav", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
            let mut command = std::process::Command::new("ffmpeg");
            command
                .args(["-v", "error", "-y", "-i"])
                .arg(source)
                .args(["-af", &filter, "-c:a", "pcm_f32le"])
                .arg(&output);
            let result = run_audio_command_cancellable(&mut command, &worker_cancelled);
            match result {
                Ok(status) if status.success() => {
                    let prepared = prepare_channel_edit(&output, &edits, &worker_cancelled);
                    if prepared.is_err() {
                        let _ = std::fs::remove_file(&output);
                    }
                    prepared
                }
                result => {
                    let _ = std::fs::remove_file(&output);
                    Err(format!("Channel edit failed: {result:?}"))
                }
            }
        });
        self.channel_tools.edit_job = Some(ChannelEditJob {
            ticket,
            cancelled,
            receiver,
            before,
        });
    }

    pub(in crate::audio_preview) fn poll_channel_edit(&mut self, ctx: &egui::Context) {
        let Some(job) = self.channel_tools.edit_job.take() else {
            return;
        };
        let now = ctx.input(|input| input.time);
        match job.receiver.try_recv() {
            Ok(Ok(Ok(outcome))) => {
                if self.edit_snapshot() != job.before {
                    let _ = std::fs::remove_file(outcome.path);
                    self.status_toast = Some((
                        "Channel edit discarded because the session changed".into(),
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
                self.analysis = Some(outcome.source.clone());
                self.install_audio_textures(ctx, outcome.spectrum, outcome.notes);
                self.completed_source_analysis = Some(outcome.source);
                self.edited_waveform = outcome.displayed;
                self.session_transients = self
                    .analysis
                    .as_ref()
                    .map(|analysis| analysis.transients.clone().into());
                self.is_loading_analysis = false;
                let display = self
                    .edited_waveform
                    .as_ref()
                    .or(self.analysis.as_ref())
                    .unwrap();
                self.channel_tools.source_samples = display
                    .ch2_samples
                    .as_ref()
                    .map(|right| [display.ch1_samples.clone(), right.clone()]);
                self.channel_tools.derived_job = None;
                self.channel_tools.derived = Some(outcome.derived);
                self.edit_analysis_observed = self.session_audio_edits();
                self.edit_analysis_inflight = self.edit_analysis_observed.clone();
                self.edit_analysis_displayed = self.edit_analysis_observed.clone();
                self.record_edit(job.before.clone());
                self.current_time_sec = current;
                if playing {
                    self.play();
                }
                self.status_toast = Some(("Channel edit applied; undo with Ctrl+Z".into(), now));
            }
            Err(TryRecvError::Empty) => {
                self.channel_tools.edit_job = Some(job);
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            result => {
                self.status_toast = Some((
                    format!("Could not apply channel edit: {result:?}").into(),
                    now,
                ))
            }
        }
    }

    fn poll_derived_signals(&mut self, ctx: &egui::Context) {
        let Some(source) = self.edited_waveform.as_ref().or(self.analysis.as_ref()) else {
            return;
        };
        let Some(right) = source.ch2_samples.as_ref() else {
            return;
        };
        let same = self
            .channel_tools
            .source_samples
            .as_ref()
            .is_some_and(|[left, old_right]| {
                Arc::ptr_eq(left, &source.ch1_samples) && Arc::ptr_eq(old_right, right)
            });
        if !same {
            self.channel_tools.derived_job = None;
            let left = Arc::clone(&source.ch1_samples);
            let right = Arc::clone(right);
            self.channel_tools.source_samples = Some([left.clone(), right.clone()]);
            let source = Arc::clone(source);
            let (ticket, receiver) = BackgroundJobs::global()
                .submit(Duration::MAX, move || build_channel_signals(&source));
            self.channel_tools.derived_job = Some(DerivedSignalsJob { ticket, receiver });
        }
        if let Some(job) = self.channel_tools.derived_job.take() {
            match job.receiver.try_recv() {
                Ok(Ok(derived)) => self.channel_tools.derived = Some(derived),
                Err(TryRecvError::Empty) => {
                    self.channel_tools.derived_job = Some(job);
                    ctx.request_repaint_after(Duration::from_millis(30));
                }
                _ => self.channel_tools.source_samples = None,
            }
        }
    }
}

/// Routes each complete edited stereo frame before emitting either channel.
/// Both speakers receive the selected sum or difference; routing changes are atomic.
pub(in crate::audio_preview) struct ChannelMonitorSource<S> {
    inner: S,
    mode: Arc<AtomicU64>,
    pending: Option<f32>,
}

impl<S> ChannelMonitorSource<S> {
    pub(in crate::audio_preview) fn new(inner: S, mode: Arc<AtomicU64>) -> Self {
        Self {
            inner,
            mode,
            pending: None,
        }
    }
}

impl<S: Source> Iterator for ChannelMonitorSource<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.inner.channels().get() != 2 {
            return self.inner.next();
        }
        if let Some(sample) = self.pending.take() {
            return Some(sample);
        }
        let left = self.inner.next()?;
        let right = self.inner.next()?;
        let mode = self.mode.load(Ordering::Acquire);
        let output = match mode {
            1 => left + right,
            2 => left - right,
            3 => right - left,
            _ => left,
        };
        self.pending = Some(if mode == 0 { right } else { output });
        Some(output)
    }
}

impl<S: Source> Source for ChannelMonitorSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner
            .current_span_len()
            .map(|n| n + usize::from(self.pending.is_some()))
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        self.pending = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

mod canvas;

mod spectra;
