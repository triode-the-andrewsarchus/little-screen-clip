//! KiCad `.kicad_pcb` board layout and `.kicad_mod` footprint S-expression parsers and rasterizers.

use super::super::{
    StripSummary, allocate_canvas_with_y_flip, bounded_source_text, check_cancelled,
    pixmap_to_thumbnail, update_bounds,
};
use super::scene_render::append_arc_points;
use crate::ImageThumbnail;
use eda_view::kicad::sexp::{SExp, Value, parse as parse_sexp};
use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

const MAX_PCB_PRIMS: usize = 120_000;
const MAX_NET_LIST: usize = 1024;
const MAX_FOOTPRINT_LIST: usize = 4096;
const PCB_BG: [u8; 4] = [18, 22, 26, 255];

/// Canonical render layer buckets for interactive visibility control and thumbnails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PcbLayerBucket {
    Zones,
    BackCopper,
    InnerCopper,
    FrontCopper,
    BackSilk,
    FrontSilk,
    FabAndCourtyard,
    EdgeCuts,
    Drills,
}

impl PcbLayerBucket {
    pub const ALL: [Self; 9] = [
        Self::Zones,
        Self::BackCopper,
        Self::InnerCopper,
        Self::FrontCopper,
        Self::BackSilk,
        Self::FrontSilk,
        Self::FabAndCourtyard,
        Self::EdgeCuts,
        Self::Drills,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Zones => "Zones",
            Self::BackCopper => "B.Cu",
            Self::InnerCopper => "In.Cu",
            Self::FrontCopper => "F.Cu",
            Self::BackSilk => "B.Silkscreen",
            Self::FrontSilk => "F.Silkscreen",
            Self::FabAndCourtyard => "Fab / Courtyard",
            Self::EdgeCuts => "Edge.Cuts",
            Self::Drills => "Drills / Holes",
        }
    }

    pub const fn rgba(self) -> [u8; 4] {
        match self {
            Self::Zones => [180, 120, 62, 58],
            Self::BackCopper => [62, 134, 214, 215],
            Self::InnerCopper => [198, 152, 74, 195],
            Self::FrontCopper => [226, 78, 72, 225],
            Self::BackSilk => [124, 178, 214, 215],
            Self::FrontSilk => [232, 236, 240, 230],
            Self::FabAndCourtyard => [138, 152, 168, 150],
            Self::EdgeCuts => [244, 206, 86, 245],
            Self::Drills => [18, 22, 26, 255],
        }
    }

    fn from_layer_name(name: &str) -> Self {
        match name {
            "Edge.Cuts" => Self::EdgeCuts,
            "F.Cu" => Self::FrontCopper,
            "B.Cu" => Self::BackCopper,
            "F.SilkS" | "F.Silkscreen" => Self::FrontSilk,
            "B.SilkS" | "B.Silkscreen" => Self::BackSilk,
            n if n.starts_with("In") && n.ends_with(".Cu") => Self::InnerCopper,
            n if n.ends_with(".Cu") => Self::InnerCopper,
            n if n.contains("Silk") => Self::FrontSilk,
            _ => Self::FabAndCourtyard,
        }
    }
}

#[derive(Clone, Debug)]
pub enum PcbPrim {
    Segment {
        layer: PcbLayerBucket,
        a: [f64; 2],
        b: [f64; 2],
        width: f64,
    },
    Rect {
        layer: PcbLayerBucket,
        center: [f64; 2],
        size: [f64; 2],
        rot_deg: f64,
        fill: bool,
        stroke: f64,
    },
    Circle {
        layer: PcbLayerBucket,
        center: [f64; 2],
        radius: f64,
        fill: bool,
        stroke: f64,
    },
    Polygon {
        layer: PcbLayerBucket,
        points: Vec<[f64; 2]>,
        fill: bool,
        stroke: f64,
    },
    Arc {
        layer: PcbLayerBucket,
        center: [f64; 2],
        radius: f64,
        start_rad: f64,
        end_rad: f64,
        width: f64,
    },
}

impl PcbPrim {
    pub fn layer(&self) -> PcbLayerBucket {
        match self {
            Self::Segment { layer, .. }
            | Self::Rect { layer, .. }
            | Self::Circle { layer, .. }
            | Self::Polygon { layer, .. }
            | Self::Arc { layer, .. } => *layer,
        }
    }

