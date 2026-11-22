use super::{Move, Plot, Result};

pub(super) fn dst(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let (header, body) = data
        .split_at_checked(512)
        .ok_or("Truncated Tajima header")?;
    if !header.windows(3).any(|bytes| bytes == b"LA:") {
        return Err("Missing Tajima header".into());
    }
    threads(header, plot);
    tap(body, plot)
}

pub(super) fn exy(data: &[u8], plot: &mut Plot<'_>) -> Result {
    tap(data.get(0x100..).ok_or("Truncated EXY header")?, plot)
}

pub(super) fn tap(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut sequin = false;
    for &[a, b, c] in data.as_chunks::<3>().0 {
        plot.record()?;
        let x = axis(a, 0, 1, 1)
            + axis(b, 0, 1, 3)
            + axis(a, 2, 3, 9)
            + axis(b, 2, 3, 27)
            + axis(c, 2, 3, 81);
        let y = axis(a, 6, 7, 1)
            + axis(b, 6, 7, 3)
            + axis(a, 4, 5, 9)
            + axis(b, 4, 5, 27)
            + axis(c, 4, 5, 81);
        match c {
            c if c & 0xf3 == 0xf3 => break,
            c if c & 0xc3 == 0xc3 => {
                plot.relative(Move::Shift, x, y);
                plot.change();
            }
            c if c & 0x43 == 0x43 => {
                plot.relative(Move::Shift, x, y);
                sequin = !sequin;
            }
            c if c & 0x83 == 0x83 && !sequin => plot.relative(Move::Jump, x, y),
            _ => plot.relative(Move::Sew, x, y),
        }
    }
    Ok(())
}

fn axis(value: u8, positive: u32, negative: u32, weight: i32) -> i32 {
    let bit = |index| i32::from((value >> index) & 1_u8);
    (bit(positive) - bit(negative)) * weight
}

fn threads(header: &[u8], plot: &mut Plot<'_>) {
    for line in header.split(|byte| matches!(byte, b'\r' | b'\n')) {
        let line = line.trim_ascii();
        if let Some(fields) = line.strip_prefix(b"TC:") {
            let color = fields
                .split(|byte| *byte == b',')
                .next()
                .and_then(hex_color);
            plot.palette.push(color);
        }
    }
}

fn hex_color(text: &[u8]) -> Option<[u8; 3]> {
    let text = text.trim_ascii();
    let hex = std::str::from_utf8(text.strip_prefix(b"#").unwrap_or(text)).ok()?;
    let [_, red, green, blue] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
    Some([red, green, blue])
}

pub(super) fn dsb(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let (header, records) = data.split_at_checked(512).ok_or("Truncated DSB header")?;
    threads(header, plot);
    triplets(records, plot, |plot, [control, y, x]| {
        let (x, y) = magnitudes(x, y, control, 0x20, 0x40);
        match (control, control & 0x1f) {
            (_, 0) => plot.relative(Move::Sew, x, y),
            (_, 1) => plot.relative(Move::Jump, x, y),
            (0xe7, _) => plot.trim(),
            (0xe8, _) => {}
            (0xe9..=0xf7, _) => plot.needle(None),
            _ => return false,
        }
        true
    })
}

pub(super) fn dsz(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let (header, records) = data.split_at_checked(512).ok_or("Truncated DSZ header")?;
    threads(header, plot);
    z_records(records, plot)
}

pub(super) fn fxy(data: &[u8], plot: &mut Plot<'_>) -> Result {
    z_records(data.get(0x100..).ok_or("Truncated FXY header")?, plot)
}

fn z_records(data: &[u8], plot: &mut Plot<'_>) -> Result {
    triplets(data, plot, |plot, [y, x, control]| {
        let (x, y) = magnitudes(x, y, control, 0x40, 0x20);
        match (control, control & 0x1f) {
            (_, 0) => plot.relative(Move::Sew, x, y),
            (_, 1) => plot.relative(Move::Jump, x, y),
            (0x82, _) => {}
            (0x83..=0x9a, _) => plot.needle(None),
            (0x9b, _) => plot.trim(),
            _ => return false,
        }
        true
    })
}

