//! Bounded reads of the legacy LeCroy WAVEDESC analog waveform layout.

use super::{
    Channel, Layout,
    binary::{ByteOrder, Encoding, Lane, fixed_text},
};
use std::{fs::File, ops::Range};

mod sequence;

const DESCRIPTOR_BYTES: usize = 346;
const SIGNATURE: &[u8] = b"WAVEDESC\0";

pub(super) fn matches_header(prefix: &[u8]) -> bool {
    descriptor(prefix).is_some()
}

fn descriptor(prefix: &[u8]) -> Option<(usize, &[u8])> {
    let offset = prefix
        .get(..prefix.len().min(64))?
        .windows(SIGNATURE.len())
        .position(|bytes| bytes == SIGNATURE)?;
    let header = prefix.get(offset..)?;
    let template = header.get(16..27)?;
    matches!(
        template,
        b"LECROY_2_2\0" | b"LECROY_2_3\0" | b"LECROY_2_4\0"
    )
    .then_some((offset, header))
}

pub(super) fn parse(file: &File, prefix: &[u8], file_len: u64) -> Result<Layout, String> {
    let (start, header) = descriptor(prefix).ok_or("Missing legacy LeCroy waveform descriptor")?;
    if header.len() < DESCRIPTOR_BYTES {
        return Err("Incomplete LeCroy waveform descriptor".into());
    }
    let (order, encoding) = sample_encoding(header)?;
    let blocks = Blocks::read(header, order, start as u64, file_len)?;
    let (segments, triggered) = blocks.windows(file, header, order, encoding)?;
    let scale = Calibration::read(header, order)?;

    let trace = match fixed_text(&header[96..112]) {
        name if name.is_empty() => "Trace".into(),
        name => name,
    };
    let instrument = fixed_text(&header[76..92]);
    let unit = match fixed_text(&header[196..244]).as_str() {
        "Volt" | "Volts" | "volts" => "V".into(),
        "Amp" | "Amps" | "amps" => "A".into(),
        unit => unit.to_owned(),
    };
    let stride = encoding.width();
    let mut lane = Lane {
        offset: blocks.first_data,
        stride,
        column: 0,
        order,
        encoding,
        zero: 0.0,
        gain: scale.gain,
        bias: scale.bias,
    };
    let model = if instrument.is_empty() {
        "LeCroy TRC".into()
    } else {
        format!("LeCroy TRC · {instrument}")
    };
    let mut channels = Vec::with_capacity(segments.len());
    let mut lanes = Vec::with_capacity(segments.len());
    for segment in segments {
        let count = segment.data.end - segment.data.start;
        let start_seconds = scale.start(&segment, count)?;
        let name = if triggered {
            format!("Segment {}", segment.index + 1)
        } else {
            trace.clone()
        };
        channels.push(Channel {
            name,
            unit: unit.clone(),
            sample_count: count,
            start_seconds,
            interval_seconds: scale.interval,
        });
        lane.offset = blocks.first_data + segment.data.start * stride as u64;
        lanes.push(lane.clone());
    }
    let mut layout = Layout::binary(model, channels, lanes);
    layout.triggered_segments = triggered;
    Ok(layout)
}

fn sample_encoding(header: &[u8]) -> Result<(ByteOrder, Encoding), String> {
    let order = match &header[34..36] {
        [0, 0] => ByteOrder::Big,
        [1, 0] => ByteOrder::Little,
        _ => return Err("Unrecognized LeCroy sample byte order".into()),
    };
    let kind = u16::from_le_bytes(order.array(&[header[32], header[33]]));
    let encoding = match kind {
        0 => Encoding::Signed8,
        1 => Encoding::Signed16,
        _ => return Err("LeCroy waveform samples must use signed byte or word encoding".into()),
    };
    Ok((order, encoding))
}

struct Calibration {
    gain: f64,
    bias: f64,
    interval: f64,
}

