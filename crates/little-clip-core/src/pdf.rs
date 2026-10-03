//! PDF metadata, bookmarks, text extraction, page previews, and lossless editing.
//!
//! Wraps Poppler utilities (`pdfinfo`, `pdftoppm`) for inspection and previews,
//! and uses `lopdf` for lossless page manipulation.

use crate::process::CommandRequest;
use crate::staging::ArtifactStore;
use lopdf::{Document, Object};
use std::path::{Path, PathBuf};

fn preview_output_paths(temp_prefix: &Path) -> Vec<PathBuf> {
    let Some(parent) = temp_prefix.parent() else {
        return Vec::new();
    };
    let Some(prefix) = temp_prefix.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{prefix}-");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".png"))
        })
        .collect()
}

fn cleanup_preview_outputs(temp_prefix: &Path) {
    for path in preview_output_paths(temp_prefix) {
        let _ = std::fs::remove_file(path);
    }
}

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
    let output = CommandRequest::new("pdfinfo")
        .arg(path)
        .run()
        .map_err(|e| format!("Failed to run pdfinfo: {}", e))?;

    if !output.status.success() {
        return Err("pdfinfo exited with error".to_string());
    }

    let stdout = output.stdout_lossy();
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
pub fn render_pdf_page_preview(
    path: &Path,
    page: usize,
    max_dim: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let workspace = ArtifactStore::new()
        .and_then(|store| store.create_workspace("pdf", "preview"))
        .map_err(|error| format!("could not create PDF preview workspace: {error}"))?;
    let temp_prefix = workspace.path().join(format!("page-{page}"));
    let page_str = page.to_string();

    let output = CommandRequest::new("pdftoppm")
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
        .run()
        .map_err(|e| format!("pdftoppm execution failed: {}", e))?;

    if !output.status.success() {
        cleanup_preview_outputs(&temp_prefix);
        return Err("pdftoppm failed to render page".to_string());
    }

    let out_png = preview_output_paths(&temp_prefix).into_iter().next();
    let Some(out_png) = out_png else {
        return Err(format!("pdftoppm did not produce a PNG for page {page}"));
    };

    let png_bytes = std::fs::read(&out_png).map_err(|e| e.to_string());
    cleanup_preview_outputs(&temp_prefix);
    let png_bytes = png_bytes?;

    let (w, h) =
        image::ImageReader::with_format(std::io::Cursor::new(&png_bytes), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|error| format!("Could not read rendered PDF dimensions: {error}"))?;

    Ok((png_bytes, w, h))
}

/// Renders thumbnails for all pages of a PDF
pub fn render_all_pdf_page_thumbnails(
    path: &Path,
    thumb_w: u32,
) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let info = get_pdf_info(path)?;
    let mut pages = Vec::new();
    for page_num in 1..=info.page_count {
        let (bytes, _, _) = render_pdf_page_preview(path, page_num, thumb_w)
            .map_err(|error| format!("Failed to render PDF page {page_num}: {error}"))?;
        pages.push((page_num, bytes));
    }
    Ok(pages)
}

/// One bookmark in document order, with a one-based destination page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfOutlineEntry {
    /// Decoded bookmark title.
    pub title: String,
    /// One-based destination page number.
    pub page: usize,
    /// Nesting depth, starting at zero for top-level bookmarks.
    pub depth: usize,
}

/// Reads embedded bookmarks, preserving nesting and repeated titles.
///
/// Returns an empty list for a document without bookmarks.
///
/// # Errors
///
/// Returns an error if the PDF cannot be read or its outline cannot be parsed.
pub fn read_pdf_outline(path: &Path) -> Result<Vec<PdfOutlineEntry>, String> {
    let document = Document::load(path).map_err(|error| error.to_string())?;
    if document
        .catalog()
        .and_then(|catalog| catalog.get(b"Outlines"))
        .is_err()
    {
        return Ok(Vec::new());
    }
    let outlines = document
        .get_outlines(None, None, &mut Default::default())
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let pages = document.get_pages();
    let mut entries = Vec::new();
    flatten_pdf_outline(&outlines, &pages, 0, &mut entries);
    Ok(entries)
}

