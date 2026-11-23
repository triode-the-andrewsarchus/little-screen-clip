//! JLS v2 fixed-rate tracks, following Jetperch's public on-disk format specification.
//!
//! https://docs.joulescope.com/en/software/jls/h_format.html

mod raw;
#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, ops::Range, sync::atomic::AtomicBool};

use super::{
    Channel, Envelope, MAX_INTEGRAL_SAMPLES, Peak, Reader, check_cancel, new_envelope,
    raw_envelope, raw_sum, u16_at, u32_at, u64_at,
};
use raw::{Chunk, Table, checksum};

const SOURCE_DEF: u8 = 0x01;
const SIGNAL_DEF: u8 = 0x02;
const FSR_HEAD: u8 = 0x21;
const FSR_DATA: u8 = 0x22;
const FSR_INDEX: u8 = 0x23;
const FSR_SUMMARY: u8 = 0x24;
const UTC_HEAD: u8 = 0x39;
const UTC_DATA: u8 = 0x3a;
const LEVELS: usize = 16;
const MAX_METADATA_CHUNKS: usize = 4096;
const MAX_METADATA_PAYLOAD: usize = 16384;
const MAX_DATA_SAMPLES: u64 = 1 << 22;
const MAX_TABLE_ENTRIES: u64 = 1 << 19;
const MAX_SUMMARY_BINS: u64 = 32768;
const MAX_LINKS: usize = 64;

#[derive(Debug)]
pub(super) struct Recording {
    pub(super) model: String,
    pub(super) channels: Vec<Channel>,
    signals: Vec<Signal>,
}

#[derive(Debug)]
struct Signal {
    id: u16,
    source: u16,
    channel: Channel,
    kind: SampleType,
    samples_per_chunk: u64,
    sample_decimate: u64,
    entries_per_summary: u64,
    summary_decimate: u64,
    offsets: [u64; LEVELS],
    utc: u64,
    origin: i64,
    end_chunk: Option<Table>,
}

impl Recording {
    pub(super) fn open(reader: &Reader) -> Result<Self, String> {
        let header = reader.array::<32>(0)?;
        if header[27] != 1 || checksum(&header[..28]) != u32_at(&header, 28) {
            return Err("Unsupported or damaged Joulescope v2 file header".into());
        }
        let length = u64_at(&header, 16);
        if length != 0 && length != reader.len {
            return Err("Joulescope file does not match its recorded length".into());
        }

        // The first definition anchors a linked list of all signal and track
        // definitions, including definitions added after recording started.
        let mut offset = 32;
        let mut source_head = 0;
        let mut signal_head = 0;
        for _ in 0..MAX_METADATA_CHUNKS {
            if offset == reader.len {
                break;
            }
            let chunk = Chunk::read(reader, offset)?;
            match chunk.tag {
                SOURCE_DEF if source_head == 0 => source_head = offset,
                SIGNAL_DEF if signal_head == 0 => signal_head = offset,
                _ => {}
            }
            if source_head != 0 && signal_head != 0 {
                break;
            }
            offset = chunk.end;
        }

        let mut definitions = BTreeMap::new();
        let mut heads = BTreeMap::new();
        let mut utc_heads = BTreeMap::new();
        walk_definitions(reader, signal_head, |chunk| {
            match chunk.tag {
                SIGNAL_DEF if chunk.meta > 0 && chunk.meta < 256 => {
                    let payload = chunk.payload(reader, MAX_METADATA_PAYLOAD)?;
                    if let Some(signal) = Signal::definition(chunk.meta, &payload)? {
                        definitions.entry(chunk.meta).or_insert(signal);
                    }
                }
                FSR_HEAD | UTC_HEAD if chunk.meta > 0 && chunk.meta < 256 => {
                    let payload = chunk.payload(reader, LEVELS * 8)?;
                    if payload.len() != LEVELS * 8 {
                        return Err("Invalid Joulescope track head".into());
                    }
                    if chunk.tag == FSR_HEAD {
                        heads.entry(chunk.meta).or_insert_with(|| {
                            std::array::from_fn(|index| u64_at(&payload, index * 8))
                        });
                    } else {
                        utc_heads.entry(chunk.meta).or_insert(u64_at(&payload, 0));
                    }
                }
                _ => {}
            }
            Ok(())
        })?;

        let mut sources = BTreeMap::new();
        walk_definitions(reader, source_head, |chunk| {
            if chunk.tag != SOURCE_DEF {
                return Err("Invalid Joulescope source definition".into());
            }
            if chunk.meta != 0 && chunk.meta < 256 {
                let payload = chunk.payload(reader, MAX_METADATA_PAYLOAD)?;
                let [name, _, model] = strings::<3>(&payload, 64)?;
                sources
                    .entry(chunk.meta)
                    .or_insert(if model.is_empty() { name } else { model });
            }
            Ok(())
        })?;

        let mut signals = Vec::new();
        let mut first_times = Vec::new();
        for (id, mut signal) in definitions {
            let Some(offsets) = heads.remove(&id) else {
                continue;
            };
            if offsets[0] == 0 {
                continue;
            }
            signal.offsets = offsets;
            signal.utc = utc_heads.remove(&id).unwrap_or(0);
            signal.find_extent(reader)?;
            if signal.channel.sample_count > i64::MAX as u64 {
                return Err("Joulescope waveform exceeds its sample count limit".into());
            }
            first_times.push(signal.first_utc(reader));
            signals.push(signal);
        }
        if signals.is_empty() {
            return Err("Joulescope recording has no supported fixed-rate signal data".into());
        }

        let mut channels = Vec::with_capacity(signals.len());
        let origin = first_times.iter().copied().flatten().min_by(f64::total_cmp);
        for (signal, time) in signals.iter().zip(first_times) {
            let mut channel = signal.channel.clone();
            if let (Some(origin), Some(time)) = (origin, time) {
                channel.start_seconds = time - origin;
            }
            if signals
                .iter()
                .any(|other| signal.id != other.id && signal.channel.name == other.channel.name)
                && let Some(source) = sources.get(&signal.source)
            {
                channel.name = format!("{source} · {}", channel.name);
            }
            channels.push(channel);
        }
        let model = match sources.len() {
            1 => sources
                .into_values()
                .find(|value| !value.is_empty())
                .unwrap_or_else(|| "Joulescope".into()),
            0 => "Joulescope".into(),
            count => format!("Joulescope · {count} sources"),
        };
        Ok(Self {
            model,
            channels,
            signals,
        })
    }

