//! Selected-page raster decoding and layer composition.
//!
//! RATTA_RLE/ASA interpretation is checked against the retained Apache-2.0
//! supernotelib reference (copyright 2020 jya), not against traced vector ink.
use super::super::check_cancel;
use super::*;
use base64::Engine;
use image::{GrayImage, Rgb, RgbImage};
use std::{io::Cursor, sync::atomic::AtomicBool};

const LAYERS: &[&str] = &["MAINLAYER", "LAYER1", "LAYER2", "LAYER3", "BGLAYER"];
const MAX_BITMAP: usize = 32 * 1024 * 1024;

/// Renders one 1-based page, decoding only its referenced layers.
///
/// Both output dimensions are bounded by `max_dim` (1–4096); requests beyond
/// native bitmap density do not invent additional ink detail.
///
/// # Errors
///
/// Returns container, page, protocol, decompression, image or cancellation
/// errors. Missing/unsupported layer structure is never a blank-success page.
pub fn render(
    input: &mut (impl Read + Seek),
    page: usize,
    max_dim: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    check_cancel(cancel)?;
    let doc = inspect(input)?;
    let sheet = page
        .checked_sub(1)
        .and_then(|index| doc.pages.get(index))
        .ok_or("Supernote page is outside the notebook")?;
    let mut reader = Reader::new(input)?;
    let canvas = if scalar(&sheet.properties, "DATA")?.is_some() {
        let data = reader.block(
            address(required(&sheet.properties, "DATA")?, "bitmap")?,
            MAX_BITMAP,
        )?;
        decode_layer(
            &data,
            required(&sheet.properties, "PROTOCOL")?,
            sheet,
            false,
            false,
            cancel,
        )?
    } else {
        compose(&mut reader, &doc, sheet, cancel)?
    };
    check_cancel(cancel)?;
    let max_dim = max_dim
        .clamp(1, 4096)
        .min(canvas.width().max(canvas.height()));
    let image = image::DynamicImage::ImageRgb8(canvas).thumbnail(max_dim, max_dim);
    let (width, height) = (image.width(), image.height());
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok((png.into_inner(), width, height))
}

fn compose<R: Read + Seek>(
    reader: &mut Reader<'_, R>,
    doc: &Document,
    page: &Page,
    cancel: &AtomicBool,
) -> Result<RgbImage, String> {
    let order = required(&page.properties, "LAYERSEQ")?
        .split(',')
        .collect::<Vec<_>>();
    if order.len() > 5 || order.is_empty() || order.iter().any(|name| !LAYERS.contains(name)) {
        return Err("Unsupported Supernote layer sequence".into());
    }
    let visibility = layer_visibility(&page.properties)?;
    let mut seen = std::collections::HashSet::new();
    let mut canvas = RgbImage::from_pixel(page.width, page.height, Rgb([255; 3]));
    let highres = doc.version.as_str() >= "20230015";
    for name in order.into_iter().rev() {
        check_cancel(cancel)?;
        if !seen.insert(name) {
            return Err("Duplicate Supernote layer in sequence".into());
        }
        if !visibility.get(name).copied().unwrap_or(false) {
            continue;
        }
        let Some(layer) = load_layer(reader, page, name, highres, cancel)? else {
            continue;
        };
        for (target, source) in canvas.pixels_mut().zip(layer.pixels()) {
            if source.0 != [255; 3] {
                *target = *source;
            }
        }
    }
    Ok(canvas)
}

