use super::*;

pub(super) fn window<R: BufRead + Seek>(
    reader: &mut Reader<R>,
    index: &Index,
    rows: Range<u64>,
    mut output: impl Write,
    cancel: &AtomicBool,
) -> Result<u64, Error> {
    let end = rows.end.min(index.points());
    let count = end.saturating_sub(rows.start);
    if count == 0 {
        return Err(Error::Invalid("empty capture export range"));
    }
    reader.position_at(index, rows.start, cancel)?;
    let global = match &reader.container {
        Container::Legacy(legacy) => {
            output.write_all(&legacy.global.raw)?;
            Some(legacy.global)
        }
        Container::Ng(_) => None,
    };
    let mut section = None;
    let mut bytes = Vec::new();
    for _ in 0..count {
        let record = reader
            .next(PACKET_BYTES, &mut bytes, cancel)?
            .ok_or(Error::Truncated)?;
        if let Some(global) = global {
            legacy_packet(&mut output, global, record.header, &bytes)?;
        } else {
            let current = index
                .sections
                .get(record.header.interface.section as usize)
                .ok_or(Error::Truncated)?;
            if section != Some(current.id) {
                section = Some(current.id);
                section_header(&mut output, current)?;
            }
            ng_packet(&mut output, current.order, record.header, &bytes)?;
        }
    }
    Ok(count)
}

fn legacy_packet(
    output: &mut impl Write,
    global: legacy::Global,
    packet: PacketHeader,
    bytes: &[u8],
) -> Result<(), Error> {
    let ticks = packet.time.ok_or(Error::Truncated)?.ticks;
    let units = u64::from(global.units);
    let fields = [
        (ticks / units) as u32,
        (ticks % units) as u32,
        packet.captured_len,
        packet.original_len,
    ];
    for field in fields {
        output.write_all(&u32_bytes(global.order, field))?;
    }
    output.write_all(bytes)?;
    Ok(())
}

fn section_header(output: &mut impl Write, section: &ng::Section) -> Result<(), Error> {
    let order = section.order;
    let mut body = Vec::with_capacity(16);
    body.extend(u32_bytes(order, 0x1a2b3c4d));
    body.extend(u16_bytes(order, 1));
    body.extend(u16_bytes(order, 0));
    body.extend([0xff; 8]);
    write_block(output, order, 0x0a0d0d0a, &[&body])?;
    for interface in &section.interfaces {
        interface_header(output, order, interface)?;
    }
    Ok(())
}

fn interface_header(
    output: &mut impl Write,
    order: Order,
    interface: &Interface,
) -> Result<(), Error> {
    let mut body = Vec::new();
    body.extend(u16_bytes(order, interface.link_type));
    body.extend([0; 2]);
    body.extend(u32_bytes(order, interface.snap_len));
    if let Some(name) = &interface.name {
        option(&mut body, order, 2, name.as_bytes());
    }
    let resolution = match interface.resolution {
        Resolution::Decimal(power) => power,
        Resolution::Binary(power) => power | 0x80,
    };
    option(&mut body, order, 9, &[resolution]);
    if let Some(bits) = interface.fcs_bits {
        option(&mut body, order, 13, &[bits]);
    }
    if interface.offset_seconds != 0 {
        let offset = match order {
            Order::Little => interface.offset_seconds.to_le_bytes(),
            Order::Big => interface.offset_seconds.to_be_bytes(),
        };
        option(&mut body, order, 14, &offset);
    }
    option(&mut body, order, 0, &[]);
    write_block(output, order, 1, &[&body])
}

fn option(body: &mut Vec<u8>, order: Order, code: u16, data: &[u8]) {
    body.extend(u16_bytes(order, code));
    body.extend(u16_bytes(order, data.len() as u16));
    body.extend(data);
    body.extend(&[0; 3][..padding(data.len())]);
}

fn ng_packet(
    output: &mut impl Write,
    order: Order,
    packet: PacketHeader,
    bytes: &[u8],
) -> Result<(), Error> {
    let pad = &[0; 3][..padding(bytes.len())];
    if let Some(time) = packet.time {
        let fields = [
            packet.interface.ordinal,
            (time.ticks >> 32) as u32,
            time.ticks as u32,
            packet.captured_len,
            packet.original_len,
        ];
        let mut header = [0; 20];
        for (target, word) in header.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
            *target = u32_bytes(order, word);
        }
        write_block(output, order, 6, &[&header, bytes, pad])
    } else {
        let original = u32_bytes(order, packet.original_len);
        write_block(output, order, 3, &[&original, bytes, pad])
    }
}

fn write_block(
    output: &mut impl Write,
    order: Order,
    kind: u32,
    parts: &[&[u8]],
) -> Result<(), Error> {
    let length = 12 + parts.iter().map(|part| part.len()).sum::<usize>();
    let length = u32::try_from(length).map_err(|_| Error::Limit("capture export block length"))?;
    let length = u32_bytes(order, length);
    output.write_all(&u32_bytes(order, kind))?;
    output.write_all(&length)?;
    for part in parts {
        output.write_all(part)?;
    }
    output.write_all(&length)?;
    Ok(())
}

fn padding(length: usize) -> usize {
    length.wrapping_neg() & 3
}

fn u16_bytes(order: Order, word: u16) -> [u8; 2] {
    match order {
        Order::Little => word.to_le_bytes(),
        Order::Big => word.to_be_bytes(),
    }
}

fn u32_bytes(order: Order, word: u32) -> [u8; 4] {
    match order {
        Order::Little => word.to_le_bytes(),
        Order::Big => word.to_be_bytes(),
    }
}
