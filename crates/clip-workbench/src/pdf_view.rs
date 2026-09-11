use eframe::egui::{self, Color32, Rounding, Stroke, Vec2};
use little_clip_core::pdf::{delete_pdf_pages, get_pdf_info, render_pdf_page_preview, rotate_pdf_page};
use std::path::PathBuf;

pub struct PdfPageItem {
    pub page_num: usize,
    pub png_bytes: Vec<u8>,
    pub texture: Option<egui::TextureHandle>,
    pub width: u32,
    pub height: u32,
    #[allow(dead_code)]
    pub rotation: u32,
}

pub struct PdfEditorState {
    pub path: PathBuf,
    pub title: String,
    pub page_count: usize,
    pub pages: Vec<PdfPageItem>,
    pub selected_page: Option<usize>,
    pub status_toast: Option<(String, f64)>,
    pub dirty: bool,
}

impl PdfEditorState {
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

        let mut pages = Vec::new();
        for p in 1..=info.page_count {
            if let Ok((bytes, w, h)) = render_pdf_page_preview(&path, p, 360) {
                pages.push(PdfPageItem {
                    page_num: p,
                    png_bytes: bytes,
                    texture: None,
                    width: w,
                    height: h,
                    rotation: 0,
                });
            }
        }

        let page_count = pages.len();
        let selected_page = if page_count > 0 { Some(1) } else { None };

