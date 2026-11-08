//! Bounded SVG/SVGZ documents and a shared stdin-only rasterization boundary.
use std::{borrow::Cow, io::Read, sync::atomic::AtomicBool, time::Duration};

/// Largest accepted encoded or expanded SVG document, matching image-file opening limits.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;

/// Borrows SVG markup or expands gzip-compressed SVG within the document limit.
///
/// Does not resolve XML entities, stylesheets or referenced files. Full XML
/// validation belongs to the renderer; this function checks the SVG header.
///
/// # Errors
/// Returns an error for oversized input, invalid/truncated gzip or non-SVG data.
///
/// # Examples
/// ```
/// use little_clip_core::svg_image::document;
/// let markup = br#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"/>"#;
/// assert_eq!(document(markup)?.as_ref(), markup);
/// # Ok::<(), String>(())
/// ```
pub fn document(bytes: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("SVG document exceeds 64 MiB".into());
    }
    let source = source(bytes, MAX_DOCUMENT_BYTES, true)?;
    if !is_markup(&source) {
        return Err("Input does not contain SVG markup".into());
    }
    Ok(source)
}
fn source(bytes: &[u8], limit: usize, complete: bool) -> Result<Cow<'_, [u8]>, String> {
    let source = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut expanded = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes)
            .take(limit as u64 + u64::from(complete))
            .read_to_end(&mut expanded)
            .map_err(|error| format!("Invalid compressed SVG: {error}"))?;
        Cow::Owned(expanded)
    } else {
        Cow::Borrowed(&bytes[..bytes.len().min(limit)])
    };
    if complete && source.len() > limit {
        return Err("Expanded SVG exceeds 64 MiB".into());
    }
    Ok(source)
}
/// Returns whether XML begins with an SVG root, without resolving external resources.
pub fn is_markup(bytes: &[u8]) -> bool {
    root(bytes).is_some()
}
fn root(bytes: &[u8]) -> Option<quick_xml::events::BytesStart<'_>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(&bytes[..bytes.len().min(MAX_DOCUMENT_BYTES)]);
    loop {
        match reader.read_event().ok()? {
            Event::Start(element) | Event::Empty(element) => {
                return (element.local_name().as_ref() == b"svg").then_some(element);
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}
/// Reads declared SVG/SVGZ dimensions from at most 64 KiB of expanded XML.
///
/// Physical lengths use 96 pixels per inch. Relative lengths fall back to
/// `viewBox` dimensions. This header probe does not validate the gzip trailer
/// or decode pixels; successful dimensions do not prove a renderable document.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let header = source(bytes, 64 * 1024, false).ok()?;
    let element = root(&header)?;
    let (mut width, mut height, mut view_box) = (None, None, None);
    for field in element.attributes() {
        let field = field.ok()?;
        let value = std::str::from_utf8(&field.value).ok()?;
        match field.key.as_ref() {
            b"width" => width = length(value),
            b"height" => height = length(value),
            b"viewBox" => view_box = viewbox(value),
            _ => {}
        }
    }
    let (width, height) = match (width, height, view_box) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((vw, vh))) => (w, w * vh / vw),
        (None, Some(h), Some((vw, vh))) => (h * vw / vh, h),
        (_, _, Some(size)) => size,
        _ => return None,
    };
    Some((
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
    ))
}
fn length(value: &str) -> Option<f64> {
    let value = value.trim().to_ascii_lowercase();
    let (number, scale) = [
        ("px", 1.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
    ]
    .into_iter()
    .find_map(|(unit, scale)| value.strip_suffix(unit).map(|number| (number, scale)))
    .unwrap_or((&value, 1.0));
    let pixels = number.trim().parse::<f64>().ok()? * scale;
    (pixels.is_finite() && pixels > 0.0).then_some(pixels)
}
fn viewbox(value: &str) -> Option<(f64, f64)> {
    let fields = value
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|field| !field.is_empty())
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [_, _, width, height]: [f64; 4] = fields.try_into().ok()?;
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then_some((width, height))
}

/// Renders SVG or SVGZ as PNG through a bounded `rsvg-convert` child.
///
/// `bounds` requests an aspect-preserving preview; `None` uses intrinsic
/// dimensions where available, or a 1024-pixel square viewport otherwise.
/// All outputs obey the shared raster allocation/dimension limits. Input goes
/// through stdin without a base URI, so external file references stay disabled.
///
/// # Errors
/// Returns an error for invalid documents/bounds, cancellation, missing renderer,
/// timeout, truncated output or a renderer result outside the requested bounds.
pub fn render_png(
    bytes: &[u8],
    bounds: Option<(u32, u32)>,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("SVG rendering cancelled".into());
    }
    let source = document(bytes)?;
    let (width, height) = render_size(&source, bounds)?;
    let budget = width as usize * height as usize * 4 + 1024 * 1024;
    let output = crate::process::CommandRequest::new("rsvg-convert")
        .arg("--format=png")
        .arg("--keep-aspect-ratio")
        .arg("--width")
        .arg(width.to_string())
        .arg("--height")
        .arg(height.to_string())
        .stdin_bytes(source.as_ref())
        .max_output_bytes(budget)
        .timeout(Duration::from_secs(10))
        .run_cancellable(cancelled)
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "SVG renderer failed: {}",
            output.stderr_lossy().chars().take(512).collect::<String>()
        ));
    }
    let size = crate::item::image_reader(&output.stdout, Some("image/png"))
        .and_then(|reader| reader.into_dimensions())
        .map_err(|error| format!("Invalid SVG raster: {error}"))?;
    if size.0 > width || size.1 > height {
        return Err("SVG raster exceeds requested bounds".into());
    }
    Ok(output.stdout)
}
/// Renders SVG/SVGZ into straight-alpha pixels under the shared raster limits.
///
/// # Errors
/// Returns rendering/cancellation errors from [`render_png`] or PNG decode errors.
pub fn render_rgba(
    bytes: &[u8],
    bounds: Option<(u32, u32)>,
    cancelled: &AtomicBool,
) -> Result<image::RgbaImage, String> {
    let png = render_png(bytes, bounds, cancelled)?;
    let mut reader =
        crate::item::image_reader(&png, Some("image/png")).map_err(|error| error.to_string())?;
    reader.limits(crate::item::raster_preview_limits());
    reader
        .decode()
        .map(|image| image.to_rgba8())
        .map_err(|error| error.to_string())
}

fn render_size(source: &[u8], bounds: Option<(u32, u32)>) -> Result<(u32, u32), String> {
    let limits = crate::item::raster_preview_limits();
    let intrinsic = dimensions(source).unwrap_or((1024, 1024));
    let (width, height) = bounds.unwrap_or(intrinsic);
    if width == 0 || height == 0 {
        return Err("SVG raster bounds must be positive".into());
    }
    let width = width.min(limits.max_image_width.unwrap());
    let height = height.min(limits.max_image_height.unwrap());
    let (iw, ih) = (f64::from(intrinsic.0), f64::from(intrinsic.1));
    let pixel_scale = (limits.max_alloc.unwrap() as f64 / 4.0 / (iw * ih)).sqrt();
    let scale = (f64::from(width) / iw)
        .min(f64::from(height) / ih)
        .min(pixel_scale);
    Ok((
        (iw * scale).floor().max(1.0) as u32,
        (ih * scale).floor().max(1.0) as u32,
    ))
}

#[cfg(test)]
#[path = "svg_image_tests.rs"]
mod tests;
