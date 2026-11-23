//! Offline logic timing and transition inspection.

mod frequency;
pub(super) mod plot;
pub(super) mod protocols;
pub(super) mod strip;

use super::{
    preview_copy::Copies,
    preview_job::Task,
    scope_view::plot as timing,
    signal_view::timeline::{self, Link},
};
use crate::file_source::ReadCheck;
use egui::{Color32, Context, PointerButton, Rect, Sense, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        logic::{Envelope, Level, Trace},
        logic_preview::{Overview, Recording},
        waveforms::display_value,
    },
};
use std::{
    io::Write,
    ops::Range,
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

const LANES_PER_PAGE: usize = 8;
const DISPLAY_BINS: usize = 1024;
const DISPLAY_EDGES: usize = 4096;
const MAX_COPY_EDGES: usize = 100_000;
static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 3));

pub(super) struct Viewer {
    recording: Arc<Recording>,
    overview: Overview,
    span: Range<f64>,
    active: usize,
    selected: Option<Range<f64>>,
    cursor: Option<f64>,
    frame: Option<Frame>,
    request: Option<WindowKey>,
    read_job: Option<Task<Result<Frame, String>>>,
    refinement: Option<Task<Result<(bool, Option<Overview>), String>>>,
    refined: bool,
    followed_cursor: Option<f64>,
    request_after: f64,
    show_transitions: bool,
    protocols: protocols::Panel,
    frequency: frequency::Frequency,
    error: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct SessionSettings {
    active: usize,
    selected: Option<Range<f64>>,
    cursor: Option<f64>,
    show_transitions: bool,
    protocols: protocols::SessionSettings,
    frequency: frequency::SavedFrequency,
}

#[derive(Clone, PartialEq)]
struct WindowKey {
    span: Range<f64>,
    visible: Vec<usize>,
    active: usize,
    show_transitions: bool,
}

struct Frame {
    key: WindowKey,
    lanes: Vec<Wave>,
    transitions: Option<Trace>,
}

enum Wave {
    Exact(Trace, Range<f64>),
    Summary(Envelope),
    Empty,
}

impl Viewer {
    pub(super) fn restoring_details(&self) -> bool {
        self.show_transitions && self.protocols.decoding()
    }

    pub(in crate::file_manager) fn session_settings(&self) -> SessionSettings {
        SessionSettings {
            active: self.active,
            selected: self.selected.clone(),
            cursor: self.cursor,
            show_transitions: self.show_transitions,
            protocols: self.protocols.session_settings(),
            frequency: self.frequency.save(),
        }
    }

    pub(in crate::file_manager) fn restore_session(
        &mut self,
        state: SessionSettings,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let count = self.overview.lanes.len();
        if state.active >= count {
            return Err("The saved logic channels do not match this recording".into());
        }
        self.protocols
            .restore_session(state.protocols, Some(&self.recording), cancel)?;
        self.active = state.active;
        self.selected = state.selected.filter(|range| !range.is_empty());
        self.cursor = state.cursor;
        self.show_transitions = state.show_transitions;
        self.frequency.restore(state.frequency, count);
        Ok(())
    }

    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let recording = Recording::open(path, cancel)?;
        let overview = recording.overview(cancel)?;
        let protocols = protocols::Panel::recorded();
        Ok(Self {
            span: overview.span.clone(),
            overview,
            recording: Arc::new(recording),
            active: 0,
            selected: None,
            cursor: None,
            frame: None,
            request: None,
            read_job: None,
            refinement: None,
            refined: false,
            followed_cursor: None,
            request_after: 0.0,
            show_transitions: false,
            protocols,
            frequency: frequency::Frequency::default(),
            error: None,
        })
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        if let Some(recording) = Arc::get_mut(&mut self.recording) {
            recording.with_check(check);
        }
    }

    pub fn focus_time(&mut self, start: f64, end: f64) {
        if start.is_finite() && end.is_finite() && end > start {
            self.selected = Some(start..end);
            self.cursor = Some(start);
            let padding = (end - start) * 0.08;
            self.span = self.fit(start - padding..end + padding);
        }
    }

    pub(super) fn domain(&self) -> Range<f64> {
        self.overview.span.clone()
    }

    pub(super) fn span(&mut self) -> &mut Range<f64> {
        &mut self.span
    }

    pub(super) fn recording(
        &self,
    ) -> (
        &Arc<Recording>,
        &[little_clip_core::engineering::logic::Lane],
    ) {
        (&self.recording, &self.overview.lanes)
    }

    pub(super) fn decoded_pins(&self) -> Option<&protocols::Pins> {
        self.protocols.pins()
    }

