//! What happens to a path once built (`\pgfusepath`): rounded corners, shortening,
//! clipping, filling and stroking, arrow tips, `double`, and the picture's bounding box.

use crate::canvas::{Fill, Item, Paint, Stroke};
use crate::scene::{Point, Segment};
use crate::tex::Token;
use crate::Error;

use super::arrows::{Curvilinear, EndTips, TipsMode, Widths};
use super::path::{valid_segment, MAX_COORDINATE};
use super::Engine;
use crate::PT;

impl Engine {
    /// Whether what is drawn now makes the picture larger: not after `use as bounding box`, nor
    /// after a clip in the scope.
    pub fn counts_for_size(&self) -> bool {
        !self.canvas_frozen && !self.scopes.iter().any(|s| s.clipped)
    }

    /// Uses the path as the state says: clip, fill, draw, arrows.
    pub fn use_path(&mut self, segments: Vec<Segment>, command: &str) -> Result<(), Error> {
        if !segments.iter().all(valid_segment) {
            self.warn(format!("{command}: coordinate is not finite or beyond ±{MAX_COORDINATE} cm (division by zero?): path ignored"));
            return Ok(());
        }
        self.use_path_here(segments)
    }

    fn use_path_here(&mut self, segments: Vec<Segment>) -> Result<(), Error> {
        let mut segs = segments;
        // `use path=\p`: the path kept by `save path=\p` instead; `save path` keeps this one
        if let Some(Token::Cs(n)) = self.st.use_saved_path.take() {
            if let Some(saved) = self.saved_paths.get(&*n) {
                segs = saved.clone();
            }
        }
        if let Some(Token::Cs(n)) = self.st.save_path.take() {
            self.saved_paths.insert(n.to_string(), segs.clone());
        }
        if !self.st.nonlinear.is_empty() {
            segs = self.nonlinear_path(&segs)?;
        }
        if self.st.rounded != 0.0 || self.st.rounded_out != 0.0 {
            segs = round_corners(&segs, self.st.rounded, self.st.rounded_out);
        }
        let mode = self.st.mode.clone();
        // the bounding box grows as the path is built: before any shortening
        if mode.bounding_box {
            self.canvas.include_path(&segs);
            self.canvas_frozen = true;
        } else if self.counts_for_size() {
            self.canvas.include_path(&segs);
            // a stroked path adds half its line width around its own box (\pgfusepath)
            if mode.draw {
                let w = match self.st.double {
                    Some(_) => 2.0 * self.st.line_width + self.st.double_distance,
                    None => self.st.line_width,
                };
                let mut path = crate::canvas::Canvas::default();
                path.include_path(&segs);
                if let (Some(lo), Some(hi)) = (path.min, path.max) {
                    self.canvas.include(Point::new(lo.x - w / 2.0, lo.y - w / 2.0));
                    self.canvas.include(Point::new(hi.x + w / 2.0, hi.y + w / 2.0));
                }
            }
        }
        let drawn = segs.iter().any(|s| !matches!(s, Segment::MoveTo(_)));
        // a functional shading: cells clipped to the path, under its stroke
        let functional = if mode.shade && drawn && !mode.pattern { self.functional_cells(&segs) } else { None };
        // a fill without a color of its own fills with the basic layer's fill pattern, if one is set
        // (`\pgfsetfillpattern{stars}{red}` before `\filldraw`)
        let pattern = mode.pattern || (mode.fill && self.st.fill_pattern && self.st.fill_color.is_none());
        let mode = super::state::Mode { shade: mode.shade && functional.is_none(), pattern, ..mode };
        let fill = if mode.fill || mode.pattern || mode.shade {
            let paint = if mode.pattern {
                let name = self.st.pattern_name.clone().unwrap_or_else(|| "north east lines".into());
                let color = self.st.pattern_color.unwrap_or(crate::color::Color::BLACK);
                match self.pattern_paint(&name, color) {
                    Some(paint) => paint,
                    None => {
                        // TeX stops at a pattern it does not know; the driver draws north east lines
                        let base = name.split('[').next().unwrap_or_default().trim();
                        if !crate::driver::known_pattern(&name) && !self.user_patterns.contains_key(base) {
                            self.warn(format!("unknown pattern '{name}': north east lines used"));
                        }
                        Paint::Pattern { name, color }
                    }
                }
            } else if mode.shade {
                Paint::Shading(self.shading_for(&segs))
            } else {
                Paint::Solid(self.st.fill_paint_color())
            };
            Some(Fill { paint, opacity: self.st.fill_opacity, even_odd: self.st.even_odd })
        } else {
            None
        };
        // a pattern is the fill: \fill[pattern=…] paints no solid color under it unless fill= names one
        let solid_under = if mode.pattern && mode.fill && self.st.fill_color.is_some() {
            Some(Fill { paint: Paint::Solid(self.st.fill_paint_color()), opacity: self.st.fill_opacity, even_odd: self.st.even_odd })
        } else {
            None
        };
        let stroke = mode.draw.then(|| self.stroke());
        // a path fading masks this path, fitted to it
        let fading = match self.st.path_fading.clone().filter(|_| mode.fade) {
            Some(f) if self.picture_fadings.contains_key(&f) => self.picture_fading_for(&f, &segs, self.st.fit_fading),
            Some(f) => self.fading_for(&f, &segs).map(|mask| Item::Fade { mask }),
            None => None,
        };
        let faded = drawn && (fill.is_some() || stroke.is_some()) && fading.is_some();
        let blended = drawn && (fill.is_some() || stroke.is_some()) && self.st.blend.is_some();
        if blended {
            self.canvas.push(Item::Blend { mode: self.st.blend.clone().unwrap_or_default() });
        }
        if faded {
            self.canvas.push(fading.expect("checked"));
        }
        // `tips=true`/`proper`: tips on a path that is neither filled nor stroked
        let tips_only = stroke.is_none() && fill.is_none() && !mode.clip && matches!(self.st.tips_mode, TipsMode::True | TipsMode::Proper);
        // a clipping path has no tips and is not shortened; a lone point still gets them
        let tips = if !segs.is_empty() && !mode.clip && (stroke.is_some() || tips_only) { self.arrow_ends(&mut segs) } else { Tips::default() };
        if let Some(cells) = functional {
            self.canvas.push(Item::Clip { segments: segs.clone(), even_odd: self.st.even_odd });
            self.canvas.items.extend(cells);
            self.canvas.push(Item::End);
        }
        if drawn && (fill.is_some() || stroke.is_some()) {
            if let Some(under) = solid_under {
                self.canvas.push(Item::Path { segments: segs.clone(), fill: Some(under), stroke: None });
            }
            match (self.st.double, &stroke) {
                (Some(core), Some(s)) => {
                    // two lines: the outer one, then the inner one in the double color
                    let outer = Stroke { width: 2.0 * s.width + self.st.double_distance, ..s.clone() };
                    let inner = Stroke { width: self.st.double_distance, color: core, ..s.clone() };
                    self.canvas.push(Item::Path { segments: segs.clone(), fill, stroke: Some(outer) });
                    self.canvas.push(Item::Path { segments: segs.clone(), fill: None, stroke: Some(inner) });
                }
                (None, Some(s)) if self.st.inner_line.0 > 0.0 => {
                    // the basic layer's inner line, over the stroke
                    let inner = Stroke { width: self.st.inner_line.0, color: self.st.inner_line.1.unwrap_or(crate::color::Color::WHITE), ..s.clone() };
                    self.canvas.push(Item::Path { segments: segs.clone(), fill, stroke: Some(s.clone()) });
                    self.canvas.push(Item::Path { segments: segs.clone(), fill: None, stroke: Some(inner) });
                }
                _ => self.canvas.push(Item::Path { segments: segs.clone(), fill, stroke }),
            }
        }
        self.push_tips(tips);
        if faded {
            self.canvas.push(Item::End);
        }
        if blended {
            self.canvas.push(Item::End);
        }
        if mode.clip {
            self.open_in_scope(Item::Clip { segments: segs, even_odd: self.st.even_odd });
            // what the scope draws after a clip does not make the picture larger (pgf's
            // \pgf@relevantforpicturesizefalse)
            if let Some(s) = self.scopes.last_mut() {
                s.clipped = true;
            }
        }
        Ok(())
    }

