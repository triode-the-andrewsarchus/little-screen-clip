use super::*;

#[test]
fn inline_preview_defers_retry_until_deadline_and_does_not_admit_hidden_page() {
    let ctx = egui::Context::default();
    let state = raster_state(1, 16);
    let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
    let mut retrying = InlinePdfPagePreview::new(Arc::clone(&raster));
    retrying.retry_at = Some(std::time::Instant::now() + Duration::from_secs(1));

    for _ in 0..3 {
        show_inline_frame(&ctx, &mut retrying);
    }
    assert!(retrying.decode_slot.is_none());

    let mut hidden = InlinePdfPagePreview::new(raster);

    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_clip_rect(egui::Rect::NOTHING);
            hidden.show(ui, ctx, Vec2::new(64.0, 64.0));
        });
    });
    assert!(hidden.decode_slot.is_none());
}

#[test]
fn malformed_png_sets_page_error_without_repeated_work_or_losing_source() {
    let ctx = egui::Context::default();
    let mut state = raster_state(1, 16);
    let malformed = b"not a PNG".to_vec();
    let raster_path = state.pages[0].raster.as_ref().unwrap().path().to_owned();
    std::fs::write(&raster_path, &malformed).unwrap();
    wait_for_decode(&mut state, 1, &ctx);
    assert!(
        state.pages[0]
            .error
            .as_ref()
            .unwrap()
            .contains("PDF preview decode failed")
    );
    assert_eq!(std::fs::read(raster_path).unwrap(), malformed);
    state.decode_requests.clear();
    for _ in 0..3 {
        assert!(state.ensure_page_texture(1, &ctx).is_none());
        state.schedule_page_decode(&ctx);
        assert!(state.decode_job.is_none());
    }
}

#[test]
fn invisible_work_is_cancelled_and_retains_its_slot_until_acknowledged() {
    let ctx = egui::Context::default();
    let mut state = raster_state(2, 16);
    let (release, ticket) = blocked_decode(&mut state);
    state.selected_page = Some(2);
    state.ensure_page_texture(2, &ctx);
    state.schedule_page_decode(&ctx);
    let job = state.decode_job.as_ref().unwrap();
    assert!(job.cancelled);
    assert_eq!(job.ticket.revision(), ticket.revision());
    state.schedule_page_decode(&ctx);
    assert_eq!(
        state.decode_job.as_ref().unwrap().ticket.revision(),
        ticket.revision()
    );
    release.send(()).unwrap();
    wait_for_decode(&mut state, 2, &ctx);
    assert!(state.pages[0].error.is_none());
    assert!(state.pages[0].texture.is_none());
    assert!(state.ensure_page_texture(2, &ctx).is_some());
}

#[test]
fn dropping_preview_receiver_stops_worker_after_bounded_in_flight_page() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let rendered = Arc::new(AtomicUsize::new(0));
    let render_count = Arc::clone(&rendered);
    let jobs = BackgroundJobs::new(1, 1);
    let (tx, rx) = sync_channel(PREVIEW_QUEUE_CAPACITY);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (ticket, result_rx) = spawn_preview_worker(
        &jobs,
        PathBuf::from("fixture.pdf"),
        100,
        1,
        Arc::new(tempfile::tempdir().unwrap()),
        tx,
        Arc::clone(&cancelled),
        move |_, _, _| {
            render_count.fetch_add(1, Ordering::SeqCst);
            Ok((Vec::new(), 1, 1))
        },
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while rendered.load(Ordering::SeqCst) < PREVIEW_QUEUE_CAPACITY + 1
        && std::time::Instant::now() < deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(rendered.load(Ordering::SeqCst), PREVIEW_QUEUE_CAPACITY + 1);
    cancelled.store(true, Ordering::Release);
    ticket.cancel();
    drop(rx);
    assert_eq!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(JobError::Cancelled)
    );
    assert_eq!(rendered.load(Ordering::SeqCst), PREVIEW_QUEUE_CAPACITY + 1);
}

