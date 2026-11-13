//! The mindmap library (`tikzlibrarymindmap`): concepts are circles filled with the `concept
//! color`, grown as a tree with `grow cyclic`; `mindmap` sets the sizes, distances and fonts of
//! each level (`root concept`, `level 1 concept`…), `small`, `large` and `huge mindmap` other
//! ones; `extra concept`, `concept connection` and `annotation` are styles.
//!
//! A child's edge is a `circle connection bar`: a bar from the parent's circle to the child's with
//! a widening cap at each end, filled with the concept color and not stroked. When the child has a
//! `concept color` of its own, the bar is shaded from the parent's color to the child's between the
//! two circles (`circle connection bar switch color=from (a) to (b)`). A bar on a path that draws
//! or fills (`\draw[circle connection bar] (a) edge (b)`) is painted as that path says. The bar
//! is also a decoration (`decoration=circle connection bar`, with `start radius`, `end radius`,
//! `amplitude`, `angle`).

use crate::canvas::{Fill, Item, Paint, Shading};
use crate::scene::{Point, Segment, Transform};
use crate::tex::{Meaning, Token};
use crate::{Error, PT};

use super::Engine;

/// Ids of the mindmap library's commands (`Meaning::Native`), after the folding library's.
pub const BASE: u16 = 890;

const COMMANDS: &[&str] = &["tikra@circlebar"];

pub fn define(e: &mut Engine) {
    for (i, name) in COMMANDS.iter().enumerate() {
        e.tex.define(name, Meaning::Native(BASE + i as u16));
    }
}

pub fn is_mindmap(id: u16) -> bool {
    (BASE..BASE + COMMANDS.len() as u16).contains(&id)
}

