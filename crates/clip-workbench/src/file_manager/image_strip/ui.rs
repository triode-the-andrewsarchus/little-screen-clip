use super::super::{filesystem::Entry, shelf::preview::Ready};
use super::{Analysis, Hover};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, Color32, FontId, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

#[allow(clippy::too_many_arguments)]
pub(in crate::file_manager) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: Option<super::super::ui::ListColumns>,
    identity: uuid::Uuid,
    ready: Option<&Ready>,
    hover: &mut Hover,
    colors: WorkbenchColors,
) {
    let analysis = ready.and_then(|ready| ready.data.photo.as_deref());
    let detail = analysis.map(|a| format!("{} × {}", a.dimensions.0, a.dimensions.1));
    super::super::ui::media_header(
        ui,
        Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
        entry,
        detail,
        columns,
        colors,
    );
    let body = Rect::from_min_max(row.min + vec2(6.0, 32.0), row.max - vec2(6.0, 10.0));
    // Reserve the same space for every image; panoramas fit by losing height.
    let thumb = Rect::from_min_size(body.min, vec2(body.height() * 16.0 / 9.0, body.height()));
    let texture = ready.and_then(|ready| ready.texture.as_ref());
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let scale = (thumb.width() / size.x)
            .min(thumb.height() / size.y)
            .min(1.0);
        ui.painter().image(
            texture.id(),
            Rect::from_center_size(thumb.center(), size * scale),
            Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        Icon::Image.paint(ui.painter(), thumb.shrink(16.0), colors.text_secondary);
    }
    let Some(analysis) = analysis else {
        text(
            ui,
            Rect::from_min_max(pos2(thumb.right() + 8.0, body.top()), body.max),
            ready.map_or("Loading image preview…", |ready| &ready.data.detail),
            colors.text_secondary,
        );
        return;
    };
    let palette = palette_rect(body, thumb);
    let mut hovered_segment = None;
    hilbert_strip_ui(ui, palette, entry, analysis, colors, &mut hovered_segment);

    let thumb_hovered = ui.is_enabled()
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|point| thumb.intersect(ui.clip_rect()).contains(point));

    if thumb_hovered {
        hover.show(ui, thumb, entry, identity, analysis, texture, None);
    } else if let Some(segment) = hovered_segment.filter(|_| ui.is_enabled()) {
        hover.show(ui, thumb, entry, identity, analysis, texture, Some(segment));
    }
    let rest = Rect::from_min_max(pos2(palette.right() + 12.0, body.top()), body.max);
    if rest.width() < 80.0 {
        return;
    }
    let tags = if rest.width() > 650.0 { 100.0 } else { 0.0 };
    let available = (rest.width() - tags - 24.0).max(1.0);
    let barcode_width = body.height() * 2.0 + 8.0;
    let histogram_width = (available * 0.25)
        .min(220.0)
        .min((available - barcode_width).max(0.0));
    let histogram = Rect::from_min_size(rest.min, vec2(histogram_width, body.height()));
    histogram_ui(ui, histogram, analysis, colors);
    let barcode = Rect::from_min_size(
        pos2(histogram.right() + 10.0, body.top()),
        vec2(barcode_width, body.height()),
    );
    hash_ui(ui, barcode, entry, analysis, ready, colors);
    let exif = Rect::from_min_max(
        pos2(barcode.right() + 10.0, body.top()),
        pos2(rest.right() - tags, body.bottom()),
    );
    metadata_ui(ui, exif, entry, analysis, colors);
    if tags > 0.0 {
        let tag = Rect::from_min_max(pos2(rest.right() - tags + 8.0, body.top()), body.max);
        // Intentionally empty: this column reserves space for future image tags.
        Icon::Tag.paint(
            ui.painter(),
            Rect::from_center_size(tag.center(), vec2(16.0, 16.0)),
            colors.text_secondary,
        );
        ui.interact(
            tag,
            Id::new(("image_strip_tags", &entry.path)),
            Sense::hover(),
        )
        .on_hover_text("Image tags");
    }
}

fn text(ui: &Ui, rect: Rect, value: &str, color: Color32) {
    if rect.width() <= 0.0 {
        return;
    }
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            value,
            FontId::proportional(13.0),
            color,
        );
}

