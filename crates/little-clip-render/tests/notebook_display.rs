//! Real notebook ingestion/IPC thumbnails through the physical-screen renderer.
//! This owned harness dispatches child decoders before any test/application state.
use little_clip_core::{ClipItem, ClipPayload, SlotManager, config::AppConfig};
use little_clip_render::ScreenRenderer;
use std::path::Path;

fn main() {
    if little_clip_core::notebook::run_worker_cli() {
        return;
    }
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks");
    for (name, source) in [
        ("supernote", "supernote/demo-a5x-20230015-1to10.note"),
        ("xournal", "xournal/load/strokes.xopp"),
        ("remarkable", "remarkable/legacy-real/test_v5_math.rm"),
        (
            "remarkable-archive",
            "remarkable/generated/layered-template.rmn",
        ),
    ] {
        let item = ClipItem::from_text(root.join(source).to_string_lossy());
        // Match daemon-to-display transfer: full source bytes are omitted, but
        // the actual prepared thumbnail must still paint the notebook's ink.
        let item: ClipItem = serde_json::from_slice(&serde_json::to_vec(&item).unwrap()).unwrap();
        let ClipPayload::Image {
            bytes,
            thumbnail: Some(_),
            ..
        } = &item.payload
        else {
            panic!("{name}: missing notebook image preview");
        };
        assert!(bytes.is_empty());
        let mut slots = SlotManager::new();
        slots.push_left(item);
        let actual = renderer.render_pixmap(&slots).unwrap();
        assert_eq!((actual.width(), actual.height()), (1920, 462));
        let ClipPayload::Image {
            thumbnail: Some(thumbnail),
            ..
        } = &mut slots.left_hand.as_mut().unwrap().payload
        else {
            unreachable!()
        };
        // Keep source identity, dimensions, title, subtitle and slot chrome
        // unchanged. Only replace the prepared artwork with blank white paper.
        thumbnail.premultiplied_rgba.fill(255);
        let blank = renderer.render_pixmap(&slots).unwrap();
        let ink_difference = actual
            .pixels()
            .iter()
            .zip(blank.pixels())
            .filter(|(a, b)| {
                u32::from(a.red().abs_diff(b.red()))
                    + u32::from(a.green().abs_diff(b.green()))
                    + u32::from(a.blue().abs_diff(b.blue()))
                    > 80
            })
            .count();
        assert!(
            ink_difference > 100,
            "{name}: notebook ink must survive actual screen compositing ({ink_difference} pixels)"
        );
        if let Some(output) = std::env::var_os("NOTEBOOK_DISPLAY_REVIEW_DIR") {
            std::fs::create_dir_all(&output).unwrap();
            actual
                .save_png(Path::new(&output).join(format!("{name}.png")))
                .unwrap();
        }
        println!(
            "{name}: visible notebook ink in 1920x462 display frame ({ink_difference} pixels)"
        );
    }
}
