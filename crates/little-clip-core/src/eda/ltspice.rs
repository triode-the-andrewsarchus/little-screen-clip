//! Read-only LTspice schematic geometry and local symbol definitions.

use super::{
    EdaFormat, RasterRegion, StripSummary, check_cancelled,
    kicad::scene_render::{render_scene_region, render_scene_thumbnail},
    spice::decode_ltspice_drawing,
};
use crate::{ImageThumbnail, file_source};
use eda_view::{
    strokes::{HJustify, TextSpec, VJustify, to_strokes},
    vector::{
        Group, GroupId, GroupKind, Item, Layer, LayerKind, Point, Prim, Prop, Role, Scene,
        SceneKind, Side,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    f64::consts::TAU,
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
};
use tiny_skia::Color;

const MAX_ITEMS: usize = 40_000;
const MAX_SYMBOLS: usize = 2_000;
const MAX_DEFINITIONS: usize = 128;
const MAX_SYMBOL_BYTES: usize = 128 * 1024;
const MAX_GLYPHS: usize = 32_768;

pub fn is_schematic_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("asc"))
        && file_source::read_file_bytes(path, 4096, false, true)
            .is_ok_and(|bytes| is_schematic_bytes(&bytes))
}

pub fn is_schematic_bytes(bytes: &[u8]) -> bool {
    let text = decode_ltspice_drawing(&bytes[..bytes.len().min(4096)]);
    let mut records = records(&text);
    records.next().is_some_and(supports_version)
        && records.next().is_some_and(|[tag, fields]| {
            let mut fields = fields.split_whitespace();
            tag.eq_ignore_ascii_case("sheet")
                && (0..3).all(|_| {
                    fields
                        .next()
                        .is_some_and(|value| value.parse::<i32>().is_ok())
                })
                && fields.next().is_none()
        })
}

#[derive(Clone, Debug)]
pub struct Schematic {
    pub scene: Scene,
    pub bounds: [f64; 4],
    pub wire_count: usize,
    pub symbol_count: usize,
    pub unresolved: Vec<String>,
    pub limited: bool,
    pub source_excerpt: String,
}

impl Schematic {
    pub(super) fn parse(
        bytes: &[u8],
        companion_dir: Option<&Path>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        if !is_schematic_bytes(bytes) {
            return Err("Expected an LTspice Version 4 schematic with a SHEET record".into());
        }
        let text = decode_ltspice_drawing(bytes);
        let Sheet {
            mut drawing,
            symbols,
            wire_count,
            symbol_count,
        } = read_sheet(&text, cancel)?;
        let unresolved = resolve_symbols(&mut drawing, symbols, companion_dir, cancel)?;
        drawing.scene.recompute_bbox();
        let bbox = drawing.scene.bbox;
        let bounds = if bbox.is_empty() {
            [0.0, 0.0, 100.0, 100.0]
        } else {
            [bbox.min[0], bbox.min[1], bbox.max[0], bbox.max[1]]
        };
        check_cancelled(cancel, "LTspice schematic parse cancelled")?;
        Ok(Self {
            scene: drawing.scene,
            bounds,
            wire_count,
            symbol_count,
            unresolved,
            limited: drawing.limited,
            source_excerpt: super::bounded_source_excerpt(&text),
        })
    }

    pub fn summary_line(&self) -> String {
        format!("{} wires · {} symbols", self.wire_count, self.symbol_count)
    }

