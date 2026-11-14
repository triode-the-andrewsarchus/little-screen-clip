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
