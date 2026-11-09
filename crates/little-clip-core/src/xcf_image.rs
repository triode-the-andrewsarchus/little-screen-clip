//! Native XCF canvas rendering through an isolated, installed GIMP 3 process.
use crate::image_document::{Layer, Summary};
use image::{ColorType, ImageDecoder, ImageError, ImageResult};
use std::{
    io::{Read, Write},
    time::Duration,
};
const MAX_BYTES: usize = 64 * 1024 * 1024;
struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(length)
            .ok_or("XCF size overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or("Truncated XCF document")?;
        self.position = end;
        Ok(bytes)
    }
    fn word(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn offset(&mut self, wide: bool) -> Result<usize, String> {
        let offset = if wide {
            u64::from_be_bytes(self.take(8)?.try_into().unwrap())
        } else {
            self.word()? as u64
        };
        usize::try_from(offset).map_err(|_| "XCF offset exceeds address range".into())
    }
    fn properties(&mut self) -> Result<Vec<(u32, &'a [u8])>, String> {
        let mut result = Vec::new();
        for _ in 0..4096 {
            let kind = self.word()?;
            let length = self.word()? as usize;
            if kind == 0 {
                if length != 0 {
                    return Err("Invalid XCF property terminator".into());
                }
                return Ok(result);
            }
            result.push((kind, self.take(length)?));
        }
        Err("XCF exceeds the property budget".into())
    }
}
struct Header {
    width: u32,
    height: u32,
    version: u32,
    precision: Option<u32>,
    mode: u32,
    position: usize,
}
impl Header {
    fn read(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES || !bytes.starts_with(b"gimp xcf ") {
            return Err("Invalid or oversized XCF document".into());
        }
        let version = version(bytes)?;
        let mut cursor = Cursor {
            bytes,
            position: 14,
        };
        let width = cursor.word()?;
        let height = cursor.word()?;
        let mode = cursor.word()?;
        check_size(width, height)?;
        if width == 0 || height == 0 {
            return Err("XCF canvas dimensions must be positive".into());
        }
        let precision = if version >= 4 {
            Some(cursor.word()?)
        } else {
            None
        };
        Ok(Self {
            width,
            height,
            version,
            precision,
            mode,
            position: cursor.position,
        })
    }
}
fn version(bytes: &[u8]) -> Result<u32, String> {
    let version: u32 = match bytes.get(9..14) {
        Some(b"file\0") => 0,
        Some(value)
            if value[0] == b'v' && value[4] == 0 && value[1..4].iter().all(u8::is_ascii_digit) =>
        {
            std::str::from_utf8(&value[1..4])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("Invalid XCF version")?
        }
        _ => return Err("Invalid XCF version header".into()),
    };
    if version > 23 {
        return Err("This XCF version is newer than the metadata reader supports".into());
    }
    Ok(version)
}
fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width > 16384 || height > 16384 || u64::from(width) * u64::from(height) > 32 * 1024 * 1024 {
        return Err("XCF dimensions exceed the raster limits".into());
    }
    Ok(())
}
struct Properties<'a>(Vec<(u32, &'a [u8])>);
impl<'a> Properties<'a> {
    fn get(&self, id: u32) -> Option<&'a [u8]> {
        self.0
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, value)| *value)
    }
    fn word(&self, id: u32) -> Option<u32> {
        Some(u32::from_be_bytes(self.get(id)?.get(..4)?.try_into().ok()?))
    }
    fn opacity(&self) -> Option<u8> {
        self.get(33)
            .and_then(|bytes| bytes.get(..4))
            .map(|bytes| f32::from_be_bytes(bytes.try_into().unwrap()))
            .filter(|f| f.is_finite())
            .map(|f| (f.clamp(0.0, 1.0) * 255.0).round() as u8)
            .or_else(|| self.word(6).map(|v| v.min(255) as u8))
    }
}
/// Reads canvas and source layer metadata without running GIMP or reading pixel tiles.
///
/// # Errors
/// Rejects invalid headers, properties, offsets or exceeded source/layer budgets.
pub fn inspect(bytes: &[u8]) -> Result<Summary, String> {
    let header = Header::read(bytes)?;
    let wide = header.version >= 11;
    let mut cursor = Cursor {
        bytes,
        position: header.position,
    };
    cursor.properties()?;
    let layers = read_layers(bytes, cursor, wide)?;
    let depth = match header.precision {
        None | Some(0 | 100 | 150 | 175) => Some(8),
        Some(200 | 250 | 275 | 500 | 550 | 575) => Some(16),
        Some(300 | 350 | 375 | 600 | 650 | 675) => Some(32),
        Some(700 | 750 | 775) => Some(64),
        _ => None,
    };
    Ok(Summary {
        dimensions: (header.width, header.height),
        depth,
        profile: None,
        color_mode: match header.mode {
            0 => "RGB",
            1 => "Grayscale",
            2 => "Indexed",
            _ => "Unknown",
        }
        .into(),
        layers,
        paths: 0,
    })
}
fn read_layers(bytes: &[u8], mut cursor: Cursor<'_>, wide: bool) -> Result<Vec<Layer>, String> {
    let mut offsets = Vec::new();
    for _ in 0..4097 {
        let offset = cursor.offset(wide)?;
        if offset == 0 {
            break;
        }
        if offset >= bytes.len() || offsets.contains(&offset) {
            return Err("Invalid or repeated XCF layer offset".into());
        }
        offsets.push(offset);
    }
    if offsets.len() > 4096 {
        return Err("XCF exceeds the 4,096-layer limit".into());
    }
    let mut layers = Vec::new();
    let mut pixels = 0u64;
    let mut names = 0usize;
    for offset in offsets {
        let layer = read_layer(bytes, offset, wide)?;
        names += layer.name.len();
        if names > 4 * 1024 * 1024 {
            return Err("XCF layer names exceed the metadata budget".into());
        }
        if let Some(bounds) = layer.bounds {
            pixels += ((bounds[2] as i64 - bounds[0] as i64)
                * (bounds[3] as i64 - bounds[1] as i64)) as u64;
        }
        if pixels > 128 * 1024 * 1024 {
            return Err("XCF stored layers exceed the decode budget".into());
        }
        layers.push(layer);
    }
    Ok(layers)
}
fn read_layer(bytes: &[u8], position: usize, wide: bool) -> Result<Layer, String> {
    let mut cursor = Cursor { bytes, position };
    let width = cursor.word()?;
    let height = cursor.word()?;
    let kind = cursor.word()?;
    let length = cursor.word()? as usize;
    check_size(width, height)?;
    if length > 65536 {
        return Err("XCF layer name exceeds the metadata limit".into());
    }
    let raw = cursor.take(length)?;
    let name = String::from_utf8_lossy(raw.strip_suffix(&[0]).unwrap_or(raw)).into_owned();
    let properties = Properties(cursor.properties()?);
    let x = properties.word(15).unwrap_or(0) as i32;
    let y = properties
        .get(15)
        .and_then(|b| b.get(4..8))
        .map(|b| i32::from_be_bytes(b.try_into().unwrap()))
        .unwrap_or(0);
    let _hierarchy = cursor.offset(wide)?;
    let mask = cursor.offset(wide)? != 0;
    Ok(Layer {
        name,
        bounds: Some([
            y,
            x,
            y.checked_add(height as i32)
                .ok_or("XCF layer extent overflow")?,
            x.checked_add(width as i32)
                .ok_or("XCF layer extent overflow")?,
        ]),
        offset: None,
        kind: Some(
            [
                "RGB layer",
                "RGBA layer",
                "Grayscale layer",
                "Grayscale with alpha",
                "Indexed layer",
                "Indexed with alpha",
            ]
            .get(kind as usize)
            .unwrap_or(&"Unknown layer")
            .to_string(),
        ),
        opacity: properties.opacity(),
        visible: properties.word(8).map(|v| v != 0),
        blend_mode: match properties.word(7) {
            Some(0 | 28) => "normal".into(),
            Some(value) => format!("XCF mode {value}"),
            None => "normal".into(),
        },
        depth: properties
            .get(30)
            .map_or(0, |b| (b.len() / 4).saturating_sub(1)),
        group: properties.get(29).is_some(),
        mask,
        vector_mask: false,
    })
}
/// Renders a canvas or aspect-preserving thumbnail through an isolated native renderer.
/// Requires installed GIMP 3, its Python batch plug-in, bubblewrap and prlimit.
///
/// # Errors
/// Reports invalid documents, unavailable isolation/rendering, cancellation or output limits.
pub fn render_rgba(
    bytes: &[u8],
    bounds: Option<(u32, u32)>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<image::RgbaImage, String> {
    let summary = inspect(bytes)?;
    let (width, height) = summary.dimensions;
    let target = if let Some((bw, bh)) = bounds {
        if bw == 0 || bh == 0 {
            return Err("XCF preview bounds must be positive".into());
        }
        let scale = (bw as f64 / width as f64)
            .min(bh as f64 / height as f64)
            .min(1.0);
        (
            ((width as f64 * scale).floor() as u32).max(1),
            ((height as f64 * scale).floor() as u32).max(1),
        )
    } else {
        summary.dimensions
    };
    let mut staging = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    staging.write_all(bytes).map_err(|e| e.to_string())?;
    let output = crate::process::CommandRequest::new("prlimit")
        .args([
            "--as=3221225472",
            "--cpu=20",
            "--fsize=269484032",
            "--",
            "bwrap",
        ])
        .args([
            "--die-with-parent",
            "--unshare-all",
            "--new-session",
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib",
            "/lib64",
            "--dir",
            "/etc",
            "--ro-bind",
            "/etc/fonts",
            "/etc/fonts",
            "--ro-bind",
            "/etc/ld.so.cache",
            "/etc/ld.so.cache",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--setenv",
            "HOME",
            "/tmp",
            "--setenv",
            "XDG_CONFIG_HOME",
            "/tmp/config",
            "--setenv",
            "XDG_CACHE_HOME",
            "/tmp/cache",
            "--setenv",
            "XDG_DATA_HOME",
            "/tmp/data",
            "--setenv",
            "GEGL_USE_OPENCL",
            "no",
            "--setenv",
            "GEGL_THREADS",
            "2",
            "--setenv",
            "GEGL_TILE_CACHE_SIZE",
            "134217728",
            "--setenv",
            "GEGL_SWAP",
            "RAM",
            "--ro-bind",
        ])
        .arg_content_file(staging.path())
        .map_err(|e| e.to_string())?
        .args([
            "/input.xcf",
            "--setenv",
            "LITTLE_CLIP_XCF_WIDTH",
            &target.0.to_string(),
            "--setenv",
            "LITTLE_CLIP_XCF_HEIGHT",
            &target.1.to_string(),
            "/usr/bin/gimp-console",
            "--new-instance",
            "--no-data",
            "--no-fonts",
            "--quit",
            "--batch-interpreter=python-fu-eval",
            "--batch",
        ])
        .arg(include_str!("../assets/xcf-render.py"))
        .timeout(Duration::from_secs(30))
        .max_output_bytes(target.0 as usize * target.1 as usize * 4 + 1024 * 1024)
        .run_cancellable(cancelled)
        .map_err(|e| format!("XCF rendering requires GIMP 3, bubblewrap and prlimit: {e}"))?;
    let start = output
        .stdout
        .windows(8)
        .position(|w| w == b"\x89PNG\r\n\x1a\n");
    if !output.status.success() || output.stdout_truncated || start.is_none() {
        return Err(format!(
            "XCF renderer failed: {}",
            output.stderr_lossy().chars().take(1024).collect::<String>()
        ));
    }
    let mut reader = image::ImageReader::with_format(
        std::io::Cursor::new(&output.stdout[start.unwrap()..]),
        image::ImageFormat::Png,
    );
    reader.limits(crate::item::raster_preview_limits());
    let image = reader.decode().map_err(|e| e.to_string())?.to_rgba8();
    if image.dimensions() != target {
        return Err("XCF rendered canvas dimensions changed".into());
    }
    Ok(image)
}
struct Decoder {
    bytes: Vec<u8>,
    dimensions: (u32, u32),
}
impl Decoder {
    fn new(reader: impl Read) -> ImageResult<Self> {
        let mut bytes = Vec::new();
        reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        let header = Header::read(&bytes).map_err(error)?;
        Ok(Self {
            bytes,
            dimensions: (header.width, header.height),
        })
    }
}
impl ImageDecoder for Decoder {
    fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }
    fn color_type(&self) -> ColorType {
        ColorType::Rgba8
    }
    fn set_limits(&mut self, mut limits: image::Limits) -> ImageResult<()> {
        limits.check_support(&image::LimitSupport::default())?;
        limits.check_dimensions(self.dimensions.0, self.dimensions.1)?;
        limits.reserve(
            self.bytes.len() as u64
                + u64::from(self.dimensions.0) * u64::from(self.dimensions.1) * 8,
        )?;
        Ok(())
    }
    fn read_image(self, buffer: &mut [u8]) -> ImageResult<()> {
        let image = render_rgba(
            &self.bytes,
            None,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .map_err(error)?;
        if image.as_raw().len() != buffer.len() {
            return Err(error("Invalid XCF output buffer"));
        }
        buffer.copy_from_slice(image.as_raw());
        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buffer: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buffer)
    }
}
fn error(message: impl std::fmt::Display) -> ImageError {
    ImageError::Decoding(image::error::DecodingError::new(
        image::error::ImageFormatHint::Name("XCF".into()),
        message.to_string(),
    ))
}
pub(crate) fn register_decoding_hook() {
    if image::hooks::register_decoding_hook(
        "xcf".into(),
        Box::new(|reader| Ok(Box::new(Decoder::new(reader)?))),
    ) {
        image::hooks::register_format_detection_hook("xcf".into(), b"gimp xcf ", None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_groups_visibility_and_raster_masks_render_without_flattening_the_source() {
        let bytes = include_bytes!("../tests/fixtures/layered.xcf");
        let summary = inspect(bytes).unwrap();
        assert_eq!(summary.dimensions, (96, 64));
        assert_eq!(
            summary
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect::<Vec<_>>(),
            ["Hidden red", "Panel group", "Blue panel", "Paper"]
        );
        assert_eq!(summary.layers[0].visible, Some(false));
        assert!(summary.layers[1].group);
        assert!(summary.layers[2].mask);
        assert_eq!(summary.layers[2].depth, 1);
        let image = render_rgba(bytes, None, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert_eq!(image.dimensions(), (96, 64));
        let paper = image.get_pixel(30, 24).0;
        let blue = image.get_pixel(60, 24).0;
        assert!(
            (paper[0] as i16 - 230).abs() <= 2 && paper[0] == paper[1] && paper[1] == paper[2],
            "{paper:?}"
        );
        assert!(blue[2] > 200 && blue[0] < 40 && blue[1] < 120, "{blue:?}");
        let mut reader = crate::item::image_reader(bytes, None).unwrap();
        reader.limits(crate::item::raster_preview_limits());
        assert_eq!(reader.into_dimensions().unwrap(), (96, 64));
        assert!(crate::item::is_image_path(std::path::Path::new(
            "PROJECT.XCF"
        )));
        assert!(render_rgba(bytes, None, &std::sync::atomic::AtomicBool::new(true)).is_err());
    }
    #[test]
    fn linear_float_sources_use_the_native_composite_and_srgb_display_conversion() {
        let bytes = include_bytes!("../tests/fixtures/layered-float.xcf");
        assert_eq!(inspect(bytes).unwrap().depth, Some(32));
        let mut reader = crate::item::image_reader(bytes, None).unwrap();
        reader.limits(crate::item::raster_preview_limits());
        let image = reader.decode().unwrap().to_rgba8();
        let paper = image.get_pixel(30, 24).0;
        let blue = image.get_pixel(60, 24).0;
        assert!((paper[0] as i16 - 230).abs() <= 2, "{paper:?}");
        assert!(blue[2] > 200 && blue[0] < 40 && blue[1] < 120, "{blue:?}");
        let thumbnail = render_rgba(
            bytes,
            Some((24, 24)),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(thumbnail.dimensions(), (24, 16));
    }
    #[test]
    fn a_document_with_no_visible_layers_has_a_transparent_canvas() {
        let bytes = include_bytes!("../tests/fixtures/hidden.xcf");
        assert!(
            inspect(bytes)
                .unwrap()
                .layers
                .iter()
                .filter(|layer| layer.depth == 0)
                .all(|layer| layer.visible == Some(false))
        );
        let image = render_rgba(bytes, None, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert!(image.pixels().all(|pixel| pixel[3] == 0));
    }
    #[test]
    fn malformed_metadata_and_large_canvas_are_rejected_before_starting_a_renderer() {
        let mut bytes = include_bytes!("../tests/fixtures/layered.xcf").to_vec();
        assert!(inspect(&bytes[..30]).is_err());
        bytes[14..18].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(inspect(&bytes).is_err());
    }
}
