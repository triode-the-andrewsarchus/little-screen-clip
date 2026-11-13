//! A figure for a pen plotter: strokes only, in G-code or HPGL. Curves become short lines, fills
//! and glyphs their outlines, and the strokes are put in the order that lifts the pen least. The
//! same strokes can be watched: an SVG where a pen draws them, or that drawing at any moment.

use std::fmt::Write;

use resvg::usvg::{self, tiny_skia_path::PathSegment};

use crate::Error;

/// Millimeters per CSS pixel, the unit `usvg` works in.
const MM: f64 = 25.4 / 96.0;
/// How long a curve's short lines are, at most, in mm.
const STEP: f64 = 0.25;
/// How much faster the pen travels lifted than drawing.
const LIFTED: f64 = 3.0;
/// Seconds the finished drawing stays before the animation starts over.
const HOLD: f64 = 1.6;

/// A pen-down line, in mm, from the paper's lower left corner.
pub type Stroke = Vec<(f64, f64)>;

/// What to do with the strokes.
#[derive(Debug, Clone)]
pub struct Pen {
    /// Keep the order TikZ drew them in.
    pub keep_order: bool,
    /// The paper, in mm: the figure is centered on it, and made smaller if it does not fit.
    pub paper: Option<(f64, f64)>,
    pub up: String,
    pub down: String,
    /// Drawing speed, in mm a minute.
    pub feed: f64,
    /// How long the animated pen takes to draw the figure, in seconds.
    pub seconds: f64,
}

impl Default for Pen {
    fn default() -> Pen {
        Pen { keep_order: false, paper: None, up: "G0 Z5".into(), down: "G0 Z0".into(), feed: 1500.0, seconds: 10.0 }
    }
}

/// The strokes of what a group draws.
///
/// The transformation is added up here, group by group: `abs_transform` applies a `<use>`'s twice,
/// which leaves each glyph a hundredth of its size.
fn walk(group: &usvg::Group, parent: usvg::Transform, height: f64, out: &mut Vec<Stroke>) {
    if group.opacity().get() == 0.0 {
        return;
    }
    let t = parent.pre_concat(group.transform());
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, t, height, out),
            usvg::Node::Path(p) if p.is_visible() && (p.fill().is_some() || p.stroke().is_some()) => {
                let at = |q: usvg::tiny_skia_path::Point| {
                    let (x, y) = (f64::from(q.x), f64::from(q.y));
                    let (sx, kx, ky, sy, tx, ty) = (f64::from(t.sx), f64::from(t.kx), f64::from(t.ky), f64::from(t.sy), f64::from(t.tx), f64::from(t.ty));
                    ((sx * x + kx * y + tx) * MM, height - (ky * x + sy * y + ty) * MM)
                };
                let mut line: Stroke = Vec::new();
                let mut first = (0.0, 0.0);
                for s in p.data().segments() {
                    match s {
                        PathSegment::MoveTo(q) => {
                            if line.len() > 1 {
                                out.push(std::mem::take(&mut line));
                            }
                            first = at(q);
                            line = vec![first];
                        }
                        PathSegment::LineTo(q) => line.push(at(q)),
                        PathSegment::QuadTo(a, q) => curve(&mut line, [at(a), at(a), at(q)], true),
                        PathSegment::CubicTo(a, b, q) => curve(&mut line, [at(a), at(b), at(q)], false),
                        PathSegment::Close => line.push(first),
                    }
                }
                if line.len() > 1 {
                    out.push(line);
                }
            }
            _ => {}
        }
    }
}

