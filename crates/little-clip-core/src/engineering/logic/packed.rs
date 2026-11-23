//! Ranged sample access for interleaved words and planar bit-packed channels.

use super::{Lane, Level, archive::Member};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

const READ_BLOCK: usize = 64 * 1024;

pub(super) struct Chunk {
    pub(super) first: u64,
    pub(super) end: u64,
    pub(super) member: Arc<Member>,
}

#[derive(Clone, Copy)]
pub(super) enum Packing {
    Words { width: usize, bit: usize },
    BitPlane,
}

pub(super) struct Stream {
    pub(super) chunks: Arc<[Chunk]>,
    pub(super) packing: Packing,
    pub(super) samples: u64,
    pub(super) rate: f64,
}

impl Stream {
    pub(super) fn sample_range(&self, lane: &Lane, time: &Range<f64>) -> Range<u64> {
        let samples = |seconds: f64| ((seconds - lane.start_seconds) * self.rate).max(0.0);
        let left = (samples(time.start).floor() as u64).min(self.samples.saturating_sub(1));
        let right = (samples(time.end).ceil() as u64)
            .max(left + 1)
            .min(self.samples);
        left..right
    }

    pub(super) fn time_at(&self, lane: &Lane, sample: u64) -> f64 {
        if sample >= self.samples {
            lane.end_seconds
        } else {
            lane.start_seconds + sample as f64 / self.rate
        }
    }

    pub(super) fn preview_range(&self, range: &Range<u64>, budget: u64) -> Range<u64> {
        let count = range.end - range.start;
        if count <= budget {
            return range.clone();
        }
        let first_chunk = self
            .chunks
            .partition_point(|chunk| chunk.first < range.start);
        // An independent archive block can reveal later activity without
        // inflating all the older blocks or seeking into a DEFLATE prefix.
        let start = self
            .chunks
            .get(first_chunk)
            .map(|chunk| chunk.first)
            .filter(|start| *start <= range.end - budget)
            .unwrap_or(range.start);
        start..start + budget
    }

    pub(super) fn scan(
        &self,
        range: Range<u64>,
        budget: &mut u64,
        cancel: &AtomicBool,
        mut observe: impl FnMut(u64, Level) -> bool,
    ) -> Result<u64, String> {
        let mut sample = range.start;
        if range.is_empty() {
            return Ok(sample);
        }
        let needed = self.packing.bytes(range.start..range.end);
        let mut bytes = vec![0; (needed.end - needed.start).min(READ_BLOCK as u64) as usize];
        let first = self.chunks.partition_point(|chunk| chunk.end <= sample);
        for chunk in &self.chunks[first..] {
            if sample >= range.end {
                break;
            }
            let end = range.end.min(chunk.end);
            while sample < end {
                let relative = sample - chunk.first;
                let count = self.packing.max_samples(relative).min(end - sample);
                let encoded = self.packing.bytes(relative..relative + count);
                let needed = (encoded.end - encoded.start) as usize;
                // Bit-plane alignment restarts at each member boundary.
                if bytes.len() < needed {
                    bytes.resize(needed, 0);
                }
                let block = &mut bytes[..needed];
                if !chunk
                    .member
                    .read_exact_at(block, encoded.start, budget, cancel)?
                {
                    return Ok(sample);
                }
                for index in 0..count {
                    let level = self.packing.level(block, relative, index);
                    if !observe(sample, level) {
                        return Ok(sample);
                    }
                    sample += 1;
                }
            }
        }
        Ok(sample)
    }
}

impl Packing {
    fn max_samples(self, start: u64) -> u64 {
        match self {
            Self::Words { width, .. } => (READ_BLOCK / width) as u64,
            Self::BitPlane => READ_BLOCK as u64 * 8 - start % 8,
        }
    }

    fn bytes(self, samples: Range<u64>) -> Range<u64> {
        match self {
            Self::Words { width, .. } => samples.start * width as u64..samples.end * width as u64,
            Self::BitPlane => samples.start / 8..samples.end.div_ceil(8),
        }
    }

    fn level(self, bytes: &[u8], relative_start: u64, sample: u64) -> Level {
        let high = match self {
            Self::Words { width, bit } => {
                bytes[sample as usize * width + bit / 8] & (1 << (bit % 8)) != 0
            }
            Self::BitPlane => {
                let bit = relative_start % 8 + sample;
                bytes[bit as usize / 8] & (1 << (bit % 8)) != 0
            }
        };
        if high { Level::High } else { Level::Low }
    }
}
