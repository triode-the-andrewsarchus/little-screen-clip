use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, MAX_BRUSH_LABEL_BYTES,
    ParametricShape, bounded_label, mask,
};

const MAX_GIMP_DIM: u32 = 4096;

pub(super) fn parse_gbr_library(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    let (preset, _) = parse_gbr_at(bytes, fallback_name)?;
    Ok(BrushLibrary {
        format: BrushFormat::GimpGbr,
        brushes: vec![preset],
        total_count: 1,
    })
}

pub(super) fn parse_gbr_at(
    bytes: &[u8],
    fallback_name: &str,
) -> Result<(BrushPreset, usize), String> {
    if bytes.len() < 28 {
        return Err("Truncated GIMP .gbr header".into());
    }
    let header_size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let width = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let height = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let bpp = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);

    if header_size < 28 || header_size > bytes.len() {
        return Err("Invalid .gbr header size".into());
    }
    if width == 0 || height == 0 || width > MAX_GIMP_DIM || height > MAX_GIMP_DIM {
        return Err("Invalid .gbr brush dimensions".into());
    }
    if bpp != 1 && bpp != 4 {
        return Err(format!("Unsupported .gbr bytes-per-pixel: {bpp}"));
    }
    if version >= 2 && &bytes[20..24] != b"GIMP" {
        return Err("Invalid .gbr magic".into());
    }

    let spacing_raw = if version >= 2 {
        u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]])
    } else {
        25
    };
    let spacing = (spacing_raw.clamp(1, 500) as f32) / 100.0;

    let name_bytes = &bytes[28..header_size.min(28 + MAX_BRUSH_LABEL_BYTES)];
    let name_end = name_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(name_bytes.len());
    let parsed_name = String::from_utf8_lossy(&name_bytes[..name_end])
        .trim()
        .to_owned();
    let name = if parsed_name.is_empty() {
        fallback_name.to_owned()
    } else {
        parsed_name
    };

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("Overflow")?;
    let payload_len = pixel_count.checked_mul(bpp as usize).ok_or("Overflow")?;
    let end = header_size.checked_add(payload_len).ok_or("Overflow")?;
    if end > bytes.len() {
        return Err("Truncated .gbr pixel payload".into());
    }

    let payload = &bytes[header_size..end];
    let alpha = if bpp == 1 {
        payload.to_vec()
    } else {
        // 4bpp RGBA: extract alpha channel; if opaque, fall back to luminance.
        let (chunks, _) = payload.as_chunks::<4>();
        let has_alpha = chunks.iter().any(|px| px[3] < 250);
        chunks
            .iter()
            .map(|px| {
                if has_alpha {
                    px[3]
                } else {
                    let lum =
                        (u32::from(px[0]) * 77 + u32::from(px[1]) * 150 + u32::from(px[2]) * 29)
                            >> 8;
                    255_u8.saturating_sub(lum as u8)
                }
            })
            .collect()
    };

    let tip = mask::raster(width, height, alpha).ok_or("Invalid .gbr mask")?;
    let detail = format!(
        "{width}×{height} · {}% spacing",
        (spacing * 100.0).round() as u32
    );
    Ok((
        BrushPreset {
            name,
            tip,
            params: BrushStrokeParams {
                size_px: width.max(height) as f32,
                spacing,
                ..Default::default()
            },
            detail,
        },
        end,
    ))
}

pub(super) fn parse_gih(
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
) -> Result<BrushLibrary, String> {
    let first_nl = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("Invalid .gih header")?;
    let second_nl = bytes[first_nl + 1..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|pos| first_nl + 1 + pos)
        .ok_or("Invalid .gih parameter line")?;

    let base_name = String::from_utf8_lossy(&bytes[..first_nl.min(MAX_BRUSH_LABEL_BYTES)])
        .trim()
        .to_owned();
    let header_line = String::from_utf8_lossy(&bytes[first_nl + 1..second_nl]);
    let ncells = header_line
        .split_whitespace()
        .next()
        .and_then(|tok| tok.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 512);

    let mut offset = second_nl + 1;
    let mut brushes = Vec::new();
    for idx in 0..ncells.min(max_brushes) {
        if offset >= bytes.len() {
            break;
        }
        let cell_name = if base_name.is_empty() {
            format!("{fallback_name} #{}", idx + 1)
        } else {
            format!("{base_name} #{}", idx + 1)
        };
        let Ok((preset, consumed)) = parse_gbr_at(&bytes[offset..], &cell_name) else {
            break;
        };
        brushes.push(preset);
        offset += consumed;
    }

    if brushes.is_empty() {
        return Err("No valid cells in GIMP .gih hose".into());
    }
    Ok(BrushLibrary {
        format: BrushFormat::GimpGih,
        brushes,
        total_count: ncells,
    })
}

pub(super) fn parse_vbr(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    Ok(BrushLibrary {
        format: BrushFormat::GimpVbr,
        brushes: vec![parse_vbr_preset(bytes, fallback_name)?],
        total_count: 1,
    })
}

pub(super) fn parse_vbr_preset(bytes: &[u8], fallback_name: &str) -> Result<BrushPreset, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in .vbr brush")?;
    let mut lines = text.lines().map(str::trim);
    if !lines
        .next()
        .is_some_and(|line| line.starts_with("GIMP-VBR"))
    {
        return Err("Invalid .vbr magic header".into());
    }
    let has_shape = match lines.next() {
        Some(version) if version.starts_with("1.0") => false,
        Some(version) if version.starts_with("1.5") => true,
        _ => return Err("Unsupported .vbr version".into()),
    };
    let name = lines
        .next()
        .filter(|name| !name.is_empty())
        .map(bounded_label)
        .unwrap_or(fallback_name)
        .to_owned();
    let shape = if has_shape {
        match lines.next() {
            Some("circle") => ParametricShape::Circle,
            Some("square") => ParametricShape::Square,
            Some("diamond") => ParametricShape::Diamond,
            _ => return Err("Invalid .vbr brush shape".into()),
        }
    } else {
        ParametricShape::Circle
    };
    let spacing = vbr_number(lines.next(), "spacing")?.clamp(1.0, 500.0) / 100.0;
    let radius = vbr_number(lines.next(), "radius")?.clamp(0.5, 2000.0);
    let spikes = if has_shape {
        lines
            .next()
            .and_then(|value| value.parse::<u8>().ok())
            .ok_or("Invalid .vbr spike count")?
            .clamp(2, 20)
    } else {
        2
    };
    let hardness = vbr_number(lines.next(), "hardness")?.clamp(0.0, 1.0);
    let aspect_ratio = vbr_number(lines.next(), "aspect ratio")?.clamp(1.0, 20.0);
    let angle_deg = vbr_number(lines.next(), "angle")?;

    let detail = format!("Ø {:.0}px · hardness {:.2}", radius * 2.0, hardness);
    Ok(BrushPreset {
        name,
        tip: BrushTipKind::Parametric {
            shape,
            hardness,
            aspect_ratio,
            angle_deg,
            spikes,
        },
        params: BrushStrokeParams {
            size_px: radius * 2.0,
            spacing,
            angle_deg,
            hardness,
            ..Default::default()
        },
        detail,
    })
}

fn vbr_number(value: Option<&str>, name: &str) -> Result<f32, String> {
    value
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("Invalid .vbr brush {name}"))
}
