use super::{Job, Source, error, keyed_poll, launch, plot, poll};
use crate::{
    file_manager::{
        capture_browse::table::{Columns, Page},
        preview_copy::{self, Copies},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Align, Layout, PointerButton, Sense, Ui, vec2};
use little_clip_core::engineering::tags::{
    self, Direction, Frame, FrameInfo, TraceBin, TraceIndex, TracePreview,
};
use std::{fmt::Write as _, ops::Range, sync::Arc, time::Duration};

const PAGE: u64 = 4096;
const WINDOW: u64 = 256;
const COLUMNS: Columns<5> = Columns {
    id: "trace records",
    headings: [
        "Frame",
        "Start ticks",
        "Duration ticks",
        "Direction",
        "Bytes",
    ],
    fractions: [0.11, 0.25, 0.23, 0.24, 0.17],
    min_height: 48.0,
    selected_fill: 0.45,
    hover_fill: 0.19,
};

pub(super) struct View {
    preview: TracePreview,
    bins: Vec<TraceBin>,
    index: Option<Arc<TraceIndex>>,
    index_job: Option<Job<TraceIndex>>,
    index_failed: bool,
    frame: Option<Frame>,
    frame_job: Option<(u64, Job<Frame>)>,
    frame_failed: Option<u64>,
    window: Option<(Range<u64>, Vec<FrameInfo>)>,
    window_job: Option<(Range<u64>, Job<Vec<FrameInfo>>)>,
    window_failed: Option<Range<u64>>,
    selected: u64,
    page: u64,
    scroll_to: Option<usize>,
    goto: u64,
    anchor: u64,
    selection: Option<Range<u64>>,
    error: Option<String>,
}

impl View {
    pub fn new(preview: TracePreview) -> Self {
        let bins = plot::trace_preview(&preview.frames);
        Self {
            preview,
            bins,
            index: None,
            index_job: None,
            index_failed: false,
            frame: None,
            frame_job: None,
            frame_failed: None,
            window: None,
            window_job: None,
            window_failed: None,
            selected: 0,
            page: 0,
            scroll_to: None,
            goto: 1,
            anchor: 0,
            selection: None,
            error: None,
        }
    }

    pub fn focus(&mut self, row: u64) {
        self.select(row, false);
        self.page = row / PAGE * PAGE;
        self.scroll_to = Some((row - self.page).saturating_sub(3) as usize);
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        colors: WorkbenchColors,
        source: &Arc<Source>,
        copies: &mut Copies,
    ) {
        self.poll();
        self.controls(ui, source, copies, colors);
        self.overview(ui, colors);
        self.navigation(ui);
        let width = ui.available_width();
        let height = ui.available_height().max(150.0);
        if width > 690.0 {
            ui.horizontal_top(|ui| {
                let left = (width * 0.50).max(344.0);
                ui.allocate_ui_with_layout(
                    vec2(left, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(left);
                        self.table(ui, height, source, colors);
                    },
                );
                ui.separator();
                ui.vertical(|ui| self.details(ui));
            });
        } else {
            self.table(ui, (height * 0.46).max(72.0), source, colors);
            ui.separator();
            self.details(ui);
        }
        self.start_index(ui, source);
        self.start_frame(ui, source);
        if self.index_job.is_some() || self.frame_job.is_some() || self.window_job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(20));
        }
    }

    fn controls(
        &self,
        ui: &mut Ui,
        source: &Arc<Source>,
        copies: &mut Copies,
        colors: WorkbenchColors,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} frames", self.rows()));
            if let Some(index) = &self.index {
                ui.colored_label(colors.accent, format!("{} reader", index.reader_frames));
                ui.colored_label(colors.warning, format!("{} tag", index.tag_frames));
            } else if !self.preview.complete {
                ui.weak("Capture prefix");
            }
            if let Some(span) = &self.selection {
                ui.monospace(format!("Frames {}–{}", span.start + 1, span.end));
            }
            let rows = self
                .selection
                .clone()
                .unwrap_or(self.selected..self.selected + 1);
            if ui
                .add_enabled(
                    self.index.is_some() && rows.end - rows.start <= 4096 && !copies.busy(),
                    egui::Button::new("Copy timing CSV"),
                )
                .on_hover_text("Copy up to 4,096 frames of timing and length measurements")
                .clicked()
                && let Some(index) = self.index.clone()
            {
                let source = source.clone();
                copies.start(ui.ctx(), move |cancel| {
                    copy_timings(&source, &index, rows, cancel)
                });
            }
        });
        if let Some(error) = self.error.as_deref() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn overview(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click_and_drag());
        let bins = self
            .index
            .as_ref()
            .map_or(&self.bins[..], |index| index.bins.as_slice());
        let axis = plot::TraceAxis::new(bins);
        let domain = axis.domain();
        let painter = ui.painter_at(rect);
        plot::trace(&painter, rect, bins, &axis, colors);
        let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, drag) =
            preview_copy::drag_range(ui, &response, egui::Id::unique("tag trace span"), modified);
        let mut selected = None;
        if let Some((left, right)) = drag {
            let first = axis.pick(bins, left);
            let end = axis.pick(bins, right).max(first) + 1;
            if response.drag_stopped_by(PointerButton::Primary) {
                selected = Some((first, Some(first..end)));
            }
            self.selection = Some(first..end);
        } else if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            selected = Some((axis.pick(bins, plot::fraction(rect, pos.x)), None));
        }
        if let Some(selection) = &self.selection {
            let first = axis.for_row(bins, selection.start, self.row(selection.start));
            let end = axis
                .for_row(bins, selection.end, self.row(selection.end))
                .max(first);
            plot::range(&painter, rect, &domain, &(first..end), colors.focus);
        }
        let marker = axis.for_row(bins, self.selected, self.row(self.selected));
        plot::marker(&painter, rect, &domain, marker, colors.focus);
        response.on_hover_text(format!(
            "{} · Reader above, tag below · Click a frame or drag a range",
            axis.label(),
        ));
        if let Some((row, range)) = selected {
            self.focus(row);
            self.selection = range;
        }
    }

    fn navigation(&mut self, ui: &mut Ui) {
        let rows = self.rows();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("Previous page"))
                .clicked()
            {
                self.page = self.page.saturating_sub(PAGE);
            }
            if ui
                .add_enabled(self.page + PAGE < rows, egui::Button::new("Next page"))
                .clicked()
            {
                self.page += PAGE;
            }
            ui.add(
                egui::DragValue::new(&mut self.goto)
                    .prefix("Frame ")
                    .range(1..=rows.max(1)),
            );
            if ui.button("Go").clicked() {
                self.focus(self.goto.saturating_sub(1));
            }
        });
    }

    fn table(&mut self, ui: &mut Ui, height: f32, source: &Arc<Source>, colors: WorkbenchColors) {
        let count = self.rows().saturating_sub(self.page).min(PAGE) as usize;
        let scroll_to = self.scroll_to.take();
        let mut output = COLUMNS.show(
            ui,
            Page {
                offset: self.page,
                count,
                selected: Some(self.selected),
                selection: self.selection.as_ref(),
                scroll_to,
                generation: 0,
                height,
            },
            colors,
            |row| self.row(row),
            |info| info.row,
            |ui, rect, info| row_contents(ui, rect, info, colors),
        );
        if let Some((row, extend)) = output.clicked {
            self.select(row, extend);
        }
        if let Some(row) = output.visible.find(|&row| self.row(row).is_none()) {
            self.start_window(ui, source, row);
        }
    }

    fn details(&self, ui: &mut Ui) {
        if let Some(info) = self.row(self.selected) {
            ui.strong(format!(
                "Frame {} · {}",
                info.row + 1,
                plot::direction(info.direction)
            ));
            ui.monospace(format!(
                "Start {} ticks · Duration {} ticks",
                info.timestamp_ticks, info.duration_ticks
            ));
            ui.weak(format!(
                "{} payload bytes · {} parity bytes",
                info.data_len,
                info.parity_len()
            ));
        }
        if let Some(frame) = self
            .frame
            .as_ref()
            .filter(|frame| frame.info.row == self.selected)
        {
            bytes(ui, frame);
        } else if self.frame_failed != Some(self.selected) {
            ui.weak("Reading captured bytes…");
        }
    }

    fn select(&mut self, row: u64, extend: bool) {
        if self.selected != row {
            self.frame_job = None;
            self.frame_failed = None;
        }
        self.selected = row;
        self.goto = row + 1;
        if extend {
            self.selection = Some(self.anchor.min(row)..self.anchor.max(row) + 1);
        } else {
            self.anchor = row;
            self.selection = None;
        }
    }

    fn row(&self, row: u64) -> Option<FrameInfo> {
        self.preview.frames.get(row as usize).copied().or_else(|| {
            let (range, rows) = self.window.as_ref()?;
            range
                .contains(&row)
                .then(|| rows.get((row - range.start) as usize))
                .flatten()
                .copied()
        })
    }

    fn rows(&self) -> u64 {
        self.index
            .as_ref()
            .map_or(self.preview.frames.len() as u64, |index| index.rows())
    }

    fn start_index(&mut self, ui: &Ui, source: &Arc<Source>) {
        if self.index.is_none() && self.index_job.is_none() && !self.index_failed {
            let source = source.clone();
            self.index_job = Some(launch(ui.ctx(), Duration::from_secs(90), move |cancel| {
                source.read(cancel, |input| tags::build_trace_index(input, cancel))
            }));
        }
    }

    fn start_frame(&mut self, ui: &Ui, source: &Arc<Source>) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let row = self.selected;
        if row >= index.rows()
            || self.frame_job.is_some()
            || self.frame_failed == Some(row)
            || self
                .frame
                .as_ref()
                .is_some_and(|frame| frame.info.row == row)
        {
            return;
        }
        let source = source.clone();
        self.frame_job = Some((
            row,
            launch(ui.ctx(), Duration::from_secs(15), move |cancel| {
                source.read(cancel, |input| {
                    tags::read_frames(input, &index, row..row + 1, cancel)?
                        .pop()
                        .ok_or(tags::Error::Truncated)
                })
            }),
        ));
    }

    fn start_window(&mut self, ui: &Ui, source: &Arc<Source>, row: u64) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let first = row.saturating_sub(48);
        let range = first..(first + WINDOW).min(index.rows());
        if self.window_job.is_some() || self.window_failed.as_ref() == Some(&range) {
            return;
        }
        let source = source.clone();
        let rows = range.clone();
        self.window_job = Some((
            range,
            launch(ui.ctx(), Duration::from_secs(15), move |cancel| {
                source.read(cancel, |input| {
                    tags::read_frame_info(input, &index, rows, cancel)
                })
            }),
        ));
    }

    fn poll(&mut self) {
        if let Some(result) = poll(&mut self.index_job) {
            match result {
                Ok(index) => self.index = Some(Arc::new(index)),
                Err(error) => {
                    self.error = Some(error);
                    self.index_failed = true;
                }
            }
        }
        if let Some((row, result)) = keyed_poll(&mut self.frame_job) {
            match result {
                Ok(frame) => self.frame = Some(frame),
                Err(error) => {
                    self.error = Some(error);
                    self.frame_failed = Some(row);
                }
            }
        }
        if let Some((range, result)) = keyed_poll(&mut self.window_job) {
            match result {
                Ok(rows) => self.window = Some((range, rows)),
                Err(error) => {
                    self.error = Some(error);
                    self.window_failed = Some(range);
                }
            }
        }
    }
}

