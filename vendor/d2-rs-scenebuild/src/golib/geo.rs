// SPDX-License-Identifier: MPL-2.0
// Port of D2 v0.9.0 lib/geo (point.go, box.go, vector.go, segment.go,
// route.go, ellipse.go, bezier.go, math.go, orientation.go, spacing.go).
// bezier.go is itself a translation of
// https://www.particleincell.com/2013/cubic-line-intersection/ and of Robert
// D. Miller's Bezier evaluation from Graphics Gems 5.
//! Faithful port of D2's `lib/geo` package.
//!
//! Floating-point evaluation order follows the Go source. The pinned Go 1.27
//! ARM64 compiler contracts `a + x*y`, `a - x*y` and `x*y - a` into
//! FMADDD/FMSUBD/FNMSUBD whenever one operand of the add/sub is an FMULD
//! value (`cmd/compile/internal/ssa/_gen/ARM64.rules`, no single-use
//! condition). Every contraction site below was read from
//! `go build -gcflags=-S` output of the pinned toolchain and is marked
//! `ARM64:`; other targets keep Go's separate (amd64) rounding. Products by a
//! power of two (`x/2` lowered to `x*0.5`) are exact, so contracting them is
//! unobservable and they are written plainly.

#![allow(dead_code)]

use d2_rs_core::stable_sort::StableSort;
use std::fmt::Write as _;

pub use d2_rs_render::go_float::gomath;
pub use d2_rs_render::go_float::{go_fmadd, go_fmsub, go_fnmsub};

/// Go `fmt` `%v` for a float64 (strconv 'g', shortest, exponent threshold 6,
/// at least two exponent digits). Used by `SvgPathContext` command strings and
/// `ToString` helpers.
pub fn go_format_v(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    // Rust's `{:e}` yields the shortest round-tripping digits, as strconv's
    // shortest mode does.
    let sci = format!("{v:e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if digits == "0" {
        out.push('0');
        return out;
    }
    if !(-4..6).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let _ = write!(out, "{:02}", exp.abs());
    } else if exp >= 0 {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(&digits);
            out.extend(std::iter::repeat_n('0', int_len - digits.len()));
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    } else {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-exp - 1) as usize));
        out.push_str(&digits);
    }
    out
}

// ---------------------------------------------------------------- math.go

/// math.go `EuclideanDistance`.
pub fn euclidean_distance(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    if x1 == x2 {
        (y1 - y2).abs()
    } else if y1 == y2 {
        (x1 - x2).abs()
    } else {
        // ARM64: FMULD dx*dx, FMADDD dy*dy + that.
        let dx = x1 - x2;
        let dy = y1 - y2;
        go_fmadd(dy, dy, dx * dx).sqrt()
    }
}

/// math.go `PrecisionCompare`.
pub fn precision_compare(a: f64, b: f64, e: f64) -> i32 {
    if (a - b).abs() < e {
        return 0;
    }
    if a < b {
        return -1;
    }
    1
}

/// math.go `TruncateDecimals`: `float64(int(v*1000)) / 1000`.
pub fn truncate_decimals(v: f64) -> f64 {
    go_float_to_int(v * 1000.0) as f64 / 1000.0
}

/// Go's `int(f)` on arm64 (FCVTZS): truncation, saturating, NaN -> 0.
#[inline]
fn go_float_to_int(v: f64) -> i64 {
    v as i64
}

/// math.go `Sign`.
pub fn sign(i: f64) -> i32 {
    if i < 0.0 {
        -1
    } else if i > 0.0 {
        1
    } else {
        0
    }
}

// --------------------------------------------------------- orientation.go

/// orientation.go `Orientation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Orientation {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Right,
    Bottom,
    Left,
    None,
}

impl Orientation {
    /// orientation.go `Orientation.ToString`.
    pub fn to_string(self) -> &'static str {
        match self {
            Orientation::TopLeft => "TopLeft",
            Orientation::TopRight => "TopRight",
            Orientation::BottomLeft => "BottomLeft",
            Orientation::BottomRight => "BottomRight",
            Orientation::Top => "Top",
            Orientation::Right => "Right",
            Orientation::Bottom => "Bottom",
            Orientation::Left => "Left",
            Orientation::None => "",
        }
    }

    /// orientation.go `Orientation.SameSide`.
    pub fn same_side(self, o2: Orientation) -> bool {
        use Orientation::*;
        let sides: [[Orientation; 3]; 4] = [
            [TopLeft, Top, TopRight],
            [BottomLeft, Bottom, BottomRight],
            [Left, TopLeft, BottomLeft],
            [Right, TopRight, BottomRight],
        ];
        sides
            .iter()
            .any(|same| same.contains(&self) && same.contains(&o2))
    }

    /// orientation.go `Orientation.IsDiagonal`.
    pub fn is_diagonal(self) -> bool {
        matches!(
            self,
            Orientation::TopLeft
                | Orientation::TopRight
                | Orientation::BottomLeft
                | Orientation::BottomRight
        )
    }

    /// orientation.go `Orientation.IsHorizontal`.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Orientation::Left | Orientation::Right)
    }

    /// orientation.go `Orientation.IsVertical`.
    pub fn is_vertical(self) -> bool {
        matches!(self, Orientation::Top | Orientation::Bottom)
    }

    /// orientation.go `Orientation.GetOpposite`.
    pub fn get_opposite(self) -> Orientation {
        use Orientation::*;
        match self {
            TopLeft => BottomRight,
            TopRight => BottomLeft,
            BottomLeft => TopRight,
            BottomRight => TopLeft,
            Top => Bottom,
            Bottom => Top,
            Right => Left,
            Left => Right,
            None => None,
        }
    }
}

