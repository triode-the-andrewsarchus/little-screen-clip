use super::{JOBS, Task, reads::RawChannel};
use crate::file_manager::signal_view::take_query_result;
use egui::Context;
use little_clip_core::engineering::{
    analysis::{SpectralSurvey, SpectrumWindow, spectral_survey},
    scope::Channel,
    waveforms::{Recording, sample_span},
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Region {
    #[default]
    Current,
    Visible,
    Whole,
}

impl Region {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Current => "Selection or visible",
            Self::Visible => "Visible interval",
            Self::Whole => "Whole lane",
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum LaneKind {
    #[default]
    Recorded,
    Calculated,
    Filtered,
}

impl LaneKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Recorded => "Recorded",
            Self::Calculated => "Math",
            Self::Filtered => "Filtered",
        }
    }
}

#[derive(Clone)]
enum Source {
    Recording { file: Arc<Recording>, index: usize },
    Derived(Arc<RawChannel>),
}

impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Recording { file: a, index: x }, Self::Recording { file: b, index: y }) => {
                Arc::ptr_eq(a, b) && x == y
            }
            (Self::Derived(a), Self::Derived(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct Input {
    source: Source,
    channel: Channel,
}

impl Input {
    pub(super) fn recorded(file: Arc<Recording>, index: usize) -> Self {
        let channel = file.channels()[index].clone();
        Self {
            source: Source::Recording { file, index },
            channel,
        }
    }

    pub(super) fn derived(name: String, unit: String, raw: Arc<RawChannel>) -> Self {
        let channel = Channel {
            name,
            unit,
            sample_count: raw.samples.len() as u64,
            start_seconds: raw.start,
            interval_seconds: raw.step,
        };
        Self {
            source: Source::Derived(raw),
            channel,
        }
    }

    pub(super) fn channel(&self) -> &Channel {
        &self.channel
    }

    fn rows(&self, interval: Option<Range<f64>>) -> Range<u64> {
        interval.map_or(0..self.channel.sample_count, |span| {
            sample_span(&self.channel, span)
        })
    }

    fn measure(
        &self,
        rows: Range<u64>,
        window: SpectrumWindow,
        cancel: &AtomicBool,
    ) -> Result<Option<SpectralSurvey>, String> {
        let rate = self.channel.interval_seconds.recip();
        match &self.source {
            Source::Recording { file, index } => file.checked(cancel, |capture| {
                spectral_survey(rows, rate, window, |rows| {
                    capture.read_samples(*index, rows, cancel)
                })
            }),
            Source::Derived(raw) => spectral_survey(rows, rate, window, |rows| {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("Spectrum was cancelled".into());
                }
                Ok(raw.samples[rows.start as usize..rows.end as usize].to_vec())
            }),
        }
    }
}

#[derive(Clone, PartialEq)]
struct Query {
    input: Input,
    rows: Range<u64>,
    window: SpectrumWindow,
}

#[derive(Default)]
pub(super) struct Spectra {
    pub kind: LaneKind,
    pub region: Region,
    pub only: bool,
    task: Option<(Query, Task<Result<Option<SpectralSurvey>, String>>)>,
    result: Option<(Query, Result<Option<SpectralSurvey>, String>)>,
    requested: Option<Query>,
    request_after: f64,
}

impl Spectra {
    pub(super) fn update(
        &mut self,
        ctx: &Context,
        input: Option<Input>,
        interval: Option<Range<f64>>,
        window: SpectrumWindow,
    ) {
        self.poll();
        let requested = input.map(|input| Query {
            rows: input.rows(interval),
            input,
            window,
        });
        self.requested = requested;
        let Some(query) = &self.requested else { return };
        if self
            .result
            .as_ref()
            .is_some_and(|(ready, _)| ready == query)
            || self.task.is_some()
        {
            return;
        }
        let now = ctx.input(|input| input.time);
        if now < self.request_after {
            ctx.request_repaint_after(Duration::from_secs_f64(self.request_after - now));
            return;
        }
        self.request_after = now + 0.10;
        let current = query.clone();
        let worker = current.clone();
        let repaint = ctx.clone();
        self.task = Some((
            current,
            Task::start(&JOBS, Duration::from_secs(20), move |cancel| {
                let result = worker.input.measure(worker.rows, worker.window, cancel);
                repaint.request_repaint();
                result
            }),
        ));
        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn poll(&mut self) {
        if let Some(result) = take_query_result(&mut self.task) {
            self.result = Some(result);
        }
    }

    pub(super) fn suspend(&mut self) {
        self.task = None;
        self.requested = None;
    }

    pub(super) fn ready(&self) -> Option<(&Input, Result<Option<&SpectralSurvey>, &str>)> {
        let (query, result) = self.result.as_ref()?;
        if self.requested.as_ref() != Some(query) {
            return None;
        }
        Some((
            &query.input,
            result.as_ref().map(Option::as_ref).map_err(String::as_str),
        ))
    }

    pub(super) fn busy(&self) -> bool {
        self.task.is_some()
    }
}
