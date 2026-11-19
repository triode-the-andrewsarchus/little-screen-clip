//! Visible-only brush stroke strips using the shared cancellable preview cache.
use super::{
    filesystem::Entry,
    media::PreviewData,
    shelf::preview::{HoverRaster, Previews as Cache, Ready, Source},
};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Ui, pos2, vec2};
use little_clip_core::brushes::BrushLibrary;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone)]
pub(in crate::file_manager) struct StrokeRequest {
    pub library: Arc<BrushLibrary>,
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub fg_rgb: [u8; 3],
    pub include_tip_stamp: bool,
}

impl StrokeRequest {
    pub fn render(&self) -> PreviewData {
        let preset = &self.library.brushes[self.index];
        let mut data = PreviewData::new(preset.name.clone(), preset.detail.clone(), Icon::Brush);
        data.image = Some(little_clip_core::brushes::render::render_brush_preview(
            preset,
            self.width,
            self.height,
            self.fg_rgb,
            self.include_tip_stamp,
        ));
        data
    }
}

pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Brush".into(), Icon::Brush);
    match little_clip_core::brushes::load_brush_library(path, 64, token) {
        Ok(library) => {
            data.detail = if library.total_count == 1 {
                library
                    .brushes
                    .first()
                    .map_or_else(|| library.format.label().into(), |b| b.detail.clone())
            } else {
                format!(
                    "{} · {} brushes",
                    library.format.label(),
                    library.total_count
                )
            };
            if let Some(first) = library.brushes.first() {
                data.image = Some(little_clip_core::brushes::render::render_brush_preview(
                    first,
                    128,
                    64,
                    [235, 235, 240],
                    true,
                ));
            }
            data.brush = Some(Arc::new(library));
        }
        Err(error) => data.detail = error,
    }
    data
}

#[derive(Default)]
pub(super) struct Previews {
    pub(in crate::file_manager) context: super::search::PreviewContext,
    cache: Cache,
    strokes: BTreeMap<(uuid::Uuid, usize), ([u8; 3], u32, u32, uuid::Uuid)>,
    hover: HoverRaster,
}

