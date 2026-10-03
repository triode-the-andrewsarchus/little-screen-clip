use super::*;

#[test]
fn audio_studio_space_starts_only_idle_empty_stem_separation_without_editor_focus() {
    let can_start = |space, shortcuts_enabled, mode, no_stems, in_progress| {
        should_start_stem_separation_on_space(space, shortcuts_enabled, mode, no_stems, in_progress)
    };
    assert!(can_start(
        true,
        true,
        AudioToolMode::StemSeparation,
        true,
        false,
    ));
    assert!(
        !can_start(true, false, AudioToolMode::StemSeparation, true, false,),
        "focused editor or save dialog owns Space"
    );
    assert!(!can_start(true, true, AudioToolMode::Inspect, true, false,));
    assert!(!can_start(
        true,
        true,
        AudioToolMode::StemSeparation,
        false,
        false,
    ));
    assert!(!can_start(
        true,
        true,
        AudioToolMode::StemSeparation,
        true,
        true,
    ));
}

#[test]
fn stopping_synthetic_audio_resets_playback_to_the_active_selection_start() {
    let (_temp_dir, path) = generate_fixture_wav("escape-stop", 0.2);
    let mut state = AudioPreviewState::new(path, "escape-stop.wav".to_owned());
    state.cancel_session_analysis();
    state.selection_mode = SelectionMode::Single;
    state.selection = Some((0.05, 0.15));
    state.current_time_sec = 0.18;
    state.playback_start_instant = Some(Instant::now());
    state.is_playing = true;

    state.stop();

    assert!(!state.is_playing);
    assert!(state.playback_start_instant.is_none());
    assert_eq!(state.current_time_sec, 0.05);
}

#[test]
fn stem_space_dispatches_one_start_for_idle_empty_state() {
    let (_temp_dir, path) = generate_fixture_wav("stem-space-start", 1.0);
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.active_mode = AudioToolMode::StemSeparation;
    assert!(state.stem_separation.stems.is_empty());
    assert!(!state.stem_separation.in_progress);

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let mut start_requests = 0;
    let mut close_requested = false;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
            events: vec![egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |_ui| {
                let closed =
                    state.handle_studio_shortcuts_with_start(ctx, &mut close_requested, |_| {
                        start_requests += 1
                    });
                assert!(!closed);
            });
        },
    );

    assert_eq!(start_requests, 1);
    assert!(!close_requested);
    assert!(state.stem_separation.stems.is_empty());
    assert!(!state.stem_separation.in_progress);
}

#[test]
fn inspect_space_starts_playback_with_a_silent_player() {
    let (_temp_dir, path) = generate_fixture_wav("inspect-space-start", 1.0);
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.is_loading_analysis = false;
    state.duration_sec = 1.0;
    // Drain the test output queue without opening an audio device.
    let (silent_player, mut silent_output) = Player::new();
    let stop_silent_output = Arc::new(AtomicBool::new(false));
    let stop_silent_output_thread = Arc::clone(&stop_silent_output);
    let silent_output_thread = std::thread::spawn(move || {
        while !stop_silent_output_thread.load(Ordering::Relaxed) {
            for _ in 0..256 {
                let _ = silent_output.next();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    state.player = Some(silent_player);
    state._sink = None;
    assert!(!state.is_playing);

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    render_inspect_frame(
        &ctx,
        &mut state,
        0.01,
        vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    assert!(state.is_playing, "Space should start idle Inspect playback");
    assert!(state.playback_start_instant.is_some());
    state.stop();
    stop_silent_output.store(true, Ordering::Relaxed);
    silent_output_thread.join().expect("silent output thread");
}

#[test]
fn audio_studio_space_yields_to_focused_text_editor() {
    let ctx = egui::Context::default();
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_audio_space_focus__.wav"),
        "test.wav".to_string(),
    );
    state.active_mode = AudioToolMode::StemSeparation;
    let mut text = String::new();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text))
                .request_focus();
        });
    });
    assert!(ctx.wants_keyboard_input());

    let mut close_requested = false;
    let mut start_requests = 0;
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            state.handle_studio_shortcuts_with_start(ctx, &mut close_requested, |_| {
                start_requests += 1
            });
        },
    );
    assert_eq!(start_requests, 0);
    assert!(!state.stem_separation.in_progress);
    assert!(!close_requested);
}

