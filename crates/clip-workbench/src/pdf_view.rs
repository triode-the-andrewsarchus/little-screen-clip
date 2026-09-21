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
    delete_pdf_pages, get_pdf_info, render_pdf_page_preview, rotate_pdf_page,
};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

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
    result: Result<(), String>,
}

/// Rendered raster thumbnail and metadata for a single PDF page.
pub struct PdfPageItem {
    pub page_num: usize,
    pub png_bytes: Vec<u8>,
    pub texture: Option<egui::TextureHandle>,
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
    preview_rx: Option<Receiver<PdfPreviewMsg>>,
    previews_loading: bool,
    edit_rx: Option<Receiver<PdfEditMsg>>,
    edit_in_progress: bool,
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

    /// Loads page metadata and generates thumbnail previews for the given PDF file.
    pub fn new(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        let info = get_pdf_info(&path).unwrap_or(little_clip_core::pdf::PdfInfo {
            page_count: 1,
            title: None,
            author: None,
            page_size: None,
        });

        let page_count = info.page_count;
        let selected_page = if page_count > 0 { Some(1) } else { None };
        let mut state = Self {
            path,
            title,
            page_count,
            pages: (1..=page_count)
                .map(|page_num| PdfPageItem {
                    page_num,
                    png_bytes: Vec::new(),
                    texture: None,
                    width: 0,
                    height: 0,
                    rotation: 0,
                })
                .collect(),
            selected_page,
            status_toast: StatusToast::new(),
            dirty: false,
            preview_rx: None,
            previews_loading: false,
            edit_rx: None,
            edit_in_progress: false,
        };
        state.start_preview_worker();
        state
    }

    fn start_preview_worker(&mut self) {
        let path = self.path.clone();
        let page_count = self.page_count;
        let selected_page = self.selected_page.unwrap_or(1).min(page_count.max(1));
        let (tx, rx) = channel();
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
                let max_dim = if page_num == selected_page { 1200 } else { 360 };
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
                            if let Ok((png_bytes, width, height)) = message.result {
                                page.png_bytes = png_bytes;
                                page.width = width;
                                page.height = height;
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
            ctx.request_repaint();
        }
    }

    pub fn reload_pages(&mut self, _ctx: &egui::Context) {
        let info = get_pdf_info(&self.path).unwrap_or(little_clip_core::pdf::PdfInfo {
            page_count: 1,
            title: None,
            author: None,
            page_size: None,
        });

        self.page_count = info.page_count;
        self.pages = (1..=self.page_count)
            .map(|page_num| PdfPageItem {
                page_num,
                png_bytes: Vec::new(),
                texture: None,
                width: 0,
                height: 0,
                rotation: 0,
            })
            .collect();
        self.start_preview_worker();
        if let Some(cur) = self.selected_page {
            if cur > self.page_count {
                self.selected_page = if self.page_count > 0 {
                    Some(self.page_count)
                } else {
                    None
                };
            }
        }
    }

