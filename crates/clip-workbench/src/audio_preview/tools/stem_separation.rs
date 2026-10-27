use super::super::*;
use crate::icons::Icon;

pub(crate) fn parse_stem_manifest(stdout: &[u8]) -> Result<Vec<(String, PathBuf)>, String> {
    let text = String::from_utf8_lossy(stdout);
    let json = text
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .ok_or_else(|| "Stem worker returned no result manifest.".to_string())?;
    let stems = ["vocals", "drums", "bass", "other"]
        .iter()
        .filter_map(|name| {
            json.get(name)
                .and_then(|path| path.as_str())
                .map(|path| ((*name).to_string(), PathBuf::from(path)))
        })
        .collect::<Vec<_>>();
    if stems.len() == 4 {
        Ok(stems)
    } else {
        Err("Stem worker did not return vocals, drums, bass, and other.".to_string())
    }
}

pub(crate) fn stem_is_audible(index: usize, muted: bool, solo_stem: Option<usize>) -> bool {
    !muted && solo_stem.map_or(true, |solo_index| solo_index == index)
}

pub(crate) fn editor_stem_switch(
    current_index: usize,
    requested_index: usize,
    stem_count: usize,
) -> Option<(usize, usize)> {
    (requested_index < stem_count && requested_index != current_index)
        .then_some((current_index, requested_index))
}

pub(crate) fn waveform_scrub_time(
    pointer_x: f32,
    min_x: f32,
    width: f32,
    duration_sec: f32,
) -> f32 {
    ((pointer_x - min_x) / width.max(1.0)).clamp(0.0, 1.0) * duration_sec.max(0.0)
}

pub(crate) fn editor_is_audacity(editor: &str) -> bool {
    Path::new(editor)
        .file_stem()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("audacity"))
}

pub(crate) fn audacity_lof_contents(paths: &[PathBuf]) -> Result<String, String> {
    let mut contents = String::new();
    for path in paths {
        let path = path.to_string_lossy();
        if path.contains(['\n', '\r', '"']) {
            return Err(
                "A stem path contains characters Audacity cannot import safely.".to_string(),
            );
        }
        contents.push_str(&format!("file \"{path}\"\n"));
    }
    Ok(contents)
}

pub(crate) fn prepare_isolated_audacity_command(
    command: &mut std::process::Command,
    workspace: &Path,
    stem_paths: &[PathBuf],
) -> Result<(), String> {
    let profile_root = workspace.join("audacity-profile");
    let home_dir = profile_root.join("home");
    let config_home = profile_root.join("config");
    let data_home = profile_root.join("data");
    let cache_home = profile_root.join("cache");
    let empty_plugins = profile_root.join("empty-plugins");
    let audacity_config_dir = config_home.join("audacity");

    for directory in [
        &home_dir,
        &audacity_config_dir,
        &data_home,
        &cache_home,
        &empty_plugins,
    ] {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create isolated Audacity profile: {error}"))?;
    }

    std::fs::write(
        audacity_config_dir.join("audacity.cfg"),
        "[GUI]\nShowSplashScreen=0\n[Effects]\nSkipEffectsScanAtStartup=1\n",
    )
    .map_err(|error| format!("Could not configure isolated Audacity profile: {error}"))?;

    let lof_path = workspace.join("separated-stems.lof");
    std::fs::write(&lof_path, audacity_lof_contents(stem_paths)?)
        .map_err(|error| format!("Could not create Audacity stem project: {error}"))?;

    command
        .arg(lof_path)
        .env("HOME", &home_dir)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CACHE_HOME", &cache_home);
    for variable in [
        "VST_PATH",
        "VST3_PATH",
        "LV2_PATH",
        "LADSPA_PATH",
        "VAMP_PATH",
        "CLAP_PATH",
    ] {
        command.env(variable, &empty_plugins);
    }
    Ok(())
}

mod editor;
mod group_playback;
mod save;
mod worker;
use worker::{StemOutput, StemWorkerCommand, run_stem_worker};

pub(crate) struct SeparatedStem {
    pub(crate) name: String,
    pub(crate) preview: AudioPreviewState,
    pub(crate) group_player: Option<Player>,
    pub(crate) muted: bool,
}

#[derive(Default)]
pub(crate) struct StemSeparationState {
    pub(crate) in_progress: bool,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    result_rx: Option<Receiver<Result<Result<Vec<StemOutput>, String>, JobError>>>,
    job_ticket: Option<JobTicket>,
    process_control: Option<Arc<crate::image_preview::ImageToolProcessControl>>,
    retry_at: Option<Instant>,
    worker_command: Option<StemWorkerCommand>,
    group_load: Option<group_playback::GroupLoad>,
    pub(crate) stems: Vec<SeparatedStem>,
    pub(crate) selected_stem: usize,
    pub(crate) solo_stem: Option<usize>,
    pub(crate) workspace: Option<PathBuf>,
    pub(crate) save_dialog_open: bool,
    pub(crate) save_destination_dir: String,
    pub(crate) group_sink: Option<MixerDeviceSink>,
    pub(crate) group_is_playing: bool,
    pub(crate) group_current_time_sec: f32,
    pub(crate) editor_playhead_active: bool,
    pub(crate) group_playback_start: Option<Instant>,
    pub(crate) group_playback_offset_sec: f32,
}

