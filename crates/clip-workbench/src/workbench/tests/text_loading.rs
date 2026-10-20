use super::*;

#[test]
fn text_file_preview_loads_on_worker_and_reports_read_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("sample.rs");
    std::fs::write(&path, "fn sample() {\n    42\n}\n").unwrap();
    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());

    app.open_slot_file_preview(&path, false);
    assert!(app.text_load_job.is_some());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    wait_for_text_load(&mut app);
    let Some(ViewerSession::Text(state)) = &app.viewer else {
        panic!("missing text viewer")
    };
    assert_eq!(state.path.as_deref(), Some(path.as_path()));
    assert!(state.content.contains("fn sample()"));
    assert_eq!(state.language, "rs");
    assert!(app.status_msg.is_none());

    app.start_text_file_load(temp_dir.path().join("missing.rs"));
    wait_for_text_load(&mut app);
    assert!(matches!(app.viewer, Some(ViewerSession::Text(_))));
    assert!(app.status_msg.as_deref().is_some_and(|message| {
        message.starts_with("Text preview failed: Could not read or prepare text file")
    }));
}

#[test]
fn text_preview_escape_without_find_returns_to_source_route_and_slots() {
    let config = AppConfig::default();
    let state = TextPreviewState::from_text(
        "in-memory preview text".to_owned(),
        Some("Text Preview Fixture".to_owned()),
        &config,
    );
    assert!(!state.show_find);

    let mut app = test_workbench_app(None, VecDeque::new());
    app.view = WorkbenchView::NodeMachine;
    app.selected_slots = vec![SlotId::RightHand, SlotId::RightPrior2];
    let original_slots = app.selected_slots.clone();
    app.viewer = Some(ViewerSession::Text(state));

    let ctx = egui::Context::default();
    assert!(ctx.memory(|memory| memory.focused()).is_none());
    let _ = ctx.run(
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

    assert!(app.viewer.is_none(), "Escape closes the idle text preview");
    assert_eq!(app.view, WorkbenchView::NodeMachine);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn clipboard_text_preview_prepares_on_worker_and_preserves_full_searchable_text() {
    let text = format!("{}\nTARGET_AT_END", "clipboard row\n".repeat(80_000));
    let expected = text.clone();
    let mut app = test_workbench_app(None, VecDeque::new());
    let mut item = ClipItem::from_text(text);
    item.title = "large clipboard".to_owned();

    app.open_slot_preview_modal(&item);
    assert!(app.spreadsheet_load_job.is_some());
    wait_for_text_load(&mut app);

    let Some(ViewerSession::Text(state)) = &mut app.viewer else {
        panic!("missing text viewer")
    };
    assert_eq!(state.title, "large clipboard");
    assert_eq!(state.content.as_str(), expected);
    state.find_query = "TARGET_AT_END".to_owned();
    text_preview::update_search_matches_and_wait(state);
    assert_eq!(state.match_count(), 1);
    let matched = state.search_match(0).unwrap();
    assert_eq!(
        &state.content[matched.start_byte..matched.end_byte],
        "TARGET_AT_END"
    );
}

#[test]
fn oversized_csv_remains_available_in_inspector_and_generic_preview() {
    let content = format!("{}name,count\nalpha,1\nbeta,2", " ".repeat(100_000));
    let mut app = test_workbench_app(None, VecDeque::new());
    let slot = SlotId::LeftHand;
    let item = ClipItem::from_text(content);
    *Arc::make_mut(&mut app.state).get_mut(slot).unwrap() = Some(item.clone());

    assert_eq!(
        app.check_inspector_text_preview(),
        Some((slot, None, false))
    );
    assert_eq!(
        app.inspector_text_preview_cache
            .as_ref()
            .and_then(|cache| cache.spreadsheet_slot),
        Some(slot)
    );

    app.open_slot_preview_modal(&item);
    assert!(app.spreadsheet_load_job.is_some());
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.spreadsheet_load_job.is_some() {
        app.poll_spreadsheet_load(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
}

#[test]
fn oversized_malformed_csv_falls_back_to_full_text_preview() {
    let content = format!("{}name,count\nalpha,1,extra", " ".repeat(100_000));
    let mut app = test_workbench_app(None, VecDeque::new());
    let slot = SlotId::LeftHand;
    *Arc::make_mut(&mut app.state).get_mut(slot).unwrap() =
        Some(ClipItem::from_text(content.clone()));

    assert_eq!(
        app.check_inspector_text_preview(),
        Some((slot, None, false))
    );
    assert_eq!(
        app.inspector_text_preview_cache
            .as_ref()
            .and_then(|cache| cache.spreadsheet_slot),
        Some(slot)
    );

    app.open_spreadsheet_item(slot);
    assert!(app.spreadsheet_load_job.is_some());
    wait_for_text_load(&mut app);
    let Some(ViewerSession::Text(state)) = &app.viewer else {
        panic!("malformed CSV should open the text viewer")
    };
    assert_eq!(state.content.as_str(), content);
}

#[test]
fn replacing_text_file_request_cancels_stale_completion() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("large.txt");
    let second = temp_dir.path().join("newer.txt");
    std::fs::write(&first, "older text\n".repeat(50_000)).unwrap();
    std::fs::write(&second, "newer text\n").unwrap();
    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());

    app.start_text_file_load(first.clone());
    app.start_text_file_load(second.clone());
    wait_for_text_load(&mut app);
    let Some(ViewerSession::Text(state)) = &app.viewer else {
        panic!("missing text viewer")
    };
    assert_eq!(state.path.as_deref(), Some(second.as_path()));
    assert!(state.content.contains("newer text"));

    app.start_text_file_load(first);
    app.replace_viewer(Some(gallery_viewer()));
    assert!(app.text_load_job.is_none());
    app.poll_text_load(&egui::Context::default());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
}

#[test]
fn replacing_spreadsheet_load_cancels_queued_previous_job() {
    let ran = Arc::new(AtomicBool::new(false));
    let worker_ran = Arc::clone(&ran);
    let (jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
        worker_ran.store(true, Ordering::Release);
        Err::<(SpreadsheetData, Option<PathBuf>), String>("obsolete job ran".into())
    });
    let mut app = test_workbench_app(None, VecDeque::new());
    app.spreadsheet_load_job = Some((ticket, receiver, None));

    app.start_spreadsheet_load(SpreadsheetLoadSource::Text {
        content: Arc::new("name,value\nclip,1".to_owned()),
        title: "replacement table".to_owned(),
    });
    assert!(app.spreadsheet_load_job.is_some());

    release_blocker_and_wait_for_queue_to_drain(&jobs, release);
    assert!(!ran.load(Ordering::Acquire));
    drop(app);
}

#[test]
fn replacing_midi_load_cancels_queued_previous_job() {
    let ran = Arc::new(AtomicBool::new(false));
    let worker_ran = Arc::clone(&ran);
    let (jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
        worker_ran.store(true, Ordering::Release);
        Err::<(MidiData, PathBuf), String>("obsolete job ran".into())
    });
    let mut app = test_workbench_app(None, VecDeque::new());
    app.midi_load_job = Some((ticket, receiver));

    app.start_midi_load(PathBuf::from("replacement.mid"));
    assert!(app.midi_load_job.is_some());

    release_blocker_and_wait_for_queue_to_drain(&jobs, release);
    assert!(!ran.load(Ordering::Acquire));
    drop(app);
}

