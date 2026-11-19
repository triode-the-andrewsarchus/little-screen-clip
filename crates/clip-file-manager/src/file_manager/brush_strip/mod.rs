//! Brush stroke strips with visible-only jobs and a bounded cache of completed previews.
use super::{
    filesystem::Entry,
    media::PreviewData,
    shelf::preview::{HoverRaster, Previews as Cache, Ready, Source},
};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use little_clip_core::{
    Oklch,
    brushes::{
        BrushLibrary,
        render::{BRUSH_HOVER_ASPECT_RATIO, render_brush_hover_preview, render_brush_preview},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    pub overlap: Option<[[u8; 3]; 2]>,
}

impl StrokeRequest {
    pub fn render(&self) -> PreviewData {
        let preset = &self.library.brushes[self.index];
        let mut data = PreviewData::new(preset.name.clone(), preset.detail.clone(), Icon::Brush);
        data.image = Some(match self.overlap {
            Some(colors) => render_brush_hover_preview(preset, self.height, colors),
            None => render_brush_preview(preset, self.width, self.height, self.fg_rgb, true),
        });
        data
    }
}

pub(super) fn load_listing(path: &Path, revision: u64, token: &AtomicBool) -> PreviewData {
    let check = || super::filesystem::check_revision(path, revision);
    check()
        .and_then(|()| {
            let preview = load(path, token);
            check().map(|()| preview)
        })
        .unwrap_or_else(|error| PreviewData::new(String::new(), error, Icon::Brush))
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
                data.image = Some(render_brush_preview(first, 128, 64, [235, 235, 240], true));
            }
            data.brush = Some(Arc::new(library));
        }
        Err(error) => data.detail = error,
    }
    data
}

const RECENT_BRUSH_ROWS: usize = 12;
const RECENT_BRUSH_RASTERS: usize = 48;

#[derive(Default)]
pub(super) struct Previews {
    pub(in crate::file_manager) context: super::search::PreviewContext,
    cache: Cache,
    strokes: BTreeMap<(uuid::Uuid, usize), ([u8; 3], u32, u32, uuid::Uuid)>,
    hover: HoverRaster,
    recent: VecDeque<uuid::Uuid>,
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

    fn remember(&mut self, source: uuid::Uuid) {
        self.forget(source);
        self.recent.push_front(source);
        self.recent.truncate(RECENT_BRUSH_ROWS);
    }

    pub fn forget(&mut self, source: uuid::Uuid) {
        self.recent.retain(|id| *id != source);
    }

    pub fn retain(&mut self, visible: &BTreeSet<uuid::Uuid>) -> &VecDeque<uuid::Uuid> {
        if visible.is_empty() {
            self.recent.clear();
        }
        let visible_rasters: BTreeSet<_> = self
            .strokes
            .iter()
            .filter(|((source, _), _)| visible.contains(source))
            .map(|(_, (_, _, _, raster))| *raster)
            .collect();
        let recent_rasters: BTreeSet<_> = self
            .recent
            .iter()
            .copied()
            .filter(|source| !visible.contains(source))
            .flat_map(|source| self.strokes.range((source, 0)..=(source, usize::MAX)))
            .map(|(_, (_, _, _, raster))| *raster)
            .filter(|&raster| {
                self.cache
                    .get(raster)
                    .is_some_and(|ready| ready.texture.is_some())
            })
            .take(RECENT_BRUSH_RASTERS)
            .collect();
        self.cache
            .retain_with_ready(&visible_rasters, |id| recent_rasters.contains(&id));
        self.strokes
            .retain(|_, (_, _, _, id)| visible_rasters.contains(id) || recent_rasters.contains(id));
        self.hover.retain(|source| visible.contains(&source));
        &self.recent
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
        let count_in_columns = columns.is_some_and(|columns| {
            columns
                .metadata_fields()
                .contains(&super::metadata::Field::BrushCount)
        });
        let summary = library
            .filter(|_| !count_in_columns)
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

        self.remember(source);
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
        let hover_idle = ui.input(|input| !input.is_scrolling() && !input.pointer.primary_down());

        for (ordinal, cell) in cells {
            let index = ordinal - 1;
            let preset = &library.brushes[index];
            let preview_rect = cell_preview_rect(cell);
            let (req_w, req_h) = raster_dimensions(preview_rect, dpi);

            let texture = self.stroke_texture(
                source,
                &entry.path,
                StrokeRequest {
                    library: library.clone(),
                    index,
                    width: req_w,
                    height: req_h,
                    fg_rgb,
                    overlap: None,
                },
            );
            if let Some(texture) = texture {
                paint_brush_texture(ui, preview_rect, texture);
            }

            let label_rect = Rect::from_min_max(
                pos2(cell.left() + 4.0, cell.bottom() - 15.0),
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
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(name_job));
            ui.painter()
                .with_clip_rect(label_rect.intersect(ui.clip_rect()))
                .galley(label_rect.min, galley, colors.text_secondary);

            if hover_idle && super::ui::preview_hovered(ui, cell) {
                let fallback = texture.cloned();
                self.show_hover(ui, entry, source, index, library, fallback.as_ref());
            }
        }
    }

