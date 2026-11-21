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
use std::{collections::HashMap, path::Path, time::Duration};

pub(in crate::file_manager::geography) struct World {
    pub body: &'static Body,
    pub camera: GlobeCamera,
    middle: map_camera::MiddleDrag,
    inspected: bool,
    job: Option<Task<LocalAssets>>,
    assets: Option<assets::Assets>,
    neighbors: HashMap<Id, sky::Texture>,
    pub error: Option<String>,
}

struct LocalAssets {
    focused: Result<Option<assets::Assets>, String>,
    neighbors: HashMap<Id, sky::Texture>,
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
            neighbors: HashMap::new(),
            error: None,
        }
    }

    pub fn navigate(&mut self, response: &egui::Response) {
        map_camera::navigate(
            &mut self.camera,
            response,
            Some(radius_m(self.body)),
            &mut self.middle,
        );
    }

    pub fn reload(&mut self) {
        self.job = None;
        self.inspected = false;
        self.error = None;
    }

    pub fn poll(&mut self, ctx: &egui::Context, root: &Path) {
        if let Some(result) = self.job.as_ref().and_then(Task::poll) {
            self.job = None;
            self.inspected = true;
            match result {
                Ok(local) => {
                    self.neighbors = local.neighbors;
                    match local.focused {
                        Ok(assets) => self.assets = assets,
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(error) => self.error = Some(format!("World asset worker: {error}")),
            }
        }
        if !self.inspected && self.job.is_none() {
            let root = root.to_owned();
            let body = self.body;
            self.job = Some(Task::start(
                super::super::super::geography_downloader::asset_workers(),
                Duration::from_secs(30),
                move |cancel| LocalAssets {
                    focused: assets::Assets::load(&root, body.id),
                    neighbors: assets::neighboring_maps(&root, body, &cancel),
                },
            ));
        }
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    pub fn pending(&self) -> bool {
        !self.inspected
    }

    pub fn installed(&self) -> bool {
        self.assets.is_some()
    }

    pub fn paint(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        time: DateTime<Utc>,
        stars: Option<std::sync::Arc<sky::Assets>>,
    ) -> Result<Option<Id>, String> {
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, Color32::from_rgb(7, 10, 17));
        if stars.is_none() {
            super::paint_stars(&painter, rect);
        }
        let mut frame = Frame::new(self.body, self.camera, rect.size().into(), self.texture());
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
        let moons = surroundings(self.body, frame.pose, rect, day);
        let pose = frame.pose;
        let clock = frame.clock;
        gpu::paint_space(ui, rect, lights)?;
        let paint_side = |behind| {
            for moon in moons.iter().filter(|moon| moon.behind == behind) {
                moon.paint(
                    ui,
                    pose,
                    rect,
                    sun.0,
                    clock,
                    self.neighbors.get(&moon.body.id),
                )?;
            }
            Ok::<_, String>(())
        };
        paint_side(true)?;
        gpu::paint_world(ui, rect, frame)?;
        paint_side(false)?;
        for moon in moons
            .iter()
            .filter(|moon| !moon.occluded && moon.radius >= 4.5)
        {
            moon.label(&painter);
        }
        let target = pick_moon(&moons, pose, response).or_else(|| pick_sun(pose, sun, response));
        Ok(target.filter(|_| response.clicked()))
    }

    pub fn texture(&self) -> Option<sky::Texture> {
        self.assets.as_ref()?.texture.clone()
    }

    pub fn ephemeris(&self) -> Option<&[[f64; 3]]> {
        self.assets.as_ref()?.ephemeris.as_deref()
    }
}

struct Nearby {
    body: &'static Body,
    point: Pos2,
    radius: f32,
    position: [f64; 3],
    display_radius: f64,
    depth: f64,
    behind: bool,
    occluded: bool,
}

impl Nearby {
    fn paint(
        &self,
        ui: &egui::Ui,
        pose: Perspective,
        canvas: Rect,
        sun: [f32; 3],
        clock: [f32; 2],
        texture: Option<&sky::Texture>,
    ) -> Result<(), String> {
        if self.radius < 2.0 && bands(self.body.id).is_empty() {
            ui.painter_at(canvas)
                .circle_filled(self.point, self.radius, self.body.color());
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
        let clip = projected_bounds(pose, canvas, self.position, bound);
        if !clip.is_positive() {
            return Ok(());
        }
        gpu::paint_world_clipped(ui, canvas, clip, frame)
    }

    fn label(&self, painter: &egui::Painter) {
        let text = painter.layout_no_wrap(
            self.body.name.to_owned(),
            FontId::proportional(11.0),
            Color32::from_rgb(206, 217, 234),
        );
        let position = self.point + egui::vec2(self.radius + 4.0, -text.size().y * 0.5);
        let rect = Rect::from_min_size(position, text.size()).expand2(egui::vec2(3.0, 1.0));
        if painter.clip_rect().contains_rect(rect) {
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

fn pick_moon(moons: &[Nearby], pose: Perspective, response: &egui::Response) -> Option<Id> {
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
    let hit = moons
        .iter()
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
    visible.sort_by(|a, b| b.radius.total_cmp(&a.radius));
    visible.truncate(48);
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
    let orbit_km = orbit.iter().map(|v| v * v).sum::<f64>().sqrt();
    let relative_distance = orbit_km / primary_km;
    let radius_ratio = moon.radius.unwrap_or(3.0) / primary_km;
    let distance =
        (1.0 + 0.66 * (relative_distance - 1.0).max(0.0).powf(0.67)).max(1.0 + radius_ratio * 1.4);
    let position = orbit.map(|v| v * distance / orbit_km);
    let offset = pose.project_position(position)?;
    let point = rect.min + egui::vec2(offset[0], offset[1]);
    let ray = std::array::from_fn(|i| position[i] - pose.eye[i]);
    let depth = ray.iter().map(|v| v * v).sum::<f64>().sqrt();
    let ray = ray.map(|v| v / depth);
    let display_radius = (radius_ratio * 3.0).min(distance * 0.32);
    let radius = (pose.focal * display_radius / depth) as f32;
    if radius < 0.26 || !pose.intersects_sphere(position, visible_radius(moon, display_radius), 2.0)
    {
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
        body: moon,
        point,
        radius: radius.max(0.9),
        position,
        display_radius,
        depth,
        behind,
        occluded,
    })
}

fn visible_radius(body: &Body, display_radius: f64) -> f64 {
    display_radius
        * bands(body.id)
            .last()
            .map_or(1.0, |band| f64::from(band[1]) / (radius_m(body) / 1000.0))
}

pub(super) fn initial_zoom(id: Id) -> f64 {
    if bands(id).is_empty() { 1.0 } else { 0.42 }
}

fn radius_m(body: &Body) -> f64 {
    body.radius.unwrap_or(5.0) * 1000.0
}

pub(in crate::file_manager::geography) struct Backdrop {
    pub axes: [[f32; 3]; 3],
    pub fov: [f32; 2],
    pub sun: ([f32; 3], f32),
    pub clock: [f32; 2],
    pub brightness: f32,
    pub assets: Option<std::sync::Arc<sky::Assets>>,
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
        camera.zoom = initial_zoom(599);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 720.0));
        let jupiter = body(599).unwrap();
        let pose = Perspective::new(&camera, rect.size().into(), radius_m(jupiter));
        let io = body(501).unwrap();
        let europa = body(502).unwrap();
        let position = [0.0, 421_800.0, 0.0];
        let larger = project_moon(jupiter, io, position, pose, rect).unwrap();
        let smaller = project_moon(jupiter, europa, position, pose, rect).unwrap();
        assert!(larger.radius > smaller.radius && larger.radius > 4.0);
        assert!(!larger.occluded);
        let behind = project_moon(jupiter, io, [-421_800.0, 0.0, 0.0], pose, rect).unwrap();
        assert!(behind.occluded && behind.behind);
    }
}
