//! Saved Photoshop composites and metadata, without decoding every layer.
use image::{ColorType, ImageDecoder, ImageError, ImageResult};
use std::io::Read;

const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub use crate::image_document::{Layer, Summary};

/// Reads layer and path metadata without loading composite or layer pixels.
///
/// # Errors
/// Returns an error for invalid, oversized or unsupported Photoshop documents.
pub fn inspect(bytes: &[u8]) -> Result<Summary, String> {
    let header = Header::read(bytes).map_err(|e| e.to_string())?;
    let mut doc = tinypsd::load(bytes, &header.options(false)).map_err(|e| e.to_string())?;
    use tinypsd::SectionDivider::{BoundingSectionDivider, ClosedFolder, OpenFolder};
    if doc
        .layers
        .iter()
        .find(|layer| {
            matches!(
                layer.section_divider,
                OpenFolder | ClosedFolder | BoundingSectionDivider
            )
        })
        .is_some_and(|layer| layer.section_divider == BoundingSectionDivider)
    {
        doc.layers.reverse();
    }
    let mut depth: usize = 0;
    let mut layers = Vec::new();
    for layer in doc.layers {
        if layer.section_divider == BoundingSectionDivider {
            depth = depth.saturating_sub(1);
            continue;
        }
        let group = matches!(layer.section_divider, OpenFolder | ClosedFolder);
        layers.push(Layer {
            blend_mode: layer.blend_mode_str(),
            name: layer.name,
            bounds: Some([
                layer.bounds.top,
                layer.bounds.left,
                layer.bounds.bottom,
                layer.bounds.right,
            ]),
            offset: None,
            kind: None,
            opacity: Some(layer.opacity),
            visible: Some(layer.visible),
            depth,
            group,
            mask: layer.mask.is_some(),
            vector_mask: layer.vector_mask.is_some(),
        });
        depth += usize::from(group);
    }
    Ok(Summary {
        dimensions: (header.width, header.height),
        depth: Some(doc.header.depth),
        profile: None,
        color_mode: format!("{:?}", doc.header.color_mode),
        layers,
        paths: doc.paths.len(),
    })
}

/// Borrows the modern Photoshop JPEG thumbnail resource, when present.
///
/// This inspects only section and resource headers and never decodes layer pixels.
pub fn embedded_thumbnail(bytes: &[u8]) -> Option<&[u8]> {
    let mut resources = image_resources(bytes)?;
    while let Some((id, payload, rest)) = resource_block(resources) {
        // Resource 1036 uses RGB JPEG; legacy 1033 uses BGR and is not selected.
        if id == 1036 && payload.get(..4)? == 1u32.to_be_bytes() {
            let length = u32::from_be_bytes(payload.get(20..24)?.try_into().ok()?) as usize;
            let jpeg = payload.get(28..28usize.checked_add(length)?)?;
            return jpeg.starts_with(b"\xff\xd8").then_some(jpeg);
        }
        resources = rest;
    }
    None
}

fn image_resources(bytes: &[u8]) -> Option<&[u8]> {
    if !bytes.starts_with(b"8BPS\0\x01") {
        return None;
    }
    Header::read(bytes).ok()?;
    let color_len = u32::from_be_bytes(bytes.get(26..30)?.try_into().ok()?) as usize;
    let section = 30usize.checked_add(color_len)?;
    let length = u32::from_be_bytes(
        bytes
            .get(section..section.checked_add(4)?)?
            .try_into()
            .ok()?,
    ) as usize;
    let start = section.checked_add(4)?;
    bytes.get(start..start.checked_add(length)?)
}

fn resource_block(bytes: &[u8]) -> Option<(u16, &[u8], &[u8])> {
    let header = bytes.get(..12)?;
    if &header[..4] != b"8BIM" {
        return None;
    }
    let id = u16::from_be_bytes([header[4], header[5]]);
    let name_len = usize::from(header[6]) + 1;
    let size_offset = 6 + name_len.next_multiple_of(2);
    let data_offset = size_offset + 4;
    let size = u32::from_be_bytes(bytes.get(size_offset..data_offset)?.try_into().ok()?) as usize;
    let end = data_offset.checked_add(size)?;
    Some((
        id,
        bytes.get(data_offset..end)?,
        bytes.get(end.checked_add(size % 2)?..)?,
    ))
}

