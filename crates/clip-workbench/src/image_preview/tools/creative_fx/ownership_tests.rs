use super::*;

fn state() -> ImagePreviewState {
    let mut bytes = Vec::new();
    RgbaImage::new(8, 8)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let mut state = ImagePreviewState::from_bytes_with_meta(
        bytes,
        "image/png".into(),
        None,
        "Creative FX ownership".into(),
        &AppConfig::default(),
    )
    .unwrap();
    state.active_mode = ImageToolMode::CreativeFx;
    state
}

fn wait_render(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while state.creative_fx_state.is_rendering {
        state.poll_creative_fx_on(jobs);
        assert!(
            std::time::Instant::now() < deadline,
            "Creative FX render timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn occupy_executor(jobs: &BackgroundJobs) -> std::sync::mpsc::Sender<()> {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let _ = jobs.submit(std::time::Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    release_tx
}

fn fake_depth_output_script(
    directory: &std::path::Path,
    name: &str,
    output: &RgbaImage,
) -> std::path::PathBuf {
    let output_path = directory.join("depth-output.png");
    let mut png_bytes = Vec::new();
    output
        .write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    std::fs::write(&output_path, png_bytes).unwrap();
    let script = directory.join(name);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat >/dev/null\ncat '{}'\n",
            output_path.display()
        ),
    )
    .unwrap();
    script
}

