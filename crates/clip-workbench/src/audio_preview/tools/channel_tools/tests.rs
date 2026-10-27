use super::*;
use crate::audio_preview::session_edit::render_session_edit;
use crate::audio_preview::tests::{wait_for_session_edit, write_stereo_pcm_fixture};

fn stereo_buffer(samples: Vec<f32>) -> rodio::buffer::SamplesBuffer {
    rodio::buffer::SamplesBuffer::new(
        std::num::NonZeroU16::new(2).unwrap(),
        std::num::NonZeroU32::new(8000).unwrap(),
        samples,
    )
}

#[test]
fn monitor_routes_complete_frames_and_switches_without_changing_time() {
    let mode = Arc::new(AtomicU64::new(1));
    let mut source = ChannelMonitorSource::new(
        stereo_buffer(vec![0.75, 0.5, 0.25, -0.5, 0.25, -0.5, 0.25, -0.5]),
        mode.clone(),
    );
    assert_eq!(source.next(), Some(1.25));
    mode.store(2, Ordering::Release);
    assert_eq!(source.next(), Some(1.25)); // The right sample of the same frame.
    assert_eq!(source.next(), Some(0.75));
    assert_eq!(source.next(), Some(0.75));
    mode.store(3, Ordering::Release);
    assert_eq!(source.next(), Some(-0.75));
    assert_eq!(source.next(), Some(-0.75));
    mode.store(0, Ordering::Release);
    assert_eq!(source.collect::<Vec<_>>(), vec![0.25, -0.5]);
}

#[test]
fn monitor_seek_discards_pending_frame_and_mono_passes_through() {
    let mode = Arc::new(AtomicU64::new(2));
    let mut source =
        ChannelMonitorSource::new(stereo_buffer(vec![0.25, -0.5, 0.125, 0.5]), mode.clone());
    assert_eq!(source.next(), Some(0.75));
    source.try_seek(Duration::ZERO).unwrap();
    assert_eq!(source.collect::<Vec<_>>(), vec![0.75, 0.75, -0.375, -0.375]);
    let mono = rodio::buffer::SamplesBuffer::new(
        std::num::NonZeroU16::new(1).unwrap(),
        std::num::NonZeroU32::new(8000).unwrap(),
        vec![0.25, -0.5],
    );
    assert_eq!(
        ChannelMonitorSource::new(mono, mode).collect::<Vec<_>>(),
        vec![0.25, -0.5]
    );
}

#[test]
fn channel_edits_preserve_stereo_and_affect_only_selected_samples() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.wav");
    write_stereo_pcm_fixture(&path, &[24576; 80], &[16384; 80], 8000);
    for operation in ChannelEdit::ALL {
        for regions in [
            vec![],
            vec![(0.002, 0.004)],
            vec![(0.004, 0.002), (0.006, 0.008), (0.003, 0.004)],
        ] {
            let output = directory.path().join("result.wav");
            let status = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-i"])
                .arg(&path)
                .args(["-af", &operation.filter(&regions), "-c:a", "pcm_f32le"])
                .arg(&output)
                .status()
                .unwrap();
            assert!(status.success());
            let decoder = Decoder::new(BufReader::new(File::open(&output).unwrap())).unwrap();
            assert_eq!(decoder.channels().get(), 2);
            assert_eq!(decoder.sample_rate().get(), 8000);
            let samples = decoder.collect::<Vec<_>>();
            assert_eq!(samples.len(), 160);
            for (frame, pair) in samples.as_chunks::<2>().0.iter().enumerate() {
                let selected = regions.is_empty()
                    || (16..32).contains(&frame)
                    || (regions.len() > 1 && (48..64).contains(&frame));
                let expected = if !selected {
                    [0.75, 0.5]
                } else {
                    match operation {
                        ChannelEdit::Sum => [1.25, 1.25],
                        ChannelEdit::Swap => [0.5, 0.75],
                        ChannelEdit::InvertLeft => [-0.75, 0.5],
                        ChannelEdit::InvertRight => [0.75, -0.5],
                        ChannelEdit::LeftToRight => [0.75, 0.75],
                        ChannelEdit::RightToLeft => [0.5, 0.5],
                    }
                };
                assert_eq!(
                    *pair, expected,
                    "{operation:?}, frame {frame}, regions {regions:?}"
                );
            }
        }
    }
}

