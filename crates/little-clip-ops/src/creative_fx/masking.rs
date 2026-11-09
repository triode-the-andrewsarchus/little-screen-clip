//! Effect rendering and spatial mask composition.

use super::*;
use image::RgbaImage;
use rayon::prelude::*;
#[derive(Clone, Copy)]
enum MaskValueChannel {
    Luminance,
    Alpha,
}

fn resample_mask(mask: &RgbaImage, width: u32, height: u32) -> std::borrow::Cow<'_, RgbaImage> {
    if mask.dimensions() == (width, height) {
        std::borrow::Cow::Borrowed(mask)
    } else {
        std::borrow::Cow::Owned(image::imageops::resize(
            mask,
            width,
            height,
            image::imageops::FilterType::Triangle,
        ))
    }
}

fn mask_weight(pixel: &[u8], config: &DepthMaskConfig, channel: MaskValueChannel) -> f32 {
    let mut d = match channel {
        MaskValueChannel::Luminance => {
            0.299 * (pixel[0] as f32 / 255.0)
                + 0.587 * (pixel[1] as f32 / 255.0)
                + 0.114 * (pixel[2] as f32 / 255.0)
        }
        MaskValueChannel::Alpha => pixel[3] as f32 / 255.0,
    };
    if config.invert {
        d = 1.0 - d;
    }
    if matches!(channel, MaskValueChannel::Luminance) && config.threshold > 0.001 {
        let feather = config.feather.clamp(0.001, 1.0);
        let low = (config.threshold - feather).clamp(0.0, 1.0);
        let high = (config.threshold + feather).clamp(0.0, 1.0);
        if high <= low {
            return if d >= config.threshold { 1.0 } else { 0.0 };
        }
        let t = ((d - low) / (high - low)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    } else {
        d.clamp(0.0, 1.0)
    }
}

/// Blend an original image and creative effect result using a spatial mask.
fn apply_spatial_mask(
    orig: &RgbaImage,
    fx: &RgbaImage,
    mask_image: &RgbaImage,
    config: &DepthMaskConfig,
    channel: MaskValueChannel,
) -> RgbaImage {
    apply_spatial_mask_cancellable(orig, fx, mask_image, config, channel, None, None)
        .expect("uncancelled mask blend cannot stop")
}

fn apply_spatial_mask_cancellable(
    orig: &RgbaImage,
    fx: &RgbaImage,
    mask_image: &RgbaImage,
    config: &DepthMaskConfig,
    channel: MaskValueChannel,
    crop_window: Option<(u32, u32, u32, u32)>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if super::is_cancelled(cancel) {
        return None;
    }
    if !config.enabled {
        return Some(fx.clone());
    }

    let (orig_w, orig_h) = orig.dimensions();
    let (start_x, start_y, out_w, out_h) = crop_window.unwrap_or((0, 0, orig_w, orig_h));
    if orig.is_empty()
        || fx.is_empty()
        || fx.dimensions() != (out_w, out_h)
        || start_x.saturating_add(out_w) > orig_w
        || start_y.saturating_add(out_h) > orig_h
    {
        return Some(fx.clone());
    }

    let resampled_mask = resample_mask(mask_image, orig_w, orig_h);

    let mut out_bytes = vec![0u8; (out_w * out_h * 4) as usize];
    let orig_stride = (orig_w * 4) as usize;
    let out_stride = (out_w * 4) as usize;
    let start_x_bytes = (start_x * 4) as usize;
    let row_start = start_y as usize * orig_stride;
    let row_end = (start_y + out_h) as usize * orig_stride;
    let orig_rows = &orig.as_raw()[row_start..row_end];
    let fx_raw = fx.as_raw();
    let mask_rows = &resampled_mask.as_raw()[row_start..row_end];

    out_bytes
        .par_chunks_exact_mut(out_stride)
        .zip(orig_rows.par_chunks_exact(orig_stride))
        .zip(fx_raw.par_chunks_exact(out_stride))
        .zip(mask_rows.par_chunks_exact(orig_stride))
        .for_each(|(((out_row, orig_row), fx_row), mask_row)| {
            if super::is_cancelled(cancel) {
                return;
            }
            let orig_row = &orig_row[start_x_bytes..start_x_bytes + out_stride];
            let mask_row = &mask_row[start_x_bytes..start_x_bytes + out_stride];
            for x in 0..(out_w as usize) {
                let px_idx = x * 4;
                let weight = mask_weight(&mask_row[px_idx..px_idx + 4], config, channel);

                for c in 0..3 {
                    let orig_c = orig_row[px_idx + c] as f32;
                    let fx_c = fx_row[px_idx + c] as f32;
                    let blended = (1.0 - weight) * orig_c + weight * fx_c;
                    out_row[px_idx + c] = blended.round().clamp(0.0, 255.0) as u8;
                }
                out_row[px_idx + 3] = orig_row[px_idx + 3];
            }
        });

    if super::is_cancelled(cancel) {
        return None;
    }
    Some(RgbaImage::from_raw(out_w, out_h, out_bytes).unwrap_or_else(|| fx.clone()))
}

fn render_creative_fx(img: &RgbaImage, params: &CreativeFxParams) -> RgbaImage {
    render_creative_fx_cancellable(img, params, None)
        .expect("uncancelled creative effect cannot stop")
}

fn render_creative_fx_cancellable(
    img: &RgbaImage,
    params: &CreativeFxParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    match params {
        CreativeFxParams::BasicControls(p) => apply_basic_controls_cancellable(img, p, cancel),
        CreativeFxParams::LensAberration(p) => apply_lens_aberration_cancellable(img, p, cancel),
        CreativeFxParams::Trichromatic(p) => apply_trichromatic_cancellable(img, p, cancel),
        CreativeFxParams::PixelSort(p) => apply_pixel_sort_cancellable(img, p, cancel),
        CreativeFxParams::CmykHalftone(p) => apply_cmyk_halftone_cancellable(img, p, cancel),
        CreativeFxParams::DitherPalette(p) => apply_dither_palette_cancellable(img, p, cancel),
        CreativeFxParams::Ntsc(p) => apply_ntsc_effect_cancellable(img, p, cancel),
        CreativeFxParams::Structure(p) => apply_structure_cancellable(img, p, cancel),
    }
}

/// Blend original image and creative effect result using a grayscale depth map as a spatial mask.
pub fn apply_depth_mask(
    orig: &RgbaImage,
    fx: &RgbaImage,
    depth_map: &RgbaImage,
    config: &DepthMaskConfig,
) -> RgbaImage {
    apply_spatial_mask(orig, fx, depth_map, config, MaskValueChannel::Luminance)
}

/// Blend original image and creative effect result using a background-removal alpha channel.
pub fn apply_alpha_mask(
    orig: &RgbaImage,
    fx: &RgbaImage,
    removal_result: &RgbaImage,
    config: &DepthMaskConfig,
) -> RgbaImage {
    apply_spatial_mask(orig, fx, removal_result, config, MaskValueChannel::Alpha)
}

/// Apply a creative effect with the mask selected by the effect configuration.
pub fn apply_creative_fx_with_masks(
    img: &RgbaImage,
    depth: Option<&RgbaImage>,
    background_removal: Option<&RgbaImage>,
    params: &CreativeFxParams,
) -> RgbaImage {
    apply_creative_fx_with_masks_inner(img, depth, background_removal, params, None)
        .expect("uncancelled creative effect cannot stop")
}

/// Renders the same effect and mask blend, stopping at cooperative checkpoints.
pub fn apply_creative_fx_with_masks_cancellable(
    img: &RgbaImage,
    depth: Option<&RgbaImage>,
    background_removal: Option<&RgbaImage>,
    params: &CreativeFxParams,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<RgbaImage> {
    apply_creative_fx_with_masks_inner(img, depth, background_removal, params, Some(cancel))
}

fn apply_creative_fx_with_masks_inner(
    img: &RgbaImage,
    depth: Option<&RgbaImage>,
    background_removal: Option<&RgbaImage>,
    params: &CreativeFxParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if super::is_cancelled(cancel) {
        return None;
    }
    let mask_config = params.depth_mask();
    if mask_config.enabled {
        let mask = match mask_config.source {
            CreativeFxMaskSource::DepthMap => {
                depth.map(|image| (image, MaskValueChannel::Luminance))
            }
            CreativeFxMaskSource::BackgroundRemovalAlpha => {
                background_removal.map(|image| (image, MaskValueChannel::Alpha))
            }
        };
        if let Some((mask_image, channel)) = mask {
            let fx_result = render_creative_fx_cancellable(img, params, cancel)?;
            let crop_window = match params {
                CreativeFxParams::Trichromatic(p) if p.auto_crop && !img.is_empty() => Some(
                    trichromatic_intersection_box(img.width(), img.height(), &p.channels),
                ),
                _ => None,
            };
            let blended = apply_spatial_mask_cancellable(
                img,
                &fx_result,
                mask_image,
                mask_config,
                channel,
                crop_window,
                cancel,
            )?;
            if let CreativeFxParams::BasicControls(p) = params
                && p.masked_zoom > 1.0
                && !img.is_empty()
            {
                let mask = resample_mask(mask_image, img.width(), img.height());
                let weights: Vec<_> = mask
                    .as_raw()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| mask_weight(pixel, mask_config, channel))
                    .collect();
                return super::masked_zoom::enlarge_masked_region(
                    blended, &fx_result, &weights, p, cancel,
                );
            }
            return Some(blended);
        }
    }

    render_creative_fx_cancellable(img, params, cancel)
}

/// Apply a creative effect to an RGBA image with optional depth map masking.
pub fn apply_creative_fx_with_depth(
    img: &RgbaImage,
    depth: Option<&RgbaImage>,
    params: &CreativeFxParams,
) -> RgbaImage {
    apply_creative_fx_with_masks(img, depth, None, params)
}

/// Apply any creative effect using its parameter union (without depth masking).
pub fn apply_creative_fx(img: &RgbaImage, params: &CreativeFxParams) -> RgbaImage {
    render_creative_fx(img, params)
}