#[test]
fn escape_cancels_nested_save_dialogs_before_closing_audio_studio() {
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_audio_dialog__.wav"),
        "test.wav".to_string(),
    );
    state.save_dialog_open = true;

    let escape = || egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let mut close_requested = false;
    let _ = ctx.run(escape(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.render_ui(ui, ctx, &palette, &mut close_requested);
        });
    });
    assert!(!close_requested);
    assert!(!state.save_dialog_open);

    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::H,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close_requested);
            });
        },
    );
    assert!(!close_requested);
    assert_eq!(state.active_mode, AudioToolMode::HfAnalysis);

    state.active_mode = AudioToolMode::StemSeparation;
    state.stem_separation.save_dialog_open = true;
    let _ = ctx.run(escape(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.render_ui(ui, ctx, &palette, &mut close_requested);
        });
    });
    assert!(!close_requested);
    assert!(!state.stem_separation.save_dialog_open);

    let _ = ctx.run(escape(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.render_ui(ui, ctx, &palette, &mut close_requested);
        });
    });
    assert!(close_requested);
}

#[test]
fn audio_tool_shortcuts_switch_modes_and_ignore_modified_keys() {
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_audio_shortcuts__.wav"),
        "test.wav".to_string(),
    );
    let mut close_requested = false;

    for (key, modifiers, expected_mode) in [
        (
            egui::Key::H,
            egui::Modifiers::NONE,
            AudioToolMode::HfAnalysis,
        ),
        (
            egui::Key::D,
            egui::Modifiers::CTRL,
            AudioToolMode::HfAnalysis,
        ),
        (
            egui::Key::D,
            egui::Modifiers::NONE,
            AudioToolMode::DigitalDecode,
        ),
        (
            egui::Key::S,
            egui::Modifiers::NONE,
            AudioToolMode::StemSeparation,
        ),
        (egui::Key::I, egui::Modifiers::NONE, AudioToolMode::Inspect),
    ] {
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
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
                state.handle_studio_shortcuts(ctx, &mut close_requested);
            },
        );
        assert_eq!(state.active_mode, expected_mode);
        assert!(!close_requested);
    }
}

#[test]
fn audio_tool_shortcuts_yield_to_focused_text_editor() {
    let ctx = egui::Context::default();
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_audio_focus__.wav"),
        "test.wav".to_string(),
    );
    state.save_dialog_open = true;
    let mut close_requested = false;
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::H,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            state.handle_studio_shortcuts(ctx, &mut close_requested);
        },
    );
    assert_eq!(state.active_mode, AudioToolMode::Inspect);

    state.save_dialog_open = false;
    let mut edit_value = String::new();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add(egui::TextEdit::singleline(&mut edit_value))
                .request_focus();
        });
    });
    assert!(ctx.wants_keyboard_input());

    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::H,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            state.handle_studio_shortcuts(ctx, &mut close_requested);
        },
    );
    assert_eq!(state.active_mode, AudioToolMode::Inspect);
    assert!(!close_requested);
}

#[test]
fn pending_audio_folder_browse_requests_repaint_and_updates_destination() {
    let (_temp_dir, path) = generate_fixture_wav("audio-folder-browse", 0.25);
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_owned());
    state.analysis_rx = None;
    state.save_dialog_open = true;
    state.save_destination_dir = "/previous/destination".to_owned();

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut close_requested = false;
    let mut render = |state: &mut AudioPreviewState, ctx: &egui::Context| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_standard_ui(ui, ctx, &palette, &mut close_requested, None);
                });
            },
        )
    };

    let _ = render(&mut state, &ctx);
    let (tx, rx) = channel();
    state.browse_rx = Some(rx);
    let output = render(&mut state, &ctx);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay <= Duration::from_millis(50),
        "pending folder browse should keep a frame scheduled to poll its receiver"
    );

    let destination_dir = tempfile::tempdir().unwrap();
    let destination = destination_dir.path().to_path_buf();
    tx.send(Some(destination.to_string_lossy().into_owned()))
        .unwrap();
    let _ = render(&mut state, &ctx);
    assert_eq!(state.save_destination_dir, destination.to_string_lossy());
}

