//! One immutable instance buffer for the active embroidery design.
use crate::file_manager::gl_mesh::Mesh;
use eframe::{
    egui_glow,
    glow::{self, HasContext},
};
use little_clip_core::embroidery::{Design, TimelineLayout};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

// Four native-endian floats (two endpoints) and normalized RGBA8.
type Instance = [[u8; 4]; 5];

pub(super) struct Geometry {
    instances: Vec<Instance>,
    size: [f32; 2],
}

impl Geometry {
    pub(super) fn new(
        design: &Design,
        layout: &TimelineLayout,
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        let mut instances = Vec::with_capacity(design.stitch_count());
        for chunk in design.stitches().chunks(2048) {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Embroidery preview cancelled".into());
            }
            instances.extend(chunk.iter().map(|stitch| {
                let [x0, y0] = layout.project(stitch.from);
                let [x1, y1] = layout.project(stitch.to);
                let [r, g, b] = stitch.color;
                [
                    x0.to_ne_bytes(),
                    y0.to_ne_bytes(),
                    x1.to_ne_bytes(),
                    y1.to_ne_bytes(),
                    [r, g, b, 255],
                ]
            }));
        }
        Ok(Self {
            instances,
            size: [layout.width as f32, layout.height as f32],
        })
    }
}

fn runtime_key() -> egui::Id {
    egui::Id::unique("embroidery_gpu")
}

pub(in crate::file_manager) struct Runtime {
    ctx: egui::Context,
    context: Weak<glow::Context>,
    renderer: Arc<Mutex<Renderer>>,
}

impl Runtime {
    pub(in crate::file_manager) fn install(ctx: &egui::Context, gl: &Arc<glow::Context>) -> Self {
        let renderer = Arc::new(Mutex::new(Renderer {
            context_id: Arc::as_ptr(gl) as usize,
            resources: None,
            error: None,
        }));
        ctx.data_mut(|data| data.insert_temp(runtime_key(), Arc::downgrade(&renderer)));
        Self {
            ctx: ctx.clone(),
            context: Arc::downgrade(gl),
            renderer,
        }
    }

    /// # Safety
    ///
    /// The host must retain its owning OpenGL context as current through this call.
    pub(in crate::file_manager) unsafe fn destroy(&mut self, gl: &glow::Context) {
        // The weak owner reserves its allocation, so the address cannot be reused.
        if std::ptr::eq(self.context.as_ptr(), gl)
            && let Ok(mut renderer) = self.renderer.lock()
            && let Some(resources) = renderer.resources.take()
        {
            // SAFETY: eframe's exit hook retains the matching current context.
            unsafe { resources.mesh.destroy(gl) };
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.ctx
            .data_mut(|data| data.remove::<Weak<Mutex<Renderer>>>(runtime_key()));
    }
}

pub(super) fn paint(
    painter: &egui::Painter,
    rect: egui::Rect,
    geometry: &Arc<Geometry>,
    stitches: usize,
) -> Result<(), String> {
    if stitches == 0 || !painter.clip_rect().intersects(rect) {
        return Ok(());
    }
    let renderer = painter
        .ctx()
        .data(|data| data.get_temp::<Weak<Mutex<Renderer>>>(runtime_key()))
        .ok_or("Live stitches require OpenGL")?;
    if let Some(error) = renderer
        .upgrade()
        .and_then(|slot| slot.lock().ok()?.error.clone())
    {
        return Err(error);
    }

    let ctx = painter.ctx().clone();
    let geometry = Arc::clone(geometry);
    painter.add(egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            let Some(renderer) = renderer.upgrade() else {
                return;
            };
            let Ok(mut renderer) = renderer.lock() else {
                return;
            };
            let gl = painter.gl();
            if renderer.context_id == Arc::as_ptr(gl) as usize && renderer.error.is_none() {
                let viewport = info.viewport_in_pixels();
                let size = [viewport.width_px, viewport.height_px];
                if size.iter().any(|&axis| axis <= 0) {
                    return;
                }
                // SAFETY: Glow invokes this callback with its own context current.
                // The identity check ties the cached resources to that same context.
                if let Err(error) = unsafe { renderer.draw(gl, size, &geometry, stitches) } {
                    tracing::warn!(%error, "Could not paint live stitches");
                    renderer.error = Some("Could not draw live stitches".into());
                    ctx.request_repaint();
                }
            }
        })),
    });
    Ok(())
}

