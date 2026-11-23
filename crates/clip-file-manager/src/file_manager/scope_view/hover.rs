use super::{PlottedChannel, Viewer, plot};
use crate::file_manager::{preview_job::Task, signal_view::take_query_result};
use egui::{
    Align2, Color32, Context, FontId, Galley, Painter, Rangef, Rect, Stroke, pos2, text::LayoutJob,
    vec2,
};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        scope::Channel,
        waveforms::{Peak, Recording, display_value},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, LazyLock},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 1));
type Address = (usize, u64);
type Samples = BTreeMap<Address, Option<f32>>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Probe {
    Value(f32),
    Gap,
    Outside,
    Pending,
    Unavailable,
}

impl Probe {
    fn from_sample(value: f32) -> Self {
        if value.is_finite() {
            Self::Value(value)
        } else {
            Self::Gap
        }
    }

    pub fn text(self, unit: &str) -> String {
        match self {
            Self::Value(value) => display_value(f64::from(value), unit),
            Self::Gap => "No sample".into(),
            Self::Outside => "Outside recording".into(),
            Self::Pending => "Reading…".into(),
            Self::Unavailable => "Sample unavailable".into(),
        }
    }
}

#[derive(Default)]
pub(super) struct Hover {
    pub time: Option<f64>,
    samples: Samples,
    job: Option<(BTreeSet<Address>, Task<Result<Samples, String>>)>,
}

impl Hover {
    pub fn pending(&self) -> bool {
        self.job.is_some()
    }

    fn value(&self, address: Address) -> Probe {
        match self.samples.get(&address) {
            Some(Some(value)) => Probe::from_sample(*value),
            Some(None) => Probe::Unavailable,
            None => Probe::Pending,
        }
    }

    fn update(&mut self, ctx: &Context, recording: &Arc<Recording>, wanted: &BTreeSet<Address>) {
        if let Some((requested, result)) = take_query_result(&mut self.job) {
            self.samples.extend(
                result.unwrap_or_else(|_| requested.into_iter().map(|row| (row, None)).collect()),
            );
        }
        self.samples.retain(|row, _| wanted.contains(row));
        let needed = wanted
            .iter()
            .copied()
            .filter(|row| !self.samples.contains_key(row))
            .collect::<BTreeSet<_>>();
        if self.job.is_some() || needed.is_empty() {
            return;
        }
        let request = needed.clone();
        let recording = recording.clone();
        let repaint = ctx.clone();
        let task = Task::start(&JOBS, Duration::from_secs(3), move |cancel| {
            let result = recording.checked(cancel, |capture| {
                // A failed channel read should leave the other channels readable.
                Ok(request
                    .into_iter()
                    .map(|address @ (index, row)| {
                        let value = capture
                            .read_samples(index, row..row + 1, cancel)
                            .ok()
                            .and_then(|samples| samples.first().copied());
                        (address, value)
                    })
                    .collect())
            });
            repaint.request_repaint();
            result
        });
        self.job = Some((needed, task));
    }
}

fn coordinate(channel: &Channel, time: f64) -> Option<u64> {
    if channel.sample_count == 0
        || !(channel.start_seconds..channel.time_at(channel.sample_count)).contains(&time)
    {
        return None;
    }
    let position = (time - channel.start_seconds) / channel.interval_seconds;
    Some((position.round() as u64).min(channel.sample_count - 1))
}

impl Viewer {
    pub(super) fn update_hover(&mut self, ctx: &Context) {
        let pointed = self
            .hover
            .time
            .into_iter()
            .flat_map(|time| self.visible_channels().map(move |index| (index, time)));
        let cursors = self
            .cursors
            .into_iter()
            .flatten()
            .map(|time| (self.active, time));
        let wanted = pointed
            .chain(cursors)
            .filter_map(|(index, time)| {
                Some((index, coordinate(&self.overview.channels[index], time)?))
            })
            .filter(|&(index, row)| self.raw_value(index, row).is_none())
            .collect();
        self.hover.update(ctx, &self.recording, &wanted);
    }

