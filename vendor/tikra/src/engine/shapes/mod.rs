//! pgf's node shapes, ported from `pgfmoduleshapes.code.tex` and the `shapes.*` libraries.
//!
//! A shape is computed once, when its node is made: from the sizes of the node's text boxes (one
//! per node part), `inner sep`, the minimum sizes, `outer sep` and its own keys, it gives a
//! [`Geometry`] in the node's coordinate system (the origin of the `text` part's box at (0, 0)): its
//! named anchors, the border on which lines to the node stop, where each part's text goes, and the
//! paths it draws. As in pgf, anchors and the border include `outer sep`; the paths do not.

mod arrows;
mod basic;
mod callouts;
mod ee;
mod gates;
mod geometric;
mod misc;
mod multipart;
mod symbols;

use crate::scene::{Point, Segment, Transform};

/// Width, height and depth of a node part's text box, in cm.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BoxSize {
    pub wd: f64,
    pub ht: f64,
    pub dp: f64,
}

/// The values a shape reads from pgf keys: lengths in cm.
pub trait Keys {
    /// A length key (`/pgf/star point height`), or `default` if unset or unreadable.
    fn length(&mut self, key: &str, default: f64) -> f64;
    /// A length written out (`.5cm`), or `default` if it is not one.
    fn length_of(&mut self, text: &str, default: f64) -> f64;
    /// A number key (`/pgf/star points`), evaluated.
    fn number(&mut self, key: &str, default: f64) -> f64;
    /// The text of a key, expanded (`/pgf/cylinder end fill`).
    fn text(&mut self, key: &str) -> String;
    /// A TeX conditional set by an `.is if` key: `pgfshapeborderusesincircle` for `\ifpgf…`.
    fn flag(&mut self, name: &str) -> bool;
    /// A TikZ coordinate stored in a key (`(a.east)`, `(0,-1)`), on the canvas; None if unset.
    fn point(&mut self, key: &str) -> Option<Point>;
    /// The box of a text typeset in the node's font (the symbol of an IEC gate).
    fn text_box(&mut self, _tex: &str) -> BoxSize {
        BoxSize::default()
    }
    /// The paths that the basic-layer code stored in a key strokes, run with `t` from its frame to
    /// the node's coordinates (`/pgf/generic circle IEC/before background`).
    fn code_paths(&mut self, _key: &str, _t: Transform) -> Vec<Vec<Segment>> {
        Vec::new()
    }
    /// The arrow tip `spec` as drawn with lines `line_width` wide (cm), its options evaluated
    /// (`length=2*3pt`): see [`crate::engine::arrows::tip_shapes`].
    fn tip_shapes(&mut self, spec: &str, line_width: f64) -> Option<(f64, Vec<crate::engine::arrows::TipPath>)> {
        crate::engine::arrows::tip_shapes(&mut crate::engine::arrows::Plain, spec, line_width)
    }
}

/// What a shape is computed from.
pub struct Input<'a> {
    /// the node parts' boxes, `text` first; a part not given has an empty box
    pub parts: &'a [(String, BoxSize)],
    pub inner_xsep: f64,
    pub inner_ysep: f64,
    pub minimum_width: f64,
    pub minimum_height: f64,
    pub outer_xsep: f64,
    pub outer_ysep: f64,
    /// `/pgf/aspect` (`\pgfshapeaspect`)
    pub aspect: f64,
    /// the node font's ex, for the `mid` anchors (at .5ex)
    pub ex: f64,
    pub line_width: f64,
    /// a callout's absolute pointer, in node coordinates, once the node is placed
    pub pointer: Option<Point>,
    pub keys: &'a mut dyn Keys,
}

impl Input<'_> {
    /// The box of a node part (empty if the text does not have it).
    pub fn part(&self, name: &str) -> BoxSize {
        self.parts.iter().find(|(n, _)| n == name).map(|(_, b)| *b).unwrap_or_default()
    }

    /// The `text` part's box (`\pgfnodeparttextbox`).
    pub fn text(&self) -> BoxSize {
        self.part("text")
    }

    /// The larger of the two outer seps, which most shapes of the libraries use.
    pub fn outer_sep(&self) -> f64 {
        self.outer_xsep.max(self.outer_ysep)
    }

    /// pgf's `\centerpoint` of most shapes: the middle of the text box.
    pub fn center_point(&self) -> Point {
        let t = self.text();
        Point::new(t.wd / 2.0, (t.ht - t.dp) / 2.0)
    }

    /// `\basepoint`: the middle of the text's baseline.
    pub fn base_point(&self) -> Point {
        Point::new(self.text().wd / 2.0, 0.0)
    }

    /// `\midpoint`: half an ex above the base point.
    pub fn mid_point(&self) -> Point {
        Point::new(self.text().wd / 2.0, 0.5 * self.ex)
    }

    /// Half the text box plus inner sep: the `x` and `y` most shapes start from.
    pub fn half_with_inner(&self) -> (f64, f64) {
        let t = self.text();
        (t.wd / 2.0 + self.inner_xsep, (t.ht + t.dp) / 2.0 + self.inner_ysep)
    }

    pub fn number(&mut self, key: &str, default: f64) -> f64 {
        self.keys.number(key, default)
    }

    pub fn length(&mut self, key: &str, default: f64) -> f64 {
        self.keys.length(key, default)
    }

    /// `/pgf/shape border rotate`, rounded as the shapes without `shape border uses incircle` do:
    /// to a multiple of 90 in [0, 360).
    pub fn border_rotation(&mut self) -> (f64, bool) {
        let incircle = self.keys.flag("tikra@sh@incircle");
        let r = self.number("/pgf/shape border rotate", 0.0);
        if incircle {
            (r, true)
        } else {
            // \pgfmathmod then integer arithmetic: (r mod 360 + 45) / 90 * 90, truncated
            let m = r % 360.0;
            let mut k = ((m.trunc() as i64 + 45) / 90) * 90;
            if k < 0 {
                k += 360;
            }
            (k as f64, false)
        }
    }
}

