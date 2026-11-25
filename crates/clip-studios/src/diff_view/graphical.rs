use super::{ImageDiffViewMode as Mode, svg_view};
use crate::{icons::Icon, ui_ext};
use egui::{Color32, Context, Painter, Pos2, Rect, Stroke, Ui, Vec2};
use little_clip_core::palette::AppPalette;
use little_clip_render::diagram::retained::compare::{Comparison, Preview};

struct Textures {
    pair: [egui::TextureHandle; 2],
    overlay: [Option<egui::TextureHandle>; 2],
}

pub(super) struct GraphicalView {
    textures: Option<Textures>,
    split: f32,
}

impl Default for GraphicalView {
    fn default() -> Self {
        Self {
            textures: None,
            split: 0.5,
        }
    }
}

impl GraphicalView {
    pub fn clear_textures(&mut self) {
        self.textures = None;
    }

    pub fn clear_overlay(&mut self) {
        if let Some(textures) = &mut self.textures {
            textures.overlay = [None, None];
        }
    }

    pub fn prepare_overlay(&mut self, ctx: &Context, index: usize, image: egui::ColorImage) {
        if let Some(textures) = &mut self.textures {
            textures.overlay[index] =
                Some(ctx.load_texture("compare-overlay", image, egui::TextureOptions::LINEAR));
        }
    }

    pub fn prepare_svg(&mut self, ctx: &Context, comparison: &Comparison) {
        if self.textures.is_some() {
            return;
        }
        let images = [&comparison.base, &comparison.revised].map(|preview| {
            let pixels = &preview.pixels;
            egui::ColorImage::from_rgba_premultiplied(
                [pixels.width() as usize, pixels.height() as usize],
                pixels.data(),
            )
        });
        self.prepare(ctx, images);
    }

    pub fn prepare(&mut self, ctx: &Context, images: [egui::ColorImage; 2]) {
        let [a, b] = images;
        let load = |name, image| ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
        self.textures = Some(Textures {
            pair: [load("compare-a", a), load("compare-b", b)],
            overlay: [None, None],
        });
    }

    pub fn render(&mut self, ui: &mut Ui, palette: &AppPalette, mode: Mode, pair: Pair<'_>) {
        let Some(textures) = &self.textures else {
            return;
        };
        let sources = pair.order.map(|index| &textures.pair[index]);
        let colors = [
            crate::hex_to_c32(&palette.diff_del),
            crate::hex_to_c32(&palette.diff_add),
        ];
        let blink = blink_operand(ui, mode);
        labels(ui, mode, pair.labels, colors, blink);

        let (area, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(
            area,
            4.0,
            crate::hex_to_c32(&palette.colors.surface_recessed),
        );
        let Some(layout) = Layout::fit(area, mode, pair.sizes) else {
            return;
        };
        match mode {
            Mode::SideBySide => {
                for (index, color) in colors.into_iter().enumerate() {
                    let sheet = layout.sheets[index];
                    painter.rect_filled(sheet, 0.0, Color32::WHITE);
                    paint_image(&painter, sources[index], layout.views[index], 255);
                    pair.marks(&painter, &layout, index, color);
                }
            }
            Mode::Blink => {
                painter.rect_filled(layout.sheets[blink], 0.0, Color32::WHITE);
                paint_image(&painter, sources[blink], layout.views[blink], 255);
            }
            Mode::Overlay => {
                painter.rect_filled(layout.sheets[0], 0.0, Color32::WHITE);
                if let Some(texture) = &textures.overlay[pair.order[0]] {
                    paint_image(&painter, texture, layout.views[0], 255);
                } else {
                    paint_image(&painter, sources[0], layout.views[0], 255);
                    paint_image(&painter, sources[1], layout.views[1], 150);
                    for (index, color) in colors.into_iter().enumerate() {
                        pair.marks(&painter, &layout, index, color);
                    }
                }
            }
            Mode::Slider => {
                let sheet = layout.sheets[0];
                let x = drag_divider(ui, sheet, &mut self.split);
                let clips = [
                    Rect::from_min_max(sheet.min, Pos2::new(x, sheet.bottom())),
                    Rect::from_min_max(Pos2::new(x, sheet.top()), sheet.max),
                ];
                painter.rect_filled(sheet, 0.0, Color32::WHITE);
                for (index, clip) in clips.into_iter().enumerate() {
                    paint_image(
                        &painter.with_clip_rect(clip.intersect(painter.clip_rect())),
                        sources[index],
                        layout.views[index],
                        255,
                    );
                }
                paint_divider(&painter, sheet, x, palette);
            }
        }
    }
}

pub(super) fn toolbar(ui: &mut Ui, mode: &mut Mode, palette: &AppPalette) {
    use crate::ui_tabs::{Tab, TabBar};
    let tabs = [
        Tab::new(Mode::SideBySide, "Side by side").icon(Icon::Grid),
        Tab::new(Mode::Blink, "Blink").icon(Icon::Eye),
        Tab::new(Mode::Overlay, "Overlay").icon(Icon::Layers),
        Tab::new(Mode::Slider, "Slider").icon(Icon::Diff),
    ];
    if let Some(selected) = TabBar::new("graphical_compare_views", *mode, &tabs, palette).show(ui) {
        *mode = selected;
    }
}

pub(super) struct Pair<'a> {
    pub labels: [&'a str; 2],
    pub sizes: [Vec2; 2],
    pub svg: Option<&'a Comparison>,
    pub order: [usize; 2],
}

impl Pair<'_> {
    fn marks(&self, painter: &Painter, layout: &Layout, index: usize, color: Color32) {
        let Some(svg) = self.svg else { return };
        let preview: &Preview = [&svg.base, &svg.revised][index];
        let viewport = layout.views[index];
        if svg.viewport_changed {
            painter.rect_stroke(
                viewport,
                0.0,
                Stroke::new(2.0, color),
                egui::StrokeKind::Outside,
            );
        }
        svg_view::paint_marks(painter, viewport, preview, layout.scale, color);
    }
}

