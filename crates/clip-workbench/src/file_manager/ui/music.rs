use super::super::BrowserView;
use super::*;

#[derive(Clone, PartialEq)]
pub(in crate::file_manager) struct MusicRange {
    pub path: PathBuf,
    pub start: f32,
    pub end: f32,
    pub revision: u64,
    pub source: crate::file_source::FileSource,
    pub snapped: bool,
}
pub(in crate::file_manager) fn waveform_id() -> Id {
    Id::new("files_music_waveform_focus")
}

impl FileManager {
    pub(in crate::file_manager) fn activate_media_view(&mut self) {
        let count = self.directory.as_ref().map_or(0, |d| {
            self.visible
                .iter()
                .filter(|i| d.entries[**i].regular_file)
                .count()
        });
        let automatic = if count > 0
            && self.audio_entries.len() * 100 >= count * self.music_threshold as usize
        {
            BrowserView::Music
        } else if count > 0 && self.video_count * 100 >= count * 75 {
            BrowserView::Videos
        } else {
            BrowserView::List
        };
        let view = self
            .location()
            .and_then(|l| l.manual_view)
            .unwrap_or(automatic);
        if self.view != view {
            if matches!(view, BrowserView::List | BrowserView::Videos) {
                self.stop_previews();
            }
            self.retire_media_previews();
            if view == BrowserView::Music {
                self.shelf.open = false;
            }
        }
        self.view = view;
    }
    pub(in crate::file_manager) fn copy_music_range(&mut self) {
        if let Some(range) = &self.music_selection {
            self.music.copy_range(range.clone());
        }
    }
    fn music_toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Title · Artist · Album · Duration");
            if ui
                .add(
                    egui::Slider::new(&mut self.music_threshold, 1..=100)
                        .suffix("%")
                        .text("Auto Music threshold"),
                )
                .changed()
            {
                self.activate_media_view();
            }
            if ui.button("Use automatic view").clicked() {
                if let Some(location) = self.location_mut() {
                    location.manual_view = None;
                }
                self.activate_media_view();
            }
        });
        let range = self.music_selection.clone();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(range.is_some(), egui::Button::new("Copy audio selection"))
                .clicked()
            {
                if let Some(range) = &range {
                    self.music.copy_range(range.clone());
                }
            }
            if ui.button("Paste as file").clicked() {
                self.paste_files();
            }
            ui.add(
                egui::Label::new(range.as_ref().map_or_else(
                    || "No audio selected".into(),
                    |range| {
                        format!(
                            "Selected {:.2}–{:.2} s",
                            range.start.min(range.end),
                            range.start.max(range.end)
                        )
                    },
                ))
                .truncate(true),
            );
        });
        if self.music.busy() {
            ui.spinner();
        }
        if let Some(status) = self.music.status() {
            ui.add(egui::Label::new(status).truncate(true));
        }
        ui.label("Click an audio strip to play or seek. Space plays the selected range; Escape stops playback. Selection edges snap to zero crossings. Ctrl/Shift+click selects files. Drag to select audio; Ctrl+C copies the selection, Ctrl+V pastes into this folder.");
    }
    pub(super) fn music_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        // Register the shared waveform focus target every frame so egui retains it
        // after the pointer leaves the per-file waveform widget.
        ui.interact(
            ui.available_rect_before_wrap(),
            waveform_id(),
            Sense::focusable_noninteractive(),
        );
        self.music_toolbar(ui);
        self.music_previews.poll(ui.ctx());
        let scroll = self.list_scroll(108.0);
        let Some(directory) = &self.directory else {
            ui.label("Loading folder…");
            return;
        };
        let selected = current_selection(&self.history, self.index);
        let mut actions = ListActions::default();
        let can_edit = self.can_edit();
        let count = self.selection_count();
        let background = actions.background(ui, can_edit, count > 0);
        let mut targets = Vec::new();
        let mut paths = std::collections::BTreeSet::new();
        let mut play = None;
        let mut seek = None;
        let output = media_rows(ui, scroll, &self.music_offsets, |ui, index, rect| {
            let entry = &directory.entries[self.visible[index]];
            let mut response = media_row(
                ui,
                rect,
                selected.is_some_and(|s| s.contains(&entry.path)),
                colors,
            );
            let is_audio =
                entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path);
            let ready = if is_audio && paths.len() < 128 {
                paths.insert(entry.path.clone());
                self.music_previews.music(ui.ctx(), entry)
            } else {
                None
            };
            let (engaged, time) = preview_row(
                ui,
                rect,
                entry,
                None,
                ready,
                &mut response,
                &mut self.music_selection,
                &self.music,
                colors,
            );
            if !engaged {
                actions.row(ui, &response, entry, selected, can_edit, count);
            } else {
                actions.selection = Some((entry.path.clone(), false, false));
            }
            if entry.directory {
                targets.push((rect.intersect(ui.clip_rect()), entry.path.clone()));
            }
            if let Some(time) = time {
                play = Some(entry.path.clone());
                seek = Some(time);
            }
        });
        self.music_previews.retain(&paths);
        if ui.is_enabled() {
            self.drop_targets.extend(targets);
        }
        self.finish_listing(ui, actions, output.state.offset.y, &background);
        if let Some(path) = play {
            self.midi.stop();
            if self.music.current() != Some(&path) {
                self.music.play(&path);
            }
            if let Some(time) = seek {
                self.music.seek(time);
            }
        }
    }
}

