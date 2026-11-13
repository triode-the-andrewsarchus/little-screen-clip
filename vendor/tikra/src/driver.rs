//! The SVG driver: writes a [`Canvas`] as SVG, the way pgf's drivers write PDF. SVG user units are
//! big points (1 cm = 28.3465 bp), the unit of PDF, so the picture has the size TeX gives it.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use mathra::font::{GlyphId, MathFont};
use mathra::layout::Item as MathItem;

use crate::canvas::{Animation, Canvas, Cap, DataPoint, Fill, Item, Join, Moving, Paint, Shading, SourceSpan, Stroke};
use crate::scene::{Label, Point, Segment, Transform};

/// Big points per centimeter.
pub const BP: f64 = 72.0 / 2.54;

pub(crate) fn n(v: f64) -> String {
    num(v, 3)
}

fn num(v: f64, places: i32) -> String {
    let k = 10f64.powi(places);
    let r = (v * k).round() / k;
    if r == 0.0 {
        return "0".into();
    }
    if r.fract() == 0.0 && r.abs() < 1e15 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// A canvas point (cm, y up) in SVG units (bp, y down).
fn at(p: Point) -> (f64, f64) {
    (p.x * BP, -p.y * BP)
}

pub fn path_data(segs: &[Segment]) -> String {
    let mut d = String::new();
    for s in segs {
        match s {
            Segment::MoveTo(p) => {
                let (x, y) = at(*p);
                let _ = write!(d, "M{} {} ", n(x), n(y));
            }
            Segment::LineTo(p) => {
                let (x, y) = at(*p);
                let _ = write!(d, "L{} {} ", n(x), n(y));
            }
            Segment::CurveTo(a, b, p) => {
                let (ax, ay) = at(*a);
                let (bx, by) = at(*b);
                let (x, y) = at(*p);
                let _ = write!(d, "C{} {} {} {} {} {} ", n(ax), n(ay), n(bx), n(by), n(x), n(y));
            }
            Segment::Close => d.push_str("Z "),
        }
    }
    d.trim_end().to_string()
}

pub(crate) fn escape(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn stroke_attributes(s: &Stroke) -> String {
    let mut a = format!(r#" stroke="{}" stroke-width="{}""#, s.color.hex(), n(s.width * BP));
    if let Some((dash, phase)) = &s.dash {
        if !dash.is_empty() && dash.iter().any(|d| *d > 0.0) {
            let _ = write!(a, r#" stroke-dasharray="{}""#, dash.iter().map(|x| n(x * BP)).collect::<Vec<_>>().join(" "));
            if *phase != 0.0 {
                let _ = write!(a, r#" stroke-dashoffset="{}""#, n(phase * BP));
            }
        }
    }
    match s.cap {
        Cap::Butt => {}
        Cap::Round => a.push_str(r#" stroke-linecap="round""#),
        Cap::Square => a.push_str(r#" stroke-linecap="square""#),
    }
    match s.join {
        Join::Miter => {
            // pgf's default miter limit is 10, SVG's is 4
            if s.miter_limit != 4.0 {
                let _ = write!(a, r#" stroke-miterlimit="{}""#, n(s.miter_limit.max(1.0)));
            }
        }
        Join::Round => a.push_str(r#" stroke-linejoin="round""#),
        Join::Bevel => a.push_str(r#" stroke-linejoin="bevel""#),
    }
    if s.opacity < 1.0 {
        let _ = write!(a, r#" stroke-opacity="{}""#, n(s.opacity.max(0.0)));
    }
    a
}

struct Defs {
    text: String,
    count: usize,
    /// glyphs drawn inside definitions (a pattern's tile)
    glyphs: Vec<(MathFont, GlyphId)>,
    /// the page's lower left corner (SVG units): patterns tile from there, as in PDF
    corner: (f64, f64),
}

impl Defs {
    fn id(&mut self, prefix: &str) -> String {
        self.count += 1;
        format!("tr-{prefix}{}", self.count)
    }
}

fn paint_value(p: &Paint, defs: &mut Defs) -> String {
    match p {
        Paint::Solid(c) => c.hex(),
        Paint::Pattern { name, color } => {
            let id = defs.id("p");
            defs.text.push_str(&pattern(name, &id, &color.hex(), defs.corner));
            format!("url(#{id})")
        }
        Paint::Tile { items, origin, size, transform: t } => {
            // the tile's items drawn first (their own definitions come before the pattern)
            let (mut inner, mut glyphs) = (String::new(), Vec::new());
            items_svg(&items.0, &mut inner, defs, &mut glyphs, &mut Steps::off(), &[], &[]);
            defs.glyphs.append(&mut glyphs);
            let id = defs.id("p");
            let (w, h) = (size.x * BP, size.y * BP);
            // the tile's corner in SVG space (y down): its content is placed from there
            let (x0, y0) = (origin.x * BP, -(origin.y + size.y) * BP);
            // from the page's corner
            let (cx, cy) = defs.corner;
            let m = format!("matrix({} {} {} {} {} {})", n(t.a), n(-t.b), n(-t.c), n(t.d), n(t.e * BP + cx), n(-t.f * BP + cy));
            let _ = write!(
                defs.text,
                r#"<pattern id="{id}" patternUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" patternTransform="{m}"><g transform="translate({} {})">{inner}</g></pattern>"#,
                n(x0),
                n(y0),
                n(w),
                n(h),
                n(-x0),
                n(-y0)
            );
            format!("url(#{id})")
        }
        Paint::Shading(sh) => {
            let id = defs.id("s");
            let stops = |stops: &[(f64, crate::color::Color)]| {
                stops.iter().map(|(o, c)| format!(r#"<stop offset="{}" stop-color="{}"/>"#, num(o.clamp(0.0, 1.0), 4), c.hex())).collect::<String>()
            };
            // the gradient in pgf's shading box, taken to SVG space by its transformation
            let (t, body, kind) = match sh {
                Shading::Axial { transform, stops: s } => (transform, format!(r#"x1="0" y1="-50" x2="0" y2="50">{}"#, stops(s)), "linearGradient"),
                Shading::Radial { transform, focus, stops: s } => {
                    (transform, format!(r#"cx="0" cy="0" r="50" fx="{}" fy="{}">{}"#, n(focus.x), n(focus.y), stops(s)), "radialGradient")
                }
            };
            let m = format!("matrix({} {} {} {} {} {})", num(t.a * BP, 5), num(-t.b * BP, 5), num(t.c * BP, 5), num(-t.d * BP, 5), n(t.e * BP), n(-t.f * BP));
            let _ = write!(defs.text, r#"<{kind} id="{id}" gradientUnits="userSpaceOnUse" gradientTransform="{m}" {body}</{kind}>"#);
            format!("url(#{id})")
        }
    }
}

fn fill_attributes(f: &Fill, defs: &mut Defs) -> String {
    let mut a = format!(r#" fill="{}""#, paint_value(&f.paint, defs));
    if f.opacity < 1.0 {
        let _ = write!(a, r#" fill-opacity="{}""#, n(f.opacity.max(0.0)));
    }
    if f.even_odd {
        a.push_str(r#" fill-rule="evenodd""#);
    }
    a
}

/// `color!f` (as xcolor mixes it with white): its hex.
fn tint(c: (f64, f64, f64), f: f64) -> String {
    let m = |v: f64| ((v * f + (1.0 - f)) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", m(c.0), m(c.1), m(c.2))
}

/// Whether `pattern` below draws `name` itself (any other name falls back to north east lines).
pub(crate) fn known_pattern(name: &str) -> bool {
    matches!(
        name,
        "north east lines"
            | "north west lines"
            | "horizontal lines"
            | "vertical lines"
            | "grid"
            | "crosshatch"
            | "dots"
            | "crosshatch dots"
            | "fivepointed stars"
            | "sixpointed stars"
            | "bricks"
            | "checkerboard"
            | "checkerboard light gray"
            | "horizontal lines light gray"
            | "horizontal lines gray"
            | "horizontal lines dark gray"
            | "horizontal lines light blue"
            | "horizontal lines dark blue"
            | "crosshatch dots gray"
            | "crosshatch dots light steel blue"
    )
}

/// pgf's patterns (the `patterns` library), at their size in TeX: 3 pt cells, 0.4 pt lines; tiled
/// from the page's lower left corner `corner`.
fn pattern(name: &str, id: &str, c: &str, corner: (f64, f64)) -> String {
    // the cell size in bp, from 3 pt
    let t = 3.0 * 72.0 / 72.27;
    let w = 0.4 * 72.0 / 72.27;
    let line = |x1: f64, y1: f64, x2: f64, y2: f64| format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="{}"/>"#, n(x1), n(y1), n(x2), n(y2), n(w));
    let half = 0.5 * 72.0 / 72.27;
    let dot = |x: f64, y: f64| format!(r#"<circle cx="{}" cy="{}" r="{}" fill="{c}"/>"#, n(x), n(y), n(half));
    let (size, body) = match name {
        "north east lines" => (t, line(-t * 0.1, t * 1.1, t * 1.1, -t * 0.1)),
        "north west lines" => (t, line(-t * 0.1, -t * 0.1, t * 1.1, t * 1.1)),
        // (pgf's y up is y down here, a tile further: pgf's y is the tile's size less y; what pgf
        // draws on a tile's edge is drawn on both edges)
        "horizontal lines" => (t, line(0.0, t / 2.0, t, t / 2.0)),
        "vertical lines" => (t, line(t / 2.0, 0.0, t / 2.0, t)),
        "grid" => (t, line(0.0, t / 2.0, t, t / 2.0) + &line(t / 2.0, 0.0, t / 2.0, t)),
        "crosshatch" => (t, line(0.0, t, t, 0.0) + &line(0.0, 0.0, t, t)),
        "dots" => (t, [(0.0, 0.0), (t, 0.0), (0.0, t), (t, t)].iter().map(|&(x, y)| dot(x, y)).collect()),
        "crosshatch dots" => (t, [(0.0, 0.0), (t, 0.0), (0.0, t), (t, t), (t / 2.0, t / 2.0)].iter().map(|&(x, y)| dot(x, y)).collect()),
        // 3 mm tiles, a star of 1 mm around (1 mm, 1 mm)
        "fivepointed stars" | "sixpointed stars" => {
            let mm = 72.0 / 25.4;
            let at = |a: f64| {
                let (s, co) = a.to_radians().sin_cos();
                format!("{},{}", n((1.0 + co) * mm), n((3.0 - 1.0 - s) * mm))
            };
            let d = if name == "fivepointed stars" {
                format!("M{} L{} L{} L{} L{} Z", at(18.0), at(162.0), at(306.0), at(90.0), at(234.0))
            } else {
                format!("M{} L{} L{} Z M{} L{} L{} Z", at(30.0), at(150.0), at(270.0), at(-30.0), at(-270.0), at(-150.0))
            };
            (3.0 * mm, format!(r#"<path d="{d}" fill="{c}"/>"#))
        }
        // 4 mm tiles (pgf's y up is y down here, a tile further: pgf's y is the tile's size less y)
        "bricks" => {
            let mm = 72.0 / 25.4;
            let w = 0.8 * 72.0 / 72.27;
            let line = |x1: f64, y1: f64, x2: f64, y2: f64| {
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="{}"/>"#, n(x1 * mm), n(y1 * mm), n(x2 * mm), n(y2 * mm), n(w))
            };
            (4.0 * mm, line(0.0, 3.0, 4.0, 3.0) + &line(0.0, 1.0, 4.0, 1.0) + &line(1.0, 4.0, 1.0, 3.0) + &line(3.0, 3.0, 3.0, 1.0) + &line(1.0, 1.0, 1.0, 0.0))
        }
        "checkerboard" => {
            let s = 2.0 * 72.0 / 25.4;
            (s * 2.0, format!(r#"<rect y="{}" width="{}" height="{}" fill="{c}"/><rect x="{}" width="{}" height="{}" fill="{c}"/>"#, n(s), n(s), n(s), n(s), n(s), n(s)))
        }
        // the inherently colored ones, in their own colors
        "checkerboard light gray" => {
            let s = 2.0 * 72.0 / 25.4;
            let gray = tint((0.0, 0.0, 0.0), 0.2);
            (
                s * 2.0,
                format!(
                    // the white beyond the tile, which clips it: no seam where two tiles meet
                    r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#ffffff"/><rect y="{}" width="{}" height="{}" fill="{gray}"/><rect x="{}" width="{}" height="{}" fill="{gray}"/>"##,
                    n(-2.0 * s),
                    n(-2.0 * s),
                    n(6.0 * s),
                    n(6.0 * s),
                    n(s),
                    n(s),
                    n(s),
                    n(s),
                    n(s),
                    n(s)
                ),
            )
        }
        // 4 pt tiles: the first color below 2 pt, the second above
        "horizontal lines light gray" | "horizontal lines gray" | "horizontal lines dark gray" | "horizontal lines light blue" | "horizontal lines dark blue" => {
            let (base, a, b) = match name {
                "horizontal lines light gray" => ((0.0, 0.0, 0.0), 0.1, 0.15),
                "horizontal lines gray" => ((0.0, 0.0, 0.0), 0.3, 0.35),
                "horizontal lines dark gray" => ((0.0, 0.0, 0.0), 0.9, 0.85),
                "horizontal lines light blue" => ((0.0, 0.0, 1.0), 0.1, 0.15),
                _ => ((0.0, 0.0, 1.0), 0.9, 0.85),
            };
            let pt = 72.0 / 72.27;
            let s = 4.0 * pt;
            (
                s,
                format!(
                    // the stripes run on beyond the tile's sides, which it clips: no seam between tiles
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/><rect x="{}" width="{}" height="{}" fill="{}"/>"#,
                    n(-s),
                    n(2.0 * pt),
                    n(3.0 * s),
                    n(2.0 * pt),
                    tint(base, a),
                    n(-s),
                    n(3.0 * s),
                    n(2.0 * pt),
                    tint(base, b)
                ),
            )
        }
        // 8 pt tiles: a light dot just below a dark one, twice, on the background
        "crosshatch dots gray" | "crosshatch dots light steel blue" => {
            let (back, dark) = if name == "crosshatch dots gray" {
                (tint((0.0, 0.0, 0.0), 0.2), (0.0, 0.0, 0.0))
            } else {
                let steel = (0.6875, 0.765625, 0.8671875);
                (tint(steel, 1.0), (steel.0 / 2.0, steel.1 / 2.0, steel.2 / 2.0))
            };
            let pt = 72.0 / 72.27;
            let s = 8.0 * pt;
            let dot = |x: f64, y: f64, c: &str| format!(r#"<circle cx="{}" cy="{}" r="{}" fill="{c}"/>"#, n(x * pt), n((8.0 - y) * pt), n(pt));
            let (light, darker) = (tint(dark, 0.1), tint(dark, 0.7));
            let body = format!(r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{back}"/>"#, n(-s), n(-s), n(3.0 * s), n(3.0 * s))
                + &dot(2.0, 1.75, &light)
                + &dot(6.0, 5.75, &light)
                + &dot(2.0, 2.25, &darker)
                + &dot(6.0, 6.25, &darker);
            (s, body)
        }
        _ => (t, line(0.0, t, t, 0.0)),
    };
    format!(r#"<pattern id="{id}" patternUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}">{body}</pattern>"#, n(corner.0), n(corner.1), n(size), n(size))
}

/// The SVG matrix for a label: inner coordinates in bp with y down, mapped by `place` (cm, y up).
fn label_matrix(t: &Transform) -> String {
    if t.a == 1.0 && t.b == 0.0 && t.c == 0.0 && t.d == 1.0 {
        return format!("translate({} {})", n(t.e * BP), n(-t.f * BP));
    }
    format!("matrix({} {} {} {} {} {})", num(t.a, 5), num(-t.b, 5), num(-t.c, 5), num(t.d, 5), n(t.e * BP), n(-t.f * BP))
}

/// The id of a glyph's definition: `tr-g` and its number in the label font, `tr-gt` in Times.
fn glyph_id(font: MathFont, g: GlyphId) -> String {
    if font == crate::label::FONT {
        format!("tr-g{}", g.0)
    } else {
        format!("tr-gt{}", g.0)
    }
}

/// The items of a mathra box with its baseline at `origin` (label cm, y up), `scale` cm per unit,
/// glyphs `stretch` times as wide.
fn math_box(c: &mathra::layout::MathBox, origin: Point, scale: f64, stretch: f64, font: MathFont, glyphs: &mut Vec<(MathFont, GlyphId)>) -> String {
    let mut s = String::new();
    let k = scale * BP;
    let inner = |x: f64, y: f64| -> (f64, f64) { ((origin.x + x * scale) * BP, -(origin.y + y * scale) * BP) };
    for item in &c.items {
        match item {
            // (no glyph comes from a fallback font: `label::typeset` gives mathra none)
            MathItem::Glyph { id, x, y, scale: gs, color, flip, .. } => {
                glyphs.push((font, *id));
                let (px, py) = inner(*x as f64, *y as f64);
                let f = *gs as f64 * k;
                // a mirrored glyph (`\reflectbox`) is drawn leftward from its x
                let fx = if *flip { -f * stretch } else { f * stretch };
                let fill = color.as_ref().map(|c| format!(r#" fill="{c}""#)).unwrap_or_default();
                let _ = write!(s, r##"<use href="#{}" transform="translate({} {}) scale({} {})"{fill}/>"##, glyph_id(font, *id), n(px), n(py), num(fx, 5), num(-f, 5));
            }
            MathItem::Rule { x, y, width, height, color } => {
                let (px, py) = inner(*x as f64, (*y + *height) as f64);
                let fill = color.as_ref().map(|c| format!(r#" fill="{c}""#)).unwrap_or_default();
                let _ = write!(s, r#"<rect x="{}" y="{}" width="{}" height="{}"{fill}/>"#, n(px), n(py), n(*width as f64 * k), n(*height as f64 * k));
            }
            MathItem::Line { x1, y1, x2, y2, thickness, color } => {
                let (a, b) = (inner(*x1 as f64, *y1 as f64), inner(*x2 as f64, *y2 as f64));
                let _ = write!(
                    s,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                    n(a.0),
                    n(a.1),
                    n(b.0),
                    n(b.1),
                    color.as_deref().unwrap_or("currentColor"),
                    n(*thickness as f64 * k)
                );
            }
            MathItem::Ellipse { cx, cy, rx, ry, thickness, color } => {
                let (px, py) = inner(*cx as f64, *cy as f64);
                let _ = write!(
                    s,
                    r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="none" stroke="{}" stroke-width="{}"/>"#,
                    n(px),
                    n(py),
                    n(*rx as f64 * k),
                    n(*ry as f64 * k),
                    color.as_deref().unwrap_or("currentColor"),
                    n(*thickness as f64 * k)
                );
            }
            MathItem::Transformed { matrix: m, items } => {
                // the items drawn from this origin, then mapped as the box says: (x, y) in the box's
                // units goes to (a·x + c·y + e, b·x + d·y + f), y up; in SVG units, y down
                let inside = mathra::layout::MathBox { items: items.clone(), ..mathra::layout::MathBox::empty() };
                let [a, b, c, d, e, f] = m.map(f64::from);
                let _ = write!(
                    s,
                    r#"<g transform="matrix({} {} {} {} {} {})">{}</g>"#,
                    num(a, 5),
                    num(-b, 5),
                    num(-c, 5),
                    num(d, 5),
                    n((origin.x + e * scale) * BP),
                    n(-(origin.y + f * scale) * BP),
                    math_box(&inside, Point::new(0.0, 0.0), scale, stretch, font, glyphs)
                );
            }
            // a group with HTML attributes (`\class`, `\htmlId`): its items, where they are; the
            // attributes are for a web page's math, not a figure
            MathItem::Group { items, .. } => {
                let inside = mathra::layout::MathBox { items: items.clone(), ..mathra::layout::MathBox::empty() };
                s.push_str(&math_box(&inside, origin, scale, stretch, font, glyphs));
            }
        }
    }
    s
}

/// A label's lines: the first baseline at y = 0, the others below, aligned within the label's width.
fn label(l: &Label, glyphs: &mut Vec<(MathFont, GlyphId)>) -> String {
    let width = l.width();
    let mut s = String::new();
    for (k, line) in l.lines.iter().enumerate() {
        let w = line.width as f64 * l.scale;
        let x = match l.align {
            'l' => 0.0,
            'r' => width - w,
            _ => (width - w) / 2.0,
        };
        s.push_str(&math_box(line, Point::new(x, l.baseline(k)), l.scale, l.stretch, l.font, glyphs));
    }
    s
}

/// The steps of the drawing, one per path, label or image, in the order TikZ drew them: each one
/// numbered (`data-step`) and, for the construction animation, timed. Off, it writes nothing.
struct Steps {
    on: bool,
    animate: bool,
    count: usize,
    /// With the source map: where what is being drawn came from, as an attribute.
    source: String,
    /// The statement being drawn, which is a part of the figure when it has words.
    part: Option<usize>,
    /// The parts already opened, and whether one is open in the SVG now.
    told: HashSet<usize>,
    open: bool,
    /// How deep in groups the drawing is, the animations it is under, and how many there have been.
    depth: usize,
    moving: Vec<Moves>,
    animated: usize,
    /// The points open, by their depth, with the tip each ends with.
    point_tips: Vec<(usize, String)>,
    /// The points of the next path, if it is bars, and whether they stand up.
    bars: Option<(Vec<DataPoint>, bool)>,
    /// Bars or marks were written: the SVG needs its rules.
    has_points: bool,
    has_bars: bool,
}

/// A group of `Item::Animated` being drawn: its depth, the `</g>`s beyond its own, and what each
/// element in it animates.
struct Moves {
    depth: usize,
    closes: usize,
    id: String,
    animations: Vec<Animation>,
}

/// The start of a point's group: focusable, with its values for a page, a reader and its tip.
fn point_open(p: &DataPoint) -> String {
    let (x, y) = (escape(&p.x), escape(&p.y));
    format!(r#"<g class="tr-point" tabindex="0" aria-label="{x}, {y}" data-x="{x}" data-y="{y}">"#)
}

/// A point's tip, over where it is: hidden until the point is under the mouse or has the focus.
fn point_tip(p: &DataPoint) -> String {
    format!(r#"<text class="tr-tip" x="{}" y="{}">{}, {}</text>"#, n(p.at.x * BP), n(-p.at.y * BP - 4.0), escape(&p.x), escape(&p.y))
}

/// The class a bar grows with: from its base, toward its value.
fn bar_class(p: &DataPoint, vertical: bool) -> &'static str {
    let down = p.x.starts_with('-') && !vertical || p.y.starts_with('-') && vertical;
    match (vertical, down) {
        (true, false) => "tr-up",
        (true, true) => "tr-down",
        (false, false) => "tr-right",
        (false, true) => "tr-left",
    }
}

/// The path's subpaths, each its own.
fn subpaths(segments: &[Segment]) -> Vec<&[Segment]> {
    let starts: Vec<usize> = segments.iter().enumerate().filter(|(_, s)| matches!(s, Segment::MoveTo(_))).map(|(i, _)| i).collect();
    starts.iter().enumerate().map(|(k, &a)| &segments[a..starts.get(k + 1).copied().unwrap_or(segments.len())]).collect()
}

/// An animation as an SVG element: on the object with the id `id` if it waits for an event.
fn smil(a: &Animation, id: &str) -> String {
    let (first, last) = (a.times[0], a.times[a.times.len() - 1]);
    let start = a.begin + first;
    let begin = match &a.on {
        Some(event) => format!("{id}.{event}+{}s", n(start)),
        None => format!("{}s", n(start)),
    };
    let dur = (last - first).max(0.001);
    let key_times: Vec<String> = a.times.iter().map(|t| n((t - first) / dur)).collect();
    let (values, key_times) = if a.times.len() < 2 { (format!("{0};{0}", a.values[0]), "0;1".to_string()) } else { (a.values.join(";"), key_times.join(";")) };
    let head = match &a.moving {
        Moving::Attribute(name, _) => format!(r#"<animate attributeName="{name}""#),
        Moving::Shift => r#"<animateTransform attributeName="transform" type="translate""#.to_string(),
        Moving::Rotation => r#"<animateTransform attributeName="transform" type="rotate""#.to_string(),
        Moving::Scaling => r#"<animateTransform attributeName="transform" type="scale""#.to_string(),
    };
    let repeat = a.repeats.as_ref().map_or(String::new(), |r| format!(r#" repeatCount="{r}""#));
    format!(r#"{head} values="{values}" keyTimes="{key_times}" dur="{}s" begin="{begin}"{repeat} fill="{}"/>"#, n(dur), if a.freeze { "freeze" } else { "remove" })
}

/// The animations of the attributes an element has, inside it: an attribute it does not draw
/// (a fill on a line) is left alone.
fn animate_element(body: &mut String, at: usize, moving: &[Moves]) {
    // a fill and a stroke at opacities of their own: a group of two paths, each animated as itself
    let group = body[at..].starts_with("<g") && !body[at..].starts_with("<g transform=") && body.ends_with("/></g>");
    let open = at + body[at..].find('>').map_or(0, |i| i + 1);
    if let Some(paths) = group.then(|| body[open..body.len() - 4].to_string()).filter(|p| p.split_inclusive("/>").all(|p| p.starts_with("<path"))) {
        body.truncate(open);
        for p in paths.split_inclusive("/>") {
            let start = body.len();
            body.push_str(p);
            animate_element(body, start, moving);
        }
        body.push_str("</g>");
        return;
    }
    let element = &body[at..];
    let is_path = element.starts_with("<path") && element.ends_with("/>");
    let is_text = element.starts_with("<g transform=");
    if !is_path && !is_text {
        return;
    }
    let drawn = |name: &str| -> bool {
        match name {
            "stroke" | "stroke-width" | "stroke-opacity" => element.contains(r#" stroke=""#),
            "fill" | "fill-opacity" => !element.contains(r#" fill="none""#),
            _ => true,
        }
    };
    let mut inside = String::new();
    for m in moving {
        for a in &m.animations {
            if let Moving::Attribute(name, only_text) = a.moving {
                if drawn(name) && (is_text || !only_text) && !(is_text && name.starts_with("stroke")) {
                    inside.push_str(&smil(a, &m.id));
                }
            }
        }
    }
    if inside.is_empty() {
        return;
    }
    if is_path {
        body.truncate(body.len() - 2);
        body.push('>');
        body.push_str(&inside);
        body.push_str("</path>");
    } else if let Some(i) = body[at..].find('>') {
        body.insert_str(at + i + 1, &inside);
    }
}

impl Steps {
    /// For what is not drawn in place: a pattern's tile, a mask.
    fn off() -> Steps {
        Steps {
            on: false,
            animate: false,
            count: 0,
            source: String::new(),
            part: None,
            told: HashSet::new(),
            open: false,
            depth: 0,
            moving: Vec::new(),
            animated: 0,
            point_tips: Vec::new(),
            bars: None,
            has_points: false,
            has_bars: false,
        }
    }

    /// The attributes of the next step: its number and, animated, a stroke drawn along its length
    /// (and its fill after it) or anything else fading in. A dashed stroke fades in, since
    /// `pathLength` would stretch its dashes.
    fn next(&mut self, drawn: bool) -> String {
        if !self.on {
            return String::new();
        }
        let i = self.count;
        self.count += 1;
        let source = &self.source;
        match (self.animate, drawn) {
            (false, _) => format!(r#" data-step="{i}"{source}"#),
            (true, true) => format!(r#" data-step="{i}"{source} class="tr-d" pathLength="1" style="--i:{i}""#),
            (true, false) => format!(r#" data-step="{i}"{source} class="tr-f" style="--i:{i}""#),
        }
    }

    /// Before an element: the group of its part, one a reader can reach. The first group of a part
    /// has its words; what the part draws after a clip or a group ends, and a part with nothing to
    /// say, are in others, kept out of the reader's way.
    fn enter(&mut self, body: &mut String, parts: &[String]) {
        let (false, Some(id)) = (self.open, self.part) else { return };
        let Some(words) = parts.get(id) else { return };
        self.open = true;
        // the role `img` makes a part one stop: inside a group, a screen reader stops at each of its
        // paths and glyphs, and says only "image"
        if !words.is_empty() && self.told.insert(id) {
            let _ = write!(body, r#"<g role="img" tabindex="0" aria-label="{}" data-part="{id}">"#, escape(words));
        } else {
            let _ = write!(body, r#"<g aria-hidden="true" data-part="{id}">"#);
        }
    }

    /// The group of a part ends where another part, a clip or a group starts or ends.
    fn leave(&mut self, body: &mut String) {
        if std::mem::take(&mut self.open) {
            body.push_str("</g>");
        }
    }

    /// How many steps there are, on the `<svg>`.
    fn total(&self) -> String {
        if self.on {
            format!(r#" data-steps="{}""#, self.count)
        } else {
            String::new()
        }
    }

    /// The animation's rules: all the steps within about six seconds, however many there are.
    fn style(&self) -> String {
        let tips = if self.has_points {
            "<style>.tr-tip{visibility:hidden;font:7px sans-serif;text-anchor:middle;fill:#15120E;stroke:#fff;stroke-width:2px;paint-order:stroke}\
             .tr-point:hover .tr-tip,.tr-point:focus .tr-tip{visibility:visible}\
             .tr-point:focus{outline:1px solid currentColor}</style>"
        } else {
            ""
        };
        if !self.animate || self.count == 0 {
            return tips.to_string();
        }
        let step = (6.0 / self.count as f64).min(0.25);
        let bars = if self.has_bars {
            format!(
                "<style>@keyframes tr-gy{{from{{transform:scaleY(0)}}}}@keyframes tr-gx{{from{{transform:scaleX(0)}}}}\
                 .tr-up,.tr-down,.tr-left,.tr-right{{transform-box:fill-box;animation-duration:.6s;animation-timing-function:ease-out;animation-fill-mode:both;animation-delay:calc(var(--i)*{}s)}}\
                 .tr-up,.tr-down{{animation-name:tr-gy}}.tr-left,.tr-right{{animation-name:tr-gx}}\
                 .tr-up{{transform-origin:50% 100%}}.tr-down{{transform-origin:50% 0}}.tr-right{{transform-origin:0 50%}}.tr-left{{transform-origin:100% 50%}}\
                 @media (prefers-reduced-motion:reduce){{.tr-up,.tr-down,.tr-left,.tr-right{{animation:none}}}}</style>",
                num(step, 4)
            )
        } else {
            String::new()
        };
        format!(
            "{tips}{bars}<style>@keyframes tr-d{{from{{stroke-dashoffset:1.01;fill-opacity:0}}70%{{stroke-dashoffset:0;fill-opacity:0}}}}\
             @keyframes tr-f{{from{{opacity:0}}}}\
             .tr-d{{stroke-dasharray:1 2;animation:tr-d .9s ease-in-out both}}\
             .tr-f{{animation:tr-f .45s ease-out both}}\
             .tr-d,.tr-f{{animation-delay:calc(var(--i)*{}s)}}\
             @media (prefers-reduced-motion:reduce){{.tr-d,.tr-f{{animation:none;stroke-dasharray:none}}}}</style>",
            num(step, 4)
        )
    }
}

/// The straight edges of a path of lines only, each subpath closed; `None` with a curve.
fn line_edges(segments: &[Segment]) -> Option<Vec<(Point, Point)>> {
    let mut edges = Vec::new();
    let (mut start, mut last) = (None, None);
    let close = |edges: &mut Vec<(Point, Point)>, start: Option<Point>, last: Option<Point>| {
        if let (Some(a), Some(b)) = (last, start) {
            edges.push((a, b));
        }
    };
    for s in segments {
        match s {
            Segment::MoveTo(p) => {
                close(&mut edges, start, last);
                (start, last) = (Some(*p), Some(*p));
            }
            Segment::LineTo(p) => {
                edges.push((last?, *p));
                last = Some(*p);
            }
            Segment::Close => {
                close(&mut edges, start, last);
                last = start;
            }
            Segment::CurveTo(..) => return None,
        }
    }
    close(&mut edges, start, last);
    Some(edges.into_iter().filter(|(a, b)| (a.x - b.x).hypot(a.y - b.y) > 1e-9).collect())
}

/// Seams: where two opaque fills of one color each, drawn in the same place of the picture, share
/// an edge, a rasterizer antialiases each against what is under it, and that shows through as a
/// line along the edge (a PDF rasterizer does not: it extends fills a little). For each such
/// fill (by its index in `items`), the edges it shares with a fill drawn after it, and how wide a
/// stroke of its color along them may be: the one after covers the stroke's half on its side,
/// the other half is on the fill's own side.
fn seams(items: &[Item]) -> HashMap<usize, (Vec<(Point, Point)>, f64)> {
    /// A fill that may share an edge: its item, where it is drawn, its edges, the narrower side of
    /// its box and a point inside it (of a convex fill: which side of an edge the fill is on).
    struct Side {
        item: usize,
        place: usize,
        edges: Vec<(Point, Point)>,
        narrow: f64,
        inside: Point,
    }
    type Key = (i64, i64);
    let key = |p: Point| -> Key { ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64) };
    let edge_key = |(a, b): (Point, Point)| {
        let (a, b) = (key(a), key(b));
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    };
    // the fills; per edge, the ones that have it
    let mut fills: Vec<Side> = Vec::new();
    let mut owners: HashMap<(usize, (Key, Key)), Vec<usize>> = HashMap::new();
    // what is drawn in one group, transformation or clip
    let mut place = 0;
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Path { segments, fill: Some(Fill { paint: Paint::Solid(_), opacity, .. }), stroke: None } if *opacity >= 1.0 => {
                let Some(edges) = line_edges(segments) else { continue };
                let (min, max) = edges
                    .iter()
                    .flat_map(|(a, b)| [*a, *b])
                    .fold(((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)), |(lo, hi), p| ((lo.0.min(p.x), lo.1.min(p.y)), (hi.0.max(p.x), hi.1.max(p.y))));
                for e in &edges {
                    owners.entry((place, edge_key(*e))).or_default().push(fills.len());
                }
                let inside = Point { x: (min.0 + max.0) / 2.0, y: (min.1 + max.1) / 2.0 };
                fills.push(Side { item: i, place, edges, narrow: (max.0 - min.0).min(max.1 - min.1), inside });
            }
            Item::Path { .. } | Item::Text { .. } | Item::Image { .. } | Item::Source(_) | Item::Bars { .. } => {}
            _ => place += 1,
        }
    }
    let mut out = HashMap::new();
    let side_of = |(a, b): (Point, Point), p: Point| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    // whether a fill with these edges has an angle under 90° at an end of the edge `e`
    let sharp = |edges: &[(Point, Point)], e: (Point, Point)| {
        [(e.0, e.1), (e.1, e.0)].iter().any(|&(p, q)| {
            edges
                .iter()
                .filter_map(|&(a, b)| {
                    if key(a) == key(p) {
                        Some(b)
                    } else if key(b) == key(p) {
                        Some(a)
                    } else {
                        None
                    }
                })
                .filter(|&r| key(r) != key(q))
                .any(|r| (q.x - p.x) * (r.x - p.x) + (q.y - p.y) * (r.y - p.y) > 1e-12)
        })
    };
    for (k, Side { item, place, edges, narrow, inside }) in fills.iter().enumerate() {
        let mut shared = Vec::new();
        let mut width = f64::MAX;
        for e in edges {
            // one after, on the other side of the edge: one that folds back over this fill would
            // not cover the stroke's half outside it
            // and with a right or wider angle at both ends, in both: the stroke's square ends stay
            // inside the two (they would stick out of a sharp corner)
            let beside = |&&j: &&usize| j > k && side_of(*e, *inside) * side_of(*e, fills[j].inside) < 0.0 && !sharp(edges, *e) && !sharp(&fills[j].edges, *e);
            if let Some(later) = owners[&(*place, edge_key(*e))].iter().find(beside) {
                shared.push(*e);
                width = width.min(narrow.min(fills[*later].narrow) / 2.0);
            }
        }
        if !shared.is_empty() && width > 0.0 {
            out.insert(*item, (shared, width));
        }
    }
    out
}

/// The widest stroke that covers a seam, in cm: 2 bp, two pixels at 100 dpi.
const SEAM: f64 = 2.0 / BP;

/// The items as SVG elements, into `body`; their definitions (gradients, clips, masks) into `defs`.
fn items_svg(items: &[Item], body: &mut String, defs: &mut Defs, glyphs: &mut Vec<(MathFont, GlyphId)>, steps: &mut Steps, sources: &[SourceSpan], parts: &[String]) {
    let seams = seams(items);
    for (i, item) in items.iter().enumerate() {
        let drawn = match item {
            Item::Path { segments, fill, stroke } => !segments.is_empty() && (fill.is_some() || stroke.is_some()),
            Item::Text { .. } | Item::Image { .. } => true,
            _ => false,
        };
        if drawn {
            steps.enter(body, parts);
        } else if !matches!(item, Item::Path { .. }) {
            steps.leave(body);
        }
        if matches!(
            item,
            Item::Clip { .. } | Item::Group { .. } | Item::Crisp | Item::Blend { .. } | Item::Transform { .. } | Item::Fade { .. } | Item::FadePicture { .. } | Item::Point(_)
        ) {
            steps.depth += 1;
        }
        let at = body.len();
        'item: {
            match item {
                Item::Path { segments, fill, stroke } => {
                    if segments.is_empty() || (fill.is_none() && stroke.is_none()) {
                        break 'item;
                    }
                    let d = path_data(segments);
                    if let (Some((edges, width)), Some(f @ Fill { paint: Paint::Solid(c), .. })) = (seams.get(&i).filter(|_| steps.bars.is_none()), fill) {
                        let fa = fill_attributes(f, defs);
                        let seam: Vec<Segment> = edges.iter().flat_map(|(a, b)| [Segment::MoveTo(*a), Segment::LineTo(*b)]).collect();
                        let _ = write!(
                            body,
                            r#"<g{}><path d="{d}"{fa}/><path d="{}" fill="none" stroke="{}" stroke-width="{}"/></g>"#,
                            steps.next(false),
                            path_data(&seam),
                            c.hex(),
                            n(width.min(SEAM) * BP)
                        );
                        break 'item;
                    }
                    let mut a = String::new();
                    // filled and stroked at one opacity: PDF paints the two as one group, so the stroke
                    // does not darken the fill under it; SVG's `opacity` of the element does the same
                    let shared = match (fill, stroke) {
                        (Some(f), Some(s)) if f.opacity < 1.0 && (f.opacity - s.opacity).abs() < 1e-9 => Some(f.opacity),
                        _ => None,
                    };
                    // at different opacities, PDF's group is a knockout one: a stroke that lets through
                    // shows what is under the path, not the fill; the fill is masked out under the stroke
                    if let (Some(f), Some(s), None) = (fill, stroke, shared) {
                        if s.opacity < 1.0 {
                            let id = defs.id("m");
                            let cut = stroke_attributes(&Stroke { color: crate::color::Color::BLACK, opacity: 1.0, ..s.clone() });
                            let _ = write!(
                                defs.text,
                                r#"<mask id="{id}" maskUnits="userSpaceOnUse" x="-100000" y="-100000" width="200000" height="200000"><rect x="-100000" y="-100000" width="200000" height="200000" fill="white"/><path d="{d}" fill="none"{cut}/></mask>"#
                            );
                            let fa = fill_attributes(f, defs);
                            let _ = write!(body, r#"<g{}><path d="{d}"{fa} mask="url(#{id})"/><path d="{d}" fill="none"{}/></g>"#, steps.next(false), stroke_attributes(s));
                            break 'item;
                        }
                    }
                    let (fill, stroke) = match shared {
                        Some(_) => (fill.clone().map(|f| Fill { opacity: 1.0, ..f }), stroke.clone().map(|s| Stroke { opacity: 1.0, ..s })),
                        None => (fill.clone(), stroke.clone()),
                    };
                    match &fill {
                        Some(f) => a.push_str(&fill_attributes(f, defs)),
                        None => a.push_str(r#" fill="none""#),
                    }
                    if let Some(s) = &stroke {
                        a.push_str(&stroke_attributes(s));
                    }
                    if let Some(o) = shared {
                        let _ = write!(a, r#" opacity="{}""#, n(o.max(0.0)));
                    }
                    let dashed = a.contains("stroke-dasharray");
                    if let Some((points, vertical)) = steps.bars.take() {
                        let parts = subpaths(segments);
                        if parts.len() == points.len() {
                            steps.has_points = true;
                            steps.has_bars = true;
                            for (part, p) in parts.iter().zip(&points) {
                                let step = steps.next(false).replace(r#"class="tr-f""#, &format!(r#"class="{}""#, bar_class(p, vertical)));
                                let _ = write!(body, r#"{}<path d="{}"{a}{step}/>{}</g>"#, point_open(p), path_data(part), point_tip(p));
                            }
                            break 'item;
                        }
                    }
                    a.push_str(&steps.next(stroke.is_some() && !dashed));
                    let _ = write!(body, r#"<path d="{d}"{a}/>"#);
                }
                Item::Text { label: l, place, opacity } => {
                    let op = if *opacity < 1.0 { format!(r#" opacity="{}""#, n(*opacity)) } else { String::new() };
                    // `color` too: mathra's lines (bonds, cancels, fraction-free strokes) are drawn in currentColor
                    let _ = write!(body, r#"<g transform="{}" fill="{c}" color="{c}"{op}{}>"#, label_matrix(place), steps.next(false), c = l.color.hex());
                    body.push_str(&label(l, glyphs));
                    body.push_str("</g>");
                }
                Item::Clip { segments, even_odd } => {
                    let id = defs.id("c");
                    let rule = if *even_odd { r#" clip-rule="evenodd""# } else { "" };
                    let _ = write!(defs.text, r#"<clipPath id="{id}"><path d="{}"{rule}/></clipPath>"#, path_data(segments));
                    let _ = write!(body, r#"<g clip-path="url(#{id})">"#);
                }
                Item::Group { opacity } => {
                    if *opacity < 1.0 {
                        let _ = write!(body, r#"<g opacity="{}">"#, n(opacity.max(0.0)));
                    } else {
                        body.push_str(r#"<g style="isolation:isolate">"#);
                    }
                }
                Item::Crisp => body.push_str(r#"<g shape-rendering="crispEdges">"#),
                Item::Blend { mode } => {
                    let _ = write!(body, r#"<g style="mix-blend-mode:{mode}">"#);
                }
                Item::Transform { transform: t } => {
                    // the canvas transformation in the SVG's coordinates: y down, in bp
                    let _ = write!(body, r#"<g transform="matrix({} {} {} {} {} {})">"#, n(t.a), n(-t.b), n(-t.c), n(t.d), n(t.e * BP), n(-t.f * BP));
                }
                Item::Fade { mask } => {
                    let gradient = paint_value(&Paint::Shading(mask.clone()), defs);
                    let id = defs.id("m");
                    let _ = write!(
                        defs.text,
                        r#"<mask id="{id}" maskUnits="userSpaceOnUse" x="-100000" y="-100000" width="200000" height="200000"><rect x="-100000" y="-100000" width="200000" height="200000" fill="{gradient}"/></mask>"#
                    );
                    let _ = write!(body, r#"<g mask="url(#{id})">"#);
                }
                Item::FadePicture { items } => {
                    // the picture as a luminance mask: white opaque, black and nothing transparent
                    let mut inner = String::new();
                    // a mask is not drawn: it is not animated
                    items_svg(items, &mut inner, defs, glyphs, &mut Steps::off(), sources, &[]);
                    let id = defs.id("m");
                    let _ = write!(defs.text, r#"<mask id="{id}" maskUnits="userSpaceOnUse" x="-100000" y="-100000" width="200000" height="200000">{inner}</mask>"#);
                    let _ = write!(body, r#"<g mask="url(#{id})">"#);
                }
                Item::Image { png, place: t } => {
                    // the unit square, y up, in the SVG's coordinates: y down, in bp; pdfTeX writes no
                    // /Interpolate, and PDF rasterizers sample the image's pixels
                    let m = [t.a * BP, -t.b * BP, -t.c * BP, t.d * BP, (t.c + t.e) * BP, -(t.d + t.f) * BP];
                    let _ = write!(
                        body,
                        r#"<image width="1" height="1" preserveAspectRatio="none" image-rendering="optimizeSpeed" transform="matrix({} {} {} {} {} {})"{} href="data:image/png;base64,{}"/>"#,
                        num(m[0], 4),
                        num(m[1], 4),
                        num(m[2], 4),
                        num(m[3], 4),
                        n(m[4]),
                        n(m[5]),
                        steps.next(false),
                        base64(png)
                    );
                }
                Item::Point(p) => {
                    steps.has_points = true;
                    body.push_str(&point_open(p));
                    steps.point_tips.push((steps.depth, point_tip(p)));
                }
                Item::Bars { points, vertical } => steps.bars = Some((points.clone(), *vertical)),
                Item::End => {
                    if steps.point_tips.last().is_some_and(|t| t.0 == steps.depth) {
                        if let Some((_, tip)) = steps.point_tips.pop() {
                            body.push_str(&tip);
                        }
                    }
                    body.push_str("</g>");
                    if steps.moving.last().is_some_and(|m| m.depth == steps.depth) {
                        if let Some(m) = steps.moving.pop() {
                            body.push_str(&"</g>".repeat(m.closes));
                        }
                    } else {
                        steps.depth = steps.depth.saturating_sub(1);
                    }
                }
                Item::Animated { animations, center } => {
                    steps.animated += 1;
                    // no `-` in the id: a `begin` would read it as a time offset
                    let id = format!("trAnim{}", steps.animated);
                    let _ = write!(body, r#"<g id="{id}">"#);
                    let mut closes = 0;
                    let (cx, cy) = (n(center.x * BP), n(-center.y * BP));
                    for a in animations {
                        match a.moving {
                            Moving::Attribute(..) => {}
                            Moving::Shift => {
                                let _ = write!(body, "<g>{}", smil(a, &id));
                                closes += 1;
                            }
                            // SVG scales around the origin: the object is taken there and back
                            Moving::Rotation | Moving::Scaling => {
                                let _ =
                                    write!(body, r#"<g transform="translate({cx} {cy})"><g>{}<g transform="translate({} {})">"#, smil(a, &id), n(-center.x * BP), n(center.y * BP));
                                closes += 3;
                            }
                        }
                    }
                    steps.moving.push(Moves { depth: steps.depth, closes, id, animations: animations.clone() });
                }
                Item::Source(id) => {
                    steps.part = Some(*id);
                    steps.source = sources.get(*id).map_or(String::new(), |s| format!(r#" data-src="{}:{}-{}:{}""#, s.first.0, s.first.1, s.last.0, s.last.1));
                }
            }
        }
        if drawn && !steps.moving.is_empty() {
            animate_element(body, at, &steps.moving);
        }
    }
    steps.leave(body);
}

/// Standard base64, with padding (an image's `data:` URI).
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let v = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            if k <= c.len() {
                s.push(char::from(ABC[(v >> (18 - 6 * k)) as usize & 63]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// FNV-1a: a hash that is the same on every platform, for ids that depend only on the input.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3))
}

pub fn svg(canvas: &Canvas, source: &str, words: &crate::describe::Description) -> String {
    svg_with(canvas, source, words, false, false)
}

/// [`svg`], with each element numbered by its step when `steps`, and with the construction
/// animation when `animate` (steps included): the figure builds itself in the order it was drawn,
/// in any browser, with CSS alone.
pub fn svg_with(canvas: &Canvas, source: &str, words: &crate::describe::Description, steps: bool, animate: bool) -> String {
    let m = canvas.margin;
    let (min, max) = match (canvas.min, canvas.max) {
        (Some(a), Some(b)) => (a, b),
        _ => (Point::default(), Point::default()),
    };
    let (x0, y0, x1, y1) = (min.x - m, min.y - m, max.x + m, max.y + m);
    let (width, height) = (((x1 - x0) * BP).max(0.0), ((y1 - y0) * BP).max(0.0));

    let mut body = String::new();
    // the page under the figure, as far as the figure's page goes
    if let Some(c) = canvas.page_color {
        let _ = write!(body, r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#, n(x0 * BP), n(-y1 * BP), n(width), n(height), c.hex());
    }
    let mut defs = Defs { text: String::new(), count: 0, glyphs: Vec::new(), corner: (x0 * BP, -y0 * BP) };
    let mut glyphs = Vec::new();
    let mut steps = Steps {
        on: steps || animate,
        animate,
        count: 0,
        source: String::new(),
        part: None,
        told: HashSet::new(),
        open: false,
        depth: 0,
        moving: Vec::new(),
        animated: 0,
        point_tips: Vec::new(),
        bars: None,
        has_points: false,
        has_bars: false,
    };
    items_svg(&canvas.items, &mut body, &mut defs, &mut glyphs, &mut steps, &canvas.sources, &canvas.parts);
    glyphs.append(&mut defs.glyphs);
    let mut seen = HashSet::new();
    for (font, g) in glyphs {
        if seen.insert((font, g.0)) {
            let d = if font == crate::label::TIMES && crate::times::is_termes(g) { crate::times::outline(g) } else { font.get().outline(g) };
            let _ = write!(defs.text, r#"<path id="{}" d="{d}"/>"#, glyph_id(font, g));
        }
    }
    // the words' ids are the figure's own: a page with many figures labels each by its own words
    let id = fnv(source.as_bytes()) & 0xffff_ffff;
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:tikra="urn:tikra:1" viewBox="{} {} {} {}" width="{}pt" height="{}pt" role="{}" aria-labelledby="tr-title-{id:08x}"{}{}>"#,
        n(x0 * BP),
        n(-y1 * BP),
        n(width),
        n(height),
        n(width),
        n(height),
        // the role `img` would hide the parts inside it
        if canvas.parts.is_empty() { "img" } else { "group" },
        if words.by_author { String::new() } else { format!(r#" aria-describedby="tr-desc-{id:08x}""#) },
        steps.total()
    );
    let _ = write!(s, r#"<title id="tr-title-{id:08x}">{}</title>"#, escape(&words.alt));
    // the author's text is the whole of it; tikra's own has a longer description
    if !words.by_author {
        let _ = write!(s, r#"<desc id="tr-desc-{id:08x}">{}</desc>"#, escape(&words.text));
    }
    let _ = write!(s, r#"<metadata><tikra:source type="tikz">{}</tikra:source></metadata>"#, escape(source));
    // a screen reader would stop at each glyph kept here, with only "image" to say
    let hidden = if canvas.parts.is_empty() { "" } else { r#" aria-hidden="true""# };
    let _ = write!(s, "{}<defs{hidden}>{}</defs>{body}</svg>", steps.style(), defs.text);
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_pads() {
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"M"), "TQ==");
        assert_eq!(super::base64(b""), "");
    }
}