    /// A fading by name, fitted to the path's bounding box (`fit fading`, on by default).
    pub(super) fn fading_for(&mut self, name: &str, segs: &[Segment]) -> Option<crate::canvas::Shading> {
        let mut c = crate::canvas::Canvas::default();
        c.include_path(segs);
        let (lo, hi) = (c.min?, c.max?);
        let center = lo.lerp(hi, 0.5);
        let fit = crate::scene::Transform::translation(center.x, center.y)
            .compose(&crate::scene::Transform::scaling(((hi.x - lo.x) / 50.0).max(1e-9), ((hi.y - lo.y) / 50.0).max(1e-9)))
            .compose(&self.st.fading_transform);
        if let Some(sh) = library_fading(name, &fit) {
            return Some(sh);
        }
        match self.fadings.get(name).cloned() {
            Some(mut sh) => {
                let (crate::canvas::Shading::Axial { transform, .. } | crate::canvas::Shading::Radial { transform, .. }) = &mut sh;
                *transform = fit.compose(transform);
                Some(sh)
            }
            None => {
                self.warn(format!("fading '{name}' not supported: drawn without it"));
                None
            }
        }
    }

    /// `blend mode=…`, `\pgfsetblendmode{…}`: pgf's names (`color dodge`) as SVG's (`color-dodge`).
    pub fn set_blend_mode(&mut self, mode: &str) {
        let mode = mode.trim().replace(' ', "-");
        self.st.blend = (mode != "normal" && !mode.is_empty()).then_some(mode);
    }

