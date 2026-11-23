use super::super::{
    capture::{Band, Capture, Cutoff, Trace, envelope_points, trace_points},
    read::Read,
};
use egui::Context;
use little_clip_core::engineering::touchstone::{
    BandSearch, Index, Metadata, Parameter, Summary, TracePoint,
    comparison::{self, Axes, Delay, Sample},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub(super) type Pair<T> = [Option<Result<T, String>>; 2];
pub(super) type Outcome<'a, T> = Option<Result<&'a Pair<T>, &'a str>>;
pub(super) type TraceKeys = [Option<Trace>; 2];
type RegionKey = (TraceKeys, Band);
type ThresholdKey = (RegionKey, Cutoff);

pub(super) struct Source {
    pub path: PathBuf,
    pub capture: Arc<Capture>,
    pub summary: Summary,
}

impl Source {
    pub(super) fn load(path: PathBuf, cancel: &AtomicBool) -> Result<Self, String> {
        let (capture, summary) = Capture::open(&path, 512, cancel)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            path,
            capture: Arc::new(capture),
            summary,
        })
    }

    pub(super) fn metadata(&self) -> &Metadata {
        &self.summary.metadata
    }

    pub(super) fn band(&self) -> Band {
        let first = self
            .summary
            .points
            .first()
            .map_or(0.0, |point| point.frequency_hz);
        let last = self
            .summary
            .points
            .last()
            .map_or(first, |point| point.frequency_hz);
        [first, last]
    }

    pub(super) fn magnitude_unit(&self, [response, stimulus]: Trace) -> &'static str {
        let meta = self.metadata();
        if meta.normalized() {
            return if meta.parameter == Parameter::S {
                ""
            } else {
                "normalized"
            };
        }
        match (meta.parameter, response, stimulus) {
            (Parameter::Z, _, _) | (Parameter::H, 0, 0) | (Parameter::G, 1, 1) => "Ω",
            (Parameter::Y, _, _) | (Parameter::G, 0, 0) | (Parameter::H, 1, 1) => "S",
            _ => "",
        }
    }

    pub(super) fn name(&self) -> &Path {
        self.path.file_name().map_or(self.path.as_path(), Path::new)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Element {
    pub parameter: Parameter,
    pub axes: Axes,
}

impl Element {
    pub(super) fn at(source: &Source, trace: Trace) -> Option<Self> {
        let metadata = source.metadata();
        Some(Self {
            parameter: metadata.parameter,
            axes: Axes::from_trace(metadata, trace)?,
        })
    }

    pub(super) fn trace(self, source: &Source) -> Option<Trace> {
        let metadata = source.metadata();
        (metadata.parameter == self.parameter)
            .then(|| self.axes.trace(metadata))
            .flatten()
    }
}

pub(super) struct Indexed {
    pub index: Arc<Index>,
    pub overview: Vec<TracePoint>,
}

#[derive(Clone)]
struct Ready {
    capture: Arc<Capture>,
    index: Arc<Index>,
    trace: Trace,
}

#[derive(Default)]
pub(super) struct Reads {
    index: Read<TraceKeys, Pair<Indexed>>,
    detail: Read<RegionKey, Pair<Vec<TracePoint>>>,
    marker: Read<(TraceKeys, f64), Pair<Option<Sample>>>,
    best: Read<RegionKey, Pair<Option<TracePoint>>>,
    thresholds: Read<ThresholdKey, Pair<BandSearch>>,
    delay: Read<RegionKey, Pair<Option<Delay>>>,
}

pub(super) struct Request {
    pub traces: TraceKeys,
    pub viewport: Band,
    pub marker: f64,
    pub thresholds: Option<(Band, Cutoff)>,
    pub aperture: Option<Band>,
}

impl Reads {
    pub(super) fn update(&mut self, ctx: &Context, sources: &[Source; 2], request: Request) {
        let captures = sources.each_ref().map(|source| source.capture.clone());
        let traces = request.traces;
        self.index.update(
            ctx,
            Some(traces),
            Duration::from_secs(120),
            0.0,
            move |cancel| {
                std::array::from_fn(|source| {
                    traces[source].map(|trace| {
                        let index = Arc::new(captures[source].index(trace, cancel)?);
                        let overview = envelope_points(&index);
                        Ok(Indexed { index, overview })
                    })
                })
            },
        );
        let ready = self.ready(sources, traces);
        self.read_points(ctx, ready, request);
    }

    fn ready(&self, sources: &[Source; 2], traces: TraceKeys) -> [Option<Ready>; 2] {
        let indexed = self.index.value(&traces);
        std::array::from_fn(|source| {
            let side = indexed?.get(source)?.as_ref()?.as_ref().ok()?;
            Some(Ready {
                capture: sources[source].capture.clone(),
                index: side.index.clone(),
                trace: traces[source]?,
            })
        })
    }

    pub(super) fn update_best(
        &mut self,
        ctx: &Context,
        sources: &[Source; 2],
        traces: TraceKeys,
        band: Option<Band>,
    ) {
        let ready = self.ready(sources, traces);
        let key = band
            .filter(|_| ready.iter().any(Option::is_some))
            .map(|band| (traces, band));
        self.best
            .update(ctx, key, Duration::from_secs(45), 0.15, move |cancel| {
                let band = band.expect("match band");
                run(ready, |side| {
                    side.capture
                        .best_match(&side.index, band, side.trace, cancel)
                })
            });
    }

    fn read_points(&mut self, ctx: &Context, ready: [Option<Ready>; 2], request: Request) {
        let available = ready.iter().any(Option::is_some);
        let traces = request.traces;
        let viewport = request.viewport;
        let hz = request.marker;
        let detail = ready.clone();
        self.detail.update(
            ctx,
            available.then_some((traces, viewport)),
            Duration::from_secs(25),
            0.12,
            move |cancel| {
                run(detail, |side| {
                    side.capture
                        .detail(&side.index, viewport, side.trace, cancel)
                })
            },
        );
        let markers = ready.clone();
        self.marker.update(
            ctx,
            available.then_some((traces, hz)),
            Duration::from_secs(15),
            0.045,
            move |cancel| {
                run(markers, |side| {
                    side.capture.sample(&side.index, hz, side.trace, cancel)
                })
            },
        );

        let threshold_key = request
            .thresholds
            .filter(|_| available)
            .map(|(band, cutoff)| ((traces, band), cutoff));
        let threshold_sources = ready.clone();
        let thresholds = request.thresholds;
        self.thresholds.update(
            ctx,
            threshold_key,
            Duration::from_secs(90),
            0.20,
            move |cancel| {
                run(threshold_sources, |side| {
                    let (band, cutoff) = thresholds.expect("threshold query");
                    side.capture
                        .search_bands(&side.index, Some(band), side.trace, cutoff, cancel)
                })
            },
        );
        let aperture_key = request
            .aperture
            .filter(|_| available)
            .map(|band| (traces, band));
        self.delay.update(
            ctx,
            aperture_key,
            Duration::from_secs(25),
            0.11,
            move |cancel| {
                run(ready, |side| {
                    let aperture = request.aperture.expect("delay query");
                    let points =
                        side.capture
                            .exact_band(&side.index, aperture, side.trace, 1, cancel)?;
                    Ok(comparison::delay(&points, aperture))
                })
            },
        );
    }

    pub(super) fn indexed(
        &self,
        traces: TraceKeys,
        source: usize,
    ) -> Option<Result<&Indexed, &str>> {
        match self.index.result(&traces)? {
            Ok(pair) => Some(pair[source].as_ref()?.as_ref().map_err(String::as_str)),
            Err(error) => Some(Err(error)),
        }
    }

    pub(super) fn data(
        &self,
        traces: TraceKeys,
        viewport: Band,
        source: usize,
    ) -> Option<&[TracePoint]> {
        let points = self
            .detail
            .value(&(traces, viewport))
            .and_then(|result| result[source].as_ref()?.as_ref().ok());
        if let Some(points) = points.filter(|points| !points.is_empty()) {
            return Some(points);
        }
        self.indexed(traces, source)?
            .ok()
            .map(|indexed| indexed.overview.as_slice())
    }

    pub(super) fn marker(&self, traces: TraceKeys, hz: f64) -> Outcome<'_, Option<Sample>> {
        self.marker.result(&(traces, hz))
    }

    pub(super) fn best(&self, traces: TraceKeys, band: Band) -> Outcome<'_, Option<TracePoint>> {
        self.best.result(&(traces, band))
    }

    pub(super) fn bands(
        &self,
        traces: TraceKeys,
        band: Band,
        cutoff: Cutoff,
    ) -> Outcome<'_, BandSearch> {
        self.thresholds.result(&((traces, band), cutoff))
    }

    pub(super) fn delay(&self, traces: TraceKeys, band: Band) -> Outcome<'_, Option<Delay>> {
        self.delay.result(&(traces, band))
    }
}

fn run<T>(ready: [Option<Ready>; 2], read: impl Fn(Ready) -> Result<T, String>) -> Pair<T> {
    ready.map(|side| side.map(&read))
}

pub(super) fn prefix(sources: &[Source; 2], traces: TraceKeys) -> [Vec<TracePoint>; 2] {
    std::array::from_fn(|side| {
        traces[side].map_or_else(Vec::new, |trace| {
            trace_points(&sources[side].summary.points, trace)
        })
    })
}