// ------------------------------------------------------------- spacing.go

/// spacing.go `Spacing`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spacing {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

// --------------------------------------------------------------- point.go

/// point.go `RelativePoint`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelativePoint {
    pub x_percentage: f64,
    pub y_percentage: f64,
}

impl RelativePoint {
    /// point.go `NewRelativePoint` (truncates to 3 decimals).
    pub fn new(x_percentage: f64, y_percentage: f64) -> Self {
        RelativePoint {
            x_percentage: truncate_decimals(x_percentage),
            y_percentage: truncate_decimals(y_percentage),
        }
    }
}

/// point.go `Point`. Go passes `*Point`; Rust copies explicitly where Go
/// calls `Copy()` and mutates in place where Go mutates through the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// point.go `Points`.
pub type Points = Vec<Point>;

impl Point {
    /// point.go `NewPoint`.
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    /// point.go `Point.Equals` (non-nil receiver and argument).
    pub fn equals(&self, p2: &Point) -> bool {
        self.x == p2.x && self.y == p2.y
    }

    /// point.go `Point.Compare`.
    pub fn compare(&self, p2: &Point) -> i32 {
        let x_compare = sign(self.x - p2.x);
        if x_compare == 0 {
            return sign(self.y - p2.y);
        }
        x_compare
    }

    /// point.go `Point.Copy`.
    pub fn copy(&self) -> Point {
        *self
    }

    /// point.go `Point.GetOrientation`.
    pub fn get_orientation(&self, p_to: &Point) -> Orientation {
        if self.y < p_to.y {
            if self.x < p_to.x {
                return Orientation::TopLeft;
            }
            if self.x > p_to.x {
                return Orientation::TopRight;
            }
            return Orientation::Top;
        }
        if self.y > p_to.y {
            if self.x < p_to.x {
                return Orientation::BottomLeft;
            }
            if self.x > p_to.x {
                return Orientation::BottomRight;
            }
            return Orientation::Bottom;
        }
        if self.x < p_to.x {
            return Orientation::Left;
        }
        if self.x > p_to.x {
            return Orientation::Right;
        }
        Orientation::None
    }

    /// point.go `Point.ToString`: `fmt.Sprintf("(%v, %v)", X, Y)`.
    pub fn to_string(&self) -> String {
        format!("({}, {})", go_format_v(self.x), go_format_v(self.y))
    }

    /// point.go `Point.DistanceToLine`.
    pub fn distance_to_line(&self, p1: &Point, p2: &Point) -> f64 {
        let a = self.x - p1.x;
        let b = self.y - p1.y;
        let c = p2.x - p1.x;
        let d = p2.y - p1.y;

        // ARM64: dot = FMADDD(b*d + round(a*c)); len_sq = FMADDD(d*d + round(c*c)).
        let dot = go_fmadd(b, d, a * c);
        let len_sq = go_fmadd(d, d, c * c);

        let mut param = -1.0;
        if len_sq != 0.0 {
            param = dot / len_sq;
        }

        let (xx, yy) = if param < 0.0 {
            (p1.x, p1.y)
        } else if param > 1.0 {
            (p2.x, p2.y)
        } else {
            // ARM64: FMADDD p1.X + param*c.
            (go_fmadd(param, c, p1.x), go_fmadd(param, d, p1.y))
        };

        let dx = self.x - xx;
        let dy = self.y - yy;
        // ARM64: FMADDD dy*dy + round(dx*dx).
        go_fmadd(dy, dy, dx * dx).sqrt()
    }

    /// point.go `Point.AddVector`: `start.ToVector().Add(v).ToPoint()`.
    pub fn add_vector(&self, v: &Vector) -> Point {
        self.to_vector().add(v).to_point()
    }

    /// point.go `Point.VectorTo`: `endpoint.ToVector().Minus(start.ToVector())`.
    pub fn vector_to(&self, endpoint: &Point) -> Vector {
        endpoint.to_vector().minus(&self.to_vector())
    }

    /// point.go `Point.FormattedCoordinates`: `"%d,%d"` of `int(X), int(Y)`.
    pub fn formatted_coordinates(&self) -> String {
        format!("{},{}", go_float_to_int(self.x), go_float_to_int(self.y))
    }

    /// point.go `Point.OnOrthogonalSegment`.
    pub fn on_orthogonal_segment(&self, a: &Point, b: &Point) -> bool {
        if a.x < b.x {
            if self.x < a.x || b.x < self.x {
                return false;
            }
        } else if self.x < b.x || a.x < self.x {
            return false;
        }
        if a.y < b.y {
            if self.y < a.y || b.y < self.y {
                return false;
            }
        } else if self.y < b.y || a.y < self.y {
            return false;
        }
        true
    }

    /// point.go `Point.ToVector`.
    pub fn to_vector(&self) -> Vector {
        Vector(vec![self.x, self.y])
    }

    /// point.go `Point.Transpose`.
    pub fn transpose(&mut self) {
        std::mem::swap(&mut self.x, &mut self.y);
    }