    pub fn strip_summary(&self) -> StripSummary {
        let specs = if self.unresolved.is_empty() {
            "LTspice drawing".to_owned()
        } else {
            format!("{} unresolved symbol definitions", self.unresolved.len())
        };
        StripSummary {
            format: EdaFormat::LtspiceSchematic,
            headline: self.summary_line(),
            specs,
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        size: [u32; 2],
        cancel: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        render_scene_thumbnail(&self.scene, size, background(), cancel)
    }

    pub fn render_region(
        &self,
        region: RasterRegion,
        cancel: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        render_scene_region(&self.scene, region, background(), cancel)
    }
}

pub(super) fn comparison_drawing(
    bytes: &[u8],
    companion_dir: Option<&Path>,
    side: &str,
    cancel: &AtomicBool,
) -> Result<super::source_compare::Drawing, String> {
    let sheet = Schematic::parse(bytes, companion_dir, cancel)?;
    let mut drawing = super::source_compare::Drawing::from_scene(&sheet.scene, cancel)?;
    drawing.warnings.push(
                    "One LTspice sheet · wire endpoints, flags and resolved pins are compared · hierarchical connectivity and electrical rules are outside coverage".into(),
                );
    if sheet.limited {
        drawing.warnings.push(format!(
            "{side}: some LTspice records could not be drawn or exceeded preview limits"
        ));
    }
    if !sheet.unresolved.is_empty() {
        let names = sheet
            .unresolved
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        drawing.warnings.push(format!(
                        "{side}: {} missing or unreadable local .asy definitions ({names}). Placements remain comparable.", sheet.unresolved.len()
                    ));
    }
    Ok(drawing)
}

struct Sheet<'a> {
    drawing: Drawing,
    symbols: Vec<Option<Placement<'a>>>,
    wire_count: usize,
    symbol_count: usize,
}

fn read_sheet<'a>(text: &'a str, cancel: &AtomicBool) -> Result<Sheet<'a>, String> {
    let mut drawing = Drawing::new();
    let mut symbols = Vec::<Option<Placement>>::new();
    let mut wire_count = 0;
    let mut symbol_count = 0;
    let mut wire_ends = BTreeMap::<[i32; 2], u32>::new();
    for (index, [tag, fields]) in records(text).enumerate().skip(2) {
        if index.is_multiple_of(256) {
            check_cancelled(cancel, "LTspice schematic parse cancelled")?;
        }
        let mut fields = fields.split_whitespace();
        match tag {
            "WIRE" | "BUSTAP" => {
                if let Some([a, b]) = points(&mut fields) {
                    if tag == "WIRE" {
                        wire_count += 1;
                    }
                    if drawing.scene.items.len() < MAX_ITEMS {
                        for p in [a, b] {
                            *wire_ends.entry(p.map(|x| x as i32)).or_default() += 1;
                        }
                        drawing.add_properties(
                            0,
                            Role::Wire,
                            line(vec![a, b], 1.5),
                            vec![Prop::new("kind", tag)],
                        );
                    } else {
                        drawing.limited = true;
                    }
                }
            }
            "SYMBOL" => {
                symbol_count += 1;
                if symbols.len() < MAX_SYMBOLS {
                    let placement = Placement::parse(fields);
                    drawing.limited |= placement.is_none();
                    symbols.push(placement);
                } else {
                    drawing.limited = true;
                }
            }
            "SYMATTR" | "WINDOW" if symbol_count <= MAX_SYMBOLS => {
                if let Some(Some(symbol)) = symbols.last_mut() {
                    symbol.properties.read_field(tag, fields);
                }
            }
            "FLAG" | "DATAFLAG" | "IOPIN" => drawing.flag(tag, fields),
            "TEXT" => drawing.annotation(fields),
            "LINE" | "RECTANGLE" | "CIRCLE" | "ARC" => {
                if let Some(prim) = shape(tag, fields, true, |p| p) {
                    drawing.add(1, Role::Graphic, prim);
                }
            }
            _ => {}
        }
    }
    for ([x, y], count) in wire_ends {
        if count > 2 {
            drawing.add(
                0,
                Role::Junction,
                Prim::Circle {
                    center: [f64::from(x), f64::from(y)],
                    radius: 3.0,
                    fill: true,
                    stroke: 0.0,
                },
            );
        }
    }

    Ok(Sheet {
        drawing,
        symbols,
        wire_count,
        symbol_count,
    })
}