fn load_layer<R: Read + Seek>(
    reader: &mut Reader<'_, R>,
    page: &Page,
    name: &str,
    highres: bool,
    cancel: &AtomicBool,
) -> Result<Option<RgbImage>, String> {
    let Some(pointer) = scalar(&page.properties, name)? else {
        return Ok(None);
    };
    let fields = reader.metadata(address(pointer, "layer")?)?;
    let Some(bitmap) = scalar(&fields, "LAYERBITMAP")? else {
        return Ok(None);
    };
    let data = reader.block(address(bitmap, "bitmap")?, MAX_BITMAP)?;
    if data.is_empty() {
        return Ok(None);
    }
    let background = name == "BGLAYER";
    let style = scalar(&page.properties, "PAGESTYLE")?.unwrap_or_default();
    let protocol = if background && style.starts_with("user_") {
        "PNG"
    } else {
        scalar(&fields, "LAYERPROTOCOL")?.unwrap_or("RATTA_RLE")
    };
    let blank = background && style == "style_white" && data.len() == 0x140e;
    decode_layer(&data, protocol, page, highres, blank, cancel).map(Some)
}

fn layer_visibility(
    fields: &Properties,
) -> Result<std::collections::HashMap<String, bool>, String> {
    let mut result = std::collections::HashMap::new();
    let info = scalar(fields, "LAYERINFO")?
        .filter(|info| *info != "none")
        .ok_or("Supernote layer visibility metadata is unavailable")?;
    let info = info.replace('#', ":");
    let values: serde_json::Value = match serde_json::from_str(&info) {
        Ok(value) => value,
        Err(_) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(info)
                .map_err(|_| "Invalid Supernote layer visibility")?;
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Supernote layer visibility")?
        }
    };
    for value in values
        .as_array()
        .ok_or("Supernote layer visibility is not an array")?
    {
        let name = if value
            .get("isBackgroundLayer")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            "BGLAYER".into()
        } else {
            match value.get("layerId").and_then(serde_json::Value::as_i64) {
                Some(0) => "MAINLAYER".into(),
                Some(id @ 1..=3) => format!("LAYER{id}"),
                _ => continue,
            }
        };
        result.insert(
            name,
            value
                .get("isVisible")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
    }
    result.entry("MAINLAYER".into()).or_insert(true);
    Ok(result)
}

fn decode_layer(
    data: &[u8],
    protocol: &str,
    page: &Page,
    highres: bool,
    blank: bool,
    cancel: &AtomicBool,
) -> Result<RgbImage, String> {
    match protocol {
        "RATTA_RLE" => {
            let pixels = rle(
                data,
                page.width as usize * page.height as usize,
                highres,
                blank,
                cancel,
            )?;
            let gray = GrayImage::from_raw(page.width, page.height, pixels)
                .ok_or("Invalid Supernote bitmap dimensions")?;
            Ok(image::DynamicImage::ImageLuma8(gray).to_rgb8())
        }
        "SN_ASA_COMPRESS" => asa(data, page),
        "PNG" => {
            let mut input =
                image::ImageReader::with_format(Cursor::new(data), image::ImageFormat::Png);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some(128 * 1024 * 1024);
            input.limits(limits);
            let rgba = input
                .decode()
                .map_err(|error| error.to_string())?
                .to_rgba8();
            if rgba.dimensions() != (page.width, page.height) {
                return Err("Supernote template dimensions do not match its page".into());
            }
            let mut result = RgbImage::from_pixel(page.width, page.height, Rgb([255; 3]));
            for (out, pixel) in result.pixels_mut().zip(rgba.pixels()) {
                let alpha = u32::from(pixel[3]);
                for channel in 0..3 {
                    out[channel] = ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127)
                        / 255) as u8;
                }
            }
            Ok(result)
        }
        _ => Err(format!("Unsupported Supernote bitmap protocol: {protocol}")),
    }
}

fn rle(
    data: &[u8],
    expected: usize,
    highres: bool,
    blank: bool,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if !data.len().is_multiple_of(2) || expected > 8 * 1024 * 1024 {
        return Err("Invalid Supernote RLE size".into());
    }
    let mut decoder = Runs {
        output: Vec::with_capacity(expected),
        expected,
        highres,
        blank,
        held: None,
    };
    for (index, pair) in data.as_chunks::<2>().0.iter().enumerate() {
        if index.is_multiple_of(32768) {
            check_cancel(cancel)?;
        }
        decoder.pair(pair[0], pair[1])?;
    }
    decoder.finish()
}

