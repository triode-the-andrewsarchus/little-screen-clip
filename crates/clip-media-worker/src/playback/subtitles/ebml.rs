//! Bounded, positioned EBML reads. Packet payloads can be skipped by their element size.

use std::{fs::File, io, ops::Range, os::unix::fs::FileExt, time::Instant};

const PAGE_SIZE: usize = 4096;
const MAX_HEADER_SIZE: usize = 12;

#[derive(Debug)]
pub(super) enum Error {
    Unsupported,
    Io(io::Error),
    Deadline,
    Limit,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(super) type Result<T> = std::result::Result<T, Error>;

#[repr(align(4096))]
struct Page([u8; PAGE_SIZE]);

pub(super) struct Reader<'a> {
    file: &'a File,
    page: Page,
    start: u64,
    len: usize,
    deadline: Instant,
}

pub(super) struct Element {
    pub id: u32,
    pub data: Range<u64>,
    pub unknown_size: bool,
}

impl<'a> Reader<'a> {
    pub fn new(file: &'a File, deadline: Instant) -> Self {
        Self {
            file,
            page: Page([0; PAGE_SIZE]),
            start: 0,
            len: 0,
            deadline,
        }
    }

    fn page_at(&mut self, offset: u64) -> Result<&[u8]> {
        let page_start = offset & !(PAGE_SIZE as u64 - 1);
        if self.len == 0 || self.start != page_start {
            if Instant::now() >= self.deadline {
                return Err(Error::Deadline);
            }
            self.len = loop {
                match self.file.read_at(&mut self.page.0, page_start) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    read => break read?,
                }
            };
            self.start = page_start;
        }
        let offset = (offset - self.start) as usize;
        self.page.0[..self.len]
            .get(offset..)
            .filter(|slice| !slice.is_empty())
            .ok_or(Error::Unsupported)
    }

    pub fn read(&mut self, mut offset: u64, mut output: &mut [u8]) -> Result<()> {
        while !output.is_empty() {
            let page = self.page_at(offset)?;
            let count = page.len().min(output.len());
            output[..count].copy_from_slice(&page[..count]);
            output = &mut output[count..];
            offset += count as u64;
        }
        Ok(())
    }

    pub fn prefix<const N: usize>(&mut self, range: Range<u64>) -> Result<([u8; N], usize)> {
        let mut buffer = [0; N];
        let len = (range.end - range.start).min(N as u64) as usize;
        self.read(range.start, &mut buffer[..len])?;
        Ok((buffer, len))
    }

    pub fn uint(&mut self, range: Range<u64>) -> Result<u64> {
        let len = range.end - range.start;
        if len > 8 {
            return Err(Error::Unsupported);
        }
        let mut bytes = [0; 8];
        self.read(range.start, &mut bytes[8 - len as usize..])?;
        Ok(u64::from_be_bytes(bytes))
    }

    pub fn element(&mut self, range: Range<u64>) -> Result<Element> {
        let start = range.start;
        let end = range.end;
        let (bytes, len) = self.prefix::<MAX_HEADER_SIZE>(range)?;
        let (id, id_len) = vint(&bytes[..len], true).ok_or(Error::Unsupported)?;
        let (size, size_len) = vint(&bytes[id_len..len], false).ok_or(Error::Unsupported)?;
        let data = start + (id_len + size_len) as u64;
        let unknown_size = size == (1 << (7 * size_len)) - 1;
        let stop = if unknown_size {
            end
        } else {
            data.checked_add(size)
                .filter(|&stop| stop <= end)
                .ok_or(Error::Unsupported)?
        };
        Ok(Element {
            id: id as u32,
            data: data..stop,
            unknown_size,
        })
    }

    pub fn walk(
        &mut self,
        mut range: Range<u64>,
        mut visit: impl FnMut(&mut Self, Element) -> Result<()>,
    ) -> Result<()> {
        while range.start < range.end {
            let element = self.element(range.clone())?;
            if element.unknown_size {
                return Err(Error::Unsupported);
            }
            range.start = element.data.end;
            visit(self, element)?;
        }
        Ok(())
    }
}

// IDs retain the marker bit. Sizes and track numbers omit it.
pub(super) fn vint(bytes: &[u8], id: bool) -> Option<(u64, usize)> {
    let marker = *bytes.first()?;
    let length = marker.leading_zeros() as usize + 1;
    if length > if id { 4 } else { 8 } {
        return None;
    }
    let first = if id {
        marker
    } else {
        marker ^ (1 << (8 - length))
    };
    let mut value = u64::from(first);
    for &byte in bytes.get(1..length)? {
        value = (value << 8) | u64::from(byte);
    }
    Some((value, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn element_header_can_cross_a_page_without_trusting_the_payload_size() {
        let file = tempfile::tempfile().unwrap();
        let offset = (PAGE_SIZE - 2) as u64;
        file.write_all_at(&[0x1f, 0x43, 0xb6, 0x75, 0x81, 0], offset)
            .unwrap();
        let mut reader = Reader::new(&file, Instant::now() + Duration::from_secs(1));
        let element = reader.element(offset..offset + 6).unwrap();
        assert_eq!(element.id, 0x1f43b675);
        assert_eq!(element.data, offset + 5..offset + 6);
        assert!(matches!(
            reader.element(offset..offset + 5),
            Err(Error::Unsupported)
        ));
    }
}
