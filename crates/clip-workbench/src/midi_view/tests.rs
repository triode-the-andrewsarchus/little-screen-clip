use super::export::*;
use super::*;
use little_clip_core::config::WorkbenchAppearance;
use little_clip_core::palette::AppPalette;

fn inline_midi_data(note_count: usize) -> Arc<MidiData> {
    Arc::new(MidiData {
        title: "Shared compact score".to_owned(),
        track_count: 1,
        track_names: vec!["Track".to_owned()],
        total_notes: note_count,
        duration_sec: note_count.max(1) as f32,
        bpm: 120.0,
        min_key: 60,
        max_key: 72,
        notes: (0..note_count)
            .map(|index| MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 100,
                start_sec: index as f32,
                duration_sec: 0.15,
            })
            .collect(),
    })
}

fn render_inline_midi_pair(
    ctx: &egui::Context,
    first: &mut InlineMidiPreview,
    second: &mut InlineMidiPreview,
    events: Vec<egui::Event>,
    time: f64,
) -> (egui::FullOutput, [Rect; 2]) {
    let palette = AppPalette::default();
    let mut rects = [Rect::NOTHING; 2];
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(480.0, 180.0))),
            events,
            time: Some(time),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    rects[0] = first.show(ui, &palette).rect;
                    rects[1] = second.show(ui, &palette).rect;
                });
            });
        },
    );
    (output, rects)
}

fn click_events(position: Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(position),
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn inline_midi_previews_share_data_with_independent_seek_and_selection() {
    let data = inline_midi_data(300);
    let mut first = InlineMidiPreview::new(Arc::clone(&data), Vec2::new(220.0, 124.0));
    let mut second = InlineMidiPreview::new(Arc::clone(&data), Vec2::new(220.0, 124.0));
    assert!(Arc::ptr_eq(&first.data, &second.data));
    assert_ne!(first.id, second.id);

    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    let (initial, rects) = render_inline_midi_pair(&ctx, &mut first, &mut second, Vec::new(), 0.0);
    let note_color = {
        let (r, g, b) = palette.midi_note_color(0, 0, 1, 60, 100);
        Color32::from_rgb(r, g, b)
    };
    let note_center = initial
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(rect)
                if shape.clip_rect.intersects(rects[0]) && rect.fill == note_color =>
            {
                Some(rect.rect.center())
            }
            _ => None,
        })
        .expect("first preview should render its notes");

    render_inline_midi_pair(
        &ctx,
        &mut first,
        &mut second,
        click_events(note_center, true),
        0.1,
    );
    render_inline_midi_pair(
        &ctx,
        &mut first,
        &mut second,
        click_events(note_center, false),
        0.2,
    );
    assert_eq!(first.selected_note, Some(0));
    assert!((first.playhead_sec - data.notes[0].start_sec).abs() < f32::EPSILON);
    assert_eq!(second.selected_note, None);
    assert_eq!(second.playhead_sec, 0.0);

    let seek_position = Pos2::new(rects[1].right() - 16.0, rects[1].top() + 30.0);
    render_inline_midi_pair(
        &ctx,
        &mut first,
        &mut second,
        click_events(seek_position, true),
        0.3,
    );
    render_inline_midi_pair(
        &ctx,
        &mut first,
        &mut second,
        click_events(seek_position, false),
        0.4,
    );
    assert!(second.playhead_sec > first.playhead_sec);
    assert_eq!(second.selected_note, None);
    assert_eq!(first.selected_note, Some(0));
    // The preview has only score data and local seek/selection state; it owns no parser,
    // synthesis receiver, or executor handle, so clicking cannot schedule either job.
}

