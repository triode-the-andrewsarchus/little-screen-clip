//! xcolor colors: names, mixes `orange!80!black`, `blue!10` (with white), `rgb`, `HTML`, `cmyk`,
//! `-red` (complement), each in its xcolor model and mixed in it, as xcolor mixes them.

use std::collections::HashMap;

pub mod brewer;
pub mod sets;

/// The xcolor model of a color, which decides that of its mixes: a mix is in its first color's model
/// (`red!50!cyan` is rgb, `cyan!50!red` cmyk), but a gray first color takes the second's
/// (`white!70!cyan` is cmyk).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Model {
    #[default]
    Rgb,
    Gray,
    /// The color's c, m, y and k: pdflatex writes it so, and Ghostscript draws it through its ICC
    /// profiles, which the color's `r`, `g` and `b` are (`cyan` is (0,174,239)).
    Cmyk([f64; 4]),
}

/// A color as drawn, in RGB from 0 to 1, and its xcolor model. Two colors are equal when they are
/// drawn the same, whatever their models.
#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub model: Model,
}

impl PartialEq for Color {
    fn eq(&self, other: &Color) -> bool {
        (self.r, self.g, self.b) == (other.r, other.g, other.b)
    }
}

impl Color {
    pub const BLACK: Color = Color::gray(0.0);
    pub const WHITE: Color = Color::gray(1.0);

    pub const fn rgb(r: f64, g: f64, b: f64) -> Color {
        Color { r, g, b, model: Model::Rgb }
    }

    pub const fn gray(v: f64) -> Color {
        Color { r: v, g: v, b: v, model: Model::Gray }
    }

    /// A cmyk color, drawn as Ghostscript draws it.
    pub fn cmyk(c: f64, m: f64, y: f64, k: f64) -> Color {
        let [r, g, b] = cmyk_drawn([c, m, y, k]);
        Color { r, g, b, model: Model::Cmyk([c, m, y, k]) }
    }

    pub fn hex(&self) -> String {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }

    /// The color as an xcolor expression that holds it exactly, in xcolor's extended syntax:
    /// `rgb,1:red,1;green,0.5;blue,0`, `cmyk,1:cyan,1;magenta,0;yellow,0;black,0.2`.
    pub fn xcolor(&self) -> String {
        let v = |x: f64| format!("{}", (x.clamp(0.0, 1.0) * 1e4).round() / 1e4);
        match self.model {
            Model::Cmyk([c, m, y, k]) => format!("cmyk,1:cyan,{};magenta,{};yellow,{};black,{}", v(c), v(m), v(y), v(k)),
            _ => format!("rgb,1:red,{};green,{};blue,{}", v(self.r), v(self.g), v(self.b)),
        }
    }

    /// The color converted to xcolor's rgb, as pgfplots takes the colors of its colormaps and
    /// meshes (its `yellow` is #ffff00, not Ghostscript's cmyk yellow).
    pub fn to_rgb(&self) -> Color {
        let [r, g, b] = self.in_rgb();
        Color::rgb(r, g, b)
    }

    /// The color in xcolor's rgb: a cmyk one's is 1 − (c + k) and so on, not how it is drawn.
    fn in_rgb(&self) -> [f64; 3] {
        match self.model {
            Model::Cmyk([c, m, y, k]) => [1.0 - (c + k).min(1.0), 1.0 - (m + k).min(1.0), 1.0 - (y + k).min(1.0)],
            _ => [self.r, self.g, self.b],
        }
    }

    /// The color in xcolor's cmyk: an rgb one's black is the least of its c, m and y.
    fn in_cmyk(&self) -> [f64; 4] {
        match self.model {
            Model::Cmyk(v) => v,
            _ => {
                let [c, m, y] = [1.0 - self.r, 1.0 - self.g, 1.0 - self.b];
                let k = c.min(m).min(y);
                [c - k, m - k, y - k, k]
            }
        }
    }

