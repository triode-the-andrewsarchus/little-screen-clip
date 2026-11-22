use super::{Bytes, Move, Plot, Result, palettes};

pub(super) fn pec(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let block = data
        .strip_prefix(b"#PEC0001")
        .ok_or("Missing PEC signature")?;
    pec_block(block, Vec::new(), plot)
}

pub(super) fn pes(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut header = Bytes(data);
    let version: [u8; 8] = header.array()?;
    if !version.starts_with(b"#PES") {
        return Err("Missing PES signature".into());
    }
    let offset = header.le32()?;
    let design_header = data.get(12..offset).ok_or("Invalid PES stitch offset")?;
    // Independent PEC geometry remains available when an optional PES design
    // header contains an unsupported object table.
    let chart = pes_chart(&version, Bytes(design_header)).unwrap_or_default();
    pec_block(Bytes::at(data, offset)?.0, chart, plot)
}

fn pes_chart(version: &[u8; 8], mut header: Bytes<'_>) -> Result<Vec<[u8; 3]>> {
    let (before_image, after_image, hoop_name) = match version {
        b"#PES0050" | b"#PES0055" | b"#PES0056" => (24, 24, false),
        b"#PES0060" | b"#PES0070" => (36, 24, false),
        b"#PES0080" => (38, 26, false),
        b"#PES0090" => (30, 34, true),
        b"#PES0100" => (38, 34, true),
        _ => return Ok(Vec::new()),
    };
    header.skip(4)?;
    for _ in 0..5 {
        header.sized8()?;
    }
    if hoop_name {
        header.skip(14)?;
        header.sized8()?;
    }
    header.skip(before_image)?;
    header.sized8()?;
    header.skip(after_image)?;
    if header.array::<6>()? != [0; 6] {
        return Ok(Vec::new());
    }
    custom_threads(header)
}

fn custom_threads(mut header: Bytes<'_>) -> Result<Vec<[u8; 3]>> {
    let count = header.le16()?;
    let mut chart = Vec::new();
    for _ in 0..count {
        header.sized8()?;
        chart.push(header.array()?);
        header.skip(5)?;
        for _ in 0..3 {
            header.sized8()?;
        }
    }
    Ok(chart)
}

fn pec_block(block: &[u8], chart: Vec<[u8; 3]>, plot: &mut Plot<'_>) -> Result {
    let header = block.get(..512).ok_or("Truncated PEC header")?;
    let count = usize::from(header[48]) + 1;
    let colors = header.get(49..49 + count).ok_or("Truncated PEC palette")?;
    plot.palette = map_palette(colors, chart);
    let [a, b, c] = Bytes::at(block, 514)?.array()?;
    let length = u32::from_le_bytes([a, b, c, 0]) as usize;
    let body = block
        .get(532..512 + length)
        .ok_or("Invalid PEC stitch block length")?;
    stitches(Bytes(body), plot)
}

fn map_palette(indices: &[u8], chart: Vec<[u8; 3]>) -> Vec<Option<[u8; 3]>> {
    if chart.len() >= indices.len() {
        return chart.into_iter().map(Some).collect();
    }
    let mut chart = chart.into_iter();
    let mut by_index = [None; 256];
    indices
        .iter()
        .map(|&index| {
            let index = usize::from(index) % palettes::PEC.len();
            *by_index[index].get_or_insert_with(|| {
                chart
                    .next()
                    .or_else(|| palettes::color(palettes::PEC, index))
            })
        })
        .collect()
}

pub(super) fn phc(data: &[u8], plot: &mut Plot<'_>) -> Result {
    if !data.starts_with(b"#PHC") {
        return Err("Missing PHC signature".into());
    }
    plot.palette = indexed_palette(Bytes::at(data, 0x4d)?)?;
    let mut offsets = Bytes::at(data, 0x2b)?;
    let header_adjustment = usize::from(offsets.u8()?);
    offsets.skip(4)?;
    let mut stream = Bytes::at(data, offsets.le16()? + header_adjustment)?;
    let header_length = stream.le16()?;
    stream.skip(header_length)?;
    let section_length = stream.le32()?;
    stream.skip(section_length)?;
    stream.skip(10)?;
    machine_stitches(stream, 0x1d, plot)
}

pub(super) fn phb(data: &[u8], plot: &mut Plot<'_>) -> Result {
    if !data.starts_with(b"#PHB") {
        return Err("Missing PHB signature".into());
    }
    plot.palette = indexed_palette(Bytes::at(data, 0x71)?)?;
    let mut stream = Bytes::at(data, 0x52)?;
    stream.skip(Bytes::at(data, 0x54)?.le32()?)?;
    let header_length = Bytes(stream.0).le32()?;
    stream.skip(header_length)?;
    stream.skip(2)?;
    let section_length = Bytes(stream.0).le32()?;
    stream.skip(section_length)?;
    stream.skip(14)?;
    machine_stitches(stream, 0x15, plot)
}

fn machine_stitches(mut stream: Bytes<'_>, prelude: usize, plot: &mut Plot<'_>) -> Result {
    let colors = usize::from(stream.u8()?);
    stream.skip(colors + prelude)?;
    stitches(stream, plot)
}

fn indexed_palette(mut header: Bytes<'_>) -> Result<Vec<Option<[u8; 3]>>> {
    let count = header.le16()?;
    Ok(header
        .take(count)?
        .iter()
        .map(|&index| palettes::color(palettes::PEC, usize::from(index)))
        .collect())
}

fn stitches(mut input: Bytes<'_>, plot: &mut Plot<'_>) -> Result {
    while let Some(&lead) = input.0.first() {
        plot.record()?;
        if lead == 0xff && input.0.get(1).is_none_or(|&next| next == 0) {
            break;
        }
        if input.0.starts_with(&[0xfe, 0xb0]) {
            input.skip(3)?;
            let next = plot.color_index + 1;
            let current = plot.color(plot.color_index);
            if plot.sewn_in_block && current.is_some() && current == plot.color(next) {
                // The same thread can appear twice to encode a stop.
                plot.color_index = next;
            } else {
                plot.change();
            }
            continue;
        }
        let (x, x_flags) = pec_axis(&mut input)?;
        let (y, y_flags) = pec_axis(&mut input)?;
        let motion = if x_flags | y_flags != 0 {
            Move::Jump
        } else {
            Move::Sew
        };
        plot.relative(motion, x, y);
    }
    Ok(())
}

fn pec_axis(input: &mut Bytes<'_>) -> Result<(i32, u8)> {
    let lead = input.u8()?;
    if lead & 0x80 != 0 {
        let value = (u16::from(lead) << 8 | u16::from(input.u8()?)) << 4;
        Ok((i32::from(value as i16 >> 4), lead & 0x30))
    } else {
        Ok((i32::from(((lead << 1) as i8) >> 1), 0))
    }
}
