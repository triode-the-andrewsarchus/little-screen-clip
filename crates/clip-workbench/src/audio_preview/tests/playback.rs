use super::*;

#[test]
fn test_audio_preview_state_creation_and_seek() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    assert!(state.duration_sec >= 4.0);
    assert!(state.metadata.is_some());

    // Test seek
    state.seek_to(2.0);
    assert_eq!(state.current_time_sec, 2.0);

    // Test selection
    state.selection = Some((1.0, 3.0));
    assert_eq!(state.selection, Some((1.0, 3.0)));

    // Test segment copy extraction (native stereo)
    state.copy_selected_segment(1.0, ChannelCopyMode::Native);
    wait_for_slice_copy(&mut state);
    assert!(state.status_toast.is_some());
    let (toast, _) = state.status_toast.clone().unwrap();
    assert!(toast.contains("slice"));
}

#[test]
fn test_zoom_and_pan_calculations() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    let init_dur = state.view_duration_sec;

    // Zoom In
    state.zoom(0.5, 2.0);
    assert!(state.view_duration_sec < init_dur);
    assert!(state.view_start_sec >= 0.0);

    // Pan right
    let s1 = state.view_start_sec;
    state.pan(0.5);
    assert!(state.view_start_sec >= s1);

    // Fit to window
    state.fit_to_window();
    assert_eq!(state.view_start_sec, 0.0);
    assert_eq!(state.view_duration_sec, state.duration_sec);
}

#[test]
fn test_channel_slicing_modes() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    state.selection = Some((0.5, 1.5));

    // Test Left Only
    state.copy_selected_segment(1.0, ChannelCopyMode::LeftOnly);
    wait_for_slice_copy(&mut state);
    assert!(state.status_toast.is_some());
    let (toast, _) = state.status_toast.as_ref().unwrap();
    assert!(toast.contains("LEFT (mono)"));

    // Test Right Only
    state.copy_selected_segment(2.0, ChannelCopyMode::RightOnly);
    wait_for_slice_copy(&mut state);
    assert!(state.status_toast.is_some());
    let (toast, _) = state.status_toast.as_ref().unwrap();
    assert!(toast.contains("RIGHT (mono)"));
}

#[test]
fn test_small_sub_50ms_slice_selection() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    state.selection = Some((1.000, 1.020));
    state.copy_selected_segment(1.0, ChannelCopyMode::Native);
    wait_for_slice_copy(&mut state);
    assert!(state.status_toast.is_some());
    let (toast, _) = state.status_toast.clone().unwrap();
    assert!(toast.contains("slice"));
    assert!(toast.contains("0.020s"));
}

#[test]
fn test_mono_and_multichannel_handling() {
    use std::io::Write;
    let temp_dir = std::env::temp_dir();

    // 1. Generate Mono WAV
    let mono_path = temp_dir.join(format!("test_mono_{}.wav", uuid::Uuid::new_v4()));
    {
        let mut f = std::fs::File::create(&mono_path).unwrap();
        let sample_rate = 44100u32;
        let num_samples = 4410usize; // 0.1s
        let num_channels = 1u16;
        let bits = 16u16;
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;
        f.write_all(b"RIFF").unwrap();
        f.write_all(&riff_len.to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&num_channels.to_le_bytes()).unwrap();
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&bits.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data_len.to_le_bytes()).unwrap();
        for _ in 0..num_samples {
            f.write_all(&1000i16.to_le_bytes()).unwrap();
        }
    }

    let mono_analysis = compute_full_audio_analysis(&mono_path, 50).expect("analyze mono");
    assert_eq!(mono_analysis.channels, 1);
    assert!(mono_analysis.ch2_samples.is_none());
    assert!(!mono_analysis.ch1_samples.is_empty());
    let _ = std::fs::remove_file(&mono_path);

    // 2. Generate 6-Channel (5.1 Surround) WAV
    let surround_path = temp_dir.join(format!("test_surround_{}.wav", uuid::Uuid::new_v4()));
    {
        let mut f = std::fs::File::create(&surround_path).unwrap();
        let sample_rate = 44100u32;
        let num_samples = 4410usize; // 0.1s
        let num_channels = 6u16;
        let bits = 16u16;
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;
        f.write_all(b"RIFF").unwrap();
        f.write_all(&riff_len.to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&num_channels.to_le_bytes()).unwrap();
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&(sample_rate * num_channels as u32 * 2).to_le_bytes())
            .unwrap();
        f.write_all(&(num_channels * 2).to_le_bytes()).unwrap();
        f.write_all(&bits.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data_len.to_le_bytes()).unwrap();
        for _ in 0..num_samples {
            for _ch in 0..6 {
                f.write_all(&1200i16.to_le_bytes()).unwrap();
            }
        }
    }

    let surround_analysis = compute_full_audio_analysis(&surround_path, 50).expect("analyze 5.1");
    assert_eq!(surround_analysis.channels, 6);
    assert!(surround_analysis.channels > 2); // Triggers warning badge in UI
    assert!(surround_analysis.ch2_samples.is_some()); // Displays L and R
    let _ = std::fs::remove_file(&surround_path);
}