    /// The cells of the current shading if it is a functional one, fitted to the path's box.
    fn functional_cells(&mut self, segs: &[Segment]) -> Option<Vec<Item>> {
        if self.basic_shadings.contains_key(&self.st.shading) {
            return None;
        }
        let name = self.st.shading.clone();
        let f = self.functional_shading(&name)?;
        let mut c = crate::canvas::Canvas::default();
        c.include_path(segs);
        let (lo, hi) = (c.min?, c.max?);
        // only the part of the box under the path is sampled
        let place = f.fit(lo, hi, self.st.shade_angle);
        let back = place.inverse();
        let corners = [lo, Point::new(hi.x, lo.y), hi, Point::new(lo.x, hi.y)].map(|p| back.apply(p));
        let (mut wlo, mut whi) = (corners[0], corners[0]);
        for c in &corners[1..] {
            (wlo, whi) = (Point::new(wlo.x.min(c.x), wlo.y.min(c.y)), Point::new(whi.x.max(c.x), whi.y.max(c.y)));
        }
        Some(f.cells_in(&place, wlo, whi))
    }

    pub fn shading_of(&self, segs: &[Segment]) -> crate::canvas::Shading {
        self.shading_for(segs)
    }

    /// The current shading fitted to the path's bounding box, as `\pgfshadepath` fits it.
    fn shading_for(&self, segs: &[Segment]) -> crate::canvas::Shading {
        // `shading=name` of a shading the basic layer declared
        if let Some(sh) = self.basic_shading_for(&self.st.shading, segs, self.st.shade_angle) {
            return sh;
        }
        let mut c = crate::canvas::Canvas::default();
        c.include_path(segs);
        let (lo, hi) = (c.min.unwrap_or_default(), c.max.unwrap_or_default());
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        let a = self.st.shade_angle;
        let (s, co) = a.to_radians().sin_cos();
        let (abs_s, abs_c) = (s.abs(), co.abs());
        // pgf's scaling: the shading's middle 50 bp cover the box along the rotated axes
        let sx = (w * abs_c + h * abs_s) / (abs_c + abs_s) / 50.0;
        let sy = (w * abs_s + h * abs_c) / (abs_c + abs_s) / 50.0;
        let center = lo.lerp(hi, 0.5);
        let transform = crate::scene::Transform::translation(center.x, center.y)
            .compose(&crate::scene::Transform::rotation(a))
            .compose(&crate::scene::Transform::scaling(sx.max(1e-9), sy.max(1e-9)));
        let st = &self.st;
        let blend =
            |x: crate::color::Color, y: crate::color::Color, t: f64| crate::color::Color::rgb(x.r * t + y.r * (1.0 - t), x.g * t + y.g * (1.0 - t), x.b * t + y.b * (1.0 - t));
        let (white, black) = (crate::color::Color::WHITE, crate::color::Color::BLACK);
        match st.shading.as_str() {
            "ball" => crate::canvas::Shading::Radial {
                transform,
                focus: Point::new(-10.0, 10.0),
                stops: vec![
                    (0.0, blend(st.ball, white, 0.15)),
                    (9.0 / 50.0, blend(st.ball, white, 0.75)),
                    (18.0 / 50.0, blend(st.ball, black, 0.7)),
                    (0.5, blend(st.ball, black, 0.5)),
                    (1.0, black),
                ],
            },
            "radial" => crate::canvas::Shading::Radial { transform, focus: Point::default(), stops: vec![(0.0, st.radial_inner), (0.5, st.radial_outer), (1.0, st.radial_outer)] },
            _ => crate::canvas::Shading::Axial {
                transform,
                stops: vec![(0.0, st.axis_bottom), (0.25, st.axis_bottom), (0.5, st.axis_middle), (0.75, st.axis_top), (1.0, st.axis_top)],
            },
        }
    }

