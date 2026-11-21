mod figures;
pub(super) mod inspection;

use crate::file_manager::geography::sky::{self, catalog::Star};
use eframe::egui;
use eframe::glow::{self, HasContext};
use little_clip_core::geography::basemap::{
    BasemapStyle, GlobeCamera,
    perspective::{EARTH_RADIUS_M, MOON_RADIUS_M, Perspective, sphere_distance},
};
use std::sync::{Arc, Weak};

#[derive(Default)]
pub(super) struct SkyRenderer {
    source: Weak<sky::Assets>,
    program: Option<glow::Program>,
    stars: Option<glow::Texture>,
    moon: Option<glow::Texture>,
    night: Option<glow::Texture>,
    figures: Option<figures::Figures>,
    figures_failed: bool,
    failed: bool,
}

struct SkyProjection {
    basis: [[f32; 3]; 3],
    focal: f32,
    radius: f32,
    distance: f32,
    eye: [f32; 3],
    sphere_eye: Option<[f32; 3]>,
}

impl SkyProjection {
    fn new(camera: GlobeCamera, [width, height]: [f32; 2]) -> Self {
        if camera.orbit.active() {
            let pose = Perspective::new(&camera, [width, height], EARTH_RADIUS_M);
            return Self::perspective(
                pose,
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                [0.0; 3],
            );
        }
        let (sin_lat, cos_lat) = (camera.center_lat as f32).to_radians().sin_cos();
        let (sin_lon, cos_lon) = (camera.center_lon as f32).to_radians().sin_cos();
        let focal = width.min(height) * 0.95;
        let radius = camera.scale_px(width, height) as f32;
        let distance = 6378.137 * (1.0 + (focal / radius).powi(2)).sqrt();
        let outward = [cos_lon * cos_lat, sin_lon * cos_lat, sin_lat];
        Self {
            basis: [
                [-sin_lon, cos_lon, 0.0],
                [-cos_lon * sin_lat, -sin_lon * sin_lat, cos_lat],
                outward,
            ],
            focal,
            radius,
            distance,
            eye: outward.map(|value| value * distance),
            sphere_eye: None,
        }
    }

    fn perspective(pose: Perspective, body_fixed: [[f32; 3]; 3], center: [f32; 3]) -> Self {
        let to_fixed = |v: [f64; 3]| {
            std::array::from_fn(|i| {
                (0..3)
                    .map(|axis| body_fixed[axis][i] * v[axis] as f32)
                    .sum::<f32>()
            })
        };
        let eye = to_fixed(pose.eye);
        Self {
            basis: pose.axes.map(to_fixed),
            focal: pose.focal as f32,
            radius: 0.0,
            distance: 0.0,
            eye: std::array::from_fn(|i| center[i] + eye[i] * (pose.radius_m / 1000.0) as f32),
            sphere_eye: Some(eye),
        }
    }

    fn for_frame(frame: &super::Frame, scene: &sky::Scene) -> Self {
        if frame.moon_view {
            Self::perspective(
                Perspective::new(&frame.camera, frame.size, MOON_RADIUS_M),
                moon_axes(scene),
                scene.moon,
            )
        } else {
            Self::new(frame.camera, frame.size)
        }
    }

    fn exposed(&self, offset: egui::Vec2) -> bool {
        match self.sphere_eye {
            Some(eye) => sphere_distance(
                eye.map(f64::from),
                self.inertial_ray(self.basis, offset).map(f64::from),
                1.0,
            )
            .is_none(),
            None => offset.length_sq() > self.radius.powi(2),
        }
    }

    fn earth_at(&self, offset: egui::Vec2) -> bool {
        // Earth is behind the Moon anywhere the two disks overlap.
        self.exposed(offset)
            && sphere_distance(
                self.eye.map(f64::from),
                self.inertial_ray(self.basis, offset).map(f64::from),
                6378.137,
            )
            .is_some()
    }

