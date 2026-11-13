//! `\addplot graphics[options] {file};` and `\addplot3 graphics`, as `pgfplotscoordprocessing.code.tex`
//! does them: a PNG image placed in the axis. In 2d it fills the box from (`xmin`, `ymin`) to
//! (`xmax`, `ymax`); with `points={(x,y,z) => (px,py) …}` the first two points with image
//! coordinates (in pt from the image's lower left corner) give its scale and position, and four of
//! them set the axis' unit vectors so that the axis projects as the image does (`auto adjust
//! axis`). The points, or the corners, count in the axis' limits.
//!
//! Only PNG files are drawn (PDF and EPS images need a renderer); the file is found as tables are
//! (`TIKRA_INPUT_DIR`).

use std::rc::Rc;

use crate::canvas::Item;
use crate::scene::{Point, Segment, Transform};
use crate::Error;
use crate::PT;

use super::keys;
use super::plot::{Coord, Plot};
use super::table;
use super::{Engine, Frame};

/// Largest image read.
pub const MAX_IMAGE: u64 = 16 << 20;

/// One bp, in cm.
const BP: f64 = 2.54 / 72.0;

/// A point of `points`: data coordinates (z NaN in 2d), and image coordinates (pt) if mapped.
pub type MapPoint = ([f64; 3], Option<(f64, f64)>);

#[derive(Debug, Clone, Default)]
pub struct Graphics {
    /// The PNG file, if it was found and read.
    pub png: Option<Rc<Vec<u8>>>,
    /// Its natural size (cm), as pdfTeX takes it from its pixels and resolution.
    pub natural: (f64, f64),
    /// `includegraphics={trim=l b r t}` (cm) and `clip`.
    pub trim: [f64; 4],
    pub clip: bool,
    /// `points`.
    pub points: Vec<MapPoint>,
}

/// The size in pixels and the resolution (dots per inch, 0 if none) of a PNG file.
pub fn png_info(data: &[u8]) -> Option<(u32, u32, f64, f64)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
    let (w, h) = (be(16), be(20));
    let mut res = (0.0, 0.0);
    let mut i = 8;
    while i + 8 <= data.len() {
        let len = be(i) as usize;
        let kind = &data[i + 4..i + 8];
        if kind == b"IDAT" {
            break;
        }
        // pixels per meter, unit 1 = meter: pdfTeX rounds to dots per inch
        if kind == b"pHYs" && len == 9 && i + 17 <= data.len() && data[i + 16] == 1 {
            res = ((f64::from(be(i + 8)) * 0.0254).round(), (f64::from(be(i + 12)) * 0.0254).round());
        }
        i += 12 + len;
    }
    Some((w, h, res.0, res.1))
}

/// The image's natural size in cm: pixels at their resolution, 72 dpi without one (pdfTeX's
/// `\pdfimageresolution` default).
fn natural_size(w: u32, h: u32, xres: f64, yres: f64) -> (f64, f64) {
    let (xr, yr) = if xres > 0.0 && yres > 0.0 { (xres, yres) } else { (72.0, 72.0) };
    (f64::from(w) * 72.27 / xr * PT, f64::from(h) * 72.27 / yr * PT)
}

/// The file `\includegraphics` would load: the name as given, or with the first of pdfTeX's
/// extensions that exists (`\Gin@extensions`).
fn find_image(name: &str) -> Result<String, String> {
    let n = name.trim();
    let base = n.rsplit('/').next().unwrap_or(n);
    if base.contains('.') {
        return Ok(n.to_string());
    }
    for ext in [".pdf", ".PDF", ".ai", ".AI", ".png", ".PNG", ".jpg", ".JPG", ".jpeg", ".JPEG", ".jbig2", ".JBIG2", ".jb2", ".JB2"] {
        let full = format!("{n}{ext}");
        if table::find_file(&full, &[]).is_ok() {
            return Ok(full);
        }
    }
    Err(format!("file '{n}' not found"))
}

/// A PNG file's bytes and its natural size (cm).
pub type Png = (Rc<Vec<u8>>, (f64, f64));

/// A PNG image as `\includegraphics` would find it, and its natural size.
pub fn load_png(name: &str) -> Result<Png, String> {
    let file = find_image(name)?;
    let data = table::read_bytes(&file, &[], MAX_IMAGE)?;
    match png_info(&data) {
        Some((w, h, xr, yr)) if w > 0 && h > 0 => Ok((Rc::new(data), natural_size(w, h, xr, yr))),
        _ => Err(format!("'{file}' is not a PNG image; only PNG images are drawn")),
    }
}

