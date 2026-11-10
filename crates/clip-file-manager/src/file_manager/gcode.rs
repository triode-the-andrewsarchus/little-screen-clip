//! Program-coordinate toolpaths with linked source lines and progress scrubbing.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    ui_appearance::WorkbenchColors,
};
use little_clip_core::{
    gcode::{Document, MAX_BYTES, Move},
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn read(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BYTES)?;
    if token.load(Ordering::Relaxed) {
        return Err("Toolpath opening cancelled".into());
    }
    Document::parse(&bytes)
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "G-code".into(), Icon::Code);
    match read(path, token) {
        Ok(mut document) => {
            data.detail = format!(
                "{} moves · {:.1} mm · {}",
                document.moves.len(),
                document.length_mm,
                if document.notes.len() > 1 {
                    "partial/qualified preview"
                } else {
                    "program coordinates"
                }
            );
            let step = document.moves.len().div_ceil(512).max(1);
            document.moves = document.moves.into_iter().step_by(step).collect();
            document.source.clear();
            document.notes.clear();
            data.toolpath = Some(Arc::new(document));
        }
        Err(error) => data.detail = error,
    }
    data
}
pub(super) struct Viewer {
    document: Document,
    lines: Vec<std::ops::Range<usize>>,
    progress: usize,
    source_scroll: bool,
    travel: bool,
    axes: [usize; 2],
    zoom: f32,
    pan: egui::Vec2,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = read(path, &AtomicBool::new(false))?;
        let mut offset = 0;
        let lines = document
            .source
            .split_inclusive('\n')
            .map(|line| {
                let range = offset..offset + line.trim_end_matches(['\r', '\n']).len();
                offset += line.len();
                range
            })
            .collect();
        let progress = document.moves.len();
        Ok(Self {
            document,
            lines,
            progress,
            source_scroll: true,
            travel: true,
            axes: [0, 1],
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        })
    }
    pub fn select_line(&mut self, line: usize) {
        if let Some(index) = self.document.moves.iter().position(|m| m.line >= line) {
            self.progress = index + 1;
            self.source_scroll = true;
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        StudioShell::new("Toolpath", Icon::Code, palette, close).render_header(ui, |_| {});
        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.details(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            let colors = WorkbenchColors::from_palette(palette);
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(
                    ui.available_width(),
                    (ui.available_height() - 170.0).max(40.0),
                ),
                egui::Sense::click_and_drag(),
            );
            crate::ui_ext::pan_zoom_canvas(
                ui,
                rect,
                &response,
                &mut self.zoom,
                &mut self.pan,
                32.0,
            );
            let hit = paint(
                ui,
                rect,
                &self.document,
                self.axes,
                self.progress,
                self.travel,
                self.zoom,
                self.pan,
                colors,
            );
            if response.clicked() {
                if let Some(index) = hit {
                    self.progress = index + 1;
                    self.source_scroll = true;
                }
            }
            ui.label("Program coordinates · drag to pan · scroll to zoom · click a move");
            let row_height = crate::ui_ext::selectable_text_row_height(ui, 24.0);
            let selected = self.current().map(|m| m.line);
            let mut source_area = egui::ScrollArea::vertical();
            if std::mem::take(&mut self.source_scroll) {
                source_area = source_area.vertical_scroll_offset(
                    selected.unwrap_or(1).saturating_sub(1) as f32
                        * (row_height + ui.spacing().item_spacing.y),
                );
            }
            source_area
                .id_source("toolpath-source")
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .show_rows(ui, row_height, self.lines.len(), |ui, rows| {
                    for index in rows {
                        let range = self.lines[index].clone();
                        let line = &self.document.source[range];
                        let line = &line[..line.floor_char_boundary(240.min(line.len()))];
                        if crate::ui_ext::selectable_text_row(
                            ui,
                            &format!("{}  {}", index + 1, line),
                            selected == Some(index + 1),
                            row_height,
                        )
                        .clicked()
                        {
                            self.select_line(index + 1);
                        }
                    }
                });
        });
    }
    fn current(&self) -> Option<&Move> {
        self.progress
            .checked_sub(1)
            .and_then(|index| self.document.moves.get(index))
    }
    fn details(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("{} moves", self.document.moves.len()));
        ui.label(format!("Path length {:.1} mm", self.document.length_mm));
        if ui
            .add(
                egui::Slider::new(&mut self.progress, 0..=self.document.moves.len())
                    .text("Progress"),
            )
            .changed()
        {
            self.source_scroll = true;
        }
        ui.checkbox(&mut self.travel, "Show rapid travel");
        ui.horizontal(|ui| {
            for (label, axes) in [("XY", [0, 1]), ("XZ", [0, 2]), ("YZ", [1, 2])] {
                if ui.selectable_label(self.axes == axes, label).clicked() {
                    self.axes = axes;
                    self.pan = egui::Vec2::ZERO;
                    self.zoom = 1.0;
                }
            }
        });
        if ui.button("Fit path").clicked() {
            self.pan = egui::Vec2::ZERO;
            self.zoom = 1.0;
        }
        ui.label("Accent: feed / cutting\nGreen: extrusion\nMuted: rapid travel");
        egui::ScrollArea::vertical().id_source("toolpath-details").auto_shrink([false,false]).min_scrolled_height(0.0).show(ui,|ui|{
            if let Some(movement)=self.current(){
                ui.strong(format!("Source line {}",movement.line));
                for (axis,label) in ["X","Y","Z"].iter().enumerate(){ui.label(format!("{label} {:.3} mm",movement.to[axis]));}
                if let Some(feed)=movement.feed_mm_min {ui.label(format!("Feed {feed:.1} mm/min"));}
                ui.label(if movement.rapid {"Rapid travel"} else if movement.extruding {"Extruding"} else {"Feed / cutting move"});
            }
            for note in &self.document.notes {ui.add(egui::Label::new(note).wrap(true));}
            ui.label("Arc drawing uses intervals of at most 2 degrees. Length is the sampled path, not a machine cycle estimate. G90/G91 also reset extrusion mode; M82/M83 can override it.");
        });
    }
}
fn position(
    rect: egui::Rect,
    doc: &Document,
    p: [f64; 3],
    axes: [usize; 2],
    zoom: f32,
    pan: egui::Vec2,
) -> egui::Pos2 {
    let [x, y] = axes;
    crate::ui_ext::fit_cartesian_point(
        rect,
        [
            doc.bounds[0][x],
            doc.bounds[0][y],
            doc.bounds[1][x],
            doc.bounds[1][y],
        ],
        [p[x], p[y]],
        zoom,
        pan,
    )
}
fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    doc: &Document,
    axes: [usize; 2],
    progress: usize,
    travel: bool,
    zoom: f32,
    pan: egui::Vec2,
    colors: WorkbenchColors,
) -> Option<usize> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let step = doc.moves.len().div_ceil(20_000).max(1);
    let mut nearest = 64.0;
    let mut hit = None;
    for (index, movement) in doc.moves.iter().enumerate().take(progress).step_by(step) {
        if movement.rapid && !travel {
            continue;
        }
        let from = position(rect, doc, movement.from, axes, zoom, pan);
        let to = position(rect, doc, movement.to, axes, zoom, pan);
        let color = if movement.rapid {
            colors.text_muted
        } else if movement.extruding {
            colors.success
        } else {
            colors.accent
        };
        painter.line_segment(
            [from, to],
            egui::Stroke::new(if movement.rapid { 1.0_f32 } else { 2.0_f32 }, color),
        );
        if let Some(pointer) = pointer {
            let (_, distance) = crate::ui_ext::segment_hit(pointer, from, to);
            if distance < nearest {
                nearest = distance;
                hit = Some(index);
            }
        }
    }
    if let Some(movement) = progress.checked_sub(1).and_then(|i| doc.moves.get(i)) {
        painter.circle_stroke(
            position(rect, doc, movement.to, axes, zoom, pan),
            6.0,
            egui::Stroke::new(2.0_f32, colors.focus),
        );
    }
    if step > 1 {
        painter.text(
            rect.left_top() + egui::vec2(5.0, 5.0),
            egui::Align2::LEFT_TOP,
            format!("Drawing 1 in {step} intervals"),
            egui::TextStyle::Small.resolve(ui.style()),
            colors.text_secondary,
        );
    }
    hit
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let rect = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Toolpath", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    let doc = ready.and_then(|r| r.data.toolpath.as_ref())?;
    paint(
        ui,
        rect,
        doc,
        [0, 1],
        doc.moves.len(),
        true,
        1.0,
        egui::Vec2::ZERO,
        colors,
    )
    .map(|i| doc.moves[i].line)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_intervals_select_real_source_lines_and_rename_independent_data() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/gcode/frame.gcode");
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("FRAME.GCODE"))
                == Some(little_clip_core::file_format::Format::Toolpath))
        );
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.toolpath.unwrap();
        assert!(summary.moves.len() <= 512);
        assert!(summary.source.is_empty());
        assert_eq!(summary.moves.last().unwrap().line, 15);
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select_line(12);
        assert_eq!(viewer.current().unwrap().line, 12);
        assert!(viewer.current().unwrap().extruding);
        assert!(read(&path, &AtomicBool::new(true)).is_err());
    }
}