/// The library's styles. `\tikra@conceptcolor` is the concept color; `concept color` also makes
/// the edge to the parent switch from the parent's color to the new one.
pub const DEFINITIONS: &str = r"
\def\tikra@conceptcolor{black}\def\tikra@barfrom{}\def\tikra@barto{}\def\tikra@barconcept{}
\tikzset{
  concept/.style={circle,fill=\tikra@conceptcolor,draw=\tikra@conceptcolor,every concept},
  every concept/.style={},
  concept color/.code={\edef\tikra@temp{\noexpand\tikzset{edge from parent path={(\noexpand\tikzparentnode)
    to[tikra@concept bar,circle connection bar switch color=from (\tikra@conceptcolor) to (#1)] (\noexpand\tikzchildnode)}}}\tikra@temp
    \def\tikra@conceptcolor{#1}},
  tikz@concept@setting/.style={edge from parent path={(\tikzparentnode) to[tikra@concept bar,circle connection bar] (\tikzchildnode)}},
  tikra@concept bar/.code={\def\tikra@barconcept{1}},
  tikz@concept@color@set/.style={},
  extra concept/.style={concept color=black!50,level 2 concept,concept,every extra concept},
  every extra concept/.style={},
  concept connection/.style={line width=1mm,shorten <=2mm,shorten >=2mm,cap=round,draw=black!50},
  circle connection bar/.code={\tikzset{to path={\pgfextra{\tikra@circlebar} (\tikztotarget) \tikztonodes}}\def\tikra@barfrom{}},
  circle connection bar switch color/.code={\tikra@barswitchcolor#1\relax},
  every circle connection bar/.style={},
  mindmap/.style={fill,draw,very thick,outer sep=0pt,inner sep=1pt,
    every child/.append style={style=tikz@concept@setting,style=tikz@concept@color@set},
    root concept, level 1/.append style={level 1 concept}, level 2/.append style={level 2 concept},
    level 3/.append style={level 3 concept}, level 4/.append style={level 4 concept},
    text centered, segment angle=20, style=every mindmap},
  every mindmap/.style={},
  root concept/.style={minimum size=4cm,text width=3.5cm,font=\large},
  level 1 concept/.style={minimum size=2.25cm,level distance=5cm,text width=2cm,sibling angle=60,font=\small},
  level 2 concept/.style={minimum size=1.75cm,level distance=2.9cm,text width=1.5cm,sibling angle=60,font=\footnotesize},
  level 3 concept/.style={minimum size=1.15cm,text width=1cm,level distance=2.4cm,sibling angle=30,font=\tiny},
  level 4 concept/.style={minimum size=0.9cm,text width=0.7cm,level distance=1.85cm,sibling angle=30,font=\tiny},
  small mindmap/.style={
    root concept/.style={minimum size=2.3cm,text width=2.1cm,font=\footnotesize},
    level 1 concept/.style={minimum size=1.5cm,level distance=2.8cm,text width=1.4cm,sibling angle=75,font=\scriptsize},
    level 2 concept/.style={minimum size=1.1cm,level distance=2.2cm,text width=1.1cm,sibling angle=60,font=\tiny},
    level 3 concept/.style={level 2 concept,sibling angle=30,font=\tiny},
    level 4 concept/.style={level 3 concept},
    mindmap, line width=2pt},
  large mindmap/.style={
    root concept/.style={minimum size=5.6cm,text width=4.5cm,font=\Large},
    level 1 concept/.style={minimum size=3.2cm,level distance=7cm,text width=2.8cm,sibling angle=60,font=},
    level 2 concept/.style={minimum size=2.45cm,level distance=4cm,text width=2.2cm,sibling angle=60,font=\small},
    level 3 concept/.style={minimum size=1.63cm,text width=1.4cm,level distance=3.38cm,sibling angle=30,font=\scriptsize},
    level 4 concept/.style={minimum size=1.27cm,text width=1cm,level distance=2.60cm,sibling angle=30,font=\tiny},
    mindmap, line width=2pt},
  huge mindmap/.style={
    root concept/.style={minimum size=8cm,text width=7cm,font=\huge},
    level 1 concept/.style={minimum size=4.5cm,level distance=10cm,text width=4cm,sibling angle=60,font=\large},
    level 2 concept/.style={minimum size=3.5cm,level distance=5.8cm,text width=3cm,sibling angle=60,font=},
    level 3 concept/.style={minimum size=2.3cm,text width=2cm,level distance=4.8cm,sibling angle=30,font=\footnotesize},
    level 4 concept/.style={minimum size=1.7cm,text width=1.4cm,level distance=3.7cm,sibling angle=30,font=\scriptsize},
    mindmap, line width=3pt},
  annotation/.style={shape=rectangle,minimum size=0pt,text width=3.5cm,outer sep=1.5mm,inner sep=1mm,
    text badly ragged,rounded corners,font=\tiny,every annotation},
  every annotation/.style={},
}
\def\tikra@barswitchcolor from (#1) to (#2)\relax{\tikzset{circle connection bar}\def\tikra@barfrom{#1}\def\tikra@barto{#2}}
";

/// A cap of a circle connection bar, in pt, around the circle's center: from the circle's border
/// at ±`angle` it widens and ends in a band of width `amplitude` at 1.5 radius, facing x.
pub fn cap(r: f64, amplitude: f64, angle: f64, t: &Transform) -> Vec<Segment> {
    let polar = |a: f64, d: f64| {
        let (s, c) = a.to_radians().sin_cos();
        Point::new(d * c, d * s)
    };
    let a = amplitude;
    let start = polar(angle, r);
    let mut v = vec![Segment::MoveTo(t.apply(start))];
    v.extend(crate::canvas::transform_segments(&super::path::arc_segments(Point::new(0.0, 0.0), r, r, angle, -angle, &Transform::default()), t));
    let low = polar(-angle, r).plus(polar(90.0 - angle, 0.25 * r));
    v.push(Segment::CurveTo(t.apply(low), t.apply(Point::new(1.25 * r, -0.5 * a)), t.apply(Point::new(1.5 * r, -0.5 * a))));
    v.push(Segment::LineTo(t.apply(Point::new(1.5 * r, 0.5 * a))));
    let high = start.plus(polar(angle - 90.0, 0.25 * r));
    v.push(Segment::CurveTo(t.apply(Point::new(1.25 * r, 0.5 * a)), t.apply(high), t.apply(start)));
    v.push(Segment::Close);
    v
}

/// The whole bar, in pt, along x from the start circle's border (at 0) to the end circle's (at
/// `length`): the start cap, the band between, the end cap, as the decoration's three states draw
/// them.
pub fn bar(r1: f64, r2: f64, amplitude: f64, angle: f64, length: f64, t: &Transform) -> Vec<Segment> {
    let mut v = cap(r1, amplitude, angle, &t.compose(&Transform::translation(-r1, 0.0)));
    let (x0, x1, h) = (0.5 * r1, length - 0.5 * r2, 0.5 * amplitude);
    for (i, (x, y)) in [(x0, -h), (x1, -h), (x1, h), (x0, h)].into_iter().enumerate() {
        let p = t.apply(Point::new(x, y));
        v.push(if i == 0 { Segment::MoveTo(p) } else { Segment::LineTo(p) });
    }
    v.push(Segment::Close);
    let end = t.compose(&Transform::translation(length, 0.0)).compose(&Transform::scaling(-1.0, 1.0)).compose(&Transform::translation(-r2, 0.0));
    v.extend(cap(r2, amplitude, angle, &end));
    v
}

impl Engine {
    /// `\tikra@circlebar`, in a `to path`: the bar from `\tikztostart` to `\tikztotarget` (two
    /// circles), filled with the concept color, or shaded when the colors switch.
    pub fn mindmap_command(&mut self, _id: u16) -> Result<(), Error> {
        let name = |e: &mut Engine, m: &str| e.tex.expand_to_string(vec![Token::cs(m)]).map(|s| s.trim().to_string());
        let (from, to) = (name(self, "tikztostart")?, name(self, "tikztotarget")?);
        let (Some(a), Some(b)) = (self.nodes.get(&from).cloned(), self.nodes.get(&to).cloned()) else {
            return Err(Error::Figure("a circle connection bar connects two nodes".into()));
        };
        if a.shape == "coordinate" || b.shape == "coordinate" {
            return Err(Error::Figure("a circle connection bar cannot connect a node of shape 'coordinate'".into()));
        }
        if self.facts.mute == 0 {
            self.facts.links.push(crate::describe::Link { from: from.clone(), to: to.clone(), arrow: crate::describe::Arrow::None, labels: Vec::new(), source: self.source });
        }
        // the radii as pgf measures them: from the center to the west anchor, on the canvas
        let radius = |n: &super::nodes::NodeInfo| n.center().x - n.anchor("west").unwrap_or(n.center()).x;
        let (ca, cb) = (a.center(), b.center());
        let (r1, r2) = (radius(&a) / PT, radius(&b) / PT);
        let amplitude = 0.175 * r1.min(r2);
        let angle = self.st.decoration.angle;
        let d = cb.minus(ca);
        let dist = (d.x * d.x + d.y * d.y).sqrt() / PT;
        if dist < 1e-9 {
            return Ok(());
        }
        let (ux, uy) = (d.x / PT / dist, d.y / PT / dist);
        // the frame of the decoration: x along the line, from the start circle's border, in pt
        let start = Point::new(ca.x / PT + r1 * ux, ca.y / PT + r1 * uy);
        let frame = Transform { a: ux * PT, b: uy * PT, c: -uy * PT, d: ux * PT, e: start.x * PT, f: start.y * PT };
        let segments = bar(r1, r2, amplitude, angle, dist - r1 - r2, &frame);
        let color = |e: &mut Engine, m: &str| -> Result<Option<crate::color::Color>, Error> {
            let s = e.tex.expand_to_string(vec![Token::cs(m)])?;
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            e.color(s).map(Some).ok_or_else(|| Error::Figure(format!("unknown color '{s}'")))
        };
        let paint = match (color(self, "tikra@barfrom")?, color(self, "tikra@barto")?) {
            (Some(from_color), Some(to_color)) => {
                // from the start circle's border to the end circle's, along y of the shading's box
                let s0 = Point::new(start.x * PT, start.y * PT);
                let e0 = Point::new((ca.x / PT + (dist - r2) * ux) * PT, (ca.y / PT + (dist - r2) * uy) * PT);
                let (vx, vy) = ((e0.x - s0.x) / 100.0, (e0.y - s0.y) / 100.0);
                let mid = s0.lerp(e0, 0.5);
                let transform = Transform { a: vy, b: -vx, c: vx, d: vy, e: mid.x, f: mid.y };
                Paint::Shading(Shading::Axial { transform, stops: vec![(0.0, from_color), (1.0, to_color)] })
            }
            _ => Paint::Solid(color(self, "tikra@conceptcolor")?.unwrap_or(crate::color::Color::BLACK)),
        };
        if self.counts_for_size() {
            self.canvas.include_path(&segments);
        }
        // a child's edge, or a bar on a path that neither fills nor draws, is filled with the
        // concept colors; on a path that does, the bar is filled and drawn as that path is
        // (`\draw[circle connection bar] (a) edge (b)` outlines it)
        let tree = self.tex.expand_to_string(vec![Token::cs("tikra@barconcept")])?.trim() == "1";
        let (fill, stroke) = if tree || !(self.st.mode.fill || self.st.mode.draw) {
            (Some(Fill { paint, opacity: self.st.fill_opacity, even_odd: false }), None)
        } else {
            let fill = self.st.mode.fill.then(|| Fill { paint: Paint::Solid(self.st.fill_paint_color()), opacity: self.st.fill_opacity, even_odd: false });
            (fill, self.st.mode.draw.then(|| self.stroke()))
        };
        self.canvas.push(Item::Path { segments, fill, stroke });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_has_three_pieces() {
        let v = bar(28.0, 14.0, 4.9, 20.0, 30.0, &Transform::default());
        assert_eq!(v.iter().filter(|s| matches!(s, Segment::Close)).count(), 3);
        // the band ends where the caps begin
        assert!(v.iter().any(|s| matches!(s, Segment::LineTo(p) if (p.x - 23.0).abs() < 1e-9)));
    }

    #[test]
    fn a_mindmap_grows() {
        let (canvas, warnings) = super::super::run_document(
            r"\tikz[mindmap,concept color=red!50] \node[concept] {Root} child[grow=right] {node[concept] {Child}} child[grow=left,concept color=blue] {node[concept] {Other}};",
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(canvas.items.iter().any(|i| matches!(i, Item::Path { fill: Some(Fill { paint: Paint::Shading(_), .. }), .. })));
    }

    #[test]
    fn a_bar_of_ones_own_is_painted_as_its_path() {
        let bars = |path: &str| {
            let (canvas, warnings) = super::super::run_document(&format!(
                r"\tikz[mindmap,concept color=red] {{\node[concept] (a) at (0,0) {{}}; \node[concept] (b) at (6,0) {{}}; {path}[circle connection bar] (a) edge (b);}}"
            ))
            .unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            // the two concepts, then the bar
            canvas.items.into_iter().filter(|i| matches!(i, Item::Path { .. })).skip(2).collect::<Vec<_>>()
        };
        // `\draw` outlines it, unfilled; `\fill` fills it in its own color
        assert!(matches!(&bars(r"\draw")[..], [Item::Path { fill: None, stroke: Some(_), .. }]), "{:?}", bars(r"\draw"));
        let blue = crate::color::Color::rgb(0.0, 0.0, 1.0);
        assert!(matches!(&bars(r"\fill[blue]")[..], [Item::Path { fill: Some(Fill { paint: Paint::Solid(c), .. }), .. }] if *c == blue), "{:?}", bars(r"\fill[blue]"));
        // on a path that neither fills nor draws, it is filled with the concept color
        let (canvas, _) =
            super::super::run_document(r"\tikz {\node[circle,draw] (a) at (0,0) {}; \node[circle,draw] (b) at (6,0) {}; \path (a) to[circle connection bar] (b);}").unwrap();
        assert!(matches!(canvas.items.last(), Some(Item::Path { fill: Some(_), stroke: None, .. })), "{:?}", canvas.items.last());
        // a child's edge is still filled with the concept color
        let (canvas, _) = super::super::run_document(r"\tikz[mindmap,concept color=red] \node[concept] {Root} child {node[concept] {Child}};").unwrap();
        let red = crate::color::Color::rgb(1.0, 0.0, 0.0);
        assert!(canvas.items.iter().any(|i| matches!(i, Item::Path { fill: Some(Fill { paint: Paint::Solid(c), .. }), stroke: None, segments } if *c == red && segments.iter().filter(|s| matches!(s, Segment::Close)).count() == 3)));
    }
}
