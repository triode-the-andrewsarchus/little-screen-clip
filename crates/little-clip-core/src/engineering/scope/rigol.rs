//! Rigol DS1000Z-family analog waveform layout.
//!
//! Field layout and conversion checked against the RigolWFM reference parser
//! and its paired instrument exports.
//! https://github.com/scottprahl/RigolWFM/tree/9e6f993103f773f81df409fc65c2cc359d7bd249

use super::{
    Channel, Layout,
    binary::{ByteOrder, Encoding, Lane},
};

pub(super) const HEADER_BYTES: usize = 280;

const CHANNEL_MASK: usize = 88;
const CHANNEL_OFFSETS: usize = 92;
const LOGIC_OFFSET: usize = 108;
const ACQUISITION_MODE: usize = 112;
const SAMPLE_MODE: usize = 114;
const TIME_MODE: usize = 115;
const PAYLOAD_BYTES: usize = 116;
const SAMPLE_RATE_GHZ: usize = 120;
const CHANNEL_HEADERS: usize = 124;
const CHANNEL_HEADER_BYTES: usize = 28;
const HORIZONTAL_BYTES: usize = 256;
const HORIZONTAL_OFFSET: usize = 260;

pub(super) fn matches_header(prefix: &[u8]) -> bool {
    matches!(
        prefix,
        [0x01, 0xff, 0xff, 0xff, 0xa5 | 0xa6, 0xa5, 0x38, 0, ..]
    ) && prefix
        .get(8..28)
        .and_then(ascii_field)
        .is_some_and(model_supported)
}

fn model_supported(model: &str) -> bool {
    matches!(
        model,
        "DS1054Z"
            | "DS1074Z"
            | "DS1074Z-S"
            | "DS1104Z"
            | "DS1104Z-S"
            | "DS1104Z PLUS"
            | "DS1202Z"
            | "DS1202Z-E"
            | "MSO1054Z"
            | "MSO1074Z"
            | "MSO1104"
            | "MSO1104Z"
            | "MSO1104Z-S"
            | "MSO1104Z PLUS"
    )
}

pub(super) fn parse(prefix: &[u8], len: u64) -> Result<Layout, String> {
    let header = prefix
        .get(..HEADER_BYTES)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("Rigol waveform header is truncated.")?;
    let acquisition = acquisition(header, len)?;
    let channel_count = acquisition.enabled.count_ones() as usize;
    let firmware = ascii_field(&header[28..48]);
    let mut channels = Vec::with_capacity(channel_count);
    let mut lanes = Vec::with_capacity(channel_count);
    for number in (0..4).filter(|number| acquisition.enabled & (1 << number) != 0) {
        let lane_offset = u64::from(read_u32(header, CHANNEL_OFFSETS + number * 4));
        let descriptor = CHANNEL_HEADERS + number * CHANNEL_HEADER_BYTES;
        if lane_offset != acquisition.offset || header[descriptor] == 0 {
            return Err("Rigol channel metadata disagrees with the saved waveform.".into());
        }
        let (gain, bias) = calibration(header, descriptor, channel_count, firmware)?;
        let column = match acquisition.stride {
            4 => 3 - number,
            _ => acquisition.stride - 1 - lanes.len(),
        };
        channels.push(Channel {
            name: format!("CH{}", number + 1),
            unit: unit_name(header[descriptor + 17])?.into(),
            sample_count: acquisition.sample_count,
            start_seconds: acquisition.start_seconds,
            interval_seconds: acquisition.interval_seconds,
        });
        lanes.push(Lane {
            offset: acquisition.offset,
            stride: acquisition.stride,
            column,
            order: ByteOrder::Little,
            encoding: Encoding::Unsigned8,
            zero: 127.0,
            gain,
            bias,
        });
    }
    Ok(Layout::binary(
        ascii_field(&header[8..28]).unwrap_or_default().into(),
        channels,
        lanes,
    ))
}

struct Acquisition {
    enabled: u8,
    stride: usize,
    offset: u64,
    sample_count: u64,
    interval_seconds: f64,
    start_seconds: f64,
}