#[test]
fn inline_midi_preview_clips_geometry_and_caps_note_rendering() {
    let mut data = (*inline_midi_data(2_000)).clone();
    data.duration_sec = 100.0;
    for (index, note) in data.notes.iter_mut().enumerate() {
        note.start_sec = index as f32 * 0.001;
    }
    data.notes.last_mut().unwrap().start_sec = 99.0;
    let data = Arc::new(data);
    let mut first = InlineMidiPreview::new(Arc::clone(&data), Vec2::new(220.0, 124.0));
    let mut second = InlineMidiPreview::new(data, Vec2::new(220.0, 124.0));
    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    let (output, rects) = render_inline_midi_pair(&ctx, &mut first, &mut second, Vec::new(), 0.0);
    let (r, g, b) = palette.midi_note_color(0, 0, 1, 60, 100);
    let note_color = Color32::from_rgb(r, g, b);
    let note_shapes: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(rect) if rect.fill == note_color => {
                Some((shape.clip_rect, rect.rect))
            }
            _ => None,
        })
        .collect();
    assert!(note_shapes.len() <= INLINE_MIDI_MAX_NOTES_DRAWN * 2);
    assert!(note_shapes.len() > INLINE_MIDI_MAX_NOTES_DRAWN);
    assert!(
        note_shapes.iter().any(|(_, note_rect)| {
            note_rect.center().x > rects[0].left() + rects[0].width() * 0.8
        }),
        "bounded sampling should include the sparse final note"
    );
    for (clip_rect, note_rect) in note_shapes {
        let owner = rects
            .iter()
            .find(|host| host.intersects(clip_rect) && host.intersects(note_rect))
            .expect("every note belongs to a visible host");
        assert!(clip_rect.intersects(*owner));
        assert!(note_rect.intersects(*owner));
        assert!(owner.contains_rect(note_rect));
        assert!(note_rect.is_positive());
    }

    let clipped_ctx = egui::Context::default();
    let mut clipped = InlineMidiPreview::new(inline_midi_data(100), Vec2::new(220.0, 124.0));
    let clipped_output = clipped_ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 100.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(200.0);
                clipped.show(ui, &palette);
            });
        },
    );
    assert!(clipped_output.shapes.iter().all(|shape| {
        !matches!(&shape.shape, egui::epaint::Shape::Rect(rect) if rect.fill == note_color)
    }));
}

#[test]
fn independent_midi_views_share_score_and_keep_transport_separate() {
    let source = Arc::new(MidiData {
        title: "Shared score".to_owned(),
        track_count: 1,
        track_names: vec!["Track".to_owned()],
        total_notes: 1,
        duration_sec: 4.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: vec![MidiNote {
            track: 0,
            channel: 0,
            program: 0,
            key: 60,
            velocity: 100,
            start_sec: 0.5,
            duration_sec: 1.0,
        }],
    });
    let mut first =
        MidiViewerState::from_shared(Arc::clone(&source), None, "First score view".to_owned());
    let second =
        MidiViewerState::from_shared(Arc::clone(&source), None, "Second score view".to_owned());

    assert!(Arc::ptr_eq(&first.data, &second.data));
    assert!(Arc::ptr_eq(&first.data, &source));
    first.is_playing = true;
    first.playhead_sec = 1.25;
    first.selected_region = Some((0.5, 1.5));
    first.solo_track = Some(0);

    assert!(!second.is_playing);
    assert_eq!(second.playhead_sec, 0.0);
    assert_eq!(second.selected_region, None);
    assert_eq!(second.solo_track, None);
}

#[test]
fn inline_midi_source_seeds_independent_full_studios_without_parsing_or_synthesis() {
    let source = inline_midi_data(12);
    let inline = InlineMidiPreview::new(Arc::clone(&source), Vec2::new(220.0, 124.0));
    let mut first =
        MidiViewerState::from_shared(Arc::clone(&source), None, "First full score".to_owned());
    let second =
        MidiViewerState::from_shared(Arc::clone(&source), None, "Second full score".to_owned());

    assert!(Arc::ptr_eq(&inline.data, &source));
    assert!(Arc::ptr_eq(&first.data, &source));
    assert!(Arc::ptr_eq(&second.data, &source));
    assert!(!first.is_synthesizing && first.synth_rx.is_none() && first.synth_ticket.is_none());
    assert!(!second.is_synthesizing && second.synth_rx.is_none() && second.synth_ticket.is_none());

    first.playhead_sec = 2.0;
    first.selected_region = Some((1.0, 2.0));
    first.view_start_sec = 1.5;
    assert_eq!(second.playhead_sec, 0.0);
    assert_eq!(second.selected_region, None);
    assert_eq!(second.view_start_sec, 0.0);
    assert_eq!(inline.playhead_sec, 0.0);
    assert_eq!(inline.selected_note, None);
}

