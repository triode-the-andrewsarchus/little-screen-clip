use super::{super::scope_view::plot, timeline};
use egui::{
    Align2, Button, Color32, FontId, Painter, PointerButton, Rect, Response, Sense, Stroke, Ui,
    pos2, vec2,
};
use little_clip_core::engineering::{
    analysis::{FilterChain, FilterKind, FilterStage},
    waveforms::display_value,
};
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

pub(in crate::file_manager) const FILTER_COLOR: Color32 = Color32::from_rgb(221, 163, 93);

pub(in crate::file_manager) fn filter_ink(dark: bool) -> Color32 {
    if dark {
        FILTER_COLOR
    } else {
        Color32::from_rgb(150, 85, 20)
    }
}
const MAX_STAGES: usize = 8;

#[derive(Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct StageList(Vec<FilterStage>);

#[derive(Clone, Default)]
pub(in crate::file_manager) struct StageFocus {
    selected: usize,
    focus_row: bool,
}

impl StageFocus {
    pub(in crate::file_manager) fn select(&mut self, index: usize) {
        self.selected = index;
        self.focus_row = true;
    }
}

impl StageList {
    pub(in crate::file_manager) fn validate(&self) -> Result<(), String> {
        if self.0.len() > MAX_STAGES
            || self.0.iter().any(|stage| {
                !stage.frequency_hz.is_finite()
                    || stage.frequency_hz <= 0.0
                    || !stage.q.is_finite()
                    || stage.q <= 0.0
            })
        {
            return Err("Invalid filter stages".into());
        }
        Ok(())
    }

    pub(in crate::file_manager) fn can_add(&self) -> bool {
        self.0.len() < MAX_STAGES
    }

    pub(in crate::file_manager) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(in crate::file_manager) fn add(
        &mut self,
        kind: FilterKind,
        rate: f64,
        span: &Range<f64>,
        marker: Option<f64>,
    ) -> Option<usize> {
        if !self.can_add() {
            return None;
        }
        let frequency = marker
            .filter(|marker| span.contains(marker))
            .unwrap_or_else(|| stage_position(span, &self.0))
            .clamp(rate * 1e-7, rate * (0.5 - 1e-7));
        self.0.push(FilterStage {
            kind,
            frequency_hz: frequency,
            q: match kind {
                FilterKind::LowPass | FilterKind::HighPass => std::f64::consts::FRAC_1_SQRT_2,
                FilterKind::BandPass | FilterKind::Notch => 4.0,
            },
        });
        Some(self.0.len() - 1)
    }

