use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, DabCurve,
    ParametricShape, bounded_label, gimp, mask,
};
use base64::Engine as _;
use std::collections::HashMap;
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
    kpp_with_resources(bytes, fallback_name, &mut |_| None)
}

fn kpp_with_resources(
    bytes: &[u8],
    fallback_name: &str,
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
) -> Result<BrushPreset, String> {
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

    let param = |name: &str| {
        root.children()
            .find(|node| node.has_tag_name("param") && node.attribute("name") == Some(name))
            .and_then(|node| node.text())
            .map(str::trim)
    };
    let mut params = paintbrush_params(&param);

    let tip = if let Some(def_xml) = param("brush_definition")
        && let Ok(def_doc) = roxmltree::Document::parse_with_options(def_xml, preset_xml_options())
    {
        parse_brush_definition(def_doc.root_element(), root, &mut params, load_resource)
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

fn paintbrush_params<'a>(param: &impl Fn(&str) -> Option<&'a str>) -> BrushStrokeParams {
    let number = |name, default| {
        param(name)
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(default)
            .clamp(0.0, 1.0)
    };
    let mut params = BrushStrokeParams {
        opacity: number("OpacityValue", number("Opacity", 1.0)),
        flow: number("FlowValue", 1.0),
        ..Default::default()
    };
    if param("PressureSize") == Some("true") {
        let strength = number("SizeValue", 1.0);
        params.dynamics.size = sensor_curve(param, "Size", "pressure", |v| v * strength);
    }
    params.dynamics.opacity = sensor_curve(param, "Opacity", "pressure", |v| v);
    params.dynamics.flow = sensor_curve(param, "Flow", "pressure", |v| v);
    if param("PressureRotation") == Some("true") {
        let strength = number("RotationValue", 1.0) * 180.0;
        params.dynamics.random_angle =
            sensor_curve(param, "Rotation", "fuzzy", |v| (2.0 * v - 1.0) * strength);
        params.dynamics.angle = sensor_curve(param, "Rotation", "pressure", |v| {
            (1.0 - 2.0 * v) * strength
        });
    }
    params
}

fn sensor_curve<'a>(
    param: &impl Fn(&str) -> Option<&'a str>,
    property: &str,
    sensor_id: &str,
    scale: impl Fn(f32) -> f32,
) -> Option<DabCurve> {
    if param(&format!("{property}UseCurve")) != Some("true") {
        return None;
    }
    let xml = param(&format!("{property}Sensor"))?;
    let xml = xml
        .strip_prefix("<!DOCTYPE params>")
        .unwrap_or(xml)
        .trim_start();
    let doc = roxmltree::Document::parse_with_options(xml, preset_xml_options()).ok()?;
    let sensor = doc.root_element();
    if !sensor.has_tag_name("params") || sensor.attribute("id") != Some(sensor_id) {
        return None;
    }
    let common = (param(&format!("{property}UseSameCurve")) == Some("true"))
        .then(|| param(&format!("{property}commonCurve")))
        .flatten();
    let curve = common.or_else(|| {
        sensor
            .children()
            .find(|node| node.has_tag_name("curve"))
            .and_then(|node| node.text())
    });
    let Some(curve) = curve else {
        return Some(DabCurve::linear(scale(0.0), scale(1.0)));
    };
    let points = curve
        .trim()
        .trim_end_matches(';')
        .split(';')
        .take(65)
        .map(|point| {
            let (x, y) = point.split_once(',')?;
            let x = x.trim().parse().ok()?;
            let y: f32 = y.trim().parse().ok()?;
            (0.0..=1.0).contains(&y).then(|| (x, scale(y)))
        })
        .collect::<Option<Vec<_>>>()?;
    DabCurve::from_points(points.into_iter())
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
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
) -> Option<BrushTipKind> {
    if let Some(spacing) = attribute_number(brush, "spacing") {
        params.spacing = spacing.clamp(0.03, 2.0);
    }
    if let Some(angle) = attribute_number(brush, "angle") {
        params.angle_deg = angle.to_degrees();
    }
    match brush.attribute("type").unwrap_or("auto_brush") {
        "auto_brush" => generated_tip(brush, params),
        "gbr_brush" | "png_brush" => predefined_tip(brush, preset, params, load_resource),
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
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
) -> Option<BrushTipKind> {
    let filename = brush.attribute("filename");
    let encoded = match filename {
        Some(filename) => find_embedded_brush(preset, filename, brush.attribute("md5sum")),
        None => brush.text(),
    };
    let resource = encoded
        .and_then(|encoded| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .ok()?;
            resource_tip(&bytes, filename)
        })
        .or_else(|| load_resource(filename?))?;
    params.size_px =
        (resource.size * attribute_number(brush, "scale").unwrap_or(1.0)).clamp(1.0, 4000.0);
    params.dynamics.tip_sequence = resource.sequence;
    Some(resource.tip)
}

#[derive(Clone)]
struct ResourceTip {
    tip: BrushTipKind,
    size: f32,
    sequence: Option<gimp::TipSequence>,
}

fn resource_tip(bytes: &[u8], filename: Option<&str>) -> Option<ResourceTip> {
    if bytes.starts_with(mask::PNG_MAGIC) {
        let (tip, width, height) = mask::png(bytes, mask::OpaqueCoverage::Light)?;
        return Some(ResourceTip {
            tip,
            size: width.max(height) as f32,
            sequence: None,
        });
    }
    let preset = if filename.is_some_and(|name| name.ends_with(".gih")) {
        gimp::parse_gih(bytes, "Krita", 64)
            .ok()?
            .brushes
            .into_iter()
            .next()?
    } else {
        gimp::parse_gbr_at(bytes, "Krita").ok()?.0
    };
    Some(ResourceTip {
        tip: preset.tip,
        size: preset.params.size_px,
        sequence: preset.params.dynamics.tip_sequence,
    })
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
    let mut resources = HashMap::new();

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
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let Some(buf) = read_bundle_entry(entry, size, &mut remaining_decode_bytes) else {
            continue;
        };
        let mut load_resource = |name: &str| {
            resources
                .entry(name.to_owned())
                .or_insert_with(|| bundle_resource(&mut archive, name, &mut remaining_decode_bytes))
                .clone()
        };
        if let Ok(preset) = bundle_preset(&buf, path, &extension, &mut load_resource) {
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
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
) -> Result<BrushPreset, String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(bounded_label)
        .unwrap_or("Brush");
    match extension {
        "kpp" => kpp_with_resources(bytes, stem, load_resource),
        "gbr" => gimp::parse_gbr_at(bytes, stem).map(|(preset, _)| preset),
        "vbr" => gimp::parse_vbr_preset(bytes, stem),
        _ => Err("Unsupported Krita bundle member".into()),
    }
}

fn bundle_resource(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    remaining: &mut u64,
) -> Option<ResourceTip> {
    if std::path::Path::new(name).file_name()?.to_str()? != name || name.contains('\\') {
        return None;
    }
    let entry = archive.by_name(&format!("brushes/{name}")).ok()?;
    let size = entry.size();
    let buf = read_bundle_entry(entry, size, remaining)?;
    resource_tip(&buf, Some(name))
}

fn read_bundle_entry(entry: impl Read, size: u64, remaining: &mut u64) -> Option<Vec<u8>> {
    if size > MAX_ZIP_ENTRY_BYTES || size > *remaining {
        return None;
    }
    *remaining -= size;
    let mut buf = Vec::with_capacity(size as usize);
    entry.take(size + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 == size).then_some(buf)
}
