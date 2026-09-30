use crate::image_preview::{DecodedImageData, InlineImagePreview};
use crate::midi_view::InlineMidiPreview;
use crate::model3d_view::InlineModel3dPreview;
use crate::spreadsheet_view::InlineSpreadsheetPreview;
use crate::text_preview::{InlineTextPreview, TextPreviewData};
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::BODY_FONT_SIZE;
use crate::ui_ext::{
    self, action_button, badge_pill, mode_tab_button, secondary_button, themed_button,
};
use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};
use little_clip_core::palette::AppPalette;
use little_clip_core::presentation::midi::{MidiData, MidiNote};
use little_clip_core::presentation::model3d::{Model3d, ModelFormat, Triangle, Vec3};
use little_clip_core::presentation::spreadsheet::{SpreadsheetData, SpreadsheetSheet};
use std::sync::Arc;

/// Interactive samples of shared workbench components for visual review.
pub struct ComponentGalleryState {
    query: String,
    notes: String,
    level: f32,
    selected_mode: usize,
    enabled: bool,
    image_source: Arc<DecodedImageData>,
    text_source: Arc<TextPreviewData>,
    spreadsheet_source: Arc<SpreadsheetData>,
    midi_source: Arc<MidiData>,
    model3d_source: Arc<Model3d>,
    pending_open: Option<GalleryPreviewKind>,
    image_previews: [InlineImagePreview; 2],
    text_previews: [InlineTextPreview; 2],
    spreadsheet_previews: [InlineSpreadsheetPreview; 2],
    midi_previews: [InlineMidiPreview; 2],
    model3d_previews: [InlineModel3dPreview; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GalleryPreviewKind {
    Image,
    Text,
    Spreadsheet,
    Midi,
    Model3d,
}

pub(crate) enum GalleryOpenRequest {
    Image(Arc<DecodedImageData>),
    Text(Arc<TextPreviewData>),
    Spreadsheet(Arc<SpreadsheetData>),
    Midi(Arc<MidiData>),
    Model3d(Arc<Model3d>),
}

impl Default for ComponentGalleryState {
    fn default() -> Self {
        let image_source = gallery_image_source();
        let text_source = TextPreviewData::from_text(Arc::new(
            "gallery-line-alpha\ngallery-line-beta\ngallery-line-gamma\n".to_owned(),
        ));
        let spreadsheet_source = Arc::new(SpreadsheetData {
            title: "Gallery workbook".to_owned(),
            sheets: vec![SpreadsheetSheet::new(
                "Gallery".to_owned(),
                vec![
                    vec!["gallery-cell-alpha".to_owned(), "42".to_owned()],
                    vec!["gallery-cell-beta".to_owned(), "84".to_owned()],
                ],
            )],
        });
        let midi_source = Arc::new(MidiData {
            title: "Gallery MIDI".to_owned(),
            track_count: 1,
            track_names: vec!["Gallery track".to_owned()],
            total_notes: 4,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 67,
            notes: (0..4)
                .map(|index| MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 60 + index as u8,
                    velocity: 96,
                    start_sec: index as f32,
                    duration_sec: 0.5,
                })
                .collect(),
        });
        let model3d_source = gallery_model3d_source();
        Self {
            query: "Sample text".to_owned(),
            notes: "Multiline fields should remain comfortable to read.".to_owned(),
            level: 0.62,
            selected_mode: 0,
            enabled: true,
            image_source: Arc::clone(&image_source),
            text_source: Arc::clone(&text_source),
            spreadsheet_source: Arc::clone(&spreadsheet_source),
            midi_source: Arc::clone(&midi_source),
            model3d_source: Arc::clone(&model3d_source),
            pending_open: None,
            image_previews: [
                InlineImagePreview::new(Arc::clone(&image_source)),
                InlineImagePreview::new(image_source),
            ],
            text_previews: [
                InlineTextPreview::new(Arc::clone(&text_source), Vec2::new(190.0, 68.0)),
                InlineTextPreview::new(text_source, Vec2::new(190.0, 68.0)),
            ],
            spreadsheet_previews: [
                InlineSpreadsheetPreview::new(
                    Arc::clone(&spreadsheet_source),
                    Vec2::new(200.0, 84.0),
                ),
                InlineSpreadsheetPreview::new(spreadsheet_source, Vec2::new(200.0, 84.0)),
            ],
            midi_previews: [
                InlineMidiPreview::new(Arc::clone(&midi_source), Vec2::new(190.0, 76.0)),
                InlineMidiPreview::new(midi_source, Vec2::new(190.0, 76.0)),
            ],
            model3d_previews: [
                InlineModel3dPreview::new(Arc::clone(&model3d_source), Vec2::new(150.0, 88.0)),
                InlineModel3dPreview::new(model3d_source, Vec2::new(150.0, 88.0)),
            ],
        }
    }
}

impl ComponentGalleryState {
    pub(crate) fn request_open(&mut self, kind: GalleryPreviewKind) {
        self.pending_open = Some(kind);
    }

