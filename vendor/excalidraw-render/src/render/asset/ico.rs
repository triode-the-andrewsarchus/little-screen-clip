//! ICO and CUR, read as the pinned reference's Blink `ICOImageDecoder`
//! (Chromium 149) reads them.
//!
//! Directory rules: the six-byte header needs only type 1 (icon) or 2 (cursor)
//! and a non-zero entry count, and the whole directory must be in the file; its
//! reserved word is ignored. Each 16-byte entry gives the width and height
//! (zero meaning 256), a bit count (icons only; zero, or any cursor entry,
//! takes the smallest depth that holds the colour count, a zero colour count
//! meaning 256) and the image offset, which must lie past the directory for
//! every entry. The reserved byte, planes and the byte size are never checked:
//! an entry's image runs from its offset to the end of the file.
//!
//! Selection: the largest area wins, then the larger bit count, then the
//! earlier directory entry. Only the selected entry is decoded, so a fault in
//! any other entry's image is never seen.
//!
//! The selected image is a PNG when its first four bytes are `\x89PNG` and a
//! DIB otherwise. Either must report exactly the directory's size. The DIB is
//! read by [`super::bmp`] in icon mode, including its AND mask.

use super::bmp;

#[derive(Clone, Copy)]
pub(super) struct Entry {
    pub(super) width: u32,
    pub(super) height: u32,
    bit_count: u16,
    pub(super) offset: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Payload {
    Png,
    Dib,
}

/// `ProcessDirectory` and `ProcessDirectoryEntries`, then the first entry of
/// the reference's quality order.
pub(super) fn selected_entry(bytes: &[u8]) -> Result<Entry, ()> {
    let (cursor, count) = directory(bytes)?;
    let directory_end = 6 + count * 16;
    let mut selected: Option<Entry> = None;
    for index in 0..count {
        let entry = entry_at(bytes, cursor, index)?;
        if entry.offset < directory_end {
            return Err(());
        }
        if selected.is_none_or(|selected| rank(entry) > rank(selected)) {
            selected = Some(entry);
        }
    }
    selected.ok_or(())
}

fn rank(entry: Entry) -> (u32, u16) {
    (entry.width * entry.height, entry.bit_count)
}

/// Whether the entry's image is a PNG or a DIB. Fewer than four bytes at its
/// offset leave the reference unable to decode it.
pub(super) fn payload(bytes: &[u8], entry: Entry) -> Result<Payload, ()> {
    let magic = bytes
        .get(entry.offset..entry.offset.checked_add(4).ok_or(())?)
        .ok_or(())?;
    Ok(if magic == b"\x89PNG" {
        Payload::Png
    } else {
        Payload::Dib
    })
}

/// A PNG entry's bytes: from its offset through its `IEND` chunk, or to the
/// end of the file when the chunk walk finds no `IEND`. The reference's PNG
/// decoder reads the file from the entry's offset and stops at the image end,
/// so neither the byte size nor anything after `IEND` is consulted.
pub(super) fn png_bytes(bytes: &[u8], entry: Entry) -> Result<&[u8], ()> {
    let payload = bytes.get(entry.offset..).ok_or(())?;
    let mut offset = 8_usize;
    while let Some(header) = payload.get(offset..offset.saturating_add(8)) {
        let length = usize::try_from(u32::from_be_bytes(header[..4].try_into().map_err(|_| ())?))
            .map_err(|_| ())?;
        let end = offset
            .checked_add(12)
            .and_then(|end| end.checked_add(length))
            .ok_or(())?;
        if &header[4..8] == b"IEND" && end <= payload.len() {
            return Ok(&payload[..end]);
        }
        offset = end;
    }
    Ok(payload)
}

/// The selected DIB's size, which must equal the directory's.
pub(super) fn dib_dimensions(bytes: &[u8], entry: Entry) -> Result<(u32, u32), ()> {
    let info = bmp::parse_icon(bytes, entry.offset)?;
    matching(&info, entry)?;
    Ok((info.width, info.height))
}

/// The selected DIB, strictly validated as [`bmp::validate`] is.
pub(super) fn validate_dib(bytes: &[u8], entry: Entry) -> Result<bmp::Info, ()> {
    let info = bmp::validate_icon(bytes, entry.offset)?;
    matching(&info, entry)?;
    Ok(info)
}

fn matching(info: &bmp::Info, entry: Entry) -> Result<(), ()> {
    if (info.width, info.height) == (entry.width, entry.height) {
        Ok(())
    } else {
        Err(())
    }
}

/// The directory sort, plus the selected DIB's RLE walk.
pub(super) fn validation_work(bytes: &[u8]) -> Result<u64, ()> {
    let count = u64::try_from(directory(bytes)?.1).map_err(|_| ())?;
    let comparisons_per_entry = u64::from(u64::BITS - count.max(1).leading_zeros());
    let sort = count.checked_mul(comparisons_per_entry).ok_or(())?;
    let selected = selected_entry(bytes)?;
    let dib = match payload(bytes, selected)? {
        Payload::Png => 0,
        Payload::Dib => bmp::parse_icon(bytes, selected.offset)?.validation_work()?,
    };
    sort.checked_add(dib).ok_or(())
}

pub(super) fn directory_workspace(bytes: &[u8]) -> Result<u64, ()> {
    u64::try_from(directory(bytes)?.1)
        .map_err(|_| ())?
        .checked_mul(32)
        .ok_or(())
}

/// Whether the file is a cursor, and its entry count.
fn directory(bytes: &[u8]) -> Result<(bool, usize), ()> {
    let file_type = read_u16(bytes, 2)?;
    let count = usize::from(read_u16(bytes, 4)?);
    if !matches!(file_type, 1 | 2) || count == 0 || 6 + count * 16 > bytes.len() {
        return Err(());
    }
    Ok((file_type == 2, count))
}

fn entry_at(bytes: &[u8], cursor: bool, index: usize) -> Result<Entry, ()> {
    let start = 6 + index * 16;
    let side = |at: usize| match bytes[start + at] {
        0 => 256,
        side => u32::from(side),
    };
    let mut bit_count = if cursor {
        0
    } else {
        read_u16(bytes, start + 6)?
    };
    if bit_count == 0 {
        let colors = match bytes[start + 2] {
            0 => 256_u32,
            colors => u32::from(colors),
        };
        bit_count = u16::try_from(u32::BITS - (colors - 1).leading_zeros()).map_err(|_| ())?;
    }
    Ok(Entry {
        width: side(0),
        height: side(1),
        bit_count,
        offset: usize::try_from(read_u32(bytes, start + 12)?).map_err(|_| ())?,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ()> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset.checked_add(2).ok_or(())?)
            .ok_or(())?
            .try_into()
            .map_err(|_| ())?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ()> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset.checked_add(4).ok_or(())?)
            .ok_or(())?
            .try_into()
            .map_err(|_| ())?,
    ))
}