#[test]
fn test_multi_region_mode_and_removal() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    assert_eq!(state.selection_mode, SelectionMode::Single);
    assert!(state.multi_selections.is_empty());

    // Single selection default
    state.selection = Some((1.0, 2.0));
    assert_eq!(state.active_selection_range(), Some((1.0, 2.0)));
    assert!(state.is_time_selected(1.5));
    assert!(!state.is_time_selected(2.5));

    // Switch to Multi-Region Mode
    state.set_selection_mode(SelectionMode::Multi);
    assert_eq!(state.selection_mode, SelectionMode::Multi);
    // Preserves the existing selection into multi_selections
    assert_eq!(state.multi_selections.len(), 1);
    assert_eq!(state.multi_selections[0], (1.0, 2.0));

    // Add second region
    state.multi_selections.push((2.5, 3.5));
    assert_eq!(state.multi_selections.len(), 2);
    assert!(state.is_time_selected(1.5));
    assert!(state.is_time_selected(3.0));
    assert!(!state.is_time_selected(2.2));

    // Test multi-region batch extraction
    state.copy_all_multi_segments(1.0, ChannelCopyMode::Native);
    wait_for_slice_copy(&mut state);
    assert!(state.status_toast.is_some());
    let (toast, _) = state.status_toast.as_ref().unwrap();
    assert!(toast.contains("2/2 audio slices") || toast.contains("2 extracted slice(s)"));

    // Remove a region
    state.multi_selections.remove(0);
    assert_eq!(state.multi_selections.len(), 1);
    assert_eq!(state.multi_selections[0], (2.5, 3.5));
    assert!(!state.is_time_selected(1.5));
    assert!(state.is_time_selected(3.0));

    // Switch back to Single Mode: keeps first multi-region as single selection
    state.set_selection_mode(SelectionMode::Single);
    assert_eq!(state.selection_mode, SelectionMode::Single);
    assert_eq!(state.selection, Some((2.5, 3.5)));
}

#[test]
fn test_zero_crossing_snapping_integration() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    assert!(!state.snap_zero_crossings);

    state.selection = Some((1.0123, 2.4567));
    // Toggle snap mode
    state.snap_zero_crossings = true;
    assert!(state.snap_zero_crossings);

    let analysis = compute_full_audio_analysis(&p, 50).expect("analysis");
    state.analysis = Some(Arc::new(analysis));

    let snapped_s = state.snap_time(1.0123, false);
    let snapped_e = state.snap_time(2.4567, false);
    // Snapped values should be valid timestamps
    assert!(snapped_s >= 0.0 && snapped_s <= state.duration_sec);
    assert!(snapped_e >= 0.0 && snapped_e <= state.duration_sec);

    // Trigger instant snap on active selection
    state.snap_active_selection_to_zero_crossings(1.0);
    assert!(state.status_toast.is_some());
    let (toast, _) = state.status_toast.as_ref().unwrap();
    assert!(toast.contains("Snapped selection to 0-crossings"));
}

