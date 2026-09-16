//! Side-by-side hex dump and binary diff visualizer workbench view.
//!
//! Renders aligned hex grids, diff blocks, ASCII previews, and interactive navigation
//! across binary byte buffers.

use crate::shell::{EscapeHatch, PanelState, StudioShell};
use crate::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, FontId, Rounding, Stroke, Vec2};
use little_clip_core::theme::AppPalette;
use little_clip_ops::binary_diff::{
    AlignedCell, BinaryDiffMode, BinaryDiffResult, DiffKind, byte_to_ascii_char,
    compute_binary_diff, search_rows,
};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// Color highlighting mode for hexadecimal byte displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteColorMode {
    /// Tint individual hex digit characters.
    Digits,
    /// Tint the entire background bounding box of the byte.
    Background,
}

/// UI and navigation state for the binary hex diff viewer.
pub struct BinaryDiffViewState {
    pub left_label: String,
    pub left_bytes: Vec<u8>,
    pub right_label: String,
    pub right_bytes: Vec<u8>,
    pub mode: BinaryDiffMode,
    pub color_mode: ByteColorMode,
    pub bytes_per_row: usize,
    pub result: Option<BinaryDiffResult>,
    pub is_computing: bool,
    pub receiver: Option<Receiver<BinaryDiffResult>>,
    // Navigation & search state
    pub search_query: String,
    pub search_matches: Vec<usize>,
    pub current_search_idx: usize,
    pub current_diff_idx: usize,
    pub scroll_target_row: Option<usize>,
    pub hovered_cell: Option<(usize, usize)>, // (row_idx, col_idx)
    pub status_toast: crate::ui_ext::StatusToast,
    pub selected_diff_block: Option<usize>,
    pub current_scroll_offset_x: f32,
    pub current_scroll_offset_y: f32,
    pub last_scrolled_diff_id: Option<usize>,
    pub visible_row_count: usize,
    pub panel_state: PanelState,
}

impl BinaryDiffViewState {
    /// Creates a new binary diff viewer state and spawns background diff computation.
    pub fn new(
        left_label: String,
        left_bytes: Vec<u8>,
        right_label: String,
        right_bytes: Vec<u8>,
    ) -> Self {
        let mode = BinaryDiffMode::Aligned;
        let bytes_per_row = 16;

        let (tx, rx) = channel();
        let l_lbl = left_label.clone();
        let l_b = left_bytes.clone();
        let r_lbl = right_label.clone();
        let r_b = right_bytes.clone();

        thread::spawn(move || {
            let res = compute_binary_diff(&l_lbl, &l_b, &r_lbl, &r_b, mode, bytes_per_row);
            let _ = tx.send(res);
        });

        Self {
            left_label,
            left_bytes,
            right_label,
            right_bytes,
            mode,
            color_mode: ByteColorMode::Digits,
            bytes_per_row,
            result: None,
            is_computing: true,
            receiver: Some(rx),
            search_query: String::new(),
            search_matches: Vec::new(),
            current_search_idx: 0,
            current_diff_idx: 0,
            scroll_target_row: None,
            hovered_cell: None,
            status_toast: crate::ui_ext::StatusToast::with_duration(3.0),
            selected_diff_block: None,
            current_scroll_offset_x: 0.0,
            current_scroll_offset_y: 0.0,
            last_scrolled_diff_id: None,
            visible_row_count: 30,
            panel_state: PanelState::new(),
        }
    }

    pub fn recompute(&mut self) {
        self.is_computing = true;
        let (tx, rx) = channel();
        let l_lbl = self.left_label.clone();
        let l_b = self.left_bytes.clone();
        let r_lbl = self.right_label.clone();
        let r_b = self.right_bytes.clone();
        let mode = self.mode;
        let bpr = self.bytes_per_row;

        thread::spawn(move || {
            let res = compute_binary_diff(&l_lbl, &l_b, &r_lbl, &r_b, mode, bpr);
            let _ = tx.send(res);
        });

        self.receiver = Some(rx);
    }

    pub fn swap_operands(&mut self) {
        std::mem::swap(&mut self.left_label, &mut self.right_label);
        std::mem::swap(&mut self.left_bytes, &mut self.right_bytes);
        self.recompute();
    }

    pub fn toggle_mode(&mut self) {
        self.mode = self.mode.toggled();
        self.recompute();
    }

    pub fn next_diff(&mut self) {
        if let Some(ref res) = self.result {
            if res.diff_blocks.is_empty() {
                return;
            }
            self.current_diff_idx = (self.current_diff_idx + 1) % res.diff_blocks.len();
            let block = &res.diff_blocks[self.current_diff_idx];
            self.selected_diff_block = Some(block.id);
            self.scroll_target_row = Some(block.start_row);
        }
    }

    pub fn prev_diff(&mut self) {
        if let Some(ref res) = self.result {
            if res.diff_blocks.is_empty() {
                return;
            }
            if self.current_diff_idx == 0 {
                self.current_diff_idx = res.diff_blocks.len() - 1;
            } else {
                self.current_diff_idx -= 1;
            }
            let block = &res.diff_blocks[self.current_diff_idx];
            self.selected_diff_block = Some(block.id);
            self.scroll_target_row = Some(block.start_row);
        }
    }

    pub fn update_search(&mut self) {
        if let Some(ref res) = self.result {
            self.search_matches = search_rows(res, &self.search_query);
            self.current_search_idx = 0;
            if let Some(&first_row) = self.search_matches.first() {
                self.scroll_target_row = Some(first_row);
            }
        }
    }

