//! Rich interactive image workbench and manipulation suite.
//!
//! Provides pan/zoom, cropping with fixed aspect ratios, pixelation/blackout redaction,
//! eyedropper color inspection, resizing, palette extraction, background removal, and 3D depth map generation.

use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use image::RgbaImage;
use little_clip_core::{AppConfig, ClipItem, HandSide, IpcRequest};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Active image manipulation tool mode selected in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageToolMode {
    /// Pan and zoom inspection mode.
    #[default]
    View,
    /// Crop region selection and trimming.
    Crop,
    /// Blackout, blur, or pixelation redaction.
    Redact,
    /// Eyedropper pixel color sampler.
    PickColor,
    /// Dimension and file size optimization resizing.
    Resize,
    /// Rotation and horizontal/vertical flipping.
    Transform,
    /// Dominant color palette extraction and harmonic scheme generation.
    Palette,
    /// AI-powered background removal.
    RemoveBg,
    /// Monocular depth map generation.
    DepthMap,
}

impl ImageToolMode {
    /// Human-readable toolbar label with icon.
    pub fn label(&self) -> &'static str {
        match self {
            ImageToolMode::View => "🔍 View / Pan",
            ImageToolMode::Crop => "✂ Crop",
            ImageToolMode::Redact => "█ Redact",
            ImageToolMode::PickColor => "💉 Pick Color",
            ImageToolMode::Resize => "↔ Resize",
            ImageToolMode::Transform => "🔄 Rotate & Flip",
            ImageToolMode::Palette => "🎨 Palette",
            ImageToolMode::RemoveBg => "✨ Remove BG",
            ImageToolMode::DepthMap => "🌊 Depth Map",
        }
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

        Ok(Self {
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
        })
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

