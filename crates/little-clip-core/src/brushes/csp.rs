use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape,
    bounded_label, mask,
};
use crate::sqlite_snapshot;
use rusqlite::Connection;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn parse_sut(
    bytes: &[u8],
    fallback_name: &str,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    if bytes.len() < 100 || !bytes.starts_with(b"SQLite format 3\0") {
        return Err("Not a Clip Studio SQLite database".into());
    }
    let mut conn = sqlite_snapshot::open(bytes, 16 * 1024 * 1024)
        .map_err(|error| format!("Cannot open Clip Studio .sut: {error}"))?;
    let has_node = has_table(&conn, "Node")?;
    let has_variant = has_table(&conn, "Variant")?;
    let has_material = has_table(&conn, "MaterialFile")?;
    if !(has_node || has_variant || has_material) {
        return Err("No Clip Studio brush tables in .sut".into());
    }
    sqlite_snapshot::authorize_reads(&mut conn).map_err(|error| error.to_string())?;

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
    let number = |column| {
        conn.query_row(
            &format!("SELECT {column} FROM main.Variant LIMIT 1"),
            [],
            |row| row.get::<_, f64>(0),
        )
        .ok()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
    };
    BrushStrokeParams {
        size_px: number("BrushSize").unwrap_or(24.0).clamp(1.0, 2000.0),
        hardness: (number("BrushHardness").unwrap_or(85.0) / 100.0).clamp(0.0, 1.0),
        spacing: (number("BrushInterval").unwrap_or(16.0) / 100.0).clamp(0.01, 5.0),
        opacity: (number("Opacity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0),
        angle_deg: number("BrushRotation").unwrap_or(0.0).rem_euclid(360.0),
        ..Default::default()
    }
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
        if let Some(png) = material_png(blob)
            && let Some((tip, _, _)) = mask::png(png, mask::OpaqueCoverage::Dark)
        {
            return Some(tip);
        }
    }
    None
}

fn material_png(blob: &[u8]) -> Option<&[u8]> {
    if blob.get(257..262) != Some(b"ustar") {
        return mask::embedded_png(blob).map(|(png, _)| png);
    }
    // A SUT can contain both texture and brush-pattern material archives.
    // Only a brush-pattern thumbnail is a substitute for the stored layer.
    let mut archive = tar::Archive::new(Cursor::new(blob));
    let mut thumbnail = None;
    let mut brush_pattern = false;
    for entry in archive.entries_with_seek().ok()?.raw(true).take(32) {
        let entry = entry.ok()?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let start = usize::try_from(entry.raw_file_position()).ok()?;
        let length = usize::try_from(entry.size()).ok()?;
        match entry.path_bytes().as_ref() {
            b"icedata/layerData.xml" if length <= 64 * 1024 => {
                let xml = std::str::from_utf8(blob.get(start..start.checked_add(length)?)?).ok()?;
                brush_pattern = is_brush_pattern(xml);
            }
            b"thumbnail/thumbnail.png" if length <= 4 * 1024 * 1024 => {
                thumbnail = blob.get(start..start.checked_add(length)?);
            }
            _ => {}
        }
    }
    thumbnail.filter(|_| brush_pattern)
}

fn is_brush_pattern(xml: &str) -> bool {
    let options = roxmltree::ParsingOptions {
        nodes_limit: 2048,
        ..Default::default()
    };
    let Ok(doc) = roxmltree::Document::parse_with_options(xml, options) else {
        return false;
    };
    doc.descendants()
        .filter(|node| node.has_tag_name("datalist") && node.attribute("key") == Some("systemtag"))
        .flat_map(|node| node.children())
        .any(|node| node.has_tag_name("data") && node.text() == Some("BrushPattern"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_saved_rotation_without_requiring_every_column() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE Variant(BrushSize REAL, BrushRotation REAL);
             INSERT INTO Variant VALUES (60, 134);",
        )
        .unwrap();
        sqlite_snapshot::authorize_reads(&mut conn).unwrap();
        let params = variant_params(&conn);
        assert_eq!(params.size_px, 60.0);
        assert_eq!(params.angle_deg, 134.0);
        assert_eq!(params.opacity, 1.0);
    }

    #[test]
    fn uses_brush_pattern_material_and_ignores_paper_texture() {
        for (tag, expected) in [
            ("BrushPattern", Some(b"tip".as_slice())),
            ("PaperTexture", None),
        ] {
            let mut archive = tar::Builder::new(Vec::new());
            let xml = format!(
                r#"<infolist><info><datalist key="systemtag"><data>{tag}</data></datalist></info></infolist>"#
            );
            for (name, data) in [
                ("thumbnail/thumbnail.png", b"tip".as_slice()),
                ("icedata/layerData.xml", xml.as_bytes()),
            ] {
                let mut header = tar::Header::new_ustar();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                archive.append_data(&mut header, name, data).unwrap();
            }
            let bytes = archive.into_inner().unwrap();
            assert_eq!(material_png(&bytes), expected);
        }
    }
}