    pub(super) fn recorded_probe(&self, index: usize, time: f64) -> (Option<u64>, Probe) {
        let Some(row) = coordinate(&self.overview.channels[index], time) else {
            return (None, Probe::Outside);
        };
        let value = self
            .raw_value(index, row)
            .map_or_else(|| self.hover.value((index, row)), Probe::from_sample);
        (Some(row), value)
    }

    fn raw_value(&self, index: usize, row: u64) -> Option<f32> {
        self.detail
            .as_ref()
            .and_then(|(_, data)| data.raw(index))
            .and_then(|raw| raw.sample_at(row))
            .or_else(|| {
                (self.measurement_key.as_ref()?.channel == index)
                    .then_some(&self.measurements.as_ref()?.raw)?
                    .sample_at(row)
            })
    }

    pub(super) fn paint_hover(
        &self,
        painter: &Painter,
        rect: Rect,
        ink: Color32,
        fill: Color32,
        clock: little_clip_core::engineering::analysis::Clock,
        channels: &[PlottedChannel],
    ) {
        let Some(time) = self.hover.time else { return };
        let x = plot::x_at(rect, &self.span, time);
        let Some(layout) = ProbeLayout::new(painter, rect, x) else {
            return;
        };
        painter.vline(x, rect.y_range(), Stroke::new(1.0, ink));
        if layout.visible.top() == rect.top() {
            selection_label(
                painter,
                layout.visible,
                x,
                rect.top() + 5.0,
                &display_value(clock.to_workspace(time), "s"),
                ink,
                fill,
            );
        }
        let mut labels = Vec::new();
        for channel in channels {
            labels.extend(layout.label(painter, self.recorded_label(channel, time, true), None));
        }
        let math_slot = self.physical_lanes();
        if let Some((raw, bounds)) = self.calculated()
            && let Some(value) = raw.value_at(time)
        {
            labels.extend(layout.label(
                painter,
                ProbeReading {
                    plot: self.lane(rect, math_slot).plot,
                    bounds: *bounds,
                    value: Probe::from_sample(value),
                    name: "Math",
                    unit: &self.math_unit(),
                    offset: 0.0,
                    color: super::MATH_COLOR,
                },
                None,
            ));
        }
        let filter_start = math_slot + usize::from(self.math_state.numeric().is_some());
        for (slot, index, name, bounds, value) in self.filters.probes(time) {
            labels.extend(layout.label(
                painter,
                ProbeReading {
                    plot: self.lane(rect, filter_start + slot).plot,
                    bounds,
                    value: Probe::from_sample(value),
                    name,
                    unit: &self.overview.channels[index].unit,
                    offset: 0.0,
                    color: super::filters::FILTER_COLOR,
                },
                None,
            ));
        }
        layout.paint(painter, fill, &mut labels);
    }

    pub(super) fn paint_lane_probe(&self, painter: &Painter, channel: PlottedChannel) {
        let Some(time) = self.hover.time.filter(|time| self.span.contains(time)) else {
            return;
        };
        let x = plot::x_at(channel.plot, &self.span, time);
        let Some(layout) = ProbeLayout::new(painter, channel.plot, x) else {
            return;
        };
        let painter = painter.with_clip_rect(layout.visible);
        let visuals = &painter.ctx().global_style().visuals;
        painter.vline(
            x,
            layout.visible.y_range(),
            Stroke::new(1.0, visuals.text_color()),
        );
        if let Some(label) = layout.label(
            &painter,
            self.recorded_label(&channel, time, false),
            Some(visuals.text_color()),
        ) {
            let height = label.1.size().y + PANEL_PADDING;
            let top = panel_top(label.0, height, layout.visible.y_range());
            layout.paint_panel(&painter, visuals.window_fill(), &[label], top, height);
        }
    }

    fn recorded_label<'a>(
        &'a self,
        plotted: &PlottedChannel,
        time: f64,
        name: bool,
    ) -> ProbeReading<'a> {
        let index = plotted.index;
        let channel = &self.overview.channels[index];
        ProbeReading {
            plot: plotted.plot,
            bounds: plotted.bounds,
            value: self.recorded_probe(index, time).1,
            name: if name { &channel.name } else { "" },
            unit: &channel.unit,
            offset: plotted.display_offset,
            color: plot::color(index),
        }
    }
}

