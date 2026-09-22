//! Effect rendering and spatial mask composition.

use super::*;
use image::RgbaImage;
use rayon::prelude::*;
#[derive(Clone, Copy)]
enum MaskValueChannel {
    Luminance,
    Alpha,
}

/// Blend an original image and creative effect result using a spatial mask.
fn apply_spatial_mask(
    orig: &RgbaImage,
    fx: &RgbaImage,
    mask_image: &RgbaImage,
    config: &DepthMaskConfig,
    channel: MaskValueChannel,
) -> RgbaImage {
    if !config.enabled {
        return fx.clone();
    }

    let (width, height) = orig.dimensions();
    if (width, height) != fx.dimensions() {
        return fx.clone();
    }

    let resampled_mask: std::borrow::Cow<'_, RgbaImage> =
        if mask_image.dimensions() != (width, height) {
            std::borrow::Cow::Owned(image::imageops::resize(
                mask_image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            ))
        } else {
            std::borrow::Cow::Borrowed(mask_image)
        };

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
            for x in 0..(width as usize) {
                let px_idx = x * 4;
                let mut d = match channel {
                    MaskValueChannel::Luminance => {
                        let r = mask_row[px_idx] as f32 / 255.0;
                        let g = mask_row[px_idx + 1] as f32 / 255.0;
                        let b = mask_row[px_idx + 2] as f32 / 255.0;
                        0.299 * r + 0.587 * g + 0.114 * b
                    }
                    MaskValueChannel::Alpha => mask_row[px_idx + 3] as f32 / 255.0,
                };

                if config.invert {
                    d = 1.0 - d;
                }

                let weight = match channel {
                    MaskValueChannel::Luminance if config.threshold > 0.001 => {
                        let feather = config.feather.clamp(0.001, 1.0);
                        let low = (config.threshold - feather).clamp(0.0, 1.0);
                        let high = (config.threshold + feather).clamp(0.0, 1.0);
                        if high <= low {
                            if d >= config.threshold { 1.0 } else { 0.0 }
                        } else {
                            let t = ((d - low) / (high - low)).clamp(0.0, 1.0);
                            t * t * (3.0 - 2.0 * t)
                        }
                    }
                    _ => d.clamp(0.0, 1.0),
                };

                for c in 0..3 {
                    let orig_c = orig_row[px_idx + c] as f32;
                    let fx_c = fx_row[px_idx + c] as f32;
                    let blended = (1.0 - weight) * orig_c + weight * fx_c;
                    out_row[px_idx + c] = blended.round().clamp(0.0, 255.0) as u8;
                }
                out_row[px_idx + 3] = orig_row[px_idx + 3];
            }
        });

    RgbaImage::from_raw(width, height, out_bytes).unwrap_or_else(|| fx.clone())
}

fn render_creative_fx(img: &RgbaImage, params: &CreativeFxParams) -> RgbaImage {
    match params {
        CreativeFxParams::LensAberration(p) => apply_lens_aberration(img, p),
        CreativeFxParams::Trichromatic(p) => apply_trichromatic(img, p),
        CreativeFxParams::PixelSort(p) => apply_pixel_sort(img, p),
        CreativeFxParams::CmykHalftone(p) => apply_cmyk_halftone(img, p),
        CreativeFxParams::DitherPalette(p) => apply_dither_palette(img, p),
        CreativeFxParams::Ntsc(p) => apply_ntsc_effect(img, p),
        CreativeFxParams::Structure(p) => apply_structure(img, p),
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
            let fx_result = render_creative_fx(img, params);
            return apply_spatial_mask(img, &fx_result, mask_image, mask_config, channel);
        }
    }

    render_creative_fx(img, params)
}

/// Apply a creative effect to an RGBA image with optional depth map masking.
pub fn apply_creative_fx_with_depth(
    img: &RgbaImage,
    depth: Option<&RgbaImage>,
    params: &CreativeFxParams,
) -> RgbaImage {
    let mask_config = params.depth_mask();
    if mask_config.enabled
        && let Some(depth_map) = depth
    {
        let fx_result = render_creative_fx(img, params);
        return apply_depth_mask(img, &fx_result, depth_map, mask_config);
    }

    render_creative_fx(img, params)
}

/// Apply any creative effect using its parameter union (without depth masking).
pub fn apply_creative_fx(img: &RgbaImage, params: &CreativeFxParams) -> RgbaImage {
    render_creative_fx(img, params)
}
