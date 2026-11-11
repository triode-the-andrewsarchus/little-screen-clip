use super::*;

#[cfg(unix)]
#[test]
fn krita_handoff_receives_current_source_image_file() {
    assert_configured_app_receives_source_image("fake-krita", |config, executable| {
        config.image.external_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn inkscape_handoff_receives_current_source_image_file() {
    assert_configured_app_receives_source_image("fake-inkscape", |config, executable| {
        config.image.svg_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn viewer_handoff_receives_current_source_image_file() {
    assert_configured_app_receives_source_image("fake-nomacs", |config, executable| {
        config.image.viewer = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn returning_from_krita_keeps_the_open_edited_image_session() {
    assert_external_handoff_keeps_edited_session("krita", |config| {
        config.image.external_editor.clone()
    });
}

#[cfg(unix)]
#[test]
fn returning_from_inkscape_keeps_the_open_edited_image_session() {
    assert_external_handoff_keeps_edited_session("inkscape", |config| {
        config.image.svg_editor.clone()
    });
}

#[cfg(unix)]
#[test]
fn returning_from_image_viewer_keeps_the_open_edited_image_session() {
    assert_external_handoff_keeps_edited_session("nomacs", |config| config.image.viewer.clone());
}

#[cfg(unix)]
#[test]
fn missing_krita_sets_image_studio_failure_toast_without_host_fallback() {
    assert_unavailable_configured_app_sets_failure_toast("krita", |config, executable| {
        config.image.external_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn missing_inkscape_sets_image_studio_failure_toast_without_host_fallback() {
    assert_unavailable_configured_app_sets_failure_toast("inkscape", |config, executable| {
        config.image.svg_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn missing_viewer_sets_image_studio_failure_toast_without_host_fallback() {
    assert_unavailable_configured_app_sets_failure_toast("nomacs", |config, executable| {
        config.image.viewer = executable.to_owned();
    });
}

#[test]
fn external_handoff_renders_pending_redaction_markup_and_crop_without_undo_history() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("source.png");
    let pixels = RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]));
    pixels.save(&original).unwrap();
    let mut state = ImagePreviewState::from_path(original.clone(), &AppConfig::default()).unwrap();
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.25, 0.25)),
        style: RedactionStyle::Black,
    });
    state.markup_state.items.push(MarkupShape::Rectangle {
        rect: Rect::from_min_max(Pos2::new(0.5, 0.5), Pos2::new(0.75, 0.75)),
        color: Color32::RED,
        width: 1.0,
        filled: true,
        rounding: 0.0,
    });
    state.active_mode = ImageToolMode::Crop;
    state.crop_state.normalized_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(0.75, 0.75));
    assert!(state.undo_stack.is_empty());

    state.start_image_handoff("fixture", BackgroundJobs::global());
    let mut launched = false;
    wait_for_image_handoff(&mut state, |_, _, path| {
        launched = true;
        assert_ne!(path, original);
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (6, 6));
        assert_eq!(*image.get_pixel(0, 0), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*image.get_pixel(5, 5), image::Rgba([255, 0, 0, 255]));
        std::fs::remove_file(path).unwrap();
        Ok("Fixture received edited image".into())
    });

    assert!(launched);
    assert_eq!(image::open(original).unwrap().to_rgba8(), pixels);
    assert!(state.undo_stack.is_empty());
}

#[test]
fn new_redaction_prevents_launching_an_outdated_handoff() {
    let mut state = image_state_from_png((8, 8), "handoff");
    state.start_image_handoff("fixture", BackgroundJobs::global());
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
        style: RedactionStyle::Black,
    });
    wait_for_image_handoff(&mut state, |_, _, _| panic!("stale image must not launch"));
    assert!(
        state
            .status_toast
            .as_ref()
            .unwrap()
            .0
            .contains("Image changed")
    );
}

#[test]
fn failed_handoff_reclaims_pathless_image_staging() {
    let mut state = image_state_from_png((8, 8), "clipboard image");
    let original_bytes = Arc::clone(&state.raw_bytes);
    state.start_image_handoff("fixture", BackgroundJobs::global());
    let mut staged_path = None;
    wait_for_image_handoff(&mut state, |_, _, path| {
        assert_eq!(std::fs::read(path).unwrap(), *original_bytes);
        staged_path = Some(path.to_owned());
        Err("Fixture launch failed".into())
    });
    assert!(
        !staged_path
            .expect("launcher received a staged file")
            .exists()
    );
    assert!(
        state
            .status_toast
            .as_ref()
            .unwrap()
            .0
            .contains("Fixture launch failed")
    );
}
