use super::*;
#[test]
fn heic_studio_import_preserves_encoded_exif_and_decodes_pixels() {
    let bytes =
        include_bytes!("../../../../little-clip-core/tests/fixtures/gradient.heic").to_vec();
    let source =
        DecodedImageData::from_bytes(bytes.clone(), "image/heic".to_string(), None).unwrap();
    assert_eq!(source.original_rgba.dimensions(), (8, 6));
    assert_eq!(source.encoded_bytes.as_ref(), &bytes);
    let metadata = source.metadata.as_ref().unwrap();
    assert_eq!(metadata.make.as_deref(), Some("LittleScreenClip"));
    assert_eq!(metadata.model.as_deref(), Some("CodecFixture"));
}

#[test]
fn decoded_image_data_is_shared_across_independent_editor_states() {
    let image = RgbaImage::from_pixel(2, 2, image::Rgba([24, 80, 180, 255]));
    let mut encoded = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .unwrap();
    let source = DecodedImageData::from_bytes(encoded, "image/png".to_string(), None).unwrap();

    let first = ImagePreviewState::from_decoded(
        source.clone(),
        None,
        "shared.png".to_string(),
        &AppConfig::default(),
    );
    let mut second = ImagePreviewState::from_decoded(
        source,
        None,
        "shared.png".to_string(),
        &AppConfig::default(),
    );
    assert!(Arc::ptr_eq(&first.current_rgba, &second.current_rgba));
    assert!(Arc::ptr_eq(&first.raw_bytes, &second.raw_bytes));
    assert_eq!(first.title, second.title);

    second.zoom = 2.0;
    assert_eq!(first.zoom, 1.0);
    Arc::make_mut(&mut second.current_rgba).put_pixel(0, 0, image::Rgba([240, 20, 40, 255]));
    assert!(!Arc::ptr_eq(&first.current_rgba, &second.current_rgba));
    assert_eq!(
        *first.current_rgba.get_pixel(0, 0),
        image::Rgba([24, 80, 180, 255])
    );
    assert_eq!(
        *second.current_rgba.get_pixel(0, 0),
        image::Rgba([240, 20, 40, 255])
    );
    assert!(Arc::ptr_eq(&first.raw_bytes, &second.raw_bytes));
}

#[test]
fn inline_decoded_image_source_seeds_independent_full_studios() {
    let image = RgbaImage::from_pixel(3, 2, image::Rgba([24, 80, 180, 255]));
    let mut encoded = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .unwrap();
    let source =
        Arc::new(DecodedImageData::from_bytes(encoded, "image/png".to_owned(), None).unwrap());
    let inline = InlineImagePreview::new(Arc::clone(&source));
    let mut first = ImagePreviewState::from_decoded(
        (*source).clone(),
        None,
        "First editor".to_owned(),
        &AppConfig::default(),
    );
    let second = ImagePreviewState::from_decoded(
        (*source).clone(),
        None,
        "Second editor".to_owned(),
        &AppConfig::default(),
    );

    assert!(Arc::ptr_eq(&inline.source, &source));
    assert!(Arc::ptr_eq(&first.current_rgba, &source.original_rgba));
    assert!(Arc::ptr_eq(&second.current_rgba, &source.original_rgba));
    assert!(Arc::ptr_eq(&first.raw_bytes, &source.encoded_bytes));
    assert!(first.palette_ticket.is_none());
    assert!(first.palette_rx.is_none());
    assert!(first.optimizer_rx.is_none());
    assert!(first.optimizer_job_rx.is_none());

    first.zoom = 2.0;
    assert_eq!(second.zoom, 1.0);
    Arc::make_mut(&mut first.current_rgba).put_pixel(0, 0, image::Rgba([240, 20, 40, 255]));
    assert!(!Arc::ptr_eq(&first.current_rgba, &second.current_rgba));
    assert_eq!(
        *second.current_rgba.get_pixel(0, 0),
        image::Rgba([24, 80, 180, 255])
    );
}

