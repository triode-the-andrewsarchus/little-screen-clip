use super::*;

const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/raw/oriented-preview.dng");

#[test]
fn selects_smallest_suitable_embedded_preview() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sample.dng");
    std::fs::write(&path, FIXTURE).unwrap();
    let cancelled = AtomicBool::new(false);
    for (edge, width, color) in [
        (6, 8, [220, 30, 10]),
        (9, 12, [20, 40, 200]),
        (24, 12, [20, 40, 200]),
    ] {
        let preview = embedded_preview(&path, edge, &cancelled).unwrap();
        let EmbeddedPixels::Rgb(pixels) = preview.pixels else {
            panic!("expected an uncompressed RGB preview");
        };
        assert_eq!(pixels.width(), width);
        assert_eq!(pixels.get_pixel(0, 0).0, color);
        assert_eq!(preview.orientation, 6);
    }
}

#[test]
fn gallery_uses_embedded_rgb_even_when_sensor_data_cannot_decode() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("corrupt-sensor.dng");
    let mut bytes = FIXTURE.to_vec();
    // Advertise deflate for the first IFD. Its strip still holds uncompressed samples.
    let compression = [3, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0];
    let index = bytes
        .windows(compression.len())
        .position(|w| w == compression)
        .unwrap();
    bytes[index + 8] = 8;
    std::fs::write(&path, &bytes).unwrap();
    assert!(develop(&bytes).is_err());

    let (clipboard, width, height) = extract_raw_image_preview(&path).unwrap();
    assert_eq!((width, height), (6, 12));
    assert_eq!(preview_dimensions(&clipboard), Some((6, 12)));

    let cancelled = AtomicBool::new(false);
    let thumbnail = crate::gallery_image::thumbnail::load(&path, 8, 1, &cancelled).unwrap();
    assert_eq!((thumbnail.width, thumbnail.height), (4, 8));
    assert_eq!(&thumbnail.premultiplied_rgba[..4], &[220, 30, 10, 255]);
    cancelled.store(true, Ordering::Relaxed);
    assert!(crate::gallery_image::thumbnail::load(&path, 8, 1, &cancelled).is_none());
}

#[test]
fn embedded_read_rejects_protected_sources() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("protected.nef");
    std::os::unix::fs::symlink("/proc/self/maps", &path).unwrap();
    assert!(embedded_preview(&path, 128, &AtomicBool::new(false)).is_none());
}
