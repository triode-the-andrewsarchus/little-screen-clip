use super::{Body, Id, SUN, assets, body, clock, satellites, scene_day, sunlight};
use crate::file_manager::geography::{gpu, map_camera, sky};
use chrono::{DateTime, Utc};
use eframe::egui::{self, Color32, FontId, Pos2, Rect};
use little_clip_core::{
    geography::basemap::{
        GlobeCamera,
        perspective::{Perspective, sphere_distance},
    },
    jobs::task::Task,
};
use std::{path::Path, sync::Arc, time::Duration};

pub(in crate::file_manager::geography) struct World {
    pub body: &'static Body,
    pub camera: GlobeCamera,
    middle: map_camera::MiddleDrag,
    inspected: bool,
    job: Option<Task<Result<Option<assets::Assets>, String>>>,
    assets: Option<assets::Assets>,
    neighbors: Option<Task<()>>,
    neighbors_checked: bool,
    pub error: Option<String>,
}

impl World {
    pub fn new(body: &'static Body) -> Self {
        let mut camera = map_camera::lunar_camera();
        camera.center_lat = 21.0;
        camera.zoom = initial_zoom(body.id);
        Self {
            body,
            camera,
            middle: Default::default(),
            inspected: false,
            job: None,
            assets: None,
            neighbors: None,
            neighbors_checked: false,
            error: None,
        }
    }

    pub fn navigate(&mut self, response: &egui::Response) -> bool {
        map_camera::navigate(
            &mut self.camera,
            response,
            Some(radius_m(self.body)),
            &mut self.middle,
        )
    }

    pub fn reload(&mut self) {
        self.job = None;
        self.neighbors = None;
        self.neighbors_checked = false;
        self.inspected = false;
        self.error = None;
    }

