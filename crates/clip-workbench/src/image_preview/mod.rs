//! Rich interactive image workbench and manipulation suite.
//!
//! Provides pan/zoom, cropping with fixed aspect ratios, pixelation/blackout redaction,
//! eyedropper color inspection, resizing, palette extraction, background removal, and 3D depth map generation.

mod encoding;
mod graphics;
mod handoff;
mod history;
mod studio;
mod viewport;

use encoding::*;
use graphics::*;

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
    viewport_size: Vec2,
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
            little_clip_core::item::image_reader(&bytes, Some(&mime))
                .and_then(|reader| reader.decode())
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
                image_texture_options(pixels),
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
            viewport_size: Vec2::ZERO,
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
}

#[cfg(test)]
mod tests;
