use super::{inputs::Samples, take_query_result, timeline};
use crate::file_manager::{preview_job::Task, scope_view::reads::RawChannel};
use egui::Context;
use little_clip_core::BackgroundJobs;
use little_clip_core::engineering::{
    analysis::{SpectralSurvey, SpectrumWindow, spectral_survey},
    logic_preview,
    scope::Channel,
    waveforms::{Recording, sample_span},
};
use std::{
    ops::Range,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 4));

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) enum Region {
    #[default]
    Current,
    Visible,
    Whole,
}

impl Region {
    pub(in crate::file_manager) fn interval(
        self,
        selection: Option<Range<f64>>,
        visible: &Range<f64>,
    ) -> Option<Range<f64>> {
        match self {
            Self::Current => selection.or_else(|| Some(visible.clone())),
            Self::Visible => Some(visible.clone()),
            Self::Whole => None,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) enum LaneKind {
    #[default]
    Recorded,
    Calculated,
    Filtered,
}

impl LaneKind {
    pub(in crate::file_manager) fn label(self) -> &'static str {
        match self {
            Self::Recorded => "Recorded",
            Self::Calculated => "Math",
            Self::Filtered => "Filtered",
        }
    }
}

#[derive(Clone)]
enum Source {
    Recording {
        file: Arc<Recording>,
        index: usize,
    },
    Derived(Arc<RawChannel>),
    Logic {
        file: Arc<logic_preview::Recording>,
        index: usize,
    },
}

impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Recording { file: a, index: x }, Self::Recording { file: b, index: y }) => {
                Arc::ptr_eq(a, b) && x == y
            }
            (Self::Derived(a), Self::Derived(b)) => Arc::ptr_eq(a, b),
            (Self::Logic { file: a, index: x }, Self::Logic { file: b, index: y }) => {
                Arc::ptr_eq(a, b) && x == y
            }
            _ => false,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::file_manager) struct Input {
    source: Source,
    channel: Channel,
}

impl Input {
    pub(in crate::file_manager) fn recorded(file: Arc<Recording>, index: usize) -> Self {
        let channel = file.channels()[index].clone();
        Self {
            source: Source::Recording { file, index },
            channel,
        }
    }

