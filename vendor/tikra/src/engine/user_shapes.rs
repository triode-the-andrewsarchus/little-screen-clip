//! Shapes declared in the document: `\pgfdeclareshape{name}{…}`. What a node of the shape has is
//! computed when the node is made, as pgf does: the saved anchors, dimensions and macros
//! (`\savedanchor`, `\saveddimen`, `\savedmacro`) from the node's text box and sizes, then the
//! anchors (`\anchor{name}{code}`) from them, then the paths of each layer (`\backgroundpath{code}`,
//! `\behindbackgroundpath{code}`, …). The shape it inherits from (`\inheritsavedanchors[from=…]`)
//! gives what it does not declare: its saved anchors, anchors, border and inherited paths.

use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Token};
use crate::{Error, PT};

use super::shapes::{Border, Geometry, Layer, ShapePath, ShapeText, Usage};
use super::Engine;

/// The box register that holds the node's text while its shape is computed (`\pgfnodeparttextbox`).
pub const TEXT_BOX: i64 = 250;

/// What a `\saved…` command saves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Saved {
    /// a point (`\pgf@x`, `\pgf@y` after the code)
    Anchor,
    /// a dimension (`\pgf@x` after the code)
    Dimen,
    /// the macro the code defines
    Macro,
}

/// A declared shape: the shape whose geometry it takes, its saved values, anchors and paths' code.
#[derive(Debug, Clone, Default)]
pub struct UserShape {
    pub base: String,
    /// `\savedanchor\name{code}` and the others, in order
    pub saved: Vec<(Saved, String, Vec<Token>)>,
    pub anchors: Vec<(String, Vec<Token>)>,
    /// `\inheritanchorborder[from=…]`
    pub border: Option<String>,
    /// the layers whose paths come from the inherited shape (`\inheritbackgroundpath`, …)
    pub inherited: Vec<Layer>,
    pub paths: Vec<(Layer, Vec<Token>)>,
}

/// The node's sizes the shape's code reads: its text box (wd, ht, dp), minimum width and height,
/// inner and outer separations.
#[derive(Debug, Clone, Copy)]
pub struct ShapeInput {
    pub text: (f64, f64, f64),
    pub minimum: Point,
    pub inner: Point,
    pub outer: Point,
}

/// What `\pgfusepath` and `\pgftext` give while a shape's path code runs: paths and texts of the
/// shape, in the layer being run.
#[derive(Debug, Default)]
pub struct ShapeCapture {
    pub layer: Option<Layer>,
    pub paths: Vec<ShapePath>,
    pub texts: Vec<ShapeText>,
}

fn layer_of(cs: &str) -> Option<Layer> {
    Some(match cs {
        "behindbackgroundpath" => Layer::BehindBackground,
        "backgroundpath" => Layer::Background,
        "beforebackgroundpath" => Layer::BeforeBackground,
        "behindforegroundpath" => Layer::BehindForeground,
        "foregroundpath" => Layer::Foreground,
        "beforeforegroundpath" => Layer::BeforeForeground,
        _ => return None,
    })
}

/// The layers in the order pgf runs them.
const LAYERS: [Layer; 6] = [Layer::BehindBackground, Layer::Background, Layer::BeforeBackground, Layer::BehindForeground, Layer::Foreground, Layer::BeforeForeground];

/// Code with `\relax` after it: a dimension at its end is not read on past it.
fn ended(code: &[Token]) -> Vec<Token> {
    let mut v = code.to_vec();
    v.push(Token::cs("relax"));
    v
}

fn sp(v: f64) -> i64 {
    (v / PT * 65536.0).round() as i64
}

