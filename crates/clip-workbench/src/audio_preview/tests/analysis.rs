use super::*;

#[test]
fn source_analysis_snapshots_share_data_while_progress_and_edits_detach() {
    let mut source_data = empty_audio_analysis();
    source_data.ch1_samples = vec![0.25, -0.5].into();
    source_data.transients = vec![0.25];
    let source = Arc::new(source_data);
    let mut state = AudioPreviewState::new(PathBuf::from("source.wav"), "source.wav".into());
    state.analysis = Some(Arc::clone(&source));
    state.completed_source_analysis = Some(Arc::clone(&source));

    let source_snapshot = Arc::clone(state.completed_source_analysis().unwrap());
    assert!(Arc::ptr_eq(&source, &source_snapshot));
    state.set_transient_sensitivity(0.8);
    assert_eq!(source_snapshot.transients, vec![0.25]);
    assert_ne!(
        state.current_transient_markers(),
        source_snapshot.transients
    );
    assert!(Arc::ptr_eq(&source, &source_snapshot));

    let mut edited_data = empty_audio_analysis();
    edited_data.ch1_samples = vec![0.0, 0.0].into();
    state.edited_waveform = Some(Arc::new(edited_data));
    let edited = state.edited_waveform.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&source, edited));
    assert_eq!(&*source.ch1_samples, &[0.25, -0.5]);
    assert_eq!(&*edited.ch1_samples, &[0.0, 0.0]);
}

#[test]
fn completed_source_analysis_is_published_only_after_progress_completes() {
    let (_dir, path) = generate_fixture_wav("completed-analysis", 0.1);
    let mut state = AudioPreviewState::new(path, "completed-analysis.wav".into());
    state.cancel_session_analysis();

    let mut data = empty_audio_analysis();
    data.ch1_samples = vec![0.1, -0.1].into();
    data.sample_rate = 44_100;
    let (progress_tx, progress_rx) = channel();
    progress_tx
        .send(AudioAnalysisProgress::WaveformReady(Box::new(data)))
        .unwrap();
    progress_tx
        .send(AudioAnalysisProgress::TransientsReady(vec![0.25]))
        .unwrap();
    state.analysis_rx = Some(progress_rx);
    let jobs = BackgroundJobs::new(1, 1);
    let ctx = egui::Context::default();
    state.poll_audio_analysis_on(&ctx, 1.0, &jobs);

    assert!(state.completed_source_analysis().is_none());
    state.set_transient_sensitivity(0.8);
    assert_eq!(state.analysis.as_ref().unwrap().transients, vec![0.25]);
    assert_ne!(state.current_transient_markers(), [0.25]);

    let (progress_tx, progress_rx) = channel();
    progress_tx
        .send(AudioAnalysisProgress::SpectrogramAndNotesReady {
            spectrogram_width: 1,
            spectrogram_height: 1,
            spectrogram_rgba: vec![255; 4],
            note_strip_width: 1,
            note_strip_height: 1,
            note_strip_rgba: vec![255; 4],
            note_strip_info: Vec::new(),
            spectral_colors: Vec::new(),
            ultrasonic_roi: Vec::new(),
            ultrasonic_peak_times: Vec::new(),
        })
        .unwrap();
    progress_tx.send(AudioAnalysisProgress::Complete).unwrap();
    state.analysis_rx = Some(progress_rx);
    state.poll_audio_analysis_on(&ctx, 1.0, &jobs);

    let completed = state
        .completed_source_analysis()
        .expect("completed source snapshot should be published");
    assert!(Arc::ptr_eq(state.analysis.as_ref().unwrap(), completed));
    assert_eq!(&*completed.ch1_samples, &[0.1, -0.1]);
    assert_eq!(completed.transients, vec![0.25]);
    assert_ne!(state.current_transient_markers(), completed.transients);

    state.restart_session_analysis_on(&jobs);
    assert!(state.completed_source_analysis().is_none());
    state.cancel_session_analysis();
}

