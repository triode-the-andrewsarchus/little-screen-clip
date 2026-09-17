//! Text and image differential inspection workbench view.
//!
//! Provides side-by-side, unified, and inline word diffs for text, and 3-up/overlay/mask
//! pixel diffs for bitmap images.

use crate::ansi::ansi_to_layout_job;
use crate::send_ipc_request;
use crate::shell::{EscapeHatch, StudioShell};
use eframe::egui::{self, Color32, Rounding, Stroke, Vec2};
use little_clip_core::HandSide;
use little_clip_core::ipc::IpcRequest;
use little_clip_ops::diff::{DiffDisplayMode, DiffResult, DiffTarget, compute_diff};
use little_clip_ops::image_diff::{ImageDiffOptions, ImageDiffResult};

/// Layout visualization mode for comparing bitmap images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageDiffViewMode {
    /// 3-Up view: `[Base] [Comparison] [Diff Mask]`.
    ThreeUp, // [ base ] [ comparison ] [ diff ] (matching ODiff demo table)
    /// Dimmed base image overlayed with glowing diff pixels.
    Overlay, // [ dimmed base + glowing diff pixels ]
    /// High-contrast black mask highlighting pixel differences.
    MaskOnly, // [ black mask + glowing diff pixels ]
    /// Split view: `[Base] [Diff Mask]`.
    Split, // [ base ] [ diff ]
}

impl ImageDiffViewMode {
    /// Human-readable display label with icon.
    pub fn label(&self) -> &'static str {
        match self {
            ImageDiffViewMode::ThreeUp => "⬌ 3-Up (Base | Compare | Diff)",
            ImageDiffViewMode::Overlay => "⬡ Overlay",
            ImageDiffViewMode::MaskOnly => "⬛ Diff Mask (Black)",
            ImageDiffViewMode::Split => "⬍ Split (Base | Diff)",
        }
    }
}

/// Interactive state for the text and visual image diff viewer.
pub struct DiffViewerState {
    pub lhs: DiffTarget,
    pub rhs: DiffTarget,
    pub mode: DiffDisplayMode,
    // Text diff state
    pub result: DiffResult,
    pub layout_job: egui::text::LayoutJob,
    // Image diff state
    pub image_result: Option<ImageDiffResult>,
    pub image_options: ImageDiffOptions,
    pub image_mode: ImageDiffViewMode,
    pub base_texture: Option<egui::TextureHandle>,
    pub comp_texture: Option<egui::TextureHandle>,
    pub mask_texture: Option<egui::TextureHandle>,
    pub overlay_texture: Option<egui::TextureHandle>,
    pub textures_loaded: bool,
    pub decode_error: Option<String>,
    pub status_toast: Option<(String, f64)>, // (text, timestamp)
    pub image_receiver: Option<std::sync::mpsc::Receiver<Result<ImageDiffResult, String>>>,
    pub is_computing: bool,
    pub compute_start_time: std::time::Instant,
}

fn spawn_image_diff_worker(
    lhs: &DiffTarget,
    rhs: &DiffTarget,
    options: ImageDiffOptions,
) -> (
    Option<std::sync::mpsc::Receiver<Result<ImageDiffResult, String>>>,
    Option<String>,
) {
    if let (Some(b1), Some(b2)) = (lhs.image_bytes(), rhs.image_bytes()) {
        if b1.is_empty() || b2.is_empty() {
            let mut missing = Vec::new();
            if b1.is_empty() {
                missing.push(format!("left target '{}'", lhs.label()));
            }
            if b2.is_empty() {
                missing.push(format!("right target '{}'", rhs.label()));
            }
            let err = format!(
                "Image data could not be loaded for {}. Please check that the file exists.",
                missing.join(" and ")
            );
            return (None, Some(err));
        }

        let b1_vec = b1.to_vec();
        let b2_vec = b2.to_vec();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let res = little_clip_ops::image_diff::compare_image_bytes(&b1_vec, &b2_vec, &options)
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });

        (Some(rx), None)
    } else {
        (
            None,
            Some("Failed to obtain image bytes from comparison targets".to_string()),
        )
    }
}