/// A curve from the end of `line` as short lines. A quadratic's control point comes twice.
fn curve(line: &mut Stroke, [a, b, q]: [(f64, f64); 3], quadratic: bool) {
    let p = line.last().copied().unwrap_or(a);
    let dist = |u: (f64, f64), v: (f64, f64)| (u.0 - v.0).hypot(u.1 - v.1);
    let length = dist(p, a) + dist(a, b) + dist(b, q);
    let n = ((length / STEP).ceil() as usize).clamp(1, 256);
    for k in 1..=n {
        let t = k as f64 / n as f64;
        let u = 1.0 - t;
        let of = |p: f64, a: f64, b: f64, q: f64| {
            if quadratic {
                u * u * p + 2.0 * u * t * a + t * t * q
            } else {
                u * u * u * p + 3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t * q
            }
        };
        line.push((of(p.0, a.0, b.0, q.0), of(p.1, a.1, b.1, q.1)));
    }
}

/// The strokes of an SVG, and the size of the figure, in mm.
pub fn strokes(svg: &str) -> Result<(Vec<Stroke>, (f64, f64)), Error> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| Error::Png(e.to_string()))?;
    let (w, h) = (f64::from(tree.size().width()) * MM, f64::from(tree.size().height()) * MM);
    let mut out = Vec::new();
    walk(tree.root(), usvg::Transform::identity(), h, &mut out);
    Ok((out, (w, h)))
}

/// The strokes centered on the paper, smaller if they do not fit it.
fn on_paper(strokes: &mut [Stroke], size: (f64, f64), paper: (f64, f64)) {
    let k = (paper.0 / size.0).min(paper.1 / size.1).min(1.0);
    let (dx, dy) = ((paper.0 - size.0 * k) / 2.0, (paper.1 - size.1 * k) / 2.0);
    for p in strokes.iter_mut().flatten() {
        *p = (p.0 * k + dx, p.1 * k + dy);
    }
}

/// Each next stroke the nearest one to where the pen is, from either of its ends.
fn nearest_first(mut left: Vec<Stroke>) -> Vec<Stroke> {
    let mut out = Vec::with_capacity(left.len());
    let mut at = (0.0, 0.0);
    while !left.is_empty() {
        let dist = |p: (f64, f64)| (p.0 - at.0).hypot(p.1 - at.1);
        let mut best = (0, false, f64::MAX);
        for (i, s) in left.iter().enumerate() {
            for (reversed, end) in [(false, s[0]), (true, s[s.len() - 1])] {
                if dist(end) < best.2 {
                    best = (i, reversed, dist(end));
                }
            }
        }
        let mut s = left.swap_remove(best.0);
        if best.1 {
            s.reverse();
        }
        at = s[s.len() - 1];
        out.push(s);
    }
    out
}

