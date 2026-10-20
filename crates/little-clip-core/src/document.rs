//! Office document inspection, conversion to paginated PDF, and text extraction.
//!
//! Provides native (rdocx / anydoc) and system-assisted (LibreOffice) conversion
//! for DOCX, DOC, ODT, RTF, and related document formats.

use crate::process::CommandRequest;
use crate::staging::ArtifactStore;
use std::path::Path;

/// Returns `true` if the path has a supported office document extension.
pub fn is_office_document(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "docx" | "doc" | "odt" | "rtf" | "pptx" | "ppt" | "odp"
    )
}

/// Returns `true` if the path is a Word document (`.docx` or `.doc`).
pub fn is_word_document(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "docx" | "doc")
}

/// Returns `true` if the path is a modern Word document (`.docx`).
pub fn is_docx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("docx"))
}

/// Returns `true` if the path is a legacy binary Word document (`.doc`).
pub fn is_doc(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("doc"))
}

/// Returns `true` if the path is an OpenDocument Text document (`.odt`).
pub fn is_odt(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("odt"))
}

/// Returns `true` if the path is a Rich Text Format document (`.rtf`).
pub fn is_rtf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rtf"))
}

/// Returns `true` if the path is a presentation document (`.pptx`, `.ppt`, `.odp`).
pub fn is_presentation(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "pptx" | "ppt" | "odp")
}

/// Attempts to render an office document into a paginated PDF.
///
/// Uses headless LibreOffice if available on the system for maximum fidelity.
/// If LibreOffice is unavailable or fails, falls back to native pure-Rust
/// rendering via `rdocx` for `.docx`, `.odt`, and `.rtf`.
pub fn render_to_pdf(input: &Path, output_pdf: &Path) -> Result<(), String> {
    // 1. Try headless LibreOffice if installed (handles doc, docx, odt, rtf, ppt, pptx)
    if let Ok(()) = render_with_libreoffice(input, output_pdf) {
        if std::fs::metadata(output_pdf)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            return Ok(());
        }
    }

    // 2. Fallback to native pure-Rust rdocx layout engine
    render_with_rdocx(input, output_pdf)
}

/// Attempts conversion using a system-installed LibreOffice / soffice binary.
fn render_with_libreoffice(input: &Path, output_pdf: &Path) -> Result<(), String> {
    let store = ArtifactStore::new().map_err(|e| e.to_string())?;
    let workspace = store
        .create_workspace("document_preview", "libreoffice_out")
        .map_err(|e| e.to_string())?;
    let out_dir = workspace.path();

    let binary = if which_exists("libreoffice") {
        "libreoffice"
    } else if which_exists("soffice") {
        "soffice"
    } else {
        return Err("Neither libreoffice nor soffice found on system".to_string());
    };

    let user_install = format!("-env:UserInstallation=file://{}/user", out_dir.display());
    let request = CommandRequest::new(binary)
        .arg(user_install)
        .arg("--headless")
        .arg("--convert-to")
        .arg("pdf")
        .arg(input)
        .arg("--outdir")
        .arg(out_dir);

    let output = request
        .run()
        .map_err(|e| format!("Failed to run {binary}: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "{binary} failed with exit code {:?}: {}",
            output.status.code(),
            output.stderr_lossy()
        ));
    }

    // LibreOffice outputs a file with the input's stem and .pdf extension in out_dir
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let generated = out_dir.join(format!("{stem}.pdf"));

    if generated.exists() {
        if let Some(parent) = output_pdf.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::copy(&generated, output_pdf).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Search out_dir for any generated pdf
    if let Ok(entries) = std::fs::read_dir(out_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                if let Some(parent) = output_pdf.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::copy(&p, output_pdf).map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    }

    Err("LibreOffice finished but no PDF output was found".to_string())
}

