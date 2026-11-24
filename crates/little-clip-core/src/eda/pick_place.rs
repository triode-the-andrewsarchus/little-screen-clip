//! Pick-and-place / component centroid (`.pos`) parser and board placement renderer.

use super::{
    allocate_canvas, bounded_source_text, check_cancelled, pixmap_to_thumbnail, update_bounds,
};
use crate::ImageThumbnail;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Stroke};

const MAX_PLACEMENTS: usize = 200_000;
const BG_COLOR: [u8; 4] = [18, 22, 26, 255];
const TOP_RGB: [u8; 3] = [232, 134, 68];
const BOTTOM_RGB: [u8; 3] = [74, 174, 232];
const OTHER_RGB: [u8; 3] = [186, 152, 230];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementSide {
    Top,
    Bottom,
    Other,
}

impl PlacementSide {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Other => "Other",
        }
    }

    pub const fn rgb(self) -> [u8; 3] {
        match self {
            Self::Top => TOP_RGB,
            Self::Bottom => BOTTOM_RGB,
            Self::Other => OTHER_RGB,
        }
    }

    fn parse(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "top" | "t" | "front" | "f" | "f.cu" | "top layer"
        ) {
            Self::Top
        } else if matches!(
            lower.as_str(),
            "bottom" | "bot" | "b" | "back" | "b.cu" | "bottom layer"
        ) {
            Self::Bottom
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub line: usize,
    pub ref_des: String,
    pub value: String,
    pub package: String,
    pub side: PlacementSide,
    pub side_raw: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub rotation_deg: f64,
}

#[derive(Clone, Debug)]
pub struct PickAndPlaceDocument {
    pub units: String,
    pub inches_input: bool,
    pub placements: Vec<Placement>,
    pub top_count: usize,
    pub bottom_count: usize,
    pub unique_packages: usize,
    pub bounds_mm: [f64; 4],
    pub source_excerpt: String,
}

impl PickAndPlaceDocument {
    /// Parses KiCad ASCII `.pos`, KiCad CSV `.pos`, or generic CSV/TSV pick-and-place files.
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "Pick-and-place parse cancelled")?;
        if bytes.contains(&0) {
            return Err("Pick-and-place file contains binary NUL bytes".into());
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "Pick-and-place file is not valid UTF-8".to_string())?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);

        let default_unit = if detect_inches(text) {
            PositionUnit::Inches
        } else {
            PositionUnit::Millimeters
        };
        let (placements, unit) = match parse_csv_or_tsv(text, default_unit, cancelled)? {
            Some(parsed) => parsed,
            None => (
                parse_kicad_ascii(text, default_unit.scale_to_mm(), cancelled)?,
                default_unit,
            ),
        };
        if placements.is_empty() {
            return Err("No component placements found in .pos file".into());
        }

        let mut top_count = 0_usize;
        let mut bottom_count = 0_usize;
        let mut packages = BTreeSet::new();
        let mut bounds = None;

        for item in &placements {
            match item.side {
                PlacementSide::Top => top_count += 1,
                PlacementSide::Bottom => bottom_count += 1,
                PlacementSide::Other => {}
            }
            if !item.package.is_empty() {
                packages.insert(item.package.as_str());
            }
            update_bounds(&mut bounds, item.x_mm, item.y_mm);
        }

        Ok(Self {
            units: unit.label().into(),
            inches_input: unit == PositionUnit::Inches,
            top_count,
            bottom_count,
            unique_packages: packages.len(),
            bounds_mm: bounds.unwrap_or([0.0, 0.0, 10.0, 10.0]),
            placements,
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        let w = (self.bounds_mm[2] - self.bounds_mm[0]).max(0.0);
        let h = (self.bounds_mm[3] - self.bounds_mm[1]).max(0.0);
        format!(
            "Pick & Place · {} parts ({}T / {}B) · {:.1}×{:.1} mm",
            self.placements.len(),
            self.top_count,
            self.bottom_count,
            w,
            h
        )
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "Pick-and-place render cancelled")?;
        let pad_span = ((self.bounds_mm[2] - self.bounds_mm[0])
            .abs()
            .max((self.bounds_mm[3] - self.bounds_mm[1]).abs())
            * 0.08)
            .clamp(1.0, 8.0);
        let padded = [
            self.bounds_mm[0] - pad_span,
            self.bounds_mm[1] - pad_span,
            self.bounds_mm[2] + pad_span,
            self.bounds_mm[3] + pad_span,
        ];
        let bg = Color::from_rgba8(BG_COLOR[0], BG_COLOR[1], BG_COLOR[2], BG_COLOR[3]);
        let (mut image, transform, scale) = allocate_canvas(padded, max_size, bg)
            .ok_or_else(|| "Could not allocate Pick & Place preview".to_string())?;

        let box_half = f64::from(3.2 / scale.max(0.05)).clamp(0.4, 2.2);
        let stroke = Stroke {
            width: (box_half * 0.25).max(0.8 / f64::from(scale.max(0.05))) as f32,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };

        for (idx, item) in self.placements.iter().enumerate() {
            if idx.is_multiple_of(256) {
                check_cancelled(cancelled, "Pick-and-place render cancelled")?;
            }
            let [r, g, b] = item.side.rgb();
            let mut fill_paint = Paint::default();
            fill_paint.set_color_rgba8(r, g, b, 210);
            fill_paint.anti_alias = true;

            let rad = item.rotation_deg.to_radians();
            let (sin_r, cos_r) = rad.sin_cos();
            let hw = box_half;
            let hh = box_half * 0.65;
            let corners = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
            let mut pb = PathBuilder::new();
            for (i, [lx, ly]) in corners.iter().enumerate() {
                let wx = (item.x_mm + lx * cos_r - ly * sin_r) as f32;
                let wy = (item.y_mm + lx * sin_r + ly * cos_r) as f32;
                if i == 0 {
                    pb.move_to(wx, wy);
                } else {
                    pb.line_to(wx, wy);
                }
            }
            pb.close();
            if let Some(path) = pb.finish() {
                image.fill_path(&path, &fill_paint, FillRule::Winding, transform, None);
            }

            let mut dir_paint = Paint::default();
            dir_paint.set_color_rgba8(245, 247, 250, 235);
            dir_paint.anti_alias = true;
            let tip_x = (item.x_mm + hw * 1.35 * cos_r) as f32;
            let tip_y = (item.y_mm + hw * 1.35 * sin_r) as f32;
            let mut dir = PathBuilder::new();
            dir.move_to(item.x_mm as f32, item.y_mm as f32);
            dir.line_to(tip_x, tip_y);
            if let Some(path) = dir.finish() {
                image.stroke_path(&path, &dir_paint, &stroke, transform, None);
            }
        }

        Ok(pixmap_to_thumbnail(image))
    }
}

