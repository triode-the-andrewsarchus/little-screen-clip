use super::super::*;

fn run_optipng_optimization(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut temp_file = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("Tempfile creation failed: {}", e))?;
    temp_file
        .write_all(png_bytes)
        .map_err(|e| format!("Tempfile write failed: {}", e))?;
    let temp_path = temp_file.path().to_path_buf();

    let status = Command::new("optipng")
        .arg("-o2")
        .arg("-quiet")
        .arg(&temp_path)
        .status()
        .map_err(|e| format!("Failed to run optipng: {}. Ensure optipng is installed.", e))?;

    if !status.success() {
        return Err(format!(
            "optipng failed with exit status {:?}",
            status.code()
        ));
    }

    std::fs::read(&temp_path).map_err(|e| format!("Failed to read optipng output: {}", e))
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConvertTargetFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ConvertTargetFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ConvertTargetFormat::Png => "png",
            ConvertTargetFormat::Jpeg => "jpg",
            ConvertTargetFormat::Webp => "webp",
        }
    }
}

pub struct ConvertState {
    pub target_format: ConvertTargetFormat,
    pub lossless: bool,
    pub quality: u8,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub encoded_bytes: Vec<u8>,
    pub converted_rgba: Option<RgbaImage>,
    pub converted_texture: Option<egui::TextureHandle>,
    pub dirty_texture: bool,
    pub is_running_optipng: bool,
    pub optipng_savings: Option<(usize, usize)>,
    pub is_encoding: bool,
    pub params_dirty_at: Option<std::time::Instant>,
    pub request_generation: u64,
    pub completed_generation: u64,
    pub render_error: Option<String>,
    render_rx: Option<std::sync::mpsc::Receiver<ConvertWorkerMsg>>,
}

struct ConvertWorkerMsg {
    generation: u64,
    result: Result<(Vec<u8>, RgbaImage), String>,
}

impl Default for ConvertState {
    fn default() -> Self {
        Self {
            target_format: ConvertTargetFormat::Webp,
            lossless: false,
            quality: 80,
            comparison_split: 0.50,
            is_dragging_split: false,
            encoded_bytes: Vec::new(),
            converted_rgba: None,
            converted_texture: None,
            dirty_texture: false,
            is_running_optipng: false,
            optipng_savings: None,
            is_encoding: false,
            params_dirty_at: None,
            request_generation: 0,
            completed_generation: 0,
            render_error: None,
            render_rx: None,
        }
    }
}

impl ConvertState {
    pub fn is_pending(&self) -> bool {
        self.params_dirty_at.is_some() || self.is_encoding
    }

    pub fn output_is_current(&self) -> bool {
        self.completed_generation == self.request_generation && self.render_error.is_none()
    }
}

fn encode_conversion(
    base_img: RgbaImage,
    format: ConvertTargetFormat,
    quality: u8,
    lossless: bool,
) -> Result<(Vec<u8>, RgbaImage), String> {
    let bytes = match format {
        ConvertTargetFormat::Png => encode_png(&base_img).map_err(|e| e.to_string()),
        ConvertTargetFormat::Jpeg => {
            encode_jpeg_with_quality(&base_img, quality).map_err(|e| e.to_string())
        }
        ConvertTargetFormat::Webp => encode_webp(&base_img, lossless, quality),
    }?;
    let decoded = image::load_from_memory(&bytes)
        .map(|image| image.to_rgba8())
        .unwrap_or(base_img);
    Ok((bytes, decoded))
}

impl ImagePreviewState {
    #[cfg(test)]
    pub fn update_conversion(&mut self) {
        let base_img = self.current_display_image();
        let fmt = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = None;
        self.convert_state.render_rx = None;
        self.convert_state.is_encoding = false;

        match encode_conversion(base_img, fmt, quality, lossless) {
            Ok((bytes, decoded_rgba)) => {
                self.convert_state.encoded_bytes = bytes;
                self.convert_state.converted_rgba = Some(decoded_rgba);
                self.convert_state.dirty_texture = true;
                self.convert_state.optipng_savings = None;
                self.convert_state.render_error = None;
                self.convert_state.completed_generation = self.convert_state.request_generation;
            }
            Err(e) => {
                self.convert_state.render_error = Some(e.clone());
                self.show_toast(format!("Conversion failed: {}", e));
            }
        }
    }

    pub fn schedule_conversion(&mut self) {
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = Some(std::time::Instant::now());
        self.convert_state.optipng_savings = None;
        self.convert_state.render_error = None;
    }

