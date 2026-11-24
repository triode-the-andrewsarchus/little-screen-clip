//! KiCad `.kicad_sch` schematics and `.kicad_sym` symbol libraries via `eda_view`.

use super::super::{
    StripSummary, allocate_canvas_with_y_flip, bounded_source_text, check_cancelled,
    pixmap_to_thumbnail, update_bounds,
};
use super::scene_render::render_scene_thumbnail;
use crate::ImageThumbnail;
use eda_view::kicad::schema::library::{SymbolLibrary, parse_unit_index, resolve_extends};
use eda_view::kicad::schema::schematic::Schematic;
use eda_view::kicad::sexp::{SExp, parse as parse_sexp};
use eda_view::schematic::{self, Palette, symbol_scene_for_unit};
use eda_view::vector::Scene;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

const SCHEMATIC_BG: [u8; 4] = [20, 24, 30, 255];
const MAX_SCHEMATIC_LIST: usize = 2048;
const MAX_SYMBOL_LIB_ENTRIES: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SchematicLabelKind {
    Local,
    Global,
    Hierarchical,
}

impl SchematicLabelKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Global => "Global",
            Self::Hierarchical => "Hierarchical",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SchematicLabelInfo {
    pub name: String,
    pub kind: SchematicLabelKind,
    pub shape: String,
    pub occurrences: usize,
}

#[derive(Clone, Debug)]
pub struct SchematicSymbolInfo {
    pub reference: String,
    pub value: String,
    pub lib_id: String,
    pub footprint: String,
    pub pos_mm: [f64; 2],
    pub is_power: bool,
}

#[derive(Clone, Debug)]
pub struct SchematicSheetInfo {
    pub name: String,
    pub filename: String,
}

/// Decoded `.kicad_sch` schematic document.
#[derive(Clone, Debug)]
pub struct KicadSchematicDocument {
    pub sheet_title: String,
    pub revision: String,
    pub date: String,
    pub company: String,
    pub paper: String,
    pub version: Option<i64>,
    pub generator: Option<String>,
    pub symbol_count: usize,
    pub component_count: usize,
    pub power_symbol_count: usize,
    pub wire_count: usize,
    pub bus_count: usize,
    pub junction_count: usize,
    pub no_connect_count: usize,
    pub sheet_count: usize,
    pub net_labels: Vec<SchematicLabelInfo>,
    pub symbols: Vec<SchematicSymbolInfo>,
    pub sheets: Vec<SchematicSheetInfo>,
    pub warnings: Vec<String>,
    pub scene: Option<Arc<Scene>>,
    pub fallback_wires: Vec<([f64; 2], [f64; 2])>,
    pub bounds_mm: [f64; 4],
    pub source_excerpt: String,
}