impl Calibration {
    fn read(header: &[u8], order: ByteOrder) -> Result<Self, String> {
        let horizontal = fixed_text(&header[244..292]);
        if !matches!(
            horizontal.to_ascii_lowercase().as_str(),
            "s" | "sec" | "seconds"
        ) {
            return Err("This LeCroy waveform has a non-time horizontal axis".into());
        }
        let measure = |offset| f64::from(f32::from_bits(order.u32(header, offset)));
        let gain = measure(156);
        let bias = -measure(160);
        let interval = measure(176);
        if !gain.is_finite()
            || gain == 0.0
            || !bias.is_finite()
            || !interval.is_finite()
            || interval <= 0.0
        {
            return Err("Invalid LeCroy waveform calibration".into());
        }
        Ok(Self {
            gain,
            bias,
            interval,
        })
    }

    fn start(&self, segment: &sequence::Segment, count: u64) -> Result<f64, String> {
        let first = (segment.local_first as f64).mul_add(self.interval, segment.origin);
        let end = (count as f64).mul_add(self.interval, first);
        if !first.is_finite()
            || !end.is_finite()
            || first + self.interval <= first
            || (count.saturating_sub(1) as f64).mul_add(self.interval, first) >= end
        {
            return Err("Invalid LeCroy sample time extent".into());
        }
        Ok(first)
    }
}

struct Blocks {
    first_data: u64,
    data_bytes: u64,
    triggers: Range<u64>,
    ris_timing: bool,
}

impl Blocks {
    fn read(header: &[u8], order: ByteOrder, start: u64, file_len: u64) -> Result<Self, String> {
        let mut sizes = [0_u64; 10];
        for (index, size) in sizes.iter_mut().enumerate() {
            let value = order.u32(header, 36 + 4 * index);
            *size = u64::try_from(value as i32).map_err(|_| "Invalid LeCroy block size")?;
        }
        if sizes[0] < DESCRIPTOR_BYTES as u64 || start + sizes.iter().sum::<u64>() > file_len {
            return Err("LeCroy waveform blocks exceed the available data".into());
        }
        let triggers_at = start + sizes[..3].iter().sum::<u64>();
        Ok(Self {
            first_data: start + sizes[..6].iter().sum::<u64>(),
            data_bytes: sizes[6],
            triggers: triggers_at..triggers_at + sizes[3],
            ris_timing: sizes[4] != 0,
        })
    }

    fn windows(
        &self,
        file: &File,
        header: &[u8],
        order: ByteOrder,
        encoding: Encoding,
    ) -> Result<(Vec<sequence::Segment>, bool), String> {
        if self.ris_timing {
            return Err("This LeCroy file uses interleaved RIS timing".into());
        }
        if order.u32(header, 132) != 0 || order.u32(header, 136) > 1 {
            return Err("This LeCroy file stores a partial or sparse waveform".into());
        }
        let count = u64::from(order.u32(header, 116));
        let window = sample_window(header, order, self, encoding, count)?;
        let segments = u64::from(order.u32(header, 144)).max(1);
        if segments > 1 || !self.triggers.is_empty() {
            let records =
                sequence::read(file, order, self.triggers.clone(), segments, count, window)?;
            return Ok((records, true));
        }
        Ok((
            vec![sequence::Segment {
                index: 0,
                local_first: window.start,
                data: window,
                origin: order.f64(header, 180),
            }],
            false,
        ))
    }
}

