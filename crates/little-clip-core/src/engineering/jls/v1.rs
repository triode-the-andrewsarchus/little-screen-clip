//! Legacy JS110 recordings following the format implemented by Jetperch's
//! Apache-2.0 pyjoulescope datafile, data_recorder, and calibration modules.
//!
//! https://github.com/jetperch/pyjoulescope

mod arrays;
mod raw;
#[cfg(test)]
mod tests;

use std::{
    ops::Range,
    sync::{Mutex, atomic::AtomicBool},
};

use serde_json::Value;

use super::{
    Channel, Envelope, Peak, RAW_ENVELOPE_SAMPLES, Reader, check_cancel, new_envelope,
    raw_envelope, raw_sum, u32_at,
};
use arrays::Arrays;
use raw::{Calibration, Chunk, check_header, raw_sample};

const FIELDS: [(&str, &str, &str); 6] = [
    ("current", "Current", "A"),
    ("voltage", "Voltage", "V"),
    ("power", "Power", "W"),
    ("current_range", "Current range", ""),
    ("current_lsb", "Current LSB", ""),
    ("voltage_lsb", "Voltage LSB", ""),
];
const MAX_HEADER_CHUNKS: usize = 128;
const MAX_BLOCKS: u64 = 1 << 18;
const MAX_BLOCK_CHUNKS: u64 = 1024;
const MAX_OVERVIEW_REDUCTIONS: u64 = 1 << 18;

#[derive(Debug)]
pub(super) struct Recording {
    pub(super) channels: Vec<Channel>,
    channel_fields: Vec<usize>,
    encoding: Encoding,
    geometry: Geometry,
    calibration: Option<Calibration>,
    index: Mutex<Index>,
}

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Raw,
    FloatArrays,
}

#[derive(Debug)]
struct Geometry {
    samples_per_tlv: u64,
    samples_per_block: u64,
    samples_per_reduction: u64,
    footer_offset: u64,
    arrays_reduction: bool,
    reduction_fields: Vec<usize>,
}

#[derive(Debug)]
struct Index {
    next: u64,
    blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug)]
struct Block {
    head: Chunk,
    end: Chunk,
}

struct Metadata {
    footer_offset: u64,
    first_block: u64,
    sample_count: u64,
    config: Value,
    calibration: Option<Calibration>,
}

impl Metadata {
    fn read(reader: &Reader) -> Result<Self, String> {
        let header = reader.array::<32>(0)?;
        check_header(&header, reader.len)?;
        let mut config = None;
        let mut sample_count = None;
        let mut first_block = None;
        let mut calibration = None;
        let mut footer_offset = 0;
        let mut offset = 32;
        for _ in 0..MAX_HEADER_CHUNKS {
            let chunk = Chunk::read(reader, offset)?;
            offset = chunk.end;
            match &chunk.tag {
                b"END" => break,
                b"CLS" if first_block.is_none() => {
                    let end = chunk.collection(reader)?;
                    first_block = Some(chunk.end);
                    footer_offset = end.payload - 8;
                    offset = end.end;
                }
                b"SUB" if first_block.is_none() && chunk.len <= 64 * 1024 => {
                    if let Some(value) = Calibration::parse(&chunk.payload(reader)?)? {
                        calibration = Some(value);
                    }
                }
                b"MJS" => {
                    let value: Value = serde_json::from_slice(&chunk.payload(reader)?)
                        .map_err(|error| format!("Decode legacy Joulescope metadata: {error}"))?;
                    match value["type"].as_str() {
                        Some("config") => config = Some(value),
                        Some("footer") => sample_count = value["size"].as_u64(),
                        _ => {}
                    }
                }
                _ => {}
            }
            if sample_count.is_some() && config.is_some() && first_block.is_some() {
                break;
            }
        }
        let sample_count = sample_count
            .filter(|count| *count > 0 && *count <= i64::MAX as u64)
            .ok_or("Legacy Joulescope recording has no sample count footer")?;
        Ok(Self {
            first_block: first_block.ok_or("Legacy Joulescope recording has no samples")?,
            footer_offset,
            sample_count,
            config: config.ok_or("Legacy Joulescope recording has no configuration")?,
            calibration,
        })
    }
}