    pub fn poll(&mut self, ctx: &egui::Context, root: &Path, cache: &Arc<assets::MapCache>) {
        if let Some(result) = self.job.as_ref().and_then(Task::poll) {
            self.job = None;
            self.inspected = true;
            match result {
                Ok(Ok(assets)) => self.assets = assets,
                Ok(Err(error)) => self.error = Some(error),
                Err(error) => self.error = Some(format!("World asset worker: {error}")),
            }
        }
        if !self.inspected && self.job.is_none() {
            let root = root.to_owned();
            let id = self.body.id;
            let cache = Arc::clone(cache);
            self.job = Some(Task::start(
                super::super::super::geography_downloader::asset_workers(),
                Duration::from_secs(30),
                move |_| assets::Assets::load(&cache, &root, id),
            ));
        }
        self.poll_neighbors(root, cache);
        if self.job.is_some() || self.neighbors.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn poll_neighbors(&mut self, root: &Path, cache: &Arc<assets::MapCache>) {
        if self.neighbors.as_ref().and_then(Task::poll).is_some() {
            self.neighbors = None;
        }
        if self.neighbors_checked || !self.inspected {
            return;
        }
        self.neighbors_checked = true;
        let root = root.to_owned();
        let focused = self.body;
        let cache = Arc::clone(cache);
        self.neighbors = Some(Task::start(
            assets::preview_workers(),
            Duration::from_secs(30),
            move |cancel| {
                if let Some(parent) = focused.parent.filter(|id| *id != SUN) {
                    cache.load_map(&root, parent, false).ok();
                }
                if focused.id == SUN {
                    return;
                }
                for moon in satellites(focused.id)
                    .take_while(|_| !cancel.load(std::sync::atomic::Ordering::Relaxed))
                {
                    cache.load_map(&root, moon.id, true).ok();
                }
            },
        ));
    }

    pub fn pending(&self) -> bool {
        !self.inspected
    }

    pub fn installed(&self) -> bool {
        self.assets.as_ref().is_some_and(|assets| assets.preferred)
    }

    pub fn paint(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        time: DateTime<Utc>,
        stars: Option<std::sync::Arc<sky::Assets>>,
        constellations: bool,
        cache: &Arc<assets::MapCache>,
    ) -> Result<Option<Id>, String> {
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, Color32::from_rgb(7, 10, 17));
        if stars.is_none() {
            super::paint_stars(&painter, rect);
        }
        let mut frame = Frame::new(
            self.body,
            self.camera,
            rect.size().into(),
            self.texture()
                .or_else(|| cache.map(self.body.id, false))
                .or_else(|| cache.preview(self.body.id)),
        );
        let animation = ui.input(|input| input.time);
        let day = scene_day(time);
        let sun = sunlight(self.body, day);
        frame.sun = sun.0;
        frame.clock = clock(time, animation);
        let lights = Backdrop {
            axes: frame.pose.axes.map(|axis| axis.map(|v| v as f32)),
            fov: frame.pose.half_fov(),
            sun,
            clock: frame.clock,
            brightness: 1.0,
            assets: stars,
            constellations,
        };
        let sun_visible = self.body.id == SUN
            || frame.pose.intersects_sphere(
                sun.0.map(|v| f64::from(v) * 1e6),
                f64::from(sun.1) * 1.65e6,
                0.0,
            );
        if sun_visible {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        let pose = frame.pose;
        let (neighbors, distant) = self.paint_bodies(ui, rect, frame, day, &lights, cache)?;
        label_major_moons(&painter, &neighbors);
        let target = pick_nearby(neighbors.iter().chain(&distant), pose, response)
            .or_else(|| pick_sun(pose, sun, response));
        if target.is_none() {
            let exposed = |point: Pos2| {
                let local = point - rect.min;
                pose.ground(local.x, local.y).is_none()
            };
            lights.hover_star(response, exposed);
        }
        Ok(target.filter(|_| response.clicked()))
    }

    fn paint_bodies(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        frame: Frame,
        day: f64,
        lights: &Backdrop,
        cache: &Arc<assets::MapCache>,
    ) -> Result<(Vec<Nearby>, Vec<Nearby>), String> {
        let neighbors = surroundings(self.body, frame.pose, rect, day);
        let distant = distant_planets(self.body, frame.pose, rect, day);
        if self.neighbors.is_none() {
            cache.request_distant(
                ui.ctx(),
                distant
                    .iter()
                    .filter(|item| item.radius >= 2.5 && !item.occluded)
                    .map(|item| item.body.id),
            );
        }
        let pose = frame.pose;
        let clock = frame.clock;
        gpu::paint_space(ui, rect, lights.clone())?;
        for planet in &distant {
            planet.paint(
                ui,
                pose,
                rect,
                rect,
                sunlight(planet.body, day).0,
                clock,
                cache.preview(planet.body.id).as_ref(),
            )?;
        }
        paint_local_bodies(ui, rect, rect, frame, self.body, &neighbors, cache)?;
        Ok((neighbors, distant))
    }

    pub fn texture(&self) -> Option<sky::Texture> {
        self.assets.as_ref()?.texture.clone()
    }

    pub fn ephemeris(&self) -> Option<&[[f64; 3]]> {
        self.assets.as_ref()?.ephemeris.as_deref()
    }
}

fn paint_local_bodies(
    ui: &egui::Ui,
    canvas: Rect,
    viewport: Rect,
    frame: Frame,
    primary: &Body,
    neighbors: &[Nearby],
    cache: &assets::MapCache,
) -> Result<(), String> {
    let pose = frame.pose;
    let clock = frame.clock;
    let sun = frame.sun;
    let paint_side = |behind| {
        for moon in neighbors.iter().filter(|moon| moon.behind == behind) {
            moon.paint(
                ui,
                pose,
                canvas,
                viewport,
                sun,
                clock,
                cache
                    .map(moon.body.id, primary.parent != Some(moon.body.id))
                    .or_else(|| cache.preview(moon.body.id))
                    .as_ref(),
            )?;
        }
        Ok::<_, String>(())
    };
    paint_side(true)?;
    gpu::paint_world_clipped(ui, canvas, viewport, frame)?;
    paint_side(false)?;
    Ok(())
}

/// The lens used while an orthographic chart becomes a focused globe.
pub(super) struct Approach {
    pub body: &'static Body,
    pub camera: GlobeCamera,
    pub center: Pos2,
    pub radius: f64,
    pub progress: f64,
}

impl Approach {
    fn canvas(&self, viewport: Rect) -> Rect {
        Rect::from_center_size(
            self.center,
            viewport.size() + (self.center - viewport.center()).abs() * 2.0,
        )
    }

