use super::*;
use egui::{Rect, StrokeKind};
use little_clip_core::svg_image;
use little_clip_render::diagram::retained::compare::Preview;

pub(super) fn is_svg(target: &DiffTarget) -> bool {
    target.image_mime().is_some_and(|mime| {
        let mime = mime.split(';').next().unwrap_or_default().trim();
        ["image/svg+xml", "image/svgz", "image/x-svg+xml"]
            .iter()
            .any(|kind| mime.eq_ignore_ascii_case(kind))
    }) || target
        .ext_hint()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg") || ext.eq_ignore_ascii_case("svgz"))
        || target.image_bytes().is_some_and(|bytes| {
            bytes.starts_with(&[0x1f, 0x8b])
                || svg_image::is_markup(&bytes[..bytes.len().min(64 * 1024)])
        })
}

pub(super) fn paint_marks(
    painter: &egui::Painter,
    viewport: Rect,
    preview: &Preview,
    scale: f32,
    color: Color32,
) {
    for &[left, top, right, bottom] in &preview.marks {
        let rect = Rect::from_min_max(
            viewport.min + egui::vec2(left, top) * scale,
            viewport.min + egui::vec2(right, bottom) * scale,
        );
        if !rect.is_finite() || !rect.intersects(viewport) {
            continue;
        }
        let rect = rect.intersect(viewport);
        let marker = Rect::from_center_size(rect.center(), rect.size().max(Vec2::splat(4.0)));
        painter.rect_stroke(
            marker,
            0.0,
            Stroke::new(3.0, Color32::BLACK),
            StrokeKind::Middle,
        );
        painter.rect_stroke(marker, 0.0, Stroke::new(1.5, color), StrokeKind::Middle);
    }
}
