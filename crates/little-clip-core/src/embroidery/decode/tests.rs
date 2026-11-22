use super::*;
use crate::embroidery::Design;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/embroidery");
const ORACLE: &str = include_str!("../../../tests/fixtures/embroidery/oracle.sha256");

#[test]
fn all_36_extensions_preserve_ordered_stitches_travel_gaps_and_thread_colors() {
    let cancelled = AtomicBool::new(false);
    let mut extensions = BTreeSet::new();
    for line in ORACLE.lines() {
        let (expected, name) = line.split_once("  ").unwrap();
        let path = Path::new(FIXTURES).join(name);
        assert!(extensions.insert(path.extension().unwrap().to_owned()));
        let design =
            Design::read(&path, &cancelled).unwrap_or_else(|error| panic!("{name}: {error}"));
        let mut hash = Sha256::new();
        for stitch in design.stitches {
            for coordinate in stitch.from.into_iter().chain(stitch.to) {
                hash.update(coordinate.to_le_bytes());
            }
            hash.update(stitch.color);
        }
        assert_eq!(format!("{:x}", hash.finalize()), expected, "{name}");
    }
    assert_eq!(extensions.len(), 36);
}

#[test]
fn truncated_headers_and_out_of_range_offsets_are_bounded() {
    let cancelled = AtomicBool::new(false);
    for line in ORACLE.lines() {
        let (_, name) = line.split_once("  ").unwrap();
        let path = Path::new(FIXTURES).join(name);
        let extension = path.extension().unwrap().to_str().unwrap();
        let decoder = reader(extension).unwrap();
        let fixture = std::fs::read(path).unwrap();
        // Some formats can render a partial stream. Every truncated prefix
        // must hold to the same bounds even when a header is still valid.
        for length in [0, 1, 2, 3, 7, 19, 51, fixture.len() / 2] {
            if let Some(prefix) = fixture.get(..length) {
                let _ = read(decoder, prefix, &cancelled);
            }
        }
        let _ = read(decoder, &[u8::MAX; 0x2000], &cancelled);
    }
    for extension in ["jef", "pcq", "pec", "pes", "phb", "phc", "vp3"] {
        assert!(read(reader(extension).unwrap(), b"broken", &cancelled).is_err());
    }
}

#[test]
fn record_budget_bounds_headerless_streams() {
    let data = [1, 1].repeat(MAX_COMMANDS + 1);
    let error = read(reader("exp").unwrap(), &data, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error, "Embroidery stitch budget exceeded");
}