    fn stroke_texture(
        &mut self,
        source: uuid::Uuid,
        path: &Path,
        request: StrokeRequest,
    ) -> Option<&egui::TextureHandle> {
        let requested = (request.fg_rgb, request.width, request.height);
        let record = self
            .strokes
            .entry((source, request.index))
            .or_insert_with(|| (requested.0, requested.1, requested.2, uuid::Uuid::new_v4()));
        if record.0 != requested.0
            || record.1.abs_diff(requested.1) > 1
            || record.2.abs_diff(requested.2) > 1
        {
            self.cache.invalidate(record.3);
            *record = (requested.0, requested.1, requested.2, uuid::Uuid::new_v4());
        }
        let id = record.3;
        if self.cache.needs(id) {
            self.cache.request(
                id,
                Source::BrushStroke(request).checked(path, &self.context),
            );
        }
        self.cache.get(id)?.texture.as_ref()
    }

    fn show_hover(
        &mut self,
        ui: &mut Ui,
        entry: &Entry,
        source: uuid::Uuid,
        index: usize,
        library: &Arc<BrushLibrary>,
        fallback: Option<&egui::TextureHandle>,
    ) {
        let edge = (140.0 * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(96.0, 280.0) as u32;
        let ready = self.hover.request_with(source, index, edge, |seed| {
            Source::BrushStroke(StrokeRequest {
                library: library.clone(),
                index,
                width: (edge as f32 * BRUSH_HOVER_ASPECT_RATIO).round() as u32,
                height: edge,
                fg_rgb: [242, 242, 245],
                overlap: Some(pastel_pair(seed)),
            })
            .checked(&entry.path, &self.context)
        });
        let preset = &library.brushes[index];
        let texture = ready.and_then(|ready| ready.texture.as_ref());
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            egui::Id::unique("brush-strip-hover"),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| {
            let available = (ui.ctx().content_rect().width() - 56.0).max(48.0);
            let h = 110.0_f32.min(available / BRUSH_HOVER_ASPECT_RATIO);
            let size = vec2(h * BRUSH_HOVER_ASPECT_RATIO, h);
            ui.set_max_width(size.x);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter()
                .rect_filled(rect, 4.0, Color32::from_rgb(37, 39, 44));
            if let Some(texture) = texture {
                paint_brush_texture(ui, rect, texture);
            } else if let Some(texture) = fallback.filter(|_| ui.visuals().dark_mode) {
                // Fit the existing white stroke into its section while the hover job runs.
                let left = Rect::from_min_size(rect.min, vec2(h * 3.0, h));
                let native = texture.size_vec2();
                let scale = (left.width() / native.x).min(left.height() / native.y);
                paint_brush_texture(
                    ui,
                    Rect::from_center_size(left.center(), native * scale),
                    texture,
                );
            }
            ui.strong(&preset.name);
            ui.weak(&preset.detail);
        });
    }
}