impl DiffViewerState {
    /// Initializes text or image diff calculation between two comparison targets.
    pub fn new(lhs: DiffTarget, rhs: DiffTarget, mode: DiffDisplayMode) -> Self {
        let is_img = lhs.is_image() && rhs.is_image();
        let image_options = ImageDiffOptions::default();

        let (image_receiver, is_computing, decode_error) = if is_img {
            let (rx, err) = spawn_image_diff_worker(&lhs, &rhs, image_options.clone());
            let computing = rx.is_some();
            (rx, computing, err)
        } else {
            (None, false, None)
        };

        let (result, layout_job) = if is_img {
            (
                compute_diff(&lhs, &rhs, mode, 120),
                ansi_to_layout_job("", 12.0),
            )
        } else {
            let width = if mode == DiffDisplayMode::SideBySide {
                150
            } else {
                120
            };
            let r = compute_diff(&lhs, &rhs, mode, width);
            let job = ansi_to_layout_job(&r.raw_ansi, 12.0);
            (r, job)
        };

        Self {
            lhs,
            rhs,
            mode,
            result,
            layout_job,
            image_result: None,
            image_options,
            image_mode: ImageDiffViewMode::ThreeUp,
            base_texture: None,
            comp_texture: None,
            mask_texture: None,
            overlay_texture: None,
            textures_loaded: false,
            decode_error,
            status_toast: None,
            image_receiver,
            is_computing,
            compute_start_time: std::time::Instant::now(),
        }
    }

    #[allow(dead_code)]
    pub fn wait_for_completion(&mut self) {
        if let Some(rx) = self.image_receiver.take() {
            if let Ok(res) = rx.recv() {
                match res {
                    Ok(r) => {
                        self.image_result = Some(r);
                        self.decode_error = None;
                    }
                    Err(e) => {
                        self.decode_error = Some(e);
                        self.image_result = None;
                    }
                }
            }
            self.is_computing = false;
        }
    }

    pub fn is_image(&self) -> bool {
        self.lhs.is_image() && self.rhs.is_image()
    }

    pub fn recompute(&mut self) {
        if self.is_image() {
            let (rx, err) =
                spawn_image_diff_worker(&self.lhs, &self.rhs, self.image_options.clone());
            self.is_computing = rx.is_some();
            self.image_receiver = rx;
            self.compute_start_time = std::time::Instant::now();
            self.decode_error = err;
            self.textures_loaded = false;
        } else {
            let width = if self.mode == DiffDisplayMode::SideBySide {
                150
            } else {
                120
            };
            self.result = compute_diff(&self.lhs, &self.rhs, self.mode, width);
            self.layout_job = ansi_to_layout_job(&self.result.raw_ansi, 12.0);
        }
    }

    pub fn toggle_mode(&mut self) {
        if self.is_image() {
            self.image_mode = match self.image_mode {
                ImageDiffViewMode::ThreeUp => ImageDiffViewMode::Overlay,
                ImageDiffViewMode::Overlay => ImageDiffViewMode::MaskOnly,
                ImageDiffViewMode::MaskOnly => ImageDiffViewMode::Split,
                ImageDiffViewMode::Split => ImageDiffViewMode::ThreeUp,
            };
        } else {
            self.mode = self.mode.toggled();
            self.recompute();
        }
    }

    pub fn swap_operands(&mut self) {
        std::mem::swap(&mut self.lhs, &mut self.rhs);
        self.recompute();
    }

