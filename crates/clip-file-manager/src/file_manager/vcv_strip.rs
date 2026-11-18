//! VCV Rack / Cardinal (`.vcv`) preview strip (importance-ranked unique modules) and
//! full Rack rail pan/zoom preview (modules placed at actual saved coordinates on rails).

use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, shell::StudioShell, ui_appearance::WorkbenchColors};
use egui::{Color32, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2};
use little_clip_core::{
    ImageThumbnail,
    palette::AppPalette,
    vcv::{
        MAX_PATCH_BYTES, PatchSummary, RACK_HP_WIDTH, RACK_ROW_HEIGHT, RenderedDiagram,
        ScreenshotBatchResult, StripLineup, build_rack_rail_layout, clear_downloaded_panel_cache,
        count_missing_local_screenshot_plugins, ensure_local_screenshots_for_patch,
        generate_all_local_rack_screenshots, inspect_path_bytes, local_rack_available,
        render_full_strip_rgba, render_rack_rails_rgba, render_strip_rgba, resolve_panel,
    },
};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

const MAX_VCV_BYTES: usize = MAX_PATCH_BYTES as usize;
const VIEWER_MIN_ZOOM: f32 = 0.15;
const VIEWER_MAX_ZOOM: f32 = 3.0;
const STRIP_RASTER_SCALE: f32 = 2.0;
const MAX_FULL_STRIP_WIDTH: f64 = 4096.0;
const MAX_HOVER_CACHE: usize = 8;

fn thumbnail_from_diagram(rendered: RenderedDiagram) -> ImageThumbnail {
    ImageThumbnail {
        width: rendered.width,
        height: rendered.height,
        premultiplied_rgba: rendered.rgba,
    }
}

pub(in crate::file_manager) struct Document {
    pub summary: Arc<PatchSummary>,
    strip_dark: ImageThumbnail,
    strip_light: Mutex<Option<ImageThumbnail>>,
    full_strip_dark: Mutex<Option<ImageThumbnail>>,
    full_strip_light: Mutex<Option<ImageThumbnail>>,
}

impl Document {
    fn cached_variant(
        &self,
        slot: &Mutex<Option<ImageThumbnail>>,
        render: impl FnOnce() -> Result<RenderedDiagram, String>,
    ) -> ImageThumbnail {
        let mut guard = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = guard.as_ref() {
            return existing.clone();
        }
        let thumb = render()
            .map(thumbnail_from_diagram)
            .unwrap_or_else(|_| self.strip_dark.clone());
        *guard = Some(thumb.clone());
        thumb
    }

    fn light_strip(&self) -> ImageThumbnail {
        self.cached_variant(&self.strip_light, || {
            render_strip_rgba(&self.summary, STRIP_RASTER_SCALE, false)
        })
    }

    fn strip_thumbnail(&self, dark: bool, full: bool) -> std::borrow::Cow<'_, ImageThumbnail> {
        match (dark, full) {
            (true, false) => std::borrow::Cow::Borrowed(&self.strip_dark),
            (false, false) => std::borrow::Cow::Owned(self.light_strip()),
            (true, true) => {
                std::borrow::Cow::Owned(self.cached_variant(&self.full_strip_dark, || {
                    render_full_strip_rgba(&self.summary, STRIP_RASTER_SCALE, true).unwrap_or_else(
                        || render_strip_rgba(&self.summary, STRIP_RASTER_SCALE, true),
                    )
                }))
            }
            (false, true) => {
                std::borrow::Cow::Owned(self.cached_variant(&self.full_strip_light, || {
                    render_full_strip_rgba(&self.summary, STRIP_RASTER_SCALE, false).unwrap_or_else(
                        || render_strip_rgba(&self.summary, STRIP_RASTER_SCALE, false),
                    )
                }))
            }
        }
    }
}

type HoverKey = (uuid::Uuid, bool, u32, u32);

struct HoverCacheEntry {
    key: HoverKey,
    path: PathBuf,
    texture: TextureHandle,
    popup_size: egui::Vec2,
}

struct HoverJob {
    generation: u64,
    key: HoverKey,
    path: PathBuf,
    popup_size: egui::Vec2,
    summary: Arc<PatchSummary>,
    render_scale: f32,
    dark: bool,
    ctx: egui::Context,
    cancel: Arc<AtomicBool>,
}

struct HoverResult {
    generation: u64,
    key: HoverKey,
    path: PathBuf,
    popup_size: egui::Vec2,
    rendered: RenderedDiagram,
}

#[derive(Default)]
struct HoverSlot {
    pending: Option<HoverJob>,
    shutdown: bool,
}

