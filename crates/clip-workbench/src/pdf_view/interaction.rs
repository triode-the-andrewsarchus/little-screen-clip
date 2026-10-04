use super::*;
use little_clip_core::pdf::PdfPageLayout;

#[derive(Default)]
pub(super) struct PdfInteraction {
    anchor: Option<usize>,
    endpoint: Option<usize>,
    selecting: bool,
    pub image: Option<usize>,
}

pub(super) fn element_rect(page: egui::Rect, rect: [f32; 4]) -> egui::Rect {
    egui::Rect::from_min_max(
        page.min + page.size() * egui::vec2(rect[0], rect[1]),
        page.min + page.size() * egui::vec2(rect[2], rect[3]),
    )
}

impl PdfInteraction {
    fn text_hit(
        layout: &PdfPageLayout,
        page: egui::Rect,
        pointer: egui::Pos2,
        nearest: bool,
    ) -> Option<usize> {
        let glyph = layout
            .glyphs
            .iter()
            .filter(|glyph| nearest || element_rect(page, glyph.rect).contains(pointer))
            .min_by(|a, b| {
                element_rect(page, a.rect)
                    .distance_to_pos(pointer)
                    .total_cmp(&element_rect(page, b.rect).distance_to_pos(pointer))
            })?;
        let rect = element_rect(page, glyph.rect);
        Some(if pointer.x < rect.center().x {
            glyph.start
        } else {
            glyph.end
        })
    }

    fn image_hit(layout: &PdfPageLayout, page: egui::Rect, pointer: egui::Pos2) -> Option<usize> {
        layout
            .images
            .iter()
            .rev()
            .find(|image| element_rect(page, image.rect).contains(pointer))
            .map(|image| image.index)
    }

    fn selected_range(&self) -> Option<std::ops::Range<usize>> {
        let (anchor, endpoint) = (self.anchor?, self.endpoint?);
        (anchor != endpoint).then_some(anchor.min(endpoint)..anchor.max(endpoint))
    }

