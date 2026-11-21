use super::{
    AU_KM, Body, Id, Kind, SUN, assets, body, clock, position_au, satellites, scene_day, sunlight,
    trajectory::Trajectory, world,
};
use crate::file_manager::geography::{gpu, map_camera, sky, timeline};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use little_clip_core::{
    geography::basemap::{BasemapAssets, GlobeCamera},
    jobs::task::Task,
};
use std::{
    f64::consts::TAU,
    path::Path,
    sync::{Arc, OnceLock},
    time::Duration,
};

pub(in crate::file_manager::geography) struct System {
    pub center: Id,
    yaw: f64,
    elevation: f64,
    zoom: f64,
    pan: [f64; 2],
    zoom_anchor: Option<ZoomAnchor>,
    middle: super::super::map_camera::MiddleDrag,
    ephemeris: Option<(Id, Vec<[f64; 3]>)>,
    trajectory: Option<Arc<Trajectory>>,
    fit_requested: bool,
    texture_job: Option<Task<()>>,
    textures_checked: bool,
    earth_checked: bool,
    flight: Option<Flight>,
}

impl System {
    pub fn new(center: Id) -> Self {
        Self {
            center,
            yaw: -0.2,
            elevation: 0.68,
            zoom: 1.0,
            pan: [0.0; 2],
            zoom_anchor: None,
            middle: Default::default(),
            ephemeris: None,
            trajectory: None,
            fit_requested: false,
            texture_job: None,
            textures_checked: false,
            earth_checked: false,
            flight: None,
        }
    }

    pub fn departing(
        target: &'static Body,
        camera: GlobeCamera,
        rect: Rect,
        now: f64,
        day: f64,
    ) -> Self {
        let mut system = Self::new(SUN);
        system.yaw = camera.center_lon.to_radians() - TAU * 0.25;
        system.elevation = camera.center_lat.to_radians();
        let scale = system.projection(rect).scale;
        let physical_radius = target.radius.unwrap_or(3.0) * scale / AU_KM;
        let from_zoom = (focus_threshold(rect) * 0.98 / physical_radius).min(MAX_ORBIT_ZOOM);
        system.zoom = from_zoom;
        let projection = system.projection(rect);
        let position = position_au(target, day);
        let [x, y, _] = projection.relative(position);
        let mark = Mark::new(
            target,
            projection.project(position),
            false,
            projection.scale / AU_KM,
        );
        let target_radius = camera.scale_px(rect.width(), rect.height());
        system.flight = Some(Flight {
            id: target.id,
            began: now,
            from_zoom,
            zoom_factor: target_radius / f64::from(mark.physical_radius),
            from_radius: f64::from(mark.radius),
            target_radius,
            screen_offset: [0.0; 2],
            orbit_offset: [x, y],
            departure: Some(camera),
        });
        system
    }

    pub fn show_ephemeris(&mut self, id: Id, positions: &[[f64; 3]]) {
        self.ephemeris = Some((id, positions.to_vec()));
    }

    pub fn set_trajectory(&mut self, trajectory: Option<Arc<Trajectory>>) {
        self.trajectory = trajectory;
    }

    pub fn fit_trajectory(&mut self) {
        self.fit_requested = true;
    }

