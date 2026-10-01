use super::*;

pub(super) fn encode_jpeg_with_quality(
    img: &RgbaImage,
    quality: u8,
) -> Result<Vec<u8>, image::ImageError> {
    let mut rgb = image::RgbImage::new(img.width(), img.height());
    for (x, y, pixel) in img.enumerate_pixels() {
        let alpha = pixel[3] as f32 / 255.0;
        let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        rgb.put_pixel(x, y, image::Rgb([r, g, b]));
    }
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

pub(super) fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

pub(super) fn encode_webp(img: &RgbaImage, lossless: bool, quality: u8) -> Result<Vec<u8>, String> {
    let encoder = webp::Encoder::from_rgba(img.as_raw(), img.width(), img.height());
    let memory = if lossless {
        encoder.encode_lossless()
    } else {
        encoder.encode(quality as f32)
    };
    Ok(memory.to_vec())
}

struct WebpProgressContext<'a> {
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
    callback_count: Option<&'a std::sync::atomic::AtomicUsize>,
    cancel_on_callback: bool,
}

struct WebpPictureGuard(libwebp_sys::WebPPicture);

impl Drop for WebpPictureGuard {
    fn drop(&mut self) {
        unsafe { libwebp_sys::WebPPictureFree(&mut self.0) };
    }
}

struct WebpMemoryWriterGuard(libwebp_sys::WebPMemoryWriter);

impl Drop for WebpMemoryWriterGuard {
    fn drop(&mut self) {
        unsafe { libwebp_sys::WebPMemoryWriterClear(&mut self.0) };
    }
}

unsafe extern "C" fn webp_cancel_progress_hook(
    _percent: std::ffi::c_int,
    picture: *const libwebp_sys::WebPPicture,
) -> std::ffi::c_int {
    if picture.is_null() {
        return 0;
    }
    let context = unsafe {
        (*picture)
            .user_data
            .cast::<WebpProgressContext<'_>>()
            .as_ref()
    };
    let Some(context) = context else {
        return 0;
    };
    if let Some(count) = context.callback_count {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    if context.cancel_on_callback {
        if let Some(cancel) = context.cancel {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
    }
    if context
        .cancel
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
    {
        0
    } else {
        1
    }
}

pub(super) fn encode_webp_cancellable(
    img: &RgbaImage,
    lossless: bool,
    quality: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<u8>, String> {
    encode_webp_cancellable_with_progress(img, lossless, quality, cancel, None, false)
}

pub(super) fn encode_webp_cancellable_with_progress(
    img: &RgbaImage,
    lossless: bool,
    quality: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    callback_count: Option<&std::sync::atomic::AtomicUsize>,
    cancel_on_callback: bool,
) -> Result<Vec<u8>, String> {
    use std::sync::atomic::Ordering;
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err("cancelled".into());
    }

    let mut config = libwebp_sys::WebPConfig::new()
        .map_err(|_| "WebP encoder configuration initialization failed".to_string())?;
    config.lossless = i32::from(lossless);
    config.alpha_compression = i32::from(!lossless);
    config.quality = if lossless { 75.0 } else { quality as f32 };
    if unsafe { libwebp_sys::WebPValidateConfig(&config) } == 0 {
        return Err("WebP encoder configuration is invalid".into());
    }

    let mut picture = WebpPictureGuard(
        libwebp_sys::WebPPicture::new()
            .map_err(|_| "WebP picture initialization failed".to_string())?,
    );
    picture.0.use_argb = 1;
    picture.0.width = img.width() as i32;
    picture.0.height = img.height() as i32;
    let context = WebpProgressContext {
        cancel,
        callback_count,
        cancel_on_callback,
    };
    picture.0.progress_hook = Some(webp_cancel_progress_hook);
    picture.0.user_data = (&context as *const WebpProgressContext<'_>)
        .cast_mut()
        .cast();

    let mut writer = std::mem::MaybeUninit::<libwebp_sys::WebPMemoryWriter>::uninit();
    unsafe { libwebp_sys::WebPMemoryWriterInit(writer.as_mut_ptr()) };
    let mut writer = WebpMemoryWriterGuard(unsafe { writer.assume_init() });
    picture.0.writer = Some(libwebp_sys::WebPMemoryWrite);
    picture.0.custom_ptr = (&mut writer.0 as *mut libwebp_sys::WebPMemoryWriter).cast();

    let imported = unsafe {
        libwebp_sys::WebPPictureImportRGBA(
            &mut picture.0,
            img.as_raw().as_ptr(),
            img.width() as i32 * 4,
        )
    };
    let result = if imported == 0 {
        Err("WebP RGBA import failed".into())
    } else if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        Err("cancelled".into())
    } else {
        let status = unsafe { libwebp_sys::WebPEncode(&config, &mut picture.0) };
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            Err("cancelled".into())
        } else if status != 0 {
            if writer.0.mem.is_null() || writer.0.size == 0 {
                Err("WebP encoder produced no output".into())
            } else {
                Ok(unsafe { std::slice::from_raw_parts(writer.0.mem, writer.0.size) }.to_vec())
            }
        } else {
            Err(format!("WebP encoding failed: {:?}", picture.0.error_code))
        }
    };
    // Both RAII guards clean up libwebp allocations on normal return or unwind.
    drop(picture);
    drop(writer);
    result
}

pub(super) fn render_svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let output = CommandRequest::new("rsvg-convert")
        .stdin_bytes(bytes)
        .run()
        .ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        Some(output.stdout)
    } else {
        None
    }
}
