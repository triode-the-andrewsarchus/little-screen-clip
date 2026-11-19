use super::*;

fn state() -> ImagePreviewState {
    let image = RgbaImage::from_pixel(32, 48, image::Rgba([120, 150, 180, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let item = little_clip_core::ClipItem::from_image(bytes.into_inner(), 32, 48, "image/png");
    let mut state = ImagePreviewState::from_item(&item, &AppConfig::default()).unwrap();
    state.set_tool_mode(ImageToolMode::Deskew);
    state
}

fn preview(state: &ImagePreviewState) -> Preview {
    Preview {
        original: Arc::new(state.current_display_image()),
        result: Arc::new(RgbaImage::from_pixel(
            32,
            48,
            image::Rgba([33, 44, 55, 255]),
        )),
        stamp: state.document_unwarp_stamp(),
        duration: std::time::Duration::from_millis(123),
    }
}

#[test]
fn both_bundled_models_match_pytorch_reference_pixels() {
    let fixtures: [(DeskewMethod, bool, &[u8], &[u8]); 6] = [
        (
            DeskewMethod::UvDoc,
            false,
            include_bytes!("../../../../assets/document-unwarp/test/uvdoc-input.png"),
            include_bytes!("../../../../assets/document-unwarp/test/uvdoc-expected.png"),
        ),
        (
            DeskewMethod::SyntheticDoc,
            false,
            include_bytes!("../../../../assets/document-unwarp/test/syntheticdoc-input.png"),
            include_bytes!("../../../../assets/document-unwarp/test/syntheticdoc-expected.png"),
        ),
        (
            DeskewMethod::SyntheticDoc,
            true,
            include_bytes!("../../../../assets/document-unwarp/test/syntheticdoc-input.png"),
            include_bytes!(
                "../../../../assets/document-unwarp/test/syntheticdoc-illuminated-expected.png"
            ),
        ),
        (
            DeskewMethod::UvDoc,
            false,
            include_bytes!("../../../../assets/document-unwarp/test/uvdoc-resized-input.png"),
            include_bytes!("../../../../assets/document-unwarp/test/uvdoc-resized-expected.png"),
        ),
        (
            DeskewMethod::SyntheticDoc,
            false,
            include_bytes!(
                "../../../../assets/document-unwarp/test/syntheticdoc-resized-input.png"
            ),
            include_bytes!(
                "../../../../assets/document-unwarp/test/syntheticdoc-resized-expected.png"
            ),
        ),
        (
            DeskewMethod::SyntheticDoc,
            true,
            include_bytes!(
                "../../../../assets/document-unwarp/test/syntheticdoc-resized-input.png"
            ),
            include_bytes!(
                "../../../../assets/document-unwarp/test/syntheticdoc-resized-illuminated-expected.png"
            ),
        ),
    ];
    let jobs = BackgroundJobs::new(1, 2);
    let ctx = egui::Context::default();
    for (method, illumination, input, expected) in fixtures {
        let source = image::load_from_memory(input).unwrap().to_rgba8();
        let expected = image::load_from_memory(expected).unwrap().to_rgba8();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            input.to_vec(),
            "image/png".into(),
            None,
            "reference".into(),
            &AppConfig::default(),
        )
        .unwrap();
        state.set_tool_mode(ImageToolMode::Deskew);
        state.select_deskew_method(method);
        state.document_unwarp.illumination = illumination;
        let original = state.current_rgba.clone();
        state.start_document_unwarp(&jobs);
        let start = Instant::now();
        while state.document_unwarp.pending() {
            state.poll_document_unwarp(&ctx, &jobs);
            assert!(
                start.elapsed().as_secs() < 60,
                "Native model did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            state.document_unwarp.error.is_none(),
            "{:?}",
            state.document_unwarp.error
        );
        assert!(Arc::ptr_eq(&state.current_rgba, &original));
        let actual = state
            .document_unwarp
            .preview
            .as_ref()
            .unwrap()
            .result
            .clone();
        assert_eq!(actual.dimensions(), source.dimensions());
        let differences: Vec<_> = actual
            .as_raw()
            .iter()
            .zip(expected.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let mean = differences.iter().map(|&v| v as f64).sum::<f64>() / differences.len() as f64;
        let maximum = differences.iter().max().unwrap();
        eprintln!(
            "{method:?} illumination={illumination} source={:?}: mean={mean:.6} max={maximum}",
            source.dimensions()
        );
        assert!(
            mean < 0.2 && *maximum <= 4,
            "{method:?} illumination={illumination} source={:?}: mean={mean} max={maximum}",
            source.dimensions()
        );
        if let Some(directory) = std::env::var_os("LITTLE_CLIP_UNWARP_ACCEPTANCE_DIR") {
            let directory = PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            actual
                .save(directory.join(format!("{method:?}-{illumination}.png")))
                .unwrap();
        }
        state.apply_document_unwarp();
        assert_eq!(state.current_rgba, actual);
        state.undo();
        assert_eq!(state.current_rgba, original);
        state.redo();
        assert_eq!(state.current_rgba, actual);
    }
}

#[test]
fn preview_is_non_destructive_and_apply_supports_undo_redo_for_both_methods() {
    for method in [DeskewMethod::UvDoc, DeskewMethod::SyntheticDoc] {
        let mut state = state();
        assert_eq!(state.deskew_state.method, DeskewMethod::Manual);
        state.select_deskew_method(method);
        let original = state.current_rgba.clone();
        let result = preview(&state);
        let expected = result.result.clone();
        state.document_unwarp.preview = Some(result);
        assert!(Arc::ptr_eq(&state.current_rgba, &original));
        assert!(state.undo_stack.is_empty());
        state.apply_document_unwarp();
        assert_eq!(state.current_rgba.as_ref(), expected.as_ref());
        assert_eq!(state.undo_stack.len(), 1);
        assert!(!state.document_unwarp.preview_ready());
        state.undo();
        assert_eq!(state.current_rgba.as_ref(), original.as_ref());
        state.redo();
        assert_eq!(state.current_rgba.as_ref(), expected.as_ref());
    }
}

#[test]
fn manual_deskew_stays_the_default_and_applies_without_a_model_job() {
    let mut state = state();
    assert_eq!(state.deskew_state.method, DeskewMethod::Manual);
    let original = state.current_rgba.clone();
    state.apply_deskew();
    assert_ne!(state.current_rgba.dimensions(), original.dimensions());
    assert_eq!(state.undo_stack.len(), 1);
    assert!(!state.document_unwarp.pending());
    assert!(!state.document_unwarp.preview_ready());
    state.undo();
    assert_eq!(state.current_rgba, original);
}

#[test]
fn preview_stamp_rejects_overlay_settings_and_pixel_changes() {
    for mutation in 0..3 {
        let mut state = state();
        state.select_deskew_method(DeskewMethod::SyntheticDoc);
        state.document_unwarp.preview = Some(preview(&state));
        match mutation {
            0 => state.document_unwarp.illumination = true,
            1 => state.markup_state.items.push(MarkupShape::Text {
                pos: Pos2::ZERO,
                text: "new".into(),
                font_size: 14.0,
            }),
            _ => state.mark_pixels_changed(),
        }
        let original = state.current_rgba.clone();
        state.apply_document_unwarp();
        assert!(Arc::ptr_eq(&state.current_rgba, &original));
        state.poll_document_unwarp(&egui::Context::default(), &BackgroundJobs::new(1, 1));
        assert!(!state.document_unwarp.preview_ready());
    }
}

#[test]
fn changing_method_leaving_tool_and_suspending_retire_preview() {
    for action in 0..3 {
        let mut state = state();
        state.select_deskew_method(DeskewMethod::UvDoc);
        state.document_unwarp.preview = Some(preview(&state));
        match action {
            0 => state.select_deskew_method(DeskewMethod::Manual),
            1 => state.set_tool_mode(ImageToolMode::View),
            _ => state.suspend_pending_work(),
        }
        assert!(!state.document_unwarp.preview_ready());
        assert!(!state.document_unwarp.pending());
        assert_eq!(state.undo_stack.len(), 0);
    }
}

#[test]
fn applying_preview_bakes_redactions_and_undo_keeps_them_covered() {
    let mut state = state();
    state.select_deskew_method(DeskewMethod::UvDoc);
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 1.0)),
        style: RedactionStyle::Black,
    });
    state.redaction_state.selected_idx = Some(0);
    state.document_unwarp.preview = Some(preview(&state));
    state.apply_document_unwarp();
    assert!(state.redaction_state.boxes.is_empty());
    assert!(state.redaction_state.selected_idx.is_none());
    state.undo();
    assert_eq!(
        *state.current_rgba.get_pixel(4, 4),
        image::Rgba([0, 0, 0, 255])
    );
    assert_eq!(
        *state.current_rgba.get_pixel(30, 4),
        image::Rgba([120, 150, 180, 255])
    );
}

