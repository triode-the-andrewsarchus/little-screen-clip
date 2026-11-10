use super::{FilmSimMsg, ImagePreviewState, film_rgb_from_rgba};
use crate::image_preview::ImageToolMode;
use little_clip_core::AppConfig;

#[test]
fn film_rgb_snapshot_preserves_channels_and_cancels_between_rows() {
    let rgba = image::RgbaImage::from_fn(2, 3, |x, y| {
        image::Rgba([x as u8, y as u8, (x + y) as u8, (x * 127 + y) as u8])
    });
    let rgb = film_rgb_from_rgba(&rgba, || false).unwrap();
    for (input, output) in rgba.pixels().zip(rgb.pixels()) {
        assert_eq!(output.0, [input[0], input[1], input[2]]);
    }

    let mut rows_checked = 0;
    assert!(
        film_rgb_from_rgba(&rgba, || {
            rows_checked += 1;
            rows_checked == 2
        })
        .is_none()
    );
    assert_eq!(rows_checked, 2);
}

#[test]
fn queued_render_keeps_no_owned_source_snapshot() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut state = state();
    state.active_mode = ImageToolMode::FilmSim;
    let source = Arc::downgrade(&state.current_rgba);
    let owners_before = Arc::strong_count(&state.current_rgba);
    state.run_film_sim_render_on(&jobs);
    assert_eq!(Arc::strong_count(&state.current_rgba), owners_before);
    assert!(state.film_sim_state.is_rendering);
    drop(state);
    assert!(source.upgrade().is_none());

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
}

#[test]
fn editing_shared_pixels_preserves_history_and_undo_redo() {
    use std::sync::Arc;

    let mut state = state();
    let old_pixels = state.current_rgba.clone();
    state.undo_stack.push(state.current_rgba.clone());
    Arc::make_mut(&mut state.current_rgba).put_pixel(0, 0, image::Rgba([7, 11, 13, 17]));
    state.mark_pixels_changed();
    assert_eq!(old_pixels.get_pixel(0, 0).0, [64, 96, 128, 255]);
    assert_eq!(state.current_rgba.get_pixel(0, 0).0, [7, 11, 13, 17]);

    state.undo();
    assert_eq!(state.current_rgba.get_pixel(0, 0).0, [64, 96, 128, 255]);
    state.redo();
    assert_eq!(state.current_rgba.get_pixel(0, 0).0, [7, 11, 13, 17]);
}

#[test]
#[ignore = "requires a working GPU compute adapter"]
fn filmr_gpu_renders_and_matches_cpu_without_grain() {
    let gpu = filmr::gpu::get_gpu_context().expect("GPU adapter and device available");
    println!("GPU adapter: {:?}", gpu.adapter_info);
    assert_ne!(
        format!("{:?}", gpu.adapter_info.device_type),
        "Cpu",
        "hardware test must not use a software adapter"
    );
    let input = image::RgbImage::from_fn(33, 19, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x + y) * 5) as u8])
    });
    let buffer = filmr::gpu_pipelines::get_linearize_pipeline(gpu)
        .process_to_gpu_buffer(gpu, &input)
        .expect("GPU linearization dispatched");
    let linear = filmr::gpu::block_on(filmr::gpu_pipelines::read_gpu_buffer(gpu, &buffer))
        .expect("GPU output read back");
    let cpu_linear = filmr::pipeline::create_linear_image(&input);
    assert_eq!(linear.dimensions(), input.dimensions());
    for (gpu, cpu) in linear.as_raw().iter().zip(cpu_linear.as_raw()) {
        assert!(
            (gpu - cpu).abs() < 0.00001,
            "GPU {gpu} differs from CPU {cpu}"
        );
    }
    let mut stock = filmr::presets::kodak::KODAK_PORTRA_400();
    stock.halation_strength = 0.0;
    stock.halation_threshold = 0.1;
    // Force MTF blur even on this small fixture to catch duplicate GPU blur.
    stock.resolution_lp_mm = 0.2;
    let mut config = filmr::processor::SimulationConfig {
        enable_grain: false,
        white_balance_mode: filmr::processor::WhiteBalanceMode::Off,
        ..Default::default()
    };
    let cpu = super::process_film_image(&input, &stock, &config);
    config.use_gpu = true;
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let rendered = super::process_film_image_inner(&input, &stock, &config, Some(&cancelled))
        .expect("GPU film processing completed");
    assert_eq!(rendered.dimensions(), input.dimensions());
    let max_error = rendered
        .as_raw()
        .iter()
        .zip(cpu.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(max_error <= 1, "GPU/CPU maximum channel error: {max_error}");
    stock.halation_strength = 1.0;
    let halated = super::process_film_image_inner(&input, &stock, &config, Some(&cancelled))
        .expect("GPU halation completed");
    assert_ne!(halated, rendered, "halation must affect the output");
    config.light_leak = filmr::light_leak::LightLeakConfig {
        enabled: true,
        leaks: vec![filmr::light_leak::LightLeak::default()],
    };
    let effects = super::process_film_image_inner(&input, &stock, &config, Some(&cancelled))
        .expect("GPU halation and light leaks completed");
    assert_eq!(effects.dimensions(), input.dimensions());
    assert_ne!(effects, halated, "GPU light leaks must affect the output");
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    assert!(super::process_film_image_inner(&input, &stock, &config, Some(&cancelled)).is_none());
}

