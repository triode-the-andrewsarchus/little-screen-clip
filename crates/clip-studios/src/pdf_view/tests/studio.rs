use super::*;

#[test]
fn pdf_selection_and_error_labels_meet_contrast_in_both_appearances() {
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = crate::ui_appearance::palette(appearance);
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
        for (surface, preferred) in [
            (colors.panel, colors.accent),
            (colors.recessed, colors.danger),
        ] {
            let foreground =
                crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), &palette);
            assert!(
                little_clip_core::palette::contrast_ratio(
                    (surface.r(), surface.g(), surface.b()),
                    (foreground.r(), foreground.g(), foreground.b()),
                ) >= 4.5,
                "{appearance:?} PDF custom label lacks contrast"
            );
        }
    }
}

#[test]
fn pdf_sibling_panes_stay_inside_compact_and_wide_hosts() {
    for host_size in [Vec2::new(1177.0, 662.0), Vec2::new(1766.0, 993.0)] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
        let mut state = raster_state(0, 16);
        for right in [
            None,
            Some(navigation::RightTab::Find),
            Some(navigation::RightTab::Ocr),
        ] {
            state.navigation.right = right;
            let host = egui::Rect::from_min_size(egui::Pos2::ZERO, host_size);
            let mut panes = Vec::new();
            let mut pane_width_budget = 0.0;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(host),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        pane_width_budget = ui.available_width()
                            - 24.0
                            - (ui.spacing().item_spacing.x * 2.0 + 1.0)
                                * if right.is_some() { 2.0 } else { 1.0 };
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Rect(rect) = &clipped.shape {
                    if rect.fill == colors.recessed && rect.rect.height() > 100.0 {
                        panes.push(rect.rect);
                    }
                }
            }
            panes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
            assert_eq!(
                panes.len(),
                if right.is_some() { 3 } else { 2 },
                "expected docked pane frames at {host_size:?}"
            );
            assert!(
                panes
                    .windows(2)
                    .all(|pair| pair[0].right() < pair[1].left()),
                "panes overlap: {panes:?}"
            );
            assert!(panes.iter().all(|pane| host.contains_rect(*pane)));
            let pane_widths = panes.iter().map(|pane| pane.width()).sum::<f32>();
            assert!(
                (pane_widths - pane_width_budget).abs() < 1.0,
                "panes do not fill their divider and gap-adjusted budget: {pane_widths} vs {pane_width_budget}"
            );
        }
    }
}

#[test]
fn selected_pdf_page_preview_stays_inside_compact_and_wide_panes() {
    for host_size in [Vec2::new(1177.0, 662.0), Vec2::new(1766.0, 993.0)] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
        let mut state = raster_state(2, 16);
        for page in &mut state.pages {
            let raster = page.raster.as_ref().unwrap();
            page.raster = Some(test_page_raster(
                page.page_num,
                raster.path().to_owned(),
                1200,
                800,
                Arc::clone(state.raster_cache.as_ref().unwrap()),
            ));
        }
        let texture = ctx.load_texture(
            "pdf-preview-fit-test",
            egui::ColorImage::new([16, 16], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let texture_id = texture.id();
        state.pages[0].texture = Some(texture);

        let host = egui::Rect::from_min_size(egui::Pos2::ZERO, host_size);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(host),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );

        let mut panes = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Rect(rect)
                    if rect.fill == colors.recessed && rect.rect.height() > 100.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        panes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        let preview_pane = *panes.get(1).expect("right preview pane should render");
        let preview_rect = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Rect(rect) if rect.fill_texture_id == texture_id => {
                    Some(rect.rect)
                }
                egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture_id => Some(
                    mesh.vertices
                        .iter()
                        .fold(egui::Rect::NOTHING, |mut bounds, vertex| {
                            bounds.extend_with(vertex.pos);
                            bounds
                        }),
                ),
                _ => None,
            })
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .expect("selected page texture should be painted");
        assert!(
            host.contains_rect(preview_pane),
            "selected PDF preview pane escapes host at {host_size:?}: pane={preview_pane:?}"
        );
        assert!(
            preview_pane.contains_rect(preview_rect),
            "selected page preview escapes its pane at {host_size:?}: page={preview_rect:?}, pane={preview_pane:?}"
        );
    }
}

