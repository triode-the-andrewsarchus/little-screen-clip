//! Rich interactive image workbench and manipulation suite.
//!
//! Provides pan/zoom, cropping with fixed aspect ratios, pixelation/blackout redaction,
//! eyedropper color inspection, resizing, palette extraction, background removal, and 3D depth map generation.

mod save;
mod tools;
use tools::*;
pub(crate) use tools::{
    ImageToolProcessControl, clear_image_tool_pid_if_owned, spawn_image_tool_child,
    terminate_image_tool_process,
};

use crate::icons::Icon;
use crate::{contrast_text_c32, hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use image::RgbaImage;
#[cfg(test)]
use little_clip_core::ClipItem;
use little_clip_core::palette::AppPalette;
use little_clip_core::{
    AppConfig, BackgroundJobs, CommandRequest, HandSide, IpcRequest, JobError, JobTicket,
};
use little_clip_ops::barcode::BarcodeDetection;
use little_clip_ops::creative_fx::{
    CmykHalftoneParams, CreativeFxMaskSource, CreativeFxParams, CreativeFxType, DepthMaskConfig,
    DitherMethod, DitherPaletteParams, LensAberrationParams, NtscChromaLowpass, NtscDemodulation,
    NtscParams, NtscPhaseShift, PaperTint, PixelSortCriteria, PixelSortDirection, PixelSortOrder,
    PixelSortParams, RetroPalette, StructureParams, TrichromaticMode, TrichromaticParams,
    VhsTapeSpeed, apply_creative_fx, apply_creative_fx_with_masks_cancellable,
    default_preset_for_fx, presets_for_fx, trichromatic_intersection_box,
};
use little_clip_ops::decorrelation_stretch::{
    DecorrelationMatrixMode, DecorrelationStats, DecorrelationStretchParams,
    DecorrelationTransform, TargetValue, auto_stride, run_decorrelation_stretch_cancellable,
};
use little_clip_ops::metadata::{ImageMetadataSummary, extract_image_metadata, strip_metadata};
use little_clip_ops::perspective::{DeskewCropMode, QuadCorners, rectify_perspective_with_mode};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

#[derive(Clone)]
struct BgRemovalRequest {
    snapshot: DisplayImageSnapshotRequest,
    prompt: Option<String>,
    command: String,
    args: Vec<String>,
    operation: BgRemovalOperation,
    initial_progress: String,
    status: String,
}

// filmr physics-based film simulation engine
use filmr::film::FilmStyle;
use filmr::light_leak::{LightLeak, LightLeakConfig, LightLeakShape};
use filmr::processor::{OutputMode, SimulationConfig, WhiteBalanceMode};

fn image_editor_hatch(
    friendly_label: &str,
    default_command: &str,
    configured_command: &str,
) -> crate::shell::EscapeHatch {
    let executable_name = Path::new(configured_command)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(configured_command);
    let label = if executable_name.eq_ignore_ascii_case(default_command) {
        friendly_label
    } else {
        executable_name
    };
    let availability = if little_clip_core::command_available(configured_command).is_some() {
        format!("Configured executable: {configured_command}.")
    } else {
        format!(
            "{configured_command} is unavailable; opening falls back to the system default viewer."
        )
    };

    crate::shell::EscapeHatch::new(
        label,
        configured_command,
        format!("Open the current image with {configured_command}. {availability}"),
    )
}

pub struct ImagePreviewState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub mime: String,
    pub file_size_bytes: usize,
    pub is_svg: bool,
    pub raw_bytes: Arc<Vec<u8>>,

    // Committed revisions are shared until an in-place edit calls Arc::make_mut.
    pub original_rgba: Arc<RgbaImage>,
    pub current_rgba: Arc<RgbaImage>,
    /// Increases whenever the committed pixel buffer changes.
    pub(crate) pixel_revision: u64,
    texture: Option<egui::TextureHandle>,
    dirty_texture: bool,

    // Undo / Redo Stack
    pub undo_stack: Vec<Arc<RgbaImage>>,
    pub redo_stack: Vec<Arc<RgbaImage>>,

    // Cropping
    pub crop_state: CropState,

    // Redaction State
    pub redaction_state: RedactionState,

    // Screenshot Markup State
    pub markup_state: MarkupState,

    // Pick Color State
    pub pick_color_state: PickColorState,

    // Sizing & Optimization State
    pub resize_state: ResizeState,
    pub(crate) pixel_resize_job: PixelResizeJobState,

    // Palette State
    pub palette_state: PaletteState,
    pub(crate) palette_ticket: Option<JobTicket>,
    pub(crate) palette_rx: Option<
        std::sync::mpsc::Receiver<Result<(Vec<PaletteColor>, usize), little_clip_core::JobError>>,
    >,
    pub(crate) palette_request_stamp: Option<ImageInputStamp>,
    pub(crate) palette_request_count: Option<usize>,
    pub(crate) palette_results_stamp: Option<ImageInputStamp>,
    pub(crate) palette_results_count: Option<usize>,

    // Viewport Navigation
    pub pan: Vec2,
    pub zoom: f32,
    pub auto_fitted: bool,

    // Active Tool Mode
    pub active_mode: ImageToolMode,
    pub panel_state: crate::shell::PanelState,

    // Configured External Programs
    pub external_editor: String,
    pub svg_editor: String,
    pub viewer: String,

    // Status Toast (message, timestamp)
    pub status_toast: Option<(String, f64)>,

    // File Dialog & Save-As
    save_job: Option<save::ImageSaveJob>,
    copy_job: Option<save::ImageCopyJob>,
    copy_requested: Option<HandSide>,
    rendering_frame: bool,
    pub show_fallback_save_modal: bool,
    fallback_save_needs_focus: bool,
    fallback_save_confirm_overwrite: bool,
    pub fallback_save_path: String,

    // File Size Optimizer Async Worker
    pub optimizer_rx: Option<std::sync::mpsc::Receiver<OptimizerMsg>>,
    optimizer_job_rx: Option<std::sync::mpsc::Receiver<Result<(), little_clip_core::JobError>>>,
    optimizer_ticket: Option<JobTicket>,
    pub(crate) optimizer_retry_at: Option<std::time::Instant>,
    optimizer_retry_stamp: Option<OptimizerRequestStamp>,
    optimizer_retry_snapshot: Option<DisplayImageSnapshotRequest>,
    pub optimizer_status: Option<String>,
    optimizer_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,

    // Background Removal Async Worker & State
    pub bg_removal_in_progress: bool,
    pub bg_removal_rx: Option<std::sync::mpsc::Receiver<BgRemovalMsg>>,
    bg_removal_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    bg_removal_ticket: Option<JobTicket>,
    bg_removal_request: Option<BgRemovalRequest>,
    pub(crate) bg_removal_retry_at: Option<std::time::Instant>,
    pub(crate) bg_removal_input_stamp: Option<ImageInputStamp>,
    bg_removal_operation: BgRemovalOperation,
    pub bg_removal_status: Option<String>,
    pub bg_removal_error: Option<String>,
    pub bg_removal_cmd: Option<String>,
    pub(crate) image_tool_child_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub(crate) image_tool_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub multimatte_prompt: String,

    // Format Conversion (Squoosh-style)
    pub convert_state: ConvertState,

    // Multi-Model Document OCR
    pub ocr_state: OcrState,
    pub ocr_rx: Option<std::sync::mpsc::Receiver<OcrMsg>>,
    ocr_job_rx: Option<std::sync::mpsc::Receiver<Result<(), little_clip_core::JobError>>>,
    ocr_ticket: Option<JobTicket>,
    ocr_request: Option<OcrRequest>,
    pub(crate) ocr_retry_at: Option<std::time::Instant>,
    pub(crate) ocr_input_stamp: Option<ImageInputStamp>,
    pub(crate) ocr_results_stamp: Option<ImageInputStamp>,
    pub(crate) ocr_process_control: Option<std::sync::Arc<ImageToolProcessControl>>,

    // Barcode / QR / DataMatrix Decoder
    pub barcode_state: BarcodeScanState,
    pub barcode_rx:
        Option<std::sync::mpsc::Receiver<Result<BarcodeScanMsg, little_clip_core::JobError>>>,
    pub(crate) barcode_cancel: Option<std::sync::Arc<ImageToolProcessControl>>,
    pub(crate) barcode_ticket: Option<JobTicket>,
    pub(crate) barcode_input_stamp: Option<ImageInputStamp>,
    pub(crate) barcode_results_stamp: Option<ImageInputStamp>,
    pub(crate) barcode_retry_at: Option<std::time::Instant>,
    pub(crate) barcode_retry_stamp: Option<ImageInputStamp>,
    pub(crate) barcode_retry_request: Option<BarcodeScanRequest>,
    pub(crate) barcode_retry_snapshot: Option<DisplayImageSnapshotRequest>,
    pub(crate) barcode_queue_status: Option<String>,

    // Decorrelation Stretch State
    pub dcs_state: DecorrelationStretchState,

    // Film Simulation State
    pub film_sim_state: FilmSimState,

    // Creative FX State
    pub creative_fx_state: CreativeFxState,

    // EXIF & Image Metadata Inspector
    pub metadata: Option<ImageMetadataSummary>,
    pub show_metadata_drawer: bool,

    // 4-Point Perspective Deskew
    pub deskew_state: DeskewState,
}

/// Immutable encoded and decoded image content shared by one or more image views.
/// Viewer identity, editing, navigation, and texture state stay outside this value.
#[derive(Clone)]
pub(crate) struct DecodedImageData {
    mime: String,
    encoded_bytes: Arc<Vec<u8>>,
    original_rgba: Arc<RgbaImage>,
    is_svg: bool,
    metadata: Option<ImageMetadataSummary>,
}

impl DecodedImageData {
    pub(crate) fn from_path(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("Failed to read file {}: {error}", path.display()))?;
        let mime = little_clip_core::item::guess_image_mime(&path);
        Self::from_bytes(bytes, mime, Some(path.to_path_buf()))
    }

    pub(crate) fn from_bytes(
        bytes: Vec<u8>,
        mime: String,
        path: Option<PathBuf>,
    ) -> Result<Self, String> {
        let is_svg = mime.contains("svg")
            || little_clip_core::item::is_svg_bytes(&bytes)
            || path
                .as_ref()
                .and_then(|path| path.extension())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"));
        let rgba = if is_svg {
            if let Some(png_bytes) = render_svg_to_png(&bytes) {
                image::load_from_memory(&png_bytes)
                    .map_err(|error| format!("Failed to decode rendered SVG PNG: {error}"))?
                    .to_rgba8()
            } else {
                return Err("Failed to rasterize SVG via rsvg-convert. Ensure librsvg / rsvg-convert is installed.".to_string());
            }
        } else {
            image::load_from_memory(&bytes)
                .map_err(|error| format!("Failed to decode image bytes: {error}"))?
                .to_rgba8()
        };

        let metadata = extract_image_metadata(&bytes);
        Ok(Self {
            mime,
            encoded_bytes: Arc::new(bytes),
            original_rgba: Arc::new(rgba),
            is_svg,
            metadata: metadata.has_any_metadata().then_some(metadata),
        })
    }
}

/// Lightweight image renderer for hosts that need a preview without the
/// editor's mutation, history, or background-job state.
pub(crate) struct InlineImagePreview {
    source: Arc<DecodedImageData>,
    texture: Option<egui::TextureHandle>,
    zoom: f32,
    pan: Vec2,
    drag_origin_pan: Vec2,
}

impl InlineImagePreview {
    pub(crate) fn new(source: Arc<DecodedImageData>) -> Self {
        Self {
            source,
            texture: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            drag_origin_pan: Vec2::ZERO,
        }
    }

