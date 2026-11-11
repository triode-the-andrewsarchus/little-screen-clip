//! File previews prepared by the frame worker before card rasterization.
use little_clip_core::{
    ClipItem, ClipPayload, ImageThumbnail,
    file_source::FileSource,
    midi::MidiData,
    spreadsheet::{self, SpreadsheetSheet},
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::Arc,
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
    Text,
}

struct Entry {
    source: FileSource,
    kind: Kind,
    extension: Option<String>,
    preview: Option<Arc<Preview>>,
    prepared: Instant,
}

#[derive(Default)]
pub(crate) struct Cache(VecDeque<Entry>);

impl crate::ScreenRenderer {
    pub(crate) fn file_preview(&self, item: &ClipItem) -> Option<Arc<Preview>> {
        let (path, documents) = match &item.payload {
            ClipPayload::FileBatch(files) if files.len() == 1 => (files[0].as_path(), true),
            ClipPayload::Text(text) => (Path::new(text.trim()), false),
            _ => return None,
        };
        let extension = path.extension().and_then(|value| value.to_str());
        let kind = if spreadsheet::is_spreadsheet_file(path) {
            Kind::Spreadsheet
        } else if extension.is_some_and(|value| {
            value.eq_ignore_ascii_case("mid") || value.eq_ignore_ascii_case("midi")
        }) {
            Kind::Midi
        } else if documents && little_clip_core::document::is_office_document(path) {
            Kind::Office
        } else if documents && little_clip_core::pdf::is_pdf_path(path) {
            Kind::Pdf
        } else if little_clip_core::syntax::is_code_file(path) {
            Kind::Text
        } else {
            return None;
        };
        let source = FileSource::capture(path).ok()?;
        {
            let mut cache = self
                .file_previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = cache.0.iter().position(|entry| {
                entry.source == source
                    && entry.kind == kind
                    && entry.extension.as_deref() == extension
            }) {
                let entry = cache.0.remove(index).expect("cache entry exists");
                if entry.preview.is_some() || entry.prepared.elapsed() < RETRY_AFTER {
                    let preview = entry.preview.clone();
                    cache.0.push_back(entry);
                    return preview;
                }
            }
        }
        let preview = load(path, kind, extension).map(Arc::new);
        if FileSource::capture(path).ok().as_ref() != Some(&source) {
            return None;
        }
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.0.retain(|entry| entry.source.path() != source.path());
        if cache.0.len() >= MAX_ENTRIES {
            cache.0.pop_front();
        }
        cache.0.push_back(Entry {
            source,
            kind,
            extension: extension.map(str::to_owned),
            preview: preview.clone(),
            prepared: Instant::now(),
        });
        preview
    }
}

fn load(path: &Path, kind: Kind, extension: Option<&str>) -> Option<Preview> {
    match kind {
        Kind::Pdf | Kind::Office => load_page(path, kind == Kind::Office).map(Preview::Page),
        Kind::Spreadsheet => {
            let data = spreadsheet::load_spreadsheet(path).ok()?;
            let mut sheet = data.sheets.into_iter().next()?;
            // Only the visible corner belongs to the card cache. Keep original dimensions.
            sheet.rows = sheet.upper_left_slice(9, 4);
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
        Kind::Text => {
            let contents = little_clip_core::kernel_interface::read_content_to_string_bounded(
                path,
                1024 * 1024,
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
