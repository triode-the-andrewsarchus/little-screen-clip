use super::capture::Band;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use little_clip_core::engineering::{
    touchstone::{Complex64, TracePoint},
    waveforms::display_value,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Projection {
    #[default]
    Decibels,
    Magnitude,
    Phase,
    Smith,
    Vswr,
}

impl Projection {
    pub fn title(self) -> &'static str {
        match self {
            Self::Decibels => "Magnitude (dB)",
            Self::Magnitude => "Magnitude",
            Self::Phase => "Phase (°)",
            Self::Smith => "Smith",
            Self::Vswr => "VSWR",
        }
    }

    fn coordinate(self, value: Complex64) -> f64 {
        match self {
            Self::Decibels => decibels(value).max(-160.0),
            Self::Phase => value.arg().to_degrees(),
            Self::Magnitude | Self::Smith => value.norm(),
            Self::Vswr => {
                let reflected = value.norm();
                if reflected < 1.0 {
                    (1.0 + reflected) / (1.0 - reflected)
                } else {
                    f64::NAN
                }
            }
        }
    }

    fn limit(self, data: &[TracePoint]) -> Band {
        if self == Self::Phase {
            return [-180.0, 180.0];
        }
        let [mut lo, mut hi] =
            data.iter()
                .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], point| {
                    let value = self.coordinate(point.value);
                    if value.is_finite() {
                        [lo.min(value), hi.max(value)]
                    } else {
                        [lo, hi]
                    }
                });
        if !lo.is_finite() {
            return [-1.0, 1.0];
        }
        if lo == hi {
            let pad = (0.08 * lo.abs()).max(0.01);
            lo -= pad;
            hi += pad;
        }
        [lo, hi]
    }
}

pub(super) fn decibels(value: Complex64) -> f64 {
    20.0 * value.norm().log10()
}

pub(super) fn domain(points: &[TracePoint]) -> Band {
    let first = points.first().map_or(0.0, |point| point.frequency_hz);
    let last = points.last().map_or(first, |point| point.frequency_hz);
    pad_single([first, last])
}

pub(super) fn pad_single([first, last]: Band) -> Band {
    if first < last {
        return [first, last];
    }
    let pad = (first * 0.001).max(0.01);
    [(first - pad).max(0.0), first + pad]
}

pub(super) fn fit_band(band: Band, full: Band) -> Band {
    let [start, end] = band;
    if !start.is_finite() || !end.is_finite() {
        return full;
    }
    let limit = full[1] - full[0];
    let width = (end - start).abs().clamp(limit * 1e-9, limit);
    let left = start
        .min(end)
        .clamp(full[0], (full[1] - width).max(full[0]));
    [left, (left + width).min(full[1])]
}

pub(super) fn hz_at(rect: Rect, band: Band, x: f32) -> f64 {
    hz_at_fraction(
        band,
        ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0),
    )
}

pub(super) fn hz_at_fraction(band: Band, fraction: f32) -> f64 {
    band[0] + f64::from(fraction) * (band[1] - band[0])
}

pub(super) fn fraction(band: Band, hz: f64) -> f32 {
    ((hz - band[0]) / (band[1] - band[0])) as f32
}

pub(super) fn x_at(rect: Rect, band: Band, hz: f64) -> f32 {
    rect.left() + rect.width() * fraction(band, hz)
}

pub(super) fn visible(data: &[TracePoint], band: Band, outside: bool) -> &[TracePoint] {
    let first = data.partition_point(|point| point.frequency_hz < band[0]);
    let end = data.partition_point(|point| point.frequency_hz <= band[1]);
    let padding = usize::from(outside);
    &data[first.saturating_sub(padding)..end.saturating_add(padding).min(data.len())]
}