    pub(super) fn pose(&self, viewport: Rect) -> Perspective {
        let radius = radius_m(self.body);
        if self.progress >= 1.0 {
            return Perspective::new(&self.camera, viewport.size().into(), radius);
        }
        let mut camera = self.camera;
        camera.orbit.tilt *= self.progress;
        camera.orbit.bearing *= self.progress;
        camera.orbit.target_height_m *= self.progress;
        camera.orbit.eye_ground_m *= self.progress;
        let mut pose = Perspective::new(&camera, self.canvas(viewport).size().into(), radius);
        // Near-orthographic lens initially, converging to the exact focused
        // camera. Keep the planet's apparent radius independent of that change
        // so the moons move continuously as their depth becomes visible.
        let back = pose.axes[2];
        let focused_distance = pose
            .eye
            .iter()
            .zip(back)
            .map(|(v, axis)| v * axis)
            .sum::<f64>();
        let distance = 1.0 / ((1.0 - self.progress) / 8192.0 + self.progress / focused_distance);
        pose.eye = std::array::from_fn(|i| pose.eye[i] + back[i] * (distance - focused_distance));
        pose.focal = self.radius * (distance * distance - 1.0).sqrt();
        pose
    }

    pub fn paint(
        &self,
        ui: &egui::Ui,
        viewport: Rect,
        day: f64,
        clock: [f32; 2],
        cache: &assets::MapCache,
    ) -> Result<(), String> {
        let canvas = self.canvas(viewport);
        let pose = self.pose(viewport);
        let texture = cache
            .map(self.body.id, false)
            .or_else(|| cache.preview(self.body.id));
        let mut frame = Frame::from_pose(self.body, pose, self.camera.zoom as f32, texture);
        frame.clock = clock;
        frame.sun = sunlight(self.body, day).0;
        let neighbors = surroundings(self.body, pose, canvas, day);
        paint_local_bodies(ui, canvas, viewport, frame, self.body, &neighbors, cache)?;
        label_major_moons(&ui.painter_at(viewport), &neighbors);
        Ok(())
    }
}

fn label_major_moons(painter: &egui::Painter, bodies: &[Nearby]) {
    let mut majors: Vec<_> = bodies
        .iter()
        .filter(|body| body.major && !body.occluded)
        .collect();
    majors.sort_by(|a, b| b.radius.total_cmp(&a.radius));
    let mut occupied = Vec::new();
    for body in majors {
        body.label(painter, &mut occupied);
    }
}

struct Nearby {
    body: &'static Body,
    point: Pos2,
    radius: f32,
    position: [f64; 3],
    display_radius: f64,
    depth: f64,
    major: bool,
    behind: bool,
    occluded: bool,
}

impl Nearby {
    fn paint(
        &self,
        ui: &egui::Ui,
        pose: Perspective,
        canvas: Rect,
        viewport: Rect,
        sun: [f32; 3],
        clock: [f32; 2],
        texture: Option<&sky::Texture>,
    ) -> Result<(), String> {
        if self.display_radius / self.depth * pose.focal < 2.0 && bands(self.body.id).is_empty() {
            ui.painter_at(canvas.intersect(viewport)).circle_filled(
                self.point,
                self.radius,
                self.body.color(),
            );
            return Ok(());
        }
        let mut frame = Frame::from_pose(self.body, pose, 1.0, texture.cloned());
        frame.pose.eye =
            std::array::from_fn(|i| (pose.eye[i] - self.position[i]) / self.display_radius);
        frame.sun = sun;
        frame.clock = clock;
        // Preserve the canvas ray projection while restricting shader work to
        // the object's possible silhouette, including the outer edge of its rings.
        let bound = visible_radius(self.body, self.display_radius);
        let clip = projected_bounds(pose, canvas, self.position, bound).intersect(viewport);
        if !clip.is_positive() {
            return Ok(());
        }
        gpu::paint_world_clipped(ui, canvas, clip, frame)
    }

