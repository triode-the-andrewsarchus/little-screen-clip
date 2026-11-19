//! VCV Rack / Cardinal (`.vcv`) preview strip (importance-ranked unique modules) and
//! full Rack rail pan/zoom preview (modules placed at actual saved coordinates on rails).

use super::{
    filesystem::Entry,
    media::PreviewData,
    shelf::preview::{HoverRaster, Previews, Ready, Source},
};
use crate::{icons::Icon, shell::StudioShell, ui_appearance::WorkbenchColors};
use egui::{Color32, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2};
use little_clip_core::{
    BackgroundJobs, ImageThumbnail,
    jobs::task::Task,
    palette::AppPalette,
    vcv::{
        MAX_PATCH_BYTES, PanelAssets, PatchAnalysis, PatchDocument, PatchSummary, RACK_HP_WIDTH,
        RACK_ROW_HEIGHT, RackRailLayout, ScreenshotBatchResult, StripLineup, analyze_patch,
        build_rack_rail_layout, clear_downloaded_panel_cache,
        count_missing_local_screenshot_plugins, download_viewer_panels,
        ensure_local_screenshots_for_patch, generate_all_local_rack_screenshots,
        inspect_path_bytes_cancellable, load_viewer_panels, local_rack_available, read_patch_path,
        render_full_strip_rgba, render_rack_rgba, render_strip_rgba,
    },
};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

const MAX_VCV_BYTES: usize = MAX_PATCH_BYTES as usize;
const VIEWER_MIN_ZOOM: f32 = 0.15;
const VIEWER_MAX_ZOOM: f32 = 3.0;
const STRIP_RASTER_SCALE: f32 = 2.0;
pub(super) static PREVIEW_JOBS: LazyLock<BackgroundJobs> =
    LazyLock::new(|| BackgroundJobs::new(2, 8));
static PANEL_JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));

pub(in crate::file_manager) struct Document {
    pub summary: Arc<PatchSummary>,
}

#[derive(Clone, Copy)]
enum Artwork {
    RankedStrip,
    FullStrip,
    Rack(f32),
}

#[derive(Clone)]
pub(super) struct RasterRequest {
    summary: Arc<PatchSummary>,
    artwork: Artwork,
    dark: bool,
}

impl RasterRequest {
    pub(super) fn load(self, token: &AtomicBool) -> PreviewData {
        let mut data = PreviewData::new(
            self.summary.title.clone(),
            "VCV Rack preview unavailable".into(),
            Icon::Midi,
        );
        if token.load(Ordering::Relaxed) {
            return data;
        }
        let rendered = match self.artwork {
            Artwork::RankedStrip => render_strip_rgba(&self.summary, STRIP_RASTER_SCALE, self.dark),
            Artwork::FullStrip => {
                render_full_strip_rgba(&self.summary, STRIP_RASTER_SCALE, self.dark)
                    .unwrap_or_else(|| Err("Patch has no single-row strip".into()))
            }
            Artwork::Rack(scale) => render_rack_rgba(&self.summary, scale, self.dark),
        };
        match rendered {
            Ok(raster) => {
                data.image = Some(ImageThumbnail {
                    width: raster.width,
                    height: raster.height,
                    premultiplied_rgba: raster.rgba,
                })
            }
            Err(error) => data.detail = error,
        }
        data
    }
}

#[derive(Default)]
pub(in crate::file_manager) struct Hover {
    rack: HoverRaster,
    strips: Previews,
    strip_ids: HashMap<(uuid::Uuid, bool, bool), uuid::Uuid>,
}

