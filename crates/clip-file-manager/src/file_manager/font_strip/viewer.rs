//! Interactive specimens and character coverage, sharing the list strip's font reader.
use super::super::preview_job::Task;
use super::{Document, SAMPLES, describe, inspect, raster};
use crate::{icons::Icon, shell::StudioShell, ui_appearance::WorkbenchColors};
use clip_ui::buttons::ButtonRole;
use egui::{Color32, TextureHandle, Ui, vec2};
use little_clip_core::{BackgroundJobs, ImageThumbnail, palette::AppPalette};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const COLUMNS: usize = 16;
const ROWS: usize = 8;
const CELL: u32 = 48;
const BLOCKS: [(&str, u32); 6] = [
    ("Latin", 0x20),
    ("Latin supplement", 0xA0),
    ("Greek", 0x370),
    ("Cyrillic", 0x400),
    ("Symbols", 0x2600),
    ("CJK", 0x4E00),
];

#[derive(Clone, PartialEq)]
struct Request {
    text: String,
    height: u32,
    width: u32,
    face: u32,
    start: u32,
}
struct Rendered {
    sample: ImageThumbnail,
    glyphs: ImageThumbnail,
    present: Vec<bool>,
    description: String,
}
pub(in crate::file_manager) struct Viewer {
    document: Arc<Document>,
    request: Request,
    submitted: Option<Request>,
    displayed: Option<Request>,
    task: Option<Task<Result<Rendered, String>>>,
    sample: Option<TextureHandle>,
    glyphs: Option<TextureHandle>,
    present: Vec<bool>,
    description: String,
    error: Option<String>,
    block: usize,
    selected: Option<char>,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = Arc::new(inspect(path, &AtomicBool::new(false))?);
        let description = document.description.clone();
        Ok(Self {
            document,
            description,
            request: Request {
                text: SAMPLES.join("\n"),
                height: 180,
                width: 1000,
                face: 0,
                start: BLOCKS[0].1,
            },
            submitted: None,
            displayed: None,
            task: None,
            sample: None,
            glyphs: None,
            present: Vec::new(),
            error: None,
            block: 0,
            selected: None,
        })
    }
    fn poll(&mut self, ui: &Ui) {
        if self.submitted.as_ref() != Some(&self.request) {
            // Dropping obsolete work cancels its bounded rendering loop.
            self.task = None;
            let document = self.document.clone();
            let request = self.request.clone();
            self.submitted = Some(request.clone());
            self.task = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(10),
                move |token| render(&document, &request, token),
            ));
        }
        let Some(task) = &self.task else { return };
        let Some(result) = task.poll() else {
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            return;
        };
        self.task = None;
        if matches!(result, Err(little_clip_core::JobError::QueueFull)) {
            self.submitted = None;
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            return;
        }
        match result.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(rendered) => {
                self.displayed = self.submitted.clone();
                let texture = |name, image: ImageThumbnail| {
                    ui.ctx().load_texture(
                        name,
                        egui::ColorImage::from_rgba_premultiplied(
                            [image.width as usize, image.height as usize],
                            &image.premultiplied_rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                };
                self.sample = Some(texture("font-specimen", rendered.sample));
                self.glyphs = Some(texture("font-coverage", rendered.glyphs));
                self.present = rendered.present;
                self.description = rendered.description;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        let mut shell = StudioShell::new("Font specimen", Icon::Text, palette, close);
        shell.render_header(ui, |_| {});
        let colors = WorkbenchColors::from_palette(palette);
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.specimen_controls(ui);
            self.block_controls(ui, palette);
            self.request.width = (ui.available_width().round() as u32).clamp(160, 4096);
            self.poll(ui);
            if let Some(error) = &self.error {
                ui.colored_label(colors.danger, error);
            }
            let specimen_size = vec2(ui.available_width(), self.request.height as f32);
            if let Some(texture) = &self.sample {
                ui.add(egui::Image::new(texture).fit_to_exact_size(specimen_size).tint(colors.text_primary));
            } else {
                ui.allocate_ui(specimen_size, |ui| { ui.spinner(); });
            }
            ui.separator();
            ui.label("Character coverage · click a glyph to select it");
            if self.displayed.as_ref() == Some(&self.request) {
                self.coverage(ui, colors.text_primary, colors.text_secondary, colors.accent);
            } else {
                let width = ui.available_width().min((COLUMNS as u32 * CELL) as f32);
                ui.allocate_ui(vec2(width, width * ROWS as f32 / COLUMNS as f32), |ui| { ui.spinner(); });
            }
            if let Some(character) = self.selected {
                ui.horizontal(|ui| {
                    ui.label(format!("Selected: {character} · U+{:04X}", character as u32));
                    if ui.add(ButtonRole::Secondary.button("Copy character", palette)).clicked() {
                        ui.output_mut(|output| output.copied_text = character.to_string());
                    }
                });
            }
            ui.label("Dim cells are missing from this font. Complex scripts are shown as individual glyphs.");
            ui.separator();
            ui.label(&self.description);
        });
    }
    fn specimen_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Specimen size");
            ui.add(egui::Slider::new(&mut self.request.height, 90..=360).suffix(" px"));
            let faces = ttf_parser::fonts_in_collection(&self.document.bytes).unwrap_or(1);
            if faces > 1 {
                ui.label("Collection face");
                let mut selected = self.request.face + 1;
                ui.add(egui::DragValue::new(&mut selected).clamp_range(1..=faces));
                self.request.face = selected - 1;
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut self.request.text)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .char_limit(512),
        );
    }
    fn block_controls(&mut self, ui: &mut Ui, palette: &AppPalette) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Character range");
            egui::ComboBox::from_id_source("font-block")
                .selected_text(BLOCKS[self.block].0)
                .show_ui(ui, |ui| {
                    for (index, (name, start)) in BLOCKS.iter().enumerate() {
                        if ui.selectable_value(&mut self.block, index, *name).clicked() {
                            self.request.start = *start;
                        }
                    }
                });
            if ui
                .add_enabled(
                    self.request.start >= 128,
                    ButtonRole::Secondary.button("Previous characters", palette),
                )
                .clicked()
            {
                self.request.start -= 128;
            }
            if ui
                .add_enabled(
                    self.request.start <= 0x10FF7F,
                    ButtonRole::Secondary.button("Next characters", palette),
                )
                .clicked()
            {
                self.request.start = (self.request.start + 128).min(0x10FF80);
            }
            ui.label(format!(
                "U+{:04X}–U+{:04X}",
                self.request.start,
                self.request.start + 127
            ));
        });
    }
    fn coverage(&mut self, ui: &mut Ui, text: Color32, dim: Color32, accent: Color32) {
        let Some(texture) = &self.glyphs else { return };
        let width = ui.available_width().min((COLUMNS as u32 * CELL) as f32);
        let (response, painter) = ui.allocate_painter(
            vec2(width, width * ROWS as f32 / COLUMNS as f32),
            egui::Sense::click(),
        );
        let cell = width / COLUMNS as f32;
        painter.image(
            texture.id(),
            response.rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            text,
        );
        for index in 0..COLUMNS * ROWS {
            let rect = egui::Rect::from_min_size(
                response.rect.min
                    + vec2(
                        (index % COLUMNS) as f32 * cell,
                        (index / COLUMNS) as f32 * cell,
                    ),
                vec2(cell, cell),
            );
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(0.5_f32, dim));
            if self.present.get(index) == Some(&false) {
                painter.line_segment(
                    [rect.left_top(), rect.right_bottom()],
                    egui::Stroke::new(0.5_f32, dim),
                );
            }
            if self
                .selected
                .is_some_and(|ch| ch as u32 == self.request.start + index as u32)
            {
                painter.rect_stroke(rect.shrink(1.0), 0.0, egui::Stroke::new(2.0_f32, accent));
            }
            if response.hover_pos().is_some_and(|pos| rect.contains(pos)) {
                let value = self.request.start + index as u32;
                response.clone().on_hover_text(format!(
                    "U+{value:04X}{}",
                    if self.present.get(index) == Some(&true) {
                        ""
                    } else {
                        " · missing"
                    }
                ));
                if response.clicked() && self.present.get(index) == Some(&true) {
                    self.selected = char::from_u32(value);
                }
            }
        }
    }
}