    pub(crate) fn take_pending_open(&mut self) -> Option<GalleryOpenRequest> {
        match self.pending_open.take()? {
            GalleryPreviewKind::Image => {
                Some(GalleryOpenRequest::Image(Arc::clone(&self.image_source)))
            }
            GalleryPreviewKind::Text => {
                Some(GalleryOpenRequest::Text(Arc::clone(&self.text_source)))
            }
            GalleryPreviewKind::Spreadsheet => Some(GalleryOpenRequest::Spreadsheet(Arc::clone(
                &self.spreadsheet_source,
            ))),
            GalleryPreviewKind::Midi => {
                Some(GalleryOpenRequest::Midi(Arc::clone(&self.midi_source)))
            }
            GalleryPreviewKind::Model3d => Some(GalleryOpenRequest::Model3d(Arc::clone(
                &self.model3d_source,
            ))),
        }
    }
}

fn gallery_image_source() -> Arc<DecodedImageData> {
    let image = image::RgbaImage::from_fn(24, 24, |x, y| {
        if (x / 6 + y / 6) % 2 == 0 {
            image::Rgba([255, 178, 112, 255])
        } else {
            image::Rgba([83, 151, 174, 255])
        }
    });
    let mut encoded = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .expect("gallery image fixture encodes");
    Arc::new(
        DecodedImageData::from_bytes(encoded, "image/png".to_owned(), None)
            .expect("gallery image fixture decodes"),
    )
}

fn gallery_model3d_source() -> Arc<Model3d> {
    let triangle = Triangle::new(
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(-0.5, -0.5, 0.0),
        Vec3::new(0.5, -0.5, 0.0),
        Vec3::new(0.0, 0.5, 0.0),
    );
    Arc::new(Model3d {
        name: "gallery-model.obj".to_owned(),
        format: ModelFormat::Obj,
        triangles: vec![triangle],
        thumbnail_triangles: vec![triangle],
        min_bounds: Vec3::new(-0.5, -0.5, 0.0),
        max_bounds: Vec3::new(0.5, 0.5, 0.0),
        center: Vec3::ZERO,
        size: Vec3::new(1.0, 1.0, 0.0),
        max_dimension: 1.0,
    })
}

impl ComponentGalleryState {
    pub fn render_ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        let colors = WorkbenchColors::from_palette(palette);
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Frame::none()
                .fill(colors.panel)
                .rounding(Rounding::same(ui_ext::PANEL_CORNER_RADIUS))
                .inner_margin(ui_ext::PANEL_INSET)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.heading("Component Gallery");
                        ui.add_space(ui_ext::SPACE_SM);
                        badge_pill(ui, "STYLE CHECK", Some(colors.accent), palette);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(secondary_button("Close (Esc)", colors.recessed, palette))
                                .clicked()
                            {
                                *close = true;
                            }
                            ui.label(
                                RichText::new("F6 switches Light / Dark")
                                    .size(BODY_FONT_SIZE)
                                    .color(colors.text_muted),
                            );
                        });
                    });
                    ui.label(
                        RichText::new("Interactive samples of the shared workbench controls")
                            .color(colors.text_secondary),
                    );
                });

            ui.add_space(ui_ext::SPACE_MD);
            render_section(ui, "Inline preview hosts", palette, |ui| {
                ui.label("Inline Image Preview • shared decoded pixels");
                ui.horizontal(|ui| {
                    self.image_previews[0].show(ui, Vec2::new(150.0, 64.0));
                    self.image_previews[1].show(ui, Vec2::new(150.0, 64.0));
                    if ui.button("Open Image Studio").clicked() {
                        self.request_open(GalleryPreviewKind::Image);
                    }
                });

                ui.label("Inline Text Preview • bounded source fragments");
                ui.horizontal(|ui| {
                    self.text_previews[0].show(ui, palette);
                    self.text_previews[1].show(ui, palette);
                    if ui.button("Open Text Studio").clicked() {
                        self.request_open(GalleryPreviewKind::Text);
                    }
                });

                ui.label("Inline Spreadsheet Preview • virtual rows and columns");
                ui.horizontal(|ui| {
                    self.spreadsheet_previews[0].show(ui, palette);
                    self.spreadsheet_previews[1].show(ui, palette);
                    if ui.button("Open Spreadsheet Studio").clicked() {
                        self.request_open(GalleryPreviewKind::Spreadsheet);
                    }
                });

                ui.label("Inline MIDI Preview • local seek and selection");
                ui.horizontal(|ui| {
                    self.midi_previews[0].show(ui, palette);
                    self.midi_previews[1].show(ui, palette);
                    if ui.button("Open MIDI Studio").clicked() {
                        self.request_open(GalleryPreviewKind::Midi);
                    }
                });

                ui.label("Inline Model3D Preview • independent camera state");
                ui.horizontal(|ui| {
                    self.model3d_previews[0].show(ui, palette);
                    self.model3d_previews[1].show(ui, palette);
                    if ui.button("Open Model3D Studio").clicked() {
                        self.request_open(GalleryPreviewKind::Model3d);
                    }
                });
            });

            ui.add_space(ui_ext::SPACE_MD);
            ui.columns(2, |columns| {
                render_section(&mut columns[0], "Buttons and modes", palette, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(action_button("Primary action", colors.accent, palette));
                        ui.add(secondary_button("Secondary", colors.recessed, palette));
                        ui.add(themed_button("Destructive", colors.danger, None, palette));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled(
                            false,
                            secondary_button("Disabled", colors.recessed, palette),
                        );
                        ui.add(secondary_button(
                            "Ordinary control",
                            colors.recessed,
                            palette,
                        ));
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (index, label) in ["Normal", "Alternate", "Expert"].iter().enumerate() {
                            if ui
                                .add(mode_tab_button(
                                    *label,
                                    self.selected_mode == index,
                                    palette,
                                ))
                                .clicked()
                            {
                                self.selected_mode = index;
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        badge_pill(ui, "Ready", Some(colors.success), palette);
                        badge_pill(ui, "Working", Some(colors.warning), palette);
                        badge_pill(ui, "Unavailable", Some(colors.recessed), palette);
                    });
                });

                render_section(&mut columns[1], "Fields and choices", palette, |ui| {
                    ui.label("Single-line field");
                    ui.add_sized(
                        Vec2::new(ui.available_width(), ui_ext::CONTROL_HEIGHT),
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Type to preview field styling"),
                    );
                    ui.label("Multiline field");
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 76.0),
                        egui::TextEdit::multiline(&mut self.notes),
                    );
                    ui.checkbox(&mut self.enabled, "Enabled setting");
                    ui.horizontal_wrapped(|ui| {
                        let mut choice = self.selected_mode == 1;
                        if ui.radio_value(&mut choice, true, "Radio choice").clicked() && choice {
                            self.selected_mode = 1;
                        }
                        ui.label("Choice controls use a shared readable baseline.");
                    });
                    ui.add(egui::Slider::new(&mut self.level, 0.0..=1.0).text("Level"));
                    ui.add(egui::Slider::new(&mut self.level, 0.0..=1.0).show_value(false));
                });

                render_section(&mut columns[0], "Bars and status", palette, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Audio preview").strong());
                        ui.add(
                            egui::ProgressBar::new(self.level)
                                .desired_width(220.0)
                                .text(format!("{:.0}%", self.level * 100.0)),
                        );
                    });
                    status_sample(ui, "Success", "Changes saved", colors.success, palette);
                    status_sample(
                        ui,
                        "Notice",
                        "Waiting for a source",
                        colors.warning,
                        palette,
                    );
                    status_sample(
                        ui,
                        "Error",
                        "Could not load preview",
                        colors.danger,
                        palette,
                    );
                });

                render_section(&mut columns[1], "Panels and toolbar", palette, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("IMAGE STUDIO").strong());
                        ui.label(RichText::new("— sample.webp").color(colors.text_muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add(secondary_button("Close", colors.recessed, palette));
                            ui.add(action_button("Save", colors.accent, palette));
                        });
                    });
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        for label in ["View", "Crop", "Markup", "Resize", "Convert"] {
                            ui.add(secondary_button(label, colors.recessed, palette));
                        }
                    });
                    egui::Frame::none()
                        .fill(colors.recessed)
                        .rounding(Rounding::same(ui_ext::CARD_CORNER_RADIUS))
                        .stroke(Stroke::new(1.0_f32, colors.border))
                        .inner_margin(ui_ext::SPACE_SM)
                        .show(ui, |ui| {
                            ui.label(RichText::new("Nested card").strong());
                            ui.label("Panel hierarchy, spacing, and edge contrast sample.");
                        });
                });
            });

            ui.add_space(ui_ext::SPACE_MD);
            ui.label(
                RichText::new("Esc closes this page. F6 previews the other appearance.")
                    .color(colors.text_muted),
            );
        });
    }
}

