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
        let preview = embedded_preview(&path, edge, &cancelled, Some).unwrap();
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
    assert!(embedded_preview(&path, 128, &AtomicBool::new(false), Some).is_none());
}

#[test]
fn clipboard_skips_a_corrupt_large_preview_for_a_decodable_smaller_one() {
    let mut jpeg = Cursor::new(Vec::new());
    RgbImage::new(64, 48)
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .unwrap();
    let mut jpeg = jpeg.into_inner();
    let scan = jpeg
        .windows(2)
        .position(|marker| marker == [0xff, 0xda])
        .unwrap();
    jpeg.truncate(scan);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    assert_eq!(preview_dimensions(&jpeg), Some((64, 48)));
    assert!(decode_preview(&jpeg, Some("image/jpeg")).is_none());

    let mut raw = FIXTURE.to_vec();
    // Replace the first embedded RGB strip with the truncated JPEG. The second
    // embedded preview is still an intact 8 by 4 RGB strip.
    let raw_ifd_entries = u16::from_le_bytes(raw[8..10].try_into().unwrap()) as usize;
    let next_ifd = 10 + raw_ifd_entries * 12;
    let preview_ifd = u32::from_le_bytes(raw[next_ifd..next_ifd + 4].try_into().unwrap()) as usize;
    let entry_count =
        u16::from_le_bytes(raw[preview_ifd..preview_ifd + 2].try_into().unwrap()) as usize;
    let offset = raw.len() as u32;
    let entries = &mut raw[preview_ifd + 2..preview_ifd + 2 + entry_count * 12];
    for entry in entries.chunks_exact_mut(12) {
        let value = match u16::from_le_bytes(entry[..2].try_into().unwrap()) {
            259 => 7, // JPEG compression
            273 => offset,
            279 => jpeg.len() as u32,
            _ => continue,
        };
        entry[8..12].copy_from_slice(&value.to_le_bytes());
    }
    raw.extend_from_slice(&jpeg);
    let parsed = FlashRaw::open_with(raw.as_slice(), raw_limits()).unwrap();
    assert!(parsed.previews().iter().any(|preview| {
        preview_candidate(&parsed, preview).is_some_and(|(_, _, edge)| edge == 64)
    }));

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("damaged-preview.dng");
    std::fs::write(&path, &raw).unwrap();
    let (bytes, width, height) = extract_raw_image_preview(&path).unwrap();
    assert_eq!((width, height), (4, 8));
    assert!(decode_preview(&bytes, Some("image/jpeg")).is_some());
}
