use super::*;
use egui::{Pos2, Rect, StrokeKind};
use little_clip_core::{palette::AppPalette, svg_image};
use little_clip_render::diagram::retained::compare::{Comparison, Preview};

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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum View {
    #[default]
    SideBySide,
    Overlay,
}

#[derive(Default)]
pub(super) struct SvgView {
    pub result: Option<Comparison>,
    view: View,
    textures: Option<[egui::TextureHandle; 2]>,
}

impl SvgView {
    pub fn clear_result(&mut self) {
        self.result = None;
        self.textures = None;
    }

    pub fn toggle(&mut self) {
        self.view = match self.view {
            View::SideBySide => View::Overlay,
            View::Overlay => View::SideBySide,
        };
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        use crate::ui_tabs::{Tab, TabBar};
        let tabs = [
            Tab::new(View::SideBySide, "Side by side").icon(Icon::Grid),
            Tab::new(View::Overlay, "Overlay").icon(Icon::Layers),
        ];
        if let Some(view) = TabBar::new("svg_compare_views", self.view, &tabs, palette).show(ui) {
            self.view = view;
        }
    }

    fn ensure_textures(&mut self, ctx: &egui::Context) {
        let Some(result) = &self.result else { return };
        self.textures.get_or_insert_with(|| {
            [
                ("svg-compare-base", &result.base),
                ("svg-compare-revised", &result.revised),
            ]
            .map(|(label, preview)| {
                let pixels = &preview.pixels;
                let size = [pixels.width() as usize, pixels.height() as usize];
                let image = egui::ColorImage::from_rgba_premultiplied(size, pixels.data());
                ctx.load_texture(label, image, egui::TextureOptions::LINEAR)
            })
        });
    }

    pub fn render(&mut self, ui: &mut egui::Ui, palette: &AppPalette, labels: [&str; 2]) {
        self.ensure_textures(ui.ctx());
        let (Some(result), Some(textures)) = (&self.result, &self.textures) else {
            return;
        };
        let colors = [
            crate::hex_to_c32(&palette.diff_del),
            crate::hex_to_c32(&palette.diff_add),
        ];
        let surface = crate::hex_to_c32(&palette.colors.surface_recessed);
        ui.columns(2, |columns| {
            for (index, (ui, label)) in columns.iter_mut().zip(labels).enumerate() {
                ui.horizontal(|ui| {
                    let side = if index == 0 { "Base" } else { "Comparison" };
                    ui.colored_label(colors[index], "■");
                    ui.add(egui::Label::new(label).truncate())
                        .on_hover_text(format!("{side} · {label}"));
                });
            }
        });

        let (area, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 4.0, surface);
        let sides = match self.view {
            View::SideBySide => {
                let middle = area.center().x;
                [
                    Rect::from_min_max(area.min, Pos2::new(middle - 4.0, area.max.y)),
                    Rect::from_min_max(Pos2::new(middle + 4.0, area.min.y), area.max),
                ]
            }
            View::Overlay => [area; 2],
        };
        let previews = [&result.base, &result.revised];
        let extent = Vec2::new(
            result.base.viewport[0].max(result.revised.viewport[0]),
            result.base.viewport[1].max(result.revised.viewport[1]),
        );
        let available = sides[0].shrink(10.0).size();
        let scale = (available.x / extent.x).min(available.y / extent.y);
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let sheets = sides.map(|side| Rect::from_center_size(side.center(), extent * scale));
        let viewports = std::array::from_fn::<_, 2, _>(|index| {
            let size = previews[index].viewport;
            Rect::from_min_size(sheets[index].min, egui::vec2(size[0], size[1]) * scale)
        });
        for sheet in sheets {
            painter.rect_filled(sheet, 0.0, Color32::WHITE);
        }
        for (index, texture) in textures.iter().enumerate() {
            let alpha = if index == 1 && self.view == View::Overlay {
                150
            } else {
                255
            };
            painter.image(
                texture.id(),
                viewports[index],
                Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::from_white_alpha(alpha),
            );
        }
        for (index, preview) in previews.into_iter().enumerate() {
            let viewport = viewports[index];
            if result.viewport_changed {
                painter.rect_stroke(
                    viewport,
                    0.0,
                    Stroke::new(2.0, colors[index]),
                    StrokeKind::Outside,
                );
            }
            paint_marks(&painter, viewport, preview, scale, colors[index]);
        }
    }
}

fn paint_marks(
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
