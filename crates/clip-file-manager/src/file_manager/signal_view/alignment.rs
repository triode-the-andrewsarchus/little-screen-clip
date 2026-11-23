mod measure;

use super::inputs::{Input, Signature};
use crate::file_manager::{
    capture_browse::{Job, poll, start},
    scope_view::plot::draw_raw,
};
use egui::{Align2, Color32, Context, FontId, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::Correlation,
        waveforms::{Peak, display_value},
    },
};
use measure::{Report, Request};
use std::{ops::Range, time::Duration};

#[derive(Clone)]
pub(super) struct Entry {
    pub source: u64,
    pub input: Input,
}

#[derive(Clone, Copy)]
pub(super) struct Suggestion {
    pub source: u64,
    pub base: f64,
    pub offset: f64,
}

#[derive(PartialEq)]
struct Key {
    reference: Signature,
    target: Signature,
    target_source: u64,
    base: f64,
    center: f64,
    duration: f64,
    search: f64,
}

#[derive(Default)]
pub(super) struct Alignment {
    pub open: bool,
    reference: Option<String>,
    target: Option<String>,
    center: f64,
    duration: f64,
    search: f64,
    pending: Option<Job<Report>>,
    result: Option<Result<Report, String>>,
    key: Option<Key>,
    picked: Option<usize>,
    before: bool,
}

impl Alignment {
    pub fn open_at(&mut self, visible: &Range<f64>, reference: Option<f64>) {
        let width = visible.end - visible.start;
        self.open = true;
        self.center = reference.unwrap_or_else(|| visible.start.midpoint(visible.end));
        self.duration = width * 0.25;
        self.search = width * 0.38;
        self.retire();
    }

    pub fn preview(&self) -> Option<Suggestion> {
        if self.before {
            return None;
        }
        self.suggestion()
    }

    fn suggestion(&self) -> Option<Suggestion> {
        let key = self.key.as_ref()?;
        let report = self.result.as_ref()?.as_ref().ok()?;
        let picked = self.picked?;
        Some(Suggestion {
            source: key.target_source,
            base: key.base,
            offset: key.base + report.change(picked),
        })
    }

    pub fn window(
        &mut self,
        ctx: &Context,
        entries: &[Entry],
        visible: &Range<f64>,
        reference: Option<f64>,
    ) -> Option<Suggestion> {
        if !self.open {
            return None;
        }
        if let Some(result) = poll(&mut self.pending) {
            self.result = Some(result);
        }
        let mut open = true;
        let mut apply = None;
        egui::Window::new("Match recording timing")
            .id(egui::Id::unique("signal waveform alignment"))
            .default_width(495.0)
            .vscroll(ctx.content_rect().height() < 640.0)
            .open(&mut open)
            .show(ctx, |ui| {
                self.pick_sources(ui, entries);
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
                self.results(ui);
                ui.horizontal(|ui| {
                    let candidate = self.suggestion();
                    if ui
                        .add_enabled(candidate.is_some(), egui::Button::new("Apply offset"))
                        .clicked()
                    {
                        apply = candidate;
                    }
                    if ui.button("Close").clicked() {
                        self.open = false;
                    }
                });
            });
        self.open &= open && apply.is_none();
        if !self.open {
            self.retire();
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(24));
        }
        apply
    }

    fn pick_sources(&mut self, ui: &mut Ui, entries: &[Entry]) {
        if self
            .reference
            .as_ref()
            .is_none_or(|id| !entries.iter().any(|entry| &entry.input.alias == id))
        {
            self.reference = entries.first().map(|entry| entry.input.alias.clone());
        }
        let reference_source = entries
            .iter()
            .find(|entry| Some(&entry.input.alias) == self.reference.as_ref())
            .map(|entry| entry.source);
        if self.target.as_ref().is_none_or(|id| {
            !entries
                .iter()
                .any(|entry| &entry.input.alias == id && Some(entry.source) != reference_source)
        }) {
            self.target = entries
                .iter()
                .find(|entry| Some(entry.source) != reference_source)
                .map(|entry| entry.input.alias.clone());
        }
        for (label, current, excluded) in [
            ("Reference", &mut self.reference, None),
            ("Target", &mut self.target, reference_source),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                let text = entries
                    .iter()
                    .find(|entry| current.as_ref() == Some(&entry.input.alias))
                    .map_or("Choose a lane", |entry| entry.input.label.as_str());
                egui::ComboBox::from_id_salt(label)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for entry in entries
                            .iter()
                            .filter(|entry| Some(entry.source) != excluded)
                        {
                            ui.selectable_value(
                                current,
                                Some(entry.input.alias.clone()),
                                &entry.input.label,
                            );
                        }
                    });
            });
        }
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
        });
        ui.horizontal_wrapped(|ui| {
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
            target_source: target.source,
            base: target.input.offset(),
            center: self.center,
            duration: self.duration,
            search: self.search,
        }
    }

    fn retire(&mut self) {
        self.key = None;
        self.pending = None;
        self.result = None;
        self.picked = None;
        self.before = false;
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

    fn results(&mut self, ui: &mut Ui) {
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
        let mut choice = graph(ui, report, self.picked);
        if report.peaks.is_empty() {
            ui.weak("No matching variation with enough overlapping samples");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for candidate in &report.peaks {
                if ui
                    .selectable_label(
                        self.picked == Some(candidate.start),
                        candidate_label(report, *candidate),
                    )
                    .clicked()
                {
                    choice = Some(candidate.start);
                }
            }
        });
        if let Some(choice) = choice {
            self.picked = Some(choice);
            self.before = false;
        }
        if let Some(picked) = self.picked.and_then(|pick| report.points.get(pick)) {
            ui.horizontal_wrapped(|ui| {
                ui.monospace(format!(
                    "{} · {} sample pairs",
                    candidate_label(report, *picked),
                    picked.overlap
                ));
                ui.toggle_value(&mut self.before, "Show before");
            });
        }
        let compare = if self.before {
            report.radius
        } else {
            self.picked.unwrap_or(report.radius)
        };
        compare_traces(ui, report, compare);
    }
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