    /// Render inside the space assigned by the host. Scroll zooms and dragging
    /// pans this view only; the decoded source remains immutable and shared.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, size: Vec2) -> egui::Response {
        let size = size.max(Vec2::splat(1.0));
        let (viewport, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        if !ui.is_rect_visible(viewport) {
            return response;
        }

        if self.texture.is_none() {
            let pixels = self.source.original_rgba.as_ref();
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [pixels.width() as usize, pixels.height() as usize],
                pixels.as_raw(),
            );
            self.texture = Some(ui.ctx().load_texture(
                "inline_image_preview",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }

        if response.hovered() {
            let scroll_y = ui.input(|input| input.raw_scroll_delta.y);
            if scroll_y != 0.0 {
                self.zoom = (self.zoom * (scroll_y / 240.0).exp()).clamp(0.25, 8.0);
                ui.ctx().request_repaint();
            }
        }
        if response.double_clicked() {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        if response.drag_started() {
            self.drag_origin_pan = self.pan;
        }
        if response.dragged() {
            self.pan = self.drag_origin_pan + response.drag_delta();
        }

        let pixels = self.source.original_rgba.as_ref();
        let fit = (viewport.width() / pixels.width() as f32)
            .min(viewport.height() / pixels.height() as f32);
        let image_size =
            Vec2::new(pixels.width() as f32, pixels.height() as f32) * (fit * self.zoom);
        let image_rect = egui::Rect::from_center_size(viewport.center() + self.pan, image_size);
        if let Some(texture) = &self.texture {
            ui.painter().with_clip_rect(viewport).image(
                texture.id(),
                image_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        response
    }
}

fn compose_display_image(mut img: RgbaImage, stamp: &ImageInputStamp) -> RgbaImage {
    // Burn any pending redactions onto the output image
    if !stamp.redactions.is_empty() {
        let img_w = img.width() as f32;
        let img_h = img.height() as f32;
        for b in &stamp.redactions {
            let px_min_x = (b.normalized_rect.min.x * img_w).round().clamp(0.0, img_w) as u32;
            let px_max_x = (b.normalized_rect.max.x * img_w).round().clamp(0.0, img_w) as u32;
            let px_min_y = (b.normalized_rect.min.y * img_h).round().clamp(0.0, img_h) as u32;
            let px_max_y = (b.normalized_rect.max.y * img_h).round().clamp(0.0, img_h) as u32;
            for y in px_min_y..px_max_y {
                for x in px_min_x..px_max_x {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
    }

    // Burn any pending vector markup layers onto the output image
    if !stamp.markup.is_empty() {
        img = rasterize_markup(&img, &stamp.markup);
    }

    if let Some(norm) = stamp.crop {
        let img_w = img.width() as f32;
        let img_h = img.height() as f32;

        let px_x = (norm.min.x * img_w).round().clamp(0.0, img_w - 1.0) as u32;
        let px_y = (norm.min.y * img_h).round().clamp(0.0, img_h - 1.0) as u32;
        let px_w = ((norm.max.x - norm.min.x) * img_w)
            .round()
            .clamp(1.0, img_w - px_x as f32) as u32;
        let px_h = ((norm.max.y - norm.min.y) * img_h)
            .round()
            .clamp(1.0, img_h - px_y as f32) as u32;

        if px_w > 0
            && px_h > 0
            && (px_w < img.width() || px_h < img.height() || px_x > 0 || px_y > 0)
        {
            return image::imageops::crop_imm(&img, px_x, px_y, px_w, px_h).to_image();
        }
    }

    img
}

/// Identity of the image pixels and overlays consumed by `current_display_image`.
/// This intentionally snapshots only small edit state, never the full pixel buffer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImageInputStamp {
    pixel_revision: u64,
    redactions: Vec<RedactionBox>,
    markup: Vec<MarkupShape>,
    crop: Option<Rect>,
}

/// Cheap immutable display-image request captured before submitting worker work.
/// Cloning this request shares the source pixels; composition is deferred to the worker.
#[derive(Clone)]
pub(crate) struct DisplayImageSnapshotRequest {
    pub(crate) pixels: Arc<RgbaImage>,
    pub(crate) input_stamp: ImageInputStamp,
}

impl DisplayImageSnapshotRequest {
    /// Compose redactions, markup, and the captured crop selection on the calling worker.
    pub(crate) fn compose(&self) -> RgbaImage {
        compose_display_image(self.pixels.as_ref().clone(), &self.input_stamp)
    }

    pub(crate) fn compose_png(&self) -> image::ImageResult<Vec<u8>> {
        let mut png_bytes = Vec::new();
        self.compose().write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )?;
        Ok(png_bytes)
    }
}

impl ImagePreviewState {
    /// Polls snapshot workers while this editor is hidden in the session cache.
    /// Returns true while a save or copy still needs a later poll.
    pub(crate) fn poll_cached_jobs(&mut self) -> bool {
        self.poll_save_job();
        self.poll_copy_job();
        self.save_job.is_some() || self.copy_job.is_some()
    }

    /// Restore worker-owned pixel snapshots before moving the editor out of the UI.
    pub(crate) fn poll_session_snapshots(&mut self) -> bool {
        self.poll_cached_jobs();
        !self.save_snapshot_pending() && !self.copy_snapshot_pending()
    }

    #[cfg(test)]
    pub fn from_path(path: PathBuf, config: &AppConfig) -> Result<Self, String> {
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Image Preview".to_string());
        let source = DecodedImageData::from_path(&path)?;
        Ok(Self::from_decoded(source, Some(path), title, config))
    }

    #[cfg(test)]
    pub fn from_item(item: &ClipItem, config: &AppConfig) -> Result<Self, String> {
        if let Some((bytes, mime, path)) = item.resolve_image_data() {
            let title = if item.title.is_empty() {
                path.as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Image Preview".to_string())
            } else {
                item.title.clone()
            };
            Self::from_bytes_with_meta(bytes, mime, path, title, config)
        } else if let Some(p) = item.image_path() {
            Self::from_path(p, config)
        } else {
            Err("Item payload does not contain a recognizable image".to_string())
        }
    }

    #[cfg(test)]
    pub fn from_bytes_with_meta(
        bytes: Vec<u8>,
        mime: String,
        path: Option<PathBuf>,
        title: String,
        config: &AppConfig,
    ) -> Result<Self, String> {
        let source = DecodedImageData::from_bytes(bytes, mime, path.clone())?;
        Ok(Self::from_decoded(source, path, title, config))
    }

    pub(crate) fn from_decoded(
        source: DecodedImageData,
        path: Option<PathBuf>,
        title: String,
        config: &AppConfig,
    ) -> Self {
        let DecodedImageData {
            mime,
            encoded_bytes,
            original_rgba,
            is_svg,
            metadata,
        } = source;
        let file_size_bytes = encoded_bytes.len();
        let mut resize_state = ResizeState::default();
        resize_state.sync_from_image(&original_rgba);
        let palette_state = PaletteState::default();
        Self {
            path,
            title,
            mime,
            file_size_bytes,
            is_svg,
            raw_bytes: encoded_bytes,
            original_rgba: original_rgba.clone(),
            current_rgba: original_rgba,
            pixel_revision: 0,
            texture: None,
            dirty_texture: true,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            crop_state: CropState::default(),
            redaction_state: RedactionState::default(),
            markup_state: MarkupState::default(),
            pick_color_state: PickColorState::default(),
            resize_state,
            pixel_resize_job: PixelResizeJobState::default(),
            palette_state,
            palette_ticket: None,
            palette_rx: None,
            palette_request_stamp: None,
            palette_request_count: None,
            palette_results_stamp: None,
            palette_results_count: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            auto_fitted: false,
            active_mode: ImageToolMode::View,
            panel_state: crate::shell::PanelState {
                bottom_height: 160.0,
                ..Default::default()
            },
            external_editor: config.image.external_editor.clone(),
            svg_editor: config.image.svg_editor.clone(),
            viewer: config.image.viewer.clone(),
            status_toast: None,
            save_job: None,
            copy_job: None,
            copy_requested: None,
            rendering_frame: false,
            show_fallback_save_modal: false,
            fallback_save_needs_focus: false,
            fallback_save_confirm_overwrite: false,
            fallback_save_path: String::new(),
            optimizer_rx: None,
            optimizer_job_rx: None,
            optimizer_ticket: None,
            optimizer_retry_at: None,
            optimizer_retry_stamp: None,
            optimizer_retry_snapshot: None,
            optimizer_status: None,
            optimizer_cancel: None,
            bg_removal_in_progress: false,
            bg_removal_rx: None,
            bg_removal_job_rx: None,
            bg_removal_ticket: None,
            bg_removal_request: None,
            bg_removal_retry_at: None,
            bg_removal_input_stamp: None,
            bg_removal_operation: BgRemovalOperation::BackgroundRemoval,
            bg_removal_status: None,
            bg_removal_error: None,
            bg_removal_cmd: config.image.bg_removal_cmd.clone(),
            image_tool_child_pid: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            image_tool_cancel: None,
            multimatte_prompt: String::new(),
            convert_state: ConvertState::default(),
            ocr_state: OcrState::default(),
            ocr_rx: None,
            ocr_job_rx: None,
            ocr_ticket: None,
            ocr_request: None,
            ocr_retry_at: None,
            ocr_input_stamp: None,
            ocr_results_stamp: None,
            ocr_process_control: None,
            barcode_state: BarcodeScanState::default(),
            barcode_rx: None,
            barcode_cancel: None,
            barcode_ticket: None,
            barcode_input_stamp: None,
            barcode_results_stamp: None,
            barcode_retry_at: None,
            barcode_retry_stamp: None,
            barcode_retry_request: None,
            barcode_retry_snapshot: None,
            barcode_queue_status: None,
            dcs_state: DecorrelationStretchState::default(),
            film_sim_state: FilmSimState::default(),
            creative_fx_state: CreativeFxState::default(),
            metadata,
            show_metadata_drawer: false,
            deskew_state: DeskewState::default(),
        }
    }

    pub fn show_toast(&mut self, msg: impl Into<String>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.status_toast = Some((msg.into(), now));
    }

    pub(crate) fn input_stamp(&self) -> ImageInputStamp {
        ImageInputStamp {
            pixel_revision: self.pixel_revision,
            redactions: self.redaction_state.boxes.clone(),
            markup: self.markup_state.items.clone(),
            crop: (self.active_mode == ImageToolMode::Crop)
                .then_some(self.crop_state.normalized_rect),
        }
    }

    pub(crate) fn display_image_snapshot_request(&self) -> DisplayImageSnapshotRequest {
        DisplayImageSnapshotRequest {
            pixels: self.current_rgba.clone(),
            input_stamp: self.input_stamp(),
        }
    }

    pub(crate) fn mark_pixels_changed(&mut self) {
        self.cancel_pixel_resize();
        self.cancel_barcode_scan();
        self.invalidate_decorrelation_stretch();
        self.film_sim_state.cancel_render();
        self.creative_fx_state.cancel_render();
        self.cancel_optimizer();
        self.cancel_optipng_process();
        self.film_sim_state.cancel_thumbnail_batch();
        self.cancel_creative_fx_thumbnail_batch();
        self.film_sim_state.stock_thumbnails.clear();
        self.film_sim_state.pending_thumbnails.clear();
        self.creative_fx_state.fx_thumbnails.clear();
        self.creative_fx_state.pending_thumbnails.clear();
        self.creative_fx_state.thumbnail_pixel_revision = None;
        self.pixel_revision = self.pixel_revision.wrapping_add(1);
    }

    pub(crate) fn ocr_output_is_current(&self) -> bool {
        self.ocr_results_stamp.as_ref() == Some(&self.input_stamp())
            && !self.ocr_state.extracted_text.is_empty()
    }

    /// Returns the image as currently displayed/composed, taking into account any active crop selection if in Crop mode,
    /// and burning any placed redaction bars so exports/copies never leak private information.
    #[cfg(test)]
    pub fn current_display_image(&self) -> RgbaImage {
        self.display_image_snapshot_request().compose()
    }

    /// Burns all pending redaction black bars permanently into current_rgba and records an undo point.
    pub fn strip_metadata(&mut self) {
        match strip_metadata(&self.current_rgba, &self.mime) {
            Ok(clean_bytes) => {
                self.undo_stack.push(self.current_rgba.clone());
                self.redo_stack.clear();
                self.raw_bytes = Arc::new(clean_bytes);
                self.file_size_bytes = self.raw_bytes.len();
                self.metadata = None;
                self.show_toast("✔ Stripped all EXIF & location metadata");
            }
            Err(e) => {
                self.show_toast(format!("⚠ Failed to strip metadata: {}", e));
            }
        }
    }

    /// Rectifies perspective based on the 4 quad corners in deskew_state.
    pub fn copy_to_hand_slot(&mut self, hand: HandSide) {
        if self.rendering_frame {
            self.copy_requested = Some(hand);
        } else {
            self.start_copy_to_hand_slot(hand, little_clip_core::BackgroundJobs::global());
        }
    }

    /// Copies the currently displayed image to the Left Hand clipboard slot via daemon IPC,
    /// and simultaneously syncs to the OS clipboard through the shared adapter.
    pub fn copy_to_clipboard_slot(&mut self) {
        self.copy_to_hand_slot(HandSide::Left);
    }

    /// Runs AI background removal asynchronously using feyninc/nobg (FeyNoBg).
    pub fn open_save_dialog(&mut self) {
        let default_target = if let Some(p) = &self.path {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let parent = p.parent().unwrap_or_else(|| Path::new("."));
            if self.undo_stack.is_empty() {
                parent.join(format!("{}.{}", stem, ext))
            } else {
                parent.join(format!("{}_edit.{}", stem, ext))
            }
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let pic_dir = PathBuf::from(home).join("Pictures");
            if pic_dir.exists() {
                pic_dir.join("edited_clip.png")
            } else {
                PathBuf::from("edited_clip.png")
            }
        };

        self.fallback_save_path = default_target.to_string_lossy().to_string();
        self.show_fallback_save_modal = true;
        self.fallback_save_needs_focus = true;
        self.fallback_save_confirm_overwrite = false;
    }

    /// Opens a save dialog targeting a specific extension (e.g. "png", "jpg", "webp").
    pub fn open_save_dialog_with_ext(&mut self, target_ext: &str) {
        let default_target = if let Some(p) = &self.path {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
            let parent = p.parent().unwrap_or_else(|| Path::new("."));
            parent.join(format!("{}_converted.{}", stem, target_ext))
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let pic_dir = PathBuf::from(home).join("Pictures");
            if pic_dir.exists() {
                pic_dir.join(format!("converted_clip.{}", target_ext))
            } else {
                PathBuf::from(format!("converted_clip.{}", target_ext))
            }
        };

        self.fallback_save_path = default_target.to_string_lossy().to_string();
        self.show_fallback_save_modal = true;
        self.fallback_save_needs_focus = true;
        self.fallback_save_confirm_overwrite = false;
    }

    // --- Transform & Crop Actions ---

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.current_rgba.clone());
            self.current_rgba = prev;
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("⟲ Undone");
        } else {
            self.show_toast("Nothing to undo");
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.current_rgba.clone());
            self.current_rgba = next;
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("⟳ Redone");
        }
    }

    pub fn revert_to_original(&mut self) {
        if !self.undo_stack.is_empty() {
            self.undo_stack.push(self.current_rgba.clone());
            self.current_rgba = self.original_rgba.clone();
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("↺ Reverted to original image");
        }
    }

    /// Ensure the file exists on disk so external programs can open it.
    /// If image was modified, saves modified buffer to disk.
    pub fn ensure_disk_path(&self) -> Result<PathBuf, String> {
        let staging_dir = little_clip_core::staging::get_staging_dir()
            .map_err(|e| format!("Could not create image staging directory: {e}"))?;

        let is_modified = !self.undo_stack.is_empty();
        if !is_modified {
            if let Some(ref p) = self.path {
                if p.exists() {
                    return Ok(p.clone());
                }
            }
        }

        let ext = if self.is_svg && !is_modified {
            "svg"
        } else if self.mime.contains("jpeg") || self.mime.contains("jpg") {
            "jpg"
        } else if self.mime.contains("webp") {
            "webp"
        } else {
            "png"
        };

        let staging_path = staging_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
        if is_modified || self.raw_bytes.is_empty() {
            self.current_rgba
                .save(&staging_path)
                .map_err(|e| format!("Failed to save modified staging file: {}", e))?;
        } else {
            std::fs::write(&staging_path, self.raw_bytes.as_slice())
                .map_err(|e| format!("Failed to write staging file: {}", e))?;
        }

        Ok(staging_path)
    }

    pub fn launch_external_app(&mut self, app: &str, _ctx: &egui::Context, _close: &mut bool) {
        self.launch_external_app_with(app, |app_name, command, path| {
            crate::shell::EscapeHatchLauncher::spawn_with_fallback(app_name, command, path)
        });
    }

    fn launch_external_app_with(
        &mut self,
        app: &str,
        launch: impl FnOnce(&str, &str, &std::path::Path) -> Result<String, String>,
    ) {
        let file_path = self.ensure_disk_path();
        self.launch_external_app_with_path(app, file_path, launch);
    }

    fn launch_external_app_with_path(
        &mut self,
        app: &str,
        file_path: Result<PathBuf, String>,
        launch: impl FnOnce(&str, &str, &std::path::Path) -> Result<String, String>,
    ) {
        match file_path {
            Ok(file_path) => {
                match launch(app, app, &file_path) {
                    Ok(msg) => {
                        self.show_toast(msg);
                        // Workbench remains open so the user does not lose their session
                    }
                    Err(e) => {
                        self.show_toast(e);
                    }
                }
            }
            Err(e) => {
                self.show_toast(format!("⚠️ File error: {}", e));
            }
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        self.poll_save_job();
        if self.save_snapshot_pending() && self.render_save_progress(ui, ctx, close) {
            return;
        }
        self.poll_copy_job();
        if self.copy_snapshot_pending() {
            self.render_copy_progress(ui, ctx, close);
            return;
        }
        if self.copy_delivery_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.rendering_frame = true;

        // Upload GPU texture if dirty or not loaded
        if self.texture.is_none() || self.dirty_texture {
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [
                    self.current_rgba.width() as usize,
                    self.current_rgba.height() as usize,
                ],
                self.current_rgba.as_raw(),
            );
            self.texture = Some(ctx.load_texture(
                "image_preview_tex",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
            self.dirty_texture = false;
        }

        // Keep ownership for this whole frame, even if Save or Cancel closes the dialog.
        let save_modal_active = self.show_fallback_save_modal;
        if save_modal_active {
            egui::Area::new(egui::Id::new("image_save_backdrop"))
                .order(egui::Order::Middle)
                .fixed_pos(ctx.screen_rect().min)
                .show(ctx, |ui| {
                    let rect = ctx.screen_rect();
                    ui.allocate_rect(rect, egui::Sense::click());
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        hex_to_c32_alpha(&palette.colors.canvas, 180),
                    );
                });
            egui::Window::new("Save Image As")
                .collapsible(false)
                .resizable(false)
                .movable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.label(egui::RichText::new("Enter destination file path:").strong());
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    let path_field = ui.add_sized(
                        [
                            ui.available_width().clamp(260.0, 440.0),
                            crate::ui_ext::CONTROL_HEIGHT,
                        ],
                        egui::TextEdit::singleline(&mut self.fallback_save_path),
                    );
                    if self.fallback_save_needs_focus {
                        path_field.request_focus();
                        self.fallback_save_needs_focus = false;
                    }
                    if self.fallback_save_confirm_overwrite {
                        ui.label(
                            egui::RichText::new("A file at this path already exists. Replace it?")
                                .color(hex_to_c32(&palette.harmonies.warm_attention)),
                        );
                    }
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.horizontal(|ui| {
                        let save_label = if self.fallback_save_confirm_overwrite {
                            "Replace"
                        } else {
                            "Save"
                        };
                        let save_color = if self.fallback_save_confirm_overwrite {
                            hex_to_c32(&palette.harmonies.warm_attention)
                        } else {
                            hex_to_c32(&palette.diff_add)
                        };
                        if ui
                            .add(crate::ui_ext::action_button_icon(
                                Some(Icon::Save),
                                save_label,
                                save_color,
                                palette,
                            ))
                            .clicked()
                        {
                            let p = PathBuf::from(&self.fallback_save_path);
                            if self.fallback_save_confirm_overwrite {
                                self.save_image_to_path(&p);
                                self.show_fallback_save_modal = false;
                                self.fallback_save_needs_focus = false;
                                self.fallback_save_confirm_overwrite = false;
                            } else if p.exists() {
                                self.fallback_save_confirm_overwrite = true;
                            } else {
                                self.save_image_to_path(&p);
                                self.show_fallback_save_modal = false;
                                self.fallback_save_needs_focus = false;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            if self.fallback_save_confirm_overwrite {
                                self.fallback_save_confirm_overwrite = false;
                            } else {
                                self.show_fallback_save_modal = false;
                                self.fallback_save_needs_focus = false;
                            }
                        }
                    });
                });
        }

        if self.render_save_progress(ui, ctx, close) {
            return;
        }

        if save_modal_active && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_fallback_save_modal = false;
            self.fallback_save_needs_focus = false;
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        }
        let image_shortcuts_enabled = !save_modal_active && !ctx.wants_keyboard_input();

        // Global hotkey handling
        if image_shortcuts_enabled && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.ocr_state.in_progress {
                self.cancel_ocr();
            } else if self.active_mode == ImageToolMode::Crop
                && self.crop_state.normalized_rect
                    != Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0))
            {
                self.crop_state.normalized_rect =
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                self.crop_state.ratio = CropRatio::Free;
            } else if self.active_mode == ImageToolMode::Deskew
                && self.deskew_state.is_clicking_points
            {
                self.deskew_state.cancel_clicking_points();
                self.show_toast("Cancelled 4-point placement");
            } else if self.active_mode == ImageToolMode::Redact
                && self.redaction_state.selected_idx.is_some()
            {
                self.redaction_state.selected_idx = None;
            } else if self.active_mode == ImageToolMode::Redact
                && !self.redaction_state.boxes.is_empty()
            {
                self.clear_all_redactions();
            } else if self.active_mode == ImageToolMode::Markup
                && self.markup_state.editing_text_idx.is_some()
            {
                self.markup_state.editing_text_idx = None;
            } else if self.active_mode == ImageToolMode::Markup
                && self.markup_state.selected_idx.is_some()
            {
                self.markup_state.selected_idx = None;
            } else {
                if self.ocr_state.in_progress {
                    self.cancel_ocr();
                }
                *close = true;
                return;
            }
        }
        if image_shortcuts_enabled
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z))
        {
            if ui.input(|i| i.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }
        if image_shortcuts_enabled
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y))
        {
            self.redo();
        }
        let enter_pressed = !save_modal_active && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let enter_from_field = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        if enter_pressed && (image_shortcuts_enabled || enter_from_field) {
            match self.active_mode {
                ImageToolMode::Crop if image_shortcuts_enabled => self.apply_crop(),
                ImageToolMode::Deskew if image_shortcuts_enabled => self.apply_deskew(),
                ImageToolMode::Redact if image_shortcuts_enabled => self.apply_redactions(),
                ImageToolMode::Markup
                    if image_shortcuts_enabled && self.markup_state.editing_text_idx.is_none() =>
                {
                    self.apply_flatten_markup();
                }
                ImageToolMode::Resize
                    if image_shortcuts_enabled
                        && self.resize_state.sub_tab == ResizeSubTab::Pixels =>
                {
                    self.apply_pixel_resize(
                        self.resize_state.target_width,
                        self.resize_state.target_height,
                        self.resize_state.filter,
                    );
                }
                ImageToolMode::RemoveBg => self.run_background_removal(),
                ImageToolMode::DepthMap if image_shortcuts_enabled => self.run_depth_estimation(),
                ImageToolMode::Convert
                    if image_shortcuts_enabled
                        && self.convert_state.output_matches(&self.input_stamp()) =>
                {
                    self.apply_converted_to_canvas();
                }
                ImageToolMode::Ocr => self.run_ocr(),
                ImageToolMode::Barcode if image_shortcuts_enabled => self.run_barcode_scan(),
                ImageToolMode::DecorrelationStretch if image_shortcuts_enabled => {
                    self.apply_decorrelation_stretch();
                }
                _ => {}
            }
        }
        if image_shortcuts_enabled {
            if self.active_mode == ImageToolMode::Redact {
                let del_pressed = ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                }) || ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                });
                if del_pressed {
                    self.delete_selected_redaction();
                }
            }
            if self.active_mode == ImageToolMode::Markup
                && self.markup_state.editing_text_idx.is_none()
            {
                let del_pressed = ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                }) || ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                });
                if del_pressed {
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            self.markup_state.items.remove(sel);
                            self.markup_state.selected_idx = None;
                            self.markup_state.sync_next_badge_number();
                            self.show_toast("🗑 Markup item deleted");
                        }
                    }
                }
            }
            if self.active_mode == ImageToolMode::Markup
                && self.markup_state.editing_text_idx.is_none()
            {
                let no_modifiers = ui.input(|i| {
                    !i.modifiers.command
                        && !i.modifiers.ctrl
                        && !i.modifiers.alt
                        && !i.modifiers.mac_cmd
                });
                if no_modifiers {
                    if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
                        self.markup_state.active_tool = MarkupToolType::Select;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num2)) {
                        self.markup_state.active_tool = MarkupToolType::Arrow;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num3)) {
                        self.markup_state.active_tool = MarkupToolType::Circle;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num4)) {
                        self.markup_state.active_tool = MarkupToolType::Rectangle;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num5)) {
                        self.markup_state.active_tool = MarkupToolType::Line;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num6)) {
                        self.markup_state.active_tool = MarkupToolType::Highlighter;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num7)) {
                        self.markup_state.active_tool = MarkupToolType::StepBadge;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num8)) {
                        self.markup_state.active_tool = MarkupToolType::Text;
                    }
                }
            } else {
                if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
                    self.zoom = 1.0;
                    self.pan = Vec2::ZERO;
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Num0)) {
                self.fit_to_view(ui.available_size());
            }
            if ui.input(|i| i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus)) {
                self.zoom = (self.zoom * 1.25).clamp(0.05, 5.0);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.zoom = (self.zoom / 1.25).clamp(0.05, 5.0);
            }

            // Unmodified single-key tool selection shortcuts
            let no_modifiers = ui.input(|i| {
                !i.modifiers.command
                    && !i.modifiers.ctrl
                    && !i.modifiers.alt
                    && !i.modifiers.mac_cmd
            });
            if no_modifiers {
                if ui.input(|i| i.key_pressed(egui::Key::V)) {
                    self.set_tool_mode(ImageToolMode::View);
                } else if ui.input(|i| i.key_pressed(egui::Key::C)) {
                    self.set_tool_mode(ImageToolMode::Crop);
                } else if ui.input(|i| i.key_pressed(egui::Key::K)) {
                    self.set_tool_mode(ImageToolMode::Deskew);
                } else if ui.input(|i| i.key_pressed(egui::Key::F)) {
                    self.set_tool_mode(ImageToolMode::Transform);
                } else if ui.input(|i| i.key_pressed(egui::Key::X)) {
                    self.set_tool_mode(ImageToolMode::Redact);
                } else if ui.input(|i| i.key_pressed(egui::Key::M)) {
                    self.set_tool_mode(ImageToolMode::Markup);
                } else if ui.input(|i| i.key_pressed(egui::Key::P)) {
                    self.set_tool_mode(ImageToolMode::PickColor);
                } else if ui.input(|i| i.key_pressed(egui::Key::S)) {
                    self.set_tool_mode(ImageToolMode::Resize);
                } else if ui.input(|i| i.key_pressed(egui::Key::T)) {
                    self.set_tool_mode(ImageToolMode::Convert);
                } else if ui.input(|i| i.key_pressed(egui::Key::O)) {
                    self.set_tool_mode(ImageToolMode::Ocr);
                } else if ui.input(|i| i.key_pressed(egui::Key::B)) {
                    self.set_tool_mode(ImageToolMode::Barcode);
                } else if ui.input(|i| i.key_pressed(egui::Key::R)) {
                    self.set_tool_mode(ImageToolMode::RemoveBg);
                } else if ui.input(|i| i.key_pressed(egui::Key::D)) {
                    self.set_tool_mode(ImageToolMode::DepthMap);
                } else if ui.input(|i| i.key_pressed(egui::Key::L)) {
                    self.set_tool_mode(ImageToolMode::FilmSim);
                }
            }
        }

        // Poll background file size optimizer if running
        self.poll_palette_extraction(ctx);

        self.poll_decorrelation_stretch(ctx, BackgroundJobs::global());
        if self.active_mode == ImageToolMode::DecorrelationStretch {
            if let Some(retry_at) = self.dcs_state.retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else if self.dcs_state.is_computing {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }

        // Poll background file size optimizer if running
        self.poll_optimizer();
        self.poll_pixel_resize(ctx, BackgroundJobs::global());
        if self.pixel_resize_job.pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if let Some(retry_at) = self.pixel_resize_job.retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }
        if let Some(retry_at) = self.optimizer_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }

        // Poll background AI background removal if running
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            if let Some(retry_at) = self.bg_removal_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        }

        // Poll format conversion and debounce costly full-resolution encodes.
        self.poll_conversion();
        if self.active_mode == ImageToolMode::Convert {
            if let Some(dirty_at) = self.convert_state.params_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(180)
                    && !self.convert_state.is_encoding
                {
                    self.run_conversion_worker();
                }
            }
            let retry_at = self
                .convert_state
                .render_retry_at
                .into_iter()
                .chain(self.convert_state.optipng_retry_at)
                .min();
            if let Some(retry_at) = retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else if self.convert_state.is_pending() {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }

        // Poll film sim background render + debounce
        self.poll_film_sim();
        if self.active_mode == ImageToolMode::FilmSim {
            if self
                .film_sim_state
                .render_retry_at
                .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
                && !self.film_sim_state.is_rendering
            {
                self.film_sim_state.render_retry_at = None;
                self.run_film_sim_render();
            }
            if let Some(retry_at) = self.film_sim_state.render_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            }
            // Debounce: fire render 400ms after last parameter change
            if let Some(dirty_at) = self.film_sim_state.params_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(400)
                    && !self.film_sim_state.is_rendering
                {
                    self.film_sim_state.params_dirty_at = None;
                    self.run_film_sim_render();
                }
            }
            // Debounce thumbnail batch: fire 500ms after last parameter change
            if let Some(dirty_at) = self.film_sim_state.thumbnail_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(500)
                    && !self.film_sim_state.is_generating_thumbnails
                {
                    self.film_sim_state.thumbnail_dirty_at = None;
                    self.run_thumbnail_batch();
                }
            }
            self.retry_thumbnail_batch_if_due(BackgroundJobs::global());
            if let Some(retry_at) = self.film_sim_state.thumbnail_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            }
            if self.film_sim_state.is_rendering || self.film_sim_state.is_generating_thumbnails {
                ctx.request_repaint();
            }
        }

        // Poll creative fx background render + debounce
        self.poll_creative_fx();
        if let Some(retry_at) = self.creative_fx_state.thumbnail_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if let Some(retry_at) = self.creative_fx_state.depth_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if let Some(retry_at) = self.creative_fx_state.alpha_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if self.active_mode == ImageToolMode::CreativeFx {
            if let Some(retry_at) = self.creative_fx_state.render_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            }
            if let Some(dirty_at) = self.creative_fx_state.params_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(150)
                    && !self.creative_fx_state.is_rendering
                {
                    self.creative_fx_state.params_dirty_at = None;
                    self.run_creative_fx_render();
                }
            }
            if self.creative_fx_state.is_rendering
                || self.creative_fx_state.is_generating_thumbnails
            {
                ctx.request_repaint();
            }
        }

        // Copy to clipboard slot (Ctrl+C / Cmd+C) at any point
        if image_shortcuts_enabled {
            let copy_requested = ui.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || (i.key_pressed(egui::Key::C)
                        && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
            }) || ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
            });

            if copy_requested {
                self.copy_to_clipboard_slot();
            }

            // Save As shortcut (Ctrl+S / Cmd+S)
            let save_requested = ui.input(|i| {
                i.key_pressed(egui::Key::S)
                    && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd)
            }) || ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
            });

            if save_requested {
                self.open_save_dialog();
            }
        }

        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.canvas))
            .show(ui, |ui| {
                let mut shell = self.create_studio_shell(palette, close);
                egui::TopBottomPanel::bottom(ui.make_persistent_id("image_studio_footer"))
                    .frame(egui::Frame::none())
                    .resizable(false)
                    .show_inside(ui, |ui| shell.render_footer(ui, &self.status_toast, now));

                // 1. TOP HEADER & ESCAPE HATCHES
                self.render_header(ui, ctx, palette, &mut shell);

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 2. TOOLBAR (Mode selection & Viewport controls)
                self.render_toolbar(ui, palette);

                // Tool modules own their compact or panel-based controls.
                self.render_active_tool_controls(ui, ctx, palette);

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 3. MAIN INTERACTIVE VIEWPORT
                self.render_active_tool_workspace(ui, ctx, palette);
            });
        self.rendering_frame = false;
        if let Some(hand) = self.copy_requested.take() {
            self.start_copy_to_hand_slot(hand, little_clip_core::BackgroundJobs::global());
        }
    }

    fn create_studio_shell<'a>(
        &self,
        palette: &'a AppPalette,
        close: &'a mut bool,
    ) -> crate::shell::StudioShell<'a> {
        let aspect_str = format_aspect_ratio(self.current_rgba.width(), self.current_rgba.height());
        let dim_text = format!(
            "{}×{} ({})",
            self.current_rgba.width(),
            self.current_rgba.height(),
            aspect_str
        );
        let fmt_str = if self.is_svg && self.undo_stack.is_empty() {
            "SVG (VECTOR)".to_string()
        } else {
            self.mime.replace("image/", "").to_uppercase()
        };
        let size_str = format_bytes_size(self.file_size_bytes);

        let is_primary_svg = self.is_svg && self.undo_stack.is_empty();
        let is_primary_raster = !is_primary_svg;

        crate::shell::StudioShell::new(
            format!("IMAGE STUDIO — {}", self.title),
            Icon::Image,
            palette,
            close,
        )
        .with_compact_title(self.title.clone())
        .with_badge(dim_text, Some(hex_to_c32(&palette.harmonies.split_comp_1)))
        .with_badge(fmt_str, Some(hex_to_c32(&palette.diff_add)))
        .with_badge(size_str, Some(hex_to_c32(&palette.diff_mod)))
        .with_escape_hatch(
            image_editor_hatch("Krita", "krita", &self.external_editor)
                .with_primary(is_primary_raster)
                .with_icon(Icon::Palette),
        )
        .with_escape_hatch(
            image_editor_hatch("Inkscape", "inkscape", &self.svg_editor)
                .with_primary(is_primary_svg)
                .with_icon(Icon::Pencil),
        )
        .with_escape_hatch(
            image_editor_hatch("Nomacs", "nomacs", &self.viewer)
                .with_primary(false)
                .with_icon(Icon::Eye),
        )
        .with_footer_hints(image_studio_footer_hint(self.active_mode))
    }

    fn render_header(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        shell: &mut crate::shell::StudioShell<'_>,
    ) {
        let mut clicked_save_as = false;
        let mut clicked_copy = false;

        let clicked_hatch = shell.render_header(ui, |ui| {
            // The shared header lays actions out from the right edge inward.
            // Draw Copy first so Save As appears immediately after Close.
            let copy_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Copy),
                "Copy (Ctrl+C)",
                hex_to_c32(&palette.harmonies.base),
                palette,
            )
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)));
            if ui
                .add(copy_btn)
                .on_hover_text(
                    "Copy current image to Left Hand clipboard slot & OS clipboard (Ctrl+C)",
                )
                .clicked()
            {
                clicked_copy = true;
            }

            ui.add_space(crate::ui_ext::SPACE_XS);

            // Save As button
            let save_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Save),
                "Save As...",
                hex_to_c32(&palette.diff_add),
                palette,
            )
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.diff_add)));
            if ui
                .add(save_btn)
                .on_hover_text("Save current image to file (Ctrl+S)")
                .clicked()
            {
                clicked_save_as = true;
            }
        });

        if clicked_save_as {
            self.open_save_dialog();
        }
        if clicked_copy {
            self.copy_to_clipboard_slot();
        }

        if let Some(hatch) = clicked_hatch.and_then(|idx| shell.escape_hatches.get(idx)) {
            self.launch_external_app(&hatch.command, ctx, shell.close);
        }
    }

    pub fn set_tool_mode(&mut self, mode: ImageToolMode) {
        let target_mode = if mode == ImageToolMode::Palette {
            ImageToolMode::PickColor
        } else {
            mode
        };
        if self.active_mode == target_mode {
            return;
        }
        let prev_mode = self.active_mode;
        self.active_mode = target_mode;
        if prev_mode == ImageToolMode::FilmSim && target_mode != ImageToolMode::FilmSim {
            self.film_sim_state.cancel_render();
            self.film_sim_state.cancel_thumbnail_batch();
        }
        if prev_mode == ImageToolMode::CreativeFx && target_mode != ImageToolMode::CreativeFx {
            self.creative_fx_state.cancel_render();
            self.cancel_creative_fx_thumbnail_batch();
        }
        if prev_mode == ImageToolMode::DecorrelationStretch
            && target_mode != ImageToolMode::DecorrelationStretch
        {
            self.cancel_decorrelation_stretch();
        }
        if prev_mode == ImageToolMode::Resize && target_mode != ImageToolMode::Resize {
            self.cancel_pixel_resize();
        }
        if prev_mode == ImageToolMode::Ocr
            && target_mode != ImageToolMode::Ocr
            && self.ocr_state.in_progress
        {
            self.cancel_ocr();
        }
        if prev_mode == ImageToolMode::Barcode && target_mode != ImageToolMode::Barcode {
            self.cancel_barcode_scan();
        }
        let leaving_active_image_tool = matches!(
            (prev_mode, self.bg_removal_operation),
            (
                ImageToolMode::RemoveBg,
                BgRemovalOperation::BackgroundRemoval
            ) | (ImageToolMode::DepthMap, BgRemovalOperation::DepthMap)
        );
        if leaving_active_image_tool && target_mode != prev_mode && self.bg_removal_in_progress {
            self.cancel_image_tool_process();
        }
        match target_mode {
            ImageToolMode::View
            | ImageToolMode::Crop
            | ImageToolMode::Deskew
            | ImageToolMode::Transform
            | ImageToolMode::Redact
            | ImageToolMode::Markup
            | ImageToolMode::Ocr
            | ImageToolMode::RemoveBg
            | ImageToolMode::DepthMap => {}
            ImageToolMode::PickColor | ImageToolMode::Palette => {
                self.invalidate_stale_palette_result();
                if !self.palette_state.is_extracted
                    || self.palette_state.extracted_colors.is_empty()
                {
                    self.run_palette_extraction();
                }
            }
            ImageToolMode::Resize => {
                self.resize_state.sync_from_image(&self.current_rgba);
            }
            ImageToolMode::Convert => {
                if !self.convert_state.output_matches(&self.input_stamp())
                    && !self.convert_state.is_encoding
                    && self.convert_state.params_dirty_at.is_none()
                {
                    self.schedule_conversion();
                }
            }
            ImageToolMode::DecorrelationStretch => {
                if !self.dcs_preview_is_current() && !self.dcs_state.is_computing {
                    self.update_decorrelation_stretch();
                }
            }
            ImageToolMode::Barcode => {
                // Decoding is explicitly user initiated. Entering this mode must
                // remain instant even for a multi-megapixel image.
            }
            ImageToolMode::FilmSim => {
                self.film_sim_state.auto_fitted = false;
                // Rerender when either the image or Film Look parameters changed.
                if !self.film_sim_preview_is_current() && !self.film_sim_state.is_rendering {
                    self.run_film_sim_render();
                }
                // Trigger thumbnail batch if not generated yet.
                if self.film_sim_state.stock_thumbnails.len()
                    + self.film_sim_state.pending_thumbnails.len()
                    < tools::all_film_stocks().len()
                    && !self.film_sim_state.is_generating_thumbnails
                {
                    self.run_thumbnail_batch();
                }
            }
            ImageToolMode::CreativeFx => {
                self.creative_fx_state.auto_fitted = false;
                // Re-entering after an image edit must not retain a hidden stale preview.
                if !self.creative_fx_preview_is_current() && !self.creative_fx_state.is_rendering {
                    self.run_creative_fx_render();
                }
                // Trigger thumbnail batch if not generated yet.
                if self.creative_fx_state.fx_thumbnails.len()
                    + self.creative_fx_state.pending_thumbnails.len()
                    < CreativeFxType::all().len()
                    && !self.creative_fx_state.is_generating_thumbnails
                {
                    self.run_creative_fx_thumbnail_batch();
                }
            }
        }
    }

    fn render_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    // Tool Modes
                    let modes = [
                        ImageToolMode::View,
                        ImageToolMode::Crop,
                        ImageToolMode::Deskew,
                        ImageToolMode::Transform,
                        ImageToolMode::Redact,
                        ImageToolMode::Markup,
                        ImageToolMode::PickColor,
                        ImageToolMode::Resize,
                        ImageToolMode::Convert,
                        ImageToolMode::Ocr,
                        ImageToolMode::Barcode,
                        ImageToolMode::RemoveBg,
                        ImageToolMode::DepthMap,
                        ImageToolMode::DecorrelationStretch,
                        ImageToolMode::FilmSim,
                        ImageToolMode::CreativeFx,
                    ];

                    for mode in modes {
                        let selected = self.active_mode == mode
                            || (mode == ImageToolMode::PickColor
                                && self.active_mode == ImageToolMode::Palette);
                        let button = crate::ui_ext::mode_tab_button_icon(
                            Some(mode.icon()),
                            mode.label(),
                            selected,
                            palette,
                        );
                        if ui.add(button).clicked() {
                            self.set_tool_mode(mode);
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        // Reset View
                        let reset_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::RotateCcw),
                            "Reset",
                            hex_to_c32(&palette.colors.surface_panel),
                            palette,
                        );
                        if ui
                            .add(reset_btn)
                            .on_hover_text("Reset zoom and pan")
                            .clicked()
                        {
                            self.zoom = 1.0;
                            self.pan = Vec2::ZERO;
                        }

                        // 1:1 Pixel Zoom
                        let pixel_btn = crate::secondary_button(
                            "1:1",
                            hex_to_c32(&palette.colors.surface_panel),
                            palette,
                        );
                        if ui
                            .add(pixel_btn)
                            .on_hover_text("100% actual pixel size (1)")
                            .clicked()
                        {
                            self.zoom = 1.0;
                        }

                        // Fit to window
                        let fit_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::ZoomFit),
                            "Fit",
                            hex_to_c32(&palette.colors.surface_panel),
                            palette,
                        );
                        if ui
                            .add(fit_btn)
                            .on_hover_text("Fit entire image into window (0 / F)")
                            .clicked()
                        {
                            self.fit_to_view(ui.available_size());
                        }

                        // Zoom In (+)
                        let plus_btn = crate::themed_button(
                            "+",
                            hex_to_c32(&palette.colors.surface_panel),
                            Some(hex_to_c32(&palette.colors.text_primary)),
                            palette,
                        );
                        if ui.add(plus_btn).clicked() {
                            self.zoom = (self.zoom * 1.25).clamp(0.05, 5.0);
                        }

                        // Zoom percentage readout
                        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(colors.text_secondary),
                        );

                        // Zoom Out (-)
                        let minus_btn = crate::themed_button(
                            "-",
                            hex_to_c32(&palette.colors.surface_panel),
                            Some(hex_to_c32(&palette.colors.text_primary)),
                            palette,
                        );
                        if ui.add(minus_btn).clicked() {
                            self.zoom = (self.zoom / 1.25).clamp(0.05, 5.0);
                        }
                    });
                });
            });
    }

    fn render_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let avail_size = ui.available_size();
        if !self.auto_fitted && avail_size.x > 50.0 && avail_size.y > 50.0 {
            self.fit_to_view(avail_size);
            self.auto_fitted = true;
        }

        let (response, painter) = ui.allocate_painter(avail_size, egui::Sense::click_and_drag());
        let canvas_rect = response.rect;

        // Background canvas fill
        painter.rect_filled(
            canvas_rect,
            Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
            hex_to_c32(&palette.colors.canvas),
        );

        // Panning interaction (drag) when in View, Resize, Palette, RemoveBg, DepthMap, Ocr, Barcode, or Convert mode (when not dragging split handle)
        let middle_drag = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let can_drag_pan = self.active_mode == ImageToolMode::View
            || self.active_mode == ImageToolMode::Resize
            || self.active_mode == ImageToolMode::Palette
            || self.active_mode == ImageToolMode::RemoveBg
            || self.active_mode == ImageToolMode::DepthMap
            || self.active_mode == ImageToolMode::Ocr
            || self.active_mode == ImageToolMode::Barcode
            || (self.active_mode == ImageToolMode::Convert
                && !self.convert_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::DecorrelationStretch
                && !self.dcs_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::FilmSim
                && !self.film_sim_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::CreativeFx
                && !self.creative_fx_state.is_dragging_split);
        if (can_drag_pan && response.dragged()) || middle_drag {
            self.pan += response.drag_delta();
        }

        // Zooming interaction (mouse wheel)
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

        // Double click resets view to fit
        if can_drag_pan && response.double_clicked() {
            self.fit_to_view(avail_size);
        }

        // Draw image texture
        if let Some(texture) = &self.texture {
            let img_w = self.current_rgba.width() as f32 * self.zoom;
            let img_h = self.current_rgba.height() as f32 * self.zoom;
            let center = canvas_rect.center() + self.pan;
            let img_rect = Rect::from_center_size(center, Vec2::new(img_w, img_h));

            // Draw checkerboard under image for alpha transparency
            draw_checkerboard(
                &painter,
                img_rect,
                self.zoom,
                hex_to_c32(&palette.colors.surface_recessed),
                hex_to_c32(&palette.colors.surface_panel),
            );

            // Draw image mesh (or Squoosh-style split view in Convert mode or DCS curtain view in DecorrelationStretch mode)
            if self.active_mode == ImageToolMode::Convert {
                self.render_convert_comparison(
                    ui,
                    &painter,
                    &response,
                    img_rect,
                    canvas_rect,
                    ctx,
                    palette,
                );
            } else if self.active_mode == ImageToolMode::DecorrelationStretch {
                self.render_decorrelation_stretch_comparison(
                    ui,
                    &painter,
                    &response,
                    img_rect,
                    canvas_rect,
                    ctx,
                    palette,
                );
            } else {
                let mut mesh = egui::Mesh::with_texture(texture.id());
                mesh.add_rect_with_uv(
                    img_rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.add(egui::Shape::mesh(mesh));
            }

            // Draw crisp subtle border around the image bounds
            painter.rect_stroke(
                img_rect,
                Rounding::ZERO,
                Stroke::new(
                    1.0_f32,
                    hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                ),
            );

            // Draw any placed redaction bars so they remain visible across all modes
            for b in &self.redaction_state.boxes {
                let screen_min = Pos2::new(
                    img_rect.min.x + b.normalized_rect.min.x * img_rect.width(),
                    img_rect.min.y + b.normalized_rect.min.y * img_rect.height(),
                );
                let screen_max = Pos2::new(
                    img_rect.min.x + b.normalized_rect.max.x * img_rect.width(),
                    img_rect.min.y + b.normalized_rect.max.y * img_rect.height(),
                );
                painter.rect_filled(
                    Rect::from_min_max(screen_min, screen_max),
                    Rounding::ZERO,
                    Color32::BLACK,
                );
            }

            // Draw any placed markup layers so they remain visible across all modes
            if self.active_mode != ImageToolMode::Markup && !self.markup_state.items.is_empty() {
                self.render_markup_passive(&painter, img_rect);
            }

            // Mode-specific on-canvas overlays
            if self.active_mode == ImageToolMode::Crop {
                self.render_crop_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Deskew {
                self.render_deskew_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Redact {
                self.render_redact_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Markup {
                self.render_markup_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::PickColor {
                self.render_pick_color_overlay(
                    ui,
                    &painter,
                    &response,
                    img_rect,
                    canvas_rect,
                    palette,
                );
            } else if self.active_mode == ImageToolMode::Barcode {
                self.render_barcode_overlay(ui, &painter, &response, img_rect, palette);
            }
        }
    }

    // ─── Film Simulation Methods ───────────────────────────────────────────────────

    /// Spawn a background thread that runs filmr's process_image() on the current
    /// image with the current FilmSimState parameters, then sends the result back.

    fn fit_to_view(&mut self, canvas_size: Vec2) {
        let img_w = self.current_rgba.width() as f32;
        let img_h = self.current_rgba.height() as f32;
        if img_w <= 0.0 || img_h <= 0.0 {
            return;
        }

        let margin = 40.0;
        let avail_w = (canvas_size.x - margin).max(10.0);
        let avail_h = (canvas_size.y - margin).max(10.0);

        let scale_x = avail_w / img_w;
        let scale_y = avail_h / img_h;
        self.zoom = scale_x.min(scale_y).clamp(0.01, 5.0);
        self.pan = Vec2::ZERO;
    }
}

fn image_studio_footer_hint(mode: ImageToolMode) -> &'static str {
    match mode {
        ImageToolMode::View => {
            "🖱 Scroll: Zoom | Drag: Pan | ▼ EXIF Details | Ctrl+C: Copy Slot | Ctrl+S: Save As | 0: Fit"
        }
        ImageToolMode::Crop => {
            "🖱 Drag crop box/handles | Enter: Apply | Ctrl+C: Copy Slot | Ctrl+S: Save As | Esc: Cancel"
        }
        ImageToolMode::Deskew => {
            "🎯 Click 4 points (TL → TR → BR → BL) | 🖱 Drag handles | Enter: Apply | Esc: Close"
        }
        ImageToolMode::Transform => {
            "🖱 Rotate / Flip buttons above | Ctrl+Z: Undo | Ctrl+C: Copy Slot | Ctrl+S: Save As"
        }
        ImageToolMode::Redact => {
            "🖱 Drag on image: Place black bar | 🛡️ Strip EXIF | Del: Remove | Enter: Burn/Apply | Ctrl+C: Copy Slot"
        }
        ImageToolMode::Markup => {
            "🖱 Drag: Draw shape | 1-8: Tools | Del: Delete | Esc: Deselect | Enter: Flatten Layers | Ctrl+C: Copy Slot"
        }
        ImageToolMode::PickColor => {
            "🖱 Hover: Magnifying loupe | Click: Copy Hex code to clipboard | Ctrl+C: Copy Slot"
        }
        ImageToolMode::Convert => {
            "🖱 Drag vertical divider: Before/After comparison | Enter: Apply to Canvas | Ctrl+S: Save As"
        }
        ImageToolMode::Ocr => {
            "🖱 Choose engine and prompt on the left | Enter: Run OCR | Results appear on the right"
        }
        ImageToolMode::Barcode => {
            "🖱 Click code card or box to inspect | Enter: Rescan | 🌐 Open Link | 📋 Copy Payload"
        }
        ImageToolMode::CreativeFx => {
            "🖱 Split slider: Before/After comparison | Double-click: Fit | Sliders: Double-click to reset"
        }
        _ => {
            "🖱 Scroll: Zoom | Drag: Pan | Ctrl+C: Copy to Slot | Ctrl+S: Save As | 0: Fit | Esc: Close"
        }
    }
}

fn chip_badge(ui: &mut egui::Ui, text: &str, text_col: Color32, bg_col: Color32) {
    let frame = egui::Frame::none()
        .fill(bg_col)
        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0));
    frame.show(ui, |ui| {
        ui.label(
            egui::RichText::new(text)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text_col),
        );
    });
}