impl AudioPreviewState {
    pub(crate) fn start_stem_separation(&mut self) {
        self.start_stem_separation_on(BackgroundJobs::global(), StemWorkerCommand::configured());
    }

    fn start_stem_separation_on(&mut self, jobs: &BackgroundJobs, worker: StemWorkerCommand) {
        if self.stem_separation.in_progress {
            return;
        }
        self.pause_group_playback();
        self.pause();
        let source = self.edit_snapshot();
        let filter = self.editor_render_filter();
        let workspace = self.stem_separation.workspace.clone().unwrap_or_else(|| {
            little_clip_core::ArtifactStore::new()
                .and_then(|store| store.create_workspace("stems", "separation"))
                .map(|workspace| workspace.retain())
                .unwrap_or_else(|_| {
                    std::env::temp_dir()
                        .join(format!("little-screen-clip-stems-{}", uuid::Uuid::new_v4()))
                })
        });
        let process_control = Arc::new(crate::image_preview::ImageToolProcessControl::new());
        let worker_control = Arc::clone(&process_control);
        let command = worker.clone();
        let worker_workspace = workspace.clone();
        let (ticket, rx) = jobs.submit(Duration::MAX, move || {
            run_stem_worker(command, source, filter, &worker_workspace, &worker_control)
        });
        self.stem_separation.in_progress = true;
        self.stem_separation.status = Some("Starting local HTDemucs v4 separation…".to_string());
        self.stem_separation.error = None;
        self.stem_separation.workspace = Some(workspace.clone());
        self.stem_separation.result_rx = Some(rx);
        self.stem_separation.job_ticket = Some(ticket);
        self.stem_separation.process_control = Some(process_control);
        self.stem_separation.worker_command = Some(worker);
        self.stem_separation.retry_at = None;
    }

    pub(crate) fn cancel_stem_separation(&mut self) {
        let was_active = self.stem_separation.in_progress;
        if let Some(ticket) = self.stem_separation.job_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.stem_separation.process_control.take() {
            control.cancel();
        }
        self.stem_separation.result_rx = None;
        self.stem_separation.retry_at = None;
        self.stem_separation.in_progress = false;
        if was_active {
            self.stem_separation.status = Some("Stem separation cancelled.".to_string());
        }
    }

    pub(crate) fn poll_stem_separation(&mut self, ctx: &egui::Context) {
        self.poll_stem_separation_on(ctx, BackgroundJobs::global());
    }