    pub fn run_conversion_worker(&mut self) {
        if self.convert_state.is_encoding || self.current_rgba.is_empty() {
            return;
        }

        let image = self.current_display_image();
        let format = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        let generation = self.convert_state.request_generation;
        let (tx, rx) = std::sync::mpsc::channel();

        self.convert_state.render_rx = Some(rx);
        self.convert_state.is_encoding = true;
        self.convert_state.params_dirty_at = None;

        let _ = std::thread::Builder::new()
            .name("image_conversion".to_string())
            .spawn(move || {
                let result = encode_conversion(image, format, quality, lossless);
                let _ = tx.send(ConvertWorkerMsg { generation, result });
            });
    }

    pub fn poll_conversion(&mut self) {
        let Some(rx) = self.convert_state.render_rx.take() else {
            return;
        };

        match rx.try_recv() {
            Ok(message) => {
                self.convert_state.is_encoding = false;
                if message.generation != self.convert_state.request_generation {
                    return;
                }
                match message.result {
                    Ok((bytes, rgba)) => {
                        self.convert_state.encoded_bytes = bytes;
                        self.convert_state.converted_rgba = Some(rgba);
                        self.convert_state.dirty_texture = true;
                        self.convert_state.completed_generation = message.generation;
                        self.convert_state.render_error = None;
                    }
                    Err(error) => {
                        self.convert_state.render_error = Some(error.clone());
                        self.show_toast(format!("Conversion failed: {}", error));
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.convert_state.render_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_error = Some("Conversion worker stopped".to_string());
            }
        }
    }

    pub fn apply_converted_to_canvas(&mut self) {
        if let Some(converted) = self.convert_state.converted_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = converted;
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.schedule_conversion();
            self.show_toast("✔ Applied converted image to canvas");
        }
    }

    pub fn run_optipng_conversion(&mut self) {
        if self.convert_state.target_format != ConvertTargetFormat::Png {
            self.show_toast("⚠ OptiPNG is only applicable for PNG format");
            return;
        }

        self.convert_state.is_running_optipng = true;
        let original_bytes = self.convert_state.encoded_bytes.clone();
        if original_bytes.is_empty() {
            self.convert_state.is_running_optipng = false;
            self.show_toast("⚠ No PNG bytes available to optimize");
            return;
        }

        let orig_len = original_bytes.len();
        match run_optipng_optimization(&original_bytes) {
            Ok(opt_bytes) => {
                let opt_len = opt_bytes.len();
                self.convert_state.encoded_bytes = opt_bytes;
                self.convert_state.optipng_savings = Some((orig_len, opt_len));
                self.convert_state.is_running_optipng = false;
                let pct = if orig_len > 0 && opt_len < orig_len {
                    (orig_len - opt_len) as f32 / orig_len as f32 * 100.0
                } else {
                    0.0
                };
                self.show_toast(format!("✔ OptiPNG complete! Saved {:.1}%", pct));
            }
            Err(e) => {
                self.convert_state.is_running_optipng = false;
                self.show_toast(format!("❌ OptiPNG error: {}", e));
            }
        }
    }

    pub(crate) fn render_convert_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.theme.b_med);
        let border = hex_to_c32(&palette.theme.b_low);
        let text = hex_to_c32(&palette.theme.f_med);
        let muted = hex_to_c32(&palette.theme.f_low);
        let success = hex_to_c32(&palette.diff_add);
        let warning = hex_to_c32(&palette.diff_mod);
        let mut changed = false;

