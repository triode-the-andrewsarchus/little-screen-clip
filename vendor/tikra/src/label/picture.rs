//! LaTeX's `picture` environment in a label: `\begin{picture}(w,h)(x,y) … \end{picture}` with
//! `\put`, `\multiput`, `\line`, `\vector`, `\circle`, `\circle*`, `\thinlines`, `\thicklines` and
//! `\linethickness`, in `\unitlength` (`\setlength{\unitlength}{…}` before it).
//!
//! As LaTeX draws it without `pict2e` (measured on TeX): the box is `w` by `h` units with no
//! depth, (x, y) at its lower left corner; horizontal and vertical lines are rules centered on
//! the line, the line thickness thick; slanted lines are the line font's, as thick as `\thinlines`
//! or `\thicklines` say; circles come from the circle font, disks in whole points up to 15pt and
//! rings in whole points up to 16pt, then in fours up to 40pt.

/// What a picture draws, in points from its box's lower left corner (y up).
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    Rule { x: f64, y: f64, width: f64, height: f64 },
    Line { x1: f64, y1: f64, x2: f64, y2: f64, thickness: f64 },
    Disk { x: f64, y: f64, diameter: f64 },
    Ring { x: f64, y: f64, diameter: f64, thickness: f64 },
}

/// A picture: its box (pt) and what it draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picture {
    pub width: f64,
    pub height: f64,
    pub marks: Vec<Mark>,
}

/// LaTeX's `\thinlines` and `\thicklines` (pt).
const THIN: f64 = 0.4;
const THICK: f64 = 0.8;

