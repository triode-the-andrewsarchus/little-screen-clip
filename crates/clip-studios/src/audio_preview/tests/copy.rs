use super::*;

fn fake_slice_artifact(
    plan: &AudioSliceCopyPlan,
    _: &AtomicBool,
) -> Result<AudioSliceArtifact, String> {
    let directory =
        little_clip_core::kernel_interface::create_content_directory(plan.output.parent().unwrap())
            .map_err(|error| error.to_string())?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.output)
        .map_err(|error| error.to_string())?;
    Ok(AudioSliceArtifact::new(
        plan.output.clone(),
        directory,
        file,
    ))
}

fn place_slice_plans(plans: &mut [AudioSliceCopyPlan], root: &Path) {
    for (index, plan) in plans.iter_mut().enumerate() {
        plan.output = root
            .join(index.to_string())
            .join(plan.output.file_name().unwrap());
    }
}

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
fn single_slice_handoff_publishes_clipboard_even_when_daemon_succeeds() {
    let path = Path::new("/tmp/audio slice.wav");
    let cancelled = AtomicBool::new(false);
    let mut daemon_called = false;
    let mut copied = None;
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
        |path| {
            copied = Some(path.to_path_buf());
            Ok(())
        },
    );
    assert_eq!(result, Ok(AudioSliceDelivery::Daemon));
    assert!(daemon_called);
    assert_eq!(copied.as_deref(), Some(path));
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
fn successful_daemon_push_does_not_hide_a_failed_desktop_copy() {
    let error = handoff_audio_slice(
        Path::new("/tmp/segment.wav"),
        &AtomicBool::new(false),
        |_| Ok(()),
        |_| Err("selection unavailable".into()),
    )
    .unwrap_err();
    assert!(error.contains("system clipboard failed"));
    assert!(error.contains("selection unavailable"));
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
    let root = tempfile::tempdir().unwrap();
    let mut plans = [(0.8, 1.2), (0.1, 0.4), (2.0, 3.0)]
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
    place_slice_plans(&mut plans, root.path());
    let expected = plans
        .iter()
        .map(|plan| plan.output.clone())
        .collect::<Vec<_>>();
    let sent = std::cell::RefCell::new(None);
    let result = run_audio_slice_batch(
        &plans,
        ChannelCopyMode::LeftOnly,
        &AtomicBool::new(false),
        fake_slice_artifact,
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
        |paths| {
            assert_eq!(paths, expected.as_slice());
            Ok(())
        },
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
        extract_audio_slice_owned,
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
        |paths| {
            assert_eq!(paths, std::slice::from_ref(&expected_output));
            Ok(())
        },
    )
    .expect("production FFmpeg extraction and fake handoff should succeed");
    assert_eq!(sent_path.as_deref(), Some(expected_output.as_path()));
    assert!(result.contains("LEFT (mono)"));
    assert!(result.contains("to Left Hand"));
}

#[test]
fn audio_batch_copy_reports_mixed_and_total_failures() {
    let root = tempfile::tempdir().unwrap();
    let mut plans = [(0.0, 0.5), (1.0, 1.5), (2.0, 2.5)]
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
    place_slice_plans(&mut plans, root.path());
    let result = run_audio_slice_batch(
        &plans,
        ChannelCopyMode::Native,
        &AtomicBool::new(false),
        |plan, _| {
            if plan.start == 1.0 {
                Err("ffmpeg exit status: 1".to_owned())
            } else {
                fake_slice_artifact(plan, &AtomicBool::new(false))
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
        |paths| {
            assert_eq!(paths, &[plans[0].output.clone(), plans[2].output.clone()]);
            Ok(())
        },
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
        read_check: Default::default(),
        file_source: None,
        source: temp.path().join("missing.wav"),
        output: temp.path().join("copy.wav"),
        start: 0.0,
        end: 0.5,
        filter: None,
    };
    let error = extract_and_handoff_audio_slice(plan, &AtomicBool::new(false)).unwrap_err();
    assert!(error.contains("Failed to extract audio segment via ffmpeg"));
}

#[test]
fn slice_copy_rejects_changed_displayed_revision_before_output_admission() {
    let (_fixture, source) = generate_fixture_wav("stale-slice-copy", 1.0);
    let expected = crate::file_source::FileSource::capture(&source).unwrap();
    let mut plan = audio_slice_copy_plan(&source, 0.1, 0.5, ChannelCopyMode::Native, None)
        .with_file_source(expected);
    let root = tempfile::tempdir().unwrap();
    plan.output = root.path().join("must-not-create").join("copy.wav");
    std::fs::write(&source, b"changed snapshot").unwrap();
    let error = extract_audio_slice_file(&plan, &AtomicBool::new(false)).unwrap_err();
    assert!(error.contains("changed"));
    assert!(!plan.output.parent().unwrap().exists());
}

#[test]
fn cancellation_during_audio_batch_preparation_removes_unpublished_slices() {
    for cancel_after in [0, 1] {
        let root = tempfile::tempdir().unwrap();
        let mut plans = [0.0, 1.0]
            .into_iter()
            .map(|start| {
                audio_slice_copy_plan(
                    Path::new("source.wav"),
                    start,
                    start + 0.5,
                    ChannelCopyMode::Native,
                    None,
                )
            })
            .collect::<Vec<_>>();
        place_slice_plans(&mut plans, root.path());
        let cancelled = AtomicBool::new(false);
        let mut extracted = 0;
        let result = run_audio_slice_batch(
            &plans,
            ChannelCopyMode::Native,
            &cancelled,
            |plan, cancel| {
                let artifact = fake_slice_artifact(plan, cancel)?;
                if extracted == cancel_after {
                    cancelled.store(true, Ordering::Release);
                }
                extracted += 1;
                Ok(artifact)
            },
            |_| panic!("cancelled preparation must not reach the daemon"),
            |_| panic!("cancelled preparation must not reach the clipboard"),
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(extracted, cancel_after + 1);
        for plan in &plans {
            assert!(!plan.output.exists());
        }
    }
}

#[test]
fn unpublished_audio_cleanup_uses_the_held_parent_and_checks_file_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut plan = audio_slice_copy_plan(
        Path::new("source.wav"),
        0.0,
        0.5,
        ChannelCopyMode::Native,
        None,
    );
    plan.output = root.path().join("original/slice.wav");
    let artifact = fake_slice_artifact(&plan, &AtomicBool::new(false)).unwrap();
    let moved = root.path().join("moved");
    std::fs::rename(plan.output.parent().unwrap(), &moved).unwrap();
    std::fs::create_dir(plan.output.parent().unwrap()).unwrap();
    std::fs::write(&plan.output, b"replacement directory").unwrap();
    drop(artifact);
    assert!(!moved.join("slice.wav").exists());
    assert_eq!(
        std::fs::read(&plan.output).unwrap(),
        b"replacement directory"
    );

    plan.output = root.path().join("original/another.wav");
    let artifact = fake_slice_artifact(&plan, &AtomicBool::new(false)).unwrap();
    std::fs::rename(&plan.output, root.path().join("relocated.wav")).unwrap();
    std::fs::write(&plan.output, b"replacement file").unwrap();
    drop(artifact);
    assert_eq!(std::fs::read(&plan.output).unwrap(), b"replacement file");
    assert!(root.path().join("relocated.wav").exists());
}