    pub fn next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.current_search_idx = (self.current_search_idx + 1) % self.search_matches.len();
        self.scroll_target_row = Some(self.search_matches[self.current_search_idx]);
    }

    pub fn prev_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.current_search_idx == 0 {
            self.current_search_idx = self.search_matches.len() - 1;
        } else {
            self.current_search_idx -= 1;
        }
        self.scroll_target_row = Some(self.search_matches[self.current_search_idx]);
    }

    /// Calculates responsive cell dimensions, font sizes, and row height based on bytes_per_row
    /// Returns: (cell_w, cell_h, font_size, ascii_w, offset_w, row_height)
    pub fn layout_metrics(&self) -> (f32, f32, f32, f32, f32, f32) {
        match self.bytes_per_row {
            8 => (24.0, 22.0, 13.5, 11.5, 80.0, 23.0),
            16 => (18.5, 21.0, 12.5, 9.0, 75.0, 22.0),
            _ => (16.5, 20.0, 11.5, 8.0, 68.0, 20.0),
        }
    }

    /// Triggers opening the binary comparison in ImHex hex editor.
    pub fn trigger_escape_hatch(&mut self, now: f64) {
        let temp_left = std::env::temp_dir().join(format!(
            "clip_bin_{}_lhs.bin",
            uuid::Uuid::new_v4().simple()
        ));
        let temp_right = std::env::temp_dir().join(format!(
            "clip_bin_{}_rhs.bin",
            uuid::Uuid::new_v4().simple()
        ));

        if let Err(e) = std::fs::write(&temp_left, &self.left_bytes) {
            self.status_toast
                .set(format!("Failed to write left temp file: {}", e), now);
            return;
        }

        if let Err(e) = std::fs::write(&temp_right, &self.right_bytes) {
            self.status_toast
                .set(format!("Failed to write right temp file: {}", e), now);
            return;
        }

        if std::process::Command::new("imhex")
            .arg(&temp_left)
            .arg(&temp_right)
            .spawn()
            .is_ok()
        {
            self.status_toast.set("🚀 Launched ImHex hex editor", now);
        } else if std::process::Command::new("xdg-open")
            .arg(&temp_left)
            .spawn()
            .is_ok()
        {
            self.status_toast
                .set("🚀 Opened left binary file with system default", now);
        } else {
            self.status_toast.set("⚠️ ImHex is not installed", now);
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);

        // Check async receiver
        if self.is_computing {
            ctx.request_repaint();
            if let Some(ref rx) = self.receiver {
                if let Ok(res) = rx.try_recv() {
                    self.result = Some(res);
                    self.is_computing = false;
                    self.receiver = None;
                    if !self.search_query.is_empty() {
                        self.update_search();
                    }
                }
            }
        }

        // Global shortcuts
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close_requested = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) && !ctx.wants_keyboard_input() {
            self.toggle_mode();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::S)) && !ctx.wants_keyboard_input() {
            self.swap_operands();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::J)) && !ctx.wants_keyboard_input() {
            self.next_diff();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::K)) && !ctx.wants_keyboard_input() {
            self.prev_diff();
        }

        let l_size_str = format_bytes_size(self.left_bytes.len());
        let r_size_str = format_bytes_size(self.right_bytes.len());
        let hatch = EscapeHatch::new("🔬 ImHex", "imhex", "Open in ImHex Hex Editor");

        let mut shell = StudioShell::new("Binary Diff", "🔬", palette, close_requested)
            .with_badge(format!("◀ {} ({})", self.left_label, l_size_str), None)
            .with_badge(format!("▶ {} ({})", self.right_label, r_size_str), None)
            .with_escape_hatch(hatch)
            .with_footer_hints("Navigation: [Esc] Close • [Tab] Mode • [S] Swap • [J/K] Diff Nav • [Search] Hex/ASCII");

        if self.is_computing {
            shell = shell.with_badge("⏳ Computing...", Some(hex_to_c32(&palette.harmonies.base)));
        } else if let Some(ref res) = self.result {
            let pct_color = if res.stats.similarity_pct >= 90.0 {
                hex_to_c32(&palette.diff_add)
            } else if res.stats.similarity_pct >= 50.0 {
                hex_to_c32(&palette.harmonies.warm_attention)
            } else {
                hex_to_c32(&palette.diff_del)
            };
            shell = shell
                .with_badge(
                    format!("{:.1}% similar", res.stats.similarity_pct),
                    Some(pct_color),
                )
                .with_badge(
                    format!("⚡ {} diffs", res.diff_blocks.len()),
                    Some(hex_to_c32(&palette.diff_del)),
                )
                .with_footer_readout(format!(
                    "Rows: {} • Width: {}B • Mode: {:?}",
                    res.rows.len(),
                    self.bytes_per_row,
                    self.mode
                ));
        }

        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.background))
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = crate::secondary_button("⇄ Swap [S]", hex_to_c32(&palette.theme.b_med), palette);
                    if ui.add(swap_btn).on_hover_text("Swap operands (S)").clicked() {
                        self.swap_operands();
                    }

                    let copy_btn = crate::secondary_button("📋 Summary", hex_to_c32(&palette.theme.b_med), palette);
                    if ui.add(copy_btn).clicked() {
                        if let Some(ref res) = self.result {
                            let summary = format!(
                                "Binary Diff Summary:\nLeft: {} ({} bytes)\nRight: {} ({} bytes)\nSimilarity: {:.2}%\nMatches: {} B | Mismatches: {} B | Insertions: {} B | Deletions: {} B\nMode: {:?}",
                                res.left_label, res.stats.left_total_bytes,
                                res.right_label, res.stats.right_total_bytes,
                                res.stats.similarity_pct,
                                res.stats.match_count,
                                res.stats.mismatch_count,
                                res.stats.right_only_count,
                                res.stats.left_only_count,
                                res.mode
                            );
                            ctx.output_mut(|o| o.copied_text = summary);
                            self.status_toast.set("Summary copied to clipboard!", now);
                        }
                    }

                    let diff_panel_label = if self.panel_state.right_collapsed { "Diffs ◧" } else { "Diffs ◨" };
                    let diff_toggle_btn = crate::mode_tab_button(diff_panel_label, !self.panel_state.right_collapsed, palette);
                    if ui.add(diff_toggle_btn).on_hover_text("Toggle Differences Panel").clicked() {
                        self.panel_state.toggle_right();
                    }
                });

                if clicked_hatch.is_some() {
                    self.trigger_escape_hatch(now);
                }

                if shell.is_closed() {
                    return;
                }

                ui.add_space(4.0);

                // Zone 2: Mode & Filter Bar
                self.render_mode_and_stats_bar(ui, palette);

                ui.add_space(6.0);

                // Zone 3: Main Layout
                let footer_h = if self.hovered_cell.is_some() { 64.0 } else { 32.0 };
                let avail_h = (ui.available_height() - footer_h - 4.0).max(100.0);

                ui.horizontal(|ui| {
                    let diff_panel_w = if self.panel_state.right_collapsed {
                        0.0
                    } else {
                        self.panel_state.effective_right_width().min(280.0)
                    };
                    let minimap_w = 16.0;
                    let table_w = if diff_panel_w > 0.0 {
                        (ui.available_width() - diff_panel_w - minimap_w - 36.0).max(350.0)
                    } else {
                        (ui.available_width() - minimap_w - 16.0).max(400.0)
                    };

                    // 1. Hex Table
                    ui.push_id("binary_hex_table_section", |ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(table_w, avail_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                self.render_hex_table(ui, palette, avail_h);
                            },
                        );
                    });

                    // 2. Minimap Track
                    ui.push_id("binary_minimap_section", |ui| {
                        self.render_minimap(ui, palette, avail_h);
                    });

                    // 3. Differences Table on the right
                    if diff_panel_w > 0.0 {
                        ui.separator();
                        ui.push_id("binary_diff_panel_section", |ui| {
                            self.render_differences_panel(ui, palette, avail_h, diff_panel_w);
                        });
                    }
                });

                // Zone 5: Footer & Cell Inspection
                self.render_footer_inspection(ui, palette);
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
    }

    fn render_mode_and_stats_bar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let f_low = hex_to_c32(&palette.theme.f_low);

        let Some(ref res) = self.result else {
            return;
        };

        let match_count = res.stats.match_count;
        let mismatch_count = res.stats.mismatch_count;
        let left_only_count = res.stats.left_only_count;
        let right_only_count = res.stats.right_only_count;
        let diff_blocks_len = res.diff_blocks.len();
        let search_len = self.search_matches.len();
        let search_idx = self.current_search_idx;

        let mut next_diff = false;
        let mut prev_diff = false;
        let mut next_search = false;
        let mut prev_search = false;
        let mut do_search = false;

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .rounding(Rounding::same(4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Mode selector
                    let mode_lbl = match self.mode {
                        BinaryDiffMode::Aligned => "Aligned",
                        BinaryDiffMode::Raw => "Raw",
                    };
                    let mode_btn =
                        crate::mode_tab_button(format!("Mode: {} [Tab]", mode_lbl), true, palette);
                    if ui
                        .add(mode_btn)
                        .on_hover_text("Toggle LCS aligned / Raw view (Tab)")
                        .clicked()
                    {
                        self.toggle_mode();
                    }

                    // Width selector
                    ui.label(
                        egui::RichText::new("Width:")
                            .font(FontId::proportional(12.0))
                            .color(f_low),
                    );
                    for &w in &[8, 16, 32] {
                        let is_active = self.bytes_per_row == w;
                        let btn = crate::mode_tab_button(format!("{}B", w), is_active, palette);
                        if ui.add(btn).clicked() && self.bytes_per_row != w {
                            self.bytes_per_row = w;
                            self.recompute();
                        }
                    }

                    // Color mode
                    for &(mode, label) in &[
                        (ByteColorMode::Digits, "Digits"),
                        (ByteColorMode::Background, "Bg"),
                    ] {
                        let is_active = self.color_mode == mode;
                        let btn = crate::mode_tab_button(label, is_active, palette);
                        if ui.add(btn).clicked() && self.color_mode != mode {
                            self.color_mode = mode;
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // Stat pills
                    crate::ui_ext::stat_pill(
                        ui,
                        "Match",
                        match_count,
                        Some(hex_to_c32(&palette.theme.f_med)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Diff",
                        mismatch_count,
                        Some(hex_to_c32(&palette.diff_mod)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Del",
                        left_only_count,
                        Some(hex_to_c32(&palette.diff_del)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Ins",
                        right_only_count,
                        Some(hex_to_c32(&palette.diff_add)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Blocks",
                        diff_blocks_len,
                        Some(hex_to_c32(&palette.harmonies.warm_attention)),
                        palette,
                    );

                    // Right side of bar: Search & Diff nav
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let next_btn = crate::secondary_button("Next [J]", b_med, palette);
                        if ui.add(next_btn).clicked() {
                            next_diff = true;
                        }
                        let prev_btn = crate::secondary_button("Prev [K]", b_med, palette);
                        if ui.add(prev_btn).clicked() {
                            prev_diff = true;
                        }

                        ui.add_space(8.0);

                        if search_len > 0 {
                            ui.label(
                                egui::RichText::new(format!("{}/{}", search_idx + 1, search_len))
                                    .font(FontId::monospace(11.0))
                                    .color(hex_to_c32(&palette.harmonies.base)),
                            );
                            let down_btn = crate::secondary_button("▼", b_med, palette);
                            if ui.add(down_btn).clicked() {
                                next_search = true;
                            }
                            let up_btn = crate::secondary_button("▲", b_med, palette);
                            if ui.add(up_btn).clicked() {
                                prev_search = true;
                            }
                        }

                        let search_edit = ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("🔍 Search Hex / ASCII...")
                                .desired_width(180.0),
                        );
                        if search_edit.changed() {
                            do_search = true;
                        }
                    });
                });
            });

        if next_diff {
            self.next_diff();
        }
        if prev_diff {
            self.prev_diff();
        }
        if next_search {
            self.next_search_match();
        }
        if prev_search {
            self.prev_search_match();
        }
        if do_search {
            self.update_search();
        }
    }

    fn render_hex_table(&mut self, ui: &mut egui::Ui, palette: &AppPalette, _avail_h: f32) {
        if self.is_computing {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.spinner();
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("Aligning binary sequences using Myers LCS diff...")
                        .font(FontId::proportional(15.0))
                        .color(hex_to_c32(&palette.harmonies.base)),
                );
            });
            return;
        }

        let Some(ref res) = self.result else {
            ui.label("No diff result available.");
            return;
        };

        let (cell_w, _cell_h, font_size, ascii_char_w, offset_w, row_height) =
            self.layout_metrics();
        let total_rows = res.rows.len();
        let bpr = res.bytes_per_row;

        let center_w = 20.0;
        let hex_group_space = 5.0;
        let num_gaps = if bpr == 8 {
            1
        } else {
            (bpr / 8).saturating_sub(1)
        };
        let hex_w = bpr as f32 * cell_w
            + (bpr.saturating_sub(1) as f32 * 2.0)
            + (num_gaps as f32 * hex_group_space);
        let ascii_w = bpr as f32 * ascii_char_w;

        // Table column headers
        let (header_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), egui::Sense::hover());
        ui.painter().rect_filled(
            header_rect,
            Rounding::ZERO,
            hex_to_c32(&palette.theme.b_low),
        );
        let header_ui_rect = egui::Rect::from_min_max(
            egui::pos2(
                header_rect.min.x + 4.0 - self.current_scroll_offset_x,
                header_rect.min.y,
            ),
            egui::pos2(
                header_rect.min.x + 4.0 - self.current_scroll_offset_x + 3500.0,
                header_rect.max.y,
            ),
        );
        let mut child_ui = ui.child_ui(
            header_ui_rect,
            egui::Layout::left_to_right(egui::Align::Center),
        );
        child_ui.set_clip_rect(header_rect);
        child_ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);

        child_ui.allocate_ui_with_layout(
            Vec2::new(offset_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("Offset L")
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_low)),
                );
            },
        );
        child_ui.allocate_ui_with_layout(
            Vec2::new(hex_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(format!("Hex Bytes ({})", res.left_label))
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_med)),
                );
            },
        );
        child_ui.add_space(8.0);
        child_ui.allocate_ui_with_layout(
            Vec2::new(ascii_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("ASCII")
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_med)),
                );
            },
        );
        child_ui.allocate_ui_with_layout(
            Vec2::new(center_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("Diff")
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.diff_mod)),
                );
            },
        );
        child_ui.allocate_ui_with_layout(
            Vec2::new(hex_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(format!("Hex Bytes ({})", res.right_label))
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_med)),
                );
            },
        );
        child_ui.add_space(8.0);
        child_ui.allocate_ui_with_layout(
            Vec2::new(ascii_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("ASCII")
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_med)),
                );
            },
        );
        child_ui.allocate_ui_with_layout(
            Vec2::new(offset_w, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("Offset R")
                        .font(FontId::monospace(11.5))
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_low)),
                );
            },
        );

        ui.separator();

        let scroll_target = self.scroll_target_row.take();
        let mut hovered = self.hovered_cell;

        let rem_h = ui.available_height();
        let mut scroll_area = egui::ScrollArea::both()
            .id_source("binary_diff_hex_scroll")
            .auto_shrink([false, false])
            .max_height(rem_h)
            // The minimap is the vertical navigator. Keep ScrollArea for wheel
            // input and horizontal panning, but never expose its tiny native bar.
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);

        if let Some(target) = scroll_target {
            let target_scroll_row = target.saturating_sub(1);
            let target_px = target_scroll_row as f32 * row_height;
            scroll_area = scroll_area.vertical_scroll_offset(target_px);
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().scroll.bar_width = 12.0;

        let output = scroll_area.show_rows(ui, row_height, total_rows, |ui, row_range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row_idx in row_range {
                let row = &res.rows[row_idx];

                let is_search_match = self.search_matches.contains(&row_idx);
                let is_selected_diff = if let Some(sel_id) = self.selected_diff_block {
                    if let Some(block) = res.diff_blocks.iter().find(|b| b.id == sel_id) {
                        row_idx >= block.start_row && row_idx <= block.end_row
                    } else {
                        false
                    }
                } else {
                    false
                };

                // Row background
                let bg_color = if is_selected_diff {
                    hex_to_c32_alpha(&palette.harmonies.base, 45)
                } else if is_search_match {
                    hex_to_c32_alpha(&palette.harmonies.base, 25)
                } else if row.has_diff {
                    hex_to_c32_alpha(&palette.diff_mod, 18)
                } else if row_idx % 2 == 1 {
                    hex_to_c32_alpha(&palette.theme.f_low, 8)
                } else {
                    Color32::TRANSPARENT
                };

                let row_rect = ui.available_rect_before_wrap();
                let actual_row_rect = egui::Rect::from_min_size(
                    row_rect.min,
                    Vec2::new(row_rect.width(), row_height),
                );
                ui.painter()
                    .rect_filled(actual_row_rect, Rounding::ZERO, bg_color);

                if is_selected_diff {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(actual_row_rect.min, Vec2::new(3.5, row_height)),
                        Rounding::ZERO,
                        hex_to_c32(&palette.harmonies.base),
                    );
                    ui.painter().line_segment(
                        [actual_row_rect.left_top(), actual_row_rect.right_top()],
                        Stroke::new(1.0, hex_to_c32_alpha(&palette.harmonies.base, 100)),
                    );
                    ui.painter().line_segment(
                        [
                            actual_row_rect.left_bottom(),
                            actual_row_rect.right_bottom(),
                        ],
                        Stroke::new(1.0, hex_to_c32_alpha(&palette.harmonies.base, 100)),
                    );
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                    ui.add_space(4.0);

                    // 1. Left Offset
                    let l_off_str = if let Some(off) = row.left_offset {
                        format!("{:08X} ", off)
                    } else {
                        "         ".to_string()
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(offset_w, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(l_off_str)
                                    .font(FontId::monospace(font_size - 1.0))
                                    .color(hex_to_c32(&palette.theme.f_low)),
                            );
                        },
                    );

                    // 2. Left Hex Bytes
                    for (col_idx, cell) in row.left_cells.iter().enumerate() {
                        if col_idx > 0
                            && ((bpr == 8 && col_idx == 4) || (bpr != 8 && col_idx % 8 == 0))
                        {
                            ui.add_space(hex_group_space);
                        }

                        let (text, color, pill_bg) =
                            format_cell_hex(cell, true, self.color_mode, palette);
                        let is_cell_hovered = hovered == Some((row_idx, col_idx));

                        let (rect, r) = ui.allocate_exact_size(
                            Vec2::new(cell_w, row_height),
                            egui::Sense::hover(),
                        );

                        if let Some(pill) = pill_bg {
                            ui.painter()
                                .rect_filled(rect.expand(0.5), Rounding::same(2.0), pill);
                        }

                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            &text,
                            FontId::monospace(font_size),
                            color,
                        );

                        if is_cell_hovered {
                            ui.painter().rect_stroke(
                                rect.expand(1.0),
                                Rounding::same(2.0),
                                Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)),
                            );
                        }

                        if r.hovered() {
                            hovered = Some((row_idx, col_idx));
                        }
                    }

                    ui.add_space(8.0);

                    // 3. Left ASCII
                    for (col_idx, cell) in row.left_cells.iter().enumerate() {
                        let (ch, color) = format_cell_ascii(cell, true, self.color_mode, palette);
                        let is_cell_hovered = hovered == Some((row_idx, col_idx));

                        let (rect, r) = ui.allocate_exact_size(
                            Vec2::new(ascii_char_w, row_height),
                            egui::Sense::hover(),
                        );

                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            ch.to_string(),
                            FontId::monospace(font_size),
                            color,
                        );

                        if is_cell_hovered {
                            ui.painter().rect_stroke(
                                rect.expand(1.0),
                                Rounding::same(2.0),
                                Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)),
                            );
                        }

                        if r.hovered() {
                            hovered = Some((row_idx, col_idx));
                        }
                    }

                    // 4. Center Match Glyph
                    let glyph_color = match row.row_glyph {
                        "≠" => hex_to_c32(&palette.diff_mod),
                        "+" => hex_to_c32(&palette.diff_add),
                        "-" => hex_to_c32(&palette.diff_del),
                        "~" => hex_to_c32(&palette.harmonies.triadic_1),
                        _ => hex_to_c32(&palette.theme.f_low),
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(center_w, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(format!(" {} ", row.row_glyph))
                                    .font(FontId::monospace(font_size))
                                    .color(glyph_color),
                            );
                        },
                    );

                    // 5. Right Hex Bytes
                    for (col_idx, cell) in row.right_cells.iter().enumerate() {
                        if col_idx > 0
                            && ((bpr == 8 && col_idx == 4) || (bpr != 8 && col_idx % 8 == 0))
                        {
                            ui.add_space(hex_group_space);
                        }

                        let (text, color, pill_bg) =
                            format_cell_hex(cell, false, self.color_mode, palette);
                        let is_cell_hovered = hovered == Some((row_idx, col_idx));

                        let (rect, r) = ui.allocate_exact_size(
                            Vec2::new(cell_w, row_height),
                            egui::Sense::hover(),
                        );

                        if let Some(pill) = pill_bg {
                            ui.painter()
                                .rect_filled(rect.expand(0.5), Rounding::same(2.0), pill);
                        }

                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            &text,
                            FontId::monospace(font_size),
                            color,
                        );

                        if is_cell_hovered {
                            ui.painter().rect_stroke(
                                rect.expand(1.0),
                                Rounding::same(2.0),
                                Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)),
                            );
                        }

                        if r.hovered() {
                            hovered = Some((row_idx, col_idx));
                        }
                    }

                    ui.add_space(8.0);

                    // 6. Right ASCII
                    for (col_idx, cell) in row.right_cells.iter().enumerate() {
                        let (ch, color) = format_cell_ascii(cell, false, self.color_mode, palette);
                        let is_cell_hovered = hovered == Some((row_idx, col_idx));

                        let (rect, r) = ui.allocate_exact_size(
                            Vec2::new(ascii_char_w, row_height),
                            egui::Sense::hover(),
                        );

                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            ch.to_string(),
                            FontId::monospace(font_size),
                            color,
                        );

                        if is_cell_hovered {
                            ui.painter().rect_stroke(
                                rect.expand(1.0),
                                Rounding::same(2.0),
                                Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)),
                            );
                        }

                        if r.hovered() {
                            hovered = Some((row_idx, col_idx));
                        }
                    }

                    // 7. Right Offset
                    let r_off_str = if let Some(off) = row.right_offset {
                        format!("{:08X}", off)
                    } else {
                        "        ".to_string()
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(offset_w, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(r_off_str)
                                    .font(FontId::monospace(font_size - 1.0))
                                    .color(hex_to_c32(&palette.theme.f_low)),
                            );
                        },
                    );
                });
            }
        });

        self.current_scroll_offset_x = output.state.offset.x;
        self.current_scroll_offset_y = output.state.offset.y;
        self.visible_row_count = (output.inner_rect.height() / row_height) as usize;
        self.hovered_cell = hovered;
    }

    fn render_minimap(&mut self, ui: &mut egui::Ui, palette: &AppPalette, avail_h: f32) {
        let Some(ref res) = self.result else {
            return;
        };

        let total_rows = res.rows.len();
        if total_rows == 0 {
            return;
        }

        let width = 16.0;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(width, avail_h - 10.0),
            egui::Sense::click_and_drag(),
        );

        // Background groove
        ui.painter()
            .rect_filled(rect, Rounding::same(3.0), hex_to_c32(&palette.theme.b_low));
        ui.painter().rect_stroke(
            rect,
            Rounding::same(3.0),
            Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)),
        );

        // Difference markers
        for block in &res.diff_blocks {
            let start_frac = block.start_row as f32 / total_rows as f32;
            let end_frac = (block.end_row + 1) as f32 / total_rows as f32;
            let y_start = rect.top() + start_frac * rect.height();
            let y_end = rect.top() + end_frac * rect.height();
            let h = (y_end - y_start).max(2.5);

            let marker_rect = egui::Rect::from_min_size(
                egui::Pos2::new(rect.left() + 2.0, y_start),
                Vec2::new(width - 4.0, h),
            );

            let marker_color = match block.kind {
                DiffKind::LeftOnly => hex_to_c32(&palette.diff_del),
                DiffKind::RightOnly => hex_to_c32(&palette.diff_add),
                _ => hex_to_c32(&palette.harmonies.warm_attention),
            };

            ui.painter()
                .rect_filled(marker_rect, Rounding::same(1.0), marker_color);
        }

        // Viewport position indicator
        let (_cell_w, _cell_h, _font_size, _ascii_w, _offset_w, row_height) = self.layout_metrics();
        let total_height = (total_rows as f32 * row_height).max(1.0);
        let scroll_frac = (self.current_scroll_offset_y / total_height).clamp(0.0, 1.0);
        let visible_frac = (avail_h / total_height).clamp(0.03, 1.0);

        let view_y = rect.top() + scroll_frac * rect.height();
        let view_h = (visible_frac * rect.height()).max(14.0);
        let view_rect = egui::Rect::from_min_size(
            egui::Pos2::new(rect.left() + 1.0, view_y.min(rect.bottom() - view_h)),
            Vec2::new(width - 2.0, view_h),
        );

        ui.painter().rect_filled(
            view_rect,
            Rounding::same(2.0),
            hex_to_c32_alpha(&palette.theme.f_high, 25),
        );
        ui.painter().rect_stroke(
            view_rect,
            Rounding::same(2.0),
            Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)),
        );

        // Click / Drag to jump
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_y = (pos.y - rect.top()).clamp(0.0, rect.height());
                let frac = rel_y / rect.height();
                let target_row =
                    ((frac * total_rows as f32) as usize).min(total_rows.saturating_sub(1));
                self.scroll_target_row = Some(target_row);
                ui.ctx().request_repaint();
            }
        }

        // Tooltip on hover
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if let Some(pos) = response.hover_pos() {
                let rel_y = (pos.y - rect.top()).clamp(0.0, rect.height());
                let frac = rel_y / rect.height();
                let hover_row =
                    ((frac * total_rows as f32) as usize).min(total_rows.saturating_sub(1));
                let approx_off = hover_row * res.bytes_per_row;
                response.on_hover_ui_at_pointer(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Jump to ~0x{:08X} (Row {})",
                            approx_off,
                            hover_row + 1
                        ))
                        .font(FontId::monospace(11.0))
                        .color(hex_to_c32(&palette.theme.f_high)),
                    );
                });
            }
        }
    }

    fn render_differences_panel(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        avail_h: f32,
        panel_width: f32,
    ) {
        let Some(ref res) = self.result else {
            return;
        };

        let mut do_next = false;
        let mut do_prev = false;

        ui.allocate_ui_with_layout(
            Vec2::new(panel_width, avail_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                // Header bar
                egui::Frame::none()
                    .fill(hex_to_c32(&palette.theme.b_low))
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "⚡ Diffs ({})",
                                    res.diff_blocks.len()
                                ))
                                .font(FontId::proportional(12.0))
                                .strong()
                                .color(hex_to_c32(&palette.diff_mod)),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let next_btn = crate::secondary_button(
                                        "▶",
                                        hex_to_c32(&palette.theme.b_med),
                                        palette,
                                    );
                                    if ui
                                        .add(next_btn)
                                        .on_hover_text("Next Difference [J]")
                                        .clicked()
                                    {
                                        do_next = true;
                                    }
                                    let prev_btn = crate::secondary_button(
                                        "◀",
                                        hex_to_c32(&palette.theme.b_med),
                                        palette,
                                    );
                                    if ui
                                        .add(prev_btn)
                                        .on_hover_text("Previous Difference [K]")
                                        .clicked()
                                    {
                                        do_prev = true;
                                    }
                                },
                            );
                        });
                    });

                ui.separator();

                if res.diff_blocks.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("✔ Files identical")
                                .font(FontId::proportional(12.0))
                                .color(hex_to_c32(&palette.diff_add)),
                        );
                    });
                    return;
                }

                // Scrollable list of diff cards
                let rem_h = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_source("binary_diff_cards_scroll")
                    .auto_shrink([false, false])
                    .max_height(rem_h)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.spacing_mut().scroll.bar_width = 6.0;
                        let card_margin_x = 6.0;
                        let card_outer_w = (ui.available_width() - 10.0).max(60.0);
                        let card_inner_w = (card_outer_w - 2.0 * card_margin_x).max(40.0);

                        let mut clicked_row = None;
                        let mut clicked_id = None;

                        for block in &res.diff_blocks {
                            let is_selected = self.selected_diff_block == Some(block.id);

                            let badge_color = match block.kind {
                                DiffKind::Mismatch => hex_to_c32(&palette.diff_mod),
                                DiffKind::LeftOnly => hex_to_c32(&palette.diff_del),
                                DiffKind::RightOnly => hex_to_c32(&palette.diff_add),
                                DiffKind::Match => hex_to_c32(&palette.theme.f_med),
                            };

                            let frame = egui::Frame::none()
                                .fill(if is_selected {
                                    hex_to_c32_alpha(&palette.harmonies.base, 35)
                                } else {
                                    hex_to_c32(&palette.theme.b_low)
                                })
                                .stroke(if is_selected {
                                    Stroke::new(1.5, hex_to_c32(&palette.harmonies.base))
                                } else {
                                    Stroke::new(1.0, hex_to_c32(&palette.theme.b_med))
                                })
                                .rounding(Rounding::same(4.0))
                                .inner_margin(egui::Margin {
                                    left: card_margin_x,
                                    right: card_margin_x,
                                    top: 4.0,
                                    bottom: 4.0,
                                });

                            ui.push_id(block.id, |ui| {
                                let resp = frame.show(ui, |ui| {
                                    ui.set_width(card_inner_w);
                                    ui.set_max_width(card_inner_w);
                                    ui.vertical(|ui| {
                                        // Row 1: ID badge + Kind + byte count
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("#{}", block.id))
                                                    .font(FontId::monospace(10.0))
                                                    .strong()
                                                    .color(hex_to_c32(&palette.theme.f_med)),
                                            );

                                            ui.label(
                                                egui::RichText::new(&block.kind_label)
                                                    .font(FontId::proportional(10.5))
                                                    .strong()
                                                    .color(badge_color),
                                            );

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{} B",
                                                            block.byte_count
                                                        ))
                                                        .font(FontId::monospace(10.0))
                                                        .strong()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                    );
                                                },
                                            );
                                        });

                                        // Row 2: Offset
                                        let off_str = match (block.left_offset, block.right_offset)
                                        {
                                            (Some(l), Some(r)) if l == r => format!("0x{:06X}", l),
                                            (Some(l), Some(r)) => {
                                                format!("L:0x{:04X} R:0x{:04X}", l, r)
                                            }
                                            (Some(l), None) => format!("L: 0x{:06X}", l),
                                            (None, Some(r)) => format!("R: 0x{:06X}", r),
                                            (None, None) => "Unknown".to_string(),
                                        };
                                        ui.label(
                                            egui::RichText::new(off_str)
                                                .font(FontId::monospace(10.0))
                                                .color(hex_to_c32(&palette.theme.f_low)),
                                        );

                                        // Row 3: Preview
                                        ui.label(
                                            egui::RichText::new(&block.preview)
                                                .font(FontId::monospace(10.0))
                                                .color(hex_to_c32(&palette.theme.f_high)),
                                        );
                                    });
                                });

                                if is_selected && self.last_scrolled_diff_id != Some(block.id) {
                                    resp.response.scroll_to_me(Some(egui::Align::Center));
                                }

                                let interact = ui.interact(
                                    resp.response.rect,
                                    resp.response.id,
                                    egui::Sense::click(),
                                );
                                if interact.clicked() {
                                    clicked_row = Some(block.start_row);
                                    clicked_id = Some(block.id);
                                    if let Some(idx) =
                                        res.diff_blocks.iter().position(|b| b.id == block.id)
                                    {
                                        self.current_diff_idx = idx;
                                    }
                                }
                                if interact.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                ui.add_space(2.0);
                            });
                        }

                        if let Some(target) = clicked_row {
                            self.scroll_target_row = Some(target);
                            self.selected_diff_block = clicked_id;
                            ui.ctx().request_repaint();
                        }
                        self.last_scrolled_diff_id = self.selected_diff_block;
                    });
            },
        );

        if do_next {
            self.next_diff();
        }
        if do_prev {
            self.prev_diff();
        }
    }

    fn render_footer_inspection(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        if let (Some(res), Some((row_idx, col_idx))) = (&self.result, self.hovered_cell) {
            if let Some(row) = res.rows.get(row_idx) {
                let l_cell = row.left_cells.get(col_idx);
                let r_cell = row.right_cells.get(col_idx);

                egui::Frame::none()
                    .fill(hex_to_c32(&palette.theme.b_low))
                    .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("🔍 INSPECT: ")
                                    .font(FontId::proportional(12.0))
                                    .strong()
                                    .color(hex_to_c32(&palette.harmonies.base)),
                            );

                            // Left cell details
                            if let Some(c) = l_cell {
                                if let Some(b) = c.byte_val {
                                    let off_str = c.offset.map(|o| format!("0x{:08X}", o)).unwrap_or_else(|| "none".to_string());
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "LEFT: 0x{:02X} (Dec: {}, Bin: {:08b}, ASCII: '{}', Off: {})",
                                            b, b, b, byte_to_ascii_char(b), off_str
                                        ))
                                        .font(FontId::monospace(12.0))
                                        .color(hex_to_c32(&palette.theme.f_high)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("LEFT: <GAP (Deletion)>")
                                            .font(FontId::monospace(12.0))
                                            .color(hex_to_c32(&palette.diff_del)),
                                    );
                                }
                            }

                            ui.separator();

                            // Diff status
                            let (kind_txt, kind_col) = match (l_cell.map(|c| c.kind), r_cell.map(|c| c.kind)) {
                                (Some(DiffKind::Match), _) => ("MATCH (=)", hex_to_c32(&palette.diff_add)),
                                (Some(DiffKind::Mismatch), _) => ("MISMATCH (≠)", hex_to_c32(&palette.diff_mod)),
                                (Some(DiffKind::LeftOnly), _) => ("LEFT ONLY (DELETION)", hex_to_c32(&palette.diff_del)),
                                (Some(DiffKind::RightOnly), _) => ("RIGHT ONLY (INSERTION)", hex_to_c32(&palette.diff_add)),
                                _ => ("UNKNOWN", hex_to_c32(&palette.theme.f_med)),
                            };
                            ui.label(
                                egui::RichText::new(format!("DIFF: {}", kind_txt))
                                    .font(FontId::monospace(12.0))
                                    .strong()
                                    .color(kind_col),
                            );

                            ui.separator();

                            // Right cell details
                            if let Some(c) = r_cell {
                                if let Some(b) = c.byte_val {
                                    let off_str = c.offset.map(|o| format!("0x{:08X}", o)).unwrap_or_else(|| "none".to_string());
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "RIGHT: 0x{:02X} (Dec: {}, Bin: {:08b}, ASCII: '{}', Off: {})",
                                            b, b, b, byte_to_ascii_char(b), off_str
                                        ))
                                        .font(FontId::monospace(12.0))
                                        .color(hex_to_c32(&palette.theme.f_high)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("RIGHT: <GAP (Insertion)>")
                                            .font(FontId::monospace(12.0))
                                            .color(hex_to_c32(&palette.diff_add)),
                                    );
                                }
                            }
                        });
                    });
            }
        }
    }
}