    fn ensure_image_textures(&mut self, ctx: &egui::Context) {
        if self.textures_loaded || self.is_computing {
            return;
        }

        if let Some(res) = &self.image_result {
            let max_tex_dim = 2048;
            let (w, h, base_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.base_rgba,
                max_tex_dim,
            );
            let (_, _, comp_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.comp_rgba,
                max_tex_dim,
            );
            let (_, _, mask_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.mask_rgba,
                max_tex_dim,
            );
            let (_, _, overlay_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.overlay_rgba,
                max_tex_dim,
            );

            let size = [w as usize, h as usize];
            if size[0] > 0 && size[1] > 0 {
                let base_img = egui::ColorImage::from_rgba_unmultiplied(size, &base_rgba);
                self.base_texture =
                    Some(ctx.load_texture("diff-base", base_img, egui::TextureOptions::LINEAR));

                let comp_img = egui::ColorImage::from_rgba_unmultiplied(size, &comp_rgba);
                self.comp_texture =
                    Some(ctx.load_texture("diff-comp", comp_img, egui::TextureOptions::LINEAR));

                let mask_img = egui::ColorImage::from_rgba_unmultiplied(size, &mask_rgba);
                self.mask_texture =
                    Some(ctx.load_texture("diff-mask", mask_img, egui::TextureOptions::NEAREST));

                let overlay_img = egui::ColorImage::from_rgba_unmultiplied(size, &overlay_rgba);
                self.overlay_texture = Some(ctx.load_texture(
                    "diff-overlay",
                    overlay_img,
                    egui::TextureOptions::LINEAR,
                ));
            }
            self.textures_loaded = true;
        }
    }

    /// Triggers launching Meld or KDiff3 for visual comparison.
    pub fn trigger_escape_hatch(&mut self, now: f64) {
        let left_path = match &self.lhs {
            DiffTarget::File { path, .. } => path.clone(),
            DiffTarget::ClipboardText {
                content, ext_hint, ..
            } => {
                let ext = ext_hint.as_deref().unwrap_or("txt");
                let p = std::env::temp_dir().join(format!(
                    "clip_diff_lhs_{}.{}",
                    uuid::Uuid::new_v4().simple(),
                    ext
                ));
                let _ = std::fs::write(&p, content);
                p
            }
            DiffTarget::Image { path: Some(p), .. } => p.clone(),
            DiffTarget::Image { bytes, mime, .. } => {
                let ext = if mime.contains("png") { "png" } else { "jpg" };
                let p = std::env::temp_dir().join(format!(
                    "clip_diff_lhs_{}.{}",
                    uuid::Uuid::new_v4().simple(),
                    ext
                ));
                let _ = std::fs::write(&p, bytes);
                p
            }
        };

        let right_path = match &self.rhs {
            DiffTarget::File { path, .. } => path.clone(),
            DiffTarget::ClipboardText {
                content, ext_hint, ..
            } => {
                let ext = ext_hint.as_deref().unwrap_or("txt");
                let p = std::env::temp_dir().join(format!(
                    "clip_diff_rhs_{}.{}",
                    uuid::Uuid::new_v4().simple(),
                    ext
                ));
                let _ = std::fs::write(&p, content);
                p
            }
            DiffTarget::Image { path: Some(p), .. } => p.clone(),
            DiffTarget::Image { bytes, mime, .. } => {
                let ext = if mime.contains("png") { "png" } else { "jpg" };
                let p = std::env::temp_dir().join(format!(
                    "clip_diff_rhs_{}.{}",
                    uuid::Uuid::new_v4().simple(),
                    ext
                ));
                let _ = std::fs::write(&p, bytes);
                p
            }
        };

        if let Some(meld) = little_clip_core::command_available("meld") {
            if std::process::Command::new(meld)
                .arg(&left_path)
                .arg(&right_path)
                .spawn()
                .is_ok()
            {
                self.status_toast = Some(("🚀 Launched Meld comparison".to_string(), now));
                return;
            }
        }
        if let Some(kdiff3) = little_clip_core::command_available("kdiff3") {
            if std::process::Command::new(kdiff3)
                .arg(&left_path)
                .arg(&right_path)
                .spawn()
                .is_ok()
            {
                self.status_toast = Some(("🚀 Launched KDiff3 comparison".to_string(), now));
                return;
            }
        }
        self.status_toast = Some(("⚠️ Neither Meld nor KDiff3 is installed".to_string(), now));
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::theme::AppPalette,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);

