use crate::audio_preview::InlineAudioWaveform;
use crate::image_preview::{DecodedImageData, InlineImagePreview};
use crate::midi_view::InlineMidiPreview;
use crate::model3d_view::InlineModel3dPreview;
use crate::pdf_view::{self, InlinePdfPagePreview, PdfPageRasterResource};
use crate::spreadsheet_view::InlineSpreadsheetPreview;
use crate::text_preview::{InlineTextPreview, TextPreviewData};
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::BODY_FONT_SIZE;
use crate::ui_ext::{
    self, action_button, badge_pill, mode_tab_button, secondary_button, themed_button,
};
use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};
use little_clip_core::audio::{FullAudioAnalysis, PeakBlock};
use little_clip_core::palette::AppPalette;
use little_clip_core::presentation::midi::{MidiData, MidiNote};
use little_clip_core::presentation::model3d::{Model3d, ModelFormat, Triangle, Vec3};
use little_clip_core::presentation::spreadsheet::{SpreadsheetData, SpreadsheetSheet};
use std::path::PathBuf;
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
    audio_source: Arc<FullAudioAnalysis>,
    audio_source_path: PathBuf,
    audio_file_guard: Option<tempfile::TempPath>,
    pdf_source_path: PathBuf,
    pdf_raster: Arc<PdfPageRasterResource>,
    pending_open: Option<GalleryPreviewKind>,
    image_previews: [InlineImagePreview; 2],
    text_previews: [InlineTextPreview; 2],
    spreadsheet_previews: [InlineSpreadsheetPreview; 2],
    midi_previews: [InlineMidiPreview; 2],
    model3d_previews: [InlineModel3dPreview; 2],
    audio_preview: InlineAudioWaveform,
    pdf_preview: InlinePdfPagePreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GalleryPreviewKind {
    Image,
    Text,
    Spreadsheet,
    Midi,
    Model3d,
    Audio,
    Pdf,
}

pub(crate) enum GalleryOpenRequest {
    Image(Arc<DecodedImageData>),
    Text(Arc<TextPreviewData>),
    Spreadsheet(Arc<SpreadsheetData>),
    Midi(Arc<MidiData>),
    Model3d(Arc<Model3d>),
    Audio(Arc<FullAudioAnalysis>, PathBuf, tempfile::TempPath),
    Pdf(PathBuf, Arc<PdfPageRasterResource>),
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
        let (audio_source, audio_source_path, audio_file_guard) = gallery_audio_source();
        let (pdf_source_path, pdf_raster) =
            pdf_view::gallery_pdf_page_source().expect("gallery PDF fixture encodes");
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
            audio_source: Arc::clone(&audio_source),
            audio_source_path,
            audio_file_guard: Some(audio_file_guard),
            pdf_source_path,
            pdf_raster: Arc::clone(&pdf_raster),
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
            audio_preview: InlineAudioWaveform::new(audio_source),
            pdf_preview: InlinePdfPagePreview::new(pdf_raster),
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
            GalleryPreviewKind::Audio => Some(GalleryOpenRequest::Audio(
                Arc::clone(&self.audio_source),
                self.audio_source_path.clone(),
                self.audio_file_guard.take()?,
            )),
            GalleryPreviewKind::Pdf => Some(GalleryOpenRequest::Pdf(
                self.pdf_source_path.clone(),
                Arc::clone(&self.pdf_raster),
            )),
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

fn gallery_audio_source() -> (Arc<FullAudioAnalysis>, PathBuf, tempfile::TempPath) {
    use std::io::Write;

    let sample_rate = 44_100_u32;
    let samples: Vec<f32> = (0..sample_rate)
        .map(|frame| {
            (std::f32::consts::TAU * 440.0 * frame as f32 / sample_rate as f32).sin() * 0.45
        })
        .collect();
    let mut file = tempfile::Builder::new()
        .prefix("little-clip-gallery-")
        .suffix(".wav")
        .tempfile()
        .expect("gallery audio fixture file creates");
    let data_len = u32::try_from(samples.len() * 2).expect("one-second fixture fits a WAV chunk");
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36_u32 + data_len).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();
    for sample in &samples {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        file.write_all(&pcm.to_le_bytes()).unwrap();
    }
    file.flush().unwrap();
    let path = file.path().to_owned();
    let file_guard = file.into_temp_path();

