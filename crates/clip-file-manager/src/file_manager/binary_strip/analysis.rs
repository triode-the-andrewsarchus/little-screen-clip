use super::{Document, Section};
use object::{Object, ObjectSection, read::ReadCache};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const BINS: usize = 65536;

// Check requested allocations before ReadCache allocates its buffers.
#[derive(Clone, Copy)]
struct Headers<'a> {
    cache: &'a ReadCache<File>,
    budget: &'a std::cell::Cell<u64>,
}
impl<'a> object::read::ReadRef<'a> for Headers<'a> {
    fn len(self) -> Result<u64, ()> {
        self.cache.len()
    }
    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'a [u8], ()> {
        self.budget
            .set(self.budget.get().checked_sub(size).ok_or(())?);
        self.cache.read_bytes_at(offset, size)
    }
    fn read_bytes_at_until(
        self,
        range: std::ops::Range<u64>,
        delimiter: u8,
    ) -> Result<&'a [u8], ()> {
        self.budget
            .set(self.budget.get().checked_sub(4096).ok_or(())?);
        self.cache.read_bytes_at_until(range, delimiter)
    }
}

pub(super) fn inspect(path: &Path, cancelled: &AtomicBool) -> Result<Document, String> {
    let source = crate::file_source::FileSource::capture(path)?;
    let mut file = crate::file_source::open_regular_file(path, true).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let mut prefix = [0; 512];
    let count = file.read(&mut prefix).map_err(|e| e.to_string())?;
    let uf2 = count == 512
        && word(&prefix, 0) == Some(0x0a324655)
        && word(&prefix, 4) == Some(0x9e5d5157)
        && word(&prefix, 508) == Some(0x0ab16f30)
        && size % 512 == 0;
    let (sections, icon, description) = metadata(&file, size, &prefix, uf2)?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let (values, hash, payloads) = summarize(&mut file, size, uf2, cancelled)?;
    source.check()?;
    Ok(Document {
        size,
        description,
        sections,
        values,
        hash,
        icon,
        uf2: uf2 && !payloads.is_empty(),
        uf2_payload: payloads,
    })
}
pub(super) fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
fn section_color(kind: object::SectionKind) -> [u8; 3] {
    use object::SectionKind::*;
    match kind {
        Text => [117, 180, 230],
        ReadOnlyData | ReadOnlyString => [113, 199, 157],
        Data | Tls => [231, 179, 99],
        Debug | DebugString | Metadata => [185, 142, 218],
        _ => [151, 158, 171],
    }
}

fn metadata(
    file: &File,
    size: u64,
    prefix: &[u8],
    uf2: bool,
) -> Result<
    (
        Vec<Section>,
        Option<little_clip_core::ImageThumbnail>,
        String,
    ),
    String,
> {
    let cache = ReadCache::new(file.try_clone().map_err(|e| e.to_string())?);
    let budget = std::cell::Cell::new(16 * 1024 * 1024);
    let object = object::File::parse(Headers {
        cache: &cache,
        budget: &budget,
    })
    .ok();
    let mut sections = Vec::new();
    let mut icon = None;
    let description = if let Some(object) = object.as_ref() {
        for section in object.sections().take(4096) {
            if let Some((offset, length)) = section.file_range().filter(|(_, length)| *length > 0) {
                if offset >= size {
                    continue;
                }
                let name = section
                    .name()
                    .unwrap_or("section")
                    .chars()
                    .take(48)
                    .collect::<String>();
                if name == ".rsrc" && length <= 8 * 1024 * 1024 {
                    if let Ok(bytes) = section.data() {
                        icon = super::icon::extract(
                            bytes,
                            section
                                .address()
                                .saturating_sub(object.relative_address_base()),
                        );
                    }
                }
                sections.push(Section {
                    start: offset,
                    end: offset.saturating_add(length).min(size),
                    color: section_color(section.kind()),
                    name,
                });
            }
        }
        format!(
            "{:?} · {:?} {}-bit · {:?} · {} sections · entry 0x{:x}",
            object.format(),
            object.architecture(),
            if object.is_64() { 64 } else { 32 },
            object.kind(),
            sections.len(),
            object.entry()
        )
    } else if uf2 {
        format!(
            "UF2 firmware · {} blocks · target 0x{:08x} · family 0x{:08x}",
            size / 512,
            word(prefix, 12).unwrap_or(0),
            if word(prefix, 8).unwrap_or(0) & 0x2000 != 0 {
                word(prefix, 28).unwrap_or(0)
            } else {
                0
            }
        )
    } else {
        "Raw binary".into()
    };
    sections.sort_by_key(|s| s.start);
    Ok((sections, icon, description))
}

fn summarize(
    file: &mut File,
    size: u64,
    uf2: bool,
    cancelled: &AtomicBool,
) -> Result<(Vec<u8>, String, Vec<u16>), String> {
    let bins = (size.min(BINS as u64) as usize).max(1);
    let mut sums = vec![0u64; bins];
    let mut counts = vec![0u64; bins];
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut offset = 0u64;
    let mut payloads = Vec::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Preview cancelled".into());
        }
        let count = (size.saturating_sub(offset)).min(buffer.len() as u64) as usize;
        if count == 0 {
            break;
        }
        file.read_exact(&mut buffer[..count])
            .map_err(|e| e.to_string())?;
        if uf2 && size / 512 <= 1_000_000 {
            for block in buffer[..count].chunks_exact(512) {
                let payload = if word(block, 0) == Some(0x0a324655)
                    && word(block, 4) == Some(0x9e5d5157)
                    && word(block, 508) == Some(0x0ab16f30)
                {
                    word(block, 16).unwrap_or(0).min(476) as u16
                } else {
                    0
                };
                payloads.push(payload);
            }
        }
        hash.update(&buffer[..count]);
        let mut consumed = 0;
        while consumed < count {
            let bin =
                ((offset as u128 * bins as u128 / size.max(1) as u128) as usize).min(bins - 1);
            let boundary = (((bin + 1) as u128 * size as u128).div_ceil(bins as u128)) as u64;
            let length = (boundary.saturating_sub(offset) as usize)
                .max(1)
                .min(count - consumed);
            sums[bin] += buffer[consumed..consumed + length]
                .iter()
                .map(|&b| u64::from(b))
                .sum::<u64>();
            counts[bin] += length as u64;
            offset += length as u64;
            consumed += length;
        }
    }
    if offset != size {
        return Err("Binary changed while reading".into());
    }
    Ok((
        sums.into_iter()
            .zip(counts)
            .map(|(sum, n)| if n == 0 { 0 } else { (sum / n) as u8 })
            .collect(),
        format!("{:x}", hash.finalize()),
        payloads,
    ))
}
