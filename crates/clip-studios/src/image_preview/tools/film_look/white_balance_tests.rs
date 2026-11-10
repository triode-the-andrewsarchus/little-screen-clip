use super::{
    DEFAULT_WB_STRENGTH, apply_auto_white_balance, apply_white_balance_gains, gray_world_gains,
    white_patch_gains,
};

fn channel_means(image: &image::RgbImage) -> [f32; 3] {
    let count = (image.width() * image.height()) as f32;
    std::array::from_fn(|channel| {
        image
            .pixels()
            .map(|pixel| f32::from(pixel[channel]))
            .sum::<f32>()
            / count
    })
}

#[test]
fn auto_white_balance_leaves_warm_scene_without_neutral_highlights_unchanged() {
    let mut image = image::RgbImage::from_pixel(64, 64, image::Rgb([220, 160, 100]));

    assert!(!apply_auto_white_balance(&mut image, 1.0));
    assert!(
        image
            .pixels()
            .all(|pixel| *pixel == image::Rgb([220, 160, 100]))
    );
}

#[test]
fn auto_white_balance_corrects_neutral_highlights_without_gray_world_scene_bias() {
    let mut image = image::RgbImage::from_fn(64, 64, |x, y| {
        if x >= 48 {
            return image::Rgb([240, 40 + (y % 5) as u8, 30]);
        }
        let base = 145 + ((x + y) % 70) as u8;
        image::Rgb([
            ((f32::from(base) * 0.78).round()) as u8,
            ((f32::from(base) * 0.92).round()) as u8,
            base,
        ])
    });
    let before = channel_means(&image::imageops::crop_imm(&image, 0, 0, 48, 64).to_image());

    assert!(apply_auto_white_balance(&mut image, 1.0));
    let after = channel_means(&image::imageops::crop_imm(&image, 0, 0, 48, 64).to_image());
    let spread = |channels: [f32; 3]| {
        channels.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - channels.iter().copied().fold(f32::INFINITY, f32::min)
    };
    assert!(
        spread(after) < 3.0,
        "WB should neutralize the cast: {before:?} -> {after:?}"
    );
    let red_patch = image.get_pixel(56, 32);
    let saturation = (f32::from(red_patch[0]) - f32::from(red_patch[1])) / f32::from(red_patch[0]);
    assert!(
        saturation > 0.75,
        "saturated scene color should remain vivid: {red_patch:?}"
    );
}

#[test]
fn auto_white_balance_strength_scales_the_estimated_correction() {
    let source = image::RgbImage::from_fn(64, 64, |x, y| {
        let base = 145 + ((x + y) % 70) as u8;
        image::Rgb([
            ((f32::from(base) * 0.78).round()) as u8,
            ((f32::from(base) * 0.92).round()) as u8,
            base,
        ])
    });
    let mut unchanged = source.clone();
    let mut corrected = source;

    assert!(!apply_auto_white_balance(&mut unchanged, 0.0));
    assert!(apply_auto_white_balance(&mut corrected, 0.5));
    assert_eq!(unchanged.get_pixel(32, 32), &image::Rgb([163, 192, 209]));
    assert!(channel_means(&corrected)[0] > channel_means(&unchanged)[0]);
    assert!(channel_means(&corrected)[2] < channel_means(&unchanged)[2]);
}

#[test]
fn default_auto_white_balance_strength_is_gentler_than_full_slider_value() {
    assert_eq!(DEFAULT_WB_STRENGTH, 0.75);
    let source = image::RgbImage::from_fn(64, 64, |x, y| {
        let base = 145 + ((x + y) % 70) as u8;
        image::Rgb([
            ((f32::from(base) * 0.78).round()) as u8,
            ((f32::from(base) * 0.92).round()) as u8,
            base,
        ])
    });
    let mut default_strength = source.clone();
    let mut full_strength = source.clone();

    assert!(apply_auto_white_balance(
        &mut default_strength,
        DEFAULT_WB_STRENGTH
    ));
    assert!(apply_auto_white_balance(&mut full_strength, 1.0));

    let total_channel_change = |image: &image::RgbImage| {
        image
            .pixels()
            .zip(source.pixels())
            .map(|(after, before)| {
                after
                    .0
                    .iter()
                    .zip(before.0)
                    .map(|(after, before)| i32::from(*after).abs_diff(i32::from(before)))
                    .sum::<u32>()
            })
            .sum::<u32>()
    };
    assert!(
        total_channel_change(&default_strength) < total_channel_change(&full_strength),
        "the default should apply a smaller channel correction"
    );
}

#[test]
fn auto_white_balance_does_not_gray_world_balance_a_warm_film_scene() {
    let source = image::RgbImage::from_pixel(64, 64, image::Rgb([220, 160, 100]));
    let stock = filmr::presets::kodak::KODAK_PORTRA_400();
    let auto = super::process_film_image(
        &source,
        &stock,
        &filmr::processor::SimulationConfig {
            white_balance_mode: filmr::processor::WhiteBalanceMode::Auto,
            white_balance_strength: 1.0,
            enable_grain: false,
            ..Default::default()
        },
    );
    let off = filmr::processor::process_image(
        &source,
        &stock,
        &filmr::processor::SimulationConfig {
            white_balance_mode: filmr::processor::WhiteBalanceMode::Off,
            enable_grain: false,
            ..Default::default()
        },
    );

    assert_eq!(
        auto, off,
        "without a neutral highlight, Auto should preserve scene color"
    );
}

