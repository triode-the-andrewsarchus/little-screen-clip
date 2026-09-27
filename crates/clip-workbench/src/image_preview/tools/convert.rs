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
    pub(crate) completed_input_stamp: Option<ImageInputStamp>,
    render_input_stamp: Option<ImageInputStamp>,
    pub(crate) render_error_input_stamp: Option<ImageInputStamp>,
    pub render_error: Option<String>,
    render_rx: Option<std::sync::mpsc::Receiver<ConvertWorkerMsg>>,
}

struct ConvertWorkerMsg {
    generation: u64,
    input_stamp: ImageInputStamp,
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
            completed_input_stamp: None,
            render_input_stamp: None,
            render_error_input_stamp: None,
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

    pub fn output_matches(&self, input_stamp: &ImageInputStamp) -> bool {
        self.output_is_current() && self.completed_input_stamp.as_ref() == Some(input_stamp)
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
        let input_stamp = self.input_stamp();
        let fmt = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = None;
        self.convert_state.render_rx = None;
        self.convert_state.is_encoding = false;
        self.convert_state.render_input_stamp = None;

        match encode_conversion(base_img, fmt, quality, lossless) {
            Ok((bytes, decoded_rgba)) => {
                self.convert_state.encoded_bytes = bytes;
                self.convert_state.converted_rgba = Some(decoded_rgba);
                self.convert_state.dirty_texture = true;
                self.convert_state.optipng_savings = None;
                self.convert_state.render_error = None;
                self.convert_state.render_error_input_stamp = None;
                self.convert_state.completed_generation = self.convert_state.request_generation;
                self.convert_state.completed_input_stamp = Some(input_stamp);
            }
            Err(e) => {
                self.convert_state.render_error = Some(e.clone());
                self.convert_state.render_error_input_stamp = Some(input_stamp);
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
        self.convert_state.render_error_input_stamp = None;
    }

    pub fn run_conversion_worker(&mut self) {
        if self.convert_state.is_encoding || self.current_rgba.is_empty() {
            return;
        }

        let image = self.current_display_image();
        let input_stamp = self.input_stamp();
        let format = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        let generation = self.convert_state.request_generation;
        let (tx, rx) = std::sync::mpsc::channel();

        self.convert_state.render_rx = Some(rx);
        self.convert_state.render_input_stamp = Some(input_stamp.clone());
        self.convert_state.is_encoding = true;
        self.convert_state.params_dirty_at = None;

        let _ = std::thread::Builder::new()
            .name("image_conversion".to_string())
            .spawn(move || {
                let result = encode_conversion(image, format, quality, lossless);
                let _ = tx.send(ConvertWorkerMsg {
                    generation,
                    input_stamp,
                    result,
                });
            });
    }

    pub fn poll_conversion(&mut self) {
        if self.convert_state.is_encoding
            && self.convert_state.render_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.convert_state.render_rx = None;
            self.convert_state.render_input_stamp = None;
            self.convert_state.is_encoding = false;
            if self.active_mode == ImageToolMode::Convert {
                self.convert_state.params_dirty_at = Some(std::time::Instant::now());
            }
            return;
        }
        let Some(rx) = self.convert_state.render_rx.take() else {
            return;
        };

        match rx.try_recv() {
            Ok(message) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_input_stamp = None;
                if message.generation != self.convert_state.request_generation
                    || message.input_stamp != self.input_stamp()
                {
                    if self.active_mode == ImageToolMode::Convert {
                        self.convert_state.params_dirty_at = Some(std::time::Instant::now());
                    }
                    return;
                }
                match message.result {
                    Ok((bytes, rgba)) => {
                        self.convert_state.encoded_bytes = bytes;
                        self.convert_state.converted_rgba = Some(rgba);
                        self.convert_state.dirty_texture = true;
                        self.convert_state.completed_generation = message.generation;
                        self.convert_state.completed_input_stamp = Some(message.input_stamp);
                        self.convert_state.render_error = None;
                        self.convert_state.render_error_input_stamp = None;
                    }
                    Err(error) => {
                        self.convert_state.render_error = Some(error.clone());
                        self.convert_state.render_error_input_stamp =
                            Some(message.input_stamp.clone());
                        self.show_toast(format!("Conversion failed: {}", error));
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.convert_state.render_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_error_input_stamp =
                    self.convert_state.render_input_stamp.take();
                self.convert_state.render_error = Some("Conversion worker stopped".to_string());
            }
        }
    }

    pub fn apply_converted_to_canvas(&mut self) {
        if !self.convert_state.output_matches(&self.input_stamp()) {
            return;
        }
        if let Some(converted) = self.convert_state.converted_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = converted;
            self.mark_pixels_changed();
            self.redaction_state.boxes.clear();
            self.markup_state.items.clear();
            self.convert_state.completed_input_stamp = None;
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.schedule_conversion();
            self.show_toast("✔ Applied converted image to canvas");
        }
    }

    pub fn run_optipng_conversion(&mut self) {
        if !self.convert_state.output_matches(&self.input_stamp()) {
            return;
        }
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
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let border = hex_to_c32(&palette.colors.surface_recessed);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
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
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), crate::ui_ext::CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if self.convert_state.render_error_input_stamp.as_ref() == Some(&self.input_stamp())
                    && let Some(error) = &self.convert_state.render_error
                {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
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
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(accent),
                    );
                }
            },
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OUTPUT FORMAT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

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
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui
                    .add_sized([format_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .clicked()
                    && !active
                {
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

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("ENCODING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        match self.convert_state.target_format {
            ConvertTargetFormat::Png => {
                ui.label(
                    egui::RichText::new("Lossless PNG")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Preserves every pixel and transparency.")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Jpeg => {
                ui.label(
                    egui::RichText::new("Lossy JPEG")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Small files; transparency is flattened.")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Webp => {
                let mut lossless = self.convert_state.lossless;
                if ui
                    .checkbox(
                        &mut lossless,
                        egui::RichText::new("Lossless WebP")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
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
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
                );
            }
        }

        if !self.convert_state.lossless {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Quality")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}%", self.convert_state.quality))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
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
                        && self.convert_state.output_matches(&self.input_stamp()),
                    |ui| {
                        ui.add_sized(
                            [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                            optimize,
                        )
                    },
                )
                .inner
                .on_hover_text("Losslessly reduce the encoded PNG file size")
                .clicked()
            {
                self.run_optipng_conversion();
            }
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("FILE SIZE")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        let original_size = self.file_size_bytes;
        let converted_size = if self.convert_state.output_matches(&self.input_stamp()) {
            self.convert_state.encoded_bytes.len()
        } else {
            0
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Original")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_bytes_size(original_size))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(text),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Converted")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(if converted_size == 0 {
                        "—".to_string()
                    } else {
                        format_bytes_size(converted_size)
                    })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
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
                ui.label(
                    egui::RichText::new("Difference")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:+.1}%", delta))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .monospace()
                            .color(delta_color),
                    );
                });
            });
        }

        if self.convert_state.output_matches(&self.input_stamp())
            && let Some((before, after)) = self.convert_state.optipng_savings
        {
            let saved = before.saturating_sub(after);
            ui.label(
                egui::RichText::new(format!("OptiPNG saved {}", format_bytes_size(saved)))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(success),
            );
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("Drag the canvas divider to compare before and after.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .italics()
                .color(muted),
        );

        ui.add_space(14.0);
        let output_ready = self.convert_state.output_matches(&self.input_stamp());
        let extension = self.convert_state.target_format.extension();
        let save = crate::action_button(format!("Save as .{}", extension), accent, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], save)
            })
            .inner
            .on_hover_text(format!("Export the converted image as .{}", extension))
            .clicked()
        {
            self.open_save_dialog_with_ext(extension);
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let apply = crate::secondary_button("Apply to canvas (Enter)", success, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
            })
            .inner
            .on_hover_text("Use the converted image as the active editor canvas")
            .clicked()
        {
            self.apply_converted_to_canvas();
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("COPY TO SLOT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let slot_width = ((ui.available_width() - 6.0) * 0.5).max(70.0);
        ui.horizontal(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], left)
                .on_hover_text("Copy the converted image to the Left Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Left);
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], right)
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
        if self.convert_state.output_matches(&self.input_stamp())
            && (self.convert_state.dirty_texture || self.convert_state.converted_texture.is_none())
        {
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
        let conv_tex_id = if self.convert_state.output_matches(&self.input_stamp()) {
            self.convert_state
                .converted_texture
                .as_ref()
                .map(|t| t.id())
                .or_else(|| self.texture.as_ref().map(|t| t.id()))
        } else {
            self.texture.as_ref().map(|t| t.id())
        };
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
            Stroke::new(3.0_f32, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(1.5_f32, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let knob_radius = crate::ui_ext::paint_split_handle(painter, knob_center, palette);

        // 5. Draggable Handle Interaction
        let ptr_pos = response.hover_pos();
        let near_divider =
            crate::ui_ext::split_handle_hit_test(ptr_pos, img_rect, knob_center, knob_radius, 16.0);

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
        let conv_size_str = if self.convert_state.output_matches(&self.input_stamp()) {
            format_bytes_size(self.convert_state.encoded_bytes.len())
        } else {
            "—".to_string()
        };
        let orig_fmt = self.mime.replace("image/", "").to_uppercase();
        let conv_fmt = self.convert_state.target_format.extension().to_uppercase();

        let bounds = img_rect.intersect(painter.clip_rect());
        let gap = 8.0;
        let max_width = ((bounds.width() - gap * 3.0) * 0.5).max(1.0);
        let before_value = format!("{} · {}", orig_fmt, orig_size_str);
        let after_value = format!("{} · {}", conv_fmt, conv_size_str);
        crate::ui_ext::CanvasBadge {
            heading: "BEFORE",
            value: Some(&before_value),
            accent: None,
        }
        .paint(
            painter,
            bounds.min + Vec2::splat(gap),
            egui::Align2::LEFT_TOP,
            bounds,
            max_width,
            palette,
        );
        crate::ui_ext::CanvasBadge {
            heading: "AFTER",
            value: Some(&after_value),
            accent: Some(base_c),
        }
        .paint(
            painter,
            Pos2::new(bounds.right() - gap, bounds.top() + gap),
            egui::Align2::RIGHT_TOP,
            bounds,
            max_width,
            palette,
        );
    }
}

#[cfg(test)]
mod input_stamp_tests {
    use super::*;

    fn state() -> ImagePreviewState {
        let mut bytes = Vec::new();
        RgbaImage::new(4, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".into(),
            None,
            "conversion stamp test".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn conversion_discards_stale_success_and_stale_failure() {
        for result in [
            Ok((
                vec![1],
                RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
            )),
            Err("stale encode error".into()),
        ] {
            let mut state = state();
            state.active_mode = ImageToolMode::Convert;
            let old_stamp = state.input_stamp();
            state.convert_state.render_input_stamp = Some(old_stamp.clone());
            state.convert_state.is_encoding = true;
            let (tx, rx) = std::sync::mpsc::channel();
            state.convert_state.render_rx = Some(rx);
            tx.send(ConvertWorkerMsg {
                generation: state.convert_state.request_generation,
                input_stamp: old_stamp,
                result,
            })
            .unwrap();
            state.mark_pixels_changed();

            state.poll_conversion();

            assert!(!state.convert_state.is_encoding);
            assert!(state.convert_state.render_rx.is_none());
            assert!(state.convert_state.render_error.is_none());
            assert!(state.convert_state.encoded_bytes.is_empty());
            assert!(state.convert_state.converted_rgba.is_none());
            assert!(state.status_toast.is_none());
        }
    }

    #[test]
    fn conversion_accepts_current_result_and_applies_baked_overlays_once() {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
        });
        let input_stamp = state.input_stamp();
        state.convert_state.render_input_stamp = Some(input_stamp.clone());
        state.convert_state.is_encoding = true;
        let output = RgbaImage::from_pixel(4, 4, image::Rgba([7, 8, 9, 255]));
        let (tx, rx) = std::sync::mpsc::channel();
        state.convert_state.render_rx = Some(rx);
        tx.send(ConvertWorkerMsg {
            generation: state.convert_state.request_generation,
            input_stamp,
            result: Ok((vec![4, 5, 6], output.clone())),
        })
        .unwrap();

        state.poll_conversion();
        assert!(state.convert_state.output_matches(&state.input_stamp()));
        assert_eq!(state.convert_state.encoded_bytes, vec![4, 5, 6]);

        state.apply_converted_to_canvas();
        assert_eq!(state.current_rgba, output);
        assert_eq!(state.pixel_revision, 1);
        assert!(state.redaction_state.boxes.is_empty());
        assert!(!state.convert_state.output_matches(&state.input_stamp()));
        let scheduled_generation = state.convert_state.request_generation;
        assert!(state.convert_state.params_dirty_at.is_some());
        state.set_tool_mode(ImageToolMode::View);
        state.set_tool_mode(ImageToolMode::Convert);
        assert_eq!(state.convert_state.request_generation, scheduled_generation);
    }
}