#[test]
fn zoom_detail_replaces_texture_without_blanking_or_resampling_pdf_text() {
    let (source, raster) = gallery_pdf_page_source().unwrap();
    let mut state = PdfEditorState::from_shared_page_raster(source, raster);
    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            max_texture_side: Some(4096),
            ..Default::default()
        },
        |_| {},
    );
    let base_id = upload_page(&mut state, 1, &ctx);
    // The portrait raster must retain its native height instead of being reduced to 1200.
    assert_eq!(
        state.pages[0].texture.as_ref().unwrap().size(),
        [1200, 1600]
    );
    let base_raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());

    state.viewport.select_page(1);
    state.viewport.zoom = 2.0;
    state.viewport.pan = egui::vec2(30.0, 50.0);
    state.viewport.update_render_width(
        egui::vec2(750.0, 1000.0),
        2.0,
        PAGE_DETAIL_MAX_DIM as usize,
    );
    assert_eq!(state.viewport.render_width, 2304);
    assert_eq!(state.ensure_page_texture(1, &ctx), Some(base_id));
    let detail_id = upload_page(&mut state, 1, &ctx);
    assert_ne!(base_id, detail_id);
    assert_eq!(
        state.pages[0].texture.as_ref().unwrap().size(),
        [2304, 3072]
    );
    let detail_raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
    assert_eq!(detail_raster.render_width, 2304);
    assert_ne!(base_raster.path(), detail_raster.path());
    assert!(
        base_raster.path().exists(),
        "replacement must preserve readers' original pixels"
    );
    assert_eq!(state.viewport.zoom, 2.0);
    assert_eq!(state.viewport.pan, egui::vec2(30.0, 50.0));
    assert!(resident_textures(&state).1 <= PAGE_TEXTURE_BYTE_LIMIT);

    state.viewport.update_render_width(
        egui::vec2(250.0, 1000.0 / 3.0),
        1.0,
        PAGE_DETAIL_MAX_DIM as usize,
    );
    assert_eq!(state.ensure_page_texture(1, &ctx), Some(detail_id));
    let reduced_id = upload_page(&mut state, 1, &ctx);
    assert_ne!(detail_id, reduced_id);
    assert_eq!(state.pages[0].raster.as_ref().unwrap().render_width, 512);
    assert_eq!(state.pages[0].texture.as_ref().unwrap().size(), [512, 683]);

    // A failed quality upgrade must preserve the usable texture and stop retrying.
    std::fs::remove_file(&state.path).unwrap();
    state.viewport.update_render_width(
        egui::vec2(750.0, 1000.0),
        2.0,
        PAGE_DETAIL_MAX_DIM as usize,
    );
    assert_eq!(state.ensure_page_texture(1, &ctx), Some(reduced_id));
    wait_for_decode(&mut state, 1, &ctx);
    assert!(state.pages[0].error.is_some());
    assert_eq!(state.ensure_page_texture(1, &ctx), Some(reduced_id));
    state.schedule_page_decode(&ctx);
    assert!(state.decode_job.is_none());
}

