//! The visible browser strips share one surface and one bounded map working set.
use super::{
    geometry::Layers,
    gpu, paint_cities,
    tiles::{Requests, Tiles},
};
use crate::file_manager::geography_downloader::asset_workers;
use little_clip_core::{
    JobError,
    geography::basemap::{BasemapAssets, GlobeCamera, MAX_MAP_ZOOM, ProjectionMode, wrap_lon},
    jobs::task::Task,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Default)]
pub(in crate::file_manager) struct Strips {
    directory: Option<PathBuf>,
    surface: Surface,
    tiles: Tiles,
    requested: Requests,
    previous: Requests,
}

#[derive(Default)]
enum Surface {
    #[default]
    Unloaded,
    Loading(Task<BasemapAssets>),
    Ready(Arc<BasemapAssets>),
    Unavailable,
}

impl Strips {
    pub(in crate::file_manager) fn configure(&mut self, directory: PathBuf) {
        self.clear();
        self.directory = Some(directory);
    }

    pub(in crate::file_manager) fn clear(&mut self) {
        *self = Self {
            directory: self.directory.take(),
            ..Self::default()
        };
    }

    pub(in crate::file_manager) fn suspend(&mut self) {
        self.tiles.suspend();
        self.requested = Requests::default();
        self.previous = Requests::default();
        if matches!(self.surface, Surface::Loading(_)) {
            self.surface = Surface::Unloaded;
        }
    }

    pub(in crate::file_manager) fn poll(&mut self, ctx: &egui::Context) {
        let wanted = std::mem::take(&mut self.requested);
        if wanted.is_empty() {
            self.suspend();
            return;
        }
        if let Surface::Loading(job) = &self.surface {
            if let Some(result) = job.poll() {
                self.surface = match result {
                    Ok(assets) if has_context(&assets) => Surface::Ready(Arc::new(assets)),
                    Err(JobError::QueueFull) => Surface::Unloaded,
                    _ => Surface::Unavailable,
                };
            } else {
                ctx.request_repaint_after(Duration::from_millis(32));
            }
        }
        if let Surface::Ready(assets) = &self.surface
            && let Err(error) = self.tiles.prepare(ctx, assets, &wanted, 32 << 20, 16 << 20)
        {
            tracing::debug!(%error, "Geography strip map preparation incomplete");
        }
        self.previous = wanted;
    }

    pub(super) fn paint(&mut self, ui: &egui::Ui, rect: egui::Rect, camera: GlobeCamera) {
        let Some(directory) = &self.directory else {
            return;
        };
        if matches!(self.surface, Surface::Unloaded) {
            let directory = directory.clone();
            self.surface =
                Surface::Loading(Task::start_unbounded(asset_workers(), move |cancel| {
                    BasemapAssets::load_surface(&directory, cancel)
                }));
            ui.ctx().request_repaint_after(Duration::from_millis(32));
        }
        if matches!(self.surface, Surface::Unavailable) {
            return;
        }
        let size = [rect.width(), rect.height()];
        let wanted = Requests::preview(&camera, size, ui.ctx().pixels_per_point());
        if !self.previous.covers(&wanted) {
            ui.ctx().request_repaint();
        }
        self.requested.include(&wanted);
        let Surface::Ready(assets) = &self.surface else {
            return;
        };
        let frame = self.tiles.frame(
            wanted,
            camera,
            size,
            Layers {
                countries: true,
                states: true,
                rivers: true,
                roads: true,
            },
        );
        if gpu::paint(ui, rect, frame).is_ok() {
            paint_cities(ui, &ui.painter_at(rect), rect, &camera, assets, 0.0, 2);
        }
    }
}

fn has_context(assets: &BasemapAssets) -> bool {
    assets.ne2.is_some()
        || assets.ne2_full.is_some()
        || assets.land_cover.is_some()
        || assets.rivers.is_some()
        || assets.grwl_summary.is_some()
        || assets.grwl_masks.is_some()
        || !assets.country_borders.is_empty()
        || !assets.state_borders.is_empty()
        || !assets.lakes.is_empty()
        || !assets.roads.is_empty()
        || !assets.cities.is_empty()
        || !assets.coastlines.is_empty()
}

/// The projected track and basemap use the same fitted flat viewport.
pub(super) fn camera(rect: egui::Rect, [west, south, east, north]: [f64; 4]) -> GlobeCamera {
    let mut camera = GlobeCamera {
        center_lat: (south + north) * 0.5,
        center_lon: wrap_lon((west + east) * 0.5),
        zoom: 1.0,
        mode: ProjectionMode::Map2D,
    };
    let cos_lat = camera.center_lat.to_radians().cos().abs().max(0.25);
    let horizontal = (east - west).max(0.001).to_radians() * cos_lat;
    let vertical = (north - south).max(0.001).to_radians();
    let scale = (f64::from((rect.width() - 18.0).max(1.0)) / horizontal)
        .min(f64::from((rect.height() - 18.0).max(1.0)) / vertical);
    camera.zoom = (scale / camera.scale_px(rect.width(), rect.height())).min(MAX_MAP_ZOOM);
    camera
}
