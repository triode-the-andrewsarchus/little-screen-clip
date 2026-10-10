use super::super::filesystem;
use crate::file_source::FileSource;
use image::RgbaImage;
use little_clip_core::{palette::AppPalette, presentation::model3d::Model3d};
use little_clip_scene::{OrbitCamera, SplatRenderer, SplatScene};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

const MESH_TRIANGLES: usize = 10_000;
#[derive(Clone)]
pub(super) enum Metadata {
    Gaussian {
        points: usize,
        center: [f32; 3],
        radius: f32,
        linear: bool,
    },
    Mesh {
        triangles: usize,
        preview_triangles: usize,
        dimensions: [f32; 3],
        units: &'static str,
        colored: bool,
        z_up: bool,
    },
}
pub(super) enum Renderer {
    Gaussian(SplatRenderer),
    Mesh {
        model: Model3d,
        rasterizer: Box<little_clip_render::ScreenRenderer>,
    },
}
pub(super) struct Frame {
    pub image: RgbaImage,
    pub render_ms: f32,
}
pub(super) struct Loaded {
    pub renderer: Renderer,
    pub metadata: Metadata,
    pub frame: Frame,
}

pub(super) fn load(
    path: &Path,
    revision: u64,
    camera: OrbitCamera,
    size: [u32; 2],
    palette: &AppPalette,
    token: &AtomicBool,
) -> Result<Loaded, String> {
    let source = FileSource::capture(path)?;
    filesystem::check_revision(path, revision)?;
    cancelled(token)?;
    let (mut renderer, metadata) = load_renderer(path, palette, token)?;
    cancelled(token)?;
    let frame = render(&mut renderer, camera, size, token)?;
    source.check()?;
    filesystem::check_revision(path, revision)?;
    Ok(Loaded {
        renderer,
        metadata,
        frame,
    })
}

fn load_renderer(
    path: &Path,
    palette: &AppPalette,
    token: &AtomicBool,
) -> Result<(Renderer, Metadata), String> {
    if little_clip_scene::is_splat_file(path) {
        match SplatScene::load(path) {
            Ok(scene) => {
                cancelled(token)?;
                let (center, radius) = scene.framing();
                let metadata = Metadata::Gaussian {
                    points: scene.len(),
                    center,
                    radius,
                    linear: scene.has_linear_color(),
                };
                let renderer =
                    SplatRenderer::new(Arc::new(scene)).map_err(|error| error.to_string())?;
                return Ok((Renderer::Gaussian(renderer), metadata));
            }
            Err(little_clip_scene::SceneError::MeshGlb) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    load_mesh(path, palette, token)
}

fn load_mesh(
    path: &Path,
    palette: &AppPalette,
    token: &AtomicBool,
) -> Result<(Renderer, Metadata), String> {
    if std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len()
        > 256 * 1024 * 1024
    {
        return Err("Mesh exceeds the 256 MiB preview import limit; open in Model Studio.".into());
    }
    let mesh = Model3d::load_cached(path)?;
    cancelled(token)?;
    let step = mesh.triangles.len().div_ceil(MESH_TRIANGLES).max(1);
    let preview = Model3d {
        name: mesh.name.clone(),
        format: mesh.format,
        triangles: Vec::new(),
        thumbnail_triangles: mesh.triangles.iter().step_by(step).copied().collect(),
        min_bounds: mesh.min_bounds,
        max_bounds: mesh.max_bounds,
        center: mesh.center,
        size: mesh.size,
        max_dimension: mesh.max_dimension,
    };
    let metadata = Metadata::Mesh {
        triangles: mesh.triangles.len(),
        preview_triangles: preview.thumbnail_triangles.len(),
        dimensions: [mesh.size.x, mesh.size.y, mesh.size.z],
        units: if mesh.format == little_clip_core::presentation::model3d::ModelFormat::Fbx {
            "meters"
        } else {
            "model units"
        },
        colored: mesh
            .triangles
            .iter()
            .any(|triangle| triangle.color.is_some()),
        z_up: mesh.format == little_clip_core::presentation::model3d::ModelFormat::Stl,
    };
    let mut rasterizer = little_clip_render::ScreenRenderer::fallback(Default::default());
    rasterizer.config.model3d.preview_triangle_limit = MESH_TRIANGLES;
    rasterizer.palette = palette.clone();
    Ok((
        Renderer::Mesh {
            model: preview,
            rasterizer: Box::new(rasterizer),
        },
        metadata,
    ))
}

pub(super) fn render(
    renderer: &mut Renderer,
    camera: OrbitCamera,
    size: [u32; 2],
    token: &AtomicBool,
) -> Result<Frame, String> {
    cancelled(token)?;
    let started = Instant::now();
    let image = match renderer {
        Renderer::Gaussian(renderer) => renderer
            .render(camera, size[0], size[1])
            .map_err(|error| error.to_string())?,
        Renderer::Mesh { model, rasterizer } => {
            let mut pixels =
                tiny_skia::Pixmap::new(size[0], size[1]).ok_or("Invalid mesh preview size")?;
            pixels.fill(rasterizer.color_from_hex(&rasterizer.palette.colors.surface_recessed));
            rasterizer.draw_model3d_viewport(
                &mut pixels,
                model,
                [0.0, 0.0, size[0] as f32, size[1] as f32],
                camera.yaw,
            );
            // tiny-skia stores premultiplied pixels; Frame images use straight RGBA.
            let bytes: Vec<_> = pixels
                .pixels()
                .iter()
                .flat_map(|pixel| {
                    let color = pixel.demultiply();
                    [color.red(), color.green(), color.blue(), color.alpha()]
                })
                .collect();
            RgbaImage::from_raw(size[0], size[1], bytes).ok_or("Invalid mesh raster")?
        }
    };
    cancelled(token)?;
    Ok(Frame {
        image,
        render_ms: started.elapsed().as_secs_f32() * 1000.0,
    })
}
pub(super) fn camera(view: usize, elapsed: f32) -> OrbitCamera {
    OrbitCamera {
        yaw: OrbitCamera::default().yaw
            + view as f32 * std::f32::consts::TAU / 4.0
            + elapsed * std::f32::consts::TAU / 12.0,
        ..Default::default()
    }
}
fn cancelled(token: &AtomicBool) -> Result<(), String> {
    if token.load(Ordering::Relaxed) {
        Err("Scene preview cancelled".into())
    } else {
        Ok(())
    }
}