    pub(super) fn read_samples(
        &self,
        reader: &Reader,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        self.signals[channel].samples(reader, range, cancel)
    }

    pub(super) fn envelope(
        &self,
        reader: &Reader,
        channel: usize,
        range: Range<u64>,
        columns: u64,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let signal = &self.signals[channel];
        let length = range.end - range.start;
        let mut step = length.div_ceil(columns).max(1);
        let level = signal.summary_level(step, length);
        let grid = signal.step(level);
        let start = range.start - range.start % grid;
        step = step.div_ceil(grid).saturating_mul(grid);
        let mut result = new_envelope(start, range.end, step);
        let end = start
            .saturating_add(step.saturating_mul(result.peaks.len() as u64))
            .min(signal.channel.sample_count);
        let mut covered = start;
        for level in (1..=level).rev() {
            if covered >= end {
                break;
            }
            let step = signal.step(level);
            covered =
                signal.visit_summary(reader, level, covered..end, cancel, |mut at, entries| {
                    for peak in entries.peaks(signal.kind.scale) {
                        let bin = (at - result.start) / result.step;
                        result.peaks[bin as usize].merge(peak);
                        at = at.saturating_add(step);
                    }
                })?;
        }
        raw_envelope(&mut result, covered..end, |range| {
            signal.samples(reader, range, cancel)
        })?;
        Ok(result)
    }

    pub(super) fn integral(
        &self,
        reader: &Reader,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
        require_known_count: bool,
    ) -> Result<Option<f64>, String> {
        let signal = &self.signals[channel];
        if signal.kind.float && require_known_count {
            return Ok(None);
        }
        let mut budget = MAX_INTEGRAL_SAMPLES;
        signal.sum_at_level(reader, range, signal.top_level(), cancel, &mut budget)
    }
}

