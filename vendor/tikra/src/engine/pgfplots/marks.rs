//! Plot marks, as `pgflibraryplothandlers` and `pgflibraryplotmarks` draw them: each mark is a
//! path around the origin of size `\pgfplotmarksize`, then stroked, filled, or both.

use std::f64::consts::FRAC_1_SQRT_2;

use crate::engine::path::arc_segments;
use crate::scene::{Point, Segment, Transform};

/// How a mark is used: `\pgfusepathqstroke`, `qfill` or `qfillstroke`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Use {
    Stroke,
    Fill,
    FillStroke,
}

fn polar(angle: f64, r: f64) -> Point {
    let (s, c) = angle.to_radians().sin_cos();
    Point::new(r * c, r * s)
}

fn circle(r: f64) -> Vec<Segment> {
    let mut v = vec![Segment::MoveTo(Point::new(r, 0.0))];
    v.extend(arc_segments(Point::default(), r, r, 0.0, 360.0, &Transform::default()));
    v.push(Segment::Close);
    v
}

fn polygon(points: &[Point]) -> Vec<Segment> {
    let mut v = vec![Segment::MoveTo(points[0])];
    v.extend(points[1..].iter().map(|p| Segment::LineTo(*p)));
    v.push(Segment::Close);
    v
}

fn lines(pairs: &[(Point, Point)]) -> Vec<Segment> {
    pairs.iter().flat_map(|(a, b)| [Segment::MoveTo(*a), Segment::LineTo(*b)]).collect()
}

/// The path of mark `name` with size `r` (cm), and how it is used. None for an unknown mark.
pub fn mark(name: &str, r: f64) -> Option<(Vec<Segment>, Use)> {
    let p = Point::new;
    let d = FRAC_1_SQRT_2 * r;
    let cross = || lines(&[(p(-d, -d), p(d, d)), (p(-d, d), p(d, -d))]);
    let plus = || lines(&[(p(-r, 0.0), p(r, 0.0)), (p(0.0, r), p(0.0, -r))]);
    Some(match name.trim() {
        "*" => (circle(r), Use::FillStroke),
        "o" => (circle(r), Use::Stroke),
        "ball" => (circle(r), Use::FillStroke),
        "+" => (plus(), Use::Stroke),
        "x" => (cross(), Use::Stroke),
        "-" => (lines(&[(p(r, 0.0), p(-r, 0.0))]), Use::Stroke),
        "|" => (lines(&[(p(0.0, r), p(0.0, -r))]), Use::Stroke),
        "asterisk" => (lines(&[(p(0.0, -r), p(0.0, r)), (polar(30.0, r), polar(210.0, r)), (polar(-30.0, r), polar(-210.0, r))]), Use::Stroke),
        "star" => (lines(&[90.0, 18.0, -54.0, 234.0, 162.0].map(|a| (Point::default(), polar(a, r)))), Use::Stroke),
        "10-pointed star" => (lines(&[90.0, 18.0, -54.0, 234.0, 162.0].map(|a| (polar(a, -r), polar(a, r)))), Use::Stroke),
        "Mercedes star" => (lines(&[90.0, -30.0, -150.0].map(|a| (polar(a, r), Point::default()))), Use::Stroke),
        "Mercedes star flipped" => (lines(&[-90.0, 30.0, 150.0].map(|a| (polar(a, r), Point::default()))), Use::Stroke),
        "oplus" | "oplus*" => {
            let mut v = circle(r);
            v.extend(plus());
            (v, if name.ends_with('*') { Use::FillStroke } else { Use::Stroke })
        }
        "otimes" | "otimes*" => {
            let mut v = circle(r);
            v.extend(cross());
            (v, if name.ends_with('*') { Use::FillStroke } else { Use::Stroke })
        }
        "square" | "square*" | "cube" | "cube*" => (polygon(&[p(-r, -r), p(r, -r), p(r, r), p(-r, r)]), if name.ends_with('*') { Use::FillStroke } else { Use::Stroke }),
        "triangle" | "triangle*" => (polygon(&[p(0.0, r), polar(-30.0, r), polar(-150.0, r)]), if name.ends_with('*') { Use::FillStroke } else { Use::Stroke }),
        "diamond" | "diamond*" => (polygon(&[p(0.0, r), p(0.75 * r, 0.0), p(0.0, -r), p(-0.75 * r, 0.0)]), if name.ends_with('*') { Use::FillStroke } else { Use::Stroke }),
        "pentagon" | "pentagon*" => (polygon(&[90.0, 18.0, -54.0, 234.0, 162.0].map(|a| polar(a, r))), if name.ends_with('*') { Use::FillStroke } else { Use::Stroke }),
        "halfcircle" | "halfcircle*" => {
            let mut v = vec![Segment::MoveTo(p(r, 0.0))];
            v.extend(arc_segments(Point::default(), r, r, 0.0, 180.0, &Transform::default()));
            v.push(Segment::Close);
            (v, if name.ends_with('*') { Use::FillStroke } else { Use::Stroke })
        }
        "halfsquare*" | "halfsquare right*" | "halfsquare left*" => (polygon(&[p(-r, -r), p(r, -r), p(r, r), p(-r, r)]), Use::FillStroke),
        "halfdiamond*" => (polygon(&[p(0.0, r), p(0.75 * r, 0.0), p(0.0, -r), p(-0.75 * r, 0.0)]), Use::FillStroke),
        "heart" => (circle(r), Use::Fill),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_marks() {
        for m in ["*", "o", "+", "x", "square*", "triangle", "diamond*", "star", "otimes*", "|", "pentagon*"] {
            assert!(mark(m, 0.07).is_some(), "{m}");
        }
        assert!(mark("nonsense", 0.07).is_none());
        assert_eq!(mark("*", 0.07).unwrap().1, Use::FillStroke);
    }
}
