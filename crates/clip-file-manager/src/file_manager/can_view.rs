//! Link CAN activity, exact frame rows, and recorded payloads.

mod activity;
mod capture;
mod database;
mod intervals;
mod signal_plot;
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
struct PlotSelection {
    point: little_clip_core::engineering::can::series::Point,
    return_field: Option<database::FieldSelection>,
}

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
    plot_selection: Option<PlotSelection>,
    reveal_detail: bool,
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
    database: database::Panel,
    intervals: intervals::Panel,
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
            plot_selection: None,
            reveal_detail: false,
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
            database: database::Panel::default(),
            intervals: intervals::Panel::default(),
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
        self.database.poll(ui.ctx(), &self.capture);
        let colors = WorkbenchColors::from_palette(palette);
        let mut shell = crate::shell::StudioShell::new(
            "CAN capture",
            crate::icons::Icon::Crosshair,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        if self.intervals.visible()
            && let Some(index) = &self.index
        {
            let full = activity::domain(&index.summary);
            let origin = full.start;
            let workspace = intervals::Workspace {
                capture: &self.capture,
                index,
                full,
                colors,
            };
            if let Some(frame) = self.intervals.show(ui, workspace) {
                self.database
                    .inspect(ui, frame, origin, ui.available_height(), colors, false);
            }
        } else {
            egui::ScrollArea::vertical()
                .id_salt("can-frame-workspace")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 24.0;
                    ui.spacing_mut().button_padding.y = 2.0;
                    self.frame_workspace(ui, colors, copies);
                });
        }
        self.request_index(ui.ctx());
        if self.index_job.is_some()
            || self.windows.pending()
            || self.detail_job.is_some()
            || self.filter_job.is_some()
            || self.database.pending()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(24));
        }
    }

    fn frame_workspace(&mut self, ui: &mut Ui, colors: WorkbenchColors, copies: &mut Copies) {
        self.toolbar(ui, copies);
        self.filter_controls(ui);
        self.database.attachment(ui, &self.capture);
        self.timeline(ui, colors);
        self.signal_plot(ui, colors, copies);
        self.navigation(ui);
        self.browser(ui, colors);
        self.request_detail(ui.ctx());
    }

    fn summary(&self) -> &Summary {
        self.index
            .as_ref()
            .map_or(&self.prefix, |index| &index.summary)
    }

    fn toolbar(&mut self, ui: &mut Ui, copies: &mut Copies) {
        let summary = self.summary();
        let stream = ui
            .horizontal_wrapped(|ui| {
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
                self.intervals_action(ui)
            })
            .inner;
        if let Some(stream) = stream {
            let full = activity::domain(self.summary());
            let scope = self.selected_span().map_or_else(
                || full.clone(),
                |span| span.start.max(full.start)..span.end.min(full.end),
            );
            self.intervals
                .open(stream, if scope.is_empty() { full } else { scope });
            self.windows.clear_request();
            ui.ctx().request_repaint();
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn intervals_action(
        &self,
        ui: &mut Ui,
    ) -> Option<little_clip_core::engineering::can::intervals::Stream> {
        use little_clip_core::engineering::can::intervals::Stream;
        let stream = self
            .inspected_row()
            .and_then(|row| self.source_frame(row))
            .and_then(Stream::of)?;
        ui.add_enabled(
            self.index.is_some(),
            egui::Button::new(format!("Intervals for {}", stream.id)),
        )
        .on_hover_text(format!(
            "Measure adjacent observed frames on {}",
            stream.channel
        ))
        .clicked()
        .then_some(stream)
    }

    fn copy_action(&self, ui: &mut Ui, copies: &mut Copies) {
        let selection = self.selection.clone().or_else(|| {
            self.inspected_row()
                .map(|row| Selection::Rows(row..row + 1))
        });
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
                ui.separator();
                self.database.toolbar(ui, &self.capture);
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
        let height = if self.database.plot_open() {
            34.0
        } else {
            92.0
        };
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        if let Some(pointer) = response.hover_pos() {
            let delta = ui.input(|input| input.smooth_scroll_delta().y);
            if delta != 0.0 {
                let anchor = activity::at(rect, &visible, pointer.x);
                visible = activity::zoom(&full, &visible, anchor, delta);
                self.viewport = Some(visible.clone());
                ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
            }
        }

        let time = self.timeline_selection(ui, rect, &response, &visible);
        let painter = ui.painter_at(rect);
        activity::draw(&painter, rect, &visible, &self.summary().activity, colors);
        if let Some(span) = self.selected_span() {
            activity::highlight(&painter, rect, &visible, &span, colors);
        }
        if let Some(cursor) = self.frame_cursor() {
            activity::marker(&painter, rect, &visible, cursor.time_ns, colors);
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

    fn signal_plot(&mut self, ui: &mut Ui, colors: WorkbenchColors, copies: &mut Copies) {
        let full = activity::domain(self.summary());
        let viewport = signal_plot::Viewport {
            span: self.viewport.as_ref().unwrap_or(&full),
            selection: self.selected_span(),
            cursor: self.frame_cursor(),
            origin: full.start,
        };
        if let Some(hit) = self.database.plot(
            ui,
            &self.capture,
            self.index.as_ref(),
            viewport,
            colors,
            copies,
        ) {
            self.follow_signal_hit(hit);
            self.reveal_detail = true;
            ui.ctx().request_repaint();
        }
    }

    fn follow_signal_hit(&mut self, hit: signal_plot::Hit) {
        let row = hit.point.row;
        let outside = self.filtered.is_some() && self.filtered_position(row).is_none();
        let return_field = if outside {
            self.retire_detail_if_moving(row);
            let field = self
                .plot_selection
                .take()
                .and_then(|previous| previous.return_field)
                .unwrap_or_else(|| self.database.field_selection());
            Some(field)
        } else {
            self.clear_plot_selection();
            self.set_selected(row);
            if self.selection.is_none() {
                self.row_anchor = Some((row, Some(hit.point.time_ns)));
            }
            self.reveal(row);
            None
        };
        self.database.reveal_plot_field(&hit.signal);
        self.plot_selection = Some(PlotSelection {
            point: hit.point,
            return_field,
        });
    }

    fn inspected_row(&self) -> Option<u64> {
        self.plot_selection
            .as_ref()
            .map(|selection| selection.point.row)
            .or(self.selected)
    }

    fn frame_cursor(&self) -> Option<signal_plot::FrameCursor> {
        if let Some(selection) = &self.plot_selection {
            return Some(signal_plot::FrameCursor {
                row: selection.point.row,
                time_ns: selection.point.time_ns,
            });
        }
        let frame = self.source_frame(self.selected?)?;
        Some(signal_plot::FrameCursor {
            row: frame.row,
            time_ns: frame.timestamp_ns,
        })
    }

    fn clear_plot_selection(&mut self) {
        self.reveal_detail = false;
        if let Some(selection) = self.plot_selection.take()
            && let Some(saved) = selection.return_field
        {
            self.detail_job = None;
            self.detail_failed = None;
            self.database.restore_field_selection(saved);
        }
    }

    fn plot_visit_controls(&mut self, ui: &mut Ui) -> bool {
        let Some(selection) = self
            .plot_selection
            .as_ref()
            .filter(|selection| selection.return_field.is_some())
        else {
            return false;
        };
        let row = selection.point.row;
        let status = if self
            .filtered
            .as_ref()
            .is_some_and(|filter| !filter.complete && row >= filter.scanned)
        {
            "Beyond scanned filter coverage"
        } else {
            "Outside filter matches"
        };
        let back = ui
            .horizontal_wrapped(|ui| {
                ui.weak(status);
                ui.small_button("Back to matches").clicked()
            })
            .inner;
        if back {
            self.clear_plot_selection();
            ui.ctx().request_repaint();
        }
        back
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
        let details = if self.inspected_row().is_none() {
            0.0
        } else if self.database.attached() {
            (height * 0.56)
                .clamp(184.0, 230.0)
                .min((height - 80.0).max(105.0))
        } else {
            (height * 0.34).clamp(70.0, 152.0)
        };
        let table_top = ui.next_widget_position();
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
            self.reveal_detail = true;
        }
        self.request_rows(ui.ctx(), table.visible);
        if self.inspected_row().is_none() {
            return;
        }
        ui.separator();
        let source = ui.scope(|ui| {
            let returned = self.plot_visit_controls(ui);
            if let Some(row) = self.inspected_row() {
                self.frame_inspector(ui, row, origin, details.min(ui.available_height()), colors);
            }
            returned
        });
        if source.inner {
            ui.scroll_to_rect(egui::Rect::from_min_size(table_top, vec2(1.0, 55.0)), None);
        } else if self.reveal_detail
            && self.inspected_row().is_some_and(|row| {
                self.source_frame(row).is_some() || self.detail_failed == Some(row)
            })
        {
            self.reveal_detail = false;
            source.response.scroll_to_me(None);
        }
    }

    fn frame_inspector(
        &mut self,
        ui: &mut Ui,
        row: u64,
        origin: i128,
        height: f32,
        colors: WorkbenchColors,
    ) {
        if let Some(frame) = self.source_frame(row).cloned() {
            self.database
                .inspect(ui, &frame, origin, height, colors, true);
        } else if self.detail_failed == Some(row) {
            ui.weak(format!("Frame {} unavailable", row + 1));
        } else {
            ui.weak(format!("Reading frame {}…", row + 1));
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
        self.clear_plot_selection();
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
        self.set_selected(row);
        if let Some(frame) = self.source_frame(row)
            && let Some(viewport) = &self.viewport
            && !viewport.contains(&frame.timestamp_ns)
        {
            self.viewport = None;
        }
    }

    fn set_selected(&mut self, row: u64) {
        self.retire_detail_if_moving(row);
        self.selected = Some(row);
        self.jump_to = row + 1;
    }

    fn retire_detail_if_moving(&mut self, row: u64) {
        if self.inspected_row() != Some(row) {
            self.detail_job = None;
            self.detail_failed = None;
        }
    }

    fn filtered_position(&self, row: u64) -> Option<usize> {
        self.filtered
            .as_ref()?
            .frames
            .binary_search_by_key(&row, |frame| frame.row)
            .ok()
    }

    fn reveal(&mut self, row: u64) {
        let position = if self.filtered.is_some() {
            let Some(position) = self.filtered_position(row) else {
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
        let (Some(row), Some(index)) = (self.inspected_row(), self.index.clone()) else {
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
        self.clear_plot_selection();
        self.filter = Filter::default();
        self.id_input.clear();
        self.filter_job = None;
        self.filtered = None;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn accept_detail(&mut self, row: u64, frame: Option<Frame>) {
        self.detail = frame;
        let resolves_anchor = self.row_anchor.as_mut().is_some_and(|(anchor, time)| {
            if *anchor != row || time.is_some() {
                return false;
            }
            *time = self.detail.as_ref().map(|frame| frame.timestamp_ns);
            true
        });
        if resolves_anchor
            && self.plot_selection.is_none()
            && let Some((span, frame)) = self.viewport.as_ref().zip(self.detail.as_ref())
            && !span.contains(&frame.timestamp_ns)
        {
            self.viewport = None;
        }
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
                Ok(mut frames) => self.accept_detail(row, frames.pop()),
                Err(error) => {
                    self.detail_failed = Some(row);
                    self.error = Some(error);
                }
            }
        }
        if let Some(result) = poll(&mut self.filter_job) {
            match result {
                Ok(filtered) => {
                    self.clear_plot_selection();
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

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::can::series::Point;

    #[test]
    fn plot_frame_visits_keep_filter_matches_brush_and_viewport() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Bench.asc");
        std::fs::write(
            &path,
            concat!(
                "date Mon Nov 2 03:22:02 2026\nbase hex timestamps absolute\n",
                "0.100 1 123 Tx d 2 0A 00\n",
                "0.180 1 124 Rx d 2 14 00\n",
                "0.400 1 123 Tx d 2 30 00\n",
                "0.680 1 124 Rx d 2 18 00\n",
            ),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut view = Viewer::load(&path, &cancel).unwrap();
        let index = view.capture.index(&cancel).unwrap();
        let command = parse_identifier("123").unwrap();
        view.filtered = Some(
            view.capture
                .filter(
                    &index,
                    &Filter {
                        id: Some(command),
                        ..Filter::default()
                    },
                    &cancel,
                )
                .unwrap(),
        );
        let brush = 150_000_000..680_000_001;
        let visible = 100_000_000..220_000_000;
        view.selection = Some(Selection::Time(brush.clone()));
        view.viewport = Some(visible.clone());
        let hit = |row, field| {
            let frame = &view.prefix.sample[row as usize];
            signal_plot::Hit {
                signal: signal_plot::Signal {
                    channel: frame.channel.clone(),
                    id: frame.id.unwrap(),
                    field,
                },
                point: Point {
                    row,
                    time_ns: frame.timestamp_ns,
                    decoded: None,
                },
            }
        };
        let original = hit(0, 0);
        let response = hit(3, 1);
        let later_command = hit(2, 1);
        view.database.reveal_plot_field(&original.signal);
        let original_field = view.database.field_selection();

        view.follow_signal_hit(response);
        assert_eq!(view.inspected_row(), Some(3));
        assert_eq!(view.selected, Some(0));
        assert!(view.filtered_position(3).is_none());
        assert_eq!(view.frame_cursor().unwrap().time_ns, 680_000_000);
        assert_eq!(view.selected_span(), Some(brush.clone()));
        assert_eq!(view.viewport, Some(visible.clone()));

        view.clear_plot_selection();
        assert_eq!(view.inspected_row(), Some(0));
        assert_eq!(view.database.field_selection(), original_field);

        view.follow_signal_hit(later_command);
        assert_eq!(view.selected, Some(2));
        assert_eq!(view.inspected_row(), Some(2));
        assert_eq!(view.scroll_to, Some(1));
        assert_eq!(view.selected_span(), Some(brush));
        assert_eq!(view.viewport, Some(visible));
        assert_eq!(view.table_count(), 2);
    }
}
