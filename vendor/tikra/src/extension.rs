//! Libraries from outside tikra: the keys and TeX definitions of a package drawn on TikZ (a circuit
//! package, a geometry package), registered on the engine next to TikZ's own.
//!
//! A [`Library`] gives two things, as TikZ's libraries do: definitions run as TeX once (styles with
//! `\tikzset`, macros with `\def`, `@` a letter), and *native keys*, Rust functions run when a key is
//! used. A native key works through a [`Context`], a small part of the engine: running keys and
//! TeX, reading numbers, lengths and coordinates, the coordinate system, and labels. Lengths are in
//! centimeters ([`PT`] centimeters to the point), as in the rest of tikra.
//!
//! ```
//! use tikra::extension::{Context, Library, Tokens};
//!
//! // `double line=color`: the path drawn twice, wide in the color and thin in white
//! fn double_line(cx: &mut Context, value: Option<Tokens>) -> Result<(), tikra::Error> {
//!     let color = cx.expand(value.unwrap_or_default())?;
//!     cx.keys(Tokens::text(&format!("double={color}, double distance=1pt")))
//! }
//!
//! fn definitions() -> String {
//!     r"\tikzset{road/.style={double line=gray, line width=2pt}}".into()
//! }
//!
//! static ROADS: Library = Library { name: "roads", definitions, keys: &[("/tikz/double line", double_line)] };
//!
//! let code = r"\begin{tikzpicture} \draw[road] (0,0) -- (2,0); \end{tikzpicture}";
//! let r = tikra::convert_with_libraries(code, &tikra::Options::default(), &[&ROADS]).unwrap();
//! assert!(r.warnings.is_empty());
//! ```
//!
//! The libraries run inside tikra with the same limits as the figure (expansions, tokens, nesting):
//! what a library cannot do is everything outside these calls, as files, the network or programs.

use crate::engine::{macro_tokens, tikra_tokens, Engine};
use crate::tex::{Cat, Meaning, Token};

pub use crate::scene::{Point, Transform};
pub use crate::{Error, PT};

/// A native key: run with the key's value (`None` without one, as in `[circuit]`).
pub type KeyFn = fn(&mut Context, Option<Tokens>) -> Result<(), Error>;

/// A library: its name, its TeX definitions and its native keys.
pub struct Library {
    /// The library's name, unique among the libraries of a conversion (the engine prepared with a
    /// set of libraries is kept by their names).
    pub name: &'static str,
    /// TeX run once after the keys are defined, with `@` a letter.
    pub definitions: fn() -> String,
    /// Each native key, by its full path (`/tikz/resistor`).
    pub keys: &'static [(&'static str, KeyFn)],
}

/// TeX tokens: a key's value, or code to run. Built from text and spliced as tokens, so a value goes
/// on unexpanded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tokens(pub(crate) Vec<Token>);

impl Tokens {
    pub fn new() -> Tokens {
        Tokens(Vec::new())
    }

    /// The text as TeX reads it, with `@` a letter.
    pub fn text(text: &str) -> Tokens {
        Tokens(tikra_tokens(text))
    }

    /// These tokens, then the text.
    pub fn and_text(mut self, text: &str) -> Tokens {
        self.0.extend(tikra_tokens(text));
        self
    }

    /// These tokens, then those.
    pub fn and(mut self, other: Tokens) -> Tokens {
        self.0.extend(other.0);
        self
    }

    /// The tokens in braces, as one argument.
    pub fn braced(self) -> Tokens {
        let mut v = Vec::with_capacity(self.0.len() + 2);
        v.push(Token::Char('{', Cat::Begin));
        v.extend(self.0);
        v.push(Token::Char('}', Cat::End));
        Tokens(v)
    }

