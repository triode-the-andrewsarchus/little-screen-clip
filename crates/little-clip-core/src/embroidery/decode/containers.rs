use super::{Bytes, Move, Plot, Record, Result, inverted_delta, palettes, signed};

pub(super) fn vp3(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let (mut file, center, colors) = vp3_design(data)?;
    for index in 0..colors {
        plot.record()?;
        if index != 0 {
            plot.change();
        }
        file.skip(3)?;
        let length = file.be32()?;
        vp3_block(Bytes(file.take(length)?), center, plot)?;
    }
    Ok(())
}

fn vp3_design(data: &[u8]) -> Result<(Bytes<'_>, [f64; 2], usize)> {
    let mut file = Bytes(
        data.strip_prefix(b"%vsm%\0")
            .ok_or("Missing VP3 signature")?,
    );
    skip_string(&mut file)?;
    file.skip(7)?;
    skip_string(&mut file)?;
    file.skip(32)?;
    let center = vp3_point(&mut file)?;
    file.skip(27)?;
    skip_string(&mut file)?;
    file.skip(24)?;
    skip_string(&mut file)?;
    let colors = file.be16()?;
    Ok((file, center, colors))
}

fn vp3_block(mut block: Bytes<'_>, [cx, cy]: [f64; 2], plot: &mut Plot<'_>) -> Result {
    let [x, y] = vp3_point(&mut block)?;
    plot.absolute(Move::Jump, x + cx, y + cy);
    let parts = usize::from(block.u8()?);
    block.skip(1)?;
    let mut color = [0; 3];
    for _ in 0..parts {
        color = block.array()?;
        block.skip(3)?;
    }
    plot.palette.push(Some(color));
    block.skip(2)?;
    for _ in 0..3 {
        skip_string(&mut block)?;
    }
    block.skip(18)?;
    vp3_stitches(block, plot)
}

fn vp3_stitches(mut stream: Bytes<'_>, plot: &mut Plot<'_>) -> Result {
    while stream.0.len() >= 2 {
        plot.record()?;
        let [x, y] = stream.array()?;
        match (x, y) {
            (0x80, 1) => {
                let x = i32::from(i16::from_be_bytes(stream.array()?));
                let y = i32::from(i16::from_be_bytes(stream.array()?));
                stream.skip(2)?;
                plot.relative(Move::Sew, x, y);
            }
            (0x80, 3) => plot.trim(),
            (0x80, _) => {}
            _ => plot.relative(Move::Sew, signed(x), signed(y)),
        }
    }
    Ok(())
}

fn vp3_point(input: &mut Bytes<'_>) -> Result<[f64; 2]> {
    let x = i32::from_be_bytes(input.array()?);
    let y = i32::from_be_bytes(input.array()?);
    Ok([f64::from(x) / 100.0, -f64::from(y) / 100.0])
}

fn skip_string(input: &mut Bytes<'_>) -> Result {
    let size = input.be16()?;
    input.skip(size)
}

pub(super) fn xxx(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let count = Bytes::at(data, 0x27)?.le16()?;
    let mut file = Bytes::at(data, 0x100)?;
    let mut starts = vec![0];
    if !xxx_stitches(&mut file, &mut starts, plot)? {
        return Ok(());
    }
    file.skip(2)?;
    let colors = file.0.as_chunks::<4>().0;
    let stitches = &mut plot.stitches;
    starts.push(stitches.len());
    for (bounds, &[_, red, green, blue]) in starts.windows(2).zip(colors.iter().take(count)) {
        for stitch in &mut stitches[bounds[0]..bounds[1]] {
            stitch.color = [red, green, blue];
        }
    }
    Ok(())
}

fn xxx_stitches(
    input: &mut Bytes<'_>,
    starts: &mut Vec<usize>,
    plot: &mut Plot<'_>,
) -> Result<bool> {
    while !input.0.is_empty() {
        plot.record()?;
        match xxx_record(input)? {
            Record::Move(motion, x, y) => plot.relative(motion, x, y),
            Record::ThreadChange => {
                starts.push(plot.stitches.len());
                plot.change();
            }
            Record::End => return Ok(true),
            Record::Skip => {}
        }
    }
    Ok(false)
}