fn sample_window(
    header: &[u8],
    order: ByteOrder,
    blocks: &Blocks,
    encoding: Encoding,
    count: u64,
) -> Result<Range<u64>, String> {
    if count == 0 || count * encoding.width() as u64 != blocks.data_bytes {
        return Err("LeCroy sample count disagrees with its data block".into());
    }
    let first = u64::from(order.u32(header, 124));
    let last = u64::from(order.u32(header, 128));
    if first > last || last >= count {
        return Err("Invalid LeCroy valid sample window".into());
    }
    Ok(first..last + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engineering::scope::Capture,
        file_format::{self, Format},
    };
    use std::{fs::File, io::Write, sync::atomic::AtomicBool};

    #[test]
    fn detects_scope_traces_and_reads_only_the_calibrated_valid_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scope.trc");
        let data = trace();
        File::create(&path).unwrap().write_all(&data).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            file_format::recognize_engineering(&path, &cancel),
            Some(Format::Oscilloscope)
        );
        let capture = Capture::open(File::open(path).unwrap()).unwrap();
        let channel = &capture.channels()[0];
        assert_eq!(channel.sample_count, 3);
        assert_eq!(channel.start_seconds, -0.125);
        assert_eq!(channel.interval_seconds, 0.125);
        assert_eq!(channel.unit, "V");
        assert_eq!(
            capture.read_samples(0, 0..3, &cancel).unwrap(),
            [-2.0, 1.0, 3.0]
        );
    }

    #[test]
    fn rejects_truncated_sample_and_trigger_arrays() {
        let mut data = trace();
        let file = tempfile::tempfile().unwrap();
        assert!(parse(&file, &data, data.len() as u64 - 1).is_err());
        let start = data
            .windows(SIGNATURE.len())
            .position(|bytes| bytes == SIGNATURE)
            .unwrap();
        data[start + 144..start + 148].copy_from_slice(&2_u32.to_be_bytes());
        assert!(
            parse(&file, &data, data.len() as u64)
                .unwrap_err()
                .contains("sequence timing")
        );
    }

    #[test]
    fn preserves_sequence_spacing_and_trimmed_samples_per_trigger() {
        let cancel = AtomicBool::new(false);
        let mut bytes = trace();
        let start = bytes
            .windows(SIGNATURE.len())
            .position(|bytes| bytes == SIGNATURE)
            .unwrap();
        let samples = bytes.split_off(start + DESCRIPTOR_BYTES);
        for (offset, value) in [(48, 32), (60, 20), (116, 10), (128, 8), (144, 2)] {
            bytes[start + offset..start + offset + 4].copy_from_slice(&u32::to_be_bytes(value));
        }
        for timing in [0.0_f64, -0.25, 2.0, -0.25] {
            bytes.extend(timing.to_be_bytes());
        }
        bytes.extend_from_slice(&samples);
        bytes.extend_from_slice(&samples);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        let capture = Capture::open(file).unwrap();
        assert!(capture.triggered_segments());
        let channels = capture.channels();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].start_seconds, -0.125);
        assert_eq!(channels[1].start_seconds, 1.75);
        assert_eq!(channels[0].sample_count, 4);
        assert_eq!(channels[1].sample_count, 4);
        assert_eq!(capture.read_samples(0, 0..2, &cancel).unwrap(), [-2.0, 1.0]);
        assert_eq!(
            capture.read_samples(1, 0..2, &cancel).unwrap(),
            [-101.0, -2.0]
        );
    }

    fn trace() -> Vec<u8> {
        let mut bytes = b"#9000000356".to_vec();
        let mut header = [0; DESCRIPTOR_BYTES];
        header[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
        header[16..27].copy_from_slice(b"LECROY_2_3\0");
        header[32..34].copy_from_slice(&1_u16.to_be_bytes());
        for (offset, value) in [
            (36, 346),
            (60, 10),
            (116, 5),
            (124, 1),
            (128, 3),
            (136, 1),
            (144, 1),
        ] {
            header[offset..offset + 4].copy_from_slice(&u32::to_be_bytes(value));
        }
        header[156..160].copy_from_slice(&0.5_f32.to_be_bytes());
        header[160..164].copy_from_slice(&1.0_f32.to_be_bytes());
        header[176..180].copy_from_slice(&0.125_f32.to_be_bytes());
        header[180..188].copy_from_slice(&(-0.25_f64).to_be_bytes());
        header[196] = b'V';
        header[244] = b'S';
        bytes.extend(header);
        for value in [-200_i16, -2, 4, 8, 300] {
            bytes.extend(value.to_be_bytes());
        }
        bytes
    }
}
