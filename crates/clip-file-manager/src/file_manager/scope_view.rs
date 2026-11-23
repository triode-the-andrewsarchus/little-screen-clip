//! File-backed oscilloscope plots with one timeline shared by cursors and analysis.

#[cfg(test)]
mod frame_benchmark;
mod math;
pub(super) mod plot;
mod reads;
mod snap;
pub(super) mod strip;

use super::{preview_copy::Copies, preview_job::Task};
use crate::file_source::ReadCheck;
use egui::{Color32, Context, PointerButton, Rect, Sense, Stroke, Ui, pos2};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::SpectrumWindow,
        waveforms::{Envelope, Overview, Recording, display_value, time_span},
    },
    palette::AppPalette,
};
use reads::{AnalysisRequest, Measurements, RawChannel};
use snap::CursorSnap;
use std::{
    ops::Range,
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(3, 4));

pub(super) struct Viewer {
    recording: Arc<Recording>,
    overview: Overview,
    domain: Range<f64>,
    span: Range<f64>,
    active: usize,
    cursors: [Option<f64>; 2],
    drag: Option<usize>,
    index: Option<Vec<Envelope>>,
    index_job: Option<Task<Result<Vec<Envelope>, String>>>,
    index_started: bool,
    detail: Option<(Range<f64>, PlotData)>,
    detail_job: Option<(Range<f64>, Task<Result<PlotData, String>>)>,
    measurements: Option<Measurements>,
    measurement_key: Option<AnalysisKey>,
    analysis_job: Option<Task<Result<Measurements, String>>>,
    analysis_key: Option<AnalysisKey>,
    analysis_dirty_at: f64,
    math: String,
    math_state: math::Math,
    spectrum: bool,
    spectrum_window: SpectrumWindow,
    spectrum_db: bool,
    spectrum_marker: Option<usize>,
    spectrum_span: Option<Range<f64>>,
    snaps: [CursorSnap; 2],
    error: Option<String>,
}

enum PlotData {
    Raw(Vec<RawChannel>),
    Summary(Vec<Envelope>),
}

impl PlotData {
    fn raw(&self, index: usize) -> Option<&RawChannel> {
        match self {
            Self::Raw(channels) => channels.get(index),
            Self::Summary(_) => None,
        }
    }
}

