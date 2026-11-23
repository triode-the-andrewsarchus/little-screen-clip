//! File-backed oscilloscope plots with one timeline shared by cursors and analysis.

mod channels;
mod comparison;
mod digital;
pub(super) use digital::ThresholdInput;
mod filters;
pub(super) use filters::{FilterSamples, FilteredChannel};
#[cfg(test)]
mod frame_benchmark;
mod hover;
mod lanes;
pub(super) use lanes::COLLAPSED_HEIGHT;
mod logic_export;
mod math;
pub(super) mod plot;
pub(super) mod reads;
mod session;
mod snap;
mod trace_fft;
pub(super) use session::{AnalysisEdits, Settings as SessionSettings};
use trace_fft::Trace as ScopeTrace;
pub(super) mod strip;

use super::signal_view::spectra as spectrum_input;
use super::{
    preview_copy::Copies,
    preview_job::Task,
    signal_view::timeline::{self, Interaction, Link},
};
use crate::file_source::ReadCheck;
use egui::{Color32, Context, PointerButton, Rect, Sense, Stroke, Ui, pos2};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::{Dimension, SignalType, SpectrumWindow},
        waveforms::{
            Envelope, Overview, Peak, Recording, display_axis_value, display_value, sample_span,
            time_span,
        },
    },
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

const MATH_COLOR: Color32 = Color32::from_rgb(215, 153, 246);

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MarkedRange {
    span: Range<f64>,
    peak_to_peak: Option<(f32, String)>,
}

#[derive(Clone, Copy)]
struct CursorDrag {
    slot: usize,
    origin: egui::Pos2,
}

struct PlottedChannel {
    index: usize,
    plot: Rect,
    bounds: Peak,
    display_offset: f32,
}

