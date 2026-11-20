use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn encode_jpeg_with_quality(
    img: &RgbaImage,
    quality: u8,
) -> Result<Vec<u8>, image::ImageError> {
    let rgb = opaque_rgb(img);
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    encoder.encode(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(buf.into_inner())
}

fn opaque_rgb(img: &RgbaImage) -> image::RgbImage {
    let mut rgb = image::RgbImage::new(img.width(), img.height());
    for (x, y, pixel) in img.enumerate_pixels() {
        let alpha = pixel[3] as f32 / 255.0;
        let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        rgb.put_pixel(x, y, image::Rgb([r, g, b]));
    }
    rgb
}

pub(super) fn encode_pcx(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let rgb = opaque_rgb(img);
    little_clip_core::pcx_image::encode_rgb(rgb.width(), rgb.height(), rgb.as_raw())
}

pub(super) fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

pub(super) fn encode_webp(img: &RgbaImage, lossless: bool, quality: u8) -> Result<Vec<u8>, String> {
    encode_webp_with_stop(img, lossless, quality, webpx::Unstoppable)
}

struct WebpCancellation<'a>(Option<&'a AtomicBool>);

impl webpx::Stop for WebpCancellation<'_> {
    fn check(&self) -> Result<(), webpx::StopReason> {
        if self.0.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(webpx::StopReason::Cancelled);
        }
        Ok(())
    }
}

pub(super) fn encode_webp_cancellable(
    img: &RgbaImage,
    lossless: bool,
    quality: u8,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, String> {
    encode_webp_with_stop(img, lossless, quality, WebpCancellation(cancel))
}

pub(super) fn encode_webp_with_stop(
    img: &RgbaImage,
    lossless: bool,
    quality: u8,
    stop: impl webpx::Stop,
) -> Result<Vec<u8>, String> {
    let config = if lossless {
        webpx::EncoderConfig::new_lossless()
    } else {
        webpx::EncoderConfig::new().quality(f32::from(quality))
    };
    let encoded = webpx::Encoder::new_rgba(img.as_raw(), img.width(), img.height())
        .config(config)
        .encode(&stop);
    stop.check().map_err(|_| "cancelled")?;
    encoded.map_err(|error| error.to_string())
}
