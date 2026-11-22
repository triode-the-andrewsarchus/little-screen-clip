//! Read plain Matroska subtitle blocks without reading the interleaved media payloads.
use super::ebml::{Error, Reader, Result, vint};
use super::{MAX_CUE_BYTES, MAX_SUBTITLE_BYTES, Progress, SubtitleCue};
use std::{
    fs::{File, OpenOptions},
    ops::Range,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Instant,
};

const EBML: u32 = 0x1a45dfa3;
const SEGMENT: u32 = 0x18538067;
const INFO: u32 = 0x1549a966;
const TRACKS: u32 = 0x1654ae6b;
const SEEK_HEAD: u32 = 0x114d9b74;
const CUES: u32 = 0x1c53bb6b;
const CLUSTER: u32 = 0x1f43b675;
const TIMESTAMP: u32 = 0xe7;
const SIMPLE_BLOCK: u32 = 0xa3;
const BLOCK_GROUP: u32 = 0xa0;
const BLOCK: u32 = 0xa1;
const DIRECT_READERS: usize = 32;
const SCAN_PARTS: usize = DIRECT_READERS * 4;
const MAX_CUE_INDEX_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn try_read(
    file: &File,
    ordinal: u32,
    deadline: Instant,
    progress: Option<&Progress>,
) -> super::Result<Option<Vec<SubtitleCue>>> {
    match read(file, ordinal, deadline, progress) {
        Ok(cues) => Ok(Some(cues)),
        Err(Error::Unsupported) => Ok(None),
        Err(Error::Io(error)) => Err(error.to_string()),
        Err(Error::Deadline) => Err("Reading the subtitle track timed out".into()),
        Err(Error::Limit) => Err("The subtitle track exceeds the search limit".into()),
    }
}

fn read(
    file: &File,
    ordinal: u32,
    deadline: Instant,
    progress: Option<&Progress>,
) -> Result<Vec<SubtitleCue>> {
    let len = file.metadata()?.len();
    let mut header = Reader::new(file, deadline);
    let ebml = header.element(0..len)?;
    if ebml.id != EBML || ebml.unknown_size {
        return Err(Error::Unsupported);
    }
    let segment = header.element(ebml.data.end..len)?;
    if segment.id != SEGMENT || segment.data.end != len {
        return Err(Error::Unsupported);
    }

    // Buffered reads preserve normal OS caching for local files. A separate
    // description gives sparse CIFS reads direct I/O without changing playback.
    let network = is_cifs(file);
    let direct = network
        .then(|| {
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECT)
                .open(format!("/proc/self/fd/{}", file.as_raw_fd()))
                .ok()
        })
        .flatten();
    let input = direct.as_ref().unwrap_or(file);
    let mut reader = Reader::new(input, deadline);
    let parts = if network { SCAN_PARTS } else { 1 };
    let mut plan = plan(&mut reader, segment.data, ordinal, parts)?;
    if plan.ranges.len() == 1 {
        let range = plan.ranges.pop().ok_or(Error::Unsupported)?;
        return plan.read_range(&mut reader, range);
    }
    scan_parallel(&plan, input, deadline, progress)
}

fn scan_parallel(
    plan: &Plan,
    input: &File,
    deadline: Instant,
    progress: Option<&Progress>,
) -> Result<Vec<SubtitleCue>> {
    let next = AtomicUsize::new(0);
    let next = &next;
    thread::scope(|scope| {
        let tasks = (0..plan.ranges.len().min(DIRECT_READERS))
            .map(|_| {
                scope.spawn(move || {
                    let mut reader = Reader::new(input, deadline);
                    let mut results = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(range) = plan.ranges.get(index) else {
                            return Ok::<_, Error>(results);
                        };
                        let cues = plan.read_range(&mut reader, range.clone())?;
                        if let Some(progress) = progress {
                            progress.publish(&cues)?;
                        }
                        results.push((index, cues));
                    }
                })
            })
            .collect::<Vec<_>>();
        let mut results = Vec::new();
        for task in tasks {
            results.extend(task.join().map_err(|_| Error::Unsupported)??);
        }
        results.sort_unstable_by_key(|(index, _)| *index);
        Ok(results.into_iter().flat_map(|(_, cues)| cues).collect())
    })
}

fn is_cifs(file: &File) -> bool {
    rustix::fs::fstatfs(file)
        .is_ok_and(|info| matches!(info.f_type as u64, 0xff53_4d42 | 0xfe53_4d42))
}

struct Plan {
    number: u64,
    scale_ns: u64,
    ranges: Vec<Range<u64>>,
    bytes: AtomicUsize,
}