impl Hover {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        self.rack.poll(ctx);
        self.strips.poll(ctx);
    }

    pub fn retain(&mut self, visible: &BTreeSet<uuid::Uuid>) {
        self.rack.retain(|id| visible.contains(&id));
        self.strip_ids.retain(|(id, _, _), _| visible.contains(id));
        self.strips
            .retain(&self.strip_ids.values().copied().collect());
    }

    fn strip_texture(
        &mut self,
        identity: uuid::Uuid,
        doc: &Document,
        dark: bool,
        full: bool,
    ) -> Option<egui::TextureId> {
        let id = *self
            .strip_ids
            .entry((identity, dark, full))
            .or_insert_with(uuid::Uuid::new_v4);
        if self.strips.needs(id) {
            self.strips.request(
                id,
                Source::VcvRaster(RasterRequest {
                    summary: Arc::clone(&doc.summary),
                    artwork: if full {
                        Artwork::FullStrip
                    } else {
                        Artwork::RankedStrip
                    },
                    dark,
                }),
            );
        }
        self.strips.get(id)?.texture.as_ref().map(TextureHandle::id)
    }

    fn show(&mut self, ui: &Ui, body: Rect, identity: uuid::Uuid, doc: &Document, dark: bool) {
        if super::ui::popup_open(ui.ctx()) {
            return;
        }
        let screen = ui.ctx().content_rect().shrink(8.0);
        let space_below = (screen.bottom() - body.bottom() - 8.0).max(0.0);
        let space_above = (body.top() - 8.0 - screen.top()).max(0.0);
        let place_below = space_below >= 240.0 || space_below >= space_above;
        let avail_h = if place_below {
            space_below
        } else {
            space_above
        };
        let avail_w = (screen.width() - 16.0).min(1440.0);
        if avail_h < 24.0 || avail_w < 24.0 {
            return;
        }
        let w_bucket = (avail_w / 32.0).floor().max(1.0) as u32;
        let h_bucket = (avail_h / 24.0).floor().max(1.0) as usize;
        let dpr_bucket = (ui.ctx().pixels_per_point().clamp(1.0, 2.0) * 4.0).round() as u32;
        let rack = &doc.summary.rack;
        let natural = vec2(rack.width as f32, rack.height as f32);
        let fit = (avail_w / natural.x).min(avail_h / natural.y).min(1.0);
        let shown_size = natural * fit;
        let render_fit = (w_bucket as f32 * 32.0 / natural.x)
            .min(h_bucket as f32 * 24.0 / natural.y)
            .min(1.0);
        let ready = self.rack.request(
            identity,
            h_bucket * 2 + usize::from(dark),
            w_bucket * 16 + dpr_bucket,
            Source::VcvRaster(RasterRequest {
                summary: Arc::clone(&doc.summary),
                artwork: Artwork::Rack(render_fit * dpr_bucket as f32 / 4.0),
                dark,
            }),
        );
        if ready.is_none() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }

        let left = body.left().clamp(
            screen.left(),
            (screen.right() - shown_size.x).max(screen.left()),
        );
        let top = if place_below {
            (body.bottom() + 6.0).min((screen.bottom() - shown_size.y).max(screen.top()))
        } else {
            (body.top() - 6.0 - shown_size.y).max(screen.top())
        };

        egui::Area::new(egui::Id::unique(("vcv_strip_hover", identity)))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos2(left, top))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                let (rect, _) = ui.allocate_exact_size(shown_size, egui::Sense::hover());
                let bg = if dark {
                    Color32::from_rgb(18, 19, 23)
                } else {
                    Color32::from_rgb(32, 34, 40)
                };
                let border = if dark {
                    Color32::from_rgb(76, 80, 92)
                } else {
                    Color32::from_rgb(130, 136, 148)
                };
                ui.painter().rect_filled(rect.expand(2.0), 6.0, bg);
                if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        ready.map_or("Rendering rack preview…", |ready| {
                            ready.data.detail.as_str()
                        }),
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(180, 186, 198),
                    );
                }
                ui.painter().rect_stroke(
                    rect.expand(2.0),
                    6.0,
                    egui::Stroke::new(1.0_f32, border),
                    egui::StrokeKind::Middle,
                );
            });
    }
}

pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(
        super::display_path(path),
        "VCV Rack patch preview unavailable".into(),
        Icon::Midi,
    );
    let bytes = match crate::file_source::read_file_snapshot(path, MAX_VCV_BYTES) {
        Ok(b) => b,
        Err(err) => {
            data.detail = err;
            return data;
        }
    };
    if token.load(Ordering::Relaxed) {
        data.detail = "Cancelled".into();
        return data;
    }
    let summary = match inspect_path_bytes_cancellable(path, &bytes, token) {
        Ok(summary) => Arc::new(summary),
        Err(error) => {
            data.detail = error;
            return data;
        }
    };
    let mut data = RasterRequest {
        summary: Arc::clone(&summary),
        artwork: Artwork::RankedStrip,
        dark: true,
    }
    .load(token);
    data.title = super::display_path(path);
    if data.image.is_some() {
        data.detail = format_summary_detail(&summary.analysis.document);
    }
    data.vcv = Some(Arc::new(Document { summary }));
    data
}

fn format_summary_detail(document: &PatchDocument) -> String {
    let modules = document.modules.len();
    let cables = document.cables.len();
    let mod_word = if modules == 1 { "module" } else { "modules" };
    let cable_word = if cables == 1 { "cable" } else { "cables" };
    let rows = document.row_count();
    let row_word = if rows == 1 { "row" } else { "rows" };
    format!("{modules} {mod_word} · {cables} {cable_word} · {rows} {row_word}")
}

