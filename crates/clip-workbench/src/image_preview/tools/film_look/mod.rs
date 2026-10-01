mod render_jobs;
mod stocks;
mod studio;
mod thumbnails;

use super::super::*;
use crate::shell::render_context_slider;
pub use stocks::*;

const DEFAULT_WB_STRENGTH: f32 = 0.75;

fn film_rgb_from_rgba(
    rgba: &RgbaImage,
    mut is_cancelled: impl FnMut() -> bool,
) -> Option<image::RgbImage> {
    if rgba.is_empty() {
        return None;
    }
    let mut pixels = Vec::with_capacity(rgba.as_raw().len() / 4 * 3);
    for row in rgba.as_raw().chunks_exact(rgba.width() as usize * 4) {
        if is_cancelled() {
            return None;
        }
        for pixel in row.chunks_exact(4) {
            pixels.extend_from_slice(&pixel[..3]);
        }
    }
    if is_cancelled() {
        return None;
    }
    image::RgbImage::from_raw(rgba.width(), rgba.height(), pixels)
}

/// Correct an Auto-WB cast only from pixels that are plausible neutral highlights.
/// Whole-image gray-world averages mistake warm or colorful scenes for lighting casts.
#[cfg(test)]
fn apply_auto_white_balance(image: &mut image::RgbImage, strength: f32) -> bool {
    apply_auto_white_balance_inner(image, strength, None)
}

fn apply_auto_white_balance_inner(
    image: &mut image::RgbImage,
    strength: f32,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> bool {
    let strength = strength.clamp(0.0, 1.0);
    if strength == 0.0 || image.is_empty() {
        return false;
    }

    let mut sums = [0.0_f32; 3];
    let mut count = 0.0_f32;
    let mut luma_sum = 0.0_f32;
    let mut luma_squared_sum = 0.0_f32;
    let sample_stride = ((image.width() as usize * image.height() as usize) / 100_000).max(1);
    for (index, pixel) in image.pixels().enumerate() {
        if index.is_multiple_of(4096)
            && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            return false;
        }
        if index % sample_stride != 0 {
            continue;
        }
        let channels = pixel.0.map(f32::from);
        let max_channel = channels.iter().copied().fold(0.0_f32, f32::max);
        let min_channel = channels.iter().copied().fold(255.0_f32, f32::min);
        let relative_chroma = (max_channel - min_channel) / max_channel.max(1.0);
        // Permit a wider blue-dominant cast while keeping the more conservative warm-scene
        // cutoff; saturated blue objects still exceed this bound and are not references.
        let chroma_limit = if channels[2] >= channels[1] && channels[2] > channels[0] {
            0.40
        } else {
            0.30
        };
        if max_channel < 128.0 || relative_chroma > chroma_limit {
            continue;
        }

        let luma = channels.iter().sum::<f32>() / 3.0;
        luma_sum += luma;
        luma_squared_sum += luma * luma;
        for (sum, channel) in sums.iter_mut().zip(channels) {
            *sum += channel;
        }
        count += 1.0;
    }

    let required_samples = 32.0;
    if count < required_samples {
        return false;
    }
    let mean_luma = luma_sum / count;
    let luma_variance = (luma_squared_sum / count - mean_luma * mean_luma).max(0.0);
    if luma_variance.sqrt() < 8.0 {
        return false;
    }

    let means = sums.map(|sum| sum / count);
    let target = means.iter().sum::<f32>() / 3.0;
    let gains = means.map(|mean| (target / mean.max(1.0)).clamp(0.7, 1.4));
    let min_gain = gains.iter().copied().fold(f32::INFINITY, f32::min);
    let max_gain = gains.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    // Reject extreme estimates, but permit bounded gains needed by reliable references
    // whose blue cast exceeds the per-pixel chroma cutoff above.
    if max_gain / min_gain > 1.60 {
        return false;
    }

    apply_white_balance_gains_inner(image, gains, strength, cancel)
}

#[cfg(test)]
fn gray_world_gains(image: &image::RgbImage) -> Option<[f32; 3]> {
    gray_world_gains_inner(image, None)
}

fn gray_world_gains_inner(
    image: &image::RgbImage,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<[f32; 3]> {
    let mut sums = [0.0_f32; 3];
    let count = image.width() as f32 * image.height() as f32;
    if count == 0.0 {
        return None;
    }
    for (index, pixel) in image.pixels().enumerate() {
        if index.is_multiple_of(4096)
            && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            return None;
        }
        for (sum, channel) in sums.iter_mut().zip(pixel.0) {
            *sum += f32::from(channel);
        }
    }
    let means = sums.map(|sum| sum / count);
    let target = means.iter().sum::<f32>() / 3.0;
    Some(means.map(|mean| (target / mean.max(1.0)).clamp(0.7, 1.4)))
}

