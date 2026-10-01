use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn wait_for_pixel_resize(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while (state.pixel_resize_job.pending || state.pixel_resize_job.retry_at.is_some())
        && std::time::Instant::now() < deadline
    {
        state.poll_pixel_resize(&ctx, jobs);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_pixel_resize(&ctx, jobs);
    assert!(!state.pixel_resize_job.pending);
    assert!(state.pixel_resize_job.error.is_none());
}

fn state() -> ImagePreviewState {
    let mut bytes = Vec::new();
    RgbaImage::new(4, 4)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    ImagePreviewState::from_bytes_with_meta(
        bytes,
        "image/png".into(),
        None,
        "optimizer stamp test".into(),
        &AppConfig::default(),
    )
    .unwrap()
}

#[test]
fn bounded_pixel_resize_matches_direct_filter_output_and_preserves_markup() {
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    let source = RgbaImage::from_fn(7, 5, |x, y| {
        image::Rgba([(x * 31) as u8, (y * 47) as u8, (x * 7 + y * 9) as u8, 255])
    });
    state.current_rgba = Arc::new(source.clone());
    state.markup_state.items.push(MarkupShape::Rectangle {
        rect: Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.8, 0.9)),
        color: Color32::RED,
        width: 2.0,
        filled: false,
        rounding: 0.0,
    });
    let markup_before = state.markup_state.items.clone();
    let expected =
        image::imageops::resize(&source, 11, 9, ResizeFilter::Triangle.to_image_filter());

    state.apply_pixel_resize_on(11, 9, ResizeFilter::Triangle, &jobs);
    assert!(state.pixel_resize_job.pending);
    wait_for_pixel_resize(&mut state, &jobs);

    assert_eq!(state.current_rgba.as_ref(), &expected);
    assert_eq!(state.markup_state.items, markup_before);
    assert_eq!(state.mime, "image/png");
    assert_eq!(state.undo_stack.len(), 1);
    assert_eq!(
        (state.current_rgba.width(), state.current_rgba.height()),
        (11, 9)
    );
}

#[test]
fn resize_with_pending_redactions_matches_legacy_output_and_keeps_two_undo_steps() {
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    let source = RgbaImage::from_fn(8, 6, |x, y| {
        image::Rgba([(x * 20) as u8, (y * 30) as u8, 90, 255])
    });
    state.current_rgba = Arc::new(source.clone());
    let redactions = vec![RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::new(0.25, 0.25), Pos2::new(0.75, 0.75)),
    }];
    state.redaction_state.boxes = redactions.clone();
    state.active_mode = ImageToolMode::Resize;
    let baked = super::super::redact::compose_redactions_only(&source, &redactions);
    let expected = image::imageops::resize(&baked, 4, 3, ResizeFilter::Lanczos3.to_image_filter());

    state.apply_pixel_resize_on(4, 3, ResizeFilter::Lanczos3, &jobs);
    wait_for_pixel_resize(&mut state, &jobs);
    assert_eq!(state.current_rgba.as_ref(), &expected);
    assert!(state.redaction_state.boxes.is_empty());
    assert_eq!(state.undo_stack.len(), 2);
    state.undo();
    assert_eq!(state.current_rgba.as_ref(), &baked);
    state.undo();
    assert_eq!(state.current_rgba.as_ref(), &source);
}

#[test]
fn pixel_resize_queue_full_shows_retry_and_reuses_source_arc() {
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let (_, filler_rx) = jobs.submit(std::time::Duration::MAX, || ());
    let mut state = state();
    let before = state.current_rgba.clone();
    state.apply_pixel_resize_on(8, 7, ResizeFilter::Nearest, &jobs);
    let retained = Arc::clone(
        &state
            .pixel_resize_job
            .retry_request
            .as_ref()
            .unwrap()
            .source,
    );
    state.poll_pixel_resize(&egui::Context::default(), &jobs);
    assert!(!state.pixel_resize_job.pending);
    assert!(state.pixel_resize_job.retry_at.is_some());
    assert!(
        state
            .pixel_resize_job
            .status
            .as_deref()
            .unwrap()
            .contains("busy")
    );
    assert!(Arc::ptr_eq(
        &retained,
        &state
            .pixel_resize_job
            .retry_request
            .as_ref()
            .unwrap()
            .source
    ));
    assert!(Arc::ptr_eq(&before, &state.current_rgba));
    let retry_at = state.pixel_resize_job.retry_at.unwrap();
    state.poll_pixel_resize(&egui::Context::default(), &jobs);
    assert_eq!(state.pixel_resize_job.retry_at, Some(retry_at));

    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    filler_rx.recv().unwrap().unwrap();
    state.pixel_resize_job.retry_at = Some(std::time::Instant::now());
    state.poll_pixel_resize(&egui::Context::default(), &jobs);
    assert!(state.pixel_resize_job.pending);
    assert!(Arc::ptr_eq(
        &retained,
        &state
            .pixel_resize_job
            .retry_request
            .as_ref()
            .unwrap()
            .source
    ));
    wait_for_pixel_resize(&mut state, &jobs);
    assert!(state.pixel_resize_job.status.is_none());
}

