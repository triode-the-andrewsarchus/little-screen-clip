//! Image Studio tool implementations.
//!
//! Each tool owns its UI and operations in a focused module. Shared viewport
//! composition belongs in `shell::StudioPanelLayout`, not in individual tools.
#![allow(unused_imports)] // Re-exports preserve the parent module's API and test surface.

use crate::icons::Icon;

/// Process a thumbnail batch in order, checking its per-run cancellation token
/// between items and stopping if publishing a result is no longer useful.
pub(super) fn run_thumbnail_items<T>(
    items: impl IntoIterator<Item = T>,
    is_cancelled: impl Fn() -> bool,
    mut process_and_publish: impl FnMut(T) -> bool,
) {
    for item in items {
        if is_cancelled() || !process_and_publish(item) {
            break;
        }
    }
}

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
    /// Interactive screenshot markup (arrow, circle, rectangle, line, text, badge).
    Markup,
}

#[cfg(test)]
mod thumbnail_batch_tests {
    use super::run_thumbnail_items;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn thumbnail_worker_stops_before_the_next_item_after_cancellation() {
        let cancelled = AtomicBool::new(false);
        let processed = AtomicUsize::new(0);
        let published = AtomicUsize::new(0);

        run_thumbnail_items(
            0..8,
            || cancelled.load(Ordering::Acquire),
            |item| {
                processed.fetch_add(1, Ordering::Relaxed);
                published.fetch_add(1, Ordering::Relaxed);
                if item == 0 {
                    cancelled.store(true, Ordering::Release);
                }
                true
            },
        );

        assert_eq!(processed.load(Ordering::Relaxed), 1);
        assert_eq!(published.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn thumbnail_worker_stops_when_receiver_rejects_a_result() {
        let processed = AtomicUsize::new(0);
        run_thumbnail_items(
            0..8,
            || false,
            |_| {
                processed.fetch_add(1, Ordering::Relaxed);
                false
            },
        );
        assert_eq!(processed.load(Ordering::Relaxed), 1);
    }
}

impl ImageToolMode {
    /// Human-readable toolbar label.
    pub fn label(&self) -> &'static str {
        match self {
            ImageToolMode::View => "[V]iew",
            ImageToolMode::Crop => "[C]rop",
            ImageToolMode::Deskew => "Des[K]ew",
            ImageToolMode::Transform => "[F]lip/Rotate",
            ImageToolMode::Redact => "[X] Redact",
            ImageToolMode::Markup => "[M]arkup",
            ImageToolMode::PickColor => "[P]ick Color",
            ImageToolMode::Resize => "Re[S]ize",
            ImageToolMode::Convert => "Conver[T]",
            ImageToolMode::Ocr => "[O]CR",
            ImageToolMode::Barcode => "[B]arcode",
            ImageToolMode::Palette => "Palette",
            ImageToolMode::RemoveBg => "[R]emove BG",
            ImageToolMode::DepthMap => "[D]epth Map",
            ImageToolMode::DecorrelationStretch => "Decorrelation Stretch",
            ImageToolMode::FilmSim => "Film [L]ook",
            ImageToolMode::CreativeFx => "Creative FX",
        }
    }

    /// SVG vector icon for tool mode tab.
    pub fn icon(&self) -> Icon {
        match self {
            ImageToolMode::View => Icon::Eye,
            ImageToolMode::Crop => Icon::Crop,
            ImageToolMode::Deskew => Icon::Grid,
            ImageToolMode::Transform => Icon::RotateCw,
            ImageToolMode::Redact => Icon::Shield,
            ImageToolMode::Markup => Icon::Pencil,
            ImageToolMode::PickColor => Icon::Eyedropper,
            ImageToolMode::Resize => Icon::Resize,
            ImageToolMode::Convert => Icon::Layers,
            ImageToolMode::Ocr => Icon::Text,
            ImageToolMode::Barcode => Icon::Barcode,
            ImageToolMode::Palette => Icon::Palette,
            ImageToolMode::RemoveBg => Icon::Scissors,
            ImageToolMode::DepthMap => Icon::Layers,
            ImageToolMode::DecorrelationStretch => Icon::Droplet,
            ImageToolMode::FilmSim => Icon::Film,
            ImageToolMode::CreativeFx => Icon::Wand,
        }
    }
}

mod background_removal;
pub(super) use background_removal::BgRemovalMsg;
pub(crate) use background_removal::ImageToolProcessControl;
pub(crate) use background_removal::{
    clear_image_tool_pid_if_owned, spawn_image_tool_child, terminate_image_tool_process,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum BgRemovalOperation {
    #[default]
    BackgroundRemoval,
    DepthMap,
}

mod barcode;
pub(super) use barcode::{BarcodeScanMsg, BarcodeScanState};
mod convert;
pub(super) use convert::{ConvertState, ConvertTargetFormat};
mod creative_fx;
pub(super) use creative_fx::{CreativeFxMsg, CreativeFxState, CreativeFxThumbnailMsg};
mod crop;
pub(super) use crop::{CropHandle, CropRatio, CropState, get_box_handles, resize_norm_rect};
mod decorrelation_stretch;
pub(super) use decorrelation_stretch::DecorrelationStretchState;
mod depth_map;
mod deskew;
pub(super) use deskew::DeskewState;
mod film_look;
pub(super) use film_look::{
    FilmSimMsg, FilmSimState, FilmSimThumbnailMsg, FilmStockEntry, LightLeakPreset,
    all_film_stocks, apply_film_sim_post_adjustments,
};
mod layout;
mod markup;
pub(super) use markup::{MarkupDrag, MarkupShape, MarkupState, MarkupToolType, rasterize_markup};
mod ocr;
pub(super) use ocr::{OcrEngine, OcrMsg, OcrState};
mod palette;
pub(super) use palette::{PaletteColor, PaletteState, extract_palette};
mod pick_color;
pub(super) use pick_color::PickColorState;
mod redact;
pub(super) use redact::{RedactionBox, RedactionDrag, RedactionState};
mod resize;
pub(super) use resize::OptimizerMsg;
pub(crate) use resize::OptimizerRequestStamp;
pub(super) use resize::{
    FileSizeOptimizationResult, ResizeFilter, ResizeState, ResizeSubTab, TargetFormat,
};
pub(super) use resize::{optimize_to_target_size, optimize_to_target_size_with_progress};
mod transform;
mod view;