/// Very lightly colors each byte along a smooth, subtle gradient:
/// 0x00: slightly blue (140, 175, 230)
/// 0x7F: slightly green (140, 220, 165)
/// 0xFF: slightly red (230, 150, 150)
pub fn byte_subtle_color(b: u8) -> Color32 {
    if b <= 127 {
        let t = b as f32 / 127.0;
        let r = 140.0;
        let g = 175.0 + (220.0 - 175.0) * t;
        let b_val = 230.0 + (165.0 - 230.0) * t;
        Color32::from_rgb(r as u8, g as u8, b_val as u8)
    } else {
        let t = (b - 127) as f32 / 128.0;
        let r = 140.0 + (230.0 - 140.0) * t;
        let g = 220.0 + (150.0 - 220.0) * t;
        let b_val = 165.0 + (150.0 - 165.0) * t;
        Color32::from_rgb(r as u8, g as u8, b_val as u8)
    }
}

/// Dark opaque background colors for Background mode to ensure high contrast with light text:
/// 0x00: deep navy (25, 40, 75)
/// 0x7F: deep forest (25, 55, 40)
/// 0xFF: deep wine (65, 25, 25)
pub fn byte_subtle_bg_opaque(b: u8) -> Color32 {
    if b <= 127 {
        let t = b as f32 / 127.0;
        let r = 25.0;
        let g = 40.0 + (55.0 - 40.0) * t;
        let b_val = 75.0 + (40.0 - 75.0) * t;
        Color32::from_rgb(r as u8, g as u8, b_val as u8)
    } else {
        let t = (b - 127) as f32 / 128.0;
        let r = 25.0 + (65.0 - 25.0) * t;
        let g = 55.0 + (25.0 - 55.0) * t;
        let b_val = 40.0 + (25.0 - 40.0) * t;
        Color32::from_rgb(r as u8, g as u8, b_val as u8)
    }
}

