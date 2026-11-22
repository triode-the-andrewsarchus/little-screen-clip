//! File previews prepared by the frame worker before card rasterization.
use little_clip_core::{
    ClipItem, ClipPayload, ImageThumbnail,
    brushes::BrushLibrary,
    file_format::Format,
    file_source::FileSource,
    geography::{Document, places, trajectory::Trajectory},
    midi::MidiData,
    spreadsheet::{self, SpreadsheetSheet},
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 8;
const RETRY_AFTER: Duration = Duration::from_secs(10);

pub(crate) struct Page {
    pub width: u32,
    pub height: u32,
    pub thumbnail: ImageThumbnail,
}

pub(crate) enum Preview {
    Page(Page),
    Spreadsheet {
        title: String,
        sheet: SpreadsheetSheet,
    },
    Midi(MidiData),
    Brush(BrushLibrary),
    Map(super::map_preview::Map),
    Text {
        contents: String,
        extension: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pdf,
    Office,
    Spreadsheet,
    Midi,
    Brush,
    Geography,
    Trajectory,
    Text,
}

struct Entry {
    source: FileSource,
    kind: Kind,
    extension: Option<String>,
    preview: Option<Arc<Preview>>,
    prepared: Instant,
}

type CachedPreview = Option<Arc<Preview>>;

#[derive(Default)]
pub(crate) struct Cache {
    files: VecDeque<Entry>,
    locations: VecDeque<(String, CachedPreview)>,
}

impl crate::ScreenRenderer {
    /// Reports whether the last card render prepared a map for this path.
    pub fn cached_map_for_file(&self, path: &Path) -> bool {
        self.file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .files
            .iter()
            .any(|entry| {
                entry.source.path() == path
                    && matches!(entry.preview.as_deref(), Some(Preview::Map(_)))
            })
    }

    pub(crate) fn file_preview(&self, item: &ClipItem) -> Option<Arc<Preview>> {
        self.path_preview(item).or_else(|| match &item.payload {
            ClipPayload::Text(text) if text.trim().len() <= 128 => {
                self.location_preview(text.trim())
            }
            _ => None,
        })
    }

    fn path_preview(&self, item: &ClipItem) -> Option<Arc<Preview>> {
        let (path, documents) = match &item.payload {
            ClipPayload::FileBatch(files) if files.len() == 1 => (files[0].as_path(), true),
            ClipPayload::Text(text) => (Path::new(text.trim()), false),
            _ => return None,
        };
        let extension = little_clip_core::diagram::Format::source_extension_hint(path);
        let kind = file_kind(path, documents, extension)?;
        let source = FileSource::capture(path).ok()?;
        {
            let mut cache = self
                .file_previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = cache.files.iter().position(|entry| {
                entry.source == source
                    && entry.kind == kind
                    && entry.extension.as_deref() == extension
            }) {
                let entry = cache.files.remove(index).expect("cache entry exists");
                if entry.preview.is_some() || entry.prepared.elapsed() < RETRY_AFTER {
                    let preview = entry.preview.clone();
                    cache.files.push_back(entry);
                    return preview;
                }
            }
        }
        let preview = self.load_path_preview(path, kind, extension).map(Arc::new);
        if FileSource::capture(path).ok().as_ref() != Some(&source) {
            return None;
        }
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache
            .files
            .retain(|entry| entry.source.path() != source.path());
        if cache.files.len() >= MAX_ENTRIES {
            cache.files.pop_front();
        }
        cache.files.push_back(Entry {
            source,
            kind,
            extension: extension.map(str::to_owned),
            preview: preview.clone(),
            prepared: Instant::now(),
        });
        preview
    }

    fn load_path_preview(
        &self,
        path: &Path,
        kind: Kind,
        extension: Option<&str>,
    ) -> Option<Preview> {
        if kind == Kind::Geography {
            return self.earth_preview(&Document::from_path(path).ok()?);
        }
        if let Ok(Some(trajectory)) = Trajectory::read_if_identified(path, &AtomicBool::new(false))
        {
            return crate::render_trajectory_thumbnail(&trajectory).map(|thumbnail| {
                Preview::Map(super::map_preview::Map {
                    thumbnail,
                    label: Some(trajectory.name),
                })
            });
        }
        load(path, kind, extension)
    }

    fn earth_preview(&self, document: &Document) -> Option<Preview> {
        let texture = self
            .map_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .earth(&self.config.geo_assets_dir());
        super::map_preview::Map::earth(self, document, texture.as_ref()).map(Preview::Map)
    }

    fn location_preview(&self, text: &str) -> CachedPreview {
        {
            let mut cache = self
                .file_previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = cache.locations.iter().position(|(key, _)| key == text) {
                let entry = cache.locations.remove(index).expect("cache entry exists");
                let result = entry.1.clone();
                cache.locations.push_back(entry);
                return result;
            }
        }
        let result = places::resolve(text)
            .and_then(|location| Document::from_location(&location).ok())
            .and_then(|document| self.earth_preview(&document))
            .map(Arc::new);
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cache.locations.len() >= MAX_ENTRIES {
            cache.locations.pop_front();
        }
        cache.locations.push_back((text.to_owned(), result.clone()));
        result
    }
}

fn file_kind(path: &Path, documents: bool, extension: Option<&str>) -> Option<Kind> {
    let kind = if Format::for_path(path) == Some(Format::Geography) {
        Kind::Geography
    } else if spreadsheet::is_spreadsheet_file(path) {
        Kind::Spreadsheet
    } else if extension.is_some_and(|value| {
        value.eq_ignore_ascii_case("mid") || value.eq_ignore_ascii_case("midi")
    }) {
        Kind::Midi
    } else if little_clip_core::brushes::is_brush_file(path) {
        Kind::Brush
    } else if documents && little_clip_core::document::is_office_document(path) {
        Kind::Office
    } else if documents && little_clip_core::pdf::is_pdf_path(path) {
        Kind::Pdf
    } else if little_clip_core::syntax::is_code_file(path) {
        Kind::Text
    } else if Trajectory::is_candidate(path) {
        Kind::Trajectory
    } else {
        return None;
    };
    Some(kind)
}

fn load(path: &Path, kind: Kind, extension: Option<&str>) -> Option<Preview> {
    match kind {
        Kind::Geography | Kind::Trajectory => None,
        Kind::Pdf | Kind::Office => load_page(path, kind == Kind::Office).map(Preview::Page),
        Kind::Spreadsheet => {
            let data = spreadsheet::load_spreadsheet_preview(
                path,
                9,
                10,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .ok()?;
            let sheet = data.sheets.into_iter().next()?;
            Some(Preview::Spreadsheet {
                title: data.title,
                sheet,
            })
        }
        Kind::Midi => {
            let mut score = MidiData::parse_file(path).ok()?;
            // Keep notes across the full timeline while bounding dense scores.
            let stride = score.notes.len().div_ceil(16_384).max(1);
            if stride > 1 {
                score.notes = score.notes.into_iter().step_by(stride).collect();
            }
            Some(Preview::Midi(score))
        }
        Kind::Brush => {
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            little_clip_core::brushes::load_brush_library(path, 6, &cancelled)
                .ok()
                .map(Preview::Brush)
        }
        Kind::Text => {
            let contents = little_clip_core::kernel_interface::read_content_to_string_bounded(
                path,
                if extension == Some("excalidraw") {
                    little_clip_core::diagram::MAX_DRAWING_SOURCE_BYTES
                } else {
                    1024 * 1024
                },
            )
            .ok()?;
            Some(Preview::Text {
                contents,
                extension: extension.map(str::to_owned),
            })
        }
    }
}

fn load_page(path: &Path, office: bool) -> Option<Page> {
    let converted;
    let pdf = if office {
        converted = little_clip_core::document::ensure_rendered_pdf(path).ok()?;
        converted.as_path()
    } else {
        path
    };
    let (png, width, height) =
        little_clip_core::pdf::render_pdf_page_thumbnail(pdf, 1, 800).ok()?;
    let thumbnail = ImageThumbnail::generate(&png, 320, 220)?;
    Some(Page {
        width,
        height,
        thumbnail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lincoln_clipboard_map_contains_a_pin_and_an_inset_city_label() {
        let root = tempfile::tempdir().unwrap();
        let mut config = little_clip_core::AppConfig::default();
        config.geography.assets_dir = Some(root.path().to_owned());
        let renderer = crate::ScreenRenderer::new(config).unwrap();
        let location = ClipItem::from_text("40.813599 -96.702606");
        let preview = renderer.file_preview(&location).unwrap();
        let Preview::Map(map) = preview.as_ref() else {
            panic!("Expected a map preview")
        };
        let pixels = &map.thumbnail.premultiplied_rgba;
        assert!(
            pixels
                .chunks_exact(4)
                .any(|rgba| rgba == [115, 241, 211, 255])
        );
        assert!(
            pixels
                .chunks_exact(4)
                .any(|rgba| rgba == [255, 255, 255, 255])
        );
    }

    #[test]
    fn maps_are_prepared_for_geography_and_standalone_clipboard_locations() {
        let renderer = crate::ScreenRenderer::fallback(Default::default());
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.gpx");
        let item = ClipItem::from_files(vec![fixture]);
        let preview = renderer.file_preview(&item).unwrap();
        let Preview::Map(map) = preview.as_ref() else {
            panic!("Expected a map preview")
        };
        assert!(
            map.thumbnail
                .premultiplied_rgba
                .chunks_exact(4)
                .any(|rgba| rgba == [115, 241, 211, 255])
        );

        for text in ["41.8781, -87.6298", "Chicago"] {
            let item = ClipItem::from_text(text);
            assert!(matches!(
                renderer.file_preview(&item).as_deref(),
                Some(Preview::Map(_))
            ));
            assert_eq!(item.to_clipboard_text().as_deref(), Some(text));
        }
        let sentence = ClipItem::from_text("The trip starts in Chicago.");
        assert!(renderer.file_preview(&sentence).is_none());
    }
}