    /// Without the spaces around them.
    pub fn trim(&self) -> Tokens {
        Tokens(crate::keys::trim(&self.0).to_vec())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text of the tokens, unexpanded (`\alpha` stays `\alpha`).
    pub fn to_text(&self) -> String {
        crate::tex::tokens_to_string(&self.0)
    }

    /// `n` arguments in braces (`{a}{b}`), empty where missing.
    pub fn arguments(&self, n: usize) -> Vec<Tokens> {
        crate::engine::options::split_arguments(&self.0, n).into_iter().map(Tokens).collect()
    }

    /// The `key=value` list: each key's name, and its value if it has one.
    pub fn key_list(&self) -> Vec<(String, Option<Tokens>)> {
        crate::keys::split_list(&self.0).into_iter().map(|i| (i.key, i.value.map(Tokens))).collect()
    }

    /// `[options] rest`: the options in brackets at the start (empty without) and the rest.
    pub fn leading_options(&self) -> (Tokens, Tokens) {
        let v = crate::keys::trim(&self.0);
        if v.first().is_some_and(|t| t.is_char('[')) {
            let mut depth = 0;
            if let Some(end) = v.iter().position(|t| {
                if t.is_char('[') {
                    depth += 1;
                } else if t.is_char(']') {
                    depth -= 1;
                }
                depth == 0
            }) {
                return (Tokens(v[1..end].to_vec()), Tokens(v[end + 1..].to_vec()));
            }
        }
        (Tokens::new(), Tokens(v.to_vec()))
    }
}

/// A node shape drawn by a library: computed for each node, as tikra's own shapes are, from what
/// the node holds. Declared with [`Context::declare_shape`].
pub type ShapeFn = fn(&mut Context, &ShapeInput) -> Result<NodeShape, Error>;

/// What a library's shape is computed from, in cm.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeInput {
    /// The shape's name, as declared: one function may draw several.
    pub name: String,
    /// The node's text box: its width, and its height and depth around the baseline.
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    /// `inner xsep` and `inner ysep`.
    pub inner_sep: Point,
    /// The line width in force.
    pub line_width: f64,
    /// The node's name with its `name prefix` (`T1`), to name what the shape adds after it
    /// (`T1-L1`); `None` for a node without one.
    pub node: Option<String>,
    /// The node's fill color when it is filled, and its line color when it is drawn, as xcolor
    /// expressions (`rgb,1:red,1;green,0.5;blue,0`: braced in a key list); `None` when not.
    pub fill: Option<String>,
    pub draw: Option<String>,
    /// The node's own transformation, without its position: `xscale`, `rotate`, the picture's with
    /// `transform shape`. Mirrored when its determinant is negative. (Not what `sloped` or a
    /// nonlinear transformation adds, which depend on where the node goes.)
    pub transform: Transform,
}

/// A library's shape, in the node's coordinates (cm, y up): the node puts it where `at` and
/// `anchor` say and turns it as its transformations do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeShape {
    /// The `center` anchor; borders are found from it.
    pub center: Point,
    /// The named anchors (`north`, `out`, `B`): a node's anchor not named here is a border point
    /// when it is an angle, else unknown.
    pub anchors: Vec<(String, Point)>,
    /// Where lines to the node stop.
    pub border: ShapeBorder,
    /// Where the origin of the text box goes (the left end of its baseline).
    pub text: Point,
    /// What it draws, in order.
    pub paths: Vec<ShapePath>,
    /// Texts of its own, set as node text is and turned with the node (an amplifier's `+` and `−`,
    /// a chip's pin names), drawn over its paths.
    pub texts: Vec<ShapeText>,
    /// Points that count in the picture's size besides its paths (its text does not).
    pub extent: Vec<Point>,
}

/// Where lines to a library's node stop.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ShapeBorder {
    /// At the center: lines reach it.
    #[default]
    Point,
    /// A rectangle around the center, by its half width and half height.
    Rectangle(Point),
    /// A closed outline: lines stop where they enter it.
    Outline(Vec<Point>),
}

/// A path of a library's shape and how it is painted: as `paint` says, in the node's colors, solid
/// and with the node's caps unless it says otherwise.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapePath {
    pub segments: Vec<PathSegment>,
    pub paint: ShapePaint,
    /// An xcolor expression (`red`, `cyan!30`) for the stroke or fill instead of the node's.
    pub color: Option<String>,
    /// A stroke's dash pattern: on and off lengths in cm, as `dash pattern` (from its start).
    pub dash: Option<Vec<f64>>,
    /// A stroke's line caps.
    pub cap: Option<LineCap>,
    /// A fill's opacity instead of the node's `fill opacity`.
    pub opacity: Option<f64>,
}

/// How the ends of a stroke are drawn (`line cap`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    /// Square, half the line width past the end.
    Rect,
}

/// A text a library's shape draws, in the node's coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeText {
    /// TeX, as a node's text (`$+$`, `CLK`, `\textbf{=1}`).
    pub text: String,
    /// The point its `anchor` goes to.
    pub at: Point,
    pub anchor: TextAnchor,
    /// Font commands instead of the node's (`\scriptsize`).
    pub font: Option<String>,
    /// Degrees counterclockwise, around `at`, in the node's coordinates (a label along a side).
    pub turn: f64,
    /// Kept upright when the node turns: its place follows the node, its box does not turn with
    /// it (a turned chip's pin numbers).
    pub upright: bool,
}

