use super::{Band, Capture, Job, Trace, plot, read_job, time_domain::GATE_COLOR};
use crate::{
    file_manager::{preview_copy::Copies, signal_view::take_query_result},
    ui_appearance::WorkbenchColors,
};
use egui::{Align2, Color32, FontId, Painter, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    touchstone::{GroupDelayPoint, Index, TracePoint, group_delay},
    waveforms::display_value,
};
use std::{fmt::Write, sync::Arc, time::Duration};

pub(super) struct DelayView {
    radius: usize,
    task: Option<(Query, Job<Arc<Curves>>)>,
    accepted: Option<(Query, Result<Arc<Curves>, String>)>,
    next_request: f64,
}

pub(super) struct Inputs {
    pub trace: Trace,
    pub band: Band,
    pub gated: Option<Arc<Vec<TracePoint>>>,
    pub selected: Option<Band>,
}

#[derive(Clone)]
struct Query {
    trace: Trace,
    band: Band,
    radius: usize,
    gated: Option<Arc<Vec<TracePoint>>>,
}

impl PartialEq for Query {
    fn eq(&self, other: &Self) -> bool {
        self.trace == other.trace
            && self.band == other.band
            && self.radius == other.radius
            && match (&self.gated, &other.gated) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

struct Curves {
    measured: Vec<GroupDelayPoint>,
    gated: Vec<GroupDelayPoint>,
}

impl Default for DelayView {
    fn default() -> Self {
        Self {
            radius: 1,
            task: None,
            accepted: None,
            next_request: 0.0,
        }
    }
}

impl DelayView {
    pub fn pending(&self) -> bool {
        self.task.is_some()
    }

    pub fn suspend(&mut self) {
        self.task = None;
    }

    pub fn content(
        &mut self,
        ui: &mut Ui,
        capture: &Arc<Capture>,
        index: Option<&Arc<Index>>,
        input: Inputs,
        colors: WorkbenchColors,
        copies: &mut Copies,
    ) -> (Rect, Response) {
        let Inputs {
            trace,
            band,
            gated,
            selected,
        } = input;
        if let Some(result) = take_query_result(&mut self.task) {
            self.accepted = Some(result);
        }
        let mut key = Query {
            trace,
            band,
            radius: self.radius,
            gated,
        };
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Group delay");
            ui.add(egui::DragValue::new(&mut self.radius).range(1..=128).prefix("Aperture ± ").suffix(" points"))
                .on_hover_text("Uses unwrapped phase within the aperture. Adjacent measured phase steps must stay within 180°.");
            key.radius = self.radius;
            let available = self.result(&key).and_then(|result| result.as_ref().ok());
            let region = selected.map_or(band, |[a, b]| [a.min(b).max(band[0]), a.max(b).min(band[1])]);
            if ui.add_enabled(
                available.is_some() && !copies.busy() && region[0] <= region[1],
                egui::Button::new("Copy delay CSV"),
            )
                .on_hover_text("Copy the group delay shown in this view. Markers narrow the exported band.")
                .clicked()
                && let Some(curves) = available
            {
                let curves = curves.clone();
                copies.start(ui.ctx(), move |_| copy_delays(&curves, region));
            }
            if key.gated.is_some() {
                ui.colored_label(colors.accent, "Measured");
                ui.colored_label(GATE_COLOR, "Gated");
            }
        });
        if let Some(index) = index {
            self.request(ui, &key, capture, index);
        }
        let height = (ui.available_height() - 14.0).max(83.0);
        let (outer, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let left = (outer.width() / 4.0).min(64.0);
        let rect = Rect::from_min_max(outer.min + vec2(left, 3.0), outer.max - vec2(9.0, 20.0));
        let painter = ui.painter_at(outer);
        painter.rect_filled(rect, 3.0, colors.recessed);
        plot::axes(&painter, rect, band, colors.text_muted);
        if let Some(selected) = selected {
            plot::highlight(&painter, rect, band, selected, colors.accent);
        }
        match self.result(&key) {
            Some(Ok(curves)) => {
                curves.paint(&painter, rect, band, colors);
                if let Some(pointer) = response.hover_pos().filter(|point| rect.contains(*point)) {
                    curves.probe(ui, &painter, rect, band, pointer.x, colors);
                }
            }
            Some(Err(error)) => notice(&painter, rect, error, ui.visuals().error_fg_color),
            None => notice(
                &painter,
                rect,
                if index.is_some() {
                    "Calculating group delay…"
                } else {
                    "Indexing measurement sweep…"
                },
                colors.text_muted,
            ),
        }
        (rect, response)
    }

    fn result(&self, key: &Query) -> Option<&Result<Arc<Curves>, String>> {
        self.accepted
            .as_ref()
            .filter(|(ready, _)| ready == key)
            .map(|(_, result)| result)
    }

    fn request(&mut self, ui: &Ui, key: &Query, capture: &Arc<Capture>, index: &Arc<Index>) {
        if self
            .accepted
            .as_ref()
            .is_some_and(|(ready, _)| ready == key)
            || self.task.is_some()
        {
            return;
        }
        let now = ui.input(|input| input.time);
        if now < self.next_request {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.next_request - now));
            return;
        }
        self.next_request = now + 0.08;
        let requested = key.clone();
        let capture = capture.clone();
        let index = index.clone();
        self.task = Some((
            key.clone(),
            read_job(ui.ctx(), Duration::from_secs(20), move |cancel| {
                let points = capture.exact_band(
                    &index,
                    requested.band,
                    requested.trace,
                    requested.radius,
                    cancel,
                )?;
                let measured =
                    group_delay(&points, requested.radius).map_err(|error| error.to_string())?;
                let gated = match requested.gated {
                    Some(points) => {
                        group_delay(&points, requested.radius).map_err(|error| error.to_string())?
                    }
                    None => Vec::new(),
                };
                Ok(Arc::new(Curves { measured, gated }))
            }),
        ));
    }
}

