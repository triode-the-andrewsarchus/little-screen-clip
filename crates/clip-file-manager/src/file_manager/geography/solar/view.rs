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
    flight: Option<FlyIn>,
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
            flight: None,
        }
    }

    pub fn show_ephemeris(&mut self, id: Id, positions: &[[f64; 3]]) {
        self.ephemeris = Some((id, positions.to_vec()));
    }

    pub fn reload_maps(&mut self) {
        self.texture_job = None;
        self.textures_checked = false;
        self.earth_checked = false;
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
        constellations: bool,
    ) -> Option<Selection> {
        let animation = ui.input(|input| input.time);
        let wheel = self.navigate(response);
        if wheel == Wheel::Beyond && self.center != SUN {
            return Some(Selection::Overview);
        }
        let blend = self
            .flight
            .as_ref()
            .map_or(0.0, |flight| flight.ease(animation));
        let arrived = self.advance_flight(response, animation);
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        let primary = body(self.center)?;
        let heliocentric = primary.id == SUN;
        let day = scene_day(time);
        let clock = clock(time, animation);
        let projection = self.projection(rect, animation, primary.id);
        let backdrop = self.backdrop(rect, projection, clock, stars, constellations, blend);
        self.paint_backdrop(ui, &painter, projection, &backdrop);
        let mut guides = painter.clone();
        guides.multiply_opacity(1.0 - blend);
        if heliocentric {
            paint_belts(&guides, projection, self.zoom);
        }
        let camera = projection.camera();
        let mut layout = OrbitLayout {
            painter: &guides,
            marks: vec![Mark::new(primary, projection.center, 0.0, true, self.zoom)],
            day,
        };
        self.paint_orbits(&mut layout, projection, primary.id);
        if wheel == Wheel::In
            && self.flight.is_none()
            && arrived.is_none()
            && let Some(pointer) = response.hover_pos()
        {
            self.start_flight(rect, pointer, &layout.marks, animation);
        }
        let clicked = paint_marks(
            ui,
            &painter,
            &layout,
            response,
            camera,
            clock,
            &self.textures,
        );
        if !heliocentric {
            backdrop.hover_star(response, |point| mark_at(&layout.marks, point).is_none());
        }
        if let Some(id) = arrived {
            let mut camera = camera;
            camera.zoom = world::initial_zoom(id);
            return Some(Selection::Focus {
                id,
                camera: Some(camera),
            });
        }
        clicked
            .filter(|_| self.flight.is_none())
            .map(|id| Selection::Focus { id, camera: None })
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

    fn backdrop(
        &self,
        rect: Rect,
        projection: Projection,
        clock: [f32; 2],
        stars: Option<Arc<sky::Assets>>,
        constellations: bool,
        blend: f32,
    ) -> world::Backdrop {
        let brightness = if self.center == SUN { 0.17 } else { 0.28 };
        let aperture = 0.60 + (1.0 / 1.9 - 0.60) * blend;
        world::Backdrop {
            axes: projection.axes(),
            fov: [rect.width(), rect.height()]
                .map(|s| s / rect.width().min(rect.height()) * aperture),
            sun: ([0.0, 0.0, 1.0], 0.0),
            clock,
            brightness: brightness + (1.0 - brightness) * blend,
            assets: stars,
            constellations,
        }
    }

    fn paint_backdrop(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        projection: Projection,
        sky: &world::Backdrop,
    ) {
        let rect = painter.clip_rect();
        painter.rect_filled(rect, 0, Color32::from_rgb(7, 10, 17));
        if sky.assets.is_none() || gpu::paint_space(ui, rect, sky.clone()).is_err() {
            paint_stars(painter, rect);
        }
        let visible_sun = self.center == SUN
            && body(SUN).is_some_and(|sun| {
                let mark = Mark::new(sun, projection.center, 0.0, true, self.zoom);
                rect.expand(mark.radius * 1.65).contains(projection.center)
            });
        let opening = self
            .opened_at
            .is_some_and(|start| ui.input(|i| i.time) - start < 0.28);
        if opening {
            ui.ctx().request_repaint();
        } else if visible_sun {
            ui.ctx().request_repaint_after(Duration::from_millis(66));
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
            if helio {
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

    fn navigate(&mut self, response: &egui::Response) -> Wheel {
        let factor = map_camera::scroll_zoom(response);
        let middle = self.middle.update(response);
        let pan = response.dragged_by(egui::PointerButton::Primary);
        let cancel = response
            .ctx
            .input(|input| input.key_pressed(egui::Key::Escape));
        if self.flight.is_some() {
            if middle.is_none() && !pan && factor >= 1.0 && !cancel {
                return Wheel::Idle;
            }
            self.flight = None;
        }
        if let Some(delta) = middle {
            self.yaw -= f64::from(delta.x) * 0.008;
            self.elevation = (self.elevation + f64::from(delta.y) * 0.007).clamp(0.04, 1.56);
        } else if pan {
            self.pan += response.drag_delta();
        }
        if let Some(pointer) = response.hover_pos()
            && factor != 1.0
        {
            const MIN_ORBIT_ZOOM: f32 = 0.55;
            let beyond = map_camera::past_zoom_limit(
                f64::from(self.zoom),
                f64::from(MIN_ORBIT_ZOOM),
                factor,
            );
            let next = (self.zoom * factor as f32).clamp(MIN_ORBIT_ZOOM, 80.0);
            let anchor = pointer - response.rect.center();
            self.pan = anchor + (self.pan - anchor) * (next / self.zoom);
            self.zoom = next;
            response.ctx.request_repaint();
            if beyond {
                return Wheel::Beyond;
            }
            if factor > 1.0 {
                return Wheel::In;
            }
        }
        Wheel::Idle
    }

    fn start_flight(&mut self, rect: Rect, pointer: Pos2, marks: &[Mark], now: f64) {
        let Some(mark) = mark_at(marks, pointer) else {
            return;
        };
        let target_radius =
            (world::initial_zoom(mark.body.id) * 0.43) as f32 * rect.width().min(rect.height());
        // Begin from the pointed-at disk, preserving an ordinary zoom into
        // empty orbital space even when a large world is elsewhere on screen.
        if mark.radius < (target_radius * 0.56).max(26.0) {
            return;
        }
        let zoom_factor = (target_radius / mark.radius).powf(1.0 / 1.12);
        let screen_offset = mark.point - rect.center();
        self.flight = Some(FlyIn {
            id: mark.body.id,
            began: now,
            from_zoom: self.zoom,
            zoom_factor,
            screen_offset,
            orbit_offset: screen_offset - self.pan,
        });
    }

    fn advance_flight(&mut self, response: &egui::Response, now: f64) -> Option<Id> {
        let flight = self.flight.as_ref()?;
        let (zoom, pan, arrived) = flight.sample(now);
        self.zoom = zoom;
        self.pan = pan;
        if arrived {
            return self.flight.take().map(|flight| flight.id);
        }
        response.ctx.request_repaint();
        None
    }
}

pub(in crate::file_manager::geography) enum Selection {
    Focus { id: Id, camera: Option<GlobeCamera> },
    Overview,
}

#[derive(PartialEq, Eq)]
enum Wheel {
    Idle,
    In,
    Beyond,
}

struct FlyIn {
    id: Id,
    began: f64,
    from_zoom: f32,
    zoom_factor: f32,
    screen_offset: Vec2,
    orbit_offset: Vec2,
}

impl FlyIn {
    fn ease(&self, now: f64) -> f32 {
        let progress = ((now - self.began) / 0.28).clamp(0.0, 1.0) as f32;
        progress * progress * (3.0 - 2.0 * progress)
    }

    fn sample(&self, now: f64) -> (f32, Vec2, bool) {
        let ease = self.ease(now);
        let scale = self.zoom_factor.powf(ease);
        (
            self.from_zoom * scale,
            self.screen_offset * (1.0 - ease) - self.orbit_offset * scale,
            ease >= 1.0,
        )
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
        let viewport = self.painter.clip_rect();
        let close = (viewport.width().min(viewport.height()) * 0.038).max(22.0);
        if mark.radius < close
            || !viewport.contains_rect(Rect::from_center_size(
                mark.point,
                Vec2::splat(4.0 * mark.radius),
            ))
        {
            return;
        }
        let radius = mark.body.radius.unwrap_or(100.0);
        let power = 0.56;
        let limit = moon_orbit_limit(mark, plane, self.day);
        let mut moons: Vec<_> = satellites(mark.body.id)
            .filter(|moon| moon.radius.unwrap_or(0.0) >= 8.0)
            .collect();
        moons.sort_unstable_by(|a, b| {
            b.radius
                .unwrap_or_default()
                .total_cmp(&a.radius.unwrap_or_default())
        });
        let alpha = (45.0 + ((self.zoom - 2.2) / 1.5).clamp(0.0, 1.0) * 88.0) as u8;
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
                    radius: radius.clamp(1.4, limit * 0.15),
                    primary: false,
                });
            }
        }
    }
}

