//! Plot marks: `plot[mark=*] coordinates {…}` draws a mark at each point after the path, as
//! `\pgfplothandlermark` does — shifted to the point, in the picture's transformation and the
//! `every mark` options (`mark options`), `mark size` its radius. The marks are pgf's own and the
//! plotmarks library's; `mark repeat`, `mark phase` and `mark indices` choose the points.

use crate::canvas::{Fill, Item, Paint};
use crate::scene::{Point, Segment, Transform};
use crate::Error;

use super::nodes::{NodeSpec, OnPath};
use super::{tikra_tokens, Engine};

/// How a piece of a mark is used: `\pgfusepathqstroke`, `qfill`, `qfillstroke`, or filled with
/// `mark color` (white when not set, nothing when `none`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Usage {
    Stroke,
    Fill,
    FillStroke,
    MarkColor,
}

fn circle(r: f64) -> Vec<Segment> {
    let mut v = vec![Segment::MoveTo(Point::new(r, 0.0))];
    v.extend(super::path::arc_segments(Point::default(), r, r, 0.0, 360.0, &Transform::default()));
    v.push(Segment::Close);
    v
}

fn polar(angle: f64, r: f64) -> Point {
    let (s, c) = angle.to_radians().sin_cos();
    Point::new(r * c, r * s)
}

/// Lines from `a` to `b`, each a subpath.
fn lines(v: &[(Point, Point)]) -> Vec<Segment> {
    v.iter().flat_map(|(a, b)| [Segment::MoveTo(*a), Segment::LineTo(*b)]).collect()
}

/// A closed polygon.
fn polygon(v: &[Point]) -> Vec<Segment> {
    let mut s = vec![Segment::MoveTo(v[0])];
    s.extend(v[1..].iter().map(|p| Segment::LineTo(*p)));
    s.push(Segment::Close);
    s
}

fn open(v: &[Point]) -> Vec<Segment> {
    let mut s = vec![Segment::MoveTo(v[0])];
    s.extend(v[1..].iter().map(|p| Segment::LineTo(*p)));
    s
}

