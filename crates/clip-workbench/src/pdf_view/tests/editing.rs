use super::*;

#[test]
fn dropping_editor_releases_temporary_raster_cache() {
    let state = raster_state(2, 16);
    let cache_path = state.raster_cache.as_ref().unwrap().path().to_owned();
    assert!(cache_path.exists());

    drop(state);

    assert!(!cache_path.exists());
}

#[test]
fn pdf_delete_and_rotate_wait_for_metadata_before_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(1));
    let jobs = BackgroundJobs::new(1, 1);
    let (ticket, receiver) = jobs.submit(Duration::MAX, || {
        Ok(PdfInfo {
            page_count: 2,
            title: Some("fixture".into()),
            author: None,
            page_size: None,
        })
    });
    state.info_job = Some((ticket, receiver));

    state.delete_current_page(1.0);
    state.rotate_current_page(90, 2.0);

    assert!(state.info_job.is_some(), "metadata load remains pending");
    assert!(!state.edit_in_progress);
    assert!(
        state.edit_rx.is_none(),
        "guarded actions must not dispatch edits"
    );
    let (ticket, _) = state.info_job.take().unwrap();
    ticket.cancel();
}

#[test]
fn saturated_pdf_edit_queue_reports_error_and_cleans_staging() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::MAX, || ());

    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(1));
    state.start_edit_on(&jobs, "rotate", 1, 1.0, |_, _| {
        panic!("full queue must not run a PDF edit")
    });
    state.poll_edit_worker(&egui::Context::default(), 1.0);
    assert!(!state.edit_in_progress);
    assert!(state.edit_ticket.is_none());
    assert!(state.edit_rx.is_none());
    assert_eq!(
        state.status_toast.message(1.0),
        Some("PDF edit unavailable: background job queue is full")
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
}

#[test]
fn dropping_editor_cancels_queued_pdf_edit_before_dispatch() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let temp = tempfile::tempdir().unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let worker_calls = Arc::clone(&calls);
    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(1));
    state.start_edit_on(&jobs, "rotate", 1, 1.0, move |_, _| {
        worker_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    assert!(state.edit_in_progress);
    drop(state);

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let (_, rx) = jobs.submit(Duration::MAX, || ());
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            Ok(()) => break,
            Err(JobError::QueueFull) if std::time::Instant::now() < deadline => {
                std::thread::yield_now();
            }
            result => panic!("sentinel job failed: {result:?}"),
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!temp.path().join("fixture.rotate.edited.pdf").exists());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn closing_editor_during_active_pdf_edit_keeps_existing_sibling_output() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("fixture.pdf");
    let output = temp.path().join("fixture.rotate.edited.pdf");
    write_two_page_pdf(&source);
    std::fs::write(&output, b"previous edit").unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let worker_calls = Arc::clone(&calls);
    let mut state = action_guard_state(source.clone(), 2, Some(2));
    state.start_edit_on(&jobs, "rotate", 2, 1.0, move |_, staged_output| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        worker_calls.fetch_add(1, Ordering::SeqCst);
        write_two_page_pdf(staged_output);
        Ok(())
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(state);

    let (_, sentinel_rx) = jobs.submit(Duration::MAX, || ());
    release_tx.send(()).unwrap();
    sentinel_rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&output).unwrap(), b"previous edit");
    assert!(
        std::fs::read_dir(temp.path())
            .unwrap()
            .all(|entry| entry.unwrap().path().is_file())
    );
}

#[test]
fn pdf_delete_and_rotate_without_selection_do_not_dispatch_edits() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, None);

    state.delete_current_page(1.0);
    state.rotate_current_page(90, 2.0);

    assert!(!state.edit_in_progress);
    assert!(state.edit_rx.is_none());
}