        // Handle common keyboard shortcuts
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close_requested = true;
        }
        if ctx.input(|i| {
            i.key_pressed(egui::Key::Tab)
                || (i.key_pressed(egui::Key::C)
                    && !i.modifiers.command
                    && !i.modifiers.ctrl
                    && !i.modifiers.mac_cmd)
        }) {
            self.toggle_mode();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::S)) {
            self.swap_operands();
        }

        let is_img = self.is_image();

        if is_img {
            if self.is_computing {
                ctx.request_repaint();
                if let Some(ref rx) = self.image_receiver {
                    match rx.try_recv() {
                        Ok(Ok(res)) => {
                            self.image_result = Some(res);
                            self.decode_error = None;
                            self.is_computing = false;
                            self.textures_loaded = false;
                            self.image_receiver = None;
                        }
                        Ok(Err(err)) => {
                            self.decode_error = Some(err);
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            self.decode_error = Some("Diff worker thread disconnected".to_string());
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                        }
                    }
                }
            }

            self.ensure_image_textures(ctx);
        }

        // Copy shortcut
        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || i.raw.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.key_pressed(egui::Key::C)
                    && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
        });

        if copy_requested {
            if is_img {
                self.copy_image_diff_to_clipboard(now);
            } else {
                ctx.output_mut(|o| o.copied_text = strip_ansi_codes(&self.result.raw_ansi));
                self.status_toast = Some(("Diff text copied to clipboard!".to_string(), now));
            }
        }

        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let success_color = crate::hex_to_c32(&palette.diff_add);
        let del_color = crate::hex_to_c32(&palette.diff_del);

        let hatch = EscapeHatch::new(
            "🔍 Open in Meld",
            "meld",
            "Open comparison in external Meld tool",
        );

        let mut shell = if is_img {
            let mut s = StudioShell::new("Picture Diff Comparison", "⑂", palette, close_requested)
                .with_escape_hatch(hatch)
                .with_footer_hints("Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Mask");

            if self.is_computing {
                let elapsed = self.compute_start_time.elapsed().as_secs_f32();
                s = s.with_badge(format!("⏳ Comparing... ({:.1}s)", elapsed), Some(accent));
            } else if let Some(res) = &self.image_result {
                if res.is_identical {
                    s = s.with_badge("✔ 0 diff pixels • Identical", Some(success_color));
                } else {
                    s = s.with_badge(
                        format!(
                            "⚡ {} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        ),
                        Some(del_color),
                    );
                }
                s = s.with_badge(format!("⛶ {} × {}", res.width, res.height), None);
                s = s.with_footer_readout(format!(
                    "Tolerance: {:.2} • {} diff pixels",
                    self.image_options.threshold, res.diff_count
                ));
            }
            s
        } else {
            let lines_count = self.result.raw_ansi.lines().count();
            let mut s = StudioShell::new("Diff Comparison", "⑂", palette, close_requested)
                .with_badge(format!("⚡ {}", self.result.engine_name), Some(accent))
                .with_escape_hatch(hatch)
                .with_footer_hints("Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Diff");

            if self.result.is_identical {
                s = s.with_badge("✔ Identical", Some(success_color));
            } else {
                s = s.with_badge(format!("⚡ {} lines in diff", lines_count), None);
            }
            s = s.with_footer_readout(format!(
                "LHS: {} • RHS: {}",
                self.lhs.label(),
                self.rhs.label()
            ));
            s
        };

        egui::Frame::none()
            .fill(crate::hex_to_c32(&palette.theme.background))
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = crate::secondary_button("⇄ Swap [S]", b_med, palette);
                    if ui
                        .add(swap_btn)
                        .on_hover_text("Swap operands (S)")
                        .clicked()
                    {
                        self.swap_operands();
                    }

                    let copy_label = if is_img {
                        "📋 Copy Diff Mask"
                    } else {
                        "📋 Copy Diff"
                    };
                    let copy_btn = crate::secondary_button(copy_label, b_med, palette);
                    if ui.add(copy_btn).clicked() {
                        if is_img {
                            self.copy_image_diff_to_clipboard(now);
                        } else {
                            ui.output_mut(|o| {
                                o.copied_text = strip_ansi_codes(&self.result.raw_ansi)
                            });
                            self.status_toast =
                                Some(("Diff text copied to clipboard!".to_string(), now));
                        }
                    }
                });

                if clicked_hatch.is_some() {
                    self.trigger_escape_hatch(now);
                }

                if shell.is_closed() {
                    return;
                }

                ui.add_space(4.0);

                // Zone 2: Mode Toolbar
                if is_img {
                    self.render_image_toolbar(ui, palette);
                } else {
                    self.render_text_toolbar(ui, palette);
                }

                ui.add_space(6.0);

                // Zone 3: Main comparison viewport
                let footer_h = 28.0;
                let avail = ui.available_size();
                let viewport_h = (avail.y - footer_h - 6.0).max(100.0);

                ui.allocate_ui_with_layout(
                    Vec2::new(avail.x, viewport_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if is_img {
                            self.render_image_viewport(ui, palette);
                        } else {
                            self.render_text_viewport(ui, palette);
                        }
                    },
                );

                ui.add_space(4.0);

                // Zone 5: Footer
                shell.render_footer(ui, &self.status_toast, now);
            });
    }

    fn render_image_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .rounding(Rounding::same(4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for mode in &[
                        ImageDiffViewMode::ThreeUp,
                        ImageDiffViewMode::Overlay,
                        ImageDiffViewMode::MaskOnly,
                        ImageDiffViewMode::Split,
                    ] {
                        let is_active = self.image_mode == *mode;
                        let btn = crate::mode_tab_button(mode.label(), is_active, palette);
                        if ui.add(btn).clicked() {
                            self.image_mode = *mode;
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.label(egui::RichText::new("Tolerance:").small().color(f_low));
                    let slider = egui::Slider::new(&mut self.image_options.threshold, 0.00..=0.50)
                        .step_by(0.01)
                        .show_value(true);
                    if ui.add(slider).on_hover_text("Allowable perceptual difference threshold (0.00 = strict, 0.50 = loose)").changed() {
                        self.recompute();
                    }

                    for (val, label) in &[(0.05f32, "0.05"), (0.10f32, "0.10"), (0.20f32, "0.20")] {
                        let is_cur = (self.image_options.threshold - val).abs() < 0.005;
                        let t_btn = crate::mode_tab_button(*label, is_cur, palette);
                        if ui.add(t_btn).clicked() && !is_cur {
                            self.image_options.threshold = *val;
                            self.recompute();
                        }
                    }
                });
            });
    }

    fn render_image_viewport(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let f_med = crate::hex_to_c32(&palette.theme.f_med);
        let del_color = crate::hex_to_c32(&palette.diff_del);

        if self.is_computing {
            self.render_computing_ui(ui, palette);
        } else if let Some(res) = &self.image_result {
            let aspect = (res.width as f32) / (res.height as f32).max(1.0);

            match self.image_mode {
                ImageDiffViewMode::ThreeUp => {
                    let avail_w = ui.available_width() - 32.0;
                    let col_w = ((avail_w - 24.0) / 3.0).max(180.0);
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = col_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.horizontal(|ui| {
                        ui.add_space(12.0);

                        // Column 1: Base
                        self.render_image_column(
                            ui,
                            palette,
                            "base",
                            self.lhs.label(),
                            f_high,
                            self.base_texture.as_ref(),
                            img_w,
                            img_h,
                        );

                        ui.add_space(12.0);

                        // Column 2: Comparison
                        self.render_image_column(
                            ui,
                            palette,
                            "comparison",
                            self.rhs.label(),
                            f_high,
                            self.comp_texture.as_ref(),
                            img_w,
                            img_h,
                        );

                        ui.add_space(12.0);

                        // Column 3: Diff
                        let diff_sub = format!(
                            "{} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        );
                        self.render_image_column(
                            ui,
                            palette,
                            "diff",
                            &diff_sub,
                            del_color,
                            self.mask_texture.as_ref(),
                            img_w,
                            img_h,
                        );
                    });
                }
                ImageDiffViewMode::Overlay => {
                    let avail_w = ui.available_width() - 32.0;
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = avail_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Diff Overlay (Dimmed Base + Glowing Difference)")
                                .strong()
                                .color(f_high),
                        );
                        ui.add_space(4.0);

                        if let Some(tex) = &self.overlay_texture {
                            egui::Frame::none()
                                .fill(b_low)
                                .rounding(Rounding::same(6.0))
                                .stroke(Stroke::new(1.0, b_med))
                                .inner_margin(egui::Margin::same(4.0))
                                .show(ui, |ui| {
                                    ui.image(egui::load::SizedTexture::new(
                                        tex.id(),
                                        Vec2::new(img_w, img_h),
                                    ));
                                });
                        }
                    });
                }
                ImageDiffViewMode::MaskOnly => {
                    let avail_w = ui.available_width() - 32.0;
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = avail_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Diff Mask (Dark Canvas + Glowing Difference)")
                                .strong()
                                .color(del_color),
                        );
                        ui.add_space(4.0);

                        if let Some(tex) = &self.mask_texture {
                            egui::Frame::none()
                                .fill(b_low)
                                .rounding(Rounding::same(6.0))
                                .stroke(Stroke::new(1.0, b_med))
                                .inner_margin(egui::Margin::same(4.0))
                                .show(ui, |ui| {
                                    ui.image(egui::load::SizedTexture::new(
                                        tex.id(),
                                        Vec2::new(img_w, img_h),
                                    ));
                                });
                        }
                    });
                }
                ImageDiffViewMode::Split => {
                    let avail_w = ui.available_width() - 32.0;
                    let col_w = ((avail_w - 16.0) / 2.0).max(240.0);
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = col_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.horizontal(|ui| {
                        ui.add_space(12.0);

                        self.render_image_column(
                            ui,
                            palette,
                            "base",
                            self.lhs.label(),
                            f_high,
                            self.base_texture.as_ref(),
                            img_w,
                            img_h,
                        );

                        ui.add_space(16.0);

                        let diff_sub = format!(
                            "{} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        );
                        self.render_image_column(
                            ui,
                            palette,
                            "diff",
                            &diff_sub,
                            del_color,
                            self.mask_texture.as_ref(),
                            img_w,
                            img_h,
                        );
                    });
                }
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("⚠️ Unable to Compare Images")
                            .color(del_color)
                            .heading(),
                    );
                    ui.add_space(8.0);
                    let err_text = self
                        .decode_error
                        .as_deref()
                        .unwrap_or("Unable to decode one or both images for comparison");
                    ui.label(egui::RichText::new(err_text).color(f_med).size(14.0));
                });
            });
        }
    }

    fn render_text_viewport(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);

        egui::Frame::none()
            .fill(b_low)
            .rounding(Rounding::same(6.0))
            .stroke(Stroke::new(1.0, b_med))
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(self.layout_job.clone()).sense(egui::Sense::click()),
                        );
                    });
            });
    }

    fn render_text_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .rounding(Rounding::same(4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let is_side = self.mode == DiffDisplayMode::SideBySide;
                    let mode_label = if is_side {
                        "◫ Side-by-Side"
                    } else {
                        "≡ Unified"
                    };
                    let btn = crate::mode_tab_button(mode_label, true, palette);
                    if ui
                        .add(btn)
                        .on_hover_text("Toggle text diff display mode (Tab / C)")
                        .clicked()
                    {
                        self.toggle_mode();
                    }
                });
            });
    }

    fn render_computing_ui(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
    ) {
        let elapsed = self.compute_start_time.elapsed().as_secs_f32();
        let progress = (elapsed * 0.75).fract();
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);

        ui.centered_and_justified(|ui| {
            egui::Frame::none()
                .fill(b_med)
                .stroke(Stroke::new(1.0, b_low))
                .rounding(Rounding::same(12.0))
                .inner_margin(egui::Margin::symmetric(36.0, 28.0))
                .show(ui, |ui| {
                    ui.set_max_width(400.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Spinner::new().size(40.0).color(accent));
                        ui.add_space(14.0);

                        ui.label(
                            egui::RichText::new("Comparing Images...")
                                .heading()
                                .strong()
                                .color(f_high),
                        );
                        ui.add_space(8.0);

                        // Animated progress indicator bar
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .animate(true)
                                .desired_width(300.0)
                                .text("Parallel Perceptual Diff"),
                        );

                        ui.add_space(12.0);

                        ui.label(
                            egui::RichText::new(format!("Base: {}", self.lhs.label()))
                                .small()
                                .color(f_low),
                        );
                        ui.label(
                            egui::RichText::new(format!("Comp: {}", self.rhs.label()))
                                .small()
                                .color(f_low),
                        );

                        ui.add_space(8.0);

                        ui.label(
                            egui::RichText::new(format!(
                                "{:.2}s elapsed • multithreaded SIMD comparison",
                                elapsed
                            ))
                            .small()
                            .color(accent),
                        );

                        ui.add_space(10.0);

                        ui.label(
                            egui::RichText::new("[Esc] Cancel / Close")
                                .small()
                                .color(f_low),
                        );
                    });
                });
        });
    }

    fn render_image_column(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
        title: &str,
        subtitle: &str,
        title_color: Color32,
        texture: Option<&egui::TextureHandle>,
        w: f32,
        h: f32,
    ) {
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);

        ui.vertical(|ui| {
            ui.set_width(w);

            // Column Header
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).strong().color(title_color));
                ui.label(
                    egui::RichText::new(format!("• {}", subtitle))
                        .small()
                        .color(f_low),
                );
            });

            ui.add_space(4.0);

            // Image Frame Box
            egui::Frame::none()
                .fill(b_low)
                .rounding(Rounding::same(6.0))
                .stroke(Stroke::new(1.0, b_med))
                .inner_margin(egui::Margin::same(2.0))
                .show(ui, |ui| {
                    if let Some(tex) = texture {
                        ui.image(egui::load::SizedTexture::new(tex.id(), Vec2::new(w, h)));
                    } else {
                        ui.allocate_space(Vec2::new(w, h));
                    }
                });
        });
    }

    fn copy_image_diff_to_clipboard(&mut self, now: f64) {
        if let Some(res) = &self.image_result {
            if let Ok(png_bytes) = res.encode_mask_png() {
                let req = IpcRequest::PushImage {
                    bytes: png_bytes,
                    mime: "image/png".to_string(),
                    hand: HandSide::Left,
                };
                let _ = send_ipc_request(&req);
                self.status_toast = Some((
                    "Diff Mask copied to Left Hand & clipboard!".to_string(),
                    now,
                ));
            }
        }
    }
}

