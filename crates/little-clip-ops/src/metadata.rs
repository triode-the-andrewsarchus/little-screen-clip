//! EXIF and image metadata inspection, extraction, and sanitization.
//!
//! Provides extraction of camera settings, lens details, timestamps, and GPS coordinates,
//! as well as secure metadata stripping for privacy before sharing or redacting.

use image::{ImageFormat, RgbaImage};
use std::io::Cursor;

/// Extracted metadata summary from an image file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageMetadataSummary {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub date_time: Option<String>,
    pub iso: Option<u32>,
    pub f_number: Option<f32>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<f32>,
    pub software: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude: Option<f64>,
    pub has_exif: bool,
}

impl ImageMetadataSummary {
    /// Check if any meaningful metadata fields are populated.
    pub fn has_any_metadata(&self) -> bool {
        self.has_exif
            || self.make.is_some()
            || self.model.is_some()
            || self.date_time.is_some()
            || self.has_gps()
            || self.software.is_some()
    }

    /// Check if valid GPS coordinates are present.
    pub fn has_gps(&self) -> bool {
        self.gps_latitude.is_some() && self.gps_longitude.is_some()
    }

    /// Human-readable GPS coordinates string, e.g. "37.7749° N, 122.4194° W".
    pub fn gps_display(&self) -> Option<String> {
        let lat = self.gps_latitude?;
        let lon = self.gps_longitude?;
        let lat_dir = if lat >= 0.0 { 'N' } else { 'S' };
        let lon_dir = if lon >= 0.0 { 'E' } else { 'W' };
        Some(format!("{:.4}° {}, {:.4}° {}", lat.abs(), lat_dir, lon.abs(), lon_dir))
    }

    /// URL to view location on OpenStreetMap.
    pub fn osm_url(&self) -> Option<String> {
        let lat = self.gps_latitude?;
        let lon = self.gps_longitude?;
        Some(format!("https://www.openstreetmap.org/?mlat={:.6}&mlon={:.6}&zoom=16", lat, lon))
    }