#[test]
fn enter_saves_edited_audio_fields_while_destination_or_filename_is_focused() {
    for (focused_id, filename) in [
        (
            egui::Id::new("audio_save_destination"),
            "edited_with_destination_focus.wav",
        ),
        (
            egui::Id::new(("audio_save_filename", 0usize)),
            "edited_with_filename_focus.wav",
        ),
    ] {
        let (_source_dir, source_path) = generate_fixture_wav("enter-save", 1.0);
        let output_dir = tempfile::tempdir().unwrap();
        let mut state = AudioPreviewState::new(source_path, "enter-save.wav".to_owned());
        state.selection = Some((0.0, 1.0));
        state.open_save_dialog();
        state.save_destination_dir = output_dir.path().to_string_lossy().into_owned();
        state.save_filenames = vec![filename.to_owned()];
        state.open_folder_after_save = false;

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.render_save_dialog(ctx, &palette, 1.0);
            ctx.memory_mut(|memory| memory.request_focus(focused_id));
        });
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.render_save_dialog(ctx, &palette, 1.0);
        });
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused_id));
        assert!(
            ctx.wants_keyboard_input(),
            "the selected save field should own focus before Enter"
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| state.render_save_dialog(ctx, &palette, 1.0),
        );
        wait_for_audio_save(&mut state);

        assert!(
            !state.save_dialog_open,
            "Enter submits while {focused_id:?} owns focus"
        );
        assert!(
            output_dir.path().join(filename).is_file(),
            "Enter should save the edited destination and filename"
        );
    }
}

#[test]
fn inspect_footer_stays_after_canvas_in_full_and_embedded_views() {
    use egui::epaint::Shape;

    fn render_case(
        width: f32,
        height: f32,
        embedded: bool,
        ready: bool,
        zoom: f32,
    ) -> (egui::FullOutput, Rect, Option<(Rect, f32)>) {
        let (_temp_dir, path) = generate_fixture_wav("inspect-layout", 0.25);
        let mut state = AudioPreviewState::new(path.clone(), "fixture.wav".to_string());
        state.analysis_rx = None;
        if ready {
            state.analysis = Some(Arc::new(compute_full_audio_analysis(&path, 100).unwrap()));
            state.is_loading_analysis = false;
        }

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        ctx.set_zoom_factor(zoom);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let host = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height));
        let mut close_requested = false;
        let mut scroll_viewport = None;
        let mut actual_host = host;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(host),
                ..Default::default()
            },
            |ctx| {
                actual_host = ctx.screen_rect();
                egui::CentralPanel::default().show(ctx, |ui| {
                    if embedded {
                        let output = egui::ScrollArea::vertical().show(ui, |ui| {
                            state.render_standard_ui(
                                ui,
                                ctx,
                                &palette,
                                &mut close_requested,
                                Some(350.0),
                            );
                        });
                        scroll_viewport = Some((output.inner_rect, output.content_size.y));
                    } else {
                        state.render_standard_ui(ui, ctx, &palette, &mut close_requested, None);
                    }
                });
            },
        );
        (output, actual_host, scroll_viewport)
    }

    for (width, height, embedded, ready, zoom) in [
        (820.0, 680.0, false, false, 1.0),
        (1180.0, 760.0, false, true, 1.25),
        (900.0, 360.0, true, false, 1.0),
        (1180.0, 380.0, true, true, 1.25),
    ] {
        let (output, host, scroll_viewport) = render_case(width, height, embedded, ready, zoom);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let canvas = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Rect(rect)
                    if rect.fill == hex_to_c32(&palette.colors.surface_recessed)
                        && rect.rounding == egui::Rounding::same(8.0)
                        && rect.rect.width() > 500.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("inspect canvas frame should render");
        let (footer_y, footer_height) = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(text) if text.galley.text().contains("Navigation:") => {
                    Some((text.pos.y, text.galley.size().y))
                }
                _ => None,
            })
            .expect("inspect footer caption should render");

        assert!(
            canvas.min.x >= host.min.x - 1.0 && canvas.max.x <= host.max.x + 1.0,
            "canvas {canvas:?} is outside host {host:?} (embedded={embedded}, ready={ready}, zoom={zoom})"
        );
        assert!(
            canvas.max.y <= footer_y,
            "canvas and footer must not overlap"
        );
        if let Some((viewport, content_height)) = scroll_viewport {
            assert!(canvas.min.x >= viewport.min.x && canvas.max.x <= viewport.max.x);
            assert!(canvas.min.y >= viewport.min.y);
            assert!(footer_y + footer_height <= viewport.min.y + content_height + 1.0);
        } else {
            assert!(canvas.min.y >= host.min.y && canvas.max.y <= host.max.y);
            assert!(footer_y + footer_height <= host.max.y);
        }
    }
}