fn strip_for_preview(summary: &PatchSummary, body_size: egui::Vec2) -> (&StripLineup, bool) {
    if let Some(full) = summary.full_strip.as_ref() {
        let scale = (f64::from(body_size.y) / full.height).min(1.0);
        if full.width * scale <= f64::from(body_size.x) {
            return (full, true);
        }
    }
    (&summary.ranked_strip, false)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::file_manager) fn show_list(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    identity: uuid::Uuid,
    ready: Option<&Ready>,
    hover: &mut Hover,
    colors: WorkbenchColors,
) {
    let detail = columns.metadata_fields().is_empty().then(|| {
        ready
            .map_or("VCV Rack patch", |r| r.data.detail.as_str())
            .to_owned()
    });
    super::ui::media_header(
        ui,
        Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
        entry,
        detail,
        Some(columns),
        colors,
    );
    let body = Rect::from_min_max(row.min + vec2(6.0, 32.0), row.max - vec2(6.0, 10.0));
    if !body.is_positive() {
        return;
    }
    let Some(doc) = ready.and_then(|r| r.data.vcv.as_ref()) else {
        ui.painter()
            .with_clip_rect(body.intersect(ui.clip_rect()))
            .text(
                body.left_center() + vec2(4.0, 0.0),
                egui::Align2::LEFT_CENTER,
                ready.map_or("Loading VCV Rack patch…", |r| r.data.detail.as_str()),
                egui::FontId::proportional(12.5),
                colors.text_secondary,
            );
        return;
    };

    let dark = ui.visuals().dark_mode;
    let (lineup, full) = strip_for_preview(&doc.summary, body.size());
    let ranked = ready
        .and_then(|ready| ready.texture.as_ref())
        .map(TextureHandle::id);
    let selected = if dark && !full {
        ranked.map(|texture| (texture, lineup))
    } else {
        hover
            .strip_texture(identity, doc, dark, full)
            .map(|texture| (texture, lineup))
            .or_else(|| ranked.map(|texture| (texture, &doc.summary.ranked_strip)))
    };
    if hover.strips.has_pending() {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let hovered = super::ui::preview_hovered(ui, body);
    if let Some((texture, layout)) = selected {
        let natural = vec2(layout.width as f32, layout.height as f32);
        let scaled = natural * (body.height() / natural.y).min(1.0);
        let visible_w = scaled.x.min(body.width());
        let uv_w = (visible_w / scaled.x).clamp(0.0, 1.0);
        let pan_u = if uv_w < 1.0
            && hovered
            && let Some(pos) = ui.ctx().pointer_hover_pos()
        {
            ((pos.x - body.left()) / body.width()).clamp(0.0, 1.0) * (1.0 - uv_w)
        } else {
            0.0
        };
        let top = body.top() + (body.height() - scaled.y).max(0.0) * 0.5;
        ui.painter()
            .with_clip_rect(body.intersect(ui.clip_rect()))
            .image(
                texture,
                Rect::from_min_size(pos2(body.left(), top), vec2(visible_w, scaled.y)),
                Rect::from_min_max(pos2(pan_u, 0.0), pos2((pan_u + uv_w).min(1.0), 1.0)),
                Color32::WHITE,
            );
    } else {
        ui.painter()
            .with_clip_rect(body.intersect(ui.clip_rect()))
            .text(
                body.left_center() + vec2(4.0, 0.0),
                egui::Align2::LEFT_CENTER,
                ready.map_or("VCV Rack preview unavailable", |ready| {
                    ready.data.detail.as_str()
                }),
                egui::FontId::proportional(12.5),
                colors.text_secondary,
            );
    }
    if hovered {
        hover.show(ui, body, identity, doc, dark);
    }
}

fn texture_from_rgba(ctx: &egui::Context, name: &str, rgba: &image::RgbaImage) -> TextureHandle {
    let size = [rgba.width() as usize, rgba.height() as usize];
    let img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    ctx.load_texture(name, img, TextureOptions::LINEAR)
}

#[derive(Clone, Copy)]
enum PanelAction {
    Download,
    Clear,
    CapturePatch,
    CaptureAll,
}

impl PanelAction {
    fn busy_label(self) -> &'static str {
        match self {
            Self::Download => "Downloading panel artwork",
            Self::Clear => "Clearing downloaded artwork",
            Self::CapturePatch | Self::CaptureAll => "Capturing local Rack screenshots",
        }
    }
}

struct PanelJob {
    action: PanelAction,
    task: Task<(ViewerArtwork, String)>,
    progress: mpsc::Receiver<(usize, usize)>,
    counts: Option<(usize, usize)>,
}

struct ViewerArtwork {
    rack: RackRailLayout,
    panels: PanelAssets,
    missing_local_plugins: usize,
}

