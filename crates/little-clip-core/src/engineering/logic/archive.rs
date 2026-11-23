//! Seekable logic archive members backed by a retained regular file.

use super::check_cancel;
use flate2::read::DeflateDecoder;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    os::unix::fs::FileExt,
    sync::{Arc, Mutex, OnceLock, atomic::AtomicBool},
};
use zip::{
    CompressionMethod, ZipArchive,
    read::{ArchiveOffset, Config},
};

const MAX_DIRECTORY: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: u64 = 32_768;
type SpoolFile = Arc<OnceLock<Result<File, String>>>;
const IO_BLOCK: usize = 64 * 1024;

pub(super) struct Archive {
    reader: ZipArchive<Positioned>,
    members: BTreeMap<String, usize>,
    file: Arc<File>,
    data_end: u64,
    spool: SpoolFile,
    spool_length: u64,
}

impl Archive {
    pub(super) fn open(file: Arc<File>, len: u64) -> Result<Self, String> {
        let mut signature = [0; 4];
        file.read_exact_at(&mut signature, 0).map_err(io_error)?;
        if signature != *b"PK\x03\x04" {
            return Err("A logic capture archive must start with a ZIP file header.".into());
        }
        let (data_end, count) = read_directory(&file, len)?;
        let config = Config {
            archive_offset: ArchiveOffset::Known(0),
        };
        let reader = ZipArchive::with_config(config, Positioned::new(Arc::clone(&file), 0..len))
            .map_err(zip_error)?;
        if reader.central_directory_start() != data_end
            || reader.len() as u64 != count
            || reader.has_duplicate_names()
        {
            return Err("Logic ZIP directory is inconsistent or has duplicate entries.".into());
        }
        let members = reader
            .file_names()
            .enumerate()
            .map(|(index, name)| name.map(|name| (name.into_owned(), index)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(zip_error)?;
        if members.len() as u64 != count {
            return Err("Logic capture archive has duplicate entries.".into());
        }
        Ok(Self {
            reader,
            members,
            file,
            data_end,
            spool: Arc::new(OnceLock::new()),
            spool_length: 0,
        })
    }

    pub(super) fn names(&self) -> impl Iterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    pub(super) fn metadata(&mut self, name: &str, limit: usize) -> Result<Vec<u8>, String> {
        crate::zip_content::entry(&mut self.reader, name, limit)
    }

    pub(super) fn member(&mut self, name: &str) -> Result<Arc<Member>, String> {
        let index = self
            .members
            .get(name)
            .ok_or_else(|| format!("Missing logic member {name}."))?;
        let member = self.reader.by_index_raw(*index).map_err(zip_error)?;
        let start = member
            .data_start()
            .ok_or("Logic archive member lacks a data offset.")?;
        let end = start
            .checked_add(member.compressed_size())
            .filter(|end| *end <= self.data_end)
            .ok_or("Logic member is outside the archive payload.")?;
        if !member.is_file() || member.encrypted() {
            return Err("Logic archive member must be a readable file.".into());
        }
        let compression = match member.compression() {
            CompressionMethod::Stored if member.compressed_size() == member.size() => {
                Storage::Stored
            }
            CompressionMethod::Deflated => {
                let offset = self.spool_length;
                self.spool_length = offset
                    .checked_add(member.size())
                    .filter(|len| *len < i64::MAX as u64)
                    .ok_or("Logic decompression offsets overflow.")?;
                Storage::Deflated {
                    offset,
                    file: Arc::clone(&self.spool),
                    state: Mutex::new(Decoder::Empty),
                }
            }
            _ => return Err("Unsupported logic archive compression.".into()),
        };
        Ok(Arc::new(Member {
            file: Arc::clone(&self.file),
            data: start..end,
            size: member.size(),
            crc: member.crc32(),
            compression,
        }))
    }
}

pub(super) struct Member {
    file: Arc<File>,
    data: Range<u64>,
    pub(super) size: u64,
    crc: u32,
    compression: Storage,
}

enum Storage {
    Stored,
    Deflated {
        offset: u64,
        file: SpoolFile,
        state: Mutex<Decoder>,
    },
}

enum Decoder {
    Empty,
    Active(Box<Inflation>),
    Ready,
    Failed(String),
}

impl Member {
    pub(super) fn read_exact_at(
        &self,
        target: &mut [u8],
        offset: u64,
        remaining_inflate_bytes: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<bool, String> {
        let end = offset
            .checked_add(target.len() as u64)
            .filter(|end| *end <= self.size)
            .ok_or("Logic data window falls outside its member.")?;
        check_cancel(cancel)?;
        match &self.compression {
            Storage::Stored => self
                .file
                .read_exact_at(target, self.data.start + offset)
                .map(|()| true)
                .map_err(io_error),
            Storage::Deflated {
                offset: start,
                file,
                state,
            } => {
                let file = file
                    .get_or_init(|| tempfile::tempfile().map_err(io_error))
                    .as_ref()
                    .map_err(Clone::clone)?;
                let mut state = state
                    .lock()
                    .map_err(|_| "Logic inflation cache is unavailable.")?;
                let ready = self.inflate(
                    &mut state,
                    file,
                    *start,
                    end,
                    remaining_inflate_bytes,
                    cancel,
                )?;
                if ready {
                    file.read_exact_at(target, start + offset)
                        .map_err(io_error)?;
                }
                Ok(ready)
            }
        }
    }

    fn inflate(
        &self,
        state: &mut Decoder,
        file: &File,
        start: u64,
        end: u64,
        budget: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<bool, String> {
        if matches!(*state, Decoder::Empty) {
            let bytes = Positioned::new(Arc::clone(&self.file), self.data.clone());
            *state = Decoder::Active(Box::new(Inflation {
                crc: flate2::Crc::new(),
                decoder: DeflateDecoder::new(bytes),
                available: 0,
            }));
        }
        match state {
            Decoder::Active(inflation) => {
                let result = inflation.advance(file, start, end, budget, cancel);
                if let Err(error) = result {
                    if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        *state = Decoder::Failed(error.clone());
                    }
                    return Err(error);
                }
                if inflation.available == self.size {
                    if inflation.check(self.crc)? {
                        *state = Decoder::Ready;
                    } else {
                        let error =
                            "Logic archive member has a mismatched checksum or length.".to_owned();
                        *state = Decoder::Failed(error.clone());
                        return Err(error);
                    }
                    return Ok(true);
                }
                Ok(inflation.available >= end)
            }
            Decoder::Ready => Ok(true),
            Decoder::Failed(error) => Err(error.clone()),
            Decoder::Empty => unreachable!("decoder initialized above"),
        }
    }
}

struct Inflation {
    decoder: DeflateDecoder<Positioned>,
    crc: flate2::Crc,
    available: u64,
}

impl Inflation {
    fn advance(
        &mut self,
        file: &File,
        start: u64,
        end: u64,
        budget: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut bytes = [0; IO_BLOCK];
        while self.available < end && *budget != 0 {
            check_cancel(cancel)?;
            let size = (end - self.available).min(bytes.len() as u64).min(*budget) as usize;
            self.decoder
                .read_exact(&mut bytes[..size])
                .map_err(io_error)?;
            file.write_all_at(&bytes[..size], start + self.available)
                .map_err(io_error)?;
            self.crc.update(&bytes[..size]);
            self.available += size as u64;
            *budget -= size as u64;
        }
        Ok(())
    }

    fn check(&mut self, expected: u32) -> Result<bool, String> {
        let mut excess = [0];
        Ok(self.decoder.read(&mut excess).map_err(io_error)? == 0 && self.crc.sum() == expected)
    }
}

#[derive(Clone)]
pub(super) struct Positioned {
    file: Arc<File>,
    bytes: Range<u64>,
    offset: u64,
}

impl Positioned {
    pub(super) fn new(file: Arc<File>, bytes: Range<u64>) -> Self {
        Self {
            file,
            bytes,
            offset: 0,
        }
    }
}

impl Read for Positioned {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let length = self.bytes.end - self.bytes.start;
        let size = length.saturating_sub(self.offset).min(target.len() as u64) as usize;
        if size == 0 {
            return Ok(0);
        }
        let read = self
            .file
            .read_at(&mut target[..size], self.bytes.start + self.offset)?;
        self.offset += read as u64;
        Ok(read)
    }
}

impl Seek for Positioned {
    fn seek(&mut self, cursor: SeekFrom) -> io::Result<u64> {
        let offset = match cursor {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(offset) => self.offset.checked_add_signed(offset),
            SeekFrom::End(offset) => (self.bytes.end - self.bytes.start).checked_add_signed(offset),
        };
        self.offset = offset
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid capture offset"))?;
        Ok(self.offset)
    }
}

fn read_directory(file: &File, len: u64) -> Result<(u64, u64), String> {
    let size = len.min(65_557);
    let mut tail = vec![0; size as usize];
    file.read_exact_at(&mut tail, len - size)
        .map_err(io_error)?;
    let at = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| is_final_record(&tail[at..]))
        .ok_or("Missing logic archive directory.")?;
    let header = &tail[at..];
    let eocd_offset = len - size + at as u64;
    let (offset, size, count, boundary) = directory_fields(file, header, eocd_offset)?;
    if count > MAX_ENTRIES || size > MAX_DIRECTORY || offset.checked_add(size) != Some(boundary) {
        return Err("Logic archive directory exceeds the import budget.".into());
    }
    Ok((offset, count))
}

fn is_final_record(tail: &[u8]) -> bool {
    tail.starts_with(b"PK\x05\x06") && tail.len() == 22 + usize::from(le_u16(tail, 20))
}

fn directory_fields(file: &File, header: &[u8], eocd: u64) -> Result<(u64, u64, u64, u64), String> {
    let count = le_u16(header, 10);
    let size = le_u32(header, 12);
    let offset = le_u32(header, 16);
    if header[4..8] != [0; 4] || le_u16(header, 8) != count {
        return Err("Multi-disk logic archives are unsupported.".into());
    }
    let basic = (u64::from(offset), u64::from(size), u64::from(count), eocd);
    if count != u16::MAX && size != u32::MAX && offset != u32::MAX && basic.0 + basic.1 == eocd {
        return Ok(basic);
    }
    let expanded = read_zip64(file, eocd)?;
    let consistent = (offset == u32::MAX || basic.0 == expanded.0)
        && (size == u32::MAX || basic.1 == expanded.1)
        && (count == u16::MAX || basic.2 == expanded.2);
    if consistent {
        Ok(expanded)
    } else {
        Err("Logic ZIP64 directory disagrees with the classic directory fields.".into())
    }
}

fn read_zip64(file: &File, eocd: u64) -> Result<(u64, u64, u64, u64), String> {
    let locator_offset = eocd.checked_sub(20).ok_or("Missing ZIP64 locator.")?;
    let mut locator = [0; 20];
    file.read_exact_at(&mut locator, locator_offset)
        .map_err(io_error)?;
    if !locator.starts_with(b"PK\x06\x07") || le_u32(&locator, 4) != 0 || le_u32(&locator, 16) != 1
    {
        return Err("Invalid or multi-disk ZIP64 logic archive.".into());
    }
    let at = le_u64(&locator, 8);
    if at.checked_add(56).is_none_or(|end| end > locator_offset) {
        return Err("Invalid ZIP64 directory position.".into());
    }
    let mut record = [0; 56];
    file.read_exact_at(&mut record, at).map_err(io_error)?;
    let record_end = at
        .checked_add(12)
        .and_then(|offset| offset.checked_add(le_u64(&record, 4)));
    if !record.starts_with(b"PK\x06\x06")
        || record_end != Some(locator_offset)
        || record[16..24] != [0; 8]
        || le_u64(&record, 24) != le_u64(&record, 32)
    {
        return Err("Invalid ZIP64 logic directory.".into());
    }
    Ok((
        le_u64(&record, 48),
        le_u64(&record, 40),
        le_u64(&record, 32),
        at,
    ))
}

fn le_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn io_error(error: io::Error) -> String {
    format!("Reading logic archive. {error}")
}

fn zip_error(error: zip::result::ZipError) -> String {
    format!("Reading logic ZIP archive. {error}")
}
