//! Normalize compressed web fonts once, before metadata and specimen rendering.
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn decode(bytes: Vec<u8>, token: &AtomicBool) -> Result<Vec<u8>, String> {
    let signature = bytes.get(..4).unwrap_or_default();
    if !matches!(signature, b"wOFF" | b"wOF2") {
        return Ok(bytes);
    }
    cancelled(token)?;
    // WOFF2's decoder independently caps its directory-derived expansion and
    // reconstruction at 128 MiB; the header is only an additional admission check.
    let declared_size = word(&bytes, 16)? as usize;
    if declared_size == 0 || declared_size > MAX_FONT_BYTES {
        return Err("Decoded font preview is limited to 32 MiB".into());
    }
    let tables = u16::from_be_bytes(
        bytes
            .get(12..14)
            .ok_or("Truncated web font header")?
            .try_into()
            .map_err(|_| "Invalid web font header")?,
    ) as usize;
    if tables == 0 || tables > 4095 {
        return Err("Invalid web font table count".into());
    }
    let decoded = match signature {
        b"wOFF" => {
            validate_woff_tables(&bytes, tables, declared_size)?;
            wuff::decompress_woff1(&bytes)
        }
        _ => wuff::decompress_woff2(&bytes),
    }
    .map_err(|error| format!("Cannot decode web font: {error}"))?;
    cancelled(token)?;
    if decoded.len() > MAX_FONT_BYTES {
        return Err("Decoded font preview is limited to 32 MiB".into());
    }
    Ok(decoded)
}

// WOFF1's decoder trusts table origLength for allocation. Validate the entire
// reconstructed size, not just totalSfntSize, before allowing decompression.
fn validate_woff_tables(bytes: &[u8], tables: usize, declared_size: usize) -> Result<(), String> {
    let directory = bytes
        .get(44..44 + tables * 20)
        .ok_or("Truncated WOFF table directory")?;
    let mut total = 12 + tables * 16;
    for entry in directory.chunks_exact(20) {
        let compressed = word(entry, 8)? as usize;
        let original = word(entry, 12)? as usize;
        if compressed > original || original > MAX_FONT_BYTES {
            return Err("Invalid WOFF table length".into());
        }
        total = total
            .checked_add((original + 3) & !3)
            .ok_or("Invalid WOFF table size")?;
        if total > MAX_FONT_BYTES {
            return Err("Decoded font preview is limited to 32 MiB".into());
        }
    }
    if total != declared_size {
        return Err("Inconsistent WOFF decoded size".into());
    }
    Ok(())
}
fn word(bytes: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("Truncated web font")?
            .try_into()
            .map_err(|_| "Invalid web font")?,
    ))
}
fn cancelled(token: &AtomicBool) -> Result<(), String> {
    if token.load(Ordering::Relaxed) {
        Err("Preview cancelled".into())
    } else {
        Ok(())
    }
}
