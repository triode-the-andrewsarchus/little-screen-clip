//! Whole-file byte brightness with format-aware section colors, never hex glyphs.
mod analysis;
mod icon;
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Rect, Ui, pos2, vec2};
use little_clip_core::ImageThumbnail;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Section {
    start: u64,
    end: u64,
    name: String,
    color: [u8; 3],
}
pub(super) struct Document {
    size: u64,
    description: String,
    sections: Vec<Section>,
    values: Vec<u8>,
    hash: String,
    icon: Option<ImageThumbnail>,
    uf2: bool,
    uf2_payload: Vec<u16>,
}
pub(super) fn known(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name.contains(".so.")
        || matches!(
            path.extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("bin" | "uf2" | "elf" | "so" | "dll" | "exe" | "o" | "a" | "dylib" | "wasm")
        )
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Binary".into(), Icon::Text);
    match analysis::inspect(path, token) {
        Ok(document) => {
            data.detail = document.description.clone();
            data.image = document
                .icon
                .clone()
                .or_else(|| super::image_strip::hash_art::exact(&document.hash));
            data.binary = Some(Arc::new(document));
        }
        Err(error) => data.detail = error,
    }
    data
}
impl Document {
    fn region(&self, offset: u64) -> (&str, [u8; 3]) {
        if self.uf2 {
            let within = offset % 512;
            return if within < 32 || within >= 508 {
                ("UF2 header / trailer", [185, 142, 218])
            } else if within
                < 32 + u64::from(
                    self.uf2_payload
                        .get((offset / 512) as usize)
                        .copied()
                        .unwrap_or(0),
                )
            {
                ("Firmware payload", [117, 180, 230])
            } else {
                ("Padding", [151, 158, 171])
            };
        }
        let index = self
            .sections
            .partition_point(|section| section.start <= offset);
        if let Some(section) = index
            .checked_sub(1)
            .and_then(|i| self.sections.get(i))
            .filter(|s| offset < s.end)
        {
            (&section.name, section.color)
        } else {
            ("Headers / unsectioned bytes", [151, 158, 171])
        }
    }
}
pub(super) fn render(
    document: &Document,
    width: u32,
    height: u32,
    token: &AtomicBool,
) -> Option<ImageThumbnail> {
    let width = width.clamp(1, 4096);
    let height = height.clamp(1, 256);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    // Each column advances through the file; its vertical cells show contiguous
    // byte ranges. Fixed summary bins keep resizing independent of file parsing.
    for y in 0..height {
        if token.load(Ordering::Relaxed) {
            return None;
        }
        for x in 0..width {
            let fraction = (x as f64 + y as f64 / height as f64) / width as f64;
            let offset = (fraction * document.size as f64) as u64;
            let start =
                ((fraction * document.values.len() as f64) as usize).min(document.values.len() - 1);
            let end_fraction = fraction + 1.0 / (width * height) as f64;
            let end = ((end_fraction * document.values.len() as f64).ceil() as usize)
                .clamp(start + 1, document.values.len());
            let values = &document.values[start..end];
            let mean =
                values.iter().map(|&v| u32::from(v)).sum::<u32>() as f32 / values.len() as f32;
            let brightness = 0.12 + 0.88 * mean / 255.0;
            let (_, color) = document.region(offset);
            rgba.extend(color.map(|channel| (channel as f32 * brightness) as u8));
            rgba.push(255);
        }
    }
    Some(ImageThumbnail {
        width,
        height,
        premultiplied_rgba: rgba,
    })
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
        ready.map_or("Binary", |r| r.data.detail.as_str()),
        colors,
    );
    let thumbnail = Rect::from_min_size(body.min, vec2(body.height(), body.height()));
    super::ui::model_thumbnail(
        ui,
        thumbnail,
        ready.and_then(|r| r.texture.as_ref()),
        colors,
    );
    let label_font = egui::FontId::monospace(9.0);
    let label_height = ui.fonts(|fonts| fonts.row_height(&label_font));
    // Leave room for the section bar, its gaps, and a full line of text.
    let strip = Rect::from_min_max(
        pos2(thumbnail.right() + 8.0, body.top()),
        pos2(body.right(), body.bottom() - label_height - 9.0),
    );
    ui.painter().rect_filled(strip, 2.0, colors.recessed);
    if let Some(document) = ready.and_then(|r| r.data.binary.as_ref()) {
        let bar = Rect::from_min_max(
            pos2(strip.left(), strip.bottom() + 2.0),
            pos2(strip.right(), strip.bottom() + 6.0),
        );
        for section in &document.sections {
            let left =
                bar.left() + bar.width() * section.start as f32 / document.size.max(1) as f32;
            let right = bar.left() + bar.width() * section.end as f32 / document.size.max(1) as f32;
            if right - left > 65.0 {
                ui.painter()
                    .with_clip_rect(
                        Rect::from_min_max(pos2(left, bar.bottom()), pos2(right, body.bottom()))
                            .intersect(ui.clip_rect()),
                    )
                    .text(
                        pos2((left + right) / 2.0, bar.bottom() + 1.0),
                        egui::Align2::CENTER_TOP,
                        &section.name,
                        label_font.clone(),
                        colors.text_secondary,
                    );
            }
            ui.painter().rect_filled(
                Rect::from_min_max(
                    pos2(left, bar.top()),
                    pos2(right.max(left + 0.5), bar.bottom()),
                ),
                0.0,
                egui::Color32::from_rgb(section.color[0], section.color[1], section.color[2]),
            );
        }
        if super::ui::preview_hovered(ui, strip) {
            let point = ui.ctx().pointer_hover_pos().unwrap_or(strip.min);
            let offset = (((point.x - strip.left()) / strip.width()).clamp(0.0, 1.0) as f64
                * document.size as f64) as u64;
            let (name, _) = document.region(offset.min(document.size.saturating_sub(1)));
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("binary-offset", &entry.path)),
                |ui| {
                    ui.label(format!("{name} · offset 0x{offset:x}"));
                    ui.label(
                        "Brightness represents byte value; dense regions use average byte values.",
                    );
                    for section in document.sections.iter().take(24) {
                        ui.label(format!(
                            "{} · {} bytes",
                            section.name,
                            section.end - section.start
                        ));
                    }
                },
            );
        }
        if super::ui::preview_hovered(ui, thumbnail) {
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("binary-hash", &entry.path)),
                |ui| {
                    ui.label(if document.icon.is_some() {
                        "Embedded executable icon"
                    } else {
                        "SHA-256 fingerprint"
                    });
                    ui.monospace(&document.hash);
                    ui.label("Matching SHA-256 hashes identify identical file bytes.");
                },
            );
        }
    }
    strip
}