impl Geometry {
    fn read(meta: &Metadata) -> Result<Self, String> {
        let config = &meta.config;
        let integer = |name| config.get(name).and_then(Value::as_u64).unwrap_or(0);
        let samples_per_tlv = integer("samples_per_tlv");
        let samples_per_block = integer("samples_per_block");
        let samples_per_reduction = integer("samples_per_reduction");
        if samples_per_tlv == 0
            || samples_per_tlv > (1 << 24)
            || samples_per_block == 0
            || samples_per_reduction == 0
            || !samples_per_tlv.is_multiple_of(samples_per_reduction)
            || !samples_per_block.is_multiple_of(samples_per_tlv)
            || samples_per_block / samples_per_tlv > MAX_BLOCK_CHUNKS
        {
            return Err("Invalid legacy Joulescope sample layout".into());
        }
        let arrays_reduction = match config["reduction_format"].as_str().unwrap_or("v1") {
            "v1" => false,
            "v2" => true,
            _ => return Err("Unsupported legacy Joulescope reduction format".into()),
        };
        let reduction_fields = match config["reduction_fields"].as_array() {
            Some(values) => values
                .iter()
                .map(|value| {
                    FIELDS
                        .iter()
                        .position(|field| value.as_str() == Some(field.0))
                        .unwrap_or(usize::MAX)
                })
                .collect(),
            None => vec![0, 1, 2],
        };
        if reduction_fields.len() > FIELDS.len() {
            return Err("Invalid legacy Joulescope reduction fields".into());
        }
        Ok(Self {
            samples_per_tlv,
            samples_per_block,
            samples_per_reduction,
            footer_offset: meta.footer_offset,
            arrays_reduction,
            reduction_fields,
        })
    }
}

impl Recording {
    pub(super) fn open(reader: &Reader) -> Result<Self, String> {
        let meta = Metadata::read(reader)?;
        let geometry = Geometry::read(&meta)?;
        let encoding = match meta.config["data_format"].as_str().unwrap_or("raw") {
            "raw" => Encoding::Raw,
            "float32_v2" => Encoding::FloatArrays,
            _ => return Err("Unsupported legacy Joulescope sample encoding".into()),
        };
        let interval_seconds = meta.config["sampling_frequency"]
            .as_f64()
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .map(f64::recip)
            .filter(|interval| interval.is_finite() && *interval > 0.0)
            .ok_or("Invalid legacy Joulescope sample rate")?;

        let first = Block::read(reader, meta.first_block, geometry.footer_offset)?;
        let first_arrays = match encoding {
            Encoding::Raw => None,
            Encoding::FloatArrays => {
                let samples = first.sample_chunk(reader, 0)?;
                Some(Arrays::open(reader, samples)?)
            }
        };
        let mut channels = Vec::new();
        let mut channel_fields = Vec::new();
        let uncalibrated = matches!(encoding, Encoding::Raw) && meta.calibration.is_none();
        for (field, &(_, mut name, mut unit)) in FIELDS.iter().enumerate() {
            if first_arrays
                .as_ref()
                .is_some_and(|arrays| !arrays.has((field as u8) << 4 | 1))
            {
                continue;
            }
            if uncalibrated {
                match field {
                    0 => (name, unit) = ("Raw current", "LSB"),
                    1 => (name, unit) = ("Raw voltage", "LSB"),
                    2 => continue,
                    _ => {}
                }
            }
            channel_fields.push(field);
            channels.push(Channel {
                name: name.into(),
                unit: unit.into(),
                sample_count: meta.sample_count,
                start_seconds: 0.0,
                interval_seconds,
            });
        }
        if channels.is_empty() {
            return Err("Legacy Joulescope recording has no supported waveform channels".into());
        }
        Ok(Self {
            channels,
            channel_fields,
            encoding,
            geometry,
            calibration: meta.calibration,
            index: Mutex::new(Index {
                next: first.end.end,
                blocks: vec![first],
            }),
        })
    }

    pub(super) fn model(&self) -> &str {
        self.calibration
            .as_ref()
            .map_or("Joulescope JS110", |value| &value.model)
    }

    fn block(&self, reader: &Reader, index: u64, cancel: &AtomicBool) -> Result<Block, String> {
        if index >= MAX_BLOCKS {
            return Err("Legacy Joulescope seek exceeds its block index limit".into());
        }
        let mut cache = self.index.lock().map_err(|error| error.to_string())?;
        while cache.blocks.len() <= index as usize {
            check_cancel(cancel)?;
            let block = Block::read(reader, cache.next, self.geometry.footer_offset)?;
            cache.blocks.push(block);
            cache.next = block.end.end;
        }
        Ok(cache.blocks[index as usize])
    }

