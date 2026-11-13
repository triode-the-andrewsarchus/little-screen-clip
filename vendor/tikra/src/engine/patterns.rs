//! Patterns declared in the document and the meta patterns:
//! - the basic layer's `\pgfdeclarepatternformonly[vars]{name}{lower left}{upper right}{tile size}{code}`
//!   (drawn in the color the pattern is used with) and `\pgfdeclarepatterninherentlycolored` (in
//!   its own colors), and TikZ's `\tikzdeclarepattern{name=…, type=…, bottom left=…, …, code=…}`;
//! - the patterns.meta library's `Lines`, `Hatch`, `Dots` and `Stars`, with their `distance`,
//!   `angle`, `xshift`, `yshift`, `line width`, `radius` and `points`.
//!
//! `pattern=name` (or `pattern={Lines[angle=45]}`) and `\pgfsetfillpattern{name}{color}` fill with
//! them, the tile repeated from the page's origin, the tiling turned by its tile transformation.
//!
//! A pattern with variables (the basic layer's `[\size,/tikz/radius]`, TikZ's `parameters` and
//! `defaults`) is drawn when it is used, with the values they have there: TikZ's from its defaults
//! and the options after its name (`mystars[points=7]`), under `/pgf/pattern keys`.

use crate::canvas::{Canvas, Fill, Item, Paint, Stroke, TileItems};
use crate::color::Color;
use crate::scene::{Point, Segment, Transform};
use crate::tex::Token;
use crate::{Error, PT};

use super::Engine;

/// A declared pattern: one tile's items, its corner and size (canvas cm), its tile transformation,
/// and whether it has its own colors.
#[derive(Debug, Clone)]
pub struct UserPattern {
    items: Vec<Item>,
    origin: Point,
    size: Point,
    transform: Transform,
    colored: bool,
    /// a pattern with variables, drawn where it is used
    lazy: Option<std::rc::Rc<LazyPattern>>,
}

/// What a pattern with variables is drawn from: its corner, tile size and code as written, its tile
/// transformation's options and its default keys.
#[derive(Debug)]
struct LazyPattern {
    origin: Vec<Token>,
    size: Vec<Token>,
    code: Vec<Token>,
    tile_options: Vec<Token>,
    defaults: Vec<Token>,
    /// TikZ's (points as `(x,y)`) or the basic layer's (`\pgfpoint{x}{y}`)
    tikz: bool,
}

impl UserPattern {
    fn lazy(p: LazyPattern, colored: bool) -> UserPattern {
        UserPattern { items: Vec::new(), origin: Point::default(), size: Point::new(PT, PT), transform: Transform::default(), colored, lazy: Some(std::rc::Rc::new(p)) }
    }
}

impl Engine {
    /// `\pgfdeclarepatternformonly` (`colored` false) and `\pgfdeclarepatterninherentlycolored`.
    pub fn declare_pattern(&mut self, colored: bool) -> Result<(), Error> {
        let vars = self.optional()?.filter(|v| v.iter().any(|t| !t.is_space()));
        let name = self.argument_string()?;
        if vars.is_some() {
            let origin = self.argument()?;
            let _upper_right = self.argument()?;
            let size = self.argument()?;
            let code = self.argument()?;
            let lazy = LazyPattern { origin, size, code, tile_options: Vec::new(), defaults: Vec::new(), tikz: false };
            self.user_patterns.insert(name.trim().to_string(), UserPattern::lazy(lazy, colored));
            return Ok(());
        }
        let origin = self.point_arg()?;
        let _upper_right = self.point_arg()?;
        let size = self.point_arg()?;
        let code = self.argument()?;
        let items = self.draw_tile(code)?;
        self.user_patterns.insert(name.trim().to_string(), UserPattern { items, origin, size, transform: Transform::default(), colored, lazy: None });
        Ok(())
    }

    /// `\tikzdeclarepattern{name=…, type=colored|uncolored, bottom left=…, top right=…, bounding
    /// box={(a) and (b)}, tile size=…, tile transformation=…, parameters=…, defaults=…, code=…}`,
    /// and the basic layer's `\pgfdeclarepattern{…}` (`tikz` false), with the same keys: its points
    /// `\pgfpoint{x}{y}`, its tile transformation basic-layer code (`\pgftransformrotate{…}`).
    pub fn tikz_declare_pattern(&mut self, tikz: bool) -> Result<(), Error> {
        let spec = self.argument()?;
        let (mut name, mut colored, mut lazy) = (String::new(), false, false);
        let mut p = LazyPattern { origin: Vec::new(), size: Vec::new(), code: Vec::new(), tile_options: Vec::new(), defaults: Vec::new(), tikz };
        for item in crate::keys::split_list(&spec) {
            let v = item.value.clone().unwrap_or_default();
            match item.key.as_str() {
                "name" => name = self.tex.expand_to_string(v)?.trim().to_string(),
                "type" => colored = self.tex.expand_to_string(v)?.trim() == "colored",
                "bottom left" => p.origin = v,
                // `(a) and (b)`: the lower left corner first
                "bounding box" => {
                    let text = crate::tex::tokens_to_string(&v);
                    let first = text.split(" and ").next().unwrap_or("").trim().to_string();
                    p.origin = super::tikra_tokens(&first);
                }
                "tile size" => p.size = v,
                "tile transformation" => p.tile_options = v,
                "code" => p.code = v,
                "parameters" => lazy = true,
                "defaults" => {
                    p.defaults = v;
                    lazy = true;
                }
                _ => {}
            }
        }
        let pattern = UserPattern::lazy(p, colored);
        let pattern = if lazy { pattern } else { self.instantiate(&pattern, "")? };
        self.user_patterns.insert(name, pattern);
        Ok(())
    }

