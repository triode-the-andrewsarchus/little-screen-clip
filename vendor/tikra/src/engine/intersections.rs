//! The intersections library: `name path=a`, then `name intersections={of=a and b}` makes the
//! coordinates `intersection-1`, `intersection-2`… (or `name=`, `by=`), and `total=\t`.

use crate::scene::{Point, Segment};
use crate::tex::Token;
use crate::Error;

use super::nodes::{NodeSpec, OnPath};
use super::{macro_from, Engine};

/// Pieces a curve is cut into to find where it crosses another path.
const CURVE_PIECES: usize = 64;

/// The path as polylines (one per subpath), curves flattened.
fn polylines(segs: &[Segment]) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut start = Point::default();
    let mut current = Point::default();
    for s in segs {
        match *s {
            Segment::MoveTo(p) => {
                out.push(vec![p]);
                start = p;
                current = p;
            }
            Segment::LineTo(p) => {
                out.last_mut().expect("a path starts with a move").push(p);
                current = p;
            }
            Segment::CurveTo(c1, c2, p) => {
                let a = current;
                let line = out.last_mut().expect("a path starts with a move");
                for k in 1..=CURVE_PIECES {
                    let t = k as f64 / CURVE_PIECES as f64;
                    let u = 1.0 - t;
                    line.push(a.scale(u * u * u).plus(c1.scale(3.0 * u * u * t)).plus(c2.scale(3.0 * u * t * t)).plus(p.scale(t * t * t)));
                }
                current = p;
            }
            Segment::Close => {
                if let Some(line) = out.last_mut() {
                    line.push(start);
                }
                current = start;
            }
        }
    }
    out
}

/// Where segment ab meets segment cd: the point and the position along ab.
fn segment_cross(a: Point, b: Point, c: Point, d: Point) -> Option<(Point, f64)> {
    let r = b.minus(a);
    let s = d.minus(c);
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-14 {
        return None;
    }
    let ca = c.minus(a);
    let t = (ca.x * s.y - ca.y * s.x) / den;
    let u = (ca.x * r.y - ca.y * r.x) / den;
    let eps = 1e-9;
    if (-eps..=1.0 + eps).contains(&t) && (-eps..=1.0 + eps).contains(&u) {
        Some((a.plus(r.scale(t)), t))
    } else {
        None
    }
}

/// Cap on pieces per path: a longer path is not searched (it would take too long).
const MAX_PIECES: usize = 200_000;