/// The strokes in the order they will be drawn, and the sheet they are on.
fn prepared(svg: &str, pen: &Pen) -> Result<(Vec<Stroke>, (f64, f64)), Error> {
    let (mut all, mut size) = strokes(svg)?;
    if let Some(paper) = pen.paper {
        on_paper(&mut all, size, paper);
        size = paper;
    }
    Ok((if pen.keep_order { all } else { nearest_first(all) }, size))
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn stroke_length(s: &[(f64, f64)]) -> f64 {
    s.windows(2).map(|w| distance(w[0], w[1])).sum()
}

/// The drawing in time: each stroke with when it starts and ends, in seconds.
struct Timeline {
    strokes: Vec<Stroke>,
    size: (f64, f64),
    spans: Vec<(f64, f64)>,
    seconds: f64,
}

impl Timeline {
    fn new(svg: &str, pen: &Pen) -> Result<Timeline, Error> {
        if !(pen.seconds > 0.0 && pen.seconds.is_finite()) {
            return Err(Error::Png(format!("invalid time for the pen: {}", pen.seconds)));
        }
        let (strokes, size) = prepared(svg, pen)?;
        if strokes.is_empty() {
            return Err(Error::Png("nothing for the pen to draw".into()));
        }
        let drawn: f64 = strokes.iter().map(|s| stroke_length(s)).sum();
        let lifted: f64 = strokes.windows(2).map(|w| distance(w[0][w[0].len() - 1], w[1][0])).sum();
        let per_mm = pen.seconds / (drawn + lifted / LIFTED).max(1e-9);
        let mut spans = Vec::with_capacity(strokes.len());
        let mut t = 0.0;
        for (k, s) in strokes.iter().enumerate() {
            if k > 0 {
                t += distance(strokes[k - 1][strokes[k - 1].len() - 1], s[0]) * per_mm / LIFTED;
            }
            let end = t + stroke_length(s) * per_mm;
            spans.push((t, end));
            t = end;
        }
        Ok(Timeline { strokes, size, spans, seconds: pen.seconds })
    }

    /// A point on the paper in the SVG's coordinates.
    fn page(&self, p: (f64, f64)) -> (f64, f64) {
        (p.0, self.size.1 - p.1)
    }

    fn line(&self, points: &[(f64, f64)]) -> String {
        let mut d = String::with_capacity(points.len() * 12);
        for (k, &p) in points.iter().enumerate() {
            let (x, y) = self.page(p);
            let _ = write!(d, "{}{x:.2} {y:.2}", if k == 0 { "M" } else { " L" });
        }
        d
    }

    fn head(&self, attributes: &str) -> String {
        let (w, h) = self.size;
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.3} {h:.3}" width="{w:.3}mm" height="{h:.3}mm"{attributes}><rect width="{w:.3}" height="{h:.3}" fill="white"/>"#
        )
    }

    /// The size of the pen's tip, and of the pen lifted.
    fn tips(&self) -> (f64, f64) {
        let r = self.size.0.max(self.size.1) * 0.012;
        (r, r * 1.7)
    }

    /// The drawing at `t` seconds, as a still SVG with the pen where it is.
    fn at(&self, t: f64) -> String {
        let mut s = self.head("");
        let mut pen = self.strokes[0][0];
        let mut down = false;
        for (stroke, &(from, to)) in self.strokes.iter().zip(&self.spans) {
            if t <= from {
                break;
            }
            if t >= to {
                pen = stroke[stroke.len() - 1];
                let _ = write!(s, r##"<path d="{}"{INK}/>"##, self.line(stroke));
                continue;
            }
            // part of the way along it
            let goal = stroke_length(stroke) * (t - from) / (to - from).max(1e-9);
            let (mut done, mut part) = (0.0, vec![stroke[0]]);
            for w in stroke.windows(2) {
                let d = distance(w[0], w[1]);
                if done + d >= goal {
                    let f = (goal - done) / d.max(1e-9);
                    part.push((w[0].0 + (w[1].0 - w[0].0) * f, w[0].1 + (w[1].1 - w[0].1) * f));
                    break;
                }
                done += d;
                part.push(w[1]);
            }
            pen = part[part.len() - 1];
            down = true;
            let _ = write!(s, r##"<path d="{}"{INK}/>"##, self.line(&part));
            break;
        }
        // lifted, between two strokes
        if !down && t < self.seconds {
            if let Some(k) = self.spans.iter().position(|&(from, _)| t < from).filter(|&k| k > 0) {
                let (a, b) = (self.strokes[k - 1][self.strokes[k - 1].len() - 1], self.strokes[k][0]);
                let (start, end) = (self.spans[k - 1].1, self.spans[k].0);
                let f = (t - start) / (end - start).max(1e-9);
                pen = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            }
        }
        let (x, y) = self.page(pen);
        let (r, lifted) = self.tips();
        let _ = write!(s, r##"<circle cx="{x:.2}" cy="{y:.2}" r="{:.2}" fill="{TIP}"/></svg>"##, if down { r } else { lifted });
        s
    }

    /// The whole drawing with SMIL: each stroke appears as the pen passes, in a loop, and the
    /// finished drawing holds a moment. No script: a browser plays it by itself.
    fn animation(&self) -> String {
        let cycle = self.seconds + HOLD;
        let key = |t: f64| format!("{:.5}", (t / cycle).clamp(0.0, 1.0));
        let mut s = self.head(r#" role="img" aria-label="A pen drawing the figure""#);
        for (stroke, &(from, to)) in self.strokes.iter().zip(&self.spans) {
            let _ = write!(
                s,
                r##"<path d="{}"{INK} pathLength="1" stroke-dasharray="1 2" stroke-dashoffset="1.01"><animate attributeName="stroke-dashoffset" values="1.01;1.01;0;0" keyTimes="0;{};{};1" dur="{cycle}s" repeatCount="indefinite"/></path>"##,
                self.line(stroke),
                key(from),
                key(to)
            );
        }
        // the pen's way, strokes and travels, and how far along it each moment is
        let route: Vec<(f64, f64)> = self.strokes.iter().flatten().copied().collect();
        let total = stroke_length(&route).max(1e-9);
        let (mut along, mut times, mut points, mut sizes, mut size_times) = (0.0, vec!["0".to_string()], vec!["0".to_string()], Vec::new(), Vec::new());
        let (r, lifted) = self.tips();
        for (k, (stroke, &(from, to))) in self.strokes.iter().zip(&self.spans).enumerate() {
            if k > 0 {
                along += distance(self.strokes[k - 1][self.strokes[k - 1].len() - 1], stroke[0]);
                times.push(key(from));
                points.push(format!("{:.5}", along / total));
            }
            sizes.push(format!("{r:.2}"));
            size_times.push(key(from));
            along += stroke_length(stroke);
            times.push(key(to));
            points.push(format!("{:.5}", (along / total).min(1.0)));
            sizes.push(format!("{lifted:.2}"));
            size_times.push(key(to));
        }
        times.push("1".to_string());
        points.push("1".to_string());
        let _ = write!(
            s,
            r##"<circle r="{lifted:.2}" fill="{TIP}"><animate attributeName="r" calcMode="discrete" values="{}" keyTimes="{}" dur="{cycle}s" repeatCount="indefinite"/><animateMotion path="{}" keyPoints="{}" keyTimes="{}" calcMode="linear" dur="{cycle}s" repeatCount="indefinite"/></circle></svg>"##,
            sizes.join(";"),
            size_times.join(";"),
            self.line(&route),
            points.join(";"),
            times.join(";")
        );
        s
    }
}

/// How a stroke of the pen is drawn.
const INK: &str = r##" fill="none" stroke="#15120E" stroke-width="0.25" stroke-linecap="round" stroke-linejoin="round""##;
/// The color of the pen's tip.
const TIP: &str = "#C8102E";

/// An SVG where a pen draws the figure, in `pen.seconds`, over and over, with no script.
pub fn animation(svg: &str, pen: &Pen) -> Result<String, Error> {
    Ok(Timeline::new(svg, pen)?.animation())
}

/// `count` + 1 still SVGs of the pen drawing, from the blank page to the finished figure, evenly
/// spaced: the frames of a GIF.
pub fn stills(svg: &str, pen: &Pen, count: usize) -> Result<Vec<String>, Error> {
    let line = Timeline::new(svg, pen)?;
    Ok((0..=count).map(|k| line.at(k as f64 / count.max(1) as f64 * line.seconds)).collect())
}

/// The sheet the pen draws on, in mm.
pub fn sheet(svg: &str, pen: &Pen) -> Result<(f64, f64), Error> {
    Ok(prepared(svg, pen)?.1)
}

/// The figure as G-code, in mm.
pub fn gcode(svg: &str, pen: &Pen) -> Result<String, Error> {
    let mut s = format!("G21\nG90\n{}\n", pen.up);
    for stroke in prepared(svg, pen)?.0 {
        s.push_str(&format!("G0 X{:.3} Y{:.3}\n{}\n", stroke[0].0, stroke[0].1, pen.down));
        for p in &stroke[1..] {
            s.push_str(&format!("G1 X{:.3} Y{:.3} F{}\n", p.0, p.1, pen.feed));
        }
        s.push_str(&pen.up);
        s.push('\n');
    }
    s.push_str("G0 X0 Y0\n");
    Ok(s)
}

/// The figure as HPGL, in its units of 0.025 mm.
pub fn hpgl(svg: &str, pen: &Pen) -> Result<String, Error> {
    let unit = |v: f64| (v * 40.0).round() as i64;
    let mut s = String::from("IN;SP1;\n");
    for stroke in prepared(svg, pen)?.0 {
        s.push_str(&format!("PU{},{};PD", unit(stroke[0].0), unit(stroke[0].1)));
        let points: Vec<String> = stroke[1..].iter().map(|p| format!("{},{}", unit(p.0), unit(p.1))).collect();
        s.push_str(&points.join(","));
        s.push_str(";\n");
    }
    s.push_str("PU0,0;SP0;\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_LINES: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><path d="M0 0 L10 0" stroke="black" fill="none"/><path d="M90 90 L90 80" stroke="black" fill="none"/><path d="M12 0 L20 0" stroke="black" fill="none"/></svg>"#;

    /// The pen's moves in a G-code: how far it draws and how far it travels up, in mm.
    fn lengths(code: &str) -> (f64, f64) {
        let (mut at, mut down, mut drawn, mut travel) = ((0.0f64, 0.0f64), false, 0.0, 0.0);
        for line in code.lines() {
            if line == "G0 Z0" {
                down = true;
            } else if line == "G0 Z5" {
                down = false;
            }
            let coordinate = |key: char| line.split(' ').find_map(|w| w.strip_prefix(key)).and_then(|v| v.parse::<f64>().ok());
            if let (Some(x), Some(y)) = (coordinate('X'), coordinate('Y')) {
                let d = (x - at.0).hypot(y - at.1);
                if line.starts_with("G1") {
                    assert!(down, "drawing with the pen up: {line}");
                    drawn += d;
                } else {
                    assert!(!down, "traveling with the pen down: {line}");
                    travel += d;
                }
                at = (x, y);
            }
        }
        (drawn, travel)
    }

    #[test]
    fn a_line_has_its_length_in_mm() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96" width="96" height="96"><path d="M0 0 L96 0" stroke="black" fill="none"/></svg>"#;
        let (drawn, _) = lengths(&gcode(svg, &Pen::default()).unwrap());
        assert!((drawn - 25.4).abs() < 0.01, "{drawn}");
    }

    #[test]
    fn y_goes_up_from_the_lower_left_corner() {
        let (all, (_, h)) = strokes(TWO_LINES).unwrap();
        assert!((all[0][0].1 - h).abs() < 1e-3);
    }

    #[test]
    fn the_nearest_order_lifts_the_pen_less() {
        let kept = lengths(&gcode(TWO_LINES, &Pen { keep_order: true, ..Pen::default() }).unwrap());
        let near = lengths(&gcode(TWO_LINES, &Pen::default()).unwrap());
        assert!((kept.0 - near.0).abs() < 1e-3);
        assert!(near.1 < kept.1, "{near:?} {kept:?}");
    }

    #[test]
    fn a_fill_is_its_outline_and_a_hidden_group_is_nothing() {
        let square = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96" width="96" height="96"><path d="M0 0 L96 0 L96 96 L0 96 Z" fill="red"/><g opacity="0"><path d="M0 0 L96 96" stroke="black"/></g></svg>"#;
        let (drawn, _) = lengths(&gcode(square, &Pen::default()).unwrap());
        assert!((drawn - 4.0 * 25.4).abs() < 0.01, "{drawn}");
    }

    #[test]
    fn a_glyph_keeps_its_size() {
        // as tikra writes text: a glyph in its font's units, scaled by a `<use>` in a group
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96" width="96" height="96"><defs><path id="g" d="M0 0 L500 0 L500 700 Z"/></defs><g transform="translate(10 20)" fill="#000000"><use href="#g" transform="translate(1 0) scale(0.01 -0.01)"/></g></svg>"##;
        let (all, _) = strokes(svg).unwrap();
        let xs: Vec<f64> = all[0].iter().map(|p| p.0).collect();
        let ys: Vec<f64> = all[0].iter().map(|p| p.1).collect();
        let span = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
        assert!((span(&xs) - 5.0 * MM).abs() < 1e-3 && (span(&ys) - 7.0 * MM).abs() < 1e-3, "{:?}", all[0]);
        assert!((xs[0] - 11.0 * MM).abs() < 1e-3, "{:?}", all[0]);
    }

    #[test]
    fn real_text_has_real_size() {
        let svg = crate::convert(r"\begin{tikzpicture}\node {$y = a \sin kx$};\end{tikzpicture}").unwrap().svg;
        let (all, _) = strokes(&svg).unwrap();
        let widest = all.iter().map(|s| s.iter().map(|p| p.0).fold(f64::MIN, f64::max) - s.iter().map(|p| p.0).fold(f64::MAX, f64::min)).fold(0.0, f64::max);
        assert!(all.len() > 10 && widest > 0.5, "{} strokes, widest {widest} mm", all.len());
    }

    #[test]
    fn hpgl_counts_in_its_units() {
        let code = hpgl(TWO_LINES, &Pen { keep_order: true, ..Pen::default() }).unwrap();
        assert!(code.starts_with("IN;SP1;\n") && code.ends_with("PU0,0;SP0;\n"));
        let first = code.lines().nth(1).unwrap();
        // 10 px are 2.646 mm, 106 units
        assert!(first.starts_with("PU0,") && first.contains(";PD106,"), "{first}");
    }

    #[test]
    fn the_pen_draws_in_the_time_asked_for_and_lifts_faster() {
        let line = Timeline::new(TWO_LINES, &Pen { keep_order: true, seconds: 8.0, ..Pen::default() }).unwrap();
        let last = line.spans[line.spans.len() - 1].1;
        assert!((last - 8.0).abs() < 1e-9, "{last}");
        // a stroke's time goes with its length
        let (a, b) = (line.spans[0].1 - line.spans[0].0, line.spans[2].1 - line.spans[2].0);
        assert!((a / b - 10.0 / 8.0).abs() < 1e-6, "{a} {b}");
    }

    #[test]
    fn a_moment_shows_what_has_been_drawn() {
        let pen = Pen::default();
        let frames = stills(TWO_LINES, &pen, 4).unwrap();
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].matches("<path").count(), 0);
        assert_eq!(frames[4].matches("<path").count(), 3);
        assert!(frames.iter().all(|f| f.contains("<circle")));
        assert!(frames.windows(2).all(|w| w[0].matches("<path").count() <= w[1].matches("<path").count()));
    }

    #[test]
    fn the_animation_needs_no_script_and_keeps_its_times_in_order() {
        let svg = animation(TWO_LINES, &Pen::default()).unwrap();
        assert_eq!(svg.matches("<path").count(), 3);
        assert!(svg.contains("<animateMotion") && !svg.contains("<script"));
        let times = svg.split("<animateMotion").nth(1).unwrap().split(r#"keyTimes=""#).nth(1).unwrap().split('"').next().unwrap();
        let numbers: Vec<f64> = times.split(';').map(|v| v.parse().unwrap()).collect();
        assert!(numbers.windows(2).all(|w| w[0] <= w[1]) && numbers[0] == 0.0 && numbers[numbers.len() - 1] == 1.0, "{times}");
    }

    #[test]
    fn an_empty_figure_is_refused_with_a_reason() {
        let empty = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10"></svg>"#;
        assert!(animation(empty, &Pen::default()).unwrap_err().to_string().contains("nothing"));
        assert!(animation(TWO_LINES, &Pen { seconds: 0.0, ..Pen::default() }).is_err());
    }

    #[test]
    fn the_figure_fits_the_paper() {
        let (all, _) = strokes(TWO_LINES).unwrap();
        let mut a = all;
        on_paper(&mut a, (26.458, 26.458), (13.229, 100.0));
        assert!(a.iter().flatten().all(|p| p.0 >= -0.01 && p.0 <= 13.3 && p.1 >= 0.0 && p.1 <= 100.0));
    }
}
