//! Registered comparison of decoded board and sheet objects.

use super::{
    EdaFormat, RasterRegion, allocate_canvas_with_y_flip, check_cancelled,
    comparison::ArtworkRaster,
    kicad::{pcb, scene_render, schematic::KicadSchematicDocument},
    pixmap_to_thumbnail, read_snapshot, update_bounds,
};
use crate::file_source::FileSource;
use eda_view::vector::{Prim, Prop, Role, Scene};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use tiny_skia::{Color, Pixmap, Transform};

const MAX_RECORDS: usize = 120_000;
const MAX_POINTS: usize = 1_000_000;

pub(in crate::eda) enum Geometry {
    Scene(Prim),
    Board(pcb::PcbPrim),
}

pub(in crate::eda) struct Record {
    key: String,
    label: String,
    geometry: Vec<Geometry>,
}

impl Record {
    pub(in crate::eda) fn new(
        label: String,
        identity: impl std::fmt::Debug,
        geometry: Vec<Geometry>,
    ) -> Self {
        let mut primitives: Vec<_> = geometry.iter().map(Geometry::key).collect();
        primitives.sort_unstable();
        let key = format!("{identity:?} {primitives:?}");
        Self {
            key,
            label,
            geometry,
        }
    }
}

#[derive(Default)]
pub(in crate::eda) struct Drawing {
    records: Vec<Record>,
    pub(in crate::eda) warnings: Vec<String>,
    points: usize,
}

impl Drawing {
    pub(in crate::eda) fn push(&mut self, record: Record) -> Result<(), String> {
        self.points += record.geometry.iter().map(Geometry::points).sum::<usize>();
        if self.records.len() >= MAX_RECORDS || self.points > MAX_POINTS {
            return Err("Decoded artwork exceeds the source comparison budget".into());
        }
        self.records.push(record);
        Ok(())
    }

