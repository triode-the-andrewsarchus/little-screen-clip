//! The spy library (`tikzlibraryspy.code.tex`): magnified parts of a picture.
//!
//! A `spy scope` (or `spy using outlines`, `spy using overlays`) collects what its scope draws and
//! the `\spy [options] on (point) in node [options] at (place);` given in it. When the scope ends,
//! what it drew stays, and each spy then draws two nodes: one on the point, as large as the other
//! divided by the lens (`magnification=3` is `lens={scale=3}`), and one at the place, showing what
//! the scope drew, transformed by the lens around the point and clipped to the node's shape. pgf
//! copies the scope's box there; tikra copies the canvas items. `connect spies` draws a line
//! between the two nodes.

use crate::canvas::{transform_segments, Item};
use crate::scene::{Point, Transform};
use crate::tex::{self, Meaning, Token};
use crate::Error;

use super::shapes::{Layer, Usage};
use super::{tikra_tokens, Engine};

/// A spy: its options, the point it looks at, and what follows `in node`.
#[derive(Debug, Clone)]
struct Spy {
    options: Vec<Token>,
    on: String,
    node: String,
}

/// An open spy scope: where its drawing starts on the canvas, and its spies.
#[derive(Debug, Clone, Default)]
pub struct SpyScope {
    start: usize,
    spies: Vec<Spy>,
}