/// The point of a text's box that goes where a [`ShapeText`] says, as a node's anchors with no
/// inner sep: `Base…` on its baseline, `Mid…` half an ex above it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAnchor {
    #[default]
    Center,
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
    Base,
    BaseEast,
    BaseWest,
    Mid,
    MidEast,
    MidWest,
}

/// A segment of a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathSegment {
    MoveTo(Point),
    LineTo(Point),
    /// Two control points, then the end.
    CurveTo(Point, Point, Point),
    Close,
}

/// How a path of a library's shape is painted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ShapePaint {
    /// Stroked in the node's line color, this wide (cm), whether the node is `draw`n or not.
    Stroke(f64),
    /// Filled with the node's `fill`, when it has one (with [`ShapePath::color`], in that color,
    /// still only when the node is filled).
    #[default]
    Fill,
    /// Filled with the node's `fill`, or white without one (an open terminal hides the wire under
    /// it).
    Background,
    /// Filled with the node's line color (or [`ShapePath::color`]).
    Solid,
}

/// A label of a node (`label=…` or `pin=…`), as the node's options gave it.
#[derive(Clone, Debug)]
pub struct Label {
    spec: Vec<Token>,
    pin: bool,
}

/// The part of the engine a native key works with.
pub struct Context<'a> {
    pub(crate) e: &'a mut Engine,
}