fn flatten_pdf_outline(
    outlines: &[lopdf::Outline],
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
    depth: usize,
    entries: &mut Vec<PdfOutlineEntry>,
) {
    for outline in outlines {
        match outline {
            lopdf::Outline::SubOutlines(children) => {
                flatten_pdf_outline(children, pages, depth + 1, entries);
            }
            lopdf::Outline::Destination(destination) => {
                let title = destination.title().and_then(lopdf::decode_text_string);
                let page = destination
                    .page()
                    .and_then(Object::as_reference)
                    .ok()
                    .and_then(|id| pages.iter().find(|(_, value)| **value == id))
                    .map(|(page, _)| *page as usize);
                if let (Ok(title), Some(page)) = (title, page) {
                    entries.push(PdfOutlineEntry { title, page, depth });
                }
            }
        }
    }
}

/// Extracts one page's text through Poppler, keeping page boundaries explicit.
///
/// `page` is one-based. Pages without a text layer return empty text.
///
/// # Errors
///
/// Returns an error if `pdftotext` fails, times out, or exceeds the output limit.
pub fn extract_pdf_page_text(path: &Path, page: usize) -> Result<String, String> {
    let output = CommandRequest::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8", "-f"])
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg(path)
        .arg("-")
        .max_output_bytes(4 * 1024 * 1024)
        .run()
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "Could not extract page {page}: {}",
            output.stderr_lossy()
        ));
    }
    Ok(output.stdout_lossy().trim_matches('\u{c}').to_owned())
}

/// Deletes specified pages from a PDF and writes the remaining pages to `output`
pub fn delete_pdf_pages(
    input: &Path,
    pages_to_delete: &[usize],
    output: &Path,
) -> Result<(), String> {
    if let Ok(()) = qpdf_edit(input, pages_to_delete, None, output) {
        return Ok(());
    }
    if let Ok(()) = rebuild_pages(input, pages_to_delete, None, output) {
        return Ok(());
    }
    let mut document = Document::load(input)
        .map_err(|error| format!("Could not parse PDF for page deletion: {error}"))?;
    let pages = document.get_pages();
    if pages.is_empty() {
        return Err("PDF is empty".to_string());
    }
    if pages_to_delete.is_empty() {
        return Err("No pages selected for deletion".to_string());
    }
    if pages_to_delete.len() >= pages.len() {
        return Err("Cannot delete all pages from PDF".to_string());
    }
    for &page in pages_to_delete {
        if !pages.contains_key(&(page as u32)) {
            return Err(format!("PDF page {page} does not exist"));
        }
    }
    let mut unique_pages = pages_to_delete
        .iter()
        .map(|&page| page as u32)
        .collect::<Vec<_>>();
    unique_pages.sort_unstable();
    unique_pages.dedup();
    if unique_pages.len() >= pages.len() {
        return Err("Cannot delete all pages from PDF".to_string());
    }
    document.delete_pages(&unique_pages);
    document.prune_objects();
    document
        .save(output)
        .map(|_| ())
        .map_err(|error| format!("Could not write edited PDF: {error}"))
}

/// Rotates a specific page by specified degrees (90, 180, 270)
pub fn rotate_pdf_page(
    input: &Path,
    page_to_rotate: usize,
    degrees: u32,
    output: &Path,
) -> Result<(), String> {
    if !matches!(degrees, 90 | 180 | 270) {
        return Err("Rotation must be 90, 180, or 270 degrees".to_string());
    }
    if let Ok(()) = qpdf_edit(input, &[], Some((page_to_rotate, degrees)), output) {
        return Ok(());
    }
    if let Ok(()) = rebuild_pages(input, &[], Some((page_to_rotate, degrees)), output) {
        return Ok(());
    }
    let mut document = Document::load(input)
        .map_err(|error| format!("Could not parse PDF for rotation: {error}"))?;
    let pages = document.get_pages();
    let page_id = pages
        .get(&(page_to_rotate as u32))
        .copied()
        .ok_or_else(|| format!("PDF page {page_to_rotate} does not exist"))?;
    let page = document
        .get_object_mut(page_id)
        .map_err(|error| format!("Could not access PDF page {page_to_rotate}: {error}"))?
        .as_dict_mut()
        .map_err(|error| format!("PDF page {page_to_rotate} is not a page dictionary: {error}"))?;
    let existing = page
        .get(b"Rotate")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(0);
    page.set(
        "Rotate",
        Object::Integer((existing + degrees as i64).rem_euclid(360)),
    );
    document
        .save(output)
        .map(|_| ())
        .map_err(|error| format!("Could not write rotated PDF: {error}"))
}

