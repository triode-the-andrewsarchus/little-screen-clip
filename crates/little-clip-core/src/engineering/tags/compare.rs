use super::{Memory, MemoryRow};

/// One side of a comparison at a recorded address and byte position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservedByte {
    Absent,
    Unread,
    Known(u8),
}

/// Coverage and agreement are counted separately for every byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum Agreement {
    Equal,
    NeitherRead,
    OneRead,
    Different,
}

impl Agreement {
    pub fn between(pair: [ObservedByte; 2]) -> Self {
        match pair {
            [ObservedByte::Known(left), ObservedByte::Known(right)] if left == right => Self::Equal,
            [ObservedByte::Known(_), ObservedByte::Known(_)] => Self::Different,
            [ObservedByte::Known(_), _] | [_, ObservedByte::Known(_)] => Self::OneRead,
            _ => Self::NeitherRead,
        }
    }
}

pub struct ComparedRow {
    pub address: u32,
    pub bytes: Box<[[ObservedByte; 2]]>,
    counts: [usize; 4],
}

impl ComparedRow {
    pub fn count(&self, agreement: Agreement) -> usize {
        self.counts[agreement as usize]
    }

    pub fn strongest(&self) -> Agreement {
        [
            Agreement::Different,
            Agreement::OneRead,
            Agreement::NeitherRead,
        ]
        .into_iter()
        .find(|&agreement| self.count(agreement) != 0)
        .unwrap_or(Agreement::Equal)
    }
}

pub struct MemoryComparison {
    pub rows: Vec<ComparedRow>,
    counts: [usize; 4],
}

impl MemoryComparison {
    /// Compares logical row addresses rather than offsets in the serialized files.
    pub fn new(left: &Memory, right: &Memory) -> Option<Self> {
        if !same_address_space(left, right) {
            return None;
        }
        let mut left = left.rows.iter().peekable();
        let mut right = right.rows.iter().peekable();
        let mut rows = Vec::new();
        let mut totals = [0; 4];
        while let Some(address) = [left.peek(), right.peek()]
            .into_iter()
            .flatten()
            .map(|row| row.address)
            .min()
        {
            let pair = [
                left.next_if(|row| row.address == address),
                right.next_if(|row| row.address == address),
            ];
            let width = pair
                .iter()
                .flatten()
                .map(|row| row.bytes.len())
                .max()
                .unwrap_or(0);
            let mut counts = [0; 4];
            let bytes = (0..width)
                .map(|offset| {
                    let values = pair.map(|row| observed_at(row, offset));
                    let agreement = Agreement::between(values) as usize;
                    counts[agreement] += 1;
                    totals[agreement] += 1;
                    values
                })
                .collect();
            rows.push(ComparedRow {
                address,
                bytes,
                counts,
            });
        }
        Some(Self {
            rows,
            counts: totals,
        })
    }

    pub fn count(&self, agreement: Agreement) -> usize {
        self.counts[agreement as usize]
    }
}

fn observed_at(row: Option<&MemoryRow>, offset: usize) -> ObservedByte {
    match row.and_then(|row| row.bytes.get(offset)) {
        Some(Some(byte)) => ObservedByte::Known(*byte),
        Some(None) => ObservedByte::Unread,
        None => ObservedByte::Absent,
    }
}

fn same_address_space(left: &Memory, right: &Memory) -> bool {
    let Some((a, b)) = left.rows.first().zip(right.rows.first()) else {
        return false;
    };
    if a.bytes.len() != b.bytes.len() {
        return false;
    }
    if a.sector.is_some() || b.sector.is_some() {
        return a.sector.is_some() && b.sector.is_some();
    }
    if left.protocol.eq_ignore_ascii_case(&right.protocol) {
        return true;
    }
    let pages = |memory: &Memory| {
        let protocol = memory.protocol.to_ascii_lowercase();
        let type_2 = protocol.starts_with("ntag") || protocol.starts_with("mifare ultralight");
        let type_2 = type_2 || protocol == "type 2 tag memory";
        memory.rows.last().filter(|_| type_2).map(|row| row.address)
    };
    a.bytes.len() == 4 && pages(left).is_some_and(|end| Some(end) == pages(right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::tags::{Format, RowRole};

    #[test]
    fn compares_recorded_addresses_without_conflating_unknown_and_zero() {
        use Agreement::*;
        use ObservedByte::*;
        let left = memory(
            "NTAG215",
            &[(4, &[Some(0), None, Some(1), None]), (9, &[Some(7)])],
        );
        let right = memory(
            "NTAG215",
            &[(4, &[Some(0), Some(0), Some(2), None]), (8, &[Some(7)])],
        );
        let result = MemoryComparison::new(&left, &right).unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.address)
                .collect::<Vec<_>>(),
            [4, 8, 9]
        );
        assert_eq!(result.rows[0].bytes[1], [Unread, Known(0)]);
        assert_eq!(result.rows[1].bytes[0], [Absent, Known(7)]);
        assert_eq!(result.rows[2].bytes[0], [Known(7), Absent]);
        assert_eq!(result.rows[0].strongest(), Different);
        assert_eq!(result.rows[1].strongest(), OneRead);
        assert_eq!(
            [Equal, NeitherRead, OneRead, Different].map(|state| result.count(state)),
            [1, 1, 3, 1]
        );
    }

    #[test]
    fn requires_shared_address_geometry() {
        let rows: &[(u32, &[Option<u8>])] = &[(4, &[Some(0); 4]), (134, &[None; 4])];
        let volume = memory("NTAG215", rows);
        assert!(MemoryComparison::new(&volume, &memory("Type 2 tag memory", rows)).is_some());
        assert!(MemoryComparison::new(&volume, &memory("SLIX", rows)).is_none());
        assert!(MemoryComparison::new(&volume, &memory("Type 2 tag memory", &rows[..1])).is_none());
    }

    fn memory(protocol: &str, rows: &[(u32, &[Option<u8>])]) -> Memory {
        Memory {
            format: Format::FlipperNfc,
            version: Some(4),
            protocol: protocol.into(),
            identifier: Vec::new(),
            rows: rows
                .iter()
                .map(|&(address, bytes)| MemoryRow {
                    address,
                    name: format!("Page {address}"),
                    sector: None,
                    role: RowRole::Data,
                    bytes: bytes.to_vec(),
                })
                .collect(),
            fields: Vec::new(),
            rom_check: None,
        }
    }
}
