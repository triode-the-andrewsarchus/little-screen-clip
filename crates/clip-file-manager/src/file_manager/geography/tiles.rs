//! Viewer-owned, cancellable tile preparation with a fixed CPU memory budget.
use super::{
    detail,
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
const DETAIL_BUDGET: usize = 64 * 1024 * 1024;
const RIVER_BUDGET: usize = 64 * 1024 * 1024;
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
    detail: TileCache<TileId, detail::Tile>,
    scene_job: Option<Task<Result<Scene, String>>>,
    scene: Option<Arc<Scene>>,
    error: Option<String>,
    pub range: [f32; 2],
    pub contour: Option<f32>,
}

/// Geographic working set shared by one viewport or a collection of strips.
#[derive(Default)]
pub(super) struct Requests {
    raster: BTreeSet<TileId>,
    rivers: BTreeSet<RiverKey>,
}

impl Requests {
    fn new(raster: BTreeSet<TileId>, camera: &GlobeCamera, size: [f32; 2], rivers: bool) -> Self {
        let level = raster.iter().map(|tile| tile.level).max().unwrap_or(0);
        Self {
            raster,
            rivers: RiverKey::visible(camera, size, rivers, level),
        }
    }

    pub(super) fn preview(camera: &GlobeCamera, size: [f32; 2], pixels_per_point: f32) -> Self {
        let raster = little_clip_core::geography::basemap::tiles::preview_tiles(
            camera,
            size[0],
            size[1],
            pixels_per_point,
        );
        Self::new(raster, camera, size, true)
    }

    pub(super) fn include(&mut self, request: &Self) {
        self.raster.extend(request.raster.iter().copied());
        self.rivers.extend(request.rivers.iter().copied());
    }

    pub(super) fn is_empty(&self) -> bool {
        self.raster.is_empty() && self.rivers.is_empty()
    }

    pub(super) fn covers(&self, request: &Self) -> bool {
        request.raster.is_subset(&self.raster) && request.rivers.is_subset(&self.rivers)
    }
}
impl Default for Tiles {
    fn default() -> Self {
        Self {
            source: 0,
            assets: None,
            raster: TileCache::default(),
            rivers: TileCache::default(),
            detail: TileCache::default(),
            scene_job: None,
            scene: None,
            error: None,
            range: [0.0, 1000.0],
            contour: None,
        }
    }
}
impl Tiles {
    pub fn prepare_frame(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        assets: Option<&Arc<BasemapAssets>>,
        transport: Option<&detail::Source>,
        show_city_names: bool,
        camera: GlobeCamera,
        style: BasemapStyle,
        contours: bool,
        layers: Layers,
    ) -> Option<(gpu::Frame, Result<(), String>)> {
        let assets = assets?;
        let size = [rect.width(), rect.height()];
        let wanted = Requests::new(
            visible_tiles(&camera, size[0], size[1], ui.ctx().pixels_per_point())
                .into_iter()
                .collect(),
            &camera,
            size,
            layers.rivers,
        );
        let mut raster_result = self.prepare(ui.ctx(), assets, &wanted, CPU_BUDGET, RIVER_BUDGET);
        let wanted_detail = detail::visible(
            &camera,
            size,
            transport.is_some() && (layers.roads || show_city_names),
        );
        if let Some(source) = transport.cloned() {
            let error = self.detail.update(
                &wanted_detail,
                DETAIL_BUDGET,
                detail::Tile::bytes,
                move |key, cancel| source.prepare(key, cancel),
            );
            if !self.detail.pending.is_empty() {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
            if let Some(error) = error {
                raster_result = raster_result.and(Err(error));
            }
        } else {
            self.detail.pending.clear();
        }
        let mut frame = self.frame(wanted, camera, size, layers);
        frame.details = wanted_detail
            .into_iter()
            .filter_map(|id| self.detail.ready.get(&id).map(|tile| (id, tile.clone())))
            .collect();
        self.update_contours(&mut frame, style, contours);
        Some((frame, raster_result))
    }

    pub(super) fn prepare(
        &mut self,
        ctx: &egui::Context,
        assets: &Arc<BasemapAssets>,
        wanted: &Requests,
        raster_budget: usize,
        river_budget: usize,
    ) -> Result<(), String> {
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
        let source = assets.clone();
        let raster_error = self.raster.update(
            &wanted.raster,
            raster_budget,
            SourceTile::bytes,
            move |id, cancel| {
                SourceTile::prepare(&source, id, cancel)
                    .ok_or_else(|| "Map tile preparation cancelled".into())
            },
        );
        let source = assets.clone();
        let river_error = self.rivers.update(
            &wanted.rivers,
            river_budget,
            Scene::bytes,
            move |key, cancel| key.prepare(&source, cancel),
        );
        if !self.raster.pending.is_empty()
            || !self.rivers.pending.is_empty()
            || self.scene_job.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        match raster_error.or(river_error) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(super) fn frame(
        &self,
        wanted: Requests,
        camera: GlobeCamera,
        size: [f32; 2],
        layers: Layers,
    ) -> gpu::Frame {
        gpu::Frame {
            source: self.source,
            size,
            camera,
            style: BasemapStyle::Natural,
            contours: None,
            range: [0.0, 1000.0],
            tiles: wanted
                .raster
                .iter()
                .filter_map(|id| self.raster.ready.get(id).cloned())
                .collect(),
            scene: self.scene.clone(),
            rivers: wanted
                .rivers
                .iter()
                .filter_map(|key| {
                    self.rivers
                        .ready
                        .get(key)
                        .map(|scene| (*key, scene.clone()))
                })
                .collect(),
            layers,
            details: Vec::new(),
            sky: None,
            show_sky: false,
            earth_lighting: false,
            city_lights: false,
        }
    }

    fn update_contours(&mut self, frame: &mut gpu::Frame, style: BasemapStyle, contours: bool) {
        let finest = frame
            .tiles
            .iter()
            .map(|tile| tile.id.level)
            .max()
            .unwrap_or(0);
        let range = frame
            .tiles
            .iter()
            .filter(|tile| tile.id.level == finest)
            .filter_map(|tile| tile.elevation_range)
            .reduce(|a, b| [a[0].min(b[0]), a[1].max(b[1])]);
        self.range = range.unwrap_or([0.0, 1000.0]);
        self.contour = range
            .filter(|_| contours && frame.camera.zoom >= 3.5)
            .map(|[low, high]| {
                choose_contour_interval_m((high - low).max(20.0), frame.camera.zoom)
            });
        frame.style = style;
        frame.range = self.range;
        frame.contours = self.contour;
    }

    pub(super) fn suspend(&mut self) {
        self.raster.pending.clear();
        self.rivers.pending.clear();
        self.detail.pending.clear();
        self.scene_job = None;
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
