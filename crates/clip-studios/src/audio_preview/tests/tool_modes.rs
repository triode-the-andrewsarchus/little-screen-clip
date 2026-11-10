use super::*;

#[test]
fn audio_tool_modes_have_stable_toolbar_labels() {
    assert_eq!(AudioToolMode::Inspect.label(), "[I]nspect & Slice");
    assert_eq!(AudioToolMode::ChannelTools.label(), "[C]hannel Tools");
    assert_eq!(AudioToolMode::StemSeparation.label(), "[S]tem Separation");
    assert_eq!(AudioToolMode::DigitalDecode.label(), "[D]igital Decode");
    assert_eq!(AudioToolMode::HfAnalysis.label(), "[H]F Analysis");
    assert_eq!(AudioToolMode::default(), AudioToolMode::Inspect);
}

#[test]
fn stem_manifest_parser_ignores_worker_log_lines() {
    let output = br#"Downloading model...
Separated track
{"vocals":"/tmp/vocals.wav","drums":"/tmp/drums.wav","bass":"/tmp/bass.wav","other":"/tmp/other.wav"}
"#;
    let stems = parse_stem_manifest(output).expect("manifest should parse");
    assert_eq!(stems.len(), 4);
    assert_eq!(
        stems[0],
        ("vocals".to_string(), PathBuf::from("/tmp/vocals.wav"))
    );
}

#[test]
fn stem_separation_state_starts_without_outputs() {
    let state = StemSeparationState::default();
    assert!(!state.in_progress);
    assert!(state.stems.is_empty());
    assert!(state.workspace.is_none());
    assert!(state.solo_stem.is_none());
}

#[test]
fn grouped_stem_routing_respects_mute_and_solo() {
    assert!(stem_is_audible(0, false, None));
    assert!(!stem_is_audible(0, true, None));
    assert!(stem_is_audible(2, false, Some(2)));
    assert!(!stem_is_audible(1, false, Some(2)));
    assert!(!stem_is_audible(2, true, Some(2)));
}

#[test]
fn switching_editor_stems_identifies_the_previous_player_to_stop() {
    assert_eq!(editor_stem_switch(0, 2, 4), Some((0, 2)));
    assert_eq!(editor_stem_switch(2, 2, 4), None);
    assert_eq!(editor_stem_switch(2, 4, 4), None);
}

#[test]
fn grouped_waveform_scrubbing_maps_and_clamps_time() {
    assert_eq!(waveform_scrub_time(50.0, 0.0, 100.0, 240.0), 120.0);
    assert_eq!(waveform_scrub_time(-10.0, 0.0, 100.0, 240.0), 0.0);
    assert_eq!(waveform_scrub_time(150.0, 0.0, 100.0, 240.0), 240.0);
}

#[test]
fn transient_sensitivity_slider_is_nonlinear_around_the_default() {
    let tolerance = 0.000_01;
    assert!((transient_sensitivity_from_slider(0.0) - 0.2).abs() < tolerance);
    assert!((transient_sensitivity_from_slider(0.5) - 0.35).abs() < tolerance);
    assert!((transient_sensitivity_from_slider(1.0) - 1.0).abs() < tolerance);

    for sensitivity in [0.2, 0.25, 0.35, 0.6, 1.0] {
        let round_trip =
            transient_sensitivity_from_slider(transient_sensitivity_to_slider(sensitivity));
        assert!((round_trip - sensitivity).abs() < tolerance);
    }
}

#[test]
fn audacity_stem_list_imports_each_file_as_a_track() {
    let paths = vec![
        PathBuf::from("/tmp/stems/vocals.wav"),
        PathBuf::from("/tmp/stems/drums.wav"),
        PathBuf::from("/tmp/stems/bass.wav"),
        PathBuf::from("/tmp/stems/other.wav"),
    ];
    assert_eq!(
        audacity_lof_contents(&paths).unwrap(),
        "file \"/tmp/stems/vocals.wav\"\n\
             file \"/tmp/stems/drums.wav\"\n\
             file \"/tmp/stems/bass.wav\"\n\
             file \"/tmp/stems/other.wav\"\n"
    );
}

#[test]
fn isolated_audacity_profile_is_writable_and_skips_effect_scanning() {
    let workspace = tempfile::tempdir().unwrap();
    let stems = vec![PathBuf::from("/tmp/stems/vocals.wav")];
    let mut command = std::process::Command::new("audacity");
    prepare_isolated_audacity_command(&mut command, workspace.path(), &stems).unwrap();

    let config = std::fs::read_to_string(
        workspace
            .path()
            .join("audacity-profile/config/audacity/audacity.cfg"),
    )
    .unwrap();
    assert!(config.contains("SkipEffectsScanAtStartup=1"));
    assert!(workspace.path().join("separated-stems.lof").is_file());

    let environment = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.unwrap().to_string_lossy().into_owned(),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        environment.get("XDG_CONFIG_HOME"),
        Some(
            &workspace
                .path()
                .join("audacity-profile/config")
                .to_string_lossy()
                .into_owned()
        ),
    );
    assert_eq!(
        environment.get("VST_PATH"),
        Some(
            &workspace
                .path()
                .join("audacity-profile/empty-plugins")
                .to_string_lossy()
                .into_owned()
        ),
    );
}