impl KicadSchematicDocument {
    pub fn parse(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "KiCad schematic parse cancelled")?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "KiCad schematic file is not valid UTF-8".to_string())?;
        let sexp =
            parse_sexp(text).map_err(|e| format!("Invalid KiCad schematic S-expression: {e}"))?;
        if !sexp.has_tag("kicad_sch") {
            return Err("Expected root (kicad_sch ...) form".into());
        }
        let bus_count =
            sexp.children_named("bus").count() + sexp.children_named("bus_entry").count();
        let sch = Schematic::new(sexp, None)
            .map_err(|e| format!("Failed to read KiCad schematic schema: {e}"))?;
        check_cancelled(cancelled, "KiCad schematic parse cancelled")?;

        let tb = sch.title_block();
        let fallback_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Schematic");
        let sheet_title = if tb.title.trim().is_empty() {
            fallback_name.to_string()
        } else {
            tb.title
        };

        let (symbols, component_count, power_symbol_count) = collect_schematic_symbols(&sch);
        let net_labels = collect_schematic_labels(&sch);
        let sheets: Vec<SchematicSheetInfo> = sch
            .sheets()
            .iter()
            .take(MAX_SCHEMATIC_LIST)
            .map(|s| SchematicSheetInfo {
                name: s.name.clone(),
                filename: s.filename.clone(),
            })
            .collect();

        let fallback_wires: Vec<([f64; 2], [f64; 2])> = sch
            .wires()
            .iter()
            .take(16_384)
            .map(|w| ([w.start.0, w.start.1], [w.end.0, w.end.1]))
            .collect();

        let (scene, mut warnings) = match schematic::parse(text) {
            Ok(sc) => {
                let w = eda_view::vector::schematic::warnings(&sc)
                    .into_iter()
                    .take(16)
                    .map(str::to_owned)
                    .collect();
                (Some(Arc::new(sc)), w)
            }
            Err(err) => (None, vec![format!("Vector fallback active: {err}")]),
        };
        if warnings.is_empty() && !sheets.is_empty() {
            warnings.push("Subsheet links are listed as metadata only".into());
        }

        let bounds_mm = scene
            .as_deref()
            .filter(|sc| !sc.bbox.is_empty())
            .map(|sc| {
                [
                    sc.bbox.min[0],
                    sc.bbox.min[1],
                    sc.bbox.max[0],
                    sc.bbox.max[1],
                ]
            })
            .unwrap_or_else(|| compute_wire_bounds(&fallback_wires, &symbols));

        Ok(Self {
            sheet_title,
            revision: tb.rev,
            date: tb.date,
            company: tb.company,
            paper: sch.paper().unwrap_or_else(|| "A4".into()),
            version: sch.version(),
            generator: sch.generator(),
            symbol_count: sch.symbols().len(),
            component_count,
            power_symbol_count,
            wire_count: sch.wires().len(),
            bus_count,
            junction_count: sch.junctions().len(),
            no_connect_count: sch.no_connects().len(),
            sheet_count: sch.sheets().len(),
            net_labels,
            symbols,
            sheets,
            warnings,
            scene,
            fallback_wires,
            bounds_mm,
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        format!(
            "KiCad Schematic {} · {} symbols · {} wires · {} buses · {} net labels",
            self.sheet_title,
            self.symbol_count,
            self.wire_count,
            self.bus_count,
            self.net_labels.len()
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        StripSummary {
            format: super::super::EdaFormat::KicadSchematic,
            headline: format!("Schematic · {}", self.sheet_title),
            specs: format!(
                "{} symbols ({} components) · {} wires · {} buses · {} net labels",
                self.symbol_count,
                self.component_count,
                self.wire_count,
                self.bus_count,
                self.net_labels.len()
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        let bg = Color::from_rgba8(
            SCHEMATIC_BG[0],
            SCHEMATIC_BG[1],
            SCHEMATIC_BG[2],
            SCHEMATIC_BG[3],
        );
        if let Some(scene) = &self.scene
            && !scene.items.is_empty()
        {
            return render_scene_thumbnail(scene, max_size, bg, cancelled);
        }
        render_schematic_fallback(self, max_size, bg, cancelled)
    }
}

#[derive(Clone, Debug)]
pub struct KicadSymbolPinInfo {
    pub number: String,
    pub name: String,
    pub pin_type: String,
    pub unit: i64,
}

#[derive(Clone, Debug)]
pub struct KicadSymbolEntry {
    pub name: String,
    pub extends: Option<String>,
    pub units: Vec<i64>,
    pub reference_prefix: String,
    pub value: String,
    pub footprint: String,
    pub description: String,
    pub pins: Vec<KicadSymbolPinInfo>,
    pub properties: Vec<(String, String)>,
    geometry: Arc<SExp>,
}

/// Decoded `.kicad_sym` symbol library document.
#[derive(Clone, Debug)]
pub struct KicadSymbolLibDocument {
    pub version: Option<i64>,
    pub generator: Option<String>,
    pub symbols: Vec<KicadSymbolEntry>,
    pub source_excerpt: String,
}

impl KicadSymbolLibDocument {
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "KiCad symbol library parse cancelled")?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "KiCad symbol library is not valid UTF-8".to_string())?;
        let mut lib = SymbolLibrary::load_from_string(text)
            .map_err(|e| format!("Failed to load KiCad symbol library: {e}"))?;
        let root = lib
            .sexp
            .take()
            .ok_or("Missing KiCad symbol library source")?;
        let version = root.get("version").and_then(|v| v.int_at(0));
        let generator = root.child_str("generator").map(str::to_owned);
        let definitions: BTreeMap<String, Arc<SExp>> = root
            .children
            .into_iter()
            .filter(|node| node.has_tag("symbol"))
            .filter_map(|node| Some((node.string_at(0)?.to_string(), Arc::new(node))))
            .collect();
        resolve_extends(&mut lib.symbols)
            .map_err(|e| format!("Invalid symbol inheritance: {e}"))?;

        let mut symbols = Vec::new();
        for (idx, (_, resolved)) in lib.symbols.iter().enumerate() {
            if symbols.len() >= MAX_SYMBOL_LIB_ENTRIES {
                break;
            }
            if idx.is_multiple_of(128) {
                check_cancelled(cancelled, "KiCad symbol library parse cancelled")?;
            }
            let Some(definition) = definitions.get(&resolved.name) else {
                continue;
            };
            let geometry = symbol_geometry(&definitions, definition);
            let name = geometry.string_at(0).unwrap_or_default();
            let short_name = name.rsplit(':').next().unwrap_or(name);
            let units: BTreeSet<i64> = geometry
                .children_named("symbol")
                .filter_map(|node| parse_unit_index(node.string_at(0)?, short_name))
                .filter(|&unit| unit > 0)
                .collect();
            let pins = resolved
                .pins
                .iter()
                .map(|p| KicadSymbolPinInfo {
                    number: p.number.clone(),
                    name: p.name.clone(),
                    pin_type: p.pin_type.clone(),
                    unit: p.unit,
                })
                .collect();
            let properties: Vec<(String, String)> = resolved
                .properties
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect();
            let prop = |key: &str| resolved.properties.get(key).cloned().unwrap_or_default();
            symbols.push(KicadSymbolEntry {
                name: resolved.name.clone(),
                extends: resolved.extends.clone(),
                units: if units.is_empty() {
                    vec![1]
                } else {
                    units.into_iter().collect()
                },
                reference_prefix: prop("Reference"),
                value: prop("Value"),
                footprint: prop("Footprint"),
                description: prop("Description"),
                pins,
                properties,
                geometry: Arc::clone(geometry),
            });
        }

        Ok(Self {
            version,
            generator,
            symbols,
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        let total_pins: usize = self.symbols.iter().map(|s| s.pins.len()).sum();
        if let Some(first) = self.symbols.first()
            && self.symbols.len() == 1
        {
            return format!(
                "KiCad Symbol {} · {} pins · {} units",
                first.name,
                first.pins.len(),
                first.units.len()
            );
        }
        format!(
            "KiCad Symbol Library · {} symbols · {} total pins",
            self.symbols.len(),
            total_pins
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let total_pins: usize = self.symbols.iter().map(|s| s.pins.len()).sum();
        let extended = self.symbols.iter().filter(|s| s.extends.is_some()).count();
        StripSummary {
            format: super::super::EdaFormat::KicadSymbolLib,
            headline: format!("Symbol Library · {} symbols", self.symbols.len()),
            specs: format!("{total_pins} pins · {extended} derived (extends)"),
            detail: self.summary_line(),
        }
    }

    /// Renders a chosen symbol & 1-indexed unit using `eda_view::schematic::symbol_scene_for_unit`.
    pub fn render_symbol_thumbnail(
        &self,
        symbol_idx: usize,
        unit: i64,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        let entry = self
            .symbols
            .get(symbol_idx)
            .ok_or_else(|| "Symbol index out of range".to_string())?;
        let palette = Palette::default();
        let scene = symbol_scene_for_unit(&entry.geometry, unit, 1, &palette);
        let bg = Color::from_rgba8(
            SCHEMATIC_BG[0],
            SCHEMATIC_BG[1],
            SCHEMATIC_BG[2],
            SCHEMATIC_BG[3],
        );
        render_scene_thumbnail(&scene, max_size, bg, cancelled)
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "KiCad symbol render cancelled")?;
        if self.symbols.is_empty() {
            let pix = Pixmap::new(max_size[0].max(64), max_size[1].max(64))
                .ok_or_else(|| "Could not allocate symbol preview".to_string())?;
            return Ok(pixmap_to_thumbnail(pix));
        }
        if self.symbols.len() == 1 {
            return self.render_symbol_thumbnail(0, self.symbols[0].units[0], max_size, cancelled);
        }
        render_symbol_grid(self, max_size, cancelled)
    }
}

fn symbol_geometry<'a>(
    definitions: &'a BTreeMap<String, Arc<SExp>>,
    definition: &'a Arc<SExp>,
) -> &'a Arc<SExp> {
    let mut node = definition;
    for _ in 0..10 {
        if node.children_named("symbol").next().is_some() {
            return node;
        }
        let Some(base) = node.child_str("extends") else {
            break;
        };
        let Some(parent) = definitions.get(base).or_else(|| {
            definitions
                .iter()
                .find(|(name, _)| name.rsplit(':').next() == Some(base))
                .map(|(_, node)| node)
        }) else {
            break;
        };
        node = parent;
    }
    node
}

