//! Nodes: text in a shape, placed by an anchor, as `\tikz@fig` and pgf's shapes do.
//!
//! A node's own coordinate system has the text's origin (the left end of its first baseline) at
//! (0, 0). The shape (see [`super::shapes`]) is computed there, from the text boxes of the node's
//! parts, `inner sep`, the minimum sizes and `outer sep`; the node is then moved so that the chosen
//! anchor lands on its position. The transformations of the picture apply to that position only,
//! unless `transform shape`; those of the node's own options apply to the whole node.

use std::rc::Rc;

use crate::canvas::{transform_segments, Fill, Item, Paint, Stroke};
use crate::keys;
use crate::label;
use crate::scene::{Label, Point, Segment, Transform};
use crate::tex::{self, Token};
use crate::{Error, PT};

use super::pgfplots::graphics;
use super::shapes::{self, Border, BoxSize, Geometry, Layer, ShapeText, Usage};
use super::{tikra_tokens, Engine};
use crate::extension::{LineCap, ShapePaint};

pub fn is_shape(name: &str) -> bool {
    shapes::is_shape(name)
}

/// Node settings that last for one node (set by its options).
#[derive(Debug, Clone, Default)]
pub struct NodeSetup {
    pub at: Option<Point>,
    pub pos: Option<f64>,
    pub sloped: bool,
    pub transform_shape: bool,
    /// `transform shape nonlinear`: the node turned and scaled as the nonlinear transformation
    /// bends the picture where it is
    pub transform_shape_nonlinear: bool,
    pub name: Option<String>,
    /// `label=` and `pin=` values, in order; true for a pin
    pub labels: Vec<(Vec<Token>, bool)>,
    /// `fit`: the width and height of the fitted area, which become the text box
    pub fit: Option<(f64, f64)>,
    /// `rotate fit`: the fitted area is turned by this angle (and so is the node)
    pub fit_rotate: f64,
    /// `circle through`: the point on the circle (canvas)
    pub through: Option<Point>,
    /// `right of=a` and its kin: their direction, the node distance they shifted by (vertical,
    /// horizontal), and the pending transformation right after their shift
    pub old_of: Vec<(Point, (f64, f64), Transform)>,
}

/// A named node: its shape in its own coordinate system, and where that system is on the canvas.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub shape: String,
    pub geometry: Rc<Geometry>,
    /// node coordinates → canvas
    pub transform: Transform,
}

impl NodeInfo {
    pub fn center(&self) -> Point {
        self.transform.apply(self.geometry.center)
    }

    /// The border point toward canvas point `p` (where a line to the node stops).
    pub fn border_toward(&self, p: Point) -> Point {
        let inv = self.transform.inverse();
        let q = inv.apply(p);
        let b = self.geometry.border_point(q.minus(self.geometry.center));
        self.transform.apply(b)
    }

    /// A named or angle anchor, on the canvas.
    pub fn anchor(&self, name: &str) -> Option<Point> {
        self.anchor_local(name).map(|p| self.transform.apply(p))
    }

    pub fn anchor_local(&self, name: &str) -> Option<Point> {
        self.geometry.anchor(name)
    }
}

/// A library's shape as the geometry of a node.
fn library_geometry(shape: crate::extension::NodeShape) -> Geometry {
    use crate::extension::{PathSegment, ShapeBorder};
    let border = match shape.border {
        ShapeBorder::Point => Border::Point,
        ShapeBorder::Rectangle(half) => Border::Rectangle(half),
        ShapeBorder::Outline(points) => Border::Outline(points),
    };
    let mut g = Geometry::new(shape.center, border);
    g.anchors = shape.anchors;
    g.parts = vec![("text".into(), shape.text)];
    g.extent = shape.extent;
    for path in shape.paths {
        let segments = path
            .segments
            .into_iter()
            .map(|s| match s {
                PathSegment::MoveTo(p) => Segment::MoveTo(p),
                PathSegment::LineTo(p) => Segment::LineTo(p),
                PathSegment::CurveTo(a, b, c) => Segment::CurveTo(a, b, c),
                PathSegment::Close => Segment::Close,
            })
            .collect();
        g.path(Layer::Background, segments, Usage::Library { paint: path.paint, color: path.color, dash: path.dash, cap: path.cap, opacity: path.opacity });
    }
    g.texts = shape.texts.into_iter().map(|t| ShapeText { tex: t.text, at: t.at, color: None, anchor: Some(t.anchor), font: t.font, turn: t.turn, upright: t.upright }).collect();
    g
}

/// Where the point `a` of a text's box is, from the left end of its baseline (as a node's anchor
/// with no inner sep: `mid` half an ex of its font above the baseline).
fn text_anchor(a: crate::extension::TextAnchor, l: &Label, font: &str) -> Point {
    use crate::extension::TextAnchor::*;
    let (w, h, d) = (l.width(), l.height(), l.depth());
    let x = match a {
        West | NorthWest | SouthWest | BaseWest | MidWest => 0.0,
        East | NorthEast | SouthEast | BaseEast | MidEast => w,
        Center | North | South | Base | Mid => w / 2.0,
    };
    let y = match a {
        North | NorthEast | NorthWest => h,
        South | SouthEast | SouthWest => -d,
        Center | East | West => (h - d) / 2.0,
        Base | BaseEast | BaseWest => 0.0,
        Mid | MidEast | MidWest => 0.5 * label::x_height(false) * 10.0 * label::font_size(font) * PT,
    };
    Point::new(x, y)
}

/// The node's text split at `\nodepart[options]{name}`: the parts in order, `text` first.
pub fn split_parts(tokens: &[Token]) -> Vec<(String, Vec<Token>)> {
    let mut parts = vec![("text".to_string(), Vec::new())];
    let mut current = 0usize;
    let mut depth = 0usize;
    let mut i = 0;
    let skip_spaces = |j: &mut usize| {
        while tokens.get(*j).is_some_and(|t| t.is_space()) {
            *j += 1;
        }
    };
    while i < tokens.len() {
        let t = &tokens[i];
        match t {
            Token::Char(_, tex::Cat::Begin) => depth += 1,
            Token::Char(_, tex::Cat::End) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && t.is_cs("nodepart") {
            let mut j = i + 1;
            skip_spaces(&mut j);
            // the part's options are not used
            if tokens.get(j).is_some_and(|t| t.is_char('[')) {
                while j < tokens.len() && !tokens[j].is_char(']') {
                    j += 1;
                }
                j += 1;
                skip_spaces(&mut j);
            }
            let mut name = Vec::new();
            if let Some(Token::Char(_, tex::Cat::Begin)) = tokens.get(j) {
                let mut d = 0usize;
                while j < tokens.len() {
                    match &tokens[j] {
                        Token::Char(_, tex::Cat::Begin) => d += 1,
                        Token::Char(_, tex::Cat::End) => d -= 1,
                        _ => {}
                    }
                    if d == 0 {
                        break;
                    }
                    if !(d == 1 && matches!(tokens[j], Token::Char(_, tex::Cat::Begin))) {
                        name.push(tokens[j].clone());
                    }
                    j += 1;
                }
                j += 1;
            } else if let Some(t) = tokens.get(j) {
                name.push(t.clone());
                j += 1;
            }
            let name = tex::tokens_to_string(&name).split_whitespace().collect::<Vec<_>>().join(" ");
            // pgf's other names for the boxes of the rectangle split
            let name = match name.as_str() {
                "one" => "text".to_string(),
                "second" => "two".to_string(),
                "third" => "three".to_string(),
                "fourth" => "four".to_string(),
                _ => name,
            };
            // a part named again starts its box anew, as `\setbox` does (`\nodepart{text}text`)
            match parts.iter().position(|(n, _)| *n == name) {
                Some(k) => {
                    parts[k].1.clear();
                    current = k;
                }
                None => {
                    parts.push((name, Vec::new()));
                    current = parts.len() - 1;
                }
            }
            i = j;
            continue;
        }
        parts[current].1.push(t.clone());
        i += 1;
    }
    parts
}

/// The shape keys, read from the engine's pgfkeys.
pub(super) struct EngineKeys<'a>(pub(super) &'a mut Engine);