#[test]
fn test_transient_toggle_and_snapping_integration() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    assert!(!state.show_transients);

    // Toggle transients ON
    state.show_transients = true;
    assert!(state.show_transients);

    let mut analysis = compute_full_audio_analysis(&p, 50).expect("analysis");
    // Add a synthetic transient if empty to verify snapping
    if analysis.transients.is_empty() {
        analysis.transients.push(1.234);
    }
    state.analysis = Some(Arc::new(analysis.clone()));

    let target_t = analysis.transients[0];
    // Test snapping near the transient (within ~5ms)
    let test_t = target_t + 0.005;
    let snapped = state.snap_time(test_t, false);
    assert_eq!(snapped, target_t);

    // With both modes enabled, choose the transient first and then refine it
    // to the nearest zero crossing rather than snapping around the pointer.
    state.snap_zero_crossings = true;
    let expected_crossing = analysis.snap_to_zero_crossing(target_t, 500);
    assert_eq!(state.snap_time(test_t, false), expected_crossing);

    // Holding Shift or Ctrl bypasses snapping even when both modes are ON
    let bypassed = state.snap_time(test_t, true);
    assert_eq!(bypassed, test_t);

    // Toggle transients OFF -> should not snap to transient
    state.show_transients = false;
    let zero_crossing_only = state.snap_time(test_t, false);
    assert_eq!(
        zero_crossing_only,
        analysis.snap_to_zero_crossing(test_t, 500),
    );
}

#[test]
fn test_middle_mouse_pan_calculation() {
    let (_dir, p) = generate_fixture_wav("fixture", 5.0);
    let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
    state.view_duration_sec = 2.0;
    state.view_start_sec = 1.0;
    state.selection = Some((1.2, 1.8));

    // Simulate middle mouse pan drag:
    // start_x = 500.0, current_x = 400.0 (mouse moved 100px left, meaning viewing future audio)
    let track_width = 1000.0;
    let start_x = 500.0;
    let cur_x = 400.0;
    let initial_start_sec = 1.0;

    let dx = cur_x - start_x; // -100.0
    let dt = (dx / track_width) * state.view_duration_sec; // (-100/1000) * 2.0 = -0.2
    let max_start = (state.duration_sec - state.view_duration_sec).max(0.0);
    state.view_start_sec = (initial_start_sec - dt).clamp(0.0, max_start); // 1.0 - (-0.2) = 1.2

    assert!((state.view_start_sec - 1.2).abs() < 0.001);
    // Selection remains intact and unchanged
    assert_eq!(state.selection, Some((1.2, 1.8)));
}

#[test]
fn test_sample_timestamp_and_duration_formatting() {
    // Sub-minute timestamps & durations
    assert_eq!(format_sample_timestamp(0.0), "00m00s000ms");
    assert_eq!(format_sample_timestamp(4.5), "00m04s500ms");
    assert_eq!(format_sample_timestamp(12.345), "00m12s345ms");
    assert_eq!(format_sample_duration(0.05), "00s050ms");
    assert_eq!(format_sample_duration(1.25), "01s250ms");

    // Over 1 minute
    assert_eq!(format_sample_timestamp(75.25), "01m15s250ms");
    assert_eq!(format_sample_duration(65.1), "01m05s100ms");

    // Over 1 hour
    assert_eq!(format_sample_timestamp(3665.2), "01h01m05s200ms");
    assert_eq!(format_sample_duration(3605.0), "01h00m05s000ms");

    // Filename formatting: original_name + timestamp + duration
    let filename = format_sample_filename("MyTrack", 4.5, 1.25, "", "wav");
    assert_eq!(filename, "MyTrack_00m04s500ms_01s250ms.wav");

    let mono_left_name = format_sample_filename("DrumBeat", 75.0, 2.0, "_left_mono", "wav");
    assert_eq!(
        mono_left_name,
        "DrumBeat_01m15s000ms_02s000ms_left_mono.wav"
    );
}