#[test]
fn inline_image_previews_render_shared_pixels_with_independent_zoom_state() {
    let image = RgbaImage::from_pixel(8, 4, image::Rgba([24, 80, 180, 255]));
    let mut encoded = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .unwrap();
    let source =
        Arc::new(DecodedImageData::from_bytes(encoded, "image/png".to_string(), None).unwrap());
    let mut first = InlineImagePreview::new(Arc::clone(&source));
    let mut second = InlineImagePreview::new(Arc::clone(&source));
    assert!(Arc::ptr_eq(&first.source, &second.source));

    let ctx = egui::Context::default();
    let viewport = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 420.0));
    let mut first_rect = Rect::NOTHING;
    let mut second_rect = Rect::NOTHING;
    let render = |ctx: &egui::Context,
                  first: &mut InlineImagePreview,
                  second: &mut InlineImagePreview,
                  events: Vec<egui::Event>,
                  first_rect: &mut Rect,
                  second_rect: &mut Rect| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical(|ui| {
                        *first_rect = first.show(ui, Vec2::new(300.0, 180.0)).rect;
                        *second_rect = second.show(ui, Vec2::new(220.0, 120.0)).rect;
                    });
                });
            },
        )
    };

    let first_frame = render(
        &ctx,
        &mut first,
        &mut second,
        Vec::new(),
        &mut first_rect,
        &mut second_rect,
    );
    let first_texture_id = first.texture.as_ref().unwrap().id();
    let second_texture_id = second.texture.as_ref().unwrap().id();
    for (texture_id, assigned_rect) in [
        (first_texture_id, first_rect),
        (second_texture_id, second_rect),
    ] {
        let mesh = first_frame
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Mesh(mesh)
                    if mesh.texture_id == texture_id && !mesh.vertices.is_empty() =>
                {
                    Some(mesh)
                }
                _ => None,
            })
            .expect("each inline preview should draw a textured mesh");
        for vertex in &mesh.vertices {
            assert!(vertex.pos.x >= assigned_rect.min.x - 0.1);
            assert!(vertex.pos.y >= assigned_rect.min.y - 0.1);
            assert!(vertex.pos.x <= assigned_rect.max.x + 0.1);
            assert!(vertex.pos.y <= assigned_rect.max.y + 0.1);
        }
    }

    let second_frame = render(
        &ctx,
        &mut first,
        &mut second,
        vec![
            egui::Event::PointerMoved(first_rect.center()),
            egui::Event::Scroll(Vec2::new(0.0, 120.0)),
        ],
        &mut first_rect,
        &mut second_rect,
    );

    assert!(first.zoom > 1.0);
    assert_eq!(second.zoom, 1.0);
    assert!(Arc::ptr_eq(
        &source.original_rgba,
        &first.source.original_rgba
    ));
    assert!(Arc::ptr_eq(
        &source.original_rgba,
        &second.source.original_rgba
    ));
    let preview_texture_ids = second_frame
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Mesh(mesh) if !mesh.vertices.is_empty() => Some(mesh.texture_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(preview_texture_ids.len(), 2);
    assert_ne!(preview_texture_ids[0], preview_texture_ids[1]);
    for clipped in &second_frame.shapes {
        let egui::epaint::Shape::Mesh(mesh) = &clipped.shape else {
            continue;
        };
        let assigned_rect = if mesh.texture_id == first_texture_id {
            Some(first_rect)
        } else if mesh.texture_id == second_texture_id {
            Some(second_rect)
        } else {
            None
        };
        if let Some(assigned_rect) = assigned_rect {
            assert!(clipped.clip_rect.min.x >= assigned_rect.min.x - 0.1);
            assert!(clipped.clip_rect.min.y >= assigned_rect.min.y - 0.1);
            assert!(clipped.clip_rect.max.x <= assigned_rect.max.x + 0.1);
            assert!(clipped.clip_rect.max.y <= assigned_rect.max.y + 0.1);
        }
    }
}

#[test]
fn clipped_inline_image_preview_defers_texture_upload() {
    let image = RgbaImage::from_pixel(4, 3, image::Rgba([24, 80, 180, 255]));
    let mut encoded = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .unwrap();
    let source =
        Arc::new(DecodedImageData::from_bytes(encoded, "image/png".to_string(), None).unwrap());
    let mut preview = InlineImagePreview::new(source);
    let ctx = egui::Context::default();

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 100.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(60.0)
                    .show(ui, |ui| {
                        ui.add_space(240.0);
                        preview.show(ui, Vec2::new(120.0, 80.0));
                    });
            });
        },
    );

    assert!(preview.texture.is_none());

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 500.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.add_space(240.0);
                        preview.show(ui, Vec2::new(120.0, 80.0));
                    });
            });
        },
    );

    assert!(preview.texture.is_some());
}

#[test]
fn raw_format_is_recognized_and_handled_in_decoded_image_data() {
    let dummy_raw_path = PathBuf::from("sample_camera_photo.NEF");
    let result = DecodedImageData::from_bytes(
        b"corrupt-or-unsupported-raw-payload".to_vec(),
        "image/x-nikon-nef".to_string(),
        Some(dummy_raw_path),
    );
    match result {
        Err(err) => assert!(err.contains("RAW")),
        Ok(_) => panic!("Expected error decoding invalid RAW payload"),
    }
}