#[test]
fn dropping_workbench_cancels_queued_spreadsheet_load() {
    let ran = Arc::new(AtomicBool::new(false));
    let worker_ran = Arc::clone(&ran);
    let (jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
        worker_ran.store(true, Ordering::Release);
        Err::<(SpreadsheetData, Option<PathBuf>), String>("obsolete job ran".into())
    });
    let mut app = test_workbench_app(None, VecDeque::new());
    app.spreadsheet_load_job = Some((ticket, receiver, None));

    drop(app);
    release_blocker_and_wait_for_queue_to_drain(&jobs, release);
    assert!(!ran.load(Ordering::Acquire));
}

#[test]
fn dropping_workbench_cancels_queued_midi_load() {
    let ran = Arc::new(AtomicBool::new(false));
    let worker_ran = Arc::clone(&ran);
    let (jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
        worker_ran.store(true, Ordering::Release);
        Err::<(MidiData, PathBuf), String>("obsolete job ran".into())
    });
    let mut app = test_workbench_app(None, VecDeque::new());
    app.midi_load_job = Some((ticket, receiver));

    drop(app);
    release_blocker_and_wait_for_queue_to_drain(&jobs, release);
    assert!(!ran.load(Ordering::Acquire));
}

#[test]
fn dropping_workbench_cancels_queued_image_load() {
    let ran = Arc::new(AtomicBool::new(false));
    let worker_ran = Arc::clone(&ran);
    let (jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
        worker_ran.store(true, Ordering::Release);
        Err::<ImageLoadResult, String>("obsolete image load".into())
    });
    let mut app = test_workbench_app(None, VecDeque::new());
    app.image_load_job = Some(ImageLoadJob {
        key: None,
        ticket,
        receiver,
    });

    drop(app);
    release_blocker_and_wait_for_queue_to_drain(&jobs, release);
    assert!(!ran.load(Ordering::Acquire));
}

#[test]
fn dropping_workbench_cancels_queued_text_load() {
    let ran = Arc::new(AtomicBool::new(false));
    let worker_ran = Arc::clone(&ran);
    let (jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
        worker_ran.store(true, Ordering::Release);
        Err::<TextPreviewState, String>("obsolete text load".into())
    });
    let mut app = test_workbench_app(None, VecDeque::new());
    app.text_load_job = Some(TextPreviewLoadJob { ticket, receiver });

    drop(app);
    release_blocker_and_wait_for_queue_to_drain(&jobs, release);
    assert!(!ran.load(Ordering::Acquire));
}

#[test]
fn office_document_loads_via_pdf_or_markdown() {
    let temp_dir = tempfile::tempdir().unwrap();
    let rtf_path = temp_dir.path().join("sample.rtf");
    std::fs::write(
        &rtf_path,
        b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Courier;}}\\f0\\fs24 Hello \\b Office World!\\b0\\par Content paragraph.}",
    )
    .unwrap();

    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());
    app.open_slot_file_preview(&rtf_path, false);
    assert!(app.office_load_job.is_some());

    wait_for_text_load(&mut app);
    assert!(app.office_load_job.is_none());
    assert!(
        matches!(
            app.viewer,
            Some(ViewerSession::Pdf(_)) | Some(ViewerSession::Text(_))
        ),
        "expected Pdf or Text viewer session for office document"
    );
}
