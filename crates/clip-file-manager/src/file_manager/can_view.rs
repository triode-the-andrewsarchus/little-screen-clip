//! Link CAN activity, exact frame rows, and recorded payloads.

mod activity;
mod capture;
pub(super) mod strip;
mod table;

use super::{
    capture_browse::{
        Job, PAGE_ROWS, keyed_poll, poll,
        rows::{RowWindows, needed_window},
        start,
        table::Page,
    },
    preview_copy::Copies,
};
use crate::{file_source::ReadCheck, ui_appearance::WorkbenchColors};
use capture::{Capture, Selection};
use egui::{Context, PointerButton, Sense, Ui, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::can::{
        Direction, Filter, Filtered, Frame, FrameKind, Identifier, IdentifierSummary, Index,
        Summary,
    },
    palette::AppPalette,
};
use std::{
    ops::Range,
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(3, 5));
pub(super) struct Viewer {
    capture: Arc<Capture>,
    prefix: Summary,
    index: Option<Arc<Index>>,
    index_job: Option<Job<Index>>,
    index_failed: bool,
    windows: RowWindows<Frame>,
    detail: Option<Frame>,
    detail_job: Option<(u64, Job<Vec<Frame>>)>,
    detail_failed: Option<u64>,
    selected: Option<u64>,
    page: u64,
    jump_to: u64,
    scroll_to: Option<usize>,
    generation: u64,
    viewport: Option<activity::Domain>,
    brush_anchor: Option<i128>,
    selection: Option<Selection>,
    row_anchor: Option<(u64, Option<i128>)>,
    selected_row_span: Option<activity::Domain>,
    filter: Filter,
    id_input: String,
    filtered: Option<Filtered>,
    filter_job: Option<Job<Filtered>>,
    error: Option<String>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (capture, prefix) = Capture::open(path, cancel)?;
        let row_anchor = prefix
            .sample
            .first()
            .map(|frame| (frame.row, Some(frame.timestamp_ns)));
        let selected = row_anchor.map(|(row, _)| row);
        Ok(Self {
            capture: Arc::new(capture),
            prefix,
            selected,
            index: None,
            index_job: None,
            index_failed: false,
            windows: RowWindows::new(4),
            detail: None,
            detail_job: None,
            detail_failed: None,
            page: 0,
            jump_to: 1,
            scroll_to: None,
            generation: 0,
            viewport: None,
            brush_anchor: None,
            selection: None,
            row_anchor,
            selected_row_span: None,
            filter: Filter::default(),
            id_input: String::new(),
            filtered: None,
            filter_job: None,
            error: None,
        })
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        if let Some(capture) = Arc::get_mut(&mut self.capture) {
            capture.check = check;
        }
    }

    pub fn focus_frame(&mut self, ordinal: u64) {
        self.select(ordinal, false);
        self.reveal(ordinal);
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette, copies: &mut Copies) {
        self.poll();
        let colors = WorkbenchColors::from_palette(palette);
        let mut shell = crate::shell::StudioShell::new(
            "CAN capture",
            crate::icons::Icon::Crosshair,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.toolbar(ui, copies);
        self.filter_controls(ui);
        self.timeline(ui, colors);
        self.navigation(ui);
        self.browser(ui, colors);
        self.request_index(ui.ctx());
        self.request_detail(ui.ctx());
        if self.index_job.is_some()
            || self.windows.pending()
            || self.detail_job.is_some()
            || self.filter_job.is_some()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(24));
        }
    }

    fn summary(&self) -> &Summary {
        self.index
            .as_ref()
            .map_or(&self.prefix, |index| &index.summary)
    }

    fn toolbar(&mut self, ui: &mut Ui, copies: &mut Copies) {
        let summary = self.summary();
        ui.horizontal_wrapped(|ui| {
            ui.strong(summary.format.label());
            let prefix = if summary.complete { "" } else { " initial" };
            ui.weak(format!(
                "{}{prefix} frames · {} buses",
                summary.frames,
                summary.channels.len()
            ));
            if summary.errors > 0 {
                ui.weak(format!("{} errors", summary.errors));
            }
            if self.index_job.is_some() && !summary.complete {
                ui.weak("Indexing all frames…");
            }
            self.copy_action(ui, copies);
        });
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn copy_action(&self, ui: &mut Ui, copies: &mut Copies) {
        let selection = self
            .selection
            .clone()
            .or_else(|| self.selected.map(|row| Selection::Rows(row..row + 1)));
        let label = if self.selection.is_some() {
            "Copy source range as CSV"
        } else {
            "Copy frame as CSV"
        };
        if ui
            .add_enabled(
                self.index.is_some() && selection.is_some() && !copies.busy(),
                egui::Button::new(label),
            )
            .on_hover_text("Copy the recorded frame headers and payloads in the source selection")
            .clicked()
            && let (Some(index), Some(selection)) = (self.index.clone(), selection)
        {
            let capture = self.capture.clone();
            copies.start(ui.ctx(), move |cancel| {
                capture.copy(&index, selection, cancel)
            });
        }
    }

    fn filter_controls(&mut self, ui: &mut Ui) {
        // Filtering only runs on explicit apply, since it scans the source.
        let summary = self
            .index
            .as_ref()
            .map_or(&self.prefix, |index| &index.summary);
        let (apply, clear) = ui
            .horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("can-bus-filter")
                    .selected_text(
                        self.filter
                            .channel
                            .as_ref()
                            .map_or_else(|| "All buses".into(), ToString::to_string),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filter.channel, None, "All buses");
                        for bus in &summary.channels {
                            ui.selectable_value(
                                &mut self.filter.channel,
                                Some(bus.channel.clone()),
                                format!("{} · {}", bus.channel, bus.frames),
                            );
                        }
                    });
                let id = ui.add(
                    egui::TextEdit::singleline(&mut self.id_input)
                        .id_salt("can-identifier-filter")
                        .desired_width(94.0)
                        .char_limit(12)
                        .hint_text("ID in hex"),
                );
                let submitted =
                    id.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let top = top_identifier(ui, &summary.identifiers);
                if let Some(item) = top {
                    self.filter.channel = Some(item.channel.clone());
                    self.id_input = item.id.to_string();
                }
                ui.checkbox(&mut self.filter.errors_only, "Errors only");
                let clicked = ui
                    .add_enabled(self.index.is_some(), egui::Button::new("Filter"))
                    .clicked();
                let clear = (self.filtered.is_some() || self.filter_job.is_some())
                    && ui.button("Clear filter").clicked();
                if self.filter_job.is_some() {
                    ui.weak("Finding matches…");
                }
                if let Some(result) = &self.filtered {
                    let count = result.frames.len();
                    let label = if result.complete {
                        format!("{count} matches")
                    } else {
                        format!("{count} matches in first {} frames", result.scanned)
                    };
                    ui.weak(label);
                }
                (submitted || top.is_some() || clicked, clear)
            })
            .inner;
        if clear {
            self.clear_filter();
            if let Some(row) = self.selected {
                self.reveal(row);
            }
        } else if apply {
            self.request_filter(ui.ctx());
        }
    }

    fn timeline(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let full = activity::domain(self.summary());
        let mut visible = self.viewport.clone().unwrap_or_else(|| full.clone());
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 92.0), Sense::click_and_drag());
        if let Some(pointer) = response.hover_pos() {
            let delta = ui.input(|input| input.smooth_scroll_delta().y);
            if delta != 0.0 {
                let anchor = activity::at(rect, &visible, pointer.x);
                visible = activity::zoom(&full, &visible, anchor, delta);
                self.viewport = Some(visible.clone());
            }
        }

        let time = self.timeline_selection(ui, rect, &response, &visible);
        let painter = ui.painter_at(rect);
        activity::draw(&painter, rect, &visible, &self.summary().activity, colors);
        if let Some(span) = self.selected_span() {
            activity::highlight(&painter, rect, &visible, &span, colors);
        }
        if let Some(frame) = self.selected.and_then(|row| self.source_frame(row)) {
            activity::marker(&painter, rect, &visible, frame.timestamp_ns, colors);
        }
        response.on_hover_ui(|ui| {
            if let Some(time) = time
                && let Some(bin) = activity::nearest(&self.summary().activity, time)
            {
                ui.strong(activity::time_label(
                    bin.time_ns.start.max(full.start) - full.start,
                ));
                ui.label(format!(
                    "{} frames · {} errors",
                    bin.frames, bin.error_frames
                ));
            }
            ui.weak("Click to reveal a frame · Drag to select time · Scroll to zoom");
        });
        ui.horizontal(|ui| {
            ui.monospace(activity::time_label(visible.start - full.start));
            ui.weak("–");
            ui.monospace(activity::time_label(visible.end - full.start - 1));
            if self.viewport.is_some() && ui.button("Fit").clicked() {
                self.viewport = None;
            }
            if self.selection.is_some() && ui.button("Clear range").clicked() {
                self.selection = None;
            }
        });
    }

    fn timeline_selection(
        &mut self,
        ui: &Ui,
        rect: egui::Rect,
        response: &egui::Response,
        visible: &activity::Domain,
    ) -> Option<i128> {
        let at = |point: egui::Pos2| activity::at(rect, visible, point.x);
        let time = at(response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())?);
        if response.drag_started_by(PointerButton::Primary) {
            self.brush_anchor = Some(
                ui.input(|input| input.pointer.press_origin())
                    .map_or(time, at),
            );
        }
        let released = response.drag_stopped_by(PointerButton::Primary);
        if let Some(anchor) = self.brush_anchor
            && (released || response.dragged_by(PointerButton::Primary))
        {
            let span = anchor.min(time)..anchor.max(time) + 1;
            if released {
                self.brush_anchor = None;
                if let Some(ordinal) = self.frame_near(span.start) {
                    self.focus_frame(ordinal);
                }
            }
            self.selection = Some(Selection::Time(span));
        } else if response.clicked_by(PointerButton::Primary)
            && let Some(ordinal) = self.frame_near(time)
        {
            self.brush_anchor = None;
            self.focus_frame(ordinal);
        }
        Some(time)
    }

    fn frame_near(&self, time: i128) -> Option<u64> {
        let bin = activity::nearest(&self.summary().activity, time)?;
        if let Some(filtered) = &self.filtered {
            return filtered
                .frames
                .iter()
                .filter(|frame| bin.time_ns.contains(&frame.timestamp_ns))
                .min_by_key(|frame| (frame.timestamp_ns - time).abs())
                .map(|frame| frame.row);
        }
        Some(bin.first_row)
    }

    fn selected_span(&self) -> Option<activity::Domain> {
        match self.selection.as_ref()? {
            Selection::Time(span) => Some(span.clone()),
            Selection::Rows(_) => self.selected_row_span.clone(),
        }
    }

    fn navigation(&mut self, ui: &mut Ui) {
        let count = self.table_count();
        let source = self
            .index
            .as_ref()
            .map_or(self.prefix.sample.len() as u64, |index| {
                index.summary.frames
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                self.set_page(self.page.saturating_sub(PAGE_ROWS));
            }
            if ui
                .add_enabled(self.page + PAGE_ROWS < count, egui::Button::new("Next"))
                .clicked()
            {
                self.set_page(self.page + PAGE_ROWS);
            }
            if count > 0 {
                ui.weak(format!(
                    "{}–{} of {} {}",
                    self.page.min(count - 1) + 1,
                    (self.page + PAGE_ROWS).min(count),
                    count,
                    if self.filtered.is_some() {
                        "matches"
                    } else {
                        "frames"
                    },
                ));
            }
            ui.separator();
            ui.label("Frame");
            let value = ui.add_enabled(
                source > 0,
                egui::DragValue::new(&mut self.jump_to).range(1..=source.max(1)),
            );
            if source > 0
                && (ui.button("Go").clicked()
                    || (value.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
            {
                let row = self.jump_to.clamp(1, source) - 1;
                self.clear_filter();
                self.focus_frame(row);
            }
        });
    }

    fn browser(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let table_count = self.table_count();
        let count = table_count.saturating_sub(self.page).min(PAGE_ROWS) as usize;
        let origin = activity::domain(self.summary()).start;
        let height = ui.available_height();
        let details = if self.selected.is_some() {
            (height * 0.34).clamp(70.0, 152.0)
        } else {
            0.0
        };
        let table = table::show(
            ui,
            Page {
                offset: self.page,
                count,
                selected: self.selected,
                selection: match &self.selection {
                    Some(Selection::Rows(rows)) => Some(rows),
                    _ => None,
                },
                scroll_to: self.scroll_to.take(),
                generation: self.generation,
                height: (height - details - 7.0).max(72.0),
            },
            origin,
            colors,
            |position| self.table_frame(position),
        );
        if let Some((row, shift)) = table.clicked {
            self.select(row, shift);
        }
        self.request_rows(ui.ctx(), table.visible);
        if let Some(row) = self.selected {
            ui.separator();
            if let Some(frame) = self.source_frame(row) {
                table::details(ui, frame, origin, details - 49.0);
            } else {
                ui.weak(format!("Reading frame {}…", row + 1));
            }
        }
    }

    fn table_count(&self) -> u64 {
        self.filtered.as_ref().map_or_else(
            || {
                self.index
                    .as_ref()
                    .map_or(self.prefix.sample.len() as u64, |index| {
                        index.summary.frames
                    })
            },
            |filtered| filtered.frames.len() as u64,
        )
    }

    fn table_frame(&self, position: u64) -> Option<&Frame> {
        if let Some(filtered) = &self.filtered {
            return filtered.frames.get(position as usize);
        }
        self.source_frame(position)
    }

    fn source_frame(&self, row: u64) -> Option<&Frame> {
        self.detail
            .as_ref()
            .filter(|frame| frame.row == row)
            .or_else(|| self.prefix.sample.get(row as usize))
            .or_else(|| self.windows.get(row))
    }

    fn select(&mut self, row: u64, extend: bool) {
        let time = self.source_frame(row).map(|frame| frame.timestamp_ns);
        self.selected_row_span = None;
        self.selection = if extend && self.filtered.is_none() {
            self.row_anchor.map(|(anchor, start)| {
                self.selected_row_span = start
                    .zip(time)
                    .map(|(start, end)| start.min(end)..start.max(end) + 1);
                Selection::Rows(anchor.min(row)..anchor.max(row) + 1)
            })
        } else {
            self.row_anchor = Some((row, time));
            None
        };
        if self.selected != Some(row) {
            self.detail_job = None;
            self.detail_failed = None;
        }
        self.selected = Some(row);
        self.jump_to = row + 1;
        if let Some(frame) = self.source_frame(row)
            && let Some(viewport) = &self.viewport
            && !viewport.contains(&frame.timestamp_ns)
        {
            self.viewport = None;
        }
    }

    fn reveal(&mut self, row: u64) {
        let position = if let Some(filtered) = &self.filtered {
            let Ok(position) = filtered
                .frames
                .binary_search_by_key(&row, |frame| frame.row)
            else {
                return;
            };
            position as u64
        } else {
            row
        };
        self.set_page(position / PAGE_ROWS * PAGE_ROWS);
        self.scroll_to = Some((position % PAGE_ROWS) as usize);
    }

    fn set_page(&mut self, page: u64) {
        if self.page != page {
            self.windows.clear_request();
            self.page = page;
            self.scroll_to = Some(0);
        }
    }

    fn request_index(&mut self, ctx: &Context) {
        if self.index.is_some() || self.index_job.is_some() || self.index_failed {
            return;
        }
        let capture = self.capture.clone();
        self.index_job = Some(start(&JOBS, ctx, Duration::from_secs(180), move |cancel| {
            capture.index(cancel)
        }));
    }

    fn request_rows(&mut self, ctx: &Context, visible: Range<u64>) {
        if self.filtered.is_some() || visible.is_empty() {
            return;
        }
        let Some(index) = self.index.clone() else {
            return;
        };
        let Some(range) = needed_window(visible, index.summary.frames, |row| {
            self.source_frame(row).is_some()
        }) else {
            return;
        };
        let capture = self.capture.clone();
        self.windows.request(
            &JOBS,
            ctx,
            Duration::from_secs(30),
            range,
            move |rows, cancel| capture.rows(&index, rows, cancel),
        );
    }

    fn request_detail(&mut self, ctx: &Context) {
        let (Some(row), Some(index)) = (self.selected, self.index.clone()) else {
            return;
        };
        if self.source_frame(row).is_some()
            || self.detail_job.is_some()
            || self.detail_failed == Some(row)
            || row >= index.summary.frames
        {
            return;
        }
        let capture = self.capture.clone();
        self.detail_job = Some((
            row,
            start(&JOBS, ctx, Duration::from_secs(30), move |cancel| {
                capture.rows(&index, row..row + 1, cancel)
            }),
        ));
    }

    fn request_filter(&mut self, ctx: &Context) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let id = self.id_input.trim();
        self.filter.id = if id.is_empty() {
            None
        } else if let Some(id) = parse_identifier(id) {
            Some(id)
        } else {
            self.error = Some("Enter an 11-bit or 29-bit hexadecimal CAN identifier".into());
            return;
        };
        self.error = None;
        let capture = self.capture.clone();
        let filter = self.filter.clone();
        self.filter_job = Some(start(&JOBS, ctx, Duration::from_secs(120), move |cancel| {
            capture.filter(&index, &filter, cancel)
        }));
    }

    fn clear_filter(&mut self) {
        self.filter = Filter::default();
        self.id_input.clear();
        self.filter_job = None;
        self.filtered = None;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn poll(&mut self) {
        if let Some(result) = poll(&mut self.index_job) {
            match result {
                Ok(index) => self.index = Some(Arc::new(index)),
                Err(error) => {
                    self.index_failed = true;
                    self.error = Some(error);
                }
            }
        }
        if let Some(error) = self.windows.poll_error() {
            self.error = Some(error);
        }
        if let Some((row, result)) = keyed_poll(&mut self.detail_job) {
            match result {
                Ok(mut frames) => {
                    self.detail = frames.pop();
                    if let Some((anchor, time)) = &mut self.row_anchor
                        && *anchor == row
                    {
                        *time = self.detail.as_ref().map(|frame| frame.timestamp_ns);
                    }
                    if let Some(viewport) = &self.viewport
                        && self
                            .detail
                            .as_ref()
                            .is_some_and(|frame| !viewport.contains(&frame.timestamp_ns))
                    {
                        self.viewport = None;
                    }
                }
                Err(error) => {
                    self.detail_failed = Some(row);
                    self.error = Some(error);
                }
            }
        }
        if let Some(result) = poll(&mut self.filter_job) {
            match result {
                Ok(filtered) => {
                    let first = filtered.frames.first().map(|frame| frame.row);
                    self.filtered = Some(filtered);
                    self.selection = None;
                    self.generation = self.generation.wrapping_add(1);
                    self.set_page(0);
                    if let Some(row) = first {
                        self.focus_frame(row);
                    } else {
                        self.selected = None;
                        self.detail_job = None;
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
}

fn top_identifier<'a>(
    ui: &mut Ui,
    identifiers: &'a [IdentifierSummary],
) -> Option<&'a IdentifierSummary> {
    if identifiers.is_empty() {
        return None;
    }
    ui.menu_button("Top IDs", |ui| {
        egui::ScrollArea::vertical()
            .max_height(225.0)
            .show(ui, |ui| {
                let mut selected = None;
                for item in identifiers.iter().take(64) {
                    if ui
                        .button(format!("{}  {}  ×{}", item.channel, item.id, item.frames))
                        .clicked()
                    {
                        ui.close();
                        selected = Some(item);
                    }
                }
                selected
            })
            .inner
    })
    .inner
    .flatten()
}

fn parse_identifier(value: &str) -> Option<Identifier> {
    let extended = value.ends_with(['x', 'X']);
    let value = value.strip_suffix(['x', 'X']).unwrap_or(value);
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    let value = u32::from_str_radix(value, 16).ok()?;
    (value <= 0x1fff_ffff).then_some(Identifier {
        value,
        extended: extended || value > 0x7ff,
    })
}

fn hex(bytes: &[u8]) -> String {
    let digits = "0123456789ABCDEF".as_bytes();
    let mut text = String::with_capacity(bytes.len().saturating_mul(3).saturating_sub(1));
    for (index, &byte) in bytes.iter().enumerate() {
        if index != 0 {
            text.push(' ');
        }
        text.push(char::from(digits[usize::from(byte >> 4)]));
        text.push(char::from(digits[usize::from(byte & 0xf)]));
    }
    text
}

fn kind(kind: FrameKind) -> &'static str {
    match kind {
        FrameKind::Classic => "CAN",
        FrameKind::Flexible { .. } => "CAN FD",
        FrameKind::CanXl => "CAN XL",
        FrameKind::J1939 => "J1939",
        FrameKind::Remote => "Remote",
        FrameKind::Error => "Error",
        FrameKind::Status => "Status",
    }
}

fn direction(direction: Direction) -> &'static str {
    match direction {
        Direction::Rx => "Rx",
        Direction::Tx => "Tx",
        Direction::Unknown => "",
    }
}
