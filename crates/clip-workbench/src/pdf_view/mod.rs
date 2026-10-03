//! Multi-page PDF thumbnail grid viewer and page manipulation workbench view.
//!
//! Renders a two-pane layout: a left-side scrollable/wrapping page selector,
//! and a right-side large preview canvas of the selected page with live 90° rotation,
//! page deletion, and dirty state tracking.

mod editing;
mod navigation;
mod page_cache;
mod preview_jobs;
mod studio;

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::StatusToast;
use eframe::egui::{self, Rounding, Stroke, Vec2};
use little_clip_core::pdf::{
    PdfInfo, delete_pdf_pages, get_pdf_info, render_pdf_page_preview, rotate_pdf_page,
};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::Duration;

const PREVIEW_QUEUE_CAPACITY: usize = 3;
const PAGE_DECODE_REQUEST_LIMIT: usize = 32;
// Bound retained RGBA textures independently of document length. Compressed
// rasters remain available for re-upload when an evicted page becomes visible.
// Backend frees follow drawing; frame turnover can temporarily use twice these
// limits, because textures uploaded in this frame cannot be evicted until the next.
const PAGE_TEXTURE_LIMIT: usize = 32;
const PAGE_TEXTURE_BYTE_LIMIT: usize = 96 * 1024 * 1024;
const PAGE_RASTER_MAX_DIM: u32 = 1200;
const PAGE_DETAIL_MAX_DIM: u32 = 4096;
const STAGED_PDF_FILENAME: &str = "edited.pdf";

struct PdfPreviewMsg {
    page_num: usize,
    result: Result<Arc<PdfPageRasterResource>, String>,
}

// One running/queued decode or completed ColorImage per editor (at most
// 4096 * 4096 * 4 bytes of published pixels). Canceled jobs retain their slot
// until acknowledged so rapid navigation cannot accumulate running decodes.
struct PdfPageDecodeSlot {
    raster: Arc<PdfPageRasterResource>,
    ticket: JobTicket,
    rx: Receiver<Result<Result<(Arc<PdfPageRasterResource>, egui::ColorImage), String>, JobError>>,
    ready: Option<egui::ColorImage>,
    cancelled: bool,
}

enum PdfDecodePoll {
    Pending,
    Ready,
    AlreadyReady,
    Failed(String),
    QueueFull,
    Cancelled,
}

impl PdfPageDecodeSlot {
    fn start(
        raster: Arc<PdfPageRasterResource>,
        source: Option<(PathBuf, u32)>,
        ctx: &egui::Context,
    ) -> Self {
        let worker_raster = Arc::clone(&raster);
        let repaint = ctx.clone();
        let decode_limit = ctx
            .input(|input| input.max_texture_side)
            .min(PAGE_DETAIL_MAX_DIM as usize) as u32;
        let (ticket, rx) = BackgroundJobs::global().submit(Duration::from_secs(10), move || {
            let result = (|| {
                let raster = if let Some((source, render_width)) = source {
                    let (png, width, height) =
                        render_pdf_page_preview(&source, worker_raster.page_num(), render_width)?;
                    let path = worker_raster._raster_directory.path().join(format!(
                        "page-{}-{render_width}px.png",
                        worker_raster.page_num()
                    ));
                    std::fs::write(&path, png).map_err(|error| error.to_string())?;
                    Arc::new(PdfPageRasterResource {
                        page_num: worker_raster.page_num(),
                        path,
                        width,
                        height,
                        render_width,
                        _raster_directory: Arc::clone(&worker_raster._raster_directory),
                    })
                } else {
                    worker_raster
                };
                let image = std::fs::read(raster.path())
                    .map_err(|error| format!("Could not read cached PDF raster: {error}"))
                    .and_then(|png| decode_page_png(png, decode_limit))?;
                Ok((raster, image))
            })();
            repaint.request_repaint();
            result
        });
        Self {
            raster,
            ticket,
            rx,
            ready: None,
            cancelled: false,
        }
    }

    fn cancel(&mut self) {
        self.ticket.cancel();
        self.cancelled = true;
        self.ready = None;
    }

    fn poll(&mut self, ctx: &egui::Context) -> PdfDecodePoll {
        if self.ready.is_some() && !self.cancelled {
            return PdfDecodePoll::AlreadyReady;
        }
        let result = match self.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(20));
                return PdfDecodePoll::Pending;
            }
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        if self.cancelled {
            return PdfDecodePoll::Cancelled;
        }
        match result {
            Ok(Ok((raster, image))) => {
                self.raster = raster;
                self.ready = Some(image);
                PdfDecodePoll::Ready
            }
            Err(JobError::QueueFull) => PdfDecodePoll::QueueFull,
            Err(error) => PdfDecodePoll::Failed(error.to_string()),
            Ok(Err(error)) => PdfDecodePoll::Failed(error),
        }
    }
}