#[test]
fn page_pan_is_limited_to_overflowing_axes_and_cleared_when_zoomed_out() {
    let viewport = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), Vec2::splat(400.0));
    for (dimensions, zoom, axes) in [
        ((200, 400), 0.5, (false, false)),
        ((200, 400), 1.0, (false, false)),
        ((200, 400), 1.5, (false, true)),
        ((400, 200), 1.5, (true, false)),
        ((200, 400), 4.0, (true, true)),
        ((400, 200), 4.0, (true, true)),
    ] {
        for direction in [-1.0, 1.0] {
            let mut view = PdfPageViewport {
                zoom,
                pan: Vec2::splat(direction * 10_000.0),
                ..Default::default()
            };
            let page = view.page_rect(viewport, dimensions);
            assert_eq!(view.pan.x != 0.0, axes.0, "{dimensions:?} at {zoom}");
            assert_eq!(view.pan.y != 0.0, axes.1, "{dimensions:?} at {zoom}");
            // An overflowing axis always covers the viewport, even at the pan limit.
            if axes.0 {
                assert!(page.left() <= viewport.left() && page.right() >= viewport.right());
            }
            if axes.1 {
                assert!(page.top() <= viewport.top() && page.bottom() >= viewport.bottom());
            }
            view.zoom = 0.5;
            let page = view.page_rect(viewport, dimensions);
            assert_eq!(view.pan, Vec2::ZERO);
            assert!(viewport.contains_rect(page));
        }
    }
}

#[test]
fn pdf_viewport_wheel_zoom_drag_and_double_click_use_constrained_canvas() {
    let ctx = egui::Context::default();
    let texture = ctx.load_texture(
        "pdf-navigation",
        egui::ColorImage::example(),
        Default::default(),
    );
    let mut view = PdfPageViewport::default();
    let mut time = 0.0;
    let mut frame = |view: &mut PdfPageViewport, events| {
        time += 0.05;
        let mut rect = egui::Rect::NOTHING;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::splat(400.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = view.show(ui, texture.id(), (200, 400)).rect;
                });
            },
        );
        (rect, output)
    };
    let (rect, _) = frame(&mut view, vec![]);
    let center = rect.center();
    // Zoom around the center to overflow only vertically.
    frame(
        &mut view,
        vec![
            egui::Event::PointerMoved(center),
            egui::Event::Scroll(egui::vec2(0.0, 200.0)),
        ],
    );
    assert!(view.zoom > 1.0 && view.zoom < 2.0);
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&mut view, vec![button(center, true)]);
    let (rect, output) = frame(
        &mut view,
        vec![egui::Event::PointerMoved(center + egui::vec2(40.0, 40.0))],
    );
    assert_eq!(view.pan.x, 0.0);
    assert!(view.pan.y > 0.0);
    // The zoomed page is painted inside a fixed viewport clip.
    let image = output
        .shapes
        .iter()
        .find(|shape| {
            matches!(&shape.shape,
            egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id())
        })
        .unwrap();
    assert!(rect.contains_rect(image.clip_rect));
    let end = center + egui::vec2(40.0, 40.0);
    frame(&mut view, vec![button(end, false)]);
    frame(&mut view, vec![button(end, true)]);
    frame(&mut view, vec![button(end, false)]);
    frame(&mut view, vec![button(end, true)]);
    frame(&mut view, vec![button(end, false)]);
    assert_eq!(view.zoom, 1.0);
    assert_eq!(view.pan, Vec2::ZERO);

    // With both axes available, wheel zoom preserves the page point under the pointer.
    view.zoom = 3.0;
    let pointer = center + egui::vec2(20.0, -30.0);
    let before = view.page_rect(rect, (200, 400));
    let source_point = (pointer - before.min) / before.size();
    frame(
        &mut view,
        vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::Scroll(egui::vec2(0.0, 100.0)),
        ],
    );
    let after = view.page_rect(rect, (200, 400));
    assert!(((pointer - after.min) / after.size() - source_point).length() < 0.0001);
    frame(
        &mut view,
        vec![egui::Event::Scroll(egui::vec2(0.0, -2000.0))],
    );
    assert_eq!(view.zoom, 0.25);
    assert_eq!(view.pan, Vec2::ZERO);
}

#[test]
fn selecting_another_page_resets_pdf_zoom_and_pan() {
    let mut view = PdfPageViewport::default();
    view.select_page(1);
    view.zoom = 3.0;
    view.pan = egui::vec2(30.0, 50.0);
    view.select_page(1);
    assert_eq!(view.zoom, 3.0);
    view.select_page(2);
    assert_eq!(view.zoom, 1.0);
    assert_eq!(view.pan, Vec2::ZERO);
}