    fn apply_trajectory_fit(&mut self, rect: Rect) {
        if !std::mem::take(&mut self.fit_requested) {
            return;
        }
        let Some(trajectory) = self.trajectory.as_ref() else {
            return;
        };
        self.flight = None;
        self.zoom_anchor = None;
        self.pan = [0.0; 2];
        self.zoom = 1.0;
        let projection = self.projection(rect);
        let (mut min, mut max) = ([0.0_f64; 2], [0.0_f64; 2]);
        for position in trajectory.positions() {
            let [x, y, _] = projection.relative(position);
            for (i, offset) in [x, y].into_iter().enumerate() {
                min[i] = min[i].min(offset);
                max[i] = max[i].max(offset);
            }
        }
        let [width, height] = [rect.width(), rect.height()].map(|size| f64::from(size) * 0.78);
        let path_share = ((max[0] - min[0]) / width).max((max[1] - min[1]) / height);
        self.zoom = path_share.recip().clamp(MIN_ORBIT_ZOOM, MAX_ORBIT_ZOOM);
        self.pan = std::array::from_fn(|axis| -0.5 * (min[axis] + max[axis]) * self.zoom);
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
        cache: &Arc<assets::MapCache>,
    ) {
        if self.texture_job.as_ref().and_then(Task::poll).is_some() {
            self.texture_job = None;
            self.textures_checked = true;
        }
        let new_earth =
            earth.as_ref().is_some_and(|earth| earth.ne2.is_some()) && !self.earth_checked;
        if self.texture_job.is_none() && (!self.textures_checked || new_earth) {
            self.earth_checked |= new_earth;
            let root = root.to_owned();
            let cache = Arc::clone(cache);
            self.texture_job = Some(Task::start(
                assets::preview_workers(),
                Duration::from_secs(30),
                move |cancel| {
                    assets::overview_maps(
                        &cache,
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
        preview: timeline::Preview,
        stars: Option<Arc<sky::Assets>>,
        constellations: bool,
        maps: &assets::MapCache,
    ) -> Option<Selection> {
        let animation = ui.input(|input| input.time);
        self.apply_trajectory_fit(response.rect);
        let wheel = self.navigate(response);
        if wheel == Wheel::Beyond && self.center != SUN {
            return Some(Selection::Overview);
        }
        let flight = self.advance_flight(response, animation);
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        let primary = body(self.center)?;
        let heliocentric = primary.id == SUN;
        let day = scene_day(preview.time);
        let clock = clock(preview.time, animation);
        let projection = self.projection(rect);
        let camera = projection.camera();
        let approach = flight.and_then(|(id, sample)| {
            let lens = world::Approach {
                body: body(id)?,
                camera: self.flight.as_ref()?.departure.unwrap_or(GlobeCamera {
                    zoom: APPROACH_ZOOM,
                    ..camera
                }),
                center: rect.center() + vec2(sample.offset[0] as f32, sample.offset[1] as f32),
                radius: sample.radius,
                progress: sample.progress,
            };
            Some((lens, sample.finished))
        });
        let blend = approach
            .as_ref()
            .map_or(0.0, |(lens, _)| lens.progress as f32);
        let mut backdrop = self.backdrop(rect, projection, clock, stars, constellations, blend);
        backdrop.star_trail = preview.star_trail;
        if let Some((lens, _)) = &approach {
            let (direction, radius) = sunlight(lens.body, day);
            backdrop.sun = (direction, radius * blend.powi(3));
            backdrop.axes = lens.pose(rect).axes.map(|axis| axis.map(|v| v as f32));
        }
        self.paint_backdrop(ui, &painter, projection, &backdrop);
        let mut guides = painter.clone();
        guides.multiply_opacity(1.0 - (blend * 2.2).min(1.0));
        let mut layout = OrbitLayout {
            painter: &guides,
            marks: vec![Mark::new(
                primary,
                projection.project([0.0; 3]),
                true,
                projection.scale / self.unit_km(),
            )],
            day,
        };
        self.paint_orbits(&mut layout, projection, primary.id);
        if wheel == Wheel::In
            && self.flight.is_none()
            && approach.is_none()
            && let Some(pointer) = response.hover_pos()
        {
            self.start_flight(rect, pointer, &layout.marks, animation);
        }
        self.zoom_anchor = response
            .hover_pos()
            .and_then(|pointer| mark_at(&layout.marks, pointer))
            .map(|mark| ZoomAnchor {
                point: mark.point,
                hit_radius: mark.radius.max(6.0),
                physical_radius: f64::from(mark.physical_radius),
            });
        if let Some((lens, finished)) = approach {
            Self::paint_approach(ui, rect, &layout, camera, clock, maps, &lens);
            if finished {
                self.flight = None;
                return Some(Selection::Focus {
                    id: lens.body.id,
                    camera: Some(lens.camera),
                });
            }
            return None;
        }
        let clicked = paint_marks(ui, &painter, &layout, response, camera, clock, maps);
        if heliocentric {
            paint_heliocentric_details(
                &painter,
                projection,
                self.trajectory.as_deref(),
                preview.time,
            );
        } else {
            backdrop.hover_star(response, |point| mark_at(&layout.marks, point).is_none());
        }
        clicked
            .filter(|_| self.flight.is_none())
            .map(|id| Selection::Focus { id, camera: None })
    }

    fn paint_approach(
        ui: &egui::Ui,
        rect: Rect,
        layout: &OrbitLayout<'_>,
        chart_camera: GlobeCamera,
        clock: [f32; 2],
        maps: &assets::MapCache,
        lens: &world::Approach,
    ) {
        let target = lens.body;
        // The local planets and moons are drawn by the perspective renderer
        // during approach. Keep the other overview objects as distant context.
        if lens.progress < 0.82 {
            for mark in layout.marks.iter().filter(|mark| {
                mark.body.id != target.id
                    && mark.body.parent != Some(target.id)
                    && target.parent != Some(mark.body.id)
            }) {
                paint_icon(ui, layout.painter, mark, chart_camera, clock, maps);
            }
        }
        let _ = lens.paint(ui, rect, layout.day, clock, maps);
    }

    fn unit_km(&self) -> f64 {
        if self.center == SUN { AU_KM } else { 1.0 }
    }

    fn projection(&self, rect: Rect) -> Projection {
        let extent = satellites(self.center)
            .filter_map(|body| body.orbit)
            .map(|orbit| orbit.0 * (1.0 + orbit.1))
            .fold(1.0, f64::max)
            .max(self.trajectory.as_ref().map_or(0.0, |path| path.radius_au));
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_el, cos_el) = self.elevation.sin_cos();
        let span = f64::from(rect.width().min(rect.height() * 1.32));
        Projection {
            center: [
                f64::from(rect.center().x) + self.pan[0],
                f64::from(rect.center().y) + self.pan[1],
            ],
            scale: span * 0.45 * self.zoom / extent,
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
            directional_light: 1.0,
            star_trail: 0.0,
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
                let mark = Mark::new(
                    sun,
                    projection.project([0.0; 3]),
                    true,
                    projection.scale / AU_KM,
                );
                rect.expand(mark.radius * 1.65).contains(mark.point)
            });
        if visible_sun {
            ui.ctx().request_repaint_after(Duration::from_millis(66));
        }
    }

    fn paint_orbits(&self, layout: &mut OrbitLayout<'_>, projection: Projection, primary: Id) {
        let helio = primary == SUN;
        if helio {
            paint_belts(layout.painter, projection, self.zoom);
        }
        let bodies = satellites(primary);
        let dense = bodies.clone().count() > 24;
        for satellite in bodies {
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
            layout.orbit(satellite, projection, opacity, samples);
            let position = satellite.at(layout.day);
            let mark = Mark::new(
                satellite,
                projection.project(position),
                false,
                projection.scale / self.unit_km(),
            );
            if helio {
                layout.moon_system(satellite, position, &mark, projection);
            }
            if layout
                .painter
                .clip_rect()
                .expand(mark.radius * 3.5)
                .contains(mark.point)
            {
                layout.marks.push(mark);
            }
        }
        if let Some(trajectory) = self.trajectory.as_deref() {
            paint_trajectory_path(layout.painter, projection, trajectory);
        }
    }

    fn navigate(&mut self, response: &egui::Response) -> Wheel {
        let factor = map_camera::scroll_zoom(response).powf(1.8).clamp(0.2, 5.0);
        let middle = self.middle.update(response);
        let pan = response.dragged_by(egui::PointerButton::Primary);
        let cancel = response
            .ctx
            .input(|input| input.key_pressed(egui::Key::Escape));
        if let Some(flight) = self.flight.as_ref() {
            if flight.departure.is_some() || (middle.is_none() && !pan && factor >= 1.0 && !cancel)
            {
                return Wheel::Idle;
            }
            self.flight = None;
        }
        if let Some(delta) = middle {
            self.yaw -= f64::from(delta.x) * 0.008;
            self.elevation = (self.elevation + f64::from(delta.y) * 0.007).clamp(-1.56, 1.56);
        } else if pan {
            let delta = response.drag_delta();
            self.pan[0] += f64::from(delta.x);
            self.pan[1] += f64::from(delta.y);
        }
        if let Some(pointer) = response.hover_pos()
            && factor != 1.0
        {
            let beyond = map_camera::past_zoom_limit(self.zoom, MIN_ORBIT_ZOOM, factor);
            let hovered = self.zoom_anchor.filter(|anchor| {
                factor > 1.0 && anchor.point.distance(pointer) <= anchor.hit_radius
            });
            let applied_factor = hovered.map_or(factor, |anchor| {
                anchor.approach_scroll_factor(response.rect, factor)
            });
            let next = (self.zoom * applied_factor).clamp(MIN_ORBIT_ZOOM, MAX_ORBIT_ZOOM);
            let anchor_point = hovered.map_or(pointer, |anchor| anchor.point);
            let anchor = anchor_point - response.rect.center();
            for (pan, anchor) in self.pan.iter_mut().zip([anchor.x, anchor.y]) {
                let anchor = f64::from(anchor);
                *pan = anchor + (*pan - anchor) * next / self.zoom;
            }
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
        let target_radius = approach_radius(rect);
        if f64::from(mark.physical_radius) < focus_threshold(rect) {
            return;
        }
        let [x, y] = (mark.point - rect.center()).into();
        let screen_offset = [f64::from(x), f64::from(y)];
        self.flight = Some(Flight {
            id: mark.body.id,
            began: now,
            from_zoom: self.zoom,
            zoom_factor: target_radius / f64::from(mark.physical_radius),
            from_radius: f64::from(mark.radius),
            target_radius,
            screen_offset,
            orbit_offset: std::array::from_fn(|i| screen_offset[i] - self.pan[i]),
            departure: None,
        });
    }

    fn advance_flight(
        &mut self,
        response: &egui::Response,
        now: f64,
    ) -> Option<(Id, FlightSample)> {
        let flight = self.flight.as_ref()?;
        let sample = flight.sample(now);
        self.zoom = sample.zoom;
        self.pan = sample.pan;
        response.ctx.request_repaint();
        if sample.finished && flight.departure.is_some() {
            self.flight = None;
            return None;
        }
        Some((flight.id, sample))
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

const APPROACH_ZOOM: f64 = 0.18;
const MIN_ORBIT_ZOOM: f64 = 0.55;
const MAX_ORBIT_ZOOM: f64 = 30_000_000.0;
const OVERVIEW_MOON_GUIDE_RADIUS_KM: f64 = 38.0;

fn approach_radius(rect: Rect) -> f64 {
    APPROACH_ZOOM * 0.43 * f64::from(rect.width().min(rect.height()))
}

fn focus_threshold(rect: Rect) -> f64 {
    (approach_radius(rect) * 0.55).max(13.0)
}

#[derive(Clone, Copy)]
struct ZoomAnchor {
    point: Pos2,
    hit_radius: f32,
    physical_radius: f64,
}

impl ZoomAnchor {
    fn approach_scroll_factor(self, rect: Rect, factor: f64) -> f64 {
        let landing_radius = approach_radius(rect).max(13.0);
        factor.min((landing_radius / self.physical_radius).max(1.0))
    }
}

struct Flight {
    id: Id,
    began: f64,
    from_zoom: f64,
    zoom_factor: f64,
    from_radius: f64,
    target_radius: f64,
    screen_offset: [f64; 2],
    orbit_offset: [f64; 2],
    departure: Option<GlobeCamera>,
}

#[derive(Clone, Copy)]
struct FlightSample {
    zoom: f64,
    pan: [f64; 2],
    offset: [f64; 2],
    radius: f64,
    progress: f64,
    finished: bool,
}

impl Flight {
    fn sample(&self, now: f64) -> FlightSample {
        let time = ((now - self.began) / 0.48).clamp(0.0, 1.0);
        let position = if self.departure.is_some() {
            1.0 - time
        } else {
            time
        };
        let progress = position * position * (3.0 - 2.0 * position);
        let scale = self.zoom_factor.powf(progress);
        let offset = self.screen_offset.map(|v| v * (1.0 - progress));
        FlightSample {
            zoom: self.from_zoom * scale,
            pan: std::array::from_fn(|i| offset[i] - self.orbit_offset[i] * scale),
            offset,
            radius: self.from_radius * (self.target_radius / self.from_radius).powf(progress),
            progress,
            finished: time >= 1.0,
        }
    }
}

#[derive(Clone, Copy)]
struct Projection {
    center: [f64; 2],
    scale: f64,
    yaw: [f64; 2],
    elevation: [f64; 2],
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

    fn relative(self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let [sy, cy] = self.yaw;
        let [se, ce] = self.elevation;
        let [x, y] = [cy * x - sy * y, sy * x + cy * y];
        [x, -(se * y + ce * z), ce * y - se * z].map(|axis| axis * self.scale)
    }

    fn project(self, position: [f64; 3]) -> (Pos2, f64) {
        let [x, y, z] = self.relative(position);
        (
            pos2((self.center[0] + x) as f32, (self.center[1] + y) as f32),
            z,
        )
    }

    fn at_parent(self, position: [f64; 3]) -> Self {
        let [x, y, _] = self.relative(position);
        Self {
            center: [self.center[0] + x, self.center[1] + y],
            scale: self.scale / AU_KM,
            ..self
        }
    }
}

struct Mark {
    body: &'static Body,
    point: Pos2,
    depth: f64,
    radius: f32,
    physical_radius: f32,
    primary: bool,
}

impl Mark {
    fn new(
        body: &'static Body,
        (point, depth): (Pos2, f64),
        primary: bool,
        px_per_km: f64,
    ) -> Self {
        let km = body.radius.unwrap_or(3.0);
        let marker = match body.kind() {
            Kind::Star => 9.0,
            Kind::Planet => 2.7 + (km / 8_000.0).sqrt().min(2.7) as f32,
            Kind::DwarfPlanet => 2.2,
            Kind::Moon => {
                if km > 100.0 {
                    1.9
                } else {
                    0.9
                }
            }
        };
        let physical_radius = (km * px_per_km) as f32;
        Self {
            body,
            point,
            depth,
            radius: physical_radius.hypot(marker),
            physical_radius,
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
        opacity: u8,
        samples: Option<&[[f64; 3]]>,
    ) {
        let color = body.color();
        let clip = self.painter.clip_rect().expand(2.0);
        let error = body.semimajor() * projection.scale;
        let stroke = Stroke::new(1.0, color.gamma_multiply(f32::from(opacity) / 255.0));
        let point = |t| projection.project(body.orbit_point(t)).0;
        let mut start = point(0.0);
        let mut paths = Vec::new();
        for i in 0..32 {
            let t = f64::from(i) / 32.0 * TAU;
            let end_t = f64::from(i + 1) / 32.0 * TAU;
            let end = point(end_t);
            orbit_arc(clip, &point, [t, end_t], [start, end], error, &mut paths);
            start = end;
        }
        for path in paths {
            self.painter.add(egui::Shape::line(path, stroke));
        }
        if let Some(samples) = samples {
            let points = samples
                .iter()
                .map(|point| projection.project(*point).0)
                .collect();
            self.painter
                .add(egui::Shape::line(points, Stroke::new(1.45, color)));
        }
    }

    fn moon_system(&mut self, parent: &Body, position: [f64; 3], mark: &Mark, plane: Projection) {
        let viewport = self.painter.clip_rect();
        let projection = plane.at_parent(position);
        let minor_visibility = f64::from((mark.physical_radius - 6.0) / 8.0).clamp(0.0, 1.0);
        for moon in satellites(parent.id) {
            let Some(orbit) = moon.orbit else { continue };
            let spread = moon.semimajor() * projection.scale;
            let reveal = f64::from((mark.radius * 1.3).max(13.0));
            if spread < reveal
                || !viewport
                    .expand((spread * (1.0 + orbit.1)) as f32)
                    .contains(mark.point)
            {
                continue;
            }
            let separation = ((spread - reveal) / reveal).clamp(0.0, 1.0);
            let guide = moon.radius.unwrap_or(0.0) > OVERVIEW_MOON_GUIDE_RADIUS_KM;
            let visibility = if guide {
                self.orbit(moon, projection, (separation * 117.0) as u8, None);
                separation
            } else {
                separation * minor_visibility
            };
            let (point, depth) = projection.project(moon.at(self.day));
            let mut next = Mark::new(moon, (point, depth + mark.depth), false, projection.scale);
            next.radius =
                world::moon_marker_radius(next.physical_radius, world::major_moon(parent, moon))
                    * visibility.sqrt() as f32;
            if next.radius > 0.6 && viewport.expand(next.radius * 3.5).contains(point) {
                self.marks.push(next);
            }
        }
    }
}

const TRAJECTORY_COLOR: Color32 = Color32::from_rgb(103, 237, 207);

fn paint_trajectory_path(painter: &egui::Painter, projection: Projection, path: &Trajectory) {
    let clip = painter.clip_rect().expand(4.0);
    let mut paths = Vec::new();
    let mut previous = None;
    for point in path
        .positions()
        .map(|position| projection.project(position).0)
    {
        if let Some(start) = previous {
            if point.distance_sq(start) < 0.09 {
                continue;
            }
            orbit_arc(
                clip,
                &|_| point,
                [0.0, 1.0],
                [start, point],
                0.0,
                &mut paths,
            );
        }
        previous = Some(point);
    }
    let stroke = Stroke::new(1.8, TRAJECTORY_COLOR.gamma_multiply(0.78));
    for path in paths {
        painter.add(egui::Shape::line(path, stroke));
    }
}

fn paint_heliocentric_details(
    painter: &egui::Painter,
    projection: Projection,
    path: Option<&Trajectory>,
    time: chrono::DateTime<chrono::Utc>,
) {
    let rect = painter.clip_rect();
    if let Some(position) = path.and_then(|path| path.position_at(time)) {
        let point = projection.project(position).0;
        if rect.expand(8.0).contains(point) {
            painter.circle_filled(point, 7.0, TRAJECTORY_COLOR.gamma_multiply(0.26));
            painter.circle(
                point,
                4.0,
                TRAJECTORY_COLOR,
                Stroke::new(1.0, Color32::BLACK),
            );
        }
    }
    orbit_scale_bar(painter, rect, projection.scale);
}

fn orbit_arc(
    clip: Rect,
    point: &impl Fn(f64) -> Pos2,
    time: [f64; 2],
    endpoints: [Pos2; 2],
    curvature: f64,
    paths: &mut Vec<Vec<Pos2>>,
) {
    let deviation = (curvature * (time[1] - time[0]).powi(2) * 0.125) as f32;
    if !Rect::from_two_pos(endpoints[0], endpoints[1])
        .expand(deviation)
        .intersects(clip)
    {
        return;
    }
    if deviation <= 0.6 {
        match paths
            .last_mut()
            .filter(|path| path.last() == Some(&endpoints[0]))
        {
            Some(path) => path.push(endpoints[1]),
            None => paths.push(endpoints.to_vec()),
        }
        return;
    }
    let middle = time[0].midpoint(time[1]);
    let p = point(middle);
    orbit_arc(
        clip,
        point,
        [time[0], middle],
        [endpoints[0], p],
        curvature,
        paths,
    );
    orbit_arc(
        clip,
        point,
        [middle, time[1]],
        [p, endpoints[1]],
        curvature,
        paths,
    );
}

fn paint_icon(
    ui: &egui::Ui,
    painter: &egui::Painter,
    mark: &Mark,
    mut camera: GlobeCamera,
    clock: [f32; 2],
    images: &assets::MapCache,
) {
    let rings = world::bands(mark.body.id);
    if mark.radius < 2.5 && rings.is_empty() && mark.body.id != SUN {
        painter.circle_filled(mark.point, mark.radius, mark.body.color());
        return;
    }
    let size_ratio = if mark.body.id == SUN {
        1.8
    } else {
        world::outer_radius_factor(mark.body) * 1.08
    };
    let rect = Rect::from_center_size(mark.point, Vec2::splat(mark.radius * size_ratio * 2.0));
    if !painter.clip_rect().intersects(rect) {
        return;
    }
    camera.zoom = 1.0 / (0.86 * f64::from(size_ratio));
    let texture = if mark.primary || mark.radius >= 135.0 {
        images
            .map(mark.body.id, false)
            .or_else(|| images.preview(mark.body.id))
    } else {
        images.preview(mark.body.id)
    };
    let mut frame = world::Frame::new(mark.body, camera, rect.size().into(), texture);
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
            let resolved = |mark: &Mark| {
                mark.physical_radius >= 2.5
                    && pointer.distance_sq(mark.point) <= mark.radius * mark.radius
            };
            match (resolved(a), resolved(b)) {
                (true, true) => a.depth.total_cmp(&b.depth),
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (false, false) => pointer
                    .distance_sq(a.point)
                    .total_cmp(&pointer.distance_sq(b.point))
                    .then_with(|| a.depth.total_cmp(&b.depth)),
            }
        })
}

fn paint_marks(
    ui: &egui::Ui,
    painter: &egui::Painter,
    layout: &OrbitLayout<'_>,
    response: &egui::Response,
    camera: GlobeCamera,
    clock: [f32; 2],
    images: &assets::MapCache,
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
            && (mark.body.radius.unwrap_or(0.0) <= OVERVIEW_MOON_GUIDE_RADIUS_KM
                || mark.radius < 1.5)
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

fn orbit_scale_bar(painter: &egui::Painter, rect: Rect, pixels_per_au: f64) {
    let (length, text) = orbit_scale_ticks(pixels_per_au);
    let main = Color32::from_rgb(244, 246, 250);
    let muted = Color32::from_rgb(186, 196, 211);
    let label = painter.layout_no_wrap(text, FontId::proportional(11.5), main);
    let note = painter.layout_no_wrap(
        "objects not to scale".into(),
        FontId::proportional(10.5),
        muted,
    );
    let width = (length + label.size().x + 24.0).max(note.size().x + 16.0);
    let badge = Rect::from_min_size(rect.left_bottom() + vec2(10.0, -46.0), vec2(width, 36.0));
    painter.rect_filled(badge, 4.0, Color32::from_black_alpha(165));
    let start = badge.min + vec2(8.0, 13.0);
    let end = start + vec2(length, 0.0);
    painter.line_segment([start, end], Stroke::new(1.6, main));
    for tick in [start, end] {
        painter.line_segment(
            [tick - vec2(0.0, 3.5), tick + vec2(0.0, 2.0)],
            Stroke::new(1.6, main),
        );
    }
    painter.galley(end + vec2(8.0, -label.size().y * 0.5), label, main);
    painter.galley(badge.min + vec2(8.0, 20.0), note, muted);
}

fn orbit_scale_ticks(pixels_per_au: f64) -> (f32, String) {
    let length = if (55.0..=160.0).contains(&pixels_per_au) {
        1.0
    } else {
        let target = 128.0 / pixels_per_au;
        let decade = 10.0_f64.powf(target.log10().floor());
        [5.0, 2.0, 1.0]
            .into_iter()
            .map(|step| step * decade)
            .find(|length| *length <= target)
            .unwrap_or(decade)
    };
    ((length * pixels_per_au) as f32, format!("{length} AU"))
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

fn paint_belts(painter: &egui::Painter, projection: Projection, zoom: f64) {
    // Fixed orbital dust positions avoid flicker while panning. Point size grows
    // slowly with zoom so individual objects stay legible.
    static PARTICLES: OnceLock<Vec<([f64; 3], u8, f32)>> = OnceLock::new();
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
                    ([a * cos, a * sin, z], belt as u8, noise(seed + 4) as f32)
                })
            })
            .collect()
    });
    let colors = [
        Color32::from_rgb(139, 117, 92),
        Color32::from_rgb(102, 125, 156),
    ];
    let clip = painter.clip_rect();
    let scale = (0.8 + 0.72 * zoom.ln().max(0.0)).min(3.8) as f32;
    for &(position, belt, bright) in particles {
        let point = projection.project(position).0;
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
        let flight = Flight {
            id: 599,
            began: 1.0,
            from_zoom: 4.0,
            zoom_factor: 2.0,
            from_radius: 25.0,
            target_radius: 50.0,
            screen_offset: [130.0, -55.0],
            orbit_offset: [-240.0, 156.0],
            departure: None,
        };
        let start = flight.sample(1.0);
        assert_eq!(start.progress, 0.0);
        assert_eq!(
            std::array::from_fn(|i| start.pan[i] + flight.orbit_offset[i]),
            flight.screen_offset
        );
        assert_eq!(start.zoom, 4.0);
        let end = flight.sample(1.6);
        assert_eq!(end.progress, 1.0);
        assert_eq!(end.zoom, 8.0);
        assert_eq!(end.radius, 50.0);
        assert_eq!(
            std::array::from_fn(|i| end.pan[i] + flight.orbit_offset[i] * end.zoom / start.zoom),
            [0.0; 2]
        );
    }

    #[test]
    fn departure_keeps_the_body_centered_one_scroll_step_from_focus() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let camera = GlobeCamera {
            center_lat: -24.0,
            center_lon: 130.0,
            zoom: 0.14,
            ..map_camera::lunar_camera()
        };
        for id in [599, super::super::EARTH, super::super::MOON] {
            let target = body(id).unwrap();
            let day = 200.0;
            let mut system = System::departing(target, camera, rect, 1.0, day);
            let flight = system.flight.take().unwrap();
            let start = flight.sample(1.0);
            let middle = flight.sample(1.24);
            let end = flight.sample(1.48);
            assert_eq!(start.progress, 1.0);
            assert!((start.radius - camera.scale_px(rect.width(), rect.height())).abs() < 1e-12);
            assert!(start.zoom > middle.zoom && middle.zoom > end.zoom);
            assert!(end.finished);

            let position = position_au(target, day);
            let centered_mark = |system: &System| {
                let projection = system.projection(rect);
                let mark = Mark::new(
                    target,
                    projection.project(position),
                    false,
                    projection.scale / AU_KM,
                );
                assert!(mark.point.distance(rect.center()) < 0.01);
                mark
            };
            system.zoom = end.zoom;
            system.pan = end.pan;
            let mark = centered_mark(&system);
            assert!(f64::from(mark.physical_radius) < focus_threshold(rect));
            system.start_flight(rect, rect.center(), &[mark], 1.6);
            assert!(system.flight.is_none());

            system.zoom *= 1.04;
            system.pan = system.pan.map(|offset| offset * 1.04);
            system.start_flight(rect, rect.center(), &[centered_mark(&system)], 1.7);
            assert_eq!(system.flight.unwrap().id, id);
        }
    }