fn plan(
    reader: &mut Reader<'_>,
    segment: Range<u64>,
    ordinal: u32,
    readers: usize,
) -> Result<Plan> {
    let mut at = segment.start;
    let mut scale_ns = 1_000_000;
    let mut number = None;
    let mut cue_offset = None;
    while at < segment.end {
        let element = reader.element(at..segment.end)?;
        if element.id == CLUSTER {
            break;
        }
        if element.unknown_size {
            return Err(Error::Unsupported);
        }
        at = element.data.end;
        match element.id {
            INFO => scale_ns = timestamp_scale(reader, element.data)?,
            TRACKS if number.is_none() => {
                number = Some(track_number(reader, element.data, ordinal)?);
            }
            TRACKS => return Err(Error::Unsupported),
            SEEK_HEAD if readers > 1 && cue_offset.is_none() => {
                cue_offset = seek_cues(reader, element.data)?;
            }
            _ => {}
        }
    }
    let ranges = scan_ranges(reader, segment.start, at..segment.end, cue_offset, readers)?;
    Ok(Plan {
        number: number.ok_or(Error::Unsupported)?,
        scale_ns,
        ranges,
        bytes: AtomicUsize::new(0),
    })
}

fn timestamp_scale(reader: &mut Reader<'_>, info: Range<u64>) -> Result<u64> {
    let mut scale = 1_000_000;
    reader.walk(info, |reader, field| {
        if field.id == 0x2ad7b1 {
            scale = reader.uint(field.data)?;
        }
        Ok(())
    })?;
    if scale == 0 {
        return Err(Error::Unsupported);
    }
    Ok(scale)
}

fn seek_cues(reader: &mut Reader<'_>, head: Range<u64>) -> Result<Option<u64>> {
    let mut offset = None;
    reader.walk(head, |reader, seek| {
        if seek.id != 0x4dbb {
            return Ok(());
        }
        let mut id = None;
        let mut position = None;
        reader.walk(seek.data, |reader, field| {
            match field.id {
                0x53ab => id = Some(reader.uint(field.data)?),
                0x53ac => position = Some(reader.uint(field.data)?),
                _ => {}
            }
            Ok(())
        })?;
        if id == Some(u64::from(CUES)) {
            offset = position;
        }
        Ok(())
    })?;
    Ok(offset)
}

fn scan_ranges(
    reader: &mut Reader<'_>,
    segment: u64,
    clusters: Range<u64>,
    cue_offset: Option<u64>,
    readers: usize,
) -> Result<Vec<Range<u64>>> {
    let mut starts = vec![clusters.start];
    if let Some(offset) = cue_offset
        .and_then(|offset| segment.checked_add(offset))
        .filter(|&offset| offset < clusters.end)
    {
        let table = reader.element(offset..clusters.end)?;
        if table.id == CUES
            && !table.unknown_size
            && table.data.end - table.data.start <= MAX_CUE_INDEX_BYTES
        {
            add_boundaries(reader, table.data, segment, &clusters, readers, &mut starts)?;
        }
    }
    starts.push(clusters.end);
    Ok(starts.windows(2).map(|pair| pair[0]..pair[1]).collect())
}

fn add_boundaries(
    reader: &mut Reader<'_>,
    table: Range<u64>,
    segment: u64,
    clusters: &Range<u64>,
    readers: usize,
    starts: &mut Vec<u64>,
) -> Result<()> {
    let span = (clusters.end - clusters.start).div_ceil(readers as u64);
    let mut previous = clusters.start;
    // Cues are optional and need not cover every cluster. They only supply
    // split points. Each worker scans all elements between adjacent points.
    reader.walk(table, |reader, point| {
        if point.id != 0xbb {
            return Ok(());
        }
        reader.walk(point.data, |reader, positions| {
            if positions.id != 0xb7 {
                return Ok(());
            }
            reader.walk(positions.data, |reader, position| {
                if position.id != 0xf1 {
                    return Ok(());
                }
                if let Some(start) = segment.checked_add(reader.uint(position.data)?)
                    && start > previous
                    && start < clusters.end
                    && start - previous >= span
                {
                    starts.push(start);
                    previous = start;
                }
                Ok(())
            })
        })
    })
}

