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

#[derive(Debug)]
pub(crate) enum StemSeparationMsg {
    Success(Vec<(String, PathBuf)>),
    Error(String),
}

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
    pub(crate) result_rx: Option<Receiver<StemSeparationMsg>>,
    pub(crate) stems: Vec<SeparatedStem>,
    pub(crate) selected_stem: usize,
    pub(crate) solo_stem: Option<usize>,
    pub(crate) workspace: Option<PathBuf>,
    pub(crate) save_dialog_open: bool,
    pub(crate) save_destination_dir: String,
    pub(crate) group_sink: Option<MixerDeviceSink>,
    pub(crate) group_is_playing: bool,
    pub(crate) group_current_time_sec: f32,
    pub(crate) group_playback_start: Option<Instant>,
    pub(crate) group_playback_offset_sec: f32,
}

impl AudioPreviewState {
    pub(crate) fn start_stem_separation(&mut self) {
        if self.stem_separation.in_progress {
            return;
        }
        let source = self.path.clone();
        let workspace = little_clip_core::ArtifactStore::new()
            .and_then(|store| store.create_workspace("stems", "separation"))
            .map(|workspace| workspace.retain())
            .unwrap_or_else(|_| {
                std::env::temp_dir()
                    .join(format!("little-screen-clip-stems-{}", uuid::Uuid::new_v4()))
            });
        let (tx, rx) = channel();
        self.stem_separation.in_progress = true;
        self.stem_separation.status = Some("Starting local HTDemucs v4 separation…".to_string());
        self.stem_separation.error = None;
        self.stem_separation.workspace = Some(workspace.clone());
        self.stem_separation.result_rx = Some(rx);
        std::thread::spawn(move || {
            let worker =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/separate_stems.py");
            let venv_python = std::env::var("HOME")
                .map(|home| {
                    PathBuf::from(home).join(".local/share/little-screen-clip/nobg-venv/bin/python")
                })
                .ok()
                .filter(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from("python3"));
            let output = std::process::Command::new(&venv_python)
                .arg(&worker)
                .arg(&source)
                .arg(&workspace)
                .output();
            match output {
                Ok(result) if result.status.success() => {
                    match parse_stem_manifest(&result.stdout) {
                        Ok(stems) => {
                            let _ = tx.send(StemSeparationMsg::Success(stems));
                        }
                        Err(error) => {
                            let stderr = String::from_utf8_lossy(&result.stderr);
                            let _ = tx.send(StemSeparationMsg::Error(format!(
                                "Could not read stem worker output: {error}\n{}",
                                stderr.trim()
                            )));
                        }
                    }
                }
                Ok(result) => {
                    let message = String::from_utf8_lossy(&result.stderr).trim().to_string();
                    let _ = tx.send(StemSeparationMsg::Error(if message.is_empty() {
                        "HTDemucs separation failed.".to_string()
                    } else {
                        message
                    }));
                }
                Err(error) => {
                    let _ = tx.send(StemSeparationMsg::Error(format!(
                        "Could not launch Python stem worker: {error}"
                    )));
                }
            }
        });
    }