impl shapes::Keys for EngineKeys<'_> {
    fn tip_shapes(&mut self, spec: &str, line_width: f64) -> Option<(f64, Vec<super::arrows::TipPath>)> {
        super::arrows::tip_shapes(self.0, spec, line_width)
    }

    fn length(&mut self, key: &str, default: f64) -> f64 {
        self.0.key_length(key, default)
    }

    fn length_of(&mut self, text: &str, default: f64) -> f64 {
        self.0.length(text).unwrap_or(default)
    }

    fn number(&mut self, key: &str, default: f64) -> f64 {
        match keys::value(&self.0.tex, key) {
            Some(v) if !keys::trim(&v).is_empty() => self.0.number_of(v).unwrap_or(default),
            _ => default,
        }
    }

    fn text(&mut self, key: &str) -> String {
        match keys::value(&self.0.tex, key) {
            Some(v) => self.0.tex.expand_to_string(v).unwrap_or_default(),
            None => String::new(),
        }
    }

    fn flag(&mut self, name: &str) -> bool {
        let m = self.0.tex.meaning_of(&format!("if{name}"));
        m == self.0.tex.meaning_of("iftrue")
    }

    fn point(&mut self, key: &str) -> Option<Point> {
        let text = self.text(key);
        let text = crate::keys::strip_braces(text.trim()).trim().to_string();
        if text.is_empty() {
            return None;
        }
        // `(a)+(b)`: the second relative to the first
        let mut total: Option<Point> = None;
        for part in text.split(")+(").map(|s| s.trim_start_matches('+')) {
            let inner = part.trim().trim_start_matches('(').trim_end_matches(')').to_string();
            let p = self.0.coordinate(&inner).ok()?.p;
            total = Some(match total {
                None => p,
                Some(t) => t.plus(p.minus(self.0.coordinate("0,0").ok()?.p)),
            });
        }
        total
    }

    fn text_box(&mut self, tex: &str) -> BoxSize {
        let (text, _) = self.0.shape_text(tex);
        if text.trim().is_empty() {
            return BoxSize::default();
        }
        let mut warnings = Vec::new();
        let l = label::compose_label(text.trim(), &self.0.st.font, None, 'c', self.0.st.text_paint_color(), &mut warnings);
        BoxSize { wd: l.width(), ht: l.height(), dp: l.depth() }
    }

    fn code_paths(&mut self, key: &str, t: Transform) -> Vec<Vec<Segment>> {
        let e = &mut *self.0;
        let Some(code) = keys::value(&e.tex, key).filter(|v| !keys::trim(v).is_empty()) else { return Vec::new() };
        // run where nothing it draws stays or counts for the picture's size
        let n = e.canvas.items.len();
        let frozen = e.canvas_frozen;
        let path = std::mem::take(&mut e.pgf_path);
        e.begin_group();
        e.canvas_frozen = true;
        e.st.transform = t;
        e.st.mode = Default::default();
        e.st.mode.draw = true;
        let r = e.run_tokens(code);
        e.end_group();
        e.canvas_frozen = frozen;
        e.pgf_path = path;
        if let Err(err) = r {
            e.warn(format!("{key}: {err}"));
        }
        e.canvas.items.split_off(n).into_iter().filter_map(|i| if let Item::Path { segments, stroke: Some(_), .. } = i { Some(segments) } else { None }).collect()
    }
}