    pub(in crate::file_manager) fn apply(
        &self,
        rate: f64,
        samples: &mut [f32],
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut chain = FilterChain::new(rate, &self.0)?;
        for block in samples.chunks_mut(4096) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Filtering was cancelled".into());
            }
            chain.process(block);
        }
        Ok(())
    }

    pub(in crate::file_manager) fn controls(
        &mut self,
        ui: &mut Ui,
        focus: &mut StageFocus,
        rate: f64,
    ) -> Option<f64> {
        let before = focus.selected;
        let requested = std::mem::take(&mut focus.focus_row).then_some(before);
        let mut reveal = None;
        let mut remove = None;
        let mut render = |ui: &mut Ui, index: usize, stage: &mut FilterStage| {
            let row = ui
                .push_id(index, |ui| {
                    stage_row(ui, focus, requested, index, stage, rate)
                })
                .inner;
            if row.select {
                reveal = Some(stage.frequency_hz);
            }
            if row.remove {
                remove = Some(index);
            }
        };
        if ui.available_width() >= 700.0 {
            for (row, stages) in self.0.chunks_mut(2).enumerate() {
                ui.columns(2, |columns| {
                    for (slot, (column, stage)) in columns.iter_mut().zip(stages).enumerate() {
                        render(column, row * 2 + slot, stage);
                    }
                });
            }
        } else {
            for (index, stage) in self.0.iter_mut().enumerate() {
                render(ui, index, stage);
            }
        }
        if let Some(index) = remove {
            self.0.remove(index);
            if index < focus.selected {
                focus.selected -= 1;
            }
            focus.selected = focus.selected.min(self.0.len().saturating_sub(1));
        }
        if remove.is_some() || reveal.is_some() || focus.selected != before {
            ui.ctx().request_repaint();
        }
        reveal
    }

    pub(in crate::file_manager) fn draw(
        &mut self,
        ui: &mut Ui,
        focus: &mut StageFocus,
        rect: Rect,
        span: &Range<f64>,
        rate: f64,
    ) {
        let color = filter_ink(ui.visuals().dark_mode);
        let painter = ui.painter_at(rect);
        if let Ok(chain) = FilterChain::new(rate, &self.0) {
            let points = (0..=160).filter_map(|index| {
                let fraction = f64::from(index) / 160.0;
                let frequency = span.start + fraction * (span.end - span.start);
                let gain = chain.gain_at(frequency)?;
                let db = (20.0 * gain.max(1e-6).log10()).clamp(-60.0, 24.0);
                Some(pos2(
                    plot::x_at(rect, span, frequency),
                    rect.bottom() - rect.height() * ((db + 60.0) / 84.0) as f32,
                ))
            });
            painter.add(egui::Shape::line(points.collect(), Stroke::new(1.2, color)));
            for db in [-60, 0, 24] {
                let y = rect.bottom() - rect.height() * (db + 60) as f32 / 84.0;
                painter.text(
                    pos2(
                        rect.right() - 4.0,
                        y.clamp(rect.top() + 7.0, rect.bottom() - 7.0),
                    ),
                    Align2::RIGHT_CENTER,
                    format!("{db} dB gain"),
                    FontId::monospace(10.0),
                    color,
                );
            }
        }
        let selected = focus.selected;
        // Draw the selected handles last so overlapping stages remain editable.
        let order = (0..self.0.len())
            .filter(|&index| index != selected)
            .chain((selected < self.0.len()).then_some(selected));
        for index in order {
            let stage = &mut self.0[index];
            let previous = *stage;
            let active = index == selected;
            let interacted = ui
                .push_id(("filter-handles", index), |ui| {
                    StagePlot {
                        painter: &painter,
                        rect,
                        span,
                        rate,
                        color,
                    }
                    .stage(ui, stage, index, active)
                })
                .inner;
            if interacted && !active {
                focus.select(index);
            }
            if previous != *stage || (interacted && !active) {
                ui.ctx().request_repaint();
            }
        }
    }
}

struct RowAction {
    select: bool,
    remove: bool,
}

fn stage_row(
    ui: &mut Ui,
    focus: &mut StageFocus,
    requested: Option<usize>,
    index: usize,
    stage: &mut FilterStage,
    rate: f64,
) -> RowAction {
    let name = format!("{} {}", index + 1, kind_label(stage.kind));
    ui.horizontal_wrapped(|ui| {
        let spacing = ui.spacing_mut();
        spacing.interact_size.y = 24.0;
        spacing.item_spacing.x = 4.0;
        let label = ui
            .add(Button::new(&name).small().selected(focus.selected == index))
            .on_hover_text("Show this stage on the spectrum");
        if requested == Some(index) {
            label.request_focus();
            if !ui.clip_rect().contains_rect(label.rect) {
                label.scroll_to_me(None);
            }
        }
        let frequency = ui
            .add(
                egui::DragValue::new(&mut stage.frequency_hz)
                    .range(rate * 1e-7..=rate * (0.5 - 1e-7))
                    .clamp_existing_to_range(false)
                    .speed(rate / 5000.0)
                    .suffix(" Hz")
                    .update_while_editing(false),
            )
            .on_hover_text(format!("{name} frequency"));
        let q = ui
            .add(
                egui::DragValue::new(&mut stage.q)
                    .range(0.1..=25.0)
                    .clamp_existing_to_range(false)
                    .speed(0.04)
                    .prefix("Q ")
                    .update_while_editing(false),
            )
            .on_hover_text(format!("{name} quality factor"));
        let editing = [&frequency, &q]
            .into_iter()
            .any(|value| value.has_focus() || value.dragged() || value.changed());
        if label.clicked() || (requested.is_none() && (label.has_focus() || editing)) {
            focus.selected = index;
        }
        let remove = ui
            .small_button("×")
            .on_hover_text(format!("Remove {name}"))
            .clicked();
        RowAction {
            select: label.clicked(),
            remove,
        }
    })
    .inner
}

