use super::*;

#[test]
fn shared_page_raster_keeps_png_alive_until_last_resource_drops() {
    let state = raster_state(1, 16);
    let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
    let raster_path = raster.path().to_owned();
    let raster_directory = raster_path.parent().unwrap().to_owned();
    assert_eq!(raster.page_num(), 1);
    assert_eq!(raster.dimensions(), (16, 16));

    drop(state);

    assert!(raster_directory.exists());
    let decoded =
        decode_page_png(std::fs::read(&raster_path).unwrap(), PAGE_DETAIL_MAX_DIM).unwrap();
    assert_eq!(decoded.size, [16, 16]);

    drop(raster);
    assert!(!raster_directory.exists());
}

#[test]
fn inline_and_editor_share_raster_with_independent_decode_and_texture_state() {
    let ctx = egui::Context::default();
    let mut state = raster_state(1, 16);
    let editor_raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
    let raster_path = editor_raster.path().to_owned();
    let raster_directory = raster_path.parent().unwrap().to_owned();
    let mut inline = InlinePdfPagePreview::new(Arc::clone(&editor_raster));
    assert!(Arc::ptr_eq(
        state.pages[0].raster.as_ref().unwrap(),
        &inline.raster
    ));

    state.ensure_page_texture(1, &ctx);
    state.schedule_page_decode(&ctx);
    show_inline_frame(&ctx, &mut inline);
    let editor_slot = state.decode_job.as_ref().unwrap();
    let inline_slot = inline.decode_slot.as_ref().unwrap();
    assert!(!std::ptr::eq(editor_slot, inline_slot));
    assert!(Arc::ptr_eq(&editor_slot.raster, &inline_slot.raster));

    let editor_texture = upload_page(&mut state, 1, &ctx);
    wait_for_inline_texture(&ctx, &mut inline);
    let inline_texture = inline.texture.as_ref().unwrap().id();
    assert_ne!(editor_texture, inline_texture);

    drop(inline);
    assert!(raster_path.exists());
    state.pages[0].texture = None;
    upload_page(&mut state, 1, &ctx);
    drop(editor_raster);
    drop(state);
    assert!(!raster_directory.exists());
}

#[test]
fn gallery_page_opens_editor_without_raster_workers_and_retains_source_files() {
    let (pdf_path, raster) = gallery_pdf_page_source().unwrap();
    let raster_path = raster.path().to_owned();
    let directory = pdf_path.parent().unwrap().to_owned();
    assert_eq!(get_pdf_info(&pdf_path).unwrap().page_count, 1);

    let mut inline = InlinePdfPagePreview::new(Arc::clone(&raster));
    let mut editor = PdfEditorState::from_shared_page_raster(pdf_path.clone(), Arc::clone(&raster));
    assert!(Arc::ptr_eq(
        editor.pages[0].raster.as_ref().unwrap(),
        &inline.raster
    ));
    assert!(editor.info_job.is_none());
    assert!(editor.preview_job.is_none());
    assert!(editor.preview_rx.is_none());
    assert!(!editor.previews_loading);
    assert_eq!(editor.selected_page, Some(1));

    let ctx = egui::Context::default();
    editor.ensure_page_texture(1, &ctx);
    editor.schedule_page_decode(&ctx);
    show_inline_frame(&ctx, &mut inline);
    assert!(!std::ptr::eq(
        editor.decode_job.as_ref().unwrap(),
        inline.decode_slot.as_ref().unwrap()
    ));
    let editor_texture = upload_page(&mut editor, 1, &ctx);
    wait_for_inline_texture(&ctx, &mut inline);
    assert_ne!(editor_texture, inline.texture.as_ref().unwrap().id());

    drop(raster);
    drop(inline);
    assert!(pdf_path.exists());
    assert!(raster_path.exists());

    // A successful edit reloads metadata and clears the original page
    // raster. The adopted document must stay alive through that reload.
    let output = editor.sibling_output_path("rotate");
    let staging = super::super::output::PdfOutput::new(&output).unwrap();
    std::fs::copy(&pdf_path, staging.staged_path()).unwrap();
    let (sender, receiver) = channel();
    editor.edit_rx = Some(receiver);
    editor.edit_in_progress = true;
    sender
        .send(Ok(PdfEditMsg {
            operation: "rotate",
            page_num: 1,
            result: Ok(staging),
        }))
        .unwrap();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        editor.poll_edit_worker(ctx, 0.0)
    });
    assert_eq!(editor.path, output);
    assert!(editor.path.is_file());
    assert!(editor.info_job.is_some());
    drop(editor);
    assert!(!directory.exists());

    let (pdf_path, raster) = gallery_pdf_page_source().unwrap();
    let directory = pdf_path.parent().unwrap().to_owned();
    let inline = InlinePdfPagePreview::new(Arc::clone(&raster));
    let editor = PdfEditorState::from_shared_page_raster(pdf_path.clone(), raster);
    drop(editor);
    assert!(pdf_path.exists());
    drop(inline);
    assert!(!directory.exists());
}