/// A node's labels to make: the node, their specifications (true for a pin), the last node.
/// and the values of [`LABEL_KEYS`] in the node.
type NodeLabels = (NodeInfo, Vec<(Vec<Token>, bool)>, (Option<NodeInfo>, Option<String>), Vec<(&'static str, tex::Meaning)>);

/// The keys of a node's options that its labels read.
const LABEL_KEYS: [&str; 3] = ["/tikz/label distance", "/tikz/label position", "/tikz/pin distance"];

/// Where a node on a path goes: the path's current point, or along its last segment.
#[derive(Debug, Clone, Copy)]
pub enum OnPath {
    Point(Point),
    /// a line or a curve: start, controls, end
    Segment(Point, Option<(Point, Point)>, Point),
    /// `-|` and `|-`: two legs through a corner (pos 0.5 is the corner)
    Corner(Point, Point, Point),
    /// an arc: its center, the ends of its axes at 0° and 90°, and its angles (pos goes along the
    /// angle, as `\pgftransformarcaxesattime`)
    Arc {
        center: Point,
        x: Point,
        y: Point,
        from: f64,
        to: f64,
    },
}

impl OnPath {
    pub fn at(&self, t: f64) -> (Point, Point) {
        match *self {
            OnPath::Point(p) => (p, Point::new(1.0, 0.0)),
            OnPath::Segment(a, None, b) => (a.lerp(b, t), b.minus(a)),
            OnPath::Segment(a, Some((c1, c2)), b) => {
                let u = 1.0 - t;
                let p = a.scale(u * u * u).plus(c1.scale(3.0 * u * u * t)).plus(c2.scale(3.0 * u * t * t)).plus(b.scale(t * t * t));
                let d = c1.minus(a).scale(3.0 * u * u).plus(c2.minus(c1).scale(6.0 * u * t)).plus(b.minus(c2).scale(3.0 * t * t));
                (p, d)
            }
            OnPath::Arc { center, x, y, from, to } => {
                let angle = (from + t * (to - from)).to_radians();
                let (s, c) = angle.sin_cos();
                let p = center.plus(x.scale(c)).plus(y.scale(s));
                let d = y.scale(c).minus(x.scale(s)).scale(if to < from { -1.0 } else { 1.0 });
                (p, d)
            }
            OnPath::Corner(a, c, b) => {
                if t <= 0.5 {
                    (a.lerp(c, 2.0 * t), c.minus(a))
                } else {
                    (c.lerp(b, 2.0 * t - 1.0), b.minus(c))
                }
            }
        }
    }

    /// The same place moved by `m` (the axes of an arc by its linear part only).
    pub fn transformed(&self, m: &Transform) -> OnPath {
        let o = m.apply(Point::default());
        let v = |p: Point| m.apply(p).minus(o);
        match *self {
            OnPath::Point(p) => OnPath::Point(m.apply(p)),
            OnPath::Segment(a, c, b) => OnPath::Segment(m.apply(a), c.map(|(c1, c2)| (m.apply(c1), m.apply(c2))), m.apply(b)),
            OnPath::Corner(a, c, b) => OnPath::Corner(m.apply(a), m.apply(c), m.apply(b)),
            OnPath::Arc { center, x, y, from, to } => OnPath::Arc { center: m.apply(center), x: v(x), y: v(y), from, to },
        }
    }
}

pub struct NodeSpec {
    pub options: Vec<Token>,
    pub name: Option<String>,
    pub at: Option<Point>,
    pub text: Vec<Token>,
    pub place: OnPath,
    /// the path's current point, where a node without `pos` goes
    pub point: Point,
    pub coordinate: bool,
    /// the default `pos` for a node between an operation and its target
    pub default_pos: Option<f64>,
}

impl Engine {
    /// A node's anchor; a name it does not have is an angle given by an expression, as pgf
    /// evaluates it (`(a.200+360/5)`).
    pub fn node_anchor(&mut self, node: &NodeInfo, a: &str) -> Option<Point> {
        if let Some(p) = node.anchor(a) {
            return Some(p);
        }
        let angle = self.number(a).ok()?;
        node.anchor(&angle.to_string())
    }
}

impl Engine {
    pub fn st_node(&mut self) -> &mut NodeSetup {
        &mut self.st.node
    }

    pub fn key_text(&mut self, path: &str) -> String {
        keys::value(&self.tex, path).map(|v| tex::tokens_to_string(&v)).unwrap_or_default()
    }

    fn key_length(&mut self, path: &str, default: f64) -> f64 {
        match keys::value(&self.tex, path) {
            Some(v) if !keys::trim(&v).is_empty() => self.length_of(v).unwrap_or(default),
            _ => default,
        }
    }

    /// `\tikzlastnode` is also known as `alias` (an unnamed node that edges need to name): the
    /// description finds it by either.
    pub fn alias_last_node(&mut self, alias: &str) {
        let last = self.tex.expand_to_string(vec![Token::cs("tikzlastnode")]).unwrap_or_default();
        let last = last.trim();
        if let Some(f) = self.facts.nodes.iter_mut().rev().find(|f| f.key == last) {
            f.key = alias.to_string();
        }
    }

    /// Creates a node: returns what it draws (drawn after its path) and registers its name.
    pub fn node(&mut self, spec: NodeSpec) -> Result<Vec<Item>, Error> {
        // `\pgfmathprintnumber{…}` in a node's text is printed as pgf prints numbers
        let mut spec = spec;
        if spec.text.iter().any(|t| t.is_cs("pgfmathprintnumber") || t.is_cs("pgfplotspointplotattime") || t.is_cs("pgfplotspointgetcoordinates")) {
            spec.text = super::pgfplots::draw::print_numbers(self, &spec.text)?;
        }
        self.begin_group();
        // `transform shape` given to a scope or a path holds for its nodes
        self.st.node = NodeSetup { transform_shape: self.st.node.transform_shape, ..NodeSetup::default() };
        self.st.mode = Default::default();
        self.st.pending = Transform::default();
        self.st.on_chain = None;
        self.st.pending_joins.clear();
        self.st.after_node_path.clear();
        self.st.aliases.clear();
        self.st.start_arrow = None;
        self.st.end_arrow = None;
        // a node's background is decorated, and has actions, by its own options only
        let outer = self.st.clone();
        self.st.decorate = false;
        self.st.preactions = Default::default();
        self.st.postactions = Default::default();
        let shifts_before = self.anim_shifts.len();
        let r = self.node_in_group(spec, &outer);
        // `behind path`: drawn before the path the node is on
        let behind = matches!(self.tex.meaning_of("tikra@behindpath"), tex::Meaning::Macro(m) if tex::tokens_to_string(&tex::substitute(&m.body, &[])).trim() == "1");
        // (`overlay` among the node's own options ends with its group)
        let overlay = self.canvas.overlay;
        self.end_group();
        let (mut items, labels) = r?;
        // animated shifts of the node: the picture makes room for it at each, unless it is an overlay
        let shifts: Vec<Point> = self.anim_shifts.drain(shifts_before..).collect();
        if let (false, false, Some(info)) = (shifts.is_empty(), overlay, self.last_node.clone()) {
            let ends: Vec<Point> = ["north", "south", "east", "west"].iter().filter_map(|a| info.anchor(a)).collect();
            if self.counts_for_size() {
                for d in shifts {
                    for p in &ends {
                        self.canvas.include(p.plus(d));
                    }
                }
            }
        }
        if behind {
            self.behind_path.append(&mut items);
        }
        // labels and pins: made after the node, out of its options (TikZ's `append after command`)
        if let Some((info, labels, last, kept)) = labels {
            // the label distances and position that the node's options set still hold
            self.begin_group();
            for (key, value) in kept {
                self.tex.set_meaning(&Token::cs(&keys::macro_name(key)), value, false);
            }
            let r = (|| {
                for (spec_tokens, pin) in labels {
                    items.extend(self.label(&info, spec_tokens, pin)?);
                }
                Ok::<(), Error>(())
            })();
            self.end_group();
            r?;
            (self.last_node, self.last_node_name) = last;
        }
        Ok(items)
    }

    /// `node also [options] (name)`, `late options={name=…, …}`: the labels and pins of the
    /// options, for a node that exists.
    pub fn late_options(&mut self, name: &str, options: Vec<Token>) -> Result<Vec<Item>, Error> {
        let Some(info) = self.lookup_node(name) else {
            self.warn(format!("late options: no node named '{name}'"));
            return Ok(Vec::new());
        };
        self.begin_group();
        self.st.node = NodeSetup::default();
        let r = (|| {
            self.process_keys(options, Some("/tikz"))?;
            let mut items = Vec::new();
            for (spec, pin) in std::mem::take(&mut self.st.node.labels) {
                items.extend(self.label(&info, spec, pin)?);
            }
            Ok(items)
        })();
        self.end_group();
        r
    }

    /// The node's drawing, and what its labels need: the node, their specifications, and the last
    /// node to leave after them.
    /// A picture inside a node's text, drawn on a canvas of its own, its coordinates from its
    /// origin: what it draws and its box's corners (none when it draws nothing).
    fn inner_picture(&mut self, code: Vec<Token>) -> Result<Option<(Vec<Item>, Point, Point)>, Error> {
        let saved_canvas = std::mem::take(&mut self.canvas);
        let saved_frozen = std::mem::replace(&mut self.canvas_frozen, false);
        self.begin_group();
        self.begin_scope();
        self.st.transform = Transform::default();
        self.st.pending = Transform::default();
        let r = self.run_tokens(code);
        self.end_scope();
        self.end_group();
        let crate::canvas::Canvas { items, min, max, .. } = std::mem::replace(&mut self.canvas, saved_canvas);
        self.canvas_frozen = saved_frozen;
        r?;
        Ok(min.zip(max).map(|(lo, hi)| (items, lo, hi)))
    }

    /// `\pgfpositionnodenow{point}`: the node `\pgfpositionnodelater` kept, moved from the origin
    /// it was made at to the point, drawn and known by its name.
    pub fn position_node_now(&mut self) -> Result<(), Error> {
        let p = self.point_arg()?;
        let d = self.st.transform.apply(p);
        let Some((items, mut info, name)) = self.later_node.take() else {
            self.warn("\\pgfpositionnodenow: no node kept by \\pgfpositionnodelater".to_string());
            return Ok(());
        };
        info.transform = Transform::translation(d.x, d.y).compose(&info.transform);
        for mut item in items {
            item.translate(d);
            if let (Item::Path { segments, .. }, true) = (&item, self.counts_for_size()) {
                self.canvas.include_path(segments);
            }
            self.canvas.items.push(item);
        }
        if let Some(n) = name {
            self.nodes.insert(n, info);
        }
        Ok(())
    }

    /// A node's text expanded, the pgfmath commands and the definitions in it run where they are,
    /// as TeX runs them while it typesets the text: `\pgfmathparse{2+2}\pgfmathresult` is 4, and
    /// `\def\a{x}\a` is x. A picture in the text is left as it is, for it to be drawn.
    fn expand_text(&mut self, tokens: Vec<Token>) -> Result<Vec<Token>, Error> {
        const RUN: &[&str] = &[
            "pgfmathparse",
            "pgfmathsetmacro",
            "pgfmathtruncatemacro",
            "pgfmathsetlengthmacro",
            "def",
            "gdef",
            "edef",
            "xdef",
            "let",
            "advance",
            "multiply",
            "divide",
            "@ifnextchar",
            "setkeys",
            "begin",
            "tikz",
        ];
        // a \relax at the end, so that an assignment's number stops there, as at the end of TeX's
        // box, and not after the text
        let mut tokens = tokens;
        tokens.push(Token::cs("relax"));
        let end = |mut out: Vec<Token>| {
            if out.last().is_some_and(|t| t.is_cs("relax")) {
                out.pop();
            }
            Ok(out)
        };
        let depth = self.tex.expand_start(tokens);
        let mut out = Vec::new();
        loop {
            let (more, stop) = self.tex.expand_more(depth, RUN)?;
            // \relax, and what is \let to it, typesets nothing
            out.extend(more.into_iter().filter(|t| !matches!(t, Token::Cs(_)) || self.tex.meaning(t) != tex::Meaning::Prim(tex::Prim::Relax)));
            let Some(t) = stop else { return end(out) };
            if t.is_cs("begin") || t.is_cs("tikz") {
                // `\begin{name}` as it is; from a picture on, the rest too (its `\foreach \number`
                // is a variable, not TeX's \number)
                let mut picture = t.is_cs("tikz");
                out.push(t);
                if !picture {
                    let mut name = Vec::new();
                    // the \relax put after the text follows \begin at the latest
                    let open = self.tex.get_token_until(depth).expect("a token after \\begin");
                    if matches!(open, Token::Char(_, tex::Cat::Begin)) {
                        while let Some(x) = self.tex.get_token_until(depth) {
                            let end = matches!(x, Token::Char(_, tex::Cat::End));
                            name.push(x);
                            if end {
                                break;
                            }
                        }
                    }
                    picture = tex::tokens_to_string(&name) == "tikzpicture}";
                    out.push(open);
                    out.extend(name);
                }
                if picture {
                    while let Some(x) = self.tex.get_token_until(depth) {
                        out.push(x);
                    }
                    return end(out);
                }
                continue;
            }
            let r = match self.tex.meaning(&t) {
                tex::Meaning::Native(id) => self.command(id, &t),
                // a register given a value (`\count=3`, `\count 3`); else it is an argument, as
                // `\linewidth` of a `\parbox` that is written as it is
                tex::Meaning::Count(_) | tex::Meaning::Dimen(_) | tex::Meaning::Skip(_) | tex::Meaning::Toks(_) => {
                    let next = self.tex.next_nonspace();
                    let value = next.as_ref().is_some_and(|n| matches!(n, Token::Char(c, _) if c.is_ascii_digit() || matches!(c, '=' | '-' | '+' | '.')));
                    if let Some(n) = next {
                        self.tex.back(n);
                    }
                    if value {
                        self.tex.execute_basic(t).map(|_| ())
                    } else {
                        out.push(t);
                        Ok(())
                    }
                }
                _ => self.tex.execute_basic(t).map(|_| ()),
            };
            if let Err(e) = r {
                self.tex.drain_to(depth);
                return Err(e);
            }
        }
    }

    fn node_in_group(&mut self, spec: NodeSpec, scope: &super::GState) -> Result<(Vec<Item>, Option<NodeLabels>), Error> {
        if spec.coordinate {
            self.tex.define_string(&keys::macro_name("/tikz/shape"), "coordinate", false);
        }
        let outer_name_path = self.st.name_path.clone();
        self.process_keys(tikra_tokens("every node/.try"), Some("/tikz"))?;
        self.process_keys(spec.options.clone(), Some("/tikz"))?;
        // `right of=a` shifts by the node distance the node ends up with, given after it too: each
        // shift is redone with it, in its place among the transformations
        for (u, (v0, h0), after) in std::mem::take(&mut self.st.node.old_of).into_iter().rev() {
            let (v, h) = super::options::node_distance(self)?;
            let shift = Transform::translation(u.x * (h - h0), u.y * (v - v0));
            self.st.pending = after.compose(&shift).compose(&after.inverse()).compose(&self.st.pending);
        }
        // the node begin hook: a node on a chain takes its place and names there
        let (chain_names, joins) = self.chain_node_found()?;
        let mut after = std::mem::take(&mut self.st.after_node_path);
        after.extend(joins);
        self.after_node_path.extend(after);
        let shape_name = self.key_text("/tikz/shape").split_whitespace().collect::<Vec<_>>().join(" ");
        let user_shape = self.user_shapes.get(shape_name.trim()).cloned();
        let library_shape = self.extension_shapes.get(shape_name.trim()).copied();
        let shape = if let Some(u) = &user_shape {
            u.base.clone()
        } else if library_shape.is_some() {
            // computed as a rectangle, then replaced by what the library's shape gives
            "rectangle".to_string()
        } else if shapes::is_shape(&shape_name) {
            shape_name.clone()
        } else {
            self.warn(format!("shape '{shape_name}' not supported: drawn as a rectangle"));
            "rectangle".to_string()
        };
        self.process_keys(tikra_tokens(&format!("every {shape_name} node/.try")), Some("/tikz"))?;
        let name = spec.name.clone().or_else(|| self.st.node.name.clone()).or_else(|| self.set_member_name());
        // what the system layer's animations give the node at the snapshot time
        if let Some(options) = name.as_ref().and_then(|n| self.sys_animations.remove(n)) {
            self.process_keys(options, Some("/tikz"))?;
        }

        // the text boxes, one per node part
        let mut texts = Vec::new();
        // expanded first: a macro may hold the `\nodepart`s
        // with what `execute at begin node` and `execute at end node` put around it
        let body = |e: &Engine, name: &str| match e.tex.meaning_of(name) {
            tex::Meaning::Macro(m) => tex::substitute(&m.body, &[]),
            _ => Vec::new(),
        };
        let mut text_tokens = body(self, "tikz@atbegin@node");
        text_tokens.extend(spec.text.clone());
        text_tokens.extend(body(self, "tikz@atend@node"));
        let expanded = self.expand_text(text_tokens)?;
        for (part, tokens) in split_parts(&expanded) {
            texts.push((part, tex::tokens_to_text(&tokens).trim().to_string()));
        }
        let text = texts[0].1.clone();
        let text_width = {
            let t = self.key_text("/tikz/text width");
            if t.trim().is_empty() {
                None
            } else {
                Some(self.length(&t)?)
            }
        };
        let align = match self.key_text("/tikz/align").trim() {
            // (the flush ones in capitals: they break lines otherwise)
            "right" => 'r',
            "flush right" => 'R',
            "center" => 'c',
            "flush center" => 'C',
            "left" => 'l',
            "flush left" => 'L',
            "justify" => 'j',
            "none" if text_width.is_some() => 'j',
            _ if text_width.is_some() => 'l',
            _ => 'c',
        };
        let color = self.st.text_paint_color();
        let font = self.st.font.clone();
        let mut labels: Vec<(String, Option<Label>)> = Vec::new();
        let mut boxes: Vec<(String, BoxSize)> = Vec::new();
        // a text that is a shading (`\pgfuseshading{name}`): its box, drawn where the text goes
        self.text_shading = None;
        if let Some(name) = texts.first().and_then(|(_, t)| t.strip_prefix("\\pgfuseshading")).map(|n| n.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string()) {
            if let Some((w, h)) = self.shading_half_size(&name) {
                self.text_shading = Some((name, w, h));
                texts[0].1.clear();
            }
        }
        // a text that is a picture: its box (its baseline at its bottom), drawn where the text goes
        self.text_picture = None;
        if texts.len() == 1 {
            if let Some((drawn, lo, hi)) = self.text_image(&texts[0].1)? {
                self.text_picture = Some((drawn, lo, hi, 0.0));
                texts[0].1.clear();
            } else if let Some(b) = self.text_box(&texts[0].1) {
                // a saved box (`\usebox{\name}`): where it was set, its baseline where it was
                let origin = Point::new(0.0, b.baseline);
                let (lo, hi) = (b.canvas.min.unwrap_or(origin), b.canvas.max.unwrap_or(origin));
                self.text_picture = Some((b.canvas.items, lo, hi, b.baseline - lo.y));
                texts[0].1.clear();
            }
        }
        let whole = texts.first().map(|(_, t)| t.trim().to_string()).unwrap_or_default();
        let tikz = whole.strip_prefix("\\tikz").is_some_and(|r| !r.starts_with(|c: char| c.is_ascii_alphabetic()));
        if texts.len() == 1 && (tikz || (whole.starts_with("\\begin{tikzpicture}") && whole.ends_with("\\end{tikzpicture}"))) {
            if let Some((drawn, lo, hi)) = self.inner_picture(spec.text.clone())? {
                self.text_picture = Some((drawn, lo, hi, 0.0));
                texts[0].1.clear();
            }
        }
        // a text that is all a `\parbox{4cm}{…}` or a `minipage`: a paragraph that wide, as a text
        // width sets it, justified unless it says otherwise (`\centering`)
        let (mut text_width, mut align) = (text_width, align);
        if let Some((width, body)) = texts.first().filter(|_| texts.len() == 1).and_then(|(_, t)| label::paragraph_box(t)) {
            if let Ok(w) = self.length(&width) {
                let first = body.trim_start();
                align = [("\\centering", 'C'), ("\\raggedright", 'L'), ("\\raggedleft", 'R')].iter().find(|(c, _)| first.starts_with(c)).map_or('j', |(_, a)| *a);
                text_width = Some(w);
                texts[0].1 = body;
            }
        }
        for (k, (part, t)) in texts.iter().enumerate() {
            let lbl = if shape == "coordinate" || t.is_empty() {
                None
            } else {
                let mut warnings = Vec::new();
                // `every lower node part/.style={font=…}`: the part's own font and color
                self.begin_group();
                let r = self.process_keys(tikra_tokens(&format!("every {part} node part/.try")), Some("/tikz"));
                let (font, color) =
                    if self.st.font != font || self.st.text_paint_color() != color { (self.st.font.clone(), self.st.text_paint_color()) } else { (font.clone(), color) };
                self.end_group();
                r?;
                // a text that is all `\textcolor{color}{…}` is in that color
                let (t, color) = match whole_textcolor(t).and_then(|(c, inner)| self.color(&c).map(|c| (inner, c))) {
                    Some((inner, c)) => (inner, c),
                    None => (t.to_string(), color),
                };
                let t = self.inline_colors(&t);
                let l = label::compose_label(&t, &font, text_width, align, color, &mut warnings);
                for w in warnings {
                    self.warn(w);
                }
                Some(l)
            };
            let (mut wd, mut ht, mut dp) = match &lbl {
                Some(l) => (text_width.unwrap_or_else(|| l.width()), l.height(), l.depth()),
                // an empty text is as wide as the text width all the same
                None if shape != "coordinate" => (text_width.unwrap_or(0.0), 0.0, 0.0),
                None => (0.0, 0.0, 0.0),
            };
            // text height and text depth replace the box's
            for (key, target) in [("/tikz/text height", &mut ht), ("/tikz/text depth", &mut dp)] {
                let v = self.key_text(key);
                if !v.trim().is_empty() {
                    *target = self.length(&v)?;
                }
            }
            if k == 0 {
                if let Some((_, w, h)) = &self.text_shading {
                    (wd, ht, dp) = (2.0 * w, 2.0 * h, 0.0);
                }
                if let Some((_, lo, hi, depth)) = &self.text_picture {
                    (wd, ht, dp) = (hi.x - lo.x, hi.y - lo.y - depth, *depth);
                }
                if let Some(b) = self.forced_box.take() {
                    (wd, ht, dp) = b;
                }
                if let Some((w, h)) = self.st.node.fit {
                    // fit: the box is the fitted area, centered on the baseline
                    (wd, ht, dp) = (w, h / 2.0, h / 2.0);
                }
            }
            boxes.push((part.clone(), BoxSize { wd, ht, dp }));
            labels.push((part.clone(), lbl));
        }
        if let Some(p) = self.st.node.through {
            // the through library: as wide as twice the distance from the node's position
            let at = self.st.node.at.or(spec.at).unwrap_or(spec.point);
            let size = format!("{}pt", 2.0 * p.minus(at).norm() / PT);
            for key in ["/pgf/minimum width", "/pgf/minimum height"] {
                self.tex.define_string(&keys::macro_name(key), &size, false);
            }
        }
        let xsep = self.key_length("/pgf/inner xsep", 0.3333 * 0.3515);
        let ysep = self.key_length("/pgf/inner ysep", 0.3333 * 0.3515);
        let min_w = self.key_length("/pgf/minimum width", PT);
        let min_h = self.key_length("/pgf/minimum height", PT);
        let ox = self.key_length("/pgf/outer xsep", 0.5 * self.st.line_width);
        let oy = self.key_length("/pgf/outer ysep", 0.5 * self.st.line_width);
        let aspect = self.key_text("/pgf/aspect").trim().parse::<f64>().ok().filter(|a| *a > 0.0).unwrap_or(1.0);
        let ex = 0.43 * self.st.font_size * PT;
        let line_width = self.st.line_width;
        let compute = |e: &mut Engine, pointer: Option<Point>| {
            let mut keys = EngineKeys(e);
            let mut input = shapes::Input {
                parts: &boxes,
                inner_xsep: xsep,
                inner_ysep: ysep,
                minimum_width: min_w,
                minimum_height: min_h,
                outer_xsep: ox,
                outer_ysep: oy,
                aspect,
                ex,
                line_width,
                pointer,
                keys: &mut keys,
            };
            shapes::compute(&shape, &mut input).expect("a known shape")
        };
        let mut geometry = compute(self, None);
        // a logic gate's anchors with the other `logic gate anchors use bounding box`, which
        // pgf reads when an anchor is asked for: under `@gatebox=true name` or `@gatebox=false name`
        if shape.contains(" gate ") {
            let flag = Token::cs("iftikra@gatebox");
            let saved = self.tex.meaning(&flag);
            let on = saved == self.tex.meaning_of("iftrue");
            let other = self.tex.meaning_of(if on { "iffalse" } else { "iftrue" });
            self.tex.set_meaning(&flag, other, false);
            let g = compute(self, None);
            self.tex.set_meaning(&flag, saved, false);
            geometry.anchors.extend(g.anchors.into_iter().map(|(n, p)| (format!("@gatebox={} {n}", !on), p)));
        }
        if let Some(u) = &user_shape {
            let (wd, ht, dp) = boxes.first().map(|(_, b)| (b.wd, b.ht, b.dp)).unwrap_or_default();
            let input = super::user_shapes::ShapeInput { text: (wd, ht, dp), minimum: Point::new(min_w, min_h), inner: Point::new(xsep, ysep), outer: Point::new(ox, oy) };
            geometry = self.user_shape_geometry(shape_name.trim(), u, geometry, input)?;
        }
        if let Some(f) = library_shape {
            let text = boxes.first().map(|(_, b)| *b).unwrap_or_default();
            // the node's own transformation, as it is placed below (without where it goes)
            let picture = if self.st.node.transform_shape { Transform { e: 0.0, f: 0.0, ..self.st.transform } } else { Transform::default() };
            let input = crate::extension::ShapeInput {
                name: shape_name.trim().to_string(),
                width: text.wd,
                height: text.ht,
                depth: text.dp,
                inner_sep: Point::new(xsep, ysep),
                line_width,
                node: name.as_ref().map(|n| format!("{}{n}", self.st.name_prefix)),
                fill: self.st.mode.fill.then(|| self.st.fill_paint_color().xcolor()),
                draw: self.st.mode.draw.then(|| self.st.stroke_color().xcolor()),
                transform: picture.compose(&self.st.pending),
            };
            geometry = library_geometry(f(&mut crate::extension::Context { e: self }, &input)?);
        }
        let mut info = NodeInfo { shape: shape.clone(), geometry: Rc::new(geometry), transform: Transform::default() };

        // where: `at`, positioning, the path's point or a point along its last segment
        let pos = self.st.node.pos.or(spec.default_pos);
        let (target, direction) = match (self.st.node.at.or(spec.at), pos) {
            (Some(p), _) => (p, Point::new(1.0, 0.0)),
            (None, Some(t)) => spec.place.at(t),
            (None, None) => (spec.point, Point::new(1.0, 0.0)),
        };
        let mut anchor = self.key_text("/tikz/anchor");
        if let (Some(side), true, Some(_)) = (self.st.auto, anchor.trim() == "center", pos) {
            // auto: to the left of the segment (right with swap), by its direction
            anchor = auto_anchor(direction, (side == 'r') != self.st.swap).to_string();
        }
        let anchor = anchor.trim();
        // pgfplots' `near ticklabel` anchors face the axis
        let align = crate::keys::value(&self.tex, "/pgfplots/near ticklabel align").map(|v| crate::tex::tokens_to_string(&v)).unwrap_or_default();
        let near = self.frame.as_ref().and_then(|f| f.near_point(anchor, &self.st.pending, &align, &|n| info.anchor_local(n)));
        let a_local = near.or_else(|| info.anchor_local(anchor)).unwrap_or_else(|| {
            self.warn(format!("unknown anchor '{anchor}': center used"));
            info.geometry.center
        });
        // the picture's transformation keeps only its translation, unless transform shape
        let mut linear = Transform::default();
        if self.st.node.transform_shape {
            linear = Transform { e: 0.0, f: 0.0, ..self.st.transform };
        }
        // under a nonlinear transformation the node goes where its point goes; with `transform
        // shape nonlinear` it is also turned and scaled as the transformation bends the picture there
        let mut target = target;
        if !self.st.nonlinear.is_empty() {
            let moved = self.nonlinear_point(target)?;
            if self.st.node.transform_shape_nonlinear {
                let h = 0.01;
                let dx = self.nonlinear_point(target.plus(Point::new(h, 0.0)))?.minus(moved).scale(1.0 / h);
                let dy = self.nonlinear_point(target.plus(Point::new(0.0, h)))?.minus(moved).scale(1.0 / h);
                linear = Transform { a: dx.x, b: dx.y, c: dy.x, d: dy.y, e: 0.0, f: 0.0 }.compose(&linear);
            }
            target = moved;
        }
        if self.st.node.sloped {
            let angle = direction.y.atan2(direction.x).to_degrees();
            let angle = if !self.st.allow_upside_down && (angle > 90.0 + 1e-9 || angle < -90.0 - 1e-9) { angle + 180.0 } else { angle };
            linear = linear.compose(&Transform::rotation(angle));
        }
        let t = Transform::translation(target.x, target.y).compose(&linear).compose(&self.st.pending).compose(&Transform::translation(-a_local.x, -a_local.y));
        info.transform = t;
        // a callout's absolute pointer is in the picture: once the node is placed, the shape is
        // computed again with it in node coordinates (its other anchors do not depend on it)
        if shape.ends_with("callout") {
            if let Some(p) = shapes::Keys::point(&mut EngineKeys(self), "/pgf/callout pointer@absolute") {
                let local = t.inverse().apply(p);
                info.geometry = Rc::new(compute(self, Some(local)));
            }
        }
        let limit = super::path::MAX_COORDINATE;
        let fine = |p: Point| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= limit && p.y.abs() <= limit;
        if !fine(info.center()) || !info.geometry.anchors.iter().all(|(_, p)| fine(t.apply(*p))) {
            self.warn(format!("node '{text}': position or size is not finite or beyond ±{limit} cm: ignored"));
            return Ok((Vec::new(), None));
        }
        // `name path` among the node's own options names its border
        if let Some(n) = self.st.name_path.clone().filter(|_| self.st.name_path != outer_name_path) {
            let g = &info.geometry;
            let border =
                g.paths.iter().filter(|p| p.layer == Layer::Background && matches!(p.usage, Usage::Node)).flat_map(|p| transform_segments(&p.segments, &info.transform)).collect();
            self.named_paths.insert(n, border);
        }
        let size = (self.canvas.min, self.canvas.max);
        let items = self.draw_shape(&info, labels, text_width, scope)?;
        if self.st.position_later {
            // kept for `\pgfpositionnodenow`: not in the picture, nor its size, nor known by name yet
            (self.canvas.min, self.canvas.max) = size;
            self.later_node = Some((items, info, name.map(|n| format!("{}{n}", self.st.name_prefix))));
            return Ok((Vec::new(), None));
        }
        let key = if let Some(n) = &name {
            // inside a pic, names get its prefix
            let n = format!("{}{n}", self.st.name_prefix);
            self.tex.define_string("tikzlastnode", &n, true);
            self.join_sets(&n);
            self.nodes.insert(n.clone(), info.clone());
            n
        } else {
            // an unnamed node is `\tikzlastnode` by an internal name, as TikZ's `tikz@f@…`
            let n = format!("tikz@f@{}", self.nodes.len());
            self.tex.define_string("tikzlastnode", &n, true);
            self.nodes.insert(n.clone(), info.clone());
            n
        };
        for n in chain_names.into_iter().chain(std::mem::take(&mut self.st.aliases)) {
            self.nodes.insert(n, info.clone());
        }
        // the last node, for trees (labels below do not count)
        let registered = name.as_ref().map(|n| format!("{}{n}", self.st.name_prefix));
        if let Some(n) = &registered {
            self.canvas.named.push((n.clone(), info.center()));
        }
        if self.facts.mute == 0 && shape != "coordinate" {
            // with the texts a library's shape draws (a chip's pin names)
            let own = info.geometry.texts.iter().filter(|t| t.anchor.is_some()).map(|t| t.tex.as_str());
            let all: Vec<&str> = texts.iter().map(|(_, t)| t.as_str()).chain(own).filter(|t| !t.trim().is_empty()).collect();
            self.facts.nodes.push(crate::describe::NodeFact { key, named: registered.is_some(), text: all.join(" "), at: info.center(), on_link: false, source: self.source });
        }
        if let Some(slot @ None) = self.tree_child_names.last_mut() {
            *slot = Some(registered.clone().unwrap_or_default());
        }
        let last = (Some(info.clone()), registered);
        (self.last_node, self.last_node_name) = last.clone();
        let labels = std::mem::take(&mut self.st.node.labels);
        let kept = LABEL_KEYS.iter().map(|k| (*k, self.tex.meaning_of(&keys::macro_name(k)))).collect();
        Ok((items, Some((info, labels, last, kept))))
    }

    /// What a node draws, in pgf's order: behind the background, the background (as the node's
    /// options say), before it, the texts of the parts, behind the foreground, the foreground and
    /// before it. Every path counts in the bounding box, as pgf protocols paths when they are made.
    fn draw_shape(&mut self, info: &NodeInfo, labels: Vec<(String, Option<Label>)>, text_width: Option<f64>, scope: &super::GState) -> Result<Vec<Item>, Error> {
        let t = info.transform;
        let g = info.geometry.clone();
        let mut items = Vec::new();
        // a shaded node: its shading fitted to the background path, as \pgfshadepath does
        let background: Vec<Segment> =
            g.paths.iter().filter(|p| p.layer == Layer::Background && matches!(p.usage, Usage::Node)).flat_map(|p| transform_segments(&p.segments, &t)).collect();
        let node_fill = if self.st.mode.shade && !background.is_empty() {
            Some(Fill { paint: Paint::Shading(self.shading_of(&background)), opacity: self.st.fill_opacity, even_odd: self.st.even_odd })
        } else if self.st.mode.fill {
            Some(Fill { paint: Paint::Solid(self.st.fill_paint_color()), opacity: self.st.fill_opacity, even_odd: self.st.even_odd })
        } else if self.st.mode.pattern {
            Some(Fill {
                paint: {
                    let (name, color) = (self.st.pattern_name.clone().unwrap_or_default(), self.st.pattern_color.unwrap_or(crate::color::Color::BLACK));
                    self.pattern_paint(&name, color).unwrap_or(Paint::Pattern { name, color })
                },
                opacity: self.st.fill_opacity,
                even_odd: false,
            })
        } else {
            None
        };
        let node_stroke = self.st.mode.draw.then(|| self.stroke());
        let layer_items = |e: &mut Engine, layers: &[Layer], items: &mut Vec<Item>| -> Result<(), Error> {
            for p in g.paths.iter().filter(|p| layers.contains(&p.layer)) {
                let mut segs = p.segments.clone();
                if p.layer == Layer::Background && g.roundable && (e.st.rounded != 0.0 || e.st.rounded_out != 0.0) {
                    segs = super::output::round_corners(&segs, e.st.rounded, e.st.rounded_out);
                }
                let segs = transform_segments(&segs, &t);
                if matches!(p.usage, Usage::Node)
                    && p.layer == Layer::Background
                    && (e.st.decorate || e.st.mode.picture.is_some() || !e.st.preactions.is_empty() || !e.st.postactions.is_empty() || (e.st.double.is_some() && e.st.mode.draw))
                {
                    // the background as a path of its own, decorated, with its actions, or double
                    let saved = std::mem::take(&mut e.canvas.items);
                    let r = e.finish_path(segs, scope, "node");
                    items.extend(std::mem::replace(&mut e.canvas.items, saved));
                    r?;
                    continue;
                }
                let (fill, stroke) = match &p.usage {
                    Usage::Node => (node_fill.clone(), node_stroke.clone()),
                    Usage::Fill(c) => {
                        let color = e.color(c).unwrap_or(crate::color::Color::WHITE);
                        (Some(Fill { paint: Paint::Solid(color), opacity: e.st.fill_opacity, even_odd: false }), None)
                    }
                    Usage::Stroke => (None, Some(e.stroke())),
                    Usage::TipStroke(cap, join) => (None, Some(crate::canvas::Stroke { cap: *cap, join: *join, ..e.stroke() })),
                    Usage::LineFill => (Some(Fill { paint: Paint::Solid(e.st.stroke_color()), opacity: e.st.fill_opacity, even_odd: false }), None),
                    Usage::Library { paint, color, dash, cap, opacity: own_opacity } => {
                        let own = color.as_deref().and_then(|c| {
                            let found = e.color(c);
                            if found.is_none() {
                                e.warn(format!("shape color '{c}' not known: the node's used"));
                            }
                            found
                        });
                        let opacity = own_opacity.unwrap_or(e.st.fill_opacity);
                        let solid = |c: crate::color::Color| Fill { paint: Paint::Solid(c), opacity, even_odd: false };
                        match paint {
                            ShapePaint::Stroke(w) => {
                                let base = e.stroke();
                                let stroke = crate::canvas::Stroke {
                                    width: *w,
                                    color: own.unwrap_or(base.color),
                                    dash: dash.as_ref().map(|d| (d.clone(), 0.0)).or(base.dash.clone()),
                                    cap: cap.map_or(base.cap, |c| match c {
                                        LineCap::Butt => crate::canvas::Cap::Butt,
                                        LineCap::Round => crate::canvas::Cap::Round,
                                        LineCap::Rect => crate::canvas::Cap::Square,
                                    }),
                                    ..base
                                };
                                (None, Some(stroke))
                            }
                            ShapePaint::Fill => (node_fill.clone().map(|f| own.map_or(f, solid)), None),
                            ShapePaint::Background => (Some(own.map(solid).or_else(|| node_fill.clone()).unwrap_or_else(|| solid(crate::color::Color::WHITE))), None),
                            ShapePaint::Solid => (Some(solid(own.unwrap_or_else(|| e.st.stroke_color()))), None),
                        }
                    }
                };
                if e.counts_for_size() {
                    e.canvas.include_path(&segs);
                    if let Some(s) = &stroke {
                        // \pgfusepath adds half the line width around a stroked path
                        e.canvas.include_path_margin(&segs, s.width / 2.0);
                    }
                }
                if fill.is_some() || stroke.is_some() {
                    items.push(Item::Path { segments: segs, fill, stroke });
                }
            }
            Ok(())
        };
        layer_items(self, &[Layer::BehindBackground, Layer::Background, Layer::BeforeBackground], &mut items)?;
        if self.counts_for_size() {
            for p in &g.extent {
                self.canvas.include(t.apply(*p));
            }
        }
        if let Some((drawn, lo, _, depth)) = self.text_picture.take() {
            if let Some(origin) = g.parts.iter().find(|(n, _)| n == "text").map(|(_, p)| *p) {
                let place = t.compose(&Transform::translation(origin.x - lo.x, origin.y - lo.y - depth));
                items.push(Item::Transform { transform: place });
                items.extend(drawn);
                items.push(Item::End);
            }
        }
        if let Some((name, w, h)) = self.text_shading.take() {
            if let Some(origin) = g.parts.iter().find(|(n, _)| n == "text").map(|(_, p)| *p) {
                let saved = std::mem::take(&mut self.canvas.items);
                let r = self.draw_shading(&name, &t.compose(&Transform::translation(origin.x + w, origin.y + h)));
                items.extend(std::mem::replace(&mut self.canvas.items, saved));
                r?;
            }
        }
        for (part, lbl) in labels {
            let (Some(l), Some(origin)) = (lbl, g.parts.iter().find(|(n, _)| *n == part).map(|(_, p)| *p)) else { continue };
            let offset = match (text_width, l.align) {
                (Some(tw), 'c') => (tw - l.width()) / 2.0,
                (Some(tw), 'r') => tw - l.width(),
                _ => 0.0,
            };
            let place = t.compose(&Transform::translation(origin.x + offset, origin.y));
            items.push(Item::Text { label: l, place, opacity: self.st.text_opacity });
        }
        for st in &g.texts {
            let (text, own) = self.shape_text(&st.tex);
            let color = own.or(st.color.clone()).and_then(|c| self.color(&c)).unwrap_or_else(|| self.st.text_paint_color());
            if text.trim().is_empty() {
                continue;
            }
            let mut warnings = Vec::new();
            let text = self.inline_colors(text.trim());
            let font = st.font.clone().unwrap_or_else(|| self.st.font.clone());
            let l = label::compose_label(&text, &font, None, 'c', color, &mut warnings);
            for w in warnings {
                self.warn(w);
            }
            let origin = match st.anchor {
                Some(a) => st.at.minus(text_anchor(a, &l, &font)),
                None => st.at,
            };
            // turned around its point, less the node's own turn when it stays upright
            let mut turn = st.turn;
            if st.upright && t.a * t.d - t.b * t.c > 0.0 {
                turn -= t.b.atan2(t.a).to_degrees();
            }
            let place = if turn.abs() < 1e-9 {
                t.compose(&Transform::translation(origin.x, origin.y))
            } else {
                let d = origin.minus(st.at);
                t.compose(&Transform::translation(st.at.x, st.at.y)).compose(&Transform::rotation(turn)).compose(&Transform::translation(d.x, d.y))
            };
            // a library's texts count in the picture's size, as TeX's boxes do
            if st.anchor.is_some() && self.counts_for_size() {
                for (x, y) in [(0.0, -l.depth()), (l.width(), -l.depth()), (0.0, l.height()), (l.width(), l.height())] {
                    self.canvas.include(place.apply(Point::new(x, y)));
                }
            }
            items.push(Item::Text { label: l, place, opacity: self.st.text_opacity });
        }
        layer_items(self, &[Layer::BehindForeground, Layer::Foreground, Layer::BeforeForeground], &mut items)?;
        Ok(items)
    }

    /// A shape's own text, expanded, with `\char`\&` as its character and a leading `\color{…}` taken
    /// out as its color.
    fn shape_text(&mut self, tex: &str) -> (String, Option<String>) {
        let mut text = self.tex.expand_to_string(tikra_tokens(tex)).unwrap_or_else(|_| tex.to_string());
        while let Some(i) = text.find("\\char") {
            let rest = text[i + 5..].trim_start();
            let rest = rest.strip_prefix('`').unwrap_or(rest);
            let rest = rest.strip_prefix('\\').unwrap_or(rest);
            let Some(ch) = rest.chars().next() else { break };
            let after = rest[ch.len_utf8()..].to_string();
            let ch = if "&%$#_{}".contains(ch) { format!("\\{ch}") } else { ch.to_string() };
            text = format!("{}{ch}{after}", &text[..i]);
        }
        let mut color = None;
        if let Some(rest) = text.trim_start().strip_prefix("\\color") {
            let rest = rest.trim_start();
            if let Some(r) = rest.strip_prefix('{') {
                if let Some(j) = r.find('}') {
                    color = Some(r[..j].to_string());
                    text = r[j + 1..].to_string();
                }
            }
        }
        (text, color)
    }

    /// `label=[options]angle:text`, `pin=…`: another node next to this one.
    pub(crate) fn label(&mut self, info: &NodeInfo, spec: Vec<Token>, pin: bool) -> Result<Vec<Item>, Error> {
        let mut v = keys::trim(&spec).to_vec();
        let mut opts = Vec::new();
        if v.first().is_some_and(|t| t.is_char('[')) {
            // the `]` that closes the first `[`
            let mut depth = 0usize;
            let end = v.iter().position(|t| {
                if t.is_char('[') {
                    depth += 1;
                } else if t.is_char(']') {
                    depth -= 1;
                }
                depth == 0
            });
            let Some(end) = end else {
                self.warn(format!("label '{}': options without ']': label ignored", tex::tokens_to_string(&v)));
                return Ok(Vec::new());
            };
            opts = v[1..end].to_vec();
            v = v[end + 1..].to_vec();
        }
        let s = tex::tokens_to_string(&v);
        // the angle is up to the first top-level colon
        let (angle_text, text_tokens) = match split_colon(&v) {
            // (expanded: `pin=\angle+15:text` is at the angle the macro gives)
            Some((a, b)) => (self.math_string(a)?.trim().to_string(), b),
            None => (String::new(), v.clone()),
        };
        let _ = s;
        // with `direction shorthands` (the quotes library's labels, `"text" above`), a direction
        // among the label's options is its position; otherwise it places the label's anchor
        let shorthands = keys::split_list(&opts).iter().any(|i| i.key == "direction shorthands");
        let directions: &[&str] =
            if shorthands { &["above", "below", "left", "right", "above left", "above right", "below left", "below right", "centered", "direction shorthands"] } else { &[] };
        let mut given = None;
        // `label position` among the label's options is its default position (`\tikz@label@angle`
        // expands it after them)
        let distance_key = if pin { "pin distance" } else { "label distance" };
        let mut given_distance = None;
        for i in keys::split_list(&opts) {
            if i.key == "label position" {
                if let Some(v) = &i.value {
                    given = Some(tex::tokens_to_string(v).trim().to_string());
                }
            }
            // and the distance among them is read after them too
            if i.key == distance_key {
                if let Some(v) = i.value {
                    given_distance = self.length_of(v).ok();
                }
            }
        }
        let items = keys::split_list(&opts);
        let taken = |i: &keys::Item| i.value.is_none() && directions.contains(&i.key.as_str());
        if items.iter().any(taken) {
            let mut rest = Vec::new();
            for i in items {
                if i.value.is_none() && directions.contains(&i.key.as_str()) {
                    if i.key != "direction shorthands" {
                        given = Some(i.key.clone());
                    }
                } else {
                    if !rest.is_empty() {
                        rest.push(Token::other(','));
                    }
                    rest.extend(tikra_tokens(&i.key));
                    if let Some(v) = i.value {
                        rest.push(Token::other('='));
                        rest.push(Token::Char('{', tex::Cat::Begin));
                        rest.extend(v);
                        rest.push(Token::Char('}', tex::Cat::End));
                    }
                }
            }
            opts = rest;
        }
        let position = if !angle_text.is_empty() {
            angle_text
        } else if let Some(g) = given {
            g
        } else {
            self.key_text("/tikz/label position").trim().to_string()
        };
        let angle = match position.as_str() {
            "above" | "north" => 90.0,
            "below" | "south" => 270.0,
            "left" | "west" => 180.0,
            "right" | "east" => 0.0,
            "above left" | "north west" => 135.0,
            "above right" | "north east" => 45.0,
            "below left" | "south west" => 225.0,
            "below right" | "south east" => 315.0,
            "center" | "centered" => f64::NAN,
            other => self.number(other).unwrap_or(90.0),
        };
        let distance = match given_distance {
            Some(d) => d,
            None if pin => self.key_length("/tikz/pin distance", 3.0 * 0.1515),
            None => self.key_length("/tikz/label distance", 0.0),
        };
        // as `tikz@label@post`: at the node's anchor of that (whole) angle, moved away from the
        // center by the distance, with the anchor on the side away from the node
        let absolute = matches!(self.tex.meaning_of("tikra@absolute"), tex::Meaning::Macro(m) if tex::tokens_to_string(&tex::substitute(&m.body, &[])).trim() == "true");
        let (at, anchor) = if angle.is_nan() {
            (info.center(), "center".to_string())
        } else if absolute {
            // `absolute`: the angle on the page, whatever the node's transformation
            let u = Point::new(angle.to_radians().cos(), angle.to_radians().sin());
            let border = info.border_toward(info.center().plus(u.scale(PT)));
            (border.plus(u.scale(distance)), direction_anchor(angle).to_string())
        } else {
            let border = info.anchor(&format!("{}", angle.trunc())).unwrap_or(info.center());
            let d = border.minus(info.center());
            if d.norm() < 1e-9 {
                // a node without size (a coordinate): straight out at the angle
                (border.plus(Point::new(angle.to_radians().cos(), angle.to_radians().sin()).scale(distance)), direction_anchor(angle).to_string())
            } else {
                let u = d.scale(1.0 / d.norm());
                (border.plus(u.scale(distance)), auto_anchor(Point::new(u.y, -u.x).scale(PT), false).to_string())
            }
        };
        let mut options = tikra_tokens(&format!("every {}/.try,anchor={anchor},", if pin { "pin" } else { "label" }));
        options.extend(opts);
        // `pin edge={…}` among the label's options or styles sets the options of its edge, the
        // last one given winning
        self.tex.set_meaning(&Token::cs("tikra@pinedge"), super::macro_from(""), true);
        let items = self.node(NodeSpec { options, name: None, at: Some(at), text: text_tokens, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None })?;
        if pin {
            // the pin edge: an edge from the node to the pin's label (arrows, decorations…)
            let Some(label) = self.last_node.clone() else { return Ok(items) };
            let edge_options = super::spy::macro_body(self, "tikra@pinedge");
            let saved = (self.last_node.clone(), self.last_node_name.clone());
            self.nodes.insert("tikra@pin@node".into(), info.clone());
            self.nodes.insert("tikra@pin@label".into(), label);
            self.begin_group();
            self.st.transform = Transform::default();
            let mut path = tikra_tokens("\\path (tikra@pin@node) edge[every pin edge/.try,");
            path.extend(edge_options);
            path.extend(tikra_tokens("] (tikra@pin@label);"));
            let r = self.run_tokens(path);
            self.end_group();
            r?;
            (self.last_node, self.last_node_name) = saved;
            return Ok(items);
        }
        Ok(items)
    }

    /// The stroke of the current state.
    pub fn stroke(&self) -> Stroke {
        Stroke {
            color: self.st.stroke_color(),
            width: self.st.line_width,
            dash: self.st.dash.clone().filter(|d| !d.0.is_empty()),
            cap: self.st.cap,
            join: self.st.join,
            miter_limit: self.st.miter_limit,
            opacity: self.st.draw_opacity,
        }
    }
}

/// TikZ's `\tikz@auto@anchor` (left of the direction) and `\tikz@auto@anchor@prime` (right).
fn auto_anchor(d: Point, right: bool) -> &'static str {
    let e = 0.05 * PT;
    let (x, y) = (d.x, d.y);
    let left = if x > e {
        if y > e {
            "south east"
        } else if y < -e {
            "south west"
        } else {
            "south"
        }
    } else if x < -e {
        if y > e {
            "north east"
        } else if y < -e {
            "north west"
        } else {
            "north"
        }
    } else if y > 0.0 {
        "east"
    } else {
        "west"
    };
    if !right {
        return left;
    }
    match left {
        "south east" => "north west",
        "south west" => "north east",
        "south" => "north",
        "north east" => "south west",
        "north west" => "south east",
        "north" => "south",
        "east" => "west",
        _ => "east",
    }
}

/// `\tikz@compute@direction`: the anchor of a label at `angle` (whole degrees) away from a point.
fn direction_anchor(angle: f64) -> &'static str {
    let a = (angle.trunc() as i64).rem_euclid(360);
    match a {
        0..=3 => "west",
        4..=86 => "south west",
        87..=93 => "south",
        94..=176 => "south east",
        177..=183 => "east",
        184..=266 => "north east",
        267..=273 => "north",
        274..=356 => "north west",
        _ => "west",
    }
}

