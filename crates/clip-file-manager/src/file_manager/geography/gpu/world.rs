use super::super::{sky as assets, solar::world::Frame};
use eframe::glow::{self, HasContext};
use std::sync::{Arc, Weak};

#[derive(Default)]
pub(super) struct Renderer {
    program: Option<(glow::Program, glow::VertexArray)>,
    texture: Option<glow::Texture>,
    source: Option<Weak<[u8]>>,
}

impl Renderer {
    pub(super) unsafe fn draw(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        world: &Frame,
    ) -> Result<bool, String> {
        // SAFETY: the callback has verified the owning context before entering.
        unsafe {
            let (program, vao) = match self.program {
                Some(program) => program,
                None => {
                    let vao = gl.create_vertex_array()?;
                    let program = match super::program(
                        gl,
                        include_str!("../sky.vert"),
                        include_str!("../world.frag"),
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
            self.set_texture(gl, world.texture.as_ref())?;
            super::configure_viewport(gl, info);
            gl.use_program(Some(program));
            gl.bind_vertex_array(Some(vao));
            let uniform = |name| gl.get_uniform_location(program, name);
            let axes = world.pose.axes.map(|axis| axis.map(|v| v as f32));
            gl.uniform_matrix_3_f32_slice(uniform("u_axes").as_ref(), false, axes.as_flattened());
            gl.uniform_3_f32_slice(uniform("u_eye").as_ref(), &world.pose.eye.map(|v| v as f32));
            gl.uniform_2_f32_slice(uniform("u_fov").as_ref(), &world.pose.half_fov());
            gl.uniform_3_f32_slice(uniform("u_color").as_ref(), &world.color);
            gl.uniform_1_f32(uniform("u_zoom").as_ref(), world.zoom);
            gl.uniform_1_i32(uniform("u_gas").as_ref(), i32::from(world.gas));
            gl.uniform_1_i32(
                uniform("u_has_albedo").as_ref(),
                i32::from(self.texture.is_some()),
            );
            super::sky::bind(gl, program, "u_albedo", 0, self.texture);
            gl.uniform_1_i32(uniform("u_band_count").as_ref(), world.bands.len() as i32);
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

    pub(super) fn texture(&self) -> Option<glow::Texture> {
        self.texture
    }

    pub(super) unsafe fn set_texture(
        &mut self,
        gl: &glow::Context,
        image: Option<&assets::Texture>,
    ) -> Result<(), String> {
        let incoming = image.map(|image| Arc::downgrade(&image.pixels));
        if self
            .source
            .as_ref()
            .zip(incoming.as_ref())
            .is_some_and(|(old, new)| old.ptr_eq(new))
        {
            return Ok(());
        }
        // SAFETY: texture objects are owned by the verified current context.
        unsafe {
            if let Some(texture) = self.texture.take() {
                gl.delete_texture(texture);
            }
            self.texture = image
                .map(|image| super::sky::upload(gl, image))
                .transpose()?;
        }
        self.source = incoming;
        Ok(())
    }

    pub(super) unsafe fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: caller has kept the owning OpenGL context current.
        unsafe {
            if let Some((program, vao)) = self.program.take() {
                gl.delete_program(program);
                gl.delete_vertex_array(vao);
            }
            if let Some(texture) = self.texture.take() {
                gl.delete_texture(texture);
            }
        }
    }
}