impl Signal {
    fn definition(id: u16, bytes: &[u8]) -> Result<Option<Self>, String> {
        if bytes.len() < 128 {
            return Err("Truncated Joulescope signal definition".into());
        }
        if bytes[2] != 0 {
            return Ok(None);
        }
        let Some(kind) = SampleType::parse(u32_at(bytes, 4)) else {
            return Ok(None);
        };
        let rate = u32_at(bytes, 8);
        let samples_per_chunk = u64::from(u32_at(bytes, 12));
        let sample_decimate = u64::from(u32_at(bytes, 16));
        let entries_per_summary = u64::from(u32_at(bytes, 20));
        let summary_decimate = u64::from(u32_at(bytes, 24));
        if rate == 0
            || samples_per_chunk == 0
            || samples_per_chunk > MAX_DATA_SAMPLES
            || sample_decimate == 0
            || samples_per_chunk % sample_decimate != 0
            || entries_per_summary == 0
            || entries_per_summary > MAX_TABLE_ENTRIES
            || entries_per_summary % (samples_per_chunk / sample_decimate) != 0
            || summary_decimate < 2
            || entries_per_summary % summary_decimate != 0
        {
            return Err("Invalid Joulescope fixed-rate index parameters".into());
        }
        let [name, unit] = strings::<2>(bytes, 128)?;
        Ok(Some(Self {
            id,
            source: u16_at(bytes, 0),
            channel: Channel {
                name: if name.is_empty() {
                    format!("Signal {id}")
                } else {
                    name
                },
                unit,
                sample_count: 0,
                start_seconds: 0.0,
                interval_seconds: 1.0 / f64::from(rate),
            },
            kind,
            samples_per_chunk,
            sample_decimate,
            entries_per_summary,
            summary_decimate,
            offsets: [0; LEVELS],
            utc: 0,
            origin: 0,
            end_chunk: None,
        }))
    }

    fn first_utc(&self, reader: &Reader) -> Option<f64> {
        let chunk = Table::read(reader, self.utc, UTC_DATA, self.id).ok()?;
        let time = i64::from_le_bytes(reader.array(chunk.data_offset).ok()?);
        if chunk.count != 1 || chunk.bits != 64 || chunk.data_bytes < 8 {
            return None;
        }
        let samples = (i128::from(chunk.sample) - i128::from(self.origin)) as f64;
        Some(time as f64 / (1u64 << 30) as f64 - samples * self.channel.interval_seconds)
    }

    fn step(&self, level: usize) -> u64 {
        match level {
            0 => 1,
            _ => self
                .sample_decimate
                .saturating_mul(self.summary_decimate.saturating_pow(level as u32 - 1)),
        }
    }

    fn child_step(&self, level: usize) -> u64 {
        match level {
            1 => self.samples_per_chunk,
            _ => self
                .step(level - 1)
                .saturating_mul(self.entries_per_summary),
        }
    }

    fn summary_level(&self, step: u64, length: u64) -> usize {
        let mut best = 0;
        for level in 1..LEVELS {
            if self.offsets[level] == 0 {
                continue;
            }
            let source_step = self.step(level);
            if source_step <= step || length.div_ceil(self.step(best)) > MAX_SUMMARY_BINS {
                best = level;
            }
        }
        best
    }

    fn top_level(&self) -> usize {
        self.offsets
            .iter()
            .rposition(|offset| *offset != 0)
            .unwrap_or(0)
    }

    fn table(&self, reader: &Reader, offset: u64, tag: u8, level: usize) -> Result<Table, String> {
        Table::read(reader, offset, tag, self.id | (level as u16) << 12)
    }

    fn data_table(&self, reader: &Reader, offset: u64) -> Result<Table, String> {
        let table = self.table(reader, offset, FSR_DATA, 0)?;
        if u64::from(table.bits) != self.kind.bits || table.count > self.samples_per_chunk {
            return Err("Invalid Joulescope data chunk size".into());
        }
        table.require_bytes((table.count * self.kind.bits).div_ceil(8))?;
        Ok(table)
    }

    fn index(&self, reader: &Reader, offset: u64, level: usize) -> Result<Table, String> {
        let table = self.table(reader, offset, FSR_INDEX, level)?;
        if table.bits != 64 || table.count == 0 || table.count > MAX_TABLE_ENTRIES {
            return Err("Invalid Joulescope index size".into());
        }
        table.require_bytes(table.count * 8)?;
        Ok(table)
    }

