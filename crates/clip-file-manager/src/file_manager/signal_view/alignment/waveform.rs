use self::measure::{Report, Request};
use super::super::inputs::{Input, Signature};
use super::{PairKey, Suggestion, measure};
use crate::file_manager::{
    capture_browse::{Job, poll, start},
    scope_view::plot::draw_raw,
};
use egui::{Align2, Color32, Context, FontId, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::{Clock, Correlation},
        waveforms::{Peak, display_value},
    },
};
use std::{ops::Range, time::Duration};

#[derive(Clone)]
pub(super) struct Entry {
    pub source: u64,
    pub input: Input,
}

#[derive(PartialEq)]
struct Key {
    reference: Signature,
    target: Signature,
    recordings: PairKey,
    reference_clock: Clock,
    base: Clock,
    center: f64,
    duration: f64,
    search: f64,
}

#[derive(Default)]
pub(super) struct Waveform {
    reference: Option<String>,
    target: Option<String>,
    center: f64,
    duration: f64,
    search: f64,
    pending: Option<Job<Report>>,
    result: Option<Result<Report, String>>,
    key: Option<Key>,
    picked: Option<usize>,
}

impl Waveform {
    pub fn use_region(&mut self, visible: &Range<f64>, reference: Option<f64>) {
        let width = visible.end - visible.start;
        self.center = reference.unwrap_or_else(|| visible.start.midpoint(visible.end));
        self.duration = width * 0.25;
        self.search = width * 0.38;
        self.retire();
    }

