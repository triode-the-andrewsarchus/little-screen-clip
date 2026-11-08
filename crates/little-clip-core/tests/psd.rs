use little_clip_core::item::{ClipItem, ClipPayload, ImageThumbnail, image_reader};

#[test]
fn saved_composites_decode_across_depths_and_compressions() {
    for bytes in [
        include_bytes!("fixtures/layers.psd").as_slice(),
        include_bytes!("fixtures/composite-8-1.psd"),
        include_bytes!("fixtures/composite-8-2.psd"),
        include_bytes!("fixtures/composite-8-3.psd"),
        include_bytes!("fixtures/composite-16-0.psd"),
        include_bytes!("fixtures/composite-16-3.psd"),
        include_bytes!("fixtures/composite-32-0.psd"),
        include_bytes!("fixtures/composite-32-3.psd"),
    ] {
        assert_eq!(
            little_clip_core::item::read_image_dimensions(bytes),
            Some((64, 32))
        );
        let image = image_reader(bytes, Some("image/png"))
            .unwrap()
            .decode()
            .unwrap()
            .to_rgba8();
        assert_eq!(image.get_pixel(0, 0).0, [30, 120, 210, 255]);
        assert_eq!(image.get_pixel(63, 31).0, [180, 60, 40, 255]);
        let thumb = ImageThumbnail::generate(bytes, 16, 16).unwrap();
        assert_eq!((thumb.width, thumb.height), (16, 8));
        for mime in ["image/vnd.adobe.photoshop", "image/x-photoshop"] {
            let ClipPayload::Image {
                path,
                width,
                height,
                ..
            } = ClipItem::from_image_bytes(bytes.to_vec(), mime)
                .unwrap()
                .payload
            else {
                panic!("PSD payload")
            };
            assert_eq!((width, height), (64, 32));
            assert_eq!(path.unwrap().extension().unwrap(), "psd");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.PSD");
    std::fs::write(&path, include_bytes!("fixtures/layers.psd")).unwrap();
    assert!(little_clip_core::item::is_image_path(&path));
    assert_eq!(
        little_clip_core::item::guess_image_mime(&path),
        "image/vnd.adobe.photoshop"
    );
    assert!(matches!(
        ClipItem::from_text(path.to_str().unwrap()).payload,
        ClipPayload::Image {
            width: 64,
            height: 32,
            ..
        }
    ));
}

#[test]
fn embedded_jpeg_preview_does_not_require_full_canvas_allocation() {
    let bytes = include_bytes!("fixtures/thumbnail.psd");
    let jpeg = little_clip_core::psd_image::embedded_thumbnail(bytes).unwrap();
    assert!(jpeg.starts_with(b"\xff\xd8"));
    let mut limits = little_clip_core::item::raster_preview_limits();
    limits.max_alloc = Some(8192);
    let mut reader = image_reader(bytes, None).unwrap();
    reader.limits(limits.clone());
    assert!(reader.decode().is_err());
    let preview = ImageThumbnail::generate_with_limits(bytes, None, 16, 16, limits).unwrap();
    assert_eq!((preview.width, preview.height), (16, 8));
    let mut invalid = bytes.to_vec();
    invalid[30..34].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(little_clip_core::psd_image::embedded_thumbnail(&invalid).is_none());
}

#[test]
fn source_layer_metadata_keeps_groups_unicode_masks_and_off_canvas_bounds() {
    let summary =
        little_clip_core::psd_image::inspect(include_bytes!("fixtures/layers.psd")).unwrap();
    assert_eq!(summary.dimensions, (64, 32));
    assert_eq!(summary.depth, Some(8));
    assert_eq!(summary.layers.len(), 4);
    assert!(summary.layers[0].group);
    assert_eq!(summary.layers[0].name, "Artwork");
    let top = &summary.layers[1];
    assert_eq!(top.name, "Top patch");
    assert_eq!(top.depth, 1);
    assert_eq!(top.bounds, Some([0, 32, 32, 64]));
    assert!(top.mask);
    assert_eq!(top.blend_name(), "Normal");
    let hidden = &summary.layers[2];
    assert_eq!(hidden.name, "Hidden tint α");
    assert_eq!(hidden.visible, Some(false));
    assert_eq!(hidden.opacity, Some(128));
    assert_eq!(hidden.blend_mode, "mul ");
    assert_eq!(hidden.blend_name(), "Multiply");
    assert_eq!(hidden.bounds, Some([-4, -8, 36, 72]));
}

#[test]
fn decoder_limits_invalid_headers_and_missing_composites_fail_explicitly() {
    let original = include_bytes!("fixtures/layers.psd");
    for allocation in [false, true] {
        let mut reader = image_reader(original, None).unwrap();
        let mut limits = little_clip_core::item::raster_preview_limits();
        if allocation {
            limits.max_alloc = Some(8);
        } else {
            limits.max_image_width = Some(63);
        }
        reader.limits(limits);
        assert!(reader.decode().is_err());
    }
    let mut no_composite = original[..26].to_vec();
    no_composite.extend_from_slice(&[0; 12]);
    assert!(
        image_reader(&no_composite, None)
            .unwrap()
            .decode()
            .unwrap_err()
            .to_string()
            .contains("saved composite")
    );
    for (offset, values) in [
        (4, vec![0, 2]),
        (18, 30_000u32.to_be_bytes().to_vec()),
        (22, vec![0, 7]),
    ] {
        let mut invalid = original.to_vec();
        invalid[offset..offset + values.len()].copy_from_slice(&values);
        assert!(image_reader(&invalid, None).unwrap().decode().is_err());
    }
    assert!(
        ClipItem::from_image_bytes(b"8BPS\0\x01".to_vec(), "image/vnd.adobe.photoshop").is_none()
    );
    assert!(
        ClipItem::from_image_bytes(
            include_bytes!("fixtures/panels.jxl").to_vec(),
            "image/vnd.adobe.photoshop"
        )
        .is_none()
    );
}