    fn find_extent(&mut self, reader: &Reader) -> Result<(), String> {
        let first = self.data_table(reader, self.offsets[0])?;
        self.origin = first.sample;
        let top = self.top_level();
        let mut offset = self.offsets[top];
        let mut summarized_end = None;
        for level in (1..=top).rev() {
            let index = self.index(reader, offset, level)?;
            offset = index.offset_at(reader, index.count - 1)?;
            if level == 1 && offset == 0 {
                let summary = self.table(reader, index.chunk.end, FSR_SUMMARY, level)?;
                let start = self.relative(summary.sample)?;
                summarized_end = start.checked_add(summary.count * self.step(level));
            }
        }
        if let Some(end) = summarized_end {
            // A final constant chunk may be represented entirely by summaries.
            self.channel.sample_count = end;
            return Ok(());
        }
        let mut last = self.data_table(reader, offset)?;
        for _ in 0..MAX_LINKS {
            if last.chunk.next == 0 {
                let start = self.relative(last.sample)?;
                self.channel.sample_count = start
                    .checked_add(last.count)
                    .ok_or("Joulescope sample count overflow")?;
                self.end_chunk = Some(last);
                return Ok(());
            }
            last = self.data_table(reader, last.chunk.forward()?)?;
        }
        Err("Joulescope recording is missing its terminal index".into())
    }

    fn relative(&self, sample: i64) -> Result<u64, String> {
        u64::try_from(i128::from(sample) - i128::from(self.origin))
            .map_err(|_| "Invalid Joulescope sample timestamp".into())
    }

    fn seek(&self, reader: &Reader, target: u64, level: usize) -> Result<u64, String> {
        let top = self.top_level();
        let mut offset = self.offsets[top];
        for current in ((level + 1)..=top).rev() {
            let index = self.index(reader, offset, current)?;
            let start = self.relative(index.sample)?;
            let entry = target
                .checked_sub(start)
                .ok_or("Joulescope index timestamp is after the requested sample")?
                / self.child_step(current);
            // A partial last chunk can extend past the summary's nominal grid.
            offset = index.offset_at(reader, entry.min(index.count - 1))?;
        }
        Ok(offset)
    }

    fn samples(
        &self,
        reader: &Reader,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        let mut output = Vec::with_capacity((range.end - range.start) as usize);
        let mut at = range.start;
        let mut chunk = None;
        while at < range.end {
            check_cancel(cancel)?;
            let data = match chunk.take() {
                Some(data) => Some(data),
                None => self.locate_data(reader, at)?,
            };
            if let Some(data) = data {
                let start = self.relative(data.sample)?;
                let end = start
                    .checked_add(data.count)
                    .ok_or("Joulescope data timestamp overflows")?;
                if at < start || at >= end {
                    return Err("Joulescope index points outside the sample window".into());
                }
                let read_end = end.min(range.end);
                self.kind
                    .decode(reader, &data, at - start..read_end - start, &mut output)?;
                at = read_end;
                if at < range.end && data.chunk.next != 0 {
                    let next = self.data_table(reader, data.chunk.forward()?)?;
                    if self.relative(next.sample)? == at {
                        chunk = Some(next);
                    }
                }
            } else {
                at = self.read_omitted(reader, at..range.end, &mut output)?;
            }
        }
        Ok(output)
    }

    fn locate_data(&self, reader: &Reader, at: u64) -> Result<Option<Table>, String> {
        if let Some(last) = self.end_chunk
            && at >= self.relative(last.sample)?
        {
            return Ok(Some(last));
        }
        let offset = self.seek(reader, at, 0)?;
        if offset == 0 {
            return Ok(None);
        }
        self.data_table(reader, offset).map(Some)
    }

    fn read_omitted(
        &self,
        reader: &Reader,
        range: Range<u64>,
        output: &mut Vec<f32>,
    ) -> Result<u64, String> {
        let offset = self.seek(reader, range.start, 1)?;
        let index = self.index(reader, offset, 1)?;
        let summary = self.table(reader, index.chunk.end, FSR_SUMMARY, 1)?;
        let index_start = self.relative(index.sample)?;
        let start = self.relative(summary.sample)?;
        if index_start != start {
            return Err("Joulescope omitted data has mismatched index and summary".into());
        }
        let sample = range
            .start
            .checked_sub(start)
            .ok_or("Invalid summary timestamp")?;
        let end = range.end.min(
            range
                .start
                .saturating_add(self.samples_per_chunk - sample % self.samples_per_chunk),
        );
        let bins = sample / self.sample_decimate..(end - start).div_ceil(self.sample_decimate);
        let entries = SummaryEntries::read(reader, &summary, bins.clone())?;
        let mut at = range.start;
        for (index, peak) in bins.zip(entries.peaks(self.kind.scale)) {
            let until = (start + (index + 1) * self.sample_decimate).min(end);
            let value = if peak.min == peak.max {
                peak.min
            } else {
                f32::NAN
            };
            output.resize(output.len() + (until - at) as usize, value);
            at = until;
        }
        if at != end {
            return Err("Omitted Joulescope data has no matching summary".into());
        }
        Ok(at)
    }