    /// A pattern drawn from what it was declared with, the options after its name applied first.
    fn instantiate(&mut self, p: &UserPattern, options: &str) -> Result<UserPattern, Error> {
        let Some(lazy) = p.lazy.clone() else { return Ok(p.clone()) };
        self.begin_group();
        let r = (|| {
            self.process_keys(lazy.defaults.clone(), Some("/pgf/pattern keys"))?;
            self.process_keys(super::tikra_tokens(options), Some("/pgf/pattern keys"))?;
            let point = |e: &mut Engine, v: &[Token], default: Point| -> Result<Point, Error> {
                if v.iter().all(|t| t.is_space()) {
                    return Ok(default);
                }
                if !lazy.tikz {
                    return e.point_of(v.to_vec());
                }
                let text = e.math_string(v.to_vec())?;
                let t = text.trim().trim_start_matches('(').trim_end_matches(')').to_string();
                Ok(e.coordinate(&t)?.p)
            };
            let origin = point(self, &lazy.origin, Point::default())?;
            let size = point(self, &lazy.size, Point::new(PT, PT))?;
            let items = self.draw_tile(lazy.code.clone())?;
            self.st.transform = Transform::default();
            self.st.pending = Transform::default();
            let transform = if lazy.tikz {
                self.process_keys(lazy.tile_options.clone(), Some("/tikz"))?;
                self.st.pending
            } else {
                self.run_tokens(lazy.tile_options.clone())?;
                self.st.transform
            };
            Ok(UserPattern { items, origin, size, transform, colored: p.colored, lazy: None })
        })();
        self.end_group();
        r
    }

    /// What `code` draws, as one tile (or an arrow tip): in a picture of its own, its coordinates kept.
    pub(super) fn draw_tile(&mut self, code: Vec<Token>) -> Result<Vec<Item>, Error> {
        let saved_canvas = std::mem::take(&mut self.canvas);
        let saved_depth = std::mem::replace(&mut self.picture_depth, 1);
        let saved_frozen = std::mem::replace(&mut self.canvas_frozen, false);
        self.begin_group();
        self.begin_scope();
        self.st.transform = Transform::default();
        let r = self.run_tokens(code);
        self.end_scope();
        self.end_group();
        let Canvas { items, .. } = std::mem::replace(&mut self.canvas, saved_canvas);
        self.picture_depth = saved_depth;
        self.canvas_frozen = saved_frozen;
        r?;
        Ok(items)
    }

    /// A pattern by name as a paint, form-only ones in `color`: a declared one, or a meta pattern
    /// (`Lines[angle=45]`).
    pub fn pattern_paint(&mut self, name: &str, color: Color) -> Option<Paint> {
        let name = name.trim();
        let (base, options) = match name.split_once('[') {
            Some((b, o)) => (b.trim(), o.trim_end().trim_end_matches(']')),
            None => (name, ""),
        };
        let p = match self.user_patterns.get(base).cloned() {
            Some(p) => match self.instantiate(&p, options) {
                Ok(p) => p,
                Err(e) => {
                    self.warn(format!("pattern '{base}': {e}"));
                    return None;
                }
            },
            None => self.meta_pattern(name)?,
        };
        let mut items = p.items;
        if !p.colored {
            for i in items.iter_mut() {
                if let Item::Path { fill, stroke, .. } = i {
                    if let Some(f) = fill {
                        f.paint = Paint::Solid(color);
                    }
                    if let Some(s) = stroke {
                        s.color = color;
                    }
                }
            }
        }
        Some(Paint::Tile { items: TileItems(std::rc::Rc::new(items)), origin: p.origin, size: p.size, transform: p.transform })
    }

