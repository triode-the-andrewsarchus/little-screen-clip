//! Bounded readers for physical stitch streams. Coordinates are in tenths
//! of a millimeter with positive Y down, independent of on-disk orientation.

mod brother;
mod containers;
mod janome;
mod legacy;
mod palettes;
mod tape;
#[cfg(test)]
mod tests;

use std::sync::atomic::AtomicBool;

use super::{MAX_COMMANDS, Stitch, check_cancelled};

type Result<T = ()> = std::result::Result<T, String>;
type Reader = fn(&[u8], &mut Plot<'_>) -> Result;

pub(super) fn reader(extension: &str) -> Option<Reader> {
    Some(match extension {
        "pec" => brother::pec,
        "pes" => brother::pes,
        "phb" => brother::phb,
        "phc" => brother::phc,
        "jef" | "ptn" => janome::jef,
        "jef+" | "jpx" => janome::jef_plus,
        "exp" => janome::exp,
        "stx" => janome::stx,
        "sew" => janome::sew,
        "hus" => janome::hus,
        "vip" => janome::vip,
        "dst" => tape::dst,
        "tap" => tape::tap,
        "exy" => tape::exy,
        "dsb" => tape::dsb,
        "dsz" => tape::dsz,
        "fxy" => tape::fxy,
        "10o" => tape::ten_o,
        "inb" => tape::inb,
        "ksm" => tape::ksm,
        "u01" => tape::u01,
        "zxy" => tape::zxy,
        "stc" => tape::stc,
        "vp3" => containers::vp3,
        "xxx" => containers::xxx,
        "tbf" => containers::tbf,
        "shv" => containers::shv,
        "emd" => legacy::emd,
        "mit" => legacy::mit,
        "pcs" | "pcq" => legacy::pc,
        "pmv" => legacy::pmv,
        "spx" => legacy::spx,
        "zhs" => legacy::zhs,
        _ => return None,
    })
}

pub(super) fn read(reader: Reader, bytes: &[u8], cancelled: &AtomicBool) -> Result<Vec<Stitch>> {
    check_cancelled(cancelled)?;
    let mut plot = Plot::new(cancelled);
    reader(bytes, &mut plot)?;
    Ok(plot.stitches)
}

#[derive(Clone, Copy)]
enum Move {
    Sew,
    Jump,
    Shift,
}

enum Record {
    Move(Move, i32, i32),
    ThreadChange,
    End,
    Skip,
}

const FALLBACK_COLORS: [[u8; 3]; 6] = [
    [19, 123, 117],
    [181, 55, 84],
    [36, 70, 143],
    [146, 96, 16],
    [112, 60, 134],
    [40, 92, 55],
];

pub(super) struct Plot<'a> {
    stitches: Vec<Stitch>,
    previous: Option<[i32; 2]>,
    thread_block: usize,
    cancelled: &'a AtomicBool,
    point: [f64; 2],
    palette: Vec<Option<[u8; 3]>>,
    color_index: usize,
    sewn_in_block: bool,
    records: usize,
}

impl<'a> Plot<'a> {
    fn new(cancelled: &'a AtomicBool) -> Self {
        Self {
            stitches: Vec::new(),
            previous: None,
            thread_block: 0,
            cancelled,
            point: [0.0; 2],
            palette: Vec::new(),
            color_index: 0,
            sewn_in_block: false,
            records: 0,
        }
    }

    fn record(&mut self) -> Result {
        if self.records >= MAX_COMMANDS {
            return Err("Embroidery stitch budget exceeded".into());
        }
        if self.records.is_multiple_of(256) {
            check_cancelled(self.cancelled)?;
        }
        self.records += 1;
        Ok(())
    }

    fn relative(&mut self, movement: Move, x: i32, y: i32) {
        self.absolute(
            movement,
            self.point[0] + f64::from(x),
            self.point[1] + f64::from(y),
        );
    }

    fn absolute(&mut self, movement: Move, x: f64, y: f64) {
        self.point = [x, y];
        match movement {
            Move::Sew => {
                let point = self.point.map(|value| value.round_ties_even() as i32);
                self.stitches.push(Stitch {
                    from: self.previous.unwrap_or(point),
                    to: point,
                    color: self
                        .color(self.color_index)
                        .unwrap_or(FALLBACK_COLORS[self.thread_block % FALLBACK_COLORS.len()]),
                });
                self.previous = Some(point);
                self.sewn_in_block = true;
            }
            Move::Jump => self.trim(),
            Move::Shift => {}
        }
    }

    fn trim(&mut self) {
        self.previous = None;
    }

    fn color(&self, index: usize) -> Option<[u8; 3]> {
        self.palette.get(index).copied().flatten()
    }

    fn change(&mut self) {
        self.color_index += 1;
        self.thread_block += 1;
        self.trim();
        self.sewn_in_block = false;
    }

    fn needle(&mut self, index: Option<usize>) {
        if self.sewn_in_block {
            self.change();
        } else {
            self.trim();
        }
        if let Some(index) = index {
            self.color_index = index;
        }
    }
}

/// Views one file or one length-delimited section. Offset arithmetic never
/// moves beyond that view, including when a nested section supplies an offset.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn at(data: &'a [u8], offset: usize) -> Result<Self> {
        data.get(offset..)
            .map(Self)
            .ok_or_else(|| "Embroidery offset outside file".into())
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let (head, tail) = self
            .0
            .split_at_checked(count)
            .ok_or("Truncated embroidery data")?;
        self.0 = tail;
        Ok(head)
    }

    fn skip(&mut self, count: usize) -> Result {
        self.take(count).map(|_| ())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (record, tail) = self
            .0
            .split_first_chunk()
            .ok_or("Truncated embroidery record")?;
        self.0 = tail;
        Ok(*record)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn le16(&mut self) -> Result<usize> {
        Ok(u16::from_le_bytes(self.array()?).into())
    }

    fn be16(&mut self) -> Result<usize> {
        Ok(u16::from_be_bytes(self.array()?).into())
    }

    fn le32(&mut self) -> Result<usize> {
        Ok(u32::from_le_bytes(self.array()?) as usize)
    }

    fn le32s<const N: usize>(&mut self) -> Result<[usize; N]> {
        let mut values = [0; N];
        for value in &mut values {
            *value = self.le32()?;
        }
        Ok(values)
    }

    fn be32(&mut self) -> Result<usize> {
        Ok(u32::from_be_bytes(self.array()?) as usize)
    }

    fn sized8(&mut self) -> Result<&'a [u8]> {
        let length = self.u8()?.into();
        self.take(length)
    }
}

fn signed(byte: u8) -> i32 {
    i32::from(byte as i8)
}

fn inverted_delta([x, y]: [u8; 2]) -> (i32, i32) {
    (signed(x), -signed(y))
}
