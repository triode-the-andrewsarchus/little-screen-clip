use super::*;

#[test]
fn hidden_rolling_audio_retires_on_view_switch_and_modal_entry() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let mut strip = RollingAudioStripState::empty_test_strip();
    strip.is_playing = true;
    strip.selected_region = Some((0.25, 0.75));
    app.rolling_audio_strip = Some(strip);
    app.retire_hidden_rolling_audio();
    assert!(app.rolling_audio_strip.as_ref().unwrap().is_playing);

    app.view = WorkbenchView::NodeMachine;
    app.retire_hidden_rolling_audio();
    assert!(!app.rolling_audio_strip.as_ref().unwrap().is_playing);
    assert_eq!(
        app.rolling_audio_strip.as_ref().unwrap().selected_region,
        Some((0.25, 0.75))
    );

    app.view = WorkbenchView::Palette;
    app.rolling_audio_strip.as_mut().unwrap().is_playing = true;
    app.viewer = Some(ViewerSession::ComponentGallery(
        component_gallery::ComponentGalleryState::default(),
    ));
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(!app.rolling_audio_strip.as_ref().unwrap().is_playing);
}

#[test]
fn spreadsheet_completion_waits_until_modal_viewer_closes() {
    let mut app = test_workbench_app(
        Some(ViewerSession::ComponentGallery(
            component_gallery::ComponentGalleryState::default(),
        )),
        VecDeque::new(),
    );
    let (ticket, _ticket_receiver) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
    let (result_tx, receiver) = std::sync::mpsc::channel::<
        Result<Result<(SpreadsheetData, Option<PathBuf>), String>, JobError>,
    >();
    result_tx.send(Ok(Err("fixture failure".into()))).unwrap();
    app.spreadsheet_load_job = Some((ticket, receiver, None));

    let ctx = egui::Context::default();
    ui_ext::configure_workbench_style(&ctx);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );
    assert!(app.viewer.is_some());
    assert!(app.spreadsheet_load_job.is_some());

    app.viewer = None;
    app.poll_spreadsheet_load(&ctx);
    assert!(app.spreadsheet_load_job.is_none());
    assert_eq!(
        app.status_msg.as_deref(),
        Some("Spreadsheet: fixture failure")
    );
    app.poll_spreadsheet_load(&ctx);
    assert!(app.viewer.is_none());
    assert_eq!(
        app.status_msg.as_deref(),
        Some("Spreadsheet: fixture failure")
    );

    app.viewer = Some(ViewerSession::ComponentGallery(
        component_gallery::ComponentGalleryState::default(),
    ));
    let (ticket, _ticket_receiver) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
    let (result_tx, receiver) = std::sync::mpsc::channel();
    result_tx
        .send(Ok(Ok((
            SpreadsheetData {
                title: "fixture".into(),
                sheets: Vec::new(),
            },
            None,
        ))))
        .unwrap();
    app.spreadsheet_load_job = Some((ticket, receiver, None));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        if !app.render_modal_viewers(ctx) {
            app.poll_spreadsheet_load(ctx);
        }
    });
    assert!(app.spreadsheet_load_job.is_some());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));

    app.viewer = None;
    app.poll_spreadsheet_load(&ctx);
    assert!(app.spreadsheet_load_job.is_none());
    assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
    app.poll_spreadsheet_load(&ctx);
    assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
}

