//! Sequence-numbered Rigol scope exports with range reads by row.

use super::{Channel, check_cancelled};
use std::{
    cmp::Ordering,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    ops::Range,
    os::unix::fs::FileExt,
    sync::atomic::AtomicBool,
};

const MAX_ROW_BYTES: usize = 512;
const BAD_ROWS: &str = "Rigol CSV samples are not a contiguous sequence.";

pub(super) struct Header<'a> {
    names: Vec<&'a str>,
    units: Vec<&'static str>,
    start_seconds: f64,
    interval_seconds: f64,
    data_start: u64,
}

impl<'a> Header<'a> {
    pub(super) fn recognize(prefix: &'a [u8]) -> Option<Self> {
        let mut lines = prefix.split_inclusive(|byte| *byte == b'\n');
        let (names_line, units_line) = (lines.next()?, lines.next()?);
        let data_start = (names_line.len() + units_line.len()) as u64;
        let names = fields(csv_line(names_line)?.trim_start_matches('\u{feff}'));
        let units = fields(csv_line(units_line)?);
        let channel_count = names.len().checked_sub(3)?;
        if !valid_labels(&names, &units, channel_count) {
            return None;
        }
        let names_end = channel_count + 1;
        let start_seconds = units[names_end].parse::<f64>().ok()?;
        let interval_seconds = units[names_end + 1].parse::<f64>().ok()?;
        if !start_seconds.is_finite() || !interval_seconds.is_finite() || interval_seconds <= 0.0 {
            return None;
        }
        Some(Self {
            names: names[1..names_end].to_vec(),
            units: units[1..names_end]
                .iter()
                .map(|name| voltage_unit(name))
                .collect::<Option<_>>()?,
            start_seconds,
            interval_seconds,
            data_start,
        })
    }

    pub(super) fn open(self, file: &File, file_len: u64) -> Result<(Vec<Channel>, Rows), String> {
        let (last, data_end) = last_sample(file, self.data_start..file_len)?;
        let sample_count = last.checked_add(1).ok_or(BAD_ROWS)?;
        if sample_count > data_end - self.data_start
            || !(sample_count as f64 * self.interval_seconds + self.start_seconds).is_finite()
        {
            return Err(BAD_ROWS.into());
        }
        let rows = Rows {
            data_start: self.data_start,
            data_end,
        };
        if sample_at(file, &rows, rows.data_start)?.0 != 0 {
            return Err(BAD_ROWS.into());
        }
        let channels = self
            .names
            .into_iter()
            .zip(self.units)
            .map(|(name, unit)| Channel {
                name: name.into(),
                unit: unit.into(),
                sample_count,
                start_seconds: self.start_seconds,
                interval_seconds: self.interval_seconds,
            })
            .collect();
        Ok((channels, rows))
    }
}

fn valid_labels(names: &[&str], units: &[&str], channel_count: usize) -> bool {
    if !(1..=4).contains(&channel_count)
        || names.len() != units.len()
        || names.first() != Some(&"X")
        || units.first() != Some(&"Sequence")
        || !names.ends_with(&["Start", "Increment"])
    {
        return false;
    }
    let mut previous = 0;
    for name in &names[1..channel_count + 1] {
        let Some(channel) = name.strip_prefix("CH").and_then(|v| v.parse::<u8>().ok()) else {
            return false;
        };
        if channel <= previous || channel > 4 {
            return false;
        }
        previous = channel;
    }
    true
}

fn fields(line: &str) -> Vec<&str> {
    line.trim_end_matches(',')
        .split(',')
        .map(str::trim)
        .collect()
}

fn voltage_unit(unit: &str) -> Option<&'static str> {
    match unit {
        "Volt" => Some("V"),
        "Ampere" => Some("A"),
        "Watt" => Some("W"),
        _ => None,
    }
}

fn csv_line(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()?
        .strip_suffix('\n')
        .map(str::trim)
}

fn last_sample(file: &File, data: Range<u64>) -> Result<(u64, u64), String> {
    let offset = data
        .end
        .saturating_sub(MAX_ROW_BYTES as u64)
        .max(data.start);
    let mut buffer = [0; MAX_ROW_BYTES];
    let bytes = &mut buffer[..(data.end - offset) as usize];
    file.read_exact_at(bytes, offset).map_err(read_error)?;
    let suffix_len = bytes.trim_ascii_end().len();
    let prefix = &bytes[..suffix_len];
    let row_start = after_line_break(prefix, offset == data.start)?;
    let sample = row_number(&prefix[row_start..])?;
    Ok((sample, offset + suffix_len as u64))
}

fn after_line_break(prefix: &[u8], starts_at_first_row: bool) -> Result<usize, String> {
    prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .or(starts_at_first_row.then_some(0))
        .ok_or_else(|| "Rigol CSV contains an overlong row.".into())
}

fn row_number(bytes: &[u8]) -> Result<u64, String> {
    let row_prefix = bytes.split(|byte| *byte == b',').next().ok_or(BAD_ROWS)?;
    std::str::from_utf8(row_prefix)
        .ok()
        .and_then(|field| field.trim().parse().ok())
        .ok_or_else(|| BAD_ROWS.to_owned())
}

/// Byte window containing the CSV samples, delimited by the header and final row.
#[derive(Debug)]
pub(super) struct Rows {
    data_start: u64,
    data_end: u64,
}

