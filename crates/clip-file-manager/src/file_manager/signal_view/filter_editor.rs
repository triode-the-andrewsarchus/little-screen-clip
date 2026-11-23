use super::super::scope_view::plot;
use egui::{Align2, Color32, FontId, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    analysis::{FilterChain, FilterKind, FilterStage},
    waveforms::display_value,
};
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

pub(in crate::file_manager) const FILTER_COLOR: Color32 = Color32::from_rgb(221, 163, 93);
const MAX_STAGES: usize = 8;

#[derive(Clone, Default, PartialEq)]
pub(in crate::file_manager) struct StageList(Vec<FilterStage>);

impl StageList {
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
    ) {
        if !self.can_add() {
            return;
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

    pub(in crate::file_manager) fn controls(&mut self, ui: &mut Ui, rate: f64) {
        let bounds = rate * 1e-7..=rate * (0.5 - 1e-7);
        let remove = ui
            .menu_button("Filter stages", |ui| {
                let mut remove = None;
                for (index, stage) in self.0.iter_mut().enumerate() {
                    ui.push_id(("filter-stage-controls", index), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(kind_label(stage.kind));
                            ui.add(
                                egui::DragValue::new(&mut stage.frequency_hz)
                                    .range(bounds.clone())
                                    .speed(rate / 5000.0)
                                    .suffix(" Hz"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut stage.q)
                                    .range(0.1..=25.0)
                                    .speed(0.04)
                                    .prefix("Q "),
                            )
                            .on_hover_text(
                                "Quality factor · higher Q narrows a band-pass or notch",
                            );
                            if ui
                                .small_button("×")
                                .on_hover_text("Remove filter")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    });
                }
                remove
            })
            .inner
            .flatten();
        if let Some(index) = remove {
            self.0.remove(index);
        }
    }

    pub(in crate::file_manager) fn draw(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        span: &Range<f64>,
        rate: f64,
    ) {
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
            painter.add(egui::Shape::line(
                points.collect(),
                Stroke::new(1.2, FILTER_COLOR),
            ));
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
                    FILTER_COLOR,
                );
            }
        }
        for (index, stage) in self.0.iter_mut().enumerate() {
            let previous = *stage;
            ui.push_id(("filter-handles", index), |ui| {
                stage_handles(ui, &painter, rect, span, rate, stage);
            });
            if previous != *stage {
                ui.ctx().request_repaint();
            }
        }
    }
}

fn stage_handles(
    ui: &mut Ui,
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    rate: f64,
    stage: &mut FilterStage,
) {
    let label = format!(
        "{} · {}",
        kind_label(stage.kind),
        display_value(stage.frequency_hz, "Hz")
    );
    if let Some(target) = handle(
        ui,
        painter,
        rect,
        span,
        stage.frequency_hz,
        ("center", 9.0),
        label,
    ) {
        stage.frequency_hz = target.clamp(rate * 1e-7, rate * (0.5 - 1e-7));
    }
    let Some(edges) = stage.band_edges(rate) else {
        return;
    };
    plot::highlight(
        painter,
        rect,
        span,
        edges[0]..edges[1],
        FILTER_COLOR.gamma_multiply(0.10),
    );
    for (name, frequency) in ["lower edge", "upper edge"].into_iter().zip(edges) {
        let label = format!(
            "{} {name} · {}",
            kind_label(stage.kind),
            display_value(frequency, "Hz")
        );
        if let Some(edge) = handle(ui, painter, rect, span, frequency, (name, 23.0), label)
            && let Some(q) = stage.q_for_edge(rate, edge)
        {
            stage.q = q;
        }
    }
}

fn handle(
    ui: &mut Ui,
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    frequency: f64,
    (id, y): (&str, f32),
    tooltip: String,
) -> Option<f64> {
    let x = plot::x_at(rect, span, frequency);
    if !rect.x_range().contains(x) {
        return None;
    }
    let target = Rect::from_center_size(pos2(x, rect.top() + y), vec2(16.0, 14.0));
    let response = ui
        .interact(target, ui.make_persistent_id(id), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_text(tooltip);
    painter.vline(x, rect.y_range(), Stroke::new(1.0, FILTER_COLOR));
    painter.rect_filled(target.shrink2(vec2(3.0, 3.0)), 2.0, FILTER_COLOR);
    response
        .dragged()
        .then(|| {
            response
                .interact_pointer_pos()
                .map(|pointer| plot::at(rect, span, pointer.x))
        })
        .flatten()
}

pub(in crate::file_manager) fn filter_menu(ui: &mut Ui) -> Option<FilterKind> {
    [
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::BandPass,
        FilterKind::Notch,
    ]
    .into_iter()
    .find(|&kind| {
        let clicked = ui.button(kind_label(kind)).clicked();
        if clicked {
            ui.close();
        }
        clicked
    })
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