    let peaks = samples
        .chunks(64)
        .map(|chunk| PeakBlock {
            min: chunk.iter().copied().fold(f32::INFINITY, f32::min),
            max: chunk.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            rgb: [72, 164, 202],
        })
        .collect();
    let overview = samples
        .chunks(344)
        .map(|chunk| {
            chunk
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        })
        .collect();
    let source = Arc::new(FullAudioAnalysis {
        channels: 1,
        sample_rate,
        duration_ms: 1000,
        total_frames: samples.len(),
        ch1_samples: Arc::from(samples),
        ch2_samples: None,
        overview_peaks1: overview,
        overview_peaks2: None,
        peaks_lvl0_ch1: peaks,
        peaks_lvl0_ch2: None,
        peaks_lvl1_ch1: Vec::new(),
        peaks_lvl1_ch2: None,
        peaks_lvl2_ch1: Vec::new(),
        peaks_lvl2_ch2: None,
        spectrogram_width: 0,
        spectrogram_height: 0,
        spectrogram_rgba: Vec::new(),
        note_strip_width: 0,
        note_strip_height: 0,
        note_strip_rgba: Vec::new(),
        note_strip_info: Vec::new(),
        spectral_colors: Vec::new(),
        transients: Vec::new(),
        is_transients_ready: false,
        is_spectrogram_ready: false,
        ultrasonic_roi: Vec::new(),
        ultrasonic_peak_times: Vec::new(),
        is_truncated: false,
        truncation_warning: None,
    });
    (source, path, file_guard)
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

                ui.label("Inline Audio Waveform • completed shared analysis");
                ui.horizontal(|ui| {
                    ui.allocate_ui(Vec2::new(260.0, 48.0), |ui| {
                        self.audio_preview.render(ui, palette);
                    });
                    if ui.button("Open Audio Studio").clicked() {
                        self.request_open(GalleryPreviewKind::Audio);
                    }
                });

