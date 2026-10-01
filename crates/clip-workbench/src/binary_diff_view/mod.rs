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
use cells::{format_bytes_size, format_cell_ascii, format_cell_hex, render_hex_table_header};
use handoff::*;

mod cells;
mod chrome;
mod handoff;
mod hex_table;
mod navigation;

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
        self.cancel_handoff();
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
            if self.search_query.is_empty() {
                *close_requested = true;
            } else {
                self.search_query.clear();
                self.update_search();
            }
            return;
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

    /// Fits the byte columns into the available width, shrinking their font when needed.
    /// Returns: (cell_w, cell_h, font_size, ascii_w, offset_w, row_height)
    pub fn layout_metrics(
        &self,
        bytes_per_row: usize,
        available_width: f32,
    ) -> (f32, f32, f32, f32, f32, f32) {
        let base_font_size = crate::ui_ext::MONO_FONT_SIZE;
        let base_glyph_width = base_font_size * 0.6;
        let bpr = bytes_per_row as f32;
        let groups = if bytes_per_row == 8 {
            1.0
        } else {
            (bytes_per_row / 8).saturating_sub(1) as f32
        };
        let base_cell_w = base_glyph_width * 2.0 + 6.0;
        let base_ascii_w = base_glyph_width + 3.0;
        let base_offset_w = base_glyph_width * 9.0 + 9.0;
        // Group breaks add their own widget spacing on both sides of the explicit gap.
        let hex_w = bpr * base_cell_w + (bpr - 1.0) * 2.0 + groups * 7.0;
        let ascii_w = bpr * base_ascii_w + (bpr - 1.0) * 2.0;
        let gaps = 42.0 + crate::ui_ext::SPACE_XS + crate::ui_ext::SPACE_SM * 2.0 + 18.0;
        let natural_width = base_offset_w * 2.0 + hex_w * 2.0 + ascii_w * 2.0 + gaps;
        let scale = (available_width.max(1.0) / natural_width).min(1.0);
        let font_size = base_font_size * scale;
        let glyph_width = font_size * 0.6;
        (
            glyph_width * 2.0 + 6.0 * scale,
            font_size + 11.0 * scale,
            font_size,
            glyph_width + 3.0 * scale,
            glyph_width * 9.0 + 9.0 * scale,
            // Text labels keep egui's minimum row height even when their font shrinks.
            // Keep virtual rows and stripe geometry in step with that fixed label padding.
            font_size + 13.0,
        )
    }
}

#[cfg(test)]
mod tests;