fn resolve_symbols(
    drawing: &mut Drawing,
    symbols: Vec<Option<Placement<'_>>>,
    companion_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> Result<Vec<String>, String> {
    let mut definitions = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    for (index, placement) in symbols.into_iter().flatten().enumerate() {
        if index.is_multiple_of(32) {
            check_cancelled(cancel, "LTspice symbol resolution cancelled")?;
        }
        if !definitions.contains_key(&placement.name) && definitions.len() < MAX_DEFINITIONS {
            let definition =
                companion_dir.and_then(|dir| read_symbol(dir, &placement.name, cancel));
            definitions.insert(placement.name.clone(), definition);
        }
        let definition = definitions.get(&placement.name).and_then(Option::as_ref);
        if !placement.paint(drawing, definition) {
            unresolved.insert(placement.name);
        }
    }

    Ok(unresolved.into_iter().collect())
}

fn background() -> Color {
    Color::from_rgba8(20, 24, 30, 255)
}

struct Drawing {
    scene: Scene,
    glyphs: usize,
    limited: bool,
    symbol: Option<GroupId>,
}

impl Drawing {
    fn new() -> Self {
        let mut scene = Scene::new(SceneKind::Schematic, true);
        scene.layers = [
            ("Wires", LayerKind::Connectivity, [158, 206, 106, 255]),
            (
                "Symbols and graphics",
                LayerKind::Symbol,
                [140, 178, 240, 255],
            ),
            ("Labels", LayerKind::Text, [225, 230, 238, 255]),
            (
                "Unresolved symbols",
                LayerKind::Overlay,
                [248, 168, 98, 255],
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(id, (name, kind, color))| Layer {
            id: id as u16,
            name: name.to_owned(),
            kind,
            side: Side::None,
            z: id as i32,
            color,
            visible: true,
        })
        .collect();
        Self {
            scene,
            glyphs: MAX_GLYPHS,
            limited: false,
            symbol: None,
        }
    }

    fn add(&mut self, layer: u16, role: Role, prim: Prim) {
        self.add_properties(layer, role, prim, Vec::new());
    }

    fn add_properties(&mut self, layer: u16, role: Role, prim: Prim, props: Vec<Prop>) {
        if self.scene.items.len() >= MAX_ITEMS {
            self.limited = true;
            return;
        }
        self.scene.items.push(Item {
            id: self.scene.items.len() as u32,
            layer,
            role,
            prim,
            net: None,
            group: self.symbol,
            props,
        });
    }

    fn flag(&mut self, tag: &str, mut fields: std::str::SplitWhitespace<'_>) {
        if let Some(p) = point(&mut fields) {
            let label = words(fields);
            if tag == "FLAG" && label == "0" {
                let [x, y] = p;
                self.add_properties(
                    0,
                    Role::Label,
                    line(
                        vec![[x - 8.0, y], [x, y + 9.0], [x + 8.0, y], [x - 8.0, y]],
                        1.5,
                    ),
                    vec![Prop::new("kind", tag), Prop::new("text", "0")],
                );
            } else if tag == "IOPIN" {
                let [x, y] = p;
                self.add_properties(
                    0,
                    Role::Label,
                    line(vec![[x - 5.0, y - 4.0], p, [x - 5.0, y + 4.0]], 1.5),
                    vec![Prop::new("kind", tag), Prop::new("text", label)],
                );
            } else {
                self.text_with_kind(p, "Bottom", 2, &label, tag);
            }
        }
    }

    fn annotation(&mut self, mut fields: std::str::SplitWhitespace<'_>) {
        if let Some(pos) = point(&mut fields)
            && let Some(justify) = fields.next()
            && let Some(size) = fields.next().and_then(|n| n.parse().ok())
        {
            let text = words(fields);
            let kind = if text.starts_with('!') {
                "Directive"
            } else {
                "Annotation"
            };
            let text = text.trim_start_matches(['!', ';']).replace("\\n", "\n");
            self.text_with_kind(pos, justify, size, &text, kind);
        }
    }

    fn text(&mut self, pos: Point, justification: &str, size: u8, text: &str) {
        self.text_with_kind(pos, justification, size, text, "Field");
    }

    fn text_with_kind(
        &mut self,
        pos: Point,
        justification: &str,
        size: u8,
        text: &str,
        kind: &str,
    ) {
        if size == 0 || justification == "Invisible" || text.is_empty() {
            return;
        }
        let (angle_deg, justify) = justification
            .strip_prefix('V')
            .map_or((0.0, justification), |s| (90.0, s));
        let size = 6.0 + 2.0 * f64::from(size.min(7));
        let mut chars = text.chars();
        let text: String = chars.by_ref().take(self.glyphs.min(256)).collect();
        self.limited |= chars.next().is_some();
        if text.is_empty() {
            self.limited = true;
            return;
        }
        self.glyphs -= text.chars().count();
        let justify_h = match justify {
            "Left" => HJustify::Left,
            "Right" => HJustify::Right,
            _ => HJustify::Center,
        };
        let justify_v = match justify {
            "Top" => VJustify::Top,
            "Bottom" => VJustify::Bottom,
            _ => VJustify::Center,
        };
        let spec = TextSpec {
            text,
            pos,
            size: [size; 2],
            angle_deg,
            justify_h,
            justify_v,
            ..TextSpec::default()
        };
        let role = if matches!(kind, "FLAG" | "DATAFLAG") {
            Role::Label
        } else {
            Role::Text
        };
        self.add_properties(
            2,
            role,
            to_strokes(&spec).into_prim(),
            vec![
                Prop::new("kind", kind),
                Prop::new("text", &spec.text),
                Prop::new("position", format!("{pos:?}")),
            ],
        );
    }
}

#[derive(Default)]
struct Definition {
    attributes: BTreeMap<&'static str, String>,
    windows: BTreeMap<u8, Window>,
    shapes: Vec<Prim>,
    pins: Vec<Pin>,
    limited: bool,
}

struct Pin {
    at: Point,
    properties: Vec<Prop>,
}

struct Window {
    at: Point,
    justify: String,
    size: u8,
}

impl Definition {
    fn read_field(&mut self, tag: &str, mut fields: std::str::SplitWhitespace<'_>) {
        if tag == "WINDOW" {
            if let Some(index) = fields
                .next()
                .and_then(|v| v.parse::<u8>().ok())
                .filter(|n| matches!(n, 0 | 3))
                && let Some(at) = point(&mut fields)
                && let Some(justify) = fields.next()
                && let Some(size) = fields.next().and_then(|v| v.parse().ok())
            {
                self.windows.insert(
                    index,
                    Window {
                        at,
                        justify: justify.chars().take(12).collect(),
                        size,
                    },
                );
            }
        } else if let Some(key) = fields.next().and_then(|key| {
            ["InstName", "Value", "Value2", "SpiceLine", "SpiceLine2"]
                .into_iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(key))
        }) {
            self.attributes.insert(key, words(fields));
        }
    }
}

struct Placement<'a> {
    name: String,
    pos: Point,
    orientation: &'a str,
    properties: Definition,
}