#[test]
fn committed_channel_edit_is_undoable_and_stale_completion_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("input.wav");
    write_stereo_pcm_fixture(&original, &[8192; 800], &[-16384; 800], 8000);
    let mut state = AudioPreviewState::new(original.clone(), "input".into());
    state.channel_tools.entire_track = true;
    state.start_channel_edit(ChannelEdit::Swap);
    wait_for_session_edit(&mut state);
    let edited = state.path.clone();
    assert_ne!(edited, original);
    state.undo();
    assert_eq!(state.path, original);
    state.redo();
    assert_eq!(state.path, edited);
    state.start_channel_edit(ChannelEdit::InvertRight);
    state.track_gain_db = -3.0;
    wait_for_session_edit(&mut state);
    assert_eq!(state.path, edited);
    assert!(state.status_toast.as_ref().unwrap().0.contains("discarded"));
    let _ = std::fs::remove_file(edited);
}

#[test]
fn channel_tab_shortcut_requires_exactly_two_channels_and_monitor_is_tool_local() {
    let mut state =
        AudioPreviewState::new(PathBuf::from("/missing-channel-test.wav"), "test".into());
    let ctx = egui::Context::default();
    for channels in [1, 4, 2] {
        state.metadata = Some(AudioMetadata {
            channels,
            sample_rate: 8000,
            duration_ms: 10,
            format: "WAV".into(),
        });
        state.active_mode = AudioToolMode::Inspect;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::C,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                state.handle_studio_shortcuts(ctx, &mut false);
            },
        );
        assert_eq!(
            state.active_mode == AudioToolMode::ChannelTools,
            channels == 2
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::C,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| {},
        );
    }
    state.channel_tools.solo = 2;
    state.channel_tools.reverse_difference = true;
    state.sync_channel_monitor();
    assert_eq!(state.channel_tools.monitor.load(Ordering::Acquire), 3);
    state.active_mode = AudioToolMode::Inspect;
    state.sync_channel_monitor();
    assert_eq!(state.channel_tools.monitor.load(Ordering::Acquire), 0);
}

#[test]
fn derived_waveforms_use_sample_sums_and_differences_and_reject_old_revisions() {
    let mut state = AudioPreviewState::new(PathBuf::from("/missing-derived.wav"), "test".into());
    state.cancel_session_analysis();
    state.analysis = Some(Arc::new(FullAudioAnalysis {
        channels: 2,
        sample_rate: 8000,
        total_frames: 4,
        duration_ms: 1,
        ch1_samples: vec![0.75, -0.5, 0.25, -0.25].into(),
        ch2_samples: Some(vec![0.5, 0.5, -0.25, -0.25].into()),
        ..Default::default()
    }));
    let ctx = egui::Context::default();
    state.poll_derived_signals(&ctx);
    let mut current = state.analysis.as_ref().unwrap().as_ref().clone();
    current.ch1_samples = vec![0.5, 0.25, 0.0, 0.0].into();
    state.edited_waveform = Some(Arc::new(current));
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.channel_tools.derived.is_none() || state.channel_tools.derived_job.is_some() {
        assert!(Instant::now() < deadline);
        state.poll_derived_signals(&ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let derived = state.channel_tools.derived.as_ref().unwrap();
    assert_eq!(&*derived.ch1_samples, &[1.0, 0.75, -0.25, -0.25]);
    assert_eq!(
        &**derived.ch2_samples.as_ref().unwrap(),
        &[0.0, -0.25, 0.25, 0.25]
    );
    let (min, max, _) = derived.query_peaks(0, 0, 4);
    assert_eq!((min, max), (-0.25, 1.0));
    state.show_transients = true;
    state.session_transients = Some(vec![0.00025].into());
    state.view_duration_sec = 0.001;
    state.snap_zero_crossings = false;
    assert_eq!(state.snap_time(0.000251, false), 0.00025);
    assert_eq!(state.snap_time(0.000251, true), 0.000251);
    state.show_transients = false;
    state.snap_zero_crossings = true;
    let expected = state
        .analysis
        .as_ref()
        .unwrap()
        .snap_to_zero_crossing(0.000251, 500);
    assert_eq!(state.snap_time(0.000251, false), expected);
}

fn press_number(
    state: &mut AudioPreviewState,
    ctx: &egui::Context,
    key: egui::Key,
    modifiers: egui::Modifiers,
) {
    for pressed in [true, false] {
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                if pressed {
                    state.handle_studio_shortcuts(ctx, &mut false);
                }
            },
        );
    }
}