    pub(in crate::eda) fn from_scene(scene: &Scene, cancel: &AtomicBool) -> Result<Self, String> {
        let mut drawing = Self::default();
        let groups: BTreeMap<_, _> = scene.groups.iter().map(|group| (group.id, group)).collect();
        let nets: BTreeMap<_, _> = scene
            .nets
            .iter()
            .map(|net| (net.id, net.name.as_str()))
            .collect();
        let layers: BTreeMap<_, _> = scene
            .layers
            .iter()
            .map(|layer| (layer.id, layer.name.as_str()))
            .collect();
        for (index, item) in scene.items.iter().enumerate() {
            if index.is_multiple_of(256) {
                check_cancelled(cancel, "Sheet comparison cancelled")?;
            }
            let layer = layers.get(&item.layer).copied();
            if matches!(layer, Some("Drawing sheet" | "Title block")) {
                continue;
            }
            let group = item.group.and_then(|id| groups.get(&id));
            let net = item.net.and_then(|id| nets.get(&id)).copied();
            let properties = semantic_props(&item.props);
            let group_props = group.map(|g| semantic_props(&g.props));
            let group_label = group.map(|g| g.label.as_str()).unwrap_or("");
            let role = match item.role {
                Role::SymbolBody => "Symbol".into(),
                role => format!("{role:?}"),
            };
            let pin_or_text = properties
                .iter()
                .find_map(|&(key, value)| matches!(key, "number" | "text").then_some(value))
                .unwrap_or("");
            let label = [role.as_str(), group_label, pin_or_text, net.unwrap_or("")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            drawing.push(Record::new(
                label,
                (role, layer, group_label, group_props, properties, net),
                vec![Geometry::Scene(item.prim.clone())],
            ))?;
        }
        Ok(drawing)
    }
}

fn semantic_props(properties: &[Prop]) -> Vec<(&str, &str)> {
    let mut properties: Vec<_> = properties
        .iter()
        .filter(|p| {
            !matches!(
                p.key.as_str(),
                "uuid" | "page" | "child_page" | "at" | "rotation"
            )
        })
        .map(|p| (p.key.as_str(), p.value.as_str()))
        .collect();
    properties.sort_unstable();
    properties
}

pub struct SourceChange {
    pub side: usize,
    pub label: String,
    pub center: Option<[f64; 2]>,
}

pub struct SourceComparison {
    pub bounds: [f64; 4],
    pub warnings: Vec<String>,
    pub changes: Vec<SourceChange>,
    pub counts: [usize; 2],
    drawings: [Drawing; 2],
    changed: [Vec<usize>; 2],
}

pub fn compare_sources(
    format: EdaFormat,
    files: [&FileSource; 2],
    cancel: &AtomicBool,
) -> Result<SourceComparison, String> {
    let load = |side: usize| -> Result<Drawing, String> {
        let file = files[side];
        let path = file.path();
        let (drawing, observed) = read_snapshot(path, cancel, |bytes, snapshot| match format {
            EdaFormat::KicadPcb => pcb::comparison_drawing(bytes, cancel),
            EdaFormat::KicadSchematic => {
                let sheet = KicadSchematicDocument::parse(path, bytes, cancel)?;
                let scene = sheet.scene.ok_or("Sheet has no decoded comparison scene")?;
                let mut drawing = Drawing::from_scene(&scene, cancel)?;
                drawing.warnings = sheet.warnings;
                for symbol in &sheet.symbols {
                    drawing.push(Record::new(
                        format!("Symbol {} · {}", symbol.reference, symbol.value),
                        (
                            &symbol.reference,
                            &symbol.value,
                            &symbol.lib_id,
                            &symbol.footprint,
                            symbol.unit,
                            symbol.rotation_deg.rem_euclid(360.0),
                            &symbol.mirror,
                        ),
                        vec![Geometry::Scene(Prim::Circle {
                            center: symbol.pos_mm,
                            radius: 0.30,
                            fill: false,
                            stroke: 0.10,
                        })],
                    ))?;
                }
                if sheet.symbols.len() < sheet.symbol_count {
                    drawing
                        .warnings
                        .push("Some symbol placements exceed the sheet inspection limit".into());
                }
                drawing.warnings.push(
                    "One sheet · connectivity follows decoded wires, junctions and named nets"
                        .into(),
                );
                Ok(drawing)
            }
            EdaFormat::LtspiceSchematic => super::ltspice::comparison_drawing(
                bytes,
                snapshot.path().parent(),
                ["A", "B"][side],
                cancel,
            ),
            _ => Err("Expected a supported board or schematic".into()),
        })?;
        if &observed != file {
            return Err("Comparison source changed".into());
        }
        Ok(drawing)
    };
    SourceComparison::new([load(0)?, load(1)?], cancel)
}

impl SourceComparison {
    fn new(drawings: [Drawing; 2], cancel: &AtomicBool) -> Result<Self, String> {
        let mut keys: BTreeMap<_, [Vec<usize>; 2]> = BTreeMap::new();
        let mut bounds = None;
        for (side, drawing) in drawings.iter().enumerate() {
            for (index, record) in drawing.records.iter().enumerate() {
                if index.is_multiple_of(256) {
                    check_cancelled(cancel, "Source comparison cancelled")?;
                }
                keys.entry(record.key.as_str()).or_default()[side].push(index);
                record.extend_bounds(&mut bounds);
            }
        }
        let mut changed = [Vec::new(), Vec::new()];
        for [a, b] in keys.into_values() {
            let matching = a.len().min(b.len());
            changed[0].extend_from_slice(&a[matching..]);
            changed[1].extend_from_slice(&b[matching..]);
        }
        let counts = changed.each_ref().map(Vec::len);
        let changes = changed
            .iter()
            .enumerate()
            .flat_map(|(side, indices)| indices.iter().map(move |&i| (side, i)))
            .map(|(side, i)| {
                let item = &drawings[side].records[i];
                let mut bounds = None;
                item.extend_bounds(&mut bounds);
                SourceChange {
                    side,
                    label: item.label.clone(),
                    center: bounds.map(|[x0, y0, x1, y1]| [(x0 + x1) * 0.5, (y0 + y1) * 0.5]),
                }
            })
            .collect();
        let warnings = drawings
            .iter()
            .flat_map(|drawing| drawing.warnings.iter())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Self {
            drawings,
            changed,
            changes,
            counts,
            warnings,
            bounds: bounds.unwrap_or([0.0, 0.0, 40.0, 40.0]),
        })
    }

    pub fn render_overview(&self, cancel: &AtomicBool) -> Result<ArtworkRaster, String> {
        let (canvas, transform, _) =
            allocate_canvas_with_y_flip(self.bounds, [1400, 1400], Color::TRANSPARENT, true)
                .ok_or("Invalid source comparison bounds")?;
        self.render(canvas, transform, None, cancel)
    }

    pub fn render_region(
        &self,
        region: RasterRegion,
        cancel: &AtomicBool,
    ) -> Result<ArtworkRaster, String> {
        let (canvas, transform, _) = region.canvas(Color::TRANSPARENT)?;
        self.render(canvas, transform, Some(region), cancel)
    }

