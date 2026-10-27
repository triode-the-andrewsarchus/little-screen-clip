use super::*;

#[test]
fn region_gain_reaches_silence_recovers_and_caps_at_100x() {
    assert_eq!(AudioPreviewState::step_gain_db(0.0, 100.0), 40.0);
    assert_eq!(
        AudioPreviewState::step_gain_db(-60.0, -1.0),
        f32::NEG_INFINITY
    );
    assert_eq!(
        AudioPreviewState::step_gain_db(f32::NEG_INFINITY, 1.0),
        -60.0
    );
    assert_eq!(
        AudioPreviewState::gain_filter(f32::NEG_INFINITY, false).as_deref(),
        Some("volume=0")
    );
    assert_eq!(
        AudioPreviewState::gain_filter(40.0, true).as_deref(),
        Some("volume=40dB,asoftclip=type=tanh")
    );
    assert_eq!(
        AudioPreviewState::gain_filter(1.0, false).as_deref(),
        Some("volume=1dB")
    );
}

#[test]
fn normalization_gain_targets_minus_one_dbfs_and_accounts_for_track_gain() {
    let peak = 0.5;
    let region_gain = AudioPreviewState::normalization_gain_db(peak, -3.0).unwrap();
    let result_dbfs = 20.0 * peak.log10() - 3.0 + region_gain;
    assert!((result_dbfs - AudioPreviewState::NORMALIZE_TARGET_DBFS).abs() < 0.000_01);
    assert_eq!(AudioPreviewState::normalization_gain_db(0.0, 0.0), None);
}

#[test]
fn normalization_targets_hovered_region_and_falls_back_to_track() {
    let (_dir, path) = generate_fixture_wav("normalize", 5.0);
    let analysis = compute_full_audio_analysis(&path, 50).expect("analysis");
    let mut state = AudioPreviewState::new(path, "normalize.wav".to_string());
    state.analysis = Some(Arc::new(analysis));
    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
    state.region_gain_db = vec![0.0, 0.0];
    state.track_gain_db = -10.0;

    state.normalize_at_time(Some(3.5), 1.0);
    assert_eq!(state.active_multi_index, Some(1));
    assert_eq!(state.track_gain_db, -10.0);
    assert_eq!(state.region_gain_db[0], 0.0);
    assert_ne!(state.region_gain_db[1], 0.0);
    let peak = state.peak_amplitude_in_range((3.0, 4.0)).unwrap();
    let result_dbfs = 20.0 * peak.log10() + state.track_gain_db + state.region_gain_db[1];
    assert!((result_dbfs - AudioPreviewState::NORMALIZE_TARGET_DBFS).abs() < 0.000_1);
    assert!(!state.active_region_edit().unwrap().soft_clip);

    state.undo();
    state.normalize_at_time(Some(2.5), 2.0);
    assert_ne!(state.track_gain_db, 0.0);
    assert_eq!(state.region_gain_db, vec![0.0, 0.0]);
    let peak = state
        .peak_amplitude_in_range((0.0, state.duration_sec))
        .unwrap();
    let result_dbfs = 20.0 * peak.log10() + state.track_gain_db;
    assert!((result_dbfs - AudioPreviewState::NORMALIZE_TARGET_DBFS).abs() < 0.000_1);
}

#[test]
fn crossfade_scroll_is_symmetric_clamped_and_resettable() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_crossfade__.wav"),
        "test".to_string(),
    );
    state.duration_sec = 10.0;
    state.selection = Some((3.0, 7.0));
    state.adjust_active_region_crossfade(5.0, Instant::now());
    assert!((state.single_crossfade_sec - 0.05).abs() < f32::EPSILON);
    assert_eq!(
        state.active_region_edit().unwrap().effect_bounds(10.0),
        (2.95, 7.05)
    );

    state.adjust_active_region_crossfade(1_000.0, Instant::now());
    assert_eq!(state.single_crossfade_sec, 3.0);
    assert_eq!(
        state.active_region_edit().unwrap().effect_bounds(10.0),
        (0.0, 10.0)
    );
    assert!(state.reset_active_region_crossfade());
    assert_eq!(state.single_crossfade_sec, 0.0);

    state.selection = Some((0.0, 2.0));
    state.adjust_active_region_crossfade(100.0, Instant::now());
    let edit = state.active_region_edit().unwrap();
    assert_eq!(edit.effect_bounds(10.0), (0.0, 3.0));
    assert_eq!(edit.effect_mix(0.0, 10.0), 1.0);
    assert_eq!(edit.effect_mix(2.5, 10.0), 0.5);
    state.single_region_gain_db = -6.0;
    let (graph, output) = state.session_crossfade_filter_complex().unwrap();
    assert!(graph.contains("clip((3.000000-t)/1.000000,0,1)"));
    assert!(graph.contains("amix=inputs=2"));
    assert_eq!(output, "[edit1]");
}

