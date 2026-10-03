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
    apply_spatial_mask_cancellable(orig, fx, mask_image, config, channel, None)
        .expect("uncancelled mask blend cannot stop")
}

fn apply_spatial_mask_cancellable(
    orig: &RgbaImage,
    fx: &RgbaImage,
    mask_image: &RgbaImage,
    config: &DepthMaskConfig,
    channel: MaskValueChannel,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if super::is_cancelled(cancel) {
        return None;
    }
    if !config.enabled {
        return Some(fx.clone());
    }

    let (width, height) = orig.dimensions();
    if orig.is_empty() || (width, height) != fx.dimensions() {
        return Some(fx.clone());
    }

    let resampled_mask = resample_mask(mask_image, width, height);

    let mut out_bytes = vec![0u8; (width * height * 4) as usize];
    let orig_raw = orig.as_raw();
    let fx_raw = fx.as_raw();
    let mask_raw = resampled_mask.as_raw();

    out_bytes
        .par_chunks_exact_mut((width * 4) as usize)
        .zip(orig_raw.par_chunks_exact((width * 4) as usize))
        .zip(fx_raw.par_chunks_exact((width * 4) as usize))
        .zip(mask_raw.par_chunks_exact((width * 4) as usize))
        .for_each(|(((out_row, orig_row), fx_row), mask_row)| {
            if super::is_cancelled(cancel) {
                return;
            }
            for x in 0..(width as usize) {
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
    Some(RgbaImage::from_raw(width, height, out_bytes).unwrap_or_else(|| fx.clone()))
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
            let blended = apply_spatial_mask_cancellable(
                img,
                &fx_result,
                mask_image,
                mask_config,
                channel,
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
