//! The folding library: nets of polyhedra to cut out and fold (`tikzlibraryfolding`).
//!
//! `\pic[folding line length=6mm, face 1={…}] {cube folding};` draws a net: polygons that share
//! edges, each built on an edge of its parent. A polygon of the net has its face (the code of
//! `face N`, placed at the polygon's center) and an item on each of its other edges: a fold
//! (`every fold`), a cut (`every cut`), an ear to glue (`every ear`, cut around and folded along
//! the edge), or another polygon of the net. Every edge has its own frame: its start at the origin,
//! its direction along x, the outside of the polygon above it, so an item is drawn the same way on
//! any edge. `numbered faces` puts each face's number in it.
//!
//! The nets are written in a notation of tikra's: `P(1 f c e T(2 …))` is a pentagon with face 1,
//! then its edges in order (`f` fold, `c` cut, `e` ear, `<a` and `>a` ears leaning by `a` degrees),
//! one of them a triangle with face 2; `3@90` is face 3 turned by 90 degrees. The kinds are `T`
//! triangle, `S` square, `P` pentagon, `H` hexagon, `O` octagon, `D` decagon, and the rhombi `R`
//! and `L` (of the rhombic dodecahedron, leaning right and left) and `G` and `g` (golden ones).
//! The edges of a polygon come clockwise from the one after its top vertex (or its top edge), so
//! the edge it hangs from is among them, as in TikZ.

use crate::scene::{Point, Transform};
use crate::tex::{Meaning, Token};
use crate::{Error, PT};

use super::{tikra_tokens, Engine};

/// Ids of the folding library's commands (`Meaning::Native`), after the calendar's.
pub const BASE: u16 = 880;

const COMMANDS: &[&str] = &["tikra@folding"];

pub fn define(e: &mut Engine) {
    for (i, name) in COMMANDS.iter().enumerate() {
        e.tex.define(name, Meaning::Native(BASE + i as u16));
    }
}

pub fn is_folding(id: u16) -> bool {
    (BASE..BASE + COMMANDS.len() as u16).contains(&id)
}