struct ProbeReading<'a> {
    plot: Rect,
    bounds: Peak,
    value: Probe,
    name: &'a str,
    unit: &'a str,
    offset: f32,
    color: Color32,
}

impl ProbeReading<'_> {
    fn caption(&self) -> String {
        let mut caption = if self.name.is_empty() {
            self.value.text(self.unit)
        } else {
            format!("{}  {}", self.name, self.value.text(self.unit))
        };
        if self.offset != 0.0
            && let Probe::Value(value) = self.value
        {
            let displayed = display_value(f64::from(value) + f64::from(self.offset), self.unit);
            caption.push_str(&format!(" raw · {displayed} shifted"));
        }
        caption
    }
}

struct ProbeLayout {
    visible: Rect,
    x: f32,
    to_right: bool,
    text_width: f32,
}

const PANEL_PADDING: f32 = 6.0;

impl ProbeLayout {
    fn new(painter: &Painter, rect: Rect, x: f32) -> Option<Self> {
        let visible = rect.intersect(painter.clip_rect());
        if !visible.is_positive() || !visible.x_range().contains(x) {
            return None;
        }
        let left = x - visible.left();
        let right = visible.right() - x;
        Some(Self {
            visible,
            x,
            to_right: right >= left,
            text_width: (left.max(right) - 19.0).max(1.0),
        })
    }