    /// point.go `Point.Interpolate`: `a.X*(1.0-t)+b.X*t`.
    pub fn interpolate(&self, b: &Point, t: f64) -> Point {
        // ARM64: round(b.X*t), then FMADDD a.X*(1-t) + that (the first
        // product is the fused one here).
        Point::new(
            go_fmadd(self.x, 1.0 - t, b.x * t),
            go_fmadd(self.y, 1.0 - t, b.y * t),
        )
    }

    /// point.go `Point.TruncateFloat32`.
    pub fn truncate_float32(&mut self) {
        self.x = self.x as f32 as f64;
        self.y = self.y as f32 as f64;
    }

    /// point.go `Point.TruncateDecimals`.
    pub fn truncate_decimals(&mut self) {
        self.x = truncate_decimals(self.x);
        self.y = truncate_decimals(self.y);
    }
}

/// point.go `Points.Equals` (set containment of `other` in `ps`, as Go does).
pub fn points_equals(ps: &[Point], other: &[Point]) -> bool {
    // Go keys a map by the Point value; float == equality (NaN never found,
    // -0 == 0 as map keys) matches a linear `==` scan.
    other
        .iter()
        .all(|o| ps.iter().any(|p| p.x == o.x && p.y == o.y))
}

/// point.go `Points.GetMedian`.
pub fn points_get_median(ps: &[Point]) -> Point {
    let mut xs: Vec<f64> = ps.iter().map(|p| p.x).collect();
    let mut ys: Vec<f64> = ps.iter().map(|p| p.y).collect();
    // sort.Float64s orders NaN first; total_cmp would put -NaN first and
    // +NaN last, so sort with Go's less (x < y || isNaN(x) && !isNaN(y)).
    let go_less = |a: &f64, b: &f64| {
        if a < b || (a.is_nan() && !b.is_nan()) {
            std::cmp::Ordering::Less
        } else if b < a || (b.is_nan() && !a.is_nan()) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    };
    xs.stable_sort_by(go_less);
    ys.stable_sort_by(go_less);
    let middle_index = xs.len() / 2;
    let mut median_x = xs[middle_index];
    let mut median_y = ys[middle_index];
    if xs.len().is_multiple_of(2) {
        median_x += xs[middle_index - 1];
        median_x /= 2.0;
        median_y += ys[middle_index - 1];
        median_y /= 2.0;
    }
    Point::new(median_x, median_y)
}

/// point.go `Points.ToString`.
pub fn points_to_string(points: &[Point]) -> String {
    points
        .iter()
        .map(Point::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// point.go `IntersectionPoint`: intersection of segments u and v, or None.
pub fn intersection_point(u0: &Point, u1: &Point, v0: &Point, v1: &Point) -> Option<Point> {
    let udx = u1.x - u0.x;
    let vdx = v1.x - v0.x;
    let uvdx = v0.x - u0.x;
    let udy = u1.y - u0.y;
    let vdy = v1.y - v0.y;
    let uvdy = v0.y - u0.y;

    // ARM64: each difference of products rounds the first product and fuses
    // the second (FMULD + FMSUBD).
    let denom = go_fmsub(udx, vdy, udy * vdx);
    if denom == 0.0 {
        return None;
    }
    let s = go_fmsub(vdy, uvdx, vdx * uvdy) / denom;
    let t = go_fmsub(udy, uvdx, udx * uvdy) / denom;

    if s < 0.0 || s > 1.0 || t < 0.0 || t > 1.0 {
        return None;
    }
    // math.Round rounds half away from zero, as f64::round does.
    Some(Point::new(
        u0.x + (s * udx).round(),
        u0.y + (s * udy).round(),
    ))
}

/// point.go `RemovePoints`.
pub fn remove_points(points: &[Point], to_remove: &[bool]) -> Points {
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| !to_remove[*i])
        .map(|(_, p)| *p)
        .collect()
}

// -------------------------------------------------------------- vector.go

/// vector.go `Vector` (Go `[]float64`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vector(pub Vec<f64>);

impl Vector {
    /// vector.go `NewVector`.
    pub fn new(components: &[f64]) -> Vector {
        Vector(components.to_vec())
    }

    /// vector.go `NewVectorFromProperties`.
    pub fn from_properties(length: f64, angle_in_radians: f64) -> Vector {
        Vector(vec![
            length * gomath::sin(angle_in_radians),
            length * gomath::cos(angle_in_radians),
        ])
    }

    /// vector.go `Vector.AddLength`: `a.Unit().Multiply(a.Length() + length)`.
    pub fn add_length(&self, length: f64) -> Vector {
        self.unit().multiply(self.length() + length)
    }

    /// vector.go `Vector.Add`.
    pub fn add(&self, b: &Vector) -> Vector {
        Vector(self.0.iter().zip(&b.0).map(|(a, b)| a + b).collect())
    }

    /// vector.go `Vector.Minus`.
    pub fn minus(&self, b: &Vector) -> Vector {
        Vector(self.0.iter().zip(&b.0).map(|(a, b)| a - b).collect())
    }

    /// vector.go `Vector.Multiply`.
    pub fn multiply(&self, v: f64) -> Vector {
        Vector(self.0.iter().map(|a| a * v).collect())
    }

    /// vector.go `Vector.Length`.
    pub fn length(&self) -> f64 {
        let mut sum = 0.0;
        for &comp in &self.0 {
            // ARM64: `sum += comp * comp` is FMADDD.
            sum = go_fmadd(comp, comp, sum);
        }
        sum.sqrt()
    }

    /// vector.go `Vector.Unit`: `a.Multiply(1 / a.Length())`.
    pub fn unit(&self) -> Vector {
        self.multiply(1.0 / self.length())
    }

    /// vector.go `Vector.ToPoint`.
    pub fn to_point(&self) -> Point {
        Point::new(self.0[0], self.0[1])
    }

    /// vector.go `Vector.Radians`: atan2 truncated through float32.
    pub fn radians(&self) -> f64 {
        gomath::atan2(self.0[1], self.0[0]) as f32 as f64
    }

    /// vector.go `Vector.Degrees`: `a.Radians() * 180 / math.Pi`.
    pub fn degrees(&self) -> f64 {
        self.radians() * 180.0 / std::f64::consts::PI
    }

    /// vector.go `Vector.Reverse`.
    pub fn reverse(&self) -> Vector {
        self.multiply(-1.0)
    }
}

/// vector.go `getNormalVector`: (x1,y1)->(x2,y2) rotated 90 degrees left.
fn get_normal_vector(x1: f64, y1: f64, x2: f64, y2: f64) -> (f64, f64) {
    (y1 - y2, x2 - x1)
}

/// vector.go `GetUnitNormalVector`.
pub fn get_unit_normal_vector(x1: f64, y1: f64, x2: f64, y2: f64) -> (f64, f64) {
    let (normal_x, normal_y) = get_normal_vector(x1, y1, x2, y2);
    let length = euclidean_distance(x1, y1, x2, y2);
    (normal_x / length, normal_y / length)
}

// ----------------------------------------------------------------- box.go

/// box.go `Box`. Go holds `*Point` for TopLeft; every Go caller that keeps a
/// box copies it (`Box.Copy`/`TopLeft.Copy()`), so a value suffices.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Box {
    pub top_left: Point,
    pub width: f64,
    pub height: f64,
}

