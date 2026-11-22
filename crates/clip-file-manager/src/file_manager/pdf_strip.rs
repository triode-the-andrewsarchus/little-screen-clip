//! Bounded previews and page selection for visible PDF rows.
mod selection;
use super::{
    filesystem::Entry,
    media::PreviewData,
    preview_copy,
    shelf::preview::{HoverRaster, Previews, Source},
};
use crate::{file_source::FileSource, icons::Icon, ui_appearance::WorkbenchColors};
pub(super) use selection::focus_id;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct Pages {
    pub(in crate::file_manager) context: super::search::PreviewContext,
    cache: Previews,
    pub(super) selection: selection::Selection,
    hover: HoverRaster,
    files: BTreeMap<PathBuf, (uuid::Uuid, u32, BTreeMap<usize, uuid::Uuid>)>,
}
impl Pages {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
        self.hover.poll(ctx);
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.hover.clear();
        self.files.clear();
        self.selection = Default::default();
    }
    pub fn retain(&mut self, paths: &BTreeSet<PathBuf>) {
        self.files.retain(|path, _| paths.contains(path));
        self.hover
            .retain(|source| self.files.values().any(|(id, _, _)| *id == source));
        self.cache.retain(
            &self
                .files
                .values()
                .flat_map(|(_, _, pages)| pages.values().copied())
                .collect(),
        );
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "The strip owns rendering and shares the enclosing row response"
    )]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        entry: &Entry,
        identity: uuid::Uuid,
        page_count: Option<usize>,
        colors: WorkbenchColors,
        response: &mut egui::Response,
    ) -> (Option<usize>, bool) {
        let strip = super::ui::strip_body(ui, row, Icon::Pdf, colors);
        ui.painter().rect_filled(strip, 3.0, colors.recessed);
        let Some(page_count) = page_count else {
            let label = if little_clip_core::document::is_office_document(&entry.path) {
                "Document preview unavailable or loading…"
            } else {
                "PDF preview unavailable or loading…"
            };
            ui.painter().text(
                strip.left_center(),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(14.0),
                colors.text_secondary,
            );
            return (None, false);
        };
        let slots = super::previews::strip_slots(
            page_count,
            ((strip.width() / 56.0).floor() as usize).clamp(3, 64),
        );
        let layout = selection::Layout {
            rect: strip,
            slots: &slots,
            count: page_count,
        };
        let width = layout.width();
        let can_select = page_count > 0
            && strip.is_positive()
            && little_clip_core::pdf::is_pdf_path(&entry.path);
        let engaged = can_select && self.selection.interact(ui, &layout, entry, response);
        let body = self.selection_body(ui, &layout, entry, colors);
        let page = self.paint_pages(ui, body, entry, (identity, slots, width), colors, engaged);
        (page, engaged)
    }

    fn selection_body(
        &mut self,
        ui: &mut egui::Ui,
        layout: &selection::Layout<'_>,
        entry: &Entry,
        colors: WorkbenchColors,
    ) -> egui::Rect {
        let mut body = layout.rect;
        let Some(selected) = self
            .selection
            .selected
            .as_ref()
            .filter(|selected| selected.matches(entry))
        else {
            return body;
        };
        body.max.y -= preview_copy::RANGE_FOOTER_HEIGHT;
        ui.painter_at(body).rect_filled(
            layout.highlight(&selected.range, body),
            2.0,
            colors.accent.gamma_multiply(0.35),
        );
        if preview_copy::paint_range_footer(
            ui,
            layout.rect,
            &format!("{}  ·  Ctrl+C", selected.label()),
            "Copy pages",
        ) {
            self.selection.requested = Some(selected.clone());
            ui.memory_mut(|memory| memory.request_focus(focus_id()));
        }
        body
    }

    fn paint_pages(
        &mut self,
        ui: &mut egui::Ui,
        strip: egui::Rect,
        entry: &Entry,
        (identity, slots, width): (uuid::Uuid, Vec<Option<usize>>, f32),
        colors: WorkbenchColors,
        engaged: bool,
    ) -> Option<usize> {
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
        let mut clicked = None;
        for (page, cell) in super::ui::strip_cells(ui, strip, slots, width, colors) {
            let id = *record.2.entry(page).or_insert_with(uuid::Uuid::new_v4);
            if self.cache.needs(id) {
                self.cache.request(
                    id,
                    Source::PdfPage {
                        path: entry.path.clone(),
                        page,
                        edge,
                    }
                    .checked(&entry.path, &self.context),
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
            if super::ui::preview_hovered(ui, cell) {
                clicked = Some(page);
            }
        }
        if let Some(page) = clicked.filter(|_| !engaged) {
            let fallback = record
                .2
                .get(&page)
                .and_then(|id| self.cache.get(*id))
                .and_then(|ready| ready.texture.clone());
            self.hover_page(ui, entry, identity, page, fallback.as_ref());
        }
        clicked
    }

    fn hover_page(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        identity: uuid::Uuid,
        page: usize,
        fallback: Option<&egui::TextureHandle>,
    ) {
        let edge = (480.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(256.0, 1024.0) as u32;
        let ready = self.hover.request(
            identity,
            page,
            edge,
            Source::PdfPage {
                path: entry.path.clone(),
                page,
                edge,
            }
            .checked(&entry.path, &self.context),
        );
        let texture = ready.and_then(|ready| ready.texture.as_ref()).or(fallback);
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            egui::Id::unique(("pdf_strip_hover", &entry.path)),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| {
            ui.label(format!("{} · Page {page}", entry.name));
            if little_clip_core::pdf::is_pdf_path(&entry.path) {
                ui.small("Drag to select pages · Right-click to copy a page");
            }
            if let Some(texture) = texture {
                let screen = ui.ctx().content_rect();
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
        });
    }
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
        let (png, _, _) = if little_clip_core::document::is_office_document(path) {
            let pdf_path = little_clip_core::document::ensure_rendered_pdf(path)?;
            little_clip_core::pdf::render_pdf_page_thumbnail(&pdf_path, page, edge)?
        } else if little_clip_core::notebook::is_candidate_path(path) {
            little_clip_core::notebook::render_page(path, page, edge, cancelled)?
        } else {
            little_clip_core::pdf::render_pdf_page_thumbnail(path, page, edge)?
        };
        source.check()?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Preview cancelled".into());
        }
        little_clip_core::ImageThumbnail::generate_with_mime(&png, Some("image/png"), edge, edge)
            .ok_or_else(|| "Could not decode document page thumbnail".to_string())
    })();
    match result {
        Ok(image) => data.image = Some(image),
        Err(error) => data.text = error,
    }
    data
}