    fn sum_at_level(
        &self,
        reader: &Reader,
        range: Range<u64>,
        level: usize,
        cancel: &AtomicBool,
        budget: &mut u64,
    ) -> Result<Option<f64>, String> {
        let length = range.end - range.start;
        if length == 0 {
            return Ok(Some(0.0));
        }
        if level == 0 {
            let Some(remaining) = budget.checked_sub(length) else {
                return Ok(None);
            };
            *budget = remaining;
            return raw_sum(&range, |range| self.samples(reader, range, cancel));
        }

        let grid = self.step(level);
        if self.offsets[level] == 0 || grid > length {
            return self.sum_at_level(reader, range, level - 1, cancel, budget);
        }
        let first = range.start.next_multiple_of(grid);
        let end = range.end - range.end % grid;
        if first >= end {
            return self.sum_at_level(reader, range, level - 1, cancel, budget);
        }

        let mut total = 0.0;
        let covered = self.visit_summary(reader, level, first..end, cancel, |_, entries| {
            total += entries.means(self.kind.scale).sum::<f64>() * grid as f64;
        })?;
        if !total.is_finite() {
            return Ok(None);
        }
        let left = self.sum_at_level(reader, range.start..first, level - 1, cancel, budget)?;
        let right = self.sum_at_level(reader, covered..range.end, level - 1, cancel, budget)?;
        Ok(left.zip(right).map(|(left, right)| left + total + right))
    }

    fn visit_summary(
        &self,
        reader: &Reader,
        level: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
        mut visit: impl FnMut(u64, &SummaryEntries),
    ) -> Result<u64, String> {
        if self.offsets[level] == 0 {
            return Ok(range.start);
        }
        let offset = self.seek(reader, range.start, level)?;
        let index = self.index(reader, offset, level)?;
        let mut offset = index.chunk.end;
        let step = self.step(level);
        let mut at = range.start;
        let mut budget = MAX_SUMMARY_BINS;
        for _ in 0..MAX_LINKS {
            check_cancel(cancel)?;
            let table = self.table(reader, offset, FSR_SUMMARY, level)?;
            let start = self.relative(table.sample)?;
            let Some(distance) = at.checked_sub(start) else {
                break;
            };
            if distance % step != 0 {
                break;
            }
            let first = distance / step;
            let entries = SummaryEntries::entry_count(&table)?;
            let end = entries
                .min((range.end - start).div_ceil(step))
                .min(first.saturating_add(budget));
            if first >= end {
                break;
            }
            let entries = SummaryEntries::read(reader, &table, first..end)?;
            visit(at, &entries);
            at = at.saturating_add((end - first).saturating_mul(step));
            budget -= end - first;
            if at >= range.end || budget == 0 || end < table.count || table.chunk.next == 0 {
                break;
            }
            offset = table.chunk.forward()?;
        }
        Ok(at.min(range.end))
    }
}

fn walk_definitions(
    reader: &Reader,
    mut offset: u64,
    mut visit: impl FnMut(Chunk) -> Result<(), String>,
) -> Result<(), String> {
    for _ in 0..MAX_METADATA_CHUNKS {
        if offset == 0 {
            return Ok(());
        }
        let chunk = Chunk::read(reader, offset)?;
        visit(chunk)?;
        offset = chunk.forward()?;
    }
    Err("Joulescope definition chain exceeds its limit".into())
}

fn strings<const N: usize>(payload: &[u8], offset: usize) -> Result<[String; N], String> {
    let mut bytes = payload
        .get(offset..)
        .ok_or("Truncated Joulescope definition")?;
    let mut output = std::array::from_fn(|_| String::new());
    for name in &mut output {
        let nul = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("Unterminated Joulescope label")?;
        *name = String::from_utf8_lossy(&bytes[..nul]).into_owned();
        bytes = &bytes[nul + 1..];
        if bytes.first() == Some(&0x1f) {
            bytes = &bytes[1..];
        }
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy)]
struct SampleType {
    bits: u64,
    scale: f64,
    signed: bool,
    float: bool,
}