    pub(in crate::file_manager) fn derived(
        name: String,
        unit: String,
        raw: Arc<RawChannel>,
    ) -> Self {
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

    pub(in crate::file_manager) fn logic(
        file: Arc<logic_preview::Recording>,
        index: usize,
        name: &str,
        time: Range<f64>,
        rate: f64,
    ) -> Self {
        let sample_count = ((time.end - time.start) * rate).ceil() as u64;
        Self {
            source: Source::Logic { file, index },
            channel: Channel {
                name: name.to_owned(),
                unit: String::new(),
                sample_count,
                start_seconds: time.start,
                interval_seconds: rate.recip(),
            },
        }
    }

    pub(in crate::file_manager) fn channel(&self) -> &Channel {
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
            Source::Logic { file, index } => file.checked(cancel, |capture| {
                spectral_survey(rows, rate, window, |rows| {
                    let start = self.channel.time_at(rows.start);
                    let end = self.channel.time_at(rows.end);
                    let native = start..end;
                    let trace = capture.read_edges(*index, native.clone(), 1 << 18, cancel)?;
                    let samples = Samples::Digital {
                        trace,
                        native,
                        clock: Default::default(),
                    };
                    Ok(rows
                        .map(|row| match samples.at(self.channel.time_at(row)) {
                            Some(little_clip_core::engineering::analysis::Value::Predicate(
                                level,
                            )) => f32::from(u8::from(level)),
                            _ => f32::NAN,
                        })
                        .collect())
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

#[derive(Clone, Copy)]
pub(in crate::file_manager) struct ResultView<'a> {
    pub input: &'a Input,
    pub rows: &'a Range<u64>,
    pub output: Result<Option<&'a SpectralSurvey>, &'a str>,
}

pub(in crate::file_manager) struct PeakWindow {
    pub interval: Range<f64>,
    pub hz: f64,
}

struct Inspection {
    window: PeakWindow,
    previous_view: Range<f64>,
    reveal: bool,
}

#[derive(Default)]
pub(in crate::file_manager) struct Spectra {
    pub kind: LaneKind,
    pub region: Region,
    pub only: bool,
    task: Option<(Query, Task<Result<Option<SpectralSurvey>, String>>)>,
    result: Option<(Query, Result<Option<SpectralSurvey>, String>)>,
    requested: Option<Query>,
    request_after: f64,
    inspection: Option<Inspection>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct DisplaySettings {
    kind: LaneKind,
    region: Region,
    only: bool,
}

impl Spectra {
    pub(in crate::file_manager) fn settings(&self) -> DisplaySettings {
        DisplaySettings {
            kind: self.kind,
            region: self.region,
            only: self.only,
        }
    }

    pub(in crate::file_manager) fn restore_settings(&mut self, settings: DisplaySettings) {
        self.kind = settings.kind;
        self.region = settings.region;
        self.only = settings.only;
        self.clear_result();
    }

    pub(in crate::file_manager) fn inspect(
        &mut self,
        window: PeakWindow,
        view: &mut Range<f64>,
        domain: &Range<f64>,
        min_width: f64,
    ) {
        let padding = (window.interval.end - window.interval.start) * 0.08;
        let next = timeline::fit(
            domain,
            window.interval.start - padding..window.interval.end + padding,
            min_width,
        );
        self.inspection = Some(Inspection {
            window,
            previous_view: std::mem::replace(view, next),
            reveal: true,
        });
    }

    pub(in crate::file_manager) fn origin(&self) -> Option<&PeakWindow> {
        Some(&self.inspection.as_ref()?.window)
    }

    pub(in crate::file_manager) fn take_origin_reveal(&mut self) -> bool {
        self.inspection
            .as_mut()
            .is_some_and(|inspection| std::mem::take(&mut inspection.reveal))
    }

    pub(in crate::file_manager) fn finish_inspection(&mut self, view: &mut Range<f64>) {
        if let Some(inspection) = self.inspection.take() {
            *view = inspection.previous_view;
        }
    }

    pub(in crate::file_manager) fn update(
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
        {
            return;
        }
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(33));
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

    pub(in crate::file_manager) fn suspend(&mut self) {
        self.task = None;
        self.requested = None;
    }

    pub(in crate::file_manager) fn clear_result(&mut self) {
        self.suspend();
        self.result = None;
        self.request_after = 0.0;
        self.inspection = None;
    }

    pub(in crate::file_manager) fn ready(&self) -> Option<ResultView<'_>> {
        let (query, _) = self.result.as_ref()?;
        if self.requested.as_ref() != Some(query) {
            return None;
        }
        self.latest()
    }

    pub(in crate::file_manager) fn latest(&self) -> Option<ResultView<'_>> {
        let (query, result) = self.result.as_ref()?;
        Some(ResultView {
            input: &query.input,
            rows: &query.rows,
            output: result.as_ref().map(Option::as_ref).map_err(String::as_str),
        })
    }

    pub(in crate::file_manager) fn busy(&self) -> bool {
        self.task.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspecting_a_peak_restores_the_original_view_and_fft_settings() {
        let mut spectra = Spectra {
            region: Region::Visible,
            only: true,
            ..Default::default()
        };
        let original = 0.25..4.0;
        let mut view = original.clone();
        spectra.inspect(
            PeakWindow {
                interval: 1.0..2.0,
                hz: 1000.0,
            },
            &mut view,
            &(0.0..4.0),
            0.0,
        );
        assert!(view.start < 1.0 && view.end > 2.0);
        assert_eq!(spectra.origin().unwrap().interval, 1.0..2.0);
        assert!(spectra.take_origin_reveal());
        assert!(!spectra.take_origin_reveal());
        assert_eq!(spectra.region, Region::Visible);
        assert!(spectra.only);
        spectra.finish_inspection(&mut view);
        assert_eq!(view, original);
        assert!(spectra.origin().is_none());
    }
}