pub(super) fn axes(painter: &Painter, rect: Rect, band: Band, ink: Color32) {
    let count = (rect.width() / 135.0).floor().clamp(1.0, 6.0) as usize;
    let line = Stroke::new(1.0, ink.gamma_multiply(0.20));
    for tick in 0..=count {
        let x = rect.left() + rect.width() * tick as f32 / count as f32;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], line);
        let align = if tick == 0 {
            Align2::LEFT_TOP
        } else if tick == count {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        painter.text(
            pos2(x, rect.bottom() + 3.0),
            align,
            display_value(hz_at(rect, band, x), "Hz"),
            FontId::monospace(10.0),
            ink,
        );
    }
}

pub(super) fn trace(
    painter: &Painter,
    rect: Rect,
    band: Band,
    data: &[TracePoint],
    projection: Projection,
    color: Color32,
    labels: bool,
) {
    let data = visible(data, band, true);
    let values = projection.limit(visible(data, band, false));
    let inside = painter.with_clip_rect(rect);
    if labels {
        scale(painter, rect, values, projection);
    }
    paint_trace(&inside, rect, band, data, values, projection, color);
}

pub(super) fn compare_traces(
    painter: &Painter,
    rect: Rect,
    band: Band,
    [recorded, gated]: [&[TracePoint]; 2],
    projection: Projection,
    [recorded_color, gated_color]: [Color32; 2],
) {
    let primary = visible(recorded, band, false);
    let comparison = visible(gated, band, false);
    let base = projection.limit(primary);
    let values = match (primary.is_empty(), comparison.is_empty()) {
        (_, true) => base,
        (true, false) => projection.limit(comparison),
        (false, false) => {
            let limits = projection.limit(comparison);
            [base[0].min(limits[0]), base[1].max(limits[1])]
        }
    };
    scale(painter, rect, values, projection);
    let inside = painter.with_clip_rect(rect);
    for (data, color) in [(recorded, recorded_color), (gated, gated_color)] {
        paint_trace(
            &inside,
            rect,
            band,
            visible(data, band, true),
            values,
            projection,
            color,
        );
    }
}

fn paint_trace(
    inside: &Painter,
    rect: Rect,
    band: Band,
    data: &[TracePoint],
    values: Band,
    projection: Projection,
    color: Color32,
) {
    let mut segment = Vec::with_capacity(data.len());
    let mut previous: Option<f64> = None;
    for point in data {
        let value = projection.coordinate(point.value);
        if !value.is_finite()
            || (projection == Projection::Phase
                && previous.is_some_and(|previous| (value - previous).abs() > 180.0))
        {
            draw_line(inside, std::mem::take(&mut segment), color);
        }
        if value.is_finite() {
            let height = fraction(values, value).clamp(-10.0, 11.0);
            segment.push(pos2(
                x_at(rect, band, point.frequency_hz),
                rect.bottom() - rect.height() * height,
            ));
            previous = Some(value);
        } else {
            previous = None;
        }
    }
    draw_line(inside, segment, color);
}

fn scale(painter: &Painter, rect: Rect, values: Band, projection: Projection) {
    let ctx = painter.ctx();
    let ink = ctx.style_of(ctx.theme()).visuals.weak_text_color();
    let suffix = match projection {
        Projection::Decibels => " dB",
        Projection::Phase => "°",
        _ => "",
    };
    for step in 0..=4 {
        let ratio = step as f32 / 4.0;
        let y = rect.top() + rect.height() * ratio;
        let value = hz_at_fraction(values, 1.0 - ratio);
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, ink.gamma_multiply(0.18)),
        );
        painter.text(
            pos2(
                rect.left() - 4.0,
                y.clamp(rect.top() + 5.0, rect.bottom() - 5.0),
            ),
            Align2::RIGHT_CENTER,
            if projection == Projection::Magnitude {
                format!("{value:.3}")
            } else {
                format!("{value:.1}{suffix}")
            },
            FontId::monospace(10.0),
            ink,
        );
    }
}

fn draw_line(painter: &Painter, points: Vec<Pos2>, color: Color32) {
    if points.len() > 1 {
        painter.add(Shape::line(points, Stroke::new(1.25, color)));
    } else if let Some(point) = points.first() {
        painter.circle_filled(*point, 1.9, color);
    }
}

