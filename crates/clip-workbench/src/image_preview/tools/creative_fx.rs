use super::super::*;
use crate::shell::render_context_slider;

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

/// All state for the Creative FX tab.
pub struct CreativeFxState {
    pub selected_fx: CreativeFxType,
    pub lens_params: LensAberrationParams,
    pub trichromatic_params: TrichromaticParams,
    pub pixel_sort_params: PixelSortParams,
    pub cmyk_params: CmykHalftoneParams,
    pub dither_params: DitherPaletteParams,
    pub ntsc_params: NtscParams,

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
        }
    }
}

impl CreativeFxState {
    pub fn mark_dirty(&mut self) {
        self.params_dirty_at = Some(std::time::Instant::now());
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

        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxMsg>();
        self.creative_fx_state.render_rx = Some(rx);
        self.creative_fx_state.is_rendering = true;
        self.creative_fx_state.render_error = None;

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply_creative_fx(&input_rgba, &params)
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
                    if let Ok(out_rgba) = res {
                        if tx
                            .send(CreativeFxThumbnailMsg {
                                epoch,
                                fx_type: *fx,
                                rgba: out_rgba,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            })
            .ok();
    }

    /// Poll messages from Creative FX background render and thumbnail threads.

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

    /// Full 3-panel Creative FX viewport (Left: FX list; Center: Split Canvas; Right: Controls).

    pub(crate) fn render_creative_fx_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.theme.f_high);
        let f_med = hex_to_c32(&palette.theme.f_med);
        let f_low = hex_to_c32(&palette.theme.f_low);
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

        let avail = ui.available_size();
        let panel_layout = ImageToolMode::CreativeFx
            .panel_layout()
            .expect("Creative FX has a panel layout");
        let main_h = (avail.y - 10.0).max(100.0);
        let left_w = panel_layout.left_width;
        let right_w = panel_layout.right_width;
        let center_w = (avail.x - left_w - right_w - 16.0).max(100.0);

