//! Barcode, QR Code, and Data Matrix recognition and decoding suite.
//!
//! Uses pure Rust `rxing` for high-performance multi-barcode detection and decoding,
//! with fallback to `zbarimg` if installed on the system.

use image::{DynamicImage, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BarcodeDetection {
    /// Human-friendly format label (e.g. "QR Code", "Data Matrix", "Code 128", "EAN-13")
    pub format: String,
    /// Raw decoded text or binary content represented as UTF-8
    pub text: String,
    /// Normalized polygon bounding points in [0.0, 1.0] image space: (x, y)
    pub points: Vec<[f32; 2]>,
    /// Minimum axis-aligned bounding box in normalized [0.0, 1.0] image space: [min_x, min_y, max_x, max_y]
    pub bbox: [f32; 4],
    /// True if the decoded string is a recognized URL (http://, https://, etc.)
    pub is_url: bool,
}

/// Detects and decodes all barcodes, QR codes, and DataMatrix symbols present in an image.
pub fn detect_barcodes(img: &RgbaImage) -> Vec<BarcodeDetection> {
    let mut results = Vec::new();
    let width = img.width();
    let height = img.height();

    if width == 0 || height == 0 {
        return results;
    }

    let dyn_img = DynamicImage::ImageRgba8(img.clone());

    // 1. Try rxing pure Rust multi-barcode reader
    if let Ok(rxing_results) = rxing_detect(&dyn_img, width, height) {
        for res in rxing_results {
            if !results.iter().any(|existing: &BarcodeDetection| existing.text == res.text && existing.format == res.format) {
                results.push(res);
            }
        }
    }

    // 2. If rxing found nothing or for additional detection, try zbarimg CLI if available
    if results.is_empty() {
        if let Ok(zbar_results) = zbar_detect(img) {
            for res in zbar_results {
                if !results.iter().any(|existing: &BarcodeDetection| existing.text == res.text) {
                    results.push(res);
                }
            }
        }
    }

    results
}

fn rxing_detect(dyn_img: &DynamicImage, width: u32, height: u32) -> Result<Vec<BarcodeDetection>, String> {
    use rxing::common::HybridBinarizer;
    use rxing::multi::{GenericMultipleBarcodeReader, MultipleBarcodeReader};
    use rxing::{BarcodeFormat, BinaryBitmap, BufferedImageLuminanceSource, DecodeHints, MultiFormatReader, Reader};
    use std::collections::HashSet;

    let lum_source = BufferedImageLuminanceSource::new(dyn_img.clone());
    let mut bitmap = BinaryBitmap::new(HybridBinarizer::new(lum_source));

    let mut hints = DecodeHints::default();
    hints.TryHarder = Some(true);

    let mut formats = HashSet::new();
    formats.insert(BarcodeFormat::QR_CODE);
    formats.insert(BarcodeFormat::DATA_MATRIX);
    formats.insert(BarcodeFormat::AZTEC);
    formats.insert(BarcodeFormat::PDF_417);
    formats.insert(BarcodeFormat::CODE_128);
    formats.insert(BarcodeFormat::CODE_39);
    formats.insert(BarcodeFormat::CODE_93);
    formats.insert(BarcodeFormat::EAN_13);
    formats.insert(BarcodeFormat::EAN_8);
    formats.insert(BarcodeFormat::UPC_A);
    formats.insert(BarcodeFormat::UPC_E);
    formats.insert(BarcodeFormat::ITF);
    formats.insert(BarcodeFormat::CODABAR);
    hints.PossibleFormats = Some(formats);

    let multi_reader = MultiFormatReader::default();
    let mut generic_reader = GenericMultipleBarcodeReader::new(multi_reader);

    let decoded_list = generic_reader
        .decode_multiple_with_hints(&mut bitmap, &hints)
        .or_else(|_| {
            let mut single_reader = MultiFormatReader::default();
            single_reader.decode_with_hints(&mut bitmap, &hints).map(|r| vec![r])
        })
        .map_err(|e| format!("rxing decoding error: {:?}", e))?;

    let w_f = width as f32;
    let h_f = height as f32;

    let mut detections = Vec::new();
    for item in decoded_list {
        let fmt_str = format_name(item.getBarcodeFormat());
        let text = item.getText().to_string();
        let is_url = text.starts_with("http://")
            || text.starts_with("https://")
            || text.starts_with("ftp://")
            || text.starts_with("www.");

        let mut pts = Vec::new();
        let mut min_x = 1.0f32;
        let mut min_y = 1.0f32;
        let mut max_x = 0.0f32;
        let mut max_y = 0.0f32;

        for pt in item.getPoints() {
            let nx = (pt.x / w_f).clamp(0.0, 1.0);
            let ny = (pt.y / h_f).clamp(0.0, 1.0);
            min_x = min_x.min(nx);
            min_y = min_y.min(ny);
            max_x = max_x.max(nx);
            max_y = max_y.max(ny);
            pts.push([nx, ny]);
        }

        if min_x > max_x {
            min_x = 0.0;
            max_x = 1.0;
            min_y = 0.0;
            max_y = 1.0;
        }

        // Pad bbox slightly if degenerate
        if max_x - min_x < 0.02 {
            let cx = (min_x + max_x) / 2.0;
            min_x = (cx - 0.02).clamp(0.0, 1.0);
            max_x = (cx + 0.02).clamp(0.0, 1.0);
        }
        if max_y - min_y < 0.02 {
            let cy = (min_y + max_y) / 2.0;
            min_y = (cy - 0.02).clamp(0.0, 1.0);
            max_y = (cy + 0.02).clamp(0.0, 1.0);
        }

        detections.push(BarcodeDetection {
            format: fmt_str,
            text,
            points: pts,
            bbox: [min_x, min_y, max_x, max_y],
            is_url,
        });
    }

    Ok(detections)
}

fn format_name(fmt: &rxing::BarcodeFormat) -> String {
    match fmt {
        rxing::BarcodeFormat::QR_CODE => "QR Code".to_string(),
        rxing::BarcodeFormat::DATA_MATRIX => "Data Matrix".to_string(),
        rxing::BarcodeFormat::AZTEC => "Aztec".to_string(),
        rxing::BarcodeFormat::PDF_417 => "PDF417".to_string(),
        rxing::BarcodeFormat::CODE_128 => "Code 128".to_string(),
        rxing::BarcodeFormat::CODE_39 => "Code 39".to_string(),
        rxing::BarcodeFormat::CODE_93 => "Code 93".to_string(),
        rxing::BarcodeFormat::EAN_13 => "EAN-13".to_string(),
        rxing::BarcodeFormat::EAN_8 => "EAN-8".to_string(),
        rxing::BarcodeFormat::UPC_A => "UPC-A".to_string(),
        rxing::BarcodeFormat::UPC_E => "UPC-E".to_string(),
        rxing::BarcodeFormat::ITF => "ITF (Interleaved 2 of 5)".to_string(),
        rxing::BarcodeFormat::CODABAR => "Codabar".to_string(),
        rxing::BarcodeFormat::MAXICODE => "MaxiCode".to_string(),
        _ => format!("{:?}", fmt),
    }
}

/// Fallback detection via `zbarimg` CLI if installed.
fn zbar_detect(img: &RgbaImage) -> Result<Vec<BarcodeDetection>, String> {
    use std::io::Write;
    use std::process::Command;

    let mut png_data = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_data),
        image::ImageFormat::Png,
    )
    .map_err(|e| e.to_string())?;

    let mut child = Command::new("zbarimg")
        .arg("-q")
        .arg("--raw")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("zbarimg spawn failed: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&png_data);
    }

    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() && output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let is_url = text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("www.");

    Ok(vec![BarcodeDetection {
        format: "Barcode / QR (zbar)".to_string(),
        text,
        points: Vec::new(),
        bbox: [0.05, 0.05, 0.95, 0.95],
        is_url,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_image_barcode() {
        let img = RgbaImage::new(0, 0);
        let results = detect_barcodes(&img);
        assert!(results.is_empty());
    }

    #[test]
    fn test_qr_code_detection() {
        use rxing::qrcode::QRCodeWriter;
        use rxing::Writer;
        let writer = QRCodeWriter {};
        let bit_matrix = writer
            .encode("https://example.com/little-clip", &rxing::BarcodeFormat::QR_CODE, 100, 100)
            .expect("Failed to encode test QR");

        let w = bit_matrix.getWidth();
        let h = bit_matrix.getHeight();
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let col = if bit_matrix.get(x, y) {
                    image::Rgba([0, 0, 0, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                };
                img.put_pixel(x, y, col);
            }
        }

        let results = detect_barcodes(&img);
        assert!(!results.is_empty(), "Expected at least 1 QR code detection");
        assert_eq!(results[0].text, "https://example.com/little-clip");
        assert!(results[0].is_url);
    }

    #[test]
    fn test_datamatrix_detection() {
        use rxing::datamatrix::DataMatrixWriter;
        use rxing::Writer;
        let writer = DataMatrixWriter {};
        let bit_matrix = writer
            .encode("DATAMATRIX-TEST-12345", &rxing::BarcodeFormat::DATA_MATRIX, 64, 64)
            .expect("Failed to encode test DataMatrix");

        let w = bit_matrix.getWidth();
        let h = bit_matrix.getHeight();
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let col = if bit_matrix.get(x, y) {
                    image::Rgba([0, 0, 0, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                };
                img.put_pixel(x, y, col);
            }
        }

        let results = detect_barcodes(&img);
        assert!(!results.is_empty(), "Expected at least 1 DataMatrix detection");
        assert_eq!(results[0].text, "DATAMATRIX-TEST-12345");
        assert_eq!(results[0].format, "Data Matrix");
    }
}
