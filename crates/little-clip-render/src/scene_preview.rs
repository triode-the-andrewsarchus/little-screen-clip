//! Bounded finite orbit cache shared by the display and workbench mirror.

use little_clip_core::ipc::{ScenePreview, ScenePreviewKey as Key};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use little_clip_scene::{OrbitLoop, SplatRenderer, SplatScene};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, mpsc};
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 8;
const MAX_BYTES: usize = 128 * 1024 * 1024;

// Long GPU/import jobs must not occupy the general preview and mirror workers.
static BAKE_JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, MAX_ENTRIES));
pub(crate) type SharedSource = Arc<dyn Fn() -> Vec<ScenePreview> + Send + Sync>;

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
pub(crate) struct PreviewCache {
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
            let (ticket, rx) =
                BAKE_JOBS.submit(Duration::from_secs(120), move || bake(worker_cancelled));
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
                Value::Ready(orbit) => orbit.disk_bytes(),
                _ => 0,
            })
            .sum()
    }

    fn retain_visible(&mut self, paths: &HashSet<PathBuf>) {
        self.entries.retain(|key, _| paths.contains(&key.path));
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
    let shared_source = renderer.scene_preview_source.clone();
    renderer
        .scene_previews
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .request(key, move |cancelled| {
            let started = Instant::now();
            let shared = shared_source
                .and_then(|fetch| fetch().into_iter().find(|shared| shared.key == source))
                .and_then(|shared| {
                    OrbitLoop::from_shared_directory(&shared.directory, source.size).ok()
                });
            let orbit = shared
                .map(Ok)
                .unwrap_or_else(|| match SplatScene::load(&source.path) {
                    Ok(scene) => {
                        if cancelled.load(Ordering::Acquire) {
                            return Err("Scene preview cancelled".into());
                        }
                        let mut renderer = SplatRenderer::new(Arc::new(scene))
                            .map_err(|error| error.to_string())?;
                        renderer
                            .bake_orbit(source.size[0], source.size[1], || {
                                cancelled.load(Ordering::Acquire)
                                    || started.elapsed() > Duration::from_secs(120)
                            })
                            .map_err(|error| error.to_string())
                    }
                    Err(little_clip_scene::SceneError::MeshGlb) => {
                        let model = little_clip_core::model3d::Model3d::load_file(&source.path)?;
                        let mut renderer = crate::ScreenRenderer::fallback(config);
                        renderer.palette = palette;
                        bake_mesh(&renderer, &model, source.size, || {
                            cancelled.load(Ordering::Acquire)
                                || started.elapsed() > Duration::from_secs(120)
                        })
                    }
                    Err(error) => Err(error.to_string()),
                })?;
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
    OrbitLoop::bake_frames(width, height, |index| {
        if cancelled() {
            return Err(little_clip_scene::SceneError::Cancelled);
        }
        let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
            little_clip_scene::SceneError::Invalid("Invalid mesh viewport".into())
        })?;
        pixmap.fill(renderer.color_from_hex(&renderer.palette.colors.surface_recessed));
        renderer.draw_model3d_viewport(
            &mut pixmap,
            model,
            [0.0, 0.0, width as f32, height as f32],
            index as f32 * std::f32::consts::TAU / little_clip_scene::ORBIT_FRAME_COUNT as f32,
        );
        image::RgbaImage::from_raw(width, height, pixmap.take())
            .ok_or_else(|| little_clip_scene::SceneError::Invalid("Invalid mesh raster".into()))
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn retain_visible(
    renderer: &crate::ScreenRenderer,
    manager: &little_clip_core::SlotManager,
    layout: &[crate::SlotLayoutRect],
) {
    let paths = layout
        .iter()
        .filter_map(|slot| manager.get(slot.slot_id))
        .filter_map(little_clip_core::ClipItem::file_path)
        .filter(|path| little_clip_scene::is_splat_file(path))
        .collect();
    renderer
        .scene_previews
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain_visible(&paths);
}

#[cfg(test)]
mod tests;

impl crate::ScreenRenderer {
    /// Configures a nonblocking-render source queried only by scene preparation workers.
    pub fn set_scene_preview_source(
        &mut self,
        source: impl Fn() -> Vec<ScenePreview> + Send + Sync + 'static,
    ) {
        self.scene_preview_source = Some(Arc::new(source));
    }

    /// Returns completed orbits without waiting for queued or running bakes.
    pub fn scene_previews(&self) -> Vec<ScenePreview> {
        self.scene_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .iter()
            .filter_map(|(key, entry)| match &entry.value {
                Value::Ready(orbit) => Some(ScenePreview {
                    key: key.clone(),
                    directory: orbit.frames()[0]
                        .parent()
                        .expect("owned orbit directory")
                        .to_owned(),
                }),
                _ => None,
            })
            .collect()
    }
}
