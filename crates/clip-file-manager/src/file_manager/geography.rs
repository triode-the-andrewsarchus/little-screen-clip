//! Offline 3D-globe / 2D-map geography viewer with Oilslick elevation shading and linked profile.
mod geometry;
pub(super) mod gpu;
mod tiles;

use super::{
    filesystem::Entry,
    geography_downloader::{AssetDownloader, DownloadPack},
    media::PreviewData,
    shelf::preview::Ready,
};
use crate::ui_ext::fit_cartesian_point as project;
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    ui_appearance::WorkbenchColors,
};
use little_clip_core::{
    config::AppConfig,
    geography::{
        Document, Feature, Kind, MAX_BYTES, Sample,
        basemap::{BasemapAssets, BasemapStyle, GlobeCamera, ProjectionMode, oilslick_color},
    },
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

fn read(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BYTES)?;
    if token.load(Ordering::Relaxed) {
        return Err("Geography opening cancelled".into());
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("geojson"))
    {
        Document::geojson(&bytes)
    } else {
        Document::gpx(&bytes)
    }
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
            data.geography = Some(Arc::new(document));
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
    selected: usize,
    current: Option<Hit>,
    camera: GlobeCamera,
    style: BasemapStyle,
    show_country_borders: bool,
    show_state_borders: bool,
    show_rivers: bool,
    show_roads: bool,
    show_contours: bool,
    contour_interval_m: Option<f32>,
    local_elev_range: [f32; 2],
    config: AppConfig,
    downloader: AssetDownloader,
    assets: Option<Arc<BasemapAssets>>,
    assets_rx: Option<mpsc::Receiver<Arc<BasemapAssets>>>,
    prompt_dismissed: bool,
    show_assets_panel: bool,
    tiles: tiles::Tiles,
    render_error: Option<String>,
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
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let loaded = BasemapAssets::load(&assets_dir, override_dir.as_deref());
            let _ = tx.send(Arc::new(loaded));
        });

        let mut viewer = Self {
            document,
            selected,
            current: None,
            camera: GlobeCamera::default(),
            style: BasemapStyle::Natural,
            show_country_borders: true,
            show_state_borders: true,
            show_rivers: true,
            show_roads: true,
            show_contours: true,
            contour_interval_m: None,
            local_elev_range: [0.0, 1000.0],
            config,
            downloader,
            assets: None,
            assets_rx: Some(rx),
            prompt_dismissed: false,
            show_assets_panel: false,
            tiles: tiles::Tiles::default(),
            render_error: None,
        };
        viewer.fit_document();
        viewer.select(selected);
        Ok(viewer)
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
        self.camera.fit_bounds(self.document.geographic_bounds);
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.assets_rx
            && let Ok(loaded) = rx.try_recv()
        {
            self.downloader.status = loaded.status();
            self.assets = Some(loaded);
            self.assets_rx = None;
            ctx.request_repaint();
        }
        if let Some(downloaded) = self.downloader.poll(ctx) {
            self.assets = Some(downloaded);
            ctx.request_repaint();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        self.poll_background(ui.ctx());
        StudioShell::new("Geography", Icon::Image, palette, close).render_header(ui, |_| {});

        let colors = WorkbenchColors::from_palette(palette);
        self.download_prompt_banner(ui, palette, colors);

        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.details(ui, palette);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            self.toolbar(ui, palette);

            let feature = &self.document.features[self.selected];
            let profile_visible =
                feature.kind != Kind::Point && feature.elevation.is_some() && feature.points > 1;
            let legend_h = if self.style == BasemapStyle::Oilslick {
                26.0
            } else {
                0.0
            };
            let map_height =
                (ui.available_height() - legend_h - if profile_visible { 190.0 } else { 26.0 })
                    .max(80.0);
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), map_height),
                egui::Sense::click_and_drag(),
            );

            // Handle drag-to-rotate/pan and scroll-to-zoom with smooth 3D -> 2D transition
            if response.dragged() {
                let delta = response.drag_delta();
                if delta.length_sq() > 0.0 {
                    self.camera
                        .pan_by(delta.x, delta.y, rect.width(), rect.height());
                }
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > 0.01 {
                    let factor = (scroll * 0.004).exp() as f64;
                    self.camera.zoom = (self.camera.zoom * factor).clamp(0.75, 512.0);
                }
            }

            ui.painter_at(rect).rect_filled(rect, 3.0, colors.recessed);
            if let Err(error) = self.tiles.update(
                ui,
                rect,
                self.assets.as_ref(),
                self.camera,
                self.style,
                self.show_contours,
                geometry::Layers {
                    countries: self.show_country_borders,
                    states: self.show_state_borders,
                    rivers: self.show_rivers,
                    roads: self.show_roads,
                },
            ) {
                self.render_error = Some(error);
            }
            self.contour_interval_m = self.tiles.contour;
            self.local_elev_range = self.tiles.range;

            let hit = paint_globe_map(
                ui,
                rect,
                &self.document,
                Some(self.selected),
                &self.camera,
                self.assets.as_deref(),
                self.contour_interval_m,
                self.local_elev_range,
                colors,
            );
            if response.clicked()
                && let Some(hit) = hit
            {
                self.selected = hit.feature;
                self.current = Some(hit);
            }
            if let Some(sample) = self.current.and_then(|hit| self.sample(hit))
                && let Some([px, py]) = self.camera.project(
                    sample.point.lat,
                    sample.point.lon,
                    rect.width(),
                    rect.height(),
                )
            {
                let point = egui::pos2(rect.left() + px, rect.top() + py);
                if rect.contains(point) {
                    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                    painter.circle_stroke(point, 7.0, egui::Stroke::new(2.0_f32, colors.focus));
                    painter.circle_filled(point, 2.5, colors.text_primary);
                }
            }

            if self.style == BasemapStyle::Oilslick {
                paint_oilslick_legend(ui, colors);
            }

            if let Some(error) = &self.render_error {
                ui.colored_label(colors.warning, error);
            }
            let blend = self.camera.globe_blend();
            let proj_label = if blend > 0.9 {
                "3D Globe"
            } else if blend < 0.1 {
                "2D Map"
            } else {
                "3D → 2D Transition"
            };
            let dem_source = if self.assets.as_ref().is_some_and(|a| {
                !a.terrain_tiles.is_empty() || a.global_dem.is_some() || a.cog_dem.is_some()
            }) {
                "DEM loaded"
            } else {
                "Terrain unavailable"
            };
            ui.label(format!(
                "Offline {proj_label} ({:.2}×) · {dem_source} · drag to rotate/pan, scroll to zoom",
                self.camera.zoom
            ));

            if !profile_visible {
                return;
            }
            let feature = &self.document.features[self.selected];
            let (profile, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ui.available_height().max(1.0)),
                egui::Sense::hover(),
            );
            let point = paint_profile(ui, profile, feature, self.selected, self.current, colors);
            if response.hovered() && point.is_some() && point != self.current {
                self.current = point;
                ui.ctx().request_repaint();
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.horizontal_wrapped(|ui| {
            ui.label("View:");
            for (label, mode) in [
                ("Auto 3D→2D", ProjectionMode::Auto),
                ("3D Globe", ProjectionMode::Globe3D),
                ("2D Map", ProjectionMode::Map2D),
            ] {
                if ui
                    .selectable_label(self.camera.mode == mode, label)
                    .clicked()
                {
                    self.camera.mode = mode;
                }
            }
            ui.separator();
            ui.label("Shading:");
            for (label, style) in [
                ("Natural", BasemapStyle::Natural),
                ("Hypsometric", BasemapStyle::Hypsometric),
                ("Oilslick", BasemapStyle::Oilslick),
                ("Hillshade", BasemapStyle::Hillshade),
            ] {
                if ui.selectable_label(self.style == style, label).clicked() {
                    self.style = style;
                }
            }
            ui.separator();
            ui.checkbox(&mut self.show_country_borders, "Countries");
            ui.checkbox(&mut self.show_state_borders, "States");
            ui.checkbox(&mut self.show_rivers, "Rivers");
            ui.checkbox(&mut self.show_roads, "Roads");
            ui.checkbox(&mut self.show_contours, "Contours");
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
        });
    }

    fn download_prompt_banner(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        colors: WorkbenchColors,
    ) {
        let needs_prompt = self.config.geography.prompt_on_open
            && !self.downloader.status.has_quick_start()
            && !self.prompt_dismissed;
        if !needs_prompt && !self.show_assets_panel && self.downloader.active.is_none() {
            return;
        }
        egui::Frame::none()
            .fill(colors.recessed)
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(pack) = self.downloader.active {
                        let prog = self.downloader.progress_snapshot();
                        ui.strong(pack.label());
                        ui.add(
                            egui::ProgressBar::new(prog.fraction)
                                .desired_width(180.0)
                                .text(prog.message),
                        );
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
                                    .button("Get all", palette)
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
                            self.downloader.max_assets_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                        ui.label(format!(
                            "Disk: {used_gb:.2}/{budget_gb:.1} GB · NE2: {} · Cities: {} · Borders: {} · Roads/Lakes: {} · GRWL: {} · Masks: {} · GSHHG: {} · GEBCO: {} · LandCover: {} · Local HGT: {}",
                            yes_no(st.has_ne2),
                            yes_no(st.has_cities),
                            yes_no(st.has_borders),
                            yes_no(st.has_roads_lakes),
                            yes_no(st.has_grwl),
                            yes_no(st.has_grwl_masks),
                            yes_no(st.has_gshhg),
                            yes_no(st.has_gebco),
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
                            || !st.has_gebco
                            || !st.has_land_cover
                            || !st.has_hydrorivers)
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Primary
                                        .button("Get all", palette)
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
                        if !st.has_gebco
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get GEBCO 2026 Terrain", palette)
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
                                        .button("Get Land Cover", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader.start(DownloadPack::LandCover, &self.config);
                        }
                        if !st.has_hydrorivers
                            && ui
                                .add(
                                    clip_ui::buttons::ButtonRole::Secondary
                                        .button("Get HydroRIVERS", palette)
                                        .compact(),
                                )
                                .clicked()
                        {
                            self.downloader
                                .start(DownloadPack::HydroRivers, &self.config);
                        }
                    }
                    if let Some(err) = &self.downloader.last_error {
                        ui.colored_label(colors.warning, err);
                    }
                });
            });
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
        let mut zoom_f32 = self.camera.zoom as f32;
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
                    egui::Slider::new(&mut zoom_f32, 0.75..=128.0)
                        .logarithmic(true)
                        .show_value(false)
                        .text("Zoom"),
                )
                .changed()
            {
                self.camera.zoom = zoom_f32 as f64;
            }
        });
        let height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_source("geography-features")
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
            .id_source("geography-details")
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
                        "Spherical length {:.2} km",
                        feature.distance_m / 1000.0
                    ));
                }
                if let Some([low, high]) = feature.elevation {
                    ui.label(format!("Supplied elevation {low:.1}–{high:.1} m"));
                }
                if let Some(sample) = self.current.and_then(|hit| self.sample(hit)) {
                    ui.separator();
                    ui.strong("Selected position");
                    ui.label(format!("Latitude {:.6}", sample.point.lat));
                    ui.label(format!("Longitude {:.6}", sample.point.lon));
                    if let Some(value) = sample.point.elevation {
                        ui.label(format!("Elevation {value:.1} m"));
                    }
                    if !sample.point.time.is_empty() {
                        ui.add(egui::Label::new(&sample.point.time).wrap(true));
                    }
                    ui.label(format!("Along feature {:.1} m", sample.distance_m));
                }
                for (name, value) in &feature.properties {
                    ui.strong(name);
                    ui.add(egui::Label::new(value).wrap(true));
                }
                for note in &self.document.notes {
                    ui.add(egui::Label::new(note).wrap(true));
                }
            });
    }
}

