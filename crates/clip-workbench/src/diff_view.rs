//! Text and image differential inspection workbench view.
//!
//! Provides side-by-side, unified, and inline word diffs for text, and 3-up/overlay/mask
//! pixel diffs for bitmap images.

use crate::ansi::ansi_to_layout_job;
use crate::icons::Icon;
use crate::send_ipc_request;
use crate::shell::{EscapeHatch, StudioShell};
use crate::ui_ext::{BODY_FONT_SIZE, CONTROL_HEIGHT};
use eframe::egui::{self, Color32, Rounding, Stroke, Vec2};
use little_clip_core::HandSide;
use little_clip_core::ipc::IpcRequest;
use little_clip_core::{BackgroundJobs, JobTicket};
use little_clip_ops::diff::{DiffDisplayMode, DiffResult, DiffTarget, compute_diff};
use little_clip_ops::image_diff::{ImageDiffOptions, ImageDiffResult};

enum DiffHandoffTarget {
    Existing(std::path::PathBuf),
    Shared(std::sync::Arc<DiffTarget>, String),
}

struct HandoffCancelGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for HandoffCancelGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl DiffHandoffTarget {
    fn from_target(target: &std::sync::Arc<DiffTarget>, side: &str) -> Self {
        match target.as_ref() {
            DiffTarget::File { path, .. }
            | DiffTarget::Image {
                path: Some(path), ..
            } => Self::Existing(path.clone()),
            _ => Self::Shared(target.clone(), side.into()),
        }
    }

    fn stage(self) -> Result<std::path::PathBuf, String> {
        match self {
            Self::Existing(path) => Ok(path),
            Self::Shared(target, side) => {
                let (bytes, ext): (&[u8], &str) = match target.as_ref() {
                    DiffTarget::ClipboardText {
                        content, ext_hint, ..
                    } => (content.as_bytes(), ext_hint.as_deref().unwrap_or("txt")),
                    DiffTarget::Image { bytes, mime, .. } => {
                        (bytes, if mime.contains("png") { "png" } else { "jpg" })
                    }
                    _ => unreachable!(),
                };
                let path = std::env::temp_dir().join(format!(
                    "clip_diff_{side}_{}.{}",
                    uuid::Uuid::new_v4().simple(),
                    ext
                ));
                std::fs::write(&path, bytes)
                    .map_err(|error| format!("Failed to stage diff {side} file: {error}"))?;
                Ok(path)
            }
        }
    }
}

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
    /// Human-readable display label.
    pub fn label(&self) -> &'static str {
        match self {
            ImageDiffViewMode::ThreeUp => "3-Up (Base | Comp | Diff)",
            ImageDiffViewMode::Overlay => "Overlay",
            ImageDiffViewMode::MaskOnly => "Diff Mask",
            ImageDiffViewMode::Split => "Split (Base | Diff)",
        }
    }

    /// SVG icon for mode tab.
    pub fn icon(&self) -> Icon {
        match self {
            ImageDiffViewMode::ThreeUp => Icon::Grid,
            ImageDiffViewMode::Overlay => Icon::Layers,
            ImageDiffViewMode::MaskOnly => Icon::Eye,
            ImageDiffViewMode::Split => Icon::Diff,
        }
    }
}

/// Interactive state for the text and visual image diff viewer.
pub struct DiffViewerState {
    pub lhs: std::sync::Arc<DiffTarget>,
    pub rhs: std::sync::Arc<DiffTarget>,
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
    pub image_receiver: Option<
        std::sync::mpsc::Receiver<
            Result<Result<ImageDiffResult, String>, little_clip_core::JobError>,
        >,
    >,
    pub text_receiver: Option<
        std::sync::mpsc::Receiver<
            Result<(DiffResult, egui::text::LayoutJob), little_clip_core::JobError>,
        >,
    >,
    compute_ticket: Option<JobTicket>,
    pub is_computing: bool,
    pub compute_start_time: std::time::Instant,
    handoff: Option<(
        JobTicket,
        std::sync::mpsc::Receiver<Result<Result<String, String>, little_clip_core::JobError>>,
    )>,
    handoff_cancel: Option<HandoffCancelGuard>,
}

impl Drop for DiffViewerState {
    fn drop(&mut self) {
        if let Some(ticket) = &self.compute_ticket {
            ticket.cancel();
        }
    }
}