        // Fit to view when first opening Creative FX
        if !self.creative_fx_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.creative_fx_state.auto_fitted = true;
        }

        ui.horizontal(|ui| {
            ui.set_height(main_h);

            // ── 1. LEFT PANEL: Creative FX List with 42x28 mini preview photos ──
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, main_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::none()
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.set_width(left_w - 16.0);
                            ui.set_height(main_h - 16.0);

                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("✨ Creative FX")
                                        .strong()
                                        .small()
                                        .color(base_c),
                                );
                                ui.label(
                                    egui::RichText::new(format!("({})", CreativeFxType::all().len()))
                                        .small()
                                        .color(f_low),
                                );
                                if self.creative_fx_state.is_generating_thumbnails {
                                    ui.label(
                                        egui::RichText::new("⏳")
                                            .small()
                                            .color(f_low),
                                    ).on_hover_text("Generating preview thumbnails…");
                                }
                            });
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            egui::ScrollArea::vertical()
                                .id_source("creative_fx_list_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(left_w - 24.0);
                                    for fx in CreativeFxType::all() {
                                        let selected = self.creative_fx_state.selected_fx == *fx;
                                        let (row_bg, name_col, meta_col) = if selected {
                                            (base_c, contrast_text_c32(base_c, None, palette), contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_low, f_high, f_med)
                                        };

                                        let row_frame = egui::Frame::none()
                                            .fill(row_bg)
                                            .rounding(Rounding::same(4.0))
                                            .inner_margin(egui::Margin::symmetric(6.0, 5.0));

                                        let resp = row_frame.show(ui, |ui| {
                                            ui.set_width(left_w - 32.0);
                                            ui.horizontal(|ui| {
                                                // 42x28 mini thumbnail preview
                                                if let Some(tex) = self.creative_fx_state.fx_thumbnails.get(fx) {
                                                    ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                                } else {
                                                    let (rect, _) = ui.allocate_exact_size(Vec2::new(42.0, 28.0), egui::Sense::hover());
                                                    ui.painter().rect_filled(rect, Rounding::same(2.0), b_med);
                                                }
                                                ui.add_space(4.0);
                                                ui.vertical(|ui| {
                                                    ui.label(egui::RichText::new(fx.name()).small().strong().color(name_col));
                                                    ui.horizontal(|ui| {
                                                        ui.label(egui::RichText::new(fx.category()).monospace().color(meta_col).size(8.5));
                                                    });
                                                    ui.label(egui::RichText::new(fx.subtitle()).color(meta_col).size(8.5));
                                                });
                                            });
                                        }).response;

                                        if resp.interact(egui::Sense::click()).clicked() && !selected {
                                            self.creative_fx_state.selected_fx = *fx;
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(4.0);
                                    }
                                });
                        });
                },
            );

            ui.add_space(4.0);

            // ── 2. CENTER PANEL: Split-view Comparison Canvas ──
            ui.allocate_ui_with_layout(
                Vec2::new(center_w, main_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::none()
                        .fill(hex_to_c32(&palette.theme.background))
                        .rounding(Rounding::same(4.0))
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
                                    self.creative_fx_state.preview_texture = Some(
                                        ctx.load_texture("creative_fx_preview_tex", ci, egui::TextureOptions::LINEAR),
                                    );
                                }
                                self.creative_fx_state.dirty_preview_texture = false;
                            }

                            let (response, painter) = ui.allocate_painter(
                                Vec2::new(center_w, main_h),
                                egui::Sense::click_and_drag(),
                            );
                            let canvas_rect = response.rect;

                            painter.rect_filled(canvas_rect, Rounding::same(4.0), hex_to_c32(&palette.theme.background));

                            // If mouse is near the comparison split divider when drag starts, initiate split dragging
                            let near_split = if let Some(pos) = response.hover_pos().or_else(|| response.interact_pointer_pos()) {
                                let img_w = self.current_rgba.width() as f32 * self.zoom;
                                let img_h = self.current_rgba.height() as f32 * self.zoom;
                                let center_pt = canvas_rect.center() + self.pan;
                                let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));
                                let split_x = img_rect.min.x + self.creative_fx_state.comparison_split.clamp(0.01, 0.99) * img_rect.width();
                                (pos.x - split_x).abs() <= 18.0 && pos.y >= img_rect.min.y && pos.y <= img_rect.max.y
                            } else {
                                false
                            };
                            if near_split && response.drag_started() {
                                self.creative_fx_state.is_dragging_split = true;
                            }

                            // Pan and Zoom
                            let middle_drag = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                            if (!self.creative_fx_state.is_dragging_split && response.dragged()) || middle_drag {
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
                                    ui, &painter, &response, img_rect, canvas_rect, ctx, &orig_tex, palette,
                                );

                                painter.rect_stroke(
                                    img_rect,
                                    Rounding::ZERO,
                                    Stroke::new(1.0, hex_to_c32_alpha(&palette.theme.f_med, 40)),
                                );
                            }

                            // Rendering spinner overlay
                            if self.creative_fx_state.is_rendering {
                                let spinner_pos = canvas_rect.center();
                                let spin_bg = hex_to_c32_alpha(&palette.theme.b_low, 210);
                                let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                                painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                                painter.rect_stroke(spin_rect, Rounding::same(8.0), Stroke::new(1.0, b_med));
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
                },
            );

            ui.add_space(4.0);

            // ── 3. RIGHT PANEL: Controls for Selected Effect ──
            ui.allocate_ui_with_layout(
                Vec2::new(right_w, main_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::none()
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.set_width(right_w - 16.0);
                            ui.set_height(main_h - 16.0);

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
                                    ui.label(egui::RichText::new(cur_fx.description()).small().color(f_low));
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(4.0);

                                    // ── Presets Selector ──
                                    ui.label(egui::RichText::new("🏷 Presets").strong().small().color(base_c));
                                    ui.add_space(3.0);
                                    let presets = presets_for_fx(cur_fx);
                                    for preset in &presets {
                                        let sel = self.creative_fx_state.selected_preset_name == preset.name;
                                        let (s_bg, s_fg) = if sel {
                                            (base_c, contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_med, f_med)
                                        };
                                        let btn = egui::Button::new(
                                            egui::RichText::new(preset.name).small().strong().color(s_fg),
                                        )
                                        .fill(s_bg)
                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                        .rounding(Rounding::same(4.0))
                                        .min_size(Vec2::new(full_w, 20.0));

                                        if ui.add(btn).on_hover_text(preset.description).clicked() {
                                            self.creative_fx_state.selected_preset_name = preset.name.to_string();
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
                                            }
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);
                                    }

                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(4.0);

                                    // ── Parameter Controls ──
                                    ui.label(egui::RichText::new("🎛 Controls").strong().small().color(base_c));
                                    ui.add_space(4.0);

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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(4.0);

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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(4.0);

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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }

                                        CreativeFxType::Trichromatic => {
                                            // Mode: RGB vs OKLab
                                            ui.label(egui::RichText::new("Color Separation Model").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                let modes = [
                                                    (TrichromaticMode::Rgb, "RGB"),
                                                    (TrichromaticMode::OkLab, "OKLab"),
                                                ];
                                                for (mode, label) in modes {
                                                    let sel = self.creative_fx_state.trichromatic_params.mode == mode;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(label).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.trichromatic_params.mode = mode;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(6.0);

                                            let names = self.creative_fx_state.trichromatic_params.mode.channel_names();
                                            for i in 0..3 {
                                                ui.label(egui::RichText::new(format!("Channel {}: {}", i + 1, names[i])).small().strong().color(base_c));
                                                ui.add_space(2.0);

                                                // Offset X
                                                if render_context_slider(
                                                    ui,
                                                    &format!("↔ X Offset (Ch {})", i + 1),
                                                    &mut self.creative_fx_state.trichromatic_params.channels[i].offset_x,
                                                    -50.0..=50.0,
                                                    0.0,
                                                    "px",
                                                    slider_w,
                                                    palette,
                                                ) {
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                                ui.add_space(2.0);

                                                // Offset Y
                                                if render_context_slider(
                                                    ui,
                                                    &format!("↕ Y Offset (Ch {})", i + 1),
                                                    &mut self.creative_fx_state.trichromatic_params.channels[i].offset_y,
                                                    -50.0..=50.0,
                                                    0.0,
                                                    "px",
                                                    slider_w,
                                                    palette,
                                                ) {
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                                ui.add_space(2.0);

                                                // Zoom
                                                if render_context_slider(
                                                    ui,
                                                    &format!("🔍 Zoom (Ch {})", i + 1),
                                                    &mut self.creative_fx_state.trichromatic_params.channels[i].zoom,
                                                    0.80..=1.20,
                                                    1.00,
                                                    "×",
                                                    slider_w,
                                                    palette,
                                                ) {
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                                ui.add_space(4.0);
                                            }

                                            // Overlap crop window calculation
                                            let (_, _, cw, ch) = trichromatic_intersection_box(
                                                self.current_rgba.width(),
                                                self.current_rgba.height(),
                                                &self.creative_fx_state.trichromatic_params.channels,
                                            );
                                            ui.add_space(2.0);
                                            ui.label(
                                                egui::RichText::new(format!("✂ Crop Window: {} × {} px", cw, ch))
                                                    .monospace()
                                                    .small()
                                                    .color(f_med),
                                            );
                                            ui.label(
                                                egui::RichText::new("(auto-cropped to 3-channel overlap)")
                                                    .size(9.0)
                                                    .color(f_low),
                                            );
                                        }

                                        CreativeFxType::PixelSort => {
                                            // Direction
                                            ui.label(egui::RichText::new("Scanline Direction").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                let dirs = [
                                                    (PixelSortDirection::Vertical, "Vertical"),
                                                    (PixelSortDirection::Horizontal, "Horizontal"),
                                                    (PixelSortDirection::Diagonal, "Diagonal"),
                                                ];
                                                for (d, label) in dirs {
                                                    let sel = self.creative_fx_state.pixel_sort_params.direction == d;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(label).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.pixel_sort_params.direction = d;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(4.0);

                                            // Criteria
                                            ui.label(egui::RichText::new("Sort Metric").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                let crit = [
                                                    (PixelSortCriteria::Brightness, "Brightness"),
                                                    (PixelSortCriteria::Hue, "Hue"),
                                                    (PixelSortCriteria::Saturation, "Saturation"),
                                                ];
                                                for (c, label) in crit {
                                                    let sel = self.creative_fx_state.pixel_sort_params.criteria == c;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(label).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.pixel_sort_params.criteria = c;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(4.0);

                                            // Order
                                            ui.label(egui::RichText::new("Sort Order").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                let orders = [
                                                    (PixelSortOrder::Ascending, "Ascending"),
                                                    (PixelSortOrder::Descending, "Descending"),
                                                ];
                                                for (o, label) in orders {
                                                    let sel = self.creative_fx_state.pixel_sort_params.order == o;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(label).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.pixel_sort_params.order = o;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(4.0);

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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(2.0);

                                            // Min Span
                                            let mut span_f = self.creative_fx_state.pixel_sort_params.min_span as f32;
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
                                                self.creative_fx_state.pixel_sort_params.min_span = span_f.round() as usize;
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(2.0);

                                            // Invert mask
                                            let mut invert = self.creative_fx_state.pixel_sort_params.invert_mask;
                                            if ui.checkbox(&mut invert, egui::RichText::new("Invert Selection Mask").small().color(f_med)).changed() {
                                                self.creative_fx_state.pixel_sort_params.invert_mask = invert;
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(4.0);

                                            // Paper Tint
                                            ui.label(egui::RichText::new("Paper Substrate Tint").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for tint in PaperTint::all() {
                                                    let sel = self.creative_fx_state.cmyk_params.paper_tint == *tint;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(
                                                        egui::RichText::new(tint.label()).small().strong().color(b_fg)
                                                    )
                                                    .fill(b_bg)
                                                    .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                    .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.cmyk_params.paper_tint = *tint;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(6.0);
                                            ui.separator();
                                            ui.add_space(4.0);

                                            // Plate Misregistration Offsets
                                            ui.label(egui::RichText::new("🗞 Plate Misregistration Offsets").small().strong().color(base_c));
                                            ui.add_space(3.0);

                                            // Cyan plate
                                            ui.label(egui::RichText::new("Cyan Plate Offset").size(10.0).color(f_med));
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(3.0);

                                            // Magenta plate
                                            ui.label(egui::RichText::new("Magenta Plate Offset").size(10.0).color(f_med));
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(3.0);

                                            // Yellow plate
                                            ui.label(egui::RichText::new("Yellow Plate Offset").size(10.0).color(f_med));
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(3.0);

                                            // Black (Key) plate
                                            ui.label(egui::RichText::new("Black (Key) Plate Offset").size(10.0).color(f_med));
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }

                                        CreativeFxType::DitherPalette => {
                                            // Palette selection
                                            ui.label(egui::RichText::new("Hardware / Aesthetic Palette").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for pal in RetroPalette::all() {
                                                    let sel = self.creative_fx_state.dither_params.palette == *pal;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(
                                                        egui::RichText::new(pal.label()).small().strong().color(b_fg)
                                                    )
                                                    .fill(b_bg)
                                                    .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                    .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.dither_params.palette = *pal;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(6.0);

                                            // Dither Method selection
                                            ui.label(egui::RichText::new("Dithering Method").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for method in DitherMethod::all() {
                                                    let sel = self.creative_fx_state.dither_params.method == *method;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(
                                                        egui::RichText::new(method.label()).small().strong().color(b_fg)
                                                    )
                                                    .fill(b_bg)
                                                    .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                    .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.dither_params.method = *method;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(6.0);

                                            // Pixel Scale selection (1x..4x)
                                            ui.label(egui::RichText::new("Pixel Grid Scale").small().color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for scale in [1u32, 2, 3, 4] {
                                                    let sel = self.creative_fx_state.dither_params.pixel_scale == scale;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(
                                                        egui::RichText::new(format!("{}×", scale)).small().strong().color(b_fg)
                                                    )
                                                    .fill(b_bg)
                                                    .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                    .rounding(Rounding::same(4.0))
                                                    .min_size(Vec2::new(32.0, 20.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.dither_params.pixel_scale = scale;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(4.0);

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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }

                                        CreativeFxType::Ntsc => {
                                            // ── 1. Broadcast RF & Filtering ──
                                            ui.label(egui::RichText::new("📡 Signal & Filtering").small().strong().color(base_c));
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(2.0);

                                            if render_context_slider(
                                                ui,
                                                "Composite Sharpness",
                                                &mut self.creative_fx_state.ntsc_params.composite_sharpening,
                                                0.0..=3.0,
                                                1.0,
                                                "×",
                                                slider_w,
                                                palette,
                                            ) {
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(3.0);

                                            // Demodulation Filter
                                            ui.label(egui::RichText::new("Demodulation Filter").size(10.0).color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for demo in NtscDemodulation::all() {
                                                    let sel = self.creative_fx_state.ntsc_params.demodulation == *demo;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(demo.label()).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.ntsc_params.demodulation = *demo;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(3.0);

                                            // Scanline Phase Shift
                                            ui.label(egui::RichText::new("Subcarrier Phase Shift (Dot Crawl)").size(10.0).color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for ps in NtscPhaseShift::all() {
                                                    let sel = self.creative_fx_state.ntsc_params.phase_shift == *ps;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(ps.label()).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0))
                                                        .min_size(Vec2::new(38.0, 20.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.ntsc_params.phase_shift = *ps;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(3.0);

                                            // Chroma Lowpass
                                            ui.label(egui::RichText::new("Chroma Lowpass Filter").size(10.0).color(f_med));
                                            ui.horizontal_wrapped(|ui| {
                                                for cl in NtscChromaLowpass::all() {
                                                    let sel = self.creative_fx_state.ntsc_params.chroma_lowpass == *cl;
                                                    let (b_bg, b_fg) = if sel {
                                                        (base_c, contrast_text_c32(base_c, None, palette))
                                                    } else {
                                                        (b_med, f_med)
                                                    };
                                                    let btn = egui::Button::new(egui::RichText::new(cl.label()).small().strong().color(b_fg))
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                    if ui.add(btn).clicked() && !sel {
                                                        self.creative_fx_state.ntsc_params.chroma_lowpass = *cl;
                                                        self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                        self.creative_fx_state.mark_dirty();
                                                    }
                                                }
                                            });
                                            ui.add_space(3.0);

                                            let mut scanlines = self.creative_fx_state.ntsc_params.scanlines;
                                            if ui.checkbox(&mut scanlines, egui::RichText::new("📺 CRT Interlaced Scanlines").small().color(f_med)).changed() {
                                                self.creative_fx_state.ntsc_params.scanlines = scanlines;
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(4.0);

                                            // ── 2. Chrominance & YIQ ──
                                            ui.separator();
                                            ui.add_space(3.0);
                                            ui.label(egui::RichText::new("🎨 Chrominance & Color").small().strong().color(base_c));
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(2.0);

                                            let mut delay_y = self.creative_fx_state.ntsc_params.chroma_delay_y as f32;
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
                                                self.creative_fx_state.ntsc_params.chroma_delay_y = delay_y.round() as i32;
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(2.0);

                                            let mut vert_blend = self.creative_fx_state.ntsc_params.chroma_vert_blend;
                                            if ui.checkbox(&mut vert_blend, egui::RichText::new("Chroma Vertical Blending").small().color(f_med)).changed() {
                                                self.creative_fx_state.ntsc_params.chroma_vert_blend = vert_blend;
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(4.0);

                                            // ── 3. Noise & Static ──
                                            ui.separator();
                                            ui.add_space(3.0);
                                            ui.label(egui::RichText::new("❄ Noise & Static").small().strong().color(base_c));
                                            ui.add_space(2.0);

                                            ui.vertical(|ui| {
                                                ui.label(egui::RichText::new("RF Static / Snow").small().color(f_med));
                                                ui.spacing_mut().slider_width = slider_w;
                                                let mut snow = self.creative_fx_state.ntsc_params.snow;
                                                if ui.add(egui::Slider::new(&mut snow, 0.0..=0.04).max_decimals(4)).changed() {
                                                    self.creative_fx_state.ntsc_params.snow = snow;
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                            ui.add_space(4.0);

                                            // ── 4. VHS Magnetic Tape Simulation ──
                                            ui.separator();
                                            ui.add_space(3.0);
                                            ui.label(egui::RichText::new("📼 VHS Tape Emulation").small().strong().color(base_c));
                                            ui.add_space(2.0);

                                            let mut vhs_enabled = self.creative_fx_state.ntsc_params.vhs_enabled;
                                            if ui.checkbox(&mut vhs_enabled, egui::RichText::new("Enable VHS Tape Pass").small().color(f_med)).changed() {
                                                self.creative_fx_state.ntsc_params.vhs_enabled = vhs_enabled;
                                                self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }

                                            if self.creative_fx_state.ntsc_params.vhs_enabled {
                                                ui.add_space(2.0);
                                                ui.label(egui::RichText::new("Tape Speed").size(10.0).color(f_med));
                                                ui.horizontal_wrapped(|ui| {
                                                    for speed in [VhsTapeSpeed::SP, VhsTapeSpeed::LP, VhsTapeSpeed::EP] {
                                                        let sel = self.creative_fx_state.ntsc_params.tape_speed == speed;
                                                        let (b_bg, b_fg) = if sel {
                                                            (base_c, contrast_text_c32(base_c, None, palette))
                                                        } else {
                                                            (b_med, f_med)
                                                        };
                                                        let btn = egui::Button::new(
                                                            egui::RichText::new(speed.label()).small().strong().color(b_fg)
                                                        )
                                                        .fill(b_bg)
                                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                                        .rounding(Rounding::same(4.0));
                                                        if ui.add(btn).clicked() && !sel {
                                                            self.creative_fx_state.ntsc_params.tape_speed = speed;
                                                            self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
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
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                                ui.add_space(2.0);

                                                let mut head_sw = self.creative_fx_state.ntsc_params.head_switching;
                                                if ui.checkbox(&mut head_sw, egui::RichText::new("Head-Switching Jitter (Bottom)").small().color(f_med)).changed() {
                                                    self.creative_fx_state.ntsc_params.head_switching = head_sw;
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                            }

                                            ui.add_space(3.0);
                                            ui.separator();
                                            ui.add_space(2.0);
                                            ui.vertical(|ui| {
                                                ui.label(egui::RichText::new("🎲 Noise Random Seed").size(10.0).color(f_med));
                                                ui.spacing_mut().slider_width = slider_w;
                                                let mut seed = self.creative_fx_state.ntsc_params.random_seed;
                                                if ui.add(egui::Slider::new(&mut seed, 0..=1000)).changed() {
                                                    self.creative_fx_state.ntsc_params.random_seed = seed;
                                                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                            });
                                        }
                                    }

                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(6.0);

                                    // ── Action Buttons ──
                                    let apply_fg = contrast_text_c32(diff_add, Some(f_high), palette);
                                    let apply_btn = egui::Button::new(
                                        egui::RichText::new("✓ Apply to Image").small().strong().color(apply_fg),
                                    )
                                    .fill(diff_add)
                                    .stroke(Stroke::new(1.0, diff_add))
                                    .rounding(Rounding::same(4.0))
                                    .min_size(Vec2::new(full_w, 24.0));

                                    let apply_enabled = self.creative_fx_state.preview_rgba.is_some();
                                    if ui.add_enabled(apply_enabled, apply_btn)
                                        .on_hover_text("Commit Creative FX result to image (pushes to undo stack)")
                                        .clicked()
                                    {
                                        self.apply_creative_fx();
                                    }

                                    ui.add_space(4.0);

                                    let reset_fg = contrast_text_c32(b_med, Some(f_med), palette);
                                    let reset_btn = egui::Button::new(egui::RichText::new("⟲ Reset Defaults").small().strong().color(reset_fg))
                                        .fill(b_med)
                                        .stroke(Stroke::new(1.0, b_low))
                                        .rounding(Rounding::same(4.0))
                                        .min_size(Vec2::new(full_w, 22.0));

                                    if ui.add(reset_btn).on_hover_text("Reset parameters to default").clicked() {
                                        match cur_fx {
                                            CreativeFxType::LensAberration => {
                                                self.creative_fx_state.lens_params = LensAberrationParams::default();
                                            }
                                            CreativeFxType::Trichromatic => {
                                                self.creative_fx_state.trichromatic_params = TrichromaticParams::default();
                                            }
                                            CreativeFxType::PixelSort => {
                                                self.creative_fx_state.pixel_sort_params = PixelSortParams::default();
                                            }
                                            CreativeFxType::CmykHalftone => {
                                                self.creative_fx_state.cmyk_params = CmykHalftoneParams::default();
                                            }
                                            CreativeFxType::DitherPalette => {
                                                self.creative_fx_state.dither_params = DitherPaletteParams::default();
                                            }
                                            CreativeFxType::Ntsc => {
                                                self.creative_fx_state.ntsc_params = NtscParams::default();
                                            }
                                        }
                                        self.creative_fx_state.selected_preset_name = "Default".to_string();
                                        self.creative_fx_state.mark_dirty();
                                        self.show_toast("⟲ Parameters reset to defaults");
                                    }

                                    if self.creative_fx_state.last_render_duration_ms > 0 {
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(
                                            format!("⏱ {:.2}s", self.creative_fx_state.last_render_duration_ms as f64 / 1000.0)
                                        ).small().color(f_low));
                                    }
                                });
                        });
                },
            );
        });
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
        let b_med = hex_to_c32(&palette.theme.b_med);
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
        let badge_bg = hex_to_c32_alpha(&palette.theme.b_low, 210);

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(100.0, 22.0),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            egui::FontId::proportional(11.0),
            hex_to_c32(&palette.theme.f_med),
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
        painter.rect_stroke(fx_rect, Rounding::same(4.0), Stroke::new(1.0, base_c));
        painter.text(
            fx_rect.center(),
            egui::Align2::CENTER_CENTER,
            fx_label,
            egui::FontId::proportional(10.0),
            base_c,
        );
    }
}
