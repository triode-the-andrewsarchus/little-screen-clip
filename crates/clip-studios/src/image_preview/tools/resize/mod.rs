mod optimizer_jobs;
mod pixel_resize;
mod studio;
mod target_size;

use super::super::*;
use target_size::optimize_to_target_size_cancellable;
#[cfg(test)]
pub use target_size::{optimize_to_target_size, optimize_to_target_size_with_progress};

#[derive(Debug)]
pub enum OptimizerMsg {
    Progress(String),
    Done(FileSizeOptimizationResult),
    Failed(String),
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OptimizerRequestStamp {
    image: ImageInputStamp,
    target_bytes: usize,
    format: TargetFormat,
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
    pub encoded_data: Arc<Vec<u8>>,
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
    pub(crate) last_result_request_stamp: Option<OptimizerRequestStamp>,
    pub(crate) optimizer_request_stamp: Option<OptimizerRequestStamp>,
    pub is_optimizing: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct PixelResizeStamp {
    image: ImageInputStamp,
    width: u32,
    height: u32,
    filter: ResizeFilter,
}

#[derive(Clone)]
struct PixelResizeRequest {
    source: Arc<RgbaImage>,
    redactions: Vec<RedactionBox>,
    stamp: PixelResizeStamp,
}

struct PixelResizeMessage {
    stamp: PixelResizeStamp,
    result: Result<RgbaImage, String>,
}

#[derive(Default)]
pub struct PixelResizeJobState {
    rx: Option<
        std::sync::mpsc::Receiver<Result<Option<PixelResizeMessage>, little_clip_core::JobError>>,
    >,
    ticket: Option<little_clip_core::JobTicket>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    stamp: Option<PixelResizeStamp>,
    pub(crate) retry_at: Option<std::time::Instant>,
    retry_request: Option<PixelResizeRequest>,
    pub(crate) pending: bool,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
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
            last_result_request_stamp: None,
            optimizer_request_stamp: None,
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
            let new_h = ((self.target_width as f32) / self.aspect_ratio_base)
                .round()
                .max(1.0) as u32;
            self.target_height = new_h;
            self.height_input = new_h.to_string();
        }
    }

    pub fn set_height(&mut self, new_h: u32) {
        self.target_height = new_h.max(1);
        self.height_input = self.target_height.to_string();
        if self.lock_aspect && self.aspect_ratio_base > 0.0 {
            let new_w = ((self.target_height as f32) * self.aspect_ratio_base)
                .round()
                .max(1.0) as u32;
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

#[cfg(test)]
mod input_stamp_tests;