fn yes_no(v: bool) -> &'static str {
    if v { "ok" } else { "missing" }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Renders projected basemap raster, coastlines, cities, and GeoJSON/GPX overlay"
)]
fn paint_globe_map(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    doc: &Document,
    selected: Option<usize>,
    camera: &GlobeCamera,
    assets: Option<&BasemapAssets>,
    contour_interval_m: Option<f32>,
    local_elev_range: [f32; 2],
    colors: WorkbenchColors,
) -> Option<Hit> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // Natural Earth Populated Places: >= 250k at globe zoom, revealing all towns on close zoom, biggest first
    if let Some(assets) = assets
        && !assets.cities.is_empty()
    {
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
            let Some([px, py]) = camera.project(city.lat, city.lon, rect.width(), rect.height())
            else {
                continue;
            };
            let pos = egui::pos2(rect.left() + px, rect.top() + py);
            if !rect.contains(pos) {
                continue;
            }
            let font_size = if city.pop_max >= 1_000_000 {
                13.5
            } else {
                12.0
            };
            let galley = ui.fonts(|f| {
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
            if occupied.len() >= 56 {
                break;
            }
        }
    }

    // User GeoJSON / GPX features
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut nearest = 64.0;
    let mut hit = None;
    for (index, feature) in doc.features.iter().enumerate() {
        let color = if selected == Some(index) {
            colors.accent
        } else {
            colors.text_primary.gamma_multiply(0.85)
        };
        for (path_index, path) in feature.paths.iter().enumerate() {
            let mut positions = Vec::with_capacity(path.len());
            let mut sample_indices = Vec::with_capacity(path.len());
            for (s_idx, sample) in path.iter().enumerate() {
                if let Some([px, py]) = camera.project(
                    sample.point.lat,
                    sample.point.lon,
                    rect.width(),
                    rect.height(),
                ) {
                    positions.push(egui::pos2(rect.left() + px, rect.top() + py));
                    sample_indices.push(s_idx);
                } else if positions.len() > 1 && matches!(feature.kind, Kind::Line | Kind::Polygon)
                {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut positions),
                        egui::Stroke::new(
                            if selected == Some(index) {
                                3.0_f32
                            } else {
                                1.75_f32
                            },
                            color,
                        ),
                    ));
                    sample_indices.clear();
                } else {
                    positions.clear();
                    sample_indices.clear();
                }
            }
            if matches!(feature.kind, Kind::Line | Kind::Polygon) && positions.len() > 1 {
                painter.add(egui::Shape::line(
                    positions.clone(),
                    egui::Stroke::new(
                        if selected == Some(index) {
                            3.0_f32
                        } else {
                            1.75_f32
                        },
                        color,
                    ),
                ));
            }
            if feature.kind == Kind::Point || path.len() == 1 {
                for position in &positions {
                    painter.circle_filled(*position, 4.5, color);
                }
            }
            if let Some(pointer) = pointer
                && let Some((pos_idx, distance)) = nearest_on_path(
                    &positions,
                    pointer,
                    matches!(feature.kind, Kind::Line | Kind::Polygon),
                )
                && distance < nearest
                && let Some(&orig_point) = sample_indices.get(pos_idx)
            {
                nearest = distance;
                hit = Some(Hit {
                    feature: index,
                    path: path_index,
                    point: orig_point,
                });
            }
        }
    }
    if let Some(hit) = hit {
        let sample = &doc.features[hit.feature].paths[hit.path][hit.point];
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("geographic-position"), |ui| {
            ui.label(&doc.features[hit.feature].name);
            ui.label(format!("{:.6}, {:.6}", sample.point.lat, sample.point.lon));
        });
    }
    paint_scale_and_contour_overlay(
        ui,
        &painter,
        rect,
        camera,
        contour_interval_m,
        local_elev_range,
    );
    hit
}

