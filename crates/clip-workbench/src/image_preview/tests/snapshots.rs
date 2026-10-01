use super::*;

#[test]
fn composed_input_stamp_tracks_every_display_image_input() {
    let mut state = image_state_from_png((8, 8), "input stamp test");
    let initial = state.input_stamp();
    assert_eq!(state.input_stamp(), initial);

    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
    });
    assert_ne!(state.input_stamp(), initial);
    let redacted = state.input_stamp();

    state.markup_state.items.push(MarkupShape::Text {
        pos: Pos2::new(0.2, 0.2),
        text: "stamp".into(),
        font_size: 12.0,
    });
    assert_ne!(state.input_stamp(), redacted);
    let marked = state.input_stamp();

    state.active_mode = ImageToolMode::Crop;
    assert_ne!(state.input_stamp(), marked);
    let cropped = state.input_stamp();
    state.crop_state.normalized_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5));
    assert_ne!(state.input_stamp(), cropped);
    let cropped_selection = state.input_stamp();

    state.active_mode = ImageToolMode::View;
    assert_ne!(state.input_stamp(), cropped_selection);
    let view_mode = state.input_stamp();
    state.mark_pixels_changed();
    assert_ne!(state.input_stamp(), view_mode);
}

#[test]
fn display_snapshot_request_shares_the_pixel_arc() {
    let state = image_state_from_png((12, 9), "snapshot arc test");

    let request = state.display_image_snapshot_request();

    assert!(Arc::ptr_eq(&request.pixels, &state.current_rgba));
    assert_eq!(request.input_stamp, state.input_stamp());
}

#[test]
fn display_snapshot_composes_redactions_markup_and_crop_like_current_image() {
    let mut state = image_state_from_png((80, 60), "snapshot compose test");
    state.current_rgba = Arc::new(RgbaImage::from_fn(80, 60, |x, y| {
        image::Rgba([(x * 3) as u8, (y * 4) as u8, (x + y) as u8, 255])
    }));
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.45, 0.5)),
    });
    state.markup_state.items.push(MarkupShape::Text {
        pos: Pos2::new(0.55, 0.5),
        text: "snapshot".into(),
        font_size: 12.0,
    });

    let edited_request = state.display_image_snapshot_request();
    assert_eq!(edited_request.compose(), state.current_display_image());

    state.active_mode = ImageToolMode::Crop;
    state.crop_state.normalized_rect =
        Rect::from_min_max(Pos2::new(0.1, 0.15), Pos2::new(0.8, 0.85));
    let crop_request = state.display_image_snapshot_request();
    assert_eq!(crop_request.compose(), state.current_display_image());
    assert_eq!(crop_request.compose().dimensions(), (56, 42));
    assert!(Arc::ptr_eq(&crop_request.pixels, &state.current_rgba));
}

#[test]
fn background_worker_discards_stale_success_and_clears_loading() {
    let mut state = image_state_from_png((8, 8), "stale background result");
    let previous_cache = RgbaImage::from_pixel(1, 1, image::Rgba([9, 8, 7, 255]));
    state.creative_fx_state.cached_background_removal = Some(previous_cache.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    state.bg_removal_input_stamp = Some(state.input_stamp());
    state.bg_removal_rx = Some(rx);
    state.bg_removal_in_progress = true;
    state.bg_removal_status = Some("working".into());
    tx.send(BgRemovalMsg::Success {
        result_rgba: RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])),
        duration_ms: 5,
    })
    .unwrap();
    state.markup_state.items.push(MarkupShape::Text {
        pos: Pos2::ZERO,
        text: "new input".into(),
        font_size: 10.0,
    });

    state.poll_bg_removal();

    assert_eq!(
        state.current_rgba.get_pixel(0, 0),
        &image::Rgba([0, 0, 0, 0])
    );
    assert_eq!(
        state.creative_fx_state.cached_background_removal,
        Some(previous_cache)
    );
    assert!(!state.bg_removal_in_progress);
    assert!(state.bg_removal_status.is_none());
    assert!(state.bg_removal_rx.is_none());
    assert!(state.bg_removal_error.is_none());
}