impl SampleType {
    fn parse(kind: u32) -> Option<Self> {
        let base = kind & 15;
        let bits = u64::from((kind >> 8) & 255);
        let q = (kind >> 16) as i8;
        let valid = match base {
            1 => matches!(bits, 4 | 8 | 16 | 24 | 32 | 64),
            3 => matches!(bits, 1 | 4 | 8 | 16 | 24 | 32 | 64),
            4 => matches!(bits, 32 | 64) && q == 0,
            _ => false,
        };
        valid.then(|| Self {
            bits,
            scale: 2.0f64.powi(-i32::from(q)),
            signed: base == 1,
            float: base == 4,
        })
    }

    fn decode(
        self,
        reader: &Reader,
        table: &Table,
        range: Range<u64>,
        output: &mut Vec<f32>,
    ) -> Result<(), String> {
        let first_bit = range.start * self.bits;
        let end_bit = range.end * self.bits;
        let offset = first_bit / 8;
        let bytes = reader.bytes(
            table.data_offset + offset,
            (end_bit.div_ceil(8) - offset) as usize,
        )?;
        match (self.float, self.bits) {
            (true, 32) => output.extend(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .copied()
                    .map(f32::from_le_bytes),
            ),
            (true, 64) => output.extend(
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|&value| f64::from_le_bytes(value) as f32),
            ),
            (false, 1 | 4) => {
                let bit_offset = first_bit % 8;
                let mask = (1 << self.bits) - 1;
                output.extend((0..(range.end - range.start)).map(|index| {
                    let bit = bit_offset + index * self.bits;
                    let raw = (bytes[(bit / 8) as usize] >> (bit % 8)) & mask;
                    self.integer(u64::from(raw))
                }));
            }
            _ => {
                let size = (self.bits / 8) as usize;
                output.extend(bytes.chunks_exact(size).map(|value| {
                    let mut raw = [0; 8];
                    raw[..size].copy_from_slice(value);
                    self.integer(u64::from_le_bytes(raw))
                }));
            }
        }
        Ok(())
    }

    fn integer(self, raw: u64) -> f32 {
        let value = if self.signed {
            let shift = 64 - self.bits;
            (((raw << shift) as i64) >> shift) as f64
        } else {
            raw as f64
        };
        (value * self.scale) as f32
    }
}

struct SummaryEntries {
    bytes: Vec<u8>,
    width: usize,
}

impl SummaryEntries {
    fn entry_count(table: &Table) -> Result<u64, String> {
        if !matches!(table.bits, 128 | 256) || table.count > MAX_TABLE_ENTRIES {
            return Err("Invalid Joulescope summary entry size".into());
        }
        let size = u64::from(table.bits) / 8;
        if table.data_bytes >= table.count * size {
            return Ok(table.count);
        }
        // Older writers saved only a prefix of each 64-bit summary chunk.
        if size == 32 && table.data_bytes == table.count * 16 {
            return Ok(table.count / 2);
        }
        Err("Truncated Joulescope summary chunk".into())
    }

    fn read(reader: &Reader, table: &Table, entries: Range<u64>) -> Result<Self, String> {
        if entries.end > Self::entry_count(table)? {
            return Err("Joulescope summary window extends beyond available data".into());
        }
        let size = u64::from(table.bits) / 8;
        let bytes = reader.bytes(
            table.data_offset + entries.start * size,
            ((entries.end - entries.start) * size) as usize,
        )?;
        Ok(Self {
            bytes,
            width: size as usize,
        })
    }

    fn peaks(&self, scale: f64) -> impl Iterator<Item = Peak> + '_ {
        self.bytes.chunks_exact(self.width).map(move |entry| {
            let peak = Peak {
                min: (Self::value(entry, 2) * scale) as f32,
                max: (Self::value(entry, 3) * scale) as f32,
            };
            if peak.valid() { peak } else { Peak::default() }
        })
    }

    fn means(&self, scale: f64) -> impl Iterator<Item = f64> + '_ {
        self.bytes
            .chunks_exact(self.width)
            .map(move |entry| Self::value(entry, 0) * scale)
    }

    fn value(entry: &[u8], index: usize) -> f64 {
        if entry.len() == 16 {
            f64::from(f32::from_bits(u32_at(entry, index * 4)))
        } else {
            f64::from_bits(u64_at(entry, index * 8))
        }
    }
}
