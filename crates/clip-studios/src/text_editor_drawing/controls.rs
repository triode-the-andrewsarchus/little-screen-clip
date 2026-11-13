//! Fixed whiteboard tools and properties; no detached inspector windows.
use super::*;
use clip_ui::{icons::Icon, ui_appearance::WorkbenchColors};

impl DrawingEditor {
    pub(super) fn choose_tool(&mut self, tool: Option<Kind>, pan: bool, ctx: &egui::Context) {
        self.tool = tool;
        self.panning = pan;
        self.gesture = None;
        self.text_draft = None;
        self.text_focus = false;
        self.text_selection = None;
        if self.tool.is_some() {
            self.selected.clear();
        }
        if let Some(id) = self.focus {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
    }

    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        // A single row stays a single row even in the smaller workbench host.
        egui::ScrollArea::horizontal()
            .id_source("drawing_tools")
            .show(ui, |ui| {
                let tools_width = 9.0 * 40.0 + 8.0 * ui.spacing().item_spacing.x;
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - tools_width) / 2.0).max(0.0));
                    for (icon, kind, pan, label, key) in [
                        (Icon::Hand, None, true, "Pan", "H"),
                        (Icon::Cursor, None, false, "Select", "V"),
                        (
                            Icon::ShapeRect,
                            Some(Kind::Rectangle),
                            false,
                            "Rectangle",
                            "R",
                        ),
                        (
                            Icon::ShapeDiamond,
                            Some(Kind::Diamond),
                            false,
                            "Diamond",
                            "D",
                        ),
                        (
                            Icon::ShapeCircle,
                            Some(Kind::Ellipse),
                            false,
                            "Ellipse",
                            "O",
                        ),
                        (Icon::ArrowRight, Some(Kind::Arrow), false, "Arrow", "A"),
                        (Icon::ShapeLine, Some(Kind::Line), false, "Line", "L"),
                        (Icon::Pencil, Some(Kind::Freedraw), false, "Pencil", "P"),
                        (Icon::Type, Some(Kind::Text), false, "Text", "T"),
                    ] {
                        let selected = self.panning == pan && self.tool == kind;
                        let fill = if selected {
                            colors.accent
                        } else {
                            colors.raised
                        };
                        let response = ui.add(
                            clip_ui::ui_ext::icon_button(icon, 20.0, fill, None, palette)
                                .min_size(Vec2::splat(38.0)),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                selected,
                                label,
                            )
                        });
                        ui.painter().text(
                            response.rect.right_bottom() - egui::vec2(3.0, 1.0),
                            egui::Align2::RIGHT_BOTTOM,
                            key,
                            egui::FontId::proportional(9.0),
                            if selected {
                                clip_ui::ui_ext::contrast_text_c32(
                                    fill,
                                    Some(colors.text_primary),
                                    palette,
                                )
                            } else {
                                colors.text_muted
                            },
                        );
                        if response.on_hover_text(format!("{label} ({key})")).clicked() {
                            self.choose_tool(kind, pan, ui.ctx());
                        }
                    }
                });
            });
        ui.add_space(6.0);
    }

    pub(super) fn navigation(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let fill = ui.visuals().widgets.inactive.bg_fill;
        ui.horizontal(|ui| {
            if ui
                .add(clip_ui::ui_ext::icon_button(
                    Icon::ZoomOut,
                    16.0,
                    fill,
                    None,
                    palette,
                ))
                .on_hover_text("Zoom out")
                .clicked()
            {
                self.camera.scale = (self.camera.scale / 1.25).clamp(0.001, 16.0);
            }
            if ui
                .button(format!("{:.0}%", self.camera.zoom_percent()))
                .on_hover_text("Actual size")
                .clicked()
            {
                self.camera.scale = 1.0;
            }
            if ui
                .add(clip_ui::ui_ext::icon_button(
                    Icon::ZoomIn,
                    16.0,
                    fill,
                    None,
                    palette,
                ))
                .on_hover_text("Zoom in")
                .clicked()
            {
                self.camera.scale = (self.camera.scale * 1.25).clamp(0.001, 16.0);
            }
            if ui
                .add(clip_ui::ui_ext::icon_button(
                    Icon::ZoomFit,
                    16.0,
                    fill,
                    None,
                    palette,
                ))
                .on_hover_text("Fit drawing")
                .clicked()
            {
                self.camera.fitted = false;
            }
            if !self.selected.is_empty() {
                ui.weak(format!("{} selected", self.selected.len()));
            }
        });
    }

    pub(super) fn properties(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.label("Stroke");
            let stroke = self.style.stroke.clone();
            if let Some(color) = swatches(
                ui,
                &stroke,
                &["#1e1e1e", "#e03131", "#2f9e44", "#1971c2", "#f08c00"],
            ) {
                self.paint_control(Paint::Stroke(color));
            }
            ui.add_space(8.0);
            ui.label("Background");
            let fill = self.style.fill.clone();
            if let Some(color) = swatches(
                ui,
                &fill,
                &["transparent", "#ffc9c9", "#b2f2bb", "#a5d8ff", "#ffec99"],
            ) {
                self.paint_control(Paint::Fill(color));
            }
            ui.add_space(8.0);
            ui.label("Stroke width");
            ui.horizontal(|ui| {
                for (width, label) in [(1.0, "Thin"), (2.0, "Medium"), (4.0, "Thick")] {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(42.0, 30.0), egui::Sense::click());
                    let active = self.style.width == width;
                    ui.painter().rect_filled(
                        rect,
                        5.0,
                        if active {
                            ui.visuals().selection.bg_fill
                        } else {
                            ui.visuals().widgets.inactive.bg_fill
                        },
                    );
                    ui.painter().line_segment(
                        [
                            rect.center() - egui::vec2(10.0, 0.0),
                            rect.center() + egui::vec2(10.0, 0.0),
                        ],
                        egui::Stroke::new(width as f32, ui.visuals().text_color()),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, active, label)
                    });
                    if response.on_hover_text(label).clicked() {
                        self.paint_control(Paint::Width(width));
                    }
                }
            });
            ui.add_space(8.0);
            ui.label("Sloppiness");
            ui.horizontal(|ui| {
                for (roughness, label) in [(0.0, "Clean"), (1.0, "Sketch"), (2.0, "Rough")] {
                    if ui
                        .selectable_label(self.style.roughness == roughness, label)
                        .clicked()
                    {
                        self.paint_control(Paint::Roughness(roughness));
                    }
                }
            });
            ui.add_space(8.0);
            ui.label("Opacity");
            let response = ui.add(egui::Slider::new(&mut self.style.opacity, 0..=100).suffix("%"));
            // One accepted transaction per completed drag, never a write for
            // each pointer frame. Keyboard changes still apply immediately.
            if response.drag_stopped() || response.changed() && !response.dragged() {
                self.paint_control(Paint::Opacity(self.style.opacity));
            }
            self.selection_actions(ui, palette);
        });
    }
    fn selection_actions(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.tool.is_none() && !self.panning && !self.selected.is_empty() {
            ui.add_space(8.0);
            ui.label("Actions");
            ui.horizontal(|ui| {
                if ui
                    .add(clip_ui::ui_ext::icon_button(
                        Icon::Trash,
                        18.0,
                        ui.visuals().widgets.inactive.bg_fill,
                        None,
                        palette,
                    ))
                    .on_hover_text("Delete selection")
                    .clicked()
                {
                    self.queue(Change::Delete {
                        ids: self.selected.clone(),
                    });
                }
                if self.selected.len() == 1
                    && ui
                        .button("Add label")
                        .on_hover_text("Add a label to a selected rectangle without a label")
                        .clicked()
                {
                    self.new_text([0.0; 2], self.selected.first().cloned());
                }
            });
        }
    }
}

fn swatches(ui: &mut egui::Ui, selected: &str, colors: &[&str]) -> Option<String> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        for &color in colors {
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(27.0), egui::Sense::click());
            if color == "transparent" {
                ui.painter().rect_filled(rect, 4.0, Color32::WHITE);
                for (x, y) in [(0.0, 0.0), (1.0, 1.0)] {
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            rect.min + egui::vec2(x * 13.5, y * 13.5),
                            Vec2::splat(13.5),
                        ),
                        0.0,
                        Color32::LIGHT_GRAY,
                    );
                }
            } else if let Ok(value) = Color32::from_hex(color) {
                ui.painter().rect_filled(rect, 4.0, value);
            }
            if selected.eq_ignore_ascii_case(color) {
                ui.painter().rect_stroke(
                    rect.expand(2.0),
                    5.0,
                    egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
                );
            }
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    selected == color,
                    color,
                )
            });
            if response.on_hover_text(color).clicked() {
                chosen = Some(color.into());
            }
        }
    });
    chosen
}