#[test]
fn background_worker_applies_unchanged_input_and_clears_baked_overlays() {
    let mut state = image_state_from_png((8, 8), "current background result");
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
    });
    state.markup_state.items.push(MarkupShape::Text {
        pos: Pos2::ZERO,
        text: "baked once".into(),
        font_size: 10.0,
    });
    let stamp = state.input_stamp();
    let output = RgbaImage::from_pixel(8, 8, image::Rgba([11, 22, 33, 255]));
    let (tx, rx) = std::sync::mpsc::channel();
    state.bg_removal_input_stamp = Some(stamp);
    state.bg_removal_rx = Some(rx);
    state.bg_removal_in_progress = true;
    tx.send(BgRemovalMsg::Success {
        result_rgba: output.clone(),
        duration_ms: 5,
    })
    .unwrap();

    state.poll_bg_removal();

    assert_eq!(state.current_rgba.as_ref(), &output);
    assert_eq!(state.pixel_revision, 1);
    assert!(state.redaction_state.boxes.is_empty());
    assert!(state.markup_state.items.is_empty());
    assert_eq!(
        state.creative_fx_state.cached_background_removal,
        Some(output)
    );
    assert_eq!(
        state.status_toast.as_ref().unwrap().0,
        "✨ Background removed in 0.01s!"
    );
    assert!(!state.bg_removal_in_progress);
    assert!(state.bg_removal_rx.is_none());
}

#[test]
fn depth_map_completion_uses_a_depth_specific_toast() {
    let mut state = image_state_from_png((8, 8), "depth map result");
    let (tx, rx) = std::sync::mpsc::channel();
    state.bg_removal_input_stamp = Some(state.input_stamp());
    state.bg_removal_operation = BgRemovalOperation::DepthMap;
    state.bg_removal_rx = Some(rx);
    state.bg_removal_in_progress = true;
    tx.send(BgRemovalMsg::Success {
        result_rgba: RgbaImage::from_pixel(8, 8, image::Rgba([24, 24, 24, 255])),
        duration_ms: 1250,
    })
    .unwrap();

    state.poll_bg_removal();

    let toast = state.status_toast.as_ref().unwrap();
    assert_eq!(toast.0, "🌊 Depth map generated in 1.25s!");
    assert_eq!(
        state.bg_removal_operation,
        BgRemovalOperation::BackgroundRemoval
    );
}

#[test]
fn background_worker_discards_stale_error_without_showing_it() {
    let mut state = image_state_from_png((8, 8), "stale background error");
    let (tx, rx) = std::sync::mpsc::channel();
    state.bg_removal_input_stamp = Some(state.input_stamp());
    state.bg_removal_rx = Some(rx);
    state.bg_removal_in_progress = true;
    tx.send(BgRemovalMsg::Error("stale worker failure".into()))
        .unwrap();
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.25, 0.25)),
    });

    state.poll_bg_removal();

    assert!(state.bg_removal_error.is_none());
    assert!(!state.bg_removal_in_progress);
    assert!(state.bg_removal_status.is_none());
    assert!(state.bg_removal_rx.is_none());
}

#[test]
fn reentering_convert_schedules_when_cached_result_source_is_stale() {
    let mut state = image_state_from_png((8, 8), "convert mode reentry");
    state.convert_state.converted_rgba = Some(RgbaImage::new(8, 8));
    state.convert_state.completed_generation = state.convert_state.request_generation;
    state.convert_state.completed_input_stamp = Some(state.input_stamp());
    state.mark_pixels_changed();

    state.set_tool_mode(ImageToolMode::Convert);

    assert!(state.convert_state.params_dirty_at.is_some());
    assert!(!state.convert_state.output_matches(&state.input_stamp()));
}
