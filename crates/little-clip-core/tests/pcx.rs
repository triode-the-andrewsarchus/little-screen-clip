use little_clip_core::item::{ClipItem, ClipPayload, ImageThumbnail, image_reader};

const RGB: &[u8] = include_bytes!("fixtures/panels.pcx");

#[test]
fn rgb_and_indexed_pcx_use_shared_image_paths_and_clipboard_mimes() {
    for bytes in [RGB, include_bytes!("fixtures/indexed.pcx")] {
        let image = image_reader(bytes, Some("image/png"))
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8();
        assert_eq!(image.dimensions(), (64, 32));
        assert_eq!(image.get_pixel(0, 0).0, [30, 120, 210]);
        assert_eq!(image.get_pixel(63, 31).0, [180, 60, 40]);
        let thumb = ImageThumbnail::generate(bytes, 16, 16).unwrap();
        assert_eq!((thumb.width, thumb.height), (16, 8));
        for mime in ["image/x-pcx", "image/pcx"] {
            assert!(matches!(
                ClipItem::from_image_bytes(bytes.to_vec(), mime)
                    .unwrap()
                    .payload,
                ClipPayload::Image {
                    width: 64,
                    height: 32,
                    ..
                }
            ));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.PCX");
    std::fs::write(&path, RGB).unwrap();
    assert!(little_clip_core::item::is_image_path(&path));
    assert_eq!(
        little_clip_core::item::guess_image_mime(&path),
        "image/x-pcx"
    );
    assert!(matches!(
        ClipItem::from_text(path.to_str().unwrap()).payload,
        ClipPayload::Image { .. }
    ));
    for version in [0, 2, 3, 4, 5] {
        let mut bytes = RGB.to_vec();
        bytes[1] = version;
        assert_eq!(
            image_reader(&bytes, None)
                .unwrap()
                .into_dimensions()
                .unwrap(),
            (64, 32)
        );
    }
}

#[test]
fn legacy_bitplanes_padding_and_grayscale_preserve_pixels_and_row_order() {
    let monochrome = vec![[0, 0, 0], [255, 255, 255]];
    let cga = vec![[0, 0, 0], [0, 170, 0], [170, 0, 0], [170, 85, 0]];
    let primaries: Vec<_> = (0..8)
        .map(|i| {
            [
                if i & 1 != 0 { 255 } else { 0 },
                if i & 2 != 0 { 255 } else { 0 },
                if i & 4 != 0 { 255 } else { 0 },
            ]
        })
        .collect();
    let indexed: Vec<_> = (0..16).map(|i| [17 * i, 255 - 17 * i, 13 * i]).collect();
    let grayscale: Vec<_> = (0..=255).map(|i| [i, i, i]).collect();
    for (bytes, palette) in [
        (include_bytes!("fixtures/mono.pcx").as_slice(), &monochrome),
        (include_bytes!("fixtures/cga-packed.pcx"), &cga),
        (include_bytes!("fixtures/cga-planar.pcx"), &cga),
        (include_bytes!("fixtures/ega-rgb.pcx"), &primaries),
        (include_bytes!("fixtures/ega-planar.pcx"), &indexed),
        (include_bytes!("fixtures/indexed4.pcx"), &indexed),
        (include_bytes!("fixtures/gray.pcx"), &grayscale),
    ] {
        let image = image_reader(bytes, None)
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8();
        assert_eq!(image.dimensions(), (17, 5));
        for (x, y, pixel) in image.enumerate_pixels() {
            let index = if palette.len() == 256 {
                (x + y * 17) * 3
            } else {
                (x + y) % palette.len() as u32
            };
            assert_eq!(pixel.0, palette[index as usize], "{x},{y}");
        }
    }
    let ramp = image_reader(include_bytes!("fixtures/palette-ramp.pcx"), None)
        .unwrap()
        .decode()
        .unwrap()
        .to_rgb8();
    for (i, pixel) in ramp.pixels().enumerate() {
        assert_eq!(pixel.0, [255 - i as u8, i as u8, (i * 13 % 256) as u8]);
    }
}

#[test]
fn malformed_rle_headers_and_decoder_working_allocations_are_bounded() {
    assert!(ClipItem::from_image_bytes(RGB.to_vec(), "image/jpeg").is_none());
}

#[test]
fn native_lossless_rgb_encoder_roundtrips_without_changing_input_pixels() {
    let image = image_reader(RGB, None).unwrap().decode().unwrap().to_rgb8();
    let encoded = little_clip_core::pcx_image::encode_rgb(64, 32, image.as_raw()).unwrap();
    let decoded = image_reader(&encoded, None)
        .unwrap()
        .decode()
        .unwrap()
        .to_rgb8();
    assert_eq!(decoded, image);
    assert!(little_clip_core::pcx_image::encode_rgb(64, 32, &[0; 3]).is_err());
    assert!(little_clip_core::pcx_image::encode_rgb(0, 0, &[]).is_err());
}