#[test]
fn inspect_space_pauses_active_transport_with_player_disabled() {
    let (_temp_dir, path) = generate_fixture_wav("inspect-space-pause", 1.0);
    let analysis = compute_full_audio_analysis(&path, 100).expect("fixture analysis");
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(analysis));
    state.duration_sec = 1.0;
    state.view_duration_sec = 1.0;
    state.is_loading_analysis = false;
    state.player = None;
    state.player_factory = || None;
    state._sink = None;
    // Model an already-active transport; with the player disabled, Space can
    // only exercise the production pause transition and cannot emit sound.
    state.is_playing = true;
    state.playback_start_instant = Some(Instant::now());

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    render_inspect_frame(
        &ctx,
        &mut state,
        0.0,
        vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    assert!(!state.is_playing);
    assert!(state.playback_start_instant.is_none());
    assert!(state.player.is_none());
    assert!(state._sink.is_none());
}

#[test]
fn hf_space_yields_to_focused_text_editor_with_player_disabled() {
    let (_temp_dir, path) = generate_fixture_wav("hf-space-focus", 1.0);
    let analysis = compute_full_audio_analysis(&path, 100).expect("fixture analysis");
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(analysis));
    state.duration_sec = 1.0;
    state.view_duration_sec = 1.0;
    state.is_loading_analysis = false;
    state.active_mode = AudioToolMode::HfAnalysis;
    state.player = None;
    state.player_factory = || None;
    state._sink = None;
    // An active transport makes an erroneously handled Space observable as a pause.
    // With playback resources disabled, the test cannot emit audio.
    state.is_playing = true;
    state.playback_start_instant = None;

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut editor_text = String::new();
    let mut close_requested = false;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
            events: vec![egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut editor_text))
                    .request_focus();
                assert!(ctx.wants_keyboard_input());
                state.render_ui(ui, ctx, &palette, &mut close_requested);
            });
        },
    );

    assert!(state.is_playing, "focused Space should not pause playback");
    assert!(!close_requested);
    assert!(state.player.is_none());
    assert!(state._sink.is_none());
}

#[test]
fn inspect_canvas_drag_selects_region_and_ruler_click_only_scrubs() {
    let (_temp_dir, path) = generate_fixture_wav("inspect-pointer", 5.0);
    let analysis = compute_full_audio_analysis(&path, 100).expect("fixture analysis");
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(analysis));
    state.duration_sec = 5.0;
    state.view_duration_sec = 5.0;
    state.is_loading_analysis = false;
    state.selection_mode = SelectionMode::Multi;

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let canvas = render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    let track_min_x = canvas.min.x + 48.0;
    let track_width = canvas.width() - 54.0;

    let scrub = Pos2::new(track_min_x + track_width * 0.8, canvas.min.y + 8.0);
    render_inspect_frame(
        &ctx,
        &mut state,
        0.01,
        vec![
            egui::Event::PointerMoved(scrub),
            pointer_button_event(
                scrub,
                egui::PointerButton::Primary,
                true,
                egui::Modifiers::NONE,
            ),
        ],
    );
    render_inspect_frame(
        &ctx,
        &mut state,
        0.02,
        vec![pointer_button_event(
            scrub,
            egui::PointerButton::Primary,
            false,
            egui::Modifiers::NONE,
        )],
    );
    assert!(
        state.multi_selections.is_empty(),
        "ruler click created a region"
    );
    assert!((state.current_time_sec - 4.0).abs() < 0.15);

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let canvas = render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    let track_min_x = canvas.min.x + 48.0;
    let track_width = canvas.width() - 54.0;
    let wave_y = canvas.min.y + 80.0;
    let start = Pos2::new(track_min_x + track_width * 0.2, wave_y);
    let end = Pos2::new(track_min_x + track_width * 0.6, wave_y);

    render_inspect_frame(
        &ctx,
        &mut state,
        0.01,
        vec![
            egui::Event::PointerMoved(start),
            pointer_button_event(
                start,
                egui::PointerButton::Primary,
                true,
                egui::Modifiers::NONE,
            ),
        ],
    );
    let drag_start = start + Vec2::new(12.0, 0.0);
    render_inspect_frame(
        &ctx,
        &mut state,
        0.02,
        vec![egui::Event::PointerMoved(drag_start)],
    );
    render_inspect_frame(&ctx, &mut state, 0.03, vec![egui::Event::PointerMoved(end)]);
    render_inspect_frame(
        &ctx,
        &mut state,
        0.04,
        vec![pointer_button_event(
            end,
            egui::PointerButton::Primary,
            false,
            egui::Modifiers::NONE,
        )],
    );

    assert_eq!(state.multi_selections.len(), 1);
    assert_eq!(state.active_multi_index, Some(0));
    let (selection_start, selection_end) = state.multi_selections[0];
    assert!(
        (selection_start - 1.0).abs() < 0.2,
        "selection={:?}",
        state.multi_selections
    );
    assert!(
        (selection_end - 3.0).abs() < 0.15,
        "selection={:?}",
        state.multi_selections
    );
    assert!((state.current_time_sec - 3.0).abs() < 0.15);
}

