//! Positioned reads of saved Keysight and Agilent analog BIN waveforms.

use super::{
    Channel, Layout,
    binary::{ByteOrder, Encoding, Lane, fixed_text},
};
use std::{fs::File, os::unix::fs::FileExt};

const FILE_HEADER: usize = 12;
const WAVEFORM_HEADER: usize = 140;
const BUFFER_HEADER: usize = 12;
const ORDER: ByteOrder = ByteOrder::Little;
const MAX_WAVEFORMS: u32 = 64;
const MAX_BUFFERS: u32 = 16;
const MAX_CHANNELS: usize = 32;

pub(super) fn matches_header(prefix: &[u8]) -> bool {
    matches!(prefix.get(..4), Some(b"AG01" | b"AG03" | b"AG10"))
}

pub(super) fn parse(file: &File, prefix: &[u8], file_len: u64) -> Result<Layout, String> {
    let header = prefix
        .get(..FILE_HEADER)
        .ok_or("Incomplete Keysight BIN header")?;
    let count = ORDER.u32(header, 8);
    if count == 0 || count > MAX_WAVEFORMS || u64::from(ORDER.u32(header, 4)) != file_len {
        return Err("Invalid or oversized Keysight BIN file".into());
    }
    let mut source = Source {
        file,
        file_len,
        cursor: FILE_HEADER as u64,
    };
    let mut model = None;
    let mut channels = Vec::new();
    let mut lanes = Vec::new();
    for index in 0..count {
        let waveform = source.waveform(index)?;
        for _ in 0..waveform.buffers {
            let buffer = source.buffer()?;
            if let Some((channel, lane)) = waveform.analog(buffer)? {
                if channels.len() == MAX_CHANNELS {
                    return Err("Keysight BIN exceeds the analog channel limit".into());
                }
                model.get_or_insert_with(|| waveform.model.clone());
                channels.push(channel);
                lanes.push(lane);
            }
        }
    }
    if source.cursor != file_len {
        return Err("Keysight BIN buffer lengths do not account for the file".into());
    }
    let Some(model) = model else {
        return Err("Keysight BIN has no supported single-acquisition analog time channels".into());
    };
    let model = if model.is_empty() {
        "Keysight BIN".into()
    } else {
        format!("Keysight BIN · {model}")
    };
    Ok(Layout::binary(model, channels, lanes))
}

struct Source<'a> {
    file: &'a File,
    cursor: u64,
    file_len: u64,
}

impl Source<'_> {
    fn waveform(&mut self, index: u32) -> Result<Waveform, String> {
        let header = self.read::<WAVEFORM_HEADER>()?;
        let size = u64::from(ORDER.u32(&header, 0));
        self.advance(size, WAVEFORM_HEADER as u64)?;
        Waveform::read(&header, index)
    }

    fn buffer(&mut self) -> Result<Buffer, String> {
        let header = self.read::<BUFFER_HEADER>()?;
        let size = u64::from(ORDER.u32(&header, 0));
        self.advance(size, BUFFER_HEADER as u64)?;
        let buffer = Buffer {
            offset: self.cursor,
            kind: u16::from_le_bytes([header[4], header[5]]),
            stride: u16::from_le_bytes([header[6], header[7]]),
            bytes: u64::from(ORDER.u32(&header, 8)),
        };
        self.advance(buffer.bytes, 0)?;
        Ok(buffer)
    }

    fn read<const N: usize>(&self) -> Result<[u8; N], String> {
        if N as u64 > self.file_len.saturating_sub(self.cursor) {
            return Err("Incomplete Keysight BIN record header".into());
        }
        let mut bytes = [0; N];
        self.file
            .read_exact_at(&mut bytes, self.cursor)
            .map_err(|error| format!("Reading Keysight BIN header. {error}"))?;
        Ok(bytes)
    }

    fn advance(&mut self, size: u64, minimum: u64) -> Result<(), String> {
        if size < minimum || size > self.file_len - self.cursor {
            return Err("Keysight BIN record exceeds the available data".into());
        }
        self.cursor += size;
        Ok(())
    }
}

struct Waveform {
    settings: Channel,
    model: String,
    kind: u32,
    buffers: u32,
    time_domain: bool,
}

struct Buffer {
    offset: u64,
    kind: u16,
    stride: u16,
    bytes: u64,
}

impl Waveform {
    fn read(header: &[u8; WAVEFORM_HEADER], index: u32) -> Result<Self, String> {
        let buffers = ORDER.u32(header, 8);
        if buffers == 0 || buffers > MAX_BUFFERS {
            return Err("Invalid Keysight BIN buffer count".into());
        }
        let label = fixed_text(&header[112..128]);
        let name = match label.parse::<u32>() {
            Ok(channel) => format!("CH{channel}"),
            Err(_) if label.is_empty() => format!("Waveform {}", index + 1),
            Err(_) => label,
        };
        let unit = match ORDER.u32(header, 52) {
            1 => "V",
            2 => "s",
            3 => "",
            4 => "A",
            5 => "dB",
            _ => "Unknown",
        };
        let frame = fixed_text(&header[88..112]);
        let model = frame
            .split(':')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        Ok(Self {
            settings: Channel {
                name,
                unit: unit.into(),
                sample_count: u64::from(ORDER.u32(header, 12)),
                start_seconds: ORDER.f64(header, 40),
                interval_seconds: ORDER.f64(header, 32),
            },
            model,
            kind: ORDER.u32(header, 4),
            buffers,
            time_domain: ORDER.u32(header, 48) == 2
                && ORDER.u32(header, 136) == 0
                && ORDER.f64(header, 128) == 0.0,
        })
    }

