//! Positioned reads of calibrated planar or interleaved binary samples.
use super::check_cancelled;
use std::{fs::File, ops::Range, os::unix::fs::FileExt, sync::atomic::AtomicBool};

pub(super) fn fixed_text(bytes: &[u8]) -> String {
    let text = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
    String::from_utf8_lossy(text).trim().to_owned()
}

/// Endianness used for both the header and sample buffer.
#[derive(Clone, Copy, Debug)]
pub(super) enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    pub(super) fn array<const N: usize>(self, bytes: &[u8; N]) -> [u8; N] {
        let mut bytes = *bytes;
        if matches!(self, Self::Big) {
            bytes.reverse();
        }
        bytes
    }

    pub(super) fn u32(self, bytes: &[u8], offset: usize) -> u32 {
        let mut part = [0; 4];
        part.copy_from_slice(&bytes[offset..offset + 4]);
        u32::from_le_bytes(self.array(&part))
    }

    pub(super) fn f64(self, bytes: &[u8], offset: usize) -> f64 {
        f64::from_le_bytes(self.array(&array_at(bytes, offset)))
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Encoding {
    Signed8,
    Unsigned8,
    Signed16,
    Signed32,
    Unsigned32,
    Unsigned64,
    Float32,
    Float64,
}

impl Encoding {
    pub(super) fn width(self) -> usize {
        match self {
            Self::Signed8 | Self::Unsigned8 => 1,
            Self::Signed16 => 2,
            Self::Signed32 | Self::Unsigned32 | Self::Float32 => 4,
            Self::Unsigned64 | Self::Float64 => 8,
        }
    }

    pub(super) fn decode(self, order: ByteOrder, point: &[u8]) -> f64 {
        match self {
            Self::Signed8 => f64::from(point[0] as i8),
            Self::Unsigned8 => f64::from(point[0]),
            Self::Signed16 => f64::from(i16::from_le_bytes(order.array(&array_at(point, 0)))),
            Self::Signed32 => f64::from(i32::from_le_bytes(order.array(&array_at(point, 0)))),
            Self::Unsigned32 => f64::from(order.u32(point, 0)),
            Self::Unsigned64 => u64::from_le_bytes(order.array(&array_at(point, 0))) as f64,
            Self::Float32 => f64::from(f32::from_le_bytes(order.array(&array_at(point, 0)))),
            Self::Float64 => order.f64(point, 0),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Lane {
    pub(super) offset: u64,
    pub(super) stride: usize,
    pub(super) column: usize,
    pub(super) order: ByteOrder,
    pub(super) encoding: Encoding,
    pub(super) zero: f64,
    pub(super) gain: f64,
    pub(super) bias: f64,
}

impl Lane {
    pub(super) fn read_samples(
        &self,
        file: &File,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        // Parser validation bounds the entire payload by the file length.
        let offset = self.offset + range.start * self.stride as u64;
        let count = (range.end - range.start) as usize;
        let mut bytes = vec![0; count * self.stride];
        const READ_BLOCK: usize = 64 * 1024;
        for (index, chunk) in bytes.chunks_mut(READ_BLOCK).enumerate() {
            check_cancelled(cancel)?;
            file.read_exact_at(chunk, offset + (index * READ_BLOCK) as u64)
                .map_err(|error| format!("Reading scope samples. {error}"))?;
        }
        check_cancelled(cancel)?;
        let frames = bytes.chunks_exact(self.stride);
        let convert = |raw| ((raw - self.zero) * self.gain + self.bias) as f32;
        // Byte-oriented exports avoid per-sample format dispatch on deep captures.
        let samples = match self.encoding {
            Encoding::Unsigned8 => frames
                .map(|frame| convert(f64::from(frame[self.column])))
                .collect(),
            encoding => frames
                .map(|frame| convert(encoding.decode(self.order, &frame[self.column..])))
                .collect(),
        };
        Ok(samples)
    }
}

fn array_at<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    let mut result = [0; N];
    result.copy_from_slice(&bytes[offset..offset + N]);
    result
}