impl Box {
    /// box.go `NewBox`.
    pub const fn new(top_left: Point, width: f64, height: f64) -> Box {
        Box {
            top_left,
            width,
            height,
        }
    }

    /// box.go `Box.Copy`.
    pub fn copy(&self) -> Box {
        *self
    }

    /// box.go `Box.Center`.
    pub fn center(&self) -> Point {
        Point::new(
            self.top_left.x + self.width / 2.0,
            self.top_left.y + self.height / 2.0,
        )
    }

    /// box.go `Box.Intersects`: whether the segment comes within buffer.
    pub fn intersects(&self, s: &Segment, buffer: f64) -> bool {
        let tl = Point::new(self.top_left.x - buffer, self.top_left.y - buffer);
        let tr = Point::new(tl.x + self.width + buffer * 2.0, tl.y);
        let br = Point::new(tr.x, tr.y + self.height + buffer * 2.0);
        let bl = Point::new(tl.x, br.y);
        intersection_point(&s.start, &s.end, &tl, &tr).is_some()
            || intersection_point(&s.start, &s.end, &tr, &br).is_some()
            || intersection_point(&s.start, &s.end, &br, &bl).is_some()
            || intersection_point(&s.start, &s.end, &bl, &tl).is_some()
    }

    /// box.go `Box.Intersections` (top, right, bottom, left side order).
    pub fn intersections(&self, s: &Segment) -> Vec<Point> {
        let tl = self.top_left;
        let tr = Point::new(tl.x + self.width, tl.y);
        let br = Point::new(tr.x, tr.y + self.height);
        let bl = Point::new(tl.x, br.y);
        [(tl, tr), (tr, br), (br, bl), (bl, tl)]
            .iter()
            .filter_map(|(a, b)| intersection_point(&s.start, &s.end, a, b))
            .collect()
    }

    /// box.go `Box.ToString`.
    pub fn to_string(&self) -> String {
        format!(
            "{{TopLeft: {}, Width: {:.0}, Height: {:.0}}}",
            self.top_left.to_string(),
            self.width,
            self.height
        )
    }

    /// box.go `Box.Contains`.
    pub fn contains(&self, p: &Point) -> bool {
        !(p.x < self.top_left.x
            || self.top_left.x + self.width < p.x
            || p.y < self.top_left.y
            || self.top_left.y + self.height < p.y)
    }

    /// box.go `Box.Overlaps`.
    pub fn overlaps(&self, b2: &Box) -> bool {
        (self.top_left.x < (b2.top_left.x + b2.width))
            && ((self.top_left.x + self.width) > b2.top_left.x)
            && (self.top_left.y < (b2.top_left.y + b2.height))
            && ((self.top_left.y + self.height) > b2.top_left.y)
    }
}

// ------------------------------------------------------------- segment.go

/// segment.go `Intersectable`, as a closed enum of the implementations the
/// D2 packages construct (Segment, BezierCurve, Ellipse, Box).
#[derive(Clone, Debug, PartialEq)]
pub enum Intersectable {
    Segment(Segment),
    BezierCurve(BezierCurve),
    Ellipse(Ellipse),
    Box(Box),
}

