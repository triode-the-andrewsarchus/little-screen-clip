use super::*;
use std::io::{Seek, SeekFrom, Write};

fn source(bytes: &[u8]) -> File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file
}

fn interpret(encoding: Encoding) -> RawInterpretation {
    RawInterpretation {
        encoding,
        sample_rate_hz: None,
        center_hz: None,
    }
}

#[test]
fn reads_rtl_unsigned_and_hackrf_signed_samples_at_retained_offsets() {
    let mut file = source(&[0, 1, 127, 128, 255, 0, 0, 0]);
    file.seek(SeekFrom::Start(3)).unwrap();
    let mut retained = file.try_clone().unwrap();
    let capture = Capture::open_raw(file, interpret(Encoding::Cu8)).unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        capture.read_samples(1..3, &cancel).unwrap(),
        [
            Complex32::new(-0.5 / 128.0, 0.5 / 128.0),
            Complex32::new(127.5 / 128.0, -127.5 / 128.0)
        ]
    );
    assert_eq!(retained.stream_position().unwrap(), 3);
    let signed = Capture::open_raw(source(&[128, 64]), interpret(Encoding::Ci8)).unwrap();
    assert_eq!(
        signed.read_samples(0..1, &cancel).unwrap(),
        [Complex32::new(-1.0, 0.5)]
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(signed.read_samples(0..1, &cancel).is_err());
    assert!(Capture::open_raw(source(&[0, 0, 0]), interpret(Encoding::Ci8)).is_err());
}

#[test]
fn reads_big_endian_complex_words_and_rejects_missing_sample_bytes() {
    let data = [i16::MIN, 16384, 0, -8192]
        .into_iter()
        .flat_map(|value| value.to_be_bytes())
        .collect::<Vec<_>>();
    let capture = Capture::open_raw(source(&data), interpret(Encoding::Ci16Be)).unwrap();
    assert_eq!(
        capture.read_samples(0..2, &AtomicBool::new(false)).unwrap(),
        [Complex32::new(-1.0, 0.5), Complex32::new(0.0, -0.25)]
    );
    assert!(capture.read_samples(1..3, &AtomicBool::new(false)).is_err());
}