    fn analog(&self, buffer: Buffer) -> Result<Option<(Channel, Lane)>, String> {
        let suffix = match (self.kind, buffer.kind) {
            (1 | 3, 1) => "",
            (2, 2) => " maximum",
            (2, 3) => " minimum",
            _ => return Ok(None),
        };
        if !self.time_domain {
            return Ok(None);
        }
        let mut channel = self.settings.clone();
        let encoding = Encoding::Float32;
        let stride = encoding.width();
        let count = channel.sample_count;
        if count == 0
            || u64::from(buffer.stride) != stride as u64
            || buffer.bytes != count * stride as u64
        {
            return Err("Keysight BIN sample count disagrees with its data buffer".into());
        }
        let end = channel.time_at(count);
        if !channel.start_seconds.is_finite() || !end.is_finite() || end <= channel.start_seconds {
            return Err("Invalid Keysight BIN sample timing".into());
        }
        channel.name.push_str(suffix);
        Ok(Some((
            channel,
            Lane {
                offset: buffer.offset,
                stride,
                column: 0,
                order: ORDER,
                encoding,
                zero: 0.0,
                gain: 1.0,
                bias: 0.0,
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engineering::scope::Capture,
        file_format::{self, Format},
    };
    use std::{io::Write, sync::atomic::AtomicBool};

    #[test]
    fn reads_normal_and_peak_buffers_and_skips_unrelated_digital_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scope.bin");
        let bytes = fixture();
        std::fs::write(&path, bytes).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            file_format::recognize_engineering(&path, &cancel),
            Some(Format::Oscilloscope)
        );
        let capture = Capture::open(File::open(path).unwrap()).unwrap();
        assert_eq!(capture.model(), "Keysight BIN · Test scope");
        let names = capture
            .channels()
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["CH1", "CH2 maximum", "CH2 minimum"]);
        assert_eq!(capture.channels()[0].start_seconds, -0.125);
        assert_eq!(capture.channels()[0].interval_seconds, 0.125);
        assert_eq!(capture.channels()[0].unit, "V");
        assert_eq!(capture.read_samples(0, 0..2, &cancel).unwrap(), [-2.0, 4.0]);
        assert_eq!(capture.read_samples(1, 1..2, &cancel).unwrap(), [7.0]);
        assert_eq!(
            capture.read_samples(2, 0..2, &cancel).unwrap(),
            [-3.0, -4.0]
        );
    }

    #[test]
    fn rejects_a_buffer_claiming_more_data_than_exists() {
        let mut bytes = fixture();
        bytes[FILE_HEADER + WAVEFORM_HEADER + 8..FILE_HEADER + WAVEFORM_HEADER + 12]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        assert!(Capture::open(file).unwrap_err().contains("record exceeds"));
    }

    fn fixture() -> Vec<u8> {
        let mut bytes = b"AG10".to_vec();
        bytes.extend([0; 4]);
        bytes.extend(3_u32.to_le_bytes());
        waveform(&mut bytes, b"1", 1, 1);
        floats(&mut bytes, 1, [-2.0, 4.0]);
        waveform(&mut bytes, b"EXT", 6, 1);
        buffer(&mut bytes, 6, 1, &[0, 1]);
        waveform(&mut bytes, b"2", 2, 2);
        floats(&mut bytes, 2, [5.0, 7.0]);
        floats(&mut bytes, 3, [-3.0, -4.0]);
        let len = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&len.to_le_bytes());
        bytes
    }

    fn waveform(bytes: &mut Vec<u8>, label: &[u8], kind: u32, buffers: u32) {
        let mut header = [0; WAVEFORM_HEADER];
        for (offset, value) in [
            (0, WAVEFORM_HEADER as u32),
            (4, kind),
            (8, buffers),
            (12, 2),
            (48, 2),
            (52, 1),
        ] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        header[32..40].copy_from_slice(&0.125_f64.to_le_bytes());
        header[40..48].copy_from_slice(&(-0.125_f64).to_le_bytes());
        header[88..103].copy_from_slice(b"Test scope:0001");
        header[112..112 + label.len()].copy_from_slice(label);
        bytes.extend(header);
    }

    fn floats(bytes: &mut Vec<u8>, kind: u16, values: [f32; 2]) {
        let data = values
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        buffer(bytes, kind, 4, &data);
    }

    fn buffer(bytes: &mut Vec<u8>, kind: u16, stride: u16, data: &[u8]) {
        bytes.extend((BUFFER_HEADER as u32).to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend(stride.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
    }
}
