//! Analog waveform exports from Siglent SDS1000X-E oscilloscopes.
//!
//! The V1 layout follows Siglent's "How to Extract Data from the Binary File
//! of SIGLENT Oscilloscope," including the two documented unit fields. The
//! USB stick export has no magic, so recognition validates the entire header
//! profile and its declared extent where a file length is available.

use super::{
    Channel,
    binary::{ByteOrder, Encoding, Lane},
};

const V1_HEADER_BYTES: usize = 0x11c;
const V1_DATA_START: u64 = 0x800;
const MAGNITUDES: [f64; 14] = [
    1e-24, 1e-21, 1e-18, 1e-15, 1e-12, 1e-9, 1e-6, 1e-3, 1.0, 1e3, 1e6, 1e9, 1e12, 1e15,
];

pub(super) fn matches_header(prefix: &[u8], file_len: u64) -> bool {
    open(prefix, file_len).is_ok()
}

pub(super) fn open(
    prefix: &[u8],
    file_len: u64,
) -> Result<(Vec<Channel>, Vec<Lane>), &'static str> {
    if prefix.len() < V1_HEADER_BYTES {
        return Err("Siglent waveform header is truncated.");
    }
    let analog_on = enabled_channels(prefix, 0, 4)?;
    let sample_count = u64::from(u32_at(prefix, 0xf4));
    validate_extent(prefix, analog_on, sample_count, file_len)?;
    let (start_seconds, interval_seconds) = sample_grid(prefix, sample_count)?;
    let channel_count = analog_on.count_ones() as usize;
    let mut channels = Vec::with_capacity(channel_count);
    let mut lanes = Vec::with_capacity(channel_count);
    let mut data_start = V1_DATA_START;
    for slot in 0..4 {
        if analog_on & (1 << slot) == 0 {
            continue;
        }
        let vertical_offset = 16 * slot;
        let (division, unit) = match u32_at(prefix, 0x1c + vertical_offset) {
            0 => (0, "V"),
            1 => (1, "A"),
            _ => return Err("Unsupported Siglent channel unit."),
        };
        let gain = positive_scaled(prefix, 0x10 + vertical_offset, division)? / 25.0;
        let bias = scaled(prefix, 0x50 + vertical_offset, division)?;
        channels.push(Channel {
            name: format!("CH{}", slot + 1),
            unit: unit.into(),
            sample_count,
            start_seconds,
            interval_seconds,
        });
        lanes.push(Lane {
            offset: data_start,
            stride: 1,
            column: 0,
            order: ByteOrder::Little,
            encoding: Encoding::Unsigned8,
            zero: 128.0,
            gain,
            bias,
        });
        data_start += sample_count;
    }
    Ok((channels, lanes))
}

fn validate_extent(
    prefix: &[u8],
    analog_on: u32,
    sample_count: u64,
    file_len: u64,
) -> Result<(), &'static str> {
    if analog_on == 0 || sample_count == 0 {
        return Err("Siglent waveform has no analog samples.");
    }
    let mut end = V1_DATA_START + u64::from(analog_on.count_ones()) * sample_count;
    let digital_on = enabled_channels(prefix, 0x90, 1)? != 0;
    if digital_on {
        let digital_channels = enabled_channels(prefix, 0x94, 16)?.count_ones();
        end += u64::from(digital_channels) * u64::from(u32_at(prefix, 0x108));
    }
    if file_len != end {
        return Err("Siglent waveform sample length disagrees with the header.");
    }
    Ok(())
}

fn sample_grid(prefix: &[u8], sample_count: u64) -> Result<(f64, f64), &'static str> {
    let time_division = positive_scaled(prefix, 0xd4, 14)?;
    let _trigger_delay = scaled(prefix, 0xe4, 14)?;
    let sample_rate_unit = u32_at(prefix, 0x104);
    if !matches!(sample_rate_unit, 12 | 15) {
        return Err("Siglent waveform has an invalid sample rate unit.");
    }
    let interval = positive_scaled(prefix, 0xf8, sample_rate_unit)?.recip();
    // The V1 time grid is relative to screen center. The documented
    // calculation does not apply the separately recorded trigger delay.
    let start = -7.0 * time_division;
    if start.is_finite() && interval > 0.0 && (start + interval * sample_count as f64).is_finite() {
        Ok((start, interval))
    } else {
        Err("Siglent waveform has an invalid time grid.")
    }
}