/// The nets: the name of TikZ's `\tikzfolding…` command, its pic (`… folding`), and the net.
const NETS: &[(&str, Option<&str>, &str)] = &[
    (
        "dodecahedron",
        Some("dodecahedron"),
        "P(1 P(2 P(3 e e f e e) e f e e) P(4 P(5 e e f c e) e f c c) P(6 P(7 e e f c e) e f c c) P(8 P(9 e e f c e) e f c c) P(10 P(11 c c f c P(12 c \
         c f c c)) c f c c))",
    ),
    (
        "alternatedodecahedron",
        Some("dodecahedron'"),
        "P(1 P(2 c e f c P(3 P(4 e e f c e) P(5 e e f c e) f P(6 e e f e c) P(7 e e f c e))) e P(8 P(9 c e f c c) P(10 c c f c c) f P(11 c e f c c) \
         P(12 c e f c c)) e e)",
    ),
    ("tetrahedron", Some("tetrahedron"), "T(1 T(2 e f c) T(3 e f c) T(4 e f c))"),
    ("cube", Some("cube"), "S(1 S(2 c e f e) S(3 c c f c) S(4 S(5 e c f e) e f e) S(6 e c f c))"),
    ("octahedron", Some("octahedron"), "T(1 T(2 T(3 c f T(4 c f c)) f T(5 e f e)) T(6 e f e) T(7 c f T(8 e f c)))"),
    (
        "icosahedron",
        Some("icosahedron"),
        "T(1 T(2 T(3 T(4 c f T(5 c f T(6 e f T(7 e f c)))) f e) f e) T(8 T(9 T(10 c f T(11 c f T(12 T(13 c f c) f T(14 e f c)))) f e) f e) T(15 T(16 \
         T(17 c f T(18 c f T(19 e f T(20 e f c)))) f e) f e))",
    ),
    ("truncatedtetrahedron", Some("tetrahedron truncated"), "H(1 H(2 e e e f e c) T(3 c f c) H(4 e e e f e c) T(5 c f c) H(6 T(7 c f c) e e f e c) T(8 c f c))"),
    (
        "cuboctahedron",
        Some("cuboctahedron"),
        "S(1 T(2 S(3 T(4 c f S(5 e e f e)) e f e) f c) T(6 S(7 e e f e) f c) T(8 S(9 e e f T(10 c f c)) f c) T(11 S(12 T(13 c f c) e f T(14 c f c)) f \
         c))",
    ),
    (
        "truncatedcube",
        Some("cube truncated"),
        "O(1 O(2 e T(3 e f e) c T(4 e f e) f T(5 e f e) c T(6 e f e)) c O(7 c c e T(8 e f e) f c e c) c O(9 O(10 c c e T(11 e f e) f T(12 e f e) e c) \
         c c c f c c c) c O(13 c c e c f T(14 e f e) e c) c)",
    ),
    (
        "truncatedoctahedron",
        Some("octahedron truncated"),
        "H(1 H(2 S(3 e c f c) H(4 e e e f e e) S(5 e e f c) f S(6 e c f e) H(7 e e e f e e)) c e S(8 H(9 H(10 c c e f e c) S(11 c e f c) H(12 c c c f \
         c c) f H(13 c c c f c c) S(14 c c f e)) e f e) e c)",
    ),
    (
        "rhombicuboctahedron",
        Some("rhombicuboctahedron"),
        "S(1 S(2 S(3 S(4 S(5 c S(6 e e f c) f S(7 e c f e)) T(8 e f c) f T(9 c f e)) S(10 e e f c) f S(11 e c f e)) T(12 e f c) f T(13 c f e)) S(14 \
         S(15 c c f c) e f c) S(16 S(17 S(18 e T(19 c f e) f T(20 e f c)) S(21 e c f e) f S(22 e e f c)) T(23 c f e) f T(24 e f c)) S(25 S(26 c c f \
         c) c f e))",
    ),
    (
        "truncatedcuboctahedron",
        Some("cuboctahedron truncated"),
        "S(1 O(2 S(3 O(4 S(5 c H(6 e e >15 f e e) f H(7 e e e f <15 e)) c S(8 e c f c) c f c S(9 e c f c) c) H(10 e e >15 f <15 e) f H(11 e e >15 f \
         <15 e)) c S(12 e c f c) c f c S(13 e c f c) c) H(14 e e >15 f <15 e) O(15 S(16 O(17 e c S(18 e c f c) c f c S(19 e c f c) c) H(20 e e >15 f \
         <15 e) f H(21 e e >15 f <15 e)) c S(22 O(23 c c c c f c c c) c f c) c f c S(24 O(25 c c c c f c c c) c f c) c) H(26 e e >15 f <15 e))",
    ),
    (
        "snubcube",
        Some("snub cube"),
        "S(1 T(2 T(3 T(4 e f S(5 T(6 T(7 e f e) f T(8 e f c)) T(9 e f e) f T(10 c f c))) f c) f c) T(11 T(12 T(13 e f S(14 T(15 T(16 e f e) f T(17 e \
         f c)) T(18 e f e) f T(19 c f c))) f c) f c) T(20 T(21 T(22 e f S(23 T(24 T(25 e f e) f T(26 e f c)) T(27 e f e) f T(28 c f c))) f c) f c) \
         T(29 T(30 T(31 e f S(32 T(33 T(34 e f e) f T(35 S(36 c c f c) f c)) T(37 e f e) f T(38 c f c))) f c) f c))",
    ),
    (
        "icosidodecahedron",
        Some("icosidodecahedron"),
        "P(1 T(2 P(3 T(4 e f e) >24 f c T(5 e f c)) f c) T(6 P(7 e >24 f c T(8 e f P(9 e T(10 e f e) f c c))) f c) T(11 >24 f c) T(12 P(13 T(14 e f \
         P(15 T(16 e f e) e f c c)) >24 f c T(17 >24 f c)) f P(18 T(19 P(20 e >24 f T(21 P(22 c >24 f c e) f c) T(23 e f c)) f c) T(24 >24 f c) f c \
         T(25 P(26 e >24 f c c) f c))) T(27 P(28 T(29 e f P(30 T(31 c f c) e f c c)) >24 f c T(32 >24 f c)) f c))",
    ),
    (
        "rhombicdodecahedron",
        Some("dodecahedron rhombic"),
        "R(2 L(3@90 c e f e) R(5 L(6@90 c c f e) c f R(4@180 c c f c)) L(8@-90 R(7@180 L(12@-90 R(11@180 e c f e) L(10@90 e c f e) f e) c f e) L(8 e \
         e f e) f e) R(6 c c f c))",
    ),
    (
        "goldenrhombicdodecahedron",
        Some("dodecahedron golden"),
        "G(1 g(2@180 c e f f) G(5@180 g(8 c c f e) c f g(7 c c f c)) g(4@180 G(9 g(12 G(11@180 e c f e) g(10@180 e c f f) f e) c f e) G(6 e e f e) f \
         f) G(3@180 c c f c))",
    ),
    (
        "rhombictricontahedron",
        None,
        "G(1 g(2 G(6 g(9 G(10 g(11 f e f e) e f e) e f f) f f G(8 f c f e)) g(7 G(12 g(14 f c f e) c f G(13 f c f e)) e f f) f c) G(3 g(14 G(14 g(14 \
         f c f e) c f e) c f e) e f G(14 f c f e)) g(4 G(4 g(4 G(4 f e f e) e f e) e f G(4 f e f e)) g(4 f e f e) f e) G(5 g(5 G(5 g(5 f e f f) e f \
         G(5 f e f c)) e f c) f f G(5 g(5 G(5 f e f f) e f e) e f f)))",
    ),
];

