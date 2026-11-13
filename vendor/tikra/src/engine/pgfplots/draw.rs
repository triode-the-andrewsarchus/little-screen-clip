//! The visualization phase of an axis (`\endpgfplots@environment@opt`): limits
//! (`\pgfplots@determinedefaultvalues`: enlargement, empty ranges), the unit vectors
//! (`\pgfplots@initsizes`: `width`/`height` less 45pt unless `scale only axis`), the ticks, then
//! drawing in pgfplots' order: grid, ticks, axis lines, tick labels, the plots clipped to the axis
//! (with the TikZ statements among them), the marks, the labels, the title and the legend.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::canvas::{Fill, Item, Paint};
use crate::keys as pk;
use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Token};
use crate::{Error, PT};

use super::super::coords::Coord as TikzCoord;
use super::super::nodes::{NodeInfo, NodeSpec, OnPath};
use super::super::shapes::{Border, Geometry};
use super::keys;
use super::number::NumberFormat;
use super::plot::Plot;
use super::ticks::{self, TickSettings};
use super::{toks, Axis, Engine, Entry};

/// Which coordinate system a plain `(x,y)` means while the axis is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cs {
    Plain,
    Axis,
    Description,
}

/// The axis as the coordinate systems see it: where its box is and how data maps to the canvas.
#[derive(Debug)]
pub struct Frame {
    /// Canvas point of the logical lower limits.
    pub origin: Point,
    /// Canvas cm per logical unit, for x, y and z.
    pub e: [Point; 3],
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// Logarithmic axes keep natural logarithms.
    pub log: [bool; 3],
    pub three_d: bool,
    pub zero: [f64; 3],
    pub cs: Cell<Cs>,
    /// The axis whose label is being placed (`ticklabel cs`, `near ticklabel`).
    pub label_axis: Cell<usize>,
    /// The largest distance from a tick to its label, outwards (cm).
    pub tick_dim: RefCell<[f64; 3]>,
    /// Where tick labels are: -1 lower, 1 upper, 0 at the other axis' zero.
    pub label_side: [i8; 3],
    /// `xlabel shift`
    pub label_shift: [f64; 3],
    /// The range of point meta (axis wide) and the colormap it maps to.
    pub meta: [f64; 2],
    pub colormap: Option<super::colormap::Colormap>,
    /// 3d: the box's foreground faces (`[axis][lower, upper]`), the lines with the tick labels
    /// (pgfplots' three-character strings like `v00`) and those lines for a boxed axis.
    pub fg: [[bool; 2]; 3],
    pub label_line: [super::cube::Part; 3],
    pub box_line: [super::cube::Part; 3],
    /// The data's range (merged with given limits), logical.
    pub data_range: [[f64; 2]; 3],
    /// `polaraxis`: x is an angle in degrees, y a radius; `e[0]`, `e[1]` are the cartesian
    /// unit vectors and `origin` the center.
    pub polar: bool,
    /// `ternaryaxis`: points are placed by `ternary::at`; `e` are the triangle's edges per
    /// logical unit. Tick labels are percentages of the ranges with relative limits.
    pub ternary: bool,
    pub ternary_relative: bool,
    /// `smithchart`: points at their reflection coefficient, `origin` the center and `e` the
    /// radii (x negative for `smithchart mirrored`).
    pub smith: bool,
    /// While a tick label is placed: the direction `sloped like y axis` follows (a smith chart's
    /// circle there), and the anchor code it keeps for an upside down label.
    pub slope: Cell<Option<Point>>,
    pub upside_down: RefCell<Option<Vec<Token>>>,
    /// `x dir=reverse` and its friends.
    pub reversed: [bool; 3],
}

impl Frame {
    pub fn logical(&self, a: usize, v: f64) -> f64 {
        if self.log[a] {
            v.ln()
        } else {
            v
        }
    }

    /// The data coordinates of a canvas point of a 2d axis (`\pgfplotspointgetcoordinates`).
    pub fn data_at(&self, p: Point) -> Option<[f64; 2]> {
        if self.three_d || self.polar || self.ternary || self.smith {
            return None;
        }
        let (a, b) = (self.e[0], self.e[1]);
        let det = a.x * b.y - a.y * b.x;
        if det.abs() < 1e-12 {
            return None;
        }
        let d = p.minus(self.origin);
        let (u, v) = ((d.x * b.y - d.y * b.x) / det, (a.x * d.y - a.y * d.x) / det);
        let l = [u + self.min[0], v + self.min[1]];
        Some([0, 1].map(|k| if self.log[k] { l[k].exp() } else { l[k] }))
    }

    /// The canvas point of logical coordinates.
    pub fn at(&self, l: [f64; 3]) -> Point {
        if self.ternary {
            return super::ternary::at(self, l);
        }
        if self.smith {
            return super::smith::at(self, l);
        }
        if self.polar {
            let (s, c) = super::polar::sin_cos(l[0]);
            return self.origin.plus(self.e[0].scale(l[1] * c)).plus(self.e[1].scale(l[1] * s));
        }
        let mut p = self.origin;
        for (i, &v) in l.iter().enumerate() {
            if i == 2 && !self.three_d {
                continue;
            }
            p = p.plus(self.e[i].scale(v - self.min[i]));
        }
        p
    }

    pub fn data(&self, x: f64, y: f64, z: f64) -> Point {
        self.at([self.logical(0, x), self.logical(1, y), if self.three_d { self.logical(2, z) } else { self.min[2] }])
    }

    /// `rel axis cs`: fractions of the ranges.
    pub fn rel(&self, r: [f64; 3]) -> Point {
        let l = [0, 1, 2].map(|i| self.min[i] + r[i] * (self.max[i] - self.min[i]));
        self.at(l)
    }