    fn extend_bounds(&self, bounds: &mut Option<[f64; 4]>) {
        match self {
            Self::Segment { a, b, .. } => {
                update_bounds(bounds, a[0], a[1]);
                update_bounds(bounds, b[0], b[1]);
            }
            Self::Rect { center, size, .. } => {
                let r = size[0].hypot(size[1]) * 0.5;
                update_bounds(bounds, center[0] - r, center[1] - r);
                update_bounds(bounds, center[0] + r, center[1] + r);
            }
            Self::Circle { center, radius, .. } | Self::Arc { center, radius, .. } => {
                update_bounds(bounds, center[0] - radius, center[1] - radius);
                update_bounds(bounds, center[0] + radius, center[1] + radius);
            }
            Self::Polygon { points, .. } => {
                for p in points {
                    update_bounds(bounds, p[0], p[1]);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct PcbLayerInfo {
    pub id: Option<i64>,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct PcbNetInfo {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct PcbFootprintSummary {
    pub ref_des: String,
    pub value: String,
    pub lib_id: String,
    pub layer: String,
    pub pos_mm: [f64; 2],
    pub rot_deg: f64,
    pub pad_count: usize,
}

#[derive(Clone, Debug)]
pub struct PcbPadInfo {
    pub number: String,
    pub pad_type: String,
    pub shape: String,
    pub pos_mm: [f64; 2],
    pub size_mm: [f64; 2],
    pub drill_mm: Option<[f64; 2]>,
    pub layers: String,
}

/// Decoded `.kicad_pcb` PCB layout document.
#[derive(Clone, Debug)]
pub struct KicadPcbDocument {
    pub version: Option<i64>,
    pub generator: Option<String>,
    pub title: String,
    pub thickness_mm: Option<f64>,
    pub outline_mm: Option<[f64; 2]>,
    pub bounds_mm: [f64; 4],
    pub layer_count: usize,
    pub copper_layer_count: usize,
    pub stackup_layer_count: usize,
    pub layers: Vec<PcbLayerInfo>,
    pub footprint_count: usize,
    pub footprints: Vec<PcbFootprintSummary>,
    pub net_count: usize,
    pub named_net_count: usize,
    pub nets: Vec<PcbNetInfo>,
    pub track_segment_count: usize,
    pub track_arc_count: usize,
    pub via_count: usize,
    pub zone_count: usize,
    pub track_length_mm: f64,
    pub primitives: Vec<PcbPrim>,
    pub source_excerpt: String,
}

impl KicadPcbDocument {
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "KiCad PCB parse cancelled")?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "KiCad PCB file is not valid UTF-8".to_string())?;
        let root = parse_sexp(text).map_err(|e| format!("Invalid KiCad PCB S-expression: {e}"))?;
        if !root.has_tag("kicad_pcb") {
            return Err("Expected root (kicad_pcb ...) form".into());
        }
        check_cancelled(cancelled, "KiCad PCB parse cancelled")?;

        let layers = collect_pcb_layers(&root);
        let copper_layer_count = layers.iter().filter(|l| l.name.ends_with(".Cu")).count();
        let stackup_layer_count = count_stackup_layers(&root).max(layers.len());
        let nets = collect_nets(&root);
        let named_net_count = nets.iter().filter(|n| !n.name.is_empty()).count();
        let title = root
            .get("title_block")
            .and_then(|tb| tb.child_str("title"))
            .unwrap_or("")
            .to_string();

        let mut acc = PcbAccum::default();
        walk_pcb_children(&root, &mut acc, cancelled)?;

        let outline_mm = acc
            .edge_bounds
            .map(|b| [(b[2] - b[0]).max(0.0), (b[3] - b[1]).max(0.0)]);
        let bounds_mm = acc
            .edge_bounds
            .or(acc.all_bounds)
            .unwrap_or([0.0, 0.0, 50.0, 50.0]);

        Ok(Self {
            version: root.get("version").and_then(|v| v.int_at(0)),
            generator: root.child_str("generator").map(str::to_owned),
            title,
            thickness_mm: root.get("general").and_then(|g| g.child_f64("thickness")),
            outline_mm,
            bounds_mm,
            layer_count: layers.len(),
            copper_layer_count,
            stackup_layer_count,
            layers,
            footprint_count: acc.footprint_count,
            footprints: acc.footprints,
            net_count: nets.len(),
            named_net_count,
            nets,
            track_segment_count: acc.track_segment_count,
            track_arc_count: acc.track_arc_count,
            via_count: acc.via_count,
            zone_count: acc.zone_count,
            track_length_mm: acc.track_length_mm,
            primitives: acc.primitives,
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        let dims = self.outline_mm.unwrap_or([
            self.bounds_mm[2] - self.bounds_mm[0],
            self.bounds_mm[3] - self.bounds_mm[1],
        ]);
        format!(
            "KiCad PCB · {:.1}×{:.1} mm · {} layers ({} Cu) · {} footprints · {} nets · {:.1} mm tracks",
            dims[0],
            dims[1],
            self.stackup_layer_count,
            self.copper_layer_count,
            self.footprint_count,
            self.named_net_count,
            self.track_length_mm
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let dims = self.outline_mm.unwrap_or([
            self.bounds_mm[2] - self.bounds_mm[0],
            self.bounds_mm[3] - self.bounds_mm[1],
        ]);
        StripSummary {
            format: super::super::EdaFormat::KicadPcb,
            headline: format!(
                "KiCad PCB · {:.1}×{:.1} mm · {} layers ({} Cu)",
                dims[0], dims[1], self.stackup_layer_count, self.copper_layer_count
            ),
            specs: format!(
                "{} footprints · {} nets · {} tracks ({:.1} mm) · {} vias",
                self.footprint_count,
                self.named_net_count,
                self.track_segment_count + self.track_arc_count,
                self.track_length_mm,
                self.via_count
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        let enabled: BTreeSet<PcbLayerBucket> = PcbLayerBucket::ALL.into_iter().collect();
        self.render_layers(max_size, &enabled, cancelled)
    }

    /// Rasterizes the board using an explicit subset of visible layer buckets.
    pub fn render_layers(
        &self,
        max_size: [u32; 2],
        visible: &BTreeSet<PcbLayerBucket>,
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        render_pcb_prims(
            &self.primitives,
            self.bounds_mm,
            max_size,
            visible,
            cancelled,
        )
    }
}

/// Decoded `.kicad_mod` footprint module document.
#[derive(Clone, Debug)]
pub struct KicadFootprintDocument {
    pub name: String,
    pub layer: String,
    pub description: String,
    pub tags: String,
    pub attributes: Vec<String>,
    pub pads: Vec<PcbPadInfo>,
    pub models_3d: Vec<String>,
    pub primitives: Vec<PcbPrim>,
    pub bounds_mm: [f64; 4],
    pub source_excerpt: String,
}

impl KicadFootprintDocument {
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "KiCad footprint parse cancelled")?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "KiCad footprint file is not valid UTF-8".to_string())?;
        let root =
            parse_sexp(text).map_err(|e| format!("Invalid KiCad footprint S-expression: {e}"))?;
        if !root.has_tag("footprint") && !root.has_tag("module") {
            return Err("Expected root (footprint ...) or (module ...) form".into());
        }

        let name = root.string_at(0).unwrap_or("Footprint").to_string();
        let layer = root.child_str("layer").unwrap_or("F.Cu").to_string();
        let description = root.child_str("descr").unwrap_or("").to_string();
        let tags = root.child_str("tags").unwrap_or("").to_string();
        let attributes = root
            .children_named("attr")
            .flat_map(|attr| attr.atoms().filter_map(Value::as_str).map(str::to_owned))
            .collect();
        let models_3d = root
            .children_named("model")
            .filter_map(|m| m.string_at(0).map(str::to_owned))
            .collect();

        let mut pads = Vec::new();
        let mut primitives = Vec::new();
        let mut bounds = None;

        for child in &root.children {
            match child.tag() {
                Some("pad") => {
                    if let Some(info) = collect_pad(child, [0.0, 0.0, 0.0], &mut primitives) {
                        pads.push(info);
                    }
                }
                Some(tag) if tag.starts_with("fp_") => {
                    collect_graphic_node(child, tag, [0.0, 0.0, 0.0], &mut primitives);
                }
                _ => {}
            }
        }
        for prim in &primitives {
            prim.extend_bounds(&mut bounds);
        }

        Ok(Self {
            name,
            layer,
            description,
            tags,
            attributes,
            pads,
            models_3d,
            primitives,
            bounds_mm: bounds.unwrap_or([-5.0, -5.0, 5.0, 5.0]),
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        let tht = self
            .pads
            .iter()
            .filter(|p| p.pad_type.contains("thru_hole"))
            .count();
        format!(
            "KiCad Footprint {} · {} pads ({} THT) · {}",
            self.name,
            self.pads.len(),
            tht,
            self.layer
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let w = (self.bounds_mm[2] - self.bounds_mm[0]).max(0.0);
        let h = (self.bounds_mm[3] - self.bounds_mm[1]).max(0.0);
        let smd = self.pads.iter().filter(|p| p.pad_type == "smd").count();
        let tht = self.pads.len().saturating_sub(smd);
        StripSummary {
            format: super::super::EdaFormat::KicadFootprint,
            headline: format!("Footprint · {}", self.name),
            specs: format!(
                "{} pads ({smd} SMD / {tht} THT) · {} 3D models · {:.1}×{:.1} mm",
                self.pads.len(),
                self.models_3d.len(),
                w,
                h
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        let enabled: BTreeSet<PcbLayerBucket> = PcbLayerBucket::ALL.into_iter().collect();
        render_pcb_prims(
            &self.primitives,
            self.bounds_mm,
            max_size,
            &enabled,
            cancelled,
        )
    }
}

#[derive(Default)]
struct PcbAccum {
    footprint_count: usize,
    footprints: Vec<PcbFootprintSummary>,
    track_segment_count: usize,
    track_arc_count: usize,
    via_count: usize,
    zone_count: usize,
    track_length_mm: f64,
    primitives: Vec<PcbPrim>,
    edge_bounds: Option<[f64; 4]>,
    all_bounds: Option<[f64; 4]>,
}

impl PcbAccum {
    fn push_prim(&mut self, prim: PcbPrim) {
        prim.extend_bounds(&mut self.all_bounds);
        if prim.layer() == PcbLayerBucket::EdgeCuts {
            prim.extend_bounds(&mut self.edge_bounds);
        }
        if self.primitives.len() < MAX_PCB_PRIMS {
            self.primitives.push(prim);
        }
    }
}

fn collect_pcb_layers(root: &SExp) -> Vec<PcbLayerInfo> {
    let Some(layers) = root.get("layers") else {
        return Vec::new();
    };
    layers
        .children
        .iter()
        .filter_map(|child| {
            let name = child.string_at(0)?.to_string();
            let id = child.name.as_deref().and_then(|s| s.parse::<i64>().ok());
            let kind = child.string_at(1).unwrap_or("signal").to_string();
            Some(PcbLayerInfo { id, name, kind })
        })
        .collect()
}

fn count_stackup_layers(root: &SExp) -> usize {
    root.get("setup")
        .and_then(|s| s.get("stackup"))
        .map_or(0, |st| st.children_named("layer").count())
}

fn collect_nets(root: &SExp) -> Vec<PcbNetInfo> {
    let mut nets = Vec::new();
    for node in root.children_named("net").take(MAX_NET_LIST) {
        if let Some(id) = node.int_at(0) {
            let name = node.string_at(1).unwrap_or("").to_string();
            nets.push(PcbNetInfo { id, name });
        }
    }
    nets
}

fn walk_pcb_children(
    root: &SExp,
    acc: &mut PcbAccum,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    for (idx, child) in root.children.iter().enumerate() {
        if idx.is_multiple_of(1024) {
            check_cancelled(cancelled, "KiCad PCB parse cancelled")?;
        }
        match child.tag() {
            Some("segment") => record_track_segment(child, acc),
            Some("arc") => record_track_arc(child, acc),
            Some("via") => record_via(child, acc),
            Some("zone") => record_zone(child, acc),
            Some("footprint" | "module") => record_footprint(child, acc),
            Some(tag) if tag.starts_with("gr_") => {
                let mut local = Vec::new();
                collect_graphic_node(child, tag, [0.0, 0.0, 0.0], &mut local);
                for prim in local {
                    acc.push_prim(prim);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn xy_child(node: &SExp, tag: &str) -> Option<[f64; 2]> {
    let sub = node.get(tag)?;
    Some([sub.float_at(0)?, sub.float_at(1)?])
}

fn record_track_segment(node: &SExp, acc: &mut PcbAccum) {
    let (Some(a), Some(b)) = (xy_child(node, "start"), xy_child(node, "end")) else {
        return;
    };
    let width = node.child_f64("width").unwrap_or(0.25).max(0.05);
    let layer = PcbLayerBucket::from_layer_name(node.child_str("layer").unwrap_or("F.Cu"));
    acc.track_segment_count += 1;
    acc.track_length_mm += (b[0] - a[0]).hypot(b[1] - a[1]);
    acc.push_prim(PcbPrim::Segment { layer, a, b, width });
}

fn record_track_arc(node: &SExp, acc: &mut PcbAccum) {
    let (Some(start), Some(mid), Some(end)) = (
        xy_child(node, "start"),
        xy_child(node, "mid"),
        xy_child(node, "end"),
    ) else {
        return;
    };
    let width = node.child_f64("width").unwrap_or(0.25).max(0.05);
    let layer = PcbLayerBucket::from_layer_name(node.child_str("layer").unwrap_or("F.Cu"));
    acc.track_arc_count += 1;

    if let Some((center, radius, start_rad, end_rad)) = arc_from_three_points(start, mid, end) {
        acc.track_length_mm += radius * (end_rad - start_rad).abs();
        acc.push_prim(PcbPrim::Arc {
            layer,
            center,
            radius,
            start_rad,
            end_rad,
            width,
        });
    } else {
        acc.track_length_mm += (end[0] - start[0]).hypot(end[1] - start[1]);
        acc.push_prim(PcbPrim::Segment {
            layer,
            a: start,
            b: end,
            width,
        });
    }
}

fn record_via(node: &SExp, acc: &mut PcbAccum) {
    let Some((x, y, _)) = node.at() else {
        return;
    };
    acc.via_count += 1;
    let size = node.child_f64("size").unwrap_or(0.6).max(0.2);
    let drill = node.child_f64("drill").unwrap_or(0.3).max(0.1);
    acc.push_prim(PcbPrim::Circle {
        layer: PcbLayerBucket::FrontCopper,
        center: [x, y],
        radius: size * 0.5,
        fill: true,
        stroke: 0.0,
    });
    acc.push_prim(PcbPrim::Circle {
        layer: PcbLayerBucket::Drills,
        center: [x, y],
        radius: drill * 0.5,
        fill: true,
        stroke: 0.0,
    });
}

fn record_zone(node: &SExp, acc: &mut PcbAccum) {
    acc.zone_count += 1;
    for fp in node.children_named("filled_polygon").take(8) {
        if let Some(pts_node) = fp.get("pts") {
            let pts: Vec<[f64; 2]> = pts_node
                .children_named("xy")
                .filter_map(|p| Some([p.float_at(0)?, p.float_at(1)?]))
                .collect();
            if pts.len() >= 3 {
                acc.push_prim(PcbPrim::Polygon {
                    layer: PcbLayerBucket::Zones,
                    points: pts,
                    fill: true,
                    stroke: 0.0,
                });
            }
        }
    }
}

fn record_footprint(node: &SExp, acc: &mut PcbAccum) {
    acc.footprint_count += 1;
    let (x, y, rot) = node.at().unwrap_or((0.0, 0.0, 0.0));
    let xf = [x, y, rot];
    let lib_id = node.string_at(0).unwrap_or("").to_string();
    let layer = node.child_str("layer").unwrap_or("F.Cu").to_string();
    let (ref_des, value) = footprint_ref_and_value(node);

    let mut pad_count = 0_usize;
    let mut local_prims = Vec::new();
    for child in &node.children {
        match child.tag() {
            Some("pad") => {
                pad_count += 1;
                let _ = collect_pad(child, xf, &mut local_prims);
            }
            Some(tag) if tag.starts_with("fp_") => {
                collect_graphic_node(child, tag, xf, &mut local_prims);
            }
            _ => {}
        }
    }
    for prim in local_prims {
        acc.push_prim(prim);
    }
    if acc.footprints.len() < MAX_FOOTPRINT_LIST {
        acc.footprints.push(PcbFootprintSummary {
            ref_des,
            value,
            lib_id,
            layer,
            pos_mm: [x, y],
            rot_deg: rot,
            pad_count,
        });
    }
}

fn footprint_ref_and_value(node: &SExp) -> (String, String) {
    if let Some(ref_val) = node.property("Reference") {
        let value = node.property("Value").unwrap_or("").to_string();
        return (ref_val.to_string(), value);
    }
    let mut ref_des = String::new();
    let mut value = String::new();
    for fp_text in node.children_named("fp_text") {
        match fp_text.string_at(0) {
            Some("reference") => ref_des = fp_text.string_at(1).unwrap_or("").to_string(),
            Some("value") => value = fp_text.string_at(1).unwrap_or("").to_string(),
            _ => {}
        }
    }
    (ref_des, value)
}

fn transform_point(xf: [f64; 3], local: [f64; 2]) -> [f64; 2] {
    if xf[2] == 0.0 {
        return [xf[0] + local[0], xf[1] + local[1]];
    }
    let rad = (-xf[2]).to_radians();
    let (s, c) = rad.sin_cos();
    [
        xf[0] + local[0] * c - local[1] * s,
        xf[1] + local[0] * s + local[1] * c,
    ]
}

fn pad_layer_bucket(layers: &[&str]) -> PcbLayerBucket {
    let has_f = layers.iter().any(|l| *l == "F.Cu" || *l == "*.Cu");
    let has_b = layers.iter().any(|l| *l == "B.Cu" || *l == "*.Cu");
    if has_f || (!has_f && !has_b) {
        PcbLayerBucket::FrontCopper
    } else {
        PcbLayerBucket::BackCopper
    }
}

fn parse_pad_drill(node: &SExp) -> Option<[f64; 2]> {
    let drill = node.get("drill")?;
    if drill.string_at(0) == Some("oval") {
        let dx = drill.float_at(1)?;
        let dy = drill.float_at(2).unwrap_or(dx);
        Some([dx, dy])
    } else {
        let dx = drill.float_at(0)?;
        let dy = drill.float_at(1).unwrap_or(dx);
        Some([dx, dy])
    }
}

fn collect_pad(node: &SExp, fp_xf: [f64; 3], out: &mut Vec<PcbPrim>) -> Option<PcbPadInfo> {
    let number = node.string_at(0).unwrap_or("").to_string();
    let pad_type = node.string_at(1).unwrap_or("smd").to_string();
    let shape = node.string_at(2).unwrap_or("rect").to_string();
    let (lx, ly, rot_deg) = node.at().unwrap_or((0.0, 0.0, 0.0));
    // KiCad stores the pad center in footprint coordinates and its angle
    // in board coordinates. The standalone footprint has a zero frame angle.
    let pos_mm = transform_point(fp_xf, [lx, ly]);
    let size = xy_child(node, "size").unwrap_or([1.0, 1.0]);
    let drill = parse_pad_drill(node);
    let layers_vec: Vec<&str> = node
        .get("layers")
        .map(|l| l.atoms().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if pad_type != "np_thru_hole" {
        let bucket = pad_layer_bucket(&layers_vec);
        if shape == "circle" {
            out.push(PcbPrim::Circle {
                layer: bucket,
                center: pos_mm,
                radius: size[0].max(size[1]) * 0.5,
                fill: true,
                stroke: 0.0,
            });
        } else {
            out.push(PcbPrim::Rect {
                layer: bucket,
                center: pos_mm,
                size,
                rot_deg,
                fill: true,
                stroke: 0.0,
            });
        }
    }
    if let Some([dx, dy]) = drill {
        out.push(PcbPrim::Circle {
            layer: PcbLayerBucket::Drills,
            center: pos_mm,
            radius: dx.max(dy) * 0.5,
            fill: true,
            stroke: 0.0,
        });
    }

    Some(PcbPadInfo {
        number,
        pad_type,
        shape,
        pos_mm,
        size_mm: size,
        drill_mm: drill,
        layers: layers_vec.join(", "),
    })
}

fn graphic_stroke_width(node: &SExp) -> f64 {
    node.get("stroke")
        .and_then(|s| s.child_f64("width"))
        .or_else(|| node.child_f64("width"))
        .unwrap_or(0.12)
        .max(0.05)
}

fn collect_graphic_node(node: &SExp, tag: &str, xf: [f64; 3], out: &mut Vec<PcbPrim>) {
    let layer = PcbLayerBucket::from_layer_name(node.child_str("layer").unwrap_or("F.SilkS"));
    let width = graphic_stroke_width(node);
    let kind = tag.trim_start_matches("gr_").trim_start_matches("fp_");

    match kind {
        "line" => {
            if let (Some(a), Some(b)) = (xy_child(node, "start"), xy_child(node, "end")) {
                out.push(PcbPrim::Segment {
                    layer,
                    a: transform_point(xf, a),
                    b: transform_point(xf, b),
                    width,
                });
            }
        }
        "rect" => {
            if let (Some(a), Some(b)) = (xy_child(node, "start"), xy_child(node, "end")) {
                let p0 = transform_point(xf, a);
                let p1 = transform_point(xf, [b[0], a[1]]);
                let p2 = transform_point(xf, b);
                let p3 = transform_point(xf, [a[0], b[1]]);
                out.push(PcbPrim::Polygon {
                    layer,
                    points: vec![p0, p1, p2, p3],
                    fill: matches!(node.child_str("fill"), Some("solid" | "yes")),
                    stroke: width,
                });
            }
        }
        "circle" => {
            if let (Some(c), Some(e)) = (xy_child(node, "center"), xy_child(node, "end")) {
                let radius = (e[0] - c[0]).hypot(e[1] - c[1]);
                out.push(PcbPrim::Circle {
                    layer,
                    center: transform_point(xf, c),
                    radius,
                    fill: matches!(node.child_str("fill"), Some("solid" | "yes")),
                    stroke: width,
                });
            }
        }
        "arc" => {
            if let (Some(a), Some(m), Some(b)) = (
                xy_child(node, "start"),
                xy_child(node, "mid"),
                xy_child(node, "end"),
            ) {
                let pa = transform_point(xf, a);
                let pm = transform_point(xf, m);
                let pb = transform_point(xf, b);
                if let Some((center, radius, start_rad, end_rad)) =
                    arc_from_three_points(pa, pm, pb)
                {
                    out.push(PcbPrim::Arc {
                        layer,
                        center,
                        radius,
                        start_rad,
                        end_rad,
                        width,
                    });
                }
            }
        }
        "poly" => {
            if let Some(pts_node) = node.get("pts") {
                let pts: Vec<[f64; 2]> = pts_node
                    .children_named("xy")
                    .filter_map(|p| Some(transform_point(xf, [p.float_at(0)?, p.float_at(1)?])))
                    .collect();
                if pts.len() >= 3 {
                    out.push(PcbPrim::Polygon {
                        layer,
                        points: pts,
                        fill: !matches!(node.child_str("fill"), Some("none")),
                        stroke: width,
                    });
                }
            }
        }
        _ => {}
    }
}

/// Computes circle center, radius, start angle, and end angle preserving passage through `mid`.
pub(crate) fn arc_from_three_points(
    a: [f64; 2],
    m: [f64; 2],
    b: [f64; 2],
) -> Option<([f64; 2], f64, f64, f64)> {
    let d = 2.0 * (a[0] * (m[1] - b[1]) + m[0] * (b[1] - a[1]) + b[0] * (a[1] - m[1]));
    if d.abs() < 1e-9 || !d.is_finite() {
        return None;
    }
    let a2 = a[0] * a[0] + a[1] * a[1];
    let m2 = m[0] * m[0] + m[1] * m[1];
    let b2 = b[0] * b[0] + b[1] * b[1];
    let cx = (a2 * (m[1] - b[1]) + m2 * (b[1] - a[1]) + b2 * (a[1] - m[1])) / d;
    let cy = (a2 * (b[0] - m[0]) + m2 * (a[0] - b[0]) + b2 * (m[0] - a[0])) / d;
    let r = (a[0] - cx).hypot(a[1] - cy);
    if !r.is_finite() || r < 1e-6 {
        return None;
    }
    let sa = (a[1] - cy).atan2(a[0] - cx);
    let sm = (m[1] - cy).atan2(m[0] - cx);
    let sb = (b[1] - cy).atan2(b[0] - cx);
    let ccw_end = if sb >= sa { sb } else { sb + 2.0 * PI };
    let sm_ccw = if sm >= sa { sm } else { sm + 2.0 * PI };
    let end_rad = if sm_ccw <= ccw_end {
        ccw_end
    } else {
        ccw_end - 2.0 * PI
    };
    Some(([cx, cy], r, sa, end_rad))
}

fn render_pcb_prims(
    prims: &[PcbPrim],
    bounds: [f64; 4],
    max_size: [u32; 2],
    visible: &BTreeSet<PcbLayerBucket>,
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    check_cancelled(cancelled, "KiCad PCB render cancelled")?;
    let bg = Color::from_rgba8(PCB_BG[0], PCB_BG[1], PCB_BG[2], PCB_BG[3]);
    let (mut pixmap, transform, scale) = allocate_canvas_with_y_flip(bounds, max_size, bg, true)
        .ok_or_else(|| "Could not allocate KiCad PCB canvas".to_string())?;

    let min_stroke = f64::from(0.9 / scale.max(0.01));
    for &bucket in &PcbLayerBucket::ALL {
        if !visible.contains(&bucket) {
            continue;
        }
        let rgba = bucket.rgba();
        for (idx, prim) in prims.iter().enumerate() {
            if idx.is_multiple_of(2048) {
                check_cancelled(cancelled, "KiCad PCB render cancelled")?;
            }
            if prim.layer() == bucket {
                draw_pcb_prim(&mut pixmap, prim, rgba, transform, min_stroke);
            }
        }
    }
    Ok(pixmap_to_thumbnail(pixmap))
}

fn draw_pcb_prim(
    pixmap: &mut Pixmap,
    prim: &PcbPrim,
    rgba: [u8; 4],
    transform: Transform,
    min_stroke: f64,
) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = true;

    match prim {
        PcbPrim::Segment { a, b, width, .. } => {
            let mut pb = PathBuilder::new();
            pb.move_to(a[0] as f32, a[1] as f32);
            pb.line_to(b[0] as f32, b[1] as f32);
            if let Some(path) = pb.finish() {
                let stroke = pcb_stroke((*width).max(min_stroke));
                pixmap.stroke_path(&path, &paint, &stroke, transform, None);
            }
        }
        PcbPrim::Circle {
            center,
            radius,
            fill,
            stroke,
            ..
        } => {
            let mut pb = PathBuilder::new();
            pb.push_circle(
                center[0] as f32,
                center[1] as f32,
                radius.abs().max(min_stroke * 0.4) as f32,
            );
            if let Some(path) = pb.finish() {
                if *fill {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                }
                if *stroke > 0.0 || !fill {
                    let s = pcb_stroke((*stroke).max(min_stroke));
                    pixmap.stroke_path(&path, &paint, &s, transform, None);
                }
            }
        }
        PcbPrim::Rect {
            center,
            size,
            rot_deg,
            fill,
            stroke,
            ..
        } => {
            let hw = size[0] * 0.5;
            let hh = size[1] * 0.5;
            let xf = [center[0], center[1], *rot_deg];
            let pts = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(|p| transform_point(xf, p));
            fill_or_stroke_poly(pixmap, &pts, *fill, *stroke, min_stroke, &paint, transform);
        }
        PcbPrim::Polygon {
            points,
            fill,
            stroke,
            ..
        } => fill_or_stroke_poly(
            pixmap, points, *fill, *stroke, min_stroke, &paint, transform,
        ),
        PcbPrim::Arc {
            center,
            radius,
            start_rad,
            end_rad,
            width,
            ..
        } => {
            let mut pb = PathBuilder::new();
            append_arc_points(
                &mut pb, center[0], center[1], *radius, *start_rad, *end_rad, true,
            );
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &pcb_stroke((*width).max(min_stroke)),
                    transform,
                    None,
                );
            }
        }
    }
}

fn fill_or_stroke_poly(
    pixmap: &mut Pixmap,
    pts: &[[f64; 2]],
    fill: bool,
    stroke: f64,
    min_stroke: f64,
    paint: &Paint<'_>,
    transform: Transform,
) {
    if pts.len() < 3 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0][0] as f32, pts[0][1] as f32);
    for p in &pts[1..] {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        if fill {
            pixmap.fill_path(&path, paint, FillRule::Winding, transform, None);
        }
        if stroke > 0.0 || !fill {
            pixmap.stroke_path(
                &path,
                paint,
                &pcb_stroke(stroke.max(min_stroke)),
                transform,
                None,
            );
        }
    }
}

fn pcb_stroke(width: f64) -> Stroke {
    Stroke {
        width: width.max(0.01) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_pad_center_and_angle_use_their_respective_coordinate_frames() {
        let pcb = br#"(kicad_pcb
            (footprint "Test:Pad" (at 10 20 90)
                (pad "1" smd rect (at 2 0 90) (size 3 1) (layers "F.Cu"))
                (pad "2" smd rect (at -2 0) (size 3 1) (layers "F.Cu"))))"#;
        let doc = KicadPcbDocument::parse(pcb, &AtomicBool::new(false)).unwrap();
        for (prim, expected) in doc
            .primitives
            .iter()
            .zip([([10.0, 18.0], 90.0), ([10.0, 22.0], 0.0)])
        {
            let PcbPrim::Rect {
                center, rot_deg, ..
            } = prim
            else {
                panic!("expected a rectangular pad");
            };
            assert_eq!((*center, *rot_deg), expected);
        }
        assert_eq!(doc.primitives.len(), 2);
    }

    #[test]
    fn kicad_pcb_and_footprint_extract_metrics_and_render() {
        let pcb = r#"(kicad_pcb
          (version 20240108)
          (generator "pcbnew")
          (general (thickness 1.6))
          (layers
            (0 "F.Cu" signal)
            (1 "In1.Cu" power)
            (31 "B.Cu" signal)
            (44 "Edge.Cuts" user)
          )
          (setup (stackup
            (layer "F.Cu" (type "copper"))
            (layer "dielectric 1" (type "prepreg"))
            (layer "In1.Cu" (type "copper"))
            (layer "B.Cu" (type "copper"))
          ))
          (net 0 "")
          (net 1 "VCC")
          (net 2 "GND")
          (footprint "Resistor_SMD:R_0603" (layer "F.Cu") (at 10 10 90)
            (property "Reference" "R1")
            (property "Value" "10k")
            (pad "1" smd roundrect (at -0.8 0) (size 0.9 0.9) (layers "F.Cu" "F.Mask"))
            (pad "2" thru_hole circle (at 0.8 0) (size 1.4 1.4) (drill 0.8) (layers "*.Cu"))
            (fp_line (start -1.2 -0.6) (end 1.2 0.6) (layer "F.SilkS") (width 0.12))
          )
          (gr_rect (start 0 0) (end 50 30) (layer "Edge.Cuts") (width 0.15))
          (segment (start 10 10) (end 20 10) (width 0.25) (layer "F.Cu") (net 1))
          (arc (start 20 10) (mid 25 12) (end 30 10) (width 0.25) (layer "F.Cu") (net 1))
          (via (at 30 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1))
        )"#;
        let cancel = AtomicBool::new(false);
        let pcb_doc = KicadPcbDocument::parse(pcb.as_bytes(), &cancel).unwrap();
        assert_eq!(pcb_doc.copper_layer_count, 3);
        assert_eq!(pcb_doc.stackup_layer_count, 4);
        assert_eq!(pcb_doc.footprint_count, 1);
        assert_eq!(pcb_doc.named_net_count, 2);
        assert_eq!(pcb_doc.track_segment_count, 1);
        assert_eq!(pcb_doc.track_arc_count, 1);
        assert_eq!(pcb_doc.via_count, 1);
        assert!(pcb_doc.track_length_mm > 20.0);
        let outline = pcb_doc.outline_mm.unwrap();
        assert!((outline[0] - 50.0).abs() < 0.01 && (outline[1] - 30.0).abs() < 0.01);

        let pcb_thumb = pcb_doc.render_thumbnail([160, 120], &cancel).unwrap();
        assert!(
            pcb_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [PCB_BG[0], PCB_BG[1], PCB_BG[2]])
        );

        let fp = r#"(footprint "Connector_PinHeader:PinHeader_1x02"
          (layer "F.Cu")
          (descr "2-pin header")
          (attr through_hole)
          (pad "1" thru_hole rect (at 0 0) (size 1.7 1.7) (drill 1.0) (layers "*.Cu" "*.Mask"))
          (pad "2" thru_hole circle (at 0 2.54) (size 1.7 1.7) (drill 1.0) (layers "*.Cu" "*.Mask"))
          (fp_line (start -1.3 -1.3) (end 1.3 3.84) (layer "F.SilkS") (width 0.12))
          (model "${KICAD8_3DMODEL_DIR}/Connector_PinHeader_2.54mm.3dshapes/PinHeader_1x02.step")
        )"#;
        let fp_doc = KicadFootprintDocument::parse(fp.as_bytes(), &cancel).unwrap();
        assert_eq!(fp_doc.pads.len(), 2);
        assert_eq!(fp_doc.models_3d.len(), 1);
        let fp_thumb = fp_doc.render_thumbnail([120, 120], &cancel).unwrap();
        assert!(
            fp_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [PCB_BG[0], PCB_BG[1], PCB_BG[2]])
        );
    }
}
