//! Bounded ZIP access for document previews; never extracts paths.
use std::io::{Cursor, Read, Seek, SeekFrom};
use zip::ZipArchive;

/// Opens a small, single-disk ZIP after bounding its central directory allocation.
///
/// # Errors
/// Rejects malformed, duplicate-entry, ZIP64, and oversized containers.
pub fn archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, String> {
    if bytes.len() > 64 * 1024 * 1024 || !bytes.starts_with(b"PK\x03\x04") {
        return Err("Invalid or oversized content archive".into());
    }
    let count = directory(
        bytes.len() as u64,
        &bytes[bytes.len().saturating_sub(65_557)..],
    )?;
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() != usize::from(count) {
        return Err("Duplicate content archive entries".into());
    }
    Ok(archive)
}

/// Opens a retained seekable file after bounding its encoded size and ZIP index.
///
/// # Errors
/// Rejects non-regular, oversized, malformed or ZIP64 containers.
pub fn archive_file(
    mut file: std::fs::File,
    max_bytes: u64,
) -> Result<ZipArchive<std::fs::File>, String> {
    if crate::kernel_interface::classify_handle(&file)
        .map_err(|e| e.to_string())?
        .protected()
    {
        return Err("Kernel interfaces are inspection-only".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err("Invalid or oversized content archive".into());
    }
    let mut signature = [0; 4];
    file.read_exact(&mut signature).map_err(|e| e.to_string())?;
    if signature != *b"PK\x03\x04" {
        return Err("Invalid content ZIP signature".into());
    }
    let tail_length = metadata.len().min(65_557);
    file.seek(SeekFrom::End(-(tail_length as i64)))
        .map_err(|e| e.to_string())?;
    let mut tail = vec![0; tail_length as usize];
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let count = directory(metadata.len(), &tail)?;
    let archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() != usize::from(count) {
        return Err("Duplicate content archive entries".into());
    }
    Ok(archive)
}
fn directory(length: u64, tail: &[u8]) -> Result<u16, String> {
    // Bound central-directory allocation before the ZIP reader constructs it.
    // ZIP64 is unnecessary within this reader's encoded and entry budgets.
    let end = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .and_then(|at| tail.get(at..))
        .ok_or("Missing ZIP directory")?;
    let header = end.get(..22).ok_or("Truncated ZIP directory")?;
    let count = u16::from_le_bytes([header[10], header[11]]);
    let directory_length = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let offset = u32::from_le_bytes(header[16..20].try_into().unwrap());
    if count > 8192
        || header[4..8] != [0; 4]
        || header[8..10] != header[10..12]
        || offset as u64 + u64::from(directory_length) != length - end.len() as u64
        || directory_length > 4 * 1024 * 1024
        || end.len() != 22 + usize::from(u16::from_le_bytes([header[20], header[21]]))
    {
        return Err("Content ZIP directory exceeds the import budget".into());
    }
    Ok(count)
}

/// Reads one member with both declared and actual expansion limits.
///
/// # Errors
/// Rejects missing, encrypted, oversized or unreadable members.
pub fn entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut file = archive.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    if !file.is_file() || file.size() > limit as u64 {
        return Err(format!("{name} exceeds the import budget"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Expanded {name} exceeds the import budget"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_and_memory_readers_share_directory_and_member_limits() {
        use std::io::Write;
        let bytes = include_bytes!("../tests/fixtures/ebooks/journey.epub");
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        let mut zip = archive_file(file.try_clone().unwrap(), bytes.len() as u64).unwrap();
        assert_eq!(
            entry(&mut zip, "mimetype", 64).unwrap(),
            b"application/epub+zip"
        );
        assert!(entry(&mut zip, "Books/main.opf", 10).is_err());
        assert!(archive_file(file, bytes.len() as u64 - 1).is_err());
    }
}
