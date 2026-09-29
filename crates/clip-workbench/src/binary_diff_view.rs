//! Side-by-side hex dump and binary diff visualizer workbench view.
//!
//! Renders aligned hex grids, diff blocks, ASCII previews, and interactive navigation
//! across binary byte buffers.

use crate::icons::Icon;
use crate::shell::{EscapeHatch, PanelState, StudioShell};
use crate::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, FontId, Rounding, Stroke, Vec2};
use little_clip_core::palette::AppPalette;
use little_clip_core::{BackgroundJobs, JobTicket};
use little_clip_ops::binary_diff::{
    AlignedCell, BinaryDiffMode, BinaryDiffResult, DiffKind, byte_to_ascii_char,
    compute_binary_diff, search_rows,
};
use std::sync::mpsc::Receiver;

struct HandoffCancelGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for HandoffCancelGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn stage_binary_handoff(bytes: &[u8], side: &str) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "clip_bin_{}_{}.bin",
        uuid::Uuid::new_v4().simple(),
        side
    ));
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Failed to write {side} temp file: {error}"))?;
    Ok(path)
}

fn launch_binary_diff_tool_with(
    left_path: &std::path::Path,
    right_path: &std::path::Path,
    mut available: impl FnMut(&str) -> Option<std::path::PathBuf>,
    mut spawn: impl FnMut(&std::path::Path, &std::path::Path, Option<&std::path::Path>) -> bool,
) -> Result<String, String> {
    let mut launch_failures = Vec::new();
    if let Some(app) = available("imhex") {
        if spawn(&app, left_path, Some(right_path)) {
            return Ok("🚀 Launched ImHex hex editor".to_string());
        }
        launch_failures.push("imhex");
    }
    if let Some(app) = available("xdg-open") {
        if spawn(&app, left_path, None) {
            return Ok("🚀 Opened left binary file with system default".to_string());
        }
        launch_failures.push("xdg-open");
    }
    if launch_failures.is_empty() {
        Ok("⚠️ Neither ImHex nor xdg-open is installed".to_string())
    } else {
        Err(format!(
            "Failed to launch available external tool(s): {}",
            launch_failures.join(", ")
        ))
    }
}

/// Color highlighting mode for hexadecimal byte displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteColorMode {
    /// Tint individual hex digit characters.
    Digits,
    /// Tint the entire background bounding box of the byte.
    Background,
}

struct HexHeaderColumns {
    offset: f32,
    hex: f32,
    ascii: f32,
    diff: f32,
}

/// Pure, testable state machine for diff block and search match navigation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BinaryDiffNavigation {
    /// Zero-based index into the current diff block list.
    pub current_diff_idx: usize,
    /// Identifier of the currently highlighted diff block, if any.
    pub selected_diff_block: Option<usize>,
    /// Row index target to smoothly scroll the hex viewport to.
    pub scroll_target_row: Option<usize>,
    /// Zero-based index into the search results row vector.
    pub current_search_idx: usize,
    /// List of row indices matching the current hex/ASCII search query.
    pub search_matches: Vec<usize>,
}

impl BinaryDiffNavigation {
    /// Advances forward to the next diff block, wrapping around if at the end.
    pub fn next_diff(&mut self, diff_blocks: &[little_clip_ops::binary_diff::BinaryDiffBlock]) {
        if diff_blocks.is_empty() {
            return;
        }
        self.current_diff_idx = (self.current_diff_idx + 1) % diff_blocks.len();
        let block = &diff_blocks[self.current_diff_idx];
        self.selected_diff_block = Some(block.id);
        self.scroll_target_row = Some(block.start_row);
    }

    /// Moves backward to the previous diff block, wrapping around to the end.
    pub fn prev_diff(&mut self, diff_blocks: &[little_clip_ops::binary_diff::BinaryDiffBlock]) {
        if diff_blocks.is_empty() {
            return;
        }
        if self.current_diff_idx == 0 {
            self.current_diff_idx = diff_blocks.len() - 1;
        } else {
            self.current_diff_idx -= 1;
        }
        let block = &diff_blocks[self.current_diff_idx];
        self.selected_diff_block = Some(block.id);
        self.scroll_target_row = Some(block.start_row);
    }

    /// Updates the list of search matches and positions on the first match.
    pub fn set_search_matches(&mut self, matches: Vec<usize>) {
        self.search_matches = matches;
        self.current_search_idx = 0;
        self.scroll_target_row = self.search_matches.first().copied();
    }

    /// Advances forward to the next search match row, wrapping around if at the end.
    pub fn next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.current_search_idx = (self.current_search_idx + 1) % self.search_matches.len();
        self.scroll_target_row = Some(self.search_matches[self.current_search_idx]);
    }

    /// Moves backward to the previous search match row, wrapping around to the end.
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
}

/// UI and navigation state for the binary hex diff viewer.
pub struct BinaryDiffViewState {
    pub nav: BinaryDiffNavigation,
    pub left_label: String,
    pub left_bytes: std::sync::Arc<Vec<u8>>,
    pub right_label: String,
    pub right_bytes: std::sync::Arc<Vec<u8>>,
    pub mode: BinaryDiffMode,
    pub color_mode: ByteColorMode,
    pub bytes_per_row: usize,
    pub result: Option<BinaryDiffResult>,
    pub is_computing: bool,
    pub current_generation: u64,
    pub receiver: Option<Receiver<Result<(u64, BinaryDiffResult), little_clip_core::JobError>>>,
    compute_ticket: Option<JobTicket>,
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
    handoff: Option<(
        JobTicket,
        Receiver<Result<Result<String, String>, little_clip_core::JobError>>,
    )>,
    handoff_cancel: Option<HandoffCancelGuard>,
}

impl Drop for BinaryDiffViewState {
    fn drop(&mut self) {
        if let Some(ticket) = &self.compute_ticket {
            ticket.cancel();
        }
    }
}

fn submit_binary_diff(
    generation: u64,
    left_label: String,
    left_bytes: std::sync::Arc<Vec<u8>>,
    right_label: String,
    right_bytes: std::sync::Arc<Vec<u8>>,
    mode: BinaryDiffMode,
    bytes_per_row: usize,
    jobs: &BackgroundJobs,
) -> (
    JobTicket,
    Receiver<Result<(u64, BinaryDiffResult), little_clip_core::JobError>>,
) {
    jobs.submit(std::time::Duration::MAX, move || {
        let result = compute_binary_diff(
            &left_label,
            left_bytes.as_ref(),
            &right_label,
            right_bytes.as_ref(),
            mode,
            bytes_per_row,
        );
        (generation, result)
    })
}

impl BinaryDiffViewState {
    /// Creates a new binary diff viewer state and spawns background diff computation.
    pub fn new(
        left_label: String,
        left_bytes: Vec<u8>,
        right_label: String,
        right_bytes: Vec<u8>,
    ) -> Self {
        Self::new_with_jobs(
            left_label,
            left_bytes,
            right_label,
            right_bytes,
            BackgroundJobs::global(),
        )
    }

    fn new_with_jobs(
        left_label: String,
        left_bytes: Vec<u8>,
        right_label: String,
        right_bytes: Vec<u8>,
        jobs: &BackgroundJobs,
    ) -> Self {
        let mode = BinaryDiffMode::Aligned;
        let bytes_per_row = 32;
        let generation = 1;
        let left_bytes = std::sync::Arc::new(left_bytes);
        let right_bytes = std::sync::Arc::new(right_bytes);
        let (ticket, rx) = submit_binary_diff(
            generation,
            left_label.clone(),
            left_bytes.clone(),
            right_label.clone(),
            right_bytes.clone(),
            mode,
            bytes_per_row,
            jobs,
        );

        Self {
            nav: BinaryDiffNavigation::default(),
            left_label,
            left_bytes,
            right_label,
            right_bytes,
            mode,
            color_mode: ByteColorMode::Digits,
            bytes_per_row,
            result: None,
            is_computing: true,
            current_generation: generation,
            receiver: Some(rx),
            compute_ticket: Some(ticket),
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
            handoff: None,
            handoff_cancel: None,
        }
    }

    pub fn recompute(&mut self) {
        if let Some(ticket) = self.compute_ticket.take() {
            ticket.cancel();
        }
        self.is_computing = true;
        self.current_generation = self.current_generation.wrapping_add(1);
        let generation = self.current_generation;
        let (ticket, rx) = submit_binary_diff(
            generation,
            self.left_label.clone(),
            self.left_bytes.clone(),
            self.right_label.clone(),
            self.right_bytes.clone(),
            self.mode,
            self.bytes_per_row,
            BackgroundJobs::global(),
        );
        self.compute_ticket = Some(ticket);
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

    fn set_bytes_per_row(&mut self, bytes_per_row: usize) {
        if self.bytes_per_row != bytes_per_row {
            self.bytes_per_row = bytes_per_row;
            self.recompute();
        }
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context, close_requested: &mut bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close_requested = true;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            self.toggle_mode();
        }
        if ctx.input(|i| {
            i.key_pressed(egui::Key::S)
                && !i.modifiers.command
                && !i.modifiers.ctrl
                && !i.modifiers.mac_cmd
                && !i.modifiers.shift
                && !i.modifiers.alt
        }) {
            self.swap_operands();
        }
        if ctx.input(|i| {
            i.key_pressed(egui::Key::J)
                || i.key_pressed(egui::Key::ArrowRight)
                || i.key_pressed(egui::Key::ArrowDown)
        }) {
            self.next_diff();
        }
        if ctx.input(|i| {
            i.key_pressed(egui::Key::K)
                || i.key_pressed(egui::Key::ArrowLeft)
                || i.key_pressed(egui::Key::ArrowUp)
        }) {
            self.prev_diff();
        }
        let bytes_per_row = ctx.input(|i| {
            if i.key_pressed(egui::Key::Num0) {
                Some(8)
            } else if i.key_pressed(egui::Key::Num1) {
                Some(16)
            } else if i.key_pressed(egui::Key::Num2) {
                Some(32)
            } else {
                None
            }
        });
        if let Some(bytes_per_row) = bytes_per_row {
            self.set_bytes_per_row(bytes_per_row);
        }
    }