#[test]
#[ignore = "requires a working GPU compute adapter"]
fn filmr_accurate_gpu_matches_all_stock_outputs() {
    assert!(filmr::gpu::get_gpu_context().is_some());
    let input = image::RgbImage::from_fn(129, 97, |x, y| {
        image::Rgb([
            (x * 17 % 256) as u8,
            (y * 23 % 256) as u8,
            ((x + y) * 31 % 256) as u8,
        ])
    });
    for entry in super::all_film_stocks() {
        let mut stock = (entry.build)();
        stock.halation_strength = 0.0;
        stock.vignette_strength = 0.3;
        let mut config = filmr::processor::SimulationConfig {
            enable_grain: false,
            white_balance_mode: filmr::processor::WhiteBalanceMode::Off,
            warmth: 0.17,
            saturation: 0.87,
            exposure_time: 1.3,
            ..Default::default()
        };
        let cpu = super::process_film_image(&input, &stock, &config);
        config.use_gpu = true;
        let gpu = super::process_film_image(&input, &stock, &config);
        let max_error = cpu
            .as_raw()
            .iter()
            .zip(gpu.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(max_error <= 1, "{} channel error: {max_error}", entry.name);
    }
}

#[test]
#[ignore = "requires a working GPU compute adapter"]
fn filmr_native_gpu_grain_renders_stock_parameters() {
    let gpu = filmr::gpu::get_gpu_context().expect("GPU adapter available");
    let pipeline = filmr::gpu_pipelines::get_grain_pipeline(gpu);
    let input = image::ImageBuffer::from_pixel(33, 19, image::Rgb([0.5f32; 3]));
    let mut stock = filmr::presets::kodak::KODAK_PORTRA_400();
    stock.grain_model.alpha = 0.0;
    stock.grain_model.sigma_read = 0.0;
    stock.grain_model.shadow_noise = 0.0;
    assert_eq!(pipeline.process_image(gpu, &input, &stock).unwrap(), input);

    // Shadow-only grain exercises the corrected uniforms and bypass condition.
    stock.grain_model.shadow_noise = 0.1;
    stock.grain_model.monochrome = true;
    let mono = pipeline.process_image(gpu, &input, &stock).unwrap();
    assert_ne!(mono, input);
    for pixel in mono.pixels() {
        assert!(pixel.0.iter().all(|v| v.is_finite() && *v >= 0.0));
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
    }
    assert_eq!(pipeline.process_image(gpu, &input, &stock).unwrap(), mono);

    stock.grain_model.monochrome = false;
    stock.grain_model.color_correlation = 0.0;
    let color = pipeline.process_image(gpu, &input, &stock).unwrap();
    assert!(color.pixels().any(|p| p[0] != p[1] || p[1] != p[2]));
    stock.grain_model.color_correlation = 1.0;
    let shared = pipeline.process_image(gpu, &input, &stock).unwrap();
    for (a, b) in shared.as_raw().iter().zip(mono.as_raw()) {
        assert!((a - b).abs() < 0.00001);
    }
    stock.grain_model.highlight_coarseness = 2.0;
    assert_ne!(pipeline.process_image(gpu, &input, &stock).unwrap(), mono);

    let input = image::RgbImage::from_fn(129, 97, |x, y| {
        image::Rgb([(x * 17) as u8, (y * 23) as u8, ((x + y) * 31) as u8])
    });
    let mut config = filmr::processor::SimulationConfig {
        use_gpu: true,
        enable_grain: false,
        ..Default::default()
    };
    let clean = super::process_film_image(&input, &stock, &config);
    config.enable_grain = true;
    let grained = super::process_film_image(&input, &stock, &config);
    assert_eq!(grained.dimensions(), input.dimensions());
    assert_ne!(grained, clean);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    assert!(super::process_film_image_inner(&input, &stock, &config, Some(&cancel)).is_none());
}

#[test]
#[ignore = "GPU performance comparison; run in release mode with --nocapture"]
fn filmr_gpu_performance_comparison() {
    assert!(filmr::gpu::get_gpu_context().is_some());
    let mut stock = filmr::presets::kodak::KODAK_PORTRA_400();
    stock.halation_strength = 0.0;
    let mut config = filmr::processor::SimulationConfig {
        white_balance_mode: filmr::processor::WhiteBalanceMode::Off,
        use_gpu: true,
        ..Default::default()
    };
    let _ = super::process_film_image(&image::RgbImage::new(128, 128), &stock, &config);
    for (width, height) in [(1024, 768), (3840, 2160)] {
        let input = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        for grain in [false, true] {
            config.enable_grain = grain;
            for gpu in [false, true] {
                config.use_gpu = gpu;
                let mut durations = Vec::new();
                for _ in 0..3 {
                    let start = std::time::Instant::now();
                    let output = super::process_film_image(&input, &stock, &config);
                    assert_eq!(output.dimensions(), input.dimensions());
                    durations.push(start.elapsed());
                }
                durations.sort();
                println!(
                    "{width}x{height} grain={grain} GPU={gpu} median: {:?}",
                    durations[1]
                );
            }
        }
    }
}

#[test]
#[ignore = "requires GPU adapters disabled (WGPU_BACKEND=vulkan VK_DRIVER_FILES=/dev/null)"]
fn filmr_gpu_unavailable_falls_back_to_cpu() {
    assert!(filmr::gpu::get_gpu_context().is_none());
    let input = image::RgbImage::from_pixel(17, 9, image::Rgb([180, 100, 60]));
    let stock = filmr::presets::kodak::KODAK_PORTRA_400();
    let mut config = filmr::processor::SimulationConfig {
        enable_grain: false,
        ..Default::default()
    };
    let cpu = super::process_film_image(&input, &stock, &config);
    config.use_gpu = true;
    let fallback = super::process_film_image(&input, &stock, &config);
    assert_eq!(fallback, cpu);
    config.enable_grain = true;
    let grained = super::process_film_image(&input, &stock, &config);
    assert_eq!(grained.dimensions(), input.dimensions());
    assert_ne!(grained, fallback, "CPU fallback must still apply grain");
}

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
        filmr::processor::process_image_cancellable(&image, &stock, &config, &cancelled).is_none()
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
    tx.send(Ok(Some(message))).unwrap();
    state.film_sim_state.render_rx = Some(rx);
    state.film_sim_state.is_rendering = true;
    state.film_sim_state.render_stamp = Some(state.current_film_sim_render_stamp());
}

