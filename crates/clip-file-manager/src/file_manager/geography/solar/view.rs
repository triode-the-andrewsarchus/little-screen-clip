use super::{Body, Id, Kind, SUN, assets, body, clock, satellites, scene_day, sunlight, world};
use crate::file_manager::geography::{gpu, map_camera, sky};
use chrono::{DateTime, Utc};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use little_clip_core::{
    geography::basemap::{BasemapAssets, GlobeCamera},
    jobs::task::Task,
};
use std::{
    collections::HashMap,
    f64::consts::TAU,
    path::Path,
    sync::{Arc, OnceLock},
    time::Duration,
};

pub(in crate::file_manager::geography) struct System {
    pub center: Id,
    yaw: f64,
    elevation: f64,
    zoom: f32,
    pan: Vec2,
    middle: super::super::map_camera::MiddleDrag,
    ephemeris: Option<(Id, Vec<[f64; 3]>)>,
    opened_at: Option<f64>,
    textures: HashMap<Id, sky::Texture>,
    texture_job: Option<Task<HashMap<Id, sky::Texture>>>,
    textures_checked: bool,
    earth_checked: bool,
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
            textures: HashMap::new(),
            texture_job: None,
            textures_checked: false,
            earth_checked: false,
        }
    }

    pub fn show_ephemeris(&mut self, id: Id, positions: &[[f64; 3]]) {
        self.ephemeris = Some((id, positions.to_vec()));
    }

    pub fn poll_maps(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        earth: Option<Arc<BasemapAssets>>,
    ) {
        if let Some(maps) = self.texture_job.as_ref().and_then(Task::poll) {
            self.texture_job = None;
            self.textures_checked = true;
            if let Ok(maps) = maps {
                self.textures = maps;
            }
        }
        let new_earth =
            earth.as_ref().is_some_and(|earth| earth.ne2.is_some()) && !self.earth_checked;
        if self.texture_job.is_none() && (!self.textures_checked || new_earth) {
            self.earth_checked |= new_earth;
            let root = root.to_owned();
            self.texture_job = Some(Task::start(
                super::super::super::geography_downloader::asset_workers(),
                Duration::from_secs(30),
                move |cancel| {
                    assets::overview_maps(
                        &root,
                        earth.as_ref().and_then(|a| a.ne2.as_ref()),
                        &cancel,
                    )
                },
            ));
        }
        if self.texture_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        time: DateTime<Utc>,
        stars: Option<Arc<sky::Assets>>,
    ) -> Option<Id> {
        self.navigate(response);
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        let primary = body(self.center)?;
        let heliocentric = primary.id == SUN;
        let day = scene_day(time);
        let animation = ui.input(|input| input.time);
        let clock = clock(time, animation);
        let projection = self.projection(rect, animation, primary.id);
        self.paint_backdrop(ui, &painter, projection, clock, stars);
        if heliocentric {
            paint_belts(&painter, projection, self.zoom);
        }
        let camera = projection.camera();
        let mut layout = OrbitLayout {
            painter: &painter,
            marks: vec![Mark::new(primary, projection.center, 0.0, true, self.zoom)],
            day,
            zoom: self.zoom,
        };
        self.paint_orbits(&mut layout, projection, primary.id);
        paint_marks(
            ui,
            &painter,
            response,
            layout.marks,
            camera,
            clock,
            &self.textures,
        )
    }

    fn projection(&mut self, rect: Rect, animation: f64, center: Id) -> Projection {
        let power = if center == SUN { 0.34 } else { 0.44 };
        let extent = satellites(center)
            .filter_map(|body| body.orbit)
            .map(|orbit| orbit.0.powf(power) * (1.0 + orbit.1))
            .fold(1.0, f64::max);
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_el, cos_el) = self.elevation.sin_cos();
        let opening_scale = if center == SUN {
            let started = *self.opened_at.get_or_insert(animation);
            let progress = ((animation - started) / 0.28).clamp(0.0, 1.0);
            1.0 + 1.4 * (1.0 - progress as f32).powi(3)
        } else {
            1.0
        };
        // An oblique orbital plane has more room horizontally.
        let span = rect.width().min(rect.height() * 1.32);
        Projection {
            center: rect.center() + self.pan,
            scale: span * 0.45 * self.zoom * opening_scale / extent as f32,
            power,
            yaw: [sin_yaw, cos_yaw],
            elevation: [sin_el, cos_el],
        }
    }

    fn paint_backdrop(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        projection: Projection,
        clock: [f32; 2],
        stars: Option<Arc<sky::Assets>>,
    ) {
        let rect = painter.clip_rect();
        painter.rect_filled(rect, 0, Color32::from_rgb(7, 10, 17));
        if stars.is_none()
            || gpu::paint_space(
                ui,
                rect,
                world::Backdrop {
                    axes: projection.axes(),
                    fov: [rect.width(), rect.height()]
                        .map(|s| s / rect.width().min(rect.height()) * 0.60),
                    sun: ([0.0, 0.0, 1.0], 0.0),
                    clock,
                    brightness: if self.center == SUN { 0.17 } else { 0.28 },
                    assets: stars,
                },
            )
            .is_err()
        {
            paint_stars(painter, rect);
        }
        let visible_sun = self.center == SUN && rect.contains(projection.center);
        if visible_sun
            || self
                .opened_at
                .is_some_and(|start| ui.input(|i| i.time) - start < 0.28)
        {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }

    fn paint_orbits(&self, layout: &mut OrbitLayout<'_>, projection: Projection, primary: Id) {
        let helio = primary == SUN;
        let bodies = satellites(primary);
        let dense = bodies.clone().count() > 24;
        for satellite in bodies {
            let scale = projection.orbit_scale(satellite.semimajor());
            let opacity = match (helio, satellite.kind(), dense && satellite.radius.is_none()) {
                (true, Kind::DwarfPlanet, _) => 84,
                (true, _, _) => 165,
                (_, _, true) => 32,
                _ => 100,
            };
            let samples = self
                .ephemeris
                .as_ref()
                .filter(|(id, _)| *id == satellite.id)
                .map(|(_, samples)| samples.as_slice());
            layout.orbit(
                satellite,
                projection,
                scale,
                opacity,
                if dense { 40 } else { 96 },
                samples,
            );
            let (point, depth) = projection.project(satellite.at(layout.day), scale);
            let mark = Mark::new(satellite, point, depth, false, self.zoom);
            if helio && self.zoom > 2.2 && layout.painter.clip_rect().expand(30.0).contains(point) {
                layout.moon_system(&mark, projection);
            }
            if layout
                .painter
                .clip_rect()
                .expand(mark.radius * 3.5)
                .contains(point)
            {
                layout.marks.push(mark);
            }
        }
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
    fn axes(self) -> [[f32; 3]; 3] {
        let [sy, cy] = self.yaw;
        let [se, ce] = self.elevation;
        [
            [cy, -sy, 0.0],
            [se * sy, se * cy, ce],
            [-ce * sy, -ce * cy, se],
        ]
        .map(|axis| axis.map(|v| v as f32))
    }

    fn camera(self) -> GlobeCamera {
        let [x, y, z] = self.axes()[2].map(f64::from);
        let mut camera = map_camera::lunar_camera();
        camera.center_lat = z.asin().to_degrees();
        camera.center_lon = y.atan2(x).to_degrees();
        camera
    }

    fn orbit_scale(self, semimajor: f64) -> f32 {
        (scale_distance(semimajor, self.power) / semimajor) as f32 * self.scale
    }

    fn project(self, [x, y, z]: [f64; 3], scale: f32) -> (Pos2, f64) {
        let [sy, cy] = self.yaw;
        let [se, ce] = self.elevation;
        let (x, y) = (cy * x - sy * y, sy * x + cy * y);
        let offset = vec2(x as f32, -(se * y + ce * z) as f32) * scale;
        (self.center + offset, (ce * y - se * z) * f64::from(scale))
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
    fn new(body: &'static Body, point: Pos2, depth: f64, primary: bool, zoom: f32) -> Self {
        let radius = body
            .radius
            .map_or(1.6, |km| (1.5 + (km / 8.0).powf(0.20) * 1.2) as f32);
        Self {
            body,
            point,
            depth,
            radius: radius * zoom.powf(1.12),
            primary,
        }
    }
}

struct OrbitLayout<'a> {
    painter: &'a egui::Painter,
    marks: Vec<Mark>,
    day: f64,
    zoom: f32,
}

impl OrbitLayout<'_> {
    fn orbit(
        &self,
        body: &Body,
        projection: Projection,
        scale: f32,
        opacity: u8,
        segments: u32,
        samples: Option<&[[f64; 3]]>,
    ) {
        let color = body.color();
        let points = (0..=segments)
            .map(|i| body.orbit_point(f64::from(i) / f64::from(segments) * TAU))
            .map(|point| projection.project(point, scale).0)
            .collect();
        self.painter.add(egui::Shape::line(
            points,
            Stroke::new(1.0, color.gamma_multiply(f32::from(opacity) / 255.0)),
        ));
        if let Some(samples) = samples {
            let points = samples
                .iter()
                .map(|point| projection.project(*point, scale).0)
                .collect();
            self.painter
                .add(egui::Shape::line(points, Stroke::new(1.45, color)));
        }
    }

    fn moon_system(&mut self, mark: &Mark, plane: Projection) {
        let radius = mark.body.radius.unwrap_or(100.0);
        let power = 0.56;
        let projection = Projection {
            center: mark.point,
            scale: mark.radius * 1.5 / radius.powf(power) as f32,
            power,
            ..plane
        };
        let mut moons: Vec<_> = satellites(mark.body.id)
            .filter(|moon| moon.radius.unwrap_or(0.0) >= 8.0)
            .collect();
        moons.sort_unstable_by(|a, b| {
            b.radius
                .unwrap_or_default()
                .total_cmp(&a.radius.unwrap_or_default())
        });
        let alpha = (((self.zoom - 2.2) / 1.1).clamp(0.05, 1.0) * 110.0) as u8;
        for moon in moons.into_iter().take(12) {
            let scale = projection.orbit_scale(moon.semimajor());
            self.orbit(moon, projection, scale, alpha, 64, None);
            let (point, depth) = projection.project(moon.at(self.day), scale);
            if self.painter.clip_rect().expand(15.0).contains(point) {
                let radius = ((moon.radius.unwrap_or_default() / radius).powf(0.68)
                    * f64::from(mark.radius)
                    * 2.0) as f32;
                self.marks.push(Mark {
                    body: moon,
                    point,
                    depth: mark.depth + depth,
                    radius: radius.max(1.4),
                    primary: false,
                });
            }
        }
    }
}