fn format_cell_hex(
    cell: &AlignedCell,
    is_left: bool,
    color_mode: ByteColorMode,
    palette: &AppPalette,
) -> (String, Color32, Option<Color32>) {
    match (cell.byte_val, cell.kind) {
        (Some(b), DiffKind::Match) => match color_mode {
            ByteColorMode::Digits => (format!("{:02X}", b), byte_subtle_color(b), None),
            ByteColorMode::Background => {
                let bg = byte_subtle_bg_opaque(b);
                (
                    format!("{:02X}", b),
                    crate::contrast_text_c32(bg, None, palette),
                    Some(bg),
                )
            }
        },
        (Some(b), DiffKind::Mismatch) => match color_mode {
            ByteColorMode::Digits => (format!("{:02X}", b), hex_to_c32(&palette.diff_mod), None),
            ByteColorMode::Background => {
                let bg = hex_to_c32(&palette.diff_mod);
                (
                    format!("{:02X}", b),
                    crate::contrast_text_c32(bg, None, palette),
                    Some(bg),
                )
            }
        },
        (Some(b), DiffKind::LeftOnly) => {
            if is_left {
                match color_mode {
                    ByteColorMode::Digits => {
                        (format!("{:02X}", b), hex_to_c32(&palette.diff_del), None)
                    }
                    ByteColorMode::Background => {
                        let bg = hex_to_c32(&palette.diff_del);
                        (
                            format!("{:02X}", b),
                            crate::contrast_text_c32(bg, None, palette),
                            Some(bg),
                        )
                    }
                }
            } else {
                ("--".to_string(), hex_to_c32(&palette.theme.f_low), None)
            }
        }
        (Some(b), DiffKind::RightOnly) => {
            if !is_left {
                match color_mode {
                    ByteColorMode::Digits => {
                        (format!("{:02X}", b), hex_to_c32(&palette.diff_add), None)
                    }
                    ByteColorMode::Background => {
                        let bg = hex_to_c32(&palette.diff_add);
                        (
                            format!("{:02X}", b),
                            crate::contrast_text_c32(bg, None, palette),
                            Some(bg),
                        )
                    }
                }
            } else {
                ("--".to_string(), hex_to_c32(&palette.theme.f_low), None)
            }
        }
        (None, _) => ("--".to_string(), hex_to_c32(&palette.theme.f_low), None),
    }
}

