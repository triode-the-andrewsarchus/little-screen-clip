//! Complex SigMF sample encodings and positioned I/Q reads.

use super::{check_cancel, io_error};
use rustfft::num_complex::Complex32;
use std::{fs::File, os::unix::fs::FileExt, sync::atomic::AtomicBool};

const BLOCK: usize = 64 * 1024;

/// Complex sample formats. The component width is the width of either I or Q.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Encoding {
    Ci8,
    Cu8,
    Ci16Le,
    Ci16Be,
    Cu16Le,
    Cu16Be,
    Ci32Le,
    Ci32Be,
    Cu32Le,
    Cu32Be,
    Cf32Le,
    Cf32Be,
    Cf64Le,
    Cf64Be,
}

impl Encoding {
    pub const ALL: [Self; 14] = [
        Self::Ci8,
        Self::Cu8,
        Self::Ci16Le,
        Self::Ci16Be,
        Self::Cu16Le,
        Self::Cu16Be,
        Self::Ci32Le,
        Self::Ci32Be,
        Self::Cu32Le,
        Self::Cu32Be,
        Self::Cf32Le,
        Self::Cf32Be,
        Self::Cf64Le,
        Self::Cf64Be,
    ];

    /// SigMF datatype token for this encoding.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ci8 => "ci8",
            Self::Cu8 => "cu8",
            Self::Ci16Le => "ci16_le",
            Self::Ci16Be => "ci16_be",
            Self::Cu16Le => "cu16_le",
            Self::Cu16Be => "cu16_be",
            Self::Ci32Le => "ci32_le",
            Self::Ci32Be => "ci32_be",
            Self::Cu32Le => "cu32_le",
            Self::Cu32Be => "cu32_be",
            Self::Cf32Le => "cf32_le",
            Self::Cf32Be => "cf32_be",
            Self::Cf64Le => "cf64_le",
            Self::Cf64Be => "cf64_be",
        }
    }

    pub fn from_sigmf(datatype: &str) -> Option<Self> {
        // Endianness is immaterial for byte components. Some writers include it.
        let datatype = match datatype {
            "ci8_le" | "ci8_be" => "ci8",
            "cu8_le" | "cu8_be" => "cu8",
            _ => datatype,
        };
        Self::ALL
            .into_iter()
            .find(|encoding| encoding.name() == datatype)
    }

    pub const fn frame_bytes(self) -> usize {
        match self {
            Self::Ci8 | Self::Cu8 => 2,
            Self::Ci16Le | Self::Ci16Be | Self::Cu16Le | Self::Cu16Be => 4,
            Self::Cf64Le | Self::Cf64Be => 16,
            _ => 8,
        }
    }

    fn decode(self, bytes: &[u8], output: &mut Vec<Complex32>) {
        match self {
            Self::Ci8 => append(bytes, output, |[x]| f32::from(x as i8) / 128.0),
            Self::Cu8 => append(bytes, output, |[x]| (f32::from(x) - 127.5) / 128.0),
            Self::Ci16Le => append(bytes, output, |x| {
                f32::from(i16::from_le_bytes(x)) / 32768.0
            }),
            Self::Ci16Be => append(bytes, output, |x| {
                f32::from(i16::from_be_bytes(x)) / 32768.0
            }),
            Self::Cu16Le => append(bytes, output, |x| {
                (f32::from(u16::from_le_bytes(x)) - 32767.5) / 32768.0
            }),
            Self::Cu16Be => append(bytes, output, |x| {
                (f32::from(u16::from_be_bytes(x)) - 32767.5) / 32768.0
            }),
            Self::Ci32Le => append(bytes, output, |x| {
                i32::from_le_bytes(x) as f32 / 2147483648.0
            }),
            Self::Ci32Be => append(bytes, output, |x| {
                i32::from_be_bytes(x) as f32 / 2147483648.0
            }),
            Self::Cu32Le => append(bytes, output, |x| unsigned32(u32::from_le_bytes(x))),
            Self::Cu32Be => append(bytes, output, |x| unsigned32(u32::from_be_bytes(x))),
            Self::Cf32Le => append(bytes, output, f32::from_le_bytes),
            Self::Cf32Be => append(bytes, output, f32::from_be_bytes),
            Self::Cf64Le => append(bytes, output, |x| f64::from_le_bytes(x) as f32),
            Self::Cf64Be => append(bytes, output, |x| f64::from_be_bytes(x) as f32),
        }
    }
}

pub(super) fn read(
    file: &File,
    encoding: Encoding,
    mut offset: u64,
    count: u64,
    output: &mut Vec<Complex32>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut remaining = count as usize * encoding.frame_bytes();
    let mut buffer = vec![0; remaining.min(BLOCK)];
    while remaining != 0 {
        check_cancel(cancel)?;
        let size = remaining.min(BLOCK);
        let bytes = &mut buffer[..size];
        file.read_exact_at(bytes, offset).map_err(io_error)?;
        encoding.decode(bytes, output);
        remaining -= size;
        offset += size as u64;
    }
    check_cancel(cancel)
}

fn append<const N: usize>(
    bytes: &[u8],
    output: &mut Vec<Complex32>,
    decode: impl Fn([u8; N]) -> f32,
) {
    let components = bytes.as_chunks::<N>().0.as_chunks::<2>().0;
    output.extend(
        components
            .iter()
            .map(|&[i, q]| Complex32::new(decode(i), decode(q))),
    );
}

fn unsigned32(value: u32) -> f32 {
    ((f64::from(value) - 2147483647.5) / 2147483648.0) as f32
}