fn notice(painter: &Painter, rect: Rect, text: &str, color: Color32) {
    painter.with_clip_rect(rect).text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(13.0),
        color,
    );
}

impl Curves {
    fn paint(&self, painter: &Painter, rect: Rect, band: Band, colors: WorkbenchColors) {
        let [measured, gated] = [
            visible(&self.measured, band, true),
            visible(&self.gated, band, true),
        ];
        let values = measured
            .iter()
            .chain(gated)
            .filter(|point| (band[0]..=band[1]).contains(&point.frequency_hz))
            .filter_map(|point| point.seconds);
        let Some((bounds, scale)) = limits(values) else {
            notice(
                painter,
                rect,
                "No continuous measured phase in this band",
                colors.text_muted,
            );
            return;
        };
        let ink = colors.text_muted;
        for tick in 0..=4 {
            let ratio = tick as f32 / 4.0;
            let y = rect.top() + rect.height() * ratio;
            let seconds =
                (plot::hz_at_fraction(bounds, 1.0 - ratio) * scale).clamp(-f64::MAX, f64::MAX);
            painter.hline(
                rect.x_range(),
                y,
                Stroke::new(0.7, ink.gamma_multiply(0.26)),
            );
            painter.text(
                pos2(
                    rect.left() - 4.0,
                    y.clamp(rect.top() + 5.0, rect.bottom() - 5.0),
                ),
                Align2::RIGHT_CENTER,
                display_value(seconds, "s"),
                FontId::monospace(10.0),
                ink,
            );
        }
        let clip = painter.with_clip_rect(rect);
        for (points, color) in [(measured, colors.accent), (gated, GATE_COLOR)] {
            let points = points.iter().map(|point| {
                (
                    point.frequency_hz,
                    point.seconds.map_or(f64::NAN, |seconds| seconds / scale),
                )
            });
            plot::paint_curve(&clip, rect, band, points, bounds, false, color);
        }
    }