    pub fn selected_text<'a>(&self, layout: &'a PdfPageLayout) -> Option<&'a str> {
        layout.text.get(self.selected_range()?)
    }

    /// Called before the viewport pans, so starting a text drag never moves the page.
    pub fn prepare(&mut self, ui: &egui::Ui, page: egui::Rect, layout: &PdfPageLayout) -> bool {
        let (pressed, position, space) = ui.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.interact_pos(),
                input.key_down(egui::Key::Space),
            )
        });
        if pressed
            && let Some(pointer) = position.filter(|pointer| ui.clip_rect().contains(*pointer))
        {
            self.image = None;
            self.anchor = if space {
                None
            } else {
                Self::text_hit(layout, page, pointer, false)
            };
            self.endpoint = self.anchor;
            self.selecting = self.anchor.is_some();
        }
        !self.selecting || space
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        page: egui::Rect,
        layout: &PdfPageLayout,
        query: &str,
    ) -> Option<usize> {
        self.update_drag(ui, page, layout);
        let copy_image = self.copy_request(ui, response, page, layout);
        self.paint_overlay(ui, response.rect, page, layout, query);
        if response.hovered()
            && let Some(pointer) = response.hover_pos()
            && Self::text_hit(layout, page, pointer, false).is_some()
        {
            ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::Text);
        }
        copy_image
    }

    fn update_drag(&mut self, ui: &egui::Ui, page: egui::Rect, layout: &PdfPageLayout) {
        if self.selecting {
            if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                self.endpoint = Self::text_hit(layout, page, pointer, true);
            }
            if !ui.input(|input| input.pointer.primary_down()) {
                self.selecting = false;
            }
        }
    }

    fn copy_request(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        page: egui::Rect,
        layout: &PdfPageLayout,
    ) -> Option<usize> {
        let mut copy_image = None;
        if response.secondary_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.image = Self::image_hit(layout, page, pointer);
            copy_image = self.image;
            self.anchor = None;
            self.endpoint = None;
        }
        let copy = !ui.ctx().wants_keyboard_input()
            && ui.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy))
                    || (input.modifiers.ctrl && input.key_pressed(egui::Key::C))
            });
        if copy {
            if let Some(text) = self.selected_text(layout) {
                ui.output_mut(|output| output.copied_text = text.to_owned());
            } else {
                copy_image = self.image;
            }
        }
        copy_image
    }

    fn paint_overlay(
        &self,
        ui: &egui::Ui,
        viewport: egui::Rect,
        page: egui::Rect,
        layout: &PdfPageLayout,
        query: &str,
    ) {
        let painter = ui.painter_at(viewport);
        let selected = self.selected_range();
        let matches = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(true)
            .build()
            .ok()
            .filter(|_| !query.trim().is_empty())
            .map(|regex| {
                regex
                    .find_iter(&layout.text)
                    .map(|matched| matched.range())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for glyph in &layout.glyphs {
            let color = if selected
                .as_ref()
                .is_some_and(|range| glyph.end > range.start && glyph.start < range.end)
            {
                Some(egui::Color32::from_rgba_unmultiplied(60, 140, 255, 85))
            } else if matches
                .get(matches.partition_point(|range| range.end <= glyph.start))
                .is_some_and(|range| glyph.end > range.start && glyph.start < range.end)
            {
                Some(egui::Color32::from_rgba_unmultiplied(255, 185, 20, 85))
            } else {
                None
            };
            if let Some(color) = color {
                painter.rect_filled(element_rect(page, glyph.rect), 0.0, color);
            }
        }
        if let Some(image) = layout
            .images
            .iter()
            .find(|image| Some(image.index) == self.image)
        {
            painter.rect_stroke(
                element_rect(page, image.rect),
                0.0,
                Stroke::new(2.0_f32, egui::Color32::RED),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::pdf::{PdfGlyph, PdfImageElement};
    #[test]
    fn find_highlight_excludes_text_before_the_match() {
        let layout = PdfPageLayout {
            text: "a b".into(),
            glyphs: vec![
                PdfGlyph {
                    start: 0,
                    end: 1,
                    rect: [0.0, 0.0, 0.1, 0.1],
                },
                PdfGlyph {
                    start: 2,
                    end: 3,
                    rect: [0.2, 0.0, 0.3, 0.1],
                },
            ],
            ..Default::default()
        };
        let page = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let output = egui::Context::default().run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                PdfInteraction::default().paint_overlay(ui, page, page, &layout, "b");
            });
        });
        let highlights = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Rect(rect)
                    if rect.fill == egui::Color32::from_rgba_unmultiplied(255, 185, 20, 85) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(highlights, vec![element_rect(page, layout.glyphs[1].rect)]);
    }
    #[test]
    fn text_selection_uses_utf8_offsets_in_both_drag_directions() {
        let layout = PdfPageLayout {
            text: "Hello 日本語\nNext".into(),
            ..Default::default()
        };
        for (anchor, endpoint) in [(6, 15), (15, 6)] {
            let selection = PdfInteraction {
                anchor: Some(anchor),
                endpoint: Some(endpoint),
                ..Default::default()
            };
            assert_eq!(selection.selected_text(&layout), Some("日本語"));
        }
    }
    #[test]
    fn scaled_page_hit_testing_selects_text_and_topmost_image_independently() {
        let page = egui::Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(400.0, 600.0));
        let layout = PdfPageLayout {
            text: "A".into(),
            glyphs: vec![PdfGlyph {
                start: 0,
                end: 1,
                rect: [0.1, 0.1, 0.2, 0.2],
            }],
            images: vec![
                PdfImageElement {
                    index: 0,
                    rect: [0.0, 0.0, 1.0, 1.0],
                },
                PdfImageElement {
                    index: 1,
                    rect: [0.1, 0.1, 0.2, 0.2],
                },
            ],
        };
        let pointer = egui::pos2(145.0, 265.0);
        assert_eq!(
            PdfInteraction::text_hit(&layout, page, pointer, false),
            Some(0)
        );
        assert_eq!(PdfInteraction::image_hit(&layout, page, pointer), Some(1));
        assert_eq!(
            PdfInteraction::image_hit(&layout, page, egui::pos2(480.0, 780.0)),
            Some(0)
        );
    }
}
