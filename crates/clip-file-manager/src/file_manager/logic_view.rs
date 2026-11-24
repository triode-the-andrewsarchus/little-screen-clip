//! Offline logic timing and transition inspection.

mod comparison;
mod frequency;
pub(super) mod plot;
pub(super) mod protocols;
pub(in crate::file_manager) mod sigrok;
pub(super) mod strip;

use super::{
    preview_copy::Copies,
    preview_job::Task,
    scope_view::plot as timing,
    signal_view::{
        lane_range::{self, Draft, Drag, Marks, Scope},
        timeline::{self, Interaction, Link},
    },
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
use protocols::wiring::{Pin, Role, Wiring};
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
    comparison_hidden: Vec<usize>,
    comparison_lanes: Vec<usize>,
    channel_picker: Wiring,
    selected: Option<Range<f64>>,
    ranges: Marks<usize>,
    cursor: Option<f64>,
    frame: Option<Frame>,
    request: Option<WindowKey>,
    read_job: Option<Task<Result<Frame, String>>>,
    refinement: Option<Task<Result<(bool, Option<Overview>), String>>>,
    refined: bool,
    followed_row: Option<usize>,
    request_after: f64,
    show_transitions: bool,
    evidence_lane: Option<usize>,
    protocols: protocols::Panel,
    sigrok: sigrok::Panel,
    frequency: frequency::Frequency,
    error: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct SessionSettings {
    active: usize,
    selected: Option<Range<f64>>,
    #[serde(default, skip_serializing_if = "Marks::empty")]
    ranges: Marks<usize>,
    cursor: Option<f64>,
    show_transitions: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    comparison_hidden: Vec<usize>,
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
    pub(in crate::file_manager) fn analysis_edits(&self) -> protocols::SessionSettings {
        self.protocols.session_settings().analysis_edits()
    }

    pub(super) fn restoring_details(&self) -> bool {
        self.show_transitions && self.protocols.decoding()
    }

    pub(in crate::file_manager) fn session_settings(&self) -> SessionSettings {
        SessionSettings {
            active: self.active,
            selected: self.selected.clone(),
            ranges: self.ranges.clone(),
            cursor: self.cursor,
            show_transitions: self.show_transitions,
            comparison_hidden: self.comparison_hidden.clone(),
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
        self.channel_picker.close();
        self.selected = state.selected.filter(|range| !range.is_empty());
        self.ranges = state.ranges;
        self.cursor = state.cursor;
        self.show_transitions = state.show_transitions;
        self.comparison_hidden = state
            .comparison_hidden
            .into_iter()
            .filter(|&index| index < count)
            .collect();
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
            comparison_hidden: Vec::new(),
            comparison_lanes: Vec::new(),
            channel_picker: Wiring::default(),
            selected: None,
            ranges: Marks::default(),
            cursor: None,
            frame: None,
            request: None,
            read_job: None,
            refinement: None,
            refined: false,
            followed_row: None,
            request_after: 0.0,
            show_transitions: false,
            evidence_lane: None,
            protocols,
            sigrok: sigrok::Panel::default(),
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
            self.ranges.active = Scope::new(vec![self.active], None);
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

    pub(super) fn measurement_channel(&self) -> (usize, Option<bool>, bool) {
        let (lane, polarity) = self
            .protocols
            .active_pwm()
            .filter(|(lane, _)| *lane < self.overview.lanes.len())
            .map_or_else(
                || (self.focused_lane(), None),
                |(lane, active_high)| (lane, Some(active_high)),
            );
        (lane, polarity, self.refined)
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
    ) -> Interaction {
        if link.is_none() {
            return egui::ScrollArea::vertical()
                .id_salt("logic-content")
                .auto_shrink([false, false])
                .show(ui, |ui| self.capture_ui(ui, copies, None))
                .inner;
        }
        self.capture_ui(ui, copies, link)
    }

    fn capture_ui(&mut self, ui: &mut Ui, copies: &mut Copies, link: Option<&Link>) -> Interaction {
        self.poll(ui.ctx());
        self.refine(ui.ctx(), link.is_none());
        self.evidence_lane = link.and_then(|link| link.visiting_input);
        if link.is_some_and(|link| link.reveal_input.is_some()) {
            self.frequency.visible = false;
        }
        let share = self.toolbar(ui, copies, link, true);
        self.channel_choices(ui);
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
            self.protocols.cancel_wiring();
            self.protocols.exit_inspection(&mut self.span);
            self.protocols.close();
            let origin = self.sigrok.close();
            self.restore_sigrok_view(origin);
            self.frequency(ui, link);
            return Interaction {
                share,
                ..Interaction::default()
            };
        }
        self.decoder_controls(ui);
        let reserved = if self.show_transitions { 238.0 } else { 39.0 };
        let minimum = (44.0 + 24.0 * self.visible_lanes().len() as f32).max(75.0);
        let height = (ui.available_height() - reserved).max(minimum);
        let probe = self.waveforms(ui, height, link);
        self.overview(ui);
        self.request_window(ui.ctx());
        self.decode_events(ui, link);
        Interaction {
            probe,
            share,
            ..Interaction::default()
        }
    }

    fn decode_events(&mut self, ui: &mut Ui, link: Option<&Link>) {
        if self.show_transitions && self.sigrok.active {
            if let Some((time, step)) = self.sigrok.table(ui, Link::clock(link)) {
                let origin = sigrok::View {
                    span: self.span.clone(),
                    selection: self.selected.clone(),
                    selection_scope: self.ranges.active.clone(),
                    cursor: self.cursor,
                };
                let domain = link.map_or(&self.overview.span, |link| &link.domain);
                self.span = self.sigrok.visit(origin, &time, domain, step);
                self.cursor = Some(time.start);
                self.selected = (!time.is_empty()).then_some(time);
                self.ranges.active = Scope::new(self.sigrok.channels().collect(), None);
                ui.ctx().request_repaint();
            }
        } else if self.show_transitions && !self.protocols.edges() {
            self.protocols
                .update(ui.ctx(), self.span.clone(), self.refined, &self.recording);
            if let Some(action) = self.protocols.table(
                ui,
                &self.span,
                self.cursor,
                self.selected.as_ref(),
                Link::clock(link),
            ) {
                self.follow_event(ui, link, action);
            }
        } else {
            self.protocols.close();
            if self.show_transitions {
                self.transitions(ui);
            }
        }
    }

    fn decoder_controls(&mut self, ui: &mut Ui) {
        let mut sigrok = false;
        if self
            .protocols
            .decoder_buttons_with(ui, &self.overview.lanes, |ui| {
                if ui.button("Sigrok catalog…").clicked() {
                    sigrok = true;
                    ui.close();
                }
            })
        {
            let origin = self.sigrok.close();
            self.restore_sigrok_view(origin);
            self.channel_picker.close();
            self.show_transitions = true;
        }
        if sigrok {
            self.protocols.cancel_wiring();
            self.protocols.exit_inspection(&mut self.span);
            self.protocols.close();
            self.sigrok.open(ui.ctx());
            self.channel_picker.close();
            self.show_transitions = true;
        }
        if !self.show_transitions {
            self.protocols.cancel_wiring();
            self.protocols.exit_inspection(&mut self.span);
            let origin = self.sigrok.close();
            self.restore_sigrok_view(origin);
        } else if self.sigrok.active {
            let origin = self.sigrok.return_controls(ui);
            self.restore_sigrok_view(origin);
            self.sigrok.controls(
                ui,
                &self.overview.lanes,
                || protocols::Source::Recording(self.recording.clone()),
                &self.span,
                self.selected.as_ref(),
            );
        } else {
            self.protocols.controls(ui, &self.overview.lanes);
            self.protocols.return_controls(ui, &mut self.span);
        }
    }

    fn restore_sigrok_view(&mut self, origin: Option<sigrok::View>) {
        if let Some(origin) = origin {
            self.span = origin.span;
            self.selected = origin.selection;
            self.ranges.active = origin.selection_scope;
            self.cursor = origin.cursor;
        }
    }

    fn toolbar(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        link: Option<&Link>,
        standalone: bool,
    ) -> Option<Range<f64>> {
        let clock = Link::clock(link);
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.overview.label);
            ui.weak(format!("{} channels", self.overview.lanes.len()));
            ui.weak(display_value(
                self.overview.span.end - self.overview.span.start,
                "s",
            ));
            let showing_waves =
                !self.frequency.visible || self.frequency.spectra.origin().is_some();
            if standalone && (!showing_waves || self.overview.lanes.len() > LANES_PER_PAGE) {
                self.channel_picker.button(
                    ui,
                    &Pin::required(Role::Channel, &mut self.active),
                    &self.overview.lanes,
                );
            }
            if standalone && ui.button("Fit").clicked() {
                self.span = self.overview.span.clone();
            }
            if standalone
                && self.frequency.spectra.origin().is_none()
                && ui
                    .add(egui::Button::new("Spectrum").selected(self.frequency.visible))
                    .clicked()
            {
                self.frequency.visible = !self.frequency.visible;
            }
            if ui
                .add(egui::Button::new("Transitions").selected(self.show_transitions))
                .clicked()
            {
                self.show_transitions = !self.show_transitions;
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
                let index = self.active;
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
            Link::share_button(link, ui, self.selected.as_ref())
        })
        .inner
    }

    fn channel_choices(&mut self, ui: &mut Ui) {
        let previous = self.active;
        self.channel_picker.choices(
            ui,
            &mut [Pin::required(Role::Channel, &mut self.active)],
            &self.overview.lanes,
        );
        if previous != self.active {
            self.frequency.spectra.clear_result();
        }
    }

    fn pick_plotted_lane(&mut self, lane: usize) {
        if self.channel_picker.active() {
            self.channel_picker
                .choose(&mut [Pin::required(Role::Channel, &mut self.active)], lane);
            self.frequency.spectra.clear_result();
        } else if self.protocols.wiring_active() {
            self.protocols.wire_lane(lane);
        } else if lane != self.active {
            self.active = lane;
            self.followed_row = None;
            self.frequency.spectra.clear_result();
        }
    }

    fn visible_lanes(&self) -> Vec<usize> {
        let configured = self
            .show_transitions
            .then(|| self.protocols.channels())
            .flatten();
        let mut visible = if self.sigrok.active {
            let mut channels: Vec<_> = self.sigrok.channels().collect();
            channels.sort_unstable();
            channels.dedup();
            channels
        } else {
            configured.unwrap_or_default()
        };
        visible.retain(|lane| *lane < self.overview.lanes.len());
        if visible.is_empty() {
            let first = self.active / LANES_PER_PAGE * LANES_PER_PAGE;
            visible.extend(first..(first + LANES_PER_PAGE).min(self.overview.lanes.len()));
        }
        if !visible.contains(&self.active) {
            visible.push(self.active);
        }
        if let Some(index) = self.evidence_lane.filter(|index| !visible.contains(index)) {
            visible.push(index);
        }
        visible
    }

    fn focused_lane(&self) -> usize {
        if self.sigrok.active {
            return self.sigrok.channels().next().unwrap_or(self.active);
        }
        if self.show_transitions
            && let Some(lane) = self
                .protocols
                .data_lane()
                .filter(|lane| *lane < self.overview.lanes.len())
        {
            return lane;
        }
        self.active
    }

    fn follow_event(&mut self, ui: &Ui, link: Option<&Link>, action: protocols::EventAction) {
        let domain = link.map_or(&self.overview.span, |link| &link.domain);
        let step = self.interval();
        self.cursor = Some(
            self.protocols
                .follow_event(action, &mut self.span, domain, step),
        );
        ui.ctx().request_repaint();
    }

    fn waveforms(&mut self, ui: &mut Ui, height: f32, link: Option<&Link>) -> Option<f64> {
        let (outer, frame) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let annotations = self.show_transitions && !self.sigrok.active && !self.protocols.edges();
        let top = if annotations { 25.0 } else { 3.0 };
        let rect = Rect::from_min_max(outer.min + vec2(78.0, top), outer.max - vec2(9.0, 18.0));
        let response = ui.interact(rect, frame.id.with("waveform"), Sense::click_and_drag());
        if let Some(span) = timeline::navigate(ui.ctx(), rect, &response, &self.span, true) {
            let domain = link.map_or(&self.overview.span, |link| &link.domain);
            self.span = timeline::fit(domain, span, self.interval());
        }
        let visible = self.visible_lanes();
        let lanes = visible
            .iter()
            .enumerate()
            .map(|(row, &lane)| (lane, timing::lane(rect, row, visible.len())));
        let draft = self.brush(ui, &response, lanes.clone());
        let painter = ui.painter_at(outer);
        let ink = ui.visuals().weak_text_color();
        if annotations {
            let band = Rect::from_min_max(pos2(rect.left(), outer.top()), rect.right_top());
            let response = ui.interact(band, frame.id.with("annotations"), Sense::click());
            if let Some(action) = plot::annotations(
                ui,
                &painter,
                response,
                &self.span,
                self.protocols.events(&self.span),
                self.protocols.picked_event(&self.span),
            ) {
                self.follow_event(ui, link, action);
            }
        }
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let axis = Link::clock(link).workspace_range(&self.span);
        timing::axes(&painter, rect, &axis, ink);
        let clipped = painter.with_clip_rect(rect);
        self.draw_channels(ui, &painter, &clipped, rect, ink, link);
        self.pick_waveform_channel(ui, rect, &response);
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
        for (lane, plot) in lanes.clone() {
            self.ranges.paint(
                &clipped,
                plot,
                &self.span,
                self.selected.as_ref(),
                lane,
                draft.as_ref(),
            );
        }
        let event = self
            .show_transitions
            .then(|| self.protocols.cursor_event(&self.span, self.cursor))
            .flatten();
        if let Some(event) = event
            && !event.time.is_empty()
        {
            plot::highlight_event(&clipped, rect, &self.span, event.time.clone());
        }
        self.paint_cursor(&clipped, rect, lanes, event.is_some());
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

    fn brush(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        lanes: impl Iterator<Item = (usize, Rect)> + Clone,
    ) -> Option<Draft<usize>> {
        if let Some(drag) = Drag::read(ui, response) {
            let (lane, rect) = lanes.clone().find(|(_, rect)| rect.contains(drag.start))?;
            let draft = Draft::new(drag, rect, &self.span, lanes);
            if !drag.finished {
                return Some(draft);
            }
            self.pick_plotted_lane(lane);
            self.apply_range(draft.times, draft.lanes, None);
        } else if let Some(point) = response.interact_pointer_pos()
            && let Some((lane, rect)) = lanes.clone().find(|(_, rect)| rect.contains(point))
        {
            self.move_cursor(response, rect, lane);
        }
        None
    }

    fn paint_cursor(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        lanes: impl Iterator<Item = (usize, Rect)>,
        event: bool,
    ) {
        let Some(cursor) = self.cursor else { return };
        let x = timing::x_at(rect, &self.span, cursor);
        let color = if event { Color32::GOLD } else { Color32::WHITE };
        for (lane, plot) in lanes {
            if self.selected.is_none() || self.ranges.active.includes(lane) || event {
                painter.vline(x, plot.y_range(), Stroke::new(1.25, color));
            }
        }
    }

    fn pick_waveform_channel(&mut self, ui: &Ui, rect: Rect, hit: &egui::Response) {
        if !hit.clicked_by(PointerButton::Primary) {
            return;
        }
        let Some(point) = hit.interact_pointer_pos() else {
            return;
        };
        let visible = self.visible_lanes();
        let row = ((point.y - rect.top()) / rect.height() * visible.len() as f32) as usize;
        if let Some(&lane) = visible.get(row) {
            self.pick_plotted_lane(lane);
            ui.ctx().request_repaint();
        }
    }

    fn draw_channels(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        clipped: &egui::Painter,
        rect: Rect,
        ink: Color32,
        link: Option<&Link>,
    ) {
        let visible = self.visible_lanes();
        let selected = self.active;
        let frame = self.frame.as_ref().filter(|frame| {
            frame.key.visible == visible
                && frame.key.span.start <= self.span.start
                && frame.key.span.end >= self.span.end
        });
        let count = visible.len();
        let mut wired = None;
        let picking = self.protocols.wiring_active() || self.channel_picker.active();
        for (offset, index) in visible.into_iter().enumerate() {
            let lane = timing::lane(rect, offset, count);
            Link::reveal_trace(link, ui, lane, index);
            Link::paint_trace_evidence(link, clipped, lane, &self.span, index);
            let color = plot::preview_color(index, ui.visuals().dark_mode);
            let name = &self.overview.lanes[index].name;
            if picking {
                if plot::wire_label(ui, painter, lane, name, index) {
                    wired = Some(index);
                }
            } else {
                let label = Rect::from_min_max(
                    pos2(lane.left() - 77.0, lane.top()),
                    pos2(lane.left() - 4.0, lane.bottom()),
                );
                let hit = ui.interact(
                    label,
                    ui.make_persistent_id(("logic trace", index)),
                    Sense::click(),
                );
                if selected == index || hit.hovered() {
                    painter.rect_filled(
                        label.shrink2(vec2(0.0, 2.0)),
                        3.0,
                        ui.visuals().selection.bg_fill.gamma_multiply(0.35),
                    );
                }
                painter
                    .with_clip_rect(label.intersect(painter.clip_rect()))
                    .text(
                        label.right_center() - vec2(3.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        name.rsplit('.').next().unwrap_or(name),
                        egui::FontId::monospace(11.0),
                        color,
                    );
                if hit.on_hover_text(name).clicked() {
                    wired = Some(index);
                }
            }
            clipped.hline(
                lane.x_range(),
                lane.bottom(),
                Stroke::new(0.5, ink.gamma_multiply(0.24)),
            );
            let inner = lane.shrink2(vec2(0.0, lane.height() * 0.2));
            self.draw_wave(
                clipped,
                inner,
                index,
                frame.and_then(|frame| frame.lanes.get(offset)),
                color,
            );
        }
        if let Some(lane) = wired {
            self.pick_plotted_lane(lane);
            ui.ctx().request_repaint();
        }
    }

    fn draw_wave(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        index: usize,
        wave: Option<&Wave>,
        color: Color32,
    ) {
        match wave {
            Some(Wave::Exact(trace, time)) => {
                plot::draw_trace(painter, rect, &self.span, time, trace, color)
            }
            Some(Wave::Summary(envelope)) => {
                plot::draw_envelope(painter, rect, &self.span, envelope, color)
            }
            Some(Wave::Empty) => {}
            None => {
                if let Some(envelope) = self.overview.lanes_previewed.get(index) {
                    plot::draw_envelope(painter, rect, &self.span, envelope, color);
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
            &mut self.followed_row,
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

    fn move_cursor(&mut self, response: &egui::Response, rect: Rect, lane: usize) {
        let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|point| rect.contains(*point))
        else {
            return;
        };
        let time = timing::at(rect, &self.span, pointer.x);
        if response.clicked_by(PointerButton::Primary) {
            self.ranges.archive(&mut self.selected);
            self.cursor = Some(time);
            self.protocols.clear_event_pick();
        } else if response.secondary_clicked() {
            self.ranges.clear_lane(&mut self.selected, lane, time);
        }
    }

    fn apply_range(&mut self, times: [f64; 2], lanes: Vec<usize>, color: Option<usize>) {
        if self.ranges.select(
            &mut self.selected,
            lane_range::interval(times),
            lanes,
            color,
        ) {
            self.cursor = Some(times[1]);
            self.protocols.clear_event_pick();
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
                    self.followed_row = None;
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
        self.request_lanes(ctx, self.visible_lanes());
    }

    fn request_lanes(&mut self, ctx: &Context, visible: Vec<usize>) {
        let key = WindowKey {
            span: self.span.clone(),
            visible,
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
    followed_row: &mut Option<usize>,
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
    if *followed_row != selected {
        if let Some(row) = selected {
            table = table.scroll_to_row(row, None);
        }
        *followed_row = selected;
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
                    ui.add(
                        egui::Label::new(egui::RichText::new(time).monospace()).selectable(false),
                    );
                });
                row.col(|ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(value).monospace()).selectable(false),
                    );
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
