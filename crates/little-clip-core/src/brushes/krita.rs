use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape, gimp,
};
use base64::Engine as _;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};

const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_ZIP_ENTRY_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn parse_kpp(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    let preset = parse_kpp_preset(bytes, fallback_name)?;
    Ok(BrushLibrary {
        format: BrushFormat::KritaPreset,
        brushes: vec![preset],
        total_count: 1,
    })
}

pub(super) fn parse_kpp_preset(bytes: &[u8], fallback_name: &str) -> Result<BrushPreset, String> {
    let xml = extract_png_preset_xml(bytes).ok_or("No Krita preset metadata in .kpp")?;
    let doc = roxmltree::Document::parse(&xml)
        .map_err(|error| format!("Invalid Krita preset XML: {error}"))?;
    let root = doc.root_element();
    let name = root
        .attribute("name")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback_name)
        .to_owned();
    let paintop = root.attribute("paintopid").unwrap_or("paintbrush");

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
        && let Ok(def_doc) = roxmltree::Document::parse(def_xml)
    {
        parse_brush_definition(&def_doc, &mut params)
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

fn parse_brush_definition(
    doc: &roxmltree::Document<'_>,
    params: &mut BrushStrokeParams,
) -> Option<BrushTipKind> {
    let brush = doc.root_element();
    if let Some(spacing) = brush
        .attribute("spacing")
        .and_then(|s| s.parse::<f32>().ok())
    {
        params.spacing = spacing.clamp(0.03, 2.0);
    }
    if let Some(angle) = brush.attribute("angle").and_then(|s| s.parse::<f32>().ok()) {
        params.angle_deg = angle.to_degrees();
    }
    let brush_type = brush.attribute("type").unwrap_or("auto_brush");
    match brush_type {
        "gbr_brush" | "png_brush" => {
            let b64 = brush
                .attribute("BrushVersion")
                .and_then(|_| brush.text())
                .or_else(|| brush.text())?;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .ok()?;
            if raw.starts_with(PNG_MAGIC) {
                let img =
                    image::load_from_memory_with_format(&raw, image::ImageFormat::Png).ok()?;
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                if w > 0 && h > 0 {
                    let has_alpha = rgba.pixels().any(|p| p[3] < 250);
                    let alpha = rgba
                        .pixels()
                        .map(|p| {
                            if has_alpha {
                                p[3]
                            } else {
                                ((u32::from(p[0]) * 77
                                    + u32::from(p[1]) * 150
                                    + u32::from(p[2]) * 29)
                                    >> 8) as u8
                            }
                        })
                        .collect();
                    params.size_px = w.max(h) as f32;
                    return Some(BrushTipKind::AlphaMask {
                        width: w,
                        height: h,
                        alpha,
                    });
                }
            } else if let Ok((preset, _)) = gimp::parse_gbr_at(&raw, "Krita") {
                params.size_px = preset.params.size_px;
                return Some(preset.tip);
            }
            None
        }
        _ => {
            // auto_brush: <MaskGenerator radius="..." fade="..." ratio="..." spikes="..." type="circle|rect"/>
            let mask = doc
                .descendants()
                .find(|n| n.tag_name().name() == "MaskGenerator");
            let radius = mask
                .and_then(|m| m.attribute("radius"))
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(12.0)
                .clamp(1.0, 2000.0);
            let fade = mask
                .and_then(|m| m.attribute("fade"))
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);
            let ratio = mask
                .and_then(|m| m.attribute("ratio"))
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(1.0)
                .clamp(0.05, 1.0);
            let spikes = mask
                .and_then(|m| m.attribute("spikes"))
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(2)
                .clamp(2, 20);
            let shape = match mask.and_then(|m| m.attribute("type")) {
                Some("rect") => ParametricShape::Square,
                _ => ParametricShape::Circle,
            };
            let hardness = (1.0 - fade * 0.5).clamp(0.05, 1.0);
            params.size_px = radius * 2.0;
            params.hardness = hardness;
            Some(BrushTipKind::Parametric {
                shape,
                hardness,
                aspect_ratio: (1.0 / ratio).clamp(1.0, 20.0),
                angle_deg: params.angle_deg,
                spikes,
            })
        }
    }
}

fn extract_png_preset_xml(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(PNG_MAGIC) {
        return None;
    }
    let mut pos = 8usize;
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start.checked_add(len)?;
        let next = data_end.checked_add(4)?;
        if next > bytes.len() {
            break;
        }
        let data = &bytes[data_start..data_end];
        if kind == b"tEXt" || kind == b"zTXt" {
            if let Some(nul) = data.iter().position(|&b| b == 0)
                && &data[..nul] == b"preset"
            {
                if kind == b"tEXt" {
                    return String::from_utf8(data[nul + 1..].to_vec()).ok();
                } else if nul + 2 <= data.len() && data[nul + 1] == 0 {
                    let decoder = flate2::read::ZlibDecoder::new(&data[nul + 2..]);
                    let mut out = String::new();
                    if decoder
                        .take(4 * 1024 * 1024)
                        .read_to_string(&mut out)
                        .is_ok()
                    {
                        return Some(out);
                    }
                }
            }
        } else if kind == b"iTXt" {
            if let Some(nul) = data.iter().position(|&b| b == 0)
                && &data[..nul] == b"preset"
            {
                let rest = &data[nul + 1..];
                if rest.len() >= 2 {
                    let compressed = rest[0] == 1;
                    // Skip compression flag, method, language tag, translated keyword
                    let mut idx = 2;
                    for _ in 0..2 {
                        if let Some(next_nul) = rest[idx..].iter().position(|&b| b == 0) {
                            idx += next_nul + 1;
                        }
                    }
                    let payload = &rest[idx..];
                    if compressed {
                        let decoder = flate2::read::ZlibDecoder::new(payload);
                        let mut out = String::new();
                        if decoder
                            .take(4 * 1024 * 1024)
                            .read_to_string(&mut out)
                            .is_ok()
                        {
                            return Some(out);
                        }
                    } else if let Ok(text) = std::str::from_utf8(payload) {
                        return Some(text.to_owned());
                    }
                }
            }
        } else if kind == b"IEND" {
            break;
        }
        pos = next;
    }
    None
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

    for i in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() || entry.size() > MAX_ZIP_ENTRY_BYTES as u64 {
            continue;
        }
        let name = entry.name().to_owned();
        let lower = name.to_ascii_lowercase();
        let is_kpp = lower.ends_with(".kpp");
        let is_gbr = lower.ends_with(".gbr");
        let is_vbr = lower.ends_with(".vbr");
        if !is_kpp && !is_gbr && !is_vbr {
            continue;
        }
        total_count += 1;
        if brushes.len() >= max_brushes {
            continue;
        }
        let mut buf = Vec::with_capacity(entry.size().min(MAX_ZIP_ENTRY_BYTES as u64) as usize);
        if entry
            .by_ref()
            .take(MAX_ZIP_ENTRY_BYTES as u64)
            .read_to_end(&mut buf)
            .is_err()
        {
            continue;
        }
        let stem = std::path::Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Brush");
        if is_kpp {
            if let Ok(preset) = parse_kpp_preset(&buf, stem) {
                brushes.push(preset);
            }
        } else if is_gbr {
            if let Ok((preset, _)) = gimp::parse_gbr_at(&buf, stem) {
                brushes.push(preset);
            }
        } else if is_vbr && let Ok(lib) = gimp::parse_vbr(&buf, stem) {
            brushes.extend(lib.brushes);
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