fn wait_depth_mask(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while state.creative_fx_state.depth_estimation_in_progress {
        state.poll_creative_fx_on(jobs);
        assert!(
            std::time::Instant::now() < deadline,
            "fake depth helper timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn depth_mask_queue_full_retries_and_publishes_current_output() {
    let directory = tempfile::tempdir().unwrap();
    let output = RgbaImage::from_pixel(5, 3, image::Rgba([70, 110, 160, 255]));
    let script = fake_depth_output_script(directory.path(), "depth-output.sh", &output);
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let drained = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drained_job = drained.clone();
    let _queued = jobs.submit(std::time::Duration::MAX, move || {
        drained_job.store(true, std::sync::atomic::Ordering::Release);
    });
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.run_depth_estimation_for_fx_with_command_on(
        "sh".into(),
        vec![script.display().to_string()],
        &jobs,
    );
    let retained = std::sync::Arc::clone(
        &state
            .creative_fx_state
            .depth_request
            .as_ref()
            .unwrap()
            .snapshot
            .pixels,
    );
    state.poll_creative_fx_on(&jobs);
    let retry_at = state.creative_fx_state.depth_retry_at.unwrap();
    assert!(
        state
            .creative_fx_state
            .depth_queue_status
            .as_deref()
            .unwrap()
            .contains("waiting")
    );
    assert!(std::sync::Arc::ptr_eq(
        &retained,
        &state
            .creative_fx_state
            .depth_request
            .as_ref()
            .unwrap()
            .snapshot
            .pixels
    ));
    state.poll_creative_fx_on(&jobs);
    assert_eq!(state.creative_fx_state.depth_retry_at, Some(retry_at));
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !drained.load(std::sync::atomic::Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.creative_fx_state.depth_retry_at = Some(std::time::Instant::now());
    state.poll_creative_fx_on(&jobs);
    wait_depth_mask(&mut state, &jobs);
    assert_eq!(
        state.creative_fx_state.cached_depth_map.as_ref(),
        Some(&output)
    );
    assert!(state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert!(state.creative_fx_state.depth_estimation_error.is_none());
}

#[test]
fn queued_depth_mask_is_canceled_when_viewer_drops() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("started");
    let script = directory.path().join("queued-depth.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho started > '{}'\ncat >/dev/null\n",
            marker.display()
        ),
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let mut state = state();
    state.run_depth_estimation_for_fx_with_command_on(
        "sh".into(),
        vec![script.display().to_string()],
        &jobs,
    );
    assert!(state.creative_fx_state.depth_job_ticket.is_some());
    drop(state);
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let (_, rx) = jobs.submit(std::time::Duration::MAX, || ());
        match rx.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(Ok(())) => break,
            Ok(Err(JobError::QueueFull)) | Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                if std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            other => panic!("scheduler did not drain canceled depth mask: {other:?}"),
        }
    }
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn running_depth_mask_is_killed_and_reaped_when_viewer_drops() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("worker.pid");
    let script = directory.path().join("running-depth.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\ncat >/dev/null\nsleep 30\n",
            pid_file.display()
        ),
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.run_depth_estimation_for_fx_with_command_on(
        "sh".into(),
        vec![script.display().to_string()],
        &jobs,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !pid_file.exists() && std::time::Instant::now() < deadline {
        state.poll_creative_fx_on(&jobs);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pid_file.exists(), "synthetic depth helper did not start");
    let pid = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let control = state
        .creative_fx_state
        .depth_process_control
        .as_ref()
        .unwrap()
        .clone();
    drop(state);
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(pid);
}

#[test]
fn depth_result_after_intent_change_is_cached_without_changing_new_selection() {
    let directory = tempfile::tempdir().unwrap();
    let output = RgbaImage::from_pixel(4, 4, image::Rgba([30, 80, 120, 255]));
    let script = fake_depth_output_script(directory.path(), "late-depth.sh", &output);
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.run_depth_estimation_for_fx_with_command_on(
        "sh".into(),
        vec![script.display().to_string()],
        &jobs,
    );
    state.creative_fx_state.select_fx(CreativeFxType::Structure);
    wait_depth_mask(&mut state, &jobs);
    assert_eq!(
        state.creative_fx_state.cached_depth_map.as_ref(),
        Some(&output)
    );
    assert!(state.depth_mask_cache_is_current());
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
}

#[test]
fn current_depth_mask_failure_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("fail-depth.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ncat >/dev/null\necho synthetic depth failure >&2\nexit 7\n",
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.run_depth_estimation_for_fx_with_command_on(
        "sh".into(),
        vec![script.display().to_string()],
        &jobs,
    );
    wait_depth_mask(&mut state, &jobs);
    assert!(
        state
            .creative_fx_state
            .depth_estimation_error
            .as_deref()
            .unwrap()
            .contains("synthetic depth failure")
    );
    assert!(!state.creative_fx_state.depth_estimation_in_progress);
}

#[test]
fn alpha_mask_queue_full_retries_and_publishes_current_output() {
    let directory = tempfile::tempdir().unwrap();
    let output = RgbaImage::from_pixel(5, 3, image::Rgba([70, 110, 160, 128]));
    let script = fake_depth_output_script(directory.path(), "alpha-output.sh", &output);
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let drained = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drained_job = drained.clone();
    let _queued = jobs.submit(std::time::Duration::MAX, move || {
        drained_job.store(true, std::sync::atomic::Ordering::Release);
    });
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.bg_removal_cmd = Some(format!("sh {}", script.display()));
    state.run_alpha_mask_for_fx_on(&jobs);
    let retained = std::sync::Arc::clone(
        &state
            .creative_fx_state
            .alpha_request
            .as_ref()
            .unwrap()
            .snapshot
            .pixels,
    );
    state.poll_creative_fx_on(&jobs);
    let retry_at = state.creative_fx_state.alpha_retry_at.unwrap();
    assert!(
        state
            .creative_fx_state
            .alpha_queue_status
            .as_deref()
            .unwrap()
            .contains("waiting")
    );
    assert!(std::sync::Arc::ptr_eq(
        &retained,
        &state
            .creative_fx_state
            .alpha_request
            .as_ref()
            .unwrap()
            .snapshot
            .pixels
    ));
    state.poll_creative_fx_on(&jobs);
    assert_eq!(state.creative_fx_state.alpha_retry_at, Some(retry_at));
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !drained.load(std::sync::atomic::Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.creative_fx_state.alpha_retry_at = Some(std::time::Instant::now());
    state.poll_creative_fx_on(&jobs);
    wait_alpha_mask(&mut state, &jobs);
    assert_eq!(
        state.creative_fx_state.cached_background_removal.as_ref(),
        Some(&output)
    );
    assert_eq!(
        state.creative_fx_state.depth_mask_for_active_fx().source,
        CreativeFxMaskSource::BackgroundRemovalAlpha
    );
    assert!(state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert!(state.creative_fx_state.alpha_mask_error.is_none());
}

fn wait_alpha_mask(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while state.creative_fx_state.alpha_mask_in_progress {
        state.poll_creative_fx_on(jobs);
        assert!(
            std::time::Instant::now() < deadline,
            "fake alpha helper timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn queued_alpha_mask_is_canceled_when_viewer_drops() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("started");
    let script = directory.path().join("queued-alpha.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho started > '{}'\ncat >/dev/null\n",
            marker.display()
        ),
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let mut state = state();
    state.bg_removal_cmd = Some(format!("sh {}", script.display()));
    state.run_alpha_mask_for_fx_on(&jobs);
    assert!(state.creative_fx_state.alpha_job_ticket.is_some());
    drop(state);
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let (_, rx) = jobs.submit(std::time::Duration::MAX, || ());
        match rx.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(Ok(())) => break,
            Ok(Err(JobError::QueueFull)) | Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                if std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            other => panic!("scheduler did not drain canceled alpha mask: {other:?}"),
        }
    }
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn running_alpha_mask_is_killed_and_reaped_when_viewer_drops() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("worker.pid");
    let script = directory.path().join("running-alpha.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\ncat >/dev/null\nsleep 30\n",
            pid_file.display()
        ),
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.bg_removal_cmd = Some(format!("sh {}", script.display()));
    state.run_alpha_mask_for_fx_on(&jobs);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !pid_file.exists() && std::time::Instant::now() < deadline {
        state.poll_creative_fx_on(&jobs);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pid_file.exists(), "synthetic alpha helper did not start");
    let pid = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let control = state
        .creative_fx_state
        .alpha_process_control
        .as_ref()
        .unwrap()
        .clone();
    drop(state);
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(pid);
}

#[test]
fn alpha_result_after_intent_change_is_cached_without_changing_new_selection() {
    let directory = tempfile::tempdir().unwrap();
    let output = RgbaImage::from_pixel(4, 4, image::Rgba([30, 80, 120, 128]));
    let script = fake_depth_output_script(directory.path(), "late-alpha.sh", &output);
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.bg_removal_cmd = Some(format!("sh {}", script.display()));
    state.run_alpha_mask_for_fx_on(&jobs);
    state.creative_fx_state.select_fx(CreativeFxType::Structure);
    wait_alpha_mask(&mut state, &jobs);
    assert_eq!(
        state.creative_fx_state.cached_background_removal.as_ref(),
        Some(&output)
    );
    assert!(state.alpha_mask_cache_is_current());
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
}

#[test]
fn current_alpha_mask_failure_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("fail-alpha.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ncat >/dev/null\necho synthetic alpha failure >&2\nexit 7\n",
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.bg_removal_cmd = Some(format!("sh {}", script.display()));
    state.run_alpha_mask_for_fx_on(&jobs);
    wait_alpha_mask(&mut state, &jobs);
    assert!(
        state
            .creative_fx_state
            .alpha_mask_error
            .as_deref()
            .unwrap()
            .contains("synthetic alpha failure")
    );
    assert!(!state.creative_fx_state.alpha_mask_in_progress);
}

#[test]
fn render_saturation_retries_after_deadline_and_matches_direct_output() {
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let drained = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drained_job = drained.clone();
    let _queued = jobs.submit(std::time::Duration::MAX, move || {
        drained_job.store(true, std::sync::atomic::Ordering::Release);
    });
    let mut state = state();
    let input = state.current_rgba.clone();
    let depth = state.creative_fx_state.cached_depth_map.clone();
    let alpha = state.creative_fx_state.cached_background_removal.clone();
    let params = state.creative_fx_state.current_params();
    let expected = apply_creative_fx_with_masks_cancellable(
        &input,
        depth.as_ref(),
        alpha.as_ref(),
        &params,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();

    state.run_creative_fx_render_on(&jobs);
    state.poll_creative_fx_on(&jobs);
    let retry_at = state.creative_fx_state.render_retry_at.unwrap();
    assert!(retry_at > std::time::Instant::now());
    assert!(
        state
            .creative_fx_state
            .render_error
            .as_deref()
            .unwrap()
            .contains("Waiting")
    );
    state.poll_creative_fx_on(&jobs);
    assert_eq!(state.creative_fx_state.render_retry_at, Some(retry_at));
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !drained.load(std::sync::atomic::Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.creative_fx_state.render_retry_at = Some(std::time::Instant::now());
    state.poll_creative_fx_on(&jobs);
    wait_render(&mut state, &jobs);
    assert_eq!(
        state.creative_fx_state.preview_rgba.as_ref(),
        Some(&expected)
    );
    assert!(state.creative_fx_state.render_error.is_none());
}

#[test]
fn leaving_creative_fx_clears_cancelled_retry_status() {
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let _queued = jobs.submit(std::time::Duration::MAX, || ());
    let mut state = state();
    state.run_creative_fx_render_on(&jobs);
    state.poll_creative_fx_on(&jobs);
    assert!(state.creative_fx_state.render_retry_at.is_some());
    state.active_mode = ImageToolMode::View;
    state.poll_creative_fx_on(&jobs);
    assert!(state.creative_fx_state.render_retry_at.is_none());
    assert!(state.creative_fx_state.render_error.is_none());
    release.send(()).unwrap();
}

#[test]
fn queued_render_is_cancelled_when_viewer_drops() {
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let mut state = state();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_calls = calls.clone();
    state.run_creative_fx_render_with_on(&jobs, move |image, _, _, _, _| {
        worker_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(image.clone())
    });
    let ticket = state
        .creative_fx_state
        .render_job_ticket
        .as_ref()
        .unwrap()
        .clone();
    let cancelled = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    drop(state);
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    drop(ticket);
    release.send(()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn active_render_cancellation_suppresses_stale_output() {
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    state.run_creative_fx_render_with_on(&jobs, move |image, _, _, _, _| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Some(image.clone())
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let cancel = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    state.creative_fx_state.mark_dirty();
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    release_tx.send(()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    state.poll_creative_fx_on(&jobs);
    assert!(state.creative_fx_state.preview_rgba.is_none());
}

#[test]
fn thumbnail_queue_saturation_retries_incrementally_with_direct_pixel_parity() {
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let drained = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drained_job = drained.clone();
    let _queued = jobs.submit(std::time::Duration::MAX, move || {
        drained_job.store(true, std::sync::atomic::Ordering::Release);
    });
    let mut state = state();
    state.run_creative_fx_thumbnail_batch_on(&jobs);
    state.poll_creative_fx_on(&jobs);
    let retry_at = state.creative_fx_state.thumbnail_retry_at.unwrap();
    assert!(retry_at > std::time::Instant::now());
    assert!(
        state
            .creative_fx_state
            .thumbnail_queue_status
            .as_deref()
            .unwrap()
            .contains("Waiting")
    );
    assert!(!state.creative_fx_state.is_generating_thumbnails);
    state.poll_creative_fx_on(&jobs);
    assert_eq!(state.creative_fx_state.thumbnail_retry_at, Some(retry_at));
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !drained.load(std::sync::atomic::Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.creative_fx_state.thumbnail_retry_at = Some(std::time::Instant::now());
    state.poll_creative_fx_on(&jobs);
    assert!(state.creative_fx_state.is_generating_thumbnails);

    let thumb_base = image::imageops::resize(
        state.current_rgba.as_ref(),
        42,
        28,
        image::imageops::FilterType::Nearest,
    );
    let expected: Vec<_> = CreativeFxType::all()
        .into_iter()
        .map(|fx| {
            let preset = default_preset_for_fx(*fx);
            (*fx, apply_creative_fx(&thumb_base, &preset.params))
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    while state.creative_fx_state.is_generating_thumbnails {
        state.poll_creative_fx_on(&jobs);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        state.creative_fx_state.pending_thumbnails.len(),
        expected.len()
    );
    for ((actual_fx, actual), (expected_fx, expected)) in state
        .creative_fx_state
        .pending_thumbnails
        .iter()
        .zip(expected.iter())
    {
        assert_eq!(actual_fx, expected_fx);
        assert_eq!(actual, expected);
    }
    assert!(state.creative_fx_state.thumbnail_queue_status.is_none());
}

#[test]
fn queued_thumbnail_batch_cancels_on_replacement() {
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let mut state = state();
    state.run_creative_fx_thumbnail_batch_on(&jobs);
    let old_cancel = state
        .creative_fx_state
        .thumbnail_cancel
        .as_ref()
        .unwrap()
        .clone();
    let result_rx = state.creative_fx_state.thumbnail_job_rx.take().unwrap();
    state.cancel_creative_fx_thumbnail_batch();
    assert!(old_cancel.load(std::sync::atomic::Ordering::Acquire));
    release.send(()).unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        Err(JobError::Cancelled)
    );
}

#[test]
fn queued_thumbnail_batch_cancels_on_viewer_drop() {
    let jobs = BackgroundJobs::new(1, 1);
    let release = occupy_executor(&jobs);
    let mut state = state();
    state.run_creative_fx_thumbnail_batch_on(&jobs);
    let dropped_cancel = state
        .creative_fx_state
        .thumbnail_cancel
        .as_ref()
        .unwrap()
        .clone();
    let result_rx = state.creative_fx_state.thumbnail_job_rx.take().unwrap();
    drop(state);
    assert!(dropped_cancel.load(std::sync::atomic::Ordering::Acquire));
    release.send(()).unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        Err(JobError::Cancelled)
    );
}

#[test]
fn stale_thumbnail_batch_messages_are_rejected_after_pixel_revision_changes() {
    let mut state = state();
    let old_revision = state.pixel_revision;
    let old_epoch = 7;
    state.creative_fx_state.thumbnail_epoch = old_epoch;
    state.creative_fx_state.thumbnail_pixel_revision = Some(old_revision);
    state.creative_fx_state.thumbnail_rx = Some({
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(CreativeFxThumbnailMsg {
            epoch: old_epoch,
            pixel_revision: old_revision,
            fx_type: CreativeFxType::LensAberration,
            rgba: RgbaImage::from_pixel(42, 28, image::Rgba([8, 9, 10, 255])),
        })
        .unwrap();
        rx
    });
    state.creative_fx_state.is_generating_thumbnails = true;
    state.mark_pixels_changed();
    state.poll_creative_fx();
    assert!(state.creative_fx_state.pending_thumbnails.is_empty());
    assert!(state.creative_fx_state.fx_thumbnails.is_empty());
    assert_ne!(state.creative_fx_state.thumbnail_epoch, old_epoch);
}

#[test]
fn creative_render_tokens_cancel_on_edits_exit_and_drop_then_reentry_renders() {
    let mut state = state();
    state.run_creative_fx_render();
    let parameter_job = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    state.creative_fx_state.mark_dirty();
    assert!(parameter_job.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.render_rx.is_none());

    state.run_creative_fx_render();
    let pixel_job = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    state.mark_pixels_changed();
    assert!(pixel_job.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.creative_fx_state.is_rendering);

    state.run_creative_fx_render();
    let exit_job = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    state.set_tool_mode(ImageToolMode::View);
    assert!(exit_job.load(std::sync::atomic::Ordering::Acquire));
    state.set_tool_mode(ImageToolMode::CreativeFx);
    assert!(state.creative_fx_state.is_rendering);
    let drop_job = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    drop(state);
    assert!(drop_job.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn replacing_active_mask_cancels_old_creative_render() {
    let mut state = state();
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.run_creative_fx_render();
    let old_job = state
        .creative_fx_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    let stamp = state.current_creative_fx_mask_stamp();
    state.accept_creative_fx_depth_mask(
        stamp,
        RgbaImage::from_pixel(8, 8, image::Rgba([128, 128, 128, 255])),
        true,
    );
    assert!(old_job.load(std::sync::atomic::Ordering::Acquire));
    assert!(state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.render_cancel.is_some());
}

#[cfg(unix)]
fn sleeping_command(label: &str) -> (String, Vec<String>, std::path::PathBuf) {
    let script = std::env::temp_dir().join(format!(
        "clip-creative-fx-{label}-{}.py",
        std::process::id()
    ));
    std::fs::write(
        &script,
        "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
    )
    .unwrap();
    ("python3".into(), vec![script.display().to_string()], script)
}

#[cfg(unix)]
fn wait_for_process(state: &ImagePreviewState, alpha: bool) -> u32 {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let control = if alpha {
            state.creative_fx_state.alpha_process_control.as_ref()
        } else {
            state.creative_fx_state.depth_process_control.as_ref()
        }
        .expect("Creative FX process control should be installed");
        let pid = control.child_pid.load(std::sync::atomic::Ordering::SeqCst);
        if pid != 0 {
            return pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child PID was not published"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn assert_process_exited(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("Creative FX child {pid} remained alive after cancellation");
}

#[cfg(unix)]
#[test]
fn creative_fx_depth_and_alpha_processes_stop_on_stale_source_and_viewer_drop() {
    let mut depth_stale = state();
    let (bin, args, script) = sleeping_command("depth-stale");
    depth_stale.run_depth_estimation_for_fx_with_command(bin, args);
    let pid = wait_for_process(&depth_stale, false);
    let control = depth_stale
        .creative_fx_state
        .depth_process_control
        .as_ref()
        .unwrap()
        .clone();
    depth_stale.mark_pixels_changed();
    depth_stale.poll_creative_fx();
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(pid);
    let _ = std::fs::remove_file(script);

    let mut alpha_stale = state();
    let (bin, args, script) = sleeping_command("alpha-stale");
    alpha_stale.bg_removal_cmd = Some(format!("{bin} {}", args[0]));
    alpha_stale.run_alpha_mask_for_fx();
    let pid = wait_for_process(&alpha_stale, true);
    let control = alpha_stale
        .creative_fx_state
        .alpha_process_control
        .as_ref()
        .unwrap()
        .clone();
    alpha_stale.mark_pixels_changed();
    alpha_stale.poll_creative_fx();
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(pid);
    let _ = std::fs::remove_file(script);

    let mut depth_drop = state();
    let (bin, args, script) = sleeping_command("depth-drop");
    depth_drop.run_depth_estimation_for_fx_with_command(bin, args);
    let pid = wait_for_process(&depth_drop, false);
    let control = depth_drop
        .creative_fx_state
        .depth_process_control
        .as_ref()
        .unwrap()
        .clone();
    drop(depth_drop);
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(pid);
    let _ = std::fs::remove_file(script);

    let mut alpha_drop = state();
    let (bin, args, script) = sleeping_command("alpha-drop");
    alpha_drop.bg_removal_cmd = Some(format!("{bin} {}", args[0]));
    alpha_drop.run_alpha_mask_for_fx();
    let pid = wait_for_process(&alpha_drop, true);
    let control = alpha_drop
        .creative_fx_state
        .alpha_process_control
        .as_ref()
        .unwrap()
        .clone();
    drop(alpha_drop);
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_process_exited(pid);
    let _ = std::fs::remove_file(script);
}

fn install_render_message(state: &mut ImagePreviewState, message: CreativeFxMsg) {
    let stamp = match &message {
        CreativeFxMsg::Done { stamp, .. } | CreativeFxMsg::Error { stamp, .. } => stamp.clone(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.render_stamp = Some(stamp);
    state.creative_fx_state.render_rx = Some(rx);
    state.creative_fx_state.is_rendering = true;
    tx.send(Ok(Some(message))).unwrap();
}

#[test]
fn render_completion_accepts_current_success_and_discards_stale_success() {
    let mut current = state();
    let stamp = current.creative_fx_render_stamp();
    install_render_message(
        &mut current,
        CreativeFxMsg::Done {
            stamp,
            rgba: RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])),
            duration_ms: 4,
        },
    );
    current.poll_creative_fx();
    assert_eq!(
        current
            .creative_fx_state
            .preview_rgba
            .as_ref()
            .unwrap()
            .get_pixel(0, 0),
        &image::Rgba([1, 2, 3, 255])
    );
    assert_eq!(
        current.creative_fx_state.preview_stamp,
        Some(current.creative_fx_render_stamp())
    );

    let mut stale = state();
    let old_stamp = stale.creative_fx_render_stamp();
    install_render_message(
        &mut stale,
        CreativeFxMsg::Done {
            stamp: old_stamp,
            rgba: RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255])),
            duration_ms: 4,
        },
    );
    stale.creative_fx_state.lens_params.strength += 1.0;
    stale.creative_fx_state.mark_dirty();
    stale.creative_fx_state.lens_params.strength -= 1.0;
    stale.creative_fx_state.mark_dirty();

    stale.poll_creative_fx();

    assert!(stale.creative_fx_state.preview_rgba.is_none());
    assert!(!stale.creative_fx_state.is_rendering);
}

#[test]
fn render_failure_is_shown_only_for_current_stamp() {
    let mut current = state();
    let stamp = current.creative_fx_render_stamp();
    install_render_message(
        &mut current,
        CreativeFxMsg::Error {
            stamp,
            message: "current failure".into(),
        },
    );
    current.poll_creative_fx();
    assert_eq!(
        current.creative_fx_state.render_error.as_deref(),
        Some("current failure")
    );

    let mut stale = state();
    let old_stamp = stale.creative_fx_render_stamp();
    install_render_message(
        &mut stale,
        CreativeFxMsg::Error {
            stamp: old_stamp,
            message: "stale failure".into(),
        },
    );
    stale.creative_fx_state.select_fx(CreativeFxType::Structure);
    stale
        .creative_fx_state
        .select_fx(CreativeFxType::LensAberration);

    stale.poll_creative_fx();

    assert!(stale.creative_fx_state.render_error.is_none());
    assert!(!stale.creative_fx_state.is_rendering);
}

#[test]
fn mask_result_after_switch_is_cached_but_does_not_change_new_fx_settings() {
    let mut state = state();
    let stamp = state.current_creative_fx_mask_stamp();
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.depth_estimation_stamp = Some(stamp.clone());
    state.creative_fx_state.depth_estimation_rx = Some(rx);
    state.creative_fx_state.depth_estimation_in_progress = true;
    tx.send(CreativeFxMaskMsg {
        stamp,
        result: Ok(RgbaImage::from_pixel(8, 8, image::Rgba([50, 50, 50, 255]))),
    })
    .unwrap();
    state.creative_fx_state.select_fx(CreativeFxType::Structure);

    state.poll_creative_fx();

    assert!(state.creative_fx_state.cached_depth_map.is_some());
    assert!(state.depth_mask_cache_is_current());
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert!(!state.creative_fx_state.depth_estimation_in_progress);
    assert!(state.creative_fx_state.depth_estimation_rx.is_none());
}

#[test]
fn accepted_mask_completion_consumes_debounce_before_rendering() {
    let mut state = state();
    let stamp = state.current_creative_fx_mask_stamp();
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.depth_estimation_stamp = Some(stamp.clone());
    state.creative_fx_state.depth_estimation_rx = Some(rx);
    state.creative_fx_state.depth_estimation_in_progress = true;
    tx.send(CreativeFxMaskMsg {
        stamp,
        result: Ok(RgbaImage::from_pixel(8, 8, image::Rgba([64, 64, 64, 255]))),
    })
    .unwrap();

    state.poll_creative_fx();

    assert!(state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert!(state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.params_dirty_at.is_none());
}

#[test]
fn depth_result_after_reset_is_cached_without_reenabling_mask() {
    let mut state = state();
    let stamp = state.current_creative_fx_mask_stamp();
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.depth_estimation_stamp = Some(stamp.clone());
    state.creative_fx_state.depth_estimation_rx = Some(rx);
    state.creative_fx_state.depth_estimation_in_progress = true;

    // Match Reset Defaults while the worker is running.
    state.creative_fx_state.lens_params.strength = 80.0;
    state.creative_fx_state.lens_params = LensAberrationParams::default();
    state.creative_fx_state.selected_preset_name = "Default".to_string();
    state.creative_fx_state.mark_dirty();
    tx.send(CreativeFxMaskMsg {
        stamp,
        result: Ok(RgbaImage::from_pixel(8, 8, image::Rgba([80, 80, 80, 255]))),
    })
    .unwrap();

    state.poll_creative_fx();

    assert!(state.creative_fx_state.cached_depth_map.is_some());
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert!(!state.creative_fx_state.is_rendering);
}

#[test]
fn depth_result_after_preset_change_preserves_new_mask_intent() {
    let mut state = state();
    let stamp = state.current_creative_fx_mask_stamp();
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.depth_estimation_stamp = Some(stamp.clone());
    state.creative_fx_state.depth_estimation_rx = Some(rx);
    state.creative_fx_state.depth_estimation_in_progress = true;

    // A preset can choose the other source and leave masking off.
    state.creative_fx_state.selected_preset_name = "Alpha Portrait".to_string();
    state.creative_fx_state.lens_params.depth_mask = DepthMaskConfig {
        enabled: false,
        source: CreativeFxMaskSource::BackgroundRemovalAlpha,
        invert: true,
        threshold: 0.4,
        feather: 0.15,
    };
    state.creative_fx_state.mark_dirty();
    tx.send(CreativeFxMaskMsg {
        stamp,
        result: Ok(RgbaImage::from_pixel(8, 8, image::Rgba([80, 80, 80, 255]))),
    })
    .unwrap();

    state.poll_creative_fx();

    assert!(state.creative_fx_state.cached_depth_map.is_some());
    assert_eq!(
        state.creative_fx_state.depth_mask_for_active_fx(),
        &DepthMaskConfig {
            enabled: false,
            source: CreativeFxMaskSource::BackgroundRemovalAlpha,
            invert: true,
            threshold: 0.4,
            feather: 0.15,
        }
    );
    assert!(!state.creative_fx_state.is_rendering);
}

#[test]
fn alpha_result_after_reset_is_cached_without_reenabling_mask() {
    let mut state = state();
    let stamp = state.current_creative_fx_mask_stamp();
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.alpha_mask_stamp = Some(stamp.clone());
    state.creative_fx_state.alpha_mask_rx = Some(rx);
    state.creative_fx_state.alpha_mask_in_progress = true;
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.creative_fx_state.lens_params = LensAberrationParams::default();
    state.creative_fx_state.selected_preset_name = "Default".to_string();
    state.creative_fx_state.mark_dirty();
    tx.send(CreativeFxMaskMsg {
        stamp,
        result: Ok(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([128, 128, 128, 128]),
        )),
    })
    .unwrap();

    state.poll_creative_fx();

    assert!(state.creative_fx_state.cached_background_removal.is_some());
    assert!(!state.creative_fx_state.depth_mask_for_active_fx().enabled);
    assert!(!state.creative_fx_state.is_rendering);
}

#[test]
fn stale_mask_completion_is_discarded_and_old_cache_is_invalidated() {
    let mut state = state();
    let old_stamp = state.input_stamp();
    state.mark_pixels_changed();
    state.creative_fx_state.cached_depth_map =
        Some(RgbaImage::from_pixel(8, 8, image::Rgba([20, 20, 20, 255])));
    state.creative_fx_state.depth_map_input_stamp = Some(old_stamp.clone());
    state.creative_fx_state.depth_estimation_stamp =
        Some(state.creative_fx_state.mask_stamp(old_stamp.clone()));
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.depth_estimation_rx = Some(rx);
    state.creative_fx_state.depth_estimation_in_progress = true;
    tx.send(CreativeFxMaskMsg {
        stamp: state.creative_fx_state.mask_stamp(old_stamp),
        result: Err("old depth failure".into()),
    })
    .unwrap();
    state.mark_pixels_changed();

    state.poll_creative_fx();

    assert!(state.creative_fx_state.cached_depth_map.is_none());
    assert!(!state.depth_mask_cache_is_current());
    assert!(state.creative_fx_state.depth_estimation_error.is_none());
    assert!(!state.creative_fx_state.depth_estimation_in_progress);
    assert!(state.creative_fx_state.depth_estimation_rx.is_none());
}

#[test]
fn invalidating_stale_mask_disables_only_configs_using_that_source() {
    let mut state = state();
    let old_stamp = state.input_stamp();
    state.mark_pixels_changed();
    state.creative_fx_state.cached_depth_map =
        Some(RgbaImage::from_pixel(8, 8, image::Rgba([20, 20, 20, 255])));
    state.creative_fx_state.depth_map_input_stamp = Some(old_stamp);
    state.creative_fx_state.cached_background_removal = Some(RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([255, 255, 255, 255]),
    ));
    state.creative_fx_state.background_removal_input_stamp = Some(state.input_stamp());
    state.creative_fx_state.lens_params.depth_mask.enabled = true;
    state.creative_fx_state.lens_params.depth_mask.source = CreativeFxMaskSource::DepthMap;
    state
        .creative_fx_state
        .trichromatic_params
        .depth_mask
        .enabled = true;
    state
        .creative_fx_state
        .trichromatic_params
        .depth_mask
        .source = CreativeFxMaskSource::BackgroundRemovalAlpha;
    state.invalidate_stale_creative_fx_masks();

    assert!(state.creative_fx_state.cached_depth_map.is_none());
    assert!(state.creative_fx_state.cached_background_removal.is_some());
    assert!(!state.creative_fx_state.lens_params.depth_mask.enabled);
    assert!(
        state
            .creative_fx_state
            .trichromatic_params
            .depth_mask
            .enabled
    );
    assert!(state.creative_fx_state.params_dirty_at.is_none());
}

#[test]
fn reentering_creative_fx_after_pixel_edit_schedules_a_fresh_render() {
    let mut state = state();
    state.creative_fx_state.preview_rgba = Some(RgbaImage::new(8, 8));
    state.creative_fx_state.preview_stamp = Some(state.creative_fx_render_stamp());
    state.active_mode = ImageToolMode::Crop;
    state.mark_pixels_changed();
    state.creative_fx_state.params_dirty_at = Some(std::time::Instant::now());

    state.set_tool_mode(ImageToolMode::CreativeFx);

    assert!(state.creative_fx_state.is_rendering);
    assert!(state.creative_fx_state.params_dirty_at.is_none());
    assert_eq!(
        state.creative_fx_state.render_stamp,
        Some(state.creative_fx_render_stamp())
    );
}

#[test]
fn thumbnail_batch_drops_pending_textures_and_messages_for_old_pixels() {
    let ctx = egui::Context::default();
    let mut state = state();
    let old_revision = state.pixel_revision;
    let old_epoch = 4;
    state.creative_fx_state.thumbnail_epoch = old_epoch;
    state.creative_fx_state.thumbnail_pixel_revision = Some(old_revision);
    state.creative_fx_state.is_generating_thumbnails = true;
    state
        .creative_fx_state
        .pending_thumbnails
        .push((CreativeFxType::LensAberration, RgbaImage::new(2, 2)));
    let texture = ctx.load_texture(
        "stale_fx_thumbnail_test",
        egui::ColorImage::new([2, 2], Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    state
        .creative_fx_state
        .fx_thumbnails
        .insert(CreativeFxType::LensAberration, texture);
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.thumbnail_rx = Some(rx);
    tx.send(CreativeFxThumbnailMsg {
        epoch: old_epoch,
        pixel_revision: old_revision,
        fx_type: CreativeFxType::LensAberration,
        rgba: RgbaImage::new(2, 2),
    })
    .unwrap();
    state.mark_pixels_changed();

    state.poll_creative_fx();

    assert!(state.creative_fx_state.pending_thumbnails.is_empty());
    assert!(state.creative_fx_state.fx_thumbnails.is_empty());
    assert!(state.creative_fx_state.thumbnail_rx.is_none());
    assert!(!state.creative_fx_state.is_generating_thumbnails);
    assert_ne!(state.creative_fx_state.thumbnail_epoch, old_epoch);
}

#[test]
fn thumbnail_poller_keeps_queued_messages_before_disconnect() {
    let mut state = state();
    let epoch = 9;
    let pixel_revision = state.pixel_revision;
    state.creative_fx_state.thumbnail_epoch = epoch;
    state.creative_fx_state.thumbnail_pixel_revision = Some(pixel_revision);
    state.creative_fx_state.is_generating_thumbnails = true;
    let (tx, rx) = std::sync::mpsc::channel();
    state.creative_fx_state.thumbnail_rx = Some(rx);
    tx.send(CreativeFxThumbnailMsg {
        epoch,
        pixel_revision,
        fx_type: CreativeFxType::LensAberration,
        rgba: RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255])),
    })
    .unwrap();
    drop(tx);

    state.poll_creative_fx();

    assert_eq!(state.creative_fx_state.pending_thumbnails.len(), 1);
    assert!(!state.creative_fx_state.is_generating_thumbnails);
    assert!(state.creative_fx_state.thumbnail_rx.is_none());
}

#[test]
fn replacing_a_thumbnail_batch_cancels_only_its_own_run() {
    let mut state = state();
    state.run_creative_fx_thumbnail_batch();
    let old_run = state
        .creative_fx_state
        .thumbnail_cancel
        .as_ref()
        .unwrap()
        .clone();

    state.run_creative_fx_thumbnail_batch();
    let current_run = state
        .creative_fx_state
        .thumbnail_cancel
        .as_ref()
        .unwrap()
        .clone();

    assert!(old_run.load(std::sync::atomic::Ordering::Acquire));
    assert!(!current_run.load(std::sync::atomic::Ordering::Acquire));
    state.cancel_creative_fx_thumbnail_batch();
    assert!(current_run.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn apply_rejects_a_preview_from_an_old_source_stamp() {
    let mut state = state();
    state.creative_fx_state.preview_rgba =
        Some(RgbaImage::from_pixel(8, 8, image::Rgba([8, 8, 8, 255])));
    state.creative_fx_state.preview_stamp = Some(state.creative_fx_render_stamp());
    state.mark_pixels_changed();

    state.apply_creative_fx();

    assert!(state.undo_stack.is_empty());
    assert_eq!(state.pixel_revision, 1);
    assert_eq!(
        state.current_rgba.get_pixel(0, 0),
        &image::Rgba([0, 0, 0, 0])
    );
}
