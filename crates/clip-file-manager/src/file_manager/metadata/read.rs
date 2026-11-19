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
        || entry.is_vcv_patch()
        || entry.is_photo()
        || entry.is_video()
        || entry.is_brush()
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
        Category::VcvRack => vcv_patch(entry, cancel)?,
        Category::Image => photo(entry, cancel)?,
        Category::Video => video(entry, cancel)?,
        Category::Document => document(entry, cancel)?,
        Category::Brushes => Summary {
            brush_count: Some(
                little_clip_core::brushes::count_brushes(&entry.path, cancel)? as u64,
            ),
            ..Default::default()
        },
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
fn vcv_patch(entry: &Entry, cancel: &AtomicBool) -> ResultSummary {
    let bytes = crate::file_source::read_file_snapshot(
        &entry.path,
        little_clip_core::vcv::MAX_PATCH_BYTES as usize,
    )?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Metadata read cancelled".into());
    }
    let document = little_clip_core::vcv::read_patch_bytes_for_path(&entry.path, &bytes)?;
    Ok(Summary {
        modules: document.modules.len().try_into().ok(),
        cables: document.cables.len().try_into().ok(),
        rows: document.row_count().try_into().ok(),
        ..Default::default()
    })
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

    #[test]
    fn vcv_patch_headers_read_modules_cables_and_rows_across_formats() {
        use std::io::Write;

        let directory = tempfile::tempdir().unwrap();
        let json_bytes = serde_json::to_vec(&serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Fundamental", "model": "VCO", "pos": [0, 0] },
                { "id": 2, "plugin": "Fundamental", "model": "VCF", "pos": [10, 0] },
                { "id": 3, "plugin": "Fundamental", "model": "VCA-1", "pos": [0, 1] },
                { "id": 4, "plugin": "Core", "model": "AudioInterface2", "pos": [8, 2] },
            ],
            "cables": [
                { "id": 1, "outputModuleId": 1, "outputId": 0, "inputModuleId": 2, "inputId": 0 },
                { "id": 2, "outputModuleId": 2, "outputId": 0, "inputModuleId": 3, "inputId": 0 },
                { "id": 3, "outputModuleId": 3, "outputId": 0, "inputModuleId": 4, "inputId": 0 },
            ]
        }))
        .unwrap();

        let raw_path = directory.path().join("raw.vcv");
        std::fs::write(&raw_path, &json_bytes).unwrap();

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_path("patch.json").unwrap();
            header.set_size(json_bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, json_bytes.as_slice()).unwrap();
            builder.finish().unwrap();
        }
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
        encoder.write_all(&tar_bytes).unwrap();
        let archive_bytes = encoder.finish().unwrap();

        let archive_path = directory.path().join("archive.vcv");
        std::fs::write(&archive_path, &archive_bytes).unwrap();

        for path in [&raw_path, &archive_path] {
            let file = entry(path);
            assert!(supported(&file));
            assert_eq!(sorting::family(&file), Category::VcvRack);
            let summary = load(&file, &AtomicBool::new(false)).unwrap();
            assert_eq!(summary.value(Field::Modules), Some(Value::Number(4)));
            assert_eq!(summary.value(Field::Cables), Some(Value::Number(3)));
            assert_eq!(summary.value(Field::Rows), Some(Value::Number(3)));
            assert!(load(&file, &AtomicBool::new(true)).is_err());
        }

        let strip_path = directory.path().join("chain.vcvss");
        std::fs::write(
            &strip_path,
            serde_json::to_vec(&serde_json::json!({
                "stripVersion": 1,
                "leftModules": [
                    { "id": 1, "plugin": "Fundamental", "model": "VCO" }
                ],
                "leftWidth": 150.0,
                "rightModules": [
                    { "id": 2, "plugin": "Fundamental", "model": "VCF" },
                    { "id": 3, "plugin": "Fundamental", "model": "VCA-1" }
                ],
                "rightWidth": 240.0,
                "cables": [
                    { "outputModuleId": 1, "outputId": 0, "inputModuleId": 2, "inputId": 0 }
                ],
                "version": "2.6.0"
            }))
            .unwrap(),
        )
        .unwrap();
        let strip_file = entry(&strip_path);
        assert!(supported(&strip_file));
        assert_eq!(sorting::family(&strip_file), Category::VcvRack);
        let strip_summary = load(&strip_file, &AtomicBool::new(false)).unwrap();
        assert_eq!(strip_summary.value(Field::Modules), Some(Value::Number(3)));
        assert_eq!(strip_summary.value(Field::Cables), Some(Value::Number(1)));
        assert_eq!(strip_summary.value(Field::Rows), Some(Value::Number(0)));
        assert!(load(&strip_file, &AtomicBool::new(true)).is_err());
    }
}