    /// Shortens the path for its arrow tips and `shorten <`/`shorten >`, and draws the tips, as
    /// `\pgf@prepare@end@of@path` and `\pgf@prepare@start@of@path` do: the end first, then the start
    /// of the path as the end left it. Nothing on a closed path.
    fn arrow_ends(&mut self, segs: &mut [Segment]) -> Tips {
        let mut out = Tips::default();
        if segs.iter().any(|s| matches!(s, Segment::Close)) || self.st.tips_mode == TipsMode::Never {
            return out;
        }
        // `tips=proper`, `on proper draw`: not on a path that stays at one point
        if matches!(self.st.tips_mode, TipsMode::Proper | TipsMode::OnProperDraw) && !proper(segs) {
            return out;
        }
        let s = self.stroke();
        let widths = match self.st.double {
            Some(_) => Widths { line: (2.0 * s.width + self.st.double_distance) / PT, inner: self.st.double_distance / PT },
            None => Widths { line: s.width / PT, inner: 0.0 },
        };
        let user = self.st.tips.clone();
        let mut warn = Vec::new();
        let (end_spec, start_spec) = (self.st.end_arrow.clone().unwrap_or_default(), self.st.start_arrow.clone().unwrap_or_default());
        let (shorten_end, shorten_start) = (self.st.shorten_end / PT, self.st.shorten_start / PT);
        let end = EndTips::new(self, &end_spec, false, &user, widths, shorten_end, &mut warn);
        let start = EndTips::new(self, &start_spec, true, &user, widths, shorten_start, &mut warn);
        for w in warn {
            self.warn(w);
        }
        let mut at_end = None;
        if !end.is_empty() || end.shortening != 0.0 {
            // (the path has a point: it starts with a move, and a closed one has no tips)
            {
                let (k, movable, last, reference) = path_end(segs).expect("a point at the end");
                // a curve with flexed tips: shortened along it, and the tips laid on it
                let curve = match (segs[k], end.flexed()) {
                    (Segment::CurveTo(c1, c2, _), true) => point_of(&segs[k - 1]).map(|prev| Curvilinear::new([last, c2, c1, prev])),
                    _ => None,
                };
                if movable && end.shortening != 0.0 {
                    match &curve {
                        Some(c) => {
                            let [p, b, a, _] = c.after(c.time(end.shortening));
                            segs[k] = Segment::CurveTo(a, b, p);
                        }
                        None => {
                            let p = toward(last, reference, end.shortening * PT);
                            if let Segment::LineTo(q) | Segment::CurveTo(_, _, q) = &mut segs[k] {
                                *q = p;
                            }
                        }
                    }
                }
                at_end = Some(match curve {
                    Some(c) => Along::Curve(c),
                    None => Along::Line(last, reference),
                });
            }
        }
        let mut at_start = None;
        if !start.is_empty() || start.shortening != 0.0 {
            {
                let (m, movable, first, reference) = path_start(segs).expect("a point at the start");
                let curve = match (segs.get(m + 1), start.flexed()) {
                    (Some(&Segment::CurveTo(c1, c2, p)), true) => Some(Curvilinear::new([first, c1, c2, p])),
                    _ => None,
                };
                if movable && start.shortening != 0.0 {
                    match &curve {
                        Some(c) => {
                            let [p, a, b, q] = c.after(c.time(start.shortening));
                            segs[m] = Segment::MoveTo(p);
                            segs[m + 1] = Segment::CurveTo(a, b, q);
                        }
                        None => segs[m] = Segment::MoveTo(toward(first, reference, start.shortening * PT)),
                    }
                }
                at_start = Some(match curve {
                    Some(c) => Along::Curve(c),
                    None => Along::Line(first, reference),
                });
            }
        }
        // the stroke color and line width, without the dash; fills with the fill opacity
        let stroke = Stroke { width: widths.line * PT, ..s };
        let fill_opacity = self.st.fill_opacity;
        for (tips, at) in [(&start, at_start), (&end, at_end)] {
            let hull = match at {
                Some(Along::Line(p, from)) => tips.draw(p, from, &stroke, fill_opacity, &mut out.items),
                Some(Along::Curve(c)) => tips.draw_on_curve(&c, &stroke, fill_opacity, &mut out.items),
                None => continue,
            };
            out.hull.extend(hull);
        }
        out
    }