#[test]
fn middle_click_reset_targets_only_the_region_under_pointer() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_middle_reset__.wav"),
        "test".to_string(),
    );
    state.selection = Some((1.0, 2.0));
    state.single_region_gain_db = f32::NEG_INFINITY;
    assert!(!state.focus_region_at_time(3.0));
    assert_eq!(state.single_region_gain_db, f32::NEG_INFINITY);
    assert!(state.focus_region_at_time(1.5));
    assert!(state.reset_active_region_gain());
    assert_eq!(state.single_region_gain_db, 0.0);

    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
    state.region_gain_db = vec![6.0, f32::NEG_INFINITY];
    state.active_multi_index = Some(0);
    assert!(state.focus_region_at_time(3.5));
    assert_eq!(state.active_multi_index, Some(1));
    assert!(state.reset_active_region_gain());
    assert_eq!(state.region_gain_db, vec![6.0, 0.0]);
}

#[test]
fn spectrum_cutoffs_step_logarithmically_and_reset_independently() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_filter_test__.wav"),
        "test".to_string(),
    );
    state.duration_sec = 10.0;
    state.selection = Some((1.0, 3.0));
    state.adjust_active_region_filter(SpectrumFilterKind::LowPass, -4.0, Instant::now());
    state.adjust_active_region_filter(SpectrumFilterKind::HighPass, 4.0, Instant::now());
    assert_eq!(state.single_low_pass_hz, Some(6_000.0));
    assert_eq!(state.single_high_pass_hz, Some(240.0));
    let filter = state.session_edit_filter().unwrap();
    assert!(filter.contains("lowpass=f=6000.000:p=2:enable='between(t,1.000000,3.000000)'"));
    assert!(filter.contains("highpass=f=240.000:p=2:enable='between(t,1.000000,3.000000)'"));
    assert!(state.reset_active_region_filter(SpectrumFilterKind::LowPass));
    assert_eq!(state.single_low_pass_hz, None);
    assert_eq!(state.single_high_pass_hz, Some(240.0));

    state.set_selection_mode(SelectionMode::Multi);
    state.multi_selections.push((4.0, 5.0));
    state.region_low_pass_hz.push(Some(2_000.0));
    state.region_high_pass_hz.push(None);
    state.region_gain_db.push(0.0);
    assert!(state.focus_region_at_time(4.5));
    assert!(state.reset_active_region_filter(SpectrumFilterKind::LowPass));
    assert_eq!(state.region_low_pass_hz, vec![None, None]);
    assert_eq!(state.region_high_pass_hz, vec![Some(240.0), None]);
}

#[test]
fn gain_and_filter_edits_undo_and_redo_in_order() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_undo_test__.wav"),
        "test".to_string(),
    );
    state.selection = Some((1.0, 2.0));
    state.adjust_active_region_gain(3.0, Instant::now());
    state.adjust_active_region_filter(SpectrumFilterKind::LowPass, -4.0, Instant::now());
    assert!(state.reset_active_region_gain());
    assert_eq!(state.undo_stack.len(), 3);

    state.undo();
    assert_eq!(state.single_region_gain_db, 3.0);
    state.undo();
    assert_eq!(state.single_low_pass_hz, None);
    state.undo();
    assert_eq!(state.single_region_gain_db, 0.0);
    state.redo();
    state.redo();
    assert_eq!(state.single_region_gain_db, 3.0);
    assert_eq!(state.single_low_pass_hz, Some(6_000.0));

    state.adjust_active_region_filter(SpectrumFilterKind::HighPass, 4.0, Instant::now());
    assert!(state.redo_stack.is_empty());
    state.undo();
    assert_eq!(state.single_high_pass_hz, None);
    assert_eq!(state.single_low_pass_hz, Some(6_000.0));
}