fn strip_ansi_codes(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut in_escape = false;

    for c in text.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            clean.push(c);
        }
    }

    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes() {
        let text = "\x1b[91m-old\x1b[0m\n\x1b[92m+new\x1b[0m";
        assert_eq!(strip_ansi_codes(text), "-old\n+new");
    }

    #[test]
    fn image_diff_state_uses_generated_fixture_images() {
        let temp_dir = tempfile::tempdir().unwrap();
        let left_path = temp_dir.path().join("left.png");
        let right_path = temp_dir.path().join("right.png");
        let left = image::RgbaImage::from_pixel(3, 2, image::Rgba([0, 0, 0, 255]));
        let mut right = left.clone();
        right.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        left.save(&left_path).unwrap();
        right.save(&right_path).unwrap();

        let left = little_clip_core::ClipItem::from_single_file(&left_path);
        let right = little_clip_core::ClipItem::from_single_file(&right_path);
        let left = little_clip_ops::diff::resolve_diff_target(&left).unwrap();
        let right = little_clip_ops::diff::resolve_diff_target(&right).unwrap();
        let mut state = DiffViewerState::new(left, right, DiffDisplayMode::SideBySide);
        state.wait_for_completion();

        let result = state.image_result.unwrap();
        assert_eq!((result.width, result.height), (3, 2));
        assert_eq!(result.diff_count, 1);
    }
}
