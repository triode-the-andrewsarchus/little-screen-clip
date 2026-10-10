use super::super::filesystem;
use crate::file_source::FileSource;
use image::RgbaImage;
use little_clip_scene::{OrbitCamera, SplatRenderer, SplatScene};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[derive(Clone)]
pub(super) struct Metadata {
    pub points: usize,
    pub center: [f32; 3],
    pub radius: f32,
    pub linear: bool,
}
pub(super) struct Frame {
    pub image: RgbaImage,
    pub render_ms: f32,
}
pub(super) struct Loaded {
    pub renderer: SplatRenderer,
    pub metadata: Metadata,
    pub frame: Frame,
}

pub(super) fn load(
    path: &Path,
    revision: u64,
    camera: OrbitCamera,
    size: [u32; 2],
    token: &AtomicBool,
) -> Result<Loaded, String> {
    let source = FileSource::capture(path)?;
    filesystem::check_revision(path, revision)?;
    let scene = SplatScene::load(path).map_err(|error| error.to_string())?;
    cancelled(token)?;
    let (center, radius) = scene.framing();
    let metadata = Metadata {
        points: scene.len(),
        center,
        radius,
        linear: scene.has_linear_color(),
    };
    let mut renderer = SplatRenderer::new(Arc::new(scene)).map_err(|error| error.to_string())?;
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

pub(super) fn render(
    renderer: &mut SplatRenderer,
    camera: OrbitCamera,
    size: [u32; 2],
    token: &AtomicBool,
) -> Result<Frame, String> {
    cancelled(token)?;
    let started = Instant::now();
    let image = renderer
        .render(camera, size[0], size[1])
        .map_err(|error| error.to_string())?;
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
