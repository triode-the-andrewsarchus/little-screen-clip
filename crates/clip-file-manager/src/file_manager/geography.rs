//! Offline 3D-globe / 2D-map geography viewer with Oilslick elevation shading and linked profile.
pub(super) mod detail;
mod finder;
mod geometry;
pub(super) mod gpu;
pub(super) mod lunar_terrain;
mod map_camera;
mod overlay;
mod radiation;
mod river_coverage;
pub(super) mod sky;
pub(super) mod solar;
mod strips;
mod terrain_download;
mod tiles;

use super::{
    filesystem::Entry,
    geography_downloader::{
        AssetDownloader, DownloadPack, asset_workers, dir_size, solar_download_bytes,
    },
    media::PreviewData,
    shelf::preview::Ready,
};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    ui_appearance::WorkbenchColors,
};
use chrono::{DateTime, TimeDelta, Utc};
use little_clip_core::{
    config::AppConfig,
    geography::{
        Document, Feature, Kind, MAX_BYTES, Sample,
        basemap::{
            BasemapAssets, BasemapStyle, GlobeCamera, MAX_MAP_ZOOM, MIN_MAP_ZOOM, ProjectionMode,
            oilslick_color,
        },
        units::Units,
    },
    jobs::task::Task,
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn read(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BYTES)?;
    if token.load(Ordering::Relaxed) {
        return Err("Geography opening cancelled".into());
    }
    let extension = path.extension().unwrap_or_default().to_ascii_lowercase();
    match extension.to_str() {
        Some("geojson") => Document::geojson(&bytes),
        Some("rctrk") => Document::radiacode(&bytes),
        _ => Document::gpx(&bytes),
    }
}

pub(super) use strips::Strips;

pub(in crate::file_manager) struct Preview {
    document: Document,
    overlay: overlay::Overlay,
}

pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Geography".into(), Icon::Image);
    match read(path, token) {
        Ok(mut document) => {
            data.detail = format!(
                "{} shapes · {} positions · partial map preview",
                document.features.len(),
                document.points
            );
            // Keep source-order indices while retaining only bounded drawing data in row caches.
            document.features.truncate(32);
            let mut remaining = 512;
            for feature in &mut document.features {
                feature.name = feature.name.chars().take(80).collect();
                feature.properties.clear();
                feature.paths.truncate(32);
                for path in &mut feature.paths {
                    let step = path.len().div_ceil(remaining.max(1)).max(1);
                    *path = path
                        .iter()
                        .step_by(step)
                        .take(remaining)
                        .cloned()
                        .map(|mut sample| {
                            sample.point.time = String::new();
                            sample
                        })
                        .collect();
                    remaining = remaining.saturating_sub(path.len());
                }
                feature.paths.retain(|path| !path.is_empty());
            }
            document.samples = document
                .features
                .iter()
                .flat_map(|feature| &feature.paths)
                .map(Vec::len)
                .sum();
            document.title = String::new();
            document.notes.clear();
            data.geography = Some(Arc::new(Preview {
                overlay: overlay::Overlay::prepare(&document, false),
                document,
            }));
        }
        Err(error) => data.detail = error,
    }
    data
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Hit {
    feature: usize,
    path: usize,
    point: usize,
}

pub(super) struct Viewer {
    document: Document,
    overlay: overlay::Overlay,
    interpolate_gpx: bool,
    selected: usize,
    current: Option<Hit>,
    camera: GlobeCamera,
    moon_camera: Option<GlobeCamera>,
    solar_system: Option<solar::System>,
    world: Option<solar::world::World>,
    world_maps: Arc<solar::assets::MapCache>,
    finder: finder::Finder,
    sight: Option<finder::Sight>,
    found_place: Option<([f64; 2], Arc<str>)>,
    middle_drag: map_camera::MiddleDrag,
    style: BasemapStyle,
    show_country_borders: bool,
    show_state_borders: bool,
    labels: detail::LabelLayers,
    show_rivers: bool,
    show_roads: bool,
    show_contours: bool,
    contour_interval_m: Option<f32>,
    local_elev_range: [f32; 2],
    config: AppConfig,
    downloader: AssetDownloader,
    assets: Option<Arc<BasemapAssets>>,
    assets_job: Option<Task<(Arc<BasemapAssets>, u64, detail::Status)>>,
    prompt_dismissed: bool,
    show_assets_panel: bool,
    tiles: tiles::Tiles,
    render_error: Option<String>,
    terrain_download: terrain_download::Regional,
    sky: SkyState,
}

struct SkyState {
    assets: Option<Arc<sky::Assets>>,
    job: Option<Task<Option<Result<sky::Assets, String>>>>,
    last: Option<(DateTime<Utc>, Option<sky::Scene>)>,
    transition: Option<SceneTransition>,
    visible: bool,
    readable: bool,
    constellations: bool,
    lighting: bool,
    error: Option<String>,
}

const SCENE_TRANSITION_SECONDS: f64 = 0.1;

struct SceneTransition {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    started_at: f64,
}

impl SceneTransition {
    fn at(&self, now: f64) -> DateTime<Utc> {
        let t = ((now - self.started_at) / SCENE_TRANSITION_SECONDS).clamp(0.0, 1.0);
        interpolate_instant(self.from, self.to, t * t * (3.0 - 2.0 * t))
    }
}

fn interpolate_instant(from: DateTime<Utc>, to: DateTime<Utc>, fraction: f64) -> DateTime<Utc> {
    if fraction >= 1.0 {
        return to;
    }
    let delta = to - from;
    if let Some(nanos) = delta.num_nanoseconds() {
        from + TimeDelta::nanoseconds((nanos as f64 * fraction).round() as i64)
    } else {
        from + TimeDelta::milliseconds((delta.num_milliseconds() as f64 * fraction).round() as i64)
    }
}

impl SkyState {
    fn new() -> Self {
        Self {
            assets: None,
            job: None,
            last: None,
            transition: None,
            visible: true,
            readable: true,
            constellations: false,
            lighting: true,
            error: None,
        }
    }

    fn reload(&mut self, root: &Path) {
        let root = root.to_owned();
        self.job = Some(Task::start_unbounded(asset_workers(), move |cancel| {
            if cancel.load(Ordering::Relaxed)
                || !root.join(sky::DIRECTORY).join(sky::KERNEL).is_file()
            {
                return None;
            }
            Some(sky::Assets::load(&root))
        }));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        if let Some(result) = job.poll() {
            self.job = None;
            self.last = None;
            self.transition = None;
            match result {
                Ok(Some(Ok(assets))) => {
                    self.assets = Some(Arc::new(assets));
                    self.error = None;
                }
                Ok(Some(Err(error))) => self.error = Some(error),
                Ok(None) => {
                    self.assets = None;
                    self.error = None;
                }
                Err(little_clip_core::JobError::QueueFull) => {}
                Err(error) => self.error = Some(error.to_string()),
            }
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    fn at(&mut self, time: DateTime<Utc>) -> Option<sky::Scene> {
        let assets = self.assets.as_ref()?;
        if let Some((last, scene)) = &self.last
            && *last == time
        {
            return scene.clone();
        }
        let scene = assets.at(time);
        self.last = Some((time, scene.clone()));
        scene
    }

    fn scene_time(
        &mut self,
        recorded: Option<DateTime<Utc>>,
        ctx: &egui::Context,
    ) -> (DateTime<Utc>, f32) {
        let Some(target) = recorded else {
            self.transition = None;
            return (live_scene_time(ctx), 0.0);
        };
        let Some((previous, _)) = self.last else {
            return (target, 0.0);
        };
        let now = ctx.input(|input| input.time);
        let current_target = self
            .transition
            .as_ref()
            .map_or(previous, |active| active.to);
        if target != current_target {
            let from = self
                .transition
                .as_ref()
                .map_or(previous, |active| active.at(now));
            self.transition = Some(SceneTransition {
                from,
                to: target,
                started_at: now,
            });
        }
        let Some(active) = &self.transition else {
            return (target, 0.0);
        };
        if now >= active.started_at + SCENE_TRANSITION_SECONDS {
            self.transition = None;
            return (target, 0.0);
        }
        let time = active.at(now);
        ctx.request_repaint();
        // Blur by the right ascension the stars have travelled since the
        // previous frame, capped at six texels of the 4K star map.
        let turns = (time - previous).num_milliseconds() as f64 / 86_164_090.0;
        let short_turn = turns - turns.round();
        let trail = short_turn.clamp(-6.0 / 4096.0, 6.0 / 4096.0) as f32;
        (time, trail)
    }
}

fn recorded_scene_time(
    document: &Document,
    selected: usize,
    hovered: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    hovered
        .or(document.features[selected].last_instant)
        .or_else(|| {
            document
                .features
                .iter()
                .rev()
                .find_map(|feature| feature.last_instant)
        })
        .or(document.capture_time)
}

impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = read(path, &AtomicBool::new(false))?;
        let selected = document
            .features
            .iter()
            .enumerate()
            .max_by_key(|(_, f)| f.points)
            .map_or(0, |(i, _)| i);
        let config = AppConfig::load();
        let downloader = AssetDownloader::new(&config);
        let assets_dir = downloader.assets_dir.clone();
        let override_dir = downloader.terrain_override_dir.clone();
        let job = Task::start_unbounded(asset_workers(), move |cancel| {
            let assets = Arc::new(BasemapAssets::load_with_cancel(
                &assets_dir,
                override_dir.as_deref(),
                cancel,
            ));
            (
                assets,
                dir_size(&assets_dir),
                detail::Status::load(&assets_dir),
            )
        });

        let interpolate_gpx = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gpx"));
        let mut viewer = Self {
            overlay: overlay::Overlay::prepare(&document, interpolate_gpx),
            document,
            interpolate_gpx,
            selected,
            current: None,
            camera: GlobeCamera::default(),
            moon_camera: None,
            solar_system: None,
            world: None,
            world_maps: Arc::default(),
            finder: Default::default(),
            sight: None,
            found_place: None,
            middle_drag: Default::default(),
            style: BasemapStyle::Natural,
            show_country_borders: true,
            show_state_borders: true,
            labels: detail::LabelLayers {
                cities: true,
                detailed: false,
            },
            show_rivers: true,
            show_roads: true,
            show_contours: true,
            contour_interval_m: None,
            local_elev_range: [0.0, 1000.0],
            config,
            downloader,
            assets: None,
            assets_job: Some(job),
            prompt_dismissed: false,
            show_assets_panel: false,
            tiles: tiles::Tiles::default(),
            render_error: None,
            terrain_download: terrain_download::Regional::default(),
            sky: SkyState::new(),
        };
        viewer.sky.reload(&viewer.downloader.assets_dir);
        viewer.fit_document();
        viewer.select(selected);
        Ok(viewer)
    }

