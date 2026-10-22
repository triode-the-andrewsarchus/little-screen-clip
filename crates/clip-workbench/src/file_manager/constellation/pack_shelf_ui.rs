//! UI implementation for the sample pack creation shelf, floating slot control window,
//! and the sample pack browser.

use super::ConstellationState;
use super::sample_pack::{
    NUM_SLOTS, SamplePackBank, delete_sample_pack, get_sample_packs_root, load_sample_pack,
    play_slot_sample, save_sample_pack,
};
use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use std::path::Path;
use std::time::{Duration, Instant};

impl ConstellationState {
    /// Handles keyboard shortcuts for triggering (keys 1-0) and assigning (Shift+1-0) sample slots.
    pub fn handle_shelf_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }

        let keys = [
            (egui::Key::Num1, 0),
            (egui::Key::Num2, 1),
            (egui::Key::Num3, 2),
            (egui::Key::Num4, 3),
            (egui::Key::Num5, 4),
            (egui::Key::Num6, 5),
            (egui::Key::Num7, 6),
            (egui::Key::Num8, 7),
            (egui::Key::Num9, 8),
            (egui::Key::Num0, 9),
        ];

        let shift_held = ctx.input(|i| i.modifiers.shift);

        for (key, slot_idx) in keys {
            if ctx.input(|i| i.key_pressed(key)) {
                if shift_held {
                    if let Some(ref selected_path) = self.selected_path.clone() {
                        if let Some(sample) = self
                            .samples
                            .iter()
                            .find(|s| &s.file_path == selected_path)
                            .cloned()
                        {
                            self.add_sample_to_slot(slot_idx, &sample);
                            self.slot_flash[slot_idx] = Some(Instant::now());
                        }
                    }
                } else {
                    self.trigger_slot(slot_idx);
                }
            }
        }
    }

    /// Renders the sample pack creation shelf on the right side of the constellation view.
    pub fn render_pack_shelf(&mut self, ui: &mut egui::Ui, current_folder: &Path) {
        ui.vertical(|ui| {
            // Header: Bank Title & Persistence Toolbar
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("🎛️ Pack Shelf").strong().size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✕").on_hover_text("Collapse shelf").clicked() {
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
                    .button("💾 Save")
                    .on_hover_text(
                        "Save sample pack to little-screen-clip-sample-packs (symlinks + JSON)",
                    )
                    .clicked()
                {
                    let root = self
                        .current_indexed_folder
                        .as_deref()
                        .unwrap_or(current_folder);
                    match save_sample_pack(root, &self.pack_bank) {
                        Ok(dir) => {
                            let name = dir
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            self.browser_status =
                                Some((format!("✓ Saved pack '{name}'"), Instant::now()));
                            self.refresh_browser_packs(current_folder);
                        }
                        Err(e) => {
                            self.browser_status = Some((format!("Error: {e}"), Instant::now()));
                        }
                    }
                }

                if ui
                    .button("📂 Browse")
                    .on_hover_text("Open Sample Pack Browser")
                    .clicked()
                {
                    self.refresh_browser_packs(current_folder);
                    self.browser_open = true;
                }

                if ui
                    .button("✨ Clear")
                    .on_hover_text("Clear all 10 slots to start a fresh bank")
                    .clicked()
                {
                    self.pack_bank = SamplePackBank::new("Untitled Pack");
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
                RichText::new("⌨ Keys 1-0 trigger · Shift+1-0 assign selected")
                    .size(10.0)
                    .color(Color32::from_gray(140)),
            );
            ui.add_space(4.0);

            // 10 Slots (Keys 1..9, 0)
            let mut hovered_slot_pos: Option<(usize, Rect)> = None;
            let mut trigger_slot_idx: Option<usize> = None;
            let mut add_selected_to_idx: Option<usize> = None;
            let mut clear_slot_idx: Option<usize> = None;
            let mut toggle_pin_idx: Option<usize> = None;

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for slot_idx in 0..NUM_SLOTS {
                        let is_flashing = self.slot_flash[slot_idx]
                            .map_or(false, |t| t.elapsed() < Duration::from_millis(220));
                        let is_pinned = self.pinned_slot_idx == Some(slot_idx);

                        let slot = &self.pack_bank.slots[slot_idx];
                        let key_char = slot.key_char;
                        let sample_count = slot.samples.len();

                        // Slot card frame
                        let card_fill = if is_flashing {
                            Color32::from_rgba_premultiplied(40, 80, 140, 240)
                        } else if is_pinned {
                            Color32::from_rgba_premultiplied(32, 42, 64, 235)
                        } else if sample_count > 0 {
                            Color32::from_rgba_premultiplied(22, 28, 42, 220)
                        } else {
                            Color32::from_rgba_premultiplied(16, 20, 30, 180)
                        };

                        let card_stroke = if is_flashing {
                            Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 255))
                        } else if is_pinned {
                            Stroke::new(1.0_f32, Color32::from_rgb(90, 150, 230))
                        } else if sample_count > 0 {
                            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(70, 95, 145, 120))
                        } else {
                            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(45, 55, 75, 80))
                        };

                        let response = egui::Frame::none()
                            .fill(card_fill)
                            .stroke(card_stroke)
                            .rounding(6.0)
                            .inner_margin(6.0)
                            .show(ui, |ui| {
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

                                    // Play icon button
                                    if ui.button("▶").on_hover_text("Play slot").clicked() {
                                        trigger_slot_idx = Some(slot_idx);
                                    }

                                    // Slot title / layers summary
                                    let title = slot.display_title();
                                    let short_title = if title.len() > 18 {
                                        format!("{}...", &title[..15])
                                    } else {
                                        title
                                    };

                                    ui.label(RichText::new(short_title).size(11.0).color(
                                        if sample_count > 0 {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_gray(130)
                                        },
                                    ));

                                    // Right action buttons
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if sample_count > 0 {
                                                if ui
                                                    .small_button("✕")
                                                    .on_hover_text("Clear slot")
                                                    .clicked()
                                                {
                                                    clear_slot_idx = Some(slot_idx);
                                                }
                                            }

                                            let pin_label =
                                                if is_pinned { "📌" } else { "🎛️" };
                                            if ui
                                                .small_button(pin_label)
                                                .on_hover_text("Toggle slot controls")
                                                .clicked()
                                            {
                                                toggle_pin_idx = Some(slot_idx);
                                            }

                                            if ui
                                                .small_button("➕")
                                                .on_hover_text(
                                                    "Add currently selected sample to this slot",
                                                )
                                                .clicked()
                                            {
                                                add_selected_to_idx = Some(slot_idx);
                                            }
                                        },
                                    );
                                });

                                // Layer badges row if multiple samples assigned
                                if sample_count > 0 {
                                    ui.horizontal_wrapped(|ui| {
                                        for (idx, sample) in slot.samples.iter().enumerate() {
                                            let cat_col = sample
                                                .category
                                                .map(|c| c.color())
                                                .unwrap_or(Color32::from_gray(160));

                                            let layer_text =
                                                format!("L{}: {}", idx + 1, sample.filename);
                                            let short_l = if layer_text.len() > 14 {
                                                format!("{}...", &layer_text[..12])
                                            } else {
                                                layer_text
                                            };

                                            let pill = egui::Frame::none()
                                                .fill(Color32::from_rgba_premultiplied(
                                                    cat_col.r() / 4,
                                                    cat_col.g() / 4,
                                                    cat_col.b() / 4,
                                                    180,
                                                ))
                                                .stroke(Stroke::new(0.8_f32, cat_col))
                                                .rounding(3.0)
                                                .inner_margin(Vec2::new(4.0, 1.0))
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        RichText::new(short_l)
                                                            .size(9.0)
                                                            .color(Color32::WHITE),
                                                    );
                                                });
                                            let _ = pill.response;
                                        }
                                    });
                                }
                            })
                            .response;

                        if response.hovered() {
                            hovered_slot_pos = Some((slot_idx, response.rect));
                        }

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
            if let Some(idx) = toggle_pin_idx {
                if self.pinned_slot_idx == Some(idx) {
                    self.pinned_slot_idx = None;
                } else {
                    self.pinned_slot_idx = Some(idx);
                }
            }

            // Record hovered slot
            self.hovered_slot_idx = hovered_slot_pos.map(|(idx, _)| idx);

            // Render floating control window if a slot is hovered or pinned
            let target_slot_idx = self.pinned_slot_idx.or(self.hovered_slot_idx);
            if let Some(idx) = target_slot_idx {
                let anchor_y = hovered_slot_pos
                    .map(|(_, rect)| rect.top())
                    .unwrap_or(220.0);
                self.render_slot_control_popup(ui.ctx(), idx, anchor_y);
            }
        });
    }

    /// Floating glass control window for adjusting start, end, and volume with zero-crossing snapping.
    pub fn render_slot_control_popup(
        &mut self,
        ctx: &egui::Context,
        slot_idx: usize,
        anchor_y: f32,
    ) {
        if slot_idx >= self.pack_bank.slots.len() {
            return;
        }

        let screen_w = ctx.screen_rect().width();
        let popup_w = 310.0;
        let popup_x = (screen_w - 320.0 - popup_w - 10.0).max(10.0);
        let popup_y = anchor_y.clamp(60.0, (ctx.screen_rect().bottom() - 340.0).max(60.0));

        let is_pinned = self.pinned_slot_idx == Some(slot_idx);
        let key_char = self.pack_bank.slots[slot_idx].key_char;

        let mut remove_layer_idx: Option<usize> = None;
        let mut audition_sample: Option<super::sample_pack::SlotSample> = None;
        let mut close_popup = false;
        let mut toggle_pin = false;

        egui::Area::new(egui::Id::new("slot_hover_control_window"))
            .fixed_pos(Pos2::new(popup_x, popup_y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(Color32::from_rgba_premultiplied(16, 20, 32, 246))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(80, 130, 220, 180)))
                    .rounding(8.0)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_width(popup_w);

                        // Header
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("🎛️ Slot [{key_char}] Controls"))
                                    .strong()
                                    .size(13.0)
                                    .color(Color32::from_rgb(120, 210, 255)),
                            );

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("✕").on_hover_text("Close control window").clicked() {
                                    close_popup = true;
                                }

                                let pin_color = if is_pinned {
                                    Color32::from_rgb(255, 200, 80)
                                } else {
                                    Color32::from_gray(140)
                                };
                                if ui
                                    .small_button(RichText::new("📌").color(pin_color))
                                    .on_hover_text(if is_pinned { "Unpin window" } else { "Pin window open" })
                                    .clicked()
                                {
                                    toggle_pin = true;
                                }
                            });
                        });

                        ui.separator();

                        let slot = &mut self.pack_bank.slots[slot_idx];

                        if slot.samples.is_empty() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Slot is empty.\nSelect a sample in constellation and click '+ Add Selected' or press Shift+1..0.")
                                    .size(11.0)
                                    .color(Color32::from_gray(150)),
                            );
                            ui.add_space(8.0);
                        } else {
                            ui.label(
                                RichText::new(format!("{} layer(s) assigned · Plays together", slot.samples.len()))
                                    .size(10.0)
                                    .color(Color32::from_gray(160)),
                            );
                            ui.add_space(4.0);

                            for (layer_idx, sample) in slot.samples.iter_mut().enumerate() {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        let cat_col = sample
                                            .category
                                            .map(|c| c.color())
                                            .unwrap_or(Color32::WHITE);

                                        ui.label(
                                            RichText::new(format!("L{}:", layer_idx + 1))
                                                .strong()
                                                .size(11.5)
                                                .color(cat_col),
                                        );

                                        let name = if sample.filename.len() > 18 {
                                            format!("{}...", &sample.filename[..15])
                                        } else {
                                            sample.filename.clone()
                                        };
                                        ui.label(RichText::new(name).size(11.0).color(Color32::WHITE));

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("🗑️").on_hover_text("Remove this layer").clicked() {
                                                remove_layer_idx = Some(layer_idx);
                                            }
                                            if ui.small_button("▶").on_hover_text("Audition layer solo").clicked() {
                                                audition_sample = Some(sample.clone());
                                            }
                                        });
                                    });

                                    ui.add_space(2.0);

                                    // Volume slider
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Volume:").size(10.5));
                                        let vol_slider = egui::Slider::new(&mut sample.volume, 0.0..=2.0)
                                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                                            .clamp_to_range(true);
                                        ui.add(vol_slider);
                                    });

                                    // Start point slider (snaps to zero crossing)
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Start:").size(10.5));
                                        let max_start = (sample.end_sec - 0.005).max(0.0);
                                        let mut start_val = sample.start_sec;
                                        let start_slider = egui::Slider::new(&mut start_val, 0.0..=max_start)
                                            .custom_formatter(|v, _| format!("{:.3} s", v))
                                            .clamp_to_range(true);
                                        if ui.add(start_slider).changed() {
                                            sample.start_sec = sample.snap_time(start_val).clamp(0.0, max_start);
                                        }
                                    });

                                    // End point slider (snaps to zero crossing)
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("End:").size(10.5));
                                        let min_end = (sample.start_sec + 0.005).min(sample.duration);
                                        let mut end_val = sample.end_sec;
                                        let end_slider = egui::Slider::new(&mut end_val, min_end..=sample.duration)
                                            .custom_formatter(|v, _| format!("{:.3} s", v))
                                            .clamp_to_range(true);
                                        if ui.add(end_slider).changed() {
                                            sample.end_sec = sample.snap_time(end_val).clamp(min_end, sample.duration);
                                        }
                                    });

                                    // Snapped length indicator
                                    let active_dur = (sample.end_sec - sample.start_sec).max(0.0);
                                    ui.label(
                                        RichText::new(format!(
                                            "⚡ Zero-crossing snapped (duration: {:.3} s)",
                                            active_dur
                                        ))
                                        .size(9.0)
                                        .color(Color32::from_rgb(120, 210, 255)),
                                    );
                                });
                                ui.add_space(2.0);
                            }
                        }

                        // Add selected sample button
                        if let Some(ref path) = self.selected_path.clone() {
                            if let Some(sample) = self.samples.iter().find(|s| &s.file_path == path) {
                                let label = format!("➕ Add Layer ({})", sample.filename);
                                let short_lbl = if label.len() > 30 {
                                    format!("{}...", &label[..27])
                                } else {
                                    label
                                };
                                if ui.button(short_lbl).clicked() {
                                    let s_clone = sample.clone();
                                    self.add_sample_to_slot(slot_idx, &s_clone);
                                }
                            }
                        }
                    });
            });

        if let Some(idx) = remove_layer_idx {
            if slot_idx < self.pack_bank.slots.len()
                && idx < self.pack_bank.slots[slot_idx].samples.len()
            {
                self.pack_bank.slots[slot_idx].samples.remove(idx);
            }
        }

        if let Some(sample) = audition_sample {
            if let Some(player) = play_slot_sample(&sample) {
                self.active_players.push(player);
            }
        }

        if toggle_pin {
            if self.pinned_slot_idx == Some(slot_idx) {
                self.pinned_slot_idx = None;
            } else {
                self.pinned_slot_idx = Some(slot_idx);
            }
        }

        if close_popup {
            self.pinned_slot_idx = None;
            self.hovered_slot_idx = None;
        }
    }

    /// Renders the Sample Pack Browser window for loading and managing banks.
    pub fn render_pack_browser(&mut self, ctx: &egui::Context, current_folder: &Path) {
        if !self.browser_open {
            return;
        }

        let mut load_bank_path: Option<std::path::PathBuf> = None;
        let mut delete_bank_path: Option<std::path::PathBuf> = None;
        let mut overwrite_bank: Option<SamplePackBank> = None;
        let mut refresh_packs = false;
        let mut new_empty_pack = false;
        let mut is_open = self.browser_open;

        egui::Window::new("📦 Sample Pack Browser")
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
                        RichText::new("📁 Packs directory:")
                            .size(11.5)
                            .color(Color32::from_gray(180)),
                    );
                    ui.label(
                        RichText::new(packs_dir.display().to_string())
                            .size(11.0)
                            .color(Color32::from_rgb(140, 200, 255)),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🔄 Refresh").clicked() {
                            refresh_packs = true;
                        }
                        if ui.button("➕ New Empty Pack").clicked() {
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
                            ui.label("No saved packs found yet.\nUse '💾 Save' on the creation shelf to save a pack.");
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(400.0)
                                .show(ui, |ui| {
                                    for (idx, summary) in self.browser_packs.iter().enumerate() {
                                        let is_selected = self.browser_selected_idx == Some(idx);
                                        let text = format!(
                                            "{} ({} slots · {} samples)",
                                            summary.name, summary.slot_count, summary.sample_count
                                        );

                                        let label = egui::SelectableLabel::new(is_selected, text);
                                        if ui.add(label).clicked() {
                                            self.browser_selected_idx = Some(idx);
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
                                        .button(RichText::new("📥 Load into Creation Shelf").strong())
                                        .clicked()
                                    {
                                        load_bank_path = Some(summary.folder_path.clone());
                                    }

                                    if ui
                                        .button("💾 Overwrite")
                                        .on_hover_text("Overwrite this pack with current shelf slots")
                                        .clicked()
                                    {
                                        let mut updated = self.pack_bank.clone();
                                        updated.name = summary.name.clone();
                                        overwrite_bank = Some(updated);
                                    }

                                    if ui
                                        .button(RichText::new("🗑️ Delete").color(Color32::from_rgb(255, 120, 120)))
                                        .clicked()
                                    {
                                        delete_bank_path = Some(summary.folder_path.clone());
                                    }
                                });

                                ui.separator();

                                // Preview 10 slots of this pack
                                if let Ok(bank) = load_sample_pack(&summary.folder_path) {
                                    ui.label(RichText::new("Slot Preview (Click ▶ to audition):").size(11.0).strong());
                                    ui.add_space(4.0);

                                    egui::ScrollArea::vertical()
                                        .max_height(280.0)
                                        .show(ui, |ui| {
                                            for slot in &bank.slots {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new(format!("[{}]", slot.key_char)).strong().size(11.0));
                                                    if !slot.samples.is_empty() {
                                                        if ui.small_button("▶").clicked() {
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

        self.browser_open = is_open;

        if refresh_packs {
            self.refresh_browser_packs(current_folder);
        }

        if new_empty_pack {
            self.pack_bank = SamplePackBank::new("Untitled Pack");
            self.browser_status = Some(("Started new empty pack".into(), Instant::now()));
        }

        // Apply deferred actions
        if let Some(path) = load_bank_path {
            match load_sample_pack(&path) {
                Ok(bank) => {
                    self.pack_bank = bank;
                    self.browser_status = Some((
                        format!("✓ Loaded pack '{}'", self.pack_bank.name),
                        Instant::now(),
                    ));
                    self.browser_open = false;
                }
                Err(e) => {
                    self.browser_status =
                        Some((format!("Error loading pack: {e}"), Instant::now()));
                }
            }
        }

        if let Some(bank) = overwrite_bank {
            let root = self
                .current_indexed_folder
                .as_deref()
                .unwrap_or(current_folder);
            match save_sample_pack(root, &bank) {
                Ok(_) => {
                    self.browser_status =
                        Some((format!("✓ Overwrote pack '{}'", bank.name), Instant::now()));
                    self.refresh_browser_packs(current_folder);
                }
                Err(e) => {
                    self.browser_status = Some((format!("Error saving pack: {e}"), Instant::now()));
                }
            }
        }

        if let Some(path) = delete_bank_path {
            let _ = delete_sample_pack(&path);
            self.browser_selected_idx = None;
            self.refresh_browser_packs(current_folder);
            self.browser_status = Some(("Deleted sample pack".into(), Instant::now()));
        }
    }
}