#[test]
fn midi_space_runs_the_render_handler_and_sets_selected_region_bounds() {
    let data = MidiData {
        title: "Space test".to_string(),
        track_count: 1,
        track_names: vec!["Track".to_string()],
        total_notes: 0,
        duration_sec: 4.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: Vec::new(),
    };
    let mut state = MidiViewerState::from_data(data, None, "Space test".to_string());
    // This empty cache is too short for the selected-region offset, so
    // play_selection updates transport bounds and play returns before
    // attempting to decode audio or open an output device.
    state.wav_cache = Some(vec![0; 44].into());
    state.selected_region = Some((1.25, 2.75));
    let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
    let mut close_requested = false;
    let ctx = egui::Context::default();

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
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, &palette, &mut close_requested);
            });
        },
    );

    assert_eq!(state.playhead_sec, 1.25);
    assert_eq!(state.play_stop_time, Some(2.75));
    assert!(!state.is_playing);
    assert!(!close_requested);
}

#[test]
fn midi_space_pauses_active_transport_without_player_or_sink() {
    let data = MidiData {
        title: "Space pause test".to_string(),
        track_count: 1,
        track_names: vec!["Track".to_string()],
        total_notes: 0,
        duration_sec: 4.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: Vec::new(),
    };
    let mut state = MidiViewerState::from_data(data, None, "Space pause test".to_string());
    state.is_playing = true;
    state.playback_start_instant = Some(Instant::now());
    state.player = None;
    state._sink = None;
    let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
    let mut close_requested = false;
    let ctx = egui::Context::default();

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
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, &palette, &mut close_requested);
            });
        },
    );

    assert!(!state.is_playing);
    assert!(state.playback_start_instant.is_none());
    assert!(state.player.is_none());
    assert!(state._sink.is_none());
    assert!(!close_requested);
}

#[test]
fn midi_escape_clears_selected_region_before_closing_viewer() {
    let data = MidiData {
        title: "Escape selection test".to_string(),
        track_count: 1,
        track_names: vec!["Track".to_string()],
        total_notes: 0,
        duration_sec: 4.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: Vec::new(),
    };
    let mut state = MidiViewerState::from_data(data, None, "Escape selection test".to_string());
    state.selected_region = Some((0.5, 2.5));
    state.play_stop_time = Some(2.5);
    // Model an active transport without audio resources so an accidental stop
    // transition is observable, while this regression cannot emit audio.
    state.is_playing = true;
    state.playback_start_instant = None;
    state.player = None;
    state._sink = None;

    let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
    let mut close_requested = false;
    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
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
                state.render_ui(ui, &palette, &mut close_requested);
            });
        },
    );

    assert!(state.selected_region.is_none());
    assert!(state.play_stop_time.is_none());
    assert!(
        !close_requested,
        "first Escape clears the region, not viewer"
    );
    assert!(
        state.is_playing,
        "clearing selection must not stop transport"
    );
    assert!(state.player.is_none());
    assert!(state._sink.is_none());
}

#[test]
fn piano_roll_pitch_labels_thin_out_before_lanes_get_too_short() {
    let label_height = crate::ui_ext::MONO_FONT_SIZE;
    assert!(should_draw_pitch_label(
        61,
        label_height + 2.0,
        label_height
    ));
    assert!(should_draw_pitch_label(60, 8.0, label_height));
    assert!(!should_draw_pitch_label(61, 8.0, label_height));
    assert!(should_draw_pitch_label(72, 8.0, label_height));
}

#[test]
fn piano_roll_octave_labels_stay_inside_canvas_edges() {
    assert_eq!(clamp_label_center(2.0, 16.0, 0.0, 100.0), Some(8.0));
    assert_eq!(clamp_label_center(98.0, 16.0, 0.0, 100.0), Some(92.0));
    assert_eq!(clamp_label_center(5.0, 16.0, 0.0, 12.0), None);
}

#[test]
fn piano_roll_time_labels_respect_measured_spacing_and_canvas_edge() {
    assert!(time_label_fits(20.0, 40.0, 0.0, 100.0));
    assert!(!time_label_fits(43.0, 30.0, 40.0, 100.0));
    assert!(!time_label_fits(70.0, 40.0, 0.0, 100.0));
}