    pub(super) fn cursor(&self) -> Option<f64> {
        self.cursor
    }

    pub(super) fn min_height(&self) -> f32 {
        if self.show_transitions { 380.0 } else { 205.0 }
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        link: Option<&Link>,
    ) -> Option<f64> {
        self.poll(ui.ctx());
        self.refine(ui.ctx(), link.is_none());
        self.toolbar(ui, copies, Link::clock(link));
        if let Some(message) = self.error.as_ref().or(self.overview.note.as_ref()) {
            ui.weak(message);
        }
        super::signal_view::frequency_plot::origin_controls(
            ui,
            &mut self.frequency.spectra,
            &mut self.span,
            Link::clock(link),
        );
        if self.frequency.visible && self.frequency.spectra.origin().is_none() {
            self.protocols.close();
            self.frequency(ui, link);
            return None;
        }
        if self.show_transitions {
            self.protocols.controls(ui, &self.overview.lanes);
        }
        let reserved = if self.show_transitions { 238.0 } else { 39.0 };
        let height = (ui.available_height() - reserved).max(75.0);
        let probe = self.waveforms(ui, height, link);
        self.overview(ui);
        self.request_window(ui.ctx());
        if self.show_transitions && !self.protocols.edges() {
            self.protocols
                .update(ui.ctx(), self.span.clone(), self.refined, &self.recording);
            if let Some(event) =
                self.protocols
                    .table(ui, &self.span, self.cursor, self.selected.as_ref())
            {
                self.cursor = Some(event.time.start);
                ui.ctx().request_repaint();
            }
        } else {
            self.protocols.close();
            if self.show_transitions {
                self.transitions(ui);
            }
        }
        probe
    }