    #[test]
    fn overview_uses_linear_au_and_reveals_the_moon_before_earth_lock() {
        let plane = Projection {
            center: [400.0, 300.0],
            scale: 10_000.0,
            yaw: [0.0, 1.0],
            elevation: [1.0, 0.0],
        };
        assert_eq!(plane.relative([3.0, 0.0, 0.0])[0], 30_000.0);
        let earth_pos = [1.0, 0.0, 0.0];
        let local = plane.at_parent(earth_pos);
        let moon = body(super::super::MOON).unwrap();
        let distance = moon.semimajor();
        assert!(
            (local.relative([distance, 0.0, 0.0])[0] - distance / AU_KM * plane.scale).abs() < 1e-8
        );
        let map = Rect::from_min_max(Pos2::ZERO, pos2(800.0, 600.0));
        egui::__run_test_ui(|ui| {
            let painter = ui.painter_at(map);
            let centered = Projection {
                center: [-9600.0, 300.0],
                ..plane
            };
            let earth = Mark::new(
                body(super::super::EARTH).unwrap(),
                centered.project(earth_pos),
                false,
                centered.scale / AU_KM,
            );
            let mut layout = OrbitLayout {
                painter: &painter,
                marks: Vec::new(),
                day: 200.0,
            };
            layout.moon_system(earth.body, earth_pos, &earth, centered);
            let lunar = layout
                .marks
                .iter()
                .find(|mark| mark.body.id == moon.id)
                .unwrap();
            assert!(earth.physical_radius < 1.0);
            assert!(earth.point.distance(lunar.point) > earth.radius * 2.0);
            assert!(lunar.radius < earth.radius);
        });
    }

    #[test]
    fn ruler_tracks_the_actual_projected_length_per_au() {
        assert_eq!(orbit_scale_ticks(90.0), (90.0, "1 AU".into()));
        assert_eq!(orbit_scale_ticks(5.0), (100.0, "20 AU".into()));
        assert_eq!(orbit_scale_ticks(100_000.0), (100.0, "0.001 AU".into()));
    }

    #[test]
    fn fast_zoom_cannot_overshoot_the_landing_radius() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let radius = approach_radius(rect);
        let anchor = ZoomAnchor {
            point: rect.center(),
            hit_radius: 0.5 * radius as f32,
            physical_radius: 0.5 * radius,
        };
        assert_eq!(anchor.approach_scroll_factor(rect, 1.2), 1.2);
        assert_eq!(anchor.approach_scroll_factor(rect, 4.0), 2.0);
    }

    #[test]
    fn pointer_picks_the_nearer_world_when_disks_overlap() {
        let here = Pos2::ZERO;
        let distant = Mark::new(body(599).unwrap(), (here, 1.0), false, 0.0);
        let nearer = Mark::new(body(501).unwrap(), (here, 0.0), false, 0.0);
        assert_eq!(mark_at(&[distant, nearer], here).unwrap().body.id, 501);
    }
}