    pub fn writes_pending(&self) -> bool {
        self.downloader.active.is_some()
    }

    pub fn select(&mut self, index: usize) {
        if index < self.document.features.len() {
            self.selected = index;
            self.current = self.document.features[index]
                .paths
                .first()
                .filter(|p| !p.is_empty())
                .map(|_| Hit {
                    feature: index,
                    path: 0,
                    point: 0,
                });
        }
    }

    fn fit_document(&mut self) {
        self.solar_system = None;
        self.world = None;
        self.sight = None;
        self.found_place = None;
        self.moon_camera = None;
        self.camera.fit_bounds(self.document.geographic_bounds);
    }

    pub(in crate::file_manager) fn poll_background(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.assets_job {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
            if let Some(result) = job.poll() {
                self.assets_job = None;
                match result {
                    Ok((loaded, disk_usage, transport)) => {
                        self.downloader.transport = transport;
                        self.downloader.disk_usage_bytes = disk_usage;
                        self.downloader.status = loaded.status();
                        self.assets = Some(loaded);
                    }
                    Err(error) => {
                        self.downloader.last_error =
                            Some(format!("Asset loading failed: {error:?}"))
                    }
                }
                ctx.request_repaint();
            }
        }
        self.sky.poll(ctx);
        let active = self.downloader.active;
        let downloaded = self.downloader.poll(ctx);
        if self.downloader.active.is_none() {
            self.refresh_downloaded_astronomy(active);
        }
        if let Some(downloaded) = downloaded {
            self.terrain_download.invalidate_coverage();
            if matches!(active, Some(DownloadPack::Sky | DownloadPack::StarCatalog)) {
                self.sky.reload(&self.downloader.assets_dir);
            }
            self.assets = Some(downloaded);
            ctx.request_repaint();
        }
    }