struct Layout {
    sheets: [Rect; 2],
    views: [Rect; 2],
    scale: f32,
}

impl Layout {
    fn fit(area: Rect, mode: Mode, sizes: [Vec2; 2]) -> Option<Self> {
        let sides = if mode == Mode::SideBySide {
            let middle = area.center().x;
            [
                Rect::from_min_max(area.min, Pos2::new(middle - 4.0, area.max.y)),
                Rect::from_min_max(Pos2::new(middle + 4.0, area.min.y), area.max),
            ]
        } else {
            [area; 2]
        };
        let extent = sizes[0].max(sizes[1]);
        let available = sides[0].shrink(10.0).size();
        let scale = (available.x / extent.x).min(available.y / extent.y);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let sheets = sides.map(|side| Rect::from_center_size(side.center(), extent * scale));
        let views = std::array::from_fn(|index| {
            Rect::from_min_size(sheets[index].min, sizes[index] * scale)
        });
        Some(Self {
            sheets,
            views,
            scale,
        })
    }
}

fn labels(ui: &mut Ui, mode: Mode, labels: [&str; 2], colors: [Color32; 2], blink: usize) {
    ui.columns(2, |columns| {
        if mode == Mode::Blink {
            columns[0].add(
                egui::Label::new(format!("{} · {}", ["A", "B"][blink], labels[blink])).truncate(),
            );
            columns[1].label("Hold Space to show B");
            return;
        }
        for (index, (ui, label)) in columns.iter_mut().zip(labels).enumerate() {
            ui.horizontal(|ui| {
                if matches!(mode, Mode::Overlay | Mode::SideBySide) {
                    ui.colored_label(colors[index], "■");
                }
                ui.add(egui::Label::new(format!("{} · {label}", ["A", "B"][index])).truncate())
                    .on_hover_text(label);
            });
        }
    });
}

fn blink_operand(ui: &Ui, mode: Mode) -> usize {
    usize::from(
        mode == Mode::Blink && ui.input(|input| input.focused && input.key_down(egui::Key::Space)),
    )
}

fn paint_image(painter: &Painter, texture: &egui::TextureHandle, viewport: Rect, alpha: u8) {
    painter.image(
        texture.id(),
        viewport,
        Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::from_white_alpha(alpha),
    );
}