fn collect_schematic_symbols(sch: &Schematic) -> (Vec<SchematicSymbolInfo>, usize, usize) {
    let mut out = Vec::new();
    let mut components = 0_usize;
    let mut power = 0_usize;
    for sym in sch.symbols() {
        let is_power = sym.lib_id.starts_with("power:");
        if is_power {
            power += 1;
        } else {
            components += 1;
        }
        if out.len() < MAX_SCHEMATIC_LIST {
            out.push(SchematicSymbolInfo {
                reference: sym.reference().to_string(),
                value: sym.value().to_string(),
                lib_id: sym.lib_id.clone(),
                footprint: sym.footprint().to_string(),
                pos_mm: [sym.position.0, sym.position.1],
                is_power,
            });
        }
    }
    (out, components, power)
}

fn collect_schematic_labels(sch: &Schematic) -> Vec<SchematicLabelInfo> {
    let mut map: BTreeMap<(SchematicLabelKind, String), (String, usize)> = BTreeMap::new();
    for l in sch.labels() {
        let entry = map
            .entry((SchematicLabelKind::Local, l.text.clone()))
            .or_insert_with(|| (String::new(), 0));
        entry.1 += 1;
    }
    for g in sch.global_labels() {
        let entry = map
            .entry((SchematicLabelKind::Global, g.text))
            .or_insert_with(|| (g.shape, 0));
        entry.1 += 1;
    }
    for h in sch.hierarchical_labels() {
        let entry = map
            .entry((SchematicLabelKind::Hierarchical, h.text.clone()))
            .or_insert_with(|| (h.shape.clone(), 0));
        entry.1 += 1;
    }
    map.into_iter()
        .map(|((kind, name), (shape, occurrences))| SchematicLabelInfo {
            name,
            kind,
            shape,
            occurrences,
        })
        .collect()
}

