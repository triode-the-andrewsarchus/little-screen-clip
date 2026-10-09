//! Bounded io_uring read-ahead for the standalone GPL video helper.
//! Each reader belongs to one decoder thread. Its borrowed descriptor stays
//! open until the reader is destroyed, and buffers stay pinned until completion.
use io_uring::{IoUring, opcode, types};
use std::{
    alloc::{Layout, alloc_zeroed, dealloc},
    io,
    ptr::{self, NonNull},
};

const BLOCK: usize = 64 * 1024;
const SLOTS: usize = 16;
const EMPTY: i32 = i32::MIN;
const PENDING: i32 = i32::MIN + 1;

struct Buffer {
    bytes: NonNull<u8>,
    offset: u64,
    result: i32,
    age: u64,
}
impl Buffer {
    fn new() -> io::Result<Self> {
        let layout = Layout::from_size_align(BLOCK, 4096).unwrap();
        // SAFETY: valid nonzero layout; allocation is exclusively owned here.
        let bytes = NonNull::new(unsafe { alloc_zeroed(layout) })
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOMEM))?;
        Ok(Self {
            bytes,
            offset: 0,
            result: EMPTY,
            age: 0,
        })
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: allocation/layout match new; Reader drops its ring first.
        unsafe {
            dealloc(
                self.bytes.as_ptr(),
                Layout::from_size_align(BLOCK, 4096).unwrap(),
            )
        };
    }
}

pub struct Reader {
    // Option permits explicitly closing the ring before freeing buffers.
    ring: Option<IoUring>,
    buffers: Vec<Buffer>,
    fd: i32,
    clock: u64,
    pending: usize,
    submitted_bytes: u64,
    delivered_bytes: u64,
}
impl Reader {
    fn new(fd: i32) -> io::Result<Self> {
        let ring = IoUring::new(SLOTS as u32)?;
        let buffers = (0..SLOTS)
            .map(|_| Buffer::new())
            .collect::<io::Result<_>>()?;
        Ok(Self {
            ring: Some(ring),
            buffers,
            fd,
            clock: 0,
            pending: 0,
            submitted_bytes: 0,
            delivered_bytes: 0,
        })
    }
    fn collect(&mut self, wait: bool) -> io::Result<()> {
        let ring = self.ring.as_mut().unwrap();
        if wait {
            loop {
                match ring.submit_and_wait(1) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    result => {
                        result?;
                        break;
                    }
                }
            }
        }
        for completion in ring.completion() {
            let slot = completion.user_data() as usize;
            self.buffers[slot].result = completion.result();
            self.pending -= 1;
        }
        Ok(())
    }
    fn queue(&mut self, offset: u64, protected: Option<usize>) -> io::Result<Option<usize>> {
        let offset = offset / BLOCK as u64 * BLOCK as u64;
        self.clock += 1;
        if let Some(slot) = self
            .buffers
            .iter()
            .position(|b| b.result != EMPTY && b.offset == offset)
        {
            self.buffers[slot].age = self.clock;
            return Ok(Some(slot));
        }
        let Some(slot) = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(slot, b)| b.result != PENDING && Some(*slot) != protected)
            .min_by_key(|(_, b)| b.age)
            .map(|(slot, _)| slot)
        else {
            return Ok(None);
        };
        let buffer = &mut self.buffers[slot];
        let entry = opcode::Read::new(types::Fd(self.fd), buffer.bytes.as_ptr(), BLOCK as u32)
            .offset(offset)
            .build()
            .user_data(slot as u64);
        // SAFETY: fd is borrowed until Reader drop. This slot's allocation is
        // never moved/freed/reused while PENDING; CQE identifies its sole read.
        unsafe { self.ring.as_mut().unwrap().submission().push(&entry) }
            .map_err(|_| io::Error::from_raw_os_error(libc::EAGAIN))?;
        buffer.offset = offset;
        buffer.age = self.clock;
        buffer.result = PENDING;
        self.pending += 1;
        self.submitted_bytes += BLOCK as u64;
        Ok(Some(slot))
    }
    fn prefetch(&mut self, offsets: &[i64]) -> io::Result<()> {
        self.collect(false)?;
        for &offset in offsets.iter().take(8).filter(|&&offset| offset >= 0) {
            let _ = self.queue(offset as u64, None)?;
        }
        self.ring.as_mut().unwrap().submit()?;
        Ok(())
    }
    fn read(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        self.collect(false)?;
        let slot = loop {
            if let Some(slot) = self.queue(offset, None)? {
                break slot;
            }
            self.collect(true)?;
        };
        // Sequential read-ahead overlaps packet demux/decode without keeping
        // whole media files. Indexed prefetch adds future, distant key blocks.
        let _ = self.queue((offset / BLOCK as u64 + 1) * BLOCK as u64, Some(slot))?;
        self.ring.as_mut().unwrap().submit()?;
        while self.buffers[slot].result == PENDING {
            self.collect(true)?;
        }
        let buffer = &self.buffers[slot];
        if buffer.result < 0 {
            return Err(io::Error::from_raw_os_error(-buffer.result));
        }
        let within = (offset - buffer.offset) as usize;
        let available = (buffer.result as usize).saturating_sub(within);
        let count = available.min(output.len());
        // SAFETY: completed buffer, within/count bound the initialized bytes.
        if count > 0 {
            unsafe {
                ptr::copy_nonoverlapping(
                    buffer.bytes.as_ptr().add(within),
                    output.as_mut_ptr(),
                    count,
                )
            };
        }
        self.delivered_bytes += count as u64;
        Ok(count)
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        // Drain outstanding reads before releasing allocations or the fd.
        // An unrecoverable ring error closes the ring first, cancelling its work.
        while self.pending > 0 {
            if self.collect(true).is_err() {
                break;
            }
        }
        drop(self.ring.take());
        if std::env::var_os("VIDEO_STRIP_IO_STATS").is_some() {
            eprintln!(
                "uring submitted={} delivered={}",
                self.submitted_bytes, self.delivered_bytes
            );
        }
    }
}

