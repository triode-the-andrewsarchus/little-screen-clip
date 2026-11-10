//! Read-only event agendas and independent wall-clock timelines.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    text_preview::{InlineTextPreview, TextPreviewData},
    ui_appearance::WorkbenchColors,
    ui_tabs::{Tab, TabBar},
};
use little_clip_core::{
    calendar::{Basis, Document, MAX_BYTES, Plan, tick_label},
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
        return Err("Calendar opening cancelled".into());
    }
    Document::parse(&bytes)
}
pub(super) struct Summary {
    plan: Option<Plan>,
}
fn largest_plan(document: &Document) -> usize {
    document
        .plans
        .iter()
        .enumerate()
        .max_by_key(|(_, plan)| plan.bars.len())
        .map_or(0, |(index, _)| index)
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Calendar".into(), Icon::Text);
    match read(path, token) {
        Ok(document) => {
            let mut plan = document.plans.get(largest_plan(&document)).cloned();
            let context = if let Some(plan) = &mut plan {
                plan.bars.retain(|bar| bar.lane < 4);
                plan.bars.truncate(128);
                plan.lanes = plan.lanes.min(4);
                format!(" · {} ({} shown)", plan.basis.label(), plan.bars.len())
            } else {
                " · no placeable dates".into()
            };
            data.detail = format!("{} events{}", document.events.len(), context);
            data.calendar = Some(Arc::new(Summary { plan }));
        }
        Err(error) => data.detail = error,
    }
    data
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Agenda,
    Timeline,
    Source,
}
pub(super) struct Viewer {
    document: Document,
    source: InlineTextPreview,
    mode: Mode,
    selected: usize,
    plan: usize,
    scroll_selected: bool,
    zoom: f32,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = read(path, &AtomicBool::new(false))?;
        let source = InlineTextPreview::new(
            TextPreviewData::from_source(Arc::new(document.source.clone()), Some("txt")),
            egui::vec2(800.0, 500.0),
        );
        let plan = largest_plan(&document);
        let selected = document
            .plans
            .get(plan)
            .and_then(|p| p.bars.first())
            .map_or(0, |b| b.event);
        Ok(Self {
            document,
            source,
            mode: Mode::Agenda,
            selected,
            plan,
            scroll_selected: false,
            zoom: 1.0,
        })
    }
    pub fn select(&mut self, index: usize) {
        if index >= self.document.events.len() {
            return;
        }
        self.selected = index;
        self.scroll_selected = true;
        if let Some(start) = &self.document.events[index].start {
            self.plan = self
                .document
                .plans
                .iter()
                .position(|p| p.basis == start.basis)
                .unwrap_or(self.plan);
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        StudioShell::new("Calendar", Icon::Text, palette, close).render_header(ui, |_| {});
        let tabs = [
            Tab::new(Mode::Agenda, "Agenda"),
            Tab::new(Mode::Timeline, "Timeline"),
            Tab::new(Mode::Source, "Source"),
        ];
        if let Some(mode) = TabBar::new("calendar-mode", self.mode, &tabs, palette).show(ui) {
            self.mode = mode;
        }
        if self.mode == Mode::Source {
            if self.document.source_truncated {
                ui.label("Source preview is truncated after 1 MiB.");
            }
            self.source
                .resize(ui.available_size().max(egui::vec2(1.0, 1.0)));
            self.source.show(ui, palette);
            return;
        }
        let layout = StudioPanelLayout::default().with_right().allocate(ui);
        if let Some(right) = layout.right {
            ui.allocate_ui_at_rect(right, |ui| {
                ui.set_clip_rect(right.intersect(ui.clip_rect()));
                self.details(ui, palette);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            if self.mode == Mode::Agenda {
                self.agenda(ui);
            } else {
                self.timeline(ui, palette);
            }
        });
    }
    fn agenda(&mut self, ui: &mut egui::Ui) {
        if !self.document.names.is_empty() {
            ui.heading(self.document.names.join(" · "));
        }
        ui.label(format!(
            "{} events · {} time bases",
            self.document.events.len(),
            self.document.plans.len()
        ));
        let height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        let mut scroll = egui::ScrollArea::vertical()
            .id_source("calendar-agenda")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0);
        if self.scroll_selected {
            let index = self
                .document
                .order
                .iter()
                .position(|i| *i == self.selected)
                .unwrap_or(0);
            scroll = scroll
                .vertical_scroll_offset(index as f32 * (height + ui.spacing().item_spacing.y));
            self.scroll_selected = false;
        }
        let mut clicked = None;
        scroll.show_rows(ui, height, self.document.order.len(), |ui, rows| {
            for row in rows {
                let index = self.document.order[row];
                let event = &self.document.events[index];
                let start = event.start.as_ref().map_or("Unplaced".into(), |m| {
                    format!("{} · {}", m.label, m.basis.label())
                });
                let text = format!(
                    "{}{} · {}",
                    if event.cancelled { "Cancelled · " } else { "" },
                    start,
                    event.title
                );
                if crate::ui_ext::selectable_text_row(ui, text, index == self.selected, height)
                    .on_hover_text(&event.title)
                    .clicked()
                {
                    clicked = Some(index);
                }
            }
        });
        if let Some(index) = clicked {
            self.select(index);
            self.scroll_selected = false;
        }
    }
    fn timeline(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.document.plans.is_empty() {
            ui.label("No readable event dates. Use Agenda or Source to inspect the records.");
            return;
        }
        let previous = self.plan;
        egui::ComboBox::from_id_source("calendar-basis")
            .selected_text(self.document.plans[self.plan].basis.label())
            .show_ui(ui, |ui| {
                for (index, plan) in self.document.plans.iter().enumerate() {
                    ui.selectable_value(&mut self.plan, index, plan.basis.label());
                }
            });
        if previous != self.plan {
            self.selected = self.document.plans[self.plan].bars[0].event;
        }
        ui.label("Declared times only · recurring instances are not expanded");
        ui.add(egui::Slider::new(&mut self.zoom, 1.0..=8.0).text("Timeline zoom"));
        let plan = &self.document.plans[self.plan];
        let width = ui.available_width().max(1.0) * self.zoom;
        egui::ScrollArea::both()
            .id_source("calendar-timeline")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(width, (plan.lanes as f32 * 34.0 + 52.0).max(160.0)),
                    egui::Sense::click(),
                );
                let hovered = paint_plan(
                    ui,
                    rect,
                    plan,
                    Some(self.selected),
                    true,
                    WorkbenchColors::from_palette(palette),
                );
                if response.clicked() {
                    if let Some(index) = hovered {
                        self.selected = index;
                    }
                }
            });
    }
    fn details(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        egui::ScrollArea::vertical()
            .id_source("calendar-details")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                if let Some(event) = self.document.events.get(self.selected) {
                    ui.heading(&event.title);
                    if event.cancelled {
                        ui.colored_label(
                            WorkbenchColors::from_palette(palette).warning,
                            "Cancelled",
                        );
                    }
                    if !event.calendar.is_empty() {
                        ui.label(&event.calendar);
                    }
                    if let Some(start) = &event.start {
                        ui.strong("Starts");
                        ui.label(&start.label);
                        ui.label(start.basis.label());
                    }
                    if let Some(end) = &event.end {
                        ui.strong("Ends (exclusive)");
                        ui.label(&end.label);
                        if event.inferred_end {
                            ui.label("Derived from duration or the all-day default");
                        }
                    }
                    if !event.location.is_empty() {
                        ui.strong("Location");
                        ui.label(&event.location);
                    }
                    if !event.description.is_empty() {
                        ui.separator();
                        ui.add(egui::Label::new(&event.description).wrap(true));
                    }
                    for note in &event.notes {
                        ui.colored_label(WorkbenchColors::from_palette(palette).warning, note);
                    }
                    ui.collapsing("Event properties", |ui| {
                        properties(ui, &event.properties);
                    });
                }
                ui.separator();
                ui.label(format!(
                    "{} time-zone definitions · {} other components",
                    self.document.timezones, self.document.other_components
                ));
                ui.collapsing("Calendar properties", |ui| {
                    properties(ui, &self.document.properties);
                });
            });
    }
}
fn properties(ui: &mut egui::Ui, fields: &[(String, String)]) {
    for (name, value) in fields {
        ui.strong(name);
        ui.add(egui::Label::new(value).wrap(true));
    }
}
fn paint_plan(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    plan: &Plan,
    selected: Option<usize>,
    axis: bool,
    colors: WorkbenchColors,
) -> Option<usize> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 2.0, colors.recessed);
    let graph = egui::Rect::from_min_max(
        rect.min + egui::vec2(4.0, if axis { 42.0 } else { 4.0 }),
        rect.max - egui::vec2(4.0, 4.0),
    );
    let span = (plan.last - plan.first).max(1) as f64;
    if axis {
        for step in 0..=4 {
            let x = graph.left() + graph.width() * step as f32 / 4.0;
            let tick = plan.first + ((plan.last - plan.first) as f64 * step as f64 / 4.0) as i64;
            let label = tick_label(tick, plan.basis == Basis::Date);
            painter.line_segment(
                [egui::pos2(x, graph.top()), egui::pos2(x, graph.bottom())],
                egui::Stroke::new(1.0_f32, colors.border),
            );
            let align = if step == 4 {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::LEFT_TOP
            };
            painter.text(
                egui::pos2(x, rect.top() + 4.0),
                align,
                label.replace(' ', "\n"),
                egui::TextStyle::Small.resolve(ui.style()),
                colors.text_secondary,
            );
        }
    }
    let lane_height = (graph.height() / plan.lanes.max(1) as f32).min(34.0);
    let mut hovered = None;
    for bar in &plan.bars {
        let x = graph.left() + ((bar.start - plan.first) as f64 / span) as f32 * graph.width();
        let end = graph.left() + ((bar.end - plan.first) as f64 / span) as f32 * graph.width();
        let width = (end - x).max(6.0).min(graph.width());
        let x = x.min(graph.right() - width);
        let cell = egui::Rect::from_min_size(
            egui::pos2(x, graph.top() + bar.lane as f32 * lane_height),
            egui::vec2(width, (lane_height - 3.0).max(1.0)),
        );
        if !cell.intersects(ui.clip_rect()) {
            continue;
        }
        let color = if bar.cancelled {
            colors.warning
        } else {
            colors.accent
        };
        painter.rect_filled(cell, 2.0, color.gamma_multiply(0.5));
        if selected == Some(bar.event) {
            painter.rect_stroke(
                cell.shrink(1.0),
                2.0,
                egui::Stroke::new(2.0_f32, colors.focus),
            );
        }
        if axis {
            painter.with_clip_rect(cell.intersect(ui.clip_rect())).text(
                cell.left_center() + egui::vec2(4.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &bar.title,
                egui::TextStyle::Body.resolve(ui.style()),
                colors.text_primary,
            );
        }
        if super::ui::preview_hovered(ui, cell) {
            hovered = Some(bar.event);
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("calendar-event", bar.event)),
                |ui| {
                    ui.label(&bar.title);
                    ui.label(format!(
                        "{} · {}",
                        tick_label(bar.start, plan.basis == Basis::Date),
                        plan.basis.label()
                    ));
                    if bar.cancelled {
                        ui.label("Cancelled");
                    }
                },
            );
        }
    }
    hovered
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let strip = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Calendar events", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(strip, 2.0, colors.recessed);
    let summary = ready.and_then(|r| r.data.calendar.as_ref())?;
    paint_plan(ui, strip, summary.plan.as_ref()?, None, false, colors)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_event_indices_open_the_same_agenda_records() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/calendar/harbor.ics");
        let data = load(&path, &AtomicBool::new(false));
        let plan = data.calendar.unwrap();
        let plan = plan.plan.as_ref().unwrap();
        assert_eq!(plan.basis, Basis::Utc);
        assert_eq!(plan.bars.len(), 2);
        assert_eq!(plan.lanes, 2);
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select(plan.bars[1].event);
        assert_eq!(viewer.document.events[viewer.selected].title, "Tide watch");
        assert!(viewer.document.events[viewer.selected].cancelled);
        viewer.select(usize::MAX);
        assert_eq!(viewer.selected, 1);
        assert!(read(&path, &AtomicBool::new(true)).is_err());
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("CALENDAR.ICS"))
                == Some(little_clip_core::file_format::Format::Calendar))
        );
    }
}
