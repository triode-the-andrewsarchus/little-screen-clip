//! Shared workspace and tool navigation. Parameter choices and on/off toggles use buttons.
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::{
    BUTTON_PADDING_X, CONTROL_CORNER_RADIUS, CONTROL_HEIGHT, CONTROL_ICON_SIZE, SPACE_SM,
    contrast_text_c32, wrapped_equal_button_width,
};
use egui::{Color32, Id, Key, Rect, Response, Rounding, Sense, Stroke, TextStyle, Ui, Vec2};
use little_clip_core::palette::AppPalette;

pub struct Tab<'a, T> {
    value: T,
    label: &'a str,
    icon: Option<Icon>,
}

impl<'a, T> Tab<'a, T> {
    pub fn new(value: T, label: &'a str) -> Self {
        Self {
            value,
            label,
            icon: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    fn show(
        &self,
        ui: &mut Ui,
        selected: bool,
        vertical: bool,
        width: Option<f32>,
        focused: bool,
        palette: &AppPalette,
    ) -> Response {
        let colors = WorkbenchColors::from_palette(palette);
        let font = TextStyle::Button.resolve(ui.style());
        let galley = ui.fonts(|fonts| {
            fonts.layout_no_wrap(self.label.to_owned(), font.clone(), Color32::WHITE)
        });
        let icon_width = self.icon.map_or(0.0, |_| CONTROL_ICON_SIZE + SPACE_SM);
        let natural_width = galley.size().x.ceil() + icon_width + 2.0 * BUTTON_PADDING_X + 1.0;
        let size = Vec2::new(
            width
                .unwrap_or(natural_width)
                .min(ui.max_rect().width())
                .max(1.0),
            CONTROL_HEIGHT,
        );
        let (rect, response) = ui.allocate_exact_size(
            size,
            Sense {
                focusable: false,
                ..Sense::click()
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, self.label)
        });
        let background = if selected {
            colors.raised
        } else if response.hovered() {
            colors.panel
        } else {
            colors.canvas
        };
        let foreground = contrast_text_c32(
            background,
            Some(if selected {
                colors.accent
            } else {
                colors.text_secondary
            }),
            palette,
        );
        let text_width = (rect.width() - icon_width - 2.0 * BUTTON_PADDING_X).max(1.0);
        let galley = if galley.size().x > text_width + 1.0 {
            let mut text_job =
                egui::text::LayoutJob::simple(self.label.to_owned(), font, foreground, text_width);
            text_job.wrap.max_rows = 1;
            text_job.wrap.break_anywhere = true;
            ui.fonts(|fonts| fonts.layout_job(text_job))
        } else {
            galley
        };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let rounding = Rounding {
            nw: CONTROL_CORNER_RADIUS,
            ne: CONTROL_CORNER_RADIUS,
            sw: 0.0,
            se: 0.0,
        };
        painter.rect(
            rect,
            rounding,
            background,
            if selected {
                Stroke::new(1.0_f32, colors.border)
            } else {
                Stroke::NONE
            },
        );
        let indicator = if vertical {
            Rect::from_min_size(
                rect.min,
                Vec2::new(if selected { 3.0 } else { 1.0 }, rect.height()),
            )
        } else {
            Rect::from_min_size(
                egui::pos2(
                    rect.left(),
                    rect.bottom() - if selected { 3.0 } else { 1.0 },
                ),
                Vec2::new(rect.width(), if selected { 3.0 } else { 1.0 }),
            )
        };
        if selected {
            painter.add(crate::ui_ext::gradient_surface(
                indicator,
                0.0,
                colors.accent,
                colors.focus,
                !vertical,
            ));
        } else {
            painter.rect_filled(indicator, Rounding::ZERO, colors.border);
        }
        let content_x = if vertical {
            rect.left() + BUTTON_PADDING_X
        } else {
            rect.center().x - (galley.size().x + icon_width) * 0.5
        };
        if let Some(icon) = self.icon {
            icon.image_tinted(CONTROL_ICON_SIZE, foreground).paint_at(
                ui,
                Rect::from_min_size(
                    egui::pos2(content_x, rect.center().y - CONTROL_ICON_SIZE * 0.5),
                    Vec2::splat(CONTROL_ICON_SIZE),
                ),
            );
        }
        painter.galley_with_override_text_color(
            egui::pos2(
                content_x + icon_width,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            foreground,
        );
        if focused && selected {
            painter.rect_stroke(rect.shrink(2.0), rounding, Stroke::new(1.0_f32, foreground));
        }
        response.on_hover_text(self.label)
    }
}

/// A wrapping tab strip, or a vertical tool rail. Returns only actual selection changes.
pub struct TabBar<'a, T> {
    id: Id,
    selected: T,
    tabs: &'a [Tab<'a, T>],
    palette: &'a AppPalette,
    equal_widths: bool,
    vertical: bool,
}

