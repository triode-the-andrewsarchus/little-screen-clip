//! Shared Earth and Moon input, with terrain clearance for the raised surface.
use super::{GlobeCamera, MAX_MAP_ZOOM, MIN_MAP_ZOOM, ProjectionMode, SkyState};
pub(super) use little_clip_core::geography::basemap::perspective::{EARTH_RADIUS_M, MOON_RADIUS_M};
use little_clip_core::geography::basemap::perspective::{Perspective, coordinates};
use little_clip_core::palette::AppPalette;

pub(super) const CONTROLS: &str = "Left-drag to pan. Scroll to zoom. Middle-drag up or down to tilt, and left or right to rotate.";

#[derive(Default)]
pub(super) struct MiddleDrag(Option<egui::Pos2>);

impl MiddleDrag {
    pub(super) fn update(&mut self, response: &egui::Response) -> Option<egui::Vec2> {
        let press = response.ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Middle,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            })
        });
        let press = press.filter(|&pos| {
            response.enabled()
                && response.interact_rect.contains(pos)
                && response.ctx.layer_id_at(pos) == Some(response.layer_id)
        });
        response
            .ctx
            .input(|input| self.motion(press, &input.events))
    }

    fn motion(&mut self, press: Option<egui::Pos2>, events: &[egui::Event]) -> Option<egui::Vec2> {
        if self.0.is_none() && press.is_none() {
            return None;
        }
        // A slow render can receive the final move and release together.
        // Keep the release position even when egui has ended widget dragging.
        use egui::{Event, PointerButton::Middle};
        let mut delta = egui::Vec2::ZERO;
        for event in events {
            match event {
                Event::PointerButton {
                    pos,
                    button: Middle,
                    pressed: true,
                    ..
                } => {
                    self.0 = press.filter(|origin| origin == pos);
                }
                Event::PointerButton {
                    pos,
                    button: Middle,
                    pressed: false,
                    ..
                } => {
                    if let Some(previous) = self.0.take() {
                        delta += *pos - previous;
                    }
                }
                Event::PointerMoved(pos) => {
                    if let Some(previous) = self.0.as_mut() {
                        delta += *pos - std::mem::replace(previous, *pos);
                    }
                }
                Event::PointerGone | Event::WindowFocused(false) => self.0 = None,
                _ => {}
            }
        }
        Some(delta)
    }
}

pub(super) fn navigate(
    camera: &mut GlobeCamera,
    response: &egui::Response,
    perspective_radius_m: Option<f64>,
    middle: &mut MiddleDrag,
) {
    let size = [response.rect.width(), response.rect.height()];
    if let Some(delta) = middle.update(response) {
        if delta != egui::Vec2::ZERO {
            camera.orbit.drag(delta.x, delta.y);
            response.ctx.request_repaint();
        }
    } else if response.dragged_by(egui::PointerButton::Primary) {
        let delta = response.drag_delta();
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
        update_ground(camera, size, EARTH_RADIUS_M, |lat, lon| {
            tiles.height_at(lat, lon)
        });
    }
}

pub(super) fn update_ground(
    camera: &mut GlobeCamera,
    size: [f32; 2],
    radius: f64,
    height: impl Fn(f64, f64) -> Option<f64>,
) {
    camera.orbit.target_height_m = height(camera.center_lat, camera.center_lon).unwrap_or(0.0);
    camera.orbit.eye_ground_m = 0.0;
    let pose = Perspective::new(camera, size, radius);
    let (lat, lon) = coordinates(pose.eye);
    camera.orbit.eye_ground_m = height(lat, lon).unwrap_or(0.0);
}

#[derive(Clone, Copy)]
pub(super) struct Draped<'a> {
    surface: Option<&'a super::gpu::Surface>,
    camera: Perspective,
    size: [f32; 2],
}

#[derive(Clone, Copy)]
pub(super) struct Anchor {
    pub pixel: [f32; 2],
    surface_point: Option<[f64; 3]>,
}

impl<'a> Draped<'a> {
    pub fn new(
        camera: &GlobeCamera,
        surface: Option<&'a super::gpu::Surface>,
        size: [f32; 2],
    ) -> Option<Self> {
        camera.orbit.active().then(|| Self {
            surface,
            camera: Perspective::new(camera, size, EARTH_RADIUS_M),
            size,
        })
    }

    pub fn project(self, lat: f64, lon: f64) -> Option<[f32; 2]> {
        let anchor = self.anchor(lat, lon)?;
        self.unobscured(anchor).then_some(anchor.pixel)
    }

    pub fn anchor(self, lat: f64, lon: f64) -> Option<Anchor> {
        if let Some(surface) = self.surface {
            if !surface.intersects_view(self.camera, lat, lon, 0.0) {
                return None;
            }
            if let Some((pixel, point)) = surface.project(self.camera, lat, lon) {
                let [x, y] = pixel;
                // A small guard keeps strokes and labels that overlap the edge.
                return ((-24.0..=self.size[0] + 24.0).contains(&x)
                    && (-24.0..=self.size[1] + 24.0).contains(&y))
                .then_some(Anchor {
                    pixel,
                    surface_point: Some(point),
                });
            }
        }
        Some(Anchor {
            pixel: self.camera.project(lat, lon, 0.0)?,
            surface_point: None,
        })
    }

    pub fn unobscured(self, anchor: Anchor) -> bool {
        self.surface
            .zip(anchor.surface_point)
            .is_none_or(|(surface, point)| !surface.occluded(self.camera.eye, point))
    }

    pub fn intersects_leg(self, a: [f64; 2], b: [f64; 2]) -> bool {
        self.surface.is_none_or(|surface| {
            let radius = ((b[0] - a[0]).abs() + (b[1] - a[1]).abs()).to_radians() * 0.5;
            surface.intersects_view(
                self.camera,
                a[0].midpoint(b[0]),
                a[1].midpoint(b[1]),
                radius,
            )
        })
    }

    pub fn mesh_step(self) -> f64 {
        self.surface.map_or(1.0, |surface| surface.mesh_step_deg)
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
    use clip_ui::buttons::ButtonRole::Secondary;
    let earth = ui
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
    earth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_drag_keeps_motion_before_a_coalesced_release() {
        let mut drag = MiddleDrag::default();
        let point = egui::pos2(100.0, 200.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let begin = button(point, true);
        let release = egui::pos2(165.0, 90.0);
        assert_eq!(drag.motion(Some(point), &[begin]), Some(egui::Vec2::ZERO));
        assert_eq!(
            drag.motion(
                None,
                &[
                    egui::Event::PointerMoved(egui::pos2(130.0, 150.0)),
                    button(release, false),
                    egui::Event::PointerMoved(egui::pos2(0.0, 0.0)),
                ],
            ),
            Some(release - point)
        );
        assert!(drag.0.is_none());
    }
}