fn spawn_image_diff_worker(
    lhs: &DiffTarget,
    rhs: &DiffTarget,
    options: ImageDiffOptions,
    jobs: &BackgroundJobs,
) -> (
    Option<JobTicket>,
    Option<
        std::sync::mpsc::Receiver<
            Result<Result<ImageDiffResult, String>, little_clip_core::JobError>,
        >,
    >,
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
            return (None, None, Some(err));
        }

        let b1_vec = b1.to_vec();
        let b2_vec = b2.to_vec();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            little_clip_ops::image_diff::compare_image_bytes(&b1_vec, &b2_vec, &options)
                .map_err(|e| e.to_string())
        });

        (Some(ticket), Some(rx), None)
    } else {
        (
            None,
            None,
            Some("Failed to obtain image bytes from comparison targets".to_string()),
        )
    }
}

fn spawn_text_diff_worker(
    lhs: &DiffTarget,
    rhs: &DiffTarget,
    mode: DiffDisplayMode,
    width: usize,
    jobs: &BackgroundJobs,
) -> (
    JobTicket,
    std::sync::mpsc::Receiver<
        Result<(DiffResult, egui::text::LayoutJob), little_clip_core::JobError>,
    >,
) {
    let lhs = lhs.clone();
    let rhs = rhs.clone();
    jobs.submit(std::time::Duration::MAX, move || {
        let r = compute_diff(&lhs, &rhs, mode, width);
        let job = ansi_to_layout_job(&r.raw_ansi, crate::ui_ext::MONO_FONT_SIZE);
        (r, job)
    })
}

impl DiffViewerState {
    /// Initializes text or image diff calculation between two comparison targets.
    pub fn new(lhs: DiffTarget, rhs: DiffTarget, mode: DiffDisplayMode) -> Self {
        Self::new_with_jobs(lhs, rhs, mode, BackgroundJobs::global())
    }

    fn new_with_jobs(
        lhs: DiffTarget,
        rhs: DiffTarget,
        mode: DiffDisplayMode,
        jobs: &BackgroundJobs,
    ) -> Self {
        let is_img = lhs.is_image() && rhs.is_image();
        let image_options = ImageDiffOptions::default();

        let (compute_ticket, image_receiver, text_receiver, is_computing, decode_error) = if is_img
        {
            let (ticket, rx, err) =
                spawn_image_diff_worker(&lhs, &rhs, image_options.clone(), jobs);
            let computing = rx.is_some();
            (ticket, rx, None, computing, err)
        } else {
            let width = if mode == DiffDisplayMode::SideBySide {
                150
            } else {
                120
            };
            let (ticket, rx) = spawn_text_diff_worker(&lhs, &rhs, mode, width, jobs);
            (Some(ticket), None, Some(rx), true, None)
        };

        let result = DiffResult {
            engine_name: "Computing...".to_string(),
            raw_ansi: String::new(),
            is_identical: false,
            mode,
        };
        let layout_job = ansi_to_layout_job("", crate::ui_ext::MONO_FONT_SIZE);

        Self {
            lhs: std::sync::Arc::new(lhs),
            rhs: std::sync::Arc::new(rhs),
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
            text_receiver,
            compute_ticket,
            is_computing,
            compute_start_time: std::time::Instant::now(),
            handoff: None,
            handoff_cancel: None,
        }
    }

    #[allow(dead_code)]
    pub fn wait_for_completion(&mut self) {
        if let Some(rx) = self.image_receiver.take() {
            if let Ok(res) = rx.recv() {
                match res {
                    Ok(Ok(r)) => {
                        self.image_result = Some(r);
                        self.decode_error = None;
                    }
                    Ok(Err(e)) => {
                        self.decode_error = Some(e);
                        self.image_result = None;
                    }
                    Err(e) => {
                        self.decode_error = Some(format!("Diff computation failed: {e}"));
                        self.image_result = None;
                    }
                }
            }
            self.is_computing = false;
            self.compute_ticket = None;
        }
        if let Some(rx) = self.text_receiver.take() {
            match rx.recv() {
                Ok(Ok((res, job))) => {
                    self.result = res;
                    self.layout_job = job;
                    self.decode_error = None;
                }
                Ok(Err(error)) => {
                    self.decode_error = Some(format!("Diff computation failed: {error}"));
                }
                Err(_) => {
                    self.decode_error = Some("Diff worker disconnected".to_string());
                }
            }
            self.is_computing = false;
            self.compute_ticket = None;
        }
    }

    pub fn is_image(&self) -> bool {
        self.lhs.is_image() && self.rhs.is_image()
    }

