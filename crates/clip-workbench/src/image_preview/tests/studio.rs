use super::*;

#[test]
fn image_texture_sampling_changes_when_edits_cross_the_pixel_art_threshold() {
    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    let mut state = image_state_from_png((8, 8), "Pixel sampling");
    let mut close = false;
    for (size, expected) in [
        ((8, 8), egui::TextureOptions::NEAREST),
        ((499, 499), egui::TextureOptions::NEAREST),
        ((500, 100), egui::TextureOptions::LINEAR),
        ((100, 500), egui::TextureOptions::LINEAR),
        ((64, 64), egui::TextureOptions::NEAREST),
    ] {
        state.current_rgba = Arc::new(RgbaImage::new(size.0, size.1));
        state.dirty_texture = true;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        let id = state
            .texture
            .as_ref()
            .expect("canvas texture uploaded")
            .id();
        let (_, uploaded) = output
            .textures_delta
            .set
            .iter()
            .find(|(texture_id, _)| *texture_id == id)
            .expect("updated canvas texture sent to renderer");
        assert_eq!(uploaded.options, expected, "sampling for {size:?}");
    }
}

#[test]
fn maximum_zoom_scales_with_image_and_canvas_size_within_limits() {
    let mut state = image_state_from_png((8, 6), "Zoom limits");
    let canvas = Vec2::new(1040.0, 640.0);
    for (size, expected) in [
        ((8, 6), 50.0),
        ((100, 100), 30.0),
        ((200, 100), 25.0),
        ((4000, 3000), 5.0),
    ] {
        state.current_rgba = Arc::new(RgbaImage::new(size.0, size.1));
        assert_eq!(state.maximum_zoom(canvas), expected);
    }
    state.current_rgba = Arc::new(RgbaImage::new(200, 100));
    assert_eq!(state.maximum_zoom(Vec2::new(240.0, 140.0)), 5.0);
    state.current_rgba = Arc::new(RgbaImage::new(100, 100));
    state.fit_to_view(canvas);
    assert_eq!(
        state.zoom, 6.0,
        "small images must be able to fit above 500%"
    );
    state.current_rgba = Arc::new(RgbaImage::new(1, 1));
    state.fit_to_view(canvas);
    assert_eq!(state.zoom, 50.0, "fitting must respect the 5000% ceiling");
}

#[test]
fn image_studio_footer_stays_inside_root_and_below_workspace_at_multiple_scales() {
    for (size, zoom, mode) in [
        (Vec2::new(820.0, 600.0), 1.0, ImageToolMode::View),
        (Vec2::new(1280.0, 720.0), 1.5, ImageToolMode::Markup),
        (Vec2::new(1600.0, 900.0), 2.0, ImageToolMode::View),
    ] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        ctx.set_zoom_factor(zoom);
        let mut state = image_state_from_png((80, 60), "Footer layout test");
        state.active_mode = mode;
        let palette = AppPalette::default();
        let mut root_rect = None;
        let mut close_requested = false;

        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    root_rect = Some(ui.max_rect());
                    state.render_ui(ui, ctx, &palette, &mut close_requested);
                });
            },
        );

        let root = root_rect.expect("root layout should render");
        let footer_hint = image_studio_footer_hint(mode);
        let footer = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == footer_hint => {
                    Some(Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("footer hint should paint");
        let viewport = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Rect(rect)
                    if rect.fill == crate::hex_to_c32(&palette.colors.canvas)
                        && rect.rounding == Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("viewport canvas should paint");
        assert!(root.contains(footer.min) && root.contains(footer.max));
        let control_bounds = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing footer control {label}"))
        };
        let reset = control_bounds("Reset");
        let pixel = control_bounds("1:1");
        let percentage = control_bounds("100%");
        for bounds in [reset, pixel, percentage] {
            assert!(root.contains(bounds.min) && root.contains(bounds.max));
            assert!(
                bounds.top() >= viewport.bottom(),
                "zoom controls belong below the canvas"
            );
        }
        let controls = reset.union(pixel).union(percentage);
        assert!(
            (controls.center().x - root.center().x).abs() < 12.0,
            "zoom controls must be centered: {controls:?} in {root:?}"
        );
        assert!(!output.shapes.iter().any(|clipped| matches!(&clipped.shape,
            egui::epaint::Shape::Text(text) if matches!(text.galley.text(), "Fit" | "+" | "-"))));

        assert!(root.contains(viewport.min) && root.contains(viewport.max));
        assert!(viewport.max.y < footer.min.y);
    }
}

#[test]
fn image_editor_hatch_keeps_configured_command_and_fallback_help_together() {
    let hatch = image_editor_hatch("Inkscape", "inkscape", "/missing/path/inkscape");

    assert_eq!(hatch.label, "Inkscape");
    assert_eq!(hatch.command, "/missing/path/inkscape");
    assert!(
        hatch
            .tooltip
            .contains("/missing/path/inkscape is unavailable")
    );
    assert!(hatch.tooltip.contains("system default viewer"));
}

