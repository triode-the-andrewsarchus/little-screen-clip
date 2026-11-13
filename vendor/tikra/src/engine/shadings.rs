//! The basic layer's shadings: `\pgfdeclarehorizontalshading{name}{height}{color(0bp)=(red); …}`,
//! `\pgfdeclareverticalshading`, `\pgfdeclareradialshading{name}{center}{…}`; `\pgfuseshading{name}`
//! draws one centered at the origin, `\pgfshadepath{name}{angle}` fills the path being built with it
//! fitted to the path's box as pgf fits its 100 bp shadings, and TikZ's `shading=name` uses them.

use crate::canvas::{Fill, Item, Paint, Shading};
use crate::color::Color;
use crate::scene::{Point, Segment, Transform};
use crate::Error;

use super::Engine;

/// Centimeters per bp.
const BP: f64 = 2.54 / 72.0;

/// A declared shading: along x, along y, or out from a center; its stops at their positions (bp);
/// for the axial ones the size across (bp), for the radial one its center. A shading declared with
/// the colors it depends on (`[mycolor]`) keeps its stops' colors as written, to take them when used.
#[derive(Debug, Clone)]
pub struct BasicShading {
    kind: char,
    stops: Vec<(f64, Color)>,
    across: f64,
    center: Point,
    colors: Option<Vec<String>>,
}

impl BasicShading {
    fn length(&self) -> (f64, f64) {
        let first = self.stops.first().map_or(0.0, |s| s.0);
        let last = self.stops.last().map_or(0.0, |s| s.0);
        (first, (last - first).max(1e-9))
    }

    /// The shading in the canvas, its box centered at the origin of `t`.
    fn paint(&self, t: &Transform) -> Shading {
        let (first, len) = self.length();
        let stops: Vec<(f64, Color)> = self.stops.iter().map(|(p, c)| ((p - first) / len, *c)).collect();
        match self.kind {
            'h' => Shading::Axial { transform: t.compose(&Transform::rotation(-90.0)).compose(&Transform::scaling(self.across * BP / 100.0, len * BP / 100.0)), stops },
            'v' => Shading::Axial { transform: t.compose(&Transform::scaling(self.across * BP / 100.0, len * BP / 100.0)), stops },
            _ => {
                // radii from 0 to the last; the inner circle at the center
                let r = self.stops.last().map_or(1.0, |s| s.0).max(1e-9);
                let k = r * BP / 50.0;
                let stops = self.stops.iter().map(|(p, c)| (p / r, *c)).collect();
                Shading::Radial { transform: t.compose(&Transform::scaling(k, k)), focus: Point::new(self.center.x / (r / 50.0), self.center.y / (r / 50.0)), stops }
            }
        }
    }

    /// The shading's box, centered at the origin (cm).
    fn half_size(&self) -> (f64, f64) {
        let (_, len) = self.length();
        match self.kind {
            'h' => (len * BP / 2.0, self.across * BP / 2.0),
            'v' => (self.across * BP / 2.0, len * BP / 2.0),
            _ => {
                let r = self.stops.last().map_or(0.0, |s| s.0) * BP;
                (r, r)
            }
        }
    }
}