impl Rows {
    pub(super) fn read_samples(
        &self,
        file: &File,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        check_cancelled(cancel)?;
        if range.is_empty() {
            return Ok(Vec::new());
        }
        let offset = self.locate(file, range.start, cancel)?;
        let window = PositionedReader {
            file,
            offset,
            end: self.data_end,
        };
        let mut reader = BufReader::with_capacity(64 * 1024, window);
        let mut row = String::with_capacity(128);
        let mut result = Vec::with_capacity((range.end - range.start) as usize);
        for expected in range {
            if expected.is_multiple_of(4096) {
                check_cancelled(cancel)?;
            }
            row.clear();
            let length = reader
                .by_ref()
                .take((MAX_ROW_BYTES + 1) as u64)
                .read_line(&mut row)
                .map_err(read_error)?;
            if length == 0 || length > MAX_ROW_BYTES {
                return Err(BAD_ROWS.into());
            }
            result.push(sample_value(&row, expected, channel)?);
        }
        check_cancelled(cancel)?;
        Ok(result)
    }

    fn locate(&self, file: &File, index: u64, cancel: &AtomicBool) -> Result<u64, String> {
        if index == 0 {
            return Ok(self.data_start);
        }
        let mut search = self.data_start..self.data_end;
        // Sequence numbers locate a row without parsing its predecessors.
        while search.start < search.end {
            check_cancelled(cancel)?;
            let middle = search.start + (search.end - search.start) / 2;
            let (sample, row) = sample_at(file, self, middle)?;
            match sample.cmp(&index) {
                Ordering::Equal => return Ok(row.start),
                Ordering::Less => search.start = row.end,
                Ordering::Greater => search.end = row.start,
            }
        }
        Err(BAD_ROWS.into())
    }
}

fn sample_at(file: &File, rows: &Rows, middle: u64) -> Result<(u64, Range<u64>), String> {
    let offset = middle
        .saturating_sub(MAX_ROW_BYTES as u64)
        .max(rows.data_start);
    let last = middle
        .saturating_add(MAX_ROW_BYTES as u64)
        .min(rows.data_end);
    let mut buffer = [0; 2 * MAX_ROW_BYTES];
    let bytes = &mut buffer[..(last - offset) as usize];
    file.read_exact_at(bytes, offset).map_err(read_error)?;
    let middle = (middle - offset) as usize;
    let row_start = after_line_break(&bytes[..middle], offset == rows.data_start)?;
    let (row_end, next) = match bytes[middle..].iter().position(|byte| *byte == b'\n') {
        Some(position) => (middle + position, middle + position + 1),
        None if last == rows.data_end => (bytes.len(), bytes.len()),
        None => return Err("Rigol CSV contains an overlong row.".into()),
    };
    if row_end - row_start > MAX_ROW_BYTES {
        return Err("Rigol CSV contains an overlong row.".into());
    }
    let sample = row_number(&bytes[row_start..row_end])?;
    Ok((sample, offset + row_start as u64..offset + next as u64))
}

fn sample_value(row: &str, expected: u64, channel: usize) -> Result<f32, String> {
    let mut fields = row.split(',').map(str::trim);
    let sequence = fields.next().and_then(|field| field.parse().ok());
    if sequence != Some(expected) {
        return Err(BAD_ROWS.into());
    }
    fields
        .nth(channel)
        .and_then(|field| field.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| "Rigol CSV contains an invalid channel value.".into())
}

fn read_error(error: io::Error) -> String {
    format!("Reading Rigol CSV. {error}")
}

struct PositionedReader<'a> {
    file: &'a File,
    offset: u64,
    end: u64,
}

impl Read for PositionedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let available = (self.end - self.offset).min(buffer.len() as u64) as usize;
        let read = self.file.read_at(&mut buffer[..available], self.offset)?;
        self.offset += read as u64;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};

    const FIXTURE: &[u8] = b"X,CH1,CH4,Start,Increment,\r\nSequence,Volt,Ampere,-0.005,2e-6\r\n";

    #[test]
    fn locates_rigol_sequence_rows_without_moving_the_retained_file_position() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(FIXTURE).unwrap();
        for index in 0..2_000 {
            writeln!(file, "{index},{},{}", index + 10, -index).unwrap();
        }
        // Instrument exports can have a final unterminated line.
        write!(file, "2000,2010,-2000").unwrap();
        file.seek(io::SeekFrom::Start(7)).unwrap();
        let header = Header::recognize(FIXTURE).unwrap();
        let (channels, rows) = header.open(&file, file.metadata().unwrap().len()).unwrap();
        assert_eq!(channels[0].sample_count, 2_001);
        assert_eq!(channels[0].start_seconds, -0.005);
        assert_eq!(channels[1].name, "CH4");
        assert_eq!(channels[1].unit, "A");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            rows.read_samples(&file, 1, 1_398..1_401, &cancel).unwrap(),
            [-1_398.0, -1_399.0, -1_400.0]
        );
        assert_eq!(
            rows.read_samples(&file, 0, 1_999..2_001, &cancel).unwrap(),
            [2_009.0, 2_010.0]
        );
        assert_eq!(file.stream_position().unwrap(), 7);
    }

    #[test]
    fn distinguishes_instrument_exports_from_generic_csv() {
        assert!(Header::recognize(FIXTURE).is_some());
        assert!(Header::recognize(b"X,CH1,Start,Increment\n2,3,4,5\n").is_none());
        assert!(Header::recognize(b"X,CH44,Start,Increment\nSequence,Volt,0,1\n").is_none());
        assert!(Header::recognize(b"X,CH1,Start,Increment\nSequence,Volt,0,0\n").is_none());
    }

    #[test]
    fn rejects_missing_sequence_rows_in_a_selection() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(FIXTURE).unwrap();
        file.write_all(b"0,1,2\n2,9,10\n").unwrap();
        let header = Header::recognize(FIXTURE).unwrap();
        let (_, rows) = header.open(&file, file.metadata().unwrap().len()).unwrap();
        assert!(
            rows.read_samples(&file, 0, 0..3, &AtomicBool::new(false))
                .is_err()
        );
    }
}
