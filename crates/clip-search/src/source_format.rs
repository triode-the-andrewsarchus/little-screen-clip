//! Existing search decoder routes; hints do not establish source readability.
use little_clip_core::file_inventory::Entry;

/// Existing coverage label when no search extractor is available.
pub const NAME_ONLY_UNSUPPORTED: &str = "name-only: extractor unavailable";

/// Returns whether an inventory entry selects an existing content extractor.
pub fn supports_content(entry: &Entry) -> bool {
    supports_text(entry)
        || entry.is_photo()
        || little_clip_core::audio::is_audio_path(&entry.path)
        || entry.is_video()
}

/// Returns whether an entry selects the existing text/PDF extraction route.
pub fn supports_text(entry: &Entry) -> bool {
    little_clip_core::pdf::is_pdf_path(&entry.path)
        || (!entry.is_photo()
            && !little_clip_core::audio::is_audio_path(&entry.path)
            && !entry.is_video()
            && text_kind(entry).is_some())
}

/// Returns the existing text decoder route, respecting observed TypeScript type.
pub fn text_kind(entry: &Entry) -> Option<&'static str> {
    let extension = entry
        .path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    match extension.as_str() {
        "html" => Some("html"),
        "htm" => Some("htm"),
        "xhtml" => Some("xhtml"),
        "docx" => Some("docx"),
        "odt" => Some("odt"),
        "pptx" => Some("pptx"),
        "xlsx" => Some("xlsx"),
        "rtf" => Some("rtf"),
        _ if entry.typescript => Some("txt"),
        _ if little_clip_core::text_format::typescript_suffix(&entry.path) => None,
        _ if little_clip_core::text_format::known_text(&entry.path)
            || matches!(extension.as_str(), "rst" | "org") =>
        {
            Some("txt")
        }
        _ => None,
    }
}
