use super::{Job, Source, error, keyed_poll, launch, plot, poll};
use crate::{
    file_manager::preview_copy::{self, Copies},
    ui_appearance::WorkbenchColors,
};
use egui::{Align2, FontId, PointerButton, Rect, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use little_clip_core::engineering::tags::{self, SampleBin, SignalIndex, SignalPreview};
use std::{fmt::Write as _, ops::Range, sync::Arc, time::Duration};

const MAX_RAW: u64 = 4096;
const MAX_COPY: u64 = 16384;

struct Window {
    range: Range<u64>,
    values: Vec<i16>,
    peaks: Vec<SampleBin>,
}

pub(super) struct View {
    preview: SignalPreview,
    prefix: Vec<SampleBin>,
    index: Option<Arc<SignalIndex>>,
    index_job: Option<Job<SignalIndex>>,
    index_failed: bool,
    viewport: Range<u64>,
    selected: Option<u64>,
    selection: Option<Range<u64>>,
    show_samples: bool,
    window: Option<Window>,
    window_job: Option<(Range<u64>, Job<Window>)>,
    window_failed: Option<Range<u64>>,
    error: Option<String>,
}

impl View {
    pub fn new(preview: SignalPreview) -> Self {
        let viewport = 0..preview.samples.len() as u64;
        let prefix = plot::sample_peaks(&preview.samples, 1);
        Self {
            preview,
            prefix,
            index: None,
            index_job: None,
            index_failed: false,
            viewport,
            selected: None,
            selection: None,
            show_samples: false,
            window: None,
            window_job: None,
            window_failed: None,
            error: None,
        }
    }

    pub fn focus(&mut self, row: u64) {
        let total = self.samples().max(1);
        let row = row.min(total - 1);
        self.selected = Some(row);
        let width = (total / 5).clamp(1, 4096);
        let start = row.saturating_sub(width / 2).min(total - width);
        self.viewport = start..start + width;
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        colors: WorkbenchColors,
        source: &Arc<Source>,
        copies: &mut Copies,
    ) {
        self.poll();
        self.controls(ui, source, copies);
        let height = if self.show_samples {
            (ui.available_height() * 0.64).max(116.0)
        } else {
            (ui.available_height() - 82.0).max(116.0)
        };
        self.graph(ui, height, colors);
        self.overview(ui, colors);
        self.readout(ui);
        if self.show_samples {
            self.table(ui);
        }
        self.start_index(ui, source);
        self.start_window(ui, source);
        if self.index_job.is_some() || self.window_job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(20));
        }
    }

    fn controls(&mut self, ui: &mut Ui, source: &Arc<Source>, copies: &mut Copies) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} samples", self.samples()));
            if self.index.is_none() && !self.preview.complete {
                ui.weak("Capture prefix");
            }
            if ui.button("Fit").clicked() {
                self.viewport = 0..self.samples();
            }
            clip_ui::buttons::toggle(ui, &mut self.show_samples, "Sample rows");
            let rows = self
                .selection
                .clone()
                .or_else(|| self.selected.map(|row| row..row + 1));
            let eligible = rows
                .as_ref()
                .is_some_and(|range| range.end - range.start <= MAX_COPY);
            if ui
                .add_enabled(
                    self.index.is_some() && eligible && !copies.busy(),
                    egui::Button::new("Copy samples as CSV"),
                )
                .on_hover_text("Copy up to 16,384 selected measurements")
                .clicked()
                && let (Some(index), Some(rows)) = (self.index.clone(), rows)
            {
                let source = source.clone();
                copies.start(ui.ctx(), move |cancel| {
                    copy_samples(&source, &index, rows, cancel)
                });
            }
        });
        if let Some(error) = self.error.as_deref() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn graph(&mut self, ui: &mut Ui, height: f32, colors: WorkbenchColors) {
        let (area, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let rect = Rect::from_min_max(area.min + vec2(61.0, 19.0), area.max - vec2(0.0, 18.0));
        self.navigate(ui, rect, &response);
        let painter = ui.painter_at(area);
        let (start, step, bins) = self.visible_peaks();
        let amplitude = plot::samples(&painter, rect, &self.viewport, start, step, bins, colors);
        amplitude.labels(&painter, colors.text_secondary);
        let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, dragged) = preview_copy::drag_range_by(
            ui,
            &response,
            egui::Id::unique("tag signal range"),
            modified,
            |point| plot::fraction(rect, point.x),
        );
        if let Some((left, right)) = dragged {
            let first = plot::position(&self.viewport, left).min(self.samples() - 1);
            let end = plot::position(&self.viewport, right)
                .max(first + 1)
                .min(self.samples());
            self.selection = Some(first..end);
            self.selected = Some(first);
        } else if response.clicked_by(PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.selection = None;
            self.selected = Some(
                plot::position(&self.viewport, plot::fraction(rect, pos.x)).min(self.samples() - 1),
            );
        }
        if let Some(selection) = &self.selection {
            plot::range(&painter, rect, &self.viewport, selection, colors.focus);
        }
        if let Some(selected) = self.selected {
            plot::marker(&painter, rect, &self.viewport, selected, colors.focus);
        }
        let stride = if rect.width() < 430.0 { 2 } else { 4 };
        for slot in 0..=stride {
            let sample =
                self.viewport.start + (self.viewport.end - self.viewport.start - 1) * slot / stride;
            let align = match slot {
                0 => Align2::LEFT_TOP,
                last if last == stride => Align2::RIGHT_TOP,
                _ => Align2::CENTER_TOP,
            };
            let x = egui::lerp(rect.x_range(), slot as f32 / stride as f32);
            painter.text(
                pos2(x, rect.bottom() + 2.0),
                align,
                format!("{}", sample + 1),
                FontId::monospace(11.0),
                colors.text_secondary,
            );
        }
        if let Some(point) = response.hover_pos().filter(|point| rect.contains(*point)) {
            let row = plot::position(&self.viewport, plot::fraction(rect, point.x))
                .min(self.viewport.end - 1);
            plot::marker(
                &painter,
                rect,
                &self.viewport,
                row,
                colors.text_secondary.gamma_multiply(0.65),
            );
            let sample = self.value(row).map_or_else(
                || "Sample detail unavailable at this zoom".to_owned(),
                |value| format!("{value} counts"),
            );
            response.on_hover_text(format!("Sample {} · {sample}", row + 1));
        }
    }

    fn navigate(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        let span = self.viewport.end - self.viewport.start;
        if response.dragged_by(PointerButton::Middle) {
            let delta = f64::from(response.drag_delta().x / rect.width().max(1.0)) * span as f64;
            self.set_span(self.viewport.start as f64 - delta, span);
        }
        if let Some(pos) = response.hover_pos() {
            let delta = ui.input(|input| input.smooth_scroll_delta().y);
            if delta != 0.0 {
                let portion = f64::from(plot::fraction(rect, pos.x));
                let width = (span as f64 * f64::from((-delta * 0.003).clamp(-2.0, 2.0).exp()))
                    .round()
                    .clamp(1.0, self.samples() as f64) as u64;
                let start = self.viewport.start as f64 + (span as f64 - width as f64) * portion;
                self.set_span(start, width);
            }
        }
    }

    fn set_span(&mut self, start: f64, width: u64) {
        let width = width.clamp(1, self.samples());
        let start = (start.round().max(0.0) as u64).min(self.samples() - width);
        self.viewport = start..start + width;
    }

    fn overview(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 35.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let domain = 0..self.samples();
        let (step, bins) = self.bins();
        plot::samples(&painter, rect, &domain, 0, step, bins, colors);
        plot::range(&painter, rect, &domain, &self.viewport, colors.focus);
        let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, dragged) = preview_copy::drag_range(
            ui,
            &response,
            egui::Id::unique("tag signal overview"),
            modified,
        );
        if let Some((left, right)) = dragged {
            let first = plot::position(&domain, left).min(self.samples() - 1);
            let end = plot::position(&domain, right)
                .max(first + 1)
                .min(self.samples());
            self.viewport = first..end;
        } else if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let point = plot::position(&domain, plot::fraction(rect, pos.x));
            let width = (self.viewport.end - self.viewport.start).min((self.samples() / 3).max(1));
            self.set_span(point as f64 - width as f64 / 2.0, width);
        }
        response.on_hover_text("Drag to zoom to a sample interval");
    }

    fn readout(&self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!(
                "Samples {}–{}",
                self.viewport.start + 1,
                self.viewport.end
            ));
            if let Some(selected) = self.selected {
                let value = self
                    .value(selected)
                    .map_or_else(|| "…".to_owned(), |value| value.to_string());
                ui.monospace(format!("Sample {} · {value}", selected + 1));
            }
            if let Some(selection) = self.selection.as_ref() {
                ui.strong(format!(
                    "Selected samples {}–{}",
                    selection.start + 1,
                    selection.end
                ));
                if let Some(values) = self.recorded_slice(selection)
                    && let Some((min, max)) = values.iter().copied().fold(None, |extrema, value| {
                        Some(extrema.map_or((value, value), |(lo, hi): (i16, i16)| {
                            (lo.min(value), hi.max(value))
                        }))
                    })
                {
                    ui.weak(format!("Min {min} · Max {max} counts"));
                }
            }
        });
    }

    fn table(&mut self, ui: &mut Ui) {
        let range = self.desired();
        let height = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
        TableBuilder::new(ui)
            .id_salt("tag raw sample rows")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .column(Column::exact(108.0).clip(true))
            .column(Column::remainder().clip(true))
            .header(height, |mut row| {
                row.col(|ui| {
                    ui.strong("Sample");
                });
                row.col(|ui| {
                    ui.strong("Value");
                });
            })
            .body(|body| {
                body.rows(
                    height,
                    (range.end - range.start) as usize,
                    |mut table_row| {
                        let row = range.start + table_row.index() as u64;
                        let value = self
                            .value(row)
                            .map_or_else(|| "…".to_owned(), |value| value.to_string());
                        table_row.set_selected(self.selected == Some(row));
                        table_row.col(|ui| {
                            ui.monospace((row + 1).to_string());
                        });
                        table_row.col(|ui| {
                            ui.monospace(value);
                        });
                        if table_row.response().clicked() {
                            self.selected = Some(row);
                        }
                    },
                );
            });
    }

    fn value(&self, row: u64) -> Option<i16> {
        self.preview.samples.get(row as usize).copied().or_else(|| {
            let window = self.window.as_ref()?;
            window
                .range
                .contains(&row)
                .then(|| window.values.get((row - window.range.start) as usize))
                .flatten()
                .copied()
        })
    }

    fn recorded_slice(&self, rows: &Range<u64>) -> Option<&[i16]> {
        self.preview
            .samples
            .get(rows.start as usize..rows.end as usize)
            .or_else(|| {
                let window = self
                    .window
                    .as_ref()
                    .filter(|window| covers(&window.range, rows))?;
                let start = (rows.start - window.range.start) as usize;
                let end = (rows.end - window.range.start) as usize;
                window.values.get(start..end)
            })
    }

    fn bins(&self) -> (u64, &[SampleBin]) {
        self.index.as_ref().map_or((1, &self.prefix[..]), |index| {
            (index.bin_samples, index.bins.as_slice())
        })
    }

    fn visible_peaks(&self) -> (u64, u64, &[SampleBin]) {
        if self.viewport.end <= self.preview.samples.len() as u64 {
            return (0, 1, &self.prefix);
        }
        if let Some(window) = self
            .window
            .as_ref()
            .filter(|window| covers(&window.range, &self.viewport))
        {
            return (window.range.start, 1, &window.peaks);
        }
        let (step, bins) = self.bins();
        (0, step, bins)
    }

    fn desired(&self) -> Range<u64> {
        if self.viewport.end - self.viewport.start <= MAX_RAW {
            self.viewport.clone()
        } else if let Some(selection) = self
            .selection
            .as_ref()
            .filter(|rows| rows.end - rows.start <= MAX_RAW)
        {
            selection.clone()
        } else {
            let point = self.selected.unwrap_or(self.viewport.start);
            point.saturating_sub(64)..(point + 64).min(self.samples())
        }
    }

    fn samples(&self) -> u64 {
        self.index
            .as_ref()
            .map_or(self.preview.samples.len() as u64, |index| index.samples())
    }

    fn start_index(&mut self, ui: &Ui, source: &Arc<Source>) {
        if self.index.is_none() && self.index_job.is_none() && !self.index_failed {
            let source = source.clone();
            self.index_job = Some(launch(ui.ctx(), Duration::from_secs(90), move |cancel| {
                source.read(cancel, |input| tags::build_signal_index(input, cancel))
            }));
        }
    }

    fn start_window(&mut self, ui: &Ui, source: &Arc<Source>) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let range = self.desired();
        if range.end <= self.preview.samples.len() as u64
            || self.window_job.is_some()
            || self.window_failed.as_ref() == Some(&range)
            || self
                .window
                .as_ref()
                .is_some_and(|window| covers(&window.range, &range))
        {
            return;
        }
        let padding = MAX_RAW / 4;
        let read_start = range.start.saturating_sub(padding);
        let read_end = (range.end + padding).min(index.samples());
        let source = source.clone();
        self.window_job = Some((
            range,
            launch(ui.ctx(), Duration::from_secs(15), move |cancel| {
                let values = source.read(cancel, |input| {
                    tags::read_samples(input, &index, read_start..read_end, cancel)
                })?;
                let peaks = plot::sample_peaks(&values, 1);
                Ok(Window {
                    range: read_start..read_end,
                    values,
                    peaks,
                })
            }),
        ));
    }

    fn poll(&mut self) {
        if let Some(result) = poll(&mut self.index_job) {
            match result {
                Ok(index) => {
                    if self.viewport == (0..self.samples()) {
                        self.viewport = 0..index.samples();
                    }
                    self.index = Some(Arc::new(index));
                }
                Err(error) => {
                    self.index_failed = true;
                    self.error = Some(error);
                }
            }
        }
        if let Some((range, result)) = keyed_poll(&mut self.window_job) {
            match result {
                Ok(window) => self.window = Some(window),
                Err(error) => {
                    self.window_failed = Some(range);
                    self.error = Some(error);
                }
            }
        }
    }
}

fn covers(full: &Range<u64>, part: &Range<u64>) -> bool {
    full.start <= part.start && full.end >= part.end
}

fn copy_samples(
    source: &Source,
    index: &SignalIndex,
    rows: Range<u64>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    let first = rows.start;
    let values = source.read(cancel, |input| {
        tags::read_samples(input, index, rows, cancel)
    })?;
    let mut csv = String::from("sample,value\n");
    for (offset, value) in values.iter().enumerate() {
        let _ = writeln!(csv, "{},{value}", first + offset as u64 + 1);
    }
    little_clip_core::copy_text_to_system_clipboard(&csv).map_err(error)?;
    Ok(format!("Copied {} samples", values.len()))
}