#[test]
fn inline_waveform_and_audio_views_share_analysis_but_keep_view_state_independent() {
    let mut data = empty_audio_analysis();
    data.channels = 1;
    data.sample_rate = 44_100;
    data.duration_ms = 6;
    data.total_frames = 256;
    data.ch1_samples = Arc::from(
        (0..data.total_frames)
            .map(|frame| ((frame as f32 / 256.0) * std::f32::consts::TAU).sin() * 0.5)
            .collect::<Vec<_>>(),
    );
    let source = Arc::new(data);
    let inline = InlineAudioWaveform::new(Arc::clone(&source));
    let path = PathBuf::from("shared-source.wav");
    let mut first = AudioPreviewState::from_completed_source(
        path.clone(),
        "First view".into(),
        None,
        Arc::clone(&source),
    );
    let second = AudioPreviewState::from_completed_source(
        path,
        "Second view".into(),
        None,
        Arc::clone(&source),
    );

    assert!(Arc::ptr_eq(inline.source_analysis(), &source));
    assert!(Arc::ptr_eq(
        first.completed_source_analysis().unwrap(),
        &source
    ));
    assert!(Arc::ptr_eq(
        second.completed_source_analysis().unwrap(),
        &source
    ));
    assert_eq!(inline.source_analysis.total_frames, 256);
    let palette = AppPalette::default();
    let context = egui::Context::default();
    let _ = context.run(Default::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            inline.render(ui, &palette);
        });
    });
    for view in [&first, &second] {
        assert!(view.analysis_rx.is_none());
        assert!(view.analysis_job_rx.is_none());
        assert!(view.analysis_ticket.is_none());
        assert!(view.analysis_cancel.is_none());
        assert!(!view.is_loading_analysis);
    }

    first.current_time_sec = 1.25;
    first.selection = Some((0.25, 0.75));
    first.track_gain_db = 3.0;
    first.region_gain_db.push(-2.0);
    assert_eq!(second.current_time_sec, 0.0);
    assert_eq!(second.selection, None);
    assert_eq!(second.track_gain_db, 0.0);
    assert!(second.region_gain_db.is_empty());
    assert!((source.ch1_samples[64] - 0.5).abs() < 0.000_01);
}

#[test]
fn completed_source_view_keeps_temporary_backing_file_alive() {
    use std::io::Write;

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"fixture audio source").unwrap();
    let path = file.path().to_owned();
    let file_guard = file.into_temp_path();
    let source = Arc::new(empty_audio_analysis());
    let state = AudioPreviewState::from_completed_source_with_file_guard(
        path.clone(),
        "Gallery audio".to_owned(),
        None,
        source,
        file_guard,
    );

    assert!(path.exists());
    assert_eq!(state.path, path);
    assert!(!state.is_loading_analysis);

    drop(state);
    assert!(!path.exists());
}

#[test]
fn inline_waveform_column_count_tracks_visible_width() {
    assert_eq!(InlineAudioWaveform::visible_column_count(0.0), 1);
    assert_eq!(InlineAudioWaveform::visible_column_count(96.0), 96);
    assert_eq!(InlineAudioWaveform::visible_column_count(96.2), 97);
    assert_eq!(
        InlineAudioWaveform::visible_column_count(f32::INFINITY),
        MAX_INLINE_WAVEFORM_COLUMNS
    );
    assert_eq!(
        InlineAudioWaveform::visible_column_count(MAX_INLINE_WAVEFORM_COLUMNS as f32 * 2.0),
        MAX_INLINE_WAVEFORM_COLUMNS
    );

    let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1_000.0, 44.0));
    let visible = Rect::from_min_max(Pos2::new(120.0, 0.0), Pos2::new(220.0, 44.0));
    assert_eq!(
        InlineAudioWaveform::visible_column_range(rect, visible, 1_000),
        Some(120..220)
    );
    let outside = Rect::from_min_max(Pos2::new(1_100.0, 0.0), Pos2::new(1_200.0, 44.0));
    assert_eq!(
        InlineAudioWaveform::visible_column_range(rect, outside, 1_000),
        None
    );
}

