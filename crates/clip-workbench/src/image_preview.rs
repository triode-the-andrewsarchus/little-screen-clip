//! Rich interactive image workbench and manipulation suite.
//!
//! Provides pan/zoom, cropping with fixed aspect ratios, pixelation/blackout redaction,
//! eyedropper color inspection, resizing, palette extraction, background removal, and 3D depth map generation.

use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use image::RgbaImage;
use little_clip_core::theme::AppPalette;
use little_clip_core::{AppConfig, ClipItem, HandSide, IpcRequest};
use little_clip_ops::barcode::BarcodeDetection;
use little_clip_ops::decorrelation_stretch::{
    auto_stride, run_decorrelation_stretch, DecorrelationMatrixMode, DecorrelationStats,
    DecorrelationStretchParams, DecorrelationTransform, TargetValue,
};
use little_clip_ops::creative_fx::{
    apply_creative_fx, default_preset_for_fx, presets_for_fx,
    trichromatic_intersection_box, CmykHalftoneParams, CreativeFxParams,
    CreativeFxType, DitherMethod, DitherPaletteParams, LensAberrationParams,
    NtscChromaLowpass, NtscDemodulation, NtscParams, NtscPhaseShift, PaperTint,
    PixelSortCriteria, PixelSortDirection, PixelSortOrder, PixelSortParams,
    RetroPalette, TrichromaticMode, TrichromaticParams, VhsTapeSpeed,
};
use little_clip_ops::metadata::{extract_image_metadata, strip_metadata, ImageMetadataSummary};
use little_clip_ops::perspective::{
    rectify_perspective_with_mode, DeskewCropMode, QuadCorners,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::{contrast_text_c32, hex_to_c32, hex_to_c32_alpha};

// filmr physics-based film simulation engine
use filmr::film::FilmStyle;
use filmr::light_leak::{LightLeak, LightLeakConfig, LightLeakShape};
use filmr::processor::{OutputMode, SimulationConfig, WhiteBalanceMode};

/// Active image manipulation tool mode selected in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageToolMode {
    /// Pan and zoom inspection mode.
    #[default]
    View,
    /// Crop region selection and trimming.
    Crop,
    /// 4-point perspective quad correction and document flattening.
    Deskew,
    /// Blackout, blur, or pixelation redaction.
    Redact,
    /// Eyedropper pixel color sampler.
    PickColor,
    /// Dimension and file size optimization resizing.
    Resize,
    /// Rotation and horizontal/vertical flipping.
    Transform,
    /// Format conversion with before/after comparison slider.
    Convert,
    /// Multi-model document OCR.
    Ocr,
    /// 1D and 2D barcode, QR, and DataMatrix decoding.
    Barcode,
    /// Dominant color palette extraction and harmonic scheme generation.
    Palette,
    /// AI-powered background removal.
    RemoveBg,
    /// Monocular depth map generation.
    DepthMap,
    /// Decorrelation stretch (Alley / JPL 1996) color enhancement.
    DecorrelationStretch,
    /// Physics-based film stock simulation (filmr engine).
    FilmSim,
    /// Creative optical and digital distortion FX suite.
    CreativeFx,
}

impl ImageToolMode {
    /// Human-readable toolbar label with icon.
    pub fn label(&self) -> &'static str {
        match self {
            ImageToolMode::View => "🔍 [V]iew",
            ImageToolMode::Crop => "✂ [C]rop",
            ImageToolMode::Deskew => "📐 Des[K]ew",
            ImageToolMode::Transform => "🔄 [F]lip/Rotate",
            ImageToolMode::Redact => "█ [X] Redact",
            ImageToolMode::PickColor => "💉 [P]ick Color",
            ImageToolMode::Resize => "↔ Re[S]ize",
            ImageToolMode::Convert => "🔄 Conver[T]",
            ImageToolMode::Ocr => "📝 [O]CR",
            ImageToolMode::Barcode => "▦ [B]arcode",
            ImageToolMode::Palette => "🎨 Palette",
            ImageToolMode::RemoveBg => "✨ [R]emove BG",
            ImageToolMode::DepthMap => "🌊 [D]epth Map",
            ImageToolMode::DecorrelationStretch => "🌈 Decorrelation Stretch",
            ImageToolMode::FilmSim => "🎞 Film [L]ook",
            ImageToolMode::CreativeFx => "✨ Creative FX",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConvertTargetFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ConvertTargetFormat {
    pub fn label(&self) -> &'static str {
        match self {
            ConvertTargetFormat::Png => "PNG (.png)",
            ConvertTargetFormat::Jpeg => "JPEG (.jpg)",
            ConvertTargetFormat::Webp => "WebP (.webp)",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            ConvertTargetFormat::Png => "png",
            ConvertTargetFormat::Jpeg => "jpg",
            ConvertTargetFormat::Webp => "webp",
        }
    }
}

#[derive(Clone)]
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OcrEngine {
    #[default]
    GlmOcr,
    Tesseract,
}

impl OcrEngine {
    pub fn label(&self) -> &'static str {
        match self {
            OcrEngine::GlmOcr => "GLM-OCR (Default • 0.9B VLM)",
            OcrEngine::Tesseract => "Tesseract (Quick Local)",
        }
    }

    pub fn arg_name(&self) -> &'static str {
        match self {
            OcrEngine::GlmOcr => "glm-ocr",
            OcrEngine::Tesseract => "tesseract",
        }
    }
}

#[derive(Debug)]
pub enum OcrMsg {
    Progress(String),
    Success {
        text: String,
        engine: String,
        duration_ms: u128,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct OcrState {
    pub engine: OcrEngine,
    pub custom_prompt: String,
    pub extracted_text: String,
    pub in_progress: bool,
    pub status_text: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<u128>,
}

impl Default for OcrState {
    fn default() -> Self {
        Self {
            engine: OcrEngine::GlmOcr,
            custom_prompt: String::new(),
            extracted_text: String::new(),
            in_progress: false,
            status_text: None,
            error: None,
            duration_ms: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BarcodeScanState {
    pub detections: Vec<BarcodeDetection>,
    pub selected_idx: Option<usize>,
    pub is_scanning: bool,
    pub has_scanned: bool,
    pub last_scan_duration_ms: u128,
}

#[derive(Clone)]
pub struct DecorrelationStretchState {
    pub params: DecorrelationStretchParams,
    pub custom_mean: f64,
    pub custom_sigma: f64,
    pub stats: Option<DecorrelationStats>,
    pub transform: Option<DecorrelationTransform>,
    pub stretched_rgba: Option<RgbaImage>,
    pub stretched_texture: Option<egui::TextureHandle>,
    pub dirty_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub is_holding_original: bool,
    pub show_stats: bool,
    pub last_duration_ms: u128,
}

impl Default for DecorrelationStretchState {
    fn default() -> Self {
        Self {
            params: DecorrelationStretchParams::nasa(),
            custom_mean: 127.5,
            custom_sigma: 50.0,
            stats: None,
            transform: None,
            stretched_rgba: None,
            stretched_texture: None,
            dirty_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            is_holding_original: false,
            show_stats: false,
            last_duration_ms: 0,
        }
    }
}

#[derive(Clone)]
pub struct DeskewState {
    /// Normalized corner coordinates in [0.0, 1.0] range relative to image:
    /// [0] Top-Left, [1] Top-Right, [2] Bottom-Right, [3] Bottom-Left
    pub corners: [Pos2; 4],
    pub dragging_corner: Option<usize>,
    pub crop_mode: DeskewCropMode,
    /// When true, clicking on the image canvas sequentially places corners 1 to 4
    pub is_clicking_points: bool,
    /// Accumulates points clicked so far during point-placement mode
    pub placed_points: Vec<Pos2>,
    /// Tracks if the quad has been customized (either by clicking 4 points or dragging handles)
    pub corners_adjusted: bool,
}

impl Default for DeskewState {
    fn default() -> Self {
        Self {
            corners: [
                Pos2::new(0.08, 0.08),
                Pos2::new(0.92, 0.08),
                Pos2::new(0.92, 0.92),
                Pos2::new(0.08, 0.92),
            ],
            dragging_corner: None,
            crop_mode: DeskewCropMode::CropToQuad,
            is_clicking_points: false,
            placed_points: Vec::new(),
            corners_adjusted: false,
        }
    }
}

impl DeskewState {
    pub fn reset_quad(&mut self) {
        self.corners = [
            Pos2::new(0.08, 0.08),
            Pos2::new(0.92, 0.08),
            Pos2::new(0.92, 0.92),
            Pos2::new(0.08, 0.92),
        ];
        self.dragging_corner = None;
        self.is_clicking_points = false;
        self.placed_points.clear();
        self.corners_adjusted = false;
    }

    pub fn start_clicking_points(&mut self) {
        self.is_clicking_points = true;
        self.placed_points.clear();
        self.dragging_corner = None;
    }

    pub fn cancel_clicking_points(&mut self) {
        self.is_clicking_points = false;
        self.placed_points.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CropRatio {
    #[default]
    Free,
    Ratio1x1,
    Ratio16x9,
    Ratio9x16,
}

impl CropRatio {
    pub fn label(&self) -> &'static str {
        match self {
            CropRatio::Free => "Free",
            CropRatio::Ratio1x1 => "1:1",
            CropRatio::Ratio16x9 => "16:9",
            CropRatio::Ratio9x16 => "9:16",
        }
    }

    pub fn value(&self) -> Option<f32> {
        match self {
            CropRatio::Free => None,
            CropRatio::Ratio1x1 => Some(1.0),
            CropRatio::Ratio16x9 => Some(16.0 / 9.0),
            CropRatio::Ratio9x16 => Some(9.0 / 16.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
    MoveBox,
}

#[derive(Debug, Clone)]
pub struct CropState {
    /// Normalized coordinates in [0.0, 1.0] range relative to image
    pub normalized_rect: Rect,
    pub ratio: CropRatio,
    pub active_drag: Option<(CropHandle, Pos2, Rect)>, // handle, start_ptr_pos, initial_norm_rect
}

impl Default for CropState {
    fn default() -> Self {
        Self {
            normalized_rect: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
            ratio: CropRatio::Free,
            active_drag: None,
        }
    }
}

impl CropState {
    pub fn set_ratio(&mut self, ratio: CropRatio, img_w: u32, img_h: u32) {
        self.ratio = ratio;
        if let Some(target_ratio) = ratio.value() {
            let img_w_f = img_w as f32;
            let img_h_f = img_h as f32;
            let img_ratio = img_w_f / img_h_f;

            let (box_w, box_h) = if img_ratio > target_ratio {
                // Image is wider than target ratio: constrained by height
                (target_ratio * img_h_f, img_h_f)
            } else {
                // Image is taller than target ratio: constrained by width
                (img_w_f, img_w_f / target_ratio)
            };

            let norm_w = (box_w / img_w_f).clamp(0.01, 1.0);
            let norm_h = (box_h / img_h_f).clamp(0.01, 1.0);
            let min_x = (1.0 - norm_w) / 2.0;
            let min_y = (1.0 - norm_h) / 2.0;
            self.normalized_rect = Rect::from_min_size(Pos2::new(min_x, min_y), Vec2::new(norm_w, norm_h));
        }
    }

    pub fn update_drag(&mut self, ptr_pos: Pos2, img_rect: Rect, img_w: u32, img_h: u32) {
        let (handle, start_ptr, init_rect) = match self.active_drag {
            Some(d) => d,
            None => return,
        };

        if img_rect.width() <= 0.0 || img_rect.height() <= 0.0 {
            return;
        }

        let delta_screen = ptr_pos - start_ptr;
        let delta_norm_x = delta_screen.x / img_rect.width();
        let delta_norm_y = delta_screen.y / img_rect.height();

        let img_w_f = img_w as f32;
        let img_h_f = img_h as f32;

        let min_norm_w = (16.0 / img_w_f).min(0.5);
        let min_norm_h = (16.0 / img_h_f).min(0.5);

        if handle == CropHandle::MoveBox {
            let box_w = init_rect.width();
            let box_h = init_rect.height();
            let new_min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
            let new_min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
            self.normalized_rect = Rect::from_min_size(Pos2::new(new_min_x, new_min_y), Vec2::new(box_w, box_h));
            return;
        }

        let mut min_x = init_rect.min.x;
        let mut max_x = init_rect.max.x;
        let mut min_y = init_rect.min.y;
        let mut max_y = init_rect.max.y;

        match handle {
            CropHandle::TopLeft => {
                min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
                min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
            }
            CropHandle::TopRight => {
                max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
                min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
            }
            CropHandle::BottomLeft => {
                min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
                max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
            }
            CropHandle::BottomRight => {
                max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
                max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
            }
            CropHandle::Top => {
                min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
            }
            CropHandle::Bottom => {
                max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
            }
            CropHandle::Left => {
                min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
            }
            CropHandle::Right => {
                max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
            }
            CropHandle::MoveBox => unreachable!(),
        }

        // Apply ratio constraint if not Free
        if let Some(target_ratio) = self.ratio.value() {
            let current_norm_w = max_x - min_x;
            let current_norm_h = max_y - min_y;

            match handle {
                CropHandle::TopLeft => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_min_y = max_y - desired_norm_h;
                    if candidate_min_y >= 0.0 {
                        min_y = candidate_min_y;
                    } else {
                        min_y = 0.0;
                        let allowed_norm_h = max_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        min_x = (max_x - allowed_norm_w).max(0.0);
                    }
                }
                CropHandle::TopRight => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_min_y = max_y - desired_norm_h;
                    if candidate_min_y >= 0.0 {
                        min_y = candidate_min_y;
                    } else {
                        min_y = 0.0;
                        let allowed_norm_h = max_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        max_x = (min_x + allowed_norm_w).min(1.0);
                    }
                }
                CropHandle::BottomLeft => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_max_y = min_y + desired_norm_h;
                    if candidate_max_y <= 1.0 {
                        max_y = candidate_max_y;
                    } else {
                        max_y = 1.0;
                        let allowed_norm_h = 1.0 - min_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        min_x = (max_x - allowed_norm_w).max(0.0);
                    }
                }
                CropHandle::BottomRight => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_max_y = min_y + desired_norm_h;
                    if candidate_max_y <= 1.0 {
                        max_y = candidate_max_y;
                    } else {
                        max_y = 1.0;
                        let allowed_norm_h = 1.0 - min_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        max_x = (min_x + allowed_norm_w).min(1.0);
                    }
                }
                CropHandle::Top | CropHandle::Bottom => {
                    let desired_norm_w = (current_norm_h * target_ratio * img_h_f) / img_w_f;
                    let center_x = (min_x + max_x) / 2.0;
                    min_x = (center_x - desired_norm_w / 2.0).max(0.0);
                    max_x = (center_x + desired_norm_w / 2.0).min(1.0);
                }
                CropHandle::Left | CropHandle::Right => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let center_y = (min_y + max_y) / 2.0;
                    min_y = (center_y - desired_norm_h / 2.0).max(0.0);
                    max_y = (center_y + desired_norm_h / 2.0).min(1.0);
                }
                CropHandle::MoveBox => unreachable!(),
            }
        }

        self.normalized_rect = Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y));
    }
}

pub fn get_box_handles(rect: Rect) -> [(CropHandle, Pos2, egui::CursorIcon); 8] {
    [
        (CropHandle::TopLeft, rect.left_top(), egui::CursorIcon::ResizeNorthWest),
        (CropHandle::TopRight, rect.right_top(), egui::CursorIcon::ResizeNorthEast),
        (CropHandle::BottomLeft, rect.left_bottom(), egui::CursorIcon::ResizeSouthWest),
        (CropHandle::BottomRight, rect.right_bottom(), egui::CursorIcon::ResizeSouthEast),
        (CropHandle::Top, Pos2::new(rect.center().x, rect.top()), egui::CursorIcon::ResizeNorth),
        (CropHandle::Bottom, Pos2::new(rect.center().x, rect.bottom()), egui::CursorIcon::ResizeSouth),
        (CropHandle::Left, Pos2::new(rect.left(), rect.center().y), egui::CursorIcon::ResizeWest),
        (CropHandle::Right, Pos2::new(rect.right(), rect.center().y), egui::CursorIcon::ResizeEast),
    ]
}

pub fn resize_norm_rect(
    init_rect: Rect,
    handle: CropHandle,
    delta_screen: Vec2,
    img_rect: Rect,
    min_norm_w: f32,
    min_norm_h: f32,
) -> Rect {
    let delta_norm_x = delta_screen.x / img_rect.width();
    let delta_norm_y = delta_screen.y / img_rect.height();

    if handle == CropHandle::MoveBox {
        let box_w = init_rect.width();
        let box_h = init_rect.height();
        let new_min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
        let new_min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
        return Rect::from_min_size(Pos2::new(new_min_x, new_min_y), Vec2::new(box_w, box_h));
    }

    let mut min_x = init_rect.min.x;
    let mut max_x = init_rect.max.x;
    let mut min_y = init_rect.min.y;
    let mut max_y = init_rect.max.y;

    match handle {
        CropHandle::TopLeft => {
            min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
            min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
        }
        CropHandle::TopRight => {
            max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
            min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
        }
        CropHandle::BottomLeft => {
            min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
            max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
        }
        CropHandle::BottomRight => {
            max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
            max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
        }
        CropHandle::Top => {
            min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
        }
        CropHandle::Bottom => {
            max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
        }
        CropHandle::Left => {
            min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
        }
        CropHandle::Right => {
            max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
        }
        CropHandle::MoveBox => unreachable!(),
    }

    Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y))
}

#[derive(Debug, Clone, PartialEq)]
pub struct RedactionBox {
    /// Normalized coordinates in [0.0, 1.0] range relative to image
    pub normalized_rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RedactionDrag {
    Creating(Pos2, Pos2),
    Moving(usize, Pos2, Rect),
    Resizing(usize, CropHandle, Pos2, Rect),
}

#[derive(Debug, Clone, Default)]
pub struct RedactionState {
    pub boxes: Vec<RedactionBox>,
    pub selected_idx: Option<usize>,
    pub active_drag: Option<RedactionDrag>,
}

#[derive(Debug, Clone, Default)]
pub struct PickColorState {
    pub hovered_color: Option<Color32>,
    pub hovered_pixel: Option<(u32, u32)>,
    pub last_picked: Option<Color32>,
    pub recent_colors: Vec<Color32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeFilter {
    #[default]
    Lanczos3,
    Triangle,
    Nearest,
}

impl ResizeFilter {
    pub fn label(&self) -> &'static str {
        match self {
            ResizeFilter::Lanczos3 => "Lanczos3 (HQ)",
            ResizeFilter::Triangle => "Triangle (Smooth)",
            ResizeFilter::Nearest => "Nearest (Sharp)",
        }
    }

    pub fn to_image_filter(&self) -> image::imageops::FilterType {
        match self {
            ResizeFilter::Lanczos3 => image::imageops::FilterType::Lanczos3,
            ResizeFilter::Triangle => image::imageops::FilterType::Triangle,
            ResizeFilter::Nearest => image::imageops::FilterType::Nearest,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetFormat {
    #[default]
    Jpeg,
    Png,
}

impl TargetFormat {
    pub fn label(&self) -> &'static str {
        match self {
            TargetFormat::Jpeg => "JPEG",
            TargetFormat::Png => "PNG",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeSubTab {
    #[default]
    Pixels,
    FileSize,
}

#[derive(Debug, Clone)]
pub struct FileSizeOptimizationResult {
    pub target_bytes: usize,
    pub achieved_bytes: usize,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub quality: u8,
    pub format: TargetFormat,
    pub encoded_data: Vec<u8>,
    pub result_rgba: RgbaImage,
}

#[derive(Debug, Clone)]
pub struct ResizeState {
    pub sub_tab: ResizeSubTab,

    // Target Pixels
    pub target_width: u32,
    pub target_height: u32,
    pub width_input: String,
    pub height_input: String,
    pub lock_aspect: bool,
    pub filter: ResizeFilter,
    pub aspect_ratio_base: f32,

    // Target File Size
    pub target_size_val: f32,
    pub target_size_unit_mb: bool,
    pub target_format: TargetFormat,
    pub last_result: Option<FileSizeOptimizationResult>,
    #[allow(dead_code)]
    pub is_optimizing: bool,
}

impl Default for ResizeState {
    fn default() -> Self {
        Self {
            sub_tab: ResizeSubTab::Pixels,
            target_width: 1920,
            target_height: 1080,
            width_input: "1920".to_string(),
            height_input: "1080".to_string(),
            lock_aspect: true,
            filter: ResizeFilter::Lanczos3,
            aspect_ratio_base: 16.0 / 9.0,
            target_size_val: 2.0,
            target_size_unit_mb: true,
            target_format: TargetFormat::Jpeg,
            last_result: None,
            is_optimizing: false,
        }
    }
}

impl ResizeState {
    pub fn sync_from_image(&mut self, img: &RgbaImage) {
        let w = img.width().max(1);
        let h = img.height().max(1);
        self.target_width = w;
        self.target_height = h;
        self.width_input = w.to_string();
        self.height_input = h.to_string();
        self.aspect_ratio_base = w as f32 / h as f32;
    }

    pub fn set_width(&mut self, new_w: u32) {
        self.target_width = new_w.max(1);
        self.width_input = self.target_width.to_string();
        if self.lock_aspect && self.aspect_ratio_base > 0.0 {
            let new_h = ((self.target_width as f32) / self.aspect_ratio_base).round().max(1.0) as u32;
            self.target_height = new_h;
            self.height_input = new_h.to_string();
        }
    }

    pub fn set_height(&mut self, new_h: u32) {
        self.target_height = new_h.max(1);
        self.height_input = self.target_height.to_string();
        if self.lock_aspect && self.aspect_ratio_base > 0.0 {
            let new_w = ((self.target_height as f32) * self.aspect_ratio_base).round().max(1.0) as u32;
            self.target_width = new_w;
            self.width_input = new_w.to_string();
        }
    }

    pub fn set_scale_preset(&mut self, factor: f32, current_w: u32, current_h: u32) {
        let new_w = ((current_w as f32) * factor).round().max(1.0) as u32;
        let new_h = ((current_h as f32) * factor).round().max(1.0) as u32;
        self.target_width = new_w;
        self.target_height = new_h;
        self.width_input = new_w.to_string();
        self.height_input = new_h.to_string();
    }

    pub fn target_bytes(&self) -> usize {
        let bytes = if self.target_size_unit_mb {
            self.target_size_val * 1024.0 * 1024.0
        } else {
            self.target_size_val * 1024.0
        };
        (bytes.max(1024.0).round()) as usize
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteColor {
    pub color: Color32,
    pub hex: String,
    pub percentage: f32,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct PaletteState {
    pub extracted_colors: Vec<PaletteColor>,
    pub num_colors: usize,
    pub is_extracted: bool,
    pub total_sampled: usize,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            extracted_colors: Vec::new(),
            num_colors: 8,
            is_extracted: false,
            total_sampled: 0,
        }
    }
}

// ─── Film Simulation Types ─────────────────────────────────────────────────────

/// Descriptor for one entry in the film stock picker list.
#[derive(Clone)]
pub struct FilmStockEntry {
    pub brand: &'static str,
    pub name: &'static str,
    pub iso: u32,
    /// "Color Neg", "Color Slide", "B&W"
    pub film_type_label: &'static str,
    pub build: fn() -> filmr::film::FilmStock,
}

/// All available film stock presets, grouped by brand in order.
pub fn all_film_stocks() -> Vec<FilmStockEntry> {
    vec![
        // ── Kodak ──
        FilmStockEntry { brand: "Kodak", name: "Portra 160",        iso: 160,  film_type_label: "Color Neg",   build: filmr::presets::kodak::KODAK_PORTRA_160 },
        FilmStockEntry { brand: "Kodak", name: "Portra 400",        iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::kodak::KODAK_PORTRA_400 },
        FilmStockEntry { brand: "Kodak", name: "Portra 400 (Art)",  iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::kodak::KODAK_PORTRA_400_ARTISTIC },
        FilmStockEntry { brand: "Kodak", name: "Portra 800",        iso: 800,  film_type_label: "Color Neg",   build: filmr::presets::kodak::KODAK_PORTRA_800 },
        FilmStockEntry { brand: "Kodak", name: "Gold 200",          iso: 200,  film_type_label: "Color Neg",   build: filmr::presets::kodak::KODAK_GOLD_200 },
        FilmStockEntry { brand: "Kodak", name: "Ektar 100",         iso: 100,  film_type_label: "Color Neg",   build: filmr::presets::kodak::KODAK_EKTAR_100 },
        FilmStockEntry { brand: "Kodak", name: "Ektachrome 100",    iso: 100,  film_type_label: "Color Slide", build: filmr::presets::kodak::KODAK_EKTACHROME_100 },
        FilmStockEntry { brand: "Kodak", name: "Ektachrome 100VS",  iso: 100,  film_type_label: "Color Slide", build: filmr::presets::kodak::KODAK_EKTACHROME_100VS },
        FilmStockEntry { brand: "Kodak", name: "Kodachrome 25",     iso: 25,   film_type_label: "Color Slide", build: filmr::presets::kodak::KODAK_KODACHROME_25 },
        FilmStockEntry { brand: "Kodak", name: "Kodachrome 64",     iso: 64,   film_type_label: "Color Slide", build: filmr::presets::kodak::KODAK_KODACHROME_64 },
        FilmStockEntry { brand: "Kodak", name: "Tri-X 400",         iso: 400,  film_type_label: "B&W",         build: filmr::presets::kodak::KODAK_TRI_X_400 },
        FilmStockEntry { brand: "Kodak", name: "Tri-X 400 (Art)",   iso: 400,  film_type_label: "B&W",         build: filmr::presets::kodak::KODAK_TRI_X_400_ARTISTIC },
        FilmStockEntry { brand: "Kodak", name: "Plus-X 125",        iso: 125,  film_type_label: "B&W",         build: filmr::presets::kodak::KODAK_PLUS_X_125 },
        // ── Fujifilm ──
        FilmStockEntry { brand: "Fujifilm", name: "Velvia 50",         iso: 50,   film_type_label: "Color Slide", build: filmr::presets::fujifilm::VELVIA_50 },
        FilmStockEntry { brand: "Fujifilm", name: "Velvia 50 (Art)",    iso: 50,   film_type_label: "Color Slide", build: filmr::presets::fujifilm::VELVIA_50_ARTISTIC },
        FilmStockEntry { brand: "Fujifilm", name: "Provia 100F",        iso: 100,  film_type_label: "Color Slide", build: filmr::presets::fujifilm::PROVIA_100F },
        FilmStockEntry { brand: "Fujifilm", name: "Astia 100F",         iso: 100,  film_type_label: "Color Slide", build: filmr::presets::fujifilm::ASTIA_100F },
        FilmStockEntry { brand: "Fujifilm", name: "Superia 100",        iso: 100,  film_type_label: "Color Neg",   build: filmr::presets::fujifilm::SUPERIA_100 },
        FilmStockEntry { brand: "Fujifilm", name: "Superia 200",        iso: 200,  film_type_label: "Color Neg",   build: filmr::presets::fujifilm::SUPERIA_200 },
        FilmStockEntry { brand: "Fujifilm", name: "Superia 400",        iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::fujifilm::SUPERIA_400 },
        FilmStockEntry { brand: "Fujifilm", name: "Neopan 100",         iso: 100,  film_type_label: "B&W",         build: filmr::presets::fujifilm::NEOPAN_100 },
        FilmStockEntry { brand: "Fujifilm", name: "Neopan 400",         iso: 400,  film_type_label: "B&W",         build: filmr::presets::fujifilm::NEOPAN_400 },
        // ── Ilford ──
        FilmStockEntry { brand: "Ilford", name: "HP5 Plus 400",       iso: 400,  film_type_label: "B&W",         build: filmr::presets::ilford::HP5_PLUS_400 },
        FilmStockEntry { brand: "Ilford", name: "HP5 Plus 400 (Art)", iso: 400,  film_type_label: "B&W",         build: filmr::presets::ilford::HP5_PLUS_400_ARTISTIC },
        FilmStockEntry { brand: "Ilford", name: "FP4 Plus 125",       iso: 125,  film_type_label: "B&W",         build: filmr::presets::ilford::FP4_PLUS_125 },
        FilmStockEntry { brand: "Ilford", name: "Pan F Plus 50",      iso: 50,   film_type_label: "B&W",         build: filmr::presets::ilford::PAN_F_PLUS_50 },
        FilmStockEntry { brand: "Ilford", name: "Delta 100 Pro",      iso: 100,  film_type_label: "B&W",         build: filmr::presets::ilford::DELTA_100_PROFESSIONAL },
        FilmStockEntry { brand: "Ilford", name: "Delta 400 Pro",      iso: 400,  film_type_label: "B&W",         build: filmr::presets::ilford::DELTA_400_PROFESSIONAL },
        FilmStockEntry { brand: "Ilford", name: "XP2 Super 400",      iso: 400,  film_type_label: "B&W",         build: filmr::presets::ilford::XP2_SUPER_400 },
        FilmStockEntry { brand: "Ilford", name: "SFX 200",            iso: 200,  film_type_label: "B&W",         build: filmr::presets::ilford::SFX_200 },
        FilmStockEntry { brand: "Ilford", name: "Ortho Plus 80",      iso: 80,   film_type_label: "B&W",         build: filmr::presets::ilford::ORTHO_PLUS_80 },
        // ── Agfa ──
        FilmStockEntry { brand: "Agfa",  name: "Vista 100",   iso: 100,  film_type_label: "Color Neg",   build: filmr::presets::agfa::VISTA_100 },
        FilmStockEntry { brand: "Agfa",  name: "Vista 200",   iso: 200,  film_type_label: "Color Neg",   build: filmr::presets::agfa::VISTA_200 },
        FilmStockEntry { brand: "Agfa",  name: "Vista 400",   iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::agfa::VISTA_400 },
        FilmStockEntry { brand: "Agfa",  name: "APX 100",     iso: 100,  film_type_label: "B&W",         build: filmr::presets::agfa::APX_100 },
        FilmStockEntry { brand: "Agfa",  name: "APX 400",     iso: 400,  film_type_label: "B&W",         build: filmr::presets::agfa::APX_400 },
        FilmStockEntry { brand: "Agfa",  name: "Precisa 100", iso: 100,  film_type_label: "Color Slide", build: filmr::presets::agfa::PRECISA_100 },
        FilmStockEntry { brand: "Agfa",  name: "Scala 200",   iso: 200,  film_type_label: "B&W",         build: filmr::presets::agfa::SCALA_200 },
        FilmStockEntry { brand: "Agfa",  name: "Optima 200",  iso: 200,  film_type_label: "Color Neg",   build: filmr::presets::agfa::OPTIMA_200 },
        // ── Polaroid ──
        FilmStockEntry { brand: "Polaroid", name: "600 Color",    iso: 640,  film_type_label: "Color Neg",   build: filmr::presets::polaroid::POLAROID_600_COLOR },
        FilmStockEntry { brand: "Polaroid", name: "SX-70 Color",  iso: 160,  film_type_label: "Color Neg",   build: filmr::presets::polaroid::POLAROID_SX70_COLOR },
        FilmStockEntry { brand: "Polaroid", name: "i-Type Color", iso: 640,  film_type_label: "Color Neg",   build: filmr::presets::polaroid::POLAROID_I_TYPE_COLOR },
        FilmStockEntry { brand: "Polaroid", name: "B&W 667",      iso: 3000, film_type_label: "B&W",         build: filmr::presets::polaroid::POLAROID_BW_667 },
        FilmStockEntry { brand: "Polaroid", name: "Spectra Color", iso: 640, film_type_label: "Color Neg",   build: filmr::presets::polaroid::POLAROID_SPECTRA_COLOR },
        FilmStockEntry { brand: "Polaroid", name: "100 Color",    iso: 100,  film_type_label: "Color Neg",   build: filmr::presets::polaroid::POLAROID_100_COLOR },
        FilmStockEntry { brand: "Polaroid", name: "55 B&W",       iso: 50,   film_type_label: "B&W",         build: filmr::presets::polaroid::POLAROID_55_BW },
        // ── CineStill / Lomography / Other ──
        FilmStockEntry { brand: "CineStill",   name: "800T",              iso: 800,  film_type_label: "Color Neg",   build: filmr::presets::other::CINESTILL_800T },
        FilmStockEntry { brand: "CineStill",   name: "50D",               iso: 50,   film_type_label: "Color Neg",   build: filmr::presets::other::CINESTILL_50D },
        FilmStockEntry { brand: "Lomography",  name: "Color Chrome",      iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::other::LOMOGRAPHY_COLOR_CHROME },
        FilmStockEntry { brand: "Lomography",  name: "LomoChrome Purple", iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::other::LOMOGRAPHY_LOMOCHROME_PURPLE },
        FilmStockEntry { brand: "Ferrania",    name: "Solaris 100",       iso: 100,  film_type_label: "Color Neg",   build: filmr::presets::other::FERRANIA_SOLARIS_100 },
        FilmStockEntry { brand: "Ferrania",    name: "Solaris 400",       iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::other::FERRANIA_SOLARIS_400 },
        FilmStockEntry { brand: "ORWO",        name: "UN54",              iso: 100,  film_type_label: "B&W",         build: filmr::presets::other::ORWO_UN54 },
        FilmStockEntry { brand: "ORWO",        name: "UN64",              iso: 400,  film_type_label: "B&W",         build: filmr::presets::other::ORWO_UN64 },
        FilmStockEntry { brand: "Ricoh",       name: "GR Street",         iso: 400,  film_type_label: "B&W",         build: filmr::presets::other::RICOH_GR_STREET },
        FilmStockEntry { brand: "Lucky",       name: "Color 200",         iso: 200,  film_type_label: "Color Neg",   build: filmr::presets::other::LUCKY_COLOR_200 },
        FilmStockEntry { brand: "Generic",     name: "Standard Daylight", iso: 400,  film_type_label: "Color Neg",   build: filmr::presets::other::STANDARD_DAYLIGHT },
    ]
}

/// Helper: renders a slider with full visual context (start, center, end, and default tick marks,
/// gauge fill line from default to current, and double-click to reset to default).
fn render_context_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default_val: f32,
    suffix: &str,
    slider_width: f32,
    palette: &AppPalette,
) -> bool {
    let mut changed = false;
    let b_med  = hex_to_c32(&palette.theme.b_med);
    let base_c = hex_to_c32(&palette.harmonies.base);
    let f_med  = hex_to_c32(&palette.theme.f_med);
    let f_low  = hex_to_c32(&palette.theme.f_low);

    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().color(f_med));

        ui.spacing_mut().slider_width = slider_width;
        let mut cur_val = *value;
        let slider = egui::Slider::new(&mut cur_val, range.clone())
            .show_value(true)
            .min_decimals(2)
            .max_decimals(2)
            .custom_formatter(|n, _| {
                if suffix.is_empty() {
                    format!("{:.2}", n)
                } else if suffix == "%" {
                    format!("{:.0}%", n * 100.0)
                } else {
                    format!("{:.2}{}", n, suffix)
                }
            });

        let response = ui.add(slider);
        if response.changed() {
            *value = cur_val;
            changed = true;
        }

        if response.double_clicked() {
            *value = default_val;
            changed = true;
        }

        let def_display = if suffix == "%" {
            format!("{:.0}%", default_val * 100.0)
        } else if suffix.is_empty() {
            format!("{:.2}", default_val)
        } else {
            format!("{:.2}{}", default_val, suffix)
        };
        response.on_hover_text(format!("Double-click to reset (default: {})", def_display));

        // Background context track bar beneath slider
        let min_val = *range.start();
        let max_val = *range.end();
        let span = (max_val - min_val).max(1e-5);

        let (track_rect, _) = ui.allocate_exact_size(Vec2::new(slider_width, 6.0), egui::Sense::hover());
        let painter = ui.painter();

        let y_bar = track_rect.center().y;
        let x_start = track_rect.min.x;
        let x_end = track_rect.max.x;
        let x_mid = (x_start + x_end) * 0.5;

        // Baseline track
        painter.line_segment(
            [Pos2::new(x_start, y_bar), Pos2::new(x_end, y_bar)],
            Stroke::new(1.0, b_med),
        );

        // Start tick (min)
        painter.line_segment(
            [Pos2::new(x_start, y_bar - 2.5), Pos2::new(x_start, y_bar + 2.5)],
            Stroke::new(1.0, f_low),
        );

        // End tick (max)
        painter.line_segment(
            [Pos2::new(x_end, y_bar - 2.5), Pos2::new(x_end, y_bar + 2.5)],
            Stroke::new(1.0, f_low),
        );

        // Center tick (midpoint of range)
        painter.line_segment(
            [Pos2::new(x_mid, y_bar - 2.0), Pos2::new(x_mid, y_bar + 2.0)],
            Stroke::new(1.0, f_low),
        );

        // Default tick (in theme accent)
        let def_norm = ((default_val - min_val) / span).clamp(0.0, 1.0);
        let x_def = x_start + def_norm * (x_end - x_start);
        painter.line_segment(
            [Pos2::new(x_def, y_bar - 3.5), Pos2::new(x_def, y_bar + 3.5)],
            Stroke::new(1.5, base_c),
        );

        // Gauge fill line from default to current value
        let cur_norm = ((*value - min_val) / span).clamp(0.0, 1.0);
        let x_cur = x_start + cur_norm * (x_end - x_start);

        let (g_start, g_end) = if x_cur >= x_def {
            (x_def, x_cur)
        } else {
            (x_cur, x_def)
        };
        if (g_end - g_start).abs() > 0.5 {
            painter.line_segment(
                [Pos2::new(g_start, y_bar), Pos2::new(g_end, y_bar)],
                Stroke::new(2.0, base_c),
            );
        }

        // Value indicator dot
        painter.circle_filled(Pos2::new(x_cur, y_bar), 2.5, base_c);
    });

    changed
}

/// Applies post-processing photographic S-curve contrast and green/magenta tint.
pub fn apply_film_sim_post_adjustments(
    out_rgb: &image::RgbImage,
    contrast: f32,
    tint: f32,
) -> image::RgbaImage {
    let has_contrast = contrast.abs() > 0.001;
    let has_tint = tint.abs() > 0.001;

    if !has_contrast && !has_tint {
        return image::RgbaImage::from_fn(out_rgb.width(), out_rgb.height(), |x, y| {
            let p = out_rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
    }

    // Precompute 256-entry S-curve LUT for contrast
    let mut lut = [0u8; 256];
    if has_contrast {
        let k = if contrast >= 0.0 {
            1.0 + contrast * 2.2
        } else {
            1.0 / (1.0 + contrast.abs() * 2.2)
        };
        for (i, slot) in lut.iter_mut().enumerate() {
            let t = i as f32 / 255.0;
            let curved = if t < 0.5 {
                0.5 * (2.0 * t).powf(k)
            } else {
                1.0 - 0.5 * (2.0 * (1.0 - t)).powf(k)
            };
            *slot = (curved.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    } else {
        for (i, slot) in lut.iter_mut().enumerate() {
            *slot = i as u8;
        }
    }

    // Tint multipliers (Green vs Magenta)
    let (r_mult, g_mult, b_mult) = if has_tint {
        if tint > 0.0 {
            // Magenta: boost Red and Blue, slightly reduce Green
            (1.0 + 0.18 * tint, 1.0 - 0.18 * tint, 1.0 + 0.18 * tint)
        } else {
            // Green: boost Green, slightly reduce Red and Blue
            let t = tint.abs();
            (1.0 - 0.18 * t, 1.0 + 0.18 * t, 1.0 - 0.18 * t)
        }
    } else {
        (1.0, 1.0, 1.0)
    };

    image::RgbaImage::from_fn(out_rgb.width(), out_rgb.height(), |x, y| {
        let p = out_rgb.get_pixel(x, y);
        let r_c = lut[p[0] as usize] as f32;
        let g_c = lut[p[1] as usize] as f32;
        let b_c = lut[p[2] as usize] as f32;

        let r_final = (r_c * r_mult).clamp(0.0, 255.0).round() as u8;
        let g_final = (g_c * g_mult).clamp(0.0, 255.0).round() as u8;
        let b_final = (b_c * b_mult).clamp(0.0, 255.0).round() as u8;
        image::Rgba([r_final, g_final, b_final, 255])
    })
}

/// Messages sent from the film sim background render thread.
#[derive(Debug)]
pub enum FilmSimMsg {
    Done { rgba: RgbaImage, duration_ms: u128 },
    Error(String),
}

/// Message streamed from the asynchronous film simulation thumbnail worker.
pub struct FilmSimThumbnailMsg {
    pub epoch: u64,
    pub stock_idx: usize,
    pub rgba: RgbaImage,
}

/// Creative light leak presets mapped to physics-based `filmr::light_leak::LightLeakConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightLeakPreset {
    #[default]
    None,
    WarmCorner,
    SunStreak,
    SoftEdge,
    OrganicBlob,
}

impl LightLeakPreset {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::WarmCorner => "Warm Corner",
            Self::SunStreak => "Sun Streak",
            Self::SoftEdge => "Soft Edge",
            Self::OrganicBlob => "Organic",
        }
    }

    pub fn to_config(&self) -> LightLeakConfig {
        match self {
            Self::None => LightLeakConfig {
                enabled: false,
                leaks: Vec::new(),
            },
            Self::WarmCorner => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.9, 0.1),
                    color: [1.0, 0.55, 0.2],
                    radius: 0.45,
                    intensity: 1.1,
                    shape: LightLeakShape::Circle,
                    rotation: 0.0,
                    roughness: 0.4,
                }],
            },
            Self::SunStreak => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.15, 0.15),
                    color: [1.0, 0.8, 0.35],
                    radius: 0.6,
                    intensity: 1.2,
                    shape: LightLeakShape::Linear,
                    rotation: 0.75,
                    roughness: 0.3,
                }],
            },
            Self::SoftEdge => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.05, 0.5),
                    color: [0.95, 0.4, 0.2],
                    radius: 0.5,
                    intensity: 0.9,
                    shape: LightLeakShape::Circle,
                    rotation: 0.0,
                    roughness: 0.6,
                }],
            },
            Self::OrganicBlob => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.8, 0.8),
                    color: [1.0, 0.45, 0.3],
                    radius: 0.55,
                    intensity: 1.15,
                    shape: LightLeakShape::Organic,
                    rotation: 0.3,
                    roughness: 0.7,
                }],
            },
        }
    }
}

