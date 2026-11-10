//! Cue navigation and timelines shared by text studios and file rows.
use crate::{ui_appearance::WorkbenchColors, ui_ext::selectable_text_row};
use egui::{Rect, Ui, pos2, vec2};
use little_clip_core::{
    palette::AppPalette,
    subtitles::{Document, Timeline, format_time},
};
use std::sync::Arc;

pub(crate) struct Viewer {
    pub(crate) document: Arc<Document>,
    selected: usize,
    scroll_to_selected: bool,
    id: egui::Id,
}
impl Viewer {
    pub fn new(document: Arc<Document>) -> Self {
        Self {
            document,
            selected: 0,
            scroll_to_selected: true,
            id: egui::Id::new(uuid::Uuid::new_v4()),
        }
    }
    pub fn select(&mut self, cue: usize) {
        self.selected = cue.min(self.document.cues.len().saturating_sub(1));
        self.scroll_to_selected = true;
    }
    pub fn source_line(&self) -> Option<usize> {
        self.document
            .cues
            .get(self.selected)
            .map(|cue| cue.source_line)
    }
    pub fn show(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        ui.label(details(self.document.cues.len(), &self.document.timeline));
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(colors.accent, "Cue coverage");
            ui.colored_label(colors.danger, "Simultaneous cues");
        });
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 48.0), egui::Sense::click());
        let selected = self
            .document
            .cues
            .get(self.selected)
            .map(|cue| (cue.start_ms, cue.end_ms));
        if let Some(cue) = paint(
            ui,
            rect,
            &self.document.timeline,
            selected,
            response.hover_pos(),
            colors,
        ) && response.clicked()
        {
            self.select(cue);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.selected > 0, egui::Button::new("Previous cue"))
                .clicked()
            {
                self.select(self.selected - 1);
            }
            if ui
                .add_enabled(
                    self.selected + 1 < self.document.cues.len(),
                    egui::Button::new("Next cue"),
                )
                .clicked()
            {
                self.select(self.selected + 1);
            }
            if !self.document.cues.is_empty() {
                let mut number = self.selected + 1;
                if ui
                    .add(
                        egui::DragValue::new(&mut number)
                            .clamp_range(1..=self.document.cues.len())
                            .prefix("Cue "),
                    )
                    .changed()
                {
                    self.select(number - 1);
                }
                ui.label(format!("of {}", self.document.cues.len()));
            }
        });
        let layout = crate::shell::StudioPanelLayout::default()
            .with_left()
            .allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.outline(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            self.caption(ui);
        });
    }
    fn outline(&mut self, ui: &mut Ui) {
        ui.label("Cues");
        let mut scroll = egui::ScrollArea::vertical()
            .id_source((self.id, "cues"))
            .auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_to_selected) {
            scroll = scroll.vertical_scroll_offset(self.selected as f32 * 28.0);
        }
        scroll.show_rows(ui, 28.0, self.document.cues.len(), |ui, rows| {
            for index in rows {
                let cue = &self.document.cues[index];
                let excerpt: String = cue
                    .text
                    .chars()
                    .take(80)
                    .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
                    .collect();
                let text = format!("{} · {} · {excerpt}", index + 1, format_time(cue.start_ms));
                if selectable_text_row(ui, text, self.selected == index, 28.0).clicked() {
                    self.select(index);
                }
            }
        });
    }
    fn caption(&self, ui: &mut Ui) {
        let Some(cue) = self.document.cues.get(self.selected) else {
            ui.label("This file has no subtitle cues.");
            return;
        };
        ui.heading(format!("Cue {}", self.selected + 1));
        ui.label(format!(
            "{} – {} · {:.3} s",
            format_time(cue.start_ms),
            format_time(cue.end_ms),
            (cue.end_ms - cue.start_ms) as f64 / 1000.0
        ));
        ui.label(format!(
            "Identifier: {} · source line {}",
            if cue.identifier.is_empty() {
                "not provided".to_owned()
            } else {
                cue.identifier.chars().take(256).collect::<String>()
            },
            cue.source_line
        ));
        if ui.button("Copy cue text").clicked() {
            ui.output_mut(|output| output.copied_text = cue.text.clone());
        }
        egui::ScrollArea::vertical().id_source((self.id,"caption",self.selected)).show(ui, |ui| {
            let prefix: String = cue.text.chars().take(800).collect();
            let display = prefix.lines().take(20).collect::<Vec<_>>().join("\n");
            ui.add(egui::Label::new(egui::RichText::new(&display).size(24.0)).wrap(true).selectable(true));
            if display.len() < cue.text.len() { ui.label("Caption display limited to 800 characters / 20 lines; Raw / Code shows the full cue."); }
            if !cue.settings.is_empty() { ui.label(format!("Cue settings: {}", cue.settings.chars().take(256).collect::<String>())); }
        });
    }
}
pub fn details(count: usize, timeline: &Timeline) -> String {
    format!(
        "{count} {} · end {} · peak {} at once",
        if count == 1 { "cue" } else { "cues" },
        format_time(timeline.duration_ms),
        timeline.peak_concurrency
    )
}
/// Paints bounded timeline aggregates and returns a navigation target for the hovered bin.
pub fn paint(
    ui: &Ui,
    rect: Rect,
    timeline: &Timeline,
    selected: Option<(u64, u64)>,
    pointer: Option<egui::Pos2>,
    colors: WorkbenchColors,
) -> Option<usize> {
    ui.painter().rect_filled(rect, 3.0, colors.recessed);
    if timeline.bins.is_empty() {
        return None;
    }
    let width = rect.width() / timeline.bins.len() as f32;
    for (index, bin) in timeline.bins.iter().enumerate() {
        if bin.cue.is_none() {
            continue;
        }
        let height =
            (bin.peak as f32 / timeline.peak_concurrency.max(1) as f32 * rect.height()).max(3.0);
        let x = rect.left() + index as f32 * width;
        let color = if bin.peak > 1 {
            colors.danger
        } else {
            colors.accent
        };
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(x, rect.bottom() - height),
                pos2(x + width, rect.bottom()),
            ),
            0.0,
            color,
        );
    }
    if let Some((start, end)) = selected {
        let x = |time| {
            rect.left() + (time as f64 / timeline.duration_ms.max(1) as f64) as f32 * rect.width()
        };
        let selected = Rect::from_min_max(
            pos2(x(start), rect.top()),
            pos2(x(end).max(x(start) + 1.0), rect.bottom()),
        );
        ui.painter()
            .rect_stroke(selected, 0.0, egui::Stroke::new(2.0_f32, colors.focus));
    }
    let pointer = pointer.filter(|point| rect.intersect(ui.clip_rect()).contains(*point))?;
    let bin = ((pointer.x - rect.left()) / width) as usize;
    let bin = bin.min(timeline.bins.len() - 1);
    let cue = timeline.bins[bin].cue?;
    let time = timeline.duration_ms as f64 * bin as f64 / timeline.bins.len() as f64;
    egui::show_tooltip_at_pointer(
        ui.ctx(),
        egui::Id::new(("subtitle-time", rect.min.x.to_bits(), rect.min.y.to_bits())),
        |ui| {
            ui.label(format!(
                "Around {} · peak {} at once · cue {}",
                format_time(time as u64),
                timeline.bins[bin].peak,
                cue + 1
            ));
            ui.label(&timeline.bins[bin].excerpt);
        },
    );
    Some(cue)
}