/// Read-only page preview with texture and decode state independent of the editor.
pub struct InlinePdfPagePreview {
    raster: Arc<PdfPageRasterResource>,
    decode_slot: Option<PdfPageDecodeSlot>,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    retry_at: Option<std::time::Instant>,
    #[cfg(test)]
    decode_start_count: usize,
}

impl InlinePdfPagePreview {
    pub fn new(raster: Arc<PdfPageRasterResource>) -> Self {
        Self {
            raster,
            decode_slot: None,
            texture: None,
            error: None,
            retry_at: None,
            #[cfg(test)]
            decode_start_count: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn decode_start_count_for_test(&self) -> usize {
        self.decode_start_count
    }

    fn defer_retry(&mut self, ctx: &egui::Context) {
        let delay = Duration::from_millis(50);
        self.retry_at = Some(std::time::Instant::now() + delay);
        ctx.request_repaint_after(delay);
    }

    /// Draw the page in a fixed-size region, decoding and uploading it on demand.
    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, size: Vec2) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let visible = ui.is_rect_visible(rect);
        if self.texture.is_none() && self.error.is_none() {
            if let Some(slot) = &mut self.decode_slot {
                match slot.poll(ctx) {
                    PdfDecodePoll::Pending => {}
                    PdfDecodePoll::Ready | PdfDecodePoll::AlreadyReady => {
                        let image = self
                            .decode_slot
                            .as_mut()
                            .and_then(|slot| slot.ready.take())
                            .expect("ready page decode has an image");
                        self.decode_slot = None;
                        self.texture = Some(ctx.load_texture(
                            format!("pdf-inline-page-{}", self.raster.page_num()),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    PdfDecodePoll::Failed(error) => {
                        self.decode_slot = None;
                        self.error = Some(error);
                    }
                    PdfDecodePoll::QueueFull => {
                        self.decode_slot = None;
                        self.defer_retry(ctx);
                    }
                    PdfDecodePoll::Cancelled => {
                        self.decode_slot = None;
                    }
                }
            } else if visible {
                if let Some(retry_at) = self.retry_at {
                    let now = std::time::Instant::now();
                    if retry_at > now {
                        ctx.request_repaint_after(retry_at - now);
                    } else {
                        self.retry_at = None;
                    }
                }
                if self.retry_at.is_none() {
                    #[cfg(test)]
                    {
                        self.decode_start_count += 1;
                    }
                    self.decode_slot = Some(PdfPageDecodeSlot::start(
                        Arc::clone(&self.raster),
                        None,
                        ctx,
                    ));
                    ctx.request_repaint_after(Duration::from_millis(20));
                }
            }
        }

        if let Some(texture) = &self.texture {
            let image_rect = fitted_pdf_page_rect(rect, self.raster.dimensions());
            ui.painter().image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else if self.error.is_some() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Preview unavailable",
                egui::FontId::default(),
                ui.visuals().weak_text_color(),
            );
        } else {
            ui.painter()
                .rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
        }
    }
}

impl Drop for InlinePdfPagePreview {
    fn drop(&mut self) {
        if let Some(slot) = &mut self.decode_slot {
            slot.cancel();
        }
    }
}

fn decode_page_png(png: Vec<u8>, max_dim: u32) -> Result<egui::ColorImage, String> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    let image = if image.width().max(image.height()) > max_dim {
        image.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

fn fitted_pdf_page_rect(rect: egui::Rect, dimensions: (u32, u32)) -> egui::Rect {
    let (width, height) = dimensions;
    let aspect = if width > 0 && height > 0 {
        width as f32 / height as f32
    } else {
        1.0
    };
    let fitted_size = if rect.width() / aspect <= rect.height() {
        Vec2::new(rect.width(), rect.width() / aspect)
    } else {
        Vec2::new(rect.height() * aspect, rect.height())
    };
    egui::Rect::from_center_size(rect.center(), fitted_size)
}

/// View-local navigation of the selected page, relative to its fitted size.
struct PdfPageViewport {
    page_num: Option<usize>,
    zoom: f32,
    pan: Vec2,
    render_width: u32,
}

impl Default for PdfPageViewport {
    fn default() -> Self {
        Self {
            page_num: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            render_width: PAGE_RASTER_MAX_DIM,
        }
    }
}

impl PdfPageViewport {
    fn select_page(&mut self, page_num: usize) {
        if self.page_num != Some(page_num) {
            *self = Self {
                page_num: Some(page_num),
                ..Self::default()
            };
        }
    }

    fn page_rect(&mut self, viewport: egui::Rect, dimensions: (u32, u32)) -> egui::Rect {
        let size = fitted_pdf_page_rect(viewport, dimensions).size() * self.zoom;
        let overflow = ((size - viewport.size()) * 0.5).max(Vec2::ZERO);
        self.pan.x = self.pan.x.clamp(-overflow.x, overflow.x);
        self.pan.y = self.pan.y.clamp(-overflow.y, overflow.y);
        egui::Rect::from_center_size(viewport.center() + self.pan, size)
    }

    fn update_render_width(
        &mut self,
        page_size: Vec2,
        pixels_per_point: f32,
        max_texture_side: usize,
    ) {
        // Supersample text, then let the GPU reduce it to physical display pixels.
        // Width buckets avoid launching a renderer for every wheel increment.
        let aspect = page_size.x / page_size.y.max(1.0);
        let max_dim = max_texture_side.min(PAGE_DETAIL_MAX_DIM as usize) as f32;
        let width_limit = (max_dim * aspect.min(1.0)).floor().max(1.0) as u32;
        let physical_width = page_size.x * pixels_per_point * 1.5;
        let requested = (physical_width / 256.0).ceil().max(1.0) as u32 * 256;
        self.render_width = requested.min(width_limit);
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        texture: egui::TextureId,
        dimensions: (u32, u32),
    ) -> egui::Response {
        let (viewport, response) = ui.allocate_exact_size(
            ui.available_size().max(Vec2::splat(1.0)),
            egui::Sense::click_and_drag(),
        );
        // Clamp before interaction as well, since the host or page aspect may change.
        self.page_rect(viewport, dimensions);
        if response.hovered() {
            let scroll = ui.input(|input| input.raw_scroll_delta.y);
            if scroll != 0.0 {
                let old_zoom = self.zoom;
                self.zoom = (old_zoom * (scroll * 0.002).exp()).clamp(0.25, 8.0);
                if let Some(pointer) = response.hover_pos() {
                    self.pan +=
                        (pointer - viewport.center() - self.pan) * (1.0 - self.zoom / old_zoom);
                }
            }
        }
        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if response.double_clicked() {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        let page_rect = self.page_rect(viewport, dimensions);
        self.update_render_width(
            page_rect.size(),
            ui.ctx().pixels_per_point(),
            ui.input(|input| input.max_texture_side),
        );
        if page_rect.width() > viewport.width() || page_rect.height() > viewport.height() {
            response.clone().on_hover_cursor(if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        ui.painter_at(viewport).image(
            texture,
            page_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        response.on_hover_text("Scroll to zoom • Drag to pan • Double-click to fit")
    }
}

/// Completion message for a potentially expensive, lossless PDF mutation.  PDF
/// parsing and saving can take seconds for large manuals, so this must never
/// run from egui's frame callback.
struct PdfEditMsg {
    operation: &'static str,
    page_num: usize,
    output: PathBuf,
    staging_dir: tempfile::TempDir,
    result: Result<(), String>,
}

/// Immutable raster data whose temporary file remains alive while any view holds it.
pub struct PdfPageRasterResource {
    page_num: usize,
    path: PathBuf,
    width: u32,
    height: u32,
    // Requested raster width distinguishes thumbnails from full page renders.
    render_width: u32,
    _raster_directory: Arc<tempfile::TempDir>,
}

impl PdfPageRasterResource {
    fn needs_render(&self, requested_width: u32) -> bool {
        // Very large retained rasters alias when minified without GPU mipmaps.
        self.render_width < requested_width || self.render_width > requested_width.saturating_mul(2)
    }

    pub fn page_num(&self) -> usize {
        self.page_num
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Builds the gallery's tiny valid document and its already-rendered page raster.
/// Both files stay alive for as long as either preview or editor retains the raster.
pub(crate) fn gallery_pdf_page_source() -> Result<(PathBuf, Arc<PdfPageRasterResource>), String> {
    let directory = Arc::new(tempfile::tempdir().map_err(|error| error.to_string())?);
    let pdf_path = directory.path().join("gallery.pdf");
    let raster_path = directory.path().join("gallery-page.png");

    let stream = "0.149 0.357 0.6 rg\n16 16 64 96 re f\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 96 128] /Resources << >> /Contents 4 0 R >>"
            .to_owned(),
        format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len() + 1);
    offsets.push(0usize);
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(&pdf_path, pdf).map_err(|error| error.to_string())?;

    let mut image = image::RgbaImage::from_pixel(96, 128, image::Rgba([255, 255, 255, 255]));
    for y in 16..112 {
        for x in 16..80 {
            image.put_pixel(x, y, image::Rgba([38, 91, 153, 255]));
        }
    }
    image::DynamicImage::ImageRgba8(image)
        .save_with_format(&raster_path, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;

    Ok((
        pdf_path,
        Arc::new(PdfPageRasterResource {
            page_num: 1,
            path: raster_path,
            width: 96,
            height: 128,
            render_width: 96,
            _raster_directory: directory,
        }),
    ))
}

/// Per-view page texture and status, plus an optional shareable raster resource.
pub struct PdfPageItem {
    pub page_num: usize,
    pub raster: Option<Arc<PdfPageRasterResource>>,
    pub texture: Option<egui::TextureHandle>,
    texture_last_used: u64,
    pub error: Option<String>,
}

/// Interactive state for the multi-page PDF thumbnail grid editor.
pub struct PdfEditorState {
    pub path: PathBuf,
    pub title: String,
    pub page_count: usize,
    pub pages: Vec<PdfPageItem>,
    pub selected_page: Option<usize>,
    pub status_toast: StatusToast,
    pub dirty: bool,
    info_job: Option<(
        JobTicket,
        Receiver<Result<Result<PdfInfo, String>, JobError>>,
    )>,
    preview_rx: Option<Receiver<PdfPreviewMsg>>,
    preview_job: Option<(JobTicket, Receiver<Result<(), JobError>>)>,
    preview_cancel: Option<Arc<AtomicBool>>,
    previews_loading: bool,
    edit_rx: Option<Receiver<Result<PdfEditMsg, JobError>>>,
    edit_ticket: Option<JobTicket>,
    edit_in_progress: bool,
    raster_cache: Option<std::sync::Arc<tempfile::TempDir>>,
    // Gallery documents may live alongside their first raster; keep the PDF
    // itself alive when a successful edit replaces that raster cache.
    source_directory: Option<Arc<tempfile::TempDir>>,
    texture_frame: u64,
    decode_job: Option<PdfPageDecodeSlot>,
    decode_requests: Vec<usize>,
    viewport: PdfPageViewport,
    navigation: navigation::PdfNavigation,
}

impl PdfEditorState {
    fn without_jobs(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        Self {
            path,
            title,
            page_count: 0,
            pages: Vec::new(),
            selected_page: None,
            status_toast: StatusToast::new(),
            dirty: false,
            info_job: None,
            preview_rx: None,
            preview_job: None,
            preview_cancel: None,
            previews_loading: false,
            edit_rx: None,
            edit_ticket: None,
            edit_in_progress: false,
            raster_cache: None,
            source_directory: None,
            texture_frame: 0,
            decode_job: None,
            decode_requests: Vec::new(),
            viewport: PdfPageViewport::default(),
            navigation: navigation::PdfNavigation::default(),
        }
    }

    /// Starts loading page metadata and thumbnail previews for the given PDF file.
    pub fn new(path: PathBuf) -> Self {
        let mut state = Self::without_jobs(path);
        state.start_info_worker();
        state
    }

    /// Opens a one-page document using a page raster already owned by another view.
    pub(crate) fn from_shared_page_raster(
        path: PathBuf,
        raster: Arc<PdfPageRasterResource>,
    ) -> Self {
        let mut state = Self::without_jobs(path);
        state.page_count = 1;
        state.selected_page = Some(1);
        state.raster_cache = Some(Arc::clone(&raster._raster_directory));
        state.source_directory = Some(Arc::clone(&raster._raster_directory));
        state.pages.push(PdfPageItem {
            page_num: 1,
            raster: Some(raster),
            texture: None,
            texture_last_used: 0,
            error: None,
        });
        state
    }
}

impl Drop for PdfEditorState {
    fn drop(&mut self) {
        self.cancel_edit_worker();
        self.cancel_preview_worker();
        if let Some(job) = &mut self.decode_job {
            job.cancel();
        }
        if let Some((ticket, _)) = &self.info_job {
            ticket.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