fn paint_brush_texture(ui: &Ui, rect: Rect, texture: &egui::TextureHandle) {
    ui.painter().image(
        texture.id(),
        rect,
        Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn pastel_pair(seed: uuid::Uuid) -> [[u8; 3]; 2] {
    let bytes = seed.as_bytes();
    let unit = |index| f32::from(bytes[index]) / 255.0;
    let first = Oklch {
        l: 0.71 + 0.08 * unit(2),
        c: 0.09 + 0.025 * unit(3),
        h: 360.0 * f32::from(u16::from_le_bytes([bytes[0], bytes[1]])) / 65_536.0,
    };
    let offset = [105.0, 125.0, 145.0][usize::from(bytes[6] % 3)];
    let signed_offset = if bytes[7] & 1 == 0 { offset } else { -offset };
    let second = Oklch {
        l: 0.71 + 0.08 * unit(4),
        c: 0.09 + 0.025 * unit(5),
        ..first.rotate_hue(signed_offset)
    };
    [first, second].map(|ink| {
        let (r, g, b) = ink.to_rgb();
        [r, g, b]
    })
}

fn cell_preview_rect(cell: Rect) -> Rect {
    let area = Rect::from_min_size(
        cell.min + vec2(4.0, 2.0),
        vec2(
            (cell.width() - 10.0).max(1.0),
            (cell.height() - 18.0).max(1.0),
        ),
    );
    // Keep space for both the isolated tip and the stroke, including narrow cells.
    let height = area.height().min(area.width() / 2.5);
    Rect::from_center_size(area.center(), vec2(area.width(), height))
}

fn raster_dimensions(rect: Rect, dpi: f32) -> (u32, u32) {
    // Screen-space row positions move fractionally while scrolling. Round away
    // subpixel error before using the dimensions as the texture cache key.
    (
        (rect.width() * dpi).round().clamp(32.0, 512.0) as u32,
        (rect.height() * dpi).round().clamp(16.0, 192.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::brushes::{
        BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape,
    };

    #[test]
    fn hover_pairs_are_distinct_and_keep_moderate_perceptual_lightness() {
        let mut pairs = BTreeSet::new();
        for hue in (0..u16::MAX).step_by(1024) {
            let [lo, hi] = hue.to_le_bytes();
            let mut seed = [lo; 16];
            seed[1] = hi;
            seed[3] = 255;
            seed[5] = 255;
            seed[6] = hi;
            seed[7] = hi;
            let pair = pastel_pair(uuid::Uuid::from_bytes(seed));
            assert!(pairs.insert(pair));
            let [a, b] =
                pair.map(|[r, g, b]| little_clip_core::Oklab::from_rgb(r, g, b).to_oklch());
            assert!(
                [a, b].into_iter().all(|ink| {
                    (0.70..=0.82).contains(&ink.l) && (0.07..=0.13).contains(&ink.c)
                })
            );
            let separation = (b.h - a.h).rem_euclid(360.0);
            assert!((95.0..=155.0).contains(&separation.min(360.0 - separation)));
        }
    }

    #[test]
    fn overlap_seed_changes_after_hover_ends() {
        let mut hover = HoverRaster::default();
        let source = uuid::Uuid::new_v4();
        let mut original = None;
        hover.request_with(source, 0, 140, |seed| {
            original = Some(seed);
            Source::Text(Default::default())
        });
        assert!(!hover.retain(|_| true));
        hover.request_with(source, 0, 140, |_| panic!("active hover was rerendered"));
        assert!(!hover.retain(|_| true));
        hover.request_with(source, 0, 280, |seed| {
            assert_eq!(Some(seed), original);
            Source::Text(Default::default())
        });
        assert!(!hover.retain(|_| true));
        assert!(hover.retain(|_| true));
        hover.request_with(source, 0, 140, |seed| {
            assert_ne!(Some(seed), original);
            Source::Text(Default::default())
        });
    }

    #[test]
    fn fractional_vertical_scroll_keeps_the_raster_size() {
        for y in [-123.335, 0.0, 12.1, 50.08, 400.30] {
            let cell = Rect::from_min_size(pos2(10.0, y), vec2(260.0, 94.0));
            assert_eq!(raster_dimensions(cell_preview_rect(cell), 1.0), (250, 76));
        }
    }

    #[test]
    fn wide_and_narrow_cells_show_the_tip_and_stroke() {
        let preset = BrushPreset {
            name: "Circle".into(),
            tip: BrushTipKind::Parametric {
                shape: ParametricShape::Circle,
                hardness: 0.8,
                aspect_ratio: 1.0,
                angle_deg: 0.0,
                spikes: 2,
            },
            params: BrushStrokeParams::default(),
            detail: String::new(),
        };
        for width in [90.0, 260.0] {
            let cell = Rect::from_min_size(pos2(10.0, 24.0), vec2(width, 94.0));
            let preview = cell_preview_rect(cell);
            assert!(cell.contains_rect(preview));
            let (width, height) = raster_dimensions(preview, 1.0);
            let image = render_brush_preview(&preset, width, height, [240; 3], true);
            let alpha =
                |x: u32, y: u32| image.premultiplied_rgba[((y * image.width + x) * 4 + 3) as usize];
            let middle = image.height / 2;
            assert!(alpha(image.height / 2, middle) > 0, "tip stamp");
            assert_eq!(alpha(image.height - 3, middle), 0, "gap after the stamp");
            assert!(
                (image.height..image.width).any(|x| alpha(x, middle) > 0),
                "stroke"
            );
        }
    }
}