    fn push_tips(&mut self, tips: Tips) {
        if self.counts_for_size() {
            for p in tips.hull {
                self.canvas.include(p);
            }
        }
        self.canvas.items.extend(tips.items);
    }
}

/// What the tips of an end are laid along: the line from a point to the end, or the end's curve.
enum Along {
    Line(Point, Point),
    Curve(Curvilinear),
}

/// The drawing of a path's tips, and the points of their hulls (for the bounding box).
#[derive(Default)]
struct Tips {
    items: Vec<Item>,
    hull: Vec<Point>,
}

/// The point at `distance` from `p` toward `q` (`\pgfpointlineatdistance`). When they coincide pgf's
/// normalized vector points up.
fn toward(p: Point, q: Point, distance: f64) -> Point {
    let d = q.minus(p);
    let u = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(0.0, 1.0) };
    p.plus(u.scale(distance))
}

fn point_of(s: &Segment) -> Option<Point> {
    let (Segment::MoveTo(p) | Segment::LineTo(p) | Segment::CurveTo(_, _, p)) = s else { return None };
    Some(*p)
}

/// Where the last subpath (the only one with tips, as pgf splits the path) begins.
fn last_subpath(segs: &[Segment]) -> Option<usize> {
    segs.iter().rposition(|s| matches!(s, Segment::MoveTo(_)))
}

/// The end of the path (`\pgf@parse@end`): the last segment, if it may be shortened (not a lone
/// point), its end point, and the point the tip is aimed from: the previous point, or the last
/// control point of a curve, the one before if they coincide. pgf moves only the end point.
fn path_end(segs: &[Segment]) -> Option<(usize, bool, Point, Point)> {
    let k = segs.len().checked_sub(1)?;
    match segs[k] {
        Segment::LineTo(last) => Some((k, true, last, point_of(&segs[k.checked_sub(1)?])?)),
        Segment::CurveTo(c1, c2, last) => {
            let r = if c2 != last {
                c2
            } else if c1 != last {
                c1
            } else {
                point_of(&segs[k.checked_sub(1)?])?
            };
            Some((k, true, last, r))
        }
        // a lone point (a closed path has no tips)
        ref s => point_of(s).map(|p| (k, false, p, p)),
    }
}

