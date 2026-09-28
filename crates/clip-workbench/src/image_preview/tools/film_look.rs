use super::super::*;
use crate::shell::render_context_slider;

const DEFAULT_WB_STRENGTH: f32 = 0.35;

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
        if max_channel < 128.0 || (max_channel - min_channel) / max_channel.max(1.0) > 0.30 {
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
    if max_gain / min_gain > 1.5 {
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
mod white_balance_tests {
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
        let saturation =
            (f32::from(red_patch[0]) - f32::from(red_patch[1])) / f32::from(red_patch[0]);
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
        assert_eq!(DEFAULT_WB_STRENGTH, 0.35);
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
    fn gray_and_white_modes_use_distinct_white_balance_references() {
        let source = image::RgbImage::from_fn(100, 100, |x, _| {
            if x < 10 {
                image::Rgb([180, 190, 210])
            } else {
                image::Rgb([80, 50, 30])
            }
        });
        let gray_gains = gray_world_gains(&source).expect("non-empty image has gray-world gains");
        let white_gains =
            white_patch_gains(&source).expect("non-empty image has white-patch gains");
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
}

#[cfg(test)]
mod render_identity_tests {
    use super::{FilmSimMsg, ImagePreviewState};
    use crate::{AppConfig, image_preview::ImageToolMode};

    #[test]
    fn cancellable_filmr_preserves_pinned_pixels_and_honors_pre_cancel() {
        let image = image::RgbImage::from_fn(32, 24, |x, y| {
            image::Rgb([
                ((x * 7 + y * 3) % 256) as u8,
                ((x * 5 + y * 11) % 256) as u8,
                ((x * 13 + y * 17) % 256) as u8,
            ])
        });
        let stock = filmr::presets::kodak::KODAK_PORTRA_400();
        let pinned_stock = filmr_pinned::presets::kodak::KODAK_PORTRA_400();
        let config = filmr::processor::SimulationConfig {
            enable_grain: false,
            ..Default::default()
        };
        let pinned_config = filmr_pinned::processor::SimulationConfig {
            enable_grain: false,
            ..Default::default()
        };
        let pinned = filmr_pinned::processor::process_image(&image, &pinned_stock, &pinned_config);
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            filmr::processor::process_image_cancellable(&image, &stock, &config, &cancelled),
            Some(pinned)
        );
        cancelled.store(true, std::sync::atomic::Ordering::Release);
        assert!(
            filmr::processor::process_image_cancellable(&image, &stock, &config, &cancelled)
                .is_none()
        );
    }

    fn state() -> ImagePreviewState {
        let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([64, 96, 128, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".to_owned(),
            None,
            "film-look-test.png".to_owned(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    fn install_message(state: &mut ImagePreviewState, message: FilmSimMsg) {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(message).unwrap();
        state.film_sim_state.render_rx = Some(rx);
        state.film_sim_state.is_rendering = true;
        state.film_sim_state.render_stamp = Some(state.current_film_sim_render_stamp());
    }

    #[test]
    fn film_render_tokens_cancel_on_edits_exit_and_drop_then_reentry_renders() {
        let mut state = state();
        state.active_mode = ImageToolMode::FilmSim;
        state.run_film_sim_render();
        let parameter_job = state.film_sim_state.render_cancel.as_ref().unwrap().clone();
        state.film_sim_state.mark_dirty();
        assert!(parameter_job.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.film_sim_state.is_rendering);
        assert!(state.film_sim_state.render_rx.is_none());

        state.run_film_sim_render();
        let pixel_job = state.film_sim_state.render_cancel.as_ref().unwrap().clone();
        state.mark_pixels_changed();
        assert!(pixel_job.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.film_sim_state.is_rendering);

        state.run_film_sim_render();
        let exit_job = state.film_sim_state.render_cancel.as_ref().unwrap().clone();
        state.set_tool_mode(ImageToolMode::View);
        assert!(exit_job.load(std::sync::atomic::Ordering::Acquire));
        state.set_tool_mode(ImageToolMode::FilmSim);
        assert!(state.film_sim_state.is_rendering);
        let drop_job = state.film_sim_state.render_cancel.as_ref().unwrap().clone();
        drop(state);
        assert!(drop_job.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn film_sim_accepts_current_result_and_rejects_stale_success_and_failure() {
        let mut current = state();
        current.active_mode = ImageToolMode::FilmSim;
        let current_stamp = current.current_film_sim_render_stamp();
        install_message(
            &mut current,
            FilmSimMsg::Done {
                stamp: current_stamp,
                rgba: image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
                duration_ms: 12,
            },
        );
        current.poll_film_sim();
        assert!(current.film_sim_preview_is_current());

        let mut stale_success = state();
        let stale_stamp = stale_success.current_film_sim_render_stamp();
        install_message(
            &mut stale_success,
            FilmSimMsg::Done {
                stamp: stale_stamp,
                rgba: image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
                duration_ms: 12,
            },
        );
        stale_success.film_sim_state.mark_dirty();
        stale_success.poll_film_sim();
        assert!(stale_success.film_sim_state.preview_rgba.is_none());
        assert!(!stale_success.film_sim_preview_is_current());

        let mut stale_failure = state();
        let stale_stamp = stale_failure.current_film_sim_render_stamp();
        install_message(
            &mut stale_failure,
            FilmSimMsg::Error {
                stamp: stale_stamp,
                message: "stale render error".to_owned(),
            },
        );
        stale_failure.mark_pixels_changed();
        stale_failure.poll_film_sim();
        assert!(stale_failure.film_sim_state.render_error.is_none());
        assert!(!stale_failure.film_sim_state.is_rendering);
    }

    #[test]
    fn film_sim_cannot_apply_a_preview_after_its_inputs_change() {
        let mut state = state();
        let original = state.current_rgba.get_pixel(0, 0).to_owned();
        state.film_sim_state.preview_rgba = Some(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([1, 2, 3, 255]),
        ));
        state.film_sim_state.preview_stamp = Some(state.current_film_sim_render_stamp());

        state.film_sim_state.mark_dirty();
        state.apply_film_sim();

        assert_eq!(state.current_rgba.get_pixel(0, 0), &original);
        assert_eq!(state.undo_stack.len(), 0);
    }
}

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

/// Descriptor for one entry in the film stock picker list.
#[derive(Clone)]
pub struct FilmStockEntry {
    pub brand: &'static str,
    pub name: &'static str,
    pub iso: u32,
    /// "Color Neg", "Color Slide", "B&W"
    pub film_type_label: &'static str,
    pub build: fn() -> filmr::film::FilmStock,
}

/// All available film stock presets, grouped by brand in order.
pub fn all_film_stocks() -> Vec<FilmStockEntry> {
    vec![
        // ── Kodak ──
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 160",
            iso: 160,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_160,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_400,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 400 (Art)",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_400_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 800",
            iso: 800,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_800,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Gold 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_GOLD_200,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Ektar 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_EKTAR_100,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Ektachrome 100",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_EKTACHROME_100,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Ektachrome 100VS",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_EKTACHROME_100VS,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Kodachrome 25",
            iso: 25,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_KODACHROME_25,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Kodachrome 64",
            iso: 64,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_KODACHROME_64,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Tri-X 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::kodak::KODAK_TRI_X_400,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Tri-X 400 (Art)",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::kodak::KODAK_TRI_X_400_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Plus-X 125",
            iso: 125,
            film_type_label: "B&W",
            build: filmr::presets::kodak::KODAK_PLUS_X_125,
        },
        // ── Fujifilm ──
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Velvia 50",
            iso: 50,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::VELVIA_50,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Velvia 50 (Art)",
            iso: 50,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::VELVIA_50_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Provia 100F",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::PROVIA_100F,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Astia 100F",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::ASTIA_100F,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Superia 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::fujifilm::SUPERIA_100,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Superia 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::fujifilm::SUPERIA_200,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Superia 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::fujifilm::SUPERIA_400,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Neopan 100",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::fujifilm::NEOPAN_100,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Neopan 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::fujifilm::NEOPAN_400,
        },
        // ── Ilford ──
        FilmStockEntry {
            brand: "Ilford",
            name: "HP5 Plus 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::HP5_PLUS_400,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "HP5 Plus 400 (Art)",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::HP5_PLUS_400_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "FP4 Plus 125",
            iso: 125,
            film_type_label: "B&W",
            build: filmr::presets::ilford::FP4_PLUS_125,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Pan F Plus 50",
            iso: 50,
            film_type_label: "B&W",
            build: filmr::presets::ilford::PAN_F_PLUS_50,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Delta 100 Pro",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::ilford::DELTA_100_PROFESSIONAL,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Delta 400 Pro",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::DELTA_400_PROFESSIONAL,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "XP2 Super 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::XP2_SUPER_400,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "SFX 200",
            iso: 200,
            film_type_label: "B&W",
            build: filmr::presets::ilford::SFX_200,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Ortho Plus 80",
            iso: 80,
            film_type_label: "B&W",
            build: filmr::presets::ilford::ORTHO_PLUS_80,
        },
        // ── Agfa ──
        FilmStockEntry {
            brand: "Agfa",
            name: "Vista 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::VISTA_100,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Vista 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::VISTA_200,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Vista 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::VISTA_400,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "APX 100",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::agfa::APX_100,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "APX 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::agfa::APX_400,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Precisa 100",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::agfa::PRECISA_100,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Scala 200",
            iso: 200,
            film_type_label: "B&W",
            build: filmr::presets::agfa::SCALA_200,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Optima 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::OPTIMA_200,
        },
        // ── Polaroid ──
        FilmStockEntry {
            brand: "Polaroid",
            name: "600 Color",
            iso: 640,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_600_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "SX-70 Color",
            iso: 160,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_SX70_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "i-Type Color",
            iso: 640,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_I_TYPE_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "B&W 667",
            iso: 3000,
            film_type_label: "B&W",
            build: filmr::presets::polaroid::POLAROID_BW_667,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "Spectra Color",
            iso: 640,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_SPECTRA_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "100 Color",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_100_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "55 B&W",
            iso: 50,
            film_type_label: "B&W",
            build: filmr::presets::polaroid::POLAROID_55_BW,
        },
        // ── CineStill / Lomography / Other ──
        FilmStockEntry {
            brand: "CineStill",
            name: "800T",
            iso: 800,
            film_type_label: "Color Neg",
            build: filmr::presets::other::CINESTILL_800T,
        },
        FilmStockEntry {
            brand: "CineStill",
            name: "50D",
            iso: 50,
            film_type_label: "Color Neg",
            build: filmr::presets::other::CINESTILL_50D,
        },
        FilmStockEntry {
            brand: "Lomography",
            name: "Color Chrome",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::LOMOGRAPHY_COLOR_CHROME,
        },
        FilmStockEntry {
            brand: "Lomography",
            name: "LomoChrome Purple",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::LOMOGRAPHY_LOMOCHROME_PURPLE,
        },
        FilmStockEntry {
            brand: "Ferrania",
            name: "Solaris 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::other::FERRANIA_SOLARIS_100,
        },
        FilmStockEntry {
            brand: "Ferrania",
            name: "Solaris 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::FERRANIA_SOLARIS_400,
        },
        FilmStockEntry {
            brand: "ORWO",
            name: "UN54",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::other::ORWO_UN54,
        },
        FilmStockEntry {
            brand: "ORWO",
            name: "UN64",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::other::ORWO_UN64,
        },
        FilmStockEntry {
            brand: "Ricoh",
            name: "GR Street",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::other::RICOH_GR_STREET,
        },
        FilmStockEntry {
            brand: "Lucky",
            name: "Color 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::other::LUCKY_COLOR_200,
        },
        FilmStockEntry {
            brand: "Generic",
            name: "Standard Daylight",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::STANDARD_DAYLIGHT,
        },
    ]
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
    /// White balance correction strength: 0.0 .. 1.0, default 0.35 to preserve scene color.
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
    pub render_rx: Option<std::sync::mpsc::Receiver<FilmSimMsg>>,
    pub render_error: Option<String>,
    pub(crate) render_stamp: Option<FilmSimRenderStamp>,
    pub(crate) preview_stamp: Option<FilmSimRenderStamp>,
    render_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    parameter_revision: u64,

    // ── Async thumbnail worker (Sidebar 42x28 mini-photos) ──
    pub stock_thumbnails: std::collections::HashMap<usize, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(usize, image::RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<FilmSimThumbnailMsg>>,
    pub thumbnail_epoch: u64,
    pub(crate) thumbnail_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub thumbnail_dirty_at: Option<std::time::Instant>,
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
            parameter_revision: 0,
            stock_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_epoch: 0,
            thumbnail_cancel: None,
            thumbnail_dirty_at: None,
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
        if let Some(cancel) = self.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.render_rx = None;
        self.render_stamp = None;
        self.is_rendering = false;
    }

    pub(crate) fn cancel_thumbnail_batch(&mut self) {
        if let Some(cancel) = self.thumbnail_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.thumbnail_epoch = self.thumbnail_epoch.wrapping_add(1);
        self.thumbnail_rx = None;
        self.is_generating_thumbnails = false;
    }
}

impl ImagePreviewState {
    fn current_film_sim_render_stamp(&self) -> FilmSimRenderStamp {
        FilmSimRenderStamp {
            image: self.input_stamp(),
            parameter_revision: self.film_sim_state.parameter_revision,
        }
    }

    pub(crate) fn film_sim_preview_is_current(&self) -> bool {
        self.film_sim_state.preview_rgba.is_some()
            && self.film_sim_state.preview_stamp.as_ref()
                == Some(&self.current_film_sim_render_stamp())
    }

    pub fn run_film_sim_render(&mut self) {
        if self.film_sim_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }
        let stocks = all_film_stocks();
        let idx = self
            .film_sim_state
            .selected_stock_idx
            .min(stocks.len().saturating_sub(1));
        let entry = &stocks[idx];
        let mut stock = (entry.build)().with_style(self.film_sim_state.style);
        stock.halation_strength = self.film_sim_state.halation;
        stock.vignette_strength = self.film_sim_state.vignette;

        let config = SimulationConfig {
            exposure_time: self.film_sim_state.brightness,
            warmth: self.film_sim_state.warmth,
            saturation: self.film_sim_state.intensity,
            auto_levels: false,
            white_balance_mode: self.film_sim_state.wb_mode,
            white_balance_strength: self.film_sim_state.wb_strength,
            output_mode: OutputMode::Positive,
            enable_grain: self.film_sim_state.enable_grain,
            light_leak: self.film_sim_state.light_leak.to_config(),
            ..SimulationConfig::default()
        };

        // Convert RgbaImage → RgbImage for filmr
        let rgba = self.current_rgba.clone();
        let w = rgba.width();
        let h = rgba.height();
        let rgb_pixels: Vec<u8> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let rgb_img =
            image::RgbImage::from_raw(w, h, rgb_pixels).expect("pixel buffer size mismatch");

        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimMsg>();
        let stamp = self.current_film_sim_render_stamp();
        self.film_sim_state.render_stamp = Some(stamp.clone());
        self.film_sim_state.render_rx = Some(rx);
        self.film_sim_state.is_rendering = true;
        self.film_sim_state.render_error = None;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.film_sim_state.render_cancel = Some(cancelled.clone());

        std::thread::spawn(move || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                process_film_image_inner(&rgb_img, &stock, &config, Some(&cancelled))
            }));
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            match process_result {
                Ok(Some(out_rgb)) => {
                    let Some(out_rgba) = apply_film_sim_post_adjustments_inner(
                        &out_rgb,
                        contrast,
                        tint,
                        Some(&cancelled),
                    ) else {
                        return;
                    };
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }
                    let duration_ms = start.elapsed().as_millis();
                    let _ = tx.send(FilmSimMsg::Done {
                        stamp,
                        rgba: out_rgba,
                        duration_ms,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    let err_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Film simulation panicked".to_string()
                    };
                    let _ = tx.send(FilmSimMsg::Error {
                        stamp,
                        message: err_msg,
                    });
                }
            }
        });
    }

    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all film stocks.

    pub fn run_thumbnail_batch(&mut self) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.film_sim_state.cancel_thumbnail_batch();
        self.film_sim_state.thumbnail_epoch = self.film_sim_state.thumbnail_epoch.wrapping_add(1);
        let epoch = self.film_sim_state.thumbnail_epoch;

        // Downscale base image to 42x28 (sub-3ms per stock)
        let thumb_base = image::imageops::resize(
            &self.current_rgba,
            42,
            28,
            image::imageops::FilterType::Nearest,
        );
        let rgb_pixels: Vec<u8> = thumb_base
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let thumb_rgb = match image::RgbImage::from_raw(42, 28, rgb_pixels) {
            Some(img) => img,
            None => return,
        };

        let brightness = self.film_sim_state.brightness;
        let warmth = self.film_sim_state.warmth;
        let intensity = self.film_sim_state.intensity;
        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;
        let wb_mode = self.film_sim_state.wb_mode;
        let wb_strength = self.film_sim_state.wb_strength;
        let halation = self.film_sim_state.halation;
        let vignette = self.film_sim_state.vignette;
        let light_leak = self.film_sim_state.light_leak;
        let style = self.film_sim_state.style;
        let enable_grain = self.film_sim_state.enable_grain;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimThumbnailMsg>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut completed: std::collections::HashSet<usize> = self
            .film_sim_state
            .stock_thumbnails
            .keys()
            .copied()
            .collect();
        completed.extend(
            self.film_sim_state
                .pending_thumbnails
                .iter()
                .map(|(idx, _)| *idx),
        );
        let todo: Vec<_> = all_film_stocks()
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| !completed.contains(idx))
            .collect();
        if todo.is_empty() {
            return;
        }
        self.film_sim_state.thumbnail_cancel = Some(cancelled.clone());
        self.film_sim_state.thumbnail_rx = Some(rx);
        self.film_sim_state.is_generating_thumbnails = true;

        let spawn_result = std::thread::Builder::new()
            .name("film_sim_thumbnails".to_string())
            .spawn(move || {
                super::run_thumbnail_items(
                    todo,
                    || cancelled.load(std::sync::atomic::Ordering::Acquire),
                    |(idx, entry)| {
                        let mut stock = (entry.build)().with_style(style);
                        stock.halation_strength = halation;
                        stock.vignette_strength = vignette;

                        let config = SimulationConfig {
                            exposure_time: brightness,
                            warmth,
                            saturation: intensity,
                            auto_levels: false,
                            white_balance_mode: wb_mode,
                            white_balance_strength: wb_strength,
                            output_mode: OutputMode::Positive,
                            enable_grain,
                            light_leak: light_leak.to_config(),
                            ..SimulationConfig::default()
                        };

                        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            process_film_image(&thumb_rgb, &stock, &config)
                        }));

                        if let Ok(out_rgb) = res {
                            let out_rgba =
                                apply_film_sim_post_adjustments(&out_rgb, contrast, tint);
                            tx.send(FilmSimThumbnailMsg {
                                epoch,
                                stock_idx: idx,
                                rgba: out_rgba,
                            })
                            .is_ok()
                        } else {
                            true
                        }
                    },
                );
            });
        if spawn_result.is_err() {
            self.film_sim_state.cancel_thumbnail_batch();
        }
    }

    /// Drain any messages from the film sim background thread and thumbnail worker.

    pub fn poll_film_sim(&mut self) {
        let current_stamp = self.current_film_sim_render_stamp();
        if self
            .film_sim_state
            .render_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &current_stamp)
            || (self.film_sim_state.is_rendering && self.active_mode != ImageToolMode::FilmSim)
        {
            self.film_sim_state.cancel_render();
        }
        if let Some(rx) = self.film_sim_state.render_rx.take() {
            match rx.try_recv() {
                Ok(FilmSimMsg::Done {
                    stamp,
                    rgba,
                    duration_ms,
                }) => {
                    if stamp == current_stamp
                        && self.film_sim_state.render_stamp.as_ref() == Some(&stamp)
                    {
                        self.film_sim_state.preview_rgba = Some(rgba);
                        self.film_sim_state.preview_stamp = Some(stamp);
                        self.film_sim_state.dirty_preview_texture = true;
                        self.film_sim_state.last_render_duration_ms = duration_ms;
                        self.film_sim_state.render_error = None;
                    }
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                }
                Ok(FilmSimMsg::Error { stamp, message }) => {
                    if stamp == current_stamp
                        && self.film_sim_state.render_stamp.as_ref() == Some(&stamp)
                    {
                        self.film_sim_state.render_error = Some(message);
                    }
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.film_sim_state.render_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                }
            }
        }

        // Drain pending thumbnail messages
        let mut thumbs_done = false;
        if let Some(rx) = self.film_sim_state.thumbnail_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        if msg.epoch == self.film_sim_state.thumbnail_epoch {
                            self.film_sim_state
                                .pending_thumbnails
                                .push((msg.stock_idx, msg.rgba));
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        keep = false;
                        thumbs_done = true;
                        break;
                    }
                }
            }
            if keep {
                self.film_sim_state.thumbnail_rx = Some(rx);
            }
        }
        if thumbs_done {
            self.film_sim_state.is_generating_thumbnails = false;
            self.film_sim_state.thumbnail_cancel = None;
        }
    }

    /// Commit the film sim preview into current_rgba and push to undo stack.

    pub fn apply_film_sim(&mut self) {
        if !self.film_sim_preview_is_current() {
            return;
        }
        let preview = match self.film_sim_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = preview;
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;
        // Reset sim state so re-entering the tab renders fresh.
        self.film_sim_state.preview_rgba = None;
        self.film_sim_state.preview_stamp = None;
        self.film_sim_state.preview_texture = None;
        self.film_sim_state.dirty_preview_texture = false;
        // Invalidate and regenerate thumbnails for updated image
        self.film_sim_state.stock_thumbnails.clear();
        self.film_sim_state.pending_thumbnails.clear();
        self.run_thumbnail_batch();
        self.show_toast("🎞 Film simulation applied");
    }

    /// Full 3-panel film simulation viewport (replaces the normal render_viewport when FilmSim is active).

    pub(crate) fn render_film_sim_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);

        // Upload any newly received thumbnails into GPU textures
        for (idx, rgba) in self.film_sim_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("film_sim_thumb_{}", idx),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.film_sim_state.stock_thumbnails.insert(idx, tex);
        }

        // Fill thumbnails that were still outstanding when the mode was left.
        if self.film_sim_state.stock_thumbnails.len() + self.film_sim_state.pending_thumbnails.len()
            < all_film_stocks().len()
            && !self.film_sim_state.is_generating_thumbnails
            && !self.current_rgba.is_empty()
        {
            self.run_thumbnail_batch();
        }

        let panel_layout = ImageToolMode::FilmSim
            .panel_layout()
            .expect("Film Look has a panel layout")
            .with_panel_sizes(&self.panel_state);
        let panels = panel_layout.allocate(ui);
        let left = panels.left.expect("Film Look has a stock browser");
        let right = panels.right.expect("Film Look has an inspector");
        let bottom = panels.bottom.expect("Film Look has a control strip");
        let left_w = left.width();
        let left_h = left.height();
        let right_w = right.width();
        let right_h = right.height();
        let center_w = panels.center.width();
        let main_h = panels.center.height();

        let stocks = all_film_stocks();

        // Ensure fit-to-view is performed on the center canvas when first entering FilmSim
        if !self.film_sim_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.film_sim_state.auto_fitted = true;
        }

        // ── 1. LEFT PANEL: Film Stock Picker with Async 42x28 Mini-Photos ──
        crate::shell::StudioPanelLayout::show_region(ui, left, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(left_w - 16.0);
                    ui.set_height((left_h - 16.0).max(1.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Film Stock")
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(base_c),
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", stocks.len()))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        if self.film_sim_state.is_generating_thumbnails {
                            ui.label(
                                egui::RichText::new("...")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            )
                            .on_hover_text("Generating thumbnails in background…");
                        }
                    });
                    ui.add_space(crate::ui_ext::SPACE_XS);

                    // Search box
                    ui.add_sized(
                        [left_w - 24.0, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut self.film_sim_state.search_query)
                            .hint_text("Filter stocks...")
                            .desired_width(left_w - 24.0),
                    );
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.separator();
                    ui.add_space(2.0);

                    let search = self.film_sim_state.search_query.trim().to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_source("film_sim_stock_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(left_w - 24.0);
                            let mut current_brand = "";
                            for (i, entry) in stocks.iter().enumerate() {
                                if !search.is_empty() {
                                    let matches = entry.name.to_lowercase().contains(&search)
                                        || entry.brand.to_lowercase().contains(&search)
                                        || entry.film_type_label.to_lowercase().contains(&search);
                                    if !matches {
                                        continue;
                                    }
                                }

                                // Brand separator heading
                                if entry.brand != current_brand {
                                    if !current_brand.is_empty() {
                                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                    }
                                    ui.label(
                                        egui::RichText::new(entry.brand.to_uppercase())
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    current_brand = entry.brand;
                                }

                                let selected = self.film_sim_state.selected_stock_idx == i;
                                let (row_bg, name_col, meta_col) = if selected {
                                    (
                                        base_c,
                                        contrast_text_c32(base_c, None, palette),
                                        contrast_text_c32(base_c, None, palette),
                                    )
                                } else {
                                    (b_low, f_high, f_med)
                                };

                                let row_frame = egui::Frame::none()
                                    .fill(row_bg)
                                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                    .inner_margin(egui::Margin::symmetric(4.0, 3.0));

                                let resp = row_frame
                                    .show(ui, |ui| {
                                        ui.set_width(left_w - 32.0);
                                        ui.horizontal(|ui| {
                                            // 42x28 mini-photo thumbnail preview
                                            if let Some(tex) =
                                                self.film_sim_state.stock_thumbnails.get(&i)
                                            {
                                                ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                            } else {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(42.0, 28.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    Rounding::same(2.0),
                                                    b_med,
                                                );
                                            }
                                            ui.add_space(2.0);
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(entry.name)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(name_col),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(entry.film_type_label)
                                                            .monospace()
                                                            .color(meta_col),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "ISO {}",
                                                            entry.iso
                                                        ))
                                                        .monospace()
                                                        .color(meta_col),
                                                    );
                                                });
                                            });
                                        });
                                    })
                                    .response;

                                if resp.interact(egui::Sense::click()).clicked() && !selected {
                                    self.film_sim_state.selected_stock_idx = i;
                                    self.film_sim_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }
                        });
                });
        });

        // ── 2. CENTER PANEL: Split-view Comparison Canvas ──
        crate::shell::StudioPanelLayout::show_region(ui, panels.center, |ui| {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.canvas))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.set_width(center_w);
                    ui.set_height(main_h);

                    // Reload preview texture if dirty
                    if self.film_sim_state.dirty_preview_texture {
                        if let Some(ref rgba) = self.film_sim_state.preview_rgba {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            );
                            self.film_sim_state.preview_texture = Some(ctx.load_texture(
                                "film_sim_preview_tex",
                                ci,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        self.film_sim_state.dirty_preview_texture = false;
                    }

                    let (response, painter) = ui.allocate_painter(
                        Vec2::new(center_w, main_h),
                        egui::Sense::click_and_drag(),
                    );
                    let canvas_rect = response.rect;

                    painter.rect_filled(
                        canvas_rect,
                        Rounding::same(4.0),
                        hex_to_c32(&palette.colors.canvas),
                    );

                    // If mouse is near the comparison split divider when drag starts, initiate split dragging
                    let near_split = if let Some(pos) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                    {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));
                        let split_x = img_rect.min.x
                            + self.film_sim_state.comparison_split.clamp(0.01, 0.99)
                                * img_rect.width();
                        (pos.x - split_x).abs() <= 18.0
                            && pos.y >= img_rect.min.y
                            && pos.y <= img_rect.max.y
                    } else {
                        false
                    };
                    if near_split && response.drag_started() {
                        self.film_sim_state.is_dragging_split = true;
                    }

                    // Pan/zoom via this response
                    let middle_drag =
                        ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                    if (!self.film_sim_state.is_dragging_split && response.dragged()) || middle_drag
                    {
                        self.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                        if scroll_delta.abs() > 0.0 {
                            let zoom_factor = (scroll_delta * 0.002).exp();
                            let old_zoom = self.zoom;
                            let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 5.0);
                            if let Some(ptr_pos) = response.hover_pos() {
                                let center = canvas_rect.center() + self.pan;
                                let to_ptr = ptr_pos - center;
                                self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                            }
                            self.zoom = new_zoom;
                        }
                    }
                    if !self.film_sim_state.is_dragging_split && response.double_clicked() {
                        let avail_size = Vec2::new(center_w, main_h);
                        self.fit_to_view(avail_size);
                    }

                    if let Some(orig_tex) = self.texture.clone() {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                        draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                        self.render_film_sim_comparison(
                            ui,
                            &painter,
                            &response,
                            img_rect,
                            canvas_rect,
                            ctx,
                            &orig_tex,
                            palette,
                        );

                        painter.rect_stroke(
                            img_rect,
                            Rounding::ZERO,
                            Stroke::new(
                                1.0_f32,
                                hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                            ),
                        );
                    }

                    // Rendering spinner overlay
                    if self.film_sim_state.is_rendering {
                        let spinner_pos = canvas_rect.center();
                        let spin_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
                        let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                        painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                        painter.rect_stroke(
                            spin_rect,
                            Rounding::same(8.0),
                            Stroke::new(1.0_f32, b_med),
                        );
                        painter.text(
                            spinner_pos,
                            egui::Align2::CENTER_CENTER,
                            "⏳ Rendering film simulation…",
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            f_med,
                        );
                    }

                    // Error overlay
                    if let Some(ref err) = self.film_sim_state.render_error.clone() {
                        let err_pos = Pos2::new(canvas_rect.min.x + 10.0, canvas_rect.min.y + 10.0);
                        painter.text(
                            err_pos,
                            egui::Align2::LEFT_TOP,
                            format!("⚠ {}", err),
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            hex_to_c32(&palette.diff_del),
                        );
                    }
                });
        });

        // ── 3. RIGHT PANEL: Style + Optical Effects + Stock Info ──
        crate::shell::StudioPanelLayout::show_region(ui, right, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(right_w - 16.0);
                    ui.set_height((right_h - 16.0).max(1.0));

                    egui::ScrollArea::vertical()
                        .id_source("film_sim_right_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(right_w - 24.0);

                            // 1. Style
                            ui.label(
                                egui::RichText::new("Style")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let styles = [
                                (
                                    FilmStyle::Accurate,
                                    "Accurate",
                                    "Physics-faithful rendering",
                                ),
                                (
                                    FilmStyle::Artistic,
                                    "Artistic",
                                    "Boosted contrast & saturation",
                                ),
                                (FilmStyle::Vintage, "Vintage", "Warm, faded, nostalgic"),
                                (FilmStyle::HighContrast, "Hi-Con", "High contrast, punchy"),
                                (FilmStyle::Pastel, "Pastel", "Low-contrast, soft tones"),
                            ];

                            for (style, label, tooltip) in &styles {
                                let sel = self.film_sim_state.style == *style;
                                let (s_bg, s_fg) = if sel {
                                    (base_c, contrast_text_c32(base_c, None, palette))
                                } else {
                                    (b_med, f_med)
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(*label)
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(s_fg),
                                )
                                .fill(s_bg)
                                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .min_size(Vec2::new(right_w - 28.0, crate::ui_ext::CONTROL_HEIGHT));

                                if ui.add(btn).on_hover_text(*tooltip).clicked() && !sel {
                                    self.film_sim_state.style = *style;
                                    self.film_sim_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // 2. Optical Effects: Halation, Vignette, Light Leak, Grain
                            ui.label(
                                egui::RichText::new("Optics & Effects")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let full_w = (right_w - 32.0).max(100.0);
                            let opt_slider_w = (full_w - 68.0).max(60.0);

                            if render_context_slider(
                                ui,
                                "Halation",
                                &mut self.film_sim_state.halation,
                                0.0..=1.0,
                                0.25,
                                "%",
                                opt_slider_w,
                                palette,
                            ) {
                                self.film_sim_state.mark_dirty();
                            }
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            if render_context_slider(
                                ui,
                                "Vignette",
                                &mut self.film_sim_state.vignette,
                                0.0..=1.0,
                                0.35,
                                "%",
                                opt_slider_w,
                                palette,
                            ) {
                                self.film_sim_state.mark_dirty();
                            }
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            ui.label(
                                egui::RichText::new("Light Leak")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_med),
                            );
                            egui::ComboBox::from_id_source("film_sim_light_leak_combo")
                                .selected_text(self.film_sim_state.light_leak.label())
                                .width(full_w)
                                .show_ui(ui, |ui| {
                                    let presets = [
                                        LightLeakPreset::None,
                                        LightLeakPreset::WarmCorner,
                                        LightLeakPreset::SunStreak,
                                        LightLeakPreset::SoftEdge,
                                        LightLeakPreset::OrganicBlob,
                                    ];
                                    for p in presets {
                                        let sel = self.film_sim_state.light_leak == p;
                                        if ui.selectable_label(sel, p.label()).clicked() && !sel {
                                            self.film_sim_state.light_leak = p;
                                            self.film_sim_state.mark_dirty();
                                        }
                                    }
                                });
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let mut grain_on = self.film_sim_state.enable_grain;
                            if ui
                                .checkbox(
                                    &mut grain_on,
                                    egui::RichText::new("Grain emulation")
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_med),
                                )
                                .changed()
                            {
                                self.film_sim_state.enable_grain = grain_on;
                                self.film_sim_state.mark_dirty();
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // 3. Stock metadata
                            let idx = self
                                .film_sim_state
                                .selected_stock_idx
                                .min(stocks.len().saturating_sub(1));
                            let entry = &stocks[idx];
                            ui.label(
                                egui::RichText::new("Stock Info")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let stock_built = (entry.build)();
                            egui::Grid::new("film_sim_stock_meta_grid")
                                .spacing(Vec2::new(6.0, 3.0))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("Brand:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.brand)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Film:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.name)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("ISO:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("{}", entry.iso))
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Type:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.film_type_label)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Res:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:.0} lp/mm",
                                            stock_built.resolution_lp_mm
                                        ))
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_low),
                                    );
                                    ui.end_row();
                                });

                            if self.film_sim_state.last_render_duration_ms > 0 {
                                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "⏱ {:.2}s",
                                        self.film_sim_state.last_render_duration_ms as f64 / 1000.0
                                    ))
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                                );
                            }
                        });
                });
        });

        // ── BOTTOM STRIP: Context Sliders + WB Controls + Action Buttons ───────────
        crate::shell::StudioPanelLayout::show_region(ui, bottom, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::symmetric(
                    crate::ui_ext::SPACE_MD,
                    crate::ui_ext::CONTROL_GROUP_INSET_Y,
                ))
                .show(ui, |ui| {
                    ui.set_width((bottom.width() - 24.0).max(1.0));
                    egui::ScrollArea::both()
                        .id_source("film_sim_bottom_controls")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                // Row 1: Exposure & Color Sliders with contextual start/center/end/default tracks
                                ui.horizontal(|ui| {
                                    let slider_w =
                                        ((bottom.width() - 48.0) / 6.0 - 12.0).clamp(70.0, 115.0);

                                    // Brightness
                                    if render_context_slider(
                                        ui,
                                        "Brightness",
                                        &mut self.film_sim_state.brightness,
                                        0.1..=4.0,
                                        1.0,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Contrast
                                    if render_context_slider(
                                        ui,
                                        "Contrast",
                                        &mut self.film_sim_state.contrast,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Warmth
                                    if render_context_slider(
                                        ui,
                                        "Warmth",
                                        &mut self.film_sim_state.warmth,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Tint (Green ↔ Magenta)
                                    if render_context_slider(
                                        ui,
                                        "Tint",
                                        &mut self.film_sim_state.tint,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Saturation
                                    if render_context_slider(
                                        ui,
                                        "Saturation",
                                        &mut self.film_sim_state.intensity,
                                        0.0..=2.0,
                                        1.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // WB Strength
                                    if render_context_slider(
                                        ui,
                                        "WB Str",
                                        &mut self.film_sim_state.wb_strength,
                                        0.0..=1.0,
                                        DEFAULT_WB_STRENGTH,
                                        "%",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                });

                                ui.add_space(3.0);

                                // Row 2: WB Mode Selector + Auto Presets + Actions
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("WB:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    let wb_modes = [
                                        (WhiteBalanceMode::Auto, "Auto"),
                                        (WhiteBalanceMode::Gray, "Gray"),
                                        (WhiteBalanceMode::White, "White"),
                                        (WhiteBalanceMode::Off, "Off"),
                                    ];
                                    for (mode, label) in wb_modes {
                                        let sel = self.film_sim_state.wb_mode == mode;
                                        let (btn_bg, btn_fg) = if sel {
                                            (base_c, contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_med, f_med)
                                        };
                                        let b = egui::Button::new(
                                            egui::RichText::new(label)
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .strong()
                                                .color(btn_fg),
                                        )
                                        .fill(btn_bg)
                                        .stroke(Stroke::new(
                                            1.0_f32,
                                            if sel { base_c } else { b_low },
                                        ))
                                        .rounding(Rounding::same(
                                            crate::ui_ext::BADGE_CORNER_RADIUS,
                                        ))
                                        .min_size(Vec2::new(36.0, crate::ui_ext::CONTROL_HEIGHT));
                                        let description = match mode {
                                            WhiteBalanceMode::Auto => {
                                                "Balance from plausible neutral highlights; skips images without reliable neutral samples"
                                            }
                                            WhiteBalanceMode::Gray => {
                                                "Balance the scene-wide average toward neutral gray"
                                            }
                                            WhiteBalanceMode::White => {
                                                "Balance each channel from its brightest 1% white-patch reference"
                                            }
                                            WhiteBalanceMode::Off => "Do not apply white-balance correction",
                                        };
                                        if ui.add(b).on_hover_text(description).clicked() && !sel {
                                            self.film_sim_state.wb_mode = mode;
                                            self.film_sim_state.mark_dirty();
                                        }
                                    }

                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                    ui.separator();
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    // Auto Levels
                                    let al_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Wand),
                                        "Auto Levels",
                                        diff_mod,
                                        palette,
                                    );
                                    if ui
                            .add(al_btn)
                            .on_hover_text(
                                "Reset brightness to neutral, warmth to 0, and tint to 0",
                            )
                            .clicked()
                        {
                            self.film_sim_state.brightness = 1.0;
                            self.film_sim_state.warmth = 0.0;
                            self.film_sim_state.tint = 0.0;
                            self.film_sim_state.mark_dirty();
                            self.show_toast("Auto levels applied");
                        }

                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Auto Enhance
                                    let ae_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Sparkles),
                                        "Auto Enhance",
                                        base_c,
                                        palette,
                                    );
                                    if ui
                            .add(ae_btn)
                            .on_hover_text(
                                "Boost intensity, warmth, and contrast for a cinematic look",
                            )
                            .clicked()
                        {
                            self.film_sim_state.brightness =
                                (self.film_sim_state.brightness * 1.05).min(4.0);
                            self.film_sim_state.intensity =
                                (self.film_sim_state.intensity + 0.15).min(2.0);
                            self.film_sim_state.warmth =
                                (self.film_sim_state.warmth + 0.05).min(1.0);
                            self.film_sim_state.contrast =
                                (self.film_sim_state.contrast + 0.08).min(1.0);
                            self.film_sim_state.mark_dirty();
                            self.show_toast("Auto enhance applied");
                        }

                                    // Right aligned: Apply, Reset, Undo
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let apply_btn = crate::ui_ext::action_button_icon(
                                                Some(Icon::Check),
                                                "Apply to Image",
                                                diff_add,
                                                palette,
                                            );
                                            let apply_enabled = self.film_sim_preview_is_current();
                                            if ui
                                .add_enabled(apply_enabled, apply_btn)
                                .on_hover_text(
                                    "Commit film simulation result to image (adds to undo stack)",
                                )
                                .clicked()
                            {
                                self.apply_film_sim();
                            }

                                            ui.add_space(crate::ui_ext::SPACE_XS);

                                            let reset_btn = crate::ui_ext::secondary_button_icon(
                                                Some(Icon::RotateCcw),
                                                "Reset",
                                                b_med,
                                                palette,
                                            );
                                            if ui
                                                .add(reset_btn)
                                                .on_hover_text(
                                                    "Reset all film sim parameters to defaults",
                                                )
                                                .clicked()
                                            {
                                                self.film_sim_state.brightness = 1.0;
                                                self.film_sim_state.contrast = 0.0;
                                                self.film_sim_state.warmth = 0.0;
                                                self.film_sim_state.tint = 0.0;
                                                self.film_sim_state.intensity = 1.0;
                                                self.film_sim_state.wb_mode =
                                                    WhiteBalanceMode::Auto;
                                                self.film_sim_state.wb_strength =
                                                    DEFAULT_WB_STRENGTH;
                                                self.film_sim_state.halation = 0.25;
                                                self.film_sim_state.vignette = 0.35;
                                                self.film_sim_state.light_leak =
                                                    LightLeakPreset::None;
                                                self.film_sim_state.enable_grain = true;
                                                self.film_sim_state.style = FilmStyle::Accurate;
                                                self.film_sim_state.mark_dirty();
                                                self.show_toast(
                                                    "⟲ All parameters reset to defaults",
                                                );
                                            }

                                            ui.add_space(crate::ui_ext::SPACE_XS);

                                            if !self.undo_stack.is_empty() {
                                                let undo_fg = contrast_text_c32(
                                                    b_med,
                                                    Some(diff_mod),
                                                    palette,
                                                );
                                                let undo_btn = egui::Button::new(
                                                    egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(undo_fg),
                                                )
                                                .fill(b_med)
                                                .rounding(Rounding::same(
                                                    crate::ui_ext::CONTROL_CORNER_RADIUS,
                                                ));
                                                if ui.add(undo_btn).clicked() {
                                                    self.undo();
                                                    self.film_sim_state.preview_rgba = None;
                                                    self.film_sim_state.preview_texture = None;
                                                    self.film_sim_state.mark_dirty();
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                        });
                });
        });
        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            "film_look",
        );
    }

    /// Split-view comparison painter for the Film Sim tab center canvas.
    #[allow(clippy::too_many_arguments)]

    pub(crate) fn render_film_sim_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        _ctx: &egui::Context,
        orig_tex: &egui::TextureHandle,
        palette: &AppPalette,
    ) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // Render split comparison using unified SplitCompareView
        let split_view = crate::shell::SplitCompareView::new(
            orig_tex,
            self.film_sim_state.preview_texture.as_ref(),
            &mut self.film_sim_state.comparison_split,
            &mut self.film_sim_state.is_dragging_split,
        );
        split_view.paint(ui, response, painter, img_rect, palette);

        // HUD badges
        let badge_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
        let badge_font = egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE);
        let orig_badge_w = painter
            .layout_no_wrap("ORIGINAL".to_owned(), badge_font.clone(), b_med)
            .size()
            .x
            + 20.0;

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(orig_badge_w, crate::ui_ext::CONTROL_HEIGHT),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0_f32, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            badge_font.clone(),
            hex_to_c32(&palette.colors.text_secondary),
        );

        let stocks = all_film_stocks();
        let idx = self
            .film_sim_state
            .selected_stock_idx
            .min(stocks.len().saturating_sub(1));
        let sim_label = if self.film_sim_state.is_rendering {
            "⏳ Rendering…".to_string()
        } else {
            format!(
                "🎞 {} ({:.1}s)",
                stocks[idx].name,
                self.film_sim_state.last_render_duration_ms as f64 / 1000.0
            )
        };
        let sim_badge_w = painter
            .layout_no_wrap(sim_label.clone(), badge_font.clone(), base_c)
            .size()
            .x
            + 20.0;
        let sim_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - sim_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(sim_badge_w, crate::ui_ext::CONTROL_HEIGHT),
        );
        painter.rect_filled(sim_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(sim_rect, Rounding::same(4.0), Stroke::new(1.0_f32, base_c));
        painter.text(
            sim_rect.center(),
            egui::Align2::CENTER_CENTER,
            sim_label,
            badge_font,
            base_c,
        );
    }
}