impl Engine {
    /// `\pgfdeclareshape{name}{body}`.
    pub fn declare_shape(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        let body = self.argument()?;
        let mut shape = UserShape { base: "rectangle".into(), ..UserShape::default() };
        let mut rest: std::collections::VecDeque<Token> = body.into_iter().collect();
        // the argument at the front, a group's content or one token
        let take_group = |rest: &mut std::collections::VecDeque<Token>| -> Vec<Token> {
            while rest.front().is_some_and(|t| t.is_space()) {
                rest.pop_front();
            }
            match rest.pop_front() {
                Some(Token::Char(_, tex::Cat::Begin)) => {
                    let mut depth = 1;
                    let mut out = Vec::new();
                    while let Some(t) = rest.pop_front() {
                        match t {
                            Token::Char(_, tex::Cat::Begin) => depth += 1,
                            Token::Char(_, tex::Cat::End) => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        out.push(t);
                    }
                    out
                }
                Some(t) => vec![t],
                None => Vec::new(),
            }
        };
        // `[from=name]`, right after the command (a control word: no space token follows it)
        let take_option = |rest: &mut std::collections::VecDeque<Token>| -> String {
            if !rest.front().is_some_and(|t| t.is_char('[')) {
                return String::new();
            }
            rest.pop_front();
            let mut out = Vec::new();
            while let Some(t) = rest.pop_front() {
                if t.is_char(']') {
                    break;
                }
                out.push(t);
            }
            let s = tex::tokens_to_string(&out);
            s.split_once('=').map_or(s.clone(), |(_, v)| v.to_string()).trim().to_string()
        };
        while let Some(t) = rest.pop_front() {
            let Token::Cs(cs) = &t else { continue };
            match &**cs {
                "inheritsavedanchors" => shape.base = take_option(&mut rest),
                "inheritanchorborder" => shape.border = Some(take_option(&mut rest)),
                "inheritanchor" => {
                    // the inherited shape's anchors are kept
                    take_option(&mut rest);
                    take_group(&mut rest);
                }
                c if c.starts_with("inherit") && layer_of(&c[7..]).is_some() => {
                    take_option(&mut rest);
                    shape.inherited.extend(layer_of(&c[7..]));
                }
                c if layer_of(c).is_some() => {
                    let code = take_group(&mut rest);
                    shape.paths.extend(layer_of(c).map(|l| (l, code)));
                }
                "savedanchor" | "saveddimen" | "savedmacro" => {
                    let kind = match &**cs {
                        "savedanchor" => Saved::Anchor,
                        "saveddimen" => Saved::Dimen,
                        _ => Saved::Macro,
                    };
                    let target = take_group(&mut rest);
                    let code = take_group(&mut rest);
                    if let [Token::Cs(n)] = &target[..] {
                        shape.saved.push((kind, n.to_string(), code));
                    }
                }
                "anchor" => {
                    let a = tex::tokens_to_string(&take_group(&mut rest));
                    let code = take_group(&mut rest);
                    shape.anchors.push((a.split_whitespace().collect::<Vec<_>>().join(" "), code));
                }
                "anchorborder" | "nodeparts" => {
                    self.warn(format!("\\pgfdeclareshape{{{}}}: \\{cs} not supported: the inherited ones used", name.trim()));
                    take_group(&mut rest);
                }
                _ => {}
            }
        }
        if !super::shapes::is_shape(&shape.base) {
            self.warn(format!("\\pgfdeclareshape{{{}}}: shape '{}' to inherit from not supported: a rectangle", name.trim(), shape.base));
            shape.base = "rectangle".into();
        }
        self.user_shapes.insert(name.trim().to_string(), shape);
        Ok(())
    }

    /// The geometry of a node of a declared shape: the inherited shape's, with the declared saved
    /// values, anchors and paths, their code run in the node's coordinates (those of pgf's shapes:
    /// the text's origin at 0).
    pub fn user_shape_geometry(&mut self, name: &str, shape: &UserShape, mut g: Geometry, input: ShapeInput) -> Result<Geometry, Error> {
        self.begin_group();
        let r = self.user_shape_in_group(name, shape, &mut g, input);
        self.end_group();
        r?;
        Ok(g)
    }

    fn user_shape_in_group(&mut self, name: &str, shape: &UserShape, g: &mut Geometry, input: ShapeInput) -> Result<(), Error> {
        let set_point = |e: &mut Engine, name: &str, p: Point| {
            let code = super::tikra_tokens(&format!("\\pgf@x={}pt\\pgf@y={}pt\\relax", p.x / PT, p.y / PT));
            e.tex.set_meaning(&Token::cs(name), super::macro_tokens(code), false);
        };
        // what the code reads: the text box, the sizes as `\pgfshape…` macros
        let (wd, ht, dp) = input.text;
        // (a box register only here: outside, `\setbox\pgfnodeparttextbox` names a node part's box)
        self.run_tokens(super::tikra_tokens(&format!("\\chardef\\pgfnodeparttextbox={TEXT_BOX}\\relax")))?;
        self.tex.box_dims.insert(TEXT_BOX, [sp(wd), sp(ht), sp(dp)]);
        for (m, v) in [
            ("pgfshapeminwidth", input.minimum.x),
            ("pgfshapeminheight", input.minimum.y),
            ("pgfshapeinnerxsep", input.inner.x),
            ("pgfshapeinnerysep", input.inner.y),
            ("pgfshapeouterxsep", input.outer.x),
            ("pgfshapeouterysep", input.outer.y),
        ] {
            self.tex.define_string(m, &format!("{}pt", v / PT), false);
        }
        self.st.transform = Transform::default();
        // the inherited shape's saved anchors
        for (n, a) in [("southwest", "south west"), ("northeast", "north east"), ("centerpoint", "center")] {
            if let Some(p) = g.anchor(a) {
                set_point(self, n, p);
            }
        }
        if let Border::Ellipse(r) = g.border {
            self.tex.define_string("radius", &format!("{}pt", r.x / PT), false);
        }
        // the declared ones, in order, each from what the ones before saved
        let mut points = Vec::new();
        for (kind, n, code) in &shape.saved {
            self.begin_group();
            let r = self.run_tokens(ended(code));
            let (p, m) = (self.pgf_xy(), self.tex.meaning(&Token::cs(n)));
            self.end_group();
            r?;
            match kind {
                Saved::Anchor => {
                    set_point(self, n, p);
                    points.push((n.as_str(), p));
                }
                Saved::Dimen => self.tex.define_string(n, &format!("{}pt", p.x / PT), false),
                Saved::Macro => self.tex.set_meaning(&Token::cs(n), m, false),
            }
        }
        // the anchors: each also `\pgf@anchor@shape@anchor`, as the code may call them
        for (a, p) in g.anchors.clone() {
            set_point(self, &format!("pgf@anchor@{name}@{a}"), p);
        }
        for (a, code) in &shape.anchors {
            self.tex.set_meaning(&Token::cs(&format!("pgf@anchor@{name}@{a}")), super::macro_tokens(code.clone()), false);
        }
        let mut declared = Vec::new();
        for (a, code) in &shape.anchors {
            self.begin_group();
            let r = self.run_tokens(ended(code));
            let p = self.pgf_xy();
            self.end_group();
            r?;
            declared.push((a.clone(), p));
        }
        g.anchors.retain(|(n, _)| !declared.iter().any(|(d, _)| d == n));
        if let Some((_, c)) = declared.iter().find(|(n, _)| n == "center") {
            g.center = *c;
        }
        g.anchors.extend(declared);
        // the rectangle's border around the shape's own corners, measured from their middle
        let saved = |n: &str| points.iter().find(|(m, _)| *m == n).map(|(_, p)| *p);
        if let (Some("rectangle"), Some(sw), Some(ne)) = (shape.border.as_deref(), saved("southwest"), saved("northeast")) {
            g.center = sw.lerp(ne, 0.5);
            g.border = Border::Rectangle(ne.minus(sw).scale(0.5));
        }
        // the inherited shape's paths only in the layers inherited
        g.paths.retain(|p| shape.inherited.contains(&p.layer));
        if !shape.paths.is_empty() {
            g.roundable = false;
        }
        // the code of each layer, in pgf's order: what it uses is drawn as it says; the path left
        // over goes on to the background or foreground path, which is used as the node says
        let mut left = Vec::new();
        for layer in LAYERS {
            for (_, code) in shape.paths.iter().filter(|(l, _)| *l == layer) {
                let saved_path = std::mem::take(&mut self.pgf_path);
                self.pgf_path.segments = std::mem::take(&mut left);
                let saved_capture = self.shape_capture.replace(ShapeCapture { layer: Some(layer), ..ShapeCapture::default() });
                self.begin_group();
                let r = self.run_tokens(ended(code));
                self.end_group();
                let capture = std::mem::replace(&mut self.shape_capture, saved_capture).unwrap_or_default();
                let path = std::mem::replace(&mut self.pgf_path, saved_path);
                r?;
                g.paths.extend(capture.paths);
                g.texts.extend(capture.texts);
                left = path.segments;
            }
            if matches!(layer, Layer::Background | Layer::Foreground) && !left.is_empty() {
                let segments: Vec<Segment> = std::mem::take(&mut left);
                g.paths.push(ShapePath { layer, segments, usage: Usage::Node });
            }
        }
        Ok(())
    }
}
