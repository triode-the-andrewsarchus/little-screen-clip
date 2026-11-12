// SPDX-License-Identifier: MPL-2.0
// Port of D2 v0.9.0 lib/svg/path.go. (lib/svg/number.go FormatFloat and
// text.go live in svgtext.rs.)
//! Faithful port of D2's `lib/svg` path builder and helpers.

#![allow(dead_code)]

use super::geo::{self, BezierCurve, Intersectable, Point, Segment, go_fmadd, go_format_v};

/// path.go `PathCommandKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathCommandKind {
    Move,
    Line,
    Cubic,
    Close,
}

/// path.go `PathCommand`: absolute path geometry. `end` is used by move,
/// line and cubic; the controls only by cubic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathCommand {
    pub kind: PathCommandKind,
    pub control1: Point,
    pub control2: Point,
    pub end: Point,
}

impl PathCommand {
    fn new(kind: PathCommandKind, end: Point) -> Self {
        PathCommand {
            kind,
            control1: Point::default(),
            control2: Point::default(),
            end,
        }
    }
}

/// path.go `chopPrecision`: float32 at 1e-4 scale, then rounded to an
/// integer; negative zero becomes positive zero.
pub fn chop_precision(f: f64) -> f64 {
    let result = (((f * 10000.0) as f32 as f64) / 10000.0).round();
    if result == 0.0 {
        return 0.0;
    }
    result
}

/// path.go `SvgPathContext`.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgPathContext {
    pub path: Vec<Intersectable>,
    pub commands: Vec<String>,
    typed_commands: Vec<PathCommand>,
    pub start: Point,
    pub current: Point,
    pub top_left: Point,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl SvgPathContext {
    /// path.go `NewSVGPathContext`.
    pub fn new(tl: Point, sx: f64, sy: f64) -> Self {
        SvgPathContext {
            path: Vec::new(),
            commands: Vec::new(),
            typed_commands: Vec::new(),
            start: Point::default(),
            current: Point::default(),
            top_left: tl,
            scale_x: sx,
            scale_y: sy,
        }
    }

    /// path.go `SvgPathContext.Relative`.
    pub fn relative(&self, base: Point, dx: f64, dy: f64) -> Point {
        // ARM64: base.X + ScaleX*dx is FMADDD.
        Point::new(
            chop_precision(go_fmadd(self.scale_x, dx, base.x)),
            chop_precision(go_fmadd(self.scale_y, dy, base.y)),
        )
    }

    /// path.go `SvgPathContext.Absolute`.
    pub fn absolute(&self, x: f64, y: f64) -> Point {
        self.relative(self.top_left, x, y)
    }

    /// path.go `SvgPathContext.StartAt`.
    pub fn start_at(&mut self, p: Point) {
        self.start = p;
        self.commands
            .push(format!("M {} {}", go_format_v(p.x), go_format_v(p.y)));
        self.typed_commands
            .push(PathCommand::new(PathCommandKind::Move, p));
        self.current = p;
    }

    /// path.go `SvgPathContext.Z`.
    pub fn z(&mut self) {
        self.path.push(Intersectable::Segment(Segment::new(
            self.current,
            self.start,
        )));
        self.commands.push("Z".to_owned());
        self.typed_commands
            .push(PathCommand::new(PathCommandKind::Close, Point::default()));
        self.current = self.start;
    }

    /// path.go `SvgPathContext.L`.
    pub fn l(&mut self, is_lower_case: bool, x: f64, y: f64) {
        let end_point = if is_lower_case {
            self.relative(self.current, x, y)
        } else {
            self.absolute(x, y)
        };
        self.path.push(Intersectable::Segment(Segment::new(
            self.current,
            end_point,
        )));
        self.commands.push(format!(
            "L {} {}",
            go_format_v(end_point.x),
            go_format_v(end_point.y)
        ));
        self.typed_commands
            .push(PathCommand::new(PathCommandKind::Line, end_point));
        self.current = end_point;
    }

    /// path.go `SvgPathContext.C`.
    #[allow(clippy::too_many_arguments)]
    pub fn c(&mut self, is_lower_case: bool, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) {
        let p = |ctx: &Self, x: f64, y: f64| {
            if is_lower_case {
                ctx.relative(ctx.current, x, y)
            } else {
                ctx.absolute(x, y)
            }
        };
        let points = vec![
            self.current,
            p(self, x1, y1),
            p(self, x2, y2),
            p(self, x3, y3),
        ];
        self.commands.push(format!(
            "C {} {} {} {} {} {}",
            go_format_v(points[1].x),
            go_format_v(points[1].y),
            go_format_v(points[2].x),
            go_format_v(points[2].y),
            go_format_v(points[3].x),
            go_format_v(points[3].y),
        ));
        self.typed_commands.push(PathCommand {
            kind: PathCommandKind::Cubic,
            control1: points[1],
            control2: points[2],
            end: points[3],
        });
        self.current = points[3];
        self.path
            .push(Intersectable::BezierCurve(BezierCurve::new(points)));
    }

    /// path.go `SvgPathContext.H`.
    pub fn h(&mut self, is_lower_case: bool, x: f64) {
        let end_point = if is_lower_case {
            self.relative(self.current, x, 0.0)
        } else {
            let mut e = self.absolute(x, 0.0);
            e.y = self.current.y;
            e
        };
        self.path.push(Intersectable::Segment(Segment::new(
            self.current,
            end_point,
        )));
        self.commands
            .push(format!("H {}", go_format_v(end_point.x)));
        self.typed_commands
            .push(PathCommand::new(PathCommandKind::Line, end_point));
        self.current = end_point;
    }

    /// path.go `SvgPathContext.V`.
    pub fn v(&mut self, is_lower_case: bool, y: f64) {
        let end_point = if is_lower_case {
            self.relative(self.current, 0.0, y)
        } else {
            let mut e = self.absolute(0.0, y);
            e.x = self.current.x;
            e
        };
        self.path.push(Intersectable::Segment(Segment::new(
            self.current,
            end_point,
        )));
        self.commands
            .push(format!("V {}", go_format_v(end_point.y)));
        self.typed_commands
            .push(PathCommand::new(PathCommandKind::Line, end_point));
        self.current = end_point;
    }

    /// path.go `SvgPathContext.PathCommands` (a copy).
    pub fn path_commands(&self) -> Vec<PathCommand> {
        self.typed_commands.clone()
    }

    /// path.go `SvgPathContext.PathData`.
    pub fn path_data(&self) -> String {
        self.commands.join(" ")
    }
}