/// All state for the Film Simulation tab.
pub struct FilmSimState {
    // ── Exposure & Color Parameters ──
    pub selected_stock_idx: usize,
    /// Exposure multiplier: 0.1 .. 4.0, default 1.0
    pub brightness: f32,
    /// Post-process S-curve contrast: -1.0 .. +1.0, default 0.0
    pub contrast: f32,
    /// Warmth: -1.0 .. +1.0, default 0.0
    pub warmth: f32,
    /// Green vs Magenta tint: -1.0 .. +1.0, default 0.0 (negative = green, positive = magenta)
    pub tint: f32,
    /// Saturation intensity: 0.0 .. 2.0, default 1.0
    pub intensity: f32,
    /// White balance correction mode
    pub wb_mode: WhiteBalanceMode,
    /// White balance correction strength: 0.0 .. 1.0, default 1.0
    pub wb_strength: f32,

    // ── Optical & Creative Effects ──
    /// Halation glow strength: 0.0 .. 1.0, default 0.25
    pub halation: f32,
    /// Vignette falloff strength: 0.0 .. 1.0, default 0.35
    pub vignette: f32,
    /// Creative light leak preset
    pub light_leak: LightLeakPreset,
    pub enable_grain: bool,
    pub style: FilmStyle,

    // ── Preview state ──
    pub preview_rgba: Option<RgbaImage>,
    pub preview_texture: Option<egui::TextureHandle>,
    pub dirty_preview_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub last_render_duration_ms: u128,

    // ── Async render worker (Main image) ──
    pub is_rendering: bool,
    pub render_rx: Option<std::sync::mpsc::Receiver<FilmSimMsg>>,
    pub render_error: Option<String>,

    // ── Async thumbnail worker (Sidebar 42x28 mini-photos) ──
    pub stock_thumbnails: std::collections::HashMap<usize, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(usize, image::RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<FilmSimThumbnailMsg>>,
    pub thumbnail_epoch: u64,
    pub thumbnail_dirty_at: Option<std::time::Instant>,
    pub is_generating_thumbnails: bool,

    // ── Debounce timer ──
    /// Set to `Some(Instant)` whenever a parameter changes; cleared when render fires.
    pub params_dirty_at: Option<std::time::Instant>,

    // ── UI helpers ──
    pub search_query: String,
    pub auto_fitted: bool,
}

impl Default for FilmSimState {
    fn default() -> Self {
        Self {
            selected_stock_idx: 1, // Default: Kodak Portra 400
            brightness: 1.0,
            contrast: 0.0,
            warmth: 0.0,
            tint: 0.0,
            intensity: 1.0,
            wb_mode: WhiteBalanceMode::Auto,
            wb_strength: 1.0,
            halation: 0.25,
            vignette: 0.35,
            light_leak: LightLeakPreset::None,
            enable_grain: true,
            style: FilmStyle::Accurate,
            preview_rgba: None,
            preview_texture: None,
            dirty_preview_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            last_render_duration_ms: 0,
            is_rendering: false,
            render_rx: None,
            render_error: None,
            stock_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_epoch: 0,
            thumbnail_dirty_at: None,
            is_generating_thumbnails: false,
            params_dirty_at: None,
            search_query: String::new(),
            auto_fitted: false,
        }
    }
}

impl FilmSimState {
    /// Mark parameters as dirty (triggers debounced re-render for both main preview and thumbnails).
    pub fn mark_dirty(&mut self) {
        let now = std::time::Instant::now();
        self.params_dirty_at = Some(now);
        self.thumbnail_dirty_at = Some(now + std::time::Duration::from_millis(250));
    }
}

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
            CreativeFxType::Trichromatic => CreativeFxParams::Trichromatic(self.trichromatic_params),
            CreativeFxType::PixelSort => CreativeFxParams::PixelSort(self.pixel_sort_params),
            CreativeFxType::CmykHalftone => CreativeFxParams::CmykHalftone(self.cmyk_params),
            CreativeFxType::DitherPalette => CreativeFxParams::DitherPalette(self.dither_params),
            CreativeFxType::Ntsc => CreativeFxParams::Ntsc(self.ntsc_params),
        }
    }
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
    texture: Option<egui::TextureHandle>,
    dirty_texture: bool,

    // Undo / Redo Stack
    pub undo_stack: Vec<RgbaImage>,
    pub redo_stack: Vec<RgbaImage>,

    // Cropping
    pub crop_state: CropState,

    // Redaction State
    pub redaction_state: RedactionState,

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

    // Configured External Programs
    pub external_editor: String,
    pub svg_editor: String,
    pub viewer: String,

    // Status Toast (message, timestamp)
    pub status_toast: Option<(String, f64)>,

    // File Dialog & Save-As
    pub save_dialog_rx: Option<std::sync::mpsc::Receiver<PathBuf>>,
    pub show_fallback_save_modal: bool,
    pub fallback_save_path: String,

    // File Size Optimizer Async Worker
    pub optimizer_rx: Option<std::sync::mpsc::Receiver<OptimizerMsg>>,
    pub optimizer_status: Option<String>,

    // Background Removal Async Worker & State
    pub bg_removal_in_progress: bool,
    pub bg_removal_rx: Option<std::sync::mpsc::Receiver<BgRemovalMsg>>,
    pub bg_removal_status: Option<String>,
    pub bg_removal_error: Option<String>,
    pub bg_removal_cmd: Option<String>,
    pub multimatte_prompt: String,

    // Format Conversion (Squoosh-style)
    pub convert_state: ConvertState,

    // Multi-Model Document OCR
    pub ocr_state: OcrState,
    pub ocr_rx: Option<std::sync::mpsc::Receiver<OcrMsg>>,
    pub ocr_child_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,

    // Barcode / QR / DataMatrix Decoder
    pub barcode_state: BarcodeScanState,

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

#[derive(Debug)]
pub enum BgRemovalMsg {
    Progress(String),
    Success {
        result_rgba: RgbaImage,
        duration_ms: u128,
    },
    Error(String),
}

#[derive(Debug)]
pub enum OptimizerMsg {
    Progress(String),
    Done(FileSizeOptimizationResult),
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

        let mut preview = Self {
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
            texture: None,
            dirty_texture: true,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            crop_state: CropState::default(),
            redaction_state: RedactionState::default(),
            pick_color_state: PickColorState::default(),
            resize_state,
            palette_state,
            pan: Vec2::ZERO,
            zoom: 1.0,
            auto_fitted: false,
            active_mode: ImageToolMode::View,
            external_editor: config.image.external_editor.clone(),
            svg_editor: config.image.svg_editor.clone(),
            viewer: config.image.viewer.clone(),
            status_toast: None,
            save_dialog_rx: None,
            show_fallback_save_modal: false,
            fallback_save_path: String::new(),
            optimizer_rx: None,
            optimizer_status: None,
            bg_removal_in_progress: false,
            bg_removal_rx: None,
            bg_removal_status: None,
            bg_removal_error: None,
            bg_removal_cmd: config.image.bg_removal_cmd.clone(),
            multimatte_prompt: String::new(),
            convert_state: ConvertState::default(),
            ocr_state: OcrState::default(),
            ocr_rx: None,
            ocr_child_pid: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            barcode_state: BarcodeScanState::default(),
            dcs_state: DecorrelationStretchState::default(),
            film_sim_state: FilmSimState::default(),
            creative_fx_state: CreativeFxState::default(),
            metadata,
            show_metadata_drawer: false,
            deskew_state: DeskewState::default(),
        };