impl ViewerArtwork {
    fn new(analysis: &PatchAnalysis, modules: &[(String, String)], panels: PanelAssets) -> Self {
        Self {
            rack: build_rack_rail_layout(analysis, &panels),
            panels,
            missing_local_plugins: count_missing_local_screenshot_plugins(modules),
        }
    }
}

/// Interactive VCV Rack rail pan/zoom viewer.
pub(in crate::file_manager) struct Viewer {
    title: String,
    analysis: Arc<PatchAnalysis>,
    modules: Arc<[(String, String)]>,
    artwork: ViewerArtwork,
    zoom: f32,
    min_zoom: f32,
    pan: egui::Vec2,
    fitted_once: bool,
    panel_textures: HashMap<(String, String), TextureHandle>,
    has_local_rack: bool,
    panel_job: Option<PanelJob>,
    panel_status: Option<String>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let document = read_patch_path(path)?;
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let analysis = Arc::new(analyze_patch(document)?);
        let modules: Arc<[(String, String)]> = analysis.document.panel_keys().into();
        let panels = load_viewer_panels(&modules, cancel);
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        Ok(Self {
            title: path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("VCV Patch")
                .into(),
            artwork: ViewerArtwork::new(&analysis, &modules, panels),
            analysis,
            modules,
            zoom: 0.65,
            min_zoom: VIEWER_MIN_ZOOM,
            pan: egui::Vec2::ZERO,
            fitted_once: false,
            panel_textures: HashMap::new(),
            has_local_rack: local_rack_available(),
            panel_job: None,
            panel_status: None,
        })
    }

    pub fn panel_job_pending(&self) -> bool {
        self.panel_job.is_some()
    }

    pub fn cancel_panel_job(&self) {
        if let Some(job) = &self.panel_job
            && !matches!(job.action, PanelAction::Clear)
        {
            job.task.token.store(true, Ordering::Relaxed);
        }
    }

    fn start_panel_job(&mut self, action: PanelAction) {
        let modules = Arc::clone(&self.modules);
        let analysis = Arc::clone(&self.analysis);
        let (send, progress) = mpsc::sync_channel(1);
        let task = Task::start_unbounded(&PANEL_JOBS, move |cancel| {
            let (panels, status) = match action {
                PanelAction::Download => {
                    let (mut panels, result) = download_viewer_panels(&modules, cancel);
                    let cancelled = cancel.load(Ordering::Relaxed);
                    if cancelled {
                        panels = load_viewer_panels(&modules, &AtomicBool::new(false));
                    }
                    let status = match result {
                        Err(error) => error,
                        Ok(count) if cancelled => {
                            format!("Download cancelled ({count} panels cached)")
                        }
                        Ok(0) => "No additional artwork available".into(),
                        Ok(count) => format!("Downloaded {count} panels"),
                    };
                    (panels, status)
                }
                PanelAction::Clear => {
                    let status = clear_downloaded_panel_cache()
                        .map_or_else(|error| error, |()| "Downloaded artwork cleared".into());
                    (
                        load_viewer_panels(&modules, &AtomicBool::new(false)),
                        status,
                    )
                }
                PanelAction::CapturePatch | PanelAction::CaptureAll => {
                    let result = if matches!(action, PanelAction::CapturePatch) {
                        ensure_local_screenshots_for_patch(&modules, cancel)
                    } else {
                        generate_all_local_rack_screenshots(cancel, |done, total, _| {
                            let _ = send.try_send((done, total));
                        })
                    };
                    let status = capture_status(result);
                    (
                        load_viewer_panels(&modules, &AtomicBool::new(false)),
                        status,
                    )
                }
            };
            (ViewerArtwork::new(&analysis, &modules, panels), status)
        });
        self.panel_job = Some(PanelJob {
            action,
            task,
            progress,
            counts: None,
        });
    }

    fn poll_panel_job(&mut self, ctx: &egui::Context) {
        let Some(job) = self.panel_job.as_mut() else {
            return;
        };
        if let Ok(counts) = job.progress.try_recv() {
            job.counts = Some(counts);
        }
        let Some(result) = job.task.poll() else {
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        };
        self.panel_job = None;
        self.panel_status = Some(match result {
            Ok((artwork, status)) => {
                self.artwork = artwork;
                self.panel_textures.clear();
                status
            }
            Err(error) => error.to_string(),
        });
    }

    fn panel_controls(&mut self, ui: &mut Ui) {
        if let Some(job) = &self.panel_job {
            if job.task.token.load(Ordering::Relaxed) {
                ui.label("Cancelling panel job…");
            } else {
                let label = job.action.busy_label();
                if let Some((done, total)) = job.counts {
                    ui.label(format!("{label} ({done}/{total})…"));
                } else {
                    ui.label(format!("{label}…"));
                }
                if !matches!(job.action, PanelAction::Clear) && ui.button("Cancel").clicked() {
                    self.cancel_panel_job();
                }
            }
            return;
        }
        ui.menu_button("Panel Artwork", |ui| {
            let actions = [
                (
                    true,
                    "Download Missing Panels".into(),
                    PanelAction::Download,
                ),
                (true, "Clear Downloaded Cache".into(), PanelAction::Clear),
                (
                    self.has_local_rack && self.artwork.missing_local_plugins > 0,
                    format!(
                        "Generate Patch Screenshots ({})",
                        self.artwork.missing_local_plugins
                    ),
                    PanelAction::CapturePatch,
                ),
                (
                    self.has_local_rack,
                    "Generate All Local Screenshots".into(),
                    PanelAction::CaptureAll,
                ),
            ];
            for (available, label, action) in actions {
                if !available {
                    continue;
                }
                let button = ui.button(label);
                let button = if matches!(action, PanelAction::Download) {
                    button.on_hover_text("Requests missing module images from the VCV Library")
                } else {
                    button
                };
                if button.clicked() {
                    self.start_panel_job(action);
                    ui.ctx().request_repaint();
                    ui.close();
                }
            }
        });
        if let Some(status) = &self.panel_status {
            ui.small(status);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        self.poll_panel_job(ui.ctx());
        StudioShell::new("VCV Rack", Icon::Midi, palette, close).render_header(ui, |ui| {
            ui.label(format!(
                "{} · {}",
                self.title,
                format_summary_detail(&self.analysis.document)
            ));
            ui.separator();
            if ui.button("−").clicked() {
                self.zoom = (self.zoom * 0.8).clamp(self.min_zoom, VIEWER_MAX_ZOOM);
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            if ui.button("+").clicked() {
                self.zoom = (self.zoom * 1.25).clamp(self.min_zoom, VIEWER_MAX_ZOOM);
            }
            if ui.button("Fit Rack").clicked() {
                self.fitted_once = false;
            }
            self.panel_controls(ui);
        });
        self.show_rack(ui);
    }

    fn show_rack(&mut self, ui: &mut Ui) {
        let rack = &self.artwork.rack;
        let viewport = ui.available_rect_before_wrap();
        if !self.fitted_once && viewport.width() > 32.0 && viewport.height() > 32.0 {
            let fit_w = (viewport.width() - 24.0) / rack.width.max(1.0) as f32;
            let fit_h = (viewport.height() - 24.0) / rack.height.max(1.0) as f32;
            self.zoom = fit_w.min(fit_h).clamp(0.0001, 1.5);
            self.min_zoom = self.zoom.min(VIEWER_MIN_ZOOM);
            let drawn = vec2(rack.width as f32, rack.height as f32) * self.zoom;
            self.pan = (viewport.size() - drawn) * 0.5;
            self.fitted_once = true;
        }

        let response = ui.allocate_rect(viewport, egui::Sense::click_and_drag());
        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.5 {
                let factor = (1.0 + scroll * 0.002).clamp(0.5, 2.0);
                let old_zoom = self.zoom;
                let new_zoom = (old_zoom * factor).clamp(self.min_zoom, VIEWER_MAX_ZOOM);
                if let Some(pointer) = ui.ctx().pointer_hover_pos() {
                    let rel = pointer - viewport.min - self.pan;
                    self.pan -= rel * (new_zoom / old_zoom - 1.0);
                }
                self.zoom = new_zoom;
            }
        }

        let clip = viewport.intersect(ui.clip_rect());
        let painter = ui.painter().with_clip_rect(clip);
        painter.rect_filled(viewport, 0.0, Color32::from_rgb(16, 17, 21));

        let origin = viewport.min + self.pan;
        let case_rect = Rect::from_min_size(
            origin,
            vec2(rack.width as f32, rack.height as f32) * self.zoom,
        );
        painter.rect_filled(case_rect, 6.0 * self.zoom, Color32::from_rgb(20, 22, 27));

        paint_visible_rails(&painter, rack, origin, self.zoom, clip);
        let hover_pos = response
            .hovered()
            .then(|| ui.ctx().pointer_hover_pos())
            .flatten();
        let mut hovered = None;
        for module in &rack.modules {
            let rect = Rect::from_min_size(
                origin + vec2(module.x as f32, module.y as f32) * self.zoom,
                vec2(module.width as f32, module.height as f32) * self.zoom,
            );
            if !rect.intersects(clip) {
                continue;
            }
            let texture = self
                .artwork
                .panels
                .viewer_image(&module.plugin, &module.model)
                .map(|rgba| {
                    self.panel_textures
                        .entry((module.plugin.clone(), module.model.clone()))
                        .or_insert_with(|| {
                            texture_from_rgba(
                                ui.ctx(),
                                &format!("vcv_mod_{}_{}", module.plugin, module.model),
                                rgba,
                            )
                        })
                        .id()
                });
            paint_texture_or_placeholder(&painter, module, rect, self.zoom, texture);
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0_f32, Color32::from_rgb(12, 13, 16)),
                egui::StrokeKind::Middle,
            );
            if hover_pos.is_some_and(|position| rect.contains(position)) {
                hovered = Some((module, rect));
            }
        }

        if let Some((m, rect)) = hovered {
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(2.0_f32, Color32::from_rgb(255, 212, 82)),
                egui::StrokeKind::Middle,
            );
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                egui::Id::unique(("vcv_viewer_module_tip", m.id)),
                egui::PopupAnchor::Pointer,
            )
            .show(|ui| {
                ui.label(format!(
                    "{} ({}/{}) · {} HP",
                    m.title, m.plugin, m.model, m.hp
                ));
                ui.small(format!(
                    "{} · {} · row {}, col {}",
                    m.role.label(),
                    m.function.label(),
                    m.row,
                    m.column_hp
                ));
            });
        }
    }
}

