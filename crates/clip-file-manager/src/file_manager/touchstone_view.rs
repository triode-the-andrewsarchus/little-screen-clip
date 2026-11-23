//! Plots and exact source inspection for offline Touchstone sweeps.

mod capture;
mod markers;
mod plot;
pub(super) mod strip;

use super::{preview_copy::Copies, preview_job::Task};
use crate::{file_source::ReadCheck, ui_appearance::WorkbenchColors};
use capture::{Band, Capture, Trace};
use egui::{Context, PointerButton, Rect, Response, Sense, Ui, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::touchstone::{Index, Parameter, Summary, TracePoint},
    palette::AppPalette,
};
use markers::{Markers, Target};
use plot::{Projection, Smith};
use std::{
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(3, 5));
type Job<T> = Task<Result<T, String>>;

struct Indexed {
    index: Arc<Index>,
    points: Vec<TracePoint>,
}

pub(super) struct Viewer {
    capture: Arc<Capture>,
    summary: Summary,
    prefix: Vec<TracePoint>,
    trace: Trace,
    projection: Projection,
    full: Band,
    viewport: Band,
    handoff: Option<Band>,
    indexed: Option<Indexed>,
    index_job: Option<(Trace, Job<Indexed>)>,
    failed_trace: Option<Trace>,
    detail: Option<(Band, Vec<TracePoint>)>,
    detail_job: Option<(Band, Job<Vec<TracePoint>>)>,
    detail_failed: Option<Band>,
    detail_at: f64,
    markers: Markers,
    draft: Option<(usize, f64)>,
    inspect: bool,
    error: Option<String>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (capture, summary) = Capture::open(path, 1024, cancel)?;
        let trace = [0, 0];
        let prefix = capture::trace_points(&summary.points, trace);
        let full = plot::domain(&prefix);
        let projection = if summary.metadata.parameter == Parameter::S {
            Projection::Decibels
        } else {
            Projection::Magnitude
        };
        Ok(Self {
            capture: Arc::new(capture),
            summary,
            prefix,
            trace,
            projection,
            full,
            viewport: full,
            handoff: None,
            indexed: None,
            index_job: None,
            failed_trace: None,
            detail: None,
            detail_job: None,
            detail_failed: None,
            detail_at: 0.0,
            markers: Markers::default(),
            draft: None,
            inspect: false,
            error: None,
        })
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        if let Some(capture) = Arc::get_mut(&mut self.capture) {
            capture.check = check;
        }
    }

    pub fn focus_band(&mut self, band: Band) {
        if !self.summary.complete {
            self.handoff = Some(band);
        }
        self.viewport = plot::fit_band(band, self.full);
        if band.iter().all(|hz| hz.is_finite()) {
            self.markers.set(0, Target::Frequency(band[0]));
            self.markers.set(1, Target::Frequency(band[1]));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette, copies: &mut Copies) {
        self.poll();
        if let Some(hz) = self.markers.take_follow() {
            self.recenter(hz);
        }
        let colors = WorkbenchColors::from_palette(palette);
        let mut shell = crate::shell::StudioShell::new(
            "RF measurements",
            crate::icons::Icon::Crosshair,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.toolbar(ui);
        self.status(ui);
        self.draw_marker_tools(ui, copies, colors);
        let details = if self.inspect { 232.0 } else { 0.0 };
        let height = (ui.available_height() - 52.0 - details).max(96.0);
        self.graph(ui, height, colors);
        self.overview(ui, colors);
        let rows = self.row_count();
        if self.inspect
            && let Some(trace) = self
                .markers
                .inspect(ui, &self.summary.metadata, rows, self.trace)
        {
            self.select_trace(trace);
        }
        self.request(ui.ctx(), ui.input(|input| input.time));
        if self.pending() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let meta = &self.summary.metadata;
        let mut trace = self.trace;
        let ports = meta.ports;
        let s_parameters = meta.parameter == Parameter::S;
        let mut fit = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{}-port", ports));
            ui.weak(format!("Touchstone {}", meta.version.label()));
            ui.separator();
            if ports <= 4 {
                for response in 0..ports {
                    for stimulus in 0..ports {
                        let selected = [response, stimulus];
                        let label = meta.parameter_label(response, stimulus).unwrap_or_default();
                        if ui.selectable_label(trace == selected, label).clicked() {
                            trace = selected;
                        }
                    }
                }
            } else {
                ui.label(meta.parameter_label(trace[0], trace[1]).unwrap_or_default());
                port_picker(ui, "Response", &mut trace[0], ports);
                port_picker(ui, "Stimulus", &mut trace[1], ports);
            }
            ui.separator();
            for mode in [
                Projection::Decibels,
                Projection::Magnitude,
                Projection::Phase,
            ] {
                if mode != Projection::Decibels || s_parameters {
                    ui.selectable_value(&mut self.projection, mode, mode.title());
                }
            }
            if s_parameters && trace[0] == trace[1] {
                ui.selectable_value(&mut self.projection, Projection::Smith, "Smith");
            }
            fit = ui.button("Fit").clicked();
            ui.toggle_value(&mut self.inspect, "Matrix");
        });
        self.select_trace(trace);
        if fit {
            self.set_viewport(self.full);
        }
    }

    fn select_trace(&mut self, trace: Trace) {
        if trace == self.trace {
            return;
        }
        self.trace = trace;
        self.prefix = capture::trace_points(&self.summary.points, trace);
        if self.projection == Projection::Smith && trace[0] != trace[1] {
            self.projection = Projection::Decibels;
        }
        self.index_job = None;
        self.failed_trace = None;
        self.detail = None;
        self.detail_job = None;
        self.detail_failed = None;
        self.error = None;
    }

    fn status(&self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.weak(self.row_count().map_or_else(
                || format!("{} initial records", self.summary.points.len()),
                |rows| format!("{rows} frequencies"),
            ));
            if self.matching_index().is_none() && !self.summary.complete {
                ui.weak("Showing the beginning of the sweep");
            }
            if self.index_job.is_some() {
                ui.weak("Indexing sweep…");
            }
            if let Some(error) = self.error.as_deref() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }

    fn row_count(&self) -> Option<u64> {
        self.indexed
            .as_ref()
            .map(|indexed| indexed.index.points())
            .or(self.summary.metadata.declared_points)
            .or(self
                .summary
                .complete
                .then_some(self.summary.points.len() as u64))
    }

    fn graph(&mut self, ui: &mut Ui, height: f32, colors: WorkbenchColors) {
        let (outer, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let left = (outer.width() / 4.0).min(64.0);
        let rect = Rect::from_min_max(outer.min + vec2(left, 3.0), outer.max - vec2(9.0, 21.0));
        let painter = ui.painter_at(outer);
        painter.rect_filled(rect, 3.0, colors.recessed);
        if self.projection == Projection::Smith {
            self.smith(&painter, rect, &response, colors);
            response.on_hover_text("Click a trace point to select its source frequency");
        } else {
            self.frequency_plot(ui, &painter, rect, &response, colors);
            response.on_hover_text("Scroll to zoom · Middle-drag to pan · Click or drag to place a marker · Shift-drag to select a band");
        }
    }

    fn frequency_plot(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        response: &Response,
        colors: WorkbenchColors,
    ) {
        self.navigate(ui, rect, response);
        self.place_markers(ui, rect, response);
        plot::axes(painter, rect, self.viewport, colors.text_muted);
        if let Some(band) = self.markers.band(self.draft) {
            plot::highlight(painter, rect, self.viewport, band, colors.accent);
        }
        plot::trace(
            painter,
            rect,
            self.viewport,
            self.current_trace(),
            self.projection,
            colors.accent,
            true,
        );
        for (index, hz) in self.markers.positions(self.draft) {
            plot::marker(
                painter,
                rect,
                self.viewport,
                hz,
                Markers::label(index),
                Markers::color(colors, index),
            );
        }
    }

    fn smith(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        response: &Response,
        colors: WorkbenchColors,
    ) {
        let smith = Smith::new(rect);
        let data = plot::visible(self.current_trace(), self.viewport, false);
        let picked = response
            .hover_pos()
            .filter(|pos| rect.contains(*pos))
            .and_then(|pos| smith.nearest(pos, data));
        let clipped = painter.with_clip_rect(rect);
        smith.paint(&clipped, data, colors.accent, colors.text_muted);
        if let Some(point) = picked {
            smith.mark(&clipped, point.value, "", colors.text_secondary);
            if response.clicked_by(PointerButton::Primary) {
                self.markers
                    .set(self.markers.active, Target::Row(point.index));
            }
        }
        for (index, point) in self.markers.exact_points() {
            if (self.viewport[0]..=self.viewport[1]).contains(&point.frequency_hz)
                && let Some(gamma) = point.value(self.trace[0], self.trace[1])
            {
                smith.mark(
                    &clipped,
                    gamma,
                    Markers::label(index),
                    Markers::color(colors, index),
                );
            }
        }
    }

    fn navigate(&mut self, ui: &Ui, rect: Rect, response: &Response) {
        let width = self.viewport[1] - self.viewport[0];
        if response.dragged_by(PointerButton::Middle) {
            let delta = f64::from(response.drag_delta().x / rect.width()) * width;
            self.set_viewport([self.viewport[0] - delta, self.viewport[1] - delta]);
        }
        if let Some(pos) = response.hover_pos().filter(|pos| rect.contains(*pos)) {
            let dy = ui.input(|input| input.smooth_scroll_delta().y);
            if dy != 0.0 {
                let anchor = plot::hz_at(rect, self.viewport, pos.x);
                let factor = f64::from((-dy * 0.003).clamp(-2.0, 2.0).exp());
                self.set_viewport(self.viewport.map(|edge| anchor + (edge - anchor) * factor));
            }
        }
    }

    fn place_markers(&mut self, ui: &Ui, rect: Rect, response: &Response) {
        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        let hz = plot::hz_at(rect, self.viewport, pos.x);
        let selecting = ui.input(|input| input.modifiers.shift);
        if selecting
            && (response.dragged_by(PointerButton::Primary)
                || response.drag_stopped_by(PointerButton::Primary))
        {
            let origin = ui.input(|input| input.pointer.press_origin());
            if let Some(origin) = origin
                && (response.drag_started_by(PointerButton::Primary) || self.draft.is_none())
            {
                self.markers.set(
                    0,
                    Target::Frequency(plot::hz_at(rect, self.viewport, origin.x)),
                );
            }
            self.draft = Some((1, hz));
        } else if response.dragged_by(PointerButton::Primary) {
            let nearest = self.markers.near(rect, self.viewport, pos.x);
            let active = self.draft.map_or_else(
                || nearest.unwrap_or(self.markers.active),
                |(index, _)| index,
            );
            self.draft = Some((active, hz));
        } else if response.clicked_by(PointerButton::Primary) {
            self.markers.set(self.markers.active, Target::Frequency(hz));
        }
        if response.drag_stopped_by(PointerButton::Primary)
            && let Some((index, _)) = self.draft.take()
        {
            self.markers.set(index, Target::Frequency(hz));
        }
    }

    fn overview(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 39.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, colors.recessed);
        plot::highlight(&painter, rect, self.full, self.viewport, colors.focus);
        let projection = if self.summary.metadata.parameter == Parameter::S {
            Projection::Decibels
        } else {
            Projection::Magnitude
        };
        plot::trace(
            &painter,
            rect.shrink2(vec2(0.0, 3.0)),
            self.full,
            self.overview_trace(),
            projection,
            colors.accent,
            false,
        );
        for (index, hz) in self.markers.positions(self.draft) {
            plot::marker(
                &painter,
                rect,
                self.full,
                hz,
                Markers::label(index),
                Markers::color(colors, index),
            );
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let hz = plot::hz_at(rect, self.full, pos.x);
            let width = self.viewport[1] - self.viewport[0];
            if response.clicked_by(PointerButton::Primary) {
                let width = width.min((self.full[1] - self.full[0]) * 0.40);
                self.set_viewport([hz - width / 2.0, hz + width / 2.0]);
            } else if response.dragged_by(PointerButton::Primary) {
                let origin = ui.input(|input| input.pointer.press_origin());
                if let Some(origin) = origin {
                    self.set_viewport([plot::hz_at(rect, self.full, origin.x), hz]);
                }
            }
        }
        response.on_hover_text("Click to center or drag a frequency band");
    }

    fn draw_marker_tools(&mut self, ui: &mut Ui, copies: &mut Copies, colors: WorkbenchColors) {
        let (show_matrix, recenter) =
            self.markers
                .controls(ui, self.trace, &self.summary.metadata, colors);
        self.inspect |= show_matrix;
        if let Some(hz) = recenter {
            self.recenter(hz);
        }
        let rows = self.markers.exact_rows();
        let index = self.indexed.as_ref().map(|indexed| &indexed.index);
        let ready = rows.is_some_and(|[a, b]| a.abs_diff(b) < capture::MAX_COPY);
        if ui
            .add_enabled(
                ready && index.is_some() && !copies.busy(),
                egui::Button::new("Copy band as CSV"),
            )
            .on_disabled_hover_text("Place two exact markers, no more than 16,384 records apart")
            .clicked()
            && let (Some(index), Some(rows)) = (index, rows)
        {
            let index = index.clone();
            let capture = self.capture.clone();
            let trace = self.trace;
            copies.start(ui.ctx(), move |cancel| {
                capture.copy(&index, rows, trace, cancel)
            });
        }
    }

    fn recenter(&mut self, hz: f64) {
        if !(self.viewport[0]..=self.viewport[1]).contains(&hz) {
            let half = (self.viewport[1] - self.viewport[0]) / 2.0;
            self.set_viewport([hz - half, hz + half]);
        }
    }

    fn matching_index(&self) -> Option<&Indexed> {
        self.indexed
            .as_ref()
            .filter(|indexed| self.summary.complete || indexed.index.trace() == self.trace)
    }

    fn overview_trace(&self) -> &[TracePoint] {
        if self.summary.complete {
            &self.prefix
        } else {
            self.matching_index()
                .map_or(&self.prefix, |indexed| &indexed.points)
        }
    }

    fn current_trace(&self) -> &[TracePoint] {
        self.detail
            .as_ref()
            .filter(|(band, points)| *band == self.viewport && !points.is_empty())
            .map_or_else(|| self.overview_trace(), |(_, points)| points)
    }

    fn set_viewport(&mut self, band: Band) {
        let band = plot::fit_band(band, self.full);
        if band != self.viewport {
            self.viewport = band;
            self.handoff = None;
            self.detail_failed = None;
            self.detail_at = f64::INFINITY;
        }
    }

    fn poll(&mut self) {
        if let Some((trace, value)) = poll(&mut self.index_job) {
            match value {
                Ok(indexed) => {
                    let followed = self.viewport == self.full;
                    self.full = plot::pad_single(indexed.index.band_hz());
                    self.viewport = if let Some(band) = self.handoff.take() {
                        plot::fit_band(band, self.full)
                    } else if followed {
                        self.full
                    } else {
                        plot::fit_band(self.viewport, self.full)
                    };
                    self.indexed = Some(indexed);
                }
                Err(error) => {
                    self.failed_trace = Some(trace);
                    self.error = Some(error);
                }
            }
        }
        if let Some((band, value)) = poll(&mut self.detail_job) {
            match value {
                Ok(points) => self.detail = Some((band, points)),
                Err(error) => {
                    self.detail_failed = Some(band);
                    self.error = Some(error);
                }
            }
        }
        if let Some(error) = self.markers.poll() {
            self.error = Some(error);
        }
    }

    fn request(&mut self, ctx: &Context, now: f64) {
        if self.matching_index().is_none()
            && self.index_job.is_none()
            && self.failed_trace != Some(self.trace)
        {
            let capture = self.capture.clone();
            let trace = self.trace;
            self.index_job = Some((
                trace,
                read_job(ctx, Duration::from_secs(120), move |cancel| {
                    let index = capture.index(trace, cancel)?;
                    let points = capture::envelope_points(&index);
                    Ok(Indexed {
                        index: Arc::new(index),
                        points,
                    })
                }),
            ));
        }
        if self.detail_at == f64::INFINITY {
            self.detail_at = now + 0.06;
        }
        if now < self.detail_at {
            ctx.request_repaint_after(Duration::from_secs_f64(self.detail_at - now));
        } else if let Some(indexed) = self.indexed.as_ref()
            && !self.summary.complete
            && self.viewport != self.full
            && self.detail_failed != Some(self.viewport)
            && self.detail_job.is_none()
            && self
                .detail
                .as_ref()
                .is_none_or(|(band, _)| *band != self.viewport)
        {
            let capture = self.capture.clone();
            let index = indexed.index.clone();
            let band = self.viewport;
            let trace = self.trace;
            self.detail_job = Some((
                band,
                read_job(ctx, Duration::from_secs(10), move |cancel| {
                    capture.detail(&index, band, trace, cancel)
                }),
            ));
        }
        self.markers.request(
            ctx,
            &self.summary,
            &self.capture,
            self.indexed.as_ref().map(|indexed| &indexed.index),
        );
    }

    fn pending(&self) -> bool {
        self.index_job.is_some() || self.detail_job.is_some() || self.markers.pending()
    }
}

