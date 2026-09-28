//! Creative FX image manipulation algorithms.

mod cmyk_halftone;
mod color;
mod common;
mod dither_palette;
mod lens_aberration;
mod masking;
mod ntsc;
mod pixel_sort;
mod presets;
mod structure;
mod trichromatic;

pub use cmyk_halftone::*;
pub use color::*;
pub use common::*;
pub use dither_palette::*;
pub use lens_aberration::*;
pub use masking::*;
pub use ntsc::*;
pub use pixel_sort::*;
pub use presets::*;
pub use structure::*;
pub use trichromatic::*;

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
static CANCEL_ON_CHECK: std::sync::Mutex<Option<(usize, usize, usize)>> =
    std::sync::Mutex::new(None);

#[inline]
fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    #[cfg(test)]
    if let Some(flag) = cancel {
        let mut slot = CANCEL_ON_CHECK.lock().unwrap();
        if let Some((token, count, limit)) = slot.as_mut()
            && *token == flag as *const AtomicBool as usize
        {
            *count += 1;
            if *count >= *limit {
                flag.store(true, Ordering::Release);
            }
        }
    }
    cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
}

#[cfg(test)]
use image::{Rgba, RgbaImage};
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creative_fx_reference_pixels() {
        let image = RgbaImage::from_fn(48, 40, |x, y| {
            Rgba([
                ((x * 17 + y * 3) % 256) as u8,
                ((x * 5 + y * 19) % 256) as u8,
                ((x * 11 + y * 7) % 256) as u8,
                200,
            ])
        });
        let params = [
            CreativeFxParams::LensAberration(LensAberrationParams::default()),
            CreativeFxParams::Trichromatic(TrichromaticParams::default()),
            CreativeFxParams::PixelSort(PixelSortParams::default()),
            CreativeFxParams::CmykHalftone(CmykHalftoneParams::default()),
            CreativeFxParams::DitherPalette(DitherPaletteParams::default()),
            CreativeFxParams::Ntsc(NtscParams::default()),
            CreativeFxParams::Structure(StructureParams::default()),
        ];
        let expected = [
            (48, 40, 0x8b22bb5e2c1659d9u64),
            (41, 39, 0x1df78d52d9c74883),
            (48, 40, 0x022022c00c34b8bd),
            (48, 40, 0x1d7116d15f7b7e38),
            (48, 40, 0x3c200feb409cc845),
            (48, 40, 0x9b41fac5214fb326),
            (48, 40, 0x66888752fe25d1a0),
        ];
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        for (params, (width, height, expected_hash)) in params.into_iter().zip(expected) {
            let output = apply_creative_fx(&image, &params);
            let hash = output
                .as_raw()
                .iter()
                .fold(0xcbf29ce484222325u64, |h, byte| {
                    (h ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                });
            assert_eq!(
                (output.width(), output.height(), hash),
                (width, height, expected_hash),
                "{params:?}"
            );
            let cancellable =
                apply_creative_fx_with_masks_cancellable(&image, None, None, &params, &cancelled)
                    .unwrap();
            assert_eq!(cancellable, output, "{params:?}");
            cancelled.store(true, std::sync::atomic::Ordering::Release);
            assert!(
                apply_creative_fx_with_masks_cancellable(&image, None, None, &params, &cancelled)
                    .is_none(),
                "{params:?}"
            );
            cancelled.store(false, std::sync::atomic::Ordering::Release);
        }
    }

    #[test]
    fn creative_fx_stops_at_effect_and_mask_row_checkpoints() {
        let image = RgbaImage::from_pixel(48, 40, Rgba([60, 90, 120, 255]));
        let mask = RgbaImage::from_pixel(48, 40, Rgba([128, 128, 128, 255]));
        let mut lens = LensAberrationParams::default();
        lens.depth_mask.enabled = true;
        let params = CreativeFxParams::LensAberration(lens);

        for limit in [3, 45] {
            let cancel = AtomicBool::new(false);
            *CANCEL_ON_CHECK.lock().unwrap() =
                Some((&cancel as *const AtomicBool as usize, 0, limit));
            let result = apply_creative_fx_with_masks_cancellable(
                &image,
                Some(&mask),
                None,
                &params,
                &cancel,
            );
            let (_, checks, _) = CANCEL_ON_CHECK.lock().unwrap().take().unwrap();
            assert!(cancel.load(Ordering::Acquire));
            assert!(checks >= limit);
            assert!(result.is_none());
        }
    }

    #[test]
    fn test_oklab_roundtrip() {
        let colors = [
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
            (0.5, 0.5, 0.5),
            (0.9, 0.8, 0.2),
        ];
        for (r, g, b) in colors {
            let (l, a, b_ok) = srgb_to_oklab(r, g, b);
            let (r2, g2, b2) = oklab_to_srgb(l, a, b_ok);
            assert!((r - r2).abs() < 0.01, "R mismatch: {r} vs {r2}");
            assert!((g - g2).abs() < 0.01, "G mismatch: {g} vs {g2}");
            assert!((b - b2).abs() < 0.01, "B mismatch: {b} vs {b2}");
        }
    }

    #[test]
    fn test_lens_aberration_execution() {
        let mut img = RgbaImage::new(40, 40);
        for x in 0..40 {
            for y in 0..40 {
                let v = if (x / 5 + y / 5) % 2 == 0 { 255 } else { 0 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }

        // Test with default params
        let res = apply_lens_aberration(&img, &LensAberrationParams::default());
        assert_eq!(res.width(), 40);
        assert_eq!(res.height(), 40);

        // Test with color balance +50 (blue/yellow) and -50 (red/blue)
        let res_by = apply_lens_aberration(
            &img,
            &LensAberrationParams {
                strength: 50.0,
                falloff: 0.0,
                color_balance: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_by.width(), 40);

        let res_rb = apply_lens_aberration(
            &img,
            &LensAberrationParams {
                strength: 50.0,
                falloff: 100.0,
                color_balance: -50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_rb.width(), 40);
    }

    #[test]
    fn test_trichromatic_rgb_and_oklab() {
        let mut img = RgbaImage::new(60, 60);
        for x in 0..60 {
            for y in 0..60 {
                img.put_pixel(x, y, Rgba([x as u8 * 4, y as u8 * 4, 128, 255]));
            }
        }

        let params_rgb = TrichromaticParams {
            mode: TrichromaticMode::Rgb,
            channels: [
                ChannelTransform {
                    offset_x: -2.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 2.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
            ],
            auto_crop: true,
            depth_mask: DepthMaskConfig::default(),
        };

        let res_rgb = apply_trichromatic(&img, &params_rgb);
        // Valid crop should be cropped slightly
        assert!(res_rgb.width() <= 60);
        assert!(res_rgb.height() <= 60);
        assert!(res_rgb.width() > 50);

        let params_oklab = TrichromaticParams {
            mode: TrichromaticMode::OkLab,
            channels: [
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: -2.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 2.0,
                    zoom: 1.0,
                },
            ],
            auto_crop: true,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_oklab = apply_trichromatic(&img, &params_oklab);
        assert!(res_oklab.width() > 0 && res_oklab.height() > 0);
    }

    #[test]
    fn test_pixel_sort_directions() {
        let mut img = RgbaImage::new(30, 30);
        for x in 0..30 {
            for y in 0..30 {
                img.put_pixel(x, y, Rgba([((x * 13 + y * 7) % 256) as u8, 100, 200, 255]));
            }
        }

        // Test vertical sort
        let res_v = apply_pixel_sort(
            &img,
            &PixelSortParams {
                direction: PixelSortDirection::Vertical,
                criteria: PixelSortCriteria::Brightness,
                order: PixelSortOrder::Ascending,
                min_threshold: 0.2,
                max_threshold: 0.8,
                min_span: 4,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_v.width(), 30);
        assert_eq!(res_v.height(), 30);

        // Test horizontal sort
        let res_h = apply_pixel_sort(
            &img,
            &PixelSortParams {
                direction: PixelSortDirection::Horizontal,
                criteria: PixelSortCriteria::Hue,
                order: PixelSortOrder::Descending,
                min_threshold: 0.1,
                max_threshold: 0.9,
                min_span: 4,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_h.width(), 30);

        // Test diagonal sort
        let res_d = apply_pixel_sort(
            &img,
            &PixelSortParams {
                direction: PixelSortDirection::Diagonal,
                criteria: PixelSortCriteria::Saturation,
                order: PixelSortOrder::Ascending,
                min_threshold: 0.0,
                max_threshold: 1.0,
                min_span: 3,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_d.width(), 30);
    }

    #[test]
    fn test_cmyk_halftone_execution() {
        let mut img = RgbaImage::new(40, 40);
        for x in 0..40 {
            for y in 0..40 {
                let r = (x * 6) as u8;
                let g = (y * 6) as u8;
                let b = ((x + y) * 3) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }

        let params = CmykHalftoneParams {
            dot_size: 6.0,
            dot_gain: 1.2,
            paper_tint: PaperTint::Newsprint,
            cyan_offset: [-2.0, 1.0],
            magenta_offset: [1.5, -1.5],
            yellow_offset: [0.0, 2.0],
            black_offset: [0.0, 0.0],
            depth_mask: DepthMaskConfig::default(),
        };

        let res = apply_cmyk_halftone(&img, &params);
        assert_eq!(res.width(), 40);
        assert_eq!(res.height(), 40);

        // Verify paper tint substrate is applied in areas
        let p = res.get_pixel(0, 0);
        assert_eq!(p[3], 255);
    }

    #[test]
    fn test_dither_palette_execution() {
        let mut img = RgbaImage::new(32, 32);
        for x in 0..32 {
            for y in 0..32 {
                let v = (x * 8) as u8;
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }

        // Test Game Boy Bayer 4x4
        let res_gb = apply_dither_palette(
            &img,
            &DitherPaletteParams {
                palette: RetroPalette::GameBoy,
                method: DitherMethod::Bayer4x4,
                pixel_scale: 2,
                dither_strength: 0.8,
                contrast: 0.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_gb.width(), 32);
        assert_eq!(res_gb.height(), 32);

        // Test Mac 1-bit Floyd Steinberg
        let res_mac = apply_dither_palette(
            &img,
            &DitherPaletteParams {
                palette: RetroPalette::Macintosh1Bit,
                method: DitherMethod::FloydSteinberg,
                pixel_scale: 1,
                dither_strength: 0.9,
                contrast: 10.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_mac.width(), 32);
        assert_eq!(res_mac.height(), 32);

        // Test PICO-8 Blue Noise
        let res_pico = apply_dither_palette(
            &img,
            &DitherPaletteParams {
                palette: RetroPalette::Pico8,
                method: DitherMethod::BlueNoise,
                pixel_scale: 2,
                dither_strength: 0.7,
                contrast: -5.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_pico.width(), 32);
    }

    #[test]
    fn test_ntsc_effect_execution() {
        let mut img = RgbaImage::new(48, 48);
        for x in 0..48 {
            for y in 0..48 {
                let r = (x * 5) as u8;
                let g = (y * 5) as u8;
                let b = ((x + y) * 2) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }

        // Test default NTSC VHS parameters
        let params = NtscParams::default();
        let res = apply_ntsc_effect(&img, &params);
        assert_eq!(res.width(), 48);
        assert_eq!(res.height(), 48);

        // Alpha channel should remain intact
        for y in 0..48 {
            for x in 0..48 {
                assert_eq!(res.get_pixel(x, y)[3], 255);
            }
        }

        // Test with custom Bad Tracking preset params
        let tracking_params = NtscParams {
            luma_smear: 0.4,
            composite_sharpening: 1.5,
            ringing: 1.0,
            snow: 0.005,
            snow_anisotropy: 0.8,
            scanlines: false,
            phase_shift: NtscPhaseShift::Degrees270,
            demodulation: NtscDemodulation::Notch,
            chroma_lowpass: NtscChromaLowpass::Full,
            chroma_delay_x: 3.0,
            chroma_delay_y: 2,
            chroma_vert_blend: false,
            chroma_phase_error: 0.0,
            chroma_phase_noise: 0.05,
            chroma_noise: 0.20,
            luma_noise: 0.05,
            composite_noise: 0.10,
            vhs_enabled: true,
            tape_speed: VhsTapeSpeed::EP,
            edge_wave: 1.2,
            head_switching: true,
            tracking_noise: 0.5,
            vhs_sharpen: 0.5,
            vhs_chroma_loss: 0.2,
            random_seed: 42,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_tracking = apply_ntsc_effect(&img, &tracking_params);
        assert_eq!(res_tracking.width(), 48);
        assert_eq!(res_tracking.height(), 48);
    }

    #[test]
    fn test_structure_identity_at_zero() {
        let mut img = RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255]);
        }
        let params = StructureParams {
            amount: 0.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res = apply_structure(&img, &params);
        assert_eq!(res.as_raw(), img.as_raw());
    }

    #[test]
    fn test_structure_enhances_texture_variance() {
        // Create an image with mid-frequency texture pattern (checkerboard ripples)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                // Base midtone 128 + texture variation +- 15
                let tex = if ((x / 4) + (y / 4)) % 2 == 0 {
                    15i16
                } else {
                    -15i16
                };
                let val = (128 + tex) as u8;
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        // Calculate variance of original texture patch
        let calc_variance = |image: &RgbaImage| -> f32 {
            let mut sum = 0.0f32;
            let mut sum_sq = 0.0f32;
            let count = (image.width() * image.height()) as f32;
            for p in image.pixels() {
                let v = p[0] as f32;
                sum += v;
                sum_sq += v * v;
            }
            let mean = sum / count;
            sum_sq / count - mean * mean
        };

        let var_orig = calc_variance(&img);

        let params_boost = StructureParams {
            amount: 60.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_boost = apply_structure(&img, &params_boost);
        let var_boost = calc_variance(&res_boost);

        // Positive structure must amplify texture variance
        assert!(
            var_boost > var_orig * 1.15,
            "Expected texture variance to increase: orig={var_orig}, boost={var_boost}"
        );
    }

    #[test]
    fn test_structure_edge_protection_vs_unsharp_mask() {
        // Create an image with a high-contrast step edge:
        // Left half = 30 (dark), Right half = 225 (bright)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let val = if x < 32 { 30u8 } else { 225u8 };
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        // Naive unsharp mask calculation at edge:
        // blur with sigma ~2
        let mut luma = vec![0.0f32; 64 * 64];
        for (i, p) in img.pixels().enumerate() {
            luma[i] = p[0] as f32 / 255.0;
        }
        let mut blurred = vec![0.0f32; 64 * 64];
        fast_gaussian_blur(&luma, &mut blurred, 64, 64, 2.0);

        // Naive unsharp mask: out = orig + 1.2 * (orig - blur)
        // At pixel x=32 (just past edge), blur is ~ (30 + 225)/2 = 127.5
        // orig - blur = 225 - 127.5 = +97.5 => severe overshoot (halo)!
        let edge_idx_right = 32 * 64 + 32;
        let naive_unsharp_overshoot = (luma[edge_idx_right] - blurred[edge_idx_right]) * 1.2;

        // Now run Structure with edge protection
        let params = StructureParams {
            amount: 60.0,
            detail_scale: 0.0,
            edge_protection: 70.0, // strong edge protection,
            depth_mask: DepthMaskConfig::default(),
        };
        let res = apply_structure(&img, &params);
        let struct_luma_edge = res.get_pixel(32, 32)[0] as f32 / 255.0;
        let struct_overshoot = (struct_luma_edge - luma[edge_idx_right]).abs();

        // Structure overshoot at step edge must be vastly lower than naive unsharp mask
        assert!(
            struct_overshoot < naive_unsharp_overshoot * 0.25,
            "Structure must protect step edges from halos: naive_overshoot={naive_unsharp_overshoot}, struct_overshoot={struct_overshoot}"
        );
    }

    #[test]
    fn test_structure_negative_texture_suppression() {
        // Create an image with both a strong boundary and fine texture on one side:
        // x < 32: dark background (val = 40)
        // x >= 32: textured object (base = 180 + alternating 10)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                if x < 32 {
                    img.put_pixel(x, y, Rgba([40, 40, 40, 255]));
                } else {
                    let tex = if ((x / 2) + (y / 2)) % 2 == 0 {
                        12i16
                    } else {
                        -12i16
                    };
                    let val = (180 + tex) as u8;
                    img.put_pixel(x, y, Rgba([val, val, val, 255]));
                }
            }
        }

        // Measure texture variance in the textured region (x >= 40, away from the boundary)
        let calc_region_variance = |image: &RgbaImage| -> f32 {
            let mut sum = 0.0f32;
            let mut sum_sq = 0.0f32;
            let mut count = 0.0f32;
            for y in 0..64 {
                for x in 40..64 {
                    let v = image.get_pixel(x, y)[0] as f32;
                    sum += v;
                    sum_sq += v * v;
                    count += 1.0;
                }
            }
            let mean = sum / count;
            sum_sq / count - mean * mean
        };

        let var_orig = calc_region_variance(&img);

        let params_smooth = StructureParams {
            amount: -60.0, // negative structure
            detail_scale: 0.0,
            edge_protection: 65.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_smooth = apply_structure(&img, &params_smooth);
        let var_smooth = calc_region_variance(&res_smooth);

        // Texture variance must be suppressed
        assert!(
            var_smooth < var_orig * 0.70,
            "Expected negative structure to suppress texture variance: orig={var_orig}, smooth={var_smooth}"
        );

        // Meanwhile, the step boundary at x=31 / x=32 must remain crisp
        let dark_edge = res_smooth.get_pixel(30, 32)[0];
        let bright_edge = res_smooth.get_pixel(33, 32)[0];
        assert!(
            bright_edge as i16 - dark_edge as i16 > 110,
            "Boundary contrast should be preserved under negative structure: dark={dark_edge}, bright={bright_edge}"
        );
    }

    #[test]
    fn test_structure_flat_area_coring() {
        // Create an almost flat image with subtle sub-threshold sensor noise (+-1 sRGB step)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let noise = if (x * 17 + y * 31) % 5 == 0 {
                    1i16
                } else {
                    0i16
                };
                let val = (180 + noise) as u8;
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        let params = StructureParams {
            amount: 50.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res = apply_structure(&img, &params);

        // Maximum pixel deviation in flat area should remain tiny (<= 1) due to coring
        for y in 0..64 {
            for x in 0..64 {
                let orig = img.get_pixel(x, y)[0] as i16;
                let processed = res.get_pixel(x, y)[0] as i16;
                assert!(
                    (processed - orig).abs() <= 1,
                    "Flat area noise should not be amplified: orig={orig}, processed={processed}"
                );
            }
        }
    }

    #[test]
    fn test_structure_detail_scale_frequency_response() {
        // Create fine pattern (period 2px) and coarse pattern (period 16px)
        let mut img_fine = RgbaImage::new(64, 64);
        let mut img_coarse = RgbaImage::new(64, 64);

        for y in 0..64 {
            for x in 0..64 {
                let f_val = if (x % 2) == 0 { 140u8 } else { 116u8 };
                let c_val = if ((x / 8) % 2) == 0 { 140u8 } else { 116u8 };
                img_fine.put_pixel(x, y, Rgba([f_val, f_val, f_val, 255]));
                img_coarse.put_pixel(x, y, Rgba([c_val, c_val, c_val, 255]));
            }
        }

        let calc_var = |image: &RgbaImage| -> f32 {
            let mut sum = 0.0f32;
            let mut sum_sq = 0.0f32;
            for p in image.pixels() {
                let v = p[0] as f32;
                sum += v;
                sum_sq += v * v;
            }
            let count = (image.width() * image.height()) as f32;
            let mean = sum / count;
            sum_sq / count - mean * mean
        };

        let var_fine_orig = calc_var(&img_fine);
        let var_coarse_orig = calc_var(&img_coarse);

        // Fine bias (-50 detail_scale)
        let res_fine_biased = apply_structure(
            &img_fine,
            &StructureParams {
                amount: 50.0,
                detail_scale: -50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_fine_under_fine_bias = calc_var(&res_fine_biased) / var_fine_orig;

        // Coarse bias (+50 detail_scale)
        let res_fine_under_coarse_bias = apply_structure(
            &img_fine,
            &StructureParams {
                amount: 50.0,
                detail_scale: 50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_fine_under_coarse_bias = calc_var(&res_fine_under_coarse_bias) / var_fine_orig;

        // Fine pattern must receive higher gain under fine bias than under coarse bias
        assert!(
            gain_fine_under_fine_bias > gain_fine_under_coarse_bias,
            "Fine bias should boost fine texture more than coarse bias: fine_bias={gain_fine_under_fine_bias}, coarse_bias={gain_fine_under_coarse_bias}"
        );

        // Similarly for coarse pattern under coarse bias vs fine bias
        let res_coarse_under_coarse_bias = apply_structure(
            &img_coarse,
            &StructureParams {
                amount: 50.0,
                detail_scale: 50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_coarse_under_coarse_bias =
            calc_var(&res_coarse_under_coarse_bias) / var_coarse_orig;

        let res_coarse_under_fine_bias = apply_structure(
            &img_coarse,
            &StructureParams {
                amount: 50.0,
                detail_scale: -50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_coarse_under_fine_bias = calc_var(&res_coarse_under_fine_bias) / var_coarse_orig;

        assert!(
            gain_coarse_under_coarse_bias > gain_coarse_under_fine_bias,
            "Coarse bias should boost coarse structure more than fine bias: coarse_bias={gain_coarse_under_coarse_bias}, fine_bias={gain_coarse_under_fine_bias}"
        );
    }

    #[test]
    fn test_apply_depth_mask_continuous_blending() {
        let orig = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 255]));
        let fx = RgbaImage::from_pixel(2, 2, Rgba([200, 200, 200, 255]));
        let mut depth = RgbaImage::new(2, 2);
        depth.put_pixel(0, 0, Rgba([0, 0, 0, 255])); // 0% fx -> orig 0
        depth.put_pixel(1, 0, Rgba([128, 128, 128, 255])); // ~50% fx -> ~100
        depth.put_pixel(0, 1, Rgba([255, 255, 255, 255])); // 100% fx -> 200
        depth.put_pixel(1, 1, Rgba([64, 64, 64, 255])); // ~25% fx -> ~50

        let config = DepthMaskConfig {
            enabled: true,
            source: CreativeFxMaskSource::DepthMap,
            invert: false,
            threshold: 0.0,
            feather: 0.1,
        };

        let masked = apply_depth_mask(&orig, &fx, &depth, &config);
        assert_eq!(masked.get_pixel(0, 0)[0], 0);
        let p10 = masked.get_pixel(1, 0)[0];
        assert!((p10 as i32 - 100).abs() <= 2, "Expected ~100, got {}", p10);
        assert_eq!(masked.get_pixel(0, 1)[0], 200);

        // Test invert
        let inv_config = DepthMaskConfig {
            enabled: true,
            source: CreativeFxMaskSource::DepthMap,
            invert: true,
            threshold: 0.0,
            feather: 0.1,
        };
        let inv_masked = apply_depth_mask(&orig, &fx, &depth, &inv_config);
        assert_eq!(inv_masked.get_pixel(0, 0)[0], 200); // 1.0 - 0 = 1.0 fx
        assert_eq!(inv_masked.get_pixel(0, 1)[0], 0); // 1.0 - 1.0 = 0.0 fx
    }

    #[test]
    fn test_apply_depth_mask_threshold_and_disabled() {
        let orig = RgbaImage::from_pixel(2, 2, Rgba([50, 50, 50, 255]));
        let fx = RgbaImage::from_pixel(2, 2, Rgba([150, 150, 150, 255]));
        let mut depth = RgbaImage::new(2, 2);
        depth.put_pixel(0, 0, Rgba([50, 50, 50, 255])); // below threshold
        depth.put_pixel(1, 1, Rgba([220, 220, 220, 255])); // above threshold

        // Disabled returns fx
        let disabled_cfg = DepthMaskConfig {
            enabled: false,
            source: CreativeFxMaskSource::DepthMap,
            invert: false,
            threshold: 0.5,
            feather: 0.1,
        };
        let res_dis = apply_depth_mask(&orig, &fx, &depth, &disabled_cfg);
        assert_eq!(res_dis.get_pixel(0, 0)[0], 150);

        // Threshold 0.5 with feather 0.05
        let thresh_cfg = DepthMaskConfig {
            enabled: true,
            source: CreativeFxMaskSource::DepthMap,
            invert: false,
            threshold: 0.5,
            feather: 0.05,
        };
        let res_thresh = apply_depth_mask(&orig, &fx, &depth, &thresh_cfg);
        // (0,0) depth is ~0.20 < 0.45 -> weight 0 -> orig (50)
        assert_eq!(res_thresh.get_pixel(0, 0)[0], 50);
        // (1,1) depth is ~0.86 > 0.55 -> weight 1 -> fx (150)
        assert_eq!(res_thresh.get_pixel(1, 1)[0], 150);
    }

    #[test]
    fn test_apply_alpha_mask_and_inversion() {
        let orig = RgbaImage::from_pixel(3, 1, Rgba([20, 20, 20, 255]));
        let fx = RgbaImage::from_pixel(3, 1, Rgba([220, 220, 220, 255]));
        let mut removal = RgbaImage::new(3, 1);
        removal.put_pixel(0, 0, Rgba([255, 255, 255, 0]));
        removal.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        removal.put_pixel(2, 0, Rgba([0, 0, 0, 128]));

        let foreground = DepthMaskConfig {
            enabled: true,
            source: CreativeFxMaskSource::BackgroundRemovalAlpha,
            invert: false,
            // A depth threshold must not quantize background-removal alpha.
            threshold: 0.9,
            feather: 0.25,
        };
        let masked = apply_alpha_mask(&orig, &fx, &removal, &foreground);
        assert_eq!(masked.get_pixel(0, 0)[0], 20);
        assert_eq!(masked.get_pixel(1, 0)[0], 220);
        assert!((masked.get_pixel(2, 0)[0] as i16 - 120).abs() <= 1);

        let background = DepthMaskConfig {
            invert: true,
            ..foreground
        };
        let inverted = apply_alpha_mask(&orig, &fx, &removal, &background);
        assert_eq!(inverted.get_pixel(0, 0)[0], 220);
        assert_eq!(inverted.get_pixel(1, 0)[0], 20);
        assert!((inverted.get_pixel(2, 0)[0] as i16 - 120).abs() <= 1);
    }
}