    pub fn delete_current_page(&mut self, now: f64) {
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
            let result = match operation {
                "delete" => delete_pdf_pages(&input, &[page_num], &output),
                "rotate" => rotate_pdf_page(&input, page_num, degrees, &output),
                _ => Err("Unknown PDF edit operation".to_string()),
            }
            .and_then(|()| {
                if !output.is_file() {
                    return Err("PDF operation produced no output file".to_string());
                }
                let info = get_pdf_info(&output)?;
                if info.page_count != expected_page_count {
                    return Err(format!(
                        "Edited PDF has {} pages; expected {}",
                        info.page_count, expected_page_count
                    ));
                }
                render_pdf_page_preview(&output, 1, 64).map(|_| ())
            });
            if result.is_err() {
                let _ = std::fs::remove_file(&output);
            }
            let _ = tx.send(PdfEditMsg {
                operation,
                page_num,
                output,
                result,
            });
        });
    }

    fn poll_edit_worker(&mut self, ctx: &egui::Context, now: f64) {
        let message = self.edit_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(message) = message {
            self.edit_rx = None;
            self.edit_in_progress = false;
            match message.result {
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
            ctx.request_repaint();
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::theme::AppPalette,
        close: &mut bool,
    ) {
        self.poll_preview_worker(ctx);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
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

        // Ensure textures loaded
        for page in &mut self.pages {
            if page.texture.is_none() {
                if let Ok(img) = image::load_from_memory(&page.png_bytes) {
                    let rgba = img.to_rgba8();
                    let color_img = egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    );
                    page.texture = Some(ctx.load_texture(
                        format!("pdf_page_{}", page.page_num),
                        color_img,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }

        let bg = crate::hex_to_c32(&palette.theme.background);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let warn_accent = crate::hex_to_c32(&palette.undo_border);
        let warn_bg = crate::hex_to_c32(&palette.undo_card_bg);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let page_count = self.page_count;
                let mut rotate_clicked = false;
                let mut delete_clicked = false;

                let mut shell = StudioShell::new(&self.title, Icon::Pdf, palette, close)
                    .with_badge(format!("{} pages", page_count), None)
                    .with_escape_hatch(EscapeHatch::primary(
                        "Okular",
                        "okular",
                        "Open PDF in external viewer (Okular / Evince)",
                    ).with_icon(Icon::Pdf))
                    .with_footer_hints("Click thumbnail: Select page | [R] Rotate 90° | [Del] Delete page | Esc: Close");

                if let Some(cur) = self.selected_page {
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
                        .add(del_btn)
                        .on_hover_text("Delete selected page from PDF (Hotkey: Del)")
                        .clicked()
                    {
                        delete_clicked = true;
                    }

                    ui.add_space(6.0);

                    // Rotate 90 CW Button
                    let rot_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCw),
                        "Rotate 90° [R]",
                        b_med,
                        palette,
                    );
                    if ui
                        .add(rot_btn)
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

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Two-Pane Split Layout
                // Left pane: page thumbnail list / grid (takes left side, wrapping if width permits)
                // Right pane: large preview of the selected page
                let avail_w = ui.available_width();
                let avail_h = (ui.available_height() - 32.0).max(100.0);
                let left_pane_w = (avail_w * 0.30).clamp(240.0, 360.0);
                let right_pane_w = (avail_w - left_pane_w - 16.0).max(200.0);

                ui.horizontal(|ui| {
                    // --- Left Pane: Page Selector Grid ---
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_pane_w, avail_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left_pane_w);
                            ui.set_height(avail_h);

                            egui::Frame::none()
                                .fill(b_low)
                                .stroke(Stroke::new(1.0_f32, b_med))
                                .rounding(Rounding::same(6.0))
                                .inner_margin(egui::Margin::same(8.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Pages")
                                                .strong()
                                                .color(f_high),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("{} total", page_count))
                                                        .small()
                                                        .color(f_low),
                                                );
                                            },
                                        );
                                    });
                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.add_space(4.0);

                                    egui::ScrollArea::vertical()
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                let pages_clone: Vec<(
                                                    usize,
                                                    Option<egui::TextureId>,
                                                    u32,
                                                    u32,
                                                )> = self
                                                    .pages
                                                    .iter()
                                                    .map(|p| {
                                                        (
                                                            p.page_num,
                                                            p.texture.as_ref().map(|t| t.id()),
                                                            p.width,
                                                            p.height,
                                                        )
                                                    })
                                                    .collect();

                                                for (page_num, tex_id_opt, w, h) in pages_clone {
                                                    let is_selected =
                                                        self.selected_page == Some(page_num);

                                                    ui.vertical(|ui| {
                                                        let border_col = if is_selected {
                                                            accent
                                                        } else {
                                                            b_med
                                                        };
                                                        let stroke_w = if is_selected { 2.0_f32 } else { 1.0_f32 };

                                                        egui::Frame::none()
                                                            .fill(if is_selected { b_med } else { bg })
                                                            .stroke(Stroke::new(stroke_w, border_col))
                                                            .rounding(Rounding::same(4.0))
                                                            .inner_margin(egui::Margin::same(6.0))
                                                            .show(ui, |ui| {
                                                                let aspect = if h > 0 {
                                                                    w as f32 / h as f32
                                                                } else {
                                                                    0.707
                                                                };
                                                                let thumb_h = 130.0;
                                                                let thumb_w = thumb_h * aspect;

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
                                                                    let placeholder = egui::Button::new("...")
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
                                                                        .small()
                                                                        .strong()
                                                                        .color(if is_selected {
                                                                            accent
                                                                        } else {
                                                                            f_low
                                                                        }),
                                                                    );
                                                                });
                                                            });
                                                    });

                                                    ui.add_space(6.0);
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
                                .fill(b_low)
                                .stroke(Stroke::new(1.0_f32, b_med))
                                .rounding(Rounding::same(6.0))
                                .inner_margin(egui::Margin::same(12.0))
                                .show(ui, |ui| {
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
                                                .color(f_high),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "• {} × {} px",
                                                    page.width, page.height
                                                ))
                                                .small()
                                                .color(f_low),
                                            );

                                        });

                                        ui.add_space(6.0);
                                        ui.separator();
                                        ui.add_space(8.0);

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
                                            } else {
                                                ui.spinner();
                                            }
                                        });
                                    } else {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                egui::RichText::new("No page selected.")
                                                    .color(f_low),
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
