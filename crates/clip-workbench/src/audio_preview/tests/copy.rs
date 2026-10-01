use super::*;

#[test]
fn single_slice_copy_plan_keeps_range_active_filter_and_name() {
    let region = AudioRegionEdit {
        gain_db: 6.0,
        soft_clip: true,
        low_pass_hz: Some(1200.0),
        high_pass_hz: Some(80.0),
        ..Default::default()
    };
    let filter = AudioPreviewState::channel_edit_filter(ChannelCopyMode::RightOnly, region);
    let plan = audio_slice_copy_plan(
        Path::new("/music/Track.wav"),
        1.25,
        2.75,
        ChannelCopyMode::RightOnly,
        filter.clone(),
    );
    assert_eq!((plan.start, plan.end), (1.25, 2.75));
    assert_eq!(plan.source, PathBuf::from("/music/Track.wav"));
    assert_eq!(
        plan.output.file_name().unwrap(),
        "Track_00m01s250ms_01s500ms_right_mono.wav"
    );
    assert_eq!(plan.filter, filter);
    assert!(
        plan.filter
            .unwrap()
            .starts_with("pan=mono|c0=c1,volume=6dB,asoftclip")
    );
}

#[test]
fn single_slice_handoff_uses_left_slot_and_skips_clipboard_when_daemon_succeeds() {
    let path = Path::new("/tmp/audio slice.wav");
    let cancelled = AtomicBool::new(false);
    let mut daemon_called = false;
    let result = handoff_audio_slice(
        path,
        &cancelled,
        |request| {
            daemon_called = true;
            match request {
                IpcRequest::PushFile {
                    path: sent_path,
                    hand,
                } => {
                    assert_eq!(sent_path, path);
                    assert_eq!(*hand, little_clip_core::HandSide::Left);
                }
                other => panic!("unexpected request: {other:?}"),
            }
            Ok(())
        },
        |_| panic!("clipboard fallback is skipped after daemon success"),
    );
    assert_eq!(result, Ok(AudioSliceDelivery::Daemon));
    assert!(daemon_called);
}

#[test]
fn single_slice_handoff_falls_back_and_reports_both_failures() {
    let path = Path::new("/tmp/audio slice.wav");
    let cancelled = AtomicBool::new(false);
    let mut copied_path = None;
    let result = handoff_audio_slice(
        path,
        &cancelled,
        |_| Err("daemon unavailable".to_owned()),
        |clipboard_path| {
            copied_path = Some(clipboard_path.to_owned());
            Ok(())
        },
    );
    assert_eq!(result, Ok(AudioSliceDelivery::SystemClipboard));
    assert_eq!(copied_path.as_deref(), Some(path));

    let error = handoff_audio_slice(
        path,
        &cancelled,
        |_| Err("daemon error".into()),
        |_| Err("clipboard error".into()),
    )
    .unwrap_err();
    assert!(error.contains("daemon error"));
    assert!(error.contains("clipboard error"));
}

#[test]
fn cancelled_single_slice_copy_does_not_publish() {
    let path = Path::new("/tmp/audio slice.wav");
    let cancelled = AtomicBool::new(true);
    let published = AtomicBool::new(false);
    let result = handoff_audio_slice(
        path,
        &cancelled,
        |_| {
            published.store(true, Ordering::Release);
            Ok(())
        },
        |_| {
            published.store(true, Ordering::Release);
            Ok(())
        },
    );
    assert_eq!(result, Err("Audio slice copy cancelled".to_owned()));
    assert!(!published.load(Ordering::Acquire));
}

#[test]
fn audio_batch_copy_preserves_selection_order_and_pushes_ordered_files() {
    let plans = [(0.8, 1.2), (0.1, 0.4), (2.0, 3.0)]
        .into_iter()
        .map(|(start, end)| {
            audio_slice_copy_plan(
                Path::new("/music/Track.wav"),
                start,
                end,
                ChannelCopyMode::LeftOnly,
                Some("pan=mono|c0=c0,volume=3dB".to_owned()),
            )
        })
        .collect::<Vec<_>>();
    let expected = plans
        .iter()
        .map(|plan| plan.output.clone())
        .collect::<Vec<_>>();
    let sent = std::cell::RefCell::new(None);
    let result = run_audio_slice_batch(
        &plans,
        ChannelCopyMode::LeftOnly,
        &AtomicBool::new(false),
        |plan, _| Ok(plan.output.clone()),
        |request| {
            match request {
                IpcRequest::PushFiles { paths, hand } => {
                    assert_eq!(*hand, little_clip_core::HandSide::Left);
                    *sent.borrow_mut() = Some(paths.clone());
                }
                other => panic!("expected ordered batch request, received {other:?}"),
            }
            Ok(())
        },
        |_| panic!("clipboard fallback is skipped after daemon success"),
    )
    .unwrap();
    assert_eq!(sent.into_inner().unwrap(), expected);
    assert!(result.contains("Copied 3/3 audio LEFT (mono) slices"));
    let first = result.find("00m00s800ms").unwrap();
    let second = result.find("00m00s100ms").unwrap();
    let third = result.find("00m02s000ms").unwrap();
    assert!(first < second && second < third);
}