impl Intersectable {
    /// `Intersectable.Intersections`.
    pub fn intersections(&self, segment: &Segment) -> Vec<Point> {
        match self {
            Intersectable::Segment(s) => s.intersections(segment),
            Intersectable::BezierCurve(b) => b.intersections(segment),
            Intersectable::Ellipse(e) => e.intersections(segment),
            Intersectable::Box(b) => b.intersections(segment),
        }
    }
}

/// segment.go `Segment`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start: Point,
    pub end: Point,
}

impl Segment {
    /// segment.go `NewSegment`.
    pub const fn new(from: Point, to: Point) -> Segment {
        Segment {
            start: from,
            end: to,
        }
    }

    /// segment.go `Segment.Overlaps`.
    pub fn overlaps(&self, other: &Segment, is_horizontal: bool, buffer: f64) -> bool {
        let (a0, a1, b0, b1) = if is_horizontal {
            (self.start.y, self.end.y, other.start.y, other.end.y)
        } else {
            (self.start.x, self.end.x, other.start.x, other.end.x)
        };
        if go_min(a0, a1) - go_max(b0, b1) >= buffer {
            return false;
        }
        if go_min(b0, b1) - go_max(a0, a1) >= buffer {
            return false;
        }
        true
    }

    /// segment.go `Segment.Intersects`.
    pub fn intersects(&self, other: &Segment) -> bool {
        intersection_point(&self.start, &self.end, &other.start, &other.end).is_some()
    }

    /// segment.go `Segment.ToString`.
    pub fn to_string(&self) -> String {
        format!("{} -> {}", self.start.to_string(), self.end.to_string())
    }

    /// segment.go `Segment.Intersections`.
    pub fn intersections(&self, other: &Segment) -> Vec<Point> {
        intersection_point(&self.start, &self.end, &other.start, &other.end)
            .into_iter()
            .collect()
    }

    /// segment.go `Segment.GetBounds` (orthogonal segments).
    pub fn get_bounds(&self, segments: &[Segment], buffer: f64) -> (f64, f64) {
        let mut ceil = f64::INFINITY;
        let mut floor = f64::NEG_INFINITY;
        if self.start.x == self.end.x && self.start.y == self.end.y {
            return (floor, ceil);
        }
        let is_horizontal = self.start.x == self.end.x;
        for other in segments {
            if is_horizontal {
                if other.end.y < self.start.y - buffer {
                    continue;
                }
                if other.start.y > self.end.y + buffer {
                    continue;
                }
                if other.start.x <= self.start.x {
                    floor = go_max(floor, other.start.x);
                }
                if other.start.x > self.start.x {
                    ceil = go_min(ceil, other.start.x);
                }
            } else {
                if other.end.x < self.start.x - buffer {
                    continue;
                }
                if other.start.x > self.end.x + buffer {
                    continue;
                }
                if other.start.y <= self.start.y {
                    floor = go_max(floor, other.start.y);
                }
                if other.start.y > self.start.y {
                    ceil = go_min(ceil, other.start.y);
                }
            }
        }
        (floor, ceil)
    }

    /// segment.go `Segment.Length`.
    pub fn length(&self) -> f64 {
        euclidean_distance(self.start.x, self.start.y, self.end.x, self.end.y)
    }

    /// segment.go `Segment.ToVector`.
    pub fn to_vector(&self) -> Vector {
        Vector(vec![self.end.x - self.start.x, self.end.y - self.start.y])
    }
}

/// Go `math.Min`: NaN if either is NaN; -0 is less than +0.
pub fn go_min(x: f64, y: f64) -> f64 {
    if x.is_infinite() && x < 0.0 || y.is_infinite() && y < 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { x } else { y };
    }
    if x < y { x } else { y }
}

/// Go `math.Max`: NaN if either is NaN; +0 is greater than -0.
pub fn go_max(x: f64, y: f64) -> f64 {
    if x.is_infinite() && x > 0.0 || y.is_infinite() && y > 0.0 {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { y } else { x };
    }
    if x > y { x } else { y }
}

// --------------------------------------------------------------- route.go

/// route.go `Route`.
pub type Route = Vec<Point>;

/// route.go `Route.Length`.
pub fn route_length(route: &[Point]) -> f64 {
    let mut l = 0.0;
    for i in 0..route.len().saturating_sub(1) {
        l += euclidean_distance(route[i].x, route[i].y, route[i + 1].x, route[i + 1].y);
    }
    l
}

/// route.go `Route.GetPointAtDistance`: the point at `distance` along the
/// route and the index of its segment. Distances past the end extrapolate
/// along the last segment. Requires at least two points (Go indexes nil
/// otherwise).
pub fn route_get_point_at_distance(route: &[Point], distance: f64) -> (Point, usize) {
    let mut remaining = distance;
    let mut curr = Point::default();
    let mut next = Point::default();
    let mut length = 0.0;
    for i in 0..route.len().saturating_sub(1) {
        curr = route[i];
        next = route[i + 1];
        length = euclidean_distance(curr.x, curr.y, next.x, next.y);
        if remaining <= length {
            let t = remaining / length;
            return (curr.interpolate(&next, t), i);
        }
        remaining -= length;
    }
    (
        curr.interpolate(&next, 1.0 + remaining / length),
        route.len().wrapping_sub(2),
    )
}

/// route.go `Route.GetBoundingBox`: (top-left, bottom-right).
pub fn route_get_bounding_box(route: &[Point]) -> (Point, Point) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in route {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }
    (Point::new(min_x, min_y), Point::new(max_x, max_y))
}

