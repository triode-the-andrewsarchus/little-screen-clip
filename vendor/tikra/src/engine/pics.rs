//! Pics: `\pic[options] (name) at (point) {type=value};` and `pic {…}` on a path. A pic is placed
//! like a node (shifted to its position, the surrounding rotation and scale reset unless `transform
//! shape`, then its own options' transformations) and runs its type's `code`, `background code` and
//! `foreground code` (`pics/type/.style={code={…}}`, or `type/.pic={…}`), with `pic actions` giving
//! its paths the pic's `draw`, `fill`… The angles library's `angle` and `right angle` are done here.

use crate::keys;
use crate::scene::{Point, Transform};
use crate::tex::{self, Token};
use crate::Error;

use super::state::Mode;
use super::{tikra_tokens, Engine};

pub struct PicSpec {
    pub options: Vec<Token>,
    pub name: Option<String>,
    pub at: Point,
    pub content: Vec<Token>,
    /// between an operation and its target: the segment, and the default `pos` on it
    pub place: Option<(super::nodes::OnPath, Option<f64>)>,
}

impl Engine {
    pub fn pic(&mut self, spec: PicSpec) -> Result<(), Error> {
        self.begin_group();
        self.st.node = Default::default();
        self.st.mode = Mode::default();
        self.st.pending = Transform::default();
        self.st.pic_text = None;
        self.st.quotes_in_pic = true;
        let r = self.pic_in_group(spec);
        self.end_group();
        r
    }

    fn pic_in_group(&mut self, spec: PicSpec) -> Result<(), Error> {
        self.process_keys(spec.options, Some("/tikz"))?;
        self.st.quotes_in_pic = false;
        self.st.pic_mode = std::mem::take(&mut self.st.mode);
        // placed like a node
        let mut linear = Transform::default();
        if self.st.node.transform_shape {
            linear = Transform { e: 0.0, f: 0.0, ..self.st.transform };
        }
        let mut at = self.st.node.at.unwrap_or(spec.at);
        // on a segment: at its `pos`, turned along it when `sloped`
        if let (None, Some((place, default_pos))) = (self.st.node.at, spec.place) {
            if let Some(t) = self.st.node.pos.or(default_pos) {
                let (p, direction) = place.at(t);
                at = p;
                if self.st.node.sloped {
                    let angle = direction.y.atan2(direction.x).to_degrees();
                    let angle = if !self.st.allow_upside_down && (angle > 90.0 + 1e-9 || angle < -90.0 - 1e-9) { angle + 180.0 } else { angle };
                    linear = linear.compose(&Transform::rotation(angle));
                }
            }
        }
        self.st.transform = Transform::translation(at.x, at.y).compose(&linear).compose(&self.st.pending);
        self.st.pending = Transform::default();
        if let Some(n) = spec.name.or_else(|| self.st.node.name.clone()) {
            self.st.name_prefix = n;
        }
        // the type and its value set the pics/… keys
        for item in keys::split_list(&spec.content) {
            let (name, value) = (item.key.clone(), item.value.clone());
            if name == "angle" || name == "right angle" {
                return self.angle_pic(name == "right angle", value);
            }
            let mut v = tikra_tokens(&name);
            if let Some(value) = value {
                v.push(Token::other('='));
                v.push(Token::Char('{', tex::Cat::Begin));
                v.extend(value);
                v.push(Token::Char('}', tex::Cat::End));
            }
            self.process_keys(v, Some("/tikz/pics"))?;
        }
        // tikzmark's `pic anchor=(-out)`: moved for that coordinate of it to be where it goes
        let anchor = crate::extension::Context { e: self }.macro_text("tikra@picanchor");
        if !anchor.trim().is_empty() {
            let name = format!("{}{}", self.st.name_prefix, anchor.trim().trim_matches(['(', ')']));
            let code = keys::value(&self.tex, "/tikz/pics/code").unwrap_or_default();
            if let (Ok(p), _) = self.run_unseen(code, |e| e.coordinate(&name).map(|c| c.p))? {
                self.st.transform = Transform::translation(at.x - p.x, at.y - p.y).compose(&self.st.transform);
            }
        }
        for key in ["setup code", "background code", "code", "foreground code"] {
            let code = keys::value(&self.tex, &format!("/tikz/pics/{key}")).unwrap_or_default();
            if keys::trim(&code).is_empty() {
                continue;
            }
            if key == "setup code" {
                self.run_tokens(code)?;
                continue;
            }
            self.begin_group();
            self.begin_scope();
            let every = match key {
                "background code" => "every behind pic/.try",
                "foreground code" => "every front pic/.try",
                _ => "every pic/.try",
            };
            // a scope with that style: its transformations too
            let r = self.process_keys(tikra_tokens(every), Some("/tikz")).and_then(|_| {
                self.st.apply_pending();
                self.run_tokens(code)
            });
            self.end_scope();
            self.end_group();
            r?;
        }
        Ok(())
    }

