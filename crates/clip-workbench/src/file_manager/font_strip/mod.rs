//! Passive font specimens; outline rasterization stays on the visible-preview workers.
mod raster;
mod web_font;
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Rect, Ui, pos2, vec2};
pub(super) use raster::render;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) const SAMPLES: [&str; 3] = [
    "Sphinx of black quartz, judge my vow.",
    "0123456789  ! ? @ # $ % & * + − = / \\ |",
    "( ) [ ] { } < > : ; , . ' \" _ ~ ^ ` € £ ¥ © ® ° ± × ÷ → ←",
];
pub(super) struct Document {
    bytes: Vec<u8>,
    pub detail: String,
    pub description: String,
}
pub(super) fn known(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2")
    )
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Font".into(), Icon::Text);
    match inspect(path, token) {
        Ok(document) => {
            data.detail = document.detail.clone();
            data.image = raster::thumbnail(&document, token);
            data.font = Some(Arc::new(document));
        }
        Err(error) => data.detail = error,
    }
    data
}
fn inspect(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let source = crate::file_source::FileSource::capture(path)?;
    if token.load(Ordering::Relaxed) {
        return Err("Preview cancelled".into());
    }
    let bytes = crate::file_source::read_file_bytes(path, 32 * 1024 * 1024, true, true)
        .map_err(|e| format!("Font preview unavailable: {e}"))?;
    let bytes = web_font::decode(bytes, token)?;
    let face = ttf_parser::Face::parse(&bytes, 0)
        .map_err(|e| format!("Unsupported or invalid font: {e:?}"))?;
    let faces = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
    let (detail, description) = describe(&face, faces);
    source.check()?;
    Ok(Document {
        bytes,
        detail,
        description,
    })
}
fn describe(face: &ttf_parser::Face<'_>, faces: u32) -> (String, String) {
    let family = name(face, ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
        .or_else(|| name(face, ttf_parser::name_id::FAMILY))
        .unwrap_or_else(|| "Unnamed font".into());
    let style = name(face, ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
        .or_else(|| name(face, ttf_parser::name_id::SUBFAMILY))
        .unwrap_or_default();
    let mut parts = vec![
        family,
        style,
        format!("{} glyphs", face.number_of_glyphs()),
        format!("weight {}", face.weight().to_number()),
    ];
    if face.is_monospaced() {
        parts.push("Monospace".into());
    }
    if face.is_variable() {
        parts.push("Variable".into());
    }
    if faces > 1 {
        parts.push(format!("{faces} faces (first shown)"));
    }
    parts.retain(|s| !s.is_empty());
    let detail = parts.join(" · ");
    let missing = SAMPLES
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace() && face.glyph_index(*c).is_none())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<String>();
    let axes = face
        .variation_axes()
        .into_iter()
        .take(32)
        .map(|a| {
            format!(
                "{}: {}–{} (default {})",
                a.tag, a.min_value, a.max_value, a.def_value
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let description = format!(
        "{detail}\n{} units/em{}{}",
        face.units_per_em(),
        if axes.is_empty() {
            String::new()
        } else {
            format!("\n{axes}")
        },
        if missing.is_empty() {
            String::new()
        } else {
            format!("\nUnavailable sample glyphs: {missing}")
        }
    );
    (detail, description)
}

fn name(face: &ttf_parser::Face<'_>, id: u16) -> Option<String> {
    face.names()
        .into_iter()
        .filter(|n| n.name_id == id)
        .filter_map(|n| n.to_string())
        .map(|name| name.chars().take(160).collect())
        .next()
}
pub(super) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Rect {
    let body = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Font", |r| r.data.detail.as_str()),
        colors,
    );
    let thumbnail = Rect::from_min_size(body.min, vec2(body.height(), body.height()));
    ui.painter().rect_filled(thumbnail, 3.0, colors.recessed);
    if let Some(texture) = ready.and_then(|r| r.texture.as_ref()) {
        ui.painter().image(
            texture.id(),
            thumbnail,
            Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
            colors.text_primary,
        );
    } else {
        ui.painter().text(
            thumbnail.center(),
            egui::Align2::CENTER_CENTER,
            "Aa",
            egui::FontId::proportional(30.0),
            colors.text_secondary,
        );
    }
    let strip = Rect::from_min_max(pos2(thumbnail.right() + 12.0, body.top()), body.max);
    if let Some(document) = ready.and_then(|r| r.data.font.as_ref()) {
        if super::ui::preview_hovered(ui, body) {
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("font-info", &entry.path)),
                |ui| {
                    ui.label(&document.description);
                },
            );
        }
    } else {
        ui.painter().text(
            strip.left_center(),
            egui::Align2::LEFT_CENTER,
            ready.map_or("Loading font…", |r| r.data.detail.as_str()),
            egui::FontId::proportional(13.0),
            colors.text_secondary,
        );
    }
    strip
}