    fn toolbar(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        clock: little_clip_core::engineering::analysis::Clock,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.overview.label);
            ui.weak(format!("{} channels", self.overview.lanes.len()));
            ui.weak(display_value(
                self.overview.span.end - self.overview.span.start,
                "s",
            ));
            if self.frequency.visible || !self.show_transitions || self.protocols.edges() {
                egui::ComboBox::from_id_salt("logic_active_lane")
                    .selected_text(&self.overview.lanes[self.active].name)
                    .show_ui(ui, |ui| {
                        for (index, lane) in self.overview.lanes.iter().enumerate() {
                            if ui
                                .selectable_value(&mut self.active, index, &lane.name)
                                .changed()
                            {
                                self.frequency.spectra.clear_result();
                            }
                        }
                    });
            }
            if ui.button("Fit").clicked() {
                self.span = self.overview.span.clone();
            }
            if self.frequency.spectra.origin().is_none() {
                ui.toggle_value(&mut self.frequency.visible, "Spectrum");
            }
            ui.toggle_value(&mut self.show_transitions, "Transitions");
            if self.protocols.add_menu(ui, &self.overview.lanes) {
                self.show_transitions = true;
            }
            if !self.refined {
                ui.weak("Finding capture extent…");
            }
            if ui
                .add_enabled(
                    self.selected.is_some() && !copies.busy(),
                    egui::Button::new("Copy edges"),
                )
                .clicked()
                && let Some(range) = self.selected.clone()
            {
                let recording = self.recording.clone();
                let index = self.focused_lane();
                copies.start(ui.ctx(), move |cancel| {
                    export(&recording, index, range, cancel)
                });
            }
            if let Some(cursor) = self.cursor {
                ui.label(display_value(clock.to_workspace(cursor), "s"));
            }
            if let Some(range) = &self.selected {
                ui.weak(format!(
                    "Δ {}",
                    display_value((range.end - range.start) * clock.scale(), "s")
                ));
            }
        });
    }

    fn visible_lanes(&self) -> Vec<usize> {
        if self.show_transitions
            && let Some(channels) = self.protocols.channels()
        {
            return channels;
        }
        let first = self.active / LANES_PER_PAGE * LANES_PER_PAGE;
        (first..(first + LANES_PER_PAGE).min(self.overview.lanes.len())).collect()
    }

    fn focused_lane(&self) -> usize {
        if self.show_transitions
            && let Some(lane) = self.protocols.data_lane()
        {
            return lane;
        }
        self.active
    }

    fn waveforms(&mut self, ui: &mut Ui, height: f32, link: Option<&Link>) -> Option<f64> {
        let (outer, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let annotations = self.show_transitions && !self.protocols.edges();
        let top = if annotations { 25.0 } else { 3.0 };
        let rect = Rect::from_min_max(outer.min + vec2(78.0, top), outer.max - vec2(9.0, 18.0));
        if let Some(span) =
            timeline::navigate(ui.ctx(), rect, &response, &self.span, link.is_some())
        {
            let domain = link.map_or(&self.overview.span, |link| &link.domain);
            self.span = timeline::fit(domain, span, self.interval());
        }
        self.move_cursor(ui, rect, &response);
        let painter = ui.painter_at(outer);
        let ink = ui.visuals().weak_text_color();
        if annotations {
            let band = Rect::from_min_max(pos2(rect.left(), outer.top()), rect.right_top());
            let events = self.protocols.events(&self.span);
            plot::annotations(
                &painter,
                band,
                &self.span,
                events,
                ui.visuals().selection.bg_fill,
                ink,
            );
            if response.clicked_by(PointerButton::Primary)
                && let Some(point) = response
                    .interact_pointer_pos()
                    .filter(|point| band.contains(*point))
            {
                self.cursor = Some(timing::at(band, &self.span, point.x));
            }
        }
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let axis = Link::clock(link).workspace_range(&self.span);
        timing::axes(&painter, rect, &axis, ink);
        let clipped = painter.with_clip_rect(rect);
        self.draw_channels(&painter, &clipped, rect, ink);
        super::signal_view::frequency_plot::paint_origin(
            ui,
            &clipped,
            rect,
            &self.span,
            &self.frequency.spectra,
        );
        if let Some(link) = link {
            link.paint_markers(&clipped, rect, &self.span);
        }
        if let Some(selected) = &self.selected {
            timing::highlight(
                &clipped,
                rect,
                &self.span,
                selected.clone(),
                Color32::from_white_alpha(25),
            );
        }
        let event = self
            .show_transitions
            .then(|| self.protocols.cursor_event(&self.span, self.cursor))
            .flatten();
        if let Some(event) = event
            && !event.time.is_empty()
        {
            timing::highlight(
                &clipped,
                rect,
                &self.span,
                event.time.clone(),
                Color32::from_rgba_unmultiplied(241, 189, 76, 75),
            );
        }
        if let Some(cursor) = self.cursor {
            let x = timing::x_at(rect, &self.span, cursor);
            let color = if event.is_some() {
                Color32::GOLD
            } else {
                Color32::WHITE
            };
            clipped.vline(x, rect.y_range(), Stroke::new(1.25, color));
        }
        let probe = response
            .hover_pos()
            .filter(|point| rect.contains(*point))
            .map(|point| timing::at(rect, &self.span, point.x));
        if let Some(time) = probe.or(link.and_then(|link| link.probe)) {
            let x = timing::x_at(rect, &self.span, time);
            clipped.vline(x, rect.y_range(), Stroke::new(1.0, ink));
        }
        probe
    }

    fn draw_channels(
        &self,
        painter: &egui::Painter,
        clipped: &egui::Painter,
        rect: Rect,
        ink: Color32,
    ) {
        let visible = self.visible_lanes();
        let selected = self.focused_lane();
        let frame = self.frame.as_ref().filter(|frame| {
            frame.key.visible == visible
                && frame.key.span.start <= self.span.start
                && frame.key.span.end >= self.span.end
        });
        let count = visible.len();
        for (offset, index) in visible.into_iter().enumerate() {
            let lane = timing::lane(rect, offset, count);
            let color = if selected == index {
                Color32::from_rgb(130, 225, 163)
            } else {
                ink
            };
            painter.text(
                pos2(lane.left() - 5.0, lane.center().y),
                egui::Align2::RIGHT_CENTER,
                &self.overview.lanes[index].name,
                egui::FontId::monospace(11.0),
                color,
            );
            clipped.hline(
                lane.x_range(),
                lane.bottom(),
                Stroke::new(0.5, ink.gamma_multiply(0.24)),
            );
            let inner = lane.shrink2(vec2(0.0, lane.height() * 0.2));
            match frame.and_then(|frame| frame.lanes.get(offset)) {
                Some(Wave::Exact(trace, time)) => {
                    plot::draw_trace(clipped, inner, &self.span, time, trace, color)
                }
                Some(Wave::Summary(envelope)) => {
                    plot::draw_envelope(clipped, inner, &self.span, envelope, color)
                }
                Some(Wave::Empty) => {}
                None => {
                    if let Some(envelope) = self.overview.lanes_previewed.get(index) {
                        plot::draw_envelope(clipped, inner, &self.span, envelope, color);
                    }
                }
            }
        }
    }

    fn overview(&mut self, ui: &mut Ui) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        if let Some(envelope) = self
            .overview
            .lanes_previewed
            .get(self.focused_lane())
            .or_else(|| self.overview.lanes_previewed.first())
        {
            plot::draw_envelope(
                &painter,
                rect.shrink(2.0),
                &self.overview.span,
                envelope,
                Color32::from_rgb(130, 225, 163),
            );
        }
        timing::highlight(
            &painter,
            rect,
            &self.overview.span,
            self.span.clone(),
            Color32::from_white_alpha(50),
        );
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let center = timing::at(rect, &self.overview.span, pointer.x);
            self.recenter(center);
        }
        response.on_hover_text("Navigate the capture");
    }

    fn transitions(&mut self, ui: &mut Ui) {
        let Some(trace) = self
            .frame
            .as_ref()
            .filter(|frame| frame.key.active == self.active)
            .and_then(|frame| frame.transitions.as_ref())
        else {
            ui.weak("Reading transitions…");
            return;
        };
        let selected = self.cursor.and_then(|time| {
            trace
                .edges
                .partition_point(|edge| edge.time_seconds <= time)
                .checked_sub(1)
        });
        if trace.inspected_until < self.span.end {
            ui.weak(format!(
                "Exact through {} · Zoom for more transitions",
                display_value(trace.inspected_until, "s")
            ));
        }
        let destination = event_list(
            ui,
            ("logic-edge-rows", "Level"),
            self.cursor,
            &mut self.followed_cursor,
            selected,
            trace.edges.len(),
            |index| {
                let edge = &trace.edges[index];
                (
                    display_value(edge.time_seconds, "s"),
                    level_label(edge.level).to_owned(),
                )
            },
        );
        if let Some(index) = destination {
            let time = trace.edges[index].time_seconds;
            self.cursor = Some(time);
            self.recenter(time);
        }
    }

    fn move_cursor(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        let Some(pointer) = response.interact_pointer_pos() else {
            return;
        };
        let time = timing::at(rect, &self.span, pointer.x);
        if response.clicked_by(PointerButton::Primary) && rect.contains(pointer) {
            self.selected = None;
            self.cursor = Some(time);
        }
        if response.dragged_by(PointerButton::Primary)
            && let Some(origin) = ui
                .input(|input| input.pointer.press_origin())
                .filter(|p| rect.contains(*p))
        {
            let start = timing::at(rect, &self.span, origin.x);
            self.selected = (time != start).then(|| time.min(start)..time.max(start));
            self.cursor = Some(time);
        }
    }

    fn recenter(&mut self, center: f64) {
        let half = (self.span.end - self.span.start) * 0.5;
        self.span = self.fit(center - half..center + half);
    }

    fn interval(&self) -> f64 {
        self.overview.lanes[self.focused_lane()]
            .sample_rate
            .map_or(1e-9, f64::recip)
    }

    fn fit(&self, requested: Range<f64>) -> Range<f64> {
        timeline::fit(&self.overview.span, requested, self.interval())
    }

    fn poll(&mut self, ctx: &Context) {
        if let Some(result) = self.read_job.as_ref().and_then(Task::poll) {
            self.read_job = None;
            match result
                .map_err(|error| error.to_string())
                .and_then(|value| value)
            {
                Ok(frame) => {
                    self.frame = Some(frame);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
        if self.read_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn refine(&mut self, ctx: &Context, follow_extent: bool) {
        if let Some(result) = self.refinement.as_ref().and_then(Task::poll) {
            self.refinement = None;
            match result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                Ok((finished, overview)) => {
                    self.refined = finished;
                    if let Some(overview) = overview {
                        let fit = self.span == self.overview.span;
                        self.overview = overview;
                        if fit && follow_extent {
                            self.span = self.overview.span.clone();
                        }
                    }
                }
                Err(error) => {
                    self.error = Some(error);
                    self.refined = true;
                }
            }
        }
        if !self.refined && self.refinement.is_none() {
            let recording = self.recording.clone();
            let previous = self.overview.span.clone();
            self.refinement = Some(Task::start(&JOBS, Duration::from_secs(30), move |cancel| {
                recording.refine(previous, cancel)
            }));
        }
        if self.refinement.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn request_window(&mut self, ctx: &Context) {
        let key = WindowKey {
            span: self.span.clone(),
            visible: self.visible_lanes(),
            active: self.focused_lane(),
            show_transitions: self.show_transitions && self.protocols.edges(),
        };
        if self.request.as_ref() == Some(&key) || self.read_job.is_some() {
            return;
        }
        let now = ctx.input(|input| input.time);
        if now < self.request_after {
            ctx.request_repaint_after(Duration::from_secs_f64(self.request_after - now));
            return;
        }
        self.request_after = now + 0.06;
        self.request = Some(key.clone());
        let recording = self.recording.clone();
        self.read_job = Some(Task::start(&JOBS, Duration::from_secs(30), move |cancel| {
            read_frame(&recording, key, cancel)
        }));
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn event_list(
    ui: &mut Ui,
    (id, heading): (&'static str, &'static str),
    cursor: Option<f64>,
    followed_cursor: &mut Option<f64>,
    selected: Option<usize>,
    count: usize,
    mut values: impl FnMut(usize) -> (String, String),
) -> Option<usize> {
    ui.spacing_mut().item_spacing.y = 1.0;
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
    let time_width = (ui.available_width() * 0.32).clamp(40.0, 111.0);
    let mut table = TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .sense(Sense::click())
        .auto_shrink([false, false])
        .column(Column::exact(time_width).clip(true))
        .column(Column::remainder().clip(true));
    if *followed_cursor != cursor {
        if let Some(row) = selected {
            table = table.scroll_to_row(row, None);
        }
        *followed_cursor = cursor;
    }
    let mut clicked = None;
    table
        .header(row_height, |mut row| {
            row.col(|ui| {
                ui.strong("Time");
            });
            row.col(|ui| {
                ui.strong(heading);
            });
        })
        .body(|body| {
            body.rows(row_height, count, |mut row| {
                let index = row.index();
                let (time, value) = values(index);
                row.set_selected(selected == Some(index));
                row.col(|ui| {
                    ui.monospace(time);
                });
                row.col(|ui| {
                    ui.monospace(value);
                });
                if row.response().clicked() {
                    clicked = Some(index);
                }
            });
        });
    clicked
}

fn read_frame(recording: &Recording, key: WindowKey, cancel: &AtomicBool) -> Result<Frame, String> {
    recording.checked(cancel, |capture| {
        let mut transitions = None;
        let mut lanes = Vec::new();
        let width = key.span.end - key.span.start;
        for &index in &key.visible {
            let lane = &capture.lanes()[index];
            let bounds = capture
                .time_bounds(index)
                .unwrap_or_else(|| key.span.clone());
            let span = key.span.start.max(bounds.start)..key.span.end.min(bounds.end);
            if span.is_empty() {
                lanes.push(Wave::Empty);
                continue;
            }
            let zoomed = lane
                .sample_rate
                .is_none_or(|rate| rate * width <= 131_072.0);
            let trace = (zoomed || (key.show_transitions && index == key.active))
                .then(|| capture.read_edges(index, span.clone(), DISPLAY_EDGES, cancel))
                .transpose()?;
            if key.show_transitions && index == key.active {
                transitions = trace.as_ref().map(|trace| Trace {
                    initial: trace.initial,
                    edges: trace.edges.clone(),
                    inspected_until: trace.inspected_until,
                });
            }
            if let Some(trace) = trace
                && trace.inspected_until >= span.end
            {
                lanes.push(Wave::Exact(trace, span));
            } else {
                lanes.push(Wave::Summary(capture.summarize(
                    index,
                    span,
                    DISPLAY_BINS,
                    65_536,
                    cancel,
                )?));
            }
        }
        Ok(Frame {
            key,
            lanes,
            transitions,
        })
    })
}

pub(super) fn level_label(level: Level) -> &'static str {
    match level {
        Level::Low => "0",
        Level::High => "1",
        Level::Unknown => "X",
        Level::HighImpedance => "Z",
    }
}

fn export(
    recording: &Recording,
    lane: usize,
    span: Range<f64>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let count = recording.checked(cancel, |capture| {
        let bounds = capture.time_bounds(lane).ok_or("Missing logic channel")?;
        let start = span.start.max(bounds.start);
        let end = span.end.min(bounds.end);
        if start >= end {
            return Err("Select a region that contains samples".into());
        }
        let trace = capture.read_edges(lane, start..end, MAX_COPY_EDGES, cancel)?;
        if trace.inspected_until < end {
            return Err("Zoom or select a shorter region to copy every edge".into());
        }
        super::preview_copy::copy_file(recording.source().path(), "edges", "csv", |file| {
            let mut output = std::io::BufWriter::new(file);
            writeln!(output, "Time (s),Level").map_err(|error| error.to_string())?;
            if let Some(level) = trace.initial {
                writeln!(output, "{start},{}", level_label(level))
                    .map_err(|error| error.to_string())?;
            }
            for edge in &trace.edges {
                writeln!(output, "{},{}", edge.time_seconds, level_label(edge.level))
                    .map_err(|error| error.to_string())?;
            }
            output.flush().map_err(|error| error.to_string())
        })?;
        Ok(trace.edges.len())
    })?;
    Ok(format!("Copied {count} transitions"))
}