    fn label(&self, painter: &egui::Painter, occupied: &mut Vec<Rect>) {
        let text = painter.layout_no_wrap(
            self.body.name.to_owned(),
            FontId::proportional(11.0),
            Color32::from_rgb(206, 217, 234),
        );
        let position = self.point + egui::vec2(self.radius + 4.0, -text.size().y * 0.5);
        let rect = Rect::from_min_size(position, text.size()).expand2(egui::vec2(3.0, 1.0));
        if painter.clip_rect().contains_rect(rect)
            && !occupied.iter().any(|used| used.intersects(rect))
        {
            occupied.push(rect);
            painter.rect_filled(rect, 2.0, Color32::from_black_alpha(180));
            painter.galley(position, text, Color32::WHITE);
        }
    }
}

fn projected_bounds(pose: Perspective, canvas: Rect, position: [f64; 3], radius: f64) -> Rect {
    let relative = std::array::from_fn::<_, 3, _>(|i| position[i] - pose.eye[i]);
    let [x, y, z] = pose
        .axes
        .map(|axis| axis.iter().zip(relative).map(|(a, b)| a * b).sum::<f64>());
    let depth = -z;
    if depth <= radius {
        return canvas;
    }
    // Extrema of x/depth and y/depth over the view-space bounding sphere.
    let range = |offset: f64| {
        let denominator = depth * depth - radius * radius;
        let delta = radius * (offset * offset + denominator).sqrt();
        [
            (offset * depth - delta) / denominator,
            (offset * depth + delta) / denominator,
        ]
        .map(|slope| (slope * pose.focal) as f32)
    };
    let [left, right] = range(x);
    let [top, bottom] = range(-y);
    Rect::from_min_max(
        canvas.center() + egui::vec2(left, top),
        canvas.center() + egui::vec2(right, bottom),
    )
    .expand(2.0)
    .intersect(canvas)
}

fn pick_nearby<'a>(
    bodies: impl Iterator<Item = &'a Nearby>,
    pose: Perspective,
    response: &egui::Response,
) -> Option<Id> {
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())?;
    let offset = pointer - response.rect.center();
    let [right, up, back] = pose.axes;
    let ray = std::array::from_fn::<_, 3, _>(|i| {
        (right[i] * f64::from(offset.x) - up[i] * f64::from(offset.y)) / pose.focal - back[i]
    });
    let norm = ray.iter().map(|v| v * v).sum::<f64>().sqrt();
    let ray = ray.map(|v| v / norm);
    let foreground = sphere_distance(pose.eye, ray, 1.0).unwrap_or(f64::INFINITY);
    let hit = bodies
        .filter_map(|moon| {
            let eye = std::array::from_fn(|i| pose.eye[i] - moon.position[i]);
            let depth = sphere_distance(eye, ray, moon.display_radius).or_else(|| {
                (moon.radius < 6.0 && !moon.occluded && moon.point.distance(pointer) <= 6.0)
                    .then_some(moon.depth)
            })?;
            (depth < foreground).then_some((moon, depth))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))?
        .0;
    response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    response.clone().on_hover_text_at_pointer(hit.body.name);
    Some(hit.body.id)
}

fn pick_sun(pose: Perspective, sun: ([f32; 3], f32), response: &egui::Response) -> Option<Id> {
    let (direction, radius) = sun;
    if radius <= 0.0 {
        return None;
    }
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())?;
    let point = pointer - response.rect.min;
    let center = pose.project_position(direction.map(|v| f64::from(v) * 1.0e7))?;
    let depth = -pose.axes[2]
        .iter()
        .zip(direction)
        .map(|(a, b)| a * f64::from(b))
        .sum::<f64>();
    let radius_px = (pose.focal * f64::from(radius) / depth.max(0.001)) as f32;
    if (point - egui::vec2(center[0], center[1])).length() > radius_px.max(5.0)
        || pose.ground(point.x, point.y).is_some()
    {
        return None;
    }
    response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    response.clone().on_hover_text_at_pointer("Solar System");
    Some(SUN)
}

fn surroundings(primary: &Body, pose: Perspective, rect: Rect, day: f64) -> Vec<Nearby> {
    let moons = satellites(primary.id)
        .filter(|moon| moon.parent != Some(SUN))
        .map(|moon| (moon, moon.at(day)));
    let parent = primary
        .parent
        .and_then(body)
        .filter(|parent| parent.id != SUN)
        .map(|parent| (parent, primary.at(day).map(|x| -x)));
    let mut visible: Vec<_> = moons
        .chain(parent)
        .filter_map(|(moon, orbit)| project_moon(primary, moon, orbit, pose, rect))
        .collect();
    visible.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    visible
}

fn project_moon(
    primary: &Body,
    moon: &'static Body,
    orbit: [f64; 3],
    pose: Perspective,
    rect: Rect,
) -> Option<Nearby> {
    let primary_km = radius_m(primary) / 1000.0;
    let position = orbit.map(|v| v / primary_km);
    let display_radius = moon.radius.unwrap_or(3.0) / primary_km;
    project_body(
        moon,
        position,
        display_radius,
        major_moon(primary, moon),
        pose,
        rect,
    )
}

