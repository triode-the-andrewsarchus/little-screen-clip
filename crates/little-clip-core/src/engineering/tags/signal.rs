use super::{
    Error,
    source::{Checkpoints, cancelled},
};
use std::{
    io::{BufRead, Read, Seek, SeekFrom},
    ops::Range,
    sync::atomic::AtomicBool,
};

const PREVIEW_SAMPLES: usize = 4096;
const PREVIEW_BYTES: u64 = 64 * 1024;
const MAX_LINE: u64 = 64;
const MAX_WINDOW: u64 = 16384;
const MAX_BINS: usize = 2048;

#[derive(Clone, Debug)]
pub struct SignalPreview {
    pub samples: Vec<i16>,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleBin {
    pub min: i16,
    pub max: i16,
}

impl SampleBin {
    fn merge(&mut self, other: Self) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }
}

#[derive(Clone, Debug)]
pub struct SignalIndex {
    offsets: Checkpoints,
    samples: u64,
    pub bins: Vec<SampleBin>,
    pub bin_samples: u64,
}

impl SignalIndex {
    pub fn samples(&self) -> u64 {
        self.samples
    }

    fn push(&mut self, value: i16) {
        let peak = SampleBin {
            min: value,
            max: value,
        };
        if self.samples.is_multiple_of(self.bin_samples) {
            if self.bins.len() == MAX_BINS {
                self.bin_samples *= 2;
                for slot in 0..MAX_BINS / 2 {
                    let mut merged = self.bins[slot * 2];
                    merged.merge(self.bins[slot * 2 + 1]);
                    self.bins[slot] = merged;
                }
                self.bins.truncate(MAX_BINS / 2);
            }
            self.bins.push(peak);
        } else if let Some(last) = self.bins.last_mut() {
            last.merge(peak);
        }
        self.samples += 1;
    }
}

pub(super) fn recognizes(prefix: &[u8], whole: bool) -> bool {
    let end = if whole {
        prefix.len()
    } else {
        prefix
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |index| index + 1)
    };
    let Ok(text) = std::str::from_utf8(&prefix[..end]) else {
        return false;
    };
    let mut samples = 0;
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_LINE as usize || parse(line).is_err() {
            return false;
        }
        samples += 1;
    }
    samples >= if whole { 1 } else { 4 }
}

pub(super) fn preview(
    input: &mut (impl BufRead + Seek),
    cancel: &AtomicBool,
) -> Result<SignalPreview, Error> {
    let length = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(0))?;
    let mut reader = Lines::new(input, 0);
    let limit = length.min(PREVIEW_BYTES);
    let mut samples = Vec::with_capacity(PREVIEW_SAMPLES);
    while samples.len() < PREVIEW_SAMPLES {
        let Some((_, value)) = reader.next(limit, limit < length, cancel)? else {
            break;
        };
        samples.push(value);
    }
    if samples.is_empty() {
        return Err(Error::Invalid("too few Proxmark samples"));
    }
    Ok(SignalPreview {
        samples,
        complete: reader.offset == length,
    })
}

pub fn build_signal_index(
    mut input: impl BufRead + Seek,
    cancel: &AtomicBool,
) -> Result<SignalIndex, Error> {
    let length = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(0))?;
    let mut reader = Lines::new(input, 0);
    let mut index = SignalIndex {
        offsets: Checkpoints::new(512),
        samples: 0,
        bins: Vec::new(),
        bin_samples: 1,
    };
    while let Some((offset, value)) = reader.next(length, false, cancel)? {
        index.offsets.record(index.samples, offset);
        index.push(value);
    }
    if index.samples == 0 {
        return Err(Error::Invalid("too few Proxmark samples"));
    }
    Ok(index)
}

pub fn read_samples(
    mut input: impl BufRead + Seek,
    index: &SignalIndex,
    rows: Range<u64>,
    cancel: &AtomicBool,
) -> Result<Vec<i16>, Error> {
    let end = rows.end.min(index.samples);
    let count = end.saturating_sub(rows.start);
    if count > MAX_WINDOW {
        return Err(Error::Limit("requested signal samples"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let [first, offset] = index.offsets.before(rows.start)?;
    let length = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(offset))?;
    let mut reader = Lines::new(input, offset);
    let mut samples = Vec::with_capacity(count as usize);
    for row in first..end {
        let (_, value) = reader
            .next(length, false, cancel)?
            .ok_or(Error::Truncated)?;
        if row >= rows.start {
            samples.push(value);
        }
    }
    Ok(samples)
}

struct Lines<R> {
    input: R,
    offset: u64,
    line: Vec<u8>,
}

impl<R: BufRead> Lines<R> {
    fn new(input: R, offset: u64) -> Self {
        Self {
            input,
            offset,
            line: Vec::with_capacity(MAX_LINE as usize),
        }
    }

    fn next(
        &mut self,
        limit: u64,
        partial: bool,
        cancel: &AtomicBool,
    ) -> Result<Option<(u64, i16)>, Error> {
        while self.offset < limit {
            cancelled(cancel)?;
            let current = self.offset;
            let budget = (limit - current).min(MAX_LINE + 1);
            self.line.clear();
            let size = (&mut self.input)
                .take(budget)
                .read_until(b'\n', &mut self.line)?;
            if size == 0 {
                return Ok(None);
            }
            if size as u64 > MAX_LINE {
                return Err(Error::Limit("signal sample line"));
            }
            self.offset += size as u64;
            if partial && self.offset == limit && self.line.last() != Some(&b'\n') {
                return Ok(None);
            }
            let text =
                std::str::from_utf8(&self.line).map_err(|_| Error::Invalid("signal text"))?;
            let text = if current == 0 {
                text.trim_start_matches('\u{feff}').trim()
            } else {
                text.trim()
            };
            if !text.is_empty() {
                return Ok(Some((current, parse(text)?)));
            }
        }
        Ok(None)
    }
}

fn parse(value: &str) -> Result<i16, Error> {
    value
        .parse()
        .ok()
        .filter(|sample| (-128..=255).contains(sample))
        .ok_or(Error::Invalid("expected an eight-bit sample value"))
}