// A pair may complete a held extended run, emit two runs, or hold the next
// extension. Keep that wire state with its bounded output sink.
struct Runs {
    output: Vec<u8>,
    expected: usize,
    highres: bool,
    blank: bool,
    held: Option<(u8, u8)>,
}
impl Runs {
    fn pair(&mut self, code: u8, count: u8) -> Result<(), String> {
        if let Some((previous, count_high)) = self.held.take() {
            let high = (usize::from(count_high & 0x7f) + 1) << 7;
            if previous == code {
                return self.append(code, high + 1 + usize::from(count));
            }
            self.append(previous, high)?;
        }
        match count {
            255 => self.append(code, if self.blank { 0x400 } else { 0x4000 }),
            count if count & 0x80 != 0 => {
                self.held = Some((code, count));
                Ok(())
            }
            count => self.append(code, usize::from(count) + 1),
        }
    }
    fn finish(mut self) -> Result<Vec<u8>, String> {
        if let Some((code, count)) = self.held.take() {
            let gap = self.expected - self.output.len();
            let tail = (0..8)
                .rev()
                .map(|shift| (usize::from(count & 0x7f) + 1) << shift)
                .find(|count| *count <= gap)
                .unwrap_or(0);
            self.append(code, tail)?;
        }
        if self.output.len() != self.expected {
            return Err("Supernote RLE bitmap is truncated".into());
        }
        Ok(self.output)
    }
    fn append(&mut self, code: u8, count: usize) -> Result<(), String> {
        if count > self.expected - self.output.len() {
            return Err("Supernote RLE bitmap exceeds its page".into());
        }
        let color = match code {
            0x61 | 0x66 => 0,
            0x62 => 255,
            0x65 => 254,
            0x9d | 0x9e if self.highres => 157,
            0xc9 | 0xca if self.highres => 201,
            0x63 if self.highres => 48,
            0x64 if self.highres => 80,
            0x63 | 0x67 if !self.highres => 157,
            0x64 | 0x68 if !self.highres => 201,
            code if self.highres => code,
            _ => return Err("Unknown Supernote grayscale code".into()),
        };
        self.output.resize(self.output.len() + count, color);
        Ok(())
    }
}

