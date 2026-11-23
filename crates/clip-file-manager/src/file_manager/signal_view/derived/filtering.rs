use super::compute::{Frame, Lane, analog};
use crate::file_manager::{
    preview_job::Task,
    scope_view::{plot, reads::RawChannel},
    signal_view::{
        filter_editor::{StageList, filter_menu},
        take_query_result,
    },
};
use egui::{Context, Rect, Response, Ui};
use little_clip_core::{
    BackgroundJobs,
    engineering::{analysis::FilterKind, scope::Channel, waveforms::sample_span},
};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Default)]
pub(super) struct Filters {
    pub settings: BTreeMap<usize, Settings>,
    task: Option<(Arc<Vec<Request>>, Task<Result<Vec<Output>, String>>)>,
    accepted: Option<(Arc<Vec<Request>>, Result<Vec<Output>, String>)>,
    retry_at: f64,
    anchor: Option<f64>,
}

#[derive(Default)]
pub(super) struct Settings {
    pub stages: StageList,
    pub interval: Option<Range<f64>>,
}

impl Settings {
    fn rows(&self, channel: &Channel, extent: &Range<f64>) -> Range<usize> {
        let span = self.interval.as_ref().unwrap_or(extent);
        let limits = sample_span(channel, span.clone());
        limits.start as usize..limits.end as usize
    }
}

pub(super) struct Output {
    pub index: usize,
    pub lane: Lane,
}

pub(super) struct PlotLane<'a> {
    pub name: String,
    pub values: Option<&'a Lane>,
}

struct Request {
    index: usize,
    name: String,
    unit: String,
    raw: Arc<RawChannel>,
    rows: Range<usize>,
    stages: StageList,
}

impl PartialEq for Request {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
            && Arc::ptr_eq(&self.raw, &other.raw)
            && self.rows == other.rows
            && self.stages == other.stages
    }
}

impl Filters {
    pub fn remap(&mut self, lanes: &[Option<usize>]) {
        self.settings = std::mem::take(&mut self.settings)
            .into_iter()
            .filter_map(|(index, settings)| Some((lanes.get(index).copied().flatten()?, settings)))
            .collect();
        self.task = None;
        self.accepted = None;
    }

