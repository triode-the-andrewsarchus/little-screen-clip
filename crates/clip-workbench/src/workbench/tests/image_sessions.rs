use super::*;

#[test]
fn image_session_suspension_retains_edited_state_for_matching_clipboard_item() {
    let key = ImageSessionKey::ClipboardItem(uuid::Uuid::new_v4());
    let mut image = test_image_preview_state();
    Arc::make_mut(&mut image.current_rgba).put_pixel(0, 0, image::Rgba([200, 10, 20, 255]));
    image.undo_stack.push(Arc::clone(&image.original_rgba));
    let edited_pixels = Arc::clone(&image.current_rgba);

    let mut app = test_workbench_app(Some(ViewerSession::Image(image)), VecDeque::new());
    app.active_image_key = Some(key.clone());
    app.retain_current_image_session();

    assert!(app.viewer.is_none());
    assert!(app.restore_image_session(&key));
    let Some(ViewerSession::Image(restored)) = &app.viewer else {
        panic!("matching image session should be restored");
    };
    assert!(Arc::ptr_eq(&restored.current_rgba, &edited_pixels));
    assert_eq!(
        restored.current_rgba.get_pixel(0, 0),
        &image::Rgba([200, 10, 20, 255])
    );
    assert_eq!(restored.undo_stack.len(), 1);
    assert_eq!(app.active_image_key, Some(key));
}

#[test]
fn file_backed_clipboard_image_restores_by_item_id_before_reading_its_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("backed.png");
    write_test_image(&path, 2);
    let item = ClipItem {
        id: uuid::Uuid::new_v4(),
        created_at: 0,
        payload: ClipPayload::Image {
            bytes: Vec::new(),
            path: Some(path.clone()),
            width: 2,
            height: 2,
            mime: "image/png".to_owned(),
            thumbnail: None,
        },
        title: "backed image".to_owned(),
        subtitle: "PNG".to_owned(),
        details: Vec::new(),
        handler_tag: "IMAGE".to_owned(),
    };
    let mut app = test_workbench_app(None, VecDeque::new());
    app.open_slot_preview_modal(&item);
    wait_for_image_load(&mut app);
    let Some(ViewerSession::Image(state)) = &mut app.viewer else {
        panic!("file-backed item should load into Image Studio");
    };
    Arc::make_mut(&mut state.current_rgba).put_pixel(0, 0, image::Rgba([9, 8, 7, 255]));
    state.undo_stack.push(Arc::clone(&state.original_rgba));
    app.retain_current_image_session();

    std::fs::remove_file(path).unwrap();
    app.open_slot_preview_modal(&item);
    assert!(app.image_load_job.is_none());
    let Some(ViewerSession::Image(restored)) = &app.viewer else {
        panic!("same clipboard item should restore without reopening its file");
    };
    assert_eq!(
        restored.current_rgba.get_pixel(0, 0),
        &image::Rgba([9, 8, 7, 255])
    );
    assert_eq!(restored.undo_stack.len(), 1);
    assert_eq!(
        app.active_image_key,
        Some(ImageSessionKey::ClipboardItem(item.id))
    );
}

#[test]
fn image_session_cache_drops_entry_for_a_different_clipboard_item() {
    let retained_key = ImageSessionKey::ClipboardItem(uuid::Uuid::new_v4());
    let requested_key = ImageSessionKey::ClipboardItem(uuid::Uuid::new_v4());
    let mut app = test_workbench_app(None, VecDeque::new());
    app.retained_image_session = Some((retained_key, test_image_preview_state()));

    assert!(!app.restore_image_session(&requested_key));
    assert!(app.retained_image_session.is_none());
    assert!(app.viewer.is_none());
}

