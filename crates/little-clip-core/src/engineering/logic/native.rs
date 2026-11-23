//! Native Saleae Logic 2 digital capture archives.

use super::{
    Envelope, EnvelopeWriter, Lane, Level, MAX_INFLATE_BYTES, MAX_LANES, MAX_WINDOW_SAMPLES,
    Opened, Trace,
    archive::{Archive, Member},
    check_cancel, push_state,
    rle::{Chunk, Header},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const MAX_METADATA: usize = 4 << 20;
const MAX_CHUNKS_PER_JOB: usize = 8192;

pub(super) struct Native {
    member: Arc<Member>,
    header: Header,
    start: u64,
    fixed_end: Option<u64>,
    indexed_end: AtomicU64,
    finished: AtomicBool,
    index: Mutex<Index>,
}

struct Index {
    chunks: Vec<Chunk>,
    next: u64,
}

pub(super) fn open(archive: &mut Archive) -> Result<Opened<Native>, String> {
    let meta = archive.metadata("meta.json", MAX_METADATA)?;
    let meta: Value =
        serde_json::from_slice(&meta).map_err(|e| format!("Invalid Saleae metadata. {e}"))?;
    let version = meta["version"]
        .as_u64()
        .ok_or("Saleae metadata version is missing.")?;
    if !(1..=22).contains(&version) {
        return Err("Unsupported Saleae metadata version.".into());
    }
    let data = meta["data"]
        .as_object()
        .ok_or("Saleae capture metadata is missing.")?;
    let manifest = meta["binData"]
        .as_array()
        .filter(|entries| entries.len() <= MAX_LANES * 4)
        .ok_or("Saleae capture channel manifest is missing or too large.")?;
    let interval = recorded_interval(meta["data"].get("captureProgress"));
    let mut channels = BTreeMap::new();
    let mut omitted = 0;
    // At open only the manifest and one bounded prefix per digital member are read.
    let cancel = AtomicBool::new(false);
    for entry in manifest {
        let Some((channel, name)) = channel_member(entry) else {
            omitted += 1;
            continue;
        };
        let label = channel_name(data.get("rowsSettings"), channel);
        let (lane, capture) =
            Native::open(archive.member(name)?, label, interval.clone(), &cancel)?;
        if channels.insert(channel, (lane, capture)).is_some() {
            return Err("Saleae capture has duplicate channel entries.".into());
        }
        if channels.len() > MAX_LANES {
            return Err("Too many saved Saleae channels.".into());
        }
    }
    if channels.is_empty() {
        return Err("This Saleae capture has no supported legacy digital channels.".into());
    }
    let note = (omitted > 0)
        .then(|| format!("{omitted} analog or instrument channel(s) are unavailable."));
    Ok(Opened {
        channels: channels.into_values().collect(),
        note,
    })
}

fn recorded_interval(progress: Option<&Value>) -> Option<Range<f64>> {
    let interval = progress?.get("processedInterval")?;
    let start = interval["begin"].as_f64()?;
    let end = interval["end"].as_f64()?;
    (start.is_finite() && end.is_finite() && start >= 0.0 && end > start).then_some(start..end)
}

fn channel_member(entry: &Value) -> Option<(u64, &str)> {
    let channel = channel_id(entry)?;
    let file = entry["file"].as_str()?.trim_start_matches("./");
    let member_channel = file
        .strip_prefix("digital-")?
        .strip_suffix(".bin")?
        .parse::<u64>()
        .ok()?;
    (channel == member_channel).then_some((channel, file))
}

fn channel_id(entry: &Value) -> Option<u64> {
    if entry["type"].as_str()? != "Digital"
        || entry["category"]
            .as_str()
            .is_some_and(|kind| kind != "legacy")
    {
        return None;
    }
    entry["deviceChannel"]
        .as_u64()
        .or_else(|| entry["index"].as_u64())
}

fn channel_name(rows: Option<&Value>, channel: u64) -> String {
    let default = || format!("Channel {channel}");
    let Some(rows) = rows.and_then(Value::as_array) else {
        return default();
    };
    rows.iter()
        .find_map(|row| {
            let id = if row["type"] == "channel" {
                channel_id(&row["channel"])
            } else {
                channel_id(row)
            };
            (id == Some(channel))
                .then(|| row["name"].as_str())
                .flatten()
        })
        .map_or_else(default, |name| name.chars().take(128).collect())
}

impl Native {
    fn open(
        member: Arc<Member>,
        name: String,
        interval: Option<Range<f64>>,
        cancel: &AtomicBool,
    ) -> Result<(Lane, Self), String> {
        let header = Header::read(&member, cancel)?;
        let start = header
            .start
            .or_else(|| {
                interval
                    .as_ref()
                    .map(|range| (range.start * header.rate).round() as u64)
            })
            .unwrap_or(0);
        let fixed_end = header.end.or_else(|| {
            interval
                .as_ref()
                .map(|range| (range.end * header.rate).round() as u64)
        });
        if fixed_end.is_some_and(|end| end <= start || end > 1 << 53) {
            return Err("Saleae channel has inconsistent captured time bounds.".into());
        }
        let source = Self {
            member,
            header,
            start,
            fixed_end,
            indexed_end: AtomicU64::new(0),
            index: Mutex::new(Index {
                chunks: Vec::new(),
                next: header.first,
            }),
            finished: AtomicBool::new(false),
        };
        if fixed_end.is_none() {
            let mut budget = MAX_INFLATE_BYTES;
            source.prepare(start.saturating_add(1), &mut budget, cancel)?;
        }
        let range = source.bounds();
        if !range.start.is_finite() || !range.end.is_finite() || range.end <= range.start {
            return Err("No bounded time window in Saleae digital data.".into());
        }
        let lane = Lane {
            name,
            start_seconds: range.start,
            end_seconds: range.end,
            sample_rate: Some(header.rate),
        };
        Ok((lane, source))
    }

    pub(super) fn bounds(&self) -> Range<f64> {
        let stop = self
            .fixed_end
            .unwrap_or_else(|| self.indexed_end.load(Ordering::Acquire));
        self.time_at(self.start)..self.time_at(stop)
    }

    pub(super) fn refine(&self, budget: &mut u64, cancel: &AtomicBool) -> Result<bool, String> {
        if self.fixed_end.is_none() && !self.finished.load(Ordering::Acquire) {
            self.prepare(u64::MAX, budget, cancel)?;
        }
        Ok(self.fixed_end.is_some() || self.finished.load(Ordering::Acquire))
    }

    fn time_at(&self, index: u64) -> f64 {
        index as f64 / self.header.rate
    }

    fn prepare(&self, target: u64, budget: &mut u64, cancel: &AtomicBool) -> Result<(), String> {
        let mut index = self
            .index
            .lock()
            .map_err(|_| "Saleae block index is unavailable.")?;
        for _ in 0..MAX_CHUNKS_PER_JOB {
            check_cancel(cancel)?;
            if index.chunks.len() as u64 == self.header.count {
                if index.next != self.member.size {
                    return Err("Saleae native channel has unexpected trailing bytes.".into());
                }
                self.finished.store(true, Ordering::Release);
                break;
            }
            let last = index.chunks.last().map_or(0, |chunk| chunk.ticks.end);
            if last >= target {
                break;
            }
            let Some(chunk) = self
                .header
                .read_chunk(&self.member, index.next, budget, cancel)?
            else {
                break;
            };
            if chunk.ticks.start < last {
                return Err("Saleae native digital chunks overlap or are out of order.".into());
            }
            index.next = chunk.next;
            self.indexed_end.store(chunk.ticks.end, Ordering::Release);
            index.chunks.push(chunk);
        }
        Ok(())
    }

    fn chunks(
        &self,
        time: &Range<f64>,
        budget: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<Chunk>, String> {
        let time_tick = |when: f64| when * self.header.rate;
        let end = time_tick(time.end).ceil() as u64;
        let begin = time_tick(time.start).floor() as u64;
        self.prepare(end, budget, cancel)?;
        let index = self
            .index
            .lock()
            .map_err(|_| "Saleae block index is unavailable.")?;
        let first = index
            .chunks
            .partition_point(|chunk| chunk.ticks.end <= begin);
        let end = index
            .chunks
            .partition_point(|chunk| chunk.ticks.start < end);
        Ok(index.chunks[first..end].to_vec())
    }

    pub(super) fn read_edges(
        &self,
        time: Range<f64>,
        max_edges: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        let mut trace = Trace {
            initial: None,
            edges: Vec::new(),
            inspected_until: time.start,
        };
        let mut last = None;
        let limit = MAX_WINDOW_SAMPLES;
        let inspected = self.visit(&time, limit, cancel, |span, level| {
            push_state(&mut trace, &mut last, max_edges, span.start, level)
        })?;
        trace.inspected_until = inspected;
        Ok(trace)
    }

    pub(super) fn summarize(
        &self,
        time: Range<f64>,
        columns: usize,
        sample_budget: u64,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let mut writer = EnvelopeWriter::new(time.clone(), columns);
        let limit = sample_budget.clamp(1, MAX_WINDOW_SAMPLES);
        self.visit(&time, limit, cancel, |span, level| {
            writer.observe(span, level);
            true
        })?;
        Ok(writer.finish())
    }

    fn visit(
        &self,
        time: &Range<f64>,
        mut remaining_runs: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(Range<f64>, Level) -> bool,
    ) -> Result<f64, String> {
        let mut budget = MAX_INFLATE_BYTES;
        let chunks = self.chunks(time, &mut budget, cancel)?;
        let mut cursor = time.start;
        for chunk in chunks {
            let start = self.time_at(chunk.ticks.start).clamp(time.start, time.end);
            if cursor < start && !consume(cursor..start, Level::Unknown) {
                return Ok(cursor);
            }
            let Some(mut runs) = chunk.read(&self.member, &mut budget, cancel)? else {
                return Ok(start);
            };
            let result =
                self.visit_runs(&mut runs, time, &mut remaining_runs, cancel, &mut consume)?;
            cursor = match result {
                Scan::More(end) => end,
                Scan::Stop(at) => return Ok(at),
            };
        }
        if self.finished.load(Ordering::Acquire)
            && cursor < time.end
            && consume(cursor..time.end, Level::Unknown)
        {
            cursor = time.end;
        }
        Ok(cursor)
    }

    fn visit_runs(
        &self,
        runs: &mut super::rle::Runs,
        time: &Range<f64>,
        budget: &mut u64,
        cancel: &AtomicBool,
        consume: &mut impl FnMut(Range<f64>, Level) -> bool,
    ) -> Result<Scan, String> {
        let mut cursor = time.start;
        for row in 0..MAX_WINDOW_SAMPLES {
            if row.is_multiple_of(4096) {
                check_cancel(cancel)?;
            }
            let Some((run, level)) = runs.next()? else {
                return Ok(Scan::More(cursor));
            };
            let start = self.time_at(run.start).max(time.start);
            let end = self.time_at(run.end).min(time.end);
            if start >= time.end {
                return Ok(Scan::Stop(time.end));
            }
            if start < end {
                if *budget == 0 || !consume(start..end, level) {
                    return Ok(Scan::Stop(start));
                }
                *budget -= 1;
                cursor = end;
            }
        }
        Ok(Scan::Stop(cursor))
    }
}

enum Scan {
    More(f64),
    Stop(f64),
}