    fn in_gray(&self) -> f64 {
        match self.model {
            Model::Gray => self.r,
            _ => {
                let [r, g, b] = self.in_rgb();
                0.3 * r + 0.59 * g + 0.11 * b
            }
        }
    }

    /// `p` of `a` and the rest of `b`, in `a`'s model (`b`'s when `a` is gray); none of `a` is `b`
    /// as it is.
    fn mix(a: Color, b: Color, p: f64) -> Color {
        if p == 0.0 {
            return b;
        }
        let model = if a.model == Model::Gray { b.model } else { a.model };
        let w = |x: f64, y: f64| x * p + y * (1.0 - p);
        match model {
            Model::Gray => Color::gray(w(a.r, b.r)),
            Model::Rgb => {
                let ([ar, ag, ab], [br, bg, bb]) = (a.in_rgb(), b.in_rgb());
                Color::rgb(w(ar, br), w(ag, bg), w(ab, bb))
            }
            Model::Cmyk(_) => {
                let (x, y) = (a.in_cmyk(), b.in_cmyk());
                Color::cmyk(w(x[0], y[0]), w(x[1], y[1]), w(x[2], y[2]), w(x[3], y[3]))
            }
        }
    }

    /// `-color`: in cmyk, the complement of its c + k, m + k and y + k, their least as the black.
    fn complement(&self) -> Color {
        match self.model {
            Model::Gray => Color::gray(1.0 - self.r),
            Model::Rgb => Color::rgb(1.0 - self.r, 1.0 - self.g, 1.0 - self.b),
            Model::Cmyk([c, m, y, k]) => {
                let [c, m, y] = [c, m, y].map(|v| 1.0 - (v + k).min(1.0));
                let k = c.min(m).min(y);
                Color::cmyk(c - k, m - k, y - k, k)
            }
        }
    }
}

/// Ghostscript's RGB for c, m, y and k: a table of N⁴ of them that `tools/cmyk/table.py` drew, between
/// which c, m and y are interpolated by tetrahedra and k linearly, as an ICC transform is.
fn cmyk_drawn(v: [f64; 4]) -> [f64; 3] {
    const N: usize = 13;
    static TABLE: &[u8; N * N * N * N * 3] = include_bytes!("color/cmyk.bin");
    let n = (N - 1) as f64;
    let mut cell = [0usize; 4];
    let mut frac = [0.0; 4];
    for d in 0..4 {
        let x = v[d].clamp(0.0, 1.0) * n;
        cell[d] = (x as usize).min(N - 2);
        frac[d] = x - cell[d] as f64;
    }
    let at = |i: [usize; 4]| {
        let o = (((i[0] * N + i[1]) * N + i[2]) * N + i[3]) * 3;
        [0, 1, 2].map(|j| f64::from(TABLE[o + j]))
    };
    // c, m and y by their fractions, the largest first: the corners of the tetrahedron the point is in
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| frac[b].total_cmp(&frac[a]));
    let tetrahedron = |k: usize| {
        let mut i = [cell[0], cell[1], cell[2], k];
        let mut out = at(i).map(|x| x * (1.0 - frac[order[0]]));
        for (j, &d) in order.iter().enumerate() {
            i[d] += 1;
            let w = frac[d] - order.get(j + 1).map_or(0.0, |&e| frac[e]);
            let corner = at(i);
            out = [0, 1, 2].map(|q| out[q] + w * corner[q]);
        }
        out
    };
    let (lo, hi) = (tetrahedron(cell[3]), tetrahedron(cell[3] + 1));
    [0, 1, 2].map(|q| (lo[q] * (1.0 - frac[3]) + hi[q] * frac[3]).round() / 255.0)
}