#[test]
fn queued_worker_is_cancelled_on_editor_drop() {
    let jobs = BackgroundJobs::new(1, 2);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_ticket, done) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    let mut state = state();
    state.select_deskew_method(DeskewMethod::UvDoc);
    state.start_document_unwarp(&jobs);
    let token = state.document_unwarp.task.as_ref().unwrap().token.clone();
    drop(state);
    assert!(token.load(std::sync::atomic::Ordering::Acquire));
    release_tx.send(()).unwrap();
    done.recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
}

#[test]
fn full_queue_retries_only_admission_and_retires_changed_input() {
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_blocker, done) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    let (_filler, filled) = jobs.submit(std::time::Duration::MAX, || ());
    let mut state = state();
    state.select_deskew_method(DeskewMethod::UvDoc);
    state.start_document_unwarp(&jobs);
    let ctx = egui::Context::default();
    state.poll_document_unwarp(&ctx, &jobs);
    assert!(state.document_unwarp.retry_at.is_some());
    assert!(state.document_unwarp.error.is_none());
    state.mark_pixels_changed();
    state.poll_document_unwarp(&ctx, &jobs);
    assert!(!state.document_unwarp.pending());
    assert!(state.document_unwarp.request.is_none());
    release_tx.send(()).unwrap();
    done.recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    filled
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
}