pub(super) fn ten_o(data: &[u8], plot: &mut Plot<'_>) -> Result {
    triplets(data, plot, |plot, [control, y, x]| {
        let (x, y) = magnitudes(x, y, control, 0x20, 0x40);
        match (control, control & 0x1f) {
            (_, 0) => plot.relative(Move::Sew, x, y),
            (_, 0x10) => plot.relative(Move::Jump, x, y),
            (0x85, _) => plot.change(),
            (0x81, _) => plot.trim(),
            (0x8a | 0x82, _) => {}
            _ => return false,
        }
        true
    })
}

pub(super) fn inb(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let records = data.get(0x2000..).ok_or("Truncated INB header")?;
    triplets(records, plot, |plot, [x, y, control]| {
        let (x, y) = magnitudes(x, y, control, 0x40, 0x20);
        match control & 0x0f {
            0 => plot.relative(Move::Sew, x, y),
            1 => {
                plot.relative(Move::Shift, x, y);
                plot.change();
            }
            2 => plot.relative(Move::Jump, x, y),
            _ => return false,
        }
        true
    })
}

pub(super) fn ksm(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let records = data.get(0x200..).ok_or("Truncated KSM header")?;
    let mut trimmed = false;
    let mut sewn = false;
    triplets(records, plot, |plot, [y, x, control]| {
        let (x, y) = magnitudes(x, y, control, 0x40, 0x20);
        if (x, y) != (0, 0) {
            plot.relative(if trimmed { Move::Jump } else { Move::Sew }, x, y);
            sewn |= !trimmed;
        }
        match control & 0x1f {
            0x07 | 0x13 | 0x1d => {
                if sewn {
                    plot.trim();
                }
                trimmed = true;
            }
            0x17..=0x19 | 0x1b => trimmed = false,
            0x0b..=0x12 => {
                plot.needle(None);
                trimmed = true;
            }
            0 | 5 => {}
            _ => return false,
        }
        true
    })
}

pub(super) fn u01(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let records = data.get(0x100..).ok_or("Truncated U01 header")?;
    triplets(records, plot, |plot, [control, y, x]| {
        let (x, y) = magnitudes(x, y, control, 0x20, 0x40);
        let movement = (x, y) != (0, 0);
        let command = control & 0x1f;
        match command {
            0 => plot.relative(Move::Sew, x, y),
            1 => plot.relative(Move::Jump, x, y),
            2..=5 => {
                if movement {
                    let kind = if command & 1 == 0 {
                        Move::Sew
                    } else {
                        Move::Jump
                    };
                    plot.relative(kind, x, y);
                }
            }
            6..=0x17 if control != 0x2b => {
                match command {
                    6 | 7 => plot.trim(),
                    9..=0x17 => plot.needle(None),
                    _ => {}
                }
                if movement {
                    plot.relative(Move::Jump, x, y);
                }
            }
            _ => return false,
        }
        true
    })
}

pub(super) fn zxy(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let skip = super::Bytes::at(data, 1)?.be16()?;
    let records = data.get(3 + skip..).ok_or("Truncated ZXY header")?;
    triplets(records, plot, |plot, [control, x, y]| {
        let (dx, dy) = magnitudes(x, y, control, 8, 4);
        match control & !0x0c {
            0 => plot.relative(Move::Sew, dx, dy),
            bits if bits & 2 != 0 => plot.relative(Move::Jump, dx, dy),
            bits if bits & 0x20 != 0 => {
                if x == 0xff {
                    return false;
                }
                plot.needle(None);
            }
            _ => {}
        }
        true
    })
}

pub(super) fn stc(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let records = data.get(0x28..).ok_or("Truncated STC header")?;
    triplets(records, plot, |plot, [x, y, control]| {
        let (x, y) = super::inverted_delta([x, y]);
        match control {
            0 => plot.relative(Move::Jump, x, y),
            1 => plot.relative(Move::Sew, x, y),
            25 => return false,
            _ => plot.needle(None),
        }
        true
    })
}

fn magnitudes(x: u8, y: u8, control: u8, x_sign: u8, y_sign: u8) -> (i32, i32) {
    let x = i32::from(x);
    let y = i32::from(y);
    (
        if control & x_sign == 0 { x } else { -x },
        if control & y_sign == 0 { -y } else { y },
    )
}

pub(super) fn triplets(
    data: &[u8],
    plot: &mut Plot<'_>,
    mut step: impl FnMut(&mut Plot<'_>, [u8; 3]) -> bool,
) -> Result {
    for &record in data.as_chunks::<3>().0 {
        plot.record()?;
        if !step(plot, record) {
            break;
        }
    }
    Ok(())
}