#[test]
fn film_render_tokens_cancel_on_edits_exit_and_drop_then_reentry_renders() {
    let jobs = little_clip_core::BackgroundJobs::new(1, 4);
    let mut state = state();
    state.active_mode = ImageToolMode::FilmSim;
    state.run_film_sim_render_on(&jobs);
    let parameter_job = state.film_sim_state.render_cancel.as_ref().unwrap().clone();
    state.film_sim_state.mark_dirty();
    assert!(parameter_job.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.film_sim_state.is_rendering);
    assert!(state.film_sim_state.render_rx.is_none());

    state.run_film_sim_render_on(&jobs);
    let pixel_job = state.film_sim_state.render_cancel.as_ref().unwrap().clone();
    state.mark_pixels_changed();
    assert!(pixel_job.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.film_sim_state.is_rendering);

    state.run_film_sim_render_on(&jobs);
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
fn full_resolution_render_queue_retries_after_saturation_and_cancels_queued_work() {
    use std::sync::mpsc;
    use std::time::Duration;

    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::from_secs(5), || ());

    let mut state = state();
    state.active_mode = ImageToolMode::FilmSim;
    state.run_film_sim_render_on(&jobs);
    state.poll_film_sim();
    assert!(!state.film_sim_state.is_rendering);
    assert!(state.film_sim_state.render_retry_at.is_some());
    assert!(state.film_sim_state.render_error.is_some());
    assert!(state.film_sim_state.preview_rgba.is_none());

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    state.run_film_sim_render_on(&jobs);
    assert!(state.film_sim_state.is_rendering);
    let ticket = state
        .film_sim_state
        .render_job_ticket
        .as_ref()
        .unwrap()
        .clone();
    assert!(ticket.revision() > 0);
    state.film_sim_state.cancel_render();
    assert!(!state.film_sim_state.is_rendering);
    assert!(state.film_sim_state.render_rx.is_none());
    ticket.cancel();
}