#[test]
fn image_session_retention_skips_unmodified_image_state() {
    let key = ImageSessionKey::ClipboardItem(uuid::Uuid::new_v4());
    let app_viewer = ViewerSession::Image(test_image_preview_state());
    let mut app = test_workbench_app(Some(app_viewer), VecDeque::new());
    app.active_image_key = Some(key);

    app.retain_current_image_session();

    assert!(app.viewer.is_none());
    assert!(app.retained_image_session.is_none());
}

#[test]
fn file_image_session_key_changes_when_file_version_changes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("image.png");
    std::fs::write(&path, b"first version").unwrap();
    let first = ImageSessionKey::file(&path).unwrap();
    std::fs::write(&path, b"a different file version").unwrap();
    let second = ImageSessionKey::file(&path).unwrap();
    assert_ne!(first, second);
}

#[test]
fn malformed_image_preview_reports_cause_and_valid_preview_recovers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let malformed_path = temp_dir.path().join("broken.png");
    std::fs::write(&malformed_path, b"not a PNG").unwrap();
    let valid_path = temp_dir.path().join("valid.png");
    write_test_image(&valid_path, 2);
    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());

    app.open_slot_file_preview(&malformed_path, true);
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    assert!(app.image_load_job.is_some());
    wait_for_image_load(&mut app);
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    assert!(app.status_msg.as_deref().is_some_and(|message| {
        message.starts_with("Image preview failed: Failed to decode image bytes:")
    }));

    app.open_slot_file_preview(&valid_path, true);
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    wait_for_image_load(&mut app);
    assert!(matches!(app.viewer, Some(ViewerSession::Image(_))));
    assert!(app.status_msg.is_none());
}

#[test]
fn mirror_context_preview_routes_pdf_image_payload_to_pdf_editor() {
    let temp_dir = tempfile::tempdir().unwrap();
    let pdf_path = temp_dir.path().join("document.data");
    std::fs::write(&pdf_path, b"%PDF-1.4\n").unwrap();
    let item = ClipItem {
        id: uuid::Uuid::new_v4(),
        created_at: 0,
        payload: ClipPayload::Image {
            bytes: Vec::new(),
            path: Some(pdf_path),
            width: 800,
            height: 600,
            mime: "application/pdf".to_owned(),
            thumbnail: None,
        },
        title: "document.pdf".to_owned(),
        subtitle: "PDF (1 pg)".to_owned(),
        details: vec!["PDF".to_owned()],
        handler_tag: "PDF (DOCUMENT)".to_owned(),
    };
    assert!(item.is_image(), "PDF slot previews are image-backed");
    assert!(
        !is_image_preview_action_item(&item),
        "PDF MIME should use the PDF editor even when its backing path has no .pdf extension"
    );

    let mut app = test_workbench_app(None, VecDeque::new());
    Arc::make_mut(&mut app.state).push_left(item);
    app.handle_mirror_action(Some(MirrorAction::Context(SlotId::LeftHand)));

    assert!(matches!(app.viewer, Some(ViewerSession::Pdf(_))));
    assert!(app.image_load_job.is_none());
}

#[test]
fn mirror_context_preview_never_sends_pathless_pdf_bytes_to_image_decoder() {
    let item = ClipItem {
        id: uuid::Uuid::new_v4(),
        created_at: 0,
        payload: ClipPayload::Image {
            bytes: b"%PDF-1.4".to_vec(),
            path: None,
            width: 800,
            height: 600,
            mime: "application/pdf".to_owned(),
            thumbnail: None,
        },
        title: "document.pdf".to_owned(),
        subtitle: "PDF".to_owned(),
        details: vec!["PDF".to_owned()],
        handler_tag: "PDF (DOCUMENT)".to_owned(),
    };
    let mut app = test_workbench_app(None, VecDeque::new());
    Arc::make_mut(&mut app.state).push_left(item);

    app.handle_mirror_action(Some(MirrorAction::Context(SlotId::LeftHand)));

    assert!(app.viewer.is_none());
    assert!(app.image_load_job.is_none());
    assert_eq!(
        app.status_msg.as_deref(),
        Some("PDF preview requires a local file path")
    );
}

