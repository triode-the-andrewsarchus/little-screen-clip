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
use std::sync::mpsc::{Receiver, TryRecvError, channel, sync_channel};
use std::time::Duration;

const PREVIEW_QUEUE_CAPACITY: usize = 3;
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
    result: Result<(Vec<u8>, u32, u32), String>,
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

/// Rendered raster thumbnail and metadata for a single PDF page.
pub struct PdfPageItem {
    pub page_num: usize,
    pub png_bytes: Vec<u8>,
    pub texture: Option<egui::TextureHandle>,
    texture_last_used: u64,
    pub error: Option<String>,
    pub width: u32,
    pub height: u32,
    #[allow(dead_code)]
    pub rotation: u32,
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
    previews_loading: bool,
    edit_rx: Option<Receiver<PdfEditMsg>>,
    edit_in_progress: bool,
    texture_frame: u64,
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

    /// Starts loading page metadata and thumbnail previews for the given PDF file.
    pub fn new(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        let mut state = Self {
            path,
            title,
            page_count: 0,
            pages: Vec::new(),
            selected_page: None,
            status_toast: StatusToast::new(),
            dirty: false,
            info_job: None,
            preview_rx: None,
            previews_loading: false,
            edit_rx: None,
            edit_in_progress: false,
            texture_frame: 0,
        };
        state.start_info_worker();
        state
    }