    fn refresh_downloaded_astronomy(&mut self, finished: Option<DownloadPack>) {
        match finished {
            Some(DownloadPack::MoonRelief) => self.tiles.notice_lunar_assets(),
            Some(DownloadPack::SolarSystem) => {
                self.world_maps.refresh_installed(None);
                self.tiles.notice_lunar_assets();
                self.sky.reload(&self.downloader.assets_dir);
                if let Some(world) = &mut self.world {
                    world.reload();
                }
                if let Some(system) = &mut self.solar_system {
                    system.reload_maps();
                }
            }
            Some(DownloadPack::World(id)) => {
                self.world_maps.refresh_installed(Some(id));
                if let Some(world) = &mut self.world {
                    world.reload();
                }
                if let Some(system) = &mut self.solar_system {
                    system.reload_maps();
                }
                if id == solar::MOON {
                    self.sky.reload(&self.downloader.assets_dir);
                }
            }
            _ => {}
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        self.finder.shortcut(ui.ctx());
        StudioShell::new("Geography", Icon::Image, palette, close).render_header(ui, |ui| {
            if ui.button("Find").on_hover_text("Ctrl+F").clicked() {
                self.finder.open();
            }
            if self.downloader.active == Some(DownloadPack::SolarSystem) {
                self.download_progress(ui);
            }
        });

        let colors = WorkbenchColors::from_palette(palette);
        let showing_earth =
            self.solar_system.is_none() && self.world.is_none() && self.sight.is_none();
        if showing_earth || self.show_assets_panel {
            self.download_prompt_banner(ui, palette, colors, showing_earth);
        }

        if let Some(world) = self.world.as_mut() {
            world.poll(ui.ctx(), &self.downloader.assets_dir, &self.world_maps);
        }
        let layout = StudioPanelLayout {
            left: showing_earth,
            right: self.finder.open,
            ..StudioPanelLayout::default()
        }
        .allocate(ui);
        if let Some(right) = layout.right {
            let found = ui
                .scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                    ui.set_clip_rect(right.intersect(ui.clip_rect()));
                    self.finder.ui(ui, &self.document, self.sky.assets.as_ref())
                })
                .inner;
            if let Some(hit) = found {
                self.apply_found(hit, ui.ctx());
            }
        }
        if let Some(left) = layout.left {
            ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.details(ui, palette);
            });
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(layout.center), |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            self.toolbar(ui, palette);

            if self.paint_astronomy(ui, colors) {
                return;
            }

            self.paint_map(ui, colors);
        });
    }

    fn paint_map(&mut self, ui: &mut egui::Ui, colors: WorkbenchColors) {
        let feature = &self.document.features[self.selected];
        let profile_visible = self.moon_camera.is_none()
            && feature.kind != Kind::Point
            && feature.elevation.is_some()
            && feature.points > 1;
        let mut available = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        available.max.y -= ui.spacing().item_spacing.y;
        if !available.is_positive() {
            return;
        }
        // Measure the bottom rows in their actual layout so wrapped labels,
        // legends and errors never compete with the map for the same space.
        let mut footer = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("map-footer")
                .max_rect(available)
                .layout(egui::Layout::bottom_up(egui::Align::Min)),
        );
        let profile = profile_visible.then(|| {
            footer
                .allocate_exact_size(
                    egui::vec2(available.width(), 160.0_f32.min(available.height() * 0.4)),
                    egui::Sense::hover(),
                )
                .0
        });
        self.map_status(&mut footer);
        if let Some(error) = &self.render_error {
            footer.colored_label(colors.warning, error);
        }
        if self.style == BasemapStyle::Oilslick {
            paint_oilslick_legend(&mut footer, colors, self.config.geography.units);
        }
        if let Some(scale) = feature.dose_rate.map(radiation::DoseScale) {
            scale.legend(&mut footer);
        }
        let map_height = footer.available_height();
        if map_height < 1.0 {
            return;
        }
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.width(), map_height),
            egui::Sense::click_and_drag(),
        );

        let size = [rect.width(), rect.height()];
        let lunar_radius = self.moon_camera.map(|_| map_camera::MOON_RADIUS_M);
        if map_camera::navigate(
            self.moon_camera.as_mut().unwrap_or(&mut self.camera),
            &response,
            lunar_radius,
            &mut self.middle_drag,
        ) {
            self.open_solar_system(solar::SUN, ui.ctx());
            return;
        }
        ui.painter_at(rect).rect_filled(rect, 3.0, colors.recessed);
        let slot = ui.painter_at(rect).add(egui::Shape::Noop);
        if let Some(camera) = self.moon_camera.as_mut() {
            map_camera::update_ground(camera, size, map_camera::MOON_RADIUS_M, |lat, lon| {
                self.tiles.lunar_height_at(lat, lon)
            });
            let mut frame = self.tiles.moon_frame(
                ui.ctx(),
                self.assets.as_ref(),
                &self.downloader.assets_dir,
                *camera,
                size,
            );
            frame.0.lunar_albedo = self.world.as_ref().and_then(solar::world::World::texture);
            self.paint_frame(ui, &response, slot, Some(frame), None);
            return;
        }
        self.paint_earth_map(ui, &response, slot, profile, colors);
    }

    fn paint_earth_map(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        slot: egui::layers::ShapeIdx,
        profile: Option<egui::Rect>,
        colors: WorkbenchColors,
    ) {
        let rect = response.rect;
        let size = [rect.width(), rect.height()];
        let prepared = self.prepare_earth_frame(ui, rect);

        let frame = prepared.as_ref().map(|(frame, _)| frame);
        let detail_tiles = frame.map_or(&[][..], |frame| frame.details.as_slice());
        let surface = frame.and_then(|frame| frame.terrain.as_deref());
        let draped = map_camera::Draped::new(&self.camera, surface, size);
        let hit = self.paint_globe_map(ui, rect, colors, detail_tiles, draped);
        if response.clicked()
            && let Some(hit) = hit
        {
            self.selected = hit.sample.feature;
            self.current = Some(hit.sample);
        }
        self.paint_selected_position(ui, rect, colors, draped);
        self.paint_found_place(ui, rect, colors, draped);

        let hovered = profile
            .and_then(|profile| self.profile_hover(ui, profile, colors))
            .or_else(|| {
                hit.map(|pick| {
                    if self.interpolate_gpx {
                        pick.time
                    } else {
                        self.sample(pick.sample)
                            .and_then(|sample| sample.point.instant())
                    }
                })
            });
        self.paint_frame(ui, response, slot, prepared, hovered);
    }

    fn paint_astronomy(&mut self, ui: &mut egui::Ui, colors: WorkbenchColors) -> bool {
        if self.sight.is_none()
            && self.solar_system.is_none()
            && (self.world.is_none() || self.moon_camera.is_some())
        {
            return false;
        }
        let available = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        if !available.is_positive() {
            return true;
        }
        let (rect, response) =
            ui.allocate_exact_size(available.size(), egui::Sense::click_and_drag());
        if let Some(sight) = self.sight.as_mut() {
            if sight.navigate(&response) {
                self.open_solar_system(solar::SUN, ui.ctx());
            } else {
                sight.paint(
                    ui,
                    &response,
                    self.sky.assets.clone(),
                    self.sky.constellations,
                );
            }
        } else if let Some(system) = self.solar_system.as_mut() {
            system.poll_maps(
                ui.ctx(),
                &self.downloader.assets_dir,
                self.assets.clone(),
                &self.world_maps,
            );
            let recorded = recorded_scene_time(&self.document, self.selected, None);
            if let Some(selection) = system.paint(
                ui,
                &response,
                recorded.unwrap_or_else(|| live_scene_time(ui.ctx())),
                self.sky.assets.clone(),
                self.sky.constellations,
                &self.world_maps,
            ) {
                self.arrive_at_world(selection, ui.ctx());
            }
        } else if let Some(world) = self.world.as_mut() {
            if world.navigate(&response) {
                self.open_solar_system(solar::SUN, ui.ctx());
                return true;
            }
            let time = recorded_scene_time(&self.document, self.selected, None)
                .unwrap_or_else(|| live_scene_time(ui.ctx()));
            match world.paint(
                ui,
                &response,
                time,
                self.sky.assets.clone(),
                self.sky.constellations,
                &self.world_maps,
            ) {
                Ok(Some(solar::SUN)) => self.open_solar_system(solar::SUN, ui.ctx()),
                Ok(Some(id)) => self.select_solar_target(id, ui.ctx()),
                Ok(None) => {}
                Err(error) => {
                    ui.painter_at(rect).text(
                        rect.left_top() + egui::vec2(8.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        error,
                        egui::FontId::proportional(14.0),
                        colors.warning,
                    );
                }
            }
        }
        true
    }

    fn apply_found(&mut self, hit: finder::Hit, ctx: &egui::Context) {
        match hit.target {
            finder::Target::Body(id) => self.select_solar_target(id, ctx),
            finder::Target::Place(kind, [latitude, longitude]) => {
                self.select_solar_target(solar::EARTH, ctx);
                self.camera.center_lat = latitude;
                self.camera.center_lon = longitude;
                self.camera.zoom = kind.zoom();
                self.camera.orbit = Default::default();
                self.found_place = Some(([latitude, longitude], hit.label));
            }
            finder::Target::Star(_) | finder::Target::Constellation(_) => {
                self.select_solar_target(solar::EARTH, ctx);
                self.sight = finder::Sight::from_hit(&hit);
            }
            finder::Target::Feature(index) => {
                self.select_solar_target(solar::EARTH, ctx);
                self.select(index);
                if let Some(point) = self
                    .current
                    .and_then(|sample| self.sample(sample))
                    .map(|sample| &sample.point)
                {
                    let (latitude, longitude) = (point.lat, point.lon);
                    self.camera.center_lat = latitude;
                    self.camera.center_lon = longitude;
                    self.camera.orbit = Default::default();
                }
            }
        }
        ctx.request_repaint();
    }

    fn paint_found_place(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        colors: WorkbenchColors,
        draped: Option<map_camera::Draped<'_>>,
    ) {
        let Some(([lat, lon], name)) = &self.found_place else {
            return;
        };
        let projected = match draped {
            Some(projector) => projector.project(*lat, *lon),
            None => self.camera.project(*lat, *lon, rect.width(), rect.height()),
        };
        let Some(point) = projected
            .map(|[x, y]| rect.min + egui::vec2(x, y))
            .filter(|p| rect.contains(*p))
        else {
            return;
        };
        let painter = ui.painter_at(rect);
        painter.circle_stroke(point, 7.5, egui::Stroke::new(2.0, colors.focus));
        painter.circle_filled(point, 2.5, colors.text_primary);
        let galley = painter.layout_no_wrap(
            name.to_string(),
            egui::FontId::proportional(13.0),
            colors.text_primary,
        );
        let label = egui::Rect::from_min_size(
            point + egui::vec2(10.0, -galley.size().y / 2.0),
            galley.size(),
        );
        painter.rect_filled(label.expand2(egui::vec2(4.0, 2.0)), 3.0, colors.recessed);
        painter.galley(label.min, galley, colors.text_primary);
    }

    fn prepare_earth_frame(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
    ) -> Option<(gpu::Frame, Result<(), String>)> {
        let size = [rect.width(), rect.height()];
        self.terrain_download.viewport = size;
        let mut prepared = self.tiles.prepare_frame(
            ui,
            rect,
            self.assets.as_ref(),
            self.downloader.transport.source.as_ref(),
            self.labels,
            self.camera,
            self.style,
            self.show_contours,
            geometry::Layers {
                countries: self.show_country_borders,
                states: self.show_state_borders,
                rivers: self.show_rivers,
                roads: self.show_roads,
            },
        );
        map_camera::follow_ground(&mut self.camera, &self.tiles, size);
        if let Some((frame, _)) = &mut prepared {
            if (frame.camera.orbit.target_height_m - self.camera.orbit.target_height_m).abs() > 0.5
                || (frame.camera.orbit.eye_ground_m - self.camera.orbit.eye_ground_m).abs() > 0.5
            {
                ui.ctx().request_repaint();
            }
            frame.camera = self.camera;
            gpu::prepare_surface(ui.ctx(), frame);
        }
        self.contour_interval_m = self.tiles.contour;
        self.local_elev_range = self.tiles.range;
        prepared
    }

    fn paint_selected_position(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        colors: WorkbenchColors,
        draped: Option<map_camera::Draped<'_>>,
    ) {
        if let Some(sample) = self.current.and_then(|hit| self.sample(hit))
            && let Some([px, py]) = match draped {
                Some(pose) => pose.project(sample.point.lat, sample.point.lon),
                None => self.camera.project(
                    sample.point.lat,
                    sample.point.lon,
                    rect.width(),
                    rect.height(),
                ),
            }
        {
            for copy in self.camera.world_shifts(rect.width(), rect.height()) {
                if self.camera.globe_blend() > 0.0 && copy != 0 {
                    continue;
                }
                let offset =
                    copy as f32 * self.camera.world_width_px(rect.width(), rect.height()) as f32;
                let point = egui::pos2(rect.left() + px + offset, rect.top() + py);
                if rect.contains(point) {
                    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                    painter.circle_stroke(point, 7.0, egui::Stroke::new(2.0_f32, colors.focus));
                    painter.circle_filled(point, 2.5, colors.text_primary);
                }
            }
        }
    }

    fn map_status(&self, ui: &mut egui::Ui) {
        if let Some(camera) = self.moon_camera {
            ui.label(format!("Offline Moon ({:.2}×)", camera.zoom))
                .on_hover_text(map_camera::CONTROLS);
            return;
        }
        let blend = self.camera.globe_blend();
        let projection = if self.camera.orbit.active() {
            "3D Terrain"
        } else if blend > 0.9 {
            "3D Globe"
        } else if blend < 0.1 {
            "2D Map"
        } else {
            "3D → 2D Transition"
        };
        let terrain =
            if self.downloader.status.has_gebco || self.downloader.status.local_hgt_count > 0 {
                "DEM loaded"
            } else {
                "Terrain unavailable"
            };
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Offline {projection} ({:.2}×) · {terrain}",
                self.camera.zoom
            ))
            .on_hover_text(map_camera::CONTROLS);
            if self.downloader.transport.source.is_some()
                && (self.show_roads || self.labels.cities || self.labels.detailed)
            {
                ui.hyperlink_to(
                    &detail::manifest().attribution,
                    "https://docs.overturemaps.org/attribution/",
                );
            }
        });
    }

    fn profile_hover(
        &mut self,
        ui: &egui::Ui,
        profile: egui::Rect,
        colors: WorkbenchColors,
    ) -> Option<Option<DateTime<Utc>>> {
        let (hit, time) = paint_profile(
            ui,
            profile,
            &self.document.features[self.selected],
            self.selected,
            self.current,
            colors,
            self.config.geography.units,
            self.interpolate_gpx,
        )?;
        if Some(hit) != self.current {
            self.current = Some(hit);
            ui.ctx().request_repaint();
        }
        Some(time)
    }

    fn paint_frame(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        slot: egui::layers::ShapeIdx,
        prepared: Option<(gpu::Frame, Result<(), String>)>,
        hovered: Option<Option<DateTime<Utc>>>,
    ) {
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        let moon = self.moon_camera.is_some();
        let camera = self.moon_camera.as_ref().unwrap_or(&self.camera);
        let scale_badge = paint_scale_and_contour_overlay(
            ui,
            &painter,
            rect,
            camera,
            self.contour_interval_m.filter(|_| !moon),
            self.local_elev_range,
            self.config.geography.units,
            moon,
        );
        let Some((mut frame, result)) = prepared else {
            return;
        };
        let visible = self.sky.visible && frame.camera.globe_blend() > 0.0;
        frame.moon_view = moon;
        if self.sky.assets.is_some() && (visible || self.sky.lighting || moon) {
            let recorded = recorded_scene_time(&self.document, self.selected, hovered.flatten());
            let (time, star_trail) = self.sky.scene_time(recorded, ui.ctx());
            frame.sky = self.sky.at(time);
            frame.star_trail = star_trail;
            paint_scene_clock(ui, &painter, rect, scale_badge, time, frame.sky.as_ref());
            frame.show_sky = visible;
            frame.readable_sky = self.sky.readable;
            frame.show_constellations = self.sky.constellations;
            frame.earth_lighting = self.sky.lighting;
        }
        if hovered.is_none() {
            show_sky_object(ui, rect, &frame);
            if response.clicked()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                if let Some(gpu::SkyObject::Body(name)) = gpu::sky_object_at(&frame, rect, pointer)
                {
                    match name {
                        "Sun" => self.open_solar_system(solar::SUN, ui.ctx()),
                        "Earth" => self.select_solar_target(solar::EARTH, ui.ctx()),
                        "Moon" => self.select_solar_target(solar::MOON, ui.ctx()),
                        _ => {
                            if let Some(body) =
                                solar::satellites(solar::SUN).find(|body| body.name == name)
                            {
                                self.select_solar_target(body.id, ui.ctx());
                            }
                        }
                    }
                }
            }
        }
        let error = gpu::paint_at(ui, rect, frame, Some(slot)).and(result).err();
        if self.render_error != error {
            self.render_error = error;
            ui.ctx().request_repaint();
        }
    }

    fn paint_globe_map(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        colors: WorkbenchColors,
        detail_tiles: &[(
            little_clip_core::geography::basemap::tiles::TileId,
            Arc<detail::Tile>,
        )],
        draped: Option<map_camera::Draped<'_>>,
    ) -> Option<overlay::Pick> {
        let painter = ui.painter_at(rect);
        let pointer = super::ui::preview_hovered(ui, rect)
            .then(|| ui.input(|i| i.pointer.hover_pos()))
            .flatten();
        let mut hit = None;
        let camera = &self.camera;
        let flat = camera.globe_blend() == 0.0;
        let period = camera.world_width_px(rect.width(), rect.height());
        let cities = self.assets.as_deref().filter(|_| self.labels.cities);
        detail::paint_labels(
            ui,
            rect,
            self.camera,
            detail_tiles,
            self.labels,
            self.show_roads,
            draped,
        );
        for copy in camera.world_shifts(rect.width(), rect.height()) {
            if copy != 0
                && (!flat || (copy.abs() as f64 - 0.5) * period > rect.width() as f64 * 0.5 + 160.0)
            {
                continue;
            }
            let offset = (copy as f64 * period) as f32;
            if let Some(assets) = cities {
                paint_cities(
                    ui,
                    &painter,
                    rect,
                    camera,
                    assets,
                    offset,
                    (56, detail_tiles),
                    draped,
                );
            }
            let projection = overlay::Projection {
                camera,
                rect,
                offset,
                draped,
            };
            if let Some(candidate) =
                self.overlay
                    .paint_copy(&painter, projection, Some(self.selected), colors, pointer)
                && candidate.precedes(hit)
            {
                hit = Some(candidate);
            }
        }
        if let Some(hit) = hit
            && let Some(sample) = self.sample(hit.sample)
        {
            show_position(ui, &self.document.features[hit.sample.feature].name, sample);
        }
        hit
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if let Some(sight) = self.sight.as_ref() {
            self.sky_toolbar(ui, palette, sight.title.clone());
        } else if let Some(center) = self.solar_system.as_ref().map(|view| view.center) {
            self.solar_toolbar(ui, palette, center);
        } else if self.moon_camera.is_some() {
            self.moon_toolbar(ui, palette);
        } else if self.world.is_some() {
            self.world_toolbar(ui, palette);
        } else {
            self.earth_toolbar(ui, palette);
        }
    }

    fn sky_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette, title: Arc<str>) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Return to Earth").clicked() {
                self.select_solar_target(solar::EARTH, ui.ctx());
            }
            ui.separator();
            ui.strong(title.as_ref());
            self.shared_sky_controls(ui, palette);
        });
    }

    fn moon_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let target = ui
            .horizontal_wrapped(|ui| {
                let camera = self.moon_camera.as_mut()?;
                let earth = map_camera::lunar_toolbar(ui, palette, camera, &mut self.sky);
                ui.separator();
                let orbit = ui.button("Moon orbit").clicked();
                self.shared_sky_controls(ui, palette);
                Some((earth, orbit))
            })
            .inner;
        match target {
            Some((true, _)) => self.select_solar_target(solar::EARTH, ui.ctx()),
            Some((_, true)) => self.open_solar_system(solar::EARTH, ui.ctx()),
            _ => {
                ui.horizontal_wrapped(|ui| {
                    self.lunar_download(ui);
                    self.body_assets(ui);
                })
                .inner
            }
        }
    }

    fn earth_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let terrain_available =
            self.downloader.status.has_gebco || self.downloader.status.local_hgt_count > 0;
        ui.horizontal_wrapped(|ui| {
            for (label, mode) in [
                ("Auto 3D→2D", ProjectionMode::Auto),
                ("3D Globe", ProjectionMode::Globe3D),
                ("2D Map", ProjectionMode::Map2D),
            ] {
                if ui
                    .selectable_label(self.camera.mode == mode && !self.camera.orbit.active(), label)
                    .clicked()
                {
                    self.camera.mode = mode;
                    self.camera.orbit = Default::default();
                }
            }
            ui.separator();
            for (label, style) in [
                ("Natural", BasemapStyle::Natural),
                ("Hypsometric", BasemapStyle::Hypsometric),
                ("Oilslick", BasemapStyle::Oilslick),
                ("Hillshade", BasemapStyle::Hillshade),
            ] {
                let enabled = style == BasemapStyle::Natural || terrain_available;
                let button = egui::Button::selectable(self.style == style, label);
                if ui
                    .add_enabled(enabled, button)
                    .on_disabled_hover_text(
                        "Load elevation data in Offline Assets to enable terrain shading.",
                    )
                    .clicked()
                {
                    self.style = style;
                }
            }
            ui.separator();
            ui.checkbox(&mut self.show_country_borders, "Countries")
                .on_hover_text("Show country borders and coastline outlines.");
            ui.checkbox(&mut self.show_state_borders, "States");
            ui.checkbox(&mut self.labels.cities, "City names");
            ui.checkbox(&mut self.labels.detailed, "Detailed names")
                .on_hover_text("Show neighborhood, road, rail, and ferry names at close zoom. Transportation names follow the Roads layer.");
            ui.checkbox(&mut self.show_rivers, "Rivers");
            ui.checkbox(&mut self.show_roads, "Roads");
            ui.add_enabled(
                terrain_available,
                egui::Checkbox::new(&mut self.show_contours, "Contours"),
            )
            .on_disabled_hover_text("Contours require GEBCO elevation or local HGT terrain.");
            self.earth_sky_options(ui);
            ui.separator();
            if ui
                .add(
                    clip_ui::buttons::ButtonRole::Secondary
                        .button("Globe", palette)
                        .compact(),
                )
                .clicked()
            {
                self.camera.zoom = 1.0;
                self.camera.orbit = Default::default();
            }
            if ui
                .add(
                    clip_ui::buttons::ButtonRole::Secondary
                        .button("Fit", palette)
                        .compact(),
                )
                .clicked()
            {
                self.fit_document();
            }
            self.shared_sky_controls(ui, palette);
        });
    }

    fn shared_sky_controls(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.separator();
        ui.checkbox(&mut self.sky.constellations, "Constellations")
            .on_hover_text("Show Stellarium’s Modern figures. The star catalog is available in Offline Assets.");
        if ui
            .add(
                clip_ui::buttons::ButtonRole::Secondary
                    .button("Offline Assets…", palette)
                    .compact(),
            )
            .clicked()
        {
            self.show_assets_panel = !self.show_assets_panel;
        }
    }

    fn earth_sky_options(&mut self, ui: &mut egui::Ui) {
        if self.sky.assets.is_some() {
            ui.separator();
            ui.checkbox(&mut self.sky.visible, "Sky and planets");
            ui.add_enabled(
                    self.sky.visible,
                    egui::Checkbox::new(&mut self.sky.readable, "Larger sky objects"),
                )
                .on_hover_text("Enlarge the Sun and Moon up to 16×, with larger planet markers. Uses physical sizes during eclipses.");
            ui.checkbox(&mut self.sky.lighting, "Day and night");
        }
    }

    fn world_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let Some(body) = self.world.as_ref().map(|world| world.body) else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            let orbit = if body.kind() == solar::Kind::Moon {
                body.parent
                    .and_then(solar::body)
                    .map(|parent| (parent.id, format!("Orbit around {}", parent.name)))
            } else {
                solar::satellites(body.id)
                    .find(|_| body.id != solar::SUN)
                    .map(|_| (body.id, "Moon orbits".to_string()))
            };
            if let Some((center, title)) = orbit
                && ui.button(title).clicked()
            {
                self.open_solar_system(center, ui.ctx());
                return;
            }
            ui.separator();
            ui.strong(body.name);
            if let Some(radius) = body.radius {
                ui.label(format!("{radius:.1} km mean radius"));
            }
            self.body_assets(ui);
            self.shared_sky_controls(ui, palette);
        });
    }

    fn body_assets(&mut self, ui: &mut egui::Ui) {
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let Some((dataset, credit, source, bytes)) = solar::assets::information(world.body) else {
            return;
        };
        let pack = DownloadPack::World(world.body.id);
        let local_error = world.error.clone();
        let size = if bytes < 1_000_000 {
            format!("{:.0} kB", bytes as f64 / 1_000.0)
        } else {
            format!("{:.1} MB", bytes as f64 / 1_000_000.0)
        };
        if self.downloader.active == Some(pack) {
            self.download_progress(ui);
        } else if world.pending() {
            ui.label("Checking offline assets…");
        } else if world.installed() {
            ui.hyperlink_to(credit, source).on_hover_text(dataset);
        } else if ui
            .add_enabled(
                self.downloader.active.is_none(),
                egui::Button::new(format!("Download {} · {size}", dataset.to_lowercase())),
            )
            .on_hover_text(credit)
            .clicked()
        {
            self.downloader.start(pack, &self.config);
        }
        if let Some(error) = local_error.as_ref().or(self.downloader.last_error.as_ref()) {
            ui.label(error);
        }
    }

    fn download_progress(&mut self, ui: &mut egui::Ui) {
        let progress = self.downloader.progress_snapshot();
        ui.add(
            egui::ProgressBar::new(progress.fraction)
                .text(progress.message)
                .desired_width(230.0),
        );
        if ui.button("Cancel").clicked() {
            self.downloader.cancel();
        }
    }

    fn lunar_download(&mut self, ui: &mut egui::Ui) {
        let pack = DownloadPack::MoonRelief;
        if self.downloader.active == Some(pack) {
            self.download_progress(ui);
        } else if self.tiles.has_lunar_assets() {
            ui.hyperlink_to(
                "NASA LOLA elevation",
                "https://pds-geosciences.wustl.edu/missions/lro/lola.htm",
            )
            .on_hover_text("Lunar Reconnaissance Orbiter · ~118 m source elevation");
        } else if self.tiles.inspecting_lunar_assets() {
            ui.label("Checking Moon elevation…");
        } else if ui
            .add_enabled(
                self.downloader.active.is_none(),
                egui::Button::new(format!(
                    "Download Moon elevation · {:.2} GB",
                    lunar_terrain::BYTES as f64 / 1_000_000_000.0
                )),
            )
            .clicked()
        {
            self.downloader.start(pack, &self.config);
        }
        if self.downloader.active.is_none()
            && let Some(error) = &self.downloader.last_error
        {
            ui.label(error);
        }
    }

    fn open_solar_system(&mut self, center: solar::Id, ctx: &egui::Context) {
        let mut system = solar::System::new(center);
        if let Some(world) = self.world.take()
            && world.body.parent == Some(center)
            && let Some(ephemeris) = world.ephemeris()
        {
            system.show_ephemeris(world.body.id, ephemeris);
        }
        self.solar_system = Some(system);
        self.moon_camera = None;
        self.sight = None;
        self.found_place = None;
        self.tiles.suspend();
        ctx.request_repaint();
    }

    fn arrive_at_world(&mut self, selection: solar::Selection, ctx: &egui::Context) {
        let solar::Selection::Focus { id, camera } = selection else {
            self.open_solar_system(solar::SUN, ctx);
            return;
        };
        self.select_solar_target(id, ctx);
        if let Some(camera) = camera {
            match id {
                solar::EARTH => self.camera = camera,
                solar::MOON => self.moon_camera = Some(camera),
                _ => {
                    if let Some(world) = &mut self.world {
                        world.camera = camera;
                    }
                }
            }
        }
    }

    fn select_solar_target(&mut self, id: solar::Id, ctx: &egui::Context) {
        self.solar_system = None;
        self.sight = None;
        self.found_place = None;
        self.moon_camera = (id == solar::MOON).then(map_camera::lunar_camera);
        self.world = solar::body(id)
            .filter(|body| body.id != solar::EARTH)
            .map(solar::world::World::new);
        self.tiles.suspend();
        ctx.request_repaint();
    }

    fn solar_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette, center: solar::Id) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Back to map").clicked() {
                self.select_solar_target(solar::EARTH, ui.ctx());
            }
            if let Some(body) = solar::body(center).filter(|body| body.id != solar::SUN) {
                ui.separator();
                ui.label(format!("{} moon orbits", body.name));
                egui::ComboBox::from_id_salt("solar-system-moons")
                    .selected_text("Moons")
                    .show_ui(ui, |ui| {
                        for moon in solar::satellites(center) {
                            if ui.button(moon.name).clicked() {
                                self.select_solar_target(moon.id, ui.ctx());
                                ui.close();
                            }
                        }
                    });
            }
            self.shared_sky_controls(ui, palette);
        });
    }

    fn solar_offline_data(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Solar System data…", |ui| {
            ui.label(
                "Surface maps, orbital samples, Moon elevation, stars, and Earth night lighting.",
            );
            let total_gb = solar_download_bytes() as f64 / 1_000_000_000.0;
            ui.label(format!(
                "Up to {total_gb:.2} GB. Installed files are reused."
            ));
            if ui
                .add_enabled(
                    self.downloader.active.is_none(),
                    egui::Button::new("Get all Solar System data"),
                )
                .clicked()
            {
                self.downloader
                    .start(DownloadPack::SolarSystem, &self.config);
                ui.close();
            }
            ui.collapsing("Surface map sources", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (body, credit, source) in solar::assets::map_sources() {
                            ui.hyperlink_to(format!("{body} · {credit}"), source);
                        }
                        ui.hyperlink_to(
                            "NASA LOLA elevation",
                            "https://pds-geosciences.wustl.edu/missions/lro/lola.htm",
                        );
                        ui.hyperlink_to("JPL Horizons orbital data", solar::assets::HORIZONS);
                    });
            });
            if let Some(errors) = &self.downloader.solar_errors {
                if errors.is_empty() {
                    ui.label("All Solar System data is installed.");
                } else {
                    ui.collapsing(format!("Download issues ({})", errors.len()), |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .show(ui, |ui| {
                                for error in errors {
                                    ui.label(error);
                                }
                            });
                    });
                }
            }
        });
    }

    fn download_prompt_banner(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        colors: WorkbenchColors,
        earth_view: bool,
    ) {
        if self.assets_job.is_some() {
            ui.label("Loading offline map assets…");
            return;
        }
        let needs_prompt = earth_view
            && self.config.geography.prompt_on_open
            && !self.downloader.status.has_quick_start()
            && !self.prompt_dismissed;
        if !needs_prompt
            && !self.show_assets_panel
            && self.downloader.active.is_none()
            && self.downloader.last_error.is_none()
        {
            return;
        }
        egui::Frame::NONE
            .fill(colors.recessed)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(pack) = self.downloader.active {
                        let prog = self.downloader.progress_snapshot();
                        ui.strong(pack.label());
                        ui.add(
                            egui::ProgressBar::new(prog.fraction)
                                .desired_width(180.0)
                                .text(format!("Overall {:.1}%", 100.0 * prog.fraction)),
                        );
                        ui.label(prog.message);
                        if ui
                            .add(
                                clip_ui::buttons::ButtonRole::Secondary
                                    .button("Cancel", palette)
                                    .compact(),
                            )
                            .clicked()
                        {
                            self.downloader.cancel();
                        }
                        return;
                    }
                    if needs_prompt {
                        ui.strong("Download offline globe & map basemap?");
                        ui.label(format!(
                            "Assets folder: {} (configurable in config.toml [geography])",
                            self.downloader.assets_dir.display()
                        ));
                        if ui
                            .add(
                                clip_ui::buttons::ButtonRole::Primary
                                    .button("Download Quick Start (NE2 + Cities ≥ 250k)", palette)
                                    .compact(),
                            )
                            .clicked()
                        {
                            self.downloader.start(DownloadPack::QuickStart, &self.config);
                        }
                        if ui
                            .add(
                                clip_ui::buttons::ButtonRole::Secondary
                                    .button("Get all base maps", palette)
                                    .compact(),
                            )
                            .clicked()
                        {
                            self.downloader.start(DownloadPack::All, &self.config);
                        }
                        if ui
                            .add(
                                clip_ui::buttons::ButtonRole::Secondary
                                    .button("Not now", palette)
                                    .compact(),
                            )
                            .clicked()
                        {
                            self.prompt_dismissed = true;
                        }
                    }
                    if self.show_assets_panel {
                        ui.separator();
                        let st = self.downloader.status.clone();
                        let used_gb =
                            self.downloader.disk_usage_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                        let budget_gb =
                            self.downloader.all_limit_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                        ui.label(format!(
                            "Installed: {used_gb:.2} GiB · Get all limit: {budget_gb:.1} GiB per run · NE2: {} · Cities: {} · Borders: {} · Roads/Lakes: {} · GRWL: {} · Masks: {} · GSHHG: {} · GEBCO: {} · LandCover: {} · Local HGT: {}",
                            yes_no(st.has_ne2),
                            yes_no(st.has_cities),
                            yes_no(st.has_borders),
                            yes_no(st.has_roads_lakes),
                            yes_no(st.has_grwl),
                            yes_no(st.has_grwl_masks),
                            yes_no(st.has_gshhg),
                            if st.has_full_gebco { "15″" } else if st.has_gebco { "overview only" } else { "no" },
                            yes_no(st.has_land_cover),
                            st.local_hgt_count
                        ));
                        if (!st.has_ne2
                            || !st.has_cities
                            || !st.has_borders
                            || !st.has_roads_lakes
                            || !st.has_grwl
                            || !st.has_grwl_masks
                            || !st.has_gshhg
                            || !st.has_full_gebco
                            || !st.has_land_cover
                            || !st.has_hydrorivers)
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Primary
                                        .button("Get all base maps", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader.start(DownloadPack::All, &self.config);
                        }
                        if !st.has_ne2
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get NE2 + Cities", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader.start(DownloadPack::QuickStart, &self.config);
                        }
                        if !st.has_borders
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get Borders (~7 MB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader.start(DownloadPack::Borders, &self.config);
                        }
                        if !st.has_roads_lakes
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get Roads & Lakes (~55 MB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::RoadsAndLakes, &self.config);
                        }
                        if !st.has_grwl
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get GRWL Widths (43 MB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::GrwlSummary, &self.config);
                        }
                        if !st.has_grwl_masks
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get GRWL 30m Masks (394 MB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::GrwlMasks, &self.config);
                        }
                        if !st.has_gshhg
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get GSHHG Water (~113 MB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::WaterOutlines, &self.config);
                        }
                        if !st.has_full_gebco
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get GEBCO 15″ Terrain (~3.26 GB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::TerrainGebco, &self.config);
                        }
                        if !st.has_land_cover
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get Land Cover (~337 MB)", palette)
                                        .compact(),
                                )
                                .on_hover_text("Importing land cover requires GDAL 3.7+ (gdalwarp) with NetCDF support. Other map layers download independently.")
                                .clicked()
                        {
                            self.downloader.start(DownloadPack::LandCover, &self.config);
                        }
                        if !st.has_hydrorivers
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get HydroRIVERS (~544 MB)", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::HydroRivers, &self.config);
                        }
                        ui.separator();
                        ui.strong("Additional packs");
                        self.optional_assets(ui, palette, colors);
                    }
                    if let Some(err) = &self.downloader.last_error {
                        ui.colored_label(colors.warning, err);
                    }
                });
                if earth_view && self.show_assets_panel && self.downloader.active.is_none() {
                    self.terrain_download.ui(ui, self.camera, self.assets.as_ref(), &self.config, &mut self.downloader);
                }
            });
    }

    fn optional_assets(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        colors: WorkbenchColors,
    ) {
        self.solar_offline_data(ui);
        self.transport_assets(ui, palette);
        let sky_download = match &self.sky.assets {
            None => Some(format!(
                "Get Sky & Earth Lighting (~{:.0} MB)",
                sky::PACK_BYTES as f64 / 1e6
            )),
            Some(assets) if assets.night.is_none() => Some("Complete Sky & Earth Lighting".into()),
            Some(_) => None,
        };
        if let Some(label) = sky_download
            && ui
                .add_enabled(
                    self.sky.job.is_none(),
                    clip_ui::buttons::ButtonRole::Secondary
                        .button(label, palette)
                        .compact(),
                )
                .clicked()
        {
            self.downloader.start(DownloadPack::Sky, &self.config);
        }
        ui.end_row();
        self.star_catalog_assets(ui, palette, colors);
        ui.end_row();
        ui.hyperlink_to(
            "JPL DE440",
            "https://naif.jpl.nasa.gov/pub/naif/generic_kernels/spk/planets/de440.bsp",
        );
        ui.hyperlink_to("NASA star map", "https://svs.gsfc.nasa.gov/4851/");
        ui.hyperlink_to("NASA Moon albedo", "https://svs.gsfc.nasa.gov/4720/");
        ui.hyperlink_to(
            "NASA ΔT uncertainty",
            "https://eclipse.gsfc.nasa.gov/SEcat5/uncertainty.html",
        );
        if let Some(error) = &self.sky.error {
            ui.colored_label(colors.warning, format!("Sky assets: {error}"));
        }
    }

    fn star_catalog_assets(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        colors: WorkbenchColors,
    ) {
        ui.hyperlink_to(
            "Stellarium team · Modern figures",
            sky::catalog::STELLARIUM_CREDIT,
        );
        ui.hyperlink_to(
            "David Nash / Astronexus · HYG v4.1",
            sky::catalog::HYG_CREDIT,
        );
        ui.hyperlink_to(
            "CC BY-SA 4.0",
            "https://creativecommons.org/licenses/by-sa/4.0/",
        );
        let Some(assets) = &self.sky.assets else {
            return;
        };
        if assets.star_catalog().is_some() {
            return;
        }
        if let Err(error) = &assets.catalog {
            ui.colored_label(colors.warning, format!("Star catalog: {error}"));
        }
        if ui
            .add_enabled(
                self.sky.job.is_none(),
                clip_ui::buttons::ButtonRole::Secondary
                    .button(
                        format!(
                            "Get Constellations & Star Catalog ({:.2} MB)",
                            sky::catalog::DOWNLOAD_BYTES as f64 / 1e6
                        ),
                        palette,
                    )
                    .compact(),
            )
            .clicked()
        {
            self.downloader
                .start(DownloadPack::StarCatalog, &self.config);
        }
    }

    fn transport_assets(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let status = &self.downloader.transport;
        let manifest = detail::manifest();
        if status.source.is_some() {
            ui.label(format!(
                "Detailed Transportation · {:.2} GiB installed · {}",
                status.bytes as f64 / (1u64 << 30) as f64,
                manifest.release
            ));
            ui.hyperlink_to(
                &manifest.attribution,
                "https://docs.overturemaps.org/attribution/",
            );
            return;
        }
        let gib = (1u64 << 30) as f64;
        ui.label(format!(
            "Global Overture {} · estimated {:.0} GiB installed, {:.0} GiB peak during import",
            manifest.release,
            detail::ESTIMATED_FINAL as f64 / gib,
            detail::ESTIMATED_PEAK as f64 / gib,
        ));
        let label = if status.bytes == 0 {
            "Get Detailed Transportation"
        } else {
            "Resume Detailed Transportation"
        };
        if ui
            .add_enabled(
                self.assets_job.is_none(),
                clip_ui::buttons::ButtonRole::Secondary
                    .button(
                        format!(
                            "{label} ({:.2} GB total)",
                            detail::DOWNLOAD_BYTES as f64 / 1e9
                        ),
                        palette,
                    )
                    .compact(),
            )
            .clicked()
        {
            self.downloader
                .start(DownloadPack::DetailedTransportation, &self.config);
        }
        ui.hyperlink_to(
            &manifest.attribution,
            "https://docs.overturemaps.org/attribution/",
        );
    }

    fn sample(&self, hit: Hit) -> Option<&Sample> {
        self.document
            .features
            .get(hit.feature)?
            .paths
            .get(hit.path)?
            .get(hit.point)
    }

    fn details(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if !self.document.title.is_empty() {
            ui.heading(&self.document.title);
        }
        ui.label(format!(
            "{} shapes · {} positions",
            self.document.features.len(),
            self.document.points
        ));
        let mut zoom_f32 = self.moon_camera.as_ref().unwrap_or(&self.camera).zoom as f32;
        ui.horizontal(|ui| {
            if ui
                .add(
                    clip_ui::buttons::ButtonRole::Secondary
                        .button("Fit map", palette)
                        .compact(),
                )
                .clicked()
            {
                self.fit_document();
                zoom_f32 = self.camera.zoom as f32;
            }
            if ui
                .add(
                    egui::Slider::new(&mut zoom_f32, MIN_MAP_ZOOM as f32..=MAX_MAP_ZOOM as f32)
                        .logarithmic(true)
                        .show_value(false)
                        .text("Zoom"),
                )
                .changed()
            {
                self.moon_camera.as_mut().unwrap_or(&mut self.camera).zoom = zoom_f32 as f64;
            }
        });
        let height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt("geography-features")
            .max_height((ui.available_height() * 0.3).max(height))
            .min_scrolled_height(0.0)
            .show_rows(ui, height, self.document.features.len(), |ui, rows| {
                for index in rows {
                    if crate::ui_ext::selectable_text_row(
                        ui,
                        &self.document.features[index].name,
                        index == self.selected,
                        height,
                    )
                    .clicked()
                        && index != self.selected
                    {
                        chosen = Some(index);
                    }
                }
            });
        if let Some(index) = chosen {
            self.select(index);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("geography-details")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                let feature = &self.document.features[self.selected];
                ui.strong(&feature.name);
                ui.label(format!(
                    "{} positions · {} segments",
                    feature.points,
                    feature.paths.len()
                ));
                if matches!(feature.kind, Kind::Line | Kind::Polygon) {
                    ui.label(format!(
                        "Spherical length {}",
                        self.config.geography.units.distance(feature.distance_m)
                    ));
                }
                if let Some([low, high]) = feature.elevation {
                    ui.label(format!(
                        "Supplied elevation {}",
                        self.config.geography.units.elevation_range(low, high)
                    ));
                }
                if let Some(sample) = self.current.and_then(|hit| self.sample(hit)) {
                    ui.separator();
                    ui.strong("Selected position");
                    ui.label(format!("Latitude {:.6}", sample.point.lat));
                    ui.label(format!("Longitude {:.6}", sample.point.lon));
                    if let Some(value) = sample.point.elevation {
                        ui.label(format!(
                            "Elevation {}",
                            self.config.geography.units.elevation(value)
                        ));
                    }
                    radiation::show_reading(ui, &sample.point);
                    if !sample.point.time.is_empty() {
                        ui.add(egui::Label::new(&sample.point.time).wrap());
                    }
                    ui.label(format!(
                        "Along feature {}",
                        self.config.geography.units.distance(sample.distance_m)
                    ));
                }
                for (name, value) in &feature.properties {
                    ui.strong(name);
                    ui.add(egui::Label::new(value).wrap());
                }
                for note in &self.document.notes {
                    ui.add(egui::Label::new(note).wrap());
                }
            });
    }
}