#[test]
fn midi_completion_waits_for_foreground_and_startup_viewers() {
    let startup_viewer =
        || ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
    let mut app = test_workbench_app(Some(startup_viewer()), VecDeque::from([startup_viewer()]));
    let (ticket, _ticket_receiver) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
    let (result_tx, receiver) =
        std::sync::mpsc::channel::<Result<Result<(MidiData, PathBuf), String>, JobError>>();
    result_tx
        .send(Ok(Ok((
            MidiData {
                title: "fixture".into(),
                track_count: 0,
                track_names: Vec::new(),
                total_notes: 0,
                duration_sec: 0.0,
                bpm: 120.0,
                min_key: 0,
                max_key: 0,
                notes: Vec::new(),
            },
            PathBuf::from("fixture.mid"),
        ))))
        .unwrap();
    app.midi_load_job = Some((ticket, receiver));

    let ctx = egui::Context::default();
    ui_ext::configure_workbench_style(&ctx);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        },
        |ctx| {
            if !app.render_modal_viewers(ctx) {
                app.poll_midi_load(ctx);
            }
        },
    );
    assert!(app.midi_load_job.is_some());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));

    app.viewer = app.startup_viewers.pop_front();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        if !app.render_modal_viewers(ctx) {
            app.poll_midi_load(ctx);
        }
    });
    assert!(app.midi_load_job.is_some());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));

    app.viewer = app.startup_viewers.pop_front();
    assert!(app.viewer.is_none());
    app.poll_midi_load(&ctx);
    assert!(app.midi_load_job.is_none());
    assert!(matches!(app.viewer, Some(ViewerSession::Midi(_))));
    app.poll_midi_load(&ctx);
    assert!(matches!(app.viewer, Some(ViewerSession::Midi(_))));

    app.viewer = Some(startup_viewer());
    let (ticket, _ticket_receiver) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
    let (result_tx, receiver) = std::sync::mpsc::channel();
    result_tx.send(Ok(Err("fixture failure".into()))).unwrap();
    app.midi_load_job = Some((ticket, receiver));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        if !app.render_modal_viewers(ctx) {
            app.poll_midi_load(ctx);
        }
    });
    assert!(app.midi_load_job.is_some());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));

    app.viewer = None;
    app.poll_midi_load(&ctx);
    assert!(app.midi_load_job.is_none());
    assert!(app.viewer.is_none());
    assert_eq!(
        app.status_msg.as_deref(),
        Some("MIDI load failed: fixture failure")
    );
    app.poll_midi_load(&ctx);
    assert!(app.viewer.is_none());
    assert_eq!(
        app.status_msg.as_deref(),
        Some("MIDI load failed: fixture failure")
    );
}

