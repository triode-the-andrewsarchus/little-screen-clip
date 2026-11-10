//! Prepare only the selected cell's bounded display data on a worker.
use super::model::{Cell, Kind, Output};
use crate::text_preview::TextPreviewData;
use base64::Engine;
use little_clip_core::ImageThumbnail;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Prepared {
    pub source: Arc<TextPreviewData>,
    pub markdown: Vec<little_clip_render::markdown::MarkdownLine>,
    pub markdown_truncated: bool,
    pub outputs: Vec<PreparedOutput>,
    pub omitted_outputs: usize,
}
pub(super) enum PreparedOutput {
    Text {
        label: String,
        source: Arc<TextPreviewData>,
        text: Arc<String>,
        error: bool,
    },
    Image(ImageThumbnail),
    Message(String),
}
pub(super) fn prepare(cell: &Cell, language: &str, token: &AtomicBool) -> Result<Prepared, String> {
    if token.load(Ordering::Relaxed) {
        return Err("Cell preparation cancelled".into());
    }
    let language = match cell.kind {
        Kind::Code => language,
        Kind::Markdown => "md",
        _ => "txt",
    };
    let source = TextPreviewData::from_source(cell.source.clone(), Some(language));
    let mut end = cell.source.len().min(64 * 1024);
    while !cell.source.is_char_boundary(end) {
        end -= 1;
    }
    let mut markdown = if cell.kind == Kind::Markdown {
        little_clip_render::markdown::parse_markdown(&cell.source[..end])
    } else {
        Vec::new()
    };
    let markdown_truncated =
        cell.kind == Kind::Markdown && (end < cell.source.len() || markdown.len() > 512);
    markdown.truncate(512);
    let mut prepared = Prepared {
        source,
        markdown,
        markdown_truncated,
        outputs: Vec::new(),
        omitted_outputs: cell.outputs.len().saturating_sub(32),
    };
    let mut image_bytes = 0;
    for output in cell.outputs.iter().take(32) {
        if token.load(Ordering::Relaxed) {
            return Err("Cell preparation cancelled".into());
        }
        let display = match output {
            Output::Text { label, text, error } => PreparedOutput::Text {
                label: label.clone(),
                text: text.clone(),
                error: *error,
                source: TextPreviewData::from_source(text.clone(), Some("txt")),
            },
            Output::Image { mime, encoded } => match image(encoded, mime, image_bytes) {
                Ok(image) => {
                    image_bytes += image.premultiplied_rgba.len();
                    PreparedOutput::Image(image)
                }
                Err(error) => PreparedOutput::Message(error),
            },
            Output::Unsupported(message) => PreparedOutput::Message(message.clone()),
        };
        prepared.outputs.push(display);
    }
    Ok(prepared)
}
fn image(encoded: &str, mime: &str, used: usize) -> Result<ImageThumbnail, String> {
    if encoded.len() > 8 * 1024 * 1024 || used >= 8 * 1024 * 1024 {
        return Err("Saved image exceeds the cell preview budget".into());
    }
    let bytes: Vec<_> = encoded
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes)
        .map_err(|e| format!("Invalid saved image: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    // Notebook raster outputs must not enter the SVG fallback or resolve resources.
    let mut reader = little_clip_core::item::image_reader(&bytes, Some(mime))
        .map_err(|e| format!("Invalid saved image: {e}"))?;
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Saved image is invalid or exceeds the raster limit: {e}"))?;
    let thumbnail = ImageThumbnail::from_rgba(decoded.thumbnail(1024, 512).to_rgba8());
    if used + thumbnail.premultiplied_rgba.len() > 8 * 1024 * 1024 {
        return Err("Saved images exceed the cell preview budget".into());
    }
    Ok(thumbnail)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> super::super::model::Document {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/notebooks/sample.ipynb");
        super::super::model::load(&path, &AtomicBool::new(false)).unwrap()
    }
    #[test]
    fn selected_cells_prepare_passive_markdown_text_and_images() {
        let notebook = document();
        let token = AtomicBool::new(false);
        let prose = prepare(&notebook.cells[0], &notebook.language, &token).unwrap();
        assert!(!prose.markdown.is_empty());
        let code = prepare(&notebook.cells[1], &notebook.language, &token).unwrap();
        assert!(
            matches!(&code.outputs[0], PreparedOutput::Text { text, error: false, .. } if text.as_str() == "[1, 4, 9]\n")
        );
        let PreparedOutput::Image(image) = &code.outputs[1] else {
            panic!("PNG output was not decoded")
        };
        assert!(image.width <= 1024 && image.height <= 512);
        assert_eq!(image.width * 90, image.height * 240);
        let html = prepare(&notebook.cells[4], &notebook.language, &token).unwrap();
        assert!(
            matches!(&html.outputs[0], PreparedOutput::Message(message) if message.contains("text/html"))
        );
        assert!(
            prepare(
                &notebook.cells[0],
                &notebook.language,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
    #[test]
    fn malformed_images_and_render_budgets_are_reported_without_losing_source() {
        assert!(image("invalid base64!", "image/png", 0).is_err());
        assert!(image("YWJj", "image/png", 0).is_err());
        let svg = base64::engine::general_purpose::STANDARD.encode(
            b"<svg xmlns='http://www.w3.org/2000/svg'><image href='file:///etc/hostname'/></svg>",
        );
        assert!(image(&svg, "image/png", 0).is_err());
        assert!(image("", "image/png", 8 * 1024 * 1024).is_err());
        let mut notebook = document();
        let cell = &mut notebook.cells[0];
        cell.source = Arc::new("line\n".repeat(2000));
        let prepared = prepare(cell, "md", &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.markdown.len(), 512);
        assert!(prepared.markdown_truncated);
        cell.kind = Kind::Code;
        cell.outputs = (0..40)
            .map(|_| Output::Unsupported("unknown".into()))
            .collect();
        let prepared = prepare(cell, "py", &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.outputs.len(), 32);
        assert_eq!(prepared.omitted_outputs, 8);
    }
}
