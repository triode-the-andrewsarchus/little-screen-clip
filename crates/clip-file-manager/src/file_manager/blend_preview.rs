//! Saved Blender thumbnails from the shared bounded native reader.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Ui, pos2, vec2};
use little_clip_core::ImageThumbnail;
use std::{path::Path, sync::atomic::AtomicBool};

pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Blender project".into(), Icon::Model3d);
    match thumbnail(path, token) {
        Ok(image) => data.image = Some(image),
        Err(error) => data.detail = error,
    }
    data
}

fn thumbnail(path: &Path, token: &AtomicBool) -> Result<ImageThumbnail, String> {
    little_clip_core::blend_image::thumbnail(path, token)
}

pub(super) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: Option<super::ui::ListColumns>,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) {
    let body = super::ui::information_strip(ui, row, entry, "BLEND", columns, colors);
    let thumbnail = Rect::from_min_size(body.min, vec2(body.height() * 16.0 / 9.0, body.height()));
    let texture = ready.and_then(|ready| ready.texture.as_ref());
    super::ui::model_thumbnail(ui, thumbnail, texture, colors);
    let details = Rect::from_min_max(pos2(thumbnail.right() + 12.0, body.top()), body.max);
    let message = ready.map_or("Loading Blender thumbnail…", |ready| {
        ready.data.detail.as_str()
    });
    ui.painter()
        .with_clip_rect(details.intersect(ui.clip_rect()))
        .text(
            details.left_center(),
            Align2::LEFT_CENTER,
            message,
            FontId::proportional(13.0),
            colors.text_secondary,
        );
    if super::ui::preview_hovered(ui, thumbnail)
        && let Some(texture) = texture
    {
        let screen = ui.ctx().screen_rect().shrink(8.0);
        let bounds = vec2(
            screen.width().min(640.0),
            (screen.bottom() - thumbnail.bottom() - 6.0).min(480.0),
        );
        if bounds.y > 16.0 {
            let size = texture.size_vec2();
            let scale = (bounds.x / size.x).min(bounds.y / size.y).min(1.0);
            super::ui::preview_popup(
                ui,
                egui::Id::new(("blend-thumbnail-hover", &entry.path)),
                thumbnail,
                size * scale,
                Some(texture),
                None,
            );
        }
    }
}
