//! Multi-page PDF thumbnail grid viewer and page manipulation workbench view.
//!
//! Renders a two-pane layout: a left-side scrollable/wrapping page selector,
//! and a right-side large preview canvas of the selected page with live 90° rotation,
//! page deletion, and dirty state tracking.

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
const PAGE_TEXTURE_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const PAGE_RASTER_MAX_DIM: u32 = 1200;
const STAGED_PDF_FILENAME: &str = "edited.pdf";

struct PdfPreviewMsg {
    page_num: usize,
    result: Result<Arc<PdfPageRasterResource>, String>,
}

// One running/queued decode or completed ColorImage per editor (at most
// 1200 * 1200 * 4 bytes of published pixels). Canceled jobs retain their slot
// until acknowledged so rapid navigation cannot accumulate running decodes.
struct PdfPageDecodeSlot {
    raster: Arc<PdfPageRasterResource>,
    ticket: JobTicket,
    rx: Receiver<Result<Result<egui::ColorImage, String>, JobError>>,
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
    fn start(raster: Arc<PdfPageRasterResource>, ctx: &egui::Context) -> Self {
        let worker_raster = Arc::clone(&raster);
        let repaint = ctx.clone();
        let (ticket, rx) = BackgroundJobs::global().submit(Duration::from_secs(10), move || {
            let result = std::fs::read(worker_raster.path())
                .map_err(|error| format!("Could not read cached PDF raster: {error}"))
                .and_then(decode_page_png);
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
            Ok(Ok(image)) => {
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
                    self.decode_slot =
                        Some(PdfPageDecodeSlot::start(Arc::clone(&self.raster), ctx));
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

fn decode_page_png(png: Vec<u8>) -> Result<egui::ColorImage, String> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    let image = if image.width().max(image.height()) > PAGE_RASTER_MAX_DIM {
        image.thumbnail(PAGE_RASTER_MAX_DIM, PAGE_RASTER_MAX_DIM)
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
    _raster_directory: Arc<tempfile::TempDir>,
}

impl PdfPageRasterResource {
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
}

impl PdfEditorState {
    /// Place generated documents beside the source.  Edits intentionally never
    /// overwrite the source: complex PDFs are too easy to damage during rewrite.
    fn sibling_output_path(&self, operation: &str) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("document");
        self.path
            .with_file_name(format!("{stem}.{operation}.edited.pdf"))
    }

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

    fn start_info_worker(&mut self) {
        if let Some((ticket, _)) = self.info_job.take() {
            ticket.cancel();
        }
        self.cancel_edit_worker();
        self.cancel_preview_worker();
        self.raster_cache = None;
        if let Some(job) = &mut self.decode_job {
            job.cancel();
        }
        self.decode_requests.clear();
        self.pages.clear();
        self.page_count = 0;
        let path = self.path.clone();
        self.info_job = Some(
            BackgroundJobs::global().submit(Duration::from_secs(30), move || get_pdf_info(&path)),
        );
    }

    fn poll_info_worker(&mut self, ctx: &egui::Context, now: f64) {
        let Some((_, rx)) = &self.info_job else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.info_job = None;
        let info = match result {
            Ok(Ok(info)) => info,
            failure => {
                let reason = match failure {
                    Ok(Err(error)) => error,
                    Err(error) => error.to_string(),
                    Ok(Ok(_)) => unreachable!(),
                };
                self.status_toast
                    .set(format!("PDF metadata unavailable: {reason}"), now);
                PdfInfo {
                    page_count: 1,
                    title: None,
                    author: None,
                    page_size: None,
                }
            }
        };
        self.page_count = info.page_count;
        self.pages = (1..=self.page_count)
            .map(|page_num| PdfPageItem {
                page_num,
                raster: None,
                texture: None,
                texture_last_used: 0,
                error: None,
            })
            .collect();
        self.selected_page =
            (self.page_count > 0).then(|| self.selected_page.unwrap_or(1).min(self.page_count));
        self.start_preview_worker();
        ctx.request_repaint();
    }

    fn start_preview_worker(&mut self) {
        self.start_preview_worker_on(BackgroundJobs::global(), render_pdf_page_preview);
    }

    fn start_preview_worker_on<F>(&mut self, jobs: &BackgroundJobs, render_page: F)
    where
        F: Fn(&Path, usize, u32) -> Result<(Vec<u8>, u32, u32), String> + Send + 'static,
    {
        self.cancel_preview_worker();
        let raster_cache = match tempfile::tempdir() {
            Ok(cache) => std::sync::Arc::new(cache),
            Err(error) => {
                self.previews_loading = false;
                for page in &mut self.pages {
                    page.error = Some(format!("PDF preview cache unavailable: {error}"));
                }
                return;
            }
        };
        self.raster_cache = Some(std::sync::Arc::clone(&raster_cache));
        let path = self.path.clone();
        let page_count = self.page_count;
        let selected_page = self.selected_page.unwrap_or(1).min(page_count.max(1));
        // Bound pending raster data if rendering outruns the UI. Cancellation
        // drops this receiver to unblock a producer waiting to publish a page.
        let (tx, rx) = sync_channel(PREVIEW_QUEUE_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (ticket, job_rx) = spawn_preview_worker(
            jobs,
            path,
            page_count,
            selected_page,
            raster_cache,
            tx,
            Arc::clone(&cancelled),
            render_page,
        );
        self.preview_rx = Some(rx);
        self.preview_job = Some((ticket, job_rx));
        self.preview_cancel = Some(cancelled);
        self.previews_loading = true;
    }

    fn cancel_preview_worker(&mut self) {
        if let Some(cancelled) = self.preview_cancel.take() {
            cancelled.store(true, Ordering::Release);
        }
        if let Some((ticket, _)) = self.preview_job.take() {
            ticket.cancel();
        }
        self.preview_rx = None;
        self.previews_loading = false;
    }

    fn poll_preview_worker(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.preview_rx {
            loop {
                match rx.try_recv() {
                    Ok(message) => {
                        if let Some(job) = &mut self.decode_job {
                            if job.raster.page_num() == message.page_num {
                                job.cancel();
                            }
                        }
                        if let Some(page) = self.pages.get_mut(message.page_num.saturating_sub(1)) {
                            match message.result {
                                Ok(raster) => {
                                    page.texture = None;
                                    page.raster = Some(raster);
                                    page.error = None;
                                }
                                Err(error) => page.error = Some(error),
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.preview_rx = None;
        }
        let completion = self
            .preview_job
            .as_ref()
            .and_then(|(_, rx)| match rx.try_recv() {
                Ok(Ok(())) => Some(Ok(())),
                Ok(Err(error)) => Some(Err(error.to_string())),
                Err(TryRecvError::Disconnected) => Some(Err(
                    "PDF preview worker disconnected unexpectedly".to_owned(),
                )),
                Err(TryRecvError::Empty) => None,
            });
        if let Some(result) = completion {
            self.preview_job = None;
            self.preview_cancel = None;
            self.preview_rx = None;
            self.previews_loading = false;
            if let Err(error) = result {
                let message = format!("PDF previews unavailable: {error}");
                for page in &mut self.pages {
                    if page.raster.is_none() && page.error.is_none() {
                        page.error = Some(message.clone());
                    }
                }
                self.status_toast
                    .set(message, ctx.input(|input| input.time));
            }
        } else if finished && self.preview_job.is_none() {
            self.previews_loading = false;
        } else if self.previews_loading {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Upload on demand and evict least recently used pages. The selected page
    /// and any texture already referenced by this frame's paint shapes are
    /// protected, so eviction cannot leave a dangling image in the draw list.
    fn ensure_page_texture(
        &mut self,
        page_num: usize,
        ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        let index = page_num.checked_sub(1)?;
        let page = self.pages.get_mut(index)?;
        debug_assert!(
            page.raster
                .as_ref()
                .is_none_or(|raster| raster.page_num() == page_num),
            "page raster metadata must match its owning page"
        );
        if let Some(texture) = &page.texture {
            page.texture_last_used = self.texture_frame;
            return Some(texture.id());
        }
        if page.raster.is_none() || page.error.is_some() {
            return None;
        }
        if !self.decode_requests.contains(&page_num) {
            if self.decode_requests.len() == PAGE_DECODE_REQUEST_LIMIT
                && self.selected_page == Some(page_num)
            {
                self.decode_requests.pop();
            }
            if self.decode_requests.len() < PAGE_DECODE_REQUEST_LIMIT {
                self.decode_requests.push(page_num);
            }
        }
        let job = self.decode_job.as_ref()?;
        if job.raster.page_num() != page_num || job.cancelled {
            return None;
        }
        let bytes = job.ready.as_ref()?.pixels.len() * 4;
        if !self.make_texture_room(bytes) {
            ctx.request_repaint();
            return None;
        }
        let color_image = self.decode_job.take()?.ready?;
        let page = &mut self.pages[index];
        let texture = ctx.load_texture(
            format!("pdf_page_{page_num}"),
            color_image,
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        page.texture = Some(texture);
        page.texture_last_used = self.texture_frame;
        Some(id)
    }

    fn poll_decode_worker(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.decode_job else {
            return;
        };
        let page_num = job.raster.page_num();
        match job.poll(ctx) {
            PdfDecodePoll::Pending => return,
            PdfDecodePoll::Ready => {}
            PdfDecodePoll::AlreadyReady => return,
            PdfDecodePoll::QueueFull | PdfDecodePoll::Cancelled => self.decode_job = None,
            PdfDecodePoll::Failed(error) => {
                if let Some(page) = self.pages.get_mut(page_num - 1) {
                    page.error = Some(format!("PDF preview decode failed: {error}"));
                }
                self.decode_job = None;
            }
        }
        ctx.request_repaint();
    }

    // Called after painting, when the complete visible demand is known. No
    // pixel work runs here; one job is submitted, selected page first.
    fn schedule_page_decode(&mut self, ctx: &egui::Context) {
        if let Some(job) = &mut self.decode_job {
            let selected_waiting = self.selected_page.is_some_and(|selected| {
                selected != job.raster.page_num()
                    && self.decode_requests.contains(&selected)
                    && self.pages.get(selected - 1).is_some_and(|page| {
                        page.texture.is_none() && page.error.is_none() && page.raster.is_some()
                    })
            });
            if !self.decode_requests.contains(&job.raster.page_num()) || selected_waiting {
                if job.ready.is_some() {
                    self.decode_job = None;
                } else {
                    job.cancel();
                }
            }
        }
        if self.decode_job.is_none() {
            let candidate = self
                .selected_page
                .into_iter()
                .chain(self.decode_requests.iter().copied())
                .find(|page_num| {
                    self.decode_requests.contains(page_num)
                        && self.pages.get(page_num - 1).is_some_and(|page| {
                            page.texture.is_none() && page.error.is_none() && page.raster.is_some()
                        })
                });
            if let Some(page_num) = candidate {
                let raster = self.pages[page_num - 1]
                    .raster
                    .as_ref()
                    .expect("decode candidates have cached rasters")
                    .clone();
                self.decode_job = Some(PdfPageDecodeSlot::start(raster, ctx));
            }
        }
        if self.decode_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
    }

    fn make_texture_room(&mut self, additional_bytes: usize) -> bool {
        let (mut count, mut bytes) = self
            .pages
            .iter()
            .filter_map(|page| page.texture.as_ref())
            .fold((0, 0), |(count, bytes), texture| {
                (count + 1, bytes + texture.byte_size())
            });
        while count >= PAGE_TEXTURE_LIMIT || bytes + additional_bytes > PAGE_TEXTURE_BYTE_LIMIT {
            let candidate = self
                .pages
                .iter_mut()
                .filter(|page| {
                    page.texture.is_some()
                        && Some(page.page_num) != self.selected_page
                        && page.texture_last_used != self.texture_frame
                })
                .min_by_key(|page| page.texture_last_used);
            let Some(page) = candidate else {
                return false;
            };
            let texture = page.texture.take().expect("eviction candidate has texture");
            count -= 1;
            bytes -= texture.byte_size();
        }
        true
    }

    pub fn reload_pages(&mut self, ctx: &egui::Context) {
        self.start_info_worker();
        ctx.request_repaint();
    }

    pub fn delete_current_page(&mut self, now: f64) {
        if self.info_job.is_some() {
            return;
        }
        let Some(cur) = self.selected_page else {
            return;
        };
        if self.page_count <= 1 {
            self.status_toast
                .set("Cannot delete only page from PDF", now);
            return;
        }

        self.start_edit("delete", cur, 0, now);
    }

    pub fn rotate_current_page(&mut self, degrees: u32, now: f64) {
        if self.info_job.is_some() {
            return;
        }
        let Some(cur) = self.selected_page else {
            return;
        };
        self.start_edit("rotate", cur, degrees, now);
    }

    fn start_edit(&mut self, operation: &'static str, page_num: usize, degrees: u32, now: f64) {
        self.start_edit_on(
            BackgroundJobs::global(),
            operation,
            page_num,
            now,
            move |input, staged_output| match operation {
                "delete" => delete_pdf_pages(input, &[page_num], staged_output),
                "rotate" => rotate_pdf_page(input, page_num, degrees, staged_output),
                _ => Err("Unknown PDF edit operation".to_string()),
            },
        );
    }

    fn start_edit_on<F>(
        &mut self,
        jobs: &BackgroundJobs,
        operation: &'static str,
        page_num: usize,
        now: f64,
        perform_edit: F,
    ) where
        F: FnOnce(&Path, &Path) -> Result<(), String> + Send + 'static,
    {
        if self.edit_in_progress {
            self.status_toast
                .set("A PDF edit is already in progress", now);
            return;
        }
        let input = self.path.clone();
        let output = self.sibling_output_path(operation);
        let output_dir = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let staging_dir = match tempfile::tempdir_in(output_dir) {
            Ok(dir) => dir,
            Err(error) => {
                self.status_toast
                    .set(format!("Could not stage PDF edit: {error}"), now);
                return;
            }
        };
        let expected_page_count = self
            .page_count
            .saturating_sub(usize::from(operation == "delete"));
        let (ticket, rx) = jobs.submit(Duration::MAX, move || {
            let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
            let result = perform_edit(&input, &staged_output).and_then(|()| {
                if !staged_output.is_file() {
                    return Err("PDF operation produced no output file".to_string());
                }
                let info = get_pdf_info(&staged_output)?;
                if info.page_count != expected_page_count {
                    return Err(format!(
                        "Edited PDF has {} pages; expected {}",
                        info.page_count, expected_page_count
                    ));
                }
                render_pdf_page_preview(&staged_output, 1, 64).map(|_| ())
            });
            PdfEditMsg {
                operation,
                page_num,
                output,
                staging_dir,
                result,
            }
        });
        self.edit_in_progress = true;
        self.edit_ticket = Some(ticket);
        self.edit_rx = Some(rx);
        self.status_toast.set(
            format!(
                "{} page {}…",
                if operation == "delete" {
                    "Deleting"
                } else {
                    "Rotating"
                },
                page_num
            ),
            now,
        );
    }

    fn cancel_edit_worker(&mut self) {
        if let Some(ticket) = self.edit_ticket.take() {
            ticket.cancel();
        }
        self.edit_rx = None;
        self.edit_in_progress = false;
    }

    fn poll_edit_worker(&mut self, ctx: &egui::Context, now: f64) {
        let message = match self.edit_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(Ok(message))) => Some(message),
            Some(Ok(Err(error))) => {
                self.edit_rx = None;
                self.edit_ticket = None;
                self.edit_in_progress = false;
                self.status_toast
                    .set(format!("PDF edit unavailable: {error}"), now);
                return;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.edit_rx = None;
                self.edit_ticket = None;
                self.edit_in_progress = false;
                self.status_toast.set("PDF edit worker disconnected", now);
                return;
            }
            _ => None,
        };
        if let Some(message) = message {
            self.edit_rx = None;
            self.edit_ticket = None;
            self.edit_in_progress = false;
            let result = message.result.and_then(|()| {
                std::fs::rename(
                    message.staging_dir.path().join(STAGED_PDF_FILENAME),
                    &message.output,
                )
                .map_err(|error| format!("Could not publish edited PDF: {error}"))
            });
            match result {
                Ok(()) => {
                    self.path = message.output;
                    self.title = self
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("edited.pdf")
                        .to_string();
                    self.reload_pages(ctx);
                    self.dirty = true;
                    self.status_toast.set(
                        format!(
                            "{} page {}",
                            if message.operation == "delete" {
                                "Deleted"
                            } else {
                                "Rotated"
                            },
                            message.page_num
                        ),
                        now,
                    );
                }
                Err(error) => self.status_toast.set(
                    format!("Failed to {} page: {}", message.operation, error),
                    now,
                ),
            }
        } else if self.edit_in_progress {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.poll_info_worker(ctx, now);
        self.poll_preview_worker(ctx);
        self.poll_edit_worker(ctx, now);

        // Hotkeys
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.edit_in_progress {
                self.status_toast
                    .set("PDF edit is still running; wait for it to finish", now);
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                *close = true;
                return;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            self.delete_current_page(now);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.rotate_current_page(90, now);
        }

        self.texture_frame += 1;
        self.decode_requests.clear();
        self.poll_decode_worker(ctx);
        if let Some(page_num) = self.selected_page {
            self.ensure_page_texture(page_num, ctx);
        }

        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let warn_accent = crate::hex_to_c32(&palette.undo_border);
        let warn_bg = crate::hex_to_c32(&palette.colors.surface_recessed);

        egui::Frame::none()
            .fill(colors.canvas)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let page_count = self.page_count;
                let metadata_loading = self.info_job.is_some();
                let page_actions_enabled =
                    !metadata_loading && !self.edit_in_progress && self.selected_page.is_some();
                let mut rotate_clicked = false;
                let mut delete_clicked = false;

                let mut shell = StudioShell::new(&self.title, Icon::Pdf, palette, close)
                    .with_badge(
                        if metadata_loading {
                            "Loading PDF…".to_owned()
                        } else {
                            format!("{} pages", page_count)
                        },
                        None,
                    )
                    .with_escape_hatch(EscapeHatch::primary(
                        "Okular",
                        "okular",
                        "Open PDF in external viewer (Okular / Evince)",
                    ).with_icon(Icon::Pdf))
                    .with_footer_hints("Click thumbnail: Select page | [R] Rotate 90° | [Del] Delete page | Esc: Close");

                if let Some(cur) = self.selected_page.filter(|_| !metadata_loading) {
                    shell = shell.with_footer_readout(format!("Selected: Page {} of {}", cur, page_count));
                }

                let clicked_hatch = shell.render_header(ui, |ui| {
                    // Delete Page Button
                    let del_btn = crate::ui_ext::danger_button_icon(
                        Some(Icon::Trash),
                        "Delete Page [Del]",
                        warn_bg,
                        palette,
                    )
                    .stroke(egui::Stroke::new(1.0_f32, warn_accent));
                    if ui
                        .add_enabled(page_actions_enabled, del_btn)
                        .on_hover_text("Delete selected page from PDF (Hotkey: Del)")
                        .clicked()
                    {
                        delete_clicked = true;
                    }

                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                    // Rotate 90 CW Button
                    let rot_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCw),
                        "Rotate 90° [R]",
                        colors.panel,
                        palette,
                    );
                    if ui
                        .add_enabled(page_actions_enabled, rot_btn)
                        .on_hover_text("Rotate selected page 90° clockwise (Hotkey: R)")
                        .clicked()
                    {
                        rotate_clicked = true;
                    }
                });

                if let Some(0) = clicked_hatch {
                    EscapeHatchLauncher::launch("PDF Viewer", "okular", &self.path, &mut self.status_toast, now);
                }

                if rotate_clicked {
                    self.rotate_current_page(90, now);
                }
                if delete_clicked {
                    self.delete_current_page(now);
                }

                ui.add_space(crate::ui_ext::SPACE_XS);
                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_XS);

                // Two-Pane Split Layout
                // Left pane: page thumbnail list / grid (takes left side, wrapping if width permits)
                // Right pane: large preview of the selected page
                let avail_w = ui.available_width();
                let avail_h = (ui.available_height() - 32.0).max(100.0);
                // The horizontal row contains two item gaps and the separator in
                // addition to the panes. Budget those first so pane minimums never
                // make the combined layout wider than a compact popup.
                let pane_gap = ui.spacing().item_spacing.x * 2.0 + 1.0;
                let pane_width = (avail_w - pane_gap).max(0.0);
                let left_pane_w = (pane_width * 0.30)
                    .clamp(180.0, 360.0)
                    .min((pane_width - 200.0).max(0.0));
                let right_pane_w = (pane_width - left_pane_w).max(0.0);
                ui.horizontal(|ui| {
                    // --- Left Pane: Page Selector Grid ---
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_pane_w, avail_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left_pane_w);
                            ui.set_height(avail_h);

                            egui::Frame::none()
                                .fill(colors.recessed)
                                .stroke(Stroke::new(1.0_f32, colors.panel))
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Pages")
                                                .strong()
                                                .color(colors.text_primary),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("{} total", page_count))
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(colors.text_muted),
                                                );
                                            },
                                        );
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);
                                    ui.separator();
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    egui::ScrollArea::vertical()
                                        .max_width(
                                            (left_pane_w
                                                - 2.0 * crate::ui_ext::SPACE_SM
                                                - 2.0)
                                                .max(0.0),
                                        )
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            let thumbnail_area_width = (left_pane_w
                                                - 2.0 * crate::ui_ext::SPACE_SM
                                                - 2.0)
                                                .max(0.0);
                                            ui.set_width(thumbnail_area_width);
                                            ui.vertical(|ui| {
                                                let pages_clone: Vec<(
                                                    usize,
                                                    u32,
                                                    u32,
                                                    bool,
                                                )> = self
                                                    .pages
                                                    .iter()
                                                    .map(|p| {
                                                        let (width, height) = p
                                                            .raster
                                                            .as_ref()
                                                            .map(|raster| raster.dimensions())
                                                            .unwrap_or_default();
                                                        (
                                                            p.page_num,
                                                            width,
                                                            height,
                                                            p.error.is_some(),
                                                        )
                                                    })
                                                    .collect();

                                                for (page_num, w, h, failed) in pages_clone {
                                                    let is_selected =
                                                        self.selected_page == Some(page_num);

                                                    ui.vertical(|ui| {
                                                        let border_col = if is_selected {
                                                            accent
                                                        } else {
                                                            colors.panel
                                                        };
                                                        let stroke_w = if is_selected { 2.0_f32 } else { 1.0_f32 };

                                                        egui::Frame::none()
                                                            .fill(if is_selected {
                                                                colors.panel
                                                            } else {
                                                                colors.canvas
                                                            })
                                                            .stroke(Stroke::new(stroke_w, border_col))
                                                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                                            .inner_margin(egui::Margin::same(crate::ui_ext::ITEM_SPACING_Y))
                                                            .show(ui, |ui| {
                                                                let aspect = if h > 0 {
                                                                    w as f32 / h as f32
                                                                } else {
                                                                    0.707
                                                                };
                                                                let panel_insets =
                                                                    2.0 * crate::ui_ext::SPACE_SM
                                                                        + 2.0;
                                                                let card_insets = 2.0
                                                                    * (2.0
                                                                        * crate::ui_ext::ITEM_SPACING_Y
                                                                        + 2.0);
                                                                let thumb_w_limit = (left_pane_w
                                                                    - panel_insets
                                                                    - card_insets)
                                                                    .max(80.0);
                                                                let thumb_w = (130.0 * aspect)
                                                                    .min(thumb_w_limit);
                                                                let thumb_h = thumb_w / aspect;

                                                                let thumbnail_rect = egui::Rect::from_min_size(
                                                                    ui.next_widget_position(),
                                                                    Vec2::new(thumb_w, thumb_h),
                                                                );
                                                                let tex_id_opt = ui.is_rect_visible(thumbnail_rect)
                                                                    .then(|| self.ensure_page_texture(page_num, ctx))
                                                                    .flatten();
                                                                if let Some(tex_id) = tex_id_opt {
                                                                    let img_btn = egui::ImageButton::new(
                                                                        egui::load::SizedTexture::new(
                                                                            tex_id,
                                                                            Vec2::new(thumb_w, thumb_h),
                                                                        ),
                                                                    );
                                                                    if ui.add(img_btn).clicked() {
                                                                        self.selected_page = Some(page_num);
                                                                    }
                                                                } else {
                                                                    let placeholder = egui::Button::new(if failed { "Unavailable" } else { "…" })
                                                                        .min_size(Vec2::new(thumb_w, thumb_h));
                                                                    if ui.add(placeholder).clicked() {
                                                                        self.selected_page = Some(page_num);
                                                                    }
                                                                }

                                                                ui.add_space(2.0);
                                                                ui.horizontal(|ui| {
                                                                    ui.label(
                                                                        egui::RichText::new(format!(
                                                                            "Page {}",
                                                                            page_num
                                                                        ))
                                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                                        .strong()
                                                                        .color(if is_selected {
                                                                            crate::ui_ext::contrast_text_c32_on(
                                                                                colors.panel,
                                                                                colors.panel,
                                                                                Some(accent),
                                                                                palette,
                                                                            )
                                                                        } else {
                                                                            colors.text_muted
                                                                        }),
                                                                    );
                                                                });
                                                            });
                                                    });

                                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                                        }
                                            });
                                        });
                                });
                        },
                    );