#[test]
fn inline_preview_decodes_after_editor_drops_and_owns_raster_until_drop() {
    let ctx = egui::Context::default();
    let state = raster_state(1, 16);
    let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
    let raster_path = raster.path().to_owned();
    let raster_directory = raster_path.parent().unwrap().to_owned();
    let mut inline = InlinePdfPagePreview::new(raster);
    drop(state);

    assert!(raster_path.exists());
    wait_for_inline_texture(&ctx, &mut inline);
    assert!(std::fs::read(&raster_path).is_ok());
    assert!(inline.texture.is_some());

    drop(inline);
    assert!(!raster_directory.exists());
}

#[test]
fn inline_preview_fits_non_square_pdf_raster_without_distortion() {
    let state = raster_state_with_dimensions(1, (100, 200));
    let raster = state.pages[0].raster.as_ref().unwrap();
    let region = egui::Rect::from_min_size(egui::Pos2::new(10.0, 20.0), Vec2::splat(100.0));

    assert_eq!(raster.dimensions(), (100, 200));
    let fitted = fitted_pdf_page_rect(region, raster.dimensions());
    assert!((fitted.width() - 50.0).abs() < 0.001);
    assert!((fitted.height() - 100.0).abs() < 0.001);
    assert_eq!(fitted.center(), region.center());
}

#[test]
fn decode_requests_are_bounded_deduplicated_and_selected_first() {
    let ctx = egui::Context::default();
    let mut state = raster_state(100, 16);
    state.selected_page = Some(100);
    for _ in 0..3 {
        for page in 1..=100 {
            assert!(state.ensure_page_texture(page, &ctx).is_none());
        }
    }
    assert_eq!(state.decode_requests.len(), PAGE_DECODE_REQUEST_LIMIT);
    assert_eq!(
        state
            .decode_requests
            .iter()
            .filter(|page| **page == 1)
            .count(),
        1
    );
    state.schedule_page_decode(&ctx);
    let job = state.decode_job.as_ref().unwrap();
    assert_eq!(job.raster.page_num(), 100);
    let revision = job.ticket.revision();
    for _ in 0..10 {
        state.schedule_page_decode(&ctx);
        assert_eq!(
            state.decode_job.as_ref().unwrap().ticket.revision(),
            revision
        );
    }
}

#[test]
fn dropping_editor_cancels_running_decode_publication() {
    let mut state = raster_state(1, 16);
    let (release, _) = blocked_decode(&mut state);
    let (_, placeholder) = channel();
    let rx = std::mem::replace(&mut state.decode_job.as_mut().unwrap().rx, placeholder);
    drop(state);
    release.send(()).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(JobError::Cancelled)
    ));
}

#[test]
fn oversized_raster_decode_bounds_published_pixel_memory() {
    let state = raster_state(1, PAGE_DETAIL_MAX_DIM + 1);
    let raster_path = state.pages[0].raster.as_ref().unwrap().path();
    let image = decode_page_png(std::fs::read(raster_path).unwrap(), PAGE_DETAIL_MAX_DIM).unwrap();
    assert_eq!(image.size, [PAGE_DETAIL_MAX_DIM as usize; 2]);
    assert_eq!(
        image.pixels.len() * 4,
        PAGE_DETAIL_MAX_DIM as usize * PAGE_DETAIL_MAX_DIM as usize * 4
    );
}

#[test]
fn replacement_raster_cancels_old_decode_before_publishing() {
    let ctx = egui::Context::default();
    let mut state = raster_state(1, 16);
    let (release, _) = blocked_decode(&mut state);
    let (tx, rx) = channel();
    state.preview_rx = Some(rx);
    let replacement_path = state
        .raster_cache
        .as_ref()
        .unwrap()
        .path()
        .join("replacement.png");
    std::fs::copy(
        state.pages[0].raster.as_ref().unwrap().path(),
        &replacement_path,
    )
    .unwrap();
    let replacement = test_page_raster(
        1,
        replacement_path.clone(),
        16,
        16,
        Arc::clone(state.raster_cache.as_ref().unwrap()),
    );
    tx.send(PdfPreviewMsg {
        page_num: 1,
        result: Ok(replacement),
    })
    .unwrap();
    state.poll_preview_worker(&ctx);
    assert!(state.decode_job.as_ref().unwrap().cancelled);
    release.send(()).unwrap();
    wait_for_decode(&mut state, 1, &ctx);
    assert!(state.ensure_page_texture(1, &ctx).is_some());
    assert!(state.pages[0].error.is_none());
    assert_eq!(
        state.pages[0].raster.as_ref().unwrap().path(),
        replacement_path
    );
}