    pub fn recompute(&mut self) {
        if self.is_image() {
            if let Some(ticket) = self.compute_ticket.take() {
                ticket.cancel();
            }
            let (ticket, rx, err) = spawn_image_diff_worker(
                &self.lhs,
                &self.rhs,
                self.image_options.clone(),
                BackgroundJobs::global(),
            );
            self.compute_ticket = ticket;
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
            if let Some(ticket) = self.compute_ticket.take() {
                ticket.cancel();
            }
            let (ticket, rx) = spawn_text_diff_worker(
                &self.lhs,
                &self.rhs,
                self.mode,
                width,
                BackgroundJobs::global(),
            );
            self.compute_ticket = Some(ticket);
            self.is_computing = true;
            self.text_receiver = Some(rx);
            self.compute_start_time = std::time::Instant::now();
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
        if let Some((ticket, _)) = self.handoff.take() {
            ticket.cancel();
        }
        self.handoff_cancel.take();
        let left = DiffHandoffTarget::from_target(&self.lhs, "lhs");
        let right = DiffHandoffTarget::from_target(&self.rhs, "rhs");
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job_cancelled = cancelled.clone();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(std::time::Duration::from_secs(30), move || {
                let left_path = left.stage()?;
                let right_path = right.stage()?;
                if job_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("External handoff cancelled".to_string());
                }
                if let Some(app) = little_clip_core::command_available("meld") {
                    if std::process::Command::new(app)
                        .arg(&left_path)
                        .arg(&right_path)
                        .spawn()
                        .is_ok()
                    {
                        return Ok("🚀 Launched Meld comparison".to_string());
                    }
                }
                if let Some(app) = little_clip_core::command_available("kdiff3") {
                    if std::process::Command::new(app)
                        .arg(&left_path)
                        .arg(&right_path)
                        .spawn()
                        .is_ok()
                    {
                        return Ok("🚀 Launched KDiff3 comparison".to_string());
                    }
                }
                Ok("⚠️ Neither Meld nor KDiff3 is installed".to_string())
            });
        self.status_toast = Some(("Preparing comparison for external tool…".to_string(), now));
        self.handoff = Some((ticket, receiver));
        self.handoff_cancel = Some(HandoffCancelGuard(cancelled));
    }