#[test]
fn queued_full_resolution_render_is_cancelled_before_worker_start() {
    use std::sync::mpsc;
    use std::time::Duration;

    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut state = state();
    state.active_mode = ImageToolMode::FilmSim;
    state.run_film_sim_render_on(&jobs);
    let ticket = state
        .film_sim_state
        .render_job_ticket
        .as_ref()
        .unwrap()
        .clone();
    assert!(ticket.revision() > 0);
    state.film_sim_state.cancel_render();
    assert!(!state.film_sim_state.is_rendering);
    assert!(state.film_sim_state.render_rx.is_none());

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
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
fn full_resolution_render_matches_direct_film_processing_and_adjustments() {
    use std::time::{Duration, Instant};

    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    let mut state = state();
    state.active_mode = ImageToolMode::FilmSim;
    state.film_sim_state.selected_stock_idx = 0;
    state.film_sim_state.brightness = 1.13;
    state.film_sim_state.contrast = 0.31;
    state.film_sim_state.warmth = -0.17;
    state.film_sim_state.tint = 0.22;
    state.film_sim_state.intensity = 0.87;
    state.film_sim_state.wb_mode = filmr::processor::WhiteBalanceMode::Off;
    state.film_sim_state.wb_strength = 0.0;
    state.film_sim_state.halation = 0.18;
    state.film_sim_state.vignette = 0.12;
    state.film_sim_state.enable_grain = false;

    let stocks = super::all_film_stocks();
    let entry = &stocks[state.film_sim_state.selected_stock_idx];
    let mut stock = (entry.build)().with_style(state.film_sim_state.style);
    stock.halation_strength = state.film_sim_state.halation;
    stock.vignette_strength = state.film_sim_state.vignette;
    let config = filmr::processor::SimulationConfig {
        use_gpu: true,
        exposure_time: state.film_sim_state.brightness,
        warmth: state.film_sim_state.warmth,
        saturation: state.film_sim_state.intensity,
        auto_levels: false,
        white_balance_mode: state.film_sim_state.wb_mode,
        white_balance_strength: state.film_sim_state.wb_strength,
        output_mode: filmr::processor::OutputMode::Positive,
        enable_grain: false,
        light_leak: state.film_sim_state.light_leak.to_config(),
        ..filmr::processor::SimulationConfig::default()
    };
    let source_rgb = image::RgbImage::from_fn(
        state.current_rgba.width(),
        state.current_rgba.height(),
        |x, y| {
            let pixel = state.current_rgba.get_pixel(x, y);
            image::Rgb([pixel[0], pixel[1], pixel[2]])
        },
    );
    let expected = super::apply_film_sim_post_adjustments(
        &super::process_film_image(&source_rgb, &stock, &config),
        state.film_sim_state.contrast,
        state.film_sim_state.tint,
    );

    state.run_film_sim_render_on(&jobs);
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.film_sim_state.is_rendering && Instant::now() < deadline {
        state.poll_film_sim();
        if state.film_sim_state.is_rendering {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    state.poll_film_sim();

    let Some(actual) = state.film_sim_state.preview_rgba.as_ref() else {
        state.film_sim_state.cancel_render();
        panic!("full-resolution Film Look render did not produce an accepted preview");
    };
    assert!(state.film_sim_preview_is_current());
    assert_eq!(actual, &expected);
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