/// Where lines to the node stop: the border, with `outer sep`.
#[derive(Debug, Clone)]
pub enum Border {
    /// the center itself (coordinate)
    Point,
    /// a rectangle around the center, by its half sizes (`\pgfpointborderrectangle`)
    Rectangle(Point),
    /// a rectangle by its half sizes, found toward the direction plus the center as measured from
    /// the text's origin: the rectangle callout's border, as pgf computes it
    RectangleShifted(Point, Point),
    /// an ellipse around the center, by its radii (`\pgfpointborderellipse`)
    Ellipse(Point),
    /// a closed outline: the border point toward a direction is where the ray from the center
    /// leaves it
    Outline(Vec<Point>),
}

/// The layer a shape's path is drawn in, as pgf's shape declarations name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    BehindBackground,
    Background,
    BeforeBackground,
    BehindForeground,
    Foreground,
    BeforeForeground,
}

/// How a shape's path is used.
#[derive(Debug, Clone, PartialEq)]
pub enum Usage {
    /// as the node says (`draw`, `fill`, a pattern): the background and foreground paths
    Node,
    /// filled with a color (a color expression, as `cylinder end fill=red!20`)
    Fill(String),
    /// stroked as the node's lines are, whether the node is drawn or not (`\pgfusepath{stroke}` in
    /// a shape's code)
    Stroke,
    /// filled in the color of the node's lines (an arrow tip the shape draws)
    LineFill,
    /// stroked as the node's lines are, with these caps and joins (an arrow tip the shape draws)
    TipStroke(crate::canvas::Cap, crate::canvas::Join),
    /// a library's shape's path: painted as it says, in its color, dash pattern and caps if it has them
    Library { paint: crate::extension::ShapePaint, color: Option<String>, dash: Option<Vec<f64>>, cap: Option<crate::extension::LineCap>, opacity: Option<f64> },
}

#[derive(Debug, Clone)]
pub struct ShapePath {
    pub layer: Layer,
    pub segments: Vec<Segment>,
    pub usage: Usage,
}

/// A text that a shape typesets itself (an IEC gate's symbol, a library's texts): TeX, where the
/// origin of its box goes (or, with an anchor, that point of its box), its color if not the node's
/// and its font if not the node's.
#[derive(Debug, Clone)]
pub struct ShapeText {
    pub tex: String,
    pub at: Point,
    pub color: Option<String>,
    pub anchor: Option<crate::extension::TextAnchor>,
    pub font: Option<String>,
    /// Degrees counterclockwise around `at`, and whether the node's rotation is undone for it.
    pub turn: f64,
    pub upright: bool,
}

/// A shape, computed for one node, in the node's coordinate system.
#[derive(Debug, Clone)]
pub struct Geometry {
    /// the `center` anchor: borders are found from it
    pub center: Point,
    /// the named anchors
    pub anchors: Vec<(String, Point)>,
    pub border: Border,
    /// where the origin of each node part's box goes
    pub parts: Vec<(String, Point)>,
    pub paths: Vec<ShapePath>,
    /// the background is made of straight lines only, so `rounded corners` apply to it
    pub roundable: bool,
    /// texts of its own, drawn in the foreground
    pub texts: Vec<ShapeText>,
    /// points that count in the picture's size besides the paths (a library's shape)
    pub extent: Vec<Point>,
}

impl Geometry {
    /// A geometry with its center and border; anchors and paths are added by the shape.
    pub fn new(center: Point, border: Border) -> Geometry {
        Geometry {
            center,
            anchors: Vec::new(),
            border,
            parts: vec![("text".into(), Point::new(0.0, 0.0))],
            paths: Vec::new(),
            roundable: false,
            texts: Vec::new(),
            extent: Vec::new(),
        }
    }

    pub fn anchor_at(&mut self, name: &str, p: Point) {
        self.anchors.push((name.to_string(), p));
    }

    /// Adds a path, in a layer.
    pub fn path(&mut self, layer: Layer, segments: Vec<Segment>, usage: Usage) {
        self.paths.push(ShapePath { layer, segments, usage });
    }

    pub fn background(&mut self, segments: Vec<Segment>) {
        self.path(Layer::Background, segments, Usage::Node);
    }

    /// The border point in direction `d` from the center.
    pub fn border_point(&self, d: Point) -> Point {
        let c = self.center;
        let n = d.norm();
        if n < 1e-12 {
            return c;
        }
        match &self.border {
            Border::Point => c,
            Border::Rectangle(h) => c.plus(border_rectangle(d, *h)),
            Border::RectangleShifted(h, shift) => c.plus(border_rectangle(d.plus(*shift), *h)),
            Border::Ellipse(r) => c.plus(border_ellipse(d, *r)),
            Border::Outline(o) => ray_exit(c, d, o).unwrap_or(c),
        }
    }

