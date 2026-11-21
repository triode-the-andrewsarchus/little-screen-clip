use super::super::{
    sky as assets,
    solar::{
        Id, SUN,
        world::{Backdrop, Frame, atmosphere},
    },
};
use eframe::glow::{self, HasContext};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

pub(super) const FULL_REGION: [f32; 4] = [1.0, 1.0, 0.0, 0.0];

pub(super) fn ray_region(canvas: egui::Rect, visible: egui::Rect) -> [f32; 4] {
    let center = visible.center() - canvas.center();
    [
        visible.width() / canvas.width(),
        visible.height() / canvas.height(),
        2.0 * center.x / canvas.width(),
        -2.0 * center.y / canvas.height(),
    ]
}

#[derive(Default)]
pub(super) struct Renderer {
    program: Option<(glow::Program, glow::VertexArray)>,
    textures: HashMap<(Id, [i32; 2]), (glow::Texture, Weak<[u8]>)>,
}

impl Renderer {
    pub(super) unsafe fn draw(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        world: &Frame,
        region: [f32; 4],
    ) -> Result<bool, String> {
        // SAFETY: the callback has verified the owning context before entering.
        unsafe {
            let program = self.bind_program(
                gl,
                info,
                world.pose.axes.map(|axis| axis.map(|v| v as f32)),
                world.pose.half_fov(),
            )?;
            let texture = self.texture(gl, world.id, world.texture.as_ref())?;
            let uniform = |name| gl.get_uniform_location(program, name);
            gl.uniform_4_f32_slice(uniform("u_ray_region").as_ref(), &region);
            gl.uniform_1_i32(uniform("u_background_only").as_ref(), 0);
            gl.uniform_1_i32(uniform("u_emissive").as_ref(), i32::from(world.id == SUN));
            gl.uniform_2_f32_slice(uniform("u_solar_clock").as_ref(), &world.clock);
            gl.uniform_3_f32_slice(uniform("u_sun_direction").as_ref(), &world.sun);
            gl.uniform_3_f32_slice(uniform("u_eye").as_ref(), &world.pose.eye.map(|v| v as f32));
            gl.uniform_3_f32_slice(uniform("u_color").as_ref(), &world.color);
            gl.uniform_1_f32(uniform("u_zoom").as_ref(), world.zoom);
            gl.uniform_1_i32(uniform("u_gas").as_ref(), i32::from(world.gas));
            gl.uniform_1_i32(
                uniform("u_has_albedo").as_ref(),
                i32::from(texture.is_some()),
            );
            super::sky::bind(gl, program, "u_albedo", 0, texture);
            for (id, name) in [(299, "u_venus"), (799, "u_uranus"), (899, "u_neptune")] {
                gl.uniform_1_i32(uniform(name).as_ref(), i32::from(world.id == id));
            }
            let air = atmosphere(world.id);
            let optics = air.as_ref().map_or([0.0; 4], |air| air.optics);
            gl.uniform_4_f32_slice(uniform("u_air_optics").as_ref(), &optics);
            if let Some(air) = air {
                gl.uniform_3_f32_slice(uniform("u_air_tint").as_ref(), &air.tint);
            }
            gl.uniform_1_i32(uniform("u_band_count").as_ref(), world.bands.len() as i32);
            gl.uniform_1_f32(uniform("u_ring_emphasis").as_ref(), world.ring_emphasis);
            if !world.bands.is_empty() {
                let radius = (world.pose.radius_m / 1000.0) as f32;
                let bands: Vec<_> = world
                    .bands
                    .iter()
                    .flat_map(|[inner, outer, opacity]| [inner / radius, outer / radius, *opacity])
                    .collect();
                gl.uniform_3_f32_slice(uniform("u_bands[0]").as_ref(), &bands);
            }
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            Ok(false)
        }
    }