    fn render(
        &self,
        mut changes: Pixmap,
        transform: Transform,
        region: Option<RasterRegion>,
        cancel: &AtomicBool,
    ) -> Result<ArtworkRaster, String> {
        let allocate = || {
            Pixmap::new(changes.width(), changes.height())
                .ok_or("Could not allocate source comparison")
        };
        let mut before = allocate()?;
        let mut after = allocate()?;
        for (side, canvas) in [&mut before, &mut after].into_iter().enumerate() {
            let records = &self.drawings[side].records;
            draw(
                canvas,
                records.iter(),
                [255, 255, 255, 255],
                transform,
                region,
                cancel,
            )?;
            let color = [[248, 140, 78, 220], [92, 220, 247, 220]][side];
            draw(
                &mut changes,
                self.changed[side].iter().map(|&i| &records[i]),
                color,
                transform,
                region,
                cancel,
            )?;
        }
        Ok(ArtworkRaster {
            pitch_mm: f64::from(transform.sx).recip(),
            before: pixmap_to_thumbnail(before),
            after: pixmap_to_thumbnail(after),
            changes: pixmap_to_thumbnail(changes),
        })
    }
}

impl Record {
    fn extend_bounds(&self, bounds: &mut Option<[f64; 4]>) {
        for primitive in &self.geometry {
            if let Some([x0, y0, x1, y1]) = primitive.bounds() {
                update_bounds(bounds, x0, y0);
                update_bounds(bounds, x1, y1);
            }
        }
    }
}

fn draw<'a>(
    canvas: &mut Pixmap,
    records: impl Iterator<Item = &'a Record>,
    color: [u8; 4],
    transform: Transform,
    region: Option<RasterRegion>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    for (index, record) in records.enumerate() {
        if index.is_multiple_of(256) {
            check_cancelled(cancel, "Source comparison drawing cancelled")?;
        }
        for primitive in &record.geometry {
            if region.is_some_and(|region| !primitive.visible_in(region)) {
                continue;
            }
            match primitive {
                Geometry::Scene(prim) => {
                    scene_render::draw_scene_prim(canvas, prim, color, transform, transform.sx)
                }
                Geometry::Board(prim) => pcb::draw_pcb_prim(
                    canvas,
                    prim,
                    color,
                    transform,
                    f64::from(0.9 / transform.sx.max(0.01)),
                ),
            }
        }
    }
    Ok(())
}

impl Geometry {
    fn visible_in(&self, region: RasterRegion) -> bool {
        match self {
            Self::Scene(_) => self
                .bounds()
                .is_some_and(|bounds| region.intersects(bounds, 2.0 / region.pixels_per_mm)),
            Self::Board(primitive) => primitive.visible_in(region),
        }
    }

    fn bounds(&self) -> Option<[f64; 4]> {
        match self {
            Self::Scene(primitive) => {
                let bounds = primitive.bbox();
                (!bounds.is_empty()).then_some([
                    bounds.min[0],
                    bounds.min[1],
                    bounds.max[0],
                    bounds.max[1],
                ])
            }
            Self::Board(primitive) => {
                let mut bounds = None;
                primitive.extend_bounds(&mut bounds);
                bounds
            }
        }
    }

    fn points(&self) -> usize {
        match self {
            Self::Scene(Prim::Polyline { points, .. })
            | Self::Board(pcb::PcbPrim::Polygon { points, .. }) => points.len(),
            Self::Scene(Prim::Polygon { outer, holes, .. }) => {
                outer.len() + holes.iter().map(Vec::len).sum::<usize>()
            }
            Self::Scene(Prim::Strokes { strokes, .. }) => strokes.iter().map(Vec::len).sum(),
            _ => 4,
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Scene(Prim::Polyline { points, width }) => {
                format!("line {width:?} {:?}", line_key(points))
            }
            Self::Scene(Prim::Polygon {
                outer,
                holes,
                fill,
                stroke,
            }) => {
                let mut holes: Vec<_> = holes.iter().map(|ring| ring_key(ring)).collect();
                holes.sort_unstable();
                format!("polygon {fill} {stroke:?} {:?} {holes:?}", ring_key(outer))
            }
            Self::Scene(Prim::Arc {
                center,
                radius,
                start,
                end,
                width,
            }) => {
                format!(
                    "arc {width:?} {:?}",
                    arc_key(*center, *radius, *start, *end)
                )
            }
            Self::Scene(prim) => format!("{prim:?}"),
            Self::Board(pcb::PcbPrim::Segment { layer, a, b, width }) => {
                format!("segment {layer:?} {width:?} {:?}", line_key(&[*a, *b]))
            }
            Self::Board(pcb::PcbPrim::Polygon {
                layer,
                points,
                fill,
                stroke,
            }) => format!("polygon {layer:?} {fill} {stroke:?} {:?}", ring_key(points)),
            Self::Board(pcb::PcbPrim::Arc {
                layer,
                center,
                radius,
                start_rad,
                end_rad,
                width,
            }) => {
                format!(
                    "arc {layer:?} {width:?} {:?}",
                    arc_key(*center, *radius, *start_rad, *end_rad)
                )
            }
            Self::Board(prim) => format!("{prim:?}"),
        }
    }
}