/// path.go `GetStrokeDashAttributes`: (dash size, gap size).
pub fn get_stroke_dash_attributes(stroke_width: f64, dash_gap_size: f64) -> (f64, f64) {
    // ARM64: `-0.6*strokeWidth+10.6` is FMADDD (and `*0.5 + 0.5`, exact).
    let scale = geo::gomath::log10(go_fmadd(-0.6, stroke_width, 10.6)) * 0.5 + 0.5;
    let scaled_dash_size = stroke_width * dash_gap_size;
    let scaled_gap_size = scale * scaled_dash_size;
    (scaled_dash_size, scaled_gap_size)
}

/// Sum of the four Bernstein terms `w0*a + w1*b + w2*c + w3*d` as the pinned
/// ARM64 build evaluates path.go:181-194 (decoded from the listing): the
/// third and fourth products are always FMADDD-accumulated. For q2/q3 the
/// first product is rounded and the second fused; for q1/q4 the second
/// product is rounded and the first fused onto it.
#[inline]
fn bernstein_sum(w: [f64; 4], v: [f64; 4], fuse_first: bool) -> f64 {
    let s = if fuse_first {
        go_fmadd(w[0], v[0], w[1] * v[1])
    } else {
        go_fmadd(w[1], v[1], w[0] * v[0])
    };
    let s = go_fmadd(w[2], v[2], s);
    go_fmadd(w[3], v[3], s)
}

