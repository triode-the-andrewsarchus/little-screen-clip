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
        let visible = rect.intersect(painter.clip_rect());
        if !visible.is_positive() || !visible.x_range().contains(x) {
            return;
        }
        painter.vline(x, rect.y_range(), Stroke::new(1.0, ink));
        if visible.top() == rect.top() {
            selection_label(
                painter,
                visible,
                x,
                rect.top() + 5.0,
                &display_value(clock.to_workspace(time), "s"),
                ink,
                fill,
            );
        }
        let to_right = visible.right() - x >= x - visible.left();
        let available = if to_right {
            visible.right() - x
        } else {
            x - visible.left()
        };
        let text_width = (available - 19.0).max(1.0);
        let mut labels = Vec::new();
        let mut add_label = |(lane, bounds): (Rect, Peak),
                             probe: Probe,
                             name: &str,
                             unit: &str,
                             display_offset,
                             color| {
            let value = match probe {
                Probe::Value(value) if bounds.valid() => Some(value),
                _ => None,
            };
            let y = value.map_or(lane.center().y, |value| plot::y_at(lane, bounds, value));
            if !visible.y_range().contains(y) {
                return;
            }
            if value.is_some() {
                painter.circle_filled(pos2(x, y), 3.0, color);
            }
            let measured = probe.text(unit);
            let text = match value {
                Some(value) if display_offset != 0.0 => {
                    let displayed = display_value(f64::from(value + display_offset), unit);
                    format!("{name}  {measured} raw · {displayed} shifted")
                }
                _ => format!("{name}  {measured}"),
            };
            let mut job = LayoutJob::simple(text, FontId::monospace(12.0), color, text_width);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            labels.push((y, painter.layout_job(job)));
        };
        for &PlottedChannel {
            index,
            plot,
            bounds,
            display_offset,
        } in channels
        {
            let channel = &self.overview.channels[index];
            add_label(
                (plot, bounds),
                self.recorded_probe(index, time).1,
                &channel.name,
                &channel.unit,
                display_offset,
                super::plot::color(index),
            );
        }
        let math_slot = self.physical_lanes();
        if let Some((raw, bounds)) = self.calculated()
            && let Some(value) = raw.value_at(time)
        {
            add_label(
                (self.lane(rect, math_slot).plot, *bounds),
                Probe::from_sample(value),
                "Math",
                &self.math_unit(),
                0.0,
                super::MATH_COLOR,
            );
        }
        let filter_start = math_slot + usize::from(self.math_state.numeric().is_some());
        for (slot, index, name, bounds, value) in self.filters.probes(time) {
            let unit = &self.overview.channels[index].unit;
            add_label(
                (self.lane(rect, filter_start + slot).plot, bounds),
                Probe::from_sample(value),
                name,
                unit,
                0.0,
                super::filters::FILTER_COLOR,
            );
        }
        paint_probe_labels(painter, visible, x, to_right, fill, &mut labels);
    }
}

fn paint_probe_labels(
    painter: &Painter,
    area: Rect,
    x: f32,
    to_right: bool,
    fill: Color32,
    labels: &mut [(f32, Arc<Galley>)],
) {
    labels.sort_by(|first, second| first.0.total_cmp(&second.0));
    let panels = label_panels(
        labels.iter().map(|(y, label)| (*y, label.size().y)),
        area.y_range(),
    );
    for panel in panels {
        let rows = &labels[panel.rows];
        let width = rows
            .iter()
            .map(|(_, line)| line.size().x)
            .fold(0.0_f32, f32::max)
            + 12.0;
        let left = if to_right { x + 7.0 } else { x - width - 7.0 };
        let rect = Rect::from_min_size(pos2(left, panel.top), vec2(width, panel.height));
        painter.rect_filled(rect, 3.0, fill);
        let mut top = rect.top() + 3.0;
        for (_, line) in rows {
            painter.galley(pos2(left + 6.0, top), line.clone(), Color32::PLACEHOLDER);
            top += line.size().y;
        }
    }
}

struct LabelPanel {
    rows: std::ops::Range<usize>,
    top: f32,
    height: f32,
    first_y: f32,
}

fn label_panels(labels: impl Iterator<Item = (f32, f32)>, area: Rangef) -> Vec<LabelPanel> {
    const PADDING: f32 = 6.0;
    let mut panels: Vec<LabelPanel> = Vec::new();
    let top_at =
        |y: f32, height: f32| (y - height * 0.5).clamp(area.min, (area.max - height).max(area.min));
    for (index, (y, height)) in labels.enumerate() {
        let height = height + PADDING;
        let mut panel = LabelPanel {
            rows: index..index + 1,
            top: top_at(y, height),
            height,
            first_y: y,
        };
        while panels
            .last()
            .is_some_and(|prev| prev.top + prev.height > panel.top)
        {
            let prev = panels.pop().expect("overlapping panel");
            panel.height += prev.height - PADDING;
            panel.first_y = prev.first_y;
            panel.rows.start = prev.rows.start;
            panel.top = top_at((prev.first_y + y) * 0.5, panel.height);
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