    pub(super) fn suggestion(&self) -> Option<Suggestion> {
        let key = self.key.as_ref()?;
        let report = self.result.as_ref()?.as_ref().ok()?;
        let picked = self.picked?;
        Some(Suggestion {
            recordings: key.recordings,
            reference_clock: key.reference_clock,
            base: key.base,
            clock: key.base.shifted(report.change(picked)),
        })
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        entries: &[Entry],
        recordings: PairKey,
        visible: &Range<f64>,
        reference: Option<f64>,
        preview: bool,
    ) {
        if let Some(result) = poll(&mut self.pending) {
            self.result = Some(result);
        }
        self.pick_sources(ui, entries, recordings);
        self.span_controls(ui, visible, reference);
        let pair = self.pair(entries);
        let key = pair
            .as_ref()
            .map(|[reference, target]| self.make_key(reference, target));
        if self.key != key {
            self.retire();
        }
        if ui
            .add_enabled(pair.is_some(), egui::Button::new("Find matches"))
            .clicked()
            && let (Some([reference, target]), Some(key)) = (pair, key)
        {
            self.request(ui.ctx(), reference, target, key);
        }
        self.results(ui, preview);
        if self.pending.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(24));
        }
    }

    fn pick_sources(&mut self, ui: &mut Ui, entries: &[Entry], recordings: PairKey) {
        ui.columns(2, |columns| {
            for (index, (label, source, current)) in [
                ("Reference lane", recordings.reference, &mut self.reference),
                ("Target lane", recordings.target, &mut self.target),
            ]
            .into_iter()
            .enumerate()
            {
                let ui = &mut columns[index];
                let available: Vec<_> = entries
                    .iter()
                    .filter(|entry| entry.source == source)
                    .collect();
                if !available
                    .iter()
                    .any(|entry| current.as_ref() == Some(&entry.input.alias))
                {
                    *current = available.first().map(|entry| entry.input.alias.clone());
                }
                ui.strong(label);
                egui::ScrollArea::vertical()
                    .id_salt(label)
                    .max_height(67.0)
                    .show_rows(
                        ui,
                        ui.spacing().interact_size.y,
                        available.len(),
                        |ui, rows| {
                            for entry in available[rows].iter() {
                                ui.selectable_value(
                                    current,
                                    Some(entry.input.alias.clone()),
                                    &entry.input.label,
                                );
                            }
                        },
                    );
                if available.is_empty() {
                    ui.weak("No readable lanes");
                }
            }
        });
    }

    fn span_controls(&mut self, ui: &mut Ui, visible: &Range<f64>, reference: Option<f64>) {
        let width = visible.end - visible.start;
        let speed = width.max(f64::MIN_POSITIVE) * 0.002;
        ui.horizontal_wrapped(|ui| {
            ui.label("Center");
            ui.add(
                egui::DragValue::new(&mut self.center)
                    .max_decimals(14)
                    .speed(speed)
                    .suffix(" s"),
            );
            if ui
                .add_enabled(reference.is_some(), egui::Button::new("Use pinned cursor"))
                .clicked()
                && let Some(reference) = reference
            {
                self.center = reference;
            }
            ui.label("Reference duration");
            ui.add(
                egui::DragValue::new(&mut self.duration)
                    .max_decimals(14)
                    .speed(speed)
                    .suffix(" s"),
            );
            ui.label("Search ±");
            ui.add(
                egui::DragValue::new(&mut self.search)
                    .max_decimals(14)
                    .speed(speed)
                    .suffix(" s"),
            );
        });
    }

    fn pair(&self, entries: &[Entry]) -> Option<[Entry; 2]> {
        let find = |alias: Option<&String>| {
            entries
                .iter()
                .find(|entry| Some(&entry.input.alias) == alias)
                .cloned()
        };
        let a = find(self.reference.as_ref())?;
        let b = find(self.target.as_ref())?;
        (a.source != b.source).then_some([a, b])
    }

    fn make_key(&self, reference: &Entry, target: &Entry) -> Key {
        Key {
            reference: reference.input.signature(),
            target: target.input.signature(),
            recordings: PairKey {
                reference: reference.source,
                target: target.source,
            },
            reference_clock: reference.input.clock(),
            base: target.input.clock(),
            center: self.center,
            duration: self.duration,
            search: self.search,
        }
    }

    pub(super) fn retire(&mut self) {
        self.key = None;
        self.pending = None;
        self.result = None;
        self.picked = None;
    }

    fn request(&mut self, ctx: &Context, reference: Entry, target: Entry, key: Key) {
        self.retire();
        let request = Request {
            reference: reference.input,
            target: target.input,
            center: key.center,
            duration: key.duration,
            search: key.search,
        };
        self.key = Some(key);
        self.pending = Some(start(
            BackgroundJobs::global(),
            ctx,
            Duration::from_secs(20),
            move |cancel| measure::compare(&request, cancel),
        ));
    }

    fn results(&mut self, ui: &mut Ui, preview: bool) {
        if self.pending.is_some() {
            ui.weak("Comparing overlapping samples…");
        }
        let Some(result) = self.result.as_ref() else {
            return;
        };
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
        };
        ui.weak(format!(
            "{} to {} · offset step {}",
            display_value(report.interval.start, "s"),
            display_value(report.interval.end, "s"),
            display_value(report.step, "s"),
        ));
        self.picked = candidate_buttons(ui, report, self.picked).or(self.picked);
        let comparison = if preview {
            self.picked.unwrap_or(report.radius)
        } else {
            report.radius
        };
        let mut choice = None;
        egui::ScrollArea::vertical()
            .id_salt("waveform-match-plots")
            .max_height((ui.available_height() * 0.60).max(64.0))
            .show(ui, |ui| {
                if ui.available_width() > 740.0 {
                    ui.columns(2, |columns| {
                        choice = graph(&mut columns[0], report, self.picked);
                        compare_traces(&mut columns[1], report, comparison);
                    });
                } else {
                    choice = graph(ui, report, self.picked);
                    compare_traces(ui, report, comparison);
                }
            });
        self.picked = choice.or(self.picked);
    }
}

fn candidate_buttons(ui: &mut Ui, report: &Report, selected: Option<usize>) -> Option<usize> {
    if report.peaks.is_empty() {
        ui.weak("No matching variation with enough overlapping samples");
    }
    let mut choice = None;
    ui.horizontal_wrapped(|ui| {
        for candidate in &report.peaks {
            let label = candidate_label(report, *candidate);
            if ui
                .add(egui::Button::new(label).selected(selected == Some(candidate.start)))
                .on_hover_text(format!("{} overlapping sample pairs", candidate.overlap))
                .clicked()
            {
                choice = Some(candidate.start);
            }
        }
    });
    choice
}

