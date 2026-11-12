//! Office document inspection, conversion to paginated PDF, and text extraction.
//!
//! Provides native (rdocx / anydoc) and system-assisted (LibreOffice) conversion
//! for DOCX, DOC, ODT, RTF, and related document formats.

use crate::process::CommandRequest;
use crate::staging::ArtifactStore;
use std::path::{Path, PathBuf};

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

/// Returns the standard MIME type for a given office document path.
pub fn guess_office_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "odt" => "application/vnd.oasis.opendocument.text",
        "rtf" => "application/rtf",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ppt" => "application/vnd.ms-powerpoint",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        _ => "application/octet-stream",
    }
}

/// Returns `true` if the MIME type corresponds to an office document.
pub fn is_office_mime(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/msword"
            | "application/vnd.oasis.opendocument.text"
            | "application/rtf"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.ms-powerpoint"
            | "application/vnd.oasis.opendocument.presentation"
    )
}

mod cache;
pub use cache::{RenderedPdf, ensure_rendered_pdf};

/// Attempts to render an office document into a paginated PDF.
///
/// Uses headless LibreOffice if available on the system for maximum fidelity.
/// If LibreOffice is unavailable or fails, falls back to native pure-Rust
/// rendering via `rdocx` for `.docx`, `.odt`, and `.rtf`.
pub fn render_to_pdf(input: &Path, output_pdf: &Path) -> Result<(), String> {
    // 1. Try headless LibreOffice if installed (handles doc, docx, odt, rtf, ppt, pptx)
    if let Ok(()) = render_with_libreoffice(input, output_pdf)
        && std::fs::metadata(output_pdf)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        return Ok(());
    }

    // 2. Fallback to native pure-Rust rdocx layout engine
    render_with_rdocx(input, output_pdf)
}

