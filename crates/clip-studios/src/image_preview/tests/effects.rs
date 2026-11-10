use super::*;

#[test]
fn test_film_sim_stocks_and_styles() {
    let stocks = all_film_stocks();
    assert_eq!(stocks.len(), 57, "Expected 57 presets across all brands");

    // Verify key presets exist
    assert!(
        stocks
            .iter()
            .any(|s| s.name == "Portra 400" && s.brand == "Kodak")
    );
    assert!(
        stocks
            .iter()
            .any(|s| s.name == "Velvia 50" && s.brand == "Fujifilm")
    );
    assert!(
        stocks
            .iter()
            .any(|s| s.name == "HP5 Plus 400" && s.brand == "Ilford")
    );
    assert!(
        stocks
            .iter()
            .any(|s| s.name == "600 Color" && s.brand == "Polaroid")
    );
    assert!(
        stocks
            .iter()
            .any(|s| s.name == "800T" && s.brand == "CineStill")
    );
    assert!(
        stocks
            .iter()
            .any(|s| s.name == "Vista 200" && s.brand == "Agfa")
    );

    // Verify each stock builds valid FilmStock
    for entry in &stocks {
        let stock = (entry.build)();
        assert!(stock.resolution_lp_mm > 0.0);
    }

    // Test style enum variants
    let styles = [
        FilmStyle::Accurate,
        FilmStyle::Artistic,
        FilmStyle::Vintage,
        FilmStyle::HighContrast,
        FilmStyle::Pastel,
    ];
    assert_eq!(styles.len(), 5);
}