fn moon_orbit_limit(parent: &Mark, projection: Projection, day: f64) -> f32 {
    let radius = parent.body.semimajor().powf(projection.power);
    let scale = projection.scale;
    let next_planet = satellites(SUN)
        .filter(|other| other.id != parent.body.id && other.kind() == Kind::Planet)
        .map(|other| {
            let point = projection
                .project(other.at(day), projection.orbit_scale(other.semimajor()))
                .0;
            parent.point.distance(point)
        })
        .fold(f32::INFINITY, f32::min);
    // Preserve a local group without colliding with neighboring visible disks.
    let local = (0.20 * radius) as f32 * scale;
    let limit = local.min(next_planet * 0.39);
    if parent.body.id != super::EARTH {
        return limit;
    }
    // Even in an oblique view, keep Earth's Moon well beyond the orbit of Mercury
    // in the compressed heliocentric coordinate system.
    let inner = body(199).and_then(|body| body.orbit).map_or(0.0, |orbit| {
        orbit.0.powf(projection.power) * (1.0 + orbit.1)
    });
    limit.min((0.65 * (radius * 0.983 - inner).max(0.0)) as f32 * scale)
}

#[cfg(test)]
#[test]
fn overview_moon_orbits_stay_outside_mercury_and_leave_room_for_the_earth_disk() {
    let plane = Projection {
        center: Pos2::ZERO,
        scale: 1300.0,
        power: 0.34,
        yaw: [0.0, 1.0],
        elevation: [1.0, 0.0],
    };
    let earth = body(super::EARTH).unwrap();
    let mut largest: f32 = 0.0;
    for day in (0..365).step_by(8) {
        let earth_scale = plane.orbit_scale(earth.semimajor());
        let (point, depth) = plane.project(earth.at(f64::from(day)), earth_scale);
        let mark = Mark::new(earth, point, depth, false, 6.0);
        let limit = moon_orbit_limit(&mark, plane, f64::from(day));
        let mercury = body(199).unwrap().orbit.unwrap();
        let aphelion = mercury.0.powf(plane.power) * (1.0 + mercury.1);
        assert!(point.distance(plane.center) - limit > aphelion as f32 * plane.scale);
        largest = largest.max(limit / mark.radius);
    }
    assert!(largest > 1.7);
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
    if !painter.clip_rect().intersects(rect) {
        return;
    }
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
    if gpu::paint_world_clipped(ui, rect, painter.clip_rect(), frame).is_err() {
        painter.circle_filled(mark.point, mark.radius, mark.body.color());
    }
}