/// path.go `BezierCurveSegment`: the t0 -> t1 piece of a cubic (De Casteljau).
pub fn bezier_curve_segment(
    p1: &Point,
    p2: &Point,
    p3: &Point,
    p4: &Point,
    t0: f64,
    t1: f64,
) -> (Point, Point, Point, Point) {
    let u0 = 1.0 - t0;
    let u1 = 1.0 - t1;
    let xs = [p1.x, p2.x, p3.x, p4.x];
    let ys = [p1.y, p2.y, p3.y, p4.y];

    let w1 = [
        u0 * u0 * u0,
        3.0 * t0 * u0 * u0,
        3.0 * t0 * t0 * u0,
        t0 * t0 * t0,
    ];
    // ARM64: each inner sum (`2*t0*u0*u1+u0*u0*t1`, ...) rounds its first
    // product and fuses the second. `2*u0` and `2*t0` are computed as x+x.
    let w2 = [
        u0 * u0 * u1,
        go_fmadd(u0 * u0, t1, 2.0 * t0 * u0 * u1),
        go_fmadd(2.0 * u0 * t0, t1, t0 * t0 * u1),
        t0 * t0 * t1,
    ];
    let w3 = [
        u0 * u1 * u1,
        go_fmadd(2.0 * u0 * t1, u1, t0 * u1 * u1),
        go_fmadd(u0 * t1, t1, 2.0 * t0 * t1 * u1),
        t0 * t1 * t1,
    ];
    let w4 = [
        u1 * u1 * u1,
        3.0 * t1 * u1 * u1,
        3.0 * t1 * t1 * u1,
        t1 * t1 * t1,
    ];

    let q = |w: [f64; 4], fuse_first: bool| {
        Point::new(
            bernstein_sum(w, xs, fuse_first),
            bernstein_sum(w, ys, fuse_first),
        )
    };
    (q(w1, true), q(w2, false), q(w3, false), q(w4, true))
}

fn index_error(i: usize, len: usize) -> String {
    format!("runtime error: index out of range [{i}] with length {len}")
}

fn get<'a>(path_data: &[&'a str], i: usize) -> Result<&'a str, String> {
    path_data
        .get(i)
        .copied()
        .ok_or_else(|| index_error(i, path_data.len()))
}

/// Go `strconv.ParseFloat(s, 64)` with the error ignored: syntax errors give
/// 0, out-of-range values give the returned +-Inf.
fn parse_float_ignoring_error(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0)
}

/// path.go `getSVGPathString`.
fn get_svg_path_string(
    path_type: &str,
    offset_idx: usize,
    path_data: &[&str],
) -> Result<String, String> {
    let g = |k: usize| get(path_data, offset_idx + k);
    match path_type {
        "M" => Ok(format!("M {} {} ", g(1)?, g(2)?)),
        "L" => Ok(format!("L {} {} ", g(1)?, g(2)?)),
        "C" => Ok(format!(
            "C {} {} {} {} {} {} ",
            g(1)?,
            g(2)?,
            g(3)?,
            g(4)?,
            g(5)?,
            g(6)?
        )),
        "S" => Ok(format!("S {} {} {} {} ", g(1)?, g(2)?, g(3)?, g(4)?)),
        _ => Err(format!("unknown svg path command \"{}\"", g(0)?)),
    }
}

/// path.go `getPathStringIncrement`.
fn get_path_string_increment(path_type: &str) -> Result<usize, String> {
    match path_type {
        "M" | "L" => Ok(3),
        "C" => Ok(7),
        "S" => Ok(5),
        _ => Err(format!("unknown svg path command \"{path_type}\"")),
    }
}

/// path.go `pathLength`: endpoint-to-endpoint length of an SVG path.
fn path_length(path_data: &[&str]) -> Result<f64, String> {
    let mut path_length = 0.0;
    let mut prev_position = Point::default();
    let mut i = 0;
    while i < path_data.len() {
        let f = |k: usize| get(path_data, i + k).map(parse_float_ignoring_error);
        let (x, y) = match path_data[i] {
            "M" => (f(1)?, f(2)?),
            "L" => (f(1)?, f(2)?),
            "C" => (f(5)?, f(6)?),
            "S" => (f(3)?, f(4)?),
            other => return Err(format!("unknown svg path command \"{other}\"")),
        };
        if path_data[i] != "M" {
            path_length += geo::euclidean_distance(prev_position.x, prev_position.y, x, y);
        }
        prev_position = Point::new(x, y);
        i += get_path_string_increment(path_data[i])?;
    }
    Ok(path_length)
}

