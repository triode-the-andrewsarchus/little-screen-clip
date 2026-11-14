//! Reuses each format's bounded header/package reader, never rendered previews.
use super::{
    Entry, ResultSummary, Summary,
    sorting::{self, Category},
};
use std::sync::atomic::{AtomicBool, Ordering};

fn epub(entry: &Entry) -> bool {
    entry
        .path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
}
pub(super) fn supported(entry: &Entry) -> bool {
    little_clip_core::audio::is_audio_path(&entry.path)
        || entry.is_photo()
        || entry.is_video()
        || little_clip_core::pdf::is_pdf_path(&entry.path)
        || epub(entry)
        || little_clip_core::notebook::is_candidate_path(&entry.path)
}
pub(super) fn load(entry: &Entry, cancel: &AtomicBool) -> ResultSummary {
    if cancel.load(Ordering::Relaxed) {
        return Err("Metadata read cancelled".into());
    }
    super::super::filesystem::check_revision(&entry.path, entry.revision)?;
    let source = crate::file_source::FileSource::capture(&entry.path)?;
    let mut summary = match sorting::family(entry) {
        Category::Audio => audio(entry, cancel),
        Category::Image => photo(entry, cancel)?,
        Category::Video => video(entry, cancel)?,
        Category::Document => document(entry, cancel)?,
        _ => return Err("No metadata header reader for this content family".into()),
    };
    source.check()?;
    super::super::filesystem::check_revision(&entry.path, entry.revision)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Metadata read cancelled".into());
    }
    for text in [
        &mut summary.artist,
        &mut summary.album,
        &mut summary.genre,
        &mut summary.codec,
        &mut summary.title,
        &mut summary.author,
    ] {
        *text = text.trim().chars().take(512).collect();
    }
    Ok(summary)
}
fn audio(entry: &Entry, cancel: &AtomicBool) -> Summary {
    let metadata = super::super::music::preview::metadata(&entry.path, cancel);
    Summary {
        artist: metadata.artist,
        album: metadata.album,
        genre: metadata.genre,
        duration: (metadata.duration.is_finite() && metadata.duration > 0.0)
            .then(|| (f64::from(metadata.duration) * 1000.0) as u64),
        ..Default::default()
    }
}
fn photo(entry: &Entry, cancel: &AtomicBool) -> ResultSummary {
    let metadata = little_clip_core::gallery_image::metadata::probe_cancellable(
        &entry.path,
        entry.revision,
        cancel,
    )?;
    let (width, height) = if (5..=8).contains(&metadata.orientation) {
        (metadata.height, metadata.width)
    } else {
        (metadata.width, metadata.height)
    };
    Ok(Summary {
        width: Some(width),
        height: Some(height),
        ..Default::default()
    })
}
fn video(entry: &Entry, cancel: &AtomicBool) -> ResultSummary {
    let metadata = super::super::media::inspect_video_header(&entry.path, cancel)?;
    Ok(Summary {
        width: Some(metadata.dimensions.0),
        height: Some(metadata.dimensions.1),
        codec: metadata.codec,
        duration: (metadata.duration_ms > 0).then_some(metadata.duration_ms),
        ..Default::default()
    })
}
fn document(entry: &Entry, cancel: &AtomicBool) -> ResultSummary {
    if little_clip_core::pdf::is_pdf_path(&entry.path) {
        let metadata = little_clip_core::pdf::get_pdf_info_cancellable(&entry.path, cancel)?;
        Ok(Summary {
            title: metadata.title.unwrap_or_default(),
            author: metadata.author.unwrap_or_default(),
            pages: metadata.page_count.try_into().ok(),
            ..Default::default()
        })
    } else if epub(entry) {
        let metadata = little_clip_core::ebook::read_metadata(&entry.path, cancel)?;
        Ok(Summary {
            title: metadata.title,
            author: metadata.authors.join("; "),
            ..Default::default()
        })
    } else if little_clip_core::notebook::is_candidate_path(&entry.path) {
        let metadata = little_clip_core::notebook::inspect(&entry.path, cancel)?;
        Ok(Summary {
            pages: metadata.pages.len().try_into().ok(),
            ..Default::default()
        })
    } else {
        Err("Document tags are not available for this format".into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Field, Value};
    use super::*;
    fn entry(path: &std::path::Path) -> Entry {
        super::super::super::filesystem::read_directory(
            path.parent().unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap()
    }
    #[test]
    fn header_only_photos_provide_dimensions_without_decodable_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("header.png");
        image::RgbImage::new(48, 32).save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..33]).unwrap();
        assert!(image::open(&path).is_err());
        let summary = load(&entry(&path), &AtomicBool::new(false)).unwrap();
        assert_eq!((summary.width, summary.height), (Some(48), Some(32)));
        assert!(summary.value(Field::Pixels) == Some(Value::Number(1536)));
    }
    #[test]
    fn ordinary_books_and_video_headers_share_their_existing_source_readers() {
        let directory = tempfile::tempdir().unwrap();
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures");
        for (source, name) in [
            ("ebooks/journey.epub", "book.epub"),
            ("video/with-cover.mp4", "video.mp4"),
        ] {
            let path = directory.path().join(name);
            std::fs::copy(fixtures.join(source), &path).unwrap();
            let summary = load(&entry(&path), &AtomicBool::new(false)).unwrap();
            if name.ends_with("epub") {
                assert_eq!(summary.title, "A Small Journey");
                assert_eq!(summary.author, "Example Author");
                assert!(summary.pages.is_none());
            } else {
                assert_eq!((summary.width, summary.height), (Some(160), Some(90)));
                assert!(!summary.codec.is_empty());
            }
            assert!(load(&entry(&path), &AtomicBool::new(true)).is_err());
        }
    }
    #[test]
    fn notebook_hints_do_not_confuse_cells_with_handwritten_pages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("notebook.ipynb");
        std::fs::write(&path, "{}").unwrap();
        let mut notebook = entry(&path);
        assert!(!supported(&notebook));
        notebook.path.set_extension("xopp");
        assert!(supported(&notebook));
        assert_eq!(sorting::family(&notebook), Category::Document);
    }
}