fn detect_inches(text: &str) -> bool {
    for line in text.lines().take(24) {
        let lower = line.to_ascii_lowercase();
        if (lower.contains("unit") || lower.contains("units"))
            && (lower.contains("inch") || lower.contains("(in)"))
        {
            return true;
        }
        if lower.contains("unit") && lower.contains("mm") {
            return false;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PositionUnit {
    Millimeters,
    Inches,
    Mils,
}

impl PositionUnit {
    fn scale_to_mm(self) -> f64 {
        match self {
            Self::Millimeters => 1.0,
            Self::Inches => 25.4,
            Self::Mils => 0.0254,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Millimeters => "Millimeters (mm)",
            Self::Inches => "Inches (converted to mm)",
            Self::Mils => "Mils (converted to mm)",
        }
    }
}

#[derive(Default, Debug)]
struct ColumnMap {
    ref_des: Option<usize>,
    value: Option<usize>,
    package: Option<usize>,
    x: Option<usize>,
    y: Option<usize>,
    rot: Option<usize>,
    side: Option<usize>,
}

#[derive(Clone, Copy)]
enum HeaderColumn {
    RefDes,
    Value,
    Package,
    X,
    Y,
    Rot,
    Side,
}

const HEADER_ALIASES: &[(&[&str], HeaderColumn)] = &[
    (
        &[
            "ref",
            "refdes",
            "designator",
            "reference",
            "comp",
            "component",
        ],
        HeaderColumn::RefDes,
    ),
    (&["val", "value", "comment", "part"], HeaderColumn::Value),
    (
        &["package", "footprint", "pattern", "case"],
        HeaderColumn::Package,
    ),
    (
        &[
            "posx",
            "pos_x",
            "x",
            "mid x",
            "center-x(mm)",
            "center-x(mil)",
            "locx",
            "x (mm)",
        ],
        HeaderColumn::X,
    ),
    (
        &[
            "posy",
            "pos_y",
            "y",
            "mid y",
            "center-y(mm)",
            "center-y(mil)",
            "locy",
            "y (mm)",
        ],
        HeaderColumn::Y,
    ),
    (&["rot", "rotation", "angle", "rot(deg)"], HeaderColumn::Rot),
    (&["side", "layer", "tb", "top/bottom"], HeaderColumn::Side),
];

fn classify_header(norm: &str) -> Option<HeaderColumn> {
    HEADER_ALIASES
        .iter()
        .find(|(aliases, _)| aliases.contains(&norm))
        .map(|(_, col)| *col)
}

impl ColumnMap {
    fn from_headers<'a>(headers: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut map = Self::default();
        for (idx, raw) in headers.enumerate() {
            let norm = raw
                .trim()
                .trim_start_matches('#')
                .trim()
                .to_ascii_lowercase();
            let Some(col) = classify_header(&norm) else {
                continue;
            };
            let slot = match col {
                HeaderColumn::RefDes => &mut map.ref_des,
                HeaderColumn::Value => &mut map.value,
                HeaderColumn::Package => &mut map.package,
                HeaderColumn::X => &mut map.x,
                HeaderColumn::Y => &mut map.y,
                HeaderColumn::Rot => &mut map.rot,
                HeaderColumn::Side => &mut map.side,
            };
            if slot.is_none() {
                *slot = Some(idx);
            }
        }
        (map.ref_des.is_some() && map.x.is_some() && map.y.is_some()).then_some(map)
    }
}

fn parse_csv_or_tsv(
    text: &str,
    default_unit: PositionUnit,
    cancelled: &AtomicBool,
) -> Result<Option<(Vec<Placement>, PositionUnit)>, String> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with("##")
        })
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }

    let delimiter = if lines[0].1.contains('\t') && !lines[0].1.contains(',') {
        b'\t'
    } else if lines[0].1.contains(',') {
        b','
    } else {
        return Ok(None);
    };

    let mut header_idx = None;
    let mut columns = None;
    for (pos, (_, line)) in lines.iter().take(16).enumerate() {
        let fields = split_delimited_line(line, delimiter);
        if let Some(map) = ColumnMap::from_headers(fields.iter().map(|s| s.as_str())) {
            header_idx = Some(pos);
            let axis_unit = |column: Option<usize>| match fields
                .get(column?)?
                .trim_end()
                .to_ascii_lowercase()
                .rsplit_once('(')
            {
                Some((_, "mil)")) => Some(PositionUnit::Mils),
                Some((_, "mm)")) => Some(PositionUnit::Millimeters),
                _ => None,
            };
            let (x, y) = (axis_unit(map.x), axis_unit(map.y));
            if x.is_some() && y.is_some() && x != y {
                return Err("Pick-and-place coordinate columns use different units".into());
            }
            columns = Some((map, x.or(y).unwrap_or(default_unit)));
            break;
        }
    }
    let (Some(header_idx), Some((cols, unit))) = (header_idx, columns) else {
        return Ok(None);
    };

    let scale = unit.scale_to_mm();
    let mut out = Vec::new();
    for &(line_idx, raw_line) in lines.iter().skip(header_idx + 1) {
        if out.len() >= MAX_PLACEMENTS {
            break;
        }
        if line_idx.is_multiple_of(256) {
            check_cancelled(cancelled, "Pick-and-place parse cancelled")?;
        }
        if raw_line.trim_start().starts_with('#') {
            continue;
        }
        let fields = split_delimited_line(raw_line, delimiter);
        if let Some(placement) = placement_from_fields(&fields, &cols, line_idx + 1, scale) {
            out.push(placement);
        }
    }
    Ok(Some((out, unit)))
}

