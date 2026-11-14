//! Real subprocess acceptance checks; this executable also dispatches its child.
use little_clip_core::notebook;
use std::{path::Path, sync::atomic::AtomicBool};

fn main() {
    if notebook::run_worker_cli() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/examples/notebooks/supernote/demo-a5x-20230015-1to10.note");
    let path = directory.path().join("arbitrary-extension.bin");
    std::fs::copy(source, &path).unwrap();
    let cancel = AtomicBool::new(false);
    let info = notebook::inspect(&path, &cancel).unwrap();
    assert_eq!(info.format, "Supernote");
    assert_eq!(info.pages.len(), 10);
    assert_eq!((info.pages[0].width, info.pages[0].height), (1404, 1872));
    let (png, width, height) = notebook::render_page(&path, 10, 320, &cancel).unwrap();
    assert_eq!((width, height), (240, 320));
    assert!(
        image::load_from_memory(&png)
            .unwrap()
            .to_rgb8()
            .pixels()
            .any(|pixel| pixel.0 != [255; 3])
    );
    assert!(notebook::render_page(&path, 11, 320, &cancel).is_err());
    assert!(notebook::render_page(&path, 0, 320, &cancel).is_err());
    assert!(notebook::render_page(&path, 1, 4097, &cancel).is_err());
    assert!(notebook::inspect(&path, &AtomicBool::new(true)).is_err());
    let note = directory.path().join("clipboard.NOTE");
    std::fs::copy(&path, &note).unwrap();
    let item = little_clip_core::ClipItem::from_text(note.to_string_lossy());
    assert!(item.subtitle.contains("Supernote (10 pg)"));
    let little_clip_core::ClipPayload::Image {
        bytes,
        path: original,
        thumbnail,
        width,
        height,
        ..
    } = &item.payload
    else {
        panic!("Notebook clipboard paths must produce page artwork");
    };
    assert_eq!(original.as_deref(), Some(note.as_path()));
    assert_eq!((*width, *height), (600, 800));
    assert!(bytes.starts_with(b"\x89PNG"));
    let thumbnail = thumbnail.as_ref().unwrap();
    assert!(thumbnail.width <= 320 && thumbnail.height <= 220);
    assert!(
        thumbnail
            .premultiplied_rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[..3] != [255; 3])
    );
    let wire = serde_json::to_vec(&item).unwrap();
    let received: little_clip_core::ClipItem = serde_json::from_slice(&wire).unwrap();
    let (restored, _, restored_path) = received.resolve_image_data().unwrap();
    assert_eq!(restored, *bytes);
    assert_eq!(restored_path.as_deref(), Some(note.as_path()));
    std::fs::write(&note, b"A normal note, not a handwritten notebook.").unwrap();
    let plain = little_clip_core::ClipItem::from_text(note.to_string_lossy());
    assert!(!matches!(
        plain.payload,
        little_clip_core::ClipPayload::Image { .. }
    ));
    std::fs::write(&path, b"A normal file with a misleading .note extension.").unwrap();
    assert!(
        notebook::inspect(&path, &cancel)
            .unwrap_err()
            .contains("header")
    );
    assert!(notebook::inspect(Path::new("/dev/zero"), &cancel).is_err());
    println!(
        "Notebook worker acceptance passed: header qualification, metadata, late page ink, dimensions, invalid pages, cancellation and protected inputs"
    );
}
