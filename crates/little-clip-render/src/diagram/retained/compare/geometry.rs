use super::{USER_TOLERANCE, usvg};
use usvg::tiny_skia_path::{PathSegment, Point};

#[derive(Clone, Copy)]
pub(super) struct Tolerance {
    x: f64,
    y: f64,
}

impl Tolerance {
    pub(super) fn new(base_scale: [f64; 2], revised_scale: [f64; 2]) -> Self {
        Self {
            x: f64::from(USER_TOLERANCE) * base_scale[0].min(revised_scale[0]),
            y: f64::from(USER_TOLERANCE) * base_scale[1].min(revised_scale[1]),
        }
    }

    fn point(self, a: Point, b: Point) -> bool {
        self.viewport_size([a.x, a.y], [b.x, b.y])
    }

    pub(super) fn viewport_size(self, a: [f32; 2], b: [f32; 2]) -> bool {
        (f64::from(a[0]) - f64::from(b[0])).abs() <= self.x
            && (f64::from(a[1]) - f64::from(b[1])).abs() <= self.y
    }
}

pub(super) fn opacity(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.000_01
}

fn fill(path: &usvg::Path) -> Option<&usvg::Fill> {
    path.fill().filter(|fill| fill.opacity().get() > 0.0)
}

fn stroke(path: &usvg::Path) -> Option<&usvg::Stroke> {
    path.stroke().filter(|stroke| stroke.opacity().get() > 0.0)
}

pub(super) fn visible(path: &usvg::Path) -> bool {
    path.is_visible() && (fill(path).is_some() || stroke(path).is_some())
}

pub(super) fn dash_intervals(path: &usvg::Path) -> f64 {
    let Some(dashes) = stroke(path).and_then(usvg::Stroke::dasharray) else {
        return 0.0;
    };
    let cycle: f64 = dashes.iter().copied().map(f64::from).sum();
    let polygon_length: f64 = path
        .data()
        .points()
        .windows(2)
        .map(|pair| {
            let x = f64::from(pair[0].x) - f64::from(pair[1].x);
            let y = f64::from(pair[0].y) - f64::from(pair[1].y);
            x.hypot(y)
        })
        .sum();
    // The control polygon bounds curve length. Doubling covers the return
    // edge of closed subpaths, which has no point in the stored datum.
    (2.0 * polygon_length / cycle + 1.0) * dashes.len() as f64
}

pub(super) fn path(a: &usvg::Path, b: &usvg::Path, tolerance: Tolerance) -> bool {
    let ta = a.abs_transform();
    let tb = b.abs_transform();
    let strokes_match = match (stroke(a), stroke(b)) {
        (None, None) => true,
        (Some(a), Some(b)) => stroke_style(a, b, ta, tb, tolerance),
        _ => false,
    };
    let fills_match = match (fill(a), fill(b)) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.rule() == b.rule()
                && opacity(a.opacity().get(), b.opacity().get())
                && paint(a.paint(), b.paint(), ta, tb, tolerance)
        }
        _ => false,
    };
    strokes_match
        && fills_match
        && (stroke(a).is_none() || fill(a).is_none() || a.paint_order() == b.paint_order())
        && a.rendering_mode() == b.rendering_mode()
        && path_data(a, b, tolerance)
}

fn stroke_style(
    a: &usvg::Stroke,
    b: &usvg::Stroke,
    ta: usvg::Transform,
    tb: usvg::Transform,
    tolerance: Tolerance,
) -> bool {
    let widths = [f64::from(a.width().get()), f64::from(b.width().get())];
    // Compare the support of the transformed stroke pen along three axes.
    // These determine the symmetric metric of an affine image of a circle.
    let supports = |t: usvg::Transform, width| {
        let [sx, kx, ky, sy] = [t.sx, t.kx, t.ky, t.sy].map(f64::from);
        [sx.hypot(kx), ky.hypot(sy), (sx + ky).hypot(kx + sy)].map(|n| n * width)
    };
    let scale_a = supports(ta, widths[0]);
    let scale_b = supports(tb, widths[1]);
    let stroke_matches = scale_a
        .into_iter()
        .zip(scale_b)
        .zip([tolerance.x, tolerance.y, tolerance.x + tolerance.y])
        .all(|((a, b), epsilon)| (a - b).abs() <= epsilon);
    let dash_matches = |a: f32, b: f32| {
        let a = f64::from(a) / widths[0];
        let b = f64::from(b) / widths[1];
        (a * scale_a[0] - b * scale_b[0]).abs() <= tolerance.x
            && (a * scale_a[1] - b * scale_b[1]).abs() <= tolerance.y
    };
    let dashes_match = match (a.dasharray(), b.dasharray()) {
        (None, None) => true,
        (Some(a_dashes), Some(b_dashes)) => {
            a_dashes.len() == b_dashes.len()
                && a_dashes
                    .iter()
                    .zip(b_dashes)
                    .all(|(&a, &b)| dash_matches(a, b))
                && dash_matches(a.dashoffset(), b.dashoffset())
        }
        _ => false,
    };
    stroke_matches
        && dashes_match
        && a.linecap() == b.linecap()
        && a.linejoin() == b.linejoin()
        && opacity(a.miterlimit().get(), b.miterlimit().get())
        && opacity(a.opacity().get(), b.opacity().get())
        && paint(a.paint(), b.paint(), ta, tb, tolerance)
}

