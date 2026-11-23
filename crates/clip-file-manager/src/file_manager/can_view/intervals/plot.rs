use super::{activity, duration};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Painter, PointerButton, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::can::intervals::{Gap, Report};
use std::ops::Range;

pub(super) enum Hit {
    None,
    Gap(Gap),
    Range(Range<i128>),
}

pub(super) struct View<'a> {
    pub scope: &'a Range<i128>,
    pub origin: i128,
    pub log: bool,
    pub selected: Option<Gap>,
    pub colors: WorkbenchColors,
}

struct Axes<'a> {
    rect: Rect,
    view: View<'a>,
    maximum: f64,
}

pub(super) fn show(ui: &mut Ui, report: &Report, view: View<'_>, brush: &mut Option<i128>) -> Hit {
    ui.strong("Interval duration by later frame time");
    let height = (ui.available_height() * 0.44).clamp(104.0, 235.0);
    let (outer, frame) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let plot = Rect::from_min_max(outer.min + vec2(65.0, 8.0), outer.max - vec2(9.0, 22.0));
    if !plot.is_positive() {
        return Hit::None;
    }
    let response = ui.interact(plot, frame.id.with("CAN gap plot"), Sense::click_and_drag());
    let maximum = report
        .longest
        .first()
        .map_or(3, |gap| gap.duration_ns().max(3)) as f64;
    let axes = Axes {
        rect: plot,
        view,
        maximum,
    };
    let painter = ui.painter_at(outer);
    axes.draw(&painter, report);
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos());
    let hovered = pointer
        .filter(|point| plot.contains(*point))
        .and_then(|point| {
            report
                .bands
                .iter()
                .flat_map(|bin| [bin.longest, bin.shortest])
                .map(|gap| (gap, axes.point(gap).distance_sq(point)))
                .filter(|&(_, distance)| distance <= 14.0 * 14.0)
                .min_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(gap, _)| gap)
        });
    if let Some(gap) = hovered {
        painter.circle_stroke(
            axes.point(gap),
            5.0,
            Stroke::new(1.5, axes.view.colors.focus),
        );
        response.clone().on_hover_text(format!(
            "{} · frames {} and {}",
            duration(gap.duration_ns()),
            gap.earlier.row + 1,
            gap.later.row + 1,
        ));
    }
    if let Some(span) = axes.brush(ui, &response, brush, &painter) {
        return Hit::Range(span);
    }
    if response.clicked_by(PointerButton::Primary)
        && let Some(gap) = hovered
    {
        return Hit::Gap(gap);
    }
    Hit::None
}

impl Axes<'_> {
    fn point(&self, gap: Gap) -> egui::Pos2 {
        let elapsed = self.scale(gap.duration_ns() as f64) / self.scale(self.maximum);
        pos2(
            activity::x(self.rect, self.view.scope, gap.later.time_ns),
            self.y(elapsed as f32),
        )
    }

    fn y(&self, fraction: f32) -> f32 {
        self.rect.bottom() - self.rect.height() * 0.92 * fraction
    }

    fn scale(&self, duration: f64) -> f64 {
        if self.view.log {
            duration.ln_1p()
        } else {
            duration
        }
    }

    fn draw(&self, painter: &Painter, report: &Report) {
        let colors = self.view.colors;
        painter.rect_filled(self.rect, 3.0, colors.recessed);
        self.ticks(painter);
        self.bands(&painter.with_clip_rect(self.rect), report);
        if let Some(gap) = self.view.selected {
            let range = gap.earlier.time_ns..gap.later.time_ns.saturating_add(1);
            activity::highlight(painter, self.rect, self.view.scope, &range, colors);
            painter.circle_filled(self.point(gap), 4.0, colors.focus);
        }
    }

    fn ticks(&self, painter: &Painter) {
        let ink = self.view.colors.text_muted;
        let font = FontId::monospace(10.5);
        let grid = Stroke::new(0.6, ink.gamma_multiply(0.42));
        for tick in 0..=3 {
            let fraction = tick as f32 / 3.0;
            let y = self.y(fraction);
            painter.hline(self.rect.x_range(), y, grid);
            let scaled = self.scale(self.maximum) * f64::from(fraction);
            let ns = if self.view.log {
                scaled.exp_m1()
            } else {
                scaled
            };
            painter.text(
                pos2(self.rect.left() - 5.0, y),
                Align2::RIGHT_CENTER,
                duration(ns.round() as u128),
                font.clone(),
                ink,
            );
        }
        let start = self.view.scope.start;
        let end = self.view.scope.end - 1;
        let middle = start.midpoint(end);
        for (time, align) in [
            (start, Align2::LEFT_TOP),
            (middle, Align2::CENTER_TOP),
            (end, Align2::RIGHT_TOP),
        ] {
            if (align == Align2::CENTER_TOP && (middle == start || middle == end))
                || (align == Align2::RIGHT_TOP && start == end)
            {
                continue;
            }
            let x = activity::x(self.rect, self.view.scope, time);
            painter.vline(x, self.rect.y_range(), grid);
            painter.text(
                pos2(x, self.rect.bottom() + 4.0),
                align,
                activity::time_label(time - self.view.origin),
                font.clone(),
                ink,
            );
        }
    }

    fn bands(&self, painter: &Painter, report: &Report) {
        let ink = self.view.colors.accent;
        for band in &report.bands {
            let shortest = self.point(band.shortest);
            let longest = self.point(band.longest);
            let x = shortest.x.midpoint(longest.x);
            let width = ((band.count as f32).ln_1p() * 0.48).clamp(1.0, 3.0);
            if shortest != longest {
                painter.vline(
                    x,
                    longest.y..=shortest.y,
                    Stroke::new(width, ink.gamma_multiply(0.55)),
                );
                painter.circle_filled(longest, 2.0, ink);
            }
            painter.circle_filled(shortest, 2.0, ink);
        }
    }

    fn brush(
        &self,
        ui: &Ui,
        response: &Response,
        anchor: &mut Option<i128>,
        painter: &Painter,
    ) -> Option<Range<i128>> {
        let at = |point: egui::Pos2| {
            activity::at(self.rect, self.view.scope, point.x).min(self.view.scope.end - 1)
        };
        if response.drag_started_by(PointerButton::Primary) {
            *anchor = ui.input(|input| input.pointer.press_origin()).map(at);
        }
        let stopped = response.drag_stopped_by(PointerButton::Primary);
        let pointer = response.interact_pointer_pos().or_else(|| {
            stopped
                .then(|| ui.input(|input| input.pointer.latest_pos()))
                .flatten()
        });
        if stopped && pointer.is_none() {
            *anchor = None;
        }
        let current = at(pointer?);
        let start = (*anchor)?;
        let span = start.min(current)..start.max(current) + 1;
        if stopped {
            *anchor = None;
            return (start != current).then_some(span);
        }
        if response.dragged_by(PointerButton::Primary) {
            activity::highlight(painter, self.rect, self.view.scope, &span, self.view.colors);
        }
        None
    }
}