#[test]
fn rapid_gain_scrolls_share_one_undo_step_until_the_gap_exceeds_300_ms() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_scroll_undo__.wav"),
        "test".to_string(),
    );
    state.selection = Some((1.0, 2.0));
    let start = Instant::now();
    state.adjust_active_region_gain(1.0, start);
    state.adjust_active_region_gain(1.0, start + Duration::from_millis(200));
    state.adjust_active_region_gain(1.0, start + Duration::from_millis(500));
    assert_eq!(state.undo_stack.len(), 1);
    assert_eq!(state.single_region_gain_db, 3.0);

    state.adjust_active_region_gain(1.0, start + Duration::from_millis(801));
    assert_eq!(state.undo_stack.len(), 2);
    state.undo();
    assert_eq!(state.single_region_gain_db, 3.0);
    state.undo();
    assert_eq!(state.single_region_gain_db, 0.0);
    state.adjust_active_region_gain(2.0, start + Duration::from_millis(900));
    assert_eq!(state.undo_stack.len(), 1);
    assert!(state.redo_stack.is_empty());
    state.undo();
    assert_eq!(state.single_region_gain_db, 0.0);
}

#[test]
fn scroll_history_separates_filter_types_and_slices() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_scroll_targets__.wav"),
        "test".to_string(),
    );
    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
    state.active_multi_index = Some(0);
    let start = Instant::now();
    state.adjust_active_region_filter(SpectrumFilterKind::LowPass, -1.0, start);
    state.adjust_active_region_filter(
        SpectrumFilterKind::LowPass,
        -1.0,
        start + Duration::from_millis(100),
    );
    state.adjust_active_region_filter(
        SpectrumFilterKind::HighPass,
        1.0,
        start + Duration::from_millis(150),
    );
    state.active_multi_index = Some(1);
    state.adjust_active_region_filter(
        SpectrumFilterKind::HighPass,
        1.0,
        start + Duration::from_millis(200),
    );
    assert_eq!(state.undo_stack.len(), 3);
    state.undo();
    assert_eq!(state.active_multi_index, Some(1));
    assert_eq!(state.region_high_pass_hz[1], None);
    assert!(state.region_high_pass_hz[0].is_some());
    state.undo();
    assert_eq!(state.active_multi_index, Some(0));
    assert_eq!(state.region_high_pass_hz.first().copied().flatten(), None);
    assert!(state.region_low_pass_hz[0].is_some());
    state.undo();
}

#[test]
fn ultrasonic_filter_cutoff_starts_at_nyquist_for_high_sample_rates() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_filter_test__.wav"),
        "test".to_string(),
    );
    state.metadata = Some(AudioMetadata {
        channels: 1,
        sample_rate: 384_000,
        duration_ms: 60_000,
        format: "wav".to_string(),
    });
    let range = state.filter_cutoff_range();
    assert_eq!(range, (20.0, 192_000.0));

    let cutoff = AudioPreviewState::step_filter_cutoff(
        None,
        0.0,
        SpectrumFilterKind::LowPass,
        range,
        384_000.0,
    );
    assert_eq!(cutoff, None);

    // First adjustment starts at half sample rate (192,000 Hz)
    let cutoff = AudioPreviewState::step_filter_cutoff(
        None,
        -4.0,
        SpectrumFilterKind::LowPass,
        range,
        384_000.0,
    );
    assert_eq!(cutoff, Some(96_000.0));

    // For standard 44.1k file, range caps at <= 20k and starts at 12k
    state.metadata = Some(AudioMetadata {
        channels: 1,
        sample_rate: 44_100,
        duration_ms: 60_000,
        format: "wav".to_string(),
    });
    let range_44k = state.filter_cutoff_range();
    assert_eq!(range_44k.1, 19_845.0);

    let cutoff_44k = AudioPreviewState::step_filter_cutoff(
        None,
        -4.0,
        SpectrumFilterKind::LowPass,
        range_44k,
        44_100.0,
    );
    assert_eq!(cutoff_44k, Some(6_000.0));
}

