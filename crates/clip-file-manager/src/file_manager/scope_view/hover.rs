use super::{RawChannel, Viewer, plot};
use crate::file_manager::preview_job::Task;
use egui::{Align2, Color32, Context, FontId, Painter, Rect, Stroke, pos2, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        scope::Channel,
        waveforms::{Recording, display_value},
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
        if let Some(result) = self.job.as_ref().and_then(|(_, task)| task.poll()) {
            let (target, _) = self.job.take().expect("completed hover read");
            self.loaded = Some((target, result.ok().and_then(Result::ok).unwrap_or_default()));
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
            .or_else(|| {
                (self.measurement_key.as_ref()?.channel == index)
                    .then_some(&self.measurements.as_ref()?.raw)
            })
            .and_then(|raw: &RawChannel| raw.value_at(time))
    }

    pub(super) fn paint_hover(&self, painter: &Painter, rect: Rect, ink: Color32, fill: Color32) {
        let Some(time) = self.hover.time else { return };
        let x = plot::x_at(rect, &self.span, time);
        let mut labels = vec![(display_value(time, "s"), ink)];
        painter.vline(x, rect.y_range(), Stroke::new(1.0, ink));
        for (slot, index) in self.visible_channels().enumerate() {
            let channel = &self.overview.channels[index];
            let color = plot::color(index);
            let value = self
                .raw_value(index, time)
                .or_else(|| self.hover.value(index, channel, time));
            labels.push((
                format!(
                    "{}  {}",
                    channel.name,
                    display_value(value.map_or(f64::NAN, f64::from), &channel.unit)
                ),
                color,
            ));
            if let Some(value) = value.filter(|value| value.is_finite()) {
                let (lane, bounds) = self.channel_lane(rect, slot, index);
                if bounds.valid() {
                    painter.circle_filled(pos2(x, plot::y_at(lane, bounds, value)), 3.0, color);
                }
            }
        }
        if let Some((raw, _)) = self.calculated() {
            let value = raw.value_at(time).map_or(f64::NAN, f64::from);
            labels.push((
                format!("Math  {}", display_value(value, &self.math_unit())),
                super::MATH_COLOR,
            ));
        }
        let font = FontId::monospace(12.0);
        let laid_out = labels
            .into_iter()
            .map(|(text, color)| painter.layout_no_wrap(text, font.clone(), color))
            .collect::<Vec<_>>();
        let width = laid_out
            .iter()
            .map(|line| line.size().x)
            .fold(0.0_f32, f32::max)
            + 14.0;
        let height: f32 = laid_out.iter().map(|line| line.size().y).sum::<f32>() + 10.0;
        let left = if x + width + 8.0 <= rect.right() {
            x + 8.0
        } else {
            (x - width - 8.0).max(rect.left())
        };
        let panel = Rect::from_min_size(pos2(left, rect.top() + 6.0), vec2(width, height));
        painter.rect_filled(panel, 4.0, fill);
        let mut top = panel.top() + 5.0;
        for line in laid_out {
            let step = line.size().y;
            painter.galley(pos2(panel.left() + 7.0, top), line, Color32::PLACEHOLDER);
            top += step;
        }
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