// ------------------------------------------------------------- ellipse.go

/// ellipse.go `Ellipse`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipse {
    pub center: Point,
    pub rx: f64,
    pub ry: f64,
}

impl Ellipse {
    /// ellipse.go `NewEllipse`.
    pub const fn new(center: Point, rx: f64, ry: f64) -> Ellipse {
        Ellipse { center, rx, ry }
    }

    /// ellipse.go `Ellipse.Intersections`.
    pub fn intersections(&self, segment: &Segment) -> Vec<Point> {
        let mut intersections = Vec::new();
        let a = self.rx;
        let b = self.ry;
        let a2 = a * a;
        let b2 = b * b;
        if a <= 0.0 || b <= 0.0 {
            return Vec::new();
        }

        let x1 = segment.start.x - self.center.x;
        let y1 = segment.start.y - self.center.y;
        let x2 = segment.end.x - self.center.x;
        let y2 = segment.end.y - self.center.y;

        if x1 == x2 {
            // ARM64: a2 - x1*x1 is FMSUBD.
            let intersection1 = Point::new(x1, b * go_fmsub(x1, x1, a2).sqrt() / a);
            let mut intersection2 = intersection1;
            intersection2.y *= -1.0;

            // Go sorts {p.Y, y1, y2} with `<` and checks the median.
            let is_point_on_line = |p: &Point| {
                let mut ps = [p.y, y1, y2];
                go_sort_slice_floats(&mut ps);
                ps[1] == p.y
            };

            if is_point_on_line(&intersection1) {
                intersections.push(intersection1);
            }
            if intersection2.y != 0.0 && is_point_on_line(&intersection2) {
                intersections.push(intersection2);
            }
            for p in &mut intersections {
                p.x += self.center.x;
                p.y += self.center.y;
            }
            return intersections;
        }

        let m = (y2 - y1) / (x2 - x1);
        // ARM64: y1 - m*x1 is FMSUBD.
        let c = go_fmsub(x1, m, y1);
        let line_start = Point::new(x1, y1);
        let line_end = Point::new(x2, y2);
        let is_point_on_line = |p: &Point| {
            precision_compare(p.distance_to_line(&line_start, &line_end), 0.0, PRECISION) == 0
        };

        // ARM64: denom = FMADDD(b*b + round(round(a2*m)*m)); the compiler
        // re-fuses b2's multiply here.
        let denom = go_fmadd(b, b, a2 * m * m);
        // ARM64: denom - c*c is FMSUBD.
        let root = (a2 * b2 * go_fmsub(c, c, denom)).sqrt();

        // ARM64: (-m*c)*a2 is fused with root; c*b2 is rounded and m*root
        // fused (FMADDD/FNMSUBD/FMSUBD).
        let neg_mc = -m * c;
        let intersection1 = Point::new(
            go_fmadd(neg_mc, a2, root) / denom,
            go_fmadd(m, root, c * b2) / denom,
        );
        let intersection2 = Point::new(
            go_fnmsub(neg_mc, a2, root) / denom,
            go_fmsub(m, root, c * b2) / denom,
        );

        if is_point_on_line(&intersection1) {
            intersections.push(intersection1);
        }
        if !intersection1.equals(&intersection2) && is_point_on_line(&intersection2) {
            intersections.push(intersection2);
        }
        for p in &mut intersections {
            p.x += self.center.x;
            p.y += self.center.y;
        }
        intersections
    }
}

/// Go's `sort.Slice` with `<` on three elements is an insertion sort
/// (pdqsort uses insertion sort below 12 elements).
fn go_sort_slice_floats(ps: &mut [f64]) {
    for i in 1..ps.len() {
        let mut j = i;
        while j > 0 && ps[j] < ps[j - 1] {
            ps.swap(j, j - 1);
            j -= 1;
        }
    }
}

// -------------------------------------------------------------- bezier.go