fn draw_checkerboard(
    painter: &egui::Painter,
    rect: Rect,
    _zoom: f32,
    dark_col: Color32,
    light_col: Color32,
) {
    let cell_size = 16.0;

    painter.rect_filled(rect, Rounding::ZERO, dark_col);

    let min_x = rect.min.x;
    let max_x = rect.max.x;
    let min_y = rect.min.y;
    let max_y = rect.max.y;

    let cols = ((max_x - min_x) / cell_size).ceil() as usize;
    let rows = ((max_y - min_y) / cell_size).ceil() as usize;

    for r in 0..rows {
        for c in 0..cols {
            if (r + c) % 2 == 1 {
                let x0 = (min_x + c as f32 * cell_size).min(max_x);
                let x1 = (min_x + (c + 1) as f32 * cell_size).min(max_x);
                let y0 = (min_y + r as f32 * cell_size).min(max_y);
                let y1 = (min_y + (r + 1) as f32 * cell_size).min(max_y);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)),
                    Rounding::ZERO,
                    light_col,
                );
            }
        }
    }
}

fn format_aspect_ratio(w: u32, h: u32) -> String {
    if h == 0 {
        return "0:0".to_string();
    }
    let ratio = w as f32 / h as f32;
    if (ratio - 16.0 / 9.0).abs() < 0.03 {
        "16:9".to_string()
    } else if (ratio - 9.0 / 16.0).abs() < 0.03 {
        "9:16".to_string()
    } else if (ratio - 4.0 / 3.0).abs() < 0.03 {
        "4:3".to_string()
    } else if (ratio - 1.0).abs() < 0.02 {
        "1:1".to_string()
    } else {
        format!("{:.2}:1", ratio)
    }
}

