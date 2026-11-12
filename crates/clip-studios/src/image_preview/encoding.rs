use super::*;

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
        // SAFETY: WebPPicture::new initialized this uniquely owned picture;
        // only libwebp populated its pixel allocations, and this guard frees them once.
        unsafe { libwebp_sys::WebPPictureFree(&mut self.0) };
    }
}

struct WebpMemoryWriterGuard(libwebp_sys::WebPMemoryWriter);

impl Drop for WebpMemoryWriterGuard {
    fn drop(&mut self) {
        // SAFETY: WebPMemoryWriterInit initialized this writer. Its encoded
        // allocation stays owned here through the output copy and is freed once.
        unsafe { libwebp_sys::WebPMemoryWriterClear(&mut self.0) };
    }
}

// Called synchronously by WebPEncode; it never retains the context or unwinds.
// SAFETY: libwebp supplies a live picture whose user_data was set immediately
// before this encode to the stationary WebpProgressContext below.
unsafe extern "C" fn webp_cancel_progress_hook(
    _percent: std::ffi::c_int,
    picture: *const libwebp_sys::WebPPicture,
) -> std::ffi::c_int {
    if picture.is_null() {
        return 0;
    }
    // SAFETY: the callback contract supplies a live picture. user_data is
    // null or the aligned context pointer installed below, live until Encode returns.
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
    if context.cancel_on_callback
        && let Some(cancel) = context.cancel
    {
        cancel.store(true, std::sync::atomic::Ordering::Release);
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
    // SAFETY: the safe constructor initializes the full configuration; validation
    // only reads it and no other thread mutates it during this call.
    if unsafe { libwebp_sys::WebPValidateConfig(&config) } == 0 {
        return Err("WebP encoder configuration is invalid".into());
    }

    let mut picture = WebpPictureGuard(
        libwebp_sys::WebPPicture::new()
            .map_err(|_| "WebP picture initialization failed".to_string())?,
    );
    picture.0.use_argb = 1;
    if [img.width(), img.height()]
        .into_iter()
        .any(|dimension| !(1..=libwebp_sys::WEBP_MAX_DIMENSION).contains(&dimension))
    {
        return Err("WebP dimensions must be between 1 and 16383 pixels".into());
    }
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
    // SAFETY: writable, correctly aligned storage for one native writer. Init
    // initializes every field before the guard or encoder observes them.
    unsafe { libwebp_sys::WebPMemoryWriterInit(writer.as_mut_ptr()) };
    // SAFETY: the immediately preceding initializer completed unconditionally.
    let mut writer = WebpMemoryWriterGuard(unsafe { writer.assume_init() });
    picture.0.writer = Some(libwebp_sys::WebPMemoryWrite);
    picture.0.custom_ptr = (&mut writer.0 as *mut libwebp_sys::WebPMemoryWriter).cast();

    // SAFETY: dimensions are within libwebp's bound, so width*4 fits i32.
    // RgbaImage owns tightly packed width*height*4 bytes, borrowed for this call;
    // libwebp copies them into the guard's picture and never retains img's pointer.
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
        // SAFETY: config, imported picture, writer and callback context remain
        // live and stationary throughout this synchronous call. The callback only
        // uses atomics, cannot panic, and cannot outlive this function's borrows.
        let status = unsafe { libwebp_sys::WebPEncode(&config, &mut picture.0) };
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            Err("cancelled".into())
        } else if status != 0 {
            if writer.0.mem.is_null() || writer.0.size == 0 || writer.0.size > isize::MAX as usize {
                Err("WebP encoder produced no output".into())
            } else {
                // SAFETY: the successful memory writer owns size initialized
                // bytes in one live allocation, checked non-null/nonempty and
                // bounded by isize::MAX. Copy before the writer guard frees it.
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