    /// A named anchor, or a number: the border point at that angle.
    pub fn anchor(&self, name: &str) -> Option<Point> {
        let name = normalize(name);
        if name.is_empty() {
            return Some(self.center);
        }
        if let Some((_, p)) = self.anchors.iter().find(|(n, _)| *n == name) {
            return Some(*p);
        }
        if name == "center" {
            return Some(self.center);
        }
        if let Some((_, p)) = self.parts.iter().find(|(n, _)| *n == name) {
            return Some(*p);
        }
        let angle: f64 = name.parse().ok()?;
        // toward a point 1pt away, as pgf asks (a border that adds its center to it depends on that)
        Some(self.border_point(polar(angle, crate::PT)))
    }

    /// Whether the shape has an anchor of this name (not counting angles).
    pub fn has_anchor(&self, name: &str) -> bool {
        let name = normalize(name);
        self.anchors.iter().any(|(n, _)| *n == name) || self.parts.iter().any(|(n, _)| *n == name)
    }
}

/// Spaces collapsed, as TeX reads anchor names.
fn normalize(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The keys of the shapes, as the shape libraries define them (run after TikZ's own definitions).
/// The values of the shapes' keys (`/pgf/…/.initial`), set directly: faster than running them.
pub const INITIAL: &[(&str, &str)] = &[
    ("/pgf/shape border rotate", "0"),
    ("/pgf/star points", "5"),
    ("/pgf/star point height", ".5cm"),
    ("/pgf/star point ratio", "1.5"),
    ("/pgf/regular polygon sides", "5"),
    ("/pgf/trapezium left angle", "60"),
    ("/pgf/trapezium right angle", "60"),
    ("/pgf/isosceles triangle apex angle", "45"),
    ("/pgf/kite upper vertex angle", "120"),
    ("/pgf/kite lower vertex angle", "60"),
    ("/pgf/dart tip angle", "45"),
    ("/pgf/dart tail angle", "135"),
    ("/pgf/circular sector angle", "60"),
    ("/pgf/cylinder end fill", "white"),
    ("/pgf/cylinder body fill", "white"),
    ("/pgf/rounded rectangle west arc", "convex"),
    ("/pgf/rounded rectangle east arc", "convex"),
    ("/pgf/rounded rectangle arc length", "180"),
    ("/pgf/chamfered rectangle corners", "chamfer all"),
    ("/pgf/chamfered rectangle angle", "45"),
    ("/pgf/chamfered rectangle xsep", ".666ex"),
    ("/pgf/chamfered rectangle ysep", ".666ex"),
    ("/pgf/starburst points", "17"),
    ("/pgf/starburst point height", ".5cm"),
    ("/pgf/random starburst", "100"),
    ("/pgf/cloud puffs", "10"),
    ("/pgf/cloud puff arc", "150"),
    ("/pgf/signal pointer angle", "90"),
    ("/pgf/signal to", "east"),
    ("/pgf/signal from", "nowhere"),
    ("/pgf/tape bend top", "in and out"),
    ("/pgf/tape bend bottom", "in and out"),
    ("/pgf/tape bend height", "5pt"),
    ("/pgf/magnifying glass handle angle", "-45"),
    ("/pgf/magnifying glass handle aspect", "1.5"),
    ("/pgf/magnetic tape tail", "0.15"),
    ("/pgf/magnetic tape tail extend", "0cm"),
    ("/pgf/single arrow tip angle", "90"),
    ("/pgf/single arrow head extend", ".25cm"),
    ("/pgf/single arrow head indent", "0cm"),
    ("/pgf/double arrow tip angle", "90"),
    ("/pgf/double arrow head extend", ".25cm"),
    ("/pgf/double arrow head indent", "0cm"),
    ("/pgf/arrow box west arrow", ".5cm"),
    ("/pgf/arrow box east arrow", ".5cm"),
    ("/pgf/arrow box south arrow", ".5cm"),
    ("/pgf/arrow box north arrow", ".5cm"),
    ("/pgf/arrow box shaft width", ".125cm"),
    ("/pgf/arrow box head extend", ".125cm"),
    ("/pgf/arrow box head indent", "0cm"),
    ("/pgf/arrow box tip angle", "90"),
    ("/pgf/arrow box arrow list", ""),
    ("/pgf/callout pointer arc", "15"),
    ("/pgf/callout pointer width", ".25cm"),
    ("/pgf/callout pointer start size", ".2 of callout"),
    ("/pgf/callout pointer end size", ".1 of callout"),
    ("/pgf/callout pointer segments", "2"),
    ("/pgf/callout pointer shorten", "0cm"),
    ("/pgf/callout pointer anchor aspect", "1"),
    ("/pgf/callout pointer@relative", "(300:.5cm)"),
    ("/pgf/callout pointer@absolute", ""),
    ("/pgf/rectangle split parts", "4"),
    ("/pgf/rectangle split part align", "center"),
    ("/pgf/rectangle split empty part width", "1ex"),
    ("/pgf/rectangle split empty part height", "1ex"),
    ("/pgf/rectangle split empty part depth", "0ex"),
    ("/pgf/rectangle split part fill", "none"),
    ("/pgf/logic gate input sep", "0.125cm"),
    ("/pgf/logic gate inputs", "normal,normal"),
    ("/pgf/logic gate inverted radius", "2pt"),
    ("/pgf/logic gate IEC symbol align", "top"),
    ("/pgf/logic gate IEC symbol color", ""),
    ("/pgf/direction ee arrow", "direction ee"),
    ("/pgf/arrow options/direction ee", "4pt"),
    ("/pgf/generic circle IEC/before background", ""),
    ("/pgf/generic diode IEC/before background", ""),
    ("/pgf/generic circle IEC/tikra mark", ""),
    ("/pgf/generic diode IEC/tikra mark", ""),
    ("/pgf/and gate IEC symbol", "$\\char`\\&$"),
    ("/pgf/nand gate IEC symbol", "$\\char`\\&$"),
    ("/pgf/or gate IEC symbol", "$\\geq1$"),
    ("/pgf/nor gate IEC symbol", "$\\geq1$"),
    ("/pgf/xor gate IEC symbol", "$=1$"),
    ("/pgf/xnor gate IEC symbol", "$=1$"),
    ("/pgf/not gate IEC symbol", "1"),
    ("/pgf/buffer gate IEC symbol", "1"),
];

pub const DEFINITIONS: &str = r"
\newif\iftikra@sh@incircle \newif\iftikra@sh@starratio \tikra@sh@starratiotrue
\newif\iftikra@sh@trapstretch \newif\iftikra@sh@trapbody \newif\iftikra@sh@isostretch
\newif\iftikra@sh@cloudnoaspect \newif\iftikra@sh@cloudellipse \newif\ifpgfcylinderusescustomfill
\newif\iftikra@sh@splithoriz \newif\iftikra@sh@splitlines \tikra@sh@splitlinestrue
\newif\iftikra@sh@splitskipempty \newif\iftikra@sh@splitfill
\newif\iftikra@gatebox
\pgfkeys{/pgf/logic gate anchors use bounding box/.is if=tikra@gatebox}
\def\tikz@atbegin@node{}
\def\tikz@atend@node{}
\tikzset{
  execute at begin node/.code={\expandafter\def\expandafter\tikz@atbegin@node\expandafter{\tikz@atbegin@node#1}},
  execute at end node/.code={\expandafter\def\expandafter\tikz@atend@node\expandafter{\tikz@atend@node#1}},
  node contents/.code={},
}
\def\tikra@kiteangles#1and#2and#3\@@{\pgfkeys{/pgf/kite upper vertex angle={#1},/pgf/kite lower vertex angle={#2}}}
\pgfkeys{/pgf/.cd,
  shape border uses incircle/.is if=tikra@sh@incircle,
  star point height/.code={\pgfkeyssetvalue{/pgf/star point height}{#1}\tikra@sh@starratiofalse},
  star point ratio/.code={\pgfkeyssetvalue{/pgf/star point ratio}{#1}\tikra@sh@starratiotrue},
  star rotate/.style={/pgf/shape border rotate={#1}},
  regular polygon rotate/.style={/pgf/shape border rotate={#1}},
  trapezium angle/.style={/pgf/trapezium left angle={#1},/pgf/trapezium right angle={#1}},
  trapezium stretches/.is if=tikra@sh@trapstretch,
  trapezium stretches body/.is if=tikra@sh@trapbody,
  isosceles triangle stretches/.is if=tikra@sh@isostretch, cylinder uses custom fill/.is if=pgfcylinderusescustomfill,
  kite vertex angles/.code={\tikra@kiteangles#1 and #1 and\@@},
  rounded rectangle left arc/.style={/pgf/rounded rectangle west arc={#1}},
  rounded rectangle right arc/.style={/pgf/rounded rectangle east arc={#1}},
  chamfered rectangle sep/.style={/pgf/chamfered rectangle xsep={#1},/pgf/chamfered rectangle ysep={#1}},
  cloud ignores aspect/.is if=tikra@sh@cloudnoaspect,
  cloud anchors use ellipse/.is if=tikra@sh@cloudellipse,
  tape bend/.style={/pgf/tape bend top={#1},/pgf/tape bend bottom={#1}},
  arrow box arrows/.code={\pgfkeyssetvalue{/pgf/arrow box arrow list}{#1}},
  callout relative pointer/.code={\pgfkeyssetvalue{/pgf/callout pointer@relative}{#1}\pgfkeyssetvalue{/pgf/callout pointer@absolute}{}},
  callout absolute pointer/.code={\pgfkeyssetvalue{/pgf/callout pointer@absolute}{#1}},
  rectangle split horizontal/.is if=tikra@sh@splithoriz,
  rectangle split ignore empty parts/.is if=tikra@sh@splitskipempty,
  rectangle split part fill/.code={\pgfkeyssetvalue{/pgf/rectangle split part fill}{#1}\tikra@sh@splitfilltrue},
  rectangle split uses custom fill/.is if=tikra@sh@splitfill,
  rectangle split draw splits/.is if=tikra@sh@splitlines,
  rectangle split allocate boxes/.code={},
  rectangle split every empty part/.code={},
}
";

/// Every shape tikra knows, by pgf's name.
pub fn is_shape(name: &str) -> bool {
    NAMES.contains(&normalize(name).as_str())
}

const NAMES: &[&str] = &[
    "rectangle",
    "circle",
    "coordinate",
    "ellipse",
    "diamond",
    "star",
    "regular polygon",
    "trapezium",
    "semicircle",
    "isosceles triangle",
    "kite",
    "dart",
    "circular sector",
    "cylinder",
    "cross out",
    "strike out",
    "rounded rectangle",
    "chamfered rectangle",
    "forbidden sign",
    "correct forbidden sign",
    "magnifying glass",
    "starburst",
    "cloud",
    "signal",
    "tape",
    "magnetic tape",
    "single arrow",
    "double arrow",
    "arrow box",
    "ellipse callout",
    "rectangle callout",
    "cloud callout",
    "circle split",
    "circle solidus",
    "ellipse split",
    "diamond split",
    "rectangle split",
    "and gate US",
    "nand gate US",
    "or gate US",
    "nor gate US",
    "xor gate US",
    "xnor gate US",
    "not gate US",
    "buffer gate US",
    "and gate CDH",
    "nand gate CDH",
    "and gate IEC",
    "nand gate IEC",
    "or gate IEC",
    "nor gate IEC",
    "xor gate IEC",
    "xnor gate IEC",
    "not gate IEC",
    "buffer gate IEC",
    "rectangle ee",
    "circle ee",
    "direction ee",
    "var resistor IEC",
    "inductor IEC",
    "capacitor IEC",
    "ground IEC",
    "battery IEC",
    "breakdown diode IEC",
    "generic circle IEC",
    "generic diode IEC",
    "make contact IEC",
    "var make contact IEC",
    "break contact IEC",
];

/// Computes the shape `name` for a node; None if the shape is not known.
pub fn compute(name: &str, input: &mut Input) -> Option<Geometry> {
    Some(match normalize(name).as_str() {
        "rectangle" => basic::rectangle(input),
        "circle" => basic::circle(input),
        "coordinate" => basic::coordinate(input),
        "ellipse" => basic::ellipse(input),
        "diamond" => basic::diamond(input),
        "star" => geometric::star(input),
        "regular polygon" => geometric::regular_polygon(input),
        "trapezium" => geometric::trapezium(input),
        "semicircle" => geometric::semicircle(input),
        "isosceles triangle" => geometric::isosceles_triangle(input),
        "kite" => geometric::kite(input),
        "dart" => geometric::dart(input),
        "circular sector" => geometric::circular_sector(input),
        "cylinder" => geometric::cylinder(input),
        "cross out" => misc::cross_out(input),
        "strike out" => misc::strike_out(input),
        "rounded rectangle" => misc::rounded_rectangle(input),
        "chamfered rectangle" => misc::chamfered_rectangle(input),
        "forbidden sign" => symbols::forbidden_sign(input),
        "correct forbidden sign" => symbols::correct_forbidden_sign(input),
        "magnifying glass" => symbols::magnifying_glass(input),
        "starburst" => symbols::starburst(input),
        "cloud" => symbols::cloud(input),
        "signal" => symbols::signal(input),
        "tape" => symbols::tape(input),
        "magnetic tape" => symbols::magnetic_tape(input),
        "single arrow" => arrows::single_arrow(input),
        "double arrow" => arrows::double_arrow(input),
        "arrow box" => arrows::arrow_box(input),
        "ellipse callout" => callouts::ellipse_callout(input),
        "rectangle callout" => callouts::rectangle_callout(input),
        "cloud callout" => callouts::cloud_callout(input),
        "circle split" => multipart::circle_split(input),
        "circle solidus" => multipart::circle_solidus(input),
        "ellipse split" => multipart::ellipse_split(input),
        "diamond split" => multipart::diamond_split(input),
        "rectangle split" => multipart::rectangle_split(input),
        "and gate US" => gates::and_gate_us(input, false),
        "nand gate US" => gates::and_gate_us(input, true),
        "or gate US" => gates::or_gate_us(input, "or"),
        "nor gate US" => gates::or_gate_us(input, "nor"),
        "xor gate US" => gates::or_gate_us(input, "xor"),
        "xnor gate US" => gates::or_gate_us(input, "xnor"),
        "not gate US" => gates::not_gate_us(input, true),
        "buffer gate US" => gates::not_gate_us(input, false),
        "and gate CDH" => gates::and_gate_cdh(input, false),
        "nand gate CDH" => gates::and_gate_cdh(input, true),
        n if n.ends_with(" gate IEC") => gates::gate_iec(input, n.trim_end_matches(" gate IEC")),
        "rectangle ee" => ee::rectangle_ee(input),
        "circle ee" => ee::circle_ee(input),
        "direction ee" => ee::direction_ee(input),
        "var resistor IEC" => ee::var_resistor(input),
        "inductor IEC" => ee::inductor(input),
        "capacitor IEC" => ee::capacitor(input),
        "ground IEC" => ee::ground(input),
        "battery IEC" => ee::battery(input),
        "breakdown diode IEC" => ee::breakdown_diode(input),
        "generic circle IEC" => ee::generic_circle(input),
        "generic diode IEC" => ee::generic_diode(input),
        "make contact IEC" => ee::make_contact(input),
        "var make contact IEC" => ee::var_make_contact(input),
        "break contact IEC" => ee::break_contact(input),
        _ => return None,
    })
}

// ------------------------------------------------------------------ geometry helpers

/// `\pgfqpointpolar`: the point at `angle` degrees and distance `r`.
pub fn polar(angle: f64, r: f64) -> Point {
    let (s, c) = angle.to_radians().sin_cos();
    Point::new(r * c, r * s)
}

/// `\pgfmathrotatepointaround`: `p` rotated by `angle` degrees around `c`.
pub fn rotate_around(p: Point, c: Point, angle: f64) -> Point {
    if angle == 0.0 {
        return p;
    }
    let (s, co) = angle.to_radians().sin_cos();
    let d = p.minus(c);
    c.plus(Point::new(d.x * co - d.y * s, d.x * s + d.y * co))
}

/// `\pgfmathanglebetweenpoints`: the angle of `q` seen from `p`, in [0, 360).
pub fn angle_between(p: Point, q: Point) -> f64 {
    let a = (q.y - p.y).atan2(q.x - p.x).to_degrees();
    if a < 0.0 {
        a + 360.0
    } else {
        a
    }
}

/// `\pgfpointintersectionoflines`: where the line through `a` and `b` meets the line through `c`
/// and `d`.
pub fn intersect_lines(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let r = b.minus(a);
    let s = d.minus(c);
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-15 {
        return None;
    }
    let t = ((c.x - a.x) * s.y - (c.y - a.y) * s.x) / den;
    Some(a.plus(r.scale(t)))
}

/// `\pgfpointborderrectangle`: the point where the ray in direction `d` leaves the rectangle of
/// half sizes `h` centered at the origin.
pub fn border_rectangle(d: Point, h: Point) -> Point {
    let tx = if d.x.abs() > 1e-12 { h.x / d.x.abs() } else { f64::INFINITY };
    let ty = if d.y.abs() > 1e-12 { h.y / d.y.abs() } else { f64::INFINITY };
    d.scale(tx.min(ty))
}

/// `\pgfpointborderellipse`: the point where the ray in direction `d` meets the ellipse of radii
/// `r` centered at the origin.
pub fn border_ellipse(d: Point, r: Point) -> Point {
    let (a, b) = (r.x.max(1e-12), r.y.max(1e-12));
    let k = 1.0 / ((d.x / a).powi(2) + (d.y / b).powi(2)).sqrt();
    d.scale(k)
}

/// Where the ray from `o` in direction `d` leaves the closed outline (its farthest crossing).
pub fn ray_exit(o: Point, d: Point, outline: &[Point]) -> Option<Point> {
    let n = outline.len();
    let mut best: Option<f64> = None;
    for k in 0..n {
        let a = outline[k];
        let b = outline[(k + 1) % n];
        let s = b.minus(a);
        let den = d.x * s.y - d.y * s.x;
        if den.abs() < 1e-15 {
            continue;
        }
        // o + t d = a + u s
        let w = a.minus(o);
        let t = (w.x * s.y - w.y * s.x) / den;
        let u = (w.x * d.y - w.y * d.x) / den;
        if t > 1e-12 && (-1e-9..=1.0 + 1e-9).contains(&u) && best.is_none_or(|bt| t > bt) {
            best = Some(t);
        }
    }
    best.map(|t| o.plus(d.scale(t)))
}

/// The border anchor from a reference point: where the ray from `o` toward `d` leaves the outline,
/// or `o` if it does not.
pub fn ray(o: Point, d: Point, outline: &[Point]) -> Point {
    ray_exit(o, d, outline).unwrap_or(o)
}

/// The polygon with each side moved out by `d` (the corners by the miter length), as the shapes
/// compute their anchor borders from `outer sep`.
pub fn miter_offset(poly: &[Point], d: f64) -> Vec<Point> {
    let n = poly.len();
    if n < 3 || d == 0.0 {
        return poly.to_vec();
    }
    // the outward normal side depends on the orientation
    let area: f64 = (0..n).map(|k| poly[k].x * poly[(k + 1) % n].y - poly[(k + 1) % n].x * poly[k].y).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let shifted = |a: Point, b: Point| -> (Point, Point) {
        let e = b.minus(a);
        let l = e.norm().max(1e-15);
        // right normal of a counterclockwise polygon points outside
        let nrm = Point::new(e.y / l, -e.x / l).scale(sign * d);
        (a.plus(nrm), b.plus(nrm))
    };
    (0..n)
        .map(|k| {
            let (p0, p1) = shifted(poly[(k + n - 1) % n], poly[k]);
            let (q0, q1) = shifted(poly[k], poly[(k + 1) % n]);
            intersect_lines(p0, p1, q0, q1).unwrap_or(p1)
        })
        .collect()
}

/// A closed polygon as a path.
pub fn polygon_path(points: &[Point]) -> Vec<Segment> {
    let mut v = Vec::with_capacity(points.len() + 1);
    for (k, p) in points.iter().enumerate() {
        v.push(if k == 0 { Segment::MoveTo(*p) } else { Segment::LineTo(*p) });
    }
    v.push(Segment::Close);
    v
}

/// `\pgfpatharc` from the current point: an elliptical arc of radii `rx`, `ry` from `a0` to `a1`
/// degrees, whose start is `from`. Returns the end point.
pub fn arc_to(v: &mut Vec<Segment>, from: Point, a0: f64, a1: f64, rx: f64, ry: f64) -> Point {
    let c = from.minus(Point::new(rx * a0.to_radians().cos(), ry * a0.to_radians().sin()));
    v.extend(super::path::arc_segments(c, rx, ry, a0, a1, &crate::scene::Transform::default()));
    c.plus(Point::new(rx * a1.to_radians().cos(), ry * a1.to_radians().sin()))
}

/// Like [`arc_to`], rotated by `rotate` degrees around the arc's start (as `\pgftransformrotate`
/// before `\pgfpatharc`).
pub fn arc_to_rotated(v: &mut Vec<Segment>, from: Point, a0: f64, a1: f64, rx: f64, ry: f64, rotate: f64) -> Point {
    let mut local = Vec::new();
    let end = arc_to(&mut local, Point::new(0.0, 0.0), a0, a1, rx, ry);
    let t = crate::scene::Transform::translation(from.x, from.y).compose(&crate::scene::Transform::rotation(rotate));
    v.extend(crate::canvas::transform_segments(&local, &t));
    t.apply(end)
}

/// Points along an elliptical arc around `c` (for outlines), every degree or so.
pub fn arc_points(c: Point, rx: f64, ry: f64, a0: f64, a1: f64) -> Vec<Point> {
    let n = ((a1 - a0).abs().ceil() as usize).clamp(2, 3600);
    (0..=n)
        .map(|k| {
            let a = (a0 + (a1 - a0) * k as f64 / n as f64).to_radians();
            c.plus(Point::new(rx * a.cos(), ry * a.sin()))
        })
        .collect()
}

/// Points along a path's segments (curves flattened), for outlines; closes are dropped.
pub fn flatten(segs: &[Segment]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    let mut cur = Point::new(0.0, 0.0);
    for s in segs {
        match *s {
            Segment::MoveTo(p) | Segment::LineTo(p) => {
                out.push(p);
                cur = p;
            }
            Segment::CurveTo(a, b, p) => {
                for k in 1..=24 {
                    let t = k as f64 / 24.0;
                    let u = 1.0 - t;
                    out.push(cur.scale(u * u * u).plus(a.scale(3.0 * u * u * t)).plus(b.scale(3.0 * u * t * t)).plus(p.scale(t * t * t)));
                }
                cur = p;
            }
            Segment::Close => {}
        }
    }
    out
}

/// pgf's trigonometry in degrees.
pub fn sin(a: f64) -> f64 {
    a.to_radians().sin()
}

pub fn cos(a: f64) -> f64 {
    a.to_radians().cos()
}

pub fn tan(a: f64) -> f64 {
    a.to_radians().tan()
}

pub fn cot(a: f64) -> f64 {
    1.0 / tan(a)
}

/// The angle between two lines (`\pgfmathanglebetweenlines`): from direction `p1 → p2` to
/// direction `q1 → q2`, in [0, 360).
pub fn angle_between_lines(p1: Point, p2: Point, q1: Point, q2: Point) -> f64 {
    let a = angle_between(p1, p2);
    let b = angle_between(q1, q2);
    let d = b - a;
    if d < 0.0 {
        d + 360.0
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PT;
    use std::collections::HashMap;
    use std::f64::consts::FRAC_1_SQRT_2;

    /// Keys from a table of numbers and lengths (cm), flags by name.
    #[derive(Default)]
    pub struct TableKeys {
        pub values: HashMap<&'static str, f64>,
        pub flags: Vec<&'static str>,
    }

    impl Keys for TableKeys {
        fn length(&mut self, key: &str, default: f64) -> f64 {
            self.values.get(key).copied().unwrap_or(default)
        }
        fn point(&mut self, _key: &str) -> Option<Point> {
            None
        }
        fn length_of(&mut self, _text: &str, default: f64) -> f64 {
            default
        }
        fn number(&mut self, key: &str, default: f64) -> f64 {
            self.values.get(key).copied().unwrap_or(default)
        }
        fn text(&mut self, _key: &str) -> String {
            String::new()
        }
        fn flag(&mut self, name: &str) -> bool {
            self.flags.contains(&name)
        }
    }

    /// The shape for a text box of `wd`×`ht`+`dp` (pt), with TikZ's default seps.
    pub fn shape(name: &str, wd: f64, ht: f64, dp: f64, keys: &mut TableKeys) -> Geometry {
        let parts = [("text".to_string(), BoxSize { wd: wd * PT, ht: ht * PT, dp: dp * PT })];
        let mut input = Input {
            parts: &parts,
            inner_xsep: 3.333 * PT,
            inner_ysep: 3.333 * PT,
            minimum_width: PT,
            minimum_height: PT,
            outer_xsep: 0.2 * PT,
            outer_ysep: 0.2 * PT,
            aspect: 1.0,
            ex: 4.3 * PT,
            line_width: 0.4 * PT,
            pointer: None,
            keys,
        };
        compute(name, &mut input).unwrap()
    }

    fn close(p: Point, x: f64, y: f64) -> bool {
        (p.x - x * PT).abs() < 1e-3 * PT && (p.y - y * PT).abs() < 1e-3 * PT
    }

    /// Keys without an engine of their own draw arrow tips with plain values (the engine's keys,
    /// the only ones that draw a shape with a tip, evaluate the tip's options).
    #[test]
    fn keys_draw_arrow_tips_with_plain_values() {
        let mut k = TableKeys::default();
        let (length, paths) = k.tip_shapes("stealth", 0.4 * PT).expect("a known tip");
        assert!(length > 0.0, "{length}");
        assert!(!paths.is_empty() && paths.iter().all(|(segments, filled, _)| *filled && !segments.is_empty()), "{paths:?}");
        assert!(k.tip_shapes("no such tip", 0.4 * PT).is_none());
    }

    #[test]
    fn rectangle_and_circle_anchors() {
        let mut k = TableKeys::default();
        let g = shape("rectangle", 20.0, 7.0, 2.0, &mut k);
        // half width 10 + 3.333 + .2 around x = 10, half height 4.5 + 3.333 + .2 around y = 2.5
        assert!(close(g.anchor("north east").unwrap(), 23.533, 10.533), "{:?}", g.anchor("north east"));
        assert!(close(g.anchor("base west").unwrap(), -3.533, 0.0));
        assert!(close(g.anchor("mid").unwrap(), 10.0, 2.15));
        assert!(close(g.anchor("text").unwrap(), 0.0, 0.0));
        let b = g.anchor("0").unwrap();
        assert!(close(b, 23.533, 2.5), "{b:?}");
        let c = shape("circle", 0.0, 0.0, 0.0, &mut k);
        let r = (2.0f64).sqrt() * 3.333 + 0.2;
        assert!(close(c.anchor("north").unwrap(), 0.0, r));
        assert!(close(c.anchor("north east").unwrap(), FRAC_1_SQRT_2 * r, FRAC_1_SQRT_2 * r));
    }

    #[test]
    fn regular_polygon_and_star() {
        let mut k = TableKeys::default();
        k.values.insert("/pgf/regular polygon sides", 4.0);
        let g = shape("regular polygon", 0.0, 0.0, 0.0, &mut k);
        // a square around the incircle of radius √2·3.333: corners at 45°, 135°…
        let r = 2.0f64.sqrt() * 3.333 / (45.0f64).to_radians().cos();
        let a = r + 0.2 / (45.0f64).to_radians().sin();
        let c1 = g.anchor("corner 1").unwrap();
        assert!(close(c1, a * FRAC_1_SQRT_2, a * FRAC_1_SQRT_2), "{c1:?}");
        assert!(g.anchor("side 4").is_some() && g.anchor("corner 5").is_none());
        let mut k = TableKeys { flags: vec!["tikra@sh@starratio"], ..Default::default() };
        let s = shape("star", 0.0, 0.0, 0.0, &mut k);
        let outer = s.anchor("outer point 1").unwrap().norm();
        let inner = s.anchor("inner point 1").unwrap().norm();
        assert!(outer > 1.4 * inner && outer < 1.8 * inner, "{outer} {inner}");
        assert!((s.anchor("outer point 1").unwrap().x).abs() < 1e-9);
    }

    #[test]
    fn geometric_named_anchors() {
        let mut k = TableKeys::default();
        let t = shape("trapezium", 20.0, 7.0, 0.0, &mut k);
        assert!(t.anchor("bottom left corner").unwrap().x < t.anchor("top left corner").unwrap().x);
        let i = shape("isosceles triangle", 10.0, 7.0, 0.0, &mut k);
        let apex = i.anchor("apex").unwrap();
        assert!(apex.x > i.anchor("east").unwrap().x - 1e-9 && (apex.y - i.center.y).abs() < 1e-9);
        let s = shape("semicircle", 10.0, 7.0, 0.0, &mut k);
        assert!(s.anchor("apex").unwrap().y > s.anchor("chord center").unwrap().y);
        let d = shape("diamond", 10.0, 7.0, 0.0, &mut k);
        let (n, e, ne) = (d.anchor("north").unwrap(), d.anchor("east").unwrap(), d.anchor("north east").unwrap());
        assert!((ne.x - (e.x + d.center.x) / 2.0).abs() < 1e-9 && (ne.y - (n.y + d.center.y) / 2.0).abs() < 1e-9);
        for name in ["kite", "dart", "circular sector", "cylinder"] {
            let g = shape(name, 10.0, 7.0, 2.0, &mut k);
            for a in ["north", "south", "east", "west", "base east", "mid west"] {
                if let Some(p) = g.anchor(a) {
                    assert!(p.x.is_finite() && p.y.is_finite(), "{name} {a}");
                }
            }
            assert!(!g.paths.is_empty(), "{name}");
        }
        let c = shape("cylinder", 10.0, 7.0, 2.0, &mut k);
        assert!(c.anchor("top").unwrap().x > c.anchor("before top").unwrap().x);
    }

    /// Keys a shape reads that the table leaves at their defaults: no symbol text, no code, no
    /// pointer, no text, no length written out (none of these defaults is reached from a document,
    /// whose keys are the engine's).
    #[test]
    fn shapes_with_keys_at_their_defaults() {
        let mut k = TableKeys::default();
        for name in ["and gate IEC", "generic circle IEC", "rectangle callout", "arrow box", "rounded rectangle"] {
            let g = shape(name, 10.0, 7.0, 2.0, &mut k);
            assert!(!g.paths.is_empty(), "{name}");
            let (n, s) = (g.anchor("north").unwrap(), g.anchor("south").unwrap());
            assert!(n.y > s.y + 10.0 * PT, "{name}: {n:?} {s:?}");
        }
    }

    #[test]
    fn miter_offset_of_a_square() {
        let sq = [Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(1.0, 1.0), Point::new(0.0, 1.0)];
        let o = miter_offset(&sq, 0.1);
        assert!((o[0].x + 0.1).abs() < 1e-12 && (o[0].y + 0.1).abs() < 1e-12, "{o:?}");
        assert!((o[2].x - 1.1).abs() < 1e-12 && (o[2].y - 1.1).abs() < 1e-12, "{o:?}");
        // clockwise gives the same
        let mut cw = sq.to_vec();
        cw.reverse();
        let o = miter_offset(&cw, 0.1);
        assert!((o[0].x + 0.1).abs() < 1e-12 && (o[0].y - 1.1).abs() < 1e-12, "{o:?}");
    }

    #[test]
    fn rays_leave_outlines() {
        let sq = [Point::new(-1.0, -1.0), Point::new(1.0, -1.0), Point::new(1.0, 1.0), Point::new(-1.0, 1.0)];
        let p = ray(Point::new(0.0, 0.0), Point::new(2.0, 1.0), &sq);
        assert!((p.x - 1.0).abs() < 1e-12 && (p.y - 0.5).abs() < 1e-12, "{p:?}");
        let e = border_ellipse(Point::new(1.0, 0.0), Point::new(2.0, 1.0));
        assert!((e.x - 2.0).abs() < 1e-12);
    }
}