#[test]
fn waveform_image_delivery_captures_identical_left_png_and_clipboard_payloads() {
    let mut analysis = empty_audio_analysis();
    analysis.sample_rate = 4;
    analysis.duration_ms = 1_000;
    analysis.total_frames = 4;
    analysis.ch1_samples = Arc::from([0.25, -0.5, 0.75, -0.25]);
    analysis.ch2_samples = Some(Arc::from([-0.25, 0.5, -0.75, 0.25]));
    let palette = AppPalette::default();
    let mut sent = None;
    let mut copied = None;

    AudioPreviewState::copy_waveform_image_on(
        &analysis,
        &analysis,
        &[],
        0.0,
        1.0,
        1.0,
        &palette,
        |request| sent = Some(request.clone()),
        |bytes, mime| copied = Some((bytes.to_vec(), mime.to_string())),
    )
    .expect("synthetic waveform should render and deliver");

    let IpcRequest::PushImage { bytes, mime, hand } = sent.expect("image request sent") else {
        panic!("waveform delivery should send an image");
    };
    let (copied_bytes, copied_mime) = copied.expect("clipboard callback invoked");
    assert_eq!(hand, little_clip_core::HandSide::Left);
    assert_eq!(mime, "image/png");
    assert_eq!(copied_mime, "image/png");
    assert_eq!(bytes, copied_bytes);

    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .expect("delivered payload is a valid PNG");
    assert_eq!(decoded.width(), tools::export_image::EXPORT_IMAGE_WIDTH);
    assert_eq!(decoded.height(), 340);
    let (r, g, b, a) =
        little_clip_core::color::parse_hex_color_rgba_or_white(&palette.colors.canvas);
    assert_eq!(decoded.to_rgba8().get_pixel(0, 0).0, [r, g, b, a]);

    let mut silent_analysis = analysis.clone();
    silent_analysis.ch1_samples = Arc::from([0.0; 4]);
    silent_analysis.ch2_samples = Some(Arc::from([0.0; 4]));
    let mut silent_png = None;
    AudioPreviewState::copy_waveform_image_on(
        &silent_analysis,
        &silent_analysis,
        &[],
        0.0,
        1.0,
        1.0,
        &palette,
        |_| {},
        |silent_bytes, _| silent_png = Some(silent_bytes.to_vec()),
    )
    .expect("silent comparison waveform should render");
    let silent_png = silent_png.expect("comparison copy callback invoked");
    let silent_pixels = image::load_from_memory_with_format(&silent_png, image::ImageFormat::Png)
        .expect("silent comparison payload is a valid PNG")
        .to_rgba8();
    assert_ne!(decoded.to_rgba8().as_raw(), silent_pixels.as_raw());
}

#[test]
fn progressive_analysis_queue_saturation_retries_without_losing_loading_state() {
    let (_dir, path) = generate_fixture_wav("queue_retry", 0.1);
    let metadata = read_audio_metadata(&path);
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::from_secs(5), || ());

    let mut state = AudioPreviewState::new_with_metadata_on_jobs(
        path,
        "queue_retry.wav".into(),
        metadata,
        &jobs,
    );
    let ctx = egui::Context::default();
    state.poll_audio_analysis_on(&ctx, 1.0, &jobs);
    assert!(state.is_loading_analysis);
    assert!(state.analysis_retry_at.is_some());
    assert!(
        state
            .status_toast
            .as_ref()
            .is_some_and(|(message, _)| { message.contains("waiting for background workers") })
    );

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    state.analysis_retry_at = Some(Instant::now() - Duration::from_millis(1));
    state.poll_audio_analysis_on(&ctx, 2.0, &jobs);
    assert!(state.analysis_retry_at.is_none());
    assert!(state.analysis_ticket.is_some() || state.analysis.is_some());
}