fn render(document: &Document, request: &Request, token: &AtomicBool) -> Result<Rendered, String> {
    let face = ttf_parser::Face::parse(&document.bytes, request.face)
        .map_err(|e| format!("Cannot read font face: {e:?}"))?;
    let (_, description) = describe(
        &face,
        ttf_parser::fonts_in_collection(&document.bytes).unwrap_or(1),
        request.face,
    );
    let mut sample =
        tiny_skia::Pixmap::new(request.width, request.height).ok_or("Cannot allocate specimen")?;
    let lines: Vec<_> = request.text.lines().take(8).collect();
    let height = request.height as f32 / lines.len().max(1) as f32;
    for (index, line) in lines.iter().enumerate() {
        raster::draw(
            &mut sample,
            &face,
            line,
            index as f32 * height,
            height,
            [255; 3],
            token,
        )
        .ok_or("Rendering cancelled")?;
    }
    let mut glyphs = tiny_skia::Pixmap::new(COLUMNS as u32 * CELL, ROWS as u32 * CELL)
        .ok_or("Cannot allocate character grid")?;
    let mut present = Vec::with_capacity(COLUMNS * ROWS);
    for index in 0..COLUMNS * ROWS {
        if token.load(Ordering::Relaxed) {
            return Err("Rendering cancelled".into());
        }
        let Some(ch) = char::from_u32(request.start + index as u32) else {
            present.push(false);
            continue;
        };
        present.push(face.glyph_index(ch).is_some());
        let mut cell = tiny_skia::Pixmap::new(CELL, CELL).ok_or("Cannot allocate character")?;
        raster::draw(
            &mut cell,
            &face,
            &ch.to_string(),
            0.0,
            CELL as f32,
            [255; 3],
            token,
        )
        .ok_or("Rendering cancelled")?;
        glyphs.draw_pixmap(
            (index % COLUMNS) as i32 * CELL as i32,
            (index / COLUMNS) as i32 * CELL as i32,
            cell.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
    }
    let thumbnail = |pixmap: tiny_skia::Pixmap| ImageThumbnail {
        width: pixmap.width(),
        height: pixmap.height(),
        premultiplied_rgba: pixmap.take(),
    };
    Ok(Rendered {
        sample: thumbnail(sample),
        glyphs: thumbnail(glyphs),
        present,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("specimen.ttf");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../little-clip-render/assets/fonts")
                .join(name),
            &path,
        )
        .unwrap();
        (folder, path)
    }

    #[test]
    fn true_type_specimens_share_strip_reader_and_render_requested_coverage() {
        let (_folder, path) = fixture("NotoEmoji-Regular.ttf");
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.request.text = "0123".into();
        viewer.request.start = 0x2600;
        let source = crate::file_source::FileSource::capture(&path).unwrap();
        let token = AtomicBool::new(false);
        let strip = super::super::load(&path, &token);
        assert!(strip.font.is_some());
        assert!(strip.image.is_some());
        let first = render(&viewer.document, &viewer.request, &token).unwrap();
        assert_eq!((first.sample.width, first.sample.height), (1000, 180));
        assert_eq!(first.present.len(), COLUMNS * ROWS);
        assert!(first.present.iter().any(|present| *present));
        assert!(first.glyphs.premultiplied_rgba.iter().any(|byte| *byte > 0));
        viewer.request.text = "4567".into();
        let second = render(&viewer.document, &viewer.request, &token).unwrap();
        assert_ne!(
            first.sample.premultiplied_rgba,
            second.sample.premultiplied_rgba
        );
        viewer.request.start = 0x10FF80;
        let missing = render(&viewer.document, &viewer.request, &token).unwrap();
        assert!(missing.present.iter().all(|present| !present));
        viewer.request.start = 0xD800;
        let surrogates = render(&viewer.document, &viewer.request, &token).unwrap();
        assert!(surrogates.present.iter().all(|present| !present));
        source.check().unwrap();
        assert!(render(&viewer.document, &viewer.request, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn collection_faces_share_data_and_can_be_selected() {
        for (name, extension) in [
            ("NotoEmoji-Regular.ttf", "ttc"),
            ("NotoSansCJK-JP-Regular.ttf", "otc"),
        ] {
            let (folder, path) = fixture(name);
            let mut font = std::fs::read(path).unwrap();
            let tables = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
            for table in 0..tables {
                let offset = 12 + table * 16 + 8;
                let value = u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());
                font[offset..offset + 4].copy_from_slice(&(value + 20).to_be_bytes());
            }
            let mut collection = b"ttcf".to_vec();
            for value in [0x00010000_u32, 2, 20, 20] {
                collection.extend_from_slice(&value.to_be_bytes());
            }
            collection.extend(font);
            let path = folder.path().join(format!("collection.{extension}"));
            std::fs::write(&path, collection).unwrap();
            let mut viewer = Viewer::load(&path).unwrap();
            assert!(viewer.description.contains("2 faces"));
            viewer.request.face = 1;
            viewer.request.text = "0123".into();
            assert!(render(&viewer.document, &viewer.request, &AtomicBool::new(false)).is_ok());
            viewer.request.face = 2;
            assert!(render(&viewer.document, &viewer.request, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn compressed_web_font_specimens_reuse_bounded_decode_and_strip_rendering() {
        for extension in ["woff", "woff2"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/fonts/latin.{extension}"));
            let viewer = Viewer::load(&path).unwrap();
            assert!(viewer.description.contains("LittleClipFixture"));
            let token = AtomicBool::new(false);
            let rendered = render(&viewer.document, &viewer.request, &token).unwrap();
            assert!(rendered.present[33]); // Latin A
            assert!(
                rendered
                    .sample
                    .premultiplied_rgba
                    .iter()
                    .any(|byte| *byte > 0)
            );
            let preview = super::super::load(&path, &token);
            assert!(preview.font.is_some());
            assert!(preview.image.is_some());
            let folder = tempfile::tempdir().unwrap();
            let invalid = folder.path().join(format!("oversized.{extension}"));
            let mut bytes = std::fs::read(path).unwrap();
            bytes[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
            std::fs::write(&invalid, bytes).unwrap();
            assert!(Viewer::load(&invalid).is_err());
        }
    }

    #[test]
    fn corrupt_font_and_invalid_collection_face_report_errors() {
        let (folder, path) = fixture("NotoEmoji-Regular.ttf");
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.request.face = u32::MAX;
        assert!(render(&viewer.document, &viewer.request, &AtomicBool::new(false)).is_err());
        let invalid = folder.path().join("corrupt.ttf");
        std::fs::write(&invalid, b"not a font").unwrap();
        assert!(Viewer::load(&invalid).is_err());
    }
}