fn capture_status(result: ScreenshotBatchResult) -> String {
    let count = result.rendered_modules;
    if result.cancelled {
        return format!("Capture cancelled ({count} panels captured)");
    }
    if !result.failed_plugins.is_empty() {
        return format!(
            "Captured {count} panels. Rack could not render {}",
            result.failed_plugins.join(", ")
        );
    }
    if count == 0 {
        "No additional Rack screenshots captured".into()
    } else {
        format!(
            "Captured {count} local panels ({} plugins)",
            result.rendered_plugins
        )
    }
}

fn paint_visible_rails(
    painter: &egui::Painter,
    rack: &RackRailLayout,
    origin: Pos2,
    zoom: f32,
    clip: Rect,
) {
    let rail_w = (rack.width as f32 - 24.0) * zoom;
    let row_h = RACK_ROW_HEIGHT as f32 * zoom;
    let rail_h = (15.0 * zoom).max(2.0).min(row_h / 3.0);
    let pitch = (RACK_HP_WIDTH as f32 * 2.0 * zoom).max(8.0);
    let radius = (2.0 * zoom).clamp(0.8, 3.0);
    for &(_, row_y) in &rack.rows {
        let row = Rect::from_min_size(
            origin + vec2(12.0 * zoom, row_y as f32 * zoom),
            vec2(rail_w, row_h),
        );
        if !row.intersects(clip) {
            continue;
        }
        painter.rect_filled(row, 0.0, Color32::from_rgb(24, 26, 32));
        for y in [row.top(), row.bottom() - rail_h] {
            let rail = Rect::from_min_size(pos2(row.left(), y), vec2(rail_w, rail_h));
            if !rail.intersects(clip) {
                continue;
            }
            painter.rect_filled(rail, 0.0, Color32::from_rgb(56, 60, 70));
            let origin = rail.left() + pitch * 0.5;
            let first = ((clip.left() - radius - origin) / pitch).ceil().max(0.0) as i32;
            let last = ((clip.right() + radius).min(rail.right() - 4.0) - origin) / pitch;
            for slot in first..=last.floor() as i32 {
                painter.circle_filled(
                    pos2(origin + pitch * slot as f32, rail.center().y),
                    radius,
                    Color32::from_rgb(24, 26, 31),
                );
            }
        }
    }
}