#[test]
fn page_textures_obey_handle_and_byte_limits_while_selected_page_stays_resident() {
    let ctx = egui::Context::default();
    for dimension in [16, PAGE_RASTER_MAX_DIM] {
        let mut state = raster_state(PAGE_TEXTURE_LIMIT * 2, dimension);
        state.texture_frame += 1;
        let mut selected_id = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            selected_id = Some(upload_page(&mut state, 1, ctx));
        });
        let selected_id = selected_id.unwrap();
        for page_num in 2..=state.page_count {
            state.texture_frame += 1;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                upload_page(&mut state, page_num, ctx);
            });
            let (count, bytes) = resident_textures(&state);
            assert!(count <= PAGE_TEXTURE_LIMIT);
            assert!(bytes <= PAGE_TEXTURE_BYTE_LIMIT);
            assert_eq!(state.pages[0].texture.as_ref().unwrap().id(), selected_id);
        }
        assert!(state.pages[1].texture.is_none());
    }
}

#[test]
fn selecting_thumbnail_renders_full_page_and_reuses_it_after_eviction() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("selection.pdf");
    write_two_page_pdf(&source);
    let mut state = raster_state(2, 16);
    state.path = source;
    let ctx = egui::Context::default();
    // Page two initially has only the raster and texture used by the grid.
    Arc::get_mut(state.pages[1].raster.as_mut().unwrap())
        .unwrap()
        .render_width = 360;
    let thumbnail_path = state.pages[1].raster.as_ref().unwrap().path().to_owned();
    let thumbnail_id = upload_page(&mut state, 2, &ctx);
    assert_eq!(state.pages[1].texture.as_ref().unwrap().size(), [16, 16]);

    state.selected_page = Some(2);
    assert_eq!(state.ensure_page_texture(2, &ctx), Some(thumbnail_id));
    let full_id = upload_page(&mut state, 2, &ctx);
    assert_ne!(thumbnail_id, full_id);
    let full_raster = Arc::clone(state.pages[1].raster.as_ref().unwrap());
    assert_eq!(full_raster.render_width, PAGE_RASTER_MAX_DIM);
    assert_eq!(full_raster.dimensions(), (1200, 400));
    assert_eq!(state.pages[1].texture.as_ref().unwrap().size(), [1200, 400]);
    assert_ne!(full_raster.path(), thumbnail_path);
    assert!(thumbnail_path.exists());
    assert!(resident_textures(&state).1 <= PAGE_TEXTURE_BYTE_LIMIT);

    state.pages[1].texture = None;
    // Cached full-page pixels must survive eviction without another PDF render.
    std::fs::remove_file(&state.path).unwrap();
    let reloaded_id = upload_page(&mut state, 2, &ctx);
    assert_ne!(full_id, reloaded_id);
    assert!(Arc::ptr_eq(
        state.pages[1].raster.as_ref().unwrap(),
        &full_raster
    ));
    assert_eq!(state.pages[1].texture.as_ref().unwrap().size(), [1200, 400]);
}

#[test]
fn evicted_page_reselection_reuploads_original_raster() {
    let ctx = egui::Context::default();
    let mut state = raster_state(PAGE_TEXTURE_LIMIT + 2, 16);
    state.texture_frame += 1;
    let original_id = upload_page(&mut state, 2, &ctx);
    let source_path = state.pages[1].raster.as_ref().unwrap().path().to_owned();
    let source = std::fs::read(&source_path).unwrap();
    for page_num in 3..=state.page_count {
        state.texture_frame += 1;
        upload_page(&mut state, page_num, &ctx);
    }
    assert!(state.pages[1].texture.is_none());
    state.selected_page = Some(2);
    state.texture_frame += 1;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        let reloaded = upload_page(&mut state, 2, ctx);
        assert_ne!(reloaded, original_id);
    });
    assert_eq!(std::fs::read(source_path).unwrap(), source);
    let texture = state.pages[1].texture.as_ref().unwrap();
    assert_eq!(texture.size(), [16, 16]);
    let upload = output
        .textures_delta
        .set
        .iter()
        .find(|(id, _)| *id == texture.id())
        .unwrap();
    match &upload.1.image {
        egui::ImageData::Color(image) => {
            assert!(
                image
                    .pixels
                    .iter()
                    .all(|pixel| *pixel == egui::Color32::from_rgb(23, 45, 67))
            );
        }
        _ => panic!("PDF reload must upload the original RGBA raster"),
    }
    let (count, bytes) = resident_textures(&state);
    assert!(count <= PAGE_TEXTURE_LIMIT && bytes <= PAGE_TEXTURE_BYTE_LIMIT);
}