#[test]
fn number_edit_shortcuts_preserve_visible_analysis_until_ready_and_apply_each_operation() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("numbers.wav");
    write_stereo_pcm_fixture(&original, &[8192; 800], &[-16384; 800], 8000);
    let source =
        Arc::new(little_clip_core::audio::compute_full_audio_analysis(&original, 40).unwrap());
    for (key, expected) in [
        (egui::Key::Num1, [-0.25, -0.25]),
        (egui::Key::Num2, [-0.5, 0.25]),
        (egui::Key::Num3, [-0.25, -0.5]),
        (egui::Key::Num4, [0.25, 0.5]),
        (egui::Key::Num5, [0.25, 0.25]),
        (egui::Key::Num6, [-0.5, -0.5]),
    ] {
        let mut state = AudioPreviewState::new(original.clone(), "numbers".into());
        state.cancel_session_analysis();
        state.active_mode = AudioToolMode::ChannelTools;
        state.analysis = Some(source.clone());
        state.is_loading_analysis = false;
        state.current_time_sec = 0.02;
        state.channel_tools.entire_track = true;
        let ctx = egui::Context::default();
        press_number(&mut state, &ctx, key, egui::Modifiers::NONE);
        assert!(state.session_edit_job.is_some());
        assert!(Arc::ptr_eq(state.analysis.as_ref().unwrap(), &source));
        assert!(!state.is_loading_analysis);
        wait_for_session_edit(&mut state);
        assert_ne!(state.path, original);
        assert!(!state.is_loading_analysis);
        assert_eq!(state.current_time_sec, 0.02);
        assert!(state.channel_tools.derived.is_some());
        let output = state.analysis.as_ref().unwrap().clone();
        assert_eq!(
            [
                output.ch1_samples[0],
                output.ch2_samples.as_ref().unwrap()[0]
            ],
            expected
        );
        let revision = state.path.clone();
        state.undo();
        assert_eq!(state.path, original);
        assert!(
            !state.is_loading_analysis,
            "undo must retain the current plots during refresh"
        );
        assert!(Arc::ptr_eq(state.analysis.as_ref().unwrap(), &output));
        let _ = std::fs::remove_file(revision);
    }
}

#[test]
fn monitoring_numbers_dim_only_other_signals_and_respect_modifiers_and_dialogs() {
    let mut state = AudioPreviewState::new(PathBuf::from("/missing-monitor.wav"), "test".into());
    state.cancel_session_analysis();
    state.active_mode = AudioToolMode::ChannelTools;
    let ctx = egui::Context::default();
    for (key, mode, reversed) in [
        (egui::Key::Num7, 1, false),
        (egui::Key::Num8, 2, false),
        (egui::Key::Num9, 2, true),
        (egui::Key::Num0, 0, true),
    ] {
        press_number(&mut state, &ctx, key, egui::Modifiers::NONE);
        assert_eq!(state.channel_tools.solo, mode);
        assert_eq!(state.channel_tools.reverse_difference, reversed);
        for row in 0..4 {
            let dim = canvas::channel_row_dimmed(mode, row);
            assert_eq!(dim, mode != 0 && row != if mode == 1 { 2 } else { 3 });
        }
    }
    state.save_dialog_open = true;
    press_number(&mut state, &ctx, egui::Key::Num7, egui::Modifiers::NONE);
    assert_eq!(state.channel_tools.solo, 0);
    state.save_dialog_open = false;
    press_number(&mut state, &ctx, egui::Key::Num7, egui::Modifiers::CTRL);
    assert_eq!(state.channel_tools.solo, 0);
    state.active_mode = AudioToolMode::Inspect;
    press_number(&mut state, &ctx, egui::Key::Num7, egui::Modifiers::NONE);
    assert_eq!(state.channel_tools.solo, 0);
}