fn hilbert_strip_ui(
    ui: &Ui,
    rect: Rect,
    entry: &Entry,
    analysis: &Analysis,
    colors: WorkbenchColors,
    hovered_segment: &mut Option<usize>,
) {
    let strip = &analysis.hilbert_strip;
    let count = strip.visible_segments.len();
    if count == 0 {
        return;
    }

    let clip = rect.intersect(ui.clip_rect());
    let width = rect.width();

    ui.painter().rect_filled(rect, 2.0, colors.recessed);

    let painter = ui.painter().with_clip_rect(clip);
    for (i, seg) in strip.visible_segments.iter().enumerate() {
        let x0 = rect.left() + (i as f32 / count as f32) * width;
        let x1 = rect.left() + ((i + 1) as f32 / count as f32) * width;
        let stripe = Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom()));
        painter.rect_filled(stripe, 0.0, seg.color);
    }

    ui.painter()
        .rect_stroke(rect, 2.0, Stroke::new(1.0_f32, colors.recessed));

    let response = ui.interact(
        rect,
        Id::new(("image_strip_hilbert", &entry.path)),
        Sense::click_and_drag(),
    );

    let pointer = ui
        .ctx()
        .pointer_hover_pos()
        .or_else(|| response.interact_pointer_pos());

    if let Some(pos) = pointer.filter(|p| clip.contains(*p)) {
        let rel_x = ((pos.x - rect.left()) / width).clamp(0.0, 1.0 - f32::EPSILON);
        let stripe_idx = ((rel_x * count as f32).floor() as usize).min(count - 1);
        let seg = &strip.visible_segments[stripe_idx];
        *hovered_segment = Some(seg.source_segment);

        let x0 = rect.left() + (stripe_idx as f32 / count as f32) * width;
        let x1 = rect.left() + ((stripe_idx + 1) as f32 / count as f32) * width;
        let stripe_width = x1 - x0;
        let marker_rect = if stripe_width < 3.0 {
            let center_x = (x0 + x1) * 0.5;
            Rect::from_center_size(pos2(center_x, rect.center().y), vec2(3.0, rect.height()))
        } else {
            Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom()))
        };
        let color = seg.color;
        let l = super::hilbert::oklab_l(color.r(), color.g(), color.b());
        let marker_color = if l >= 0.5 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        ui.painter().with_clip_rect(clip).rect_stroke(
            marker_rect,
            0.0,
            Stroke::new(1.5_f32, marker_color),
        );

        let hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
        if response.clicked() {
            ui.output_mut(|output| {
                output.copied_text = hex.clone();
            });
        }
        response.on_hover_text(format!("Click to copy color\n{hex}"));
    }
}

fn histogram_ui(ui: &Ui, rect: Rect, analysis: &Analysis, colors: WorkbenchColors) {
    ui.painter().rect_filled(rect, 2.0, colors.recessed);
    let max = analysis
        .histogram
        .iter()
        .flatten()
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    for (channel, color) in [
        Color32::from_rgb(230, 75, 75),
        Color32::from_rgb(80, 205, 100),
        Color32::from_rgb(85, 130, 245),
    ]
    .into_iter()
    .enumerate()
    {
        let mut mesh = egui::Mesh::default();
        let mut points = Vec::with_capacity(256);
        for (i, count) in analysis.histogram[channel].iter().enumerate() {
            let point = pos2(
                rect.left() + i as f32 / 255.0 * rect.width(),
                rect.bottom() - *count as f32 / max * (rect.height() - 2.0),
            );
            points.push(point);
            mesh.colored_vertex(pos2(point.x, rect.bottom()), color.linear_multiply(0.16));
            mesh.colored_vertex(point, color.linear_multiply(0.16));
            if i > 0 {
                let n = (i * 2) as u32;
                mesh.add_triangle(n - 2, n - 1, n);
                mesh.add_triangle(n - 1, n + 1, n);
            }
        }
        ui.painter().add(mesh);
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(1.0_f32, color)));
    }
    ui.interact(
        rect,
        Id::new((
            "image_rgb_histogram",
            ui.id(),
            rect.min.x.to_bits(),
            rect.min.y.to_bits(),
        )),
        Sense::hover(),
    )
    .on_hover_text("RGB histogram · sampled image colors");
}

