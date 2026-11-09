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
    assert!(known(Path::new("BANK.SF2")));
    assert!(known(Path::new("BANK.NKI")));
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
