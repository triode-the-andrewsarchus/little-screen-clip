//! Bounded reads of the legacy LeCroy WAVEDESC analog waveform layout.

use super::{
    Channel, Layout,
    binary::{ByteOrder, Encoding, Lane},
};
use std::ops::Range;

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

pub(super) fn parse(prefix: &[u8], file_len: u64) -> Result<Layout, String> {
    let (start, header) = descriptor(prefix).ok_or("Missing legacy LeCroy waveform descriptor")?;
    if header.len() < DESCRIPTOR_BYTES {
        return Err("Incomplete LeCroy waveform descriptor".into());
    }
    let (order, encoding) = sample_encoding(header)?;
    let blocks = Blocks::read(header, order, start as u64, file_len)?;
    if blocks.timing || order.u32(header, 144) > 1 {
        return Err("This LeCroy file contains segmented or interleaved acquisitions".into());
    }
    if order.u32(header, 132) != 0 || order.u32(header, 136) > 1 {
        return Err("This LeCroy file stores a partial or sparse waveform".into());
    }
    let window = sample_window(header, order, &blocks, encoding)?;
    let scale = Calibration::read(header, order, &window)?;

    let name = text(&header[96..112]);
    let instrument = text(&header[76..92]);
    let unit = match text(&header[196..244]).as_str() {
        "Volt" | "Volts" | "volts" => "V".into(),
        "Amp" | "Amps" | "amps" => "A".into(),
        unit => unit.to_owned(),
    };
    let channel = Channel {
        name: if name.is_empty() {
            "Trace".into()
        } else {
            name
        },
        unit,
        sample_count: window.end - window.start,
        start_seconds: scale.first,
        interval_seconds: scale.interval,
    };
    let stride = encoding.width();
    let lane = Lane {
        offset: blocks.first_data + window.start * stride as u64,
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
    Ok(Layout::binary(model, vec![channel], vec![lane]))
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
    first: f64,
}

impl Calibration {
    fn read(header: &[u8], order: ByteOrder, window: &Range<u64>) -> Result<Self, String> {
        let horizontal = text(&header[244..292]);
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
        let origin = order.f64(header, 180);
        let first = (window.start as f64).mul_add(interval, origin);
        let end = (window.end as f64).mul_add(interval, origin);
        if !gain.is_finite()
            || gain == 0.0
            || !bias.is_finite()
            || interval <= 0.0
            || !first.is_finite()
            || !end.is_finite()
            || end <= first
        {
            return Err("Invalid LeCroy waveform calibration".into());
        }
        Ok(Self {
            gain,
            bias,
            interval,
            first,
        })
    }
}

struct Blocks {
    first_data: u64,
    data_bytes: u64,
    timing: bool,
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
        Ok(Self {
            first_data: start + sizes[..6].iter().sum::<u64>(),
            data_bytes: sizes[6],
            timing: sizes[3] != 0 || sizes[4] != 0,
        })
    }
}

fn sample_window(
    header: &[u8],
    order: ByteOrder,
    blocks: &Blocks,
    encoding: Encoding,
) -> Result<Range<u64>, String> {
    let count = u64::from(order.u32(header, 116));
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

fn text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
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
    fn rejects_a_truncated_sample_array_and_a_segmented_time_axis() {
        let mut data = trace();
        assert!(parse(&data, data.len() as u64 - 1).is_err());
        let start = data
            .windows(SIGNATURE.len())
            .position(|bytes| bytes == SIGNATURE)
            .unwrap();
        data[start + 144..start + 148].copy_from_slice(&2_u32.to_be_bytes());
        assert!(
            parse(&data, data.len() as u64)
                .unwrap_err()
                .contains("segmented")
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