fn acquisition(header: &[u8; HEADER_BYTES], len: u64) -> Result<Acquisition, String> {
    if !matches_header(header) || header[48..50] != [1, 0] || header[84..86] != [0xd8, 0] {
        return Err("This oscilloscope waveform layout is unsupported.".into());
    }
    if read_u32(header, LOGIC_OFFSET) != 0
        || header[SAMPLE_MODE] != 0
        || header[ACQUISITION_MODE] == 1
        || header[TIME_MODE] != 0
    {
        return Err("This Rigol recording uses an unsupported acquisition layout.".into());
    }
    let enabled = header[CHANNEL_MASK] & 0x0f;
    let stride = (enabled.count_ones() as usize).next_power_of_two();
    let sample_rate = f64::from(read_f32(header, SAMPLE_RATE_GHZ)) * 1e9;
    if enabled == 0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err("Rigol capture lacks a valid channel or sample rate.".into());
    }
    let payload_len = u64::from(read_u32(header, PAYLOAD_BYTES));
    let offset = u64::from(read_u32(header, HORIZONTAL_OFFSET))
        + u64::from(read_u32(header, HORIZONTAL_BYTES));
    let sample_count = payload_len / stride as u64;
    if !valid_extent(offset, payload_len, stride, len) {
        return Err("Rigol capture has inconsistent or truncated sample data.".into());
    }

    let interval_seconds = 1.0 / sample_rate;
    let trigger_offset_seconds = i64::from_le_bytes(bytes_at(header, 72)) as f64 * 1e-12;
    let start_seconds = trigger_offset_seconds - (sample_count as f64 * interval_seconds * 0.5);
    Ok(Acquisition {
        enabled,
        stride,
        offset,
        sample_count,
        interval_seconds,
        start_seconds,
    })
}

fn valid_extent(offset: u64, payload_len: u64, stride: usize, file_len: u64) -> bool {
    offset >= HEADER_BYTES as u64
        && payload_len != 0
        && payload_len.is_multiple_of(stride as u64)
        && offset + payload_len == file_len
}

fn calibration(
    header: &[u8; HEADER_BYTES],
    descriptor: usize,
    channels: usize,
    firmware: Option<&str>,
) -> Result<(f64, f64), String> {
    let volts_per_division = f64::from(read_f32(header, descriptor + 8));
    let shift = f64::from(read_f32(header, descriptor + 12));
    if !volts_per_division.is_finite()
        || volts_per_division <= 0.0
        || !shift.is_finite()
        || volts_per_division > f64::from(f32::MAX) / 32.0
        || shift.abs() > f64::from(f32::MAX) / 2.0
    {
        return Err("Rigol channel calibration is invalid.".into());
    }

    let scale = if header[descriptor + 16] == 0 {
        volts_per_division
    } else {
        -volts_per_division
    };
    let gain = scale / 20.0;
    // Paired exports from SP3 use a different vertical offset for two channels.
    let vertical_bias = if firmware == Some("00.04.04.SP3") && channels == 2 {
        if shift < 0.0 { scale / 5.0 } else { 0.0 }
    } else {
        scale
    };
    Ok((gain, vertical_bias - shift))
}

fn unit_name(value: u8) -> Result<&'static str, String> {
    match value {
        0 => Ok("W"),
        1 => Ok("A"),
        2 => Ok("V"),
        3 => Ok(""),
        _ => Err("Rigol channel has an unknown unit.".into()),
    }
}

fn ascii_field(bytes: &[u8]) -> Option<&str> {
    let value = bytes.split(|byte| *byte == 0).next()?;
    if !value.is_ascii() {
        return None;
    }
    std::str::from_utf8(value).ok().map(str::trim)
}

fn read_u32(header: &[u8; HEADER_BYTES], offset: usize) -> u32 {
    u32::from_le_bytes(bytes_at(header, offset))
}

fn read_f32(header: &[u8; HEADER_BYTES], offset: usize) -> f32 {
    f32::from_le_bytes(bytes_at(header, offset))
}

fn bytes_at<const N: usize>(header: &[u8; HEADER_BYTES], offset: usize) -> [u8; N] {
    let mut result = [0; N];
    result.copy_from_slice(&header[offset..offset + N]);
    result
}