struct StagePlot<'a> {
    painter: &'a Painter,
    rect: Rect,
    span: &'a Range<f64>,
    rate: f64,
    color: Color32,
}

impl StagePlot<'_> {
    fn stage(&self, ui: &mut Ui, stage: &mut FilterStage, index: usize, active: bool) -> bool {
        let title = format!("{} {}", index + 1, kind_label(stage.kind));
        let mut interacted = false;
        if let Some(center) = self.handle(ui, stage.frequency_hz, &title, Some(index + 1), active) {
            interacted = center.clicked() || center.dragged_by(PointerButton::Primary);
            if let Some(target) = self.drag_frequency(&center) {
                stage.frequency_hz = target.clamp(self.rate * 1e-7, self.rate * (0.5 - 1e-7));
            }
        }
        let Some(edges) = stage.band_edges(self.rate) else {
            return interacted;
        };
        plot::highlight(
            self.painter,
            self.rect,
            self.span,
            edges[0]..edges[1],
            self.color.gamma_multiply(if active { 0.13 } else { 0.05 }),
        );
        if active {
            for (edge, frequency) in ["Lower edge", "Upper edge"].into_iter().zip(edges) {
                if let Some(response) = self.handle(ui, frequency, edge, None, true)
                    && let Some(frequency) = self.drag_frequency(&response)
                    && let Some(q) = stage.q_for_edge(self.rate, frequency)
                {
                    stage.q = q;
                }
            }
        }
        interacted
    }

    fn handle(
        &self,
        ui: &mut Ui,
        frequency: f64,
        label: &str,
        number: Option<usize>,
        active: bool,
    ) -> Option<Response> {
        let x = plot::x_at(self.rect, self.span, frequency);
        if !self.rect.x_range().contains(x) {
            return None;
        }
        let y = if number.is_some() { 10.0 } else { 25.0 };
        let target = Rect::from_center_size(pos2(x, self.rect.top() + y), vec2(19.0, 16.0));
        let response = ui
            .interact(
                target,
                ui.make_persistent_id(label),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text(format!("{label} · {}", display_value(frequency, "Hz")));
        let color = if active {
            self.color
        } else {
            self.color.gamma_multiply(0.56)
        };
        self.painter.vline(
            x,
            self.rect.y_range(),
            Stroke::new(if active { 1.5 } else { 1.0 }, color),
        );
        let tag = target.shrink2(vec2(2.0, 2.0));
        self.painter.rect_filled(tag, 2.0, color);
        if let Some(number) = number {
            self.painter.text(
                tag.center(),
                Align2::CENTER_CENTER,
                number,
                FontId::monospace(10.0),
                ui.visuals().extreme_bg_color,
            );
        }
        Some(response)
    }

    fn drag_frequency(&self, response: &Response) -> Option<f64> {
        response
            .interact_pointer_pos()
            .filter(|_| response.dragged_by(PointerButton::Primary))
            .map(|pointer| plot::at(self.rect, self.span, pointer.x))
    }
}

#[derive(Clone, Copy)]
pub(in crate::file_manager) enum SpectrumPane {
    Chart,
    Filters,
}