#[test]
fn pdf_delete_and_rotate_do_not_replace_an_active_edit() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 2, Some(2));
    let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
    let (sender, receiver) = channel();
    state.edit_rx = Some(receiver);
    state.edit_in_progress = true;

    state.delete_current_page(1.0);
    state.rotate_current_page(90, 2.0);

    assert!(state.edit_in_progress);
    assert_eq!(
        state.status_toast.message(2.0),
        Some("A PDF edit is already in progress")
    );
    assert!(state.edit_rx.is_some());
    assert!(
        sender
            .send(Ok(PdfEditMsg {
                operation: "sentinel",
                page_num: 2,
                output: temp.path().join("untouched.pdf"),
                staging_dir,
                result: Err("sentinel only".into()),
            }))
            .is_ok(),
        "the original edit receiver should remain connected"
    );
    assert!(!temp.path().join("untouched.pdf").exists());
}

#[test]
fn pdf_delete_only_page_keeps_feedback_without_dispatching_edit() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 1, Some(1));

    state.delete_current_page(3.5);

    assert_eq!(
        state.status_toast.message(3.5),
        Some("Cannot delete only page from PDF")
    );
    assert!(!state.edit_in_progress);
    assert!(state.edit_rx.is_none());
    assert!(!temp.path().join("fixture.delete.edited.pdf").exists());
}

#[test]
fn idle_r_rotates_selected_pdf_page_and_adopts_sibling_output() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("fixture.pdf");
    let output = temp.path().join("fixture.rotate.edited.pdf");
    write_two_page_pdf(&source);
    let source_bytes = std::fs::read(&source).unwrap();
    assert_eq!(get_pdf_info(&source).unwrap().page_count, 2);
    let (_, source_page_width, source_page_height) =
        render_pdf_page_preview(&source, 2, 64).expect("render original page two");
    assert!(source_page_width > source_page_height);

    let mut state = action_guard_state(source.clone(), 2, Some(2));
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut close = false;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 600.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::R,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );
    assert!(
        state.edit_in_progress,
        "idle R should dispatch the selected-page edit"
    );
    assert_eq!(state.selected_page, Some(2));

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while state.edit_in_progress && std::time::Instant::now() < deadline {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.poll_edit_worker(ctx, 1.0)
        });
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!state.edit_in_progress, "PDF rotate edit did not finish");
    assert_eq!(state.path, output);
    assert!(output.is_file());
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert_eq!(get_pdf_info(&output).unwrap().page_count, 2);
    assert_eq!(state.selected_page, Some(2));
    let (_, rotated_page_width, rotated_page_height) =
        render_pdf_page_preview(&output, 2, 64).expect("render rotated page two");
    assert!(rotated_page_width < rotated_page_height);

    while state.info_job.is_some() && std::time::Instant::now() < deadline {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.poll_info_worker(ctx, 1.0)
        });
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        state.info_job.is_none(),
        "edited PDF metadata did not reload"
    );
    assert_eq!(state.selected_page, Some(2));
    assert!(!close);
}

#[test]
fn escape_during_pdf_edit_keeps_session_until_synthetic_result_arrives() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.pdf");
    let mut state = action_guard_state(path.clone(), 0, None);
    let (sender, receiver) = channel();
    state.edit_rx = Some(receiver);
    state.edit_in_progress = true;
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    state.status_toast.set("Rotating page 1…", started_at);

    let ctx = egui::Context::default();
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let escape = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    let mut close = false;
    let busy_output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 600.0),
            )),
            events: vec![escape.clone()],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );

    assert!(!close, "Escape must not drop a running PDF edit");
    assert_eq!(state.path, path);
    assert!(state.edit_in_progress);
    assert!(state.edit_rx.is_some());
    assert!(
        busy_output.shapes.iter().any(|clipped| {
            matches!(
                &clipped.shape,
                egui::Shape::Text(text)
                    if text.galley.text().contains("PDF edit is still running")
            )
        }),
        "busy feedback should be rendered in the viewer footer"
    );
    let feedback_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert_eq!(
        state.status_toast.message(feedback_time),
        Some("PDF edit is still running; wait for it to finish")
    );

    sender
        .send(Ok(PdfEditMsg {
            operation: "rotate",
            page_num: 1,
            output: temp.path().join("source.rotate.edited.pdf"),
            staging_dir: tempfile::tempdir_in(temp.path()).unwrap(),
            result: Err("synthetic worker failure".to_owned()),
        }))
        .unwrap();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );
    assert!(!state.edit_in_progress);
    assert!(state.edit_rx.is_none());
    assert_eq!(state.path, path, "failed edit keeps the source document");
    let feedback_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert_eq!(
        state.status_toast.message(feedback_time),
        Some("Failed to rotate page: synthetic worker failure")
    );

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 600.0),
            )),
            events: vec![escape],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );
    assert!(
        close,
        "Escape may close the session after the edit resolves"
    );
}