fn compute_wire_bounds(
    wires: &[([f64; 2], [f64; 2])],
    symbols: &[SchematicSymbolInfo],
) -> [f64; 4] {
    let mut bounds = None;
    for &(a, b) in wires {
        update_bounds(&mut bounds, a[0], a[1]);
        update_bounds(&mut bounds, b[0], b[1]);
    }
    for s in symbols {
        update_bounds(&mut bounds, s.pos_mm[0] - 5.0, s.pos_mm[1] - 5.0);
        update_bounds(&mut bounds, s.pos_mm[0] + 5.0, s.pos_mm[1] + 5.0);
    }
    bounds.unwrap_or([0.0, 0.0, 297.0, 210.0])
}

fn render_schematic_fallback(
    doc: &KicadSchematicDocument,
    max_size: [u32; 2],
    bg: Color,
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    check_cancelled(cancelled, "KiCad schematic render cancelled")?;
    let (mut pixmap, transform, scale) =
        allocate_canvas_with_y_flip(doc.bounds_mm, max_size, bg, true)
            .ok_or_else(|| "Could not allocate schematic fallback canvas".to_string())?;
    let mut wire_paint = Paint::default();
    wire_paint.set_color_rgba8(158, 206, 106, 255);
    wire_paint.anti_alias = true;
    let stroke = Stroke {
        width: (1.2 / scale.max(0.01)).max(0.15),
        ..Stroke::default()
    };
    let mut pb = PathBuilder::new();
    for &(a, b) in &doc.fallback_wires {
        pb.move_to(a[0] as f32, a[1] as f32);
        pb.line_to(b[0] as f32, b[1] as f32);
    }
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &wire_paint, &stroke, transform, None);
    }
    let mut sym_paint = Paint::default();
    sym_paint.set_color_rgba8(122, 162, 247, 220);
    let r = (3.5 / scale.max(0.01)).max(1.2);
    for sym in &doc.symbols {
        let mut spb = PathBuilder::new();
        if let Some(rect) = tiny_skia::Rect::from_xywh(
            sym.pos_mm[0] as f32 - r,
            sym.pos_mm[1] as f32 - r,
            r * 2.0,
            r * 2.0,
        ) {
            spb.push_rect(rect);
        }
        if let Some(path) = spb.finish() {
            pixmap.stroke_path(&path, &sym_paint, &stroke, transform, None);
        }
    }
    Ok(pixmap_to_thumbnail(pixmap))
}

