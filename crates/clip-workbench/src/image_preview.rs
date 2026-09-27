//! Rich interactive image workbench and manipulation suite.
//!
//! Provides pan/zoom, cropping with fixed aspect ratios, pixelation/blackout redaction,
//! eyedropper color inspection, resizing, palette extraction, background removal, and 3D depth map generation.

mod tools;
use tools::*;

use crate::icons::Icon;
use crate::{contrast_text_c32, hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use image::RgbaImage;
use little_clip_core::palette::AppPalette;
use little_clip_core::{AppConfig, ClipItem, CommandRequest, HandSide, IpcRequest};
use little_clip_ops::barcode::BarcodeDetection;
use little_clip_ops::creative_fx::{
    CmykHalftoneParams, CreativeFxMaskSource, CreativeFxParams, CreativeFxType, DepthMaskConfig,
    DitherMethod, DitherPaletteParams, LensAberrationParams, NtscChromaLowpass, NtscDemodulation,
    NtscParams, NtscPhaseShift, PaperTint, PixelSortCriteria, PixelSortDirection, PixelSortOrder,
    PixelSortParams, RetroPalette, StructureParams, TrichromaticMode, TrichromaticParams,
    VhsTapeSpeed, apply_creative_fx, apply_creative_fx_with_masks, default_preset_for_fx,
    presets_for_fx, trichromatic_intersection_box,
};
use little_clip_ops::decorrelation_stretch::{
    DecorrelationMatrixMode, DecorrelationStats, DecorrelationStretchParams,
    DecorrelationTransform, TargetValue, auto_stride, run_decorrelation_stretch,
};
use little_clip_ops::metadata::{ImageMetadataSummary, extract_image_metadata, strip_metadata};
use little_clip_ops::perspective::{DeskewCropMode, QuadCorners, rectify_perspective_with_mode};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    #[allow(dead_code)]
    pub original_width: u32,
    #[allow(dead_code)]
    pub original_height: u32,
    pub is_svg: bool,
    pub raw_bytes: Vec<u8>,

    // Images
    #[allow(dead_code)]
    pub original_rgba: RgbaImage,
    pub current_rgba: RgbaImage,
    /// Increases whenever the committed pixel buffer changes.
    pub(crate) pixel_revision: u64,
    texture: Option<egui::TextureHandle>,
    dirty_texture: bool,

    // Undo / Redo Stack
    pub undo_stack: Vec<RgbaImage>,
    pub redo_stack: Vec<RgbaImage>,

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

    // Palette State
    pub palette_state: PaletteState,

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
    pub save_dialog_rx: Option<std::sync::mpsc::Receiver<PathBuf>>,
    pub show_fallback_save_modal: bool,
    fallback_save_needs_focus: bool,
    pub fallback_save_path: String,

    // File Size Optimizer Async Worker
    pub optimizer_rx: Option<std::sync::mpsc::Receiver<OptimizerMsg>>,
    pub optimizer_status: Option<String>,

    // Background Removal Async Worker & State
    pub bg_removal_in_progress: bool,
    pub bg_removal_rx: Option<std::sync::mpsc::Receiver<BgRemovalMsg>>,
    pub(crate) bg_removal_input_stamp: Option<ImageInputStamp>,
    pub bg_removal_status: Option<String>,
    pub bg_removal_error: Option<String>,
    pub bg_removal_cmd: Option<String>,
    pub multimatte_prompt: String,

    // Format Conversion (Squoosh-style)
    pub convert_state: ConvertState,

    // Multi-Model Document OCR
    pub ocr_state: OcrState,
    pub ocr_rx: Option<std::sync::mpsc::Receiver<OcrMsg>>,
    pub(crate) ocr_input_stamp: Option<ImageInputStamp>,
    pub(crate) ocr_results_stamp: Option<ImageInputStamp>,
    pub ocr_child_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,

    // Barcode / QR / DataMatrix Decoder
    pub barcode_state: BarcodeScanState,
    pub barcode_rx: Option<std::sync::mpsc::Receiver<BarcodeScanMsg>>,
    pub(crate) barcode_input_stamp: Option<ImageInputStamp>,
    pub(crate) barcode_results_stamp: Option<ImageInputStamp>,

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

/// Identity of the image pixels and overlays consumed by `current_display_image`.
/// This intentionally snapshots only small edit state, never the full pixel buffer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImageInputStamp {
    pixel_revision: u64,
    redactions: Vec<RedactionBox>,
    markup: Vec<MarkupShape>,
    crop: Option<Rect>,
}