fn paint(
    a: &usvg::Paint,
    b: &usvg::Paint,
    ta: usvg::Transform,
    tb: usvg::Transform,
    tolerance: Tolerance,
) -> bool {
    match (a, b) {
        (usvg::Paint::Color(a), usvg::Paint::Color(b)) => a == b,
        (usvg::Paint::LinearGradient(a), usvg::Paint::LinearGradient(b)) => {
            gradient(a, b)
                && points(
                    linear_points(a),
                    linear_points(b),
                    ta.pre_concat(a.transform()),
                    tb.pre_concat(b.transform()),
                    tolerance,
                )
        }
        (usvg::Paint::RadialGradient(a), usvg::Paint::RadialGradient(b)) => {
            gradient(a, b)
                && points(
                    radial_points(a),
                    radial_points(b),
                    ta.pre_concat(a.transform()),
                    tb.pre_concat(b.transform()),
                    tolerance,
                )
        }
        _ => false,
    }
}

fn gradient(a: &usvg::BaseGradient, b: &usvg::BaseGradient) -> bool {
    a.spread_method() == b.spread_method()
        && a.stops().len() == b.stops().len()
        && a.stops().iter().zip(b.stops()).all(|(a, b)| {
            a.color() == b.color()
                && opacity(a.opacity().get(), b.opacity().get())
                && opacity(a.offset().get(), b.offset().get())
        })
}

fn linear_points(g: &usvg::LinearGradient) -> [Point; 3] {
    [
        Point::from_xy(g.x1(), g.y1()),
        Point::from_xy(g.x2(), g.y2()),
        Point::from_xy(g.x1() - (g.y2() - g.y1()), g.y1() + (g.x2() - g.x1())),
    ]
}

fn radial_points(g: &usvg::RadialGradient) -> [Point; 6] {
    [
        Point::from_xy(g.cx(), g.cy()),
        Point::from_xy(g.fx(), g.fy()),
        Point::from_xy(g.cx() + g.r().get(), g.cy()),
        Point::from_xy(g.cx(), g.cy() + g.r().get()),
        Point::from_xy(g.fx() + g.fr().get(), g.fy()),
        Point::from_xy(g.fx(), g.fy() + g.fr().get()),
    ]
}

fn points<const N: usize>(
    mut a: [Point; N],
    mut b: [Point; N],
    ta: usvg::Transform,
    tb: usvg::Transform,
    tolerance: Tolerance,
) -> bool {
    ta.map_points(&mut a);
    tb.map_points(&mut b);
    a.into_iter().zip(b).all(|(a, b)| tolerance.point(a, b))
}

fn path_data(a: &usvg::Path, b: &usvg::Path, tolerance: Tolerance) -> bool {
    let (Some(a_bounds), Some(b_bounds)) = (a.abs_bounding_box(), b.abs_bounding_box()) else {
        return false;
    };
    if !tolerance.point(
        Point::from_xy(a_bounds.left(), a_bounds.top()),
        Point::from_xy(b_bounds.left(), b_bounds.top()),
    ) || !tolerance.point(
        Point::from_xy(a_bounds.right(), a_bounds.bottom()),
        Point::from_xy(b_bounds.right(), b_bounds.bottom()),
    ) {
        return false;
    }

    let (mut a_segments, mut b_segments) = (a.data().segments(), b.data().segments());
    a_segments.set_auto_close(true);
    b_segments.set_auto_close(true);
    let (ta, tb) = (a.abs_transform(), b.abs_transform());
    let (mut pa, mut pb) = (Point::zero(), Point::zero());
    loop {
        let matches = match (a_segments.next(), b_segments.next()) {
            (None, None) => return true,
            (Some(PathSegment::Close), Some(PathSegment::Close)) => true,
            (Some(PathSegment::MoveTo(a)), Some(PathSegment::MoveTo(b))) => {
                (pa, pb) = (a, b);
                points([a], [b], ta, tb, tolerance)
            }
            (Some(a), Some(b)) => match (as_cubic(pa, a), as_cubic(pb, b)) {
                (Some(a), Some(b)) => {
                    (pa, pb) = (a[2], b[2]);
                    points(a, b, ta, tb, tolerance)
                }
                _ => false,
            },
            _ => false,
        };
        if !matches {
            return false;
        }
    }
}

fn as_cubic(start: Point, segment: PathSegment) -> Option<[Point; 3]> {
    // Degree elevation makes equivalent line, quadratic and cubic commands
    // comparable without flattening curves or choosing a sampling density.
    let toward = |a: Point, b: Point, fraction| {
        Point::from_xy(a.x + (b.x - a.x) * fraction, a.y + (b.y - a.y) * fraction)
    };
    Some(match segment {
        PathSegment::LineTo(end) => [
            toward(start, end, 1.0 / 3.0),
            toward(end, start, 1.0 / 3.0),
            end,
        ],
        PathSegment::QuadTo(control, end) => [
            toward(start, control, 2.0 / 3.0),
            toward(end, control, 2.0 / 3.0),
            end,
        ],
        PathSegment::CubicTo(a, b, end) => [a, b, end],
        _ => return None,
    })
}