#[cfg(test)]
fn white_patch_gains(image: &image::RgbImage) -> Option<[f32; 3]> {
    white_patch_gains_inner(image, None)
}

fn white_patch_gains_inner(
    image: &image::RgbImage,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<[f32; 3]> {
    let pixel_count = image.width() as usize * image.height() as usize;
    if pixel_count == 0 {
        return None;
    }
    let mut histograms = [[0_usize; 256]; 3];
    for (index, pixel) in image.pixels().enumerate() {
        if index.is_multiple_of(4096)
            && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            return None;
        }
        for (histogram, channel) in histograms.iter_mut().zip(pixel.0) {
            histogram[usize::from(channel)] += 1;
        }
    }

    let percentile_index = pixel_count * 99 / 100;
    let white_points = histograms.map(|histogram| {
        let mut remaining = percentile_index;
        for level in 0..=255 {
            let count = histogram[level];
            if remaining < count {
                return level as f32;
            }
            remaining -= count;
        }
        255.0
    });
    let target = white_points.iter().sum::<f32>() / 3.0;
    Some(white_points.map(|white_point| (target / white_point.max(1.0)).clamp(0.7, 1.4)))
}

#[cfg(test)]
fn apply_white_balance_gains(image: &mut image::RgbImage, gains: [f32; 3], strength: f32) -> bool {
    apply_white_balance_gains_inner(image, gains, strength, None)
}

fn apply_white_balance_gains_inner(
    image: &mut image::RgbImage,
    gains: [f32; 3],
    strength: f32,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> bool {
    let strength = strength.clamp(0.0, 1.0);
    if strength == 0.0 {
        return false;
    }
    for (index, pixel) in image.pixels_mut().enumerate() {
        if index.is_multiple_of(4096)
            && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            return false;
        }
        for (channel, gain) in pixel.0.iter_mut().zip(gains) {
            let adjusted_gain = 1.0 + (gain - 1.0) * strength;
            *channel = (f32::from(*channel) * adjusted_gain)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    true
}

#[cfg(test)]
mod white_balance_tests;

#[cfg(test)]
mod render_identity_tests;

fn process_film_image(
    image: &image::RgbImage,
    stock: &filmr::film::FilmStock,
    config: &SimulationConfig,
) -> image::RgbImage {
    process_film_image_inner(image, stock, config, None)
        .expect("uncancelled film processing cannot stop")
}

fn process_film_image_inner(
    image: &image::RgbImage,
    stock: &filmr::film::FilmStock,
    config: &SimulationConfig,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<image::RgbImage> {
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return None;
    }
    let mut source = image.clone();
    let mut config = config.clone();
    let gains = match config.white_balance_mode {
        WhiteBalanceMode::Auto => {
            apply_auto_white_balance_inner(&mut source, config.white_balance_strength, cancel);
            None
        }
        WhiteBalanceMode::Gray => gray_world_gains_inner(&source, cancel),
        WhiteBalanceMode::White => white_patch_gains_inner(&source, cancel),
        WhiteBalanceMode::Off => None,
    };
    if let Some(gains) = gains {
        apply_white_balance_gains_inner(&mut source, gains, config.white_balance_strength, cancel);
    }
    if config.white_balance_mode != WhiteBalanceMode::Off {
        // Apply these modes before film simulation. The pinned filmr release currently
        // treats Gray and White like Off and uses scene-wide gray-world for Auto.
        config.white_balance_mode = WhiteBalanceMode::Off;
        config.white_balance_strength = 0.0;
    }
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return None;
    }
    match cancel {
        Some(flag) => filmr::processor::process_image_cancellable(&source, stock, &config, flag),
        None => Some(filmr::processor::process_image(&source, stock, &config)),
    }
}

/// Helper: renders a slider with full visual context (start, center, end, and default tick marks,
/// gauge fill line from default to current, and double-click to reset to default).

/// Applies post-processing photographic S-curve contrast and green/magenta tint.
pub fn apply_film_sim_post_adjustments(
    out_rgb: &image::RgbImage,
    contrast: f32,
    tint: f32,
) -> image::RgbaImage {
    apply_film_sim_post_adjustments_inner(out_rgb, contrast, tint, None)
        .expect("uncancelled film adjustment cannot stop")
}

fn apply_film_sim_post_adjustments_inner(
    out_rgb: &image::RgbImage,
    contrast: f32,
    tint: f32,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<image::RgbaImage> {
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return None;
    }
    let has_contrast = contrast.abs() > 0.001;
    let has_tint = tint.abs() > 0.001;

    if !has_contrast && !has_tint {
        let mut out = image::RgbaImage::new(out_rgb.width(), out_rgb.height());
        for y in 0..out_rgb.height() {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
                return None;
            }
            for x in 0..out_rgb.width() {
                let p = out_rgb.get_pixel(x, y);
                out.put_pixel(x, y, image::Rgba([p[0], p[1], p[2], 255]));
            }
        }
        return Some(out);
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

    let mut out = image::RgbaImage::new(out_rgb.width(), out_rgb.height());
    for y in 0..out_rgb.height() {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return None;
        }
        for x in 0..out_rgb.width() {
            let p = out_rgb.get_pixel(x, y);
            let r_c = lut[p[0] as usize] as f32;
            let g_c = lut[p[1] as usize] as f32;
            let b_c = lut[p[2] as usize] as f32;

            let r_final = (r_c * r_mult).clamp(0.0, 255.0).round() as u8;
            let g_final = (g_c * g_mult).clamp(0.0, 255.0).round() as u8;
            let b_final = (b_c * b_mult).clamp(0.0, 255.0).round() as u8;
            out.put_pixel(x, y, image::Rgba([r_final, g_final, b_final, 255]));
        }
    }
    Some(out)
}