                ui.label("Inline PDF Page • shared cached raster");
                ui.horizontal(|ui| {
                    let ctx = ui.ctx().clone();
                    self.pdf_preview.show(ui, &ctx, Vec2::new(96.0, 128.0));
                    if ui.button("Open PDF Editor").clicked() {
                        self.request_open(GalleryPreviewKind::Pdf);
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
    fn gallery_renders_in_light_and_dark_at_7_by_4_and_ultrawide_sizes() {
        for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
            for size in [egui::vec2(2240.0, 1280.0), egui::vec2(3440.0, 1440.0)] {
                let ctx = egui::Context::default();
                ui_ext::configure_workbench_style(&ctx);
                let palette = ui_appearance::palette(appearance);
                ui_appearance::apply_visuals(&ctx, &palette);
                let mut gallery = ComponentGalleryState::default();
                let mut close = false;
                let mut outputs = vec![ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            gallery.render_ui(ui, &palette, &mut close);
                        });
                    },
                )];
                for _ in 0..12 {
                    let has_pdf_action = outputs.last().is_some_and(|output| {
                        output.shapes.iter().any(|shape| {
                            matches!(
                                &shape.shape,
                                egui::epaint::Shape::Text(text)
                                    if text.galley.job.text.contains("Open PDF Editor")
                            )
                        })
                    });
                    if has_pdf_action {
                        break;
                    }
                    outputs.push(ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            events: vec![
                                egui::Event::PointerMoved(egui::pos2(size.x * 0.5, size.y * 0.75)),
                                egui::Event::MouseWheel {
                                    unit: egui::MouseWheelUnit::Point,
                                    delta: egui::vec2(0.0, -500.0),
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                gallery.render_ui(ui, &palette, &mut close);
                            });
                        },
                    ));
                }
                let shape_count: usize = outputs.iter().map(|output| output.shapes.len()).sum();
                assert!(
                    shape_count > 30,
                    "gallery rendered only {} shapes at {size:?}",
                    shape_count
                );
                let text_shapes: Vec<_> = outputs
                    .iter()
                    .flat_map(|output| output.shapes.iter())
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
                    "Inline Audio Waveform",
                    "Inline PDF Page",
                    "Open Image Studio",
                    "Open Text Studio",
                    "Open Spreadsheet Studio",
                    "Open MIDI Studio",
                    "Open Model3D Studio",
                    "Open Audio Studio",
                    "Open PDF Editor",
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
                    outputs
                        .iter()
                        .flat_map(|output| output.shapes.iter())
                        .filter(|shape| matches!(
                            &shape.shape,
                            egui::epaint::Shape::LineSegment { .. }
                        ))
                        .count()
                        >= 128,
                    "audio waveform should render at {appearance:?} and {size:?}"
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
                    outputs
                        .iter()
                        .flat_map(|output| output.shapes.iter())
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
                    outputs
                        .iter()
                        .flat_map(|output| output.shapes.iter())
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

    #[test]
    fn audio_gallery_source_is_shared_with_inline_and_completed_studio() {
        let mut gallery = ComponentGalleryState::default();
        assert!(Arc::ptr_eq(
            gallery.audio_preview.source_analysis(),
            &gallery.audio_source
        ));
        gallery.request_open(GalleryPreviewKind::Audio);
        let GalleryOpenRequest::Audio(source, path, file_guard) =
            gallery.take_pending_open().unwrap()
        else {
            panic!("audio request should return the retained completed analysis");
        };
        assert!(Arc::ptr_eq(&source, &gallery.audio_source));
        assert!(path.is_file());
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"RIFF");
        assert!(
            rodio::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()))
                .unwrap()
                .next()
                .is_some()
        );
        assert!(source.ch1_samples[11] != 0.0);
        assert!(source.peaks_lvl0_ch1.iter().any(|peak| peak.max > 0.0));

        let state = crate::audio_preview::AudioPreviewState::from_completed_source_with_file_guard(
            path.clone(),
            "Gallery audio".to_owned(),
            None,
            Arc::clone(&source),
            file_guard,
        );
        assert!(Arc::ptr_eq(
            state.completed_source_analysis().unwrap(),
            &gallery.audio_source
        ));
        assert!(state.analysis_rx.is_none());
        assert!(!state.is_loading_analysis);
        assert!(state.path.is_file());

        drop(gallery);
        assert!(state.path.is_file());
        drop(state);
        assert!(!path.exists());
    }

    #[test]
    fn repeated_gallery_renders_keep_audio_source_and_pdf_texture_state() {
        let ctx = egui::Context::default();
        ui_ext::configure_workbench_style(&ctx);
        let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
        let mut gallery = ComponentGalleryState::default();
        let mut close = false;
        let clipped_size = egui::vec2(1280.0, 720.0);
        let visible_size = egui::vec2(2240.0, 1280.0);
        let audio_source = Arc::clone(&gallery.audio_source);
        let pdf_raster = Arc::clone(&gallery.pdf_raster);

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, clipped_size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    gallery.render_ui(ui, &palette, &mut close);
                });
            },
        );
        assert_eq!(
            gallery.pdf_preview.decode_start_count_for_test(),
            0,
            "the clipped PDF page must not be decoded"
        );

        let mut pdf_texture_id = None;
        for frame in 0..200 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, visible_size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        gallery.render_ui(ui, &palette, &mut close);
                    });
                },
            );

            assert!(Arc::ptr_eq(
                gallery.audio_preview.source_analysis(),
                &audio_source
            ));
            assert!(gallery.audio_file_guard.is_some());

            assert!(
                gallery.pdf_preview.decode_start_count_for_test() <= 1,
                "one inline host must admit at most one PDF decode"
            );

            pdf_texture_id = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Mesh(mesh) => {
                    let bounds = mesh.calc_bounds();
                    ((bounds.width() - 96.0).abs() < 1.0 && (bounds.height() - 128.0).abs() < 1.0)
                        .then_some(mesh.texture_id)
                }
                _ => None,
            });
            if pdf_texture_id.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert!(
                frame < 199,
                "gallery PDF page should decode and upload (starts: {})",
                gallery.pdf_preview.decode_start_count_for_test()
            );
        }

        let pdf_texture_id = pdf_texture_id.expect("gallery PDF texture should be resident");
        assert_eq!(gallery.pdf_preview.decode_start_count_for_test(), 1);
        let retained_raster_refs = Arc::strong_count(&pdf_raster);
        assert_eq!(
            retained_raster_refs, 3,
            "gallery and inline widget retain one raster Arc each"
        );
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, visible_size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        gallery.render_ui(ui, &palette, &mut close);
                    });
                },
            );
            assert_eq!(
                output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Mesh(mesh) => {
                        let bounds = mesh.calc_bounds();
                        ((bounds.width() - 96.0).abs() < 1.0
                            && (bounds.height() - 128.0).abs() < 1.0)
                            .then_some(mesh.texture_id)
                    }
                    _ => None,
                }),
                Some(pdf_texture_id),
                "a later gallery frame should reuse the uploaded PDF texture"
            );
            assert_eq!(Arc::strong_count(&pdf_raster), retained_raster_refs);
            assert_eq!(gallery.pdf_preview.decode_start_count_for_test(), 1);
            assert!(Arc::ptr_eq(
                gallery.audio_preview.source_analysis(),
                &audio_source
            ));
            assert_eq!(Arc::strong_count(&audio_source), 3);
        }
    }
}