#[test]
fn audio_inspect_left_copy_extracts_exact_mono_payload_for_left_hand() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("stereo-source.wav");
    let sample_rate = 8_000;
    let left_samples = (0..4_000).map(|sample| sample as i16).collect::<Vec<_>>();
    let right_samples = (0..4_000)
        .map(|sample| -(sample as i16) - 1)
        .collect::<Vec<_>>();
    write_stereo_pcm_fixture(&source, &left_samples, &right_samples, sample_rate);

    let start = 0.125;
    let end = 0.375;
    let filter = AudioPreviewState::channel_edit_filter(
        ChannelCopyMode::LeftOnly,
        AudioRegionEdit::default(),
    );
    assert_eq!(filter.as_deref(), Some("pan=mono|c0=c0"));
    let mut plan = audio_slice_copy_plan(&source, start, end, ChannelCopyMode::LeftOnly, filter);
    assert_eq!((plan.start, plan.end), (start, end));
    plan.output = temp.path().join("selected-left.wav");

    let first_sample = (start * sample_rate as f32) as usize;
    let sample_count = ((end - start) * sample_rate as f32) as usize;
    let expected_samples = left_samples[first_sample..first_sample + sample_count].to_vec();
    let expected_output = plan.output.clone();
    let mut sent_path = None;
    let result = run_audio_slice_batch(
        &[plan],
        ChannelCopyMode::LeftOnly,
        &AtomicBool::new(false),
        extract_audio_slice_file,
        |request| match request {
            IpcRequest::PushFile { path, hand } => {
                assert_eq!(*hand, little_clip_core::HandSide::Left);
                assert_eq!(path, &expected_output);
                sent_path = Some(path.clone());
                let (channels, output_rate, output_samples) = read_pcm16_wav(path);
                assert_eq!(channels, 1);
                assert_eq!(output_rate, sample_rate);
                assert_eq!(output_samples, expected_samples);
                Ok(())
            }
            other => panic!("expected one PushFile request, got {other:?}"),
        },
        |_| panic!("clipboard fallback is skipped after fake daemon success"),
    )
    .expect("production FFmpeg extraction and fake handoff should succeed");
    assert_eq!(sent_path.as_deref(), Some(expected_output.as_path()));
    assert!(result.contains("LEFT (mono)"));
    assert!(result.contains("to Left Hand"));
}

#[test]
fn audio_batch_copy_reports_mixed_and_total_failures() {
    let plans = [(0.0, 0.5), (1.0, 1.5), (2.0, 2.5)]
        .into_iter()
        .map(|(start, end)| {
            audio_slice_copy_plan(
                Path::new("/music/Track.wav"),
                start,
                end,
                ChannelCopyMode::Native,
                None,
            )
        })
        .collect::<Vec<_>>();
    let result = run_audio_slice_batch(
        &plans,
        ChannelCopyMode::Native,
        &AtomicBool::new(false),
        |plan, _| {
            if plan.start == 1.0 {
                Err("ffmpeg exit status: 1".to_owned())
            } else {
                Ok(plan.output.clone())
            }
        },
        |request| {
            match request {
                IpcRequest::PushFiles { paths, hand } => {
                    assert_eq!(paths.len(), 2);
                    assert_eq!(*hand, little_clip_core::HandSide::Left);
                }
                other => panic!("expected PushFiles, received {other:?}"),
            }
            Ok(())
        },
        |_| panic!("clipboard fallback is skipped after daemon success"),
    )
    .unwrap();
    assert!(result.contains("Copied 2/3"));
    assert!(result.contains("failed: Track_00m01s000ms_00s500ms.wav: ffmpeg exit status: 1"));

    let error = run_audio_slice_batch(
        &plans,
        ChannelCopyMode::Native,
        &AtomicBool::new(false),
        |plan, _| Err(format!("{} could not be decoded", plan.source.display())),
        |_| panic!("failed exports must not reach daemon"),
        |_| panic!("failed exports must not reach clipboard"),
    )
    .unwrap_err();
    assert!(error.contains("All 3 audio slices failed"));
    assert!(error.contains("Track_00m00s000ms_00s500ms.wav"));
    assert!(error.contains("Track_00m02s000ms_00s500ms.wav"));
}

#[test]
fn cancelled_audio_batch_never_extracts_or_hands_off() {
    let plans = vec![audio_slice_copy_plan(
        Path::new("/music/Track.wav"),
        0.0,
        1.0,
        ChannelCopyMode::Native,
        None,
    )];
    let result = run_audio_slice_batch(
        &plans,
        ChannelCopyMode::Native,
        &AtomicBool::new(true),
        |_, _| panic!("cancelled batch must not start extraction"),
        |_| panic!("cancelled batch must not reach daemon"),
        |_| panic!("cancelled batch must not reach clipboard"),
    );
    assert_eq!(result, Err("Audio slice copy cancelled".to_owned()));
}

#[test]
fn audio_batch_handoff_uses_ordered_clipboard_fallback() {
    let paths = vec![
        PathBuf::from("/tmp/first.wav"),
        PathBuf::from("/tmp/second.wav"),
    ];
    let mut copied = None;
    let result = handoff_audio_files(
        &paths,
        &AtomicBool::new(false),
        |request| {
            assert!(
                matches!(request, IpcRequest::PushFiles { paths: sent, hand: little_clip_core::HandSide::Left } if sent == &paths)
            );
            Err("daemon offline".to_owned())
        },
        |fallback_paths| {
            copied = Some(fallback_paths.to_vec());
            Ok(())
        },
    );
    assert_eq!(result, Ok(AudioSliceDelivery::SystemClipboard));
    assert_eq!(copied, Some(paths));
}

#[test]
fn single_slice_copy_surfaces_ffmpeg_failure() {
    let temp = tempfile::tempdir().unwrap();
    let plan = AudioSliceCopyPlan {
        source: temp.path().join("missing.wav"),
        output: temp.path().join("copy.wav"),
        start: 0.0,
        end: 0.5,
        filter: None,
    };
    let error = extract_and_handoff_audio_slice(plan, &AtomicBool::new(false)).unwrap_err();
    assert!(error.contains("Failed to extract audio segment via ffmpeg"));
}