/// Messages sent from the film sim background render thread.
#[derive(Debug)]
pub enum FilmSimMsg {
    Done {
        stamp: FilmSimRenderStamp,
        rgba: RgbaImage,
        duration_ms: u128,
    },
    Error {
        stamp: FilmSimRenderStamp,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FilmSimRenderStamp {
    image: ImageInputStamp,
    parameter_revision: u64,
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
    /// White balance correction strength: 0.0 .. 1.0, default 0.75.
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
    pub render_rx:
        Option<std::sync::mpsc::Receiver<Result<Option<FilmSimMsg>, little_clip_core::JobError>>>,
    pub render_error: Option<String>,
    pub(crate) render_stamp: Option<FilmSimRenderStamp>,
    pub(crate) preview_stamp: Option<FilmSimRenderStamp>,
    render_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    render_job_ticket: Option<little_clip_core::JobTicket>,
    pub(crate) render_retry_at: Option<std::time::Instant>,
    parameter_revision: u64,

    // ── Async thumbnail worker (Sidebar 42x28 mini-photos) ──
    pub stock_thumbnails: std::collections::HashMap<usize, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(usize, image::RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<FilmSimThumbnailMsg>>,
    pub(crate) thumbnail_job_rx:
        Option<std::sync::mpsc::Receiver<Result<(), little_clip_core::JobError>>>,
    pub thumbnail_epoch: u64,
    pub(crate) thumbnail_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) thumbnail_job_ticket: Option<little_clip_core::JobTicket>,
    pub thumbnail_dirty_at: Option<std::time::Instant>,
    pub(crate) thumbnail_retry_at: Option<std::time::Instant>,
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
            wb_strength: DEFAULT_WB_STRENGTH,
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
            render_stamp: None,
            preview_stamp: None,
            render_cancel: None,
            render_job_ticket: None,
            render_retry_at: None,
            parameter_revision: 0,
            stock_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_job_rx: None,
            thumbnail_epoch: 0,
            thumbnail_cancel: None,
            thumbnail_job_ticket: None,
            thumbnail_dirty_at: None,
            thumbnail_retry_at: None,
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
        self.cancel_render();
        self.cancel_thumbnail_batch();
        self.stock_thumbnails.clear();
        self.pending_thumbnails.clear();
        self.parameter_revision = self.parameter_revision.wrapping_add(1);
        let now = std::time::Instant::now();
        self.params_dirty_at = Some(now);
        self.thumbnail_dirty_at = Some(now + std::time::Duration::from_millis(250));
    }

    pub(crate) fn cancel_render(&mut self) {
        if let Some(ticket) = self.render_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.render_rx = None;
        self.render_stamp = None;
        self.is_rendering = false;
        self.render_retry_at = None;
    }

    pub(crate) fn cancel_thumbnail_batch(&mut self) {
        if let Some(cancel) = self.thumbnail_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(ticket) = self.thumbnail_job_ticket.take() {
            ticket.cancel();
        }
        self.thumbnail_epoch = self.thumbnail_epoch.wrapping_add(1);
        self.thumbnail_rx = None;
        self.thumbnail_job_rx = None;
        self.is_generating_thumbnails = false;
        self.thumbnail_retry_at = None;
    }

    fn poll_thumbnail_job_queue(&mut self) {
        let Some(rx) = self.thumbnail_job_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Err(little_clip_core::JobError::QueueFull)) => {
                self.cancel_thumbnail_batch();
                self.thumbnail_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
            }
            Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.thumbnail_job_ticket = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => self.thumbnail_job_rx = Some(rx),
        }
    }

    fn thumbnail_retry_ready(&self, now: std::time::Instant) -> bool {
        self.thumbnail_retry_at
            .is_none_or(|retry_at| now >= retry_at)
    }
}

#[cfg(test)]
mod thumbnail_executor_tests;