    /// The patterns.meta library's `Lines`, `Hatch`, `Dots` and `Stars`, with their options.
    fn meta_pattern(&mut self, spec: &str) -> Option<UserPattern> {
        let (kind, options) = match spec.split_once('[') {
            Some((k, o)) => (k.trim(), o.trim_end().trim_end_matches(']')),
            None => (spec, ""),
        };
        if !matches!(kind, "Lines" | "Hatch" | "Dots" | "Stars") {
            return None;
        }
        let star = kind == "Stars";
        let (mut d, mut angle, mut xs, mut ys) = (if star { 3.0 * 0.1 } else { 3.0 * PT }, 0.0, 0.0, 0.0);
        let (mut lw, mut r, mut points) = (self.st.line_width, if star { 0.1 } else { 0.5 * PT }, 5.0);
        for item in crate::keys::split_list(&super::tikra_tokens(options)) {
            let v = crate::tex::tokens_to_string(&item.value.unwrap_or_default());
            let v = v.trim().trim_start_matches('{').trim_end_matches('}').to_string();
            match item.key.as_str() {
                "distance" => d = self.length(&v).ok()?,
                "angle" => angle = self.number(&v).ok()?,
                "xshift" => xs = self.length(&v).ok()?,
                "yshift" => ys = self.length(&v).ok()?,
                "line width" => lw = self.length(&v).ok()?,
                "radius" => r = self.length(&v).ok()?,
                "points" => points = self.number(&v).ok()?,
                _ => {}
            }
        }
        let black = Color::BLACK;
        let stroke = Stroke { width: lw, color: black, ..self.stroke() };
        let fill = Fill { paint: Paint::Solid(black), opacity: 1.0, even_odd: false };
        let p = Point::new;
        let h = d / 2.0;
        let items = match kind {
            "Lines" | "Hatch" => {
                let mut segs = vec![Segment::MoveTo(p(-h, 0.0)), Segment::LineTo(p(h, 0.0))];
                if kind == "Hatch" {
                    segs.extend([Segment::MoveTo(p(0.0, -h)), Segment::LineTo(p(0.0, h))]);
                }
                vec![Item::Path { segments: segs, fill: None, stroke: Some(stroke) }]
            }
            "Dots" => {
                let mut segs = vec![Segment::MoveTo(p(r, 0.0))];
                segs.extend(super::path::arc_segments(Point::default(), r, r, 0.0, 360.0, &Transform::default()));
                segs.push(Segment::Close);
                vec![Item::Path { segments: segs, fill: Some(fill), stroke: None }]
            }
            _ => {
                let a = 180.0 / points;
                let polar = |deg: f64, len: f64| p(len * deg.to_radians().cos(), len * deg.to_radians().sin());
                let mut segs = vec![Segment::MoveTo(polar(90.0, r))];
                for i in 1..=(points as usize) {
                    let i = i as f64;
                    segs.push(Segment::LineTo(polar(90.0 + 2.0 * i * a - a, 0.5 * r)));
                    segs.push(Segment::LineTo(polar(90.0 + 2.0 * i * a, r)));
                }
                segs.push(Segment::Close);
                vec![Item::Path { segments: segs, fill: Some(fill), stroke: None }]
            }
        };
        let transform = Transform::translation(xs, ys).compose(&Transform::rotation(angle));
        Some(UserPattern { items, origin: p(-h, -h), size: p(d, d), transform, colored: false, lazy: None })
    }

    /// `\pgfsetfillpattern{name}{color}`: the basic layer's fills from here on use the pattern.
    pub fn set_fill_pattern(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        let color = self.argument_string()?;
        let c = self.color(color.trim()).unwrap_or(Color::BLACK);
        self.st.pattern_name = Some(name.trim().to_string());
        self.st.pattern_color = Some(c);
        self.st.fill_pattern = true;
        Ok(())
    }
}

impl Engine {
    /// `\pgfarrowsdeclare{start}{end}{extents}{code}`: a tip of the old kind, its extents as
    /// `\pgfarrowsleftextend{…}` and `\pgfarrowsrightextend{…}` give them and its drawing as the
    /// code makes it, with the line width of the moment.
    pub fn arrows_declare(&mut self) -> Result<(), Error> {
        let start = self.argument_string()?;
        let end = self.argument_string()?;
        let extents = self.argument()?;
        let code = self.argument()?;
        let text = crate::tex::tokens_to_string(&extents);
        let extent = |e: &mut Engine, name: &str| -> Result<f64, Error> {
            let Some(rest) = text.split(&format!("\\{name}")).nth(1) else { return Ok(0.0) };
            let arg = rest.trim_start().strip_prefix('{').and_then(|r| r.split_once('}')).map_or("0pt", |(a, _)| a);
            Ok(e.length(arg)? / PT)
        };
        let (left, right) = (extent(self, "pgfarrowsleftextend")?, extent(self, "pgfarrowsrightextend")?);
        let items = self.draw_tile(code)?;
        let to_pt = Transform::scaling(1.0 / PT, 1.0 / PT);
        let paths = items
            .into_iter()
            .filter_map(|i| match i {
                Item::Path { segments, fill, stroke } => {
                    let segs = crate::canvas::transform_segments(&segments, &to_pt);
                    Some((segs, fill.is_some(), stroke.is_some(), stroke.map_or(0.0, |s| s.width / PT)))
                }
                _ => None,
            })
            .collect();
        super::arrows::declare_tip(super::arrows::DeclaredTip { names: (start.trim().to_string(), end.trim().to_string()), left, right, paths });
        Ok(())
    }
}