struct HoverWorker {
    slot: Arc<(Mutex<HoverSlot>, Condvar)>,
    rx: mpsc::Receiver<HoverResult>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl HoverWorker {
    fn new() -> Self {
        let slot = Arc::new((Mutex::new(HoverSlot::default()), Condvar::new()));
        let (tx, rx) = mpsc::sync_channel::<HoverResult>(4);
        let worker_slot = Arc::clone(&slot);
        let handle = std::thread::Builder::new()
            .name("vcv-hover-preview".into())
            .spawn(move || {
                loop {
                    let job = {
                        let (lock, cv) = &*worker_slot;
                        let mut guard = lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        while guard.pending.is_none() && !guard.shutdown {
                            guard = cv
                                .wait(guard)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                        if guard.shutdown {
                            return;
                        }
                        guard.pending.take()
                    };
                    let Some(job) = job else {
                        continue;
                    };
                    if job.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let rack = if job.dark {
                        &job.summary.rack_dark
                    } else {
                        &job.summary.rack_light
                    };
                    let Ok(rendered) = render_rack_rails_rgba(rack, job.render_scale, job.dark)
                    else {
                        continue;
                    };
                    if job.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let _ = tx.send(HoverResult {
                        generation: job.generation,
                        key: job.key,
                        path: job.path,
                        popup_size: job.popup_size,
                        rendered,
                    });
                    job.ctx.request_repaint();
                }
            })
            .ok();
        Self { slot, rx, handle }
    }

    fn enqueue(&self, job: HoverJob) {
        let (lock, cv) = &*self.slot;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(old) = guard.pending.take() {
            old.cancel.store(true, Ordering::Relaxed);
        }
        guard.pending = Some(job);
        cv.notify_one();
    }
}

impl Drop for HoverWorker {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.slot;
            let mut guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.shutdown = true;
            if let Some(old) = guard.pending.take() {
                old.cancel.store(true, Ordering::Relaxed);
            }
            cv.notify_all();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
pub(in crate::file_manager) struct Hover {
    generation: u64,
    entries: Vec<HoverCacheEntry>,
    in_flight: Option<(HoverKey, Arc<AtomicBool>)>,
    worker: Option<HoverWorker>,
    strip_textures: HashMap<(uuid::Uuid, bool, bool), (PathBuf, TextureHandle)>,
}

impl Hover {
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
        if let Some((_, cancel)) = self.in_flight.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.strip_textures.clear();
    }

    pub fn retain(&mut self, visible: &BTreeSet<PathBuf>) {
        self.entries.retain(|entry| visible.contains(&entry.path));
        self.strip_textures
            .retain(|_, (path, _)| visible.contains(path));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.worker else {
            return;
        };
        while let Ok(res) = worker.rx.try_recv() {
            if res.generation != self.generation {
                continue;
            }
            if self
                .in_flight
                .as_ref()
                .is_some_and(|(key, _)| *key == res.key)
            {
                self.in_flight = None;
            }
            let (id, dark, w_bucket, h_bucket) = res.key;
            let texture = ctx.load_texture(
                format!("vcv_hover_{id}_{dark}_{w_bucket}_{h_bucket}"),
                egui::ColorImage::from_rgba_premultiplied(
                    [res.rendered.width as usize, res.rendered.height as usize],
                    &res.rendered.rgba,
                ),
                TextureOptions::LINEAR,
            );
            self.entries.retain(|entry| entry.key != res.key);
            if self.entries.len() >= MAX_HOVER_CACHE {
                self.entries.remove(0);
            }
            self.entries.push(HoverCacheEntry {
                key: res.key,
                path: res.path,
                texture,
                popup_size: res.popup_size,
            });
        }
    }

    fn cached_entry(&mut self, key: HoverKey) -> Option<(egui::TextureId, egui::Vec2)> {
        let idx = self.entries.iter().position(|entry| entry.key == key)?;
        if idx + 1 < self.entries.len() {
            let entry = self.entries.remove(idx);
            self.entries.push(entry);
        }
        let latest = self.entries.last()?;
        Some((latest.texture.id(), latest.popup_size))
    }

    #[allow(clippy::too_many_arguments)]
    fn strip_texture(
        &mut self,
        ctx: &egui::Context,
        identity: uuid::Uuid,
        entry_path: &Path,
        doc: &Document,
        dark: bool,
        use_full: bool,
        ready: Option<&Ready>,
    ) -> Option<TextureHandle> {
        if dark
            && !use_full
            && let Some(tex) = ready.and_then(|r| r.texture.as_ref())
        {
            return Some(tex.clone());
        }
        if let Some((_, tex)) = self.strip_textures.get(&(identity, dark, use_full)) {
            return Some(tex.clone());
        }
        let thumb = doc.strip_thumbnail(dark, use_full);
        let handle = ctx.load_texture(
            format!("vcv_strip_{identity}_{dark}_{use_full}"),
            egui::ColorImage::from_rgba_premultiplied(
                [thumb.width as usize, thumb.height as usize],
                &thumb.premultiplied_rgba,
            ),
            TextureOptions::LINEAR,
        );
        self.strip_textures.insert(
            (identity, dark, use_full),
            (entry_path.to_path_buf(), handle.clone()),
        );
        Some(handle)
    }

    pub fn show(
        &mut self,
        ui: &Ui,
        entry_path: &Path,
        body: Rect,
        identity: uuid::Uuid,
        doc: &Document,
        dark: bool,
    ) {
        self.poll(ui.ctx());
        let screen = ui.ctx().screen_rect().shrink(8.0);
        let space_below = (screen.bottom() - body.bottom() - 8.0).max(0.0);
        let space_above = (body.top() - 8.0 - screen.top()).max(0.0);
        let place_below = space_below >= 240.0 || space_below >= space_above;
        let avail_h = if place_below {
            space_below
        } else {
            space_above
        }
        .clamp(160.0, screen.height().max(160.0));
        let avail_w = (screen.width() - 16.0).clamp(320.0, 1440.0);

        let w_bucket = (avail_w / 32.0).round() as u32 * 32;
        let h_bucket = (avail_h / 24.0).round() as u32 * 24;
        let key: HoverKey = (identity, dark, w_bucket, h_bucket);

        let rack = if dark {
            &doc.summary.rack_dark
        } else {
            &doc.summary.rack_light
        };
        let natural = vec2(rack.width as f32, rack.height as f32);
        let fit = (avail_w / natural.x.max(1.0))
            .min(avail_h / natural.y.max(1.0))
            .min(1.0);
        let popup_size = natural * fit;

        let cached = self.cached_entry(key);
        if cached.is_none()
            && !self
                .in_flight
                .as_ref()
                .is_some_and(|(active_key, _)| *active_key == key)
        {
            if let Some((_, old_cancel)) = self.in_flight.take() {
                old_cancel.store(true, Ordering::Relaxed);
            }
            let cancel = Arc::new(AtomicBool::new(false));
            self.in_flight = Some((key, Arc::clone(&cancel)));
            let dpr = ui.ctx().pixels_per_point().clamp(1.0, 2.0);
            let render_scale = (fit * dpr).clamp(0.25, 2.0);
            let worker = self.worker.get_or_insert_with(HoverWorker::new);
            worker.enqueue(HoverJob {
                generation: self.generation,
                key,
                path: entry_path.to_path_buf(),
                popup_size,
                summary: Arc::clone(&doc.summary),
                render_scale,
                dark,
                ctx: ui.ctx().clone(),
                cancel,
            });
        }

        if super::ui::popup_open(ui.ctx()) {
            return;
        }
        let shown_size = cached.map_or(popup_size, |(_, size)| size);
        let left = body.left().clamp(
            screen.left(),
            (screen.right() - shown_size.x).max(screen.left()),
        );
        let top = if place_below {
            (body.bottom() + 6.0).min((screen.bottom() - shown_size.y).max(screen.top()))
        } else {
            (body.top() - 6.0 - shown_size.y).max(screen.top())
        };

        egui::Area::new(egui::Id::new(("vcv_strip_hover", identity)))
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
                if let Some((tid, _)) = cached {
                    ui.painter().image(
                        tid,
                        rect,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Rendering rack preview…",
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(180, 186, 198),
                    );
                }
                ui.painter()
                    .rect_stroke(rect.expand(2.0), 6.0, egui::Stroke::new(1.0_f32, border));
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
    let summary = match inspect_path_bytes(path, &bytes) {
        Ok(s) => s,
        Err(err) => {
            data.detail = err;
            return data;
        }
    };
    if token.load(Ordering::Relaxed) {
        data.detail = "Cancelled".into();
        return data;
    }

    let dark_render = match render_strip_rgba(&summary, STRIP_RASTER_SCALE, true) {
        Ok(r) => r,
        Err(err) => {
            data.detail = err;
            return data;
        }
    };
    let strip_dark = thumbnail_from_diagram(dark_render);
    let full_strip_dark = if summary
        .full_strip
        .as_ref()
        .is_some_and(|full| full.width <= MAX_FULL_STRIP_WIDTH)
    {
        if token.load(Ordering::Relaxed) {
            data.detail = "Cancelled".into();
            return data;
        }
        render_full_strip_rgba(&summary, STRIP_RASTER_SCALE, true)
            .and_then(Result::ok)
            .map(thumbnail_from_diagram)
    } else {
        None
    };

    data.detail = format_summary_detail(&summary);
    data.image = Some(strip_dark.clone());
    data.vcv = Some(Arc::new(Document {
        summary: Arc::new(summary),
        strip_dark,
        strip_light: Mutex::new(None),
        full_strip_dark: Mutex::new(full_strip_dark),
        full_strip_light: Mutex::new(None),
    }));
    data
}

fn format_summary_detail(summary: &PatchSummary) -> String {
    let mod_word = if summary.total_modules == 1 {
        "module"
    } else {
        "modules"
    };
    let cable_word = if summary.cable_count == 1 {
        "cable"
    } else {
        "cables"
    };
    let rows = summary.analysis.document.row_count();
    let row_word = if rows == 1 { "row" } else { "rows" };
    format!(
        "{} {mod_word} · {} {cable_word} · {rows} {row_word}",
        summary.total_modules, summary.cable_count
    )
}

fn strip_for_preview(
    summary: &PatchSummary,
    body_size: egui::Vec2,
    dark: bool,
) -> (&StripLineup, bool) {
    if let Some(full) = summary.full_strip.as_ref() {
        let natural = vec2(full.width as f32, full.height as f32);
        let scale = (body_size.y / natural.y.max(1.0)).min(1.0);
        if natural.x * scale <= body_size.x {
            return (full, true);
        }
    }
    let ranked = if dark {
        &summary.strip_dark
    } else {
        &summary.strip_light
    };
    (ranked, false)
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
) -> Option<usize> {
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
        return None;
    };

    let dark = ui.visuals().dark_mode;
    let (lineup, use_full) = strip_for_preview(&doc.summary, body.size(), dark);
    let natural = vec2(lineup.width as f32, lineup.height as f32);
    let scale = (body.height() / natural.y.max(1.0)).min(1.0);
    let scaled = natural * scale;
    let visible_w = scaled.x.min(body.width());
    let uv_w = (visible_w / scaled.x.max(1.0)).clamp(0.0, 1.0);
    let pan_u = if uv_w < 1.0
        && super::ui::preview_hovered(ui, body)
        && let Some(pos) = ui.ctx().pointer_hover_pos()
    {
        let frac = ((pos.x - body.left()) / body.width().max(1.0)).clamp(0.0, 1.0);
        frac * (1.0 - uv_w)
    } else {
        0.0
    };
    let top = body.top() + (body.height() - scaled.y).max(0.0) * 0.5;
    let clip_rect = body.intersect(ui.clip_rect());

    if let Some(tex) =
        hover.strip_texture(ui.ctx(), identity, &entry.path, doc, dark, use_full, ready)
    {
        let draw_rect = Rect::from_min_size(pos2(body.left(), top), vec2(visible_w, scaled.y));
        let uv = Rect::from_min_max(pos2(pan_u, 0.0), pos2((pan_u + uv_w).min(1.0), 1.0));
        ui.painter()
            .with_clip_rect(clip_rect)
            .image(tex.id(), draw_rect, uv, Color32::WHITE);
    }

    if super::ui::preview_hovered(ui, body) {
        hover.show(ui, &entry.path, body, identity, doc, dark);
    }

    None
}

fn texture_from_rgba(ctx: &egui::Context, name: &str, rgba: &image::RgbaImage) -> TextureHandle {
    let size = [rgba.width() as usize, rgba.height() as usize];
    let img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    ctx.load_texture(name, img, TextureOptions::LINEAR)
}

enum ScreenshotJobMsg {
    Progress {
        done: usize,
        total: usize,
        rendered_modules: usize,
    },
    Finished(ScreenshotBatchResult),
}

struct ScreenshotJob {
    rx: mpsc::Receiver<ScreenshotJobMsg>,
    cancel: Arc<AtomicBool>,
    done: usize,
    total: usize,
    rendered_modules: usize,
    is_full_library: bool,
}

/// Interactive VCV Rack rail pan/zoom viewer for opening `.vcv` files cleanly.
pub(in crate::file_manager) struct Viewer {
    summary: PatchSummary,
    zoom: f32,
    pan: egui::Vec2,
    fitted_once: bool,
    unique_modules: Vec<(String, String)>,
    panel_rasters: HashMap<(String, String), Option<Arc<image::RgbaImage>>>,
    panel_textures: HashMap<(String, String), Option<TextureHandle>>,
    visible_buf: Vec<(usize, (String, String), Rect)>,
    has_local_rack: bool,
    missing_local_plugins: usize,
    screenshot_job: Option<ScreenshotJob>,
    screenshot_status: Option<String>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        if let Some(job) = self.screenshot_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = crate::file_source::read_file_snapshot(path, MAX_VCV_BYTES)?;
        let summary = inspect_path_bytes(path, &bytes)?;
        let mut unique_modules = Vec::new();
        let mut seen = BTreeSet::new();
        for m in &summary.rack_dark.modules {
            let key = (m.plugin.clone(), m.model.clone());
            if seen.insert(key.clone()) {
                unique_modules.push(key);
            }
        }
        let has_local_rack = local_rack_available();
        let mut viewer = Self {
            summary,
            zoom: 0.65,
            pan: egui::Vec2::ZERO,
            fitted_once: false,
            unique_modules,
            panel_rasters: HashMap::new(),
            panel_textures: HashMap::new(),
            visible_buf: Vec::new(),
            has_local_rack,
            missing_local_plugins: 0,
            screenshot_job: None,
            screenshot_status: None,
        };
        viewer.reload_panels();
        Ok(viewer)
    }

    fn reload_panels(&mut self) {
        self.panel_rasters.clear();
        self.panel_textures.clear();
        self.summary.rack_dark = build_rack_rail_layout(&self.summary.analysis, true);
        self.summary.rack_light = build_rack_rail_layout(&self.summary.analysis, false);
        self.missing_local_plugins = if self.has_local_rack {
            count_missing_local_screenshot_plugins(&self.unique_modules)
        } else {
            0
        };

        for (plugin, model) in &self.unique_modules {
            let rgba = resolve_panel(plugin, model).map(|panel| Arc::clone(&panel.rgba));
            self.panel_rasters
                .insert((plugin.clone(), model.clone()), rgba);
        }
    }

    fn start_patch_screenshot_job(&mut self, ctx: &egui::Context) {
        if self.screenshot_job.is_some() {
            return;
        }
        let modules = self.unique_modules.clone();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_ctx = ctx.clone();
        self.screenshot_job = Some(ScreenshotJob {
            rx,
            cancel,
            done: 0,
            total: count_missing_local_screenshot_plugins(&modules),
            rendered_modules: 0,
            is_full_library: false,
        });
        let _ = std::thread::Builder::new()
            .name("vcv-local-screenshots".into())
            .spawn(move || {
                let res = ensure_local_screenshots_for_patch(&modules, &worker_cancel);
                let _ = tx.send(ScreenshotJobMsg::Finished(res));
                worker_ctx.request_repaint();
            });
    }

    fn start_all_plugins_screenshot_job(&mut self, ctx: &egui::Context) {
        if self.screenshot_job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_ctx = ctx.clone();
        self.screenshot_job = Some(ScreenshotJob {
            rx,
            cancel,
            done: 0,
            total: 0,
            rendered_modules: 0,
            is_full_library: true,
        });
        let _ = std::thread::Builder::new()
            .name("vcv-all-screenshots".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let progress_ctx = worker_ctx.clone();
                let res = generate_all_local_rack_screenshots(
                    &worker_cancel,
                    |done, total, rendered_modules| {
                        let _ = progress_tx.send(ScreenshotJobMsg::Progress {
                            done,
                            total,
                            rendered_modules,
                        });
                        progress_ctx.request_repaint();
                    },
                );
                let _ = tx.send(ScreenshotJobMsg::Finished(res));
                worker_ctx.request_repaint();
            });
    }