/// The keys (`folding line length`, `face N`, `numbered faces`), the pics and the commands.
pub fn definitions() -> String {
    let mut s = String::from(
        r"\def\tikra@fold@length{2cm}
\tikzset{folding line length/.code={\def\tikra@fold@length{#1}}, every ear/.style={}, every cut/.style={}, every fold/.style={help lines}}
",
    );
    let mut numbered = Vec::new();
    for n in 1..=92 {
        s.push_str(&format!(
            "\\expandafter\\def\\csname tikra@fold@face@{n}\\endcsname{{}}\\tikzset{{face {n}/.code={{\\expandafter\\def\\csname tikra@fold@face@{n}\\endcsname{{#1}}}}}}\n"
        ));
        numbered.push(format!("face {n}={{\\node{{{n}}};}}"));
    }
    s.push_str(&format!("\\tikzset{{numbered faces/.style={{{}}}}}\n", numbered.join(",")));
    for (name, pic, _) in NETS {
        s.push_str(&format!("\\def\\tikzfolding{name}#1[#2]#3;{{\\begingroup\\tikzset{{#2}}\\tikra@folding{{{name}}}\\endgroup}}\n"));
        if let Some(pic) = pic {
            s.push_str(&format!("\\tikzset{{pics/{pic} folding/.style={{code={{\\tikra@folding{{{name}}}}}}}}}\n"));
        }
    }
    s
}

/// What is on an edge of a polygon, or in its middle.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    /// a face's code, turned by some degrees
    Face(usize, f64),
    Fold,
    Cut,
    Ear,
    /// an ear whose tip leans toward the edge's start (`<`) or end (`>`), by its angle
    EarLeft(f64),
    EarRight(f64),
    Polygon(Polygon),
}

#[derive(Debug, Clone, PartialEq)]
struct Polygon {
    kind: char,
    face: Option<Box<Item>>,
    edges: Vec<Item>,
}

/// Reads a net in the notation above.
fn parse(s: &str) -> Option<Polygon> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                word.push('(');
                words.push(std::mem::take(&mut word));
            }
            ')' | ' ' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                if c == ')' {
                    words.push(")".into());
                }
            }
            c => word.push(c),
        }
    }
    let mut it = words.into_iter().peekable();
    // a net is a polygon: `K(`, then its items
    let kind = it.next()?.strip_suffix('(')?.chars().next()?;
    polygon_body(kind, &mut it)
}

