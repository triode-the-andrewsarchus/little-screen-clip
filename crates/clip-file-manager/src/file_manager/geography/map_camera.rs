//! Shared Earth and Moon input, with terrain clearance for the raised surface.
use super::{GlobeCamera, MAX_MAP_ZOOM, MIN_MAP_ZOOM, ProjectionMode, SkyState};
pub(super) use little_clip_core::geography::basemap::perspective::{EARTH_RADIUS_M, MOON_RADIUS_M};
use little_clip_core::geography::basemap::perspective::{Perspective, coordinates};
use little_clip_core::palette::AppPalette;

pub(super) const CONTROLS: &str = "Left-drag to pan. Scroll to zoom. Middle-drag up or down to tilt, and left or right to rotate.";

pub(super) fn navigate(
    camera: &mut GlobeCamera,
    response: &egui::Response,
    perspective_radius_m: Option<f64>,
) {
    let size = [response.rect.width(), response.rect.height()];
    let delta = response.drag_delta();
    if response.dragged_by(egui::PointerButton::Middle) {
        camera.orbit.drag(delta.x, delta.y);
    } else if response.dragged_by(egui::PointerButton::Primary) {
        if let Some(radius_m) = perspective_radius_m {
            camera.pan_perspective_by(delta.into(), size, radius_m);
        } else {
            camera.pan_by(delta.x, delta.y, size[0], size[1]);
        }
    }
    if response.hovered() {
        let scroll = response.ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.01 {
            let factor = (scroll * 0.004).exp() as f64;
            camera.zoom = (camera.zoom * factor).clamp(MIN_MAP_ZOOM, MAX_MAP_ZOOM);
            response.ctx.request_repaint();
        }
    }
    camera.constrain_view(size[0], size[1]);
}

pub(super) fn follow_ground(camera: &mut GlobeCamera, tiles: &super::tiles::Tiles, size: [f32; 2]) {
    if camera.orbit.active() {
        camera.orbit.target_height_m = tiles
            .height_at(camera.center_lat, camera.center_lon)
            .unwrap_or(0.0);
        camera.orbit.eye_ground_m = 0.0;
        let pose = Perspective::new(camera, size, EARTH_RADIUS_M);
        let (lat, lon) = coordinates(pose.eye);
        camera.orbit.eye_ground_m = tiles.height_at(lat, lon).unwrap_or(0.0);
    }
}

#[derive(Clone, Copy)]
pub(super) struct Draped<'a> {
    tiles: &'a super::tiles::Tiles,
    camera: Perspective,
    reference_height: f64,
}

impl<'a> Draped<'a> {
    pub fn new(
        camera: &GlobeCamera,
        tiles: &'a super::tiles::Tiles,
        size: [f32; 2],
    ) -> Option<Self> {
        camera.orbit.active().then(|| Self {
            tiles,
            camera: Perspective::new(camera, size, EARTH_RADIUS_M),
            reference_height: camera.orbit.target_height_m,
        })
    }

    pub fn project(self, lat: f64, lon: f64) -> Option<[f32; 2]> {
        self.camera.project(
            lat,
            lon,
            self.tiles
                .height_at(lat, lon)
                .unwrap_or(self.reference_height),
        )
    }
}

pub(super) fn lunar_camera() -> GlobeCamera {
    GlobeCamera {
        center_lat: 0.0,
        center_lon: 0.0,
        zoom: 1.0,
        mode: ProjectionMode::Globe3D,
        ..GlobeCamera::default()
    }
}

pub(super) fn lunar_toolbar(
    ui: &mut egui::Ui,
    palette: &AppPalette,
    camera: &mut GlobeCamera,
    sky: &mut SkyState,
) -> bool {
    let mut earth = false;
    ui.horizontal_wrapped(|ui| {
        use clip_ui::buttons::ButtonRole::Secondary;
        earth = ui
            .add(Secondary.button("Return to Earth", palette).compact())
            .clicked();
        if ui
            .add(Secondary.button("Reset Moon view", palette).compact())
            .clicked()
        {
            *camera = lunar_camera();
        }
        ui.separator();
        ui.checkbox(&mut sky.visible, "Sky and planets");
        ui.checkbox(&mut sky.lighting, "Day and night");
        if sky
            .assets
            .as_ref()
            .is_some_and(|assets| assets.star_catalog().is_some())
        {
            ui.add_enabled(
                sky.visible,
                egui::Checkbox::new(&mut sky.constellations, "Constellations"),
            );
        }
    });
    earth
}
