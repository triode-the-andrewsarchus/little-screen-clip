//! Rendered standalone diagrams share the visible text-row source/cache owner.
use super::shelf::preview::{Data, Ready};
use little_clip_core::{ImageThumbnail, diagram::Format};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Document {
    pub format: Format,
    source: Arc<str>,
    guard: crate::file_source::FileSource,
}
impl Document {
    /// Reads only the already-guarded handle used by text analysis.
    pub fn read(
        path: &Path,
        file: &mut File,
        bytes: u64,
        cancel: &AtomicBool,
        guard: &crate::file_source::FileSource,
        lossy: bool,
        note: &mut String,
    ) -> Result<Option<Self>, String> {
        let explicit = Format::from_path(path);
        let ambiguous = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "json" | "json5" | "dot" | "mmd" | "xml" | "yaml" | "yml"
                )
            });
        if explicit.is_none() && !ambiguous {
            return Ok(None);
        }
        let limit = if explicit == Some(Format::Excalidraw) {
            little_clip_core::diagram::MAX_DRAWING_SOURCE_BYTES
        } else {
            256 * 1024
        };
        if lossy || bytes > limit as u64 {
            if explicit.is_some() {
                *note = if lossy {
                    "Rendered preview requires UTF-8; showing text"
                } else {
                    "Rendered preview exceeds the source budget; open to inspect"
                }
                .into();
            }
            return Ok(None);
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        let mut source = String::new();
        file.take(limit as u64 + 1)
            .read_to_string(&mut source)
            .map_err(|error| error.to_string())?;
        if source.len() > limit || cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Format::from_source_path(path, &source).map(|format| Self {
            format,
            source: Arc::from(source),
            guard: guard.clone(),
        }))
    }

    fn preview_source(&self) -> Result<String, String> {
        if self.format != Format::Wavedrom {
            return Ok(self.source.to_string());
        }
        let mut value = little_clip_core::diagram::wavejson::parse(&self.source)?;
        // Strip presentation titles only in the passive preview copy. Time
        // ticks, waveform labels, grouping and the editable source are retained.
        if let Some(root) = value.as_object_mut() {
            for key in ["head", "foot"] {
                if let Some(object) = root.get_mut(key).and_then(serde_json::Value::as_object_mut) {
                    object.remove("text");
                    if object.is_empty() {
                        root.remove(key);
                    }
                }
            }
        }
        serde_json::to_string(&value).map_err(|error| error.to_string())
    }

    pub fn render(&self, data: &mut Data, cancel: &AtomicBool) {
        let rendered = self
            .guard
            .check()
            .and_then(|_| self.preview_source())
            .and_then(|source| little_clip_render::diagram::render(self.format, &source, cancel))
            .and_then(|rendered| {
                self.guard.check()?;
                if cancel.load(Ordering::Relaxed) {
                    return Err("Diagram preview cancelled".into());
                }
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(1536);
                limits.max_image_height = Some(1024);
                limits.max_alloc = Some(8 * 1024 * 1024);
                let image = ImageThumbnail::generate_with_limits(
                    &rendered.png,
                    Some("image/png"),
                    1536,
                    1024,
                    limits,
                )
                .ok_or("Diagram preview pixels unavailable")?;
                self.guard.check()?;
                Ok((image, rendered.warnings))
            });
        match rendered {
            Ok((image, warnings)) => {
                data.image = Some(image);
                data.rendered_diagram = true;
                data.detail = warnings.join("\n");
            }
            Err(error) => data.detail = format!("Rendered preview unavailable: {error}"),
        }
    }
}

/// Shows the top of the diagram at a useful width, without shrinking a tall
/// drawing into an unreadable band. The same ready image supplies the popout.
pub(super) fn show(ui: &mut egui::Ui, rect: egui::Rect, id: uuid::Uuid, ready: &Ready) -> bool {
    if ready
        .data
        .document
        .as_ref()
        .is_none_or(|doc| doc.diagram.is_none())
    {
        return false;
    }
    let Some(texture) = &ready.texture else {
        return false;
    };
    let format = ready
        .data
        .document
        .as_ref()
        .and_then(|doc| doc.diagram.as_ref())
        .map(|diagram| diagram.format);
    let scale = match format {
        Some(Format::Wavedrom) => 0.25,
        // Component overviews need their relations and compartments visible.
        // Cropping the top can show only titles or the outer paper margin.
        Some(Format::Wireviz | Format::Nomnoml) => rect.height() / texture.size_vec2().y,
        _ => 0.5,
    };
    paint(ui, rect, texture, scale);
    if super::ui::preview_hovered(ui, rect) {
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new(("diagram_strip_hover", id)), |ui| {
            hover(ui, ready);
        });
    }
    true
}