        let save_res = if ext == "jpg" || ext == "jpeg" {
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
            self.show_toast("⟳ Redone");
        }
    }

    pub fn revert_to_original(&mut self) {
        if !self.undo_stack.is_empty() {
            self.undo_stack.push(self.current_rgba.clone());
            self.current_rgba = self.original_rgba.clone();
            self.dirty_texture = true;
            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
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

    pub fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
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
            if self.active_mode == ImageToolMode::Crop && self.crop_state.normalized_rect != Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)) {
                self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                self.crop_state.ratio = CropRatio::Free;
            } else if self.active_mode == ImageToolMode::Redact && self.redaction_state.selected_idx.is_some() {
                self.redaction_state.selected_idx = None;
            } else if self.active_mode == ImageToolMode::Redact && !self.redaction_state.boxes.is_empty() {
                self.clear_all_redactions();
            } else {
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
        if !ctx.wants_keyboard_input() {
            if self.active_mode == ImageToolMode::Redact {
                let del_pressed = ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                    || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
                if del_pressed {
                    self.delete_selected_redaction();
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Num0) || i.key_pressed(egui::Key::F)) {
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
            .fill(Color32::from_rgb(15, 17, 23))
            .show(ui, |ui| {
                // 1. TOP HEADER & ESCAPE HATCHES
                self.render_header(ui, ctx, close);

                ui.add_space(4.0);

                // 2. TOOLBAR (Mode selection & Viewport controls)
                self.render_toolbar(ui);

                // 2b. Secondary tool-specific strip (Crop / Transforms / Redact / PickColor / Resize / Palette / RemoveBg / DepthMap)
                if self.active_mode == ImageToolMode::Crop {
                    ui.add_space(2.0);
                    self.render_crop_toolbar(ui);
                } else if self.active_mode == ImageToolMode::Transform {
                    ui.add_space(2.0);
                    self.render_transform_toolbar(ui);
                } else if self.active_mode == ImageToolMode::Redact {
                    ui.add_space(2.0);
                    self.render_redact_toolbar(ui);
                } else if self.active_mode == ImageToolMode::PickColor {
                    ui.add_space(2.0);
                    self.render_pick_color_toolbar(ui, ctx);
                } else if self.active_mode == ImageToolMode::Resize {
                    ui.add_space(2.0);
                    self.render_resize_toolbar(ui, ctx);
                } else if self.active_mode == ImageToolMode::Palette {
                    ui.add_space(2.0);
                    self.render_palette_toolbar(ui, ctx);
                } else if self.active_mode == ImageToolMode::RemoveBg {
                    ui.add_space(2.0);
                    self.render_remove_bg_toolbar(ui, ctx);
                } else if self.active_mode == ImageToolMode::DepthMap {
                    ui.add_space(2.0);
                    self.render_depth_map_toolbar(ui, ctx);
                }

                ui.add_space(4.0);

                // 3. MAIN INTERACTIVE VIEWPORT
                self.render_viewport(ui, ctx);

                // 4. BOTTOM STATUS / HELP BAR
                self.render_footer(ui, now);
            });
    }

    fn render_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        egui::Frame::none()
            .fill(Color32::from_rgb(20, 24, 34))
            .inner_margin(egui::Margin::symmetric(14.0, 8.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Title badge
                    ui.label(
                        egui::RichText::new("🖼 IMAGE STUDIO")
                            .strong()
                            .color(Color32::from_rgb(168, 85, 247)),
                    );

                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .color(Color32::from_rgb(241, 245, 249)),
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
                    chip_badge(ui, &dim_text, Color32::from_rgb(56, 189, 248), Color32::from_rgb(15, 23, 42));

                    // Format chip
                    let fmt_str = if self.is_svg && self.undo_stack.is_empty() {
                        "SVG (VECTOR)".to_string()
                    } else {
                        self.mime.replace("image/", "").to_uppercase()
                    };
                    chip_badge(ui, &fmt_str, Color32::from_rgb(16, 185, 129), Color32::from_rgb(6, 78, 59));

                    // File size chip
                    let size_str = format_bytes_size(self.file_size_bytes);
                    chip_badge(ui, &size_str, Color32::from_rgb(245, 158, 11), Color32::from_rgb(69, 26, 3));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Close button
                        let close_btn = egui::Button::new(
                            egui::RichText::new("✕ Close (Esc)")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(248, 113, 113)),
                        )
                        .fill(Color32::from_rgb(30, 41, 59))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(71, 85, 105)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(close_btn).clicked() {
                            *close = true;
                        }

                        ui.add_space(8.0);

                        // Escape Hatch 3: Viewer (Nomacs / default)
                        let viewer_label = format!("👁 {}", self.viewer);
                        let viewer_btn = egui::Button::new(
                            egui::RichText::new(viewer_label)
                                .small()
                                .strong()
                                .color(Color32::from_rgb(203, 213, 225)),
                        )
                        .fill(Color32::from_rgb(28, 35, 51))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(viewer_btn).on_hover_text(format!("Open in external viewer ({})", self.viewer)).clicked() {
                            let v = self.viewer.clone();
                            self.launch_external_app(&v, ctx, close);
                        }

                        // Escape Hatch 2: Inkscape (SVG)
                        let inkscape_label = format!("✒ {}", self.svg_editor);
                        let is_primary_svg = self.is_svg && self.undo_stack.is_empty();
                        let inkscape_btn = egui::Button::new(
                            egui::RichText::new(inkscape_label)
                                .small()
                                .strong()
                                .color(if is_primary_svg { Color32::WHITE } else { Color32::from_rgb(148, 163, 184) }),
                        )
                        .fill(if is_primary_svg { Color32::from_rgb(13, 148, 136) } else { Color32::from_rgb(24, 30, 44) })
                        .stroke(Stroke::new(1.0, if is_primary_svg { Color32::from_rgb(45, 212, 191) } else { Color32::from_rgb(51, 65, 85) }))
                        .rounding(Rounding::same(4.0));
                        if ui.add(inkscape_btn).on_hover_text("Open file in Inkscape (Vector Editor)").clicked() {
                            let ed = self.svg_editor.clone();
                            self.launch_external_app(&ed, ctx, close);
                        }

                        // Escape Hatch 1: Krita (Raster)
                        let krita_label = format!("🎨 {}", self.external_editor);
                        let is_primary_raster = !is_primary_svg;
                        let krita_btn = egui::Button::new(
                            egui::RichText::new(krita_label)
                                .small()
                                .strong()
                                .color(if is_primary_raster { Color32::WHITE } else { Color32::from_rgb(148, 163, 184) }),
                        )
                        .fill(if is_primary_raster { Color32::from_rgb(99, 102, 241) } else { Color32::from_rgb(24, 30, 44) })
                        .stroke(Stroke::new(1.0, if is_primary_raster { Color32::from_rgb(129, 140, 248) } else { Color32::from_rgb(51, 65, 85) }))
                        .rounding(Rounding::same(4.0));
                        if ui.add(krita_btn).on_hover_text("Open file in Krita (Full Photo & Painting Studio)").clicked() {
                            let ed = self.external_editor.clone();
                            self.launch_external_app(&ed, ctx, close);
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        // Save As button
                        let save_btn = egui::Button::new(
                            egui::RichText::new("💾 Save As...")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(226, 232, 240)),
                        )
                        .fill(Color32::from_rgb(16, 78, 59))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(52, 211, 153)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(save_btn).on_hover_text("Save current image to file (Ctrl+S)").clicked() {
                            self.open_save_dialog();
                        }

                        // Copy to Clipboard Slot button
                        let copy_btn = egui::Button::new(
                            egui::RichText::new("📋 Copy (Ctrl+C)")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(241, 245, 249)),
                        )
                        .fill(Color32::from_rgb(88, 28, 135))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(192, 132, 252)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(copy_btn).on_hover_text("Copy current image to Left Hand clipboard slot & OS clipboard (Ctrl+C)").clicked() {
                            self.copy_to_clipboard_slot();
                        }
                    });
                });
            });
    }

    fn render_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(18, 22, 32))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(37, 45, 62)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Tool Modes
                    let modes = [
                        ImageToolMode::View,
                        ImageToolMode::Crop,
                        ImageToolMode::Transform,
                        ImageToolMode::Redact,
                        ImageToolMode::PickColor,
                        ImageToolMode::Resize,
                        ImageToolMode::Palette,
                        ImageToolMode::RemoveBg,
                        ImageToolMode::DepthMap,
                    ];

                    for mode in modes {
                        let is_active = self.active_mode == mode;
                        let (bg, border, text_col) = if is_active {
                            (Color32::from_rgb(99, 102, 241), Color32::from_rgb(165, 180, 252), Color32::WHITE)
                        } else {
                            (Color32::from_rgb(26, 32, 46), Color32::from_rgb(45, 55, 75), Color32::from_rgb(148, 163, 184))
                        };

                        let btn = egui::Button::new(
                            egui::RichText::new(mode.label()).small().strong().color(text_col),
                        )
                        .fill(bg)
                        .stroke(Stroke::new(1.0, border))
                        .rounding(Rounding::same(4.0));

                        let resp = ui.add(btn);
                        if resp.clicked() {
                            self.active_mode = mode;
                            match mode {
                                ImageToolMode::View
                                | ImageToolMode::Crop
                                | ImageToolMode::Transform
                                | ImageToolMode::Redact
                                | ImageToolMode::PickColor
                                | ImageToolMode::RemoveBg
                                | ImageToolMode::DepthMap => {}
                                ImageToolMode::Resize => {
                                    self.resize_state.sync_from_image(&self.current_rgba);
                                }
                                ImageToolMode::Palette => {
                                    if !self.palette_state.is_extracted || self.palette_state.extracted_colors.is_empty() {
                                        self.run_palette_extraction();
                                    }
                                }
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Reset View
                        let reset_btn = egui::Button::new(
                            egui::RichText::new("⟲ Reset").small().color(Color32::from_rgb(203, 213, 225)),
                        )
                        .fill(Color32::from_rgb(28, 35, 51))
                        .rounding(Rounding::same(4.0));
                        if ui.add(reset_btn).on_hover_text("Reset zoom and pan").clicked() {
                            self.zoom = 1.0;
                            self.pan = Vec2::ZERO;
                        }

                        // 1:1 Pixel Zoom
                        let pixel_btn = egui::Button::new(
                            egui::RichText::new("1:1").small().strong().color(Color32::from_rgb(203, 213, 225)),
                        )
                        .fill(Color32::from_rgb(28, 35, 51))
                        .rounding(Rounding::same(4.0));
                        if ui.add(pixel_btn).on_hover_text("100% actual pixel size (1)").clicked() {
                            self.zoom = 1.0;
                        }

                        // Fit to window
                        let fit_btn = egui::Button::new(
                            egui::RichText::new("⛶ Fit").small().strong().color(Color32::from_rgb(56, 189, 248)),
                        )
                        .fill(Color32::from_rgb(28, 35, 51))
                        .rounding(Rounding::same(4.0));
                        if ui.add(fit_btn).on_hover_text("Fit entire image into window (0 / F)").clicked() {
                            self.fit_to_view(ui.available_size());
                        }

                        // Zoom In (+)
                        let plus_btn = egui::Button::new(
                            egui::RichText::new("+").small().strong().color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(37, 45, 62))
                        .rounding(Rounding::same(4.0));
                        if ui.add(plus_btn).clicked() {
                            self.zoom = (self.zoom * 1.25).clamp(0.05, 50.0);
                        }

                        // Zoom percentage readout
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                                .small()
                                .color(Color32::from_rgb(148, 163, 184)),
                        );

                        // Zoom Out (-)
                        let minus_btn = egui::Button::new(
                            egui::RichText::new("-").small().strong().color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(37, 45, 62))
                        .rounding(Rounding::same(4.0));
                        if ui.add(minus_btn).clicked() {
                            self.zoom = (self.zoom / 1.25).clamp(0.05, 50.0);
                        }
                    });
                });
            });
    }

    fn render_crop_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(22, 27, 39))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("✂ Crop Mode:")
                            .strong()
                            .color(Color32::from_rgb(52, 211, 153)),
                    );

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Ratio:")
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    let ratios = [
                        CropRatio::Free,
                        CropRatio::Ratio1x1,
                        CropRatio::Ratio16x9,
                        CropRatio::Ratio9x16,
                    ];

                    for r in ratios {
                        let is_sel = self.crop_state.ratio == r;
                        let (bg, border, text_col) = if is_sel {
                            (Color32::from_rgb(16, 185, 129), Color32::from_rgb(52, 211, 153), Color32::WHITE)
                        } else {
                            (Color32::from_rgb(30, 41, 59), Color32::from_rgb(51, 65, 85), Color32::from_rgb(203, 213, 225))
                        };

                        let btn = egui::Button::new(
                            egui::RichText::new(r.label()).small().strong().color(text_col),
                        )
                        .fill(bg)
                        .stroke(Stroke::new(1.0, border))
                        .rounding(Rounding::same(4.0));

                        if ui.add(btn).clicked() {
                            self.crop_state.set_ratio(r, self.current_rgba.width(), self.current_rgba.height());
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Apply Crop Button
                        let apply_btn = egui::Button::new(
                            egui::RichText::new("✓ Apply Crop (Enter)")
                                .small()
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(16, 185, 129))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(52, 211, 153)))
                        .rounding(Rounding::same(4.0));

                        if ui.add(apply_btn).clicked() {
                            self.apply_crop();
                        }

                        // Reset Crop Button
                        let reset_crop_btn = egui::Button::new(
                            egui::RichText::new("✕ Full Image").small().color(Color32::from_rgb(203, 213, 225)),
                        )
                        .fill(Color32::from_rgb(30, 41, 59))
                        .rounding(Rounding::same(4.0));

                        if ui.add(reset_crop_btn).clicked() {
                            self.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                            self.crop_state.ratio = CropRatio::Free;
                        }

                        if !self.undo_stack.is_empty() {
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                    .small()
                                    .strong()
                                    .color(Color32::from_rgb(245, 158, 11)),
                            )
                            .fill(Color32::from_rgb(30, 41, 59))
                            .rounding(Rounding::same(4.0));

                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    fn render_transform_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(22, 27, 39))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🔄 Transforms:")
                            .strong()
                            .color(Color32::from_rgb(99, 102, 241)),
                    );

                    ui.add_space(8.0);

                    // Rotate CCW
                    let ccw_btn = egui::Button::new(
                        egui::RichText::new("⟲ 90° CCW").small().strong().color(Color32::from_rgb(241, 245, 249)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(ccw_btn).on_hover_text("Rotate 90 degrees counter-clockwise").clicked() {
                        self.rotate_ccw();
                    }

                    // Rotate CW
                    let cw_btn = egui::Button::new(
                        egui::RichText::new("⟳ 90° CW").small().strong().color(Color32::from_rgb(241, 245, 249)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(cw_btn).on_hover_text("Rotate 90 degrees clockwise").clicked() {
                        self.rotate_cw();
                    }

                    // Rotate 180
                    let r180_btn = egui::Button::new(
                        egui::RichText::new("180°").small().strong().color(Color32::from_rgb(241, 245, 249)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(r180_btn).on_hover_text("Rotate 180 degrees").clicked() {
                        self.rotate_180();
                    }

                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Mirror:")
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    // Flip H
                    let fh_btn = egui::Button::new(
                        egui::RichText::new("⇆ Flip Horiz").small().strong().color(Color32::from_rgb(241, 245, 249)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(fh_btn).on_hover_text("Flip horizontally (mirror left-right)").clicked() {
                        self.flip_horizontal();
                    }

                    // Flip V
                    let fv_btn = egui::Button::new(
                        egui::RichText::new("⇅ Flip Vert").small().strong().color(Color32::from_rgb(241, 245, 249)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(fv_btn).on_hover_text("Flip vertically (mirror up-down)").clicked() {
                        self.flip_vertical();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Revert all
                        let revert_btn = egui::Button::new(
                            egui::RichText::new("↺ Revert to Original").small().color(Color32::from_rgb(248, 113, 113)),
                        )
                        .fill(Color32::from_rgb(30, 41, 59))
                        .rounding(Rounding::same(4.0));
                        if ui.add(revert_btn).clicked() {
                            self.revert_to_original();
                        }

                        // Undo
                        if !self.undo_stack.is_empty() {
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(Color32::from_rgb(245, 158, 11)),
                            )
                            .fill(Color32::from_rgb(30, 41, 59))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    fn render_redact_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(24, 28, 40))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("█ Privacy Redaction:")
                            .strong()
                            .small()
                            .color(Color32::from_rgb(248, 113, 113)),
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
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    ui.add_space(8.0);

                    // Burn / Apply Redactions button
                    let has_boxes = !self.redaction_state.boxes.is_empty();
                    let burn_btn = egui::Button::new(
                        egui::RichText::new("🔥 Burn / Apply (Enter)")
                            .small()
                            .strong()
                            .color(if has_boxes { Color32::WHITE } else { Color32::from_rgb(100, 116, 139) }),
                    )
                    .fill(if has_boxes { Color32::from_rgb(185, 28, 28) } else { Color32::from_rgb(30, 41, 59) })
                    .stroke(Stroke::new(1.0, if has_boxes { Color32::from_rgb(239, 68, 68) } else { Color32::from_rgb(51, 65, 85) }))
                    .rounding(Rounding::same(4.0));

                    if ui.add_enabled(has_boxes, burn_btn).on_hover_text("Permanently rasterize black bars into image pixels (Enter)").clicked() {
                        self.apply_redactions();
                    }

                    // Delete Selected button
                    let has_selection = self.redaction_state.selected_idx.is_some();
                    let del_btn = egui::Button::new(
                        egui::RichText::new("🗑 Delete Selected (Del)")
                            .small()
                            .strong()
                            .color(if has_selection { Color32::from_rgb(254, 202, 202) } else { Color32::from_rgb(100, 116, 139) }),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, if has_selection { Color32::from_rgb(239, 68, 68) } else { Color32::from_rgb(51, 65, 85) }))
                    .rounding(Rounding::same(4.0));

                    if ui.add_enabled(has_selection, del_btn).clicked() {
                        self.delete_selected_redaction();
                    }

                    // Clear All button
                    let clear_btn = egui::Button::new(
                        egui::RichText::new("✕ Clear All")
                            .small()
                            .color(if has_boxes { Color32::from_rgb(203, 213, 225) } else { Color32::from_rgb(100, 116, 139) }),
                    )
                    .fill(Color32::from_rgb(24, 30, 44))
                    .rounding(Rounding::same(4.0));

                    if ui.add_enabled(has_boxes, clear_btn).clicked() {
                        self.clear_all_redactions();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(Color32::from_rgb(245, 158, 11)),
                            )
                            .fill(Color32::from_rgb(30, 41, 59))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    fn render_pick_color_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Frame::none()
            .fill(Color32::from_rgb(24, 28, 40))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("💉 Magnifying Color Picker:")
                            .strong()
                            .small()
                            .color(Color32::from_rgb(56, 189, 248)),
                    );

                    let display_col = self.pick_color_state.hovered_color
                        .or(self.pick_color_state.last_picked)
                        .unwrap_or(Color32::WHITE);
                    let hex_code = format!("#{:02X}{:02X}{:02X}", display_col.r(), display_col.g(), display_col.b());

                    // Swatch preview box
                    let (swatch_rect, _) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), egui::Sense::hover());
                    ui.painter().rect_filled(swatch_rect, Rounding::same(3.0), display_col);
                    ui.painter().rect_stroke(swatch_rect, Rounding::same(3.0), Stroke::new(1.0, Color32::from_rgb(148, 163, 184)));

                    ui.label(
                        egui::RichText::new(&hex_code)
                            .strong()
                            .monospace()
                            .small()
                            .color(Color32::WHITE),
                    );

                    let rgb_str = format!("RGB({}, {}, {})", display_col.r(), display_col.g(), display_col.b());
                    ui.label(
                        egui::RichText::new(rgb_str)
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    let copy_btn = egui::Button::new(
                        egui::RichText::new("📋 Copy Hex")
                            .small()
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(14, 116, 144))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(6, 182, 212)))
                    .rounding(Rounding::same(4.0));

                    if ui.add(copy_btn).clicked() {
                        self.copy_color_to_clipboard(display_col, ctx);
                    }

                    // Recent palette history
                    if !self.pick_color_state.recent_colors.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Recent:").small().color(Color32::from_rgb(148, 163, 184)));

                        let recents = self.pick_color_state.recent_colors.clone();
                        for c in recents {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), egui::Sense::click());
                            ui.painter().rect_filled(rect, Rounding::same(2.0), c);
                            ui.painter().rect_stroke(rect, Rounding::same(2.0), Stroke::new(1.0, Color32::from_rgb(71, 85, 105)));
                            let hex = format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
                            if resp.on_hover_text(format!("Click to copy {}", hex)).clicked() {
                                self.copy_color_to_clipboard(c, ctx);
                            }
                        }
                    }
                });
            });
    }

    fn render_resize_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.poll_optimizer();
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 27, 39))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    let is_pixels = self.resize_state.sub_tab == ResizeSubTab::Pixels;
                    let is_filesize = self.resize_state.sub_tab == ResizeSubTab::FileSize;

                    // Row 1: Primary Controls
                    ui.horizontal(|ui| {
                        // Sub-tab toggles
                        let px_btn = egui::Button::new(
                            egui::RichText::new("↔ Pixel Size")
                                .small()
                                .strong()
                                .color(if is_pixels { Color32::WHITE } else { Color32::from_rgb(148, 163, 184) }),
                        )
                        .fill(if is_pixels { Color32::from_rgb(79, 70, 229) } else { Color32::from_rgb(30, 41, 59) })
                        .stroke(Stroke::new(1.0, if is_pixels { Color32::from_rgb(129, 140, 248) } else { Color32::from_rgb(51, 65, 85) }))
                        .rounding(Rounding::same(4.0));

                        if ui.add(px_btn).clicked() {
                            self.resize_state.sub_tab = ResizeSubTab::Pixels;
                            self.resize_state.sync_from_image(&self.current_rgba);
                        }

                        let fs_btn = egui::Button::new(
                            egui::RichText::new("🎯 Target File Size Optimizer")
                                .small()
                                .strong()
                                .color(if is_filesize { Color32::WHITE } else { Color32::from_rgb(148, 163, 184) }),
                        )
                        .fill(if is_filesize { Color32::from_rgb(13, 148, 136) } else { Color32::from_rgb(30, 41, 59) })
                        .stroke(Stroke::new(1.0, if is_filesize { Color32::from_rgb(45, 212, 191) } else { Color32::from_rgb(51, 65, 85) }))
                        .rounding(Rounding::same(4.0));

                        if ui.add(fs_btn).clicked() {
                            self.resize_state.sub_tab = ResizeSubTab::FileSize;
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        if is_pixels {
                            // 1. Pixel Resize UI
                            ui.label(egui::RichText::new("W:").small().strong().color(Color32::from_rgb(148, 163, 184)));

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
                                .fill(if self.resize_state.lock_aspect { Color32::from_rgb(30, 41, 59) } else { Color32::from_rgb(15, 23, 42) })
                                .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                                .rounding(Rounding::same(4.0));

                            if ui.add(lock_btn).on_hover_text("Lock aspect ratio").clicked() {
                                self.resize_state.lock_aspect = !self.resize_state.lock_aspect;
                                if self.resize_state.lock_aspect && self.current_rgba.height() > 0 {
                                    self.resize_state.aspect_ratio_base = self.current_rgba.width() as f32 / self.current_rgba.height() as f32;
                                }
                            }

                            ui.label(egui::RichText::new("H:").small().strong().color(Color32::from_rgb(148, 163, 184)));

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
                                let p_btn = egui::Button::new(egui::RichText::new(label).small().color(Color32::from_rgb(203, 213, 225)))
                                    .fill(Color32::from_rgb(30, 41, 59))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
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
                            let apply_btn = egui::Button::new(
                                egui::RichText::new("↔ Apply (Enter)")
                                    .small()
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(79, 70, 229))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(129, 140, 248)))
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
                                let p_btn = egui::Button::new(
                                    egui::RichText::new(lbl)
                                        .small()
                                        .strong()
                                        .color(if is_sel { Color32::WHITE } else { Color32::from_rgb(203, 213, 225) }),
                                )
                                .fill(if is_sel { Color32::from_rgb(13, 148, 136) } else { Color32::from_rgb(30, 41, 59) })
                                .stroke(Stroke::new(1.0, if is_sel { Color32::from_rgb(45, 212, 191) } else { Color32::from_rgb(51, 65, 85) }))
                                .rounding(Rounding::same(3.0));

                                if ui.add(p_btn).clicked() {
                                    self.resize_state.target_size_val = val;
                                    self.resize_state.target_size_unit_mb = is_mb;
                                }
                            }

                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Target:").small().color(Color32::from_rgb(148, 163, 184)));

                            ui.add(
                                egui::DragValue::new(&mut self.resize_state.target_size_val)
                                    .speed(0.1)
                                    .clamp_range(0.01..=1000.0)
                            );

                            let unit_str = if self.resize_state.target_size_unit_mb { "MB" } else { "KB" };
                            let unit_btn = egui::Button::new(egui::RichText::new(unit_str).small().strong().color(Color32::from_rgb(56, 189, 248)))
                                .fill(Color32::from_rgb(30, 41, 59))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
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
                                let run_btn = egui::Button::new(
                                    egui::RichText::new("⏳ Optimizing...")
                                        .small()
                                        .strong()
                                        .color(Color32::from_rgb(148, 163, 184)),
                                )
                                .fill(Color32::from_rgb(30, 41, 59))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                                .rounding(Rounding::same(4.0));
                                ui.add_enabled(false, run_btn);
                            } else {
                                let run_btn = egui::Button::new(
                                    egui::RichText::new("⚡ Run Optimizer")
                                        .small()
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(13, 148, 136))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(45, 212, 191)))
                                .rounding(Rounding::same(4.0));

                                if ui.add(run_btn).on_hover_text("Find optimal quality and resolution trade-off to fit target size").clicked() {
                                    self.run_target_file_size_optimizer();
                                }
                            }
                        }

                        // Right aligned undo/revert
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !self.undo_stack.is_empty() {
                                let undo_btn = egui::Button::new(
                                    egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(Color32::from_rgb(245, 158, 11)),
                                )
                                .fill(Color32::from_rgb(30, 41, 59))
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
                                ui.add(egui::Spinner::new().size(14.0).color(Color32::from_rgb(45, 212, 191)));
                                ui.add_space(6.0);
                                let st = self.optimizer_status.as_deref().unwrap_or("Optimizing image file size...");
                                ui.label(
                                    egui::RichText::new(st)
                                        .small()
                                        .monospace()
                                        .strong()
                                        .color(Color32::from_rgb(45, 212, 191)),
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
                                        .color(Color32::from_rgb(52, 211, 153)),
                                );

                                ui.add_space(8.0);
                                let apply_opt_btn = egui::Button::new(
                                    egui::RichText::new("✔ Apply to Canvas").small().strong().color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(16, 185, 129))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(52, 211, 153)))
                                .rounding(Rounding::same(4.0));
                                if ui.add(apply_opt_btn).on_hover_text("Apply optimized image to canvas").clicked() {
                                    apply_clicked = true;
                                }

                                let copy_opt_btn = egui::Button::new(
                                    egui::RichText::new("📋 Copy to Slot").small().strong().color(Color32::from_rgb(241, 245, 249)),
                                )
                                .fill(Color32::from_rgb(88, 28, 135))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(192, 132, 252)))
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

    fn render_palette_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Frame::none()
            .fill(Color32::from_rgb(22, 27, 39))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🎨 Dominant Palette:")
                            .strong()
                            .small()
                            .color(Color32::from_rgb(244, 114, 182)),
                    );

                    let colors_len = self.palette_state.extracted_colors.len();
                    let count_str = if colors_len == 0 {
                        "No palette extracted".to_string()
                    } else {
                        format!("{} dominant hues", colors_len)
                    };
                    ui.label(
                        egui::RichText::new(count_str)
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    ui.add_space(6.0);

                    // Re-extract button
                    let re_btn = egui::Button::new(
                        egui::RichText::new("↺ Refresh").small().color(Color32::from_rgb(203, 213, 225)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                    .rounding(Rounding::same(4.0));
                    if ui.add(re_btn).on_hover_text("Re-extract dominant color palette from image").clicked() {
                        self.run_palette_extraction();
                    }

                    // Copy All Hex CSV
                    if !self.palette_state.extracted_colors.is_empty() {
                        let csv_btn = egui::Button::new(
                            egui::RichText::new("📋 Copy Hex CSV").small().color(Color32::from_rgb(203, 213, 225)),
                        )
                        .fill(Color32::from_rgb(30, 41, 59))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(csv_btn).on_hover_text("Copy all hex codes as comma-separated list").clicked() {
                            let csv = self.palette_state.extracted_colors.iter().map(|c| c.hex.as_str()).collect::<Vec<_>>().join(", ");
                            ctx.output_mut(|o| o.copied_text = csv.clone());
                            let csv_for_cmd = csv.clone();
                            std::thread::spawn(move || {
                                let _ = Command::new("wl-copy").arg(&csv_for_cmd).output();
                                let _ = Command::new("xclip").arg("-selection").arg("clipboard").arg(&csv_for_cmd).output();
                            });
                            self.show_toast("📋 Copied all palette hex codes to clipboard");
                        }

                        // Copy CSS Variables
                        let css_btn = egui::Button::new(
                            egui::RichText::new("📋 Copy CSS").small().color(Color32::from_rgb(203, 213, 225)),
                        )
                        .fill(Color32::from_rgb(30, 41, 59))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                        .rounding(Rounding::same(4.0));
                        if ui.add(css_btn).on_hover_text("Copy palette formatted as CSS custom properties").clicked() {
                            let mut css = String::new();
                            for (i, c) in self.palette_state.extracted_colors.iter().enumerate() {
                                css.push_str(&format!("  --color-{}: {}; /* {:.1}% */\n", i + 1, c.hex, c.percentage));
                            }
                            ctx.output_mut(|o| o.copied_text = css.clone());
                            let css_for_cmd = css.clone();
                            std::thread::spawn(move || {
                                let _ = Command::new("wl-copy").arg(&css_for_cmd).output();
                                let _ = Command::new("xclip").arg("-selection").arg("clipboard").arg(&css_for_cmd).output();
                            });
                            self.show_toast("📋 Copied palette CSS variables to clipboard");
                        }
                    }

                    ui.add_space(8.0);

                    // Dominant color swatches
                    for pal in self.palette_state.extracted_colors.clone() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, Rounding::same(3.0), pal.color);
                        ui.painter().rect_stroke(rect, Rounding::same(3.0), Stroke::new(1.0, Color32::from_rgb(71, 85, 105)));

                        let tip = format!("{} ({:.1}%) - Click to copy", pal.hex, pal.percentage);
                        if resp.on_hover_text(tip).clicked() {
                            self.copy_color_to_clipboard(pal.color, ctx);
                        }

                        ui.label(
                            egui::RichText::new(format!("{} ({:.0}%)", pal.hex, pal.percentage))
                                .small()
                                .monospace()
                                .color(Color32::from_rgb(226, 232, 240)),
                        );
                        ui.add_space(4.0);
                    }
                });
            });
    }

    fn render_remove_bg_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
        }

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 27, 39))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 55, 75)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.bg_removal_in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(Color32::from_rgb(168, 85, 247)));
                        ui.add_space(6.0);
                        let st = self.bg_removal_status.as_deref().unwrap_or("Segmenting background with FeyNoBg...");
                        ui.label(
                            egui::RichText::new(st)
                                .small()
                                .monospace()
                                .strong()
                                .color(Color32::from_rgb(192, 132, 252)),
                        );
                    } else {
                        let has_prompt = !self.multimatte_prompt.trim().is_empty();
                        let btn_text = if has_prompt {
                            format!("✨ MultiMatte ('{}')", self.multimatte_prompt.trim())
                        } else {
                            "✨ Cut Out BG (Enter)".to_string()
                        };

                        let cut_btn = egui::Button::new(
                            egui::RichText::new(btn_text)
                                .small()
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(if has_prompt { Color32::from_rgb(147, 51, 234) } else { Color32::from_rgb(126, 34, 206) })
                        .stroke(Stroke::new(1.0, Color32::from_rgb(192, 132, 252)))
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
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                    }

                    if let Some(err) = &self.bg_removal_error {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .small()
                                .strong()
                                .color(Color32::from_rgb(248, 113, 113)),
                        );
                        if err.contains("nobg") || err.contains("pip") {
                            let copy_install_btn = egui::Button::new(
                                egui::RichText::new("📋 Copy Install Cmd").small().strong().color(Color32::from_rgb(224, 231, 255)),
                            )
                            .fill(Color32::from_rgb(30, 41, 59))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(99, 102, 241)))
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
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(Color32::from_rgb(245, 158, 11)),
                            )
                            .fill(Color32::from_rgb(30, 41, 59))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }

                        // Direct Slot Push buttons
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Right Hand").small().color(Color32::from_rgb(52, 211, 153)),
                        )
                        .fill(Color32::from_rgb(15, 23, 42))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(16, 185, 129)))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_rh).on_hover_text("Push current cutout directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Left Hand").small().color(Color32::from_rgb(129, 140, 248)),
                        )
                        .fill(Color32::from_rgb(15, 23, 42))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(99, 102, 241)))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_lh).on_hover_text("Push current cutout directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });
                });
            });
    }

    fn render_depth_map_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
        }

        egui::Frame::none()
            .fill(Color32::from_rgb(17, 24, 39))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, Color32::from_rgb(30, 58, 138)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.bg_removal_in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(Color32::from_rgb(96, 165, 250)));
                        ui.add_space(6.0);
                        let st = self.bg_removal_status.as_deref().unwrap_or("Estimating depth map with Depth-Anything...");
                        ui.label(
                            egui::RichText::new(st)
                                .small()
                                .monospace()
                                .strong()
                                .color(Color32::from_rgb(147, 197, 253)),
                        );
                    } else {
                        let depth_btn = egui::Button::new(
                            egui::RichText::new("🌊 Generate Depth Map (Enter)")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(238, 242, 255)),
                        )
                        .fill(Color32::from_rgb(30, 58, 138)) // Deep blue
                        .stroke(Stroke::new(1.0, Color32::from_rgb(96, 165, 250)))
                        .rounding(Rounding::same(4.0));

                        if ui.add(depth_btn).on_hover_text("Generate AI metric depth map with Depth-Anything V2 neural network (Enter)").clicked() {
                            self.run_depth_estimation();
                        }

                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Depth-Anything V2 • Metric Monocular Depth Estimation • CUDA")
                                .small()
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                    }

                    if let Some(err) = &self.bg_removal_error {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .small()
                                .strong()
                                .color(Color32::from_rgb(248, 113, 113)),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(Color32::from_rgb(245, 158, 11)),
                            )
                            .fill(Color32::from_rgb(30, 41, 59))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }

                        // Direct Slot Push buttons
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Right Hand").small().color(Color32::from_rgb(52, 211, 153)),
                        )
                        .fill(Color32::from_rgb(15, 23, 42))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(16, 185, 129)))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_rh).on_hover_text("Push current depth map directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Left Hand").small().color(Color32::from_rgb(129, 140, 248)),
                        )
                        .fill(Color32::from_rgb(15, 23, 42))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(99, 102, 241)))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_lh).on_hover_text("Push current depth map directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });
                });
            });
    }

    fn render_viewport(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let avail_size = ui.available_size() - Vec2::new(0.0, 32.0);
        if !self.auto_fitted && avail_size.x > 50.0 && avail_size.y > 50.0 {
            self.fit_to_view(avail_size);
            self.auto_fitted = true;
        }

        let (response, painter) = ui.allocate_painter(avail_size, egui::Sense::click_and_drag());
        let canvas_rect = response.rect;

        // Background canvas fill
        painter.rect_filled(canvas_rect, Rounding::same(6.0), Color32::from_rgb(12, 14, 20));

        // Panning interaction (drag) when in View, Resize, Palette, RemoveBg, or DepthMap mode or middle-click dragged
        let middle_drag = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let can_drag_pan = self.active_mode == ImageToolMode::View
            || self.active_mode == ImageToolMode::Resize
            || self.active_mode == ImageToolMode::Palette
            || self.active_mode == ImageToolMode::RemoveBg
            || self.active_mode == ImageToolMode::DepthMap;
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
            draw_checkerboard(&painter, img_rect, self.zoom);

            // Draw image mesh
            let mut mesh = egui::Mesh::with_texture(texture.id());
            mesh.add_rect_with_uv(
                img_rect,
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.add(egui::Shape::mesh(mesh));

            // Draw crisp subtle border around the image bounds
            painter.rect_stroke(
                img_rect,
                Rounding::ZERO,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 40)),
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
                self.render_crop_overlay(ui, &painter, &response, img_rect);
            } else if self.active_mode == ImageToolMode::Redact {
                self.render_redact_overlay(ui, &painter, &response, img_rect);
            } else if self.active_mode == ImageToolMode::PickColor {
                self.render_pick_color_overlay(ui, &painter, &response, img_rect);
            }
        }
    }

    fn render_crop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
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
        let grid_stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 60));
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
        painter.rect_stroke(crop_screen_rect, Rounding::ZERO, Stroke::new(1.5, Color32::WHITE));

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
            painter.rect_filled(handle_rect, Rounding::same(1.0), Color32::WHITE);
            painter.rect_stroke(handle_rect, Rounding::same(1.0), Stroke::new(1.0, Color32::from_rgb(15, 23, 42)));
        }

        // 5. Floating Dimensions Badge
        let px_w = ((norm.max.x - norm.min.x) * self.current_rgba.width() as f32).round() as u32;
        let px_h = ((norm.max.y - norm.min.y) * self.current_rgba.height() as f32).round() as u32;
        let badge_str = format!("{} × {} px ({})", px_w, px_h, format_aspect_ratio(px_w, px_h));
        let badge_pos = Pos2::new(
            crop_screen_rect.center().x,
            (crop_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
        );
        let galley = ui.painter().layout_no_wrap(
            badge_str,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(241, 245, 249),
        );
        let badge_rect = Rect::from_center_size(badge_pos, galley.size() + Vec2::new(12.0, 4.0));
        painter.rect_filled(badge_rect, Rounding::same(4.0), Color32::from_rgba_unmultiplied(15, 23, 42, 230));
        painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, Color32::from_rgb(52, 211, 153)));
        painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, Color32::from_rgb(241, 245, 249));

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

    fn render_redact_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
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
                    Stroke::new(2.0, Color32::from_rgb(239, 68, 68)),
                );

                // 8 Grab Handles
                let handles = get_box_handles(box_screen_rect);
                for (_, pos, _) in &handles {
                    let handle_rect = Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
                    painter.rect_filled(handle_rect, Rounding::same(1.0), Color32::WHITE);
                    painter.rect_stroke(handle_rect, Rounding::same(1.0), Stroke::new(1.0, Color32::BLACK));
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
                    Color32::WHITE,
                );
                let badge_rect = Rect::from_center_size(badge_pos, galley.size() + Vec2::new(12.0, 4.0));
                painter.rect_filled(badge_rect, Rounding::same(4.0), Color32::from_rgba_unmultiplied(15, 23, 42, 230));
                painter.rect_stroke(badge_rect, Rounding::same(4.0), Stroke::new(1.0, Color32::from_rgb(239, 68, 68)));
                painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, Color32::WHITE);
            } else {
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 60)),
                );
            }
        }

        // 2. Draw in-progress creating box
        if let Some(RedactionDrag::Creating(start_norm, curr_norm)) = self.redaction_state.active_drag {
            let start_s = to_screen(start_norm);
            let curr_s = to_screen(curr_norm);
            let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
            painter.rect_filled(in_progress_rect, Rounding::ZERO, Color32::from_rgba_unmultiplied(0, 0, 0, 210));
            painter.rect_stroke(in_progress_rect, Rounding::ZERO, Stroke::new(1.5, Color32::from_rgb(239, 68, 68)));
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

                // Card background & sleek indigo border
                painter.rect_filled(card_rect, Rounding::same(8.0), Color32::from_rgba_unmultiplied(15, 23, 42, 245));
                painter.rect_stroke(card_rect, Rounding::same(8.0), Stroke::new(1.5, Color32::from_rgb(99, 102, 241)));

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
                            painter.rect_stroke(cell_rect, Rounding::ZERO, Stroke::new(2.0, Color32::WHITE));
                            painter.rect_stroke(cell_rect.expand(1.0), Rounding::ZERO, Stroke::new(1.0, Color32::BLACK));
                        }
                    }
                }

                // Color Swatch below grid
                let swatch_y = grid_origin.y + grid_w + 8.0;
                let swatch_rect = Rect::from_min_size(Pos2::new(card_min.x + 10.0, swatch_y), Vec2::new(card_w - 20.0, 16.0));
                painter.rect_filled(swatch_rect, Rounding::same(3.0), col);
                painter.rect_stroke(swatch_rect, Rounding::same(3.0), Stroke::new(1.0, Color32::from_rgb(148, 163, 184)));

                // Hex Code Text
                let hex_str = format!("#{:02X}{:02X}{:02X}", pixel_data[0], pixel_data[1], pixel_data[2]);
                let hex_galley = painter.layout_no_wrap(
                    hex_str,
                    egui::FontId::monospace(12.0),
                    Color32::WHITE,
                );
                painter.galley(Pos2::new(card_min.x + 10.0, swatch_y + 20.0), hex_galley, Color32::WHITE);

                // RGB & Coordinate Info
                let info_str = format!("RGB({}, {}, {}) | {}, {}", pixel_data[0], pixel_data[1], pixel_data[2], px, py);
                let info_galley = painter.layout_no_wrap(
                    info_str,
                    egui::FontId::proportional(9.5),
                    Color32::from_rgb(148, 163, 184),
                );
                painter.galley(Pos2::new(card_min.x + 10.0, swatch_y + 36.0), info_galley, Color32::from_rgb(148, 163, 184));

                // Click to copy hint
                let hint_galley = painter.layout_no_wrap(
                    "🖱 Click to copy".to_string(),
                    egui::FontId::proportional(9.0),
                    Color32::from_rgb(56, 189, 248),
                );
                painter.galley(Pos2::new(card_min.x + 10.0, swatch_y + 49.0), hint_galley, Color32::from_rgb(56, 189, 248));
            } else {
                self.pick_color_state.hovered_color = None;
                self.pick_color_state.hovered_pixel = None;
            }
        } else {
            self.pick_color_state.hovered_color = None;
            self.pick_color_state.hovered_pixel = None;
        }
    }

    fn render_footer(&mut self, ui: &mut egui::Ui, now: f64) {
        egui::Frame::none()
            .fill(Color32::from_rgb(18, 22, 32))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Help hints
                    let hint_text = match self.active_mode {
                        ImageToolMode::Crop => "🖱 Drag crop box/handles | Enter: Apply | Ctrl+C: Copy Slot | Ctrl+S: Save As | Esc: Cancel",
                        ImageToolMode::Transform => "🖱 Rotate / Flip buttons above | Ctrl+Z: Undo | Ctrl+C: Copy Slot | Ctrl+S: Save As",
                        ImageToolMode::Redact => "🖱 Drag on image: Place black bar | Del: Remove | Enter: Burn/Apply | Ctrl+C: Copy Slot | Ctrl+S: Save As",
                        ImageToolMode::PickColor => "🖱 Hover: Magnifying loupe | Click: Copy Hex code to clipboard | Ctrl+C: Copy Slot",
                        _ => "🖱 Scroll: Zoom | Drag: Pan | Ctrl+C: Copy to Slot | Ctrl+S: Save As | F: Fit | Esc: Close",
                    };

                    ui.label(
                        egui::RichText::new(hint_text)
                            .small()
                            .color(Color32::from_rgb(100, 116, 139)),
                    );

                    // Toast message
                    if let Some((ref msg, toast_time)) = self.status_toast {
                        let elapsed = now - toast_time;
                        if elapsed < 5.0 {
                            let alpha = ((5.0 - elapsed) / 1.0).clamp(0.0, 1.0);
                            let col = Color32::from_rgba_unmultiplied(52, 211, 153, (alpha * 255.0) as u8);
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new(msg).small().strong().color(col));
                        } else {
                            self.status_toast = None;
                        }
                    }
                });
            });
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

fn draw_checkerboard(painter: &egui::Painter, rect: Rect, _zoom: f32) {
    let cell_size = 16.0;
    let dark_col = Color32::from_rgb(22, 26, 36);
    let light_col = Color32::from_rgb(32, 38, 52);

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
        for _ in 0..200 {
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
}