#[test]
fn indexes_nonconforming_headers_and_annotations_relative_to_the_sigmf_offset() {
    let directory = tempfile::tempdir().unwrap();
    let metadata = directory.path().join("capture.sigmf-meta");
    let data = directory.path().join("capture.iq");
    std::fs::write(
        &metadata,
        br#"{
            "global":{"core:version":"1.2.0","core:datatype":"ci8","core:sample_rate":4000,
                      "core:dataset":"capture.iq","core:offset":100,"core:trailing_bytes":1},
            "captures":[{"core:sample_start":100,"core:header_bytes":2,"core:frequency":1000000},
                        {"core:sample_start":102,"core:header_bytes":1,"core:frequency":2000000,"core:global_index":120}],
            "annotations":[{"core:sample_start":101,"core:label":"Burst","core:freq_lower_edge":1000000,
                            "core:freq_upper_edge":1001000}]}"#,
    ).unwrap();
    std::fs::write(data, [9, 9, 1, 2, 3, 4, 99, 5, 6, 7, 8, 9]).unwrap();
    let cancel = AtomicBool::new(false);
    let recording = Recording::open_sigmf(&metadata, None, ReadCheck::default(), &cancel).unwrap();
    recording
        .checked(&cancel, |capture| {
            assert_eq!(capture.sample_count(), 4);
            assert_eq!(capture.metadata().sample_offset, 100);
            assert_eq!(capture.metadata().annotations[0].samples, 1..2);
            assert_eq!(capture.tuning_at(2).unwrap().original_sample, Some(120));
            assert_eq!(
                capture.read_samples(1..3, &cancel)?,
                [
                    Complex32::new(3.0 / 128.0, 4.0 / 128.0),
                    Complex32::new(5.0 / 128.0, 6.0 / 128.0),
                ]
            );
            let spectrum = capture.spectrum(0..4, &cancel)?;
            assert!(spectrum.crosses_tuning);
            assert!(spectrum.center_hz.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn admits_either_pair_member_and_revalidates_both_revisions() {
    let directory = tempfile::tempdir().unwrap();
    let metadata = directory.path().join("recording.sigmf-meta");
    let data = directory.path().join("recording.sigmf-data");
    let json = br#"{"global":{"core:version":"1.0.0","core:datatype":"cu8"},"captures":[],"annotations":[]}"#;
    std::fs::write(&metadata, json).unwrap();
    std::fs::write(&data, [13, 14, 15, 16, 17, 18]).unwrap();
    let cancel = AtomicBool::new(false);
    let a = Recording::open_sigmf(&metadata, None, ReadCheck::default(), &cancel).unwrap();
    let b = Recording::open_sigmf(&data, None, ReadCheck::default(), &cancel).unwrap();
    assert_eq!(a.revision_key(), b.revision_key());
    assert_ne!(a.source(), b.source());
    assert_eq!(a.capture().sample_count(), 3);
    assert_eq!(a.capture().duration_seconds(), None);
    std::fs::write(&metadata, br#"{"global":{"core:dataset":"../outside.iq"}}"#).unwrap();
    assert!(
        a.checked(&cancel, |capture| capture.read_samples(0..1, &cancel))
            .is_err()
    );
    assert!(
        b.checked(&cancel, |capture| capture.read_samples(0..1, &cancel))
            .is_err()
    );
}

#[test]
fn rejects_non_sibling_dataset_names_before_opening_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recording.sigmf-meta");
    std::fs::write(
        &path, br#"{"global":{"core:version":"1.0.0","core:datatype":"ci8","core:dataset":"../outside.iq"},"captures":[],"annotations":[]}"#,
    ).unwrap();
    let error = Recording::open_sigmf(&path, None, ReadCheck::default(), &AtomicBool::new(false))
        .err()
        .unwrap();
    assert!(error.contains("metadata directory"));
}

#[test]
fn finds_a_centered_complex_tone_with_a_calibrated_peak() {
    let count = 1024;
    let bytes = (0..count)
        .flat_map(|index| {
            let angle = -std::f64::consts::TAU * 128.0 * index as f64 / count as f64;
            [(0.5 * angle.cos()) as f32, (0.5 * angle.sin()) as f32]
                .into_iter()
                .flat_map(f32::to_le_bytes)
        })
        .collect::<Vec<_>>();
    let capture = Capture::open_raw(
        source(&bytes),
        RawInterpretation {
            encoding: Encoding::Cf32Le,
            sample_rate_hz: Some(32_000.0),
            center_hz: Some(915_000_000.0),
        },
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let spectrum = capture.spectrum(0..count, &cancel).unwrap();
    let (index, peak) = spectrum
        .bins_dbfs
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap();
    assert_eq!(index, 384);
    assert_eq!(spectrum.relative_hz_at(index), Some(-4000.0));
    assert_eq!(spectrum.frequency_hz_at(index), Some(914_996_000.0));
    assert!((*peak + 6.0206).abs() < 0.001);
    assert!((spectrum.noise_bandwidth_hz.unwrap() - 46.875).abs() < 1e-4);
    assert!((spectrum.density_dbfs_at(index).unwrap() + 22.7300).abs() < 0.02);
    let waterfall = capture.waterfall(0..count, 256, 4, &cancel).unwrap();
    assert!(!waterfall.sampled);
    assert_eq!(waterfall.rows.len(), 4);
    assert_eq!(waterfall.rows[3].samples, 768..1024);
}

#[test]
fn seals_approximate_power_coverage_and_preserves_detected_peak_bins() {
    let mut data = vec![0; 16_384];
    data[0..2].copy_from_slice(&[127, 0]);
    data[2..4].copy_from_slice(&[127, 0]);
    let capture = Capture::open_raw(source(&data), interpret(Encoding::Ci8)).unwrap();
    let cancel = AtomicBool::new(false);
    let overview = capture.overview(0..8192, 2, &cancel).unwrap();
    assert!(overview.sampled());
    assert_eq!(overview.bins[0].observed_samples, 1024);
    let exact = capture.overview(0..64, 2, &cancel).unwrap();
    assert!(!exact.sampled());
    let bursts = exact.bursts_above(-20.0);
    assert_eq!(bursts.len(), 1);
    assert_eq!(bursts[0], 0..32);
}
