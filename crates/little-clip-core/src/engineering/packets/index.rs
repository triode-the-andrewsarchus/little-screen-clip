use super::*;
use std::collections::BTreeMap;

const MAX_CHECKPOINTS: usize = 65_536;
const INITIAL_STRIDE: u64 = 256;
const MAX_BUCKETS: i128 = 1024;

#[derive(Clone, Copy, Debug)]
struct Checkpoint {
    row: u64,
    offset: u64,
    section: u32,
    interface_count: usize,
}

/// Aggregate activity of captured packets in a half-open timestamp interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivityBin {
    pub start_ns: i128,
    pub end_ns: i128,
    pub packets: u64,
    pub captured_bytes: u64,
    pub first_row: u64,
    pub last_row: u64,
}

#[derive(Clone, Debug)]
pub struct Index {
    format: Format,
    points: u64,
    pub(super) sections: Vec<ng::Section>,
    legacy_interface: Option<Interface>,
    checkpoints: Vec<Checkpoint>,
    activity: Vec<ActivityBin>,
    time_bounds: Option<[i128; 2]>,
    untimed: u64,
}

impl Index {
    pub fn format(&self) -> Format {
        self.format
    }

    pub fn points(&self) -> u64 {
        self.points
    }

    pub fn activity(&self) -> &[ActivityBin] {
        &self.activity
    }

    pub fn time_bounds_ns(&self) -> Option<[i128; 2]> {
        self.time_bounds
    }

    pub fn untimed_packets(&self) -> u64 {
        self.untimed
    }

    pub fn interfaces(&self) -> impl Iterator<Item = &Interface> {
        self.legacy_interface
            .iter()
            .chain(self.sections.iter().flat_map(|section| &section.interfaces))
    }

    pub fn interface(&self, id: InterfaceId) -> Option<&Interface> {
        if let Some(interface) = &self.legacy_interface {
            return (interface.id == id).then_some(interface);
        }
        self.sections
            .get(id.section as usize)?
            .interfaces
            .get(id.ordinal as usize)
    }

    pub(super) fn position<R: BufRead + Seek>(
        &self,
        reader: &mut Reader<R>,
        row: u64,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        if reader.format() != self.format {
            return Err(Error::Invalid(
                "index belongs to a different kind of capture",
            ));
        }
        if row >= self.points {
            return Err(Error::Invalid("packet row is outside the capture"));
        }
        let choice = self
            .checkpoints
            .partition_point(|point| point.row <= row)
            .saturating_sub(1);
        let checkpoint = self.checkpoints.get(choice).ok_or(Error::Truncated)?;
        if !(checkpoint.row..=row).contains(&reader.row) {
            reader.source.seek(checkpoint.offset, cancel)?;
            reader.row = checkpoint.row;
            if let Container::Ng(ng) = &mut reader.container {
                let section = self
                    .sections
                    .get(checkpoint.section as usize)
                    .ok_or(Error::Truncated)?;
                *ng = ng::Ng::from_checkpoint(section, checkpoint.interface_count);
            }
        }
        let mut bytes = Vec::new();
        while reader.row < row {
            reader
                .next(0, &mut bytes, cancel)?
                .ok_or(Error::Truncated)?;
        }
        Ok(())
    }
}

pub(super) fn build<R: BufRead + Seek>(
    reader: &mut Reader<R>,
    cancel: &AtomicBool,
) -> Result<Index, Error> {
    reader.rewind(cancel)?;
    let mut activity = Activity::default();
    let mut checkpoints = Vec::new();
    let mut stride = INITIAL_STRIDE;
    let mut bytes = Vec::new();
    while let Some(record) = reader.next(0, &mut bytes, cancel)? {
        activity.push(record.header);
        if record.header.row.is_multiple_of(stride) {
            if checkpoints.len() == MAX_CHECKPOINTS {
                stride *= 2;
                let mut index = 0;
                checkpoints.retain(|_| {
                    index += 1;
                    index % 2 == 1
                });
            }
            if record.header.row.is_multiple_of(stride) {
                checkpoints.push(make_checkpoint(&reader.container, record));
            }
        }
    }
    let (sections, legacy_interface) = match &reader.container {
        Container::Legacy(legacy) => (Vec::new(), Some(legacy.interface.clone())),
        Container::Ng(ng) => (ng.sections().cloned().collect(), None),
    };
    let time_bounds = activity.bounds;
    let untimed = activity.untimed;
    Ok(Index {
        format: reader.format(),
        points: reader.row,
        sections,
        legacy_interface,
        checkpoints,
        activity: activity.finish(),
        time_bounds,
        untimed,
    })
}

