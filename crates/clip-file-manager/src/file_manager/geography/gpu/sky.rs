mod figures;

use crate::file_manager::geography::sky::{self, catalog::Star};
use eframe::egui;
use eframe::glow::{self, HasContext};
use little_clip_core::geography::basemap::{BasemapStyle, GlobeCamera};
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
}

impl SkyProjection {
    fn new(camera: GlobeCamera, [width, height]: [f32; 2]) -> Self {
        let (sin_lat, cos_lat) = (camera.center_lat as f32).to_radians().sin_cos();
        let (sin_lon, cos_lon) = (camera.center_lon as f32).to_radians().sin_cos();
        let focal = width.min(height) * 0.95;
        let radius = camera.scale_px(width, height) as f32;
        Self {
            basis: [
                [-sin_lon, cos_lon, 0.0],
                [-cos_lon * sin_lat, -sin_lon * sin_lat, cos_lat],
                [cos_lon * cos_lat, sin_lon * cos_lat, sin_lat],
            ],
            focal,
            radius,
            distance: 6378.137 * (1.0 + (focal / radius).powi(2)).sqrt(),
        }
    }

    fn project(&self, position: [f32; 3]) -> Option<egui::Vec2> {
        let [x, y, z] = self.basis.map(|axis| dot(axis, position));
        let depth = self.distance - z;
        if depth <= 0.0 {
            return None;
        }
        let offset = egui::vec2(x, -y) * (self.focal / depth);
        (offset.length_sq() > self.radius.powi(2)).then_some(offset)
    }

    fn inertial_axes(&self, rows: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
        self.basis
            .map(|axis| std::array::from_fn(|i| dot(axis, [rows[0][i], rows[1][i], rows[2][i]])))
    }

    fn star_offset(&self, axes: [[f32; 3]; 3], direction: [f32; 3]) -> Option<egui::Vec2> {
        let [x, y, z] = axes.map(|axis| dot(axis, direction));
        (z < 0.0).then(|| egui::vec2(x, -y) * (self.focal / -z))
    }

    fn inertial_ray(&self, [east, north, outward]: [[f32; 3]; 3], offset: egui::Vec2) -> [f32; 3] {
        let slope = offset / self.focal;
        let ray: [f32; 3] =
            std::array::from_fn(|i| east[i] * slope.x - north[i] * slope.y - outward[i]);
        let magnitude = dot(ray, ray).sqrt();
        ray.map(|v| v / magnitude)
    }