#[cfg(test)]
mod work_tests {
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn directory_sort_work_uses_the_count_width_on_every_target() {
        for (count, expected) in [(1_u16, 1_u64), (2, 4), (3, 6), (4, 12), (255, 2040)] {
            let mut bytes = vec![0; 6 + usize::from(count) * 16 + 4];
            bytes[2..4].copy_from_slice(&1_u16.to_le_bytes());
            bytes[4..6].copy_from_slice(&count.to_le_bytes());
            let image = u32::try_from(bytes.len() - 4).expect("fixture offset fits");
            for index in 0..usize::from(count) {
                let start = 6 + index * 16 + 12;
                bytes[start..start + 4].copy_from_slice(&image.to_le_bytes());
            }
            bytes[usize::try_from(image).expect("fixture offset fits")..]
                .copy_from_slice(b"\x89PNG");
            assert_eq!(super::validation_work(&bytes), Ok(expected));
        }
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn a_missing_bit_count_is_derived_from_the_colour_count() {
        for (colors, expected) in [(0_u8, 8_u16), (1, 0), (2, 1), (3, 2), (16, 4), (17, 5)] {
            let mut bytes = vec![0; 22];
            bytes[2] = 1;
            bytes[4] = 1;
            bytes[6 + 2] = colors;
            assert_eq!(
                super::entry_at(&bytes, false, 0).map(|e| e.bit_count),
                Ok(expected)
            );
        }
    }
}
