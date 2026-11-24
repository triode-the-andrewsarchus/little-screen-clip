use super::capture::Band;
use egui::{
    Align2, Color32, FontId, Id, Painter, PointerButton, Pos2, Rect, Response, Sense, Shape,
    Stroke, Ui, pos2, vec2,
};
use little_clip_core::engineering::{
    touchstone::{Complex64, TracePoint, comparison::Sample},
    waveforms::display_axis_value,
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

    pub(super) fn coordinate(self, value: Complex64) -> f64 {
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

    pub(super) fn marker_value(self, sample: Sample) -> f64 {
        let [a, b] = sample.rows;
        if self == Self::Phase && [a, b].iter().any(|point| point.value == Complex64::ZERO) {
            return f64::NAN;
        }
        let left = self.coordinate(a.value);
        if sample.exact() {
            return left;
        }
        let step = self.difference(self.coordinate(b.value), left);
        let t = (sample.frequency_hz - a.frequency_hz) / (b.frequency_hz - a.frequency_hz);
        let result = step.mul_add(t, left);
        if self == Self::Phase {
            wrap_phase(result)
        } else {
            result
        }
    }

    pub(super) fn difference(self, after: f64, before: f64) -> f64 {
        let delta = after - before;
        if self == Self::Phase {
            wrap_phase(delta)
        } else {
            delta
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

fn wrap_phase(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

pub(super) use little_clip_core::engineering::touchstone::magnitude_db as decibels;

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

pub(super) fn navigate(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    mut viewport: Band,
    full: Band,
    in_scroll_area: bool,
) -> Band {
    if response.dragged_by(PointerButton::Middle) {
        let width = viewport[1] - viewport[0];
        let delta = f64::from(response.drag_delta().x / rect.width()) * width;
        viewport = fit_band(viewport.map(|edge| edge - delta), full);
    }
    if let Some(pos) = response.hover_pos().filter(|pos| rect.contains(*pos)) {
        let scroll = ui.input_mut(|input| {
            if in_scroll_area && input.modifiers.shift {
                return 0.0;
            }
            let scroll = input.smooth_scroll_delta().y;
            if in_scroll_area {
                input.smooth_scroll_delta.y = 0.0;
            }
            scroll
        });
        if scroll != 0.0 {
            let anchor = hz_at(rect, viewport, pos.x);
            let factor = f64::from((-scroll * 0.003).clamp(-2.0, 2.0).exp());
            viewport = fit_band(viewport.map(|edge| anchor + (edge - anchor) * factor), full);
        }
    }
    viewport
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
            display_axis_value(
                hz_at(rect, band, x),
                (band[1] - band[0]) / count as f64,
                "Hz",
            ),
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
) -> Band {
    let data = visible(data, band, true);
    let values = projection.limit(visible(data, band, false));
    let inside = painter.with_clip_rect(rect);
    if labels {
        trace_scale(painter, rect, values, projection);
    }
    paint_trace(&inside, rect, band, data, values, projection, color);
    values
}

pub(super) fn compare_traces(
    painter: &Painter,
    rect: Rect,
    band: Band,
    [recorded, gated]: [&[TracePoint]; 2],
    projection: Projection,
    [recorded_color, gated_color]: [Color32; 2],
) -> Band {
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
    trace_scale(painter, rect, values, projection);
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
    values
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
    let points = data
        .iter()
        .map(|point| (point.frequency_hz, projection.coordinate(point.value)));
    paint_curve(
        inside,
        rect,
        band,
        points,
        values,
        projection == Projection::Phase,
        color,
    );
}

pub(super) fn paint_curve(
    inside: &Painter,
    rect: Rect,
    band: Band,
    data: impl ExactSizeIterator<Item = (f64, f64)>,
    values: Band,
    wrap_phase: bool,
    color: Color32,
) {
    let mut segment = Vec::with_capacity(data.len());
    let mut previous: Option<f64> = None;
    for (frequency_hz, value) in data {
        if !value.is_finite()
            || (wrap_phase && previous.is_some_and(|previous| (value - previous).abs() > 180.0))
        {
            draw_line(inside, std::mem::take(&mut segment), color);
        }
        if value.is_finite() {
            let height = fraction(values, value).clamp(-10.0, 11.0);
            segment.push(pos2(
                x_at(rect, band, frequency_hz),
                rect.bottom() - rect.height() * height,
            ));
            previous = Some(value);
        } else {
            previous = None;
        }
    }
    draw_line(inside, segment, color);
}

fn trace_scale(painter: &Painter, rect: Rect, values: Band, projection: Projection) {
    let suffix = match projection {
        Projection::Decibels => " dB",
        Projection::Phase => "°",
        _ => "",
    };
    scale(painter, rect, values, |value| {
        if projection == Projection::Magnitude {
            format!("{value:.3}")
        } else {
            format!("{value:.1}{suffix}")
        }
    });
}

pub(super) fn scale(painter: &Painter, rect: Rect, values: Band, label: impl Fn(f64) -> String) {
    let ctx = painter.ctx();
    let ink = ctx.style_of(ctx.theme()).visuals.weak_text_color();
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
            label(value),
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

pub(super) fn cutoff_handle(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    values: Band,
    db: f64,
    color: Color32,
    id: Id,
) -> Option<f64> {
    if !(values[0]..=values[1]).contains(&db) {
        return None;
    }
    let y = rect.bottom() - fraction(values, db) * rect.height();
    let center = pos2(rect.right() - 4.0, y);
    let handle = Rect::from_center_size(center, vec2(22.0, 18.0));
    let response = ui
        .interact(handle, id, Sense::drag())
        .on_hover_text("Drag the cutoff");
    painter.hline(rect.x_range(), y, Stroke::new(1.0, color));
    painter.circle_filled(center, if response.hovered() { 5.5 } else { 4.0 }, color);
    let pointer = response
        .interact_pointer_pos()
        .filter(|_| response.dragged())?;
    let height = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
    Some(f64::from(height).mul_add(values[1] - values[0], values[0]))
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
    zoom: f32,
}

#[derive(Clone, Copy)]
struct SmithCamera {
    zoom: f32,
    offset: egui::Vec2,
}

impl Smith {
    pub fn interactive(rect: Rect, hit: &Response) -> Self {
        let id = hit.id.with("smith-camera");
        let ctx = &hit.ctx;
        let mut camera = ctx.data_mut(|data| {
            data.get_temp::<SmithCamera>(id).unwrap_or(SmithCamera {
                zoom: 1.0,
                offset: egui::Vec2::ZERO,
            })
        });
        let radius = (rect.width().min(rect.height()) * 0.44).max(1.0);
        if hit.double_clicked_by(PointerButton::Primary) {
            camera.zoom = 1.0;
            camera.offset = egui::Vec2::ZERO;
        } else if let Some(point) = hit.hover_pos().filter(|p| rect.contains(*p)) {
            let wheel = ctx.input_mut(|input| {
                let wheel = input.smooth_scroll_delta().y;
                if wheel != 0.0 {
                    input.smooth_scroll_delta().y = 0.0;
                }
                wheel
            });
            let zoom = (camera.zoom * (wheel * 0.004).exp()).clamp(1.0, 24.0);
            let anchor = (point - rect.center()) / radius;
            camera.offset = anchor + (camera.offset - anchor) * (zoom / camera.zoom);
            camera.zoom = zoom;
        }
        if hit.dragged_by(PointerButton::Middle) && camera.zoom > 1.0 {
            camera.offset += hit.drag_delta() / radius;
        }
        if camera.offset.length() > camera.zoom - 1.0 {
            camera.offset = camera.offset.normalized() * (camera.zoom - 1.0);
        }
        ctx.data_mut(|data| data.insert_temp(id, camera));
        Self {
            center: rect.center() + camera.offset * radius,
            radius: radius * camera.zoom,
            zoom: camera.zoom,
        }
    }

    fn screen(&self, value: Complex64) -> Pos2 {
        let coordinate = |value: f64| value.clamp(-100.0, 100.0) as f32;
        self.center + vec2(coordinate(value.re), -coordinate(value.im)) * self.radius
    }

    pub fn nearest(
        &self,
        pointer: Pos2,
        points: impl Iterator<Item = TracePoint>,
    ) -> Option<TracePoint> {
        points
            .map(|point| (self.screen(point.value).distance_sq(pointer), point))
            .filter(|(d, _)| *d < 18.0_f32.powi(2))
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, point)| point)
    }

    pub fn grid(&self, painter: &Painter, ink: Color32) {
        let grid = Stroke::new(1.0, ink.gamma_multiply(0.36));
        painter.circle_stroke(self.center, self.radius, grid);
        painter.line_segment(
            [
                self.center - vec2(self.radius, 0.0),
                self.center + vec2(self.radius, 0.0),
            ],
            grid,
        );
        let values = self.grid_values();
        for &factor in values {
            let center = Complex64::new(factor / (1.0 + factor), 0.0);
            painter.circle_stroke(
                self.screen(center),
                self.radius / (1.0 + factor as f32),
                grid,
            );
        }
        self.reactance_grid(painter, ink, grid, values);
        self.resistance_labels(painter, ink);
    }

    fn reactance_grid(&self, painter: &Painter, ink: Color32, stroke: Stroke, values: &[f64]) {
        let resolution = (self.zoom.sqrt() * 40.0) as u32;
        let clip = painter.clip_rect();
        let rect = Rect::from_min_max(clip.min + vec2(24.0, 29.0), clip.max - vec2(24.0, 19.0));
        let mut labels: Vec<Pos2> = Vec::new();
        for reactance in values.iter().flat_map(|&value| [value, -value]) {
            let mut arc: Vec<_> = (0..resolution)
                .map(|step| {
                    let r = f64::from(step) / f64::from(resolution - step);
                    let z = Complex64::new(r, reactance);
                    self.screen((z - 1.0) / (z + 1.0))
                })
                .collect();
            let origin = arc[0];
            let label = if rect.contains(origin) {
                Some(origin)
            } else if self.zoom > 1.0 {
                arc.iter()
                    .copied()
                    .find(|point| rect.contains(*point) && (point.y - self.center.y).abs() >= 22.0)
            } else {
                None
            };
            arc.push(self.screen(Complex64::new(1.0, 0.0)));
            painter.add(Shape::line(arc, stroke));
            if let Some(point) = label
                && (self.zoom > 1.0 || reactance.abs() == 1.0)
                && labels
                    .iter()
                    .all(|p| p.distance_sq(point) >= 28.0_f32.powi(2))
            {
                labels.push(point);
                let sign = if reactance < 0.0 { "−" } else { "+" };
                let offset = -5.0 * reactance.signum() as f32;
                painter.text(
                    point + vec2(0.0, offset),
                    if reactance < 0.0 {
                        Align2::CENTER_TOP
                    } else {
                        Align2::CENTER_BOTTOM
                    },
                    format!("{sign}j{}", reactance.abs()),
                    FontId::monospace(10.0),
                    ink,
                );
            }
        }
    }

    fn grid_values(&self) -> &'static [f64] {
        match self.zoom {
            zoom if zoom >= 6.0 => &[
                0.025, 0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0,
                7.5, 10.0, 15.0, 20.0, 40.0,
            ],
            zoom if zoom >= 2.0 => &[0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0],
            _ => &[0.2, 0.5, 1.0, 2.0, 5.0],
        }
    }

    fn resistance_labels(&self, painter: &Painter, ink: Color32) {
        let font = FontId::monospace(10.0);
        let rect = painter.clip_rect();
        let mut last_x = f32::NEG_INFINITY;
        for resistance in std::iter::once(0.0).chain(self.grid_values().iter().copied()) {
            let point = self.screen(Complex64::new((resistance - 1.0) / (resistance + 1.0), 0.0));
            if point.x - last_x < 25.0 || !(rect.left()..=rect.right()).contains(&point.x) {
                continue;
            }
            last_x = point.x;
            let y = (point.y + 3.0)
                .min(rect.bottom() - 14.0)
                .max(rect.top() + 3.0);
            painter.text(
                pos2(point.x, y),
                Align2::CENTER_TOP,
                resistance.to_string(),
                font.clone(),
                ink,
            );
        }
    }

    pub fn trace(&self, painter: &Painter, data: &[TracePoint], color: Color32) {
        let points = data.iter().map(|point| self.screen(point.value)).collect();
        draw_line(painter, points, color);
    }

    pub fn mark(&self, painter: &Painter, value: Complex64, label: &str, color: Color32) {
        self.mark_sample(painter, value, label, color, true, false);
    }

    pub fn mark_sample(
        &self,
        painter: &Painter,
        value: Complex64,
        label: &str,
        color: Color32,
        exact: bool,
        left: bool,
    ) {
        let point = self.screen(value);
        if exact {
            painter.circle_filled(point, if left { 5.0 } else { 3.2 }, color);
        } else {
            painter.circle_stroke(point, if left { 5.2 } else { 3.6 }, Stroke::new(1.4, color));
        }
        let (offset, alignment) = if left {
            (vec2(-8.0, -4.0), Align2::RIGHT_BOTTOM)
        } else {
            (vec2(6.0, -2.0), Align2::LEFT_BOTTOM)
        };
        painter.text(
            point + offset,
            alignment,
            label,
            FontId::monospace(11.0),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::touchstone::comparison::sample;

    #[test]
    fn off_grid_marker_uses_the_drawn_decibel_scale_and_wraps_phase() {
        let rows = [
            TracePoint {
                index: 0,
                frequency_hz: 100e6,
                value: Complex64::from_polar(0.1, 170_f64.to_radians()),
            },
            TracePoint {
                index: 1,
                frequency_hz: 150e6,
                value: Complex64::from_polar(1.0, -170_f64.to_radians()),
            },
        ];
        let marker = sample(&rows, 125e6).unwrap();
        assert!((Projection::Decibels.marker_value(marker) + 10.0).abs() < 1e-12);
        assert!((Projection::Magnitude.marker_value(marker) - 0.55).abs() < 1e-12);
        assert!((Projection::Phase.marker_value(marker).abs() - 180.0).abs() < 1e-12);
        assert!((Projection::Phase.difference(-170.0, 170.0) - 20.0).abs() < 1e-12);
    }
}
