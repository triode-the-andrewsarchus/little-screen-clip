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
    assert!(notebook::has_header(&path, &cancel).unwrap());
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
    assert!(!notebook::has_header(&note, &cancel).unwrap());
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
    xournal_sources();
    remarkable_sources();
    println!(
        "Notebook worker acceptance passed: header qualification, metadata, late page ink, dimensions, invalid pages, cancellation and protected inputs"
    );
}

fn xournal_sources() {
    use little_clip_core::{ClipItem, ClipPayload};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks/xournal");
    let cancel = AtomicBool::new(false);
    for (name, page, count) in [
        ("legacy-stroke.xoj", 1, 1),
        ("load/pages.xopp", 11, 11),
        ("load/latex-fileversion-5.xopp", 1, 1),
        ("packaged/image.xopp", 1, 1),
        ("packaged/pdf-background.xopp", 2, 2),
    ] {
        let path = root.join(name);
        assert!(notebook::is_candidate_path(&path));
        assert!(notebook::has_header(&path, &cancel).unwrap(), "{name}");
        let info = notebook::inspect(&path, &cancel).unwrap();
        assert_eq!(info.format, "Xournal");
        assert_eq!(info.pages.len(), count, "{name}");
        let (png, width, height) = notebook::render_page(&path, page, 320, &cancel).unwrap();
        assert!(width <= 320 && height <= 320);
        assert!(
            image::load_from_memory(&png)
                .unwrap()
                .to_rgb8()
                .pixels()
                .any(|pixel| pixel.0 != [255; 3]),
            "{name}"
        );
        let item = ClipItem::from_text(path.to_string_lossy());
        assert!(
            item.subtitle.contains(&format!("Xournal ({count} pg)")),
            "{}",
            item.subtitle
        );
        let ClipPayload::Image {
            thumbnail: Some(thumbnail),
            path: Some(original),
            ..
        } = &item.payload
        else {
            panic!("{name}: missing notebook clipboard artwork");
        };
        assert_eq!(original, &path);
        assert!(thumbnail.width > 0 && thumbnail.height > 0);
        let transferred: ClipItem =
            serde_json::from_slice(&serde_json::to_vec(&item).unwrap()).unwrap();
        let (restored, _, original) = transferred.resolve_image_data().unwrap();
        assert_eq!(original.as_deref(), Some(path.as_path()));
        assert!(restored.starts_with(b"\x89PNG"));
    }
    let directory = tempfile::tempdir().unwrap();
    let fake = directory.path().join("ordinary.xopp");
    std::fs::write(
        &fake,
        b"<not-a-notebook><page width='10' height='20'/></not-a-notebook>",
    )
    .unwrap();
    assert!(!notebook::has_header(&fake, &cancel).unwrap());
    assert!(notebook::inspect(&fake, &cancel).is_err());
    assert!(!matches!(
        ClipItem::from_text(fake.to_string_lossy()).payload,
        ClipPayload::Image { .. }
    ));
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(
        &mut gzip,
        b"<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>",
    )
    .unwrap();
    std::fs::write(&fake, gzip.finish().unwrap()).unwrap();
    assert!(
        !notebook::has_header(&fake, &cancel).unwrap(),
        "A gzip container does not prove Xournal content"
    );
    std::fs::write(
        &fake,
        format!(
            "{}<xournal><page width='100' height='200'><layer/></page></xournal>",
            " ".repeat(128)
        ),
    )
    .unwrap();
    assert!(
        notebook::has_header(&fake, &cancel).unwrap(),
        "Indented plain XML must retain header qualification"
    );
    std::fs::write(&fake,b"<xournal><page width='1' height='1'><background type='pdf' filename='/dev/zero'/><layer/></page></xournal>").unwrap();
    assert!(notebook::has_header(&fake, &cancel).unwrap());
    let error = notebook::render_page(&fake, 1, 128, &cancel).unwrap_err();
    assert!(
        error.contains("background unavailable"),
        "Protected background input must be rejected before reading: {error}"
    );
    println!(
        "Xournal worker/clipboard acceptance passed for legacy, gzip, LaTeX and ZIP assets, including rehydration and rejected impostors/protected backgrounds"
    );
}

fn remarkable_sources() {
    use little_clip_core::{ClipItem, ClipPayload};
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks/remarkable");
    let cancel = AtomicBool::new(false);
    for (file, page, count, warned) in [
        ("legacy-real/test_v5_math.rm", 1, 1, false),
        ("rmscene/With_SceneInfo_Block.rm", 1, 1, false),
        ("archives/Test.rmdoc", 2, 2, true),
        ("generated/layered-template.rmn", 2, 2, false),
        ("generated/legacy-paper.rmn", 2, 2, false),
        ("generated/redirected-pdf.rmdoc", 3, 3, false),
    ] {
        let path = root.join(file);
        let before = std::fs::read(&path).unwrap();
        assert!(notebook::is_candidate_path(&path));
        assert!(notebook::has_header(&path, &cancel).unwrap(), "{file}");
        let info = notebook::inspect(&path, &cancel).unwrap();
        assert_eq!(info.format, "reMarkable");
        assert_eq!(info.pages.len(), count);
        assert_eq!(!info.warnings.is_empty(), warned);
        let (png, width, height) = notebook::render_page(&path, page, 512, &cancel).unwrap();
        assert!(width <= 512 && height <= 512);
        assert_eq!(image::load_from_memory(&png).unwrap().width(), width);
        let item = ClipItem::from_text(path.to_string_lossy());
        assert!(
            matches!(&item.payload,ClipPayload::Image { path:Some(original), thumbnail:Some(_), .. } if original == &path),
            "{file}"
        );
        assert_eq!(item.subtitle.contains("paper template unavailable"), warned);
        let transferred: ClipItem =
            serde_json::from_slice(&serde_json::to_vec(&item).unwrap()).unwrap();
        let (restored, _, original) = transferred.resolve_image_data().unwrap();
        assert!(restored.starts_with(b"\x89PNG"));
        assert_eq!(original.as_deref(), Some(path.as_path()));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(notebook::render_page(&path, count + 1, 128, &cancel).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("realmedia.rm");
    std::fs::write(&path, b".RMF\0\0\0\x12\0\0 ordinary non-notebook content").unwrap();
    assert!(
        !notebook::has_header(&path, &cancel).unwrap(),
        "An .rm suffix must not claim RealMedia"
    );
    assert!(notebook::inspect(&path, &cancel).is_err());
    assert!(!matches!(
        ClipItem::from_text(path.to_string_lossy()).payload,
        ClipPayload::Image { .. }
    ));
    let ordinary = directory.path().join("ordinary.rmdoc");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&ordinary).unwrap());
    zip.start_file("ordinary.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut zip, b"not a notebook").unwrap();
    drop(zip.finish().unwrap());
    assert!(!notebook::has_header(&ordinary, &cancel).unwrap());
    println!(
        "reMarkable worker/clipboard acceptance passed: real v5/v6, RMDOC/RMN templates and PDF pages, notices, rehydration, unchanged sources and rejected extension collisions"
    );
}
