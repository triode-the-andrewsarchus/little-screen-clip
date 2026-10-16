//! Optional Blender thumbnails; helper failures remain ordinary preview results.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{file_source::FileSource, icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Ui, pos2, vec2};
use little_clip_core::{
    ImageThumbnail,
    process::{CommandRequest, command_available},
};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Blender project".into(), Icon::Model3d);
    match thumbnail(path, token) {
        Ok(image) => data.image = Some(image),
        Err(error) => data.detail = error,
    }
    data
}

fn thumbnail(path: &Path, token: &AtomicBool) -> Result<ImageThumbnail, String> {
    if token.load(Ordering::Relaxed) {
        return Err("Preview cancelled".into());
    }
    let helper = command_available("blender-thumbnailer")
        .ok_or("Blender thumbnail unavailable: blender-thumbnailer is not in PATH")?;
    let source = FileSource::capture(path)?;
    let directory = tempfile::Builder::new()
        .prefix("veylar-blend-thumbnail-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let output = directory.path().join("preview.png");
    let result = CommandRequest::new(helper.into_os_string())
        .arg_content_file(path)
        .map_err(|e| e.to_string())?
        .arg(output.as_os_str())
        .timeout(Duration::from_secs(8))
        .max_output_bytes(16 * 1024)
        .run_cancellable(token)
        .map_err(|error| format!("Blender thumbnail unavailable: {error}"))?;
    if !result.status.success() {
        return Err("Blender thumbnail unavailable; the file may have no saved preview".into());
    }
    source.check()?;
    if token.load(Ordering::Relaxed) {
        return Err("Preview cancelled".into());
    }
    let bytes = crate::file_source::read_file_bytes(&output, 4 * 1024 * 1024, true, false)
        .map_err(|_| {
            "Blender thumbnail unavailable; the helper produced no usable preview".to_string()
        })?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(16 * 1024 * 1024);
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    ImageThumbnail::generate_with_limits(&bytes, Some("image/png"), 512, 512, limits)
        .ok_or_else(|| "Blender thumbnail unavailable; invalid or oversized PNG".into())
}

pub(super) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) {
    let body = super::ui::information_strip(ui, row, entry, "BLEND", colors);
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