/// The points where two paths cross, in order along the first. The second path's pieces are put in
/// a grid of buckets, so each piece of the first is only compared with the pieces near it.
pub fn intersections(first: &[Segment], second: &[Segment]) -> Vec<Point> {
    let (p, q) = (polylines(first), polylines(second));
    let pieces = |v: &Vec<Vec<Point>>| -> Vec<(Point, Point)> { v.iter().flat_map(|l| l.windows(2).map(|w| (w[0], w[1]))).collect() };
    let (pp, qq) = (pieces(&p), pieces(&q));
    if pp.is_empty() || qq.is_empty() || pp.len() > MAX_PIECES || qq.len() > MAX_PIECES {
        return Vec::new();
    }
    let (mut lo, mut hi) = (Point::new(f64::INFINITY, f64::INFINITY), Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
    for (a, b) in qq.iter().chain(pp.iter()) {
        for c in [a, b] {
            lo = Point::new(lo.x.min(c.x), lo.y.min(c.y));
            hi = Point::new(hi.x.max(c.x), hi.y.max(c.y));
        }
    }
    let n = ((qq.len() as f64).sqrt().ceil() as usize).clamp(1, 512);
    let (w, h) = (((hi.x - lo.x) / n as f64).max(1e-9), ((hi.y - lo.y) / n as f64).max(1e-9));
    let cell = |c: Point| -> (usize, usize) { (((c.x - lo.x) / w).floor().clamp(0.0, (n - 1) as f64) as usize, ((c.y - lo.y) / h).floor().clamp(0.0, (n - 1) as f64) as usize) };
    let mut grid: Vec<Vec<usize>> = vec![Vec::new(); n * n];
    for (k, (a, b)) in qq.iter().enumerate() {
        let (ca, cb) = (cell(*a), cell(*b));
        for i in ca.0.min(cb.0)..=ca.0.max(cb.0) {
            for j in ca.1.min(cb.1)..=ca.1.max(cb.1) {
                grid[j * n + i].push(k);
            }
        }
    }
    let mut found: Vec<(usize, f64, Point)> = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for (index, (a, b)) in pp.iter().enumerate() {
        let (ca, cb) = (cell(*a), cell(*b));
        seen.clear();
        for i in ca.0.min(cb.0)..=ca.0.max(cb.0) {
            for j in ca.1.min(cb.1)..=ca.1.max(cb.1) {
                for &k in &grid[j * n + i] {
                    if seen.contains(&k) {
                        continue;
                    }
                    seen.push(k);
                    let (c, d) = qq[k];
                    if let Some((pt, t)) = segment_cross(*a, *b, c, d) {
                        found.push((index, t, pt));
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // a crossing at the joint of two pieces is found twice
    let mut points: Vec<Point> = Vec::new();
    for (_, _, pt) in found {
        if !points.iter().any(|x| x.minus(pt).norm() < 1e-6) {
            points.push(pt);
        }
    }
    points
}

/// `name intersections={of=a and b, name=i, total=\t, by={x,y}, sort by=b}`
pub fn name_intersections(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let Some(v) = v else { return Ok(()) };
    let mut of = None;
    let mut prefix = "intersection".to_string();
    let mut total: Option<Vec<Token>> = None;
    let mut by: Vec<String> = Vec::new();
    let mut sort_by: Option<String> = None;
    for item in crate::keys::split_list(&v) {
        let value = match item.value {
            Some(x) => Some(e.tex.expand_to_string(x.clone()).map(|s| (s, x))?),
            None => None,
        };
        match (item.key.as_str(), value) {
            ("of", Some((s, _))) => of = s.split_once(" and ").map(|(a, b)| (a.trim().to_string(), b.trim().to_string())),
            ("name", Some((s, _))) => prefix = s.trim().to_string(),
            ("total", Some((_, t))) => total = Some(t),
            ("by", Some((s, _))) => by = crate::keys::split_top_level(&s).into_iter().map(|x| x.trim().to_string()).collect(),
            ("sort by", Some((s, _))) => sort_by = Some(s.trim().to_string()),
            (other, _) => e.warn(format!("name intersections: '{other}' not supported: ignored")),
        }
    }
    let Some((a, b)) = of else {
        e.warn("name intersections without of=a and b: ignored");
        return Ok(());
    };
    let (Some(pa), Some(pb)) = (e.named_paths.get(&a).cloned(), e.named_paths.get(&b).cloned()) else {
        return Err(Error::Figure(format!("name intersections: no path named '{}'", if e.named_paths.contains_key(&a) { &b } else { &a })));
    };
    let points = if sort_by.as_deref() == Some(b.as_str()) { intersections(&pb, &pa) } else { intersections(&pa, &pb) };
    for (k, p) in points.iter().enumerate() {
        let mut names = vec![(format!("{prefix}-{}", k + 1), String::new())];
        if let Some(n) = by.get(k) {
            // `[label=above:$C$]C`: the coordinate's options and name
            let (options, name) = match n.strip_prefix('[').and_then(|r| r.rsplit_once(']')) {
                Some((o, name)) => (o.to_string(), name.trim().to_string()),
                None => (String::new(), n.trim().to_string()),
            };
            names.push((name, options));
        }
        for (name, options) in names {
            let items = e.node(NodeSpec {
                options: super::tikra_tokens(&options),
                name: Some(name),
                at: Some(*p),
                text: Vec::new(),
                place: OnPath::Point(*p),
                point: *p,
                coordinate: true,
                default_pos: None,
            })?;
            e.canvas.items.extend(items);
        }
    }
    if let Some(t) = total {
        if let Some(name) = t.into_iter().find(|x| !x.is_space()) {
            e.tex.set_meaning(&name, macro_from(&points.len().to_string()), false);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_curves_cross() {
        let a = vec![Segment::MoveTo(Point::new(0.0, 0.0)), Segment::LineTo(Point::new(2.0, 2.0))];
        let b = vec![Segment::MoveTo(Point::new(0.0, 2.0)), Segment::LineTo(Point::new(2.0, 0.0))];
        let p = intersections(&a, &b);
        assert_eq!(p.len(), 1);
        assert!(p[0].minus(Point::new(1.0, 1.0)).norm() < 1e-9);
        // a circle (four curves) and a line through its center: two points, in order along the line
        let c = Point::new(0.0, 0.0);
        let mut circle = vec![Segment::MoveTo(Point::new(1.0, 0.0))];
        circle.extend(crate::engine::path::arc_segments(c, 1.0, 1.0, 0.0, 360.0, &crate::scene::Transform::default()));
        let line = vec![Segment::MoveTo(Point::new(-2.0, 0.5)), Segment::LineTo(Point::new(2.0, 0.5))];
        let p = intersections(&line, &circle);
        assert_eq!(p.len(), 2);
        assert!(p[0].x < p[1].x && (p[0].x + 0.866).abs() < 1e-3);
    }
}
