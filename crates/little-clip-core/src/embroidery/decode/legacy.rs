use super::{Bytes, Move, Plot, Result, inverted_delta, signed, tape};

pub(super) fn emd(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut input = Bytes::at(data, 0x30)?;
    while input.0.len() >= 2 {
        plot.record()?;
        let [x, y] = input.array()?;
        match (x, y) {
            (0x80, 0x80) => {
                let (x, y) = inverted_delta(input.array()?);
                plot.relative(Move::Jump, x, y);
            }
            (0x80, 0x2a) => plot.change(),
            (0x80, 0xad | 0x90) => plot.trim(),
            (0x80, 0x7d) => {}
            (0x80, _) => break,
            _ => plot.relative(Move::Sew, signed(x), -signed(y)),
        }
    }
    Ok(())
}

pub(super) fn mit(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut previous = 0;
    for &[x, y] in data.as_chunks::<2>().0 {
        plot.record()?;
        let control = ((x & 0x60) >> 3) | ((y & 0x60) >> 5);
        // MIT deltas are in fifths of a millimeter, with a separate sign bit.
        let delta = |b: u8| 2 * i32::from(b & 0x1f) * if b & 0x80 == 0 { 1 } else { -1 };
        match control {
            0 => break,
            0b1000 => {
                if previous == 0b0111 {
                    plot.change();
                }
            }
            0b1100 => plot.relative(Move::Jump, delta(x), -delta(y)),
            _ => plot.relative(Move::Sew, delta(x), -delta(y)),
        }
        previous = control;
    }
    Ok(())
}

pub(super) fn pc(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut input = Bytes::at(data, 2)?;
    let count = input.le16()?;
    for &[red, green, blue, _] in input.take(count * 4)?.as_chunks::<4>().0 {
        plot.palette.push(Some([red, green, blue]));
    }
    input.skip(2)?; // Advisory stitch count, which some writers leave zero.
    for &[_, a, b, c, _, d, e, f, control] in input.0.as_chunks::<9>().0 {
        plot.record()?;
        let decode = |a, b, c| {
            let extended = i32::from_le_bytes([0, a, b, c]) >> 8;
            f64::from(extended) * (5.0 / 3.0)
        };
        let (x, y) = (decode(a, b, c), -decode(d, e, f));
        match control {
            0 => plot.absolute(Move::Sew, x, y),
            bits if bits & 1 != 0 => plot.change(),
            bits if bits & 4 != 0 => plot.absolute(Move::Jump, x, y),
            _ => break,
        }
    }
    Ok(())
}

pub(super) fn pmv(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut input = Bytes::at(data, 0x64)?;
    let mut x = 0.0;
    while input.0.len() >= 4 {
        plot.record()?;
        let count = input.le16()?;
        if input.le16()? >= 256 {
            break;
        }
        for &[dx, y] in input.take(count * 2)?.as_chunks::<2>().0 {
            plot.record()?;
            // The format stores a horizontal delta and an absolute Y.
            let decode = |value: u8, modulus: u8| {
                let value = i32::from(value);
                let modulus = i32::from(modulus);
                f64::from(if value > modulus / 2 {
                    value - modulus
                } else {
                    value
                }) * 2.5
            };
            x += decode(dx, 64);
            plot.absolute(Move::Sew, x, -decode(y, 32));
        }
    }
    Ok(())
}

pub(super) fn spx(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let input = Bytes::at(data, 0x11e)?;
    for &[_, _, a, b, c, d, correction, _, _] in input.0.as_chunks::<9>().0 {
        plot.record()?;
        let x = i32::from(i16::from_be_bytes([c, d]));
        let y = -i32::from(i16::from_be_bytes([a, b])) - signed(correction);
        plot.relative(Move::Sew, x, y);
    }
    Ok(())
}

pub(super) fn zhs(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let [stitches, header] = Bytes::at(data, 0x0f)?.le32s()?;
    let mut palette = Bytes::at(data, header)?;
    let colors = usize::from(palette.u8()?);
    for &color in palette.take(colors * 3)?.as_chunks::<3>().0 {
        plot.palette.push(Some(color));
    }
    let mut x = 0;
    let mut y = 0;
    tape::triplets(
        Bytes::at(data, stitches)?.0,
        plot,
        |plot, [control, a, b]| {
            if control == 0x10 {
                return true; // Checksum.
            }
            x += zhs_delta((a & 0x55) | (b & 0xaa));
            y -= zhs_delta((a & 0xaa) | (b & 0x55));
            match control {
                0x01 | 0x02 => {
                    let movement = if control == 0x01 {
                        Move::Jump
                    } else {
                        Move::Sew
                    };
                    plot.relative(movement, x, y);
                    (x, y) = (0, 0);
                }
                0x04 => {
                    (x, y) = (0, 0);
                    plot.change();
                }
                0x80 => return false,
                _ => {}
            }
            true
        },
    )
}

fn zhs_delta(value: u8) -> i32 {
    let value = signed(value);
    if value.abs() >= 63 {
        value + value.signum()
    } else {
        value
    }
}
