use crate::file_manager::geography::sky;
use eframe::glow::{self, HasContext};
use little_clip_core::geography::basemap::GlobeCamera;
use std::sync::{Arc, Weak};

#[derive(Default)]
pub(super) struct SkyRenderer {
    source: Weak<sky::Assets>,
    program: Option<glow::Program>,
    stars: Option<glow::Texture>,
    moon: Option<glow::Texture>,
    night: Option<glow::Texture>,
    failed: bool,
}

impl SkyRenderer {
    pub(super) unsafe fn paint_frame(&mut self, gl: &glow::Context, frame: &super::Frame) {
        if self.failed || !frame.show_sky {
            return;
        }
        let Some(scene) = &frame.sky else { return };
        // SAFETY: called during the owner's paint callback with its context current.
        if let Err(error) = unsafe { self.paint(gl, scene, frame.camera, frame.size) } {
            self.failed = true;
            tracing::warn!(%error, "Sky renderer unavailable");
        }
    }

    unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        scene: &sky::Scene,
        camera: GlobeCamera,
        [width, height]: [f32; 2],
    ) -> Result<(), String> {
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
            let (sin_lat, cos_lat) = (camera.center_lat as f32).to_radians().sin_cos();
            let (sin_lon, cos_lon) = (camera.center_lon as f32).to_radians().sin_cos();
            let basis = [
                -sin_lon,
                cos_lon,
                0.0,
                -cos_lon * sin_lat,
                -sin_lon * sin_lat,
                cos_lat,
                cos_lon * cos_lat,
                sin_lon * cos_lat,
                sin_lat,
            ];
            let focal = width.min(height) * 0.95;
            let radius = camera.scale_px(width, height) as f32;
            let distance = 6378.137 * (1.0 + (focal / radius).powi(2)).sqrt();

            gl.use_program(Some(p));
            gl.uniform_matrix_3_f32_slice(
                gl.get_uniform_location(p, "u_inertial_from_fixed").as_ref(),
                false,
                scene.inertial_rows.as_flattened(),
            );
            gl.uniform_matrix_3_f32_slice(
                gl.get_uniform_location(p, "u_camera_basis").as_ref(),
                false,
                &basis,
            );
            gl.uniform_2_f32(
                gl.get_uniform_location(p, "u_fov").as_ref(),
                width / (2.0 * focal),
                height / (2.0 * focal),
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_camera_distance").as_ref(),
                distance,
            );
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_opacity").as_ref(), opacity);
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_atmosphere").as_ref(),
                atmosphere(camera),
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
            gl.active_texture(glow::TEXTURE0);
        }
        Ok(())
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
            let scene = frame.sky.as_ref().filter(|_| frame.earth_lighting);
            gl.uniform_1_i32(loc("u_earth_lighting").as_ref(), i32::from(scene.is_some()));
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
                        atmosphere(frame.camera)
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

    pub(super) unsafe fn road_uniforms(
        &mut self,
        gl: &glow::Context,
        program: glow::Program,
        frame: &super::Frame,
    ) {
        // SAFETY: current context and active program are owned by the renderer.
        unsafe {
            let loc = |name| gl.get_uniform_location(program, name);
            let scene = frame
                .sky
                .as_ref()
                .filter(|scene| frame.earth_lighting && frame.city_lights && scene.modern_lights);
            gl.uniform_1_f32(loc("u_longitude").as_ref(), frame.camera.center_lon as f32);
            gl.uniform_1_f32(
                loc("u_detail_span").as_ref(),
                super::super::detail::SIDE as f32,
            );
            gl.uniform_1_i32(
                loc("u_road_night").as_ref(),
                i32::from(scene.is_some() && self.night.is_some()),
            );
            bind(
                gl,
                program,
                "u_night",
                5,
                self.night.filter(|_| scene.is_some()),
            );
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
}

fn atmosphere(camera: GlobeCamera) -> f32 {
    let amount = ((2.15 - camera.zoom) / 0.65).clamp(0.0, 1.0) as f32;
    amount * amount * (3.0 - 2.0 * amount) * camera.globe_blend() as f32
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
    fn atmosphere_fades_before_the_close_globe_and_flat_map() {
        use little_clip_core::geography::basemap::ProjectionMode;
        let mut camera = GlobeCamera::default();
        assert_eq!(atmosphere(camera), 1.0);
        camera.zoom = 1.8;
        assert!((0.01..0.99).contains(&atmosphere(camera)));
        camera.mode = ProjectionMode::Map2D;
        assert_eq!(atmosphere(camera), 0.0);
        camera.mode = ProjectionMode::Globe3D;
        camera.zoom = 2.15;
        assert_eq!(atmosphere(camera), 0.0);
    }
}