/// Attempts conversion using a system-installed LibreOffice / soffice binary.
fn render_with_libreoffice(input: &Path, output_pdf: &Path) -> Result<(), String> {
    let store = ArtifactStore::new().map_err(|e| e.to_string())?;
    let workspace = store
        .create_temporary_workspace("document_preview", "libreoffice_out")
        .map_err(|e| e.to_string())?;
    let out_dir = workspace.path();

    let binary = if crate::process::command_available("libreoffice").is_some() {
        "libreoffice"
    } else if crate::process::command_available("soffice").is_some() {
        "soffice"
    } else {
        return Err("Neither libreoffice nor soffice found on system".to_string());
    };

    let user_install = format!("-env:UserInstallation=file://{}/user", out_dir.display());
    let request = workspace
        .guard_command(CommandRequest::new(binary))
        .map_err(|error| error.to_string())?
        .arg(user_install)
        .arg("--headless")
        .arg("--convert-to")
        .arg("pdf")
        .arg_content_file(input)
        .map_err(|error| format!("Cannot open office input: {error}"))?
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

    // Descriptor input names are opaque. This private workspace contains one
    // conversion, so discover its PDF rather than relying on the source basename.
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

/// Prepared studio preview payload for an office document.
pub enum PreparedOfficePreview {
    /// Paginated PDF staged in a temporary workspace owned by this result.
    Pdf {
        pdf_path: PathBuf,
        workspace: crate::staging::TemporaryWorkspace,
    },
    /// Fallback Markdown text when paginated PDF rendering is unavailable.
    Markdown(String),
}

/// Prepares a studio preview for an office document, preferring a cached paginated PDF
/// in an isolated temporary workspace and falling back to Markdown conversion.
pub fn prepare_office_preview(input: &Path) -> Result<PreparedOfficePreview, String> {
    let store = crate::staging::ArtifactStore::new().map_err(|e| e.to_string())?;
    let workspace = store
        .create_temporary_workspace("office_preview", "pdf_render")
        .map_err(|e| e.to_string())?;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let pdf_path = workspace.path().join(format!("{stem}.pdf"));
    if let Ok(cached_pdf) = ensure_rendered_pdf(input)
        && std::fs::copy(&cached_pdf, &pdf_path).is_ok()
        && std::fs::metadata(&pdf_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        return Ok(PreparedOfficePreview::Pdf {
            pdf_path,
            workspace,
        });
    }
    render_to_markdown(input).map(PreparedOfficePreview::Markdown)
}

/// Body text and coverage from a passive RTF conversion.
pub struct RtfText {
    /// Accepted body paragraphs and table cells in document order.
    pub text: String,
    /// Whether conversion completed without reported loss, images or truncation.
    pub complete: bool,
}

/// Extracts bounded body text from RTF bytes without resolving external resources.
///
/// The native reader accepts at most 64 MiB and limits group nesting, retained
/// content, runs and tables. This function performs no file or network access,
/// field evaluation, rendering or script execution. The caller must read a
/// verified ordinary source and isolate parsing in a bounded worker process.
/// Conversion diagnostics and embedded images make coverage partial, including
/// skipped headers, footnotes, objects and field results. Output is truncated
/// at a UTF-8 boundary to `max_text_bytes`.
///
/// # Errors
///
/// Returns an error for malformed RTF or an exceeded native-reader limit.
///
/// # Examples
///
/// ```
/// let result = little_clip_core::document::extract_rtf_text(
///     br"{\rtf1\ansi Hello \b world\b0.}", 1024,
/// )?;
/// assert!(result.text.contains("Hello world."));
/// assert!(result.complete);
/// # Ok::<(), String>(())
/// ```
pub fn extract_rtf_text(bytes: &[u8], max_text_bytes: usize) -> Result<RtfText, String> {
    let parsed = rdocx::Document::from_rtf_bytes(bytes)
        .map_err(|error| format!("RTF extraction failed: {error}"))?;
    let mut text = parsed.document.text();
    let complete = parsed.diagnostics.is_empty()
        && parsed.document.images().is_empty()
        && text.len() <= max_text_bytes;
    text.truncate(text.floor_char_boundary(max_text_bytes.min(text.len())));
    Ok(RtfText { text, complete })
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
    fn passive_rtf_text_preserves_formatting_boundaries_codepages_and_unicode() {
        let result = extract_rtf_text(
            br"{\rtf1\ansi\ansicpg1252 Hello \b world\b0.\par caf\'e9 \u-10179?\u-8704?}",
            1024,
        )
        .unwrap();
        assert!(result.complete);
        assert_eq!(result.text, "Hello world.\ncafé 😀\n");
        let short = extract_rtf_text(br"{\rtf1\ansi A\u233?B}", 2).unwrap();
        assert_eq!(short.text, "A");
        assert!(
            !short.complete,
            "UTF-8 output truncation is partial coverage"
        );
    }

    #[test]
    fn passive_rtf_skipped_resources_and_embedded_images_cannot_claim_complete_text() {
        for input in [
            br"{\rtf1\ansi visible{\header hidden header}}".as_slice(),
            br"{\rtf1\ansi visible{\object\objautlink{\*\objclass Package}{\*\objdata 010203}}}",
            br#"{\rtf1\ansi visible{\field{\*\fldinst INCLUDETEXT "/proc/self/environ"}{\fldrslt hidden result}}}"#,
            br"{\rtf1\ansi visible{\pict\pngblip\picwgoal100\pichgoal100 89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360f8cff00000040101089d1de10000000049454e44ae426082}}",
        ] {
            let extracted = extract_rtf_text(input, 1024).unwrap();
            assert!(extracted.text.contains("visible"));
            assert!(!extracted.text.contains("hidden"));
            assert!(!extracted.text.contains("/proc"));
            assert!(!extracted.complete);
        }
    }

    #[test]
    fn passive_rtf_rejects_malformed_input_and_excessive_nesting() {
        for input in [
            b"not RTF".as_slice(),
            br"{\rtf1 truncated",
            br"{\rtf1\ansi\bin100 short}",
        ] {
            assert!(extract_rtf_text(input, 1024).is_err());
        }
        let nested = format!("{{\\rtf1 {}text{}}}", "{".repeat(257), "}".repeat(257));
        assert!(extract_rtf_text(nested.as_bytes(), 1024).is_err());
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
    fn libreoffice_pins_input_before_a_child_can_replace_its_name() {
        const ENV: &str = "LITTLE_CLIP_OFFICE_PIN_TEST";
        let Some(root) = std::env::var_os(ENV).map(PathBuf::from) else {
            use std::os::unix::fs::PermissionsExt;
            let root = tempfile::tempdir().unwrap();
            let binary = root.path().join("libreoffice");
            std::fs::write(
                &binary,
                r#"#!/bin/sh
: > "$LITTLE_CLIP_OFFICE_PIN_TEST/child-ran"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --convert-to) shift ;;
        --outdir) shift; outdir="$1" ;;
        --headless|-env:*) ;;
        *) input="$1" ;;
    esac
    shift
done
/bin/mv "$LITTLE_CLIP_OFFICE_PIN_TEST/source.rtf" "$LITTLE_CLIP_OFFICE_PIN_TEST/moved.rtf"
printf 'replacement input' > "$LITTLE_CLIP_OFFICE_PIN_TEST/source.rtf"
/bin/cat "$input" > "$LITTLE_CLIP_OFFICE_PIN_TEST/observed"
printf '%%PDF-1.4\nfixture\n' > "$outdir/converted.pdf"
"#,
            )
            .unwrap();
            std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "document::tests::libreoffice_pins_input_before_a_child_can_replace_its_name",
                    "--nocapture",
                ])
                .env(ENV, root.path())
                .env("PATH", root.path())
                .env("XDG_CACHE_HOME", root.path().join("cache"))
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("DISPLAY")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        };
        let input = root.join("source.rtf");
        let output = root.join("result.pdf");
        std::fs::write(&input, "original input").unwrap();
        render_with_libreoffice(&input, &output).unwrap();
        assert_eq!(
            std::fs::read(root.join("observed")).unwrap(),
            b"original input"
        );
        assert!(std::fs::read(&output).unwrap().starts_with(b"%PDF-"));
        std::fs::remove_file(root.join("child-ran")).unwrap();
        let protected = root.join("protected.rtf");
        std::os::unix::fs::symlink("/proc/uptime", &protected).unwrap();
        let rejected = root.join("rejected.pdf");
        let error = render_with_libreoffice(&protected, &rejected).unwrap_err();
        assert!(error.contains("inspection-only"), "{error}");
        assert!(!root.join("child-ran").exists());
        assert!(!rejected.exists());
    }

    #[test]
    fn installed_libreoffice_renders_pinned_rtf_and_docx_without_native_fallback() {
        if crate::process::command_available("libreoffice").is_none()
            && crate::process::command_available("soffice").is_none()
        {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let rtf = root.path().join("example.rtf");
        std::fs::write(&rtf, b"{\\rtf1\\ansi Pinned RTF input.}").unwrap();
        let docx = root.path().join("example.docx");
        let mut document = rdocx::Document::new();
        document.add_paragraph("Pinned DOCX input");
        document.save(&docx).unwrap();
        for input in [rtf, docx] {
            let output = root.path().join(format!(
                "{}.pdf",
                input.extension().unwrap().to_string_lossy()
            ));
            render_with_libreoffice(&input, &output).unwrap();
            assert!(std::fs::read(output).unwrap().starts_with(b"%PDF-"));
        }
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
    fn undelivered_office_preview_reclaims_its_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("cancelled.rtf");
        std::fs::write(
            &input,
            br"{\rtf1\ansi An Office preview that is never opened.}",
        )
        .unwrap();

        let preview = prepare_office_preview(&input).unwrap();
        let PreparedOfficePreview::Pdf {
            pdf_path,
            workspace,
        } = &preview
        else {
            panic!("expected a staged PDF");
        };
        let workspace_path = workspace.path().to_owned();
        assert!(pdf_path.is_file());
        let (send, receive) = std::sync::mpsc::channel();
        send.send(preview).unwrap();
        assert!(workspace_path.is_dir());
        drop(receive);
        assert!(!workspace_path.exists());
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

    #[test]
    fn test_ensure_rendered_pdf_and_mime() {
        assert_eq!(
            guess_office_mime(Path::new("file.docx")),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert!(is_office_mime(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ));
        assert!(!is_office_mime("image/png"));

        let mut doc = rdocx::Document::new();
        doc.add_paragraph("Cached DOCX Test Page 1");

        let dir = tempfile::tempdir().unwrap();
        let docx_path = dir.path().join("cached_test.docx");
        doc.save(&docx_path).expect("save docx");

        let cached_pdf = ensure_rendered_pdf(&docx_path).expect("ensure_rendered_pdf");
        assert!(cached_pdf.exists());
        assert!(std::fs::metadata(&cached_pdf).unwrap().len() > 0);

        // A second call should return the exact same cached path without re-rendering
        let second_call = ensure_rendered_pdf(&docx_path).expect("ensure_rendered_pdf second call");
        assert_eq!(cached_pdf.as_path(), second_call.as_path());
    }
}