#[test]
fn piano_roll_key_labels_keep_contrast_on_black_and_white_keys() {
    for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
        let palette = AppPalette::for_appearance(appearance);
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
        for (key_fill, preferred) in [
            (colors.panel, colors.text_muted),
            (colors.text_primary, colors.canvas),
        ] {
            let foreground = crate::ui_ext::contrast_text_c32_on(
                key_fill,
                colors.recessed,
                Some(preferred),
                &palette,
            );
            assert!(
                little_clip_core::palette::contrast_ratio(
                    (key_fill.r(), key_fill.g(), key_fill.b()),
                    (foreground.r(), foreground.g(), foreground.b()),
                ) >= 4.5,
                "{appearance:?} piano key label lacks contrast"
            );
        }
    }
}

#[test]
fn test_midi_view_track_colors() {
    for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
        let palette = AppPalette::for_appearance(appearance);
        let t0 = get_track_color(0, 0, 4, &palette);
        let t1 = get_track_color(1, 0, 4, &palette);
        let drums = get_track_color(2, 9, 4, &palette);
        assert_ne!(t0, t1, "{appearance:?}: tracks should be distinct");
        assert_ne!(
            t0, drums,
            "{appearance:?}: melodic and drum tracks should differ"
        );
    }
}

#[test]
fn fixed_appearance_midi_notes_are_vibrant_and_distinct() {
    for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
        let palette = AppPalette::for_appearance(appearance);
        let note_c = palette.midi_note_color(0, 0, 4, 60, 100);
        let note_d = palette.midi_note_color(1, 0, 4, 62, 100);
        assert_ne!(note_c, note_d, "{appearance:?}: track notes should differ");
        let chroma = little_clip_core::palette::Oklab::from_rgb(note_c.0, note_c.1, note_c.2)
            .to_oklch()
            .c;
        assert!(
            chroma >= 0.15,
            "{appearance:?}: note chroma {chroma:.3} is muted"
        );
        let (r, g, b, _) = palette.rgba(&palette.colors.surface_recessed);
        assert!(
            little_clip_core::palette::contrast_ratio((r, g, b), note_c) >= 2.5,
            "{appearance:?}: MIDI note lacks contrast against display surface"
        );
    }
}

#[test]
fn test_midi_view_single_track_pitch_differentiation() {
    for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
        let palette = AppPalette::for_appearance(appearance);
        let c4 = palette.midi_note_color(0, 0, 1, 60, 100);
        let g4 = palette.midi_note_color(0, 0, 1, 67, 100);
        assert_ne!(
            c4, g4,
            "{appearance:?}: C4 and G4 should have distinct hues"
        );
    }
}

#[test]
fn midi_viewer_tracks_selection_and_solo_for_export() {
    let midi = MidiData {
        title: "Test Score".to_string(),
        track_count: 2,
        track_names: vec!["Melody".to_string(), "Bass".to_string()],
        total_notes: 4,
        duration_sec: 5.0,
        bpm: 120.0,
        min_key: 36,
        max_key: 72,
        notes: vec![
            MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 90,
                start_sec: 1.0,
                duration_sec: 1.0,
            },
            MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 64,
                velocity: 90,
                start_sec: 2.5,
                duration_sec: 1.0,
            },
            MidiNote {
                track: 1,
                channel: 1,
                program: 32,
                key: 36,
                velocity: 95,
                start_sec: 0.5,
                duration_sec: 2.0,
            },
            MidiNote {
                track: 1,
                channel: 1,
                program: 32,
                key: 40,
                velocity: 95,
                start_sec: 3.0,
                duration_sec: 1.5,
            },
        ],
    };

    let mut state = MidiViewerState {
        path: None,
        title: "Test Score".to_string(),
        data: Arc::new(midi),
        is_playing: false,
        playhead_sec: 0.0,
        playback_start_instant: None,
        playback_start_offset: 0.0,
        player: None,
        _sink: None,
        wav_cache: None,
        view_start_sec: 0.0,
        view_duration_sec: 5.0,
        user_panned_during_playback: false,
        hovered_note: None,
        status_toast: StatusToast::new(),
        selected_region: None,
        solo_track: None,
        drag_start_time: None,
        is_dragging_selection: false,
        play_stop_time: None,
        is_synthesizing: false,
        pending_play: false,
        synth_rx: None,
        synth_ticket: None,
        export_job: None,
    };

    // Test solo toggling
    state.toggle_solo(1);
    assert_eq!(state.solo_track, Some(1));
    state.toggle_solo(1);
    assert_eq!(state.solo_track, None);

    state.toggle_solo(0);
    assert_eq!(state.solo_track, Some(0));
    state.clear_solo();
    assert_eq!(state.solo_track, None);

    // Selection and solo state are captured by the asynchronous export request.
    state.selected_region = Some((1.0, 3.0));
    state.solo_track = Some(0);
    assert_eq!(state.selected_region, Some((1.0, 3.0)));
    assert_eq!(state.solo_track, Some(0));
}

