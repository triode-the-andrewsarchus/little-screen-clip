use super::*;

#[test]
fn test_load_png_and_meta() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(640, 360);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba([59, 130, 246, 255]);
    }
    let temp_file = std::env::temp_dir().join(format!("test_preview_{}.png", uuid::Uuid::new_v4()));
    img.save(&temp_file).unwrap();

    let state =
        ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load PNG");
    assert_eq!(state.current_rgba.width(), 640);
    assert_eq!(state.current_rgba.height(), 360);
    assert!(!state.is_svg);
    assert_eq!(format_aspect_ratio(640, 360), "16:9");
    let disk_path = state.ensure_disk_path().expect("ensure_disk_path failed");
    assert!(disk_path.exists());
    let _ = std::fs::remove_file(temp_file);
}

#[test]
fn test_load_jpeg_and_meta() {
    let config = AppConfig::default();
    let img = image::RgbImage::new(800, 600);
    let temp_file = std::env::temp_dir().join(format!("test_preview_{}.jpg", uuid::Uuid::new_v4()));
    img.save(&temp_file).unwrap();

    let state =
        ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load JPEG");
    assert_eq!(state.current_rgba.width(), 800);
    assert_eq!(state.current_rgba.height(), 600);
    assert!(!state.is_svg);
    assert_eq!(format_aspect_ratio(800, 600), "4:3");
    let _ = std::fs::remove_file(temp_file);
}

#[test]
fn test_load_svg_and_meta() {
    let config = AppConfig::default();
    let svg_content = r##"<svg width="400" height="400" viewBox="0 0 400 400" xmlns="http://www.w3.org/2000/svg"><rect width="400" height="400" fill="#8b5cf6"/></svg>"##;
    let temp_file = std::env::temp_dir().join(format!("test_preview_{}.svg", uuid::Uuid::new_v4()));
    std::fs::write(&temp_file, svg_content).unwrap();

    let state =
        ImagePreviewState::from_path(temp_file.clone(), &config).expect("Failed to load SVG");
    assert_eq!(state.current_rgba.width(), 400);
    assert_eq!(state.current_rgba.height(), 400);
    assert!(state.is_svg);
    assert_eq!(format_aspect_ratio(400, 400), "1:1");
    let _ = std::fs::remove_file(temp_file);
}

#[test]
fn test_load_item_in_memory() {
    let config = AppConfig::default();
    let img = RgbaImage::new(320, 240);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();

    let item = ClipItem::from_image(png_bytes, 320, 240, "image/png");
    let state = ImagePreviewState::from_item(&item, &config).expect("Failed to load from ClipItem");
    assert_eq!(state.current_rgba.width(), 320);
    assert_eq!(state.current_rgba.height(), 240);
    let disk_path = state.ensure_disk_path().expect("ensure_disk_path failed");
    assert!(disk_path.exists());
}

#[test]
fn test_crop_free() {
    let config = AppConfig::default();
    let img = RgbaImage::new(100, 100);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Set normalized crop: x from 0.1 to 0.6, y from 0.2 to 0.7 (50x50)
    state.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.6, 0.7));
    state.apply_crop();

    assert_eq!(state.current_rgba.width(), 50);
    assert_eq!(state.current_rgba.height(), 50);
    assert_eq!(state.undo_stack.len(), 1);

    // Test undo
    state.undo();
    assert_eq!(state.current_rgba.width(), 100);
    assert_eq!(state.current_rgba.height(), 100);
}

#[test]
fn test_crop_aspect_ratio_presets() {
    let mut crop = CropState::default();

    // 1920x1080 (16:9) image with 1:1 ratio
    crop.set_ratio(CropRatio::Ratio1x1, 1920, 1080);
    let norm = crop.normalized_rect;
    let px_w = (norm.width() * 1920.0).round();
    let px_h = (norm.height() * 1080.0).round();
    assert_eq!(px_w, 1080.0);
    assert_eq!(px_h, 1080.0);

    // 1920x1080 with 16:9 ratio
    crop.set_ratio(CropRatio::Ratio16x9, 1920, 1080);
    let norm = crop.normalized_rect;
    let px_w = (norm.width() * 1920.0).round();
    let px_h = (norm.height() * 1080.0).round();
    assert_eq!(px_w, 1920.0);
    assert_eq!(px_h, 1080.0);

    // 1920x1080 with 9:16 ratio
    crop.set_ratio(CropRatio::Ratio9x16, 1920, 1080);
    let norm = crop.normalized_rect;
    let px_w = (norm.width() * 1920.0).round();
    let px_h = (norm.height() * 1080.0).round();
    assert_eq!(px_h, 1080.0);
    assert_eq!((px_w / px_h - 9.0 / 16.0).abs() < 0.01, true);
}