fn split_delimited_line(line: &str, delimiter: u8) -> Vec<String> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(line.as_bytes());
    let mut record = csv::StringRecord::new();
    match rdr.read_record(&mut record) {
        Ok(true) => record.iter().map(|s| s.trim().to_string()).collect(),
        _ => Vec::new(),
    }
}

fn parse_kicad_ascii(
    text: &str,
    scale: f64,
    cancelled: &AtomicBool,
) -> Result<Vec<Placement>, String> {
    let mut cols: Option<ColumnMap> = None;
    let mut out = Vec::new();

    for (line_idx, raw_line) in text.lines().enumerate() {
        if out.len() >= MAX_PLACEMENTS {
            break;
        }
        if line_idx.is_multiple_of(256) {
            check_cancelled(cancelled, "Pick-and-place parse cancelled")?;
        }
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with("##") {
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            let tokens = tokenize_ws_quoted(comment);
            if let Some(map) = ColumnMap::from_headers(tokens.iter().map(|s| s.as_str())) {
                cols = Some(map);
            }
            continue;
        }
        let tokens = tokenize_ws_quoted(trimmed);
        if tokens.is_empty() {
            continue;
        }
        let default_cols = ColumnMap {
            ref_des: Some(0),
            value: Some(1),
            package: Some(2),
            x: Some(3),
            y: Some(4),
            rot: Some(5),
            side: Some(6),
        };
        let active = cols.as_ref().unwrap_or(&default_cols);
        if let Some(placement) = placement_from_fields(&tokens, active, line_idx + 1, scale) {
            out.push(placement);
        }
    }
    Ok(out)
}