fn mark_at(marks: &[Mark], pointer: Pos2) -> Option<&Mark> {
    marks
        .iter()
        .filter(|mark| mark.point.distance(pointer) <= mark.radius.max(6.0))
        .min_by(|a, b| {
            let a_in_front = pointer.distance_sq(a.point) <= a.radius * a.radius;
            let b_in_front = pointer.distance_sq(b.point) <= b.radius * b.radius;
            b_in_front
                .cmp(&a_in_front)
                .then_with(|| a.depth.total_cmp(&b.depth))
                .then_with(|| {
                    pointer
                        .distance_sq(a.point)
                        .total_cmp(&pointer.distance_sq(b.point))
                })
        })
}

fn paint_marks(
    ui: &egui::Ui,
    painter: &egui::Painter,
    layout: &OrbitLayout<'_>,
    response: &egui::Response,
    camera: GlobeCamera,
    clock: [f32; 2],
    images: &HashMap<Id, sky::Texture>,
) -> Option<Id> {
    let mut marks: Vec<_> = layout.marks.iter().collect();
    marks.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let mut hit = pointer.and_then(|point| mark_at(&layout.marks, point).map(|mark| mark.body.id));
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
        let box_rect = paint_label(layout.painter, response.rect, mark, &mut occupied);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_flight_keeps_its_anchor_and_hands_off_centered() {
        let flight = FlyIn {
            id: 599,
            began: 1.0,
            from_zoom: 4.0,
            zoom_factor: 2.0,
            screen_offset: vec2(130.0, -55.0),
            orbit_offset: vec2(-240.0, 156.0),
        };
        let (start_zoom, start_pan, started) = flight.sample(1.0);
        assert!(!started);
        assert_eq!(start_pan + flight.orbit_offset, flight.screen_offset);
        assert_eq!(start_zoom, 4.0);
        let (zoom, pan, arrived) = flight.sample(1.3);
        assert!(arrived);
        assert_eq!(zoom, 8.0);
        assert_eq!(pan + flight.orbit_offset * (zoom / start_zoom), Vec2::ZERO);
    }

    #[test]
    fn pointer_picks_the_nearer_world_when_disks_overlap() {
        let here = Pos2::ZERO;
        let distant = Mark::new(body(599).unwrap(), here, 1.0, false, 1.0);
        let nearer = Mark::new(body(501).unwrap(), here, 0.0, false, 1.0);
        assert_eq!(mark_at(&[distant, nearer], here).unwrap().body.id, 501);
    }
}