#[test]
fn test_rotation_and_flipping() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(100, 200);
    // Paint top-left pixel red
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));

    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 100, 200, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    assert_eq!(state.current_rgba.width(), 100);
    assert_eq!(state.current_rgba.height(), 200);
    assert_eq!(
        *state.current_rgba.get_pixel(0, 0),
        image::Rgba([255, 0, 0, 255])
    );

    // Rotate 90 CW: dimensions become 200x100, red pixel moves to (199, 0)
    state.rotate_cw();
    assert_eq!(state.current_rgba.width(), 200);
    assert_eq!(state.current_rgba.height(), 100);
    assert_eq!(
        *state.current_rgba.get_pixel(199, 0),
        image::Rgba([255, 0, 0, 255])
    );

    // Rotate 180: dimensions stay 200x100, red pixel moves to (0, 99)
    state.rotate_180();
    assert_eq!(state.current_rgba.width(), 200);
    assert_eq!(state.current_rgba.height(), 100);
    assert_eq!(
        *state.current_rgba.get_pixel(0, 99),
        image::Rgba([255, 0, 0, 255])
    );

    // Flip Horizontal: pixel at (0, 99) moves to (199, 99)
    state.flip_horizontal();
    assert_eq!(
        *state.current_rgba.get_pixel(199, 99),
        image::Rgba([255, 0, 0, 255])
    );

    // Flip Vertical: pixel at (199, 99) moves to (199, 0)
    state.flip_vertical();
    assert_eq!(
        *state.current_rgba.get_pixel(199, 0),
        image::Rgba([255, 0, 0, 255])
    );

    // Test Revert
    state.revert_to_original();
    assert_eq!(state.current_rgba.width(), 100);
    assert_eq!(state.current_rgba.height(), 200);
    assert_eq!(
        *state.current_rgba.get_pixel(0, 0),
        image::Rgba([255, 0, 0, 255])
    );
}

#[test]
fn test_current_display_image_crop_selection() {
    let config = AppConfig::default();
    let img = RgbaImage::new(200, 200);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 200, 200, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // In View mode, display image is full 200x200
    assert_eq!(state.current_display_image().width(), 200);
    assert_eq!(state.current_display_image().height(), 200);

    // Switch to Crop mode and set crop rect to 50%
    state.active_mode = ImageToolMode::Crop;
    state.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 0.5));

    let cropped_disp = state.current_display_image();
    assert_eq!(cropped_disp.width(), 100);
    assert_eq!(cropped_disp.height(), 100);
    // Canvas image remains untouched until applied
    assert_eq!(state.current_rgba.width(), 200);
}

#[test]
fn test_save_image_to_path_png_and_jpeg() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(120, 80);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba([120, 180, 240, 200]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 120, 80, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    let tmp_png = std::env::temp_dir().join(format!("test_save_{}.png", uuid::Uuid::new_v4()));
    let tmp_jpg = std::env::temp_dir().join(format!("test_save_{}.jpg", uuid::Uuid::new_v4()));

    // Save as PNG
    state.save_image_to_path(&tmp_png);
    save::wait_for_save(&mut state);
    assert!(tmp_png.exists());
    let reloaded_png = image::open(&tmp_png).expect("Reload PNG failed");
    assert_eq!(reloaded_png.width(), 120);
    assert_eq!(reloaded_png.height(), 80);
    assert_eq!(state.mime, "image/png");

    // Save as JPEG (exercises RGBA -> RGB flattening)
    state.save_image_to_path(&tmp_jpg);
    save::wait_for_save(&mut state);
    assert!(tmp_jpg.exists());
    let reloaded_jpg = image::open(&tmp_jpg).expect("Reload JPG failed");
    assert_eq!(reloaded_jpg.width(), 120);
    assert_eq!(reloaded_jpg.height(), 80);
    assert_eq!(state.mime, "image/jpeg");

    let _ = std::fs::remove_file(tmp_png);
    let _ = std::fs::remove_file(tmp_jpg);
}