/// The library's styles, read as TeX with `@` a letter.
pub const DEFINITIONS: &str = r"
\tikzset{
  spy scope/.style={execute at begin scope={\tikra@spybegin},execute at end scope={\tikra@spyend},
    tikra@spy@style/.style={#1},tikra@spy@reset},
  spy using outlines/.style={spy scope={every spy on node/.style={very thin,draw},
    every spy in node/.style={thick,draw},#1}},
  spy using overlays/.style={spy scope={every spy on node/.style={fill,fill opacity=0.2,text opacity=1},
    every spy in node/.style={fill,fill opacity=0.2,text opacity=1},#1}},
  spy scope/.default=, spy using outlines/.default=, spy using overlays/.default=,
  connect spies/.style={spy connection path={\draw[thin] (tikzspyonnode) -- (tikzspyinnode);}},
  size/.style={minimum size={#1}}, height/.style={minimum height={#1}}, width/.style={minimum width={#1}},
  lens/.store in=\tikra@spy@lens, lens=, magnification/.style={lens={scale={#1}}},
  spy connection path/.store in=\tikra@spy@path, spy connection path=,
  tikra@spy@style/.style={},
  tikra@spy@reset/.style={black,thin,solid,opaque,line cap=butt,line join=miter},
}
";

/// The text of a parameterless macro (`\tikz@lib@spy@lens`), as tokens.
pub(super) fn macro_body(e: &Engine, name: &str) -> Vec<Token> {
    match e.tex.meaning_of(name) {
        Meaning::Macro(m) => tex::substitute(&m.body, &[]),
        _ => Vec::new(),
    }
}

/// A canvas point as a coordinate of TikZ text.
fn point_text(p: Point) -> String {
    format!("tikra cs:{}pt,{}pt", p.x / crate::PT, p.y / crate::PT)
}

impl Engine {
    /// `execute at begin scope` of a spy scope.
    pub fn spy_begin(&mut self) {
        self.spy_scopes.push(SpyScope { start: self.canvas.items.len(), spies: Vec::new() });
    }

    /// `\spy [options] on (point) in node …;`: kept for the end of the spy scope.
    pub fn spy_command(&mut self) -> Result<(), Error> {
        let options = self.optional()?.unwrap_or_default();
        let rest = self.skip_statement()?;
        let parsed = rest.split_once("in node").and_then(|(on, node)| Some((on.trim().strip_prefix("on")?.trim().to_string(), node.trim().to_string())));
        let Some((on, node)) = parsed else {
            self.warn(format!("\\spy without 'on (point) in node': '{}' ignored", rest.trim()));
            return Ok(());
        };
        match self.spy_scopes.last_mut() {
            Some(s) => s.spies.push(Spy { options, on, node }),
            None => self.warn("\\spy outside a spy scope: ignored"),
        }
        Ok(())
    }

    /// `execute at end scope` of a spy scope: the spies, over what the scope drew.
    pub fn spy_end(&mut self) -> Result<(), Error> {
        let Some(scope) = self.spy_scopes.pop() else { return Ok(()) };
        let mut drawing: Vec<Item> = self.canvas.items[scope.start.min(self.canvas.items.len())..].to_vec();
        // clips and groups of the scope that are still open are closed in the copy
        let open = drawing.iter().fold(0i64, |d, i| match i {
            Item::Clip { .. }
            | Item::Group { .. }
            | Item::Fade { .. }
            | Item::Blend { .. }
            | Item::Crisp
            | Item::Transform { .. }
            | Item::FadePicture { .. }
            | Item::Animated { .. }
            | Item::Point(_) => d + 1,
            Item::End => d - 1,
            _ => d,
        });
        drawing.extend((0..open.max(0)).map(|_| Item::End));
        for spy in scope.spies {
            self.begin_group();
            self.begin_scope();
            let r = self.spy(&spy, &drawing);
            self.end_scope();
            self.end_group();
            r?;
        }
        Ok(())
    }

    fn spy(&mut self, spy: &Spy, drawing: &[Item]) -> Result<(), Error> {
        let mut options = tikra_tokens("tikra@spy@style,");
        options.extend(spy.options.clone());
        self.process_keys(options, Some("/tikz"))?;
        // the lens, from the identity
        let saved = (self.st.transform, self.st.pending);
        (self.st.transform, self.st.pending) = (Transform::default(), Transform::default());
        let r = self.process_keys(macro_body(self, "tikra@spy@lens"), Some("/tikz"));
        let lens = self.st.transform.compose(&self.st.pending);
        (self.st.transform, self.st.pending) = saved;
        r?;
        // the point is read when the scope ends, with the macros it names as they are then
        let on = self.tex.expand_to_string(tikra_tokens(&spy.on))?;
        let on = self.coordinate(on.trim().trim_start_matches('(').trim_end_matches(')'))?.p;
        // the node on the point: the other one's shape through the inverse lens
        let shrink = Transform { e: 0.0, f: 0.0, ..lens }.inverse();
        let on_node = format!(
            "\\node[alias=tikzspyonnode,inner sep=0pt,outer sep=0pt,every spy on node/.try,cm={{{},{},{},{},(0,0)}}] at ({}) {{}};",
            shrink.a,
            shrink.b,
            shrink.c,
            shrink.d,
            point_text(on)
        );
        self.run_tokens(tikra_tokens(&on_node))?;
        // the node at the place, with the magnified drawing in it
        let first = self.canvas.items.len();
        // at the point unless the spy says where
        let placed = spy.node.contains("at (") || spy.node.contains("at(");
        let at = if placed { String::new() } else { format!("at={{({})}},", point_text(on)) };
        let in_node = format!("\\node[alias=tikzspyinnode,inner sep=0pt,outer sep=0pt,{at}every spy in node/.try] {} {{}};", spy.node);
        self.run_tokens(tikra_tokens(&in_node))?;
        if let Some(info) = self.nodes.get("tikzspyinnode").cloned() {
            let border: Vec<_> = info.geometry.paths.iter().filter(|p| p.layer == Layer::Background && matches!(p.usage, Usage::Node)).flat_map(|p| p.segments.clone()).collect();
            let center = info.center();
            let to_place = Transform::translation(center.x, center.y).compose(&lens).compose(&Transform::translation(-on.x, -on.y));
            let border = transform_segments(&border, &info.transform);
            let mut picture = vec![Item::Clip { segments: border.clone(), even_odd: false }];
            picture.extend(drawing.iter().cloned().map(|mut i| {
                i.transform(&to_place);
                i
            }));
            picture.push(Item::End);
            self.insert_path_picture(first, &border, picture);
        }
        let connection = macro_body(self, "tikra@spy@path");
        self.run_tokens(connection)
    }

    /// Puts a path picture into the node drawn from item `first` on: after the fill of its
    /// background (the last path on `border`, not a shadow), before its stroke, as `\tikz@finish` does.
    fn insert_path_picture(&mut self, first: usize, border: &[crate::scene::Segment], picture: Vec<Item>) {
        let items = &self.canvas.items;
        let on_border = |k: &usize| matches!(&items[*k], Item::Path { segments, .. } if segments.as_slice() == border);
        // the last one: a shadow before it has the same outline
        let at = (first..items.len()).rfind(on_border).or_else(|| (first..items.len()).find(|&k| matches!(items[k], Item::Path { .. })));
        let Some(k) = at else {
            self.canvas.items.extend(picture);
            return;
        };
        let Item::Path { segments, fill, stroke } = self.canvas.items[k].clone() else { return };
        let mut replaced = Vec::new();
        if fill.is_some() {
            replaced.push(Item::Path { segments: segments.clone(), fill, stroke: None });
        }
        replaced.extend(picture);
        if stroke.is_some() {
            replaced.push(Item::Path { segments, fill: None, stroke });
        }
        self.canvas.items.splice(k..=k, replaced);
    }
}
