//! UI implementation for the sample pack creation shelf and the sample pack browser.
//! Includes inline waveform rendering per sample layer with draggable zero-crossing start/end flags
//! and compact volume sliders directly inside each slot card.

use super::ConstellationState;
use super::sample_pack::{SamplePackBank, get_sample_packs_root, play_slot_sample};
use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use std::path::Path;
use std::time::{Duration, Instant};

impl ConstellationState {
    /// Triggers numbered pads or assigns the selected sample with Shift+number.
    ///
    /// Uses physical digit positions and each event's modifiers. Text editors retain
    /// their keys; other focused controls do not block the pads. Consumes handled
    /// presses before other file manager navigation handlers receive them.
    pub fn handle_shelf_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        let editing_text = ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if editing_text || self.browser_open {
            return;
        }

        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
            egui::Key::Num0,
        ];
        let mut actions = Vec::new();
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                let egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                else {
                    return true;
                };
                if modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd {
                    return true;
                }
                // Pad positions remain stable when Shift changes the logical character.
                let Some(slot_idx) = keys
                    .iter()
                    .position(|keycode| Some(*keycode) == *physical_key)
                    .or_else(|| keys.iter().position(|keycode| keycode == key))
                else {
                    return true;
                };
                actions.push((slot_idx, modifiers.shift));
                false
            })
        });
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

    /// Renders the sample pack creation shelf on the right side of the constellation view.
    pub fn render_pack_shelf(&mut self, ui: &mut egui::Ui, current_folder: &Path) {
        ui.vertical(|ui| {
            // Header: Bank Title & Persistence Toolbar
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Pack Shelf").strong().size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").on_hover_text("Collapse shelf").clicked() {
                        self.pack_shelf_open = false;
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Name:")
                        .size(11.5)
                        .color(Color32::from_gray(180)),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.pack_bank.name)
                        .hint_text("Pack name...")
                        .desired_width(140.0),
                );
            });

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.writing_pack(), egui::Button::new("Save"))
                    .on_hover_text(
                        "Save sample pack to little-screen-clip-sample-packs (symlinks + JSON)",
                    )
                    .clicked()
                {
                    let root = self
                        .current_indexed_folder
                        .as_deref()
                        .unwrap_or(current_folder);
                    self.save_bank(root.to_path_buf(), self.pack_bank.clone());
                }

                if ui
                    .button("Browse")
                    .on_hover_text("Open Sample Pack Browser")
                    .clicked()
                {
                    self.refresh_browser_packs(current_folder);
                    self.browser_open = true;
                }

                if ui
                    .button("Clear")
                    .on_hover_text("Clear all 10 slots to start a fresh bank")
                    .clicked()
                {
                    self.clear_bank();
                    self.browser_status = Some(("Cleared all slots".to_string(), Instant::now()));
                }
            });

            if let Some((ref msg, time)) = self.browser_status {
                if time.elapsed() < Duration::from_secs(4) {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(msg)
                            .size(10.5)
                            .color(if msg.starts_with("Error") {
                                Color32::from_rgb(255, 100, 100)
                            } else {
                                Color32::from_rgb(100, 230, 160)
                            }),
                    );
                }
            }

            ui.separator();

            // Quick instruction badge
            ui.label(
                RichText::new("Keys 1-0 trigger · Shift+1-0 assign selected")
                    .size(10.0)
                    .color(Color32::from_gray(140)),
            );
            ui.add_space(4.0);

            // 10 Slots (Keys 1..9, 0)
            let mut trigger_slot_idx: Option<usize> = None;
            let mut add_selected_to_idx: Option<usize> = None;
            let mut clear_slot_idx: Option<usize> = None;
            let mut remove_layer: Option<(usize, usize)> = None;
            let mut audition_sample: Option<super::sample_pack::SlotSample> = None;

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for (slot_idx, slot) in self.pack_bank.slots.iter_mut().enumerate() {
                        let is_flashing = self.slot_flash[slot_idx]
                            .map_or(false, |t| t.elapsed() < Duration::from_millis(220));

                        if is_flashing {
                            ui.ctx().request_repaint_after(Duration::from_millis(16));
                        }

                        let key_char = slot.key_char;
                        let sample_count = slot.samples.len();

                        // Slot card frame
                        let card_fill = if is_flashing {
                            Color32::from_rgba_premultiplied(40, 80, 140, 240)
                        } else if sample_count > 0 {
                            Color32::from_rgba_premultiplied(22, 28, 42, 220)
                        } else {
                            Color32::from_rgba_premultiplied(16, 20, 30, 180)
                        };

                        let card_stroke = if is_flashing {
                            Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 255))
                        } else if sample_count > 0 {
                            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(70, 95, 145, 120))
                        } else {
                            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(45, 55, 75, 80))
                        };

                        egui::Frame::none()
                            .fill(card_fill)
                            .stroke(card_stroke)
                            .rounding(6.0)
                            .inner_margin(6.0)
                            .show(ui, |ui| {
                                // Slot Header Row
                                ui.horizontal(|ui| {
                                    // Key badge button (clicking triggers slot)
                                    let badge_color = match slot_idx {
                                        0 | 1 => Color32::from_rgb(255, 150, 80),  // Kicks/Basses
                                        2 | 3 => Color32::from_rgb(255, 210, 80),  // Snares/Claps
                                        4 | 5 => Color32::from_rgb(100, 220, 255), // Hats/Cymbals
                                        6 | 7 => Color32::from_rgb(180, 140, 255), // Melodic/Keys
                                        _ => Color32::from_rgb(120, 240, 180),     // Perc/FX
                                    };

                                    let badge_btn = egui::Button::new(
                                        RichText::new(format!("[{key_char}]"))
                                            .strong()
                                            .size(12.0)
                                            .color(badge_color),
                                    )
                                    .fill(Color32::from_black_alpha(100));

                                    if ui
                                        .add(badge_btn)
                                        .on_hover_text(format!("Trigger slot (Key {key_char})"))
                                        .clicked()
                                    {
                                        trigger_slot_idx = Some(slot_idx);
                                    }

                                    let title = slot.display_title();
                                    // Right action buttons
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if sample_count > 0 {
                                                if ui
                                                    .small_button("Clear")
                                                    .on_hover_text("Clear slot")
                                                    .clicked()
                                                {
                                                    clear_slot_idx = Some(slot_idx);
                                                }
                                            }

                                            if ui
                                                .small_button("Add")
                                                .on_hover_text(
                                                    "Add currently selected sample to this slot",
                                                )
                                                .clicked()
                                            {
                                                add_selected_to_idx = Some(slot_idx);
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&title).size(11.0).color(
                                                        if sample_count > 0 {
                                                            Color32::WHITE
                                                        } else {
                                                            Color32::from_gray(130)
                                                        },
                                                    ),
                                                )
                                                .truncate(true),
                                            )
                                            .on_hover_text(&title);
                                        },
                                    );
                                });

                                // Empty slot state or Sample Layers with Waveforms
                                if sample_count == 0 {
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("Empty slot · Press Shift+")
                                                .size(9.5)
                                                .color(Color32::from_gray(120)),
                                        );
                                        ui.label(
                                            RichText::new(format!("{key_char}"))
                                                .size(9.5)
                                                .strong()
                                                .color(Color32::from_gray(170)),
                                        );
                                        ui.label(
                                            RichText::new("to assign selected")
                                                .size(9.5)
                                                .color(Color32::from_gray(120)),
                                        );
                                    });
                                } else {
                                    ui.add_space(4.0);

                                    // Render each sample layer with inline waveform & draggable trim flags
                                    for (layer_idx, sample) in slot.samples.iter_mut().enumerate() {
                                        // Layer sub-header
                                        ui.horizontal(|ui| {
                                            let cat_col = sample
                                                .category
                                                .map(|c| c.color())
                                                .unwrap_or(Color32::from_rgb(100, 200, 255));

                                            let short_name =
                                                super::ui::short_label(&sample.filename, 22);

                                            ui.label(
                                                RichText::new(format!(
                                                    "{}: {}",
                                                    layer_idx + 1,
                                                    short_name
                                                ))
                                                .size(10.5)
                                                .strong()
                                                .color(cat_col),
                                            );

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if sample_count > 1 {
                                                        if ui
                                                            .small_button("Remove")
                                                            .on_hover_text("Remove this layer")
                                                            .clicked()
                                                        {
                                                            remove_layer =
                                                                Some((slot_idx, layer_idx));
                                                        }
                                                    }
                                                    if ui
                                                        .small_button("Reset")
                                                        .on_hover_text("Reset start/end trim")
                                                        .clicked()
                                                    {
                                                        sample.start_sec = 0.0;
                                                        sample.end_sec = sample.duration;
                                                    }
                                                    if ui
                                                        .small_button("Play")
                                                        .on_hover_text(
                                                            "Audition this trimmed layer",
                                                        )
                                                        .clicked()
                                                    {
                                                        audition_sample = Some(sample.clone());
                                                    }
                                                },
                                            );
                                        });

                                        // Waveform display with draggable start/end flags
                                        let wave_height = 32.0;
                                        let wave_w = ui.available_width().max(160.0);
                                        let (rect, wave_resp) = ui.allocate_exact_size(
                                            Vec2::new(wave_w, wave_height),
                                            egui::Sense::click_and_drag(),
                                        );
                                        let painter = ui.painter_at(rect);

                                        // Waveform background box
                                        painter.rect_filled(
                                            rect,
                                            4.0,
                                            Color32::from_rgb(14, 18, 26),
                                        );
                                        painter.rect_stroke(
                                            rect,
                                            4.0,
                                            Stroke::new(
                                                1.0_f32,
                                                Color32::from_rgba_premultiplied(45, 55, 75, 120),
                                            ),
                                        );

                                        let total_dur = sample.duration.max(0.01);
                                        let start_ratio =
                                            (sample.start_sec / total_dur).clamp(0.0, 1.0);
                                        let end_ratio =
                                            (sample.end_sec / total_dur).clamp(0.0, 1.0);

                                        let start_x = rect.left() + start_ratio * rect.width();
                                        let end_x = rect.left() + end_ratio * rect.width();

                                        // Draw waveform peaks
                                        if let Some(ref cached) = sample.cached_audio {
                                            let peaks = &cached.waveform_peaks;
                                            let num_peaks = peaks.len();
                                            if num_peaks > 0 {
                                                let mid_y = rect.center().y;
                                                let h_half = rect.height() * 0.40;
                                                let cat_color = sample
                                                    .category
                                                    .map(|c| c.color())
                                                    .unwrap_or(Color32::from_rgb(90, 210, 255));
                                                let dim_color = Color32::from_rgb(50, 56, 68);

                                                for (i, &(min_val, max_val)) in
                                                    peaks.iter().enumerate()
                                                {
                                                    let px = rect.left()
                                                        + (i as f32 / num_peaks as f32)
                                                            * rect.width();
                                                    let is_active = px >= start_x && px <= end_x;
                                                    let col = if is_active {
                                                        cat_color
                                                    } else {
                                                        dim_color
                                                    };
                                                    let top_y =
                                                        mid_y - (max_val.abs() * h_half).max(1.0);
                                                    let btm_y =
                                                        mid_y + (min_val.abs() * h_half).max(1.0);
                                                    painter.line_segment(
                                                        [
                                                            Pos2::new(px, top_y),
                                                            Pos2::new(px, btm_y),
                                                        ],
                                                        Stroke::new(1.2_f32, col),
                                                    );
                                                }
                                            }
                                        }

                                        // Dim inactive regions
                                        if start_x > rect.left() {
                                            let inactive_left = Rect::from_min_max(
                                                rect.min,
                                                Pos2::new(start_x, rect.bottom()),
                                            );
                                            painter.rect_filled(
                                                inactive_left,
                                                4.0,
                                                Color32::from_black_alpha(110),
                                            );
                                        }
                                        if end_x < rect.right() {
                                            let inactive_right = Rect::from_min_max(
                                                Pos2::new(end_x, rect.top()),
                                                rect.max,
                                            );
                                            painter.rect_filled(
                                                inactive_right,
                                                4.0,
                                                Color32::from_black_alpha(110),
                                            );
                                        }

                                        // Midline
                                        painter.line_segment(
                                            [
                                                Pos2::new(rect.left(), rect.center().y),
                                                Pos2::new(rect.right(), rect.center().y),
                                            ],
                                            Stroke::new(
                                                0.5_f32,
                                                Color32::from_rgba_premultiplied(255, 255, 255, 25),
                                            ),
                                        );

                                        // Start flag [S] (emerald green)
                                        let start_col = Color32::from_rgb(50, 230, 150);
                                        painter.line_segment(
                                            [
                                                Pos2::new(start_x, rect.top()),
                                                Pos2::new(start_x, rect.bottom()),
                                            ],
                                            Stroke::new(2.0_f32, start_col),
                                        );
                                        let flag_s_rect = Rect::from_min_size(
                                            Pos2::new(start_x - 4.0, rect.top() - 1.0),
                                            Vec2::new(9.0, 9.0),
                                        );
                                        painter.rect_filled(flag_s_rect, 2.0, start_col);
                                        painter.text(
                                            flag_s_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "S",
                                            egui::FontId::monospace(7.5),
                                            Color32::BLACK,
                                        );

                                        // End flag [E] (coral red)
                                        let end_col = Color32::from_rgb(255, 90, 90);
                                        painter.line_segment(
                                            [
                                                Pos2::new(end_x, rect.top()),
                                                Pos2::new(end_x, rect.bottom()),
                                            ],
                                            Stroke::new(2.0_f32, end_col),
                                        );
                                        let flag_e_rect = Rect::from_min_size(
                                            Pos2::new(end_x - 5.0, rect.top() - 1.0),
                                            Vec2::new(9.0, 9.0),
                                        );
                                        painter.rect_filled(flag_e_rect, 2.0, end_col);
                                        painter.text(
                                            flag_e_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "E",
                                            egui::FontId::monospace(7.5),
                                            Color32::BLACK,
                                        );

                                        // Interactive dragging of flags
                                        let drag_id =
                                            ui.id().with(("flag_drag", slot_idx, layer_idx));
                                        if wave_resp.drag_started() {
                                            if let Some(pos) = wave_resp.interact_pointer_pos() {
                                                let dist_s = (pos.x - start_x).abs();
                                                let dist_e = (pos.x - end_x).abs();
                                                let chosen_flag: u8 =
                                                    if dist_s <= dist_e { 0 } else { 1 };
                                                ui.memory_mut(|m| {
                                                    m.data.insert_persisted(drag_id, chosen_flag)
                                                });
                                            }
                                        }

                                        if wave_resp.dragged() {
                                            let chosen_flag: u8 = ui
                                                .memory_mut(|m| m.data.get_persisted(drag_id))
                                                .unwrap_or(0);
                                            if let Some(pos) = wave_resp.interact_pointer_pos() {
                                                let ratio = ((pos.x - rect.left()) / rect.width())
                                                    .clamp(0.0, 1.0);
                                                let raw_t = ratio * total_dur;
                                                if chosen_flag == 0 {
                                                    let snapped = sample.snap_time(raw_t);
                                                    sample.start_sec = snapped.clamp(
                                                        0.0,
                                                        (sample.end_sec - 0.005).max(0.0),
                                                    );
                                                } else {
                                                    let snapped = sample.snap_time(raw_t);
                                                    sample.end_sec = snapped.clamp(
                                                        (sample.start_sec + 0.005).min(total_dur),
                                                        total_dur,
                                                    );
                                                }
                                            }
                                        }

                                        if wave_resp.drag_stopped() {
                                            audition_sample = Some(sample.clone());
                                        }

                                        // Compact controls strip below waveform
                                        ui.horizontal(|ui| {
                                            let start_ms = (sample.start_sec * 1000.0) as u32;
                                            let end_ms = (sample.end_sec * 1000.0) as u32;
                                            ui.label(
                                                RichText::new(format!("{start_ms}ms → {end_ms}ms"))
                                                    .size(9.5)
                                                    .color(Color32::from_gray(160)),
                                            );

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let vol_slider = egui::Slider::new(
                                                        &mut sample.volume,
                                                        0.0..=2.0,
                                                    )
                                                    .show_value(true)
                                                    .custom_formatter(|v, _| {
                                                        format!("{:.0}%", v * 100.0)
                                                    });
                                                    ui.add_sized(Vec2::new(55.0, 16.0), vol_slider);
                                                    ui.label(
                                                        RichText::new("Vol:")
                                                            .size(9.5)
                                                            .color(Color32::from_gray(160)),
                                                    );
                                                },
                                            );
                                        });

                                        ui.add_space(2.0);
                                    }
                                }
                            });

                        ui.add_space(3.0);
                    }
                });

            // Handle actions deferred to avoid borrow checker conflicts
            if let Some(idx) = trigger_slot_idx {
                self.trigger_slot(idx);
            }
            if let Some(idx) = add_selected_to_idx {
                if let Some(ref path) = self.selected_path.clone() {
                    if let Some(sample) =
                        self.samples.iter().find(|s| &s.file_path == path).cloned()
                    {
                        self.add_sample_to_slot(idx, &sample);
                    }
                }
            }
            if let Some(idx) = clear_slot_idx {
                self.clear_slot(idx);
            }
            if let Some((s_idx, l_idx)) = remove_layer {
                if s_idx < self.pack_bank.slots.len()
                    && l_idx < self.pack_bank.slots[s_idx].samples.len()
                {
                    self.pack_bank.slots[s_idx].samples.remove(l_idx);
                }
            }
            if let Some(sample) = audition_sample {
                if let Some(player) = play_slot_sample(&sample) {
                    self.active_players.push(player);
                }
            }
        });
    }

    /// Renders the Sample Pack Browser window for loading and managing banks.
    pub fn render_pack_browser(&mut self, ctx: &egui::Context, current_folder: &Path) {
        if !self.browser_open {
            return;
        }

        let mut load_bank_path: Option<std::path::PathBuf> = None;
        let mut preview_bank_path: Option<std::path::PathBuf> = None;
        let mut delete_bank_path: Option<std::path::PathBuf> = None;
        let mut overwrite_bank: Option<SamplePackBank> = None;
        let mut refresh_packs = false;
        let mut new_empty_pack = false;
        let mut is_open = self.browser_open;

        egui::Window::new("Sample Pack Browser")
            .id(egui::Id::new("constellation_pack_browser_window"))
            .open(&mut is_open)
            .default_size(Vec2::new(740.0, 520.0))
            .min_size(Vec2::new(620.0, 420.0))
            .resizable(true)
            .show(ctx, |ui| {
                let root = self
                    .current_indexed_folder
                    .as_deref()
                    .unwrap_or(current_folder);
                let packs_dir = get_sample_packs_root(root);

                // Top banner
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Packs directory:")
                            .size(11.5)
                            .color(Color32::from_gray(180)),
                    );
                    ui.label(
                        RichText::new(packs_dir.display().to_string())
                            .size(11.0)
                            .color(Color32::from_rgb(140, 200, 255)),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Refresh").clicked() {
                            refresh_packs = true;
                        }
                        if ui.button("New Empty Pack").clicked() {
                            new_empty_pack = true;
                        }
                    });
                });

                ui.separator();

                // Split layout: Packs list on left, Bank preview on right
                ui.columns(2, |columns| {
                    // Left Column: Saved Packs List
                    columns[0].vertical(|ui| {
                        ui.heading(RichText::new("Saved Packs").size(14.0));
                        ui.add_space(4.0);

                        if self.browser_packs.is_empty() {
                            ui.label("No saved packs found yet.\nUse 'Save' on the creation shelf to save a pack.");
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(400.0)
                                .show(ui, |ui| {
                                    for (idx, summary) in self.browser_packs.iter().enumerate() {
                                        let is_selected = self.browser_selected_idx == Some(idx);
                                        let text = format!(
                                            "{} ({} slots · {} samples)",
                                            summary.folder_path.file_name().unwrap_or_default().to_string_lossy(),
                                            summary.slot_count, summary.sample_count
                                        );

                                        let label = egui::SelectableLabel::new(is_selected, text);
                                        if ui.add(label).clicked() {
                                            self.browser_selected_idx = Some(idx);
                                            self.browser_preview = None;
                                            preview_bank_path = Some(summary.folder_path.clone());
                                        }
                                    }
                                });
                        }
                    });

                    // Right Column: Selected Pack Preview & Actions
                    columns[1].vertical(|ui| {
                        if let Some(idx) = self.browser_selected_idx {
                            if idx < self.browser_packs.len() {
                                let summary = &self.browser_packs[idx];
                                ui.heading(RichText::new(&summary.name).size(15.0).color(Color32::WHITE));
                                ui.label(
                                    RichText::new(format!(
                                        "{} slots with {} total samples",
                                        summary.slot_count, summary.sample_count
                                    ))
                                    .size(11.0)
                                    .color(Color32::from_gray(170)),
                                );

                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .button(RichText::new("Load into Creation Shelf").strong())
                                        .clicked()
                                    {
                                        load_bank_path = Some(summary.folder_path.clone());
                                    }

                                    if ui
                                        .add_enabled(!self.writing_pack(), egui::Button::new("Save Copy"))
                                        .on_hover_text("Save current shelf slots as a new version of this pack")
                                        .clicked()
                                    {
                                        let mut updated = self.pack_bank.clone();
                                        updated.name = summary.name.clone();
                                        overwrite_bank = Some(updated);
                                    }

                                    if ui
                                        .add_enabled(!self.writing_pack(), egui::Button::new(RichText::new("Delete").color(Color32::from_rgb(255, 120, 120))))
                                        .clicked()
                                    {
                                        delete_bank_path = Some(summary.folder_path.clone());
                                    }
                                });

                                ui.separator();

                                // Preview 10 slots of this pack
                                if let Some((_, bank)) = self.browser_preview.as_ref().filter(|(path, _)| path == &summary.folder_path) {
                                    ui.label(RichText::new("Slot Preview (Click Play to audition):").size(11.0).strong());
                                    ui.add_space(4.0);

                                    egui::ScrollArea::vertical()
                                        .max_height(280.0)
                                        .show(ui, |ui| {
                                            for slot in &bank.slots {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new(format!("[{}]", slot.key_char)).strong().size(11.0));
                                                    if !slot.samples.is_empty() {
                                                        if ui.small_button("Play").clicked() {
                                                            for s in &slot.samples {
                                                                if let Some(p) = play_slot_sample(s) {
                                                                    self.active_players.push(p);
                                                                }
                                                            }
                                                        }
                                                    }
                                                    ui.label(RichText::new(slot.display_title()).size(10.5));
                                                });
                                            }
                                        });
                                }
                            }
                        } else {
                            ui.vertical_centered(|ui| {
                                ui.add_space(40.0);
                                ui.label(RichText::new("Select a sample pack on the left to preview or load.").color(Color32::from_gray(150)));
                            });
                        }
                    });
                });
            });

        if !is_open {
            self.close_pack_browser();
        }

        if refresh_packs {
            self.refresh_browser_packs(current_folder);
        }

        if new_empty_pack {
            self.clear_bank();
            self.browser_status = Some(("Started new empty pack".into(), Instant::now()));
        }

        // Apply deferred actions
        if let Some(path) = preview_bank_path {
            self.load_browser_bank(path, false);
        }
        if let Some(path) = load_bank_path {
            self.load_browser_bank(path, true);
        }
        if let Some(bank) = overwrite_bank {
            let root = self
                .current_indexed_folder
                .as_deref()
                .unwrap_or(current_folder)
                .to_path_buf();
            self.save_bank(root, bank);
        }
        if let Some(path) = delete_bank_path {
            self.delete_bank(path);
        }
    }
}