pub(super) fn major_moon(primary: &Body, moon: &Body) -> bool {
    primary.parent == Some(moon.id)
        || moon
            .radius
            .is_some_and(|radius| radius >= (radius_m(primary) * 1e-6).max(3.0))
}

pub(super) fn moon_marker_radius(physical: f32, major: bool) -> f32 {
    let minimum = if major { 1.9 } else { 0.9 };
    physical.max(minimum)
}

fn distant_planets(primary: &Body, pose: Perspective, rect: Rect, day: f64) -> Vec<Nearby> {
    if primary.id == SUN {
        return Vec::new();
    }
    let origin = super::position_au(primary, day);
    let mut planets: Vec<_> = satellites(SUN)
        .filter(|planet| {
            planet.kind() == super::Kind::Planet
                && planet.id != primary.id
                && Some(planet.id) != primary.parent
        })
        .filter_map(|planet| {
            let target = super::position_au(planet, day);
            let delta: [f64; 3] = std::array::from_fn(|i| target[i] - origin[i]);
            let au = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
            let angular = (planet.radius? / (super::AU_KM * au) * 120.0).min(0.045);
            let sky_distance = 1e6;
            let position = delta.map(|v| v * sky_distance / au);
            project_body(planet, position, angular * sky_distance, false, pose, rect)
        })
        .collect();
    planets.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    planets
}

fn project_body(
    body: &'static Body,
    position: [f64; 3],
    display_radius: f64,
    major: bool,
    pose: Perspective,
    rect: Rect,
) -> Option<Nearby> {
    let offset = pose.project_position(position)?;
    let point = rect.min + egui::vec2(offset[0], offset[1]);
    let ray = std::array::from_fn(|i| position[i] - pose.eye[i]);
    let depth = ray.iter().map(|v| v * v).sum::<f64>().sqrt();
    let ray = ray.map(|v| v / depth);
    let radius = (pose.focal * display_radius / depth) as f32;
    if !pose.intersects_sphere(position, visible_radius(body, display_radius), 2.0) {
        return None;
    }
    let occluded = sphere_distance(pose.eye, ray, 1.0).is_some_and(|t| t < depth);
    let behind = pose.axes[2]
        .iter()
        .zip(position)
        .map(|(a, b)| a * b)
        .sum::<f64>()
        < 0.0;
    Some(Nearby {
        body,
        point,
        radius: moon_marker_radius(radius, major),
        position,
        display_radius,
        depth,
        major,
        behind,
        occluded,
    })
}

fn visible_radius(body: &Body, display_radius: f64) -> f64 {
    display_radius * f64::from(outer_radius_factor(body))
}

pub(super) fn outer_radius_factor(body: &Body) -> f32 {
    let halo = atmosphere(body.id).map_or(1.0, |air| 1.0 + 6.0 * air.optics[0]);
    bands(body.id).last().map_or(halo, |band| {
        halo.max(band[1] / (radius_m(body) / 1000.0) as f32)
    })
}

pub(in crate::file_manager::geography) struct Atmosphere {
    pub tint: [f32; 3],
    // Effective scale height in body radii, vertical optical depth, aerosol
    // anisotropy and proportion of cool scattering at forward angles.
    pub optics: [f32; 4],
}

pub(in crate::file_manager::geography) fn atmosphere(id: Id) -> Option<Atmosphere> {
    // Representative visible-light profiles, guided by NASA planetary fact
    // sheets and spacecraft color observations. For the giants the reference
    // radius is the visible cloud deck, not the pressure datum.
    let (tint, optics) = match id {
        299 => ([1.00, 0.91, 0.72], [0.0026, 0.16, 0.76, 0.0]),
        399 => ([0.35, 0.60, 1.00], [0.0013, 0.065, 0.38, 0.0]),
        499 => ([0.89, 0.59, 0.35], [0.0031, 0.042, 0.73, 0.96]),
        599 => ([0.73, 0.81, 0.92], [0.00085, 0.024, 0.60, 0.0]),
        606 => ([0.98, 0.63, 0.31], [0.017, 0.19, 0.73, 0.72]),
        699 => ([0.89, 0.81, 0.60], [0.0010, 0.033, 0.66, 0.0]),
        799 => ([0.62, 0.91, 0.94], [0.0014, 0.038, 0.62, 0.0]),
        801 => ([0.53, 0.72, 0.87], [0.010, 0.00045, 0.65, 0.0]),
        899 => ([0.52, 0.77, 0.94], [0.0009, 0.030, 0.58, 0.0]),
        999 => ([0.42, 0.64, 1.00], [0.032, 0.008, 0.77, 0.0]),
        _ => return None,
    };
    Some(Atmosphere { tint, optics })
}

