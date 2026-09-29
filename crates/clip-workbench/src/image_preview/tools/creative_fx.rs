use super::super::*;
use crate::shell::{render_context_slider, slider_double_clicked};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};

/// Message sent back from background thread rendering full creative FX.
pub enum CreativeFxMsg {
    Done {
        stamp: CreativeFxRenderStamp,
        rgba: RgbaImage,
        duration_ms: u128,
    },
    Error {
        stamp: CreativeFxRenderStamp,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CreativeFxRenderStamp {
    pixel_revision: u64,
    fx_type: CreativeFxType,
    params: CreativeFxParams,
    mask_cache_revision: u64,
    params_revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct CreativeFxMaskStamp {
    input: ImageInputStamp,
    fx_type: CreativeFxType,
    selection_revision: u64,
    params_revision: u64,
}

#[derive(Debug)]
pub(crate) struct CreativeFxMaskMsg {
    stamp: CreativeFxMaskStamp,
    result: Result<RgbaImage, String>,
}

#[derive(Clone)]
struct CreativeFxMaskRequest {
    png_bytes: std::sync::Arc<Vec<u8>>,
    cmd_bin: String,
    cmd_args: Vec<String>,
    stamp: CreativeFxMaskStamp,
}

fn send_creative_fx_mask_result(
    tx: &std::sync::mpsc::Sender<CreativeFxMaskMsg>,
    stamp: &CreativeFxMaskStamp,
    result: Result<RgbaImage, String>,
) {
    let _ = tx.send(CreativeFxMaskMsg {
        stamp: stamp.clone(),
        result,
    });
}

#[cfg(test)]
mod ownership_tests {
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
                .png_bytes,
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
                .png_bytes
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
                .png_bytes,
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
                .png_bytes
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
}

/// Message sent back from low-priority background thread rendering mini 42x28 thumbnails.
pub struct CreativeFxThumbnailMsg {
    pub epoch: u64,
    pub pixel_revision: u64,
    pub fx_type: CreativeFxType,
    pub rgba: RgbaImage,
}

fn render_creative_fx_mask_loading(ui: &mut egui::Ui, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add(
            egui::Spinner::new()
                .size(crate::ui_ext::CONTROL_ICON_SIZE)
                .color(color),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(label)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .monospace()
                .color(color),
        );
    });
}

fn render_creative_fx_mask_button(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    loading: bool,
    width: f32,
    palette: &AppPalette,
) -> egui::Response {
    let fill = if active {
        hex_to_c32(&palette.diff_add)
    } else {
        hex_to_c32(&palette.colors.surface_panel)
    };
    let foreground = contrast_text_c32(
        fill,
        Some(hex_to_c32(&palette.colors.text_primary)),
        palette,
    );
    let button = egui::Button::new(
        egui::RichText::new(label)
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .strong()
            .color(foreground),
    )
    .fill(fill)
    .stroke(Stroke::new(1.0_f32, fill))
    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
    .min_size(Vec2::new(width, crate::ui_ext::CONTROL_HEIGHT));
    ui.add_enabled(!loading, button)
}

/// All state for the Creative FX tab.
pub struct CreativeFxState {
    pub selected_fx: CreativeFxType,
    pub lens_params: LensAberrationParams,
    pub trichromatic_params: TrichromaticParams,
    pub pixel_sort_params: PixelSortParams,
    pub cmyk_params: CmykHalftoneParams,
    pub dither_params: DitherPaletteParams,
    pub ntsc_params: NtscParams,
    pub structure_params: StructureParams,

    pub selected_preset_name: String,
    params_revision: u64,
    selection_revision: u64,

    // Preview state
    pub preview_rgba: Option<RgbaImage>,
    pub preview_texture: Option<egui::TextureHandle>,
    pub dirty_preview_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub last_render_duration_ms: u128,

    // Async render worker (Main preview image)
    pub is_rendering: bool,
    pub render_rx: Option<std::sync::mpsc::Receiver<Result<Option<CreativeFxMsg>, JobError>>>,
    render_stamp: Option<CreativeFxRenderStamp>,
    preview_stamp: Option<CreativeFxRenderStamp>,
    render_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    render_job_ticket: Option<JobTicket>,
    pub render_retry_at: Option<std::time::Instant>,
    pub render_error: Option<String>,

    // Async thumbnail worker (Sidebar 42x28 mini-photos for each FX)
    pub fx_thumbnails: std::collections::HashMap<CreativeFxType, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(CreativeFxType, RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<CreativeFxThumbnailMsg>>,
    thumbnail_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    pub thumbnail_epoch: u64,
    pub thumbnail_pixel_revision: Option<u64>,
    pub(in crate::image_preview) thumbnail_cancel:
        Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    thumbnail_job_ticket: Option<JobTicket>,
    pub thumbnail_retry_at: Option<std::time::Instant>,
    pub thumbnail_queue_status: Option<String>,
    pub is_generating_thumbnails: bool,

    // Debounce timer
    pub params_dirty_at: Option<std::time::Instant>,
    pub auto_fitted: bool,

    // Depth mask state
    pub cached_depth_map: Option<RgbaImage>,
    pub(crate) depth_map_input_stamp: Option<ImageInputStamp>,
    pub(super) mask_cache_revision: u64,
    pub depth_estimation_in_progress: bool,
    pub depth_estimation_rx: Option<std::sync::mpsc::Receiver<CreativeFxMaskMsg>>,
    depth_estimation_stamp: Option<CreativeFxMaskStamp>,
    depth_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    pub(super) depth_job_ticket: Option<JobTicket>,
    depth_request: Option<CreativeFxMaskRequest>,
    pub depth_retry_at: Option<std::time::Instant>,
    pub depth_queue_status: Option<String>,
    pub(super) depth_process_control: Option<std::sync::Arc<ImageToolProcessControl>>,
    pub depth_estimation_error: Option<String>,

    // Background-removal alpha mask state
    pub cached_background_removal: Option<RgbaImage>,
    pub(super) background_removal_input_stamp: Option<ImageInputStamp>,
    pub alpha_mask_in_progress: bool,
    pub alpha_mask_rx: Option<std::sync::mpsc::Receiver<CreativeFxMaskMsg>>,
    alpha_mask_stamp: Option<CreativeFxMaskStamp>,
    alpha_job_rx: Option<std::sync::mpsc::Receiver<Result<(), JobError>>>,
    pub(super) alpha_job_ticket: Option<JobTicket>,
    alpha_request: Option<CreativeFxMaskRequest>,
    pub alpha_retry_at: Option<std::time::Instant>,
    pub alpha_queue_status: Option<String>,
    pub(super) alpha_process_control: Option<std::sync::Arc<ImageToolProcessControl>>,
    pub alpha_mask_error: Option<String>,
}

impl Default for CreativeFxState {
    fn default() -> Self {
        Self {
            selected_fx: CreativeFxType::LensAberration,
            lens_params: LensAberrationParams::default(),
            trichromatic_params: TrichromaticParams::default(),
            pixel_sort_params: PixelSortParams::default(),
            cmyk_params: CmykHalftoneParams::default(),
            dither_params: DitherPaletteParams::default(),
            ntsc_params: NtscParams::default(),
            structure_params: StructureParams::default(),
            selected_preset_name: "Default".to_string(),
            params_revision: 0,
            selection_revision: 0,
            preview_rgba: None,
            preview_texture: None,
            dirty_preview_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            last_render_duration_ms: 0,
            is_rendering: false,
            render_rx: None,
            render_stamp: None,
            preview_stamp: None,
            render_cancel: None,
            render_job_ticket: None,
            render_retry_at: None,
            render_error: None,
            fx_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_job_rx: None,
            thumbnail_epoch: 0,
            thumbnail_pixel_revision: None,
            thumbnail_cancel: None,
            thumbnail_job_ticket: None,
            thumbnail_retry_at: None,
            thumbnail_queue_status: None,
            is_generating_thumbnails: false,
            params_dirty_at: None,
            auto_fitted: false,
            cached_depth_map: None,
            depth_map_input_stamp: None,
            mask_cache_revision: 0,
            depth_estimation_in_progress: false,
            depth_estimation_rx: None,
            depth_estimation_stamp: None,
            depth_job_rx: None,
            depth_job_ticket: None,
            depth_request: None,
            depth_retry_at: None,
            depth_queue_status: None,
            depth_process_control: None,
            depth_estimation_error: None,
            cached_background_removal: None,
            background_removal_input_stamp: None,
            alpha_mask_in_progress: false,
            alpha_mask_rx: None,
            alpha_mask_stamp: None,
            alpha_job_rx: None,
            alpha_job_ticket: None,
            alpha_request: None,
            alpha_retry_at: None,
            alpha_queue_status: None,
            alpha_process_control: None,
            alpha_mask_error: None,
        }
    }
}

impl CreativeFxState {
    pub fn mark_dirty(&mut self) {
        self.cancel_render();
        self.params_dirty_at = Some(std::time::Instant::now());
        self.params_revision = self.params_revision.wrapping_add(1);
    }

    pub(crate) fn cancel_render(&mut self) {
        let was_retry_waiting = self.render_retry_at.take().is_some();
        if let Some(ticket) = self.render_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.render_rx = None;
        self.render_stamp = None;
        self.is_rendering = false;
        if was_retry_waiting {
            self.render_error = None;
        }
    }

    fn select_fx(&mut self, fx_type: CreativeFxType) {
        if self.selected_fx != fx_type {
            self.selected_fx = fx_type;
            self.selection_revision = self.selection_revision.wrapping_add(1);
            self.mark_dirty();
        }
    }

    pub fn depth_mask_for_active_fx(&self) -> &DepthMaskConfig {
        match self.selected_fx {
            CreativeFxType::LensAberration => &self.lens_params.depth_mask,
            CreativeFxType::Trichromatic => &self.trichromatic_params.depth_mask,
            CreativeFxType::PixelSort => &self.pixel_sort_params.depth_mask,
            CreativeFxType::CmykHalftone => &self.cmyk_params.depth_mask,
            CreativeFxType::DitherPalette => &self.dither_params.depth_mask,
            CreativeFxType::Ntsc => &self.ntsc_params.depth_mask,
            CreativeFxType::Structure => &self.structure_params.depth_mask,
        }
    }

    pub fn depth_mask_for_active_fx_mut(&mut self) -> &mut DepthMaskConfig {
        match self.selected_fx {
            CreativeFxType::LensAberration => &mut self.lens_params.depth_mask,
            CreativeFxType::Trichromatic => &mut self.trichromatic_params.depth_mask,
            CreativeFxType::PixelSort => &mut self.pixel_sort_params.depth_mask,
            CreativeFxType::CmykHalftone => &mut self.cmyk_params.depth_mask,
            CreativeFxType::DitherPalette => &mut self.dither_params.depth_mask,
            CreativeFxType::Ntsc => &mut self.ntsc_params.depth_mask,
            CreativeFxType::Structure => &mut self.structure_params.depth_mask,
        }
    }

