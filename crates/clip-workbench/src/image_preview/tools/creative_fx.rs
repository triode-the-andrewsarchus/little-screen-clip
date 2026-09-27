use super::super::*;
use crate::shell::{render_context_slider, slider_double_clicked};

/// Message sent back from background thread rendering full creative FX.
pub enum CreativeFxMsg {
    Done {
        fx_type: CreativeFxType,
        rgba: RgbaImage,
        duration_ms: u128,
    },
    Error(String),
}

/// Message sent back from low-priority background thread rendering mini 42x28 thumbnails.
pub struct CreativeFxThumbnailMsg {
    pub epoch: u64,
    pub fx_type: CreativeFxType,
    pub rgba: RgbaImage,
}

fn render_creative_fx_mask_loading(ui: &mut egui::Ui, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add(
            egui::Spinner::new()
                .size(crate::ui_ext::CONTROL_ICON_SIZE)
                .color(color),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(label)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .monospace()
                .color(color),
        );
    });
}

fn render_creative_fx_mask_button(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    loading: bool,
    width: f32,
    palette: &AppPalette,
) -> egui::Response {
    let fill = if active {
        hex_to_c32(&palette.diff_add)
    } else {
        hex_to_c32(&palette.colors.surface_panel)
    };
    let foreground = contrast_text_c32(
        fill,
        Some(hex_to_c32(&palette.colors.text_primary)),
        palette,
    );
    let button = egui::Button::new(
        egui::RichText::new(label)
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .strong()
            .color(foreground),
    )
    .fill(fill)
    .stroke(Stroke::new(1.0_f32, fill))
    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
    .min_size(Vec2::new(width, crate::ui_ext::CONTROL_HEIGHT));
    ui.add_enabled(!loading, button)
}

/// All state for the Creative FX tab.
pub struct CreativeFxState {
    pub selected_fx: CreativeFxType,
    pub lens_params: LensAberrationParams,
    pub trichromatic_params: TrichromaticParams,
    pub pixel_sort_params: PixelSortParams,
    pub cmyk_params: CmykHalftoneParams,
    pub dither_params: DitherPaletteParams,
    pub ntsc_params: NtscParams,
    pub structure_params: StructureParams,

    pub selected_preset_name: String,

    // Preview state
    pub preview_rgba: Option<RgbaImage>,
    pub preview_texture: Option<egui::TextureHandle>,
    pub dirty_preview_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub last_render_duration_ms: u128,

    // Async render worker (Main preview image)
    pub is_rendering: bool,
    pub render_rx: Option<std::sync::mpsc::Receiver<CreativeFxMsg>>,
    pub render_error: Option<String>,

    // Async thumbnail worker (Sidebar 42x28 mini-photos for each FX)
    pub fx_thumbnails: std::collections::HashMap<CreativeFxType, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(CreativeFxType, RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<CreativeFxThumbnailMsg>>,
    pub thumbnail_epoch: u64,
    pub is_generating_thumbnails: bool,

    // Debounce timer
    pub params_dirty_at: Option<std::time::Instant>,
    pub auto_fitted: bool,

    // Depth mask state
    pub cached_depth_map: Option<RgbaImage>,
    pub depth_estimation_in_progress: bool,
    pub depth_estimation_rx: Option<std::sync::mpsc::Receiver<Result<RgbaImage, String>>>,
    pub depth_estimation_error: Option<String>,

    // Background-removal alpha mask state
    pub cached_background_removal: Option<RgbaImage>,
    pub alpha_mask_in_progress: bool,
    pub alpha_mask_rx: Option<std::sync::mpsc::Receiver<Result<RgbaImage, String>>>,
    pub alpha_mask_error: Option<String>,
}

impl Default for CreativeFxState {
    fn default() -> Self {
        Self {
            selected_fx: CreativeFxType::LensAberration,
            lens_params: LensAberrationParams::default(),
            trichromatic_params: TrichromaticParams::default(),
            pixel_sort_params: PixelSortParams::default(),
            cmyk_params: CmykHalftoneParams::default(),
            dither_params: DitherPaletteParams::default(),
            ntsc_params: NtscParams::default(),
            structure_params: StructureParams::default(),
            selected_preset_name: "Default".to_string(),
            preview_rgba: None,
            preview_texture: None,
            dirty_preview_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            last_render_duration_ms: 0,
            is_rendering: false,
            render_rx: None,
            render_error: None,
            fx_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_epoch: 0,
            is_generating_thumbnails: false,
            params_dirty_at: None,
            auto_fitted: false,
            cached_depth_map: None,
            depth_estimation_in_progress: false,
            depth_estimation_rx: None,
            depth_estimation_error: None,
            cached_background_removal: None,
            alpha_mask_in_progress: false,
            alpha_mask_rx: None,
            alpha_mask_error: None,
        }
    }
}

impl CreativeFxState {
    pub fn mark_dirty(&mut self) {
        self.params_dirty_at = Some(std::time::Instant::now());
    }

    pub fn depth_mask_for_active_fx(&self) -> &DepthMaskConfig {
        match self.selected_fx {
            CreativeFxType::LensAberration => &self.lens_params.depth_mask,
            CreativeFxType::Trichromatic => &self.trichromatic_params.depth_mask,
            CreativeFxType::PixelSort => &self.pixel_sort_params.depth_mask,
            CreativeFxType::CmykHalftone => &self.cmyk_params.depth_mask,
            CreativeFxType::DitherPalette => &self.dither_params.depth_mask,
            CreativeFxType::Ntsc => &self.ntsc_params.depth_mask,
            CreativeFxType::Structure => &self.structure_params.depth_mask,
        }
    }

    pub fn depth_mask_for_active_fx_mut(&mut self) -> &mut DepthMaskConfig {
        match self.selected_fx {
            CreativeFxType::LensAberration => &mut self.lens_params.depth_mask,
            CreativeFxType::Trichromatic => &mut self.trichromatic_params.depth_mask,
            CreativeFxType::PixelSort => &mut self.pixel_sort_params.depth_mask,
            CreativeFxType::CmykHalftone => &mut self.cmyk_params.depth_mask,
            CreativeFxType::DitherPalette => &mut self.dither_params.depth_mask,
            CreativeFxType::Ntsc => &mut self.ntsc_params.depth_mask,
            CreativeFxType::Structure => &mut self.structure_params.depth_mask,
        }
    }

    pub fn current_params(&self) -> CreativeFxParams {
        match self.selected_fx {
            CreativeFxType::LensAberration => CreativeFxParams::LensAberration(self.lens_params),
            CreativeFxType::Trichromatic => {
                CreativeFxParams::Trichromatic(self.trichromatic_params)
            }
            CreativeFxType::PixelSort => CreativeFxParams::PixelSort(self.pixel_sort_params),
            CreativeFxType::CmykHalftone => CreativeFxParams::CmykHalftone(self.cmyk_params),
            CreativeFxType::DitherPalette => CreativeFxParams::DitherPalette(self.dither_params),
            CreativeFxType::Ntsc => CreativeFxParams::Ntsc(self.ntsc_params),
            CreativeFxType::Structure => CreativeFxParams::Structure(self.structure_params),
        }
    }
}

impl ImagePreviewState {
    pub fn run_creative_fx_render(&mut self) {
        if self.creative_fx_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }

        let input_rgba = self.current_rgba.clone();
        let selected_fx = self.creative_fx_state.selected_fx;
        let params = self.creative_fx_state.current_params();
        let cached_depth = self.creative_fx_state.cached_depth_map.clone();
        let cached_background_removal = self.creative_fx_state.cached_background_removal.clone();

        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxMsg>();
        self.creative_fx_state.render_rx = Some(rx);
        self.creative_fx_state.is_rendering = true;
        self.creative_fx_state.render_error = None;

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply_creative_fx_with_masks(
                    &input_rgba,
                    cached_depth.as_ref(),
                    cached_background_removal.as_ref(),
                    &params,
                )
            }));
            match process_result {
                Ok(out_rgba) => {
                    let duration_ms = start.elapsed().as_millis();
                    let _ = tx.send(CreativeFxMsg::Done {
                        fx_type: selected_fx,
                        rgba: out_rgba,
                        duration_ms,
                    });
                }
                Err(e) => {
                    let err_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Creative FX processing panicked".to_string()
                    };
                    let _ = tx.send(CreativeFxMsg::Error(err_msg));
                }
            }
        });
    }

    /// Spawn background thread to estimate AI depth map using Depth-Anything V2.
    pub fn run_depth_estimation_for_fx(&mut self) {
        if self.creative_fx_state.depth_estimation_in_progress || self.current_rgba.is_empty() {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            self.creative_fx_state.depth_estimation_error =
                Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        let depth_script = "scripts/depth_estimation.py";
        let (cmd_bin, cmd_args) = if std::path::Path::new(depth_script).exists() {
            (depth_script.to_string(), Vec::new())
        } else {
            ("python3".to_string(), vec![depth_script.to_string()])
        };

        let (tx, rx) = std::sync::mpsc::channel::<Result<RgbaImage, String>>();
        self.creative_fx_state.depth_estimation_rx = Some(rx);
        self.creative_fx_state.depth_estimation_in_progress = true;
        self.creative_fx_state.depth_estimation_error = None;

        std::thread::spawn(move || {
            let mut cmd = Command::new(&cmd_bin);
            cmd.args(&cmd_args).arg("-").arg("-");

            let mut child = match cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(format!(
                        "Failed to launch depth estimation '{}': {}. Ensure Python environment is setup.",
                        cmd_bin, e
                    )));
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    let _ = tx.send(Err(format!("Failed to write image data to worker: {}", e)));
                    return;
                }
            }

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    let _ = tx.send(Err(format!("Depth estimation process failed: {}", e)));
                    return;
                }
            };

            if !output.status.success() {
                let err_str = String::from_utf8_lossy(&output.stderr);
                let _ = tx.send(Err(format!("Depth estimation failed:\n{}", err_str.trim())));
                return;
            }

            match image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png) {
                Ok(dyn_img) => {
                    let _ = tx.send(Ok(dyn_img.to_rgba8()));
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("Failed to decode output PNG: {}", e)));
                }
            }
        });
    }

    /// Spawn automatic FeyNoBg removal to provide an alpha mask for Creative FX.
    pub fn run_alpha_mask_for_fx(&mut self) {
        if self.creative_fx_state.alpha_mask_in_progress || self.current_rgba.is_empty() {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(error) = base_img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            self.creative_fx_state.alpha_mask_error =
                Some(format!("Failed to encode image to PNG: {}", error));
            return;
        }

        let configured_cmd = self.bg_removal_cmd.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<RgbaImage, String>>();
        self.creative_fx_state.alpha_mask_rx = Some(rx);
        self.creative_fx_state.alpha_mask_in_progress = true;
        self.creative_fx_state.alpha_mask_error = None;

        std::thread::spawn(move || {
            let result = super::background_removal::remove_background_rgba(
                &png_bytes,
                configured_cmd.as_deref(),
                None,
            );
            let _ = tx.send(result);
        });
    }

    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all FX.
    pub fn run_creative_fx_thumbnail_batch(&mut self) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.creative_fx_state.thumbnail_epoch += 1;
        let epoch = self.creative_fx_state.thumbnail_epoch;

        // Downscale base image to 42x28 (sub-millisecond)
        let thumb_base = image::imageops::resize(
            &self.current_rgba,
            42,
            28,
            image::imageops::FilterType::Nearest,
        );

        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxThumbnailMsg>();
        self.creative_fx_state.thumbnail_rx = Some(rx);
        self.creative_fx_state.is_generating_thumbnails = true;

        std::thread::Builder::new()
            .name("creative_fx_thumbnails".to_string())
            .spawn(move || {
                for fx in CreativeFxType::all() {
                    let preset = default_preset_for_fx(*fx);
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        apply_creative_fx(&thumb_base, &preset.params)
                    }));
                    if let Ok(rgba) = res {
                        let _ = tx.send(CreativeFxThumbnailMsg {
                            epoch,
                            fx_type: *fx,
                            rgba,
                        });
                    }
                }
            })
            .ok();
    }

    /// Poll messages from Creative FX background render, depth estimation, and thumbnail threads.
    pub fn poll_creative_fx(&mut self) {
        if let Some(rx) = self.creative_fx_state.render_rx.take() {
            match rx.try_recv() {
                Ok(CreativeFxMsg::Done {
                    fx_type,
                    rgba,
                    duration_ms,
                }) => {
                    if fx_type == self.creative_fx_state.selected_fx {
                        self.creative_fx_state.preview_rgba = Some(rgba);
                        self.creative_fx_state.dirty_preview_texture = true;
                        self.creative_fx_state.last_render_duration_ms = duration_ms;
                    }
                    self.creative_fx_state.is_rendering = false;
                    self.creative_fx_state.render_error = None;
                }
                Ok(CreativeFxMsg::Error(e)) => {
                    self.creative_fx_state.render_error = Some(e);
                    self.creative_fx_state.is_rendering = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.creative_fx_state.render_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.creative_fx_state.is_rendering = false;
                }
            }
        }

        if let Some(rx) = self.creative_fx_state.depth_estimation_rx.take() {
            match rx.try_recv() {
                Ok(Ok(depth_rgba)) => {
                    self.creative_fx_state.cached_depth_map = Some(depth_rgba);
                    self.creative_fx_state.depth_estimation_in_progress = false;
                    self.creative_fx_state.depth_estimation_error = None;
                    self.creative_fx_state.depth_mask_for_active_fx_mut().source =
                        CreativeFxMaskSource::DepthMap;
                    self.creative_fx_state
                        .depth_mask_for_active_fx_mut()
                        .enabled = true;
                    self.run_creative_fx_render();
                }
                Ok(Err(e)) => {
                    self.creative_fx_state.depth_estimation_error = Some(e);
                    self.creative_fx_state.depth_estimation_in_progress = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.creative_fx_state.depth_estimation_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.creative_fx_state.depth_estimation_in_progress = false;
                }
            }
        }

        if let Some(rx) = self.creative_fx_state.alpha_mask_rx.take() {
            match rx.try_recv() {
                Ok(Ok(removal_result)) => {
                    self.creative_fx_state.cached_background_removal = Some(removal_result);
                    self.creative_fx_state.alpha_mask_in_progress = false;
                    self.creative_fx_state.alpha_mask_error = None;
                    self.creative_fx_state.depth_mask_for_active_fx_mut().source =
                        CreativeFxMaskSource::BackgroundRemovalAlpha;
                    self.creative_fx_state
                        .depth_mask_for_active_fx_mut()
                        .enabled = true;
                    self.run_creative_fx_render();
                }
                Ok(Err(error)) => {
                    self.creative_fx_state.alpha_mask_error = Some(error);
                    self.creative_fx_state.alpha_mask_in_progress = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.creative_fx_state.alpha_mask_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.creative_fx_state.alpha_mask_in_progress = false;
                }
            }
        }

        if let Some(rx) = self.creative_fx_state.thumbnail_rx.take() {
            let mut disconnected = false;
            while let Ok(msg) = rx.try_recv() {
                if msg.epoch == self.creative_fx_state.thumbnail_epoch {
                    self.creative_fx_state
                        .pending_thumbnails
                        .push((msg.fx_type, msg.rgba));
                }
            }
            match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                }
                _ => {}
            }
            if disconnected {
                self.creative_fx_state.is_generating_thumbnails = false;
            } else {
                self.creative_fx_state.thumbnail_rx = Some(rx);
            }
        }
    }

    /// Commit creative FX result into current_rgba and push to undo stack.

    pub fn apply_creative_fx(&mut self) {
        let preview = match self.creative_fx_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        let fx_name = self.creative_fx_state.selected_fx.name();
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = preview;
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.creative_fx_state.preview_rgba = None;
        self.creative_fx_state.preview_texture = None;
        self.creative_fx_state.dirty_preview_texture = false;

        self.creative_fx_state.fx_thumbnails.clear();
        self.creative_fx_state.pending_thumbnails.clear();
        self.run_creative_fx_thumbnail_batch();
        self.show_toast(format!("✨ {} applied", fx_name));
    }

    fn toggle_creative_fx_mask_source(&mut self, source: CreativeFxMaskSource) {
        let active = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == source
        };
        if active {
            self.creative_fx_state
                .depth_mask_for_active_fx_mut()
                .enabled = false;
            self.creative_fx_state.mark_dirty();
            return;
        }

        let cached = match source {
            CreativeFxMaskSource::DepthMap => self.creative_fx_state.cached_depth_map.is_some(),
            CreativeFxMaskSource::BackgroundRemovalAlpha => {
                self.creative_fx_state.cached_background_removal.is_some()
            }
        };
        if cached {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = source;
            config.enabled = true;
            self.creative_fx_state.mark_dirty();
        } else {
            match source {
                CreativeFxMaskSource::DepthMap => self.run_depth_estimation_for_fx(),
                CreativeFxMaskSource::BackgroundRemovalAlpha => self.run_alpha_mask_for_fx(),
            }
        }
    }

    fn render_creative_fx_mask_sources(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        full_w: f32,
    ) {
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let config = *self.creative_fx_state.depth_mask_for_active_fx();
        let depth_active = config.enabled && config.source == CreativeFxMaskSource::DepthMap;
        let alpha_active =
            config.enabled && config.source == CreativeFxMaskSource::BackgroundRemovalAlpha;
        let has_depth = self.creative_fx_state.cached_depth_map.is_some();
        let has_alpha = self.creative_fx_state.cached_background_removal.is_some();
        let depth_loading = self.creative_fx_state.depth_estimation_in_progress;
        let alpha_loading = self.creative_fx_state.alpha_mask_in_progress;
        let mask_loading = depth_loading || alpha_loading;

        ui.label(
            egui::RichText::new("Masking")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_high),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        if depth_loading {
            render_creative_fx_mask_loading(
                ui,
                "Estimating depth map with Depth-Anything...",
                base_c,
            );
            ctx.request_repaint();
        } else if alpha_loading {
            render_creative_fx_mask_loading(ui, "Removing background with FeyNoBg...", base_c);
            ctx.request_repaint();
        }

        let depth_label = if depth_active {
            "🌊 Depth Mask: ON"
        } else if has_depth {
            "🌊 Depth map • cached"
        } else {
            "🌊 Use depth map as mask"
        };
        if render_creative_fx_mask_button(
            ui,
            depth_label,
            depth_active,
            mask_loading,
            full_w,
            palette,
        )
        .on_hover_text("Use AI monocular depth to mask this effect")
        .clicked()
        {
            self.toggle_creative_fx_mask_source(CreativeFxMaskSource::DepthMap);
        }

        ui.add_space(3.0);
        let alpha_label = if alpha_active {
            "✂ Removal Alpha: ON"
        } else if has_alpha {
            "✂ Removal alpha • cached"
        } else {
            "✂ Use removal alpha as mask"
        };
        if render_creative_fx_mask_button(
            ui,
            alpha_label,
            alpha_active,
            mask_loading,
            full_w,
            palette,
        )
        .on_hover_text("Use the previous Remove BG result, or run automatic FeyNoBg removal")
        .clicked()
        {
            self.toggle_creative_fx_mask_source(CreativeFxMaskSource::BackgroundRemovalAlpha);
        }

        if let Some(error) = &self.creative_fx_state.depth_estimation_error {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("⚠ {}", error))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(diff_del),
            );
        }
        if let Some(error) = &self.creative_fx_state.alpha_mask_error {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("⚠ {}", error))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(diff_del),
            );
        }
    }

    fn render_creative_fx_mask_options(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        cur_fx: CreativeFxType,
        full_w: f32,
        slider_w: f32,
    ) {
        let config = *self.creative_fx_state.depth_mask_for_active_fx();
        if !config.enabled {
            return;
        }

        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        ui.add_space(crate::ui_ext::SPACE_XS);
        let invert_text = match (config.source, config.invert) {
            (CreativeFxMaskSource::DepthMap, true) => "⇄ Invert Mask: ON (Far / Background)",
            (CreativeFxMaskSource::DepthMap, false) => "⇄ Invert Mask: OFF (Near / Foreground)",
            (CreativeFxMaskSource::BackgroundRemovalAlpha, true) => {
                "⇄ Invert Mask: ON (Background)"
            }
            (CreativeFxMaskSource::BackgroundRemovalAlpha, false) => {
                "⇄ Invert Mask: OFF (Foreground)"
            }
        };
        let invert_button = egui::Button::new(
            egui::RichText::new(invert_text)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_high),
        )
        .fill(b_med)
        .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
        .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));
        if ui
            .add(invert_button)
            .on_hover_text("Swap the foreground and background mask regions")
            .clicked()
        {
            self.creative_fx_state.depth_mask_for_active_fx_mut().invert = !config.invert;
            self.creative_fx_state.mark_dirty();
        }

        let supports_depth_cutoff = matches!(
            cur_fx,
            CreativeFxType::PixelSort
                | CreativeFxType::CmykHalftone
                | CreativeFxType::DitherPalette
                | CreativeFxType::Ntsc
                | CreativeFxType::Structure
        );
        if config.source != CreativeFxMaskSource::DepthMap || !supports_depth_cutoff {
            return;
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let mut threshold = config.threshold;
        if render_context_slider(
            ui,
            "Depth Cutoff (0: Continuous gradient)",
            &mut threshold,
            0.0..=1.0,
            0.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state
                .depth_mask_for_active_fx_mut()
                .threshold = threshold;
            self.creative_fx_state.mark_dirty();
        }

        if threshold > 0.001 {
            let mut feather = config.feather;
            if render_context_slider(
                ui,
                "Feather Softness",
                &mut feather,
                0.01..=0.50,
                0.25,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state
                    .depth_mask_for_active_fx_mut()
                    .feather = feather;
                self.creative_fx_state.mark_dirty();
            }
        }
    }

    /// Full 3-panel Creative FX viewport (Left: FX list; Center: Split Canvas; Right: Controls).

    pub(crate) fn render_creative_fx_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_add = hex_to_c32(&palette.diff_add);

        // Upload any newly received thumbnails into GPU textures
        for (fx_type, rgba) in self.creative_fx_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("creative_fx_thumb_{:?}", fx_type),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.creative_fx_state.fx_thumbnails.insert(fx_type, tex);
        }

        // Kick off thumbnail generation if empty
        if self.creative_fx_state.fx_thumbnails.is_empty()
            && !self.creative_fx_state.is_generating_thumbnails
            && !self.current_rgba.is_empty()
        {
            self.run_creative_fx_thumbnail_batch();
        }

        let panel_layout = ImageToolMode::CreativeFx
            .panel_layout()
            .expect("Creative FX has a panel layout")
            .with_panel_sizes(&self.panel_state);
        let panels = panel_layout.allocate(ui);
        let left = panels.left.expect("Creative FX has a browser rail");
        let right = panels.right.expect("Creative FX has an inspector rail");
        let left_w = left.width();
        let left_h = left.height();
        let right_w = right.width();
        let right_h = right.height();
        let center_w = panels.center.width();
        let main_h = panels.center.height();

        // Fit to view when first opening Creative FX
        if !self.creative_fx_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.creative_fx_state.auto_fitted = true;
        }

        // ── 1. LEFT PANEL: Creative FX List with 42x28 mini preview photos ──
        crate::shell::StudioPanelLayout::show_region(ui, left, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(left_w - 16.0);
                    ui.set_height((left_h - 16.0).max(1.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("✨ Creative FX")
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(base_c),
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", CreativeFxType::all().len()))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        if self.creative_fx_state.is_generating_thumbnails {
                            ui.label(
                                egui::RichText::new("⏳")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            )
                            .on_hover_text("Generating preview thumbnails…");
                        }
                    });
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_XS);

                    egui::ScrollArea::vertical()
                        .id_source("creative_fx_list_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(left_w - 24.0);
                            for fx in CreativeFxType::all() {
                                let selected = self.creative_fx_state.selected_fx == *fx;
                                let (row_bg, name_col, meta_col) = if selected {
                                    (
                                        base_c,
                                        contrast_text_c32(base_c, None, palette),
                                        contrast_text_c32(base_c, None, palette),
                                    )
                                } else {
                                    (b_low, f_high, f_med)
                                };

                                let row_frame = egui::Frame::none()
                                    .fill(row_bg)
                                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                    .inner_margin(egui::Margin::symmetric(6.0, 5.0));

                                let resp = row_frame
                                    .show(ui, |ui| {
                                        ui.set_width(left_w - 32.0);
                                        ui.horizontal(|ui| {
                                            // 42x28 mini thumbnail preview
                                            if let Some(tex) =
                                                self.creative_fx_state.fx_thumbnails.get(fx)
                                            {
                                                ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                            } else {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(42.0, 28.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    Rounding::same(2.0),
                                                    b_med,
                                                );
                                            }
                                            ui.add_space(crate::ui_ext::SPACE_XS);
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(fx.name())
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(name_col),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(fx.category())
                                                            .monospace()
                                                            .color(meta_col),
                                                    );
                                                });
                                                ui.label(
                                                    egui::RichText::new(fx.subtitle())
                                                        .color(meta_col)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                            });
                                        });
                                    })
                                    .response;

                                if resp.interact(egui::Sense::click()).clicked() && !selected {
                                    self.creative_fx_state.selected_fx = *fx;
                                    self.creative_fx_state.mark_dirty();
                                }
                                ui.add_space(crate::ui_ext::SPACE_XS);
                            }
                        });
                });
        });

        // ── 2. CENTER PANEL: Split-view Comparison Canvas ──
        crate::shell::StudioPanelLayout::show_region(ui, panels.center, |ui| {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.canvas))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.set_width(center_w);
                    ui.set_height(main_h);

                    // Reload preview texture if dirty
                    if self.creative_fx_state.dirty_preview_texture {
                        if let Some(ref rgba) = self.creative_fx_state.preview_rgba {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            );
                            self.creative_fx_state.preview_texture = Some(ctx.load_texture(
                                "creative_fx_preview_tex",
                                ci,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        self.creative_fx_state.dirty_preview_texture = false;
                    }

                    let (response, painter) = ui.allocate_painter(
                        Vec2::new(center_w, main_h),
                        egui::Sense::click_and_drag(),
                    );
                    let canvas_rect = response.rect;

                    painter.rect_filled(
                        canvas_rect,
                        Rounding::same(4.0),
                        hex_to_c32(&palette.colors.canvas),
                    );

                    // If mouse is near the comparison split divider when drag starts, initiate split dragging
                    let near_split = if let Some(pos) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                    {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));
                        let split_x = img_rect.min.x
                            + self.creative_fx_state.comparison_split.clamp(0.01, 0.99)
                                * img_rect.width();
                        (pos.x - split_x).abs() <= 18.0
                            && pos.y >= img_rect.min.y
                            && pos.y <= img_rect.max.y
                    } else {
                        false
                    };
                    if near_split && response.drag_started() {
                        self.creative_fx_state.is_dragging_split = true;
                    }

                    // Pan and Zoom
                    let middle_drag =
                        ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                    if (!self.creative_fx_state.is_dragging_split && response.dragged())
                        || middle_drag
                    {
                        self.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                        if scroll_delta.abs() > 0.0 {
                            let zoom_factor = (scroll_delta * 0.002).exp();
                            let old_zoom = self.zoom;
                            let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 5.0);
                            if let Some(ptr_pos) = response.hover_pos() {
                                let center = canvas_rect.center() + self.pan;
                                let to_ptr = ptr_pos - center;
                                self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                            }
                            self.zoom = new_zoom;
                        }
                    }
                    if !self.creative_fx_state.is_dragging_split && response.double_clicked() {
                        let avail_size = Vec2::new(center_w, main_h);
                        self.fit_to_view(avail_size);
                    }

                    if let Some(orig_tex) = self.texture.clone() {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                        draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                        self.render_creative_fx_comparison(
                            ui,
                            &painter,
                            &response,
                            img_rect,
                            canvas_rect,
                            ctx,
                            &orig_tex,
                            palette,
                        );

                        painter.rect_stroke(
                            img_rect,
                            Rounding::ZERO,
                            Stroke::new(
                                1.0_f32,
                                hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                            ),
                        );
                    }

                    // Rendering spinner overlay
                    if self.creative_fx_state.is_rendering {
                        let spinner_pos = canvas_rect.center();
                        let spin_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
                        let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                        painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                        painter.rect_stroke(
                            spin_rect,
                            Rounding::same(8.0),
                            Stroke::new(1.0_f32, b_med),
                        );
                        painter.text(
                            spinner_pos,
                            egui::Align2::CENTER_CENTER,
                            "⏳ Processing Creative FX…",
                            egui::FontId::proportional(12.0),
                            f_med,
                        );
                    }

                    // Error overlay
                    if let Some(ref err) = self.creative_fx_state.render_error.clone() {
                        let err_pos = Pos2::new(canvas_rect.min.x + 10.0, canvas_rect.min.y + 10.0);
                        painter.text(
                            err_pos,
                            egui::Align2::LEFT_TOP,
                            format!("⚠ {}", err),
                            egui::FontId::proportional(11.0),
                            hex_to_c32(&palette.diff_del),
                        );
                    }
                });
        });

        // ── 3. RIGHT PANEL: Controls for Selected Effect ──
        crate::shell::StudioPanelLayout::show_region(ui, right, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(right_w - 16.0);
                    ui.set_height((right_h - 16.0).max(1.0));

                    egui::ScrollArea::vertical()
                        .id_source("creative_fx_right_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let full_w = (right_w - 32.0).max(120.0);
                            let slider_w = (full_w - 68.0).max(80.0);
                            let cur_fx = self.creative_fx_state.selected_fx;

                            // Header
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(cur_fx.icon()).size(16.0));
                                ui.label(egui::RichText::new(cur_fx.name()).strong().color(base_c));
                            });
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(cur_fx.description())
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            );
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // ── Presets Selector ──
                            ui.label(
                                egui::RichText::new("🏷 Presets")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(3.0);
                            let presets = presets_for_fx(cur_fx);
                            for preset in &presets {
                                let sel =
                                    self.creative_fx_state.selected_preset_name == preset.name;
                                let (s_bg, s_fg) = if sel {
                                    (base_c, contrast_text_c32(base_c, None, palette))
                                } else {
                                    (b_med, f_med)
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(preset.name)
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(s_fg),
                                )
                                .fill(s_bg)
                                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                                if ui.add(btn).on_hover_text(preset.description).clicked() {
                                    self.creative_fx_state.selected_preset_name =
                                        preset.name.to_string();
                                    match &preset.params {
                                        CreativeFxParams::LensAberration(p) => {
                                            self.creative_fx_state.lens_params = *p;
                                        }
                                        CreativeFxParams::Trichromatic(p) => {
                                            self.creative_fx_state.trichromatic_params = *p;
                                        }
                                        CreativeFxParams::PixelSort(p) => {
                                            self.creative_fx_state.pixel_sort_params = *p;
                                        }
                                        CreativeFxParams::CmykHalftone(p) => {
                                            self.creative_fx_state.cmyk_params = *p;
                                        }
                                        CreativeFxParams::DitherPalette(p) => {
                                            self.creative_fx_state.dither_params = *p;
                                        }
                                        CreativeFxParams::Ntsc(p) => {
                                            self.creative_fx_state.ntsc_params = *p;
                                        }
                                        CreativeFxParams::Structure(p) => {
                                            self.creative_fx_state.structure_params = *p;
                                        }
                                    }
                                    self.creative_fx_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }

                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // ── Parameter Controls ──
                            ui.label(
                                egui::RichText::new("🎛 Controls")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            match cur_fx {
                                CreativeFxType::LensAberration => {
                                    // Strength: 0..100
                                    if render_context_slider(
                                        ui,
                                        "⚡ Strength",
                                        &mut self.creative_fx_state.lens_params.strength,
                                        0.0..=100.0,
                                        25.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Falloff: 0..100 (0: entire image, 100: edges only)
                                    if render_context_slider(
                                        ui,
                                        "🔘 Falloff (0: All, 100: Edges)",
                                        &mut self.creative_fx_state.lens_params.falloff,
                                        0.0..=100.0,
                                        50.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Color Balance: -50..+50 (-50: red/blue, +50: blue/yellow)
                                    if render_context_slider(
                                        ui,
                                        "🎨 Color Balance (-50: Red/Blue, +50: Blue/Yellow)",
                                        &mut self.creative_fx_state.lens_params.color_balance,
                                        -50.0..=50.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::Trichromatic => {
                                    // Mode: RGB vs OKLab
                                    ui.label(
                                        egui::RichText::new("Color Separation Model")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let modes = [
                                            (TrichromaticMode::Rgb, "RGB"),
                                            (TrichromaticMode::OkLab, "OKLab"),
                                        ];
                                        for (mode, label) in modes {
                                            let sel =
                                                self.creative_fx_state.trichromatic_params.mode
                                                    == mode;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.trichromatic_params.mode =
                                                    mode;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    let names = self
                                        .creative_fx_state
                                        .trichromatic_params
                                        .mode
                                        .channel_names();
                                    for i in 0..3 {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Channel {}: {}",
                                                i + 1,
                                                names[i]
                                            ))
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                        );
                                        ui.add_space(2.0);

                                        // Offset X
                                        if render_context_slider(
                                            ui,
                                            &format!("↔ X Offset (Ch {})", i + 1),
                                            &mut self
                                                .creative_fx_state
                                                .trichromatic_params
                                                .channels[i]
                                                .offset_x,
                                            -50.0..=50.0,
                                            0.0,
                                            "px",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        // Offset Y
                                        if render_context_slider(
                                            ui,
                                            &format!("↕ Y Offset (Ch {})", i + 1),
                                            &mut self
                                                .creative_fx_state
                                                .trichromatic_params
                                                .channels[i]
                                                .offset_y,
                                            -50.0..=50.0,
                                            0.0,
                                            "px",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        // Zoom
                                        if render_context_slider(
                                            ui,
                                            &format!("🔍 Zoom (Ch {})", i + 1),
                                            &mut self
                                                .creative_fx_state
                                                .trichromatic_params
                                                .channels[i]
                                                .zoom,
                                            0.80..=1.20,
                                            1.00,
                                            "×",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(crate::ui_ext::SPACE_XS);
                                    }

                                    // Overlap crop window calculation
                                    let (_, _, cw, ch) = trichromatic_intersection_box(
                                        self.current_rgba.width(),
                                        self.current_rgba.height(),
                                        &self.creative_fx_state.trichromatic_params.channels,
                                    );
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "✂ Crop Window: {} × {} px",
                                            cw, ch
                                        ))
                                        .monospace()
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new("(auto-cropped to 3-channel overlap)")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_low),
                                    );
                                }

                                CreativeFxType::PixelSort => {
                                    // Direction
                                    ui.label(
                                        egui::RichText::new("Scanline Direction")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let dirs = [
                                            (PixelSortDirection::Vertical, "Vertical"),
                                            (PixelSortDirection::Horizontal, "Horizontal"),
                                            (PixelSortDirection::Diagonal, "Diagonal"),
                                        ];
                                        for (d, label) in dirs {
                                            let sel =
                                                self.creative_fx_state.pixel_sort_params.direction
                                                    == d;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state
                                                    .pixel_sort_params
                                                    .direction = d;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Criteria
                                    ui.label(
                                        egui::RichText::new("Sort Metric")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let crit = [
                                            (PixelSortCriteria::Brightness, "Brightness"),
                                            (PixelSortCriteria::Hue, "Hue"),
                                            (PixelSortCriteria::Saturation, "Saturation"),
                                        ];
                                        for (c, label) in crit {
                                            let sel =
                                                self.creative_fx_state.pixel_sort_params.criteria
                                                    == c;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.pixel_sort_params.criteria =
                                                    c;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Order
                                    ui.label(
                                        egui::RichText::new("Sort Order")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let orders = [
                                            (PixelSortOrder::Ascending, "Ascending"),
                                            (PixelSortOrder::Descending, "Descending"),
                                        ];
                                        for (o, label) in orders {
                                            let sel =
                                                self.creative_fx_state.pixel_sort_params.order == o;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.pixel_sort_params.order = o;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Min Threshold
                                    if render_context_slider(
                                        ui,
                                        "Min Threshold",
                                        &mut self.creative_fx_state.pixel_sort_params.min_threshold,
                                        0.0..=1.0,
                                        0.25,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    // Max Threshold
                                    if render_context_slider(
                                        ui,
                                        "Max Threshold",
                                        &mut self.creative_fx_state.pixel_sort_params.max_threshold,
                                        0.0..=1.0,
                                        0.80,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    // Min Span
                                    let mut span_f =
                                        self.creative_fx_state.pixel_sort_params.min_span as f32;
                                    if render_context_slider(
                                        ui,
                                        "Min Span Length",
                                        &mut span_f,
                                        2.0..=50.0,
                                        6.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.pixel_sort_params.min_span =
                                            span_f.round() as usize;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    // Invert mask
                                    let mut invert =
                                        self.creative_fx_state.pixel_sort_params.invert_mask;
                                    if ui
                                        .checkbox(
                                            &mut invert,
                                            egui::RichText::new("Invert Selection Mask")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.pixel_sort_params.invert_mask =
                                            invert;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::CmykHalftone => {
                                    // Dot Size: 2..=16 px
                                    if render_context_slider(
                                        ui,
                                        "🔘 Dot Screen Size",
                                        &mut self.creative_fx_state.cmyk_params.dot_size,
                                        2.0..=16.0,
                                        5.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Dot Gain: 0.5..=2.0
                                    if render_context_slider(
                                        ui,
                                        "📈 Dot Gain (Ink Bleed)",
                                        &mut self.creative_fx_state.cmyk_params.dot_gain,
                                        0.50..=2.00,
                                        1.10,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Paper Tint
                                    ui.label(
                                        egui::RichText::new("Paper Substrate Tint")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for tint in PaperTint::all() {
                                            let sel = self.creative_fx_state.cmyk_params.paper_tint
                                                == *tint;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(tint.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.cmyk_params.paper_tint =
                                                    *tint;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                    ui.separator();
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Plate Misregistration Offsets
                                    ui.label(
                                        egui::RichText::new("🗞 Plate Misregistration Offsets")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(3.0);

                                    // Cyan plate
                                    ui.label(
                                        egui::RichText::new("Cyan Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Cyan X",
                                        &mut self.creative_fx_state.cmyk_params.cyan_offset[0],
                                        -10.0..=10.0,
                                        -1.5,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Cyan Y",
                                        &mut self.creative_fx_state.cmyk_params.cyan_offset[1],
                                        -10.0..=10.0,
                                        0.5,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Magenta plate
                                    ui.label(
                                        egui::RichText::new("Magenta Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Magenta X",
                                        &mut self.creative_fx_state.cmyk_params.magenta_offset[0],
                                        -10.0..=10.0,
                                        1.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Magenta Y",
                                        &mut self.creative_fx_state.cmyk_params.magenta_offset[1],
                                        -10.0..=10.0,
                                        -1.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Yellow plate
                                    ui.label(
                                        egui::RichText::new("Yellow Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Yellow X",
                                        &mut self.creative_fx_state.cmyk_params.yellow_offset[0],
                                        -10.0..=10.0,
                                        0.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Yellow Y",
                                        &mut self.creative_fx_state.cmyk_params.yellow_offset[1],
                                        -10.0..=10.0,
                                        1.5,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Black (Key) plate
                                    ui.label(
                                        egui::RichText::new("Black (Key) Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Black X",
                                        &mut self.creative_fx_state.cmyk_params.black_offset[0],
                                        -10.0..=10.0,
                                        0.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Black Y",
                                        &mut self.creative_fx_state.cmyk_params.black_offset[1],
                                        -10.0..=10.0,
                                        0.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::DitherPalette => {
                                    // Palette selection
                                    ui.label(
                                        egui::RichText::new("Hardware / Aesthetic Palette")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for pal in RetroPalette::all() {
                                            let sel = self.creative_fx_state.dither_params.palette
                                                == *pal;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(pal.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.dither_params.palette = *pal;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    // Dither Method selection
                                    ui.label(
                                        egui::RichText::new("Dithering Method")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for method in DitherMethod::all() {
                                            let sel = self.creative_fx_state.dither_params.method
                                                == *method;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(method.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.dither_params.method =
                                                    *method;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    // Pixel Scale selection (1x..4x)
                                    ui.label(
                                        egui::RichText::new("Pixel Grid Scale")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for scale in [1u32, 2, 3, 4] {
                                            let sel =
                                                self.creative_fx_state.dither_params.pixel_scale
                                                    == scale;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(format!("{}×", scale))
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ))
                                            .min_size(Vec2::new(32.0, 20.0));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.dither_params.pixel_scale =
                                                    scale;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Dither Strength
                                    if render_context_slider(
                                        ui,
                                        "⚡ Dither Strength",
                                        &mut self.creative_fx_state.dither_params.dither_strength,
                                        0.0..=1.0,
                                        0.80,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Pre-dither Contrast
                                    if render_context_slider(
                                        ui,
                                        "🌓 Contrast Boost",
                                        &mut self.creative_fx_state.dither_params.contrast,
                                        -50.0..=50.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::Ntsc => {
                                    // ── 1. Broadcast RF & Filtering ──
                                    ui.label(
                                        egui::RichText::new("📡 Signal & Filtering")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Luma Smear (Bandwidth)",
                                        &mut self.creative_fx_state.ntsc_params.luma_smear,
                                        0.0..=1.0,
                                        0.25,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Composite Sharpness",
                                        &mut self
                                            .creative_fx_state
                                            .ntsc_params
                                            .composite_sharpening,
                                        0.0..=3.0,
                                        1.0,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Edge Ringing / Ghosting",
                                        &mut self.creative_fx_state.ntsc_params.ringing,
                                        0.0..=2.0,
                                        0.5,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Demodulation Filter
                                    ui.label(
                                        egui::RichText::new("Demodulation Filter")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for demo in NtscDemodulation::all() {
                                            let sel =
                                                self.creative_fx_state.ntsc_params.demodulation
                                                    == *demo;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(demo.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.ntsc_params.demodulation =
                                                    *demo;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(3.0);

                                    // Scanline Phase Shift
                                    ui.label(
                                        egui::RichText::new("Subcarrier Phase Shift (Dot Crawl)")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for ps in NtscPhaseShift::all() {
                                            let sel =
                                                self.creative_fx_state.ntsc_params.phase_shift
                                                    == *ps;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(ps.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ))
                                            .min_size(Vec2::new(38.0, 20.0));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.ntsc_params.phase_shift =
                                                    *ps;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(3.0);

                                    // Chroma Lowpass
                                    ui.label(
                                        egui::RichText::new("Chroma Lowpass Filter")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for cl in NtscChromaLowpass::all() {
                                            let sel =
                                                self.creative_fx_state.ntsc_params.chroma_lowpass
                                                    == *cl;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(cl.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.ntsc_params.chroma_lowpass =
                                                    *cl;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(3.0);

                                    let mut scanlines =
                                        self.creative_fx_state.ntsc_params.scanlines;
                                    if ui
                                        .checkbox(
                                            &mut scanlines,
                                            egui::RichText::new("📺 CRT Interlaced Scanlines")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.ntsc_params.scanlines = scanlines;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // ── 2. Chrominance & YIQ ──
                                    ui.separator();
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new("🎨 Chrominance & Color")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "↔ Chroma Horizontal Bleed",
                                        &mut self.creative_fx_state.ntsc_params.chroma_delay_x,
                                        -8.0..=8.0,
                                        1.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    let mut delay_y =
                                        self.creative_fx_state.ntsc_params.chroma_delay_y as f32;
                                    if render_context_slider(
                                        ui,
                                        "↕ Chroma Vertical Shift",
                                        &mut delay_y,
                                        -4.0..=4.0,
                                        0.0,
                                        "lines",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.ntsc_params.chroma_delay_y =
                                            delay_y.round() as i32;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    let mut vert_blend =
                                        self.creative_fx_state.ntsc_params.chroma_vert_blend;
                                    if ui
                                        .checkbox(
                                            &mut vert_blend,
                                            egui::RichText::new("Chroma Vertical Blending")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.ntsc_params.chroma_vert_blend =
                                            vert_blend;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "🎨 Global Tint / Hue Shift (0: Normal)",
                                        &mut self.creative_fx_state.ntsc_params.chroma_phase_error,
                                        0.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "〰 Chroma Phase Jitter",
                                        &mut self.creative_fx_state.ntsc_params.chroma_phase_noise,
                                        0.0..=0.10,
                                        0.005,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "🌈 Color Grain / Chroma Noise",
                                        &mut self.creative_fx_state.ntsc_params.chroma_noise,
                                        0.0..=0.50,
                                        0.05,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // ── 3. Noise & Static ──
                                    ui.separator();
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new("❄ Noise & Static")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("RF Static / Snow")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        );
                                        ui.spacing_mut().slider_width = slider_w;
                                        let mut snow = self.creative_fx_state.ntsc_params.snow;
                                        let slider_response = ui.add(
                                            egui::Slider::new(&mut snow, 0.0..=0.04)
                                                .max_decimals(4),
                                        );
                                        let default_snow = NtscParams::default().snow;
                                        let slider_response =
                                            slider_response.on_hover_text(format!(
                                                "Double-click to reset (default: {:.4})",
                                                default_snow
                                            ));
                                        let is_double_click =
                                            slider_double_clicked(ui, &slider_response);
                                        if is_double_click {
                                            snow = default_snow;
                                        }
                                        if slider_response.changed() || is_double_click {
                                            self.creative_fx_state.ntsc_params.snow = snow;
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                    });
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Snow Streaking (Anisotropy)",
                                        &mut self.creative_fx_state.ntsc_params.snow_anisotropy,
                                        0.0..=1.0,
                                        0.5,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Luma Noise Grain",
                                        &mut self.creative_fx_state.ntsc_params.luma_noise,
                                        0.0..=0.50,
                                        0.01,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Composite Signal Noise",
                                        &mut self.creative_fx_state.ntsc_params.composite_noise,
                                        0.0..=0.50,
                                        0.05,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // ── 4. VHS Magnetic Tape Simulation ──
                                    ui.separator();
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new("📼 VHS Tape Emulation")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    let mut vhs_enabled =
                                        self.creative_fx_state.ntsc_params.vhs_enabled;
                                    if ui
                                        .checkbox(
                                            &mut vhs_enabled,
                                            egui::RichText::new("Enable VHS Tape Pass")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.ntsc_params.vhs_enabled =
                                            vhs_enabled;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }

                                    if self.creative_fx_state.ntsc_params.vhs_enabled {
                                        ui.add_space(2.0);
                                        ui.label(
                                            egui::RichText::new("Tape Speed")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        );
                                        ui.horizontal_wrapped(|ui| {
                                            for speed in [
                                                VhsTapeSpeed::SP,
                                                VhsTapeSpeed::LP,
                                                VhsTapeSpeed::EP,
                                            ] {
                                                let sel =
                                                    self.creative_fx_state.ntsc_params.tape_speed
                                                        == speed;
                                                let (b_bg, b_fg) = if sel {
                                                    (
                                                        base_c,
                                                        contrast_text_c32(base_c, None, palette),
                                                    )
                                                } else {
                                                    (b_med, f_med)
                                                };
                                                let btn = egui::Button::new(
                                                    egui::RichText::new(speed.label())
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(b_fg),
                                                )
                                                .fill(b_bg)
                                                .stroke(Stroke::new(
                                                    1.0_f32,
                                                    if sel { base_c } else { b_med },
                                                ))
                                                .rounding(Rounding::same(
                                                    crate::ui_ext::CONTROL_CORNER_RADIUS,
                                                ));
                                                if ui.add(btn).clicked() && !sel {
                                                    self.creative_fx_state.ntsc_params.tape_speed =
                                                        speed;
                                                    self.creative_fx_state.selected_preset_name =
                                                        "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                            }
                                        });
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "〰 Tape Wobble / Jitter",
                                            &mut self.creative_fx_state.ntsc_params.edge_wave,
                                            0.0..=2.0,
                                            0.5,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "📶 Tracking Noise Bar",
                                            &mut self.creative_fx_state.ntsc_params.tracking_noise,
                                            0.0..=1.0,
                                            0.0,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "Playhead Sharpening",
                                            &mut self.creative_fx_state.ntsc_params.vhs_sharpen,
                                            0.0..=1.0,
                                            0.25,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "Chroma Loss / Dropout",
                                            &mut self.creative_fx_state.ntsc_params.vhs_chroma_loss,
                                            0.0..=1.0,
                                            0.05,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        let mut head_sw =
                                            self.creative_fx_state.ntsc_params.head_switching;
                                        if ui
                                            .checkbox(
                                                &mut head_sw,
                                                egui::RichText::new(
                                                    "Head-Switching Jitter (Bottom)",
                                                )
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                            )
                                            .changed()
                                        {
                                            self.creative_fx_state.ntsc_params.head_switching =
                                                head_sw;
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                    }

                                    ui.add_space(3.0);
                                    ui.separator();
                                    ui.add_space(2.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("🎲 Noise Random Seed")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        );
                                        ui.spacing_mut().slider_width = slider_w;
                                        let mut seed =
                                            self.creative_fx_state.ntsc_params.random_seed;
                                        let slider_response =
                                            ui.add(egui::Slider::new(&mut seed, 0..=1000));
                                        let default_seed = NtscParams::default().random_seed;
                                        let slider_response =
                                            slider_response.on_hover_text(format!(
                                                "Double-click to reset (default: {})",
                                                default_seed
                                            ));
                                        let is_double_click =
                                            slider_double_clicked(ui, &slider_response);
                                        if is_double_click {
                                            seed = default_seed;
                                        }
                                        if slider_response.changed() || is_double_click {
                                            self.creative_fx_state.ntsc_params.random_seed = seed;
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                    });
                                }
                                CreativeFxType::Structure => {
                                    // Structure Amount: -100..+100
                                    if render_context_slider(
                                        ui,
                                        "⚡ Structure Amount (-100: Soften, +100: Enhance)",
                                        &mut self.creative_fx_state.structure_params.amount,
                                        -100.0..=100.0,
                                        35.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Detail Scale: -50..+50
                                    if render_context_slider(
                                        ui,
                                        "🔍 Detail Scale (-50: Fine, +50: Coarse)",
                                        &mut self.creative_fx_state.structure_params.detail_scale,
                                        -50.0..=50.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Edge Protection: 0..100
                                    if render_context_slider(
                                        ui,
                                        "🛡 Edge Protection (0: Off, 100: Strict)",
                                        &mut self
                                            .creative_fx_state
                                            .structure_params
                                            .edge_protection,
                                        0.0..=100.0,
                                        50.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }
                            }

                            // ── Masking Controls ──
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            ui.separator();
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            self.render_creative_fx_mask_sources(ui, ctx, palette, full_w);
                            self.render_creative_fx_mask_options(
                                ui, palette, cur_fx, full_w, slider_w,
                            );

                            ui.add_space(crate::ui_ext::SPACE_SM);
                            ui.separator();
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                            // ── Action Buttons ──
                            let apply_fg = contrast_text_c32(diff_add, Some(f_high), palette);
                            let apply_btn = egui::Button::new(
                                egui::RichText::new("✓ Apply to Image")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(apply_fg),
                            )
                            .fill(diff_add)
                            .stroke(Stroke::new(1.0_f32, diff_add))
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                            let apply_enabled = self.creative_fx_state.preview_rgba.is_some();
                            if ui
                                .add_enabled(apply_enabled, apply_btn)
                                .on_hover_text(
                                    "Commit Creative FX result to image (pushes to undo stack)",
                                )
                                .clicked()
                            {
                                self.apply_creative_fx();
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let reset_fg = contrast_text_c32(b_med, Some(f_med), palette);
                            let reset_btn = egui::Button::new(
                                egui::RichText::new("⟲ Reset Defaults")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(reset_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0_f32, b_low))
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                            if ui
                                .add(reset_btn)
                                .on_hover_text("Reset parameters to default")
                                .clicked()
                            {
                                match cur_fx {
                                    CreativeFxType::LensAberration => {
                                        self.creative_fx_state.lens_params =
                                            LensAberrationParams::default();
                                    }
                                    CreativeFxType::Trichromatic => {
                                        self.creative_fx_state.trichromatic_params =
                                            TrichromaticParams::default();
                                    }
                                    CreativeFxType::PixelSort => {
                                        self.creative_fx_state.pixel_sort_params =
                                            PixelSortParams::default();
                                    }
                                    CreativeFxType::CmykHalftone => {
                                        self.creative_fx_state.cmyk_params =
                                            CmykHalftoneParams::default();
                                    }
                                    CreativeFxType::DitherPalette => {
                                        self.creative_fx_state.dither_params =
                                            DitherPaletteParams::default();
                                    }
                                    CreativeFxType::Ntsc => {
                                        self.creative_fx_state.ntsc_params = NtscParams::default();
                                    }
                                    CreativeFxType::Structure => {
                                        self.creative_fx_state.structure_params =
                                            StructureParams::default();
                                    }
                                }
                                self.creative_fx_state.selected_preset_name = "Default".to_string();
                                self.creative_fx_state.mark_dirty();
                                self.show_toast("⟲ Parameters reset to defaults");
                            }

                            if self.creative_fx_state.last_render_duration_ms > 0 {
                                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "⏱ {:.2}s",
                                        self.creative_fx_state.last_render_duration_ms as f64
                                            / 1000.0
                                    ))
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                                );
                            }
                        });
                });
        });
        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            "creative_fx",
        );
    }

    /// Split-view comparison painter for the Creative FX tab center canvas.
    #[allow(clippy::too_many_arguments)]

    pub(crate) fn render_creative_fx_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        _ctx: &egui::Context,
        orig_tex: &egui::TextureHandle,
        palette: &AppPalette,
    ) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // Render split comparison using unified SplitCompareView
        let split_view = crate::shell::SplitCompareView::new(
            orig_tex,
            self.creative_fx_state.preview_texture.as_ref(),
            &mut self.creative_fx_state.comparison_split,
            &mut self.creative_fx_state.is_dragging_split,
        );
        split_view.paint(ui, response, painter, img_rect, palette);

        // HUD badges
        let badge_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(100.0, 22.0),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0_f32, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            egui::FontId::proportional(11.0),
            hex_to_c32(&palette.colors.text_secondary),
        );

        let cur_fx = self.creative_fx_state.selected_fx;
        let fx_label = if self.creative_fx_state.is_rendering {
            "⏳ Processing…".to_string()
        } else {
            format!(
                "{} {} ({:.1}s)",
                cur_fx.icon(),
                cur_fx.name(),
                self.creative_fx_state.last_render_duration_ms as f64 / 1000.0
            )
        };
        let fx_badge_w = (fx_label.len() as f32 * 6.5).max(130.0);
        let fx_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - fx_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(fx_badge_w, 22.0),
        );
        painter.rect_filled(fx_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(fx_rect, Rounding::same(4.0), Stroke::new(1.0_f32, base_c));
        painter.text(
            fx_rect.center(),
            egui::Align2::CENTER_CENTER,
            fx_label,
            egui::FontId::proportional(10.0),
            base_c,
        );
    }
}