#[test]
fn test_save_to_file_single_and_multi() {
    let (_dir, p) = generate_fixture_wav("test_track", 5.0);
    let source_analysis = compute_full_audio_analysis(&p, 100).expect("analyze source fixture");
    let mut state = AudioPreviewState::new(p.clone(), "test_track.wav".to_string());
    let temp_dest =
        std::env::temp_dir().join(format!("little_clip_save_test_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&temp_dest);
    state.save_destination_dir = temp_dest.to_string_lossy().to_string();
    state.open_folder_after_save = false; // don't open file manager in test

    // 1. Single Selection Save
    state.set_selection_mode(SelectionMode::Single);
    state.selection = Some((1.0, 2.5)); // start 1.0s, duration 1.5s
    state.open_save_dialog();
    assert!(state.save_dialog_open);
    assert_eq!(state.save_filenames.len(), 1);
    assert_eq!(
        state.save_filenames[0],
        "test_track_00m01s000ms_01s500ms.wav"
    );

    state.execute_save(1.0).unwrap();
    assert!(
        state.execute_save(1.0).is_err(),
        "duplicate saves are rejected"
    );
    wait_for_audio_save(&mut state);
    assert!(
        state
            .status_toast
            .as_ref()
            .unwrap()
            .0
            .contains("Saved 1 file(s)")
    );
    let saved_single = temp_dest.join("test_track_00m01s000ms_01s500ms.wav");
    assert!(saved_single.exists());
    assert!(std::fs::metadata(&saved_single).unwrap().len() > 0);
    let single_analysis =
        compute_full_audio_analysis(&saved_single, 100).expect("decode saved single selection");
    assert_eq!(single_analysis.channels, 2);
    assert_eq!(single_analysis.sample_rate, 44100);
    assert!((single_analysis.duration_ms as i64 - 1500).abs() <= 1);
    assert!((single_analysis.total_frames as isize - 66_150).abs() <= 44);
    for (saved, original) in single_analysis.ch1_samples[..100]
        .iter()
        .zip(&source_analysis.ch1_samples[44_100..44_200])
    {
        assert!((saved - original).abs() <= 0.0001);
    }

    // 2. Multi-Selection Save
    state.set_selection_mode(SelectionMode::Multi);
    state.multi_selections = vec![(0.5, 1.2), (2.0, 2.8)];
    state.open_save_dialog();
    assert!(state.save_dialog_open);
    assert_eq!(state.save_filenames.len(), 2);
    assert_eq!(
        state.save_filenames[0],
        "test_track_00m00s500ms_00s700ms.wav"
    );
    assert_eq!(
        state.save_filenames[1],
        "test_track_00m02s000ms_00s800ms.wav"
    );

    state.execute_save(2.0).unwrap();
    wait_for_audio_save(&mut state);
    assert!(
        state
            .status_toast
            .as_ref()
            .unwrap()
            .0
            .contains("Saved 2 file(s)")
    );

    let saved_multi_1 = temp_dest.join("test_track_00m00s500ms_00s700ms.wav");
    let saved_multi_2 = temp_dest.join("test_track_00m02s000ms_00s800ms.wav");
    assert!(saved_multi_1.exists());
    assert!(saved_multi_2.exists());
    assert!(std::fs::metadata(&saved_multi_1).unwrap().len() > 0);
    assert!(std::fs::metadata(&saved_multi_2).unwrap().len() > 0);

    // Cleanup test files
    let _ = std::fs::remove_file(saved_single);
    let _ = std::fs::remove_file(saved_multi_1);
    let _ = std::fs::remove_file(saved_multi_2);
    let _ = std::fs::remove_dir(temp_dest);
}

#[test]
fn audio_save_reports_destination_directory_creation_failure() {
    let (_source_dir, source) = generate_fixture_wav("save_error", 1.0);
    let blocker =
        std::env::temp_dir().join(format!("little_clip_save_blocker_{}", uuid::Uuid::new_v4()));
    std::fs::write(&blocker, b"file blocks directory creation").unwrap();
    let plan = AudioSavePlan {
        destination: blocker.join("child"),
        source,
        regions: vec![AudioSaveRegion {
            start: 0.0,
            end: 0.5,
            filename: "slice.wav".to_owned(),
            filter: Some("volume=6dB,lowpass=f=3200.000:p=2".to_owned()),
        }],
        open_folder: false,
    };
    let error = run_audio_save(plan).unwrap_err();
    assert!(error.contains("Cannot create destination directory"));
    let _ = std::fs::remove_file(blocker);
}

#[test]
fn audio_save_reports_ffmpeg_process_failure_with_filename() {
    let destination = tempfile::tempdir().unwrap();
    let result = run_audio_save(AudioSavePlan {
        destination: destination.path().to_owned(),
        source: destination.path().join("missing-source.wav"),
        regions: vec![AudioSaveRegion {
            start: 0.0,
            end: 0.5,
            filename: "failed-slice.wav".to_owned(),
            filter: None,
        }],
        open_folder: false,
    });
    let error = result.unwrap_err();
    assert!(error.contains("Failed to save audio file(s)"));
    assert!(error.contains("failed-slice.wav"));
}

#[test]
fn test_slice_preview_start_to_stop() {
    let (_dir, p) = generate_fixture_wav("test", 5.0);
    let mut state = AudioPreviewState::new(p, "test.wav".to_string());
    state.duration_sec = 10.0;

    assert_eq!(state.preview_stop_time, None);

    // 1. Play slice preview from 1.5s to 3.2s
    state.play_slice_preview(1.5, 3.2);
    assert_eq!(state.preview_stop_time, Some(3.2));
    assert!((state.current_time_sec - 1.5).abs() < 1e-4);

    // 2. Pause clears preview stop time
    state.pause();
    assert_eq!(state.preview_stop_time, None);

    // 3. Play slice preview with inverted bounds (4.0s down to 2.0s)
    state.play_slice_preview(4.0, 2.0);
    assert_eq!(state.preview_stop_time, Some(4.0));
    assert!((state.current_time_sec - 2.0).abs() < 1e-4);

    // 4. Seek clears preview stop time
    state.seek_to(0.5);
    assert_eq!(state.preview_stop_time, None);
    assert!((state.current_time_sec - 0.5).abs() < 1e-4);
}

#[test]
fn test_fine_grained_consistent_zoom() {
    let (_dir, p) = generate_fixture_wav("test", 5.0);
    let mut state = AudioPreviewState::new(p, "test.wav".to_string());
    state.duration_sec = 20.0;
    state.view_start_sec = 2.0;
    state.view_duration_sec = 10.0;

    let initial_dur = state.view_duration_sec;

    // 1 Notch Zoom In (delta = +50.0 pt -> steps = +1.0)
    let step_ratio: f32 = 1.055;
    let factor_in = step_ratio.powf(-1.0); // ~0.9478
    state.zoom(factor_in, 5.0);

    // View duration should shrink by ~5.2% (fine-grained)
    assert!(state.view_duration_sec < initial_dur);
    assert!((state.view_duration_sec - (initial_dur * factor_in)).abs() < 1e-4);

    // 1 Notch Zoom Out (delta = -50.0 pt -> steps = -1.0)
    let factor_out = step_ratio.powf(1.0); // 1.055
    state.zoom(factor_out, 5.0);

    // View duration must return back to initial duration (perfect mathematical symmetry)
    assert!((state.view_duration_sec - initial_dur).abs() < 1e-4);

    // Ultra-fine mode (Ctrl/Alt held): ~2.4% zoom
    let ultra_fine_ratio: f32 = 1.025;
    let ultra_fine_factor = ultra_fine_ratio.powf(-1.0);
    state.zoom(ultra_fine_factor, 5.0);
    assert!((state.view_duration_sec - (initial_dur * ultra_fine_factor)).abs() < 1e-4);
}

#[test]
fn test_audio_viewport_zoom_pan_and_fit() {
    let mut vp = AudioViewport::new(10.0);
    assert_eq!(vp.start_sec, 0.0);
    assert_eq!(vp.duration_sec, 10.0);

    // Zoom in by factor 0.5 centered at 4.0 seconds
    vp.zoom(0.5, 4.0, 10.0, 44100.0);
    assert!((vp.duration_sec - 5.0).abs() < 1e-4);
    assert!(vp.start_sec >= 0.0 && vp.start_sec + vp.duration_sec <= 10.0);

    // Pan right by 2 seconds
    let prev_start = vp.start_sec;
    vp.pan(2.0, 10.0);
    assert!((vp.start_sec - (prev_start + 2.0)).abs() < 1e-4);

    // Pan past right boundary clamps to 10.0 - duration
    vp.pan(100.0, 10.0);
    assert!((vp.start_sec - (10.0 - vp.duration_sec)).abs() < 1e-4);

    // Pan negative clamps to 0.0
    vp.pan(-100.0, 10.0);
    assert_eq!(vp.start_sec, 0.0);

    // Reset to full window
    vp.fit_to_window(10.0);
    assert_eq!(vp.start_sec, 0.0);
    assert_eq!(vp.duration_sec, 10.0);
}

#[test]
fn test_audio_selection_model_contains_and_normalize() {
    // Normal range
    assert!(AudioSelectionModel::contains((2.0, 5.0), 3.0));
    assert!(AudioSelectionModel::contains((2.0, 5.0), 2.0));
    assert!(AudioSelectionModel::contains((2.0, 5.0), 5.0));
    assert!(!AudioSelectionModel::contains((2.0, 5.0), 1.9));
    assert!(!AudioSelectionModel::contains((2.0, 5.0), 5.1));

    // Inverted range (dragged backwards)
    assert!(AudioSelectionModel::contains((5.0, 2.0), 3.5));
    assert!(!AudioSelectionModel::contains((5.0, 2.0), 1.0));

    // Normalization
    assert_eq!(AudioSelectionModel::normalize((5.0, 2.0)), (2.0, 5.0));
    assert_eq!(AudioSelectionModel::normalize((2.0, 5.0)), (2.0, 5.0));

    // Clamp to duration
    assert_eq!(
        AudioSelectionModel::clamp_to_duration((-1.0, 15.0), 10.0),
        (0.0, 10.0)
    );
}

#[test]
fn test_slice_preview_looping_and_loop_seek() {
    let (_dir, p) = generate_fixture_wav("test", 5.0);
    let mut state = AudioPreviewState::new(p, "test.wav".to_string());
    state.duration_sec = 10.0;

    assert_eq!(state.preview_stop_time, None);
    assert_eq!(state.preview_slice_range, None);
    assert!(!state.preview_is_looping);

    // 1. Play slice preview with looping enabled
    state.play_slice_preview_loop(1.2, 3.8);
    assert_eq!(state.preview_stop_time, Some(3.8));
    assert_eq!(state.preview_slice_range, Some((1.2, 3.8)));
    assert!(state.preview_is_looping);
    assert!((state.current_time_sec - 1.2).abs() < 1e-4);

    // 2. Loop seek preserves preview bounds and looping flag
    state.loop_seek_to(1.2);
    assert_eq!(state.preview_stop_time, Some(3.8));
    assert_eq!(state.preview_slice_range, Some((1.2, 3.8)));
    assert!(state.preview_is_looping);
    assert!((state.current_time_sec - 1.2).abs() < 1e-4);

    // 3. Normal seek clears preview state
    state.seek_to(0.5);
    assert_eq!(state.preview_stop_time, None);
    assert_eq!(state.preview_slice_range, None);
    assert!(!state.preview_is_looping);
    assert!((state.current_time_sec - 0.5).abs() < 1e-4);

    // 4. Play slice preview non-looping
    state.play_slice_preview(2.0, 4.0);
    assert_eq!(state.preview_stop_time, Some(4.0));
    assert_eq!(state.preview_slice_range, Some((2.0, 4.0)));
    assert!(!state.preview_is_looping);

    // 5. Pause clears preview state
    state.pause();
    assert_eq!(state.preview_stop_time, None);
    assert_eq!(state.preview_slice_range, None);
    assert!(!state.preview_is_looping);
}

#[test]
fn test_right_click_hold_state_and_timeout() {
    assert_eq!(RIGHT_CLICK_HOLD_TIMEOUT, Duration::from_millis(300));

    let now = Instant::now();
    let hold = RightClickHoldState {
        start_instant: now,
        target_range: (1.0, 3.0),
        slice_index: Some(2),
        started_looping: false,
    };

    assert_eq!(hold.target_range, (1.0, 3.0));
    assert_eq!(hold.slice_index, Some(2));
    assert!(!hold.started_looping);
}