pub(in crate::file_manager) fn spectrum_workspace(
    ui: &mut Ui,
    filterable: bool,
    chart_height: f32,
    mut render: impl FnMut(&mut Ui, SpectrumPane),
) {
    const EDITOR_WIDTH: f32 = 338.0;
    ui.scope(|ui| {
        if filterable && ui.available_width() >= 910.0 {
            let chart_width = ui.available_width() - EDITOR_WIDTH - ui.spacing().item_spacing.x;
            let layout = egui::Layout::top_down(egui::Align::Min);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(vec2(chart_width, chart_height), layout, |ui| {
                    render(ui, SpectrumPane::Chart);
                });
                ui.allocate_ui_with_layout(vec2(EDITOR_WIDTH, chart_height), layout, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            render(ui, SpectrumPane::Filters);
                        });
                });
            });
        } else {
            if filterable {
                render(ui, SpectrumPane::Filters);
            }
            ui.allocate_ui(vec2(ui.available_width(), chart_height), |ui| {
                render(ui, SpectrumPane::Chart);
            });
        }
    });
}

pub(in crate::file_manager) fn add_buttons(ui: &mut Ui, enabled: bool) -> Option<FilterKind> {
    ui.weak("Add filter");
    let mut add = None;
    for kind in [
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::BandPass,
        FilterKind::Notch,
    ] {
        if ui
            .add_enabled(enabled, Button::new(kind_label(kind)).small())
            .on_disabled_hover_text("Remove a stage to add another filter")
            .clicked()
        {
            add = Some(kind);
        }
    }
    add
}

pub(in crate::file_manager) fn reveal_frequency(span: &mut Option<Range<f64>>, hz: f64, rate: f64) {
    let Some(range) = span.as_ref().filter(|range| !range.contains(&hz)) else {
        return;
    };
    let half = (range.end - range.start) * 0.5;
    *span = Some(timeline::fit(&(0.0..rate * 0.5), hz - half..hz + half, 0.0));
}

fn stage_position(span: &Range<f64>, stages: &[FilterStage]) -> f64 {
    let mut edges = stages
        .iter()
        .map(|stage| stage.frequency_hz)
        .filter(|frequency| span.contains(frequency))
        .collect::<Vec<_>>();
    edges.push(span.start);
    edges.push(span.end);
    edges.sort_by(f64::total_cmp);
    edges
        .windows(2)
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map_or(span.start.midpoint(span.end), |pair| {
            pair[0].midpoint(pair[1])
        })
}

fn kind_label(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::LowPass => "Low-pass",
        FilterKind::HighPass => "High-pass",
        FilterKind::BandPass => "Band-pass",
        FilterKind::Notch => "Notch",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_at_visible_marker_and_spreads_other_stages_in_the_view() {
        let span = 0.0..10_000.0;
        let mut stages = StageList::default();
        assert_eq!(
            stages.add(FilterKind::LowPass, 40_000.0, &span, Some(4000.0)),
            Some(0)
        );
        assert_eq!(
            stages.add(FilterKind::Notch, 40_000.0, &span, Some(18_000.0)),
            Some(1)
        );
        assert_eq!(stages.0[0].frequency_hz, 4000.0);
        assert_eq!(stages.0[1].frequency_hz, 7000.0);
        assert_eq!(stages.0[1].q, 4.0);
        let recipe = serde_json::to_value(&stages).unwrap();
        assert_eq!(recipe[0]["frequency_hz"], 4000.0);
        assert!(serde_json::from_value::<StageList>(recipe).unwrap() == stages);
    }

    #[test]
    fn reveal_moves_zoom_to_an_offscreen_stage_without_changing_width() {
        let mut view = Some(3000.0..5000.0);
        reveal_frequency(&mut view, 4000.0, 12_000.0);
        assert_eq!(view, Some(3000.0..5000.0));
        reveal_frequency(&mut view, 1000.0, 12_000.0);
        assert_eq!(view, Some(0.0..2000.0));
    }
}