    fn fov(&self, [width, height]: [f32; 2]) -> [f32; 2] {
        [width, height].map(|size| size / (2.0 * self.focal))
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(in super::super) enum SkyObject<'a> {
    Body(&'static str),
    Star(&'a Star),
}

pub(in super::super) fn sky_object_at(
    frame: &super::Frame,
    rect: egui::Rect,
    pointer: egui::Pos2,
    identify_stars: bool,
) -> Option<SkyObject<'_>> {
    if !frame.show_sky || frame.camera.globe_blend() < 1.0 || !rect.contains(pointer) {
        return None;
    }
    let projection = SkyProjection::new(frame.camera, frame.size);
    let center = rect.center();
    if pointer.distance_sq(center) <= projection.radius.powi(2) {
        return None;
    }
    let scene = frame.sky.as_ref()?;
    let body = scene
        .bodies()
        .filter_map(|(name, position)| {
            let point = center + projection.project(position)?;
            let distance = point.distance_sq(pointer);
            (rect.contains(point) && distance <= 14.0_f32.powi(2)).then_some((name, distance))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(name, _)| SkyObject::Body(name));
    body.or_else(|| {
        identify_stars
            .then(|| find_star(scene, &projection, rect, pointer))
            .flatten()
            .map(SkyObject::Star)
    })
}

fn find_star<'a>(
    scene: &'a sky::Scene,
    projection: &SkyProjection,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<&'a Star> {
    let catalog = scene.assets.star_catalog()?;
    if projection.focal < 32.0 {
        return None;
    }
    let center = rect.center();
    let axes = projection.inertial_axes(scene.inertial_rows);
    let ray = projection.inertial_ray(axes, pointer - center);
    const PICK_RADIUS: f32 = 5.0;
    catalog
        .nearby(ray, PICK_RADIUS / projection.focal)
        .filter_map(|star| {
            let offset = projection.star_offset(axes, star.direction)?;
            let point = center + offset;
            let distance = pointer.distance_sq(point);
            (distance <= PICK_RADIUS.powi(2)
                && rect.contains(point)
                && offset.length_sq() > projection.radius.powi(2))
            .then_some((star, distance))
        })
        .min_by(|(a, da), (b, db)| {
            a.magnitude
                .total_cmp(&b.magnitude)
                .then_with(|| da.total_cmp(db))
        })
        .map(|(star, _)| star)
}

impl SkyRenderer {
    pub(super) unsafe fn paint_frame(&mut self, gl: &glow::Context, frame: &super::Frame) {
        if self.failed || !frame.show_sky {
            return;
        }
        let Some(scene) = &frame.sky else { return };
        // SAFETY: called during the owner's paint callback with its context current.
        if let Err(error) = unsafe { self.paint(gl, scene, frame) } {
            self.failed = true;
            tracing::warn!(%error, "Sky renderer unavailable");
        }
    }

    unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        scene: &sky::Scene,
        frame: &super::Frame,
    ) -> Result<(), String> {
        let camera = frame.camera;
        let opacity = camera.globe_blend() as f32;
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
            self.prepare(gl, scene);
            if self.stars.is_none() {
                return Ok(());
            }
            let projection = SkyProjection::new(camera, frame.size);
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
                gl.get_uniform_location(p, "u_atmosphere").as_ref(),
                atmosphere(camera, frame.style),
            );
            gl.uniform_3_f32_slice(gl.get_uniform_location(p, "u_sun").as_ref(), &scene.sun);
            gl.uniform_3_f32_slice(gl.get_uniform_location(p, "u_moon").as_ref(), &scene.moon);
            gl.uniform_3_f32_slice(
                gl.get_uniform_location(p, "u_planets[0]").as_ref(),
                scene.planets.as_flattened(),
            );
            gl.uniform_3_f32_slice(
                gl.get_uniform_location(p, "u_lunar_sun").as_ref(),
                &scene.lunar_sun,
            );
            gl.uniform_3_f32_slice(
                gl.get_uniform_location(p, "u_lunar_earth").as_ref(),
                &scene.lunar_earth,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(p, "u_has_lunar_albedo").as_ref(),
                i32::from(self.moon.is_some()),
            );
            bind(gl, p, "u_lunar_albedo", 4, self.moon);
            bind(gl, p, "u_stars", 3, self.stars);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            if frame.show_constellations {
                self.paint_figures(gl, scene, &projection, [half_x, half_y], opacity);
            }
            gl.active_texture(glow::TEXTURE0);
        }
        Ok(())
    }

    unsafe fn paint_figures(
        &mut self,
        gl: &glow::Context,
        scene: &sky::Scene,
        projection: &SkyProjection,
        fov: [f32; 2],
        opacity: f32,
    ) {
        let Some(catalog) = scene.assets.star_catalog() else {
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
                figures.paint(
                    gl,
                    projection.inertial_axes(scene.inertial_rows),
                    fov,
                    opacity,
                );
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
                let marble = frame.city_lights && scene.modern_lights;
                if marble {
                    self.prepare(gl, scene);
                }
                gl.uniform_1_i32(
                    loc("u_black_marble").as_ref(),
                    i32::from(marble && self.night.is_some()),
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
                bind(gl, program, "u_night", 5, self.night.filter(|_| marble));
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
            let marble = scene.is_some_and(|scene| frame.city_lights && scene.modern_lights);
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

    unsafe fn prepare(&mut self, gl: &glow::Context, scene: &sky::Scene) {
        let source = Arc::downgrade(&scene.assets);
        if self.source.ptr_eq(&source) {
            return;
        }
        // SAFETY: texture work stays in the owner callback on the current context.
        unsafe {
            self.clear_figures(gl);
            for (old, image) in [
                (&mut self.stars, Some(&scene.assets.stars)),
                (&mut self.moon, scene.assets.moon.as_ref()),
                (&mut self.night, scene.assets.night.as_ref()),
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

unsafe fn bind(
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

unsafe fn upload(gl: &glow::Context, source: &sky::Texture) -> Result<glow::Texture, String> {
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
        assert!((projection.star_offset(axes, ray).unwrap() - offset).length() < 0.001);
        assert!(projection.star_offset(axes, ray.map(|v| -v)).is_none());
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