fn yes_no(v: bool) -> &'static str {
    if v { "ok" } else { "missing" }
}

fn paint_cities(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    camera: &GlobeCamera,
    assets: &BasemapAssets,
    offset: f32,
    (label_limit, detail_tiles): (
        usize,
        &[(
            little_clip_core::geography::basemap::tiles::TileId,
            Arc<detail::Tile>,
        )],
    ),
    draped: Option<map_camera::Draped<'_>>,
) {
    // Natural Earth Populated Places: >= 250k at globe zoom, revealing all towns on close zoom, biggest first
    let min_pop = if camera.zoom < 3.0 {
        250_000
    } else if camera.zoom < 8.0 {
        50_000
    } else {
        0
    };
    let mut occupied: Vec<egui::Rect> = Vec::new();
    for city in &assets.cities {
        if city.pop_max < min_pop {
            break;
        }
        let Some([px, py]) = (match draped {
            Some(pose) => pose.project(city.lat, city.lon),
            None => camera.project(city.lat, city.lon, rect.width(), rect.height()),
        }) else {
            continue;
        };
        let pos = egui::pos2(rect.left() + px + offset, rect.top() + py);
        if !rect.contains(pos) {
            continue;
        }
        let cell = little_clip_core::geography::basemap::tiles::TileId::at(
            detail::LEVEL,
            city.lat,
            city.lon,
        );
        if detail_tiles
            .iter()
            .any(|(id, tile)| *id == cell && tile.has_settlements())
        {
            continue;
        }
        let font_size = if city.pop_max >= 1_000_000 {
            13.5
        } else {
            12.0
        };
        let galley = ui.fonts_mut(|f| {
            f.layout_no_wrap(
                city.name.clone(),
                egui::FontId::proportional(font_size),
                egui::Color32::from_rgb(248, 250, 252),
            )
        });
        let label_pos = pos + egui::vec2(6.0, -galley.size().y * 0.5);
        let pill_rect =
            egui::Rect::from_min_size(label_pos, galley.size()).expand2(egui::vec2(4.0, 1.5));
        let box_rect = pill_rect.expand(4.0);
        if occupied.iter().any(|r| r.intersects(box_rect)) {
            continue;
        }
        occupied.push(box_rect);
        painter.circle_filled(pos, 3.0, egui::Color32::from_rgb(255, 214, 96));
        painter.circle_stroke(
            pos,
            3.0,
            egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(180)),
        );
        painter.rect_filled(pill_rect, 3.0, egui::Color32::from_black_alpha(150));
        painter.galley(label_pos, galley, egui::Color32::from_rgb(248, 250, 252));
        if occupied.len() >= label_limit {
            break;
        }
    }
}