/// bezier.go `PRECISION`.
pub const PRECISION: f64 = 0.0001;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BezierPoint {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BezierControlPoint {
    point: BezierPoint,
    control: BezierPoint,
}

/// bezier.go `newBezierCurveImpl` (Graphics Gems 5 binomial weights).
fn new_bezier_curve_impl(cp: &[BezierPoint]) -> Vec<BezierControlPoint> {
    let mut c: Vec<BezierControlPoint> = cp
        .iter()
        .map(|p| BezierControlPoint {
            point: *p,
            control: BezierPoint::default(),
        })
        .collect();
    let n = c.len();
    let mut w = 0.0;
    for i in 0..n {
        match i {
            0 => w = 1.0,
            1 => w = n as f64 - 1.0,
            _ => w *= (n - i) as f64 / i as f64,
        }
        c[i].control.x = c[i].point.x * w;
        c[i].control.y = c[i].point.y * w;
    }
    c
}

/// bezier.go `bezierCurveImpl.pointAt`. Mutates the Point slots as Go does.
fn bezier_point_at(c: &mut [BezierControlPoint], t: f64) -> BezierPoint {
    c[0].point = c[0].control;
    let mut u = t;
    for i in 1..c.len() {
        let control = c[i].control;
        c[i].point = BezierPoint {
            x: control.x * u,
            y: control.y * u,
        };
        u *= t;
    }
    let t1 = 1.0 - t;
    let mut tt = t1;
    let mut p = c[c.len() - 1].point;
    for i in (0..c.len() - 1).rev() {
        // ARM64: `p.X += c[i].Point.X * tt` is FMADDD.
        p.x = go_fmadd(c[i].point.x, tt, p.x);
        p.y = go_fmadd(c[i].point.y, tt, p.y);
        tt *= t1;
    }
    p
}

/// bezier.go `BezierCurve`.
#[derive(Clone, Debug, PartialEq)]
pub struct BezierCurve {
    curve: Vec<BezierControlPoint>,
    pub points: Vec<Point>,
}

impl BezierCurve {
    /// bezier.go `NewBezierCurve`.
    pub fn new(points: Vec<Point>) -> BezierCurve {
        let local: Vec<BezierPoint> = points
            .iter()
            .map(|p| BezierPoint { x: p.x, y: p.y })
            .collect();
        BezierCurve {
            curve: new_bezier_curve_impl(&local),
            points,
        }
    }

    /// bezier.go `BezierCurve.Intersections` (cubic curves).
    pub fn intersections(&self, segment: &Segment) -> Vec<Point> {
        let p = &self.points;
        compute_intersections(
            [p[0].x, p[1].x, p[2].x, p[3].x],
            [p[0].y, p[1].y, p[2].y, p[3].y],
            [segment.start.x, segment.end.x],
            [segment.start.y, segment.end.y],
        )
    }

    /// bezier.go `BezierCurve.At`. Go's value receiver shares the backing
    /// array, so pointAt's scratch writes persist; they never affect results.
    pub fn at(&self, point: f64) -> Point {
        let mut curve = self.curve.clone();
        let p = bezier_point_at(&mut curve, point);
        Point::new(p.x, p.y)
    }
}

/// bezier.go `ComputeIntersections`.
pub fn compute_intersections(px: [f64; 4], py: [f64; 4], lx: [f64; 2], ly: [f64; 2]) -> Vec<Point> {
    let mut out = Vec::new();

    let a = ly[1] - ly[0];
    let b = lx[0] - lx[1];
    // ARM64 (bezier.go:136): round(ly0*(lx1-lx0)), then FMADDD fuses the
    // first product: lx0*(ly0-ly1) + that.
    let c = go_fmadd(lx[0], ly[0] - ly[1], ly[0] * (lx[1] - lx[0]));

    let bx = bezier_coeffs(px[0], px[1], px[2], px[3]);
    let by = bezier_coeffs(py[0], py[1], py[2], py[3]);

    // ARM64 (bezier.go:142-145): round(B*by[i]), then FMADDD fuses A*bx[i].
    let p = [
        go_fmadd(a, bx[0], b * by[0]),
        go_fmadd(a, bx[1], b * by[1]),
        go_fmadd(a, bx[2], b * by[2]),
        go_fmadd(a, bx[3], b * by[3]) + c,
    ];

    let r = cubic_roots(p);

    for &t in r.iter().take(3) {
        let point = Point::new(bezier_cubic_eval(bx, t), bezier_cubic_eval(by, t));

        let s = if (lx[1] - lx[0]) != 0.0 {
            (point.x - lx[0]) / (lx[1] - lx[0])
        } else {
            (point.y - ly[0]) / (ly[1] - ly[0])
        };

        let t_lt0 = precision_compare(t, 0.0, PRECISION) < 0;
        let t_gt1 = precision_compare(t, 1.0, PRECISION) > 0;
        let s_lt0 = precision_compare(s, 0.0, PRECISION) < 0;
        let s_gt1 = precision_compare(s, 1.0, PRECISION) > 0;
        if !(t_lt0 || t_gt1 || s_lt0 || s_gt1) {
            out.push(point);
        }
    }
    out
}

/// bezier.go:153-154 `b0*t*t*t + b1*t*t + b2*t + b3`. ARM64: `b0*t*t` and
/// `b1*t*t` are rounded, the last `*t` of the first term is fused onto
/// `b1*t*t` (FMADDD), `b2*t` is fused onto that sum, then `+ b3`.
fn bezier_cubic_eval(b: [f64; 4], t: f64) -> f64 {
    let b0tt = b[0] * t * t;
    let b1tt = b[1] * t * t;
    let sum = go_fmadd(b0tt, t, b1tt);
    go_fmadd(b[2], t, sum) + b[3]
}

/// bezier.go `cubicRoots`.
fn cubic_roots(p: [f64; 4]) -> [f64; 3] {
    if precision_compare(p[0], 0.0, PRECISION) == 0 {
        if precision_compare(p[1], 0.0, PRECISION) == 0 {
            let mut t = [-(p[3] / p[2]), -1.0, -1.0];
            for i in 0..1 {
                let ti_lt0 = precision_compare(t[i], 0.0, PRECISION) < 0;
                let ti_gt1 = precision_compare(t[i], 1.0, PRECISION) > 0;
                if ti_lt0 || ti_gt1 {
                    t[i] = -1.0;
                }
            }
            sort_special(&mut t);
            return t;
        }

        // ARM64: Pow(P2,2) - round(4*P1)*P3 is FMSUBD.
        let mut dq = go_fmsub(4.0 * p[1], p[3], gomath::pow(p[2], 2.0));
        if precision_compare(dq, 0.0, PRECISION) >= 0 {
            dq = dq.sqrt();
            let mut t = [
                -((dq + p[2]) / (2.0 * p[1])),
                (dq - p[2]) / (2.0 * p[1]),
                -1.0,
            ];
            // Go returns inside the first loop iteration.
            let ti_lt0 = precision_compare(t[0], 0.0, PRECISION) < 0;
            let ti_gt1 = precision_compare(t[0], 1.0, PRECISION) > 0;
            if ti_lt0 || ti_gt1 {
                t[0] = -1.0;
            }
            sort_special(&mut t);
            return t;
        }
    }

    let a = p[0];
    let b = p[1];
    let c = p[2];
    let d = p[3];

    let aa = b / a;
    let bb = c / a;
    let cc = d / a;

    // ARM64 (bezier.go:232): FNMSUBD 3*B - Pow(A,2).
    let q = go_fnmsub(3.0, bb, gomath::pow(aa, 2.0)) / 9.0;
    // ARM64 (bezier.go:233): round(round(9*A)*B), FMSUBD 27*C, then minus
    // Pow(A,3)+Pow(A,3) (2*x is exact).
    let r = (go_fmsub(27.0, cc, 9.0 * aa * bb) - 2.0 * gomath::pow(aa, 3.0)) / 54.0;
    let dd = gomath::pow(q, 3.0) + gomath::pow(r, 2.0);

    let mut t = [0.0; 3];

    if precision_compare(dd, 0.0, PRECISION) >= 0 {
        let sgn_s = sgn(r + dd.sqrt());
        let pow_s = gomath::pow((r + dd.sqrt()).abs(), 1.0 / 3.0);
        let sgn_t = sgn(r - dd.sqrt());
        let pow_t = gomath::pow((r - dd.sqrt()).abs(), 1.0 / 3.0);
        let s = sgn_s * pow_s;
        let tt = sgn_t * pow_t;

        let neg_a3 = -aa / 3.0;
        // ARM64 (bezier.go:242-245): S+T re-fuses S's product onto the rounded
        // T; S-T fuses T's product onto the rounded S.
        let s_plus_t = go_fmadd(sgn_s, pow_s, tt);
        t[0] = neg_a3 + s_plus_t;
        t[1] = neg_a3 - s_plus_t / 2.0;
        t[2] = neg_a3 - s_plus_t / 2.0;
        let im = (3f64.sqrt() * go_fmsub(sgn_t, pow_t, s) / 2.0).abs();

        if precision_compare(im, 0.0, PRECISION) != 0 {
            t[1] = -1.0;
            t[2] = -1.0;
        }
    } else {
        let th = gomath::acos(r / (-gomath::pow(q, 3.0)).sqrt());
        let two_sqrt_q = 2.0 * (-q).sqrt();
        let a3 = aa / 3.0;
        // ARM64 (bezier.go:255-257): FNMSUBD 2*sqrt(-Q)*cos(..) - A/3.
        t[0] = go_fnmsub(two_sqrt_q, gomath::cos(th / 3.0), a3);
        t[1] = go_fnmsub(
            two_sqrt_q,
            gomath::cos((th + 2.0 * std::f64::consts::PI) / 3.0),
            a3,
        );
        t[2] = go_fnmsub(
            two_sqrt_q,
            gomath::cos((th + 4.0 * std::f64::consts::PI) / 3.0),
            a3,
        );
    }

    for ti in &mut t {
        let ti_lt0 = precision_compare(*ti, 0.0, PRECISION) < 0;
        let ti_gt1 = precision_compare(*ti, 1.0, PRECISION) > 0;
        if ti_lt0 || ti_gt1 {
            *ti = -1.0;
        }
    }
    sort_special(&mut t);
    t
}

/// bezier.go `sortSpecial`.
fn sort_special(a: &mut [f64]) {
    loop {
        let mut flip = false;
        for i in 0..a.len() - 1 {
            let ai1_gteq0 = precision_compare(a[i + 1], 0.0, PRECISION) >= 0;
            let ai_gt_ai1 = precision_compare(a[i], a[i + 1], PRECISION) > 0;
            let ai_lt0 = precision_compare(a[i], 0.0, PRECISION) < 0;
            if (ai1_gteq0 && ai_gt_ai1) || (ai_lt0 && ai1_gteq0) {
                flip = true;
                a.swap(i, i + 1);
            }
        }
        if !flip {
            break;
        }
    }
}

/// bezier.go `sgn`.
fn sgn(x: f64) -> f64 {
    if x < 0.0 { -1.0 } else { 1.0 }
}

/// bezier.go `bezierCoeffs`.
fn bezier_coeffs(p0: f64, p1: f64, p2: f64, p3: f64) -> [f64; 4] {
    // ARM64 (bezier.go:311-313): each `k*P` term is accumulated with FMADDD.
    [
        go_fmadd(-3.0, p2, go_fmadd(3.0, p1, -p0)) + p3,
        go_fmadd(3.0, p2, go_fmsub(6.0, p1, 3.0 * p0)),
        go_fmadd(3.0, p1, -3.0 * p0),
        p0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_v_matches_go() {
        for (v, want) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (-12.5, "-12.5"),
            (123456.0, "123456"),
            (1234567.0, "1.234567e+06"),
            (1e6, "1e+06"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1e21, "1e+21"),
            (0.1, "0.1"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "+Inf"),
        ] {
            assert_eq!(go_format_v(v), want, "{v}");
        }
    }
}