/// Creates a reader borrowing an open descriptor. Null requests synchronous fallback.
#[unsafe(no_mangle)]
pub extern "C" fn vs_io_new(fd: i32) -> *mut Reader {
    match Reader::new(fd) {
        Ok(reader) => Box::into_raw(Box::new(reader)),
        Err(error) => {
            eprintln!("io_uring unavailable; using synchronous reads: {error}");
            ptr::null_mut()
        }
    }
}
/// # Safety
/// Reader must be live and exclusively owned by the calling decoder thread.
/// `offsets` must contain `count` readable i64 values, with count <= 8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vs_io_prefetch(reader: *mut Reader, offsets: *const i64, count: usize) {
    if reader.is_null() || offsets.is_null() || count > 8 {
        return;
    }
    // SAFETY: caller satisfies the documented pointer and exclusive-use contract.
    let reader = unsafe { &mut *reader };
    let offsets = unsafe { std::slice::from_raw_parts(offsets, count) };
    let _ = reader.prefetch(offsets);
}
/// # Safety
/// Reader must be live and exclusive; output must be writable for count bytes
/// and must not alias the reader state or its buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vs_io_read(
    reader: *mut Reader,
    offset: u64,
    output: *mut u8,
    count: usize,
) -> i32 {
    if reader.is_null() || output.is_null() || count > i32::MAX as usize {
        return -libc::EINVAL;
    }
    // SAFETY: caller supplies the documented live reader and output allocation.
    let reader = unsafe { &mut *reader };
    let output = unsafe { std::slice::from_raw_parts_mut(output, count) };
    match reader.read(offset, output) {
        Ok(count) => count as i32,
        Err(error) => -error.raw_os_error().unwrap_or(libc::EIO),
    }
}
/// # Safety
/// Pointer is null or a reader from vs_io_new, destroyed exactly once before fd close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vs_io_free(reader: *mut Reader) {
    if !reader.is_null() {
        drop(unsafe { Box::from_raw(reader) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io::Write,
        os::fd::AsRawFd,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        file: File,
        path: PathBuf,
        bytes: Vec<u8>,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "video-uring-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let bytes = (0..BLOCK * 25 + 113)
                .map(|i| (i * 17 + i / 251) as u8)
                .collect::<Vec<_>>();
            file.write_all(&bytes).unwrap();
            Self { file, path, bytes }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_file(&self.path).unwrap();
        }
    }

    fn verify_reader(file: &File, bytes: &[u8]) {
        let mut reader = Reader::new(file.as_raw_fd()).unwrap();
        let mut output = vec![0; 32768];
        for round in 0..3 {
            for i in 0..32 {
                let offset = (i * 137113 + round * 997) % bytes.len();
                let ahead = (0..8)
                    .map(|j| ((offset + j * BLOCK * 3) % bytes.len()) as i64)
                    .collect::<Vec<_>>();
                reader.prefetch(&ahead).unwrap();
                let count = reader.read(offset as u64, &mut output).unwrap();
                assert!(count > 0);
                assert_eq!(&output[..count], &bytes[offset..offset + count]);
            }
        }
        let offset = bytes.len() - 13;
        assert_eq!(reader.read(offset as u64, &mut output).unwrap(), 13);
        assert_eq!(&output[..13], &bytes[offset..]);
        assert_eq!(reader.read(bytes.len() as u64, &mut output).unwrap(), 0);
        assert_eq!(
            reader
                .read((bytes.len() + BLOCK) as u64, &mut output)
                .unwrap(),
            0
        );
        // Drop with outstanding look-ahead must drain before freeing buffers.
        reader
            .prefetch(&[0, (BLOCK * 9) as i64, (BLOCK * 24) as i64])
            .unwrap();
    }
    #[test]
    fn random_prefetch_eviction_and_unaligned_eof_preserve_bytes() {
        let fixture = Fixture::new();
        verify_reader(&fixture.file, &fixture.bytes);
    }
    #[test]
    fn aligned_direct_reads_preserve_bytes_and_short_final_block() {
        let fixture = Fixture::new();
        // SAFETY: live fd; flags are changed only for this test-owned file.
        let result =
            unsafe { libc::fcntl(fixture.file.as_raw_fd(), libc::F_SETFL, libc::O_DIRECT) };
        assert_eq!(result, 0);
        verify_reader(&fixture.file, &fixture.bytes);
    }
    #[test]
    fn failed_kernel_reads_return_an_error_and_drain_safely() {
        let mut reader = Reader::new(-1).unwrap();
        assert_eq!(
            reader.read(0, &mut [0; 64]).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
    }
}