pub(super) struct Viewer {
    recording: Arc<Recording>,
    overview: Overview,
    domain: Range<f64>,
    span: Range<f64>,
    active: usize,
    cursors: [Option<f64>; 2],
    marks: Vec<MarkedRange>,
    stacked: bool,
    visible: Vec<bool>,
    display_offsets: Vec<f32>,
    trace_fft: std::collections::BTreeMap<ScopeTrace, trace_fft::TraceSpectrum>,
    hover: hover::Hover,
    digital: digital::Digital,
    drag: Option<CursorDrag>,
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
    filters: filters::Filters,
    spectra: spectrum_input::Spectra,
    spectrum: bool,
    spectrum_window: SpectrumWindow,
    spectrum_db: bool,
    spectrum_marker: Option<f64>,
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
    expression: String,
    math_span: Range<f64>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let recording = Recording::open(path, ReadCheck::default(), cancel)?;
        let overview = recording.overview(cancel)?;
        let domain = time_span(&overview.channels);
        let segmented = recording.triggered_segments();
        let mut visible = vec![!segmented; overview.channels.len()];
        visible[0] = true;
        let display_offsets = vec![0.0; visible.len()];
        let span = if segmented {
            time_span(&overview.channels[..1])
        } else {
            domain.clone()
        };
        let recording = Arc::new(recording);
        let digital = digital::Digital::new(&recording);
        Ok(Self {
            recording,
            overview,
            span,
            domain,
            active: 0,
            cursors: [None; 2],
            marks: Vec::new(),
            stacked: true,
            visible,
            display_offsets,
            trace_fft: Default::default(),
            hover: hover::Hover::default(),
            digital,
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
            filters: filters::Filters::default(),
            spectra: spectrum_input::Spectra::default(),
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

    pub(super) fn domain(&self) -> Range<f64> {
        self.domain.clone()
    }

    pub(super) fn span(&mut self) -> &mut Range<f64> {
        &mut self.span
    }

    pub(super) fn filtered_channels(&self) -> impl Iterator<Item = FilteredChannel<'_>> {
        self.filters.channels()
    }

    pub(super) fn recording(&self) -> &Arc<Recording> {
        &self.recording
    }

    pub(super) fn threshold_inputs(&self) -> Arc<ThresholdInput> {
        self.digital.input(self.recording.clone())
    }

    pub(super) fn measurement_channel(&self) -> usize {
        self.active
    }

    pub(super) fn cursor(&self) -> Option<f64> {
        self.cursors[0].or(self.cursors[1])
    }

    pub(super) fn min_height(&self) -> f32 {
        if self.spectrum { 410.0 } else { 205.0 }
    }

    fn reveal_recorded(&mut self, index: usize) {
        self.visible[index] = true;
        self.trace_fft.remove(&ScopeTrace::Recorded(index));
        self.spectra.only = false;
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        link: Option<&Link>,
        can_add_equation: bool,
    ) -> Interaction {
        if link.is_none() {
            return egui::ScrollArea::vertical()
                .id_salt("scope-content")
                .auto_shrink([false, false])
                .show(ui, |ui| self.capture_ui(ui, copies, None, can_add_equation))
                .inner;
        }
        self.capture_ui(ui, copies, link, can_add_equation)
    }

    fn capture_ui(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        link: Option<&Link>,
        can_add_equation: bool,
    ) -> Interaction {
        self.poll(ui.ctx());
        if let Some(channel) = link.and_then(|link| link.reveal_input) {
            self.reveal_recorded(channel);
        }
        let (spectrum_opened, derive_predicate) = ui
            .scope(|ui| {
                ui.spacing_mut().interact_size.y = 24.0;
                ui.spacing_mut().button_padding.y = 2.0;
                let spectrum_opened = self.toolbar(ui, copies);
                let derive_predicate = self.math_controls(ui, copies, can_add_equation);
                (spectrum_opened, derive_predicate)
            })
            .inner;
        self.power_totals(ui);
        let returned = super::signal_view::frequency_plot::origin_controls(
            ui,
            &mut self.spectra,
            &mut self.span,
            Link::clock(link),
        );
        let showing_spectrum = self.spectrum && self.spectra.origin().is_none();
        let show_time = !showing_spectrum || !self.spectra.only;
        let interaction = if show_time {
            let clock = Link::clock(link);
            let domain = link.map_or_else(|| self.domain.clone(), |link| link.domain.clone());
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 24.0;
                ui.spacing_mut().button_padding.y = 2.0;
                self.digital
                    .content(ui, &self.recording, &mut self.span, &domain, clock);
            });
            let available = ui.available_height();
            let content = (available - 92.0).max(100.0);
            let height = if showing_spectrum {
                // Leave a readable chart and spectrum controls beneath the time lanes.
                (content * 0.60).min(content - 220.0).max(68.0)
            } else {
                content
            };
            let probe = self.waveform(ui, height, link);
            self.overview_ui(ui);
            let share = ui
                .scope(|ui| {
                    ui.spacing_mut().interact_size.y = 24.0;
                    ui.spacing_mut().button_padding.y = 2.0;
                    self.cursor_readout(ui, link)
                })
                .inner;
            self.digital
                .table(ui, &mut self.span, &domain, probe, clock);
            Interaction {
                probe,
                share,
                derive_predicate,
            }
        } else {
            self.hover.time = None;
            self.digital.suspend();
            Interaction {
                derive_predicate,
                ..Interaction::default()
            }
        };
        if showing_spectrum {
            let filter_added = std::mem::take(&mut self.filters.reveal_spectrum);
            let view = ui.scope(|ui| self.spectrum_ui(ui, link, true));
            let near_edge = view.response.rect.top() > ui.clip_rect().bottom() - 166.0;
            if (spectrum_opened && near_edge) || filter_added || returned {
                view.response.scroll_to_me(Some(egui::Align::Min));
            }
        } else {
            self.spectra.suspend();
        }
        self.update_source(ui);
        interaction
    }

    fn update_source(&mut self, ui: &Ui) {
        self.request_detail();
        self.update_hover(ui.ctx());
        self.math_state
            .request(&self.recording, ui.ctx(), &self.span);
        for snap in &mut self.snaps {
            snap.request(&self.recording, ui.ctx());
        }
        self.request_analysis(ui.ctx());
        self.filters.update(ui, &self.recording, self.active);
        if self.pending() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, copies: &mut Copies) -> bool {
        let mut spectrum_opened = false;
        let mut decoder_row = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong(&self.overview.model);
            ui.weak(display_value(self.overview.duration_seconds(), "s"));
            ui.separator();
            if ui.button("Fit").clicked() {
                self.span = self.domain.clone();
            }
            if self.recording.triggered_segments() && ui.button("Fit segment").clicked() {
                self.fit_active();
            }
            if self.spectra.origin().is_none()
                && ui
                    .add(egui::Button::new("Spectrum").selected(self.spectrum))
                    .clicked()
            {
                self.spectrum = !self.spectrum;
                spectrum_opened = self.spectrum;
            }
            let same_units = self
                .overview
                .channels
                .windows(2)
                .all(|pair| pair[0].unit == pair[1].unit);
            if same_units
                && ui
                    .button(if self.stacked {
                        "Overlay traces"
                    } else {
                        "Stack traces"
                    })
                    .clicked()
            {
                self.stacked = !self.stacked;
            }
            self.copy_selection(ui, copies);
            self.digital.toggle_lanes(ui);
            decoder_row = ui.available_size_before_wrap().x < 390.0;
            if !decoder_row {
                self.digital.decoder_buttons(ui);
            }
        });
        if decoder_row {
            self.digital.decoder_buttons(ui);
        }
        self.channel_controls(ui);
        if self.filters.any() {
            ui.horizontal_wrapped(|ui| self.filters.visibility_controls(ui));
        }
        spectrum_opened
    }

    fn copy_selection(&mut self, ui: &mut Ui, copies: &mut Copies) {
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
    }

    fn math_controls(&mut self, ui: &mut Ui, copies: &mut Copies, can_add_equation: bool) -> bool {
        let promote = ui
            .horizontal_wrapped(|ui| {
                let channel = &self.overview.channels[self.active];
                if Dimension::from_unit(&channel.unit).is_some() {
                    ui.label("Math");
                    let hint: String = match channel.unit.as_str() {
                        "V" if !self.recording.power_recording() => "> 1V or CH1 - CH2".into(),
                        "FS" => "abs(x) > 0.1 FS".into(),
                        "" => "abs(x) > 0.1".into(),
                        unit => format!("x > 1{unit}"),
                    };
                    ui.add(
                        egui::TextEdit::singleline(&mut self.math)
                            .id_salt("scope_math_expression")
                            .hint_text(hint)
                            .char_limit(256)
                            .desired_width((ui.available_width() - 10.0).clamp(140.0, 320.0)),
                    );
                }
                let input = if self.recording.power_recording() && channel.unit != "V" {
                    ""
                } else {
                    &self.math
                };
                self.math_state
                    .compile(input, self.active, &self.overview.channels);
                let promote = self.predicate_actions(ui, copies, can_add_equation);
                if let Some(status) =
                    self.math_state
                        .status(channel, &self.span, self.math_envelope())
                {
                    ui.weak(status);
                }
                promote
            })
            .inner;
        if let Some(message) = self.math_state.error().or(self.error.as_deref()) {
            ui.colored_label(ui.visuals().error_fg_color, message);
        } else if self.index_job.is_some() {
            ui.weak("Refining overview…");
        }
        promote
    }

    pub(super) fn explicit_predicate(&self, names: &[String]) -> Result<String, String> {
        self.math_state
            .explicit_predicate(&self.overview.channels, names)
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

    fn waveform(&mut self, ui: &mut Ui, height: f32, link: Option<&Link>) -> Option<f64> {
        let lane_height = if self.trace_fft.is_empty() {
            lanes::MIN_HEIGHT
        } else {
            95.0
        };
        let height = height.max(lane_height * self.lane_count().max(1) as f32 + 20.0);
        let (outer, frame) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
        let rect = Rect::from_min_max(
            outer.min + egui::vec2(78.0, 4.0),
            outer.max - egui::vec2(9.0, 16.0),
        );
        let response = ui.interact(rect, frame.id.with("waveform"), Sense::click_and_drag());
        let fft_lanes = self
            .fft_slots()
            .map(|slot| self.lane(rect, slot).frame)
            .collect::<Vec<_>>();
        let in_fft_lane = |point| fft_lanes.iter().any(|lane| lane.contains(point));
        let hover_in_fft = ui.input(|input| input.pointer.hover_pos().is_some_and(in_fft_lane));
        if !hover_in_fft
            && let Some(span) = timeline::navigate(ui.ctx(), rect, &response, &self.span, true)
        {
            let domain = link.map_or(&self.domain, |link| &link.domain);
            self.span = timeline::fit(
                domain,
                span,
                self.overview.channels[self.active].interval_seconds,
            );
        }
        let origin_in_fft = ui.input(|input| input.pointer.press_origin().is_some_and(in_fft_lane));
        if !origin_in_fft && (!hover_in_fft || self.drag.is_some()) {
            self.move_cursors(ui, rect, &response);
        }
        let painter = ui.painter_at(outer);
        let lane_count = self.lane_count();
        let lanes = (0..lane_count)
            .map(|slot| lanes::Lane::new(rect, slot, lane_count))
            .filter(|lane| painter.clip_rect().intersects(lane.frame));
        for lane in lanes.clone() {
            lane.background(ui, &painter);
        }
        let ink = ui.visuals().weak_text_color();
        let axis = Link::clock(link).workspace_range(&self.span);
        plot::x_axis_with_grid(
            &painter,
            rect,
            &axis,
            "s",
            ink,
            lanes
                .filter(|lane| !in_fft_lane(lane.plot.center()))
                .map(|lane| lane.plot.y_range()),
        );
        let clipped = painter.with_clip_rect(rect);
        let probe = response
            .hover_pos()
            .filter(|point| rect.contains(*point) && !in_fft_lane(*point))
            .map(|point| plot::at(rect, &self.span, point.x));
        self.hover.time = probe
            .or(link.and_then(|link| link.probe))
            .or_else(|| self.digital.cursor());
        let (channels, fft_change) = self.draw_channels(ui, &painter, rect, ink, link);
        self.digital
            .highlight_analog_inputs(&clipped, &self.span, &channels, self.stacked);
        self.draw_math(ui, &painter, &clipped, rect);
        self.draw_filters(ui, &painter, &clipped, rect);
        let clock = Link::clock(link);
        if fft_lanes.is_empty() {
            self.paint_time_markers(ui, &clipped, rect, link, true);
            self.paint_hover(
                &clipped,
                rect,
                ui.visuals().text_color(),
                ui.visuals().window_fill(),
                clock,
                &channels,
            );
        } else {
            let mut labelled = false;
            for lane in (0..self.lane_count()).map(|slot| self.lane(rect, slot)) {
                if in_fft_lane(lane.plot.center()) || !ui.is_rect_visible(lane.frame) {
                    continue;
                }
                let painter = clipped.with_clip_rect(lane.frame.intersect(clipped.clip_rect()));
                let time_rect = Rect::from_min_max(
                    pos2(rect.left(), lane.frame.top()),
                    lane.frame.right_bottom(),
                );
                self.paint_time_markers(ui, &painter, time_rect, link, !labelled);
                labelled = true;
                self.paint_hover(
                    &painter,
                    rect,
                    ui.visuals().text_color(),
                    ui.visuals().window_fill(),
                    clock,
                    &channels,
                );
            }
        }
        if let Some(index) = fft_change {
            self.toggle_trace_fft(ScopeTrace::Recorded(index));
            ui.ctx().request_repaint();
        }
        probe
    }

    fn paint_time_markers(
        &self,
        ui: &Ui,
        clipped: &egui::Painter,
        rect: Rect,
        link: Option<&Link>,
        show_labels: bool,
    ) {
        let clock = Link::clock(link);
        self.paint_marks(ui, clipped, rect, clock.scale(), show_labels);
        super::signal_view::frequency_plot::paint_origin(
            ui,
            clipped,
            rect,
            &self.span,
            &self.spectra,
        );
        if let Some(link) = link {
            link.paint_markers(clipped, rect, &self.span);
        }
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
                if show_labels {
                    clipped.text(
                        pos2(x + 4.0, rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                    );
                }
            }
        }
    }

    fn draw_channels(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        rect: Rect,
        ink: Color32,
        link: Option<&Link>,
    ) -> (Vec<PlottedChannel>, Option<usize>) {
        let overlay = self.overlay_scale();
        let order = self.channel_slots().collect::<Vec<_>>();
        let mut plotted = Vec::with_capacity(order.len());
        let mut change = None;
        let mut overlay_axis = false;
        for (slot, index) in order {
            let lane = self.lane(rect, slot);
            Link::reveal_trace(link, ui, lane.frame, index);
            if !ui.is_rect_visible(lane.frame) {
                if let Some(spectrum) = self.trace_fft.get_mut(&ScopeTrace::Recorded(index)) {
                    spectrum.suspend();
                }
                continue;
            }
            let color = plot::color(index);
            let trace = ScopeTrace::Recorded(index);
            let fft = self.trace_fft.contains_key(&trace);
            let raw = (self.stacked && !fft).then(|| self.channel_bounds(index));
            if fft || self.stacked {
                change = self.recorded_lane_controls(ui, lane, index, raw).or(change);
            }
            if fft {
                self.paint_trace_fft(ui, lane.plot, trace);
                continue;
            }
            let channel = &self.overview.channels[index];
            let offset = self.display_offsets[index];
            let display =
                overlay.unwrap_or_else(|| lanes::stacked_scale(raw.unwrap_or_default(), offset));
            if self.stacked || !overlay_axis {
                if overlay.is_some() {
                    lane.label(painter, "Shared scale", ink);
                }
                lane.scale(
                    painter,
                    display,
                    &channel.unit,
                    if self.stacked { color } else { ink },
                );
                overlay_axis = true;
            }
            let bounds = lanes::shifted(display, -offset);
            self.paint_channel(painter, index, lane.plot, bounds, color);
            Link::paint_trace_evidence(link, painter, lane.plot, &self.span, index);
            plotted.push(PlottedChannel {
                index,
                plot: lane.plot,
                bounds,
                display_offset: offset,
            });
        }
        (plotted, change)
    }

    fn recorded_lane_controls(
        &mut self,
        ui: &mut Ui,
        lane: lanes::Lane,
        index: usize,
        raw: Option<Peak>,
    ) -> Option<usize> {
        let mut heading = lane.heading(ui, ("scope-channel", index));
        heading.colored_label(plot::color(index), &self.overview.channels[index].name);
        let toggle = self.trace_fft_toggle(&mut heading, ScopeTrace::Recorded(index));
        if let Some(bounds) = raw {
            self.offset_control(&mut heading, index, bounds);
        }
        toggle.then_some(index)
    }

    fn overlay_scale(&self) -> Option<Peak> {
        (!self.stacked).then(|| {
            let mut bounds = Peak::default();
            for index in self
                .visible_channels()
                .filter(|index| !self.trace_fft.contains_key(&ScopeTrace::Recorded(*index)))
            {
                let original = self.channel_bounds(index);
                bounds.merge(original);
                bounds.merge(lanes::shifted(original, self.display_offsets[index]));
            }
            bounds
        })
    }

    fn channel_bounds(&self, index: usize) -> Peak {
        self.index.as_ref().unwrap_or(&self.overview.envelopes)[index].extrema()
    }

    fn paint_channel(
        &self,
        painter: &egui::Painter,
        index: usize,
        plot: Rect,
        bounds: Peak,
        color: Color32,
    ) {
        if !bounds.valid() {
            return;
        }
        let clipped = painter.with_clip_rect(plot.intersect(painter.clip_rect()));
        let visible = self.span.start.max(self.domain.start)..self.span.end.min(self.domain.end);
        let detail = self
            .detail
            .as_ref()
            .filter(|(cover, _)| contains(cover, &visible))
            .map(|(_, detail)| detail);
        if let Some(raw) = detail.and_then(|detail| detail.raw(index)) {
            let (start, samples) = raw.visible(&self.span);
            plot::draw_raw(
                &clipped, plot, &self.span, start, raw.step, samples, bounds, color,
            );
        } else {
            let envelopes = match detail {
                Some(PlotData::Summary(envelopes)) => envelopes,
                _ => self.index.as_ref().unwrap_or(&self.overview.envelopes),
            };
            plot::draw_envelope(
                &clipped,
                plot,
                &self.span,
                &self.overview.channels[index],
                &envelopes[index],
                bounds,
                color,
            );
        }
    }

    fn fft_slots(&self) -> impl Iterator<Item = usize> + '_ {
        let math = self.math_state.numeric().is_some();
        let physical = self.physical_lanes();
        self.channel_slots()
            .filter(|&(_, index)| self.trace_fft.contains_key(&ScopeTrace::Recorded(index)))
            .map(|(slot, _)| slot)
            .chain((math && self.trace_fft.contains_key(&ScopeTrace::Math)).then_some(physical))
            .chain(
                self.filters
                    .visible_indices()
                    .enumerate()
                    .filter_map(move |(slot, index)| {
                        self.trace_fft
                            .contains_key(&ScopeTrace::Filtered(index))
                            .then_some(physical + usize::from(math) + slot)
                    }),
            )
    }

    fn channel_slots(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let mut next_fft = usize::from(
            self.visible_channels()
                .any(|index| !self.trace_fft.contains_key(&ScopeTrace::Recorded(index))),
        );
        self.visible_channels()
            .enumerate()
            .map(move |(slot, index)| {
                if self.stacked {
                    return (slot, index);
                }
                if !self.trace_fft.contains_key(&ScopeTrace::Recorded(index)) {
                    return (0, index);
                }
                // Overlay keeps all time traces on the first lane. FFTs stay independent.
                let slot = next_fft;
                next_fft += 1;
                (slot, index)
            })
    }

    fn visible_channels(&self) -> impl Iterator<Item = usize> + '_ {
        self.visible
            .iter()
            .enumerate()
            .filter_map(|(index, &show)| show.then_some(index))
    }

    fn physical_lanes(&self) -> usize {
        if self.stacked {
            self.visible_channels().count()
        } else {
            let total = self.visible_channels().count();
            let spectra = self
                .visible_channels()
                .filter(|index| self.trace_fft.contains_key(&ScopeTrace::Recorded(*index)))
                .count();
            spectra + usize::from(total > spectra)
        }
    }

    fn lane_count(&self) -> usize {
        self.physical_lanes()
            + usize::from(self.math_state.numeric().is_some())
            + self.filters.len()
    }

    fn calculated(&self) -> Option<&(Arc<RawChannel>, Peak)> {
        let key = self.measurement_key.as_ref()?;
        if key.expression != self.math || key.channel != self.active {
            return None;
        }
        self.measurements.as_ref()?.math.as_ref()
    }

    fn math_unit(&self) -> String {
        match self
            .math_state
            .numeric()
            .map(|expression| expression.signal_type())
        {
            Some(SignalType::Analog(dimension)) => dimension.to_string(),
            _ => String::new(),
        }
    }

    fn draw_math(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        clip: &egui::Painter,
        rect: Rect,
    ) {
        if let Some((slot, _)) = self
            .channel_slots()
            .find(|&(_, index)| index == self.active)
            && !self
                .trace_fft
                .contains_key(&ScopeTrace::Recorded(self.active))
        {
            let lane = self.lane(rect, slot);
            self.math_state.paint(
                clip,
                lane.plot,
                &self.span,
                &self.overview.channels[self.active],
                self.math_envelope(),
            );
        }
        if self.math_state.numeric().is_none() {
            return;
        }
        let lane = self.lane(rect, self.physical_lanes());
        if !painter.clip_rect().intersects(lane.frame) {
            if let Some(state) = self.trace_fft.get_mut(&ScopeTrace::Math) {
                state.suspend();
            }
            return;
        }
        let calculated = self.calculated();
        let status = match calculated {
            Some((raw, _))
                if raw.start > self.span.start + 2.0 * raw.step
                    || raw.time(raw.samples.len()) < self.span.end - 2.0 * raw.step =>
            {
                " · bounded sample window"
            }
            Some(_) => "",
            None => " · calculating…",
        };
        let mut heading = lane.heading(ui, "scope-math-lane");
        let toggle = self.trace_fft_toggle(&mut heading, ScopeTrace::Math);
        heading.add(
            egui::Label::new(
                egui::RichText::new(format!("{}{status}", self.math)).color(MATH_COLOR),
            )
            .truncate(),
        );
        if self.trace_fft.contains_key(&ScopeTrace::Math) {
            self.paint_trace_fft(ui, lane.plot, ScopeTrace::Math);
        } else if let Some((raw, bounds)) = calculated.filter(|(_, bounds)| bounds.valid()) {
            lane.scale(painter, *bounds, &self.math_unit(), MATH_COLOR);
            let (start, samples) = raw.visible(&self.span);
            let stride = samples
                .len()
                .div_ceil((lane.plot.width() as usize).max(8))
                .max(1);
            let display = samples.iter().copied().step_by(stride).collect::<Vec<_>>();
            plot::draw_raw(
                &clip.with_clip_rect(lane.plot),
                lane.plot,
                &self.span,
                start,
                raw.step * stride as f64,
                &display,
                *bounds,
                MATH_COLOR,
            );
        }
        if toggle {
            self.toggle_trace_fft(ScopeTrace::Math);
            ui.ctx().request_repaint();
        }
    }

    fn paint_marks(
        &self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        time_scale: f64,
        show_labels: bool,
    ) {
        let active = self.active_mark();
        let ink = ui.visuals().text_color();
        let fill = ui.visuals().window_fill();
        for (index, mark) in self.marks.iter().chain(active.iter()).enumerate() {
            if mark.span.end <= self.span.start || mark.span.start >= self.span.end {
                continue;
            }
            let color = plot::color(index).gamma_multiply(0.25);
            plot::highlight(painter, rect, &self.span, mark.span.clone(), color);
            if !show_labels {
                continue;
            }
            let length = (mark.span.end - mark.span.start) * time_scale;
            let mut label = format!(
                "{}  Δt {}  1/Δt {}",
                index + 1,
                display_value(length, "s"),
                display_value(length.recip(), "Hz")
            );
            if let Some((peak, unit)) = &mark.peak_to_peak {
                label.push_str(&format!(
                    "  Pk–Pk {}",
                    display_value(f64::from(*peak), unit)
                ));
            }
            let x = plot::x_at(rect, &self.span, mark.span.start).max(rect.left());
            let y = (rect.bottom() - 21.0 - (index % 3) as f32 * 18.0).max(rect.top());
            hover::selection_label(painter, rect, x, y, &label, ink, fill);
        }
    }

    fn active_mark(&self) -> Option<MarkedRange> {
        let span = self.selection()?;
        let peak_to_peak = self
            .measurements_for(&span)
            .filter(|data| {
                data.extrema.valid()
                    && !data.limited
                    && data.valid_samples == data.raw.samples.len()
            })
            .map(|data| {
                (
                    data.extrema.max - data.extrema.min,
                    self.overview.channels[self.active].unit.clone(),
                )
            });
        Some(MarkedRange { span, peak_to_peak })
    }

    fn archive_mark(&mut self) {
        if let Some(mark) = self.active_mark() {
            self.marks.push(mark);
        }
        self.cursors = [None; 2];
        self.snaps.iter_mut().for_each(CursorSnap::clear);
    }

    fn overview_ui(&mut self, ui: &mut Ui) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 26.0),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let segmented = self.recording.triggered_segments();
        if segmented {
            self.paint_segment_rail(&painter, rect);
        } else {
            let channel = &self.overview.channels[self.active];
            let envelope = &self.index.as_ref().unwrap_or(&self.overview.envelopes)[self.active];
            plot::draw_envelope(
                &painter,
                rect.shrink(2.0),
                &self.domain,
                channel,
                envelope,
                envelope.extrema(),
                plot::color(self.active),
            );
        }
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
            if segmented && response.clicked() {
                self.open_segment_near(center);
            } else {
                let half = (self.span.end - self.span.start) / 2.0;
                self.span = self.clamp_span(center - half..center + half);
            }
        }
        response.on_hover_text(if segmented {
            "Click a trigger segment to open it. Drag to pan across the capture."
        } else {
            "Navigate the full capture"
        });
    }

    fn measurements_for(&self, range: &Range<f64>) -> Option<&Measurements> {
        let key = self.measurement_key.as_ref()?;
        (key.channel == self.active && key.span == *range).then_some(self.measurements.as_ref()?)
    }

    fn cursor_readout(&mut self, ui: &mut Ui, link: Option<&Link>) -> Option<Range<f64>> {
        let clock = Link::clock(link);
        let mut center = None;
        let channel = &self.overview.channels[self.active];
        let selection = self.selection();
        ui.horizontal_wrapped(|ui| {
            if self.cursors.iter().any(Option::is_some) {
                ui.colored_label(plot::color(self.active), &channel.name);
            }
            for (label, time) in ["A", "B"].into_iter().zip(self.cursors) {
                if let Some(time) = time {
                    let (row, probe) = self.recorded_probe(self.active, time);
                    let position = display_axis_value(
                        clock.to_workspace(time),
                        channel.interval_seconds * clock.scale(),
                        "s",
                    );
                    let sample =
                        row.map_or_else(String::new, |row| format!("sample {} · ", row + 1));
                    ui.horizontal(|ui| {
                        if ui.button(format!("Center {label}")).clicked() {
                            center = Some(time);
                        }
                        ui.label(format!(
                            "{position} · {sample}{}",
                            probe.text(&channel.unit)
                        ));
                    });
                }
            }
            if selection.is_some() {
                ui.weak("Shift bypasses snap · Right-click to remove");
            } else {
                ui.weak("Scroll to zoom · Middle-drag to pan · Drag to measure");
            }
        });
        let share = ui
            .horizontal_wrapped(|ui| {
                let ready = selection
                    .as_ref()
                    .filter(|_| self.snaps.iter().all(|snap| !snap.pending()));
                let share = Link::share_button(link, ui, ready);
                if let Some(range) = &selection {
                    let width = (range.end - range.start) * clock.scale();
                    ui.label(format!(
                        "Δt  {}    1/Δt  {}",
                        display_value(width, "s"),
                        display_value(width.recip(), "Hz")
                    ));
                    let rows = sample_span(channel, range.clone());
                    ui.weak(format!("{} samples in range", rows.end - rows.start));
                }
                if let Some(measured) =
                    self.measurements_for(selection.as_ref().unwrap_or(&self.span))
                {
                    let missing = measured.raw.samples.len() - measured.valid_samples;
                    if missing > 0 {
                        ui.weak(format!(
                            "{missing} missing samples{}",
                            if measured.limited { " in window" } else { "" }
                        ));
                    }
                    if measured.extrema.valid() {
                        let reading = display_value(
                            f64::from(measured.extrema.max - measured.extrema.min),
                            &channel.unit,
                        );
                        if missing > 0 || measured.limited {
                            ui.weak(format!("Observed Pk–Pk  {reading}"));
                        } else {
                            ui.weak(format!("Pk–Pk  {reading}"));
                        }
                    }
                    if measured.limited {
                        ui.weak("Waveform measurements use a 65k-sample window");
                    }
                }
                share
            })
            .inner;
        if let Some(time) = center {
            let half = (self.span.end - self.span.start) / 2.0;
            let domain = link.map_or(&self.domain, |link| &link.domain);
            let step = self.overview.channels[self.active].interval_seconds;
            self.span = timeline::fit(domain, time - half..time + half, step);
        }
        share
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
            || self.filters.pending()
            || self.spectra.busy()
            || self.hover.pending()
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
        timeline::fit(
            &self.domain,
            requested,
            self.overview.channels[self.active].interval_seconds,
        )
    }

    fn move_cursors(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        let active = response.dragged_by(PointerButton::Primary);
        let stopped = response.drag_stopped_by(PointerButton::Primary);
        let (down, origin, latest, free) = ui.input(|input| {
            (
                input.pointer.button_down(PointerButton::Primary),
                input.pointer.press_origin(),
                input.pointer.latest_pos(),
                input.modifiers.shift,
            )
        });
        let point = response
            .interact_pointer_pos()
            .or_else(|| stopped.then_some(latest).flatten());
        let Some(point) = point else {
            if !down {
                self.drag = None;
            }
            return;
        };
        let time = plot::at(rect, &self.span, point.x);
        let radius = (self.span.end - self.span.start) * 10.0 / f64::from(rect.width().max(1.0));
        if response.secondary_clicked() && rect.contains(point) {
            self.remove_mark(time);
            return;
        }
        let cursors = self.cursors;
        let near = |t: f64| {
            let [a, b] = cursors.map(|time| time.map_or(f64::INFINITY, |time| (time - t).abs()));
            (a.min(b) <= radius).then_some(usize::from(b < a))
        };
        if active
            && let Some(origin) = origin
            && self.drag.is_none_or(|drag| drag.origin != origin)
            && rect.contains(origin)
        {
            let time = plot::at(rect, &self.span, origin.x);
            self.snaps.iter_mut().for_each(CursorSnap::clear);
            let slot = near(time).unwrap_or_else(|| {
                self.archive_mark();
                self.place_cursor(0, time, radius, free);
                1
            });
            self.drag = Some(CursorDrag { slot, origin });
        }
        if response.clicked_by(PointerButton::Primary) {
            self.select_at(time, radius, free, near(time));
        } else if (active || stopped)
            && let Some(drag) = self.drag
        {
            self.place_cursor(drag.slot, time, radius, free);
        }
        if !down {
            self.drag = None;
        }
    }

    fn remove_mark(&mut self, time: f64) {
        if self
            .selection()
            .is_some_and(|range| time >= range.start && time <= range.end)
        {
            self.cursors = [None; 2];
            self.snaps.iter_mut().for_each(CursorSnap::clear);
        } else if let Some(index) = self.mark_at(time) {
            self.marks.remove(index);
        }
    }

    fn mark_at(&self, time: f64) -> Option<usize> {
        self.marks
            .iter()
            .rposition(|mark| time >= mark.span.start && time <= mark.span.end)
    }

    fn select_at(&mut self, time: f64, radius: f64, free: bool, cursor: Option<usize>) {
        self.snaps.iter_mut().for_each(CursorSnap::clear);
        if cursor.is_none() {
            let mark = self.mark_at(time).map(|index| self.marks.remove(index));
            self.archive_mark();
            if let Some(mark) = mark {
                self.cursors = [Some(mark.span.start), Some(mark.span.end)];
                return;
            }
        }
        self.place_cursor(cursor.unwrap_or(0), time, radius, free);
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
        if self.detail_job.is_some()
            || self.span.end <= self.domain.start
            || self.span.start >= self.domain.end
        {
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
        let visible = self.span.start.max(self.domain.start)..self.span.end.min(self.domain.end);
        if self.detail.as_ref().is_some_and(|(cover, detail)| {
            contains(cover, &visible)
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
            math_span: if self.math_state.numeric().is_some() {
                self.span.clone()
            } else {
                0.0..0.0
            },
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
        let request = AnalysisRequest {
            span: key.span.clone(),
            active: key.channel,
            math: self
                .math_state
                .numeric()
                .cloned()
                .map(|expression| (expression, key.math_span.clone())),
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