        Self {
            path,
            title,
            page_count,
            pages,
            selected_page,
            status_toast: None,
            dirty: false,
        }
    }

    pub fn reload_pages(&mut self, ctx: &egui::Context) {
        let info = get_pdf_info(&self.path).unwrap_or(little_clip_core::pdf::PdfInfo {
            page_count: 1,
            title: None,
            author: None,
            page_size: None,
        });

        let mut pages = Vec::new();
        for p in 1..=info.page_count {
            if let Ok((bytes, w, h)) = render_pdf_page_preview(&self.path, p, 360) {
                let img = image::load_from_memory(&bytes).ok();
                let tex = img.map(|i| {
                    let rgba = i.to_rgba8();
                    let color_img = egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    );
                    ctx.load_texture(
                        format!("pdf_page_{}_{}", p, uuid::Uuid::new_v4()),
                        color_img,
                        egui::TextureOptions::LINEAR,
                    )
                });

                pages.push(PdfPageItem {
                    page_num: p,
                    png_bytes: bytes,
                    texture: tex,
                    width: w,
                    height: h,
                    rotation: 0,
                });
            }
        }

        self.page_count = pages.len();
        self.pages = pages;
        if let Some(cur) = self.selected_page {
            if cur > self.page_count {
                self.selected_page = if self.page_count > 0 { Some(self.page_count) } else { None };
            }
        }
    }

    pub fn delete_current_page(&mut self, ctx: &egui::Context, now: f64) {
        let Some(cur) = self.selected_page else { return; };
        if self.page_count <= 1 {
            self.status_toast = Some(("Cannot delete only page from PDF".to_string(), now));
            return;
        }

        let temp_out = std::env::temp_dir().join(format!("pdf_del_{}.pdf", uuid::Uuid::new_v4()));
        if let Ok(()) = delete_pdf_pages(&self.path, &[cur], &temp_out) {
            let _ = std::fs::copy(&temp_out, &self.path);
            let _ = std::fs::remove_file(&temp_out);
            self.reload_pages(ctx);
            self.dirty = true;
            self.status_toast = Some((format!("Deleted page {}", cur), now));
        } else {
            self.status_toast = Some(("Failed to delete page".to_string(), now));
        }
    }

    pub fn rotate_current_page(&mut self, ctx: &egui::Context, degrees: u32, now: f64) {
        let Some(cur) = self.selected_page else { return; };
        let temp_out = std::env::temp_dir().join(format!("pdf_rot_{}.pdf", uuid::Uuid::new_v4()));
        if let Ok(()) = rotate_pdf_page(&self.path, cur, degrees, &temp_out) {
            let _ = std::fs::copy(&temp_out, &self.path);
            let _ = std::fs::remove_file(&temp_out);
            self.reload_pages(ctx);
            self.dirty = true;
            self.status_toast = Some((format!("Rotated page {} by {}°", cur, degrees), now));
        } else {
            self.status_toast = Some(("Failed to rotate page".to_string(), now));
        }
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Hotkeys
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            self.delete_current_page(ctx, now);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.rotate_current_page(ctx, 90, now);
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

        egui::Frame::none()
            .fill(Color32::from_rgb(15, 23, 42)) // Slate 900
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                // Header Toolbar
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("📑").size(24.0));
                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(18.0)
                            .color(Color32::WHITE),
                    );

                    ui.add_space(8.0);
                    let info_badge = format!("{} pages", self.page_count);
                    ui.label(
                        egui::RichText::new(info_badge)
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    ui.add_space(16.0);

                    // Rotate 90 CW Button
                    let rot_btn = egui::Button::new(
                        egui::RichText::new("↻ Rotate 90° [R]")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(226, 232, 240)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(71, 85, 105)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(rot_btn).on_hover_text("Rotate selected page 90° clockwise (Hotkey: R)").clicked() {
                        self.rotate_current_page(ctx, 90, now);
                    }

                    // Delete Page Button
                    let del_btn = egui::Button::new(
                        egui::RichText::new("🗑 Delete Page [Del]")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(248, 113, 113)),
                    )
                    .fill(Color32::from_rgb(69, 10, 10))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(220, 38, 38)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(del_btn).on_hover_text("Delete selected page from PDF (Hotkey: Del)").clicked() {
                        self.delete_current_page(ctx, now);
                    }

                    // Right-aligned close button & toast
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui::RichText::new("✕ Close (Esc)")
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(220, 38, 38))
                        .rounding(Rounding::same(6.0));
                        if ui.add(close_btn).clicked() {
                            *close = true;
                        }

                        if let Some((msg, ts)) = &self.status_toast {
                            if now - ts < 4.0 {
                                ui.label(
                                    egui::RichText::new(msg)
                                        .small()
                                        .color(Color32::from_rgb(52, 211, 153)),
                                );
                            }
                        }
                    });
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Grid of PDF Pages
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let pages_clone: Vec<(usize, Option<egui::TextureId>, u32, u32)> = self
                            .pages
                            .iter()
                            .map(|p| (p.page_num, p.texture.as_ref().map(|t| t.id()), p.width, p.height))
                            .collect();

                        for (page_num, tex_id_opt, w, h) in pages_clone {
                            let is_selected = self.selected_page == Some(page_num);

                            ui.vertical(|ui| {
                                let border_col = if is_selected {
                                    Color32::from_rgb(59, 130, 246) // Blue 500
                                } else {
                                    Color32::from_rgb(51, 65, 85) // Slate 700
                                };

                                let stroke_w = if is_selected { 2.5 } else { 1.0 };

                                egui::Frame::none()
                                    .fill(Color32::from_rgb(30, 41, 59))
                                    .stroke(Stroke::new(stroke_w, border_col))
                                    .rounding(Rounding::same(6.0))
                                    .inner_margin(egui::Margin::same(8.0))
                                    .show(ui, |ui| {
                                        let aspect = if h > 0 { w as f32 / h as f32 } else { 0.707 };
                                        let thumb_h = 240.0;
                                        let thumb_w = thumb_h * aspect;

                                        if let Some(tex_id) = tex_id_opt {
                                            let img_btn = egui::ImageButton::new(
                                                egui::load::SizedTexture::new(tex_id, Vec2::new(thumb_w, thumb_h)),
                                            );
                                            if ui.add(img_btn).clicked() {
                                                self.selected_page = Some(page_num);
                                            }
                                        } else {
                                            let placeholder = egui::Button::new("Loading...")
                                                .min_size(Vec2::new(thumb_w, thumb_h));
                                            if ui.add(placeholder).clicked() {
                                                self.selected_page = Some(page_num);
                                            }
                                        }

                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("Page {}", page_num))
                                                    .small()
                                                    .strong()
                                                    .color(if is_selected { Color32::from_rgb(96, 165, 250) } else { Color32::from_rgb(148, 163, 184) }),
                                            );
                                        });
                                    });
                            });

                            ui.add_space(12.0);
                        }
                    });
                });
            });
    }
}
