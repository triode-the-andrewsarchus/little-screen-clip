use super::super::*;

mod basic_controls;
mod cmyk_halftone;
mod dither_palette;
mod lens_aberration;
mod masks;
mod ntsc;
mod pixel_sort;
mod render;
mod structure;
mod thumbnails;
mod trichromatic;
mod viewport;

use crate::shell::{render_context_slider, slider_double_clicked};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};

/// Message sent back from background thread rendering full creative FX.
pub enum CreativeFxMsg {
    Done {
        stamp: CreativeFxRenderStamp,
        rgba: RgbaImage,
        duration_ms: u128,
    },
    Error {
        stamp: CreativeFxRenderStamp,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreativeFxRenderStamp {
    pixel_revision: u64,
    fx_type: CreativeFxType,
    params: CreativeFxParams,
    mask_cache_revision: u64,
    params_revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct CreativeFxMaskStamp {
    input: ImageInputStamp,
    fx_type: CreativeFxType,
    selection_revision: u64,
    params_revision: u64,
}

#[derive(Debug)]
pub struct CreativeFxMaskMsg {
    stamp: CreativeFxMaskStamp,
    result: Result<RgbaImage, String>,
}

#[derive(Clone)]
struct CreativeFxMaskRequest {
    snapshot: DisplayImageSnapshotRequest,
    cmd_bin: String,
    cmd_args: Vec<String>,
    stamp: CreativeFxMaskStamp,
}

fn send_creative_fx_mask_result(
    tx: &std::sync::mpsc::Sender<CreativeFxMaskMsg>,
    stamp: &CreativeFxMaskStamp,
    result: Result<RgbaImage, String>,
) {
    let _ = tx.send(CreativeFxMaskMsg {
        stamp: stamp.clone(),
        result,
    });
}

#[cfg(test)]
mod ownership_tests;

/// Message sent back from low-priority background thread rendering mini 42x28 thumbnails.
pub struct CreativeFxThumbnailMsg {
    pub epoch: u64,
    pub pixel_revision: u64,
    pub fx_type: CreativeFxType,
    pub rgba: RgbaImage,
}

fn creative_fx_choice_button(
    label: impl Into<String>,
    selected: bool,
    palette: &AppPalette,
) -> egui::Button<'static> {
    let fill = if selected {
        hex_to_c32(&palette.harmonies.base)
    } else {
        hex_to_c32(&palette.colors.surface_panel)
    };
    let foreground = if selected {
        contrast_text_c32(fill, None, palette)
    } else {
        hex_to_c32(&palette.colors.text_secondary)
    };
    egui::Button::new(
        egui::RichText::new(label)
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .strong()
            .color(foreground),
    )
    .fill(fill)
    .stroke(Stroke::new(1.0_f32, fill))
    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
}

fn creative_fx_icon(effect: CreativeFxType) -> crate::icons::Icon {
    use crate::icons::Icon;
    match effect {
        CreativeFxType::BasicControls => Icon::Settings,
        CreativeFxType::LensAberration => Icon::Search,
        CreativeFxType::Trichromatic => Icon::Camera,
        CreativeFxType::PixelSort => Icon::Zap,
        CreativeFxType::CmykHalftone => Icon::Layers,
        CreativeFxType::DitherPalette => Icon::Grid,
        CreativeFxType::Ntsc => Icon::Tv,
        CreativeFxType::Structure => Icon::ShapeRect,
    }
}

fn render_creative_fx_mask_loading(ui: &mut egui::Ui, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add(
            egui::Spinner::new()
                .size(crate::ui_ext::CONTROL_ICON_SIZE)
                .color(color),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(label)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .monospace()
                .color(color),
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn render_creative_fx_mask_button(
    ui: &mut egui::Ui,
    icon: crate::icons::Icon,
    label: &str,
    active: bool,
    loading: bool,
    width: f32,
    palette: &AppPalette,
) -> egui::Response {
    let fill = if active {
        hex_to_c32(&palette.diff_add)
    } else {
        hex_to_c32(&palette.colors.surface_panel)
    };
    let foreground = contrast_text_c32(
        fill,
        Some(hex_to_c32(&palette.colors.text_primary)),
        palette,
    );
    let button = egui::Button::image_and_text(
        icon.image_tinted(crate::ui_ext::CONTROL_ICON_SIZE, foreground),
        egui::RichText::new(label)
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .strong()
            .color(foreground),
    )
    .fill(fill)
    .stroke(Stroke::new(1.0_f32, fill))
    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
    .min_size(Vec2::new(width, crate::ui_ext::CONTROL_HEIGHT));
    ui.add_enabled(!loading, button)
}

/// All state for the Creative FX tab.
pub struct CreativeFxState {
    pub selected_fx: CreativeFxType,
    pub basic_params: BasicControlsParams,
    pub lens_params: LensAberrationParams,
    pub trichromatic_params: TrichromaticParams,
    pub pixel_sort_params: PixelSortParams,
    pub cmyk_params: CmykHalftoneParams,
    pub dither_params: DitherPaletteParams,
    pub ntsc_params: NtscParams,
    pub structure_params: StructureParams,

    pub selected_preset_name: String,
    params_revision: u64,
    selection_revision: u64,

    // Preview state
    pub preview_rgba: Option<RgbaImage>,
    pub preview_texture: Option<egui::TextureHandle>,
    pub dirty_preview_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub last_render_duration_ms: u128,

    // Async render worker (Main preview image)
    pub is_rendering: bool,
    pub render_rx: Option<std::sync::mpsc::Receiver<Result<Option<CreativeFxMsg>, JobError>>>,
    render_stamp: Option<CreativeFxRenderStamp>,
    preview_stamp: Option<CreativeFxRenderStamp>,
    render_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    render_job_ticket: Option<JobTicket>,
    pub render_retry_at: Option<std::time::Instant>,
    pub render_error: Option<String>,

    // Async thumbnail worker (Sidebar 42x28 mini-photos for each FX)
    pub fx_thumbnails: std::collections::HashMap<CreativeFxType, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(CreativeFxType, RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<CreativeFxThumbnailMsg>>,
    thumbnail_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    pub thumbnail_epoch: u64,
    pub thumbnail_pixel_revision: Option<u64>,
    pub(in crate::image_preview) thumbnail_cancel:
        Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    thumbnail_job_ticket: Option<JobTicket>,
    pub thumbnail_retry_at: Option<std::time::Instant>,
    pub thumbnail_queue_status: Option<String>,
    pub is_generating_thumbnails: bool,

    // Debounce timer
    pub params_dirty_at: Option<std::time::Instant>,
    pub auto_fitted: bool,

    // Depth mask state
    pub cached_depth_map: Option<RgbaImage>,
    pub(crate) depth_map_input_stamp: Option<ImageInputStamp>,
    pub(super) mask_cache_revision: u64,
    pub depth_estimation_in_progress: bool,
    pub depth_estimation_rx: Option<std::sync::mpsc::Receiver<CreativeFxMaskMsg>>,
    depth_estimation_stamp: Option<CreativeFxMaskStamp>,
    depth_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    pub(super) depth_job_ticket: Option<JobTicket>,
    depth_request: Option<CreativeFxMaskRequest>,
    pub depth_retry_at: Option<std::time::Instant>,
    pub depth_queue_status: Option<String>,
    pub(super) depth_process_control: Option<std::sync::Arc<ImageToolProcessControl>>,
    pub depth_estimation_error: Option<String>,

    // Background-removal alpha mask state
    pub cached_background_removal: Option<RgbaImage>,
    pub(super) background_removal_input_stamp: Option<ImageInputStamp>,
    pub alpha_mask_in_progress: bool,
    pub alpha_mask_rx: Option<std::sync::mpsc::Receiver<CreativeFxMaskMsg>>,
    alpha_mask_stamp: Option<CreativeFxMaskStamp>,
    alpha_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    pub(super) alpha_job_ticket: Option<JobTicket>,
    alpha_request: Option<CreativeFxMaskRequest>,
    pub alpha_retry_at: Option<std::time::Instant>,
    pub alpha_queue_status: Option<String>,
    pub(super) alpha_process_control: Option<std::sync::Arc<ImageToolProcessControl>>,
    pub alpha_mask_error: Option<String>,
}

impl Default for CreativeFxState {
    fn default() -> Self {
        Self {
            selected_fx: CreativeFxType::BasicControls,
            basic_params: BasicControlsParams::default(),
            lens_params: LensAberrationParams::default(),
            trichromatic_params: TrichromaticParams::default(),
            pixel_sort_params: PixelSortParams::default(),
            cmyk_params: CmykHalftoneParams::default(),
            dither_params: DitherPaletteParams::default(),
            ntsc_params: NtscParams::default(),
            structure_params: StructureParams::default(),
            selected_preset_name: "Default".to_string(),
            params_revision: 0,
            selection_revision: 0,
            preview_rgba: None,
            preview_texture: None,
            dirty_preview_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            last_render_duration_ms: 0,
            is_rendering: false,
            render_rx: None,
            render_stamp: None,
            preview_stamp: None,
            render_cancel: None,
            render_job_ticket: None,
            render_retry_at: None,
            render_error: None,
            fx_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_job_rx: None,
            thumbnail_epoch: 0,
            thumbnail_pixel_revision: None,
            thumbnail_cancel: None,
            thumbnail_job_ticket: None,
            thumbnail_retry_at: None,
            thumbnail_queue_status: None,
            is_generating_thumbnails: false,
            params_dirty_at: None,
            auto_fitted: false,
            cached_depth_map: None,
            depth_map_input_stamp: None,
            mask_cache_revision: 0,
            depth_estimation_in_progress: false,
            depth_estimation_rx: None,
            depth_estimation_stamp: None,
            depth_job_rx: None,
            depth_job_ticket: None,
            depth_request: None,
            depth_retry_at: None,
            depth_queue_status: None,
            depth_process_control: None,
            depth_estimation_error: None,
            cached_background_removal: None,
            background_removal_input_stamp: None,
            alpha_mask_in_progress: false,
            alpha_mask_rx: None,
            alpha_mask_stamp: None,
            alpha_job_rx: None,
            alpha_job_ticket: None,
            alpha_request: None,
            alpha_retry_at: None,
            alpha_queue_status: None,
            alpha_process_control: None,
            alpha_mask_error: None,
        }
    }
}

impl CreativeFxState {
    fn mark_custom(&mut self) {
        self.selected_preset_name = "Custom".to_string();
        self.mark_dirty();
    }

    pub fn mark_dirty(&mut self) {
        self.cancel_render();
        self.params_dirty_at = Some(std::time::Instant::now());
        self.params_revision = self.params_revision.wrapping_add(1);
    }

    pub(crate) fn cancel_render(&mut self) {
        let was_retry_waiting = self.render_retry_at.take().is_some();
        if let Some(ticket) = self.render_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.render_rx = None;
        self.render_stamp = None;
        self.is_rendering = false;
        if was_retry_waiting {
            self.render_error = None;
        }
    }

    pub(in crate::image_preview) fn select_fx(&mut self, fx_type: CreativeFxType) {
        if self.selected_fx != fx_type {
            self.selected_fx = fx_type;
            self.selection_revision = self.selection_revision.wrapping_add(1);
            self.mark_dirty();
        }
    }

    pub fn depth_mask_for_active_fx(&self) -> &DepthMaskConfig {
        match self.selected_fx {
            CreativeFxType::BasicControls => &self.basic_params.depth_mask,
            CreativeFxType::LensAberration => &self.lens_params.depth_mask,
            CreativeFxType::Trichromatic => &self.trichromatic_params.depth_mask,
            CreativeFxType::PixelSort => &self.pixel_sort_params.depth_mask,
            CreativeFxType::CmykHalftone => &self.cmyk_params.depth_mask,
            CreativeFxType::DitherPalette => &self.dither_params.depth_mask,
            CreativeFxType::Ntsc => &self.ntsc_params.depth_mask,
            CreativeFxType::Structure => &self.structure_params.depth_mask,
        }
    }

    pub fn depth_mask_for_active_fx_mut(&mut self) -> &mut DepthMaskConfig {
        match self.selected_fx {
            CreativeFxType::BasicControls => &mut self.basic_params.depth_mask,
            CreativeFxType::LensAberration => &mut self.lens_params.depth_mask,
            CreativeFxType::Trichromatic => &mut self.trichromatic_params.depth_mask,
            CreativeFxType::PixelSort => &mut self.pixel_sort_params.depth_mask,
            CreativeFxType::CmykHalftone => &mut self.cmyk_params.depth_mask,
            CreativeFxType::DitherPalette => &mut self.dither_params.depth_mask,
            CreativeFxType::Ntsc => &mut self.ntsc_params.depth_mask,
            CreativeFxType::Structure => &mut self.structure_params.depth_mask,
        }
    }

    pub fn current_params(&self) -> CreativeFxParams {
        match self.selected_fx {
            CreativeFxType::BasicControls => {
                CreativeFxParams::BasicControls(self.basic_params.clone())
            }
            CreativeFxType::LensAberration => CreativeFxParams::LensAberration(self.lens_params),
            CreativeFxType::Trichromatic => {
                CreativeFxParams::Trichromatic(self.trichromatic_params)
            }
            CreativeFxType::PixelSort => CreativeFxParams::PixelSort(self.pixel_sort_params),
            CreativeFxType::CmykHalftone => CreativeFxParams::CmykHalftone(self.cmyk_params),
            CreativeFxType::DitherPalette => CreativeFxParams::DitherPalette(self.dither_params),
            CreativeFxType::Ntsc => CreativeFxParams::Ntsc(self.ntsc_params),
            CreativeFxType::Structure => CreativeFxParams::Structure(self.structure_params),
        }
    }

    fn render_stamp(&self, pixel_revision: u64) -> CreativeFxRenderStamp {
        CreativeFxRenderStamp {
            pixel_revision,
            fx_type: self.selected_fx,
            params: self.current_params(),
            mask_cache_revision: self.mask_cache_revision,
            params_revision: self.params_revision,
        }
    }

    fn mask_stamp(&self, input: ImageInputStamp) -> CreativeFxMaskStamp {
        CreativeFxMaskStamp {
            input,
            fx_type: self.selected_fx,
            selection_revision: self.selection_revision,
            params_revision: self.params_revision,
        }
    }

    fn disable_mask_source(&mut self, source: CreativeFxMaskSource) -> bool {
        let configs = [
            &mut self.basic_params.depth_mask,
            &mut self.lens_params.depth_mask,
            &mut self.trichromatic_params.depth_mask,
            &mut self.pixel_sort_params.depth_mask,
            &mut self.cmyk_params.depth_mask,
            &mut self.dither_params.depth_mask,
            &mut self.ntsc_params.depth_mask,
            &mut self.structure_params.depth_mask,
        ];
        configs.into_iter().fold(false, |changed, config| {
            if config.enabled && config.source == source {
                config.enabled = false;
                true
            } else {
                changed
            }
        })
    }
}