impl<'a, T: Copy + PartialEq> TabBar<'a, T> {
    pub fn new(
        id: impl std::hash::Hash,
        selected: T,
        tabs: &'a [Tab<'a, T>],
        palette: &'a AppPalette,
    ) -> Self {
        Self {
            id: Id::new(id),
            selected,
            tabs,
            palette,
            equal_widths: false,
            vertical: false,
        }
    }

    pub fn equal_widths(mut self) -> Self {
        self.equal_widths = true;
        self
    }

    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Option<T> {
        ui.push_id(self.id, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let width = if self.vertical {
                Some(ui.available_width())
            } else if self.equal_widths {
                let labels: Vec<_> = self.tabs.iter().map(|tab| tab.label).collect();
                Some(
                    wrapped_equal_button_width(
                        ui,
                        &labels,
                        self.tabs.iter().any(|tab| tab.icon.is_some()),
                        self.tabs.len(),
                    )
                    .floor(),
                )
            } else {
                None
            };
            let id = ui.make_persistent_id("tab_focus");
            let focused = ui.memory(|memory| memory.has_focus(id));
            let mut changed = None;
            let mut render = |ui: &mut Ui| {
                for tab in self.tabs {
                    let selected = tab.value == self.selected;
                    let response =
                        tab.show(ui, selected, self.vertical, width, focused, self.palette);
                    if response.clicked() && !selected {
                        changed = Some(tab.value);
                    }
                }
            };
            let row = if self.vertical {
                ui.vertical(&mut render)
            } else {
                ui.horizontal_wrapped(&mut render)
            };
            // The strip is one keyboard stop. Its focused indicator follows the active tab.
            let response = ui.interact(row.response.rect, id, Sense::focusable_noninteractive());
            if response.gained_focus() {
                ui.ctx().request_repaint();
            }
            if response.has_focus() {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            ..Default::default()
                        },
                    )
                });
                if let Some(index) = self.tabs.iter().position(|tab| tab.value == self.selected) {
                    if let Some(target) = self.keyboard_target(ui, index) {
                        if self.tabs[target].value != self.selected {
                            changed = Some(self.tabs[target].value);
                        }
                    }
                }
            }
            changed
        })
        .inner
    }

    fn keyboard_target(&self, ui: &mut Ui, index: usize) -> Option<usize> {
        let previous = if self.vertical {
            Key::ArrowUp
        } else {
            Key::ArrowLeft
        };
        let next = if self.vertical {
            Key::ArrowDown
        } else {
            Key::ArrowRight
        };
        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, previous) {
                Some((index + self.tabs.len() - 1) % self.tabs.len())
            } else if input.consume_key(egui::Modifiers::NONE, next) {
                Some((index + 1) % self.tabs.len())
            } else if input.consume_key(egui::Modifiers::NONE, Key::Home) {
                Some(0)
            } else if input.consume_key(egui::Modifiers::NONE, Key::End) {
                Some(self.tabs.len() - 1)
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, Pos2, RawInput};
    use little_clip_core::config::WorkbenchAppearance;

    fn render(
        ctx: &egui::Context,
        selected: usize,
        width: f32,
        vertical: bool,
        events: Vec<Event>,
    ) -> (egui::FullOutput, Option<usize>) {
        let palette = crate::ui_appearance::palette(WorkbenchAppearance::Dark);
        let tabs = [
            Tab::new(0, "First").icon(Icon::Image),
            Tab::new(1, "Second"),
            Tab::new(2, "Third"),
        ];
        let mut changed = None;
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let bar = TabBar::new("test_tabs", selected, &tabs, &palette).equal_widths();
                    changed = if vertical {
                        bar.vertical().show(ui)
                    } else {
                        bar.show(ui)
                    };
                });
            },
        );
        (output, changed)
    }

    fn tab_rects(output: &egui::FullOutput) -> Vec<Rect> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Rect(rect)
                    if (rect.rect.height() - CONTROL_HEIGHT).abs() < 0.1
                        && rect.rect.width() > 3.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect()
    }

    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn content_sized_tabs_show_their_full_labels() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = crate::ui_appearance::palette(WorkbenchAppearance::Light);
        let tabs = [
            Tab::new(0, "[V]iew").icon(Icon::Image),
            Tab::new(1, "[C]rop").icon(Icon::Crop),
            Tab::new(2, "Auto-Actions [F3]").icon(Icon::Settings),
        ];
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 400.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    TabBar::new("natural_tabs", 0, &tabs, &palette).show(ui);
                });
            },
        );
        for shape in &output.shapes {
            if let egui::epaint::Shape::Text(text) = &shape.shape {
                assert!(
                    !text.galley.elided,
                    "tab label was needlessly truncated: {}",
                    text.galley.text()
                );
                assert!(
                    shape
                        .clip_rect
                        .contains_rect(Rect::from_min_size(text.pos, text.galley.size()))
                );
            }
        }
    }

    #[test]
    fn tabs_wrap_keep_equal_widths_and_stay_inside_the_host() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        for (width, vertical) in [(180.0, false), (900.0, false), (180.0, true)] {
            let (output, changed) = render(&ctx, 0, width, vertical, Vec::new());
            assert!(changed.is_none());
            let rects = tab_rects(&output);
            assert_eq!(rects.len(), 3);
            for rect in &rects {
                assert!((rect.width() - rects[0].width()).abs() < 0.1);
                assert!(rect.left() >= 0.0 && rect.right() <= width);
            }
            assert_eq!(
                rects[0].top() == rects[2].top(),
                width == 900.0 && !vertical,
                "width={width}, vertical={vertical}, rects={rects:?}"
            );
        }
    }

    #[test]
    fn clicking_another_tab_changes_selection_and_active_click_is_a_noop() {
        let ctx = egui::Context::default();
        let (output, _) = render(&ctx, 0, 900.0, false, Vec::new());
        let position = tab_rects(&output)[1].center();
        let mut selected = 0;
        for expected_change in [Some(1), None] {
            let (_, change) = render(
                &ctx,
                selected,
                900.0,
                false,
                vec![
                    Event::PointerMoved(position),
                    Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
            assert!(change.is_none());
            let (_, change) = render(
                &ctx,
                selected,
                900.0,
                false,
                vec![Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                }],
            );
            assert_eq!(change, expected_change);
            if let Some(value) = change {
                selected = value;
            }
        }
    }

    #[test]
    fn focused_tabs_support_arrows_home_end_and_wraparound() {
        for vertical in [false, true] {
            let ctx = egui::Context::default();
            let _ = render(&ctx, 0, 900.0, vertical, Vec::new());
            let _ = render(&ctx, 0, 900.0, vertical, vec![key(Key::Tab)]);
            assert!(ctx.memory(|memory| memory.focused().is_some()));
            let _ = render(&ctx, 0, 900.0, vertical, Vec::new());
            let next = if vertical {
                Key::ArrowDown
            } else {
                Key::ArrowRight
            };
            let previous = if vertical {
                Key::ArrowUp
            } else {
                Key::ArrowLeft
            };
            let mut selected = 0;
            for (pressed, expected) in [
                (next, 1),
                (Key::End, 2),
                (next, 0),
                (previous, 2),
                (Key::Home, 0),
            ] {
                let (_, change) = render(&ctx, selected, 900.0, vertical, vec![key(pressed)]);
                assert_eq!(
                    change,
                    Some(expected),
                    "vertical={vertical}, key={pressed:?}, focus={:?}",
                    ctx.memory(|memory| memory.focused())
                );
                selected = expected;
            }
        }
    }
}