    fn probe(
        &self,
        ui: &Ui,
        painter: &Painter,
        rect: Rect,
        band: Band,
        x: f32,
        colors: WorkbenchColors,
    ) {
        let frequency = plot::hz_at(rect, band, x);
        let Some(point) = nearest(visible(&self.measured, band, false), frequency) else {
            return;
        };
        let x = plot::x_at(rect, band, point.frequency_hz);
        painter.vline(x, rect.y_range(), Stroke::new(1.0, colors.focus));
        let Some((delay, aperture)) = point.seconds.zip(point.aperture_hz) else {
            return;
        };
        let mut text = format!(
            "{} · {} · aperture {}",
            display_value(point.frequency_hz, "Hz"),
            display_value(delay, "s"),
            display_value(aperture, "Hz")
        );
        let gated = visible(&self.gated, band, false);
        if let Some((first, last)) = gated.first().zip(gated.last())
            && (first.frequency_hz..=last.frequency_hz).contains(&point.frequency_hz)
            && let Some(other) = nearest(gated, point.frequency_hz).and_then(|point| point.seconds)
        {
            text.push_str(&format!("   Gated {}", display_value(other, "s")));
        }
        let galley = painter.layout_no_wrap(text, FontId::monospace(11.0), colors.text_primary);
        let x = (x + 4.0)
            .min(rect.right() - galley.size().x - 8.0)
            .max(rect.left());
        let label = Rect::from_min_size(pos2(x, rect.top() + 4.0), galley.size() + vec2(7.0, 5.0));
        let clip = painter.with_clip_rect(rect);
        clip.rect_filled(label, 3.0, ui.visuals().window_fill());
        clip.galley(label.min + vec2(3.0, 2.0), galley, Color32::PLACEHOLDER);
    }
}

fn visible(points: &[GroupDelayPoint], band: Band, padded: bool) -> &[GroupDelayPoint] {
    let start = points.partition_point(|point| point.frequency_hz < band[0]);
    let end = points.partition_point(|point| point.frequency_hz <= band[1]);
    let pad = usize::from(padded);
    &points[start.saturating_sub(pad)..(end + pad).min(points.len())]
}

fn nearest(points: &[GroupDelayPoint], hz: f64) -> Option<GroupDelayPoint> {
    let next = points.partition_point(|point| point.frequency_hz < hz);
    let previous = next.checked_sub(1).and_then(|index| points.get(index));
    match (previous, points.get(next)) {
        (Some(previous), Some(next)) if hz - previous.frequency_hz < next.frequency_hz - hz => {
            Some(*previous)
        }
        (_, Some(next)) => Some(*next),
        (previous, None) => previous.copied(),
    }
}

fn limits(values: impl Iterator<Item = f64>) -> Option<(Band, f64)> {
    let [low, high] = values.fold([f64::INFINITY, f64::NEG_INFINITY], |[low, high], value| {
        [low.min(value), high.max(value)]
    });
    if !low.is_finite() {
        return None;
    }
    let scale = low.abs().max(high.abs()).max(1e-13);
    let low = low / scale;
    let high = high / scale;
    let pad = ((high - low) * 0.06).max(1e-5);
    Some(([low - pad, high + pad], scale))
}

fn copy_delays(curves: &Curves, region: Band) -> Result<String, String> {
    let mut csv = String::from("trace,frequency_hz,delay_seconds,aperture_hz\n");
    let mut rows = 0;
    for (name, points) in [("measured", &curves.measured), ("gated", &curves.gated)] {
        for point in visible(points, region, false) {
            let delay = point
                .seconds
                .map_or_else(String::new, |value| value.to_string());
            let aperture = point
                .aperture_hz
                .map_or_else(String::new, |value| value.to_string());
            let _ = writeln!(csv, "{name},{},{delay},{aperture}", point.frequency_hz);
            rows += 1;
        }
    }
    if rows == 0 {
        return Err("No measured frequencies in this interval".into());
    }
    little_clip_core::copy_text_to_system_clipboard(&csv).map_err(|error| error.to_string())?;
    Ok(format!("Copied {rows} group delay records"))
}