fn paint_scale_and_contour_overlay(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    camera: &GlobeCamera,
    contour_interval_m: Option<f32>,
    local_elev_range: [f32; 2],
) {
    let scale_px = camera.scale_px(rect.width(), rect.height()).max(1.0);
    let meters_per_px = 6_371_000.0 / scale_px;
    let target_m = meters_per_px * 110.0;
    let nice_m = [
        500.0,
        1_000.0,
        2_000.0,
        5_000.0,
        10_000.0,
        20_000.0,
        50_000.0,
        100_000.0,
        200_000.0,
        500_000.0,
        1_000_000.0,
        2_000_000.0,
        5_000_000.0,
    ]
    .into_iter()
    .rfind(|&v| v <= target_m)
    .unwrap_or(500.0);
    let bar_w = (nice_m / meters_per_px).clamp(32.0, 180.0) as f32;
    let scale_label = if nice_m >= 1000.0 {
        format!("{:.0} km", nice_m / 1000.0)
    } else {
        format!("{nice_m:.0} m")
    };

    let mut text = scale_label;
    if let Some(interval) = contour_interval_m {
        text.push_str(&format!(
            "  ·  Contour: {interval:.0} m (index {:.0} m)  ·  Elev: {:.0}–{:.0} m",
            interval * 5.0,
            local_elev_range[0],
            local_elev_range[1]
        ));
    }
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(
            text,
            egui::FontId::proportional(11.5),
            egui::Color32::from_rgb(244, 246, 250),
        )
    });
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
}

