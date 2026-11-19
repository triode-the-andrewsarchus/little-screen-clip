use super::{BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, mask};

use std::sync::atomic::{AtomicBool, Ordering};

/// Best-effort extraction for Affinity `.afbrushes` files by scanning embedded PNG tip rasters.
pub(super) fn parse_afbrushes(
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    let mut brushes = Vec::new();
    for png in embedded_tips(bytes, cancelled)? {
        if brushes.len() >= max_brushes {
            break;
        }
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

    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
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

pub(super) fn count(bytes: &[u8], cancelled: &AtomicBool) -> Result<usize, String> {
    let count = embedded_tips(bytes, cancelled)?
        .filter(|png| mask::png_reader(png).into_dimensions().is_ok())
        .count();
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    if count == 0 {
        return Err("No embedded tip rasters found in .afbrushes".into());
    }
    Ok(count)
}

fn embedded_tips<'a>(
    mut bytes: &'a [u8],
    cancelled: &'a AtomicBool,
) -> Result<impl Iterator<Item = &'a [u8]>, String> {
    if !bytes.starts_with(b"\x00\xffKA") {
        return Err("Unrecognized Affinity .afbrushes header".into());
    }
    Ok(std::iter::from_fn(move || {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let (png, rest) = mask::embedded_png(bytes)?;
        bytes = rest;
        Some(png)
    }))
}