fn paint_scale_and_contour_overlay(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    camera: &GlobeCamera,
    contour_interval_m: Option<f32>,
    local_elev_range: [f32; 2],
    units: Units,
    moon: bool,
) -> egui::Rect {
    use little_clip_core::geography::basemap::perspective::Perspective;

    let size = [rect.width(), rect.height()];
    let radius_m = if moon {
        map_camera::MOON_RADIUS_M
    } else {
        map_camera::EARTH_RADIUS_M
    };
    let meters_per_px = (moon || camera.orbit.active())
        .then(|| Perspective::new(camera, size, radius_m).meters_per_pixel())
        .flatten()
        .filter(|scale| *scale > 0.0)
        .unwrap_or_else(|| radius_m / camera.scale_px(size[0], size[1]).max(1.0));
    let target_m = meters_per_px * 110.0;
    let (nice_m, scale_label) = units.scale_bar(target_m);
    let bar_w = (nice_m / meters_per_px) as f32;

    let mut text = scale_label;
    if let Some(interval) = contour_interval_m {
        text.push_str(&format!(
            "  ·  Contour: {} (index {})  ·  Elev: {}",
            units.elevation(interval as f64),
            units.elevation(interval as f64 * 5.0),
            units.elevation_range(local_elev_range[0] as f64, local_elev_range[1] as f64)
        ));
    }
    let galley = map_badge_text(ui, text);
    let box_w = (bar_w + galley.size().x + 26.0).min(rect.width() - 20.0);
    let box_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 32.0),
        egui::vec2(box_w, 22.0),
    );
    painter.rect_filled(box_rect, 4.0, egui::Color32::from_black_alpha(165));
    let bar_y = box_rect.center().y + 2.0;
    let bar_x0 = box_rect.left() + 8.0;
    let bar_x1 = bar_x0 + bar_w;
    let stroke = egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(244, 246, 250));
    painter.line_segment(
        [egui::pos2(bar_x0, bar_y), egui::pos2(bar_x1, bar_y)],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(bar_x0, bar_y - 4.0),
            egui::pos2(bar_x0, bar_y + 1.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(bar_x1, bar_y - 4.0),
            egui::pos2(bar_x1, bar_y + 1.0),
        ],
        stroke,
    );
    painter.galley(
        egui::pos2(bar_x1 + 8.0, box_rect.center().y - galley.size().y * 0.5),
        galley,
        egui::Color32::from_rgb(244, 246, 250),
    );
    box_rect
}