/// Go `%f`.
fn f6(v: f64) -> String {
    format!("{v:.6}")
}

/// path.go `SplitPath`: split an SVG path into two, the first being
/// ~`percentage` of its length.
pub fn split_path(path: &str, percentage: f64) -> Result<(String, String), String> {
    let mut sum_path_lens = 0.0;
    let mut prev_position = Point::default();
    let mut path1 = String::new();
    let mut path2 = String::new();

    let mut past_half = false;
    let path_data: Vec<&str> = path.split(' ').collect();
    let path_len = path_length(&path_data)?;

    let mut i = 0;
    while i < path_data.len() {
        let f = |k: usize| get(&path_data, i + k).map(parse_float_ignoring_error);
        let (x, y) = match path_data[i] {
            "M" => (f(1)?, f(2)?),
            "L" => (f(1)?, f(2)?),
            "C" => (f(5)?, f(6)?),
            "S" => (f(3)?, f(4)?),
            other => return Err(format!("unknown svg path command \"{other}\"")),
        };
        let cur_path_len = if path_data[i] == "M" {
            0.0
        } else {
            geo::euclidean_distance(prev_position.x, prev_position.y, x, y)
        };

        let cur_path = get_svg_path_string(path_data[i], i, &path_data)?;

        sum_path_lens += cur_path_len;

        if past_half {
            path2 += &cur_path;
        } else if sum_path_lens < path_len * percentage {
            path1 += &cur_path;
        } else {
            // ARM64 (path.go:329): pathLen*percentage - sumPathLens is FNMSUBD.
            let t =
                (geo::go_fnmsub(path_len, percentage, sum_path_lens) + cur_path_len) / cur_path_len;
            let s = |k: usize| get(&path_data, i + k);

            match path_data[i] {
                "M" => {
                    path2 += &format!("M {} {} ", s(3)?, s(4)?);
                }
                "L" => {
                    // ARM64 (path.go:335-336): (x-prev)*t + prev is FMADDD.
                    let ix = go_fmadd(x - prev_position.x, t, prev_position.x);
                    let iy = go_fmadd(y - prev_position.y, t, prev_position.y);
                    path1 += &format!("L {} {} ", f6(ix), f6(iy));
                    path2 += &format!("M {} {} L {} {} ", f6(ix), f6(iy), f6(x), f6(y));
                }
                "C" => {
                    let heading1 = Point::new(f(1)?, f(2)?);
                    let heading2 = Point::new(f(3)?, f(4)?);
                    let next_point = Point::new(x, y);

                    let (_, q2, q3, q4) = bezier_curve_segment(
                        &prev_position,
                        &heading1,
                        &heading2,
                        &next_point,
                        0.0,
                        0.5,
                    );
                    path1 += &format!(
                        "C {} {} {} {} {} {} ",
                        f6(q2.x),
                        f6(q2.y),
                        f6(q3.x),
                        f6(q3.y),
                        f6(q4.x),
                        f6(q4.y)
                    );

                    let (q1, q2, q3, q4) = bezier_curve_segment(
                        &prev_position,
                        &heading1,
                        &heading2,
                        &next_point,
                        0.5,
                        1.0,
                    );
                    path2 += &format!(
                        "M {} {} C {} {} {} {} {} {} ",
                        f6(q1.x),
                        f6(q1.y),
                        f6(q2.x),
                        f6(q2.y),
                        f6(q3.x),
                        f6(q3.y),
                        f6(q4.x),
                        f6(q4.y)
                    );
                }
                "S" => {
                    path1 += &format!("S {} {} {} {} ", s(1)?, s(2)?, s(3)?, s(4)?);
                    path2 += &format!("M {} {} ", s(3)?, s(4)?);
                }
                other => return Err(format!("unknown svg path command \"{other}\"")),
            }
            past_half = true;
        }

        i += get_path_string_increment(path_data[i])?;
        prev_position = Point::new(x, y);
    }
    Ok((path1, path2))
}