fn drag_divider(ui: &Ui, sheet: Rect, split: &mut f32) -> f32 {
    let x = egui::lerp(sheet.x_range(), *split);
    let width = ui_ext::CONTROL_HEIGHT.max(26.0);
    let hit = Rect::from_center_size(
        egui::pos2(x, sheet.center().y),
        egui::vec2(width, sheet.height()),
    );
    let response = ui
        .interact(
            hit,
            ui.scope_id().with("compare-divider"),
            egui::Sense::drag(),
        )
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        if let Some(pointer) = response.interact_pointer_pos() {
            *split = ((pointer.x - sheet.left()) / sheet.width()).clamp(0.0, 1.0);
        }
    }
    egui::lerp(sheet.x_range(), *split)
}

fn paint_divider(painter: &Painter, sheet: Rect, x: f32, palette: &AppPalette) {
    let line = [egui::pos2(x, sheet.top()), egui::pos2(x, sheet.bottom())];
    painter.line_segment(line, Stroke::new(3.0, Color32::from_black_alpha(150)));
    painter.line_segment(line, Stroke::new(1.5, Color32::WHITE));
    ui_ext::paint_split_handle(painter, egui::pos2(x, sheet.center().y), palette);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        view: &mut GraphicalView,
        ctx: &Context,
        mode: Mode,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        clip_ui::test_support::run_frame(
            ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        if view.textures.is_none() {
                            view.prepare(
                                ctx,
                                [Color32::RED, Color32::GREEN]
                                    .map(|color| egui::ColorImage::filled([2, 2], color)),
                            );
                        }
                        view.render(
                            ui,
                            &AppPalette::default(),
                            mode,
                            Pair {
                                labels: ["Base", "Revision"],
                                sizes: [Vec2::splat(2.0); 2],
                                svg: None,
                                order: [0, 1],
                            },
                        );
                    })
                });
            },
        )
    }

    fn painted_image(output: &egui::FullOutput) -> egui::TextureId {
        let images: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.texture_id),
                _ => None,
            })
            .collect();
        let [image] = images.as_slice() else {
            panic!("expected one painted image")
        };
        *image
    }

    #[test]
    fn blink_uses_b_only_while_space_is_held() {
        let (ctx, mut view) = (Context::default(), GraphicalView::default());
        let idle = render(&mut view, &ctx, Mode::Blink, vec![]);
        let [a, b] = view
            .textures
            .as_ref()
            .unwrap()
            .pair
            .each_ref()
            .map(egui::TextureHandle::id);
        assert_eq!(painted_image(&idle), a);
        let key = |pressed| egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let held = render(&mut view, &ctx, Mode::Blink, vec![key(true)]);
        assert_eq!(painted_image(&held), b);
        let released = render(&mut view, &ctx, Mode::Blink, vec![key(false)]);
        assert_eq!(painted_image(&released), a);
    }

    #[test]
    fn slider_drag_moves_the_clip_boundary_for_both_images() {
        let (ctx, mut view) = (Context::default(), GraphicalView::default());
        let initial = render(&mut view, &ctx, Mode::Slider, vec![]);
        let image_center = initial
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds().center()),
                _ => None,
            })
            .unwrap();
        let point = image_center + Vec2::new(60.0, 0.0);
        render(
            &mut view,
            &ctx,
            Mode::Slider,
            vec![
                egui::Event::PointerMoved(image_center),
                egui::Event::PointerButton {
                    pos: image_center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let moved = render(
            &mut view,
            &ctx,
            Mode::Slider,
            vec![egui::Event::PointerMoved(point)],
        );
        assert!(view.split > 0.6);
        let clips: Vec<_> = moved
            .shapes
            .iter()
            .filter_map(|shape| {
                matches!(shape.shape, egui::Shape::Mesh(_)).then_some(shape.clip_rect)
            })
            .collect();
        let [left, right] = clips.as_slice() else {
            panic!("expected two clipped images")
        };
        assert!((left.right() - point.x).abs() < 1.0);
        assert_eq!(left.right(), right.left());
    }
}
