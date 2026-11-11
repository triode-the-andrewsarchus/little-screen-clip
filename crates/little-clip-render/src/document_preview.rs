//! First-page previews prepared by the frame worker before card rasterization.
use little_clip_core::{ClipItem, ClipPayload, ImageThumbnail, file_source::FileSource};
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

struct Entry {
    source: FileSource,
    office: bool,
    page: Option<Arc<Page>>,
    prepared: Instant,
}

#[derive(Default)]
pub(crate) struct Cache(VecDeque<Entry>);

impl crate::ScreenRenderer {
    pub(crate) fn document_preview(&self, item: &ClipItem) -> Option<Arc<Page>> {
        let ClipPayload::FileBatch(files) = &item.payload else {
            return None;
        };
        let [path] = files.as_slice() else {
            return None;
        };
        let office = little_clip_core::document::is_office_document(path);
        if !office && !little_clip_core::pdf::is_pdf_path(path) {
            return None;
        }
        let source = FileSource::capture(path).ok()?;
        {
            let mut cache = self
                .document_previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = cache
                .0
                .iter()
                .position(|entry| entry.source == source && entry.office == office)
            {
                let entry = cache.0.remove(index).expect("cache entry exists");
                if entry.page.is_some() || entry.prepared.elapsed() < RETRY_AFTER {
                    let page = entry.page.clone();
                    cache.0.push_back(entry);
                    return page;
                }
            }
        }
        let page = load_page(path, office).map(Arc::new);
        if FileSource::capture(path).ok().as_ref() != Some(&source) {
            return None;
        }
        let mut cache = self
            .document_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.0.retain(|entry| entry.source.path() != source.path());
        if cache.0.len() >= MAX_ENTRIES {
            cache.0.pop_front();
        }
        cache.0.push_back(Entry {
            source,
            office,
            page: page.clone(),
            prepared: Instant::now(),
        });
        page
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