fn named(name: &str) -> Option<Color> {
    let rgb = |r, g, b| Some(Color::rgb(r, g, b));
    match name {
        "black" => Some(Color::gray(0.0)),
        "white" => Some(Color::gray(1.0)),
        "gray" => Some(Color::gray(0.5)),
        "darkgray" => Some(Color::gray(0.25)),
        "lightgray" => Some(Color::gray(0.75)),
        "cyan" => Some(Color::cmyk(1.0, 0.0, 0.0, 0.0)),
        "magenta" => Some(Color::cmyk(0.0, 1.0, 0.0, 0.0)),
        "yellow" => Some(Color::cmyk(0.0, 0.0, 1.0, 0.0)),
        "olive" => Some(Color::cmyk(0.0, 0.0, 1.0, 0.5)),
        "red" => rgb(1.0, 0.0, 0.0),
        "green" => rgb(0.0, 1.0, 0.0),
        "blue" => rgb(0.0, 0.0, 1.0),
        "brown" => rgb(0.75, 0.5, 0.25),
        "lime" => rgb(0.75, 1.0, 0.0),
        "orange" => rgb(1.0, 0.5, 0.0),
        "pink" => rgb(1.0, 0.75, 0.75),
        "purple" => rgb(0.75, 0.0, 0.25),
        "teal" => rgb(0.0, 0.5, 0.5),
        "violet" => rgb(0.5, 0.0, 0.5),
        _ => None,
    }
}

/// Parses an xcolor expression. `defined` comes from `\definecolor` and `\colorlet`.
pub fn parse(expr: &str, defined: &HashMap<String, Color>) -> Option<Color> {
    // each leading `-` complements; counted in a loop, not by recursion
    let mut e = expr.trim();
    let mut complements = 0usize;
    while let Some(rest) = e.strip_prefix('-') {
        complements += 1;
        e = rest.trim_start();
    }
    if e.is_empty() {
        return None;
    }
    let color = parse_unsigned(e, defined)?;
    Some(if complements % 2 == 1 { color.complement() } else { color })
}

fn parse_unsigned(e: &str, defined: &HashMap<String, Color>) -> Option<Color> {
    // xcolor's extended syntax: `model,div:color,factor;color,factor…`, the colors weighted by their
    // factors in the model and the sum divided by `div`
    if let Some((head, mix)) = e.split_once(':') {
        let (model, div) = head.split_once(',')?;
        let div: f64 = div.trim().parse().ok().filter(|d: &f64| *d > 0.0)?;
        let mut sum = [0.0; 4];
        for part in mix.split(';') {
            let (name, factor) = part.rsplit_once(',')?;
            let c = parse(name, defined)?;
            let f: f64 = factor.trim().parse().ok()?;
            let v = match model.trim() {
                "cmyk" => c.in_cmyk(),
                "gray" => [c.in_gray(), 0.0, 0.0, 0.0],
                _ => {
                    let [r, g, b] = c.in_rgb();
                    [r, g, b, 0.0]
                }
            };
            sum = [0, 1, 2, 3].map(|i| sum[i] + v[i] * f);
        }
        let [a, b, c, d] = sum.map(|v| v / div);
        return Some(match model.trim() {
            "cmyk" => Color::cmyk(a, b, c, d),
            "gray" => Color::gray(a),
            _ => Color::rgb(a, b, c),
        });
    }
    let parts: Vec<&str> = e.split('!').map(str::trim).collect();
    let lookup = |n: &str| defined.get(n).copied().or_else(|| named(n)).or_else(|| sets::color(n)).or_else(|| brewer::color(n));
    let mut color = lookup(parts[0])?;
    let mut i = 1;
    while i < parts.len() {
        // a trailing `!` with no percentage leaves the color as it is (`red!` is red)
        if parts[i].is_empty() && i + 1 == parts.len() {
            break;
        }
        let p: f64 = parts[i].parse::<f64>().ok()? / 100.0;
        // a missing or empty name is white: `red!50`, `red!50!` and `red!50!white` are the same
        let other = match parts.get(i + 1) {
            Some(n) if !n.is_empty() => lookup(n)?,
            _ => Color::WHITE,
        };
        color = Color::mix(color, other, p);
        i += 2;
    }
    Some(color)
}