pub(super) fn preview_row(
    ui: &mut Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    columns: Option<super::ListColumns>,
    ready: Option<&super::super::shelf::preview::Ready>,
    response: &mut egui::Response,
    selection: &mut Option<MusicRange>,
    music: &super::super::music::Music,
    colors: WorkbenchColors,
) -> (bool, Option<f32>) {
    let range = selection
        .as_ref()
        .filter(|range| range.path == entry.path && range.revision == entry.revision);
    let wave = music_card(ui, rect, entry, columns, ready, colors, range);
    if let Some(ready) = ready
        && !super::popup_open(ui.ctx())
    {
        response.clone().on_hover_text(&ready.data.detail);
    }
    wave.map_or((false, None), |wave| {
        interact_waveform(ui, entry, wave, response, selection, music, colors)
    })
}

fn interact_waveform(
    ui: &mut Ui,
    entry: &super::super::filesystem::Entry,
    wave: (Rect, f32, &crate::file_source::FileSource),
    row_response: &mut egui::Response,
    selection: &mut Option<MusicRange>,
    music: &super::super::music::Music,
    colors: WorkbenchColors,
) -> (bool, Option<f32>) {
    let (rect, duration, source) = wave;
    if duration <= 0.0 {
        return (false, None);
    }
    let interaction = ui.interact(
        rect,
        Id::new(("music_wave", &entry.path)),
        Sense::click_and_drag(),
    );
    paint_range(
        ui,
        rect,
        duration,
        selection
            .as_ref()
            .filter(|s| s.path == entry.path && s.revision == entry.revision),
        colors,
    );
    if music.current() == Some(&entry.path) {
        let x = rect.left() + music.position().clamp(0.0, duration) / duration * rect.width();
        ui.painter().line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            egui::Stroke::new(2.0_f32, colors.accent),
        );
    }
    if ui.input(|input| input.modifiers.ctrl || input.modifiers.command || input.modifiers.shift) {
        // Keep the waveform hit target stable and forward its response to the
        // row, even when modifiers changed in the same frame as the click.
        *row_response = row_response.union(interaction);
        return (false, None);
    }
    let time_at =
        |point: egui::Pos2| ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * duration;
    let mut seek = None;
    if let Some(point) = interaction.interact_pointer_pos() {
        let time = time_at(point);
        if interaction.clicked() {
            seek = Some(time);
        }
        if interaction.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).map_or(time, time_at);
            *selection = Some(MusicRange {
                path: entry.path.clone(),
                start: origin,
                end: time,
                revision: entry.revision,
                source: source.clone(),
                snapped: false,
            });
        }
        if interaction.dragged()
            && let Some(range) = selection
        {
            range.end = time;
            range.snapped = false;
        }
    }
    let engaged =
        interaction.clicked() || interaction.dragged() || interaction.is_pointer_button_down_on();
    if engaged {
        ui.memory_mut(|m| m.request_focus(waveform_id()));
    }
    (engaged, seek)
}