#[test]
fn stale_resize_settings_cancel_queued_work_and_viewer_drop_cancels_ticket() {
    let jobs = BackgroundJobs::new(1, 2);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let mut active = state();
    active.apply_pixel_resize_on(9, 8, ResizeFilter::Nearest, &jobs);
    let cancel = active.pixel_resize_job.cancel.as_ref().unwrap().clone();
    let worker_rx = active.pixel_resize_job.rx.take().unwrap();
    active.resize_state.filter = ResizeFilter::Triangle;
    active.poll_pixel_resize(&egui::Context::default(), &jobs);
    assert!(cancel.load(Ordering::Acquire));
    assert!(!active.pixel_resize_job.pending);
    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    assert!(matches!(
        worker_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        Err(little_clip_core::JobError::Cancelled)
    ));

    let jobs = BackgroundJobs::new(1, 2);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let mut dropped = state();
    dropped.apply_pixel_resize_on(9, 8, ResizeFilter::Nearest, &jobs);
    let cancel = dropped.pixel_resize_job.cancel.as_ref().unwrap().clone();
    let worker_rx = dropped.pixel_resize_job.rx.take().unwrap();
    drop(dropped);
    assert!(cancel.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    assert!(matches!(
        worker_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        Err(little_clip_core::JobError::Cancelled)
    ));
}

fn result(color: [u8; 4]) -> FileSizeOptimizationResult {
    FileSizeOptimizationResult {
        target_bytes: 1024,
        achieved_bytes: 90,
        width: 4,
        height: 4,
        scale: 1.0,
        quality: 80,
        format: TargetFormat::Png,
        encoded_data: Arc::new(vec![1, 2, 3]),
        result_rgba: RgbaImage::from_pixel(4, 4, image::Rgba(color)),
    }
}

#[test]
fn optimizer_discards_stale_result_and_keeps_current_result_applicable() {
    let mut stale = state();
    stale.resize_state.target_size_unit_mb = false;
    stale.resize_state.target_size_val = 1.0;
    stale.resize_state.target_format = TargetFormat::Png;
    stale.resize_state.optimizer_request_stamp = Some(stale.current_optimizer_request_stamp());
    stale.resize_state.is_optimizing = true;
    stale.optimizer_status = Some("working".into());
    let cancelled = Arc::new(AtomicBool::new(false));
    stale.optimizer_cancel = Some(cancelled.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    stale.optimizer_rx = Some(rx);
    tx.send(OptimizerMsg::Done(result([1, 2, 3, 255]))).unwrap();
    stale.mark_pixels_changed();

    stale.poll_optimizer();

    assert!(stale.resize_state.last_result.is_none());
    assert!(!stale.resize_state.is_optimizing);
    assert!(stale.optimizer_status.is_none());
    assert!(stale.optimizer_rx.is_none());
    assert!(cancelled.load(Ordering::Acquire));

    let mut current = state();
    current.resize_state.target_size_unit_mb = false;
    current.resize_state.target_size_val = 1.0;
    current.resize_state.target_format = TargetFormat::Png;
    current.resize_state.optimizer_request_stamp = Some(current.current_optimizer_request_stamp());
    current.resize_state.is_optimizing = true;
    let (tx, rx) = std::sync::mpsc::channel();
    current.optimizer_rx = Some(rx);
    tx.send(OptimizerMsg::Done(result([7, 8, 9, 255]))).unwrap();

    current.poll_optimizer();

    assert!(current.resize_state.last_result.is_some());
    assert_eq!(
        current.resize_state.last_result_request_stamp,
        Some(current.current_optimizer_request_stamp())
    );
    let original_pixel = current.current_rgba.get_pixel(0, 0).to_owned();
    current.resize_state.target_size_val = 2.0;
    assert!(!current.optimized_result_is_current());
    current.apply_optimized_result();
    assert_eq!(current.current_rgba.get_pixel(0, 0), &original_pixel);
    current.resize_state.target_size_val = 1.0;
    current.apply_optimized_result();
    assert_eq!(
        current.current_rgba.get_pixel(0, 0),
        &image::Rgba([7, 8, 9, 255])
    );
    assert_eq!(current.pixel_revision, 1);
    assert!(current.resize_state.last_result_request_stamp.is_none());
}

#[test]
fn optimizer_discards_completion_when_target_format_or_size_changes() {
    for change_format in [false, true] {
        let mut state = state();
        state.resize_state.target_size_unit_mb = false;
        state.resize_state.target_size_val = 1.0;
        state.resize_state.target_format = TargetFormat::Png;
        state.resize_state.optimizer_request_stamp = Some(state.current_optimizer_request_stamp());
        state.resize_state.is_optimizing = true;
        state.optimizer_status = Some("working".into());
        let cancelled = Arc::new(AtomicBool::new(false));
        state.optimizer_cancel = Some(cancelled.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        state.optimizer_rx = Some(rx);
        tx.send(OptimizerMsg::Done(result([1, 2, 3, 255]))).unwrap();

        if change_format {
            state.resize_state.target_format = TargetFormat::Jpeg;
        } else {
            state.resize_state.target_size_val = 2.0;
        }
        state.poll_optimizer();

        assert!(state.resize_state.last_result.is_none());
        assert!(!state.resize_state.is_optimizing);
        assert!(state.optimizer_status.is_none());
        assert!(state.optimizer_rx.is_none());
        assert!(cancelled.load(Ordering::Acquire));
    }
}

#[test]
fn dropping_image_preview_cancels_optimizer() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut state = state();
    state.optimizer_cancel = Some(cancelled.clone());
    drop(state);
    assert!(cancelled.load(Ordering::Acquire));
}

#[test]
fn optimizer_stops_between_resample_and_encode() {
    let img = RgbaImage::from_fn(64, 64, |x, y| {
        image::Rgba([(x * 17) as u8, (y * 31) as u8, (x ^ y) as u8, 255])
    });
    let cancelled = AtomicBool::new(false);
    let progress = |status: String| {
        if status.contains("Resampling") {
            cancelled.store(true, Ordering::Release);
        }
    };

    let result =
        optimize_to_target_size_cancellable(&img, 1, TargetFormat::Png, Some(&progress), &|| {
            cancelled.load(Ordering::Acquire)
        });

    assert!(cancelled.load(Ordering::Acquire));
    assert!(result.is_none());
}

#[test]
fn optimizer_stops_after_jpeg_quality_search_is_cancelled() {
    let img = RgbaImage::from_fn(64, 64, |x, y| {
        image::Rgba([(x * 17) as u8, (y * 31) as u8, (x ^ y) as u8, 255])
    });
    let cancelled = AtomicBool::new(false);
    let progress = |status: String| {
        if status.contains("Trying quality") {
            cancelled.store(true, Ordering::Release);
        }
    };

    let result =
        optimize_to_target_size_cancellable(&img, 1, TargetFormat::Jpeg, Some(&progress), &|| {
            cancelled.load(Ordering::Acquire)
        });

    assert!(cancelled.load(Ordering::Acquire));
    assert!(result.is_none());
}

#[test]
fn bounded_optimizer_matches_direct_tiny_png_output_and_streams_progress() {
    let mut state = state();
    state.resize_state.target_format = TargetFormat::Png;
    state.resize_state.target_size_unit_mb = false;
    state.resize_state.target_size_val = 1.0;
    let image = state.current_display_image();
    let expected = optimize_to_target_size_cancellable(
        &image,
        state.resize_state.target_bytes(),
        TargetFormat::Png,
        None,
        &|| false,
    )
    .expect("the test optimizer cannot be cancelled");
    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    state.active_mode = ImageToolMode::Resize;
    state.resize_state.sub_tab = ResizeSubTab::FileSize;
    state.run_target_file_size_optimizer_on(&jobs);
    let saw_progress = matches!(
        state
            .optimizer_rx
            .as_ref()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        OptimizerMsg::Progress(status) if status.contains("full resolution")
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.resize_state.is_optimizing && std::time::Instant::now() < deadline {
        state.poll_optimizer_on(&jobs);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_optimizer_on(&jobs);
    let actual = state.resize_state.last_result.as_ref().unwrap();
    assert!(saw_progress);
    assert_eq!(actual.encoded_data, expected.encoded_data);
    assert_eq!(actual.result_rgba, expected.result_rgba);
    assert_eq!(actual.width, expected.width);
    assert_eq!(actual.height, expected.height);
}

#[test]
fn optimizer_queue_full_preserves_result_and_retries_after_delay() {
    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
    let mut state = state();
    state.active_mode = ImageToolMode::Resize;
    state.resize_state.sub_tab = ResizeSubTab::FileSize;
    state.resize_state.last_result = Some(result([2, 3, 4, 255]));
    state.resize_state.last_result_request_stamp = Some(state.current_optimizer_request_stamp());
    state.run_target_file_size_optimizer_on(&jobs);
    let retained_pixels =
        std::sync::Arc::clone(&state.optimizer_retry_snapshot.as_ref().unwrap().pixels);
    state.poll_optimizer_on(&jobs);

    assert!(!state.resize_state.is_optimizing);
    assert!(state.optimizer_retry_at.is_some());
    assert!(std::sync::Arc::ptr_eq(
        &retained_pixels,
        &state.optimizer_retry_snapshot.as_ref().unwrap().pixels
    ));
    assert!(
        state
            .optimizer_status
            .as_deref()
            .is_some_and(|status| status.contains("busy"))
    );
    assert_eq!(
        state
            .resize_state
            .last_result
            .as_ref()
            .unwrap()
            .encoded_data
            .as_slice(),
        [1, 2, 3]
    );
    let retry_deadline = state.optimizer_retry_at.unwrap();
    state.poll_optimizer_on(&jobs);
    assert!(!state.resize_state.is_optimizing);
    assert_eq!(state.optimizer_retry_at, Some(retry_deadline));

    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    queued_rx.recv().unwrap().unwrap();
    state.optimizer_retry_at = Some(std::time::Instant::now());
    state.poll_optimizer_on(&jobs);
    assert!(state.resize_state.is_optimizing);
    assert!(state.optimizer_retry_at.is_none());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.resize_state.is_optimizing && std::time::Instant::now() < deadline {
        state.poll_optimizer_on(&jobs);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_optimizer_on(&jobs);
    assert!(state.resize_state.last_result.is_some());
}

#[test]
fn optimizer_drop_cancels_queued_ticket_and_cooperative_flag() {
    let jobs = little_clip_core::BackgroundJobs::new(1, 2);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
    let mut state = state();
    state.run_target_file_size_optimizer_on(&jobs);
    let cancel = state.optimizer_cancel.as_ref().unwrap().clone();
    let worker_rx = state.optimizer_job_rx.take().unwrap();
    drop(state);
    assert!(cancel.load(Ordering::Acquire));

    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    queued_rx.recv().unwrap().unwrap();
    assert_eq!(
        worker_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        Err(little_clip_core::JobError::Cancelled)
    );
}

#[test]
fn optimizer_rejects_stale_retry_and_completion() {
    let mut retry = state();
    retry.active_mode = ImageToolMode::Resize;
    retry.resize_state.sub_tab = ResizeSubTab::FileSize;
    retry.optimizer_retry_stamp = Some(retry.current_optimizer_request_stamp());
    retry.optimizer_retry_at =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
    retry.optimizer_status = Some("waiting".into());
    retry.resize_state.target_size_val += 1.0;
    retry.poll_optimizer_on(&little_clip_core::BackgroundJobs::new(1, 1));
    assert!(retry.optimizer_retry_at.is_none());
    assert!(!retry.resize_state.is_optimizing);
    assert!(retry.optimizer_status.is_none());

    let mut stale = state();
    stale.resize_state.target_size_unit_mb = false;
    stale.resize_state.target_size_val = 1.0;
    stale.resize_state.target_format = TargetFormat::Png;
    stale.resize_state.optimizer_request_stamp = Some(stale.current_optimizer_request_stamp());
    stale.resize_state.is_optimizing = true;
    let cancelled = Arc::new(AtomicBool::new(false));
    stale.optimizer_cancel = Some(cancelled.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    stale.optimizer_rx = Some(rx);
    tx.send(OptimizerMsg::Done(result([9, 8, 7, 255]))).unwrap();
    stale.resize_state.target_size_val = 2.0;
    stale.poll_optimizer_on(&little_clip_core::BackgroundJobs::new(1, 1));
    assert!(stale.resize_state.last_result.is_none());
    assert!(cancelled.load(Ordering::Acquire));
}