impl<'a> Placement<'a> {
    fn parse(mut fields: std::str::SplitWhitespace<'a>) -> Option<Self> {
        let name = fields.next()?;
        let pos = point(&mut fields)?;
        let orientation = fields.next()?;
        if !matches!(
            orientation,
            "R0" | "R90" | "R180" | "R270" | "M0" | "M90" | "M180" | "M270"
        ) {
            return None;
        }
        Some(Self {
            name: name.chars().take(256).collect(),
            pos,
            orientation,
            properties: Definition::default(),
        })
    }

    fn paint(&self, drawing: &mut Drawing, definition: Option<&Definition>) -> bool {
        let property = |key| {
            self.properties
                .attributes
                .get(key)
                .or_else(|| definition?.attributes.get(key))
        };
        let reference = property("InstName")
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.name);
        let group_id = drawing.scene.groups.len() as GroupId;
        let mut props = vec![
            Prop::new("symbol", self.name.replace('\\', "/")),
            Prop::new("orientation", self.orientation),
            Prop::new("position", format!("{:?}", self.pos)),
        ];
        for key in ["Value", "Value2", "SpiceLine", "SpiceLine2"] {
            if let Some(value) = property(key) {
                props.push(Prop::new(key, value));
            }
        }
        drawing.scene.groups.push(Group {
            id: group_id,
            label: reference.clone(),
            kind: GroupKind::Symbol,
            props,
        });
        drawing.symbol = Some(group_id);
        let start = drawing.scene.items.len();
        let transform = |p| transform(p, self.pos, self.orientation);
        if let Some(def) = definition {
            drawing.limited |= def.limited;
            for graphic in &def.shapes {
                if drawing.scene.items.len() >= MAX_ITEMS {
                    drawing.limited = true;
                    break;
                }
                if let Prim::Polyline { points, width } = graphic {
                    drawing.add(
                        1,
                        Role::SymbolBody,
                        line(points.iter().copied().map(transform).collect(), *width),
                    );
                }
            }
            for pin in &def.pins {
                drawing.add_properties(
                    1,
                    Role::Pin,
                    Prim::Circle {
                        center: transform(pin.at),
                        radius: 1.5,
                        fill: true,
                        stroke: 0.0,
                    },
                    pin.properties.clone(),
                );
            }
        } else {
            let corners = [
                [-9.0, -9.0],
                [9.0, -9.0],
                [9.0, 9.0],
                [-9.0, 9.0],
                [-9.0, -9.0],
            ];
            drawing.add(
                3,
                Role::SymbolBody,
                line(corners.map(transform).to_vec(), 1.5),
            );
            drawing.text(
                [self.pos[0] + 13.0, self.pos[1]],
                "Left",
                1,
                &format!("{} (unresolved)", self.name),
            );
        }
        for (index, key) in [(0, "InstName"), (3, "Value")] {
            let value = property(key);
            let window = self
                .properties
                .windows
                .get(&index)
                .or_else(|| definition?.windows.get(&index));
            if let (Some(value), Some(window)) = (value, window) {
                drawing.text(transform(window.at), &window.justify, window.size, value);
            }
        }
        if drawing.scene.items.len() == start {
            drawing.add(
                3,
                Role::SymbolBody,
                Prim::Circle {
                    center: self.pos,
                    radius: 4.0,
                    fill: false,
                    stroke: 1.5,
                },
            );
        }
        drawing.symbol = None;
        definition.is_some()
    }
}

