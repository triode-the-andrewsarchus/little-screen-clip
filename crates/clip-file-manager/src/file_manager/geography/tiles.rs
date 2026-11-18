//! Viewer-owned, cancellable tile preparation with a fixed CPU memory budget.
use super::{
    geometry::{Layers, RiverKey, Scene},
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
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    raster: TileCache<TileId, SourceTile>,
    rivers: TileCache<RiverKey, Scene>,
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
            raster: TileCache::default(),
            rivers: TileCache::default(),
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
        let source = assets.clone();
        let raster_error =
            self.raster
                .update(&wanted, CPU_BUDGET, SourceTile::bytes, move |id, cancel| {
                    SourceTile::prepare(&source, id, cancel)
                        .ok_or_else(|| "Map tile preparation cancelled".into())
                });
        let river_keys = RiverKey::visible(
            &camera,
            [rect.width(), rect.height()],
            layers.rivers,
            wanted.iter().map(|tile| tile.level).max().unwrap_or(0),
        );
        let source = assets.clone();
        let river_error = self.rivers.update(
            &river_keys,
            64 * 1024 * 1024,
            Scene::bytes,
            move |key, cancel| key.prepare(&source, cancel),
        );
        if !self.raster.pending.is_empty()
            || !self.rivers.pending.is_empty()
            || self.scene_job.is_some()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let tiles: Vec<_> = wanted
            .iter()
            .filter_map(|id| self.raster.ready.get(id).cloned())
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
                rivers: river_keys
                    .iter()
                    .filter_map(|key| {
                        self.rivers
                            .ready
                            .get(key)
                            .map(|scene| (*key, scene.clone()))
                    })
                    .collect(),
                layers,
            },
        )?;
        match raster_error.or(river_error) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Raster and river tiles share admission, cancellation and byte-bounded retention.
struct TileCache<K, T> {
    ready: BTreeMap<K, Arc<T>>,
    pending: BTreeMap<K, Task<Result<T, String>>>,
    failed: BTreeMap<K, String>,
    order: VecDeque<K>,
    bytes: usize,
}
impl<K, T> Default for TileCache<K, T> {
    fn default() -> Self {
        Self {
            ready: BTreeMap::new(),
            pending: BTreeMap::new(),
            failed: BTreeMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }
    }
}
impl<K: Copy + Ord + Send + 'static, T: Send + 'static> TileCache<K, T> {
    fn update(
        &mut self,
        wanted: &BTreeSet<K>,
        budget: usize,
        bytes: fn(&T) -> usize,
        prepare: impl Fn(K, &AtomicBool) -> Result<T, String> + Clone + Send + 'static,
    ) -> Option<String> {
        self.pending.retain(|id, _| wanted.contains(id));
        self.failed.retain(|id, _| wanted.contains(id));
        self.pending.retain(|id, job| {
            match job.poll() {
                None => return true,
                Some(Ok(Ok(tile))) => {
                    let size = bytes(&tile);
                    if size > budget {
                        self.failed
                            .insert(*id, "Map tile exceeds the preparation memory budget".into());
                    } else {
                        self.bytes += size;
                        self.ready.insert(*id, Arc::new(tile));
                        self.order.push_back(*id);
                    }
                }
                Some(Err(little_clip_core::JobError::QueueFull)) => {}
                Some(Ok(Err(error))) => {
                    self.failed.insert(*id, error);
                }
                Some(Err(error)) => {
                    self.failed.insert(*id, error.to_string());
                }
            }
            false
        });
        self.order.retain(|id| !wanted.contains(id));
        self.order
            .extend(wanted.iter().filter(|id| self.ready.contains_key(id)));
        while self.bytes > budget {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(tile) = self.ready.remove(&old) {
                self.bytes -= bytes(&tile);
            }
        }
        for &id in wanted {
            if self.pending.len() >= 4 {
                break;
            }
            if self.ready.contains_key(&id)
                || self.pending.contains_key(&id)
                || self.failed.contains_key(&id)
            {
                continue;
            }
            let prepare = prepare.clone();
            self.pending.insert(
                id,
                Task::start(workers(), Duration::from_secs(60), move |cancel| {
                    prepare(id, cancel)
                }),
            );
        }
        self.failed.values().next().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_tiles_are_reported_once_without_blocking_the_next_view() {
        let mut cache = TileCache::<u8, Vec<u8>>::default();
        let wanted = BTreeSet::from([1]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let error = cache.update(&wanted, 8, Vec::len, |_, _| Ok(vec![0; 16]));
            if error.is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(cache.bytes, 0);
        assert!(cache.ready.is_empty() && cache.pending.is_empty());
        let next = BTreeSet::from([2]);
        loop {
            assert!(
                cache
                    .update(&next, 8, Vec::len, |_, _| Ok(vec![1; 4]))
                    .is_none()
            );
            if cache.ready.contains_key(&2) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(cache.bytes, 4);
        assert!(!cache.ready.contains_key(&1));
    }
}
