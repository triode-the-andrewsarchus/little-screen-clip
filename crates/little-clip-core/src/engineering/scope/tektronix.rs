//! Tektronix WFM#001 through WFM#003 scalar analog records.
//!
//! Header fields follow Tektronix's reference waveform format.
//! https://download.tek.com/manual/077022005Web_1.pdf
//! Time origin and user-data bounds are checked against `tm_data_types` fixtures.

use super::{
    Channel,
    binary::{ByteOrder, Encoding, Lane},
};

pub(super) const HEADER_BYTES: usize = 838;

#[derive(Clone, Copy)]
struct Header {
    order: ByteOrder,
    version: u8,
    shift: usize,
}

impl Header {
    fn recognize(prefix: &[u8]) -> Option<Self> {
        let order = match prefix.get(..2)? {
            [0x0f, 0x0f] => ByteOrder::Little,
            [0xf0, 0xf0] => ByteOrder::Big,
            _ => return None,
        };
        let version_at = if prefix.get(2) == Some(&b':') { 3 } else { 2 };
        let version = match prefix.get(version_at..version_at + 7)? {
            b"WFM#001" => 1,
            b"WFM#002" => 2,
            b"WFM#003" => 3,
            _ => return None,
        };
        // A normal version field includes a colon. Older files omit it.
        let shift = 3 - version_at;
        Some(Self {
            order,
            version,
            shift,
        })
    }

    fn u32(self, bytes: &[u8], offset: usize) -> u32 {
        self.order.u32(bytes, offset - self.shift)
    }

    fn f64(self, bytes: &[u8], offset: usize) -> f64 {
        self.order.f64(bytes, offset - self.shift)
    }

    fn offsets(self) -> (usize, usize, usize) {
        match self.version {
            1 => (166, 478, 790),
            2 => (168, 480, 792),
            _ => (168, 488, 808),
        }
    }
}

pub(super) fn matches_header(prefix: &[u8]) -> bool {
    Header::recognize(prefix).is_some()
}

pub(super) fn parse(bytes: &[u8], file_len: u64) -> Result<(Channel, Lane), String> {
    let header = Header::recognize(bytes).ok_or("Unrecognized Tektronix waveform header.")?;
    let (explicit, implicit, curve) = header.offsets();
    let header_end = curve + 30 - header.shift;
    if bytes.len() < header_end {
        return Err("Tektronix waveform header is truncated.".into());
    }
    validate_record(bytes, header, explicit)?;
    let sample_width = usize::from(bytes[15 - header.shift]);
    let encoding = curve_encoding(header, bytes, explicit, sample_width)?;
    let (data_start, sample_count) =
        sample_extent(bytes, header, curve, header_end, file_len, sample_width)?;

    let gain = header.f64(bytes, explicit);
    let bias = header.f64(bytes, explicit + 8);
    let interval_seconds = header.f64(bytes, implicit);
    // The implicit origin refers to the first visible sample. Precharge is
    // skipped in byte space without advancing the printed time origin.
    let start_seconds = header.f64(bytes, implicit + 8);
    if !valid_scale(gain, bias, start_seconds, interval_seconds, sample_count) {
        return Err("Tektronix waveform has invalid sample calibration.".into());
    }
    let name = header_text(bytes, 40 - header.shift, 32).unwrap_or("CH1");
    let unit = header_text(bytes, explicit + 20 - header.shift, 20).unwrap_or("");
    Ok((
        Channel {
            name: name.into(),
            unit: unit.into(),
            sample_count,
            start_seconds,
            interval_seconds,
        },
        Lane {
            offset: data_start,
            stride: sample_width,
            column: 0,
            order: header.order,
            encoding,
            zero: 0.0,
            gain,
            bias,
        },
    ))
}

fn validate_record(bytes: &[u8], header: Header, explicit: usize) -> Result<(), String> {
    if header.u32(bytes, 72) != 0 || header.u32(bytes, 78) != 0 || header.u32(bytes, 82) != 1 {
        return Err("This Tektronix waveform contains multiple acquisitions.".into());
    }
    if header.u32(bytes, 114) != 1
        || header.u32(bytes, 118) != 1
        || header.u32(bytes, 122) != 2
        || header.u32(bytes, explicit + 76) != 0
    {
        return Err("This Tektronix waveform is not a sampled scalar analog record.".into());
    }
    Ok(())
}

fn curve_encoding(
    header: Header,
    bytes: &[u8],
    explicit: usize,
    sample_width: usize,
) -> Result<Encoding, String> {
    use Encoding::*;
    let format = header.u32(bytes, explicit + 72);
    let encoding = match format {
        0 => Signed16,
        1 => Signed32,
        2 => Unsigned32,
        3 => Unsigned64,
        4 => Float32,
        5 => Float64,
        // These enum values follow the current vendor reader and real
        // WFM#003 fixtures. Older released documentation labels them differently.
        7 if header.version == 3 => Unsigned8,
        8 if header.version == 3 => Signed8,
        _ => return Err("Unsupported Tektronix sample encoding.".into()),
    };
    if encoding.width() != sample_width {
        return Err("Tektronix sample width disagrees with its encoding.".into());
    }
    Ok(encoding)
}

