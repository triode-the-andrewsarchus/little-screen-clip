//! Compact drum rack, selected-pad editor, and reference-pack browser.

use super::ConstellationState;
use egui::{Rect, RichText, Stroke, Vec2};
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
                self.playback.lock().unwrap().show_midi_notes = false;
                self.trigger_slot(slot_idx);
            }
        }
    }

    /// Renders the sixteen-pad rack and only the selected pad's layer controls.
    pub fn render_pack_shelf(&mut self, ui: &mut egui::Ui, current_folder: &Path) {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
        self.render_history_controls(ui);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 120.0).max(40.0);
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
        if ui
            .add(
                egui::Button::new("Override MIDI velocity")
                    .selected(self.pack_bank.override_velocity),
            )
            .clicked()
        {
            self.pack_bank.override_velocity = !self.pack_bank.override_velocity;
        }
        ui.add_space(4.0);
        self.render_pad_grid(ui);
        ui.add_space(4.0);
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
        self.pad_drop_targets.clear();
        if ui.input(|input| !input.modifiers.shift || input.key_pressed(egui::Key::Escape)) {
            self.choke_anchor = None;
        }
        let mut hovered = None;
        let (playing, show_midi_notes): ([bool; super::sample_pack::NUM_SLOTS], bool) = {
            let rack = self.playback.lock().unwrap();
            (
                std::array::from_fn(|index| {
                    rack.voices[index].iter().any(|player| !player.empty())
                }),
                rack.show_midi_notes,
            )
        };
        let gap = 5.0;
        let status_height = if self
            .browser_status
            .as_ref()
            .is_some_and(|(_, time)| time.elapsed() < Duration::from_secs(5))
        {
            28.0
        } else {
            0.0
        };
        let width =
            ((ui.available_width().min(ui.clip_rect().width()) - gap * 3.0) / 4.0).max(28.0);
        let pad_height =
            ((ui.available_height() - status_height - gap * 3.0 - 32.0) / 4.0).clamp(28.0, width);
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = pad_height;
            ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
            for row in (0..4).rev() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for col in 0..4 {
                        let index = row * 4 + col;
                        let slot = &self.pack_bank.slots[index];
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(width, pad_height),
                            egui::Sense::click_and_drag(),
                        );
                        let unavailable = slot
                            .samples
                            .iter()
                            .any(|sample| self.failed_audio.contains(&sample.file_path));
                        let loading = slot.samples.iter().any(|sample| {
                            sample.cached_audio.is_none()
                                && !self.failed_audio.contains(&sample.file_path)
                        });
                        let flashing = self.slot_flash[index]
                            .is_some_and(|time| time.elapsed() < Duration::from_millis(150));
                        if flashing {
                            ui.ctx().request_repaint_after(Duration::from_millis(16));
                        }
                        let painter = ui.painter_at(rect);
                        let visuals = ui.visuals().clone();
                        let fill = if flashing || response.hovered() {
                            visuals.widgets.hovered.bg_fill
                        } else {
                            visuals.widgets.inactive.weak_bg_fill
                        };
                        let stroke = if self.selected_slot == index {
                            Stroke::new(1.5_f32, visuals.text_color())
                        } else {
                            visuals.widgets.noninteractive.bg_stroke
                        };
                        painter.rect(rect, 4.0, fill, stroke);
                        let key = if show_midi_notes {
                            super::ui::freq_to_note_str(
                                440.0 * 2.0_f32.powf((36.0 + index as f32 - 69.0) / 12.0),
                            )
                        } else {
                            self.pad_keys[index].to_ascii_uppercase().to_string()
                        };
                        painter.text(
                            rect.min + Vec2::new(7.0, 4.0),
                            egui::Align2::LEFT_TOP,
                            key,
                            egui::FontId::monospace(if pad_height < 44.0 { 10.0 } else { 14.0 }),
                            visuals.text_color(),
                        );
                        Self::paint_pad_status(
                            ui,
                            rect,
                            slot,
                            unavailable,
                            loading,
                            playing[index],
                        );
                        let font_size = if pad_height >= 56.0 { 12.0 } else { 9.0 };
                        let line_height = font_size + 1.0;
                        let text_top = if pad_height < 44.0 { 15.0 } else { 22.0 };
                        let capacity = ((pad_height - text_top - 3.0) / line_height)
                            .floor()
                            .max(1.0) as usize;
                        let overflow = slot.samples.len() > capacity;
                        let visible = if overflow && capacity > 1 {
                            capacity - 1
                        } else {
                            capacity
                        };
                        let titles = slot
                            .samples
                            .iter()
                            .take(visible)
                            .map(|sample| sample.filename.as_str())
                            .chain((overflow && capacity > 1).then_some("..."));
                        for (line, title) in titles.enumerate() {
                            let limit = ((width - 12.0) / 5.0).max(3.0) as usize;
                            let label = if overflow && capacity == 1 {
                                format!(
                                    "{}...",
                                    super::ui::short_label(title, limit.saturating_sub(3))
                                )
                            } else {
                                super::ui::short_label(title, limit)
                            };
                            painter.text(
                                rect.left_top()
                                    + Vec2::new(7.0, text_top + line as f32 * line_height),
                                egui::Align2::LEFT_TOP,
                                label,
                                egui::FontId::proportional(font_size),
                                visuals.text_color(),
                            );
                        }
                        if let Some(category) =
                            slot.samples.first().and_then(|sample| sample.category)
                        {
                            painter.line_segment(
                                [
                                    rect.left_bottom() + Vec2::new(5.0, -2.0),
                                    rect.right_bottom() + Vec2::new(-5.0, -2.0),
                                ],
                                Stroke::new(2.0_f32, category.color()),
                            );
                        }
                        self.pad_drop_targets
                            .push((rect.intersect(ui.clip_rect()), index));
                        let native_hover =
                            self.pad_drop_hover.is_some_and(|pos| rect.contains(pos));
                        if response
                            .dnd_hover_payload::<crate::file_manager::transfers::FileDrag>()
                            .is_some()
                            || native_hover
                        {
                            painter.rect_stroke(
                                rect.shrink(1.0),
                                4.0,
                                Stroke::new(2.0_f32, visuals.selection.stroke.color),
                            );
                        }
                        if response.hovered() {
                            hovered = Some(index);
                        }
                        let title = self.pad_tooltip(
                            index,
                            unavailable,
                            loading,
                            playing[index],
                            ui.input(|input| input.modifiers.alt),
                        );
                        self.handle_pad_response(ui, index, &response, title);
                    }
                });
                if row != 0 {
                    ui.add_space(gap - ui.spacing().item_spacing.y.min(gap));
                }
            }
        });
        self.paint_mute_links(ui, hovered);
        ui.weak(self.mute_group_hint(hovered));
    }

    /// Keeps transient readiness and explicit pad flags out of the assignment logic.
    fn paint_pad_status(
        ui: &mut egui::Ui,
        rect: Rect,
        slot: &super::sample_pack::PackSlot,
        unavailable: bool,
        loading: bool,
        playing: bool,
    ) {
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        if slot.choke_group != 0 {
            painter.text(
                rect.right_top() + Vec2::new(-6.0, 6.0),
                egui::Align2::RIGHT_TOP,
                format!("G{}", slot.choke_group),
                egui::FontId::monospace(9.0),
                visuals.weak_text_color(),
            );
        }
        if slot.mute || slot.solo {
            painter.text(
                rect.min + Vec2::new(22.0, 6.0),
                egui::Align2::LEFT_TOP,
                if slot.mute { "M" } else { "S" },
                egui::FontId::monospace(9.0),
                visuals.weak_text_color(),
            );
        }
        let state_pos = rect.right_top() + Vec2::new(-28.0, 10.0);
        if unavailable {
            painter.circle_filled(state_pos, 3.0, visuals.error_fg_color);
        } else if loading {
            ui.put(
                Rect::from_center_size(state_pos, Vec2::splat(10.0)),
                egui::Spinner::new().size(10.0),
            );
        } else if playing {
            painter.circle_filled(state_pos, 3.0, visuals.selection.stroke.color);
        }
    }

    fn pad_tooltip(
        &self,
        index: usize,
        unavailable: bool,
        loading: bool,
        playing: bool,
        detailed: bool,
    ) -> Option<String> {
        let slot = &self.pack_bank.slots[index];
        if !detailed {
            return (slot.choke_group == 0).then(|| {
                format!(
                    "{} · MIDI {}\n{}\nDouble-click: edit · Shift+right-click: link · Alt: details",
                    self.pad_keys[index].to_ascii_uppercase(),
                    36 + index,
                    slot.samples
                        .first()
                        .map_or("Empty pad", |sample| sample.filename.as_str())
                )
            });
        }
        let mut title = slot.display_title();
        if unavailable {
            title.push_str("\nSome sources are unavailable or could not be decoded.");
        }
        if loading {
            title.push_str("\nPreparing audio…");
        }
        if playing {
            title.push_str("\nPlaying");
        }
        if slot.choke_group != 0 {
            title.push_str(&format!("\nChoke group {}", slot.choke_group));
        }

        Some(title)
    }

    pub(super) fn assign_selected_to_pad(&mut self, index: usize) {
        if let Some(sample) = self
            .samples
            .iter()
            .find(|sample| Some(&sample.file_path) == self.selected_path.as_ref())
            .cloned()
        {
            self.add_sample_to_slot(index, &sample);
        }
    }

    pub(super) fn render_selected_pad(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        // Parameter rows use compact chrome so the essential controls and fades
        // remain accessible below the waveform at ordinary file-manager window sizes.
        ui.spacing_mut().interact_size.y = 22.0;
        ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let index = self.selected_slot;
        self.render_selected_pad_header(ui, index);
        let slot = &mut self.pack_bank.slots[index];
        if slot.samples.is_empty() {
            ui.weak("Select a sample on the map, then Add selected or Shift+click a pad.");
        }
        let mut remove = None;
        let mut studio_layer = None;
        // One layer at a time keeps the waveform and controls at a predictable height.
        let layer_id = ui.id().with(("edited_layer", index));
        let mut selected_layer =
            self.selected_layers[index].min(slot.samples.len().saturating_sub(1));
        if !slot.samples.is_empty() {
            use crate::ui_tabs::{Tab, TabBar};
            let labels: Vec<_> = slot
                .samples
                .iter()
                .map(|sample| super::ui::short_label(&sample.filename, 22))
                .collect();
            let tabs: Vec<_> = labels
                .iter()
                .enumerate()
                .map(|(layer, label)| Tab::new(layer, label))
                .collect();
            let (selection, closed) =
                TabBar::new(layer_id, selected_layer, &tabs, palette).show_closable(ui);
            if let Some(layer) = selection {
                selected_layer = layer;
            }
            remove = closed;
            self.selected_layers[index] = selected_layer;
        }
        for (layer, sample) in slot
            .samples
            .iter_mut()
            .enumerate()
            .filter(|(layer, _)| *layer == selected_layer)
        {
            ui.push_id((index, layer), |ui| {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let title_limit = ((ui.available_width() - 180.0) / 8.0).max(12.0) as usize;
                    ui.label(
                        RichText::new(super::ui::short_label(&sample.filename, title_limit))
                            .strong(),
                    )
                    .on_hover_text(&sample.filename);
                    if ui.small_button("Edit in Audio Studio").clicked() {
                        studio_layer = Some(layer);
                    }
                });
                if ui.available_width() >= 700.0 {
                    let wave_width = ui.available_width() - 260.0;
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui(Vec2::new(wave_width, 200.0), |ui| {
                            Self::render_layer_trim(ui, sample)
                        });
                        ui.allocate_ui(Vec2::new(240.0, 200.0), |ui| {
                            ui.strong("Sound");
                            Self::render_layer_parameters(ui, sample);
                        });
                    });
                } else {
                    ui.strong("Sound");
                    Self::render_layer_parameters(ui, sample);
                    Self::render_layer_trim(ui, sample);
                }
                ui.separator();
            });
        }
        if let Some(layer) = remove {
            slot.samples.remove(layer);
            if layer < selected_layer {
                selected_layer -= 1;
            }
            self.selected_layers[index] = selected_layer.min(slot.samples.len().saturating_sub(1));
        }
        if let Some(layer) = studio_layer {
            self.request_pad_studio(index, layer);
        }
    }

    fn render_layer_trim(ui: &mut egui::Ui, sample: &mut super::sample_pack::SlotSample) {
        let frame_duration = sample
            .cached_audio
            .as_ref()
            .map_or(0.000_001, |audio| 1.0 / audio.sample_rate as f32);
        ui.horizontal_wrapped(|ui| {
            ui.strong("Trim");
            ui.label("Start");
            ui.add(
                egui::DragValue::new(&mut sample.start_sec)
                    .speed(0.001)
                    .clamp_range(0.0..=(sample.end_sec - frame_duration).max(0.0))
                    .max_decimals(3)
                    .suffix(" s"),
            );
            ui.label("End");
            ui.add(
                egui::DragValue::new(&mut sample.end_sec)
                    .speed(0.001)
                    .clamp_range(
                        (sample.start_sec + frame_duration).min(sample.duration)..=sample.duration,
                    )
                    .max_decimals(3)
                    .suffix(" s"),
            );
        });
        Self::render_layer_waveform(ui, sample);
    }

    fn render_layer_parameters(ui: &mut egui::Ui, sample: &mut super::sample_pack::SlotSample) {
        let mut gain = if sample.volume <= 0.0 {
            -60.0
        } else {
            20.0 * sample.volume.log10()
        };
        let mut gain_changed = false;
        let columns = if ui.available_width() >= 380.0 { 2 } else { 1 };
        ui.spacing_mut().slider_width =
            (ui.available_width() / columns as f32 - 145.0).clamp(50.0, 160.0);
        egui::Grid::new("pad_sound_controls").num_columns(columns).spacing(Vec2::new(12.0, 4.0)).show(ui, |ui| {
        for (index, (label, value, range, timed)) in [
            ("Gain", &mut gain, -60.0..=6.0206, false),
            ("Pan", &mut sample.pan, -1.0..=1.0, false),
            ("Attack", &mut sample.attack_ms, 0.0..=500.0, true),
            ("Release", &mut sample.release_ms, 0.0..=500.0, true),
        ].into_iter().enumerate() {
            let response = ui.add(
                egui::Slider::new(value, range)
                    .text(label)
                    .logarithmic(timed)
                    .smallest_positive(0.1)
                    .custom_formatter(move |number, _| {
                        if timed {
                            format!("{number:.1} ms")
                        } else if label == "Gain" {
                            if number <= -60.0 {
                                "−∞ dB".into()
                            } else {
                                format!("{number:.1} dB")
                            }
                        } else if number.abs() < 0.005 {
                            "Center".into()
                        } else {
                            format!(
                                "{} {:.0}%",
                                if number < 0.0 { "L" } else { "R" },
                                number.abs() * 100.0
                            )
                        }
                    }),
            );
            let reset = crate::shell::studio_controls::slider_double_clicked(ui, &response);
            if reset {
                *value = 0.0;
            }
            if label == "Gain" {
                gain_changed = response.changed() || reset;
            }
            response.on_hover_text("Drag to adjust · click the value to type · double-click to reset. Pan: -1 left, 0 center, +1 right.");
            if (index + 1) % columns == 0 { ui.end_row(); }
        }
        });
        if gain_changed {
            sample.volume = if gain <= -60.0 {
                0.0
            } else {
                10.0_f32.powf(gain / 20.0).min(2.0)
            };
        }
    }

    fn render_selected_pad_header(&mut self, ui: &mut egui::Ui, index: usize) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} · Pad {}", self.pad_keys[index].to_ascii_uppercase(), index + 1));
            ui.toggle_value(&mut self.pack_bank.slots[index].mute, "Mute");
            ui.toggle_value(&mut self.pack_bank.slots[index].solo, "Solo");
            let group = self.pack_bank.slots[index].choke_group;
            ui.label(if group == 0 { "Mute group: none".into() } else { format!("Mute group {group}") })
                .on_hover_text("Pads in the same mute group cut each other off. Hold Shift and right-click pads to link them; hover to see partners.");
            ui.menu_button("Pad actions", |ui| {
                if ui.add_enabled(self.selected_path.is_some(), egui::Button::new("Add map sample")).clicked() { self.assign_selected_to_pad(index); ui.close_menu(); }
                if ui.button("Leave mute group").clicked() { self.pack_bank.slots[index].choke_group = 0; ui.close_menu(); }
                if ui.button("Clear pad").clicked() { self.clear_slot(index); ui.close_menu(); }
            });
            if self.pack_bank.slots[index].samples.iter().any(|sample| self.failed_audio.contains(&sample.file_path)) && ui.small_button("Retry audio").clicked() {
                for sample in &self.pack_bank.slots[index].samples { self.failed_audio.remove(&sample.file_path); }
            }
        });
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
            self.audition_layers(&samples);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    #[test]
    fn inset_end_handle_trims_without_moving_the_start() {
        let ctx = egui::Context::default();
        let mut sample = super::super::sample_pack::SlotSample::new(
            "hit.wav".into(),
            "hit.wav".into(),
            None,
            1.0,
        );
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 180.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(ctx, |ui| {
                            ConstellationState::render_layer_waveform(ui, &mut sample);
                        });
                },
            )
        };
        frame(vec![]);
        let end = Pos2::new(628.0, 40.0);
        frame(vec![
            egui::Event::PointerMoved(end),
            egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let trimmed = Pos2::new(420.0, 40.0);
        frame(vec![egui::Event::PointerMoved(trimmed)]);
        frame(vec![egui::Event::PointerButton {
            pos: trimmed,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(sample.start_sec, 0.0);
        assert!((sample.end_sec - 408.0 / 616.0).abs() < 0.001);
    }
}