/// The mark `name` with radius `r`, at the origin: its pieces in drawing order.
fn shape(name: &str, r: f64) -> Option<Vec<(Vec<Segment>, Usage)>> {
    let p = Point::new;
    let k = std::f64::consts::FRAC_1_SQRT_2 * r;
    let plus = lines(&[(p(-r, 0.0), p(r, 0.0)), (p(0.0, r), p(0.0, -r))]);
    let cross = lines(&[(p(-k, -k), p(k, k)), (p(-k, k), p(k, -k))]);
    let pentagon = [p(0.0, r), polar(18.0, r), polar(-54.0, r), polar(234.0, r), polar(162.0, r)];
    let diamond = [p(0.0, r), p(0.75 * r, 0.0), p(0.0, -r), p(-0.75 * r, 0.0)];
    let triangle = [p(0.0, r), polar(-30.0, r), polar(-150.0, r)];
    let square = [p(-r, -r), p(r, -r), p(r, r), p(-r, r)];
    let with = |mut a: Vec<Segment>, b: Vec<Segment>| {
        a.extend(b);
        a
    };
    // the half-filled marks: the lower half filled, the upper in mark color, then the outline
    let half = |lower: &[Point], upper: &[Point], outline: Vec<Segment>| vec![(open(lower), Usage::Fill), (open(upper), Usage::MarkColor), (outline, Usage::Stroke)];
    let outline4 = |a: f64| polygon(&[p(a, 0.0), p(0.0, r), p(-a, 0.0), p(0.0, -r)]);
    Some(match name {
        "*" => vec![(circle(r), Usage::FillStroke)],
        "+" => vec![(plus, Usage::Stroke)],
        "x" => vec![(cross, Usage::Stroke)],
        "o" => vec![(circle(r), Usage::Stroke)],
        "-" => vec![(lines(&[(p(r, 0.0), p(-r, 0.0))]), Usage::Stroke)],
        "|" => vec![(lines(&[(p(0.0, r), p(0.0, -r))]), Usage::Stroke)],
        "Mercedes star" => vec![(lines(&[(polar(90.0, r), p(0.0, 0.0)), (polar(-30.0, r), p(0.0, 0.0)), (polar(-150.0, r), p(0.0, 0.0))]), Usage::Stroke)],
        "Mercedes star flipped" => vec![(lines(&[(polar(-90.0, r), p(0.0, 0.0)), (polar(30.0, r), p(0.0, 0.0)), (polar(150.0, r), p(0.0, 0.0))]), Usage::Stroke)],
        "asterisk" => vec![(lines(&[(p(0.0, -r), p(0.0, r)), (polar(30.0, r), polar(210.0, r)), (polar(-30.0, r), polar(-210.0, r))]), Usage::Stroke)],
        "star" => vec![(lines(&[90.0, 18.0, -54.0, 234.0, 162.0].map(|a| (p(0.0, 0.0), polar(a, r)))), Usage::Stroke)],
        "10-pointed star" => vec![(lines(&[90.0, 18.0, -54.0, 234.0, 162.0].map(|a| (polar(a, -r), polar(a, r)))), Usage::Stroke)],
        "oplus" => vec![(with(circle(r), plus), Usage::Stroke)],
        "oplus*" => vec![(with(circle(r), plus), Usage::FillStroke)],
        "otimes" => vec![(with(circle(r), cross), Usage::Stroke)],
        "otimes*" => vec![(with(circle(r), cross), Usage::FillStroke)],
        "square" => vec![(polygon(&square), Usage::Stroke)],
        "square*" => vec![(polygon(&square), Usage::FillStroke)],
        "triangle" => vec![(polygon(&triangle), Usage::Stroke)],
        "triangle*" => vec![(polygon(&triangle), Usage::FillStroke)],
        "diamond" => vec![(polygon(&diamond), Usage::Stroke)],
        "diamond*" => vec![(polygon(&diamond), Usage::FillStroke)],
        "pentagon" => vec![(polygon(&pentagon), Usage::Stroke)],
        "pentagon*" => vec![(polygon(&pentagon), Usage::FillStroke)],
        "halfcircle" => {
            let mut lower = vec![Segment::MoveTo(p(-r, 0.0))];
            lower.extend(super::path::arc_segments(Point::default(), r, r, 180.0, 360.0, &Transform::default()));
            vec![(lower, Usage::MarkColor), (with(lines(&[(p(-r, 0.0), p(r, 0.0))]), circle(r)), Usage::Stroke)]
        }
        "halfcircle*" => {
            let mut upper = vec![Segment::MoveTo(p(r, 0.0))];
            upper.extend(super::path::arc_segments(Point::default(), r, r, 0.0, 180.0, &Transform::default()));
            upper.push(Segment::Close);
            let mut lower = vec![Segment::MoveTo(p(-r, 0.0))];
            lower.extend(super::path::arc_segments(Point::default(), r, r, 180.0, 360.0, &Transform::default()));
            vec![(upper, Usage::Fill), (lower, Usage::MarkColor), (circle(r), Usage::Stroke)]
        }
        "halfdiamond*" => half(&[p(-0.75 * r, 0.0), p(0.0, -r), p(0.75 * r, 0.0)], &[p(0.75 * r, 0.0), p(0.0, r), p(-0.75 * r, 0.0)], outline4(0.75 * r)),
        "halfsquare*" => half(&[p(-r, 0.0), p(0.0, -r), p(r, 0.0)], &[p(r, 0.0), p(0.0, r), p(-r, 0.0)], outline4(r)),
        "halfsquare right*" => half(&[p(0.0, -r), p(r, 0.0), p(0.0, r)], &[p(0.0, -r), p(-r, 0.0), p(0.0, r)], outline4(r)),
        "halfsquare left*" => half(&[p(0.0, -r), p(-r, 0.0), p(0.0, r)], &[p(0.0, -r), p(r, 0.0), p(0.0, r)], outline4(r)),
        "heart" => {
            let q = |x: f64, y: f64| p(x * r, y * r);
            let c = |a: Point, b: Point, e: Point| Segment::CurveTo(a, b, e);
            vec![(
                vec![
                    Segment::MoveTo(q(0.0, -1.75)),
                    c(q(0.0, -1.75), q(0.0, -1.66), q(-0.5, -1.165)),
                    c(q(-0.5, -1.165), q(-1.0, -0.75), q(-1.0, 0.0)),
                    c(q(-1.0, 0.0), q(-1.0, 0.5825), q(-0.5825, 0.5825)),
                    c(q(-0.5825, 0.5825), q(0.0, 0.5825), q(0.0, 0.0)),
                    c(q(0.0, 0.0), q(0.0, 0.5825), q(0.5825, 0.5825)),
                    c(q(0.5825, 0.5825), q(1.0, 0.5825), q(1.0, 0.0)),
                    c(q(1.0, 0.0), q(1.0, -0.75), q(0.5, -1.165)),
                    c(q(0.5, -1.165), q(0.0, -1.66), q(0.0, -1.75)),
                    Segment::Close,
                ],
                Usage::FillStroke,
            )]
        }
        _ => return None,
    })
}

/// Which points get a mark: `mark repeat`, `mark phase` (the first one marked), `mark indices`.
fn chosen(k: usize, repeat: usize, phase: usize, indices: &Option<Vec<usize>>) -> bool {
    match indices {
        Some(list) => list.contains(&k),
        None => k >= phase && (k - phase).is_multiple_of(repeat.max(1)),
    }
}

impl Engine {
    /// The marks of a plot at its points (canvas coordinates), drawn after its path.
    pub fn plot_marks(&mut self, points: &[Point]) -> Result<Vec<Item>, Error> {
        let Some(name) = self.st.plot_mark.clone() else { return Ok(Vec::new()) };
        self.begin_group();
        let r = self.plot_marks_in_group(&name, points);
        self.end_group();
        r
    }