fn port_picker(ui: &mut Ui, name: &str, port: &mut usize, ports: usize) {
    egui::ComboBox::from_id_salt(("touchstone", name))
        .selected_text(format!("{name} {}", *port + 1))
        .show_ui(ui, |ui| {
            for index in 0..ports {
                ui.selectable_value(port, index, (index + 1).to_string());
            }
        });
}

fn read_job<T: Send + 'static>(
    ctx: &Context,
    timeout: Duration,
    work: impl FnOnce(&AtomicBool) -> Result<T, String> + Send + 'static,
) -> Job<T> {
    let ctx = ctx.clone();
    Task::start(&JOBS, timeout, move |cancel| {
        let result = work(cancel);
        ctx.request_repaint();
        result
    })
}

fn poll<K, T: Send + 'static>(job: &mut Option<(K, Job<T>)>) -> Option<(K, Result<T, String>)> {
    let result = job.as_ref()?.1.poll()?;
    let (key, _) = job.take()?;
    Some((
        key,
        result
            .map_err(|error| error.to_string())
            .and_then(|result| result),
    ))
}

fn marker_value(
    metadata: &little_clip_core::engineering::touchstone::Metadata,
    trace: Trace,
) -> Option<f64> {
    if metadata.parameter == Parameter::S && trace[0] == trace[1] && metadata.mixed_mode.is_empty()
    {
        metadata.reference_ohms.get(trace[0]).copied()
    } else {
        None
    }
}