fn enabled_channels(prefix: &[u8], start: usize, count: usize) -> Result<u32, &'static str> {
    let mut mask = 0;
    for slot in 0..count {
        match u32_at(prefix, start + 4 * slot) {
            0 => {}
            1 => mask |= 1 << slot,
            _ => return Err("Siglent waveform has invalid channel flags."),
        }
    }
    Ok(mask)
}

fn positive_scaled(prefix: &[u8], offset: usize, unit: u32) -> Result<f64, &'static str> {
    let value = scaled(prefix, offset, unit)?;
    if value <= 0.0 {
        return Err("Siglent waveform has a nonpositive sample scale.");
    }
    Ok(value)
}

fn scaled(prefix: &[u8], offset: usize, unit: u32) -> Result<f64, &'static str> {
    let magnitude = u32_at(prefix, offset + 8) as usize;
    let multiplier = MAGNITUDES
        .get(magnitude)
        .ok_or("Siglent waveform has an invalid magnitude.")?;
    let value = f64::from_le_bytes(prefix[offset..offset + 8].try_into().unwrap()) * multiplier;
    if u32_at(prefix, offset + 12) != unit || !value.is_finite() {
        return Err("Siglent waveform has an invalid scaled field.");
    }
    Ok(value)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut data = vec![0; V1_DATA_START as usize + 8];
        data[..4].copy_from_slice(&1_u32.to_le_bytes());
        data[12..16].copy_from_slice(&1_u32.to_le_bytes());
        data[0xf4..0xf8].copy_from_slice(&4_u32.to_le_bytes());
        for (offset, raw, magnitude, unit) in [
            (0x10, 250.0_f64, 7_u32, 0_u32),
            (0x40, 1.0, 8, 1),
            (0x50, 0.2, 8, 0),
            (0x80, 0.0, 8, 1),
            (0xd4, 2.0, 6, 14),
            (0xe4, 1.0, 6, 14),
            (0xf8, 0.5, 10, 15),
        ] {
            data[offset..offset + 8].copy_from_slice(&raw.to_le_bytes());
            data[offset + 8..offset + 12].copy_from_slice(&magnitude.to_le_bytes());
            data[offset + 12..offset + 16].copy_from_slice(&unit.to_le_bytes());
        }
        data
    }

    #[test]
    fn reads_planar_channels_with_magnitude_and_time_scale() {
        let bytes = fixture();
        assert!(matches_header(&bytes, bytes.len() as u64));
        let (channels, lanes) = open(&bytes, bytes.len() as u64).unwrap();
        assert_eq!(channels[0].name, "CH1");
        assert_eq!(channels[1].name, "CH4");
        assert_eq!(channels[1].unit, "A");
        assert_eq!(channels[1].sample_count, 4);
        assert!((channels[0].start_seconds + 14e-6).abs() < 1e-18);
        assert_eq!(channels[0].interval_seconds, 2e-6);
        assert_eq!(lanes[0].gain, 0.01);
        assert_eq!(lanes[0].bias, 0.2);
        assert_eq!(lanes[1].offset, V1_DATA_START + 4);
    }

    #[test]
    fn rejects_unrelated_binary_and_inconsistent_extents() {
        assert!(!matches_header(
            &[0; V1_HEADER_BYTES],
            V1_HEADER_BYTES as u64
        ));
        let mut bytes = fixture();
        assert!(open(&bytes, bytes.len() as u64 - 1).is_err());
        assert!(open(&bytes, bytes.len() as u64 + 1).is_err());
        bytes[0x104] = 99;
        assert!(!matches_header(&bytes, bytes.len() as u64));
    }
}