    /// `mark=text` at a point: the `text mark`, set as a node in the `text mark style`.
    pub fn text_mark(&mut self, p: Point) -> Result<Vec<Item>, Error> {
        let text = crate::keys::value(&self.tex, "/pgf/text mark").unwrap_or_else(|| tikra_tokens("p"));
        let options = crate::keys::value(&self.tex, "/pgf/text mark style").unwrap_or_default();
        self.node(NodeSpec { options, name: None, at: Some(p), text, place: OnPath::Point(p), point: p, coordinate: false, default_pos: None })
    }

    fn plot_marks_in_group(&mut self, name: &str, points: &[Point]) -> Result<Vec<Item>, Error> {
        self.st.pending = Transform::default();
        self.process_keys(tikra_tokens("every mark/.try"), Some("/tikz"))?;
        let linear = Transform { e: 0.0, f: 0.0, ..self.st.transform }.compose(&self.st.pending);
        let (repeat, phase, indices) = (self.st.mark_repeat, self.st.mark_phase, self.st.mark_indices.clone());
        let mut items = Vec::new();
        if name == "text" || name == "ball" {
            for (k, p) in points.iter().enumerate() {
                if !chosen(k + 1, repeat, phase, &indices) {
                    continue;
                }
                if name == "ball" {
                    let t = Transform::translation(p.x, p.y).compose(&linear);
                    let segs = crate::canvas::transform_segments(&circle(self.st.mark_size), &t);
                    self.st.shading = "ball".into();
                    let paint = Paint::Shading(self.shading_of(&segs));
                    if self.counts_for_size() {
                        self.canvas.include_path(&segs);
                    }
                    items.push(Item::Path { segments: segs, fill: Some(Fill { paint, opacity: self.st.fill_opacity, even_odd: false }), stroke: None });
                } else {
                    items.extend(self.text_mark(*p)?);
                }
            }
            return Ok(items);
        }
        if let Some(code) = self.plot_marks_declared.get(name).cloned() {
            // a mark of `\pgfdeclareplotmark`: its code at each point
            for (k, p) in points.iter().enumerate() {
                if !p.x.is_finite() || !chosen(k + 1, repeat, phase, &indices) {
                    continue;
                }
                let from = self.canvas.items.len();
                self.begin_group();
                self.begin_scope();
                self.st.transform = Transform::translation(p.x, p.y).compose(&linear);
                let r = self.run_tokens(code.clone());
                self.end_scope();
                self.end_group();
                r?;
                items.extend(self.canvas.items.drain(from..));
            }
            return Ok(items);
        }
        let Some(parts) = shape(name, self.st.mark_size) else {
            self.warn(format!("plot mark '{name}' not supported: no marks drawn"));
            return Ok(items);
        };
        let stroke = self.stroke();
        let fill = Fill { paint: Paint::Solid(self.st.fill_paint_color()), opacity: self.st.fill_opacity, even_odd: false };
        let mark_color = match self.st.mark_color.as_deref() {
            Some("none") => None,
            Some(c) => Some(self.color(c).unwrap_or(crate::color::Color::WHITE)),
            None => Some(crate::color::Color::WHITE),
        };
        for (k, p) in points.iter().enumerate() {
            if !p.x.is_finite() || !chosen(k + 1, repeat, phase, &indices) {
                continue;
            }
            let t = Transform::translation(p.x, p.y).compose(&linear);
            for (segs, usage) in &parts {
                let segs = crate::canvas::transform_segments(segs, &t);
                let (f, s) = match usage {
                    Usage::Stroke => (None, Some(stroke.clone())),
                    Usage::Fill => (Some(fill.clone()), None),
                    Usage::FillStroke => (Some(fill.clone()), Some(stroke.clone())),
                    Usage::MarkColor => match mark_color {
                        Some(c) => (Some(Fill { paint: Paint::Solid(c), ..fill.clone() }), None),
                        None => continue,
                    },
                };
                if self.counts_for_size() {
                    self.canvas.include_path(&segs);
                    if let Some(s) = &s {
                        self.canvas.include_path_margin(&segs, s.width / 2.0);
                    }
                }
                items.push(Item::Path { segments: segs, fill: f, stroke: s });
            }
        }
        Ok(items)
    }

    /// `\pgfuseplotmark{name}`: the mark at the origin of the coordinate system.
    pub fn use_plot_mark(&mut self, name: &str) -> Result<(), Error> {
        let origin = self.st.transform.apply(Point::default());
        let saved = self.st.transform;
        // the marks take the linear part from the coordinate system here, as at a plot's points
        let items = self.plot_marks_at(name, &[origin]);
        self.st.transform = saved;
        let items = items?;
        self.canvas.items.extend(items);
        Ok(())
    }

    fn plot_marks_at(&mut self, name: &str, points: &[Point]) -> Result<Vec<Item>, Error> {
        self.begin_group();
        let r = self.plot_marks_in_group(name, points);
        self.end_group();
        r
    }
}