#[test]
fn test_redaction_placement_and_burn_in() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(100, 100);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba([255, 255, 255, 255]); // white canvas
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Place redaction box covering x: [0.2, 0.6], y: [0.3, 0.7] (pixels 20..60, 30..70)
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.3), Pos2::new(0.6, 0.7)),
        style: RedactionStyle::Black,
    });

    // Test that current_display_image burns pending redactions non-destructively
    let disp_img = state.current_display_image();
    assert_eq!(*disp_img.get_pixel(40, 50), image::Rgba([0, 0, 0, 255]));
    assert_eq!(
        *disp_img.get_pixel(10, 10),
        image::Rgba([255, 255, 255, 255])
    );
    assert_eq!(
        *state.current_rgba.get_pixel(40, 50),
        image::Rgba([255, 255, 255, 255])
    ); // canvas untouched yet

    // Burn / apply redactions permanently
    state.apply_redactions();
    assert_eq!(
        *state.current_rgba.get_pixel(40, 50),
        image::Rgba([0, 0, 0, 255])
    ); // burned
    assert_eq!(
        *state.current_rgba.get_pixel(10, 10),
        image::Rgba([255, 255, 255, 255])
    ); // untouched
    assert_eq!(state.redaction_state.boxes.len(), 0);
    assert_eq!(state.undo_stack.len(), 1);

    // Undo restoration
    state.undo();
    assert_eq!(
        *state.current_rgba.get_pixel(40, 50),
        image::Rgba([255, 255, 255, 255])
    );
}

#[test]
fn test_redaction_delete_and_clear() {
    let config = AppConfig::default();
    let img = RgbaImage::new(100, 100);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.3, 0.3)),
        style: RedactionStyle::Black,
    });
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::new(0.5, 0.5), Pos2::new(0.8, 0.8)),
        style: RedactionStyle::Black,
    });
    assert_eq!(state.redaction_state.boxes.len(), 2);

    // Select and delete box 0
    state.redaction_state.selected_idx = Some(0);
    state.delete_selected_redaction();
    assert_eq!(state.redaction_state.boxes.len(), 1);
    assert!(state.redaction_state.selected_idx.is_none());

    // Clear all
    state.clear_all_redactions();
    assert_eq!(state.redaction_state.boxes.len(), 0);
}

#[test]
fn test_color_picker_hex_and_recent() {
    let mut pick_state = PickColorState::default();
    let c1 = Color32::from_rgb(168, 85, 247);
    let c2 = Color32::from_rgb(56, 189, 248);

    let hex1 = format!("#{:02X}{:02X}{:02X}", c1.r(), c1.g(), c1.b());
    assert_eq!(hex1, "#A855F7");

    let hex2 = format!("#{:02X}{:02X}{:02X}", c2.r(), c2.g(), c2.b());
    assert_eq!(hex2, "#38BDF8");

    pick_state.recent_colors.push(c1);
    pick_state.recent_colors.push(c2);
    assert_eq!(pick_state.recent_colors.len(), 2);
}

#[test]
fn pick_color_hover_renders_loupe_and_click_records_sampled_pixel() {
    let expected = Color32::from_rgb(230, 83, 122);
    let palette = AppPalette::for_appearance(little_clip_core::config::WorkbenchAppearance::Light);
    for viewport_size in [Vec2::new(400.0, 300.0), Vec2::new(1200.0, 800.0)] {
        let mut state = image_state_from_png((8, 8), "color picker interaction");
        state.current_rgba = Arc::new(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([expected.r(), expected.g(), expected.b(), 255]),
        ));
        let ctx = egui::Context::default();
        let render = |state: &mut ImagePreviewState, time, events| {
            let mut canvas_bounds = None;
            let output = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, viewport_size)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (response, painter) =
                            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
                        let bounds = response.rect;
                        let sample_area = bounds.shrink(12.0);
                        state.render_pick_color_overlay(
                            ui,
                            &painter,
                            &response,
                            sample_area,
                            bounds,
                            &palette,
                        );
                        canvas_bounds = Some(bounds);
                    });
                },
            );
            (output, canvas_bounds.expect("canvas bounds were allocated"))
        };

        let (_, bounds) = render(&mut state, 0.0, Vec::new());
        let sample_area = bounds.shrink(12.0);
        let pointer = sample_area.center();
        let (hover, _) = render(&mut state, 0.1, vec![egui::Event::PointerMoved(pointer)]);
        assert_eq!(state.pick_color_state.hovered_color, Some(expected));
        assert_eq!(state.pick_color_state.hovered_pixel, Some((4, 4)));
        let readout_rects: Vec<_> = hover
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
        for expected_line in ["#E6537A", "R230 G83 B122", "X: 4  Y: 4", "Click to keep"] {
            let readout = readout_rects
                .iter()
                .find(|(text, _)| text == expected_line)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("loupe renders {expected_line}"));
            assert!(
                bounds.contains_rect(readout),
                "readout {expected_line} at {readout:?} must stay within {bounds:?} for {viewport_size:?}"
            );
        }
        let card_rect = hover.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if (rect.stroke.width - 1.5).abs() < f32::EPSILON && rect.rect.width() >= 92.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        });
        let card_rect = card_rect.expect("loupe card border is rendered");
        assert!(
            bounds.contains_rect(card_rect),
            "loupe card {card_rect:?} must stay within {bounds:?} for {viewport_size:?}"
        );
        assert!(
            readout_rects
                .iter()
                .all(|(_, rect)| card_rect.contains_rect(*rect))
        );

        render(
            &mut state,
            0.2,
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        render(
            &mut state,
            0.3,
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        assert_eq!(state.pick_color_state.picked_colors, vec![expected]);
        assert_eq!(state.pick_color_state.last_picked, Some(expected));
    }
}

