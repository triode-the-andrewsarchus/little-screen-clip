//! Compact drum rack, selected-pad editor, and reference-pack browser.

use super::ConstellationState;
use super::sample_pack::play_slot_sample;
use egui::{Pos2, Rect, RichText, Stroke, Vec2};
use std::path::Path;
use std::time::{Duration, Instant};

impl ConstellationState {
    /// Triggers configured pads or assigns the selected sample with Shift+key.
    ///
    /// Uses unshifted logical characters and each event's modifiers. Text editors retain
    /// their keys; other focused controls do not block the pads. Consumes handled
    /// presses before other file manager navigation handlers receive them.
    pub fn handle_shelf_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        let native = egui_winit::take_logical_key_presses(ctx);
        let editing_text = ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if editing_text || self.browser_open || self.focus_search {
            return;
        }
        if ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::Y)
                || input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z)
        }) {
            self.undo_bank(true);
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Z)) {
            self.undo_bank(false);
            return;
        }
        let presses: Vec<_> = if native.is_empty() {
            // Also support synthetic egui input. Native input above retains full
            // logical punctuation, including apostrophes absent from egui::Key.
            ctx.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| {
                        let egui::Event::Key {
                            key,
                            physical_key,
                            pressed: true,
                            modifiers,
                            ..
                        } = event
                        else {
                            return None;
                        };
                        let index = self
                            .pad_keys
                            .iter()
                            .position(|character| {
                                egui::Key::from_name(&character.to_string()) == Some(*key)
                            })
                            .or_else(|| {
                                modifiers
                                    .shift
                                    .then(|| {
                                        self.pad_keys.iter().position(|character| {
                                            physical_key.is_some()
                                                && egui::Key::from_name(&character.to_string())
                                                    == *physical_key
                                        })
                                    })
                                    .flatten()
                            });
                        index.map(|index| (index, *modifiers, Some(*key)))
                    })
                    .collect()
            })
        } else {
            native
                .iter()
                .filter_map(|press| {
                    self.pad_keys
                        .iter()
                        .position(|key| *key == press.character.to_ascii_lowercase())
                        .map(|index| (index, press.modifiers, press.egui_key))
                })
                .collect()
        };
        let presses: Vec<_> = presses
            .into_iter()
            .filter(|(_, modifiers, _)| {
                !(modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd)
            })
            .collect();
        ctx.input_mut(|input| input.events.retain(|event| {
            !matches!(event, egui::Event::Key { key, modifiers, pressed: true, .. } if presses.iter().any(|(_, handled_modifiers, handled_key)| *handled_key == Some(*key) && handled_modifiers == modifiers))
        }));
        let actions = presses
            .into_iter()
            .map(|(index, modifiers, _)| (index, modifiers.shift));
        for (slot_idx, assign) in actions {
            if assign {
                if let Some(sample) = self
                    .samples
                    .iter()
                    .find(|sample| Some(&sample.file_path) == self.selected_path.as_ref())
                    .cloned()
                {
                    self.add_sample_to_slot(slot_idx, &sample);
                    self.slot_flash[slot_idx] = Some(Instant::now());
                }
            } else {
                self.trigger_slot(slot_idx);
            }
        }
    }

    /// Renders the sixteen-pad rack and only the selected pad's layer controls.
    pub fn render_pack_shelf(&mut self, ui: &mut egui::Ui, current_folder: &Path) {
        self.render_history_controls(ui);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 110.0).max(40.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.pack_bank.name)
                    .desired_width(width)
                    .hint_text("Pack name"),
            );
            if ui
                .add_enabled(!self.writing_pack(), egui::Button::new("Save pack"))
                .clicked()
            {
                let root = self
                    .current_indexed_folder
                    .as_deref()
                    .unwrap_or(current_folder);
                self.save_bank(root.to_path_buf(), self.pack_bank.clone());
            }
        });
        ui.add_space(4.0);
        self.render_pad_grid(ui);
        ui.add_space(4.0);
        if !self.pack_bank.slots[self.selected_slot].is_empty() {
            let label = if self.pad_editor_open {
                "Hide pad editor".to_string()
            } else {
                format!("Edit pad {}", self.selected_slot + 1)
            };
            if ui.button(label).clicked() {
                self.pad_editor_open = !self.pad_editor_open;
            }
        }
        // Errors and persistence completions remain visible below the rack.
        egui::ScrollArea::vertical()
            .id_source("selected_pad_editor")
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                if let Some((message, time)) = &self.browser_status
                    && time.elapsed() < Duration::from_secs(5)
                {
                    ui.separator();
                    ui.label(message);
                }
            });
    }

    fn render_pad_grid(&mut self, ui: &mut egui::Ui) {
        let gap = 5.0;
        let width = ((ui.available_width() - gap * 3.0) / 4.0).max(24.0);
        for row in (0..4).rev() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for col in 0..4 {
                    let index = row * 4 + col;
                    let slot = &self.pack_bank.slots[index];
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 44.0), egui::Sense::click());
                    let flashing = self.slot_flash[index].is_some_and(|time| time.elapsed() < Duration::from_millis(150));
                    if flashing { ui.ctx().request_repaint_after(Duration::from_millis(16)); }
                    let painter = ui.painter_at(rect);
                    let visuals = ui.visuals();
                    let fill = if flashing || response.hovered() { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.weak_bg_fill };
                    let stroke = if self.selected_slot == index { Stroke::new(1.5_f32, visuals.text_color()) } else { visuals.widgets.noninteractive.bg_stroke };
                    painter.rect(rect, 4.0, fill, stroke);
                    let key = self.pad_keys[index].to_ascii_uppercase().to_string();
                    painter.text(rect.min + Vec2::new(7.0, 5.0), egui::Align2::LEFT_TOP, key, egui::FontId::monospace(11.0), visuals.text_color());
                    painter.text(rect.right_top() + Vec2::new(-6.0, 6.0), egui::Align2::RIGHT_TOP, format!("{}", 36 + index), egui::FontId::monospace(9.0), visuals.weak_text_color());
                    let title = slot.samples.first().map_or("—", |sample| sample.filename.as_str());
                    let label = super::ui::short_label(title, ((width - 12.0) / 5.5).max(3.0) as usize);
                    painter.text(rect.left_bottom() + Vec2::new(7.0, -9.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(10.0), visuals.text_color());
                    if let Some(category) = slot.samples.first().and_then(|sample| sample.category) {
                        painter.line_segment([rect.left_bottom() + Vec2::new(5.0, -3.0), rect.right_bottom() + Vec2::new(-5.0, -3.0)], Stroke::new(2.0_f32, category.color()));
                    }
                    let response = response.on_hover_text(format!("Pad {} · MIDI {}\n{}\nClick to select and play. Shift+click to assign the map selection.", index + 1, 36 + index, slot.display_title()));
                    if response.clicked() {
                        self.selected_slot = index;
                        if ui.input(|input| input.modifiers.shift) { self.assign_selected_to_pad(index); }
                        else { self.trigger_slot(index); }
                    }
                }
            });
            if row != 0 {
                ui.add_space(gap - ui.spacing().item_spacing.y.min(gap));
            }
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new(format!(
                "{} · Shift = assign",
                self.pad_keys
                    .chunks(4)
                    .map(|row| row
                        .iter()
                        .map(|key| key.to_ascii_uppercase())
                        .collect::<String>())
                    .collect::<Vec<_>>()
                    .join(" / ")
            ))
            .size(10.0)
            .color(ui.visuals().weak_text_color()),
        );
    }

    fn assign_selected_to_pad(&mut self, index: usize) {
        if let Some(sample) = self
            .samples
            .iter()
            .find(|sample| Some(&sample.file_path) == self.selected_path.as_ref())
            .cloned()
        {
            self.add_sample_to_slot(index, &sample);
        }
    }

    pub(super) fn render_rack_output(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("MIDI input", |ui| {
                if self.midi.is_none() {
                    match super::midi::Controller::new(ui.ctx().clone()) {
                        Ok(controller) => self.midi = Some(controller),
                        Err(error) => {
                            ui.label(error);
                        }
                    }
                }
                if let Some(controller) = &self.midi {
                    let status = controller.status();
                    if ui.button("Refresh inputs").clicked() {
                        controller.refresh();
                    }
                    if ui
                        .selectable_label(status.connected.is_none(), "Off")
                        .clicked()
                    {
                        controller.disconnect();
                    }
                    for (index, name) in status.ports.iter().enumerate() {
                        if ui
                            .selectable_label(status.connected == Some(index), name)
                            .clicked()
                        {
                            controller.connect(index, name.clone());
                            ui.close_menu();
                        }
                    }
                    if status.ports.is_empty() {
                        ui.weak("No MIDI inputs found");
                    }
                    if let Some(error) = status.error {
                        ui.label(error);
                    }
                    ui.separator();
                    ui.weak("Notes 36–51 · all channels · one-shot pads");
                }
            });
            ui.toggle_value(&mut self.pack_bank.override_velocity, "Override velocity")
                .on_hover_text(
                    "Force all hits to MIDI velocity 127. Keyboard and mouse hits already use 127.",
                );
            ui.separator();
            ui.label("Glue");
            ui.toggle_value(&mut self.pack_bank.compressor_bypass, "Bypass").on_hover_text("Bypass the stereo pad compressor and peak ceiling. Pad input headroom stays unchanged.");
            let reduction = self.output.as_ref().map_or(0.0, |output| output.reduction_db());
            ui.weak(format!("{reduction:.1} dB reduction"));
        });
    }

    pub(super) fn render_selected_pad(&mut self, ui: &mut egui::Ui) {
        let index = self.selected_slot;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Pad {}", index + 1));
            let group = &mut self.pack_bank.slots[index].choke_group;
            egui::ComboBox::from_id_source(("pad_choke", index)).width(100.0).selected_text(if *group == 0 { "Choke: Off".to_string() } else { format!("Choke: {group}") }).show_ui(ui, |ui| {
                ui.selectable_value(group, 0, "Off");
                for value in 1..=8 { ui.selectable_value(group, value, format!("Group {value}")); }
            }).response.on_hover_text("Pads in the same group cut each other off; useful for open and closed hi-hats.");
            if ui.add_enabled(self.selected_path.is_some(), egui::Button::new("Add selected")).clicked() { self.assign_selected_to_pad(index); }
            if ui.small_button("Clear").clicked() { self.clear_slot(index); }
        });
        let slot = &mut self.pack_bank.slots[index];
        if slot.samples.is_empty() {
            ui.weak("Select a sample on the map, then Add selected or Shift+click a pad.");
        }
        let mut remove = None;
        let mut audition = None;
        for (layer, sample) in slot.samples.iter_mut().enumerate() {
            ui.push_id((index, layer), |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(super::ui::short_label(&sample.filename, 28)).strong())
                        .on_hover_text(&sample.filename);
                });
                Self::render_layer_waveform(ui, sample);
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Play").clicked() {
                        audition = Some(sample.clone());
                    }
                    if ui.small_button("Reset trim").clicked() {
                        sample.start_sec = 0.0;
                        sample.end_sec = sample.duration;
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(layer);
                    }
                    ui.add(
                        egui::Slider::new(&mut sample.volume, 0.0..=2.0)
                            .text("Gain")
                            .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                    );
                });
                ui.separator();
            });
        }
        if let Some(layer) = remove {
            slot.samples.remove(layer);
        }
        if let Some(sample) = audition
            && let Some(player) = play_slot_sample(&sample)
        {
            self.active_players.push(player);
        }
    }

    fn render_layer_waveform(ui: &mut egui::Ui, sample: &mut super::sample_pack::SlotSample) {
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 48.0),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let duration = sample.duration.max(0.01);
        let start = rect.left() + sample.start_sec / duration * rect.width();
        let end = rect.left() + sample.end_sec / duration * rect.width();
        if let Some(audio) = &sample.cached_audio {
            let color = sample
                .category
                .map_or(ui.visuals().text_color(), |category| category.color());
            for (index, &(low, high)) in audio.waveform_peaks.iter().enumerate() {
                let x = rect.left()
                    + index as f32 / audio.waveform_peaks.len().max(1) as f32 * rect.width();
                let color = if x >= start && x <= end {
                    color
                } else {
                    ui.visuals().weak_text_color()
                };
                painter.line_segment(
                    [
                        Pos2::new(x, rect.center().y - high.abs() * 20.0),
                        Pos2::new(x, rect.center().y + low.abs() * 20.0),
                    ],
                    Stroke::new(1.0_f32, color),
                );
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Preparing audio…",
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }
        let marker = ui.visuals().text_color();
        for x in [start, end] {
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.5_f32, marker),
            );
            painter.rect_filled(
                Rect::from_center_size(Pos2::new(x, rect.top() + 4.0), Vec2::new(6.0, 8.0)),
                1.0,
                marker,
            );
        }
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            ui.data_mut(|data| {
                data.insert_temp(
                    response.id.with("trim_start"),
                    (pointer.x - start).abs() <= (pointer.x - end).abs(),
                )
            });
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let time = ((pointer.x - rect.left()) / rect.width() * duration).clamp(0.0, duration);
            let time = sample.snap_time(time);
            if ui
                .data(|data| data.get_temp::<bool>(response.id.with("trim_start")))
                .unwrap_or(true)
            {
                sample.start_sec = time.clamp(0.0, (sample.end_sec - 0.005).max(0.0));
            } else {
                sample.end_sec = time.clamp((sample.start_sec + 0.005).min(duration), duration);
            }
        }
        ui.weak(format!(
            "{:.3}s – {:.3}s · drag edges to trim",
            sample.start_sec, sample.end_sec
        ));
    }

    /// Browses saved reference packs in the rack's side panel, keeping the map visible.
    pub fn render_pack_browser(&mut self, ui: &mut egui::Ui, current_folder: &Path) {
        ui.horizontal(|ui| {
            ui.strong("Saved packs");
            if ui.small_button("Refresh").clicked() {
                self.refresh_browser_packs(current_folder);
            }
        });
        if self.browser_packs.is_empty() {
            ui.weak("No saved packs here yet. Save a pack from the Rack tab.");
        }
        let mut preview = None;
        egui::ScrollArea::vertical()
            .id_source("saved_pack_list")
            .max_height(150.0)
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                for (index, summary) in self.browser_packs.iter().enumerate() {
                    let folder = summary
                        .folder_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    if ui
                        .selectable_label(self.browser_selected_idx == Some(index), folder.as_ref())
                        .clicked()
                    {
                        self.browser_selected_idx = Some(index);
                        preview = Some(summary.folder_path.clone());
                    }
                }
            });
        if let Some(path) = preview {
            self.stop_playback();
            self.browser_preview = None;
            self.load_browser_bank(path, false);
        }
        ui.separator();
        let Some(summary) = self
            .browser_selected_idx
            .and_then(|index| self.browser_packs.get(index))
            .cloned()
        else {
            ui.weak("Select a pack to inspect its pads or load it into the rack.");
            return;
        };
        ui.strong(&summary.name);
        ui.weak(format!(
            "{} pads · {} samples",
            summary.slot_count, summary.sample_count
        ));
        ui.horizontal_wrapped(|ui| {
            if ui.button("Load pack").clicked() {
                self.load_browser_bank(summary.folder_path.clone(), true);
            }
            if ui
                .add_enabled(!self.writing_pack(), egui::Button::new("Save new version"))
                .on_hover_text("Save the current rack as a new version of this pack")
                .clicked()
            {
                let mut bank = self.pack_bank.clone();
                bank.name = summary.name.clone();
                let root = self
                    .current_indexed_folder
                    .as_deref()
                    .unwrap_or(current_folder);
                self.save_bank(root.to_path_buf(), bank);
            }
            if ui
                .add_enabled(!self.writing_pack(), egui::Button::new("Delete"))
                .clicked()
            {
                self.delete_bank(summary.folder_path.clone());
            }
        });
        let mut audition = None;
        egui::ScrollArea::vertical()
            .id_source("saved_pack_preview")
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                if let Some((_, bank)) = self
                    .browser_preview
                    .as_ref()
                    .filter(|(path, _)| path == &summary.folder_path)
                {
                    for (index, slot) in bank
                        .slots
                        .iter()
                        .enumerate()
                        .filter(|(_, slot)| !slot.is_empty())
                    {
                        ui.horizontal(|ui| {
                            if ui.small_button(format!("Play {}", index + 1)).clicked() {
                                audition = Some(slot.samples.clone());
                            }
                            ui.label(slot.display_title());
                        });
                    }
                } else {
                    ui.spinner();
                }
                if let Some((message, _)) = &self.browser_status {
                    ui.label(message);
                }
            });
        if let Some(samples) = audition {
            self.stop_playback();
            for sample in samples {
                if let Some(player) = play_slot_sample(&sample) {
                    self.active_players.push(player);
                }
            }
        }
    }
}
