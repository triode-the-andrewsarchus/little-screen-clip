//! Bounded nonblocking pipe transfers; never wait for a desktop peer on the UI thread.
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub(super) const LIMIT: usize = 1024 * 1024;

fn prepare(fd: &OwnedFd) -> Result<(), String> {
    // SAFETY: fd is owned and remains valid throughout both calls.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn retry_io<T>(
    deadline: Instant,
    cancelled: &AtomicBool,
    mut operation: impl FnMut() -> std::io::Result<T>,
) -> Result<T, String> {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Desktop drag cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("Desktop file transfer timed out".into());
        }
        match operation() {
            Ok(result) => return Ok(result),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

pub(super) fn read(
    fd: OwnedFd,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
) -> Result<Vec<std::path::PathBuf>, String> {
    prepare(&fd)?;
    let mut pipe = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = retry_io(deadline, &cancelled, || pipe.read(&mut buffer))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if bytes.len() > LIMIT {
            return Err("Desktop file references exceed 1 MiB".into());
        }
    }
    let payload = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    super::super::transfers::clipboard::parse(&payload, false)
        .map(|refs| refs.paths)
        .map_err(|e| e.to_string())
}

pub(super) fn write(
    fd: OwnedFd,
    bytes: Arc<Vec<u8>>,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    written: Arc<AtomicBool>,
) -> Result<(), String> {
    prepare(&fd)?;
    let mut pipe = std::fs::File::from(fd);
    let mut offset = 0;
    while offset < bytes.len() {
        let n = retry_io(deadline, &cancelled, || pipe.write(&bytes[offset..]))?;
        if n == 0 {
            return Err("Desktop receiver closed its transfer".into());
        }
        offset += n;
    }
    written.store(true, Ordering::Release);
    Ok(())
}

pub(super) fn snapshot(
    roots: &std::collections::BTreeMap<std::path::PathBuf, Option<super::Identity>>,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<std::collections::BTreeMap<std::path::PathBuf, super::Identity>, String> {
    let mut pending: Vec<_> = roots.keys().cloned().collect();
    let mut entries = std::collections::BTreeMap::new();
    let mut directories = std::collections::BTreeSet::new();
    while let Some(path) = pending.pop() {
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err("Desktop source scan cancelled or timed out".into());
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let identity = super::super::filesystem::identity(&metadata);
        if roots
            .get(&path)
            .is_some_and(|expected| *expected != Some(identity))
        {
            return Err("Source changed before desktop transfer".into());
        }
        entries.insert(path.clone(), identity);
        if metadata.is_dir() {
            use std::os::unix::fs::MetadataExt;
            if !directories.insert((metadata.dev(), metadata.ino())) {
                return Err("Directory cycle in desktop selection".into());
            }
            for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                pending.push(entry.map_err(|e| e.to_string())?.path());
            }
        }
        if entries.len() > 100_000 {
            return Err(
                "Desktop selection exceeds 100,000 entries; use the file operation queue".into(),
            );
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    #[test]
    fn bounded_pipe_transfer_round_trips_uri_payload() {
        let (reader, writer) = UnixStream::pair().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let written = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(b"file:///tmp/space%20name\r\n".to_vec());
        let worker = {
            let cancelled = Arc::clone(&cancelled);
            let written = Arc::clone(&written);
            std::thread::spawn(move || {
                write(
                    writer.into(),
                    bytes,
                    Instant::now() + Duration::from_secs(2),
                    cancelled,
                    written,
                )
            })
        };
        let paths = read(
            reader.into(),
            Instant::now() + Duration::from_secs(2),
            cancelled,
        )
        .unwrap();
        worker.join().unwrap().unwrap();
        assert!(written.load(Ordering::Acquire));
        assert_eq!(paths, [std::path::PathBuf::from("/tmp/space name")]);
    }
    #[test]
    fn stalled_or_cancelled_peer_never_blocks_indefinitely() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        assert!(
            read(
                reader.into(),
                Instant::now(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        let (reader, _writer) = UnixStream::pair().unwrap();
        assert!(
            read(
                reader.into(),
                Instant::now() + Duration::from_secs(10),
                Arc::new(AtomicBool::new(true))
            )
            .is_err()
        );
    }
}