    /// Exposure summary string, e.g. "f/2.8 · 1/250s · ISO 100 · 50mm".
    pub fn exposure_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(f) = self.f_number {
            parts.push(format!("f/{:.1}", f));
        }
        if let Some(exp) = &self.exposure_time {
            parts.push(exp.clone());
        }
        if let Some(iso) = self.iso {
            parts.push(format!("ISO {}", iso));
        }
        if let Some(fl) = self.focal_length {
            parts.push(format!("{:.0}mm", fl));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    /// Camera & lens summary line, e.g. "Sony A7IV · FE 24-70mm F2.8 GM II".
    pub fn camera_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(m) = &self.model {
            parts.push(m.clone());
        } else if let Some(make) = &self.make {
            parts.push(make.clone());
        }
        if let Some(lens) = &self.lens_model {
            parts.push(lens.clone());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }
}

/// Extract EXIF metadata from raw image bytes (JPEG, PNG, WebP, TIFF).
pub fn extract_image_metadata(bytes: &[u8]) -> ImageMetadataSummary {
    let mut cursor = Cursor::new(bytes);
    let exif_reader = exif::Reader::new();

    let exif_data = match exif_reader.read_from_container(&mut cursor) {
        Ok(data) => data,
        Err(_) => {
            // Also try reading raw EXIF dump
            match exif_reader.read_raw(bytes.to_vec()) {
                Ok(data) => data,
                Err(_) => return ImageMetadataSummary::default(),
            }
        }
    };

    let mut summary = ImageMetadataSummary {
        has_exif: true,
        ..Default::default()
    };

    for field in exif_data.fields() {
        match field.tag {
            exif::Tag::Make => {
                summary.make = Some(field.display_value().to_string().trim_matches('"').to_string());
            }
            exif::Tag::Model => {
                summary.model = Some(field.display_value().to_string().trim_matches('"').to_string());
            }
            exif::Tag::LensModel => {
                summary.lens_model = Some(field.display_value().to_string().trim_matches('"').to_string());
            }
            exif::Tag::DateTimeOriginal | exif::Tag::DateTime => {
                if summary.date_time.is_none() {
                    summary.date_time = Some(field.display_value().to_string().trim_matches('"').to_string());
                }
            }
            exif::Tag::PhotographicSensitivity => {
                if let Some(val) = field.value.get_uint(0) {
                    summary.iso = Some(val);
                }
            }
            exif::Tag::FNumber => {
                if let exif::Value::Rational(ref rats) = field.value {
                    if let Some(r) = rats.first() {
                        if r.denom != 0 {
                            summary.f_number = Some(r.num as f32 / r.denom as f32);
                        }
                    }
                }
            }
            exif::Tag::ExposureTime => {
                summary.exposure_time = Some(field.display_value().to_string());
            }
            exif::Tag::FocalLength => {
                if let exif::Value::Rational(ref rats) = field.value {
                    if let Some(r) = rats.first() {
                        if r.denom != 0 {
                            summary.focal_length = Some(r.num as f32 / r.denom as f32);
                        }
                    }
                }
            }
            exif::Tag::Software => {
                summary.software = Some(field.display_value().to_string().trim_matches('"').to_string());
            }
            _ => {}
        }
    }

    // Parse GPS latitude and longitude
    summary.gps_latitude = parse_gps_coord(&exif_data, exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef);
    summary.gps_longitude = parse_gps_coord(&exif_data, exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef);
    if let Some(alt_field) = exif_data.get_field(exif::Tag::GPSAltitude, exif::In::PRIMARY) {
        if let exif::Value::Rational(ref rats) = alt_field.value {
            if let Some(r) = rats.first() {
                if r.denom != 0 {
                    let mut alt = r.num as f64 / r.denom as f64;
                    if let Some(ref_field) = exif_data.get_field(exif::Tag::GPSAltitudeRef, exif::In::PRIMARY) {
                        if let Some(1) = ref_field.value.get_uint(0) {
                            alt = -alt; // Below sea level
                        }
                    }
                    summary.gps_altitude = Some(alt);
                }
            }
        }
    }

    summary
}

fn parse_gps_coord(exif: &exif::Exif, coord_tag: exif::Tag, ref_tag: exif::Tag) -> Option<f64> {
    let field = exif.get_field(coord_tag, exif::In::PRIMARY)?;
    let ref_field = exif.get_field(ref_tag, exif::In::PRIMARY);

    let (d, m, s) = match field.value {
        exif::Value::Rational(ref rats) if rats.len() >= 3 => {
            let deg = if rats[0].denom != 0 { rats[0].num as f64 / rats[0].denom as f64 } else { 0.0 };
            let min = if rats[1].denom != 0 { rats[1].num as f64 / rats[1].denom as f64 } else { 0.0 };
            let sec = if rats[2].denom != 0 { rats[2].num as f64 / rats[2].denom as f64 } else { 0.0 };
            (deg, min, sec)
        }
        _ => return None,
    };

    let mut decimal = d + (m / 60.0) + (s / 3600.0);

    if let Some(rf) = ref_field {
        let ref_str = rf.display_value().to_string();
        if ref_str.starts_with('S') || ref_str.starts_with('s') || ref_str.starts_with('W') || ref_str.starts_with('w') {
            decimal = -decimal;
        }
    }

    Some(decimal)
}

/// Strip all EXIF, GPS, device, and auxiliary metadata by re-encoding pristine pixels into PNG or JPEG.
pub fn strip_metadata(img: &RgbaImage, format_hint: &str) -> Result<Vec<u8>, String> {
    let mut clean_bytes = Vec::new();
    let is_jpeg = format_hint.eq_ignore_ascii_case("jpg") || format_hint.eq_ignore_ascii_case("jpeg") || format_hint.eq_ignore_ascii_case("image/jpeg");

    if is_jpeg {
        let mut rgb = image::RgbImage::new(img.width(), img.height());
        for (x, y, pixel) in img.enumerate_pixels() {
            let alpha = pixel[3] as f32 / 255.0;
            let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
            let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
            let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
            rgb.put_pixel(x, y, image::Rgb([r, g, b]));
        }
        rgb.write_to(&mut Cursor::new(&mut clean_bytes), ImageFormat::Jpeg)
            .map_err(|e| format!("Failed to encode clean JPEG: {}", e))?;
    } else {
        img.write_to(&mut Cursor::new(&mut clean_bytes), ImageFormat::Png)
            .map_err(|e| format!("Failed to encode clean PNG: {}", e))?;
    }

    Ok(clean_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_metadata() {
        let clean_img = RgbaImage::new(10, 10);
        let mut bytes = Vec::new();
        clean_img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png).unwrap();

        let summary = extract_image_metadata(&bytes);
        assert!(!summary.has_any_metadata());
        assert!(!summary.has_gps());
        assert!(summary.camera_summary().is_none());
        assert!(summary.exposure_summary().is_none());
    }

    #[test]
    fn test_gps_display_and_osm() {
        let summary = ImageMetadataSummary {
            has_exif: true,
            gps_latitude: Some(37.7749),
            gps_longitude: Some(-122.4194),
            ..Default::default()
        };

        assert!(summary.has_gps());
        let disp = summary.gps_display().unwrap();
        assert!(disp.contains("37.7749° N"));
        assert!(disp.contains("122.4194° W"));

        let osm = summary.osm_url().unwrap();
        assert!(osm.starts_with("https://www.openstreetmap.org/?mlat=37.7749"));
    }

    #[test]
    fn test_strip_metadata_produces_clean_bytes() {
        let mut img = RgbaImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = image::Rgba([120, 150, 200, 255]);
        }

        let clean_png = strip_metadata(&img, "png").unwrap();
        assert!(!clean_png.is_empty());
        let summary = extract_image_metadata(&clean_png);
        assert!(!summary.has_any_metadata());

        let clean_jpg = strip_metadata(&img, "jpg").unwrap();
        assert!(!clean_jpg.is_empty());
        let summary_jpg = extract_image_metadata(&clean_jpg);
        assert!(!summary_jpg.has_any_metadata());
    }
}