    /// The lower left and upper right corners of the box as drawn (a 3d box's projection).
    pub fn bbox(&self) -> (Point, Point) {
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        if self.smith {
            return super::smith::bbox(self);
        }
        if self.ternary {
            for p in super::ternary::corners(self) {
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            return (lo, hi);
        }
        if self.polar {
            for p in super::polar::outline(self) {
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            return (lo, hi);
        }
        for i in 0..8 {
            if !self.three_d && i >= 4 {
                break;
            }
            let p = self.rel([(i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64]);
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        (lo, hi)
    }

    /// `axis description cs`: (0,0) the lower left corner of the box as drawn, (1,1) the upper right
    /// one (`\pgfplotspointdescriptionxy`). pgfplots takes the corners of the bounding box of the
    /// axis, so with skewed unit vectors (`x={(5pt,1pt)}`) these are not the axis' corners.
    pub fn desc(&self, x: f64, y: f64) -> Point {
        let (lo, hi) = self.bbox();
        Point::new(lo.x + x * (hi.x - lo.x), lo.y + y * (hi.y - lo.y))
    }

    /// The outward unit normal of the line that holds axis `a`'s tick labels.
    pub fn normal(&self, a: usize) -> Point {
        if self.ternary {
            return super::ternary::normal(self, a);
        }
        if self.smith {
            // at the tick being labelled: outwards of the circle
            if let (1, Some(u)) = (a, self.slope.get()) {
                return Point::new(u.y, -u.x);
            }
            return super::smith::normal(self, a);
        }
        if self.three_d {
            return super::cube::outer_normal(&self.e, self.label_line[a], self.box_line[a]);
        }
        let b = if a == 0 { 1 } else { 0 };
        let d = self.e[b];
        let u = unit(d);
        let side = if self.label_side[a] > 0 { 1.0 } else { -1.0 };
        // e[b] points to growing b: the lower side is outward against it
        let sign = if self.max[b] >= self.min[b] { 1.0 } else { -1.0 };
        u.scale(side * sign)
    }

    /// The direction axis `a` grows in on paper (at the tick being labelled, for a smith chart's
    /// circle).
    fn direction(&self, a: usize) -> Point {
        if self.smith && a == 1 {
            return self.slope.get().unwrap_or(Point::new(0.0, -1.0));
        }
        unit(self.e[a])
    }

    /// pgfplots' `near ticklabel` anchors (`\pgfplots@borderanchor@for@axis@`): in the node's own
    /// (possibly rotated) coordinates, the compass anchor that faces the axis is snapped to, then
    /// moved along the line through it parallel to the axis to where the line from the node's
    /// `near ticklabel align` anchor (center, by default) towards the axis meets it.
    /// `local` gives the node's named anchors.
    pub fn near_point(&self, name: &str, rotation: &Transform, align: &str, local: &dyn Fn(&str) -> Option<Point>) -> Option<Point> {
        let rest = name.strip_prefix("near ")?;
        let (axis_part, opposite) = match rest.strip_suffix(" opposite") {
            Some(r) => (r, true),
            None => (rest, false),
        };
        let a = match axis_part {
            "ticklabel" => self.label_axis.get(),
            "xticklabel" => 0,
            "yticklabel" => 1,
            "zticklabel" => 2,
            _ => return None,
        };
        let lin = Transform { e: 0.0, f: 0.0, ..*rotation }.inverse();
        // towards the axis (away from it for `opposite`), as unit vectors: a scaled node turns them
        // only
        let n = unit(lin.apply(self.normal(a).scale(if opposite { 1.0 } else { -1.0 })));
        let d = unit(lin.apply(self.direction(a)));
        let snap = |v: Point| -> &'static str {
            const T: f64 = 0.17;
            match (v.y > T, v.y < -T, v.x > T, v.x < -T) {
                (true, _, true, _) => "north east",
                (true, _, _, true) => "north west",
                (true, ..) => "north",
                (_, true, true, _) => "south east",
                (_, true, _, true) => "south west",
                (_, true, ..) => "south",
                (.., true, _) => "east",
                (.., true) => "west",
                _ => "center",
            }
        };
        let reversed = self.reversed[a];
        let inner = match align.trim() {
            "inside" => snap(if reversed { d.scale(-1.0) } else { d }),
            "outside" => snap(if reversed { d } else { d.scale(-1.0) }),
            _ => "center",
        };
        let at = local(snap(n))?;
        let from = local(inner)?;
        let det = n.x * d.y - n.y * d.x;
        if (n.x * d.x + n.y * d.y).abs() >= 0.8 || det.abs() < 1e-12 {
            return Some(at);
        }
        // from + r·n = at + s·d
        let rhs = at.minus(from);
        let r = (rhs.x * d.y - rhs.y * d.x) / det;
        Some(from.plus(n.scale(r)))
    }
}

/// A plot's logical coordinates (after stacking) and its stack's base; None for TikZ statements.
pub type PlotPoints = Option<(Vec<[f64; 3]>, Vec<f64>)>;

/// Items and the bounding box they grew.
type Captured<T> = (T, Vec<Item>, Option<(Point, Point)>);

/// What an axis knows once its limits are final.
pub(super) struct Setup {
    pub frame: Rc<Frame>,
    /// per plot: logical coordinates (after stacking) and the stack's base
    pub points: Vec<PlotPoints>,
    pub ticks: [Vec<f64>; 3],
    pub tick_distance: [f64; 3],
    pub hide: [bool; 3],
    pub line: [String; 3],
}

pub(super) fn axis_char(a: usize) -> char {
    ['x', 'y', 'z'][a]
}

/// `enlarge x limits=…`: (lower, upper, relative amount, absolute amount).
fn enlarge_setting(v: &str, auto_min: bool, auto_max: bool) -> (bool, bool, f64, Option<f64>) {
    let v = v.trim();
    let mut rel = 0.1;
    let mut abs = None;
    let (mut lo, mut hi) = (false, false);
    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part {
            "auto" => {
                lo = auto_min;
                hi = auto_max;
            }
            "true" => {
                lo = true;
                hi = true;
            }
            "false" => {
                lo = false;
                hi = false;
            }
            "upper" => {
                lo = false;
                hi = true;
            }
            "lower" => {
                lo = true;
                hi = false;
            }
            other => {
                let (k, val) = other.split_once('=').map(|(a, b)| (a.trim(), b.trim())).unwrap_or(("value", other));
                match k {
                    "abs" | "abs value" => {
                        abs = val.parse().ok();
                        if k == "abs" {
                            lo = true;
                            hi = true;
                        }
                    }
                    "rel" | "value" => {
                        rel = val.parse().unwrap_or(0.1);
                        if k == "rel" || !v.contains("upper") && !v.contains("lower") {
                            lo = true;
                            hi = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (lo, hi, rel, abs)
}

impl Engine {
    /// A value of the axis (`/pgfplots/<key>`), expanded.
    pub(super) fn pv(&mut self, key: &str) -> String {
        keys::value(self, &format!("/pgfplots/{key}"))
    }

    /// Items pushed and the bounding box grown while `f` runs, taken out of the canvas.
    fn capture<T>(&mut self, f: impl FnOnce(&mut Engine) -> Result<T, Error>) -> Result<Captured<T>, Error> {
        let start = self.canvas.items.len();
        let (min, max) = (self.canvas.min.take(), self.canvas.max.take());
        let r = f(self);
        let bbox = self.canvas.min.zip(self.canvas.max);
        self.canvas.min = min;
        self.canvas.max = max;
        let items: Vec<Item> = self.canvas.items.drain(start..).collect();
        Ok((r?, items, bbox))
    }

    /// Runs a key list on a path or node, with `/pgfplots` styles reachable.
    pub(super) fn keys_tikz(&mut self, list: &str) -> Result<(), Error> {
        self.process_keys(toks(list), Some("/tikz"))
    }

    /// Draws a path with the options `list` (as `\draw[list] …;`).
    pub(super) fn draw_with(&mut self, list: &str, segs: Vec<Segment>) -> Result<(), Error> {
        if segs.is_empty() {
            return Ok(());
        }
        self.begin_group();
        self.st.mode = Default::default();
        self.st.pending = Transform::default();
        let r = (|| {
            self.st.mode.draw = true;
            self.keys_tikz(list)?;
            self.st.apply_pending();
            self.use_path(segs, "axis")
        })();
        self.end_group();
        r
    }
}

/// The axis' `at` (in the picture), and whether its own `xshift` moves it: not when a colorbar is
/// placed at plain description coordinates of its parent (as pgfplots' `colorbar top`
/// customizations show).
fn axis_at(e: &mut Engine) -> Result<(Point, bool), Error> {
    let mut shifts = true;
    let at = match keys::value_tokens(e, "/pgfplots/at") {
        Some(v) => {
            let s = e.tex.expand_to_string(v)?;
            let inner = s.trim().trim_start_matches('{').trim_end_matches('}').trim().trim_start_matches('(').trim_end_matches(')').to_string();
            // a colorbar's `at` is in its parent's description coordinates
            let parent = super::colorbar::parent();
            if parent.is_some() && inner.chars().all(|c| c.is_ascii_digit() || ".,-+ ".contains(c)) {
                shifts = false;
            }
            if let Some(p) = &parent {
                p.cs.set(Cs::Description);
                e.frame = Some(p.clone());
            }
            let r = e.coordinate(&inner);
            if parent.is_some() {
                e.frame = None;
            }
            match r {
                Ok(c) => c.p,
                Err(err) => {
                    e.warn(format!("axis at={s}: {err}"));
                    e.st.transform.apply(Point::default())
                }
            }
        }
        None => e.st.transform.apply(Point::default()),
    };
    Ok((at, shifts))
}

/// Draws the axis collected by the survey; returns its colorbar, drawn once its group is closed.
/// A picture scaled or rotated (`\begin{tikzpicture}[scale=0.75]`) has the whole axis drawn so, its
/// text and lines too, as pgfplots does: it is drawn unscaled from the picture's origin, then
/// transformed around it.
pub fn draw_axis(e: &mut Engine, axis: &Axis) -> Result<Option<super::colorbar::Colorbar>, Error> {
    let saved = e.st.transform;
    let linear = Transform { e: 0.0, f: 0.0, ..saved };
    let identity = Transform::default();
    if [linear.a - identity.a, linear.b, linear.c, linear.d - identity.d].iter().all(|v| v.abs() < 1e-9) {
        return draw_axis_unscaled(e, axis);
    }
    let origin = saved.apply(Point::default());
    let m = Transform::translation(origin.x, origin.y).compose(&linear).compose(&Transform::translation(-origin.x, -origin.y));
    e.st.transform = Transform::translation(origin.x, origin.y);
    let (items_before, box_before) = (e.canvas.items.len(), (e.canvas.min.take(), e.canvas.max.take()));
    let nodes_before: std::collections::HashSet<String> = e.nodes.keys().cloned().collect();
    let r = draw_axis_unscaled(e, axis);
    e.st.transform = saved;
    e.canvas.items[items_before..].iter_mut().for_each(|it| it.transform(&m));
    for (name, n) in e.nodes.iter_mut() {
        if !nodes_before.contains(name) {
            n.transform = m.compose(&n.transform);
        }
    }
    let axis_box = e.canvas.min.zip(e.canvas.max);
    (e.canvas.min, e.canvas.max) = box_before;
    if let Some((lo, hi)) = axis_box {
        for corner in [lo, Point::new(hi.x, lo.y), hi, Point::new(lo.x, hi.y)] {
            e.canvas.include(m.apply(corner));
        }
    }
    r
}

fn draw_axis_unscaled(e: &mut Engine, axis: &Axis) -> Result<Option<super::colorbar::Colorbar>, Error> {
    let mut setup = setup(e, axis)?;
    let anchor = keys::value(e, "/pgfplots/anchor");
    // an anchor of the box alone (`origin`, `south west`…) places the axis before it is drawn, as
    // pgfplots does: what it draws at named coordinates of the picture stays where they are
    let early = {
        let (lo, hi) = setup.frame.bbox();
        let a = anchor.trim();
        let box_only =
            !a.starts_with("outer") && !["above", "below", "left of", "right of"].iter().any(|w| a.starts_with(w)) && named_anchor(&anchor, lo, hi, lo, hi, &setup.frame).is_some();
        if box_only && super::colorbar::parent().is_none() {
            let anchor_point = named_anchor(&anchor, lo, hi, lo, hi, &setup.frame).unwrap_or(lo);
            let (at, shifts) = axis_at(e)?;
            let p = e.st.pending;
            Some(at.minus(anchor_point).plus(if shifts { Point::new(p.e, p.f) } else { Point::default() }))
        } else {
            None
        }
    };
    if let Some(d) = early {
        setup.frame = Rc::new(Frame { origin: setup.frame.origin.plus(d), ..clone_frame(&setup.frame) });
    }
    let frame = setup.frame.clone();
    e.frame = Some(frame.clone());
    // the canvas items and bounding box of the axis alone, then moved by `at` and `anchor`
    let nodes_before: std::collections::HashSet<String> = e.nodes.keys().cloned().collect();
    if e.facts.mute == 0 && super::colorbar::parent().is_none() {
        note_axis(e, axis, &frame);
    }
    // its ticks, labels and legend are not the figure's text
    e.facts.mute += 1;
    let r = e.capture(|e| draw_all(e, axis, &setup));
    e.facts.mute -= 1;
    let ((), items, bbox) = match r {
        Ok(v) => v,
        Err(err) => {
            e.frame = None;
            return Err(err);
        }
    };
    // where the axis goes: `at` (in the picture), by `anchor`
    let (lo, hi) = frame.bbox();
    let (blo, bhi) = bbox.unwrap_or((lo, hi));
    // `anchor=mynode.center`: a node drawn in the axis (`title style={name=…}`)
    let node_anchor = || {
        let (n, a) = anchor.trim().rsplit_once('.')?;
        e.nodes.get(n.trim()).filter(|_| !nodes_before.contains(n.trim())).and_then(|info| info.anchor(a.trim()))
    };
    let anchor_point = named_anchor(&anchor, lo, hi, blo, bhi, &frame).or_else(node_anchor).unwrap_or(lo);
    e.frame = None;
    let d = match early {
        Some(_) => Point::default(),
        None => {
            let (at, shifts) = axis_at(e)?;
            // `xshift`, `yshift` and `shift` in the axis options move it (`colorbar shift`)
            let p = e.st.pending;
            at.minus(anchor_point).plus(if shifts { Point::new(p.e, p.f) } else { Point::default() })
        }
    };
    for mut it in items {
        it.translate(d);
        e.canvas.items.push(it);
    }
    if !e.canvas_frozen {
        e.canvas.include(blo.plus(d));
        e.canvas.include(bhi.plus(d));
    }
    // nodes made inside the axis move with it
    for (name, n) in e.nodes.iter_mut() {
        if !nodes_before.contains(name) {
            n.transform = Transform::translation(d.x, d.y).compose(&n.transform);
        }
    }
    // the axis as a node: `current axis` and its name, with pgfplots' extra anchors
    let name = keys::value(e, "/pgfplots/name");
    let (lo, hi, blo, bhi) = (lo.plus(d), hi.plus(d), blo.plus(d), bhi.plus(d));
    let shifted = Frame { origin: frame.origin.plus(d), ..clone_frame(&frame) };
    let colorbar = super::colorbar::snapshot(e, frame.meta, (hi.x - lo.x, hi.y - lo.y), clone_frame(&shifted)).map(|mut cb| {
        // a filled contour's colorbar shows its bands
        cb.bands = axis.entries.iter().find_map(|en| match en {
            Entry::Plot(p) if p.handler == "contour filled" => {
                crate::keys::split_list(&p.style).into_iter().find(|i| i.key.trim_start_matches("/pgfplots/") == "contour filled").map(|i| i.value.unwrap_or_default())
            }
            _ => None,
        });
        cb
    });
    let parent = if colorbar.is_some() { "parent axis".to_string() } else { String::new() };
    for n in ["current axis".to_string(), name, parent].into_iter().filter(|n| !n.is_empty()) {
        register_axis_node(e, &n, lo, hi, blo, bhi, &shifted);
    }
    Ok(colorbar)
}

/// The axis for the figure's description: its kind, title, labels, ranges and plots.
fn note_axis(e: &mut Engine, axis: &Axis, f: &Frame) {
    use crate::describe::{AxisFact, Series};
    let mut names = super::legend::entries(e, axis).into_iter().map(|l| tex::tokens_to_string(&l.text));
    let mut series = Vec::new();
    let mut colored = false;
    let marks_only = tex::tokens_to_string(&axis.options).contains("only marks");
    for en in &axis.entries {
        let Entry::Plot(p) = en else { continue };
        let name = if p.forget { String::new() } else { names.next().unwrap_or_default() };
        colored |= p.mesh.is_some() || p.patch || p.scatter || p.handler.starts_with("contour");
        let points = p.coords.iter().map(|c| [c.x, c.y, c.z]).collect();
        let boxplot = p.boxplot.as_ref().map(|b| [b.lower_whisker, b.lower_quartile, b.median, b.upper_quartile, b.upper_whisker]);
        series.push(Series { source: p.source, kind: plot_kind(p, marks_only), name, expression: p.expression.clone(), points, boxplot });
    }
    let kind = if f.polar {
        "polar"
    } else if f.ternary {
        "ternary"
    } else if f.smith {
        "smith"
    } else {
        ""
    };
    // bars stand on zero: their value axis shows it
    let base = |k: usize| series.iter().any(|s| s.kind == ["horizontal bar", "bar"][k.min(1)]) && k < 2;
    let range = |k: usize| {
        let [mut lo, mut hi] = f.data_range[k];
        if base(k) && !f.log[k] {
            (lo, hi) = (lo.min(0.0), hi.max(0.0));
        }
        let (lo, hi) = if f.log[k] { (lo.exp(), hi.exp()) } else { (lo, hi) };
        (lo.is_finite() && hi.is_finite() && lo <= hi).then_some([lo, hi])
    };
    let fact = AxisFact {
        source: axis.source,
        kind,
        three_d: f.three_d,
        title: keys::value(e, "/pgfplots/title"),
        labels: [keys::value(e, "/pgfplots/xlabel"), keys::value(e, "/pgfplots/ylabel"), keys::value(e, "/pgfplots/zlabel")],
        range: [range(0), range(1), range(2)],
        log: f.log,
        symbolic: [super::symbolic::names(e, 'x'), super::symbolic::names(e, 'y'), super::symbolic::names(e, 'z')],
        series,
        colors: (colored && f.colormap.is_some() && f.meta.iter().all(|v| v.is_finite())).then_some(f.meta),
    };
    e.facts.axes.push(fact);
}

/// What kind of plot it is, in words.
fn plot_kind(p: &Plot, marks_only: bool) -> &'static str {
    let style = tex::tokens_to_string(&p.style);
    let h = p.handler.as_str();
    if p.boxplot.is_some() || h == "boxplot" {
        "box plot"
    } else if h.starts_with("contour") {
        "contour"
    } else if h == "quiver" {
        "vector field"
    } else if h.ends_with("interval") {
        "histogram"
    } else if h.starts_with("ybar") {
        "bar"
    } else if h.starts_with("xbar") {
        "horizontal bar"
    } else if h.contains("comb") {
        "stem"
    } else if p.mesh == Some('s') || p.patch {
        "surface"
    } else if p.mesh == Some('m') {
        "mesh"
    } else if p.scatter || marks_only || style.contains("only marks") {
        "scatter"
    } else if p.fill_between.is_some() || p.post.iter().any(|t| t.is_cs("closedcycle")) {
        "area"
    } else {
        "line"
    }
}

fn clone_frame(f: &Frame) -> Frame {
    Frame {
        origin: f.origin,
        e: f.e,
        min: f.min,
        max: f.max,
        log: f.log,
        three_d: f.three_d,
        zero: f.zero,
        cs: Cell::new(f.cs.get()),
        label_axis: Cell::new(f.label_axis.get()),
        tick_dim: RefCell::new(*f.tick_dim.borrow()),
        label_side: f.label_side,
        fg: f.fg,
        label_line: f.label_line,
        box_line: f.box_line,
        data_range: f.data_range,
        polar: f.polar,
        ternary: f.ternary,
        ternary_relative: f.ternary_relative,
        smith: f.smith,
        slope: Cell::new(f.slope.get()),
        upside_down: RefCell::new(f.upside_down.borrow().clone()),
        reversed: f.reversed,
        label_shift: f.label_shift,
        meta: f.meta,
        colormap: f.colormap.clone(),
    }
}

/// pgfplots' anchors of an axis: the box's compass anchors, `outer …` ones of the bounding box,
/// `origin`, `left of origin`… and `right of north east`…
fn named_anchor(name: &str, lo: Point, hi: Point, blo: Point, bhi: Point, f: &Frame) -> Option<Point> {
    let compass = |a: &str, lo: Point, hi: Point| -> Option<Point> {
        let (cx, cy) = ((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0);
        Some(match a {
            "south west" => lo,
            "south east" => Point::new(hi.x, lo.y),
            "north west" => Point::new(lo.x, hi.y),
            "north east" => hi,
            "south" => Point::new(cx, lo.y),
            "north" => Point::new(cx, hi.y),
            "west" => Point::new(lo.x, cy),
            "east" => Point::new(hi.x, cy),
            "center" => Point::new(cx, cy),
            _ => return None,
        })
    };
    let n = name.trim();
    if let Some(rest) = n.strip_prefix("outer ") {
        return compass(rest, blo, bhi);
    }
    if let Some(p) = compass(n, lo, hi) {
        return Some(p);
    }
    let origin = f.at([f.zero[0], f.zero[1], f.zero[2]]);
    // where the line through the origin along an axis meets an edge of the box (a 3d axis'
    // lines are slanted)
    let at_y = |dir: Point, y: f64| if dir.y.abs() > 1e-12 { origin.plus(dir.scale((y - origin.y) / dir.y)) } else { Point::new(origin.x, y) };
    let at_x = |dir: Point, x: f64| if dir.x.abs() > 1e-12 { origin.plus(dir.scale((x - origin.x) / dir.x)) } else { Point::new(x, origin.y) };
    Some(match n {
        "origin" => origin,
        "left of origin" => at_x(f.e[0], lo.x),
        "right of origin" => at_x(f.e[0], hi.x),
        "above origin" => at_y(f.e[1], hi.y),
        "below origin" => at_y(f.e[1], lo.y),
        "right of north east" => Point::new(bhi.x, hi.y),
        "right of north west" => Point::new(blo.x.max(lo.x), hi.y),
        "right of south east" => Point::new(bhi.x, lo.y),
        "left of north west" => Point::new(blo.x, hi.y),
        "left of south west" => Point::new(blo.x, lo.y),
        "left of north east" => Point::new(lo.x, hi.y),
        "above north west" => Point::new(lo.x, bhi.y),
        "above north east" => Point::new(hi.x, bhi.y),
        "above south west" => Point::new(lo.x, bhi.y),
        "below south west" => Point::new(lo.x, blo.y),
        "below south east" => Point::new(hi.x, blo.y),
        "below north west" => Point::new(lo.x, blo.y),
        _ => return None,
    })
}

fn register_axis_node(e: &mut Engine, name: &str, lo: Point, hi: Point, blo: Point, bhi: Point, f: &Frame) {
    let c = Point::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0);
    let (hx, hy) = ((hi.x - lo.x) / 2.0, (hi.y - lo.y) / 2.0);
    let mut g = Geometry::new(Point::default(), Border::Rectangle(Point::new(hx, hy)));
    for (a, x, y) in [
        ("center", 0.0, 0.0),
        ("north", 0.0, hy),
        ("south", 0.0, -hy),
        ("east", hx, 0.0),
        ("west", -hx, 0.0),
        ("north east", hx, hy),
        ("north west", -hx, hy),
        ("south east", hx, -hy),
        ("south west", -hx, -hy),
        ("mid", 0.0, 0.0),
        ("mid east", hx, 0.0),
        ("mid west", -hx, 0.0),
        ("base", 0.0, 0.0),
        ("base east", hx, 0.0),
        ("base west", -hx, 0.0),
    ] {
        g.anchor_at(a, Point::new(x, y));
    }
    let info = NodeInfo { shape: "rectangle".into(), geometry: Rc::new(g), transform: Transform::translation(c.x, c.y) };
    e.nodes.insert(name.to_string(), info);
    for a in [
        "origin",
        "left of origin",
        "right of origin",
        "above origin",
        "below origin",
        "outer north east",
        "outer north west",
        "outer south east",
        "outer south west",
        "outer north",
        "outer south",
        "outer east",
        "outer west",
        "outer center",
        "right of north east",
        "right of south east",
        "left of north west",
        "left of south west",
        "above north west",
        "above north east",
        "below south west",
        "below south east",
    ] {
        if let Some(p) = named_anchor(a, lo, hi, blo, bhi, f) {
            let mut g = Geometry::new(Point::default(), Border::Point);
            g.anchor_at("center", Point::default());
            let info = NodeInfo { shape: "coordinate".into(), geometry: Rc::new(g), transform: Transform::translation(p.x, p.y) };
            e.nodes.insert(format!("{name}@{a}"), info);
        }
    }
}

/// Limits, scaling and ticks.
fn setup(e: &mut Engine, axis: &Axis) -> Result<Setup, Error> {
    let ternary = axis.kind == "ternaryaxis";
    let smith = axis.kind == "smithchart";
    let three_d = ternary || axis.entries.iter().any(|en| matches!(en, Entry::Plot(p) if p.three_d)) || !e.pv("zmin").is_empty() || !e.pv("zmax").is_empty();
    if three_d {
        three_d_descriptions(e, axis)?;
    }
    // `late options` of the plots (`matrix plot`: `y dir=reverse, axis on top`)
    for en in &axis.entries {
        if let Entry::Plot(p) = en {
            if !p.late.is_empty() {
                let saved = e.st.key_path.clone();
                e.process_keys(p.late.clone(), Some("/pgfplots"))?;
                e.st.key_path = saved;
            }
        }
    }
    let log = [e.pv("xmode") == "log", e.pv("ymode") == "log", e.pv("zmode") == "log"];
    let lg = |a: usize, v: f64| {
        if log[a] {
            if v > 0.0 {
                v.ln()
            } else {
                f64::NAN
            }
        } else {
            v
        }
    };
    let mut user: [(Option<f64>, Option<f64>); 3] = [(None, None); 3];
    for (a, u) in user.iter_mut().enumerate() {
        let c = axis_char(a);
        *u = (super::symbolic::number(e, c, &format!("/pgfplots/{c}min")).map(|v| lg(a, v)), super::symbolic::number(e, c, &format!("/pgfplots/{c}max")).map(|v| lg(a, v)));
    }
    // the coordinates, stacked
    let mut points: Vec<PlotPoints> = Vec::new();
    let mut stack_base: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    // `stack negative=separate`: the negative values stack on their own, from zero down
    let separate = e.pv("stack negative") == "separate";
    let mut stack_base_neg: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut dmin = [f64::INFINITY; 3];
    let mut dmax = [f64::NEG_INFINITY; 3];
    let mut boxplots = 0usize;
    super::boxplot::variable_widths(&axis.entries.iter().filter_map(|en| if let Entry::Plot(p) = en { p.boxplot.as_deref() } else { None }).collect::<Vec<_>>());
    for en in &axis.entries {
        let Entry::Plot(p) = en else {
            points.push(None);
            continue;
        };
        let mut v: Vec<[f64; 3]> = p.coords.iter().map(|c| [lg(0, c.x), lg(1, c.y), lg(2, c.z)]).collect();
        let mut base = vec![0.0; v.len()];
        // quiver: the arrows' ends are in view too (`quiver/update limits`)
        if p.handler == "quiver" && keys::flag(e, "/pgfplots/quiver/update limits") {
            let ends: Vec<[f64; 3]> = p.coords.iter().map(|c| [lg(0, c.x + c.uvw[0]), lg(1, c.y + c.uvw[1]), lg(2, c.z + c.uvw[2])]).collect();
            for c in ends {
                for i in 0..if three_d { 3 } else { 2 } {
                    if c[i].is_finite() {
                        dmin[i] = dmin[i].min(c[i]);
                        dmax[i] = dmax[i].max(c[i]);
                    }
                }
            }
        }
        // error bars: their ends are in view (`\pgfplots@PREPARE@errorbar@stream@it@`)
        if !three_d && p.boxplot.is_none() {
            for (k, ends) in super::visual::error_ends(e, p)?.into_iter().enumerate() {
                let Some(c) = v.get(k).copied() else { break };
                for (j, end) in ends.into_iter().enumerate() {
                    let a = j / 2;
                    let end = lg(a, end);
                    if end.is_finite() && c[1 - a].is_finite() {
                        dmin[a] = dmin[a].min(end);
                        dmax[a] = dmax[a].max(end);
                    }
                }
            }
        }
        // a box plot: its outliers at its position, and its box and whiskers in view
        if p.boxplot.is_some() {
            let index = boxplots;
            boxplots += 1;
            for c in super::boxplot::extent(e, p, index, &mut v) {
                for i in 0..2 {
                    if c[i].is_finite() {
                        dmin[i] = dmin[i].min(c[i]);
                        dmax[i] = dmax[i].max(c[i]);
                    }
                }
            }
        }
        if let Some(dir) = p.stack {
            let a = if dir == 'x' { 0 } else { 1 };
            for (i, c) in v.iter_mut().enumerate() {
                let val = if p.stack_minus { -c[a] } else { c[a] };
                for list in [&mut stack_base[a], &mut stack_base_neg[a]] {
                    if list.len() <= i {
                        list.resize(i + 1, 0.0);
                    }
                }
                let b = if separate && val < 0.0 { &mut stack_base_neg[a] } else { &mut stack_base[a] };
                // (both lists reach i: resized above)
                let prev = b[i];
                base[i] = prev;
                c[a] = prev + if val.is_finite() { val } else { 0.0 };
                b[i] = c[a];
            }
        }
        for c in &v {
            if !(c[0].is_finite() && c[1].is_finite()) {
                continue;
            }
            for i in 0..3 {
                if i == 2 && !three_d {
                    continue;
                }
                // `clip limits`: a point beyond another axis' given limits does not widen this one
                let outside = (0..3).any(|j| j != i && (j < 2 || three_d) && (user[j].0.is_some_and(|m| c[j] < m) || user[j].1.is_some_and(|m| c[j] > m)));
                if c[i].is_finite() && !outside {
                    dmin[i] = dmin[i].min(c[i]);
                    dmax[i] = dmax[i].max(c[i]);
                }
            }
        }
        // a matrix plot's cells reach halfway to the next point: in view as pgfplots' heuristic
        // finds them
        if let Some((rows, cols)) = p.lattice.filter(|_| p.image) {
            for c in super::mesh::image_limits(&v, rows, cols, 0) {
                for i in 0..2 {
                    if c[i].is_finite() {
                        dmin[i] = dmin[i].min(c[i]);
                        dmax[i] = dmax[i].max(c[i]);
                    }
                }
            }
        }
        // bars stand on the zero level: their ends are not data but pgfplots keeps them in view
        points.push(Some((v, base)));
    }
    // the data range of the tick algorithm: the data, and given limits when there are any
    let any_given = user.iter().take(if three_d { 3 } else { 2 }).any(|(a, b)| a.is_some() || b.is_some());
    let data_range = [0, 1, 2].map(|a| {
        let (mut lo, mut hi) = (dmin[a], dmax[a]);
        if any_given {
            if let Some(v) = user[a].0 {
                lo = lo.min(v);
            }
            if let Some(v) = user[a].1 {
                hi = hi.max(v);
            }
        }
        [lo, hi]
    });
    // user limits
    let mut min = dmin;
    let mut max = dmax;
    let mut auto_min = [true; 3];
    let mut auto_max = [true; 3];
    for a in 0..3 {
        let c = axis_char(a);
        if let Some(v) = super::symbolic::number(e, c, &format!("/pgfplots/{c}min")) {
            min[a] = lg(a, v);
            auto_min[a] = false;
        }
        if let Some(v) = super::symbolic::number(e, c, &format!("/pgfplots/{c}max")) {
            max[a] = lg(a, v);
            auto_max[a] = false;
        }
    }
    let dims = if three_d { 3 } else { 2 };
    let polar = axis.kind == "polaraxis";
    if polar {
        // angles: 0 to 360 unless both are given; radii: from 0 (or ymin) to the largest |r|
        if auto_min[0] || auto_max[0] {
            (min[0], max[0]) = (0.0, 360.0);
        }
        if auto_min[1] {
            min[1] = 0.0;
        }
        if auto_max[1] {
            max[1] = dmin[1].abs().max(dmax[1].abs());
            if !max[1].is_finite() || max[1] == 0.0 {
                max[1] = 1.0;
            }
        }
    }
    let invalid = (0..dims).any(|a| !(min[a].is_finite() && max[a].is_finite()));
    if invalid {
        e.warn("axis without data: drew the default window from 0 to 1");
        min = [0.0, 0.0, 0.0];
        max = [1.0, 1.0, 1.0];
        if log[0] {
            min[0] = 0.0;
        }
    }
    for a in 0..dims {
        if min[a] > max[a] && !(auto_min[a] && auto_max[a]) {
            std::mem::swap(&mut min[a], &mut max[a]);
        }
    }
    // enlargelimits (a polar axis enlarges only the largest radius, `\pgfplots@enlarge@limit@for@polar`)
    // `abs=1cm`: an amount on paper, applied once the axis is scaled
    let mut abs_dim: [Option<(f64, bool, bool)>; 3] = [None; 3];
    for a in if ternary || smith {
        0..0
    } else if polar {
        1..2
    } else {
        0..dims
    } {
        let c = axis_char(a);
        let v = e.pv(&format!("enlarge {c} limits"));
        // 'auto' enlarges only limits that were computed: x and y of a 2d axis, z of a 3d one
        let auto = if three_d { a == 2 } else { a < 2 };
        let (lo, hi, rel, abs) = enlarge_setting(&v, auto_min[a] && auto, auto_max[a] && auto);
        let lo = lo && !polar;
        let dim = v.trim_matches(|c| c == '{' || c == '}').split(',').find_map(|p| {
            let (k, val) = p.split_once('=')?;
            (matches!(k.trim(), "abs" | "abs value") && has_unit(val)).then(|| val.trim().to_string())
        });
        if let Some(d) = dim {
            if let Ok(len) = e.length(&d) {
                abs_dim[a] = Some((len, lo, hi));
            }
            continue;
        }
        // TeX multiplies the range (a dimension) by the factor read as a dimension: 0.1 is 6554 sp
        let rel = (rel * 65536.0).round() / 65536.0;
        // an absolute amount is a coordinate: logarithmic on a log axis
        let abs = abs.map(|v| if log[a] { v.ln() } else { v });
        let amount = abs.unwrap_or(rel * (max[a] - min[a]));
        if amount > 1e-9 * (max[a].abs() + 1.0) && !invalid {
            if lo {
                min[a] -= amount;
            }
            if hi {
                max[a] += amount;
            }
        }
        // an empty range is widened
        if (max[a] - min[a]).abs() <= 1e-9 * min[a].abs().max(1e-4) || (max[a] - min[a]).abs() < 1e-12 {
            if log[a] {
                min[a] -= 1.0;
                max[a] += 1.0;
            } else if max[a] == 0.0 {
                min[a] = -1.0;
                max[a] = 1.0;
            } else if max[a] > 0.0 {
                min[a] *= 0.8;
                max[a] *= 1.2;
            } else {
                min[a] *= 1.2;
                max[a] *= 0.8;
            }
        }
    }
    if !three_d {
        min[2] = 0.0;
        max[2] = 1.0;
    }

    // sizes
    let default = (240.0 * PT, 207.0 * PT);
    let w = keys::length(e, "/pgfplots/width");
    let h = keys::length(e, "/pgfplots/height");
    let (mut width, mut height) = match (w, h) {
        (None, None) => default,
        (Some(w), None) => (w, w / (default.0 / default.1)),
        (None, Some(h)) => (h * (default.0 / default.1), h),
        (Some(w), Some(h)) => (w, h),
    };
    if !keys::flag(e, "/pgfplots/scale only axis") {
        width = (width - 45.0 * PT).max(0.0);
        height = (height - 45.0 * PT).max(0.0);
    }
    let dir = [e.pv("x dir") == "reverse", e.pv("y dir") == "reverse", e.pv("z dir") == "reverse"];
    let mut ev = [Point::default(); 3];
    let user_unit = |e: &mut Engine, c: char| -> Option<Point> {
        let s = keys::value(e, &format!("/pgfplots/{c}"));
        if s.is_empty() {
            return None;
        }
        let t = s.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string();
        if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
            let parts = crate::keys::split_top_level(inner);
            let x = parts.first().and_then(|p| e.length(p).ok()).unwrap_or(0.0);
            let y = parts.get(1).and_then(|p| e.length(p).ok()).unwrap_or(0.0);
            return Some(Point::new(x, y));
        }
        let l = e.length(&t).ok()?;
        Some(match c {
            'x' => Point::new(l, 0.0),
            'y' => Point::new(0.0, l),
            _ => Point::new(0.0, l),
        })
    };
    let units = [user_unit(e, 'x'), user_unit(e, 'y'), user_unit(e, 'z')];
    if smith {
        // the diameter is `width` (less the labels' room), the height that of the width
        let (w, h) = (keys::length(e, "/pgfplots/width"), keys::length(e, "/pgfplots/height"));
        let (mut pw, mut ph) = match (w, h) {
            (None, None) => (207.0 * PT, 207.0 * PT),
            (Some(w), None) => (w, w),
            (None, Some(h)) => (h, h),
            (Some(w), Some(h)) => (w, h),
        };
        if !keys::flag(e, "/pgfplots/scale only axis") {
            pw = (pw - 45.0 * PT).max(0.0);
            ph = (ph - 45.0 * PT).max(0.0);
        }
        let mirrored = if keys::flag(e, "/pgfplots/smithchart mirrored") { -1.0 } else { 1.0 };
        ev[0] = Point::new(mirrored * pw / 2.0, 0.0);
        ev[1] = Point::new(0.0, ph / 2.0);
    } else if ternary {
        // a triangle of side `width` (less the labels' room)
        let v = super::ternary::vectors(width);
        for a in 0..3 {
            ev[a] = v[a].scale(1.0 / (max[a] - min[a]).abs().max(1e-300));
        }
    } else if polar {
        // the circle (or sector) stretched to width × height, which are equal by default
        let (w, h) = (keys::length(e, "/pgfplots/width"), keys::length(e, "/pgfplots/height"));
        let (mut pw, mut ph) = match (w, h) {
            (None, None) => (240.0 * PT, 240.0 * PT),
            (Some(w), None) => (w, w),
            (None, Some(h)) => (h, h),
            (Some(w), Some(h)) => (w, h),
        };
        if !keys::flag(e, "/pgfplots/scale only axis") {
            pw = (pw - 45.0 * PT).max(0.0);
            ph = (ph - 45.0 * PT).max(0.0);
        }
        // the whole circle's diameter, also for a sector (`…get@unit@scales@for@limits@polar`)
        let d = 2.0 * max[1].abs();
        ev[0] = Point::new(if d > 1e-12 { pw / d } else { 1.0 }, 0.0);
        ev[1] = Point::new(0.0, if d > 1e-12 { ph / d } else { 1.0 });
    } else if three_d && units.iter().all(Option::is_none) {
        let (az, el) = view_angles(e);
        let (saz, caz) = az.to_radians().sin_cos();
        let (sel, cel) = (-el).to_radians().sin_cos();
        let ratio: Vec<f64> = e.pv("plot box ratio").replace(['{', '}'], " ").split_whitespace().filter_map(|x| x.parse().ok()).collect();
        let r = |k: usize| ratio.get(k).copied().unwrap_or(1.0);
        let base = [Point::new(caz, saz * sel).scale(r(0)), Point::new(saz, -sel * caz).scale(r(1)), Point::new(0.0, cel).scale(r(2))];
        // stretch to fill: scale x and y components so that the box's projection is W × H
        let mut corners = Vec::new();
        for i in 0..8 {
            let r = [(i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64];
            let mut p = Point::default();
            for k in 0..3 {
                p = p.plus(base[k].scale(r[k] * if dir[k] { -1.0 } else { 1.0 }));
            }
            corners.push(p);
        }
        let (bw, bh) = bbox_size(&corners);
        let (sx, sy) = (if bw > 1e-9 { width / bw } else { 1.0 }, if bh > 1e-9 { height / bh } else { 1.0 });
        let ratio = e.pv("unit vector ratio");
        let mode = e.pv("scale mode");
        // `unit vector ratio` asks for one scale for all unit vectors
        let uniform = mode == "scale uniformly" || (mode == "auto" && !ratio.trim().is_empty());
        if uniform {
            let inp = super::scaling::Input {
                e: [0, 1, 2].map(|k| base[k].scale(if dir[k] { -1.0 } else { 1.0 })),
                width,
                height,
                unit_box: true,
                three_d: true,
                strategy: super::scaling::Strategy::parse(&e.pv("scale uniformly strategy")),
                ratio: &ratio,
                ratio_axis: &e.pv("unit vector ratio axis"),
                keep: super::scaling::KeepSize::parse(&e.pv("unit rescale keep size")),
                auto_min,
                auto_max,
            };
            ev = super::scaling::uniform(&inp, &mut min, &mut max);
        }
        for k in (0..3).filter(|_| !uniform) {
            let range = (max[k] - min[k]).abs().max(1e-12);
            let s = if dir[k] { -1.0 } else { 1.0 };
            // 1/range as pgfplots multiplies it (compat before 1.3's pgfbasic scaling): a TeX
            // dimension, in sp, of the range in the data scale transformation's units
            let [lo, hi] = data_range[k];
            let f = if log[k] || keys::flag(e, "/pgfplots/@disabledatascaling") || !(lo.is_finite() && hi.is_finite()) {
                1.0
            } else {
                let size = keys::length(e, if k == 0 { "/pgfplots/width" } else { "/pgfplots/height" }).unwrap_or(if k == 0 { 240.0 * PT } else { 207.0 * PT }) / PT;
                ticks::data_scale(lo, hi, size)
            };
            let inverse = ((65536.0 / (range * f)).round() / 65536.0 * f).max(1e-300);
            let inverse = if inverse.is_finite() && inverse > 0.0 { inverse } else { 1.0 / range };
            ev[k] = Point::new(base[k].x * sx * s * inverse, base[k].y * sy * s * inverse);
        }
    } else {
        for a in 0..dims {
            let range = (max[a] - min[a]).abs().max(1e-300);
            let s = if dir[a] { -1.0 } else { 1.0 };
            ev[a] = match units[a] {
                Some(u) => u.scale(s),
                None => {
                    if a == 0 {
                        Point::new(s * width / range, 0.0)
                    } else {
                        Point::new(0.0, s * height / range)
                    }
                }
            };
        }
        // a 3d axis with given unit vectors, scaled uniformly to width × height
        let ratio = e.pv("unit vector ratio");
        let mode = e.pv("scale mode");
        if three_d && units.iter().all(Option::is_some) && (mode == "scale uniformly" || (mode == "auto" && !ratio.trim().is_empty())) {
            let inp = super::scaling::Input {
                e: ev,
                width,
                height,
                unit_box: false,
                three_d: true,
                strategy: super::scaling::Strategy::parse(&e.pv("scale uniformly strategy")),
                ratio: &ratio,
                ratio_axis: &e.pv("unit vector ratio axis"),
                keep: super::scaling::KeepSize::parse(&e.pv("unit rescale keep size")),
                auto_min,
                auto_max,
            };
            ev = super::scaling::uniform(&inp, &mut min, &mut max);
        }
        // unit vector ratio (axis equal): one scale for both, the limits enlarged to keep the size
        if !ratio.is_empty() && !three_d && units[0].is_none() && units[1].is_none() {
            let r: Vec<f64> = ratio.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            let (rx, ry) = (r.first().copied().unwrap_or(1.0), r.get(1).copied().unwrap_or(1.0));
            let keep = e.pv("unit rescale keep size");
            let ux = ev[0].x.abs() / rx;
            let uy = ev[1].y.abs() / ry;
            let u = ux.min(uy);
            let declared = !auto_min[0] && !auto_max[0] && !auto_min[1] && !auto_max[1];
            let keep_size = keep == "true" || (keep == "unless limits declared" && !declared);
            for (a, k) in [(0usize, rx), (1usize, ry)] {
                let target = u * k;
                let current = if a == 0 { ev[0].x.abs() } else { ev[1].y.abs() };
                if keep_size && current > target + 1e-12 {
                    // enlarge the range around its center
                    let range = max[a] - min[a];
                    let new = range * current / target;
                    let c = (max[a] + min[a]) / 2.0;
                    min[a] = c - new / 2.0;
                    max[a] = c + new / 2.0;
                }
                let s = if dir[a] { -1.0 } else { 1.0 };
                if a == 0 {
                    ev[0] = Point::new(s * target, 0.0);
                } else {
                    ev[1] = Point::new(0.0, s * target);
                }
            }
        }
    }
    // `enlarge x limits={abs=1cm}`: the axis keeps its length, the data leave that room
    if !three_d {
        for a in 0..2 {
            let Some((d, lo, hi)) = abs_dim[a] else { continue };
            let range = (max[a] - min[a]).abs();
            let unit = ev[a].norm();
            let length = unit * range;
            let room = d * (f64::from(u8::from(lo)) + f64::from(u8::from(hi)));
            if range > 0.0 && length > room {
                let u2 = (length - room) / range;
                if lo {
                    min[a] -= d / u2;
                }
                if hi {
                    max[a] += d / u2;
                }
                ev[a] = ev[a].scale(u2 / unit);
            }
        }
    }
    // post scale
    for (a, v) in ev.iter_mut().enumerate() {
        if let Some(f) = keys::number(e, &format!("/pgfplots/{} post scale", axis_char(a))) {
            *v = v.scale(f);
        }
    }
    // the origin: the box's lower left corner at the picture's origin
    let origin_local = Point::default();
    let mut zero = [0.0; 3];
    for a in 0..3 {
        zero[a] = if log[a] { 0.0 } else { 0.0f64.clamp(min[a].min(max[a]), max[a].max(min[a])) };
    }
    let lines = [e.pv("@x line"), e.pv("@y line"), e.pv("@z line")];
    let mut label_side = [-1i8; 3];
    for a in 0..3 {
        let c = axis_char(a);
        let pos = e.pv(&format!("{c}ticklabel pos"));
        let tickpos = e.pv(&format!("{c}tick pos"));
        label_side[a] = match lines[a].as_str() {
            "center" => 0,
            "high" => 1,
            "low" => -1,
            _ => match pos.as_str() {
                "right" | "top" | "upper" => 1,
                "left" | "bottom" | "lower" => -1,
                _ => {
                    if tickpos == "right" {
                        1
                    } else {
                        -1
                    }
                }
            },
        };
    }
    let label_shift = [0, 1, 2].map(|a| keys::length(e, &format!("/pgfplots/{}label shift", axis_char(a))).unwrap_or(0.0));
    // the box with its lower left corner at the origin of the picture's coordinates
    let base = e.st.transform.apply(origin_local);
    let mut frame = Frame {
        origin: base,
        e: ev,
        min,
        max,
        log,
        three_d,
        zero,
        cs: Cell::new(Cs::Plain),
        label_axis: Cell::new(0),
        tick_dim: RefCell::new([0.0; 3]),
        label_side,
        label_shift,
        meta: meta_range(e, axis),
        colormap: colormap(e),
        fg: [[false, true]; 3],
        label_line: [*b"v00", *b"0v0", *b"00v"],
        box_line: [*b"v00", *b"0v0", *b"00v"],
        data_range,
        polar,
        ternary,
        ternary_relative: ternary && keys::flag(e, "/pgfplots/ternary limits relative"),
        smith,
        slope: Cell::new(None),
        upside_down: RefCell::new(None),
        reversed: dir,
    };
    if smith {
        // the chart's lower left corner at the origin
        frame.origin = base.plus(Point::new(frame.e[0].norm(), frame.e[1].norm()));
    } else if ternary {
        // the lower left corner is at the origin already
    } else if three_d {
        super::cube::prepare(e, &mut frame, dir, &lines);
    } else {
        // `lower` is lower on paper (left for y): with `y dir=reverse` the x tick labels go to
        // ymax (`\pgfplots@init@ticklabelaxisspec@twodim@for`)
        for a in 0..2 {
            let lower_is_min = if a == 0 { frame.e[1].y >= 0.0 } else { frame.e[0].x >= 0.0 };
            if !lower_is_min && frame.label_side[a] != 0 {
                frame.label_side[a] = -frame.label_side[a];
            }
        }
    }
    // the box's lowest corner goes to the origin
    if polar {
        let (lo, _) = frame.bbox();
        frame.origin = frame.origin.plus(base.minus(lo));
    } else if !three_d {
        let a = frame.at(frame.min);
        let b = frame.at(frame.max);
        let lo = Point::new(a.x.min(b.x), a.y.min(b.y));
        frame.origin = frame.origin.plus(base.minus(lo));
    }
    // ticks
    let mut hide = [keys::flag(e, "/pgfplots/hide x axis"), keys::flag(e, "/pgfplots/hide y axis"), keys::flag(e, "/pgfplots/hide z axis")];
    // an axis of no length on paper (z at view {0}{90}) is hidden (`\pgfplots@hide@ztrue`)
    for (a, h) in hide.iter_mut().enumerate() {
        if a < dims && frame.e[a].norm() * (frame.max[a] - frame.min[a]).abs() < 1e-9 {
            *h = true;
        }
    }
    let mut ticks: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut dist = [0.0; 3];
    for a in 0..dims {
        let (mut t, d) = tick_list(e, a, &frame, axis)?;
        // `xtickmin`, `xtickmax`: no tick outside
        let c = axis_char(a);
        let tol = 1e-9 * (frame.max[a] - frame.min[a]).abs().max(1e-300);
        if let Some(lo) = keys::number(e, &format!("/pgfplots/{c}tickmin")) {
            let lo = frame.logical(a, lo);
            t.retain(|v| *v >= lo - tol);
        }
        if let Some(hi) = keys::number(e, &format!("/pgfplots/{c}tickmax")) {
            let hi = frame.logical(a, hi);
            t.retain(|v| *v <= hi + tol);
        }
        ticks[a] = t;
        dist[a] = d;
    }
    Ok(Setup { frame: Rc::new(frame), points, ticks, tick_distance: dist, hide, line: lines })
}

/// The styles that pgfplots' `descriptions` family holds, and the keys that only set them.
const DESCRIPTION_KEYS: &[&str] = &[
    "every axis label",
    "every axis x label",
    "every axis y label",
    "every axis z label",
    "every axis title",
    "every x tick scale label",
    "every y tick scale label",
    "every z tick scale label",
    "label style",
    "xlabel style",
    "ylabel style",
    "zlabel style",
    "title style",
    "x tick scale label style",
    "y tick scale label style",
    "z tick scale label style",
    "xlabel near ticks",
    "ylabel near ticks",
    "xlabel absolute",
    "ylabel absolute",
    // their label styles
    "axis lines",
    "axis x line",
    "axis y line",
    "axis z line",
];

/// `\pgfplots@BEGIN@prepare@description@styles`: once an axis is known to be 3d it gets
/// `every 3d description` (and `every 3d view {az}{el}`); the description styles of `every axis`
/// and of the axis' options are then set again, so that they still win.
fn three_d_descriptions(e: &mut Engine, axis: &Axis) -> Result<(), Error> {
    let az = e.pv("view/az");
    let el = e.pv("view/el");
    let saved = e.st.key_path.clone();
    e.st.key_path = "/pgfplots".into();
    let r = (|| {
        keys::run(e, "every 3d description")?;
        keys::run(e, &format!("every 3d view {{{az}}}{{{el}}}/.try"))?;
        let mut lists = Vec::new();
        if let Some(Meaning(body)) = style_code(e, "/pgfplots/every axis") {
            lists.push(body);
        }
        lists.push(axis.options.clone());
        for list in lists {
            let kept = description_items(&list);
            if !kept.is_empty() {
                e.process_keys(kept, Some("/pgfplots"))?;
            }
        }
        Ok(())
    })();
    e.st.key_path = saved;
    r
}

/// The items of a key list (or of a style's `\pgfkeysalso{…}`s) that set description styles.
fn description_items(list: &[Token]) -> Vec<Token> {
    let mut lists = Vec::new();
    let mut i = 0;
    let mut plain = Vec::new();
    while i < list.len() {
        if list[i].is_cs("pgfkeysalso") {
            let (arg, end) = group_at(list, i + 1);
            lists.push(arg);
            i = end;
            continue;
        }
        plain.push(list[i].clone());
        i += 1;
    }
    lists.push(plain);
    let mut out = Vec::new();
    for l in lists {
        for item in pk::split_list(&l) {
            let path = item.key.trim_start_matches("/pgfplots/");
            let name = pk::handler(path).map_or(path, |(base, _)| base);
            if !DESCRIPTION_KEYS.contains(&name.trim()) {
                continue;
            }
            if !out.is_empty() {
                out.extend(tex::string_tokens(","));
            }
            out.extend(tex::string_tokens(&item.key));
            if let Some(v) = item.value {
                out.extend(tex::string_tokens("="));
                out.push(Token::Char('{', tex::Cat::Begin));
                out.extend(v);
                out.push(Token::Char('}', tex::Cat::End));
            }
        }
    }
    out
}

/// The point meta range of the axis' plots (`point meta min/max` if given).
fn meta_range(e: &mut Engine, axis: &Axis) -> [f64; 2] {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for en in &axis.entries {
        if let Entry::Plot(p) = en {
            if !p.has_meta {
                continue;
            }
            for c in &p.coords {
                if c.meta.is_finite() {
                    lo = lo.min(c.meta);
                    hi = hi.max(c.meta);
                }
            }
        }
    }
    if let Some(v) = keys::number(e, "/pgfplots/point meta min") {
        lo = v;
    }
    if let Some(v) = keys::number(e, "/pgfplots/point meta max") {
        hi = v;
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return [0.0, 1.0];
    }
    if hi - lo < 1e-12 {
        return [lo - 0.5, hi + 0.5];
    }
    [lo, hi]
}

/// The current colormap (`colormap name`).
pub fn colormap(e: &mut Engine) -> Option<super::colormap::Colormap> {
    let name = keys::value(e, "/pgfplots/colormap name");
    let m = colormap_named(e, &name);
    if m.is_none() {
        e.warn(format!("colormap '{name}' not defined"));
    }
    m
}

/// A colormap by name, with the current `colormap access`.
pub fn colormap_named(e: &mut Engine, name: &str) -> Option<super::colormap::Colormap> {
    let spec = keys::value(e, &format!("/pgfplots/@colormaps/{name}"));
    let mut m = super::colormap::Colormap::parse(&spec, &e.colors);
    if let Some(m) = m.as_mut() {
        m.access = match keys::value(e, "/pgfplots/colormap access").trim() {
            "piecewise constant" | "const" => super::colormap::Access::Const,
            "direct" => super::colormap::Access::Direct,
            _ => super::colormap::Access::Map,
        };
    }
    m
}

fn bbox_size(points: &[Point]) -> (f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in points {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    (x1 - x0, y1 - y0)
}

/// A list of tick values (`1,2,...,10`, `{0,0.5}`), in data units.
pub(super) fn number_list(e: &mut Engine, tokens: Vec<Token>) -> Result<Vec<f64>, Error> {
    let list = e.tex.expand_fully(tokens)?;
    let mut v = Vec::new();
    for item in super::super::foreach_items(&list) {
        if let Ok(x) = e.number_of(item) {
            if x.is_finite() {
                v.push(x);
            }
        }
    }
    Ok(v)
}

/// The tick candidates of axis `a` (logical units) and their distance.
fn tick_list(e: &mut Engine, a: usize, f: &Frame, axis: &Axis) -> Result<(Vec<f64>, f64), Error> {
    let c = axis_char(a);
    let raw = pk::value(&e.tex, &format!("/pgfplots/{c}tick")).unwrap_or_default();
    let trimmed = pk::trim(&raw).to_vec();
    if trimmed.len() == 1 && trimmed[0].is_cs("empty") {
        return Ok((Vec::new(), 0.0));
    }
    // `xtickten={1,2}` on a log axis: ticks at 10^1 and 10^2
    if f.log[a] && text_is_empty(&e.tex.expand_to_string(trimmed.clone())?) {
        if let Some(list) = pk::value(&e.tex, &format!("/pgfplots/{c}tickten")).filter(|l| !pk::trim(l).is_empty()) {
            let v = number_list(e, list)?;
            return Ok((v.into_iter().map(|k| k * std::f64::consts::LN_10).collect(), 0.0));
        }
    }
    let text = e.tex.expand_to_string(trimmed.clone())?;
    let text = text.trim();
    if text == "data" {
        let first = axis.entries.iter().find_map(|en| if let Entry::Plot(p) = en { Some(p) } else { None });
        let mut v: Vec<f64> = Vec::new();
        if let Some(p) = first {
            for co in &p.coords {
                let x = [co.x, co.y, co.z][a];
                if x.is_finite() && !v.iter().any(|y| (y - x).abs() < 1e-12) {
                    v.push(x);
                }
            }
        }
        return Ok((v.into_iter().map(|x| f.logical(a, x)).collect(), 0.0));
    }
    if !text.is_empty() {
        // names of a symbolic axis
        if !super::symbolic::names(e, c).is_empty() {
            let mut v = Vec::new();
            for item in super::super::foreach_items(&e.tex.expand_fully(trimmed.clone())?) {
                let s = tex::tokens_to_string(&item);
                match super::symbolic::index(e, c, &s) {
                    Some(i) => v.push(i),
                    None => {
                        if let Ok(x) = e.number_of(item) {
                            v.push(x);
                        }
                    }
                }
            }
            return Ok((v, 0.0));
        }
        let v = number_list(e, trimmed)?;
        return Ok((v.into_iter().map(|x| f.logical(a, x)).filter(|x| x.is_finite()).collect(), 0.0));
    }
    if f.polar {
        if let Some(t) = super::polar::ticks(e, a, f) {
            return Ok(t);
        }
    }
    let length_pt = f.e[a].norm() * (f.max[a] - f.min[a]).abs() / PT;
    let max_space = keys::number(e, "/pgfplots/max space between ticks").unwrap_or(35.0);
    let distance = keys::number(e, &format!("/pgfplots/{c}tick distance"));
    let (lo, hi) = (f.min[a].min(f.max[a]), f.min[a].max(f.max[a]));
    if f.log[a] {
        let try_min = keys::number(e, "/pgfplots/try min ticks log").unwrap_or(3.0) as usize;
        // `log basis y=2`: the ticks at powers of the basis (`log basis ticks`), found as powers of
        // ten in logarithms scaled by ln 10 / ln b
        let k = match keys::number(e, &format!("/pgfplots/log basis {c}")) {
            Some(b) if b > 0.0 && b != 1.0 && b != 10.0 => std::f64::consts::LN_10 / b.ln(),
            _ => 1.0,
        };
        let distance = distance.map(|d| d.powf(k));
        let (t, d, _) = ticks::logarithmic(lo * k, hi * k, TickSettings { length_pt, max_space, try_min, distance, scale: None });
        Ok((t.into_iter().map(|v| v / k).collect(), d / k))
    } else {
        let try_min = keys::number(e, "/pgfplots/try min ticks").unwrap_or(4.0) as usize;
        // the data scale transformation: the data range (with given limits) and `width`/`height`
        let size = keys::length(e, if a == 0 { "/pgfplots/width" } else { "/pgfplots/height" }).unwrap_or(if a == 0 { 240.0 * PT } else { 207.0 * PT }) / PT;
        let [dlo, dhi] = f.data_range[a];
        let scale = Some((dlo, dhi, size));
        Ok(ticks::linear(lo, hi, TickSettings { length_pt, max_space, try_min, distance, scale }))
    }
}

/// The view's azimuth and elevation, in degrees (`trig format=rad` gives them in radians).
pub(super) fn view_angles(e: &mut Engine) -> (f64, f64) {
    let az = keys::number(e, "/pgfplots/view/az").unwrap_or(25.0);
    let el = keys::number(e, "/pgfplots/view/el").unwrap_or(30.0);
    if e.st.trig_rad {
        (az.to_degrees(), el.to_degrees())
    } else {
        (az, el)
    }
}

/// Whether a coordinate's part has a unit (`1cm`, `-2pt`).
fn has_unit(part: &str) -> bool {
    let t = part.trim().trim_end_matches('}');
    ["cm", "mm", "pt", "in", "em", "ex", "bp", "pc"].iter().any(|u| t.ends_with(u))
}

fn text_is_empty(s: &str) -> bool {
    s.trim().is_empty()
}

/// The ticks of axis `a` that are drawn: (index in the list, logical value).
pub(super) fn visible(f: &Frame, a: usize, list: &[f64]) -> Vec<(usize, f64)> {
    let (lo, hi) = (f.min[a].min(f.max[a]), f.min[a].max(f.max[a]));
    let tol = 0.05 * PT / f.e[a].norm().max(f64::MIN_POSITIVE);
    list.iter().copied().enumerate().filter(|(_, t)| *t >= lo - tol && *t <= hi + tol).collect()
}

/// The point of logical value `t` on axis `a`, with the other 2d axis at `b_value`.
fn on_axis(f: &Frame, a: usize, t: f64, b_value: f64) -> Point {
    let b = 1 - a;
    let mut l = f.min;
    l[a] = t;
    l[b] = b_value;
    f.at(l)
}

pub(super) fn unit(p: Point) -> Point {
    // tiny vectors are not zero: an axis up to 10^21 has a unit vector of 10^-20 pt
    let n = p.norm();
    if n > 0.0 {
        p.scale(1.0 / n)
    } else {
        p
    }
}

/// Everything of the axis, in pgfplots' order.
fn draw_all(e: &mut Engine, axis: &Axis, s: &Setup) -> Result<(), Error> {
    let f = s.frame.clone();
    let on_top = keys::flag(e, "/pgfplots/axis on top");
    // axis background
    if let Some(Meaning(code)) = style_code(e, "/pgfplots/axis background") {
        let _ = code;
        let rect = box_path(&f);
        e.draw_with("draw=none,/pgfplots/axis background", rect)?;
    }
    if f.ternary || f.smith {
        if !on_top {
            other_axis(e, s)?;
        }
    } else if f.polar && !on_top {
        super::polar::draw_axis(e, s)?;
    } else if !f.three_d && !on_top {
        draw_axis_2d(e, s)?;
    }
    if f.three_d && !f.ternary && !on_top {
        super::cube::draw(e, s, false)?;
    }
    // the plots, clipped
    let clip = keys::flag(e, "/pgfplots/clip");
    e.begin_scope();
    if clip {
        let segs = if f.smith {
            super::smith::clip_path(&f)
        } else if f.ternary {
            super::ternary::outline(&f)
        } else if f.polar {
            super::polar::clip_path(&f)
        } else if f.three_d {
            hull_path(&f)
        } else {
            box_path(&f)
        };
        // `clip bounding box=default tikz`: the clip path counts in the picture's size
        if !e.canvas_frozen {
            e.canvas.include_path(&segs);
        }
        e.open_in_scope(Item::Clip { segments: segs, even_odd: false });
    }
    // what is clipped does not grow the picture
    let saved_bbox = (e.canvas.min, e.canvas.max);
    // the layer `pre main`: fill between regions go below the other plots
    let pre_main = e.canvas.items.len();
    // where the next fill between region goes: after those before it
    let mut pre_main_end = pre_main;
    let compat = keys::compat(e);
    f.cs.set(if keys::substitute_pgfpoint(e) { Cs::Axis } else { Cs::Plain });
    // before compat 1.11 TikZ's unit vectors are those of the data scale transformation: a radius
    // or a relative coordinate is in its units
    let units = (e.st.x, e.st.y, e.st.z);
    if compat < 111 && !f.polar && !f.smith && !f.ternary {
        let inverse = e.st.transform.inverse();
        let unit = |e: &mut Engine, a: usize| inverse.linear(f.e[a].scale(1.0 / data_scale_trafo(e, &f, a).0));
        e.st.x = unit(e, 0);
        e.st.y = unit(e, 1);
        if f.three_d {
            e.st.z = unit(e, 2);
        }
    }
    let mut marks = Vec::new();
    let plots: Vec<&Plot> = axis.entries.iter().filter_map(|en| if let Entry::Plot(p) = en { Some(&**p) } else { None }).collect();
    let mut k = 0usize;
    for (i, en) in axis.entries.iter().enumerate() {
        match en {
            Entry::Plot(p) => {
                let (pts, base) = s.points[i].clone().unwrap_or_default();
                let of_type = plots.iter().filter(|q| q.handler == p.handler && !q.forget).count().max(1);
                let index_of_type = plots[..k].iter().filter(|q| q.handler == p.handler && !q.forget).count();
                k += 1;
                let before = e.canvas.items.len();
                let source = std::mem::replace(&mut e.source, p.source);
                e.mark_source(p.source);
                let r = super::visual::draw_plot(e, p, &pts, &base, &f, of_type, index_of_type);
                e.mark_source(axis.source);
                e.source = source;
                if p.fill_between.is_some() && before <= e.canvas.items.len() && pre_main_end <= before {
                    let drawn: Vec<Item> = e.canvas.items.drain(before..).collect();
                    let at = pre_main_end.min(e.canvas.items.len());
                    pre_main_end = at + drawn.len();
                    e.canvas.items.splice(at..at, drawn);
                }
                match r {
                    Ok(Some(m)) => marks.push(m),
                    Ok(None) => {}
                    Err(err) if super::super::is_limit(&err) => return Err(err),
                    Err(err) => e.warn(format!("\\addplot: {err}; plot not drawn")),
                }
            }
            Entry::Tikz(tokens, source) => {
                e.mark_source(*source);
                let here = std::mem::replace(&mut e.source, *source);
                // what the axis' body draws is the figure's
                let mute = std::mem::take(&mut e.facts.mute);
                let r = e.run_tokens(tokens.clone());
                e.facts.mute = mute;
                e.source = here;
                // (a statement reports its own errors and goes on: only limits come back)
                r?;
                e.mark_source(axis.source);
            }
        }
    }
    // `before end axis/.code`: the end of the axis' body
    // (it, `extra description` and `after end axis` are always defined: empty code by default)
    let r = in_plain_units(e, &f, |e| e.process_keys(toks("/pgfplots/before end axis"), None));
    if let Err(err) = r {
        if super::super::is_limit(&err) {
            return Err(err);
        }
        e.warn(format!("before end axis: {err}"));
    }
    e.end_scope();
    (e.st.x, e.st.y, e.st.z) = units;
    if clip {
        (e.canvas.min, e.canvas.max) = saved_bbox;
    }
    // `3d box=complete`: the foreground edges, on top of the plots
    if f.three_d && !f.ternary && e.pv("3d box") != "background" {
        super::cube::draw(e, s, true)?;
    }
    if on_top {
        if f.ternary || f.smith {
            other_axis(e, s)?;
        } else if f.polar {
            super::polar::draw_axis(e, s)?;
        } else if f.three_d {
            super::cube::draw(e, s, false)?;
        } else {
            draw_axis_2d(e, s)?;
        }
    }
    // marks are not clipped (`clip marker paths=false`)
    // inside the box: by the point's axis coordinates (the box is a parallelogram with unit
    // vectors of any direction)
    let inside = |p: &Point| match f.data_at(*p) {
        Some(v) => (0..2).all(|k| {
            let (l, tol) = (f.logical(k, v[k]), 0.05 * PT / f.e[k].norm().max(f64::MIN_POSITIVE));
            l >= f.min[k].min(f.max[k]) - tol && l <= f.min[k].max(f.max[k]) + tol
        }),
        None => true,
    };
    for mut m in marks {
        // marks outside the axis are not drawn, those on its border are drawn whole
        if clip && !f.three_d && !f.polar && !f.smith {
            m.points.retain(|(p, _)| inside(p));
            m.styled.retain(|(p, _, _)| inside(p));
            m.nodes.retain(|n| inside(&n.point));
        } else if clip && f.polar {
            m.points.retain(|(p, _)| super::polar::contains(&f, *p));
            m.nodes.retain(|n| super::polar::contains(&f, n.point));
        }
        super::visual::draw_marks(e, &m, &f)?;
        super::visual::draw_near_nodes(e, &m)?;
    }
    // `current axis` exists for the descriptions (`current axis.right of origin`)
    let (lo, hi) = f.bbox();
    let (blo, bhi) = e.canvas.min.zip(e.canvas.max).unwrap_or((lo, hi));
    register_axis_node(e, "current axis", lo, hi, blo, bhi, &f);
    // descriptions, in description coordinates
    f.cs.set(Cs::Description);
    // the descriptions see the limits as computed (`\pgfkeysvalueof{/pgfplots/xmin}` in a title)
    e.begin_group();
    for a in 0..if f.three_d { 3 } else { 2 } {
        let c = axis_char(a);
        for (k, v) in [("min", f.min[a]), ("max", f.max[a])] {
            let v = if f.log[a] { v.exp() } else { v };
            if v.is_finite() && !f.polar && !f.ternary && !f.smith {
                keys::set(e, &format!("/pgfplots/{c}{k}"), &super::super::pgf_number(v));
            }
        }
    }
    let r = descriptions(e, axis, s);
    e.end_group();
    r?;
    f.cs.set(Cs::Plain);
    if f.smith && keys::flag(e, "/pgfplots/show origin") {
        super::smith::show_origin(e, &f)?;
    }
    Ok(())
}

/// The axes of the libraries that draw their own.
fn other_axis(e: &mut Engine, s: &Setup) -> Result<(), Error> {
    if s.frame.smith {
        super::smith::draw_axis(e, s)
    } else {
        super::ternary::draw_axis(e, s)
    }
}

/// A style's code, if it does something.
struct Meaning(Vec<Token>);

fn style_code(e: &Engine, path: &str) -> Option<Meaning> {
    // (the styles asked for are pgfplots', always macros)
    let crate::tex::Meaning::Macro(m) = e.tex.meaning_of(&pk::macro_name(&format!("{path}/.@cmd"))) else { return None };
    let body = tex::substitute(&m.body, &[vec![]]);
    let text = tex::tokens_to_string(&body);
    let inner = text.replace("\\pgfkeysalso", "").replace(['{', '}', ' '], "");
    if inner.is_empty() {
        None
    } else {
        Some(Meaning(body))
    }
}

fn box_path(f: &Frame) -> Vec<Segment> {
    let p = |x: f64, y: f64| f.at([x, y, f.min[2]]);
    vec![
        Segment::MoveTo(p(f.min[0], f.min[1])),
        Segment::LineTo(p(f.max[0], f.min[1])),
        Segment::LineTo(p(f.max[0], f.max[1])),
        Segment::LineTo(p(f.min[0], f.max[1])),
        Segment::Close,
    ]
}

/// The outline of a 3d box's projection (the clip path).
fn hull_path(f: &Frame) -> Vec<Segment> {
    let mut pts = Vec::new();
    for i in 0..8 {
        let l = [if i & 1 == 0 { f.min[0] } else { f.max[0] }, if i & 2 == 0 { f.min[1] } else { f.max[1] }, if i & 4 == 0 { f.min[2] } else { f.max[2] }];
        pts.push(f.at(l));
    }
    let hull = convex_hull(pts);
    let mut v = Vec::new();
    for (i, p) in hull.iter().enumerate() {
        v.push(if i == 0 { Segment::MoveTo(*p) } else { Segment::LineTo(*p) });
    }
    v.push(Segment::Close);
    v
}

fn convex_hull(mut pts: Vec<Point>) -> Vec<Point> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let cross = |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Point> = Vec::new();
    for p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 1e-12 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 1e-12 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The 2d axis: grids, ticks, lines, tick labels and extra ticks, x and y in turn.
fn draw_axis_2d(e: &mut Engine, s: &Setup) -> Result<(), Error> {
    let f = s.frame.clone();
    for a in 0..2 {
        if !s.hide[a] {
            grid(e, s, a, &s.ticks[a], "")?;
        }
    }
    for a in 0..2 {
        if !s.hide[a] {
            tick_lines(e, s, a, &s.ticks[a], "")?;
        }
    }
    axis_lines(e, s)?;
    for a in 0..2 {
        if !s.hide[a] {
            tick_labels(e, s, a, &s.ticks[a], false)?;
        }
    }
    for a in 0..2 {
        if s.hide[a] {
            continue;
        }
        let c = axis_char(a);
        let extra = pk::value(&e.tex, &format!("/pgfplots/extra {c} ticks")).unwrap_or_default();
        if pk::trim(&extra).is_empty() {
            continue;
        }
        let v: Vec<f64> = number_list(e, extra)?.into_iter().map(|x| f.logical(a, x)).collect();
        e.begin_group();
        let r = (|| {
            e.st.key_path = "/pgfplots".into();
            keys::run(e, &format!("every extra {c} tick"))?;
            grid(e, s, a, &v, "extra")?;
            tick_lines(e, s, a, &v, "extra")?;
            tick_labels(e, s, a, &v, true)
        })();
        e.end_group();
        r?;
    }
    Ok(())
}

/// Grid lines at the ticks of axis `a`.
fn grid(e: &mut Engine, s: &Setup, a: usize, list: &[f64], extra: &str) -> Result<(), Error> {
    let f = &s.frame;
    let c = axis_char(a);
    let b = 1 - a;
    let major = keys::flag(e, &format!("/pgfplots/{c}majorgrids"));
    let minor = keys::flag(e, &format!("/pgfplots/{c}minorgrids")) && extra.is_empty();
    let line = |t: f64| vec![Segment::MoveTo(on_axis(f, a, t, f.min[b])), Segment::LineTo(on_axis(f, a, t, f.max[b]))];
    if minor {
        let segs: Vec<Segment> = minor_ticks(e, s, a, list).into_iter().flat_map(line).collect();
        e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every minor grid,/pgfplots/every axis {c} grid,/pgfplots/every minor {c} grid"), segs)?;
    }
    if major {
        let segs: Vec<Segment> = visible(f, a, list).into_iter().flat_map(|(_, t)| line(t)).collect();
        e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every major grid,/pgfplots/every axis {c} grid,/pgfplots/every major {c} grid"), segs)?;
    }
    Ok(())
}

/// Minor tick positions: `minor x tick num` between the ticks (9 per decade on log axes), or
/// `minor xtick`.
pub(super) fn minor_ticks(e: &mut Engine, s: &Setup, a: usize, list: &[f64]) -> Vec<f64> {
    let f = &s.frame;
    let c = axis_char(a);
    if let Some(v) = keys::value_tokens(e, &format!("/pgfplots/minor {c}tick")) {
        return number_list(e, v).unwrap_or_default().into_iter().map(|x| f.logical(a, x)).filter(|t| visible(f, a, &[*t]).len() == 1).collect();
    }
    let mut out = Vec::new();
    if f.log[a] {
        let uniform = list.windows(2).all(|w| ((w[1] - w[0]) - std::f64::consts::LN_10).abs() < 1e-6);
        if !uniform || list.len() < 2 {
            return out;
        }
        for t in &list[..list.len() - 1] {
            for i in 2..=9 {
                out.push(t + (i as f64).ln());
            }
        }
    } else {
        let n = keys::number(e, &format!("/pgfplots/minor {c} tick num")).unwrap_or(0.0) as usize;
        if n == 0 || list.len() < 2 {
            return out;
        }
        let d = s.tick_distance[a];
        let d = if d > 0.0 { d } else { list[1] - list[0] };
        for t in &list[..list.len() - 1] {
            for i in 1..=n {
                out.push(t + i as f64 * d / (n + 1) as f64);
            }
        }
    }
    out.into_iter().filter(|t| visible(f, a, &[*t]).len() == 1).collect()
}

/// The tick marks of axis `a`.
fn tick_lines(e: &mut Engine, s: &Setup, a: usize, list: &[f64], extra: &str) -> Result<(), Error> {
    let f = &s.frame;
    let c = axis_char(a);
    let b = 1 - a;
    let pos = e.pv(&format!("{c}tick pos"));
    let align = e.pv(&format!("{c}tick align"));
    let (lower, upper, center) = match pos.as_str() {
        "left" | "bottom" => (true, false, false),
        "right" | "top" => (false, true, false),
        "center" => (true, false, true),
        "none" => (false, false, false),
        _ => (true, true, false),
    };
    let reversed_b = (f.max[b] - f.min[b]) * if f.e[b].x + f.e[b].y < 0.0 { -1.0 } else { 1.0 } < 0.0;
    let (lower, upper) = if reversed_b && !center { (upper, lower) } else { (lower, upper) };
    let ub = unit(f.e[b]).scale(if f.max[b] >= f.min[b] { 1.0 } else { -1.0 });
    let sh = line_shift(e, f, a);
    let mut kinds = Vec::new();
    if keys::flag(e, &format!("/pgfplots/{c}minorticks")) && extra.is_empty() {
        let m = minor_ticks(e, s, a, list);
        if !m.is_empty() {
            let len = keys::length(e, "/pgfplots/minor tick length").unwrap_or(0.1);
            kinds.push(("minor", m, len));
        }
    }
    if keys::flag(e, &format!("/pgfplots/{c}majorticks")) {
        let m: Vec<f64> = visible(f, a, list).into_iter().map(|(_, t)| t).filter(|t| !obscured(e, s, a, *t)).collect();
        let len = keys::length(e, "/pgfplots/major tick length").unwrap_or(0.15);
        kinds.push(("major", m, len));
    }
    for (kind, ticks, len) in kinds {
        let off = match align.as_str() {
            "outside" => len,
            "center" => len / 2.0,
            _ => 0.0,
        };
        let mut segs = Vec::new();
        for t in ticks {
            if lower {
                let bv = if center { f.zero[b] } else { f.min[b] };
                let out = if center { f.normal(a) } else { ub.scale(-1.0) };
                let p = on_axis(f, a, t, bv).plus(out.scale(sh));
                segs.push(Segment::MoveTo(p.plus(ub.scale(-off))));
                segs.push(Segment::LineTo(p.plus(ub.scale(len - off))));
            }
            if upper {
                let p = on_axis(f, a, t, f.max[b]).plus(ub.scale(sh));
                segs.push(Segment::MoveTo(p.plus(ub.scale(off))));
                segs.push(Segment::LineTo(p.plus(ub.scale(off - len))));
            }
        }
        e.draw_with(&format!("/pgfplots/every tick,/pgfplots/every {kind} tick,/pgfplots/every {c} tick,/pgfplots/every {kind} {c} tick"), segs)?;
    }
    Ok(())
}

/// Ticks of a centered axis line that would cross the other axis line are hidden
/// (`hide obscured x ticks`); none when that line is shifted.
fn obscured(e: &mut Engine, s: &Setup, a: usize, t: f64) -> bool {
    let f = &s.frame;
    if s.line[a] != "center" || f.three_d || line_shift(e, f, 1 - a) != 0.0 {
        return false;
    }
    let tol = 0.05 * PT / f.e[a].norm().max(f64::MIN_POSITIVE);
    let near = |v: f64| (t - v).abs() <= tol;
    let (lo, hi) = (f.min[a].min(f.max[a]), f.min[a].max(f.max[a]));
    match s.line[1 - a].as_str() {
        "box" => near(lo) || near(hi),
        "low" => near(lo),
        "center" => near(f.zero[a]),
        "high" => near(hi),
        _ => false,
    }
}

/// The data scale transformation of axis `a` (`\pgfplots@set@optimal@datatrafo@for@`): `10^E` and
/// the smallest data value, which goes to 0; (1, 0) for a logarithmic axis or `disabledatascaling`.
/// Before compat 1.11, plain coordinates in an axis are in its units.
pub(super) fn data_scale_trafo(e: &mut Engine, f: &Frame, a: usize) -> (f64, f64) {
    let [lo, hi] = f.data_range[a];
    if f.log[a] || keys::flag(e, "/pgfplots/@disabledatascaling") || !(lo.is_finite() && hi.is_finite()) {
        return (1.0, 0.0);
    }
    let size = keys::length(e, if a == 0 { "/pgfplots/width" } else { "/pgfplots/height" }).unwrap_or(if a == 0 { 240.0 * PT } else { 207.0 * PT }) / PT;
    (ticks::data_scale(lo, hi, size), lo)
}

/// The axis lines: a closed box, or separate lines (with arrows) for `axis lines=…`.
fn axis_lines(e: &mut Engine, s: &Setup) -> Result<(), Error> {
    let f = &s.frame;
    let shift = [line_shift(e, f, 0), line_shift(e, f, 1)];
    if s.line[0] == "box" && s.line[1] == "box" && !keys::flag(e, "/pgfplots/separate axis lines") && shift == [0.0, 0.0] {
        if s.hide[0] && s.hide[1] {
            return Ok(());
        }
        let p = |x: f64, y: f64| f.at([x, y, f.min[2]]);
        let (x0, x1, y0, y1) = (f.min[0], f.max[0], f.min[1], f.max[1]);
        let mut segs = vec![Segment::MoveTo(p(x0, y0))];
        let mut step = |hidden: bool, q: Point| segs.push(if hidden { Segment::MoveTo(q) } else { Segment::LineTo(q) });
        step(s.hide[1], p(x0, y1));
        step(s.hide[0], p(x1, y1));
        step(s.hide[1], p(x1, y0));
        step(s.hide[0], p(x0, y0));
        if !s.hide[0] && !s.hide[1] {
            segs.push(Segment::Close);
        }
        return e.draw_with("/pgfplots/every outer x axis line,/pgfplots/every outer y axis line", segs);
    }
    for (a, &sa) in shift.iter().enumerate() {
        if s.hide[a] {
            continue;
        }
        let c = axis_char(a);
        let b = 1 - a;
        let discontinuity = e.pv(&format!("axis {c} discontinuity"));
        // shifted outwards: away from the other axis' middle, along the normal for a centered line
        let ub = unit(f.e[b]).scale(if f.max[b] >= f.min[b] { 1.0 } else { -1.0 });
        let line = |bv: f64| {
            let out = if bv == f.max[b] {
                ub
            } else if bv == f.min[b] {
                ub.scale(-1.0)
            } else {
                f.normal(a)
            };
            let d = out.scale(sa);
            let (p0, p1) = (on_axis(f, a, f.min[a], bv).plus(d), on_axis(f, a, f.max[a], bv).plus(d));
            discontinuous(p0, p1, &discontinuity)
        };
        match s.line[a].as_str() {
            "center" => e.draw_with(&format!("/pgfplots/every inner {c} axis line"), line(f.zero[b]))?,
            "box" => {
                e.draw_with(&format!("/pgfplots/every outer {c} axis line"), line(f.min[b]))?;
                e.draw_with(&format!("/pgfplots/every outer {c} axis line"), line(f.max[b]))?;
            }
            "low" => e.draw_with(&format!("/pgfplots/every outer {c} axis line"), line(if f.label_side[a] > 0 { f.max[b] } else { f.min[b] }))?,
            "high" => e.draw_with(&format!("/pgfplots/every outer {c} axis line"), line(f.max[b]))?,
            _ => {}
        }
    }
    Ok(())
}

/// An axis line from `p0` to `p1`, with `axis x discontinuity` near its start: a zigzag of 12pt
/// (`crunch`) or a 4pt gap between two bars (`parallel`), 4pt from the start.
fn discontinuous(p0: Point, p1: Point, kind: &str) -> Vec<Segment> {
    let u = unit(p1.minus(p0));
    let n = Point::new(-u.y, u.x);
    let at = |s: f64, h: f64| p0.plus(u.scale(s * PT)).plus(n.scale(h * PT));
    match kind {
        "crunch" => vec![
            Segment::MoveTo(p0),
            Segment::LineTo(at(4.0, 0.0)),
            Segment::LineTo(at(7.0, 4.0)),
            Segment::LineTo(at(13.0, -4.0)),
            Segment::LineTo(at(16.0, 0.0)),
            Segment::LineTo(p1),
        ],
        "parallel" => vec![
            Segment::MoveTo(p0),
            Segment::LineTo(at(4.0, 0.0)),
            Segment::MoveTo(at(4.0, -8.0)),
            Segment::LineTo(at(4.0, 8.0)),
            Segment::MoveTo(at(8.0, -8.0)),
            Segment::LineTo(at(8.0, 8.0)),
            Segment::MoveTo(at(8.0, 0.0)),
            Segment::LineTo(p1),
        ],
        _ => vec![Segment::MoveTo(p0), Segment::LineTo(p1)],
    }
}

/// Replaces `\pgfmathprintnumber[options]{x}` in `tokens` by the number as printed.
pub fn print_numbers(e: &mut Engine, tokens: &[Token]) -> Result<Vec<Token>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    // inside `$…$`: a number printed outside is put in math, as `\pgfmathprintnumber` does
    let mut math = false;
    while i < tokens.len() {
        if matches!(tokens[i], Token::Char(_, tex::Cat::Math)) {
            math = !math;
        }
        if tokens[i].is_cs("pgfplotspointplotattime") {
            i = super::attime::run(e, tokens, i)?;
            continue;
        }
        // `\pgfplotspointgetcoordinates{(point)}`: the point's axis coordinates as `/data point/x`…
        if tokens[i].is_cs("pgfplotspointgetcoordinates") {
            // (no space after the command: TeX skips those when it reads it)
            let (arg, end) = group_at(tokens, i + 1);
            i = end;
            let text = e.tex.expand_to_string(arg)?;
            let inner = text.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or(text.trim()).to_string();
            let (Some(f), Ok(c)) = (e.frame.clone(), e.coordinate(&inner)) else { continue };
            if let Some([x, y]) = f.data_at(c.p) {
                super::keys::set(e, "/data point/x", &super::super::pgf_number(x));
                super::keys::set(e, "/data point/y", &super::super::pgf_number(y));
            }
            continue;
        }
        if tokens[i].is_cs("pgfmathprintnumber") {
            // (no space after the command: TeX skips those when it reads it)
            let mut j = i + 1;
            let mut opts = Vec::new();
            if j < tokens.len() && tokens[j].is_char('[') {
                let k = tokens[j..].iter().position(|t| t.is_char(']')).map_or(tokens.len(), |p| j + p);
                opts = tokens[j + 1..k.min(tokens.len())].to_vec();
                j = k + 1;
            }
            while j < tokens.len() && tokens[j].is_space() {
                j += 1;
            }
            let (mut arg, mut end) = group_at(tokens, j);
            // an expanded `\pgfmathprintnumber\tick`: the number's characters, unbraced
            let numeric = |t: &Token, first: bool| match t {
                Token::Char(c, tex::Cat::Other | tex::Cat::Letter) => c.is_ascii_digit() || matches!(c, '-' | '+' | '.') || (!first && matches!(c, 'e' | 'E')),
                _ => false,
            };
            if arg.len() == 1 && numeric(&arg[0], true) && numeric(&tokens[j], true) {
                let mut k = j;
                while k < tokens.len() && numeric(&tokens[k], k == j) {
                    k += 1;
                }
                // the longest prefix that is a number (`1--4` is 1, then `--4`)
                while k > j + 1 && tex::tokens_to_string(&tokens[j..k]).trim().parse::<f64>().is_err() {
                    k -= 1;
                }
                arg = tokens[j..k].to_vec();
                end = k;
            }
            let saved = e.st.number_format.clone();
            if !opts.is_empty() {
                e.process_keys(opts, Some("/pgf/number format"))?;
            }
            let v = e.number_of(arg).unwrap_or(f64::NAN);
            let s = e.st.number_format.print(v);
            e.st.number_format = saved;
            if math {
                out.extend(tex::string_tokens(&s));
            } else {
                out.extend(toks(&format!("${s}$")));
            }
            i = end;
            continue;
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    Ok(out)
}

/// The argument at `j`: a group's content or one token; and the index after it.
fn group_at(tokens: &[Token], j: usize) -> (Vec<Token>, usize) {
    if j >= tokens.len() {
        return (Vec::new(), j);
    }
    if matches!(tokens[j], Token::Char(_, crate::tex::Cat::Begin)) {
        // the group's end (token lists are balanced; one that is not ends with the list)
        let mut depth = 0;
        let end = tokens[j..]
            .iter()
            .position(|t| {
                match t {
                    Token::Char(_, crate::tex::Cat::Begin) => depth += 1,
                    Token::Char(_, crate::tex::Cat::End) => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map_or(tokens.len(), |k| j + k);
        return (tokens[j + 1..end].to_vec(), (end + 1).min(tokens.len()));
    }
    (vec![tokens[j].clone()], j + 1)
}

/// The text of tick `t` of axis `a`, the `k`-th.
pub(super) fn tick_text_for(e: &mut Engine, a: usize, t: f64, k: usize) -> Result<Vec<Token>, Error> {
    tick_text(e, a, t, None, k, 1.0, false)
}

/// The text of tick `t` (index `k` in the list) of axis `a`.
fn tick_text(e: &mut Engine, a: usize, t: f64, next: Option<f64>, k: usize, factor: f64, extra: bool) -> Result<Vec<Token>, Error> {
    let f = e.frame.clone().expect("drawing an axis");
    let c = axis_char(a);
    // groupplots' inner plots: the labels are there, empty
    if !extra && keys::flag(e, &format!("/pgfplots/@empty {c}ticklabels")) {
        return Ok(Vec::new());
    }
    let labels_key = if extra { format!("/pgfplots/extra {c} tick labels") } else { format!("/pgfplots/{c}ticklabels") };
    if let Some(list) = pk::value(&e.tex, &labels_key).filter(|l| !pk::trim(l).is_empty()) {
        let items = split_labels(&list);
        return Ok(items.get(k).cloned().unwrap_or_default());
    }
    let shown = |v: f64| if f.log[a] { v } else { v * factor };
    let label_key = if extra { format!("/pgfplots/extra {c} tick label") } else { format!("/pgfplots/{c}ticklabel") };
    if let Some(code) = keys::value_tokens(&*e, &label_key) {
        e.begin_group();
        let value = if f.log[a] { t } else { shown(t) };
        let tick = super::super::pgf_number(value);
        e.tex.set_meaning(&Token::cs("tick"), super::super::macro_from(&tick), false);
        e.tex.set_meaning(&Token::cs("ticknum"), super::super::macro_from(&k.to_string()), false);
        if super::dateplot::is_date(e, c) {
            super::dateplot::define_tick(e, value);
        }
        if let Some(n) = next {
            e.tex.set_meaning(&Token::cs("nexttick"), super::super::macro_from(&super::super::pgf_number(shown(n))), false);
        }
        let r = e.tex.expand_fully(code).and_then(|x| print_numbers(e, &x));
        e.end_group();
        return r;
    }
    if f.log[a] {
        let basis = keys::number(e, &format!("/pgfplots/log basis {c}"));
        let (base_text, exp) = match basis {
            Some(b) if b > 0.0 && b != 10.0 => (super::super::pgf_number(b), t / b.ln()),
            _ => ("10".to_string(), t / std::f64::consts::LN_10),
        };
        let fmt = NumberFormat { style: super::number::Style::Fixed, precision: 2, ..e.st.number_format.clone() };
        // `log identify minor tick positions` (extra ticks): 5·10^-6 rather than 10^-5.3
        if base_text == "10" && keys::flag(e, "/pgfplots/log identify minor tick positions") {
            let j = exp.floor();
            let m = 10f64.powf(exp - j);
            let i = m.round();
            if (exp - exp.round()).abs() > 1e-4 && (2.0..=9.0).contains(&i) && (m - i).abs() < 1e-3 * i {
                let sci = NumberFormat { style: super::number::Style::Sci, ..e.st.number_format.clone() };
                return Ok(tex::string_tokens(&format!("${}$", sci.print(i * 10f64.powf(j)))));
            }
        }
        if keys::flag(e, "/pgfplots/@log fixed point") {
            let v = (exp * (base_text.parse::<f64>().unwrap_or(10.0)).ln()).exp();
            let ff = NumberFormat { style: super::number::Style::FixedRelative, precision: 3, ..e.st.number_format.clone() };
            return Ok(tex::string_tokens(&format!("${}$", ff.print(v))));
        }
        return Ok(tex::string_tokens(&format!("${base_text}^{{{}}}$", fmt.print(exp))));
    }
    if let Some(name) = super::symbolic::label(e, c, t) {
        return Ok(super::super::tikra_tokens(&name));
    }
    // `scaled y ticks=manual:{label}{code}`: the code maps the tick (#1) to \pgfmathresult
    if let Some((_, code)) = manual_scale(e, c) {
        let body = tex::substitute(&tex::parse_body(code), &[toks(&super::super::pgf_number(t))]);
        e.begin_group();
        let r = e.run_tokens(body).and_then(|_| e.tex.expand_to_string(vec![Token::cs("pgfmathresult")]));
        let v = r.ok().and_then(|s| e.number(s.trim()).ok());
        e.end_group();
        if let Some(v) = v {
            return Ok(tex::string_tokens(&format!("${}$", e.st.number_format.print(v))));
        }
    }
    Ok(tex::string_tokens(&format!("${}$", e.st.number_format.print(shown(t)))))
}

/// `scaled x ticks=manual:{label}{code}`: the label and the code.
fn manual_scale(e: &Engine, c: char) -> Option<(Vec<Token>, Vec<Token>)> {
    let v = pk::value(&e.tex, &format!("/pgfplots/scaled {c} ticks"))?;
    let text = tex::tokens_to_string(&v);
    let at = text.find("manual:")?;
    let _ = at;
    // the two groups after `manual:`
    let start = v.iter().position(|t| t.is_char(':'))? + 1;
    let mut groups = Vec::new();
    let mut depth = 0usize;
    let mut cur = Vec::new();
    for t in &v[start..] {
        match t {
            Token::Char(_, tex::Cat::Begin) => {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            }
            Token::Char(_, tex::Cat::End) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    groups.push(std::mem::take(&mut cur));
                    continue;
                }
            }
            _ => {}
        }
        if depth > 0 {
            cur.push(t.clone());
        }
    }
    let mut it = groups.into_iter();
    Some((it.next()?, it.next()?))
}

fn split_labels(list: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0usize;
    for t in list {
        match t {
            Token::Char(_, crate::tex::Cat::Begin) => depth += 1,
            Token::Char(_, crate::tex::Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(',', _) if depth == 0 => {
                out.push(Vec::new());
                continue;
            }
            _ => {}
        }
        if let Some(l) = out.last_mut() {
            l.push(t.clone());
        }
    }
    out.into_iter().map(|v| tex::strip_outer_braces(pk::trim(&v).to_vec())).collect()
}

/// The tick labels of axis `a`, and the scale label (`\cdot 10^{4}`).
fn tick_labels(e: &mut Engine, s: &Setup, a: usize, list: &[f64], extra: bool) -> Result<(), Error> {
    let f = s.frame.clone();
    let c = axis_char(a);
    if !keys::flag(e, &format!("/pgfplots/{c}majorticks")) {
        return Ok(());
    }
    let b = 1 - a;
    let off = tick_offset(e, a);
    let n = f.normal(a);
    let side = f.label_side[a];
    let bv = if side > 0 {
        f.max[b]
    } else if side == 0 {
        f.zero[b]
    } else {
        f.min[b]
    };
    let sh = n.scale(line_shift(e, &f, a));
    tick_labels_at(e, s, a, list, extra, &|t| on_axis(&f, a, t, bv).plus(sh), n.scale(off), off)
}

/// `axis x line shift=10pt`: how far axis `a`'s line, ticks and tick labels move outwards; a
/// number without a unit is in units of the other axis (`\pgfplots@init@axis@shift`).
pub(super) fn line_shift(e: &mut Engine, f: &Frame, a: usize) -> f64 {
    let v = keys::value(e, &format!("/pgfplots/axis {} line shift", axis_char(a)));
    let v = v.trim();
    if v.is_empty() {
        return 0.0;
    }
    if let Ok(n) = v.parse::<f64>() {
        return if f.three_d { 0.0 } else { n * f.e[1 - a].norm() };
    }
    e.length(v).unwrap_or(0.0)
}

/// How far tick labels move out for the tick lines (`tick align`).
pub(super) fn tick_offset(e: &mut Engine, a: usize) -> f64 {
    let align = e.pv(&format!("{}tick align", axis_char(a)));
    let len = keys::length(e, "/pgfplots/major tick length").unwrap_or(0.15);
    match align.as_str() {
        "outside" => len,
        "center" => len / 2.0,
        _ => 0.0,
    }
}

/// The tick labels of axis `a` at `base(t)` moved by `shift`, and the scale label; `off` is how
/// far the labels are from the axis line at least.
#[allow(clippy::too_many_arguments)]
pub(super) fn tick_labels_at(e: &mut Engine, s: &Setup, a: usize, list: &[f64], extra: bool, base: &dyn Fn(f64) -> Point, shift: Point, off: f64) -> Result<(), Error> {
    let f = s.frame.clone();
    let c = axis_char(a);
    // scaled ticks: the labels divided by a power of ten
    // scaled ticks: the labels multiplied by `factor`, and a label that says by what
    let mut factor = 1.0;
    let mut scale_label: Option<String> = None;
    if !f.log[a] && !extra {
        let v = e.pv(&format!("scaled {c} ticks"));
        let above = keys::number(e, "/pgfplots/scale ticks above exponent").unwrap_or(3.0) as i32;
        let below = keys::number(e, "/pgfplots/scale ticks below exponent").unwrap_or(-1.0) as i32;
        let exponent = match v.as_str() {
            "true" => Some(ticks::scale_exponent(f.min[a], f.max[a], above, below)),
            "false" => None,
            other => other.strip_prefix("base 10:").and_then(|n| n.trim().parse().ok()),
        };
        let binop = keys::value_tokens(e, "/pgfplots/tick scale binop").map_or_else(|| "\\cdot".to_string(), |t| tex::tokens_to_string(&t));
        // `xtick scale label code/.code={…}`: the label, given the exponent (or the factor)
        let code = pk::macro_name(&format!("/pgfplots/{c}tick scale label code/.@cmd"));
        let custom = matches!(e.tex.meaning_of(&code), tex::Meaning::Macro(_));
        let label_of = |e: &mut Engine, arg: String, default: String| -> String {
            if !custom {
                return default;
            }
            let mut call = vec![Token::cs(&code), Token::Char('{', tex::Cat::Begin)];
            call.extend(toks(&arg));
            call.push(Token::Char('}', tex::Cat::End));
            e.tex.expand_fully(call).map(|t| tex::tokens_to_string(&t)).unwrap_or(default)
        };
        if let Some((label, _)) = manual_scale(e, c) {
            scale_label = Some(tex::tokens_to_string(&label));
        }
        if let Some(x) = exponent.filter(|x| *x != 0) {
            factor = 10f64.powi(x);
            scale_label = Some(label_of(e, (-x).to_string(), format!("${binop} 10^{{{}}}$", -x)));
        }
        if let Some(r) = v.strip_prefix("real:") {
            if let Ok(d) = e.number(r) {
                if d != 0.0 {
                    factor = 1.0 / d;
                    let default = format!("${binop} {}$", e.st.number_format.print(d));
                    scale_label = Some(label_of(e, super::super::pgf_number(d), default));
                }
            }
        }
    }
    let interval = keys::flag(e, &format!("/pgfplots/{c} tick label as interval"));
    let n = f.normal(a);
    let anchor = format!("near {c}ticklabel");
    let styles = format!("/pgfplots/every tick label,/pgfplots/every {c} tick label");
    let mut dim = 0.0f64;
    let vis = visible(&f, a, list);
    let mut entries: Vec<(usize, f64, Option<f64>, Point)> = Vec::new();
    if interval {
        for w in vis.windows(2) {
            let (k, t) = w[0];
            let (_, t2) = w[1];
            entries.push((k, t, Some(t2), base((t + t2) / 2.0)));
        }
    } else {
        for (k, t) in &vis {
            if obscured(e, s, a, *t) {
                continue;
            }
            let next = list.get(k + 1).copied();
            entries.push((*k, *t, next, base(*t)));
        }
    }
    f.label_axis.set(a);
    for (k, t, next, p) in entries {
        let at = p.plus(shift);
        // the number format of the tick label styles applies to the text
        e.begin_group();
        let r = (|| {
            e.keys_tikz(&styles)?;
            let (t, next) = if f.ternary { (super::ternary::shown(&f, a, t), next.map(|n| super::ternary::shown(&f, a, n))) } else { (t, next) };
            tick_text(e, a, t, next, k, factor, extra)
        })();
        e.end_group();
        let mut text = r?;
        // `typeset ticklabels with strut`: every label as high and deep as a strut
        if keys::flag(e, "/pgfplots/@ticklabel strut") {
            let mut strut = toks("\\vrule width 0pt height 0.84em depth 0.36em");
            strut.extend(text);
            text = strut;
        }
        let options = toks(&format!("anchor={anchor},{styles}"));
        // `\ticknum` in the label's options too (`name=tick no \ticknum`)
        e.begin_group();
        e.tex.set_meaning(&Token::cs("ticknum"), super::super::macro_from(&k.to_string()), false);
        let r = e.capture(|e| e.node(NodeSpec { options, name: None, at: Some(at), text, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None }));
        e.end_group();
        let (items, _, bbox) = r?;
        if let Some((lo, hi)) = bbox {
            let corner = Point::new(if n.x > 0.0 { hi.x } else { lo.x }, if n.y > 0.0 { hi.y } else { lo.y });
            let d = (corner.x - p.x) * n.x + (corner.y - p.y) * n.y;
            dim = dim.max(d);
            e.canvas.include(lo);
            e.canvas.include(hi);
        }
        e.canvas.items.extend(items);
    }
    if !extra {
        let mut td = f.tick_dim.borrow_mut();
        td[a] = td[a].max(dim.max(off));
    }
    if let Some(label) = scale_label.filter(|_| !vis.is_empty() && !keys::flag(e, &format!("/pgfplots/@{c} scale label off"))) {
        let text = tex::string_tokens(&label);
        let options = toks(&format!("/pgfplots/every tick label,/pgfplots/every {c} tick label,/pgfplots/every {c} tick scale label"));
        f.cs.set(Cs::Description);
        let p = f.desc(0.0, 0.0);
        let items = e.node(NodeSpec { options, name: None, at: None, text, place: OnPath::Point(p), point: p, coordinate: false, default_pos: None });
        f.cs.set(Cs::Plain);
        e.canvas.items.extend(items?);
    }
    Ok(())
}

/// Labels, title and legend.
fn descriptions(e: &mut Engine, axis: &Axis, s: &Setup) -> Result<(), Error> {
    let f = s.frame.clone();
    let labels: &[(usize, &str)] = if f.three_d { &[(0, "x"), (1, "y"), (2, "z")] } else { &[(0, "x"), (1, "y")] };
    for &(a, c) in labels {
        // an empty label is still a node in TeX's drawing: it takes room at its place
        let mut text = keys::value_tokens(&*e, &format!("/pgfplots/{c}label")).unwrap_or_default();
        if s.hide[a] {
            continue;
        }
        // (but not in a colorbar, which TeX's drawing shows without them)
        if pk::trim(&text).is_empty() && keys::flag(e, "/pgfplots/@colorbar axis") {
            continue;
        }
        // the units library: ` [km]` after the label (`x unit=m`, `x unit prefix=k`)
        let unit = keys::value(e, &format!("/pgfplots/{c} unit"));
        if !unit.trim().is_empty() && keys::flag(e, "/pgfplots/use units") {
            let prefix = keys::value(e, &format!("/pgfplots/{c} unit prefix"));
            text.extend(tex::string_tokens(" "));
            text.extend(toks(&format!("$\\left[\\mathrm{{{prefix}{unit}}}\\right]$")));
        }
        f.label_axis.set(a);
        let p = f.desc(0.0, 0.0);
        let options = toks(&format!("/pgfplots/every axis label,/pgfplots/every axis {c} label"));
        let text = print_numbers(e, &text)?;
        // `sloped` in a label is `sloped like x axis`
        e.begin_group();
        let slope = e.tex.meaning_of(&pk::macro_name(&format!("/tikz/sloped like {c} axis/.@cmd")));
        e.tex.set_meaning(&Token::cs(&pk::macro_name("/tikz/sloped/.@cmd")), slope, false);
        let items = e.node(NodeSpec { options, name: None, at: None, text, place: OnPath::Point(p), point: p, coordinate: false, default_pos: None });
        e.end_group();
        e.canvas.items.extend(items?);
    }
    if let Some(text) = keys::value_tokens(&*e, "/pgfplots/title") {
        let p = f.desc(0.0, 0.0);
        let text = print_numbers(e, &text)?;
        let items =
            e.node(NodeSpec { options: toks("/pgfplots/every axis title"), name: None, at: None, text, place: OnPath::Point(p), point: p, coordinate: false, default_pos: None })?;
        e.canvas.items.extend(items);
    }
    super::legend::legend(e, axis, &f)?;
    // `extra description/.code`, in description coordinates too
    e.begin_group();
    let r = e.process_keys(toks("/pgfplots/extra description"), None);
    e.end_group();
    r?;
    // `after end axis/.code`: plain coordinates again, nothing clipped
    let cs = f.cs.get();
    f.cs.set(if keys::substitute_pgfpoint(e) { Cs::Axis } else { Cs::Plain });
    e.begin_group();
    let r = in_plain_units(e, &f, |e| e.process_keys(toks("/pgfplots/after end axis"), None));
    e.end_group();
    f.cs.set(cs);
    r?;
    Ok(())
}

/// A filled or stroked path for a solid color (used by legend frames).
pub fn solid(color: crate::color::Color, opacity: f64) -> Fill {
    Fill { paint: Paint::Solid(color), opacity, even_odd: false }
}

/// Runs `f` with TikZ's unit vectors those of the data scale transformation (so that a radius of
/// 1 is one of its units), when plain coordinates are its coordinates.
fn in_plain_units<T>(e: &mut Engine, f: &Frame, run: impl FnOnce(&mut Engine) -> Result<T, Error>) -> Result<T, Error> {
    let units = (e.st.x, e.st.y, e.st.z);
    if f.cs.get() == Cs::Plain && !f.polar && !f.smith && !f.ternary {
        let inverse = e.st.transform.inverse();
        let v: Vec<Point> = (0..3).map(|a| inverse.linear(f.e[a].scale(1.0 / data_scale_trafo(e, f, a).0))).collect();
        (e.st.x, e.st.y) = (v[0], v[1]);
        if f.three_d {
            e.st.z = v[2];
        }
    }
    let r = run(e);
    (e.st.x, e.st.y, e.st.z) = units;
    r
}

/// `axis cs`, `rel axis cs`, `axis description cs`, `ticklabel cs`: points of the axis being
/// drawn, for TikZ statements inside it.
impl Engine {
    pub fn pgfplots_cs(&mut self, system: &str, args: &str) -> Option<Result<TikzCoord, Error>> {
        let f = self.frame.clone()?;
        let parts = crate::keys::split_top_level(args);
        let named = |k: &str| parts.iter().find_map(|p| p.split_once('=').filter(|(a, _)| a.trim() == k).map(|(_, b)| b.trim().to_string()));
        let positional = parts.iter().all(|p| !p.contains('='));
        let value = |e: &mut Engine, i: usize, k: &str| -> Result<Option<f64>, Error> {
            let t = if positional { parts.get(i).cloned() } else { named(k) };
            match t {
                // a name of a symbolic axis, or a date
                Some(t) if !t.trim().is_empty() && i < 3 && super::symbolic::index(e, axis_char(i), &t).is_some() => Ok(super::symbolic::index(e, axis_char(i), &t)),
                Some(t) if !t.trim().is_empty() => Ok(Some(e.number(crate::keys::strip_braces(t.trim()))?)),
                _ => Ok(None),
            }
        };
        let r = (|| -> Result<Point, Error> {
            match system {
                "cartesian" if f.ternary => {
                    let x = value(self, 0, "x")?.unwrap_or(0.0);
                    let y = value(self, 1, "y")?.unwrap_or(0.0);
                    Ok(super::ternary::cartesian(&f, x, y))
                }
                "cartesian" if f.smith => {
                    let x = value(self, 0, "x")?.unwrap_or(0.0);
                    let y = value(self, 1, "y")?.unwrap_or(0.0);
                    Ok(super::smith::canvas(&f, x, y))
                }
                "axis" => {
                    let x = value(self, 0, "x")?.unwrap_or(0.0);
                    let y = value(self, 1, "y")?.unwrap_or(0.0);
                    // a ternary axis deduces a missing z
                    let z = value(self, 2, "z")?.unwrap_or(if f.ternary { f64::NAN } else { 0.0 });
                    let l = [f.logical(0, x), f.logical(1, y), if f.three_d { f.logical(2, z) } else { f.min[2] }];
                    Ok(f.at(l))
                }
                "rel axis" => {
                    let x = value(self, 0, "x")?.unwrap_or(0.0);
                    let y = value(self, 1, "y")?.unwrap_or(0.0);
                    let z = value(self, 2, "z")?.unwrap_or(0.0);
                    Ok(f.rel([x, y, z]))
                }
                "axis description" => {
                    let x = value(self, 0, "x")?.unwrap_or(0.0);
                    let y = value(self, 1, "y")?.unwrap_or(0.0);
                    Ok(f.desc(x, y))
                }
                "boxplot" | "boxplot box" | "boxplot whisker" => {
                    let parts = crate::keys::split_top_level(args);
                    let a = parts.first().map(|p| self.number(p.trim())).transpose()?.unwrap_or(0.0);
                    let b = parts.get(1).map(|p| self.number(p.trim())).transpose()?.unwrap_or(0.0);
                    match super::boxplot::cs(system, a, b) {
                        Some([x, y]) => Ok(f.at([x, y, f.min[2]])),
                        None => Err(Error::Figure(format!("{system} cs outside a box plot"))),
                    }
                }
                "axis direction" => {
                    let x = value(self, 0, "x")?.unwrap_or(0.0);
                    let y = value(self, 1, "y")?.unwrap_or(0.0);
                    let z = value(self, 2, "z")?.unwrap_or(0.0);
                    let o = f.at(f.min);
                    let l = [f.min[0] + x, f.min[1] + y, f.min[2] + z];
                    Ok(f.at(l).minus(o))
                }
                "ticklabel" | "ticklabel*" | "xticklabel" | "yticklabel" | "zticklabel" | "xticklabel*" | "yticklabel*" | "zticklabel*" => {
                    let a = match system.trim_end_matches('*') {
                        "xticklabel" => 0,
                        "yticklabel" => 1,
                        "zticklabel" => 2,
                        _ => f.label_axis.get(),
                    };
                    let t = parts.first().map(|p| self.number(p)).transpose()?.unwrap_or(0.5);
                    let extra = match parts.get(1) {
                        Some(p) => self.length(p)?,
                        None => 0.0,
                    };
                    let base = if f.ternary {
                        super::ternary::edge(&f, a, t)
                    } else if f.three_d {
                        // a point of the line that holds the tick labels
                        let line = f.label_line[a];
                        let l = [0, 1, 2].map(|k| match line[k] {
                            b'v' => f.min[k] + t * (f.max[k] - f.min[k]),
                            b'1' => f.max[k],
                            b'2' => f.zero[k],
                            _ => f.min[k],
                        });
                        f.at(l)
                    } else {
                        let b = if a == 0 { 1 } else { 0 };
                        let side = f.label_side[a];
                        let bv = if side > 0 {
                            f.max[b]
                        } else if side == 0 {
                            f.zero[b]
                        } else {
                            f.min[b]
                        };
                        let mut l = f.min;
                        l[a] = f.min[a] + t * (f.max[a] - f.min[a]);
                        l[b] = bv;
                        f.at(l)
                    };
                    let shift = if system.ends_with('*') { extra } else { f.tick_dim.borrow()[a] + extra + f.label_shift[a] };
                    Ok(base.plus(f.normal(a).scale(shift)))
                }
                _ => Err(Error::Figure(String::new())),
            }
        })();
        match r {
            Ok(p) => Some(Ok(TikzCoord { p, node: None })),
            Err(err) if err.to_string() == "figure: " => None,
            Err(err) => Some(Err(err)),
        }
    }

    /// A plain `(x,y)` in an axis: `axis cs` (compat 1.11 on) or the description system.
    pub fn pgfplots_plain(&mut self, parts: &[String]) -> Option<Result<Point, Error>> {
        let f = self.frame.clone()?;
        match f.cs.get() {
            // plain coordinates of a ternary axis are cartesian
            Cs::Plain if f.ternary => {
                let r = (|| -> Result<Point, Error> {
                    let x = self.number(crate::keys::strip_braces(parts.first().map_or("0", |p| p.trim())))?;
                    let y = self.number(crate::keys::strip_braces(parts.get(1).map_or("0", |p| p.trim())))?;
                    Ok(super::ternary::cartesian(&f, x, y))
                })();
                Some(r)
            }
            // a coordinate with units (`(1cm,0pt)`) stays TikZ's
            Cs::Plain if parts.iter().any(|p| has_unit(p)) => None,
            // before compat 1.11, the coordinates of the data scale transformation: 10^E times the
            // data, less the smallest data value (`\pgfplots@set@optimal@datatrafo@for@`)
            Cs::Plain if !f.polar && !f.smith => {
                let r = (|| -> Result<Point, Error> {
                    let mut l = [f.min[0], f.min[1], f.min[2]];
                    for (a, la) in l.iter_mut().enumerate().take(if f.three_d { 3 } else { 2 }) {
                        // an empty coordinate (`\pgfkeysvalueof{/pgfplots/xmin}` unset) is 0, as TeX recovers
                        let v = match crate::keys::strip_braces(parts.get(a).map_or("0", |p| p.trim())) {
                            "" => 0.0,
                            p => self.number(p)?,
                        };
                        let (factor, shift) = data_scale_trafo(self, &f, a);
                        *la = shift + v / factor;
                    }
                    Ok(f.at(l))
                })();
                Some(r)
            }
            Cs::Plain => None,
            cs => {
                let r = (|| -> Result<Point, Error> {
                    let mut v = [0.0; 3];
                    for (i, p) in parts.iter().enumerate().take(3) {
                        v[i] = self.number(crate::keys::strip_braces(p.trim()))?;
                    }
                    Ok(if cs == Cs::Axis { f.at([f.logical(0, v[0]), f.logical(1, v[1]), if f.three_d { f.logical(2, v[2]) } else { f.min[2] }]) } else { f.desc(v[0], v[1]) })
                })();
                Some(r)
            }
        }
    }
}
