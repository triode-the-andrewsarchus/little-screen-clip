//! Bounded PC Paintbrush decoding and lossless RGB encoding.
use image::{ColorType, ImageDecoder, ImageError, ImageResult};
use std::io::Read;

const MAX_BYTES: u64 = 64 * 1024 * 1024;

struct Decoder {
    bytes: Vec<u8>,
    info: oxideav_pcx::ImageInfo,
    options: oxideav_pcx::DecodeOptions,
}

impl Decoder {
    fn new(reader: impl Read) -> ImageResult<Self> {
        let mut bytes = Vec::new();
        reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES || bytes.first() != Some(&0x0a) {
            return Err(error("Invalid or oversized PCX image"));
        }
        let info = oxideav_pcx::info(&bytes).map_err(error)?;
        Ok(Self {
            bytes,
            info,
            options: oxideav_pcx::DecodeOptions::new().with_max_bytes(512 * 1024 * 1024),
        })
    }
}

impl ImageDecoder for Decoder {
    fn dimensions(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }
    fn color_type(&self) -> ColorType {
        ColorType::Rgb8
    }
    fn set_limits(&mut self, mut limits: image::Limits) -> ImageResult<()> {
        limits.check_support(&image::LimitSupport::default())?;
        limits.check_dimensions(self.info.width, self.info.height)?;
        let pixels = u64::from(self.info.width) * u64::from(self.info.height);
        let planar = u64::from(self.info.bytes_per_line)
            * u64::from(self.info.n_planes)
            * u64::from(self.info.height);
        let native = pixels * self.info.format.bytes_per_pixel() as u64;
        // The reader reserves its RGB output. Account separately for the peak
        // planar/native decode or native/RGB conversion, plus palette storage.
        limits.reserve((planar + native).max(native + pixels * 3) + 2048)?;
        self.options.max_width = limits.max_image_width;
        self.options.max_height = limits.max_image_height;
        self.options.max_bytes = Some(planar.max(native));
        Ok(())
    }
    fn read_image(self, buf: &mut [u8]) -> ImageResult<()> {
        let image = oxideav_pcx::decode_with(&self.bytes, &self.options).map_err(error)?;
        let rgb = image.try_to_rgb8().map_err(error)?;
        if rgb.len() != buf.len() {
            return Err(error("Invalid PCX output buffer"));
        }
        buf.copy_from_slice(&rgb);
        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buf)
    }
}

fn error(message: impl std::fmt::Display) -> ImageError {
    ImageError::Decoding(image::error::DecodingError::new(
        image::error::ImageFormatHint::Name("PCX".into()),
        message.to_string(),
    ))
}

/// Encodes tightly packed RGB pixels as a lossless 24-bit PCX image.
///
/// # Errors
/// Returns an error for invalid dimensions, mismatched pixels or oversized output.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let bytes =
        oxideav_pcx::encode_rgb8(width, height, rgb, &oxideav_pcx::EncodeOptions::default())
            .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Encoded PCX exceeds 64 MiB".into());
    }
    Ok(bytes)
}

pub(crate) fn register_decoding_hook() {
    if image::hooks::register_decoding_hook(
        "pcx".into(),
        Box::new(|reader| Ok(Box::new(Decoder::new(reader)?))),
    ) {
        // PCX has no fixed multi-byte magic. Check the manufacturer, known
        // version and RLE encoding here; the decoder validates the full header.
        for signature in [
            b"\x0a\x00\x01\0",
            b"\x0a\x02\x01\0",
            b"\x0a\x03\x01\0",
            b"\x0a\x04\x01\0",
            b"\x0a\x05\x01\0",
        ] {
            image::hooks::register_format_detection_hook(
                "pcx".into(),
                signature,
                Some(b"\xff\xff\xff\0"),
            );
        }
    }
}