fn paint_icon(
    ui: &egui::Ui,
    painter: &egui::Painter,
    mark: &Mark,
    mut camera: GlobeCamera,
    clock: [f32; 2],
    images: &HashMap<Id, sky::Texture>,
) {
    let rings = world::bands(mark.body.id);
    if mark.radius < 2.5 && rings.is_empty() && mark.body.id != SUN {
        painter.circle_filled(mark.point, mark.radius, mark.body.color());
        return;
    }
    let size_ratio = rings
        .last()
        .map_or(if mark.body.id == SUN { 1.8 } else { 1.08 }, |band| {
            (f64::from(band[1]) / mark.body.radius.unwrap_or(1.0)) as f32 * 1.08
        });
    let rect = Rect::from_center_size(mark.point, Vec2::splat(mark.radius * size_ratio * 2.0));
    camera.zoom = 1.0 / (0.86 * f64::from(size_ratio));
    let mut frame = world::Frame::new(
        mark.body,
        camera,
        rect.size().into(),
        images.get(&mark.body.id).cloned(),
    );
    frame.clock = clock;
    frame.sun = sunlight(mark.body, f64::from(clock[0])).0;
    frame.ring_emphasis = (20.0 / mark.radius).clamp(1.0, 2.8);
    if gpu::paint_world(ui, rect, frame).is_err() {
        painter.circle_filled(mark.point, mark.radius, mark.body.color());
    }
}

