use super::*;
use std::io::{BufRead, Seek};

const MAX_SECTIONS: usize = 4096;
const MAX_INTERFACES: usize = 16_384;
const MAX_INTERFACE_OPTIONS: u64 = 64 * 1024;
const MAX_INTERFACE_NAME: usize = 256;

#[derive(Clone, Debug)]
pub(super) struct Section {
    pub id: u32,
    pub order: Order,
    pub interfaces: Vec<Interface>,
}

pub(super) struct Ng {
    pub section: Section,
    completed: Vec<Section>,
    interface_count: usize,
}

impl Ng {
    pub fn open(source: &mut Source<impl BufRead + Seek>) -> Result<Self, Error> {
        Ok(Self {
            section: section(source, 0)?,
            completed: Vec::new(),
            interface_count: 0,
        })
    }

    pub fn from_checkpoint(section: &Section, interface_count: usize) -> Self {
        let mut section = section.clone();
        section.interfaces.truncate(interface_count);
        Self {
            section,
            completed: Vec::new(),
            interface_count,
        }
    }

    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        self.completed.iter().chain(std::iter::once(&self.section))
    }

    pub fn interfaces(&self) -> impl Iterator<Item = &Interface> {
        self.sections().flat_map(|section| &section.interfaces)
    }

    pub fn next(
        &mut self,
        source: &mut Source<impl BufRead + Seek>,
        row: u64,
        take: usize,
        scratch: &mut Vec<u8>,
        cancel: &AtomicBool,
    ) -> Result<Option<Record>, Error> {
        loop {
            cancelled(cancel)?;
            let offset = source.position;
            let Some(kind) = source.read_optional::<4>()? else {
                return Ok(None);
            };
            if kind == NG_MAGIC {
                let id = self.section.id + 1;
                if id as usize >= MAX_SECTIONS {
                    return Err(Error::Limit("PCAPNG sections"));
                }
                let next = section(source, id)?;
                self.completed
                    .push(std::mem::replace(&mut self.section, next));
                continue;
            }
            let order = self.section.order;
            let block = block(offset, order.u32(source.read()?), 12)?;
            match order.u32(kind) {
                1 => self.add_interface(source, block, cancel)?,
                2 | 3 | 6 => {
                    let header = self.packet_header(source, block, order.u32(kind), row)?;
                    read_payload(source, header.captured_len, take, scratch, cancel)?;
                    block.finish(source, order, cancel)?;
                    return Ok(Some(Record { offset, header }));
                }
                _ => block.finish(source, order, cancel)?,
            }
        }
    }

    fn add_interface(
        &mut self,
        source: &mut Source<impl BufRead + Seek>,
        block: Block,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        if self.interface_count >= MAX_INTERFACES {
            return Err(Error::Limit("PCAPNG interfaces"));
        }
        let order = self.section.order;
        block.require(source, 8)?;
        let fields = source.read::<8>()?;
        let id = InterfaceId {
            section: self.section.id,
            ordinal: self.section.interfaces.len() as u32,
        };
        let mut interface = Interface {
            id,
            link_type: order.u16(at(&fields, 0)?),
            snap_len: order.u32(at(&fields, 4)?),
            resolution: Resolution::Decimal(6),
            offset_seconds: 0,
            name: None,
            fcs_bits: None,
        };
        interface_options(source, block, order, &mut interface, cancel)?;
        self.section.interfaces.push(interface);
        self.interface_count += 1;
        Ok(())
    }

    fn packet_header(
        &self,
        source: &mut Source<impl BufRead + Seek>,
        block: Block,
        kind: u32,
        row: u64,
    ) -> Result<PacketHeader, Error> {
        let order = self.section.order;
        let PacketFields {
            ordinal,
            ticks,
            captured,
            original,
        } = if kind == 3 {
            let interface = self.interface(0)?;
            simple_fields(source, block, order, interface.snap_len)?
        } else {
            timed_fields(source, block, order, kind == 2)?
        };
        if captured > original {
            return Err(Error::Invalid(
                "captured packet length exceeds original length",
            ));
        }
        block.require(source, align4(captured))?;
        let interface = self.interface(ordinal)?;
        let time = ticks.map(|ticks| Timestamp {
            ticks,
            resolution: interface.resolution,
            offset_seconds: interface.offset_seconds,
        });
        Ok(PacketHeader {
            row,
            interface: interface.id,
            link_type: interface.link_type,
            time,
            captured_len: captured,
            original_len: original,
        })
    }

    fn interface(&self, ordinal: u32) -> Result<&Interface, Error> {
        self.section
            .interfaces
            .get(ordinal as usize)
            .ok_or(Error::Invalid("unknown section interface"))
    }
}

struct PacketFields {
    ordinal: u32,
    ticks: Option<u64>,
    captured: u32,
    original: u32,
}

fn simple_fields(
    source: &mut Source<impl BufRead + Seek>,
    block: Block,
    order: Order,
    snap_len: u32,
) -> Result<PacketFields, Error> {
    block.require(source, 4)?;
    let original = order.u32(source.read()?);
    let captured = if snap_len == 0 {
        original
    } else {
        original.min(snap_len)
    };
    if align4(captured) != block.body_end - source.position {
        return Err(Error::Invalid("invalid Simple Packet Block length"));
    }
    Ok(PacketFields {
        ordinal: 0,
        ticks: None,
        captured,
        original,
    })
}