    pub fn next_diff(&mut self) {
        if let Some(ref res) = self.result {
            self.nav.next_diff(&res.diff_blocks);
            self.current_diff_idx = self.nav.current_diff_idx;
            self.selected_diff_block = self.nav.selected_diff_block;
            self.scroll_target_row = self.nav.scroll_target_row;
        }
    }

    pub fn prev_diff(&mut self) {
        if let Some(ref res) = self.result {
            self.nav.prev_diff(&res.diff_blocks);
            self.current_diff_idx = self.nav.current_diff_idx;
            self.selected_diff_block = self.nav.selected_diff_block;
            self.scroll_target_row = self.nav.scroll_target_row;
        }
    }

    pub fn update_search(&mut self) {
        if let Some(ref res) = self.result {
            let matches = search_rows(res, &self.search_query);
            self.nav.set_search_matches(matches);
            self.search_matches = self.nav.search_matches.clone();
            self.current_search_idx = self.nav.current_search_idx;
            self.scroll_target_row = self.nav.scroll_target_row;
        }
    }

    pub fn next_search_match(&mut self) {
        self.nav.next_search_match();
        self.current_search_idx = self.nav.current_search_idx;
        self.scroll_target_row = self.nav.scroll_target_row;
    }

    pub fn prev_search_match(&mut self) {
        self.nav.prev_search_match();
        self.current_search_idx = self.nav.current_search_idx;
        self.scroll_target_row = self.nav.scroll_target_row;
    }

    /// Keeps byte hit targets and row geometry in step with the readable monospace font.
    /// Wider rows pan horizontally instead of shrinking their contents.
    /// Returns: (cell_w, cell_h, font_size, ascii_w, offset_w, row_height)
    pub fn layout_metrics(&self) -> (f32, f32, f32, f32, f32, f32) {
        let font_size = crate::ui_ext::MONO_FONT_SIZE;
        let glyph_width = font_size * 0.6;
        (
            glyph_width * 2.0 + 6.0,
            font_size + 11.0,
            font_size,
            glyph_width + 3.0,
            glyph_width * 9.0 + 9.0,
            font_size + 13.0,
        )
    }