fn xxx_record(input: &mut Bytes<'_>) -> Result<Record> {
    let lead = input.u8()?;
    if matches!(lead, 0x7d | 0x7e) {
        let x = i32::from(i16::from_le_bytes(input.array()?));
        let y = -i32::from(i16::from_le_bytes(input.array()?));
        return Ok(Record::Move(Move::Jump, x, y));
    }
    let next = input.u8()?;
    if lead != 0x7f {
        return Ok(Record::Move(Move::Sew, signed(lead), -signed(next)));
    }
    let (x, y) = inverted_delta(input.array()?);
    Ok(match next {
        1 | 3 => Record::Move(Move::Jump, x, y),
        8 | 0x0a..=0x17 => Record::ThreadChange,
        0x7f | 0x18 => Record::End,
        _ => Record::Skip,
    })
}

pub(super) fn tbf(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut order = Bytes::at(data, 0x10a)?.take(0x100)?.iter();
    let header = data.get(0x20e..0x600).ok_or("Truncated TBF header")?;
    for &[mark, r, g, b, _] in header.as_chunks::<5>().0 {
        if mark != 0x45 {
            break;
        }
        plot.palette.push(Some([r, g, b]));
    }
    for &[x, y, control] in data[0x600..].as_chunks::<3>().0 {
        plot.record()?;
        let (x, y) = inverted_delta([x, y]);
        match control {
            0x80 => plot.relative(Move::Sew, x, y),
            0x81 => {
                let needle = order.next().ok_or("TBF needle table exhausted")?;
                if let Some(index) = needle.checked_sub(1) {
                    plot.needle(Some(index.into()));
                }
            }
            0x90 => plot.relative(Move::Jump, x, y),
            0x86 => plot.trim(),
            0x40 => {}
            _ => break,
        }
    }
    Ok(())
}

pub(super) fn shv(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let (mut file, sections) = shv_sections(data, plot)?;
    let mut sections = sections.into_iter();
    let first = sections.next().ok_or("SHV has no color blocks")?;
    let mut count = first;
    let mut in_jump = false;
    while file.0.len() >= 2 {
        plot.record()?;
        if count == 0 {
            count = sections.next().unwrap_or(usize::MAX);
            plot.change();
        }
        let [x, y] = file.array()?;
        count = count.saturating_sub(1);
        match (x, y) {
            (0x80, 3) => {}
            (0x80, 2) => in_jump = false,
            (0x80, 1) => {
                let x = i32::from(i16::from_be_bytes(file.array()?));
                let y = i32::from(i16::from_be_bytes(file.array()?));
                plot.relative(Move::Jump, x, y);
                count = count.saturating_sub(2);
                in_jump = true;
            }
            _ => plot.relative(
                if in_jump { Move::Jump } else { Move::Sew },
                signed(x),
                signed(y),
            ),
        }
    }
    Ok(())
}

fn shv_sections<'a>(data: &'a [u8], plot: &mut Plot<'_>) -> Result<(Bytes<'a>, Vec<usize>)> {
    let mut file = Bytes::at(data, 0x56)?;
    file.sized8()?;
    let [width, height] = file.array()?;
    let graphic_size = usize::from(width) * usize::from(height).div_ceil(2);
    file.skip(4 + graphic_size)?;
    let count = file.u8()?;
    file.skip(18)?;
    let mut sections = Vec::with_capacity(count.into());
    for i in 0..count {
        sections.push(file.be32()?);
        let index = usize::from(file.u8()?);
        plot.palette.push(palettes::color(palettes::SHV, index));
        // The final color header shares its last two bytes with the stream.
        file.skip(if i + 1 == count { 7 } else { 9 })?;
    }
    Ok((file, sections))
}
