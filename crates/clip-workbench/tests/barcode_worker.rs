// This integration test must launch Cargo's actual binary target to cover the
// hidden helper-mode dispatch in `main`; unit tests live beside the worker.
use image::RgbaImage;
use little_clip_ops::barcode::{BarcodeDetection, detect_barcodes};
use rxing::Writer;

#[test]
fn process_helper_matches_direct_barcode_detection() {
    let bits = rxing::qrcode::QRCodeWriter
        .encode("clipboard 日本語 ✓", &rxing::BarcodeFormat::QR_CODE, 96, 96)
        .unwrap();
    let image = RgbaImage::from_fn(96, 96, |x, y| {
        if bits.get(x, y) {
            image::Rgba([0, 0, 0, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.rgba");
    let output = directory.path().join("output.json");
    let mut bytes = Vec::with_capacity(8 + image.as_raw().len());
    bytes.extend_from_slice(&image.width().to_le_bytes());
    bytes.extend_from_slice(&image.height().to_le_bytes());
    bytes.extend_from_slice(image.as_raw());
    std::fs::write(&input, bytes).unwrap();

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_clip-workbench"))
        .arg("--internal-barcode-worker")
        .arg(&input)
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    let actual: Vec<BarcodeDetection> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(actual, detect_barcodes(&image));
    assert_eq!(actual[0].text, "clipboard 日本語 ✓");
}
