//! Passive, bounded page strips for visible PDF rows.
use super::{
    filesystem::Entry,
    media::PreviewData,
    shelf::preview::{Previews, Source},
};
use crate::{file_source::FileSource, icons::Icon, ui_appearance::WorkbenchColors};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct Pages {
    cache: Previews,
    hover_cache: Previews,
    hover_key: Option<(uuid::Uuid, usize, u32, uuid::Uuid)>,
    hover_seen: bool,
    files: BTreeMap<PathBuf, (uuid::Uuid, u32, BTreeMap<usize, uuid::Uuid>)>,
}
impl Pages {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
        self.hover_cache.poll(ctx);
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.clear_hover();
        self.files.clear();
    }
    pub fn retain(&mut self, paths: &BTreeSet<PathBuf>) {
        if !self.hover_seen {
            self.clear_hover();
        }
        self.hover_seen = false;
        self.files.retain(|path, _| paths.contains(path));
        self.cache.retain(
            &self
                .files
                .values()
                .flat_map(|(_, _, pages)| pages.values().copied())
                .collect(),
        );
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        entry: &Entry,
        identity: uuid::Uuid,
        info: Option<&little_clip_core::pdf::PdfInfo>,
        colors: WorkbenchColors,
    ) -> Option<usize> {
        let icon =
            egui::Rect::from_min_size(row.min + egui::vec2(6.0, 32.0), egui::vec2(64.0, 66.0));
        Icon::Pdf.paint(ui.painter(), icon.shrink(18.0), colors.text_secondary);
        let strip = egui::Rect::from_min_max(
            egui::pos2(icon.right() + 8.0, icon.top()),
            egui::pos2(row.right() - 6.0, row.bottom() - 10.0),
        );
        ui.painter().rect_filled(strip, 3.0, colors.recessed);
        let Some(info) = info else {
            ui.painter().text(
                strip.left_center(),
                egui::Align2::LEFT_CENTER,
                "PDF preview unavailable or loading…",
                egui::FontId::proportional(14.0),
                colors.text_secondary,
            );
            return None;
        };
        let slots = page_slots(
            info.page_count,
            ((strip.width() / 56.0).floor() as usize).clamp(3, 64),
        );
        let edge = (128.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(64.0, 256.0) as u32;
        let record = self
            .files
            .entry(entry.path.clone())
            .or_insert_with(|| (identity, edge, BTreeMap::new()));
        if record.0 != identity || record.1 != edge {
            for id in record.2.values() {
                self.cache.invalidate(*id);
            }
            *record = (identity, edge, BTreeMap::new());
        }
        record.2.retain(|page, id| {
            let keep = slots.contains(&Some(*page));
            if !keep {
                self.cache.invalidate(*id);
            }
            keep
        });
        let width = 56.0_f32.min(strip.width() / slots.len().max(1) as f32);
        let mut clicked = None;
        for (index, page) in slots.into_iter().enumerate() {
            let cell = egui::Rect::from_min_size(
                strip.min + egui::vec2(index as f32 * width, 0.0),
                egui::vec2(width, strip.height()),
            );
            let Some(page) = page else {
                ui.painter().text(
                    cell.center(),
                    egui::Align2::CENTER_CENTER,
                    "…",
                    egui::FontId::proportional(20.0),
                    colors.text_secondary,
                );
                continue;
            };
            let id = *record.2.entry(page).or_insert_with(uuid::Uuid::new_v4);
            if self.cache.needs(id) {
                self.cache.request(
                    id,
                    Source::PdfPage {
                        path: entry.path.clone(),
                        page,
                        edge,
                    },
                );
            }
            let image_box = egui::Rect::from_min_max(
                cell.min + egui::vec2(2.0, 2.0),
                cell.max - egui::vec2(2.0, 14.0),
            );
            if let Some(texture) = self.cache.get(id).and_then(|ready| ready.texture.as_ref()) {
                let size = texture.size_vec2();
                let scale = (image_box.width() / size.x).min(image_box.height() / size.y);
                let image = egui::Rect::from_center_size(image_box.center(), size * scale);
                ui.painter().image(
                    texture.id(),
                    image,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                Icon::Pdf.paint(ui.painter(), image_box.shrink(8.0), colors.text_secondary);
            }
            ui.painter().text(
                egui::pos2(cell.center().x, cell.bottom() - 6.0),
                egui::Align2::CENTER_CENTER,
                page.to_string(),
                egui::FontId::monospace(10.0),
                colors.text_secondary,
            );
            if ui.is_enabled()
                && ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|point| cell.intersect(ui.clip_rect()).contains(point))
            {
                clicked = Some(page);
            }
        }
        if let Some(page) = clicked {
            let fallback = record
                .2
                .get(&page)
                .and_then(|id| self.cache.get(*id))
                .and_then(|ready| ready.texture.clone());
            self.hover_page(ui, entry, identity, page, fallback.as_ref());
        }
        clicked
    }

    fn clear_hover(&mut self) {
        self.hover_cache.clear();
        self.hover_key = None;
        self.hover_seen = false;
    }

    fn hover_page(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        identity: uuid::Uuid,
        page: usize,
        fallback: Option<&egui::TextureHandle>,
    ) {
        self.hover_seen = true;
        let edge = (480.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(256.0, 1024.0) as u32;
        if !self.hover_key.is_some_and(|(source, previous, size, _)| {
            source == identity && previous == page && size == edge
        }) {
            self.hover_cache.clear();
            self.hover_key = Some((identity, page, edge, uuid::Uuid::new_v4()));
        }
        let id = self.hover_key.unwrap().3;
        if self.hover_cache.needs(id) {
            self.hover_cache.request(
                id,
                Source::PdfPage {
                    path: entry.path.clone(),
                    page,
                    edge,
                },
            );
        }
        let ready = self.hover_cache.get(id);
        let texture = ready.and_then(|ready| ready.texture.as_ref()).or(fallback);
        egui::show_tooltip_at_pointer(
            ui.ctx(),
            egui::Id::new(("pdf_strip_hover", &entry.path)),
            |ui| {
                ui.label(format!("{} · Page {page}", entry.name));
                if let Some(texture) = texture {
                    let screen = ui.ctx().screen_rect();
                    let bounds = egui::vec2(
                        (screen.width() - 40.0).clamp(1.0, 400.0),
                        (screen.height() - 80.0).clamp(1.0, 480.0),
                    );
                    let size = texture.size_vec2();
                    let scale = (bounds.x / size.x).min(bounds.y / size.y);
                    ui.add(egui::Image::new(texture).fit_to_exact_size(size * scale));
                } else if let Some(ready) = ready {
                    ui.label(&ready.data.text);
                } else {
                    ui.spinner();
                    ui.label("Loading page preview…");
                }
            },
        );
    }
}

fn page_slots(pages: usize, capacity: usize) -> Vec<Option<usize>> {
    if pages <= capacity {
        return (1..=pages).map(Some).collect();
    }
    let first = capacity / 2;
    let last = capacity - first - 1;
    (1..=first)
        .map(Some)
        .chain(std::iter::once(None))
        .chain((pages - last + 1..=pages).map(Some))
        .collect()
}

pub(super) fn load_page(
    path: &Path,
    page: usize,
    edge: u32,
    cancelled: &AtomicBool,
) -> PreviewData {
    let mut data = PreviewData::new(String::new(), format!("Page {page}"), Icon::Pdf);
    let result = (|| {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Preview cancelled".into());
        }
        let source = FileSource::capture(path)?;
        let (png, _, _) = little_clip_core::pdf::render_pdf_page_thumbnail(path, page, edge)?;
        source.check()?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Preview cancelled".into());
        }
        little_clip_core::ImageThumbnail::generate_with_mime(&png, Some("image/png"), edge, edge)
            .ok_or_else(|| "Could not decode PDF page thumbnail".to_string())
    })();
    match result {
        Ok(image) => data.image = Some(image),
        Err(error) => data.text = error,
    }
    data
}
