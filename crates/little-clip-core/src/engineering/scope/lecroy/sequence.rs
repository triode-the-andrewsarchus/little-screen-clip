use super::ByteOrder;
use std::{fs::File, ops::Range, os::unix::fs::FileExt};

const TRIGGER_RECORD_BYTES: u64 = 16;
const MAX_SEGMENTS: u64 = 200;

pub(super) struct Segment {
    pub index: u64,
    pub data: Range<u64>,
    pub local_first: u64,
    pub origin: f64,
}

pub(super) fn read(
    file: &File,
    order: ByteOrder,
    records: Range<u64>,
    segments: u64,
    samples: u64,
    eligible: Range<u64>,
) -> Result<Vec<Segment>, String> {
    if !(1..=MAX_SEGMENTS).contains(&segments)
        || records.end - records.start != segments * TRIGGER_RECORD_BYTES
        || !samples.is_multiple_of(segments)
    {
        return Err("Invalid or oversized LeCroy sequence timing table".into());
    }
    let mut timing = vec![0; (records.end - records.start) as usize];
    file.read_exact_at(&mut timing, records.start)
        .map_err(|error| format!("Reading LeCroy sequence timing. {error}"))?;
    let width = samples / segments;
    let mut result = Vec::new();
    for (index, record) in timing
        .chunks_exact(TRIGGER_RECORD_BYTES as usize)
        .enumerate()
    {
        let index = index as u64;
        let at = index * width;
        let data = eligible.start.max(at)..eligible.end.min(at + width);
        if data.is_empty() {
            continue;
        }
        // The first word is the segment trigger time on the capture timeline.
        // The second is the offset from that trigger to the nominal first sample.
        let origin = order.f64(record, 0) + order.f64(record, 8);
        if !origin.is_finite() {
            return Err("Invalid LeCroy segment time".into());
        }
        let local_first = data.start - at;
        result.push(Segment {
            index,
            data,
            local_first,
            origin,
        });
    }
    Ok(result)
}
