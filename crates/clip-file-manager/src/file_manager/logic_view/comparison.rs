use super::{Copies, Link, Rect, Sense, Ui, Viewer, plot, timeline, timing};
use crate::file_manager::scope_view::{Lane, TraceInteraction, lanes::coverage_note, trace_header};
use egui::{Color32, Painter, PointerButton, Stroke, vec2};

impl Viewer {
    pub(in crate::file_manager) fn comparison_visible(&self, index: usize) -> bool {
        !self.comparison_hidden.contains(&index)
    }

    pub(in crate::file_manager) fn prepare_comparison(
        &mut self,
        ui: &Ui,
        link: &Link,
        span: std::ops::Range<f64>,
    ) {
        self.poll(ui.ctx());
        self.span = span;
        self.refine(ui.ctx(), false);
        self.comparison_lanes.clear();
        if let Some(index) = link.reveal_input {
            self.comparison_hidden.retain(|&hidden| hidden != index);
        }
    }

    pub(in crate::file_manager) fn finish_comparison(&mut self, ui: &Ui) {
        let lanes = std::mem::take(&mut self.comparison_lanes);
        self.request_lanes(ui.ctx(), lanes);
    }

    pub(in crate::file_manager) fn comparison_trace<T: Copy + Send + Sync + 'static>(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        index: usize,
        source: &str,
        active: bool,
        link: &Link,
        key: T,
    ) -> TraceInteraction {
        let visible = self.comparison_visible(index);
        let lane = Lane::comparison(area, visible);
        if !ui.is_rect_visible(lane.frame) {
            return TraceInteraction::default();
        }
        let painter = ui.painter_at(area);
        lane.background(ui, &painter);
        let mut heading = lane.heading(ui, "logic-heading");
        let color = plot::preview_color(index, ui.visuals().dark_mode);
        let (selected, hide) = trace_header(
            &mut heading,
            key,
            &self.overview.lanes[index].name,
            Some(source),
            color,
            active,
            visible,
        );
        if hide && visible {
            self.comparison_hidden.push(index);
            ui.ctx().request_repaint();
        } else if !visible && (selected || hide) {
            self.comparison_hidden.retain(|&hidden| hidden != index);
            ui.ctx().request_repaint();
        }
        if !visible {
            return TraceInteraction {
                selected,
                ..Default::default()
            };
        }
        self.comparison_lanes.push(index);
        let hit = ui.interact(
            lane.plot,
            ui.make_persistent_id("logic-trace"),
            Sense::click_and_drag(),
        );
        let previous = self.span.clone();
        if let Some(span) = timeline::navigate(ui.ctx(), lane.plot, &hit, &self.span, true) {
            self.span = timeline::fit(&link.domain, span, self.interval());
        }
        let pointer_select =
            hit.clicked_by(PointerButton::Primary) || hit.drag_started_by(PointerButton::Primary);
        if pointer_select || selected {
            self.pick_plotted_lane(index);
        }
        self.move_cursor(ui, lane.plot, &hit);
        let rect = lane.plot;
        let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        self.paint_comparison_wave(&clipped, rect, index, color, link);
        let probe = hit
            .hover_pos()
            .map(|point| timing::at(rect, &self.span, point.x));
        if let Some(time) = probe.or(link.probe) {
            clipped.vline(
                timing::x_at(rect, &self.span, time),
                rect.y_range(),
                Stroke::new(0.85, ui.visuals().weak_text_color()),
            );
        }
        TraceInteraction {
            selected: selected || pointer_select,
            probe,
            span_changed: self.span != previous,
            ..Default::default()
        }
    }

    fn paint_comparison_wave(
        &self,
        painter: &Painter,
        rect: Rect,
        index: usize,
        color: Color32,
        link: &Link,
    ) {
        let inner = rect.shrink2(vec2(0.0, rect.height() * 0.19));
        let wave = self
            .frame
            .as_ref()
            .filter(|frame| {
                frame.key.span.start <= self.span.start && frame.key.span.end >= self.span.end
            })
            .and_then(|frame| {
                let offset = frame.key.visible.iter().position(|&shown| shown == index)?;
                frame.lanes.get(offset)
            });
        self.draw_wave(painter, inner, index, wave, color);
        let recorded = &self.overview.lanes[index];
        coverage_note(
            painter,
            rect,
            &self.span,
            recorded.start_seconds..recorded.end_seconds,
            painter.ctx().global_style().visuals.weak_text_color(),
        );
        if let Some(selected) = &self.selected {
            timing::highlight(
                painter,
                rect,
                &self.span,
                selected.clone(),
                color.gamma_multiply(0.18),
            );
        }
        Link::paint_trace_evidence(Some(link), painter, rect, &self.span, index);
        link.paint_markers(painter, rect, &self.span);
    }

    pub(in crate::file_manager) fn comparison_tools(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        index: usize,
        link: &Link,
    ) -> Option<std::ops::Range<f64>> {
        if self.active != index {
            self.active = index;
            self.frequency.spectra.clear_result();
        }
        let share = self.toolbar(ui, copies, Some(link), false);
        if let Some(message) = self.error.as_ref().or(self.overview.note.as_ref()) {
            ui.weak(message);
        }
        self.decoder_controls(ui);
        self.decode_events(ui, Some(link));
        share
    }

    pub(in crate::file_manager) fn spectrum_time_evidence(&self) -> bool {
        self.frequency.spectra.origin().is_some()
    }

    pub(in crate::file_manager) fn comparison_spectrum(
        &mut self,
        ui: &mut Ui,
        index: usize,
        link: &Link,
    ) -> bool {
        self.active = index;
        self.frequency.visible = true;
        ui.horizontal(|ui| {
            if ui.small_button("Close spectrum").clicked() {
                self.frequency.visible = false;
            }
        });
        super::super::signal_view::frequency_plot::origin_controls(
            ui,
            &mut self.frequency.spectra,
            &mut self.span,
            link.clock,
        );
        if self.frequency.visible && self.frequency.spectra.origin().is_none() {
            self.frequency(ui, Some(link));
        }
        self.frequency.visible
    }
}