/// qpdf rewrites page trees without decoding/re-encoding page contents, making
/// it the preferred path for encrypted and object-stream-heavy manuals.
fn qpdf_edit(
    input: &Path,
    pages_to_delete: &[usize],
    rotation: Option<(usize, u32)>,
    output: &Path,
) -> Result<(), String> {
    if rotation.is_none() && pages_to_delete.is_empty() {
        return Err("No PDF edit requested".into());
    }
    let mut command = CommandRequest::new("qpdf");
    if let Some((page, degrees)) = rotation {
        command = command
            .arg(format!("--rotate=+{degrees}:{page}"))
            .arg(input)
            .arg(output);
    } else {
        let count = get_pdf_info(input)?.page_count;
        let kept = (1..=count)
            .filter(|page| !pages_to_delete.contains(page))
            .map(|page| page.to_string())
            .collect::<Vec<_>>();
        if kept.is_empty() {
            return Err("Cannot delete all pages".into());
        }
        command = command
            .arg(input)
            .arg("--pages")
            .arg(input)
            .arg(kept.join(","))
            .arg("--")
            .arg(output);
    }
    let result = command
        .run()
        .map_err(|e| format!("qpdf unavailable: {e}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!("qpdf failed: {}", result.stderr_lossy().trim()))
    }
}

/// Use Poppler to isolate pages and Ghostscript only for the page that needs
/// rotation. This preserves page resources in PDFs that `lopdf` cannot safely
/// rewrite (notably object-stream-heavy firmware manuals).
fn rebuild_pages(
    input: &Path,
    pages_to_delete: &[usize],
    rotation: Option<(usize, u32)>,
    output: &Path,
) -> Result<(), String> {
    let page_count = get_pdf_info(input)?.page_count;
    let workspace = ArtifactStore::new()
        .and_then(|store| store.create_workspace("pdf", "rebuild"))
        .map_err(|error| format!("could not create PDF rebuild workspace: {error}"))?;
    let temp = workspace.path();
    let pattern = temp.join("page-%d.pdf");
    let separated = CommandRequest::new("pdfseparate")
        .arg(input)
        .arg(&pattern)
        .run()
        .map_err(|e| format!("pdfseparate unavailable: {e}"))?;
    if !separated.status.success() {
        let _ = std::fs::remove_dir_all(temp);
        return Err("pdfseparate failed".into());
    }
    let mut inputs = Vec::new();
    for page in 1..=page_count {
        if pages_to_delete.contains(&page) {
            continue;
        }
        let source = temp.join(format!("page-{page}.pdf"));
        if rotation.map(|(target, _)| target) == Some(page) {
            let degrees = rotation.map(|(_, d)| d).unwrap_or(0);
            let rotated = temp.join(format!("rotated-{page}.pdf"));
            let orientation = match degrees {
                90 => 3,
                180 => 2,
                270 => 1,
                _ => 0,
            };
            let status = CommandRequest::new("gs")
                .arg("-q")
                .arg("-dNOPAUSE")
                .arg("-dBATCH")
                .arg("-sDEVICE=pdfwrite")
                .arg(format!("-sOutputFile={}", rotated.display()))
                .arg("-c")
                .arg(format!("<</Orientation {orientation}>> setpagedevice"))
                .arg("-f")
                .arg(&source)
                .run()
                .map_err(|e| format!("Ghostscript unavailable: {e}"))?;
            if !status.status.success() {
                let _ = std::fs::remove_dir_all(temp);
                return Err("Ghostscript rotation failed".into());
            }
            inputs.push(rotated);
        } else {
            inputs.push(source);
        }
    }
    if inputs.is_empty() {
        let _ = std::fs::remove_dir_all(temp);
        return Err("Cannot delete all pages".into());
    }
    let mut unite = CommandRequest::new("pdfunite");
    for input_file in &inputs {
        unite = unite.arg(input_file);
    }
    let status = unite
        .arg(output)
        .run()
        .map_err(|e| format!("pdfunite unavailable: {e}"))?;
    let _ = std::fs::remove_dir_all(temp);
    if status.status.success() {
        Ok(())
    } else {
        Err("pdfunite failed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn navigation_fixture() -> (tempfile::TempDir, PathBuf) {
        use lopdf::{Stream, dictionary};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("navigation.pdf");
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font = document.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let resources = document.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut pages = Vec::new();
        for text in [
            "Cobalt needle on page one",
            "Amber lighthouse on page two",
            "Cobalt needle again",
        ] {
            let stream = document.add_object(Stream::new(
                dictionary! {},
                format!("BT /F1 18 Tf 50 700 Td ({text}) Tj ET").into_bytes(),
            ));
            pages.push(document.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Resources" => resources,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()], "Contents" => stream
            }));
        }
        document.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => pages.iter().copied().map(Object::Reference).collect::<Vec<_>>(), "Count" => 3 }.into());
        let outlines = document.new_object_id();
        let first = document.new_object_id();
        let second = document.new_object_id();
        let child = document.new_object_id();
        document.objects.insert(
            outlines,
            dictionary! { "Type" => "Outlines", "First" => first, "Last" => second, "Count" => 3 }
                .into(),
        );
        document.objects.insert(first, dictionary! { "Title" => Object::string_literal("Repeated"), "Parent" => outlines, "Next" => second, "First" => child, "Last" => child, "Count" => 1, "Dest" => vec![pages[0].into(), Object::Name(b"Fit".to_vec())] }.into());
        document.objects.insert(second, dictionary! { "Title" => Object::string_literal("Repeated"), "Parent" => outlines, "Prev" => first, "Dest" => vec![pages[2].into(), Object::Name(b"Fit".to_vec())] }.into());
        document.objects.insert(child, dictionary! { "Title" => lopdf::text_string("日本語"), "Parent" => first, "Dest" => vec![pages[1].into(), Object::Name(b"Fit".to_vec())] }.into());
        let catalog = document.add_object(
            dictionary! { "Type" => "Catalog", "Pages" => pages_id, "Outlines" => outlines },
        );
        document.trailer.set("Root", catalog);
        document.save(&path).unwrap();
        (directory, path)
    }

    #[test]
    fn outline_preserves_duplicate_titles_nested_destinations_and_unicode() {
        let (_directory, path) = navigation_fixture();
        let entries = read_pdf_outline(&path).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.title.as_str(), entry.page, entry.depth))
                .collect::<Vec<_>>(),
            vec![("Repeated", 1, 0), ("日本語", 2, 1), ("Repeated", 3, 0)]
        );
    }

    #[test]
    fn poppler_extraction_keeps_text_on_its_own_page() {
        let (_directory, path) = navigation_fixture();
        let first = extract_pdf_page_text(&path, 1).unwrap();
        let second = extract_pdf_page_text(&path, 2).unwrap();
        assert!(first.contains("Cobalt needle"));
        assert!(!first.contains("Amber lighthouse"));
        assert!(second.contains("Amber lighthouse"));
    }

    #[test]
    fn documents_without_bookmarks_have_an_empty_outline() {
        let (_directory, path) = navigation_fixture();
        let mut document = Document::load(&path).unwrap();
        let root = document
            .trailer
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        document
            .get_dictionary_mut(root)
            .unwrap()
            .remove(b"Outlines");
        document.save(&path).unwrap();
        assert!(read_pdf_outline(&path).unwrap().is_empty());
    }

    #[test]
    fn test_is_pdf_path() {
        assert!(is_pdf_path(Path::new("doc.pdf")));
        assert!(is_pdf_path(Path::new("DOC.PDF")));
        assert!(!is_pdf_path(Path::new("image.png")));
    }

    #[test]
    fn test_delete_and_rotate_pdf() {
        let input = Path::new("/tmp/test_multi.pdf");
        if input.is_file() {
            let del_out = Path::new("/tmp/test_del_out.pdf");
            let del_res = delete_pdf_pages(input, &[1], del_out);
            println!("del_res: {:?}", del_res);
            assert!(del_res.is_ok());

            let rot_out = Path::new("/tmp/test_rot_out.pdf");
            let rot_res = rotate_pdf_page(input, 1, 90, rot_out);
            println!("rot_res: {:?}", rot_res);
            assert!(rot_res.is_ok());
        }
    }
}