/// Reads `graphics[options] {file}` and adds the points that count in the limits.
pub fn survey(e: &mut Engine, plot: &mut Plot, three_d: bool) -> Result<(), Error> {
    if let Some(o) = e.optional()? {
        let saved = e.st.key_path.clone();
        let r = e.process_keys(o, Some("/pgfplots/plot graphics"));
        e.st.key_path = saved;
        r?;
    }
    let name = e.argument_string()?;
    let mut g = Graphics::default();
    match find_image(&name).and_then(|f| table::read_bytes(&f, &[], MAX_IMAGE).map(|d| (f, d))) {
        Ok((file, data)) => match png_info(&data) {
            Some((w, h, xr, yr)) if w > 0 && h > 0 => {
                g.natural = natural_size(w, h, xr, yr);
                g.png = Some(Rc::new(data));
            }
            _ => e.warn(format!("\\addplot graphics: '{file}' is not a PNG image; only PNG images are drawn")),
        },
        Err(m) => e.warn(format!("\\addplot graphics: {m}")),
    }
    includegraphics_options(e, &mut g);
    let gv = |e: &mut Engine, k: &str| keys::value(e, &format!("/pgfplots/plot graphics/{k}"));
    let points = gv(e, "points");
    g.points = parse_points(e, &points)?;
    for (p, _) in &g.points {
        if !three_d || p[2].is_finite() {
            plot.coords.push(Coord::new(p[0], p[1], if three_d { p[2] } else { f64::NAN }));
        }
    }
    // the lower left and upper right corners, when given (required without `points`)
    let axes: &[&str] = if three_d { &["x", "y", "z"] } else { &["x", "y"] };
    for end in ["min", "max"] {
        let mut c = [f64::NAN; 3];
        let mut missing = None;
        for (k, a) in axes.iter().enumerate() {
            let v = gv(e, &format!("{a}{end}"));
            match e.number(&v) {
                Ok(x) if !v.trim().is_empty() => c[k] = x,
                _ => missing = missing.or(Some(format!("{a}{end}"))),
            }
        }
        match missing {
            None => plot.coords.push(Coord::new(c[0], c[1], c[2])),
            Some(k) if g.points.is_empty() => {
                e.warn(format!("\\addplot graphics: the key '{k}' is missing: no place for the image (give xmin, xmax, ymin, ymax or points)"));
                break;
            }
            Some(_) => {}
        }
    }
    if three_d && keys::value(e, "/pgfplots/plot graphics/auto adjust axis") == "true" {
        plot.late = required_view(e, &g.points);
    }
    plot.graphics = Some(Box::new(g));
    Ok(())
}

/// `trim` and `clip` of `includegraphics={…}` (numbers in bp, as graphicx reads them).
fn includegraphics_options(e: &mut Engine, g: &mut Graphics) {
    let text = keys::value(e, "/pgfplots/plot graphics/includegraphics");
    for part in text.split(',') {
        let (k, v) = part.split_once('=').map_or((part.trim(), ""), |(k, v)| (k.trim(), v.trim()));
        match k {
            "clip" => g.clip = v.is_empty() || v == "true",
            "trim" => {
                for (i, x) in v.split_whitespace().take(4).enumerate() {
                    g.trim[i] = match x.parse::<f64>() {
                        Ok(n) => n * BP,
                        Err(_) => e.length(x).unwrap_or(0.0),
                    };
                }
            }
            "" => {}
            other => e.warn(format!("\\addplot graphics: includegraphics option '{other}' ignored")),
        }
    }
}

/// `(x,y[,z]) [=> (px,py)] …`: data points, with image coordinates if mapped.
fn parse_points(e: &mut Engine, text: &str) -> Result<Vec<MapPoint>, Error> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while let Some(start) = rest.find('(') {
        let Some((inner, after)) = paren_group(&rest[start..]) else { break };
        let parts = split_top(inner);
        let num = |e: &mut Engine, s: &str| e.number(s.trim());
        let x = num(e, parts.first().copied().unwrap_or(""))?;
        let y = num(e, parts.get(1).copied().unwrap_or(""))?;
        let z = match parts.get(2) {
            Some(s) if !s.trim().is_empty() => num(e, s)?,
            _ => f64::NAN,
        };
        rest = after.trim_start();
        let mut img = None;
        if let Some(r) = rest.strip_prefix("=>") {
            let r = r.trim_start();
            if let Some((inner, after)) = paren_group(r).filter(|_| r.starts_with('(')) {
                let parts = split_top(inner);
                if parts.len() == 2 && !parts[1].trim().is_empty() {
                    img = Some((num(e, parts[0])?, num(e, parts[1])?));
                }
                rest = after;
            }
        }
        out.push(([x, y, z], img));
    }
    Ok(out)
}