/// `\color{color}` in a text as `\textcolor{color}{…}` around what follows it in its group (as the
/// switch colors up to the group's end), the spaces after it dropped (xcolor's `\color` ends with
/// `\ignorespaces`): `The {\color{red} angle} is` is `The \textcolor{red}{angle} is`.
fn color_switches(text: &str) -> String {
    let mut s = text.to_string();
    let mut from = 0;
    while let Some(k) = s[from..].find("\\color{") {
        let i = from + k;
        let Some(close) = s[i + 7..].find('}') else { break };
        let spec_end = i + 7 + close;
        // the group the switch is in ends at the first `}` not closing a group opened after it; in
        // math, the formula's end ends it too
        let math = label::in_math(&s[..i]);
        let mut depth = 0usize;
        let mut end = s.len();
        let after = &s[spec_end + 1..];
        for (j, ch) in after.char_indices() {
            let closes_math = math && depth == 0 && (ch == '$' || after[j..].starts_with("\\]") || after[j..].starts_with("\\)"));
            match ch {
                _ if closes_math => {
                    end = spec_end + 1 + j;
                    break;
                }
                '{' => depth += 1,
                '}' if depth == 0 => {
                    end = spec_end + 1 + j;
                    break;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        let body = s[spec_end + 1..end].trim_start().to_string();
        let spec = s[i + 7..spec_end].to_string();
        s = format!("{}\\textcolor{{{spec}}}{{{body}}}{}", &s[..i], &s[end..]);
        from = i + 1;
    }
    s
}

/// `\textcolor{color}{text}` as the whole text, or `\color{color}` before it: (color, text).
fn whole_textcolor(t: &str) -> Option<(String, String)> {
    let group = |s: &str| -> Option<(String, usize)> {
        let s2 = s.strip_prefix('{')?;
        let mut depth = 1;
        // (`\{` and `\}` are characters, not braces)
        let mut escaped = false;
        // (the text is made of TeX's tokens, whose braces pair up)
        let (i, _) = s2.char_indices().find(|&(_, ch)| {
            if std::mem::take(&mut escaped) {
                return false;
            }
            match ch {
                '\\' => escaped = true,
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            depth == 0
        })?;
        Some((s2[..i].to_string(), i + 2))
    };
    // `\color{…}` first: the rest of the text in that color
    if let Some(rest) = t.trim().strip_prefix("\\color").map(str::trim_start) {
        let (c, n) = group(rest)?;
        return Some((c, rest[n..].trim().to_string()));
    }
    let rest = t.trim().strip_prefix("\\textcolor")?.trim_start();
    let (c, n) = group(rest)?;
    let rest = rest[n..].trim_start();
    let (inner, m) = group(rest)?;
    rest[m..].trim().is_empty().then_some((c, inner))
}

/// `30:text` → (`30`, `text`): split at the first colon outside braces.
fn split_colon(v: &[Token]) -> Option<(Vec<Token>, Vec<Token>)> {
    let mut depth = 0usize;
    for (i, t) in v.iter().enumerate() {
        match t {
            Token::Char(_, tex::Cat::Begin) => depth += 1,
            Token::Char(_, tex::Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(':', _) if depth == 0 => return Some((v[..i].to_vec(), tex::strip_outer_braces(keys::trim(&v[i + 1..]).to_vec()))),
            _ => {}
        }
    }
    None
}

impl Engine {
    /// `\textcolor{color}{text}` inside a node's text, its color resolved for the math typesetter
    /// (which colors only in math): `$\textcolor[HTML]{rrggbb}{\text{text}}$`.
    /// `\pgfdeclareimage[options]{name}{file}`: the image `\pgfuseimage{name}` puts in a text.
    pub(super) fn declare_image(&mut self) -> Result<(), Error> {
        let options = self.optional()?.map(|o| tex::tokens_to_string(&o)).unwrap_or_default();
        let name = self.argument_string()?;
        let file = self.argument_string()?;
        let name = name.trim();
        self.tex.define_string(&format!("tikra@image@{name}@options"), &options, false);
        self.tex.define_string(&format!("tikra@image@{name}@file"), file.trim(), false);
        Ok(())
    }

    /// A text that is an image (`\pgfuseimage{name}`, `\pgfimage[options]{file}`,
    /// `\includegraphics[options]{file}`): what it draws from its box's lower left corner, and the
    /// box's corners. Only PNG images are drawn; one not drawn keeps the size its options give.
    fn text_image(&mut self, text: &str) -> Result<Option<(Vec<Item>, Point, Point)>, Error> {
        let t = text.trim();
        let (options, file) = if let Some(r) = t.strip_prefix("\\pgfuseimage") {
            let name = r.trim().trim_start_matches('{').trim_end_matches('}').trim();
            let value = |e: &Engine, k: &str| match e.tex.meaning_of(&format!("tikra@image@{name}@{k}")) {
                tex::Meaning::Macro(m) => Some(tex::tokens_to_string(&tex::substitute(&m.body, &[]))),
                _ => None,
            };
            let Some(file) = value(self, "file") else {
                self.warn(format!("\\pgfuseimage: no image '{name}' declared"));
                return Ok(None);
            };
            (value(self, "options").unwrap_or_default(), file)
        } else if let Some(r) = t.strip_prefix("\\pgfimage").or_else(|| t.strip_prefix("\\includegraphics")) {
            let r = r.trim_start();
            let (options, r) = match r.strip_prefix('[').and_then(|r| r.split_once(']')) {
                Some((o, r)) => (o.to_string(), r.trim_start()),
                None => (String::new(), r),
            };
            let Some(file) = r.strip_prefix('{').and_then(|r| r.strip_suffix('}')).filter(|f| !f.contains(['{', '}'])) else { return Ok(None) };
            (options, file.trim().to_string())
        } else {
            return Ok(None);
        };
        let (mut width, mut height, mut scale) = (None, None, 1.0);
        for item in crate::keys::split_list(&tikra_tokens(&options)) {
            match (item.key.as_str(), item.value) {
                ("width", Some(v)) => width = Some(self.length_of(v)?),
                ("height", Some(v)) => height = Some(self.length_of(v)?),
                ("scale", Some(v)) => scale = self.number_of(v)?,
                ("interpolate" | "keepaspectratio", _) => {}
                (k, _) => self.warn(format!("image option '{k}' ignored")),
            }
        }
        // the `mwe` package's placeholder, which documents load as if it were a file of theirs
        if file.trim().trim_end_matches(".pdf") == "example-image-a" {
            let bp = 2.54 / 72.0;
            let (nw, nh) = (320.0 * bp, 240.0 * bp);
            let (w, h) = match (width, height) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, w * nh / nw),
                (None, Some(h)) => (h * nw / nh, h),
                (None, None) => (nw * scale, nh * scale),
            };
            return Ok(Some((self.example_image(w, h), Point::default(), Point::new(w, h))));
        }
        let image = graphics::load_png(&file);
        let natural = image.as_ref().map(|(_, n)| *n).ok();
        let (w, h) = match (width, height, natural) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some((nw, nh))) => (w, w * nh / nw),
            (None, Some(h), Some((nw, nh))) => (h * nw / nh, h),
            (None, None, Some((nw, nh))) => (nw * scale, nh * scale),
            (w, h, None) => (w.unwrap_or(0.0), h.unwrap_or(0.0)),
        };
        let mut drawn = Vec::new();
        match image {
            Ok((png, _)) => drawn.push(Item::Image { png, place: Transform { a: w, b: 0.0, c: 0.0, d: h, e: 0.0, f: 0.0 } }),
            Err(m) => self.warn(format!("image not drawn: {m}")),
        }
        Ok(Some((drawn, Point::default(), Point::new(w, h))))
    }

    /// `example-image-a` of the `mwe` package, `w` by `h` from the origin: as its PDF draws it on
    /// its 320 by 240 bp, a 75% gray box with its diagonals and middle lines in 60% gray, a frame
    /// and a 160 bp Latin Modern Sans `A`.
    fn example_image(&mut self, w: f64, h: f64) -> Vec<Item> {
        // cm per bp of the image
        let (sx, sy) = (w / 320.0, h / 240.0);
        let p = |x: f64, y: f64| Point::new(x * sx, y * sy);
        let rect = |inset: f64| {
            vec![
                Segment::MoveTo(p(inset, inset)),
                Segment::LineTo(p(inset, 240.0 - inset)),
                Segment::LineTo(p(320.0 - inset, 240.0 - inset)),
                Segment::LineTo(p(320.0 - inset, inset)),
                Segment::Close,
            ]
        };
        let base = self.stroke();
        let stroke = |gray: f64, width: f64| Stroke { color: crate::color::Color::gray(gray), width: width * sx, dash: None, ..base.clone() };
        let lines = [(0.0, 0.0, 320.0, 240.0), (320.0, 0.0, 0.0, 240.0), (160.0, 0.0, 160.0, 240.0), (0.0, 120.0, 320.0, 120.0)];
        let mut items = vec![
            Item::Path { segments: rect(0.0), fill: Some(Fill { paint: Paint::Solid(crate::color::Color::gray(0.75)), opacity: 1.0, even_odd: false }), stroke: None },
            Item::Path {
                segments: lines.iter().flat_map(|&(a, b, c, d)| [Segment::MoveTo(p(a, b)), Segment::LineTo(p(c, d))]).collect(),
                fill: None,
                stroke: Some(stroke(0.6, 0.79701)),
            },
            // (its 1.59404 bp frame is clipped to the box: the inner half)
            Item::Path { segments: rect(0.39851), fill: None, stroke: Some(stroke(0.0, 0.79702)) },
        ];
        let mut warnings = Vec::new();
        let label = label::compose_label("A", "\\sffamily", None, 'l', crate::color::Color::BLACK, &mut warnings);
        // 160 bp, from the 10 pt the label is set in
        let k = 160.0 * sx / (10.0 * PT);
        items.push(Item::Text { label, place: Transform { a: k, b: 0.0, c: 0.0, d: k, e: 110.042 * sx, f: 63.623 * sy }, opacity: 1.0 });
        items
    }

    fn inline_colors(&mut self, text: &str) -> String {
        let text = color_switches(text);
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(i) = rest.find("\\textcolor{") {
            out.push_str(&rest[..i]);
            let after = &rest[i + "\\textcolor{".len()..];
            // the text is made of TeX's tokens, whose braces pair up: the color's `{` is closed
            let end = after.find('}').expect("the color of a \\textcolor is closed");
            let spec = &after[..end];
            let body_start = &after[end + 1..];
            // the braced text after the color; unbraced, its one character or command (`\textcolor{red}x`)
            let Some(body) = body_start.strip_prefix('{') else {
                let arg = body_start.trim_start();
                let n = match arg.strip_prefix('\\') {
                    Some(cs) => {
                        let letters = cs.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(cs.len());
                        1 + if letters > 0 { letters } else { cs.chars().next().map_or(0, char::len_utf8) }
                    }
                    None => arg.chars().next().map_or(0, char::len_utf8),
                };
                if n == 0 {
                    // no text left to color: TeX stops on the missing argument
                    self.warn(format!("\\textcolor{{{spec}}} without a text to color: dropped"));
                    rest = body_start;
                    continue;
                }
                let braced = format!("\\textcolor{{{spec}}}{{{}}}{}", &arg[..n], &arg[n..]);
                out.push_str(&self.inline_colors(&braced));
                return out;
            };
            let mut depth = 1;
            let mut close = body.len();
            let mut escaped = false;
            for (k, c) in body.char_indices() {
                if std::mem::take(&mut escaped) {
                    continue;
                }
                match c {
                    '\\' => escaped = true,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            close = k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = &body[..close];
            match self.color(spec) {
                Some(c) => {
                    let hex = c.hex();
                    let hex = hex.trim_start_matches('#');
                    // in math, a color of math; math inside stays math; text goes in \text
                    if label::in_math(&out) {
                        out.push_str(&format!("\\textcolor[HTML]{{{hex}}}{{{inner}}}"));
                    } else if inner.contains('$') {
                        // the words around the math stay text
                        let parts: String =
                            inner.split('$').enumerate().filter(|(_, p)| !p.is_empty()).map(|(k, p)| if k % 2 == 1 { p.to_string() } else { format!("\\text{{{p}}}") }).collect();
                        out.push_str(&format!("$\\textcolor[HTML]{{{hex}}}{{{parts}}}$"));
                    } else {
                        out.push_str(&format!("$\\textcolor[HTML]{{{hex}}}{{\\text{{{inner}}}}}$"));
                    }
                }
                None => out.push_str(inner),
            }
            rest = body.get(close + 1..).unwrap_or("");
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::color_switches;

    #[test]
    fn a_color_switch_colors_the_rest_of_its_group() {
        assert_eq!(color_switches(r"The {\color{red} angle $\alpha$} is"), r"The {\textcolor{red}{angle $\alpha$}} is");
        assert_eq!(color_switches(r"a \color{blue}b"), r"a \textcolor{blue}{b}");
        // in math, up to the formula's end
        assert_eq!(color_switches(r"$a \color{black}= b$ c"), r"$a \textcolor{black}{= b}$ c");
        assert_eq!(color_switches(r"{\color{red}x}{\color{blue}y}"), r"{\textcolor{red}{x}}{\textcolor{blue}{y}}");
        // `\colorbox` and `\textcolor` stay
        assert_eq!(color_switches(r"\colorbox{red}{x} \textcolor{red}{y}"), r"\colorbox{red}{x} \textcolor{red}{y}");
    }
}