fn transform([x, y]: Point, [tx, ty]: Point, orientation: &str) -> Point {
    // LTspice's M90/M270 mirror in the sheet frame after rotation.
    let (mirror, rotation) = orientation.split_at(orientation.floor_char_boundary(1));
    let [x, y] = match rotation {
        "90" => [-y, x],
        "180" => [-x, -y],
        "270" => [y, -x],
        _ => [x, y],
    };
    [tx + if mirror == "M" { -x } else { x }, ty + y]
}

fn read_symbol(dir: &Path, name: &str, cancel: &AtomicBool) -> Option<Definition> {
    check_cancelled(cancel, "LTspice symbol read cancelled").ok()?;
    let name = name.replace('\\', "/");
    let relative = Path::new(&name);
    if name.is_empty()
        || name.contains(':')
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let mut path = PathBuf::from(dir).join(relative);
    path.as_mut_os_string().push(".asy");
    let mut file = file_source::open_regular_file(&path, false).ok()?;
    // Check the held object's resolved path before reading any symbol bytes.
    if file_source::FileSource::capture_file(&file).ok()?.path() != path {
        return None;
    }
    let (bytes, source) = file_source::read_held_snapshot(&mut file, MAX_SYMBOL_BYTES).ok()?;
    source.check().ok()?;
    parse_symbol(&decode_ltspice_drawing(&bytes))
}

fn parse_symbol(text: &str) -> Option<Definition> {
    let mut records = records(text);
    if !records.next().is_some_and(supports_version)
        || !records
            .next()
            .is_some_and(|[tag, _]| tag.eq_ignore_ascii_case("symboltype"))
    {
        return None;
    }
    let mut definition = Definition::default();
    for (index, [tag, value]) in records.enumerate() {
        if index >= 512 {
            definition.limited = true;
            break;
        }
        match tag {
            "PIN" => {
                if let Some(at) = point(&mut value.split_whitespace()) {
                    definition.pins.push(Pin {
                        at,
                        properties: Vec::new(),
                    });
                }
            }
            "PINATTR" => {
                let mut fields = value.split_whitespace();
                if let Some(kind @ ("PinName" | "SpiceOrder")) = fields.next()
                    && let Some(pin) = definition.pins.last_mut()
                {
                    pin.properties.push(Prop::new(
                        if kind == "SpiceOrder" {
                            "number"
                        } else {
                            "name"
                        },
                        words(fields),
                    ));
                }
            }
            "WINDOW" | "SYMATTR" => definition.read_field(tag, value.split_whitespace()),
            "LINE" | "CIRCLE" | "ARC" | "RECTANGLE" => {
                definition
                    .shapes
                    .extend(shape(tag, value.split_whitespace(), false, |p| p));
            }
            _ => {}
        }
    }
    Some(definition)
}