type Words = std::iter::Peekable<std::vec::IntoIter<String>>;

/// The items of a polygon of kind `kind`, after its `K(`, up to its `)`.
fn polygon_body(kind: char, it: &mut Words) -> Option<Polygon> {
    let mut items = Vec::new();
    while it.peek().map(String::as_str) != Some(")") {
        items.push(polygon(it)?);
    }
    it.next();
    let mut items = items.into_iter();
    let face = items.next().filter(|f| matches!(f, Item::Face(..))).map(Box::new);
    Some(Polygon { kind, face, edges: items.collect() })
}

fn polygon(it: &mut Words) -> Option<Item> {
    let w = it.next()?;
    if let Some(kind) = w.strip_suffix('(') {
        return polygon_body(kind.chars().next()?, it).map(Item::Polygon);
    }
    Some(match w.as_str() {
        "f" => Item::Fold,
        "c" => Item::Cut,
        "e" => Item::Ear,
        _ if w.starts_with('<') => Item::EarLeft(w[1..].parse().ok()?),
        _ if w.starts_with('>') => Item::EarRight(w[1..].parse().ok()?),
        _ => {
            let (n, turn) = w.split_once('@').unwrap_or((&w, "0"));
            Item::Face(n.parse().ok()?, turn.parse().ok()?)
        }
    })
}

/// A polygon's vertices (counterclockwise from the origin, its first edge along x, length 1), and
/// where its face goes: the center, and the slant of a rhombus.
fn vertices(kind: char) -> (Vec<Point>, Point, f64) {
    let rhombus = |angle: f64, center: (f64, f64), slant: f64| {
        let (s, c) = angle.to_radians().sin_cos();
        (vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(1.0 + c, s), Point::new(c, s)], Point::new(center.0, center.1), slant)
    };
    // the rhombi's faces are where TikZ puts them, a little off their centers
    match kind {
        'R' => return rhombus(70.529, (0.63, 0.4755), 0.34),
        'L' => return rhombus(109.471, (0.33, 0.4755), -0.34),
        'G' => return rhombus(63.435, (0.724, 0.447), 0.447),
        'g' => return rhombus(116.565, (0.276, 0.447), -0.447),
        _ => {}
    }
    let n = [('T', 3), ('S', 4), ('P', 5), ('H', 6), ('O', 8), ('D', 10)].iter().find(|(k, _)| *k == kind).map(|(_, n)| *n).expect("a polygon kind of the nets");
    let mut v = vec![Point::new(0.0, 0.0)];
    for k in 0..n - 1 {
        let (s, c) = (360.0 * k as f64 / n as f64).to_radians().sin_cos();
        let last = v[v.len() - 1];
        v.push(Point::new(last.x + c, last.y + s));
    }
    let center = Point::new(0.5, 0.5 / (std::f64::consts::PI / n as f64).tan());
    (v, center, 0.0)
}

/// The frames of a polygon's edges, clockwise from the one after its top vertex: each at the edge's
/// start, turned along it.
fn edge_frames(v: &[Point], length: f64) -> Vec<Transform> {
    let n = v.len();
    let start = n / 2 + 1;
    (0..n)
        .map(|i| {
            let (a, b) = (v[(start + n - i) % n], v[(start + 2 * n - i - 1) % n]);
            let angle = (b.y - a.y).atan2(b.x - a.x).to_degrees();
            Transform::translation(a.x * length, a.y * length).compose(&Transform::rotation(angle))
        })
        .collect()
}

impl Engine {
    pub fn folding_command(&mut self, _id: u16) -> Result<(), Error> {
        let name = self.argument_string()?;
        // (only the library's own commands and pics name a net, and its nets are well formed)
        let net = NETS.iter().find(|(n, _, _)| *n == name).and_then(|(_, _, net)| parse(net)).ok_or_else(|| Error::Figure(format!("no folding named '{name}'")))?;
        let length = self.tex.expand_to_string(vec![Token::cs("tikra@fold@length")])?;
        let length = self.length(&length)?;
        self.fold_polygon(&net, length)
    }