    pub fn current_params(&self) -> CreativeFxParams {
        match self.selected_fx {
            CreativeFxType::LensAberration => CreativeFxParams::LensAberration(self.lens_params),
            CreativeFxType::Trichromatic => {
                CreativeFxParams::Trichromatic(self.trichromatic_params)
            }
            CreativeFxType::PixelSort => CreativeFxParams::PixelSort(self.pixel_sort_params),
            CreativeFxType::CmykHalftone => CreativeFxParams::CmykHalftone(self.cmyk_params),
            CreativeFxType::DitherPalette => CreativeFxParams::DitherPalette(self.dither_params),
            CreativeFxType::Ntsc => CreativeFxParams::Ntsc(self.ntsc_params),
            CreativeFxType::Structure => CreativeFxParams::Structure(self.structure_params),
        }
    }

    fn render_stamp(&self, pixel_revision: u64) -> CreativeFxRenderStamp {
        CreativeFxRenderStamp {
            pixel_revision,
            fx_type: self.selected_fx,
            params: self.current_params(),
            mask_cache_revision: self.mask_cache_revision,
            params_revision: self.params_revision,
        }
    }

    fn mask_stamp(&self, input: ImageInputStamp) -> CreativeFxMaskStamp {
        CreativeFxMaskStamp {
            input,
            fx_type: self.selected_fx,
            selection_revision: self.selection_revision,
            params_revision: self.params_revision,
        }
    }

    fn disable_mask_source(&mut self, source: CreativeFxMaskSource) -> bool {
        let configs = [
            &mut self.lens_params.depth_mask,
            &mut self.trichromatic_params.depth_mask,
            &mut self.pixel_sort_params.depth_mask,
            &mut self.cmyk_params.depth_mask,
            &mut self.dither_params.depth_mask,
            &mut self.ntsc_params.depth_mask,
            &mut self.structure_params.depth_mask,
        ];
        configs.into_iter().fold(false, |changed, config| {
            if config.enabled && config.source == source {
                config.enabled = false;
                true
            } else {
                changed
            }
        })
    }
}

impl ImagePreviewState {
    fn invalidate_stale_creative_fx_masks(&mut self) {
        let current = self.input_stamp();
        let stale_depth = self.creative_fx_state.cached_depth_map.is_some()
            && self.creative_fx_state.depth_map_input_stamp.as_ref() != Some(&current);
        let stale_alpha = self.creative_fx_state.cached_background_removal.is_some()
            && self
                .creative_fx_state
                .background_removal_input_stamp
                .as_ref()
                != Some(&current);
        if stale_depth {
            self.clear_creative_fx_mask_cache(CreativeFxMaskSource::DepthMap);
        }
        if stale_alpha {
            self.clear_creative_fx_mask_cache(CreativeFxMaskSource::BackgroundRemovalAlpha);
        }
        self.finish_creative_fx_mask_invalidation(stale_depth, stale_alpha);
    }

    fn clear_creative_fx_mask_cache(&mut self, source: CreativeFxMaskSource) {
        match source {
            CreativeFxMaskSource::DepthMap => {
                self.creative_fx_state.cached_depth_map = None;
                self.creative_fx_state.depth_map_input_stamp = None;
            }
            CreativeFxMaskSource::BackgroundRemovalAlpha => {
                self.creative_fx_state.cached_background_removal = None;
                self.creative_fx_state.background_removal_input_stamp = None;
            }
        }
    }

    fn finish_creative_fx_mask_invalidation(&mut self, stale_depth: bool, stale_alpha: bool) {
        if !stale_depth && !stale_alpha {
            return;
        }
        let active_config = *self.creative_fx_state.depth_mask_for_active_fx();
        let active_source_invalidated = (stale_depth
            && active_config.enabled
            && active_config.source == CreativeFxMaskSource::DepthMap)
            || (stale_alpha
                && active_config.enabled
                && active_config.source == CreativeFxMaskSource::BackgroundRemovalAlpha);
        let disabled_depth_configs = stale_depth
            && self
                .creative_fx_state
                .disable_mask_source(CreativeFxMaskSource::DepthMap);
        let disabled_alpha_configs = stale_alpha
            && self
                .creative_fx_state
                .disable_mask_source(CreativeFxMaskSource::BackgroundRemovalAlpha);
        self.schedule_mask_preview_after_invalidation(
            active_source_invalidated,
            disabled_depth_configs || disabled_alpha_configs,
        );
    }

    fn schedule_mask_preview_after_invalidation(
        &mut self,
        active_source_invalidated: bool,
        any_config_disabled: bool,
    ) {
        if !active_source_invalidated || !any_config_disabled {
            return;
        }
        self.creative_fx_state.mask_cache_revision =
            self.creative_fx_state.mask_cache_revision.wrapping_add(1);
        self.creative_fx_state.mark_dirty();
        let preview_needs_refresh = self.active_mode == ImageToolMode::CreativeFx
            && (self.creative_fx_state.preview_rgba.is_some()
                || self.creative_fx_state.is_rendering);
        if !preview_needs_refresh {
            self.creative_fx_state.params_dirty_at = None;
        }
    }

    fn current_creative_fx_mask_stamp(&self) -> CreativeFxMaskStamp {
        self.creative_fx_state.mask_stamp(self.input_stamp())
    }

    fn creative_fx_render_stamp(&self) -> CreativeFxRenderStamp {
        self.creative_fx_state.render_stamp(self.pixel_revision)
    }

    pub(crate) fn creative_fx_preview_is_current(&self) -> bool {
        self.creative_fx_state.preview_stamp.as_ref() == Some(&self.creative_fx_render_stamp())
    }

    fn creative_fx_mask_source_matches(
        &self,
        expected: Option<&CreativeFxMaskStamp>,
        actual: &CreativeFxMaskStamp,
    ) -> bool {
        expected == Some(actual) && actual.input == self.input_stamp()
    }

    fn creative_fx_mask_intent_matches(&self, stamp: &CreativeFxMaskStamp) -> bool {
        stamp.params_revision == self.creative_fx_state.params_revision
            && stamp.fx_type == self.creative_fx_state.selected_fx
            && stamp.selection_revision == self.creative_fx_state.selection_revision
    }

    fn depth_mask_cache_is_current(&self) -> bool {
        self.creative_fx_state.cached_depth_map.is_some()
            && self.creative_fx_state.depth_map_input_stamp.as_ref() == Some(&self.input_stamp())
    }

    fn alpha_mask_cache_is_current(&self) -> bool {
        self.creative_fx_state.cached_background_removal.is_some()
            && self
                .creative_fx_state
                .background_removal_input_stamp
                .as_ref()
                == Some(&self.input_stamp())
    }

    fn invalidate_stale_creative_fx_thumbnails(&mut self) {
        if self
            .creative_fx_state
            .thumbnail_pixel_revision
            .is_some_and(|revision| revision != self.pixel_revision)
        {
            self.cancel_creative_fx_thumbnail_batch();
            self.creative_fx_state.thumbnail_epoch =
                self.creative_fx_state.thumbnail_epoch.wrapping_add(1);
            self.creative_fx_state.thumbnail_pixel_revision = None;
            self.creative_fx_state.thumbnail_rx = None;
            self.creative_fx_state.pending_thumbnails.clear();
            self.creative_fx_state.fx_thumbnails.clear();
            self.creative_fx_state.is_generating_thumbnails = false;
        }
    }

    pub(crate) fn cancel_creative_fx_thumbnail_batch(&mut self) {
        if let Some(ticket) = self.creative_fx_state.thumbnail_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.creative_fx_state.thumbnail_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.creative_fx_state.thumbnail_epoch =
            self.creative_fx_state.thumbnail_epoch.wrapping_add(1);
        self.creative_fx_state.thumbnail_rx = None;
        self.creative_fx_state.thumbnail_job_rx = None;
        self.creative_fx_state.is_generating_thumbnails = false;
        self.creative_fx_state.thumbnail_retry_at = None;
        self.creative_fx_state.thumbnail_queue_status = None;
    }

    pub fn run_creative_fx_render(&mut self) {
        self.run_creative_fx_render_with_on(
            BackgroundJobs::global(),
            |image, depth, alpha, params, cancel| {
                apply_creative_fx_with_masks_cancellable(image, depth, alpha, params, cancel)
            },
        );
    }

    fn run_creative_fx_render_on(&mut self, jobs: &BackgroundJobs) {
        self.run_creative_fx_render_with_on(jobs, |image, depth, alpha, params, cancel| {
            apply_creative_fx_with_masks_cancellable(image, depth, alpha, params, cancel)
        });
    }

    fn run_creative_fx_render_with_on<F>(&mut self, jobs: &BackgroundJobs, processor: F)
    where
        F: FnOnce(
                &RgbaImage,
                Option<&RgbaImage>,
                Option<&RgbaImage>,
                &CreativeFxParams,
                &std::sync::atomic::AtomicBool,
            ) -> Option<RgbaImage>
            + Send
            + 'static,
    {
        if self.creative_fx_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }

        self.invalidate_stale_creative_fx_masks();

        let input_rgba = self.current_rgba.clone();
        let stamp = self.creative_fx_render_stamp();
        let params = self.creative_fx_state.current_params();
        let cached_depth = self.creative_fx_state.cached_depth_map.clone();
        let cached_background_removal = self.creative_fx_state.cached_background_removal.clone();