impl Engine {
    /// `\pgfdeclare(horizontal|vertical|radial)shading[colors]{name}{size or center}{stops}`.
    pub fn declare_shading(&mut self, kind: char) -> Result<(), Error> {
        let depends = self.optional()?.is_some();
        let name = self.argument_string()?;
        let size = self.argument()?;
        let spec = self.argument_string()?;
        let (across, center) = if kind == 'r' {
            let p = self.point_of(size)?;
            (0.0, Point::new(p.x / BP, p.y / BP))
        } else {
            (self.length_of(size)? / BP, Point::default())
        };
        let mut stops = Vec::new();
        let mut written = Vec::new();
        for part in spec.split(';') {
            let part = part.trim();
            let Some((model, rest)) = part.split_once('(') else { continue };
            let Some((pos, value)) = rest.split_once(")=(") else { continue };
            let value = value.trim_end_matches(')').trim();
            let p = self.length(pos.trim())? / BP;
            let c = match model.trim() {
                "rgb" => {
                    let v: Vec<f64> = value.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    Color::rgb(v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0))
                }
                "gray" => {
                    let g = value.parse().unwrap_or(0.0);
                    Color::rgb(g, g, g)
                }
                _ => self.color(value).unwrap_or(Color::BLACK),
            };
            stops.push((p, c));
            written.push(if model.trim() == "color" { value.to_string() } else { String::new() });
        }
        let mut order: Vec<usize> = (0..stops.len()).collect();
        order.sort_by(|&a, &b| stops[a].0.total_cmp(&stops[b].0));
        let stops: Vec<(f64, Color)> = order.iter().map(|&i| stops[i]).collect();
        let colors = depends.then(|| order.iter().map(|&i| written[i].clone()).collect());
        if !stops.is_empty() {
            self.basic_shadings.insert(name.trim().to_string(), BasicShading { kind, stops, across, center, colors });
        }
        Ok(())
    }

    /// `\pgfuseshading{name}`: the shading's box, centered at the origin; outside a picture, a box
    /// of its own on the baseline.
    pub fn use_shading(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        if self.picture_depth > 0 {
            let t = self.st.transform;
            return self.draw_shading(name.trim(), &t);
        }
        let Some((w, h)) = self.shading_half_size(name.trim()) else {
            self.warn(format!("shading '{}' not declared: ignored", name.trim()));
            return Ok(());
        };
        // a box: in vertical mode (a paragraph not started) a line of its own
        let vertical = matches!(self.layout.last(), None | Some(super::Layout::Par));
        self.begin_picture(None)?;
        let r = self.draw_shading(name.trim(), &Transform::translation(w, h));
        self.end_picture();
        if vertical {
            self.layout.push(super::Layout::Par);
        }
        r
    }

    /// The declared shading `name`, its colors as they are now if it depends on them.
    fn shading_named(&self, name: &str) -> Option<BasicShading> {
        let mut sh = self.basic_shadings.get(name)?.clone();
        if let Some(colors) = &sh.colors {
            for (stop, written) in sh.stops.iter_mut().zip(colors) {
                if let Some(c) = self.color(written).filter(|_| !written.is_empty()) {
                    stop.1 = c;
                }
            }
        }
        Some(sh)
    }

    /// The shading `name` drawn in its box, centered at the origin of `t`.
    pub fn draw_shading(&mut self, name: &str, t: &Transform) -> Result<(), Error> {
        let Some(sh) = self.shading_named(name) else {
            if let Some(f) = self.functional_shading(name) {
                // its box centered at the origin
                let c = f.center();
                let place = t.compose(&Transform::scaling(BP, BP)).compose(&Transform::translation(-c.x, -c.y));
                let cells = f.cells(&place);
                if self.counts_for_size() {
                    // the cells are all paths
                    for segments in cells.iter().filter_map(|i| if let Item::Path { segments, .. } = i { Some(segments) } else { None }) {
                        self.canvas.include_path(segments);
                    }
                }
                self.canvas.items.extend(cells);
                return Ok(());
            }
            if matches!(name, "axis" | "radial" | "ball") {
                // TikZ's own shadings, in the colors set last (`top color=…`), at their 100 bp size
                let saved = std::mem::replace(&mut self.st.shading, name.to_string());
                let fit = [Segment::MoveTo(t.apply(Point::new(-25.0 * BP, -25.0 * BP))), Segment::LineTo(t.apply(Point::new(25.0 * BP, 25.0 * BP)))];
                let paint = self.shading_of(&fit);
                self.st.shading = saved;
                let corners = [Point::new(-50.0, -50.0), Point::new(50.0, -50.0), Point::new(50.0, 50.0), Point::new(-50.0, 50.0)].map(|p| t.apply(p.scale(BP)));
                let mut segs = vec![Segment::MoveTo(corners[0])];
                segs.extend(corners[1..].iter().map(|p| Segment::LineTo(*p)));
                segs.push(Segment::Close);
                if self.counts_for_size() {
                    self.canvas.include_path(&segs);
                }
                let fill = Fill { paint: Paint::Shading(paint), opacity: self.st.fill_opacity, even_odd: false };
                self.canvas.push(Item::Path { segments: segs, fill: Some(fill), stroke: None });
                return Ok(());
            }
            self.warn(format!("shading '{name}' not declared: ignored"));
            return Ok(());
        };
        let (w, h) = sh.half_size();
        let corners = [Point::new(-w, -h), Point::new(w, -h), Point::new(w, h), Point::new(-w, h)].map(|p| t.apply(p));
        let mut segs = vec![Segment::MoveTo(corners[0])];
        if sh.kind == 'r' {
            // a radial shading paints its outer circle only
            segs = vec![Segment::MoveTo(t.apply(Point::new(w, 0.0)))];
            segs.extend(super::path::arc_segments(t.apply(Point::default()), w, w, 0.0, 360.0, t));
        } else {
            segs.extend(corners[1..].iter().map(|p| Segment::LineTo(*p)));
        }
        segs.push(Segment::Close);
        if self.counts_for_size() {
            self.canvas.include_path(&segs);
        }
        let fill = Fill { paint: Paint::Shading(sh.paint(t)), opacity: self.st.fill_opacity, even_odd: false };
        self.canvas.push(Item::Path { segments: segs, fill: Some(fill), stroke: None });
        Ok(())
    }

    /// The half sizes of a declared shading's box.
    pub fn shading_half_size(&self, name: &str) -> Option<(f64, f64)> {
        self.basic_shadings.get(name).map(BasicShading::half_size).or_else(|| {
            let f = self.functional_shadings.get(name)?;
            let (lo, hi) = f.corners();
            Some(((hi.x - lo.x) * BP / 2.0, (hi.y - lo.y) * BP / 2.0))
        })
    }

    /// `\pgfshadepath{name}{angle}`: the path being built filled with the shading, fitted to it.
    pub fn shade_path(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        let angle = self.argument()?;
        let angle = self.number_of(angle)?;
        let segs = self.pgf_path.segments.clone();
        let Some(paint) = self.basic_shading_for(name.trim(), &segs, angle) else {
            self.warn(format!("shading '{}' not declared: ignored", name.trim()));
            return Ok(());
        };
        self.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint: Paint::Shading(paint), opacity: self.st.fill_opacity, even_odd: false }), stroke: None });
        Ok(())
    }

    /// A declared shading fitted to the box of `segs`, turned by `angle`, as `\pgfshadepath` does
    /// (the middle 50 bp around its center spread over the box along the turned axes).
    pub fn basic_shading_for(&self, name: &str, segs: &[Segment], angle: f64) -> Option<Shading> {
        let sh = self.shading_named(name)?;
        let mut c = crate::canvas::Canvas::default();
        c.include_path(segs);
        let (lo, hi) = (c.min?, c.max?);
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        let (s, co) = angle.to_radians().sin_cos();
        let (s, co) = (s.abs(), co.abs());
        let sx = (w * co + h * s) / (co + s) / (50.0 * BP);
        let sy = (w * s + h * co) / (co + s) / (50.0 * BP);
        let center = lo.lerp(hi, 0.5);
        let t = Transform::translation(center.x, center.y).compose(&Transform::rotation(angle)).compose(&Transform::scaling(sx, sy));
        Some(sh.paint(&t))
    }
}
