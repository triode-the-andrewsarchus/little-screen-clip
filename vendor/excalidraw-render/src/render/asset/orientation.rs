//! Shared TIFF orientation reader and RGBA transforms. Each codec owns its
//! container's EXIF discovery and precedence; the reader stops at the first
//! orientation tag, including when that tag is malformed.

use super::{try_copy, ResolveError};

pub(super) fn exif_orientation(exif: &[u8]) -> Option<u8> {
    let little_endian = match exif.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let read_u16 = |bytes: &[u8]| {
        let bytes: [u8; 2] = bytes.try_into().ok()?;
        Some(if little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let read_u32 = |bytes: &[u8]| {
        let bytes: [u8; 4] = bytes.try_into().ok()?;
        Some(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    if read_u16(exif.get(2..4)?)? != 42 {
        return None;
    }
    let ifd_offset = usize::try_from(read_u32(exif.get(4..8)?)?).ok()?;
    let entry_count = usize::from(read_u16(exif.get(ifd_offset..ifd_offset.checked_add(2)?)?)?);
    let entries_start = ifd_offset.checked_add(2)?;
    for index in 0..entry_count {
        let start = entries_start.checked_add(index.checked_mul(12)?)?;
        let entry = exif.get(start..start.checked_add(12)?)?;
        if read_u16(&entry[..2])? != 0x0112 {
            continue;
        }
        if read_u16(&entry[2..4])? != 3 || read_u32(&entry[4..8])? != 1 {
            return None;
        }
        let orientation = read_u16(&entry[8..10])?;
        return u8::try_from(orientation)
            .ok()
            .filter(|value| (1..=8).contains(value));
    }
    None
}

pub(super) fn oriented_dimensions(width: u32, height: u32, orientation: u8) -> (u32, u32) {
    if matches!(orientation, 5..=8) {
        (height, width)
    } else {
        (width, height)
    }
}

pub(super) fn orient_rgba(
    source: &[u8],
    width: u32,
    height: u32,
    orientation: u8,
) -> Result<Vec<u8>, ResolveError> {
    if orientation == 1 {
        return try_copy(source);
    }
    let (output_width, _) = oriented_dimensions(width, height, orientation);
    let mut output = Vec::new();
    output
        .try_reserve_exact(source.len())
        .map_err(|_| ResolveError::Memory)?;
    output.resize(source.len(), 0);
    let width = usize::try_from(width).map_err(|_| ResolveError::Memory)?;
    let height = usize::try_from(height).map_err(|_| ResolveError::Memory)?;
    let output_width = usize::try_from(output_width).map_err(|_| ResolveError::Memory)?;
    for y in 0..height {
        for x in 0..width {
            let (destination_x, destination_y) = match orientation {
                2 => (width - 1 - x, y),
                3 => (width - 1 - x, height - 1 - y),
                4 => (x, height - 1 - y),
                5 => (y, x),
                6 => (height - 1 - y, x),
                7 => (height - 1 - y, width - 1 - x),
                8 => (y, width - 1 - x),
                _ => (x, y),
            };
            let source_start = (y * width + x) * 4;
            let destination_start = (destination_y * output_width + destination_x) * 4;
            output[destination_start..destination_start + 4]
                .copy_from_slice(&source[source_start..source_start + 4]);
        }
    }
    Ok(output)
}