#[test]
fn auto_white_balance_leaves_a_near_threshold_warm_gradient_unchanged() {
    let source = image::RgbImage::from_fn(64, 64, |x, y| {
        let base = 165 + ((x + y) % 85) as u8;
        image::Rgb([
            base,
            (f32::from(base) * 0.82).round() as u8,
            (f32::from(base) * 0.68).round() as u8,
        ])
    });
    let stock = filmr::presets::kodak::KODAK_PORTRA_400();
    let config = |mode| filmr::processor::SimulationConfig {
        white_balance_mode: mode,
        white_balance_strength: 1.0,
        enable_grain: false,
        ..Default::default()
    };
    let auto = super::process_film_image(
        &source,
        &stock,
        &config(filmr::processor::WhiteBalanceMode::Auto),
    );
    let off = super::process_film_image(
        &source,
        &stock,
        &config(filmr::processor::WhiteBalanceMode::Off),
    );
    assert_eq!(
        auto, off,
        "a warm scene gradient near the Auto chroma cutoff has no neutral reference"
    );
}

#[test]
fn auto_white_balance_keeps_near_threshold_green_dominant_scene_unchanged() {
    let source = image::RgbImage::from_fn(64, 64, |x, y| {
        let base = 165 + ((x + y) % 85) as u8;
        image::Rgb([
            (f32::from(base) * 0.68).round() as u8,
            base,
            (f32::from(base) * 0.82).round() as u8,
        ])
    });
    let stock = filmr::presets::kodak::KODAK_PORTRA_400();
    let config = |mode| filmr::processor::SimulationConfig {
        white_balance_mode: mode,
        white_balance_strength: 1.0,
        enable_grain: false,
        ..Default::default()
    };
    let auto = super::process_film_image(
        &source,
        &stock,
        &config(filmr::processor::WhiteBalanceMode::Auto),
    );
    let off = super::process_film_image(
        &source,
        &stock,
        &config(filmr::processor::WhiteBalanceMode::Off),
    );
    assert_eq!(
        auto, off,
        "a green-dominant scene must retain the conservative chroma cutoff"
    );
}

#[test]
fn film_pipeline_corrects_strong_blue_reference_without_flattening_vivid_colors() {
    let source = image::RgbImage::from_fn(128, 64, |x, y| {
        if x < 96 {
            let base = 150 + ((x + y) % 61) as u8;
            image::Rgb([
                (f32::from(base) * 0.65).round() as u8,
                (f32::from(base) * 0.78).round() as u8,
                base,
            ])
        } else {
            let color = match y / 22 {
                0 => [230, 25, 20],
                1 => [20, 230, 25],
                _ => [25, 30, 230],
            };
            image::Rgb(color)
        }
    });
    let stock = filmr::presets::kodak::KODAK_PORTRA_400();
    let config = |mode, strength| filmr::processor::SimulationConfig {
        white_balance_mode: mode,
        white_balance_strength: strength,
        enable_grain: false,
        ..Default::default()
    };
    let off = super::process_film_image(
        &source,
        &stock,
        &config(filmr::processor::WhiteBalanceMode::Off, 1.0),
    );
    let default_strength = super::process_film_image(
        &source,
        &stock,
        &config(
            filmr::processor::WhiteBalanceMode::Auto,
            DEFAULT_WB_STRENGTH,
        ),
    );
    let full_strength = super::process_film_image(
        &source,
        &stock,
        &config(filmr::processor::WhiteBalanceMode::Auto, 1.0),
    );
    let channel_spread = |pixel: &image::Rgb<u8>| {
        let max = *pixel.0.iter().max().unwrap();
        let min = *pixel.0.iter().min().unwrap();
        u16::from(max - min)
    };
    let off_neutral = off.get_pixel(40, 30);
    let default_neutral = default_strength.get_pixel(40, 30);
    let off_neutral_spread = channel_spread(off_neutral);
    let default_neutral_spread = channel_spread(default_neutral);
    assert!(
        default_neutral_spread * 2 < off_neutral_spread,
        "the default should substantially neutralize a reliable blue-cast reference: off={off_neutral:?}, default={default_neutral:?}"
    );

    let colorfulness = |pixel: &image::Rgb<u8>| {
        channel_spread(pixel) as f32 / f32::from(*pixel.0.iter().max().unwrap())
    };
    for (name, x, y) in [("red", 110, 10), ("green", 110, 30), ("blue", 110, 55)] {
        let off_pixel = off.get_pixel(x, y);
        let full_pixel = full_strength.get_pixel(x, y);
        assert!(
            colorfulness(full_pixel) >= colorfulness(off_pixel) * 0.85,
            "100% WB should retain vivid {name} scene colors: off={off_pixel:?}, full={full_pixel:?}"
        );
    }
}

#[test]
fn gray_and_white_modes_use_distinct_white_balance_references() {
    let source = image::RgbImage::from_fn(100, 100, |x, _| {
        if x < 10 {
            image::Rgb([180, 190, 210])
        } else {
            image::Rgb([80, 50, 30])
        }
    });
    let gray_gains = gray_world_gains(&source).expect("non-empty image has gray-world gains");
    let white_gains = white_patch_gains(&source).expect("non-empty image has white-patch gains");
    assert_ne!(gray_gains, white_gains);

    let mut corrected_white = source.clone();
    assert!(apply_white_balance_gains(
        &mut corrected_white,
        white_gains,
        1.0
    ));
    let white_patch = corrected_white.get_pixel(5, 50);
    assert!(
        (i16::from(white_patch[0]) - i16::from(white_patch[2])).abs() <= 1,
        "White mode should neutralize the bright reference patch: {white_patch:?}"
    );
}
