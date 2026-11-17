use super::{BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind};

const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PNG_IEND: &[u8; 8] = b"IEND\xaeB`\x82";

/// Best-effort extraction for Affinity `.afbrushes` files by scanning embedded PNG tip rasters.
pub(super) fn parse_afbrushes(
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
) -> Result<BrushLibrary, String> {
    if !bytes.starts_with(b"\x00\xffKA") {
        return Err("Unrecognized Affinity .afbrushes header".into());
    }
    let mut brushes = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() && brushes.len() < max_brushes {
        let Some(rel_start) = bytes[cursor..]
            .windows(PNG_MAGIC.len())
            .position(|w| w == PNG_MAGIC)
        else {
            break;
        };
        let start = cursor + rel_start;
        let Some(rel_end) = bytes[start..]
            .windows(PNG_IEND.len())
            .position(|w| w == PNG_IEND)
        else {
            break;
        };
        let end = start + rel_end + PNG_IEND.len();
        if let Ok(img) =
            image::load_from_memory_with_format(&bytes[start..end], image::ImageFormat::Png)
        {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            if w > 0 && h > 0 && w <= 2048 && h <= 2048 {
                let has_alpha = rgba.pixels().any(|p| p[3] < 250);
                let alpha: Vec<u8> = rgba
                    .pixels()
                    .map(|p| {
                        if has_alpha {
                            p[3]
                        } else {
                            ((u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29)
                                >> 8) as u8
                        }
                    })
                    .collect();
                let idx = brushes.len() + 1;
                brushes.push(BrushPreset {
                    name: format!("{fallback_name} #{idx}"),
                    tip: BrushTipKind::AlphaMask {
                        width: w,
                        height: h,
                        alpha,
                    },
                    params: BrushStrokeParams::default(),
                    detail: format!("{w}×{h} tip"),
                });
            }
        }
        cursor = end;
    }

    if brushes.is_empty() {
        return Err("No embedded tip rasters found in .afbrushes".into());
    }
    let total_count = brushes.len();
    Ok(BrushLibrary {
        format: BrushFormat::AffinityBrushes,
        brushes,
        total_count,
    })
}
