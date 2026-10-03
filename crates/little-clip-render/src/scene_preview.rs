//! Bounded finite orbit cache shared by the display and workbench mirror.

use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use little_clip_scene::{OrbitLoop, SplatRenderer, SplatScene};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

const MAX_ENTRIES: usize = 8;
const MAX_BYTES: usize = 128 * 1024 * 1024;
static CACHE: LazyLock<Mutex<PreviewCache>> = LazyLock::new(|| Mutex::new(PreviewCache::default()));

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    file_bytes: u64,
    size: [u32; 2],
    appearance: String,
    triangle_budget: usize,
}

#[derive(Clone)]
pub(crate) enum Preview {
    Loading,
    Ready(Arc<OrbitLoop>),
    Error(String),
}

struct Pending {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Result<OrbitLoop, String>, JobError>>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

enum Value {
    Pending(Pending),
    Ready(Arc<OrbitLoop>),
    Error(String),
}

struct Entry {
    value: Value,
    access: u64,
}

#[derive(Default)]
struct PreviewCache {
    entries: HashMap<Key, Entry>,
    clock: u64,
}

impl PreviewCache {
    fn request(
        &mut self,
        key: Key,
        bake: impl FnOnce(Arc<AtomicBool>) -> Result<OrbitLoop, String> + Send + 'static,
    ) -> Preview {
        self.entries.retain(|old, _| {
            old.path != key.path
                || (old.modified == key.modified
                    && old.file_bytes == key.file_bytes
                    && old.appearance == key.appearance
                    && old.triangle_budget == key.triangle_budget)
        });
        self.clock = self.clock.saturating_add(1);
        if !self.entries.contains_key(&key) {
            while self.entries.len() >= MAX_ENTRIES {
                self.evict_oldest();
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            let worker_cancelled = Arc::clone(&cancelled);
            let (ticket, rx) = BackgroundJobs::global()
                .submit(Duration::from_secs(120), move || bake(worker_cancelled));
            self.entries.insert(
                key.clone(),
                Entry {
                    value: Value::Pending(Pending {
                        ticket,
                        cancelled,
                        rx,
                    }),
                    access: self.clock,
                },
            );
        }
        let entry = self
            .entries
            .get_mut(&key)
            .expect("preview entry just inserted");
        entry.access = self.clock;
        if let Value::Pending(pending) = &entry.value {
            match pending.rx.try_recv() {
                Ok(Ok(Ok(orbit))) => entry.value = Value::Ready(Arc::new(orbit)),
                Ok(Ok(Err(error))) => entry.value = Value::Error(error),
                Ok(Err(JobError::QueueFull)) => {
                    self.entries.remove(&key);
                    return Preview::Loading;
                }
                Ok(Err(error)) => entry.value = Value::Error(error.to_string()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    entry.value = Value::Error("Scene preview worker disconnected".into())
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let result = match &entry.value {
            Value::Pending(_) => Preview::Loading,
            Value::Ready(orbit) => Preview::Ready(Arc::clone(orbit)),
            Value::Error(error) => Preview::Error(error.clone()),
        };
        while self.retained_bytes() > MAX_BYTES {
            self.evict_oldest();
        }
        result
    }

    fn retained_bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| match &entry.value {
                Value::Ready(orbit) => orbit
                    .frames()
                    .iter()
                    .map(|image| image.as_raw().len())
                    .sum(),
                _ => 0,
            })
            .sum()
    }

    fn evict_oldest(&mut self) {
        if let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.access)
            .map(|(key, _)| key.clone())
        {
            self.entries.remove(&key);
        }
    }
}

pub(crate) fn request(path: &Path, size: [u32; 2], renderer: &crate::ScreenRenderer) -> Preview {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return Preview::Error(error.to_string()),
    };
    let key = Key {
        path: path.to_owned(),
        modified: metadata.modified().ok(),
        file_bytes: metadata.len(),
        size,
        appearance: renderer.palette.colors.surface_recessed.clone(),
        triangle_budget: renderer.config.model3d.preview_triangle_limit,
    };
    let source = key.clone();
    let config = renderer.config.clone();
    let palette = renderer.palette.clone();
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .request(key, move |cancelled| {
            let started = Instant::now();
            let orbit = match SplatScene::load(&source.path) {
                Ok(scene) => {
                    if cancelled.load(Ordering::Acquire) {
                        return Err("Scene preview cancelled".into());
                    }
                    let mut renderer =
                        SplatRenderer::new(Arc::new(scene)).map_err(|error| error.to_string())?;
                    renderer
                        .bake_orbit(source.size[0], source.size[1], || {
                            cancelled.load(Ordering::Acquire)
                                || started.elapsed() > Duration::from_secs(120)
                        })
                        .map_err(|error| error.to_string())?
                }
                Err(little_clip_scene::SceneError::MeshGlb) => {
                    let model = little_clip_core::model3d::Model3d::load_file(&source.path)?;
                    let mut renderer = crate::ScreenRenderer::fallback(config);
                    renderer.palette = palette;
                    bake_mesh(&renderer, &model, source.size, || {
                        cancelled.load(Ordering::Acquire)
                            || started.elapsed() > Duration::from_secs(120)
                    })?
                }
                Err(error) => return Err(error.to_string()),
            };
            let current = std::fs::metadata(&source.path).map_err(|error| error.to_string())?;
            if current.modified().ok() != source.modified || current.len() != source.file_bytes {
                return Err("Scene file changed while preparing its preview".into());
            }
            Ok(orbit)
        })
}

fn bake_mesh(
    renderer: &crate::ScreenRenderer,
    model: &little_clip_core::model3d::Model3d,
    size: [u32; 2],
    cancelled: impl Fn() -> bool,
) -> Result<OrbitLoop, String> {
    let [width, height] = size;
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|bytes| bytes.checked_mul(4 * little_clip_scene::ORBIT_FRAME_COUNT as u64));
    if width == 0 || height == 0 || !bytes.is_some_and(|bytes| bytes <= MAX_BYTES as u64) {
        return Err("Mesh orbit exceeds the preview pixel budget".into());
    }
    let mut frames = Vec::with_capacity(little_clip_scene::ORBIT_FRAME_COUNT);
    for index in 0..little_clip_scene::ORBIT_FRAME_COUNT {
        if cancelled() {
            return Err("Scene preview cancelled".into());
        }
        let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("Invalid mesh viewport")?;
        pixmap.fill(renderer.color_from_hex(&renderer.palette.colors.surface_recessed));
        renderer.draw_model3d_viewport(
            &mut pixmap,
            model,
            [0.0, 0.0, width as f32, height as f32],
            index as f32 * std::f32::consts::TAU / little_clip_scene::ORBIT_FRAME_COUNT as f32,
        );
        let image = image::RgbaImage::from_raw(width, height, pixmap.take())
            .ok_or("Invalid mesh raster")?;
        frames.push(image);
    }
    OrbitLoop::from_frames(frames).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
