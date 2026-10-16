//! Visible-only material spheres using the shared cancellable preview cache.
mod render;
use super::{
    filesystem::Entry,
    shelf::preview::{HoverRaster, Previews as Cache, Ready, Source},
};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Ui, pos2, vec2};
use little_clip_core::model3d::materials::Material;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

pub(super) type Library = Arc<Vec<Arc<Material>>>;
#[derive(Clone)]
pub(in crate::file_manager) enum Request {
    Library(PathBuf),
    Sphere { material: Arc<Material>, edge: u32 },
}
#[derive(Default)]
pub(super) struct Previews {
    cache: Cache,
    spheres: BTreeMap<(uuid::Uuid, usize), (u32, uuid::Uuid)>,
    hover: HoverRaster,
}
impl Previews {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
        self.hover.poll(ctx);
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn retain(&mut self, visible: &BTreeSet<uuid::Uuid>) {
        self.spheres
            .retain(|(source, _), _| visible.contains(source));
        self.cache
            .retain(&self.spheres.values().map(|(_, id)| *id).collect());
        self.hover.retain(|source| visible.contains(&source));
    }
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
        let materials = ready.and_then(|ready| ready.data.materials.as_ref());
        let summary = materials
            .map(|materials| format!("MTL · {} materials", materials.len()))
            .unwrap_or_else(|| "MTL".into());
        super::ui::media_header(
            ui,
            Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
            entry,
            Some(summary),
            columns,
            colors,
        );
        let body = super::ui::strip_body(ui, row, Icon::Palette, colors);
        let Some(materials) = materials.filter(|materials| !materials.is_empty()) else {
            let message = ready.map_or("Loading materials…", |ready| {
                if ready.data.materials.is_some() {
                    "No materials in this library"
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
        let preferred_width = body.height() + 112.0;
        let capacity = ((body.width() / preferred_width).floor() as usize).clamp(3, 32);
        let slots = super::previews::strip_slots(materials.len(), capacity);
        let width = (body.width() / slots.len() as f32).min(preferred_width);
        let edge = (128.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(64.0, 256.0) as u32;
        let cells = super::ui::strip_cells(ui, body, slots, width, colors);
        let shown: BTreeSet<_> = cells.iter().map(|(ordinal, _)| ordinal - 1).collect();
        self.spheres.retain(|(id, index), (_, raster)| {
            let keep = *id != source || shown.contains(index);
            if !keep {
                self.cache.invalidate(*raster);
            }
            keep
        });
        for (ordinal, cell) in cells {
            let index = ordinal - 1;
            let material = &materials[index];
            let record = self
                .spheres
                .entry((source, index))
                .or_insert_with(|| (edge, uuid::Uuid::new_v4()));
            if record.0 != edge {
                self.cache.invalidate(record.1);
                *record = (edge, uuid::Uuid::new_v4());
            }
            let id = record.1;
            if self.cache.needs(id) {
                self.cache.request(
                    id,
                    Source::Material(Request::Sphere {
                        material: material.clone(),
                        edge,
                    }),
                );
            }
            let size = cell.height().min(cell.width() - 16.0).max(1.0);
            let sphere = Rect::from_min_size(cell.min, vec2(size, size));
            let label = Rect::from_min_max(
                pos2(sphere.right() + 8.0, cell.top()),
                pos2(cell.right() - 10.0, cell.bottom()),
            );
            let ready = self.cache.get(id);
            if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
                ui.painter().image(
                    texture.id(),
                    sphere,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                ui.painter().circle_filled(
                    sphere.center(),
                    size * 0.45,
                    egui::Color32::from_rgb(
                        (material.diffuse[0] * 255.0) as u8,
                        (material.diffuse[1] * 255.0) as u8,
                        (material.diffuse[2] * 255.0) as u8,
                    ),
                );
            }
            for (offset, rgb) in [material.diffuse, material.specular]
                .into_iter()
                .enumerate()
            {
                let swatch = Rect::from_min_size(
                    pos2(label.left() + offset as f32 * 14.0, label.bottom() - 14.0),
                    vec2(10.0, 10.0),
                );
                ui.painter().rect_filled(
                    swatch,
                    1.0,
                    egui::Color32::from_rgb(
                        (rgb[0] * 255.0) as u8,
                        (rgb[1] * 255.0) as u8,
                        (rgb[2] * 255.0) as u8,
                    ),
                );
            }
            if ready.is_some_and(|ready| !ready.data.text.is_empty()) {
                Icon::Alert.paint(
                    ui.painter(),
                    Rect::from_min_size(pos2(cell.right() - 14.0, cell.top()), vec2(12.0, 12.0)),
                    colors.warning,
                );
            }
            let mut name = egui::text::LayoutJob::simple(
                material.name.chars().take(60).collect::<String>(),
                FontId::proportional(12.0),
                colors.text_secondary,
                label.width().max(1.0),
            );
            name.wrap.max_rows = 2;
            name.wrap.break_anywhere = true;
            let name = ui.fonts(|fonts| fonts.layout_job(name));
            ui.painter()
                .with_clip_rect(label.intersect(ui.clip_rect()))
                .galley(label.min + vec2(0.0, 6.0), name, colors.text_secondary);
            if super::ui::preview_hovered(ui, cell) {
                let fallback = ready.and_then(|ready| ready.texture.clone());
                self.show_hover(ui, source, index, material, fallback.as_ref());
            }
        }
    }
    fn show_hover(
        &mut self,
        ui: &mut Ui,
        source: uuid::Uuid,
        index: usize,
        material: &Arc<Material>,
        fallback: Option<&egui::TextureHandle>,
    ) {
        let edge = (384.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(256.0, 768.0) as u32;
        let ready = self.hover.request(
            source,
            index,
            edge,
            Source::Material(Request::Sphere {
                material: material.clone(),
                edge,
            }),
        );
        let texture = ready.and_then(|ready| ready.texture.as_ref()).or(fallback);
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("material-strip-hover"), |ui| {
            ui.set_max_width((ui.ctx().screen_rect().width() - 40.0).clamp(32.0, 360.0));
            let size = (ui.ctx().screen_rect().height() - 260.0)
                .clamp(32.0, 320.0)
                .min((ui.ctx().screen_rect().width() - 40.0).max(32.0));
            if let Some(texture) = texture {
                ui.image((texture.id(), vec2(size, size)));
            }
            ui.label(ready.map_or_else(
                || material.name.chars().take(60).collect::<String>(),
                |ready| ready.data.detail.clone(),
            ));
        });
    }
}
