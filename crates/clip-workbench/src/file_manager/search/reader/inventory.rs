//! Large inventories use an owned anonymous file rather than the stdout payload cap.
use super::*;
use std::io::{BufReader, BufWriter, Seek, Write};
use std::os::fd::BorrowedFd;
use std::sync::atomic::Ordering;

const MAX_BYTES: u64 = 128 * 1024 * 1024;

pub(super) fn call(request: &Request, cancel: &AtomicBool) -> Result<Reply, String> {
    let mut output = tempfile::tempfile().map_err(|e| e.to_string())?;
    let (command, fd) = CommandRequest::new("/proc/self/exe")
        .inherit_handle(&output)
        .map_err(|e| e.to_string())?;
    let mut request = request.clone();
    let Request::List { output: slot, .. } = &mut request else {
        return Err("Invalid inventory request".into());
    };
    *slot = Some(fd);
    match super::call_with(&request, command, cancel)? {
        Reply::DirectoryWritten => {}
        _ => return Err("Invalid inventory transport reply".into()),
    }
    if cancel.load(Ordering::Acquire) {
        return Err("Search cancelled".into());
    }
    if output.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("Directory inventory exceeds 128 MiB transfer limit".into());
    }
    output.rewind().map_err(|e| e.to_string())?;
    let directory = serde_json::from_reader(BufReader::new(output.take(MAX_BYTES)))
        .map_err(|e| format!("Invalid directory inventory: {e}"))?;
    if cancel.load(Ordering::Acquire) {
        return Err("Search cancelled".into());
    }
    Ok(Reply::Directory(directory))
}

pub(super) fn reply(directory: Directory, output: Option<i32>) -> Result<Reply, String> {
    let Some(fd) = output else {
        return Ok(Reply::Directory(directory));
    };
    if fd < 3 {
        return Err("Invalid inventory output descriptor".into());
    }
    // SAFETY: the parent inherits this owned regular-file descriptor. Clone it
    // without taking ownership of the command's original inherited descriptor.
    let owned = unsafe { BorrowedFd::borrow_raw(fd) }
        .try_clone_to_owned()
        .map_err(|e| e.to_string())?;
    let mut file = std::fs::File::from(owned);
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.len() != 0
        || file.stream_position().map_err(|e| e.to_string())? != 0
    {
        return Err("Inventory output must be an empty regular file".into());
    }
    let mut writer = LimitedOutput {
        output: BufWriter::new(file),
        remaining: MAX_BYTES,
    };
    serde_json::to_writer(&mut writer, &directory).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(Reply::DirectoryWritten)
}

struct LimitedOutput<W> {
    output: W,
    remaining: u64,
}
impl<W: Write> Write for LimitedOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other(
                "Directory inventory exceeds 128 MiB transfer limit",
            ));
        }
        let written = self.output.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_transport_never_overwrites_supplied_content_or_standard_io() {
        let fixture = tempfile::tempdir().unwrap();
        let directory =
            filesystem::read_directory(fixture.path(), &AtomicBool::new(false)).unwrap();
        assert!(reply(directory.clone(), Some(1)).is_err());
        let mut output = tempfile::tempfile().unwrap();
        output.write_all(b"retained").unwrap();
        assert!(reply(directory.clone(), Some(output.as_raw_fd())).is_err());
        output.rewind().unwrap();
        let mut original = String::new();
        output.read_to_string(&mut original).unwrap();
        assert_eq!(original, "retained");
        let mut output = tempfile::tempfile().unwrap();
        assert!(matches!(
            reply(directory.clone(), Some(output.as_raw_fd())).unwrap(),
            Reply::DirectoryWritten
        ));
        output.rewind().unwrap();
        let restored: Directory = serde_json::from_reader(output).unwrap();
        assert_eq!(restored.path, directory.path);
        assert_eq!(restored.signature, directory.signature);
    }
    #[test]
    fn inventory_larger_than_stdout_limit_round_trips_without_a_payload_buffer() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("ordinary-file.unknown"), b"").unwrap();
        let mut directory =
            filesystem::read_directory(fixture.path(), &AtomicBool::new(false)).unwrap();
        let sample = directory.entries[0].clone();
        directory.entries = (0..100_000)
            .map(|index| {
                let mut entry = sample.clone();
                entry.name = format!("ordinary-file-{index:06}.unknown");
                entry.path = fixture.path().join(&entry.name);
                entry
            })
            .collect();
        let mut output = tempfile::tempfile().unwrap();
        assert!(matches!(
            reply(directory, Some(output.as_raw_fd())).unwrap(),
            Reply::DirectoryWritten
        ));
        assert!(output.metadata().unwrap().len() > 24 * 1024 * 1024);
        output.rewind().unwrap();
        let restored: Directory = serde_json::from_reader(BufReader::new(output)).unwrap();
        assert_eq!(restored.entries.len(), 100_000);
        assert_eq!(
            restored.entries.last().unwrap().name,
            "ordinary-file-099999.unknown"
        );
        assert_eq!(restored.skipped, 0);
    }

    #[test]
    fn inventory_output_stops_at_its_bound_and_reports_failure() {
        let mut writer = LimitedOutput {
            output: Vec::new(),
            remaining: 3,
        };
        writer.write_all(b"abc").unwrap();
        assert!(writer.write_all(b"d").is_err());
        assert_eq!(writer.output, b"abc");
    }
}