#[test]
fn clipboard_image_preview_decodes_owned_bytes_on_worker() {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([12, 34, 56, 255]),
    ))
    .write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let mut item = ClipItem::from_image_bytes(bytes, "image/png".to_string()).unwrap();
    if let ClipPayload::Image { path, .. } = &mut item.payload {
        *path = None;
    }
    let mut app = test_workbench_app(None, VecDeque::new());
    app.open_slot_preview_modal(&item);
    assert!(app.viewer.is_none());
    assert!(app.image_load_job.is_some());
    wait_for_image_load(&mut app);
    let Some(ViewerSession::Image(state)) = &app.viewer else {
        panic!("missing image viewer")
    };
    assert_eq!(state.current_rgba.dimensions(), (3, 2));
    assert_eq!(state.title, item.title);
    assert!(matches!(&item.payload, ClipPayload::Image { bytes, .. } if !bytes.is_empty()));
}

#[test]
fn rendered_image_studio_escape_restores_only_the_matching_edited_session() {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([12, 34, 56, 255]),
    ))
    .write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let item = ClipItem::from_image_bytes(bytes, "image/png".to_owned()).unwrap();
    let mut app = test_workbench_app(None, VecDeque::new());
    app.view = WorkbenchView::NodeMachine;
    app.selected_slots = vec![SlotId::LeftHand];
    Arc::make_mut(&mut app.state).left_hand = Some(item.clone());
    let state_before = app.state.clone();

    app.open_slot_preview_modal(&item);
    wait_for_image_load(&mut app);
    let Some(ViewerSession::Image(state)) = &mut app.viewer else {
        panic!("missing image viewer")
    };
    Arc::make_mut(&mut state.current_rgba).put_pixel(0, 0, image::Rgba([210, 30, 40, 255]));
    state.undo_stack.push(Arc::clone(&state.original_rgba));
    let edited_pixel = *state.current_rgba.get_pixel(0, 0);

    let _ = egui::Context::default().run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![key_press_event(egui::Key::Escape)],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(
        app.viewer.is_none(),
        "Escape closes the Image Studio viewer"
    );
    assert_eq!(app.view, WorkbenchView::NodeMachine);
    assert_eq!(app.selected_slots, vec![SlotId::LeftHand]);
    assert_eq!(
        app.state, state_before,
        "source clipboard item is unchanged"
    );
    assert_eq!(
        app.retained_image_session.as_ref().map(|(key, _)| key),
        Some(&ImageSessionKey::ClipboardItem(item.id))
    );

    app.open_slot_preview_modal(&item);
    let Some(ViewerSession::Image(restored)) = &app.viewer else {
        panic!("matching image item should restore its edited session")
    };
    assert_eq!(restored.current_rgba.get_pixel(0, 0), &edited_pixel);
    assert_eq!(restored.undo_stack.len(), 1);
    assert!(app.retained_image_session.is_none());

    let mut other_bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([1, 2, 3, 255]),
    ))
    .write_to(
        &mut std::io::Cursor::new(&mut other_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let other_item = ClipItem::from_image_bytes(other_bytes, "image/png".to_owned()).unwrap();
    app.open_slot_preview_modal(&other_item);
    wait_for_image_load(&mut app);
    let Some(ViewerSession::Image(replacement)) = &app.viewer else {
        panic!("different clipboard item should open a new image session")
    };
    assert_eq!(replacement.current_rgba.dimensions(), (2, 2));
    assert_eq!(
        replacement.current_rgba.get_pixel(0, 0),
        &image::Rgba([1, 2, 3, 255])
    );
    assert!(replacement.undo_stack.is_empty());
    assert!(app.retained_image_session.is_none());
    assert!(matches!(&item.payload, ClipPayload::Image { bytes, .. } if !bytes.is_empty()));
}