pub(super) fn highlight(
    painter: &Painter,
    rect: Rect,
    domain: Band,
    selected: Band,
    color: Color32,
) {
    let x = |hz| x_at(rect, domain, hz).clamp(rect.left(), rect.right());
    painter.rect_filled(
        Rect::from_min_max(
            pos2(x(selected[0]), rect.top()),
            pos2(x(selected[1]), rect.bottom()),
        ),
        0.0,
        color.gamma_multiply(0.27),
    );
}

pub(super) fn marker(
    painter: &Painter,
    rect: Rect,
    band: Band,
    hz: f64,
    label: &str,
    color: Color32,
) {
    if hz < band[0] || hz > band[1] {
        return;
    }
    let x = x_at(rect, band, hz);
    painter.line_segment(
        [pos2(x, rect.top()), pos2(x, rect.bottom())],
        Stroke::new(1.25, color),
    );
    painter.text(
        pos2((x + 4.0).min(rect.right() - 14.0), rect.top() + 2.0),
        Align2::LEFT_TOP,
        label,
        FontId::monospace(12.0),
        color,
    );
}

pub(super) struct Smith {
    center: Pos2,
    radius: f32,
}

impl Smith {
    pub fn new(rect: Rect) -> Self {
        Self {
            center: rect.center(),
            radius: (rect.width().min(rect.height()) * 0.44).max(1.0),
        }
    }

    fn screen(&self, value: Complex64) -> Pos2 {
        let coordinate = |value: f64| value.clamp(-100.0, 100.0) as f32;
        self.center + vec2(coordinate(value.re), -coordinate(value.im)) * self.radius
    }

    pub fn nearest(&self, pointer: Pos2, data: &[TracePoint]) -> Option<TracePoint> {
        data.iter()
            .map(|point| (self.screen(point.value).distance_sq(pointer), *point))
            .filter(|(d, _)| *d < 18.0_f32.powi(2))
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, point)| point)
    }

    pub fn paint(&self, painter: &Painter, data: &[TracePoint], color: Color32, ink: Color32) {
        let grid = Stroke::new(1.0, ink.gamma_multiply(0.36));
        painter.circle_stroke(self.center, self.radius, grid);
        painter.line_segment(
            [
                self.center - vec2(self.radius, 0.0),
                self.center + vec2(self.radius, 0.0),
            ],
            grid,
        );
        for factor in [0.2_f64, 0.5, 1.0, 2.0, 5.0] {
            let center = Complex64::new(factor / (1.0 + factor), 0.0);
            painter.circle_stroke(
                self.screen(center),
                self.radius / (1.0 + factor as f32),
                grid,
            );
        }
        for reactance in [0.2, 0.5, 1.0, 2.0, 5.0, -0.2, -0.5, -1.0, -2.0, -5.0] {
            let arc = (0..=40).map(|step| {
                if step == 40 {
                    self.screen(Complex64::new(1.0, 0.0))
                } else {
                    let r = step as f64 / (40 - step) as f64;
                    let z = Complex64::new(r, reactance);
                    self.screen((z - 1.0) / (z + 1.0))
                }
            });
            painter.add(Shape::line(arc.collect(), grid));
        }
        self.trace(painter, data, color);
    }

    pub fn trace(&self, painter: &Painter, data: &[TracePoint], color: Color32) {
        let points = data.iter().map(|point| self.screen(point.value)).collect();
        draw_line(painter, points, color);
    }

    pub fn mark(&self, painter: &Painter, value: Complex64, label: &str, color: Color32) {
        let point = self.screen(value);
        painter.circle_filled(point, 3.0, color);
        painter.text(
            point + vec2(5.0, -2.0),
            Align2::LEFT_BOTTOM,
            label,
            FontId::monospace(11.0),
            color,
        );
    }
}