#[test]
fn document_controls_expose_all_methods_and_only_syntheticdoc_illumination() {
    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    let mut state = state();
    for method in [
        DeskewMethod::Manual,
        DeskewMethod::UvDoc,
        DeskewMethod::SyntheticDoc,
    ] {
        state.select_deskew_method(method);
        let output = clip_ui::test_support::run_frame(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(360.0, 900.0))),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        state.render_deskew_toolbar(ui, ctx, &palette);
                    })
                });
            },
        );
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            text.contains("Manual") && text.contains("UVDoc") && text.contains("SyntheticDoc"),
            "{text}"
        );
        assert_eq!(
            text.contains("Correct illumination"),
            method == DeskewMethod::SyntheticDoc
        );
        assert_eq!(
            text.contains("Click 4 Points"),
            method == DeskewMethod::Manual
        );
        assert_eq!(
            text.contains("Preview Unwarp"),
            method != DeskewMethod::Manual
        );
    }
}

#[test]
#[ignore = "requires a CUDA GPU; verifies actual execution-provider kernel events"]
fn bundled_document_models_execute_on_cuda() {
    for (method, bytes) in [
        (DeskewMethod::UvDoc, inference::UVDOC),
        (DeskewMethod::SyntheticDoc, inference::SYNTHETICDOC),
    ] {
        let (width, height) = method.input_size();
        crate::onnx_runtime::assert_bundled_cuda_execution(method.label(), bytes, |session| {
            let input = ort::value::Tensor::from_array((
                [1_usize, 3, height, width],
                vec![0.5_f32; width * height * 3],
            ))
            .unwrap();
            session.run(ort::inputs!["image" => input]).unwrap();
        });
    }
}