fn track_number(reader: &mut Reader<'_>, tracks: Range<u64>, ordinal: u32) -> Result<u64> {
    let mut selected = None;
    let mut index = 0;
    reader.walk(tracks, |reader, entry| {
        if entry.id != 0xae {
            return Ok(());
        }
        let mut number = None;
        let mut kind = 0;
        let mut plain_text = false;
        let mut transformed = false;
        reader.walk(entry.data, |reader, field| {
            match field.id {
                0xd7 => number = Some(reader.uint(field.data)?),
                0x83 => kind = reader.uint(field.data)?,
                0x86 => {
                    let (codec, len) = reader.prefix::<16>(field.data)?;
                    plain_text = &codec[..len] == b"S_TEXT/UTF8";
                }
                // TrackTimestampScale, TrackOffset, CodecDelay, ContentEncodings.
                0x23314f | 0x537f | 0x56aa | 0x6d80 => transformed = true,
                _ => {}
            }
            Ok(())
        })?;
        if kind == 17 {
            // Use the general decoder for mixed subtitle formats so its stream
            // ordinals also cover formats this reader does not recognize.
            if !plain_text || transformed {
                return Err(Error::Unsupported);
            }
            if index == ordinal {
                selected = number;
            }
            index += 1;
        }
        Ok(())
    })?;
    selected
        .filter(|&number| number > 0)
        .ok_or(Error::Unsupported)
}

impl Plan {
    fn read_range(&self, reader: &mut Reader<'_>, range: Range<u64>) -> Result<Vec<SubtitleCue>> {
        let mut cues = Vec::new();
        reader.walk(range, |reader, element| {
            if element.id == CLUSTER {
                self.read_cluster(reader, element.data, &mut cues)?;
            }
            Ok(())
        })?;
        Ok(cues)
    }

    fn read_cluster(
        &self,
        reader: &mut Reader<'_>,
        data: Range<u64>,
        cues: &mut Vec<SubtitleCue>,
    ) -> Result<()> {
        let mut timestamp = None;
        reader.walk(data, |reader, element| {
            match element.id {
                TIMESTAMP => timestamp = Some(reader.uint(element.data)?),
                SIMPLE_BLOCK => self.read_block(reader, element.data, timestamp, cues)?,
                BLOCK_GROUP => reader.walk(element.data, |reader, child| {
                    if child.id == BLOCK {
                        self.read_block(reader, child.data, timestamp, cues)?;
                    }
                    Ok(())
                })?,
                _ => {}
            }
            Ok(())
        })
    }

    fn read_block(
        &self,
        reader: &mut Reader<'_>,
        data: Range<u64>,
        timestamp: Option<u64>,
        cues: &mut Vec<SubtitleCue>,
    ) -> Result<()> {
        let (bytes, len) = reader.prefix::<11>(data.clone())?;
        let (number, header) = vint(&bytes[..len], false).ok_or(Error::Unsupported)?;
        if number != self.number {
            return Ok(());
        }
        let [high, low, flags] = bytes[..len]
            .get(header..header + 3)
            .ok_or(Error::Unsupported)?
        else {
            return Err(Error::Unsupported);
        };
        if flags & 0x06 != 0 {
            return Err(Error::Unsupported); // Laced subtitle block.
        }
        let offset = i16::from_be_bytes([*high, *low]);
        let ticks = i128::from(timestamp.ok_or(Error::Unsupported)?) + i128::from(offset);
        let ns = ticks
            .checked_mul(i128::from(self.scale_ns))
            .ok_or(Error::Unsupported)?;
        let start_ms = u64::try_from(ns.max(0) / 1_000_000).map_err(|_| Error::Unsupported)?;
        let position = data.start + header as u64 + 3;
        let len = data.end - position;
        if len > MAX_CUE_BYTES as u64 {
            return Err(Error::Limit);
        }
        let used = self.bytes.fetch_add(len as usize + 64, Ordering::Relaxed);
        if used > MAX_SUBTITLE_BYTES {
            return Err(Error::Limit);
        }
        let mut text = vec![0; len as usize];
        reader.read(position, &mut text)?;
        let mut text = String::from_utf8(text).map_err(|_| Error::Unsupported)?;
        text.truncate(text.trim_end_matches('\0').len());
        if !text.trim().is_empty() {
            cues.push(SubtitleCue {
                start_ms,
                text,
                markup: true,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sparse_matroska_reader_returns_the_selected_text_track() {
        let file = File::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../little-clip-core/tests/fixtures/video/playback-tracks.mkv",
        ))
        .unwrap();
        let cues = try_read(&file, 1, Instant::now() + Duration::from_secs(2), None)
            .unwrap()
            .expect("plain Matroska subtitle fast path");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "SECOND SUBTITLE");
        assert_eq!(cues[0].start_ms, 0);
    }
}