struct Renderer {
    context_id: usize,
    resources: Option<Resources>,
    error: Option<String>,
}

impl Renderer {
    // gl must be the owning current context.
    unsafe fn draw(
        &mut self,
        gl: &glow::Context,
        size: [i32; 2],
        geometry: &Arc<Geometry>,
        stitches: usize,
    ) -> Result<(), String> {
        // SAFETY: the callback checked that this context owns the renderer.
        unsafe {
            let resources = match &mut self.resources {
                Some(resources) => resources,
                slot => slot.insert(Resources::new(gl)?),
            };
            gl.bind_vertex_array(Some(resources.mesh.vao));
            let source = Arc::downgrade(geometry);
            if !resources.uploaded.ptr_eq(&source) {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(resources.mesh.buffer));
                gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    geometry.instances.as_flattened().as_flattened(),
                    glow::STATIC_DRAW,
                );
                let error = gl.get_error();
                if error != glow::NO_ERROR {
                    return Err(format!(
                        "Stitch buffer upload failed with GL error {error:#x}"
                    ));
                }
                resources.uploaded = source;
            }

            let [width, height] = geometry.size;
            let feather = (width / size[0] as f32).max(height / size[1] as f32);
            // Glow supplies the physical viewport, scissor and premultiplied blending.
            gl.use_program(Some(resources.mesh.program));
            gl.uniform_2_f32(resources.to_clip.as_ref(), 2.0 / width, -2.0 / height);
            gl.uniform_1_f32(resources.feather.as_ref(), feather);
            gl.draw_arrays_instanced(
                glow::TRIANGLE_STRIP,
                0,
                4,
                stitches.min(geometry.instances.len()) as i32,
            );
        }
        Ok(())
    }
}

struct Resources {
    mesh: Mesh,
    to_clip: Option<glow::UniformLocation>,
    feather: Option<glow::UniformLocation>,
    uploaded: Weak<Geometry>,
}