fn render_symbol_grid(
    lib: &KicadSymbolLibDocument,
    max_size: [u32; 2],
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    let w = max_size[0].clamp(64, 1024);
    let h = max_size[1].clamp(64, 1024);
    let cell = [(w / 2).max(32), (h / 2).max(32)];
    let mut canvas = Pixmap::new(cell[0] * 2, cell[1] * 2)
        .ok_or_else(|| "Could not allocate symbol library canvas".to_string())?;
    canvas.fill(Color::from_rgba8(
        SCHEMATIC_BG[0],
        SCHEMATIC_BG[1],
        SCHEMATIC_BG[2],
        SCHEMATIC_BG[3],
    ));

    let count = lib.symbols.len().min(4);
    for idx in 0..count {
        if let Ok(tile) =
            lib.render_symbol_thumbnail(idx, lib.symbols[idx].units[0], cell, cancelled)
            && let Some(tile_pix) = Pixmap::from_vec(
                tile.premultiplied_rgba,
                tiny_skia::IntSize::from_wh(tile.width, tile.height).unwrap(),
            )
        {
            let col = (idx % 2) as i32;
            let row = (idx / 2) as i32;
            let ox = col * cell[0] as i32 + ((cell[0] as i32 - tile.width as i32) / 2);
            let oy = row * cell[1] as i32 + ((cell[1] as i32 - tile.height as i32) / 2);
            canvas.draw_pixmap(
                ox,
                oy,
                tile_pix.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
    Ok(pixmap_to_thumbnail(canvas))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_preview_uses_the_selected_unit_and_keeps_shared_geometry() {
        let bytes = br#"(kicad_symbol_lib
            (symbol "Base"
                (symbol "Base_0_1" (circle (center 0 0) (radius 1)))
                (symbol "Base_1_1" (rectangle (start -2 -2) (end 2 2)))
                (symbol "Base_3_1"
                    (rectangle (start -4 -1) (end 4 1))
                    (pin passive line (at 5 0 180) (length 1) (name "~") (number "3"))))
            (symbol "Derived" (extends "Base")))"#;
        let cancel = AtomicBool::new(false);
        let doc = KicadSymbolLibDocument::parse(bytes, &cancel).unwrap();
        assert_eq!(doc.symbols[1].units, [1, 3]);
        let one = doc
            .render_symbol_thumbnail(0, 1, [120, 120], &cancel)
            .unwrap();
        let three = doc
            .render_symbol_thumbnail(0, 3, [120, 120], &cancel)
            .unwrap();
        let derived = doc
            .render_symbol_thumbnail(1, 3, [120, 120], &cancel)
            .unwrap();
        assert_ne!(one.premultiplied_rgba, three.premultiplied_rgba);
        assert_eq!(three.premultiplied_rgba, derived.premultiplied_rgba);
    }

    #[test]
    fn kicad_schematic_and_symbol_lib_parse_resolve_extends_and_render() {
        let sch = r#"(kicad_sch
          (version 20231120)
          (generator "eeschema")
          (paper "A4")
          (title_block (title "Sensor Node") (rev "A"))
          (lib_symbols
            (symbol "Device:R"
              (property "Reference" "R")
              (property "Value" "R")
              (symbol "R_0_1"
                (rectangle (start -1.016 -2.54) (end 1.016 2.54))
              )
              (symbol "R_1_1"
                (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1"))
                (pin passive line (at 0 -3.81 90) (length 1.27) (name "~") (number "2"))
              )
            )
          )
          (wire (pts (xy 50 50) (xy 80 50)))
          (bus (pts (xy 80 60) (xy 110 60)))
          (bus_entry (at 80 60) (size 2.54 -2.54))
          (label "SDA" (at 60 50 0))
          (global_label "VCC_3V3" (shape input) (at 80 50 0))
          (symbol (lib_id "Device:R") (at 50 60 0) (unit 1) (uuid "u1")
            (property "Reference" "R1")
            (property "Value" "4.7k")
          )
        )"#;
        let cancel = AtomicBool::new(false);
        let sch_doc =
            KicadSchematicDocument::parse(Path::new("sensor.kicad_sch"), sch.as_bytes(), &cancel)
                .unwrap();
        assert_eq!(sch_doc.sheet_title, "Sensor Node");
        assert_eq!(sch_doc.symbol_count, 1);
        assert_eq!(sch_doc.wire_count, 1);
        assert_eq!(sch_doc.bus_count, 2);
        assert_eq!(sch_doc.net_labels.len(), 2);
        let sch_thumb = sch_doc.render_thumbnail([180, 120], &cancel).unwrap();
        assert!(
            sch_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [SCHEMATIC_BG[0], SCHEMATIC_BG[1], SCHEMATIC_BG[2]])
        );

        let sym_lib = r#"(kicad_symbol_lib
          (version 20231120)
          (generator "kicad_symbol_editor")
          (symbol "BaseRes"
            (property "Reference" "R")
            (property "Value" "BaseRes")
            (symbol "BaseRes_0_1" (rectangle (start -1.0 -2.5) (end 1.0 2.5)))
            (symbol "BaseRes_1_1"
              (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1"))
              (pin passive line (at 0 -3.81 90) (length 1.27) (name "~") (number "2"))
            )
          )
          (symbol "PullUp"
            (extends "BaseRes")
            (property "Value" "PullUp")
          )
        )"#;
        let sym_doc = KicadSymbolLibDocument::parse(sym_lib.as_bytes(), &cancel).unwrap();
        assert_eq!(sym_doc.symbols.len(), 2);
        assert_eq!(sym_doc.symbols[1].extends.as_deref(), Some("BaseRes"));
        assert_eq!(sym_doc.symbols[1].value, "PullUp");
        assert_eq!(sym_doc.symbols[1].pins.len(), 2);
        let sym_thumb = sym_doc.render_thumbnail([160, 120], &cancel).unwrap();
        assert!(
            sym_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [SCHEMATIC_BG[0], SCHEMATIC_BG[1], SCHEMATIC_BG[2]])
        );
    }
}