#[test]
fn image_editor_hatch_displays_custom_executable_name_and_target() {
    let hatch = image_editor_hatch("Inkscape", "inkscape", "/opt/custom-tools/vector-editor");

    assert_eq!(hatch.label, "vector-editor");
    assert_eq!(hatch.command, "/opt/custom-tools/vector-editor");
    assert!(
        hatch
            .tooltip
            .contains("with /opt/custom-tools/vector-editor")
    );
}

#[test]
fn save_dialog_owns_image_shortcuts_until_dismissed() {
    let mut state = image_state_from_png((100, 100), "Test image");
    state.active_mode = ImageToolMode::Crop;
    state.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.6, 0.6));
    state.undo_stack.push(state.current_rgba.clone());
    state.show_fallback_save_modal = true;
    state.fallback_save_needs_focus = true;
    state.fallback_save_path = "test.png".into();

    fn key_frame(
        state: &mut ImagePreviewState,
        ctx: &egui::Context,
        palette: &AppPalette,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, palette, &mut close);
                });
            },
        );
        close
    }

    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    assert!(!key_frame(
        &mut state,
        &ctx,
        &palette,
        egui::Key::F12,
        egui::Modifiers::NONE
    ));
    assert!(ctx.wants_keyboard_input());
    assert!(!key_frame(
        &mut state,
        &ctx,
        &palette,
        egui::Key::Enter,
        egui::Modifiers::NONE
    ));
    assert_eq!(state.current_rgba.dimensions(), (100, 100));
    assert!(!key_frame(
        &mut state,
        &ctx,
        &palette,
        egui::Key::Z,
        egui::Modifiers::COMMAND
    ));
    assert_eq!(state.undo_stack.len(), 1);
    assert!(!key_frame(
        &mut state,
        &ctx,
        &palette,
        egui::Key::V,
        egui::Modifiers::NONE
    ));
    assert_eq!(state.active_mode, ImageToolMode::Crop);
    assert!(!key_frame(
        &mut state,
        &ctx,
        &palette,
        egui::Key::Escape,
        egui::Modifiers::NONE
    ));
    assert!(!state.show_fallback_save_modal);
    assert_eq!(state.crop_state.normalized_rect.min, Pos2::new(0.1, 0.1));
    assert!(!key_frame(
        &mut state,
        &ctx,
        &palette,
        egui::Key::V,
        egui::Modifiers::NONE
    ));
    assert_eq!(state.active_mode, ImageToolMode::View);
}

#[test]
fn unmodified_v_shortcut_selects_view_tool() {
    let mut state = image_state_from_png((100, 100), "View shortcut fixture");
    state.active_mode = ImageToolMode::Crop;
    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    let mut close = false;

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );

    assert_eq!(state.active_mode, ImageToolMode::View);
    assert!(!close, "selecting a tool does not close Image Studio");
}

#[test]
fn crop_enter_applies_selected_region_through_rendered_shortcut() {
    let mut state = image_state_from_png((100, 100), "Crop Enter fixture");
    state.active_mode = ImageToolMode::Crop;
    state.crop_state.normalized_rect = Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.6, 0.7));
    let palette = AppPalette::default();
    let mut close = false;

    let _ = egui::Context::default().run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );

    assert_eq!(state.current_rgba.dimensions(), (50, 50));
    assert_eq!(state.undo_stack.len(), 1);
    assert!(!close, "applying Crop must not close Image Studio");
}

#[test]
fn redact_escape_deselects_region_without_burning_or_closing() {
    let mut state = image_state_from_png((100, 100), "Redact Escape fixture");
    state.current_rgba = Arc::new(RgbaImage::from_pixel(
        100,
        100,
        image::Rgba([80, 120, 160, 255]),
    ));
    state.active_mode = ImageToolMode::Redact;
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.8, 0.8)),
        style: RedactionStyle::Black,
    });
    state.redaction_state.selected_idx = Some(0);
    let pixels_before = state.current_rgba.clone();
    let palette = AppPalette::default();
    let mut close = false;

    let _ = egui::Context::default().run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );

    assert_eq!(state.redaction_state.boxes.len(), 1);
    assert!(state.redaction_state.selected_idx.is_none());
    assert_eq!(state.current_rgba.as_ref(), pixels_before.as_ref());
    assert!(
        !close,
        "Escape deselects the redaction without closing the studio"
    );
}

#[test]
fn save_dialogs_use_the_in_workbench_modal_and_keep_default_extensions() {
    let mut state = image_state_from_png((100, 100), "Test image");
    state.path = Some(PathBuf::from("/tmp/source.png"));

    state.open_save_dialog();
    assert!(state.show_fallback_save_modal);
    assert_eq!(state.fallback_save_path, "/tmp/source.png");

    state.open_save_dialog_with_ext("webp");
    assert!(state.show_fallback_save_modal);
    assert_eq!(state.fallback_save_path, "/tmp/source_converted.webp");
}