#[test]
fn inspect_canvas_effect_gestures_only_edit_the_hit_multi_region() {
    fn wheel_event(delta: f32, modifiers: egui::Modifiers) -> egui::Event {
        if modifiers.shift {
            egui::Event::Scroll(Vec2::new(0.0, delta))
        } else {
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, delta),
                modifiers,
            }
        }
    }

    let (_temp_dir, path) = generate_fixture_wav("inspect-effects", 5.0);
    let analysis = compute_full_audio_analysis(&path, 100).expect("fixture analysis");
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(analysis));
    state.duration_sec = 5.0;
    state.view_duration_sec = 5.0;
    state.is_loading_analysis = false;
    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
    state.active_multi_index = Some(0);
    state.region_gain_db = vec![6.0, -6.0];
    state.region_low_pass_hz = vec![Some(1_000.0), Some(2_000.0)];
    state.region_high_pass_hz = vec![Some(80.0), Some(120.0)];
    state.region_crossfade_sec = vec![0.1, 0.2];

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let canvas = render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    let track = Rect::from_min_max(
        Pos2::new(canvas.min.x + 48.0, canvas.min.y),
        Pos2::new(canvas.max.x - 6.0, canvas.max.y),
    );
    let [_, wave, spec, _, _] = layout::AudioWorkspace::inspect_timeline_rects(track);
    let x_at = |time: f32| track.min.x + track.width() * time / state.duration_sec;
    let region_one_x = x_at(1.5);
    let region_two_x = x_at(3.5);
    let region_one_wave = Pos2::new(region_one_x, wave.center().y);
    let region_two_wave = Pos2::new(region_two_x, wave.center().y);
    let region_one_low_pass = Pos2::new(region_one_x, spec.min.y + spec.height() * 0.25);
    let region_two_high_pass = Pos2::new(region_two_x, spec.min.y + spec.height() * 0.75);

    let middle = egui::PointerButton::Middle;
    let low_pass_before_gain_reset = state.region_low_pass_hz.clone();
    let high_pass_before_gain_reset = state.region_high_pass_hz.clone();
    let crossfade_before_gain_reset = state.region_crossfade_sec.clone();
    render_inspect_frame(
        &ctx,
        &mut state,
        0.01,
        vec![
            egui::Event::PointerMoved(region_two_wave),
            pointer_button_event(region_two_wave, middle, true, egui::Modifiers::NONE),
        ],
    );
    render_inspect_frame(
        &ctx,
        &mut state,
        0.02,
        vec![pointer_button_event(
            region_two_wave,
            middle,
            false,
            egui::Modifiers::NONE,
        )],
    );
    assert_eq!(state.region_gain_db, vec![6.0, 0.0]);
    assert_eq!(state.active_multi_index, Some(1));
    assert_eq!(state.region_low_pass_hz, low_pass_before_gain_reset);
    assert_eq!(state.region_high_pass_hz, high_pass_before_gain_reset);
    assert_eq!(state.region_crossfade_sec, crossfade_before_gain_reset);

    let low_pass_before = state.region_low_pass_hz.clone();
    let shift = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    render_inspect_frame_with_modifiers(
        &ctx,
        &mut state,
        0.03,
        vec![
            egui::Event::PointerMoved(region_one_low_pass),
            wheel_event(50.0, shift),
        ],
        shift,
    );
    assert!(state.region_low_pass_hz[0] > low_pass_before[0]);
    assert_eq!(state.region_low_pass_hz[1], low_pass_before[1]);
    assert_eq!(state.active_multi_index, Some(0));

    let high_pass_before = state.region_high_pass_hz.clone();
    render_inspect_frame_with_modifiers(
        &ctx,
        &mut state,
        0.04,
        vec![
            egui::Event::PointerMoved(region_two_high_pass),
            wheel_event(50.0, shift),
        ],
        shift,
    );
    assert_eq!(state.region_high_pass_hz[0], high_pass_before[0]);
    assert!(state.region_high_pass_hz[1] > high_pass_before[1]);
    assert_eq!(state.active_multi_index, Some(1));

    let crossfade_before = state.region_crossfade_sec.clone();
    let ctrl = egui::Modifiers {
        ctrl: true,
        ..Default::default()
    };
    render_inspect_frame_with_modifiers(
        &ctx,
        &mut state,
        0.05,
        vec![
            egui::Event::PointerMoved(region_one_wave),
            wheel_event(50.0, ctrl),
        ],
        ctrl,
    );
    assert!(state.region_crossfade_sec[0] > crossfade_before[0]);
    assert_eq!(state.region_crossfade_sec[1], crossfade_before[1]);
    assert_eq!(state.active_multi_index, Some(0));

    let low_pass_before_crossfade_reset = state.region_low_pass_hz.clone();
    let high_pass_before_crossfade_reset = state.region_high_pass_hz.clone();
    render_inspect_frame(
        &ctx,
        &mut state,
        0.06,
        vec![
            egui::Event::PointerMoved(region_two_wave),
            pointer_button_event(
                region_two_wave,
                middle,
                true,
                egui::Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
        ],
    );
    render_inspect_frame(
        &ctx,
        &mut state,
        0.07,
        vec![pointer_button_event(
            region_two_wave,
            middle,
            false,
            egui::Modifiers {
                ctrl: true,
                ..Default::default()
            },
        )],
    );
    assert_eq!(state.region_crossfade_sec[0], crossfade_before[0] + 0.01);
    assert_eq!(state.region_crossfade_sec[1], 0.0);
    assert_eq!(state.region_gain_db, vec![6.0, 0.0]);
    assert_eq!(state.region_low_pass_hz, low_pass_before_crossfade_reset);
    assert_eq!(state.region_high_pass_hz, high_pass_before_crossfade_reset);
}