    pub(super) unsafe fn backdrop(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        view: &Backdrop,
        stars: Option<glow::Texture>,
    ) -> Result<(), String> {
        // SAFETY: the callback verified the owning GL context.
        unsafe {
            let program = self.bind_program(gl, info, view.axes, view.fov)?;
            let loc = |name| gl.get_uniform_location(program, name);
            gl.uniform_4_f32_slice(loc("u_ray_region").as_ref(), &FULL_REGION);
            gl.uniform_1_i32(loc("u_background_only").as_ref(), 1);
            gl.uniform_1_i32(loc("u_has_stars").as_ref(), i32::from(stars.is_some()));
            gl.uniform_3_f32_slice(loc("u_sun_direction").as_ref(), &view.sun.0);
            gl.uniform_1_f32(loc("u_sun_radius").as_ref(), view.sun.1);
            gl.uniform_2_f32_slice(loc("u_solar_clock").as_ref(), &view.clock);
            gl.uniform_1_f32(loc("u_sky_brightness").as_ref(), view.brightness);
            super::sky::bind(gl, program, "u_stars", 0, stars);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    unsafe fn bind_program(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        axes: [[f32; 3]; 3],
        fov: [f32; 2],
    ) -> Result<glow::Program, String> {
        // SAFETY: shader objects are created and bound only on their owning context.
        unsafe {
            let (program, vao) = match self.program {
                Some(program) => program,
                None => {
                    let vao = gl.create_vertex_array()?;
                    let program = match super::program(
                        gl,
                        include_str!("../sky.vert"),
                        &super::with_celestial(include_str!("../world.frag")),
                    ) {
                        Ok(program) => program,
                        Err(error) => {
                            gl.delete_vertex_array(vao);
                            return Err(error);
                        }
                    };
                    self.program = Some((program, vao));
                    (program, vao)
                }
            };
            super::configure_viewport(gl, info);
            gl.use_program(Some(program));
            gl.bind_vertex_array(Some(vao));
            let loc = |name| gl.get_uniform_location(program, name);
            gl.uniform_matrix_3_f32_slice(loc("u_axes").as_ref(), false, axes.as_flattened());
            gl.uniform_2_f32_slice(loc("u_fov").as_ref(), &fov);
            Ok(program)
        }
    }

    pub(super) unsafe fn texture(
        &mut self,
        gl: &glow::Context,
        id: Id,
        image: Option<&assets::Texture>,
    ) -> Result<Option<glow::Texture>, String> {
        let Some(image) = image else { return Ok(None) };
        let key = (id, image.size);
        let source = Arc::downgrade(&image.pixels);
        if let Some((texture, saved)) = self.textures.get(&key)
            && source.ptr_eq(saved)
        {
            return Ok(Some(*texture));
        }
        // SAFETY: texture objects are owned by the verified current context.
        unsafe {
            self.textures.retain(|existing, (texture, saved)| {
                let keep = *existing != key && saved.strong_count() != 0;
                if !keep {
                    gl.delete_texture(*texture)
                };
                keep
            });
            let texture = super::sky::upload(gl, image)?;
            self.textures.insert(key, (texture, source));
            Ok(Some(texture))
        }
    }

    pub(super) unsafe fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: caller has kept the owning OpenGL context current.
        unsafe {
            if let Some((program, vao)) = self.program.take() {
                gl.delete_program(program);
                gl.delete_vertex_array(vao);
            }
            for (_, (texture, _)) in self.textures.drain() {
                gl.delete_texture(texture);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cropped_globe_keeps_original_projection_at_the_screen_corner() {
        let full = egui::Rect::from_min_max(egui::pos2(-40.0, -20.0), egui::pos2(60.0, 80.0));
        let crop = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(60.0, 80.0));
        let [sx, sy, dx, dy] = ray_region(full, crop);
        assert!((dx - sx + 0.2).abs() < 1e-6);
        assert!((dy + sy - 0.6).abs() < 1e-6);
        assert!((dx + sx - 1.0).abs() < 1e-6);
        assert!((dy - sy + 1.0).abs() < 1e-6);
    }
}