fn paint_texture_or_placeholder(
    painter: &egui::Painter,
    m: &little_clip_core::vcv::PlacedRackModule,
    mod_rect: Rect,
    zoom: f32,
    texture_id: Option<egui::TextureId>,
) {
    if let Some(tid) = texture_id {
        painter.image(
            tid,
            mod_rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.rect_filled(mod_rect, 2.0, Color32::from_rgb(38, 41, 49));
        painter.text(
            mod_rect.center(),
            egui::Align2::CENTER_CENTER,
            &m.title,
            egui::FontId::proportional((12.0 * zoom).clamp(7.0, 18.0)),
            Color32::from_rgb(228, 232, 238),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vcv_bytes() -> Vec<u8> {
        let json = serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Core", "model": "MIDIToCVInterface", "pos": [0, 0] },
                { "id": 2, "plugin": "Fundamental", "model": "VCO", "pos": [10, 0] },
                { "id": 3, "plugin": "Fundamental", "model": "VCF", "pos": [20, 0] },
                { "id": 4, "plugin": "Fundamental", "model": "VCA-1", "pos": [30, 0] },
                { "id": 5, "plugin": "Core", "model": "AudioInterface2", "pos": [40, 0] },
                { "id": 6, "plugin": "Fundamental", "model": "Scope", "pos": [50, 0] }
            ],
            "cables": [
                { "id": 1, "outputModuleId": 1, "outputId": 0, "inputModuleId": 2, "inputId": 0, "color": "#ffd452" },
                { "id": 2, "outputModuleId": 2, "outputId": 0, "inputModuleId": 3, "inputId": 0, "color": "#ff5252" },
                { "id": 3, "outputModuleId": 3, "outputId": 0, "inputModuleId": 4, "inputId": 0, "color": "#ff5252" },
                { "id": 4, "outputModuleId": 4, "outputId": 0, "inputModuleId": 5, "inputId": 0, "color": "#ff5252" },
                { "id": 5, "outputModuleId": 3, "outputId": 0, "inputModuleId": 6, "inputId": 0, "color": "#ff5252" }
            ]
        });
        serde_json::to_vec(&json).unwrap()
    }

    fn wait_for<T>(mut poll: impl FnMut() -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = poll() {
                return value;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "VCV preview job did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loads_ranked_strip_and_viewer_and_renders_hover_on_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.vcv");
        std::fs::write(&path, sample_vcv_bytes()).unwrap();
        let token = AtomicBool::new(false);
        let data = load(&path, &token);
        let doc = data.vcv.as_ref().expect("vcv document");
        assert_eq!(data.detail, "6 modules · 5 cables · 1 row");
        assert!(data.image.as_ref().unwrap().width > 0);

        let ctx = egui::Context::default();
        let mut hover = Hover::default();
        let row_id = uuid::Uuid::new_v4();
        let body = Rect::from_min_size(pos2(10.0, 40.0), vec2(760.0, 66.0));
        wait_for(|| {
            hover.poll(&ctx);
            let output = clip_ui::test_support::run_frame(&ctx, egui::RawInput::default(), |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default()
                        .show(ui, |ui| hover.show(ui, body, row_id, doc, true))
                });
            });
            output
                .shapes
                .iter()
                .any(|shape| {
                    matches!(
                        &shape.shape,
                        egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default()
                    )
                })
                .then_some(())
        });

        let mut viewer = Viewer::load(&path, &token).expect("viewer load");
        assert_eq!(viewer.artwork.rack.modules.len(), 6);
        let mut close = false;
        let _ = clip_ui::test_support::run_frame(&ctx, egui::RawInput::default(), |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    viewer.ui(ui, &mut close, &AppPalette::default());
                })
            });
        });
        assert!(!close);
        assert!(viewer.fitted_once);
        assert!(viewer.zoom <= VIEWER_MAX_ZOOM);
        token.store(true, Ordering::Relaxed);
        assert!(Viewer::load(&path, &token).is_err());
    }

    #[test]
    fn strip_preset_vcvss_renders_modules_and_zero_row_detail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chain.vcvss");
        let json = serde_json::json!({
            "stripVersion": 1,
            "leftModules": [
                { "id": 1, "plugin": "Fundamental", "model": "VCO" }
            ],
            "rightModules": [
                { "id": 2, "plugin": "Fundamental", "model": "VCF" },
                { "id": 3, "plugin": "Fundamental", "model": "VCA-1" }
            ],
            "cables": [
                { "outputModuleId": 1, "outputId": 0, "inputModuleId": 2, "inputId": 0 },
                { "outputModuleId": 2, "outputId": 0, "inputModuleId": 3, "inputId": 0 }
            ],
            "version": "2.6.0"
        });
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

        let data = load(&path, &AtomicBool::new(false));
        let doc = data.vcv.as_ref().expect("vcvss document");
        assert_eq!(data.detail, "3 modules · 2 cables · 0 rows");
        assert!(doc.summary.ranked_strip.width > 0.0);

        let viewer = Viewer::load(&path, &AtomicBool::new(false)).expect("viewer load vcvss");
        assert_eq!(viewer.artwork.rack.modules.len(), 3);
        assert_eq!(viewer.analysis.document.row_count(), 0);
    }

    #[test]
    fn hover_retires_on_exit_and_reports_terminal_render_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.vcv");
        std::fs::write(&path, sample_vcv_bytes()).unwrap();
        let data = load(&path, &AtomicBool::new(false));
        let doc = data.vcv.unwrap();
        let ctx = egui::Context::default();
        let mut hover = Hover::default();
        let row = uuid::Uuid::new_v4();
        let request = || {
            Source::VcvRaster(RasterRequest {
                summary: Arc::clone(&doc.summary),
                artwork: Artwork::Rack(f32::NAN),
                dark: true,
            })
        };
        let error = wait_for(|| {
            hover.poll(&ctx);
            hover.rack.request(row, 0, 0, request()).map(|ready| {
                assert!(ready.texture.is_none());
                ready.data.detail.clone()
            })
        });
        assert!(error.contains("scale"));
        let visible = BTreeSet::from([row]);
        hover.retain(&visible);
        hover.retain(&visible);
        assert!(hover.rack.request(row, 0, 0, request()).is_none());
    }

    #[test]
    fn wide_rack_paints_only_screws_in_the_viewport() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sparse.vcv");
        std::fs::write(&path, br#"{"modules":[{"id":1,"plugin":"Test","model":"Panel","pos":[-16000,0]},{"id":2,"plugin":"Test","model":"Panel","pos":[16000,0]}]}"#).unwrap();
        let viewer = Viewer::load(&path, &AtomicBool::new(false)).unwrap();
        let clip = Rect::from_min_size(Pos2::ZERO, vec2(700.0, 400.0));
        let output = clip_ui::test_support::run_frame(
            &egui::Context::default(),
            egui::RawInput::default(),
            |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        paint_visible_rails(
                            &ui.painter().with_clip_rect(clip),
                            &viewer.artwork.rack,
                            pos2(-200_000.0, 0.0),
                            1.0,
                            clip,
                        );
                    })
                });
            },
        );
        let screws = output
            .shapes
            .iter()
            .filter(|shape| matches!(shape.shape, egui::Shape::Circle(_)))
            .count();
        assert!((40..60).contains(&screws), "{screws} screw shapes");
    }

    #[test]
    fn single_row_strip_shows_full_patch_order_only_when_it_fits() {
        let dir = tempfile::tempdir().unwrap();
        let single_path = dir.path().join("single.vcvss");
        let multi_path = dir.path().join("multi.vcv");

        let single_json = serde_json::json!({
            "stripVersion": 1,
            "leftModules": [
                { "id": 2, "plugin": "Fundamental", "model": "VCO" },
                { "id": 1, "plugin": "Fundamental", "model": "VCF" }
            ],
            "rightModules": [
                { "id": 3, "plugin": "Fundamental", "model": "Scope" },
                { "id": 4, "plugin": "Fundamental", "model": "VCO" }
            ],
            "cables": [],
            "version": "2.6.0"
        });
        let multi_json = serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Fundamental", "model": "VCO", "pos": [0, 0] },
                { "id": 2, "plugin": "Fundamental", "model": "VCF", "pos": [10, 0] },
                { "id": 3, "plugin": "Fundamental", "model": "Scope", "pos": [0, 1] },
                { "id": 4, "plugin": "Fundamental", "model": "VCO", "pos": [10, 1] }
            ],
            "cables": []
        });
        std::fs::write(&single_path, serde_json::to_vec(&single_json).unwrap()).unwrap();
        std::fs::write(&multi_path, serde_json::to_vec(&multi_json).unwrap()).unwrap();

        let single_data = load(&single_path, &AtomicBool::new(false));
        let single_doc = single_data.vcv.as_ref().expect("single document");
        let multi_data = load(&multi_path, &AtomicBool::new(false));
        let multi_doc = multi_data.vcv.as_ref().expect("multi document");

        // When 1 row fits horizontally, full patch order (including duplicate VCOs) is chosen.
        let (wide_lineup, wide_full) = strip_for_preview(&single_doc.summary, vec2(2000.0, 66.0));
        assert!(wide_full);
        assert_eq!(
            wide_lineup
                .modules
                .iter()
                .map(|m| m.model.as_str())
                .collect::<Vec<_>>(),
            ["VCF", "VCO", "Scope", "VCO"]
        );

        // When too narrow, it falls back to the deduplicated ranked lineup.
        let (narrow_lineup, narrow_full) = strip_for_preview(&single_doc.summary, vec2(80.0, 66.0));
        assert!(!narrow_full);
        assert_eq!(narrow_lineup.modules.len(), 3);

        // Multi-row patches always stay on the deduplicated ranked lineup regardless of width.
        let (_multi_lineup, multi_full) = strip_for_preview(&multi_doc.summary, vec2(4000.0, 66.0));
        assert!(!multi_full);

        // Ranked and full strip variants maintain distinct texture entries.
        let ctx = egui::Context::default();
        let mut hover = Hover::default();
        let id = uuid::Uuid::new_v4();
        let full_tex = wait_for(|| {
            hover.poll(&ctx);
            hover.strip_texture(id, single_doc, true, true)
        });
        let ranked_tex = wait_for(|| {
            hover.poll(&ctx);
            hover.strip_texture(id, single_doc, true, false)
        });
        assert_ne!(full_tex, ranked_tex);
        assert_eq!(hover.strip_ids.len(), 2);
        hover.retain(&BTreeSet::new());
        assert!(hover.strip_ids.is_empty());
    }
}
