use super::*;

#[test]
fn cancellable_webp_bytes_match_the_existing_encoder_for_lossy_and_lossless() {
    let image = RgbaImage::from_fn(37, 23, |x, y| {
        image::Rgba([
            (x.wrapping_mul(17) ^ y.wrapping_mul(3)) as u8,
            (y.wrapping_mul(29) ^ x.wrapping_mul(5)) as u8,
            ((x + y).wrapping_mul(11)) as u8,
            ((x * 7 + y * 13) % 256) as u8,
        ])
    });
    for (lossless, quality) in [(false, 83), (true, 100)] {
        let expected = super::super::super::encode_webp(&image, lossless, quality).unwrap();
        let actual =
            super::super::super::encode_webp_cancellable(&image, lossless, quality, None).unwrap();
        assert_eq!(actual, expected, "lossless={lossless}, quality={quality}");
    }
}

#[test]
fn webp_progress_hook_aborts_when_preview_cancel_token_becomes_stale() {
    let image = RgbaImage::from_fn(512, 512, |x, y| {
        image::Rgba([
            (x.wrapping_mul(37) ^ y.wrapping_mul(7)) as u8,
            (y.wrapping_mul(53) ^ x.wrapping_mul(13)) as u8,
            ((x * 19 + y * 31) % 256) as u8,
            255,
        ])
    });
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let callbacks = std::sync::atomic::AtomicUsize::new(0);
    let result = super::super::super::encode_webp_cancellable_with_progress(
        &image,
        false,
        86,
        Some(&cancelled),
        Some(&callbacks),
        true,
    );
    assert_eq!(result.unwrap_err(), "cancelled");
    assert!(callbacks.load(std::sync::atomic::Ordering::Relaxed) > 0);
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
}

#[cfg(unix)]
fn start_sleeping_optipng(
    state: &mut ImagePreviewState,
    label: &str,
) -> (
    u32,
    std::sync::Arc<ImageToolProcessControl>,
    std::path::PathBuf,
) {
    let script =
        std::env::temp_dir().join(format!("clip-optipng-{label}-{}.py", std::process::id()));
    std::fs::write(&script, "import time\ntime.sleep(30)\n").unwrap();
    let command_script = script.clone();
    state.run_optipng_conversion_with(move |_, control| {
        let mut command = Command::new("python3");
        command
            .arg(&command_script)
            .arg("-o2")
            .arg("-quiet")
            .arg("fixture.png");
        let status =
            super::super::background_removal::run_image_tool_status_command(command, control)?;
        if !status.success() {
            return Err(format!("fake optipng exited with {:?}", status.code()));
        }
        Ok(Vec::new())
    });
    let control = state
        .convert_state
        .optipng_process_control
        .as_ref()
        .unwrap()
        .clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while control.child_pid.load(std::sync::atomic::Ordering::SeqCst) == 0
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let pid = control.child_pid.load(std::sync::atomic::Ordering::SeqCst);
    assert_ne!(pid, 0, "OptiPNG child PID was not published");
    (pid, control, script)
}

#[cfg(unix)]
fn assert_optipng_exited(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("OptiPNG child {pid} remained alive after cancellation");
}

#[cfg(unix)]
fn state_with_png_conversion() -> ImagePreviewState {
    let mut state = state();
    state.active_mode = ImageToolMode::Convert;
    state.convert_state.target_format = ConvertTargetFormat::Png;
    state.update_conversion();
    state
}

#[cfg(unix)]
#[test]
fn optipng_process_stops_on_settings_input_change_and_viewer_drop() {
    let mut settings = state_with_png_conversion();
    let (pid, control, script) = start_sleeping_optipng(&mut settings, "settings");
    settings.schedule_conversion();
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_optipng_exited(pid);
    let _ = std::fs::remove_file(script);

    let mut input = state_with_png_conversion();
    let (pid, control, script) = start_sleeping_optipng(&mut input, "input");
    input.mark_pixels_changed();
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_optipng_exited(pid);
    let _ = std::fs::remove_file(script);

    let mut dropped = state_with_png_conversion();
    let (pid, control, script) = start_sleeping_optipng(&mut dropped, "drop");
    drop(dropped);
    assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_optipng_exited(pid);
    let _ = std::fs::remove_file(script);
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
        "conversion stamp test".into(),
        &AppConfig::default(),
    )
    .unwrap()
}