#[test]
fn saturated_preview_queue_reports_an_error_instead_of_loading_forever() {
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::MAX, || ());

    let mut state = raster_state(2, 16);
    for page in &mut state.pages {
        page.raster = None;
    }
    state.start_preview_worker_on(&jobs, |_, _, _| panic!("full queue must not render"));
    state.poll_preview_worker(&egui::Context::default());
    assert!(!state.previews_loading);
    assert!(state.preview_job.is_none());
    assert!(state.pages.iter().all(|page| {
        page.error
            .as_deref()
            .is_some_and(|error| error.contains("background job queue is full"))
    }));

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
fn dropping_editor_cancels_queued_preview_before_rendering() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let render_count = Arc::new(AtomicUsize::new(0));
    let worker_count = Arc::clone(&render_count);
    let mut state = raster_state(1, 16);
    state.start_preview_worker_on(&jobs, move |_, _, _| {
        worker_count.fetch_add(1, Ordering::SeqCst);
        Ok((vec![1], 1, 1))
    });
    let cache_path = state.raster_cache.as_ref().unwrap().path().to_owned();
    drop(state);

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    // A later job in the same single-worker queue proves the cancelled
    // preview has been dequeued, without relying on a scheduling delay.
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
    assert_eq!(render_count.load(Ordering::SeqCst), 0);
    assert!(!cache_path.exists());
}

#[test]
fn active_preview_publishes_selected_page_before_remaining_pages() {
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = raster_state(3, 16);
    state.selected_page = Some(3);
    for page in &mut state.pages {
        page.raster = None;
    }
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    state.start_preview_worker_on(&jobs, move |_, page_num, max_dim| {
        started_tx.send((page_num, max_dim)).unwrap();
        if page_num == 1 {
            release_rx.recv().unwrap();
        }
        Ok((vec![page_num as u8], 1, 1))
    });
    assert_eq!(
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (3, PAGE_RASTER_MAX_DIM)
    );
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while state.pages[2].raster.is_none() && std::time::Instant::now() < deadline {
        state.poll_preview_worker(&ctx);
        std::thread::yield_now();
    }
    assert!(state.pages[2].raster.is_some());
    assert!(state.pages[0].raster.is_none());
    assert!(state.pages[1].raster.is_none());
    assert_eq!(
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (1, 360)
    );

    release_tx.send(()).unwrap();
    while state.previews_loading && std::time::Instant::now() < deadline {
        state.poll_preview_worker(&ctx);
        std::thread::yield_now();
    }
    assert!(!state.previews_loading);
    assert_eq!(
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (2, 360)
    );
    assert!(state.pages.iter().all(|page| {
        page.raster
            .as_ref()
            .is_some_and(|raster| raster.path().exists())
    }));
}

#[test]
fn cancelling_active_render_discards_stale_page_and_releases_cache() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let jobs = BackgroundJobs::new(1, 1);
    let rendered = Arc::new(AtomicUsize::new(0));
    let render_count = Arc::clone(&rendered);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let mut state = raster_state(2, 16);
    for page in &mut state.pages {
        page.raster = None;
    }
    state.start_preview_worker_on(&jobs, move |_, _, _| {
        render_count.fetch_add(1, Ordering::SeqCst);
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok((vec![1], 1, 1))
    });
    let cache = Arc::clone(state.raster_cache.as_ref().unwrap());
    let cache_path = cache.path().to_owned();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    state.cancel_preview_worker();
    let (_, sentinel_rx) = jobs.submit(Duration::MAX, || ());
    release_tx.send(()).unwrap();
    sentinel_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(rendered.load(Ordering::SeqCst), 1);
    state.poll_preview_worker(&egui::Context::default());
    assert!(state.pages.iter().all(|page| page.raster.is_none()));
    assert!(!cache.path().join("page-1.png").exists());
    drop(state);
    drop(cache);
    assert!(!cache_path.exists());
}

#[test]
fn pdf_keyboard_page_actions_reach_the_guarded_selected_page_methods() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut state = action_guard_state(temp.path().join("fixture.pdf"), 1, Some(1));

    for key in [egui::Key::Delete, egui::Key::Backspace] {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 600.0),
                )),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );

        assert_eq!(
            state.status_toast.message(now),
            Some("Cannot delete only page from PDF"),
            "{key:?} should reach delete for selected page 1"
        );
        assert!(!state.edit_in_progress);
        assert!(state.edit_rx.is_none());
    }

    // An active edit is a safe guard for Rotate: the keyboard route reaches
    // the selected-page action but cannot start another PDF operation.
    let (_sender, receiver) = channel();
    state.page_count = 2;
    state.pages.push(PdfPageItem {
        page_num: 2,
        raster: None,
        texture: None,
        texture_last_used: 0,
        error: None,
    });
    state.selected_page = Some(2);
    state.edit_rx = Some(receiver);
    state.edit_in_progress = true;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
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
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );
    assert_eq!(
        state.status_toast.message(now),
        Some("A PDF edit is already in progress"),
        "R should reach rotate for the selected page"
    );
    assert!(state.edit_in_progress);
    assert!(state.edit_rx.is_some());
}