fn format_cell_ascii(
    cell: &AlignedCell,
    is_left: bool,
    color_mode: ByteColorMode,
    palette: &AppPalette,
) -> (char, Color32) {
    match (cell.byte_val, cell.kind) {
        (Some(b), DiffKind::Match) => {
            let col = match color_mode {
                ByteColorMode::Digits => byte_subtle_color(b),
                ByteColorMode::Background => hex_to_c32(&palette.theme.f_med),
            };
            (byte_to_ascii_char(b), col)
        }
        (Some(b), DiffKind::Mismatch) => (byte_to_ascii_char(b), hex_to_c32(&palette.diff_mod)),
        (Some(b), DiffKind::LeftOnly) => {
            if is_left {
                (byte_to_ascii_char(b), hex_to_c32(&palette.diff_del))
            } else {
                ('·', hex_to_c32(&palette.theme.f_low))
            }
        }
        (Some(b), DiffKind::RightOnly) => {
            if !is_left {
                (byte_to_ascii_char(b), hex_to_c32(&palette.diff_add))
            } else {
                ('·', hex_to_c32(&palette.theme.f_low))
            }
        }
        (None, _) => ('·', hex_to_c32(&palette.theme.f_low)),
    }
}

fn format_bytes_size(len: usize) -> String {
    if len < 1024 {
        format!("{} B", len)
    } else if len < 1024 * 1024 {
        format!("{:.1} KB", len as f64 / 1024.0)
    } else {
        format!("{:.2} MB", len as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_subtle_color_endpoints_and_interpolation() {
        // 0x00: slightly blue
        let c_00 = byte_subtle_color(0x00);
        assert_eq!(c_00, Color32::from_rgb(140, 175, 230));

        // 0x7F: slightly green
        let c_7f = byte_subtle_color(0x7F);
        assert_eq!(c_7f, Color32::from_rgb(140, 220, 165));

        // 0xFF: slightly red
        let c_ff = byte_subtle_color(0xFF);
        assert_eq!(c_ff, Color32::from_rgb(230, 150, 150));
    }

    #[test]
    fn test_binary_diff_view_state_creation_and_recompute() {
        let mut state = BinaryDiffViewState::new(
            "file_a.bin".to_string(),
            b"HELLO WORLD BINARY TEST".to_vec(),
            "file_b.bin".to_string(),
            b"HELLO RUSTY BINARY TEST".to_vec(),
        );

        // Wait for worker thread result
        if let Some(rx) = state.receiver.take() {
            let res = rx.recv().expect("computed result");
            state.result = Some(res);
            state.is_computing = false;
        }

        assert!(state.result.is_some());
        let res = state.result.as_ref().unwrap();
        assert_eq!(res.left_label, "file_a.bin");
        assert_eq!(res.right_label, "file_b.bin");
        assert!(!res.rows.is_empty());
        assert!(!res.diff_blocks.is_empty());

        // Test search
        state.search_query = "RUSTY".to_string();
        state.update_search();
        assert!(!state.search_matches.is_empty());

        // Test navigation
        state.next_diff();
        assert!(state.scroll_target_row.is_some());
        state.prev_diff();

        // Test swap operands
        state.swap_operands();
        assert_eq!(state.left_label, "file_b.bin");
        assert_eq!(state.right_label, "file_a.bin");

        // Drain recomputed worker thread
        if let Some(rx) = state.receiver.take() {
            let res = rx.recv().expect("swapped result");
            state.result = Some(res);
            state.is_computing = false;
        }
        let res = state.result.as_ref().unwrap();
        assert_eq!(res.left_label, "file_b.bin");
        assert_eq!(res.right_label, "file_a.bin");

        // Test toggle mode
        assert_eq!(state.mode, BinaryDiffMode::Aligned);
        state.toggle_mode();
        assert_eq!(state.mode, BinaryDiffMode::Raw);
    }

    #[test]
    fn test_cell_formatting() {
        let palette = AppPalette::default();
        let match_cell = AlignedCell {
            offset: Some(0),
            byte_val: Some(0x41),
            kind: DiffKind::Match,
        };
        // Digits mode: text is colored, no background pill
        let (hex, color, bg) = format_cell_hex(&match_cell, true, ByteColorMode::Digits, &palette);
        assert_eq!(hex, "41");
        assert_eq!(color, byte_subtle_color(0x41));
        assert!(bg.is_none());

        // Background mode: background pill is colored, text is light neutral
        let (hex_bg, color_bg, pill) =
            format_cell_hex(&match_cell, true, ByteColorMode::Background, &palette);
        assert_eq!(hex_bg, "41");
        assert_eq!(color_bg, hex_to_c32(&palette.theme.f_high));
        assert!(pill.is_some());

        let (ascii, a_color) =
            format_cell_ascii(&match_cell, true, ByteColorMode::Digits, &palette);
        assert_eq!(ascii, 'A');
        assert_eq!(a_color, byte_subtle_color(0x41));

        let gap_cell = AlignedCell {
            offset: None,
            byte_val: None,
            kind: DiffKind::LeftOnly,
        };
        let (gap_hex, _, _) = format_cell_hex(&gap_cell, false, ByteColorMode::Digits, &palette);
        assert_eq!(gap_hex, "--");
        let (gap_ascii, _) = format_cell_ascii(&gap_cell, false, ByteColorMode::Digits, &palette);
        assert_eq!(gap_ascii, '·');
    }

    #[test]
    fn test_layout_fits_window() {
        let palette = AppPalette::default();
        for bpr in [8, 16, 32] {
            let mut state = BinaryDiffViewState::new(
                "file_a.bin".to_string(),
                vec![0u8; 1000],
                "file_b.bin".to_string(),
                vec![1u8; 1000],
            );
            let res = little_clip_ops::binary_diff::compute_binary_diff(
                "file_a.bin",
                &[0u8; 1000],
                "file_b.bin",
                &[1u8; 1000],
                BinaryDiffMode::Aligned,
                bpr,
            );
            state.result = Some(res);
            state.is_computing = false;
            state.bytes_per_row = bpr;

            let ctx = egui::Context::default();
            // Frame 1
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(1180.0, 680.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(ctx, |ui| {
                            let mut close_req = false;
                            state.render_ui(ui, ctx, &palette, &mut close_req);
                        });
                },
            );

            // Frame 2
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(1180.0, 680.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(ctx, |ui| {
                            let mut close_req = false;
                            state.render_ui(ui, ctx, &palette, &mut close_req);
                            println!("[TEST BPR={}] min_rect: {:?}", bpr, ui.min_rect());
                            assert!(
                                ui.min_rect().max.x <= 1180.5,
                                "bpr={} overflows width: max.x={}",
                                bpr,
                                ui.min_rect().max.x
                            );
                        });
                },
            );
        }
    }
}