fn supports_version([tag, value]: [&str; 2]) -> bool {
    tag.eq_ignore_ascii_case("version") && matches!(value, "4" | "4.1")
}

fn records(text: &str) -> impl Iterator<Item = [&str; 2]> {
    text.split(['\r', '\n']).filter_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let (tag, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        Some([tag, rest.trim()])
    })
}

fn point(fields: &mut std::str::SplitWhitespace<'_>) -> Option<Point> {
    let coordinate = |s: &str| s.parse::<i32>().ok().map(f64::from);
    Some([coordinate(fields.next()?)?, coordinate(fields.next()?)?])
}

fn points(fields: &mut std::str::SplitWhitespace<'_>) -> Option<[Point; 2]> {
    Some([point(fields)?, point(fields)?])
}

fn words(fields: std::str::SplitWhitespace<'_>) -> String {
    fields
        .take(64)
        .map(|token| token.chars().take(256).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

fn line(points: Vec<Point>, width: f64) -> Prim {
    Prim::Polyline { points, width }
}

fn shape(
    tag: &str,
    mut fields: std::str::SplitWhitespace<'_>,
    sheet: bool,
    map: impl Fn(Point) -> Point,
) -> Option<Prim> {
    let width = if fields.next()?.eq_ignore_ascii_case("Wide") {
        3.0
    } else {
        1.5
    };
    let [a, b] = points(&mut fields)?;
    let geometry = match tag {
        "LINE" => vec![a, b],
        "RECTANGLE" => vec![a, [b[0], a[1]], b, [a[0], b[1]], a],
        "CIRCLE" | "ARC" => {
            let center = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let radius = [(a[0] - b[0]).abs() * 0.5, (a[1] - b[1]).abs() * 0.5];
            if radius.contains(&0.0) {
                return None;
            }
            let (start, sweep) = if tag == "ARC" {
                let [mut start, mut end] = points(&mut fields)?;
                if sheet {
                    std::mem::swap(&mut start, &mut end);
                }
                let angle = |[x, y]: Point| {
                    ((center[1] - y) / radius[1]).atan2((x - center[0]) / radius[0])
                };
                let start = angle(start);
                (start, (angle(end) - start).rem_euclid(TAU))
            } else {
                (0.0, TAU)
            };
            let segments = (sweep / TAU * 64.0).ceil().max(1.0) as u32;
            (0..=segments)
                .map(|step| {
                    let t = start + sweep * f64::from(step) / f64::from(segments);
                    [
                        center[0] + t.cos() * radius[0],
                        center[1] - t.sin() * radius[1],
                    ]
                })
                .collect()
        }
        _ => return None,
    };
    Some(line(geometry.into_iter().map(map).collect(), width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        eda::{Document, read_document, source_compare::compare_sources},
        file_source::FileSource,
    };

    #[test]
    fn asc_compare_registers_wires_symbols_and_labels_across_encodings() {
        let folder = tempfile::tempdir().unwrap();
        let paths = ["a.asc", "b.asc"].map(|name| folder.path().join(name));
        std::fs::write(folder.path().join("local.asy"),
            "Version 4\nSymbolType CELL\nLINE Normal 0 0 10 0\nPIN 10 0 NONE 0\nPINATTR PinName Plus\nPINATTR SpiceOrder 1\n"
        ).unwrap();
        let original = "Version 4\nSHEET 1 880 680\nWIRE 20 10 20 70\nFLAG 20 70 entrée\nSYMBOL local 20 10 R90\nSYMATTR InstName X1\nSYMBOL missing 60 70 R0\nSYMATTR InstName X2\n";
        let equivalent = "Version 4.1\r\nSHEET 1 880 680\r\nSYMBOL missing 60 70 R0\r\nSYMATTR InstName X2\r\nWIRE 20 70 20 10\r\nFLAG 20 70 entrée\r\nSYMBOL local 20 10 R90\r\nSYMATTR InstName X1\r\n";
        std::fs::write(
            &paths[0],
            original.chars().map(|c| c as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        let utf16 = [0xff, 0xfe]
            .into_iter()
            .chain(equivalent.encode_utf16().flat_map(u16::to_le_bytes))
            .collect::<Vec<_>>();
        std::fs::write(&paths[1], utf16).unwrap();
        let compare = || {
            let files = paths
                .each_ref()
                .map(|path| FileSource::capture(path).unwrap());
            compare_sources(
                EdaFormat::LtspiceSchematic,
                files.each_ref(),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        let same = compare();
        assert_eq!(same.counts, [0, 0]);
        assert!(
            same.warnings
                .iter()
                .any(|line| line.contains("local .asy") && line.contains("missing"))
        );
        let changed = original
            .replace("local 20 10 R90", "local 20 10 M270")
            .replace("missing 60 70 R0", "missing 60 70 M0")
            .replace("WIRE 20 10 20 70", "WIRE 20 10 40 70")
            .replace("entrée", "sortie");
        std::fs::write(&paths[1], changed).unwrap();
        let changed = compare();
        for target in ["Wire", "X1", "X2", "sortie"] {
            assert!(
                changed
                    .changes
                    .iter()
                    .any(|change| change.side == 1 && change.label.contains(target)),
                "{target}"
            );
        }
        assert!(
            changed
                .changes
                .iter()
                .any(|change| change.side == 1 && change.label.contains("Pin · X1 · 1"))
        );
    }

    #[test]
    fn asc_uses_sheet_coordinates_encoding_and_local_rotated_symbols() {
        let base = tempfile::tempdir().unwrap();
        let source = base.path().join("demo.asc");
        let bytes = b"Version 4.1\r\nSHEET 1 880 680\r\nWIRE 20 -10 20 80\r\nFLAG 20 80 0\r\nSYMBOL local 20 -10 R90\r\nSYMATTR InstName L1\r\nSYMATTR Value 22\xb5\r\nSYMBOL ../outside 50 70 M0\r\n";
        std::fs::write(&source, bytes).unwrap();
        std::fs::write(
            base.path().join("local.asy"),
            "Version 4\nSymbolType CELL\nLINE Normal 0 0 10 0\nWINDOW 0 20 0 Left 2\nWINDOW 3 20 10 Left 2\nPIN 10 0 NONE 0\n",
        ).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(is_schematic_path(&source));
        let Document::LtspiceSchematic(doc) = read_document(&source, &cancel).unwrap() else {
            panic!("expected LTspice schematic");
        };
        assert_eq!(doc.unresolved, ["../outside"]);
        assert_eq!(doc.wire_count, 1);
        assert!(doc.source_excerpt.contains("22µ"));
        assert!(doc.scene.items.iter().any(|item| {
            item.role == Role::SymbolBody
                && matches!(&item.prim, Prim::Polyline { points, .. } if points == &[[20.0, -10.0], [20.0, 0.0]])
        }));
        let img = doc.render_thumbnail([320, 200], &cancel).unwrap();
        assert!(
            img.premultiplied_rgba
                .chunks_exact(4)
                .any(|p| p == [158, 206, 106, 255])
        );
    }

    #[test]
    fn schematic_probe_preserves_can_and_pgp_and_accepts_utf16() {
        let source = "Version 4\nSHEET 1 880 680\nWIRE 1 2 3 4\n";
        let encoded: Vec<_> = [0xfe, 0xff]
            .into_iter()
            .chain(source.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert!(is_schematic_bytes(&encoded));
        for other in [
            b"-----BEGIN PGP SIGNATURE-----\nVersion: GnuPG".as_slice(),
            b"date Fri Sep 15 12:50:18.871 2023\nbase hex timestamps absolute\n",
            b"Version 4\nSHEET bogus fields here\n",
        ] {
            assert!(!is_schematic_bytes(other));
        }
    }
}