#[test]
fn stereo_number_shortcut_keeps_playhead_and_multi_region_number_does_not_preview_a_slice() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("keyboard.wav");
    write_stereo_pcm_fixture(&path, &[8192; 8000], &[-16384; 8000], 8000);
    let source = Arc::new(little_clip_core::audio::compute_full_audio_analysis(&path, 40).unwrap());
    let mut state = AudioPreviewState::new(path, "keyboard".into());
    state.cancel_session_analysis();
    state.active_mode = AudioToolMode::ChannelTools;
    state.analysis = Some(source);
    state.is_loading_analysis = false;
    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![
        (0.1, 0.2),
        (0.2, 0.3),
        (0.3, 0.4),
        (0.4, 0.5),
        (0.5, 0.6),
        (0.6, 0.7),
        (0.7, 0.8),
    ];
    state.active_multi_index = Some(0);
    state.current_time_sec = 0.35;
    let ctx = egui::Context::default();
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    for (key, monitor) in [(egui::Key::Num7, 1), (egui::Key::Num0, 0)] {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| state.render_ui(ui, ctx, &palette, &mut false));
            },
        );
        assert_eq!(state.channel_tools.solo, monitor);
        assert_eq!(state.current_time_sec, 0.35);
        assert!(
            !state.is_playing,
            "monitor shortcuts must not start numeric slice previews"
        );
        assert_eq!(state.active_multi_index, Some(0));
    }
}

#[test]
fn channel_render_rejects_expired_context_before_opening_the_source() {
    let check = crate::file_source::ReadCheck::new(|_| Err("source context expired".into()));
    let result = render_session_edit(
        Path::new("/missing-channel-source.wav"),
        None,
        &check,
        ChannelEdit::Swap.filter(&[]),
        export::AudioRenderMode::Session,
        &SessionAudioEdits::default(),
        &AtomicBool::new(false),
    );
    assert_eq!(result.unwrap_err(), "source context expired");
}

#[test]
fn channel_render_rejects_a_replaced_observed_input() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.wav");
    let replacement = directory.path().join("replacement.wav");
    write_stereo_pcm_fixture(&input, &[8192; 800], &[16384; 800], 8000);
    let observed = crate::file_source::FileSource::capture(&input).unwrap();
    write_stereo_pcm_fixture(&replacement, &[4096; 800], &[12288; 800], 8000);
    std::fs::rename(replacement, &input).unwrap();
    let result = render_session_edit(
        &input,
        Some(&observed),
        &Default::default(),
        ChannelEdit::Swap.filter(&[]),
        export::AudioRenderMode::Session,
        &SessionAudioEdits::default(),
        &AtomicBool::new(false),
    );
    assert!(result.unwrap_err().contains("source changed"));
}

#[test]
fn derived_channel_input_survives_original_invalidation_and_undo_restores_its_guard() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.wav");
    write_stereo_pcm_fixture(&input, &[8192; 800], &[-16384; 800], 8000);
    let observed = crate::file_source::FileSource::capture(&input).unwrap();
    let valid = Arc::new(AtomicBool::new(true));
    let original_valid = valid.clone();
    let original_check = crate::file_source::ReadCheck::new(move |_| {
        if original_valid.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err("source context expired".into())
        }
    });
    let mut state =
        AudioPreviewState::new(input.clone(), "input".into()).with_file_source(observed.clone());
    state.cancel_session_analysis();
    state.read_check = original_check.clone();
    state.channel_tools.entire_track = true;
    state.start_channel_edit(ChannelEdit::Swap);
    wait_for_session_edit(&mut state);
    let derived = state.path.clone();
    assert_ne!(derived, input, "{:?}", state.status_toast);
    assert_eq!(state.file_source.as_ref().unwrap().path(), derived);
    assert_ne!(state.read_check, original_check);
    assert!(state.has_session_edits());
    valid.store(false, Ordering::Release);
    state.read_check.validate(&AtomicBool::new(false)).unwrap();
    state.file_source.as_ref().unwrap().check().unwrap();
    state.undo();
    assert_eq!(state.path, input);
    assert_eq!(state.file_source, Some(observed));
    assert_eq!(state.read_check, original_check);
    assert!(!state.has_session_edits());
    assert!(state.read_check.validate(&AtomicBool::new(false)).is_err());
    state.redo();
    assert_eq!(state.path, derived);
    assert!(state.has_session_edits());
    state.read_check.validate(&AtomicBool::new(false)).unwrap();
    let deferred_check = state.read_check.clone();
    drop(state);
    assert!(
        derived.exists(),
        "Deferred jobs must retain their input artifact"
    );
    drop(deferred_check);
    let deadline = Instant::now() + Duration::from_secs(5);
    while derived.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !derived.exists(),
        "The final artifact owner must remove its staged file"
    );
}
