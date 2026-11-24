use super::{
    super::{filter_editor::filter_ink, timeline},
    compute::{Frame, Lane},
    frequency::Frequency,
};
use crate::file_manager::{
    logic_view,
    scope_view::{
        hover::{ProbeReading, paint_trace_readout},
        lanes, plot,
    },
};
use egui::{Color32, Painter, Rect, Response, Sense, Ui, vec2};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Trace {
    Equation(usize),
    Filtered(usize),
}

impl Trace {
    pub fn index(self) -> usize {
        match self {
            Self::Equation(index) | Self::Filtered(index) => index,
        }
    }

    pub fn color(self, ui: &Ui) -> Color32 {
        match self {
            Self::Equation(index) => {
                let color = plot::color(index + 1);
                if ui.visuals().dark_mode {
                    color
                } else {
                    let [r, g, b] = [color.r(), color.g(), color.b()]
                        .map(|value| (f32::from(value) * 0.60) as u8);
                    Color32::from_rgb(r, g, b)
                }
            }
            Self::Filtered(_) => filter_ink(ui.visuals().dark_mode),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct EquationDisplay {
    pub equation: TraceDisplay,
    pub filtered: TraceDisplay,
}

impl EquationDisplay {
    pub fn valid(self) -> bool {
        [self.equation.offset, self.filtered.offset]
            .into_iter()
            .all(|value| value.abs() <= 1.0e12)
    }
}

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct TraceDisplay {
    pub hidden: bool,
    pub offset: f32,
}

impl TraceDisplay {
    pub fn controls(&mut self, ui: &mut Ui, lane: Option<&Lane>, origin: bool) {
        if self.hidden && origin {
            ui.weak("Peak source");
        } else {
            let action = if self.hidden { "Show" } else { "Hide" };
            if ui
                .small_button(action)
                .on_hover_text("Show or hide this trace on the time axis")
                .clicked()
            {
                self.hidden = !self.hidden;
            }
        }
        if (!self.hidden || origin)
            && let Some(Lane::Analog {
                channel, bounds, ..
            }) = lane
        {
            lanes::offset_control(ui, &channel.unit, *bounds, &mut self.offset);
        }
    }
}

pub(super) struct TracePlot {
    pub rect: Rect,
    pub response: Response,
    pub trace: Trace,
    offset: f32,
}

impl TracePlot {
    pub fn allocate(ui: &mut Ui, trace: Trace, offset: f32) -> Self {
        let (area, reserved) =
            ui.allocate_exact_size(vec2(ui.available_width(), 65.0), Sense::hover());
        let rect = Rect::from_min_max(area.min + vec2(78.0, 7.0), area.max - vec2(9.0, 7.0));
        let response = ui.interact(
            rect,
            reserved.id.with("derived-time"),
            Sense::click_and_drag(),
        );
        Self {
            rect,
            response,
            trace,
            offset,
        }
    }

    fn samples<'a>(&self, frame: &'a Frame, frequency: &'a Frequency) -> Option<&'a Lane> {
        match self.trace {
            Trace::Equation(index) => frame.lanes.get(index),
            Trace::Filtered(index) => frequency.filtered_lane(index),
        }
    }

    fn analog(
        &self,
        ui: &Ui,
        clipped: &Painter,
        lane: &Lane,
        visible: &Range<f64>,
        probe: Option<f64>,
    ) {
        let Lane::Analog {
            channel,
            raw,
            envelope,
            bounds,
        } = lane
        else {
            return;
        };
        let rect = self.rect;
        let color = self.trace.color(ui);
        let display = lanes::stacked_scale(*bounds, self.offset);
        let sample_bounds = lanes::shifted(display, -self.offset);
        plot::paint_y_scale(ui.painter(), rect, display, &channel.unit, color);
        if raw.samples.len() <= 4096 {
            let (start, samples) = raw.visible(visible);
            plot::draw_raw(
                clipped,
                rect,
                visible,
                start,
                raw.step,
                samples,
                sample_bounds,
                color,
            );
        } else {
            plot::draw_envelope(
                clipped,
                rect,
                visible,
                channel,
                envelope,
                sample_bounds,
                color,
            );
        }
        if let Some(time) = probe {
            let x = plot::x_at(rect, visible, time);
            if let Some(value) = raw.value_at(time) {
                ProbeReading::derived(
                    rect,
                    sample_bounds,
                    value,
                    &channel.unit,
                    self.offset,
                    color,
                )
                .paint(clipped, x);
            } else {
                paint_trace_readout(clipped, rect, x, None, None, color);
            }
        }
    }

    fn digital(
        &self,
        ui: &Ui,
        painter: &Painter,
        lane: &Lane,
        visible: &Range<f64>,
        captured: &Range<f64>,
        probe: Option<f64>,
    ) {
        let Lane::Digital {
            trace, envelope, ..
        } = lane
        else {
            return;
        };
        let rect = self.rect;
        let color = self.trace.color(ui);
        if trace.edges.len() <= 4096 {
            logic_view::plot::draw_trace(painter, rect, visible, captured, trace, color);
        } else {
            logic_view::plot::draw_envelope(painter, rect, visible, envelope, color);
        }
        if let Some(time) = probe {
            let level = envelope
                .time
                .contains(&time)
                .then(|| {
                    let edge = trace
                        .edges
                        .partition_point(|edge| edge.time_seconds <= time);
                    edge.checked_sub(1)
                        .map(|index| trace.edges[index].level)
                        .or(trace.initial)
                })
                .flatten();
            let x = plot::x_at(rect, visible, time);
            let y = level.map(|level| logic_view::plot::y_at(rect, Some(level)));
            let caption = level.map(|level| logic_view::level_label(level).to_owned());
            paint_trace_readout(painter, rect, x, y, caption, color);
        }
    }
}

impl Frame {
    #[expect(
        clippy::too_many_arguments,
        reason = "plot rows share a timeline and selection"
    )]
    pub(super) fn paint(
        &self,
        ui: &mut Ui,
        plots: &[TracePlot],
        span: &Range<f64>,
        selected: Option<&Range<f64>>,
        frequency: &Frequency,
        probe: Option<f64>,
        reference: Option<f64>,
    ) {
        let last = plots.last().expect("nonempty plot rows").rect;
        ui.allocate_space(vec2(0.0, 14.0));
        let painter = ui.painter();
        let rects = plots
            .iter()
            .map(|plot| plot.rect)
            .filter(|rect| ui.is_rect_visible(*rect));
        for rect in rects.clone() {
            painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        }
        plot::x_axis_with_grid(
            painter,
            last,
            span,
            "s",
            ui.visuals().weak_text_color(),
            rects.map(|rect| rect.y_range()),
        );
        let probe = probe.filter(|time| span.contains(time));
        for row in plots.iter().filter(|plot| ui.is_rect_visible(plot.rect)) {
            let rect = row.rect;
            let clipped = painter.with_clip_rect(rect);
            if let Some(selected) = selected {
                plot::highlight(
                    &clipped,
                    rect,
                    span,
                    selected.clone(),
                    plot::color(1).gamma_multiply(0.22),
                );
            }
            match row.samples(self, frequency) {
                Some(lane @ Lane::Analog { .. }) => row.analog(ui, &clipped, lane, span, probe),
                Some(lane @ Lane::Digital { .. }) => {
                    row.digital(ui, &clipped, lane, span, &self.span, probe)
                }
                None => {}
            }
            timeline::paint_reference(&clipped, rect, span, reference);
        }
    }
}