#[test]
fn midi_export_worker_writes_selected_solo_slice_with_original_timing_and_name() {
    let data = MidiData {
        title: "Test Score".to_string(),
        track_count: 2,
        track_names: vec!["Melody".to_string(), "Bass Line".to_string()],
        total_notes: 2,
        duration_sec: 4.0,
        bpm: 120.0,
        min_key: 40,
        max_key: 72,
        notes: vec![
            MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 90,
                start_sec: 0.75,
                duration_sec: 1.0,
            },
            MidiNote {
                track: 1,
                channel: 1,
                program: 32,
                key: 40,
                velocity: 95,
                start_sec: 1.0,
                duration_sec: 1.25,
            },
        ],
    };
    let test_dir = std::env::temp_dir().join(format!(
        "little-screen-clip-midi-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let artifact = write_midi_export(
        &data,
        0.5,
        2.0,
        true,
        Some(1),
        123,
        test_dir.clone(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();

    assert_eq!(artifact.total_notes, 1);
    assert_eq!(artifact.duration_sec, 1.5);
    assert!(
        artifact
            .path
            .ends_with("midi_slice_123_0.5s-2.0s_Bass_Line.mid")
    );
    assert_eq!(artifact.solo_info, " (solo 'Bass Line')");
    let bytes = std::fs::read(&artifact.path).unwrap();
    let parsed = MidiData::parse_bytes(&bytes, "round trip".to_string()).unwrap();
    assert_eq!(parsed.total_notes, 1);
    assert_eq!(
        parsed.notes[0].track, 0,
        "solo track is remapped to track zero"
    );
    assert!((parsed.notes[0].start_sec - 0.5).abs() < 0.01);
    drop(artifact);
    std::fs::remove_dir(test_dir).unwrap();
}

#[test]
fn midi_full_score_copy_uses_injected_job_directory_and_handoff() {
    let data = MidiData {
        title: "Injected full score".to_string(),
        track_count: 1,
        track_names: vec!["Piano".to_string()],
        total_notes: 1,
        duration_sec: 2.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: vec![MidiNote {
            track: 0,
            channel: 0,
            program: 0,
            key: 60,
            velocity: 96,
            start_sec: 0.25,
            duration_sec: 0.5,
        }],
    };
    let output_dir = tempfile::tempdir().unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = MidiViewerState::from_data(data, None, "Injected full score".to_string());
    let (request_tx, request_rx) = std::sync::mpsc::channel();

    assert!(state.copy_selection_on(
        1.0,
        &jobs,
        output_dir.path().to_owned(),
        move |mut artifact, _cancelled, _started| {
            let request = IpcRequest::PushFile {
                path: artifact.path.clone(),
                hand: HandSide::Left,
            };
            let bytes = std::fs::read(&artifact.path).map_err(|error| error.to_string())?;
            request_tx
                .send((request, bytes))
                .map_err(|error| error.to_string())?;
            artifact.retain();
            Ok(artifact)
        },
    ));

    let (request, bytes) = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("injected handoff callback should receive export");
    let IpcRequest::PushFile { path, hand } = request else {
        panic!("MIDI full-score handoff should request a file");
    };
    assert_eq!(hand, HandSide::Left);
    assert_eq!(path.parent(), Some(output_dir.path()));
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("midi_export_")
    );
    let parsed = MidiData::parse_bytes(&bytes, "injected round trip".to_string())
        .expect("full-score export should be parseable MIDI");
    assert_eq!(parsed.total_notes, 1);
    assert_eq!(parsed.notes[0].key, 60);
    assert!((parsed.notes[0].start_sec - 0.25).abs() < 0.01);
    assert!((parsed.notes[0].duration_sec - 0.5).abs() < 0.01);

    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.export_job.is_some() && Instant::now() < deadline {
        state.poll_export(1.1, &ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        state.export_job.is_none(),
        "injected export job did not resolve"
    );
    assert_eq!(
        state.status_toast.message(1.1),
        Some("Copied full MIDI (1 notes) to Clipboard & Left Hand!")
    );
}

#[test]
fn rendered_full_score_copy_shortcut_exports_parseable_midi_to_left_hand() {
    let data = MidiData {
        title: "Rendered full score".to_string(),
        track_count: 1,
        track_names: vec!["Piano".to_string()],
        total_notes: 1,
        duration_sec: 2.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: vec![MidiNote {
            track: 0,
            channel: 0,
            program: 0,
            key: 60,
            velocity: 96,
            start_sec: 0.25,
            duration_sec: 0.5,
        }],
    };
    let expected_bytes = data.to_midi_bytes().expect("serialize fixture score");
    let mut state = MidiViewerState::from_data(data, None, "Rendered full score".to_string());
    let output_dir = tempfile::tempdir().unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let (request_tx, request_rx) = std::sync::mpsc::channel();
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut close = false;
    let mut time = 0.0;
    let output_dir_path = output_dir.path().to_owned();
    let mut dispatch_count = 0;
    let mut dispatch = |state: &mut MidiViewerState, now| {
        dispatch_count += 1;
        let request_tx = request_tx.clone();
        state.copy_selection_on(
            now,
            &jobs,
            output_dir_path.clone(),
            move |mut artifact, _cancelled, _started| {
                let request = IpcRequest::PushFile {
                    path: artifact.path.clone(),
                    hand: HandSide::Left,
                };
                let bytes = std::fs::read(&artifact.path).map_err(|error| error.to_string())?;
                request_tx
                    .send((request, bytes))
                    .map_err(|error| error.to_string())?;
                artifact.retain();
                Ok(artifact)
            },
        );
    };
    let mut render = |state: &mut MidiViewerState, events: Vec<egui::Event>| {
        time += 0.1;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui_with_copy_dispatch(ui, &palette, &mut close, &mut dispatch);
                });
            },
        );
        output
    };

    let initial = render(&mut state, Vec::new());
    assert!(
        initial.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Text(text) if text.galley.text() == "Copy MIDI [Ctrl+C]"
        )),
        "rendered full-score Copy MIDI button label"
    );
    render(&mut state, vec![egui::Event::Copy]);
    drop(render);
    assert_eq!(dispatch_count, 1, "rendered copy shortcut dispatch count");

    let (request, bytes) = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("rendered copy action should dispatch one MIDI export");
    let IpcRequest::PushFile { path, hand } = request else {
        panic!("rendered MIDI copy should request a file");
    };
    assert_eq!(hand, HandSide::Left);
    assert_eq!(path.parent(), Some(output_dir.path()));
    assert_eq!(bytes, expected_bytes);
    let parsed = MidiData::parse_bytes(&bytes, "rendered round trip".to_string())
        .expect("rendered copy payload should be parseable MIDI");
    assert_eq!(parsed.total_notes, 1);
    assert_eq!(parsed.notes[0].key, 60);

    assert!(!close);
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.export_job.is_some() && Instant::now() < deadline {
        state.poll_export(time + 0.1, &ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        state.export_job.is_none(),
        "rendered MIDI export did not finish"
    );
    assert_eq!(
        state.status_toast.message(time + 0.1),
        Some("Copied full MIDI (1 notes) to Clipboard & Left Hand!")
    );
}