fn music_card<'a>(
    ui: &Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    columns: Option<super::ListColumns>,
    ready: Option<&'a super::super::shelf::preview::Ready>,
    colors: WorkbenchColors,
    selection: Option<&MusicRange>,
) -> Option<(Rect, f32, &'a crate::file_source::FileSource)> {
    let audio = entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path);
    let art = Rect::from_min_size(
        rect.min + vec2(6.0, if audio { 32.0 } else { 4.0 }),
        if audio {
            vec2(
                (rect.height() - 42.0).max(1.0),
                (rect.height() - 42.0).max(1.0),
            )
        } else {
            vec2(20.0, 20.0)
        },
    );
    if let Some(texture) = ready.and_then(|r| r.texture.as_ref()) {
        ui.painter().image(
            texture.id(),
            art,
            Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else if audio {
        paint_audio_fallback(
            ui,
            art,
            entry,
            ready.and_then(|r| r.data.music.as_ref()),
            colors,
        );
    } else {
        entry_icon(entry).paint(ui.painter(), art, colors.text_secondary);
    }
    let metadata = ready.and_then(|r| r.data.music.as_ref());
    let left = if audio { 6.0 } else { 32.0 };
    let text = Rect::from_min_size(
        rect.min + vec2(left, 0.0),
        vec2((rect.width() - left - 6.0).max(1.0), 24.0),
    );
    let details = metadata.map(|m| {
        let mut fields: Vec<&str> = [&m.title, &m.artist, &m.album]
            .into_iter()
            .map(String::as_str)
            .filter(|text| !text.is_empty())
            .collect();
        let duration = format!("{:.0} s", m.duration);
        fields.push(&duration);
        fields.join(" · ")
    });
    media_header(ui, text, entry, details, columns, colors);
    if let Some(metadata) = metadata {
        let wave = Rect::from_min_max(
            pos2(art.right() + 8.0, art.top()),
            pos2(rect.right() - 6.0, art.bottom()),
        );
        ui.painter().rect_filled(wave, 0.0, colors.recessed);
        if let Some(ready) = ready {
            if let Some(texture) = &ready.spectrum_texture {
                ui.painter().image(
                    texture.id(),
                    wave,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                if ready.data.waveform.is_empty() {
                    paint_text(
                        ui,
                        wave,
                        0.0,
                        wave.width(),
                        "Audio strip unavailable",
                        colors.text_secondary,
                    );
                }
                let count = ready.data.waveform.len().max(1);
                for (index, amplitude) in ready.data.waveform.iter().enumerate() {
                    let x = wave.left() + index as f32 / count as f32 * wave.width();
                    let height = amplitude * wave.height() * 0.5;
                    ui.painter().line_segment(
                        [
                            pos2(x, wave.center().y - height),
                            pos2(x, wave.center().y + height),
                        ],
                        egui::Stroke::new(
                            1.0_f32,
                            if selection.is_some_and(|range| {
                                let time = (index as f32 + 0.5) / count as f32 * metadata.duration;
                                time >= range.start.min(range.end)
                                    && time <= range.start.max(range.end)
                            }) {
                                colors.accent
                            } else {
                                ready
                                    .data
                                    .waveform_colors
                                    .get(index)
                                    .map_or(colors.text_secondary, |rgb| {
                                        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                                    })
                            },
                        ),
                    );
                }
            }
        }
        return metadata
            .source
            .as_ref()
            .map(|s| (wave, metadata.duration, s));
    }
    None
}

fn paint_audio_fallback(
    ui: &Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    metadata: Option<&super::super::media::MusicMetadata>,
    colors: WorkbenchColors,
) {
    let extension = entry
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let icon = match extension.as_str() {
        "wav" | "wave" => crate::icons::Icon::AudioWav,
        "flac" => crate::icons::Icon::AudioFlac,
        "aiff" | "aif" => crate::icons::Icon::AudioAiff,
        "aac" => crate::icons::Icon::AudioAac,
        "ogg" => crate::icons::Icon::AudioOgg,
        "oga" => crate::icons::Icon::AudioOga,
        "m4a" | "alac" => crate::icons::Icon::AudioM4a,
        "wma" => crate::icons::Icon::AudioWma,
        _ => crate::icons::Icon::Audio,
    };
    let quality = audio_quality(&extension, metadata);
    let tint = quality.map_or(colors.text_secondary, |quality| quality.color());
    ui.painter().rect_filled(
        rect,
        4.0,
        quality.map_or(colors.recessed, |quality| quality.background()),
    );
    icon.paint(ui.painter(), rect.shrink(rect.width() * 0.20), tint);

    if let Some(sample_rate) = metadata.and_then(|metadata| metadata.sample_rate) {
        let mark = if sample_rate < 44_100 {
            Some(crate::icons::Icon::ClefBass)
        } else if sample_rate <= 48_000 {
            None
        } else if sample_rate <= 96_000 {
            Some(crate::icons::Icon::ClefAlto)
        } else if sample_rate <= 192_000 {
            Some(crate::icons::Icon::ClefTreble)
        } else {
            Some(crate::icons::Icon::Bat)
        };
        if let Some(mark) = mark {
            let side = (rect.width() * 0.30).clamp(12.0, 20.0);
            let badge = Rect::from_min_size(
                rect.right_bottom() - egui::vec2(side, side),
                egui::vec2(side, side),
            );
            ui.painter().rect_filled(badge, side * 0.2, colors.panel);
            mark.paint(ui.painter(), badge.shrink(2.0), colors.text_primary);
        }
    }
}

#[derive(Clone, Copy)]
enum AudioQuality {
    Bronze,
    Silver,
    Gold,
}

impl AudioQuality {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Bronze => egui::Color32::from_rgb(198, 130, 82),
            Self::Silver => egui::Color32::from_rgb(192, 204, 218),
            Self::Gold => egui::Color32::from_rgb(242, 194, 76),
        }
    }

    fn background(self) -> egui::Color32 {
        let color = self.color();
        egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 44)
    }
}

