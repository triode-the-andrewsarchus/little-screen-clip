//! Decode a bounded Windows RT_ICON resource without executing the binary.
use little_clip_core::ImageThumbnail;
use object::{
    endian::LittleEndian as LE,
    read::pe::{ResourceDirectory, ResourceDirectoryEntryData},
};

pub(super) fn extract(bytes: &[u8], address: u64) -> Option<ImageThumbnail> {
    let directory = ResourceDirectory::new(bytes);
    let root = directory.root().ok()?;
    let icons = root
        .entries
        .iter()
        .find(|e| e.name_or_id().id() == Some(3))?
        .data(directory)
        .ok()?
        .table()?;
    let mut best = None;
    for entry in icons.entries.iter().take(64) {
        let languages = entry.data(directory).ok()?.table()?;
        for language in languages.entries.iter().take(8) {
            let ResourceDirectoryEntryData::Data(data) = language.data(directory).ok()? else {
                continue;
            };
            let image = resource_image(bytes, address, data);
            if let Some(image) = image {
                if best.as_ref().is_none_or(|old: &ImageThumbnail| {
                    image.width * image.height > old.width * old.height
                }) {
                    best = Some(image);
                }
            }
        }
    }
    best
}
fn decode(bytes: &[u8], mime: &str) -> Option<ImageThumbnail> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(8 * 1024 * 1024);
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    ImageThumbnail::generate_with_limits(bytes, Some(mime), 128, 128, limits)
}

fn resource_image(
    bytes: &[u8],
    address: u64,
    data: &object::pe::ImageResourceDataEntry,
) -> Option<ImageThumbnail> {
    let offset = u64::from(data.offset_to_data.get(LE)).checked_sub(address)? as usize;
    let size = data.size.get(LE) as usize;
    if size > 1024 * 1024 {
        return None;
    }
    let payload = bytes.get(offset..offset.checked_add(size)?)?;
    let image = if payload.starts_with(b"\x89PNG") {
        decode(payload, "image/png")
    } else {
        let width = super::analysis::word(payload, 4)?;
        let height = super::analysis::word(payload, 8)? / 2;
        if width == 0 || height == 0 || width > 256 || height > 256 {
            return None;
        }
        let mut ico = vec![
            0,
            0,
            1,
            0,
            1,
            0,
            width as u8,
            height as u8,
            0,
            0,
            1,
            0,
            32,
            0,
        ];
        ico.extend_from_slice(&(size as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(payload);
        decode(&ico, "image/x-icon")
    };
    image
}
