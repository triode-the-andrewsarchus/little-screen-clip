use super::{Capture, JOBS, activity, database};
use crate::{
    file_manager::{
        capture_browse::{Job, keyed_poll, start},
        preview_copy::Copies,
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Align2, FontId, Painter, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::can::{
    Channel, Identifier, Index,
    dbc::Database,
    series::{Point, Series},
};
use std::{fmt::Write, sync::Arc, time::Duration};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Signal {
    pub channel: Channel,
    pub id: Identifier,
    pub field: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct Query {
    signal: Signal,
    span: activity::Domain,
}

#[derive(Default)]
pub(super) struct Plot {
    primary: Option<Lane>,
    comparison: Option<Lane>,
    interval: Option<activity::Domain>,
}

struct Lane {
    signal: Signal,
    task: Option<(Query, Job<Arc<Series>>)>,
    accepted: Option<(Query, Result<Arc<Series>, String>)>,
}

pub(super) struct Viewport<'a> {
    pub span: &'a activity::Domain,
    pub selection: Option<activity::Domain>,
    pub selected_time: Option<i128>,
    pub origin: i128,
}

struct Chart<'a> {
    capture: &'a Arc<Capture>,
    database: &'a Arc<Database>,
    index: Option<&'a Arc<Index>>,
    span: &'a activity::Domain,
    selected_time: Option<i128>,
    origin: i128,
    colors: WorkbenchColors,
    paired: bool,
}

impl Plot {
    pub fn pending(&self) -> bool {
        [&self.primary, &self.comparison]
            .into_iter()
            .flatten()
            .any(|lane| lane.task.is_some())
    }

    pub fn active(&self) -> bool {
        self.primary.is_some()
    }

    pub fn field_actions(&mut self, ui: &mut Ui, signal: Signal) {
        let primary = self
            .primary
            .as_ref()
            .is_some_and(|lane| lane.signal == signal);
        let comparison = self
            .comparison
            .as_ref()
            .is_some_and(|lane| lane.signal == signal);
        if ui
            .add_enabled(!primary, egui::Button::new("Plot signal").small())
            .on_hover_text("Start a plot for this decoded field.")
            .clicked()
        {
            *self = Self {
                primary: Some(Lane::new(signal.clone())),
                ..Self::default()
            };
            return;
        }
        if self.active()
            && ui
                .add_enabled(
                    !primary && !comparison,
                    egui::Button::new("Compare signal").small(),
                )
                .on_hover_text("Show this decoded field on the same time axis.")
                .clicked()
        {
            self.comparison = Some(Lane::new(signal));
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        capture: &Arc<Capture>,
        database: &Arc<Database>,
        index: Option<&Arc<Index>>,
        viewport: Viewport<'_>,
        colors: WorkbenchColors,
        copies: &mut Copies,
    ) -> Option<u64> {
        if !self.active() {
            return None;
        }
        if !self.toolbar(ui, viewport.selection) {
            *self = Self::default();
            return None;
        }
        let chart = Chart {
            capture,
            database,
            index,
            span: self.interval.as_ref().unwrap_or(viewport.span),
            selected_time: viewport.selected_time,
            origin: viewport.origin,
            colors,
            paired: self.comparison.is_some(),
        };
        let row = self.primary.as_mut()?.show(ui, &chart, copies);
        let comparison_row = self
            .comparison
            .as_mut()
            .and_then(|lane| lane.show(ui, &chart, copies));
        let (axis, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
        paint_time_axis(
            &ui.painter_at(axis),
            data_rect(axis),
            chart.span,
            chart.origin,
            colors,
        );
        row.or(comparison_row)
    }

    fn toolbar(&mut self, ui: &mut Ui, selection: Option<activity::Domain>) -> bool {
        ui.horizontal_wrapped(|ui| {
            let capable = selection
                .as_ref()
                .is_some_and(|span| self.interval.as_ref() != Some(span));
            if ui
                .add_enabled(capable, egui::Button::new("Use selected time").small())
                .clicked()
            {
                self.interval = selection;
            }
            if ui
                .add_enabled(
                    self.interval.is_some(),
                    egui::Button::new("Follow timeline").small(),
                )
                .clicked()
            {
                self.interval = None;
            }
            if self.comparison.is_some() && ui.small_button("Remove comparison").clicked() {
                self.comparison = None;
            }
            !ui.small_button("Close plot").clicked()
        })
        .inner
    }
}

impl Lane {
    fn new(signal: Signal) -> Self {
        Self {
            signal,
            task: None,
            accepted: None,
        }
    }

    fn show(&mut self, ui: &mut Ui, chart: &Chart<'_>, copies: &mut Copies) -> Option<u64> {
        let field = chart
            .database
            .message(self.signal.id)?
            .signals
            .get(self.signal.field)?;
        if let Some(ready) = keyed_poll(&mut self.task) {
            self.accepted = Some(ready);
        }
        let key = Query {
            signal: self.signal.clone(),
            span: chart.span.clone(),
        };
        if let Some(index) = chart.index {
            self.request(ui, &key, chart.capture, chart.database, index);
        }
        let result = self
            .accepted
            .as_ref()
            .filter(|(ready, _)| ready == &key)
            .map(|(_, result)| result);
        ui.horizontal_wrapped(|ui| {
            ui.strong(if field.unit.is_empty() {
                field.name.clone()
            } else {
                format!("{} ({})", field.name, field.unit)
            });
            ui.weak(format!("{} · {}", self.signal.channel, self.signal.id));
            match result {
                Some(Ok(series)) => series_actions(ui, series, &field.unit, copies),
                Some(Err(error)) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                None => {
                    ui.weak(if chart.index.is_some() {
                        "Reading decoded values…"
                    } else {
                        "Waiting for the CAN index…"
                    });
                }
            }
        });
        let height = if chart.paired { 45.0 } else { 72.0 };
        let (area, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let rect = data_rect(area).shrink2(vec2(0.0, 2.0));
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, 3.0, chart.colors.recessed);
        for tick in 1..4 {
            let x = rect.left() + rect.width() * tick as f32 / 4.0;
            painter.vline(
                x,
                rect.y_range(),
                Stroke::new(0.5, chart.colors.text_muted.gamma_multiply(0.35)),
            );
        }
        let Some(Ok(series)) = result else {
            return None;
        };
        paint_series(
            &painter,
            rect,
            chart.span,
            series,
            &field.unit,
            chart.colors,
        );
        if let Some(time) = chart.selected_time {
            activity::marker(&painter, rect, chart.span, time, chart.colors);
        }
        let hover = response.hover_pos().filter(|pos| rect.contains(*pos));
        let click = response
            .interact_pointer_pos()
            .filter(|pos| rect.contains(*pos));
        let point_at =
            |pos: egui::Pos2| nearest_point(&series.points, activity::at(rect, chart.span, pos.x));
        let clicked = response
            .clicked()
            .then(|| click.and_then(point_at))
            .flatten();
        if let Some(point) = hover.and_then(point_at) {
            activity::marker(&painter, rect, chart.span, point.time_ns, chart.colors);
            response.on_hover_text(tooltip(point, &field.unit, chart.origin));
        }
        clicked.map(|point| point.row)
    }

    fn request(
        &mut self,
        ui: &Ui,
        key: &Query,
        capture: &Arc<Capture>,
        database: &Arc<Database>,
        index: &Arc<Index>,
    ) {
        if self
            .accepted
            .as_ref()
            .is_some_and(|(ready, _)| ready == key)
            || self
                .task
                .as_ref()
                .is_some_and(|(loading, _)| loading == key)
        {
            return;
        }
        let requested = key.clone();
        let capture = capture.clone();
        let database = database.clone();
        let index = index.clone();
        self.task = Some((
            key.clone(),
            start(&JOBS, ui.ctx(), Duration::from_secs(30), move |cancel| {
                capture
                    .signal_series(&index, &database, &requested.signal, requested.span, cancel)
                    .map(Arc::new)
            }),
        ));
    }
}

fn data_rect(area: Rect) -> Rect {
    Rect::from_min_max(area.min + vec2(76.0, 0.0), area.max - vec2(4.0, 0.0))
}

fn paint_time_axis(
    painter: &Painter,
    rect: Rect,
    span: &activity::Domain,
    origin: i128,
    colors: WorkbenchColors,
) {
    let divisions: i128 = if rect.width() >= 460.0 { 4 } else { 1 };
    let width = span.end.saturating_sub(1) - span.start;
    for tick in 0..=divisions {
        let time = span.start + width * tick / divisions;
        let x = rect.left() + rect.width() * tick as f32 / divisions as f32;
        let align = match tick {
            0 => Align2::LEFT_TOP,
            tick if tick == divisions => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        painter.text(
            pos2(x, rect.top()),
            align,
            activity::time_label(time - origin),
            FontId::monospace(10.0),
            colors.text_muted,
        );
    }
}

fn nearest_point(points: &[Point], time: i128) -> Option<Point> {
    let next = points.partition_point(|point| point.time_ns < time);
    let before = next.checked_sub(1).and_then(|index| points.get(index));
    match (before, points.get(next)) {
        (Some(before), Some(after)) if time - before.time_ns < after.time_ns - time => {
            Some(*before)
        }
        (_, Some(after)) => Some(*after),
        (before, None) => before.copied(),
    }
}

fn tooltip(point: Point, unit: &str, origin: i128) -> String {
    let value = point.decoded.map_or_else(
        || "No value in this frame".into(),
        |reading| database::physical(reading.value, unit),
    );
    format!(
        "{} · Frame {} · {value}",
        activity::time_label(point.time_ns - origin),
        point.row + 1
    )
}

fn paint_series(
    painter: &Painter,
    rect: Rect,
    span: &activity::Domain,
    series: &Series,
    unit: &str,
    colors: WorkbenchColors,
) {
    let values = series
        .points
        .iter()
        .filter_map(|point| point.decoded.map(|reading| reading.value));
    let Some((low, high)) = values
        .map(|value| (value, value))
        .reduce(|(low, high), (min, max)| (low.min(min), high.max(max)))
    else {
        return;
    };
    let scale = low.abs().max(high.abs()).max(f64::MIN_POSITIVE);
    let floor = low / scale;
    let range = high / scale - floor;
    let font = FontId::monospace(10.0);
    let ticks: &[f64] = if high == low {
        &[0.5]
    } else {
        &[0.0, 0.5, 1.0]
    };
    for &tick in ticks {
        let y = rect.bottom() - rect.height() * tick as f32;
        painter.hline(
            rect.x_range(),
            y,
            Stroke::new(0.5, colors.text_muted.gamma_multiply(0.35)),
        );
        painter.text(
            pos2(rect.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            database::physical(range.mul_add(tick, floor) * scale, unit),
            font.clone(),
            colors.text_muted,
        );
    }
    let data_rect = rect.shrink2(vec2(0.0, 3.0));
    let plot = painter.with_clip_rect(rect);
    let mut points: Vec<egui::Pos2> = Vec::new();
    let mut dots = egui::Mesh::default();
    for point in &series.points {
        if let Some(value) = point.decoded {
            let fraction = if high == low {
                0.5
            } else {
                (value.value / scale - floor) / range
            };
            let y = data_rect.bottom() - data_rect.height() * fraction as f32;
            let x = activity::x(rect, span, point.time_ns);
            points.push(pos2(x, y));
        } else {
            paint_points(&plot, &mut points, &mut dots, colors.accent, series.reduced);
        }
    }
    paint_points(&plot, &mut points, &mut dots, colors.accent, series.reduced);
    if !dots.is_empty() {
        plot.add(dots);
    }
}

fn paint_points(
    painter: &Painter,
    points: &mut Vec<egui::Pos2>,
    dots: &mut egui::Mesh,
    color: egui::Color32,
    reduced: bool,
) {
    if reduced || points.len() <= 1 {
        for point in points.drain(..) {
            dots.add_colored_rect(Rect::from_center_size(point, vec2(2.6, 2.6)), color);
        }
    } else {
        let line = std::mem::take(points);
        if line.len() <= 160 {
            for &point in &line {
                painter.circle_filled(point, 1.7, color);
            }
        }
        painter.add(Shape::line(line, Stroke::new(1.1, color)));
    }
}

fn series_actions(ui: &mut Ui, series: &Arc<Series>, unit: &str, copies: &mut Copies) {
    ui.weak(format!(
        "{} decoded · {} frames scanned",
        series.observations, series.frames_read
    ));
    if series.reduced {
        ui.weak("Showing time-binned extrema · Zoom for individual values");
    }
    if series.truncated {
        ui.weak("Scan limit reached · Zoom into a time range");
    }
    let exact = !series.truncated && !series.reduced && series.observations > 0;
    if ui
        .add_enabled(
            exact && !copies.busy(),
            egui::Button::new("Copy signal CSV").small(),
        )
        .clicked()
    {
        let series = series.clone();
        let unit = unit.to_owned();
        copies.start(ui.ctx(), move |_| copy_values(&series, &unit));
    }
}

fn copy_values(series: &Series, unit: &str) -> Result<String, String> {
    let mut csv = String::from("frame,timestamp_ns,value,unit,raw\n");
    let unit = unit.replace('"', "\"\"");
    for point in &series.points {
        let row = point.row + 1;
        let timestamp = point.time_ns;
        if let Some(reading) = point.decoded {
            let raw = database::raw_value(reading.raw);
            let value = reading.value;
            let _ = writeln!(csv, "{row},{timestamp},{value},\"{unit}\",{raw}");
        } else {
            let _ = writeln!(csv, "{row},{timestamp},,\"{unit}\",");
        }
    }
    little_clip_core::copy_text_to_system_clipboard(&csv).map_err(|error| error.to_string())?;
    Ok(format!("Copied {} decoded values", series.observations))
}
