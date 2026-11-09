//! Chapter navigation and reflowed reading, sharing one bounded EPUB model with rows.
use super::{media::PreviewData, preview_job::Task};
use crate::{icons::Icon, shell::StudioShell};
use little_clip_core::{
    BackgroundJobs,
    ebook::{Block, Book},
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) fn known(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
}
fn read(path: &Path, token: &AtomicBool) -> Result<Book, String> {
    let source = crate::file_source::FileSource::capture(path)?;
    let bytes = crate::file_source::read_file_bytes(path, 64 * 1024 * 1024, true, true)
        .map_err(|e| e.to_string())?;
    if token.load(Ordering::Relaxed) {
        return Err("Book opening cancelled".into());
    }
    let book = Book::parse(bytes)?;
    source.check()?;
    Ok(book)
}
pub(super) struct Summary {
    pub chapters: Vec<String>,
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "EPUB".into(), Icon::Text);
    match read(path, token) {
        Ok(book) => {
            data.detail = format!(
                "{} · {} · {} chapters",
                book.title,
                book.authors.join(", "),
                book.chapters.len()
            );
            data.book = Some(Arc::new(Summary {
                chapters: book.chapters.iter().map(|c| c.title.clone()).collect(),
            }));
            if let Some(cover) = book.cover.as_ref().and_then(|p| book.image(p).ok()) {
                data.image = thumbnail(&cover, 160)
                    .ok()
                    .map(little_clip_core::ImageThumbnail::from_rgba);
            }
        }
        Err(e) => data.detail = e,
    }
    data
}
fn thumbnail(bytes: &[u8], edge: u32) -> Result<image::RgbaImage, String> {
    let mut reader =
        little_clip_core::item::image_reader(bytes, None).map_err(|e| e.to_string())?;
    let mut limits = little_clip_core::item::raster_preview_limits();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map(|img| {
            if img.width() > edge || img.height() > edge {
                img.thumbnail(edge, edge).to_rgba8()
            } else {
                img.to_rgba8()
            }
        })
        .map_err(|e| e.to_string())
}
enum Element {
    Heading(u8, String),
    Text(String),
    Image(image::RgbaImage, String),
}
fn prepare(book: &Book, index: usize, token: &AtomicBool) -> Result<Vec<Element>, String> {
    let mut output = Vec::new();
    let mut image_bytes = 0usize;
    for block in book.chapter(index)? {
        if token.load(Ordering::Relaxed) {
            return Err("Chapter loading cancelled".into());
        }
        output.push(match block {
            Block::Heading(level, text) => Element::Heading(level, text),
            Block::Text(text) => Element::Text(text),
            Block::Image(path, alt) => {
                if image_bytes >= 32 * 1024 * 1024 {
                    Element::Text(format!("Image omitted: {alt}"))
                } else {
                    match book.image(&path).and_then(|bytes| thumbnail(&bytes, 1200)) {
                        Ok(image) => {
                            image_bytes += image.len();
                            Element::Image(image, alt)
                        }
                        Err(error) => Element::Text(format!("Image unavailable: {alt} ({error})")),
                    }
                }
            }
        });
    }
    Ok(output)
}
enum Display {
    Heading(u8, String),
    Text(String),
    Image(egui::TextureHandle, String),
}
pub(super) struct Viewer {
    book: Arc<Book>,
    selected: usize,
    loaded: bool,
    task: Option<Task<Result<Vec<Element>, String>>>,
    content: Vec<Display>,
    error: Option<String>,
    font_size: f32,
    scroll_top: bool,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        Ok(Self {
            book: Arc::new(read(path, &AtomicBool::new(false))?),
            selected: 0,
            loaded: false,
            task: None,
            content: Vec::new(),
            error: None,
            font_size: 19.0,
            scroll_top: true,
        })
    }
    pub fn select(&mut self, index: usize) {
        if index < self.book.chapters.len() && index != self.selected {
            self.selected = index;
            self.loaded = false;
            self.task = None;
            self.content.clear();
            self.error = None;
            self.scroll_top = true;
        }
    }
    fn poll(&mut self, ui: &egui::Ui) {
        if self.loaded {
            return;
        }
        if self.task.is_none() {
            let book = self.book.clone();
            let index = self.selected;
            self.task = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(15),
                move |token| prepare(&book, index, token),
            ));
        }
        let result = self.task.as_ref().and_then(Task::poll);
        ui.ctx().request_repaint_after(Duration::from_millis(40));
        let Some(result) = result else { return };
        self.task = None;
        if matches!(result, Err(little_clip_core::JobError::QueueFull)) {
            return;
        }
        self.loaded = true;
        match result.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(elements) => {
                self.content = elements
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| match e {
                        Element::Heading(level, text) => Display::Heading(level, text),
                        Element::Text(text) => Display::Text(text),
                        Element::Image(image, alt) => Display::Image(
                            ui.ctx().load_texture(
                                format!("epub-{}-{i}", self.selected),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [image.width() as usize, image.height() as usize],
                                    &image,
                                ),
                                egui::TextureOptions::LINEAR,
                            ),
                            alt,
                        ),
                    })
                    .collect()
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        let mut shell = StudioShell::new("Book", Icon::Text, palette, close);
        shell.render_header(ui, |_| {});
        self.poll(ui);
        ui.label(format!(
            "{} · {}",
            self.book.title,
            self.book.authors.join(", ")
        ));
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Chapter {} / {}",
                self.selected + 1,
                self.book.chapters.len()
            ));
            egui::ComboBox::from_id_source("book-chapters")
                .selected_text(&self.book.chapters[self.selected].title)
                .width(260.0)
                .show_ui(ui, |ui| {
                    let mut selected = self.selected;
                    for (index, chapter) in self.book.chapters.iter().enumerate() {
                        ui.selectable_value(&mut selected, index, &chapter.title);
                    }
                    self.select(selected);
                });
            if ui
                .add_enabled(self.selected > 0, egui::Button::new("Previous chapter"))
                .clicked()
            {
                self.select(self.selected - 1);
            }
            if ui
                .add_enabled(
                    self.selected + 1 < self.book.chapters.len(),
                    egui::Button::new("Next chapter"),
                )
                .clicked()
            {
                self.select(self.selected + 1);
            }
            ui.add(egui::Slider::new(&mut self.font_size, 14.0..=30.0).text("Text size"));
        });
        if let Some(error) = &self.error {
            ui.colored_label(
                crate::ui_appearance::WorkbenchColors::from_palette(palette).danger,
                error,
            );
        }
        let mut scroll = egui::ScrollArea::vertical()
            .id_source("book-reading")
            .auto_shrink([false, false]);
        if self.scroll_top {
            scroll = scroll.vertical_scroll_offset(0.0);
            self.scroll_top = false;
        }
        scroll.show(ui, |ui| {
            ui.set_max_width(900.0_f32.min(ui.available_width()));
            if self.task.is_some() {
                ui.spinner();
            }
            for item in &self.content {
                match item {
                    Display::Heading(level, text) => {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(text)
                                .size(self.font_size + (7 - level) as f32 * 2.0)
                                .strong(),
                        );
                    }
                    Display::Text(text) => {
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).size(self.font_size))
                                .wrap(true),
                        );
                        ui.add_space(8.0);
                    }
                    Display::Image(texture, alt) => {
                        ui.add(
                            egui::Image::new(texture)
                                .max_width(ui.available_width())
                                .maintain_aspect_ratio(true),
                        )
                        .on_hover_text(alt);
                    }
                }
            }
        });
    }
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &super::filesystem::Entry,
    columns: super::ui::ListColumns,
    ready: Option<&super::shelf::preview::Ready>,
    colors: crate::ui_appearance::WorkbenchColors,
) -> Option<usize> {
    let mut strip = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("EPUB chapters", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(strip, 2.0, colors.recessed);
    if let Some(texture) = ready.and_then(|r| r.texture.as_ref()) {
        let natural = texture.size_vec2();
        let size = natural * (strip.height() / natural.y).min(80.0 / natural.x);
        let image = egui::Rect::from_min_size(
            egui::pos2(strip.left(), strip.center().y - size.y * 0.5),
            size,
        );
        ui.painter().image(
            texture.id(),
            image,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        strip.min.x += size.x + 8.0;
    }
    let summary = ready.and_then(|r| r.data.book.as_ref())?;
    let count = summary.chapters.len().min(128);
    let width = strip.width() / count as f32;
    let mut hovered = None;
    for i in 0..count {
        let index = i * summary.chapters.len() / count;
        let cell = egui::Rect::from_min_size(
            egui::pos2(strip.left() + i as f32 * width, strip.top()),
            egui::vec2((width - 2.0).max(1.0), strip.height()),
        );
        ui.painter()
            .rect_filled(cell, 2.0, colors.accent.gamma_multiply(0.25));
        if width > 26.0 {
            let label = if width > 90.0 {
                format!(
                    "{} · {}",
                    index + 1,
                    summary.chapters[index].chars().take(40).collect::<String>()
                )
            } else {
                (index + 1).to_string()
            };
            ui.painter().with_clip_rect(cell.shrink(3.0)).text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                colors.text_primary,
            );
        }
        if super::ui::preview_hovered(ui, cell) {
            hovered = Some(index);
            egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new(("book-chapter", index)), |ui| {
                ui.label(&summary.chapters[index]);
            });
        }
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/ebooks/journey.epub")
    }
    #[test]
    fn chapter_selection_retires_previous_work_and_prepares_local_images() {
        let mut viewer = Viewer::load(&fixture()).unwrap();
        let first = prepare(&viewer.book, 0, &AtomicBool::new(false)).unwrap();
        assert!(
            first
                .iter()
                .any(|e| matches!(e, Element::Image(image, _) if image.width() == 320))
        );
        viewer.loaded = true;
        viewer.content.push(Display::Text("old".into()));
        viewer.select(1);
        assert_eq!(viewer.selected, 1);
        assert!(!viewer.loaded);
        assert!(viewer.content.is_empty());
        let second = prepare(&viewer.book, 1, &AtomicBool::new(false)).unwrap();
        assert!(
            second
                .iter()
                .any(|e| matches!(e, Element::Text(text) if text.contains("harbor")))
        );
        assert!(prepare(&viewer.book, 0, &AtomicBool::new(true)).is_err());
        viewer.select(usize::MAX);
        assert_eq!(viewer.selected, 1);
    }
    #[test]
    fn list_summary_shares_order_and_cover_with_the_reader() {
        let preview = load(&fixture(), &AtomicBool::new(false));
        assert_eq!(preview.book.unwrap().chapters, ["Departure", "Arrival"]);
        assert!(preview.detail.contains("Example Author"));
        assert!(preview.image.is_some());
        assert!(known(Path::new("BOOK.EPUB")));
    }
}