fn asa(data: &[u8], page: &Page) -> Result<RgbImage, String> {
    const WIDTH: usize = 1404;
    const HEIGHT: usize = 1888;
    if (page.width, page.height) != (1404, 1872) {
        return Err("Unsupported legacy Supernote page dimensions".into());
    }
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take((WIDTH * HEIGHT * 2 + 1) as u64)
        .read_to_end(&mut decoded)
        .map_err(io_error)?;
    if decoded.len() != WIDTH * HEIGHT * 2 {
        return Err("Invalid legacy Supernote bitmap size".into());
    }
    let mut image = RgbImage::new(page.width, page.height);
    for (x, y, target) in image.enumerate_pixels_mut() {
        let index = ((WIDTH - 1 - x as usize) * HEIGHT + y as usize) * 2;
        let value = match u16::from_le_bytes([decoded[index], decoded[index + 1]]) {
            0x0000 => 0,
            0xffff => 254,
            0x2104 => 157,
            0xe1e2 => 201,
            _ => return Err("Unknown legacy Supernote grayscale code".into()),
        };
        *target = Rgb([value; 3]);
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rle_short_extended_special_and_tail_runs_are_checked() {
        let cancel = AtomicBool::new(false);
        assert_eq!(
            rle(&[0x61, 2, 0x65, 0], 4, false, false, &cancel).unwrap(),
            [0, 0, 0, 254]
        );
        assert_eq!(
            rle(&[0x61, 0x80, 0x61, 1], 130, false, false, &cancel).unwrap(),
            vec![0; 130]
        );
        assert_eq!(
            rle(&[0x62, 255], 1024, true, true, &cancel).unwrap(),
            vec![255; 1024]
        );
        assert_eq!(
            rle(&[0x61, 0x80], 64, true, false, &cancel).unwrap(),
            vec![0; 64]
        );
        for data in [&[0x61][..], &[0x61, 255][..], &[0x61, 1][..]] {
            assert!(rle(data, 1, false, false, &cancel).is_err());
        }
    }
    #[test]
    fn native_pixels_match_the_canonical_supernotelib_reference_exports() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/examples/notebooks/supernote");
        for (stem, pages) in [
            ("demo-a5x-20230015-1to10", &[1, 10][..]),
            ("blank-a6x-3.26.40-two-pages", &[2][..]),
            ("layout-n5-20230015-horizontal-1090", &[1][..]),
            ("render-n6-20230015-moonchild-user-bg", &[1][..]),
            ("test-a5x-20220011-old-pen-ids", &[1][..]),
            (
                "stroke-n5-20260016-isolation-tools-colors-widths",
                &[1, 5][..],
            ),
        ] {
            let source = std::fs::read(root.join(format!("{stem}.note"))).unwrap();
            for &page in pages {
                let expected = image::open(root.join(format!("{stem}-page-{page}-reference.png")))
                    .unwrap()
                    .to_rgb8();
                let (png, _, _) = render(
                    &mut Cursor::new(&source),
                    page,
                    expected.width().max(expected.height()),
                    &AtomicBool::new(false),
                )
                .unwrap();
                let actual = image::load_from_memory(&png).unwrap().to_rgb8();
                assert_eq!(actual.dimensions(), expected.dimensions(), "{stem}/{page}");
                let mismatch = actual
                    .pixels()
                    .zip(expected.pixels())
                    .filter(|(a, b)| a != b)
                    .count();
                assert_eq!(
                    mismatch, 0,
                    "{stem}/{page}: native pixels differ from the independent renderer"
                );
            }
        }
    }

    #[test]
    fn rendering_a_page_does_not_decode_unrelated_page_bitmaps() {
        let mut source = include_bytes!(
            "../../../../../docs/examples/notebooks/supernote/demo-a5x-20230015-1to10.note"
        )
        .to_vec();
        let document = inspect(&mut Cursor::new(&source)).unwrap();
        let layer = address(
            required(&document.pages[4].properties, "MAINLAYER").unwrap(),
            "layer",
        )
        .unwrap();
        let mut input = Cursor::new(&source);
        let fields = Reader::new(&mut input).unwrap().metadata(layer).unwrap();
        let bitmap = address(required(&fields, "LAYERBITMAP").unwrap(), "bitmap").unwrap() as usize;
        source[bitmap..bitmap + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let cancel = AtomicBool::new(false);
        assert!(render(&mut Cursor::new(&source), 1, 128, &cancel).is_ok());
        assert!(
            render(&mut Cursor::new(&source), 5, 128, &cancel)
                .unwrap_err()
                .contains("budget")
        );
    }

    #[test]
    fn real_notebook_pages_render_bounded_visible_ink_and_cancel_cleanly() {
        let source = include_bytes!(
            "../../../../../docs/examples/notebooks/supernote/demo-a5x-20230015-1to10.note"
        );
        let (png, width, height) =
            render(&mut Cursor::new(source), 10, 320, &AtomicBool::new(false)).unwrap();
        assert_eq!((width, height), (240, 320));
        let image = image::load_from_memory(&png).unwrap().to_rgb8();
        assert!(
            image
                .pixels()
                .filter(|pixel| pixel.0.iter().all(|value| *value < 100))
                .count()
                > 100
        );
        assert!(render(&mut Cursor::new(source), 0, 320, &AtomicBool::new(false)).is_err());
        assert!(render(&mut Cursor::new(source), 1, 320, &AtomicBool::new(true)).is_err());
    }
}