    pub(super) fn read_samples(
        &self,
        reader: &Reader,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        let geometry = &self.geometry;
        let field = self.channel_fields[channel];
        let mut output = Vec::with_capacity((range.end - range.start) as usize);
        let mut at = range.start;
        while at < range.end {
            check_cancel(cancel)?;
            let block = self.block(reader, at / geometry.samples_per_block, cancel)?;
            let tlv_index = (at % geometry.samples_per_block) / geometry.samples_per_tlv;
            let start_in_tlv = at % geometry.samples_per_tlv;
            let take = (range.end - at).min(geometry.samples_per_tlv - start_in_tlv);
            let tlv_range = start_in_tlv..start_in_tlv + take;
            let chunk = block.sample_chunk(reader, tlv_index)?;
            match self.encoding {
                Encoding::Raw => {
                    let voltage_range = block.voltage_range(reader)?;
                    if !chunk.is_plain() || tlv_range.end * 4 > chunk.len {
                        return Err("Legacy Joulescope raw sample payload is incomplete".into());
                    }
                    let bytes =
                        reader.bytes(chunk.payload + tlv_range.start * 4, take as usize * 4)?;
                    output.extend(bytes.as_chunks::<4>().0.iter().map(|pair| {
                        raw_sample(pair, self.calibration.as_ref(), field, voltage_range)
                    }));
                }
                Encoding::FloatArrays => {
                    let arrays = Arrays::open(reader, chunk)?;
                    let samples = arrays.read(reader, (field as u8) << 4 | 1, tlv_range)?;
                    let scale = match field {
                        3 => 1.0 / 16.0,
                        4 | 5 => 1.0 / 15.0,
                        _ => 1.0,
                    };
                    output.extend(samples.into_iter().map(|sample| sample * scale));
                }
            }
            at += take;
        }
        Ok(output)
    }

    pub(super) fn envelope(
        &self,
        reader: &Reader,
        channel: usize,
        range: Range<u64>,
        columns: u64,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let field = self.channel_fields[channel];
        let geometry = &self.geometry;
        let length = range.end - range.start;
        let desired_step = length.div_ceil(columns).max(1);
        let grid = geometry.samples_per_reduction;
        let read = |range| self.read_samples(reader, channel, range, cancel);
        let reduction_field = geometry
            .reduction_fields
            .iter()
            .position(|index| *index == field);
        let use_reduction = if (matches!(self.encoding, Encoding::Raw)
            && self.calibration.is_none())
            || length.div_ceil(grid) > MAX_OVERVIEW_REDUCTIONS
            || (desired_step < grid && length <= RAW_ENVELOPE_SAMPLES)
        {
            None
        } else {
            reduction_field
        };
        let Some(reduction_field) = use_reduction else {
            let mut result = new_envelope(range.start, range.end, desired_step);
            raw_envelope(&mut result, range, read)?;
            return Ok(result);
        };

        let start = range.start - range.start % grid;
        let step = desired_step.div_ceil(grid) * grid;
        let mut result = new_envelope(start, range.end, step);
        let end = start
            .saturating_add(step.saturating_mul(result.peaks.len() as u64))
            .min(self.channels[channel].sample_count);
        let mut at = start;
        while at < end {
            check_cancel(cancel)?;
            let block_index = at / geometry.samples_per_block;
            let block = self.block(reader, block_index, cancel)?;
            let block_start = block_index * geometry.samples_per_block;
            let until = block_start
                .saturating_add(geometry.samples_per_block)
                .min(end);
            let first = (at - block_start) / grid;
            let stop = (until - block_start) / grid;
            let peaks = self.reductions(reader, block.end, reduction_field, first..stop)?;
            for peak in peaks {
                result.peaks[((at - start) / step) as usize].merge(peak);
                at += grid;
            }
            raw_envelope(&mut result, at..until, read)?;
            at = until;
        }
        Ok(result)
    }

