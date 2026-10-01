use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use little_clip_ops::{
    image_diff::{
        ImageDiffOptions, compare_image_bytes_with_mime, load_rgba_from_bytes, load_rgba_from_path,
    },
    metadata::extract_image_metadata,
};
use std::io::Cursor;

#[test]
fn heic_exif_and_pixels_use_existing_metadata_and_comparison_paths() {
    let bytes = include_bytes!("../../little-clip-core/tests/fixtures/gradient.heic");
    let metadata = extract_image_metadata(bytes);
    assert!(metadata.has_exif);
    assert_eq!(metadata.make.as_deref(), Some("LittleScreenClip"));
    assert_eq!(metadata.model.as_deref(), Some("CodecFixture"));
    assert_eq!(metadata.date_time.as_deref(), Some("2026-09-30 12:00:00"));
    assert_eq!(load_rgba_from_bytes(bytes).unwrap().dimensions(), (8, 6));
    let result = compare_image_bytes_with_mime(
        bytes,
        Some("image/heic"),
        bytes,
        Some("image/heic"),
        &ImageDiffOptions::default(),
    )
    .unwrap();
    assert!(result.is_identical);
}

#[test]
fn tga_file_and_comparison_respect_format_hint() {
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([128, 64, 32])));
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, ImageFormat::Tga).unwrap();
    let bytes = encoded.into_inner();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.TGA");
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(load_rgba_from_path(&path).unwrap().dimensions(), (8, 6));
    let result = compare_image_bytes_with_mime(
        &bytes,
        Some("image/x-targa"),
        &bytes,
        Some("image/x-targa"),
        &ImageDiffOptions::default(),
    )
    .unwrap();
    assert!(result.is_identical);
}

#[test]
fn heic_pipeline_roundtrip_keeps_extension_mime_pixels_and_exif() {
    use little_clip_core::{ClipItem, ClipPayload};
    use little_clip_ops::pipeline::PortValue;
    let bytes = include_bytes!("../../little-clip-core/tests/fixtures/gradient.heic").to_vec();
    let mut input = ClipItem::from_image(bytes.clone(), 8, 6, "image/heic");
    if let ClipPayload::Image { path, .. } = &mut input.payload {
        assert_eq!(path.as_ref().unwrap().extension().unwrap(), "heic");
        *path = None;
    }
    let port = PortValue::from_clip_item(&input).unwrap();
    assert_eq!(port.as_path().unwrap().extension().unwrap(), "heic");
    for port in [
        port.clone(),
        PortValue::FilePath(port.as_path().unwrap().to_owned()),
    ] {
        let ClipPayload::Image {
            bytes: actual,
            mime,
            width,
            height,
            ..
        } = port.into_clip_item("HEIC").payload
        else {
            panic!("image port lost its type")
        };
        assert_eq!(actual, bytes);
        assert_eq!(mime, "image/heic");
        assert_eq!((width, height), (8, 6));
        assert!(extract_image_metadata(&actual).has_exif);
    }
}