    fn start_info_worker(&mut self) {
        if let Some((ticket, _)) = self.info_job.take() {
            ticket.cancel();
        }
        self.preview_rx = None;
        self.previews_loading = false;
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
                png_bytes: Vec::new(),
                texture: None,
                texture_last_used: 0,
                error: None,
                width: 0,
                height: 0,
                rotation: 0,
            })
            .collect();
        self.selected_page =
            (self.page_count > 0).then(|| self.selected_page.unwrap_or(1).min(self.page_count));
        self.start_preview_worker();
        ctx.request_repaint();
    }

    fn start_preview_worker(&mut self) {
        let path = self.path.clone();
        let page_count = self.page_count;
        let selected_page = self.selected_page.unwrap_or(1).min(page_count.max(1));
        // Bound pending raster data if rendering outruns the UI. Dropping this
        // receiver on a route change also stops the old worker at its next send.
        let (tx, rx) = sync_channel(PREVIEW_QUEUE_CAPACITY);
        self.preview_rx = Some(rx);
        self.previews_loading = true;
        std::thread::spawn(move || {
            // A large selected-page raster arrives first; thumbnails then fill
            // in incrementally. This keeps long manuals responsive.
            let mut order = Vec::with_capacity(page_count);
            if page_count > 0 {
                order.push(selected_page);
            }
            order.extend((1..=page_count).filter(|page| *page != selected_page));
            for page_num in order {
                let max_dim = if page_num == selected_page {
                    PAGE_RASTER_MAX_DIM
                } else {
                    360
                };
                let result = render_pdf_page_preview(&path, page_num, max_dim);
                if tx.send(PdfPreviewMsg { page_num, result }).is_err() {
                    break;
                }
            }
        });
    }

    fn poll_preview_worker(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.preview_rx {
            loop {
                match rx.try_recv() {
                    Ok(message) => {
                        if let Some(page) = self.pages.get_mut(message.page_num.saturating_sub(1)) {
                            match message.result {
                                Ok((png_bytes, width, height)) => {
                                    page.texture = None;
                                    page.png_bytes = png_bytes;
                                    page.width = width;
                                    page.height = height;
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
        if let Some(texture) = &page.texture {
            page.texture_last_used = self.texture_frame;
            return Some(texture.id());
        }
        if page.png_bytes.is_empty() {
            return None;
        }
        let image = match image::load_from_memory(&page.png_bytes) {
            Ok(image) => image,
            Err(error) => {
                page.error = Some(format!("PDF preview decode failed: {error}"));
                page.png_bytes.clear();
                return None;
            }
        };
        // Workers already render at this limit; defend the cache budget if a
        // future producer supplies a larger raster.
        let image = if image.width().max(image.height()) > PAGE_RASTER_MAX_DIM {
            image.thumbnail(PAGE_RASTER_MAX_DIM, PAGE_RASTER_MAX_DIM)
        } else {
            image
        };
        let rgba = image.to_rgba8();
        let bytes = rgba.as_raw().len();
        if !self.make_texture_room(bytes) {
            // Newly selected pages get first priority on the next frame. Avoid
            // freeing a texture that a thumbnail has already painted this frame.
            if self.selected_page == Some(page_num) {
                ctx.request_repaint();
            }
            return None;
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        );
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
        let (tx, rx) = channel();
        self.edit_in_progress = true;
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
        std::thread::spawn(move || {
            let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
            let result = match operation {
                "delete" => delete_pdf_pages(&input, &[page_num], &staged_output),
                "rotate" => rotate_pdf_page(&input, page_num, degrees, &staged_output),
                _ => Err("Unknown PDF edit operation".to_string()),
            }
            .and_then(|()| {
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
            let _ = tx.send(PdfEditMsg {
                operation,
                page_num,
                output,
                staging_dir,
                result,
            });
        });
    }

    fn poll_edit_worker(&mut self, ctx: &egui::Context, now: f64) {
        let message = match self.edit_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(message)) => Some(message),
            Some(Err(TryRecvError::Disconnected)) => {
                self.edit_rx = None;
                self.edit_in_progress = false;
                self.status_toast.set("PDF edit worker disconnected", now);
                return;
            }
            _ => None,
        };
        if let Some(message) = message {
            self.edit_rx = None;
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
            *close = true;
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            self.delete_current_page(now);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.rotate_current_page(90, now);
        }

        self.texture_frame += 1;
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
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                let pages_clone: Vec<(
                                                    usize,
                                                    u32,
                                                    u32,
                                                    bool,
                                                )> = self
                                                    .pages
                                                    .iter()
                                                    .map(|p| {
                                                        (
                                                            p.page_num,
                                                            p.width,
                                                            p.height,
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
                                                                let thumb_h = 130.0;
                                                                let thumb_w = thumb_h * aspect;

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
                                                    page.width, page.height
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

                                        let aspect = if page.height > 0 {
                                            page.width as f32 / page.height as f32
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
    }
}

impl Drop for PdfEditorState {
    fn drop(&mut self) {
        if let Some((ticket, _)) = &self.info_job {
            ticket.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster_state(page_count: usize, dimension: u32) -> PdfEditorState {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            dimension,
            dimension,
            image::Rgba([23, 45, 67, 255]),
        ));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        PdfEditorState {
            path: PathBuf::from("fixture.pdf"),
            title: "fixture.pdf".to_owned(),
            page_count,
            pages: (1..=page_count)
                .map(|page_num| PdfPageItem {
                    page_num,
                    png_bytes: png.get_ref().clone(),
                    texture: None,
                    texture_last_used: 0,
                    error: None,
                    width: dimension,
                    height: dimension,
                    rotation: 0,
                })
                .collect(),
            selected_page: (page_count > 0).then_some(1),
            status_toast: StatusToast::new(),
            dirty: false,
            info_job: None,
            preview_rx: None,
            previews_loading: false,
            edit_rx: None,
            edit_in_progress: false,
            texture_frame: 0,
        }
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

    #[test]
    fn page_textures_obey_handle_and_byte_limits_while_selected_page_stays_resident() {
        let ctx = egui::Context::default();
        for dimension in [16, PAGE_RASTER_MAX_DIM] {
            let mut state = raster_state(PAGE_TEXTURE_LIMIT * 2, dimension);
            state.texture_frame += 1;
            let mut selected_id = None;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                selected_id = state.ensure_page_texture(1, ctx);
            });
            let selected_id = selected_id.unwrap();
            for page_num in 2..=state.page_count {
                state.texture_frame += 1;
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    assert!(state.ensure_page_texture(page_num, ctx).is_some());
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
        let original_id = state.ensure_page_texture(2, &ctx).unwrap();
        let source = state.pages[1].png_bytes.clone();
        for page_num in 3..=state.page_count {
            state.texture_frame += 1;
            state.ensure_page_texture(page_num, &ctx).unwrap();
        }
        assert!(state.pages[1].texture.is_none());
        state.selected_page = Some(2);
        state.texture_frame += 1;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            let reloaded = state.ensure_page_texture(2, ctx).unwrap();
            assert_ne!(reloaded, original_id);
        });
        assert_eq!(state.pages[1].png_bytes, source);
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
            .map(|page_num| state.ensure_page_texture(page_num, &ctx).unwrap())
            .collect();
        state.selected_page = Some(PAGE_TEXTURE_LIMIT + 1);
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
        let (count, bytes) = resident_textures(&state);
        assert!(count > 1 && count < PAGE_TEXTURE_LIMIT);
        assert!(bytes <= PAGE_TEXTURE_BYTE_LIMIT);
        assert!(state.pages[99].texture.is_some());
        assert!(state.pages[50].texture.is_none());
        assert!(state.pages.iter().all(|page| !page.png_bytes.is_empty()));
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
                previews_loading: false,
                edit_rx: None,
                edit_in_progress: false,
                texture_frame: 0,
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
            png_bytes: Vec::new(),
            texture: None,
            texture_last_used: 0,
            error: None,
            width: 0,
            height: 0,
            rotation: 0,
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
            .send(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Ok(()),
            })
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
            .send(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Err("render failed".to_owned()),
            })
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
            .send(PdfEditMsg {
                operation: "rotate",
                page_num: 1,
                output: output.clone(),
                staging_dir,
                result: Ok(()),
            })
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
