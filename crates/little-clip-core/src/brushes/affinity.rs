use super::{BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, mask};

use std::sync::atomic::{AtomicBool, Ordering};

/// Best-effort extraction for Affinity `.afbrushes` files by scanning embedded PNG tip rasters.
pub(super) fn parse_afbrushes(
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    if !bytes.starts_with(b"\x00\xffKA") {
        return Err("Unrecognized Affinity .afbrushes header".into());
    }
    let mut brushes = Vec::new();
    let mut remaining = bytes;
    while brushes.len() < max_brushes {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let Some((png, rest)) = mask::embedded_png(remaining) else {
            break;
        };
        remaining = rest;
        if let Some((tip, width, height)) = mask::png(png, mask::OpaqueCoverage::Light) {
            let idx = brushes.len() + 1;
            brushes.push(BrushPreset {
                name: format!("{fallback_name} #{idx}"),
                tip,
                params: BrushStrokeParams::default(),
                detail: format!("{width}×{height} tip"),
            });
        }
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
