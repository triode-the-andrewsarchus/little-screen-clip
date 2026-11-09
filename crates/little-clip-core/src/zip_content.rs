//! Bounded in-memory ZIP access for document previews; never extracts paths.
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Opens a small, single-disk ZIP after bounding its central directory allocation.
///
/// # Errors
/// Rejects malformed, duplicate-entry, ZIP64, and oversized containers.
pub fn archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, String> {
    if bytes.len() > 64 * 1024 * 1024 || !bytes.starts_with(b"PK\x03\x04") {
        return Err("Invalid or oversized content archive".into());
    }
    // Bound central-directory allocation before the ZIP reader constructs it.
    // ZIP64 is unnecessary within this reader's encoded and entry budgets.
    let tail = &bytes[bytes.len().saturating_sub(65_557)..];
    let end = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .and_then(|at| tail.get(at..))
        .ok_or("Missing ZIP directory")?;
    let header = end.get(..22).ok_or("Truncated ZIP directory")?;
    let count = u16::from_le_bytes([header[10], header[11]]);
    let length = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let offset = u32::from_le_bytes(header[16..20].try_into().unwrap());
    if count > 8192
        || header[4..8] != [0; 4]
        || header[8..10] != header[10..12]
        || offset as u64 + u64::from(length) != (bytes.len() - end.len()) as u64
        || length > 4 * 1024 * 1024
        || end.len() != 22 + usize::from(u16::from_le_bytes([header[20], header[21]]))
    {
        return Err("Content ZIP directory exceeds the import budget".into());
    }
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() != usize::from(count) {
        return Err("Duplicate content archive entries".into());
    }
    Ok(archive)
}

/// Reads one member with both declared and actual expansion limits.
///
/// # Errors
/// Rejects missing, encrypted, oversized or unreadable members.
pub fn entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
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