pub(super) fn initial_zoom(id: Id) -> f64 {
    if bands(id).is_empty() { 1.0 } else { 0.42 }
}

fn radius_m(body: &Body) -> f64 {
    body.radius.unwrap_or(5.0) * 1000.0
}

#[derive(Clone)]
pub(in crate::file_manager::geography) struct Backdrop {
    pub axes: [[f32; 3]; 3],
    pub fov: [f32; 2],
    pub sun: ([f32; 3], f32),
    pub clock: [f32; 2],
    pub brightness: f32,
    pub assets: Option<std::sync::Arc<sky::Assets>>,
    pub constellations: bool,
}

impl Backdrop {
    pub(in crate::file_manager::geography) fn inertial_axes(&self) -> [[f32; 3]; 3] {
        self.axes
            .map(|[x, y, z]| [x, 0.917482 * y - 0.397777 * z, 0.397777 * y + 0.917482 * z])
    }

    pub(super) fn hover_star(&self, response: &egui::Response, exposed: impl Fn(Pos2) -> bool) {
        let Some(pointer) = response.hover_pos().filter(|point| exposed(*point)) else {
            return;
        };
        let Some(catalog) = self.assets.as_deref().and_then(sky::Assets::star_catalog) else {
            return;
        };
        let view = gpu::StarView {
            axes: self.inertial_axes(),
            focal: response.rect.width() / (2.0 * self.fov[0]),
        };
        if let Some(star) = view.pick(catalog, response.rect, pointer, exposed) {
            egui::Tooltip::for_widget(response)
                .at_pointer()
                .show(|ui| gpu::star_name(ui, star));
        }
    }
}

pub(in crate::file_manager::geography) struct Frame {
    pub id: Id,
    pub pose: Perspective,
    pub zoom: f32,
    pub color: [f32; 3],
    pub texture: Option<sky::Texture>,
    pub bands: &'static [[f32; 3]],
    pub gas: bool,
    pub sun: [f32; 3],
    pub clock: [f32; 2],
    pub ring_emphasis: f32,
}

impl Frame {
    pub fn new(
        body: &Body,
        camera: GlobeCamera,
        size: [f32; 2],
        texture: Option<sky::Texture>,
    ) -> Self {
        Self::from_pose(
            body,
            Perspective::new(&camera, size, radius_m(body)),
            camera.zoom as f32,
            texture,
        )
    }

    fn from_pose(
        body: &Body,
        mut pose: Perspective,
        zoom: f32,
        texture: Option<sky::Texture>,
    ) -> Self {
        let color = body.color().to_array();
        pose.radius_m = radius_m(body);
        Self {
            id: body.id,
            pose,
            zoom,
            color: [color[0], color[1], color[2]].map(|value| value as f32 / 255.0),
            texture,
            bands: bands(body.id),
            gas: matches!(body.id, 599 | 699 | 799 | 899),
            sun: [0.811, 0.324, 0.487],
            clock: [0.0; 2],
            ring_emphasis: 1.0,
        }
    }
}

