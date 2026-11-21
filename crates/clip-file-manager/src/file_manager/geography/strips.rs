//! The visible browser strips share one surface and one bounded map working set.
use super::{
    geometry::Layers,
    gpu, paint_cities,
    tiles::{Requests, Tiles},
};
use crate::file_manager::geography_downloader::asset_workers;
use little_clip_core::{
    JobError,
    geography::{
        basemap::{BasemapAssets, GlobeCamera},
        preview::City,
    },
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

    pub(super) fn paint(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        camera: GlobeCamera,
        show_city_labels: bool,
    ) {
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
        if gpu::paint(ui, rect, frame).is_ok() && show_city_labels {
            paint_cities(
                ui,
                &ui.painter_at(rect),
                rect,
                &camera,
                assets,
                0.0,
                (2, &[]),
                None,
            );
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

pub(super) fn paint_city(
    painter: &egui::Painter,
    rect: egui::Rect,
    camera: GlobeCamera,
    city: City,
) {
    use egui::{Color32, FontId, Rect, pos2, vec2};
    let [lat, lon] = city.lat_lon;
    let Some([x, y]) = camera.project(lat, lon, rect.width(), rect.height()) else {
        return;
    };
    let point = rect.min + vec2(x, y);
    let galley =
        painter.layout_no_wrap(city.name.into(), FontId::proportional(12.0), Color32::WHITE);
    let size = galley.size() + vec2(8.0, 4.0);
    let left = if point.x + size.x + 6.0 <= rect.right() {
        point.x + 5.0
    } else {
        point.x - size.x - 5.0
    };
    let top = (point.y - size.y * 0.5).clamp(rect.top(), (rect.bottom() - size.y).max(rect.top()));
    let label = Rect::from_min_size(pos2(left.max(rect.left()), top), size);
    painter.rect_filled(label, 3.0, Color32::from_black_alpha(200));
    painter.galley(label.min + vec2(4.0, 2.0), galley, Color32::WHITE);
    painter.circle_filled(point, 3.0, Color32::from_rgb(255, 214, 112));
}