#[test]
fn inspect_canvas_right_click_preview_loop_and_shift_remove_stay_session_owned() {
    let (_temp_dir, path) = generate_fixture_wav("inspect-right-click", 5.0);
    let analysis = compute_full_audio_analysis(&path, 100).expect("fixture analysis");
    let mut state = AudioPreviewState::new(path, "fixture.wav".to_string());
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(analysis));
    state.duration_sec = 5.0;
    state.view_duration_sec = 5.0;
    state.is_loading_analysis = false;
    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
    state.active_multi_index = Some(0);
    state.region_gain_db = vec![6.0, -6.0];
    state.region_low_pass_hz = vec![Some(1_000.0), Some(2_000.0)];
    state.region_high_pass_hz = vec![Some(80.0), Some(120.0)];
    state.region_crossfade_sec = vec![0.1, 0.2];
    // Keep the production preview-state transitions while preventing output to speakers.
    state.player = None;
    state.player_factory = || None;
    state._sink = None;

    let secondary = egui::PointerButton::Secondary;
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let canvas = render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    let track = Rect::from_min_max(
        Pos2::new(canvas.min.x + 48.0, canvas.min.y),
        Pos2::new(canvas.max.x - 6.0, canvas.max.y),
    );
    let [_, wave, _, _, _] = layout::AudioWorkspace::inspect_timeline_rects(track);
    let x_at = |time: f32| track.min.x + track.width() * time / state.duration_sec;
    let second_region = Pos2::new(x_at(3.5), wave.center().y);

    render_inspect_frame(
        &ctx,
        &mut state,
        0.01,
        vec![
            egui::Event::PointerMoved(second_region),
            pointer_button_event(second_region, secondary, true, egui::Modifiers::NONE),
        ],
    );
    let hold = state.right_click_hold.as_ref().expect("slice hold starts");
    assert_eq!(hold.target_range, (3.0, 4.0));
    assert_eq!(hold.slice_index, Some(1));
    assert!(!hold.started_looping);
    render_inspect_frame(
        &ctx,
        &mut state,
        0.02,
        vec![pointer_button_event(
            second_region,
            secondary,
            false,
            egui::Modifiers::NONE,
        )],
    );
    assert!(state.right_click_hold.is_none());
    assert_eq!(state.active_multi_index, Some(1));
    assert_eq!(state.preview_slice_range, Some((3.0, 4.0)));
    assert_eq!(state.preview_stop_time, Some(4.0));
    assert!(!state.preview_is_looping);
    assert!(!state.is_playing, "test must not start physical playback");

    state.pause();
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let canvas = render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    let track = Rect::from_min_max(
        Pos2::new(canvas.min.x + 48.0, canvas.min.y),
        Pos2::new(canvas.max.x - 6.0, canvas.max.y),
    );
    let [_, wave, _, _, _] = layout::AudioWorkspace::inspect_timeline_rects(track);
    let first_region = Pos2::new(
        track.min.x + track.width() * 1.5 / state.duration_sec,
        wave.center().y,
    );
    render_inspect_frame(
        &ctx,
        &mut state,
        0.01,
        vec![
            egui::Event::PointerMoved(first_region),
            pointer_button_event(first_region, secondary, true, egui::Modifiers::NONE),
        ],
    );
    state
        .right_click_hold
        .as_mut()
        .expect("hold state remains session-owned")
        .start_instant = Instant::now() - RIGHT_CLICK_HOLD_TIMEOUT - Duration::from_millis(1);
    render_inspect_frame(&ctx, &mut state, 0.4, Vec::new());
    let hold = state
        .right_click_hold
        .as_ref()
        .expect("hold state continues");
    assert!(hold.started_looping);
    assert_eq!(hold.target_range, (1.0, 2.0));
    assert_eq!(state.preview_slice_range, Some((1.0, 2.0)));
    assert!(state.preview_is_looping);
    assert!(
        !state.is_playing,
        "loop state must not start physical playback"
    );

    render_inspect_frame(
        &ctx,
        &mut state,
        0.41,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(state.right_click_hold.is_none());
    assert_eq!(state.preview_slice_range, None);
    assert!(!state.preview_is_looping);
    assert!(!state.is_playing);

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let canvas = render_inspect_frame(&ctx, &mut state, 0.0, Vec::new());
    let track = Rect::from_min_max(
        Pos2::new(canvas.min.x + 48.0, canvas.min.y),
        Pos2::new(canvas.max.x - 6.0, canvas.max.y),
    );
    let [_, wave, _, _, _] = layout::AudioWorkspace::inspect_timeline_rects(track);
    let first_region = Pos2::new(
        track.min.x + track.width() * 1.5 / state.duration_sec,
        wave.center().y,
    );
    let shift = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    render_inspect_frame_with_modifiers(
        &ctx,
        &mut state,
        0.01,
        vec![
            egui::Event::PointerMoved(first_region),
            pointer_button_event(first_region, secondary, true, shift),
        ],
        shift,
    );
    render_inspect_frame_with_modifiers(
        &ctx,
        &mut state,
        0.02,
        vec![pointer_button_event(first_region, secondary, false, shift)],
        shift,
    );
    assert_eq!(state.multi_selections, vec![(3.0, 4.0)]);
    assert_eq!(state.region_gain_db, vec![-6.0]);
    assert_eq!(state.region_low_pass_hz, vec![Some(2_000.0)]);
    assert_eq!(state.region_high_pass_hz, vec![Some(120.0)]);
    assert_eq!(state.region_crossfade_sec, vec![0.2]);
}
