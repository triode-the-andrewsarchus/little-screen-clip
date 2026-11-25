//! OpenGL shader compilation and vertex buffer ownership for File Manager callbacks.
use eframe::glow::{self, HasContext};

pub(super) struct Mesh {
    pub program: glow::Program,
    pub vao: glow::VertexArray,
    pub buffer: glow::Buffer,
}

impl Mesh {
    /// # Safety
    ///
    /// The caller must keep the owning context current throughout creation and cleanup.
    /// On success, the caller must release this mesh via `destroy`.
    pub(super) unsafe fn new(
        gl: &glow::Context,
        vertex: &str,
        fragment: &str,
    ) -> Result<Self, String> {
        // SAFETY: the caller retains the current context. Each allocated object
        // is released on failure or transferred to the returned mesh.
        unsafe {
            let program = program(gl, vertex, fragment)?;
            let vao = gl
                .create_vertex_array()
                .inspect_err(|_| gl.delete_program(program))?;
            let buffer = gl.create_buffer().inspect_err(|_| {
                gl.delete_vertex_array(vao);
                gl.delete_program(program);
            })?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            Ok(Self {
                program,
                vao,
                buffer,
            })
        }
    }

    /// # Safety
    ///
    /// The context used for creation must be current, and the handles must still be live.
    pub(super) unsafe fn destroy(self, gl: &glow::Context) {
        // SAFETY: the caller retains the context that owns all three handles.
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.buffer);
            gl.delete_program(self.program);
        }
    }
}

/// # Safety
///
/// The caller must retain its current GL context through compilation and own the returned program.
pub(super) unsafe fn program(
    gl: &glow::Context,
    vertex: &str,
    fragment: &str,
) -> Result<glow::Program, String> {
    // SAFETY: caller retains the owning current GL context.
    unsafe {
        link_program(
            gl,
            [
                (glow::VERTEX_SHADER, vertex),
                (glow::FRAGMENT_SHADER, fragment),
            ],
        )
    }
}

/// # Safety
///
/// The caller must retain its current GL context through compilation and own the returned program.
pub(super) unsafe fn link_program<'a>(
    gl: &glow::Context,
    shaders: impl IntoIterator<Item = (u32, &'a str)>,
) -> Result<glow::Program, String> {
    // SAFETY: compilation runs on the caller's current context. Every shader is
    // deleted after linking, including compile/link failure paths.
    unsafe {
        let program = gl.create_program()?;
        for (kind, source) in shaders {
            let shader = match gl.create_shader(kind) {
                Ok(shader) => shader,
                Err(error) => {
                    gl.delete_program(program);
                    return Err(error);
                }
            };
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let error = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("GL shader: {error}"));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("GL shader link: {error}"));
        }
        Ok(program)
    }
}
