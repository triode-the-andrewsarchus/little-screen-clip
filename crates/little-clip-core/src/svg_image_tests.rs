use super::*;
use image::GenericImageView;
use std::{io::Write, sync::atomic::AtomicBool};
const SVG: &[u8] = br##"<svg width="64" height="32" xmlns="http://www.w3.org/2000/svg"><rect width="32" height="32" fill="#1e78d2"/><rect x="32" width="32" height="32" fill="#b43c28"/></svg>"##;
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap()
}
fn renderer_available() -> bool {
    std::process::Command::new("rsvg-convert")
        .arg("--version")
        .output()
        .is_ok()
}
#[test]
fn svgz_document_and_header_preserve_plain_markup_and_check_complete_gzip() {
    assert!(matches!(
        document(SVG).unwrap(),
        std::borrow::Cow::Borrowed(_)
    ));
    let compressed = gzip(SVG);
    assert_eq!(document(&compressed).unwrap().as_ref(), SVG);
    assert_eq!(dimensions(&compressed), Some((64, 32)));
    let long = [SVG.to_vec(), "é".repeat(100_000).into_bytes()].concat();
    let mut partial = gzip(&long);
    partial.truncate(partial.len() - 8);
    assert_eq!(dimensions(&partial), Some((64, 32))); // Bounded header, no full CRC claim.
    assert!(document(&partial).is_err());
    assert!(document(&gzip(b"plain text")).is_err());
    assert!(document(&gzip(b"<html><svg/></html>")).is_err());
    let mut corrupt = compressed;
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(document(&corrupt).is_err());
}
#[test]
fn svgz_expansion_and_encoded_documents_have_separate_limits() {
    let bomb = gzip(&vec![b' '; MAX_DOCUMENT_BYTES + 1]);
    assert!(document(&bomb).unwrap_err().contains("Expanded SVG"));
    assert!(
        document(&vec![b' '; MAX_DOCUMENT_BYTES + 1])
            .unwrap_err()
            .contains("document exceeds")
    );
}
#[test]
fn svg_headers_handle_namespaces_preambles_units_and_declared_size_before_viewbox() {
    let source = format!(
        "<?xml version=\"1.0\"?><!--{}--><s:svg xmlns:s=\"http://www.w3.org/2000/svg\" width=\"2in\" height=\"1in\" viewBox=\"0 0 64 32\"/>",
        "license ".repeat(1000)
    );
    assert!(is_markup(source.as_bytes()));
    assert_eq!(dimensions(source.as_bytes()), Some((192, 96)));
    assert_eq!(
        dimensions(br#"<svg width="80" viewBox="0,0,64,32"/>"#),
        Some((80, 40))
    );
    assert_eq!(
        dimensions(br#"<svg width="100%" height="100%" viewBox="0 0 64 32"/>"#),
        Some((64, 32))
    );
    assert_eq!(dimensions(br#"<svg width="NaN" height="inf"/>"#), None);
}
#[test]
fn svgz_rasters_are_bounded_cancellable_and_do_not_load_external_files() {
    if !renderer_available() {
        return;
    }
    let png = render_png(&gzip(SVG), Some((16, 16)), &AtomicBool::new(false)).unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (16, 8));
    assert_eq!(image.get_pixel(0, 0).0, [30, 120, 210, 255]);
    assert!(render_png(SVG, None, &AtomicBool::new(true)).is_err());
    assert!(render_png(SVG, Some((0, 16)), &AtomicBool::new(false)).is_err());
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.png");
    image::RgbaImage::from_pixel(32, 16, image::Rgba([255, 0, 0, 255]))
        .save(&secret)
        .unwrap();
    let source = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"32\" height=\"16\"><image width=\"32\" height=\"16\" xlink:href=\"file://{}\"/><rect width=\"4\" height=\"4\" fill=\"blue\"/></svg>",
        secret.display()
    );
    let png = render_png(&gzip(source.as_bytes()), None, &AtomicBool::new(false)).unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(image.get_pixel(1, 1).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(20, 8).0, [0, 0, 0, 0]);
}
#[test]
fn svgz_embedded_raster_output_can_exceed_the_old_command_capture_limit() {
    if !renderer_available() {
        return;
    }
    use base64::Engine;
    let mut seed = 5_u32;
    let image = image::RgbaImage::from_fn(512, 512, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let [r, g, b, _] = seed.to_le_bytes();
        image::Rgba([r, g, b, 255])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let source = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"512\" height=\"512\"><image width=\"512\" height=\"512\" xlink:href=\"data:image/png;base64,{encoded}\"/></svg>"
    );
    let output = render_png(&gzip(source.as_bytes()), None, &AtomicBool::new(false)).unwrap();
    assert!(output.len() > 256 * 1024);
    assert_eq!(
        image::load_from_memory(&output).unwrap().dimensions(),
        (512, 512)
    );
}

#[test]
fn svg_render_sizes_fit_the_shared_raster_allocation_and_preserve_aspect() {
    let source = br#"<svg width="1000000000" height="1000000000"/>"#;
    let (width, height) = render_size(source, None).unwrap();
    let limits = crate::item::raster_preview_limits();
    assert!(width <= limits.max_image_width.unwrap());
    assert!(height <= limits.max_image_height.unwrap());
    assert!(u64::from(width) * u64::from(height) * 4 <= limits.max_alloc.unwrap());
    assert_eq!(render_size(SVG, Some((16, 4))).unwrap(), (8, 4));
}