pub(super) fn hover(ui: &mut egui::Ui, ready: &Ready) -> bool {
    let Some(diagram) = ready
        .data
        .document
        .as_ref()
        .and_then(|doc| doc.diagram.as_ref())
    else {
        return false;
    };
    ui.label(diagram.format.name());
    if !ready.data.detail.is_empty() {
        ui.label(&ready.data.detail);
    }
    let Some(texture) = &ready.texture else {
        return false;
    };
    let screen = ui.ctx().screen_rect();
    let width = (screen.width() - 40.0).clamp(160.0, 960.0);
    let max_height = (screen.height() * 0.6).clamp(100.0, 640.0);
    let scale = (width / texture.size_vec2().x).min(1.0);
    let size = egui::vec2(
        texture.size_vec2().x * scale,
        (texture.size_vec2().y * scale).min(max_height),
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint(ui, rect, texture, 1.0);
    true
}

fn paint(ui: &egui::Ui, rect: egui::Rect, texture: &egui::TextureHandle, max_scale: f32) {
    let original = texture.size_vec2();
    let mut scale = (rect.width() / original.x).min(max_scale);
    // Fit short diagrams completely; tall ones keep readable artwork and crop
    // vertically instead of collapsing the whole drawing into a tiny thumbnail.
    if original.y * scale <= rect.height() * 2.0 {
        scale = scale.min(rect.height() / original.y);
    }
    let size = original * scale;
    let image = egui::Rect::from_min_size(
        egui::pos2(
            rect.left(),
            rect.top() + (rect.height() - size.y).max(0.0) / 2.0,
        ),
        size,
    );
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.rect_filled(rect, 3.0, egui::Color32::WHITE);
    painter.image(
        texture.id(),
        image,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str, source: &str) -> Arc<super::super::text_strip::Document> {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(name);
        std::fs::write(&path, source).unwrap();
        super::super::text_strip::load(&path, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn diagram_sources_use_shared_classification_and_keep_text_fallbacks() {
        for (name, source, format) in [
            ("graph.gv", "digraph { A -> B }", Format::Graphviz),
            (
                "timing.json5",
                "{signal:[{name:'clk',wave:'p...'}]}",
                Format::Wavedrom,
            ),
            ("flow.mmd", "flowchart LR\n A --> B", Format::Mermaid),
            ("uml.nomnoml", "[Source] -> [Preview]", Format::Nomnoml),
            ("draft.noml", "[Unclosed", Format::Nomnoml),
            (
                "drawing.excalidraw.json",
                "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[]}",
                Format::Excalidraw,
            ),
        ] {
            let document = load(name, source);
            let diagram = document.diagram.as_ref().unwrap();
            assert_eq!(diagram.format, format);
            assert_eq!(&*diagram.source, source);
            assert!(
                document.lines > 0,
                "failed renders still retain the text summary"
            );
        }
        for (name, source) in [
            ("data.json", "{\"title\":\"ordinary data\"}"),
            ("page.mmd", "# Ordinary Markdown\n\nWords here."),
            ("data.xml", "<ordinary/>"),
            ("notes.txt", "digraph { A -> B }"),
        ] {
            assert!(load(name, source).diagram.is_none(), "{name}");
        }
        let oversized = format!("digraph {{\n{}\n}}", " ".repeat(256 * 1024));
        let document = load("large.gv", &oversized);
        assert!(document.diagram.is_none());
        assert!(document.note.contains("source budget"));
    }

    #[test]
    fn waveform_preview_omits_titles_but_keeps_time_ticks_and_original_source() {
        let source = "{head:{text:'Long title',tick:0},foot:{text:'Footer'},signal:[{name:'clock',wave:'p...'}]}";
        let document = load("clock.wavejson", source);
        let diagram = document.diagram.as_ref().unwrap();
        let passive: serde_json::Value =
            serde_json::from_str(&diagram.preview_source().unwrap()).unwrap();
        assert_eq!(passive["head"]["tick"], 0);
        assert!(passive["head"].get("text").is_none());
        assert!(passive.get("foot").is_none());
        assert_eq!(passive["signal"][0]["name"], "clock");
        assert_eq!(&*diagram.source, source);
    }
    #[test]
    fn replaced_diagram_source_is_rejected_before_rendering() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("graph.gv");
        std::fs::write(&path, "digraph { a -> b }").unwrap();
        let document = super::super::text_strip::load(&path, &AtomicBool::new(false)).unwrap();
        std::fs::write(&path, "digraph { replaced -> source }").unwrap();
        let mut data = Data::new(String::new(), String::new(), crate::icons::Icon::Text);
        document
            .diagram
            .as_ref()
            .unwrap()
            .render(&mut data, &AtomicBool::new(false));
        assert!(data.image.is_none());
        assert!(data.detail.contains("changed"), "{}", data.detail);
    }
}
