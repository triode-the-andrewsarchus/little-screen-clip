//! Lossless, bounded terrain storage. New tiles use checksummed Zstandard frames;
//! ordinary HGT files remain readable throughout an interrupted migration.
use super::{hgt_samples, storage::AssetDirectory};
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_BYTES: usize = 3601 * 3601 * 2;
pub const MAX_STORED_BYTES: usize = MAX_BYTES + 65_536;

pub fn compressed_name(name: &OsStr) -> std::ffi::OsString {
    let mut name = name.to_owned();
    name.push(".zst");
    name
}

/// Reads a bounded snapshot and its source identity through an already guarded handle.
///
/// # Errors
/// Returns errors for protected, replaced, oversized or unreadable files.
pub fn read_stored(file: &std::fs::File) -> io::Result<(Vec<u8>, crate::file_source::FileSource)> {
    if file.metadata()?.len() > MAX_STORED_BYTES as u64 {
        return Err(io::Error::other("Oversized terrain input"));
    }
    crate::file_source::read_file_snapshot_with_source(
        std::path::Path::new(&format!("/proc/self/fd/{}", file.as_raw_fd())),
        MAX_STORED_BYTES,
    )
}

/// Decodes one bounded HGT payload, preserving every original sample byte.
///
/// # Errors
/// Returns errors for invalid dimensions, corrupt frames or excessive output/workspace.
pub fn decode(bytes: Vec<u8>, compressed: bool) -> io::Result<Vec<u8>> {
    let result = if compressed {
        let size = zstd::zstd_safe::get_frame_content_size(&bytes)
            .map_err(|_| io::Error::other("Invalid terrain frame header"))?
            .and_then(|size| usize::try_from(size).ok())
            .filter(|size| hgt_samples(*size).is_some())
            .ok_or_else(|| io::Error::other("Terrain frame must declare valid HGT dimensions"))?;
        // Frames written by encode() declare their exact size. Decode straight
        // into that allocation, without a streaming window or growing/copying
        // an output buffer. The destination itself bounds history and output.
        zstd::bulk::Decompressor::new()?.decompress(&bytes, size)?
    } else {
        bytes
    };
    hgt_samples(result.len()).ok_or_else(|| io::Error::other("Invalid HGT dimensions"))?;
    Ok(result)
}

/// Streams one validated HGT payload into a checksummed, size-tagged frame.
///
/// # Errors
/// Returns errors for cancellation, invalid sizes, truncated data or I/O failures.
pub fn encode(
    reader: &mut impl Read,
    writer: impl Write,
    expected: u64,
    cancel: &AtomicBool,
) -> io::Result<()> {
    hgt_samples(usize::try_from(expected).map_err(io::Error::other)?)
        .ok_or_else(|| io::Error::other("Invalid HGT dimensions"))?;
    let mut encoder = zstd::stream::write::Encoder::new(writer, 9)?;
    encoder.include_checksum(true)?;
    encoder.set_pledged_src_size(Some(expected))?;
    let mut remaining = expected;
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("Terrain compression cancelled"));
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        remaining = remaining
            .checked_sub(count as u64)
            .ok_or_else(|| io::Error::other("HGT payload exceeds declared size"))?;
        encoder.write_all(&buffer[..count])?;
    }
    if remaining != 0 {
        return Err(io::Error::other("Truncated HGT payload"));
    }
    encoder.finish()?;
    Ok(())
}

/// Checks legacy file size or the size-tagged frame header without decoding a tile.
pub fn present(directory: &AssetDirectory, name: &OsStr, expected: u64) -> bool {
    if directory
        .open_file(name)
        .and_then(|f| f.metadata())
        .is_ok_and(|m| m.len() == expected)
    {
        return true;
    }
    let Ok(mut file) = directory.open_file(&compressed_name(name)) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.len() <= MAX_STORED_BYTES as u64)
    {
        return false;
    }
    let mut header = [0; 18];
    file.read_exact(&mut header).is_ok()
        && zstd::zstd_safe::get_frame_content_size(&header).is_ok_and(|size| size == Some(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terrain_frames_roundtrip_voids_and_reject_truncation_corruption_and_size_lies() {
        let raw: Vec<_> = [10_i16, i16::MIN, -100, 9000]
            .into_iter()
            .flat_map(i16::to_be_bytes)
            .collect();
        let cancel = AtomicBool::new(false);
        let mut encoded = Vec::new();
        encode(&mut raw.as_slice(), &mut encoded, 8, &cancel).unwrap();
        let decoded = decode(encoded.clone(), true).unwrap();
        assert_eq!(decoded, raw);
        // The zstd convenience encoder now records the size of small inputs.
        let mut unsized_frame = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
        unsized_frame.include_contentsize(false).unwrap();
        unsized_frame.write_all(&raw).unwrap();
        let unsized_frame = unsized_frame.finish().unwrap();
        assert_eq!(zstd::decompressed_size(&unsized_frame), None);
        assert!(decode(unsized_frame, true).is_err());
        for end in [0, 1, encoded.len() - 1] {
            assert!(decode(encoded[..end].to_vec(), true).is_err());
        }
        *encoded.last_mut().unwrap() ^= 1;
        assert!(decode(encoded, true).is_err());
        for size in [6, 18] {
            assert!(encode(&mut raw.as_slice(), Vec::new(), size, &cancel).is_err());
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(encode(&mut raw.as_slice(), Vec::new(), 8, &cancel).is_err());
        // A valid compressed frame cannot expand beyond the terrain memory cap.
        let oversized = zstd::bulk::compress(&vec![0; MAX_BYTES + 2], 1).unwrap();
        assert!(decode(oversized, true).is_err());
    }
    #[test]
    fn compressed_inventory_uses_frame_size_and_legacy_tiles_remain_valid() {
        let root = tempfile::tempdir().unwrap();
        let dir = AssetDirectory::open(root.path()).unwrap();
        let name = OsStr::new("N00E000.hgt");
        let mut encoded = Vec::new();
        encode(&mut &[0; 8][..], &mut encoded, 8, &AtomicBool::new(false)).unwrap();
        dir.write(&compressed_name(name), &encoded).unwrap();
        assert!(present(&dir, name, 8));
        assert!(!present(&dir, name, MAX_BYTES as u64));
        dir.write(&compressed_name(name), b"incomplete").unwrap();
        assert!(!present(&dir, name, 8));
        dir.write(name, &[0; 8]).unwrap();
        assert!(present(&dir, name, 8));
    }
}
