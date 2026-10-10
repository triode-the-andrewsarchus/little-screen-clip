use super::super::{filesystem::Entry, shelf::preview::Ready};
use super::{Analysis, Hover};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, Color32, FontId, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

pub(in crate::file_manager) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    identity: uuid::Uuid,
    ready: Option<&Ready>,
    hover: &mut Hover,
    colors: WorkbenchColors,
) {
    let analysis = ready.and_then(|ready| ready.data.photo.as_deref());
    let detail = analysis
        .map(|a| {
            format!(
                "{} × {} · {} · {}",
                a.dimensions.0, a.dimensions.1, entry.size, entry.modified
            )
        })
        .or_else(|| Some(format!("{} · {}", entry.size, entry.modified)));
    super::super::ui::media_header(
        ui,
        Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
        entry,
        detail,
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
    if ui.is_enabled()
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|point| thumb.intersect(ui.clip_rect()).contains(point))
    {
        hover.show(ui, thumb, entry, identity, analysis, texture);
    }
    let palette = Rect::from_min_size(
        pos2(thumb.right() + 10.0, body.top()),
        vec2(body.height(), body.height()),
    );
    swatches(ui, palette, entry, analysis);
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

fn swatches(ui: &Ui, rect: Rect, entry: &Entry, analysis: &Analysis) {
    if analysis.palette.is_empty() {
        return;
    }
    let edge = rect.width() / 3.0;
    for index in 0..9 {
        let color = analysis.palette[index % analysis.palette.len()];
        let cell = Rect::from_min_size(
            rect.min + vec2((index % 3) as f32 * edge, (index / 3) as f32 * edge),
            vec2(edge, edge),
        )
        .shrink(1.0);
        ui.painter().rect_filled(cell, 1.0, color);
        let response = ui.interact(
            cell,
            Id::new(("image_strip_color", &entry.path, index)),
            Sense::click(),
        );
        if response.clicked() {
            ui.output_mut(|output| {
                output.copied_text = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
            });
        }
        response.on_hover_text("Click to copy color");
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