impl ImagePreviewState {
    pub fn from_path(path: PathBuf, config: &AppConfig) -> Result<Self, String> {
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("Failed to read file {}: {}", path.display(), e))?;
        let mime = little_clip_core::item::guess_image_mime(&path);
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Image Preview".to_string());
        Self::from_bytes_with_meta(bytes, mime, Some(path), title, config)
    }

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

    pub fn from_bytes_with_meta(
        bytes: Vec<u8>,
        mime: String,
        path: Option<PathBuf>,
        title: String,
        config: &AppConfig,
    ) -> Result<Self, String> {
        let file_size_bytes = bytes.len();
        let is_svg = mime.contains("svg")
            || little_clip_core::item::is_svg_bytes(&bytes)
            || path
                .as_ref()
                .and_then(|p| p.extension())
                .map_or(false, |e| e.eq_ignore_ascii_case("svg"));

        let rgba = if is_svg {
            if let Some(png_bytes) = render_svg_to_png(&bytes) {
                image::load_from_memory(&png_bytes)
                    .map_err(|e| format!("Failed to decode rendered SVG PNG: {}", e))?
                    .to_rgba8()
            } else {
                return Err("Failed to rasterize SVG via rsvg-convert. Ensure librsvg / rsvg-convert is installed.".to_string());
            }
        } else {
            image::load_from_memory(&bytes)
                .map_err(|e| format!("Failed to decode image bytes: {}", e))?
                .to_rgba8()
        };

        let (width, height) = rgba.dimensions();
        let mut resize_state = ResizeState::default();
        resize_state.sync_from_image(&rgba);
        let palette_state = PaletteState::default();
        let meta = extract_image_metadata(&bytes);
        let metadata = if meta.has_any_metadata() {
            Some(meta)
        } else {
            None
        };

        let preview = Self {
            path,
            title,
            mime,
            file_size_bytes,
            original_width: width,
            original_height: height,
            is_svg,
            raw_bytes: bytes,
            original_rgba: rgba.clone(),
            current_rgba: rgba,
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
            palette_state,
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
            save_dialog_rx: None,
            show_fallback_save_modal: false,
            fallback_save_needs_focus: false,
            fallback_save_path: String::new(),
            optimizer_rx: None,
            optimizer_status: None,
            bg_removal_in_progress: false,
            bg_removal_rx: None,
            bg_removal_input_stamp: None,
            bg_removal_status: None,
            bg_removal_error: None,
            bg_removal_cmd: config.image.bg_removal_cmd.clone(),
            multimatte_prompt: String::new(),
            convert_state: ConvertState::default(),
            ocr_state: OcrState::default(),
            ocr_rx: None,
            ocr_input_stamp: None,
            ocr_results_stamp: None,
            ocr_child_pid: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            barcode_state: BarcodeScanState::default(),
            barcode_rx: None,
            barcode_input_stamp: None,
            barcode_results_stamp: None,
            dcs_state: DecorrelationStretchState::default(),
            film_sim_state: FilmSimState::default(),
            creative_fx_state: CreativeFxState::default(),
            metadata,
            show_metadata_drawer: false,
            deskew_state: DeskewState::default(),
        };

        Ok(preview)
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

    pub(crate) fn mark_pixels_changed(&mut self) {
        self.pixel_revision = self.pixel_revision.wrapping_add(1);
    }

    pub(crate) fn ocr_output_is_current(&self) -> bool {
        self.ocr_results_stamp.as_ref() == Some(&self.input_stamp())
            && !self.ocr_state.extracted_text.is_empty()
    }

    /// Returns the image as currently displayed/composed, taking into account any active crop selection if in Crop mode,
    /// and burning any placed redaction bars so exports/copies never leak private information.
    pub fn current_display_image(&self) -> RgbaImage {
        let mut img = self.current_rgba.clone();

        // Burn any pending redactions onto the output image
        if !self.redaction_state.boxes.is_empty() {
            let img_w = img.width() as f32;
            let img_h = img.height() as f32;
            for b in &self.redaction_state.boxes {
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
        if !self.markup_state.items.is_empty() {
            img = rasterize_markup(&img, &self.markup_state.items);
        }

        if self.active_mode == ImageToolMode::Crop {
            let norm = self.crop_state.normalized_rect;
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

    /// Burns all pending redaction black bars permanently into current_rgba and records an undo point.
    pub fn strip_metadata(&mut self) {
        match strip_metadata(&self.current_rgba, &self.mime) {
            Ok(clean_bytes) => {
                self.undo_stack.push(self.current_rgba.clone());
                self.redo_stack.clear();
                self.raw_bytes = clean_bytes;
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
        let img = self.current_display_image();
        let (w, h) = (img.width(), img.height());

        let mut png_bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        if let Err(e) = img.write_to(&mut cursor, image::ImageFormat::Png) {
            self.show_toast(format!("❌ Failed to encode PNG: {}", e));
            return;
        }

        // 1. Send to daemon slot via IPC
        let req = IpcRequest::PushImage {
            bytes: png_bytes.clone(),
            mime: "image/png".to_string(),
            hand,
        };
        let ipc_ok = crate::send_ipc_request(&req).is_ok();

        // 2. Also copy directly for instantaneous OS clipboard synchronization.
        let bytes_for_cmd = png_bytes;
        std::thread::spawn(move || {
            if let Err(error) =
                little_clip_core::copy_bytes_to_system_clipboard(&bytes_for_cmd, "image/png")
            {
                tracing::debug!(%error, "failed to copy rendered image to system clipboard");
            }
        });

        let slot_label = match hand {
            HandSide::Left => "Left Hand (LH)",
            HandSide::Right => "Right Hand (RH)",
        };

        if ipc_ok {
            self.show_toast(format!(
                "📋 Copied {}×{} image to Clipboard Slot ({})",
                w, h, slot_label
            ));
        } else {
            self.show_toast(format!("📋 Copied {}×{} image to OS Clipboard", w, h));
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

        let target_str = default_target.to_string_lossy().to_string();

        let zenity_available = crate::ui_ext::is_zenity_available();

        if zenity_available {
            let (tx, rx) = std::sync::mpsc::channel();
            self.save_dialog_rx = Some(rx);

            std::thread::spawn(move || {
                let mut cmd = Command::new("zenity");
                cmd.arg("--file-selection")
                    .arg("--save")
                    .arg("--confirm-overwrite")
                    .arg(format!("--filename={}", target_str))
                    .arg("--file-filter=Images (*.png, *.jpg, *.webp, *.bmp) | *.png *.jpg *.jpeg *.webp *.bmp *.gif")
                    .arg("--file-filter=All files | *");
                if let Ok(output) = cmd.output() {
                    if output.status.success() {
                        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !path_str.is_empty() {
                            let _ = tx.send(PathBuf::from(path_str));
                        }
                    }
                }
            });
        } else {
            self.fallback_save_path = target_str;
            self.show_fallback_save_modal = true;
            self.fallback_save_needs_focus = true;
        }
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

        let target_str = default_target.to_string_lossy().to_string();

        let zenity_available = crate::ui_ext::is_zenity_available();

        if zenity_available {
            let (tx, rx) = std::sync::mpsc::channel();
            self.save_dialog_rx = Some(rx);

            let filter_arg = format!("Target (*.{}) | *.{}", target_ext, target_ext);
            std::thread::spawn(move || {
                let mut cmd = Command::new("zenity");
                cmd.arg("--file-selection")
                    .arg("--save")
                    .arg("--confirm-overwrite")
                    .arg(format!("--filename={}", target_str))
                    .arg(format!("--file-filter={}", filter_arg))
                    .arg("--file-filter=All files | *");
                if let Ok(output) = cmd.output() {
                    if output.status.success() {
                        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !path_str.is_empty() {
                            let _ = tx.send(PathBuf::from(path_str));
                        }
                    }
                }
            });
        } else {
            self.fallback_save_path = target_str;
            self.show_fallback_save_modal = true;
            self.fallback_save_needs_focus = true;
        }
    }

    /// Saves the current display image to the specified file path, creating parent dirs if needed.
    pub fn save_image_to_path(&mut self, path: &Path) {
        let mut final_path = path.to_path_buf();
        if final_path.extension().is_none() {
            final_path.set_extension("png");
        }

        if let Some(parent) = final_path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(err) = std::fs::create_dir_all(parent) {
                    tracing::warn!(
                        "Failed to create destination directory {}: {err}",
                        parent.display()
                    );
                    self.status_toast = Some((format!("Failed to create directory: {err}"), 0.0));
                    return;
                }
            }
        }

        let img = self.current_display_image();
        let ext = final_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_lowercase();

        let save_res = if self.active_mode == ImageToolMode::Convert
            && !self.convert_state.encoded_bytes.is_empty()
            && ext == self.convert_state.target_format.extension()
        {
            std::fs::write(&final_path, &self.convert_state.encoded_bytes)
                .map_err(image::ImageError::IoError)
        } else if ext == "jpg" || ext == "jpeg" {
            // Composite RGBA onto white RGB background for JPEG
            let mut rgb = image::RgbImage::new(img.width(), img.height());
            for (x, y, pixel) in img.enumerate_pixels() {
                let alpha = pixel[3] as f32 / 255.0;
                let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
                let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
                let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
                rgb.put_pixel(x, y, image::Rgb([r, g, b]));
            }
            rgb.save(&final_path)
        } else if ext == "webp" {
            let quality = self.convert_state.quality;
            let lossless = self.convert_state.lossless;
            if let Ok(bytes) = encode_webp(&img, lossless, quality) {
                std::fs::write(&final_path, bytes).map_err(image::ImageError::IoError)
            } else {
                Err(image::ImageError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "WebP encoding failed",
                )))
            }
        } else {
            img.save(&final_path)
        };

        match save_res {
            Ok(_) => {
                let file_name = final_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                self.show_toast(format!("💾 Saved image to {}", file_name));
                self.path = Some(final_path.clone());
                self.title = file_name;
                if let Ok(meta) = std::fs::metadata(&final_path) {
                    self.file_size_bytes = meta.len() as usize;
                }
                self.mime = little_clip_core::item::guess_image_mime(&final_path);
            }
            Err(e) => {
                self.show_toast(format!("❌ Failed to save: {}", e));
            }
        }
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
            std::fs::write(&staging_path, &self.raw_bytes)
                .map_err(|e| format!("Failed to write staging file: {}", e))?;
        }

        Ok(staging_path)
    }

    pub fn launch_external_app(&mut self, app: &str, _ctx: &egui::Context, _close: &mut bool) {
        match self.ensure_disk_path() {
            Ok(file_path) => {
                match crate::shell::EscapeHatchLauncher::spawn_with_fallback(app, app, &file_path) {
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

        // Check asynchronous save dialog result from zenity
        if let Some(rx) = &self.save_dialog_rx {
            match rx.try_recv() {
                Ok(chosen_path) => {
                    self.save_image_to_path(&chosen_path);
                    self.save_dialog_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.save_dialog_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
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
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.horizontal(|ui| {
                        if ui
                            .add(crate::ui_ext::action_button_icon(
                                Some(Icon::Save),
                                "Save",
                                hex_to_c32(&palette.diff_add),
                                palette,
                            ))
                            .clicked()
                        {
                            let p = PathBuf::from(&self.fallback_save_path);
                            self.save_image_to_path(&p);
                            self.show_fallback_save_modal = false;
                            self.fallback_save_needs_focus = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_fallback_save_modal = false;
                            self.fallback_save_needs_focus = false;
                        }
                    });
                });
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
        self.poll_optimizer();
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }

        // Poll background AI background removal if running
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
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
            if self.convert_state.is_pending() {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }

        // Poll film sim background render + debounce
        self.poll_film_sim();
        if self.active_mode == ImageToolMode::FilmSim {
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
            if self.film_sim_state.is_rendering || self.film_sim_state.is_generating_thumbnails {
                ctx.request_repaint();
            }
        }

        // Poll creative fx background render + debounce
        self.poll_creative_fx();
        if self.active_mode == ImageToolMode::CreativeFx {
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
                let footer_hint = image_studio_footer_hint(self.active_mode);
                let footer_text_height = ui.fonts(|fonts| {
                    fonts
                        .layout_no_wrap(
                            footer_hint.to_owned(),
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            Color32::WHITE,
                        )
                        .size()
                        .y
                });
                let footer_height = footer_text_height
                    + egui::Margin::symmetric(
                        crate::ui_ext::PANEL_INSET,
                        crate::ui_ext::CONTROL_GROUP_INSET_Y,
                    )
                    .sum()
                    .y;
                egui::TopBottomPanel::bottom(ui.make_persistent_id("image_studio_footer"))
                    .frame(egui::Frame::none())
                    .resizable(false)
                    .exact_height(footer_height)
                    .show_inside(ui, |ui| {
                        self.render_footer(ui, palette, now, footer_text_height)
                    });

                // 1. TOP HEADER & ESCAPE HATCHES
                self.render_header(ui, ctx, palette, close);

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 2. TOOLBAR (Mode selection & Viewport controls)
                self.render_toolbar(ui, palette);

                // Tool modules own their compact or panel-based controls.
                self.render_active_tool_controls(ui, ctx, palette);

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 3. MAIN INTERACTIVE VIEWPORT
                self.render_active_tool_workspace(ui, ctx, palette);
            });
    }

    fn render_header(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close: &mut bool,
    ) {
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

        let mut shell = crate::shell::StudioShell::new(
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
        );

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
            self.launch_external_app(&hatch.command, ctx, close);
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
        if prev_mode == ImageToolMode::Ocr
            && target_mode != ImageToolMode::Ocr
            && self.ocr_state.in_progress
        {
            self.cancel_ocr();
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
                if self.dcs_state.stretched_rgba.is_none() {
                    self.update_decorrelation_stretch();
                }
            }
            ImageToolMode::Barcode => {
                // Decoding is explicitly user initiated. Entering this mode must
                // remain instant even for a multi-megapixel image.
            }
            ImageToolMode::FilmSim => {
                self.film_sim_state.auto_fitted = false;
                // Trigger initial render if no preview yet.
                if self.film_sim_state.preview_rgba.is_none() && !self.film_sim_state.is_rendering {
                    self.run_film_sim_render();
                }
                // Trigger thumbnail batch if not generated yet.
                if self.film_sim_state.stock_thumbnails.is_empty()
                    && self.film_sim_state.pending_thumbnails.is_empty()
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
                if self.creative_fx_state.fx_thumbnails.is_empty()
                    && self.creative_fx_state.pending_thumbnails.is_empty()
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
                self.render_pick_color_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Barcode {
                self.render_barcode_overlay(ui, &painter, &response, img_rect, palette);
            }
        }
    }

    fn render_footer(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        now: f64,
        text_height: f32,
    ) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), text_height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(image_studio_footer_hint(self.active_mode))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(colors.text_secondary),
                        );

                        // Toast message
                        ui.add_space(20.0);
                        crate::ui_ext::StatusToast::render_opt(
                            &self.status_toast,
                            5.0,
                            ui,
                            palette,
                            now,
                        );
                    },
                );
            });
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

        assert_eq!(state.current_rgba, output);
        assert_eq!(state.pixel_revision, 1);
        assert!(state.redaction_state.boxes.is_empty());
        assert!(state.markup_state.items.is_empty());
        assert_eq!(
            state.creative_fx_state.cached_background_removal,
            Some(output)
        );
        assert!(!state.bg_removal_in_progress);
        assert!(state.bg_removal_rx.is_none());
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
        assert!(tmp_png.exists());
        let reloaded_png = image::open(&tmp_png).expect("Reload PNG failed");
        assert_eq!(reloaded_png.width(), 120);
        assert_eq!(reloaded_png.height(), 80);
        assert_eq!(state.mime, "image/png");

        // Save as JPEG (exercises RGBA -> RGB flattening)
        state.save_image_to_path(&tmp_jpg);
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
        // Generate a QR code using rxing
        let writer = rxing::qrcode::QRCodeWriter;
        use rxing::Writer;
        let bit_matrix = writer
            .encode("https://github.com", &rxing::BarcodeFormat::QR_CODE, 64, 64)
            .unwrap();

        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
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
        let item = ClipItem::from_image(png_bytes, 64, 64, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Run barcode scan
        state.run_barcode_scan();
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !state.barcode_state.has_scanned && std::time::Instant::now() < deadline {
            state.poll_barcode_scan(&ctx);
            std::thread::yield_now();
        }
        assert!(state.barcode_state.has_scanned);
        assert_eq!(state.barcode_state.detections.len(), 1);
        assert_eq!(state.barcode_state.detections[0].text, "https://github.com");
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
        state
            .ocr_child_pid
            .store(999999, std::sync::atomic::Ordering::SeqCst);

        // Cancel OCR
        state.cancel_ocr();

        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.status_text.is_none());
        assert_eq!(
            state
                .ocr_child_pid
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
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
        assert!(state.palette_state.is_extracted);

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
        assert!(state.dcs_state.stretched_rgba.is_some());

        // Switching from OCR in progress cancels OCR
        state.set_tool_mode(ImageToolMode::Ocr);
        assert_eq!(state.active_mode, ImageToolMode::Ocr);
        state.ocr_state.in_progress = true;
        state
            .ocr_child_pid
            .store(12345, std::sync::atomic::Ordering::SeqCst);
        state.set_tool_mode(ImageToolMode::View);
        assert_eq!(state.active_mode, ImageToolMode::View);
        assert!(!state.ocr_state.in_progress);
        assert_eq!(
            state
                .ocr_child_pid
                .load(std::sync::atomic::Ordering::SeqCst),
            0
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
        assert_eq!(state.film_sim_state.wb_strength, 1.0);
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
