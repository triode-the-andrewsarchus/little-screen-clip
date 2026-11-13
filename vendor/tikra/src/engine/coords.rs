//! Coordinates, as `\tikz@scan@one@point` reads them, in canvas centimeters.
//!
//! `(1,2)`, `(1cm,2pt)`, `(30:2)`, `(1,2,3)`, `(a)`, `(a.north east)`, `(a.30)`, `(a |- b)`,
//! `([xshift=1cm]a.east)`, the `cs:` systems (`canvas`, `xyz`, `polar`, `node`), `calc`
//! (`$(a)!0.5!(b)$`, `$(a)+(1,0)$`, `$2*(a)$`, `$(a)!1cm!90:(b)$`) and `(intersection of a--b and c--d)`.

use crate::scene::{Point, Transform};
use crate::tex;
use crate::Error;

use super::Engine;

/// A point read from a coordinate, and the node it names (a line to a node stops at its border).
#[derive(Debug, Clone)]
pub struct Coord {
    pub p: Point,
    pub node: Option<String>,
}

/// Where the `:` of a polar coordinate is, outside braces and parentheses: `({90+45*2}:2)`.
fn top_level(t: &str, target: char) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in t.char_indices() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            c if c == target && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Where the `]` that closes the `[` at the start of `t` is (braces and brackets nest).
fn closing_bracket(t: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in t.char_indices() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 && c == ']' {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits at top-level commas.
pub fn split_commas(t: &str) -> Vec<String> {
    crate::keys::split_top_level(t)
}

fn strip_braces(t: &str) -> &str {
    crate::keys::strip_braces(t)
}

impl Engine {
    /// The local (untransformed) vector of `(x,y)` or `(x,y,z)` with the current unit vectors.
    fn xyz(&mut self, parts: &[String]) -> Result<Point, Error> {
        let axes = [self.st.x, self.st.y, self.st.z];
        // TikZ evaluates the components once to see whether they have units, then again: random
        // numbers are drawn twice, and the second ones kept
        if parts.iter().any(|s| s.contains("rand") || s.contains("rnd")) {
            for s in parts.iter().take(3) {
                crate::expr::eval_length(strip_braces(s), &self.vars())?;
            }
        }
        let mut p = Point::default();
        for (k, s) in parts.iter().enumerate().take(3) {
            let (v, unit) = crate::expr::eval_length(strip_braces(s), &self.vars())?;
            let axis = axes[k];
            p = p.plus(if unit {
                // a length follows the axis direction, without its scale (pgf: \pgfpointxy vs \pgfpoint)
                if k == 0 {
                    Point::new(v, 0.0)
                } else if k == 1 {
                    Point::new(0.0, v)
                } else {
                    let n = axis.norm().max(1e-12);
                    axis.scale(v / n)
                }
            } else {
                axis.scale(v)
            });
        }
        Ok(p)
    }

    /// Reads a coordinate's text (macros already expanded) as a canvas point.
    pub fn coordinate(&mut self, inner: &str) -> Result<Coord, Error> {
        // `({$(A)!.5!(B)$} -| C)`: a coordinate in braces is that coordinate
        let t = strip_braces(inner.trim()).trim();
        // ([options] point): the options transform this point only
        if let Some(rest) = t.strip_prefix('[') {
            let Some(end) = closing_bracket(t) else { return Err(Error::Figure(format!("unmatched bracket in ({t})"))) };
            let opts = &rest[..end - 1];
            let point = rest[end..].trim().to_string();
            self.begin_group();
            self.st.pending = Transform::default();
            let r = self.process_keys(super::tikra_tokens(opts), Some("/tikz")).and_then(|_| {
                let pending = self.st.pending;
                let before = self.st.transform;
                // the point in the system with the extra transformations, as TikZ applies them
                self.st.transform = before.compose(&pending);
                self.st.pending = Transform::default();
                let moved = self.coordinate(&point)?;
                // a node's point does not follow the transformation: the options move it as TikZ
                // does, from the current coordinate system
                self.st.transform = before;
                let fixed = self.coordinate(&point)?;
                if moved.p.minus(fixed.p).norm() < 1e-9 && pending != Transform::default() {
                    let t = before.compose(&pending).compose(&before.inverse());
                    return Ok(Coord { p: t.apply(moved.p), node: None });
                }
                Ok(moved)
            });
            self.end_group();
            return r;
        }
        if let Some(calc) = t.strip_prefix('$').and_then(|s| s.strip_suffix('$')) {
            return Ok(Coord { p: self.calc(calc)?, node: None });
        }
        if let Some(rest) = t.strip_prefix("intersection of") {
            return Ok(Coord { p: self.line_intersection(rest)?, node: None });
        }
        // (a |- b), (a -| b)
        for op in ["|-", "-|"] {
            if let Some(i) = find_perpendicular(t, op) {
                // in the current coordinate system, as pgf's points are before they are transformed
                let current = self.st.transform;
                let inv = current.inverse();
                let a = inv.apply(self.coordinate(&t[..i])?.p);
                let b = inv.apply(self.coordinate(&t[i + 2..])?.p);
                return Ok(Coord { p: current.apply(if op == "|-" { Point::new(a.x, b.y) } else { Point::new(b.x, a.y) }), node: None });
            }
        }
        // a coordinate system: (canvas cs:x=1cm,y=2cm)
        if let Some(i) = t.find(" cs:").or_else(|| t.find("cs:").filter(|&k| k > 0)) {
            let system = t[..i].trim();
            let args = t[i..].trim_start().trim_start_matches("cs:").to_string();
            return self.coordinate_system(system, &args);
        }
        let commas = split_commas(t);
        let colon = top_level(t, ':');
        // `(\pgfkeysvalueof{/pgfplots/xmin},0)` with xmin unset: an empty part of a plain point in an axis
        if colon.is_none() && !t.contains(['(', '{', '[']) && t.split(',').any(|p| p.trim().is_empty()) && t.split(',').count() > 1 {
            let parts: Vec<String> = t.split(',').map(|p| p.trim().to_string()).collect();
            if let Some(p) = self.pgfplots_plain(&parts) {
                return Ok(Coord { p: p?, node: None });
            }
        }
        if commas.len() == 1 && colon.is_none() {
            // a node: name, name.anchor, name.angle
            let (name, anchor) = match t.split_once('.') {
                Some((n, a)) if !n.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '+') && self.lookup_node(n.trim()).is_some() => (n.trim(), Some(a.trim())),
                _ => (t, None),
            };
            if name == "current subpath start" {
                return Ok(Coord { p: self.subpath_start, node: None });
            }
            // pgfplots' extra anchors of an axis are coordinates of their own
            if let Some(a) = anchor {
                if let Some(node) = self.nodes.get(&format!("{name}@{a}")) {
                    return Ok(Coord { p: node.center(), node: None });
                }
            }
            if let Some(node) = self.lookup_node(name) {
                return Ok(match anchor {
                    Some(a) => {
                        // a logic gate's anchors as `logic gate anchors use bounding box` is now
                        let gatebox = format!("@gatebox={} {a}", self.tex.meaning_of("iftikra@gatebox") == self.tex.meaning_of("iftrue"));
                        let p = node.anchor(&gatebox).or_else(|| self.node_anchor(&node, a));
                        Coord { p: p.ok_or_else(|| Error::Figure(format!("unknown anchor: {name}.{a}")))?, node: None }
                    }
                    // the name it is registered by: with the scope's name prefix when that is the one found
                    None => Coord { p: node.center(), node: Some(self.node_key(name)) },
                });
            }
            return Err(Error::Figure(format!("unknown coordinate: ({t})")));
        }
        let local = if let (Some(c), 1) = (colon, commas.len()) {
            // polar: (angle:radius), (angle:x radius and y radius)
            // TikZ's names of directions (`\tikz@polar@dir@up`…)
            let angle = match strip_braces(&t[..c]).trim() {
                "up" | "north" => 90.0,
                "down" | "south" => -90.0,
                "left" | "west" => 180.0,
                "right" | "east" => 0.0,
                "north east" => 45.0,
                "north west" => 135.0,
                "south east" => -45.0,
                "south west" => -135.0,
                a if self.st.trig_rad => self.number(a)?.to_degrees(),
                a => self.number(a)?,
            };
            let r = t[c + 1..].trim();
            let (rx, ry, unit) = if let Some((a, b)) = r.split_once(" and ") {
                let (x, ux) = crate::expr::eval_length(strip_braces(a), &self.vars())?;
                let (y, _) = crate::expr::eval_length(strip_braces(b), &self.vars())?;
                (x, y, ux)
            } else {
                let (v, u) = crate::expr::eval_length(strip_braces(r), &self.vars())?;
                (v, v, u)
            };
            let (s, c) = angle.to_radians().sin_cos();
            if unit {
                Point::new(rx * c, ry * s)
            } else {
                self.st.x.scale(rx * c).plus(self.st.y.scale(ry * s))
            }
        } else if let Some(p) = self.pgfplots_plain(&commas) {
            // inside an axis: axis or description coordinates
            return Ok(Coord { p: p?, node: None });
        } else {
            self.xyz(&commas)?
        };
        Ok(Coord { p: self.st.transform.apply(local), node: None })
    }

    fn coordinate_system(&mut self, system: &str, args: &str) -> Result<Coord, Error> {
        let items: Vec<(String, String)> = split_commas(args)
            .into_iter()
            .map(|p| match p.split_once('=') {
                Some((k, v)) => (k.trim().to_string(), strip_braces(v.trim()).to_string()),
                None => (p.trim().to_string(), String::new()),
            })
            .collect();
        let get = |k: &str| items.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone());
        match system {
            "tikra" => {
                // a point already on the canvas (\p1 of `let`)
                let x = self.length(&items.first().map(|i| i.0.clone()).unwrap_or_default())?;
                let y = self.length(&items.get(1).map(|i| i.0.clone()).unwrap_or_default())?;
                Ok(Coord { p: Point::new(x, y), node: None })
            }
            "canvas" => {
                let x = get("x").map(|v| self.length(&v)).transpose()?.unwrap_or(0.0);
                let y = get("y").map(|v| self.length(&v)).transpose()?.unwrap_or(0.0);
                Ok(Coord { p: self.st.transform.apply(Point::new(x, y)), node: None })
            }
            "xyz" => {
                let parts = vec![get("x").unwrap_or("0".into()), get("y").unwrap_or("0".into()), get("z").unwrap_or("0".into())];
                let p = self.xyz(&parts)?;
                Ok(Coord { p: self.st.transform.apply(p), node: None })
            }
            "canvas polar" | "polar" | "xyz polar" => {
                let angle = get("angle").map(|v| self.number(&v)).transpose()?.unwrap_or(0.0);
                // the keys in order: `radius` sets both radii, a later `x radius` changes one again
                let (mut rx, mut ry) = (0.0, 0.0);
                for (k, v) in &items {
                    if !matches!(k.as_str(), "radius" | "x radius" | "y radius") {
                        continue;
                    }
                    let r = if system == "canvas polar" { self.length(v)? } else { self.number(v)? };
                    match k.as_str() {
                        "radius" => (rx, ry) = (r, r),
                        "x radius" => rx = r,
                        _ => ry = r,
                    }
                }
                let (s, c) = angle.to_radians().sin_cos();
                let local = if system == "canvas polar" { Point::new(rx * c, ry * s) } else { self.st.x.scale(rx * c).plus(self.st.y.scale(ry * s)) };
                Ok(Coord { p: self.st.transform.apply(local), node: None })
            }
            "visualization" => Ok(Coord { p: self.visualization_cs(args)?, node: None }),
            "intersection" => {
                // `first line={(A)--(B)}, second line={(C)--(D)}`
                let (Some(a), Some(b)) = (get("first line"), get("second line")) else {
                    return Err(Error::Figure("intersection cs: only lines are supported".into()));
                };
                Ok(Coord { p: self.line_intersection(&format!("{a} and {b}"))?, node: None })
            }
            "barycentric" => {
                // the nodes' centers, weighted
                let (mut sum, mut weight) = (Point::default(), 0.0);
                for (name, w) in &items {
                    let w = self.number(w)?;
                    let node = self.nodes.get(name).ok_or_else(|| Error::Figure(format!("unknown node: {name}")))?;
                    sum = sum.plus(node.center().scale(w));
                    weight += w;
                }
                if weight.abs() < 1e-12 {
                    return Err(Error::Figure("barycentric cs: the weights add up to zero".into()));
                }
                Ok(Coord { p: sum.scale(1.0 / weight), node: None })
            }
            "tangent" => {
                // where a line from the point touches the node's circle (its east anchor gives the
                // radius); `solution=2` is the other side
                let name = get("node").unwrap_or_default();
                let node = self.nodes.get(&name).cloned().ok_or_else(|| Error::Figure(format!("unknown node: {name}")))?;
                let point = get("point").unwrap_or_default();
                let point = self.coordinate(point.trim().trim_start_matches('(').trim_end_matches(')'))?.p;
                let c = node.center();
                let r = node.anchor("east").map_or(0.0, |e| e.minus(c).norm());
                let d = point.minus(c);
                let mut angle = (r / d.norm().max(1e-12)).clamp(-1.0, 1.0).acos().to_degrees();
                if get("solution").and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(1) > 1 {
                    angle = -angle;
                }
                let dir = d.scale(1.0 / d.norm().max(1e-12));
                Ok(Coord { p: c.plus(Transform::rotation(angle).apply(dir.scale(r))), node: None })
            }
            "node" => {
                let name = get("name").unwrap_or_default();
                let node = self.nodes.get(&name).cloned().ok_or_else(|| Error::Figure(format!("unknown node: {name}")))?;
                let p = match (get("anchor"), get("angle")) {
                    (Some(a), _) => self.node_anchor(&node, &a).ok_or_else(|| Error::Figure(format!("unknown anchor: {name}.{a}")))?,
                    (None, Some(g)) => node.anchor(&g).unwrap_or(node.center()),
                    _ => return Ok(Coord { p: node.center(), node: Some(name) }),
                };
                Ok(Coord { p, node: None })
            }
            other if matches!(self.tex.meaning_of(&format!("tikra@cs@{other}")), tex::Meaning::Macro(_)) => {
                // \tikzdeclarecoordinatesystem{name}{code}: the code runs with the options as #1,
                // and its last point (a pgf point) is the coordinate
                let tex::Meaning::Macro(m) = self.tex.meaning_of(&format!("tikra@cs@{other}")) else { unreachable!() };
                let body = tex::substitute(&m.body, &[super::tikra_tokens(args)]);
                let at = body.iter().rposition(|t| matches!(t, tex::Token::Cs(n) if n.starts_with("pgfpoint") || n.starts_with("pgfqpoint")));
                let at = at.ok_or_else(|| Error::Figure(format!("coordinate system '{other}' gives no point")))?;
                // the point is the last one at the top level
                let mut depth = 0i32;
                let mut start = at;
                for (k, t) in body.iter().enumerate() {
                    match t {
                        tex::Token::Char(_, tex::Cat::Begin) => depth += 1,
                        tex::Token::Char(_, tex::Cat::End) => depth -= 1,
                        tex::Token::Cs(n) if depth == 0 && (n.starts_with("pgfpoint") || n.starts_with("pgfqpoint")) => start = k,
                        _ => {}
                    }
                }
                self.run_tokens(body[..start].to_vec())?;
                let p = self.point_of(body[start..].to_vec())?;
                Ok(Coord { p: self.st.transform.apply(p), node: None })
            }
            other => match self.pgfplots_cs(other, args) {
                Some(r) => r,
                None => match self.three_d_coordinate(other, &items)? {
                    Some(c) => Ok(c),
                    None => Err(Error::Figure(format!("coordinate system '{other}' not supported"))),
                },
            },
        }
    }

    /// `(intersection of A--B and C--D)`: where the two lines (not segments) meet.
    fn line_intersection(&mut self, spec: &str) -> Result<Point, Error> {
        let (l1, l2) = spec.split_once(" and ").ok_or_else(|| Error::Figure("intersection of: missing 'and'".into()))?;
        let ends = |e: &mut Engine, l: &str| -> Result<(Point, Point), Error> {
            let (a, b) = l.split_once("--").ok_or_else(|| Error::Figure("intersection of: a line is A--B".into()))?;
            Ok((e.coordinate(a.trim().trim_start_matches('(').trim_end_matches(')'))?.p, e.coordinate(b.trim().trim_start_matches('(').trim_end_matches(')'))?.p))
        };
        let (a, b) = ends(self, l1)?;
        let (c, d) = ends(self, l2)?;
        let r = b.minus(a);
        let s = d.minus(c);
        let den = r.x * s.y - r.y * s.x;
        if den.abs() < 1e-12 {
            return Err(Error::Figure("intersection of parallel lines".into()));
        }
        let t = ((c.x - a.x) * s.y - (c.y - a.y) * s.x) / den;
        Ok(a.plus(r.scale(t)))
    }

    /// `$(A)!0.5!(B)$`, `$(A)+(1,0)$`, `$0.5*(A)+0.5*(B)$`, `$(A)!0.5!90:(B)$`, `$(A)!(C)!(B)$`.
    pub fn calc(&mut self, text: &str) -> Result<Point, Error> {
        let c: Vec<char> = text.chars().collect();
        let mut i = 0;
        let mut total = Point::default();
        let mut sign = 1.0;
        let mut last: Option<Point> = None;
        let mut factor = 1.0;
        while i < c.len() {
            match c[i] {
                ' ' => i += 1,
                '+' | '-' => {
                    if let Some(p) = last.take() {
                        total = total.plus(p.scale(sign * factor));
                    }
                    sign = if c[i] == '+' { 1.0 } else { -1.0 };
                    factor = 1.0;
                    i += 1;
                }
                '(' => {
                    let (inside, end) = paren(&c, i).ok_or_else(|| Error::Figure("unmatched parenthesis in calc".into()))?;
                    last = Some(self.coordinate(&inside)?.p);
                    i = end;
                }
                '!' => {
                    // (A)!t!(B), (A)!t!angle:(B), (A)!(C)!(B) (projection)
                    let rest: String = c[i + 1..].iter().collect();
                    let a = last.unwrap_or_default();
                    if rest.trim_start().starts_with('(') {
                        let rc: Vec<char> = rest.chars().collect();
                        let k0 = rc.iter().position(|x| *x == '(').unwrap_or(0);
                        let (proj, pend) = paren(&rc, k0).ok_or_else(|| Error::Figure("calc: unmatched parenthesis".into()))?;
                        let after: String = rc[pend..].iter().collect();
                        let after = after.trim_start().trim_start_matches('!');
                        let ac: Vec<char> = after.chars().collect();
                        let k1 = ac.iter().position(|x| *x == '(').ok_or_else(|| Error::Figure("calc: missing the second point".into()))?;
                        let (bt, bend) = paren(&ac, k1).ok_or_else(|| Error::Figure("calc: unmatched parenthesis".into()))?;
                        let p = self.coordinate(&proj)?.p;
                        let b = self.coordinate(&bt)?.p;
                        let d = b.minus(a);
                        let t = if d.norm() > 1e-12 { (p.minus(a).x * d.x + p.minus(a).y * d.y) / (d.x * d.x + d.y * d.y) } else { 0.0 };
                        last = Some(a.lerp(b, t));
                        let consumed = rest.len() - after.len() + ac[..bend].iter().collect::<String>().len();
                        i += 1 + rest[..consumed].chars().count();
                        continue;
                    }
                    let t_end = rest.find('!').ok_or_else(|| Error::Figure("calc: unmatched '!'".into()))?;
                    let t_text = rest[..t_end].to_string();
                    let after = &rest[t_end + 1..];
                    let (angle, target_text) = match after.find('(') {
                        Some(j) if after[..j].contains(':') => (Some(self.number(after[..j].trim().trim_end_matches(':'))?), &after[j..]),
                        Some(j) => (None, &after[j..]),
                        None => return Err(Error::Figure("calc: missing the second point".into())),
                    };
                    let tc: Vec<char> = target_text.chars().collect();
                    let (inside, end) = paren(&tc, 0).ok_or_else(|| Error::Figure("calc: unmatched parenthesis".into()))?;
                    let b = self.coordinate(&inside)?.p;
                    let mut target = b;
                    if let Some(ang) = angle {
                        target = a.plus(Transform::rotation(ang).apply(b.minus(a)));
                    }
                    let (v, unit) = crate::expr::eval_length(&t_text, &self.vars())?;
                    // a distance is measured before the transformation (`scale=3` makes it 3 times as
                    // long), and one toward the point itself goes up: the direction of no length is (0,1)
                    let toward = |d: Point| if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(0.0, 1.0) };
                    last = Some(if unit {
                        let inv = self.st.transform.inverse();
                        let (la, lt) = (inv.apply(a), inv.apply(target));
                        self.st.transform.apply(la.plus(toward(lt.minus(la)).scale(v)))
                    } else {
                        a.lerp(target, v)
                    });
                    let consumed: String = tc[..end].iter().collect();
                    i += 1 + t_end + 1 + (after.chars().count() - target_text.chars().count()) + consumed.chars().count();
                }
                _ => {
                    // a factor: 0.5*(A), {\x}*(A), cos(0)*sin(90)*(A): up to the `*` before a point
                    let start = i;
                    let mut depth = 0;
                    let before_point = |i: usize| c[i] == '*' && c[i + 1..].iter().find(|x| **x != ' ') == Some(&'(');
                    while i < c.len() && (depth > 0 || !before_point(i)) {
                        match c[i] {
                            '{' | '(' => depth += 1,
                            '}' | ')' => depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    let f: String = c[start..i].iter().collect();
                    factor = self.number(strip_braces(f.trim())).unwrap_or(1.0);
                    if c.get(i) == Some(&'*') {
                        i += 1;
                    }
                }
            }
        }
        if let Some(p) = last {
            total = total.plus(p.scale(sign * factor));
        }
        Ok(total)
    }
}

/// `(…)` starting at `start` (a `(`, as every caller has found): the content and the index after
/// the closing parenthesis.
fn paren(c: &[char], start: usize) -> Option<(String, usize)> {
    let mut depth = 0i32;
    for (k, ch) in c.iter().enumerate().skip(start) {
        match ch {
            '(' | '{' => depth += 1,
            ')' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((c[start + 1..k].iter().collect(), k + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// The position of ` |- ` / ` -| ` between two coordinates, outside braces.
fn find_perpendicular(t: &str, op: &str) -> Option<usize> {
    let mut depth = 0i32;
    let b = t.as_bytes();
    let o = op.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i + 1 < b.len() && b[i] == o[0] && b[i + 1] == o[1] && i > 0 {
            return Some(i);
        }
    }
    None
}

/// Reads `(…)` from the token stream (the `(` already read): up to the matching `)`, braces and
/// nested parentheses respected; `($…$)` up to `$)`.
pub fn read_coordinate_tokens(e: &mut Engine) -> Result<Vec<tex::Token>, Error> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut braces = 0usize;
    let mut math = false;
    loop {
        let mut t = e.tex.get_token().ok_or_else(|| Error::Figure("unmatched parenthesis".into()))?;
        if let tex::Token::Char(c, tex::Cat::Active) = t {
            if e.st.active_in_code {
                t = tex::Token::other(c);
            }
        }
        match &t {
            tex::Token::Char(_, tex::Cat::Begin) => braces += 1,
            tex::Token::Char(_, tex::Cat::End) => braces = braces.saturating_sub(1),
            tex::Token::Char('$', _) if braces == 0 => math = !math,
            tex::Token::Char('(', _) if braces == 0 => depth += 1,
            tex::Token::Char(')', _) if braces == 0 && !math => {
                if depth == 0 {
                    return Ok(out);
                }
                depth -= 1;
            }
            tex::Token::Char(')', _) if braces == 0 && math => depth = depth.saturating_sub(1),
            _ => {}
        }
        out.push(t);
        if out.len() > tex::MAX_TOKENS {
            return Err(Error::Figure("unmatched parenthesis".into()));
        }
    }
}