    fn poll_stem_separation_on(&mut self, ctx: &egui::Context, jobs: &BackgroundJobs) {
        if let Some(retry_at) = self.stem_separation.retry_at {
            if Instant::now() >= retry_at && !self.stem_separation.in_progress {
                if let Some(worker) = self.stem_separation.worker_command.clone() {
                    self.start_stem_separation_on(jobs, worker);
                }
            } else {
                ctx.request_repaint_after(retry_at.saturating_duration_since(Instant::now()));
            }
        }
        let mut message = None;
        if let Some(rx) = &self.stem_separation.result_rx {
            if self.stem_separation.in_progress {
                ctx.request_repaint();
            }
            match rx.try_recv() {
                Ok(Ok(stems)) => message = Some(Ok(stems)),
                Ok(Err(error)) => message = Some(Err(error)),
                Err(TryRecvError::Disconnected) => message = Some(Err(JobError::Cancelled)),
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(message) = message {
            self.stem_separation.in_progress = false;
            self.stem_separation.result_rx = None;
            self.stem_separation.job_ticket = None;
            self.stem_separation.process_control = None;
            self.stem_separation.retry_at = None;
            match message {
                Ok(Ok(stems)) => {
                    let editor = self.audio_editor.clone();
                    let volume = self.volume;
                    self.stem_separation.stems = stems
                        .into_iter()
                        .map(|output| {
                            let name = output.name;
                            let group_player = crate::ui_ext::new_shared_audio_player();
                            if let Some(ref p) = group_player {
                                p.set_volume(volume);
                            }
                            let mut preview = AudioPreviewState::new_with_metadata_checked(
                                output.identity.path().to_owned(),
                                format!("{} stem — {}", name.to_uppercase(), self.title),
                                output.metadata,
                                output.check,
                            )
                            .with_file_source(output.identity)
                            .with_editor(editor.clone());
                            preview.set_studio_volume(volume);
                            SeparatedStem {
                                preview,
                                name,
                                group_player,
                                muted: false,
                            }
                        })
                        .collect();
                    self.stem_separation.group_sink = None;
                    self.stem_separation.group_current_time_sec = 0.0;
                    self.stem_separation.group_is_playing = false;
                    self.stem_separation.selected_stem = 0;
                    self.stem_separation.editor_playhead_active = false;
                    self.stem_separation.status =
                        Some("HTDemucs created four temporary stereo stems.".to_string());
                }
                Ok(Err(error)) => {
                    self.stem_separation.status = None;
                    self.stem_separation.error = Some(error);
                }
                Err(JobError::QueueFull) => {
                    self.stem_separation.retry_at =
                        Some(Instant::now() + Duration::from_millis(500));
                    self.stem_separation.status = Some(
                        "Stem separation is waiting for background workers; retrying shortly"
                            .to_string(),
                    );
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
                Err(JobError::Cancelled) => {
                    self.stem_separation.status = Some("Stem separation cancelled.".to_string())
                }
                Err(error) => {
                    self.stem_separation.status = None;
                    self.stem_separation.error = Some(format!("HTDemucs worker failed: {error}"))
                }
            }
        }
    }

    pub(crate) fn select_editor_stem(&mut self, requested_index: usize) {
        let Some((previous_index, next_index)) = editor_stem_switch(
            self.stem_separation.selected_stem,
            requested_index,
            self.stem_separation.stems.len(),
        ) else {
            return;
        };
        if let Some(previous) = self.stem_separation.stems.get_mut(previous_index) {
            previous.preview.stop();
        }
        self.stem_separation.selected_stem = next_index;
    }

    pub(crate) fn render_stem_waveform(
        ui: &mut egui::Ui,
        preview: Option<&AudioPreviewState>,
        palette: &AppPalette,
        muted: bool,
        playhead_sec: f32,
        duration_sec: f32,
    ) -> Option<f32> {
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 58.0),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        let bg = hex_to_c32(&palette.colors.surface_recessed);
        let accent = if muted {
            hex_to_c32(&palette.colors.text_muted)
        } else {
            hex_to_c32(&palette.harmonies.base)
        };
        painter.rect_filled(rect, Rounding::same(4.0), bg);
        painter.rect_stroke(rect, Rounding::same(4.0), Stroke::new(1.0_f32, accent));
        let Some(preview) = preview else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Waveform will appear after separation",
                egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                accent,
            );
            return response
                .interact_pointer_pos()
                .filter(|_| response.clicked() || response.dragged())
                .map(|pos| waveform_scrub_time(pos.x, rect.min.x, rect.width(), duration_sec));
        };
        let Some(analysis) = preview.analysis.as_ref() else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Decoding waveform…",
                egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                accent,
            );
            return None;
        };
        let mid = rect.center().y;
        let width = rect.width().max(1.0);
        let columns = width as usize;
        for x in 0..columns {
            let frame_start = x * analysis.total_frames / columns;
            let frame_end = ((x + 1) * analysis.total_frames / columns).max(frame_start + 1);
            let (left_min, left_max, rgb) = analysis.query_peaks(0, frame_start, frame_end);
            let (right_min, right_max, _) = analysis.query_peaks(1, frame_start, frame_end);
            let (min, max) = if analysis.channels > 1 {
                ((left_min + right_min) * 0.5, (left_max + right_max) * 0.5)
            } else {
                (left_min, left_max)
            };
            let color = if muted {
                accent
            } else {
                Color32::from_rgb(rgb[0], rgb[1], rgb[2])
            };
            painter.line_segment(
                [
                    Pos2::new(
                        rect.min.x + x as f32,
                        mid - max.clamp(-1.0, 1.0) * rect.height() * 0.42,
                    ),
                    Pos2::new(
                        rect.min.x + x as f32,
                        mid - min.clamp(-1.0, 1.0) * rect.height() * 0.42,
                    ),
                ],
                Stroke::new(1.0_f32, color),
            );
        }
        let playhead_x =
            rect.min.x + (playhead_sec / duration_sec.max(0.001)).clamp(0.0, 1.0) * rect.width();
        painter.line_segment(
            [
                Pos2::new(playhead_x, rect.min.y),
                Pos2::new(playhead_x, rect.max.y),
            ],
            Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.complementary)),
        );
        response
            .interact_pointer_pos()
            .filter(|_| response.clicked() || response.dragged())
            .map(|pos| waveform_scrub_time(pos.x, rect.min.x, rect.width(), duration_sec))
    }

    pub(crate) fn render_stem_separation_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        self.poll_stem_separation(ctx);
        let now = ctx.input(|input| input.time);
        for stem in &mut self.stem_separation.stems {
            stem.preview.poll_audio_analysis(ctx, now);
        }
        self.update_group_playback(ctx);

        let viewport_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_source("stem_tool_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let content_start_y = ui.cursor().min.y;
                layout::AudioWorkspace::control_row(ui, palette, "Separation", |ui| {
                    if !self.stem_separation.stems.is_empty() {
                        if ui
                            .add(layout::AudioWorkspace::secondary_button_icon(
                                Some(Icon::RotateCw),
                                "Separate Again",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            ))
                            .clicked()
                        {
                            self.start_stem_separation();
                        }
                        if ui
                            .add(layout::AudioWorkspace::primary_button_icon(
                                Some(Icon::Save),
                                "Save All Stems…",
                                hex_to_c32(&palette.diff_add),
                                palette,
                            ))
                            .clicked()
                        {
                            self.stem_separation.save_destination_dir = self
                                .path
                                .parent()
                                .unwrap_or_else(|| Path::new("."))
                                .to_string_lossy()
                                .to_string();
                            self.stem_separation.save_dialog_open = true;
                        }
                    }
                    if let Some(status) = &self.stem_separation.status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(hex_to_c32(&palette.diff_add)),
                        );
                    }
                });
                if let Some(error) = &self.stem_separation.error {
                    ui.label(
                        egui::RichText::new(error)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.diff_del)),
                    );
                }

                ui.add_space(5.0);
                if self.stem_separation.stems.is_empty() {
                    for name in ["VOCALS", "DRUMS", "BASS", "OTHER"] {
                        egui::Frame::none()
                            .fill(hex_to_c32(&palette.colors.surface_panel))
                            .inner_margin(egui::Margin::symmetric(10.0, 5.0))
                            .rounding(Rounding::same(5.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(name).strong());
                                let _ = Self::render_stem_waveform(
                                    ui,
                                    None,
                                    palette,
                                    false,
                                    0.0,
                                    self.duration_sec,
                                );
                            });
                        ui.add_space(3.0);
                    }
                } else {
                    let duration = self.group_duration();
                    layout::AudioWorkspace::control_row(ui, palette, "All stems", |ui| {
                        let (play_label, play_icon) = if self.stem_separation.group_is_playing {
                            ("Pause All", Icon::Pause)
                        } else {
                            ("Play All", Icon::Play)
                        };
                        if ui
                            .add(layout::AudioWorkspace::primary_button_icon(
                                Some(play_icon),
                                play_label,
                                if self.stem_separation.group_is_playing {
                                    hex_to_c32(&palette.diff_mod)
                                } else {
                                    hex_to_c32(&palette.diff_add)
                                },
                                palette,
                            ))
                            .clicked()
                        {
                            if self.stem_separation.group_is_playing {
                                self.pause_group_playback();
                            } else {
                                self.start_group_playback();
                            }
                        }
                        if ui
                            .add(layout::AudioWorkspace::secondary_button_icon(
                                Some(Icon::Stop),
                                "Stop All",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            ))
                            .clicked()
                        {
                            self.stop_group_playback();
                        }
                        ui.label(
                    egui::RichText::new(
                        "Click or drag any waveform to select its stem and scrub all tracks",
                    )
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_secondary)),
                );
                    });

                    let mut selected = None;
                    let mut scrubbed = None;
                    let mut routing_changed = false;
                    let playhead = self.stem_separation.group_current_time_sec;
                    let solo_stem = self.stem_separation.solo_stem;
                    for (index, stem) in self.stem_separation.stems.iter_mut().enumerate() {
                        let selected_now = self.stem_separation.selected_stem == index;
                        let effectively_muted = !stem_is_audible(index, stem.muted, solo_stem);
                        egui::Frame::none()
                            .fill(if selected_now {
                                hex_to_c32_alpha(&palette.harmonies.base, 18)
                            } else {
                                hex_to_c32(&palette.colors.surface_panel)
                            })
                            .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                            .stroke(Stroke::new(
                                if selected_now { 2.0_f32 } else { 1.0_f32 },
                                if selected_now {
                                    hex_to_c32(&palette.harmonies.base)
                                } else {
                                    hex_to_c32(&palette.colors.surface_panel)
                                },
                            ))
                            .rounding(Rounding::same(5.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(layout::AudioWorkspace::toggle_button(
                                            &stem.name.to_uppercase(),
                                            selected_now,
                                            palette,
                                        ))
                                        .clicked()
                                    {
                                        selected = Some(index);
                                    }
                                    if ui
                                        .add(layout::AudioWorkspace::toggle_button(
                                            "Mute", stem.muted, palette,
                                        ))
                                        .clicked()
                                    {
                                        stem.muted = !stem.muted;
                                        routing_changed = true;
                                    }
                                    if ui
                                        .add(layout::AudioWorkspace::toggle_button(
                                            "Solo",
                                            solo_stem == Some(index),
                                            palette,
                                        ))
                                        .clicked()
                                    {
                                        self.stem_separation.solo_stem = if solo_stem == Some(index)
                                        {
                                            None
                                        } else {
                                            Some(index)
                                        };
                                        routing_changed = true;
                                    }
                                });
                                if let Some(time) = Self::render_stem_waveform(
                                    ui,
                                    Some(&stem.preview),
                                    palette,
                                    effectively_muted,
                                    playhead,
                                    duration,
                                ) {
                                    selected = Some(index);
                                    scrubbed = Some(time);
                                }
                            });
                        ui.add_space(3.0);
                    }
                    if let Some(index) = selected {
                        self.select_editor_stem(index);
                    }
                    if let Some(time) = scrubbed {
                        self.seek_group_playback(time);
                    }
                    if routing_changed {
                        self.refresh_group_routing();
                    }

                    ui.separator();
                    let selected_index = self.stem_separation.selected_stem;
                    let editor_started =
                        if let Some(stem) = self.stem_separation.stems.get_mut(selected_index) {
                            ui.label(
                                egui::RichText::new(format!("Editing {} stem", stem.name)).strong(),
                            );
                            let content_height = ui.cursor().min.y - content_start_y;
                            // The embedded editor's allocation also consumes a row spacing.
                            let editor_height =
                                (viewport_height - content_height - ui.spacing().item_spacing.y)
                                    .max(0.0);
                            let was_playing = stem.preview.is_playing;
                            let previous_time = stem.preview.current_time_sec;
                            stem.preview.render_standard_ui(
                                ui,
                                ctx,
                                palette,
                                close_requested,
                                Some(editor_height),
                            );
                            if was_playing != stem.preview.is_playing
                                || previous_time != stem.preview.current_time_sec
                            {
                                self.stem_separation.editor_playhead_active = true;
                            }
                            !was_playing && stem.preview.is_playing
                        } else {
                            false
                        };
                    if editor_started {
                        self.pause_group_playback();
                    }
                }
            });

        if self.stem_separation.save_dialog_open {
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Save all separated stems")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Destination folder (four stereo WAV files will be copied here):");
                    ui.add_sized(
                        egui::vec2(ui.available_width(), crate::ui_ext::CONTROL_HEIGHT),
                        egui::TextEdit::singleline(&mut self.stem_separation.save_destination_dir)
                            .font(egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE)),
                    );
                    ui.horizontal(|ui| {
                        save = ui
                            .add(layout::AudioWorkspace::primary_button(
                                "Save all stems",
                                hex_to_c32(&palette.diff_add),
                                palette,
                            ))
                            .clicked();
                        cancel = ui
                            .add(layout::AudioWorkspace::secondary_button(
                                "Cancel",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            ))
                            .clicked();
                    });
                });
            if save {
                self.save_all_stems();
            }
            cancel |= ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if cancel {
                self.stem_separation.save_dialog_open = false;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("stems-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn fake_worker(directory: &Path, body: &str) -> StemWorkerCommand {
        let path = directory.join("fake-stem-worker.sh");
        let body = body.replace(
            "$2/descendant",
            &format!("{}/workspace/descendant", directory.display()),
        );
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        StemWorkerCommand {
            program: path,
            prefix_args: Vec::new(),
        }
    }

    fn manifest_worker(directory: &Path) -> StemWorkerCommand {
        fake_worker(
            directory,
            r#"for stem in vocals drums bass other; do cp "$1" "$2/$stem.wav"; done
printf '{"vocals":"vocals.wav","drums":"drums.wav","bass":"bass.wav","other":"other.wav"}\n'"#,
        )
    }

    fn test_state(path: &Path) -> AudioPreviewState {
        if !path.exists() {
            crate::audio_preview::tests::write_stereo_pcm_fixture(
                path,
                &[8192; 800],
                &[-4096; 800],
                8000,
            );
        }
        let mut state = AudioPreviewState::new(path.to_path_buf(), "stem-test.wav".into());
        state.cancel_session_analysis();
        let workspace = path.parent().unwrap().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        state.stem_separation.workspace = Some(workspace);
        state
    }

    fn wait_for_process_exit(pid: u32) {
        for _ in 0..200 {
            let running = unsafe { libc::kill(pid as i32, 0) == 0 };
            if !running {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("stem worker process {pid} was not reaped");
    }

    fn wait_for_stem_terminal(state: &mut AudioPreviewState, jobs: &BackgroundJobs) {
        let ctx = egui::Context::default();
        for _ in 0..500 {
            state.poll_stem_separation_on(&ctx, jobs);
            if !state.stem_separation.in_progress {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("stem worker did not finish within five seconds");
    }

    #[test]
    fn stem_preparation_rejects_expired_and_replaced_sources() {
        for changed in [false, true] {
            let directory = test_dir("checked-input");
            let input = directory.join("input.wav");
            let mut state = test_state(&input);
            state.file_source = Some(crate::file_source::FileSource::capture(&input).unwrap());
            if changed {
                std::fs::write(&input, b"replacement").unwrap();
            } else {
                state.read_check =
                    crate::file_source::ReadCheck::new(|_| Err("source context expired".into()));
            }
            let jobs = BackgroundJobs::new(1, 1);
            state.start_stem_separation_on(
                &jobs,
                fake_worker(&directory, "echo must-not-run >&2; exit 9"),
            );
            wait_for_stem_terminal(&mut state, &jobs);
            let error = state.stem_separation.error.as_deref().unwrap();
            assert!(
                error.contains(if changed {
                    "source changed"
                } else {
                    "source context expired"
                }),
                "{error}"
            );
            assert!(state.stem_separation.stems.is_empty());
            assert_eq!(
                std::fs::read_dir(state.stem_separation.workspace.as_ref().unwrap())
                    .unwrap()
                    .count(),
                0
            );
            drop(state);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn separated_stems_own_output_lifetime_and_reject_external_manifests() {
        let directory = test_dir("owned-output");
        let input = directory.join("input.wav");
        let state = test_state(&input);
        let control = crate::image_preview::ImageToolProcessControl::new();
        let outputs = run_stem_worker(
            manifest_worker(&directory),
            state.edit_snapshot(),
            state.editor_render_filter(),
            state.stem_separation.workspace.as_ref().unwrap(),
            &control,
        )
        .unwrap();
        let identity = outputs[0].identity.clone();
        let check = outputs[0].check.clone();
        let output = identity.path().to_owned();
        let job = output.parent().unwrap().to_owned();
        assert_eq!(
            std::fs::metadata(&job).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_file(&input).unwrap();
        drop(outputs);
        check
            .open(&output, Some(&identity), &AtomicBool::new(false))
            .unwrap();
        assert!(job.is_dir());
        drop(check);
        assert!(
            !job.exists(),
            "the final read-context owner must release the private job"
        );
        drop(state);
        let state = test_state(&input);
        let malformed = fake_worker(
            &directory,
            r#"printf '{"vocals":"/etc/passwd","drums":"../drums.wav","bass":"bass.wav","other":"other.wav"}\n'"#,
        );
        let error = run_stem_worker(
            malformed,
            state.edit_snapshot(),
            state.editor_render_filter(),
            state.stem_separation.workspace.as_ref().unwrap(),
            &control,
        )
        .err()
        .unwrap();
        assert!(error.contains("outside its job directory"));
        assert_eq!(
            std::fs::read_dir(state.stem_separation.workspace.as_ref().unwrap())
                .unwrap()
                .count(),
            0
        );
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn save_all_stems_copies_declared_files_and_closes_dialog() {
        let directory = test_dir("save-all");
        let destination = directory.join("chosen-output");
        let mut state = test_state(&directory.join("source_clip.wav"));
        state.stem_separation.save_destination_dir = destination.to_string_lossy().into_owned();
        state.stem_separation.save_dialog_open = true;

        let stems = [
            ("vocals", b"synthetic vocals".as_slice()),
            ("drums", b"synthetic drums".as_slice()),
            ("bass", b"synthetic bass".as_slice()),
            ("other", b"synthetic other".as_slice()),
        ];
        for (name, contents) in stems {
            let source = directory.join(format!("{name}-source.wav"));
            std::fs::write(&source, contents).unwrap();
            state.stem_separation.stems.push(SeparatedStem {
                name: name.to_owned(),
                preview: test_state(&source),
                group_player: None,
                muted: false,
            });
        }

        state.save_all_stems();
        assert!(state.file_write_pending());
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.file_write_pending() {
            assert!(Instant::now() < deadline);
            state.poll_save_job(&egui::Context::default(), 0.0);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!state.stem_separation.save_dialog_open);
        assert!(
            state
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .text
                .contains("Saved 4 file(s)")
        );
        assert!(state.stem_separation.error.is_none());
        for (name, contents) in stems {
            let output = destination.join(format!("source_clip_{name}.wav"));
            assert_eq!(std::fs::read(output).unwrap(), contents);
        }
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stem_job_queue_saturation_retries_and_imports_fake_manifest() {
        let directory = test_dir("queue");
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_, queued_rx) = jobs.submit(Duration::from_secs(5), || ());
        let mut state = test_state(&directory.join("input.wav"));
        state.start_stem_separation_on(&jobs, manifest_worker(&directory));
        let ctx = egui::Context::default();
        for _ in 0..100 {
            state.poll_stem_separation_on(&ctx, &jobs);
            if state.stem_separation.retry_at.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(state.stem_separation.retry_at.is_some());
        assert!(state.stem_separation.error.is_none());
        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        queued_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        state.stem_separation.retry_at = Some(Instant::now());
        state.poll_stem_separation_on(&ctx, &jobs);
        wait_for_stem_terminal(&mut state, &jobs);
        assert_eq!(state.stem_separation.stems.len(), 4);
        assert_eq!(state.stem_separation.stems[0].name, "vocals");
        assert!(state.stem_separation.retry_at.is_none());
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stem_worker_failure_is_reported() {
        let directory = test_dir("failure");
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = test_state(&directory.join("input.wav"));
        state.start_stem_separation_on(
            &jobs,
            fake_worker(&directory, "echo fake failure >&2\nexit 7"),
        );
        wait_for_stem_terminal(&mut state, &jobs);
        assert!(
            state
                .stem_separation
                .error
                .as_deref()
                .unwrap()
                .contains("fake failure")
        );
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completed_stems_do_not_scroll_when_the_editor_fits_the_viewport() {
        let (_fixture, path) =
            crate::audio_preview::tests::generate_fixture_wav("stem-layout", 0.25);
        let analysis =
            Arc::new(little_clip_core::audio::compute_full_audio_analysis(&path, 100).unwrap());
        for height in [1000.0, 1050.0, 1100.0, 1200.0, 1600.0] {
            let mut state = test_state(&path);
            state.duration_sec = 0.25;
            for name in ["vocals", "drums", "bass", "other"] {
                let mut preview = test_state(&path);
                preview.analysis = Some(Arc::clone(&analysis));
                preview.is_loading_analysis = false;
                preview.duration_sec = 0.25;
                state.stem_separation.stems.push(SeparatedStem {
                    name: name.to_owned(),
                    preview,
                    group_player: None,
                    muted: false,
                });
            }
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let mut scroll_id = egui::Id::NULL;
            for events in [
                Vec::new(),
                Vec::new(),
                vec![
                    egui::Event::PointerMoved(Pos2::new(30.0, 30.0)),
                    egui::Event::Scroll(Vec2::new(0.0, -100.0)),
                ],
            ] {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(1180.0, height),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            scroll_id = ui.make_persistent_id(egui::Id::new("stem_tool_scroll"));
                            state.render_stem_separation_ui(ui, ctx, &palette, &mut false);
                        });
                    },
                );
            }
            let scroll = egui::scroll_area::State::load(&ctx, scroll_id).unwrap();
            assert_eq!(
                scroll.offset.y, 0.0,
                "completed stems overflow a {height}-point viewport"
            );
        }
    }

    #[test]
    fn studio_readout_follows_source_group_and_selected_stem_playheads() {
        let directory = test_dir("header-playhead");
        let mut state = test_state(&directory.join("input.wav"));
        state.duration_sec = 100.0;
        state.current_time_sec = 12.0;
        for &mode in AudioToolMode::ALL {
            state.active_mode = mode;
            assert_eq!(state.studio_playhead(), (12.0, 100.0));
        }
        let mut preview = test_state(&directory.join("vocals.wav"));
        preview.duration_sec = 90.0;
        preview.current_time_sec = 42.0;
        state.stem_separation.stems.push(SeparatedStem {
            name: "vocals".to_owned(),
            preview,
            group_player: None,
            muted: false,
        });
        state.active_mode = AudioToolMode::StemSeparation;
        state.stem_separation.group_current_time_sec = 24.0;
        assert_eq!(state.studio_playhead(), (24.0, 100.0));
        state.stem_separation.editor_playhead_active = true;
        assert_eq!(state.studio_playhead(), (42.0, 90.0));
        state.seek_group_playback(30.0);
        assert_eq!(state.studio_playhead(), (30.0, 100.0));
        state.active_mode = AudioToolMode::Inspect;
        assert_eq!(state.studio_playhead(), (12.0, 100.0));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn entering_stem_tab_starts_once_and_restarts_after_cancellation() {
        let directory = test_dir("auto-start");
        let mut state = test_state(&directory.join("input.wav"));
        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let starts = std::cell::Cell::new(0);
        let render = |state: &mut AudioPreviewState, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui_with_start(ui, ctx, &palette, &mut false, |state| {
                            starts.set(starts.get() + 1);
                            state.stem_separation.in_progress = true;
                        });
                    });
                },
            )
        };
        let output = render(&mut state, Vec::new());
        let tab_position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.text() == AudioToolMode::StemSeparation.label() =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("Stem Separation tab is visible");
        for pressed in [true, false] {
            let _ = render(
                &mut state,
                vec![
                    egui::Event::PointerMoved(tab_position),
                    egui::Event::PointerButton {
                        pos: tab_position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(state.active_mode, AudioToolMode::StemSeparation);
        assert!(state.stem_separation.in_progress);
        let _ = render(&mut state, Vec::new());
        assert_eq!(starts.get(), 1, "ordinary frames do not restart separation");
        // Keyboard tab switches use the same lifecycle as mouse clicks.
        for key in [egui::Key::I, egui::Key::S] {
            let _ = render(
                &mut state,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        assert_eq!(
            starts.get(),
            2,
            "returning after cancellation restarts separation"
        );
        state.cancel_stem_separation();
        state.stem_separation.stems.push(SeparatedStem {
            name: "vocals".to_owned(),
            preview: test_state(&directory.join("vocals.wav")),
            group_player: None,
            muted: false,
        });
        for key in [egui::Key::I, egui::Key::S] {
            let _ = render(
                &mut state,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        assert_eq!(starts.get(), 2, "completed stems are retained on re-entry");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn leaving_stem_tab_cancels_worker_process_group() {
        let directory = test_dir("cancel");
        let jobs = BackgroundJobs::new(1, 1);
        let script = format!(
            "(sleep 1; touch \"$2/descendant-finished\") &\necho $! > \"$2/descendant.pid\"\nwait"
        );
        let mut state = test_state(&directory.join("input.wav"));
        state.active_mode = AudioToolMode::StemSeparation;
        state.start_stem_separation_on(&jobs, fake_worker(&directory, &script));
        let control = state
            .stem_separation
            .process_control
            .as_ref()
            .unwrap()
            .clone();
        let descendant_pid_path = state
            .stem_separation
            .workspace
            .as_ref()
            .unwrap()
            .join("descendant.pid");
        for _ in 0..200 {
            if descendant_pid_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(descendant_pid_path.exists());
        let direct_pid = control.child_pid.load(Ordering::Acquire);
        assert_ne!(direct_pid, 0);
        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut close = false;
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut close);
                    });
                },
            )
        };
        let output = render(Vec::new());
        let tab_position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.text() == AudioToolMode::Inspect.label() =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("Inspect tab is visible");
        let _ = render(vec![
            egui::Event::PointerMoved(tab_position),
            egui::Event::PointerButton {
                pos: tab_position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let _ = render(vec![egui::Event::PointerButton {
            pos: tab_position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(state.active_mode, AudioToolMode::Inspect);
        assert!(!state.stem_separation.in_progress);
        assert!(state.stem_separation.retry_at.is_none());
        assert!(control.cancel.load(Ordering::Acquire));
        wait_for_process_exit(direct_pid);
        std::thread::sleep(Duration::from_millis(1100));
        assert!(
            !state
                .stem_separation
                .workspace
                .as_ref()
                .unwrap()
                .join("descendant-finished")
                .exists()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dropping_audio_viewer_cancels_stem_worker_and_descendants() {
        let directory = test_dir("drop");
        let jobs = BackgroundJobs::new(1, 1);
        let script =
            "(sleep 1; touch \"$2/descendant-finished\") &\necho $! > \"$2/descendant.pid\"\nwait";
        let mut state = test_state(&directory.join("input.wav"));
        state.start_stem_separation_on(&jobs, fake_worker(&directory, script));
        let control = state
            .stem_separation
            .process_control
            .as_ref()
            .unwrap()
            .clone();
        let workspace = state.stem_separation.workspace.as_ref().unwrap().clone();
        for _ in 0..200 {
            if workspace.join("descendant.pid").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(workspace.join("descendant.pid").exists());
        let direct_pid = control.child_pid.load(Ordering::Acquire);
        assert_ne!(direct_pid, 0);
        drop(state);
        assert!(control.cancel.load(Ordering::Acquire));
        wait_for_process_exit(direct_pid);
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!workspace.join("descendant-finished").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn escape_during_stem_separation_closes_and_cancels_worker_without_changing_source() {
        let directory = test_dir("escape-running");
        let source = directory.join("input.wav");
        crate::audio_preview::tests::write_stereo_pcm_fixture(
            &source,
            &[8192; 800],
            &[-4096; 800],
            8000,
        );
        let original_bytes = std::fs::read(&source).unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let script =
            "(sleep 1; touch \"$2/descendant-finished\") &\necho $! > \"$2/descendant.pid\"\nwait";
        let mut state = test_state(&source);
        state.active_mode = AudioToolMode::StemSeparation;
        state.start_stem_separation_on(&jobs, fake_worker(&directory, script));
        let control = state
            .stem_separation
            .process_control
            .as_ref()
            .unwrap()
            .clone();
        let workspace = state.stem_separation.workspace.as_ref().unwrap().clone();
        for _ in 0..200 {
            if workspace.join("descendant.pid").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(workspace.join("descendant.pid").exists());
        let direct_pid = control.child_pid.load(Ordering::Acquire);
        assert_ne!(direct_pid, 0);

        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: Some(egui::Key::Escape),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        assert!(close, "Escape requests closing the running Stems viewer");
        assert!(state.stem_separation.in_progress);

        drop(state);
        assert!(control.cancel.load(Ordering::Acquire));
        wait_for_process_exit(direct_pid);
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!workspace.join("descendant-finished").exists());
        assert_eq!(std::fs::read(source).unwrap(), original_bytes);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