        ui.label(
            egui::RichText::new("Convert")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Choose an output format and compare it on the canvas.")
                .size(11.5)
                .color(muted),
        );
        ui.add_space(6.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if let Some(error) = &self.convert_state.render_error {
                    ui.add(
                        egui::Label::new(egui::RichText::new(error).size(11.5).color(warning))
                            .truncate(true),
                    );
                } else if self.convert_state.is_pending() {
                    ui.add(egui::Spinner::new().size(13.0).color(accent));
                    ui.label(
                        egui::RichText::new(if self.convert_state.is_encoding {
                            "Encoding preview…"
                        } else {
                            "Waiting for changes…"
                        })
                        .size(11.5)
                        .color(accent),
                    );
                }
            },
        );

        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("OUTPUT FORMAT")
                .size(12.0)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        let formats = [
            (ConvertTargetFormat::Png, "PNG"),
            (ConvertTargetFormat::Jpeg, "JPEG"),
            (ConvertTargetFormat::Webp, "WebP"),
        ];
        let format_width = ((ui.available_width() - 8.0) / 3.0).max(54.0);
        ui.horizontal(|ui| {
            for (format, label) in formats {
                let active = self.convert_state.target_format == format;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(12.0)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(1.0, if active { foreground } else { border }))
                .rounding(Rounding::same(4.0));
                if ui.add_sized([format_width, 30.0], button).clicked() && !active {
                    self.convert_state.target_format = format;
                    match format {
                        ConvertTargetFormat::Png => self.convert_state.lossless = true,
                        ConvertTargetFormat::Jpeg => self.convert_state.lossless = false,
                        ConvertTargetFormat::Webp => {}
                    }
                    changed = true;
                }
            }
        });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("ENCODING")
                .size(12.0)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        match self.convert_state.target_format {
            ConvertTargetFormat::Png => {
                ui.label(
                    egui::RichText::new("Lossless PNG")
                        .size(13.0)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Preserves every pixel and transparency.")
                        .size(11.5)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Jpeg => {
                ui.label(
                    egui::RichText::new("Lossy JPEG")
                        .size(13.0)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Small files; transparency is flattened.")
                        .size(11.5)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Webp => {
                let mut lossless = self.convert_state.lossless;
                if ui
                    .checkbox(
                        &mut lossless,
                        egui::RichText::new("Lossless WebP")
                            .size(13.0)
                            .strong()
                            .color(text),
                    )
                    .on_hover_text("Preserve every pixel using WebP lossless encoding")
                    .changed()
                {
                    self.convert_state.lossless = lossless;
                    changed = true;
                }
                ui.label(
                    egui::RichText::new(if lossless {
                        "Preserves every pixel and transparency."
                    } else {
                        "Compresses for a smaller output file."
                    })
                    .size(11.5)
                    .color(muted),
                );
            }
        }

        if !self.convert_state.lossless {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Quality")
                        .size(12.0)
                        .strong()
                        .color(text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}%", self.convert_state.quality))
                            .size(12.0)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let mut quality = self.convert_state.quality;
            ui.spacing_mut().slider_width = ui.available_width();
            if ui
                .add(egui::Slider::new(&mut quality, 1..=100).show_value(false))
                .on_hover_text("Higher quality produces a larger file")
                .changed()
            {
                self.convert_state.quality = quality;
                changed = true;
            }
        }

        if changed {
            self.schedule_conversion();
        }

        if self.convert_state.target_format == ConvertTargetFormat::Png {
            ui.add_space(10.0);
            let optimize_label = if self.convert_state.is_running_optipng {
                "Optimizing…"
            } else {
                "Run OptiPNG"
            };
            let optimize = crate::secondary_button(optimize_label, warning, palette);
            if ui
                .add_enabled_ui(
                    !self.convert_state.is_running_optipng
                        && self.convert_state.output_is_current(),
                    |ui| ui.add_sized([ui.available_width(), 30.0], optimize),
                )
                .inner
                .on_hover_text("Losslessly reduce the encoded PNG file size")
                .clicked()
            {
                self.run_optipng_conversion();
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("FILE SIZE")
                .size(12.0)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        let original_size = self.file_size_bytes;
        let converted_size = self.convert_state.encoded_bytes.len();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Original").size(12.0).color(muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_bytes_size(original_size))
                        .size(12.0)
                        .monospace()
                        .color(text),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Converted").size(12.0).color(muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(if converted_size == 0 {
                        "—".to_string()
                    } else {
                        format_bytes_size(converted_size)
                    })
                    .size(12.0)
                    .monospace()
                    .color(text),
                );
            });
        });

        if converted_size > 0 && original_size > 0 {
            let delta =
                (converted_size as f64 - original_size as f64) / original_size as f64 * 100.0;
            let delta_color = if delta <= 0.0 { success } else { warning };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Difference").size(12.0).color(muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:+.1}%", delta))
                            .size(12.0)
                            .strong()
                            .monospace()
                            .color(delta_color),
                    );
                });
            });
        }

        if let Some((before, after)) = self.convert_state.optipng_savings {
            let saved = before.saturating_sub(after);
            ui.label(
                egui::RichText::new(format!("OptiPNG saved {}", format_bytes_size(saved)))
                    .size(11.5)
                    .strong()
                    .color(success),
            );
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Drag the canvas divider to compare before and after.")
                .size(11.5)
                .italics()
                .color(muted),
        );

        ui.add_space(14.0);
        let output_ready = self.convert_state.output_is_current();
        let extension = self.convert_state.target_format.extension();
        let save = crate::action_button(format!("Save as .{}", extension), accent, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), 32.0], save)
            })
            .inner
            .on_hover_text(format!("Export the converted image as .{}", extension))
            .clicked()
        {
            self.open_save_dialog_with_ext(extension);
        }

        ui.add_space(4.0);
        let apply = crate::secondary_button("Apply to canvas (Enter)", success, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), 30.0], apply)
            })
            .inner
            .on_hover_text("Use the converted image as the active editor canvas")
            .clicked()
        {
            self.apply_converted_to_canvas();
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("COPY TO SLOT")
                .size(12.0)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);
        let slot_width = ((ui.available_width() - 6.0) * 0.5).max(70.0);
        ui.horizontal(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_sized([slot_width, 28.0], left)
                .on_hover_text("Copy the converted image to the Left Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Left);
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_sized([slot_width, 28.0], right)
                .on_hover_text("Copy the converted image to the Right Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Right);
            }
        });

        // A control can become dirty after the top-level polling pass for this
        // frame. Ensure the debounce timer and worker continue without needing
        // another pointer event to wake the UI.
        if self.convert_state.is_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub(crate) fn render_convert_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        // Ensure converted texture is loaded
        if self.convert_state.dirty_texture || self.convert_state.converted_texture.is_none() {
            if let Some(ref conv_rgba) = self.convert_state.converted_rgba {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [conv_rgba.width() as usize, conv_rgba.height() as usize],
                    conv_rgba.as_raw(),
                );
                self.convert_state.converted_texture = Some(ctx.load_texture(
                    "converted_preview_tex",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                self.convert_state.dirty_texture = false;
            }
        }

        let split = self.convert_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // 1. Draw Left Side (Original / Before)
        if let Some(ref orig_tex) = self.texture {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut left_mesh = egui::Mesh::with_texture(orig_tex.id());
            left_mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(left_mesh));
        }

        // 2. Draw Right Side (Converted / After)
        let conv_tex_id = self
            .convert_state
            .converted_texture
            .as_ref()
            .map(|t| t.id())
            .or_else(|| self.texture.as_ref().map(|t| t.id()));
        if let Some(tex_id) = conv_tex_id {
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut right_mesh = egui::Mesh::with_texture(tex_id);
            right_mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(right_mesh));
        }

        // 3. Draw Crisp Divider Line
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(3.0, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(1.5, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_radius = 16.0;
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let knob_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
        painter.circle(knob_center, knob_radius, b_low, Stroke::new(2.0, base_c));
        painter.text(
            knob_center,
            egui::Align2::CENTER_CENTER,
            "◀ ▶",
            egui::FontId::monospace(10.0),
            knob_fg,
        );

        // 5. Draggable Handle Interaction
        let ptr_pos = response.hover_pos();
        let near_divider = if let Some(pos) = ptr_pos {
            (pos.x - split_x).abs() <= 16.0 && pos.y >= img_rect.min.y && pos.y <= img_rect.max.y
        } else {
            false
        };

        if near_divider || self.convert_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            self.convert_state.is_dragging_split = true;
        }

        if self.convert_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let new_split = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    self.convert_state.comparison_split = new_split;
                }
            }
            if response.drag_stopped() {
                self.convert_state.is_dragging_split = false;
            }
        }

        // 6. Squoosh-Style Floating HUD Badges
        let orig_size_str = format_bytes_size(self.file_size_bytes);
        let conv_size_str = format_bytes_size(self.convert_state.encoded_bytes.len());
        let orig_fmt = self.mime.replace("image/", "").to_uppercase();
        let conv_fmt = self.convert_state.target_format.extension().to_uppercase();

        let before_badge_pos = Pos2::new(img_rect.min.x + 12.0, img_rect.min.y + 12.0);
        let before_rect = Rect::from_min_size(before_badge_pos, Vec2::new(140.0, 24.0));
        let badge_bg = hex_to_c32_alpha(&palette.theme.b_low, 210);
        let before_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_med)), palette);
        painter.rect_filled(before_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(before_rect, Rounding::same(4.0), Stroke::new(1.0, b_med));
        painter.text(
            before_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("BEFORE: {} ({})", orig_fmt, orig_size_str),
            egui::FontId::proportional(11.0),
            before_fg,
        );

        let after_badge_pos = Pos2::new(img_rect.max.x - 152.0, img_rect.min.y + 12.0);
        let after_rect = Rect::from_min_size(after_badge_pos, Vec2::new(140.0, 24.0));
        let after_fg = contrast_text_c32(b_low, Some(base_c), palette);
        painter.rect_filled(after_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(after_rect, Rounding::same(4.0), Stroke::new(1.0, base_c));
        painter.text(
            after_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("AFTER: {} ({})", conv_fmt, conv_size_str),
            egui::FontId::proportional(11.0),
            after_fg,
        );
    }
}
