//! Camera RAW format detection and embedded JPEG preview extraction.
//!
//! Uses `exiftool` to rapidly extract embedded full-resolution JPEG previews from camera RAW files
//! (`.CR2`, `.NEF`, `.ARW`, `.DNG`, `.RAF`, etc.) with fallback to ImageMagick.

use std::path::Path;
use std::process::Command;

/// Returns `true` if the path has a known camera RAW file extension.
pub fn is_raw_image_path(p: &Path) -> bool {
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
            "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" | "pef" | "srw" | "srf"
        )
    } else {
        false
    }
}

/// Extracts embedded JPEG preview bytes and dimensions from a camera RAW image file.
pub fn extract_raw_image_preview(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    // 1. Try exiftool embedded preview extraction (near-instant)
    let exiftool_tags = ["-PreviewImage", "-JpgFromRaw", "-ThumbnailImage"];
    for tag in exiftool_tags {
        let out = Command::new("exiftool")
            .arg("-b")
            .arg(tag)
            .arg(path)
            .output();

        if let Ok(output) = out {
            if output.status.success() && output.stdout.len() > 1024 {
                if let Ok(img) = image::load_from_memory(&output.stdout) {
                    return Ok((output.stdout, img.width(), img.height()));
                }
            }
        }
    }

    // 2. Fallback to ImageMagick
    let out = Command::new("magick")
        .arg(format!("{}[0]", path.to_string_lossy()))
        .arg("-resize")
        .arg("1600x1200>")
        .arg("jpeg:-")
        .output();

    if let Ok(output) = out {
        if output.status.success() && output.stdout.len() > 1024 {
            if let Ok(img) = image::load_from_memory(&output.stdout) {
                return Ok((output.stdout, img.width(), img.height()));
            }
        }
    }

    Err(format!("Could not extract camera RAW preview from {:?}", path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_raw_image_path() {
        assert!(is_raw_image_path(Path::new("photo.CR2")));
        assert!(is_raw_image_path(Path::new("/path/to/image.NEF")));
        assert!(is_raw_image_path(Path::new("sony.arw")));
        assert!(is_raw_image_path(Path::new("fuji.RAF")));
        assert!(is_raw_image_path(Path::new("adobe.dng")));
        assert!(is_raw_image_path(Path::new("sensor.raw")));
        assert!(!is_raw_image_path(Path::new("photo.png")));
        assert!(!is_raw_image_path(Path::new("document.pdf")));
    }
}
