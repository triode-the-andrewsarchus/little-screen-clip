use crate::{
    file_manager::{preview_copy, scope_view::plot as axis, signal_view::timeline},
    ui_appearance::WorkbenchColors,
};
use egui::{Align2, Color32, FontId, Mesh, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::radio::{Channel, frequency_label};
use std::ops::{Range, RangeInclusive};

pub(super) struct View {
    pub open: bool,
    pub brush: Option<RangeInclusive<u64>>,
    lanes: [Vec<Tick>; 2],
    domain: Range<f64>,
    span: Range<f64>,
}

#[derive(Clone, Copy)]
struct Tick {
    hz: u64,
    channel: usize,
}

pub(super) enum Action {
    Select(usize),
    Filter,
}

impl Default for View {
    fn default() -> Self {
        Self {
            open: true,
            brush: None,
            lanes: [Vec::new(), Vec::new()],
            domain: 0.0..1.0,
            span: 0.0..1.0,
        }
    }
}

impl View {
    pub(super) fn rebuild(&mut self, channels: &[Channel], matched: &[usize]) {
        self.lanes.iter_mut().for_each(Vec::clear);
        for &index in matched {
            let channel = &channels[index];
            self.lanes[0].push(Tick {
                hz: channel.receive_hz,
                channel: index,
            });
            if let Some(hz) = channel.transmit_hz() {
                self.lanes[1].push(Tick { hz, channel: index });
            }
        }
        for lane in &mut self.lanes {
            lane.sort_unstable_by_key(|tick| (tick.hz, tick.channel));
        }
        let low = self
            .lanes
            .iter()
            .filter_map(|lane| lane.first())
            .map(|tick| tick.hz)
            .min();
        let high = self
            .lanes
            .iter()
            .filter_map(|lane| lane.last())
            .map(|tick| tick.hz)
            .max();
        self.domain = low.zip(high).map_or(0.0..1.0, |(low, high)| {
            let padding = (((high - low) as f64) * 0.035).max(500.0);
            (low as f64 - padding).max(0.0)..high as f64 + padding
        });
        self.fit();
    }

    pub(super) fn fit(&mut self) {
        self.span = self.domain.clone();
    }

    pub(super) fn includes(&self, channel: &Channel) -> bool {
        self.brush.as_ref().is_none_or(|brush| {
            brush.contains(&channel.receive_hz)
                || channel.transmit_hz().is_some_and(|hz| brush.contains(&hz))
        })
    }

    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        channels: &[Channel],
        selected: Option<usize>,
        colors: WorkbenchColors,
    ) -> Option<Action> {
        if !self.open || self.lanes[0].is_empty() {
            return None;
        }
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 83.0), Sense::hover());
        let rect = Rect::from_min_max(outer.min + vec2(33.0, 2.0), outer.max - vec2(9.0, 17.0));
        let response = ui.interact(
            rect,
            ui.make_persistent_id("radio frequency map"),
            Sense::click_and_drag(),
        );
        if let Some(next) = timeline::navigate(ui.ctx(), rect, &response, &self.span, true) {
            self.span = timeline::fit(&self.domain, next, 10.0);
        }
        let modifier = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, brush) = preview_copy::drag_range(ui, &response, response.id, modifier);
        let action = if let Some((start, end)) = brush {
            let left = (self.span.end - self.span.start).mul_add(f64::from(start), self.span.start);
            let right = (self.span.end - self.span.start).mul_add(f64::from(end), self.span.start);
            let limits = left.ceil() as u64..=right.floor() as u64;
            self.update_brush(Some(limits))
        } else if response.secondary_clicked() {
            self.update_brush(None)
        } else {
            None
        };
        self.paint(ui.painter_at(outer), rect, channels, selected, colors);
        let Some(pointer) = response.hover_pos() else {
            return action;
        };
        let lane = usize::from(pointer.y >= rect.center().y);
        let hz = axis::at(rect, &self.span, pointer.x);
        let painter = ui.painter_at(rect);
        painter.vline(
            pointer.x,
            rect.y_range(),
            Stroke::new(0.85, colors.text_muted),
        );
        let tick = self.nearest(rect, lane, hz, selected);
        let clicked = response.clicked();
        response.on_hover_text(self.tooltip(channels, tick, lane, hz));
        if clicked && let Some(tick) = tick {
            if !self.includes(&channels[tick.channel]) {
                self.brush = None;
            }
            return Some(Action::Select(tick.channel));
        }
        action
    }

    fn update_brush(&mut self, brush: Option<RangeInclusive<u64>>) -> Option<Action> {
        if self.brush == brush || brush.as_ref().is_some_and(RangeInclusive::is_empty) {
            return None;
        }
        self.brush = brush;
        Some(Action::Filter)
    }

    fn nearest(&self, rect: Rect, lane: usize, hz: f64, selected: Option<usize>) -> Option<Tick> {
        let positions = &self.lanes[lane];
        let index = positions.partition_point(|tick| (tick.hz as f64) < hz);
        let previous = index.checked_sub(1).map(|index| positions[index]);
        let next = positions.get(index).copied();
        let chosen = [previous, next]
            .into_iter()
            .flatten()
            .filter(|tick| (self.span.start..=self.span.end).contains(&(tick.hz as f64)))
            .min_by(|a, b| ((a.hz as f64 - hz).abs()).total_cmp(&(b.hz as f64 - hz).abs()))?;
        let max_distance =
            (self.span.end - self.span.start) * 9.0 / f64::from(rect.width().max(1.0));
        if (chosen.hz as f64 - hz).abs() > max_distance {
            return None;
        }
        let first = positions.partition_point(|tick| tick.hz < chosen.hz);
        let end = positions.partition_point(|tick| tick.hz <= chosen.hz);
        positions[first..end]
            .iter()
            .find(|tick| Some(tick.channel) == selected)
            .or_else(|| positions.get(first))
            .copied()
    }

    fn tooltip(&self, channels: &[Channel], tick: Option<Tick>, lane: usize, hz: f64) -> String {
        let Some(tick) = tick else {
            return format!(
                "{}\nDrag to filter both lanes · Scroll to zoom\nMiddle drag to pan · Right-click to clear",
                frequency_label(hz.round() as u64)
            );
        };
        let channel = &channels[tick.channel];
        let label = ["RX", "TX"][lane];
        format!(
            "{label} {}\n{}  {}",
            frequency_label(tick.hz),
            channel.number,
            channel.name
        )
    }

    fn paint(
        &self,
        painter: Painter,
        rect: Rect,
        channels: &[Channel],
        selected: Option<usize>,
        colors: WorkbenchColors,
    ) {
        painter.rect_filled(rect, 3.0, colors.recessed);
        let clipped = painter.with_clip_rect(rect);
        if let Some(brush) = &self.brush {
            axis::highlight(
                &clipped,
                rect,
                &self.span,
                *brush.start() as f64..*brush.end() as f64,
                colors.accent.gamma_multiply(0.23),
            );
        }
        axis::labelled_x_axis(&painter, rect, &self.span, "Hz", colors.text_muted);
        for (index, label) in ["RX", "TX"].into_iter().enumerate() {
            let lane = axis::lane(rect, index, 2);
            painter.text(
                lane.left_center() - vec2(6.0, 0.0),
                Align2::RIGHT_CENTER,
                label,
                FontId::monospace(11.0),
                colors.text_primary,
            );
            clipped.hline(
                lane.x_range(),
                lane.center().y,
                Stroke::new(0.7, colors.text_muted.gamma_multiply(0.27)),
            );
            self.paint_lane(&clipped, lane, index, axis::color(index));
        }
        if let Some(channel) = selected.and_then(|selected| channels.get(selected)) {
            self.paint_selected(&clipped, rect, channel, colors.focus);
        }
    }

    fn paint_lane(&self, painter: &Painter, lane: Rect, index: usize, color: Color32) {
        let positions = &self.lanes[index];
        let start = positions.partition_point(|tick| (tick.hz as f64) < self.span.start);
        let end = positions.partition_point(|tick| (tick.hz as f64) <= self.span.end);
        let mut marks = Mesh::default();
        let mut ticks = positions[start..end].iter().peekable();
        let column = |tick: &Tick| axis::x_at(lane, &self.span, tick.hz as f64).round();
        while let Some(tick) = ticks.next() {
            let x = column(tick);
            let mut count = 1;
            while ticks.next_if(|tick| column(tick) == x).is_some() {
                count += 1;
            }
            let height = (5.0 + (count as f32).log2() * 2.1).min(lane.height() * 0.42);
            let y = lane.center().y;
            marks.add_colored_rect(
                Rect::from_min_max(pos2(x - 0.6, y - height), pos2(x + 0.6, y + height)),
                color,
            );
        }
        painter.add(marks);
    }

    fn paint_selected(&self, painter: &Painter, rect: Rect, channel: &Channel, color: Color32) {
        let [rx, tx] = [Some(channel.receive_hz), channel.transmit_hz()]
            .map(|frequency| frequency.map(|hz| axis::x_at(rect, &self.span, hz as f64)));
        let rx_y = axis::lane(rect, 0, 2).center().y;
        let tx_y = axis::lane(rect, 1, 2).center().y;
        if let Some((rx, tx)) = rx.zip(tx)
            && rect.x_range().contains(rx)
            && rect.x_range().contains(tx)
        {
            painter.line_segment(
                [pos2(rx, rx_y), pos2(tx, tx_y)],
                Stroke::new(1.0, color.gamma_multiply(0.55)),
            );
        }
        for (x, y) in [(rx, rx_y), (tx, tx_y)] {
            if let Some(x) = x.filter(|&x| rect.x_range().contains(x)) {
                painter.circle_stroke(pos2(x, y), 6.5, Stroke::new(1.35, color));
            }
        }
    }
}