fn hash_ui(
    ui: &Ui,
    bounds: Rect,
    entry: &Entry,
    analysis: &Analysis,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) {
    let edge = bounds.height();
    let group = Rect::from_center_size(bounds.center(), vec2(edge * 2.0 + 8.0, edge));
    for (half, (label, value)) in [
        ("SHA-256 · exact file hash", analysis.exact.as_deref()),
        (
            "Color + dHash-64 · perceptual image hash",
            Some(analysis.perceptual.as_str()),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let rect = Rect::from_min_size(
            group.min + vec2(half as f32 * (edge + 8.0), 0.0),
            vec2(edge, edge),
        );
        if let Some(texture) = ready.and_then(|ready| ready.hash_textures[half].as_ref()) {
            ui.painter().image(
                texture.id(),
                rect,
                Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter().rect_filled(rect, 3.0, colors.recessed);
        }
        let response = ui.interact(
            rect,
            Id::new(("image_strip_hash", &entry.path, half)),
            if value.is_some() {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if let Some(hash) = value {
            if response.clicked() {
                ui.output_mut(|output| output.copied_text = hash.to_owned());
            }
            let explanation = if half == 0 {
                "If these are the exact same for two images, it means the images are pixel-for-pixel identical.\nSHA-256 hashes the entire file, including encoding and metadata. Images with identical pixels can still have different file hashes."
            } else {
                "Use this to spot visually similar images, including resized or recompressed copies. The colors summarize the image's coarse color; the 8×8 grid describes its light/dark structure. Similar colors and patterns suggest similarity, but do not prove the images are identical."
            };
            response.on_hover_text(format!(
                "{label}\n{hash}\n\n{explanation}\n\nClick to copy hash"
            ));
        } else {
            response.on_hover_text("Exact file hash unavailable");
        }
    }
}

fn metadata_ui(ui: &Ui, rect: Rect, entry: &Entry, analysis: &Analysis, colors: WorkbenchColors) {
    let meta = &analysis.metadata;
    let fields = [
        (
            Icon::Camera,
            meta.model.clone().or_else(|| meta.make.clone()),
        ),
        (
            Icon::Contrast,
            meta.f_number.map(|value| format!("f/{value}")),
        ),
        (Icon::Badge, meta.iso.map(|value| format!("ISO {value}"))),
        (Icon::Clock, meta.exposure_time.clone()),
        (
            Icon::Crosshair,
            meta.focal_length.map(|value| format!("{value} mm")),
        ),
        (Icon::Calendar, meta.date_time.clone()),
    ];
    let populated: Vec<_> = fields
        .into_iter()
        .filter_map(|(icon, value)| value.map(|value| (icon, value)))
        .collect();
    let columns = if rect.width() >= 230.0 { 2 } else { 1 };
    let width = rect.width() / columns as f32;
    for (index, (icon, value)) in populated.iter().enumerate().take(columns * 3) {
        let cell = Rect::from_min_size(
            rect.min
                + vec2(
                    (index % columns) as f32 * width,
                    (index / columns) as f32 * 22.0,
                ),
            vec2(width, 22.0),
        );
        icon.paint(
            ui.painter(),
            Rect::from_center_size(cell.left_center() + vec2(8.0, 0.0), vec2(16.0, 16.0)),
            colors.text_secondary,
        );
        text(
            ui,
            Rect::from_min_max(cell.min + vec2(23.0, 0.0), cell.max),
            value,
            colors.text_primary,
        );
    }
    if !populated.is_empty() {
        let mut details: Vec<_> = populated.iter().map(|(_, value)| value.clone()).collect();
        details.extend(meta.lens_model.iter().cloned());
        details.extend(meta.software.iter().cloned());
        ui.interact(
            rect,
            Id::new(("image_strip_exif", &entry.path)),
            Sense::hover(),
        )
        .on_hover_text(details.join("\n"));
    }
}

/// Target aspect ratio (width:height) for the palette strip when space allows.
pub const PALETTE_ASPECT_RATIO: f32 = 8.0;

/// Computes the palette rectangle, targeting an 8:1 rectangle when space allows,
/// but trading space down to square (1:1) when the window is resized thinner.
pub(super) fn palette_rect(body: Rect, thumb: Rect) -> Rect {
    let available_space = (body.right() - thumb.right() - 10.0).max(0.0);
    let ideal_palette_width = body.height() * PALETTE_ASPECT_RATIO;
    let min_palette_width = body.height().min(available_space);
    let rest_comfort_width = body.height() * 4.0 + 20.0;
    let palette_width = if available_space >= ideal_palette_width + 12.0 + rest_comfort_width {
        ideal_palette_width
    } else {
        (available_space - 12.0 - rest_comfort_width)
            .clamp(min_palette_width, ideal_palette_width)
            .min(available_space)
    };
    Rect::from_min_size(
        pos2(thumb.right() + 10.0, body.top()),
        vec2(palette_width, body.height()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_rect_aspect_ratio_and_space_trading() {
        let h = 66.0;
        let body_wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, h));
        let thumb = Rect::from_min_size(pos2(0.0, 0.0), vec2(h * 16.0 / 9.0, h));

        // Wide window: gets the full 8:1 aspect ratio
        let pal_wide = palette_rect(body_wide, thumb);
        assert!((pal_wide.width() - h * 8.0).abs() < 0.01);
        assert_eq!(pal_wide.height(), h);

        // Medium window: trades space smoothly between 8:1 and 1:1
        let body_med = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, h));
        let pal_med = palette_rect(body_med, thumb);
        assert!(pal_med.width() < h * 8.0);
        assert!(pal_med.width() >= h);

        // Narrow window: clamps at 1:1 square
        let body_narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(450.0, h));
        let pal_narrow = palette_rect(body_narrow, thumb);
        assert!((pal_narrow.width() - h).abs() < 0.01);

        // Extremely narrow window: bounded by available space
        let body_tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(thumb.right() + 30.0, h));
        let pal_tiny = palette_rect(body_tiny, thumb);
        assert!((pal_tiny.width() - 20.0).abs() < 0.01);
    }
}