    /// Triggers opening the binary comparison in ImHex hex editor.
    pub fn trigger_escape_hatch(&mut self, now: f64) {
        if let Some((ticket, _)) = self.handoff.take() {
            ticket.cancel();
        }
        self.handoff_cancel.take();
        let left = self.left_bytes.clone();
        let right = self.right_bytes.clone();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job_cancelled = cancelled.clone();
        let (ticket, rx) =
            BackgroundJobs::global().submit(std::time::Duration::from_secs(30), move || {
                let temp_left = stage_binary_handoff(left.as_slice(), "lhs")?;
                let temp_right = stage_binary_handoff(right.as_slice(), "rhs")?;
                if job_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("External handoff cancelled".to_string());
                }
                launch_binary_diff_tool_with(
                    &temp_left,
                    &temp_right,
                    |command| little_clip_core::command_available(command),
                    |app, left, right| {
                        let mut command = std::process::Command::new(app);
                        command.arg(left);
                        if let Some(right) = right {
                            command.arg(right);
                        }
                        command.spawn().is_ok()
                    },
                )
            });
        self.status_toast
            .set("Preparing binary files for external tool…", now);
        self.handoff = Some((ticket, rx));
        self.handoff_cancel = Some(HandoffCancelGuard(cancelled));
    }

    fn poll_handoff(&mut self, now: f64, ctx: &egui::Context) {
        let Some((_, rx)) = &self.handoff else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Ok(message))) => {
                self.status_toast.set(message, now);
                self.handoff = None;
            }
            Ok(Ok(Err(error))) => {
                self.status_toast.set(format!("❌ {error}"), now);
                self.handoff = None;
            }
            Ok(Err(error)) => {
                self.status_toast
                    .set(format!("❌ External handoff failed: {error}"), now);
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_toast
                    .set("❌ External handoff was cancelled", now);
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(80))
            }
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
        self.poll_handoff(now, ctx);

        // Check async receiver
        if self.is_computing {
            ctx.request_repaint();
            if let Some(ref rx) = self.receiver {
                match rx.try_recv() {
                    Ok(Ok((req_gen, res))) if req_gen == self.current_generation => {
                        self.result = Some(res);
                        self.is_computing = false;
                        self.receiver = None;
                        self.compute_ticket = None;
                        if !self.search_query.is_empty() {
                            self.update_search();
                        }
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        self.status_toast
                            .set(format!("❌ Binary diff failed: {error}"), now);
                        self.is_computing = false;
                        self.receiver = None;
                        self.compute_ticket = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.status_toast
                            .set("❌ Binary diff worker disconnected", now);
                        self.is_computing = false;
                        self.receiver = None;
                        self.compute_ticket = None;
                    }
                }
            }
        }

        self.handle_global_shortcuts(ctx, close_requested);

        let l_size_str = format_bytes_size(self.left_bytes.len());
        let r_size_str = format_bytes_size(self.right_bytes.len());
        let hatch = EscapeHatch::new("ImHex", "imhex", "Open in ImHex Hex Editor")
            .with_icon(Icon::BinaryDiff);

        let mut shell = StudioShell::new("Binary Diff", Icon::BinaryDiff, palette, close_requested)
            .with_badge(format!("Left: {} ({})", self.left_label, l_size_str), None)
            .with_badge(format!("Right: {} ({})", self.right_label, r_size_str), None)
            .with_escape_hatch(hatch)
            .with_footer_hints("Navigation: [Esc] Close • [Tab] Mode • [S] Swap • [J/K/←/↑/→/↓] Diff Nav • [0/1/2] Width • [Search] Hex/ASCII");

        if self.is_computing {
            shell = shell.with_badge("Computing...", Some(hex_to_c32(&palette.harmonies.base)));
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
                    format!("{} diffs", res.diff_blocks.len()),
                    Some(hex_to_c32(&palette.diff_del)),
                )
                .with_footer_readout(format!(
                    "{} vs {} • Block count: {}",
                    res.left_label,
                    res.right_label,
                    res.diff_blocks.len()
                ));
        }

        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = crate::ui_ext::secondary_button_icon(Some(Icon::Swap), "Swap [S]", hex_to_c32(&palette.colors.surface_panel), palette);
                    if ui.add(swap_btn).on_hover_text("Swap operands (S)").clicked() {
                        self.swap_operands();
                    }

                    let copy_btn = crate::ui_ext::secondary_button_icon(Some(Icon::Copy), "Summary", hex_to_c32(&palette.colors.surface_panel), palette);
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

                    let diff_panel_label = if self.panel_state.right_collapsed { "Diffs (Show)" } else { "Diffs (Hide)" };
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

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 2: Mode & Filter Bar
                self.render_mode_and_stats_bar(ui, palette);

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

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
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let f_low = hex_to_c32(&palette.colors.text_muted);

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
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::SPACE_MD,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
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
                            .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                            .color(f_low),
                    );
                    for &w in &[8, 16, 32] {
                        let is_active = self.bytes_per_row == w;
                        let btn = crate::mode_tab_button(format!("{}B", w), is_active, palette);
                        if ui.add(btn).clicked() {
                            self.set_bytes_per_row(w);
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

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Stat pills
                    crate::ui_ext::stat_pill(
                        ui,
                        "Match",
                        match_count,
                        Some(hex_to_c32(&palette.colors.text_secondary)),
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
                        let next_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::ArrowDown),
                            "Next [J]",
                            b_med,
                            palette,
                        );
                        if ui.add(next_btn).clicked() {
                            next_diff = true;
                        }
                        let prev_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::ArrowUp),
                            "Prev [K]",
                            b_med,
                            palette,
                        );
                        if ui.add(prev_btn).clicked() {
                            prev_diff = true;
                        }

                        ui.add_space(crate::ui_ext::SPACE_SM);

                        if search_len > 0 {
                            let canvas = hex_to_c32(&palette.colors.canvas);
                            ui.label(
                                egui::RichText::new(format!("{}/{}", search_idx + 1, search_len))
                                    .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                    .color(crate::ui_ext::contrast_text_c32_on(
                                        canvas,
                                        canvas,
                                        Some(hex_to_c32(&palette.harmonies.base)),
                                        palette,
                                    )),
                            );
                            let down_btn = crate::ui_ext::icon_button(
                                Icon::ArrowDown,
                                14.0,
                                b_med,
                                None,
                                palette,
                            );
                            if ui.add(down_btn).clicked() {
                                next_search = true;
                            }
                            let up_btn = crate::ui_ext::icon_button(
                                Icon::ArrowUp,
                                14.0,
                                b_med,
                                None,
                                palette,
                            );
                            if ui.add(up_btn).clicked() {
                                prev_search = true;
                            }
                        }

                        let search_edit = ui.add_sized(
                            [180.0, crate::ui_ext::CONTROL_HEIGHT],
                            egui::TextEdit::singleline(&mut self.search_query)
                                .id(egui::Id::new("binary_diff_search_field"))
                                .hint_text("Search Hex / ASCII..."),
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
                        .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                        .color(crate::ui_ext::contrast_text_c32_on(
                            hex_to_c32(&palette.colors.canvas),
                            hex_to_c32(&palette.colors.canvas),
                            Some(hex_to_c32(&palette.harmonies.base)),
                            palette,
                        )),
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

        let center_w = 42.0;
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

        render_hex_table_header(
            ui,
            palette,
            res,
            self.current_scroll_offset_x,
            HexHeaderColumns {
                offset: offset_w,
                hex: hex_w,
                ascii: ascii_w,
                diff: center_w,
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
                    hex_to_c32_alpha(&palette.colors.text_muted, 8)
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
                        Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.harmonies.base, 100)),
                    );
                    ui.painter().line_segment(
                        [
                            actual_row_rect.left_bottom(),
                            actual_row_rect.right_bottom(),
                        ],
                        Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.harmonies.base, 100)),
                    );
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                    ui.add_space(crate::ui_ext::SPACE_XS);

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
                                    .font(FontId::monospace(font_size))
                                    .color(hex_to_c32(&palette.colors.text_muted)),
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
                                Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                            );
                        }

                        if r.hovered() {
                            hovered = Some((row_idx, col_idx));
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);

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
                                Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
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
                        _ => hex_to_c32(&palette.colors.text_muted),
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
                                Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                            );
                        }

                        if r.hovered() {
                            hovered = Some((row_idx, col_idx));
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);

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
                                Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
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
                                    .font(FontId::monospace(font_size))
                                    .color(hex_to_c32(&palette.colors.text_muted)),
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
        ui.painter().rect_filled(
            rect,
            Rounding::same(3.0),
            hex_to_c32(&palette.colors.surface_recessed),
        );
        ui.painter().rect_stroke(
            rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
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
            hex_to_c32_alpha(&palette.colors.text_primary, 25),
        );
        ui.painter().rect_stroke(
            view_rect,
            Rounding::same(2.0),
            Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
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
                        .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                        .color(hex_to_c32(&palette.colors.text_primary)),
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
                    .fill(hex_to_c32(&palette.colors.surface_recessed))
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Diffs ({})", res.diff_blocks.len()))
                                    .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                                    .strong()
                                    .color(crate::ui_ext::contrast_text_c32_on(
                                        hex_to_c32(&palette.colors.surface_recessed),
                                        hex_to_c32(&palette.colors.surface_recessed),
                                        Some(hex_to_c32(&palette.diff_mod)),
                                        palette,
                                    )),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let next_btn = crate::ui_ext::icon_button(
                                        Icon::ArrowDown,
                                        14.0,
                                        hex_to_c32(&palette.colors.surface_panel),
                                        None,
                                        palette,
                                    );
                                    if ui
                                        .add(next_btn)
                                        .on_hover_text("Next Difference [J]")
                                        .clicked()
                                    {
                                        do_next = true;
                                    }
                                    let prev_btn = crate::ui_ext::icon_button(
                                        Icon::ArrowUp,
                                        14.0,
                                        hex_to_c32(&palette.colors.surface_panel),
                                        None,
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
                            egui::RichText::new("Files identical")
                                .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                                .color(crate::ui_ext::contrast_text_c32_on(
                                    hex_to_c32(&palette.colors.canvas),
                                    hex_to_c32(&palette.colors.canvas),
                                    Some(hex_to_c32(&palette.diff_add)),
                                    palette,
                                )),
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
                                DiffKind::Match => hex_to_c32(&palette.colors.text_secondary),
                            };
                            let card_fill = if is_selected {
                                hex_to_c32_alpha(&palette.harmonies.base, 35)
                            } else {
                                hex_to_c32(&palette.colors.surface_recessed)
                            };
                            let badge_text = crate::ui_ext::contrast_text_c32_on(
                                card_fill,
                                hex_to_c32(&palette.colors.canvas),
                                Some(badge_color),
                                palette,
                            );

                            let frame = egui::Frame::none()
                                .fill(card_fill)
                                .stroke(if is_selected {
                                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base))
                                } else {
                                    Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel))
                                })
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
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
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("#{}", block.id))
                                                    .font(FontId::monospace(
                                                        crate::ui_ext::MONO_FONT_SIZE,
                                                    ))
                                                    .strong()
                                                    .color(hex_to_c32(
                                                        &palette.colors.text_secondary,
                                                    )),
                                            );

                                            ui.label(
                                                egui::RichText::new(&block.kind_label)
                                                    .font(FontId::proportional(
                                                        crate::ui_ext::BODY_FONT_SIZE,
                                                    ))
                                                    .strong()
                                                    .color(badge_text),
                                            );

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{} B",
                                                            block.byte_count
                                                        ))
                                                        .font(FontId::monospace(
                                                            crate::ui_ext::MONO_FONT_SIZE,
                                                        ))
                                                        .strong()
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
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
                                                .font(FontId::monospace(
                                                    crate::ui_ext::MONO_FONT_SIZE,
                                                ))
                                                .color(hex_to_c32(&palette.colors.text_muted)),
                                        );

                                        // Row 3: Preview
                                        ui.label(
                                            egui::RichText::new(&block.preview)
                                                .font(FontId::monospace(
                                                    crate::ui_ext::MONO_FONT_SIZE,
                                                ))
                                                .color(hex_to_c32(&palette.colors.text_primary)),
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
                    .fill(hex_to_c32(&palette.colors.surface_recessed))
                    .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .show(ui, |ui| {
                        let inspection_surface = hex_to_c32(&palette.colors.surface_recessed);
                        let inspection_text = |preferred| {
                            crate::ui_ext::contrast_text_c32_on(
                                inspection_surface,
                                inspection_surface,
                                Some(preferred),
                                palette,
                            )
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new("INSPECT: ")
                                    .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                                    .strong()
                                    .color(inspection_text(hex_to_c32(&palette.harmonies.base))),
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
                                        .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                        .color(hex_to_c32(&palette.colors.text_primary)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("LEFT: <GAP (Deletion)>")
                                            .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                            .color(inspection_text(hex_to_c32(&palette.diff_del))),
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
                                _ => ("UNKNOWN", hex_to_c32(&palette.colors.text_secondary)),
                            };
                            ui.label(
                                egui::RichText::new(format!("DIFF: {}", kind_txt))
                                    .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                    .strong()
                                    .color(inspection_text(kind_col)),
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
                                        .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                        .color(hex_to_c32(&palette.colors.text_primary)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("RIGHT: <GAP (Insertion)>")
                                            .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                            .color(inspection_text(hex_to_c32(&palette.diff_add))),
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

fn render_hex_table_header(
    ui: &mut egui::Ui,
    palette: &AppPalette,
    res: &BinaryDiffResult,
    scroll_x: f32,
    columns: HexHeaderColumns,
) {
    // Table column headers
    let (header_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), egui::Sense::hover());
    ui.painter().rect_filled(
        header_rect,
        Rounding::ZERO,
        hex_to_c32(&palette.colors.surface_recessed),
    );
    let header_ui_rect = egui::Rect::from_min_max(
        egui::pos2(header_rect.min.x + 4.0 - scroll_x, header_rect.min.y),
        egui::pos2(
            header_rect.min.x + 4.0 - scroll_x + 3500.0,
            header_rect.max.y,
        ),
    );
    let mut child_ui = ui.child_ui(
        header_ui_rect,
        egui::Layout::left_to_right(egui::Align::Center),
    );
    child_ui.set_clip_rect(header_rect);
    child_ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);

    let diff_header_color = crate::ui_ext::contrast_text_c32_on(
        hex_to_c32(&palette.colors.surface_recessed),
        hex_to_c32(&palette.colors.surface_recessed),
        Some(hex_to_c32(&palette.diff_mod)),
        palette,
    );
    let diff_header_hex = format!(
        "#{:02X}{:02X}{:02X}",
        diff_header_color.r(),
        diff_header_color.g(),
        diff_header_color.b()
    );
    let headers = [
        (
            "Offset L".to_owned(),
            columns.offset,
            &palette.colors.text_muted,
            false,
        ),
        (
            format!("Hex Bytes ({})", res.left_label),
            columns.hex,
            &palette.colors.text_secondary,
            false,
        ),
        (
            "ASCII".to_owned(),
            columns.ascii,
            &palette.colors.text_secondary,
            true,
        ),
        ("Diff".to_owned(), columns.diff, &diff_header_hex, false),
        (
            format!("Hex Bytes ({})", res.right_label),
            columns.hex,
            &palette.colors.text_secondary,
            false,
        ),
        (
            "ASCII".to_owned(),
            columns.ascii,
            &palette.colors.text_secondary,
            true,
        ),
        (
            "Offset R".to_owned(),
            columns.offset,
            &palette.colors.text_muted,
            false,
        ),
    ];
    for (label, width, color, gap_before) in headers {
        if gap_before {
            child_ui.add_space(crate::ui_ext::SPACE_SM);
        }
        child_ui.allocate_ui_with_layout(
            Vec2::new(width, 28.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                        .strong()
                        .color(hex_to_c32(color)),
                );
            },
        );
    }
}

fn format_cell_hex(
    cell: &AlignedCell,
    is_left: bool,
    color_mode: ByteColorMode,
    palette: &AppPalette,
) -> (String, Color32, Option<Color32>) {
    let surface = hex_to_c32(&palette.colors.canvas);
    let readable =
        |preferred| crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), palette);
    match (cell.byte_val, cell.kind) {
        (Some(b), DiffKind::Match) => match color_mode {
            ByteColorMode::Digits => (format!("{:02X}", b), readable(byte_subtle_color(b)), None),
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
            ByteColorMode::Digits => (
                format!("{:02X}", b),
                readable(hex_to_c32(&palette.diff_mod)),
                None,
            ),
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
                    ByteColorMode::Digits => (
                        format!("{:02X}", b),
                        readable(hex_to_c32(&palette.diff_del)),
                        None,
                    ),
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
                (
                    "--".to_string(),
                    hex_to_c32(&palette.colors.text_muted),
                    None,
                )
            }
        }
        (Some(b), DiffKind::RightOnly) => {
            if !is_left {
                match color_mode {
                    ByteColorMode::Digits => (
                        format!("{:02X}", b),
                        readable(hex_to_c32(&palette.diff_add)),
                        None,
                    ),
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
                (
                    "--".to_string(),
                    hex_to_c32(&palette.colors.text_muted),
                    None,
                )
            }
        }
        (None, _) => (
            "--".to_string(),
            hex_to_c32(&palette.colors.text_muted),
            None,
        ),
    }
}

fn format_cell_ascii(
    cell: &AlignedCell,
    is_left: bool,
    color_mode: ByteColorMode,
    palette: &AppPalette,
) -> (char, Color32) {
    let surface = hex_to_c32(&palette.colors.canvas);
    let readable =
        |preferred| crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), palette);
    match (cell.byte_val, cell.kind) {
        (Some(b), DiffKind::Match) => {
            let col = match color_mode {
                ByteColorMode::Digits => readable(byte_subtle_color(b)),
                ByteColorMode::Background => hex_to_c32(&palette.colors.text_secondary),
            };
            (byte_to_ascii_char(b), col)
        }
        (Some(b), DiffKind::Mismatch) => (
            byte_to_ascii_char(b),
            readable(hex_to_c32(&palette.diff_mod)),
        ),
        (Some(b), DiffKind::LeftOnly) => {
            if is_left {
                (
                    byte_to_ascii_char(b),
                    readable(hex_to_c32(&palette.diff_del)),
                )
            } else {
                ('·', hex_to_c32(&palette.colors.text_muted))
            }
        }
        (Some(b), DiffKind::RightOnly) => {
            if !is_left {
                (
                    byte_to_ascii_char(b),
                    readable(hex_to_c32(&palette.diff_add)),
                )
            } else {
                ('·', hex_to_c32(&palette.colors.text_muted))
            }
        }
        (None, _) => ('·', hex_to_c32(&palette.colors.text_muted)),
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
    fn binary_handoff_preserves_exact_bytes() {
        let bytes = [0, 255, 0, 0x7f, 13, 10];
        let path = stage_binary_handoff(&bytes, "lhs").unwrap();
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("bin"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binary_external_tool_prefers_imhex_then_uses_xdg_open_fallback() {
        let left = std::path::PathBuf::from("/private/left.bin");
        let right = std::path::PathBuf::from("/private/right.bin");

        let mut lookups = Vec::new();
        let mut launches = Vec::new();
        let result = launch_binary_diff_tool_with(
            &left,
            &right,
            |command| {
                lookups.push(command.to_string());
                Some(std::path::PathBuf::from(format!("/fake/{command}")))
            },
            |app, left_arg, right_arg| {
                launches.push((
                    app.file_name().unwrap().to_string_lossy().into_owned(),
                    left_arg.to_path_buf(),
                    right_arg.map(std::path::Path::to_path_buf),
                ));
                true
            },
        );
        assert_eq!(result, Ok("🚀 Launched ImHex hex editor".to_string()));
        assert_eq!(lookups, vec!["imhex"]);
        assert_eq!(
            launches,
            vec![("imhex".to_string(), left.clone(), Some(right.clone()))]
        );

        let mut lookups = Vec::new();
        let mut launches = Vec::new();
        let result = launch_binary_diff_tool_with(
            &left,
            &right,
            |command| {
                lookups.push(command.to_string());
                (command == "xdg-open").then(|| std::path::PathBuf::from("/fake/xdg-open"))
            },
            |app, left_arg, right_arg| {
                launches.push((
                    app.file_name().unwrap().to_string_lossy().into_owned(),
                    left_arg.to_path_buf(),
                    right_arg.map(std::path::Path::to_path_buf),
                ));
                true
            },
        );
        assert_eq!(
            result,
            Ok("🚀 Opened left binary file with system default".to_string())
        );
        assert_eq!(lookups, vec!["imhex", "xdg-open"]);
        assert_eq!(launches, vec![("xdg-open".to_string(), left.clone(), None)]);

        let mut launches = Vec::new();
        let result = launch_binary_diff_tool_with(
            &left,
            &right,
            |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
            |app, left_arg, right_arg| {
                launches.push(app.file_name().unwrap().to_string_lossy().into_owned());
                let _ = (left_arg, right_arg);
                app.file_name().is_some_and(|name| name == "xdg-open")
            },
        );
        assert_eq!(
            result,
            Ok("🚀 Opened left binary file with system default".to_string())
        );
        assert_eq!(launches, vec!["imhex", "xdg-open"]);

        let mut attempts = Vec::new();
        let result = launch_binary_diff_tool_with(
            &left,
            &right,
            |command| (command == "xdg-open").then(|| std::path::PathBuf::from("/fake/xdg-open")),
            |app, _, _| {
                attempts.push(app.file_name().unwrap().to_string_lossy().into_owned());
                false
            },
        );
        assert_eq!(
            result,
            Err("Failed to launch available external tool(s): xdg-open".to_string())
        );
        assert_eq!(attempts, vec!["xdg-open"]);

        let mut attempts = Vec::new();
        let result = launch_binary_diff_tool_with(
            &left,
            &right,
            |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
            |app, _, _| {
                attempts.push(app.file_name().unwrap().to_string_lossy().into_owned());
                false
            },
        );
        assert_eq!(
            result,
            Err("Failed to launch available external tool(s): imhex, xdg-open".to_string())
        );
        assert_eq!(attempts, vec!["imhex", "xdg-open"]);

        let mut lookups = Vec::new();
        let result = launch_binary_diff_tool_with(
            &left,
            &right,
            |command| {
                lookups.push(command.to_string());
                None
            },
            |_, _, _| panic!("spawn must not run when no opener is installed"),
        );
        assert_eq!(
            result,
            Ok("⚠️ Neither ImHex nor xdg-open is installed".to_string())
        );
        assert_eq!(lookups, vec!["imhex", "xdg-open"]);
    }

    #[test]
    fn dropping_handoff_guard_cancels_stale_publication() {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        drop(HandoffCancelGuard(cancelled.clone()));
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    }

    fn trigger_global_shortcut(state: &mut BinaryDiffViewState, key: egui::Key) {
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let mut close_requested = false;
        state.handle_global_shortcuts(&ctx, &mut close_requested);
        let _ = ctx.end_frame();
        assert!(!close_requested);
    }

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
    fn binary_cell_text_meets_contrast_for_all_byte_values_and_modes() {
        fn contrast(a: Color32, b: Color32) -> f32 {
            little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
        }

        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = AppPalette::for_appearance(appearance);
            let surface = hex_to_c32(&palette.colors.canvas);
            for byte in 0..=u8::MAX {
                for kind in [
                    DiffKind::Match,
                    DiffKind::Mismatch,
                    DiffKind::LeftOnly,
                    DiffKind::RightOnly,
                ] {
                    let cell = AlignedCell {
                        byte_val: Some(byte),
                        offset: Some(byte as usize),
                        kind,
                    };
                    for color_mode in [ByteColorMode::Digits, ByteColorMode::Background] {
                        for is_left in [true, false] {
                            let (hex, foreground, fill) =
                                format_cell_hex(&cell, is_left, color_mode, &palette);
                            let background = fill.unwrap_or(surface);
                            assert!(
                                contrast(background, foreground) >= 4.5,
                                "{appearance:?} byte {byte:02X} {kind:?} {color_mode:?} hex label lacks contrast"
                            );

                            let (character, ascii_foreground) =
                                format_cell_ascii(&cell, is_left, color_mode, &palette);
                            if character != '·' {
                                assert!(
                                    contrast(surface, ascii_foreground) >= 4.5,
                                    "{appearance:?} byte {byte:02X} {kind:?} {color_mode:?} ASCII label lacks contrast"
                                );
                            }
                            assert!(!hex.is_empty());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn binary_diff_status_accents_meet_contrast_on_canvas_and_inspector() {
        fn contrast(a: Color32, b: Color32) -> f32 {
            little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
        }

        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = AppPalette::for_appearance(appearance);
            let canvas = hex_to_c32(&palette.colors.canvas);
            let recessed = hex_to_c32(&palette.colors.surface_recessed);
            let status_accents = [
                palette.harmonies.base.as_str(),
                palette.diff_add.as_str(),
                palette.diff_del.as_str(),
                palette.diff_mod.as_str(),
            ];

            for accent in status_accents {
                let preferred = hex_to_c32(accent);
                let on_canvas =
                    crate::ui_ext::contrast_text_c32_on(canvas, canvas, Some(preferred), &palette);
                let on_inspector = crate::ui_ext::contrast_text_c32_on(
                    recessed,
                    recessed,
                    Some(preferred),
                    &palette,
                );
                assert!(
                    contrast(canvas, on_canvas) >= 4.5,
                    "{appearance:?} {accent} on canvas"
                );
                assert!(
                    contrast(recessed, on_inspector) >= 4.5,
                    "{appearance:?} {accent} on inspector"
                );
            }
        }
    }

    #[test]
    fn binary_difference_panel_labels_meet_contrast_in_both_appearances() {
        fn contrast(a: Color32, b: Color32) -> f32 {
            little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
        }

        fn composite(fill: Color32, surface: Color32) -> Color32 {
            let fg = egui::Rgba::from(fill);
            let bg = egui::Rgba::from(surface);
            let remaining = 1.0 - fg.a();
            Color32::from(egui::Rgba::from_rgb(
                fg.r() + bg.r() * remaining,
                fg.g() + bg.g() * remaining,
                fg.b() + bg.b() * remaining,
            ))
        }

        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = AppPalette::for_appearance(appearance);
            let canvas = hex_to_c32(&palette.colors.canvas);
            let recessed = hex_to_c32(&palette.colors.surface_recessed);
            let header_color = crate::ui_ext::contrast_text_c32_on(
                recessed,
                recessed,
                Some(hex_to_c32(&palette.diff_mod)),
                &palette,
            );
            let identical_color = crate::ui_ext::contrast_text_c32_on(
                canvas,
                canvas,
                Some(hex_to_c32(&palette.diff_add)),
                &palette,
            );
            assert!(
                contrast(recessed, header_color) >= 4.5,
                "{appearance:?} panel heading"
            );
            assert!(
                contrast(canvas, identical_color) >= 4.5,
                "{appearance:?} identical status"
            );

            for kind in [
                DiffKind::Mismatch,
                DiffKind::LeftOnly,
                DiffKind::RightOnly,
                DiffKind::Match,
            ] {
                let preferred = match kind {
                    DiffKind::Mismatch => hex_to_c32(&palette.diff_mod),
                    DiffKind::LeftOnly => hex_to_c32(&palette.diff_del),
                    DiffKind::RightOnly => hex_to_c32(&palette.diff_add),
                    DiffKind::Match => hex_to_c32(&palette.colors.text_secondary),
                };
                for selected in [false, true] {
                    let fill = if selected {
                        hex_to_c32_alpha(&palette.harmonies.base, 35)
                    } else {
                        recessed
                    };
                    let label = crate::ui_ext::contrast_text_c32_on(
                        fill,
                        canvas,
                        Some(preferred),
                        &palette,
                    );
                    assert!(
                        contrast(composite(fill, canvas), label) >= 4.5,
                        "{appearance:?} {kind:?} selected={selected} badge"
                    );
                }
            }
        }
    }

    #[test]
    fn binary_diff_ordinary_chrome_renders_within_compact_and_wide_hosts() {
        let left = b"header shared-left tail".to_vec();
        let right = b"header altered-right tail".to_vec();
        let mut state = BinaryDiffViewState::new(
            "left-source.bin".into(),
            left.clone(),
            "right-source.bin".into(),
            right.clone(),
        );
        state.result = Some(compute_binary_diff(
            "left-source.bin",
            &left,
            "right-source.bin",
            &right,
            BinaryDiffMode::Aligned,
            8,
        ));
        state.is_computing = false;
        state.receiver = None;
        state.search_query = "header".into();
        state.update_search();
        state.hovered_cell = Some((0, 0));

        let palette = AppPalette::default();
        for width in [1200.0, 1760.0] {
            let ctx = egui::Context::default();
            let host = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 760.0));
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(host),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );

            let visible_text: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some((
                        text.galley.text().to_string(),
                        clipped.clip_rect,
                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    _ => None,
                })
                .collect();
            for label in [
                "Binary Diff",
                "left-source.bin",
                "right-source.bin",
                "Swap [S]",
                "Summary",
                "Diffs (Hide)",
                "Mode: Aligned [Tab]",
                "Width:",
                "8B",
                "16B",
                "32B",
                "Digits",
                "Bg",
                "Prev [K]",
                "Next [J]",
                "1/1",
                "header",
                "#1",
                "Replacement",
                "9 B",
                "0x000000",
                "shle ≠ lter",
                "INSPECT: ",
            ] {
                let (clip_rect, text_rect) = visible_text
                    .iter()
                    .find_map(|(text, clip_rect, text_rect)| {
                        (text == label
                            || (matches!(label, "left-source.bin" | "right-source.bin")
                                && text.contains(label)))
                        .then_some((*clip_rect, *text_rect))
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "{label:?} missing at host width {width}; rendered: {:?}",
                            visible_text
                                .iter()
                                .map(|(text, _, _)| text)
                                .collect::<Vec<_>>()
                        )
                    });
                assert!(
                    host.contains_rect(text_rect),
                    "{label:?} outside {width}-point host: {text_rect:?}"
                );
                assert!(
                    clip_rect.contains_rect(text_rect),
                    "{label:?} elided/clipped at {width} points: text={text_rect:?}, clip={clip_rect:?}"
                );
            }

            let footer_hint = visible_text
                .iter()
                .find(|(text, _, _)| text.contains("Navigation: [Esc] Close"))
                .unwrap_or_else(|| panic!("navigation footer missing at host width {width}"));
            assert!(
                host.contains_rect(footer_hint.2),
                "navigation footer outside {width}-point host: {:?}",
                footer_hint.2
            );
            assert!(
                footer_hint.1.contains_rect(footer_hint.2),
                "navigation footer clipped at {width} points: text={:?}, clip={:?}",
                footer_hint.2,
                footer_hint.1
            );
        }
    }

    #[test]
    fn test_binary_diff_view_state_creation_and_recompute() {
        let mut state = BinaryDiffViewState::new(
            "file_a.bin".to_string(),
            b"HELLO WORLD BINARY TEST".to_vec(),
            "file_b.bin".to_string(),
            b"HELLO RUSTY BINARY TEST".to_vec(),
        );
        assert_eq!(state.bytes_per_row, 32);

        // Wait for worker thread result
        if let Some(rx) = state.receiver.take() {
            let (_gen, res) = rx.recv().expect("job result").expect("computed result");
            state.result = Some(res);
            state.is_computing = false;
            state.compute_ticket = None;
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
            let (_gen, res) = rx.recv().expect("job result").expect("swapped result");
            state.result = Some(res);
            state.is_computing = false;
            state.compute_ticket = None;
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
    fn binary_diff_tab_shortcut_recomputes_raw_result() {
        let mut state = BinaryDiffViewState::new(
            "left.bin".to_string(),
            b"A B C D".to_vec(),
            "right.bin".to_string(),
            b"A X B C D".to_vec(),
        );
        let initial_rx = state.receiver.take().expect("initial compute receiver");
        let (initial_generation, initial_result) = initial_rx
            .recv()
            .expect("initial job result")
            .expect("initial diff result");
        assert_eq!(initial_generation, state.current_generation);
        assert_eq!(initial_result.mode, BinaryDiffMode::Aligned);
        state.result = Some(initial_result);
        state.is_computing = false;
        state.compute_ticket = None;

        let previous_generation = state.current_generation;
        let ctx = egui::Context::default();
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| state.handle_global_shortcuts(ctx, &mut close),
        );

        assert!(!close);
        assert_eq!(state.mode, BinaryDiffMode::Raw);
        assert_eq!(state.current_generation, previous_generation + 1);
        assert!(state.is_computing);
        assert_eq!(state.result.as_ref().unwrap().mode, BinaryDiffMode::Aligned);

        let recompute_rx = state.receiver.take().expect("raw recompute receiver");
        let (generation, recomputed) = recompute_rx
            .recv()
            .expect("recompute job result")
            .expect("raw diff result");
        assert_eq!(generation, state.current_generation);
        assert_eq!(recomputed.mode, BinaryDiffMode::Raw);
        assert_eq!(recomputed.left_label, "left.bin");
        assert_eq!(recomputed.right_label, "right.bin");
        state.result = Some(recomputed);
        state.is_computing = false;
        state.compute_ticket = None;
        assert_eq!(state.result.as_ref().unwrap().mode, BinaryDiffMode::Raw);
    }

    #[test]
    fn binary_diff_width_shortcuts_recompute_each_requested_width() {
        let left_bytes: Vec<u8> = (0..48).collect();
        let mut right_bytes = left_bytes.clone();
        right_bytes[17] = 250;
        let mut state = BinaryDiffViewState::new(
            "left.bin".to_string(),
            left_bytes,
            "right.bin".to_string(),
            right_bytes,
        );
        let initial_rx = state.receiver.take().expect("initial compute receiver");
        let (initial_generation, initial_result) = initial_rx
            .recv()
            .expect("initial job result")
            .expect("initial diff result");
        assert_eq!(initial_generation, state.current_generation);
        assert_eq!(initial_result.bytes_per_row, 32);
        state.result = Some(initial_result);
        state.is_computing = false;
        state.compute_ticket = None;

        for (key, expected_width) in [
            (egui::Key::Num0, 8),
            (egui::Key::Num1, 16),
            (egui::Key::Num2, 32),
        ] {
            let previous_generation = state.current_generation;
            let previous_width = state.result.as_ref().unwrap().bytes_per_row;
            trigger_global_shortcut(&mut state, key);
            assert_eq!(state.bytes_per_row, expected_width);
            assert_eq!(state.current_generation, previous_generation + 1);
            assert!(state.is_computing);
            assert_eq!(state.result.as_ref().unwrap().bytes_per_row, previous_width);

            let rx = state.receiver.take().expect("width recompute receiver");
            let (generation, result) = rx
                .recv()
                .expect("width recompute job result")
                .expect("width recompute result");
            assert_eq!(generation, state.current_generation);
            assert_eq!(result.bytes_per_row, expected_width);
            assert_eq!(result.mode, BinaryDiffMode::Aligned);
            state.result = Some(result);
            state.is_computing = false;
            state.compute_ticket = None;
        }
    }

    #[test]
    fn binary_diff_s_shortcut_swaps_operands_and_recomputes_result() {
        let left_bytes = vec![0xA1, 0xA2, 0xA3];
        let right_bytes = vec![0xB1, 0xB2, 0xB3];
        let mut state = BinaryDiffViewState::new(
            "left.bin".to_string(),
            left_bytes.clone(),
            "right.bin".to_string(),
            right_bytes.clone(),
        );
        let initial_rx = state.receiver.take().expect("initial compute receiver");
        let (initial_generation, initial_result) = initial_rx
            .recv()
            .expect("initial job result")
            .expect("initial diff result");
        assert_eq!(initial_generation, state.current_generation);
        state.result = Some(initial_result);
        state.is_computing = false;
        state.compute_ticket = None;

        let previous_generation = state.current_generation;
        trigger_global_shortcut(&mut state, egui::Key::S);
        assert_eq!(state.left_label, "right.bin");
        assert_eq!(state.right_label, "left.bin");
        assert_eq!(&*state.left_bytes, &right_bytes);
        assert_eq!(&*state.right_bytes, &left_bytes);
        assert_eq!(state.current_generation, previous_generation + 1);
        assert!(state.is_computing);
        assert_eq!(state.result.as_ref().unwrap().left_label, "left.bin");

        let recompute_rx = state.receiver.take().expect("swapped compute receiver");
        let (generation, result) = recompute_rx
            .recv()
            .expect("recompute job result")
            .expect("swapped diff result");
        assert_eq!(generation, state.current_generation);
        assert_eq!(result.left_label, "right.bin");
        assert_eq!(result.right_label, "left.bin");
        assert_eq!(result.bytes_per_row, state.bytes_per_row);
        assert_eq!(
            result
                .rows
                .first()
                .unwrap()
                .left_cells
                .first()
                .unwrap()
                .byte_val,
            Some(right_bytes[0])
        );
        assert_eq!(
            result
                .rows
                .first()
                .unwrap()
                .right_cells
                .first()
                .unwrap()
                .byte_val,
            Some(left_bytes[0])
        );
        state.result = Some(result);
        state.is_computing = false;
        state.compute_ticket = None;
    }

    #[test]
    fn modified_s_does_not_swap_binary_diff_operands() {
        let left_bytes = vec![0xA1, 0xA2, 0xA3];
        let right_bytes = vec![0xB1, 0xB2, 0xB3];
        let mut state = BinaryDiffViewState::new(
            "left.bin".to_string(),
            left_bytes.clone(),
            "right.bin".to_string(),
            right_bytes.clone(),
        );
        let initial_rx = state.receiver.take().expect("initial compute receiver");
        let (generation, result) = initial_rx
            .recv()
            .expect("initial job result")
            .expect("initial diff result");
        assert_eq!(generation, state.current_generation);
        state.result = Some(result);
        state.is_computing = false;
        state.compute_ticket = None;

        let baseline_generation = state.current_generation;
        let baseline_result = state.result.as_ref().unwrap();
        let baseline_left_label = baseline_result.left_label.clone();
        let baseline_right_label = baseline_result.right_label.clone();
        for modifiers in [
            egui::Modifiers::CTRL,
            egui::Modifiers::COMMAND,
            egui::Modifiers::MAC_CMD,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
            egui::Modifiers::ALT,
        ] {
            let ctx = egui::Context::default();
            let mut close = false;
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key: egui::Key::S,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| state.handle_global_shortcuts(ctx, &mut close),
            );

            assert!(!close);
            assert_eq!(state.left_label, "left.bin");
            assert_eq!(state.right_label, "right.bin");
            assert_eq!(&*state.left_bytes, &left_bytes);
            assert_eq!(&*state.right_bytes, &right_bytes);
            assert_eq!(state.current_generation, baseline_generation);
            assert!(!state.is_computing);
            let result = state.result.as_ref().unwrap();
            assert_eq!(result.left_label, baseline_left_label);
            assert_eq!(result.right_label, baseline_right_label);
        }
    }

    #[test]
    fn binary_diff_queue_saturation_is_visible_and_recovers() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let _queued = jobs.submit(Duration::MAX, || ());

        let mut state = BinaryDiffViewState::new_with_jobs(
            "left.bin".into(),
            b"left".to_vec(),
            "right.bin".into(),
            b"right".to_vec(),
            &jobs,
        );
        assert!(state.is_computing);

        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let expected = "Binary diff failed: background job queue is full";
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1000.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );

        assert!(!state.is_computing);
        assert!(state.receiver.is_none());
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains(expected))
        }));
        assert_eq!(
            state.status_toast.message(0.0),
            Some(&format!("❌ {expected}")[..])
        );

        let _ = release_tx.send(());
    }

    #[test]
    fn dropping_binary_diff_viewer_cancels_queued_compute() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let mut state = BinaryDiffViewState::new_with_jobs(
            "left.bin".into(),
            b"left".to_vec(),
            "right.bin".into(),
            b"right".to_vec(),
            &jobs,
        );
        let result_rx = state.receiver.take().unwrap();
        drop(state);

        let _ = release_tx.send(());
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
    }

    #[test]
    fn test_global_shortcuts_navigate_diffs_and_switch_widths() {
        let left_bytes = vec![0; 24];
        let mut right_bytes = left_bytes.clone();
        right_bytes[0] = 1;
        right_bytes[16] = 1;
        let mut state = BinaryDiffViewState::new(
            "file_a.bin".to_string(),
            left_bytes.clone(),
            "file_b.bin".to_string(),
            right_bytes.clone(),
        );
        state.result = Some(compute_binary_diff(
            "file_a.bin",
            &left_bytes,
            "file_b.bin",
            &right_bytes,
            BinaryDiffMode::Aligned,
            8,
        ));
        state.is_computing = false;
        let diff_blocks = state.result.as_ref().unwrap().diff_blocks.clone();
        assert!(diff_blocks.len() >= 2);

        for key in [egui::Key::J, egui::Key::ArrowRight, egui::Key::ArrowDown] {
            state.nav.current_diff_idx = 0;
            state.current_diff_idx = 0;
            state.nav.selected_diff_block = None;
            state.nav.scroll_target_row = None;
            trigger_global_shortcut(&mut state, key);
            assert_eq!(state.current_diff_idx, 1);
            assert_eq!(state.nav.selected_diff_block, Some(diff_blocks[1].id));
            assert_eq!(state.selected_diff_block, Some(diff_blocks[1].id));
            assert_eq!(state.nav.scroll_target_row, Some(diff_blocks[1].start_row));
            assert_eq!(state.scroll_target_row, Some(diff_blocks[1].start_row));
        }
        for key in [egui::Key::K, egui::Key::ArrowLeft, egui::Key::ArrowUp] {
            state.nav.current_diff_idx = 1;
            state.current_diff_idx = 1;
            state.nav.selected_diff_block = None;
            state.nav.scroll_target_row = None;
            trigger_global_shortcut(&mut state, key);
            assert_eq!(state.current_diff_idx, 0);
            assert_eq!(state.nav.selected_diff_block, Some(diff_blocks[0].id));
            assert_eq!(state.selected_diff_block, Some(diff_blocks[0].id));
            assert_eq!(state.nav.scroll_target_row, Some(diff_blocks[0].start_row));
            assert_eq!(state.scroll_target_row, Some(diff_blocks[0].start_row));
        }
        for (key, expected_width) in [
            (egui::Key::Num0, 8),
            (egui::Key::Num1, 16),
            (egui::Key::Num2, 32),
        ] {
            trigger_global_shortcut(&mut state, key);
            assert_eq!(state.bytes_per_row, expected_width);
        }
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
        assert_eq!(color_bg, hex_to_c32(&palette.colors.text_primary));
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

    #[test]
    fn horizontal_pan_keeps_hex_header_offset_and_hover_targets_aligned() {
        let left: Vec<u8> = (0x41..0x61).collect();
        let mut right = left.clone();
        right[0] = b'Z';
        right[20] ^= 0x20;
        let mut state = BinaryDiffViewState::new(
            "lhs.bin".into(),
            left.clone(),
            "rhs.bin".into(),
            right.clone(),
        );
        state.result = Some(compute_binary_diff(
            "lhs.bin",
            &left,
            "rhs.bin",
            &right,
            BinaryDiffMode::Aligned,
            32,
        ));
        state.is_computing = false;
        state.receiver = None;
        state.panel_state.right_collapsed = true;

        let palette = AppPalette::default();
        let ctx = egui::Context::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1100.0, 760.0));
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            (output, state.current_scroll_offset_x, state.hovered_cell)
        };

        let (initial, initial_scroll_x, _) = render(Vec::new());
        let find_text_pos = |output: &egui::FullOutput, text: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(galley) if galley.galley.text() == text => {
                        Some(galley.pos + galley.galley.size() * 0.5)
                    }
                    _ => None,
                })
        };
        let find_header_pos = |output: &egui::FullOutput, text: &str| {
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(galley) if galley.galley.text() == text => Some(galley.pos),
                    _ => None,
                })
                .min_by(|a, b| a.x.total_cmp(&b.x))
        };
        let initial_viewport = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "41" => Some(clipped.clip_rect),
                _ => None,
            })
            .expect("first hex byte should render in the table viewport");
        let initial_hex_header = find_header_pos(&initial, "Hex Bytes (lhs.bin)")
            .expect("left hex column header should render");
        let initial_offset = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().trim() == "00000000" => {
                    Some(text.pos)
                }
                _ => None,
            })
            .expect("left row offset should render");
        assert_eq!(initial_scroll_x, 0.0);

        let pointer = initial_viewport.center();
        let _ = render(vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::Scroll(Vec2::new(-40.0, 0.0)),
        ]);
        let mut settled = None;
        for _ in 0..10 {
            settled = Some(render(Vec::new()));
        }
        let (hex_scrolled, hex_scroll_x, _) = settled.unwrap();
        assert!(
            hex_scroll_x > 10.0,
            "horizontal scroll offset: {hex_scroll_x}"
        );

        let scrolled_hex_header = find_header_pos(&hex_scrolled, "Hex Bytes (lhs.bin)")
            .expect("hex header should track the horizontal viewport");
        let scrolled_offset = hex_scrolled
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().trim() == "00000000" => {
                    Some(text.pos)
                }
                _ => None,
            })
            .expect("row offset text should pan with its byte row");
        let scrolled_hex_cell = find_text_pos(&hex_scrolled, "55")
            .expect("a later hex byte should remain visible during horizontal scrolling");
        for (label, before, after) in [
            ("hex header", initial_hex_header.x, scrolled_hex_header.x),
            (
                "later hex byte",
                find_text_pos(&initial, "55").unwrap().x,
                scrolled_hex_cell.x,
            ),
            ("row offset", initial_offset.x, scrolled_offset.x),
        ] {
            assert!(
                ((after - before) + hex_scroll_x).abs() <= 3.0,
                "{label} should move with the horizontal content: {before} -> {after}, offset {hex_scroll_x}"
            );
        }

        let (hex_hovered_output, _, hex_hovered) =
            render(vec![egui::Event::PointerMoved(scrolled_hex_cell)]);
        assert_eq!(
            hex_hovered,
            Some((0, 20)),
            "later hex hit target should map to byte 20"
        );
        assert!(hex_hovered_output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text().contains("LEFT: 0x55") && text.galley.text().contains("Off: 0x00000014"))
        }), "inspection footer should report the hovered later hex byte and offset");

        // Pan farther in a second stage so the right ASCII section enters the
        // viewport while the earlier hex checks remain at their smaller offset.
        let _ = render(vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::Scroll(Vec2::new(-410.0, 0.0)),
        ]);
        let mut ascii_settled = None;
        for _ in 0..10 {
            ascii_settled = Some(render(Vec::new()));
        }
        let (ascii_scrolled, ascii_scroll_x, _) = ascii_settled.unwrap();

        let ascii_headers = |output: &egui::FullOutput| {
            let mut headers: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "ASCII" => {
                        Some((text.pos, text.galley.size(), clipped.clip_rect))
                    }
                    _ => None,
                })
                .collect();
            headers.sort_by(|left, right| left.0.x.total_cmp(&right.0.x));
            headers
        };
        let initial_ascii_headers = ascii_headers(&initial);
        let scrolled_ascii_headers = ascii_headers(&ascii_scrolled);
        assert_eq!(
            initial_ascii_headers.len(),
            2,
            "both ASCII headers should be laid out"
        );
        assert!(
            !scrolled_ascii_headers.is_empty(),
            "right ASCII header should remain laid out after panning"
        );
        // At this scroll position the left ASCII header has moved offscreen; the
        // surviving label belongs to the right ASCII block. Match it to the
        // rightmost initial header's expected translated x, not by sorted index.
        let initial_right_ascii_header = initial_ascii_headers[1];
        let expected_right_header_x = initial_right_ascii_header.0.x - ascii_scroll_x;
        let (right_ascii_header, header_size, header_clip) = scrolled_ascii_headers
            .iter()
            .copied()
            .min_by(|left, right| {
                (left.0.x - expected_right_header_x)
                    .abs()
                    .total_cmp(&(right.0.x - expected_right_header_x).abs())
            })
            .expect("right ASCII header should remain visible after panning");
        assert!(
            (right_ascii_header.x - expected_right_header_x).abs() <= 3.0,
            "right ASCII header should move by -scroll_x: {} -> {}, offset {}",
            initial_right_ascii_header.0.x,
            right_ascii_header.x,
            ascii_scroll_x
        );
        assert!(
            header_clip.contains_rect(egui::Rect::from_min_size(right_ascii_header, header_size)),
            "right ASCII header should be fully visible at x={}",
            right_ascii_header.x
        );

        let initial_right_ascii_byte = initial
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "F" && text.pos.x > initial_right_ascii_header.0.x =>
                {
                    Some((text.pos, text.galley.size()))
                }
                _ => None,
            })
            .min_by(|left, right| left.0.x.total_cmp(&right.0.x))
            .expect("right ASCII byte F should be laid out initially");
        let expected_byte_x = initial_right_ascii_byte.0.x - ascii_scroll_x;
        let ascii_byte = ascii_scrolled
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "F"
                        && clipped.clip_rect.contains_rect(egui::Rect::from_min_size(
                            text.pos,
                            text.galley.size(),
                        )) =>
                {
                    Some((text.pos, text.galley.size()))
                }
                _ => None,
            })
            .min_by(|left, right| {
                (left.0.x - expected_byte_x)
                    .abs()
                    .total_cmp(&(right.0.x - expected_byte_x).abs())
            })
            .filter(|(pos, _)| (pos.x - expected_byte_x).abs() <= 3.0)
            .map(|(pos, size)| pos + size * 0.5)
            .expect(
                "right ASCII byte F (column 5) should be fully visible in the intermediate view",
            );
        let (ascii_hovered_output, _, ascii_hovered) =
            render(vec![egui::Event::PointerMoved(ascii_byte)]);
        assert_eq!(
            ascii_hovered,
            Some((0, 5)),
            "right ASCII hit target should map to row 0, byte 5"
        );
        assert!(ascii_hovered_output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text().contains("RIGHT: 0x46") && text.galley.text().contains("Off: 0x00000005"))
        }), "inspection footer should report the hovered right ASCII byte and offset");
    }

    #[test]
    fn byte_density_does_not_shrink_text_or_hit_targets() {
        let mut state =
            BinaryDiffViewState::new("left".to_owned(), vec![0], "right".to_owned(), vec![1]);
        for bpr in [8, 16, 32] {
            state.bytes_per_row = bpr;
            let (cell_w, _, font_size, ascii_w, offset_w, row_height) = state.layout_metrics();
            assert_eq!(font_size, crate::ui_ext::MONO_FONT_SIZE);
            assert!(cell_w >= font_size * 1.2);
            assert!(ascii_w >= font_size * 0.6);
            assert!(offset_w >= font_size * 5.4);
            assert!(row_height >= font_size + 10.0);
        }
    }

    #[test]
    fn test_binary_diff_navigation_cycles_and_bounds() {
        use little_clip_ops::binary_diff::BinaryDiffBlock;

        let mut nav = BinaryDiffNavigation::default();
        let blocks = vec![
            BinaryDiffBlock {
                id: 10,
                start_row: 5,
                end_row: 6,
                left_offset: None,
                right_offset: None,
                byte_count: 2,
                kind: DiffKind::Mismatch,
                kind_label: "Mismatch".into(),
                preview: String::new(),
            },
            BinaryDiffBlock {
                id: 20,
                start_row: 15,
                end_row: 15,
                left_offset: None,
                right_offset: None,
                byte_count: 1,
                kind: DiffKind::RightOnly,
                kind_label: "Insertion".into(),
                preview: String::new(),
            },
            BinaryDiffBlock {
                id: 30,
                start_row: 25,
                end_row: 27,
                left_offset: None,
                right_offset: None,
                byte_count: 3,
                kind: DiffKind::LeftOnly,
                kind_label: "Replacement".into(),
                preview: String::new(),
            },
        ];

        // Empty block safeguard
        nav.next_diff(&[]);
        assert_eq!(nav.current_diff_idx, 0);
        assert_eq!(nav.selected_diff_block, None);

        // Next forward
        nav.next_diff(&blocks);
        assert_eq!(nav.current_diff_idx, 1);
        assert_eq!(nav.selected_diff_block, Some(20));
        assert_eq!(nav.scroll_target_row, Some(15));

        nav.next_diff(&blocks);
        assert_eq!(nav.current_diff_idx, 2);
        assert_eq!(nav.selected_diff_block, Some(30));
        assert_eq!(nav.scroll_target_row, Some(25));

        // Wrap forward
        nav.next_diff(&blocks);
        assert_eq!(nav.current_diff_idx, 0);
        assert_eq!(nav.selected_diff_block, Some(10));
        assert_eq!(nav.scroll_target_row, Some(5));

        // Wrap backward
        nav.prev_diff(&blocks);
        assert_eq!(nav.current_diff_idx, 2);
        assert_eq!(nav.selected_diff_block, Some(30));

        nav.prev_diff(&blocks);
        assert_eq!(nav.current_diff_idx, 1);
        assert_eq!(nav.selected_diff_block, Some(20));
    }

    #[test]
    fn test_binary_diff_search_navigation() {
        let mut nav = BinaryDiffNavigation::default();
        nav.set_search_matches(vec![3, 7, 12]);
        assert_eq!(nav.current_search_idx, 0);
        assert_eq!(nav.scroll_target_row, Some(3));

        nav.next_search_match();
        assert_eq!(nav.current_search_idx, 1);
        assert_eq!(nav.scroll_target_row, Some(7));

        nav.next_search_match();
        assert_eq!(nav.current_search_idx, 2);
        assert_eq!(nav.scroll_target_row, Some(12));

        // Wrap forward
        nav.next_search_match();
        assert_eq!(nav.current_search_idx, 0);
        assert_eq!(nav.scroll_target_row, Some(3));

        // Wrap backward
        nav.prev_search_match();
        assert_eq!(nav.current_search_idx, 2);
        assert_eq!(nav.scroll_target_row, Some(12));
    }

    #[test]
    fn binary_diff_search_field_updates_matches_and_navigates_while_focused() {
        let mut bytes = vec![b'x'; 96];
        bytes[1..4].copy_from_slice(b"ABC");
        bytes[65..68].copy_from_slice(b"ABC");
        let mut state = BinaryDiffViewState::new(
            "left.bin".to_string(),
            bytes.clone(),
            "right.bin".to_string(),
            bytes,
        );
        let initial_rx = state.receiver.take().expect("initial compute receiver");
        let (generation, result) = initial_rx
            .recv()
            .expect("initial job result")
            .expect("initial diff result");
        assert_eq!(generation, state.current_generation);
        state.result = Some(result);
        state.is_computing = false;
        state.compute_ticket = None;

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let search_id = egui::Id::new("binary_diff_search_field");
        ctx.memory_mut(|memory| memory.request_focus(search_id));
        let palette = AppPalette::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

        let mut keyboard_focus_kept = false;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                events: vec![egui::Event::Text("ABC".to_string())],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
                keyboard_focus_kept = ctx.wants_keyboard_input();
            },
        );
        assert!(keyboard_focus_kept);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
        assert_eq!(state.search_query, "ABC");
        assert_eq!(state.search_matches, vec![0, 2]);
        assert_eq!(state.current_search_idx, 0);
        assert_eq!(state.nav.scroll_target_row, Some(0));

        state.next_search_match();
        assert_eq!(state.current_search_idx, 1);
        assert_eq!(state.scroll_target_row, Some(2));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
        state.prev_search_match();
        assert_eq!(state.current_search_idx, 0);
        assert_eq!(state.scroll_target_row, Some(0));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
    }

    #[test]
    fn focused_binary_diff_search_preempts_global_shortcuts() {
        fn render_frame(
            state: &mut BinaryDiffViewState,
            ctx: &egui::Context,
            screen_rect: egui::Rect,
            events: Vec<egui::Event>,
        ) -> bool {
            let mut wants_keyboard_before_render = false;
            let palette = AppPalette::default();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    wants_keyboard_before_render = ctx.wants_keyboard_input();
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            wants_keyboard_before_render
        }

        let mut left_bytes = vec![b'x'; 96];
        left_bytes[1..4].copy_from_slice(b"ABC");
        left_bytes[65..68].copy_from_slice(b"ABC");
        let mut right_bytes = left_bytes.clone();
        right_bytes[20] = b'y';
        right_bytes[80] = b'z';
        let mut state = BinaryDiffViewState::new(
            "left.bin".to_string(),
            left_bytes,
            "right.bin".to_string(),
            right_bytes,
        );
        let initial_rx = state.receiver.take().expect("initial compute receiver");
        let (generation, result) = initial_rx
            .recv()
            .expect("initial job result")
            .expect("initial diff result");
        assert_eq!(generation, state.current_generation);
        state.result = Some(result);
        state.is_computing = false;
        state.compute_ticket = None;
        state.search_query = "ABC".to_string();
        state.update_search();
        assert_eq!(state.search_matches, vec![0, 2]);
        state.next_diff();
        assert!(state.selected_diff_block.is_some());

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let search_id = egui::Id::new("binary_diff_search_field");
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
        ctx.memory_mut(|memory| memory.request_focus(search_id));
        let _ = render_frame(&mut state, &ctx, screen_rect, Vec::new());
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

        let baseline = (
            state.mode,
            state.left_label.clone(),
            state.right_label.clone(),
            (*state.left_bytes).clone(),
            (*state.right_bytes).clone(),
            state.bytes_per_row,
            state.current_diff_idx,
            state.selected_diff_block,
            state.scroll_target_row,
        );
        let mut keys = vec![
            egui::Key::Tab,
            egui::Key::S,
            egui::Key::J,
            egui::Key::ArrowRight,
            egui::Key::ArrowDown,
            egui::Key::K,
            egui::Key::ArrowLeft,
            egui::Key::ArrowUp,
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
        ];

        for key in keys.drain(..) {
            ctx.memory_mut(|memory| memory.request_focus(search_id));
            let _ = render_frame(&mut state, &ctx, screen_rect, Vec::new());
            assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

            let wants_keyboard_before_render = render_frame(
                &mut state,
                &ctx,
                screen_rect,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(
                wants_keyboard_before_render,
                "{key:?} frame must enter with search focused"
            );
            assert_eq!(
                (
                    state.mode,
                    state.left_label.clone(),
                    state.right_label.clone(),
                    (*state.left_bytes).clone(),
                    (*state.right_bytes).clone(),
                    state.bytes_per_row,
                    state.current_diff_idx,
                    state.selected_diff_block,
                    state.scroll_target_row,
                ),
                baseline,
                "{key:?} must not run a global shortcut while search owns keyboard input"
            );
        }
    }

    #[test]
    fn test_stale_generation_discarded() {
        let mut state = BinaryDiffViewState::new(
            "file_a.bin".to_string(),
            b"A".to_vec(),
            "file_b.bin".to_string(),
            b"B".to_vec(),
        );

        let initial_gen = state.current_generation;
        assert_eq!(initial_gen, 1);

        state.recompute();
        assert_eq!(state.current_generation, 2);

        // Inject an artificial stale message from generation 1
        let (tx, rx) = std::sync::mpsc::channel::<
            Result<(u64, BinaryDiffResult), little_clip_core::JobError>,
        >();
        let dummy_res = compute_binary_diff("stale", b"A", "stale", b"B", BinaryDiffMode::Raw, 16);
        tx.send(Ok((1, dummy_res))).unwrap();
        state.receiver = Some(rx);

        // Poll receiver with generation 2 active
        if let Some(ref rx) = state.receiver {
            if let Ok(Ok((req_gen, res))) = rx.try_recv() {
                if req_gen == state.current_generation {
                    state.result = Some(res);
                }
            }
        }

        // Stale result should NOT have been applied
        assert!(state.result.is_none());
    }
}