#[test]
fn current_frame_textures_are_not_evicted_and_next_frame_admits_new_selection() {
    let ctx = egui::Context::default();
    let mut state = raster_state(PAGE_TEXTURE_LIMIT + 1, 16);
    state.texture_frame += 1;
    let ids: Vec<_> = (1..=PAGE_TEXTURE_LIMIT)
        .map(|page_num| upload_page(&mut state, page_num, &ctx))
        .collect();
    state.selected_page = Some(PAGE_TEXTURE_LIMIT + 1);
    wait_for_decode(&mut state, PAGE_TEXTURE_LIMIT + 1, &ctx);
    assert!(
        state
            .ensure_page_texture(PAGE_TEXTURE_LIMIT + 1, &ctx)
            .is_none()
    );
    for (page, id) in state.pages.iter().zip(ids) {
        assert_eq!(page.texture.as_ref().unwrap().id(), id);
    }
    assert_eq!(resident_textures(&state).0, PAGE_TEXTURE_LIMIT);
    state.texture_frame += 1;
    assert!(
        state
            .ensure_page_texture(PAGE_TEXTURE_LIMIT + 1, &ctx)
            .is_some()
    );
    assert_eq!(resident_textures(&state).0, PAGE_TEXTURE_LIMIT);
}

#[test]
fn long_document_uploads_visible_thumbnails_and_selected_page_only() {
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut state = raster_state(100, 16);
    state.selected_page = Some(100);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1177.0, 662.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        if state.decode_job.is_none() && resident_textures(&state).0 > 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "visible decodes did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let (count, bytes) = resident_textures(&state);
    assert!(count > 1 && count < PAGE_TEXTURE_LIMIT);
    assert!(bytes <= PAGE_TEXTURE_BYTE_LIMIT);
    assert!(state.pages[99].texture.is_some());
    assert!(state.pages[50].texture.is_none());
    assert!(state.pages.iter().all(|page| page.raster.is_some()));
}

#[test]
fn pdf_detail_resolution_tracks_physical_size_and_stays_bounded() {
    let mut viewport = PdfPageViewport::default();
    viewport.update_render_width(egui::vec2(300.0, 600.0), 1.0, PAGE_DETAIL_MAX_DIM as usize);
    assert_eq!(viewport.render_width, 512);
    viewport.update_render_width(egui::vec2(300.0, 600.0), 2.0, PAGE_DETAIL_MAX_DIM as usize);
    assert_eq!(viewport.render_width, 1024);
    viewport.update_render_width(egui::vec2(600.0, 1200.0), 2.0, PAGE_DETAIL_MAX_DIM as usize);
    assert_eq!(viewport.render_width, 2048);
    viewport.update_render_width(
        egui::vec2(8000.0, 16000.0),
        2.0,
        PAGE_DETAIL_MAX_DIM as usize,
    );
    assert_eq!(viewport.render_width, PAGE_DETAIL_MAX_DIM / 2);
    viewport.update_render_width(
        egui::vec2(16000.0, 8000.0),
        2.0,
        PAGE_DETAIL_MAX_DIM as usize,
    );
    assert_eq!(viewport.render_width, PAGE_DETAIL_MAX_DIM);
}

#[test]
fn failed_page_raster_is_reported_instead_of_spinning_forever() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = PdfEditorState::new(temp.path().join("missing.pdf"));
    let (sender, receiver) = channel();
    state.preview_rx = Some(receiver);
    state.previews_loading = true;
    state.pages.push(PdfPageItem {
        page_num: 1,
        raster: None,
        texture: None,
        texture_last_used: 0,
        error: None,
    });
    sender
        .send(PdfPreviewMsg {
            page_num: 1,
            result: Err("raster failed".to_owned()),
        })
        .unwrap();
    drop(sender);

    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        state.poll_preview_worker(ctx)
    });
    assert_eq!(state.pages[0].error.as_deref(), Some("raster failed"));
    assert!(!state.previews_loading);
}