    fn poll_screenshot_job(&mut self) {
        let mut finished: Option<ScreenshotBatchResult> = None;
        let mut should_reload = false;
        if let Some(job) = self.screenshot_job.as_mut() {
            while let Ok(msg) = job.rx.try_recv() {
                match msg {
                    ScreenshotJobMsg::Progress {
                        done,
                        total,
                        rendered_modules,
                    } => {
                        if rendered_modules > job.rendered_modules {
                            should_reload = true;
                        }
                        job.done = done;
                        job.total = total;
                        job.rendered_modules = rendered_modules;
                    }
                    ScreenshotJobMsg::Finished(res) => {
                        if res.rendered_modules > 0 {
                            should_reload = true;
                        }
                        finished = Some(res);
                        break;
                    }
                }
            }
        }
        if let Some(res) = finished {
            let was_full = self
                .screenshot_job
                .as_ref()
                .is_some_and(|j| j.is_full_library);
            self.screenshot_job = None;
            self.screenshot_status = Some(if res.cancelled {
                format!("Cancelled ({} panels captured)", res.rendered_modules)
            } else if res.rendered_modules > 0 {
                format!(
                    "Captured {} local panels ({} plugins)",
                    res.rendered_modules, res.rendered_plugins
                )
            } else if was_full {
                format!(
                    "Local screenshots up to date ({} plugins checked)",
                    res.skipped_plugins
                )
            } else if !res.failed_plugins.is_empty() {
                format!("Rack could not render {}", res.failed_plugins.join(", "))
            } else {
                "Using local Rack screenshots".to_string()
            });
        }
        if should_reload {
            self.reload_panels();
        }
    }