/// `\definecolor{name}{model}{values}`.
pub fn define(model: &str, values: &str) -> Option<Color> {
    if model.trim() == "HTML" {
        let h = values.trim().trim_start_matches('#');
        let v = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|x| f64::from(x) / 255.0);
        return Some(Color::rgb(v(0)?, v(2)?, v(4)?));
    }
    // xcolor separates the values by commas or by spaces (`{0.79 0.0 0.1}`)
    let nums: Vec<f64> = values.split(|c: char| c == ',' || c.is_whitespace()).filter(|v| !v.is_empty()).map(|v| v.parse().ok()).collect::<Option<_>>()?;
    match model.trim() {
        "rgb" if nums.len() == 3 => Some(Color::rgb(nums[0], nums[1], nums[2])),
        "cmy" if nums.len() == 3 => Some(Color::rgb(1.0 - nums[0], 1.0 - nums[1], 1.0 - nums[2])),
        // a gray of 16 levels, 0 black to 15 white
        "Gray" if nums.len() == 1 => Some(Color::gray(nums[0] / 15.0)),
        "hsb" if nums.len() == 3 => {
            // hue, saturation and brightness, each from 0 to 1; the hue goes round red, yellow, green, cyan, blue, magenta
            let (h, s, b) = (nums[0].rem_euclid(1.0) * 6.0, nums[1].clamp(0.0, 1.0), nums[2].clamp(0.0, 1.0));
            let f = h - h.floor();
            let (p, q, t) = (b * (1.0 - s), b * (1.0 - s * f), b * (1.0 - s * (1.0 - f)));
            let (r, g, bl) = match h.floor() as u8 {
                0 => (b, t, p),
                1 => (q, b, p),
                2 => (p, b, t),
                3 => (p, q, b),
                4 => (t, p, b),
                _ => (b, p, q),
            };
            Some(Color::rgb(r, g, bl))
        }
        "RGB" if nums.len() == 3 => Some(Color::rgb(nums[0] / 255.0, nums[1] / 255.0, nums[2] / 255.0)),
        "gray" if nums.len() == 1 => Some(Color::gray(nums[0])),
        "cmyk" if nums.len() == 4 => Some(Color::cmyk(nums[0], nums[1], nums[2], nums[3])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xcolor_mixes() {
        let d = HashMap::new();
        assert_eq!(parse("blue!10", &d).unwrap().hex(), "#e6e6ff");
        assert_eq!(parse("orange!80!black", &d).unwrap().hex(), "#cc6600");
        assert_eq!(parse("gray!20", &d).unwrap().hex(), "#e6e6e6");
        // an empty name is white, and a trailing `!` changes nothing, as in xcolor
        assert_eq!(parse("red!50!", &d), parse("red!50!white", &d));
        assert_eq!(parse("blue!50!!", &d), parse("blue!50", &d));
        assert_eq!(parse("red!", &d), parse("red", &d));
        assert_eq!(parse("red!30!blue!50!", &d).unwrap().hex(), parse("red!30!blue!50!white", &d).unwrap().hex());
        assert_eq!(define("HTML", "FF8800").unwrap().hex(), "#ff8800");
        assert_eq!(define("rgb", "1 0.5 0").unwrap().hex(), "#ff8000");
        assert_eq!(define("rgb", " 1, 0.5,0 ").unwrap().hex(), "#ff8000");
        assert_eq!(define("hsb", "0.5,1,1").unwrap().hex(), "#00ffff");
        assert_eq!(define("hsb", "0,0,0.5").unwrap().hex(), "#808080");
        assert_eq!(define("cmy", "0,1,1").unwrap().hex(), "#ff0000");
        assert_eq!(define("Gray", "15").unwrap().hex(), "#ffffff");
        assert_eq!(define("rgb", "1,x,0"), None);
    }

    #[test]
    fn extended_syntax_holds_a_color() {
        let d = HashMap::new();
        let c = parse("orange!80!black", &d).unwrap();
        assert_eq!(c.xcolor(), "rgb,1:red,0.8;green,0.4;blue,0");
        assert_eq!(parse(&c.xcolor(), &d), Some(c));
        // weights and a divisor, as xcolor mixes them
        assert_eq!(parse("rgb,2:red,1;blue,1", &d).unwrap().hex(), "#800080");
        for bad in ["rgb:red,1", "rgb,0:red,1", "rgb,1:red", "rgb,1:nothing,1", "rgb,1:red,x"] {
            assert_eq!(parse(bad, &d), None, "{bad}");
        }
    }

    #[test]
    fn cmyk_colors_drawn_as_ghostscript_draws_them() {
        // as pdflatex and Ghostscript 10.08 draw them (`texlive/texlive:TL2025-historic`), to a unit:
        // tikra interpolates between the colors of a table
        let d = HashMap::new();
        let near = |c: Color, hex: &str| {
            let v = |i: usize| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
            [(c.r, v(1)), (c.g, v(3)), (c.b, v(5))].iter().all(|(a, b)| (a - b).abs() <= 1.0 / 255.0 + 1e-9)
        };
        for (e, rgb) in [
            ("cyan", "#00aeef"),
            ("magenta", "#ec008c"),
            ("yellow", "#fff200"),
            ("cyan!30!white", "#abe1fa"),
            ("white!70!cyan", "#abe1fa"),
            ("cyan!50!red", "#8f807d"),
            ("cyan!50!black", "#3e7c94"),
            ("Gray", "#939598"),
            ("ForestGreen", "#009a55"),
        ] {
            let c = parse(e, &d).unwrap();
            assert!(near(c, rgb), "{e}: {} for {rgb}", c.hex());
        }
        let c = define("cmyk", "0.2,0.4,0.1,0.3").unwrap();
        assert!(near(c, "#97798e"), "{}", c.hex());
    }

    #[test]
    fn mixes_in_the_first_colors_model() {
        let d = HashMap::new();
        let model = |e: &str| parse(e, &d).unwrap().model;
        let cmyk = |e: &str| {
            let Model::Cmyk(v) = model(e) else { panic!("{e}: not in cmyk") };
            v.map(|x| (x * 1e4).round() / 1e4)
        };
        // as xcolor's \extractcolorspec gives them
        assert_eq!(model("red!50!cyan"), Model::Rgb);
        assert_eq!(parse("red!50!cyan", &d).unwrap().hex(), "#808080");
        assert_eq!(model("black!50!red"), Model::Rgb);
        assert_eq!(model("gray!20"), Model::Gray);
        assert_eq!(cmyk("cyan!50!brown"), [0.5, 0.125, 0.25, 0.125]);
        assert_eq!(cmyk("cyan!50!gray"), [0.5, 0.0, 0.0, 0.25]);
        assert_eq!(cmyk("gray!50!cyan!50!red"), [0.25, 0.5, 0.5, 0.125]);
        assert_eq!(cmyk("-olive"), [0.5, 0.5, 0.0, 0.0]);
        assert_eq!(cmyk("cmyk,1:cyan,0.5;red,0.5"), [0.5, 0.5, 0.5, 0.0]);
        // none of the first is the second as it is
        assert_eq!(model("cyan!0!brown"), Model::Rgb);
        let mine: HashMap<String, Color> = [("mine".to_string(), define("cmyk", "0.2,0.4,0.1,0.3").unwrap())].into();
        assert_eq!(parse("rgb,1:mine,1", &mine).unwrap().hex(), "#804d99");
        assert_eq!(parse("gray,1:mine,1", &mine).unwrap().r, 0.393);
        assert_eq!(cmyk_of(&parse("-mine", &mine).unwrap()), [0.2, 0.0, 0.3, 0.3]);
        // the expression that holds a cmyk color holds its model
        let c = parse("cyan!30", &d).unwrap();
        assert_eq!(c.xcolor(), "cmyk,1:cyan,0.3;magenta,0;yellow,0;black,0");
        assert_eq!(parse(&c.xcolor(), &d).unwrap().model, c.model);
    }

    fn cmyk_of(c: &Color) -> [f64; 4] {
        c.in_cmyk().map(|x| (x * 1e4).round() / 1e4)
    }
}
