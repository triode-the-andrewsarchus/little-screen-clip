use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape,
};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PNG_IEND: &[u8; 8] = b"IEND\xaeB`\x82";

pub(super) fn load_sut(
    path: &Path,
    _max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let source = crate::file_source::FileSource::capture(path)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(|error| format!("Cannot open Clip Studio .sut: {error}"))?;
    let _ = conn.execute_batch("PRAGMA query_only = ON;");
    source.check()?;

    let name: String = conn
        .query_row(
            "SELECT NodeName FROM Node WHERE NodeName IS NOT NULL AND NodeName != '' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("CSP Brush")
                .to_owned()
        });

    let mut params = BrushStrokeParams::default();
    if let Ok(mut stmt) =
        conn.prepare("SELECT BrushSize, BrushHardness, BrushInterval, Opacity FROM Variant LIMIT 1")
        && let Ok(mut rows) = stmt.query([])
        && let Ok(Some(row)) = rows.next()
    {
        if let Ok(size) = row.get::<_, f64>(0) {
            params.size_px = (size as f32).clamp(1.0, 2000.0);
        }
        if let Ok(hardness) = row.get::<_, f64>(1) {
            params.hardness = ((hardness as f32) / 100.0).clamp(0.05, 1.0);
        }
        if let Ok(interval) = row.get::<_, f64>(2) {
            params.spacing = ((interval as f32) / 100.0).clamp(0.04, 2.0);
        }
        if let Ok(opacity) = row.get::<_, f64>(3) {
            params.opacity = ((opacity as f32) / 100.0).clamp(0.05, 1.0);
        }
    }

    let tip = extract_material_tip(&conn).unwrap_or_else(|| BrushTipKind::Parametric {
        shape: ParametricShape::Circle,
        hardness: params.hardness,
        aspect_ratio: 1.0,
        angle_deg: params.angle_deg,
        spikes: 2,
    });
    source.check()?;

    let detail = format!(
        "Ø {:.0}px · hardness {:.0}%",
        params.size_px,
        params.hardness * 100.0
    );
    Ok(BrushLibrary {
        format: BrushFormat::ClipStudioSut,
        brushes: vec![BrushPreset {
            name,
            tip,
            params,
            detail,
        }],
        total_count: 1,
    })
}

fn extract_material_tip(conn: &Connection) -> Option<BrushTipKind> {
    let mut stmt = conn
        .prepare("SELECT FileData FROM MaterialFile WHERE FileData IS NOT NULL LIMIT 8")
        .ok()?;
    let mut rows = stmt.query([]).ok()?;
    while let Ok(Some(row)) = rows.next() {
        let Ok(blob) = row.get::<_, Vec<u8>>(0) else {
            continue;
        };
        let Some(start) = blob.windows(PNG_MAGIC.len()).position(|w| w == PNG_MAGIC) else {
            continue;
        };
        let Some(rel_end) = blob[start..]
            .windows(PNG_IEND.len())
            .position(|w| w == PNG_IEND)
        else {
            continue;
        };
        let end = start + rel_end + PNG_IEND.len();
        if let Ok(img) =
            image::load_from_memory_with_format(&blob[start..end], image::ImageFormat::Png)
        {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            if w > 0 && h > 0 && w <= 2048 && h <= 2048 {
                let has_alpha = rgba.pixels().any(|p| p[3] < 250);
                let alpha = rgba
                    .pixels()
                    .map(|p| {
                        if has_alpha {
                            p[3]
                        } else {
                            let lum = (u32::from(p[0]) * 77
                                + u32::from(p[1]) * 150
                                + u32::from(p[2]) * 29)
                                >> 8;
                            255_u8.saturating_sub(lum as u8)
                        }
                    })
                    .collect();
                return Some(BrushTipKind::AlphaMask {
                    width: w,
                    height: h,
                    alpha,
                });
            }
        }
    }
    None
}
