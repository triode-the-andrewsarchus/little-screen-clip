use super::{Body, Id, SUN, body, satellites};
use chrono::{DateTime, Utc};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use std::{f64::consts::TAU, sync::OnceLock};

pub(in crate::file_manager::geography) struct System {
    pub center: Id,
    yaw: f64,
    elevation: f64,
    zoom: f32,
    pan: Vec2,
    middle: super::super::map_camera::MiddleDrag,
    ephemeris: Option<(Id, Vec<[f64; 3]>)>,
    opened_at: Option<f64>,
}

impl System {
    pub fn new(center: Id) -> Self {
        Self {
            center,
            yaw: -0.2,
            elevation: 0.68,
            zoom: 1.0,
            pan: Vec2::ZERO,
            middle: Default::default(),
            ephemeris: None,
            opened_at: None,
        }
    }

    pub fn show_ephemeris(&mut self, id: Id, positions: &[[f64; 3]]) {
        self.ephemeris = Some((id, positions.to_vec()));
    }

    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        time: DateTime<Utc>,
    ) -> Option<Id> {
        self.navigate(response);
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, Color32::from_rgb(7, 10, 17));
        paint_stars(&painter, rect);
        let primary = body(self.center)?;
        let heliocentric = primary.id == SUN;
        let moons = satellites(primary.id);
        let power = if heliocentric { 0.43 } else { 0.51 };
        let extent = moons
            .clone()
            .filter_map(|body| body.orbit)
            .map(|orbit| orbit.0.powf(power) * (1.0 + orbit.1))
            .fold(1.0, f64::max);
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_el, cos_el) = self.elevation.sin_cos();
        let opening_scale = if heliocentric {
            let time = ui.input(|input| input.time);
            let started = *self.opened_at.get_or_insert(time);
            let progress = ((time - started) / 0.28).clamp(0.0, 1.0);
            if progress < 1.0 {
                ui.ctx().request_repaint();
            }
            1.0 + 1.4 * (1.0 - progress as f32).powi(3)
        } else {
            1.0
        };
        let projection = Projection {
            center: rect.center() + self.pan,
            scale: rect.size().min_elem() * 0.45 * self.zoom * opening_scale / extent as f32,
            power,
            yaw: [sin_yaw, cos_yaw],
            elevation: [sin_el, cos_el],
        };
        if heliocentric {
            paint_belts(&painter, projection);
        }
        let day = (time.timestamp() as f64 - 946_728_000.0) / 86_400.0;
        let mut marks = vec![Mark::new(primary, projection.center, 0.0, true)];
        let dense = moons.clone().count() > 24;
        for satellite in moons {
            let scale = projection.orbit_scale(satellite.semimajor());
            let color = satellite.color();
            let opacity: u8 = if heliocentric {
                match satellite.kind() {
                    super::Kind::DwarfPlanet => 84,
                    _ => 165,
                }
            } else if dense && satellite.radius.is_none() {
                32
            } else {
                100
            };
            let segments = if dense { 40 } else { 96 };
            let points: Vec<_> = (0..=segments)
                .map(|i| satellite.orbit_point(f64::from(i) / f64::from(segments) * TAU))
                .map(|point| projection.project(point, scale).0)
                .collect();
            painter.add(egui::Shape::line(
                points,
                Stroke::new(1.0, color.gamma_multiply(f32::from(opacity) / 255.0)),
            ));
            if let Some((_, samples)) = self
                .ephemeris
                .as_ref()
                .filter(|(id, _)| *id == satellite.id)
            {
                let points = samples
                    .iter()
                    .map(|point| projection.project(*point, scale).0)
                    .collect();
                painter.add(egui::Shape::line(points, Stroke::new(1.45, color)));
            }
            let (point, depth) = projection.project(satellite.at(day), scale);
            if rect.expand(10.0).contains(point) {
                marks.push(Mark::new(satellite, point, depth, false));
            }
        }
        paint_marks(&painter, response, marks, heliocentric)
    }

    fn navigate(&mut self, response: &egui::Response) {
        if let Some(delta) = self.middle.update(response) {
            self.yaw -= f64::from(delta.x) * 0.008;
            self.elevation = (self.elevation + f64::from(delta.y) * 0.007).clamp(0.04, 1.56);
        } else if response.dragged_by(egui::PointerButton::Primary) {
            self.pan += response.drag_delta();
        }
        if let Some(pointer) = response.hover_pos() {
            let scroll = response.ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                let next = (self.zoom * (scroll * 0.004).exp()).clamp(0.55, 18.0);
                let anchor = pointer - response.rect.center();
                self.pan = anchor + (self.pan - anchor) * (next / self.zoom);
                self.zoom = next;
                response.ctx.request_repaint();
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Projection {
    center: Pos2,
    scale: f32,
    power: f64,
    yaw: [f64; 2],
    elevation: [f64; 2],
}

fn scale_distance(distance: f64, power: f64) -> f64 {
    distance.powf(power)
}

impl Projection {
    fn orbit_scale(self, semimajor: f64) -> f32 {
        (scale_distance(semimajor, self.power) / semimajor) as f32 * self.scale
    }

    fn project(self, [x, y, z]: [f64; 3], scale: f32) -> (Pos2, f64) {
        let [sy, cy] = self.yaw;
        let [se, ce] = self.elevation;
        let (x, y) = (cy * x - sy * y, sy * x + cy * y);
        let offset = vec2(x as f32, -(se * y + ce * z) as f32) * scale;
        (self.center + offset, ce * y - se * z)
    }
}

struct Mark {
    body: &'static Body,
    point: Pos2,
    depth: f64,
    radius: f32,
    primary: bool,
}

impl Mark {
    fn new(body: &'static Body, point: Pos2, depth: f64, primary: bool) -> Self {
        let radius = body
            .radius
            .map_or(1.6, |km| (1.5 + (km / 8.0).powf(0.20) * 1.2) as f32);
        Self {
            body,
            point,
            depth,
            radius,
            primary,
        }
    }
}

fn paint_marks(
    painter: &egui::Painter,
    response: &egui::Response,
    mut marks: Vec<Mark>,
    solar: bool,
) -> Option<Id> {
    marks.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let mut hit = pointer.and_then(|point| {
        marks
            .iter()
            .filter(|mark| mark.point.distance(point) <= mark.radius.max(6.0))
            .min_by(|a, b| {
                a.point
                    .distance_sq(point)
                    .total_cmp(&b.point.distance_sq(point))
            })
            .map(|mark| mark.body.id)
    });
    for mark in &marks {
        let color = mark.body.color();
        if mark.primary {
            painter.circle_filled(mark.point, mark.radius * 2.5, color.gamma_multiply(0.12));
            painter.circle_filled(mark.point, mark.radius * 1.65, color.gamma_multiply(0.24));
        }
        painter.circle_filled(mark.point, mark.radius, color);
    }
    let mut occupied: Vec<Rect> = Vec::new();
    marks.sort_by(|a, b| {
        (b.primary, hit == Some(b.body.id))
            .cmp(&(a.primary, hit == Some(a.body.id)))
            .then_with(|| b.radius.total_cmp(&a.radius))
    });
    for mark in &marks {
        if !solar
            && !mark.primary
            && hit != Some(mark.body.id)
            && mark.body.radius.unwrap_or(0.0) <= 38.0
        {
            continue;
        }
        let box_rect = paint_label(painter, response.rect, mark, &mut occupied);
        if hit.is_none() && pointer.zip(box_rect).is_some_and(|(p, r)| r.contains(p)) {
            hit = Some(mark.body.id);
        }
    }
    if hit.is_some() {
        response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    hit.filter(|_| response.clicked())
}

fn paint_label(
    painter: &egui::Painter,
    viewport: Rect,
    mark: &Mark,
    occupied: &mut Vec<Rect>,
) -> Option<Rect> {
    let color = mark.body.color();
    let galley =
        painter.layout_no_wrap(mark.body.name.to_owned(), FontId::proportional(13.0), color);
    let size = galley.size() + vec2(12.0, 4.0);
    let gap = mark.radius + 5.0;
    let offsets = [
        vec2(gap, -gap - size.y),
        vec2(-gap - size.x, -gap - size.y),
        vec2(gap, gap),
        vec2(-gap - size.x, gap),
        vec2(gap, -size.y * 0.5),
        vec2(-gap - size.x, -size.y * 0.5),
    ];
    let rect = offsets
        .into_iter()
        .map(|offset| Rect::from_min_size(mark.point + offset, size))
        .find(|r| {
            viewport.contains_rect(*r) && !occupied.iter().any(|other| other.intersects(*r))
        })?;
    occupied.push(rect.expand(2.0));
    let lead = pos2(
        mark.point.x.clamp(rect.left(), rect.right()),
        mark.point.y.clamp(rect.top(), rect.bottom()),
    );
    painter.line_segment(
        [mark.point, lead],
        Stroke::new(0.8, color.gamma_multiply(0.53)),
    );
    painter.rect(
        rect,
        2.0,
        Color32::from_rgb(12, 16, 25),
        Stroke::new(0.8, color.gamma_multiply(0.67)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(6.0, 2.0), galley, color);
    Some(rect)
}

fn noise(i: u32) -> f64 {
    let hash = i.wrapping_mul(0x9e37_79b9) ^ i.rotate_left(13);
    f64::from(hash.wrapping_mul(0x85eb_ca6b) >> 8) / f64::from(1 << 24)
}

pub(in crate::file_manager::geography) fn paint_stars(painter: &egui::Painter, rect: Rect) {
    static POSITIONS: OnceLock<Vec<(f32, f32, u8)>> = OnceLock::new();
    let stars = POSITIONS.get_or_init(|| {
        (1..650)
            .map(|i| {
                let bright = 25.0 + 128.0 * noise(i * 5 + 3).powi(5);
                (
                    noise(i * 5 + 1) as f32,
                    noise(i * 5 + 2) as f32,
                    bright as u8,
                )
            })
            .collect()
    });
    for &(x, y, bright) in stars {
        let point = rect.min + vec2(x * rect.width(), y * rect.height());
        painter.circle_filled(
            point,
            if bright > 110 { 1.0 } else { 0.6 },
            Color32::from_rgb(bright, bright, bright.saturating_add(18)),
        );
    }
}

fn paint_belts(painter: &egui::Painter, projection: Projection) {
    for (index, [inner, outer, height]) in [[2.1, 3.35, 0.45], [39.0, 49.0, 4.0]]
        .into_iter()
        .enumerate()
    {
        let color = if index == 0 {
            Color32::from_rgb(76, 70, 59)
        } else {
            Color32::from_rgb(52, 63, 80)
        };
        for i in 1..=450 {
            let a = inner + (outer - inner) * noise(i * 13 + 1);
            let (sin, cos) = (TAU * noise(i * 13 + 2)).sin_cos();
            let z = height * (noise(i * 13 + 3) - 0.5);
            let point = projection
                .project([a * cos, a * sin, z], projection.orbit_scale(a))
                .0;
            if painter.clip_rect().contains(point) {
                painter.circle_filled(
                    point,
                    0.7,
                    color.gamma_multiply(0.42 + 0.58 * noise(i * 13 + 4) as f32),
                );
            }
        }
    }
}
