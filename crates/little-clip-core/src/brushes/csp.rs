use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape,
    bounded_label, mask,
};
use crate::sqlite_snapshot;
use rusqlite::Connection;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn parse_sut(
    bytes: &[u8],
    fallback_name: &str,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    if bytes.len() < 100 || !bytes.starts_with(b"SQLite format 3\0") {
        return Err("Not a Clip Studio SQLite database".into());
    }
    let conn = sqlite_snapshot::open(bytes, 16 * 1024 * 1024)
        .map_err(|error| format!("Cannot open Clip Studio .sut: {error}"))?;
    let has_node = has_table(&conn, "Node")?;
    let has_variant = has_table(&conn, "Variant")?;
    let has_material = has_table(&conn, "MaterialFile")?;
    if !(has_node || has_variant || has_material) {
        return Err("No Clip Studio brush tables in .sut".into());
    }
    sqlite_snapshot::authorize_reads(&conn);

    let name =
        if has_node {
            conn.query_row(
            "SELECT NodeName FROM main.Node WHERE NodeName IS NOT NULL AND NodeName != '' LIMIT 1",
            [],
            |row| row.get::<_, String>(0).map(|name| bounded_label(&name).to_owned()),
        )
        .unwrap_or_else(|_| fallback_name.to_owned())
        } else {
            fallback_name.to_owned()
        };

    let params = if has_variant {
        variant_params(&conn)
    } else {
        BrushStrokeParams::default()
    };

    let tip = has_material
        .then(|| extract_material_tip(&conn, cancelled))
        .flatten()
        .unwrap_or(BrushTipKind::Parametric {
            shape: ParametricShape::Circle,
            hardness: params.hardness,
            aspect_ratio: 1.0,
            angle_deg: params.angle_deg,
            spikes: 2,
        });
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }

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

fn variant_params(conn: &Connection) -> BrushStrokeParams {
    conn.query_row(
        "SELECT BrushSize, BrushHardness, BrushInterval, Opacity FROM main.Variant LIMIT 1",
        [],
        |row| {
            let number = |index| {
                row.get::<_, f64>(index)
                    .ok()
                    .map(|value| value as f32)
                    .filter(|value| value.is_finite())
            };
            let mut params = BrushStrokeParams::default();
            if let Some(size) = number(0) {
                params.size_px = size.clamp(1.0, 2000.0);
            }
            if let Some(hardness) = number(1) {
                params.hardness = (hardness / 100.0).clamp(0.05, 1.0);
            }
            if let Some(interval) = number(2) {
                params.spacing = (interval / 100.0).clamp(0.04, 2.0);
            }
            if let Some(opacity) = number(3) {
                params.opacity = (opacity / 100.0).clamp(0.05, 1.0);
            }
            Ok(params)
        },
    )
    .unwrap_or_default()
}

fn has_table(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_list(?1) WHERE schema = 'main' AND type = 'table')",
        [name],
        |row| row.get(0),
    )
    .map_err(|error| format!("Cannot read Clip Studio .sut schema: {error}"))
}

fn extract_material_tip(conn: &Connection, cancelled: &AtomicBool) -> Option<BrushTipKind> {
    let mut stmt = conn
        .prepare("SELECT FileData FROM main.MaterialFile WHERE FileData IS NOT NULL LIMIT 8")
        .ok()?;
    let mut rows = stmt.query([]).ok()?;
    while !cancelled.load(Ordering::Relaxed)
        && let Ok(Some(row)) = rows.next()
    {
        let Ok(rusqlite::types::ValueRef::Blob(blob)) = row.get_ref(0) else {
            continue;
        };
        if let Some((png, _)) = mask::embedded_png(blob)
            && let Some((tip, _, _)) = mask::png(png, mask::OpaqueCoverage::Dark)
        {
            return Some(tip);
        }
    }
    None
}
