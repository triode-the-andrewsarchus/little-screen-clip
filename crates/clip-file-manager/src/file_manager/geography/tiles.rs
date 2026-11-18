//! Viewer-owned, cancellable tile preparation with a fixed CPU memory budget.
use super::{
    geometry::{Layers, Scene},
    gpu,
};
use little_clip_core::{
    BackgroundJobs,
    geography::basemap::{
        BasemapAssets, BasemapStyle, GlobeCamera, choose_contour_interval_m,
        tiles::{SourceTile, TileId, visible_tiles},
    },
    jobs::task::Task,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
const CPU_BUDGET: usize = 96 * 1024 * 1024;
static NEXT_SOURCE: AtomicU64 = AtomicU64::new(1);
fn workers() -> &'static BackgroundJobs {
    static WORKERS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 8));
    &WORKERS
}

pub(super) struct Tiles {
    source: u64,
    assets: Option<Arc<BasemapAssets>>,
    ready: BTreeMap<TileId, Arc<SourceTile>>,
    pending: BTreeMap<TileId, Task<Option<SourceTile>>>,
    order: VecDeque<TileId>,
    bytes: usize,
    scene_job: Option<Task<Result<Scene, String>>>,
    scene: Option<Arc<Scene>>,
    error: Option<String>,
    pub range: [f32; 2],
    pub contour: Option<f32>,
}
impl Default for Tiles {
    fn default() -> Self {
        Self {
            source: 0,
            assets: None,
            ready: BTreeMap::new(),
            pending: BTreeMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            scene_job: None,
            scene: None,
            error: None,
            range: [0.0, 1000.0],
            contour: None,
        }
    }
}
impl Tiles {
    pub fn update(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        assets: Option<&Arc<BasemapAssets>>,
        camera: GlobeCamera,
        style: BasemapStyle,
        contours: bool,
        layers: Layers,
    ) -> Result<(), String> {
        let Some(assets) = assets else {
            return Ok(());
        };
        if !self
            .assets
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, assets))
        {
            *self = Self {
                source: NEXT_SOURCE.fetch_add(1, Ordering::Relaxed),
                assets: Some(assets.clone()),
                ..Self::default()
            };
        }
        if self.scene.is_none() && self.scene_job.is_none() && self.error.is_none() {
            let assets = assets.clone();
            self.scene_job = Some(Task::start(
                workers(),
                Duration::from_secs(120),
                move |cancel| Scene::prepare(&assets, cancel),
            ));
        }
        if let Some(result) = self.scene_job.as_ref().and_then(Task::poll) {
            self.scene_job = None;
            match result {
                Ok(Ok(scene)) => self.scene = Some(Arc::new(scene)),
                Ok(Err(error)) => {
                    self.error = Some(error.clone());
                    return Err(error);
                }
                Err(little_clip_core::JobError::QueueFull) => {}
                Err(error) => {
                    self.error = Some(error.to_string());
                    return Err(error.to_string());
                }
            }
        }
        let wanted: BTreeSet<_> = visible_tiles(
            &camera,
            rect.width(),
            rect.height(),
            ui.ctx().pixels_per_point(),
        )
        .into_iter()
        .collect();
        self.pending.retain(|id, _| wanted.contains(id));
        let mut finished = Vec::new();
        self.pending.retain(|id, job| {
            if let Some(result) = job.poll() {
                if let Ok(Some(tile)) = result {
                    finished.push((*id, Arc::new(tile)));
                }
                false
            } else {
                true
            }
        });
        for (id, tile) in finished {
            self.bytes += tile.bytes();
            self.ready.insert(id, tile);
            self.order.push_back(id);
        }
        while self.bytes > CPU_BUDGET {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(tile) = self.ready.remove(&old) {
                self.bytes -= tile.bytes();
            }
        }
        for &id in &wanted {
            if self.pending.len() >= 4 {
                break;
            }
            if self.ready.contains_key(&id) || self.pending.contains_key(&id) {
                continue;
            }
            let assets = assets.clone();
            self.pending.insert(
                id,
                Task::start(workers(), Duration::from_secs(60), move |cancel| {
                    SourceTile::prepare(&assets, id, cancel)
                }),
            );
        }
        if !self.pending.is_empty() || self.scene_job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let tiles: Vec<_> = wanted
            .iter()
            .filter_map(|id| self.ready.get(id).cloned())
            .collect();
        let finest = tiles.iter().map(|tile| tile.id.level).max().unwrap_or(0);
        let ranges: Vec<_> = tiles
            .iter()
            .filter(|tile| tile.id.level == finest)
            .filter_map(|tile| tile.elevation_range)
            .collect();
        self.range = ranges
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |r, x| {
                [r[0].min(x[0]), r[1].max(x[1])]
            });
        self.contour = (contours && camera.zoom >= 3.5 && !ranges.is_empty()).then(|| {
            choose_contour_interval_m((self.range[1] - self.range[0]).max(20.0), camera.zoom)
        });
        if ranges.is_empty() {
            self.range = [0.0, 1000.0];
        }
        gpu::paint(
            ui,
            rect,
            gpu::Frame {
                source: self.source,
                size: [rect.width(), rect.height()],
                camera,
                style,
                contours: self.contour,
                range: self.range,
                tiles,
                scene: self.scene.clone(),
                layers,
            },
        )
    }
}