#[test]
fn source_restart_cancels_old_analysis_and_drops_its_progress_receiver() {
    let (_dir, path) = generate_fixture_wav("source_restart", 0.1);
    let jobs = BackgroundJobs::new(1, 2);
    let mut state = AudioPreviewState::new_with_metadata_on_jobs(
        path,
        "source_restart.wav".into(),
        None,
        &jobs,
    );
    let old_cancel = state.analysis_cancel.as_ref().unwrap().clone();
    state.restart_session_analysis_on(&jobs);
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(state.analysis_rx.is_some());
    assert!(state.analysis_ticket.is_some());
}

#[test]
fn stale_edit_progress_does_not_replace_the_current_edited_waveform() {
    let (_dir, path) = generate_fixture_wav("stale_edit", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let mut state =
        AudioPreviewState::new_with_metadata_on_jobs(path, "stale_edit.wav".into(), None, &jobs);
    state.analysis = Some(Arc::new(empty_audio_analysis()));
    state.edit_analysis_observed = state.session_audio_edits();
    state.edit_analysis_inflight = SessionAudioEdits {
        track_gain_db: 1.0,
        regions: Vec::new(),
    };
    let (tx, rx) = channel();
    tx.send(AudioAnalysisProgress::WaveformReady(Box::new(
        empty_audio_analysis(),
    )))
    .unwrap();
    state.edit_analysis_rx = Some(rx);
    state.poll_edit_analysis(&egui::Context::default(), 1.0, &jobs);
    assert!(state.edited_waveform.is_none());
    assert_ne!(state.edit_analysis_inflight, state.edit_analysis_observed);
}

#[test]
fn disconnected_current_edit_worker_reports_error_without_resubmitting() {
    let (_dir, path) = generate_fixture_wav("edit_disconnect", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = AudioPreviewState::new_with_metadata_on_jobs(
        path,
        "edit_disconnect.wav".into(),
        None,
        &jobs,
    );
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(empty_audio_analysis()));
    state.edit_analysis_observed = state.session_audio_edits();
    state.edit_analysis_inflight = state.edit_analysis_observed.clone();
    state.edit_analysis_displayed = SessionAudioEdits {
        track_gain_db: 1.0,
        regions: Vec::new(),
    };
    let (tx, rx) = channel::<AudioAnalysisProgress>();
    drop(tx);
    state.edit_analysis_rx = Some(rx);
    let (job_tx, job_rx) = channel::<Result<(), JobError>>();
    drop(job_tx);
    state.edit_analysis_job_rx = Some(job_rx);

    state.poll_edit_analysis(&egui::Context::default(), 1.0, &jobs);

    assert_eq!(state.edit_analysis_displayed, state.edit_analysis_observed);
    assert!(state.edit_analysis_rx.is_none());
    assert!(state.edit_analysis_ticket.is_none());
    assert!(state.status_toast.as_ref().is_some_and(|(message, _)| {
        message.contains("stopped before completing")
            || message.contains("disconnected unexpectedly")
    }));
}

#[test]
fn disconnected_initial_analysis_progress_reports_visible_error() {
    let (_dir, path) = generate_fixture_wav("analysis_disconnect", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let mut state = AudioPreviewState::new_with_metadata_on_jobs(
        path,
        "analysis_disconnect.wav".into(),
        None,
        &jobs,
    );
    state.cancel_session_analysis();
    state.is_loading_analysis = true;
    let (tx, rx) = channel::<AudioAnalysisProgress>();
    drop(tx);
    state.analysis_rx = Some(rx);
    let (job_tx, job_rx) = channel::<Result<(), JobError>>();
    drop(job_tx);
    state.analysis_job_rx = Some(job_rx);

    state.poll_audio_analysis_on(&egui::Context::default(), 1.0, &jobs);

    assert!(!state.is_loading_analysis);
    assert!(state.status_toast.as_ref().is_some_and(|(message, _)| {
        message.contains("stopped before completing")
            || message.contains("disconnected unexpectedly")
    }));
}

#[test]
fn edited_analysis_queue_saturation_keeps_existing_waveform_and_retries() {
    let (_dir, path) = generate_fixture_wav("edit_queue_retry", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::from_secs(5), || ());

    let mut state = AudioPreviewState::new_with_metadata_on_jobs(
        path,
        "edit_queue_retry.wav".into(),
        None,
        &jobs,
    );
    state.analysis_rx = None;
    state.analysis_job_rx = None;
    state.analysis_cancel = None;
    state.analysis_ticket = None;
    state.analysis = Some(Arc::new(empty_audio_analysis()));
    state.edited_waveform = Some(Arc::new(empty_audio_analysis()));
    state.edit_analysis_observed = state.session_audio_edits();
    state.edit_analysis_displayed = SessionAudioEdits {
        track_gain_db: 1.0,
        regions: Vec::new(),
    };
    state.edit_change_at = Instant::now() - Duration::from_secs(1);
    state.start_edit_analysis_job(&jobs);

    state.poll_edit_analysis(&egui::Context::default(), 1.0, &jobs);
    assert!(state.edit_analysis_retry_at.is_some());
    assert!(state.analysis.is_some());
    assert!(state.edited_waveform.is_some());

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    state.edit_analysis_retry_at = Some(Instant::now() - Duration::from_millis(1));
    state.poll_edit_analysis(&egui::Context::default(), 2.0, &jobs);
    assert!(state.edit_analysis_retry_at.is_none());
    assert!(
        state.edit_analysis_ticket.is_some()
            || state.edit_analysis_displayed == state.edit_analysis_observed
    );
    assert!(state.analysis.is_some());
}

#[test]
fn cancelled_core_progressive_analysis_publishes_no_progress() {
    let (_dir, path) = generate_fixture_wav("analysis_cancel", 0.1);
    let cancelled = AtomicBool::new(true);
    let mut messages = Vec::new();
    let analysis =
        compute_audio_analysis_progressive_cancellable(&path, 1200, &cancelled, |progress| {
            messages.push(progress)
        });
    assert!(analysis.is_none());
    assert!(messages.is_empty());
}

#[test]
fn dropping_audio_viewer_signals_cancellation_for_queued_analysis() {
    let (_dir, path) = generate_fixture_wav("drop_cancel", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let state =
        AudioPreviewState::new_with_metadata_on_jobs(path, "drop_cancel.wav".into(), None, &jobs);
    let cancelled = state.analysis_cancel.as_ref().unwrap().clone();
    drop(state);
    assert!(cancelled.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
}

#[test]
fn digital_decode_queue_saturation_retries_without_clearing_current_symbols() {
    let (_dir, path) = generate_fixture_wav("digital_queue_retry", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::from_secs(5), || ());

    let mut state = AudioPreviewState::new(path, "digital_queue_retry.wav".into());
    state.analysis = Some(Arc::new(empty_audio_analysis()));
    state
        .digital_decode
        .symbols
        .push(little_clip_ops::digital_decode::DecodedSymbol {
            protocol: little_clip_ops::digital_decode::DecoderProtocol::Dtmf,
            symbol: "old".into(),
            start_sec: 0.0,
            end_sec: 0.1,
            confidence: 1.0,
            details: None,
        });
    state.start_digital_decode_on(&jobs);
    for _ in 0..100 {
        state.poll_digital_decode_on(&egui::Context::default(), &jobs);
        if state.digital_decode.retry_at.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(state.digital_decode.retry_at.is_some());
    assert_eq!(state.digital_decode.symbols[0].symbol, "old");

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    state.digital_decode.retry_at = Some(Instant::now());
    state.poll_digital_decode_on(&egui::Context::default(), &jobs);
    for _ in 0..100 {
        state.poll_digital_decode_on(&egui::Context::default(), &jobs);
        if !state.digital_decode.in_progress {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        state.digital_decode.status_msg.as_deref(),
        Some("Decoded 0 symbols in 0.00s")
    );
}

#[test]
fn digital_decode_rescan_and_viewer_drop_cancel_queued_jobs() {
    let (_dir, path) = generate_fixture_wav("digital_cancel", 0.1);
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut state = AudioPreviewState::new(path, "digital_cancel.wav".into());
    state.analysis = Some(Arc::new(empty_audio_analysis()));
    state.start_digital_decode_on(&jobs);
    let old_flag = state.digital_decode.cancel_flag.as_ref().unwrap().clone();
    state.start_digital_decode_on(&jobs);
    assert!(old_flag.load(Ordering::Acquire));
    let drop_flag = state.digital_decode.cancel_flag.as_ref().unwrap().clone();
    drop(state);
    assert!(drop_flag.load(Ordering::Acquire));

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
}

#[test]
fn progressive_source_checks_keep_earlier_results_and_reject_changed_stages() {
    let changed = Arc::new(AtomicBool::new(false));
    let observed = changed.clone();
    let check = crate::file_source::ReadCheck::new(move |_| {
        if observed.load(Ordering::Acquire) {
            Err("source context expired".into())
        } else {
            Ok(())
        }
    });
    let cancel = AtomicBool::new(false);
    let (tx, rx) = channel();
    checked_analysis(&check, &cancel, &tx, |send| {
        send(AudioAnalysisProgress::TransientsReady(vec![0.25]));
        changed.store(true, Ordering::Release);
        send(AudioAnalysisProgress::TransientsReady(vec![0.5]));
        send(AudioAnalysisProgress::Complete);
    });
    assert!(
        matches!(rx.try_recv().unwrap(), AudioAnalysisProgress::TransientsReady(v) if v == vec![0.25])
    );
    assert!(
        matches!(rx.try_recv().unwrap(), AudioAnalysisProgress::Failed(e) if e == "source context expired")
    );
    assert!(rx.try_recv().is_err());
    assert!(cancel.load(Ordering::Acquire));
    checked_analysis(&check, &AtomicBool::new(false), &tx, |_| {
        panic!("must not reopen a stale source")
    });
    assert!(
        matches!(rx.try_recv().unwrap(), AudioAnalysisProgress::Failed(e) if e == "source context expired")
    );
}

#[test]
fn checked_audio_analysis_rejects_initial_retries_and_edited_source_reads() {
    let jobs = BackgroundJobs::new(1, 2);
    let check = crate::file_source::ReadCheck::new(|_| Err("source context expired".into()));
    let mut state = AudioPreviewState::with_source_analysis_on_jobs(
        PathBuf::from("missing-source.wav"),
        "Checked audio".into(),
        None,
        &jobs,
        None,
        check,
    );
    for retry in 0..3 {
        if retry == 1 {
            state.restart_session_analysis_on(&jobs);
        }
        if retry == 2 {
            state.start_session_analysis_job(&jobs);
        }
        let progress = state
            .analysis_rx
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(
            matches!(progress, AudioAnalysisProgress::Failed(e) if e == "source context expired")
        );
        state
            .analysis_job_rx
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        state.cancel_session_analysis();
    }
    // Both gain-only and spectral-filter edits reopen the original input.
    for filtered in [false, true] {
        if filtered {
            state
                .edit_analysis_observed
                .regions
                .push(little_clip_core::audio::AudioRegionEdit {
                    low_pass_hz: Some(1000.0),
                    ..Default::default()
                });
        }
        state.start_edit_analysis_job(&jobs);
        let progress = state
            .edit_analysis_rx
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(
            matches!(progress, AudioAnalysisProgress::Failed(e) if e == "source context expired")
        );
        state
            .edit_analysis_job_rx
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        state.cancel_edit_analysis();
    }
}