fn tokenize_ws_quoted(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
        } else if ch == '"' && current.is_empty() {
            in_quotes = true;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn placement_from_fields(
    fields: &[String],
    cols: &ColumnMap,
    line: usize,
    scale_to_mm: f64,
) -> Option<Placement> {
    let ref_des = cols.ref_des.and_then(|i| fields.get(i))?.trim();
    if ref_des.is_empty() {
        return None;
    }
    let x_raw = cols.x.and_then(|i| fields.get(i))?;
    let y_raw = cols.y.and_then(|i| fields.get(i))?;
    let x_mm = parse_coord(x_raw)? * scale_to_mm;
    let y_mm = parse_coord(y_raw)? * scale_to_mm;
    let rotation_deg = cols
        .rot
        .and_then(|i| fields.get(i))
        .and_then(|s| parse_coord(s))
        .unwrap_or(0.0);
    let value = cols
        .value
        .and_then(|i| fields.get(i))
        .cloned()
        .unwrap_or_default();
    let package = cols
        .package
        .and_then(|i| fields.get(i))
        .cloned()
        .unwrap_or_default();
    let side_raw = cols
        .side
        .and_then(|i| fields.get(i))
        .cloned()
        .unwrap_or_else(|| "top".into());
    let side = PlacementSide::parse(&side_raw);

    Some(Placement {
        line,
        ref_des: ref_des.to_string(),
        value,
        package,
        side,
        side_raw,
        x_mm,
        y_mm,
        rotation_deg,
    })
}

fn parse_coord(raw: &str) -> Option<f64> {
    let stripped = raw
        .trim()
        .trim_end_matches("mm")
        .trim_end_matches("deg")
        .trim_end_matches('°')
        .trim();
    let value: f64 = stripped.parse().ok()?;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centroid_headers_convert_mils_to_millimeters() {
        let bytes = b"Designator,Center-X(mil),Center-Y(mil),Layer\nR1,1000,-500,Top\n";
        let doc = PickAndPlaceDocument::parse(bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(doc.units, "Mils (converted to mm)");
        let part = &doc.placements[0];
        assert!((part.x_mm - 25.4).abs() < 1e-9);
        assert!((part.y_mm + 12.7).abs() < 1e-9);
    }

    #[test]
    fn kicad_ascii_and_csv_pick_and_place_files_extract_all_fields() {
        let ascii = "\
### Module positions - created on 2026-01-01 ###
## Unit = mm, Angle = deg.
## Side : All
# Ref     Val              Package                PosX       PosY       Rot  Side
C1        100nF            C_0603_1608Metric     110.2500   -85.1000   90.0000  top
U1        \"ATmega 328P\"    TQFP-32               125.0000   -92.5000  180.0000  bottom
## End
";
        let cancel = AtomicBool::new(false);
        let doc = PickAndPlaceDocument::parse(ascii.as_bytes(), &cancel).unwrap();
        assert_eq!(doc.placements.len(), 2);
        assert_eq!(doc.top_count, 1);
        assert_eq!(doc.bottom_count, 1);
        assert_eq!(doc.unique_packages, 2);
        assert_eq!(doc.placements[0].ref_des, "C1");
        assert_eq!(doc.placements[0].package, "C_0603_1608Metric");
        assert_eq!(doc.placements[0].side, PlacementSide::Top);
        assert!((doc.placements[0].x_mm - 110.25).abs() < 1e-6);
        assert!((doc.placements[0].y_mm + 85.1).abs() < 1e-6);
        assert!((doc.placements[0].rotation_deg - 90.0).abs() < 1e-6);
        assert_eq!(doc.placements[1].value, "ATmega 328P");
        assert_eq!(doc.placements[1].side, PlacementSide::Bottom);

        let csv_data = "Ref,Val,Package,PosX,PosY,Rot,Side\nR1,10k,R_0402,1.0,2.0,45.0,top\n";
        let csv_doc = PickAndPlaceDocument::parse(csv_data.as_bytes(), &cancel).unwrap();
        assert_eq!(csv_doc.placements.len(), 1);
        assert_eq!(csv_doc.placements[0].ref_des, "R1");
        assert_eq!(csv_doc.placements[0].rotation_deg, 45.0);

        let thumb = doc.render_thumbnail([160, 100], &cancel).unwrap();
        assert!(
            thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [BG_COLOR[0], BG_COLOR[1], BG_COLOR[2]])
        );
    }
}