#[test]
fn deleting_a_region_restores_the_previous_session_on_undo() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.wav");
    let samples = vec![0_u8; 16_000];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&8_000_u32.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);
    std::fs::write(&original, wav).unwrap();

    let mut state = AudioPreviewState::new(original.clone(), "test".to_string());
    state.duration_sec = 1.0;
    state.selection = Some((0.25, 0.5));
    state.adjust_active_region_gain(6.0, Instant::now());
    state.delete_active_slice_from_session(0.0);
    wait_for_session_edit(&mut state);
    let edited = state.path.clone();
    assert_ne!(edited, original);
    assert!(edited.is_file());
    assert_eq!(state.duration_sec, 0.75);
    state.undo();
    assert_eq!(state.path, original);
    assert_eq!(state.selection, Some((0.25, 0.5)));
    assert_eq!(state.single_region_gain_db, 6.0);
    assert_eq!(state.duration_sec, 1.0);
    state.redo();
    assert_eq!(state.path, edited);
    assert_eq!(state.selection, None);
    assert_eq!(state.duration_sec, 0.75);
}

#[test]
fn range_deletion_bakes_gain_and_handles_reversed_and_boundary_ranges() {
    for (range, frames) in [
        ((0.75, 0.25), 4000),
        ((0.0, 0.25), 6000),
        ((0.75, 1.0), 6000),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("range.wav");
        write_stereo_pcm_fixture(&input, &[8192; 8000], &[-16384; 8000], 8000);
        let mut state = AudioPreviewState::new(input.clone(), "range".into());
        state.cancel_session_analysis();
        state.duration_sec = 1.0;
        state.selection = Some(range);
        state.track_gain_db = -6.020_6;
        state.delete_active_slice_from_session(0.0);
        assert_eq!(
            state.path, input,
            "submission must leave the visible session intact"
        );
        wait_for_session_edit(&mut state);
        assert_ne!(state.path, input, "{:?}", state.status_toast);
        let analysis = state.analysis.as_ref().unwrap();
        assert_eq!(analysis.total_frames, frames);
        assert!((analysis.ch1_samples[frames / 2] - 0.125).abs() < 0.0001);
        assert!((analysis.ch2_samples.as_ref().unwrap()[frames / 2] + 0.25).abs() < 0.0001);
        assert_eq!(state.track_gain_db, 0.0, "the gain is baked exactly once");
        assert!(state.has_session_edits());
        state.undo();
        assert_eq!(state.path, input);
        assert_eq!(state.track_gain_db, -6.020_6);
    }
}

#[test]
fn range_deletion_keeps_mono_and_preserves_crossfaded_surviving_effects() {
    let (directory, stereo) = generate_fixture_wav("delete-stereo", 1.0);
    let input = directory.path().join("mono.wav");
    assert!(
        std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(stereo)
            .args(["-ac", "1"])
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
    let original = compute_full_audio_analysis(&input, 50).unwrap();
    let mut state = AudioPreviewState::new(input.clone(), "mono".into());
    state.cancel_session_analysis();
    state.duration_sec = 1.0;
    state.selection_mode = SelectionMode::Multi;
    state.multi_selections = vec![(0.0, 0.2), (0.4, 0.6)];
    state.region_gain_db = vec![-6.020_6, 0.0];
    state.region_crossfade_sec = vec![0.02, 0.0];
    state.active_multi_index = Some(1);
    state.delete_active_slice_from_session(0.0);
    wait_for_session_edit(&mut state);
    assert_ne!(state.path, input, "{:?}", state.status_toast);
    let edited = state.analysis.as_ref().unwrap();
    assert_eq!(original.channels, 1);
    assert_eq!(edited.channels, 1);
    assert_eq!(edited.ch2_samples.is_some(), original.ch2_samples.is_some());
    let before = (0.1 * original.sample_rate as f32) as usize;
    let after = (0.7 * original.sample_rate as f32) as usize;
    assert!((edited.ch1_samples[before] - original.ch1_samples[before] * 0.5).abs() < 0.001);
    assert!(
        (edited.ch1_samples[after]
            - original.ch1_samples[after + (0.2 * original.sample_rate as f32) as usize])
            .abs()
            < 0.001
    );
}

#[test]
fn range_deletion_rejects_expired_source_and_stale_completion() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("checked.wav");
    write_stereo_pcm_fixture(&input, &[8192; 8000], &[4096; 8000], 8000);
    let mut state = AudioPreviewState::new(input.clone(), "checked".into());
    state.cancel_session_analysis();
    state.duration_sec = 1.0;
    state.selection = Some((0.25, 0.5));
    state.read_check =
        crate::file_source::ReadCheck::new(|_| Err("expired source observation".into()));
    state.delete_active_slice_from_session(0.0);
    wait_for_session_edit(&mut state);
    assert_eq!(state.path, input);
    assert!(
        state
            .status_toast
            .as_ref()
            .unwrap()
            .0
            .text
            .contains("expired source observation")
    );
    state.read_check = Default::default();
    state.delete_active_slice_from_session(0.0);
    state.selection = Some((0.5, 0.75));
    wait_for_session_edit(&mut state);
    assert_eq!(state.path, input);
    assert_eq!(state.selection, Some((0.5, 0.75)));
    assert!(
        state
            .status_toast
            .as_ref()
            .unwrap()
            .0
            .text
            .contains("session changed")
    );
}

