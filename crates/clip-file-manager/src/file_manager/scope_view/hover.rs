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
    sync::{Arc, LazyLock},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 1));
type Coordinates = Vec<Option<u64>>;
type Values = Vec<Option<f32>>;

#[derive(Default)]
pub(super) struct Hover {
    pub time: Option<f64>,
    loaded: Option<(Coordinates, Values)>,
    job: Option<(Coordinates, Task<Result<Values, String>>)>,
}

impl Hover {
    pub fn pending(&self) -> bool {
        self.job.is_some()
    }

    pub fn value(&self, index: usize, channel: &Channel, time: f64) -> Option<f32> {
        let (loaded, values) = self.loaded.as_ref()?;
        if loaded.get(index).copied().flatten() == coordinate(channel, time) {
            values.get(index).copied().flatten()
        } else {
            None
        }
    }

    pub fn update(&mut self, ctx: &Context, recording: &Arc<Recording>, cached: bool) {
        if let Some((target, result)) = take_query_result(&mut self.job) {
            self.loaded = Some((target, result.unwrap_or_default()));
        }
        let Some(time) = self.time.filter(|_| !cached && self.job.is_none()) else {
            return;
        };
        let target = recording
            .channels()
            .iter()
            .map(|channel| coordinate(channel, time))
            .collect::<Coordinates>();
        if self
            .loaded
            .as_ref()
            .is_some_and(|(coordinates, _)| *coordinates == target)
        {
            return;
        }
        let request = target.clone();
        let recording = recording.clone();
        let repaint = ctx.clone();
        let task = Task::start(&JOBS, Duration::from_secs(3), move |cancel| {
            let result = recording.checked(cancel, |capture| {
                request
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| {
                        row.map(|row| {
                            capture
                                .read_samples(index, row..row + 1, cancel)
                                .map(|values| values.first().copied())
                        })
                        .transpose()
                        .map(Option::flatten)
                    })
                    .collect()
            });
            repaint.request_repaint();
            result
        });
        self.job = Some((target, task));
    }
}

fn coordinate(channel: &Channel, time: f64) -> Option<u64> {
    let position = (time - channel.start_seconds) / channel.interval_seconds;
    let row = position.round() as u64;
    (position >= 0.0 && row < channel.sample_count).then_some(row)
}

impl Viewer {
    pub(super) fn update_hover(&mut self, ctx: &Context) {
        let cached = self.hover.time.is_some_and(|time| {
            self.overview
                .channels
                .iter()
                .enumerate()
                .all(|(index, channel)| {
                    coordinate(channel, time).is_none() || self.raw_value(index, time).is_some()
                })
        });
        self.hover.update(ctx, &self.recording, cached);
    }

    fn raw_value(&self, index: usize, time: f64) -> Option<f32> {
        self.detail
            .as_ref()
            .and_then(|(_, data)| data.raw(index))
            .and_then(|raw| raw.value_at(time))
            .or_else(|| {
                (self.measurement_key.as_ref()?.channel == index)
                    .then_some(&self.measurements.as_ref()?.raw)?
                    .value_at(time)
            })
    }

    pub(super) fn paint_hover(
        &self,
        painter: &Painter,
        rect: Rect,
        ink: Color32,
        fill: Color32,
        offset: f64,
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
                &display_value(time + offset, "s"),
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
                             value: f32,
                             name: &str,
                             unit: &str,
                             display_offset,
                             color| {
            if !bounds.valid() || !value.is_finite() {
                return;
            }
            let y = plot::y_at(lane, bounds, value);
            if !visible.y_range().contains(y) {
                return;
            }
            painter.circle_filled(pos2(x, y), 3.0, color);
            let measured = display_value(f64::from(value), unit);
            let text = if display_offset == 0.0 {
                format!("{name}  {measured}")
            } else {
                let displayed = display_value(f64::from(value + display_offset), unit);
                format!("{name}  {measured} raw · {displayed} shifted")
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
            let value = self
                .raw_value(index, time)
                .or_else(|| self.hover.value(index, channel, time));
            if let Some(value) = value {
                add_label(
                    (plot, bounds),
                    value,
                    &channel.name,
                    &channel.unit,
                    display_offset,
                    super::plot::color(index),
                );
            }
        }
        let math_slot = self.physical_lanes();
        if let Some((raw, bounds)) = self.calculated()
            && let Some(value) = raw.value_at(time)
        {
            add_label(
                (self.lane(rect, math_slot).plot, *bounds),
                value,
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
                value,
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