                    ui.separator();

                    // --- Right Pane: Large Page Preview ---
                    ui.allocate_ui_with_layout(
                        Vec2::new(right_pane_w, avail_h),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.set_width(right_pane_w);
                            ui.set_height(avail_h);

                            egui::Frame::none()
                                .fill(colors.recessed)
                                .stroke(Stroke::new(1.0_f32, colors.panel))
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
                                .show(ui, |ui| {
                                    if let Some(page_num) = self.selected_page {
                                        self.ensure_page_texture(page_num, ctx);
                                    }
                                    let selected_item = self
                                        .selected_page
                                        .and_then(|sel| self.pages.iter().find(|p| p.page_num == sel));

                                    if let Some(page) = selected_item {
                                        let (page_width, page_height) = page
                                            .raster
                                            .as_ref()
                                            .map(|raster| raster.dimensions())
                                            .unwrap_or_default();
                                        // Header bar above preview
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Page {} of {}",
                                                    page.page_num, page_count
                                                ))
                                                .strong()
                                                .color(colors.text_primary),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "• {} × {} px",
                                                    page_width, page_height
                                                ))
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(colors.text_muted),
                                            );

                                        });

                                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                        ui.separator();
                                        ui.add_space(crate::ui_ext::SPACE_SM);

                                        // Large page preview image fitted to right pane
                                        let max_w = (ui.available_width() - 16.0).max(50.0);
                                        let max_h = (ui.available_height() - 16.0).max(50.0);

                                        let aspect = if page_height > 0 {
                                            page_width as f32 / page_height as f32
                                        } else {
                                            0.707
                                        };

                                        let (draw_w, draw_h) = if max_w / aspect <= max_h {
                                            (max_w, max_w / aspect)
                                        } else {
                                            (max_h * aspect, max_h)
                                        };

                                        ui.centered_and_justified(|ui| {
                                            if let Some(ref tex) = page.texture {
                                                ui.image(egui::load::SizedTexture::new(
                                                    tex.id(),
                                                    Vec2::new(draw_w, draw_h),
                                                ));
                                            } else if let Some(error) = &page.error {
                                                ui.label(
                                                    egui::RichText::new(error)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(crate::ui_ext::contrast_text_c32_on(
                                                            colors.recessed,
                                                            colors.recessed,
                                                            Some(colors.danger),
                                                            palette,
                                                        )),
                                                );
                                            } else {
                                                ui.spinner();
                                            }
                                        });
                                    } else {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                egui::RichText::new(if metadata_loading {
                                                    "Loading PDF pages…"
                                                } else {
                                                    "No page selected."
                                                })
                                                    .color(colors.text_muted),
                                            );
                                        });
                                    }
                                });
                        },
                    );
                });

                // Footer
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
        self.schedule_page_decode(ctx);
    }
}

