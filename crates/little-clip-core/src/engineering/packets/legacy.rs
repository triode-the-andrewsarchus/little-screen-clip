use super::*;
use std::io::{BufRead, Seek};

#[derive(Clone, Copy, Debug)]
pub(super) struct Global {
    pub order: Order,
    pub raw: [u8; 24],
    pub units: u32,
}

pub(super) struct Legacy {
    pub global: Global,
    pub interface: Interface,
}

impl Legacy {
    pub fn open(
        signature: [u8; 4],
        source: &mut Source<impl BufRead + Seek>,
    ) -> Result<Self, Error> {
        let (order, units) = match signature {
            [0xd4, 0xc3, 0xb2, 0xa1] => (Order::Little, 1_000_000),
            [0xa1, 0xb2, 0xc3, 0xd4] => (Order::Big, 1_000_000),
            [0x4d, 0x3c, 0xb2, 0xa1] => (Order::Little, 1_000_000_000),
            [0xa1, 0xb2, 0x3c, 0x4d] => (Order::Big, 1_000_000_000),
            _ => return Err(Error::Invalid("unknown PCAP or PCAPNG signature")),
        };
        let header = source.read::<20>()?;
        let major = order.u16(at(&header, 0)?);
        let minor = order.u16(at(&header, 2)?);
        if (major, minor) != (2, 4) {
            return Err(Error::Unsupported("classic PCAP version other than 2.4"));
        }
        let snap_len = order.u32(at(&header, 12)?);
        let link = order.u32(at(&header, 16)?);
        let interface = Interface {
            id: InterfaceId {
                section: 0,
                ordinal: 0,
            },
            link_type: link as u16,
            snap_len,
            resolution: Resolution::Decimal(if units == 1_000_000 { 6 } else { 9 }),
            offset_seconds: 0,
            name: None,
            fcs_bits: (link & (1 << 26) != 0).then_some(((link >> 28) as u8) * 16),
        };
        let mut raw = [0; 24];
        raw[..4].copy_from_slice(&signature);
        raw[4..].copy_from_slice(&header);
        Ok(Self {
            global: Global { order, raw, units },
            interface,
        })
    }

    pub fn next(
        &self,
        source: &mut Source<impl BufRead + Seek>,
        row: u64,
        take: usize,
        scratch: &mut Vec<u8>,
        cancel: &AtomicBool,
    ) -> Result<Option<Record>, Error> {
        let offset = source.position;
        let Some(fields) = source.read_optional::<16>()? else {
            return Ok(None);
        };
        let order = self.global.order;
        let seconds = order.u32(at(&fields, 0)?);
        let fraction = order.u32(at(&fields, 4)?);
        let captured = order.u32(at(&fields, 8)?);
        let original = order.u32(at(&fields, 12)?);
        if fraction >= self.global.units || captured > original {
            return Err(Error::Invalid("classic PCAP packet header"));
        }
        read_payload(source, captured, take, scratch, cancel)?;
        let time = Timestamp {
            ticks: u64::from(seconds) * u64::from(self.global.units) + u64::from(fraction),
            resolution: self.interface.resolution,
            offset_seconds: 0,
        };
        Ok(Some(Record {
            offset,
            header: PacketHeader {
                row,
                time: Some(time),
                interface: self.interface.id,
                link_type: self.interface.link_type,
                captured_len: captured,
                original_len: original,
            },
        }))
    }
}
