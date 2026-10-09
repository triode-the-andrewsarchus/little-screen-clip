//! Two streaming passes, with bounded line storage even for minified documents.
use std::{
    io::{self, BufRead, Read},
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) const LINE_LIMIT: usize = 16 * 1024;
pub(super) struct Shape {
    pub lines: u64,
    pub longest: usize,
    pub lossy: bool,
    pub prefix: String,
}

#[derive(Default)]
struct Utf8Check {
    pending: Vec<u8>,
    lossy: bool,
}
impl Utf8Check {
    fn update(&mut self, bytes: &[u8]) {
        if self.lossy {
            return;
        }
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.clear(),
            Err(error) if error.error_len().is_none() => {
                self.pending.drain(..error.valid_up_to());
            }
            Err(_) => {
                self.lossy = true;
                self.pending.clear();
            }
        }
    }
}

pub(super) fn scan(reader: &mut impl Read, cancelled: &AtomicBool) -> io::Result<Shape> {
    let mut buffer = [0; 64 * 1024];
    let mut utf8 = Utf8Check::default();
    let mut prefix = Vec::new();
    let (mut lines, mut longest, mut length, mut previous) = (0, 0, 0, None);
    loop {
        check_cancel(cancelled)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        prefix.extend(
            buffer[..count]
                .iter()
                .take((8192_usize).saturating_sub(prefix.len())),
        );
        for &byte in &buffer[..count] {
            if byte == 0 || (byte < 32 && !matches!(byte, b'\n' | b'\r' | b'\t' | 12)) {
                return Err(io::Error::other("Binary file"));
            }
            if byte == b'\r' || (byte == b'\n' && previous != Some(b'\r')) {
                lines += 1;
                longest = longest.max(length);
                length = 0;
            } else if byte != b'\n' {
                length += 1;
            }
            previous = Some(byte);
        }
        utf8.update(&buffer[..count]);
    }
    lines += u64::from(previous.is_some_and(|byte| !matches!(byte, b'\r' | b'\n')));
    Ok(Shape {
        lines,
        longest: longest.max(length),
        lossy: utf8.lossy || !utf8.pending.is_empty(),
        prefix: String::from_utf8_lossy(&prefix).into_owned(),
    })
}

pub(super) fn check_cancel(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(io::Error::other("Preview cancelled"))
    } else {
        Ok(())
    }
}

pub(super) struct Lines<R> {
    reader: R,
    skip_lf: bool,
}
impl<R: BufRead> Lines<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            skip_lf: false,
        }
    }
    pub fn next(&mut self, cancelled: &AtomicBool) -> io::Result<Option<String>> {
        let mut bytes = Vec::new();
        let mut seen = false;
        loop {
            check_cancel(cancelled)?;
            let buffer = self.reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok(seen.then(|| String::from_utf8_lossy(&bytes).into_owned()));
            }
            if self.skip_lf {
                self.skip_lf = false;
                if buffer[0] == b'\n' {
                    self.reader.consume(1);
                    continue;
                }
            }
            seen = true;
            let end = buffer.iter().position(|byte| matches!(byte, b'\r' | b'\n'));
            let length = end.unwrap_or(buffer.len());
            bytes.extend(
                buffer[..length]
                    .iter()
                    .take(LINE_LIMIT.saturating_sub(bytes.len())),
            );
            if let Some(end) = end {
                self.skip_lf = buffer[end] == b'\r';
                self.reader.consume(end + 1);
                bytes.push(b'\n');
                return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
            }
            self.reader.consume(length);
        }
    }
}