fn map_badge_text(ui: &egui::Ui, text: String) -> Arc<egui::Galley> {
    ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(
            text,
            egui::FontId::proportional(11.5),
            egui::Color32::from_rgb(244, 246, 250),
        )
    })
}

fn paint_scene_clock(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    scale: egui::Rect,
    time: DateTime<Utc>,
    scene: Option<&sky::Scene>,
) {
    let galley = map_badge_text(ui, time.format("%Y-%m-%d %H:%M:%S").to_string());
    let size = egui::vec2(galley.size().x + 16.0, 22.0);
    let mut badge =
        egui::Align2::RIGHT_BOTTOM.anchor_size(rect.right_bottom() - egui::vec2(10.0, 10.0), size);
    if badge.left() < scale.right() + 6.0 {
        badge = badge.translate(egui::vec2(0.0, -26.0));
    }
    if !rect.contains_rect(badge) {
        return;
    }
    painter.rect_filled(badge, 4.0, egui::Color32::from_black_alpha(165));
    painter.galley(
        badge.center() - galley.size() * 0.5,
        galley,
        egui::Color32::from_rgb(244, 246, 250),
    );
    let description = match scene.map(|s| s.rotation_source) {
        Some(sky::RotationSource::Observed) => "UTC · IERS observed Earth rotation",
        Some(sky::RotationSource::Predicted) => "UTC · IERS predicted Earth rotation",
        Some(sky::RotationSource::Modeled) => {
            "UTC · Modeled Earth rotation · geographic shadow paths are approximate"
        }
        None => "UTC · Outside the DE440 ephemeris coverage",
    };
    ui.interact(
        badge,
        ui.scope_id().with("scene-clock"),
        egui::Sense::hover(),
    )
    .on_hover_text(description);
}