#[test]
fn test_film_sim_state_and_render_dispatch() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(16, 16);
    for p in img.pixels_mut() {
        *p = image::Rgba([120, 140, 160, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 16, 16, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Defaults
    assert_eq!(state.film_sim_state.brightness, 1.0);
    assert_eq!(state.film_sim_state.contrast, 0.0);
    assert_eq!(state.film_sim_state.warmth, 0.0);
    assert_eq!(state.film_sim_state.intensity, 1.0);
    assert!(state.film_sim_state.enable_grain);
    assert_eq!(state.film_sim_state.style, FilmStyle::Accurate);

    // Switch to FilmSim mode
    state.set_tool_mode(ImageToolMode::FilmSim);
    assert_eq!(state.active_mode, ImageToolMode::FilmSim);

    // Trigger render
    state.run_film_sim_render();
    assert!(state.film_sim_state.is_rendering);
    assert!(state.film_sim_state.render_rx.is_some());

    // Poll until background render finishes (should be sub-second for 16x16)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while state.film_sim_state.is_rendering && std::time::Instant::now() < deadline {
        state.poll_film_sim();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(!state.film_sim_state.is_rendering);
    assert!(state.film_sim_state.preview_rgba.is_some());
    assert_eq!(state.film_sim_state.render_error, None);

    // Apply film sim
    let pre_undo_len = state.undo_stack.len();
    state.apply_film_sim();
    assert_eq!(state.undo_stack.len(), pre_undo_len + 1);
    assert!(state.dirty_texture);
    assert!(state.status_toast.is_some());
}

#[test]
fn test_film_sim_contrast_curve() {
    let compute_lut = |contrast: f32| -> [u8; 256] {
        let k = if contrast >= 0.0 {
            1.0 + contrast * 2.2
        } else {
            1.0 / (1.0 + contrast.abs() * 2.2)
        };
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let t = i as f32 / 255.0;
            let curved = if t < 0.5 {
                0.5 * (2.0 * t).powf(k)
            } else {
                1.0 - 0.5 * (2.0 * (1.0 - t)).powf(k)
            };
            *slot = (curved.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        lut
    };

    // Neutral
    let lut_0 = compute_lut(0.0);
    for i in 0..=255 {
        assert_eq!(lut_0[i], i as u8);
    }

    // High contrast (e.g. +0.5): shadows darker, highlights brighter
    let lut_pos = compute_lut(0.5);
    assert!(
        lut_pos[64] < 64,
        "Shadows should be darkened for high contrast"
    );
    assert!(
        lut_pos[192] > 192,
        "Highlights should be brightened for high contrast"
    );
    assert!(
        (lut_pos[128] as i32 - 128).abs() <= 1,
        "Midtones should remain anchored within 1 level"
    );

    // Low contrast (e.g. -0.5): shadows lifted, highlights softened
    let lut_neg = compute_lut(-0.5);
    assert!(
        lut_neg[64] > 64,
        "Shadows should be lifted for low contrast"
    );
    assert!(
        lut_neg[192] < 192,
        "Highlights should be softened for low contrast"
    );
    assert!(
        (lut_neg[128] as i32 - 128).abs() <= 1,
        "Midtones should remain anchored within 1 level"
    );
}

#[test]
fn test_film_sim_tint_and_post_adjustments() {
    let mut img = image::RgbImage::new(4, 4);
    for p in img.pixels_mut() {
        *p = image::Rgb([128, 128, 128]);
    }

    // Neutral tint and neutral contrast: should be exact identity
    let neutral = apply_film_sim_post_adjustments(&img, 0.0, 0.0);
    assert_eq!(neutral.get_pixel(0, 0), &image::Rgba([128, 128, 128, 255]));

    // Magenta tint (+0.5): Red and Blue boosted, Green attenuated
    let magenta = apply_film_sim_post_adjustments(&img, 0.0, 0.5);
    let p_mag = magenta.get_pixel(0, 0);
    assert!(p_mag[0] > 128, "Red should be boosted for magenta tint");
    assert!(p_mag[2] > 128, "Blue should be boosted for magenta tint");
    assert!(p_mag[1] < 128, "Green should be reduced for magenta tint");

    // Green tint (-0.5): Green boosted, Red and Blue attenuated
    let green = apply_film_sim_post_adjustments(&img, 0.0, -0.5);
    let p_grn = green.get_pixel(0, 0);
    assert!(p_grn[1] > 128, "Green should be boosted for green tint");
    assert!(p_grn[0] < 128, "Red should be reduced for green tint");
    assert!(p_grn[2] < 128, "Blue should be reduced for green tint");
}

#[test]
fn test_film_sim_light_leak_presets() {
    let presets = [
        LightLeakPreset::None,
        LightLeakPreset::WarmCorner,
        LightLeakPreset::SunStreak,
        LightLeakPreset::SoftEdge,
        LightLeakPreset::OrganicBlob,
    ];
    for p in presets {
        let config = p.to_config();
        if p == LightLeakPreset::None {
            assert!(!config.enabled);
            assert!(config.leaks.is_empty());
            assert_eq!(p.label(), "None");
        } else {
            assert!(config.enabled);
            assert_eq!(config.leaks.len(), 1);
            assert!(!p.label().is_empty());
        }
    }
}

#[test]
fn test_film_sim_thumbnail_batch_streaming() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(42, 28);
    for p in img.pixels_mut() {
        *p = image::Rgba([100, 120, 140, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Defaults for expanded controls
    assert_eq!(state.film_sim_state.tint, 0.0);
    assert_eq!(state.film_sim_state.wb_mode, WhiteBalanceMode::Auto);
    assert_eq!(state.film_sim_state.wb_strength, 0.75);
    assert_eq!(state.film_sim_state.halation, 0.25);
    assert_eq!(state.film_sim_state.vignette, 0.35);
    assert_eq!(state.film_sim_state.light_leak, LightLeakPreset::None);

    // Run thumbnail batch
    state.run_thumbnail_batch();
    assert!(state.film_sim_state.is_generating_thumbnails);
    assert!(state.film_sim_state.thumbnail_rx.is_some());

    // Wait for at least some thumbnails to stream in
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while state.film_sim_state.pending_thumbnails.is_empty() && std::time::Instant::now() < deadline
    {
        state.poll_film_sim();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    assert!(
        !state.film_sim_state.pending_thumbnails.is_empty(),
        "Pending thumbnails should receive streamed items from worker"
    );
    let first_thumb = &state.film_sim_state.pending_thumbnails[0];
    assert_eq!(first_thumb.1.width(), 42);
    assert_eq!(first_thumb.1.height(), 28);
}

#[test]
fn test_creative_fx_mode_switching_and_defaults() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(32, 32);
    for p in img.pixels_mut() {
        *p = image::Rgba([180, 100, 50, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 32, 32, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Label check (no shortcut letter)
    assert_eq!(ImageToolMode::CreativeFx.label(), "Creative FX");

    // Defaults
    assert_eq!(
        state.creative_fx_state.selected_fx,
        CreativeFxType::BasicControls
    );
    assert_eq!(state.creative_fx_state.lens_params.strength, 25.0);
    assert_eq!(state.creative_fx_state.lens_params.falloff, 50.0);
    assert_eq!(state.creative_fx_state.lens_params.color_balance, 0.0);
    assert_eq!(
        state.creative_fx_state.trichromatic_params.mode,
        TrichromaticMode::Rgb
    );
    assert_eq!(
        state.creative_fx_state.trichromatic_params.channels.len(),
        3
    );
    assert_eq!(
        state.creative_fx_state.pixel_sort_params.direction,
        PixelSortDirection::Vertical
    );
    assert_eq!(state.creative_fx_state.cmyk_params.dot_size, 5.0);
    assert_eq!(
        state.creative_fx_state.cmyk_params.paper_tint,
        PaperTint::Newsprint
    );
    assert_eq!(
        state.creative_fx_state.dither_params.palette,
        RetroPalette::GameBoy
    );
    assert_eq!(
        state.creative_fx_state.dither_params.method,
        DitherMethod::Bayer4x4
    );
    assert_eq!(state.creative_fx_state.ntsc_params.luma_smear, 0.25);
    assert_eq!(
        state.creative_fx_state.ntsc_params.tape_speed,
        VhsTapeSpeed::SP
    );
    assert_eq!(
        state.creative_fx_state.ntsc_params.demodulation,
        NtscDemodulation::Notch
    );
    assert_eq!(
        state.creative_fx_state.ntsc_params.phase_shift,
        NtscPhaseShift::Degrees180
    );
    assert_eq!(
        state.creative_fx_state.ntsc_params.chroma_lowpass,
        NtscChromaLowpass::Full
    );
    assert_eq!(state.creative_fx_state.structure_params.amount, 35.0);
    assert_eq!(state.creative_fx_state.structure_params.detail_scale, 0.0);
    assert_eq!(
        state.creative_fx_state.structure_params.edge_protection,
        50.0
    );

    // Switch to Creative FX mode
    state.set_tool_mode(ImageToolMode::CreativeFx);
    assert_eq!(state.active_mode, ImageToolMode::CreativeFx);

    // Verify async render triggered
    assert!(state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.render_rx.is_some());

    // Poll until render completes
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.preview_rgba.is_some());
    assert_eq!(state.creative_fx_state.render_error, None);

    // Switch to Structure effect and trigger render
    state.creative_fx_state.selected_fx = CreativeFxType::Structure;
    state.run_creative_fx_render();
    assert!(state.creative_fx_state.is_rendering);
    let deadline_structure = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline_structure {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.preview_rgba.is_some());
    assert_eq!(state.creative_fx_state.render_error, None);

    // Switch to NTSC effect and trigger render
    state.creative_fx_state.selected_fx = CreativeFxType::Ntsc;
    state.run_creative_fx_render();
    assert!(state.creative_fx_state.is_rendering);
    let deadline_ntsc = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline_ntsc {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.preview_rgba.is_some());
    assert_eq!(state.creative_fx_state.render_error, None);

    // Apply Creative FX to image
    let pre_undo = state.undo_stack.len();
    state.apply_creative_fx();
    assert_eq!(state.undo_stack.len(), pre_undo + 1);
    assert!(state.dirty_texture);
    assert!(state.status_toast.is_some());
}

#[test]
fn test_creative_fx_thumbnail_streaming() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(42, 28);
    for (i, p) in img.pixels_mut().enumerate() {
        *p = image::Rgba([(i % 255) as u8, 120, 200, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.run_creative_fx_thumbnail_batch();
    assert!(state.creative_fx_state.is_generating_thumbnails);
    assert!(state.creative_fx_state.thumbnail_rx.is_some());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.pending_thumbnails.is_empty()
        && std::time::Instant::now() < deadline
    {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    assert!(
        !state.creative_fx_state.pending_thumbnails.is_empty(),
        "Pending thumbnails should contain generated items"
    );
    let first_thumb = &state.creative_fx_state.pending_thumbnails[0];
    assert_eq!(first_thumb.1.width(), 42);
    assert_eq!(first_thumb.1.height(), 28);
}

#[test]
fn test_creative_fx_depth_mask_workflow() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(16, 16);
    for p in img.pixels_mut() {
        *p = image::Rgba([80, 80, 80, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 16, 16, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();
    state.active_mode = ImageToolMode::CreativeFx;

    // 1. Verify depth mask is disabled by default across effects
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert_eq!(
        state.creative_fx_state.depth_mask_for_active_fx().threshold,
        0.0
    );
    assert_eq!(
        state.creative_fx_state.depth_mask_for_active_fx().feather,
        0.25
    );
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().invert);
    assert!(state.creative_fx_state.cached_depth_map.is_none());

    // 2. Set synthetic depth map (left half dark / near, right half bright / far)
    let mut depth_map = RgbaImage::new(16, 16);
    for (x, _y, p) in depth_map.enumerate_pixels_mut() {
        let val = if x < 8 { 0u8 } else { 255u8 };
        *p = image::Rgba([val, val, val, 255]);
    }
    state.creative_fx_state.cached_depth_map = Some(depth_map);
    state.creative_fx_state.depth_map_input_stamp = Some(state.input_stamp());

    // 3. Enable depth mask on PixelSort
    state.creative_fx_state.selected_fx = CreativeFxType::PixelSort;
    state
        .creative_fx_state
        .depth_mask_for_active_fx_mut()
        .enabled = true;
    assert!(state.creative_fx_state.depth_mask_for_active_fx().enabled);

    // 4. Render with depth mask
    state.run_creative_fx_render();
    assert!(state.creative_fx_state.is_rendering);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.preview_rgba.is_some());
    assert_eq!(state.creative_fx_state.render_error, None);

    // 5. Test mask inversion and threshold
    state
        .creative_fx_state
        .depth_mask_for_active_fx_mut()
        .invert = true;
    state
        .creative_fx_state
        .depth_mask_for_active_fx_mut()
        .threshold = 0.5;
    state
        .creative_fx_state
        .depth_mask_for_active_fx_mut()
        .feather = 0.1;
    state.run_creative_fx_render();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.preview_rgba.is_some());

    // 6. Switch to Structure effect, verify cached depth map is shared
    state.creative_fx_state.selected_fx = CreativeFxType::Structure;
    assert!(state.creative_fx_state.cached_depth_map.is_some());
    state
        .creative_fx_state
        .depth_mask_for_active_fx_mut()
        .enabled = true;
    state.run_creative_fx_render();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.is_rendering && std::time::Instant::now() < deadline {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.preview_rgba.is_some());
}

#[test]
fn test_creative_fx_alpha_mask_runs_fey_nobg_without_prompt() {
    let config = AppConfig::default();
    let img = RgbaImage::from_pixel(16, 16, image::Rgba([80, 80, 80, 255]));
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 16, 16, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    let mock_script = "import sys, io\nfrom PIL import Image\nassert '--prompt' not in sys.argv, 'Creative FX must use FeyNoBg without a prompt'\nimg = Image.open(io.BytesIO(sys.stdin.buffer.read())).convert('RGBA')\nfor y in range(img.height):\n    for x in range(img.width):\n        img.putpixel((x, y), (*img.getpixel((x, y))[:3], 0 if x < 8 else 255))\nbuf = io.BytesIO()\nimg.save(buf, format='PNG')\nsys.stdout.buffer.write(buf.getvalue())\n";
    let script_path = std::env::temp_dir().join("mock_nobg_fx_worker.py");
    std::fs::write(&script_path, mock_script).unwrap();
    state.bg_removal_cmd = Some(format!("python3 {}", script_path.display()));
    state.multimatte_prompt = "person in red".to_string();

    state.run_alpha_mask_for_fx();
    assert!(state.creative_fx_state.alpha_mask_in_progress);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.creative_fx_state.alpha_mask_in_progress && std::time::Instant::now() < deadline {
        state.poll_creative_fx();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(!state.creative_fx_state.alpha_mask_in_progress);
    assert_eq!(state.creative_fx_state.alpha_mask_error, None);
    let removal = state
        .creative_fx_state
        .cached_background_removal
        .as_ref()
        .expect("FeyNoBg result should be cached");
    assert_eq!(removal.get_pixel(2, 2)[3], 0);
    assert_eq!(removal.get_pixel(12, 2)[3], 255);
    assert_eq!(state.current_rgba.get_pixel(2, 2)[3], 255);

    let mask = state.creative_fx_state.depth_mask_for_active_fx();
    assert!(mask.enabled);
    assert_eq!(mask.source, CreativeFxMaskSource::BackgroundRemovalAlpha);
}
