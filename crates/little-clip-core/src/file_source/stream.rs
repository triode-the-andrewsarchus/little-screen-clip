//! Checked bounded streaming shared by file transfers and drive operations.
use super::FileSource;
use std::{
    fs::File,
    os::unix::fs::FileExt,
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
/// admission, metadata, synchronization and atomic installation. An expected digest
/// is checked during streaming, before the caller can publish the staged output.
///
/// # Errors
/// Returns cancellation, changed-source, seek, read or write failures.
pub fn copy_checked(
    input: &mut File,
    output: &mut File,
    cancelled: &AtomicBool,
    delta: bool,
    expected_sha256: Option<&str>,
    mut progress: impl FnMut(u64),
) -> Result<CopyStats, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Copy cancelled".into());
    }
    let source = FileSource::capture_file(input)?;
    let size = input.metadata().map_err(|e| e.to_string())?.len();
    let mut buffer = vec![0; 256 * 1024];
    let mut previous = vec![0; buffer.len()];
    use sha2::{Digest, Sha256};
    let mut digest = expected_sha256.map(|_| Sha256::new());
    let mut stats = CopyStats::default();
    while stats.read < size {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Copy cancelled".into());
        }
        let amount = (size - stats.read).min(buffer.len() as u64) as usize;
        input
            .read_exact_at(&mut buffer[..amount], stats.read)
            .map_err(|e| e.to_string())?;
        if let Some(digest) = &mut digest {
            digest.update(&buffer[..amount]);
        }
        let equal = delta
            && output
                .read_exact_at(&mut previous[..amount], stats.read)
                .is_ok()
            && previous[..amount] == buffer[..amount];
        if !equal {
            output
                .write_all_at(&buffer[..amount], stats.read)
                .map_err(|e| e.to_string())?;
            stats.written += amount as u64;
        }
        stats.read += amount as u64;
        progress(amount as u64);
    }
    output.set_len(size).map_err(|e| e.to_string())?;
    source.check_file(input)?;
    if let (Some(digest), Some(expected)) = (digest, expected_sha256)
        && format!("{:x}", digest.finalize()) != expected
    {
        return Err("Source contents changed after inventory; staging was not published".into());
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
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
            None,
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
                None,
                |_| {}
            )
            .is_err()
        );
    }
}