        preview.update_conversion();
        Ok(preview)
    }

    pub fn show_toast(&mut self, msg: impl Into<String>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.status_toast = Some((msg.into(), now));
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

        if self.active_mode == ImageToolMode::Crop {
            let norm = self.crop_state.normalized_rect;
            let img_w = img.width() as f32;
            let img_h = img.height() as f32;

            let px_x = (norm.min.x * img_w).round().clamp(0.0, img_w - 1.0) as u32;
            let px_y = (norm.min.y * img_h).round().clamp(0.0, img_h - 1.0) as u32;
            let px_w = ((norm.max.x - norm.min.x) * img_w).round().clamp(1.0, img_w - px_x as f32) as u32;
            let px_h = ((norm.max.y - norm.min.y) * img_h).round().clamp(1.0, img_h - px_y as f32) as u32;

            if px_w > 0 && px_h > 0 && (px_w < img.width() || px_h < img.height() || px_x > 0 || px_y > 0) {
                return image::imageops::crop_imm(&img, px_x, px_y, px_w, px_h).to_image();
            }
        }

        img
    }

    /// Burns all pending redaction black bars permanently into current_rgba and records an undo point.
    pub fn apply_redactions(&mut self) {
        if self.redaction_state.boxes.is_empty() {
            return;
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let img_w = self.current_rgba.width() as f32;
        let img_h = self.current_rgba.height() as f32;
        let count = self.redaction_state.boxes.len();

        for b in &self.redaction_state.boxes {
            let px_min_x = (b.normalized_rect.min.x * img_w).round().clamp(0.0, img_w) as u32;
            let px_max_x = (b.normalized_rect.max.x * img_w).round().clamp(0.0, img_w) as u32;
            let px_min_y = (b.normalized_rect.min.y * img_h).round().clamp(0.0, img_h) as u32;
            let px_max_y = (b.normalized_rect.max.y * img_h).round().clamp(0.0, img_h) as u32;

            for y in px_min_y..px_max_y {
                for x in px_min_x..px_max_x {
                    self.current_rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }

        self.redaction_state.boxes.clear();
        self.redaction_state.selected_idx = None;
        self.redaction_state.active_drag = None;
        self.dirty_texture = true;

        // Automatically ensure metadata is stripped when redactions are permanently burned
        if let Ok(clean) = strip_metadata(&self.current_rgba, &self.mime) {
            self.raw_bytes = clean;
            self.file_size_bytes = self.raw_bytes.len();
            self.metadata = None;
        }

        self.show_toast(format!("█ Burned {} redaction bars into image", count));
    }

    /// Deletes the currently selected redaction bar.
    pub fn delete_selected_redaction(&mut self) {
        if let Some(idx) = self.redaction_state.selected_idx {
            if idx < self.redaction_state.boxes.len() {
                self.redaction_state.boxes.remove(idx);
                self.redaction_state.selected_idx = None;
                self.redaction_state.active_drag = None;
                self.show_toast("🗑 Redaction bar removed");
            }
        }
    }

    /// Clears all pending redaction bars without burning.
    pub fn clear_all_redactions(&mut self) {
        let count = self.redaction_state.boxes.len();
        if count > 0 {
            self.redaction_state.boxes.clear();
            self.redaction_state.selected_idx = None;
            self.redaction_state.active_drag = None;
            self.show_toast(format!("✕ Cleared {} pending redaction bars", count));
        }
    }

    /// Strips all EXIF, GPS, camera metadata, and timestamps by cleanly re-encoding pristine pixels.
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
    pub fn apply_deskew(&mut self) {
        let w_f = self.current_rgba.width() as f32;
        let h_f = self.current_rgba.height() as f32;
        let c = self.deskew_state.corners;
        let quad = QuadCorners::new(
            [c[0].x * w_f, c[0].y * h_f],
            [c[1].x * w_f, c[1].y * h_f],
            [c[2].x * w_f, c[2].y * h_f],
            [c[3].x * w_f, c[3].y * h_f],
        );

        if self.deskew_state.is_clicking_points {
            self.show_toast("⚠ Place all 4 points first, or press Esc to cancel");
            return;
        }

        let (est_w, est_h) = quad.estimate_dimensions();

        match rectify_perspective_with_mode(
            &self.current_rgba,
            &quad,
            est_w,
            est_h,
            self.deskew_state.crop_mode,
        ) {
            Ok(rectified) => {
                let out_w = rectified.width();
                let out_h = rectified.height();
                self.undo_stack.push(self.current_rgba.clone());
                self.redo_stack.clear();
                self.current_rgba = rectified;
                self.dirty_texture = true;
                self.resize_state.sync_from_image(&self.current_rgba);
                self.deskew_state.reset_quad();
                let mode_str = match self.deskew_state.crop_mode {
                    DeskewCropMode::CropToQuad => "cropped to quad",
                    DeskewCropMode::MinimalCrop => "minimal crop",
                };
                self.show_toast(format!(
                    "📐 Deskewed perspective ({}): {}×{} px",
                    mode_str, out_w, out_h
                ));
            }
            Err(e) => {
                self.show_toast(format!("⚠ Deskew failed: {}", e));
            }
        }
    }

    /// Resizes current_rgba to the specified pixel dimensions using the chosen filter,
    /// recording an undo point.
    pub fn apply_pixel_resize(&mut self, target_w: u32, target_h: u32, filter: ResizeFilter) {
        let (w, h) = (target_w.clamp(1, 16384), target_h.clamp(1, 16384));
        if w == self.current_rgba.width() && h == self.current_rgba.height() {
            return;
        }

        // Burn any pending redactions first so they scale with the image
        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let resized = image::imageops::resize(&self.current_rgba, w, h, filter.to_image_filter());
        self.current_rgba = resized;
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.show_toast(format!("↔ Resized to {}×{} px ({})", w, h, filter.label()));
    }

    /// Runs the target file size optimizer solving the quality vs. resolution trade-off asynchronously.
    pub fn run_target_file_size_optimizer(&mut self) {
        if self.resize_state.is_optimizing {
            return;
        }

        let target_bytes = self.resize_state.target_bytes();
        let format = self.resize_state.target_format;
        let base_img = self.current_display_image();

        let (tx, rx) = std::sync::mpsc::channel::<OptimizerMsg>();
        self.optimizer_rx = Some(rx);
        self.resize_state.is_optimizing = true;
        self.optimizer_status = Some("Starting optimizer...".to_string());

        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let progress_cb = move |msg: String| {
                let _ = tx_prog.send(OptimizerMsg::Progress(msg));
            };

            let result = optimize_to_target_size_with_progress(
                &base_img,
                target_bytes,
                format,
                Some(&progress_cb),
            );

            let _ = tx.send(OptimizerMsg::Done(result));
        });
    }

    /// Polls the background file size optimizer channel, updating status messages
    /// and storing the final result when complete.
    pub fn poll_optimizer(&mut self) {
        let mut done_res = None;
        let mut disconnected = false;
        if let Some(rx) = &self.optimizer_rx {
            loop {
                match rx.try_recv() {
                    Ok(OptimizerMsg::Progress(status)) => {
                        self.optimizer_status = Some(status);
                    }
                    Ok(OptimizerMsg::Done(res)) => {
                        done_res = Some(res);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if let Some(res) = done_res {
            self.resize_state.last_result = Some(res);
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
        } else if disconnected {
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
        }
    }

    /// Applies the optimized image result directly to current_rgba.
    pub fn apply_optimized_result(&mut self) {
        let res = match self.resize_state.last_result.clone() {
            Some(r) => r,
            None => return,
        };

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        self.current_rgba = res.result_rgba;
        self.file_size_bytes = res.achieved_bytes;
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.show_toast(format!(
            "✔ Applied optimized {}×{} image ({})",
            res.width,
            res.height,
            format_bytes_size(res.achieved_bytes)
        ));
    }

    /// Copies the optimized result directly to the Left Hand clipboard slot via daemon IPC
    /// and synchronizes with OS clipboard.
    pub fn copy_optimized_to_slot(&mut self) {
        let res = match &self.resize_state.last_result {
            Some(r) => r,
            None => return,
        };

        let mime = match res.format {
            TargetFormat::Jpeg => "image/jpeg",
            TargetFormat::Png => "image/png",
        };

        let req = IpcRequest::PushImage {
            bytes: res.encoded_data.clone(),
            mime: mime.to_string(),
            hand: HandSide::Left,
        };
        let ipc_ok = crate::send_ipc_request(&req).is_ok();

        let bytes_for_cmd = res.encoded_data.clone();
        let mime_for_cmd = mime.to_string();
        std::thread::spawn(move || {
            if let Ok(mut child) = Command::new("wl-copy")
                .arg("-t")
                .arg(&mime_for_cmd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, &bytes_for_cmd);
                }
                let _ = child.wait();
            } else if let Ok(mut child) = Command::new("xclip")
                .arg("-selection")
                .arg("clipboard")
                .arg("-t")
                .arg(&mime_for_cmd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, &bytes_for_cmd);
                }
                let _ = child.wait();
            }
        });

        if ipc_ok {
            self.show_toast(format!(
                "📋 Copied optimized {} ({}) to Clipboard Slot (LH)",
                format_bytes_size(res.achieved_bytes),
                res.format.label()
            ));
        } else {
            self.show_toast(format!(
                "📋 Copied optimized {} ({}) to OS Clipboard",
                format_bytes_size(res.achieved_bytes),
                res.format.label()
            ));
        }
    }

    /// Extracts dominant colors from current_display_image into palette_state.
    pub fn run_palette_extraction(&mut self) {
        let disp_img = self.current_display_image();
        let (colors, total_sampled) = extract_palette(&disp_img, self.palette_state.num_colors);
        self.palette_state.extracted_colors = colors;
        self.palette_state.total_sampled = total_sampled;
        self.palette_state.is_extracted = true;
        self.show_toast(format!(
            "🎨 Extracted {} dominant colors ({} sampled px)",
            self.palette_state.extracted_colors.len(),
            total_sampled
        ));
    }

    /// Copies the given color as a hex code string to the clipboard and tracks recent colors.
    pub fn copy_color_to_clipboard(&mut self, col: Color32, ctx: &egui::Context) {
        let hex = format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b());
        ctx.output_mut(|o| o.copied_text = hex.clone());

        let hex_for_cmd = hex.clone();
        std::thread::spawn(move || {
            let _ = Command::new("wl-copy").arg(&hex_for_cmd).output();
            let _ = Command::new("xclip").arg("-selection").arg("clipboard").arg(&hex_for_cmd).output();
        });

        self.pick_color_state.last_picked = Some(col);
        if !self.pick_color_state.recent_colors.contains(&col) {
            self.pick_color_state.recent_colors.insert(0, col);
            if self.pick_color_state.recent_colors.len() > 12 {
                self.pick_color_state.recent_colors.pop();
            }
        }

        self.show_toast(format!("💉 Copied {} to clipboard!", hex));
    }

    /// Copies the currently displayed image to a specific clipboard hand slot (Left or Right) via daemon IPC,
    /// and simultaneously syncs to the OS clipboard (wl-copy / xclip).
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

        // 2. Also pipe directly to wl-copy / xclip for instantaneous OS clipboard sync
        let bytes_for_cmd = png_bytes;
        std::thread::spawn(move || {
            if let Ok(mut child) = Command::new("wl-copy")
                .arg("-t")
                .arg("image/png")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, &bytes_for_cmd);
                }
                let _ = child.wait();
            } else if let Ok(mut child) = Command::new("xclip")
                .arg("-selection")
                .arg("clipboard")
                .arg("-t")
                .arg("image/png")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, &bytes_for_cmd);
                }
                let _ = child.wait();
            }
        });

        let slot_label = match hand {
            HandSide::Left => "Left Hand (LH)",
            HandSide::Right => "Right Hand (RH)",
        };

        if ipc_ok {
            self.show_toast(format!("📋 Copied {}×{} image to Clipboard Slot ({})", w, h, slot_label));
        } else {
            self.show_toast(format!("📋 Copied {}×{} image to OS Clipboard", w, h));
        }
    }

    /// Copies the currently displayed image to the Left Hand clipboard slot via daemon IPC,
    /// and simultaneously syncs to the OS clipboard (wl-copy / xclip).
    pub fn copy_to_clipboard_slot(&mut self) {
        self.copy_to_hand_slot(HandSide::Left);
    }

    /// Runs AI background removal asynchronously using feyninc/nobg (FeyNoBg).
    pub fn run_background_removal(&mut self) {
        if self.bg_removal_in_progress {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png) {
            self.bg_removal_error = Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        // Determine executable command
        let (cmd_bin, cmd_args) = if let Some(ref cmd) = self.bg_removal_cmd {
            let parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
            if !parts.is_empty() {
                (parts[0].clone(), parts[1..].to_vec())
            } else {
                ("scripts/remove_bg.py".to_string(), Vec::new())
            }
        } else {
            let candidates = [
                "scripts/remove_bg.py",
                "./scripts/remove_bg.py",
            ];
            let mut found = None;
            for path in &candidates {
                if std::path::Path::new(path).exists() {
                    found = Some((path.to_string(), Vec::new()));
                    break;
                }
            }
            found.unwrap_or_else(|| ("python3".to_string(), vec!["scripts/remove_bg.py".to_string()]))
        };

        let prompt_opt = if !self.multimatte_prompt.trim().is_empty() {
            Some(self.multimatte_prompt.trim().to_string())
        } else {
            None
        };
        let status_desc = if let Some(ref p) = prompt_opt {
            format!("Segmenting '{}' with MultiMatte...", p)
        } else {
            "Starting FeyNoBg AI model...".to_string()
        };

        let (tx, rx) = std::sync::mpsc::channel::<BgRemovalMsg>();
        self.bg_removal_rx = Some(rx);
        self.bg_removal_in_progress = true;
        self.bg_removal_status = Some(status_desc);
        self.bg_removal_error = None;

        let thread_prompt = prompt_opt.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let tx_prog = tx.clone();
            let init_msg = if let Some(ref p) = thread_prompt {
                format!("Invoking MultiMatte for '{}'...", p)
            } else {
                "Invoking FeyNoBg neural network...".to_string()
            };
            let _ = tx_prog.send(BgRemovalMsg::Progress(init_msg));

            let mut cmd = Command::new(&cmd_bin);
            cmd.args(&cmd_args);
            if let Some(ref p) = thread_prompt {
                cmd.arg("--prompt").arg(p);
            }
            cmd.arg("-").arg("-");

            let mut child = match cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to launch background removal process '{}': {}. Ensure Python with nobg is installed.",
                        cmd_bin, e
                    )));
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    let _ = tx.send(BgRemovalMsg::Error(format!("Failed to write image data to worker: {}", e)));
                    return;
                }
            }

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!("Worker process failed: {}", e)));
                    return;
                }
            };

            if !output.status.success() {
                let err_str = String::from_utf8_lossy(&output.stderr);
                let _ = tx.send(BgRemovalMsg::Error(format!(
                    "Background removal failed (exit code {:?}):\n{}",
                    output.status.code(),
                    err_str.trim()
                )));
                return;
            }

            let res_img = match image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png) {
                Ok(dyn_img) => dyn_img.to_rgba8(),
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!("Failed to decode output PNG: {}", e)));
                    return;
                }
            };

            let duration_ms = start.elapsed().as_millis();
            let _ = tx.send(BgRemovalMsg::Success {
                result_rgba: res_img,
                duration_ms,
            });
        });
    }

    /// Runs AI depth estimation asynchronously using Depth-Anything / MiDaS.
    pub fn run_depth_estimation(&mut self) {
        if self.bg_removal_in_progress {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png) {
            self.bg_removal_error = Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        let depth_script = "scripts/depth_estimation.py";
        let (cmd_bin, cmd_args) = if std::path::Path::new(depth_script).exists() {
            (depth_script.to_string(), Vec::new())
        } else {
            ("python3".to_string(), vec![depth_script.to_string()])
        };

        let (tx, rx) = std::sync::mpsc::channel::<BgRemovalMsg>();
        self.bg_removal_rx = Some(rx);
        self.bg_removal_in_progress = true;
        self.bg_removal_status = Some("Estimating depth map with Depth-Anything...".to_string());
        self.bg_removal_error = None;

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let tx_prog = tx.clone();
            let _ = tx_prog.send(BgRemovalMsg::Progress("Invoking depth estimation neural network...".to_string()));

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
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to launch depth estimation '{}': {}. Ensure Python environment is setup.",
                        cmd_bin, e
                    )));
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    let _ = tx.send(BgRemovalMsg::Error(format!("Failed to write image data to worker: {}", e)));
                    return;
                }
            }

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!("Depth estimation process failed: {}", e)));
                    return;
                }
            };

            if !output.status.success() {
                let err_str = String::from_utf8_lossy(&output.stderr);
                let _ = tx.send(BgRemovalMsg::Error(format!(
                    "Depth estimation failed:\n{}",
                    err_str.trim()
                )));
                return;
            }

            let res_img = match image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png) {
                Ok(dyn_img) => dyn_img.to_rgba8(),
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!("Failed to decode output PNG: {}", e)));
                    return;
                }
            };

            let duration_ms = start.elapsed().as_millis();
            let _ = tx.send(BgRemovalMsg::Success {
                result_rgba: res_img,
                duration_ms,
            });
        });
    }

    /// Polls the background removal worker channel and applies result on success.
    pub fn poll_bg_removal(&mut self) {
        let mut success_data = None;
        let mut error_msg = None;
        let mut disconnected = false;

        if let Some(rx) = &self.bg_removal_rx {
            loop {
                match rx.try_recv() {
                    Ok(BgRemovalMsg::Progress(st)) => {
                        self.bg_removal_status = Some(st);
                    }
                    Ok(BgRemovalMsg::Success { result_rgba, duration_ms }) => {
                        success_data = Some((result_rgba, duration_ms));
                        break;
                    }
                    Ok(BgRemovalMsg::Error(err)) => {
                        error_msg = Some(err);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if let Some((result_rgba, duration_ms)) = success_data {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = result_rgba;
            self.dirty_texture = true;
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_error = None;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;

            self.show_toast(format!("✨ Background removed in {:.2}s!", duration_ms as f64 / 1000.0));
        } else if let Some(err) = error_msg {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_error = Some(err);
        } else if disconnected {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
        }
    }

    /// Opens a native save-as dialog (zenity) or falls back to an in-app path input modal.
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

        let zenity_available = Path::new("/usr/bin/zenity").exists()
            || Command::new("zenity").arg("--version").output().is_ok();

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

        let zenity_available = Path::new("/usr/bin/zenity").exists()
            || Command::new("zenity").arg("--version").output().is_ok();

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
                let _ = std::fs::create_dir_all(parent);
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
            std::fs::write(&final_path, &self.convert_state.encoded_bytes).map_err(image::ImageError::IoError)
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
                Err(image::ImageError::IoError(std::io::Error::new(std::io::ErrorKind::Other, "WebP encoding failed")))
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

    pub fn apply_crop(&mut self) {
        let img_w = self.current_rgba.width() as f32;
        let img_h = self.current_rgba.height() as f32;
        let norm = self.crop_state.normalized_rect;

        let px_x = (norm.min.x * img_w).round().clamp(0.0, img_w - 1.0) as u32;
        let px_y = (norm.min.y * img_h).round().clamp(0.0, img_h - 1.0) as u32;
        let px_w = ((norm.max.x - norm.min.x) * img_w).round().clamp(1.0, img_w - px_x as f32) as u32;
        let px_h = ((norm.max.y - norm.min.y) * img_h).round().clamp(1.0, img_h - px_y as f32) as u32;

        if px_w > 0 && px_h > 0 && (px_w < self.current_rgba.width() || px_h < self.current_rgba.height() || px_x > 0 || px_y > 0) {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();

            let cropped = image::imageops::crop_imm(&self.current_rgba, px_x, px_y, px_w, px_h).to_image();
            self.current_rgba = cropped;
            self.dirty_texture = true;
            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.show_toast(format!("✂ Cropped to {}×{}", px_w, px_h));
        }
    }

    pub fn rotate_cw(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::rotate90(&self.current_rgba);
        self.dirty_texture = true;
        self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("⟳ Rotated 90° Clockwise");
    }

    pub fn rotate_ccw(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::rotate270(&self.current_rgba);
        self.dirty_texture = true;
        self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("⟲ Rotated 90° Counter-Clockwise");
    }

    pub fn rotate_180(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::rotate180(&self.current_rgba);
        self.dirty_texture = true;
        self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("🔄 Rotated 180°");
    }

    pub fn flip_horizontal(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::flip_horizontal(&self.current_rgba);
        self.dirty_texture = true;
        self.show_toast("⇆ Flipped Horizontally");
    }

    pub fn flip_vertical(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::flip_vertical(&self.current_rgba);
        self.dirty_texture = true;
        self.show_toast("⇅ Flipped Vertically");
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.current_rgba.clone());
            self.current_rgba = prev;
            self.dirty_texture = true;
            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
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
            self.dirty_texture = true;
            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("⟳ Redone");
        }
    }

    pub fn revert_to_original(&mut self) {
        if !self.undo_stack.is_empty() {
            self.undo_stack.push(self.current_rgba.clone());
            self.current_rgba = self.original_rgba.clone();
            self.dirty_texture = true;
            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("↺ Reverted to original image");
        }
    }

    /// Ensure the file exists on disk so external programs can open it.
    /// If image was modified, saves modified buffer to disk.
    pub fn ensure_disk_path(&self) -> Result<PathBuf, String> {
        let staging_dir = little_clip_core::staging::get_staging_dir();

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
            self.current_rgba.save(&staging_path)
                .map_err(|e| format!("Failed to save modified staging file: {}", e))?;
        } else {
            std::fs::write(&staging_path, &self.raw_bytes)
                .map_err(|e| format!("Failed to write staging file: {}", e))?;
        }

        Ok(staging_path)
    }

    pub fn launch_external_app(&mut self, app: &str, ctx: &egui::Context, close: &mut bool) {
        match self.ensure_disk_path() {
            Ok(file_path) => {
                let mut cmd = Command::new(app);
                cmd.arg(&file_path);
                match cmd.spawn() {
                    Ok(_) => {
                        let filename = file_path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "file".to_string());
                        self.show_toast(format!("🚀 Launched {} with {}", app, filename));
                        *close = true;
                        // Close overall workbench floating window so external app receives full focus
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => {
                        self.show_toast(format!("⚠️ Failed to launch '{}': {}", app, e));
                    }
                }
            }
            Err(e) => {
                self.show_toast(format!("⚠️ File error: {}", e));
            }
        }
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette, close: &mut bool) {
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

        // Render fallback save-as modal if active
        if self.show_fallback_save_modal {
            egui::Window::new("💾 Save Image As")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Enter destination file path:").strong());
                    ui.add_space(4.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.fallback_save_path)
                            .desired_width(380.0),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new("💾 Save")
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .clicked()
                        {
                            let p = PathBuf::from(&self.fallback_save_path);
                            self.save_image_to_path(&p);
                            self.show_fallback_save_modal = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_fallback_save_modal = false;
                        }
                    });
                });
        }

        // Global hotkey handling
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.ocr_state.in_progress {
                self.cancel_ocr();
            } else if self.active_mode == ImageToolMode::Crop && self.crop_state.normalized_rect != Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)) {
                self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                self.crop_state.ratio = CropRatio::Free;
            } else if self.active_mode == ImageToolMode::Deskew && self.deskew_state.is_clicking_points {
                self.deskew_state.cancel_clicking_points();
                self.show_toast("Cancelled 4-point placement");
            } else if self.active_mode == ImageToolMode::Redact && self.redaction_state.selected_idx.is_some() {
                self.redaction_state.selected_idx = None;
            } else if self.active_mode == ImageToolMode::Redact && !self.redaction_state.boxes.is_empty() {
                self.clear_all_redactions();
            } else {
                if self.ocr_state.in_progress {
                    self.cancel_ocr();
                }
                *close = true;
                return;
            }
        }
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            if ui.input(|i| i.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y)) {
            self.redo();
        }
        if self.active_mode == ImageToolMode::Crop && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.apply_crop();
        }
        if self.active_mode == ImageToolMode::Deskew && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.apply_deskew();
        }
        if self.active_mode == ImageToolMode::Redact && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.apply_redactions();
        }
        if self.active_mode == ImageToolMode::Resize && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if self.resize_state.sub_tab == ResizeSubTab::Pixels {
                self.apply_pixel_resize(
                    self.resize_state.target_width,
                    self.resize_state.target_height,
                    self.resize_state.filter,
                );
            }
        }
        if self.active_mode == ImageToolMode::RemoveBg && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.run_background_removal();
        }
        if self.active_mode == ImageToolMode::DepthMap && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.run_depth_estimation();
        }
        if self.active_mode == ImageToolMode::Convert && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.apply_converted_to_canvas();
        }
        if self.active_mode == ImageToolMode::Ocr && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.run_ocr();
        }
        if self.active_mode == ImageToolMode::Barcode && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.run_barcode_scan();
        }
        if self.active_mode == ImageToolMode::DecorrelationStretch && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.apply_decorrelation_stretch();
        }
        if !ctx.wants_keyboard_input() {
            if self.active_mode == ImageToolMode::Redact {
                let del_pressed = ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                    || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
                if del_pressed {
                    self.delete_selected_redaction();
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Num0)) {
                self.fit_to_view(ui.available_size());
            }
            if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus)) {
                self.zoom = (self.zoom * 1.25).clamp(0.05, 50.0);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.zoom = (self.zoom / 1.25).clamp(0.05, 50.0);
            }

            // Unmodified single-key tool selection shortcuts
            let no_modifiers = ui.input(|i| !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.mac_cmd);
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
            if self.creative_fx_state.is_rendering || self.creative_fx_state.is_generating_thumbnails {
                ctx.request_repaint();
            }
        }

        // Copy to clipboard slot (Ctrl+C / Cmd+C) at any point
        if !ctx.wants_keyboard_input() {
            let copy_requested = ui.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || (i.key_pressed(egui::Key::C) && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
            }) || ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
            });

            if copy_requested {
                self.copy_to_clipboard_slot();
            }

            // Save As shortcut (Ctrl+S / Cmd+S)
            let save_requested = ui.input(|i| {
                i.key_pressed(egui::Key::S) && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd)
            }) || ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
            });

            if save_requested {
                self.open_save_dialog();
            }
        }

        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.background))
            .show(ui, |ui| {
                // 1. TOP HEADER & ESCAPE HATCHES
                self.render_header(ui, ctx, palette, close);

                ui.add_space(4.0);

                // 2. TOOLBAR (Mode selection & Viewport controls)
                self.render_toolbar(ui, palette);

                // 2b. Secondary tool-specific strip (View / Crop / Deskew / Transforms / Redact / PickColor / Resize / Palette / RemoveBg / DepthMap)
                if self.active_mode == ImageToolMode::View {
                    ui.add_space(2.0);
                    self.render_view_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Crop {
                    ui.add_space(2.0);
                    self.render_crop_toolbar(ui, palette);
                } else if self.active_mode == ImageToolMode::Deskew {
                    ui.add_space(2.0);
                    self.render_deskew_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Transform {
                    ui.add_space(2.0);
                    self.render_transform_toolbar(ui, palette);
                } else if self.active_mode == ImageToolMode::Redact {
                    ui.add_space(2.0);
                    self.render_redact_toolbar(ui, palette);
                } else if self.active_mode == ImageToolMode::PickColor {
                    ui.add_space(2.0);
                    self.render_pick_color_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Resize {
                    ui.add_space(2.0);
                    self.render_resize_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Convert {
                    ui.add_space(2.0);
                    self.render_convert_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Ocr {
                    ui.add_space(2.0);
                    self.render_ocr_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Barcode {
                    ui.add_space(2.0);
                    self.render_barcode_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::Palette {
                    ui.add_space(2.0);
                    self.render_pick_color_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::RemoveBg {
                    ui.add_space(2.0);
                    self.render_remove_bg_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::DepthMap {
                    ui.add_space(2.0);
                    self.render_depth_map_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::DecorrelationStretch {
                    ui.add_space(2.0);
                    self.render_decorrelation_stretch_toolbar(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::FilmSim {
                    // Film sim has its own full-viewport layout — no secondary strip needed here;
                    // the render_film_sim_viewport handles all three panels.
                } else if self.active_mode == ImageToolMode::CreativeFx {
                    // Creative FX has its own full 3-panel layout — no secondary strip needed here;
                    // the render_creative_fx_viewport handles all three panels.
                }

                ui.add_space(4.0);

                // 3. MAIN INTERACTIVE VIEWPORT
                if self.active_mode == ImageToolMode::FilmSim {
                    self.render_film_sim_viewport(ui, ctx, palette);
                } else if self.active_mode == ImageToolMode::CreativeFx {
                    self.render_creative_fx_viewport(ui, ctx, palette);
                } else {
                    self.render_viewport(ui, ctx, palette);
                }

                // 4. BOTTOM STATUS / HELP BAR
                self.render_footer(ui, palette, now);
            });
    }

    fn render_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette, close: &mut bool) {
        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 8.0))
            .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Title badge
                    ui.label(
                        egui::RichText::new("🖼 IMAGE STUDIO")
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_high)),
                    );

                    ui.add_space(10.0);

                    // Dimensions chip
                    let aspect_str = format_aspect_ratio(self.current_rgba.width(), self.current_rgba.height());
                    let dim_text = format!(
                        "{}×{} ({})",
                        self.current_rgba.width(),
                        self.current_rgba.height(),
                        aspect_str
                    );
                    crate::ui_ext::badge_pill(ui, &dim_text, Some(hex_to_c32(&palette.harmonies.split_comp_1)), palette);

                    // Format chip
                    let fmt_str = if self.is_svg && self.undo_stack.is_empty() {
                        "SVG (VECTOR)".to_string()
                    } else {
                        self.mime.replace("image/", "").to_uppercase()
                    };
                    crate::ui_ext::badge_pill(ui, &fmt_str, Some(hex_to_c32(&palette.diff_add)), palette);

                    // File size chip
                    let size_str = format_bytes_size(self.file_size_bytes);
                    crate::ui_ext::badge_pill(ui, &size_str, Some(hex_to_c32(&palette.diff_mod)), palette);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Close button
                        let close_bg = hex_to_c32(&palette.theme.b_med);
                        let close_fg = contrast_text_c32(close_bg, Some(hex_to_c32(&palette.diff_del)), palette);
                        let close_btn = egui::Button::new(
                            egui::RichText::new("✕ Close (Esc)")
                                .small()
                                .strong()
                                .color(close_fg),
                        )
                        .fill(close_bg)
                        .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_low)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(close_btn).clicked() {
                            if self.ocr_state.in_progress {
                                self.cancel_ocr();
                            }
                            *close = true;
                        }

                        ui.add_space(8.0);

                        // Escape Hatch 3: Viewer (Nomacs / default)
                        let viewer_label = format!("👁 {}", self.viewer);
                        let viewer_btn = crate::secondary_button(viewer_label, hex_to_c32(&palette.theme.b_med), palette)
                            .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_low)));
                        if ui.add(viewer_btn).on_hover_text(format!("Open in external viewer ({})", self.viewer)).clicked() {
                            let v = self.viewer.clone();
                            self.launch_external_app(&v, ctx, close);
                        }

                        // Escape Hatch 2: Inkscape (SVG)
                        let inkscape_label = format!("✒ {}", self.svg_editor);
                        let is_primary_svg = self.is_svg && self.undo_stack.is_empty();
                        let ink_bg = if is_primary_svg { hex_to_c32(&palette.harmonies.triadic_1) } else { hex_to_c32(&palette.theme.b_med) };
                        let ink_pref = if is_primary_svg { hex_to_c32(&palette.theme.f_high) } else { hex_to_c32(&palette.theme.f_low) };
                        let inkscape_btn = crate::themed_button(inkscape_label, ink_bg, Some(ink_pref), palette)
                            .stroke(Stroke::new(1.0, if is_primary_svg { hex_to_c32(&palette.harmonies.triadic_1) } else { hex_to_c32(&palette.theme.b_low) }));
                        if ui.add(inkscape_btn).on_hover_text("Open file in Inkscape (Vector Editor)").clicked() {
                            let ed = self.svg_editor.clone();
                            self.launch_external_app(&ed, ctx, close);
                        }

                        // Escape Hatch 1: Krita (Raster)
                        let krita_label = format!("🎨 {}", self.external_editor);
                        let is_primary_raster = !is_primary_svg;
                        let krita_bg = if is_primary_raster { hex_to_c32(&palette.harmonies.base) } else { hex_to_c32(&palette.theme.b_med) };
                        let krita_pref = if is_primary_raster { hex_to_c32(&palette.theme.f_high) } else { hex_to_c32(&palette.theme.f_low) };
                        let krita_btn = crate::themed_button(krita_label, krita_bg, Some(krita_pref), palette)
                            .stroke(Stroke::new(1.0, if is_primary_raster { hex_to_c32(&palette.harmonies.base) } else { hex_to_c32(&palette.theme.b_low) }));
                        if ui.add(krita_btn).on_hover_text("Open file in Krita (Full Photo & Painting Studio)").clicked() {
                            let ed = self.external_editor.clone();
                            self.launch_external_app(&ed, ctx, close);
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        // Save As button
                        let save_btn = crate::action_button("💾 Save As...", hex_to_c32(&palette.diff_add), palette)
                            .stroke(Stroke::new(1.0, hex_to_c32(&palette.diff_add)));
                        if ui.add(save_btn).on_hover_text("Save current image to file (Ctrl+S)").clicked() {
                            self.open_save_dialog();
                        }

                        // Copy to Clipboard Slot button
                        let copy_btn = crate::action_button("📋 Copy (Ctrl+C)", hex_to_c32(&palette.harmonies.base), palette)
                            .stroke(Stroke::new(1.0, hex_to_c32(&palette.harmonies.base)));
                        if ui.add(copy_btn).on_hover_text("Copy current image to Left Hand clipboard slot & OS clipboard (Ctrl+C)").clicked() {
                            self.copy_to_clipboard_slot();
                        }
                    });
                });
            });
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
        if prev_mode == ImageToolMode::Ocr && target_mode != ImageToolMode::Ocr && self.ocr_state.in_progress {
            self.cancel_ocr();
        }
        match target_mode {
            ImageToolMode::View
            | ImageToolMode::Crop
            | ImageToolMode::Deskew
            | ImageToolMode::Transform
            | ImageToolMode::Redact
            | ImageToolMode::Ocr
            | ImageToolMode::RemoveBg
            | ImageToolMode::DepthMap => {}
            ImageToolMode::PickColor | ImageToolMode::Palette => {
                if !self.palette_state.is_extracted || self.palette_state.extracted_colors.is_empty() {
                    self.run_palette_extraction();
                }
            }
            ImageToolMode::Resize => {
                self.resize_state.sync_from_image(&self.current_rgba);
            }
            ImageToolMode::Convert => {
                if self.convert_state.converted_rgba.is_none() {
                    self.update_conversion();
                }
            }
            ImageToolMode::DecorrelationStretch => {
                if self.dcs_state.stretched_rgba.is_none() {
                    self.update_decorrelation_stretch();
                }
            }
            ImageToolMode::Barcode => {
                if !self.barcode_state.has_scanned {
                    self.run_barcode_scan();
                }
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
                // Trigger initial render if no preview yet.
                if self.creative_fx_state.preview_rgba.is_none() && !self.creative_fx_state.is_rendering {
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
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Tool Modes
                    let modes = [
                        ImageToolMode::View,
                        ImageToolMode::Crop,
                        ImageToolMode::Deskew,
                        ImageToolMode::Transform,
                        ImageToolMode::Redact,
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
                        let is_active = self.active_mode == mode || (mode == ImageToolMode::PickColor && self.active_mode == ImageToolMode::Palette);
                        let btn = crate::mode_tab_button(mode.label(), is_active, palette);

                        if ui.add(btn).clicked() {
                            self.set_tool_mode(mode);
                        }
                    }


                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Reset View
                        let reset_btn = crate::secondary_button("⟲ Reset", hex_to_c32(&palette.theme.b_med), palette);
                        if ui.add(reset_btn).on_hover_text("Reset zoom and pan").clicked() {
                            self.zoom = 1.0;
                            self.pan = Vec2::ZERO;
                        }

                        // 1:1 Pixel Zoom
                        let pixel_btn = crate::secondary_button("1:1", hex_to_c32(&palette.theme.b_med), palette);
                        if ui.add(pixel_btn).on_hover_text("100% actual pixel size (1)").clicked() {
                            self.zoom = 1.0;
                        }

                        // Fit to window
                        let fit_btn = crate::themed_button(
                            "⛶ Fit",
                            hex_to_c32(&palette.theme.b_med),
                            Some(hex_to_c32(&palette.harmonies.base)),
                            palette,
                        );
                        if ui.add(fit_btn).on_hover_text("Fit entire image into window (0 / F)").clicked() {
                            self.fit_to_view(ui.available_size());
                        }

                        // Zoom In (+)
                        let plus_btn = crate::themed_button(
                            "+",
                            hex_to_c32(&palette.theme.b_med),
                            Some(hex_to_c32(&palette.theme.f_high)),
                            palette,
                        );
                        if ui.add(plus_btn).clicked() {
                            self.zoom = (self.zoom * 1.25).clamp(0.05, 50.0);
                        }

                        // Zoom percentage readout
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                                .small()
                                .color(hex_to_c32(&palette.theme.f_low)),
                        );

                        // Zoom Out (-)
                        let minus_btn = crate::themed_button(
                            "-",
                            hex_to_c32(&palette.theme.b_med),
                            Some(hex_to_c32(&palette.theme.f_high)),
                            palette,
                        );
                        if ui.add(minus_btn).clicked() {
                            self.zoom = (self.zoom / 1.25).clamp(0.05, 50.0);
                        }
                    });
                });
            });
    }

    fn render_view_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("🔍 Inspector:")
                                .strong()
                                .small()
                                .color(hex_to_c32(&palette.harmonies.base)),
                        );

                        if let Some(meta) = &self.metadata {
                            if let Some(cam) = meta.camera_summary() {
                                crate::ui_ext::badge_pill(ui, &format!("📷 {}", cam), Some(hex_to_c32(&palette.harmonies.split_comp_1)), palette);
                            }
                            if let Some(exp) = meta.exposure_summary() {
                                crate::ui_ext::badge_pill(ui, &format!("⚡ {}", exp), Some(hex_to_c32(&palette.diff_mod)), palette);
                            }
                            if let Some(dt) = &meta.date_time {
                                crate::ui_ext::badge_pill(ui, &format!("📅 {}", dt), Some(hex_to_c32(&palette.theme.f_med)), palette);
                            }
                            if let Some(gps) = meta.gps_display() {
                                crate::ui_ext::badge_pill(ui, &format!("📍 {}", gps), Some(hex_to_c32(&palette.diff_add)), palette);
                                if let Some(osm) = meta.osm_url() {
                                    let map_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_add)), palette);
                                    let map_btn = egui::Button::new(
                                        egui::RichText::new("🗺️ Map").small().strong().color(map_fg),
                                    )
                                    .fill(btn_bg)
                                    .stroke(Stroke::new(1.0, btn_border))
                                    .rounding(Rounding::same(4.0));
                                    if ui.add(map_btn).on_hover_text("Open GPS location in OpenStreetMap").clicked() {
                                        ctx.open_url(egui::output::OpenUrl::new_tab(osm));
                                    }
                                }
                            }

                            // Drawer toggle button
                            let toggle_txt = if self.show_metadata_drawer { "▲ Hide Details" } else { "▼ EXIF Details" };
                            let toggle_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                            let toggle_btn = egui::Button::new(
                                egui::RichText::new(toggle_txt).small().strong().color(toggle_fg),
                            )
                            .fill(btn_bg)
                            .stroke(Stroke::new(1.0, btn_border))
                            .rounding(Rounding::same(4.0));
                            if ui.add(toggle_btn).clicked() {
                                self.show_metadata_drawer = !self.show_metadata_drawer;
                            }
                        } else {
                            ui.label(
                                egui::RichText::new("🛡️ Clean Image (No EXIF or metadata tags detected)")
                                    .small()
                                    .color(hex_to_c32(&palette.theme.f_med)),
                            );
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let dim_str = format!("{} × {} px", self.current_rgba.width(), self.current_rgba.height());
                            ui.label(egui::RichText::new(dim_str).small().color(hex_to_c32(&palette.theme.f_low)));
                        });
                    });

                    // Expanded Metadata Drawer
                    if self.show_metadata_drawer {
                        if let Some(meta) = &self.metadata {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .show(ui, |ui| {
                                    egui::Grid::new("exif_metadata_grid")
                                        .striped(true)
                                        .spacing(Vec2::new(16.0, 4.0))
                                        .show(ui, |ui| {
                                            if let Some(m) = &meta.make {
                                                ui.label(egui::RichText::new("Camera Make:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(m).small());
                                                ui.end_row();
                                            }
                                            if let Some(m) = &meta.model {
                                                ui.label(egui::RichText::new("Camera Model:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(m).small());
                                                ui.end_row();
                                            }
                                            if let Some(l) = &meta.lens_model {
                                                ui.label(egui::RichText::new("Lens:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(l).small());
                                                ui.end_row();
                                            }
                                            if let Some(s) = &meta.software {
                                                ui.label(egui::RichText::new("Software:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(s).small());
                                                ui.end_row();
                                            }
                                            if let Some(dt) = &meta.date_time {
                                                ui.label(egui::RichText::new("Timestamp:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(dt).small());
                                                ui.end_row();
                                            }
                                            if let Some(iso) = meta.iso {
                                                ui.label(egui::RichText::new("ISO Speed:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(iso.to_string()).small());
                                                ui.end_row();
                                            }
                                            if let Some(f) = meta.f_number {
                                                ui.label(egui::RichText::new("Aperture:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(format!("f/{:.1}", f)).small());
                                                ui.end_row();
                                            }
                                            if let Some(exp) = &meta.exposure_time {
                                                ui.label(egui::RichText::new("Shutter Speed:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(exp).small());
                                                ui.end_row();
                                            }
                                            if let Some(fl) = meta.focal_length {
                                                ui.label(egui::RichText::new("Focal Length:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(format!("{:.1} mm", fl)).small());
                                                ui.end_row();
                                            }
                                            if let (Some(lat), Some(lon)) = (meta.gps_latitude, meta.gps_longitude) {
                                                ui.label(egui::RichText::new("GPS Coordinates:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.horizontal(|ui| {
                                                    ui.label(egui::RichText::new(format!("{:.6}, {:.6}", lat, lon)).small());
                                                    let copy_btn = egui::Button::new(egui::RichText::new("📋 Copy").small())
                                                        .fill(btn_bg)
                                                        .stroke(Stroke::new(1.0, btn_border));
                                                    if ui.add(copy_btn).clicked() {
                                                        ctx.output_mut(|o| o.copied_text = format!("{:.6}, {:.6}", lat, lon));
                                                    }
                                                });
                                                ui.end_row();
                                            }
                                            if let Some(alt) = meta.gps_altitude {
                                                ui.label(egui::RichText::new("GPS Altitude:").strong().small().color(hex_to_c32(&palette.theme.f_med)));
                                                ui.label(egui::RichText::new(format!("{:.1} m", alt)).small());
                                                ui.end_row();
                                            }
                                        });
                                });
                        }
                    }
                });
            });
    }

    fn render_crop_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("✂ Crop Mode:")
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Ratio:")
                            .small()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    let ratios = [
                        CropRatio::Free,
                        CropRatio::Ratio1x1,
                        CropRatio::Ratio16x9,
                        CropRatio::Ratio9x16,
                    ];

                    for r in ratios {
                        let is_sel = self.crop_state.ratio == r;
                        let btn = crate::mode_tab_button(r.label(), is_sel, palette);

                        if ui.add(btn).clicked() {
                            self.crop_state.set_ratio(r, self.current_rgba.width(), self.current_rgba.height());
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Apply Crop Button
                        let apply_btn = crate::action_button("✓ Apply Crop (Enter)", hex_to_c32(&palette.diff_add), palette);

                        if ui.add(apply_btn).clicked() {
                            self.apply_crop();
                        }

                        // Reset Crop Button
                        let reset_crop_btn = crate::secondary_button("✕ Full Image", hex_to_c32(&palette.theme.b_med), palette);

                        if ui.add(reset_crop_btn).clicked() {
                            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                            self.crop_state.ratio = CropRatio::Free;
                        }

                        if !self.undo_stack.is_empty() {
                            let undo_bg = hex_to_c32(&palette.theme.b_med);
                            let undo_fg = contrast_text_c32(undo_bg, Some(hex_to_c32(&palette.diff_mod)), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                    .small()
                                    .strong()
                                    .color(undo_fg),
                            )
                            .fill(undo_bg)
                            .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_low)))
                            .rounding(Rounding::same(4.0));

                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    fn render_deskew_toolbar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📐 4-Point Deskew:")
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(4.0);

                    // 1. Click 4 Points toggle button
                    let is_placing = self.deskew_state.is_clicking_points;
                    let click_lbl = if is_placing {
                        format!("🎯 Placing ({}/4)...", self.deskew_state.placed_points.len())
                    } else {
                        "🎯 Click 4 Points".to_string()
                    };
                    let click_btn = crate::mode_tab_button(&click_lbl, is_placing, palette);
                    if ui.add(click_btn)
                        .on_hover_text("Click 4 corners directly on the image: Top-Left → Top-Right → Bottom-Right → Bottom-Left")
                        .clicked()
                    {
                        if is_placing {
                            self.deskew_state.cancel_clicking_points();
                        } else {
                            self.deskew_state.start_clicking_points();
                            self.show_toast("🎯 Click 4 corners: TL → TR → BR → BL");
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // 2. Cropping mode buttons (Crop to Quad vs Minimal Crop)
                    ui.label(
                        egui::RichText::new("Cropping:")
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    let is_crop_to_quad = self.deskew_state.crop_mode == DeskewCropMode::CropToQuad;
                    let crop_quad_btn = crate::mode_tab_button("✂ Crop to Quad", is_crop_to_quad, palette);
                    if ui.add(crop_quad_btn)
                        .on_hover_text("Crop tightly to the 4-point quadrilateral")
                        .clicked()
                    {
                        self.deskew_state.crop_mode = DeskewCropMode::CropToQuad;
                    }

                    let is_minimal_crop = self.deskew_state.crop_mode == DeskewCropMode::MinimalCrop;
                    let minimal_crop_btn = crate::mode_tab_button("🖼 Minimal Crop", is_minimal_crop, palette);
                    if ui.add(minimal_crop_btn)
                        .on_hover_text("Preserve the entire rectified image, cropping only as strictly necessary")
                        .clicked()
                    {
                        self.deskew_state.crop_mode = DeskewCropMode::MinimalCrop;
                    }

                    ui.add_space(6.0);

                    // 3. Target dimensions calculation preview
                    let w_f = self.current_rgba.width() as f32;
                    let h_f = self.current_rgba.height() as f32;
                    let c = self.deskew_state.corners;
                    let quad = QuadCorners::new(
                        [c[0].x * w_f, c[0].y * h_f],
                        [c[1].x * w_f, c[1].y * h_f],
                        [c[2].x * w_f, c[2].y * h_f],
                        [c[3].x * w_f, c[3].y * h_f],
                    );
                    let (est_w, est_h) = quad.estimate_dimensions();
                    let (disp_w, disp_h) = match self.deskew_state.crop_mode {
                        DeskewCropMode::CropToQuad => (est_w, est_h),
                        DeskewCropMode::MinimalCrop => quad.estimate_minimal_crop_dimensions(
                            self.current_rgba.width(),
                            self.current_rgba.height(),
                            est_w,
                            est_h,
                        ),
                    };

                    crate::ui_ext::badge_pill(
                        ui,
                        &format!("Output: ~{}×{} px", disp_w, disp_h),
                        Some(hex_to_c32(&palette.harmonies.split_comp_1)),
                        palette,
                    );

                    ui.add_space(8.0);

                    // 4. Apply Deskew button
                    let apply_bg = hex_to_c32(&palette.diff_add);
                    let apply_fg = contrast_text_c32(apply_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                    let apply_btn = egui::Button::new(
                        egui::RichText::new("✓ Apply Deskew (Enter)")
                            .small()
                            .strong()
                            .color(apply_fg),
                    )
                    .fill(apply_bg)
                    .stroke(Stroke::new(1.0, apply_bg))
                    .rounding(Rounding::same(4.0));

                    if ui.add(apply_btn).on_hover_text("Rectify quad perspective into rectangular output (Enter)").clicked() {
                        self.apply_deskew();
                    }

                    // 5. Reset button
                    let reset_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_med)), palette);
                    let reset_btn = egui::Button::new(
                        egui::RichText::new("⟲ Reset Quad").small().color(reset_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));

                    if ui.add(reset_btn).on_hover_text("Reset corner handles to default positions").clicked() {
                        self.deskew_state.reset_quad();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("Click 4 points or drag handles")
                                .small()
                                .color(hex_to_c32(&palette.theme.f_low)),
                        );
                    });
                });
            });
    }

    fn render_transform_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);
        let btn_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🔄 Transforms:")
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(8.0);

                    // Rotate CCW
                    let ccw_btn = egui::Button::new(
                        egui::RichText::new("⟲ 90° CCW").small().strong().color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui.add(ccw_btn).on_hover_text("Rotate 90 degrees counter-clockwise").clicked() {
                        self.rotate_ccw();
                    }

                    // Rotate CW
                    let cw_btn = egui::Button::new(
                        egui::RichText::new("⟳ 90° CW").small().strong().color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui.add(cw_btn).on_hover_text("Rotate 90 degrees clockwise").clicked() {
                        self.rotate_cw();
                    }

                    // Rotate 180
                    let r180_btn = egui::Button::new(
                        egui::RichText::new("180°").small().strong().color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui.add(r180_btn).on_hover_text("Rotate 180 degrees").clicked() {
                        self.rotate_180();
                    }

                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Mirror:")
                            .small()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    // Flip H
                    let fh_btn = egui::Button::new(
                        egui::RichText::new("⇆ Flip Horiz").small().strong().color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui.add(fh_btn).on_hover_text("Flip horizontally (mirror left-right)").clicked() {
                        self.flip_horizontal();
                    }

                    // Flip V
                    let fv_btn = egui::Button::new(
                        egui::RichText::new("⇅ Flip Vert").small().strong().color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui.add(fv_btn).on_hover_text("Flip vertically (mirror up-down)").clicked() {
                        self.flip_vertical();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Revert all
                        let rev_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_del)), palette);
                        let revert_btn = egui::Button::new(
                            egui::RichText::new("↺ Revert to Original").small().color(rev_fg),
                        )
                        .fill(btn_bg)
                        .stroke(Stroke::new(1.0, btn_border))
                        .rounding(Rounding::same(4.0));
                        if ui.add(revert_btn).clicked() {
                            self.revert_to_original();
                        }

                        // Undo
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_mod)), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(btn_bg)
                            .stroke(Stroke::new(1.0, btn_border))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    fn render_redact_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("█ Privacy Redaction:")
                            .strong()
                            .small()
                            .color(hex_to_c32(&palette.diff_del)),
                    );

                    let box_count = self.redaction_state.boxes.len();
                    let count_str = if box_count == 0 {
                        "Drag on image to draw black bar".to_string()
                    } else if box_count == 1 {
                        "1 bar placed".to_string()
                    } else {
                        format!("{} bars placed", box_count)
                    };
                    ui.label(
                        egui::RichText::new(count_str)
                            .small()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    ui.add_space(8.0);

                    // Burn / Apply Redactions button
                    let has_boxes = !self.redaction_state.boxes.is_empty();
                    let (burn_bg, burn_stroke, pref_fg) = if has_boxes {
                        (hex_to_c32(&palette.diff_del), hex_to_c32(&palette.diff_del), hex_to_c32(&palette.theme.f_high))
                    } else {
                        (btn_bg, btn_border, hex_to_c32(&palette.theme.f_low))
                    };
                    let burn_fg = contrast_text_c32(burn_bg, Some(pref_fg), palette);
                    let burn_btn = egui::Button::new(
                        egui::RichText::new("🔥 Burn / Apply (Enter)")
                            .small()
                            .strong()
                            .color(burn_fg),
                    )
                    .fill(burn_bg)
                    .stroke(Stroke::new(1.0, burn_stroke))
                    .rounding(Rounding::same(4.0));

                    if ui.add_enabled(has_boxes, burn_btn).on_hover_text("Permanently rasterize black bars into image pixels (Enter)").clicked() {
                        self.apply_redactions();
                    }

                    // Delete Selected button
                    let has_selection = self.redaction_state.selected_idx.is_some();
                    let del_fg = contrast_text_c32(btn_bg, Some(if has_selection { hex_to_c32(&palette.diff_del) } else { hex_to_c32(&palette.theme.f_low) }), palette);
                    let del_btn = egui::Button::new(
                        egui::RichText::new("🗑 Delete Selected (Del)")
                            .small()
                            .strong()
                            .color(del_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, if has_selection { hex_to_c32(&palette.diff_del) } else { btn_border }))
                    .rounding(Rounding::same(4.0));

                    if ui.add_enabled(has_selection, del_btn).clicked() {
                        self.delete_selected_redaction();
                    }

                    // Clear All button
                    let clear_fg = contrast_text_c32(btn_bg, Some(if has_boxes { hex_to_c32(&palette.theme.f_high) } else { hex_to_c32(&palette.theme.f_low) }), palette);
                    let clear_btn = egui::Button::new(
                        egui::RichText::new("✕ Clear All")
                            .small()
                            .color(clear_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));

                    if ui.add_enabled(has_boxes, clear_btn).clicked() {
                        self.clear_all_redactions();
                    }

                    ui.add_space(8.0);

                    // Strip Metadata / EXIF button
                    let has_meta = self.metadata.is_some();
                    let strip_col = if has_meta { hex_to_c32(&palette.diff_mod) } else { hex_to_c32(&palette.theme.f_low) };
                    let strip_fg = contrast_text_c32(btn_bg, Some(strip_col), palette);
                    let strip_btn = egui::Button::new(
                        egui::RichText::new("🛡️ Strip Metadata / EXIF")
                            .small()
                            .strong()
                            .color(strip_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, if has_meta { hex_to_c32(&palette.diff_mod) } else { btn_border }))
                    .rounding(Rounding::same(4.0));

                    if ui.add(strip_btn)
                        .on_hover_text(if has_meta {
                            "Permanently remove camera, GPS, timestamps, and all EXIF tags from this image"
                        } else {
                            "Image is already clean (No EXIF metadata detected)"
                        })
                        .clicked()
                    {
                        self.strip_metadata();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_mod)), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(btn_bg)
                            .stroke(Stroke::new(1.0, btn_border))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    fn render_pick_color_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("💉 Magnifying Color Picker:")
                            .strong()
                            .small()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    let display_col = self.pick_color_state.hovered_color
                        .or(self.pick_color_state.last_picked)
                        .unwrap_or(Color32::WHITE);
                    let hex_code = format!("#{:02X}{:02X}{:02X}", display_col.r(), display_col.g(), display_col.b());

                    // Swatch preview box
                    let (swatch_rect, _) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), egui::Sense::hover());
                    ui.painter().rect_filled(swatch_rect, Rounding::same(3.0), display_col);
                    ui.painter().rect_stroke(swatch_rect, Rounding::same(3.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)));

                    let hex_fg = contrast_text_c32(frame_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                    ui.label(
                        egui::RichText::new(&hex_code)
                            .strong()
                            .monospace()
                            .small()
                            .color(hex_fg),
                    );

                    let rgb_str = format!("RGB({}, {}, {})", display_col.r(), display_col.g(), display_col.b());
                    ui.label(
                        egui::RichText::new(rgb_str)
                            .small()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    let copy_bg = hex_to_c32(&palette.harmonies.base);
                    let copy_fg = contrast_text_c32(copy_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                    let copy_btn = egui::Button::new(
                        egui::RichText::new("📋 Copy Hex")
                            .small()
                            .strong()
                            .color(copy_fg),
                    )
                    .fill(copy_bg)
                    .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
                    .rounding(Rounding::same(4.0));

                    if ui.add(copy_btn).clicked() {
                        self.copy_color_to_clipboard(display_col, ctx);
                    }

                    // Recent palette history
                    if !self.pick_color_state.recent_colors.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Recent:").small().color(hex_to_c32(&palette.theme.f_med)));

                        let recents = self.pick_color_state.recent_colors.clone();
                        for c in recents {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), egui::Sense::click());
                            ui.painter().rect_filled(rect, Rounding::same(2.0), c);
                            ui.painter().rect_stroke(rect, Rounding::same(2.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)));
                            let hex = format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
                            if resp.on_hover_text(format!("Click to copy {}", hex)).clicked() {
                                self.copy_color_to_clipboard(c, ctx);
                            }
                        }
                    }

                    // Integrated Dominant Palette
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.label(
                        egui::RichText::new("🎨 Dominant Palette:")
                            .strong()
                            .small()
                            .color(hex_to_c32(&palette.harmonies.triadic_1)),
                    );

                    // Re-extract button
                    let re_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_med)), palette);
                    let re_btn = egui::Button::new(
                        egui::RichText::new("↺ Refresh").small().color(re_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui.add(re_btn).on_hover_text("Re-extract dominant color palette from image").clicked() {
                        self.run_palette_extraction();
                    }

                    if !self.palette_state.extracted_colors.is_empty() {
                        let csv_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_med)), palette);
                        let csv_btn = egui::Button::new(
                            egui::RichText::new("📋 CSV").small().color(csv_fg),
                        )
                        .fill(btn_bg)
                        .stroke(Stroke::new(1.0, btn_border))
                        .rounding(Rounding::same(4.0));
                        if ui.add(csv_btn).on_hover_text("Copy all hex codes as CSV").clicked() {
                            let csv = self.palette_state.extracted_colors.iter().map(|c| c.hex.as_str()).collect::<Vec<_>>().join(", ");
                            ctx.output_mut(|o| o.copied_text = csv.clone());
                            let csv_for_cmd = csv.clone();
                            std::thread::spawn(move || {
                                let _ = Command::new("wl-copy").arg(&csv_for_cmd).output();
                                let _ = Command::new("xclip").arg("-selection").arg("clipboard").arg(&csv_for_cmd).output();
                            });
                            self.show_toast("📋 Copied all palette hex codes to clipboard");
                        }

                        // Swatches
                        for pal in self.palette_state.extracted_colors.clone() {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), egui::Sense::click());
                            ui.painter().rect_filled(rect, Rounding::same(2.0), pal.color);
                            ui.painter().rect_stroke(rect, Rounding::same(2.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)));

                            let tip = format!("{} ({:.1}%) - Click to copy", pal.hex, pal.percentage);
                            if resp.on_hover_text(tip).clicked() {
                                self.copy_color_to_clipboard(pal.color, ctx);
                            }

                            ui.label(
                                egui::RichText::new(format!("{} ({:.0}%)", pal.hex, pal.percentage))
                                    .small()
                                    .monospace()
                                    .color(hex_to_c32(&palette.theme.f_med)),
                            );
                            ui.add_space(2.0);
                        }
                    } else {
                        ui.label(
                            egui::RichText::new("(No palette extracted)")
                                .small()
                                .italics()
                                .color(hex_to_c32(&palette.theme.f_low)),
                        );
                    }
                });
            });
    }

    fn render_resize_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        self.poll_optimizer();
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }

        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    let is_pixels = self.resize_state.sub_tab == ResizeSubTab::Pixels;
                    let is_filesize = self.resize_state.sub_tab == ResizeSubTab::FileSize;

                    // Row 1: Primary Controls
                    ui.horizontal(|ui| {
                        // Sub-tab toggles
                        let px_bg = if is_pixels { hex_to_c32(&palette.harmonies.base) } else { btn_bg };
                        let px_border = if is_pixels { hex_to_c32(&palette.harmonies.base) } else { btn_border };
                        let px_pref = if is_pixels { hex_to_c32(&palette.theme.f_high) } else { hex_to_c32(&palette.theme.f_med) };
                        let px_fg = contrast_text_c32(px_bg, Some(px_pref), palette);
                        let px_btn = egui::Button::new(
                            egui::RichText::new("↔ Pixel Size")
                                .small()
                                .strong()
                                .color(px_fg),
                        )
                        .fill(px_bg)
                        .stroke(Stroke::new(1.0, px_border))
                        .rounding(Rounding::same(4.0));

                        if ui.add(px_btn).clicked() {
                            self.resize_state.sub_tab = ResizeSubTab::Pixels;
                            self.resize_state.sync_from_image(&self.current_rgba);
                        }

                        let fs_bg = if is_filesize { hex_to_c32(&palette.diff_add) } else { btn_bg };
                        let fs_border = if is_filesize { hex_to_c32(&palette.diff_add) } else { btn_border };
                        let fs_pref = if is_filesize { hex_to_c32(&palette.theme.f_high) } else { hex_to_c32(&palette.theme.f_med) };
                        let fs_fg = contrast_text_c32(fs_bg, Some(fs_pref), palette);
                        let fs_btn = egui::Button::new(
                            egui::RichText::new("🎯 Target File Size Optimizer")
                                .small()
                                .strong()
                                .color(fs_fg),
                        )
                        .fill(fs_bg)
                        .stroke(Stroke::new(1.0, fs_border))
                        .rounding(Rounding::same(4.0));

                        if ui.add(fs_btn).clicked() {
                            self.resize_state.sub_tab = ResizeSubTab::FileSize;
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        if is_pixels {
                            // 1. Pixel Resize UI
                            ui.label(egui::RichText::new("W:").small().strong().color(hex_to_c32(&palette.theme.f_med)));

                            let mut w_str = self.resize_state.width_input.clone();
                            let w_resp = ui.add(
                                egui::TextEdit::singleline(&mut w_str)
                                    .desired_width(55.0)
                                    .font(egui::TextStyle::Monospace)
                            );
                            if w_resp.changed() {
                                self.resize_state.width_input = w_str.clone();
                                if let Ok(w_val) = w_str.trim().parse::<u32>() {
                                    self.resize_state.set_width(w_val);
                                }
                            }

                            // Aspect Lock Toggle Button
                            let lock_text = if self.resize_state.lock_aspect { "🔒" } else { "🔓" };
                            let lock_btn = egui::Button::new(egui::RichText::new(lock_text).small())
                                .fill(if self.resize_state.lock_aspect { btn_bg } else { frame_bg })
                                .stroke(Stroke::new(1.0, btn_border))
                                .rounding(Rounding::same(4.0));

                            if ui.add(lock_btn).on_hover_text("Lock aspect ratio").clicked() {
                                self.resize_state.lock_aspect = !self.resize_state.lock_aspect;
                                if self.resize_state.lock_aspect && self.current_rgba.height() > 0 {
                                    self.resize_state.aspect_ratio_base = self.current_rgba.width() as f32 / self.current_rgba.height() as f32;
                                }
                            }

                            ui.label(egui::RichText::new("H:").small().strong().color(hex_to_c32(&palette.theme.f_med)));

                            let mut h_str = self.resize_state.height_input.clone();
                            let h_resp = ui.add(
                                egui::TextEdit::singleline(&mut h_str)
                                    .desired_width(55.0)
                                    .font(egui::TextStyle::Monospace)
                            );
                            if h_resp.changed() {
                                self.resize_state.height_input = h_str.clone();
                                if let Ok(h_val) = h_str.trim().parse::<u32>() {
                                    self.resize_state.set_height(h_val);
                                }
                            }

                            ui.add_space(4.0);

                            // Scale Presets
                            let (cur_w, cur_h) = (self.current_rgba.width(), self.current_rgba.height());
                            let presets = [("25%", 0.25f32), ("50%", 0.5), ("75%", 0.75), ("100%", 1.0), ("150%", 1.5), ("200%", 2.0)];
                            for (label, factor) in presets {
                                let p_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_med)), palette);
                                let p_btn = egui::Button::new(egui::RichText::new(label).small().color(p_fg))
                                    .fill(btn_bg)
                                    .stroke(Stroke::new(1.0, btn_border))
                                    .rounding(Rounding::same(3.0));
                                if ui.add(p_btn).clicked() {
                                    self.resize_state.set_scale_preset(factor, cur_w, cur_h);
                                }
                            }

                            ui.add_space(4.0);

                            // Filter Combo
                            egui::ComboBox::from_id_source("resize_filter_combo")
                                .selected_text(self.resize_state.filter.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.resize_state.filter, ResizeFilter::Lanczos3, ResizeFilter::Lanczos3.label());
                                    ui.selectable_value(&mut self.resize_state.filter, ResizeFilter::Triangle, ResizeFilter::Triangle.label());
                                    ui.selectable_value(&mut self.resize_state.filter, ResizeFilter::Nearest, ResizeFilter::Nearest.label());
                                });

                            ui.add_space(4.0);

                            // Apply Button
                            let apply_bg = hex_to_c32(&palette.harmonies.base);
                            let apply_fg = contrast_text_c32(apply_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                            let apply_btn = egui::Button::new(
                                egui::RichText::new("↔ Apply (Enter)")
                                    .small()
                                    .strong()
                                    .color(apply_fg),
                            )
                            .fill(apply_bg)
                            .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
                            .rounding(Rounding::same(4.0));

                            if ui.add(apply_btn).on_hover_text("Apply pixel dimensions to image (Enter)").clicked() {
                                self.apply_pixel_resize(
                                    self.resize_state.target_width,
                                    self.resize_state.target_height,
                                    self.resize_state.filter,
                                );
                            }

                        } else {
                            // 2. Target File Size Optimizer UI
                            let presets = [
                                ("250 KB", 250.0f32, false),
                                ("500 KB", 500.0, false),
                                ("1 MB", 1.0, true),
                                ("2 MB", 2.0, true),
                            ];

                            for (lbl, val, is_mb) in presets {
                                let is_sel = (self.resize_state.target_size_val - val).abs() < 0.01 && self.resize_state.target_size_unit_mb == is_mb;
                                let p_bg = if is_sel { hex_to_c32(&palette.diff_add) } else { btn_bg };
                                let p_pref = if is_sel { hex_to_c32(&palette.theme.f_high) } else { hex_to_c32(&palette.theme.f_med) };
                                let p_fg = contrast_text_c32(p_bg, Some(p_pref), palette);
                                let p_btn = egui::Button::new(
                                    egui::RichText::new(lbl)
                                        .small()
                                        .strong()
                                        .color(p_fg),
                                )
                                .fill(p_bg)
                                .stroke(Stroke::new(1.0, if is_sel { hex_to_c32(&palette.diff_add) } else { btn_border }))
                                .rounding(Rounding::same(3.0));

                                if ui.add(p_btn).clicked() {
                                    self.resize_state.target_size_val = val;
                                    self.resize_state.target_size_unit_mb = is_mb;
                                }
                            }

                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Target:").small().color(hex_to_c32(&palette.theme.f_med)));

                            ui.add(
                                egui::DragValue::new(&mut self.resize_state.target_size_val)
                                    .speed(0.1)
                                    .clamp_range(0.01..=1000.0)
                            );

                            let unit_str = if self.resize_state.target_size_unit_mb { "MB" } else { "KB" };
                            let unit_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.harmonies.base)), palette);
                            let unit_btn = egui::Button::new(egui::RichText::new(unit_str).small().strong().color(unit_fg))
                                .fill(btn_bg)
                                .stroke(Stroke::new(1.0, btn_border))
                                .rounding(Rounding::same(3.0));
                            if ui.add(unit_btn).on_hover_text("Toggle between MB and KB").clicked() {
                                self.resize_state.target_size_unit_mb = !self.resize_state.target_size_unit_mb;
                            }

                            ui.add_space(4.0);

                            // Format selector
                            egui::ComboBox::from_id_source("opt_format_combo")
                                .selected_text(self.resize_state.target_format.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.resize_state.target_format, TargetFormat::Jpeg, "JPEG (Quality+Res)");
                                    ui.selectable_value(&mut self.resize_state.target_format, TargetFormat::Png, "PNG (Lossless Res)");
                                });

                            ui.add_space(4.0);

                            // Run Optimizer Button
                            if self.resize_state.is_optimizing {
                                let run_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_low)), palette);
                                let run_btn = egui::Button::new(
                                    egui::RichText::new("⏳ Optimizing...")
                                        .small()
                                        .strong()
                                        .color(run_fg),
                                )
                                .fill(btn_bg)
                                .stroke(Stroke::new(1.0, btn_border))
                                .rounding(Rounding::same(4.0));
                                ui.add_enabled(false, run_btn);
                            } else {
                                let run_bg = hex_to_c32(&palette.diff_add);
                                let run_fg = contrast_text_c32(run_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                                let run_btn = egui::Button::new(
                                    egui::RichText::new("⚡ Run Optimizer")
                                        .small()
                                        .strong()
                                        .color(run_fg),
                                )
                                .fill(run_bg)
                                .stroke(Stroke::new(1.0, hex_to_c32(&palette.diff_add)))
                                .rounding(Rounding::same(4.0));

                                if ui.add(run_btn).on_hover_text("Find optimal quality and resolution trade-off to fit target size").clicked() {
                                    self.run_target_file_size_optimizer();
                                }
                            }
                        }

                        // Right aligned undo/revert
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !self.undo_stack.is_empty() {
                                let undo_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_mod)), palette);
                                let undo_btn = egui::Button::new(
                                    egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                                )
                                .fill(btn_bg)
                                .stroke(Stroke::new(1.0, btn_border))
                                .rounding(Rounding::same(4.0));
                                if ui.add(undo_btn).clicked() {
                                    self.undo();
                                }
                            }
                        });
                    });

                    // Row 2 underneath: When in FileSize mode, show live progress or result badges
                    if is_filesize {
                        if self.resize_state.is_optimizing {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                ui.add(egui::Spinner::new().size(14.0).color(hex_to_c32(&palette.harmonies.base)));
                                ui.add_space(6.0);
                                let st = self.optimizer_status.as_deref().unwrap_or("Optimizing image file size...");
                                ui.label(
                                    egui::RichText::new(st)
                                        .small()
                                        .monospace()
                                        .strong()
                                        .color(hex_to_c32(&palette.harmonies.base)),
                                );
                            });
                        } else if let Some(res) = &self.resize_state.last_result {
                            ui.add_space(4.0);
                            let mut apply_clicked = false;
                            let mut copy_clicked = false;

                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                let stat_text = format!(
                                    "✔ Optimal Result: {} / {} (Scale: {:.0}%, Res: {}×{}, Q: {}%)",
                                    format_bytes_size(res.achieved_bytes),
                                    format_bytes_size(res.target_bytes),
                                    res.scale * 100.0,
                                    res.width,
                                    res.height,
                                    res.quality
                                );
                                ui.label(
                                    egui::RichText::new(stat_text)
                                        .small()
                                        .strong()
                                        .color(hex_to_c32(&palette.diff_add)),
                                );

                                ui.add_space(8.0);
                                let apply_bg = hex_to_c32(&palette.diff_add);
                                let apply_fg = contrast_text_c32(apply_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                                let apply_opt_btn = egui::Button::new(
                                    egui::RichText::new("✔ Apply to Canvas").small().strong().color(apply_fg),
                                )
                                .fill(apply_bg)
                                .stroke(Stroke::new(1.0, hex_to_c32(&palette.diff_add)))
                                .rounding(Rounding::same(4.0));
                                if ui.add(apply_opt_btn).on_hover_text("Apply optimized image to canvas").clicked() {
                                    apply_clicked = true;
                                }

                                let copy_bg = hex_to_c32(&palette.harmonies.triadic_1);
                                let copy_fg = contrast_text_c32(copy_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                                let copy_opt_btn = egui::Button::new(
                                    egui::RichText::new("📋 Copy to Slot").small().strong().color(copy_fg),
                                )
                                .fill(copy_bg)
                                .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
                                .rounding(Rounding::same(4.0));
                                if ui.add(copy_opt_btn).on_hover_text("Push optimized image buffer directly to Left Hand slot and clipboard").clicked() {
                                    copy_clicked = true;
                                }
                            });

                            if apply_clicked {
                                self.apply_optimized_result();
                            }
                            if copy_clicked {
                                self.copy_optimized_to_slot();
                            }
                        }
                    }
                });
            });
    }

    fn render_remove_bg_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
        }

        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let triadic_1 = hex_to_c32(&palette.harmonies.triadic_1);
        let triadic_2 = hex_to_c32(&palette.harmonies.triadic_2);
        let diff_del = hex_to_c32(&palette.diff_del);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_low = hex_to_c32(&palette.theme.f_low);
        let f_high = hex_to_c32(&palette.theme.f_high);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.bg_removal_in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(triadic_1));
                        ui.add_space(6.0);
                        let st = self.bg_removal_status.as_deref().unwrap_or("Segmenting background with FeyNoBg...");
                        ui.label(
                            egui::RichText::new(st)
                                .small()
                                .monospace()
                                .strong()
                                .color(triadic_1),
                        );
                    } else {
                        let has_prompt = !self.multimatte_prompt.trim().is_empty();
                        let btn_text = if has_prompt {
                            format!("✨ MultiMatte ('{}')", self.multimatte_prompt.trim())
                        } else {
                            "✨ Cut Out BG (Enter)".to_string()
                        };

                        let cut_bg = if has_prompt { triadic_2 } else { triadic_1 };
                        let cut_fg = contrast_text_c32(cut_bg, None, palette);
                        let cut_btn = egui::Button::new(
                            egui::RichText::new(btn_text)
                                .small()
                                .strong()
                                .color(cut_fg),
                        )
                        .fill(cut_bg)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));

                        let tooltip = if has_prompt {
                            "Execute prompt-guided concept segmentation with feyninc/multimatte (Enter)"
                        } else {
                            "Execute AI background removal with feyninc/FeyNobg model (Enter)"
                        };

                        if ui.add(cut_btn).on_hover_text(tooltip).clicked() {
                            self.run_background_removal();
                        }

                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.multimatte_prompt)
                                .hint_text("MultiMatte prompt (e.g. 'cat', 'person')...")
                                .desired_width(180.0),
                        );

                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("feyninc/FeyNoBg / MultiMatte • CUDA")
                                .small()
                                .color(f_low),
                        );
                    }

                    if let Some(err) = &self.bg_removal_error {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .small()
                                .strong()
                                .color(diff_del),
                        );
                        if err.contains("nobg") || err.contains("pip") {
                            let copy_fg = contrast_text_c32(b_med, Some(f_high), palette);
                            let copy_install_btn = egui::Button::new(
                                egui::RichText::new("📋 Copy Install Cmd").small().strong().color(copy_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0, base_c))
                            .rounding(Rounding::same(3.0));
                            if ui.add(copy_install_btn).clicked() {
                                let install_cmd = "uv venv --python /usr/bin/python3 --system-site-packages ~/.local/share/little-screen-clip/nobg-venv && uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python nobg";
                                ctx.output_mut(|o| o.copied_text = install_cmd.to_string());
                                self.show_toast("📋 Copied setup command to clipboard!");
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(b_med, Some(diff_mod), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(b_med)
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }

                        // Direct Slot Push buttons
                        let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Right Hand").small().color(rh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, diff_add))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_rh).on_hover_text("Push current cutout directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let lh_fg = contrast_text_c32(b_low, Some(base_c), palette);
                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Left Hand").small().color(lh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, base_c))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_lh).on_hover_text("Push current cutout directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });
                });
            });
    }

    fn render_depth_map_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
        }

        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let f_low = hex_to_c32(&palette.theme.f_low);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.bg_removal_in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(base_c));
                        ui.add_space(6.0);
                        let st = self.bg_removal_status.as_deref().unwrap_or("Estimating depth map with Depth-Anything...");
                        ui.label(
                            egui::RichText::new(st)
                                .small()
                                .monospace()
                                .strong()
                                .color(base_c),
                        );
                    } else {
                        let depth_fg = contrast_text_c32(base_c, None, palette);
                        let depth_btn = egui::Button::new(
                            egui::RichText::new("🌊 Generate Depth Map (Enter)")
                                .small()
                                .strong()
                                .color(depth_fg),
                        )
                        .fill(base_c)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));

                        if ui.add(depth_btn).on_hover_text("Generate AI metric depth map with Depth-Anything V2 neural network (Enter)").clicked() {
                            self.run_depth_estimation();
                        }

                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Depth-Anything V2 • Metric Monocular Depth Estimation • CUDA")
                                .small()
                                .color(f_low),
                        );
                    }

                    if let Some(err) = &self.bg_removal_error {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .small()
                                .strong()
                                .color(diff_del),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(b_med, Some(diff_mod), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(b_med)
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }

                        // Direct Slot Push buttons
                        let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Right Hand").small().color(rh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, diff_add))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_rh).on_hover_text("Push current depth map directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let lh_fg = contrast_text_c32(b_low, Some(base_c), palette);
                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Left Hand").small().color(lh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, base_c))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_lh).on_hover_text("Push current depth map directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });
                });
            });
    }

    // --- Format Conversion (Squoosh-style) ---

    pub fn update_conversion(&mut self) {
        let base_img = self.current_display_image();
        let fmt = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;

        let encode_res = match fmt {
            ConvertTargetFormat::Png => encode_png(&base_img).map_err(|e| e.to_string()),
            ConvertTargetFormat::Jpeg => encode_jpeg_with_quality(&base_img, quality).map_err(|e| e.to_string()),
            ConvertTargetFormat::Webp => encode_webp(&base_img, lossless, quality),
        };

        match encode_res {
            Ok(bytes) => {
                let decoded_rgba = match image::load_from_memory(&bytes) {
                    Ok(dyn_img) => Some(dyn_img.to_rgba8()),
                    Err(_) => Some(base_img),
                };
                self.convert_state.encoded_bytes = bytes;
                self.convert_state.converted_rgba = decoded_rgba;
                self.convert_state.dirty_texture = true;
                self.convert_state.optipng_savings = None;
            }
            Err(e) => {
                self.show_toast(format!("Conversion failed: {}", e));
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
            self.update_conversion();
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

    fn render_convert_toolbar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, palette: &AppPalette) {
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.theme.f_high);
        let f_med = hex_to_c32(&palette.theme.f_med);
        let f_low = hex_to_c32(&palette.theme.f_low);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let lbl_fg = contrast_text_c32(b_low, Some(f_med), palette);
                    ui.label(
                        egui::RichText::new("Format:")
                            .small()
                            .strong()
                            .color(lbl_fg),
                    );

                    let fmts = [
                        ConvertTargetFormat::Png,
                        ConvertTargetFormat::Jpeg,
                        ConvertTargetFormat::Webp,
                    ];

                    let mut changed = false;
                    for fmt in fmts {
                        let label = fmt.label();
                        let active = self.convert_state.target_format == fmt;
                        let (bg, border, text_col) = if active {
                            (base_c, b_med, contrast_text_c32(base_c, None, palette))
                        } else {
                            (b_med, b_low, contrast_text_c32(b_med, Some(f_med), palette))
                        };

                        let btn = egui::Button::new(
                            egui::RichText::new(label).small().strong().color(text_col),
                        )
                        .fill(bg)
                        .stroke(Stroke::new(1.0, border))
                        .rounding(Rounding::same(4.0));

                        if ui.add(btn).clicked() && self.convert_state.target_format != fmt {
                            self.convert_state.target_format = fmt;
                            if fmt == ConvertTargetFormat::Png {
                                self.convert_state.lossless = true;
                            } else if fmt == ConvertTargetFormat::Jpeg {
                                self.convert_state.lossless = false;
                            }
                            changed = true;
                        }
                    }

                    ui.add_space(8.0);

                    // Lossless toggle
                    match self.convert_state.target_format {
                        ConvertTargetFormat::Png => {
                            let mut dummy_true = true;
                            ui.add_enabled_ui(false, |ui| {
                                ui.checkbox(&mut dummy_true, egui::RichText::new("Lossless").small().color(f_med))
                                    .on_disabled_hover_text("PNG is always a lossless format");
                            });
                        }
                        ConvertTargetFormat::Jpeg => {
                            let mut dummy_false = false;
                            ui.add_enabled_ui(false, |ui| {
                                ui.checkbox(&mut dummy_false, egui::RichText::new("Lossless").small().color(f_low))
                                    .on_disabled_hover_text("JPEG standard does not support lossless encoding");
                            });
                        }
                        ConvertTargetFormat::Webp => {
                            let mut is_lossless = self.convert_state.lossless;
                            let chk_fg = contrast_text_c32(b_low, Some(f_high), palette);
                            if ui.checkbox(&mut is_lossless, egui::RichText::new("Lossless").small().strong().color(chk_fg))
                                .on_hover_text("Encode as lossless WebP (VP8L)")
                                .changed()
                            {
                                self.convert_state.lossless = is_lossless;
                                changed = true;
                            }
                        }
                    }

                    // Quality slider (if not lossless)
                    if !self.convert_state.lossless {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Quality:").small().color(f_med),
                        );
                        let mut q = self.convert_state.quality;
                        let slider = egui::Slider::new(&mut q, 1..=100)
                            .show_value(true)
                            .suffix("%");
                        let resp = ui.add(slider);
                        if resp.changed() {
                            self.convert_state.quality = q;
                            changed = true;
                        }
                    }

                    // OptiPNG button for PNG
                    if self.convert_state.target_format == ConvertTargetFormat::Png {
                        ui.add_space(8.0);
                        let opti_fg = contrast_text_c32(diff_mod, None, palette);
                        let opti_btn = egui::Button::new(
                            egui::RichText::new("⚡ Run OptiPNG").small().strong().color(opti_fg),
                        )
                        .fill(diff_mod)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));

                        if ui.add(opti_btn).on_hover_text("Run lossless bytecode optimization via /usr/bin/optipng").clicked() {
                            self.run_optipng_conversion();
                        }

                        if let Some((orig, opti)) = self.convert_state.optipng_savings {
                            let diff = orig.saturating_sub(opti);
                            let pct = if orig > 0 { (diff as f64 / orig as f64) * 100.0 } else { 0.0 };
                            let label_txt = format!("Saved {} (-{:.1}%)", format_bytes_size(diff), pct);
                            chip_badge(ui, &label_txt, diff_add, b_low);
                        }
                    }

                    // Size comparison badge
                    ui.add_space(8.0);
                    let orig_size = self.file_size_bytes;
                    let conv_size = self.convert_state.encoded_bytes.len();
                    if conv_size > 0 {
                        let pct_delta = if orig_size > 0 {
                            ((conv_size as f64 - orig_size as f64) / orig_size as f64) * 100.0
                        } else {
                            0.0
                        };
                        let (sign, fg) = if pct_delta <= 0.0 {
                            ("-", diff_add)
                        } else {
                            ("+", diff_mod)
                        };
                        let badge_txt = format!(
                            "{} ➔ {} ({}{:.1}%)",
                            format_bytes_size(orig_size),
                            format_bytes_size(conv_size),
                            sign,
                            pct_delta.abs()
                        );
                        chip_badge(ui, &badge_txt, fg, b_low);
                    }

                    // Squoosh split slider hint
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Drag vertical divider on canvas to compare")
                            .small()
                            .italics()
                            .color(f_low),
                    );

                    // Right-aligned actions
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Apply button
                        let apply_fg = contrast_text_c32(diff_add, None, palette);
                        let apply_btn = egui::Button::new(
                            egui::RichText::new("✔ Apply (Enter)").small().strong().color(apply_fg),
                        )
                        .fill(diff_add)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));
                        if ui.add(apply_btn).on_hover_text("Apply converted image as active editor canvas (Enter)").clicked() {
                            self.apply_converted_to_canvas();
                        }

                        // Save As button
                        let ext = self.convert_state.target_format.extension();
                        let save_fg = contrast_text_c32(b_med, Some(f_high), palette);
                        let save_btn = egui::Button::new(
                            egui::RichText::new(format!("💾 Save as .{}", ext)).small().strong().color(save_fg),
                        )
                        .fill(b_med)
                        .stroke(Stroke::new(1.0, base_c))
                        .rounding(Rounding::same(4.0));
                        if ui.add(save_btn).on_hover_text(format!("Export converted image as .{}", ext)).clicked() {
                            self.open_save_dialog_with_ext(ext);
                        }

                        // Copy slot buttons
                        let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: RH").small().color(rh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, diff_add))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_rh).on_hover_text("Push converted image directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let lh_fg = contrast_text_c32(b_low, Some(base_c), palette);
                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: LH").small().color(lh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0, base_c))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_lh).on_hover_text("Push converted image directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });

                    if changed {
                        self.update_conversion();
                    }
                });
            });
    }

    fn render_convert_comparison(
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
        let conv_tex_id = self.convert_state.converted_texture.as_ref().map(|t| t.id())
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
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(3.0, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(1.5, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_radius = 16.0;
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let knob_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
        painter.circle(
            knob_center,
            knob_radius,
            b_low,
            Stroke::new(2.0, base_c),
        );
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

    // --- Decorrelation Stretch (DCS) Tool Suite ---

    pub fn update_decorrelation_stretch(&mut self) {
        let base_img = self.current_display_image();
        let t0 = std::time::Instant::now();
        let (stats, transform, stretched) = run_decorrelation_stretch(&base_img, &self.dcs_state.params);
        let duration_ms = t0.elapsed().as_millis();

        self.dcs_state.stats = Some(stats);
        self.dcs_state.transform = Some(transform);
        self.dcs_state.stretched_rgba = Some(stretched);
        self.dcs_state.dirty_texture = true;
        self.dcs_state.last_duration_ms = duration_ms;
    }

    pub fn apply_decorrelation_stretch(&mut self) {
        if let Some(stretched) = self.dcs_state.stretched_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = stretched;
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.update_decorrelation_stretch();
            self.show_toast("✔ Applied decorrelation stretch to canvas");
        }
    }

    fn render_decorrelation_stretch_toolbar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, palette: &AppPalette) {
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_med = hex_to_c32(&palette.theme.f_med);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);

        let mut changed = false;

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🌈 Decorrelation Stretch:")
                            .strong()
                            .color(base_c),
                    );

                    ui.add_space(4.0);

                    // Preset buttons
                    let nasa_btn = crate::secondary_button("NASA/ASTER", b_med, palette);
                    if ui.add(nasa_btn).on_hover_text("Preset: Correlation matrix, target mean 127.5, sigma 50.0, tol 0").clicked() {
                        self.dcs_state.params = DecorrelationStretchParams::nasa();
                        self.dcs_state.custom_mean = 127.5;
                        self.dcs_state.custom_sigma = 50.0;
                        changed = true;
                    }

                    let matlab_btn = crate::secondary_button("MATLAB", b_med, palette);
                    if ui.add(matlab_btn).on_hover_text("Preset: Correlation matrix, preserve band mean and sigma").clicked() {
                        self.dcs_state.params = DecorrelationStretchParams::matlab_preserve();
                        changed = true;
                    }

                    ui.add_space(6.0);

                    // Matrix Mode
                    let is_corr = self.dcs_state.params.mode == DecorrelationMatrixMode::Correlation;
                    let (corr_bg, corr_border, corr_fg) = if is_corr {
                        (base_c, b_med, contrast_text_c32(base_c, None, palette))
                    } else {
                        (b_med, b_low, contrast_text_c32(b_med, Some(f_med), palette))
                    };
                    let corr_btn = egui::Button::new(egui::RichText::new("Correlation").small().strong().color(corr_fg))
                        .fill(corr_bg)
                        .stroke(Stroke::new(1.0, corr_border))
                        .rounding(Rounding::same(4.0));
                    if ui.add(corr_btn).on_hover_text("Correlation matrix (equal band weight - standard for decorrelation stretching)").clicked() && !is_corr {
                        self.dcs_state.params.mode = DecorrelationMatrixMode::Correlation;
                        changed = true;
                    }

                    let is_cov = self.dcs_state.params.mode == DecorrelationMatrixMode::Covariance;
                    let (cov_bg, cov_border, cov_fg) = if is_cov {
                        (base_c, b_med, contrast_text_c32(base_c, None, palette))
                    } else {
                        (b_med, b_low, contrast_text_c32(b_med, Some(f_med), palette))
                    };
                    let cov_btn = egui::Button::new(egui::RichText::new("Covariance").small().strong().color(cov_fg))
                        .fill(cov_bg)
                        .stroke(Stroke::new(1.0, cov_border))
                        .rounding(Rounding::same(4.0));
                    if ui.add(cov_btn).on_hover_text("Covariance matrix (weights bands by original variance)").clicked() && !is_cov {
                        self.dcs_state.params.mode = DecorrelationMatrixMode::Covariance;
                        changed = true;
                    }

                    ui.add_space(6.0);

                    // Target Mean
                    ui.label(egui::RichText::new("Mean:").small().color(f_med));
                    let is_mean_preserve = matches!(self.dcs_state.params.target_mean, TargetValue::Preserve);
                    let (mean_p_bg, mean_p_border, mean_p_fg) = if is_mean_preserve {
                        (base_c, b_med, contrast_text_c32(base_c, None, palette))
                    } else {
                        (b_med, b_low, contrast_text_c32(b_med, Some(f_med), palette))
                    };
                    let mean_p_btn = egui::Button::new(egui::RichText::new("Preserve").small().strong().color(mean_p_fg))
                        .fill(mean_p_bg)
                        .stroke(Stroke::new(1.0, mean_p_border))
                        .rounding(Rounding::same(4.0));
                    if ui.add(mean_p_btn).on_hover_text("Preserve each band's input mean").clicked() {
                        if is_mean_preserve {
                            self.dcs_state.params.target_mean = TargetValue::Custom(self.dcs_state.custom_mean);
                        } else {
                            self.dcs_state.params.target_mean = TargetValue::Preserve;
                        }
                        changed = true;
                    }
                    if !is_mean_preserve {
                        let mut m_val = self.dcs_state.custom_mean;
                        let drag = egui::DragValue::new(&mut m_val).speed(0.5).clamp_range(0.0..=255.0);
                        if ui.add(drag).on_hover_text("Target output mean (0-255)").changed() {
                            self.dcs_state.custom_mean = m_val;
                            self.dcs_state.params.target_mean = TargetValue::Custom(m_val);
                            changed = true;
                        }
                    }

                    ui.add_space(6.0);

                    // Target Sigma
                    ui.label(egui::RichText::new("Sigma:").small().color(f_med));
                    let is_sig_preserve = matches!(self.dcs_state.params.target_sigma, TargetValue::Preserve);
                    let (sig_p_bg, sig_p_border, sig_p_fg) = if is_sig_preserve {
                        (base_c, b_med, contrast_text_c32(base_c, None, palette))
                    } else {
                        (b_med, b_low, contrast_text_c32(b_med, Some(f_med), palette))
                    };
                    let sig_p_btn = egui::Button::new(egui::RichText::new("Preserve").small().strong().color(sig_p_fg))
                        .fill(sig_p_bg)
                        .stroke(Stroke::new(1.0, sig_p_border))
                        .rounding(Rounding::same(4.0));
                    if ui.add(sig_p_btn).on_hover_text("Preserve each band's input standard deviation").clicked() {
                        if is_sig_preserve {
                            self.dcs_state.params.target_sigma = TargetValue::Custom(self.dcs_state.custom_sigma);
                        } else {
                            self.dcs_state.params.target_sigma = TargetValue::Preserve;
                        }
                        changed = true;
                    }
                    if !is_sig_preserve {
                        let mut s_val = self.dcs_state.custom_sigma;
                        let drag = egui::DragValue::new(&mut s_val).speed(0.5).clamp_range(1.0..=200.0);
                        if ui.add(drag).on_hover_text("Target standard deviation (1-200, NASA default: 50)").changed() {
                            self.dcs_state.custom_sigma = s_val;
                            self.dcs_state.params.target_sigma = TargetValue::Custom(s_val);
                            changed = true;
                        }
                    }

                    ui.add_space(6.0);

                    // Tol (contrast clip)
                    ui.label(egui::RichText::new("Tol:").small().color(f_med));
                    let current_tol = self.dcs_state.params.tol;
                    let tol_label = if current_tol <= 0.0 { "0 (off)".to_string() } else { format!("{}", current_tol) };
                    egui::ComboBox::from_id_source("dcs_tol_combo")
                        .selected_text(tol_label)
                        .width(64.0)
                        .show_ui(ui, |ui| {
                            for (t_val, t_name) in [(0.0, "0 (off)"), (0.005, "0.005"), (0.01, "0.01"), (0.02, "0.02")] {
                                if ui.selectable_label((current_tol - t_val).abs() < 1e-4, t_name).clicked() {
                                    self.dcs_state.params.tol = t_val;
                                    changed = true;
                                }
                            }
                        });

                    ui.add_space(6.0);

                    // Stride
                    ui.label(egui::RichText::new("Stride:").small().color(f_med));
                    let current_stride = self.dcs_state.params.stride;
                    let stride_label = match current_stride {
                        0 => "auto".to_string(),
                        1 => "1 (all)".to_string(),
                        s => format!("{}", s),
                    };
                    egui::ComboBox::from_id_source("dcs_stride_combo")
                        .selected_text(stride_label)
                        .width(64.0)
                        .show_ui(ui, |ui| {
                            for (s_val, s_name) in [(0, "auto"), (1, "1 (all)"), (2, "2"), (3, "3"), (5, "5")] {
                                if ui.selectable_label(current_stride == s_val, s_name).clicked() {
                                    self.dcs_state.params.stride = s_val;
                                    changed = true;
                                }
                            }
                        });

                    // Hold to compare button
                    ui.add_space(6.0);
                    let hold_btn = egui::Button::new(
                        egui::RichText::new("👁 Hold to Compare")
                            .small()
                            .strong()
                            .color(if self.dcs_state.is_holding_original { base_c } else { f_med }),
                    )
                    .fill(if self.dcs_state.is_holding_original { b_med } else { b_low })
                    .stroke(Stroke::new(1.0, b_med))
                    .rounding(Rounding::same(4.0));
                    let hold_resp = ui.add(hold_btn).on_hover_text("Click and hold to momentarily view the original image");
                    self.dcs_state.is_holding_original = hold_resp.is_pointer_button_down_on();

                    // Stats toggle
                    let stats_active = self.dcs_state.show_stats;
                    let (stats_bg, stats_fg) = if stats_active {
                        (base_c, contrast_text_c32(base_c, None, palette))
                    } else {
                        (b_med, contrast_text_c32(b_med, Some(f_med), palette))
                    };
                    let stats_btn = egui::Button::new(egui::RichText::new("📊 Stats").small().strong().color(stats_fg))
                        .fill(stats_bg)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));
                    if ui.add(stats_btn).on_hover_text("Toggle detailed statistics report (means, covariance, correlation, eigenvalues, eigenvectors, transform matrix)").clicked() {
                        self.dcs_state.show_stats = !self.dcs_state.show_stats;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Apply button
                        let apply_btn = crate::action_button("✓ Apply (Enter)", diff_add, palette);
                        if ui.add(apply_btn).on_hover_text("Commit decorrelation stretch result to image (Enter)").clicked() {
                            self.apply_decorrelation_stretch();
                        }

                        // Recompute button
                        let recompute_btn = crate::secondary_button("⟲ Recompute", b_med, palette);
                        if ui.add(recompute_btn).on_hover_text("Recalculate decorrelation stretch statistics and transform").clicked() {
                            self.update_decorrelation_stretch();
                        }

                        // Undo if available
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(b_med, Some(diff_mod), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(b_med)
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                                self.update_decorrelation_stretch();
                            }
                        }
                    });
                });
            });

        // Recompute if parameters changed
        if changed {
            self.update_decorrelation_stretch();
        }

        // Stats popup / collapsible drawer
        if self.dcs_state.show_stats {
            self.render_decorrelation_stretch_stats_drawer(ui, palette);
        }
    }

    fn render_decorrelation_stretch_stats_drawer(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_med = hex_to_c32(&palette.theme.b_med);
        let f_high = hex_to_c32(&palette.theme.f_high);
        let f_med = hex_to_c32(&palette.theme.f_med);
        let f_low = hex_to_c32(&palette.theme.f_low);
        let diff_del = hex_to_c32(&palette.diff_del);
        let base_c = hex_to_c32(&palette.harmonies.base);

        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.background))
            .inner_margin(egui::Margin::symmetric(14.0, 8.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                if let (Some(stats), Some(tr)) = (&self.dcs_state.stats, &self.dcs_state.transform) {
                    ui.horizontal_wrapped(|ui| {
                        // Samples & timing
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("SAMPLES & TIMING").small().strong().color(base_c));
                            ui.label(egui::RichText::new(format!("Count: {} px", stats.sample_count)).small().monospace().color(f_high));
                            ui.label(egui::RichText::new(format!("Compute: {} ms", self.dcs_state.last_duration_ms)).small().monospace().color(f_low));
                            let stride_used = if self.dcs_state.params.stride == 0 {
                                auto_stride(self.current_rgba.width(), self.current_rgba.height())
                            } else {
                                self.dcs_state.params.stride
                            };
                            ui.label(egui::RichText::new(format!("Stride: {}", stride_used)).small().monospace().color(f_low));
                        });

                        ui.add_space(16.0);

                        // Mean & Sigma
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("BAND MEAN / SIGMA").small().strong().color(base_c));
                            ui.label(egui::RichText::new(format!("R: {:.1} / {:.1}", stats.mean[0], stats.sd[0])).small().monospace().color(f_high));
                            ui.label(egui::RichText::new(format!("G: {:.1} / {:.1}", stats.mean[1], stats.sd[1])).small().monospace().color(f_high));
                            ui.label(egui::RichText::new(format!("B: {:.1} / {:.1}", stats.mean[2], stats.sd[2])).small().monospace().color(f_high));
                        });

                        ui.add_space(16.0);

                        // Correlation Matrix
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("CORRELATION MATRIX").small().strong().color(base_c));
                            for row in 0..3 {
                                let r = &stats.corr[row];
                                ui.label(egui::RichText::new(format!("[{:>6.3}, {:>6.3}, {:>6.3}]", r[0], r[1], r[2])).small().monospace().color(f_high));
                            }
                        });

                        ui.add_space(16.0);

                        // Covariance Matrix
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("COVARIANCE MATRIX").small().strong().color(base_c));
                            for row in 0..3 {
                                let r = &stats.cov[row];
                                ui.label(egui::RichText::new(format!("[{:>7.1}, {:>7.1}, {:>7.1}]", r[0], r[1], r[2])).small().monospace().color(f_high));
                            }
                        });

                        ui.add_space(16.0);

                        // Eigenvalues & Eigenvectors
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("EIGENVALUES").small().strong().color(base_c));
                            ui.label(egui::RichText::new(format!("λ: [{:.4}, {:.4}, {:.4}]", tr.eigenvalues[0], tr.eigenvalues[1], tr.eigenvalues[2])).small().monospace().color(f_high));
                            ui.label(egui::RichText::new("EIGENVECTORS (COLUMNS)").small().strong().color(base_c));
                            for row in 0..3 {
                                let v = &tr.eigenvectors[row];
                                ui.label(egui::RichText::new(format!("[{:>6.3}, {:>6.3}, {:>6.3}]", v[0], v[1], v[2])).small().monospace().color(f_high));
                            }
                        });

                        ui.add_space(16.0);

                        // Transform Matrix T & Offset
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("TRANSFORM MATRIX T").small().strong().color(base_c));
                            for row in 0..3 {
                                let t_row = &tr.transform[row];
                                ui.label(egui::RichText::new(format!("[{:>6.3}, {:>6.3}, {:>6.3}]", t_row[0], t_row[1], t_row[2])).small().monospace().color(f_high));
                            }
                            ui.label(egui::RichText::new(format!("Offset: [{:.1}, {:.1}, {:.1}]", tr.offset[0], tr.offset[1], tr.offset[2])).small().monospace().color(f_low));
                        });
                    });

                    // Diagnostic warnings
                    if !tr.warnings.is_empty() {
                        ui.add_space(4.0);
                        for w in &tr.warnings {
                            ui.label(egui::RichText::new(format!("⚠ {}", w)).small().strong().color(diff_del));
                        }
                    }

                    if let Some(lims) = tr.tol_limits {
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(format!("Tol clip limits: R: [{:.1}, {:.1}], G: [{:.1}, {:.1}], B: [{:.1}, {:.1}]",
                                lims[0][0], lims[0][1], lims[1][0], lims[1][1], lims[2][0], lims[2][1]))
                                .small()
                                .monospace()
                                .color(f_low),
                        );
                    }
                } else {
                    ui.label(egui::RichText::new("No statistics computed yet.").small().color(f_med));
                }
            });
    }

    fn render_decorrelation_stretch_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        // Ensure stretched texture is loaded
        if self.dcs_state.dirty_texture || self.dcs_state.stretched_texture.is_none() {
            if let Some(ref dcs_rgba) = self.dcs_state.stretched_rgba {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [dcs_rgba.width() as usize, dcs_rgba.height() as usize],
                    dcs_rgba.as_raw(),
                );
                self.dcs_state.stretched_texture = Some(ctx.load_texture(
                    "decorrelation_stretch_preview_tex",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                self.dcs_state.dirty_texture = false;
            }
        }

        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // If user is holding original (via Hold to Compare button), show full original
        if self.dcs_state.is_holding_original {
            if let Some(ref orig_tex) = self.texture {
                let mut full_mesh = egui::Mesh::with_texture(orig_tex.id());
                full_mesh.add_rect_with_uv(
                    img_rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.add(egui::Shape::mesh(full_mesh));

                let badge_pos = Pos2::new(img_rect.min.x + 12.0, img_rect.min.y + 12.0);
                let badge_rect = Rect::from_min_size(badge_pos, Vec2::new(170.0, 24.0));
                let badge_bg = hex_to_c32_alpha(&palette.theme.b_low, 210);
                let badge_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_med)), palette);
                painter.rect_filled(badge_rect, Rounding::same(4.0), badge_bg);
                painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, b_med));
                painter.text(
                    badge_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "ORIGINAL (HOLDING)",
                    egui::FontId::proportional(11.0),
                    badge_fg,
                );
            }
            return;
        }

        let split = self.dcs_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // 1. Draw Left Side (Original / Before)
        if let Some(ref orig_tex) = self.texture {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut left_mesh = egui::Mesh::with_texture(orig_tex.id());
            left_mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(left_mesh));
        }

        // 2. Draw Right Side (Decorrelation Stretched / After)
        let dcs_tex_id = self.dcs_state.stretched_texture.as_ref().map(|t| t.id())
            .or_else(|| self.texture.as_ref().map(|t| t.id()));
        if let Some(tex_id) = dcs_tex_id {
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut right_mesh = egui::Mesh::with_texture(tex_id);
            right_mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(right_mesh));
        }

        // 3. Draw Crisp Divider Line
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(3.0, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(1.5, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_radius = 16.0;
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

        if near_divider || self.dcs_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            self.dcs_state.is_dragging_split = true;
        }

        if self.dcs_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let new_split = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    self.dcs_state.comparison_split = new_split;
                }
            }
            if response.drag_stopped() {
                self.dcs_state.is_dragging_split = false;
            }
        }

        // 6. Floating HUD Badges
        let before_badge_pos = Pos2::new(img_rect.min.x + 12.0, img_rect.min.y + 12.0);
        let before_rect = Rect::from_min_size(before_badge_pos, Vec2::new(140.0, 24.0));
        let badge_bg = hex_to_c32_alpha(&palette.theme.b_low, 210);
        let before_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_med)), palette);
        painter.rect_filled(before_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(before_rect, Rounding::same(4.0), Stroke::new(1.0, b_med));
        painter.text(
            before_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            egui::FontId::proportional(11.0),
            before_fg,
        );

        let after_badge_pos = Pos2::new(img_rect.max.x - 172.0, img_rect.min.y + 12.0);
        let after_rect = Rect::from_min_size(after_badge_pos, Vec2::new(160.0, 24.0));
        let after_fg = contrast_text_c32(b_low, Some(base_c), palette);
        painter.rect_filled(after_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(after_rect, Rounding::same(4.0), Stroke::new(1.0, base_c));
        painter.text(
            after_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("DCS ({:.0} ms)", self.dcs_state.last_duration_ms),
            egui::FontId::proportional(11.0),
            after_fg,
        );
    }

    // --- Document OCR Suite ---

    pub fn run_ocr(&mut self) {
        if self.ocr_state.in_progress {
            return;
        }
        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png) {
            self.ocr_state.error = Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        let engine = self.ocr_state.engine;
        let engine_arg = engine.arg_name().to_string();
        let custom_prompt = if self.ocr_state.custom_prompt.trim().is_empty() {
            None
        } else {
            Some(self.ocr_state.custom_prompt.trim().to_string())
        };

        let (tx, rx) = std::sync::mpsc::channel::<OcrMsg>();
        self.ocr_rx = Some(rx);
        self.ocr_state.in_progress = true;
        self.ocr_state.error = None;
        self.ocr_state.status_text = Some(format!("Starting {}...", engine.label()));
        let child_pid_arc = self.ocr_child_pid.clone();

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let _ = tx.send(OcrMsg::Progress(format!("Launching OCR engine ({})...", engine_arg)));

            let script_candidates = [
                "scripts/ocr.py",
                "./scripts/ocr.py",
            ];
            let mut script_path = "scripts/ocr.py".to_string();
            for p in &script_candidates {
                if std::path::Path::new(p).exists() {
                    script_path = p.to_string();
                    break;
                }
            }

            let venv_python = std::env::var("HOME")
                .map(|h| format!("{}/.local/share/little-screen-clip/nobg-venv/bin/python", h))
                .unwrap_or_default();
            let python_bin = if std::path::Path::new(&venv_python).exists() {
                venv_python
            } else {
                "python3".to_string()
            };

            let mut cmd = Command::new(python_bin);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            cmd.arg(&script_path);
            cmd.arg("-").arg("-");
            cmd.arg("--engine").arg(&engine_arg);
            if let Some(ref p) = custom_prompt {
                cmd.arg("--prompt").arg(p);
            }

            let mut child = match cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(OcrMsg::Error(format!("Failed to spawn python3 {}: {}", script_path, e)));
                    return;
                }
            };

            let pid = child.id();
            child_pid_arc.store(pid, std::sync::atomic::Ordering::SeqCst);

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    child_pid_arc.store(0, std::sync::atomic::Ordering::SeqCst);
                    let _ = tx.send(OcrMsg::Error(format!("Failed to pipe image to OCR worker: {}", e)));
                    return;
                }
            }

            let stdout_handle = child.stdout.take();
            let stderr_handle = child.stderr.take();

            let (stdout_tx, stdout_rx) = std::sync::mpsc::channel::<Vec<u8>>();
            if let Some(mut stdout) = stdout_handle {
                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut out = Vec::new();
                    let _ = stdout.read_to_end(&mut out);
                    let _ = stdout_tx.send(out);
                });
            }

            let (stderr_tx, stderr_rx) = std::sync::mpsc::channel::<String>();
            let tx_progress = tx.clone();
            if let Some(stderr) = stderr_handle {
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stderr);
                    let mut all_stderr = Vec::new();
                    for line in reader.lines().flatten() {
                        if line.starts_with("[ocr:progress]") {
                            let msg = line.trim_start_matches("[ocr:progress]").trim().to_string();
                            let _ = tx_progress.send(OcrMsg::Progress(msg));
                        } else {
                            all_stderr.push(line);
                        }
                    }
                    let _ = stderr_tx.send(all_stderr.join("\n"));
                });
            }

            let status = match child.wait() {
                Ok(s) => {
                    child_pid_arc.store(0, std::sync::atomic::Ordering::SeqCst);
                    s
                }
                Err(e) => {
                    child_pid_arc.store(0, std::sync::atomic::Ordering::SeqCst);
                    let _ = tx.send(OcrMsg::Error(format!("Error waiting for OCR worker: {}", e)));
                    return;
                }
            };

            let stdout_bytes = stdout_rx.recv().unwrap_or_default();
            let stderr_text = stderr_rx.recv().unwrap_or_default();

            let duration_ms = start.elapsed().as_millis();
            if status.success() {
                let text = String::from_utf8_lossy(&stdout_bytes).to_string();
                if text.trim().is_empty() {
                    let err_msg = if !stderr_text.trim().is_empty() {
                        format!("OCR finished with no recognized text. Diagnostics:\n{}", stderr_text)
                    } else {
                        "OCR engine returned empty output (no text recognized in image).".to_string()
                    };
                    eprintln!("[OCR Warning] {}", err_msg);
                    let _ = tx.send(OcrMsg::Error(err_msg));
                } else {
                    let _ = tx.send(OcrMsg::Success {
                        text,
                        engine: engine_arg,
                        duration_ms,
                    });
                }
            } else {
                let err_detail = if !stderr_text.trim().is_empty() {
                    stderr_text
                } else {
                    format!("Process exited with status code: {:?}", status.code())
                };
                eprintln!("[OCR Error] {}", err_detail);
                let _ = tx.send(OcrMsg::Error(format!("OCR worker failed: {}", err_detail)));
            }
        });
    }

    pub fn poll_ocr(&mut self) {
        if let Some(ref rx) = self.ocr_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    OcrMsg::Progress(status) => {
                        self.ocr_state.status_text = Some(status);
                    }
                    OcrMsg::Success { text, engine, duration_ms } => {
                        self.ocr_state.extracted_text = text;
                        self.ocr_state.duration_ms = Some(duration_ms);
                        self.ocr_state.in_progress = false;
                        self.ocr_state.status_text = None;
                        self.ocr_rx = None;
                        self.ocr_child_pid.store(0, std::sync::atomic::Ordering::SeqCst);
                        self.show_toast(format!("✔ OCR complete with {} in {:.2}s", engine, duration_ms as f64 / 1000.0));
                        break;
                    }
                    OcrMsg::Error(err) => {
                        self.ocr_state.error = Some(err);
                        self.ocr_state.in_progress = false;
                        self.ocr_state.status_text = None;
                        self.ocr_rx = None;
                        self.ocr_child_pid.store(0, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
    }

    pub fn cancel_ocr(&mut self) {
        let pid = self.ocr_child_pid.swap(0, std::sync::atomic::Ordering::SeqCst);
        if pid != 0 {
            #[cfg(unix)]
            unsafe {
                // Send SIGTERM then SIGKILL to the entire process group
                libc::kill(-(pid as i32), libc::SIGTERM);
                libc::kill(-(pid as i32), libc::SIGKILL);
                // Also signal the process directly
                libc::kill(pid as i32, libc::SIGTERM);
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        if self.ocr_state.in_progress {
            self.ocr_state.in_progress = false;
            self.ocr_state.status_text = None;
            self.ocr_rx = None;
            self.show_toast("⏹ OCR process cancelled");
        }
    }

    pub fn push_text_to_hand_slot(&mut self, hand: HandSide, text: String) {
        let req = IpcRequest::PushText {
            text: text.clone(),
            hand,
        };
        let _ = crate::send_ipc_request(&req);
        let text_for_cmd = text;
        std::thread::spawn(move || {
            use std::io::Write;
            if let Ok(mut child) = Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text_for_cmd.as_bytes());
                }
                let _ = child.wait();
            } else if let Ok(mut child) = Command::new("xclip")
                .arg("-selection")
                .arg("clipboard")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text_for_cmd.as_bytes());
                }
                let _ = child.wait();
            }
        });
        self.show_toast(format!("📋 Pushed to {} Hand slot", match hand { HandSide::Left => "Left", HandSide::Right => "Right" }));
    }

    fn render_ocr_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        self.poll_ocr();
        if self.ocr_state.in_progress {
            ctx.request_repaint();
        }

        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let triadic_1 = hex_to_c32(&palette.harmonies.triadic_1);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let diff_add = hex_to_c32(&palette.diff_add);
        let f_med = hex_to_c32(&palette.theme.f_med);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.ocr_state.in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(triadic_1));
                        ui.add_space(6.0);
                        let st = self.ocr_state.status_text.as_deref().unwrap_or("Running OCR model on image...");
                        ui.label(
                            egui::RichText::new(st)
                                .small()
                                .monospace()
                                .strong()
                                .color(triadic_1),
                        );

                        ui.add_space(10.0);
                        let cancel_fg = contrast_text_c32(diff_del, None, palette);
                        let cancel_btn = egui::Button::new(
                            egui::RichText::new("⏹ Cancel (Esc)")
                                .small()
                                .strong()
                                .color(cancel_fg),
                        )
                        .fill(diff_del)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));

                        if ui.add(cancel_btn).on_hover_text("Stop running OCR process and free GPU memory immediately (Esc)").clicked() {
                            self.cancel_ocr();
                        }
                    } else {
                        let engines = [
                            (OcrEngine::GlmOcr, "GLM-OCR (Default)"),
                            (OcrEngine::Tesseract, "Tesseract (Quick)"),
                        ];

                        for (eng, label) in engines {
                            let active = self.ocr_state.engine == eng;
                            let (bg, border, text_col) = if active {
                                (triadic_1, b_med, contrast_text_c32(triadic_1, None, palette))
                            } else {
                                (b_med, b_low, contrast_text_c32(b_med, Some(f_med), palette))
                            };

                            let btn = egui::Button::new(
                                egui::RichText::new(label).small().strong().color(text_col),
                            )
                            .fill(bg)
                            .stroke(Stroke::new(1.0, border))
                            .rounding(Rounding::same(4.0));

                            if ui.add(btn).clicked() {
                                self.ocr_state.engine = eng;
                            }
                        }

                        ui.add_space(8.0);

                        let run_fg = contrast_text_c32(triadic_1, None, palette);
                        let run_btn = egui::Button::new(
                            egui::RichText::new("📝 Run OCR (Enter)").small().strong().color(run_fg),
                        )
                        .fill(triadic_1)
                        .stroke(Stroke::new(1.0, b_med))
                        .rounding(Rounding::same(4.0));

                        if ui.add(run_btn).on_hover_text("Execute AI text recognition with chosen model (Enter)").clicked() {
                            self.run_ocr();
                        }

                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.ocr_state.custom_prompt)
                                .hint_text("Prompt (optional, e.g. 'Text Recognition:')...")
                                .desired_width(180.0),
                        );
                    }

                    if let Some(err) = &self.ocr_state.error {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .small()
                                .strong()
                                .color(diff_del),
                        );
                    }

                    if !self.ocr_state.extracted_text.is_empty() {
                        let chars = self.ocr_state.extracted_text.len();
                        let lines = self.ocr_state.extracted_text.lines().count();
                        let dur_str = if let Some(ms) = self.ocr_state.duration_ms {
                            format!("{:.2}s", ms as f64 / 1000.0)
                        } else {
                            "".to_string()
                        };
                        let stat_txt = if dur_str.is_empty() {
                            format!("{} chars • {} lines", chars, lines)
                        } else {
                            format!("{} chars • {} lines • {}", chars, lines, dur_str)
                        };
                        chip_badge(ui, &stat_txt, triadic_1, b_low);

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let copy_fg = contrast_text_c32(triadic_1, None, palette);
                            let copy_btn = egui::Button::new(
                                egui::RichText::new("📋 Copy Text").small().strong().color(copy_fg),
                            )
                            .fill(triadic_1)
                            .stroke(Stroke::new(1.0, b_med))
                            .rounding(Rounding::same(4.0));

                            if ui.add(copy_btn).clicked() {
                                ctx.output_mut(|o| o.copied_text = self.ocr_state.extracted_text.clone());
                                self.show_toast("📋 Extracted text copied to clipboard!");
                            }

                            let text_copy = self.ocr_state.extracted_text.clone();
                            let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                            let push_rh = egui::Button::new(
                                egui::RichText::new("📋 Slot: RH").small().color(rh_fg),
                            )
                            .fill(b_low)
                            .stroke(Stroke::new(1.0, diff_add))
                            .rounding(Rounding::same(3.0));

                            if ui.add(push_rh).on_hover_text("Push extracted text to Right Hand slot").clicked() {
                                self.push_text_to_hand_slot(HandSide::Right, text_copy.clone());
                            }

                            let lh_fg = contrast_text_c32(b_low, Some(base_c), palette);
                            let push_lh = egui::Button::new(
                                egui::RichText::new("📋 Slot: LH").small().color(lh_fg),
                            )
                            .fill(b_low)
                            .stroke(Stroke::new(1.0, base_c))
                            .rounding(Rounding::same(3.0));

                            if ui.add(push_lh).on_hover_text("Push extracted text to Left Hand slot").clicked() {
                                self.push_text_to_hand_slot(HandSide::Left, text_copy);
                            }
                        });
                    }
                });

                if !self.ocr_state.extracted_text.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        egui::ScrollArea::vertical()
                            .max_height(90.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.ocr_state.extracted_text)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(ui.available_width())
                                        .desired_rows(3),
                                );
                            });
                    });
                }
            });
    }

    // --- Barcode / QR / DataMatrix Decoder ---

    pub fn run_barcode_scan(&mut self) {
        self.barcode_state.is_scanning = true;
        let start = std::time::Instant::now();
        let base_img = self.current_display_image();
        let detections = little_clip_ops::barcode::detect_barcodes(&base_img);
        self.barcode_state.last_scan_duration_ms = start.elapsed().as_millis();
        self.barcode_state.has_scanned = true;
        self.barcode_state.is_scanning = false;
        let count = detections.len();
        self.barcode_state.detections = detections;
        self.barcode_state.selected_idx = if count > 0 { Some(0) } else { None };
        if count > 0 {
            self.show_toast(format!("✔ Found {} code{} in {}ms", count, if count == 1 { "" } else { "s" }, self.barcode_state.last_scan_duration_ms));
        } else {
            self.show_toast("No barcodes or QR codes found");
        }
    }

    fn render_barcode_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let f_high = hex_to_c32(&palette.theme.f_high);
        let f_med = hex_to_c32(&palette.theme.f_med);
        let f_low = hex_to_c32(&palette.theme.f_low);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let rescan_fg = contrast_text_c32(base_c, None, palette);
                    let rescan_btn = egui::Button::new(
                        egui::RichText::new("🔄 Rescan (Enter)").small().strong().color(rescan_fg),
                    )
                    .fill(base_c)
                    .stroke(Stroke::new(1.0, b_med))
                    .rounding(Rounding::same(4.0));

                    if ui.add(rescan_btn).on_hover_text("Scan current image for QR codes, Data Matrix, and barcodes (Enter)").clicked() {
                        self.run_barcode_scan();
                    }

                    ui.add_space(8.0);
                    let count = self.barcode_state.detections.len();
                    let (badge_txt, fg, bg) = if count > 0 {
                        (format!("{} Codes Detected ({}ms)", count, self.barcode_state.last_scan_duration_ms), base_c, b_low)
                    } else if self.barcode_state.has_scanned {
                        ("No Barcodes Detected".to_string(), f_med, b_low)
                    } else {
                        ("Ready to Scan".to_string(), f_med, b_low)
                    };
                    chip_badge(ui, &badge_txt, fg, bg);

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("QR • DataMatrix • Aztec • PDF417 • Code 128 • EAN-13")
                            .small()
                            .color(f_low),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.barcode_state.detections.is_empty() {
                            let copy_fg = contrast_text_c32(b_med, Some(f_med), palette);
                            let copy_all_btn = egui::Button::new(
                                egui::RichText::new("📋 Copy All").small().color(copy_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0, b_low))
                            .rounding(Rounding::same(3.0));

                            if ui.add(copy_all_btn).clicked() {
                                let combined: Vec<String> = self.barcode_state.detections.iter().map(|d| d.text.clone()).collect();
                                ctx.output_mut(|o| o.copied_text = combined.join("\n"));
                                self.show_toast("📋 Copied all barcode payloads to clipboard!");
                            }
                        }
                    });
                });

                if !self.barcode_state.detections.is_empty() {
                    ui.add_space(4.0);
                    egui::ScrollArea::horizontal()
                        .max_height(75.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (idx, det) in self.barcode_state.detections.clone().iter().enumerate() {
                                    let is_selected = self.barcode_state.selected_idx == Some(idx);
                                    let card_border = if is_selected {
                                        diff_mod
                                    } else {
                                        b_med
                                    };

                                    egui::Frame::none()
                                        .fill(b_low)
                                        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                                        .stroke(Stroke::new(1.0, card_border))
                                        .rounding(Rounding::same(4.0))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                chip_badge(ui, &det.format, base_c, b_low);

                                                let display_text = if det.text.len() > 36 {
                                                    format!("{}...", &det.text[..36])
                                                } else {
                                                    det.text.clone()
                                                };
                                                let text_fg = contrast_text_c32(b_low, Some(f_high), palette);
                                                ui.label(
                                                    egui::RichText::new(&display_text)
                                                        .small()
                                                        .monospace()
                                                        .color(text_fg),
                                                )
                                                .on_hover_text(&det.text);

                                                if det.is_url {
                                                    let url_fg = contrast_text_c32(b_med, Some(base_c), palette);
                                                    let url_btn = egui::Button::new(
                                                        egui::RichText::new("🌐 Open").small().color(url_fg),
                                                    )
                                                    .fill(b_med)
                                                    .rounding(Rounding::same(3.0));
                                                    if ui.add(url_btn).clicked() {
                                                        let _ = Command::new("xdg-open").arg(&det.text).spawn();
                                                    }
                                                }

                                                let copy_btn = crate::secondary_button("📋", b_med, palette)
                                                    .rounding(Rounding::same(3.0));
                                                if ui.add(copy_btn).on_hover_text("Copy this code").clicked() {
                                                    ctx.output_mut(|o| o.copied_text = det.text.clone());
                                                    self.show_toast("📋 Copied code payload to clipboard!");
                                                }

                                                let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                                                let push_btn = egui::Button::new(
                                                    egui::RichText::new("RH").small().color(rh_fg),
                                                )
                                                .fill(b_low)
                                                .stroke(Stroke::new(1.0, diff_add))
                                                .rounding(Rounding::same(3.0));
                                                if ui.add(push_btn).on_hover_text("Push to Right Hand slot").clicked() {
                                                    self.push_text_to_hand_slot(HandSide::Right, det.text.clone());
                                                }
                                            });
                                        });
                                }
                            });
                        });
                }
            });
    }

    fn render_barcode_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let ptr_pos = response.hover_pos();
        let clicked = response.clicked();
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let b_low = hex_to_c32(&palette.theme.b_low);
        let f_high = hex_to_c32(&palette.theme.f_high);

        for (idx, det) in self.barcode_state.detections.iter().enumerate() {
            let is_selected = self.barcode_state.selected_idx == Some(idx);
            let b = det.bbox;
            let scr_min = Pos2::new(img_rect.min.x + b[0] * img_rect.width(), img_rect.min.y + b[1] * img_rect.height());
            let scr_max = Pos2::new(img_rect.min.x + b[2] * img_rect.width(), img_rect.min.y + b[3] * img_rect.height());
            let code_rect = Rect::from_min_max(scr_min, scr_max);

            let hovered = ptr_pos.map(|p| code_rect.contains(p)).unwrap_or(false);

            let (fill, border, stroke_w) = if is_selected || hovered {
                (hex_to_c32_alpha(&palette.diff_mod, 50), diff_mod, 2.5)
            } else {
                (hex_to_c32_alpha(&palette.harmonies.base, 30), base_c, 1.5)
            };

            if det.points.len() >= 3 {
                let pts: Vec<Pos2> = det.points.iter().map(|p| Pos2::new(img_rect.min.x + p[0] * img_rect.width(), img_rect.min.y + p[1] * img_rect.height())).collect();
                for i in 0..pts.len() {
                    let next = pts[(i + 1) % pts.len()];
                    painter.line_segment([pts[i], next], Stroke::new(stroke_w, border));
                }
                painter.rect_filled(code_rect, Rounding::same(3.0), fill);
            } else {
                painter.rect_filled(code_rect, Rounding::same(4.0), fill);
                painter.rect_stroke(code_rect, Rounding::same(4.0), Stroke::new(stroke_w, border));
            }

            let badge_pos = Pos2::new(code_rect.min.x, (code_rect.min.y - 18.0).max(img_rect.min.y));
            let badge_rect = Rect::from_min_size(badge_pos, Vec2::new(75.0, 16.0));
            let badge_text_col = contrast_text_c32(b_low, Some(f_high), palette);
            painter.rect_filled(badge_rect, Rounding::same(3.0), b_low);
            painter.rect_stroke(badge_rect, Rounding::same(3.0), Stroke::new(1.0, border));
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                &det.format,
                egui::FontId::proportional(9.0),
                badge_text_col,
            );

            if hovered {
                egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new(format!("barcode_tip_{}", idx)), |ui| {
                    ui.label(egui::RichText::new(format!("Format: {}", det.format)).strong().color(base_c));
                    ui.label(egui::RichText::new(&det.text).monospace());
                    if det.is_url {
                        let triadic_1 = hex_to_c32(&palette.harmonies.triadic_1);
                        ui.label(egui::RichText::new("🌐 Click in toolbar to open URL").small().color(triadic_1));
                    }
                });

                if clicked {
                    self.barcode_state.selected_idx = Some(idx);
                }
            }
        }
    }

    fn render_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let avail_size = ui.available_size() - Vec2::new(0.0, 32.0);
        if !self.auto_fitted && avail_size.x > 50.0 && avail_size.y > 50.0 {
            self.fit_to_view(avail_size);
            self.auto_fitted = true;
        }

        let (response, painter) = ui.allocate_painter(avail_size, egui::Sense::click_and_drag());
        let canvas_rect = response.rect;

        // Background canvas fill
        painter.rect_filled(canvas_rect, Rounding::same(6.0), hex_to_c32(&palette.theme.background));

        // Panning interaction (drag) when in View, Resize, Palette, RemoveBg, DepthMap, Ocr, Barcode, or Convert mode (when not dragging split handle)
        let middle_drag = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let can_drag_pan = self.active_mode == ImageToolMode::View
            || self.active_mode == ImageToolMode::Resize
            || self.active_mode == ImageToolMode::Palette
            || self.active_mode == ImageToolMode::RemoveBg
            || self.active_mode == ImageToolMode::DepthMap
            || self.active_mode == ImageToolMode::Ocr
            || self.active_mode == ImageToolMode::Barcode
            || (self.active_mode == ImageToolMode::Convert && !self.convert_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::DecorrelationStretch && !self.dcs_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::FilmSim && !self.film_sim_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::CreativeFx && !self.creative_fx_state.is_dragging_split);
        if (can_drag_pan && response.dragged()) || middle_drag {
            self.pan += response.drag_delta();
        }

        // Zooming interaction (mouse wheel)
        if response.hovered() {
            let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
            if scroll_delta.abs() > 0.0 {
                let zoom_factor = (scroll_delta * 0.002).exp();
                let old_zoom = self.zoom;
                let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 50.0);

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
            draw_checkerboard(&painter, img_rect, self.zoom, hex_to_c32(&palette.theme.b_low), hex_to_c32(&palette.theme.b_med));

            // Draw image mesh (or Squoosh-style split view in Convert mode or DCS curtain view in DecorrelationStretch mode)
            if self.active_mode == ImageToolMode::Convert {
                self.render_convert_comparison(ui, &painter, &response, img_rect, canvas_rect, ctx, palette);
            } else if self.active_mode == ImageToolMode::DecorrelationStretch {
                self.render_decorrelation_stretch_comparison(ui, &painter, &response, img_rect, canvas_rect, ctx, palette);
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
                Stroke::new(1.0, hex_to_c32_alpha(&palette.theme.f_med, 40)),
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
                painter.rect_filled(Rect::from_min_max(screen_min, screen_max), Rounding::ZERO, Color32::BLACK);
            }

            // Mode-specific on-canvas overlays
            if self.active_mode == ImageToolMode::Crop {
                self.render_crop_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Deskew {
                self.render_deskew_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Redact {
                self.render_redact_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::PickColor {
                self.render_pick_color_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Barcode {
                self.render_barcode_overlay(ui, &painter, &response, img_rect, palette);
            }
        }
    }

    fn render_crop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let norm = self.crop_state.normalized_rect;
        let crop_screen_min = Pos2::new(
            img_rect.min.x + norm.min.x * img_rect.width(),
            img_rect.min.y + norm.min.y * img_rect.height(),
        );
        let crop_screen_max = Pos2::new(
            img_rect.min.x + norm.max.x * img_rect.width(),
            img_rect.min.y + norm.max.y * img_rect.height(),
        );
        let crop_screen_rect = Rect::from_min_max(crop_screen_min, crop_screen_max);

        // 1. Draw Exterior Darkened Mask
        let mask_col = Color32::from_black_alpha(170);
        if crop_screen_rect.min.y > img_rect.min.y {
            painter.rect_filled(
                Rect::from_min_max(img_rect.min, Pos2::new(img_rect.max.x, crop_screen_rect.min.y)),
                Rounding::ZERO,
                mask_col,
            );
        }
        if crop_screen_rect.max.y < img_rect.max.y {
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(img_rect.min.x, crop_screen_rect.max.y), img_rect.max),
                Rounding::ZERO,
                mask_col,
            );
        }
        if crop_screen_rect.min.x > img_rect.min.x {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(img_rect.min.x, crop_screen_rect.min.y),
                    Pos2::new(crop_screen_rect.min.x, crop_screen_rect.max.y),
                ),
                Rounding::ZERO,
                mask_col,
            );
        }
        if crop_screen_rect.max.x < img_rect.max.x {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(crop_screen_rect.max.x, crop_screen_rect.min.y),
                    Pos2::new(img_rect.max.x, crop_screen_rect.max.y),
                ),
                Rounding::ZERO,
                mask_col,
            );
        }

        // 2. Rule of Thirds Grid
        let w = crop_screen_rect.width();
        let h = crop_screen_rect.height();
        let grid_stroke = Stroke::new(1.0, hex_to_c32_alpha(&palette.theme.f_med, 60));
        painter.line_segment(
            [
                Pos2::new(crop_screen_rect.min.x + w / 3.0, crop_screen_rect.min.y),
                Pos2::new(crop_screen_rect.min.x + w / 3.0, crop_screen_rect.max.y),
            ],
            grid_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(crop_screen_rect.min.x + w * 2.0 / 3.0, crop_screen_rect.min.y),
                Pos2::new(crop_screen_rect.min.x + w * 2.0 / 3.0, crop_screen_rect.max.y),
            ],
            grid_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(crop_screen_rect.min.x, crop_screen_rect.min.y + h / 3.0),
                Pos2::new(crop_screen_rect.max.x, crop_screen_rect.min.y + h / 3.0),
            ],
            grid_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(crop_screen_rect.min.x, crop_screen_rect.min.y + h * 2.0 / 3.0),
                Pos2::new(crop_screen_rect.max.x, crop_screen_rect.min.y + h * 2.0 / 3.0),
            ],
            grid_stroke,
        );

        // 3. Crisp Crop Outline
        painter.rect_stroke(crop_screen_rect, Rounding::ZERO, Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)));

        // 4. Handles definitions & drawing
        let handle_radius = 5.0;
        let handles: [(CropHandle, Pos2, egui::CursorIcon); 8] = [
            (CropHandle::TopLeft, crop_screen_rect.left_top(), egui::CursorIcon::ResizeNorthWest),
            (CropHandle::TopRight, crop_screen_rect.right_top(), egui::CursorIcon::ResizeNorthEast),
            (CropHandle::BottomLeft, crop_screen_rect.left_bottom(), egui::CursorIcon::ResizeSouthWest),
            (CropHandle::BottomRight, crop_screen_rect.right_bottom(), egui::CursorIcon::ResizeSouthEast),
            (CropHandle::Top, Pos2::new(crop_screen_rect.center().x, crop_screen_rect.top()), egui::CursorIcon::ResizeNorth),
            (CropHandle::Bottom, Pos2::new(crop_screen_rect.center().x, crop_screen_rect.bottom()), egui::CursorIcon::ResizeSouth),
            (CropHandle::Left, Pos2::new(crop_screen_rect.left(), crop_screen_rect.center().y), egui::CursorIcon::ResizeWest),
            (CropHandle::Right, Pos2::new(crop_screen_rect.right(), crop_screen_rect.center().y), egui::CursorIcon::ResizeEast),
        ];

        for (_, pos, _) in &handles {
            let handle_rect = Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
            painter.rect_filled(handle_rect, Rounding::same(1.0), hex_to_c32(&palette.harmonies.base));
            painter.rect_stroke(handle_rect, Rounding::same(1.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)));
        }

        // 5. Floating Dimensions Badge
        let px_w = ((norm.max.x - norm.min.x) * self.current_rgba.width() as f32).round() as u32;
        let px_h = ((norm.max.y - norm.min.y) * self.current_rgba.height() as f32).round() as u32;
        let badge_str = format!("{} × {} px ({})", px_w, px_h, format_aspect_ratio(px_w, px_h));
        let badge_pos = Pos2::new(
            crop_screen_rect.center().x,
            (crop_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
        );
        let b_low = hex_to_c32(&palette.theme.b_low);
        let badge_text_col = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
        let galley = ui.painter().layout_no_wrap(
            badge_str,
            egui::FontId::proportional(11.0),
            badge_text_col,
        );
        let badge_rect = Rect::from_center_size(badge_pos, galley.size() + Vec2::new(12.0, 4.0));
        painter.rect_filled(badge_rect, Rounding::same(4.0), b_low);
        painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, hex_to_c32(&palette.harmonies.base)));
        painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, badge_text_col);

        // 6. Interaction logic
        if let Some(ptr_pos) = response.hover_pos() {
            let hit_dist = 12.0;
            let hovered_handle = handles.iter().find_map(|(h, pos, cur)| {
                if pos.distance(ptr_pos) <= hit_dist {
                    Some((*h, *cur))
                } else {
                    None
                }
            });

            if let Some((h, cur)) = hovered_handle {
                ui.output_mut(|o| o.cursor_icon = cur);
                if response.drag_started() {
                    self.crop_state.active_drag = Some((h, ptr_pos, self.crop_state.normalized_rect));
                }
            } else if crop_screen_rect.contains(ptr_pos) {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
                if response.drag_started() {
                    self.crop_state.active_drag = Some((CropHandle::MoveBox, ptr_pos, self.crop_state.normalized_rect));
                }
            } else if img_rect.contains(ptr_pos) {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                if response.drag_started() {
                    let norm_x = ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                    let norm_y = ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                    let initial = Rect::from_min_max(Pos2::new(norm_x, norm_y), Pos2::new(norm_x, norm_y));
                    self.crop_state.normalized_rect = initial;
                    self.crop_state.active_drag = Some((CropHandle::BottomRight, ptr_pos, initial));
                }
            }
        }

        if response.dragged() {
            if let Some(ptr_pos) = response.interact_pointer_pos() {
                self.crop_state.update_drag(ptr_pos, img_rect, self.current_rgba.width(), self.current_rgba.height());
            }
        }

        if response.drag_stopped() {
            self.crop_state.active_drag = None;
        }

        if response.double_clicked() {
            if let Some(ptr) = response.interact_pointer_pos() {
                if crop_screen_rect.contains(ptr) {
                    self.apply_crop();
                }
            }
        }
    }

    fn render_deskew_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let labels = ["TL (1)", "TR (2)", "BR (3)", "BL (4)"];
        let corner_names = ["Top-Left", "Top-Right", "Bottom-Right", "Bottom-Left"];

        if self.deskew_state.is_clicking_points {
            // --- 4-Point Sequential Clicking Mode ---
            let n = self.deskew_state.placed_points.len();
            let placed_screen: Vec<Pos2> = self.deskew_state.placed_points.iter().map(|p| {
                Pos2::new(img_rect.min.x + p.x * img_rect.width(), img_rect.min.y + p.y * img_rect.height())
            }).collect();

            // Draw existing placed points
            let pt_stroke = Stroke::new(2.0, hex_to_c32(&palette.diff_add));
            for (i, &pt) in placed_screen.iter().enumerate() {
                painter.circle_stroke(pt, 8.0, Stroke::new(2.0, hex_to_c32(&palette.theme.b_low)));
                painter.circle_filled(pt, 6.0, hex_to_c32(&palette.diff_add));
                painter.text(
                    pt + Vec2::new(8.0, -18.0),
                    egui::Align2::LEFT_TOP,
                    labels[i],
                    egui::FontId::proportional(11.0),
                    hex_to_c32(&palette.theme.f_high),
                );
            }

            // Draw connecting lines between placed points
            if n >= 2 {
                painter.line_segment([placed_screen[0], placed_screen[1]], pt_stroke);
            }
            if n >= 3 {
                painter.line_segment([placed_screen[1], placed_screen[2]], pt_stroke);
            }

            // Cursor rubber-banding and guide badge
            if let Some(ptr_pos) = response.hover_pos() {
                if img_rect.contains(ptr_pos) {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);

                    // Rubber-band line from last placed point to cursor
                    let rubber_stroke = Stroke::new(1.5, hex_to_c32_alpha(&palette.diff_add, 180));
                    if n >= 1 {
                        painter.line_segment([*placed_screen.last().unwrap(), ptr_pos], rubber_stroke);
                    }
                    // Closing segment preview if 3 points placed
                    if n == 3 {
                        painter.line_segment([ptr_pos, placed_screen[0]], rubber_stroke);
                    }

                    // Floating guide badge following pointer
                    let next_name = corner_names.get(n).copied().unwrap_or("Next");
                    let badge_text = format!("🎯 Click {next_idx}/4: {next_name}", next_idx = n + 1);
                    let b_low = hex_to_c32(&palette.theme.b_low);
                    let badge_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
                    let galley = ui.painter().layout_no_wrap(
                        badge_text,
                        egui::FontId::proportional(11.0),
                        badge_fg,
                    );
                    let badge_pos = Pos2::new(
                        (ptr_pos.x + 16.0).clamp(img_rect.min.x + 10.0, img_rect.max.x - galley.size().x - 14.0),
                        (ptr_pos.y + 16.0).clamp(img_rect.min.y + 10.0, img_rect.max.y - 26.0),
                    );
                    let badge_rect = Rect::from_min_size(badge_pos, galley.size() + Vec2::new(12.0, 6.0));
                    painter.rect_filled(badge_rect, Rounding::same(4.0), b_low);
                    painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, hex_to_c32(&palette.diff_add)));
                    painter.galley(badge_rect.min + Vec2::new(6.0, 3.0), galley, badge_fg);
                }
            }

            // Top banner instruction
            let top_banner_pos = Pos2::new(img_rect.center().x, img_rect.min.y + 18.0);
            let next_name = corner_names.get(n).copied().unwrap_or("");
            let banner_text = format!(
                "🎯 Click 4 points: (1) TL → (2) TR → (3) BR → (4) BL | Next: {} of 4 ({}) [Esc: Cancel]",
                n + 1,
                next_name
            );
            let b_low = hex_to_c32(&palette.theme.b_low);
            let b_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
            let banner_galley = ui.painter().layout_no_wrap(
                banner_text,
                egui::FontId::proportional(11.0),
                b_fg,
            );
            let banner_rect = Rect::from_center_size(top_banner_pos, banner_galley.size() + Vec2::new(16.0, 6.0));
            painter.rect_filled(banner_rect, Rounding::same(4.0), b_low);
            painter.rect_stroke(banner_rect, Rounding::same(4.0), Stroke::new(1.0, hex_to_c32(&palette.harmonies.base)));
            painter.galley(banner_rect.min + Vec2::new(8.0, 3.0), banner_galley, b_fg);

            // Register point placement on click
            if response.clicked() {
                if let Some(ptr_pos) = response.interact_pointer_pos() {
                    if img_rect.contains(ptr_pos) {
                        let norm_x = ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let norm_y = ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.deskew_state.placed_points.push(Pos2::new(norm_x, norm_y));

                        if self.deskew_state.placed_points.len() == 4 {
                            self.deskew_state.corners = [
                                self.deskew_state.placed_points[0],
                                self.deskew_state.placed_points[1],
                                self.deskew_state.placed_points[2],
                                self.deskew_state.placed_points[3],
                            ];
                            self.deskew_state.is_clicking_points = false;
                            self.deskew_state.placed_points.clear();
                            self.deskew_state.corners_adjusted = true;
                            self.show_toast("✓ 4 points set! Drag handles to fine-tune, Enter to apply.");
                        }
                    }
                }
            }
        } else {
            // --- Standard Quad Handles & Inspection Mode ---
            let corners = self.deskew_state.corners;
            let screen_pts = [
                Pos2::new(img_rect.min.x + corners[0].x * img_rect.width(), img_rect.min.y + corners[0].y * img_rect.height()),
                Pos2::new(img_rect.min.x + corners[1].x * img_rect.width(), img_rect.min.y + corners[1].y * img_rect.height()),
                Pos2::new(img_rect.min.x + corners[2].x * img_rect.width(), img_rect.min.y + corners[2].y * img_rect.height()),
                Pos2::new(img_rect.min.x + corners[3].x * img_rect.width(), img_rect.min.y + corners[3].y * img_rect.height()),
            ];

            // 1. Perspective Grid (rule of thirds inside quad)
            let grid_stroke = Stroke::new(1.0, hex_to_c32_alpha(&palette.theme.f_med, 80));
            for &u in &[1.0 / 3.0, 2.0 / 3.0] {
                let top_pt = screen_pts[0].to_vec2() * (1.0 - u) + screen_pts[1].to_vec2() * u;
                let bot_pt = screen_pts[3].to_vec2() * (1.0 - u) + screen_pts[2].to_vec2() * u;
                painter.line_segment([top_pt.to_pos2(), bot_pt.to_pos2()], grid_stroke);

                let left_pt = screen_pts[0].to_vec2() * (1.0 - u) + screen_pts[3].to_vec2() * u;
                let right_pt = screen_pts[1].to_vec2() * (1.0 - u) + screen_pts[2].to_vec2() * u;
                painter.line_segment([left_pt.to_pos2(), right_pt.to_pos2()], grid_stroke);
            }

            // 2. Quad Outer Edges
            let edge_stroke = Stroke::new(2.0, hex_to_c32(&palette.harmonies.base));
            painter.line_segment([screen_pts[0], screen_pts[1]], edge_stroke);
            painter.line_segment([screen_pts[1], screen_pts[2]], edge_stroke);
            painter.line_segment([screen_pts[2], screen_pts[3]], edge_stroke);
            painter.line_segment([screen_pts[3], screen_pts[0]], edge_stroke);

            // 3. Corner Handles & Labels
            let handle_radius = 6.0;
            for i in 0..4 {
                let pt = screen_pts[i];
                let is_dragging = self.deskew_state.dragging_corner == Some(i);
                let handle_col = if is_dragging {
                    hex_to_c32(&palette.diff_add)
                } else {
                    hex_to_c32(&palette.harmonies.base)
                };

                // Outer dark ring for contrast on any background
                painter.circle_stroke(pt, handle_radius + 2.0, Stroke::new(2.0, hex_to_c32(&palette.theme.b_low)));
                painter.circle_filled(pt, handle_radius, handle_col);

                let offset = match i {
                    0 => Vec2::new(-24.0, -18.0),
                    1 => Vec2::new(8.0, -18.0),
                    2 => Vec2::new(8.0, 8.0),
                    _ => Vec2::new(-24.0, 8.0),
                };
                painter.text(
                    pt + offset,
                    egui::Align2::LEFT_TOP,
                    labels[i],
                    egui::FontId::proportional(11.0),
                    hex_to_c32(&palette.theme.f_high),
                );
            }

            // 4. Centroid Guide Badge
            let centroid = Pos2::new(
                (screen_pts[0].x + screen_pts[1].x + screen_pts[2].x + screen_pts[3].x) * 0.25,
                (screen_pts[0].y + screen_pts[1].y + screen_pts[2].y + screen_pts[3].y) * 0.25,
            );
            let badge_bg = hex_to_c32(&palette.theme.b_low);
            let badge_fg = contrast_text_c32(badge_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
            let badge_str = if !self.deskew_state.corners_adjusted {
                "🎯 Click 4 Points (or drag handles) | Enter: Apply"
            } else {
                "Enter / Double-Click: Apply Deskew | Drag handles to adjust"
            };
            let galley = ui.painter().layout_no_wrap(
                badge_str.to_string(),
                egui::FontId::proportional(11.0),
                badge_fg,
            );
            let badge_rect = Rect::from_center_size(centroid, galley.size() + Vec2::new(14.0, 6.0));
            painter.rect_filled(badge_rect, Rounding::same(4.0), badge_bg);
            painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, hex_to_c32(&palette.harmonies.base)));
            painter.galley(badge_rect.min + Vec2::new(7.0, 3.0), galley, badge_fg);

            // 5. Interaction Logic
            let hit_dist = 16.0;
            let mut hovered_corner: Option<usize> = None;

            if let Some(ptr_pos) = response.hover_pos() {
                hovered_corner = screen_pts.iter().enumerate().find_map(|(idx, pt)| {
                    if pt.distance(ptr_pos) <= hit_dist {
                        Some(idx)
                    } else {
                        None
                    }
                });

                if let Some(corner_idx) = hovered_corner {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grab);
                    if response.drag_started() {
                        self.deskew_state.dragging_corner = Some(corner_idx);
                        self.deskew_state.corners_adjusted = true;
                    }
                } else if self.deskew_state.dragging_corner.is_none() && img_rect.contains(ptr_pos) {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                    // If corners haven't been adjusted yet, hint to the user that clicking sets point 1
                    if !self.deskew_state.corners_adjusted {
                        let hint_text = "🎯 Click to place Point 1 (Top-Left)";
                        let b_low = hex_to_c32(&palette.theme.b_low);
                        let h_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
                        let h_galley = ui.painter().layout_no_wrap(
                            hint_text.to_string(),
                            egui::FontId::proportional(10.5),
                            h_fg,
                        );
                        let h_pos = Pos2::new(
                            (ptr_pos.x + 14.0).clamp(img_rect.min.x + 10.0, img_rect.max.x - h_galley.size().x - 14.0),
                            (ptr_pos.y + 14.0).clamp(img_rect.min.y + 10.0, img_rect.max.y - 24.0),
                        );
                        let h_rect = Rect::from_min_size(h_pos, h_galley.size() + Vec2::new(10.0, 4.0));
                        painter.rect_filled(h_rect, Rounding::same(3.0), b_low);
                        painter.rect_stroke(h_rect, Rounding::same(3.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)));
                        painter.galley(h_rect.min + Vec2::new(5.0, 2.0), h_galley, h_fg);
                    }
                }
            }

            if response.dragged() {
                if let Some(corner_idx) = self.deskew_state.dragging_corner {
                    if let Some(ptr_pos) = response.interact_pointer_pos() {
                        let norm_x = ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let norm_y = ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.deskew_state.corners[corner_idx] = Pos2::new(norm_x, norm_y);
                    }
                }
            }

            if response.drag_stopped() {
                self.deskew_state.dragging_corner = None;
            }

            // If user clicks on canvas when corners are at default state, immediately start point 1!
            if response.clicked() && hovered_corner.is_none() && !self.deskew_state.corners_adjusted {
                if let Some(ptr_pos) = response.interact_pointer_pos() {
                    if img_rect.contains(ptr_pos) {
                        let norm_x = ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let norm_y = ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.deskew_state.is_clicking_points = true;
                        self.deskew_state.placed_points = vec![Pos2::new(norm_x, norm_y)];
                    }
                }
            }

            if response.double_clicked() {
                self.apply_deskew();
            }
        }
    }

    fn render_redact_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let img_w = self.current_rgba.width();
        let img_h = self.current_rgba.height();
        let handle_radius = 5.0;
        let hit_dist = 12.0;

        let to_screen = |norm_p: Pos2| -> Pos2 {
            Pos2::new(
                img_rect.min.x + norm_p.x * img_rect.width(),
                img_rect.min.y + norm_p.y * img_rect.height(),
            )
        };
        let to_screen_rect = |norm_r: Rect| -> Rect {
            Rect::from_min_max(to_screen(norm_r.min), to_screen(norm_r.max))
        };
        let to_norm = |screen_p: Pos2| -> Pos2 {
            Pos2::new(
                ((screen_p.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0),
                ((screen_p.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0),
            )
        };

        let diff_del = hex_to_c32(&palette.diff_del);
        let b_low_badge = hex_to_c32_alpha(&palette.theme.b_low, 230);
        let badge_text_col = contrast_text_c32(b_low_badge, Some(hex_to_c32(&palette.theme.f_high)), palette);

        // 1. Draw existing redaction boxes
        for (i, b) in self.redaction_state.boxes.iter().enumerate() {
            let box_screen_rect = to_screen_rect(b.normalized_rect);

            // Draw solid black redaction bar
            painter.rect_filled(box_screen_rect, Rounding::ZERO, Color32::BLACK);

            let is_selected = self.redaction_state.selected_idx == Some(i);
            if is_selected {
                // Highlight border
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(2.0, diff_del),
                );

                // 8 Grab Handles
                let handles = get_box_handles(box_screen_rect);
                for (_, pos, _) in &handles {
                    let handle_rect = Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
                    painter.rect_filled(handle_rect, Rounding::same(1.0), hex_to_c32(&palette.theme.f_high));
                    painter.rect_stroke(handle_rect, Rounding::same(1.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_low)));
                }

                // Floating size badge
                let px_w = (b.normalized_rect.width() * img_w as f32).round() as u32;
                let px_h = (b.normalized_rect.height() * img_h as f32).round() as u32;
                let badge_str = format!("{} × {} px", px_w, px_h);
                let badge_pos = Pos2::new(
                    box_screen_rect.center().x,
                    (box_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
                );
                let galley = ui.painter().layout_no_wrap(
                    badge_str,
                    egui::FontId::proportional(11.0),
                    badge_text_col,
                );
                let badge_rect = Rect::from_center_size(badge_pos, galley.size() + Vec2::new(12.0, 4.0));
                painter.rect_filled(badge_rect, Rounding::same(4.0), b_low_badge);
                painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, diff_del));
                painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, badge_text_col);
            } else {
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(1.0, hex_to_c32_alpha(&palette.theme.f_med, 60)),
                );
            }
        }

        // 2. Draw in-progress creating box
        if let Some(RedactionDrag::Creating(start_norm, curr_norm)) = self.redaction_state.active_drag {
            let start_s = to_screen(start_norm);
            let curr_s = to_screen(curr_norm);
            let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
            painter.rect_filled(in_progress_rect, Rounding::ZERO, Color32::from_rgba_unmultiplied(0, 0, 0, 210));
            painter.rect_stroke(in_progress_rect, Rounding::ZERO, Stroke::new(1.5, diff_del));
        }

        // 3. Set Cursor icon
        if let Some(ptr_pos) = response.hover_pos() {
            if let Some(selected_idx) = self.redaction_state.selected_idx {
                if selected_idx < self.redaction_state.boxes.len() {
                    let box_screen_rect = to_screen_rect(self.redaction_state.boxes[selected_idx].normalized_rect);
                    let handles = get_box_handles(box_screen_rect);
                    let hovered_handle = handles.iter().find_map(|(_, pos, cur)| {
                        if pos.distance(ptr_pos) <= hit_dist {
                            Some(*cur)
                        } else {
                            None
                        }
                    });

                    if let Some(cur) = hovered_handle {
                        ui.ctx().set_cursor_icon(cur);
                    } else if box_screen_rect.contains(ptr_pos) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                } else {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }

        // 4. Handle Dragging
        if response.drag_started() {
            if let Some(ptr) = response.interact_pointer_pos() {
                let mut handled = false;
                if let Some(sel_idx) = self.redaction_state.selected_idx {
                    if sel_idx < self.redaction_state.boxes.len() {
                        let init_norm = self.redaction_state.boxes[sel_idx].normalized_rect;
                        let box_screen_rect = to_screen_rect(init_norm);
                        let handles = get_box_handles(box_screen_rect);

                        if let Some((h, _, _)) = handles.iter().find(|(_, pos, _)| pos.distance(ptr) <= hit_dist) {
                            self.redaction_state.active_drag = Some(RedactionDrag::Resizing(sel_idx, *h, ptr, init_norm));
                            handled = true;
                        } else if box_screen_rect.contains(ptr) {
                            self.redaction_state.active_drag = Some(RedactionDrag::Moving(sel_idx, ptr, init_norm));
                            handled = true;
                        }
                    }
                }

                if !handled {
                    let found_idx = self.redaction_state.boxes.iter().enumerate().rev().find_map(|(i, b)| {
                        let r = to_screen_rect(b.normalized_rect);
                        if r.contains(ptr) {
                            Some(i)
                        } else {
                            None
                        }
                    });

                    if let Some(idx) = found_idx {
                        self.redaction_state.selected_idx = Some(idx);
                        let init_norm = self.redaction_state.boxes[idx].normalized_rect;
                        self.redaction_state.active_drag = Some(RedactionDrag::Moving(idx, ptr, init_norm));
                    } else if img_rect.contains(ptr) {
                        let start_norm = to_norm(ptr);
                        self.redaction_state.selected_idx = None;
                        self.redaction_state.active_drag = Some(RedactionDrag::Creating(start_norm, start_norm));
                    }
                }
            }
        }

        if response.dragged() {
            if let Some(ptr) = response.interact_pointer_pos() {
                match self.redaction_state.active_drag {
                    Some(RedactionDrag::Creating(start_norm, _)) => {
                        let curr_norm = to_norm(ptr);
                        self.redaction_state.active_drag = Some(RedactionDrag::Creating(start_norm, curr_norm));
                    }
                    Some(RedactionDrag::Moving(idx, start_ptr, init_norm)) => {
                        let delta_s = ptr - start_ptr;
                        let delta_norm_x = delta_s.x / img_rect.width();
                        let delta_norm_y = delta_s.y / img_rect.height();
                        let box_w = init_norm.width();
                        let box_h = init_norm.height();
                        let new_min_x = (init_norm.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
                        let new_min_y = (init_norm.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
                        if idx < self.redaction_state.boxes.len() {
                            self.redaction_state.boxes[idx].normalized_rect = Rect::from_min_size(
                                Pos2::new(new_min_x, new_min_y),
                                Vec2::new(box_w, box_h),
                            );
                        }
                    }
                    Some(RedactionDrag::Resizing(idx, handle, start_ptr, init_norm)) => {
                        let delta_s = ptr - start_ptr;
                        let min_w = 4.0 / img_w as f32;
                        let min_h = 4.0 / img_h as f32;
                        let new_rect = resize_norm_rect(init_norm, handle, delta_s, img_rect, min_w, min_h);
                        if idx < self.redaction_state.boxes.len() {
                            self.redaction_state.boxes[idx].normalized_rect = new_rect;
                        }
                    }
                    None => {}
                }
            }
        }

        if response.drag_stopped() {
            if let Some(RedactionDrag::Creating(start_norm, end_norm)) = self.redaction_state.active_drag {
                let min_x = start_norm.x.min(end_norm.x);
                let max_x = start_norm.x.max(end_norm.x);
                let min_y = start_norm.y.min(end_norm.y);
                let max_y = start_norm.y.max(end_norm.y);
                let norm_w = max_x - min_x;
                let norm_h = max_y - min_y;

                let min_norm_w = 4.0 / img_w as f32;
                let min_norm_h = 4.0 / img_h as f32;
                if norm_w >= min_norm_w && norm_h >= min_norm_h {
                    self.redaction_state.boxes.push(RedactionBox {
                        normalized_rect: Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y)),
                    });
                    self.redaction_state.selected_idx = Some(self.redaction_state.boxes.len() - 1);
                }
            }
            self.redaction_state.active_drag = None;
        }

        // Clicking without dragging on empty background deselects
        if response.clicked() {
            if let Some(ptr) = response.interact_pointer_pos() {
                let hit_any = self.redaction_state.boxes.iter().enumerate().rev().find_map(|(i, b)| {
                    if to_screen_rect(b.normalized_rect).contains(ptr) {
                        Some(i)
                    } else {
                        None
                    }
                });
                self.redaction_state.selected_idx = hit_any;
            }
        }
    }

    fn render_pick_color_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);

        let img_w = self.current_rgba.width() as i32;
        let img_h = self.current_rgba.height() as i32;

        let ptr_opt = response.hover_pos();
        if let Some(ptr) = ptr_opt {
            if img_rect.contains(ptr) && img_rect.width() > 0.0 && img_rect.height() > 0.0 {
                let norm_x = ((ptr.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 0.9999);
                let norm_y = ((ptr.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 0.9999);

                let px = (norm_x * img_w as f32).floor() as i32;
                let py = (norm_y * img_h as f32).floor() as i32;

                let pixel_data = *self.current_rgba.get_pixel(px as u32, py as u32);
                let col = Color32::from_rgba_unmultiplied(pixel_data[0], pixel_data[1], pixel_data[2], pixel_data[3]);
                self.pick_color_state.hovered_color = Some(col);
                self.pick_color_state.hovered_pixel = Some((px as u32, py as u32));

                if response.clicked() {
                    self.copy_color_to_clipboard(col, ui.ctx());
                }

                // Render Magnifying Loupe Card
                let canvas_rect = ui.available_rect_before_wrap();
                let loupe_r: i32 = 4; // 9x9 grid (-4..=4)
                let cell_size = 14.0;
                let grid_w = (loupe_r * 2 + 1) as f32 * cell_size; // 126.0
                let card_w = grid_w + 20.0; // 146.0
                let card_h = grid_w + 80.0; // 206.0

                let mut card_min = ptr + Vec2::new(20.0, 20.0);
                if card_min.x + card_w > canvas_rect.max.x {
                    card_min.x = ptr.x - card_w - 20.0;
                }
                if card_min.y + card_h > canvas_rect.max.y {
                    card_min.y = ptr.y - card_h - 20.0;
                }
                let card_rect = Rect::from_min_size(card_min, Vec2::new(card_w, card_h));

                // Card background & sleek harmonic border
                let card_bg = hex_to_c32_alpha(&palette.theme.b_low, 245);
                painter.rect_filled(card_rect, Rounding::same(8.0), card_bg);
                painter.rect_stroke(card_rect, Rounding::same(8.0), Stroke::new(1.5, hex_to_c32(&palette.harmonies.base)));

                // Loupe Grid
                let grid_origin = card_min + Vec2::new(10.0, 10.0);
                for dy in -loupe_r..=loupe_r {
                    for dx in -loupe_r..=loupe_r {
                        let samp_x = (px + dx).clamp(0, img_w - 1) as u32;
                        let samp_y = (py + dy).clamp(0, img_h - 1) as u32;
                        let samp_pix = self.current_rgba.get_pixel(samp_x, samp_y);
                        let cell_col = Color32::from_rgba_unmultiplied(samp_pix[0], samp_pix[1], samp_pix[2], samp_pix[3]);

                        let cell_min = grid_origin + Vec2::new((dx + loupe_r) as f32 * cell_size, (dy + loupe_r) as f32 * cell_size);
                        let cell_rect = Rect::from_min_size(cell_min, Vec2::splat(cell_size));

                        // Draw alpha checkerboard under transparent pixels
                        if samp_pix[3] < 255 {
                            let half = cell_size / 2.0;
                            painter.rect_filled(Rect::from_min_size(cell_min, Vec2::splat(half)), Rounding::ZERO, Color32::from_gray(180));
                            painter.rect_filled(Rect::from_min_size(cell_min + Vec2::splat(half), Vec2::splat(half)), Rounding::ZERO, Color32::from_gray(180));
                        }

                        painter.rect_filled(cell_rect, Rounding::ZERO, cell_col);

                        // Highlight center cell
                        if dx == 0 && dy == 0 {
                            painter.rect_stroke(cell_rect, Rounding::ZERO, Stroke::new(2.0, hex_to_c32(&palette.theme.f_high)));
                            painter.rect_stroke(cell_rect.expand(1.0), Rounding::ZERO, Stroke::new(1.0, hex_to_c32(&palette.theme.b_low)));
                        }
                    }
                }

                // Color Swatch below grid
                let swatch_y = grid_origin.y + grid_w + 8.0;
                let swatch_rect = Rect::from_min_size(Pos2::new(card_min.x + 10.0, swatch_y), Vec2::new(card_w - 20.0, 16.0));
                painter.rect_filled(swatch_rect, Rounding::same(3.0), col);
                painter.rect_stroke(swatch_rect, Rounding::same(3.0), Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)));

                // Hex Code Text
                let hex_str = format!("#{:02X}{:02X}{:02X}", pixel_data[0], pixel_data[1], pixel_data[2]);
                let hex_text_col = contrast_text_c32(card_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                let hex_galley = painter.layout_no_wrap(
                    hex_str,
                    egui::FontId::monospace(12.0),
                    hex_text_col,
                );
                painter.galley(Pos2::new(card_min.x + 10.0, swatch_y + 20.0), hex_galley, hex_text_col);

                // RGB & Coordinate Info
                let info_str = format!("RGB({}, {}, {}) | {}, {}", pixel_data[0], pixel_data[1], pixel_data[2], px, py);
                let info_text_col = contrast_text_c32(card_bg, Some(hex_to_c32(&palette.theme.f_med)), palette);
                let info_galley = painter.layout_no_wrap(
                    info_str,
                    egui::FontId::proportional(9.5),
                    info_text_col,
                );
                painter.galley(Pos2::new(card_min.x + 10.0, swatch_y + 36.0), info_galley, info_text_col);

                // Click to copy hint
                let hint_text_col = contrast_text_c32(card_bg, Some(hex_to_c32(&palette.harmonies.triadic_1)), palette);
                let hint_galley = painter.layout_no_wrap(
                    "🖱 Click to copy".to_string(),
                    egui::FontId::proportional(9.0),
                    hint_text_col,
                );
                painter.galley(Pos2::new(card_min.x + 10.0, swatch_y + 49.0), hint_galley, hint_text_col);
            } else {
                self.pick_color_state.hovered_color = None;
                self.pick_color_state.hovered_pixel = None;
            }
        } else {
            self.pick_color_state.hovered_color = None;
            self.pick_color_state.hovered_pixel = None;
        }
    }

    fn render_footer(&mut self, ui: &mut egui::Ui, palette: &AppPalette, now: f64) {
        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Help hints
                    let hint_text = match self.active_mode {
                        ImageToolMode::View => "🖱 Scroll: Zoom | Drag: Pan | ▼ EXIF Details | Ctrl+C: Copy Slot | Ctrl+S: Save As | 0: Fit",
                        ImageToolMode::Crop => "🖱 Drag crop box/handles | Enter: Apply | Ctrl+C: Copy Slot | Ctrl+S: Save As | Esc: Cancel",
                        ImageToolMode::Deskew => "🎯 Click 4 points (TL → TR → BR → BL) | 🖱 Drag handles | Enter: Apply | Esc: Close",
                        ImageToolMode::Transform => "🖱 Rotate / Flip buttons above | Ctrl+Z: Undo | Ctrl+C: Copy Slot | Ctrl+S: Save As",
                        ImageToolMode::Redact => "🖱 Drag on image: Place black bar | 🛡️ Strip EXIF | Del: Remove | Enter: Burn/Apply | Ctrl+C: Copy Slot",
                        ImageToolMode::PickColor => "🖱 Hover: Magnifying loupe | Click: Copy Hex code to clipboard | Ctrl+C: Copy Slot",
                        ImageToolMode::Convert => "🖱 Drag vertical divider: Before/After comparison | Enter: Apply to Canvas | Ctrl+S: Save As",
                        ImageToolMode::Ocr => "🖱 Choose model above | Enter: Run OCR | 📋 Copy Text / Push to Slot",
                        ImageToolMode::Barcode => "🖱 Click code card or box to inspect | Enter: Rescan | 🌐 Open Link | 📋 Copy Payload",
                        ImageToolMode::CreativeFx => "🖱 Split slider: Before/After comparison | Double-click: Fit | Sliders: Double-click to reset",
                        _ => "🖱 Scroll: Zoom | Drag: Pan | Ctrl+C: Copy to Slot | Ctrl+S: Save As | 0: Fit | Esc: Close",
                    };


                    ui.label(
                        egui::RichText::new(hint_text)
                            .small()
                            .color(hex_to_c32(&palette.theme.f_low)),
                    );

                    // Toast message
                    ui.add_space(20.0);
                    crate::ui_ext::StatusToast::render_opt(&self.status_toast, 5.0, ui, palette, now);
                });
            });
    }

    // ─── Film Simulation Methods ───────────────────────────────────────────────────

    /// Spawn a background thread that runs filmr's process_image() on the current
    /// image with the current FilmSimState parameters, then sends the result back.
    pub fn run_film_sim_render(&mut self) {
        if self.film_sim_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }
        let stocks = all_film_stocks();
        let idx = self.film_sim_state.selected_stock_idx.min(stocks.len().saturating_sub(1));
        let entry = &stocks[idx];
        let mut stock = (entry.build)().with_style(self.film_sim_state.style);
        stock.halation_strength = self.film_sim_state.halation;
        stock.vignette_strength = self.film_sim_state.vignette;

        let config = SimulationConfig {
            exposure_time: self.film_sim_state.brightness,
            warmth: self.film_sim_state.warmth,
            saturation: self.film_sim_state.intensity,
            auto_levels: false,
            white_balance_mode: self.film_sim_state.wb_mode,
            white_balance_strength: self.film_sim_state.wb_strength,
            output_mode: OutputMode::Positive,
            enable_grain: self.film_sim_state.enable_grain,
            light_leak: self.film_sim_state.light_leak.to_config(),
            ..SimulationConfig::default()
        };

        // Convert RgbaImage → RgbImage for filmr
        let rgba = self.current_rgba.clone();
        let w = rgba.width();
        let h = rgba.height();
        let rgb_pixels: Vec<u8> = rgba
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let rgb_img = image::RgbImage::from_raw(w, h, rgb_pixels)
            .expect("pixel buffer size mismatch");

        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimMsg>();
        self.film_sim_state.render_rx = Some(rx);
        self.film_sim_state.is_rendering = true;
        self.film_sim_state.render_error = None;

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                filmr::processor::process_image(&rgb_img, &stock, &config)
            }));
            match process_result {
                Ok(out_rgb) => {
                    let out_rgba = apply_film_sim_post_adjustments(&out_rgb, contrast, tint);
                    let duration_ms = start.elapsed().as_millis();
                    let _ = tx.send(FilmSimMsg::Done { rgba: out_rgba, duration_ms });
                }
                Err(e) => {
                    let err_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Film simulation panicked".to_string()
                    };
                    let _ = tx.send(FilmSimMsg::Error(err_msg));
                }
            }
        });
    }

    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all film stocks.
    pub fn run_thumbnail_batch(&mut self) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.film_sim_state.thumbnail_epoch += 1;
        let epoch = self.film_sim_state.thumbnail_epoch;

        // Downscale base image to 42x28 (sub-3ms per stock)
        let thumb_base = image::imageops::resize(
            &self.current_rgba,
            42,
            28,
            image::imageops::FilterType::Nearest,
        );
        let rgb_pixels: Vec<u8> = thumb_base.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let thumb_rgb = match image::RgbImage::from_raw(42, 28, rgb_pixels) {
            Some(img) => img,
            None => return,
        };

        let brightness = self.film_sim_state.brightness;
        let warmth = self.film_sim_state.warmth;
        let intensity = self.film_sim_state.intensity;
        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;
        let wb_mode = self.film_sim_state.wb_mode;
        let wb_strength = self.film_sim_state.wb_strength;
        let halation = self.film_sim_state.halation;
        let vignette = self.film_sim_state.vignette;
        let light_leak = self.film_sim_state.light_leak;
        let style = self.film_sim_state.style;
        let enable_grain = self.film_sim_state.enable_grain;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimThumbnailMsg>();
        self.film_sim_state.thumbnail_rx = Some(rx);
        self.film_sim_state.is_generating_thumbnails = true;

        std::thread::Builder::new()
            .name("film_sim_thumbnails".to_string())
            .spawn(move || {
                let stocks = all_film_stocks();
                for (idx, entry) in stocks.iter().enumerate() {
                    let mut stock = (entry.build)().with_style(style);
                    stock.halation_strength = halation;
                    stock.vignette_strength = vignette;

                    let config = SimulationConfig {
                        exposure_time: brightness,
                        warmth,
                        saturation: intensity,
                        auto_levels: false,
                        white_balance_mode: wb_mode,
                        white_balance_strength: wb_strength,
                        output_mode: OutputMode::Positive,
                        enable_grain,
                        light_leak: light_leak.to_config(),
                        ..SimulationConfig::default()
                    };

                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        filmr::processor::process_image(&thumb_rgb, &stock, &config)
                    }));

                    if let Ok(out_rgb) = res {
                        let out_rgba = apply_film_sim_post_adjustments(&out_rgb, contrast, tint);
                        if tx.send(FilmSimThumbnailMsg { epoch, stock_idx: idx, rgba: out_rgba }).is_err() {
                            // Receiver dropped (new epoch started), terminate worker
                            break;
                        }
                    }
                }
            })
            .ok();
    }

    /// Drain any messages from the film sim background thread and thumbnail worker.
    pub fn poll_film_sim(&mut self) {
        if let Some(rx) = self.film_sim_state.render_rx.take() {
            match rx.try_recv() {
                Ok(FilmSimMsg::Done { rgba, duration_ms }) => {
                    self.film_sim_state.preview_rgba = Some(rgba);
                    self.film_sim_state.dirty_preview_texture = true;
                    self.film_sim_state.last_render_duration_ms = duration_ms;
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_error = None;
                }
                Ok(FilmSimMsg::Error(e)) => {
                    self.film_sim_state.render_error = Some(e);
                    self.film_sim_state.is_rendering = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.film_sim_state.render_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.film_sim_state.is_rendering = false;
                }
            }
        }

        // Drain pending thumbnail messages
        let mut thumbs_done = false;
        if let Some(rx) = self.film_sim_state.thumbnail_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        if msg.epoch == self.film_sim_state.thumbnail_epoch {
                            self.film_sim_state.pending_thumbnails.push((msg.stock_idx, msg.rgba));
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        keep = false;
                        thumbs_done = true;
                        break;
                    }
                }
            }
            if keep {
                self.film_sim_state.thumbnail_rx = Some(rx);
            }
        }
        if thumbs_done {
            self.film_sim_state.is_generating_thumbnails = false;
        }
    }

    /// Commit the film sim preview into current_rgba and push to undo stack.
    pub fn apply_film_sim(&mut self) {
        let preview = match self.film_sim_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = preview;
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;
        // Reset sim state so re-entering the tab renders fresh.
        self.film_sim_state.preview_rgba = None;
        self.film_sim_state.preview_texture = None;
        self.film_sim_state.dirty_preview_texture = false;
        // Invalidate and regenerate thumbnails for updated image
        self.film_sim_state.stock_thumbnails.clear();
        self.film_sim_state.pending_thumbnails.clear();
        self.run_thumbnail_batch();
        self.show_toast("🎞 Film simulation applied");
    }

    /// Full 3-panel film simulation viewport (replaces the normal render_viewport when FilmSim is active).
    fn render_film_sim_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let b_low   = hex_to_c32(&palette.theme.b_low);
        let b_med   = hex_to_c32(&palette.theme.b_med);
        let base_c  = hex_to_c32(&palette.harmonies.base);
        let f_high  = hex_to_c32(&palette.theme.f_high);
        let f_med   = hex_to_c32(&palette.theme.f_med);
        let f_low   = hex_to_c32(&palette.theme.f_low);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);

        // Upload any newly received thumbnails into GPU textures
        for (idx, rgba) in self.film_sim_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("film_sim_thumb_{}", idx),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.film_sim_state.stock_thumbnails.insert(idx, tex);
        }

        // Kick off thumbnail generation if empty
        if self.film_sim_state.stock_thumbnails.is_empty()
            && !self.film_sim_state.is_generating_thumbnails
            && !self.current_rgba.is_empty()
        {
            self.run_thumbnail_batch();
        }

        let avail = ui.available_size();
        let bottom_h = 96.0_f32;
        let main_h = (avail.y - bottom_h - 10.0).max(100.0);
        let left_w   = 235.0_f32;
        let right_w  = 210.0_f32;
        let center_w = (avail.x - left_w - right_w - 16.0).max(100.0);

        let stocks = all_film_stocks();

        // Ensure fit-to-view is performed on the center canvas when first entering FilmSim
        if !self.film_sim_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.film_sim_state.auto_fitted = true;
        }

        // ── TOP ROW: LEFT PANEL + CENTER CANVAS + RIGHT PANEL ─────────────────────
        ui.horizontal(|ui| {
            ui.set_height(main_h);

            // ── 1. LEFT PANEL: Film Stock Picker with Async 42x28 Mini-Photos ──
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
                                    egui::RichText::new("🎞 Film Stock")
                                        .strong()
                                        .small()
                                        .color(base_c),
                                );
                                ui.label(
                                    egui::RichText::new(format!("({})", stocks.len()))
                                        .small()
                                        .color(f_low),
                                );
                                if self.film_sim_state.is_generating_thumbnails {
                                    ui.label(
                                        egui::RichText::new("⏳")
                                            .small()
                                            .color(f_low),
                                    ).on_hover_text("Generating thumbnails in background…");
                                }
                            });
                            ui.add_space(4.0);

                            // Search box
                            ui.add(
                                egui::TextEdit::singleline(&mut self.film_sim_state.search_query)
                                    .hint_text("🔍 Filter stocks...")
                                    .desired_width(left_w - 24.0),
                            );
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(2.0);

                            let search = self.film_sim_state.search_query.trim().to_lowercase();
                            egui::ScrollArea::vertical()
                                .id_source("film_sim_stock_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(left_w - 24.0);
                                    let mut current_brand = "";
                                    for (i, entry) in stocks.iter().enumerate() {
                                        if !search.is_empty() {
                                            let matches = entry.name.to_lowercase().contains(&search)
                                                || entry.brand.to_lowercase().contains(&search)
                                                || entry.film_type_label.to_lowercase().contains(&search);
                                            if !matches {
                                                continue;
                                            }
                                        }

                                        // Brand separator heading
                                        if entry.brand != current_brand {
                                            if !current_brand.is_empty() {
                                                ui.add_space(6.0);
                                            }
                                            ui.label(
                                                egui::RichText::new(entry.brand.to_uppercase())
                                                    .small()
                                                    .strong()
                                                    .color(base_c),
                                            );
                                            current_brand = entry.brand;
                                        }

                                        let selected = self.film_sim_state.selected_stock_idx == i;
                                        let (row_bg, name_col, meta_col) = if selected {
                                            (base_c, contrast_text_c32(base_c, None, palette), contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_low, f_high, f_med)
                                        };

                                        let row_frame = egui::Frame::none()
                                            .fill(row_bg)
                                            .rounding(Rounding::same(4.0))
                                            .inner_margin(egui::Margin::symmetric(4.0, 3.0));

                                        let resp = row_frame.show(ui, |ui| {
                                            ui.set_width(left_w - 32.0);
                                            ui.horizontal(|ui| {
                                                // 42x28 mini-photo thumbnail preview
                                                if let Some(tex) = self.film_sim_state.stock_thumbnails.get(&i) {
                                                    ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                                } else {
                                                    let (rect, _) = ui.allocate_exact_size(Vec2::new(42.0, 28.0), egui::Sense::hover());
                                                    ui.painter().rect_filled(rect, Rounding::same(2.0), b_med);
                                                }
                                                ui.add_space(2.0);
                                                ui.vertical(|ui| {
                                                    ui.label(egui::RichText::new(entry.name).small().strong().color(name_col));
                                                    ui.horizontal(|ui| {
                                                        ui.label(egui::RichText::new(entry.film_type_label).monospace().color(meta_col).size(9.0));
                                                        ui.label(egui::RichText::new(format!("ISO {}", entry.iso)).monospace().color(meta_col).size(9.0));
                                                    });
                                                });
                                            });
                                        }).response;

                                        if resp.interact(egui::Sense::click()).clicked() && !selected {
                                            self.film_sim_state.selected_stock_idx = i;
                                            self.film_sim_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);
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
                            if self.film_sim_state.dirty_preview_texture {
                                if let Some(ref rgba) = self.film_sim_state.preview_rgba {
                                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                                        [rgba.width() as usize, rgba.height() as usize],
                                        rgba.as_raw(),
                                    );
                                    self.film_sim_state.preview_texture = Some(
                                        ctx.load_texture("film_sim_preview_tex", ci, egui::TextureOptions::LINEAR),
                                    );
                                }
                                self.film_sim_state.dirty_preview_texture = false;
                            }

                            let (response, painter) = ui.allocate_painter(
                                Vec2::new(center_w, main_h),
                                egui::Sense::click_and_drag(),
                            );
                            let canvas_rect = response.rect;

                            painter.rect_filled(canvas_rect, Rounding::same(4.0), hex_to_c32(&palette.theme.background));

                            // Pan/zoom via this response
                            let middle_drag = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                            if (!self.film_sim_state.is_dragging_split && response.dragged()) || middle_drag {
                                self.pan += response.drag_delta();
                            }
                            if response.hovered() {
                                let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                                if scroll_delta.abs() > 0.0 {
                                    let zoom_factor = (scroll_delta * 0.002).exp();
                                    let old_zoom = self.zoom;
                                    let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 50.0);
                                    if let Some(ptr_pos) = response.hover_pos() {
                                        let center = canvas_rect.center() + self.pan;
                                        let to_ptr = ptr_pos - center;
                                        self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                                    }
                                    self.zoom = new_zoom;
                                }
                            }
                            if !self.film_sim_state.is_dragging_split && response.double_clicked() {
                                let avail_size = Vec2::new(center_w, main_h);
                                self.fit_to_view(avail_size);
                            }

                            if let Some(orig_tex) = self.texture.clone() {
                                let img_w = self.current_rgba.width() as f32 * self.zoom;
                                let img_h = self.current_rgba.height() as f32 * self.zoom;
                                let center_pt = canvas_rect.center() + self.pan;
                                let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                                draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                                self.render_film_sim_comparison(
                                    ui, &painter, &response, img_rect, canvas_rect, ctx, &orig_tex, palette
                                );

                                painter.rect_stroke(
                                    img_rect,
                                    Rounding::ZERO,
                                    Stroke::new(1.0, hex_to_c32_alpha(&palette.theme.f_med, 40)),
                                );
                            }

                            // Rendering spinner overlay
                            if self.film_sim_state.is_rendering {
                                let spinner_pos = canvas_rect.center();
                                let spin_bg = hex_to_c32_alpha(&palette.theme.b_low, 210);
                                let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                                painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                                painter.rect_stroke(spin_rect, Rounding::same(8.0), Stroke::new(1.0, b_med));
                                painter.text(
                                    spinner_pos,
                                    egui::Align2::CENTER_CENTER,
                                    "⏳ Rendering film simulation…",
                                    egui::FontId::proportional(12.0),
                                    f_med,
                                );
                            }

                            // Error overlay
                            if let Some(ref err) = self.film_sim_state.render_error.clone() {
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

            // ── 3. RIGHT PANEL: Style + Optical Effects + Stock Info ──
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
                                .id_source("film_sim_right_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(right_w - 24.0);

                                    // 1. Style
                                    ui.label(egui::RichText::new("✨ Style").strong().small().color(base_c));
                                    ui.add_space(4.0);

                                    let styles = [
                                        (FilmStyle::Accurate,     "Accurate",  "Physics-faithful rendering"),
                                        (FilmStyle::Artistic,     "Artistic",  "Boosted contrast & saturation"),
                                        (FilmStyle::Vintage,      "Vintage",   "Warm, faded, nostalgic"),
                                        (FilmStyle::HighContrast, "Hi-Con",    "High contrast, punchy"),
                                        (FilmStyle::Pastel,       "Pastel",    "Low-contrast, soft tones"),
                                    ];

                                    for (style, label, tooltip) in &styles {
                                        let sel = self.film_sim_state.style == *style;
                                        let (s_bg, s_fg) = if sel {
                                            (base_c, contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_med, f_med)
                                        };
                                        let btn = egui::Button::new(
                                            egui::RichText::new(*label).small().strong().color(s_fg),
                                        )
                                        .fill(s_bg)
                                        .stroke(Stroke::new(1.0, if sel { base_c } else { b_med }))
                                        .rounding(Rounding::same(4.0))
                                        .min_size(Vec2::new(right_w - 28.0, 20.0));

                                        if ui.add(btn).on_hover_text(*tooltip).clicked() && !sel {
                                            self.film_sim_state.style = *style;
                                            self.film_sim_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);
                                    }

                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.add_space(4.0);

                                    // 2. Optical Effects: Halation, Vignette, Light Leak, Grain
                                    ui.label(egui::RichText::new("📸 Optics & Effects").strong().small().color(base_c));
                                    ui.add_space(4.0);

                                    let full_w = (right_w - 32.0).max(100.0);
                                    let opt_slider_w = (full_w - 68.0).max(60.0);

                                    if render_context_slider(
                                        ui,
                                        "🌟 Halation",
                                        &mut self.film_sim_state.halation,
                                        0.0..=1.0,
                                        0.25,
                                        "%",
                                        opt_slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(4.0);

                                    if render_context_slider(
                                        ui,
                                        "🔘 Vignette",
                                        &mut self.film_sim_state.vignette,
                                        0.0..=1.0,
                                        0.35,
                                        "%",
                                        opt_slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(4.0);

                                    ui.label(egui::RichText::new("💡 Light Leak").small().color(f_med));
                                    egui::ComboBox::from_id_source("film_sim_light_leak_combo")
                                        .selected_text(self.film_sim_state.light_leak.label())
                                        .width(full_w)
                                        .show_ui(ui, |ui| {
                                            let presets = [
                                                LightLeakPreset::None,
                                                LightLeakPreset::WarmCorner,
                                                LightLeakPreset::SunStreak,
                                                LightLeakPreset::SoftEdge,
                                                LightLeakPreset::OrganicBlob,
                                            ];
                                            for p in presets {
                                                let sel = self.film_sim_state.light_leak == p;
                                                if ui.selectable_label(sel, p.label()).clicked() && !sel {
                                                    self.film_sim_state.light_leak = p;
                                                    self.film_sim_state.mark_dirty();
                                                }
                                            }
                                        });
                                    ui.add_space(4.0);

                                    let mut grain_on = self.film_sim_state.enable_grain;
                                    if ui.checkbox(&mut grain_on, egui::RichText::new("⟁ Grain emulation").small().color(f_med)).changed() {
                                        self.film_sim_state.enable_grain = grain_on;
                                        self.film_sim_state.mark_dirty();
                                    }

                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.add_space(4.0);

                                    // 3. Stock metadata
                                    let idx = self.film_sim_state.selected_stock_idx.min(stocks.len().saturating_sub(1));
                                    let entry = &stocks[idx];
                                    ui.label(egui::RichText::new("📋 Stock Info").strong().small().color(base_c));
                                    ui.add_space(4.0);

                                    let stock_built = (entry.build)();
                                    egui::Grid::new("film_sim_stock_meta_grid")
                                        .spacing(Vec2::new(6.0, 3.0))
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new("Brand:").small().color(f_med));
                                            ui.label(egui::RichText::new(entry.brand).small().color(f_high));
                                            ui.end_row();

                                            ui.label(egui::RichText::new("Film:").small().color(f_med));
                                            ui.label(egui::RichText::new(entry.name).small().color(f_high));
                                            ui.end_row();

                                            ui.label(egui::RichText::new("ISO:").small().color(f_med));
                                            ui.label(egui::RichText::new(format!("{}", entry.iso)).small().color(f_high));
                                            ui.end_row();

                                            ui.label(egui::RichText::new("Type:").small().color(f_med));
                                            ui.label(egui::RichText::new(entry.film_type_label).small().color(f_high));
                                            ui.end_row();

                                            ui.label(egui::RichText::new("Res:").small().color(f_med));
                                            ui.label(egui::RichText::new(format!("{:.0} lp/mm", stock_built.resolution_lp_mm)).small().color(f_low));
                                            ui.end_row();
                                        });

                                    if self.film_sim_state.last_render_duration_ms > 0 {
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(
                                            format!("⏱ {:.2}s", self.film_sim_state.last_render_duration_ms as f64 / 1000.0)
                                        ).small().color(f_low));
                                    }
                                });
                        });
                },
            );
        });

        // ── BOTTOM STRIP: Context Sliders + WB Controls + Action Buttons ───────────
        ui.add_space(4.0);
        egui::Frame::none()
            .fill(b_low)
            .stroke(Stroke::new(1.0, b_med))
            .rounding(Rounding::same(4.0))
            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(avail.x - 24.0);
                ui.vertical(|ui| {
                    // Row 1: Exposure & Color Sliders with contextual start/center/end/default tracks
                    ui.horizontal(|ui| {
                        let slider_w = ((avail.x - 48.0) / 6.0 - 12.0).clamp(70.0, 115.0);

                        // Brightness
                        if render_context_slider(ui, "☀ Brightness", &mut self.film_sim_state.brightness, 0.1..=4.0, 1.0, "×", slider_w, palette) {
                            self.film_sim_state.mark_dirty();
                        }
                        ui.add_space(3.0);

                        // Contrast
                        if render_context_slider(ui, "◑ Contrast", &mut self.film_sim_state.contrast, -1.0..=1.0, 0.0, "", slider_w, palette) {
                            self.film_sim_state.mark_dirty();
                        }
                        ui.add_space(3.0);

                        // Warmth
                        if render_context_slider(ui, "🌡 Warmth", &mut self.film_sim_state.warmth, -1.0..=1.0, 0.0, "", slider_w, palette) {
                            self.film_sim_state.mark_dirty();
                        }
                        ui.add_space(3.0);

                        // Tint (Green ↔ Magenta)
                        if render_context_slider(ui, "🧪 Tint", &mut self.film_sim_state.tint, -1.0..=1.0, 0.0, "", slider_w, palette) {
                            self.film_sim_state.mark_dirty();
                        }
                        ui.add_space(3.0);

                        // Saturation
                        if render_context_slider(ui, "🎨 Saturation", &mut self.film_sim_state.intensity, 0.0..=2.0, 1.0, "", slider_w, palette) {
                            self.film_sim_state.mark_dirty();
                        }
                        ui.add_space(3.0);

                        // WB Strength
                        if render_context_slider(ui, "⚖ WB Str", &mut self.film_sim_state.wb_strength, 0.0..=1.0, 1.0, "%", slider_w, palette) {
                            self.film_sim_state.mark_dirty();
                        }
                    });

                    ui.add_space(3.0);

                    // Row 2: WB Mode Selector + Auto Presets + Actions
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("WB:").small().strong().color(base_c));
                        let wb_modes = [
                            (WhiteBalanceMode::Auto,  "Auto"),
                            (WhiteBalanceMode::Gray,  "Gray"),
                            (WhiteBalanceMode::White, "White"),
                            (WhiteBalanceMode::Off,   "Off"),
                        ];
                        for (mode, label) in wb_modes {
                            let sel = self.film_sim_state.wb_mode == mode;
                            let (btn_bg, btn_fg) = if sel {
                                (base_c, contrast_text_c32(base_c, None, palette))
                            } else {
                                (b_med, f_med)
                            };
                            let b = egui::Button::new(egui::RichText::new(label).small().strong().color(btn_fg))
                                .fill(btn_bg)
                                .stroke(Stroke::new(1.0, if sel { base_c } else { b_low }))
                                .rounding(Rounding::same(3.0))
                                .min_size(Vec2::new(36.0, 18.0));
                            if ui.add(b).clicked() && !sel {
                                self.film_sim_state.wb_mode = mode;
                                self.film_sim_state.mark_dirty();
                            }
                        }

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // Auto Levels
                        let al_fg = contrast_text_c32(diff_mod, Some(f_high), palette);
                        let al_btn = egui::Button::new(egui::RichText::new("⚡ Auto Levels").small().strong().color(al_fg))
                            .fill(diff_mod)
                            .stroke(Stroke::new(1.0, diff_mod))
                            .rounding(Rounding::same(4.0));
                        if ui.add(al_btn).on_hover_text("Reset brightness to neutral, warmth to 0, and tint to 0").clicked() {
                            self.film_sim_state.brightness = 1.0;
                            self.film_sim_state.warmth = 0.0;
                            self.film_sim_state.tint = 0.0;
                            self.film_sim_state.mark_dirty();
                            self.show_toast("⚡ Auto levels applied");
                        }

                        ui.add_space(4.0);

                        // Auto Enhance
                        let ae_fg = contrast_text_c32(base_c, Some(f_high), palette);
                        let ae_btn = egui::Button::new(egui::RichText::new("✨ Auto Enhance").small().strong().color(ae_fg))
                            .fill(base_c)
                            .stroke(Stroke::new(1.0, base_c))
                            .rounding(Rounding::same(4.0));
                        if ui.add(ae_btn).on_hover_text("Boost intensity, warmth, and contrast for a cinematic look").clicked() {
                            self.film_sim_state.brightness = (self.film_sim_state.brightness * 1.05).min(4.0);
                            self.film_sim_state.intensity  = (self.film_sim_state.intensity  + 0.15).min(2.0);
                            self.film_sim_state.warmth     = (self.film_sim_state.warmth     + 0.05).min(1.0);
                            self.film_sim_state.contrast   = (self.film_sim_state.contrast   + 0.08).min(1.0);
                            self.film_sim_state.mark_dirty();
                            self.show_toast("✨ Auto enhance applied");
                        }

                        // Right aligned: Apply, Reset, Undo
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let apply_fg = contrast_text_c32(diff_add, Some(f_high), palette);
                            let apply_btn = egui::Button::new(
                                egui::RichText::new("✓ Apply to Image").small().strong().color(apply_fg),
                            )
                            .fill(diff_add)
                            .stroke(Stroke::new(1.0, diff_add))
                            .rounding(Rounding::same(4.0));
                            let apply_enabled = self.film_sim_state.preview_rgba.is_some();
                            if ui.add_enabled(apply_enabled, apply_btn)
                                .on_hover_text("Commit film simulation result to image (adds to undo stack)")
                                .clicked()
                            {
                                self.apply_film_sim();
                            }

                            ui.add_space(4.0);

                            let reset_fg = contrast_text_c32(b_med, Some(f_med), palette);
                            let reset_btn = egui::Button::new(egui::RichText::new("⟲ Reset").small().strong().color(reset_fg))
                                .fill(b_med)
                                .stroke(Stroke::new(1.0, b_low))
                                .rounding(Rounding::same(4.0));
                            if ui.add(reset_btn).on_hover_text("Reset all film sim parameters to defaults").clicked() {
                                self.film_sim_state.brightness = 1.0;
                                self.film_sim_state.contrast   = 0.0;
                                self.film_sim_state.warmth     = 0.0;
                                self.film_sim_state.tint       = 0.0;
                                self.film_sim_state.intensity  = 1.0;
                                self.film_sim_state.wb_mode    = WhiteBalanceMode::Auto;
                                self.film_sim_state.wb_strength = 1.0;
                                self.film_sim_state.halation   = 0.25;
                                self.film_sim_state.vignette   = 0.35;
                                self.film_sim_state.light_leak = LightLeakPreset::None;
                                self.film_sim_state.enable_grain = true;
                                self.film_sim_state.style = FilmStyle::Accurate;
                                self.film_sim_state.mark_dirty();
                                self.show_toast("⟲ All parameters reset to defaults");
                            }

                            ui.add_space(4.0);

                            if !self.undo_stack.is_empty() {
                                let undo_fg = contrast_text_c32(b_med, Some(diff_mod), palette);
                                let undo_btn = egui::Button::new(
                                    egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                                )
                                .fill(b_med)
                                .rounding(Rounding::same(4.0));
                                if ui.add(undo_btn).clicked() {
                                    self.undo();
                                    self.film_sim_state.preview_rgba = None;
                                    self.film_sim_state.preview_texture = None;
                                    self.film_sim_state.mark_dirty();
                                }
                            }
                        });
                    });
                });
            });
    }

    /// Split-view comparison painter for the Film Sim tab center canvas.
    #[allow(clippy::too_many_arguments)]
    fn render_film_sim_comparison(
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
        let b_low  = hex_to_c32(&palette.theme.b_low);
        let b_med  = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // If no preview yet, just show original
        let preview_tex_id = self.film_sim_state.preview_texture.as_ref().map(|t| t.id());

        let split = self.film_sim_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // Left side: original
        {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv   = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut mesh  = egui::Mesh::with_texture(orig_tex.id());
            mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        }

        // Right side: film simulation preview (or original if not ready)
        {
            let tex_id = preview_tex_id.unwrap_or_else(|| orig_tex.id());
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv   = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut mesh   = egui::Mesh::with_texture(tex_id);
            mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        }

        // Divider line
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(3.0, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(1.5, Color32::WHITE),
        );

        // Draggable knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
        painter.circle(knob_center, 16.0, b_low, Stroke::new(2.0, base_c));
        painter.text(
            knob_center,
            egui::Align2::CENTER_CENTER,
            "◀ ▶",
            egui::FontId::monospace(10.0),
            knob_fg,
        );

        // Drag logic
        let ptr_pos = response.hover_pos();
        let near_divider = ptr_pos.map_or(false, |pos| {
            (pos.x - split_x).abs() <= 16.0 && pos.y >= img_rect.min.y && pos.y <= img_rect.max.y
        });

        if near_divider || self.film_sim_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if near_divider && response.drag_started() {
            self.film_sim_state.is_dragging_split = true;
        }
        if self.film_sim_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    self.film_sim_state.comparison_split =
                        ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                }
            }
            if response.drag_stopped() {
                self.film_sim_state.is_dragging_split = false;
            }
        }

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

        let stocks = all_film_stocks();
        let idx = self.film_sim_state.selected_stock_idx.min(stocks.len().saturating_sub(1));
        let sim_label = if self.film_sim_state.is_rendering {
            "⏳ Rendering…".to_string()
        } else {
            format!("🎞 {} ({:.1}s)", stocks[idx].name, self.film_sim_state.last_render_duration_ms as f64 / 1000.0)
        };
        let sim_badge_w = (sim_label.len() as f32 * 6.5).max(130.0);
        let sim_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - sim_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(sim_badge_w, 22.0),
        );
        painter.rect_filled(sim_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(sim_rect, Rounding::same(4.0), Stroke::new(1.0, base_c));
        painter.text(
            sim_rect.center(),
            egui::Align2::CENTER_CENTER,
            sim_label,
            egui::FontId::proportional(10.0),
            base_c,
        );
    }

    // ─── Creative FX Methods ─────────────────────────────────────────────────────

    /// Spawn a background thread that runs the currently selected creative effect
    /// with the current parameters and sends the result back.
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
                        if tx.send(CreativeFxThumbnailMsg {
                            epoch,
                            fx_type: *fx,
                            rgba: out_rgba,
                        }).is_err() {
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
                Ok(CreativeFxMsg::Done { fx_type, rgba, duration_ms }) => {
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
                    self.creative_fx_state.pending_thumbnails.push((msg.fx_type, msg.rgba));
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
    fn render_creative_fx_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let b_low    = hex_to_c32(&palette.theme.b_low);
        let b_med    = hex_to_c32(&palette.theme.b_med);
        let base_c   = hex_to_c32(&palette.harmonies.base);
        let f_high   = hex_to_c32(&palette.theme.f_high);
        let f_med    = hex_to_c32(&palette.theme.f_med);
        let f_low    = hex_to_c32(&palette.theme.f_low);
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
        let main_h = (avail.y - 10.0).max(100.0);
        let left_w   = 230.0_f32;
        let right_w  = 295.0_f32;
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
                                    let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 50.0);
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
    fn render_creative_fx_comparison(
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
        let b_low  = hex_to_c32(&palette.theme.b_low);
        let b_med  = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);

        let preview_tex_id = self.creative_fx_state.preview_texture.as_ref().map(|t| t.id());
        let split = self.creative_fx_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // Left side: original
        {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv   = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut mesh  = egui::Mesh::with_texture(orig_tex.id());
            mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        }

        // Right side: creative FX preview (or original if not ready)
        {
            let tex_id = preview_tex_id.unwrap_or_else(|| orig_tex.id());
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv   = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut mesh   = egui::Mesh::with_texture(tex_id);
            mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        }

        // Divider line
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(3.0, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [Pos2::new(split_x, img_rect.min.y), Pos2::new(split_x, img_rect.max.y)],
            Stroke::new(1.5, Color32::WHITE),
        );

        // Draggable knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_fg = contrast_text_c32(b_low, Some(hex_to_c32(&palette.theme.f_high)), palette);
        painter.circle(knob_center, 16.0, b_low, Stroke::new(2.0, base_c));
        painter.text(
            knob_center,
            egui::Align2::CENTER_CENTER,
            "◀ ▶",
            egui::FontId::monospace(10.0),
            knob_fg,
        );

        // Drag interaction
        let ptr_pos = response.hover_pos();
        let near_divider = ptr_pos.map_or(false, |pos| {
            (pos.x - split_x).abs() <= 16.0 && pos.y >= img_rect.min.y && pos.y <= img_rect.max.y
        });

        if near_divider || self.creative_fx_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if near_divider && response.drag_started() {
            self.creative_fx_state.is_dragging_split = true;
        }
        if self.creative_fx_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    self.creative_fx_state.comparison_split =
                        ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                }
            }
            if response.drag_stopped() {
                self.creative_fx_state.is_dragging_split = false;
            }
        }

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
            format!("{} {} ({:.1}s)", cur_fx.icon(), cur_fx.name(), self.creative_fx_state.last_render_duration_ms as f64 / 1000.0)
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