    pub fn update(&mut self, ctx: &Context, frame: &Frame) {
        if let Some(result) = take_query_result(&mut self.task) {
            self.accepted = Some(result);
        }
        let request = self.requests(frame);
        if self
            .accepted
            .as_ref()
            .is_some_and(|(key, _)| key.as_ref() == &request)
        {
            self.task = None;
            return;
        }
        if self
            .task
            .as_ref()
            .is_some_and(|(key, _)| key.as_ref() == &request)
        {
            ctx.request_repaint_after(Duration::from_millis(24));
            return;
        }
        self.accepted = None;
        self.task = None;
        if request.is_empty() {
            return;
        }
        let now = ctx.input(|input| input.time);
        if now < self.retry_at {
            ctx.request_repaint_after(Duration::from_secs_f64(self.retry_at - now));
            return;
        }
        self.retry_at = now + 0.06;
        let request = Arc::new(request);
        let work = request.clone();
        let repaint = ctx.clone();
        let task = Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(10),
            move |cancel| {
                let result = work.iter().map(|entry| entry.evaluate(cancel)).collect();
                repaint.request_repaint();
                result
            },
        );
        self.task = Some((request, task));
        ctx.request_repaint_after(Duration::from_millis(24));
    }

    pub fn edit_plot(
        &mut self,
        ui: &mut Ui,
        index: usize,
        rect: Rect,
        response: &Response,
        span: &Range<f64>,
        rate: f64,
    ) -> Option<(FilterKind, Option<f64>)> {
        if response.secondary_clicked() {
            self.anchor = response
                .interact_pointer_pos()
                .map(|pointer| plot::at(rect, span, pointer.x));
        }
        if let Some(settings) = self.settings.get_mut(&index) {
            settings.stages.draw(ui, rect, span, rate);
        }
        let enabled = self
            .settings
            .get(&index)
            .is_none_or(|settings| settings.stages.can_add());
        let mut added = None;
        response.context_menu(|ui| {
            added = ui.add_enabled_ui(enabled, filter_menu).inner;
        });
        added.map(|kind| (kind, self.anchor.take()))
    }

    pub fn lanes(&self) -> &[Output] {
        self.accepted
            .as_ref()
            .and_then(|(_, result)| result.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }

    pub fn plots(&self, frame: &Frame) -> Vec<PlotLane<'_>> {
        self.settings
            .keys()
            .filter_map(|&index| {
                let Lane::Analog { channel, .. } = frame.lanes.get(index)? else {
                    return None;
                };
                let values = self.lanes().iter().find(|output| output.index == index);
                let name = format!("{} filtered", channel.name);
                Some(PlotLane {
                    name,
                    values: values.map(|output| &output.lane),
                })
            })
            .collect()
    }

    pub fn has_samples(&self, index: usize, frame: &Frame) -> bool {
        let Some(Lane::Analog { channel, .. }) = frame.lanes.get(index) else {
            return false;
        };
        self.settings
            .get(&index)
            .is_some_and(|settings| !settings.rows(channel, &frame.span).is_empty())
    }

    pub fn error(&self) -> Option<&str> {
        let (_, result) = self.accepted.as_ref()?;
        result.as_ref().err().map(String::as_str)
    }

    fn requests(&self, frame: &Frame) -> Vec<Request> {
        self.settings
            .iter()
            .filter_map(|(&index, settings)| {
                let Lane::Analog { channel, raw, .. } = frame.lanes.get(index)? else {
                    return None;
                };
                if settings.stages.is_empty() {
                    return None;
                }
                let rows = settings.rows(channel, &frame.span);
                if rows.is_empty() {
                    return None;
                }
                Some(Request {
                    index,
                    name: format!("{} filtered", channel.name),
                    unit: channel.unit.clone(),
                    raw: raw.clone(),
                    rows,
                    stages: settings.stages.clone(),
                })
            })
            .collect()
    }
}

impl Request {
    fn evaluate(&self, cancel: &AtomicBool) -> Result<Output, String> {
        let raw = self.raw.as_ref();
        let mut values = raw.samples[self.rows.clone()].to_vec();
        self.stages.apply(raw.step.recip(), &mut values, cancel)?;
        let time = raw.time(self.rows.start)..raw.time(self.rows.end);
        let lane = analog(
            self.name.clone(),
            self.unit.clone(),
            &time,
            raw.step,
            values,
        );
        Ok(Output {
            index: self.index,
            lane,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_lane_keeps_the_start_and_units_of_the_chosen_rows() {
        let raw = Arc::new(RawChannel {
            first: 0,
            start: 0.5,
            step: 0.01,
            samples: vec![2.0; 8],
        });
        let mut stages = StageList::default();
        stages.add(FilterKind::LowPass, 100.0, &(0.0..50.0), Some(25.0));
        let request = Request {
            index: 2,
            name: "Voltage filtered".into(),
            unit: "V".into(),
            raw,
            rows: 2..6,
            stages,
        };
        let output = request.evaluate(&AtomicBool::new(false)).unwrap();
        let Lane::Analog { channel, raw, .. } = output.lane else {
            panic!("expected an analog lane");
        };
        assert_eq!(output.index, 2);
        assert_eq!(channel.unit, "V");
        assert_eq!(raw.start, 0.52);
        assert_eq!(raw.samples.len(), 4);
        assert!((raw.samples[0] - (2.0 - 2.0_f32.sqrt())).abs() < 1e-6);
        assert!(raw.value_at(0.51).is_none());
        assert!(raw.value_at(0.56).is_none());
    }
}