    fn ensure_texture(
        &mut self,
        ctx: &egui::Context,
        key: &(String, String),
    ) -> Option<egui::TextureId> {
        if let Some(cached) = self.panel_textures.get(key) {
            return cached.as_ref().map(|t| t.id());
        }
        let tex = self
            .panel_rasters
            .get(key)
            .and_then(|opt| opt.as_ref())
            .map(|rgba| texture_from_rgba(ctx, &format!("vcv_mod_{}_{}", key.0, key.1), rgba));
        let id = tex.as_ref().map(|t| t.id());
        self.panel_textures.insert(key.clone(), tex);
        id
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        let dark = ui.visuals().dark_mode;
        self.poll_screenshot_job();

        StudioShell::new("VCV Rack", Icon::Midi, palette, close).render_header(ui, |ui| {
            ui.label(format!(
                "{} · {}",
                self.summary.title,
                format_summary_detail(&self.summary)
            ));
            ui.separator();
            if ui.button("−").clicked() {
                self.zoom = (self.zoom * 0.8).clamp(VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            if ui.button("+").clicked() {
                self.zoom = (self.zoom * 1.25).clamp(VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
            }
            if ui.button("Fit Rack").clicked() {
                self.fitted_once = false;
            }
            if ui.button("Clear Downloaded Cache").clicked() {
                let _ = clear_downloaded_panel_cache();
                self.screenshot_status = None;
                self.reload_panels();
            }
            if self.has_local_rack {
                ui.separator();
                if let Some(job) = &self.screenshot_job {
                    let msg = if job.total > 0 {
                        format!(
                            "Capturing {} ({}/{})…",
                            if job.is_full_library {
                                "all plugins"
                            } else {
                                "patch plugins"
                            },
                            job.done,
                            job.total
                        )
                    } else {
                        "Capturing local Rack screenshots…".to_string()
                    };
                    ui.label(msg);
                    if ui.button("Cancel Capture").clicked() {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                } else {
                    if self.missing_local_plugins > 0
                        && ui
                            .button(format!(
                                "Generate Patch Screenshots ({})",
                                self.missing_local_plugins
                            ))
                            .on_hover_text(
                                "Opens VCV Rack briefly to render local PNGs for missing patch plugins",
                            )
                            .clicked()
                    {
                        self.start_patch_screenshot_job(ui.ctx());
                    }
                    if ui
                        .button("Generate All Local Screenshots")
                        .on_hover_text(
                            "Runs VCV Rack -t 4 in batches to capture all installed plugins without screenshots",
                        )
                        .clicked()
                    {
                        self.start_all_plugins_screenshot_job(ui.ctx());
                    }
                    if let Some(status) = &self.screenshot_status {
                        ui.small(status);
                    }
                }
            }
        });

        let rack = if dark {
            &self.summary.rack_dark
        } else {
            &self.summary.rack_light
        };

        let viewport = ui.available_rect_before_wrap();
        if !self.fitted_once && viewport.width() > 32.0 && viewport.height() > 32.0 {
            let fit_w = (viewport.width() - 24.0) / rack.width.max(1.0) as f32;
            let fit_h = (viewport.height() - 24.0) / rack.height.max(1.0) as f32;
            self.zoom = fit_w.min(fit_h).clamp(VIEWER_MIN_ZOOM, 1.5);
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
                let new_zoom = (old_zoom * factor).clamp(VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
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

        let rail_x = 12.0 * self.zoom;
        let rail_w = ((rack.width - 24.0) as f32 * self.zoom).max(16.0);
        let row_h = RACK_ROW_HEIGHT as f32 * self.zoom;
        let rail_h = (15.0 * self.zoom).max(2.0);
        for &(_, ry) in &rack.rows {
            let row_rect = Rect::from_min_size(
                origin + vec2(rail_x, ry as f32 * self.zoom),
                vec2(rail_w, row_h),
            );
            painter.rect_filled(row_rect, 0.0, Color32::from_rgb(24, 26, 32));
            let top_rail = Rect::from_min_size(row_rect.min, vec2(rail_w, rail_h));
            let bot_rail = Rect::from_min_size(
                pos2(row_rect.left(), row_rect.bottom() - rail_h),
                vec2(rail_w, rail_h),
            );
            painter.rect_filled(top_rail, 0.0, Color32::from_rgb(56, 60, 70));
            painter.rect_filled(bot_rail, 0.0, Color32::from_rgb(56, 60, 70));

            let hp_step = (RACK_HP_WIDTH as f32 * 2.0 * self.zoom).max(8.0);
            let mut sx = row_rect.left() + hp_step * 0.5;
            while sx < row_rect.right() - 4.0 {
                painter.circle_filled(
                    pos2(sx, top_rail.center().y),
                    (2.0 * self.zoom).clamp(0.8, 3.0),
                    Color32::from_rgb(24, 26, 31),
                );
                painter.circle_filled(
                    pos2(sx, bot_rail.center().y),
                    (2.0 * self.zoom).clamp(0.8, 3.0),
                    Color32::from_rgb(24, 26, 31),
                );
                sx += hp_step;
            }
        }

        let hover_pos = response
            .hovered()
            .then(|| ui.ctx().pointer_hover_pos())
            .flatten();
        self.visible_buf.clear();

        for (idx, m) in rack.modules.iter().enumerate() {
            let mod_rect = Rect::from_min_size(
                origin + vec2(m.x as f32, m.y as f32) * self.zoom,
                vec2(m.width as f32, m.height as f32) * self.zoom,
            );
            if !mod_rect.intersects(viewport) {
                continue;
            }
            self.visible_buf
                .push((idx, (m.plugin.clone(), m.model.clone()), mod_rect));
        }

        for i in 0..self.visible_buf.len() {
            let (idx, ref key, mod_rect) = self.visible_buf[i];
            let key_owned = key.clone();
            let tid = self.ensure_texture(ui.ctx(), &key_owned);
            let rack = if dark {
                &self.summary.rack_dark
            } else {
                &self.summary.rack_light
            };
            paint_texture_or_placeholder(&painter, &rack.modules[idx], mod_rect, self.zoom, tid);
        }

        let rack = if dark {
            &self.summary.rack_dark
        } else {
            &self.summary.rack_light
        };
        let mut hovered_idx = None;
        for &(idx, _, mod_rect) in &self.visible_buf {
            painter.rect_stroke(
                mod_rect,
                0.0,
                egui::Stroke::new(1.0_f32, Color32::from_rgb(12, 13, 16)),
            );
            if hover_pos.is_some_and(|p| mod_rect.contains(p)) {
                painter.rect_stroke(
                    mod_rect,
                    1.0,
                    egui::Stroke::new(2.0_f32, Color32::from_rgb(255, 212, 82)),
                );
                hovered_idx = Some(idx);
            }
        }

        if let Some(idx) = hovered_idx {
            let m = &rack.modules[idx];
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("vcv_viewer_module_tip", m.id)),
                |ui| {
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
                },
            );
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

    #[test]
    fn loads_ranked_strip_rack_rails_and_hover_without_panics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.vcv");
        std::fs::write(&path, sample_vcv_bytes()).unwrap();

        let data = load(&path, &AtomicBool::new(false));
        let doc = data.vcv.as_ref().expect("vcv document");
        assert!(data.detail.contains("module"));
        assert!(data.detail.contains("row"));
        assert!(doc.strip_dark.width > 0);
        let light = doc.light_strip();
        assert!(light.width > 0);

        let ctx = egui::Context::default();
        let mut hover = Hover::default();
        let row_id = uuid::Uuid::new_v4();
        let rect = Rect::from_min_size(pos2(10.0, 40.0), vec2(760.0, 66.0));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                hover.show(ui, &path, rect, row_id, doc, true);
            });
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hover.entries.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    hover.show(ui, &path, rect, row_id, doc, true);
                });
            });
        }
        assert!(
            !hover.entries.is_empty(),
            "background hover worker must populate LRU texture cache"
        );

        let mut viewer = Viewer::load(&path).expect("viewer load");
        assert_eq!(viewer.summary.rack_dark.modules.len(), 6);
        let palette = AppPalette::default();
        let mut close = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                viewer.ui(ui, &mut close, &palette);
            });
        });
        assert!(!close);
        assert!(viewer.fitted_once);
        assert!(viewer.zoom <= VIEWER_MAX_ZOOM);

        // Verify clicking zoom `+` caps at VIEWER_MAX_ZOOM (300%).
        viewer.zoom = 2.8;
        for _ in 0..4 {
            viewer.zoom = (viewer.zoom * 1.25).clamp(VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
        }
        assert!((viewer.zoom - 3.0).abs() < 1e-4);

        if let Ok(home) = std::env::var("HOME") {
            let dir = Path::new(&home).join("Pictures/vcv");
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let fpath = entry.path();
                    if fpath.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("vcv") || e.eq_ignore_ascii_case("vcvss")
                    }) {
                        let data = load(&fpath, &AtomicBool::new(false));
                        assert!(
                            data.vcv.is_some(),
                            "failed load for {}: {}",
                            fpath.display(),
                            data.detail
                        );
                    }
                }
            }
        }
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
        assert!(doc.strip_dark.width > 0);

        let viewer = Viewer::load(&path).expect("viewer load vcvss");
        assert_eq!(viewer.summary.rack_dark.modules.len(), 3);
        assert_eq!(viewer.summary.analysis.document.row_count(), 0);

        if let Ok(home) = std::env::var("HOME") {
            let demo = Path::new(&home)
                .join(".local/share/Rack2/plugins-lin-x64/Stoermelder-P1/presets/Strip/Demo.vcvss");
            if demo.is_file() {
                let demo_data = load(&demo, &AtomicBool::new(false));
                assert!(
                    demo_data.vcv.is_some(),
                    "Demo.vcvss load: {}",
                    demo_data.detail
                );
                assert_eq!(demo_data.detail, "1 module · 0 cables · 0 rows");
            }
        }
    }

    #[test]
    fn viewer_uses_single_texture_cache_and_hover_retains_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("big.vcv");
        let path_b = dir.path().join("other.vcv");
        let big_json = serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Fundamental", "model": "VCO", "pos": [0, 0] },
                { "id": 2, "plugin": "Fundamental", "model": "VCF", "pos": [10, 0] },
                { "id": 3, "plugin": "Fundamental", "model": "VCA-1", "pos": [18, 0] },
                { "id": 4, "plugin": "Fundamental", "model": "ADSR", "pos": [24, 0] },
                { "id": 5, "plugin": "Fundamental", "model": "LFO", "pos": [32, 0] },
                { "id": 6, "plugin": "Fundamental", "model": "Mixer", "pos": [42, 0] },
                { "id": 7, "plugin": "Fundamental", "model": "Scope", "pos": [50, 0] },
                { "id": 8, "plugin": "Fundamental", "model": "VCMixer", "pos": [62, 0] }
            ],
            "cables": []
        });
        std::fs::write(&path_a, serde_json::to_vec(&big_json).unwrap()).unwrap();
        std::fs::write(&path_b, sample_vcv_bytes()).unwrap();

        let data_a = load(&path_a, &AtomicBool::new(false));
        let doc_a = data_a.vcv.as_ref().unwrap();
        let data_b = load(&path_b, &AtomicBool::new(false));
        let doc_b = data_b.vcv.as_ref().unwrap();

        let ctx = egui::Context::default();
        let mut hover = Hover::default();
        let id_a = uuid::Uuid::new_v4();
        let id_b = uuid::Uuid::new_v4();
        let rect = Rect::from_min_size(pos2(10.0, 40.0), vec2(760.0, 66.0));
        let mut tick = |p: &Path, id: uuid::Uuid, d: &Document| {
            let start = hover.entries.len();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    hover.show(ui, p, rect, id, d, true);
                });
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while hover.entries.len() == start && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        hover.show(ui, p, rect, id, d, true);
                    });
                });
            }
        };
        tick(&path_a, id_a, doc_a);
        tick(&path_b, id_b, doc_b);
        assert_eq!(hover.entries.len(), 2);

        let visible: BTreeSet<PathBuf> = [path_a.clone()].into_iter().collect();
        hover.retain(&visible);
        assert_eq!(hover.entries.len(), 1);
        assert_eq!(hover.entries[0].path, path_a);
        hover.clear();
        assert!(hover.entries.is_empty());

        let mut viewer = Viewer::load(&path_a).unwrap();
        let palette = AppPalette::default();
        let mut close = false;
        viewer.fitted_once = true;
        viewer.zoom = 1.4;
        viewer.pan = egui::vec2(20.0, 20.0);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                viewer.ui(ui, &mut close, &palette);
            });
        });
        assert_eq!(viewer.panel_rasters.len(), 8);
        assert_eq!(viewer.panel_textures.len(), 8);
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
        let (wide_lineup, wide_full) =
            strip_for_preview(&single_doc.summary, vec2(2000.0, 66.0), true);
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
        let (narrow_lineup, narrow_full) =
            strip_for_preview(&single_doc.summary, vec2(80.0, 66.0), true);
        assert!(!narrow_full);
        assert_eq!(narrow_lineup.modules.len(), 3);

        // Multi-row patches always stay on the deduplicated ranked lineup regardless of width.
        let (_multi_lineup, multi_full) =
            strip_for_preview(&multi_doc.summary, vec2(4000.0, 66.0), true);
        assert!(!multi_full);

        // Ranked and full strip variants maintain distinct texture entries.
        let ctx = egui::Context::default();
        let mut hover = Hover::default();
        let id = uuid::Uuid::new_v4();
        let full_tex = hover
            .strip_texture(&ctx, id, &single_path, single_doc, true, true, None)
            .expect("full strip texture");
        let ranked_tex = hover
            .strip_texture(&ctx, id, &single_path, single_doc, true, false, None)
            .expect("ranked strip texture");
        assert_ne!(full_tex.id(), ranked_tex.id());
        assert_eq!(hover.strip_textures.len(), 2);
    }
}