        self.creative_fx_state.render_stamp = Some(stamp.clone());
        self.creative_fx_state.is_rendering = true;
        self.creative_fx_state.render_error = None;
        self.creative_fx_state.render_retry_at = None;
        self.creative_fx_state.params_dirty_at = None;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.creative_fx_state.render_cancel = Some(cancelled.clone());
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                processor(
                    &input_rgba,
                    cached_depth.as_ref(),
                    cached_background_removal.as_ref(),
                    &params,
                    &cancelled,
                )
            }));
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            match process_result {
                Ok(Some(out_rgba)) => {
                    let duration_ms = start.elapsed().as_millis();
                    Some(CreativeFxMsg::Done {
                        stamp,
                        rgba: out_rgba,
                        duration_ms,
                    })
                }
                Ok(None) => None,
                Err(e) => {
                    let err_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Creative FX processing panicked".to_string()
                    };
                    Some(CreativeFxMsg::Error {
                        stamp,
                        message: err_msg,
                    })
                }
            }
        });
        self.creative_fx_state.render_job_ticket = Some(ticket);
        self.creative_fx_state.render_rx = Some(rx);
    }

    /// Spawn background thread to estimate AI depth map using Depth-Anything V2.
    pub fn run_depth_estimation_for_fx(&mut self) {
        let depth_script = "scripts/depth_estimation.py";
        let (cmd_bin, cmd_args) = if std::path::Path::new(depth_script).exists() {
            (depth_script.to_string(), Vec::new())
        } else {
            ("python3".to_string(), vec![depth_script.to_string()])
        };
        self.run_depth_estimation_for_fx_with_command(cmd_bin, cmd_args);
    }

    fn run_depth_estimation_for_fx_with_command(&mut self, cmd_bin: String, cmd_args: Vec<String>) {
        self.run_depth_estimation_for_fx_with_command_on(
            cmd_bin,
            cmd_args,
            BackgroundJobs::global(),
        );
    }

    fn run_depth_estimation_for_fx_with_command_on(
        &mut self,
        cmd_bin: String,
        cmd_args: Vec<String>,
        jobs: &BackgroundJobs,
    ) {
        if self.creative_fx_state.depth_estimation_in_progress || self.current_rgba.is_empty() {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            self.creative_fx_state.depth_estimation_error =
                Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        let stamp = self.current_creative_fx_mask_stamp();
        self.creative_fx_state.depth_estimation_stamp = Some(stamp.clone());
        self.creative_fx_state.depth_estimation_in_progress = true;
        self.creative_fx_state.depth_estimation_error = None;
        self.creative_fx_state.depth_queue_status = None;
        self.creative_fx_state.depth_retry_at = None;
        self.creative_fx_state.depth_request = Some(CreativeFxMaskRequest {
            png_bytes: std::sync::Arc::new(png_bytes),
            cmd_bin,
            cmd_args,
            stamp,
        });
        self.submit_creative_fx_depth_attempt(jobs);
    }

    fn submit_creative_fx_depth_attempt(&mut self, jobs: &BackgroundJobs) {
        let Some(request) = self.creative_fx_state.depth_request.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxMaskMsg>();
        self.creative_fx_state.depth_estimation_rx = Some(rx);
        self.creative_fx_state.depth_retry_at = None;
        self.creative_fx_state.depth_queue_status = None;
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        self.creative_fx_state.depth_process_control = Some(control.clone());
        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let mut cmd = Command::new(&request.cmd_bin);
            cmd.args(&request.cmd_args);
            let result = super::background_removal::run_image_tool_subprocess(
                cmd,
                &request.png_bytes,
                None,
                Some(&control.cancel),
                Some(&control.child_pid),
                BgRemovalOperation::DepthMap,
                &request.cmd_bin,
            )
            .and_then(super::background_removal::decode_image_tool_output);
            if result.is_err() && control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            send_creative_fx_mask_result(&tx, &request.stamp, result);
        });
        self.creative_fx_state.depth_job_ticket = Some(ticket);
        self.creative_fx_state.depth_job_rx = Some(job_rx);
    }

    /// Spawn automatic FeyNoBg removal to provide an alpha mask for Creative FX.
    pub fn run_alpha_mask_for_fx(&mut self) {
        self.run_alpha_mask_for_fx_on(BackgroundJobs::global());
    }

    fn run_alpha_mask_for_fx_on(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.alpha_mask_in_progress || self.current_rgba.is_empty() {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(error) = base_img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            self.creative_fx_state.alpha_mask_error =
                Some(format!("Failed to encode image to PNG: {}", error));
            return;
        }

        let (cmd_bin, cmd_args) =
            super::background_removal::background_removal_command(self.bg_removal_cmd.as_deref());
        let stamp = self.current_creative_fx_mask_stamp();
        self.creative_fx_state.alpha_mask_stamp = Some(stamp.clone());
        self.creative_fx_state.alpha_mask_in_progress = true;
        self.creative_fx_state.alpha_mask_error = None;
        self.creative_fx_state.alpha_queue_status = None;
        self.creative_fx_state.alpha_retry_at = None;
        self.creative_fx_state.alpha_request = Some(CreativeFxMaskRequest {
            png_bytes: std::sync::Arc::new(png_bytes),
            cmd_bin,
            cmd_args,
            stamp,
        });
        self.submit_creative_fx_alpha_attempt(jobs);
    }

    fn submit_creative_fx_alpha_attempt(&mut self, jobs: &BackgroundJobs) {
        let Some(request) = self.creative_fx_state.alpha_request.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxMaskMsg>();
        self.creative_fx_state.alpha_mask_rx = Some(rx);
        self.creative_fx_state.alpha_retry_at = None;
        self.creative_fx_state.alpha_queue_status = None;
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        self.creative_fx_state.alpha_process_control = Some(control.clone());
        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let mut cmd = Command::new(&request.cmd_bin);
            cmd.args(&request.cmd_args);
            let result = super::background_removal::run_image_tool_subprocess(
                cmd,
                &request.png_bytes,
                None,
                Some(&control.cancel),
                Some(&control.child_pid),
                BgRemovalOperation::BackgroundRemoval,
                &request.cmd_bin,
            )
            .and_then(super::background_removal::decode_image_tool_output);
            if result.is_err() && control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            send_creative_fx_mask_result(&tx, &request.stamp, result);
        });
        self.creative_fx_state.alpha_job_ticket = Some(ticket);
        self.creative_fx_state.alpha_job_rx = Some(job_rx);
    }

    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all FX.
    pub fn run_creative_fx_thumbnail_batch(&mut self) {
        self.run_creative_fx_thumbnail_batch_on(BackgroundJobs::global());
    }

    fn run_creative_fx_thumbnail_batch_on(&mut self, jobs: &BackgroundJobs) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.cancel_creative_fx_thumbnail_batch();
        self.creative_fx_state.thumbnail_epoch =
            self.creative_fx_state.thumbnail_epoch.wrapping_add(1);
        let epoch = self.creative_fx_state.thumbnail_epoch;
        let pixel_revision = self.pixel_revision;
        self.creative_fx_state.thumbnail_pixel_revision = Some(pixel_revision);

        // Downscale base image to 42x28 (sub-millisecond)
        let thumb_base = image::imageops::resize(
            self.current_rgba.as_ref(),
            42,
            28,
            image::imageops::FilterType::Nearest,
        );

        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxThumbnailMsg>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed: std::collections::HashSet<_> = self
            .creative_fx_state
            .fx_thumbnails
            .keys()
            .copied()
            .chain(
                self.creative_fx_state
                    .pending_thumbnails
                    .iter()
                    .map(|(fx, _)| *fx),
            )
            .collect();
        let todo: Vec<_> = CreativeFxType::all()
            .into_iter()
            .copied()
            .filter(|fx| !completed.contains(fx))
            .collect();
        if todo.is_empty() {
            return;
        }
        self.creative_fx_state.thumbnail_cancel = Some(cancelled.clone());
        self.creative_fx_state.thumbnail_rx = Some(rx);
        self.creative_fx_state.is_generating_thumbnails = true;
        self.creative_fx_state.thumbnail_queue_status = None;
        self.creative_fx_state.thumbnail_retry_at = None;

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            super::run_thumbnail_items(
                todo,
                || cancelled.load(std::sync::atomic::Ordering::Acquire),
                |fx| {
                    let preset = default_preset_for_fx(fx);
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        apply_creative_fx(&thumb_base, &preset.params)
                    }));
                    if let Ok(rgba) = res {
                        tx.send(CreativeFxThumbnailMsg {
                            epoch,
                            pixel_revision,
                            fx_type: fx,
                            rgba,
                        })
                        .is_ok()
                    } else {
                        true
                    }
                },
            );
        });
        self.creative_fx_state.thumbnail_job_ticket = Some(ticket);
        self.creative_fx_state.thumbnail_job_rx = Some(job_rx);
    }

    /// Poll each Creative FX worker independently so result ownership and cleanup stay local.
    pub fn poll_creative_fx(&mut self) {
        self.poll_creative_fx_on(BackgroundJobs::global());
    }

    fn poll_creative_fx_on(&mut self, jobs: &BackgroundJobs) {
        self.invalidate_stale_creative_fx_masks();
        self.invalidate_stale_creative_fx_thumbnails();
        self.poll_creative_fx_render_on(jobs);
        self.poll_creative_fx_depth_mask(jobs);
        self.poll_creative_fx_alpha_mask(jobs);
        self.poll_creative_fx_thumbnail_job();
        self.poll_creative_fx_thumbnails();
        self.retry_creative_fx_thumbnail_batch_if_due(jobs);
    }

    fn poll_creative_fx_thumbnail_job(&mut self) {
        let Some(rx) = self.creative_fx_state.thumbnail_job_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Err(JobError::QueueFull)) => {
                self.cancel_creative_fx_thumbnail_batch();
                self.creative_fx_state.thumbnail_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.creative_fx_state.thumbnail_queue_status =
                    Some("Waiting for background workers; retrying thumbnail batch shortly".into());
            }
            Ok(Err(error)) => {
                self.creative_fx_state.thumbnail_job_ticket = None;
                self.creative_fx_state.is_generating_thumbnails = false;
                self.creative_fx_state.thumbnail_queue_status =
                    Some(format!("Thumbnail worker failed: {error}"));
            }
            Ok(Ok(())) => self.creative_fx_state.thumbnail_job_ticket = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.thumbnail_job_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.creative_fx_state.thumbnail_job_ticket = None;
                self.creative_fx_state.is_generating_thumbnails = false;
                self.creative_fx_state.thumbnail_queue_status =
                    Some("Thumbnail worker disconnected".into());
            }
        }
    }

    fn retry_creative_fx_thumbnail_batch_if_due(&mut self, jobs: &BackgroundJobs) -> bool {
        if self.current_rgba.is_empty()
            || self.creative_fx_state.is_generating_thumbnails
            || self
                .creative_fx_state
                .thumbnail_retry_at
                .is_none_or(|retry_at| std::time::Instant::now() < retry_at)
            || self.creative_fx_state.fx_thumbnails.len()
                + self.creative_fx_state.pending_thumbnails.len()
                >= CreativeFxType::all().len()
            || self.creative_fx_state.thumbnail_pixel_revision != Some(self.pixel_revision)
        {
            return false;
        }
        self.creative_fx_state.thumbnail_retry_at = None;
        self.run_creative_fx_thumbnail_batch_on(jobs);
        true
    }

    fn poll_creative_fx_render_on(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.render_retry_at.is_some()
            && self.active_mode != ImageToolMode::CreativeFx
        {
            self.creative_fx_state.cancel_render();
        }
        if self
            .creative_fx_state
            .render_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &self.creative_fx_render_stamp())
            || (self.creative_fx_state.is_rendering
                && self.active_mode != ImageToolMode::CreativeFx)
        {
            self.creative_fx_state.cancel_render();
        }
        if self.creative_fx_state.render_retry_at.is_some()
            && self.creative_fx_state.render_stamp.as_ref()
                != Some(&self.creative_fx_render_stamp())
        {
            self.creative_fx_state.render_retry_at = None;
            self.creative_fx_state.render_stamp = None;
            self.creative_fx_state.render_error = None;
            if self.active_mode == ImageToolMode::CreativeFx {
                self.creative_fx_state.params_dirty_at = Some(std::time::Instant::now());
            }
        }
        if self
            .creative_fx_state
            .render_retry_at
            .is_some_and(|retry_at| {
                std::time::Instant::now() >= retry_at
                    && self.active_mode == ImageToolMode::CreativeFx
            })
        {
            self.creative_fx_state.render_retry_at = None;
            self.run_creative_fx_render_on(jobs);
        }
        let Some(rx) = self.creative_fx_state.render_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(CreativeFxMsg::Done {
                stamp,
                rgba,
                duration_ms,
            }))) => {
                let is_current = self.creative_fx_state.render_stamp.as_ref() == Some(&stamp)
                    && stamp == self.creative_fx_render_stamp();
                if is_current {
                    self.creative_fx_state.preview_rgba = Some(rgba);
                    self.creative_fx_state.preview_stamp = Some(stamp);
                    self.creative_fx_state.dirty_preview_texture = true;
                    self.creative_fx_state.last_render_duration_ms = duration_ms;
                    self.creative_fx_state.render_error = None;
                } else if self.active_mode == ImageToolMode::CreativeFx
                    && self.creative_fx_state.params_dirty_at.is_none()
                {
                    self.creative_fx_state.mark_dirty();
                }
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_retry_at = None;
            }
            Ok(Ok(Some(CreativeFxMsg::Error { stamp, message }))) => {
                let is_current = self.creative_fx_state.render_stamp.as_ref() == Some(&stamp)
                    && stamp == self.creative_fx_render_stamp();
                if is_current {
                    self.creative_fx_state.render_error = Some(message);
                } else if self.active_mode == ImageToolMode::CreativeFx
                    && self.creative_fx_state.params_dirty_at.is_none()
                {
                    self.creative_fx_state.mark_dirty();
                }
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_retry_at = None;
            }
            Ok(Ok(None)) | Ok(Err(JobError::Cancelled)) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
            }
            Ok(Err(JobError::QueueFull)) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_rx = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.creative_fx_state.render_error =
                    Some("Waiting for background workers; retrying shortly".to_string());
            }
            Ok(Err(error)) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_error =
                    Some(format!("Creative FX worker failed: {error}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.render_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_error =
                    Some("Creative FX worker disconnected".into());
            }
        }
    }

    fn poll_creative_fx_depth_mask(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.depth_estimation_in_progress
            && self
                .creative_fx_state
                .depth_estimation_stamp
                .as_ref()
                .map(|stamp| &stamp.input)
                != Some(&self.input_stamp())
        {
            if let Some(ticket) = self.creative_fx_state.depth_job_ticket.take() {
                ticket.cancel();
            }
            self.creative_fx_state.depth_job_rx = None;
            if let Some(control) = self.creative_fx_state.depth_process_control.take() {
                control.cancel();
            }
            self.creative_fx_state.depth_estimation_rx = None;
            self.creative_fx_state.depth_estimation_stamp = None;
            self.creative_fx_state.depth_request = None;
            self.creative_fx_state.depth_retry_at = None;
            self.creative_fx_state.depth_queue_status = None;
            self.creative_fx_state.depth_estimation_in_progress = false;
            return;
        }

        if self
            .creative_fx_state
            .depth_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.submit_creative_fx_depth_attempt(jobs);
        } else if self.creative_fx_state.depth_retry_at.is_some() {
            return;
        }

        if let Some(job_rx) = self.creative_fx_state.depth_job_rx.as_ref() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.creative_fx_state.depth_job_rx = None;
                    self.creative_fx_state.depth_job_ticket = None;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.creative_fx_state.depth_job_rx = None;
                    self.creative_fx_state.depth_job_ticket = None;
                    self.creative_fx_state.depth_estimation_rx = None;
                    self.creative_fx_state.depth_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                    self.creative_fx_state.depth_queue_status =
                        Some("Depth map waiting for workers; retrying shortly".into());
                    return;
                }
                Ok(Err(JobError::Cancelled)) => {
                    self.finish_creative_fx_depth_mask();
                    return;
                }
                Ok(Err(error)) => {
                    let should_report = self
                        .creative_fx_state
                        .depth_estimation_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    self.finish_creative_fx_depth_mask();
                    if should_report {
                        self.creative_fx_state.depth_estimation_error =
                            Some(format!("Depth map worker failed: {error}"));
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let should_report = self
                        .creative_fx_state
                        .depth_estimation_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    if let Some(control) = self.creative_fx_state.depth_process_control.take() {
                        control.cancel();
                    }
                    self.finish_creative_fx_depth_mask();
                    if should_report {
                        self.creative_fx_state.depth_estimation_error =
                            Some("Depth map worker stopped".into());
                    }
                    return;
                }
            }
        }
        let Some(rx) = self.creative_fx_state.depth_estimation_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(message) => {
                let stamp = message.stamp;
                let source_is_current = self.creative_fx_mask_source_matches(
                    self.creative_fx_state.depth_estimation_stamp.as_ref(),
                    &stamp,
                );
                let can_configure_selected =
                    source_is_current && self.creative_fx_mask_intent_matches(&stamp);
                match message.result {
                    Ok(depth_rgba) if source_is_current => {
                        self.accept_creative_fx_depth_mask(
                            stamp,
                            depth_rgba,
                            can_configure_selected,
                        );
                    }
                    Err(error) if can_configure_selected => {
                        self.creative_fx_state.depth_estimation_error = Some(error);
                    }
                    _ => {}
                }
                self.creative_fx_state.depth_estimation_in_progress = false;
                self.creative_fx_state.depth_estimation_stamp = None;
                self.creative_fx_state.depth_request = None;
                self.creative_fx_state.depth_retry_at = None;
                self.creative_fx_state.depth_queue_status = None;
                self.creative_fx_state.depth_process_control = None;
                self.creative_fx_state.depth_job_rx = None;
                self.creative_fx_state.depth_job_ticket = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.depth_estimation_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let should_report = self
                    .creative_fx_state
                    .depth_estimation_stamp
                    .as_ref()
                    .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                self.creative_fx_state.depth_estimation_in_progress = false;
                self.creative_fx_state.depth_estimation_stamp = None;
                self.creative_fx_state.depth_request = None;
                self.creative_fx_state.depth_retry_at = None;
                self.creative_fx_state.depth_queue_status = None;
                self.creative_fx_state.depth_process_control = None;
                self.creative_fx_state.depth_job_rx = None;
                self.creative_fx_state.depth_job_ticket = None;
                if should_report {
                    self.creative_fx_state.depth_estimation_error =
                        Some("Depth map worker stopped".into());
                }
            }
        }
    }

    fn finish_creative_fx_depth_mask(&mut self) {
        if let Some(control) = self.creative_fx_state.depth_process_control.take() {
            control.cancel();
        }
        self.creative_fx_state.depth_estimation_in_progress = false;
        self.creative_fx_state.depth_estimation_rx = None;
        self.creative_fx_state.depth_estimation_stamp = None;
        self.creative_fx_state.depth_job_rx = None;
        self.creative_fx_state.depth_job_ticket = None;
        self.creative_fx_state.depth_request = None;
        self.creative_fx_state.depth_retry_at = None;
        self.creative_fx_state.depth_queue_status = None;
    }

    fn poll_creative_fx_alpha_mask(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.alpha_mask_in_progress
            && self
                .creative_fx_state
                .alpha_mask_stamp
                .as_ref()
                .map(|stamp| &stamp.input)
                != Some(&self.input_stamp())
        {
            if let Some(ticket) = self.creative_fx_state.alpha_job_ticket.take() {
                ticket.cancel();
            }
            self.creative_fx_state.alpha_job_rx = None;
            if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
                control.cancel();
            }
            self.creative_fx_state.alpha_mask_rx = None;
            self.creative_fx_state.alpha_mask_stamp = None;
            self.creative_fx_state.alpha_request = None;
            self.creative_fx_state.alpha_retry_at = None;
            self.creative_fx_state.alpha_queue_status = None;
            self.creative_fx_state.alpha_mask_in_progress = false;
            return;
        }

        if self
            .creative_fx_state
            .alpha_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.submit_creative_fx_alpha_attempt(jobs);
        } else if self.creative_fx_state.alpha_retry_at.is_some() {
            return;
        }

        if let Some(job_rx) = self.creative_fx_state.alpha_job_rx.as_ref() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.creative_fx_state.alpha_job_rx = None;
                    self.creative_fx_state.alpha_job_ticket = None;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.creative_fx_state.alpha_job_rx = None;
                    self.creative_fx_state.alpha_job_ticket = None;
                    self.creative_fx_state.alpha_mask_rx = None;
                    self.creative_fx_state.alpha_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                    self.creative_fx_state.alpha_queue_status =
                        Some("Alpha mask waiting for workers; retrying shortly".into());
                    return;
                }
                Ok(Err(JobError::Cancelled)) => {
                    self.finish_creative_fx_alpha_mask();
                    return;
                }
                Ok(Err(error)) => {
                    let should_report = self
                        .creative_fx_state
                        .alpha_mask_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    self.finish_creative_fx_alpha_mask();
                    if should_report {
                        self.creative_fx_state.alpha_mask_error =
                            Some(format!("Alpha mask worker failed: {error}"));
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let should_report = self
                        .creative_fx_state
                        .alpha_mask_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
                        control.cancel();
                    }
                    self.finish_creative_fx_alpha_mask();
                    if should_report {
                        self.creative_fx_state.alpha_mask_error =
                            Some("Alpha mask worker stopped".into());
                    }
                    return;
                }
            }
        }
        let Some(rx) = self.creative_fx_state.alpha_mask_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(message) => {
                let stamp = message.stamp;
                let source_is_current = self.creative_fx_mask_source_matches(
                    self.creative_fx_state.alpha_mask_stamp.as_ref(),
                    &stamp,
                );
                let can_configure_selected =
                    source_is_current && self.creative_fx_mask_intent_matches(&stamp);
                match message.result {
                    Ok(removal_result) if source_is_current => {
                        self.accept_creative_fx_alpha_mask(
                            stamp,
                            removal_result,
                            can_configure_selected,
                        );
                    }
                    Err(error) if can_configure_selected => {
                        self.creative_fx_state.alpha_mask_error = Some(error);
                    }
                    _ => {}
                }
                self.creative_fx_state.alpha_mask_in_progress = false;
                self.creative_fx_state.alpha_mask_stamp = None;
                self.creative_fx_state.alpha_process_control = None;
                self.creative_fx_state.alpha_request = None;
                self.creative_fx_state.alpha_retry_at = None;
                self.creative_fx_state.alpha_queue_status = None;
                self.creative_fx_state.alpha_job_rx = None;
                self.creative_fx_state.alpha_job_ticket = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.alpha_mask_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let should_report = self
                    .creative_fx_state
                    .alpha_mask_stamp
                    .as_ref()
                    .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                self.creative_fx_state.alpha_mask_in_progress = false;
                self.creative_fx_state.alpha_mask_stamp = None;
                self.creative_fx_state.alpha_process_control = None;
                self.creative_fx_state.alpha_request = None;
                self.creative_fx_state.alpha_retry_at = None;
                self.creative_fx_state.alpha_queue_status = None;
                self.creative_fx_state.alpha_job_rx = None;
                self.creative_fx_state.alpha_job_ticket = None;
                if should_report {
                    self.creative_fx_state.alpha_mask_error =
                        Some("Alpha mask worker stopped".into());
                }
            }
        }
    }

    fn finish_creative_fx_alpha_mask(&mut self) {
        if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
            control.cancel();
        }
        self.creative_fx_state.alpha_mask_in_progress = false;
        self.creative_fx_state.alpha_mask_rx = None;
        self.creative_fx_state.alpha_mask_stamp = None;
        self.creative_fx_state.alpha_job_rx = None;
        self.creative_fx_state.alpha_job_ticket = None;
        self.creative_fx_state.alpha_request = None;
        self.creative_fx_state.alpha_retry_at = None;
        self.creative_fx_state.alpha_queue_status = None;
    }

    fn poll_creative_fx_thumbnails(&mut self) {
        let Some(rx) = self.creative_fx_state.thumbnail_rx.take() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(msg)
                    if msg.epoch == self.creative_fx_state.thumbnail_epoch
                        && Some(msg.pixel_revision)
                            == self.creative_fx_state.thumbnail_pixel_revision
                        && msg.pixel_revision == self.pixel_revision =>
                {
                    self.creative_fx_state
                        .pending_thumbnails
                        .push((msg.fx_type, msg.rgba));
                }
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.creative_fx_state.thumbnail_rx = Some(rx);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.creative_fx_state.is_generating_thumbnails = false;
                    self.creative_fx_state.thumbnail_cancel = None;
                    break;
                }
            }
        }
    }

    fn accept_creative_fx_depth_mask(
        &mut self,
        stamp: CreativeFxMaskStamp,
        depth_rgba: RgbaImage,
        can_configure_selected: bool,
    ) {
        self.creative_fx_state.cached_depth_map = Some(depth_rgba);
        self.creative_fx_state.depth_map_input_stamp = Some(stamp.input);
        self.creative_fx_state.depth_estimation_error = None;
        let active_uses_cache = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == CreativeFxMaskSource::DepthMap
        };
        if can_configure_selected {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = CreativeFxMaskSource::DepthMap;
            config.enabled = true;
        }
        if can_configure_selected || active_uses_cache {
            self.creative_fx_state.mask_cache_revision =
                self.creative_fx_state.mask_cache_revision.wrapping_add(1);
            self.creative_fx_state.mark_dirty();
            self.run_creative_fx_render();
        }
    }

    fn accept_creative_fx_alpha_mask(
        &mut self,
        stamp: CreativeFxMaskStamp,
        removal_result: RgbaImage,
        can_configure_selected: bool,
    ) {
        self.creative_fx_state.cached_background_removal = Some(removal_result);
        self.creative_fx_state.background_removal_input_stamp = Some(stamp.input);
        self.creative_fx_state.alpha_mask_error = None;
        let active_uses_cache = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == CreativeFxMaskSource::BackgroundRemovalAlpha
        };
        if can_configure_selected {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = CreativeFxMaskSource::BackgroundRemovalAlpha;
            config.enabled = true;
        }
        if can_configure_selected || active_uses_cache {
            self.creative_fx_state.mask_cache_revision =
                self.creative_fx_state.mask_cache_revision.wrapping_add(1);
            self.creative_fx_state.mark_dirty();
            self.run_creative_fx_render();
        }
    }

    /// Commit creative FX result into current_rgba and push to undo stack.

    pub fn apply_creative_fx(&mut self) {
        self.invalidate_stale_creative_fx_masks();
        if self.creative_fx_state.preview_stamp.as_ref() != Some(&self.creative_fx_render_stamp()) {
            return;
        }
        let preview = match self.creative_fx_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        let fx_name = self.creative_fx_state.selected_fx.name();
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(preview);
        self.mark_pixels_changed();
        self.invalidate_stale_creative_fx_masks();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.creative_fx_state.preview_rgba = None;
        self.creative_fx_state.preview_stamp = None;
        self.creative_fx_state.preview_texture = None;
        self.creative_fx_state.dirty_preview_texture = false;

        self.creative_fx_state.fx_thumbnails.clear();
        self.creative_fx_state.pending_thumbnails.clear();
        self.creative_fx_state.thumbnail_pixel_revision = None;
        self.run_creative_fx_thumbnail_batch();
        self.show_toast(format!("✨ {} applied", fx_name));
    }

    fn toggle_creative_fx_mask_source(&mut self, source: CreativeFxMaskSource) {
        let active = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == source
        };
        if active {
            self.creative_fx_state
                .depth_mask_for_active_fx_mut()
                .enabled = false;
            self.creative_fx_state.mark_dirty();
            return;
        }

        self.invalidate_stale_creative_fx_masks();
        let cached = match source {
            CreativeFxMaskSource::DepthMap => self.depth_mask_cache_is_current(),
            CreativeFxMaskSource::BackgroundRemovalAlpha => self.alpha_mask_cache_is_current(),
        };
        if cached {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = source;
            config.enabled = true;
            self.creative_fx_state.mark_dirty();
        } else {
            match source {
                CreativeFxMaskSource::DepthMap => self.run_depth_estimation_for_fx(),
                CreativeFxMaskSource::BackgroundRemovalAlpha => self.run_alpha_mask_for_fx(),
            }
        }
    }

    fn render_creative_fx_mask_sources(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        full_w: f32,
    ) {
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let config = *self.creative_fx_state.depth_mask_for_active_fx();
        let depth_active = config.enabled && config.source == CreativeFxMaskSource::DepthMap;
        let alpha_active =
            config.enabled && config.source == CreativeFxMaskSource::BackgroundRemovalAlpha;
        self.invalidate_stale_creative_fx_masks();
        let has_depth = self.depth_mask_cache_is_current();
        let has_alpha = self.alpha_mask_cache_is_current();
        let depth_loading = self.creative_fx_state.depth_estimation_in_progress;
        let alpha_loading = self.creative_fx_state.alpha_mask_in_progress;
        let mask_loading = depth_loading || alpha_loading;

        ui.label(
            egui::RichText::new("Masking")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_high),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        if depth_loading {
            render_creative_fx_mask_loading(
                ui,
                self.creative_fx_state
                    .depth_queue_status
                    .as_deref()
                    .unwrap_or("Estimating depth map with Depth-Anything..."),
                base_c,
            );
            if let Some(retry_at) = self.creative_fx_state.depth_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        } else if alpha_loading {
            render_creative_fx_mask_loading(
                ui,
                self.creative_fx_state
                    .alpha_queue_status
                    .as_deref()
                    .unwrap_or("Removing background with FeyNoBg..."),
                base_c,
            );
            if let Some(retry_at) = self.creative_fx_state.alpha_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        }

        let depth_label = if depth_active {
            "🌊 Depth Mask: ON"
        } else if has_depth {
            "🌊 Depth map • cached"
        } else {
            "🌊 Use depth map as mask"
        };
        if render_creative_fx_mask_button(
            ui,
            depth_label,
            depth_active,
            mask_loading,
            full_w,
            palette,
        )
        .on_hover_text("Use AI monocular depth to mask this effect")
        .clicked()
        {
            self.toggle_creative_fx_mask_source(CreativeFxMaskSource::DepthMap);
        }

        ui.add_space(3.0);
        let alpha_label = if alpha_active {
            "✂ Removal Alpha: ON"
        } else if has_alpha {
            "✂ Removal alpha • cached"
        } else {
            "✂ Use removal alpha as mask"
        };
        if render_creative_fx_mask_button(
            ui,
            alpha_label,
            alpha_active,
            mask_loading,
            full_w,
            palette,
        )
        .on_hover_text("Use the previous Remove BG result, or run automatic FeyNoBg removal")
        .clicked()
        {
            self.toggle_creative_fx_mask_source(CreativeFxMaskSource::BackgroundRemovalAlpha);
        }

        if let Some(error) = &self.creative_fx_state.depth_estimation_error {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("⚠ {}", error))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(diff_del),
            );
        }
        if let Some(error) = &self.creative_fx_state.alpha_mask_error {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("⚠ {}", error))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(diff_del),
            );
        }
    }

    fn render_creative_fx_mask_options(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        cur_fx: CreativeFxType,
        full_w: f32,
        slider_w: f32,
    ) {
        let config = *self.creative_fx_state.depth_mask_for_active_fx();
        if !config.enabled {
            return;
        }

        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        ui.add_space(crate::ui_ext::SPACE_XS);
        let invert_text = match (config.source, config.invert) {
            (CreativeFxMaskSource::DepthMap, true) => "⇄ Invert Mask: ON (Far / Background)",
            (CreativeFxMaskSource::DepthMap, false) => "⇄ Invert Mask: OFF (Near / Foreground)",
            (CreativeFxMaskSource::BackgroundRemovalAlpha, true) => {
                "⇄ Invert Mask: ON (Background)"
            }
            (CreativeFxMaskSource::BackgroundRemovalAlpha, false) => {
                "⇄ Invert Mask: OFF (Foreground)"
            }
        };
        let invert_button = egui::Button::new(
            egui::RichText::new(invert_text)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_high),
        )
        .fill(b_med)
        .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
        .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));
        if ui
            .add(invert_button)
            .on_hover_text("Swap the foreground and background mask regions")
            .clicked()
        {
            self.creative_fx_state.depth_mask_for_active_fx_mut().invert = !config.invert;
            self.creative_fx_state.mark_dirty();
        }

        let supports_depth_cutoff = matches!(
            cur_fx,
            CreativeFxType::PixelSort
                | CreativeFxType::CmykHalftone
                | CreativeFxType::DitherPalette
                | CreativeFxType::Ntsc
                | CreativeFxType::Structure
        );
        if config.source != CreativeFxMaskSource::DepthMap || !supports_depth_cutoff {
            return;
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let mut threshold = config.threshold;
        if render_context_slider(
            ui,
            "Depth Cutoff (0: Continuous gradient)",
            &mut threshold,
            0.0..=1.0,
            0.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state
                .depth_mask_for_active_fx_mut()
                .threshold = threshold;
            self.creative_fx_state.mark_dirty();
        }

        if threshold > 0.001 {
            let mut feather = config.feather;
            if render_context_slider(
                ui,
                "Feather Softness",
                &mut feather,
                0.01..=0.50,
                0.25,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state
                    .depth_mask_for_active_fx_mut()
                    .feather = feather;
                self.creative_fx_state.mark_dirty();
            }
        }
    }

    /// Full 3-panel Creative FX viewport (Left: FX list; Center: Split Canvas; Right: Controls).

    pub(crate) fn render_creative_fx_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.invalidate_stale_creative_fx_masks();
        self.invalidate_stale_creative_fx_thumbnails();
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_add = hex_to_c32(&palette.diff_add);

        // Upload any newly received thumbnails into GPU textures
        for (fx_type, rgba) in self.creative_fx_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("creative_fx_thumb_{:?}", fx_type),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.creative_fx_state.fx_thumbnails.insert(fx_type, tex);
        }

        // Fill thumbnails that were still outstanding when the mode was left.
        if self.creative_fx_state.fx_thumbnails.len()
            + self.creative_fx_state.pending_thumbnails.len()
            < CreativeFxType::all().len()
            && !self.creative_fx_state.is_generating_thumbnails
            && self.creative_fx_state.thumbnail_retry_at.is_none()
            && !self.current_rgba.is_empty()
        {
            self.run_creative_fx_thumbnail_batch();
        }

        let panel_layout = ImageToolMode::CreativeFx
            .panel_layout()
            .expect("Creative FX has a panel layout")
            .with_panel_sizes(&self.panel_state);
        let panels = panel_layout.allocate(ui);
        let left = panels.left.expect("Creative FX has a browser rail");
        let right = panels.right.expect("Creative FX has an inspector rail");
        let left_w = left.width();
        let left_h = left.height();
        let right_w = right.width();
        let right_h = right.height();
        let center_w = panels.center.width();
        let main_h = panels.center.height();

        // Fit to view when first opening Creative FX
        if !self.creative_fx_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.creative_fx_state.auto_fitted = true;
        }

        // ── 1. LEFT PANEL: Creative FX List with 42x28 mini preview photos ──
        crate::shell::StudioPanelLayout::show_region(ui, left, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(left_w - 16.0);
                    ui.set_height((left_h - 16.0).max(1.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("✨ Creative FX")
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(base_c),
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", CreativeFxType::all().len()))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        if self.creative_fx_state.is_generating_thumbnails {
                            ui.label(
                                egui::RichText::new("⏳")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            )
                            .on_hover_text("Generating preview thumbnails…");
                        }
                    });
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    if let Some(status) = &self.creative_fx_state.thumbnail_queue_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                    }
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_XS);

                    egui::ScrollArea::vertical()
                        .id_source("creative_fx_list_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(left_w - 24.0);
                            for fx in CreativeFxType::all() {
                                let selected = self.creative_fx_state.selected_fx == *fx;
                                let (row_bg, name_col, meta_col) = if selected {
                                    (
                                        base_c,
                                        contrast_text_c32(base_c, None, palette),
                                        contrast_text_c32(base_c, None, palette),
                                    )
                                } else {
                                    (b_low, f_high, f_med)
                                };

                                let row_frame = egui::Frame::none()
                                    .fill(row_bg)
                                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                    .inner_margin(egui::Margin::symmetric(6.0, 5.0));

                                let resp = row_frame
                                    .show(ui, |ui| {
                                        ui.set_width(left_w - 32.0);
                                        ui.horizontal(|ui| {
                                            // 42x28 mini thumbnail preview
                                            if let Some(tex) =
                                                self.creative_fx_state.fx_thumbnails.get(fx)
                                            {
                                                ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                            } else {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(42.0, 28.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    Rounding::same(2.0),
                                                    b_med,
                                                );
                                            }
                                            ui.add_space(crate::ui_ext::SPACE_XS);
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(fx.name())
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(name_col),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(fx.category())
                                                            .monospace()
                                                            .color(meta_col),
                                                    );
                                                });
                                                ui.label(
                                                    egui::RichText::new(fx.subtitle())
                                                        .color(meta_col)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                            });
                                        });
                                    })
                                    .response;

                                if resp.interact(egui::Sense::click()).clicked() && !selected {
                                    self.creative_fx_state.select_fx(*fx);
                                }
                                ui.add_space(crate::ui_ext::SPACE_XS);
                            }
                        });
                });
        });

        // ── 2. CENTER PANEL: Split-view Comparison Canvas ──
        crate::shell::StudioPanelLayout::show_region(ui, panels.center, |ui| {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.canvas))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.set_width(center_w);
                    ui.set_height(main_h);

                    // Reload preview texture if dirty
                    let preview_is_current = self.creative_fx_state.preview_stamp.as_ref()
                        == Some(&self.creative_fx_render_stamp());
                    if !preview_is_current {
                        self.creative_fx_state.preview_texture = None;
                        self.creative_fx_state.dirty_preview_texture = false;
                    } else if self.creative_fx_state.dirty_preview_texture {
                        if let Some(ref rgba) = self.creative_fx_state.preview_rgba {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            );
                            self.creative_fx_state.preview_texture = Some(ctx.load_texture(
                                "creative_fx_preview_tex",
                                ci,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        self.creative_fx_state.dirty_preview_texture = false;
                    }

                    let (response, painter) = ui.allocate_painter(
                        Vec2::new(center_w, main_h),
                        egui::Sense::click_and_drag(),
                    );
                    let canvas_rect = response.rect;

                    painter.rect_filled(
                        canvas_rect,
                        Rounding::same(4.0),
                        hex_to_c32(&palette.colors.canvas),
                    );

                    // If mouse is near the comparison split divider when drag starts, initiate split dragging
                    let near_split = if let Some(pos) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                    {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));
                        let split_x = img_rect.min.x
                            + self.creative_fx_state.comparison_split.clamp(0.01, 0.99)
                                * img_rect.width();
                        (pos.x - split_x).abs() <= 18.0
                            && pos.y >= img_rect.min.y
                            && pos.y <= img_rect.max.y
                    } else {
                        false
                    };
                    if near_split && response.drag_started() {
                        self.creative_fx_state.is_dragging_split = true;
                    }

                    // Pan and Zoom
                    let middle_drag =
                        ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                    if (!self.creative_fx_state.is_dragging_split && response.dragged())
                        || middle_drag
                    {
                        self.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                        if scroll_delta.abs() > 0.0 {
                            let zoom_factor = (scroll_delta * 0.002).exp();
                            let old_zoom = self.zoom;
                            let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 5.0);
                            if let Some(ptr_pos) = response.hover_pos() {
                                let center = canvas_rect.center() + self.pan;
                                let to_ptr = ptr_pos - center;
                                self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                            }
                            self.zoom = new_zoom;
                        }
                    }
                    if !self.creative_fx_state.is_dragging_split && response.double_clicked() {
                        let avail_size = Vec2::new(center_w, main_h);
                        self.fit_to_view(avail_size);
                    }

                    if let Some(orig_tex) = self.texture.clone() {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                        draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                        self.render_creative_fx_comparison(
                            ui,
                            &painter,
                            &response,
                            img_rect,
                            canvas_rect,
                            ctx,
                            &orig_tex,
                            palette,
                        );

                        painter.rect_stroke(
                            img_rect,
                            Rounding::ZERO,
                            Stroke::new(
                                1.0_f32,
                                hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                            ),
                        );
                    }

                    // Rendering spinner overlay
                    if self.creative_fx_state.is_rendering {
                        let spinner_pos = canvas_rect.center();
                        let spin_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
                        let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                        painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                        painter.rect_stroke(
                            spin_rect,
                            Rounding::same(8.0),
                            Stroke::new(1.0_f32, b_med),
                        );
                        painter.text(
                            spinner_pos,
                            egui::Align2::CENTER_CENTER,
                            "⏳ Processing Creative FX…",
                            egui::FontId::proportional(12.0),
                            f_med,
                        );
                    }

                    // Error overlay
                    if let Some(ref err) = self.creative_fx_state.render_error.clone() {
                        let err_pos = Pos2::new(canvas_rect.min.x + 10.0, canvas_rect.min.y + 10.0);
                        painter.text(
                            err_pos,
                            egui::Align2::LEFT_TOP,
                            format!("⚠ {}", err),
                            egui::FontId::proportional(11.0),
                            hex_to_c32(&palette.diff_del),
                        );
                    }
                });
        });

        // ── 3. RIGHT PANEL: Controls for Selected Effect ──
        crate::shell::StudioPanelLayout::show_region(ui, right, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(right_w - 16.0);
                    ui.set_height((right_h - 16.0).max(1.0));

                    egui::ScrollArea::vertical()
                        .id_source("creative_fx_right_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let full_w = (right_w - 32.0).max(120.0);
                            let slider_w = (full_w - 68.0).max(80.0);
                            let cur_fx = self.creative_fx_state.selected_fx;

                            // Header
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(cur_fx.icon()).size(16.0));
                                ui.label(egui::RichText::new(cur_fx.name()).strong().color(base_c));
                            });
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(cur_fx.description())
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            );
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // ── Presets Selector ──
                            ui.label(
                                egui::RichText::new("🏷 Presets")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(3.0);
                            let presets = presets_for_fx(cur_fx);
                            for preset in &presets {
                                let sel =
                                    self.creative_fx_state.selected_preset_name == preset.name;
                                let (s_bg, s_fg) = if sel {
                                    (base_c, contrast_text_c32(base_c, None, palette))
                                } else {
                                    (b_med, f_med)
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(preset.name)
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(s_fg),
                                )
                                .fill(s_bg)
                                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                                if ui.add(btn).on_hover_text(preset.description).clicked() {
                                    self.creative_fx_state.selected_preset_name =
                                        preset.name.to_string();
                                    match &preset.params {
                                        CreativeFxParams::LensAberration(p) => {
                                            self.creative_fx_state.lens_params = *p;
                                        }
                                        CreativeFxParams::Trichromatic(p) => {
                                            self.creative_fx_state.trichromatic_params = *p;
                                        }
                                        CreativeFxParams::PixelSort(p) => {
                                            self.creative_fx_state.pixel_sort_params = *p;
                                        }
                                        CreativeFxParams::CmykHalftone(p) => {
                                            self.creative_fx_state.cmyk_params = *p;
                                        }
                                        CreativeFxParams::DitherPalette(p) => {
                                            self.creative_fx_state.dither_params = *p;
                                        }
                                        CreativeFxParams::Ntsc(p) => {
                                            self.creative_fx_state.ntsc_params = *p;
                                        }
                                        CreativeFxParams::Structure(p) => {
                                            self.creative_fx_state.structure_params = *p;
                                        }
                                    }
                                    self.creative_fx_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }

                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // ── Parameter Controls ──
                            ui.label(
                                egui::RichText::new("🎛 Controls")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            match cur_fx {
                                CreativeFxType::LensAberration => {
                                    // Strength: 0..100
                                    if render_context_slider(
                                        ui,
                                        "⚡ Strength",
                                        &mut self.creative_fx_state.lens_params.strength,
                                        0.0..=100.0,
                                        25.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Falloff: 0..100 (0: entire image, 100: edges only)
                                    if render_context_slider(
                                        ui,
                                        "🔘 Falloff (0: All, 100: Edges)",
                                        &mut self.creative_fx_state.lens_params.falloff,
                                        0.0..=100.0,
                                        50.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Color Balance: -50..+50 (-50: red/blue, +50: blue/yellow)
                                    if render_context_slider(
                                        ui,
                                        "🎨 Color Balance (-50: Red/Blue, +50: Blue/Yellow)",
                                        &mut self.creative_fx_state.lens_params.color_balance,
                                        -50.0..=50.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::Trichromatic => {
                                    // Mode: RGB vs OKLab
                                    ui.label(
                                        egui::RichText::new("Color Separation Model")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let modes = [
                                            (TrichromaticMode::Rgb, "RGB"),
                                            (TrichromaticMode::OkLab, "OKLab"),
                                        ];
                                        for (mode, label) in modes {
                                            let sel =
                                                self.creative_fx_state.trichromatic_params.mode
                                                    == mode;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.trichromatic_params.mode =
                                                    mode;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    let names = self
                                        .creative_fx_state
                                        .trichromatic_params
                                        .mode
                                        .channel_names();
                                    for i in 0..3 {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Channel {}: {}",
                                                i + 1,
                                                names[i]
                                            ))
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                        );
                                        ui.add_space(2.0);

                                        // Offset X
                                        if render_context_slider(
                                            ui,
                                            &format!("↔ X Offset (Ch {})", i + 1),
                                            &mut self
                                                .creative_fx_state
                                                .trichromatic_params
                                                .channels[i]
                                                .offset_x,
                                            -50.0..=50.0,
                                            0.0,
                                            "px",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        // Offset Y
                                        if render_context_slider(
                                            ui,
                                            &format!("↕ Y Offset (Ch {})", i + 1),
                                            &mut self
                                                .creative_fx_state
                                                .trichromatic_params
                                                .channels[i]
                                                .offset_y,
                                            -50.0..=50.0,
                                            0.0,
                                            "px",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        // Zoom
                                        if render_context_slider(
                                            ui,
                                            &format!("🔍 Zoom (Ch {})", i + 1),
                                            &mut self
                                                .creative_fx_state
                                                .trichromatic_params
                                                .channels[i]
                                                .zoom,
                                            0.80..=1.20,
                                            1.00,
                                            "×",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(crate::ui_ext::SPACE_XS);
                                    }

                                    // Overlap crop window calculation
                                    let (_, _, cw, ch) = trichromatic_intersection_box(
                                        self.current_rgba.width(),
                                        self.current_rgba.height(),
                                        &self.creative_fx_state.trichromatic_params.channels,
                                    );
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "✂ Crop Window: {} × {} px",
                                            cw, ch
                                        ))
                                        .monospace()
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new("(auto-cropped to 3-channel overlap)")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_low),
                                    );
                                }

                                CreativeFxType::PixelSort => {
                                    // Direction
                                    ui.label(
                                        egui::RichText::new("Scanline Direction")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let dirs = [
                                            (PixelSortDirection::Vertical, "Vertical"),
                                            (PixelSortDirection::Horizontal, "Horizontal"),
                                            (PixelSortDirection::Diagonal, "Diagonal"),
                                        ];
                                        for (d, label) in dirs {
                                            let sel =
                                                self.creative_fx_state.pixel_sort_params.direction
                                                    == d;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state
                                                    .pixel_sort_params
                                                    .direction = d;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Criteria
                                    ui.label(
                                        egui::RichText::new("Sort Metric")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let crit = [
                                            (PixelSortCriteria::Brightness, "Brightness"),
                                            (PixelSortCriteria::Hue, "Hue"),
                                            (PixelSortCriteria::Saturation, "Saturation"),
                                        ];
                                        for (c, label) in crit {
                                            let sel =
                                                self.creative_fx_state.pixel_sort_params.criteria
                                                    == c;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.pixel_sort_params.criteria =
                                                    c;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Order
                                    ui.label(
                                        egui::RichText::new("Sort Order")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        let orders = [
                                            (PixelSortOrder::Ascending, "Ascending"),
                                            (PixelSortOrder::Descending, "Descending"),
                                        ];
                                        for (o, label) in orders {
                                            let sel =
                                                self.creative_fx_state.pixel_sort_params.order == o;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label)
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.pixel_sort_params.order = o;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Min Threshold
                                    if render_context_slider(
                                        ui,
                                        "Min Threshold",
                                        &mut self.creative_fx_state.pixel_sort_params.min_threshold,
                                        0.0..=1.0,
                                        0.25,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    // Max Threshold
                                    if render_context_slider(
                                        ui,
                                        "Max Threshold",
                                        &mut self.creative_fx_state.pixel_sort_params.max_threshold,
                                        0.0..=1.0,
                                        0.80,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    // Min Span
                                    let mut span_f =
                                        self.creative_fx_state.pixel_sort_params.min_span as f32;
                                    if render_context_slider(
                                        ui,
                                        "Min Span Length",
                                        &mut span_f,
                                        2.0..=50.0,
                                        6.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.pixel_sort_params.min_span =
                                            span_f.round() as usize;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    // Invert mask
                                    let mut invert =
                                        self.creative_fx_state.pixel_sort_params.invert_mask;
                                    if ui
                                        .checkbox(
                                            &mut invert,
                                            egui::RichText::new("Invert Selection Mask")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.pixel_sort_params.invert_mask =
                                            invert;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::CmykHalftone => {
                                    // Dot Size: 2..=16 px
                                    if render_context_slider(
                                        ui,
                                        "🔘 Dot Screen Size",
                                        &mut self.creative_fx_state.cmyk_params.dot_size,
                                        2.0..=16.0,
                                        5.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Dot Gain: 0.5..=2.0
                                    if render_context_slider(
                                        ui,
                                        "📈 Dot Gain (Ink Bleed)",
                                        &mut self.creative_fx_state.cmyk_params.dot_gain,
                                        0.50..=2.00,
                                        1.10,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Paper Tint
                                    ui.label(
                                        egui::RichText::new("Paper Substrate Tint")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for tint in PaperTint::all() {
                                            let sel = self.creative_fx_state.cmyk_params.paper_tint
                                                == *tint;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(tint.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.cmyk_params.paper_tint =
                                                    *tint;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                    ui.separator();
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Plate Misregistration Offsets
                                    ui.label(
                                        egui::RichText::new("🗞 Plate Misregistration Offsets")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(3.0);

                                    // Cyan plate
                                    ui.label(
                                        egui::RichText::new("Cyan Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Cyan X",
                                        &mut self.creative_fx_state.cmyk_params.cyan_offset[0],
                                        -10.0..=10.0,
                                        -1.5,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Cyan Y",
                                        &mut self.creative_fx_state.cmyk_params.cyan_offset[1],
                                        -10.0..=10.0,
                                        0.5,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Magenta plate
                                    ui.label(
                                        egui::RichText::new("Magenta Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Magenta X",
                                        &mut self.creative_fx_state.cmyk_params.magenta_offset[0],
                                        -10.0..=10.0,
                                        1.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Magenta Y",
                                        &mut self.creative_fx_state.cmyk_params.magenta_offset[1],
                                        -10.0..=10.0,
                                        -1.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Yellow plate
                                    ui.label(
                                        egui::RichText::new("Yellow Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Yellow X",
                                        &mut self.creative_fx_state.cmyk_params.yellow_offset[0],
                                        -10.0..=10.0,
                                        0.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Yellow Y",
                                        &mut self.creative_fx_state.cmyk_params.yellow_offset[1],
                                        -10.0..=10.0,
                                        1.5,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Black (Key) plate
                                    ui.label(
                                        egui::RichText::new("Black (Key) Plate Offset")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    if render_context_slider(
                                        ui,
                                        "↔ Black X",
                                        &mut self.creative_fx_state.cmyk_params.black_offset[0],
                                        -10.0..=10.0,
                                        0.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    if render_context_slider(
                                        ui,
                                        "↕ Black Y",
                                        &mut self.creative_fx_state.cmyk_params.black_offset[1],
                                        -10.0..=10.0,
                                        0.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::DitherPalette => {
                                    // Palette selection
                                    ui.label(
                                        egui::RichText::new("Hardware / Aesthetic Palette")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for pal in RetroPalette::all() {
                                            let sel = self.creative_fx_state.dither_params.palette
                                                == *pal;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(pal.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.dither_params.palette = *pal;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    // Dither Method selection
                                    ui.label(
                                        egui::RichText::new("Dithering Method")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for method in DitherMethod::all() {
                                            let sel = self.creative_fx_state.dither_params.method
                                                == *method;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(method.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.dither_params.method =
                                                    *method;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    // Pixel Scale selection (1x..4x)
                                    ui.label(
                                        egui::RichText::new("Pixel Grid Scale")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for scale in [1u32, 2, 3, 4] {
                                            let sel =
                                                self.creative_fx_state.dither_params.pixel_scale
                                                    == scale;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(format!("{}×", scale))
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ))
                                            .min_size(Vec2::new(32.0, 20.0));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.dither_params.pixel_scale =
                                                    scale;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Dither Strength
                                    if render_context_slider(
                                        ui,
                                        "⚡ Dither Strength",
                                        &mut self.creative_fx_state.dither_params.dither_strength,
                                        0.0..=1.0,
                                        0.80,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Pre-dither Contrast
                                    if render_context_slider(
                                        ui,
                                        "🌓 Contrast Boost",
                                        &mut self.creative_fx_state.dither_params.contrast,
                                        -50.0..=50.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }

                                CreativeFxType::Ntsc => {
                                    // ── 1. Broadcast RF & Filtering ──
                                    ui.label(
                                        egui::RichText::new("📡 Signal & Filtering")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Luma Smear (Bandwidth)",
                                        &mut self.creative_fx_state.ntsc_params.luma_smear,
                                        0.0..=1.0,
                                        0.25,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Composite Sharpness",
                                        &mut self
                                            .creative_fx_state
                                            .ntsc_params
                                            .composite_sharpening,
                                        0.0..=3.0,
                                        1.0,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Edge Ringing / Ghosting",
                                        &mut self.creative_fx_state.ntsc_params.ringing,
                                        0.0..=2.0,
                                        0.5,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Demodulation Filter
                                    ui.label(
                                        egui::RichText::new("Demodulation Filter")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for demo in NtscDemodulation::all() {
                                            let sel =
                                                self.creative_fx_state.ntsc_params.demodulation
                                                    == *demo;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(demo.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.ntsc_params.demodulation =
                                                    *demo;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(3.0);

                                    // Scanline Phase Shift
                                    ui.label(
                                        egui::RichText::new("Subcarrier Phase Shift (Dot Crawl)")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for ps in NtscPhaseShift::all() {
                                            let sel =
                                                self.creative_fx_state.ntsc_params.phase_shift
                                                    == *ps;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(ps.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ))
                                            .min_size(Vec2::new(38.0, 20.0));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.ntsc_params.phase_shift =
                                                    *ps;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(3.0);

                                    // Chroma Lowpass
                                    ui.label(
                                        egui::RichText::new("Chroma Lowpass Filter")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for cl in NtscChromaLowpass::all() {
                                            let sel =
                                                self.creative_fx_state.ntsc_params.chroma_lowpass
                                                    == *cl;
                                            let (b_bg, b_fg) = if sel {
                                                (base_c, contrast_text_c32(base_c, None, palette))
                                            } else {
                                                (b_med, f_med)
                                            };
                                            let btn = egui::Button::new(
                                                egui::RichText::new(cl.label())
                                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                                    .strong()
                                                    .color(b_fg),
                                            )
                                            .fill(b_bg)
                                            .stroke(Stroke::new(
                                                1.0_f32,
                                                if sel { base_c } else { b_med },
                                            ))
                                            .rounding(Rounding::same(
                                                crate::ui_ext::CONTROL_CORNER_RADIUS,
                                            ));
                                            if ui.add(btn).clicked() && !sel {
                                                self.creative_fx_state.ntsc_params.chroma_lowpass =
                                                    *cl;
                                                self.creative_fx_state.selected_preset_name =
                                                    "Custom".to_string();
                                                self.creative_fx_state.mark_dirty();
                                            }
                                        }
                                    });
                                    ui.add_space(3.0);

                                    let mut scanlines =
                                        self.creative_fx_state.ntsc_params.scanlines;
                                    if ui
                                        .checkbox(
                                            &mut scanlines,
                                            egui::RichText::new("📺 CRT Interlaced Scanlines")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.ntsc_params.scanlines = scanlines;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // ── 2. Chrominance & YIQ ──
                                    ui.separator();
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new("🎨 Chrominance & Color")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "↔ Chroma Horizontal Bleed",
                                        &mut self.creative_fx_state.ntsc_params.chroma_delay_x,
                                        -8.0..=8.0,
                                        1.0,
                                        "px",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    let mut delay_y =
                                        self.creative_fx_state.ntsc_params.chroma_delay_y as f32;
                                    if render_context_slider(
                                        ui,
                                        "↕ Chroma Vertical Shift",
                                        &mut delay_y,
                                        -4.0..=4.0,
                                        0.0,
                                        "lines",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.ntsc_params.chroma_delay_y =
                                            delay_y.round() as i32;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    let mut vert_blend =
                                        self.creative_fx_state.ntsc_params.chroma_vert_blend;
                                    if ui
                                        .checkbox(
                                            &mut vert_blend,
                                            egui::RichText::new("Chroma Vertical Blending")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.ntsc_params.chroma_vert_blend =
                                            vert_blend;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "🎨 Global Tint / Hue Shift (0: Normal)",
                                        &mut self.creative_fx_state.ntsc_params.chroma_phase_error,
                                        0.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "〰 Chroma Phase Jitter",
                                        &mut self.creative_fx_state.ntsc_params.chroma_phase_noise,
                                        0.0..=0.10,
                                        0.005,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "🌈 Color Grain / Chroma Noise",
                                        &mut self.creative_fx_state.ntsc_params.chroma_noise,
                                        0.0..=0.50,
                                        0.05,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // ── 3. Noise & Static ──
                                    ui.separator();
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new("❄ Noise & Static")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("RF Static / Snow")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        );
                                        ui.spacing_mut().slider_width = slider_w;
                                        let mut snow = self.creative_fx_state.ntsc_params.snow;
                                        let slider_response = ui.add(
                                            egui::Slider::new(&mut snow, 0.0..=0.04)
                                                .max_decimals(4),
                                        );
                                        let default_snow = NtscParams::default().snow;
                                        let slider_response =
                                            slider_response.on_hover_text(format!(
                                                "Double-click to reset (default: {:.4})",
                                                default_snow
                                            ));
                                        let is_double_click =
                                            slider_double_clicked(ui, &slider_response);
                                        if is_double_click {
                                            snow = default_snow;
                                        }
                                        if slider_response.changed() || is_double_click {
                                            self.creative_fx_state.ntsc_params.snow = snow;
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                    });
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Snow Streaking (Anisotropy)",
                                        &mut self.creative_fx_state.ntsc_params.snow_anisotropy,
                                        0.0..=1.0,
                                        0.5,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Luma Noise Grain",
                                        &mut self.creative_fx_state.ntsc_params.luma_noise,
                                        0.0..=0.50,
                                        0.01,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(2.0);

                                    if render_context_slider(
                                        ui,
                                        "Composite Signal Noise",
                                        &mut self.creative_fx_state.ntsc_params.composite_noise,
                                        0.0..=0.50,
                                        0.05,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // ── 4. VHS Magnetic Tape Simulation ──
                                    ui.separator();
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new("📼 VHS Tape Emulation")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    ui.add_space(2.0);

                                    let mut vhs_enabled =
                                        self.creative_fx_state.ntsc_params.vhs_enabled;
                                    if ui
                                        .checkbox(
                                            &mut vhs_enabled,
                                            egui::RichText::new("Enable VHS Tape Pass")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        )
                                        .changed()
                                    {
                                        self.creative_fx_state.ntsc_params.vhs_enabled =
                                            vhs_enabled;
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }

                                    if self.creative_fx_state.ntsc_params.vhs_enabled {
                                        ui.add_space(2.0);
                                        ui.label(
                                            egui::RichText::new("Tape Speed")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        );
                                        ui.horizontal_wrapped(|ui| {
                                            for speed in [
                                                VhsTapeSpeed::SP,
                                                VhsTapeSpeed::LP,
                                                VhsTapeSpeed::EP,
                                            ] {
                                                let sel =
                                                    self.creative_fx_state.ntsc_params.tape_speed
                                                        == speed;
                                                let (b_bg, b_fg) = if sel {
                                                    (
                                                        base_c,
                                                        contrast_text_c32(base_c, None, palette),
                                                    )
                                                } else {
                                                    (b_med, f_med)
                                                };
                                                let btn = egui::Button::new(
                                                    egui::RichText::new(speed.label())
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(b_fg),
                                                )
                                                .fill(b_bg)
                                                .stroke(Stroke::new(
                                                    1.0_f32,
                                                    if sel { base_c } else { b_med },
                                                ))
                                                .rounding(Rounding::same(
                                                    crate::ui_ext::CONTROL_CORNER_RADIUS,
                                                ));
                                                if ui.add(btn).clicked() && !sel {
                                                    self.creative_fx_state.ntsc_params.tape_speed =
                                                        speed;
                                                    self.creative_fx_state.selected_preset_name =
                                                        "Custom".to_string();
                                                    self.creative_fx_state.mark_dirty();
                                                }
                                            }
                                        });
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "〰 Tape Wobble / Jitter",
                                            &mut self.creative_fx_state.ntsc_params.edge_wave,
                                            0.0..=2.0,
                                            0.5,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "📶 Tracking Noise Bar",
                                            &mut self.creative_fx_state.ntsc_params.tracking_noise,
                                            0.0..=1.0,
                                            0.0,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "Playhead Sharpening",
                                            &mut self.creative_fx_state.ntsc_params.vhs_sharpen,
                                            0.0..=1.0,
                                            0.25,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        if render_context_slider(
                                            ui,
                                            "Chroma Loss / Dropout",
                                            &mut self.creative_fx_state.ntsc_params.vhs_chroma_loss,
                                            0.0..=1.0,
                                            0.05,
                                            "",
                                            slider_w,
                                            palette,
                                        ) {
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                        ui.add_space(2.0);

                                        let mut head_sw =
                                            self.creative_fx_state.ntsc_params.head_switching;
                                        if ui
                                            .checkbox(
                                                &mut head_sw,
                                                egui::RichText::new(
                                                    "Head-Switching Jitter (Bottom)",
                                                )
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                            )
                                            .changed()
                                        {
                                            self.creative_fx_state.ntsc_params.head_switching =
                                                head_sw;
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                    }

                                    ui.add_space(3.0);
                                    ui.separator();
                                    ui.add_space(2.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("🎲 Noise Random Seed")
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(f_med),
                                        );
                                        ui.spacing_mut().slider_width = slider_w;
                                        let mut seed =
                                            self.creative_fx_state.ntsc_params.random_seed;
                                        let slider_response =
                                            ui.add(egui::Slider::new(&mut seed, 0..=1000));
                                        let default_seed = NtscParams::default().random_seed;
                                        let slider_response =
                                            slider_response.on_hover_text(format!(
                                                "Double-click to reset (default: {})",
                                                default_seed
                                            ));
                                        let is_double_click =
                                            slider_double_clicked(ui, &slider_response);
                                        if is_double_click {
                                            seed = default_seed;
                                        }
                                        if slider_response.changed() || is_double_click {
                                            self.creative_fx_state.ntsc_params.random_seed = seed;
                                            self.creative_fx_state.selected_preset_name =
                                                "Custom".to_string();
                                            self.creative_fx_state.mark_dirty();
                                        }
                                    });
                                }
                                CreativeFxType::Structure => {
                                    // Structure Amount: -100..+100
                                    if render_context_slider(
                                        ui,
                                        "⚡ Structure Amount (-100: Soften, +100: Enhance)",
                                        &mut self.creative_fx_state.structure_params.amount,
                                        -100.0..=100.0,
                                        35.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Detail Scale: -50..+50
                                    if render_context_slider(
                                        ui,
                                        "🔍 Detail Scale (-50: Fine, +50: Coarse)",
                                        &mut self.creative_fx_state.structure_params.detail_scale,
                                        -50.0..=50.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Edge Protection: 0..100
                                    if render_context_slider(
                                        ui,
                                        "🛡 Edge Protection (0: Off, 100: Strict)",
                                        &mut self
                                            .creative_fx_state
                                            .structure_params
                                            .edge_protection,
                                        0.0..=100.0,
                                        50.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.creative_fx_state.selected_preset_name =
                                            "Custom".to_string();
                                        self.creative_fx_state.mark_dirty();
                                    }
                                }
                            }

                            // ── Masking Controls ──
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            ui.separator();
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            self.render_creative_fx_mask_sources(ui, ctx, palette, full_w);
                            self.render_creative_fx_mask_options(
                                ui, palette, cur_fx, full_w, slider_w,
                            );

                            ui.add_space(crate::ui_ext::SPACE_SM);
                            ui.separator();
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                            // ── Action Buttons ──
                            let apply_fg = contrast_text_c32(diff_add, Some(f_high), palette);
                            let apply_btn = egui::Button::new(
                                egui::RichText::new("✓ Apply to Image")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(apply_fg),
                            )
                            .fill(diff_add)
                            .stroke(Stroke::new(1.0_f32, diff_add))
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                            let apply_enabled = self.creative_fx_state.preview_rgba.is_some()
                                && self.creative_fx_state.preview_stamp.as_ref()
                                    == Some(&self.creative_fx_render_stamp());
                            if ui
                                .add_enabled(apply_enabled, apply_btn)
                                .on_hover_text(
                                    "Commit Creative FX result to image (pushes to undo stack)",
                                )
                                .clicked()
                            {
                                self.apply_creative_fx();
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let reset_fg = contrast_text_c32(b_med, Some(f_med), palette);
                            let reset_btn = egui::Button::new(
                                egui::RichText::new("⟲ Reset Defaults")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(reset_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0_f32, b_low))
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                            if ui
                                .add(reset_btn)
                                .on_hover_text("Reset parameters to default")
                                .clicked()
                            {
                                match cur_fx {
                                    CreativeFxType::LensAberration => {
                                        self.creative_fx_state.lens_params =
                                            LensAberrationParams::default();
                                    }
                                    CreativeFxType::Trichromatic => {
                                        self.creative_fx_state.trichromatic_params =
                                            TrichromaticParams::default();
                                    }
                                    CreativeFxType::PixelSort => {
                                        self.creative_fx_state.pixel_sort_params =
                                            PixelSortParams::default();
                                    }
                                    CreativeFxType::CmykHalftone => {
                                        self.creative_fx_state.cmyk_params =
                                            CmykHalftoneParams::default();
                                    }
                                    CreativeFxType::DitherPalette => {
                                        self.creative_fx_state.dither_params =
                                            DitherPaletteParams::default();
                                    }
                                    CreativeFxType::Ntsc => {
                                        self.creative_fx_state.ntsc_params = NtscParams::default();
                                    }
                                    CreativeFxType::Structure => {
                                        self.creative_fx_state.structure_params =
                                            StructureParams::default();
                                    }
                                }
                                self.creative_fx_state.selected_preset_name = "Default".to_string();
                                self.creative_fx_state.mark_dirty();
                                self.show_toast("⟲ Parameters reset to defaults");
                            }

                            if self.creative_fx_state.last_render_duration_ms > 0 {
                                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "⏱ {:.2}s",
                                        self.creative_fx_state.last_render_duration_ms as f64
                                            / 1000.0
                                    ))
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                                );
                            }
                        });
                });
        });
        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            "creative_fx",
        );
    }

    /// Split-view comparison painter for the Creative FX tab center canvas.
    #[allow(clippy::too_many_arguments)]

    pub(crate) fn render_creative_fx_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        _ctx: &egui::Context,
        orig_tex: &egui::TextureHandle,
        palette: &AppPalette,
    ) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // Render split comparison using unified SplitCompareView
        let split_view = crate::shell::SplitCompareView::new(
            orig_tex,
            if self.creative_fx_state.preview_stamp.as_ref()
                == Some(&self.creative_fx_render_stamp())
            {
                self.creative_fx_state.preview_texture.as_ref()
            } else {
                None
            },
            &mut self.creative_fx_state.comparison_split,
            &mut self.creative_fx_state.is_dragging_split,
        );
        split_view.paint(ui, response, painter, img_rect, palette);

        // HUD badges
        let badge_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(100.0, 22.0),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0_f32, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            egui::FontId::proportional(11.0),
            hex_to_c32(&palette.colors.text_secondary),
        );

        let cur_fx = self.creative_fx_state.selected_fx;
        let fx_label = if self.creative_fx_state.is_rendering {
            "⏳ Processing…".to_string()
        } else {
            format!(
                "{} {} ({:.1}s)",
                cur_fx.icon(),
                cur_fx.name(),
                self.creative_fx_state.last_render_duration_ms as f64 / 1000.0
            )
        };
        let fx_badge_w = (fx_label.len() as f32 * 6.5).max(130.0);
        let fx_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - fx_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(fx_badge_w, 22.0),
        );
        painter.rect_filled(fx_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(fx_rect, Rounding::same(4.0), Stroke::new(1.0_f32, base_c));
        painter.text(
            fx_rect.center(),
            egui::Align2::CENTER_CENTER,
            fx_label,
            egui::FontId::proportional(10.0),
            base_c,
        );
    }
}
