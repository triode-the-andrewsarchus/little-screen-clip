use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape,
    bounded_label, gimp, mask,
};
use base64::Engine as _;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_PRESET_XML_BYTES: usize = 8 * 1024 * 1024;
const MAX_ZIP_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BUNDLE_DECODE_BYTES: u64 = 128 * 1024 * 1024;

pub(super) fn parse_kpp(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    let preset = parse_kpp_preset(bytes, fallback_name)?;
    Ok(BrushLibrary {
        format: BrushFormat::KritaPreset,
        brushes: vec![preset],
        total_count: 1,
    })
}

pub(super) fn parse_kpp_preset(bytes: &[u8], fallback_name: &str) -> Result<BrushPreset, String> {
    let xml = extract_png_preset_xml(bytes)?;
    let doc = roxmltree::Document::parse_with_options(&xml, preset_xml_options())
        .map_err(|error| format!("Invalid Krita preset XML: {error}"))?;
    let root = doc.root_element();
    let name = root
        .attribute("name")
        .filter(|s| !s.trim().is_empty())
        .map(bounded_label)
        .unwrap_or(fallback_name)
        .to_owned();
    let paintop = bounded_label(root.attribute("paintopid").unwrap_or("paintbrush"));

    let mut params = BrushStrokeParams::default();
    let mut brush_def_xml = None;
    for node in doc.descendants() {
        if node.tag_name().name() != "param" {
            continue;
        }
        match node.attribute("name") {
            Some("Opacity") => {
                if let Some(val) = node.text().and_then(|t| t.trim().parse::<f32>().ok()) {
                    params.opacity = val.clamp(0.05, 1.0);
                }
            }
            Some("brush_definition") => {
                brush_def_xml = node.text().map(str::to_owned);
            }
            _ => {}
        }
    }

    let tip = if let Some(ref def_xml) = brush_def_xml
        && let Ok(def_doc) = roxmltree::Document::parse_with_options(def_xml, preset_xml_options())
    {
        parse_brush_definition(def_doc.root_element(), root, &mut params)
    } else {
        None
    }
    .unwrap_or(BrushTipKind::Parametric {
        shape: ParametricShape::Circle,
        hardness: params.hardness,
        aspect_ratio: 1.0,
        angle_deg: params.angle_deg,
        spikes: 2,
    });

    let detail = format!("{paintop} · Ø {:.0}px", params.size_px);
    Ok(BrushPreset {
        name,
        tip,
        params,
        detail,
    })
}

fn preset_xml_options() -> roxmltree::ParsingOptions {
    roxmltree::ParsingOptions {
        nodes_limit: 100_000,
        ..Default::default()
    }
}

fn parse_brush_definition(
    brush: roxmltree::Node<'_, '_>,
    preset: roxmltree::Node<'_, '_>,
    params: &mut BrushStrokeParams,
) -> Option<BrushTipKind> {
    if let Some(spacing) = attribute_number(brush, "spacing") {
        params.spacing = spacing.clamp(0.03, 2.0);
    }
    if let Some(angle) = attribute_number(brush, "angle") {
        params.angle_deg = angle.to_degrees();
    }
    match brush.attribute("type").unwrap_or("auto_brush") {
        "auto_brush" => generated_tip(brush, params),
        "gbr_brush" | "png_brush" => predefined_tip(brush, preset, params),
        _ => None,
    }
}

fn generated_tip(
    brush: roxmltree::Node<'_, '_>,
    params: &mut BrushStrokeParams,
) -> Option<BrushTipKind> {
    let mask = brush
        .children()
        .find(|node| node.has_tag_name("MaskGenerator"))?;
    // Krita's legacy "radius" field also stores a diameter.
    params.size_px = attribute_number(mask, "diameter")
        .or_else(|| attribute_number(mask, "radius"))
        .unwrap_or(24.0)
        .clamp(1.0, 4000.0);
    let ratio = attribute_number(mask, "ratio")
        .unwrap_or(1.0)
        .clamp(0.05, 1.0);
    let horizontal = attribute_number(mask, "hfade");
    let vertical = attribute_number(mask, "vfade");
    // The preview uses one hardness value for both axes.
    let hardness = match (horizontal, vertical) {
        (Some(horizontal), Some(vertical)) => (horizontal + vertical) * 0.5,
        (Some(fade), None) | (None, Some(fade)) => fade,
        (None, None) => 1.0 - attribute_number(mask, "fade").unwrap_or(0.5) * 0.5,
    }
    .clamp(0.02, 1.0);
    let shape = match mask.attribute("type") {
        Some("rct" | "rect") => ParametricShape::Square,
        _ => ParametricShape::Circle,
    };
    let spikes = mask
        .attribute("spikes")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(2);
    params.hardness = hardness;
    Some(BrushTipKind::Parametric {
        shape,
        hardness,
        aspect_ratio: ratio.recip(),
        angle_deg: params.angle_deg,
        spikes: spikes.clamp(2, 20),
    })
}

