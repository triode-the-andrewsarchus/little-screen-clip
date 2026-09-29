use image::{ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;
use std::process::Command;

#[test]
fn helper_matches_image_decoder_for_png_jpeg_and_webp() {
    let source = RgbaImage::from_fn(37, 19, |x, y| {
        Rgba([
            ((x * 17 + y * 3) % 256) as u8,
            ((x * 5 + y * 19) % 256) as u8,
            ((x * 13 + y * 7) % 256) as u8,
            ((x * 11 + y * 23) % 256) as u8,
        ])
    });
    let mut png = Vec::new();
    source
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    let mut jpeg = Vec::new();
    let rgb = image::DynamicImage::ImageRgba8(source.clone()).to_rgb8();
    rgb.write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
        .unwrap();
    let webp = webp::Encoder::from_rgba(source.as_raw(), source.width(), source.height())
        .encode(80.0)
        .to_vec();
    let webp_lossless = webp::Encoder::from_rgba(source.as_raw(), source.width(), source.height())
        .encode_lossless()
        .to_vec();

    for (name, encoded) in [
        ("png", png),
        ("jpeg", jpeg),
        ("webp", webp),
        ("webp-lossless", webp_lossless),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join(format!("input.{name}"));
        let output = directory.path().join("output.rgba");
        std::fs::write(&input, &encoded).unwrap();
        let status = Command::new(env!("CARGO_BIN_EXE_clip-workbench"))
            .arg("--internal-conversion-decode-worker")
            .arg(&input)
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success(), "{name} helper failed: {status}");
        let actual = std::fs::read(output).unwrap();
        let expected = image::load_from_memory(&encoded).unwrap().to_rgba8();
        assert_eq!(actual[0], 1, "{name}");
        assert_eq!(
            u32::from_le_bytes(actual[1..5].try_into().unwrap()),
            expected.width()
        );
        assert_eq!(
            u32::from_le_bytes(actual[5..9].try_into().unwrap()),
            expected.height()
        );
        assert_eq!(&actual[9..], expected.as_raw(), "{name} pixels changed");
    }
}

#[test]
fn helper_reports_decode_failure_as_existing_source_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("invalid.image");
    let output = directory.path().join("output.rgba");
    std::fs::write(&input, b"invalid image").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_clip-workbench"))
        .arg("--internal-conversion-decode-worker")
        .arg(&input)
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read(output).unwrap(), [0]);
}