fn chip_badge(ui: &mut egui::Ui, text: &str, text_col: Color32, bg_col: Color32) {
    let frame = egui::Frame::none()
        .fill(bg_col)
        .rounding(Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0));
    frame.show(ui, |ui| {
        ui.label(egui::RichText::new(text).small().strong().color(text_col));
    });
}

fn draw_checkerboard(painter: &egui::Painter, rect: Rect, _zoom: f32, dark_col: Color32, light_col: Color32) {
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
    encoder.encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
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

fn run_optipng_optimization(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut temp_file = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("Tempfile creation failed: {}", e))?;
    temp_file.write_all(png_bytes)
        .map_err(|e| format!("Tempfile write failed: {}", e))?;
    let temp_path = temp_file.path().to_path_buf();

    let status = Command::new("optipng")
        .arg("-o2")
        .arg("-quiet")
        .arg(&temp_path)
        .status()
        .map_err(|e| format!("Failed to run optipng: {}. Ensure optipng is installed.", e))?;

    if !status.success() {
        return Err(format!("optipng failed with exit status {:?}", status.code()));
    }

    std::fs::read(&temp_path).map_err(|e| format!("Failed to read optipng output: {}", e))
}


#[allow(dead_code)]
pub fn optimize_to_target_size(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
) -> FileSizeOptimizationResult {
    optimize_to_target_size_with_progress(base_img, target_bytes, format, None)
}

