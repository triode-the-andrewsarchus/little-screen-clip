use crate::file_manager::geography::sky;
use eframe::glow::{self, HasContext};
use little_clip_core::geography::basemap::GlobeCamera;
use std::sync::{Arc, Weak};

#[derive(Default)]
pub(super) struct SkyRenderer {
    source: Weak<sky::Assets>,
    program: Option<glow::Program>,
    stars: Option<glow::Texture>,
}

impl SkyRenderer {
    pub(super) unsafe fn paint(
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
                        include_str!("../sky.frag"),
                    )?;
                    self.program = Some(program);
                    program
                }
            };
            if !self.source.ptr_eq(&Arc::downgrade(&scene.assets)) {
                let stars = upload(gl, &scene.assets.stars)?;
                if let Some(old) = self.stars.replace(stars) {
                    gl.delete_texture(old);
                }
                self.source = Arc::downgrade(&scene.assets);
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
            gl.uniform_3_f32_slice(gl.get_uniform_location(p, "u_sun").as_ref(), &scene.sun);
            gl.uniform_3_f32_slice(gl.get_uniform_location(p, "u_moon").as_ref(), &scene.moon);
            gl.uniform_3_f32_slice(
                gl.get_uniform_location(p, "u_planets[0]").as_ref(),
                scene.planets.as_flattened(),
            );
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, self.stars);
            gl.uniform_1_i32(gl.get_uniform_location(p, "u_stars").as_ref(), 3);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.active_texture(glow::TEXTURE0);
        }
        Ok(())
    }

    pub(super) unsafe fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: only the renderer's current context deletes its objects.
        unsafe {
            if let Some(texture) = self.stars.take() {
                gl.delete_texture(texture);
            }
            if let Some(program) = self.program.take() {
                gl.delete_program(program);
            }
        }
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
