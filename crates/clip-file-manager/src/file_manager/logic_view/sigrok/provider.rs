use super::super::protocols::{Source, TraceInput};
use little_clip_core::{
    CommandRequest,
    engineering::logic::{Lane, Level, Trace},
};
use rustix::process::Resource;
use std::{
    ops::Range,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const PROGRAM: &str = "clip-sigrok-decoder";
const MAX_RATE: u64 = 10_000_000_000;
const MAX_SAMPLES: usize = 4_000_000;
const MAX_EDGES: usize = 100_000;
const MAX_ANNOTATIONS: usize = 4_000;
const GAP: u8 = 255;

pub(super) struct Catalog {
    pub version: String,
    pub decoders: Vec<Decoder>,
    pub omitted: usize,
}

pub(super) struct Decoder {
    pub id: String,
    pub name: String,
    pub pins: Vec<Pin>,
    pub defaults: bool,
}

pub(super) struct Pin {
    pub id: String,
    pub name: String,
    pub required: bool,
}

pub(super) struct Request {
    pub id: String,
    pub version: String,
    pub pins: Vec<(String, Option<usize>)>,
    pub range: Range<f64>,
    pub grid_hz: Option<u64>,
}

pub(super) struct Annotation {
    pub time: Range<f64>,
    pub text: String,
}

pub(super) struct Output {
    pub range: Range<f64>,
    pub rate: u64,
    pub resampled: bool,
    pub off_grid: usize,
    pub gaps: usize,
    pub partial: bool,
    pub omitted: usize,
    pub annotations: Vec<Annotation>,
}

pub(super) fn catalog(cancel: &AtomicBool) -> Result<Catalog, String> {
    let response = invoke(command().arg("catalog"), cancel)?;
    let mut lines = response.lines();
    let header = lines
        .next()
        .and_then(|line| line.strip_prefix("sigrok\t1\t"));
    let version = header
        .filter(|value| !value.is_empty())
        .ok_or("Unsupported Sigrok helper")?;
    let mut decoders = Vec::<Decoder>::new();
    for line in lines.by_ref() {
        if let Some(omitted) = line.strip_prefix("done\t") {
            let omitted = omitted
                .parse()
                .map_err(|_| "Invalid Sigrok decoder count")?;
            if lines.next().is_some() || decoders.iter().any(|decoder| decoder.pins.is_empty()) {
                break;
            }
            decoders.sort_unstable_by(|a, b| a.name.cmp(&b.name));
            return Ok(Catalog {
                version: version.to_owned(),
                decoders,
                omitted,
            });
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["decoder", id, defaults @ ("0" | "1"), name] if decoders.len() < 512 => {
                decoders.push(Decoder {
                    id: (*id).to_owned(),
                    name: (*name).to_owned(),
                    pins: Vec::new(),
                    defaults: *defaults == "1",
                });
            }
            ["pin", id, required @ ("0" | "1"), name] => {
                let decoder = decoders.last_mut().ok_or("Invalid Sigrok pin catalog")?;
                if decoder.pins.len() >= 7 {
                    break;
                }
                decoder.pins.push(Pin {
                    id: (*id).to_owned(),
                    name: (*name).to_owned(),
                    required: *required == "1",
                });
            }
            _ => break,
        }
    }
    Err("Invalid Sigrok decoder catalog".into())
}

fn command() -> CommandRequest {
    CommandRequest::new(program_path())
        .current_dir("/")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONSAFEPATH", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", "")
        .env("PYTHONHOME", "")
        .env("SIGROKDECODE_DIR", "")
        .env("SIGROKDECODE_PATH", "")
        .env("XDG_DATA_HOME", "/usr/share")
        .env("XDG_DATA_DIRS", "/usr/local/share:/usr/share")
        .timeout(Duration::from_secs(12))
        .max_output_bytes(2 * 1024 * 1024)
        .resource_limit(Resource::As, 512 * 1024 * 1024)
        .resource_limit(Resource::Cpu, 10)
        .resource_limit(Resource::Core, 0)
        .own_process_group()
        .die_with_parent()
}