fn paint_marks(
    ui: &egui::Ui,
    painter: &egui::Painter,
    response: &egui::Response,
    mut marks: Vec<Mark>,
    camera: GlobeCamera,
    clock: [f32; 2],
    images: &HashMap<Id, sky::Texture>,
) -> Option<Id> {
    marks.sort_by(|a, b| b.depth.total_cmp(&a.depth));
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
        paint_icon(ui, painter, mark, camera, clock, images);
    }
    let mut occupied: Vec<Rect> = Vec::new();
    marks.sort_by(|a, b| {
        (b.primary, hit == Some(b.body.id))
            .cmp(&(a.primary, hit == Some(a.body.id)))
            .then_with(|| b.radius.total_cmp(&a.radius))
    });
    for mark in &marks {
        if mark.body.kind() == Kind::Moon
            && !mark.primary
            && hit != Some(mark.body.id)
            && (mark.body.radius.unwrap_or(0.0) <= 38.0 || mark.radius < 2.5)
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

fn paint_belts(painter: &egui::Painter, projection: Projection, zoom: f32) {
    // Fixed orbital dust positions avoid flicker while panning. Point size grows
    // slowly with zoom so individual objects stay legible.
    static PARTICLES: OnceLock<Vec<([f64; 3], f64, u8, f32)>> = OnceLock::new();
    let particles = PARTICLES.get_or_init(|| {
        [(2.1, 3.35, 0.45, 2200), (39.0, 49.0, 4.0, 3200)]
            .into_iter()
            .enumerate()
            .flat_map(|(belt, (inner, outer, height, count))| {
                (1..=count).map(move |i| {
                    let seed = i * 13 + belt as u32 * 100_003;
                    let a = inner + (outer - inner) * noise(seed + 1);
                    let (sin, cos) = (TAU * noise(seed + 2)).sin_cos();
                    let z = height * (noise(seed + 3) + noise(seed + 7) - 1.0);
                    ([a * cos, a * sin, z], a, belt as u8, noise(seed + 4) as f32)
                })
            })
            .collect()
    });
    let colors = [
        Color32::from_rgb(139, 117, 92),
        Color32::from_rgb(102, 125, 156),
    ];
    let clip = painter.clip_rect();
    let scale = (0.8 + 0.72 * zoom.ln().max(0.0)).min(3.8);
    for &(position, a, belt, bright) in particles {
        let point = projection.project(position, projection.orbit_scale(a)).0;
        if clip.contains(point) {
            painter.circle_filled(
                point,
                scale * (0.52 + 0.66 * bright),
                colors[belt as usize].gamma_multiply(0.22 + 0.64 * bright),
            );
        }
    }
}
