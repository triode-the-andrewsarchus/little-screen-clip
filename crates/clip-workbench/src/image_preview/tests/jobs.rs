use super::*;

#[test]
fn test_bg_removal_worker_invocation_and_undo() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(100, 100);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba([200, 100, 50, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 100, 100, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Create a temporary mock background removal script
    let mock_script = "import sys, io\nfrom PIL import Image\nimg = Image.open(io.BytesIO(sys.stdin.buffer.read())).convert('RGBA')\nout = Image.new('RGBA', img.size)\npixels = [(r, g, b, 0 if x < 50 else 255) for y in range(img.height) for x in range(img.width) for (r, g, b, a) in [img.getpixel((x, y))]]\nout.putdata(pixels)\nbuf = io.BytesIO()\nout.save(buf, format='PNG')\nsys.stdout.buffer.write(buf.getvalue())\n";
    let script_path = std::env::temp_dir().join("mock_nobg_worker.py");
    std::fs::write(&script_path, mock_script).unwrap();
    state.bg_removal_cmd = Some(format!("python3 {}", script_path.display()));

    state.run_background_removal();
    assert!(state.bg_removal_in_progress);

    for _ in 0..100 {
        state.poll_bg_removal();
        if !state.bg_removal_in_progress {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(!state.bg_removal_in_progress);
    assert!(
        state.bg_removal_error.is_none(),
        "Error: {:?}",
        state.bg_removal_error
    );
    assert_eq!(state.undo_stack.len(), 1);

    // Verify transparent pixel was applied on left half
    assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 0);
    assert_eq!(state.current_rgba.get_pixel(60, 60)[3], 255);
    let cached_removal = state
        .creative_fx_state
        .cached_background_removal
        .as_ref()
        .expect("Remove BG result should be cached for Creative FX");
    assert_eq!(cached_removal.as_raw(), state.current_rgba.as_raw());

    // Test Undo
    state.undo();
    assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 255);
    assert_eq!(state.undo_stack.len(), 0);

    // Test Redo
    state.redo();
    assert_eq!(state.current_rgba.get_pixel(10, 10)[3], 0);
}

#[cfg(unix)]
#[test]
fn background_removal_process_is_terminated_for_stale_input_mode_exit_and_drop() {
    fn start_sleeping_job(
        state: &mut ImagePreviewState,
    ) -> (
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        u32,
        std::path::PathBuf,
    ) {
        let script = std::env::temp_dir().join(format!("clip-bg-sleep-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
        )
        .unwrap();
        state.bg_removal_cmd = Some(format!("python3 {}", script.display()));
        state.set_tool_mode(ImageToolMode::RemoveBg);
        state.run_background_removal();
        let cancel = state.image_tool_cancel.as_ref().unwrap().clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state
            .image_tool_child_pid
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_ne!(
            state
                .image_tool_child_pid
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let pid = state
            .image_tool_child_pid
            .load(std::sync::atomic::Ordering::SeqCst);
        (cancel, pid, script)
    }

    fn assert_process_exited(pid: u32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("background-removal child {pid} remained alive after cancellation");
    }

    let mut stale = image_state_from_png((8, 8), "cancel stale background");
    let (stale_cancel, stale_pid, stale_script) = start_sleeping_job(&mut stale);
    stale.markup_state.items.push(MarkupShape::Text {
        pos: Pos2::ZERO,
        text: "changed".into(),
        font_size: 10.0,
    });
    stale.poll_bg_removal();
    assert!(stale_cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(stale_pid);
    assert_eq!(
        stale
            .image_tool_child_pid
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    drop(stale);
    let _ = std::fs::remove_file(stale_script);

    let mut mode_exit = image_state_from_png((8, 8), "cancel background mode");
    let (mode_cancel, mode_pid, mode_script) = start_sleeping_job(&mut mode_exit);
    mode_exit.set_tool_mode(ImageToolMode::View);
    assert!(mode_cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(mode_pid);
    assert!(!mode_exit.bg_removal_in_progress);
    drop(mode_exit);
    let _ = std::fs::remove_file(mode_script);

    let mut viewer_drop = image_state_from_png((8, 8), "drop background viewer");
    let (drop_cancel, drop_pid, drop_script) = start_sleeping_job(&mut viewer_drop);
    drop(viewer_drop);
    assert!(drop_cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(drop_pid);
    let _ = std::fs::remove_file(drop_script);
}

#[test]
fn test_convert_state_png_jpeg_webp_and_optipng() {
    let config = AppConfig::default();
    let mut img = RgbaImage::new(40, 40);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        *pixel = image::Rgba([((x * 6) % 255) as u8, ((y * 6) % 255) as u8, 128, 255]);
    }
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 40, 40, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // 1. Conversion is lazy so loading an image never performs an eager encode.
    assert_eq!(state.convert_state.target_format, ConvertTargetFormat::Webp);
    assert!(state.convert_state.encoded_bytes.is_empty());
    assert!(state.convert_state.converted_rgba.is_none());

    // Exercise each encoder synchronously here; the UI dispatches the same work to its worker.
    state.update_conversion();
    assert!(!state.convert_state.encoded_bytes.is_empty());
    assert!(state.convert_state.converted_rgba.is_some());

    // 2. Switch to PNG and test OptiPNG
    state.convert_state.target_format = ConvertTargetFormat::Png;
    state.convert_state.lossless = true;
    state.update_conversion();
    assert!(!state.convert_state.encoded_bytes.is_empty());
    assert!(state.convert_state.converted_rgba.is_some());

    state.run_optipng_conversion();
    assert!(!state.convert_state.encoded_bytes.is_empty());

    // 3. Switch to JPEG lossy
    state.convert_state.target_format = ConvertTargetFormat::Jpeg;
    state.convert_state.lossless = false;
    state.convert_state.quality = 75;
    state.update_conversion();
    assert!(!state.convert_state.encoded_bytes.is_empty());
    assert!(state.convert_state.converted_rgba.is_some());

    // 4. Switch to WebP lossless
    state.convert_state.target_format = ConvertTargetFormat::Webp;
    state.convert_state.lossless = true;
    state.update_conversion();
    assert!(!state.convert_state.encoded_bytes.is_empty());
    assert!(state.convert_state.converted_rgba.is_some());

    // 5. Switch to WebP lossy
    state.convert_state.lossless = false;
    state.convert_state.quality = 60;
    state.update_conversion();
    assert!(!state.convert_state.encoded_bytes.is_empty());
    assert!(state.convert_state.converted_rgba.is_some());

    // 6. Apply converted to canvas and verify undo stack
    state.apply_converted_to_canvas();
    assert_eq!(state.undo_stack.len(), 1);
}

#[test]
fn conversion_worker_coalesces_changes_and_rejects_stale_results() {
    let config = AppConfig::default();
    let mut image = RgbaImage::new(96, 96);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgba([(x * 2) as u8, (y * 2) as u8, (x ^ y) as u8, 255]);
    }
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let item = ClipItem::from_image(png, 96, 96, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();
    let initial_bytes = state.convert_state.encoded_bytes.clone();

    state.convert_state.target_format = ConvertTargetFormat::Jpeg;
    state.convert_state.lossless = false;
    state.convert_state.quality = 20;
    state.schedule_conversion();
    assert!(state.convert_state.is_pending());
    assert_eq!(state.convert_state.encoded_bytes, initial_bytes);

    state.run_conversion_worker();
    state.convert_state.quality = 90;
    state.schedule_conversion();
    let latest_generation = state.convert_state.request_generation;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.convert_state.is_encoding && std::time::Instant::now() < deadline {
        state.poll_conversion();
        std::thread::yield_now();
    }
    assert!(!state.convert_state.output_is_current());
    assert_eq!(state.convert_state.encoded_bytes, initial_bytes);

    state.run_conversion_worker();
    let latest_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.convert_state.is_encoding && std::time::Instant::now() < latest_deadline {
        state.poll_conversion();
        std::thread::yield_now();
    }
    assert!(state.convert_state.output_is_current());
    assert_eq!(state.convert_state.completed_generation, latest_generation);
    assert_ne!(state.convert_state.encoded_bytes, initial_bytes);
}

#[test]
fn test_barcode_state_and_scan() {
    let config = AppConfig::default();
    let writer = rxing::qrcode::QRCodeWriter;
    use rxing::Writer;
    for payload in ["https://github.com", "clipboard test 日本語 ✓"] {
        let bit_matrix = writer
            .encode(payload, &rxing::BarcodeFormat::QR_CODE, 96, 96)
            .unwrap();

        let mut img = RgbaImage::new(96, 96);
        for y in 0..96 {
            for x in 0..96 {
                let color = if bit_matrix.get(x, y) {
                    image::Rgba([0, 0, 0, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                };
                img.put_pixel(x as u32, y as u32, color);
            }
        }

        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let item = ClipItem::from_image(png_bytes, 96, 96, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

        state.run_barcode_scan();
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !state.barcode_state.has_scanned && std::time::Instant::now() < deadline {
            state.poll_barcode_scan(&ctx);
            std::thread::yield_now();
        }
        assert!(state.barcode_state.has_scanned);
        assert_eq!(state.barcode_state.detections.len(), 1);
        assert_eq!(state.barcode_state.detections[0].text, payload);
    }
}

#[test]
fn barcode_copy_actions_emit_exact_payloads() {
    let mut state = image_state_from_png((8, 8), "barcode copy actions");
    let payloads = ["https://example.com", "clipboard 日本語 ✓"];
    state.barcode_state = BarcodeScanState {
        detections: payloads
            .iter()
            .map(|text| BarcodeDetection {
                format: "QR Code".to_owned(),
                text: (*text).to_owned(),
                points: vec![],
                bbox: [0.0; 4],
                is_url: text.starts_with("https://"),
            })
            .collect(),
        selected_idx: Some(0),
        is_scanning: false,
        has_scanned: true,
        last_scan_duration_ms: 3,
    };

    let palette = AppPalette::for_appearance(little_clip_core::config::WorkbenchAppearance::Dark);
    let ctx = egui::Context::default();
    let render = |state: &mut ImagePreviewState, time, events| {
        ctx.run(
            egui::RawInput {
                time: Some(time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 500.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_barcode_toolbar(ui, ctx, &palette);
                });
            },
        )
    };
    let text_center = |output: &egui::FullOutput, expected: &str| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == expected => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("rendered action text {expected:?} not found"))
    };
    let click = |state: &mut ImagePreviewState, pos: Pos2, time: f64| {
        render(state, time, vec![egui::Event::PointerMoved(pos)]);
        render(
            state,
            time + 0.1,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        render(
            state,
            time + 0.2,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        )
    };

    render(&mut state, 0.0, vec![]);
    let initial = render(&mut state, 0.1, vec![]);
    let all_pos = text_center(&initial, "📋 Copy All");
    let copied_all = click(&mut state, all_pos, 0.2);
    assert_eq!(copied_all.platform_output.copied_text, payloads.join("\n"));

    let after_copy_all = render(&mut state, 0.5, vec![]);
    let selected_pos = text_center(&after_copy_all, "📋");
    let copied_selected = click(&mut state, selected_pos, 0.6);
    assert_eq!(copied_selected.platform_output.copied_text, payloads[0]);

    let with_url_action = render(&mut state, 0.9, vec![]);
    let open_url_pos = text_center(&with_url_action, "🌐 Open");
    let opened = click(&mut state, open_url_pos, 1.0);
    let open_url = opened
        .platform_output
        .open_url
        .expect("clicking a URL result requests opening its link");
    assert_eq!(open_url.url, payloads[0]);
    assert!(open_url.new_tab);

    state.barcode_state.detections = vec![BarcodeDetection {
        format: "QR Code".to_owned(),
        text: payloads[1].to_owned(),
        points: vec![],
        bbox: [0.0; 4],
        is_url: false,
    }];
    let text_only = render(&mut state, 1.3, vec![]);
    assert!(
        text_only
            .shapes
            .iter()
            .all(|shape| !matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "🌐 Open")),
        "non-URL payloads must not render an Open action"
    );
    assert!(text_only.platform_output.open_url.is_none());
}

#[test]
fn test_ocr_state_transitions() {
    let config = AppConfig::default();
    let img = RgbaImage::new(20, 20);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
    let state = ImagePreviewState::from_item(&item, &config).unwrap();

    assert_eq!(state.ocr_state.engine, OcrEngine::GlmOcr);
    assert!(!state.ocr_state.in_progress);
    assert!(state.ocr_state.extracted_text.is_empty());

    // Test engine enum conversions
    assert_eq!(OcrEngine::GlmOcr.arg_name(), "glm-ocr");
    assert_eq!(OcrEngine::Tesseract.arg_name(), "tesseract");
}

#[test]
fn test_ocr_cancellation_and_cleanup() {
    let config = AppConfig::default();
    let img = RgbaImage::new(20, 20);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.ocr_state.in_progress = true;
    state.ocr_state.status_text = Some("Processing image with GLM-OCR...".into());
    let control = std::sync::Arc::new(ImageToolProcessControl::new());
    control
        .child_pid
        .store(999999, std::sync::atomic::Ordering::SeqCst);
    state.ocr_process_control = Some(control);

    // Cancel OCR
    state.cancel_ocr();

    assert!(!state.ocr_state.in_progress);
    assert!(state.ocr_state.status_text.is_none());
    assert_eq!(
        state.status_toast.as_ref().map(|toast| toast.0.as_str()),
        Some("⏹ OCR process cancelled")
    );
    assert!(state.ocr_process_control.is_none());
    assert!(state.ocr_rx.is_none());

    // Drop state test - ensuring drop executes cancel_ocr without panic
    {
        let mut scoped_state = ImagePreviewState::from_item(&item, &config).unwrap();
        scoped_state.ocr_state.in_progress = true;
        // Dropping scoped_state here invokes Drop -> cancel_ocr
    }
}

#[test]
fn test_tool_mode_labels_and_shortcuts() {
    assert_eq!(ImageToolMode::View.label(), "[V]iew");
    assert_eq!(ImageToolMode::Crop.label(), "[C]rop");
    assert_eq!(ImageToolMode::Deskew.label(), "Des[K]ew");
    assert_eq!(ImageToolMode::Transform.label(), "[F]lip/Rotate");
    assert_eq!(ImageToolMode::Redact.label(), "[X] Redact");
    assert_eq!(ImageToolMode::PickColor.label(), "[P]ick Color");
    assert_eq!(ImageToolMode::Resize.label(), "Re[S]ize");
    assert_eq!(ImageToolMode::Convert.label(), "Conver[T]");
    assert_eq!(ImageToolMode::Ocr.label(), "[O]CR");
    assert_eq!(ImageToolMode::Barcode.label(), "[B]arcode");
    assert_eq!(ImageToolMode::RemoveBg.label(), "[R]emove BG");
    assert_eq!(ImageToolMode::DepthMap.label(), "[D]epth Map");
    assert_eq!(
        ImageToolMode::DecorrelationStretch.label(),
        "Decorrelation Stretch"
    );
}

#[test]
fn test_set_tool_mode_integration() {
    let config = AppConfig::default();
    let img = RgbaImage::new(20, 20);
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image(png_bytes, 20, 20, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    // Switching to Palette routes to PickColor and auto-runs palette extraction
    state.set_tool_mode(ImageToolMode::Palette);
    assert_eq!(state.active_mode, ImageToolMode::PickColor);
    assert!(state.palette_state.is_extracting);

    // Switching to Crop
    state.set_tool_mode(ImageToolMode::Crop);
    assert_eq!(state.active_mode, ImageToolMode::Crop);

    // Switching to Deskew
    state.set_tool_mode(ImageToolMode::Deskew);
    assert_eq!(state.active_mode, ImageToolMode::Deskew);

    // Switching to Transform
    state.set_tool_mode(ImageToolMode::Transform);
    assert_eq!(state.active_mode, ImageToolMode::Transform);

    // Switching to DecorrelationStretch
    state.set_tool_mode(ImageToolMode::DecorrelationStretch);
    assert_eq!(state.active_mode, ImageToolMode::DecorrelationStretch);
    wait_for_dcs(&mut state);
    assert!(state.dcs_state.stretched_rgba.is_some());

    // Switching from OCR in progress cancels OCR
    state.set_tool_mode(ImageToolMode::Ocr);
    assert_eq!(state.active_mode, ImageToolMode::Ocr);
    state.ocr_state.in_progress = true;
    state.ocr_process_control = Some(std::sync::Arc::new(ImageToolProcessControl::new()));
    state.set_tool_mode(ImageToolMode::View);
    assert_eq!(state.active_mode, ImageToolMode::View);
    assert!(!state.ocr_state.in_progress);
    assert!(state.ocr_process_control.is_none());
}

#[test]
fn thumbnail_batches_cancel_on_mode_exit_and_source_change() {
    let config = AppConfig::default();
    let image = RgbaImage::from_pixel(42, 28, image::Rgba([40, 80, 120, 255]));
    let mut png_bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let item = ClipItem::from_image(png_bytes, 42, 28, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &config).unwrap();

    state.set_tool_mode(ImageToolMode::FilmSim);
    let film_cancel = state
        .film_sim_state
        .thumbnail_cancel
        .as_ref()
        .expect("Film Look batch started")
        .clone();
    state.set_tool_mode(ImageToolMode::View);
    assert!(film_cancel.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.film_sim_state.is_generating_thumbnails);
    assert!(state.film_sim_state.thumbnail_rx.is_none());

    state.set_tool_mode(ImageToolMode::CreativeFx);
    let fx_cancel = state
        .creative_fx_state
        .thumbnail_cancel
        .as_ref()
        .expect("Creative FX batch started")
        .clone();
    state.mark_pixels_changed();
    assert!(fx_cancel.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.creative_fx_state.is_generating_thumbnails);
    assert!(state.creative_fx_state.thumbnail_rx.is_none());
    assert!(state.creative_fx_state.pending_thumbnails.is_empty());
    assert!(state.creative_fx_state.fx_thumbnails.is_empty());
}

#[test]
fn dropping_image_preview_cancels_both_thumbnail_batches() {
    let mut state = image_state_from_png((42, 28), "thumbnail drop test");
    state.run_thumbnail_batch();
    state.run_creative_fx_thumbnail_batch();
    let film_cancel = state
        .film_sim_state
        .thumbnail_cancel
        .as_ref()
        .expect("Film Look batch started")
        .clone();
    let fx_cancel = state
        .creative_fx_state
        .thumbnail_cancel
        .as_ref()
        .expect("Creative FX batch started")
        .clone();

    drop(state);

    assert!(film_cancel.load(std::sync::atomic::Ordering::Acquire));
    assert!(fx_cancel.load(std::sync::atomic::Ordering::Acquire));
}