fn program_path() -> PathBuf {
    if let Some(path) = std::env::var_os("LITTLE_CLIP_SIGROK_DECODER") {
        return path.into();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(profile) = exe.parent()
    {
        for candidate in [
            profile.join(PROGRAM),
            profile.join("../tools/sigrok-decoder").join(PROGRAM),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PROGRAM.into()
}

fn invoke(request: CommandRequest, cancel: &AtomicBool) -> Result<String, String> {
    let result = request
        .run_cancellable(cancel)
        .map_err(|err| err.to_string())?;
    if !result.status.success() {
        let stderr = result.stderr_lossy();
        let detail = stderr.lines().rev().find(|line| !line.trim().is_empty());
        return Err(detail.map_or_else(
            || format!("Sigrok helper exited with {}", result.status),
            |line| line.chars().take(220).collect(),
        ));
    }
    if result.stdout_truncated {
        return Err("Sigrok helper exceeded its response budget".into());
    }
    String::from_utf8(result.stdout).map_err(|_| "Invalid Sigrok helper response".into())
}

pub(super) fn parse_rate(text: &str) -> Option<u64> {
    text.trim()
        .parse()
        .ok()
        .filter(|rate| (1..=MAX_RATE).contains(rate))
}

pub(super) fn native_clock(
    lanes: &[Lane],
    indices: impl IntoIterator<Item = usize>,
) -> Option<(u64, f64)> {
    let mut indices = indices.into_iter();
    let first = lanes.get(indices.next()?)?;
    let rate = first.sample_rate?;
    if rate.fract() != 0.0 || !(1.0..=MAX_RATE as f64).contains(&rate) {
        return None;
    }
    indices
        .all(|index| {
            lanes.get(index).is_some_and(|lane| {
                lane.sample_rate == Some(rate)
                    && clock_coordinate(lane.start_seconds, first.start_seconds, rate).fract()
                        == 0.0
            })
        })
        .then_some((rate as u64, first.start_seconds))
}

struct Grid {
    first: f64,
    rate: u64,
    origin: f64,
    resampled: bool,
}

impl Grid {
    fn coordinate(&self, time: f64) -> f64 {
        clock_coordinate(time, self.origin, self.rate as f64) - self.first
    }

    fn time(&self, sample: usize) -> f64 {
        self.origin + (self.first + sample as f64) / self.rate as f64
    }
}

fn clock_coordinate(time: f64, origin: f64, rate: f64) -> f64 {
    let sample = (time - origin) * rate;
    // Subtracting two large timestamps may lose precision even when the
    // resulting sample index is small. Limit the tolerance to a small fraction
    // of a sample so genuinely off-grid edges remain visible.
    let tolerance =
        ((time.abs() + origin.abs()).mul_add(rate, 1.0) * f64::EPSILON * 16.0).min(0.001);
    if (sample - sample.round()).abs() <= tolerance {
        sample.round()
    } else {
        sample
    }
}

fn rasterize(samples: &mut [u8], trace: Trace, grid: &Grid, bit: u8) -> usize {
    let mut off_grid = 0;
    let mut start = 0.0;
    let mut level = trace.initial.unwrap_or(Level::Unknown);
    for edge in trace.edges {
        let end = grid.coordinate(edge.time_seconds);
        if end < samples.len() as f64 {
            off_grid += usize::from(end.fract() != 0.0);
        }
        paint(samples, start, end, level, bit);
        start = end;
        level = edge.level;
    }
    paint(samples, start, samples.len() as f64, level, bit);
    off_grid
}

fn paint(samples: &mut [u8], start: f64, end: f64, level: Level, bit: u8) {
    if end <= start || level == Level::Low {
        return;
    }
    let unknown = matches!(level, Level::Unknown | Level::HighImpedance);
    // Omit any sample interval touched by X or Z, including gaps shorter
    // than a sample. A later known level cannot overwrite this marker.
    let start = if unknown { start.floor() } else { start.ceil() };
    let end = (end.ceil() as usize).min(samples.len());
    let start = (start as usize).min(end);
    if unknown {
        samples[start..end].fill(GAP);
    } else {
        samples[start..end]
            .iter_mut()
            .for_each(|value| *value |= bit);
    }
}

struct Samples {
    grid: Grid,
    overlap: Range<f64>,
    values: Vec<u8>,
    off_grid: usize,
}

fn sample_grid(
    capture: &dyn TraceInput,
    lanes: &[Lane],
    request: &Request,
) -> Result<(Grid, Range<f64>, usize), String> {
    let indices = || request.pins.iter().filter_map(|(_, index)| *index);
    let mut range = request.range.clone();
    for index in indices() {
        let bound = capture
            .time_bounds(index)
            .ok_or("Mapped channel is unavailable")?;
        range.start = range.start.max(bound.start);
        range.end = range.end.min(bound.end);
    }
    let (rate, origin, resampled) = match native_clock(lanes, indices()) {
        Some((rate, origin)) => (rate, origin, false),
        None => (
            request.grid_hz.ok_or("Enter a sample grid in Hz")?,
            0.0,
            true,
        ),
    };
    let first = clock_coordinate(range.start, origin, rate as f64).ceil();
    let stop = clock_coordinate(range.end, origin, rate as f64).ceil();
    if !(first.abs().max(stop.abs()) < (1_u64 << 49) as f64 && stop > first) {
        return Err(
            "Mapped channels have no overlapping samples in this window at this rate".into(),
        );
    }
    let grid = Grid {
        first,
        origin,
        rate,
        resampled,
    };
    if grid.time(1) <= grid.time(0) {
        return Err("Sample grid is finer than the capture can represent".into());
    }
    Ok((grid, range, ((stop - first) as usize).min(MAX_SAMPLES)))
}

fn read_samples(
    capture: &dyn TraceInput,
    lanes: &[Lane],
    request: &Request,
    cancel: &AtomicBool,
) -> Result<Samples, String> {
    let (grid, overlap, mut count) = sample_grid(capture, lanes, request)?;
    let search_end = grid.time(count).min(overlap.end);
    let mut traces = Vec::new();
    for (pin, (_, index)) in request.pins.iter().enumerate() {
        let Some(index) = index else { continue };
        let trace = capture.read_edges(*index, grid.time(0)..search_end, MAX_EDGES, cancel)?;
        count = count.min(grid.coordinate(trace.inspected_until).ceil() as usize);
        traces.push((1 << pin, trace));
    }
    if count == 0 {
        return Err("No exact logic samples were available in this window".into());
    }
    let mut values = vec![0; count];
    let mut off_grid = 0;
    for (bit, trace) in traces {
        if cancel.load(Ordering::Relaxed) {
            return Err("Sigrok decoding cancelled".into());
        }
        off_grid += rasterize(&mut values, trace, &grid, bit);
    }
    Ok(Samples {
        grid,
        overlap,
        values,
        off_grid,
    })
}

pub(super) fn decode(
    source: &Source,
    lanes: &[Lane],
    request: Request,
    cancel: &AtomicBool,
) -> Result<Output, String> {
    let Samples {
        grid,
        overlap,
        values,
        off_grid,
    } = source.checked(cancel, |capture| {
        read_samples(capture, lanes, &request, cancel)
    })?;
    let input_len = values.len();
    let mapped = request
        .pins
        .iter()
        .map(|(id, index)| if index.is_some() { id.as_str() } else { "-" });
    let response = invoke(
        command()
            .arg("decode")
            .arg(request.id)
            .arg(request.version)
            .arg(grid.rate.to_string())
            .args(mapped)
            .stdin_bytes(values),
        cancel,
    )?;
    // Recheck the guarded source after the child, before displaying its data.
    source.checked(cancel, |_| Ok(()))?;
    let Reply {
        mut annotations,
        consumed,
        omitted,
        gaps,
    } = annotations(&response, &grid, input_len)?;
    annotations.sort_by(|a, b| {
        a.time
            .start
            .total_cmp(&b.time.start)
            .then(a.time.end.total_cmp(&b.time.end))
    });
    Ok(Output {
        range: grid.time(0)..grid.time(consumed).min(overlap.end),
        rate: grid.rate,
        resampled: grid.resampled,
        off_grid,
        gaps,
        partial: consumed < input_len
            || grid.coordinate(overlap.end).ceil() > consumed as f64
            || overlap != request.range,
        omitted,
        annotations,
    })
}

struct Reply {
    annotations: Vec<Annotation>,
    consumed: usize,
    omitted: usize,
    gaps: usize,
}

fn annotations(response: &str, grid: &Grid, input_len: usize) -> Result<Reply, String> {
    let invalid = || "Invalid Sigrok annotation response".to_owned();
    let mut annotations = Vec::new();
    let mut last_sample = 0;
    let mut lines = response.lines();
    for line in lines.by_ref() {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["ann", start, end, _class, text] if annotations.len() < MAX_ANNOTATIONS => {
                let start = start.parse::<usize>().map_err(|_| invalid())?;
                let end = end.parse::<usize>().map_err(|_| invalid())?;
                if start > end || end >= input_len {
                    return Err(invalid());
                }
                last_sample = last_sample.max(end + 1);
                annotations.push(Annotation {
                    time: grid.time(start)..grid.time(end),
                    text: (*text).to_owned(),
                });
            }
            ["done", consumed, omitted, gaps] => {
                let counts = [consumed, omitted, gaps].map(|field| field.parse::<usize>());
                let [Ok(consumed), Ok(omitted), Ok(gaps)] = counts else {
                    return Err(invalid());
                };
                if lines.next().is_some()
                    || !coverage_valid(consumed, gaps, last_sample..=input_len)
                {
                    return Err(invalid());
                }
                return Ok(Reply {
                    annotations,
                    consumed,
                    omitted,
                    gaps,
                });
            }
            _ => return Err(invalid()),
        }
    }
    Err(invalid())
}

fn coverage_valid(consumed: usize, gaps: usize, input: std::ops::RangeInclusive<usize>) -> bool {
    input.contains(&consumed) && gaps <= consumed
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::logic::Edge;

    #[test]
    fn a_recorded_clock_keeps_its_phase_when_the_capture_starts_late() {
        let start = 1.2345;
        let rate = 100_000_000.0;
        assert_eq!(clock_coordinate(start + 1.0 / rate, start, rate), 1.0);
        assert_ne!(clock_coordinate(start + 1.01 / rate, start, rate), 1.0);
    }

    #[test]
    fn a_subsample_gap_retires_decoder_state() {
        let grid = Grid {
            first: 10.0,
            rate: 1_000_000,
            origin: 0.0,
            resampled: true,
        };
        let mut samples = [0; 4];
        let trace = Trace {
            initial: Some(Level::High),
            edges: vec![
                Edge {
                    time_seconds: 11.1e-6,
                    level: Level::HighImpedance,
                },
                Edge {
                    time_seconds: 11.2e-6,
                    level: Level::High,
                },
                Edge {
                    time_seconds: 12e-6,
                    level: Level::Low,
                },
            ],
            inspected_until: 14e-6,
        };
        assert_eq!(rasterize(&mut samples, trace, &grid, 2), 2);
        assert_eq!(samples, [2, GAP, 0, 0]);
    }

    #[test]
    #[ignore = "requires the optional libsigrokdecode helper and decoder catalog"]
    fn sigrok_onewire_vcd_events_preserve_saved_sample_anchors() {
        use little_clip_core::engineering::logic_preview::Recording;
        use std::{io::Write, sync::Arc};

        let cancel = AtomicBool::new(false);
        let mut fixture = tempfile::Builder::new().suffix(".vcd").tempfile().unwrap();
        fixture.write_all(b"$timescale 1 us $end\n$scope module engineering $end\n$var wire 1 ! data $end\n$upscope $end\n$enddefinitions $end\n#0\nx!\n#12000\nx!\n#12037\n1!\n#12187\n0!\n#12707\n1!\n#12737\n0!\n#12857\n1!\n#13387\n0!\n#13393\n1!\n#13487\n0!\n#13547\n1!\n#14452\n0!\n#14472\nx!\n#14513\nz!\n#14556\n1!\n#14706\n0!\n#15226\n1!\n#15256\n0!\n#15376\n1!\n#15906\n0!\n#15912\n1!\n#16006\n0!\n#16066\n1!\n#16956\nx!\n#16973\n").unwrap();
        let recording = Recording::open(fixture.path(), &cancel).unwrap();
        let overview = recording.overview(&cancel).unwrap();
        recording.refine(overview.span, &cancel).unwrap();
        let catalog = catalog(&cancel).unwrap();
        let decoder = catalog
            .decoders
            .iter()
            .find(|decoder| decoder.id == "onewire_link")
            .unwrap();
        let output = decode(
            &Source::Recording(Arc::new(recording)),
            &overview.lanes,
            Request {
                id: decoder.id.clone(),
                version: catalog.version,
                pins: vec![(decoder.pins[0].id.clone(), Some(0))],
                range: 0.012..0.016973,
                grid_hz: Some(1_000_000),
            },
            &cancel,
        )
        .unwrap();
        let anchors = output
            .annotations
            .iter()
            .map(|annotation| {
                (
                    (annotation.time.start * 1e6).round() as u64,
                    (annotation.time.end * 1e6).round() as u64,
                    annotation.text.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            anchors,
            [
                (12_187, 12_707, "Reset"),
                (12_707, 13_187, "Presence: true"),
                (13_387, 13_447, "Bit: 1"),
                (13_487, 13_547, "Bit: 0"),
                (14_706, 15_226, "Reset"),
                (15_226, 15_706, "Presence: true"),
                (15_906, 15_966, "Bit: 1"),
                (16_006, 16_066, "Bit: 0"),
            ]
        );
        assert_eq!(output.off_grid, 0);
        assert_eq!(output.gaps, 138);
        assert_eq!(output.omitted, 0);
        assert!(!output.partial);
    }
}