fn render_section(
    ui: &mut egui::Ui,
    title: &str,
    palette: &AppPalette,
    contents: impl FnOnce(&mut egui::Ui),
) {
    let colors = WorkbenchColors::from_palette(palette);
    egui::Frame::none()
        .fill(colors.panel)
        .rounding(Rounding::same(ui_ext::PANEL_CORNER_RADIUS))
        .stroke(Stroke::new(1.0_f32, colors.border))
        .inner_margin(ui_ext::PANEL_INSET)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(title);
            ui.add_space(ui_ext::SPACE_SM);
            contents(ui);
        });
    ui.add_space(ui_ext::SPACE_MD);
}

fn status_sample(
    ui: &mut egui::Ui,
    label: &str,
    message: &str,
    accent: Color32,
    palette: &AppPalette,
) {
    let colors = WorkbenchColors::from_palette(palette);
    egui::Frame::none()
        .fill(colors.recessed)
        .rounding(Rounding::same(ui_ext::CONTROL_CORNER_RADIUS))
        .stroke(Stroke::new(1.0_f32, accent))
        .inner_margin(Vec2::splat(ui_ext::SPACE_SM))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).strong().color(accent));
                ui.label(message);
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_appearance;
    use little_clip_core::config::WorkbenchAppearance;

    #[test]
    fn gallery_renders_in_light_and_dark_at_compact_and_wide_sizes() {
        for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
            for size in [egui::vec2(900.0, 700.0), egui::vec2(1760.0, 1000.0)] {
                let ctx = egui::Context::default();
                ui_ext::configure_workbench_style(&ctx);
                let palette = ui_appearance::palette(appearance);
                ui_appearance::apply_visuals(&ctx, &palette);
                let mut gallery = ComponentGalleryState::default();
                let mut close = false;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            gallery.render_ui(ui, &palette, &mut close);
                        });
                    },
                );
                assert!(
                    output.shapes.len() > 30,
                    "gallery rendered only {} shapes at {size:?}",
                    output.shapes.len()
                );
                let text_shapes: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) => Some(text),
                        _ => None,
                    })
                    .collect();
                for label in [
                    "Inline Image Preview",
                    "Inline Text Preview",
                    "Inline Spreadsheet Preview",
                    "Inline MIDI Preview",
                    "Inline Model3D Preview",
                    "Open Image Studio",
                    "Open Text Studio",
                    "Open Spreadsheet Studio",
                    "Open MIDI Studio",
                    "Open Model3D Studio",
                ] {
                    assert!(
                        text_shapes
                            .iter()
                            .any(|text| text.galley.job.text.contains(label)),
                        "{label} missing at {appearance:?} and {size:?}"
                    );
                }
                assert!(
                    text_shapes
                        .iter()
                        .filter(|text| text.galley.job.text.contains("gallery-line-"))
                        .count()
                        >= 2,
                    "both text preview hosts should render at {appearance:?} and {size:?}"
                );
                assert!(
                    text_shapes
                        .iter()
                        .filter(|text| text.galley.job.text.contains("gallery-cell-"))
                        .count()
                        >= 2,
                    "both spreadsheet hosts should render at {appearance:?} and {size:?}"
                );
                assert!(
                    output
                        .shapes
                        .iter()
                        .filter(|shape| matches!(
                            &shape.shape,
                            egui::epaint::Shape::Mesh(mesh) if !mesh.vertices.is_empty()
                        ))
                        .count()
                        >= 4,
                    "both image and Model3D hosts should paint geometry at {appearance:?} and {size:?}"
                );
                let (note_r, note_g, note_b) = palette.midi_note_color(0, 0, 1, 60, 96);
                let note_color = Color32::from_rgb(note_r, note_g, note_b);
                assert!(
                    output
                        .shapes
                        .iter()
                        .filter(|shape| matches!(
                            &shape.shape,
                            egui::epaint::Shape::Rect(rect) if rect.fill == note_color
                        ))
                        .count()
                        >= 2,
                    "both MIDI hosts should render notes at {appearance:?} and {size:?}"
                );
                assert!(!close);
            }
        }
    }
}
