//! Inertial line segments drawn in the existing sky pass, before the globe.
use crate::file_manager::gl_mesh::Mesh;
use eframe::glow::{self, HasContext};

pub(super) struct Figures {
    mesh: Mesh,
    count: i32,
}

impl Figures {
    pub(super) unsafe fn upload(
        gl: &glow::Context,
        endpoints: &[[f32; 3]],
    ) -> Result<Self, String> {
        // SAFETY: the sky callback owns the current context and initialized endpoints.
        unsafe {
            let mesh = Mesh::new(
                gl,
                include_str!("../../figures.vert"),
                include_str!("../../figures.frag"),
            )?;
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                super::super::float_bytes(endpoints.as_flattened()),
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);
            Ok(Self {
                mesh,
                count: endpoints.len() as i32,
            })
        }
    }

    pub(super) unsafe fn paint(
        &self,
        gl: &glow::Context,
        axes: [[f32; 3]; 3],
        [half_x, half_y]: [f32; 2],
        opacity: f32,
    ) {
        // SAFETY: the sky callback owns these objects and the current context.
        unsafe {
            gl.use_program(Some(self.mesh.program));
            let uniform = |name| gl.get_uniform_location(self.mesh.program, name);
            gl.uniform_matrix_3_f32_slice(
                uniform("u_camera_inertial").as_ref(),
                false,
                axes.as_flattened(),
            );
            gl.uniform_2_f32(uniform("u_fov").as_ref(), half_x, half_y);
            gl.uniform_1_f32(uniform("u_opacity").as_ref(), opacity);
            gl.bind_vertex_array(Some(self.mesh.vao));
            gl.line_width(1.0);
            gl.draw_arrays(glow::LINES, 0, self.count);
        }
    }

    pub(super) unsafe fn destroy(self, gl: &glow::Context) {
        // SAFETY: the sky callback owns these objects and the current context.
        unsafe { self.mesh.destroy(gl) }
    }
}