    fn poll_handoff(&mut self, now: f64, ctx: &egui::Context) {
        let Some((_, rx)) = &self.handoff else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Ok(message))) => {
                self.status_toast = Some((message, now));
                self.handoff = None;
            }
            Ok(Ok(Err(error))) => {
                self.status_toast = Some((format!("❌ {error}"), now));
                self.handoff = None;
            }
            Ok(Err(error)) => {
                self.status_toast = Some((format!("❌ External handoff failed: {error}"), now));
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_toast = Some(("❌ External handoff was cancelled".to_string(), now));
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(80))
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context, close_requested: &mut bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close_requested = true;
        }
        if ctx.wants_keyboard_input() {
            return;
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
        if ctx.input(|i| {
            i.key_pressed(egui::Key::S)
                && !i.modifiers.command
                && !i.modifiers.ctrl
                && !i.modifiers.mac_cmd
                && !i.modifiers.shift
                && !i.modifiers.alt
        }) {
            self.swap_operands();
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);
        self.poll_handoff(now, ctx);

        self.handle_shortcuts(ctx, close_requested);

        let is_img = self.is_image();

        if is_img {
            if self.is_computing {
                ctx.request_repaint();
                if let Some(ref rx) = self.image_receiver {
                    match rx.try_recv() {
                        Ok(Ok(Ok(res))) => {
                            self.image_result = Some(res);
                            self.decode_error = None;
                            self.is_computing = false;
                            self.textures_loaded = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                        Ok(Ok(Err(err))) => {
                            self.decode_error = Some(err);
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                        Ok(Err(err)) => {
                            self.decode_error = Some(format!("Diff computation failed: {err}"));
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            self.decode_error = Some("Diff worker thread disconnected".to_string());
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                    }
                }
            }

            self.ensure_image_textures(ctx);
        } else if self.is_computing {
            ctx.request_repaint();
            if let Some(ref rx) = self.text_receiver {
                match rx.try_recv() {
                    Ok(Ok((res, job))) => {
                        self.result = res;
                        self.layout_job = job;
                        self.is_computing = false;
                        self.text_receiver = None;
                        self.decode_error = None;
                        self.compute_ticket = None;
                    }
                    Ok(Err(err)) => {
                        self.is_computing = false;
                        self.text_receiver = None;
                        self.decode_error = Some(format!("Diff computation failed: {err}"));
                        self.compute_ticket = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.is_computing = false;
                        self.text_receiver = None;
                        self.decode_error = Some("Diff worker disconnected".to_string());
                        self.compute_ticket = None;
                    }
                }
            }
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

        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let success_color = crate::hex_to_c32(&palette.diff_add);
        let del_color = crate::hex_to_c32(&palette.diff_del);

        let hatch = EscapeHatch::new("Meld", "meld", "Open comparison in external Meld tool")
            .with_icon(Icon::Diff);

        let mut shell = if is_img {
            let mut s = StudioShell::new("Picture Diff Comparison", Icon::Diff, palette, close_requested)
                .with_escape_hatch(hatch)
                .with_footer_hints("Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Mask");

            if self.is_computing {
                let elapsed = self.compute_start_time.elapsed().as_secs_f32();
                s = s.with_badge(format!("Comparing... ({:.1}s)", elapsed), Some(accent));
            } else if let Some(res) = &self.image_result {
                if res.is_identical {
                    s = s.with_badge("0 diff pixels • Identical", Some(success_color));
                } else {
                    s = s.with_badge(
                        format!(
                            "{} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        ),
                        Some(del_color),
                    );
                }
                s = s.with_badge(format!("{} × {}", res.width, res.height), None);
                s = s.with_footer_readout(format!(
                    "Tolerance: {:.2} • {} diff pixels",
                    self.image_options.threshold, res.diff_count
                ));
            }
            s
        } else {
            let lines_count = self.result.raw_ansi.lines().count();
            let mut s = StudioShell::new("Diff Comparison", Icon::Diff, palette, close_requested)
                .with_badge(format!("{}", self.result.engine_name), Some(accent))
                .with_escape_hatch(hatch)
                .with_footer_hints("Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Diff");

            if self.result.is_identical {
                s = s.with_badge("Identical", Some(success_color));
            } else {
                s = s.with_badge(format!("{} lines in diff", lines_count), None);
            }
            s = s.with_footer_readout(format!(
                "LHS: {} • RHS: {}",
                self.lhs.label(),
                self.rhs.label()
            ));
            s
        };

        egui::Frame::none()
            .fill(crate::hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Swap),
                        "Swap [S]",
                        b_med,
                        palette,
                    );
                    if ui
                        .add(swap_btn)
                        .on_hover_text("Swap operands (S)")
                        .clicked()
                    {
                        self.swap_operands();
                    }

                    let copy_label = if is_img {
                        "Copy Diff Mask"
                    } else {
                        "Copy Diff"
                    };
                    let copy_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Copy),
                        copy_label,
                        b_med,
                        palette,
                    );
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

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 2: Mode Toolbar
                if is_img {
                    self.render_image_toolbar(ui, palette);
                } else {
                    self.render_text_toolbar(ui, palette);
                }

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

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

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 5: Footer
                shell.render_footer(ui, &self.status_toast, now);
            });
    }

    fn render_image_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::SPACE_MD,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
            .show(ui, |ui| {
                ui.set_min_height(CONTROL_HEIGHT);
                ui.horizontal(|ui| {
                    for mode in &[
                        ImageDiffViewMode::ThreeUp,
                        ImageDiffViewMode::Overlay,
                        ImageDiffViewMode::MaskOnly,
                        ImageDiffViewMode::Split,
                    ] {
                        let is_active = self.image_mode == *mode;
                        let btn = crate::ui_ext::mode_tab_button_icon(Some(mode.icon()), mode.label(), is_active, palette);
                        if ui.add(btn).clicked() {
                            self.image_mode = *mode;
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_SM);

                    ui.label(
                        egui::RichText::new("Tolerance:")
                            .size(BODY_FONT_SIZE)
                            .color(f_low),
                    );
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
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);
        let f_med = crate::hex_to_c32(&palette.colors.text_secondary);
        let del_color = crate::hex_to_c32(&palette.diff_del);
        let del_label_color = crate::ui_ext::contrast_text_c32_on(
            ui.visuals().panel_fill,
            ui.visuals().panel_fill,
            Some(del_color),
            palette,
        );

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
                        ui.add_space(crate::ui_ext::SPACE_MD);

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

                        ui.add_space(crate::ui_ext::SPACE_MD);

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

                        ui.add_space(crate::ui_ext::SPACE_MD);

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
                        ui.add_space(crate::ui_ext::SPACE_XS);

                        if let Some(tex) = &self.overlay_texture {
                            egui::Frame::none()
                                .fill(b_low)
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .stroke(Stroke::new(1.0_f32, b_med))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_XS))
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
                                .color(del_label_color),
                        );
                        ui.add_space(crate::ui_ext::SPACE_XS);

                        if let Some(tex) = &self.mask_texture {
                            egui::Frame::none()
                                .fill(b_low)
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .stroke(Stroke::new(1.0_f32, b_med))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_XS))
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
                        ui.add_space(crate::ui_ext::SPACE_MD);

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
                        egui::RichText::new("Unable to Compare Images")
                            .color(del_label_color)
                            .heading(),
                    );
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    let err_text = self
                        .decode_error
                        .as_deref()
                        .unwrap_or("Unable to decode one or both images for comparison");
                    ui.label(
                        egui::RichText::new(err_text)
                            .color(f_med)
                            .size(BODY_FONT_SIZE),
                    );
                });
            });
        }
    }

    fn render_text_viewport(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        if self.is_computing {
            self.render_computing_ui(ui, palette);
            return;
        }

        if let Some(error) = &self.decode_error {
            ui.label(
                egui::RichText::new(format!("Diff unavailable: {error}"))
                    .color(crate::hex_to_c32(&palette.colors.text_primary))
                    .size(BODY_FONT_SIZE),
            );
        }

        let b_low = crate::hex_to_c32(&palette.colors.console_surface);
        let b_med = crate::hex_to_c32(&palette.chrome_border);

        egui::Frame::none()
            .fill(b_low)
            .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
            .stroke(Stroke::new(1.0_f32, b_med))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(self.layout_job.clone())
                                .wrap(false)
                                .sense(egui::Sense::click()),
                        );
                    });
            });
    }

    fn render_text_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::SPACE_MD,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
            .show(ui, |ui| {
                ui.set_min_height(CONTROL_HEIGHT);
                ui.horizontal(|ui| {
                    let is_side = self.mode == DiffDisplayMode::SideBySide;
                    let (mode_label, mode_icon) = if is_side {
                        ("Side-by-Side", Icon::Layers)
                    } else {
                        ("Unified", Icon::Text)
                    };
                    let btn = crate::ui_ext::mode_tab_button_icon(
                        Some(mode_icon),
                        mode_label,
                        true,
                        palette,
                    );
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
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let elapsed = self.compute_start_time.elapsed().as_secs_f32();
        let progress = (elapsed * 0.75).fract();
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let accent = crate::hex_to_c32(&palette.harmonies.base);

        let heading = if self.is_image() {
            "Comparing Images..."
        } else {
            "Computing Diff..."
        };
        let bar_text = if self.is_image() {
            "Parallel Perceptual Diff"
        } else {
            "Syntax-Highlighted Diff"
        };
        let readout = if self.is_image() {
            format!("{:.2}s elapsed • multithreaded SIMD comparison", elapsed)
        } else {
            format!("{:.2}s elapsed • background syntax diff engine", elapsed)
        };

        ui.centered_and_justified(|ui| {
            egui::Frame::none()
                .fill(b_med)
                .stroke(Stroke::new(1.0_f32, b_low))
                .rounding(Rounding::same(12.0))
                .inner_margin(egui::Margin::symmetric(36.0, 28.0))
                .show(ui, |ui| {
                    ui.set_max_width(400.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Spinner::new().size(40.0).color(accent));
                        ui.add_space(14.0);

                        ui.label(
                            egui::RichText::new(heading)
                                .heading()
                                .strong()
                                .color(f_high),
                        );
                        ui.add_space(crate::ui_ext::SPACE_SM);

                        // Animated progress indicator bar
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .animate(true)
                                .desired_width(300.0)
                                .text(bar_text),
                        );

                        ui.add_space(crate::ui_ext::SPACE_MD);

                        ui.label(
                            egui::RichText::new(format!("Base: {}", self.lhs.label()))
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        ui.label(
                            egui::RichText::new(format!("Comp: {}", self.rhs.label()))
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );

                        ui.add_space(crate::ui_ext::SPACE_SM);

                        ui.label(egui::RichText::new(readout).size(BODY_FONT_SIZE).color(
                            crate::ui_ext::contrast_text_c32_on(
                                b_med,
                                b_med,
                                Some(accent),
                                palette,
                            ),
                        ));

                        ui.add_space(10.0);

                        ui.label(
                            egui::RichText::new("[Esc] Cancel / Close")
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );
                    });
                });
        });
    }

    fn render_image_column(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
        title: &str,
        subtitle: &str,
        title_color: Color32,
        texture: Option<&egui::TextureHandle>,
        w: f32,
        h: f32,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let surface = ui.visuals().panel_fill;

        ui.vertical(|ui| {
            ui.set_width(w);

            // Column Header
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).strong().color(
                    crate::ui_ext::contrast_text_c32_on(
                        surface,
                        surface,
                        Some(title_color),
                        palette,
                    ),
                ));
                ui.label(
                    egui::RichText::new(format!("• {}", subtitle))
                        .size(BODY_FONT_SIZE)
                        .color(f_low),
                );
            });

            ui.add_space(crate::ui_ext::SPACE_XS);

            // Image Frame Box
            egui::Frame::none()
                .fill(b_low)
                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                .stroke(Stroke::new(1.0_f32, b_med))
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
    fn image_diff_labels_meet_contrast_on_the_view_surface() {
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = crate::ui_appearance::palette(appearance);
            let surface = crate::hex_to_c32(&palette.colors.canvas);
            for preferred in [
                crate::hex_to_c32(&palette.diff_del),
                crate::hex_to_c32(&palette.harmonies.base),
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
                    "{appearance:?} image-diff custom label lacks contrast"
                );
            }
        }
    }

    #[test]
    fn modified_s_does_not_swap_diff_operands() {
        fn send_modified_s(state: &mut DiffViewerState, modifiers: egui::Modifiers) {
            let ctx = egui::Context::default();
            let mut close = false;
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key: egui::Key::S,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| state.handle_shortcuts(ctx, &mut close),
            );
            assert!(!close);
        }

        let left = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "old text\n".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let right = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "new text\n".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
        state.wait_for_completion();
        let original_result = state.result.raw_ansi.clone();

        for modifiers in [
            egui::Modifiers::CTRL,
            egui::Modifiers::COMMAND,
            egui::Modifiers::MAC_CMD,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
            egui::Modifiers::ALT,
        ] {
            send_modified_s(&mut state, modifiers);
            assert_eq!(state.lhs.label(), "Left");
            assert_eq!(state.rhs.label(), "Right");
            assert!(!state.is_computing);
            assert_eq!(state.result.raw_ansi, original_result);
        }
    }

    #[test]
    fn unmodified_s_swaps_operands_and_recomputes_text_diff() {
        let left = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "old text\n".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let right = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "new text\n".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
        state.wait_for_completion();

        let ctx = egui::Context::default();
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::S,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| state.handle_shortcuts(ctx, &mut close),
        );

        assert!(!close);
        assert_eq!(state.lhs.label(), "Right");
        assert_eq!(state.rhs.label(), "Left");
        assert!(state.is_computing);
        state.wait_for_completion();
        let plain_result = strip_ansi_codes(&state.result.raw_ansi);
        assert!(plain_result.contains("+ old text"));
        assert!(plain_result.contains("- new text"));
    }

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

        let result = state.image_result.as_ref().unwrap();
        assert_eq!((result.width, result.height), (3, 2));
        assert_eq!(result.diff_count, 1);
    }

    #[test]
    fn text_diff_state_computes_in_background() {
        let left = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "hello world\nalpha\nbravo".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let right = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "hello world\ncharlie\nbravo".to_string(),
            ext_hint: Some("txt".to_string()),
        };

        let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
        assert!(state.is_computing);
        state.wait_for_completion();
        assert!(!state.is_computing);
        assert!(!state.result.raw_ansi.is_empty());

        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette = crate::ui_appearance::palette(appearance);
            let console = crate::hex_to_c32(&palette.colors.console_surface);
            let mut close = false;
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            });
            assert!(output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == console)
            }));
        }
    }

    #[test]
    fn text_diff_tab_and_c_recompute_the_selected_mode() {
        fn send_shortcut(state: &mut DiffViewerState, key: egui::Key) {
            let ctx = egui::Context::default();
            let mut close = false;
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| state.handle_shortcuts(ctx, &mut close),
            );
            assert!(!close);
        }

        let left = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "alpha\nold line\nomega\n".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let right = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "alpha\nnew line\nomega\n".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
        state.wait_for_completion();
        let inline_result = state.result.raw_ansi.clone();

        send_shortcut(&mut state, egui::Key::Tab);
        assert_eq!(state.mode, DiffDisplayMode::SideBySide);
        assert!(state.is_computing, "Tab should schedule a new text diff");
        assert_eq!(state.result.mode, DiffDisplayMode::Inline);
        state.wait_for_completion();
        assert_eq!(state.result.mode, DiffDisplayMode::SideBySide);
        let split_result = state.result.raw_ansi.clone();
        assert_ne!(split_result, inline_result);

        send_shortcut(&mut state, egui::Key::C);
        assert_eq!(state.mode, DiffDisplayMode::Inline);
        assert!(state.is_computing, "C should schedule a new text diff");
        assert_eq!(state.result.mode, DiffDisplayMode::SideBySide);
        state.wait_for_completion();
        assert_eq!(state.result.mode, DiffDisplayMode::Inline);
        assert_eq!(state.result.raw_ansi, inline_result);
    }

    #[test]
    fn image_diff_tab_and_c_cycle_modes_without_recomputing() {
        fn send_shortcut(state: &mut DiffViewerState, key: egui::Key) {
            let ctx = egui::Context::default();
            let mut close = false;
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| state.handle_shortcuts(ctx, &mut close),
            );
            assert!(!close);
        }

        let image_target = |label: &str, color: [u8; 4]| {
            let rgba = image::RgbaImage::from_pixel(2, 2, image::Rgba(color));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(rgba)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .expect("encode tiny PNG fixture");
            DiffTarget::Image {
                label: label.to_owned(),
                path: None,
                bytes: bytes.into_inner(),
                width: 2,
                height: 2,
                mime: "image/png".to_owned(),
            }
        };
        let mut state = DiffViewerState::new(
            image_target("Left", [255, 0, 0, 255]),
            image_target("Right", [0, 0, 255, 255]),
            DiffDisplayMode::SideBySide,
        );
        state.wait_for_completion();
        let initial_result = state.image_result.as_ref().expect("image diff result");
        let result_address = std::ptr::from_ref(initial_result);
        let compute_started = state.compute_start_time;

        for (key, expected_mode) in [
            (egui::Key::Tab, ImageDiffViewMode::Overlay),
            (egui::Key::C, ImageDiffViewMode::MaskOnly),
            (egui::Key::Tab, ImageDiffViewMode::Split),
            (egui::Key::C, ImageDiffViewMode::ThreeUp),
        ] {
            send_shortcut(&mut state, key);
            assert_eq!(state.image_mode, expected_mode);
            assert!(!state.is_computing, "presentation change must not compute");
            assert!(state.image_receiver.is_none());
            assert_eq!(state.compute_start_time, compute_started);
            assert_eq!(
                std::ptr::from_ref(state.image_result.as_ref().unwrap()),
                result_address,
                "presentation change must retain the computed image result"
            );
        }
    }

    #[test]
    fn saturated_diff_executor_reports_recoverable_error() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let _queued = jobs.submit(Duration::MAX, || ());

        let left = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "left".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let right = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "right".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let mut state = DiffViewerState::new_with_jobs(left, right, DiffDisplayMode::Inline, &jobs);
        assert!(state.is_computing);
        state.wait_for_completion();

        assert!(!state.is_computing);
        assert!(state.text_receiver.is_none());
        assert_eq!(
            state.decode_error.as_deref(),
            Some("Diff computation failed: background job queue is full")
        );

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let error_text = "Diff unavailable: Diff computation failed: background job queue is full";
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 500.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == error_text)
        }));

        let _ = release_tx.send(());
    }

    #[test]
    fn dropping_diff_viewer_cancels_queued_compute() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let left = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "left".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let right = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "right".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let mut state = DiffViewerState::new_with_jobs(left, right, DiffDisplayMode::Inline, &jobs);
        let result_rx = state.text_receiver.take().unwrap();
        drop(state);

        let _ = release_tx.send(());
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
    }

    #[test]
    fn ansi_long_line_scrolls_to_full_tail_with_readable_colors_in_both_appearances() {
        use little_clip_core::config::WorkbenchAppearance;
        use little_clip_core::palette::contrast_ratio;

        let prefix = "x".repeat(5_000);
        let ansi = format!(
            "{prefix}\x1b[31mSTANDARD_END\x1b[0m \x1b[38;2;14;43;61mTRUECOLOR_END\x1b[0m \x1b[38;5;196m256COLOR_END\x1b[0m"
        );
        let layout_job = ansi_to_layout_job(&ansi, crate::ui_ext::MONO_FONT_SIZE);
        let plain = layout_job.text.clone();
        let marker = "256COLOR_END";
        let marker_start = plain.find(marker).expect("plain ANSI tail marker");

        for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
            let palette = crate::ui_appearance::palette(appearance);
            let console = crate::hex_to_c32(&palette.colors.console_surface);
            for token in ["STANDARD_END", "TRUECOLOR_END", "256COLOR_END"] {
                let section = layout_job
                    .sections
                    .iter()
                    .find(|section| layout_job.text[section.byte_range.clone()].contains(token))
                    .unwrap_or_else(|| panic!("missing ANSI section for {token}"));
                let foreground = section.format.color;
                assert!(
                    contrast_ratio(
                        (console.r(), console.g(), console.b()),
                        (foreground.r(), foreground.g(), foreground.b()),
                    ) >= 4.5,
                    "{appearance:?}: {token} foreground lacks contrast against console surface"
                );
            }

            let lhs = DiffTarget::ClipboardText {
                label: "Left".to_string(),
                content: "left placeholder".to_string(),
                ext_hint: Some("txt".to_string()),
            };
            let rhs = DiffTarget::ClipboardText {
                label: "Right".to_string(),
                content: "right placeholder".to_string(),
                ext_hint: Some("txt".to_string()),
            };
            let mut state = DiffViewerState::new(lhs, rhs, DiffDisplayMode::Inline);
            state.result = DiffResult {
                engine_name: "ANSI fixture".to_string(),
                raw_ansi: ansi.clone(),
                is_identical: false,
                mode: DiffDisplayMode::Inline,
            };
            state.layout_job = layout_job.clone();
            state.is_computing = false;
            state.text_receiver = None;

            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 620.0));
            let mut time = 0.0;
            let mut render = |events: Vec<egui::Event>| {
                time += 0.05;
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen_rect),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_ui(ui, ctx, &palette, &mut false);
                        });
                    },
                )
            };

            let initial = render(Vec::new());
            let find_line = |output: &egui::FullOutput| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.text() == plain => {
                            Some((text.pos, text.galley.size(), clipped.clip_rect))
                        }
                        _ => None,
                    })
            };
            let find_console_rect = |output: &egui::FullOutput| {
                output
                    .shapes
                    .iter()
                    .filter_map(|clipped| match &clipped.shape {
                        egui::Shape::Rect(rect) if rect.fill == console => Some(rect.rect),
                        _ => None,
                    })
                    .max_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
            };
            let (initial_pos, initial_size, viewport) =
                find_line(&initial).expect("full ANSI line should render without elision");
            assert!(
                initial_size.x > viewport.width() * 10.0,
                "long-line size {} should greatly exceed viewport width {}",
                initial_size.x,
                viewport.width()
            );
            assert!(plain.ends_with(marker));

            let console_rect = find_console_rect(&initial).expect("console surface should render");
            let content_viewport = console_rect.shrink(crate::ui_ext::SPACE_SM);
            let scroll_point = content_viewport.center();
            let _ = render(vec![
                egui::Event::PointerMoved(scroll_point),
                egui::Event::Scroll(egui::vec2(-100_000.0, 0.0)),
            ]);
            let mut scrolled = None;
            for _ in 0..24 {
                scrolled = Some(render(Vec::new()));
            }
            let scrolled = scrolled.unwrap();
            let (scrolled_pos, scrolled_size, scrolled_viewport) =
                find_line(&scrolled).expect("scrolled ANSI line should retain its complete galley");
            let scrolled_console_rect =
                find_console_rect(&scrolled).expect("scrolled console surface should render");
            let scrolled_content_viewport = scrolled_console_rect.shrink(crate::ui_ext::SPACE_SM);
            assert_eq!(scrolled_size, initial_size);
            assert!(
                scrolled_pos.x < initial_pos.x - viewport.width(),
                "horizontal pan should advance across the long line: {initial_pos:?} -> {scrolled_pos:?}"
            );
            let glyph_width = scrolled_size.x / plain.chars().count() as f32;
            let marker_rect = egui::Rect::from_min_size(
                egui::pos2(
                    scrolled_pos.x + marker_start as f32 * glyph_width,
                    scrolled_pos.y,
                ),
                egui::vec2(marker.chars().count() as f32 * glyph_width, scrolled_size.y),
            );
            assert!(
                scrolled_viewport.contains_rect(marker_rect)
                    && scrolled_content_viewport.contains_rect(marker_rect),
                "{appearance:?}: far ANSI marker should be brought fully into the console viewport: {marker_rect:?} in {scrolled_content_viewport:?}"
            );
            assert_eq!(
                find_line(&scrolled).unwrap().1.x,
                initial_size.x,
                "long ANSI line must not be elided after scrolling"
            );
        }
    }

    #[test]
    fn external_handoff_preserves_clipboard_bytes_and_extension() {
        let target = std::sync::Arc::new(DiffTarget::ClipboardText {
            label: "fixture".into(),
            content: "exact\nclipboard bytes\n".into(),
            ext_hint: Some("md".into()),
        });
        let path = DiffHandoffTarget::from_target(&target, "lhs")
            .stage()
            .unwrap();
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("md"));
        assert_eq!(std::fs::read(&path).unwrap(), b"exact\nclipboard bytes\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn external_handoff_reports_temp_write_failure() {
        let target = std::sync::Arc::new(DiffTarget::ClipboardText {
            label: "fixture".into(),
            content: "data".into(),
            ext_hint: Some("missing-parent/file".into()),
        });
        assert!(
            DiffHandoffTarget::from_target(&target, "lhs")
                .stage()
                .unwrap_err()
                .contains("Failed to stage diff lhs file")
        );
    }
}