    fn label(
        &self,
        painter: &Painter,
        reading: ProbeReading<'_>,
        foreground: Option<Color32>,
    ) -> Option<(f32, Arc<Galley>)> {
        let intersection = match reading.value {
            Probe::Value(value) if reading.bounds.valid() => {
                Some(plot::y_at(reading.plot, reading.bounds, value))
            }
            _ => None,
        };
        let y = intersection.unwrap_or_else(|| reading.plot.center().y);
        if !self.visible.y_range().contains(y) {
            return None;
        }
        if intersection.is_some() {
            painter.circle_filled(pos2(self.x, y), 3.0, reading.color);
        }
        let mut job = LayoutJob::simple(
            reading.caption(),
            FontId::monospace(12.0),
            foreground.unwrap_or(reading.color),
            self.text_width,
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        Some((y, painter.layout_job(job)))
    }

    fn paint(&self, painter: &Painter, fill: Color32, labels: &mut [(f32, Arc<Galley>)]) {
        labels.sort_by(|first, second| first.0.total_cmp(&second.0));
        let panels = label_panels(
            labels.iter().map(|(y, label)| (*y, label.size().y)),
            self.visible.y_range(),
        );
        for panel in panels {
            self.paint_panel(painter, fill, &labels[panel.rows], panel.top, panel.height);
        }
    }

    fn paint_panel(
        &self,
        painter: &Painter,
        fill: Color32,
        rows: &[(f32, Arc<Galley>)],
        top: f32,
        height: f32,
    ) {
        let width = rows
            .iter()
            .map(|(_, line)| line.size().x)
            .fold(0.0_f32, f32::max)
            + 12.0;
        let left = if self.to_right {
            self.x + 7.0
        } else {
            self.x - width - 7.0
        };
        let rect = Rect::from_min_size(pos2(left, top), vec2(width, height));
        painter.rect_filled(rect, 3.0, fill);
        let mut top = rect.top() + 3.0;
        for (_, line) in rows {
            painter.galley(pos2(left + 6.0, top), line.clone(), Color32::PLACEHOLDER);
            top += line.size().y;
        }
    }
}

fn panel_top(y: f32, height: f32, area: Rangef) -> f32 {
    (y - height * 0.5).clamp(area.min, (area.max - height).max(area.min))
}

struct LabelPanel {
    rows: std::ops::Range<usize>,
    top: f32,
    height: f32,
    first_y: f32,
}

fn label_panels(labels: impl Iterator<Item = (f32, f32)>, area: Rangef) -> Vec<LabelPanel> {
    let mut panels: Vec<LabelPanel> = Vec::new();
    for (index, (y, height)) in labels.enumerate() {
        let height = height + PANEL_PADDING;
        let mut panel = LabelPanel {
            rows: index..index + 1,
            top: panel_top(y, height, area),
            height,
            first_y: y,
        };
        while panels
            .last()
            .is_some_and(|prev| prev.top + prev.height > panel.top)
        {
            let prev = panels.pop().expect("overlapping panel");
            panel.height += prev.height - PANEL_PADDING;
            panel.first_y = prev.first_y;
            panel.rows.start = prev.rows.start;
            panel.top = panel_top((prev.first_y + y) * 0.5, panel.height, area);
        }
        panels.push(panel);
    }
    panels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_zero_failed_and_unread_addresses_stay_distinct() {
        let hover = Hover {
            samples: BTreeMap::from([
                ((0, 100), Some(f32::NAN)),
                ((1, 100), Some(0.0)),
                ((2, 100), None),
            ]),
            ..Default::default()
        };
        assert_eq!(hover.value((0, 100)), Probe::Gap);
        assert_eq!(hover.value((1, 100)), Probe::Value(0.0));
        assert_eq!(hover.value((2, 100)), Probe::Unavailable);
        assert_eq!(hover.value((1, 101)), Probe::Pending);
        assert_eq!(Probe::Value(0.0).text("A"), "0 A");
    }

    #[test]
    fn probe_rows_are_native_to_each_channel_and_stop_at_its_coverage() {
        let channel = Channel {
            name: "CH1".into(),
            unit: "V".into(),
            sample_count: 4,
            start_seconds: -0.5,
            interval_seconds: 0.125,
        };
        assert_eq!(coordinate(&channel, -0.51), None);
        assert_eq!(coordinate(&channel, -0.5), Some(0));
        assert_eq!(coordinate(&channel, -0.26), Some(2));
        assert_eq!(coordinate(&channel, -0.01), Some(3));
        assert_eq!(coordinate(&channel, 0.0), None);
    }

    #[test]
    fn shifted_probe_reports_original_units_and_preserves_missing_sample_state() {
        let reading = |value, offset| ProbeReading {
            plot: Rect::NOTHING,
            bounds: Peak::default(),
            value,
            name: "",
            unit: "V",
            offset,
            color: Color32::WHITE,
        };
        assert_eq!(
            reading(Probe::Value(1.0), 0.5).caption(),
            "1 V raw · 1.5 V shifted"
        );
        assert_eq!(reading(Probe::Value(1.0), 0.0).caption(), "1 V");
        assert_eq!(reading(Probe::Gap, 0.5).caption(), "No sample");
    }

    #[test]
    fn probe_panels_merge_overlapping_labels_and_keep_separate_intersections() {
        let panels = label_panels(
            [(15.0, 12.0), (28.0, 12.0), (96.0, 12.0)].into_iter(),
            Rangef::new(0.0, 100.0),
        );
        assert_eq!(
            panels
                .iter()
                .map(|panel| panel.rows.clone())
                .collect::<Vec<_>>(),
            [0..2, 2..3]
        );
        assert_eq!(panels[1].top + panels[1].height, 100.0);
        assert!(panels[0].top + panels[0].height < panels[1].top);
    }
}

pub(super) fn selection_label(
    painter: &Painter,
    rect: Rect,
    x: f32,
    y: f32,
    label: &str,
    ink: Color32,
    fill: Color32,
) {
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::monospace(11.0), ink);
    let size = galley.size() + vec2(10.0, 4.0);
    let left = (x + 4.0).min(rect.right() - size.x).max(rect.left());
    let panel = Rect::from_min_size(pos2(left, y), size);
    painter.rect_filled(panel, 3.0, fill);
    painter.galley(
        Align2::CENTER_CENTER
            .align_size_within_rect(galley.size(), panel)
            .min,
        galley,
        ink,
    );
}