fn paint_oilslick_legend(ui: &mut egui::Ui, colors: WorkbenchColors) {
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
        "Oilslick: 500m cyclic lightness + sea-level break",
        egui::TextStyle::Small.resolve(ui.style()),
        colors.text_secondary,
    );
}

fn paint_map(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    doc: &Document,
    selected: Option<usize>,
    zoom: f32,
    pan: egui::Vec2,
    colors: WorkbenchColors,
) -> Option<Hit> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut nearest = 64.0;
    let mut hit = None;
    for (index, feature) in doc.features.iter().enumerate() {
        let color = if selected == Some(index) {
            colors.accent
        } else {
            colors.text_secondary
        };
        for (path_index, path) in feature.paths.iter().enumerate() {
            let positions: Vec<_> = path
                .iter()
                .map(|sample| project(rect, doc.bounds, sample.xy, zoom, pan))
                .collect();
            if matches!(feature.kind, Kind::Line | Kind::Polygon) && positions.len() > 1 {
                painter.add(egui::Shape::line(
                    positions.clone(),
                    egui::Stroke::new(
                        if selected == Some(index) {
                            3.0_f32
                        } else {
                            1.5_f32
                        },
                        color,
                    ),
                ));
            }
            if feature.kind == Kind::Point || path.len() == 1 {
                for position in &positions {
                    painter.circle_filled(*position, 4.0, color);
                }
            }
            if let Some(pointer) = pointer
                && let Some((point, distance)) = nearest_on_path(
                    &positions,
                    pointer,
                    matches!(feature.kind, Kind::Line | Kind::Polygon),
                )
                && distance < nearest
            {
                nearest = distance;
                hit = Some(Hit {
                    feature: index,
                    path: path_index,
                    point,
                });
            }
        }
    }
    if let Some(hit) = hit {
        let sample = &doc.features[hit.feature].paths[hit.path][hit.point];
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("geographic-position"), |ui| {
            ui.label(&doc.features[hit.feature].name);
            ui.label(format!("{:.6}, {:.6}", sample.point.lat, sample.point.lon));
        });
    }
    hit
}