    /// The angles library: an arc (or a square corner) at B between the directions to A and to C,
    /// drawn with the pic's actions, and the pic's text on the bisector.
    fn angle_pic(&mut self, right: bool, value: Option<Vec<Token>>) -> Result<(), Error> {
        let spec = value.map(|v| tex::tokens_to_string(&v)).unwrap_or_else(|| "A--B--C".into());
        let parts: Vec<String> = spec.split("--").map(|p| p.trim().to_string()).collect();
        if parts.len() != 3 {
            return Err(Error::Figure(format!("angle={spec}: three points A--B--C expected")));
        }
        let center = |e: &mut Engine, n: &str| -> Result<Point, Error> {
            match e.lookup_node(n) {
                Some(node) => Ok(node.center()),
                None => Err(Error::Figure(format!("angle: unknown coordinate ({n})"))),
            }
        };
        let (a, b, c) = (center(self, &parts[0])?, center(self, &parts[1])?, center(self, &parts[2])?);
        let radius_text = keys::value(&self.tex, "/tikz/angle radius").map(|v| tex::tokens_to_string(&v)).unwrap_or_else(|| "5mm".into());
        let mut radius = self.length(&radius_text)?;
        if radius <= 0.0 {
            radius = 12.0 * crate::PT;
        }
        let eccentricity: f64 = keys::value(&self.tex, "/tikz/angle eccentricity").map(|v| tex::tokens_to_string(&v)).and_then(|s| s.trim().parse().ok()).unwrap_or(0.6);
        let start = (a.y - b.y).atan2(a.x - b.x).to_degrees();
        let end = (c.y - b.y).atan2(c.x - b.x).to_degrees();
        // (both in (-180, 180]: the end is after the start once the start is a turn before)
        let mut start = start;
        if end < start {
            start -= 360.0;
        }
        // the pic's coordinates are the canvas ones: nodes were placed in them
        self.st.transform = Transform::default();
        let pt = |v: f64| format!("{}pt", v / crate::PT);
        let (bx, by) = (pt(b.x), pt(b.y));
        let r = pt(radius);
        let polar = |angle: f64, d: f64| Point::new(b.x + d * angle.to_radians().cos(), b.y + d * angle.to_radians().sin());
        let s = polar(start, radius);
        let (sx, sy) = (pt(s.x), pt(s.y));
        let (background, foreground) = if right {
            let e1 = polar(end, radius);
            let corner = Point::new(e1.x + s.x - b.x, e1.y + s.y - b.y);
            let (cx, cy, ex, ey) = (pt(corner.x), pt(corner.y), pt(e1.x), pt(e1.y));
            (format!("({bx},{by}) -- ({sx},{sy}) -- ({cx},{cy}) -- ({ex},{ey}) -- cycle"), format!("({sx},{sy}) -- ({cx},{cy}) -- ({ex},{ey})"))
        } else {
            (
                format!("({bx},{by}) -- ({sx},{sy}) arc [start angle={start}, end angle={end}, radius={r}] -- cycle"),
                format!("({sx},{sy}) arc [start angle={start}, end angle={end}, radius={r}]"),
            )
        };
        let mode = self.st.pic_mode.clone();
        if mode.fill || mode.pattern || mode.shade {
            self.begin_group();
            self.st.mode = Mode { draw: false, ..mode.clone() };
            let r = self.pic_path(&background);
            self.end_group();
            r?;
        }
        if mode.draw {
            self.begin_group();
            self.st.mode = Mode { fill: false, pattern: false, shade: false, ..mode.clone() };
            let r = self.pic_path(&foreground);
            self.end_group();
            r?;
        }
        if let Some((text, options)) = self.st.pic_text.take() {
            let distance = eccentricity * radius * if right { std::f64::consts::SQRT_2 } else { 1.0 };
            let at = polar(0.5 * start + 0.5 * end, distance);
            let items = self.node(super::nodes::NodeSpec {
                options,
                name: None,
                at: Some(at),
                text,
                place: super::nodes::OnPath::Point(at),
                point: at,
                coordinate: false,
                default_pos: None,
            })?;
            self.canvas.items.extend(items);
        }
        Ok(())
    }

    /// A path in the pic, with the mode already set.
    fn pic_path(&mut self, path: &str) -> Result<(), Error> {
        let mode = self.st.mode.clone();
        let mut options = Vec::new();
        if mode.draw {
            options.push("draw");
        }
        if mode.fill {
            options.push("fill");
        }
        self.tex.push_tokens(tikra_tokens(&format!("{path};")));
        super::path::statement(self, &options.join(","))
    }

    /// The key a node is registered by: its name with the current name prefix when there is one.
    pub fn node_key(&self, name: &str) -> String {
        let prefixed = format!("{}{name}", self.st.name_prefix);
        if self.nodes.contains_key(&prefixed) {
            prefixed
        } else {
            name.to_string()
        }
    }

    /// A node by name, with the current name prefix tried first.
    pub fn lookup_node(&self, name: &str) -> Option<super::nodes::NodeInfo> {
        let prefixed = format!("{}{name}", self.st.name_prefix);
        self.nodes.get(&prefixed).or_else(|| self.nodes.get(name)).cloned().or_else(|| {
            // TikZ's pseudo nodes: rectangles over the picture so far and over the path being used
            let (lo, hi) = match name {
                "current bounding box" => self.canvas.min.zip(self.canvas.max)?,
                "current path bounding box" => self.st.path_box?,
                _ => return None,
            };
            Some(super::libraries::box_node(lo, hi))
        })
    }
}