fn format_bytes_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn encode_jpeg_with_quality(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, image::ImageError> {
    let mut rgb = image::RgbImage::new(img.width(), img.height());
    for (x, y, pixel) in img.enumerate_pixels() {
        let alpha = pixel[3] as f32 / 255.0;
        let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        rgb.put_pixel(x, y, image::Rgb([r, g, b]));
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    encoder.encode(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(buf.into_inner())
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

fn encode_webp(img: &RgbaImage, lossless: bool, quality: u8) -> Result<Vec<u8>, String> {
    let encoder = webp::Encoder::from_rgba(img.as_raw(), img.width(), img.height());
    let memory = if lossless {
        encoder.encode_lossless()
    } else {
        encoder.encode(quality as f32)
    };
    Ok(memory.to_vec())
}

struct WebpProgressContext<'a> {
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
    callback_count: Option<&'a std::sync::atomic::AtomicUsize>,
    cancel_on_callback: bool,
}

struct WebpPictureGuard(libwebp_sys::WebPPicture);

impl Drop for WebpPictureGuard {
    fn drop(&mut self) {
        unsafe { libwebp_sys::WebPPictureFree(&mut self.0) };
    }
}

struct WebpMemoryWriterGuard(libwebp_sys::WebPMemoryWriter);

impl Drop for WebpMemoryWriterGuard {
    fn drop(&mut self) {
        unsafe { libwebp_sys::WebPMemoryWriterClear(&mut self.0) };
    }
}

unsafe extern "C" fn webp_cancel_progress_hook(
    _percent: std::ffi::c_int,
    picture: *const libwebp_sys::WebPPicture,
) -> std::ffi::c_int {
    if picture.is_null() {
        return 0;
    }
    let context = unsafe {
        (*picture)
            .user_data
            .cast::<WebpProgressContext<'_>>()
            .as_ref()
    };
    let Some(context) = context else {
        return 0;
    };
    if let Some(count) = context.callback_count {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    if context.cancel_on_callback {
        if let Some(cancel) = context.cancel {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
    }
    if context
        .cancel
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
    {
        0
    } else {
        1
    }
}

fn encode_webp_cancellable(
    img: &RgbaImage,
    lossless: bool,
    quality: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<u8>, String> {
    encode_webp_cancellable_with_progress(img, lossless, quality, cancel, None, false)
}

fn encode_webp_cancellable_with_progress(
    img: &RgbaImage,
    lossless: bool,
    quality: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    callback_count: Option<&std::sync::atomic::AtomicUsize>,
    cancel_on_callback: bool,
) -> Result<Vec<u8>, String> {
    use std::sync::atomic::Ordering;
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err("cancelled".into());
    }

    let mut config = libwebp_sys::WebPConfig::new()
        .map_err(|_| "WebP encoder configuration initialization failed".to_string())?;
    config.lossless = i32::from(lossless);
    config.alpha_compression = i32::from(!lossless);
    config.quality = if lossless { 75.0 } else { quality as f32 };
    if unsafe { libwebp_sys::WebPValidateConfig(&config) } == 0 {
        return Err("WebP encoder configuration is invalid".into());
    }

    let mut picture = WebpPictureGuard(
        libwebp_sys::WebPPicture::new()
            .map_err(|_| "WebP picture initialization failed".to_string())?,
    );
    picture.0.use_argb = 1;
    picture.0.width = img.width() as i32;
    picture.0.height = img.height() as i32;
    let context = WebpProgressContext {
        cancel,
        callback_count,
        cancel_on_callback,
    };
    picture.0.progress_hook = Some(webp_cancel_progress_hook);
    picture.0.user_data = (&context as *const WebpProgressContext<'_>)
        .cast_mut()
        .cast();

    let mut writer = std::mem::MaybeUninit::<libwebp_sys::WebPMemoryWriter>::uninit();
    unsafe { libwebp_sys::WebPMemoryWriterInit(writer.as_mut_ptr()) };
    let mut writer = WebpMemoryWriterGuard(unsafe { writer.assume_init() });
    picture.0.writer = Some(libwebp_sys::WebPMemoryWrite);
    picture.0.custom_ptr = (&mut writer.0 as *mut libwebp_sys::WebPMemoryWriter).cast();

    let imported = unsafe {
        libwebp_sys::WebPPictureImportRGBA(
            &mut picture.0,
            img.as_raw().as_ptr(),
            img.width() as i32 * 4,
        )
    };
    let result = if imported == 0 {
        Err("WebP RGBA import failed".into())
    } else if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        Err("cancelled".into())
    } else {
        let status = unsafe { libwebp_sys::WebPEncode(&config, &mut picture.0) };
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            Err("cancelled".into())
        } else if status != 0 {
            if writer.0.mem.is_null() || writer.0.size == 0 {
                Err("WebP encoder produced no output".into())
            } else {
                Ok(unsafe { std::slice::from_raw_parts(writer.0.mem, writer.0.size) }.to_vec())
            }
        } else {
            Err(format!("WebP encoding failed: {:?}", picture.0.error_code))
        }
    };
    // Both RAII guards clean up libwebp allocations on normal return or unwind.
    drop(picture);
    drop(writer);
    result
}

fn render_svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let output = CommandRequest::new("rsvg-convert")
        .stdin_bytes(bytes)
        .run()
        .ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        Some(output.stdout)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_image_data_is_shared_across_independent_editor_states() {
        let image = RgbaImage::from_pixel(2, 2, image::Rgba([24, 80, 180, 255]));
        let mut encoded = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Png,
            )
            .unwrap();
        let source = DecodedImageData::from_bytes(encoded, "image/png".to_string(), None).unwrap();

        let first = ImagePreviewState::from_decoded(
            source.clone(),
            None,
            "shared.png".to_string(),
            &AppConfig::default(),
        );
        let mut second = ImagePreviewState::from_decoded(
            source,
            None,
            "shared.png".to_string(),
            &AppConfig::default(),
        );
        assert!(Arc::ptr_eq(&first.current_rgba, &second.current_rgba));
        assert!(Arc::ptr_eq(&first.raw_bytes, &second.raw_bytes));
        assert_eq!(first.title, second.title);

        second.zoom = 2.0;
        assert_eq!(first.zoom, 1.0);
        Arc::make_mut(&mut second.current_rgba).put_pixel(0, 0, image::Rgba([240, 20, 40, 255]));
        assert!(!Arc::ptr_eq(&first.current_rgba, &second.current_rgba));
        assert_eq!(
            *first.current_rgba.get_pixel(0, 0),
            image::Rgba([24, 80, 180, 255])
        );
        assert_eq!(
            *second.current_rgba.get_pixel(0, 0),
            image::Rgba([240, 20, 40, 255])
        );
        assert!(Arc::ptr_eq(&first.raw_bytes, &second.raw_bytes));
    }

    #[test]
    fn inline_decoded_image_source_seeds_independent_full_studios() {
        let image = RgbaImage::from_pixel(3, 2, image::Rgba([24, 80, 180, 255]));
        let mut encoded = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Png,
            )
            .unwrap();
        let source =
            Arc::new(DecodedImageData::from_bytes(encoded, "image/png".to_owned(), None).unwrap());
        let inline = InlineImagePreview::new(Arc::clone(&source));
        let mut first = ImagePreviewState::from_decoded(
            (*source).clone(),
            None,
            "First editor".to_owned(),
            &AppConfig::default(),
        );
        let second = ImagePreviewState::from_decoded(
            (*source).clone(),
            None,
            "Second editor".to_owned(),
            &AppConfig::default(),
        );

        assert!(Arc::ptr_eq(&inline.source, &source));
        assert!(Arc::ptr_eq(&first.current_rgba, &source.original_rgba));
        assert!(Arc::ptr_eq(&second.current_rgba, &source.original_rgba));
        assert!(Arc::ptr_eq(&first.raw_bytes, &source.encoded_bytes));
        assert!(first.palette_ticket.is_none());
        assert!(first.palette_rx.is_none());
        assert!(first.optimizer_rx.is_none());
        assert!(first.optimizer_job_rx.is_none());

        first.zoom = 2.0;
        assert_eq!(second.zoom, 1.0);
        Arc::make_mut(&mut first.current_rgba).put_pixel(0, 0, image::Rgba([240, 20, 40, 255]));
        assert!(!Arc::ptr_eq(&first.current_rgba, &second.current_rgba));
        assert_eq!(
            *second.current_rgba.get_pixel(0, 0),
            image::Rgba([24, 80, 180, 255])
        );
    }

    #[test]
    fn inline_image_previews_render_shared_pixels_with_independent_zoom_state() {
        let image = RgbaImage::from_pixel(8, 4, image::Rgba([24, 80, 180, 255]));
        let mut encoded = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Png,
            )
            .unwrap();
        let source =
            Arc::new(DecodedImageData::from_bytes(encoded, "image/png".to_string(), None).unwrap());
        let mut first = InlineImagePreview::new(Arc::clone(&source));
        let mut second = InlineImagePreview::new(Arc::clone(&source));
        assert!(Arc::ptr_eq(&first.source, &second.source));

        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 420.0));
        let mut first_rect = Rect::NOTHING;
        let mut second_rect = Rect::NOTHING;
        let render = |ctx: &egui::Context,
                      first: &mut InlineImagePreview,
                      second: &mut InlineImagePreview,
                      events: Vec<egui::Event>,
                      first_rect: &mut Rect,
                      second_rect: &mut Rect| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.vertical(|ui| {
                            *first_rect = first.show(ui, Vec2::new(300.0, 180.0)).rect;
                            *second_rect = second.show(ui, Vec2::new(220.0, 120.0)).rect;
                        });
                    });
                },
            )
        };

        let first_frame = render(
            &ctx,
            &mut first,
            &mut second,
            Vec::new(),
            &mut first_rect,
            &mut second_rect,
        );
        let first_texture_id = first.texture.as_ref().unwrap().id();
        let second_texture_id = second.texture.as_ref().unwrap().id();
        for (texture_id, assigned_rect) in [
            (first_texture_id, first_rect),
            (second_texture_id, second_rect),
        ] {
            let mesh = first_frame
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Mesh(mesh)
                        if mesh.texture_id == texture_id && !mesh.vertices.is_empty() =>
                    {
                        Some(mesh)
                    }
                    _ => None,
                })
                .expect("each inline preview should draw a textured mesh");
            for vertex in &mesh.vertices {
                assert!(vertex.pos.x >= assigned_rect.min.x - 0.1);
                assert!(vertex.pos.y >= assigned_rect.min.y - 0.1);
                assert!(vertex.pos.x <= assigned_rect.max.x + 0.1);
                assert!(vertex.pos.y <= assigned_rect.max.y + 0.1);
            }
        }

        let second_frame = render(
            &ctx,
            &mut first,
            &mut second,
            vec![
                egui::Event::PointerMoved(first_rect.center()),
                egui::Event::Scroll(Vec2::new(0.0, 120.0)),
            ],
            &mut first_rect,
            &mut second_rect,
        );

        assert!(first.zoom > 1.0);
        assert_eq!(second.zoom, 1.0);
        assert!(Arc::ptr_eq(
            &source.original_rgba,
            &first.source.original_rgba
        ));
        assert!(Arc::ptr_eq(
            &source.original_rgba,
            &second.source.original_rgba
        ));
        let preview_texture_ids = second_frame
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Mesh(mesh) if !mesh.vertices.is_empty() => {
                    Some(mesh.texture_id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(preview_texture_ids.len(), 2);
        assert_ne!(preview_texture_ids[0], preview_texture_ids[1]);
        for clipped in &second_frame.shapes {
            let egui::epaint::Shape::Mesh(mesh) = &clipped.shape else {
                continue;
            };
            let assigned_rect = if mesh.texture_id == first_texture_id {
                Some(first_rect)
            } else if mesh.texture_id == second_texture_id {
                Some(second_rect)
            } else {
                None
            };
            if let Some(assigned_rect) = assigned_rect {
                assert!(clipped.clip_rect.min.x >= assigned_rect.min.x - 0.1);
                assert!(clipped.clip_rect.min.y >= assigned_rect.min.y - 0.1);
                assert!(clipped.clip_rect.max.x <= assigned_rect.max.x + 0.1);
                assert!(clipped.clip_rect.max.y <= assigned_rect.max.y + 0.1);
            }
        }
    }

    #[test]
    fn clipped_inline_image_preview_defers_texture_upload() {
        let image = RgbaImage::from_pixel(4, 3, image::Rgba([24, 80, 180, 255]));
        let mut encoded = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Png,
            )
            .unwrap();
        let source =
            Arc::new(DecodedImageData::from_bytes(encoded, "image/png".to_string(), None).unwrap());
        let mut preview = InlineImagePreview::new(source);
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 100.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(60.0)
                        .show(ui, |ui| {
                            ui.add_space(240.0);
                            preview.show(ui, Vec2::new(120.0, 80.0));
                        });
                });
            },
        );

        assert!(preview.texture.is_none());

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 500.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .show(ui, |ui| {
                            ui.add_space(240.0);
                            preview.show(ui, Vec2::new(120.0, 80.0));
                        });
                });
            },
        );

        assert!(preview.texture.is_some());
    }

    fn wait_for_dcs(state: &mut ImagePreviewState) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state.dcs_state.is_computing && std::time::Instant::now() < deadline {
            state.poll_decorrelation_stretch(&ctx, BackgroundJobs::global());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_decorrelation_stretch(&ctx, BackgroundJobs::global());
        assert!(!state.dcs_state.is_computing);
        assert!(state.dcs_state.error.is_none());
    }

    fn wait_for_pixel_resize(state: &mut ImagePreviewState) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while (state.pixel_resize_job.pending || state.pixel_resize_job.retry_at.is_some())
            && std::time::Instant::now() < deadline
        {
            state.poll_pixel_resize(&ctx, BackgroundJobs::global());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_pixel_resize(&ctx, BackgroundJobs::global());
        assert!(!state.pixel_resize_job.pending);
        assert!(state.pixel_resize_job.error.is_none());
    }

    fn image_state_from_png(size: (u32, u32), title: &str) -> ImagePreviewState {
        let mut png = Vec::new();
        RgbaImage::new(size.0, size.1)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            png,
            "image/png".into(),
            None,
            title.into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[cfg(unix)]
    fn assert_configured_app_receives_source_image(
        executable_name: &str,
        configure: impl FnOnce(&mut AppConfig, &str),
    ) {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("source.png");
        RgbaImage::from_pixel(3, 2, image::Rgba([17, 83, 149, 255]))
            .save(&source_path)
            .unwrap();
        let expected_bytes = std::fs::read(&source_path).unwrap();
        let executable_path = temp_dir.path().join(executable_name);
        let argument_capture = temp_dir.path().join(format!("{executable_name}.arg"));
        let bytes_capture = temp_dir.path().join(format!("{executable_name}.bytes"));
        let script = format!(
            "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\ncat \"$1\" > '{}'\n",
            argument_capture.display(),
            bytes_capture.display()
        );
        std::fs::write(&executable_path, script).unwrap();
        let mut permissions = std::fs::metadata(&executable_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable_path, permissions).unwrap();

        let mut config = AppConfig::default();
        configure(&mut config, executable_path.to_str().unwrap());
        let mut state = ImagePreviewState::from_path(source_path.clone(), &config).unwrap();
        let ctx = egui::Context::default();
        let mut close = false;
        let app = match executable_name {
            "fake-krita" => state.external_editor.clone(),
            "fake-inkscape" => state.svg_editor.clone(),
            _ => state.viewer.clone(),
        };
        state.launch_external_app(&app, &ctx, &mut close);

        assert!(matches!(
            state.status_toast.as_ref(),
            Some((message, _)) if message.contains("Launched")
        ));
        assert!(!close, "external handoff leaves Image Studio open");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while (!argument_capture.exists() || !bytes_capture.exists())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            std::fs::read_to_string(argument_capture).unwrap(),
            source_path.to_string_lossy()
        );
        assert_eq!(std::fs::read(bytes_capture).unwrap(), expected_bytes);
    }

    #[cfg(unix)]
    fn assert_external_handoff_keeps_edited_session(
        app: &str,
        command: impl FnOnce(&AppConfig) -> String,
    ) {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("edited-source.png");
        RgbaImage::from_pixel(3, 2, image::Rgba([17, 83, 149, 255]))
            .save(&source_path)
            .unwrap();
        let config = AppConfig::default();
        let mut state = ImagePreviewState::from_path(source_path.clone(), &config).unwrap();
        Arc::make_mut(&mut state.current_rgba).put_pixel(0, 0, image::Rgba([200, 30, 40, 255]));
        state.undo_stack.push(Arc::clone(&state.original_rgba));
        let edited_pixels = Arc::clone(&state.current_rgba);
        let undo_len = state.undo_stack.len();
        let command = command(&config);

        state.launch_external_app_with_path(
            &command,
            Ok(source_path.clone()),
            |app_name, target, path| {
                assert_eq!(app_name, app);
                assert_eq!(target, command);
                assert_eq!(path, source_path);
                Ok("Launched test editor".to_owned())
            },
        );

        assert!(Arc::ptr_eq(&state.current_rgba, &edited_pixels));
        assert_eq!(
            state.current_rgba.get_pixel(0, 0),
            &image::Rgba([200, 30, 40, 255])
        );
        assert_eq!(state.undo_stack.len(), undo_len);
        assert!(matches!(
            state.status_toast.as_ref(),
            Some((message, _)) if message == "Launched test editor"
        ));
    }

    #[cfg(unix)]
    fn assert_unavailable_configured_app_sets_failure_toast(
        executable_name: &str,
        configure: impl FnOnce(&mut AppConfig, &str),
    ) {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("source.png");
        RgbaImage::from_pixel(3, 2, image::Rgba([17, 83, 149, 255]))
            .save(&source_path)
            .unwrap();
        let missing_app = temp_dir.path().join(format!("missing-{executable_name}"));
        let missing_fallback = temp_dir.path().join("missing-default-opener");
        let mut config = AppConfig::default();
        configure(&mut config, missing_app.to_str().unwrap());
        let mut state = ImagePreviewState::from_path(source_path, &config).unwrap();
        let app = match executable_name {
            "krita" => state.external_editor.clone(),
            "inkscape" => state.svg_editor.clone(),
            _ => state.viewer.clone(),
        };
        state.launch_external_app_with(&app, |app_name, command, path| {
            crate::shell::EscapeHatchLauncher::spawn_with_fallback_command(
                app_name,
                command,
                path,
                missing_fallback.to_str().unwrap(),
            )
        });

        let expected = format!(
            "⚠️ Failed to launch '{}' (tool not installed)",
            missing_app.display()
        );
        assert!(matches!(
            state.status_toast.as_ref(),
            Some((message, _)) if message == &expected
        ));
    }

    #[cfg(unix)]
    #[test]
    fn krita_handoff_receives_current_source_image_file() {
        assert_configured_app_receives_source_image("fake-krita", |config, executable| {
            config.image.external_editor = executable.to_owned();
        });
    }

    #[cfg(unix)]
    #[test]
    fn inkscape_handoff_receives_current_source_image_file() {
        assert_configured_app_receives_source_image("fake-inkscape", |config, executable| {
            config.image.svg_editor = executable.to_owned();
        });
    }

    #[cfg(unix)]
    #[test]
    fn viewer_handoff_receives_current_source_image_file() {
        assert_configured_app_receives_source_image("fake-nomacs", |config, executable| {
            config.image.viewer = executable.to_owned();
        });
    }

    #[cfg(unix)]
    #[test]
    fn returning_from_krita_keeps_the_open_edited_image_session() {
        assert_external_handoff_keeps_edited_session("krita", |config| {
            config.image.external_editor.clone()
        });
    }

    #[cfg(unix)]
    #[test]
    fn returning_from_inkscape_keeps_the_open_edited_image_session() {
        assert_external_handoff_keeps_edited_session("inkscape", |config| {
            config.image.svg_editor.clone()
        });
    }

    #[cfg(unix)]
    #[test]
    fn returning_from_image_viewer_keeps_the_open_edited_image_session() {
        assert_external_handoff_keeps_edited_session("nomacs", |config| {
            config.image.viewer.clone()
        });
    }

    #[cfg(unix)]
    #[test]
    fn missing_krita_sets_image_studio_failure_toast_without_host_fallback() {
        assert_unavailable_configured_app_sets_failure_toast("krita", |config, executable| {
            config.image.external_editor = executable.to_owned();
        });
    }

    #[cfg(unix)]
    #[test]
    fn missing_inkscape_sets_image_studio_failure_toast_without_host_fallback() {
        assert_unavailable_configured_app_sets_failure_toast("inkscape", |config, executable| {
            config.image.svg_editor = executable.to_owned();
        });
    }

    #[cfg(unix)]
    #[test]
    fn missing_viewer_sets_image_studio_failure_toast_without_host_fallback() {
        assert_unavailable_configured_app_sets_failure_toast("nomacs", |config, executable| {
            config.image.viewer = executable.to_owned();
        });
    }

    #[test]
    fn composed_input_stamp_tracks_every_display_image_input() {
        let mut state = image_state_from_png((8, 8), "input stamp test");
        let initial = state.input_stamp();
        assert_eq!(state.input_stamp(), initial);

        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
        });
        assert_ne!(state.input_stamp(), initial);
        let redacted = state.input_stamp();

        state.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::new(0.2, 0.2),
            text: "stamp".into(),
            font_size: 12.0,
        });
        assert_ne!(state.input_stamp(), redacted);
        let marked = state.input_stamp();

        state.active_mode = ImageToolMode::Crop;
        assert_ne!(state.input_stamp(), marked);
        let cropped = state.input_stamp();
        state.crop_state.normalized_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5));
        assert_ne!(state.input_stamp(), cropped);
        let cropped_selection = state.input_stamp();

        state.active_mode = ImageToolMode::View;
        assert_ne!(state.input_stamp(), cropped_selection);
        let view_mode = state.input_stamp();
        state.mark_pixels_changed();
        assert_ne!(state.input_stamp(), view_mode);
    }

    #[test]
    fn display_snapshot_request_shares_the_pixel_arc() {
        let state = image_state_from_png((12, 9), "snapshot arc test");

        let request = state.display_image_snapshot_request();

        assert!(Arc::ptr_eq(&request.pixels, &state.current_rgba));
        assert_eq!(request.input_stamp, state.input_stamp());
    }

    #[test]
    fn display_snapshot_composes_redactions_markup_and_crop_like_current_image() {
        let mut state = image_state_from_png((80, 60), "snapshot compose test");
        state.current_rgba = Arc::new(RgbaImage::from_fn(80, 60, |x, y| {
            image::Rgba([(x * 3) as u8, (y * 4) as u8, (x + y) as u8, 255])
        }));
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.45, 0.5)),
        });
        state.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::new(0.55, 0.5),
            text: "snapshot".into(),
            font_size: 12.0,
        });

        let edited_request = state.display_image_snapshot_request();
        assert_eq!(edited_request.compose(), state.current_display_image());

        state.active_mode = ImageToolMode::Crop;
        state.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.1, 0.15), Pos2::new(0.8, 0.85));
        let crop_request = state.display_image_snapshot_request();
        assert_eq!(crop_request.compose(), state.current_display_image());
        assert_eq!(crop_request.compose().dimensions(), (56, 42));
        assert!(Arc::ptr_eq(&crop_request.pixels, &state.current_rgba));
    }

    #[test]
    fn background_worker_discards_stale_success_and_clears_loading() {
        let mut state = image_state_from_png((8, 8), "stale background result");
        let previous_cache = RgbaImage::from_pixel(1, 1, image::Rgba([9, 8, 7, 255]));
        state.creative_fx_state.cached_background_removal = Some(previous_cache.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        state.bg_removal_input_stamp = Some(state.input_stamp());
        state.bg_removal_rx = Some(rx);
        state.bg_removal_in_progress = true;
        state.bg_removal_status = Some("working".into());
        tx.send(BgRemovalMsg::Success {
            result_rgba: RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])),
            duration_ms: 5,
        })
        .unwrap();
        state.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::ZERO,
            text: "new input".into(),
            font_size: 10.0,
        });

        state.poll_bg_removal();

        assert_eq!(
            state.current_rgba.get_pixel(0, 0),
            &image::Rgba([0, 0, 0, 0])
        );
        assert_eq!(
            state.creative_fx_state.cached_background_removal,
            Some(previous_cache)
        );
        assert!(!state.bg_removal_in_progress);
        assert!(state.bg_removal_status.is_none());
        assert!(state.bg_removal_rx.is_none());
        assert!(state.bg_removal_error.is_none());
    }

    #[test]
    fn background_worker_applies_unchanged_input_and_clears_baked_overlays() {
        let mut state = image_state_from_png((8, 8), "current background result");
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
        });
        state.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::ZERO,
            text: "baked once".into(),
            font_size: 10.0,
        });
        let stamp = state.input_stamp();
        let output = RgbaImage::from_pixel(8, 8, image::Rgba([11, 22, 33, 255]));
        let (tx, rx) = std::sync::mpsc::channel();
        state.bg_removal_input_stamp = Some(stamp);
        state.bg_removal_rx = Some(rx);
        state.bg_removal_in_progress = true;
        tx.send(BgRemovalMsg::Success {
            result_rgba: output.clone(),
            duration_ms: 5,
        })
        .unwrap();

        state.poll_bg_removal();

        assert_eq!(state.current_rgba.as_ref(), &output);
        assert_eq!(state.pixel_revision, 1);
        assert!(state.redaction_state.boxes.is_empty());
        assert!(state.markup_state.items.is_empty());
        assert_eq!(
            state.creative_fx_state.cached_background_removal,
            Some(output)
        );
        assert_eq!(
            state.status_toast.as_ref().unwrap().0,
            "✨ Background removed in 0.01s!"
        );
        assert!(!state.bg_removal_in_progress);
        assert!(state.bg_removal_rx.is_none());
    }

    #[test]
    fn depth_map_completion_uses_a_depth_specific_toast() {
        let mut state = image_state_from_png((8, 8), "depth map result");
        let (tx, rx) = std::sync::mpsc::channel();
        state.bg_removal_input_stamp = Some(state.input_stamp());
        state.bg_removal_operation = BgRemovalOperation::DepthMap;
        state.bg_removal_rx = Some(rx);
        state.bg_removal_in_progress = true;
        tx.send(BgRemovalMsg::Success {
            result_rgba: RgbaImage::from_pixel(8, 8, image::Rgba([24, 24, 24, 255])),
            duration_ms: 1250,
        })
        .unwrap();

        state.poll_bg_removal();

        let toast = state.status_toast.as_ref().unwrap();
        assert_eq!(toast.0, "🌊 Depth map generated in 1.25s!");
        assert_eq!(
            state.bg_removal_operation,
            BgRemovalOperation::BackgroundRemoval
        );
    }

    #[test]
    fn background_worker_discards_stale_error_without_showing_it() {
        let mut state = image_state_from_png((8, 8), "stale background error");
        let (tx, rx) = std::sync::mpsc::channel();
        state.bg_removal_input_stamp = Some(state.input_stamp());
        state.bg_removal_rx = Some(rx);
        state.bg_removal_in_progress = true;
        tx.send(BgRemovalMsg::Error("stale worker failure".into()))
            .unwrap();
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.25, 0.25)),
        });

        state.poll_bg_removal();

        assert!(state.bg_removal_error.is_none());
        assert!(!state.bg_removal_in_progress);
        assert!(state.bg_removal_status.is_none());
        assert!(state.bg_removal_rx.is_none());
    }

    #[test]
    fn reentering_convert_schedules_when_cached_result_source_is_stale() {
        let mut state = image_state_from_png((8, 8), "convert mode reentry");
        state.convert_state.converted_rgba = Some(RgbaImage::new(8, 8));
        state.convert_state.completed_generation = state.convert_state.request_generation;
        state.convert_state.completed_input_stamp = Some(state.input_stamp());
        state.mark_pixels_changed();

        state.set_tool_mode(ImageToolMode::Convert);

        assert!(state.convert_state.params_dirty_at.is_some());
        assert!(!state.convert_state.output_matches(&state.input_stamp()));
    }

    #[test]
    fn image_studio_footer_stays_inside_root_and_below_workspace_at_multiple_scales() {
        for (size, zoom, mode) in [
            (Vec2::new(820.0, 600.0), 1.0, ImageToolMode::View),
            (Vec2::new(1280.0, 720.0), 1.5, ImageToolMode::Markup),
            (Vec2::new(1600.0, 900.0), 2.0, ImageToolMode::View),
        ] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            ctx.set_zoom_factor(zoom);
            let mut state = image_state_from_png((80, 60), "Footer layout test");
            state.active_mode = mode;
            let palette = AppPalette::default();
            let mut root_rect = None;
            let mut close_requested = false;

            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        root_rect = Some(ui.max_rect());
                        state.render_ui(ui, ctx, &palette, &mut close_requested);
                    });
                },
            );

            let root = root_rect.expect("root layout should render");
            let footer_hint = image_studio_footer_hint(mode);
            let footer = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == footer_hint => {
                        Some(Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .expect("footer hint should paint");
            let viewport = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Rect(rect)
                        if rect.fill == crate::hex_to_c32(&palette.colors.canvas)
                            && rect.rounding
                                == Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("viewport canvas should paint");
            assert!(root.contains(footer.min) && root.contains(footer.max));
            assert!(root.contains(viewport.min) && root.contains(viewport.max));
            assert!(viewport.max.y < footer.min.y);
        }
    }

    #[test]
    fn image_editor_hatch_keeps_configured_command_and_fallback_help_together() {
        let hatch = image_editor_hatch("Inkscape", "inkscape", "/missing/path/inkscape");

        assert_eq!(hatch.label, "Inkscape");
        assert_eq!(hatch.command, "/missing/path/inkscape");
        assert!(
            hatch
                .tooltip
                .contains("/missing/path/inkscape is unavailable")
        );
        assert!(hatch.tooltip.contains("system default viewer"));
    }

    #[test]
    fn image_editor_hatch_displays_custom_executable_name_and_target() {
        let hatch = image_editor_hatch("Inkscape", "inkscape", "/opt/custom-tools/vector-editor");

        assert_eq!(hatch.label, "vector-editor");
        assert_eq!(hatch.command, "/opt/custom-tools/vector-editor");
        assert!(
            hatch
                .tooltip
                .contains("with /opt/custom-tools/vector-editor")
        );
    }

    #[test]
    fn save_dialog_owns_image_shortcuts_until_dismissed() {
        let mut state = image_state_from_png((100, 100), "Test image");
        state.active_mode = ImageToolMode::Crop;
        state.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.6, 0.6));
        state.undo_stack.push(state.current_rgba.clone());
        state.show_fallback_save_modal = true;
        state.fallback_save_needs_focus = true;
        state.fallback_save_path = "test.png".into();

        fn key_frame(
            state: &mut ImagePreviewState,
            ctx: &egui::Context,
            palette: &AppPalette,
            key: egui::Key,
            modifiers: egui::Modifiers,
        ) -> bool {
            let mut close = false;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, palette, &mut close);
                    });
                },
            );
            close
        }

        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        assert!(!key_frame(
            &mut state,
            &ctx,
            &palette,
            egui::Key::F12,
            egui::Modifiers::NONE
        ));
        assert!(ctx.wants_keyboard_input());
        assert!(!key_frame(
            &mut state,
            &ctx,
            &palette,
            egui::Key::Enter,
            egui::Modifiers::NONE
        ));
        assert_eq!(state.current_rgba.dimensions(), (100, 100));
        assert!(!key_frame(
            &mut state,
            &ctx,
            &palette,
            egui::Key::Z,
            egui::Modifiers::COMMAND
        ));
        assert_eq!(state.undo_stack.len(), 1);
        assert!(!key_frame(
            &mut state,
            &ctx,
            &palette,
            egui::Key::V,
            egui::Modifiers::NONE
        ));
        assert_eq!(state.active_mode, ImageToolMode::Crop);
        assert!(!key_frame(
            &mut state,
            &ctx,
            &palette,
            egui::Key::Escape,
            egui::Modifiers::NONE
        ));
        assert!(!state.show_fallback_save_modal);
        assert_eq!(state.crop_state.normalized_rect.min, Pos2::new(0.1, 0.1));
        assert!(!key_frame(
            &mut state,
            &ctx,
            &palette,
            egui::Key::V,
            egui::Modifiers::NONE
        ));
        assert_eq!(state.active_mode, ImageToolMode::View);
    }

    #[test]
    fn unmodified_v_shortcut_selects_view_tool() {
        let mut state = image_state_from_png((100, 100), "View shortcut fixture");
        state.active_mode = ImageToolMode::Crop;
        let palette = AppPalette::default();
        let ctx = egui::Context::default();
        let mut close = false;

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );

        assert_eq!(state.active_mode, ImageToolMode::View);
        assert!(!close, "selecting a tool does not close Image Studio");
    }

    #[test]
    fn crop_enter_applies_selected_region_through_rendered_shortcut() {
        let mut state = image_state_from_png((100, 100), "Crop Enter fixture");
        state.active_mode = ImageToolMode::Crop;
        state.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.6, 0.7));
        let palette = AppPalette::default();
        let mut close = false;

        let _ = egui::Context::default().run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );

        assert_eq!(state.current_rgba.dimensions(), (50, 50));
        assert_eq!(state.undo_stack.len(), 1);
        assert!(!close, "applying Crop must not close Image Studio");
    }

    #[test]
    fn redact_escape_deselects_region_without_burning_or_closing() {
        let mut state = image_state_from_png((100, 100), "Redact Escape fixture");
        state.current_rgba = Arc::new(RgbaImage::from_pixel(
            100,
            100,
            image::Rgba([80, 120, 160, 255]),
        ));
        state.active_mode = ImageToolMode::Redact;
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.8, 0.8)),
        });
        state.redaction_state.selected_idx = Some(0);
        let pixels_before = state.current_rgba.clone();
        let palette = AppPalette::default();
        let mut close = false;

        let _ = egui::Context::default().run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );

        assert_eq!(state.redaction_state.boxes.len(), 1);
        assert!(state.redaction_state.selected_idx.is_none());
        assert_eq!(state.current_rgba.as_ref(), pixels_before.as_ref());
        assert!(
            !close,
            "Escape deselects the redaction without closing the studio"
        );
    }

    #[test]
    fn save_dialogs_use_the_in_workbench_modal_and_keep_default_extensions() {
        let mut state = image_state_from_png((100, 100), "Test image");
        state.path = Some(PathBuf::from("/tmp/source.png"));

        state.open_save_dialog();
        assert!(state.show_fallback_save_modal);
        assert_eq!(state.fallback_save_path, "/tmp/source.png");

        state.open_save_dialog_with_ext("webp");
        assert!(state.show_fallback_save_modal);
        assert_eq!(state.fallback_save_path, "/tmp/source_converted.webp");
    }

    #[test]
    fn test_load_png_and_meta() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(640, 360);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([59, 130, 246, 255]);
        }
        let temp_file =
            std::env::temp_dir().join(format!("test_preview_{}.png", uuid::Uuid::new_v4()));
        img.save(&temp_file).unwrap();

        let state =
            ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load PNG");
        assert_eq!(state.current_rgba.width(), 640);
        assert_eq!(state.current_rgba.height(), 360);
        assert!(!state.is_svg);
        assert_eq!(format_aspect_ratio(640, 360), "16:9");
        let disk_path = state.ensure_disk_path().expect("ensure_disk_path failed");
        assert!(disk_path.exists());
        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_load_jpeg_and_meta() {
        let config = AppConfig::default();
        let img = image::RgbImage::new(800, 600);
        let temp_file =
            std::env::temp_dir().join(format!("test_preview_{}.jpg", uuid::Uuid::new_v4()));
        img.save(&temp_file).unwrap();

        let state =
            ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load JPEG");
        assert_eq!(state.current_rgba.width(), 800);
        assert_eq!(state.current_rgba.height(), 600);
        assert!(!state.is_svg);
        assert_eq!(format_aspect_ratio(800, 600), "4:3");
        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_load_svg_and_meta() {
        let config = AppConfig::default();
        let svg_content = r##"<svg width="400" height="400" viewBox="0 0 400 400" xmlns="http://www.w3.org/2000/svg"><rect width="400" height="400" fill="#8b5cf6"/></svg>"##;
        let temp_file =
            std::env::temp_dir().join(format!("test_preview_{}.svg", uuid::Uuid::new_v4()));
        std::fs::write(&temp_file, svg_content).unwrap();

        let state =
            ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load SVG");
        assert_eq!(state.current_rgba.width(), 400);
        assert_eq!(state.current_rgba.height(), 400);
        assert!(state.is_svg);
        assert_eq!(format_aspect_ratio(400, 400), "1:1");
        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_load_item_in_memory() {
        let config = AppConfig::default();
        let img = RgbaImage::new(320, 240);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();

        let item = ClipItem::from_image(png_bytes, 320, 240, "image/png");
        let state =
            ImagePreviewState::from_item(&item, &config).expect("Failed to load from ClipItem");
        assert_eq!(state.current_rgba.width(), 320);
        assert_eq!(state.current_rgba.height(), 240);
        let disk_path = state.ensure_disk_path().expect("ensure_disk_path failed");
        assert!(disk_path.exists());
    }

    #[test]
    fn test_crop_free() {
        let config = AppConfig::default();
        let img = RgbaImage::new(100, 100);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Set normalized crop: x from 0.1 to 0.6, y from 0.2 to 0.7 (50x50)
        state.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.6, 0.7));
        state.apply_crop();

        assert_eq!(state.current_rgba.width(), 50);
        assert_eq!(state.current_rgba.height(), 50);
        assert_eq!(state.undo_stack.len(), 1);

        // Test undo
        state.undo();
        assert_eq!(state.current_rgba.width(), 100);
        assert_eq!(state.current_rgba.height(), 100);
    }

    #[test]
    fn test_crop_aspect_ratio_presets() {
        let mut crop = CropState::default();

        // 1920x1080 (16:9) image with 1:1 ratio
        crop.set_ratio(CropRatio::Ratio1x1, 1920, 1080);
        let norm = crop.normalized_rect;
        let px_w = (norm.width() * 1920.0).round();
        let px_h = (norm.height() * 1080.0).round();
        assert_eq!(px_w, 1080.0);
        assert_eq!(px_h, 1080.0);

        // 1920x1080 with 16:9 ratio
        crop.set_ratio(CropRatio::Ratio16x9, 1920, 1080);
        let norm = crop.normalized_rect;
        let px_w = (norm.width() * 1920.0).round();
        let px_h = (norm.height() * 1080.0).round();
        assert_eq!(px_w, 1920.0);
        assert_eq!(px_h, 1080.0);

        // 1920x1080 with 9:16 ratio
        crop.set_ratio(CropRatio::Ratio9x16, 1920, 1080);
        let norm = crop.normalized_rect;
        let px_w = (norm.width() * 1920.0).round();
        let px_h = (norm.height() * 1080.0).round();
        assert_eq!(px_h, 1080.0);
        assert_eq!((px_w / px_h - 9.0 / 16.0).abs() < 0.01, true);
    }

    #[test]
    fn test_rotation_and_flipping() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(100, 200);
        // Paint top-left pixel red
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));

        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 200, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        assert_eq!(state.current_rgba.width(), 100);
        assert_eq!(state.current_rgba.height(), 200);
        assert_eq!(
            *state.current_rgba.get_pixel(0, 0),
            image::Rgba([255, 0, 0, 255])
        );

        // Rotate 90 CW: dimensions become 200x100, red pixel moves to (199, 0)
        state.rotate_cw();
        assert_eq!(state.current_rgba.width(), 200);
        assert_eq!(state.current_rgba.height(), 100);
        assert_eq!(
            *state.current_rgba.get_pixel(199, 0),
            image::Rgba([255, 0, 0, 255])
        );

        // Rotate 180: dimensions stay 200x100, red pixel moves to (0, 99)
        state.rotate_180();
        assert_eq!(state.current_rgba.width(), 200);
        assert_eq!(state.current_rgba.height(), 100);
        assert_eq!(
            *state.current_rgba.get_pixel(0, 99),
            image::Rgba([255, 0, 0, 255])
        );

        // Flip Horizontal: pixel at (0, 99) moves to (199, 99)
        state.flip_horizontal();
        assert_eq!(
            *state.current_rgba.get_pixel(199, 99),
            image::Rgba([255, 0, 0, 255])
        );

        // Flip Vertical: pixel at (199, 99) moves to (199, 0)
        state.flip_vertical();
        assert_eq!(
            *state.current_rgba.get_pixel(199, 0),
            image::Rgba([255, 0, 0, 255])
        );

        // Test Revert
        state.revert_to_original();
        assert_eq!(state.current_rgba.width(), 100);
        assert_eq!(state.current_rgba.height(), 200);
        assert_eq!(
            *state.current_rgba.get_pixel(0, 0),
            image::Rgba([255, 0, 0, 255])
        );
    }

    #[test]
    fn test_current_display_image_crop_selection() {
        let config = AppConfig::default();
        let img = RgbaImage::new(200, 200);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 200, 200, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // In View mode, display image is full 200x200
        assert_eq!(state.current_display_image().width(), 200);
        assert_eq!(state.current_display_image().height(), 200);

        // Switch to Crop mode and set crop rect to 50%
        state.active_mode = ImageToolMode::Crop;
        state.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 0.5));

        let cropped_disp = state.current_display_image();
        assert_eq!(cropped_disp.width(), 100);
        assert_eq!(cropped_disp.height(), 100);
        // Canvas image remains untouched until applied
        assert_eq!(state.current_rgba.width(), 200);
    }

    #[test]
    fn test_save_image_to_path_png_and_jpeg() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(120, 80);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([120, 180, 240, 200]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 120, 80, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        let tmp_png = std::env::temp_dir().join(format!("test_save_{}.png", uuid::Uuid::new_v4()));
        let tmp_jpg = std::env::temp_dir().join(format!("test_save_{}.jpg", uuid::Uuid::new_v4()));

        // Save as PNG
        state.save_image_to_path(&tmp_png);
        save::wait_for_save(&mut state);
        assert!(tmp_png.exists());
        let reloaded_png = image::open(&tmp_png).expect("Reload PNG failed");
        assert_eq!(reloaded_png.width(), 120);
        assert_eq!(reloaded_png.height(), 80);
        assert_eq!(state.mime, "image/png");

        // Save as JPEG (exercises RGBA -> RGB flattening)
        state.save_image_to_path(&tmp_jpg);
        save::wait_for_save(&mut state);
        assert!(tmp_jpg.exists());
        let reloaded_jpg = image::open(&tmp_jpg).expect("Reload JPG failed");
        assert_eq!(reloaded_jpg.width(), 120);
        assert_eq!(reloaded_jpg.height(), 80);
        assert_eq!(state.mime, "image/jpeg");

        let _ = std::fs::remove_file(tmp_png);
        let _ = std::fs::remove_file(tmp_jpg);
    }

    #[test]
    fn test_redaction_placement_and_burn_in() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(100, 100);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([255, 255, 255, 255]); // white canvas
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Place redaction box covering x: [0.2, 0.6], y: [0.3, 0.7] (pixels 20..60, 30..70)
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.3), Pos2::new(0.6, 0.7)),
        });

        // Test that current_display_image burns pending redactions non-destructively
        let disp_img = state.current_display_image();
        assert_eq!(*disp_img.get_pixel(40, 50), image::Rgba([0, 0, 0, 255]));
        assert_eq!(
            *disp_img.get_pixel(10, 10),
            image::Rgba([255, 255, 255, 255])
        );
        assert_eq!(
            *state.current_rgba.get_pixel(40, 50),
            image::Rgba([255, 255, 255, 255])
        ); // canvas untouched yet

        // Burn / apply redactions permanently
        state.apply_redactions();
        assert_eq!(
            *state.current_rgba.get_pixel(40, 50),
            image::Rgba([0, 0, 0, 255])
        ); // burned
        assert_eq!(
            *state.current_rgba.get_pixel(10, 10),
            image::Rgba([255, 255, 255, 255])
        ); // untouched
        assert_eq!(state.redaction_state.boxes.len(), 0);
        assert_eq!(state.undo_stack.len(), 1);

        // Undo restoration
        state.undo();
        assert_eq!(
            *state.current_rgba.get_pixel(40, 50),
            image::Rgba([255, 255, 255, 255])
        );
    }

    #[test]
    fn test_redaction_delete_and_clear() {
        let config = AppConfig::default();
        let img = RgbaImage::new(100, 100);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.3, 0.3)),
        });
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.5, 0.5), Pos2::new(0.8, 0.8)),
        });
        assert_eq!(state.redaction_state.boxes.len(), 2);

        // Select and delete box 0
        state.redaction_state.selected_idx = Some(0);
        state.delete_selected_redaction();
        assert_eq!(state.redaction_state.boxes.len(), 1);
        assert!(state.redaction_state.selected_idx.is_none());

        // Clear all
        state.clear_all_redactions();
        assert_eq!(state.redaction_state.boxes.len(), 0);
    }

    #[test]
    fn test_color_picker_hex_and_recent() {
        let mut pick_state = PickColorState::default();
        let c1 = Color32::from_rgb(168, 85, 247);
        let c2 = Color32::from_rgb(56, 189, 248);

        let hex1 = format!("#{:02X}{:02X}{:02X}", c1.r(), c1.g(), c1.b());
        assert_eq!(hex1, "#A855F7");

        let hex2 = format!("#{:02X}{:02X}{:02X}", c2.r(), c2.g(), c2.b());
        assert_eq!(hex2, "#38BDF8");

        pick_state.recent_colors.push(c1);
        pick_state.recent_colors.push(c2);
        assert_eq!(pick_state.recent_colors.len(), 2);
    }

    #[test]
    fn pick_color_hover_renders_loupe_and_click_records_sampled_pixel() {
        let expected = Color32::from_rgb(230, 83, 122);
        let palette =
            AppPalette::for_appearance(little_clip_core::config::WorkbenchAppearance::Light);
        for viewport_size in [Vec2::new(400.0, 300.0), Vec2::new(1200.0, 800.0)] {
            let mut state = image_state_from_png((8, 8), "color picker interaction");
            state.current_rgba = Arc::new(RgbaImage::from_pixel(
                8,
                8,
                image::Rgba([expected.r(), expected.g(), expected.b(), 255]),
            ));
            let ctx = egui::Context::default();
            let render = |state: &mut ImagePreviewState, time, events| {
                let mut canvas_bounds = None;
                let output = ctx.run(
                    egui::RawInput {
                        time: Some(time),
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, viewport_size)),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let (response, painter) = ui.allocate_painter(
                                ui.available_size(),
                                egui::Sense::click_and_drag(),
                            );
                            let bounds = response.rect;
                            let sample_area = bounds.shrink(12.0);
                            state.render_pick_color_overlay(
                                ui,
                                &painter,
                                &response,
                                sample_area,
                                bounds,
                                &palette,
                            );
                            canvas_bounds = Some(bounds);
                        });
                    },
                );
                (output, canvas_bounds.expect("canvas bounds were allocated"))
            };

            let (_, bounds) = render(&mut state, 0.0, Vec::new());
            let sample_area = bounds.shrink(12.0);
            let pointer = sample_area.center();
            let (hover, _) = render(&mut state, 0.1, vec![egui::Event::PointerMoved(pointer)]);
            assert_eq!(state.pick_color_state.hovered_color, Some(expected));
            assert_eq!(state.pick_color_state.hovered_pixel, Some((4, 4)));
            let readout_rects: Vec<_> = hover
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some((
                        text.galley.text().to_owned(),
                        Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    _ => None,
                })
                .collect();
            for expected_line in ["#E6537A", "R230 G83 B122", "X: 4  Y: 4", "Click to keep"] {
                let readout = readout_rects
                    .iter()
                    .find(|(text, _)| text == expected_line)
                    .map(|(_, rect)| *rect)
                    .unwrap_or_else(|| panic!("loupe renders {expected_line}"));
                assert!(
                    bounds.contains_rect(readout),
                    "readout {expected_line} at {readout:?} must stay within {bounds:?} for {viewport_size:?}"
                );
            }
            let card_rect = hover.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if (rect.stroke.width - 1.5).abs() < f32::EPSILON
                        && rect.rect.width() >= 92.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            });
            let card_rect = card_rect.expect("loupe card border is rendered");
            assert!(
                bounds.contains_rect(card_rect),
                "loupe card {card_rect:?} must stay within {bounds:?} for {viewport_size:?}"
            );
            assert!(
                readout_rects
                    .iter()
                    .all(|(_, rect)| card_rect.contains_rect(*rect))
            );

            render(
                &mut state,
                0.2,
                vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
            render(
                &mut state,
                0.3,
                vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
            assert_eq!(state.pick_color_state.picked_colors, vec![expected]);
            assert_eq!(state.pick_color_state.last_picked, Some(expected));
        }
    }

    #[test]
    fn test_pixel_resize_aspect_locked_and_unlocked() {
        let config = AppConfig::default();
        let img = RgbaImage::new(200, 100);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 200, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        assert_eq!(state.current_rgba.width(), 200);
        assert_eq!(state.current_rgba.height(), 100);

        // Aspect ratio is 2.0 (2:1). With lock enabled:
        state.resize_state.lock_aspect = true;
        state.resize_state.aspect_ratio_base = 2.0;

        // Setting width to 100 should calculate height to 50
        state.resize_state.set_width(100);
        assert_eq!(state.resize_state.target_width, 100);
        assert_eq!(state.resize_state.target_height, 50);

        // Setting height to 150 should calculate width to 300
        state.resize_state.set_height(150);
        assert_eq!(state.resize_state.target_width, 300);
        assert_eq!(state.resize_state.target_height, 150);

        // Unlock aspect ratio
        state.resize_state.lock_aspect = false;
        state.resize_state.set_width(75);
        assert_eq!(state.resize_state.target_width, 75);
        assert_eq!(state.resize_state.target_height, 150); // height unchanged

        // Apply pixel resize to 75x150
        state.apply_pixel_resize(75, 150, ResizeFilter::Lanczos3);
        wait_for_pixel_resize(&mut state);
        assert_eq!(state.current_rgba.width(), 75);
        assert_eq!(state.current_rgba.height(), 150);
        assert_eq!(state.undo_stack.len(), 1);

        // Test Undo
        state.undo();
        assert_eq!(state.current_rgba.width(), 200);
        assert_eq!(state.current_rgba.height(), 100);
    }

    #[test]
    fn test_target_file_size_optimizer_jpeg() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(400, 300);
        for (i, pixel) in img.pixels_mut().enumerate() {
            let r = (i % 255) as u8;
            let g = ((i * 3) % 255) as u8;
            let b = ((i * 7) % 255) as u8;
            *pixel = image::Rgba([r, g, b, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 400, 300, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Target: 20 KB (20,480 bytes)
        let target_bytes = 20 * 1024;
        state.resize_state.target_size_val = 20.0;
        state.resize_state.target_size_unit_mb = false;
        state.resize_state.target_format = TargetFormat::Jpeg;

        state.run_target_file_size_optimizer();
        // Poll for async optimizer completion
        for _ in 0..500 {
            state.poll_optimizer();
            if !state.resize_state.is_optimizing && state.resize_state.last_result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let opt_res = state
            .resize_state
            .last_result
            .clone()
            .expect("Optimization result missing");

        assert!(
            opt_res.achieved_bytes <= target_bytes,
            "Achieved {} > target {}",
            opt_res.achieved_bytes,
            target_bytes
        );
        assert!(opt_res.width <= 400);
        assert!(opt_res.height <= 300);
        assert!(!opt_res.encoded_data.is_empty());

        // Apply optimization
        state.apply_optimized_result();
        assert_eq!(state.current_rgba.width(), opt_res.width);
        assert_eq!(state.current_rgba.height(), opt_res.height);
        assert_eq!(state.undo_stack.len(), 1);

        // Undo
        state.undo();
        assert_eq!(state.current_rgba.width(), 400);
        assert_eq!(state.current_rgba.height(), 300);
    }

    #[test]
    fn test_target_file_size_optimizer_progress_callback() {
        let mut img = RgbaImage::new(200, 150);
        for (i, pixel) in img.pixels_mut().enumerate() {
            let r = (i % 255) as u8;
            let g = ((i * 5) % 255) as u8;
            let b = ((i * 11) % 255) as u8;
            *pixel = image::Rgba([r, g, b, 255]);
        }
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        let cb = move |msg: String| {
            captured_clone.lock().unwrap().push(msg);
        };

        let target_bytes = 10 * 1024;
        let res = optimize_to_target_size_with_progress(
            &img,
            target_bytes,
            TargetFormat::Jpeg,
            Some(&cb),
        );
        assert!(res.achieved_bytes <= target_bytes + 2048);
        let list = captured.lock().unwrap();
        assert!(
            !list.is_empty(),
            "Progress messages should have been captured"
        );
        assert!(
            list.iter().any(|m| m.contains("Testing full resolution")
                || m.contains("Trying quality")
                || m.contains("Complete")),
            "Expected recognizable progress messages: {:?}",
            list
        );
    }

    #[test]
    fn test_target_file_size_optimizer_png() {
        let mut img = RgbaImage::new(300, 300);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([100, 150, 200, 255]);
        }

        // Target: 10 KB
        let target_bytes = 10 * 1024;
        let res = optimize_to_target_size(&img, target_bytes, TargetFormat::Png);

        assert!(
            res.achieved_bytes <= target_bytes,
            "Achieved {} > target {}",
            res.achieved_bytes,
            target_bytes
        );
        assert!(res.width <= 300);
        assert!(res.height <= 300);
    }

    #[test]
    fn test_palette_extraction_colors_and_percentages() {
        // Create 100x100 image: left half pure Red, right half pure Blue
        let mut img = RgbaImage::new(100, 100);
        for x in 0..100 {
            for y in 0..100 {
                if x < 50 {
                    img.put_pixel(x, y, image::Rgba([255, 0, 0, 255])); // Red
                } else {
                    img.put_pixel(x, y, image::Rgba([0, 0, 255, 255])); // Blue
                }
            }
        }

        let (palette, total_sampled) = extract_palette(&img, 8);
        assert!(total_sampled > 0);
        assert!(palette.len() >= 2);

        let hex_codes: Vec<String> = palette.iter().map(|p| p.hex.clone()).collect();
        assert!(
            hex_codes.iter().any(|h| h == "#FF0000"),
            "Red should be in palette: {:?}",
            hex_codes
        );
        assert!(
            hex_codes.iter().any(|h| h == "#0000FF"),
            "Blue should be in palette: {:?}",
            hex_codes
        );

        // Each half should have approximately 50% representation
        let red_pal = palette.iter().find(|p| p.hex == "#FF0000").unwrap();
        let blue_pal = palette.iter().find(|p| p.hex == "#0000FF").unwrap();
        assert!(
            (red_pal.percentage - 50.0).abs() < 5.0,
            "Red % was {}",
            red_pal.percentage
        );
        assert!(
            (blue_pal.percentage - 50.0).abs() < 5.0,
            "Blue % was {}",
            blue_pal.percentage
        );
    }

    #[test]
    fn test_bg_removal_worker_invocation_and_undo() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(100, 100);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([200, 100, 50, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Create a temporary mock background removal script
        let mock_script = "import sys, io\nfrom PIL import Image\nimg = Image.open(io.BytesIO(sys.stdin.buffer.read())).convert('RGBA')\nout = Image.new('RGBA', img.size)\npixels = [(r, g, b, 0 if x < 50 else 255) for y in range(img.height) for x in range(img.width) for (r, g, b, a) in [img.getpixel((x, y))]]\nout.putdata(pixels)\nbuf = io.BytesIO()\nout.save(buf, format='PNG')\nsys.stdout.buffer.write(buf.getvalue())\n";
        let script_path = std::env::temp_dir().join("mock_nobg_worker.py");
        std::fs::write(&script_path, mock_script).unwrap();
        state.bg_removal_cmd = Some(format!("python3 {}", script_path.display()));

        state.run_background_removal();
        assert!(state.bg_removal_in_progress);

        for _ in 0..100 {
            state.poll_bg_removal();
            if !state.bg_removal_in_progress {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(!state.bg_removal_in_progress);
        assert!(
            state.bg_removal_error.is_none(),
            "Error: {:?}",
            state.bg_removal_error
        );
        assert_eq!(state.undo_stack.len(), 1);

        // Verify transparent pixel was applied on left half
        assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 0);
        assert_eq!(state.current_rgba.get_pixel(60, 60)[3], 255);
        let cached_removal = state
            .creative_fx_state
            .cached_background_removal
            .as_ref()
            .expect("Remove BG result should be cached for Creative FX");
        assert_eq!(cached_removal.as_raw(), state.current_rgba.as_raw());

        // Test Undo
        state.undo();
        assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 255);
        assert_eq!(state.undo_stack.len(), 0);

        // Test Redo
        state.redo();
        assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 0);
    }

    #[cfg(unix)]
    #[test]
    fn background_removal_process_is_terminated_for_stale_input_mode_exit_and_drop() {
        fn start_sleeping_job(
            state: &mut ImagePreviewState,
        ) -> (
            std::sync::Arc<std::sync::atomic::AtomicBool>,
            u32,
            std::path::PathBuf,
        ) {
            let script =
                std::env::temp_dir().join(format!("clip-bg-sleep-{}.py", std::process::id()));
            std::fs::write(
                &script,
                "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
            )
            .unwrap();
            state.bg_removal_cmd = Some(format!("python3 {}", script.display()));
            state.set_tool_mode(ImageToolMode::RemoveBg);
            state.run_background_removal();
            let cancel = state.image_tool_cancel.as_ref().unwrap().clone();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while state
                .image_tool_child_pid
                .load(std::sync::atomic::Ordering::SeqCst)
                == 0
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_ne!(
                state
                    .image_tool_child_pid
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            let pid = state
                .image_tool_child_pid
                .load(std::sync::atomic::Ordering::SeqCst);
            (cancel, pid, script)
        }

        fn assert_process_exited(pid: u32) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                let result = unsafe { libc::kill(pid as i32, 0) };
                if result == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("background-removal child {pid} remained alive after cancellation");
        }

        let mut stale = image_state_from_png((8, 8), "cancel stale background");
        let (stale_cancel, stale_pid, stale_script) = start_sleeping_job(&mut stale);
        stale.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::ZERO,
            text: "changed".into(),
            font_size: 10.0,
        });
        stale.poll_bg_removal();
        assert!(stale_cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_process_exited(stale_pid);
        assert_eq!(
            stale
                .image_tool_child_pid
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        drop(stale);
        let _ = std::fs::remove_file(stale_script);

        let mut mode_exit = image_state_from_png((8, 8), "cancel background mode");
        let (mode_cancel, mode_pid, mode_script) = start_sleeping_job(&mut mode_exit);
        mode_exit.set_tool_mode(ImageToolMode::View);
        assert!(mode_cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_process_exited(mode_pid);
        assert!(!mode_exit.bg_removal_in_progress);
        drop(mode_exit);
        let _ = std::fs::remove_file(mode_script);

        let mut viewer_drop = image_state_from_png((8, 8), "drop background viewer");
        let (drop_cancel, drop_pid, drop_script) = start_sleeping_job(&mut viewer_drop);
        drop(viewer_drop);
        assert!(drop_cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_process_exited(drop_pid);
        let _ = std::fs::remove_file(drop_script);
    }

    #[test]
    fn test_copy_to_hand_slot_dispatch() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(50, 50);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([10, 20, 30, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 50, 50, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.copy_to_hand_slot(HandSide::Left);
        assert!(state.status_toast.is_some());
        state.copy_to_hand_slot(HandSide::Right);
        assert!(state.status_toast.is_some());
    }

    #[test]
    fn test_convert_state_png_jpeg_webp_and_optipng() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(40, 40);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgba([((x * 6) % 255) as u8, ((y * 6) % 255) as u8, 128, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 40, 40, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // 1. Conversion is lazy so loading an image never performs an eager encode.
        assert_eq!(state.convert_state.target_format, ConvertTargetFormat::Webp);
        assert!(state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_none());

        // Exercise each encoder synchronously here; the UI dispatches the same work to its worker.
        state.update_conversion();
        assert!(!state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_some());

        // 2. Switch to PNG and test OptiPNG
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.convert_state.lossless = true;
        state.update_conversion();
        assert!(!state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_some());

        state.run_optipng_conversion();
        assert!(!state.convert_state.encoded_bytes.is_empty());

        // 3. Switch to JPEG lossy
        state.convert_state.target_format = ConvertTargetFormat::Jpeg;
        state.convert_state.lossless = false;
        state.convert_state.quality = 75;
        state.update_conversion();
        assert!(!state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_some());

        // 4. Switch to WebP lossless
        state.convert_state.target_format = ConvertTargetFormat::Webp;
        state.convert_state.lossless = true;
        state.update_conversion();
        assert!(!state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_some());

        // 5. Switch to WebP lossy
        state.convert_state.lossless = false;
        state.convert_state.quality = 60;
        state.update_conversion();
        assert!(!state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_some());

        // 6. Apply converted to canvas and verify undo stack
        state.apply_converted_to_canvas();
        assert_eq!(state.undo_stack.len(), 1);
    }

    #[test]
    fn conversion_worker_coalesces_changes_and_rejects_stale_results() {
        let config = AppConfig::default();
        let mut image = RgbaImage::new(96, 96);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([(x * 2) as u8, (y * 2) as u8, (x ^ y) as u8, 255]);
        }
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let item = ClipItem::from_image(png, 96, 96, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();
        let initial_bytes = state.convert_state.encoded_bytes.clone();

        state.convert_state.target_format = ConvertTargetFormat::Jpeg;
        state.convert_state.lossless = false;
        state.convert_state.quality = 20;
        state.schedule_conversion();
        assert!(state.convert_state.is_pending());
        assert_eq!(state.convert_state.encoded_bytes, initial_bytes);

        state.run_conversion_worker();
        state.convert_state.quality = 90;
        state.schedule_conversion();
        let latest_generation = state.convert_state.request_generation;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.convert_state.is_encoding && std::time::Instant::now() < deadline {
            state.poll_conversion();
            std::thread::yield_now();
        }
        assert!(!state.convert_state.output_is_current());
        assert_eq!(state.convert_state.encoded_bytes, initial_bytes);

        state.run_conversion_worker();
        let latest_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.convert_state.is_encoding && std::time::Instant::now() < latest_deadline {
            state.poll_conversion();
            std::thread::yield_now();
        }
        assert!(state.convert_state.output_is_current());
        assert_eq!(state.convert_state.completed_generation, latest_generation);
        assert_ne!(state.convert_state.encoded_bytes, initial_bytes);
    }

    #[test]
    fn test_barcode_state_and_scan() {
        let config = AppConfig::default();
        let writer = rxing::qrcode::QRCodeWriter;
        use rxing::Writer;
        for payload in ["https://github.com", "clipboard test 日本語 ✓"] {
            let bit_matrix = writer
                .encode(payload, &rxing::BarcodeFormat::QR_CODE, 96, 96)
                .unwrap();

            let mut img = RgbaImage::new(96, 96);
            for y in 0..96 {
                for x in 0..96 {
                    let color = if bit_matrix.get(x, y) {
                        image::Rgba([0, 0, 0, 255])
                    } else {
                        image::Rgba([255, 255, 255, 255])
                    };
                    img.put_pixel(x as u32, y as u32, color);
                }
            }

            let mut png_bytes = Vec::new();
            img.write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
            let item = ClipItem::from_image(png_bytes, 96, 96, "image/png");
            let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

            state.run_barcode_scan();
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !state.barcode_state.has_scanned && std::time::Instant::now() < deadline {
                state.poll_barcode_scan(&ctx);
                std::thread::yield_now();
            }
            assert!(state.barcode_state.has_scanned);
            assert_eq!(state.barcode_state.detections.len(), 1);
            assert_eq!(state.barcode_state.detections[0].text, payload);
        }
    }

    #[test]
    fn barcode_copy_actions_emit_exact_payloads() {
        let mut state = image_state_from_png((8, 8), "barcode copy actions");
        let payloads = ["https://example.com", "clipboard 日本語 ✓"];
        state.barcode_state = BarcodeScanState {
            detections: payloads
                .iter()
                .map(|text| BarcodeDetection {
                    format: "QR Code".to_owned(),
                    text: (*text).to_owned(),
                    points: vec![],
                    bbox: [0.0; 4],
                    is_url: text.starts_with("https://"),
                })
                .collect(),
            selected_idx: Some(0),
            is_scanning: false,
            has_scanned: true,
            last_scan_duration_ms: 3,
        };

        let palette =
            AppPalette::for_appearance(little_clip_core::config::WorkbenchAppearance::Dark);
        let ctx = egui::Context::default();
        let render = |state: &mut ImagePreviewState, time, events| {
            ctx.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 500.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_barcode_toolbar(ui, ctx, &palette);
                    });
                },
            )
        };
        let text_center = |output: &egui::FullOutput, expected: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == expected => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("rendered action text {expected:?} not found"))
        };
        let click = |state: &mut ImagePreviewState, pos: Pos2, time: f64| {
            render(state, time, vec![egui::Event::PointerMoved(pos)]);
            render(
                state,
                time + 0.1,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
            render(
                state,
                time + 0.2,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            )
        };

        render(&mut state, 0.0, vec![]);
        let initial = render(&mut state, 0.1, vec![]);
        let all_pos = text_center(&initial, "📋 Copy All");
        let copied_all = click(&mut state, all_pos, 0.2);
        assert_eq!(copied_all.platform_output.copied_text, payloads.join("\n"));

        let after_copy_all = render(&mut state, 0.5, vec![]);
        let selected_pos = text_center(&after_copy_all, "📋");
        let copied_selected = click(&mut state, selected_pos, 0.6);
        assert_eq!(copied_selected.platform_output.copied_text, payloads[0]);

        let with_url_action = render(&mut state, 0.9, vec![]);
        let open_url_pos = text_center(&with_url_action, "🌐 Open");
        let opened = click(&mut state, open_url_pos, 1.0);
        let open_url = opened
            .platform_output
            .open_url
            .expect("clicking a URL result requests opening its link");
        assert_eq!(open_url.url, payloads[0]);
        assert!(open_url.new_tab);

        state.barcode_state.detections = vec![BarcodeDetection {
            format: "QR Code".to_owned(),
            text: payloads[1].to_owned(),
            points: vec![],
            bbox: [0.0; 4],
            is_url: false,
        }];
        let text_only = render(&mut state, 1.3, vec![]);
        assert!(
            text_only
                .shapes
                .iter()
                .all(|shape| !matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "🌐 Open")),
            "non-URL payloads must not render an Open action"
        );
        assert!(text_only.platform_output.open_url.is_none());
    }

    #[test]
    fn test_ocr_state_transitions() {
        let config = AppConfig::default();
        let img = RgbaImage::new(20, 20);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
        let state = ImagePreviewState::from_item(&item, &config).unwrap();

        assert_eq!(state.ocr_state.engine, OcrEngine::GlmOcr);
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.extracted_text.is_empty());

        // Test engine enum conversions
        assert_eq!(OcrEngine::GlmOcr.arg_name(), "glm-ocr");
        assert_eq!(OcrEngine::Tesseract.arg_name(), "tesseract");
    }

    #[test]
    fn test_ocr_cancellation_and_cleanup() {
        let config = AppConfig::default();
        let img = RgbaImage::new(20, 20);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.ocr_state.in_progress = true;
        state.ocr_state.status_text = Some("Processing image with GLM-OCR...".into());
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        control
            .child_pid
            .store(999999, std::sync::atomic::Ordering::SeqCst);
        state.ocr_process_control = Some(control);

        // Cancel OCR
        state.cancel_ocr();

        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.status_text.is_none());
        assert_eq!(
            state.status_toast.as_ref().map(|toast| toast.0.as_str()),
            Some("⏹ OCR process cancelled")
        );
        assert!(state.ocr_process_control.is_none());
        assert!(state.ocr_rx.is_none());

        // Drop state test - ensuring drop executes cancel_ocr without panic
        {
            let mut scoped_state = ImagePreviewState::from_item(&item, &config).unwrap();
            scoped_state.ocr_state.in_progress = true;
            // Dropping scoped_state here invokes Drop -> cancel_ocr
        }
    }

    #[test]
    fn test_tool_mode_labels_and_shortcuts() {
        assert_eq!(ImageToolMode::View.label(), "[V]iew");
        assert_eq!(ImageToolMode::Crop.label(), "[C]rop");
        assert_eq!(ImageToolMode::Deskew.label(), "Des[K]ew");
        assert_eq!(ImageToolMode::Transform.label(), "[F]lip/Rotate");
        assert_eq!(ImageToolMode::Redact.label(), "[X] Redact");
        assert_eq!(ImageToolMode::PickColor.label(), "[P]ick Color");
        assert_eq!(ImageToolMode::Resize.label(), "Re[S]ize");
        assert_eq!(ImageToolMode::Convert.label(), "Conver[T]");
        assert_eq!(ImageToolMode::Ocr.label(), "[O]CR");
        assert_eq!(ImageToolMode::Barcode.label(), "[B]arcode");
        assert_eq!(ImageToolMode::RemoveBg.label(), "[R]emove BG");
        assert_eq!(ImageToolMode::DepthMap.label(), "[D]epth Map");
        assert_eq!(
            ImageToolMode::DecorrelationStretch.label(),
            "Decorrelation Stretch"
        );
    }

    #[test]
    fn test_set_tool_mode_integration() {
        let config = AppConfig::default();
        let img = RgbaImage::new(20, 20);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Switching to Palette routes to PickColor and auto-runs palette extraction
        state.set_tool_mode(ImageToolMode::Palette);
        assert_eq!(state.active_mode, ImageToolMode::PickColor);
        assert!(state.palette_state.is_extracting);

        // Switching to Crop
        state.set_tool_mode(ImageToolMode::Crop);
        assert_eq!(state.active_mode, ImageToolMode::Crop);

        // Switching to Deskew
        state.set_tool_mode(ImageToolMode::Deskew);
        assert_eq!(state.active_mode, ImageToolMode::Deskew);

        // Switching to Transform
        state.set_tool_mode(ImageToolMode::Transform);
        assert_eq!(state.active_mode, ImageToolMode::Transform);

        // Switching to DecorrelationStretch
        state.set_tool_mode(ImageToolMode::DecorrelationStretch);
        assert_eq!(state.active_mode, ImageToolMode::DecorrelationStretch);
        wait_for_dcs(&mut state);
        assert!(state.dcs_state.stretched_rgba.is_some());

        // Switching from OCR in progress cancels OCR
        state.set_tool_mode(ImageToolMode::Ocr);
        assert_eq!(state.active_mode, ImageToolMode::Ocr);
        state.ocr_state.in_progress = true;
        state.ocr_process_control = Some(std::sync::Arc::new(ImageToolProcessControl::new()));
        state.set_tool_mode(ImageToolMode::View);
        assert_eq!(state.active_mode, ImageToolMode::View);
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_process_control.is_none());
    }

    #[test]
    fn thumbnail_batches_cancel_on_mode_exit_and_source_change() {
        let config = AppConfig::default();
        let image = RgbaImage::from_pixel(42, 28, image::Rgba([40, 80, 120, 255]));
        let mut png_bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.set_tool_mode(ImageToolMode::FilmSim);
        let film_cancel = state
            .film_sim_state
            .thumbnail_cancel
            .as_ref()
            .expect("Film Look batch started")
            .clone();
        state.set_tool_mode(ImageToolMode::View);
        assert!(film_cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.film_sim_state.is_generating_thumbnails);
        assert!(state.film_sim_state.thumbnail_rx.is_none());

        state.set_tool_mode(ImageToolMode::CreativeFx);
        let fx_cancel = state
            .creative_fx_state
            .thumbnail_cancel
            .as_ref()
            .expect("Creative FX batch started")
            .clone();
        state.mark_pixels_changed();
        assert!(fx_cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.creative_fx_state.is_generating_thumbnails);
        assert!(state.creative_fx_state.thumbnail_rx.is_none());
        assert!(state.creative_fx_state.pending_thumbnails.is_empty());
        assert!(state.creative_fx_state.fx_thumbnails.is_empty());
    }

    #[test]
    fn dropping_image_preview_cancels_both_thumbnail_batches() {
        let mut state = image_state_from_png((42, 28), "thumbnail drop test");
        state.run_thumbnail_batch();
        state.run_creative_fx_thumbnail_batch();
        let film_cancel = state
            .film_sim_state
            .thumbnail_cancel
            .as_ref()
            .expect("Film Look batch started")
            .clone();
        let fx_cancel = state
            .creative_fx_state
            .thumbnail_cancel
            .as_ref()
            .expect("Creative FX batch started")
            .clone();

        drop(state);

        assert!(film_cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(fx_cancel.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn decorrelation_stretch_apply_undo_and_redo_restore_image_pixels() {
        let config = AppConfig::default();
        let image = RgbaImage::from_fn(16, 16, |x, y| {
            image::Rgba([
                (x * 11 + y * 3) as u8,
                (y * 13 + x * 5) as u8,
                ((x + y) * 7) as u8,
                255,
            ])
        });
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            png,
            "image/png".into(),
            None,
            "DCS undo redo test".into(),
            &config,
        )
        .unwrap();
        let original = state.current_rgba.clone();
        assert!(
            (0..3).all(|channel| {
                let min = image.pixels().map(|pixel| pixel[channel]).min().unwrap();
                let max = image.pixels().map(|pixel| pixel[channel]).max().unwrap();
                min < max
            }),
            "fixture must have nonzero variance in every RGB channel"
        );

        state.update_decorrelation_stretch();
        wait_for_dcs(&mut state);
        let stretched = state
            .dcs_state
            .stretched_rgba
            .clone()
            .expect("analysis creates a stretched result");
        assert_ne!(
            stretched, *original,
            "stretch result changes the fixture pixels"
        );

        state.apply_decorrelation_stretch();
        let applied = state.current_rgba.clone();
        assert_eq!(*applied, stretched);
        assert_ne!(applied, original);

        state.undo();
        assert_eq!(
            state.current_rgba, original,
            "undo restores pre-stretch pixels"
        );

        state.redo();
        assert_eq!(
            state.current_rgba, applied,
            "redo restores stretched pixels"
        );
    }

    #[test]
    fn test_metadata_inspection_and_stripping() {
        let config = AppConfig::default();
        let img = RgbaImage::new(30, 30);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 30, 30, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Initially no metadata in plain generated image
        assert!(state.metadata.is_none());

        // Test stripping metadata updates raw_bytes, pushes to undo_stack, and clears metadata
        state.strip_metadata();
        assert!(state.metadata.is_none());
        assert_eq!(state.undo_stack.len(), 1);
        assert!(state.status_toast.is_some());
        let toast_msg = &state.status_toast.as_ref().unwrap().0;
        assert!(toast_msg.contains("Stripped all EXIF"));
    }

    #[test]
    fn test_perspective_deskew_apply() {
        let config = AppConfig::default();
        let img = RgbaImage::new(40, 40);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 40, 40, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.set_tool_mode(ImageToolMode::Deskew);
        assert_eq!(state.active_mode, ImageToolMode::Deskew);

        // Apply deskew on default quad
        state.apply_deskew();
        assert_eq!(state.undo_stack.len(), 1);
        assert!(state.status_toast.is_some());
        let toast_msg = &state.status_toast.as_ref().unwrap().0;
        assert!(toast_msg.contains("Deskewed perspective"));
    }

    #[test]
    fn test_perspective_deskew_crop_modes_and_point_placement() {
        let config = AppConfig::default();
        let img = RgbaImage::new(60, 60);
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 60, 60, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.set_tool_mode(ImageToolMode::Deskew);
        assert_eq!(state.deskew_state.crop_mode, DeskewCropMode::CropToQuad);
        assert!(!state.deskew_state.is_clicking_points);

        // Test MinimalCrop mode execution
        state.deskew_state.crop_mode = DeskewCropMode::MinimalCrop;
        state.apply_deskew();
        assert_eq!(state.undo_stack.len(), 1);
        let toast_msg = &state.status_toast.as_ref().unwrap().0;
        assert!(toast_msg.contains("minimal crop"));

        // Test 4-point placement flow
        state.deskew_state.start_clicking_points();
        assert!(state.deskew_state.is_clicking_points);
        assert_eq!(state.deskew_state.placed_points.len(), 0);

        // While placing points, apply_deskew warns and returns early
        state.apply_deskew();
        let warn_msg = &state.status_toast.as_ref().unwrap().0;
        assert!(warn_msg.contains("Place all 4 points first"));

        // Place points sequentially: TL -> TR -> BR -> BL
        state.deskew_state.placed_points.push(Pos2::new(0.1, 0.1));
        state.deskew_state.placed_points.push(Pos2::new(0.85, 0.15));
        state.deskew_state.placed_points.push(Pos2::new(0.9, 0.85));
        state.deskew_state.placed_points.push(Pos2::new(0.15, 0.9));

        // Complete 4-point set
        state.deskew_state.corners = [
            state.deskew_state.placed_points[0],
            state.deskew_state.placed_points[1],
            state.deskew_state.placed_points[2],
            state.deskew_state.placed_points[3],
        ];
        state.deskew_state.is_clicking_points = false;
        state.deskew_state.placed_points.clear();
        state.deskew_state.corners_adjusted = true;

        assert_eq!(state.deskew_state.corners[0], Pos2::new(0.1, 0.1));
        assert_eq!(state.deskew_state.corners[2], Pos2::new(0.9, 0.85));

        // Reset quad reverts corners and adjusted flag
        state.deskew_state.reset_quad();
        assert!(!state.deskew_state.corners_adjusted);
        assert_eq!(state.deskew_state.corners[0], Pos2::new(0.08, 0.08));
    }

    #[test]
    fn test_film_sim_stocks_and_styles() {
        let stocks = all_film_stocks();
        assert_eq!(stocks.len(), 57, "Expected 57 presets across all brands");

        // Verify key presets exist
        assert!(
            stocks
                .iter()
                .any(|s| s.name == "Portra 400" && s.brand == "Kodak")
        );
        assert!(
            stocks
                .iter()
                .any(|s| s.name == "Velvia 50" && s.brand == "Fujifilm")
        );
        assert!(
            stocks
                .iter()
                .any(|s| s.name == "HP5 Plus 400" && s.brand == "Ilford")
        );
        assert!(
            stocks
                .iter()
                .any(|s| s.name == "600 Color" && s.brand == "Polaroid")
        );
        assert!(
            stocks
                .iter()
                .any(|s| s.name == "800T" && s.brand == "CineStill")
        );
        assert!(
            stocks
                .iter()
                .any(|s| s.name == "Vista 200" && s.brand == "Agfa")
        );

        // Verify each stock builds valid FilmStock
        for entry in &stocks {
            let stock = (entry.build)();
            assert!(stock.resolution_lp_mm > 0.0);
        }

        // Test style enum variants
        let styles = [
            FilmStyle::Accurate,
            FilmStyle::Artistic,
            FilmStyle::Vintage,
            FilmStyle::HighContrast,
            FilmStyle::Pastel,
        ];
        assert_eq!(styles.len(), 5);
    }

    #[test]
    fn test_film_sim_state_and_render_dispatch() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(16, 16);
        for p in img.pixels_mut() {
            *p = image::Rgba([120, 140, 160, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 16, 16, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Defaults
        assert_eq!(state.film_sim_state.brightness, 1.0);
        assert_eq!(state.film_sim_state.contrast, 0.0);
        assert_eq!(state.film_sim_state.warmth, 0.0);
        assert_eq!(state.film_sim_state.intensity, 1.0);
        assert!(state.film_sim_state.enable_grain);
        assert_eq!(state.film_sim_state.style, FilmStyle::Accurate);

        // Switch to FilmSim mode
        state.set_tool_mode(ImageToolMode::FilmSim);
        assert_eq!(state.active_mode, ImageToolMode::FilmSim);

        // Trigger render
        state.run_film_sim_render();
        assert!(state.film_sim_state.is_rendering);
        assert!(state.film_sim_state.render_rx.is_some());

        // Poll until background render finishes (should be sub-second for 16x16)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while state.film_sim_state.is_rendering && std::time::Instant::now() < deadline {
            state.poll_film_sim();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(!state.film_sim_state.is_rendering);
        assert!(state.film_sim_state.preview_rgba.is_some());
        assert_eq!(state.film_sim_state.render_error, None);

        // Apply film sim
        let pre_undo_len = state.undo_stack.len();
        state.apply_film_sim();
        assert_eq!(state.undo_stack.len(), pre_undo_len + 1);
        assert!(state.dirty_texture);
        assert!(state.status_toast.is_some());
    }

    #[test]
    fn test_film_sim_contrast_curve() {
        let compute_lut = |contrast: f32| -> [u8; 256] {
            let k = if contrast >= 0.0 {
                1.0 + contrast * 2.2
            } else {
                1.0 / (1.0 + contrast.abs() * 2.2)
            };
            let mut lut = [0u8; 256];
            for (i, slot) in lut.iter_mut().enumerate() {
                let t = i as f32 / 255.0;
                let curved = if t < 0.5 {
                    0.5 * (2.0 * t).powf(k)
                } else {
                    1.0 - 0.5 * (2.0 * (1.0 - t)).powf(k)
                };
                *slot = (curved.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            lut
        };

        // Neutral
        let lut_0 = compute_lut(0.0);
        for i in 0..=255 {
            assert_eq!(lut_0[i], i as u8);
        }

        // High contrast (e.g. +0.5): shadows darker, highlights brighter
        let lut_pos = compute_lut(0.5);
        assert!(
            lut_pos[64] < 64,
            "Shadows should be darkened for high contrast"
        );
        assert!(
            lut_pos[192] > 192,
            "Highlights should be brightened for high contrast"
        );
        assert!(
            (lut_pos[128] as i32 - 128).abs() <= 1,
            "Midtones should remain anchored within 1 level"
        );

        // Low contrast (e.g. -0.5): shadows lifted, highlights softened
        let lut_neg = compute_lut(-0.5);
        assert!(
            lut_neg[64] > 64,
            "Shadows should be lifted for low contrast"
        );
        assert!(
            lut_neg[192] < 192,
            "Highlights should be softened for low contrast"
        );
        assert!(
            (lut_neg[128] as i32 - 128).abs() <= 1,
            "Midtones should remain anchored within 1 level"
        );
    }

    #[test]
    fn test_film_sim_tint_and_post_adjustments() {
        let mut img = image::RgbImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = image::Rgb([128, 128, 128]);
        }

        // Neutral tint and neutral contrast: should be exact identity
        let neutral = apply_film_sim_post_adjustments(&img, 0.0, 0.0);
        assert_eq!(neutral.get_pixel(0, 0), &image::Rgba([128, 128, 128, 255]));

        // Magenta tint (+0.5): Red and Blue boosted, Green attenuated
        let magenta = apply_film_sim_post_adjustments(&img, 0.0, 0.5);
        let p_mag = magenta.get_pixel(0, 0);
        assert!(p_mag[0] > 128, "Red should be boosted for magenta tint");
        assert!(p_mag[2] > 128, "Blue should be boosted for magenta tint");
        assert!(p_mag[1] < 128, "Green should be reduced for magenta tint");

        // Green tint (-0.5): Green boosted, Red and Blue attenuated
        let green = apply_film_sim_post_adjustments(&img, 0.0, -0.5);
        let p_grn = green.get_pixel(0, 0);
        assert!(p_grn[1] > 128, "Green should be boosted for green tint");
        assert!(p_grn[0] < 128, "Red should be reduced for green tint");
        assert!(p_grn[2] < 128, "Blue should be reduced for green tint");
    }

    #[test]
    fn test_film_sim_light_leak_presets() {
        let presets = [
            LightLeakPreset::None,
            LightLeakPreset::WarmCorner,
            LightLeakPreset::SunStreak,
            LightLeakPreset::SoftEdge,
            LightLeakPreset::OrganicBlob,
        ];
        for p in presets {
            let config = p.to_config();
            if p == LightLeakPreset::None {
                assert!(!config.enabled);
                assert!(config.leaks.is_empty());
                assert_eq!(p.label(), "None");
            } else {
                assert!(config.enabled);
                assert_eq!(config.leaks.len(), 1);
                assert!(!p.label().is_empty());
            }
        }
    }

    #[test]
    fn test_film_sim_thumbnail_batch_streaming() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(42, 28);
        for p in img.pixels_mut() {
            *p = image::Rgba([100, 120, 140, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Defaults for expanded controls
        assert_eq!(state.film_sim_state.tint, 0.0);
        assert_eq!(state.film_sim_state.wb_mode, WhiteBalanceMode::Auto);
        assert_eq!(state.film_sim_state.wb_strength, 0.75);
        assert_eq!(state.film_sim_state.halation, 0.25);
        assert_eq!(state.film_sim_state.vignette, 0.35);
        assert_eq!(state.film_sim_state.light_leak, LightLeakPreset::None);

        // Run thumbnail batch
        state.run_thumbnail_batch();
        assert!(state.film_sim_state.is_generating_thumbnails);
        assert!(state.film_sim_state.thumbnail_rx.is_some());

        // Wait for at least some thumbnails to stream in
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while state.film_sim_state.pending_thumbnails.is_empty()
            && std::time::Instant::now() < deadline
        {
            state.poll_film_sim();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        assert!(
            !state.film_sim_state.pending_thumbnails.is_empty(),
            "Pending thumbnails should receive streamed items from worker"
        );
        let first_thumb = &state.film_sim_state.pending_thumbnails[0];
        assert_eq!(first_thumb.1.width(), 42);
        assert_eq!(first_thumb.1.height(), 28);
    }

    #[test]
    fn test_creative_fx_mode_switching_and_defaults() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = image::Rgba([180, 100, 50, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 32, 32, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Label check (no shortcut letter)
        assert_eq!(ImageToolMode::CreativeFx.label(), "Creative FX");

        // Defaults
        assert_eq!(
            state.creative_fx_state.selected_fx,
            CreativeFxType::LensAberration
        );
        assert_eq!(state.creative_fx_state.lens_params.strength, 25.0);
        assert_eq!(state.creative_fx_state.lens_params.falloff, 50.0);
        assert_eq!(state.creative_fx_state.lens_params.color_balance, 0.0);
        assert_eq!(
            state.creative_fx_state.trichromatic_params.mode,
            TrichromaticMode::Rgb
        );
        assert_eq!(
            state.creative_fx_state.trichromatic_params.channels.len(),
            3
        );
        assert_eq!(
            state.creative_fx_state.pixel_sort_params.direction,
            PixelSortDirection::Vertical
        );
        assert_eq!(state.creative_fx_state.cmyk_params.dot_size, 5.0);
        assert_eq!(
            state.creative_fx_state.cmyk_params.paper_tint,
            PaperTint::Newsprint
        );
        assert_eq!(
            state.creative_fx_state.dither_params.palette,
            RetroPalette::GameBoy
        );
        assert_eq!(
            state.creative_fx_state.dither_params.method,
            DitherMethod::Bayer4x4
        );
        assert_eq!(state.creative_fx_state.ntsc_params.luma_smear, 0.25);
        assert_eq!(
            state.creative_fx_state.ntsc_params.tape_speed,
            VhsTapeSpeed::SP
        );
        assert_eq!(
            state.creative_fx_state.ntsc_params.demodulation,
            NtscDemodulation::Notch
        );
        assert_eq!(
            state.creative_fx_state.ntsc_params.phase_shift,
            NtscPhaseShift::Degrees180
        );
        assert_eq!(
            state.creative_fx_state.ntsc_params.chroma_lowpass,
            NtscChromaLowpass::Full
        );
        assert_eq!(state.creative_fx_state.structure_params.amount, 35.0);
        assert_eq!(state.creative_fx_state.structure_params.detail_scale, 0.0);
        assert_eq!(
            state.creative_fx_state.structure_params.edge_protection,
            50.0
        );

        // Switch to Creative FX mode
        state.set_tool_mode(ImageToolMode::CreativeFx);
        assert_eq!(state.active_mode, ImageToolMode::CreativeFx);

        // Verify async render triggered
        assert!(state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.render_rx.is_some());

        // Poll until render completes
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(!state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.preview_rgba.is_some());
        assert_eq!(state.creative_fx_state.render_error, None);

        // Switch to Structure effect and trigger render
        state.creative_fx_state.selected_fx = CreativeFxType::Structure;
        state.run_creative_fx_render();
        assert!(state.creative_fx_state.is_rendering);
        let deadline_structure = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline_structure
        {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.preview_rgba.is_some());
        assert_eq!(state.creative_fx_state.render_error, None);

        // Switch to NTSC effect and trigger render
        state.creative_fx_state.selected_fx = CreativeFxType::Ntsc;
        state.run_creative_fx_render();
        assert!(state.creative_fx_state.is_rendering);
        let deadline_ntsc = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline_ntsc {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.preview_rgba.is_some());
        assert_eq!(state.creative_fx_state.render_error, None);

        // Apply Creative FX to image
        let pre_undo = state.undo_stack.len();
        state.apply_creative_fx();
        assert_eq!(state.undo_stack.len(), pre_undo + 1);
        assert!(state.dirty_texture);
        assert!(state.status_toast.is_some());
    }

    #[test]
    fn test_creative_fx_thumbnail_streaming() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(42, 28);
        for (i, p) in img.pixels_mut().enumerate() {
            *p = image::Rgba([(i % 255) as u8, 120, 200, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.run_creative_fx_thumbnail_batch();
        assert!(state.creative_fx_state.is_generating_thumbnails);
        assert!(state.creative_fx_state.thumbnail_rx.is_some());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.pending_thumbnails.is_empty()
            && std::time::Instant::now() < deadline
        {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        assert!(
            !state.creative_fx_state.pending_thumbnails.is_empty(),
            "Pending thumbnails should contain generated items"
        );
        let first_thumb = &state.creative_fx_state.pending_thumbnails[0];
        assert_eq!(first_thumb.1.width(), 42);
        assert_eq!(first_thumb.1.height(), 28);
    }

    #[test]
    fn test_creative_fx_depth_mask_workflow() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(16, 16);
        for p in img.pixels_mut() {
            *p = image::Rgba([80, 80, 80, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 16, 16, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();
        state.active_mode = ImageToolMode::CreativeFx;

        // 1. Verify depth mask is disabled by default across effects
        assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
        assert_eq!(
            state.creative_fx_state.depth_mask_for_active_fx().threshold,
            0.0
        );
        assert_eq!(
            state.creative_fx_state.depth_mask_for_active_fx().feather,
            0.25
        );
        assert!(!state.creative_fx_state.depth_mask_for_active_fx().invert);
        assert!(state.creative_fx_state.cached_depth_map.is_none());

        // 2. Set synthetic depth map (left half dark / near, right half bright / far)
        let mut depth_map = RgbaImage::new(16, 16);
        for (x, _y, p) in depth_map.enumerate_pixels_mut() {
            let val = if x < 8 { 0u8 } else { 255u8 };
            *p = image::Rgba([val, val, val, 255]);
        }
        state.creative_fx_state.cached_depth_map = Some(depth_map);
        state.creative_fx_state.depth_map_input_stamp = Some(state.input_stamp());

        // 3. Enable depth mask on PixelSort
        state.creative_fx_state.selected_fx = CreativeFxType::PixelSort;
        state
            .creative_fx_state
            .depth_mask_for_active_fx_mut()
            .enabled = true;
        assert!(state.creative_fx_state.depth_mask_for_active_fx().enabled);

        // 4. Render with depth mask
        state.run_creative_fx_render();
        assert!(state.creative_fx_state.is_rendering);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.preview_rgba.is_some());
        assert_eq!(state.creative_fx_state.render_error, None);

        // 5. Test mask inversion and threshold
        state
            .creative_fx_state
            .depth_mask_for_active_fx_mut()
            .invert = true;
        state
            .creative_fx_state
            .depth_mask_for_active_fx_mut()
            .threshold = 0.5;
        state
            .creative_fx_state
            .depth_mask_for_active_fx_mut()
            .feather = 0.1;
        state.run_creative_fx_render();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.preview_rgba.is_some());

        // 6. Switch to Structure effect, verify cached depth map is shared
        state.creative_fx_state.selected_fx = CreativeFxType::Structure;
        assert!(state.creative_fx_state.cached_depth_map.is_some());
        state
            .creative_fx_state
            .depth_mask_for_active_fx_mut()
            .enabled = true;
        state.run_creative_fx_render();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!state.creative_fx_state.is_rendering);
        assert!(state.creative_fx_state.preview_rgba.is_some());
    }

    #[test]
    fn test_creative_fx_alpha_mask_runs_fey_nobg_without_prompt() {
        let config = AppConfig::default();
        let img = RgbaImage::from_pixel(16, 16, image::Rgba([80, 80, 80, 255]));
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 16, 16, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        let mock_script = "import sys, io\nfrom PIL import Image\nassert '--prompt' not in sys.argv, 'Creative FX must use FeyNoBg without a prompt'\nimg = Image.open(io.BytesIO(sys.stdin.buffer.read())).convert('RGBA')\nfor y in range(img.height):\n    for x in range(img.width):\n        img.putpixel((x, y), (*img.getpixel((x, y))[:3], 0 if x < 8 else 255))\nbuf = io.BytesIO()\nimg.save(buf, format='PNG')\nsys.stdout.buffer.write(buf.getvalue())\n";
        let script_path = std::env::temp_dir().join("mock_nobg_fx_worker.py");
        std::fs::write(&script_path, mock_script).unwrap();
        state.bg_removal_cmd = Some(format!("python3 {}", script_path.display()));
        state.multimatte_prompt = "person in red".to_string();

        state.run_alpha_mask_for_fx();
        assert!(state.creative_fx_state.alpha_mask_in_progress);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.alpha_mask_in_progress && std::time::Instant::now() < deadline
        {
            state.poll_creative_fx();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(!state.creative_fx_state.alpha_mask_in_progress);
        assert_eq!(state.creative_fx_state.alpha_mask_error, None);
        let removal = state
            .creative_fx_state
            .cached_background_removal
            .as_ref()
            .expect("FeyNoBg result should be cached");
        assert_eq!(removal.get_pixel(2, 2)[3], 0);
        assert_eq!(removal.get_pixel(12, 2)[3], 255);
        assert_eq!(state.current_rgba.get_pixel(2, 2)[3], 255);

        let mask = state.creative_fx_state.depth_mask_for_active_fx();
        assert!(mask.enabled);
        assert_eq!(mask.source, CreativeFxMaskSource::BackgroundRemovalAlpha);
    }
}