fn row_contents(ui: &Ui, rect: egui::Rect, info: FrameInfo, colors: WorkbenchColors) {
    let color = if info.direction == Direction::Reader {
        colors.accent
    } else {
        colors.warning
    };
    COLUMNS.cells(
        ui,
        rect,
        [
            &(info.row + 1).to_string(),
            &info.timestamp_ticks.to_string(),
            &info.duration_ticks.to_string(),
            plot::direction(info.direction),
            &info.data_len.to_string(),
        ],
        color,
    );
}

fn bytes(ui: &mut Ui, frame: &Frame) {
    ui.separator();
    ui.strong("Captured bytes");
    let len = frame.data.len() + frame.parity.len();
    let columns = if ui.available_width() > 400.0 { 16 } else { 8 };
    let height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::both()
        .id_salt(("trace bytes", frame.info.row))
        .max_height(ui.available_height().max(45.0))
        .show_rows(ui, height, len.div_ceil(columns), |ui, range| {
            for index in range {
                let first = index * columns;
                let end = (first + columns).min(len);
                let data = (first..end).map(|offset| {
                    frame
                        .data
                        .get(offset)
                        .or_else(|| frame.parity.get(offset.saturating_sub(frame.data.len())))
                        .copied()
                });
                ui.monospace(format!("{first:04X}  {}", super::hex(data)));
            }
        });
}

fn copy_timings(
    source: &Source,
    index: &TraceIndex,
    rows: Range<u64>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    let mut csv = String::from("frame,start_ticks,duration_ticks,direction,captured_bytes\n");
    let count = rows.end - rows.start;
    let mut start = rows.start;
    while start < rows.end {
        let end = (start + 1024).min(rows.end);
        let records = source.read(cancel, |input| {
            tags::read_frame_info(input, index, start..end, cancel)
        })?;
        for record in records {
            let _ = writeln!(
                csv,
                "{},{},{},{},{}",
                record.row + 1,
                record.timestamp_ticks,
                record.duration_ticks,
                plot::direction(record.direction),
                record.data_len
            );
        }
        start = end;
    }
    little_clip_core::copy_text_to_system_clipboard(&csv).map_err(error)?;
    Ok(format!("Copied timing for {count} frames"))
}