fn compare_traces(ui: &mut Ui, report: &Report, offset: usize) {
    let Some(traces) = report.traces(offset) else {
        return;
    };
    let colors = [
        Color32::from_rgb(85, 181, 235),
        Color32::from_rgb(229, 173, 90),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(colors[0], "Reference");
        ui.colored_label(colors[1], "Target");
        ui.weak("Normalized amplitude");
    });
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), Sense::hover());
    let rect = Rect::from_min_max(area.min + vec2(1.0, 3.0), area.max - vec2(2.0, 16.0));
    let painter = ui.painter_at(area);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let mut bounds = Peak::default();
    bounds.add(&[-1.0, 1.0]);
    let plot = painter.with_clip_rect(rect);
    for (values, color) in traces.into_iter().zip(colors) {
        let values = normalize(values);
        draw_raw(
            &plot,
            rect,
            &report.interval,
            report.interval.start,
            report.step,
            &values,
            bounds,
            color,
        );
    }
    for (x, time, align) in [
        (rect.left(), report.interval.start, Align2::LEFT_TOP),
        (rect.right(), report.interval.end, Align2::RIGHT_TOP),
    ] {
        painter.text(
            pos2(x, rect.bottom() + 2.0),
            align,
            display_value(time, "s"),
            FontId::monospace(10.0),
            ui.visuals().weak_text_color(),
        );
    }
}

fn normalize(samples: &[f32]) -> Vec<f32> {
    let (sum, count) = samples
        .iter()
        .filter(|value| value.is_finite())
        .fold((0.0, 0), |(sum, count), &value| {
            (sum + f64::from(value), count + 1)
        });
    let mean = sum / f64::from(count.max(1));
    let width = samples
        .iter()
        .filter(|value| value.is_finite())
        .map(|&value| (f64::from(value) - mean).abs())
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    samples
        .iter()
        .map(|&value| ((f64::from(value) - mean) / width) as f32)
        .collect()
}

fn candidate_label(report: &Report, candidate: Correlation) -> String {
    let change = report.change(candidate.start);
    let score = candidate.coefficient.unwrap_or_default();
    format!(
        "{}{} · r {score:+.3}",
        if change > 0.0 { "+" } else { "" },
        display_value(change, "s")
    )
}

fn graph(ui: &mut Ui, report: &Report, selected: Option<usize>) -> Option<usize> {
    let (area, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::click_and_drag());
    let rect = Rect::from_min_max(area.min + vec2(25.0, 3.0), area.max - vec2(3.0, 16.0));
    let painter = ui.painter_at(area);
    let ink = ui.visuals().weak_text_color();
    let focus = ui.visuals().selection.bg_fill;
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let last = report.points.len().saturating_sub(1).max(1);
    let x = |index: usize| rect.right() - rect.width() * (index as f32 / last as f32);
    let y = |coefficient: f64| rect.center().y - 0.5 * rect.height() * coefficient as f32;
    for coefficient in [-1.0, 0.0, 1.0] {
        let height = y(coefficient);
        painter.hline(
            rect.x_range(),
            height,
            Stroke::new(0.5, ink.gamma_multiply(0.45)),
        );
        painter.text(
            pos2(rect.left() - 3.0, height),
            Align2::RIGHT_CENTER,
            format!("{coefficient:.0}"),
            FontId::monospace(10.0),
            ink,
        );
    }
    let mut path = Vec::new();
    let draw = |points: &mut Vec<egui::Pos2>| {
        if points.len() > 1 {
            painter.add(Shape::line(
                std::mem::take(points),
                Stroke::new(1.25, focus),
            ));
        } else if let Some(point) = points.pop() {
            painter.circle_filled(point, 2.0, focus);
        }
    };
    for point in &report.points {
        if let Some(score) = point.coefficient {
            path.push(pos2(x(point.start), y(score)));
        } else {
            draw(&mut path);
        }
    }
    draw(&mut path);
    if let Some(index) = selected {
        painter.vline(x(index), rect.y_range(), Stroke::new(1.0, Color32::GOLD));
    }
    let left = report.change(last);
    let right = report.change(0);
    for (position, align, time) in [
        (rect.left(), Align2::LEFT_TOP, left),
        (rect.right(), Align2::RIGHT_TOP, right),
    ] {
        painter.text(
            pos2(position, rect.bottom() + 2.0),
            align,
            display_value(time, "s"),
            FontId::monospace(10.0),
            ink,
        );
    }
    if response.clicked() || response.dragged() {
        let position = response.interact_pointer_pos()?.x;
        let index = ((rect.right() - position) / rect.width().max(1.0) * last as f32)
            .round()
            .clamp(0.0, last as f32) as usize;
        if report.points.get(index)?.coefficient.is_some() {
            return Some(index);
        }
    }
    None
}