fn spawn_preview_worker<F>(
    jobs: &BackgroundJobs,
    path: PathBuf,
    page_count: usize,
    selected_page: usize,
    raster_cache: std::sync::Arc<tempfile::TempDir>,
    tx: std::sync::mpsc::SyncSender<PdfPreviewMsg>,
    cancelled: Arc<AtomicBool>,
    render_page: F,
) -> (JobTicket, Receiver<Result<(), JobError>>)
where
    F: Fn(&Path, usize, u32) -> Result<(Vec<u8>, u32, u32), String> + Send + 'static,
{
    jobs.submit(Duration::MAX, move || {
        // A large selected-page raster arrives first; thumbnails then fill in
        // incrementally. An in-flight renderer may complete after cancellation,
        // but its result must not be written or published.
        let mut order = Vec::with_capacity(page_count);
        if page_count > 0 {
            order.push(selected_page);
        }
        order.extend((1..=page_count).filter(|page| *page != selected_page));
        for page_num in order {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let max_dim = if page_num == selected_page {
                PAGE_RASTER_MAX_DIM
            } else {
                360
            };
            let rendered = render_page(&path, page_num, max_dim);
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let result = rendered.and_then(|(png_bytes, width, height)| {
                let raster_path = raster_cache.path().join(format!("page-{page_num}.png"));
                std::fs::write(&raster_path, png_bytes)
                    .map(|()| {
                        Arc::new(PdfPageRasterResource {
                            page_num,
                            path: raster_path,
                            width,
                            height,
                            _raster_directory: Arc::clone(&raster_cache),
                        })
                    })
                    .map_err(|error| format!("Could not cache PDF page {page_num}: {error}"))
            });
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            if tx.send(PdfPreviewMsg { page_num, result }).is_err() {
                break;
            }
        }
    })
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
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn action_guard_state(
        path: PathBuf,
        page_count: usize,
        selected_page: Option<usize>,
    ) -> PdfEditorState {
        let mut state = raster_state(page_count, 1);
        state.path = path;
        state.title = "fixture.pdf".to_owned();
        state.selected_page = selected_page;
        state
    }

    fn write_two_page_pdf(path: &Path) {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 300] /Resources << >> /Contents 4 0 R >>",
            "<< /Length 0 >>\nstream\n\nendstream",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Resources << >> /Contents 6 0 R >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ];
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes
                .extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref_offset = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        bytes.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets.into_iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        std::fs::write(path, bytes).unwrap();
    }

    fn raster_state(page_count: usize, dimension: u32) -> PdfEditorState {
        raster_state_with_dimensions(page_count, (dimension, dimension))
    }

    fn raster_state_with_dimensions(page_count: usize, dimensions: (u32, u32)) -> PdfEditorState {
        let (width, height) = dimensions;
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([23, 45, 67, 255]),
        ));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let raster_cache = std::sync::Arc::new(tempfile::tempdir().unwrap());
        PdfEditorState {
            path: PathBuf::from("fixture.pdf"),
            title: "fixture.pdf".to_owned(),
            page_count,
            pages: (1..=page_count)
                .map(|page_num| PdfPageItem {
                    page_num,
                    raster: {
                        let path = raster_cache.path().join(format!("page-{page_num}.png"));
                        std::fs::write(&path, png.get_ref()).unwrap();
                        Some(test_page_raster(
                            page_num,
                            path,
                            width,
                            height,
                            Arc::clone(&raster_cache),
                        ))
                    },
                    texture: None,
                    texture_last_used: 0,
                    error: None,
                })
                .collect(),
            selected_page: (page_count > 0).then_some(1),
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
            raster_cache: Some(raster_cache),
            source_directory: None,
            texture_frame: 0,
            decode_job: None,
            decode_requests: Vec::new(),
        }
    }

    fn wait_for_decode(state: &mut PdfEditorState, page_num: usize, ctx: &egui::Context) {
        state.decode_requests.clear();
        state.decode_requests.push(page_num);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            state.poll_decode_worker(ctx);
            state.schedule_page_decode(ctx);
            if state
                .decode_job
                .as_ref()
                .is_some_and(|job| job.raster.page_num() == page_num && job.ready.is_some())
                || state.pages[page_num - 1].error.is_some()
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "decode did not complete"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn show_inline_frame(ctx: &egui::Context, preview: &mut InlinePdfPagePreview) {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                preview.show(ui, ctx, Vec2::new(64.0, 64.0));
            });
        });
    }

    fn wait_for_inline_texture(ctx: &egui::Context, preview: &mut InlinePdfPagePreview) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            show_inline_frame(ctx, preview);
            if preview.texture.is_some() || preview.error.is_some() {
                assert!(
                    preview.texture.is_some(),
                    "inline PDF decode failed: {:?}",
                    preview.error
                );
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "inline PDF decode did not complete"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn upload_page(
        state: &mut PdfEditorState,
        page_num: usize,
        ctx: &egui::Context,
    ) -> egui::TextureId {
        wait_for_decode(state, page_num, ctx);
        state.ensure_page_texture(page_num, ctx).unwrap()
    }

    fn resident_textures(state: &PdfEditorState) -> (usize, usize) {
        state
            .pages
            .iter()
            .filter_map(|page| page.texture.as_ref())
            .fold((0, 0), |(count, bytes), texture| {
                (count + 1, bytes + texture.byte_size())
            })
    }

    fn test_page_raster(
        page_num: usize,
        path: PathBuf,
        width: u32,
        height: u32,
        raster_directory: Arc<tempfile::TempDir>,
    ) -> Arc<PdfPageRasterResource> {
        Arc::new(PdfPageRasterResource {
            page_num,
            path,
            width,
            height,
            _raster_directory: raster_directory,
        })
    }

    #[test]
    fn dropping_editor_releases_temporary_raster_cache() {
        let state = raster_state(2, 16);
        let cache_path = state.raster_cache.as_ref().unwrap().path().to_owned();
        assert!(cache_path.exists());

        drop(state);

        assert!(!cache_path.exists());
    }

    #[test]
    fn shared_page_raster_keeps_png_alive_until_last_resource_drops() {
        let state = raster_state(1, 16);
        let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
        let raster_path = raster.path().to_owned();
        let raster_directory = raster_path.parent().unwrap().to_owned();
        assert_eq!(raster.page_num(), 1);
        assert_eq!(raster.dimensions(), (16, 16));

        drop(state);

        assert!(raster_directory.exists());
        let decoded = decode_page_png(std::fs::read(&raster_path).unwrap()).unwrap();
        assert_eq!(decoded.size, [16, 16]);

        drop(raster);
        assert!(!raster_directory.exists());
    }

    #[test]
    fn inline_and_editor_share_raster_with_independent_decode_and_texture_state() {
        let ctx = egui::Context::default();
        let mut state = raster_state(1, 16);
        let editor_raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
        let raster_path = editor_raster.path().to_owned();
        let raster_directory = raster_path.parent().unwrap().to_owned();
        let mut inline = InlinePdfPagePreview::new(Arc::clone(&editor_raster));
        assert!(Arc::ptr_eq(
            state.pages[0].raster.as_ref().unwrap(),
            &inline.raster
        ));

        state.ensure_page_texture(1, &ctx);
        state.schedule_page_decode(&ctx);
        show_inline_frame(&ctx, &mut inline);
        let editor_slot = state.decode_job.as_ref().unwrap();
        let inline_slot = inline.decode_slot.as_ref().unwrap();
        assert!(!std::ptr::eq(editor_slot, inline_slot));
        assert!(Arc::ptr_eq(&editor_slot.raster, &inline_slot.raster));

        let editor_texture = upload_page(&mut state, 1, &ctx);
        wait_for_inline_texture(&ctx, &mut inline);
        let inline_texture = inline.texture.as_ref().unwrap().id();
        assert_ne!(editor_texture, inline_texture);

        drop(inline);
        assert!(raster_path.exists());
        state.pages[0].texture = None;
        upload_page(&mut state, 1, &ctx);
        drop(editor_raster);
        drop(state);
        assert!(!raster_directory.exists());
    }

    #[test]
    fn gallery_page_opens_editor_without_raster_workers_and_retains_source_files() {
        let (pdf_path, raster) = gallery_pdf_page_source().unwrap();
        let raster_path = raster.path().to_owned();
        let directory = pdf_path.parent().unwrap().to_owned();
        assert_eq!(get_pdf_info(&pdf_path).unwrap().page_count, 1);

        let mut inline = InlinePdfPagePreview::new(Arc::clone(&raster));
        let mut editor =
            PdfEditorState::from_shared_page_raster(pdf_path.clone(), Arc::clone(&raster));
        assert!(Arc::ptr_eq(
            editor.pages[0].raster.as_ref().unwrap(),
            &inline.raster
        ));
        assert!(editor.info_job.is_none());
        assert!(editor.preview_job.is_none());
        assert!(editor.preview_rx.is_none());
        assert!(!editor.previews_loading);
        assert_eq!(editor.selected_page, Some(1));

        let ctx = egui::Context::default();
        editor.ensure_page_texture(1, &ctx);
        editor.schedule_page_decode(&ctx);
        show_inline_frame(&ctx, &mut inline);
        assert!(!std::ptr::eq(
            editor.decode_job.as_ref().unwrap(),
            inline.decode_slot.as_ref().unwrap()
        ));
        let editor_texture = upload_page(&mut editor, 1, &ctx);
        wait_for_inline_texture(&ctx, &mut inline);
        assert_ne!(editor_texture, inline.texture.as_ref().unwrap().id());

        drop(raster);
        drop(inline);
        assert!(pdf_path.exists());
        assert!(raster_path.exists());

        // A successful edit reloads metadata and clears the original page
        // raster. The adopted document must stay alive through that reload.
        let output = editor.sibling_output_path("rotate");
        let staging_dir = tempfile::tempdir_in(&directory).unwrap();
        std::fs::copy(&pdf_path, staging_dir.path().join(STAGED_PDF_FILENAME)).unwrap();
        let (sender, receiver) = channel();
        editor.edit_rx = Some(receiver);
        editor.edit_in_progress = true;
        sender
            .send(Ok(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Ok(()),
            }))
            .unwrap();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            editor.poll_edit_worker(ctx, 0.0)
        });
        assert_eq!(editor.path, output);
        assert!(editor.path.is_file());
        assert!(editor.info_job.is_some());
        drop(editor);
        assert!(!directory.exists());

        let (pdf_path, raster) = gallery_pdf_page_source().unwrap();
        let directory = pdf_path.parent().unwrap().to_owned();
        let inline = InlinePdfPagePreview::new(Arc::clone(&raster));
        let editor = PdfEditorState::from_shared_page_raster(pdf_path.clone(), raster);
        drop(editor);
        assert!(pdf_path.exists());
        drop(inline);
        assert!(!directory.exists());
    }

    #[test]
    fn inline_preview_decodes_after_editor_drops_and_owns_raster_until_drop() {
        let ctx = egui::Context::default();
        let state = raster_state(1, 16);
        let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
        let raster_path = raster.path().to_owned();
        let raster_directory = raster_path.parent().unwrap().to_owned();
        let mut inline = InlinePdfPagePreview::new(raster);
        drop(state);

        assert!(raster_path.exists());
        wait_for_inline_texture(&ctx, &mut inline);
        assert!(std::fs::read(&raster_path).is_ok());
        assert!(inline.texture.is_some());

        drop(inline);
        assert!(!raster_directory.exists());
    }

    #[test]
    fn inline_preview_fits_non_square_pdf_raster_without_distortion() {
        let state = raster_state_with_dimensions(1, (100, 200));
        let raster = state.pages[0].raster.as_ref().unwrap();
        let region = egui::Rect::from_min_size(egui::Pos2::new(10.0, 20.0), Vec2::splat(100.0));

        assert_eq!(raster.dimensions(), (100, 200));
        let fitted = fitted_pdf_page_rect(region, raster.dimensions());
        assert!((fitted.width() - 50.0).abs() < 0.001);
        assert!((fitted.height() - 100.0).abs() < 0.001);
        assert_eq!(fitted.center(), region.center());
    }

    #[test]
    fn inline_preview_defers_retry_until_deadline_and_does_not_admit_hidden_page() {
        let ctx = egui::Context::default();
        let state = raster_state(1, 16);
        let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
        let mut retrying = InlinePdfPagePreview::new(Arc::clone(&raster));
        retrying.retry_at = Some(std::time::Instant::now() + Duration::from_secs(1));

        for _ in 0..3 {
            show_inline_frame(&ctx, &mut retrying);
        }
        assert!(retrying.decode_slot.is_none());

        let mut hidden = InlinePdfPagePreview::new(raster);

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(egui::Rect::NOTHING);
                hidden.show(ui, ctx, Vec2::new(64.0, 64.0));
            });
        });
        assert!(hidden.decode_slot.is_none());
    }

    #[test]
    fn decode_requests_are_bounded_deduplicated_and_selected_first() {
        let ctx = egui::Context::default();
        let mut state = raster_state(100, 16);
        state.selected_page = Some(100);
        for _ in 0..3 {
            for page in 1..=100 {
                assert!(state.ensure_page_texture(page, &ctx).is_none());
            }
        }
        assert_eq!(state.decode_requests.len(), PAGE_DECODE_REQUEST_LIMIT);
        assert_eq!(
            state
                .decode_requests
                .iter()
                .filter(|page| **page == 1)
                .count(),
            1
        );
        state.schedule_page_decode(&ctx);
        let job = state.decode_job.as_ref().unwrap();
        assert_eq!(job.raster.page_num(), 100);
        let revision = job.ticket.revision();
        for _ in 0..10 {
            state.schedule_page_decode(&ctx);
            assert_eq!(
                state.decode_job.as_ref().unwrap().ticket.revision(),
                revision
            );
        }
    }

    #[test]
    fn malformed_png_sets_page_error_without_repeated_work_or_losing_source() {
        let ctx = egui::Context::default();
        let mut state = raster_state(1, 16);
        let malformed = b"not a PNG".to_vec();
        let raster_path = state.pages[0].raster.as_ref().unwrap().path().to_owned();
        std::fs::write(&raster_path, &malformed).unwrap();
        wait_for_decode(&mut state, 1, &ctx);
        assert!(
            state.pages[0]
                .error
                .as_ref()
                .unwrap()
                .contains("PDF preview decode failed")
        );
        assert_eq!(std::fs::read(raster_path).unwrap(), malformed);
        state.decode_requests.clear();
        for _ in 0..3 {
            assert!(state.ensure_page_texture(1, &ctx).is_none());
            state.schedule_page_decode(&ctx);
            assert!(state.decode_job.is_none());
        }
    }

    // Block a running job so stale publication and admission can be inspected
    // deterministically, without depending on image decoder timing.
    fn blocked_decode(state: &mut PdfEditorState) -> (std::sync::mpsc::Sender<()>, JobTicket) {
        let jobs = BackgroundJobs::new(1, 1);
        let (release, wait) = channel();
        let (started, start) = channel();
        let (ticket, rx) = jobs.submit(Duration::from_secs(10), move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            Err("obsolete decode error".to_owned())
        });
        start.recv_timeout(Duration::from_secs(2)).unwrap();
        let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
        state.decode_job = Some(PdfPageDecodeSlot {
            raster,
            ticket: ticket.clone(),
            rx,
            ready: None,
            cancelled: false,
        });
        (release, ticket)
    }

    #[test]
    fn invisible_work_is_cancelled_and_retains_its_slot_until_acknowledged() {
        let ctx = egui::Context::default();
        let mut state = raster_state(2, 16);
        let (release, ticket) = blocked_decode(&mut state);
        state.selected_page = Some(2);
        state.ensure_page_texture(2, &ctx);
        state.schedule_page_decode(&ctx);
        let job = state.decode_job.as_ref().unwrap();
        assert!(job.cancelled);
        assert_eq!(job.ticket.revision(), ticket.revision());
        state.schedule_page_decode(&ctx);
        assert_eq!(
            state.decode_job.as_ref().unwrap().ticket.revision(),
            ticket.revision()
        );
        release.send(()).unwrap();
        wait_for_decode(&mut state, 2, &ctx);
        assert!(state.pages[0].error.is_none());
        assert!(state.pages[0].texture.is_none());
        assert!(state.ensure_page_texture(2, &ctx).is_some());
    }

    #[test]
    fn dropping_editor_cancels_running_decode_publication() {
        let mut state = raster_state(1, 16);
        let (release, _) = blocked_decode(&mut state);
        let (_, placeholder) = channel();
        let rx = std::mem::replace(&mut state.decode_job.as_mut().unwrap().rx, placeholder);
        drop(state);
        release.send(()).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(JobError::Cancelled)
        ));
    }

    #[test]
    fn oversized_raster_decode_bounds_published_pixel_memory() {
        let state = raster_state(1, PAGE_RASTER_MAX_DIM + 1);
        let raster_path = state.pages[0].raster.as_ref().unwrap().path();
        let image = decode_page_png(std::fs::read(raster_path).unwrap()).unwrap();
        assert_eq!(image.size, [PAGE_RASTER_MAX_DIM as usize; 2]);
        assert_eq!(
            image.pixels.len() * 4,
            PAGE_RASTER_MAX_DIM as usize * PAGE_RASTER_MAX_DIM as usize * 4
        );
    }

    #[test]
    fn replacement_raster_cancels_old_decode_before_publishing() {
        let ctx = egui::Context::default();
        let mut state = raster_state(1, 16);
        let (release, _) = blocked_decode(&mut state);
        let (tx, rx) = channel();
        state.preview_rx = Some(rx);
        let replacement_path = state
            .raster_cache
            .as_ref()
            .unwrap()
            .path()
            .join("replacement.png");
        std::fs::copy(
            state.pages[0].raster.as_ref().unwrap().path(),
            &replacement_path,
        )
        .unwrap();
        let replacement = test_page_raster(
            1,
            replacement_path.clone(),
            16,
            16,
            Arc::clone(state.raster_cache.as_ref().unwrap()),
        );
        tx.send(PdfPreviewMsg {
            page_num: 1,
            result: Ok(replacement),
        })
        .unwrap();
        state.poll_preview_worker(&ctx);
        assert!(state.decode_job.as_ref().unwrap().cancelled);
        release.send(()).unwrap();
        wait_for_decode(&mut state, 1, &ctx);
        assert!(state.ensure_page_texture(1, &ctx).is_some());
        assert!(state.pages[0].error.is_none());
        assert_eq!(
            state.pages[0].raster.as_ref().unwrap().path(),
            replacement_path
        );
    }

    #[test]
    fn dropping_preview_receiver_stops_worker_after_bounded_in_flight_page() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rendered = Arc::new(AtomicUsize::new(0));
        let render_count = Arc::clone(&rendered);
        let jobs = BackgroundJobs::new(1, 1);
        let (tx, rx) = sync_channel(PREVIEW_QUEUE_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (ticket, result_rx) = spawn_preview_worker(
            &jobs,
            PathBuf::from("fixture.pdf"),
            100,
            1,
            Arc::new(tempfile::tempdir().unwrap()),
            tx,
            Arc::clone(&cancelled),
            move |_, _, _| {
                render_count.fetch_add(1, Ordering::SeqCst);
                Ok((Vec::new(), 1, 1))
            },
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while rendered.load(Ordering::SeqCst) < PREVIEW_QUEUE_CAPACITY + 1
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(rendered.load(Ordering::SeqCst), PREVIEW_QUEUE_CAPACITY + 1);
        cancelled.store(true, Ordering::Release);
        ticket.cancel();
        drop(rx);
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(JobError::Cancelled)
        );
        assert_eq!(rendered.load(Ordering::SeqCst), PREVIEW_QUEUE_CAPACITY + 1);
    }

    #[test]
    fn saturated_preview_queue_reports_an_error_instead_of_loading_forever() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_, queued_rx) = jobs.submit(Duration::MAX, || ());

        let mut state = raster_state(2, 16);
        for page in &mut state.pages {
            page.raster = None;
        }
        state.start_preview_worker_on(&jobs, |_, _, _| panic!("full queue must not render"));
        state.poll_preview_worker(&egui::Context::default());
        assert!(!state.previews_loading);
        assert!(state.preview_job.is_none());
        assert!(state.pages.iter().all(|page| {
            page.error
                .as_deref()
                .is_some_and(|error| error.contains("background job queue is full"))
        }));

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        queued_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn dropping_editor_cancels_queued_preview_before_rendering() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let render_count = Arc::new(AtomicUsize::new(0));
        let worker_count = Arc::clone(&render_count);
        let mut state = raster_state(1, 16);
        state.start_preview_worker_on(&jobs, move |_, _, _| {
            worker_count.fetch_add(1, Ordering::SeqCst);
            Ok((vec![1], 1, 1))
        });
        let cache_path = state.raster_cache.as_ref().unwrap().path().to_owned();
        drop(state);

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        // A later job in the same single-worker queue proves the cancelled
        // preview has been dequeued, without relying on a scheduling delay.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let (_, rx) = jobs.submit(Duration::MAX, || ());
            match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
                Ok(()) => break,
                Err(JobError::QueueFull) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                result => panic!("sentinel job failed: {result:?}"),
            }
        }
        assert_eq!(render_count.load(Ordering::SeqCst), 0);
        assert!(!cache_path.exists());
    }

    #[test]
    fn active_preview_publishes_selected_page_before_remaining_pages() {
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = raster_state(3, 16);
        state.selected_page = Some(3);
        for page in &mut state.pages {
            page.raster = None;
        }
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        state.start_preview_worker_on(&jobs, move |_, page_num, max_dim| {
            started_tx.send((page_num, max_dim)).unwrap();
            if page_num == 1 {
                release_rx.recv().unwrap();
            }
            Ok((vec![page_num as u8], 1, 1))
        });
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            (3, PAGE_RASTER_MAX_DIM)
        );
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while state.pages[2].raster.is_none() && std::time::Instant::now() < deadline {
            state.poll_preview_worker(&ctx);
            std::thread::yield_now();
        }
        assert!(state.pages[2].raster.is_some());
        assert!(state.pages[0].raster.is_none());
        assert!(state.pages[1].raster.is_none());
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            (1, 360)
        );

        release_tx.send(()).unwrap();
        while state.previews_loading && std::time::Instant::now() < deadline {
            state.poll_preview_worker(&ctx);
            std::thread::yield_now();
        }
        assert!(!state.previews_loading);
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            (2, 360)
        );
        assert!(state.pages.iter().all(|page| {
            page.raster
                .as_ref()
                .is_some_and(|raster| raster.path().exists())
        }));
    }

    #[test]
    fn cancelling_active_render_discards_stale_page_and_releases_cache() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let jobs = BackgroundJobs::new(1, 1);
        let rendered = Arc::new(AtomicUsize::new(0));
        let render_count = Arc::clone(&rendered);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let mut state = raster_state(2, 16);
        for page in &mut state.pages {
            page.raster = None;
        }
        state.start_preview_worker_on(&jobs, move |_, _, _| {
            render_count.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok((vec![1], 1, 1))
        });
        let cache = Arc::clone(state.raster_cache.as_ref().unwrap());
        let cache_path = cache.path().to_owned();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        state.cancel_preview_worker();
        let (_, sentinel_rx) = jobs.submit(Duration::MAX, || ());
        release_tx.send(()).unwrap();
        sentinel_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(rendered.load(Ordering::SeqCst), 1);
        state.poll_preview_worker(&egui::Context::default());
        assert!(state.pages.iter().all(|page| page.raster.is_none()));
        assert!(!cache.path().join("page-1.png").exists());
        drop(state);
        drop(cache);
        assert!(!cache_path.exists());
    }

    #[test]
    fn page_textures_obey_handle_and_byte_limits_while_selected_page_stays_resident() {
        let ctx = egui::Context::default();
        for dimension in [16, PAGE_RASTER_MAX_DIM] {
            let mut state = raster_state(PAGE_TEXTURE_LIMIT * 2, dimension);
            state.texture_frame += 1;
            let mut selected_id = None;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                selected_id = Some(upload_page(&mut state, 1, ctx));
            });
            let selected_id = selected_id.unwrap();
            for page_num in 2..=state.page_count {
                state.texture_frame += 1;
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    upload_page(&mut state, page_num, ctx);
                });
                let (count, bytes) = resident_textures(&state);
                assert!(count <= PAGE_TEXTURE_LIMIT);
                assert!(bytes <= PAGE_TEXTURE_BYTE_LIMIT);
                assert_eq!(state.pages[0].texture.as_ref().unwrap().id(), selected_id);
            }
            assert!(state.pages[1].texture.is_none());
        }
    }

    #[test]
    fn evicted_page_reselection_reuploads_original_raster() {
        let ctx = egui::Context::default();
        let mut state = raster_state(PAGE_TEXTURE_LIMIT + 2, 16);
        state.texture_frame += 1;
        let original_id = upload_page(&mut state, 2, &ctx);
        let source_path = state.pages[1].raster.as_ref().unwrap().path().to_owned();
        let source = std::fs::read(&source_path).unwrap();
        for page_num in 3..=state.page_count {
            state.texture_frame += 1;
            upload_page(&mut state, page_num, &ctx);
        }
        assert!(state.pages[1].texture.is_none());
        state.selected_page = Some(2);
        state.texture_frame += 1;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            let reloaded = upload_page(&mut state, 2, ctx);
            assert_ne!(reloaded, original_id);
        });
        assert_eq!(std::fs::read(source_path).unwrap(), source);
        let texture = state.pages[1].texture.as_ref().unwrap();
        assert_eq!(texture.size(), [16, 16]);
        let upload = output
            .textures_delta
            .set
            .iter()
            .find(|(id, _)| *id == texture.id())
            .unwrap();
        match &upload.1.image {
            egui::ImageData::Color(image) => {
                assert!(
                    image
                        .pixels
                        .iter()
                        .all(|pixel| *pixel == egui::Color32::from_rgb(23, 45, 67))
                );
            }
            _ => panic!("PDF reload must upload the original RGBA raster"),
        }
        let (count, bytes) = resident_textures(&state);
        assert!(count <= PAGE_TEXTURE_LIMIT && bytes <= PAGE_TEXTURE_BYTE_LIMIT);
    }

    #[test]
    fn current_frame_textures_are_not_evicted_and_next_frame_admits_new_selection() {
        let ctx = egui::Context::default();
        let mut state = raster_state(PAGE_TEXTURE_LIMIT + 1, 16);
        state.texture_frame += 1;
        let ids: Vec<_> = (1..=PAGE_TEXTURE_LIMIT)
            .map(|page_num| upload_page(&mut state, page_num, &ctx))
            .collect();
        state.selected_page = Some(PAGE_TEXTURE_LIMIT + 1);
        wait_for_decode(&mut state, PAGE_TEXTURE_LIMIT + 1, &ctx);
        assert!(
            state
                .ensure_page_texture(PAGE_TEXTURE_LIMIT + 1, &ctx)
                .is_none()
        );
        for (page, id) in state.pages.iter().zip(ids) {
            assert_eq!(page.texture.as_ref().unwrap().id(), id);
        }
        assert_eq!(resident_textures(&state).0, PAGE_TEXTURE_LIMIT);
        state.texture_frame += 1;
        assert!(
            state
                .ensure_page_texture(PAGE_TEXTURE_LIMIT + 1, &ctx)
                .is_some()
        );
        assert_eq!(resident_textures(&state).0, PAGE_TEXTURE_LIMIT);
    }

    #[test]
    fn long_document_uploads_visible_thumbnails_and_selected_page_only() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut state = raster_state(100, 16);
        state.selected_page = Some(100);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1177.0, 662.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            if state.decode_job.is_none() && resident_textures(&state).0 > 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "visible decodes did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let (count, bytes) = resident_textures(&state);
        assert!(count > 1 && count < PAGE_TEXTURE_LIMIT);
        assert!(bytes <= PAGE_TEXTURE_BYTE_LIMIT);
        assert!(state.pages[99].texture.is_some());
        assert!(state.pages[50].texture.is_none());
        assert!(state.pages.iter().all(|page| page.raster.is_some()));
    }

    #[test]
    fn pdf_selection_and_error_labels_meet_contrast_in_both_appearances() {
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = crate::ui_appearance::palette(appearance);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            for (surface, preferred) in [
                (colors.panel, colors.accent),
                (colors.recessed, colors.danger),
            ] {
                let foreground = crate::ui_ext::contrast_text_c32_on(
                    surface,
                    surface,
                    Some(preferred),
                    &palette,
                );
                assert!(
                    little_clip_core::palette::contrast_ratio(
                        (surface.r(), surface.g(), surface.b()),
                        (foreground.r(), foreground.g(), foreground.b()),
                    ) >= 4.5,
                    "{appearance:?} PDF custom label lacks contrast"
                );
            }
        }
    }

    #[test]
    fn pdf_sibling_panes_stay_inside_compact_and_wide_hosts() {
        for host_size in [Vec2::new(1177.0, 662.0), Vec2::new(1766.0, 993.0)] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            let mut state = PdfEditorState {
                path: PathBuf::from("fixture.pdf"),
                title: "fixture.pdf".to_owned(),
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
            };
            let host = egui::Rect::from_min_size(egui::Pos2::ZERO, host_size);
            let mut panes = Vec::new();
            let mut pane_width_budget = 0.0;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(host),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        pane_width_budget =
                            ui.available_width() - 24.0 - (ui.spacing().item_spacing.x * 2.0 + 1.0);
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Rect(rect) = &clipped.shape {
                    if rect.fill == colors.recessed && rect.rect.height() > 100.0 {
                        panes.push(rect.rect);
                    }
                }
            }
            panes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
            assert_eq!(
                panes.len(),
                2,
                "expected left/right pane frames at {host_size:?}"
            );
            assert!(
                panes[0].right() < panes[1].left(),
                "panes overlap: {panes:?}"
            );
            assert!(panes.iter().all(|pane| host.contains_rect(*pane)));
            let pane_widths = panes.iter().map(|pane| pane.width()).sum::<f32>();
            assert!(
                (pane_widths - pane_width_budget).abs() < 1.0,
                "panes do not fill their divider and gap-adjusted budget: {pane_widths} vs {pane_width_budget}"
            );
        }
    }

    #[test]
    fn selected_pdf_page_preview_stays_inside_compact_and_wide_panes() {
        for host_size in [Vec2::new(1177.0, 662.0), Vec2::new(1766.0, 993.0)] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            let mut state = raster_state(2, 16);
            for page in &mut state.pages {
                let raster = page.raster.as_ref().unwrap();
                page.raster = Some(test_page_raster(
                    page.page_num,
                    raster.path().to_owned(),
                    1200,
                    800,
                    Arc::clone(state.raster_cache.as_ref().unwrap()),
                ));
            }
            let texture = ctx.load_texture(
                "pdf-preview-fit-test",
                egui::ColorImage::new([16, 16], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            );
            let texture_id = texture.id();
            state.pages[0].texture = Some(texture);

            let host = egui::Rect::from_min_size(egui::Pos2::ZERO, host_size);
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(host),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );

            let mut panes = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Rect(rect)
                        if rect.fill == colors.recessed && rect.rect.height() > 100.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            panes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
            let preview_pane = *panes.get(1).expect("right preview pane should render");
            let preview_rect = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Rect(rect) if rect.fill_texture_id == texture_id => {
                        Some(rect.rect)
                    }
                    egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture_id => Some(
                        mesh.vertices
                            .iter()
                            .fold(egui::Rect::NOTHING, |mut bounds, vertex| {
                                bounds.extend_with(vertex.pos);
                                bounds
                            }),
                    ),
                    _ => None,
                })
                .max_by(|a, b| a.area().total_cmp(&b.area()))
                .expect("selected page texture should be painted");
            assert!(
                host.contains_rect(preview_pane),
                "selected PDF preview pane escapes host at {host_size:?}: pane={preview_pane:?}"
            );
            assert!(
                preview_pane.contains_rect(preview_rect),
                "selected page preview escapes its pane at {host_size:?}: page={preview_rect:?}, pane={preview_pane:?}"
            );
        }
    }

    #[test]
    fn pdf_delete_and_rotate_wait_for_metadata_before_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(1));
        let jobs = BackgroundJobs::new(1, 1);
        let (ticket, receiver) = jobs.submit(Duration::MAX, || {
            Ok(PdfInfo {
                page_count: 2,
                title: Some("fixture".into()),
                author: None,
                page_size: None,
            })
        });
        state.info_job = Some((ticket, receiver));

        state.delete_current_page(1.0);
        state.rotate_current_page(90, 2.0);

        assert!(state.info_job.is_some(), "metadata load remains pending");
        assert!(!state.edit_in_progress);
        assert!(
            state.edit_rx.is_none(),
            "guarded actions must not dispatch edits"
        );
        let (ticket, _) = state.info_job.take().unwrap();
        ticket.cancel();
    }

    #[test]
    fn saturated_pdf_edit_queue_reports_error_and_cleans_staging() {
        let temp = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_, queued_rx) = jobs.submit(Duration::MAX, || ());

        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(1));
        state.start_edit_on(&jobs, "rotate", 1, 1.0, |_, _| {
            panic!("full queue must not run a PDF edit")
        });
        state.poll_edit_worker(&egui::Context::default(), 1.0);
        assert!(!state.edit_in_progress);
        assert!(state.edit_ticket.is_none());
        assert!(state.edit_rx.is_none());
        assert_eq!(
            state.status_toast.message(1.0),
            Some("PDF edit unavailable: background job queue is full")
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        queued_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn dropping_editor_cancels_queued_pdf_edit_before_dispatch() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(1));
        state.start_edit_on(&jobs, "rotate", 1, 1.0, move |_, _| {
            worker_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(state.edit_in_progress);
        drop(state);

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let (_, rx) = jobs.submit(Duration::MAX, || ());
            match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
                Ok(()) => break,
                Err(JobError::QueueFull) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                result => panic!("sentinel job failed: {result:?}"),
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!temp.path().join("fixture.rotate.edited.pdf").exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn closing_editor_during_active_pdf_edit_keeps_existing_sibling_output() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("fixture.pdf");
        let output = temp.path().join("fixture.rotate.edited.pdf");
        write_two_page_pdf(&source);
        std::fs::write(&output, b"previous edit").unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let mut state = action_guard_state(source.clone(), 2, Some(2));
        state.start_edit_on(&jobs, "rotate", 2, 1.0, move |_, staged_output| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            worker_calls.fetch_add(1, Ordering::SeqCst);
            write_two_page_pdf(staged_output);
            Ok(())
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(state);

        let (_, sentinel_rx) = jobs.submit(Duration::MAX, || ());
        release_tx.send(()).unwrap();
        sentinel_rx
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read(&output).unwrap(), b"previous edit");
        assert!(
            std::fs::read_dir(temp.path())
                .unwrap()
                .all(|entry| entry.unwrap().path().is_file())
        );
    }

    #[test]
    fn pdf_delete_and_rotate_without_selection_do_not_dispatch_edits() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, None);

        state.delete_current_page(1.0);
        state.rotate_current_page(90, 2.0);

        assert!(!state.edit_in_progress);
        assert!(state.edit_rx.is_none());
    }

    #[test]
    fn pdf_delete_and_rotate_do_not_replace_an_active_edit() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(2));
        let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
        let (sender, receiver) = channel();
        state.edit_rx = Some(receiver);
        state.edit_in_progress = true;

        state.delete_current_page(1.0);
        state.rotate_current_page(90, 2.0);

        assert!(state.edit_in_progress);
        assert_eq!(
            state.status_toast.message(2.0),
            Some("A PDF edit is already in progress")
        );
        assert!(state.edit_rx.is_some());
        assert!(
            sender
                .send(Ok(PdfEditMsg {
                    operation: "sentinel",
                    page_num: 2,
                    output: temp.path().join("untouched.pdf"),
                    staging_dir,
                    result: Err("sentinel only".into()),
                }))
                .is_ok(),
            "the original edit receiver should remain connected"
        );
        assert!(!temp.path().join("untouched.pdf").exists());
    }

    #[test]
    fn pdf_delete_only_page_keeps_feedback_without_dispatching_edit() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 1, Some(1));

        state.delete_current_page(3.5);

        assert_eq!(
            state.status_toast.message(3.5),
            Some("Cannot delete only page from PDF")
        );
        assert!(!state.edit_in_progress);
        assert!(state.edit_rx.is_none());
        assert!(!temp.path().join("fixture.delete.edited.pdf").exists());
    }

    #[test]
    fn pdf_keyboard_page_actions_reach_the_guarded_selected_page_methods() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut state = action_guard_state(temp.path().join("fixture.pdf"), 1, Some(1));

        for key in [egui::Key::Delete, egui::Key::Backspace] {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(900.0, 600.0),
                    )),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::default(),
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );

            assert_eq!(
                state.status_toast.message(now),
                Some("Cannot delete only page from PDF"),
                "{key:?} should reach delete for selected page 1"
            );
            assert!(!state.edit_in_progress);
            assert!(state.edit_rx.is_none());
        }

        // An active edit is a safe guard for Rotate: the keyboard route reaches
        // the selected-page action but cannot start another PDF operation.
        let (_sender, receiver) = channel();
        state.page_count = 2;
        state.pages.push(PdfPageItem {
            page_num: 2,
            raster: None,
            texture: None,
            texture_last_used: 0,
            error: None,
        });
        state.selected_page = Some(2);
        state.edit_rx = Some(receiver);
        state.edit_in_progress = true;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 600.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::R,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        assert_eq!(
            state.status_toast.message(now),
            Some("A PDF edit is already in progress"),
            "R should reach rotate for the selected page"
        );
        assert!(state.edit_in_progress);
        assert!(state.edit_rx.is_some());
    }

    #[test]
    fn idle_r_rotates_selected_pdf_page_and_adopts_sibling_output() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("fixture.pdf");
        let output = temp.path().join("fixture.rotate.edited.pdf");
        write_two_page_pdf(&source);
        let source_bytes = std::fs::read(&source).unwrap();
        assert_eq!(get_pdf_info(&source).unwrap().page_count, 2);
        let (_, source_page_width, source_page_height) =
            render_pdf_page_preview(&source, 2, 64).expect("render original page two");
        assert!(source_page_width > source_page_height);

        let mut state = action_guard_state(source.clone(), 2, Some(2));
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 600.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::R,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        assert!(
            state.edit_in_progress,
            "idle R should dispatch the selected-page edit"
        );
        assert_eq!(state.selected_page, Some(2));

        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while state.edit_in_progress && std::time::Instant::now() < deadline {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                state.poll_edit_worker(ctx, 1.0)
            });
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!state.edit_in_progress, "PDF rotate edit did not finish");
        assert_eq!(state.path, output);
        assert!(output.is_file());
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(get_pdf_info(&output).unwrap().page_count, 2);
        assert_eq!(state.selected_page, Some(2));
        let (_, rotated_page_width, rotated_page_height) =
            render_pdf_page_preview(&output, 2, 64).expect("render rotated page two");
        assert!(rotated_page_width < rotated_page_height);

        while state.info_job.is_some() && std::time::Instant::now() < deadline {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                state.poll_info_worker(ctx, 1.0)
            });
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            state.info_job.is_none(),
            "edited PDF metadata did not reload"
        );
        assert_eq!(state.selected_page, Some(2));
        assert!(!close);
    }

    #[test]
    fn escape_during_pdf_edit_keeps_session_until_synthetic_result_arrives() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.pdf");
        let mut state = action_guard_state(path.clone(), 0, None);
        let (sender, receiver) = channel();
        state.edit_rx = Some(receiver);
        state.edit_in_progress = true;
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        state.status_toast.set("Rotating page 1…", started_at);

        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        let mut close = false;
        let busy_output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 600.0),
                )),
                events: vec![escape.clone()],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );

        assert!(!close, "Escape must not drop a running PDF edit");
        assert_eq!(state.path, path);
        assert!(state.edit_in_progress);
        assert!(state.edit_rx.is_some());
        assert!(
            busy_output.shapes.iter().any(|clipped| {
                matches!(
                    &clipped.shape,
                    egui::Shape::Text(text)
                        if text.galley.text().contains("PDF edit is still running")
                )
            }),
            "busy feedback should be rendered in the viewer footer"
        );
        let feedback_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert_eq!(
            state.status_toast.message(feedback_time),
            Some("PDF edit is still running; wait for it to finish")
        );

        sender
            .send(Ok(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: temp.path().join("source.rotate.edited.pdf"),
                staging_dir: tempfile::tempdir_in(temp.path()).unwrap(),
                result: Err("synthetic worker failure".to_owned()),
            }))
            .unwrap();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        assert!(!state.edit_in_progress);
        assert!(state.edit_rx.is_none());
        assert_eq!(state.path, path, "failed edit keeps the source document");
        let feedback_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert_eq!(
            state.status_toast.message(feedback_time),
            Some("Failed to rotate page: synthetic worker failure")
        );

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 600.0),
                )),
                events: vec![escape],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        assert!(
            close,
            "Escape may close the session after the edit resolves"
        );
    }

    #[test]
    fn missing_pdf_metadata_completes_after_initial_view_creation() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = PdfEditorState::new(temp.path().join("missing.pdf"));
        assert_eq!(state.page_count, 0);
        assert!(state.info_job.is_some());

        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while state.info_job.is_some() && std::time::Instant::now() < deadline {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                state.poll_info_worker(ctx, 0.0)
            });
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(state.info_job.is_none(), "PDF metadata job did not finish");
        assert_eq!(state.page_count, 1);
        assert_eq!(state.selected_page, Some(1));
        assert!(state.status_toast.message(0.0).is_some());
    }

    #[test]
    fn failed_page_raster_is_reported_instead_of_spinning_forever() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = PdfEditorState::new(temp.path().join("missing.pdf"));
        let (sender, receiver) = channel();
        state.preview_rx = Some(receiver);
        state.previews_loading = true;
        state.pages.push(PdfPageItem {
            page_num: 1,
            raster: None,
            texture: None,
            texture_last_used: 0,
            error: None,
        });
        sender
            .send(PdfPreviewMsg {
                page_num: 1,
                result: Err("raster failed".to_owned()),
            })
            .unwrap();
        drop(sender);

        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.poll_preview_worker(ctx)
        });
        assert_eq!(state.pages[0].error.as_deref(), Some("raster failed"));
        assert!(!state.previews_loading);
    }

    #[test]
    fn closed_viewer_discards_completed_edit_before_publication() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("document.rotate.edited.pdf");
        let mut state = PdfEditorState::new(temp.path().join("document.pdf"));
        let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
        let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
        std::fs::write(&staged_output, b"staged edit").unwrap();
        let (sender, receiver) = channel();
        state.edit_rx = Some(receiver);
        sender
            .send(Ok(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Ok(()),
            }))
            .unwrap();
        drop(state);

        assert!(!staged_output.exists());
        assert!(!output.exists());
    }

    #[test]
    fn failed_edit_preserves_existing_sibling_output() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("document.rotate.edited.pdf");
        std::fs::write(&output, b"prior edit").unwrap();
        let mut state = PdfEditorState::new(temp.path().join("document.pdf"));
        let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
        let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
        std::fs::write(&staged_output, b"failed edit").unwrap();
        let (sender, receiver) = channel();
        state.edit_rx = Some(receiver);
        state.edit_in_progress = true;
        sender
            .send(Ok(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Err("render failed".to_owned()),
            }))
            .unwrap();

        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.poll_edit_worker(ctx, 0.0)
        });
        assert_eq!(std::fs::read(output).unwrap(), b"prior edit");
        assert!(!staged_output.exists());
        assert!(!state.edit_in_progress);
    }

    #[test]
    fn validated_edit_publishes_sibling_output_from_open_viewer() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("document.rotate.edited.pdf");
        let mut state = PdfEditorState::new(temp.path().join("document.pdf"));
        let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
        let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
        std::fs::write(&staged_output, b"validated edit").unwrap();
        let (sender, receiver) = channel();
        state.edit_rx = Some(receiver);
        state.edit_in_progress = true;
        sender
            .send(Ok(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Ok(()),
            }))
            .unwrap();

        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.poll_edit_worker(ctx, 0.0)
        });
        assert_eq!(std::fs::read(&output).unwrap(), b"validated edit");
        assert!(!staged_output.exists());
        assert_eq!(state.path, output);
        assert!(state.dirty);
    }
}