fn audio_quality(
    extension: &str,
    metadata: Option<&super::super::media::MusicMetadata>,
) -> Option<AudioQuality> {
    let metadata = metadata?;
    if matches!(extension, "wav" | "wave" | "aiff" | "aif" | "flac" | "alac") {
        return match metadata.bit_depth? {
            depth if depth >= 24 => Some(AudioQuality::Gold),
            depth if depth >= 16 => Some(AudioQuality::Silver),
            _ => Some(AudioQuality::Bronze),
        };
    }
    match metadata.bit_rate? / 1000 {
        bitrate if bitrate >= 256 => Some(AudioQuality::Gold),
        bitrate if bitrate >= 128 => Some(AudioQuality::Silver),
        _ => Some(AudioQuality::Bronze),
    }
}

fn paint_range(
    ui: &Ui,
    rect: Rect,
    duration: f32,
    selection: Option<&MusicRange>,
    colors: WorkbenchColors,
) {
    if duration <= 0.0 {
        return;
    }
    if let Some(range) = selection {
        let start = range.start.min(range.end).clamp(0.0, duration) / duration;
        let end = range.start.max(range.end).clamp(0.0, duration) / duration;
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(rect.left() + start * rect.width(), rect.top()),
                pos2(rect.left() + end * rect.width(), rect.bottom()),
            ),
            0.0,
            colors.accent.gamma_multiply(0.25),
        );
    }
}