fn nearest_on_path(
    positions: &[egui::Pos2],
    pointer: egui::Pos2,
    connected: bool,
) -> Option<(usize, f32)> {
    let mut best = positions
        .iter()
        .enumerate()
        .map(|(index, p)| (index, p.distance_sq(pointer)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    if connected {
        for (point, pair) in positions.windows(2).enumerate() {
            let (t, distance) = crate::ui_ext::segment_hit(pointer, pair[0], pair[1]);
            if distance < best.1 {
                best = (point + usize::from(t > 0.5), distance);
            }
        }
    }
    Some(best)
}

fn paint_profile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    feature: &Feature,
    feature_index: usize,
    current: Option<Hit>,
    colors: WorkbenchColors,
) -> Option<Hit> {
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
            "Supplied elevation · {low:.1}–{high:.1} m · {:.2} km",
            feature.distance_m / 1000.0
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
    hit
}

pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
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
    let document = ready.and_then(|r| r.data.geography.as_ref())?;
    let map = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.left() + rect.width().min(240.0), rect.bottom()),
    );
    let mut hovered =
        paint_map(ui, map, document, None, 1.0, egui::Vec2::ZERO, colors).map(|hit| hit.feature);
    let mut x = map.right() + 4.0;
    for (index, feature) in document.features.iter().enumerate().take(4) {
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
    fn sampled_folder_features_and_viewer_selection_keep_source_indices() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.gpx");
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.geography.unwrap();
        assert!(summary.samples <= 512);
        assert_eq!(summary.features[1].name, "Ferry route");
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
        assert_eq!(summary.features[0].kind, Kind::Polygon);
        assert_eq!(summary.features[0].paths.len(), 2);
        assert!(summary.samples <= 512);
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
    #[test]
    fn line_hits_choose_actual_endpoints_and_unconnected_points_have_no_line_hits() {
        let path = [egui::pos2(10.0, 10.0), egui::pos2(110.0, 10.0)];
        assert_eq!(
            nearest_on_path(&path, egui::pos2(75.0, 11.0), true),
            Some((1, 1.0))
        );
        assert!(
            nearest_on_path(&path, egui::pos2(75.0, 11.0), false)
                .unwrap()
                .1
                > 1000.0
        );
        assert!(nearest_on_path(&[], egui::Pos2::ZERO, true).is_none());
    }
}