fn paint_oilslick_legend(ui: &mut egui::Ui, colors: WorkbenchColors, units: Units) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);
    let bar = egui::Rect::from_min_max(
        rect.min + egui::vec2(4.0, 4.0),
        egui::pos2(
            (rect.right() - 190.0).max(rect.left() + 80.0),
            rect.bottom() - 4.0,
        ),
    );
    let steps = 64usize;
    let step_w = bar.width() / steps as f32;
    for i in 0..steps {
        let elev = if i < 4 {
            0.0
        } else {
            ((i - 4) as f32 / (steps - 4) as f32) * 3500.0
        };
        let [r, g, b] = oilslick_color(elev, 0.9);
        let seg = egui::Rect::from_min_size(
            egui::pos2(bar.left() + i as f32 * step_w, bar.top()),
            egui::vec2(step_w + 0.5, bar.height()),
        );
        painter.rect_filled(seg, 0.0, egui::Color32::from_rgb(r, g, b));
    }
    painter.text(
        egui::pos2(bar.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!(
            "Oilslick: {} cycle + sea-level break",
            units.elevation(500.0)
        ),
        egui::TextStyle::Small.resolve(ui.style()),
        colors.text_secondary,
    );
}

fn paint_strip_map(
    ui: &egui::Ui,
    rect: egui::Rect,
    preview: &Preview,
    maps: &mut Strips,
    colors: WorkbenchColors,
) -> Option<Hit> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, colors.recessed);
    let bounds = preview
        .document
        .geographic_bounds
        .filter(|_| rect.is_positive())?;
    let camera = strips::camera(rect, bounds);
    maps.paint(ui, rect, camera);
    let pointer = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|_| super::ui::preview_hovered(ui, rect));
    let projection = overlay::Projection {
        camera: &camera,
        rect,
        offset: 0.0,
        draped: None,
    };
    let hit = preview
        .overlay
        .paint_copy(&painter, projection, None, colors, pointer)?
        .sample;
    let sample = &preview.document.features[hit.feature].paths[hit.path][hit.point];
    show_position(ui, &preview.document.features[hit.feature].name, sample);
    Some(hit)
}

fn live_scene_time(ctx: &egui::Context) -> DateTime<Utc> {
    let now = Utc::now();
    ctx.request_repaint_after(std::time::Duration::from_nanos(
        1_000_000_000 - u64::from(now.timestamp_subsec_nanos()),
    ));
    DateTime::from_timestamp(now.timestamp(), 0).unwrap_or(now)
}

fn show_sky_object(ui: &egui::Ui, rect: egui::Rect, frame: &gpu::Frame) {
    let target = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|_| super::ui::preview_hovered(ui, rect))
        .and_then(|pointer| gpu::sky_object_at(frame, rect, pointer));
    if let Some(target) = target {
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            egui::Id::unique("sky-object-name"),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| match target {
            gpu::SkyObject::Body(name) => {
                ui.label(name);
                match name {
                    "Earth" => {
                        ui.label("Click to return");
                    }
                    "Moon" => {
                        ui.label("Click to explore");
                    }
                    "Sun" => {
                        ui.label("Click to explore the Solar System");
                    }
                    _ => {
                        ui.label("Click to explore");
                    }
                }
            }
            gpu::SkyObject::Star(star) => gpu::star_name(ui, star),
        });
    }
}

fn show_position(ui: &egui::Ui, name: &str, sample: &Sample) {
    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        egui::Id::unique("geographic-position"),
        egui::PopupAnchor::Pointer,
    )
    .show(|ui| {
        ui.label(name);
        ui.label(format!("{:.6}, {:.6}", sample.point.lat, sample.point.lon));
        radiation::show_reading(ui, &sample.point);
    });
}

fn paint_profile(
    ui: &egui::Ui,
    rect: egui::Rect,
    feature: &Feature,
    feature_index: usize,
    current: Option<Hit>,
    colors: WorkbenchColors,
    units: Units,
    interpolate_gpx: bool,
) -> Option<(Hit, Option<DateTime<Utc>>)> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);
    let Some([low, high]) = feature.elevation else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No supplied elevations",
            egui::TextStyle::Body.resolve(ui.style()),
            colors.text_secondary,
        );
        return None;
    };
    painter.text(
        rect.left_top() + egui::vec2(5.0, 3.0),
        egui::Align2::LEFT_TOP,
        format!(
            "Supplied elevation · {} · {}",
            units.elevation_range(low, high),
            units.distance(feature.distance_m)
        ),
        egui::TextStyle::Small.resolve(ui.style()),
        colors.text_secondary,
    );
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(8.0, 28.0),
        rect.max - egui::vec2(8.0, 8.0),
    );
    let pointer = super::ui::preview_hovered(ui, plot)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut nearest = f32::INFINITY;
    let mut hit = None;
    for (path_index, path) in feature.paths.iter().enumerate() {
        let mut previous = None;
        for (index, sample) in path.iter().enumerate() {
            let Some(value) = sample.point.elevation else {
                previous = None;
                continue;
            };
            if sample.profile_break {
                previous = None;
            }
            let scale = low.abs().max(high.abs()).max(1.0);
            let fraction =
                (value / scale - low / scale) / (high / scale - low / scale).max(1.0 / scale);
            let position = egui::pos2(
                plot.left()
                    + plot.width() * (sample.distance_m / feature.distance_m.max(1.0)) as f32,
                plot.bottom() - plot.height() * fraction as f32,
            );
            if let Some(previous) = previous {
                painter.line_segment(
                    [previous, position],
                    egui::Stroke::new(2.0_f32, colors.accent),
                );
            }
            painter.circle_filled(position, 2.0, colors.accent);
            previous = Some(position);
            let candidate = Hit {
                feature: feature_index,
                path: path_index,
                point: index,
            };
            if current == Some(candidate) {
                painter.circle_stroke(position, 5.0, egui::Stroke::new(2.0_f32, colors.focus));
            }
            if let Some(pointer) = pointer {
                let distance = (position.x - pointer.x).abs();
                if distance < nearest {
                    nearest = distance;
                    hit = Some(candidate);
                }
            }
        }
    }
    let hit = hit?;
    let path = &feature.paths[hit.path];
    let time = match pointer {
        Some(pointer) if interpolate_gpx && nearest > overlay::SNAP_RADIUS => {
            let position = f64::from((pointer.x - plot.left()) / plot.width());
            profile_segment_time(path, hit.point, position * feature.distance_m.max(1.0))
        }
        _ => path[hit.point].point.instant(),
    };
    Some((hit, time))
}