#[test]
fn test_pixel_resize_aspect_locked_and_unlocked() {
    let config = AppConfig::default();
    let img = RgbaImage::new(200, 100);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 200, 100, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    assert_eq!(state.current_rgba.width(), 200);
    assert_eq!(state.current_rgba.height(), 100);

    // Aspect ratio is 2.0 (2:1). With lock enabled:
    state.resize_state.lock_aspect = true;
    state.resize_state.aspect_ratio_base = 2.0;

    // Setting width to 100 should calculate height to 50
    state.resize_state.set_width(100);
    assert_eq!(state.resize_state.target_width, 100);
    assert_eq!(state.resize_state.target_height, 50);

    // Setting height to 150 should calculate width to 300
    state.resize_state.set_height(150);
    assert_eq!(state.resize_state.target_width, 300);
    assert_eq!(state.resize_state.target_height, 150);

    // Unlock aspect ratio
    state.resize_state.lock_aspect = false;
    state.resize_state.set_width(75);
    assert_eq!(state.resize_state.target_width, 75);
    assert_eq!(state.resize_state.target_height, 150); // height unchanged

    // Apply pixel resize to 75x150
    state.apply_pixel_resize(75, 150, ResizeFilter::Lanczos3);
    wait_for_pixel_resize(&mut state);
    assert_eq!(state.current_rgba.width(), 75);
    assert_eq!(state.current_rgba.height(), 150);
    assert_eq!(state.undo_stack.len(), 1);

    // Test Undo
    state.undo();
    assert_eq!(state.current_rgba.width(), 200);
    assert_eq!(state.current_rgba.height(), 100);
}

