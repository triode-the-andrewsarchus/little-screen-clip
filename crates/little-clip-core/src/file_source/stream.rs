//! Checked bounded streaming shared by file transfers and drive operations.
use super::FileSource;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CopyStats {
    pub read: u64,
    pub written: u64,
}

/// Copies a pinned regular-file revision with bounded memory and optional delta writes.
///
/// `output` must be an owned unpublished staging file. Delta mode requires read/write
/// access and preserves equal ranges of a reflink clone. The caller owns destination
/// admission, metadata, synchronization and atomic installation.
///
/// # Errors
/// Returns cancellation, changed-source, seek, read or write failures.
pub fn copy_checked(
    input: &mut File,
    output: &mut File,
    cancelled: &AtomicBool,
    delta: bool,
    mut progress: impl FnMut(u64),
) -> Result<CopyStats, String> {
    let source = FileSource::capture_file(input)?;
    let size = input.metadata().map_err(|e| e.to_string())?.len();
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut buffer = vec![0; 256 * 1024];
    let mut previous = vec![0; buffer.len()];
    let mut stats = CopyStats::default();
    while stats.read < size {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Copy cancelled".into());
        }
        let amount = (size - stats.read).min(buffer.len() as u64) as usize;
        input
            .read_exact(&mut buffer[..amount])
            .map_err(|e| e.to_string())?;
        let equal = delta
            && output.read_exact(&mut previous[..amount]).is_ok()
            && previous[..amount] == buffer[..amount];
        output
            .seek(SeekFrom::Start(stats.read))
            .map_err(|e| e.to_string())?;
        if !equal {
            output
                .write_all(&buffer[..amount])
                .map_err(|e| e.to_string())?;
            stats.written += amount as u64;
        } else {
            output
                .seek(SeekFrom::Current(amount as i64))
                .map_err(|e| e.to_string())?;
        }
        stats.read += amount as u64;
        progress(amount as u64);
    }
    output.set_len(size).map_err(|e| e.to_string())?;
    source.check_file(input)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delta_copy_skips_equal_ranges_and_truncates_old_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        let mut bytes = vec![1; 512 * 1024];
        bytes[300000] = 2;
        std::fs::write(&path, &bytes).unwrap();
        let mut input = crate::file_source::open_regular_file(&path, false).unwrap();
        let mut output = tempfile::tempfile().unwrap();
        output.write_all(&vec![1; 768 * 1024]).unwrap();
        let stats = copy_checked(
            &mut input,
            &mut output,
            &AtomicBool::new(false),
            true,
            |_| {},
        )
        .unwrap();
        assert_eq!(stats.read, 512 * 1024);
        assert_eq!(stats.written, 256 * 1024);
        output.seek(SeekFrom::Start(0)).unwrap();
        let mut actual = Vec::new();
        output.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
    }
    #[test]
    fn cancelled_copy_never_claims_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        std::fs::write(&path, b"content").unwrap();
        let mut input = crate::file_source::open_regular_file(&path, false).unwrap();
        assert!(
            copy_checked(
                &mut input,
                &mut tempfile::tempfile().unwrap(),
                &AtomicBool::new(true),
                false,
                |_| {}
            )
            .is_err()
        );
    }
}