struct Header {
    width: u32,
    height: u32,
    channels: u16,
    depth: u16,
}
impl Header {
    fn read(bytes: &[u8]) -> ImageResult<Self> {
        let h = bytes
            .get(..26)
            .ok_or_else(|| decoding_error("Truncated PSD header"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(decoding_error("PSD encoded data exceeds 64 MiB"));
        }
        if &h[..6] != b"8BPS\0\x01" {
            return Err(decoding_error(
                "PSD version 1 required; PSB is not supported",
            ));
        }
        let channels = u16::from_be_bytes([h[12], h[13]]);
        let height = u32::from_be_bytes(h[14..18].try_into().unwrap());
        let width = u32::from_be_bytes(h[18..22].try_into().unwrap());
        let depth = u16::from_be_bytes([h[22], h[23]]);
        let mode = u16::from_be_bytes([h[24], h[25]]);
        let colors = match mode {
            3 => 3,
            4 => 4,
            _ => 1,
        };
        let supported = match mode {
            0 => depth == 1,
            2 => depth == 8,
            1 | 3 | 4 | 8 => matches!(depth, 8 | 16 | 32),
            _ => false,
        };
        if !(1..=30_000).contains(&width)
            || !(1..=30_000).contains(&height)
            || !(colors..=56).contains(&channels)
            || !supported
        {
            return Err(decoding_error(
                "Unsupported PSD dimensions, channels or color mode",
            ));
        }
        Ok(Self {
            width,
            height,
            channels,
            depth,
        })
    }
    fn options(&self, composite: bool) -> tinypsd::LoadOptions {
        tinypsd::LoadOptions {
            load_layer_pixels: false,
            load_composite: composite,
            max_pixels: self.width as usize * self.height as usize,
            ..Default::default()
        }
    }
}

struct Decoder {
    bytes: Vec<u8>,
    header: Header,
}
impl Decoder {
    fn new(reader: impl Read) -> ImageResult<Self> {
        let mut bytes = Vec::new();
        reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        let header = Header::read(&bytes)?;
        Ok(Self { bytes, header })
    }
}
impl ImageDecoder for Decoder {
    fn dimensions(&self) -> (u32, u32) {
        (self.header.width, self.header.height)
    }
    fn color_type(&self) -> ColorType {
        ColorType::Rgba8
    }
    fn set_limits(&mut self, mut limits: image::Limits) -> ImageResult<()> {
        limits.check_support(&image::LimitSupport::default())?;
        limits.check_dimensions(self.header.width, self.header.height)?;
        // Include all ZIP channel planes, converted color planes and the RGBA
        // composite. Layer pixels and raster masks are deliberately not loaded.
        let h = &self.header;
        let samples = u64::from(h.depth.div_ceil(8));
        let channels = u64::from(h.channels);
        let pixels = u64::from(h.width) * u64::from(h.height);
        let planes = pixels * (channels * samples + channels.min(5) + 4);
        let row_table = u64::from(h.height) * channels * std::mem::size_of::<usize>() as u64;
        let row_scratch = u64::from(h.width) * (samples + 1);
        limits.reserve(planes + row_table + row_scratch)
    }
    fn read_image(self, buf: &mut [u8]) -> ImageResult<()> {
        let doc = tinypsd::load(&self.bytes, &self.header.options(true)).map_err(decoding_error)?;
        if doc.composite.pixels.len() != buf.len() || doc.composite.is_empty() {
            return Err(decoding_error("PSD has no saved composite image"));
        }
        buf.copy_from_slice(&doc.composite.pixels);
        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buf)
    }
}

fn decoding_error(error: impl std::fmt::Display) -> ImageError {
    ImageError::Decoding(image::error::DecodingError::new(
        image::error::ImageFormatHint::Name("Photoshop document".into()),
        error.to_string(),
    ))
}

pub(crate) fn register_decoding_hook() {
    if image::hooks::register_decoding_hook(
        "psd".into(),
        Box::new(|r| Ok(Box::new(Decoder::new(r)?))),
    ) {
        image::hooks::register_format_detection_hook("psd".into(), b"8BPS", None);
    }
}