#[test]
fn midi_export_worker_honors_cancellation_before_writing() {
    let data = MidiData {
        title: "Cancelled".to_string(),
        track_count: 0,
        track_names: Vec::new(),
        total_notes: 0,
        duration_sec: 1.0,
        bpm: 120.0,
        min_key: 0,
        max_key: 127,
        notes: Vec::new(),
    };
    let test_dir = std::env::temp_dir().join(format!(
        "little-screen-clip-midi-cancel-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cancelled = Arc::new(AtomicBool::new(true));
    let result = write_midi_export(
        &data,
        0.0,
        1.0,
        false,
        None,
        456,
        test_dir.clone(),
        cancelled,
    );
    assert_eq!(result.unwrap_err(), "MIDI export cancelled");
    assert!(!test_dir.exists());
}

#[test]
fn midi_export_file_creation_retries_collision_without_overwriting_existing_file() {
    let test_dir = std::env::temp_dir().join(format!(
        "little-screen-clip-midi-collision-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&test_dir).unwrap();
    let original_path = test_dir.join("midi_slice_123_0.5s-2.0s_Bass.mid");
    std::fs::write(&original_path, b"original export").unwrap();

    let second_path = write_unique_midi_export(
        &test_dir,
        "midi_slice_123_0.5s-2.0s_Bass.mid",
        b"new export",
    )
    .unwrap();

    assert_eq!(
        second_path,
        test_dir.join("midi_slice_123_0.5s-2.0s_Bass_1.mid")
    );
    assert_eq!(std::fs::read(&original_path).unwrap(), b"original export");
    assert_eq!(std::fs::read(&second_path).unwrap(), b"new export");
    std::fs::remove_file(original_path).unwrap();
    std::fs::remove_file(second_path).unwrap();
    std::fs::remove_dir(test_dir).unwrap();
}

#[test]
fn test_midi_viewer_render_ui_tracks_and_legend() {
    let midi = MidiData {
        title: "Multi-Track Score".to_string(),
        track_count: 2,
        track_names: vec!["Melody".to_string(), "Harmony".to_string()],
        total_notes: 2,
        duration_sec: 5.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 64,
        notes: vec![
            MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 80,
                start_sec: 0.0,
                duration_sec: 1.0,
            },
            MidiNote {
                track: 1,
                channel: 1,
                program: 0,
                key: 64,
                velocity: 80,
                start_sec: 1.0,
                duration_sec: 1.0,
            },
        ],
    };

    let mut state = MidiViewerState {
        path: None,
        title: "Multi-Track Score".to_string(),
        data: Arc::new(midi),
        is_playing: false,
        playhead_sec: 0.0,
        playback_start_instant: None,
        playback_start_offset: 0.0,
        player: None,
        _sink: None,
        wav_cache: None,
        view_start_sec: 0.0,
        view_duration_sec: 5.0,
        user_panned_during_playback: false,
        hovered_note: None,
        status_toast: StatusToast::new(),
        selected_region: None,
        solo_track: None,
        drag_start_time: None,
        is_dragging_selection: false,
        play_stop_time: None,
        is_synthesizing: false,
        pending_play: false,
        synth_rx: None,
        synth_ticket: None,
        export_job: None,
    };
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();

    state.toggle_solo(0);
    assert_eq!(state.solo_track, Some(0));
    state.selected_region = Some((0.5, 1.5));

    for width in [760.0, 1000.0, 1760.0] {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(width, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut close = false;
                    state.render_ui(ui, &palette, &mut close);
                    assert!(!close);
                });
            },
        );

        let required_labels = [
            "Copy Solo Slice (Melody)",
            "Sel: 0.50s - 1.50s",
            "All Tracks",
            "Tracks:",
        ];
        let visible_text = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some(shape),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            visible_text.iter().all(|shape| {
                !matches!(shape.galley.job.text.as_str(), "Unsolo" | "Solo: Melody")
            })
        );

        for required in required_labels {
            assert!(
                visible_text.iter().any(|shape| {
                    shape.galley.job.text.contains(required) && !shape.galley.elided
                }),
                "{required:?} missing or elided at {width} points"
            );
        }

        for shape in visible_text {
            let bounds = shape.galley.rect.translate(shape.pos.to_vec2());
            assert!(
                bounds.left() >= 0.0 && bounds.right() <= width + 0.5,
                "MIDI label {:?} exceeds {width}-point host: {bounds:?}",
                shape.galley.job.text
            );
        }
    }

    state.clear_solo();
    assert_eq!(state.solo_track, None);
}