    fn project(&self, position: [f32; 3]) -> Option<egui::Vec2> {
        let relative = subtract(position, self.eye);
        let [x, y, z] = self.basis.map(|axis| dot(axis, relative));
        let depth = -z;
        if depth <= 0.0 {
            return None;
        }
        let offset = egui::vec2(x, -y) * (self.focal / depth);
        self.exposed(offset).then_some(offset)
    }

    fn inertial_axes(&self, rows: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
        self.basis
            .map(|axis| std::array::from_fn(|i| dot(axis, [rows[0][i], rows[1][i], rows[2][i]])))
    }

    fn inertial_ray(&self, axes: [[f32; 3]; 3], offset: egui::Vec2) -> [f32; 3] {
        inspection::View {
            axes,
            focal: self.focal,
        }
        .ray(offset)
    }

    fn fov(&self, [width, height]: [f32; 2]) -> [f32; 2] {
        [width, height].map(|size| size / (2.0 * self.focal))
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn cross([ax, ay, az]: [f32; 3], [bx, by, bz]: [f32; 3]) -> [f32; 3] {
    [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx]
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    v.map(|x| x / length)
}

fn moon_axes(scene: &sky::Scene) -> [[f32; 3]; 3] {
    let pole = [-0.0000355, -0.39813, 0.91733];
    let north = unit(scene.inertial_rows.map(|row| dot(row, pole)));
    let projected = dot(scene.moon, north);
    let front = unit(std::array::from_fn(|i| {
        -scene.moon[i] + projected * north[i]
    }));
    [front, cross(north, front), north]
}

pub(in super::super) fn moon_at(
    frame: &super::Frame,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> bool {
    let Some(scene) = frame.sky.as_ref() else {
        return false;
    };
    if frame.moon_view
        || !frame.show_sky
        || frame.camera.globe_blend() < 1.0
        || !rect.contains(pointer)
    {
        return false;
    }
    let pose = SkyProjection::for_frame(frame, scene);
    let offset = pointer - rect.center();
    pose.exposed(offset) && luminary_at(frame, scene, &pose, offset, false)
}

fn luminary_at(
    frame: &super::Frame,
    scene: &sky::Scene,
    pose: &SkyProjection,
    offset: egui::Vec2,
    is_sun: bool,
) -> bool {
    let ray = pose.inertial_ray(pose.basis, offset);
    let (body, disk_radius) = luminary(frame, scene, pose, is_sun);
    let across = cross(ray, body);
    let radius = disk_radius.max(dot(body, body).sqrt() * 5.0 / pose.focal);
    dot(ray, body) > 0.0 && dot(across, across) <= radius * radius
}

fn luminary(
    frame: &super::Frame,
    scene: &sky::Scene,
    pose: &SkyProjection,
    is_sun: bool,
) -> ([f32; 3], f32) {
    let sun = subtract(scene.sun, pose.eye);
    let moon = subtract(scene.moon, pose.eye);
    let (body, body_radius) = if is_sun {
        (sun, 695_700.0)
    } else {
        (moon, 1737.4)
    };
    let scale = if frame.readable_sky {
        let [a, b] = [unit(sun), unit(moon)];
        let separation = dot(cross(a, b), cross(a, b)).sqrt().atan2(dot(a, b));
        let radii = 695_700.0 / dot(sun, sun).sqrt() + 1737.4 / dot(moon, moon).sqrt();
        let direction = if is_sun { a } else { b };
        let clearance = std::array::from_fn(|i| {
            pose.eye[i] + direction[i] * (-dot(pose.eye, direction)).max(0.0)
        });
        let above_limb = dot(clearance, clearance).sqrt() - 6378.137;
        let smooth = |x: f32| {
            let t = x.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let enlargement = if frame.moon_view {
            1.0
        } else {
            smooth((separation / radii - 16.0) / 1.5)
        };
        let zoom = if frame.moon_view {
            1.0
        } else {
            readable_disk_zoom(frame.camera.zoom)
        };
        1.0 + 15.0 * zoom * enlargement * smooth(above_limb / 600.0)
    } else {
        1.0
    };
    (body, body_radius * scale)
}

pub(in super::super) fn resolved_planets(frame: &super::Frame) -> [bool; 7] {
    let Some(scene) = frame.sky.as_ref().filter(|_| frame.show_sky) else {
        return [false; 7];
    };
    let pose = SkyProjection::for_frame(frame, scene);
    let viewport = egui::Rect::from_center_size(egui::Pos2::ZERO, frame.size.into());
    std::array::from_fn(|i| {
        let relative = subtract(scene.planets[i], pose.eye);
        let distance = dot(relative, relative).sqrt();
        let Some(offset) = pose.project(scene.planets[i]) else {
            return false;
        };
        let body = super::super::solar::body(sky::PLANET_IDS[i]);
        let radius = body.and_then(|planet| planet.radius).unwrap_or_default() as f32;
        let direction = relative.map(|x| x / distance);
        let limb = pose.eye.map(|value| value / 6378.137);
        let t = (-dot(limb, direction)).max(0.0);
        let nearest = std::array::from_fn(|axis| limb[axis] + direction[axis] * t);
        let height = (dot(nearest, nearest).sqrt() - 1.0) * 6378.137;
        let growth = if frame.readable_sky {
            let weight = (height / 250.0).clamp(0.0, 1.0);
            1.0 + 3.0 * weight * weight * (3.0 - 2.0 * weight)
        } else {
            1.0
        };
        let pixels = radius * growth * pose.focal / distance;
        pixels >= 1.35 && viewport.expand(pixels).contains(offset.to_pos2())
    })
}

pub(super) fn animated_sun_visible(frame: &super::Frame) -> bool {
    let Some(scene) = &frame.sky else {
        return false;
    };
    if !frame.moon_view && (!frame.show_sky || frame.camera.globe_blend() == 0.0) {
        return false;
    }
    let pose = SkyProjection::for_frame(frame, scene);
    let (sun, radius) = luminary(frame, scene, &pose, true);
    let view = inspection::View {
        axes: pose.basis,
        focal: pose.focal,
    };
    let Some(offset) = view.offset(sun) else {
        return false;
    };
    let radius_px = radius * pose.focal / -dot(sun, pose.basis[2]);
    // Only schedule animated repaints when surface or coronal detail resolves.
    // Sample the limb too, so a horizon or the viewport can cover the center.
    let viewport = egui::Rect::from_center_size(egui::Pos2::ZERO, frame.size.into());
    radius_px >= 4.0
        && viewport.expand(radius_px * 1.3).contains(offset.to_pos2())
        && [
            egui::Vec2::ZERO,
            egui::Vec2::X,
            -egui::Vec2::X,
            egui::Vec2::Y,
            -egui::Vec2::Y,
        ]
        .iter()
        .any(|edge| pose.exposed(offset + *edge * radius_px))
}

pub(in super::super) fn earth_at(
    frame: &super::Frame,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> bool {
    frame.moon_view
        && frame.show_sky
        && rect.contains(pointer)
        && frame.sky.as_ref().is_some_and(|scene| {
            SkyProjection::for_frame(frame, scene).earth_at(pointer - rect.center())
        })
}

pub(in super::super) enum SkyObject<'a> {
    Body(&'static str),
    Star(&'a Star),
}

pub(in super::super) fn sky_object_at(
    frame: &super::Frame,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<SkyObject<'_>> {
    if !frame.show_sky || frame.camera.globe_blend() < 1.0 || !rect.contains(pointer) {
        return None;
    }
    let scene = frame.sky.as_ref()?;
    let projection = SkyProjection::for_frame(frame, scene);
    let center = rect.center();
    if !projection.exposed(pointer - center) {
        return None;
    }
    if moon_at(frame, rect, pointer) {
        return Some(SkyObject::Body("Moon"));
    }
    if earth_at(frame, rect, pointer) {
        return Some(SkyObject::Body("Earth"));
    }
    if luminary_at(frame, scene, &projection, pointer - center, true) {
        return Some(SkyObject::Body("Sun"));
    }
    let body = scene
        .bodies()
        .filter(|(name, _)| !matches!(*name, "Sun" | "Moon"))
        .filter_map(|(name, position)| {
            let point = center + projection.project(position)?;
            let distance = point.distance_sq(pointer);
            (rect.contains(point) && distance <= 14.0_f32.powi(2)).then_some((name, distance))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(name, _)| SkyObject::Body(name));
    body.or_else(|| find_star(scene, &projection, rect, pointer).map(SkyObject::Star))
}

fn find_star<'a>(
    scene: &'a sky::Scene,
    projection: &SkyProjection,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<&'a Star> {
    inspection::View {
        axes: projection.inertial_axes(scene.inertial_rows),
        focal: projection.focal,
    }
    .pick(scene.assets.star_catalog()?, rect, pointer, |position| {
        projection.exposed(position - rect.center())
    })
}

impl SkyRenderer {
    pub(super) unsafe fn paint_frame(
        &mut self,
        gl: &glow::Context,
        frame: &super::Frame,
        earth: [Option<glow::Texture>; 2],
        planet_maps: [Option<glow::Texture>; 7],
    ) {
        if self.failed || (!frame.show_sky && !frame.moon_view) {
            return;
        }
        let Some(scene) = &frame.sky else { return };
        // SAFETY: called during the owner's paint callback with its context current.
        if let Err(error) = unsafe { self.paint(gl, scene, frame, earth, planet_maps) } {
            self.failed = true;
            tracing::warn!(%error, "Sky renderer unavailable");
        }
    }

    unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        scene: &sky::Scene,
        frame: &super::Frame,
        earth: [Option<glow::Texture>; 2],
        planet_maps: [Option<glow::Texture>; 7],
    ) -> Result<(), String> {
        let camera = frame.camera;
        let opacity = if frame.moon_view {
            1.0
        } else {
            camera.globe_blend() as f32
        };
        if opacity <= 0.0 {
            return Ok(());
        }
        // SAFETY: this runs inside the owner callback with its GL context current.
        unsafe {
            let p = match self.program {
                Some(program) => program,
                None => {
                    let program = super::program(
                        gl,
                        include_str!("../sky.vert"),
                        &super::with_shadow(include_str!("../sky.frag")),
                    )?;
                    self.program = Some(program);
                    program
                }
            };
            self.prepare(gl, &scene.assets);
            if self.stars.is_none() {
                return Ok(());
            }
            let projection = SkyProjection::for_frame(frame, scene);
            let [half_x, half_y] = projection.fov(frame.size);

            gl.use_program(Some(p));
            gl.uniform_matrix_3_f32_slice(
                gl.get_uniform_location(p, "u_inertial_from_fixed").as_ref(),
                false,
                scene.inertial_rows.as_flattened(),
            );
            gl.uniform_matrix_3_f32_slice(
                gl.get_uniform_location(p, "u_camera_basis").as_ref(),
                false,
                projection.basis.as_flattened(),
            );
            gl.uniform_2_f32(gl.get_uniform_location(p, "u_fov").as_ref(), half_x, half_y);
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_camera_distance").as_ref(),
                projection.distance,
            );
            let loc = |name| gl.get_uniform_location(p, name);
            gl.uniform_1_i32(
                loc("u_sky_eye_override").as_ref(),
                i32::from(projection.sphere_eye.is_some()),
            );
            gl.uniform_3_f32_slice(loc("u_sky_eye").as_ref(), &projection.eye);
            gl.uniform_1_i32(loc("u_moon_view").as_ref(), i32::from(frame.moon_view));
            gl.uniform_1_i32(loc("u_moon_only").as_ref(), 0);
            if frame.moon_view {
                gl.uniform_3_f32_slice(
                    loc("u_moon_eye").as_ref(),
                    &projection.sphere_eye.unwrap_or_default(),
                );
            }
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_opacity").as_ref(), opacity);
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_star_trail").as_ref(),
                frame.star_trail,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(p, "u_readable_sky").as_ref(),
                i32::from(frame.readable_sky),
            );
            gl.uniform_1_f32(
                loc("u_readable_disk_zoom").as_ref(),
                if frame.moon_view {
                    1.0
                } else {
                    readable_disk_zoom(camera.zoom)
                },
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_atmosphere").as_ref(),
                if frame.moon_view {
                    0.0
                } else {
                    atmosphere(camera, frame.style)
                },
            );
            gl.uniform_3_f32_slice(loc("u_sun").as_ref(), &scene.sun);
            gl.uniform_2_f32_slice(
                loc("u_solar_clock").as_ref(),
                &super::super::solar::clock(scene.time, frame.animation_seconds),
            );
            gl.uniform_3_f32_slice(gl.get_uniform_location(p, "u_moon").as_ref(), &scene.moon);
            gl.uniform_3_f32_slice(
                gl.get_uniform_location(p, "u_planets[0]").as_ref(),
                scene.planets.as_flattened(),
            );
            let mut installed = 0;
            for (i, texture) in planet_maps
                .into_iter()
                .enumerate()
                .filter(|(_, texture)| texture.is_some())
            {
                bind(gl, p, &format!("u_planet_maps[{i}]"), 6 + i as u32, texture);
                installed |= 1 << i;
            }
            gl.uniform_1_i32(loc("u_planet_maps_mask").as_ref(), installed);
            self.lunar_uniforms(gl, p, frame, None);
            bind(gl, p, "u_stars", 3, self.stars);
            if frame.moon_view {
                self.bind_earth(gl, p, frame.earth_lighting, earth);
            }
            if frame.show_sky {
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                if frame.show_constellations {
                    let vao = gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING);
                    self.paint_figures(
                        gl,
                        &scene.assets,
                        projection.inertial_axes(scene.inertial_rows),
                        [half_x, half_y],
                        opacity,
                    );
                    gl.bind_vertex_array(vao);
                }
            }
            if frame.moon_view && frame.terrain.is_none() {
                gl.use_program(Some(p));
                gl.uniform_1_i32(loc("u_moon_only").as_ref(), 1);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.active_texture(glow::TEXTURE0);
        }
        Ok(())
    }

    pub(super) unsafe fn lunar_uniforms(
        &self,
        gl: &glow::Context,
        p: glow::Program,
        frame: &super::Frame,
        fallback: Option<glow::Texture>,
    ) {
        let scene = frame.sky.as_ref();
        let phase = if scene.is_some() && frame.earth_lighting {
            zoom_fade(frame.camera.zoom, 2.5, 30.0)
        } else {
            0.0
        };
        let basis = scene.map_or(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            moon_axes,
        );
        let sun = scene.map_or([150_000_000.0, 0.0, 0.0], |scene| scene.lunar_sun);
        let earth = scene.map_or([-380_000.0, 0.0, 0.0], |scene| scene.lunar_earth);
        let albedo = self.moon.filter(|_| scene.is_some()).or(fallback);
        // SAFETY: the current context owns the bound program and retained lunar texture.
        unsafe {
            let loc = |name| gl.get_uniform_location(p, name);
            gl.uniform_matrix_3_f32_slice(
                loc("u_lunar_basis").as_ref(),
                false,
                basis.as_flattened(),
            );
            gl.uniform_3_f32_slice(loc("u_lunar_sun").as_ref(), &sun);
            gl.uniform_3_f32_slice(loc("u_lunar_earth").as_ref(), &earth);
            gl.uniform_1_f32(loc("u_longitude").as_ref(), frame.camera.center_lon as f32);
            gl.uniform_1_f32(loc("u_moon_phase").as_ref(), phase);
            gl.uniform_1_i32(
                loc("u_has_lunar_albedo").as_ref(),
                i32::from(albedo.is_some()),
            );
            bind(gl, p, "u_lunar_albedo", 4, albedo);
        }
    }

    unsafe fn bind_earth(
        &self,
        gl: &glow::Context,
        program: glow::Program,
        lighting: bool,
        [west, east]: [Option<glow::Texture>; 2],
    ) {
        // SAFETY: the sky program is active and all textures share this context.
        unsafe {
            let uniform = |name| gl.get_uniform_location(program, name);
            gl.uniform_2_i32(
                uniform("u_earth_tiles").as_ref(),
                i32::from(west.is_some()),
                i32::from(east.is_some()),
            );
            gl.uniform_1_i32(uniform("u_light_earth_disk").as_ref(), i32::from(lighting));
            gl.uniform_1_i32(
                uniform("u_earth_night_available").as_ref(),
                i32::from(self.night.is_some()),
            );
            bind(gl, program, "u_earth_west", 0, west);
            bind(gl, program, "u_earth_east", 1, east);
            bind(gl, program, "u_earth_night", 5, self.night);
        }
    }

    pub(super) unsafe fn paint_figures(
        &mut self,
        gl: &glow::Context,
        assets: &sky::Assets,
        axes: [[f32; 3]; 3],
        fov: [f32; 2],
        opacity: f32,
    ) {
        let Some(catalog) = assets.star_catalog() else {
            return;
        };
        // SAFETY: this pass shares the current context and viewport with the sky.
        unsafe {
            if self.figures.is_none() && !self.figures_failed {
                match figures::Figures::upload(gl, &catalog.figures) {
                    Ok(figures) => self.figures = Some(figures),
                    Err(error) => {
                        self.figures_failed = true;
                        tracing::warn!(%error, "Constellation overlay unavailable");
                    }
                }
            }
            if let Some(figures) = &self.figures {
                figures.paint(gl, axes, fov, opacity);
            }
        }
    }

    pub(super) unsafe fn earth_uniforms(
        &mut self,
        gl: &glow::Context,
        program: glow::Program,
        frame: &super::Frame,
    ) {
        // SAFETY: the paint callback owns this current context and program.
        unsafe {
            let loc = |name| gl.get_uniform_location(program, name);
            let lighting = surface_lighting(frame);
            let scene = frame.sky.as_ref().filter(|_| lighting > 0.0);
            gl.uniform_1_f32(loc("u_earth_lighting").as_ref(), lighting);
            // A sampler must be assigned even when its layer is disabled.
            gl.uniform_1_i32(loc("u_night").as_ref(), 5);
            if let Some(scene) = scene {
                self.prepare(gl, &scene.assets);
                gl.uniform_1_i32(
                    loc("u_black_marble").as_ref(),
                    i32::from(self.night.is_some()),
                );
                gl.uniform_1_i32(
                    loc("u_solar_possible").as_ref(),
                    i32::from(scene.solar_possible),
                );
                gl.uniform_1_f32(loc("u_longitude").as_ref(), frame.camera.center_lon as f32);
                gl.uniform_1_f32(
                    loc("u_atmosphere").as_ref(),
                    if frame.show_sky {
                        atmosphere(frame.camera, frame.style)
                    } else {
                        0.0
                    },
                );
                gl.uniform_3_f32_slice(loc("u_sun").as_ref(), &scene.sun);
                gl.uniform_3_f32_slice(loc("u_moon").as_ref(), &scene.moon);
                bind(gl, program, "u_night", 5, self.night);
                gl.active_texture(glow::TEXTURE0);
            }
        }
    }

    pub(super) unsafe fn vector_uniforms(
        &mut self,
        gl: &glow::Context,
        program: glow::Program,
        frame: &super::Frame,
    ) {
        // SAFETY: current context and active program are owned by the renderer.
        unsafe {
            let loc = |name| gl.get_uniform_location(program, name);
            let lighting = surface_lighting(frame);
            let scene = frame.sky.as_ref().filter(|_| lighting > 0.0);
            let marble = scene.is_some();
            gl.uniform_1_f32(loc("u_earth_lighting").as_ref(), lighting);
            gl.uniform_1_f32(loc("u_longitude").as_ref(), frame.camera.center_lon as f32);
            gl.uniform_1_f32(
                loc("u_detail_span").as_ref(),
                super::super::detail::SIDE as f32,
            );
            gl.uniform_1_f32(
                loc("u_road_night").as_ref(),
                if marble && self.night.is_some() {
                    lighting
                } else {
                    0.0
                },
            );
            bind(gl, program, "u_night", 5, self.night.filter(|_| marble));
            if let Some(scene) = scene {
                gl.uniform_3_f32_slice(loc("u_sun").as_ref(), &scene.sun);
            }
            gl.active_texture(glow::TEXTURE0);
        }
    }

    pub(super) unsafe fn prepare(&mut self, gl: &glow::Context, assets: &Arc<sky::Assets>) {
        let source = Arc::downgrade(assets);
        if self.source.ptr_eq(&source) {
            return;
        }
        // SAFETY: texture work stays in the owner callback on the current context.
        unsafe {
            self.clear_figures(gl);
            for (old, image) in [
                (&mut self.stars, Some(&assets.stars)),
                (&mut self.moon, assets.moon.as_ref()),
                (&mut self.night, assets.night.as_ref()),
            ] {
                if let Some(texture) = old.take() {
                    gl.delete_texture(texture);
                }
                *old = image.and_then(|image| {
                    upload(gl, image)
                        .map_err(|error| {
                            tracing::warn!(%error, "Sky texture upload failed");
                        })
                        .ok()
                });
            }
            self.source = source;
        }
    }

    pub(super) fn stars(&self) -> Option<glow::Texture> {
        self.stars
    }

    pub(super) unsafe fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: only the renderer's current context deletes its objects.
        unsafe {
            self.clear_figures(gl);
            for texture in [self.stars.take(), self.moon.take(), self.night.take()]
                .into_iter()
                .flatten()
            {
                gl.delete_texture(texture);
            }
            if let Some(program) = self.program.take() {
                gl.delete_program(program);
            }
        }
    }

    unsafe fn clear_figures(&mut self, gl: &glow::Context) {
        if let Some(figures) = self.figures.take() {
            // SAFETY: both destruction and source replacement run on the owning context.
            unsafe { figures.destroy(gl) };
        }
        self.figures_failed = false;
    }
}

fn readable_disk_zoom(zoom: f64) -> f32 {
    // Reduce only the artificial enlargement as Earth gets smaller on screen.
    zoom.clamp(0.0, 1.0) as f32
}

fn atmosphere(camera: GlobeCamera, style: BasemapStyle) -> f32 {
    if style == BasemapStyle::Natural {
        zoom_fade(camera.zoom, 1.5, 2.15) * camera.globe_blend() as f32
    } else {
        0.0
    }
}

fn surface_lighting(frame: &super::Frame) -> f32 {
    if frame.earth_lighting && frame.sky.is_some() {
        surface_exposure(frame.style, frame.camera.zoom)
    } else {
        0.0
    }
}

fn surface_exposure(style: BasemapStyle, zoom: f64) -> f32 {
    match style {
        BasemapStyle::Natural | BasemapStyle::Hypsometric => zoom_fade(zoom, 64.0, 768.0),
        BasemapStyle::Oilslick | BasemapStyle::Hillshade => 0.0,
    }
}

fn zoom_fade(zoom: f64, start: f64, end: f64) -> f32 {
    let amount = ((end - zoom) / (end - start)).clamp(0.0, 1.0) as f32;
    amount * amount * (3.0 - 2.0 * amount)
}

pub(super) unsafe fn bind(
    gl: &glow::Context,
    program: glow::Program,
    name: &str,
    unit: u32,
    texture: Option<glow::Texture>,
) {
    // SAFETY: current context and active program come from the renderer.
    unsafe {
        gl.active_texture(glow::TEXTURE0 + unit);
        gl.bind_texture(glow::TEXTURE_2D, texture);
        gl.uniform_1_i32(gl.get_uniform_location(program, name).as_ref(), unit as i32);
    }
}

pub(super) unsafe fn upload(
    gl: &glow::Context,
    source: &sky::Texture,
) -> Result<glow::Texture, String> {
    // SAFETY: the decoded RGB buffer fits the validated texture dimensions.
    unsafe {
        gl.active_texture(glow::TEXTURE3);
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGB8 as i32,
            source.size[0],
            source.size[1],
            0,
            glow::RGB,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&source.pixels)),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR_MIPMAP_LINEAR as i32,
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        Ok(texture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earth_disk_can_be_picked_only_where_it_is_visible_from_the_moon() {
        let mut projection = SkyProjection {
            basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            focal: 600.0,
            radius: 0.0,
            distance: 0.0,
            eye: [0.0, 0.0, 384_400.0],
            sphere_eye: Some([0.0, 0.0, -2.0]),
        };
        assert!(projection.earth_at(egui::vec2(8.0, 0.0)));
        assert!(!projection.earth_at(egui::vec2(12.0, 0.0)));

        projection.sphere_eye = Some([1.0, 0.0, 2.0]);
        assert!(projection.earth_at(egui::vec2(8.0, 0.0)));
        assert!(!projection.earth_at(egui::vec2(-8.0, 0.0)));
    }

    #[test]
    fn sky_projection_keeps_bodies_off_the_globe_and_behind_the_camera() {
        let camera = GlobeCamera {
            center_lat: 0.0,
            center_lon: 0.0,
            ..GlobeCamera::default()
        };
        let projection = SkyProjection::new(camera, [1000.0, 600.0]);
        assert!(projection.project([1e8, 2e7, 0.0]).is_none());
        assert!(projection.project([-1e8, 0.0, 0.0]).is_none());
        let offset = projection.project([-1e8, 5e7, 2e7]).unwrap();
        assert!((offset.x - 285.0).abs() < 0.1);
        assert!((offset.y + 114.0).abs() < 0.1);
    }

    #[test]
    fn star_catalog_projection_round_trips_an_inertial_ray() {
        let camera = GlobeCamera {
            center_lat: 38.0,
            center_lon: -104.0,
            ..GlobeCamera::default()
        };
        let projection = SkyProjection::new(camera, [1000.0, 600.0]);
        let rotation = [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let axes = projection.inertial_axes(rotation);
        let offset = egui::vec2(310.0, -190.0);
        let shader_ray = projection.inertial_ray(projection.basis, offset);
        let ray = [-shader_ray[1], shader_ray[0], shader_ray[2]];
        assert!(
            ((inspection::View {
                axes,
                focal: projection.focal
            })
            .offset(ray)
            .unwrap()
                - offset)
                .length()
                < 0.001
        );
        assert!(
            (inspection::View {
                axes,
                focal: projection.focal
            })
            .offset(ray.map(|v| -v))
            .is_none()
        );
    }

    #[test]
    fn atmosphere_only_shows_on_the_wide_natural_globe() {
        use little_clip_core::geography::basemap::ProjectionMode;
        let mut camera = GlobeCamera::default();
        for style in [
            BasemapStyle::Hypsometric,
            BasemapStyle::Oilslick,
            BasemapStyle::Hillshade,
        ] {
            assert_eq!(atmosphere(camera, style), 0.0);
        }
        assert_eq!(atmosphere(camera, BasemapStyle::Natural), 1.0);
        camera.zoom = 1.8;
        assert!((0.01..0.99).contains(&atmosphere(camera, BasemapStyle::Natural)));
        camera.mode = ProjectionMode::Map2D;
        assert_eq!(atmosphere(camera, BasemapStyle::Natural), 0.0);
        camera.mode = ProjectionMode::Globe3D;
        camera.zoom = 2.15;
        assert_eq!(atmosphere(camera, BasemapStyle::Natural), 0.0);
    }

    #[test]
    fn only_natural_and_hypsometric_have_night_lighting_and_it_fades_as_you_zoom_in() {
        for style in [BasemapStyle::Natural, BasemapStyle::Hypsometric] {
            assert_eq!(surface_exposure(style, 1.0), 1.0);
            assert_eq!(surface_exposure(style, 416.0), 0.5);
            assert_eq!(surface_exposure(style, 768.0), 0.0);
        }
        for style in [BasemapStyle::Oilslick, BasemapStyle::Hillshade] {
            assert_eq!(surface_exposure(style, 1.0), 0.0);
            assert_eq!(surface_exposure(style, 416.0), 0.0);
        }
    }
}