impl Context<'_> {
    /// Runs keys under `/tikz`, as in `\tikzset{…}`.
    pub fn keys(&mut self, list: Tokens) -> Result<(), Error> {
        self.e.process_keys(list.0, Some("/tikz"))
    }

    /// Runs keys under `path` (`/pgf/decoration`).
    pub fn keys_at(&mut self, path: &str, list: Tokens) -> Result<(), Error> {
        self.e.process_keys(list.0, Some(path))
    }

    /// Runs TeX: commands, paths, `\node`s.
    pub fn run(&mut self, code: Tokens) -> Result<(), Error> {
        self.e.run_tokens(code.0)
    }

    /// The tokens expanded to text, trimmed (`\tikztostart` to the name of the start).
    pub fn expand(&mut self, tokens: Tokens) -> Result<String, Error> {
        Ok(self.e.tex.expand_to_string(tokens.0)?.trim().to_string())
    }

    /// `\def\name{tokens}`, local to the group.
    pub fn define(&mut self, name: &str, body: Tokens) {
        self.e.tex.set_meaning(&Token::cs(name), macro_tokens(body.0), false);
    }

    /// `\def\name{text}` with the text as characters, local or global (`\gdef`).
    pub fn define_text(&mut self, name: &str, text: &str, global: bool) {
        self.e.tex.define_string(name, text, global);
    }

    /// `\def\name{tex}` with the TeX read as tokens (`\tikzlastnode` stays a control sequence, `@` a
    /// letter), local or global (`\gdef`).
    pub fn define_tex(&mut self, name: &str, tex: &str, global: bool) {
        self.e.tex.set_meaning(&Token::cs(name), macro_tokens(tikra_tokens(tex)), global);
    }

    /// `\pgfkeyssetvalue{path}{text}`: the value of the key at `path`, local to the group.
    pub fn set_key_value(&mut self, path: &str, text: &str) {
        self.e.tex.define_string(&crate::keys::macro_name(path), text, false);
    }

    /// The body of `\name`, a macro without parameters; empty if it is not one.
    pub fn macro_body(&self, name: &str) -> Tokens {
        match self.e.tex.meaning_of(name) {
            Meaning::Macro(m) => Tokens(crate::tex::substitute(&m.body, &[])),
            _ => Tokens::new(),
        }
    }

    /// [`Context::macro_body`] as text, trimmed.
    pub fn macro_text(&self, name: &str) -> String {
        self.macro_body(name).to_text().trim().to_string()
    }

    /// A pgfmath number (`2*3`, `\x+1`).
    pub fn number(&mut self, tokens: Tokens) -> Result<f64, Error> {
        self.e.number_of(tokens.0)
    }

    /// A pgfmath length in cm (`3pt`, `1cm+2mm`; a number alone is in pt).
    pub fn length(&mut self, tokens: Tokens) -> Result<f64, Error> {
        self.e.length_of(tokens.0)
    }

    /// The point of a coordinate's text (`1,2`, `A`, `A.north`), on the canvas, in cm.
    pub fn point(&mut self, coordinate: &str) -> Result<Point, Error> {
        Ok(self.e.coordinate(coordinate)?.p)
    }

    /// The coordinate system in force (picture to canvas).
    pub fn transform(&self) -> Transform {
        self.e.st.transform
    }

    pub fn set_transform(&mut self, t: Transform) {
        self.e.st.transform = t;
    }

    /// The line width in force, in cm.
    pub fn line_width(&self) -> f64 {
        self.e.st.line_width
    }

    /// Whether the current stroke is dashed.
    pub fn is_dashed(&self) -> bool {
        self.e.st.dash.as_ref().is_some_and(|d| !d.0.is_empty())
    }

    /// Whether `transform shape` is on: nodes turn and scale with the picture.
    pub fn transform_shape(&self) -> bool {
        self.e.st.node.transform_shape
    }

    /// `\pgfarrowsdeclarealias`-like: the tip `name` means the tip specification `means`
    /// (`stealth[length=3pt]`), in the current group.
    pub fn declare_arrow_tip(&mut self, name: &str, means: &str) {
        std::rc::Rc::make_mut(&mut self.e.st.tips).declare(name, means);
    }

    /// A TeX and TikZ group: what changes inside is undone at [`Context::end_group`].
    pub fn begin_group(&mut self) {
        self.e.begin_group();
    }

    pub fn end_group(&mut self) {
        self.e.end_group();
    }

    /// Runs `code` without drawing it, to measure it: what it draws stays out of the picture, which
    /// does not grow, and the nodes and coordinates it names, its named paths and what changes in
    /// its group are forgotten after. `read` is called when `code` has run, before that, to read
    /// them (`cx.point("probe.east")`). Returns what `read` returned and the box of what `code` drew
    /// on the canvas (its lower left and upper right corners, in cm), `None` if it drew nothing.
    /// Global definitions (`\gdef`) stay.
    ///
    /// ```
    /// use tikra::extension::{Context, Library, Tokens};
    ///
    /// // `from west={code}`: shifts the picture for `code` to start where its node `w` is
    /// fn from_west(cx: &mut Context, value: Option<Tokens>) -> Result<(), tikra::Error> {
    ///     let (west, _) = cx.measure(value.unwrap_or_default(), |cx| cx.point("w.west"))?;
    ///     let west = west?;
    ///     cx.keys(Tokens::text(&format!("shift={{({}cm,{}cm)}}", -west.x, -west.y)))
    /// }
    ///
    /// static LIB: Library = Library { name: "west", definitions: String::new, keys: &[("/tikz/from west", from_west)] };
    /// let code = r"\begin{tikzpicture} \path[from west={\node (w) at (2,0) {x};}]; \end{tikzpicture}";
    /// assert!(tikra::convert_with_libraries(code, &tikra::Options::default(), &[&LIB]).unwrap().warnings.is_empty());
    /// ```
    pub fn measure<T>(&mut self, code: Tokens, read: impl FnOnce(&mut Context) -> T) -> Result<(T, Option<(Point, Point)>), Error> {
        self.e.run_unseen(code.0, |e| read(&mut Context { e }))
    }

    /// The labels and pins that `options` give a node, without drawing them.
    pub fn labels_of(&mut self, options: Tokens) -> Result<Vec<Label>, Error> {
        self.e.begin_group();
        let saved = std::mem::take(&mut self.e.st.node.labels);
        let r = self.e.process_keys(options.0, Some("/tikz"));
        let labels = std::mem::replace(&mut self.e.st.node.labels, saved);
        self.e.end_group();
        r?;
        Ok(labels.into_iter().map(|(spec, pin)| Label { spec, pin }).collect())
    }

    /// Draws a label on the last node drawn, after the keys `setup` (`label distance=2pt`). Nothing
    /// without a node.
    pub fn draw_label_on_last_node(&mut self, label: &Label, setup: Tokens) -> Result<(), Error> {
        let Some(node) = self.e.last_node.clone() else { return Ok(()) };
        self.e.begin_group();
        let r = self.e.process_keys(setup.0, Some("/tikz")).and_then(|_| self.e.label(&node, label.spec.clone(), label.pin));
        self.e.end_group();
        self.e.canvas.items.extend(r?);
        Ok(())
    }

    /// A warning in the conversion's output: what was ignored or approximated.
    pub fn warn(&mut self, message: impl Into<String>) {
        self.e.warn(message);
    }

    /// Declares the node shape `name`, drawn by `shape`: `node[shape=name]`, or a style that sets
    /// it, makes a node of that shape, for the rest of the conversion.
    pub fn declare_shape(&mut self, name: &str, shape: ShapeFn) {
        self.e.extension_shapes.insert(name.trim().to_string(), shape);
    }
}