    fn fold_polygon(&mut self, p: &Polygon, length: f64) -> Result<(), Error> {
        let (v, center, slant) = vertices(p.kind);
        // the face first, in the middle, then the edges
        let face = p.face.iter().map(|f| (Transform::translation(center.x * length, center.y * length).compose(&Transform { c: slant, ..Transform::default() }), &**f));
        for (frame, item) in face.chain(edge_frames(&v, length).into_iter().zip(&p.edges)) {
            self.fold_scope(frame, item, length)?;
        }
        Ok(())
    }

    /// An item in its frame, in a scope.
    fn fold_scope(&mut self, frame: Transform, item: &Item, length: f64) -> Result<(), Error> {
        self.begin_group();
        self.begin_scope();
        self.st.transform = self.st.transform.compose(&frame);
        let r = self.fold_item(item, length);
        self.end_scope();
        self.end_group();
        r
    }

    fn fold_item(&mut self, item: &Item, length: f64) -> Result<(), Error> {
        let l = length / PT;
        let pt = |x: f64, y: f64| format!("({}pt,{}pt)", x, y);
        let (o, end) = (pt(0.0, 0.0), pt(l, 0.0));
        let fold = format!(r"\draw[every fold]{o} -- {end};");
        let code = match item {
            Item::Face(n, turn) => {
                self.st.transform = self.st.transform.compose(&Transform::rotation(*turn));
                return self.run_tokens(vec![Token::cs(&format!("tikra@fold@face@{n}"))]);
            }
            Item::Fold => fold,
            Item::Cut => format!(r"\draw[every cut]{o} -- {end};"),
            Item::Ear | Item::EarLeft(_) | Item::EarRight(_) => {
                // the ear's tip, and the order of its sides
                let (tip, from_end) = match item {
                    Item::EarLeft(a) => {
                        let (s, c) = a.to_radians().sin_cos();
                        (pt(0.66 * l * c, 0.66 * l * s), false)
                    }
                    Item::EarRight(a) => {
                        let (s, c) = a.to_radians().sin_cos();
                        (pt(l - 0.66 * l * c, 0.66 * l * s), true)
                    }
                    _ => (pt(0.5 * l, 0.2 * l), false),
                };
                let cut = if from_end { format!("{end} -- {tip} -- {o}") } else { format!("{o} -- {tip} -- {end}") };
                let area = if from_end { format!("{o} -- {end} -- {tip}") } else { format!("{o} -- {tip} -- {end}") };
                format!(r"\path[every ear]{area} -- {o} -- cycle;{fold}\draw[every cut]{cut};")
            }
            Item::Polygon(p) => return self.fold_polygon(p, length),
        };
        self.run_tokens(tikra_tokens(&code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_net_reads() {
        for (name, _, net) in NETS {
            let p = parse(net).unwrap_or_else(|| panic!("{name}"));
            assert!(p.face.is_some(), "{name}");
        }
        let t = parse("T(1 T(2 e f c) <30 3@90)").unwrap();
        assert_eq!(t.edges[1], Item::EarLeft(30.0));
        assert_eq!(t.edges[2], Item::Face(3, 90.0));
    }

    #[test]
    fn edges_go_clockwise_from_the_top() {
        // a square: top edge to the right first, then down, the bottom back to the origin, then up
        let (v, center, _) = vertices('S');
        let f = edge_frames(&v, 1.0);
        let at = |t: &Transform, x: f64| t.apply(Point::new(x, 0.0));
        assert!((at(&f[0], 0.0).y - 1.0).abs() < 1e-9 && (at(&f[0], 1.0).x - 1.0).abs() < 1e-9);
        assert!((at(&f[2], 0.0).x - 1.0).abs() < 1e-9 && at(&f[2], 1.0).x.abs() < 1e-9);
        assert!((center.x - 0.5).abs() < 1e-9 && (center.y - 0.5).abs() < 1e-9);
    }
}