impl Previews {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
        self.hover.poll(ctx);
    }

    pub fn clear(&mut self) {
        *self = Self {
            context: self.context.clone(),
            ..Self::default()
        };
    }

    pub fn retain(&mut self, visible: &BTreeSet<uuid::Uuid>) {
        self.strokes
            .retain(|(source, _), _| visible.contains(source));
        self.cache
            .retain(&self.strokes.values().map(|(_, _, _, id)| *id).collect());
        self.hover.retain(|source| visible.contains(&source));
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        row: Rect,
        entry: &Entry,
        columns: Option<super::ui::ListColumns>,
        source: uuid::Uuid,
        ready: Option<&Ready>,
        colors: WorkbenchColors,
    ) {
        let library: Option<&Arc<BrushLibrary>> = ready.and_then(|ready| ready.data.brush.as_ref());
        let summary = library
            .map(|lib| {
                if lib.total_count == 1 {
                    format!("{} · 1 brush", lib.format.label())
                } else {
                    format!("{} · {} brushes", lib.format.label(), lib.total_count)
                }
            })
            .unwrap_or_else(|| {
                little_clip_core::brushes::BrushFormat::from_path(&entry.path)
                    .map_or("BRUSH", |f| f.label())
                    .into()
            });
        super::ui::media_header(
            ui,
            Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
            entry,
            Some(summary),
            columns,
            colors,
        );
        let body = super::ui::strip_body(ui, row, Icon::Brush, colors);
        let Some(library) = library.filter(|lib| !lib.brushes.is_empty()) else {
            let message = ready.map_or("Loading brushes…", |ready| {
                if ready.data.brush.is_some() {
                    "No readable brushes in file"
                } else {
                    &ready.data.detail
                }
            });
            ui.painter().text(
                body.left_center(),
                Align2::LEFT_CENTER,
                message,
                FontId::proportional(13.0),
                colors.text_secondary,
            );
            return;
        };

        let preferred_width = (body.height() * 2.2 + 84.0).max(160.0);
        let capacity = ((body.width() / preferred_width).floor() as usize).clamp(1, 24);
        let slots = super::previews::strip_slots(library.brushes.len(), capacity);
        let width = (body.width() / slots.len() as f32).min(preferred_width);
        let cells = super::ui::strip_cells(ui, body, slots, width, colors);
        let shown: BTreeSet<_> = cells.iter().map(|(ordinal, _)| ordinal - 1).collect();
        self.strokes.retain(|(id, index), (_, _, _, raster)| {
            let keep = *id != source || shown.contains(index);
            if !keep {
                self.cache.invalidate(*raster);
            }
            keep
        });

        let fg_rgb = [
            colors.text_primary.r(),
            colors.text_primary.g(),
            colors.text_primary.b(),
        ];
        let dpi = ui.ctx().pixels_per_point();

        for (ordinal, cell) in cells {
            let index = ordinal - 1;
            let preset = &library.brushes[index];
            let stroke_h = (cell.height() - 18.0).max(16.0);
            let stroke_w = (cell.width() - 10.0).max(24.0);
            let req_w = (stroke_w * dpi).ceil().clamp(32.0, 512.0) as u32;
            let req_h = (stroke_h * dpi).ceil().clamp(16.0, 192.0) as u32;

            let record = self
                .strokes
                .entry((source, index))
                .or_insert_with(|| (fg_rgb, req_w, req_h, uuid::Uuid::new_v4()));
            if record.0 != fg_rgb || record.1 != req_w || record.2 != req_h {
                self.cache.invalidate(record.3);
                *record = (fg_rgb, req_w, req_h, uuid::Uuid::new_v4());
            }
            let id = record.3;
            if self.cache.needs(id) {
                self.cache.request(
                    id,
                    Source::BrushStroke(StrokeRequest {
                        library: library.clone(),
                        index,
                        width: req_w,
                        height: req_h,
                        fg_rgb,
                        include_tip_stamp: false,
                    })
                    .checked(&entry.path, &self.context),
                );
            }

            let stroke_rect =
                Rect::from_min_size(cell.min + vec2(4.0, 2.0), vec2(stroke_w, stroke_h));
            let ready = self.cache.get(id);
            if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
                ui.painter().image(
                    texture.id(),
                    stroke_rect,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            let label_rect = Rect::from_min_max(
                pos2(cell.left() + 4.0, stroke_rect.bottom() + 1.0),
                pos2(cell.right() - 6.0, cell.bottom()),
            );
            let mut name_job = egui::text::LayoutJob::simple(
                preset.name.chars().take(48).collect::<String>(),
                FontId::proportional(11.0),
                colors.text_secondary,
                label_rect.width().max(1.0),
            );
            name_job.wrap.max_rows = 1;
            name_job.wrap.break_anywhere = true;
            let galley = ui.fonts(|fonts| fonts.layout_job(name_job));
            ui.painter()
                .with_clip_rect(label_rect.intersect(ui.clip_rect()))
                .galley(label_rect.min, galley, colors.text_secondary);

            if super::ui::preview_hovered(ui, cell) {
                let fallback = ready.and_then(|ready| ready.texture.clone());
                self.show_hover(ui, entry, source, index, library, fg_rgb, fallback.as_ref());
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn show_hover(
        &mut self,
        ui: &mut Ui,
        entry: &Entry,
        source: uuid::Uuid,
        index: usize,
        library: &Arc<BrushLibrary>,
        fg_rgb: [u8; 3],
        fallback: Option<&egui::TextureHandle>,
    ) {
        let edge = (140.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(96.0, 280.0) as u32;
        let ready = self.hover.request(
            source,
            index,
            edge,
            Source::BrushStroke(StrokeRequest {
                library: library.clone(),
                index,
                width: edge * 3,
                height: edge,
                fg_rgb,
                include_tip_stamp: true,
            })
            .checked(&entry.path, &self.context),
        );
        let preset = &library.brushes[index];
        let texture = ready.and_then(|ready| ready.texture.as_ref()).or(fallback);
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("brush-strip-hover"), |ui| {
            ui.set_max_width((ui.ctx().screen_rect().width() - 40.0).clamp(32.0, 440.0));
            let h = 110.0_f32;
            let w = (h * 3.0).min((ui.ctx().screen_rect().width() - 56.0).max(64.0));
            if let Some(texture) = texture {
                ui.image((texture.id(), vec2(w, h)));
            }
            ui.strong(&preset.name);
            ui.weak(&preset.detail);
        });
    }
}