#[test]
fn failed_rolling_selection_action_keeps_the_current_viewer_open() {
    let ctx = egui::Context::default();
    let mut app = test_workbench_app(
        Some(ViewerSession::ComponentGallery(
            component_gallery::ComponentGalleryState::default(),
        )),
        VecDeque::new(),
    );

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.wav");
    std::fs::write(&path, b"snapshot identity").unwrap();
    let source = crate::file_source::FileSource::capture(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    app.apply_rolling_audio_action(
        RollingAudioAction::OpenEditor {
            source,
            snapshot_lease: None,
            duration_sec: 2.0,
            selected_region: Some((1.0, 2.0)),
        },
        &ctx,
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app
        .rolling_editor_job
        .as_ref()
        .is_some_and(|job| job.ready.is_some())
    {
        app.poll_rolling_editor_load(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "rolling editor job should finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.poll_rolling_editor_load(&ctx);
    assert!(app.rolling_editor_job.is_some());
    assert!(
        app.status_msg
            .as_deref()
            .unwrap()
            .starts_with("Could not open rolling audio selection:")
    );
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    app.viewer = None;
    app.poll_rolling_editor_load(&ctx);

    assert!(app.viewer.is_none());
    assert!(
        app.status_msg
            .as_deref()
            .is_some_and(|message| message.starts_with("Could not open rolling audio selection:"))
    );
}

#[test]
fn rolling_editor_materialization_waits_for_active_viewer_and_preserves_range_editor() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("rolling-capture.wav");
    let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(2, 44_100, 2);
    buffer.push_samples(&vec![1000i16; 44_100 * 2 * 2]);
    buffer.write_wav(&source).unwrap();
    let gallery =
        || ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
    let mut app = test_workbench_app(Some(gallery()), VecDeque::new());
    app.config.audio.editor = "custom-audio-editor".to_owned();
    let ctx = egui::Context::default();
    app.apply_rolling_audio_action(
        RollingAudioAction::OpenEditor {
            source: crate::file_source::FileSource::capture(&source).unwrap(),
            snapshot_lease: None,
            duration_sec: 2.0,
            selected_region: Some((1.25, 0.5)),
        },
        &ctx,
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app
        .rolling_editor_job
        .as_ref()
        .is_some_and(|job| job.ready.is_some())
    {
        app.poll_rolling_editor_load(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "rolling editor materialization should finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.poll_rolling_editor_load(&ctx);
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    assert!(app.rolling_editor_job.is_some());
    app.viewer = None;
    app.poll_rolling_editor_load(&ctx);

    let Some(ViewerSession::Audio(editor)) = app.viewer.as_ref() else {
        panic!("rolling selection should open the audio editor after the current viewer closes");
    };
    assert_eq!(editor.title, "Rolling Audio Selection");
    assert_eq!(editor.audio_editor, "custom-audio-editor");
    assert!(
        (0.5..=0.8).contains(&editor.duration_sec),
        "selected range should remain bounded after optional edge trimming: {}",
        editor.duration_sec
    );
}

#[test]
fn cancelling_ready_full_snapshot_handoff_reclaims_only_its_owned_result() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("snapshot.wav");
    let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 8000, 1);
    buffer.push_samples(&vec![1234; 8000]);
    buffer.write_wav(&path).unwrap();
    let mut app = test_workbench_app(
        Some(ViewerSession::ComponentGallery(
            component_gallery::ComponentGalleryState::default(),
        )),
        VecDeque::new(),
    );
    let ctx = egui::Context::default();
    app.apply_rolling_audio_action(
        RollingAudioAction::OpenEditor {
            source: crate::file_source::FileSource::capture(&path).unwrap(),
            snapshot_lease: None,
            duration_sec: 1.0,
            selected_region: None,
        },
        &ctx,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app.rolling_editor_job.as_ref().unwrap().ready.is_some() {
        app.poll_rolling_editor_load(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let job = app.rolling_editor_job.as_ref().unwrap();
    assert_eq!(job.title, "Rolling Audio Snapshot");
    let prepared = job.ready.as_ref().unwrap().as_ref().unwrap();
    let output = prepared.path().to_owned();
    assert!(output.is_file());
    assert_ne!(output, path);
    app.rolling_editor_job = None;
    assert!(!output.exists());
    assert!(path.is_file());
    assert!(app.viewer.is_some());
}

#[test]
fn idle_audio_mode_escape_closes_to_the_originating_workbench_route() {
    let temp_dir = tempfile::tempdir().unwrap();
    let wav_path = temp_dir.path().join("escape-audio.wav");
    let mut wav = Vec::with_capacity(44 + 16_000);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + 16_000).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8_000u32.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.resize(44 + 16_000, 0);
    std::fs::write(&wav_path, wav).unwrap();

    for mode in [
        audio_preview::AudioToolMode::Inspect,
        audio_preview::AudioToolMode::HfAnalysis,
        audio_preview::AudioToolMode::StemSeparation,
    ] {
        let mut audio = AudioPreviewState::new(wav_path.clone(), "escape-audio.wav".into());
        audio.active_mode = mode;
        audio.is_playing = true;
        let mut app = test_workbench_app(Some(ViewerSession::Audio(audio)), VecDeque::new());
        app.view = WorkbenchView::NodeMachine;
        app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
        let selected_slots = app.selected_slots.clone();

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

        assert!(app.viewer.is_none(), "Escape closes {mode:?} Audio Studio");
        assert_eq!(app.view, WorkbenchView::NodeMachine);
        assert_eq!(app.selected_slots, selected_slots);
    }
}

#[test]
fn midi_escape_retains_accepted_export_and_handles_its_completion() {
    let data = MidiData {
        title: "retained MIDI export".into(),
        track_count: 1,
        track_names: vec!["fixture".into()],
        total_notes: 0,
        duration_sec: 1.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: Vec::new(),
    };
    let mut midi = MidiViewerState::from_shared(Arc::new(data), None, "fixture".into());
    let complete = clip_studios::midi_view::hold_export_for_test(&mut midi);
    let mut app = test_workbench_app(Some(ViewerSession::Midi(midi)), VecDeque::new());
    let ctx = egui::Context::default();
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
    let output = ctx.run(escape(), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(matches!(&app.viewer, Some(ViewerSession::Midi(state)) if state.export_pending()));
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    complete("controlled host export failure".into());
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        app.render_modal_viewers(ctx);
    });
    let Some(ViewerSession::Midi(state)) = &app.viewer else {
        panic!("completion should remain inspectable");
    };
    assert!(!state.export_pending());
    assert_eq!(
        state.status_toast.message(crate::ui_ext::feedback_time()),
        Some("controlled host export failure")
    );
    let _ = ctx.run(escape(), |ctx| {
        app.render_modal_viewers(ctx);
    });
    assert!(app.viewer.is_none());
}
