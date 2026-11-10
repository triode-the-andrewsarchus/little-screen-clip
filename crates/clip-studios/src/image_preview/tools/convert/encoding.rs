use super::*;

pub(super) fn encode_conversion(
    base_img: RgbaImage,
    format: ConvertTargetFormat,
    quality: u8,
    lossless: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    decoder: Option<(&std::path::Path, &ImageToolProcessControl)>,
) -> Result<(Vec<u8>, RgbaImage), String> {
    let cancelled = || cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
    if cancelled() {
        return Err("cancelled".to_string());
    }
    let bytes = match format {
        ConvertTargetFormat::Png => encode_png_conversion(&base_img, cancel),
        ConvertTargetFormat::Jpeg => encode_jpeg_conversion(&base_img, quality, cancel),
        ConvertTargetFormat::Webp => encode_webp_cancellable(&base_img, lossless, quality, cancel),
    }?;
    if cancelled() {
        return Err("cancelled".to_string());
    }
    // The RGBA PNG stream is lossless; the other codecs must preserve the
    // image crate's exact output, but their opaque decoders run in a child.
    let decoded = if format == ConvertTargetFormat::Png {
        base_img
    } else if let Some((program, control)) = decoder {
        crate::conversion_decode_worker::decode_in_child(
            &bytes,
            base_img.as_raw().len(),
            program,
            control,
        )?
        .unwrap_or(base_img)
    } else {
        image::load_from_memory(&bytes)
            .map(|image| image.to_rgba8())
            .unwrap_or(base_img)
    };
    if cancelled() {
        return Err("cancelled".to_string());
    }
    Ok((bytes, decoded))
}

pub(super) fn encode_png_conversion(
    img: &RgbaImage,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<u8>, String> {
    let writer = CancellableVecWriter::new(cancel);
    let output = writer.output.clone();
    write_png_stream(img, writer, cancel)?;
    let mut output = output.lock().expect("conversion output lock poisoned");
    Ok(std::mem::take(&mut *output))
}

pub(super) fn write_png_stream<W: std::io::Write>(
    img: &RgbaImage,
    writer: W,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    use std::io::Write;

    if img.width() == 0 || img.height() == 0 {
        return Err("PNG dimensions must be nonzero".to_string());
    }
    let mut encoder = png::Encoder::new(writer, img.width(), img.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_deflate_compression(png::DeflateCompression::FdeflateUltraFast);
    encoder.set_filter(png::Filter::Adaptive);
    let mut png_writer = encoder.write_header().map_err(|error| {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            "cancelled".to_string()
        } else {
            error.to_string()
        }
    })?;
    let mut stream = png_writer
        .stream_writer_with_size(CONVERSION_OUTPUT_CHUNK_SIZE)
        .map_err(|error| {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
                "cancelled".to_string()
            } else {
                error.to_string()
            }
        })?;
    let row_bytes = img.width() as usize * 4;
    for row in img.as_raw().chunks_exact(row_bytes) {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return Err("cancelled".to_string());
        }
        stream.write_all(row).map_err(|error| {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
                "cancelled".to_string()
            } else {
                error.to_string()
            }
        })?;
    }
    stream.finish().map_err(|error| {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            "cancelled".to_string()
        } else {
            error.to_string()
        }
    })?;
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err("cancelled".to_string());
    }
    png_writer.finish().map_err(|error| {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            "cancelled".to_string()
        } else {
            error.to_string()
        }
    })?;
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err("cancelled".to_string());
    }
    Ok(())
}

pub(super) struct CancellableVecWriter<'a> {
    pub(super) bytes: Vec<u8>,
    pub(super) output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    pub(super) cancel: Option<&'a std::sync::atomic::AtomicBool>,
    pub(super) bytes_since_cancel_check: usize,
}

pub(super) const CONVERSION_OUTPUT_CHUNK_SIZE: usize = 64 * 1024;
pub(super) const CONVERSION_CANCEL_CHECK_INTERVAL: usize = 4 * 1024;

impl CancellableVecWriter<'_> {
    pub(super) fn new(cancel: Option<&std::sync::atomic::AtomicBool>) -> CancellableVecWriter<'_> {
        CancellableVecWriter {
            bytes: Vec::with_capacity(CONVERSION_OUTPUT_CHUNK_SIZE),
            output: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel,
            bytes_since_cancel_check: 0,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
    }

    fn flush_chunk(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        self.output
            .lock()
            .expect("conversion output lock poisoned")
            .extend_from_slice(&self.bytes);
        self.bytes.clear();
    }
}

impl std::io::Write for CancellableVecWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes_since_cancel_check == 0 && self.is_cancelled() {
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        let mut written = 0;
        while written < bytes.len() {
            let check_after = CONVERSION_CANCEL_CHECK_INTERVAL - self.bytes_since_cancel_check;
            let count = check_after.min(bytes.len() - written);
            self.bytes
                .extend_from_slice(&bytes[written..written + count]);
            written += count;
            self.bytes_since_cancel_check += count;

            if self.bytes_since_cancel_check == CONVERSION_CANCEL_CHECK_INTERVAL {
                self.bytes_since_cancel_check = 0;
                if self.is_cancelled() {
                    return Ok(written);
                }
            }
            if self.bytes.len() >= CONVERSION_OUTPUT_CHUNK_SIZE {
                self.flush_chunk();
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.is_cancelled() {
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    }
}

impl Drop for CancellableVecWriter<'_> {
    fn drop(&mut self) {
        self.flush_chunk();
    }
}

pub(super) fn encode_jpeg_conversion(
    img: &RgbaImage,
    quality: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<u8>, String> {
    let mut rgb = image::RgbImage::new(img.width(), img.height());
    for (x, y, pixel) in img.enumerate_pixels() {
        if x == 0 && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return Err("cancelled".to_string());
        }
        let alpha = pixel[3] as f32 / 255.0;
        let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        rgb.put_pixel(x, y, image::Rgb([r, g, b]));
    }
    let writer = CancellableVecWriter::new(cancel);
    let bytes = writer.output.clone();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(writer, quality);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|error| error.to_string())?;
    drop(encoder);
    let mut output = bytes.lock().expect("conversion output lock poisoned");
    Ok(std::mem::take(&mut *output))
}