fn profile_segment_time(path: &[Sample], nearest: usize, position: f64) -> Option<DateTime<Utc>> {
    let sample = &path[nearest];
    let (from, to) = if position < sample.distance_m {
        (path.get(nearest.checked_sub(1)?)?, sample)
    } else {
        (sample, path.get(nearest + 1)?)
    };
    let span = to.distance_m - from.distance_m;
    if to.profile_break
        || from.point.elevation.is_none()
        || to.point.elevation.is_none()
        || span <= 0.0
    {
        return None;
    }
    let t = (position - from.distance_m) / span;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some(interpolate_instant(
        from.point.instant()?,
        to.point.instant()?,
        t,
    ))
}

pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    maps: &mut Strips,
    colors: WorkbenchColors,
) -> Option<usize> {
    let rect = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Geographic paths", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(rect, 3.0, colors.recessed);
    let preview = ready.and_then(|r| r.data.geography.as_ref())?;
    let map = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.left() + rect.width().min(240.0), rect.bottom()),
    );
    let mut hovered = paint_strip_map(ui, map, preview, maps, colors).map(|hit| hit.feature);
    let mut x = map.right() + 4.0;
    for (index, feature) in preview.document.features.iter().enumerate().take(4) {
        let width = 180.0;
        if x + width > rect.right() {
            break;
        }
        let card = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 4.0),
            egui::vec2(width, rect.height() - 8.0),
        );
        let painter = ui.painter().with_clip_rect(card);
        painter.rect_filled(card, 3.0, colors.accent.gamma_multiply(0.18));
        painter.text(
            card.left_top() + egui::vec2(5.0, 3.0),
            egui::Align2::LEFT_TOP,
            &feature.name,
            egui::TextStyle::Body.resolve(ui.style()),
            colors.text_primary,
        );
        painter.text(
            card.left_top() + egui::vec2(5.0, 26.0),
            egui::Align2::LEFT_TOP,
            format!("{} positions", feature.points),
            egui::TextStyle::Small.resolve(ui.style()),
            colors.text_secondary,
        );
        if super::ui::preview_hovered(ui, card) {
            hovered = Some(index);
        }
        x += width + 4.0;
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_clock_uses_dated_hover_and_restores_source_order_track_default() {
        let gpx = r#"<gpx xmlns="http://www.topografix.com/GPX/1/1">
            <metadata><time>2019-01-01T08:00:00+02:00</time></metadata>
            <trk><trkseg>
            <trkpt lat="1" lon="1"><time>2024-04-08T13:17:00-05:00</time></trkpt>
            <trkpt lat="1" lon="2"><time>2023-10-14T18:00:00Z</time></trkpt>
            <trkpt lat="1" lon="3"><time>2022-11-08T10:59:00</time></trkpt>
            </trkseg></trk></gpx>"#;
        let mut document = Document::gpx(gpx.as_bytes()).unwrap();
        let times = &document.features[0].paths[0];
        let hovered = times[0].point.instant().unwrap();
        let last = times[1].point.instant().unwrap();
        assert_eq!(hovered.to_rfc3339(), "2024-04-08T18:17:00+00:00");
        assert_eq!(
            recorded_scene_time(&document, 0, times[0].point.instant()),
            Some(hovered)
        );
        assert_eq!(
            recorded_scene_time(&document, 0, times[2].point.instant()),
            Some(last)
        );
        assert_eq!(recorded_scene_time(&document, 0, None), Some(last));
        document.features[0].last_instant = None;
        assert_eq!(
            recorded_scene_time(&document, 0, None)
                .unwrap()
                .to_rfc3339(),
            "2019-01-01T06:00:00+00:00"
        );
        document.capture_time = None;
        assert_eq!(recorded_scene_time(&document, 0, None), None);
        assert_eq!(
            recorded_scene_time(
                &document,
                0,
                document.features[0].paths[0][2].point.instant()
            ),
            None
        );
    }

    #[test]
    fn scene_transition_eases_through_real_instants_in_both_directions() {
        let from = "2024-04-08T16:40:00Z".parse::<DateTime<Utc>>().unwrap();
        let to = from + TimeDelta::hours(1);
        for (from, to) in [(from, to), (to, from)] {
            let transition = SceneTransition {
                from,
                to,
                started_at: 10.0,
            };
            assert_eq!(transition.at(10.0), from);
            assert_eq!(transition.at(10.025), from + (to - from) * 5 / 32);
            assert_eq!(transition.at(10.050), from + (to - from) / 2);
            assert_eq!(transition.at(10.101), to);
        }
    }

    #[test]
    fn elevation_scrub_interpolates_only_inside_a_contiguous_dated_segment() {
        let gpx = br#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><trk><trkseg>
            <trkpt lat="0" lon="0"><ele>10</ele><time>2024-04-08T16:00:00Z</time></trkpt>
            <trkpt lat="0" lon="1"><ele>30</ele><time>2024-04-08T18:00:00Z</time></trkpt>
            <trkpt lat="0" lon="2"><ele>20</ele></trkpt>
            </trkseg></trk></gpx>"#;
        let mut document = Document::gpx(gpx).unwrap();
        let path = &mut document.features[0].paths[0];
        let middle = path[0].distance_m.midpoint(path[1].distance_m);
        for nearest in [0, 1] {
            assert_eq!(
                profile_segment_time(path, nearest, middle)
                    .unwrap()
                    .to_rfc3339(),
                "2024-04-08T17:00:00+00:00"
            );
        }
        let undated = path[1].distance_m.midpoint(path[2].distance_m);
        assert!(profile_segment_time(path, 1, undated).is_none());
        path[1].profile_break = true;
        assert!(profile_segment_time(path, 1, middle).is_none());
    }

    #[test]
    fn radiacode_preview_and_both_projections_draw_dose_colors() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/survey.rctrk");
        let summary = load(&path, &AtomicBool::new(false)).geography.unwrap();
        assert_eq!(summary.document.features[0].dose_rate, Some([0.06, 0.17]));

        let mut viewer = Viewer::load(&path).unwrap();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let ctx = egui::Context::default();
        let colors = WorkbenchColors::from_palette(&crate::ui_appearance::palette(
            little_clip_core::config::WorkbenchAppearance::Dark,
        ));
        let scale = radiation::DoseScale([0.06, 0.17]);
        for mode in [ProjectionMode::Map2D, ProjectionMode::Globe3D] {
            viewer.camera.mode = mode;
            let output = clip_ui::test_support::run_frame(
                &ctx,
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            viewer.overlay.paint_copy(
                                ui.painter(),
                                overlay::Projection {
                                    camera: &viewer.camera,
                                    rect,
                                    offset: 0.0,
                                    draped: None,
                                },
                                Some(0),
                                colors,
                                None,
                            );
                        })
                    });
                },
            );
            let outline_index = output
                .shapes
                .iter()
                .position(|shape| {
                    matches!(&shape.shape,
                        egui::Shape::Path(path) if path.stroke.color == egui::epaint::ColorMode::Solid(egui::Color32::BLACK)
                            && path.stroke.width > 4.5 && path.points.len() == 4
                    )
                })
                .expect("Thin black outline should trace the whole route");
            for rate in [0.06_f64.midpoint(0.08), 0.17_f64.midpoint(0.15)] {
                assert!(
                    output.shapes[outline_index + 1..]
                        .iter()
                        .any(|shape| matches!(&shape.shape,
                            egui::Shape::Path(path) if path.stroke.color == egui::epaint::ColorMode::Solid(scale.color(rate))
                        )),
                    "Missing dose color for {rate} in {mode:?}"
                );
            }
        }
    }

    #[test]
    fn sampled_folder_features_and_viewer_selection_keep_source_indices() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.gpx");
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.geography.unwrap();
        assert!(summary.document.samples <= 512);
        assert_eq!(summary.document.features[1].name, "Ferry route");
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select(1);
        assert_eq!(
            viewer.document.features[viewer.selected].name,
            "Ferry route"
        );
        assert_eq!(
            viewer
                .sample(viewer.current.unwrap())
                .unwrap()
                .point
                .elevation,
            Some(0.0)
        );
        assert!(read(&path, &AtomicBool::new(true)).is_err());
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("TRACK.GPX"))
                == Some(little_clip_core::file_format::Format::Geography))
        );
    }
    #[test]
    fn geojson_preview_and_null_geometry_selection_preserve_source_order() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.geojson");
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("MAP.GEOJSON"))
                == Some(little_clip_core::file_format::Format::Geography))
        );
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.geography.unwrap();
        assert_eq!(summary.document.features[0].kind, Kind::Polygon);
        assert_eq!(summary.document.features[0].paths.len(), 2);
        assert!(summary.document.samples <= 512);
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select(3);
        assert_eq!(
            viewer.document.features[viewer.selected].name,
            "Unplaced record"
        );
        assert!(viewer.current.is_none());
        viewer.select(1);
        assert_eq!(
            viewer
                .sample(viewer.current.unwrap())
                .unwrap()
                .point
                .elevation,
            Some(12.0)
        );
    }
}