fn make_checkpoint(container: &Container, record: Record) -> Checkpoint {
    Checkpoint {
        row: record.header.row,
        offset: record.offset,
        section: record.header.interface.section,
        interface_count: match container {
            Container::Legacy(_) => 1,
            Container::Ng(ng) => ng.section.interfaces.len(),
        },
    }
}

pub(super) fn window<R: BufRead + Seek>(
    reader: &mut Reader<R>,
    index: &Index,
    rows: Range<u64>,
    cancel: &AtomicBool,
) -> Result<Vec<PacketRow>, Error> {
    let end = rows.end.min(index.points());
    let count = end.saturating_sub(rows.start);
    if count > DETAIL_ROWS {
        return Err(Error::Limit("requested packet rows"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    reader.position_at(index, rows.start, cancel)?;
    let mut result = Vec::with_capacity(count as usize);
    let mut bytes = Vec::new();
    for _ in 0..count {
        let record = reader
            .next(PROTOCOL_BYTES, &mut bytes, cancel)?
            .ok_or(Error::Truncated)?;
        result.push(record.row(&bytes));
    }
    Ok(result)
}

#[derive(Default)]
struct Activity {
    bounds: Option<[i128; 2]>,
    untimed: u64,
    buckets: BTreeMap<i128, Count>,
    shift: u32,
}

pub(super) fn activity(headers: impl Iterator<Item = PacketHeader>) -> Vec<ActivityBin> {
    let mut activity = Activity::default();
    for header in headers {
        activity.push(header);
    }
    activity.finish()
}

#[derive(Default)]
struct Count {
    packets: u64,
    bytes: u64,
    first: u64,
    last: u64,
}

impl Count {
    fn merge(&mut self, other: Self) {
        if self.packets == 0 {
            self.first = other.first;
        } else {
            self.first = self.first.min(other.first);
        }
        self.last = self.last.max(other.last);
        self.packets += other.packets;
        self.bytes += other.bytes;
    }
}

impl Activity {
    fn push(&mut self, header: PacketHeader) {
        let Some(time) = header.time else {
            self.untimed += 1;
            return;
        };
        let ns = time.unix_nanoseconds();
        let [mut first, mut end] = self.bounds.unwrap_or([ns, ns]);
        first = first.min(ns);
        end = end.max(ns);
        self.bounds = Some([first, end]);
        self.fit(first, end);
        self.buckets
            .entry(ns >> self.shift)
            .or_default()
            .merge(Count {
                packets: 1,
                bytes: u64::from(header.captured_len),
                first: header.row,
                last: header.row,
            });
    }

    fn fit(&mut self, first: i128, end: i128) {
        let mut shift = self.shift;
        while (end >> shift) - (first >> shift) >= MAX_BUCKETS {
            shift += 1;
        }
        let delta = shift - self.shift;
        if delta != 0 {
            self.shift = shift;
            let prior = std::mem::take(&mut self.buckets);
            for (key, count) in prior {
                self.buckets.entry(key >> delta).or_default().merge(count);
            }
        }
    }

    fn finish(self) -> Vec<ActivityBin> {
        let shift = self.shift;
        self.buckets
            .into_iter()
            .map(|(key, count)| ActivityBin {
                start_ns: key << shift,
                end_ns: (key + 1) << shift,
                packets: count.packets,
                captured_bytes: count.bytes,
                first_row: count.first,
                last_row: count.last,
            })
            .collect()
    }
}