impl Resources {
    // gl must be current throughout initialization and any cleanup.
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        // SAFETY: creation happens on the owning current context. Each handle
        // is released on failure, or transferred to Resources on success.
        unsafe {
            let header = if gl.version().is_embedded {
                "#version 300 es\nprecision highp float;\nprecision highp int;"
            } else {
                "#version 330 core"
            };
            let vertex = format!("{header}\n{}", include_str!("stitch.vert"));
            let fragment = format!("{header}\n{}", include_str!("stitch.frag"));
            let mesh = Mesh::new(gl, &vertex, &fragment)?;
            let stride = std::mem::size_of::<Instance>() as i32;
            gl.vertex_attrib_pointer_f32(0, 4, glow::FLOAT, false, stride, 0);
            gl.vertex_attrib_pointer_f32(1, 4, glow::UNSIGNED_BYTE, true, stride, 16);
            for attrib in 0..2 {
                gl.enable_vertex_attrib_array(attrib);
                gl.vertex_attrib_divisor(attrib, 1);
            }
            Ok(Self {
                to_clip: gl.get_uniform_location(mesh.program, "to_clip"),
                feather: gl.get_uniform_location(mesh.program, "feather"),
                mesh,
                uploaded: Weak::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_exp(bytes: &[u8]) -> Design {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.exp");
        std::fs::write(&path, bytes).unwrap();
        Design::read(&path, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn scrub_prefixes_keep_order_colors_and_penetrations_without_travel() {
        let design = read_exp(&[
            0, 0, 40, 0, // First thread.
            0x80, 0x04, 40, 216, // Jump (+40, +40), with EXP Y inverted.
            4, 0, 0, 236, // Resume after the jump.
            0x80, 0x80, 0, 0, // Trim at the current position.
            0, 248, 5, 0, // Resume after the trim.
            0x80, 0x01, 0, 0, // Change thread without a connecting stitch.
            250, 0, 0, 50,
        ]);
        let layout = design.timeline_layout([256, 220], 4);
        let geometry = Geometry::new(&design, &layout, &AtomicBool::new(false)).unwrap();
        let first = [19, 123, 117];
        let second = [181, 55, 84];
        let expected = [
            ([0, 0], [0, 0], first),
            ([0, 0], [40, 0], first),
            ([84, 40], [84, 40], first),
            ([84, 40], [84, 60], first),
            ([84, 68], [84, 68], first),
            ([84, 68], [89, 68], first),
            ([83, 68], [83, 68], second),
            ([83, 68], [83, 18], second),
        ];
        assert_eq!(geometry.instances.len(), expected.len());
        assert_eq!(layout.colors(), &[first, first, first, second]);
        for (frame, count) in [(0, 0), (1, 2), (4, 8), (2, 4), (3, 6), (0, 0)] {
            let prefix = layout.stitches_at(frame);
            assert_eq!(prefix, count);
            for (instance, &(from, to, [r, g, b])) in
                geometry.instances[..prefix].iter().zip(&expected)
            {
                let coordinates = instance.map(f32::from_ne_bytes);
                assert_eq!(coordinates[..2], layout.project(from));
                assert_eq!(coordinates[2..4], layout.project(to));
                assert_eq!(instance[4], [r, g, b, 255]);
            }
        }
    }

    #[test]
    #[ignore = "release timing comparison"]
    fn dense_embroidery_preparation_benchmark() {
        use std::{hint::black_box, time::Instant};

        let mut exp = Vec::new();
        for row in 0..400 {
            if row % 80 == 0 && row != 0 {
                exp.extend([0x80, 0x01, 0, 0]);
            }
            if row != 0 {
                exp.extend([0x80, 0x04, 0, 250]);
            }
            let dx = if row % 2 == 0 { 4_i8 } else { -4_i8 };
            for _ in 0..600 {
                exp.extend([dx as u8, 0]);
            }
        }
        let design = read_exp(&exp);
        assert_eq!(design.stitch_count(), 240_000);
        let cancelled = AtomicBool::new(false);
        let reference = super::super::VIEWER_TILE;
        let mut atlas_times = Vec::new();
        let mut geometry_times = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let atlas = design.timeline(reference, 63, &cancelled).unwrap();
            atlas_times.push(start.elapsed());
            let start = Instant::now();
            let layout = design.timeline_layout(reference, 63);
            let geometry = Geometry::new(&design, &layout, &cancelled).unwrap();
            geometry_times.push(start.elapsed());
            black_box((&atlas, &geometry));
        }
        atlas_times.sort_unstable();
        geometry_times.sort_unstable();
        let layout = design.timeline_layout(reference, 63);
        let mut raster_times = Vec::new();
        for frame in [20, 51, 35, 61, 63] {
            let start = Instant::now();
            black_box(
                design
                    .detailed_prefix(
                        reference,
                        [1024, 1024],
                        layout.stitches_at(frame),
                        &cancelled,
                    )
                    .unwrap(),
            );
            raster_times.push(start.elapsed());
        }
        raster_times.sort_unstable();
        eprintln!(
            "240,000 stitches, CPU preparation median: atlas {:?}, live instances {:?};              old 1024px detail scrub median {:?}, slowest {:?}",
            atlas_times[2], geometry_times[2], raster_times[2], raster_times[4]
        );
    }
}
