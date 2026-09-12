//! PDF inspection, preview rendering via `pdftoppm`, page deletion, and rotation.
//!
//! Wraps poppler utilities (`pdfinfo`, `pdftoppm`, `pdfseparate`, `pdfunite`) and `magick`
//! for manipulation of multi-page PDF documents.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Metadata summary extracted from a PDF document.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfInfo {
    /// Total number of pages.
    pub page_count: usize,
    /// Document title if present in metadata.
    pub title: Option<String>,
    /// Document author if present in metadata.
    pub author: Option<String>,
    /// Page size description string (e.g. `"595 x 842 pts (A4)"`).
    pub page_size: Option<String>,
}

/// Returns `true` if the path has a `.pdf` extension (case-insensitive).
pub fn is_pdf_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Runs `pdfinfo` on a file and parses its page count and metadata tags.
pub fn get_pdf_info(path: &Path) -> Result<PdfInfo, String> {
    let output = Command::new("pdfinfo")
        .arg(path)
        .output()
        .map_err(|e| format!("Failed to run pdfinfo: {}", e))?;

    if !output.status.success() {
        return Err("pdfinfo exited with error".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut page_count = 1;
    let mut title = None;
    let mut author = None;
    let mut page_size = None;

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Pages:") {
            page_count = rest.trim().parse().unwrap_or(1);
        } else if let Some(rest) = line.strip_prefix("Title:") {
            let t = rest.trim();
            if !t.is_empty() {
                title = Some(t.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("Author:") {
            let a = rest.trim();
            if !a.is_empty() {
                author = Some(a.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("Page size:") {
            page_size = Some(rest.trim().to_string());
        }
    }

    Ok(PdfInfo {
        page_count,
        title,
        author,
        page_size,
    })
}

/// Renders a single page of a PDF file to PNG bytes
pub fn render_pdf_page_preview(path: &Path, page: usize, max_dim: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let temp_prefix = std::env::temp_dir().join(format!("pdf_preview_{}_{}", uuid::Uuid::new_v4(), page));
    let page_str = page.to_string();

    let output = Command::new("pdftoppm")
        .arg("-png")
        .arg("-f")
        .arg(&page_str)
        .arg("-l")
        .arg(&page_str)
        .arg("-scale-to-x")
        .arg(max_dim.to_string())
        .arg("-scale-to-y")
        .arg("-1")
        .arg(path)
        .arg(&temp_prefix)
        .output()
        .map_err(|e| format!("pdftoppm execution failed: {}", e))?;

    if !output.status.success() {
        return Err("pdftoppm failed to render page".to_string());
    }

    let out_png = PathBuf::from(format!("{}-{}.png", temp_prefix.to_string_lossy(), page));
    if !out_png.is_file() {
        // Sometimes pdftoppm outputs with 0-padded digits or 1 digit
        let _glob_pattern = format!("{}-*.png", temp_prefix.to_string_lossy());
        return Err(format!("Expected output PNG at {:?} but not found", out_png));
    }

    let png_bytes = std::fs::read(&out_png).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&out_png);

    let (w, h) = if let Ok(dyn_img) = image::load_from_memory(&png_bytes) {
        (dyn_img.width(), dyn_img.height())
    } else {
        (max_dim, (max_dim as f32 * 1.414) as u32)
    };

    Ok((png_bytes, w, h))
}

/// Renders thumbnails for all pages of a PDF
pub fn render_all_pdf_page_thumbnails(path: &Path, thumb_w: u32) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let info = get_pdf_info(path)?;
    let mut pages = Vec::new();
    for page_num in 1..=info.page_count {
        if let Ok((bytes, _, _)) = render_pdf_page_preview(path, page_num, thumb_w) {
            pages.push((page_num, bytes));
        }
    }
    Ok(pages)
}

/// Deletes specified pages from a PDF and writes the remaining pages to `output`
pub fn delete_pdf_pages(input: &Path, pages_to_delete: &[usize], output: &Path) -> Result<(), String> {
    let info = get_pdf_info(input)?;
    if info.page_count == 0 {
        return Err("PDF is empty".to_string());
    }

    let temp_dir = std::env::temp_dir().join(format!("pdf_sep_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

    let pattern = temp_dir.join("page-%d.pdf");
    let sep_status = Command::new("pdfseparate")
        .arg(input)
        .arg(pattern.to_string_lossy().to_string())
        .status()
        .map_err(|e| format!("Failed to separate PDF: {}", e))?;

    if !sep_status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err("pdfseparate failed".to_string());
    }

    let mut remaining_files = Vec::new();
    for p in 1..=info.page_count {
        if !pages_to_delete.contains(&p) {
            let page_path = temp_dir.join(format!("page-{}.pdf", p));
            if page_path.is_file() {
                remaining_files.push(page_path);
            }
        }
    }

    if remaining_files.is_empty() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err("Cannot delete all pages from PDF".to_string());
    }

    let mut unite_cmd = Command::new("pdfunite");
    for f in &remaining_files {
        unite_cmd.arg(f);
    }
    unite_cmd.arg(output);

    let unite_status = unite_cmd.status().map_err(|e| format!("pdfunite failed: {}", e))?;
    let _ = std::fs::remove_dir_all(&temp_dir);

    if !unite_status.success() {
        return Err("pdfunite failed to create output file".to_string());
    }

    Ok(())
}

/// Rotates a specific page by specified degrees (90, 180, 270)
pub fn rotate_pdf_page(input: &Path, page_to_rotate: usize, degrees: u32, output: &Path) -> Result<(), String> {
    let info = get_pdf_info(input)?;
    let temp_dir = std::env::temp_dir().join(format!("pdf_rot_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

    let pattern = temp_dir.join("page-%d.pdf");
    let sep_status = Command::new("pdfseparate")
        .arg(input)
        .arg(pattern.to_string_lossy().to_string())
        .status()
        .map_err(|e| format!("Failed to separate PDF: {}", e))?;

    if !sep_status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err("pdfseparate failed".to_string());
    }

    let target_page = temp_dir.join(format!("page-{}.pdf", page_to_rotate));
    if !target_page.is_file() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!("PDF page {} does not exist", page_to_rotate));
    }
    let rotated_page = temp_dir.join(format!("page-{}-rot.pdf", page_to_rotate));
    let rot_status = Command::new("magick")
        .arg(&target_page)
        .arg("-rotate")
        .arg(degrees.to_string())
        .arg(&rotated_page)
        .status()
        .map_err(|e| {
            let _ = std::fs::remove_dir_all(&temp_dir);
            format!("Failed to spawn magick for page rotation: {}", e)
        })?;

    if !rot_status.success() || !rotated_page.is_file() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!("magick failed to rotate page {} by {} degrees", page_to_rotate, degrees));
    }

    if let Err(e) = std::fs::rename(&rotated_page, &target_page) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!("Failed to replace rotated page: {}", e));
    }

    let mut unite_cmd = Command::new("pdfunite");
    for p in 1..=info.page_count {
        let page_path = temp_dir.join(format!("page-{}.pdf", p));
        if page_path.is_file() {
            unite_cmd.arg(page_path);
        }
    }
    unite_cmd.arg(output);

    let unite_status = unite_cmd.status().map_err(|e| format!("pdfunite failed: {}", e))?;
    let _ = std::fs::remove_dir_all(&temp_dir);

    if !unite_status.success() {
        return Err("pdfunite failed to assemble rotated PDF".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pdf_path() {
        assert!(is_pdf_path(Path::new("doc.pdf")));
        assert!(is_pdf_path(Path::new("DOC.PDF")));
        assert!(!is_pdf_path(Path::new("image.png")));
    }
}