#[derive(Clone, PartialEq)]
struct AnalysisKey {
    span: Range<f64>,
    channel: usize,
    fft: Option<SpectrumWindow>,
    expression: String,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let recording = Recording::open(path, ReadCheck::default(), cancel)?;
        let overview = recording.overview(cancel)?;
        let domain = time_span(&overview.channels);
        Ok(Self {
            recording: Arc::new(recording),
            overview,
            span: domain.clone(),
            domain,
            active: 0,
            cursors: [None; 2],
            drag: None,
            index: None,
            index_job: None,
            index_started: false,
            detail: None,
            detail_job: None,
            measurements: None,
            measurement_key: None,
            analysis_job: None,
            analysis_key: None,
            analysis_dirty_at: 0.0,
            math: String::new(),
            math_state: math::Math::default(),
            spectrum: false,
            spectrum_window: SpectrumWindow::Hann,
            spectrum_db: true,
            spectrum_marker: None,
            spectrum_span: None,
            snaps: std::array::from_fn(|_| CursorSnap::default()),
            error: None,
        })
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        // The loading job has not submitted any deferred reads when it hands
        // this recording to the session.
        if let Some(recording) = Arc::get_mut(&mut self.recording) {
            recording.set_read_check(check);
        }
    }

    pub fn focus_samples(&mut self, start: u64, end: u64) {
        let channel = &self.overview.channels[0];
        let left = channel.time_at(start.min(channel.sample_count.saturating_sub(1)));
        let right = channel.time_at(end.max(start.saturating_add(1)).min(channel.sample_count));
        self.cursors = [Some(left), Some(right)];
        let pad = (right - left) * 0.08;
        self.span = self.clamp_span(left - pad..right + pad);
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette, copies: &mut Copies) {
        self.poll(ui.ctx());
        let (label, icon) = if self.recording.power_recording() {
            ("Power recording", crate::icons::Icon::Zap)
        } else {
            ("Oscilloscope", crate::icons::Icon::Waves)
        };
        let mut shell = crate::shell::StudioShell::new(label, icon, palette, close);
        shell.render_header(ui, |_| {});
        self.toolbar(ui, copies);
        self.power_totals(ui);
        let has_spectrum = self.spectrum;
        let plot_height = if has_spectrum {
            (ui.available_height() * 0.54).max(80.0)
        } else {
            (ui.available_height() - 57.0).max(80.0)
        };
        self.waveform(ui, plot_height);
        self.overview_ui(ui);
        self.cursor_readout(ui);
        if has_spectrum {
            self.spectrum_ui(ui);
        }
        self.request_detail();
        self.math_state
            .request(&self.recording, ui.ctx(), &self.span);
        for snap in &mut self.snaps {
            snap.request(&self.recording, ui.ctx());
        }
        self.request_analysis(ui.ctx());
        if self.pending() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, copies: &mut Copies) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(&self.overview.model);
            ui.weak(display_value(self.overview.duration_seconds(), "s"));
            ui.separator();
            for (index, channel) in self.overview.channels.iter().enumerate() {
                let frequency = self
                    .measurements
                    .as_ref()
                    .and_then(|measured| measured.frequencies.get(index).copied().flatten());
                let text = if self.recording.power_recording() {
                    channel.name.clone()
                } else {
                    format!(
                        "{} · {}",
                        channel.name,
                        display_value(frequency.unwrap_or(f64::NAN), "Hz")
                    )
                };
                let selected = self.active == index;
                let label = egui::RichText::new(text);
                let label = if selected {
                    label
                } else {
                    label.color(plot::color(index))
                };
                if ui.selectable_label(selected, label).clicked() {
                    if index != self.active {
                        self.snaps.iter_mut().for_each(CursorSnap::clear);
                    }
                    self.active = index;
                }
            }
            if ui.button("Fit").clicked() {
                self.span = self.domain.clone();
            }
            ui.toggle_value(&mut self.spectrum, "Spectrum");
            let range = self.selection();
            if ui
                .add_enabled(
                    range.is_some() && !copies.busy(),
                    egui::Button::new("Copy samples"),
                )
                .clicked()
                && let Some(range) = range
            {
                let recording = self.recording.clone();
                let index = self.active;
                copies.start(ui.ctx(), move |cancel| {
                    copy_samples(&recording, index, range, cancel)
                });
            }
        });
        self.math_controls(ui);
    }

    fn math_controls(&mut self, ui: &mut Ui) {
        let channel = &self.overview.channels[self.active];
        if !self.recording.power_recording() || channel.unit == "V" {
            let status = self
                .math_state
                .status(channel, &self.span, self.math_envelope());
            ui.horizontal(|ui| {
                ui.label("Math");
                let hint = if self.recording.power_recording() {
                    "> 1V"
                } else {
                    "> 1V or CH1 - CH2"
                };
                ui.add(
                    egui::TextEdit::singleline(&mut self.math)
                        .hint_text(hint)
                        .char_limit(256)
                        .desired_width((ui.available_width() - 10.0).min(420.0)),
                );
                if let Some(status) = status {
                    ui.weak(status);
                }
            });
        }
        let input = if self.recording.power_recording() && channel.unit != "V" {
            ""
        } else {
            &self.math
        };
        self.math_state
            .compile(input, self.active, &self.overview.channels);
        if let Some(message) = self.math_state.error().or(self.error.as_deref()) {
            ui.colored_label(ui.visuals().error_fg_color, message);
        } else if self.index_job.is_some() {
            ui.weak("Refining overview…");
        }
    }

    fn math_envelope(&self) -> &Envelope {
        if let Some((cover, PlotData::Summary(envelopes))) = &self.detail
            && contains(cover, &self.span)
            && let Some(envelope) = envelopes.get(self.active)
        {
            return envelope;
        }
        &self.index.as_ref().unwrap_or(&self.overview.envelopes)[self.active]
    }

    fn waveform(&mut self, ui: &mut Ui, height: f32) {
        let (outer, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height.max(40.0)),
            Sense::click_and_drag(),
        );
        let rect = Rect::from_min_max(
            outer.min + egui::vec2(56.0, 3.0),
            outer.max - egui::vec2(8.0, 16.0),
        );
        self.navigate(ui, rect, &response);
        self.move_cursors(ui, rect, &response);
        let painter = ui.painter_at(outer);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let ink = ui.visuals().weak_text_color();
        plot::axes(&painter, rect, &self.span, ink);
        let clipped = painter.with_clip_rect(rect);
        if let Some(selection) = self.selection() {
            plot::highlight(
                &clipped,
                rect,
                &self.span,
                selection,
                Color32::from_white_alpha(25),
            );
        }
        self.draw_channels(&painter, &clipped, rect, ink);
        self.draw_math(&clipped, rect);
        for (label, time) in ["A", "B"]
            .into_iter()
            .zip(self.cursors)
            .filter_map(|(label, time)| time.map(|time| (label, time)))
        {
            let x = plot::x_at(rect, &self.span, time);
            if (rect.left()..=rect.right()).contains(&x) {
                clipped.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.4, Color32::WHITE),
                );
                clipped.text(
                    pos2(x + 4.0, rect.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::monospace(12.0),
                    Color32::WHITE,
                );
            }
        }
        response.on_hover_text("Scroll to zoom at the pointer · Middle drag to pan · Drag A/B to measure · Shift bypasses zero-crossing snap");
    }

    fn draw_channels(
        &self,
        painter: &egui::Painter,
        clipped: &egui::Painter,
        rect: Rect,
        ink: Color32,
    ) {
        let channels = &self.overview.channels;
        let envelopes = self.index.as_ref().unwrap_or(&self.overview.envelopes);
        for (index, (channel, envelope)) in channels.iter().zip(envelopes).enumerate() {
            let lane = plot::lane(rect, index, channels.len()).shrink2(egui::vec2(0.0, 5.0));
            let bounds = envelope.extrema();
            if !bounds.valid() {
                continue;
            }
            let channel_color = plot::color(index);
            let axis = lane.left() - 5.0;
            for (y, value) in [(lane.top(), bounds.max), (lane.bottom(), bounds.min)] {
                painter.text(
                    pos2(axis, y),
                    egui::Align2::RIGHT_CENTER,
                    display_value(f64::from(value), &channel.unit),
                    egui::FontId::monospace(10.0),
                    ink,
                );
            }
            if bounds.min <= 0.0 && bounds.max >= 0.0 {
                let y = plot::y_at(lane, bounds, 0.0);
                clipped.hline(
                    lane.x_range(),
                    y,
                    Stroke::new(1.0, ink.gamma_multiply(0.38)),
                );
            }
            let detail = self
                .detail
                .as_ref()
                .filter(|(cover, _)| contains(cover, &self.span))
                .map(|(_, detail)| detail);
            if let Some(raw) = detail.and_then(|detail| detail.raw(index)) {
                let (start, samples) = raw.visible(&self.span);
                plot::draw_raw(
                    clipped,
                    lane,
                    &self.span,
                    start,
                    raw.step,
                    samples,
                    bounds,
                    channel_color,
                );
            } else {
                let envelope = match detail {
                    Some(PlotData::Summary(envelopes)) => &envelopes[index],
                    _ => envelope,
                };
                plot::draw_envelope(
                    clipped,
                    lane,
                    &self.span,
                    channel,
                    envelope,
                    bounds,
                    channel_color,
                );
            }
        }
    }

    fn draw_math(&self, painter: &egui::Painter, rect: Rect) {
        let lane = plot::lane(rect, self.active, self.overview.channels.len());
        self.math_state.paint(
            painter,
            lane,
            &self.span,
            &self.overview.channels[self.active],
            self.math_envelope(),
        );
        if self
            .measurement_key
            .as_ref()
            .is_none_or(|key| key.expression != self.math || key.channel != self.active)
        {
            return;
        }
        let Some((raw, bounds)) = self
            .measurements
            .as_ref()
            .and_then(|data| data.math.as_ref())
        else {
            return;
        };
        let (start, samples) = raw.visible(&self.span);
        let stride = samples
            .len()
            .div_ceil((lane.width() as usize).max(8))
            .max(1);
        let display = samples.iter().copied().step_by(stride).collect::<Vec<_>>();
        plot::draw_raw(
            painter,
            lane,
            &self.span,
            start,
            raw.step * stride as f64,
            &display,
            *bounds,
            Color32::from_rgb(235, 242, 125),
        );
    }

    fn overview_ui(&mut self, ui: &mut Ui) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 26.0),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let channel = &self.overview.channels[self.active];
        let envelope = self.index.as_ref().unwrap_or(&self.overview.envelopes);
        let envelope = &envelope[self.active];
        plot::draw_envelope(
            &painter,
            rect.shrink(2.0),
            &self.domain,
            channel,
            envelope,
            envelope.extrema(),
            plot::color(self.active),
        );
        plot::highlight(
            &painter,
            rect,
            &self.domain,
            self.span.clone(),
            Color32::from_white_alpha(65),
        );
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let center = plot::at(rect, &self.domain, pointer.x);
            let half = (self.span.end - self.span.start) / 2.0;
            self.span = self.clamp_span(center - half..center + half);
        }
        response.on_hover_text("Navigate the full capture");
    }

    fn cursor_readout(&self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let unit = &self.overview.channels[self.active].unit;
            for (label, time) in ["A", "B"].into_iter().zip(self.cursors) {
                if let Some(time) = time {
                    let value = self
                        .detail
                        .as_ref()
                        .and_then(|(_, detail)| detail.raw(self.active)?.value_at(time))
                        .or_else(|| {
                            if self.measurement_key.as_ref()?.channel == self.active {
                                self.measurements.as_ref()?.raw.value_at(time)
                            } else {
                                None
                            }
                        });
                    ui.label(format!(
                        "{label}  {}  {}",
                        display_value(time, "s"),
                        display_value(value.map_or(f64::NAN, f64::from), unit)
                    ));
                }
            }
            if let Some(range) = self.selection() {
                let width = range.end - range.start;
                ui.label(format!(
                    "Δt  {}    1/Δt  {}",
                    display_value(width, "s"),
                    display_value(width.recip(), "Hz")
                ));
            }
            if let Some(measured) = &self.measurements
                && self
                    .measurement_key
                    .as_ref()
                    .is_some_and(|key| key.channel == self.active)
            {
                if measured.extrema.valid() {
                    ui.weak(format!(
                        "Pk–Pk  {}",
                        display_value(f64::from(measured.extrema.max - measured.extrema.min), unit)
                    ));
                }
                if measured.limited {
                    ui.weak("Waveform measurements use a 65k-sample window");
                }
            }
        });
    }

    fn power_totals(&self, ui: &mut Ui) {
        if !self.recording.power_recording() {
            return;
        }
        let selected = self.selection();
        let span = selected.as_ref().unwrap_or(&self.span);
        let totals = self
            .measurement_key
            .as_ref()
            .filter(|key| key.span == *span)
            .and_then(|_| self.measurements.as_ref()?.power_totals.as_ref());
        ui.horizontal_wrapped(|ui| {
            ui.strong(if selected.is_some() {
                "Selection"
            } else {
                "Visible interval"
            });
            if let Some(totals) = totals {
                for (name, unit, integral) in [
                    ("Charge", "C", &totals.charge),
                    ("Energy", "J", &totals.energy),
                ] {
                    let value = integral.as_ref().map_or_else(
                        || format!("- {unit}"),
                        |integral| {
                            format!(
                                "{}{}{}",
                                if integral.approximate { "≈ " } else { "" },
                                display_value(integral.value, unit),
                                if integral.partial { " (partial)" } else { "" },
                            )
                        },
                    );
                    ui.label(format!("{name}  {value}"));
                }
            } else {
                ui.weak("Measuring charge and energy…");
            }
        });
    }

    fn pending(&self) -> bool {
        self.index_job.is_some()
            || self.detail_job.is_some()
            || self.analysis_job.is_some()
            || self.snaps.iter().any(CursorSnap::pending)
            || self.math_state.pending()
            || self.analysis_key.is_none()
    }

    fn selection(&self) -> Option<Range<f64>> {
        let [Some(a), Some(b)] = self.cursors else {
            return None;
        };
        (a != b).then(|| a.min(b)..a.max(b))
    }

    fn clamp_span(&self, requested: Range<f64>) -> Range<f64> {
        let total = self.domain.end - self.domain.start;
        let min = (self.overview.channels[self.active].interval_seconds * 4.0).min(total);
        let width = (requested.end - requested.start).clamp(min, total);
        let first = requested.start.clamp(
            self.domain.start,
            (self.domain.end - width).max(self.domain.start),
        );
        first..first + width
    }

    fn navigate(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        let pointer = ui.input(|input| input.pointer.hover_pos());
        if !response.hovered() || !pointer.is_some_and(|pointer| rect.contains(pointer)) {
            return;
        }
        let (scroll, movement) =
            ui.input(|input| (input.smooth_scroll_delta, input.pointer.delta()));
        let span = self.span.end - self.span.start;
        if scroll.y != 0.0
            && let Some(pointer) = pointer
        {
            let t = plot::at(rect, &self.span, pointer.x);
            let scale = f64::from((-scroll.y * 0.012).exp()).clamp(0.1, 10.0);
            self.span =
                self.clamp_span(t + (self.span.start - t) * scale..t + (self.span.end - t) * scale);
        }
        let drag = if response.dragged_by(PointerButton::Middle) {
            movement.x
        } else {
            0.0
        };
        let offset = -f64::from((scroll.x + drag) / rect.width().max(1.0)) * span;
        if offset != 0.0 {
            self.span = self.clamp_span(self.span.start + offset..self.span.end + offset);
        }
    }

    fn move_cursors(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        let Some(point) = response.interact_pointer_pos() else {
            return;
        };
        let time = plot::at(rect, &self.span, point.x);
        let radius = (self.span.end - self.span.start) * 10.0 / f64::from(rect.width().max(1.0));
        let free = ui.input(|input| input.modifiers.shift);
        let cursors = self.cursors;
        let near = |t: f64| {
            cursors
                .iter()
                .position(|value| value.is_some_and(|value| (value - t).abs() <= radius))
        };
        if response.drag_started_by(PointerButton::Primary) {
            let origin = ui
                .input(|input| input.pointer.press_origin())
                .unwrap_or(point);
            let origin = plot::at(rect, &self.span, origin.x);
            self.snaps.iter_mut().for_each(CursorSnap::clear);
            self.drag = near(origin).or_else(|| {
                self.place_cursor(0, origin, radius, free);
                Some(1)
            });
        }
        if response.clicked_by(PointerButton::Primary) {
            let selected = near(time);
            self.snaps.iter_mut().for_each(CursorSnap::clear);
            if selected.is_none() {
                self.cursors[1] = None;
            }
            self.place_cursor(selected.unwrap_or(0), time, radius, free);
        } else if let Some(slot) = self.drag {
            let done = response.drag_stopped_by(PointerButton::Primary);
            self.place_cursor(slot, time, radius, free);
            if done {
                self.drag = None;
            }
        }
    }

    fn place_cursor(&mut self, slot: usize, time: f64, radius: f64, free: bool) {
        let detail = self
            .detail
            .as_ref()
            .and_then(|(_, data)| data.raw(self.active));
        let meta = &self.overview.channels[self.active];
        self.cursors[slot] =
            Some(self.snaps[slot].place(self.active, meta, time, radius, free, detail));
    }

    fn poll(&mut self, ctx: &Context) {
        if !self.index_started {
            self.index_started = true;
            if self.overview.sampled() || self.recording.power_recording() {
                let recording = self.recording.clone();
                self.index_job = Some(Task::start(
                    &JOBS,
                    Duration::from_secs(120),
                    move |cancel| recording.index(cancel),
                ));
            }
        }
        if let Some(result) = self.index_job.as_ref().and_then(Task::poll) {
            self.index_job = None;
            match result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                Ok(index) => self.index = Some(index),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some((cover, result)) = self
            .detail_job
            .as_ref()
            .and_then(|(cover, job)| job.poll().map(|result| (cover.clone(), result)))
        {
            self.detail_job = None;
            if let Ok(Ok(raw)) = result {
                self.detail = Some((cover, raw));
            }
        }
        if let Some(result) = self.analysis_job.as_ref().and_then(Task::poll) {
            self.analysis_job = None;
            if let Ok(Ok(data)) = result {
                self.measurements = Some(data);
                self.measurement_key = self.analysis_key.clone();
                self.spectrum_marker = None;
            }
        }
        self.math_state.poll();
        for (cursor, snap) in self.cursors.iter_mut().zip(&mut self.snaps) {
            if let Some(time) = snap.poll(self.active) {
                *cursor = Some(time);
            }
        }
        if self.pending() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn request_detail(&mut self) {
        if self.detail_job.is_some() {
            return;
        }
        let span = self.span.end - self.span.start;
        let raw = self
            .overview
            .channels
            .iter()
            .all(|channel| span / channel.interval_seconds <= reads::DETAIL_SAMPLES as f64);
        if !raw
            && (!self.recording.power_recording()
                || span * 3.0 >= self.domain.end - self.domain.start)
        {
            return;
        }
        if self.detail.as_ref().is_some_and(|(cover, detail)| {
            contains(cover, &self.span)
                && matches!(detail, PlotData::Raw(_)) == raw
                && cover.end - cover.start <= span * 4.0
        }) {
            return;
        }
        let cover = (self.span.start - span).max(self.domain.start)
            ..(self.span.end + span).min(self.domain.end);
        let recording = self.recording.clone();
        let load = cover.clone();
        self.detail_job = Some((
            cover,
            Task::start(&JOBS, Duration::from_secs(10), move |cancel| {
                if raw {
                    reads::detail(&recording, load, cancel).map(PlotData::Raw)
                } else {
                    recording
                        .power_envelopes(load, cancel)
                        .map(PlotData::Summary)
                }
            }),
        ));
    }

    fn request_analysis(&mut self, ctx: &Context) {
        let now = ctx.input(|input| input.time);
        let key = AnalysisKey {
            span: self.selection().unwrap_or_else(|| self.span.clone()),
            channel: self.active,
            fft: self.spectrum.then_some(self.spectrum_window),
            expression: self
                .math_state
                .numeric()
                .map_or_else(String::new, |_| self.math.clone()),
        };
        if self.analysis_key.as_ref() == Some(&key) {
            return;
        }
        if self.analysis_job.is_some() {
            return;
        }
        if now < self.analysis_dirty_at {
            ctx.request_repaint_after(Duration::from_secs_f64(self.analysis_dirty_at - now));
            return;
        }
        self.analysis_dirty_at = now + 0.1;
        self.spectrum_span = None;
        let request = AnalysisRequest {
            span: key.span.clone(),
            active: key.channel,
            spectrum: key.fft,
            math: self.math_state.numeric().cloned(),
        };
        self.analysis_key = Some(key);
        let recording = self.recording.clone();
        self.analysis_job = Some(Task::start(&JOBS, Duration::from_secs(15), move |cancel| {
            reads::analyze(&recording, request, cancel)
        }));
    }
}

fn contains(outer: &Range<f64>, inner: &Range<f64>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

fn copy_samples(
    recording: &Recording,
    channel: usize,
    span: Range<f64>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut rows = 0;
    super::preview_copy::copy_file(recording.source().path(), "samples", "csv", |file| {
        let mut output = std::io::BufWriter::new(file);
        rows = recording.export_channel(channel, span, cancel, &mut output)?;
        std::io::Write::flush(&mut output).map_err(|error| error.to_string())
    })?;
    Ok(format!("Copied {rows} samples"))
}

mod spectrum;