    pub(super) fn integral(
        &self,
        reader: &Reader,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Option<f64>, String> {
        let geometry = &self.geometry;
        let field = self.channel_fields[channel];
        let grid = geometry.samples_per_reduction;
        if !geometry.arrays_reduction
            || (matches!(self.encoding, Encoding::Raw) && self.calibration.is_none())
            || (range.end - range.start).div_ceil(grid) > MAX_OVERVIEW_REDUCTIONS
            || !geometry.reduction_fields.contains(&field)
        {
            return Ok(None);
        }
        let start = range.start.next_multiple_of(grid).min(range.end);
        let end = (range.end - range.end % grid).max(start);
        let read = |range| self.read_samples(reader, channel, range, cancel);
        let Some(leading) = raw_sum(&(range.start..start), read)? else {
            return Ok(None);
        };
        let Some(trailing) = raw_sum(&(end..range.end), read)? else {
            return Ok(None);
        };
        let mut total = leading + trailing;
        let mut at = start;
        while at < end {
            check_cancel(cancel)?;
            let block_index = at / geometry.samples_per_block;
            let block_start = block_index * geometry.samples_per_block;
            let until = block_start
                .saturating_add(geometry.samples_per_block)
                .min(end);
            let stats = self.block(reader, block_index, cancel)?.end;
            let entries = (at - block_start) / grid..(until - block_start) / grid;
            let Some(sum) = reduction_sum(reader, stats, field, entries, grid)? else {
                return Ok(None);
            };
            total += sum;
            at = until;
        }
        Ok(Some(total))
    }

    fn reductions(
        &self,
        reader: &Reader,
        chunk: Chunk,
        column: usize,
        range: Range<u64>,
    ) -> Result<Vec<Peak>, String> {
        if range.is_empty() || chunk.len == 0 {
            return Ok(Vec::new());
        }
        if self.geometry.arrays_reduction {
            let arrays = Arrays::open(reader, chunk)?;
            let field = self.geometry.reduction_fields[column] as u8;
            let (min, max) = (field << 4 | 3, field << 4 | 4);
            let end = range.end.min(arrays.count);
            if end <= range.start || !arrays.has(min) || !arrays.has(max) {
                return Ok(Vec::new());
            }
            let minima = arrays.read(reader, min, range.start..end)?;
            let maxima = arrays.read(reader, max, range.start..end)?;
            return Ok(minima.into_iter().zip(maxima).map(peak).collect());
        }

        if !chunk.is_plain() {
            return Ok(Vec::new());
        }
        let stride = self.geometry.reduction_fields.len() as u64 * 16;
        let end = range.end.min(chunk.len / stride);
        if end <= range.start {
            return Ok(Vec::new());
        }
        let bytes = reader.bytes(
            chunk.payload + range.start * stride,
            ((end - range.start) * stride) as usize,
        )?;
        Ok(bytes
            .chunks_exact(stride as usize)
            .map(|row| {
                let offset = column * 16;
                peak((
                    f32::from_bits(u32_at(row, offset + 8)),
                    f32::from_bits(u32_at(row, offset + 12)),
                ))
            })
            .collect())
    }
}

fn reduction_sum(
    reader: &Reader,
    chunk: Chunk,
    field: usize,
    entries: Range<u64>,
    grid: u64,
) -> Result<Option<f64>, String> {
    if chunk.len == 0 {
        return Ok(None);
    }
    let arrays = Arrays::open(reader, chunk)?;
    let mean = (field as u8) << 4 | 1;
    if arrays.count < entries.end || !arrays.has(0) || !arrays.has(mean) {
        return Ok(None);
    }
    let counts = arrays.read(reader, 0, entries.clone())?;
    let means = arrays.read(reader, mean, entries)?;
    let mut total = 0.0;
    for (count, mean) in counts.into_iter().zip(means) {
        if count != grid as f32 || !mean.is_finite() {
            return Ok(None);
        }
        total += f64::from(mean) * f64::from(count);
    }
    Ok(Some(total))
}

fn peak((min, max): (f32, f32)) -> Peak {
    let peak = Peak { min, max };
    if peak.valid() { peak } else { Peak::default() }
}

impl Block {
    fn read(reader: &Reader, offset: u64, footer: u64) -> Result<Self, String> {
        if offset >= footer {
            return Err("Legacy Joulescope sample collection ends early".into());
        }
        let head = Chunk::read(reader, offset)?;
        let end = head.collection(reader)?;
        if end.end > footer {
            return Err("Legacy Joulescope sample collection overlaps its footer".into());
        }
        Ok(Self { head, end })
    }

    fn voltage_range(self, reader: &Reader) -> Result<usize, String> {
        if self.head.len == 12 {
            return Ok(0);
        }
        let payload = self.head.payload(reader)?;
        let value: Value = serde_json::from_slice(&payload[12..])
            .map_err(|error| format!("Decode legacy Joulescope block metadata: {error}"))?;
        match value["v_range"].as_u64().unwrap_or(0) {
            range @ 0..=1 => Ok(range as usize),
            _ => Err("Unsupported legacy Joulescope voltage range".into()),
        }
    }

    fn sample_chunk(self, reader: &Reader, index: u64) -> Result<Chunk, String> {
        let mut offset = self.head.end;
        let mut count = 0;
        for _ in 0..MAX_BLOCK_CHUNKS {
            if offset >= self.end.payload - 8 {
                break;
            }
            let chunk = Chunk::read(reader, offset)?;
            if chunk.end > self.end.payload - 8 {
                break;
            }
            if &chunk.tag == b"ABN" {
                if count == index {
                    return Ok(chunk);
                }
                count += 1;
            }
            offset = chunk.end;
        }
        Err("Legacy Joulescope sample block ends early".into())
    }
}