fn attribute_number(node: roxmltree::Node<'_, '_>, name: &str) -> Option<f32> {
    node.attribute(name)?
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

fn predefined_tip(
    brush: roxmltree::Node<'_, '_>,
    preset: roxmltree::Node<'_, '_>,
    params: &mut BrushStrokeParams,
) -> Option<BrushTipKind> {
    let encoded = match brush.attribute("filename") {
        Some(filename) => find_embedded_brush(preset, filename, brush.attribute("md5sum"))?,
        None => brush.text()?,
    };
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let (tip, size) = if raw.starts_with(mask::PNG_MAGIC) {
        let (tip, width, height) = mask::png(&raw, mask::OpaqueCoverage::Light)?;
        (tip, width.max(height) as f32)
    } else {
        let (preset, _) = gimp::parse_gbr_at(&raw, "Krita").ok()?;
        (preset.tip, preset.params.size_px)
    };
    params.size_px = (size * attribute_number(brush, "scale").unwrap_or(1.0)).clamp(1.0, 4000.0);
    Some(tip)
}

fn find_embedded_brush<'a>(
    preset: roxmltree::Node<'a, '_>,
    filename: &str,
    hash: Option<&str>,
) -> Option<&'a str> {
    let resources = preset
        .children()
        .find(|node| node.has_tag_name("resources"))?;
    let candidates = resources
        .children()
        .filter(|node| node.has_tag_name("resource") && node.attribute("type") == Some("brushes"));
    let hash = hash.filter(|hash| !hash.is_empty());
    if let Some(hash) = hash
        && let Some(resource) = candidates
            .clone()
            .find(|node| node.attribute("md5sum") == Some(hash))
    {
        return resource.text();
    }
    candidates
        .filter(|node| node.attribute("filename") == Some(filename))
        .find_map(|node| node.text())
}

fn extract_png_preset_xml(bytes: &[u8]) -> Result<String, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: 2 * MAX_PRESET_XML_BYTES,
    });
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    if let Some(xml) = preset_text(reader.info()) {
        return xml;
    }
    reader.finish().map_err(|error| error.to_string())?;
    preset_text(reader.info()).ok_or_else(|| "No Krita preset metadata in .kpp".to_owned())?
}

fn preset_text(info: &png::Info<'_>) -> Option<Result<String, String>> {
    if let Some(text) = info
        .uncompressed_latin1_text
        .iter()
        .find(|text| text.keyword == "preset")
    {
        return Some(bounded_xml(&text.text).map(str::to_owned));
    }
    if let Some(text) = info
        .compressed_latin1_text
        .iter()
        .find(|text| text.keyword == "preset")
    {
        let mut text = text.clone();
        return Some(
            text.decompress_text_with_limit(MAX_PRESET_XML_BYTES)
                .and_then(|()| text.get_text())
                .map_err(|error| error.to_string())
                .and_then(|xml| {
                    bounded_xml(&xml)?;
                    Ok(xml)
                }),
        );
    }
    info.utf8_text
        .iter()
        .find(|text| text.keyword == "preset")
        .map(|text| {
            let mut text = text.clone();
            text.decompress_text_with_limit(MAX_PRESET_XML_BYTES)
                .and_then(|()| text.get_text())
                .map_err(|error| error.to_string())
                .and_then(|xml| {
                    bounded_xml(&xml)?;
                    Ok(xml)
                })
        })
}

fn bounded_xml(xml: &str) -> Result<&str, String> {
    (xml.len() <= MAX_PRESET_XML_BYTES)
        .then_some(xml)
        .ok_or_else(|| "Krita preset metadata exceeds its size limit".to_owned())
}

pub(super) fn parse_bundle(
    bytes: &[u8],
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Invalid Krita .bundle archive: {error}"))?;
    let mut brushes = Vec::new();
    let mut total_count = 0usize;
    let mut remaining_decode_bytes = MAX_BUNDLE_DECODE_BYTES;

    for i in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let Ok(entry) = archive.by_index_raw(i) else {
            continue;
        };
        let (name, size) = (entry.name().to_owned(), entry.size());
        if entry.is_dir() || size > MAX_ZIP_ENTRY_BYTES {
            continue;
        }
        drop(entry);
        let path = std::path::Path::new(&name);
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "kpp" | "gbr" | "vbr") {
            continue;
        }
        total_count += 1;
        if brushes.len() >= max_brushes || size > remaining_decode_bytes {
            continue;
        }
        remaining_decode_bytes -= size;
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let mut buf = Vec::with_capacity(size as usize);
        if entry.take(size + 1).read_to_end(&mut buf).is_ok()
            && buf.len() as u64 == size
            && let Ok(preset) = bundle_preset(&buf, path, &extension)
        {
            brushes.push(preset);
        }
    }

    if brushes.is_empty() {
        return Err("No readable brushes in Krita .bundle".into());
    }
    let total_count = total_count.max(brushes.len());
    Ok(BrushLibrary {
        format: BrushFormat::KritaBundle,
        brushes,
        total_count,
    })
}

fn bundle_preset(
    bytes: &[u8],
    path: &std::path::Path,
    extension: &str,
) -> Result<BrushPreset, String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(bounded_label)
        .unwrap_or("Brush");
    match extension {
        "kpp" => parse_kpp_preset(bytes, stem),
        "gbr" => gimp::parse_gbr_at(bytes, stem).map(|(preset, _)| preset),
        "vbr" => gimp::parse_vbr_preset(bytes, stem),
        _ => Err("Unsupported Krita bundle member".into()),
    }
}