#[test]
fn test_target_file_size_optimizer_jpeg() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(400, 300);
    for (i, pixel) in img.pixels_mut().enumerate() {
        let r = (i % 255) as u8;
        let g = ((i * 3) % 255) as u8;
        let b = ((i * 7) % 255) as u8;
        *pixel = image::Rgba([r, g, b, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 400, 300, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Target: 20 KB (20,480 bytes)
    let target_bytes = 20 * 1024;
    state.resize_state.target_size_val = 20.0;
    state.resize_state.target_size_unit_mb = false;
    state.resize_state.target_format = TargetFormat::Jpeg;

    state.run_target_file_size_optimizer();
    // Poll for async optimizer completion
    for _ in 0..500 {
        state.poll_optimizer();
        if !state.resize_state.is_optimizing && state.resize_state.last_result.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let opt_res = state
        .resize_state
        .last_result
        .clone()
        .expect("Optimization result missing");

    assert!(
        opt_res.achieved_bytes <= target_bytes,
        "Achieved {} > target {}",
        opt_res.achieved_bytes,
        target_bytes
    );
    assert!(opt_res.width <= 400);
    assert!(opt_res.height <= 300);
    assert!(!opt_res.encoded_data.is_empty());

    // Apply optimization
    state.apply_optimized_result();
    assert_eq!(state.current_rgba.width(), opt_res.width);
    assert_eq!(state.current_rgba.height(), opt_res.height);
    assert_eq!(state.undo_stack.len(), 1);

    // Undo
    state.undo();
    assert_eq!(state.current_rgba.width(), 400);
    assert_eq!(state.current_rgba.height(), 300);
}

#[test]
fn test_target_file_size_optimizer_progress_callback() {
    let mut img = RgbaImage::new(200, 150);
    for (i, pixel) in img.pixels_mut().enumerate() {
        let r = (i % 255) as u8;
        let g = ((i * 5) % 255) as u8;
        let b = ((i * 11) % 255) as u8;
        *pixel = image::Rgba([r, g, b, 255]);
    }
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let cb = move |msg: String| {
        captured_clone.lock().unwrap().push(msg);
    };

    let target_bytes = 10 * 1024;
    let res =
        optimize_to_target_size_with_progress(&img, target_bytes, TargetFormat::Jpeg, Some(&cb));
    assert!(res.achieved_bytes <= target_bytes + 2048);
    let list = captured.lock().unwrap();
    assert!(
        !list.is_empty(),
        "Progress messages should have been captured"
    );
    assert!(
        list.iter().any(|m| m.contains("Testing full resolution")
            || m.contains("Trying quality")
            || m.contains("Complete")),
        "Expected recognizable progress messages: {:?}",
        list
    );
}

#[test]
fn test_target_file_size_optimizer_png() {
    let mut img = RgbaImage::new(300, 300);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba([100, 150, 200, 255]);
    }

    // Target: 10 KB
    let target_bytes = 10 * 1024;
    let res = optimize_to_target_size(&img, target_bytes, TargetFormat::Png);

    assert!(
        res.achieved_bytes <= target_bytes,
        "Achieved {} > target {}",
        res.achieved_bytes,
        target_bytes
    );
    assert!(res.width <= 300);
    assert!(res.height <= 300);
}

#[test]
fn test_palette_extraction_colors_and_percentages() {
    // Create 100x100 image: left half pure Red, right half pure Blue
    let mut img = RgbaImage::new(100, 100);
    for x in 0..100 {
        for y in 0..100 {
            if x < 50 {
                img.put_pixel(x, y, image::Rgba([255, 0, 0, 255])); // Red
            } else {
                img.put_pixel(x, y, image::Rgba([0, 0, 255, 255])); // Blue
            }
        }
    }

    let (palette, total_sampled) = extract_palette(&img, 8);
    assert!(total_sampled > 0);
    assert!(palette.len() >= 2);

    let hex_codes: Vec<String> = palette.iter().map(|p| p.hex.clone()).collect();
    assert!(
        hex_codes.iter().any(|h| h == "#FF0000"),
        "Red should be in palette: {:?}",
        hex_codes
    );
    assert!(
        hex_codes.iter().any(|h| h == "#0000FF"),
        "Blue should be in palette: {:?}",
        hex_codes
    );

    // Each half should have approximately 50% representation
    let red_pal = palette.iter().find(|p| p.hex == "#FF0000").unwrap();
    let blue_pal = palette.iter().find(|p| p.hex == "#0000FF").unwrap();
    assert!(
        (red_pal.percentage - 50.0).abs() < 5.0,
        "Red % was {}",
        red_pal.percentage
    );
    assert!(
        (blue_pal.percentage - 50.0).abs() < 5.0,
        "Blue % was {}",
        blue_pal.percentage
    );
}

#[test]
fn test_copy_to_hand_slot_dispatch() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(50, 50);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba([10, 20, 30, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 50, 50, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.copy_to_hand_slot(HandSide::Left);
    assert!(state.status_toast.is_some());
    state.copy_to_hand_slot(HandSide::Right);
    assert!(state.status_toast.is_some());
}

#[test]
fn decorrelation_stretch_apply_undo_and_redo_restore_image_pixels() {
    let config = AppConfig::default();
    let image = RgbaImage::from_fn(16, 16, |x, y| {
        image::Rgba([
            (x * 11 + y * 3) as u8,
            (y * 13 + x * 5) as u8,
            ((x + y) * 7) as u8,
            255,
        ])
    });
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut state = ImagePreviewState::from_bytes_with_meta(
        png,
        "image/png".into(),
        None,
        "DCS undo redo test".into(),
        &config,
    )
    .unwrap();
    let original = state.current_rgba.clone();
    assert!(
        (0..3).all(|channel| {
            let min = image.pixels().map(|pixel| pixel[channel]).min().unwrap();
            let max = image.pixels().map(|pixel| pixel[channel]).max().unwrap();
            min < max
        }),
        "fixture must have nonzero variance in every RGB channel"
    );

    state.update_decorrelation_stretch();
    wait_for_dcs(&mut state);
    let stretched = state
        .dcs_state
        .stretched_rgba
        .clone()
        .expect("analysis creates a stretched result");
    assert_ne!(
        stretched, *original,
        "stretch result changes the fixture pixels"
    );

    state.apply_decorrelation_stretch();
    let applied = state.current_rgba.clone();
    assert_eq!(*applied, stretched);
    assert_ne!(applied, original);

    state.undo();
    assert_eq!(
        state.current_rgba, original,
        "undo restores pre-stretch pixels"
    );

    state.redo();
    assert_eq!(
        state.current_rgba, applied,
        "redo restores stretched pixels"
    );
}

#[test]
fn test_metadata_inspection_and_stripping() {
    let config = AppConfig::default();
    let img = RgbaImage::new(30, 30);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 30, 30, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Initially no metadata in plain generated image
    assert!(state.metadata.is_none());

    // Test stripping metadata updates raw_bytes, pushes to undo_stack, and clears metadata
    state.strip_metadata();
    assert!(state.metadata.is_none());
    assert_eq!(state.undo_stack.len(), 1);
    assert!(state.status_toast.is_some());
    let toast_msg = &state.status_toast.as_ref().unwrap().0;
    assert!(toast_msg.contains("Stripped all EXIF"));
}

#[test]
fn test_perspective_deskew_apply() {
    let config = AppConfig::default();
    let img = RgbaImage::new(40, 40);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 40, 40, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.set_tool_mode(ImageToolMode::Deskew);
    assert_eq!(state.active_mode, ImageToolMode::Deskew);

    // Apply deskew on default quad
    state.apply_deskew();
    assert_eq!(state.undo_stack.len(), 1);
    assert!(state.status_toast.is_some());
    let toast_msg = &state.status_toast.as_ref().unwrap().0;
    assert!(toast_msg.contains("Deskewed perspective"));
}

#[test]
fn test_perspective_deskew_crop_modes_and_point_placement() {
    let config = AppConfig::default();
    let img = RgbaImage::new(60, 60);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 60, 60, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.set_tool_mode(ImageToolMode::Deskew);
    assert_eq!(state.deskew_state.crop_mode, DeskewCropMode::CropToQuad);
    assert!(!state.deskew_state.is_clicking_points);

    // Test MinimalCrop mode execution
    state.deskew_state.crop_mode = DeskewCropMode::MinimalCrop;
    state.apply_deskew();
    assert_eq!(state.undo_stack.len(), 1);
    let toast_msg = &state.status_toast.as_ref().unwrap().0;
    assert!(toast_msg.contains("minimal crop"));

    // Test 4-point placement flow
    state.deskew_state.start_clicking_points();
    assert!(state.deskew_state.is_clicking_points);
    assert_eq!(state.deskew_state.placed_points.len(), 0);

    // While placing points, apply_deskew warns and returns early
    state.apply_deskew();
    let warn_msg = &state.status_toast.as_ref().unwrap().0;
    assert!(warn_msg.contains("Place all 4 points first"));

    // Place points sequentially: TL -> TR -> BR -> BL
    state.deskew_state.placed_points.push(Pos2::new(0.1, 0.1));
    state.deskew_state.placed_points.push(Pos2::new(0.85, 0.15));
    state.deskew_state.placed_points.push(Pos2::new(0.9, 0.85));
    state.deskew_state.placed_points.push(Pos2::new(0.15, 0.9));

    // Complete 4-point set
    state.deskew_state.corners = [
        state.deskew_state.placed_points[0],
        state.deskew_state.placed_points[1],
        state.deskew_state.placed_points[2],
        state.deskew_state.placed_points[3],
    ];
    state.deskew_state.is_clicking_points = false;
    state.deskew_state.placed_points.clear();
    state.deskew_state.corners_adjusted = true;

    assert_eq!(state.deskew_state.corners[0], Pos2::new(0.1, 0.1));
    assert_eq!(state.deskew_state.corners[2], Pos2::new(0.9, 0.85));

    // Reset quad reverts corners and adjusted flag
    state.deskew_state.reset_quad();
    assert!(!state.deskew_state.corners_adjusted);
    assert_eq!(state.deskew_state.corners[0], Pos2::new(0.08, 0.08));
}