/// The text inside the parentheses `s` starts with, and what follows them.
fn paren_group(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[1..i], &s[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits at the commas outside parentheses and braces.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' | '{' => depth += 1,
            ')' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// `\pgfplotsplothandlergraphicspointmapcomputerequiredview`: the projection T(x,y,z) = o + x e_x +
/// y e_y + z e_z of the image, solved from four points with image coordinates, as the axis' unit
/// vectors, scaled uniformly. Empty if there are not four such 3d points.
fn required_view(e: &mut Engine, points: &[MapPoint]) -> Vec<crate::tex::Token> {
    let mapped: Vec<([f64; 3], (f64, f64))> = points.iter().filter_map(|(p, i)| i.map(|i| (*p, i))).take(4).collect();
    if mapped.len() < 4 {
        return Vec::new();
    }
    if mapped.iter().any(|(p, _)| !p[2].is_finite()) {
        return Vec::new();
    }
    // unknowns: exx exy eyx eyy ezx ezy ox oy
    let mut m = [[0.0f64; 9]; 8];
    for (k, (p, (ix, iy))) in mapped.iter().enumerate() {
        m[2 * k] = [p[0], 0.0, p[1], 0.0, p[2], 0.0, 1.0, 0.0, *ix];
        m[2 * k + 1] = [0.0, p[0], 0.0, p[1], 0.0, p[2], 0.0, 1.0, *iy];
    }
    let Some(s) = solve(m) else {
        e.warn("\\addplot graphics: the points are not linearly independent; the axis is not adjusted to the image");
        return Vec::new();
    };
    let (mut ex, mut ey, mut ez) = ((s[0], s[1]), (s[2], s[3]), (s[4], s[5]));
    // `snap z`: a z unit vector that is nearly vertical is made vertical
    let snap = keys::value(e, "/pgfplots/plot graphics/snap z");
    if let Ok(snap) = e.number(&snap) {
        if ez.0.abs() < snap * ez.0.hypot(ez.1) {
            ez.0 = 0.0;
        }
    }
    // reversed axes reverse their unit vectors again later
    for (a, v) in [("x", &mut ex), ("y", &mut ey), ("z", &mut ez)] {
        if e.pv(&format!("{a} dir")) == "reverse" {
            *v = (-v.0, -v.1);
        }
    }
    let f = |v: f64| super::super::pgf_number(v);
    let mut s = format!("x={{({},{})}},y={{({},{})}},z={{({},{})}},scale mode=scale uniformly,", f(ex.0), f(ex.1), f(ey.0), f(ey.1), f(ez.0), f(ez.1));
    // before compat 1.6, the only strategy
    if keys::compat(e) < 106 {
        s.push_str("scale uniformly strategy=change vertical limits,");
    }
    super::toks(&s)
}

/// Gaussian elimination with partial pivoting of an 8 × 8 system (the last column the right side).
fn solve(mut m: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for c in 0..8 {
        let p = (c..8).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        for r in 0..8 {
            if r != c {
                let k = m[r][c] / m[c][c];
                let pivot = m[c];
                for (x, p) in m[r].iter_mut().zip(pivot).skip(c) {
                    *x -= k * p;
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for (i, v) in x.iter_mut().enumerate() {
        *v = m[i][8] / m[i][i];
    }
    Some(x)
}

/// Draws the image: placed by the first two mapped points, else filling the box of the plot's
/// points (`\pgfplots@plot@handler@graphics@usebb`), as `\node[anchor=south west, inner sep=0pt]
/// {\includegraphics[width=w, height=h]{file}}`.
pub fn draw(e: &mut Engine, g: &Graphics, pts: &[[f64; 3]], f: &Frame) {
    let Some(png) = g.png.clone() else { return };
    let (tw, th) = ((g.natural.0 - g.trim[0] - g.trim[2]).max(0.0), (g.natural.1 - g.trim[1] - g.trim[3]).max(0.0));
    let mapped: Vec<([f64; 3], (f64, f64))> = g.points.iter().filter_map(|(p, i)| i.map(|i| (*p, i))).take(2).collect();
    let (q, w, h) = if let [(pa, ia), (pb, ib)] = mapped[..] {
        let at = |p: [f64; 3]| f.data(p[0], p[1], if p[2].is_finite() { p[2] } else { f.min[2] });
        let (a, b) = (at(pa), at(pb));
        let (ddx, ddy) = ((ib.0 - ia.0) * PT, (ib.1 - ia.1) * PT);
        if ddx == 0.0 || ddy == 0.0 {
            e.warn("\\addplot graphics: the first two points with '=>' need different image x and y coordinates");
            return;
        }
        // canvas length per image length, and the lower left corner where image (0,0) goes
        let (rx, ry) = ((b.x - a.x) / ddx, (b.y - a.y) / ddy);
        (Point::new(a.x - ia.0 * PT * rx, a.y - ia.1 * PT * ry), rx * tw, ry * th)
    } else {
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for l in pts.iter().filter(|l| l.iter().take(if f.three_d { 3 } else { 2 }).all(|v| v.is_finite())) {
            let p = f.at(*l);
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        if !(hi.x - lo.x > 0.0 && hi.y - lo.y > 0.0) {
            e.warn("\\addplot graphics: too few coordinates; the lower left and upper right corners are needed");
            return;
        }
        (lo, hi.x - lo.x, hi.y - lo.y)
    };
    if !(w.is_finite() && h.is_finite() && tw > 0.0 && th > 0.0) {
        return;
    }
    // the whole image, scaled as its trimmed part is to w × h
    let (sx, sy) = (w / tw, h / th);
    let place = Transform { a: g.natural.0 * sx, b: 0.0, c: 0.0, d: g.natural.1 * sy, e: q.x - g.trim[0] * sx, f: q.y - g.trim[1] * sy };
    let corners = [q, Point::new(q.x + w, q.y), Point::new(q.x + w, q.y + h), Point::new(q.x, q.y + h)];
    if e.counts_for_size() {
        corners.iter().for_each(|c| e.canvas.include(*c));
    }
    if g.clip {
        let mut segments = vec![Segment::MoveTo(corners[0])];
        segments.extend(corners[1..].iter().map(|c| Segment::LineTo(*c)));
        segments.push(Segment::Close);
        e.canvas.push(Item::Clip { segments, even_odd: false });
    }
    e.canvas.push(Item::Image { png, place });
    if g.clip {
        e.canvas.push(Item::End);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_projection() {
        // T(x,y,z) = (1,2) + x (3,1) + y (-2,1) + z (0,4)
        let t = |p: [f64; 3]| (1.0 + 3.0 * p[0] - 2.0 * p[1], 2.0 + p[0] + p[1] + 4.0 * p[2]);
        let pts = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.5, 1.8], [0.0, 0.0, 0.0]];
        let mut m = [[0.0; 9]; 8];
        for (k, p) in pts.iter().enumerate() {
            let (ix, iy) = t(*p);
            m[2 * k] = [p[0], 0.0, p[1], 0.0, p[2], 0.0, 1.0, 0.0, ix];
            m[2 * k + 1] = [0.0, p[0], 0.0, p[1], 0.0, p[2], 0.0, 1.0, iy];
        }
        let s = solve(m).expect("solvable");
        let want = [3.0, 1.0, -2.0, 1.0, 0.0, 4.0, 1.0, 2.0];
        for (a, b) in s.iter().zip(want) {
            assert!((a - b).abs() < 1e-9, "{s:?}");
        }
    }

    #[test]
    fn reads_png_resolution() {
        let mut d = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x03\xa3\0\0\x01\xb1\x08\x06\0\0\0\0\0\0\0".to_vec();
        d.extend(b"\0\0\0\x09pHYs\0\0\x17\x12\0\0\x17\x12\x01\0\0\0\0");
        let (w, h, xr, yr) = png_info(&d).expect("a png");
        assert_eq!((w, h, xr, yr), (931, 433, 150.0, 150.0));
        // pdfTeX: 448.55469pt × 208.61888pt
        let (nw, nh) = natural_size(w, h, xr, yr);
        assert!((nw / PT - 448.5547).abs() < 1e-2 && (nh / PT - 208.6189).abs() < 1e-2);
    }
}
