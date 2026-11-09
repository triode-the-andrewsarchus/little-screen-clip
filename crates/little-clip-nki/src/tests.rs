// SPDX-License-Identifier: LGPL-3.0-only
use super::*;
const LEGACY: &[u8] = include_bytes!("../tests/fixtures/legacy.nki");
const MODERN: &[u8] = include_bytes!("../tests/fixtures/kontakt680.nki");
#[test]
fn exs_metadata_requires_sampler_headers_and_preserves_endianness() {
    for bytes in [
        include_bytes!("../tests/fixtures/harbor.exs").as_slice(),
        include_bytes!("../tests/fixtures/harbor-big.exs").as_slice(),
    ] {
        assert!(exs::recognizes(bytes));
        let doc = read(bytes).unwrap();
        assert_eq!(doc[0]["name"], "Harbor EXS");
        let regions = doc[0]["regions"].as_array().unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0]["key"], serde_json::json!([48, 71]));
        assert_eq!(regions[1]["velocity"], serde_json::json!([64, 127]));
        assert_eq!(regions[1]["sample"], "bright.wav");
        assert_eq!(regions[0]["loops"], serde_json::json!([[10, 22050]]));
        for n in [0, 4, 20, 83, 100, bytes.len() - 1] {
            assert!(read(&bytes[..n]).is_err());
        }
    }
    assert!(!exs::recognizes(
        b"# ordinary Elixir script\nIO.puts(\"hello\")\n"
    ));
}
#[test]
fn reads_legacy_and_modern_mapping_without_a_runtime_or_sample_files() {
    let legacy = read(LEGACY).unwrap();
    assert_eq!(legacy[0]["name"], "Harbor legacy");
    assert_eq!(legacy[0]["regions"].as_array().unwrap().len(), 2);
    assert_eq!(
        legacy[0]["regions"][1]["velocity"],
        serde_json::json!([64, 127])
    );
    let modern = read(MODERN).unwrap();
    assert_eq!(modern[0]["name"], "FM8 Jam");
    assert_eq!(modern[0]["regions"].as_array().unwrap().len(), 61);
    assert_eq!(modern[0]["regions"][0]["root"], 71);
    assert_eq!(
        modern[0]["regions"][0]["sample"],
        "FM8 Jam Samples/FM8 Jam_ms0_071_h3.wav"
    );
}
#[test]
fn malformed_lengths_and_compression_references_fail_without_panics() {
    for input in [LEGACY, MODERN] {
        for n in 0..input.len().min(512) {
            assert!(read(&input[..n]).is_err());
        }
        for index in (0..input.len()).step_by(17) {
            let mut changed = input.to_vec();
            changed[index] = 255;
            assert!(
                std::panic::catch_unwind(|| read(&changed)).is_ok(),
                "offset {index}"
            );
        }
    }
    assert!(reader::fastlz(&[32, 255, 255], 16).is_err());
    assert!(reader::fastlz(&[0, 42], crate::PAYLOAD + 1).is_err());
    assert_eq!(reader::fastlz(&[0, 42], 1).unwrap(), [42]);
}
#[test]
fn external_entities_and_excessive_nesting_are_rejected() {
    use std::io::Write;
    let mut compressed =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compressed
        .write_all(
            b"<!DOCTYPE x [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><NiSS_Program name='&x;'/>",
        )
        .unwrap();
    let mut input = LEGACY[..36].to_vec();
    input.extend(compressed.finish().unwrap());
    assert!(read(&input).is_err());
    assert!(read(&vec![0; INPUT + 1]).is_err());
    let mut nested = Vec::new();
    for _ in 0..70 {
        let mut structure = vec![1, 0x80, 0];
        structure.extend(0u32.to_le_bytes());
        structure.extend(0u32.to_le_bytes());
        structure.extend((nested.len() as u32).to_le_bytes());
        structure.extend(nested);
        nested = 0x29u16.to_le_bytes().to_vec();
        nested.extend((structure.len() as u32).to_le_bytes());
        nested.extend(structure);
    }
    assert!(binary::read(&nested).unwrap_err().contains("nesting/count"));
}