fn sample_extent(
    bytes: &[u8],
    header: Header,
    curve: usize,
    header_end: usize,
    file_len: u64,
    width: usize,
) -> Result<(u64, u64), String> {
    let base = u64::from(header.u32(bytes, 16));
    let offsets = [10, 14, 18, 22, 26].map(|offset| u64::from(header.u32(bytes, curve + offset)));
    let [_, user_start, user_end, _, allocated] = offsets;
    if base < header_end as u64
        || !offsets.is_sorted()
        || base + allocated > file_len
        || user_start >= user_end
        || !user_start.is_multiple_of(width as u64)
        || !user_end.is_multiple_of(width as u64)
    {
        return Err("Tektronix waveform has inconsistent or truncated sample data.".into());
    }
    Ok((base + user_start, (user_end - user_start) / width as u64))
}

fn valid_scale(gain: f64, bias: f64, start: f64, step: f64, count: u64) -> bool {
    gain.is_finite()
        && gain != 0.0
        && bias.is_finite()
        && start.is_finite()
        && step > 0.0
        && step.is_finite()
        && (start + step * count as f64).is_finite()
}

fn header_text(bytes: &[u8], start: usize, len: usize) -> Option<&str> {
    let field = bytes[start..start + len].split(|byte| *byte == 0).next()?;
    std::str::from_utf8(field)
        .ok()
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(version: u8, order: ByteOrder, colon: bool) -> Vec<u8> {
        let (explicit, implicit, curve, base) = match version {
            1 => (166, 478, 790, 820),
            2 => (168, 480, 792, 822),
            _ => (168, 488, 808, 838),
        };
        let omitted = usize::from(!colon);
        let mut bytes = vec![0; base - omitted + 12];
        bytes[..2].fill(match order {
            ByteOrder::Little => 0x0f,
            ByteOrder::Big => 0xf0,
        });
        if colon {
            bytes[2] = b':';
        }
        bytes[3 - omitted..10 - omitted].copy_from_slice(b"WFM#000");
        bytes[9 - omitted] = b'0' + version;
        bytes[15 - omitted] = 2;
        bytes[40 - omitted..43 - omitted].copy_from_slice(b"CH2");
        bytes[explicit + 20 - omitted] = b'A';
        for (offset, value) in [
            (16, (base - omitted) as u32),
            (82, 1),
            (114, 1),
            (118, 1),
            (122, 2),
            (curve + 14, 4),
            (curve + 18, 10),
            (curve + 22, 12),
            (curve + 26, 12),
        ] {
            bytes[offset - omitted..offset - omitted + 4]
                .copy_from_slice(&order.array(&value.to_le_bytes()));
        }
        for (offset, value) in [
            (explicit, 0.25_f64),
            (explicit + 8, 1.0),
            (implicit, 2e-9),
            (implicit + 8, -3e-9),
        ] {
            bytes[offset - omitted..offset - omitted + 8]
                .copy_from_slice(&order.array(&value.to_le_bytes()));
        }
        bytes[base - omitted + 4..base - omitted + 6]
            .copy_from_slice(&order.array(&(-6_i16).to_le_bytes()));
        bytes
    }

    #[test]
    fn recognizes_supported_versions_and_byte_orders() {
        for version in 1..=3 {
            for order in [ByteOrder::Little, ByteOrder::Big] {
                for colon in [false, true] {
                    let bytes = fixture(version, order, colon);
                    assert!(matches_header(&bytes));
                    let (channel, lane) = parse(&bytes, bytes.len() as u64).unwrap();
                    assert_eq!(channel.name, "CH2");
                    assert_eq!(channel.unit, "A");
                    assert_eq!(channel.sample_count, 3);
                    assert_eq!(channel.start_seconds, -3e-9);
                    assert_eq!(channel.interval_seconds, 2e-9);
                    let first = lane.offset as usize;
                    let raw = lane
                        .encoding
                        .decode(lane.order, &bytes[first..first + lane.stride]);
                    assert_eq!(raw * lane.gain + lane.bias, -0.5);
                }
            }
        }
    }

    #[test]
    fn rejects_partial_waveforms_and_multiple_acquisitions() {
        let mut bytes = fixture(3, ByteOrder::Little, true);
        let len = bytes.len() as u64;
        assert!(parse(&bytes[..35], len).is_err());
        assert!(parse(&bytes, len - 1).is_err());
        bytes[82] = 2;
        assert!(parse(&bytes, len).unwrap_err().contains("multiple"));
    }
}