pub fn optimize_to_target_size_with_progress(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
    progress: Option<&dyn Fn(String)>,
) -> FileSizeOptimizationResult {
    let orig_w = base_img.width();
    let orig_h = base_img.height();
    let target_str = format_bytes_size(target_bytes);

    let notify = |msg: String| {
        if let Some(cb) = progress {
            cb(msg);
        }
    };

    match format {
        TargetFormat::Jpeg => {
            notify(format!("Testing full resolution ({}×{}) at JPEG quality 95%...", orig_w, orig_h));
            // 1. Try quality 95 at 100% scale
            if let Ok(bytes) = encode_jpeg_with_quality(base_img, 95) {
                if bytes.len() <= target_bytes {
                    notify(format!(
                        "Full resolution fits budget: {} <= {}",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                    return FileSizeOptimizationResult {
                        target_bytes,
                        achieved_bytes: bytes.len(),
                        width: orig_w,
                        height: orig_h,
                        scale: 1.0,
                        quality: 95,
                        format,
                        encoded_data: bytes,
                        result_rgba: base_img.clone(),
                    };
                } else {
                    notify(format!(
                        "Quality 95% is {} (budget: {}). Searching JPEG quality [60..95]...",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                }
            }

            // 2. Binary search quality in [60, 95] at scale 1.0
            let mut best_qual_at_100 = None;
            let mut low_q = 60u8;
            let mut high_q = 95u8;

            while low_q <= high_q {
                let mid_q = low_q + (high_q - low_q) / 2;
                notify(format!("Trying quality {}% at 100% resolution ({}×{})...", mid_q, orig_w, orig_h));
                if let Ok(bytes) = encode_jpeg_with_quality(base_img, mid_q) {
                    let len = bytes.len();
                    if len <= target_bytes {
                        notify(format!("Quality {}% fits budget: {} <= {}", mid_q, format_bytes_size(len), target_str));
                        best_qual_at_100 = Some((mid_q, bytes));
                        low_q = mid_q + 1; // Try higher quality
                    } else {
                        notify(format!("Quality {}% exceeds budget: {} > {}", mid_q, format_bytes_size(len), target_str));
                        if mid_q == 0 {
                            break;
                        }
                        high_q = mid_q - 1; // Try lower quality
                    }
                } else {
                    break;
                }
            }

            if let Some((q, bytes)) = best_qual_at_100 {
                notify(format!(
                    "Found optimal quality {}% at 100% resolution ({})",
                    q,
                    format_bytes_size(bytes.len())
                ));
                return FileSizeOptimizationResult {
                    target_bytes,
                    achieved_bytes: bytes.len(),
                    width: orig_w,
                    height: orig_h,
                    scale: 1.0,
                    quality: q,
                    format,
                    encoded_data: bytes,
                    result_rgba: base_img.clone(),
                };
            }

            // 3. At scale 1.0, even quality 60 exceeds target_bytes!
            notify(format!(
                "Exceeds budget at 100% resolution. Scaling down with Lanczos3 filter..."
            ));
            let mut best_scale = 0.5f32;
            let mut best_scaled_img = base_img.clone();
            let mut best_bytes = Vec::new();
            let mut best_w = orig_w;
            let mut best_h = orig_h;
            let mut best_q = 80u8;

            let mut low_s = 0.05f32;
            let mut high_s = 1.0f32;

            for iter in 1..=10 {
                let mid_s = (low_s + high_s) / 2.0;
                let candidate_w = ((orig_w as f32) * mid_s).round().max(16.0) as u32;
                let candidate_h = ((orig_h as f32) * mid_s).round().max(16.0) as u32;

                notify(format!(
                    "[Step {}/10] Resampling to {:.1}% ({}×{}) with Lanczos3...",
                    iter,
                    mid_s * 100.0,
                    candidate_w,
                    candidate_h
                ));

                let scaled = image::imageops::resize(
                    base_img,
                    candidate_w,
                    candidate_h,
                    image::imageops::FilterType::Lanczos3,
                );

                if let Ok(bytes) = encode_jpeg_with_quality(&scaled, 80) {
                    let len = bytes.len();
                    if len <= target_bytes {
                        notify(format!(
                            "Scale {:.1}% ({}×{}) fits: {} <= {}",
                            mid_s * 100.0,
                            candidate_w,
                            candidate_h,
                            format_bytes_size(len),
                            target_str
                        ));
                        best_scale = mid_s;
                        best_w = candidate_w;
                        best_h = candidate_h;
                        best_bytes = bytes;
                        best_scaled_img = scaled;
                        best_q = 80;
                        low_s = mid_s;
                    } else {
                        notify(format!(
                            "Scale {:.1}% ({}×{}) exceeds: {} > {}",
                            mid_s * 100.0,
                            candidate_w,
                            candidate_h,
                            format_bytes_size(len),
                            target_str
                        ));
                        high_s = mid_s;
                    }
                } else {
                    break;
                }
            }

            if !best_bytes.is_empty() {
                notify(format!("Fine-tuning JPEG quality on {}×{}...", best_w, best_h));
                for test_q in (81..=95).step_by(2) {
                    if let Ok(b) = encode_jpeg_with_quality(&best_scaled_img, test_q) {
                        let len = b.len();
                        if len <= target_bytes {
                            notify(format!("Tuned quality to {}%: {} <= {}", test_q, format_bytes_size(len), target_str));
                            best_q = test_q;
                            best_bytes = b;
                        } else {
                            break;
                        }
                    }
                }
            } else {
                notify(format!("Applying minimal scale fallback..."));
                let min_w = ((orig_w as f32) * 0.1).round().max(16.0) as u32;
                let min_h = ((orig_h as f32) * 0.1).round().max(16.0) as u32;
                best_scaled_img = image::imageops::resize(
                    base_img,
                    min_w,
                    min_h,
                    image::imageops::FilterType::Triangle,
                );
                best_w = min_w;
                best_h = min_h;
                best_scale = 0.1;
                best_q = 40;
                if let Ok(b) = encode_jpeg_with_quality(&best_scaled_img, 40) {
                    best_bytes = b;
                }
            }

            notify(format!(
                "Complete: {} achieved at {}×{} (Quality {}%)",
                format_bytes_size(best_bytes.len()),
                best_w,
                best_h,
                best_q
            ));

            FileSizeOptimizationResult {
                target_bytes,
                achieved_bytes: best_bytes.len(),
                width: best_w,
                height: best_h,
                scale: best_scale,
                quality: best_q,
                format,
                encoded_data: best_bytes,
                result_rgba: best_scaled_img,
            }
        }
        TargetFormat::Png => {
            notify(format!("Testing full resolution ({}×{}) as lossless PNG...", orig_w, orig_h));
            let mut best_scale = 1.0f32;
            let mut best_scaled_img = base_img.clone();
            let mut best_bytes = Vec::new();
            let mut best_w = orig_w;
            let mut best_h = orig_h;

            // Check 100% first
            let mut cursor = std::io::Cursor::new(Vec::new());
            if base_img.write_to(&mut cursor, image::ImageFormat::Png).is_ok() {
                let bytes = cursor.into_inner();
                if bytes.len() <= target_bytes {
                    notify(format!(
                        "Full resolution PNG fits budget: {} <= {}",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                    return FileSizeOptimizationResult {
                        target_bytes,
                        achieved_bytes: bytes.len(),
                        width: orig_w,
                        height: orig_h,
                        scale: 1.0,
                        quality: 100,
                        format,
                        encoded_data: bytes,
                        result_rgba: base_img.clone(),
                    };
                } else {
                    notify(format!(
                        "PNG at 100% is {} > {}. Searching resolution scale...",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                }
            }

            let mut low_s = 0.05f32;
            let mut high_s = 1.0f32;

            for iter in 1..=10 {
                let mid_s = (low_s + high_s) / 2.0;
                let candidate_w = ((orig_w as f32) * mid_s).round().max(16.0) as u32;
                let candidate_h = ((orig_h as f32) * mid_s).round().max(16.0) as u32;

                notify(format!(
                    "[Step {}/10] Resampling to {:.1}% ({}×{})...",
                    iter,
                    mid_s * 100.0,
                    candidate_w,
                    candidate_h
                ));

                let scaled = image::imageops::resize(
                    base_img,
                    candidate_w,
                    candidate_h,
                    image::imageops::FilterType::Lanczos3,
                );

                let mut cur = std::io::Cursor::new(Vec::new());
                if scaled.write_to(&mut cur, image::ImageFormat::Png).is_ok() {
                    let bytes = cur.into_inner();
                    let len = bytes.len();
                    if len <= target_bytes {
                        notify(format!(
                            "Scale {:.1}% fits: {} <= {}",
                            mid_s * 100.0,
                            format_bytes_size(len),
                            target_str
                        ));
                        best_scale = mid_s;
                        best_w = candidate_w;
                        best_h = candidate_h;
                        best_bytes = bytes;
                        best_scaled_img = scaled;
                        low_s = mid_s;
                    } else {
                        notify(format!(
                            "Scale {:.1}% exceeds: {} > {}",
                            mid_s * 100.0,
                            format_bytes_size(len),
                            target_str
                        ));
                        high_s = mid_s;
                    }
                }
            }

            notify(format!(
                "Complete: {} achieved at {}×{}",
                format_bytes_size(best_bytes.len()),
                best_w,
                best_h
            ));

            FileSizeOptimizationResult {
                target_bytes,
                achieved_bytes: best_bytes.len(),
                width: best_w,
                height: best_h,
                scale: best_scale,
                quality: 100,
                format,
                encoded_data: best_bytes,
                result_rgba: best_scaled_img,
            }
        }
    }
}

impl Drop for ImagePreviewState {
    fn drop(&mut self) {
        self.cancel_ocr();
    }
}

pub fn extract_palette(img: &RgbaImage, target_count: usize) -> (Vec<PaletteColor>, usize) {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return (Vec::new(), 0);
    }

    let total_pixels = (w as usize) * (h as usize);
    let sample_stride = ((total_pixels / 5000).max(1)).max(1);

    use std::collections::HashMap;
    let mut bucket_counts: HashMap<u16, (usize, u64, u64, u64)> = HashMap::with_capacity(4096);
    let mut sampled_valid_pixels = 0usize;

    let mut i = 0;
    for pixel in img.pixels() {
        if i % sample_stride == 0 {
            if pixel[3] >= 64 {
                sampled_valid_pixels += 1;
                let r = pixel[0] as u64;
                let g = pixel[1] as u64;
                let b = pixel[2] as u64;
                let key = (((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3)) as u16;
                let entry = bucket_counts.entry(key).or_insert((0, 0, 0, 0));
                entry.0 += 1;
                entry.1 += r;
                entry.2 += g;
                entry.3 += b;
            }
        }
        i += 1;
    }

    if sampled_valid_pixels == 0 {
        return (Vec::new(), 0);
    }

    let mut candidates: Vec<(usize, u8, u8, u8)> = bucket_counts
        .into_iter()
        .map(|(_, (count, r_sum, g_sum, b_sum))| {
            let r_avg = (r_sum / count as u64).clamp(0, 255) as u8;
            let g_avg = (g_sum / count as u64).clamp(0, 255) as u8;
            let b_avg = (b_sum / count as u64).clamp(0, 255) as u8;
            (count, r_avg, g_avg, b_avg)
        })
        .collect();

    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    fn color_dist(r1: f32, g1: f32, b1: f32, r2: f32, g2: f32, b2: f32) -> f32 {
        let r_mean = (r1 + r2) / 2.0;
        let dr = r1 - r2;
        let dg = g1 - g2;
        let db = b1 - b2;
        let weight_r = 2.0 + (r_mean / 256.0);
        let weight_g = 4.0;
        let weight_b = 2.0 + ((255.0 - r_mean) / 256.0);
        ((weight_r * dr * dr) + (weight_g * dg * dg) + (weight_b * db * db)).sqrt()
    }

    let min_dist = 42.0;
    let mut selected: Vec<(Color32, usize)> = Vec::new();

    for (count, r, g, b) in &candidates {
        let r_f = *r as f32;
        let g_f = *g as f32;
        let b_f = *b as f32;

        let mut closest_idx = None;
        let mut min_d = f32::MAX;
        for (idx, (sel_col, _)) in selected.iter().enumerate() {
            let d = color_dist(r_f, g_f, b_f, sel_col.r() as f32, sel_col.g() as f32, sel_col.b() as f32);
            if d < min_d {
                min_d = d;
                closest_idx = Some(idx);
            }
        }

        if min_d < min_dist {
            if let Some(idx) = closest_idx {
                selected[idx].1 += *count;
            }
        } else if selected.len() < target_count {
            selected.push((Color32::from_rgb(*r, *g, *b), *count));
        } else if let Some(idx) = closest_idx {
            selected[idx].1 += *count;
        }
    }

    selected.sort_by(|a, b| b.1.cmp(&a.1));

    let total_counted: usize = selected.iter().map(|(_, c)| *c).sum();
    let norm_factor = if total_counted > 0 {
        total_counted as f32
    } else {
        sampled_valid_pixels as f32
    };

    let result: Vec<PaletteColor> = selected
        .into_iter()
        .map(|(color, count)| {
            let hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
            let percentage = (count as f32 / norm_factor) * 100.0;
            PaletteColor {
                color,
                hex,
                percentage,
                count,
            }
        })
        .collect();

    (result, sampled_valid_pixels)
}

fn render_svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut child = Command::new("rsvg-convert")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(bytes);
    }

    let output = child.wait_with_output().ok()?;
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
    fn test_load_png_and_meta() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(640, 360);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([59, 130, 246, 255]);
        }
        let temp_file = std::env::temp_dir().join(format!("test_preview_{}.png", uuid::Uuid::new_v4()));
        img.save(&temp_file).unwrap();

        let state = ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load PNG");
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
        let temp_file = std::env::temp_dir().join(format!("test_preview_{}.jpg", uuid::Uuid::new_v4()));
        img.save(&temp_file).unwrap();

        let state = ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load JPEG");
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
        let temp_file = std::env::temp_dir().join(format!("test_preview_{}.svg", uuid::Uuid::new_v4()));
        std::fs::write(&temp_file, svg_content).unwrap();

        let state = ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load SVG");
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();

        let item = ClipItem::from_image(png_bytes, 320, 240, "image/png");
        let state = ImagePreviewState::from_item(&item, &config).expect("Failed to load from ClipItem");
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Set normalized crop: x from 0.1 to 0.6, y from 0.2 to 0.7 (50x50)
        state.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.6, 0.7));
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 200, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        assert_eq!(state.current_rgba.width(), 100);
        assert_eq!(state.current_rgba.height(), 200);
        assert_eq!(*state.current_rgba.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));

        // Rotate 90 CW: dimensions become 200x100, red pixel moves to (199, 0)
        state.rotate_cw();
        assert_eq!(state.current_rgba.width(), 200);
        assert_eq!(state.current_rgba.height(), 100);
        assert_eq!(*state.current_rgba.get_pixel(199, 0), image::Rgba([255, 0, 0, 255]));

        // Rotate 180: dimensions stay 200x100, red pixel moves to (0, 99)
        state.rotate_180();
        assert_eq!(state.current_rgba.width(), 200);
        assert_eq!(state.current_rgba.height(), 100);
        assert_eq!(*state.current_rgba.get_pixel(0, 99), image::Rgba([255, 0, 0, 255]));

        // Flip Horizontal: pixel at (0, 99) moves to (199, 99)
        state.flip_horizontal();
        assert_eq!(*state.current_rgba.get_pixel(199, 99), image::Rgba([255, 0, 0, 255]));

        // Flip Vertical: pixel at (199, 99) moves to (199, 0)
        state.flip_vertical();
        assert_eq!(*state.current_rgba.get_pixel(199, 0), image::Rgba([255, 0, 0, 255]));

        // Test Revert
        state.revert_to_original();
        assert_eq!(state.current_rgba.width(), 100);
        assert_eq!(state.current_rgba.height(), 200);
        assert_eq!(*state.current_rgba.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn test_current_display_image_crop_selection() {
        let config = AppConfig::default();
        let img = RgbaImage::new(200, 200);
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 200, 200, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // In View mode, display image is full 200x200
        assert_eq!(state.current_display_image().width(), 200);
        assert_eq!(state.current_display_image().height(), 200);

        // Switch to Crop mode and set crop rect to 50%
        state.active_mode = ImageToolMode::Crop;
        state.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 0.5));
        
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Place redaction box covering x: [0.2, 0.6], y: [0.3, 0.7] (pixels 20..60, 30..70)
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.3), Pos2::new(0.6, 0.7)),
        });

        // Test that current_display_image burns pending redactions non-destructively
        let disp_img = state.current_display_image();
        assert_eq!(*disp_img.get_pixel(40, 50), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*disp_img.get_pixel(10, 10), image::Rgba([255, 255, 255, 255]));
        assert_eq!(*state.current_rgba.get_pixel(40, 50), image::Rgba([255, 255, 255, 255])); // canvas untouched yet

        // Burn / apply redactions permanently
        state.apply_redactions();
        assert_eq!(*state.current_rgba.get_pixel(40, 50), image::Rgba([0, 0, 0, 255])); // burned
        assert_eq!(*state.current_rgba.get_pixel(10, 10), image::Rgba([255, 255, 255, 255])); // untouched
        assert_eq!(state.redaction_state.boxes.len(), 0);
        assert_eq!(state.undo_stack.len(), 1);

        // Undo restoration
        state.undo();
        assert_eq!(*state.current_rgba.get_pixel(40, 50), image::Rgba([255, 255, 255, 255]));
    }

    #[test]
    fn test_redaction_delete_and_clear() {
        let config = AppConfig::default();
        let img = RgbaImage::new(100, 100);
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        let opt_res = state.resize_state.last_result.clone().expect("Optimization result missing");

        assert!(opt_res.achieved_bytes <= target_bytes, "Achieved {} > target {}", opt_res.achieved_bytes, target_bytes);
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
        let res = optimize_to_target_size_with_progress(&img, target_bytes, TargetFormat::Jpeg, Some(&cb));
        assert!(res.achieved_bytes <= target_bytes + 2048);
        let list = captured.lock().unwrap();
        assert!(!list.is_empty(), "Progress messages should have been captured");
        assert!(list.iter().any(|m| m.contains("Testing full resolution") || m.contains("Trying quality") || m.contains("Complete")), "Expected recognizable progress messages: {:?}", list);
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

        assert!(res.achieved_bytes <= target_bytes, "Achieved {} > target {}", res.achieved_bytes, target_bytes);
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
        assert!(hex_codes.iter().any(|h| h == "#FF0000"), "Red should be in palette: {:?}", hex_codes);
        assert!(hex_codes.iter().any(|h| h == "#0000FF"), "Blue should be in palette: {:?}", hex_codes);

        // Each half should have approximately 50% representation
        let red_pal = palette.iter().find(|p| p.hex == "#FF0000").unwrap();
        let blue_pal = palette.iter().find(|p| p.hex == "#0000FF").unwrap();
        assert!((red_pal.percentage - 50.0).abs() < 5.0, "Red % was {}", red_pal.percentage);
        assert!((blue_pal.percentage - 50.0).abs() < 5.0, "Blue % was {}", blue_pal.percentage);
    }

    #[test]
    fn test_bg_removal_worker_invocation_and_undo() {
        let config = AppConfig::default();
        let mut img = RgbaImage::new(100, 100);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([200, 100, 50, 255]);
        }
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        assert!(state.bg_removal_error.is_none(), "Error: {:?}", state.bg_removal_error);
        assert_eq!(state.undo_stack.len(), 1);

        // Verify transparent pixel was applied on left half
        assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 0);
        assert_eq!(state.current_rgba.get_pixel(60, 60)[3], 255);

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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 40, 40, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // 1. Initial convert state should be WebP
        assert_eq!(state.convert_state.target_format, ConvertTargetFormat::Webp);
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
    fn test_barcode_state_and_scan() {
        let config = AppConfig::default();
        // Generate a QR code using rxing
        let writer = rxing::qrcode::QRCodeWriter;
        use rxing::Writer;
        let bit_matrix = writer
            .encode(
                "https://github.com",
                &rxing::BarcodeFormat::QR_CODE,
                64,
                64,
            )
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 64, 64, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Run barcode scan
        state.run_barcode_scan();
        assert!(state.barcode_state.has_scanned);
        assert_eq!(state.barcode_state.detections.len(), 1);
        assert_eq!(state.barcode_state.detections[0].text, "https://github.com");
    }

    #[test]
    fn test_ocr_state_transitions() {
        let config = AppConfig::default();
        let img = RgbaImage::new(20, 20);
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.ocr_state.in_progress = true;
        state.ocr_state.status_text = Some("Processing image with GLM-OCR...".into());
        state.ocr_child_pid.store(999999, std::sync::atomic::Ordering::SeqCst);

        // Cancel OCR
        state.cancel_ocr();

        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.status_text.is_none());
        assert_eq!(state.ocr_child_pid.load(std::sync::atomic::Ordering::SeqCst), 0);
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
        assert_eq!(ImageToolMode::View.label(), "🔍 [V]iew");
        assert_eq!(ImageToolMode::Crop.label(), "✂ [C]rop");
        assert_eq!(ImageToolMode::Deskew.label(), "📐 Des[K]ew");
        assert_eq!(ImageToolMode::Transform.label(), "🔄 [F]lip/Rotate");
        assert_eq!(ImageToolMode::Redact.label(), "█ [X] Redact");
        assert_eq!(ImageToolMode::PickColor.label(), "💉 [P]ick Color");
        assert_eq!(ImageToolMode::Resize.label(), "↔ Re[S]ize");
        assert_eq!(ImageToolMode::Convert.label(), "🔄 Conver[T]");
        assert_eq!(ImageToolMode::Ocr.label(), "📝 [O]CR");
        assert_eq!(ImageToolMode::Barcode.label(), "▦ [B]arcode");
        assert_eq!(ImageToolMode::RemoveBg.label(), "✨ [R]emove BG");
        assert_eq!(ImageToolMode::DepthMap.label(), "🌊 [D]epth Map");
        assert_eq!(ImageToolMode::DecorrelationStretch.label(), "🌈 Decorrelation Stretch");
    }

    #[test]
    fn test_set_tool_mode_integration() {
        let config = AppConfig::default();
        let img = RgbaImage::new(20, 20);
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        state.ocr_child_pid.store(12345, std::sync::atomic::Ordering::SeqCst);
        state.set_tool_mode(ImageToolMode::View);
        assert_eq!(state.active_mode, ImageToolMode::View);
        assert!(!state.ocr_state.in_progress);
        assert_eq!(state.ocr_child_pid.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn test_metadata_inspection_and_stripping() {
        let config = AppConfig::default();
        let img = RgbaImage::new(30, 30);
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        assert!(stocks.iter().any(|s| s.name == "Portra 400" && s.brand == "Kodak"));
        assert!(stocks.iter().any(|s| s.name == "Velvia 50" && s.brand == "Fujifilm"));
        assert!(stocks.iter().any(|s| s.name == "HP5 Plus 400" && s.brand == "Ilford"));
        assert!(stocks.iter().any(|s| s.name == "600 Color" && s.brand == "Polaroid"));
        assert!(stocks.iter().any(|s| s.name == "800T" && s.brand == "CineStill"));
        assert!(stocks.iter().any(|s| s.name == "Vista 200" && s.brand == "Agfa"));

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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
        assert!(lut_pos[64] < 64, "Shadows should be darkened for high contrast");
        assert!(lut_pos[192] > 192, "Highlights should be brightened for high contrast");
        assert!((lut_pos[128] as i32 - 128).abs() <= 1, "Midtones should remain anchored within 1 level");

        // Low contrast (e.g. -0.5): shadows lifted, highlights softened
        let lut_neg = compute_lut(-0.5);
        assert!(lut_neg[64] > 64, "Shadows should be lifted for low contrast");
        assert!(lut_neg[192] < 192, "Highlights should be softened for low contrast");
        assert!((lut_neg[128] as i32 - 128).abs() <= 1, "Midtones should remain anchored within 1 level");
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.film_sim_state.pending_thumbnails.is_empty() && std::time::Instant::now() < deadline {
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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 32, 32, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        // Label check (no shortcut letter)
        assert_eq!(ImageToolMode::CreativeFx.label(), "✨ Creative FX");

        // Defaults
        assert_eq!(state.creative_fx_state.selected_fx, CreativeFxType::LensAberration);
        assert_eq!(state.creative_fx_state.lens_params.strength, 25.0);
        assert_eq!(state.creative_fx_state.lens_params.falloff, 50.0);
        assert_eq!(state.creative_fx_state.lens_params.color_balance, 0.0);
        assert_eq!(state.creative_fx_state.trichromatic_params.mode, TrichromaticMode::Rgb);
        assert_eq!(state.creative_fx_state.trichromatic_params.channels.len(), 3);
        assert_eq!(state.creative_fx_state.pixel_sort_params.direction, PixelSortDirection::Vertical);
        assert_eq!(state.creative_fx_state.cmyk_params.dot_size, 5.0);
        assert_eq!(state.creative_fx_state.cmyk_params.paper_tint, PaperTint::Newsprint);
        assert_eq!(state.creative_fx_state.dither_params.palette, RetroPalette::GameBoy);
        assert_eq!(state.creative_fx_state.dither_params.method, DitherMethod::Bayer4x4);
        assert_eq!(state.creative_fx_state.ntsc_params.luma_smear, 0.25);
        assert_eq!(state.creative_fx_state.ntsc_params.tape_speed, VhsTapeSpeed::SP);
        assert_eq!(state.creative_fx_state.ntsc_params.demodulation, NtscDemodulation::Notch);
        assert_eq!(state.creative_fx_state.ntsc_params.phase_shift, NtscPhaseShift::Degrees180);
        assert_eq!(state.creative_fx_state.ntsc_params.chroma_lowpass, NtscChromaLowpass::Full);

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
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();
        let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.run_creative_fx_thumbnail_batch();
        assert!(state.creative_fx_state.is_generating_thumbnails);
        assert!(state.creative_fx_state.thumbnail_rx.is_some());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.creative_fx_state.pending_thumbnails.is_empty() && std::time::Instant::now() < deadline {
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
}