#[test]
fn bounded_conversion_matches_direct_encoder_output_for_tiny_png() {
    let mut state = state();
    state.convert_state.target_format = ConvertTargetFormat::Png;
    state.convert_state.quality = 77;
    let image = state.current_display_image();
    let expected =
        encode_conversion(image, ConvertTargetFormat::Png, 77, false, None, None).unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    state.run_conversion_worker_on(&jobs);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.convert_state.is_encoding && std::time::Instant::now() < deadline {
        state.poll_conversion_on(&jobs);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_conversion_on(&jobs);
    assert!(!state.convert_state.is_encoding);
    assert_eq!(state.convert_state.encoded_bytes, expected.0);
    assert_eq!(
        state.convert_state.converted_rgba.as_ref(),
        Some(&expected.1)
    );
    assert!(state.convert_state.render_error.is_none());
}

#[test]
fn conversion_queue_saturation_retries_after_delay() {
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let (_, queued_rx) = jobs.submit(std::time::Duration::from_secs(5), || ());
    let mut state = state();
    state.convert_state.target_format = ConvertTargetFormat::Png;
    state.run_conversion_worker_on(&jobs);
    let retained_pixels =
        std::sync::Arc::clone(&state.convert_state.render_snapshot.as_ref().unwrap().pixels);
    state.poll_conversion_on(&jobs);
    assert!(state.convert_state.render_retry_at.is_some());
    assert!(std::sync::Arc::ptr_eq(
        &retained_pixels,
        &state.convert_state.render_snapshot.as_ref().unwrap().pixels
    ));
    assert_eq!(
        state.convert_state.render_queue_status.as_deref(),
        Some("Waiting for background workers; retrying shortly")
    );
    assert!(state.convert_state.render_error.is_none());

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    state.convert_state.render_retry_at = Some(std::time::Instant::now());
    assert!(std::sync::Arc::ptr_eq(
        &retained_pixels,
        &state.convert_state.render_snapshot.as_ref().unwrap().pixels
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while (state.convert_state.is_encoding || state.convert_state.render_retry_at.is_some())
        && std::time::Instant::now() < deadline
    {
        state.poll_conversion_on(&jobs);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_conversion_on(&jobs);
    assert!(state.convert_state.render_retry_at.is_none());
    assert!(state.convert_state.render_queue_status.is_none());
    assert!(state.convert_state.output_matches(&state.input_stamp()));
}

#[test]
fn stale_conversion_retry_is_discarded_after_image_change() {
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = state();
    state.active_mode = ImageToolMode::Convert;
    state.convert_state.render_retry_at =
        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
    state.convert_state.render_retry_input_stamp = Some(state.input_stamp());
    state.convert_state.render_queue_status = Some("retrying".into());
    state.mark_pixels_changed();

    state.poll_conversion_on(&jobs);

    assert!(state.convert_state.render_retry_at.is_none());
    assert!(state.convert_state.render_retry_input_stamp.is_none());
    assert!(state.convert_state.render_queue_status.is_none());
    assert!(state.convert_state.params_dirty_at.is_some());
    assert!(!state.convert_state.is_encoding);
}

#[test]
fn dropping_convert_state_cancels_queued_job_and_worker_flag() {
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_, blocker_rx) = jobs.submit(std::time::Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_ran = ran.clone();
    let (ticket, result_rx) = jobs.submit(std::time::Duration::from_secs(5), move || {
        worker_ran.store(true, std::sync::atomic::Ordering::Release);
    });
    let mut state = state();
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.convert_state.render_cancel = Some(cancelled.clone());
    state.convert_state.render_job_ticket = Some(ticket);
    drop(state);
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        Err(JobError::Cancelled)
    );
    assert!(!ran.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn conversion_discards_stale_success_and_stale_failure() {
    for result in [
        Ok((
            vec![1],
            RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
        )),
        Err("stale encode error".into()),
    ] {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        let old_stamp = state.input_stamp();
        state.convert_state.render_input_stamp = Some(old_stamp.clone());
        state.convert_state.is_encoding = true;
        let (tx, rx) = std::sync::mpsc::channel::<Result<ConvertWorkerMsg, JobError>>();
        state.convert_state.render_rx = Some(rx);
        tx.send(Ok(ConvertWorkerMsg {
            generation: state.convert_state.request_generation,
            input_stamp: old_stamp,
            result,
        }))
        .unwrap();
        state.mark_pixels_changed();

        state.poll_conversion();

        assert!(!state.convert_state.is_encoding);
        assert!(state.convert_state.render_rx.is_none());
        assert!(state.convert_state.render_error.is_none());
        assert!(state.convert_state.encoded_bytes.is_empty());
        assert!(state.convert_state.converted_rgba.is_none());
        assert!(state.status_toast.is_none());
    }
}

#[test]
fn conversion_worker_is_cancelled_when_settings_or_input_go_stale() {
    let mut settings = state();
    settings.run_conversion_worker();
    let settings_decoder = std::sync::Arc::new(ImageToolProcessControl::new());
    settings.convert_state.render_decode_control = Some(settings_decoder.clone());
    let cancel = settings
        .convert_state
        .render_cancel
        .as_ref()
        .unwrap()
        .clone();
    settings.schedule_conversion();
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    assert!(
        settings_decoder
            .cancel
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(!settings.convert_state.is_encoding);
    assert!(settings.convert_state.render_rx.is_none());

    let mut input = state();
    input.run_conversion_worker();
    let input_decoder = std::sync::Arc::new(ImageToolProcessControl::new());
    input.convert_state.render_decode_control = Some(input_decoder.clone());
    let cancel = input.convert_state.render_cancel.as_ref().unwrap().clone();
    input.mark_pixels_changed();
    input.poll_conversion();
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    assert!(
        input_decoder
            .cancel
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(!input.convert_state.is_encoding);
    assert!(input.convert_state.render_rx.is_none());

    let mut dropped = state();
    let drop_decoder = std::sync::Arc::new(ImageToolProcessControl::new());
    dropped.convert_state.render_decode_control = Some(drop_decoder.clone());
    drop(dropped);
    assert!(
        drop_decoder
            .cancel
            .load(std::sync::atomic::Ordering::Acquire)
    );
}

#[test]
fn conversion_worker_cancellation_is_checked_before_encoding() {
    let image = RgbaImage::new(4, 4);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(
        encode_conversion(
            image,
            ConvertTargetFormat::Png,
            80,
            false,
            Some(&cancel),
            None
        )
        .unwrap_err(),
        "cancelled"
    );
}

#[cfg(unix)]
#[test]
fn process_decode_failure_uses_the_original_rgba_preview() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("decode-fallback.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf '\\000' > \"$3\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let image = RgbaImage::from_pixel(3, 2, image::Rgba([11, 29, 47, 83]));
    let control = ImageToolProcessControl::new();
    let (_, preview) = encode_conversion(
        image.clone(),
        ConvertTargetFormat::Jpeg,
        80,
        false,
        None,
        Some((&script, &control)),
    )
    .unwrap();
    assert_eq!(preview, image);
}

#[test]
fn disconnected_cancelled_conversion_does_not_report_failure() {
    let mut state = state();
    state.convert_state.is_encoding = true;
    state.convert_state.render_input_stamp = Some(state.input_stamp());
    let (tx, rx) = std::sync::mpsc::channel::<Result<ConvertWorkerMsg, JobError>>();
    drop(tx);
    state.convert_state.render_rx = Some(rx);
    state.convert_state.render_cancel = Some(std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(true),
    ));

    state.poll_conversion();

    assert!(!state.convert_state.is_encoding);
    assert!(state.convert_state.render_error.is_none());
    assert!(state.convert_state.render_error_input_stamp.is_none());
}

#[test]
fn cancellable_jpeg_path_preserves_the_existing_encoded_output() {
    let image = RgbaImage::from_fn(3, 2, |x, y| {
        image::Rgba([x as u8 * 30, y as u8 * 50, 90, (x + y) as u8 * 40])
    });
    assert_eq!(
        encode_jpeg_conversion(&image, 73, None).unwrap(),
        encode_jpeg_with_quality(&image, 73).unwrap()
    );
}

#[test]
fn streaming_png_matches_existing_encoder_bytes_and_rgba() {
    let image = RgbaImage::from_fn(37, 19, |x, y| {
        image::Rgba([
            (x * 17 + y * 3) as u8,
            (y * 13 + x * 5) as u8,
            (x * 7 + y * 11) as u8,
            ((x * 19 + y * 23) % 256) as u8,
        ])
    });
    let streamed = encode_png_conversion(&image, None).unwrap();
    let existing = encode_png(&image).unwrap();

    assert_eq!(streamed, existing);
    let decoded = image::load_from_memory(&streamed).unwrap().to_rgba8();
    assert_eq!(decoded.dimensions(), image.dimensions());
    assert_eq!(decoded, image);
}

#[test]
fn streaming_png_preserves_pixels_for_buffered_encoder_fallback_inputs() {
    let image = RgbaImage::from_fn(4, 4, |x, y| {
        image::Rgba([
            (x * 31 + y * 7) as u8,
            (y * 23 + x * 5) as u8,
            (x * 11 + y * 17) as u8,
            (x * 29 + y * 13) as u8,
        ])
    });
    let streamed = encode_png_conversion(&image, None).unwrap();
    let buffered = encode_png(&image).unwrap();

    assert_ne!(streamed, buffered);
    for encoded in [streamed, buffered] {
        let decoded = image::load_from_memory(&encoded).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (4, 4));
        assert_eq!(decoded, image);
    }
}

#[test]
fn streaming_png_rejects_zero_dimensions() {
    for image in [
        RgbaImage::new(0, 2),
        RgbaImage::new(2, 0),
        RgbaImage::new(0, 0),
    ] {
        assert_eq!(
            encode_png_conversion(&image, None).unwrap_err(),
            "PNG dimensions must be nonzero"
        );
    }
}

struct CancelAfterPngOutput<'a> {
    inner: CancellableVecWriter<'a>,
    cancel: &'a std::sync::atomic::AtomicBool,
    bytes_written: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
}

impl std::io::Write for CancelAfterPngOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = std::io::Write::write(&mut self.inner, bytes)?;
        let total = self
            .bytes_written
            .fetch_add(written, std::sync::atomic::Ordering::AcqRel)
            + written;
        if total >= self.threshold {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}

#[test]
fn png_stream_cancellation_after_idat_output_discards_partial_result() {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let inner = CancellableVecWriter::new(Some(&cancel));
    let output = inner.output.clone();
    let bytes_written = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut sink = CancelAfterPngOutput {
        inner,
        cancel: &cancel,
        bytes_written: bytes_written.clone(),
        threshold: 1024,
    };
    let image = RgbaImage::from_fn(512, 128, |x, y| {
        let seed = x
            .wrapping_mul(1_664_525)
            .wrapping_add(y.wrapping_mul(1_013_904_223));
        image::Rgba([
            seed as u8,
            (seed >> 8) as u8,
            (seed >> 16) as u8,
            (seed >> 24) as u8,
        ])
    });

    assert_eq!(
        write_png_stream(&image, &mut sink, Some(&cancel)).unwrap_err(),
        "cancelled"
    );
    drop(sink);
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    assert!(bytes_written.load(std::sync::atomic::Ordering::Acquire) >= 1024);
    let partial_png = output.lock().unwrap();
    assert!(partial_png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(partial_png.windows(4).any(|chunk| chunk == b"IDAT"));
}

#[test]
fn png_stream_cancellation_at_final_output_does_not_publish_completed_bytes() {
    let image = RgbaImage::from_fn(4, 4, |x, y| {
        image::Rgba([
            (x * 31 + y * 7) as u8,
            (y * 23 + x * 5) as u8,
            (x * 11 + y * 17) as u8,
            (x * 29 + y * 13) as u8,
        ])
    });
    let expected = encode_png_conversion(&image, None).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let inner = CancellableVecWriter::new(Some(&cancel));
    let output = inner.output.clone();
    let bytes_written = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut sink = CancelAfterPngOutput {
        inner,
        cancel: &cancel,
        bytes_written: bytes_written.clone(),
        threshold: expected.len(),
    };

    assert_eq!(
        write_png_stream(&image, &mut sink, Some(&cancel)).unwrap_err(),
        "cancelled"
    );
    drop(sink);
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(
        bytes_written.load(std::sync::atomic::Ordering::Acquire),
        expected.len()
    );
    assert_eq!(*output.lock().unwrap(), expected);
}

#[test]
fn cancellation_aware_encoder_sink_interrupts_writes() {
    use std::io::Write;

    let writer = CancellableVecWriter::new(None);
    let output = writer.output.clone();
    let mut writer = writer;
    writer.write_all(b"partial").unwrap();
    drop(writer);
    assert_eq!(*output.lock().unwrap(), b"partial".to_vec());

    let cancel = std::sync::atomic::AtomicBool::new(true);
    let mut writer = CancellableVecWriter::new(Some(&cancel));
    assert_eq!(
        writer.write(b"discarded").unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
    drop(writer);
}

#[test]
fn cancellation_aware_sink_checks_within_its_output_interval() {
    use std::io::Write;

    let cancel = std::sync::atomic::AtomicBool::new(true);
    let mut writer = CancellableVecWriter::new(Some(&cancel));
    writer.bytes_since_cancel_check = CONVERSION_CANCEL_CHECK_INTERVAL - 1;

    assert_eq!(writer.write(&[1, 2, 3, 4]).unwrap(), 1);
    assert_eq!(
        writer.write(&[5]).unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
}

#[test]
fn conversion_accepts_current_result_and_applies_baked_overlays_once() {
    let mut state = state();
    state.active_mode = ImageToolMode::Convert;
    state.redaction_state.boxes.push(RedactionBox {
        normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
    });
    let input_stamp = state.input_stamp();
    state.convert_state.render_input_stamp = Some(input_stamp.clone());
    state.convert_state.is_encoding = true;
    let output = RgbaImage::from_pixel(4, 4, image::Rgba([7, 8, 9, 255]));
    let (tx, rx) = std::sync::mpsc::channel::<Result<ConvertWorkerMsg, JobError>>();
    state.convert_state.render_rx = Some(rx);
    tx.send(Ok(ConvertWorkerMsg {
        generation: state.convert_state.request_generation,
        input_stamp,
        result: Ok((vec![4, 5, 6], output.clone())),
    }))
    .unwrap();

    state.poll_conversion();
    assert!(state.convert_state.output_matches(&state.input_stamp()));
    assert_eq!(state.convert_state.encoded_bytes, vec![4, 5, 6]);

    state.apply_converted_to_canvas();
    assert_eq!(state.current_rgba.as_ref(), &output);
    assert_eq!(state.pixel_revision, 1);
    assert!(state.redaction_state.boxes.is_empty());
    assert!(!state.convert_state.output_matches(&state.input_stamp()));
    let scheduled_generation = state.convert_state.request_generation;
    assert!(state.convert_state.params_dirty_at.is_some());
    state.set_tool_mode(ImageToolMode::View);
    state.set_tool_mode(ImageToolMode::Convert);
    assert_eq!(state.convert_state.request_generation, scheduled_generation);
}

fn wait_for_optipng(state: &mut ImagePreviewState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.convert_state.is_running_optipng && std::time::Instant::now() < deadline {
        state.poll_conversion();
        std::thread::yield_now();
    }
    assert!(!state.convert_state.is_running_optipng);
}

fn wait_for_optipng_on(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.convert_state.is_running_optipng && std::time::Instant::now() < deadline {
        state.poll_conversion_on(jobs);
        std::thread::yield_now();
    }
    assert!(!state.convert_state.is_running_optipng);
}

#[test]
fn optipng_worker_publishes_current_success_and_preserves_bytes_on_failure() {
    let mut state = state();
    state.convert_state.target_format = ConvertTargetFormat::Png;
    state.update_conversion();
    let original = state.convert_state.encoded_bytes.clone();
    let original_len = original.len();
    let optimized = b"smaller encoded PNG".to_vec();
    let expected_optimized = optimized.clone();
    state.run_optipng_conversion_with(move |bytes, _control| {
        assert_eq!(bytes, original);
        Ok(optimized.clone())
    });
    assert!(state.convert_state.is_running_optipng);
    wait_for_optipng(&mut state);
    assert_eq!(state.convert_state.encoded_bytes, expected_optimized);
    assert_eq!(
        state.convert_state.optipng_savings,
        Some((original_len, expected_optimized.len()))
    );

    state.update_conversion();
    let original = state.convert_state.encoded_bytes.clone();
    state.run_optipng_conversion_with(|_, _control| Err("fixture optimizer failure".to_owned()));
    wait_for_optipng(&mut state);
    assert_eq!(state.convert_state.encoded_bytes, original);
    assert_eq!(state.convert_state.optipng_savings, None);
    assert!(state.status_toast.is_some());
}

#[test]
fn optipng_worker_discards_stale_success_after_input_revision_changes() {
    let mut state = state();
    state.convert_state.target_format = ConvertTargetFormat::Png;
    state.update_conversion();
    let original = state.convert_state.encoded_bytes.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
    let worker_release_rx = release_rx.clone();
    state.run_optipng_conversion_with(move |_, _control| {
        started_tx.send(()).unwrap();
        worker_release_rx.lock().unwrap().recv().unwrap();
        Ok(b"stale optimized PNG".to_vec())
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("optimizer worker should start");
    state.schedule_conversion();
    state.poll_conversion();
    assert_eq!(state.convert_state.encoded_bytes, original);
    release_tx.send(()).unwrap();
    wait_for_optipng(&mut state);
    assert_eq!(state.convert_state.encoded_bytes, original);
    assert_eq!(state.convert_state.optipng_savings, None);
    assert!(state.convert_state.params_dirty_at.is_some());
}

#[test]
fn optipng_queue_full_preserves_output_and_retries_after_delay() {
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
    let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());

    let mut state = state_with_png_conversion();
    let original = state.convert_state.encoded_bytes.clone();
    let original_len = original.len();
    state.convert_state.optipng_savings = Some((original_len + 4, original_len));
    let saved_before = state.convert_state.optipng_savings;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_calls = calls.clone();
    let optimizer: std::sync::Arc<OptipngOptimizer> = std::sync::Arc::new(move |_, _| {
        worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(b"synthetic optimized png".to_vec())
    });
    state.run_optipng_conversion_with_on(optimizer, &jobs);
    state.poll_conversion_on(&jobs);

    assert!(!state.convert_state.is_running_optipng);
    assert!(state.convert_state.optipng_retry_at.is_some());
    assert!(
        state
            .convert_state
            .optipng_queue_status
            .as_deref()
            .is_some_and(|status| status.contains("busy"))
    );
    assert_eq!(state.convert_state.encoded_bytes, original);
    assert_eq!(state.convert_state.optipng_savings, saved_before);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let retry_deadline = state.convert_state.optipng_retry_at.unwrap();
    state.poll_conversion_on(&jobs);
    assert!(!state.convert_state.is_running_optipng);
    assert_eq!(state.convert_state.optipng_retry_at, Some(retry_deadline));

    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    queued_rx.recv().unwrap().unwrap();
    state.convert_state.optipng_retry_at = Some(std::time::Instant::now());
    state.poll_conversion_on(&jobs);
    assert!(state.convert_state.is_running_optipng);
    assert!(state.convert_state.optipng_retry_at.is_none());
    wait_for_optipng_on(&mut state, &jobs);
    state.poll_conversion_on(&jobs);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        state.convert_state.encoded_bytes,
        b"synthetic optimized png"
    );
    assert_eq!(
        state.convert_state.optipng_savings,
        Some((original_len, b"synthetic optimized png".len()))
    );
    assert!(state.convert_state.optipng_queue_status.is_none());
}

#[test]
fn optipng_stale_retry_is_cancelled_for_input_and_settings_changes() {
    for change_input in [false, true] {
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
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let mut state = state_with_png_conversion();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let optimizer: std::sync::Arc<OptipngOptimizer> = std::sync::Arc::new(move |_, _| {
            worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![1, 2, 3])
        });
        state.run_optipng_conversion_with_on(optimizer, &jobs);
        state.poll_conversion_on(&jobs);
        assert!(state.convert_state.optipng_retry_at.is_some());

        if change_input {
            state.mark_pixels_changed();
        } else {
            state.convert_state.quality = state.convert_state.quality.saturating_sub(1);
            state.poll_conversion_on(&jobs);
        }
        assert!(state.convert_state.optipng_retry_at.is_none());
        assert!(state.convert_state.optipng_retry_work.is_none());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
    }
}

#[test]
fn dropping_viewer_cancels_queued_optipng_ticket_and_worker() {
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
    let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
    let mut state = state_with_png_conversion();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_calls = calls.clone();
    let optimizer: std::sync::Arc<OptipngOptimizer> = std::sync::Arc::new(move |_, _| {
        worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![9, 8, 7])
    });
    state.run_optipng_conversion_with_on(optimizer, &jobs);
    let process_control = state
        .convert_state
        .optipng_process_control
        .as_ref()
        .unwrap()
        .clone();
    let worker_rx = state.convert_state.optipng_rx.take().unwrap();
    drop(state);
    assert!(
        process_control
            .cancel
            .load(std::sync::atomic::Ordering::SeqCst)
    );

    release_tx.send(()).unwrap();
    blocker_rx.recv().unwrap().unwrap();
    queued_rx.recv().unwrap().unwrap();
    assert!(matches!(
        worker_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        Err(JobError::Cancelled)
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