#[test]
fn missing_pdf_metadata_completes_after_initial_view_creation() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = PdfEditorState::new(temp.path().join("missing.pdf"));
    assert_eq!(state.page_count, 0);
    assert!(state.info_job.is_some());

    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.info_job.is_some() && std::time::Instant::now() < deadline {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.poll_info_worker(ctx, 0.0)
        });
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(state.info_job.is_none(), "PDF metadata job did not finish");
    assert_eq!(state.page_count, 1);
    assert_eq!(state.selected_page, Some(1));
    assert!(state.status_toast.message(0.0).is_some());
}

#[test]
fn closed_viewer_discards_completed_edit_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("document.rotate.edited.pdf");
    let mut state = PdfEditorState::new(temp.path().join("document.pdf"));
    let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
    let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
    std::fs::write(&staged_output, b"staged edit").unwrap();
    let (sender, receiver) = channel();
    state.edit_rx = Some(receiver);
    sender
        .send(Ok(PdfEditMsg {
            operation: "rotate",
            page_num: 1,
            output: output.clone(),
            staging_dir,
            result: Ok(()),
        }))
        .unwrap();
    drop(state);

    assert!(!staged_output.exists());
    assert!(!output.exists());
}

#[test]
fn failed_edit_preserves_existing_sibling_output() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("document.rotate.edited.pdf");
    std::fs::write(&output, b"prior edit").unwrap();
    let mut state = PdfEditorState::new(temp.path().join("document.pdf"));
    let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
    let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
    std::fs::write(&staged_output, b"failed edit").unwrap();
    let (sender, receiver) = channel();
    state.edit_rx = Some(receiver);
    state.edit_in_progress = true;
    sender
        .send(Ok(PdfEditMsg {
            operation: "rotate",
            page_num: 1,
            output: output.clone(),
            staging_dir,
            result: Err("render failed".to_owned()),
        }))
        .unwrap();

    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        state.poll_edit_worker(ctx, 0.0)
    });
    assert_eq!(std::fs::read(output).unwrap(), b"prior edit");
    assert!(!staged_output.exists());
    assert!(!state.edit_in_progress);
}

#[test]
fn validated_edit_publishes_sibling_output_from_open_viewer() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("document.rotate.edited.pdf");
    let mut state = PdfEditorState::new(temp.path().join("document.pdf"));
    let staging_dir = tempfile::tempdir_in(temp.path()).unwrap();
    let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
    std::fs::write(&staged_output, b"validated edit").unwrap();
    let (sender, receiver) = channel();
    state.edit_rx = Some(receiver);
    state.edit_in_progress = true;
    sender
        .send(Ok(PdfEditMsg {
            operation: "rotate",
            page_num: 1,
            output: output.clone(),
            staging_dir,
            result: Ok(()),
        }))
        .unwrap();

    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        state.poll_edit_worker(ctx, 0.0)
    });
    assert_eq!(std::fs::read(&output).unwrap(), b"validated edit");
    assert!(!staged_output.exists());
    assert_eq!(state.path, output);
    assert!(state.dirty);
}

#[test]
fn protected_pdf_publication_preserves_an_existing_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let staged = dir.path().join("staged.pdf");
    let output = dir.path().join("source.rotate.edited.pdf");
    std::fs::write(&source, b"original").unwrap();
    std::fs::write(&staged, b"completed").unwrap();
    let state = PdfEditorState::new(source.clone()).with_protected_outputs();
    state.publish_output(&staged, &output).unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"completed");
    std::fs::remove_file(&staged).unwrap();
    std::fs::write(&staged, b"another result").unwrap();
    assert!(state.publish_output(&staged, &output).is_err());
    assert_eq!(std::fs::read(output).unwrap(), b"completed");
    assert_eq!(std::fs::read(source).unwrap(), b"original");
}