type PointKey = [i64; 2];

fn points_key(points: &[[f64; 2]]) -> Vec<PointKey> {
    points
        .iter()
        .map(|p| p.map(|v| (v * 1_000_000.0).round() as i64))
        .collect()
}

fn line_key(points: &[[f64; 2]]) -> Vec<PointKey> {
    let mut points = points_key(points);
    if points.iter().rev().cmp(points.iter()).is_lt() {
        points.reverse();
    }
    points
}

fn arc_key(center: [f64; 2], radius: f64, start: f64, end: f64) -> Vec<PointKey> {
    line_key(&[start, (start + end) * 0.5, end].map(|angle| {
        let (y, x) = angle.sin_cos();
        [center[0] + radius * x, center[1] + radius * y]
    }))
}

fn ring_key(points: &[[f64; 2]]) -> Vec<PointKey> {
    let mut ring = points_key(points);
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    let normalize = |mut ring: Vec<_>| {
        if let Some((start, _)) = ring.iter().enumerate().min_by_key(|(_, p)| *p) {
            ring.rotate_left(start);
        }
        ring
    };
    normalize(ring.clone()).min(normalize(ring.into_iter().rev().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compare(format: EdaFormat, extension: &str, a: &str, b: &str) -> SourceComparison {
        let folder = tempfile::tempdir().unwrap();
        let files = [0, 1].map(|side| {
            let text = [a, b][side];
            let path = folder.path().join(format!("side-{side}.{extension}"));
            std::fs::write(&path, text).unwrap();
            FileSource::capture(&path).unwrap()
        });
        compare_sources(format, files.each_ref(), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn kicad_sheet_compare_detects_a_rewire_with_unchanged_counts() {
        let sheet = |wire: &str| {
            format!(
                "(kicad_sch (version 20231120) (generator eeschema) (paper \"A4\") (wire (pts {wire})) (label \"IN\" (at 0 0 0)))"
            )
        };
        let original = sheet("(xy 0 0) (xy 10 0)");
        let compare = |b: String| {
            super::tests::compare(EdaFormat::KicadSchematic, "kicad_sch", &original, &b)
        };
        assert_eq!(compare(sheet("(xy 10 0) (xy 0 0)")).counts, [0, 0]);
        let changed = compare(sheet("(xy 0 0) (xy 10 3)"));
        assert_eq!(changed.counts, [1, 1]);
        assert!(
            changed
                .changes
                .iter()
                .all(|record| record.label.contains("Wire"))
        );
        let raster = changed.render_overview(&AtomicBool::new(false)).unwrap();
        assert!(
            raster
                .changes
                .premultiplied_rgba
                .chunks_exact(4)
                .any(|color| color[3] > 0)
        );
    }

    #[test]
    fn kicad_board_compare_detects_track_geometry_and_pad_net_changes() {
        let board = |ends: &str, track_net: &str, pad_net: &str| {
            format!(
                r#"(kicad_pcb (version 20240108) (generator pcbnew)
                (net 1 "SIG") (net 2 "GND")
                (segment {ends} (width 0.25) (layer "F.Cu") (net {track_net}))
                (footprint "Test" (layer "F.Cu") (at 2 3 0)
                    (property "Reference" "U1")
                    (pad "1" smd rect (at 0 0) (size 1 2) (layers "F.Cu" "F.Paste") (net {pad_net}))))"#
            )
        };
        let original = board("(start 0 0) (end 10 0)", "1", "1");
        let compare =
            |b: String| super::tests::compare(EdaFormat::KicadPcb, "kicad_pcb", &original, &b);
        assert_eq!(
            compare(board("(end 0.00 0) (start 10.0 0)", "\"SIG\"", "1")).counts,
            [0, 0]
        );
        let changed = compare(board("(start 0 0) (end 10 3)", "1", "2"));
        assert_eq!(changed.counts, [2, 2]);
        assert!(
            changed
                .changes
                .iter()
                .any(|record| record.label == "Pad U1.1 · GND")
        );
        assert!(
            changed
                .changes
                .iter()
                .any(|record| record.label == "Track · F.Cu · SIG")
        );
    }
}