fn timed_fields(
    source: &mut Source<impl BufRead + Seek>,
    block: Block,
    order: Order,
    obsolete: bool,
) -> Result<PacketFields, Error> {
    block.require(source, 20)?;
    let fields = source.read::<20>()?;
    let ordinal = if obsolete {
        u32::from(order.u16(at(&fields, 0)?))
    } else {
        order.u32(at(&fields, 0)?)
    };
    let high = u64::from(order.u32(at(&fields, 4)?));
    let low = u64::from(order.u32(at(&fields, 8)?));
    Ok(PacketFields {
        ordinal,
        ticks: Some(high << 32 | low),
        captured: order.u32(at(&fields, 12)?),
        original: order.u32(at(&fields, 16)?),
    })
}

fn section(source: &mut Source<impl BufRead + Seek>, id: u32) -> Result<Section, Error> {
    let start = source.position - 4;
    let length = source.read::<4>()?;
    let order = match source.read::<4>()? {
        [0x4d, 0x3c, 0x2b, 0x1a] => Order::Little,
        [0x1a, 0x2b, 0x3c, 0x4d] => Order::Big,
        _ => return Err(Error::Invalid("unknown PCAPNG byte-order magic")),
    };
    let block = block(start, order.u32(length), 28)?;
    let fields = source.read::<12>()?;
    if order.u16(at(&fields, 0)?) != 1 {
        return Err(Error::Unsupported("PCAPNG major version other than 1"));
    }
    // A conforming reader permits unknown section options and minor extensions.
    if block.body_end - source.position > MAX_INTERFACE_OPTIONS {
        return Err(Error::Limit("PCAPNG section options"));
    }
    block.finish(source, order, &AtomicBool::new(false))?;
    Ok(Section {
        id,
        order,
        interfaces: Vec::new(),
    })
}

#[derive(Clone, Copy)]
struct Block {
    size: u32,
    body_end: u64,
}

fn block(start: u64, size: u32, minimum: u32) -> Result<Block, Error> {
    if size < minimum || !size.is_multiple_of(4) {
        return Err(Error::Invalid("PCAPNG block length"));
    }
    let body_end = start
        .checked_add(u64::from(size - 4))
        .ok_or(Error::Truncated)?;
    Ok(Block { size, body_end })
}

impl Block {
    fn require(self, source: &Source<impl BufRead>, amount: u64) -> Result<(), Error> {
        if source
            .position
            .checked_add(amount)
            .is_some_and(|end| end <= self.body_end)
        {
            Ok(())
        } else {
            Err(Error::Invalid("field overruns PCAPNG block"))
        }
    }

    fn finish(
        self,
        source: &mut Source<impl BufRead + Seek>,
        order: Order,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        self.require(source, 0)?;
        source.skip(self.body_end - source.position, cancel)?;
        if order.u32(source.read()?) != self.size {
            return Err(Error::Invalid("PCAPNG block length trailer disagrees"));
        }
        Ok(())
    }
}

fn align4(amount: u32) -> u64 {
    (u64::from(amount) + 3) & !3
}

fn interface_options(
    source: &mut Source<impl BufRead + Seek>,
    block: Block,
    order: Order,
    interface: &mut Interface,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    if block.body_end - source.position > MAX_INTERFACE_OPTIONS {
        return Err(Error::Limit("PCAPNG interface options"));
    }
    while source.position < block.body_end {
        cancelled(cancel)?;
        block.require(source, 4)?;
        let code = order.u16(source.read()?);
        let size = order.u16(source.read()?);
        let padded = align4(u32::from(size));
        block.require(source, padded)?;
        let end = source.position + padded;
        if code == 0 {
            if size != 0 {
                return Err(Error::Invalid("PCAPNG end-of-options item has payload"));
            }
            break;
        }
        apply_option(source, order, interface, code, size)?;
        source.skip(end - source.position, cancel)?;
    }
    block.finish(source, order, cancel)
}

fn apply_option(
    source: &mut Source<impl BufRead + Seek>,
    order: Order,
    interface: &mut Interface,
    code: u16,
    size: u16,
) -> Result<(), Error> {
    match (code, size) {
        (2, length) => {
            let mut text = [0; MAX_INTERFACE_NAME];
            let length = usize::from(length).min(text.len());
            source.fill(&mut text[..length])?;
            let text = String::from_utf8_lossy(&text[..length]);
            interface.name = Some(text.trim_end_matches('\0').to_owned());
        }
        (9, 1) => {
            let [resolution] = source.read()?;
            interface.resolution = if resolution & 0x80 != 0 {
                Resolution::Binary(resolution & 0x7f)
            } else {
                Resolution::Decimal(resolution)
            };
        }
        (13, 1) => interface.fcs_bits = Some(source.read::<1>()?[0]),
        (14, 8) => interface.offset_seconds = order.u64(source.read()?) as i64,
        _ => {}
    }
    Ok(())
}
