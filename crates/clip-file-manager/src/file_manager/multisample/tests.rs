use super::*;
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../little-clip-core/tests/fixtures/multisample")
        .join(name)
}
#[test]
fn sfz_includes_defines_and_preview_selection_keep_real_region_indices() {
    let path = fixture("harbor.sfz");
    let token = AtomicBool::new(false);
    let document = read(&path, &token).unwrap().document;
    let zones = &document.instruments[0].regions;
    assert_eq!(zones.len(), 3);
    assert_eq!(zones[0].root, 60);
    assert_eq!(zones[0].sample, "./soft.wav");
    assert_eq!(zones[1].velocity, [64, 127]);
    assert_eq!(zones[2].key, [36, 47]);
    let data = load(&path, &token);
    let summary = data.multisample.unwrap();
    assert!(summary.pcm.is_none());
    assert!(summary.instruments[0].regions[0].sample.is_empty());
    let mut viewer = Viewer::load(&path, &token).unwrap();
    viewer.select(0, 1);
    assert_eq!(viewer.current().unwrap().name, "Bright keys");
    assert!(
        (little_clip_core::file_format::Format::for_path(Path::new("BANK.SF2"))
            == Some(little_clip_core::file_format::Format::Multisample))
    );
    assert!(
        (little_clip_core::file_format::Format::for_path(Path::new("BANK.NKI"))
            == Some(little_clip_core::file_format::Format::Multisample))
    );
}
#[test]
fn preprocessing_is_bounded_and_cannot_read_kernel_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cycle.sfz");
    std::fs::write(&path, "#include \"cycle.sfz\"\n").unwrap();
    assert!(
        read(&path, &AtomicBool::new(false))
            .err()
            .unwrap()
            .contains("32-file")
    );
    std::fs::write(&path, "#include \"/dev/zero\"\n").unwrap();
    assert!(read(&path, &AtomicBool::new(false)).is_err());
    std::fs::write(&path, "#define $A $A\n<region> sample=$A\n").unwrap();
    assert!(read(&path, &AtomicBool::new(false)).is_err());
    assert!(read(&fixture("harbor.sfz"), &AtomicBool::new(true)).is_err());
}
#[test]
fn ambiguous_xml_and_elixir_names_keep_their_normal_handling_without_sampler_contents() {
    let dir = tempfile::tempdir().unwrap();
    let token = AtomicBool::new(false);
    let xml = dir.path().join("mapping.xml");
    std::fs::copy(fixture("harbor.dspreset"), &xml).unwrap();
    assert_eq!(
        read_if_identified(&xml, &token).unwrap().instruments[0]
            .regions
            .len(),
        3
    );
    std::fs::write(&xml, b"<notes><sample>ordinary XML</sample></notes>").unwrap();
    assert!(read_if_identified(&xml, &token).is_none());
    let script = dir.path().join("mix.exs");
    std::fs::write(&script, b"# Elixir build script\nIO.puts(\"hello\")\n").unwrap();
    assert!(read_if_identified(&script, &token).is_none());
    for name in ["ordinary.iso", "ordinary.preset"] {
        let path = dir.path().join(name);
        std::fs::copy(fixture("harbor.dspreset"), &path).unwrap();
        assert!(read_if_identified(&path, &token).is_none());
    }
}
#[test]
fn selected_external_and_embedded_samples_decode_to_bounded_raw_audio() {
    use rodio::Source;
    for name in [
        "harbor.sfz",
        "compressed.sfz",
        "harbor.xrni",
        "harbor.sf2",
        "harbor.dspreset",
        "harbor.multisample",
        "harbor.dslibrary",
    ] {
        let path = fixture(name);
        let mapping = read(&path, &AtomicBool::new(false)).unwrap();
        let doc = &mapping.document;
        let audio = audition::prepare(
            &doc,
            &doc.instruments[0].regions[1],
            path.parent().unwrap(),
            mapping.archive.as_ref(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(audio.source.channels().get(), 1);
        assert_eq!(audio.source.sample_rate().get(), 22050);
        assert!((audio.seconds - 2.0).abs() < 0.001);
        assert!(audio.peaks.iter().any(|p| *p > 0.6));
        assert!(audio.peaks.len() <= 256);
        assert!(
            audition::prepare(
                &doc,
                &doc.instruments[0].regions[0],
                path.parent().unwrap(),
                mapping.archive.as_ref(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
    let doc = read(&fixture("harbor.sfz"), &AtomicBool::new(false))
        .unwrap()
        .document;
    let mut region = doc.instruments[0].regions[0].clone();
    region.sample = "/dev/zero".into();
    assert!(
        audition::prepare(&doc, &region, Path::new("/"), None, &AtomicBool::new(false)).is_err()
    );
}
#[test]
#[ignore = "requires packaged native helper and Bubblewrap"]
fn nki_helper_reads_legacy_and_current_containers_in_its_private_sandbox() {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/nki-helper");
    let token = AtomicBool::new(false);
    let legacy = nki::read_with(
        &helper,
        std::fs::read(fixture("legacy.nki")).unwrap(),
        &token,
    )
    .unwrap();
    assert_eq!(legacy.instruments[0].regions.len(), 2);
    assert_eq!(legacy.instruments[0].regions[1].velocity, [64, 127]);
    let bytes =
        include_bytes!("../../../../little-clip-nki/tests/fixtures/kontakt680.nki").to_vec();
    let current = nki::read_with(&helper, bytes, &token).unwrap();
    assert_eq!(current.instruments[0].name, "FM8 Jam");
    assert_eq!(current.instruments[0].regions.len(), 61);
    assert_eq!(current.instruments[0].regions[0].root, 71);
    for name in ["harbor.exs", "harbor-big.exs"] {
        let doc = nki::read_with(&helper, std::fs::read(fixture(name)).unwrap(), &token).unwrap();
        assert_eq!(doc.instruments[0].name, "Harbor EXS");
        assert_eq!(doc.instruments[0].regions[1].velocity, [64, 127]);
        let audio = audition::prepare(
            &doc,
            &doc.instruments[0].regions[1],
            fixture(name).parent().unwrap(),
            None,
            &token,
        )
        .unwrap();
        assert!((audio.seconds - 2.0).abs() < 0.001);
        assert!(audio.peaks.iter().any(|p| *p > 0.6));
    }
    assert!(nki::read_with(&helper, vec![0; 32], &token).is_err());
    assert!(nki::read_with(&helper, vec![0; 32], &AtomicBool::new(true)).is_err());
}

#[test]
fn renoise_slice_aliases_use_the_indexed_primary_and_relative_interval() {
    let mapping = read(&fixture("harbor.xrni"), &AtomicBool::new(false)).unwrap();
    let regions = &mapping.document.instruments[0].regions;
    assert_eq!(regions.len(), 3);
    assert_eq!(regions[1].sample, "SampleData/Sample01 (bright).wav");
    assert_eq!(regions[2].sample, regions[1].sample);
    assert_eq!(regions[2].start, 11025);
    assert_eq!(regions[2].end, Some(22050));
    let audio = audition::prepare(
        &mapping.document,
        &regions[2],
        Path::new("/"),
        mapping.archive.as_ref(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!((audio.seconds - 0.5).abs() < 0.001);
    let bytes = br#"<RenoiseInstrument><SampleGenerator><Samples><Sample><Mapping/></Sample></Samples></SampleGenerator></RenoiseInstrument>"#;
    assert!(little_clip_core::multisample::xml::parse_renoise(bytes, "Missing", &[]).is_err());
    assert!(
        little_clip_core::multisample::xml::parse_renoise(
            bytes,
            "Duplicate",
            &[(0, "a.wav".into()), (0, "b.wav".into())]
        )
        .is_err()
    );
}

#[test]
fn hex_selection_respects_velocity_gaps_and_cycles_overlapping_recordings() {
    let doc = read(&fixture("harbor.sfz"), &AtomicBool::new(false))
        .unwrap()
        .document;
    let regions = &doc.instruments[0].regions;
    let soft = surface::Strike {
        key: 60,
        velocity: 32,
    };
    let hard = surface::Strike {
        key: 60,
        velocity: 100,
    };
    assert_eq!(surface::choose(regions, soft, None), Some(0));
    assert_eq!(surface::choose(regions, hard, None), Some(1));
    for velocity in 1..=127 {
        let index = surface::choose(regions, surface::Strike { key: 60, velocity }, None).unwrap();
        let expected = if velocity <= 63 {
            "soft.wav"
        } else {
            "bright.wav"
        };
        assert!(
            regions[index].sample.ends_with(expected),
            "Wrong recording at velocity {velocity}"
        );
    }

    assert_eq!(
        surface::choose(regions, surface::Strike { key: 100, ..hard }, None),
        None
    );
    let overlap = vec![regions[0].clone(), regions[0].clone()];
    assert_eq!(surface::choose(&overlap, soft, None), Some(0));
    assert_eq!(surface::choose(&overlap, soft, Some(0)), Some(1));
    assert_eq!(surface::choose(&overlap, soft, Some(1)), Some(0));
    assert_eq!(surface::note(60), "C4");
    assert_eq!(surface::note(0), "C-1");
    assert_eq!(surface::note(127), "G9");
    assert_eq!(surface::bounds(&[]), [48, 71]);
}

#[test]
fn harmony_voicings_are_exact_midi_notes_and_never_wrap_at_the_ceiling() {
    use explorer::{Quality::*, Voicing::*};
    assert_eq!(explorer::chord(60, Major, Root), Some(vec![60, 64, 67]));
    assert_eq!(explorer::chord(60, Minor, First), Some(vec![63, 67, 72]));
    assert_eq!(explorer::chord(60, Major, Second), Some(vec![67, 72, 76]));
    assert_eq!(explorer::chord(60, Minor, Open), Some(vec![60, 67, 75]));
    assert_eq!(explorer::chord(120, Major, First), None);
}

#[test]
fn automatic_sampler_modes_require_mapping_hints_and_keep_unknowns_conservative() {
    use explorer::Kind::*;
    let doc = read(&fixture("harbor.sfz"), &AtomicBool::new(false))
        .unwrap()
        .document;
    let mut regions = doc.instruments[0].regions.clone();
    assert_eq!(explorer::classify(&regions), Pitched);
    regions[0]
        .properties
        .push(("pitch_keytrack".into(), "0".into()));
    assert_eq!(explorer::classify(&regions), Hybrid);
    for region in &mut regions {
        region
            .properties
            .push(("Preset".into(), "Bank 128 · program 0".into()));
    }
    assert_eq!(explorer::classify(&regions), Drums);
    for region in &mut regions {
        region.properties.clear();
        region.key = [region.root, region.root];
    }
    assert_eq!(explorer::classify(&regions), Unknown);
    assert_eq!(explorer::classify(&[]), Unknown);
}

#[test]
fn chord_audio_contains_all_three_transposed_voices_with_bounded_headroom() {
    use rodio::Source;
    let path = fixture("harbor.sfz");
    let document = read(&path, &AtomicBool::new(false)).unwrap().document;
    let strikes: Vec<_> = [60, 64, 67]
        .into_iter()
        .map(|key| surface::Strike { key, velocity: 32 })
        .collect();
    let audio = audition::prepare_chord(
        &document,
        0,
        &[0, 0, 0],
        &strikes,
        path.parent().unwrap(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(audio.source.channels().get(), 2);
    assert!((audio.seconds - 2.0).abs() < 0.02);
    let mono: Vec<_> = audio.source.clone().step_by(2).take(22050).collect();
    let energy = |frequency: f64| {
        let (real, imag) =
            mono.iter()
                .enumerate()
                .fold((0.0, 0.0), |(real, imag), (i, &sample)| {
                    let angle = std::f64::consts::TAU * frequency * i as f64 / 44100.0;
                    (
                        real + sample as f64 * angle.cos(),
                        imag + sample as f64 * angle.sin(),
                    )
                });
        real.hypot(imag) / mono.len() as f64
    };
    for frequency in [261.6256, 329.6276, 391.9954] {
        assert!(
            energy(frequency) > 0.025 * f64::from(audition::velocity_gain(32)),
            "Missing chord frequency {frequency}"
        );
    }
    assert!(energy(300.0) < 0.005);
    assert!(mono.iter().all(|sample| sample.abs() <= 0.8));
    assert!(
        audition::prepare_chord(
            &document,
            0,
            &[0, 0, 0],
            &strikes,
            path.parent().unwrap(),
            None,
            &AtomicBool::new(true)
        )
        .is_err()
    );
}

#[test]
fn unmapped_strike_cancels_chord_loading_without_replaying_the_previous_source() {
    let mut viewer = Viewer::load(&fixture("harbor.sfz"), &AtomicBool::new(false)).unwrap();
    viewer.trigger_chord(surface::Strike {
        key: 60,
        velocity: 96,
    });
    assert!(viewer.task.is_some());
    assert_eq!(viewer.chord.len(), 3);
    viewer.trigger(surface::Strike {
        key: 127,
        velocity: 96,
    });
    assert!(viewer.task.is_none());
    assert!(viewer.current().is_none());
    assert!(viewer.audio.is_none());
    assert!(viewer.chord.is_empty());
    // Repeating the unmapped note must not overflow an invalid selection index.
    viewer.trigger(surface::Strike {
        key: 127,
        velocity: 96,
    });
    viewer.select(0, 0);
    assert!(viewer.current().is_some());
    assert!(viewer.error.is_none());
}

#[test]
fn velocity_changes_amplitude_within_one_recording_without_mutating_pcm() {
    use rodio::Source;
    let path = fixture("harbor.sfz");
    let document = read(&path, &AtomicBool::new(false)).unwrap().document;
    let audio = audition::prepare(
        &document,
        &document.instruments[0].regions[0],
        path.parent().unwrap(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    let energy = |velocity| {
        audio
            .source
            .clone()
            .amplify(audition::velocity_gain(velocity))
            .map(|v| v * v)
            .sum::<f32>()
    };
    let soft = energy(32);
    let loud = energy(64);
    assert!((loud / soft - 16.0).abs() < 0.01);
    assert_eq!(energy(0), 0.0);
    assert_eq!(audition::velocity_gain(127), 1.0);
}

#[test]
fn crossing_velocity_boundary_cancels_pending_recording_and_selects_new_sample() {
    let path = fixture("harbor.sfz");
    let mut viewer = Viewer::load(&path, &AtomicBool::new(false)).unwrap();
    viewer.trigger(surface::Strike {
        key: 60,
        velocity: 63,
    });
    assert!(viewer.current().unwrap().sample.ends_with("soft.wav"));
    let soft_request = viewer.task.as_ref().unwrap().token.clone();
    viewer.trigger(surface::Strike {
        key: 60,
        velocity: 64,
    });
    assert!(soft_request.load(Ordering::Relaxed));
    assert!(viewer.current().unwrap().sample.ends_with("bright.wav"));
    let bright_request = viewer.task.as_ref().unwrap().token.clone();
    viewer.trigger(surface::Strike {
        key: 60,
        velocity: 63,
    });
    assert!(bright_request.load(Ordering::Relaxed));
    assert!(viewer.current().unwrap().sample.ends_with("soft.wav"));
}