/// The text with each picture as `\tikrapicture{k}`, `k` its index in the pictures, and the
/// settings of `\unitlength` left out.
pub fn extract(text: &str, warnings: &mut Vec<String>) -> (String, Vec<Picture>) {
    let mut out = String::with_capacity(text.len());
    let mut pictures = Vec::new();
    let mut unit = 1.0;
    let mut rest = text;
    loop {
        let set = rest.find("\\setlength").filter(|&i| rest[i + 10..].trim_start().trim_start_matches('{').trim_start().starts_with("\\unitlength"));
        let begin = rest.find("\\begin{picture}");
        let i = match (set, begin) {
            (Some(s), Some(b)) => s.min(b),
            (Some(s), None) => s,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        if let Some(r) = after.strip_prefix("\\setlength") {
            // \setlength{\unitlength}{0.8mm} or \setlength\unitlength{1cm}
            let r = r.trim_start();
            let r = r.strip_prefix('{').map_or(r, |r| r.trim_start());
            let r = r.strip_prefix("\\unitlength").unwrap_or(r).trim_start();
            let r = r.strip_prefix('}').unwrap_or(r);
            match group(r) {
                Some((v, more)) => {
                    match length(v) {
                        Some(u) => unit = u,
                        None => warnings.push(format!("\\unitlength: '{v}' is not a length; kept {unit}pt")),
                    }
                    rest = more;
                }
                None => rest = r,
            }
            continue;
        }
        let r = &after["\\begin{picture}".len()..];
        let Some(end) = r.find("\\end{picture}") else {
            out.push_str("\\begin{picture}");
            rest = r;
            continue;
        };
        let (body, more) = (&r[..end], &r[end + "\\end{picture}".len()..]);
        let mut p = Parser { s: body, unit, thickness: THIN, warnings };
        let size = p.pair().unwrap_or((0.0, 0.0));
        let origin = p.pair().unwrap_or((0.0, 0.0));
        let mut marks = Vec::new();
        p.objects(&mut marks);
        // from the lower left corner
        let (dx, dy) = (origin.0 * unit, origin.1 * unit);
        for m in &mut marks {
            match m {
                Mark::Rule { x, y, .. } | Mark::Disk { x, y, .. } | Mark::Ring { x, y, .. } => {
                    *x -= dx;
                    *y -= dy;
                }
                Mark::Line { x1, y1, x2, y2, .. } => {
                    *x1 -= dx;
                    *x2 -= dx;
                    *y1 -= dy;
                    *y2 -= dy;
                }
            }
        }
        out.push_str(&format!("\\tikrapicture{{{}}}", pictures.len()));
        pictures.push(Picture { width: size.0 * unit, height: size.1 * unit, marks });
        rest = more;
    }
    out.push_str(rest);
    (out, pictures)
}

/// A length in points (`0.8mm`, `1pt`, `2cm`).
fn length(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(s.len());
    let v: f64 = if end == 0 { 1.0 } else { s[..end].parse().ok()? };
    let pt = match s[end..].trim() {
        "pt" => 1.0,
        "cm" => 72.27 / 2.54,
        "mm" => 72.27 / 25.4,
        "in" => 72.27,
        "bp" => 72.27 / 72.0,
        "sp" => 1.0 / 65536.0,
        _ => return None,
    };
    Some(v * pt)
}

/// The text of the braced group at the start of `s`, and what follows it.
fn group(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start().strip_prefix('{')?;
    let mut depth = 1;
    for (k, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[..k], &s[k + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

struct Parser<'a, 'w> {
    s: &'a str,
    unit: f64,
    /// The thickness of the lines (pt): `\thinlines`, `\thicklines`, `\linethickness{…}` (which
    /// only straight rules take).
    thickness: f64,
    warnings: &'w mut Vec<String>,
}

impl<'a> Parser<'a, '_> {
    fn skip(&mut self) {
        self.s = self.s.trim_start();
    }

    /// `(a,b)`, in units.
    fn pair(&mut self) -> Option<(f64, f64)> {
        self.skip();
        let r = self.s.strip_prefix('(')?;
        let close = r.find(')')?;
        let (a, b) = r[..close].split_once(',')?;
        let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        self.s = &r[close + 1..];
        Some((a, b))
    }

    /// `{…}`.
    fn group(&mut self) -> Option<&'a str> {
        let (g, more) = group(self.s)?;
        self.s = more;
        Some(g)
    }

    fn command(&mut self) -> Option<String> {
        self.skip();
        let r = self.s.strip_prefix('\\')?;
        let name: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        if name.is_empty() {
            return None;
        }
        self.s = &r[name.len()..];
        Some(name)
    }

    /// The objects and settings of a picture's body, or of a `\put`'s group.
    fn objects(&mut self, marks: &mut Vec<Mark>) {
        loop {
            self.skip();
            if self.s.is_empty() {
                return;
            }
            let Some(name) = self.command() else {
                // a stray character (a `%` left in, a brace)
                let mut c = self.s.chars();
                c.next();
                self.s = c.as_str();
                continue;
            };
            match name.as_str() {
                "put" => {
                    let at = self.pair().unwrap_or((0.0, 0.0));
                    let body = self.group().unwrap_or_default();
                    self.object(body, (at.0 * self.unit, at.1 * self.unit), marks);
                }
                "multiput" => {
                    let at = self.pair().unwrap_or((0.0, 0.0));
                    let step = self.pair().unwrap_or((0.0, 0.0));
                    let n = self.group().and_then(|g| g.trim().parse::<i64>().ok()).unwrap_or(0).max(0);
                    let body = self.group().unwrap_or_default();
                    for k in 0..n {
                        let k = k as f64;
                        self.object(body, ((at.0 + k * step.0) * self.unit, (at.1 + k * step.1) * self.unit), marks);
                    }
                }
                "thinlines" => self.thickness = THIN,
                "thicklines" => self.thickness = THICK,
                "linethickness" => {
                    if let Some(t) = self.group().and_then(length) {
                        self.thickness = t;
                    }
                }
                other => {
                    self.warnings.push(format!("picture: \\{other} not supported: ignored"));
                }
            }
        }
    }

    /// What `\put` puts at `at` (pt): the settings in its group hold in it only.
    fn object(&mut self, body: &'a str, at: (f64, f64), marks: &mut Vec<Mark>) {
        let saved = (self.s, self.thickness);
        self.s = body;
        let mut thick = self.thickness;
        loop {
            let Some(name) = self.command() else {
                if !self.s.trim().is_empty() {
                    self.warnings.push(format!("picture: '{}' not supported: ignored", self.s.trim()));
                }
                break;
            };
            match name.as_str() {
                "thinlines" => thick = THIN,
                "thicklines" => thick = THICK,
                "linethickness" => {
                    if let Some(t) = self.group().and_then(length) {
                        thick = t;
                    }
                }
                "line" | "vector" => {
                    if name == "vector" {
                        self.warnings.push("picture: \\vector drawn as a line".to_string());
                    }
                    let (a, b) = self.pair().unwrap_or((1.0, 0.0));
                    let len = self.group().and_then(|g| g.trim().parse::<f64>().ok()).unwrap_or(0.0) * self.unit;
                    let t = thick;
                    if b == 0.0 {
                        let x = if a < 0.0 { at.0 - len } else { at.0 };
                        marks.push(Mark::Rule { x, y: at.1 - t / 2.0, width: len, height: t });
                    } else if a == 0.0 {
                        let y = if b < 0.0 { at.1 - len } else { at.1 };
                        marks.push(Mark::Rule { x: at.0 - t / 2.0, y, width: t, height: len });
                    } else {
                        // the line font's are thin or thick only
                        let t = if t >= THICK { THICK } else { THIN };
                        let (dx, dy) = (len * a.signum(), len * b / a.abs());
                        marks.push(Mark::Line { x1: at.0, y1: at.1, x2: at.0 + dx, y2: at.1 + dy, thickness: t });
                    }
                    break;
                }
                "circle" => {
                    let filled = self.s.starts_with('*');
                    if filled {
                        self.s = &self.s[1..];
                    }
                    let d = self.group().and_then(|g| g.trim().parse::<f64>().ok()).unwrap_or(0.0) * self.unit;
                    let t = if thick >= THICK { THICK } else { THIN };
                    marks.push(if filled {
                        Mark::Disk { x: at.0, y: at.1, diameter: d.round().clamp(1.0, 15.0) }
                    } else {
                        let d = if d <= 16.0 { d.round().max(1.0) } else { ((d / 4.0).round() * 4.0).min(40.0) };
                        Mark::Ring { x: at.0, y: at.1, diameter: d, thickness: t }
                    });
                    break;
                }
                other => {
                    self.warnings.push(format!("picture: \\{other} not supported: ignored"));
                    break;
                }
            }
        }
        (self.s, self.thickness) = saved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_dots() {
        let mut w = Vec::new();
        let (text, pictures) = extract(
            r"\setlength{\unitlength}{0.8mm}\begin{picture}(4,4)(-2,-2) \put(-2,0){\line(1,0){4}} \put(0,-2){\line(0,1){4}} \multiput(-1,1)(2,0){2}{\circle*{1}} \end{picture}",
            &mut w,
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(text, r"\tikrapicture{0}");
        let p = &pictures[0];
        let u = 0.8 * 72.27 / 25.4;
        assert!((p.width - 4.0 * u).abs() < 1e-9 && (p.height - 4.0 * u).abs() < 1e-9);
        assert_eq!(p.marks.len(), 4);
        // the horizontal line through the middle, a rule centered on it
        let Mark::Rule { x, y, width, height } = p.marks[0] else { panic!("{:?}", p.marks[0]) };
        assert!(x.abs() < 1e-9 && (y - (2.0 * u - 0.2)).abs() < 1e-9 && (width - 4.0 * u).abs() < 1e-9 && (height - 0.4).abs() < 1e-9);
        // a disk of 2.28pt is the circle font's of 2pt
        assert_eq!(p.marks[2], Mark::Disk { x: u, y: 3.0 * u, diameter: 2.0 });
    }

    #[test]
    fn thickness_and_slanted_lines() {
        let mut w = Vec::new();
        let (_, pictures) = extract(r"\begin{picture}(100,40)\put(0,20){\line(2,1){20}}\put(60,20){\linethickness{2pt}\line(0,1){10}}\put(80,0){\circle{20}}\end{picture}", &mut w);
        let m = &pictures[0].marks;
        assert_eq!(m[0], Mark::Line { x1: 0.0, y1: 20.0, x2: 20.0, y2: 30.0, thickness: 0.4 });
        assert_eq!(m[1], Mark::Rule { x: 59.0, y: 20.0, width: 2.0, height: 10.0 });
        assert_eq!(m[2], Mark::Ring { x: 80.0, y: 0.0, diameter: 20.0, thickness: 0.4 });
    }
}