/// The start of the last subpath (`\pgf@parse@start`): its index, if it may be shortened, the first
/// point and the point the tip is aimed from: the next point, or the first control point of a curve
/// (then the second, then its end, while they coincide).
fn path_start(segs: &[Segment]) -> Option<(usize, bool, Point, Point)> {
    let m = last_subpath(segs)?;
    let first = point_of(&segs[m])?;
    match segs.get(m + 1) {
        Some(&Segment::LineTo(p)) => Some((m, true, first, p)),
        Some(&Segment::CurveTo(c1, c2, p)) => Some((
            m,
            true,
            first,
            if c1 != first {
                c1
            } else if c2 != first {
                c2
            } else {
                p
            },
        )),
        // a lone point: the subpath ends there (a closed path has no tips)
        _ => Some((m, false, first, first)),
    }
}

/// A path whose last subpath leaves its first point (`\pgf@path@check@proper`: each move starts
/// the test again).
fn proper(segs: &[Segment]) -> bool {
    let last = segs.iter().rposition(|s| matches!(s, Segment::MoveTo(_))).unwrap_or(0);
    let points: Vec<Point> = segs[last..]
        .iter()
        // (a closed path has no tips: no `Close` comes here)
        .flat_map(|s| if let Segment::CurveTo(a, b, p) = *s { vec![a, b, p] } else { point_of(s).into_iter().collect() })
        .collect();
    points.iter().any(|p| Some(p) != points.first())
}

/// One piece of a subpath: a line or a curve to its end point.
#[derive(Clone, Copy)]
enum Piece {
    Line(Point),
    Curve(Point, Point, Point),
}

impl Piece {
    fn end(self) -> Point {
        match self {
            Piece::Line(p) | Piece::Curve(_, _, p) => p,
        }
    }

    fn with_end(self, p: Point) -> Piece {
        match self {
            Piece::Line(_) => Piece::Line(p),
            Piece::Curve(a, b, _) => Piece::Curve(a, b, p),
        }
    }

    /// The point the piece comes from into its end: its start, or its second control point.
    fn into_end(self, start: Point) -> Point {
        match self {
            Piece::Line(_) => start,
            Piece::Curve(_, b, _) => b,
        }
    }

    /// The point the piece heads to from its start: its end, or its first control point.
    fn out_of_start(self) -> Point {
        match self {
            Piece::Line(p) | Piece::Curve(p, _, _) => p,
        }
    }

    fn segment(self) -> Segment {
        match self {
            Piece::Line(p) => Segment::LineTo(p),
            Piece::Curve(a, b, p) => Segment::CurveTo(a, b, p),
        }
    }
}

/// The arc that replaces corner `v`, coming from `from` and going on to `to`, as pgf rounds it
/// (`\pgfprocessround`): it starts `r_in` before the corner and ends `r_out` after it, with no regard
/// for the length of the sides. None when there is no corner: a side of no length, or a joint
/// where the path goes on straight (inside an arc, where pgf has no corner to round either).
fn corner_arc(from: Point, v: Point, to: Point, r_in: f64, r_out: f64) -> Option<(Point, Point, Point, Point)> {
    let (d1, d2) = (from.minus(v), to.minus(v));
    let (n1, n2) = (d1.norm(), d2.norm());
    if n1 < 1e-9 || n2 < 1e-9 {
        return None;
    }
    let (u1, u2) = (d1.scale(1.0 / n1), d2.scale(1.0 / n2));
    // straight on: the two directions opposite each other
    if (u1.x * u2.y - u1.y * u2.x).abs() < 1e-6 && u1.x * u2.x + u1.y * u2.y < 0.0 {
        return None;
    }
    let (a, b) = (v.plus(u1.scale(r_in)), v.plus(u2.scale(r_out)));
    Some((a, a.lerp(v, 0.5522847), b.lerp(v, 0.5522847), b))
}

