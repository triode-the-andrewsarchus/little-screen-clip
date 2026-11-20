//! Host-level regression coverage: every viewer shares the same folder return.
use super::*;
use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

fn escape() -> Event {
    Event::Key {
        key: Key::Escape,
        physical_key: Some(Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}
fn frame(viewers: &mut Viewers, ctx: &Context, events: Vec<Event>) -> (bool, egui::FullOutput) {
    let mut closed = false;
    let output = clip_ui::test_support::run_frame(
        ctx,
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 800.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    closed = viewers.ui(ui, &AppPalette::default());
                })
            });
        },
    );
    (closed, output)
}
fn open(path: &Path) -> (Viewers, Context) {
    let mut viewers = Viewers::default();
    let ctx = Context::default();
    egui_extras::install_image_loaders(&ctx);
    viewers.open(path.to_path_buf());
    super::tests::finish_loading(&mut viewers, &ctx);
    assert!(
        viewers.session.is_some(),
        "{}: {:?}",
        path.display(),
        viewers.error
    );
    (viewers, ctx)
}
#[test]
fn escape_returns_to_folder_for_all_read_only_viewer_families() {
    for fixture in [
        "tests/fixtures/notebooks/sample.ipynb",
        "tests/fixtures/comic.cbz",
        "tests/fixtures/database/harbor.sqlite",
        "tests/fixtures/fonts/latin.woff2",
        "../little-clip-core/tests/fixtures/ebooks/journey.epub",
        "../little-clip-core/tests/fixtures/geography/harbor.gpx",
        "../little-clip-core/tests/fixtures/gcode/frame.gcode",
        "../little-clip-core/tests/fixtures/multisample/harbor.sfz",
        "../little-clip-core/tests/fixtures/calendar/harbor.ics",
        "../little-clip-core/tests/fixtures/email/harbor.eml",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture);
        let (mut viewers, ctx) = open(&path);
        assert!(
            frame(&mut viewers, &ctx, vec![escape()]).0,
            "Escape did not return from {fixture}"
        );
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("example.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("note.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut zip, b"archive member").unwrap();
    zip.finish().unwrap();
    let (mut viewers, ctx) = open(&path);
    assert!(frame(&mut viewers, &ctx, vec![escape()]).0);
}
#[test]
fn popup_escape_is_consumed_before_folder_return() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../little-clip-core/tests/fixtures/calendar/harbor.ics");
    let (mut viewers, ctx) = open(&path);
    egui::Popup::open_id(&ctx, egui::Id::unique("viewer-popup"));
    assert!(!frame(&mut viewers, &ctx, vec![escape()]).0);
    assert!(!egui::Popup::is_any_open(&ctx));
    assert!(frame(&mut viewers, &ctx, vec![escape()]).0);
}
#[test]
fn escape_from_text_edit_mode_uses_the_host_close_path() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.txt");
    std::fs::write(&path, "unchanged draft").unwrap();
    let (mut viewers, ctx) = open(&path);
    let (_, output) = frame(&mut viewers, &ctx, vec![]);
    let target = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Edit text" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .expect("Edit text tab");
    let (_, output) = frame(
        &mut viewers,
        &ctx,
        vec![
            Event::PointerMoved(target),
            Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert!(
        output.shapes.iter().any(
            |shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Save")
        ),
        "Did not enter text edit mode"
    );
    assert!(frame(&mut viewers, &ctx, vec![escape()]).0);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "unchanged draft");
}
#[test]
fn rename_and_unsaved_confirmation_own_escape_before_folder_return() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255]))
        .save(&path)
        .unwrap();
    let (mut viewers, ctx) = open(&path);
    viewers.rename.open = true;
    assert!(!frame(&mut viewers, &ctx, vec![escape()]).0);
    assert!(!viewers.rename.open);
    let Some(ViewerSession::Image(state)) = &mut viewers.session else {
        panic!("image")
    };
    state.mark_pixels_changed();
    assert!(!frame(&mut viewers, &ctx, vec![escape()]).0);
    assert!(viewers.confirm_close);
    assert!(!frame(&mut viewers, &ctx, vec![escape()]).0);
    assert!(!viewers.confirm_close);
    assert!(viewers.session_dirty());
}