/// Fallback native pure-Rust PDF layout rendering via `rdocx`.
fn render_with_rdocx(input: &Path, output_pdf: &Path) -> Result<(), String> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if let Some(parent) = output_pdf.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match ext.as_str() {
        "docx" => {
            let doc = rdocx::Document::open(input).map_err(|e| format!("rdocx open error: {e}"))?;
            doc.save_pdf(output_pdf)
                .map_err(|e| format!("rdocx PDF rendering error: {e}"))
        }
        "odt" => {
            let read_result = rdocx::Document::open_odt(input)
                .map_err(|e| format!("rdocx ODT read error: {e}"))?;
            read_result
                .document
                .save_pdf(output_pdf)
                .map_err(|e| format!("rdocx ODT PDF rendering error: {e}"))
        }
        "rtf" => {
            let read_result = rdocx::Document::open_rtf(input)
                .map_err(|e| format!("rdocx RTF read error: {e}"))?;
            read_result
                .document
                .save_pdf(output_pdf)
                .map_err(|e| format!("rdocx RTF PDF rendering error: {e}"))
        }
        _ => Err(format!(
            "Native PDF layout rendering not supported for .{ext}; install LibreOffice for full fidelity"
        )),
    }
}

/// Converts an office document into clean Markdown using `anydoc`.
pub fn render_to_markdown(input: &Path) -> Result<String, String> {
    anydoc::to_markdown(input).map_err(|e| format!("anydoc markdown error: {e}"))
}

fn which_exists(binary: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(binary);
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_classification() {
        assert!(is_office_document(Path::new("report.docx")));
        assert!(is_office_document(Path::new("legacy.doc")));
        assert!(is_office_document(Path::new("open.odt")));
        assert!(is_office_document(Path::new("formatted.rtf")));
        assert!(is_office_document(Path::new("slides.pptx")));
        assert!(is_office_document(Path::new("slides.ppt")));
        assert!(is_office_document(Path::new("presentation.odp")));
        assert!(!is_office_document(Path::new("plain.txt")));
        assert!(!is_office_document(Path::new("image.png")));

        assert!(is_word_document(Path::new("doc.docx")));
        assert!(is_word_document(Path::new("doc.doc")));
        assert!(!is_word_document(Path::new("doc.odt")));

        assert!(is_docx(Path::new("file.DOCX")));
        assert!(!is_docx(Path::new("file.doc")));

        assert!(is_doc(Path::new("file.DOC")));
        assert!(!is_doc(Path::new("file.docx")));

        assert!(is_odt(Path::new("file.ODT")));
        assert!(!is_odt(Path::new("file.rtf")));

        assert!(is_rtf(Path::new("file.RTF")));
        assert!(!is_rtf(Path::new("file.odt")));
    }

    #[test]
    fn test_native_rtf_to_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let rtf_path = dir.path().join("test.rtf");
        std::fs::write(
            &rtf_path,
            b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Courier;}}\\f0\\fs24 Hello \\b World!\\b0\\par This is a test.}",
        )
        .unwrap();

        let md = render_to_markdown(&rtf_path).expect("anydoc should parse RTF");
        assert!(md.contains("Hello"));
        assert!(md.contains("World"));
    }

    #[test]
    fn test_libreoffice_or_native_rtf_to_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let rtf_path = dir.path().join("test.rtf");
        let pdf_path = dir.path().join("test.pdf");
        std::fs::write(
            &rtf_path,
            b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Courier;}}\\f0\\fs24 Hello \\b World!\\b0\\par This is a test.}",
        )
        .unwrap();

        let res = render_to_pdf(&rtf_path, &pdf_path);
        assert!(res.is_ok(), "render_to_pdf failed: {:?}", res);
        assert!(pdf_path.exists());
        assert!(std::fs::metadata(&pdf_path).unwrap().len() > 0);
    }

    #[test]
    fn test_native_docx_to_pdf_and_markdown() {
        // Build a minimal DOCX programmatically
        let mut doc = rdocx::Document::new();
        doc.add_paragraph("Hello from DOCX Test");
        doc.add_paragraph("Second paragraph with more content.");

        let dir = tempfile::tempdir().unwrap();
        let docx_path = dir.path().join("test.docx");
        let pdf_path = dir.path().join("test.pdf");

        doc.save(&docx_path).expect("save docx");

        // Test markdown extraction
        let md = render_to_markdown(&docx_path).expect("markdown conversion");
        assert!(md.contains("Hello from DOCX Test"));

        // Test PDF conversion
        let res = render_to_pdf(&docx_path, &pdf_path);
        assert!(res.is_ok(), "docx render_to_pdf failed: {:?}", res);
        assert!(pdf_path.exists());
        assert!(std::fs::metadata(&pdf_path).unwrap().len() > 0);
    }
}