/// Rounded corners, as pgf makes them: at every corner where a line or a curve ends and another
/// one (or the closing line) begins, an arc from `r_in` before the corner to `r_out` after it. A
/// curve's corner is found along its control points, and the curve is cut to the arc's start.
pub fn round_corners(segs: &[Segment], r_in: f64, r_out: f64) -> Vec<Segment> {
    // the subpaths: start, pieces, closed
    let mut subpaths: Vec<(Point, Vec<Piece>, bool)> = Vec::new();
    for s in segs {
        match *s {
            Segment::MoveTo(p) => subpaths.push((p, Vec::new(), false)),
            Segment::LineTo(p) => {
                if let Some(sp) = subpaths.last_mut() {
                    sp.1.push(Piece::Line(p));
                }
            }
            Segment::CurveTo(a, b, p) => {
                if let Some(sp) = subpaths.last_mut() {
                    sp.1.push(Piece::Curve(a, b, p));
                }
            }
            Segment::Close => {
                if let Some(sp) = subpaths.last_mut() {
                    sp.2 = true;
                }
            }
        }
    }
    let mut out = Vec::new();
    for (start, mut pieces, closed) in subpaths {
        if closed {
            // the closing line, unless the path is already back at its start
            match pieces.last() {
                Some(last) if last.end().minus(start).norm() > 1e-9 => pieces.push(Piece::Line(start)),
                _ => {}
            }
        }
        let n = pieces.len();
        if n == 0 {
            out.push(Segment::MoveTo(start));
            if closed {
                out.push(Segment::Close);
            }
            continue;
        }
        let begin = |k: usize, pieces: &[Piece]| if k == 0 { start } else { pieces[k - 1].end() };
        // the arc at the end of piece k, toward piece k + 1 (and for a closed path the last one
        // toward the first)
        let mut arcs: Vec<Option<(Point, Point, Point, Point)>> = vec![None; n];
        for k in 0..n {
            let next = if k + 1 < n {
                pieces[k + 1]
            } else if closed {
                pieces[0]
            } else {
                continue;
            };
            arcs[k] = corner_arc(pieces[k].into_end(begin(k, &pieces)), pieces[k].end(), next.out_of_start(), r_in, r_out);
        }
        let first = if closed { arcs[n - 1].map_or(start, |a| a.3) } else { start };
        out.push(Segment::MoveTo(first));
        for k in 0..n {
            match arcs[k] {
                Some((a, c1, c2, b)) => {
                    out.push(pieces[k].with_end(a).segment());
                    out.push(Segment::CurveTo(c1, c2, b));
                }
                None => out.push(pieces[k].segment()),
            }
        }
        if closed {
            out.push(Segment::Close);
        }
    }
    out
}

/// The fadings of the fadings library (opaque on one side and transparent on the other, or round),
/// their 100 bp shadings placed by `fit`.
pub fn library_fading(name: &str, fit: &crate::scene::Transform) -> Option<crate::canvas::Shading> {
    let fit = *fit;
    let (white, black) = (crate::color::Color::WHITE, crate::color::Color::BLACK);
    let axial = |angle: f64, from: crate::color::Color, to: crate::color::Color| crate::canvas::Shading::Axial {
        transform: fit.compose(&crate::scene::Transform::rotation(angle)),
        stops: vec![(0.0, from), (0.25, from), (0.75, to), (1.0, to)],
    };
    let circle =
        |edge: f64| crate::canvas::Shading::Radial { transform: fit, focus: Point::default(), stops: vec![(0.0, white), (edge / 50.0, white), (0.5, black), (1.0, black)] };
    Some(match name {
        // the library's: opaque on one side, transparent on the other
        "north" => axial(0.0, white, black),
        "south" => axial(0.0, black, white),
        "east" => axial(-90.0, white, black),
        "west" => axial(-90.0, black, white),
        "circle with fuzzy edge 10 percent" => circle(22.5),
        "circle with fuzzy edge 15 percent" => circle(21.25),
        "circle with fuzzy edge 20 percent" => circle(20.0),
        "fuzzy ring 15 percent" => crate::canvas::Shading::Radial {
            transform: fit,
            focus: Point::default(),
            stops: vec![(0.0, black), (21.25 / 50.0, black), (23.125 / 50.0, white), (0.5, black), (1.0, black)],
        },
        _ => return None,
    })
}