pub(in crate::file_manager::geography) fn bands(id: Id) -> &'static [[f32; 3]] {
    // Inner and outer radii in km, with presentation opacity weights.
    // NASA PDS Ring-Moon Systems Node and USGS Gazetteer planetary ring tables.
    // Haumea's ring: Ortiz et al. (2017), https://doi.org/10.1038/nature24051.
    match id {
        599 => &[
            [92_000., 122_500., 0.09],
            [122_500., 129_000., 0.44],
            [129_000., 182_000., 0.11],
            [182_000., 226_000., 0.05],
        ],
        699 => &[
            [67_000., 74_510., 0.19],
            [74_658., 92_000., 0.43],
            [92_000., 117_580., 0.75],
            [122_170., 133_590., 0.57],
            [133_618., 136_775., 0.53],
            [140_180., 140_360., 0.42],
        ],
        799 => &[
            [41_820., 41_865., 0.27],
            [42_225., 42_245., 0.23],
            [44_710., 44_755., 0.33],
            [45_645., 45_680., 0.31],
            [47_160., 47_200., 0.43],
            [48_295., 48_345., 0.34],
            [50_990., 51_185., 0.73],
            [66_100., 69_900., 0.1],
        ],
        899 => &[
            [41_700., 42_220., 0.21],
            [53_150., 53_310., 0.34],
            [53_500., 57_200., 0.13],
            [61_900., 62_025., 0.5],
        ],
        2_136_108 => &[[2252., 2322., 0.48]],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jupiter_approach_preserves_io_scale_and_ends_on_the_focused_camera() {
        let mut camera = map_camera::lunar_camera();
        camera.zoom = 0.18;
        camera.center_lat = 72.0;
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 800.0));
        let planet = body(599).unwrap();
        let io = body(501).unwrap();
        let final_radius = camera.scale_px(viewport.width(), viewport.height());
        let body_pose = Perspective::new(&camera, viewport.size().into(), radius_m(planet));
        let world = project_moon(planet, io, [0.0, 421_800.0, 0.0], body_pose, viewport).unwrap();
        let mut previous: Option<Pos2> = None;
        for step in 0..=100 {
            let progress = f64::from(step) / 100.0;
            let lens = Approach {
                body: planet,
                camera,
                center: viewport.center(),
                radius: 28.0 * (final_radius / 28.0).powf(progress),
                progress,
            };
            let pose = lens.pose(viewport);
            let point = project_moon(planet, io, [0.0, 421_800.0, 0.0], pose, viewport).unwrap();
            if let Some(previous) = previous {
                assert!(point.point.distance(previous) < 8.0);
            }
            previous = Some(point.point);
            let distance_squared = pose.eye.iter().map(|v| v * v).sum::<f64>();
            assert!((pose.focal / (distance_squared - 1.0).sqrt() - lens.radius).abs() < 1e-7);
            if step == 100 {
                assert_eq!(point.point, world.point);
                assert_eq!(point.radius, world.radius);
                assert_eq!(pose.half_fov(), body_pose.half_fov());
            }
        }
    }

    #[test]
    fn saturn_keeps_minor_satellites_in_a_wide_view() {
        let mut camera = map_camera::lunar_camera();
        camera.zoom = 0.14;
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1800.0, 720.0));
        let saturn = body(699).unwrap();
        let pose = Perspective::new(&camera, rect.size().into(), radius_m(saturn));
        let visible = surroundings(saturn, pose, rect, 8_100.0);
        assert!(visible.len() > 48);
        assert!(visible.iter().any(|m| m.major));
        assert!(visible.iter().any(|m| !m.major && m.radius <= 1.0));
    }

    #[test]
    fn a_parent_uses_the_viewers_projection_and_its_own_ring_scale() {
        let camera = map_camera::lunar_camera();
        let europa = body(502).unwrap();
        let jupiter = body(599).unwrap();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let pose = Perspective::new(&camera, rect.size().into(), radius_m(europa));
        let neighbor =
            project_moon(europa, jupiter, [-671_100.0, 25_000.0, 0.0], pose, rect).unwrap();
        let frame = Frame::from_pose(jupiter, pose, 1.0, None);
        assert_eq!(frame.pose.half_fov(), pose.half_fov());
        assert_eq!(frame.pose.radius_m, radius_m(jupiter));
        assert_eq!(frame.bands, bands(jupiter.id));
        let radius = visible_radius(jupiter, neighbor.display_radius);
        let clip = projected_bounds(pose, rect, neighbor.position, radius);
        assert!(clip.is_positive() && clip.width() > neighbor.radius * 2.0);
    }

    #[test]
    fn major_moons_keep_relative_sizes_and_globe_occlusion() {
        let mut camera = map_camera::lunar_camera();
        camera.zoom = 0.18;
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 720.0));
        let jupiter = body(599).unwrap();
        let pose = Perspective::new(&camera, rect.size().into(), radius_m(jupiter));
        let io = body(501).unwrap();
        let europa = body(502).unwrap();
        let position = [0.0, 421_800.0, 0.0];
        let larger = project_moon(jupiter, io, position, pose, rect).unwrap();
        let smaller = project_moon(jupiter, europa, position, pose, rect).unwrap();
        assert!(larger.display_radius > smaller.display_radius && larger.radius >= smaller.radius);
        assert!(!larger.occluded);
        let behind = project_moon(jupiter, io, [-421_800.0, 0.0, 0.0], pose, rect).unwrap();
        assert!(behind.occluded && behind.behind);
    }
}