    pub(crate) fn poll_stem_separation(&mut self, ctx: &egui::Context) {
        let mut message = None;
        if let Some(rx) = &self.stem_separation.result_rx {
            ctx.request_repaint();
            if let Ok(result) = rx.try_recv() {
                message = Some(result);
            }
        }
        if let Some(message) = message {
            self.stem_separation.in_progress = false;
            self.stem_separation.result_rx = None;
            match message {
                StemSeparationMsg::Success(stems) => {
                    let editor = self.audio_editor.clone();
                    let volume = self.volume;
                    self.stem_separation.stems = stems
                        .into_iter()
                        .map(|(name, path)| {
                            let group_player = crate::ui_ext::new_shared_audio_player();
                            if let Some(ref p) = group_player {
                                p.set_volume(volume);
                            }
                            let mut preview = AudioPreviewState::new(
                                path,
                                format!("{} stem — {}", name.to_uppercase(), self.title),
                            )
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
                    self.stem_separation.status =
                        Some("HTDemucs created four temporary stereo stems.".to_string());
                }
                StemSeparationMsg::Error(error) => self.stem_separation.error = Some(error),
            }
        }
    }

    pub(crate) fn group_duration(&self) -> f32 {
        self.stem_separation
            .stems
            .iter()
            .map(|stem| stem.preview.duration_sec)
            .fold(self.duration_sec, f32::max)
            .max(0.01)
    }

    pub(crate) fn load_group_stem(stem: &SeparatedStem, offset: f32) {
        let Some(player) = &stem.group_player else {
            return;
        };
        player.clear();
        if let Ok(file) = File::open(&stem.preview.path) {
            if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                player.append(decoder);
                let _ = player.try_seek(Duration::from_secs_f32(offset.max(0.0)));
                player.play();
            }
        }
    }

    pub(crate) fn start_group_playback(&mut self) {
        let duration = self.group_duration();
        if self.stem_separation.group_current_time_sec >= duration - 0.05 {
            self.stem_separation.group_current_time_sec = 0.0;
        }
        for stem in &mut self.stem_separation.stems {
            stem.preview.pause();
        }
        let offset = self.stem_separation.group_current_time_sec;
        let solo = self.stem_separation.solo_stem;
        for (index, stem) in self.stem_separation.stems.iter().enumerate() {
            if stem_is_audible(index, stem.muted, solo) {
                Self::load_group_stem(stem, offset);
            } else if let Some(player) = &stem.group_player {
                player.clear();
            }
        }
        self.stem_separation.group_playback_offset_sec = offset;
        self.stem_separation.group_playback_start = Some(Instant::now());
        self.stem_separation.group_is_playing = true;
    }

    pub(crate) fn update_group_playback(&mut self, ctx: &egui::Context) {
        if !self.stem_separation.group_is_playing {
            return;
        }
        ctx.request_repaint();
        if let Some(started) = self.stem_separation.group_playback_start {
            let current =
                self.stem_separation.group_playback_offset_sec + started.elapsed().as_secs_f32();
            let duration = self.group_duration();
            if current >= duration {
                self.stop_group_playback();
                self.stem_separation.group_current_time_sec = duration;
            } else {
                self.stem_separation.group_current_time_sec = current;
            }
        }
    }

    pub(crate) fn pause_group_playback(&mut self) {
        if self.stem_separation.group_is_playing {
            if let Some(started) = self.stem_separation.group_playback_start {
                self.stem_separation.group_current_time_sec =
                    self.stem_separation.group_playback_offset_sec
                        + started.elapsed().as_secs_f32();
            }
        }
        for stem in &self.stem_separation.stems {
            if let Some(player) = &stem.group_player {
                player.pause();
            }
        }
        self.stem_separation.group_is_playing = false;
        self.stem_separation.group_playback_start = None;
    }

    pub(crate) fn stop_group_playback(&mut self) {
        for stem in &self.stem_separation.stems {
            if let Some(player) = &stem.group_player {
                player.stop();
            }
        }
        self.stem_separation.group_is_playing = false;
        self.stem_separation.group_playback_start = None;
        self.stem_separation.group_playback_offset_sec = 0.0;
        self.stem_separation.group_current_time_sec = 0.0;
    }

    pub(crate) fn seek_group_playback(&mut self, time_sec: f32) {
        let target = time_sec.clamp(0.0, self.group_duration());
        self.stem_separation.group_current_time_sec = target;
        self.stem_separation.group_playback_offset_sec = target;
        if self.stem_separation.group_is_playing {
            self.stem_separation.group_playback_start = Some(Instant::now());
            let solo = self.stem_separation.solo_stem;
            for (index, stem) in self.stem_separation.stems.iter().enumerate() {
                if stem_is_audible(index, stem.muted, solo) {
                    Self::load_group_stem(stem, target);
                }
            }
        }
    }

    pub(crate) fn refresh_group_routing(&mut self) {
        if !self.stem_separation.group_is_playing {
            return;
        }
        let target = self.stem_separation.group_current_time_sec;
        let solo = self.stem_separation.solo_stem;
        for (index, stem) in self.stem_separation.stems.iter().enumerate() {
            if stem_is_audible(index, stem.muted, solo) {
                Self::load_group_stem(stem, target);
            } else if let Some(player) = &stem.group_player {
                player.clear();
            }
        }
        self.stem_separation.group_playback_offset_sec = target;
        self.stem_separation.group_playback_start = Some(Instant::now());
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

    pub(crate) fn launch_stems_in_external_editor(
        &mut self,
        _ctx: &egui::Context,
        _close_requested: &mut bool,
    ) {
        let editor = if self.audio_editor.is_empty() {
            "audacity".to_string()
        } else {
            self.audio_editor.clone()
        };
        let stem_paths = self
            .stem_separation
            .stems
            .iter()
            .map(|stem| stem.preview.path.clone())
            .collect::<Vec<_>>();
        let mut command = std::process::Command::new(&editor);
        if editor_is_audacity(&editor) {
            let Some(workspace) = self.stem_separation.workspace.as_deref() else {
                self.stem_separation.error =
                    Some("The temporary stem workspace is no longer available.".to_string());
                return;
            };
            if let Err(error) =
                prepare_isolated_audacity_command(&mut command, workspace, &stem_paths)
            {
                self.stem_separation.error = Some(error);
                return;
            }
        } else {
            command.args(&stem_paths);
        }
        match command.spawn() {
            Ok(_) => {
                self.stop_group_playback();
                for stem in &mut self.stem_separation.stems {
                    stem.preview.stop();
                }
                self.status_toast = Some((
                    format!("🚀 Opened stems in {}", editor),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64(),
                ));
            }
            Err(error) => {
                self.stem_separation.error = Some(format!(
                    "Failed to open separated tracks in {editor}: {error}"
                ));
            }
        }
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

    pub(crate) fn save_all_stems(&mut self) {
        let destination = PathBuf::from(self.stem_separation.save_destination_dir.trim());
        if destination.as_os_str().is_empty() {
            self.stem_separation.error = Some("Choose a destination folder first.".to_string());
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&destination) {
            self.stem_separation.error = Some(format!("Cannot create destination folder: {error}"));
            return;
        }
        let base = self
            .path
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("audio");
        let mut saved = 0;
        for stem in &self.stem_separation.stems {
            let out = destination.join(format!("{base}_{}.wav", stem.name));
            if std::fs::copy(&stem.preview.path, out).is_ok() {
                saved += 1;
            }
        }
        if saved == self.stem_separation.stems.len() {
            self.stem_separation.status = Some(format!("Saved all {saved} stereo stems."));
            self.stem_separation.save_dialog_open = false;
        } else {
            self.stem_separation.error = Some(format!(
                "Saved {saved} of {} stems.",
                self.stem_separation.stems.len()
            ));
        }
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
                    if self.stem_separation.stems.is_empty() {
                        let (label, icon) = if self.stem_separation.in_progress {
                            ("Separating locally…", None)
                        } else {
                            ("Separate Stems", Some(Icon::Waves))
                        };
                        if ui
                            .add_enabled(
                                !self.stem_separation.in_progress,
                                layout::AudioWorkspace::primary_button_icon(
                                    icon,
                                    label,
                                    hex_to_c32(&palette.harmonies.base),
                                    palette,
                                ),
                            )
                            .clicked()
                        {
                            self.start_stem_separation();
                        }
                    } else {
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
                    if let Some(error) = &self.stem_separation.error {
                        ui.label(
                            egui::RichText::new(error)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(hex_to_c32(&palette.diff_del)),
                        );
                    }
                });

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
                        let current = self.stem_separation.group_current_time_sec;
                        ui.label(
                            egui::RichText::new(format!(
                                "{:02}:{:04.1} / {:02}:{:04.1}",
                                (current / 60.0) as u32,
                                current % 60.0,
                                (duration / 60.0) as u32,
                                duration % 60.0,
                            ))
                            .monospace()
                            .strong(),
                        );
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
                            let editor_height = (viewport_height - content_height).max(350.0);
                            let was_playing = stem.preview.is_playing;
                            stem.preview.render_standard_ui(
                                ui,
                                ctx,
                                palette,
                                close_requested,
                                Some(editor_height),
                            );
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