#[test]
fn editor_handoff_renders_crossfade_filter_graph() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    let generated = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&source)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    assert!(generated);

    let mut state = AudioPreviewState::new(source, "test".to_string());
    state.duration_sec = 1.0;
    state.selection = Some((0.4, 0.6));
    state.single_region_gain_db = -6.0;
    state.single_crossfade_sec = 0.2;
    let (filter, mode) = state.editor_render_filter();
    let rendered = session_edit::render_session_file(
        &state.path,
        None,
        &Default::default(),
        filter,
        mode,
        &AtomicBool::new(false),
    )
    .unwrap();
    let analysis = compute_full_audio_analysis(rendered.staging.path(), 50).unwrap();
    assert_eq!(analysis.total_frames, 44100);
    assert!(
        analysis
            .ch1_samples
            .iter()
            .any(|sample| sample.abs() > 0.01)
    );
}

#[test]
fn sorting_and_removing_multi_regions_keeps_their_effects_attached() {
    let mut state = AudioPreviewState::new(
        PathBuf::from("/__little_clip_missing_region_sort__.wav"),
        "test".to_string(),
    );
    state.multi_selections = vec![(4.0, 5.0), (1.0, 2.0)];
    state.region_gain_db = vec![6.0, -3.0];
    state.region_crossfade_sec = vec![0.4, 0.1];
    state.region_low_pass_hz = vec![Some(2_000.0), None];
    state.region_high_pass_hz = vec![None, Some(200.0)];
    state.active_multi_index = Some(0);
    state.sort_multi_regions();
    assert_eq!(state.multi_selections, vec![(1.0, 2.0), (4.0, 5.0)]);
    assert_eq!(state.region_gain_db, vec![-3.0, 6.0]);
    assert_eq!(state.region_crossfade_sec, vec![0.1, 0.4]);
    assert_eq!(state.region_low_pass_hz, vec![None, Some(2_000.0)]);
    assert_eq!(state.region_high_pass_hz, vec![Some(200.0), None]);
    assert_eq!(state.active_multi_index, Some(1));
    state.remove_multi_region(0);
    assert_eq!(state.multi_selections, vec![(4.0, 5.0)]);
    assert_eq!(state.region_gain_db, vec![6.0]);
    assert_eq!(state.region_crossfade_sec, vec![0.4]);
    assert_eq!(state.region_low_pass_hz, vec![Some(2_000.0)]);
    assert_eq!(state.active_multi_index, Some(0));
}

#[test]
fn playback_reads_updated_session_gain_without_restarting_source() {
    let (shared, mut preview) = preview_source_with_region(AudioRegionEdit {
        start_sec: 0.0,
        end_sec: 1.0,
        gain_db: f32::NEG_INFINITY,
        ..AudioRegionEdit::default()
    });
    assert_eq!(preview.next(), Some(0.0));
    shared.edits.write().unwrap().regions[0].gain_db = 0.0;
    shared.generation.fetch_add(1, Ordering::Release);
    assert_eq!(preview.next(), Some(0.5));
}

#[test]
fn playback_blends_gain_across_region_crossfades() {
    let (_, mut preview) = preview_source_with_region(AudioRegionEdit {
        start_sec: 0.5,
        end_sec: 1.0,
        crossfade_sec: 0.5,
        gain_db: f32::NEG_INFINITY,
        ..AudioRegionEdit::default()
    });
    assert_eq!(preview.next(), Some(0.5));
    assert_eq!(preview.next(), Some(0.25));
    assert_eq!(preview.next(), Some(0.0));
    assert_eq!(preview.next(), Some(0.0));
    assert_eq!(preview.next(), Some(0.0));
    assert_eq!(preview.next(), Some(0.25));
    assert_eq!(preview.next(), Some(0.5));
}
