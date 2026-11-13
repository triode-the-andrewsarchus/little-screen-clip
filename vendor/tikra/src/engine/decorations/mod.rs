//! Decorations, as pgf's decoration module and libraries do them, and TikZ's glue
//! (`tikzlibrarydecorations.code.tex`): the `decorate` option and path operation, the
//! `/pgf/decoration/…` keys, `pre` and `post`; and `preaction` and `postaction`, which use the
//! path again with other options (`tikz.code.tex`, `\tikz@finish`).

pub mod feet;
pub mod library;
pub mod textfx;
pub mod walker;

use std::rc::Rc;

use crate::canvas::{transform_segments, Item};
use crate::keys;
use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Cat, Meaning, Token};
use crate::{Error, PT};

use super::state::{GState, Mode};
use super::{tikra_tokens, Engine};
use library::Run;
use walker::Walker;

/// A position of a mark: an expression, or a length already computed (pt).
#[derive(Debug, Clone, PartialEq)]
pub enum Pos {
    Tokens(Vec<Token>),
    Pt(f64),
}

/// `mark=at position … with {…}` and `mark=between positions … and … step … with {…}`.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    At { pos: Pos, action: Vec<Token> },
    Between { start: Pos, end: Pos, step: Pos, action: Vec<Token> },
}

/// The decoration settings kept in the graphic state (the `/pgf/decoration/…` keys that pgf keeps
/// in registers and macros; those it keeps with `.initial` stay pgfkeys values). Lengths in pt.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `\tikz@decoration@name`; None is `none`
    pub name: Option<String>,
    pub amplitude: f64,
    pub segment_length: f64,
    pub angle: f64,
    pub aspect: f64,
    pub meta_amplitude: f64,
    pub meta_segment_length: f64,
    pub raise: Option<f64>,
    pub mirror: bool,
    pub user_transform: Option<Transform>,
    pub has_corners: bool,
    pub reverse: bool,
    pub marks: Rc<Vec<Mark>>,
    pub connection_node: String,
    /// `moveto code`, `lineto code`, `curveto code`, `closepath code`
    pub segment_code: [Option<Vec<Token>>; 4],
    pub text: Vec<Token>,
    pub text_color: String,
    pub text_format: (char, Option<char>),
    pub text_fit: bool,
    pub text_stretch: bool,
    pub shape_sloped: bool,
    pub shape_scaled: bool,
    pub shape_sep: String,
    pub shape_spread: String,
    pub te: textfx::Settings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: None,
            amplitude: 2.5,
            segment_length: 10.0,
            angle: 45.0,
            aspect: 0.5,
            meta_amplitude: 2.5,
            meta_segment_length: 1.0 / PT,
            raise: None,
            mirror: false,
            user_transform: None,
            has_corners: false,
            reverse: false,
            marks: Rc::new(Vec::new()),
            connection_node: String::new(),
            segment_code: [None, None, None, None],
            text: Vec::new(),
            text_color: "black".into(),
            text_format: ('|', None),
            text_fit: false,
            text_stretch: false,
            shape_sloped: true,
            shape_scaled: false,
            shape_sep: ".25cm, between centers".into(),
            shape_spread: String::new(),
            te: Default::default(),
        }
    }
}

impl Settings {
    /// `\tikz@dec@user@transform\tikz@dec@mirror\tikz@dec@shift`, in pt.
    pub fn extra_transform(&self) -> Transform {
        let mut t = self.user_transform.unwrap_or_default();
        if self.mirror {
            t = t.compose(&Transform::scaling(1.0, -1.0));
        }
        if let Some(r) = self.raise {
            t = t.compose(&Transform::translation(0.0, r));
        }
        t
    }
}

/// How a path is decorated.
enum How {
    /// TikZ's `decorate`, with `pre` and `post`
    Tikz(String),
    /// `\pgfdecoration`: `{name}{length}{before code}{after code}, …`
    List(Vec<Token>),
    /// `\pgfmetadecoration`
    Meta(String),
}

/// An open `\pgfdecoration`: its list, and the path there was before.
#[derive(Debug, Clone)]
pub struct Env {
    list: Vec<Token>,
    meta: bool,
    existing: super::basic::PgfPath,
}

/// Items of a list at top-level commas.
fn split_commas(v: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0usize;
    for t in v {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(',', _) if depth == 0 => {
                out.push(Vec::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().expect("one").push(t.clone());
    }
    out.retain(|i| !keys::trim(i).is_empty());
    out
}

/// Dimension registers that decoration code can read (`\pgfdecoratedpathlength`…).
const REGISTERS: &[(&str, u32)] = &[
    ("pgfdecoratedpathlength", 4010),
    ("pgfdecoratedcompleteddistance", 4011),
    ("pgfdecoratedremainingdistance", 4012),
    ("pgfdecoratedinputsegmentlength", 4013),
    ("pgfdecoratedinputsegmentcompleteddistance", 4014),
    ("pgfdecoratedinputsegmentremainingdistance", 4015),
    ("pgfdecorationsegmentlength", 4016),
    ("pgfdecorationsegmentamplitude", 4017),
];

fn sp(pt: f64) -> i64 {
    (pt * 65536.0).round() as i64
}

/// Sets the registers to where the walk is.
pub fn set_registers(e: &mut Engine, w: &Walker) {
    let values = [w.section, w.completed, w.remaining, w.seg.len, w.seg_completed, w.seg_remaining];
    for ((_, r), v) in REGISTERS.iter().zip(values) {
        e.tex.dimens.insert(*r, sp(v));
    }
    e.tex.define_string("pgfdecoratedangle", &format!("{}", w.angle), false);
}

macro_rules! keyfn {
    ($name:ident, |$e:ident, $v:ident| $body:expr) => {
        fn $name($e: &mut Engine, $v: Option<Vec<Token>>) -> Result<(), Error> {
            $body
        }
    };
}

fn text(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<String, Error> {
    Ok(match v {
        Some(v) => e.tex.expand_to_string(v.clone())?.trim().to_string(),
        None => String::new(),
    })
}

fn pt(e: &mut Engine, v: Option<Vec<Token>>) -> Result<Option<f64>, Error> {
    match v {
        Some(v) => Ok(Some(e.length_of(v)? / PT)),
        None => Ok(None),
    }
}

/// `true` (or no value), `false`.
fn boolean(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<bool, Error> {
    Ok(!matches!(text(e, v)?.as_str(), "false"))
}

pub fn current_name(e: &Engine) -> String {
    let name = keys::value(&e.tex, "pgfkeyscurrentname").or_else(|| library::macro_body(e, "pgfkeyscurrentname"));
    name.map(|v| tex::tokens_to_string(&v)).unwrap_or_default()
}

keyfn!(k_decorate, |e, v| {
    e.st.decorate = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_snake, |e, v| {
    let n = text(e, &v)?;
    e.st.snake = if n == "none" { None } else { Some(n) };
    Ok(())
});
keyfn!(k_decoration, |e, v| match v {
    Some(v) => e.process_keys(v, Some("/pgf/decoration")),
    None => Ok(()),
});
keyfn!(k_unknown, |e, v| {
    // a decoration's name, else an unknown key
    let name = current_name(e);
    if v.is_none() && (library::resolve(e, &name).is_some() || library::resolve_meta(e, &name).is_some()) {
        e.st.decoration.name = Some(name);
    } else {
        e.warn(format!("option '{name}' not supported: ignored"));
    }
    Ok(())
});
keyfn!(k_name, |e, v| {
    let n = text(e, &v)?;
    e.st.decoration.name = if n == "none" { None } else { Some(n) };
    Ok(())
});
keyfn!(k_amplitude, |e, v| {
    if let Some(x) = pt(e, v)? {
        e.st.decoration.amplitude = x;
    }
    Ok(())
});
keyfn!(k_segment_length, |e, v| {
    if let Some(x) = pt(e, v)? {
        e.st.decoration.segment_length = x;
    }
    Ok(())
});
keyfn!(k_meta_amplitude, |e, v| {
    if let Some(x) = pt(e, v)? {
        e.st.decoration.meta_amplitude = x;
    }
    Ok(())
});
keyfn!(k_meta_segment_length, |e, v| {
    if let Some(x) = pt(e, v)? {
        e.st.decoration.meta_segment_length = x;
    }
    Ok(())
});
keyfn!(k_angle, |e, v| {
    if let Some(v) = v {
        e.st.decoration.angle = e.number_of(v)?;
    }
    Ok(())
});
keyfn!(k_aspect, |e, v| {
    if let Some(v) = v {
        e.st.decoration.aspect = e.number_of(v)?;
        // the number, for decoration code to read
        e.tex.define_string("pgfdecorationsegmentaspect", &super::pgf_number(e.st.decoration.aspect), false);
    }
    Ok(())
});
keyfn!(k_raise, |e, v| {
    e.st.decoration.raise = pt(e, v)?;
    Ok(())
});
keyfn!(k_mirror, |e, v| {
    e.st.decoration.mirror = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_transform, |e, v| {
    // the transformations of TikZ options, from the identity
    let saved = e.st.pending;
    e.st.pending = Transform::default();
    let r = e.process_keys(v.unwrap_or_default(), Some("/tikz"));
    let t = e.st.pending;
    e.st.pending = saved;
    r?;
    e.st.decoration.user_transform = Some(Transform { e: t.e / PT, f: t.f / PT, ..t });
    Ok(())
});
keyfn!(k_corners, |e, v| {
    e.st.decoration.has_corners = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_reverse, |e, v| {
    e.st.decoration.reverse = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_reset_marks, |e, _v| {
    e.st.decoration.marks = Rc::new(Vec::new());
    Ok(())
});
keyfn!(k_mark, |e, v| {
    let v = v.unwrap_or_default();
    match parse_mark(&v) {
        Some(m) => Rc::make_mut(&mut e.st.decoration.marks).push(m),
        None => e.warn(format!("mark '{}' not understood: ignored", tex::tokens_to_string(&v).trim())),
    }
    Ok(())
});
keyfn!(k_connection_node, |e, v| {
    e.st.decoration.connection_node = text(e, &v)?;
    Ok(())
});
keyfn!(k_moveto_code, |e, v| {
    e.st.decoration.segment_code[0] = v;
    Ok(())
});
keyfn!(k_lineto_code, |e, v| {
    e.st.decoration.segment_code[1] = v;
    Ok(())
});
keyfn!(k_curveto_code, |e, v| {
    e.st.decoration.segment_code[2] = v;
    Ok(())
});
keyfn!(k_closepath_code, |e, v| {
    e.st.decoration.segment_code[3] = v;
    Ok(())
});
keyfn!(k_text, |e, v| {
    e.st.decoration.text = v.unwrap_or_default();
    Ok(())
});
keyfn!(k_text_color, |e, v| {
    e.st.decoration.text_color = text(e, &v)?;
    Ok(())
});
keyfn!(k_text_format_delimiters, |e, v| {
    let v = v.unwrap_or_default();
    let parts = super::options::split_arguments(&v, 2);
    let first = |p: &Vec<Token>| tex::tokens_to_string(p).trim().chars().next();
    if let Some(open) = first(&parts[0]) {
        e.st.decoration.text_format = (open, first(&parts[1]));
    }
    Ok(())
});
keyfn!(k_fit, |e, v| {
    e.st.decoration.text_fit = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_fit_spaces, |e, v| {
    let b = boolean(e, &v)?;
    e.st.decoration.text_fit = b;
    e.st.decoration.text_stretch = b;
    Ok(())
});
keyfn!(k_shape_sloped, |e, v| {
    e.st.decoration.shape_sloped = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_shape_scaled, |e, v| {
    e.st.decoration.shape_scaled = boolean(e, &v)?;
    Ok(())
});
keyfn!(k_shape_sep, |e, v| {
    e.st.decoration.shape_sep = text(e, &v)?;
    Ok(())
});
keyfn!(k_shape_spread, |e, v| {
    e.st.decoration.shape_spread = text(e, &v)?;
    Ok(())
});
keyfn!(k_preaction, |e, v| {
    Rc::make_mut(&mut e.st.preactions).push(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_postaction, |e, v| {
    Rc::make_mut(&mut e.st.postactions).push(v.unwrap_or_default());
    Ok(())
});

/// The keys done natively.
pub const KEYS: &[(&str, super::options::KeyFn)] = &[
    ("/tikz/decorate", k_decorate),
    ("/tikz/snake", k_snake),
    ("/tikz/preaction", k_preaction),
    ("/tikz/postaction", k_postaction),
    ("/pgf/decoration", k_decoration),
    ("/pgf/decoration/.unknown", k_unknown),
    ("/pgf/decoration/name", k_name),
    ("/pgf/decoration/amplitude", k_amplitude),
    ("/pgf/decoration/segment length", k_segment_length),
    ("/pgf/decoration/meta-amplitude", k_meta_amplitude),
    ("/pgf/decoration/meta-segment length", k_meta_segment_length),
    ("/pgf/decoration/angle", k_angle),
    ("/pgf/decoration/aspect", k_aspect),
    ("/pgf/decoration/raise", k_raise),
    ("/pgf/decoration/mirror", k_mirror),
    ("/pgf/decoration/transform", k_transform),
    ("/pgf/decoration/path has corners", k_corners),
    ("/pgf/decoration/reverse path", k_reverse),
    ("/pgf/decoration/reset marks", k_reset_marks),
    ("/pgf/decoration/mark", k_mark),
    ("/pgf/decoration/mark connection node", k_connection_node),
    ("/pgf/decoration/moveto code", k_moveto_code),
    ("/pgf/decoration/lineto code", k_lineto_code),
    ("/pgf/decoration/curveto code", k_curveto_code),
    ("/pgf/decoration/closepath code", k_closepath_code),
    ("/pgf/decoration/text", k_text),
    ("/pgf/decoration/text color", k_text_color),
    ("/pgf/decoration/text format delimiters", k_text_format_delimiters),
    ("/pgf/decoration/text align/fit to path", k_fit),
    ("/pgf/decoration/text align/fit to path stretching spaces", k_fit_spaces),
    ("/pgf/decoration/shape sloped", k_shape_sloped),
    ("/pgf/decoration/shape scaled", k_shape_scaled),
    ("/pgf/decoration/shape sep", k_shape_sep),
    ("/pgf/decoration/shape evenly spread", k_shape_spread),
    ("/tikz/text effects", textfx::k_text_effects),
    ("/pgf/decoration/text effects/.unknown", textfx::key_unknown),
    ("/pgf/decoration/text effects/path from text", textfx::k_path_from_text),
    ("/pgf/decoration/text effects/path from text angle", textfx::k_path_from_text_angle),
    ("/pgf/decoration/text effects/fit text to path", textfx::k_fit),
    ("/pgf/decoration/text effects/scale text to path", textfx::k_scale),
    ("/pgf/decoration/text effects/segment from width", textfx::k_segment_from_width),
    ("/pgf/decoration/text effects/word separator", textfx::k_word_separator),
    ("/pgf/decoration/text effects/character command", textfx::k_character_command),
    ("/pgf/decoration/text effects/reverse text", textfx::k_reverse),
    ("/pgf/decoration/text effects/group letters into words", textfx::k_group),
    ("/pgf/decoration/text effects/group letters", textfx::k_group),
    ("/pgf/decoration/text effects/repeat text", textfx::k_repeat),
    ("/pgf/decoration/text effects/character count", textfx::k_character_count),
    ("/pgf/decoration/text effects/character total", textfx::k_character_total),
    ("/pgf/decoration/text effects/letter count", textfx::k_letter_count),
    ("/pgf/decoration/text effects/letter total", textfx::k_letter_total),
    ("/pgf/decoration/text effects/word count", textfx::k_word_count),
    ("/pgf/decoration/text effects/word total", textfx::k_word_total),
    ("/pgf/decoration/text effects/style characters", textfx::k_style_characters),
    ("/pgf/decoration/text effects/replace characters", textfx::k_replace_characters),
    ("/pgf/decoration/text effects/characters/.append", textfx::k_characters_append),
    ("/pgf/decoration/text effects/character widths/.append", textfx::k_widths_append),
];

/// The keys pgf defines as values and styles (`.initial`, `.style`), from its own definitions.
pub const DEFINITIONS: &str = r"
\def\pgfdecorationsegmentaspect{0.5}
\pgfkeys{/pgf/decoration/.cd,
  pre/.initial=lineto, post/.initial=lineto, pre length/.initial=0pt, post length/.initial=0pt,
  start radius/.initial=2.5pt, end radius/.initial=2.5pt, radius/.style={/pgf/decoration/start radius={#1},/pgf/decoration/end radius={#1}},
  mirror/.default=true, path has corners/.default=true, reverse path/.default=true, shape sloped/.default=true,
  shape scaled/.default=true,
  shape/.initial=circle, anchor/.initial=center,
  shape start width/.initial=2.5pt, shape start height/.initial=2.5pt,
  shape end width/.initial=2.5pt, shape end height/.initial=2.5pt,
  shape start size/.style={/pgf/decoration/shape start width={#1},/pgf/decoration/shape start height={#1}},
  shape end size/.style={/pgf/decoration/shape end width={#1},/pgf/decoration/shape end height={#1}},
  shape size/.style={/pgf/decoration/shape start size={#1},/pgf/decoration/shape end size={#1}},
  shape width/.style={/pgf/decoration/shape start width={#1},/pgf/decoration/shape end width={#1}},
  shape height/.style={/pgf/decoration/shape start height={#1},/pgf/decoration/shape end height={#1}},
  foot length/.initial=10pt, stride length/.initial=30pt, foot sep/.initial=4pt, foot angle/.initial=10,
  foot of/.initial=human,
  mark info/sequence number/.initial=0, mark info/distance from start/.initial=,
  text align/.code={\pgfqkeys{/pgf/decoration/text align}{#1}},
  text align/align/.initial=left,
  text align/left/.style={/pgf/decoration/text align/align=left},
  text align/right/.style={/pgf/decoration/text align/align=right},
  text align/center/.style={/pgf/decoration/text align/align=center},
  text align/left indent/.initial=0pt, text align/right indent/.initial=0pt,
  text align/fit to path/.default=true, text align/fit to path stretching spaces/.default=true,
}
% the older names of the decoration keys, under /pgf
\pgfkeys{/pgf/.cd,
  segment length/.style={/pgf/decoration/segment length={#1}}, segment angle/.style={/pgf/decoration/angle={#1}},
  segment aspect/.style={/pgf/decoration/aspect={#1}},
  segment amplitude/.style={/pgf/decoration/amplitude={#1},/pgf/decoration/shape height={2*(#1)}},
  segment object length/.style={/pgf/decoration/shape width={#1},/pgf/decoration/radius={#1}},
}
% the snakes library's keys, as the decoration keys they stand for
\tikzset{
  line before snake/.style={/pgf/decoration/pre=lineto,/pgf/decoration/pre length={#1}},
  line after snake/.style={/pgf/decoration/post=lineto,/pgf/decoration/post length={#1}},
  gap before snake/.style={/pgf/decoration/pre=moveto,/pgf/decoration/pre length={#1}},
  gap after snake/.style={/pgf/decoration/post=moveto,/pgf/decoration/post length={#1}},
  gap around snake/.style={gap before snake={#1},gap after snake={#1}},
  raise snake/.style={/pgf/decoration/raise={#1}}, mirror snake/.style={/pgf/decoration/mirror={#1}},
  mirror snake/.default=true,
}
";

/// Defines the registers.
pub fn define(e: &mut Engine) {
    for (name, r) in REGISTERS {
        e.tex.define(name, Meaning::Dimen(*r));
    }
}

/// The decorations of the document, stored as pgf stores them, and the basic layer's
/// decoration environments.
pub const DECLARATIONS: &str = r"
\long\def\pgfdeclaredecoration#1#2#3{\def\pgf@decorate@name{#1}\expandafter\def\csname pgf@decorate@@#1@initial\endcsname{#2}\let\state\pgf@decorate@state#3\let\state\relax}
\def\pgf@decorate@state#1{\pgfutil@ifnextchar[{\pgf@decorate@@start{#1}}{\pgf@decorate@@start{#1}[]}}
\long\def\pgf@decorate@@start#1[#2]#3{\expandafter\def\csname pgf@decorate@@\pgf@decorate@name @#1@options\endcsname{#2}\expandafter\def\csname pgf@decorate@@\pgf@decorate@name @#1@code\endcsname{#3}}
\long\def\pgfdeclaremetadecoration#1#2#3{\def\pgf@metadecoration@name{#1}\expandafter\def\csname pgf@metadecoration@@#1@initial\endcsname{#2}\let\state\pgf@metadecoration@state#3\let\state\relax}
\def\pgf@metadecoration@state#1{\pgfutil@ifnextchar[{\pgf@metadecoration@@state{#1}}{\pgf@metadecoration@@state{#1}[]}}
\long\def\pgf@metadecoration@@state#1[#2]#3{\expandafter\def\csname pgf@metadecoration@@\pgf@metadecoration@name @#1@options\endcsname{#2}\expandafter\def\csname pgf@metadecoration@@\pgf@metadecoration@name @#1@code\endcsname{#3}}
\def\pgfsetdecorationsegmenttransformation#1{\def\pgf@decorate@additionaltransform{#1}}
\def\pgf@decorate@additionaltransform{}
\def\pgfdecoration#1{\tikra@decoration{#1}{0}}
\def\endpgfdecoration{\tikra@enddecoration}
\def\pgfmetadecoration#1{\tikra@decoration{#1}{1}}
\def\endpgfmetadecoration{\tikra@enddecoration}
\let\startpgfdecoration\pgfdecoration \let\stoppgfdecoration\endpgfdecoration
\let\startpgfmetadecoration\pgfmetadecoration \let\stoppgfmetadecoration\endpgfmetadecoration
\newenvironment{pgfdecoration}[1]{\tikra@decoration{#1}{0}}{\tikra@enddecoration}
\newenvironment{pgfmetadecoration}[1]{\tikra@decoration{#1}{1}}{\tikra@enddecoration}
\long\def\pgfdecoratepath#1#2{\pgfdecoration{{#1}{\pgfdecoratedpathlength}}#2\endpgfdecoration}
\def\pgfdecoratecurrentpath{\tikra@decoratecurrentpath}
";

/// Splits tokens at the first `word` (letters) outside braces.
pub fn split_word(v: &[Token], word: &str) -> Option<(Vec<Token>, Vec<Token>)> {
    let w: Vec<char> = word.chars().collect();
    let mut depth = 0usize;
    for i in 0..v.len() {
        match v[i] {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && i + w.len() <= v.len() && w.iter().enumerate().all(|(k, c)| matches!(v[i + k], Token::Char(d, _) if d == *c)) {
            return Some((v[..i].to_vec(), v[i + w.len()..].to_vec()));
        }
    }
    None
}

fn unbrace(v: Vec<Token>) -> Vec<Token> {
    tex::strip_outer_braces(keys::trim(&v).to_vec())
}

/// `at position #1 with #2`, `between positions #1 and #2 step #3 with #4`.
fn parse_mark(v: &[Token]) -> Option<Mark> {
    let v = keys::trim(v).to_vec();
    if let Some((_, rest)) = split_word(&v, "between positions") {
        let (start, rest) = split_word(&rest, "and")?;
        let (end, rest) = split_word(&rest, "step")?;
        let (step, action) = split_word(&rest, "with")?;
        return Some(Mark::Between { start: Pos::Tokens(unbrace(start)), end: Pos::Tokens(unbrace(end)), step: Pos::Tokens(unbrace(step)), action: unbrace(action) });
    }
    let (_, rest) = split_word(&v, "at position")?;
    let (pos, action) = split_word(&rest, "with")?;
    Some(Mark::At { pos: Pos::Tokens(unbrace(pos)), action: unbrace(action) })
}

/// The graphic settings that TikZ installs when a path is used (`\tikz@options`), not when its
/// options are read: decorations and preactions happen before, with the scope's.
fn take_graphics(st: &mut GState, from: &GState) {
    if st.text_color.is_none() && st.color != from.color {
        // `color=` sets the text color at once
        st.text_color = Some(st.color);
    }
    st.color = from.color;
    st.draw_color = from.draw_color;
    st.fill_color = from.fill_color;
    st.pattern_color = from.pattern_color;
    st.line_width = from.line_width;
    st.dash = from.dash.clone();
    st.cap = from.cap;
    st.join = from.join;
    st.miter_limit = from.miter_limit;
    st.draw_opacity = from.draw_opacity;
    st.fill_opacity = from.fill_opacity;
    st.even_odd = from.even_odd;
}

impl Engine {
    /// `\tikz@finish`: decorates the path, runs the preactions, uses the path, runs the
    /// postactions. `outer` is the state before the path's options.
    pub fn finish_path(&mut self, segments: Vec<Segment>, outer: &GState, command: &str) -> Result<(), Error> {
        let mut segs = segments;
        let mut b = crate::canvas::Canvas::default();
        b.include_path(&segs);
        self.st.path_box = b.min.zip(b.max);
        if self.st.decorate && !segs.is_empty() && segs.iter().all(super::path::valid_segment) {
            // pgf counts a path in the picture's size as it is built, before it is decorated
            if self.counts_for_size() {
                self.canvas.include_path(&segs);
            }
            self.begin_group();
            take_graphics(&mut self.st, outer);
            let r = self.decorate_segments(&segs, None);
            self.end_group();
            segs = r?;
        }
        // a name the path still has (a pre- or postaction's `name path`) is the decorated path's
        if let Some(name) = self.st.name_path.take() {
            self.named_paths.insert(name, segs.clone());
        }
        let pre = std::mem::take(&mut self.st.preactions);
        let post = std::mem::take(&mut self.st.postactions);
        for a in pre.iter() {
            self.action(&segs, a.clone(), Some(outer), command)?;
        }
        match self.st.mode.picture.take() {
            Some(code) => self.path_picture(&segs, code, command)?,
            None => self.use_path(segs.clone(), command)?,
        }
        for a in post.iter() {
            self.action(&segs, a.clone(), None, command)?;
        }
        Ok(())
    }

    /// A path with a `path picture`, in `\tikz@finish`'s order: the fill, the picture clipped to the
    /// path (with the node `path picture bounding box` around it), then the stroke.
    fn path_picture(&mut self, segs: &[Segment], code: Vec<Token>, command: &str) -> Result<(), Error> {
        let mode = self.st.mode.clone();
        if mode.fill || mode.shade || mode.pattern {
            self.st.mode = Mode { draw: false, clip: false, ..mode.clone() };
            self.use_path(segs.to_vec(), command)?;
        }
        let mut c = crate::canvas::Canvas::default();
        c.include_path(segs);
        if let (Some(lo), Some(hi)) = (c.min, c.max) {
            self.begin_group();
            self.begin_scope();
            let r = (|| {
                self.open_in_scope(crate::canvas::Item::Clip { segments: segs.to_vec(), even_odd: self.st.even_odd });
                if let Some(s) = self.scopes.last_mut() {
                    s.clipped = true;
                }
                self.st.mode = Mode::default();
                let at = lo.lerp(hi, 0.5);
                let size = format!("shape=rectangle,inner sep=0pt,outer sep=0pt,minimum width={}pt,minimum height={}pt", (hi.x - lo.x) / crate::PT, (hi.y - lo.y) / crate::PT);
                let items = self.node(super::nodes::NodeSpec {
                    options: tikra_tokens(&size),
                    name: Some("path picture bounding box".into()),
                    at: Some(at),
                    text: Vec::new(),
                    place: super::nodes::OnPath::Point(at),
                    point: at,
                    coordinate: false,
                    default_pos: None,
                })?;
                self.canvas.items.extend(items);
                self.run_tokens(code)
            })();
            self.end_scope();
            self.end_group();
            r?;
        }
        if mode.draw || mode.clip {
            self.st.mode = Mode { fill: false, shade: false, pattern: false, ..mode };
            self.use_path(segs.to_vec(), command)?;
        }
        Ok(())
    }

    /// `\tikz@extra@preaction`, `\tikz@extra@postaction`: `\path[options]` on the same path.
    fn action(&mut self, segs: &[Segment], options: Vec<Token>, graphics: Option<&GState>, command: &str) -> Result<(), Error> {
        self.begin_group();
        self.begin_scope();
        let r = (|| {
            if let Some(g) = graphics {
                // the path's own colors stay known as `\tikz@fillcolor` and `\tikz@strokecolor` (the shadows library copies them)
                let (fill, stroke) = (self.st.mode.fill.then(|| self.st.fill_paint_color()), self.st.mode.draw.then(|| self.st.stroke_color()));
                for (name, c) in [("fillcolor", fill), ("strokecolor", stroke)] {
                    match c {
                        Some(c) => {
                            self.colors.insert(format!("tikra@{name}"), c);
                            self.tex.define_string(&format!("tikz@{name}"), &format!("tikra@{name}"), false);
                        }
                        None => self.tex.define_string(&format!("tikz@{name}"), "none", false),
                    }
                }
                take_graphics(&mut self.st, g);
            }
            let outer = self.st.clone();
            self.st.mode = Mode::default();
            self.st.decorate = false;
            self.st.preactions = Rc::default();
            self.st.postactions = Rc::default();
            self.st.pending = Transform::default();
            self.process_keys(tikra_tokens("every path/.try"), Some("/tikz"))?;
            self.process_keys(options, Some("/tikz"))?;
            self.finish_path(segs.to_vec(), &outer, command)
        })();
        self.end_scope();
        self.end_group();
        r
    }

    /// Decorates segments (cm) with the current decoration (`\tikz@lib@dec@decorate@path` and
    /// `\pgfdecoration`). `existing` is the point before them, for a path that does not start
    /// with a moveto.
    pub fn decorate_segments(&mut self, segs: &[Segment], existing: Option<Point>) -> Result<Vec<Segment>, Error> {
        let Some(name) = self.st.decoration.name.clone() else { return Ok(segs.to_vec()) };
        // pgfplots' `soft clip`: the path cut to `soft clip path`
        if name.trim() == "soft clip" {
            let spec = keys::value(&self.tex, "/pgf/decoration/soft clip path").map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
            return Ok(match crate::engine::pgfplots::fillbetween::soft_clip_region(self, &spec) {
                Some(r) => {
                    // it starts where the path started
                    let start = segs.iter().find_map(|s| if let Segment::MoveTo(p) = s { Some(*p) } else { None });
                    start.map(Segment::MoveTo).into_iter().chain(crate::engine::pgfplots::fillbetween::clip_lines(segs, r)).collect()
                }
                None => {
                    self.warn(format!("soft clip path '{}' not understood: path drawn unclipped", spec.trim()));
                    segs.to_vec()
                }
            });
        }
        if library::resolve_meta(self, &name).is_none() && library::resolve(self, &name).is_none() {
            self.warn(format!("decoration '{name}' not supported: path drawn undecorated"));
            return Ok(segs.to_vec());
        }
        if self.st.rounded != 0.0 || self.st.rounded_out != 0.0 {
            // the round corners of the path are made before it is decorated (`\pgfprocessround`)
            let rounded = super::output::round_corners(segs, self.st.rounded, self.st.rounded_out);
            return self.decorate_how(&rounded, existing, How::Tikz(name));
        }
        self.decorate_how(segs, existing, How::Tikz(name))
    }

    /// Decorates segments (cm): `\pgfdecoration` with a list, `\pgfmetadecoration`, or TikZ's way.
    fn decorate_how(&mut self, segs: &[Segment], existing: Option<Point>, how: How) -> Result<Vec<Segment>, Error> {
        let existing = existing.unwrap_or_default().scale(1.0 / PT);
        let mut pts = transform_segments(segs, &Transform::scaling(1.0 / PT, 1.0 / PT));
        // `path from text`: a lone moveto becomes a line as long as the text
        let mut prepared = None;
        if let (How::Tikz(name), [Segment::MoveTo(q)]) = (&how, pts.as_slice()) {
            if name.trim() == "text effects along path" && self.st.decoration.te.path_from_text {
                let q = *q;
                let probe = Walker::new(walker::parse(&[Segment::MoveTo(q), Segment::LineTo(Point::new(q.x + 1.0, q.y))], q), q);
                let mut run = Run::new(self, probe, self.st.decoration.clone());
                run.te_prepare()?;
                let width: f64 = run.te.chars.iter().map(|c| c.pre + c.post).sum();
                let te = std::mem::take(&mut run.te);
                let a = self.st.decoration.te.path_from_text_angle.to_radians();
                pts.push(Segment::LineTo(Point::new(q.x + width * a.cos(), q.y + width * a.sin())));
                prepared = Some(te);
            }
        }
        // a lone moveto becomes a tiny line
        if let [Segment::MoveTo(q)] = pts.as_slice() {
            let q = *q;
            pts.push(Segment::LineTo(Point::new(q.x + 0.0001, q.y)));
        }
        if !pts.iter().any(|s| !matches!(s, Segment::MoveTo(_))) {
            return Ok(Vec::new());
        }
        let mut parsed = walker::parse(&pts, existing);
        self.process_keys(tikra_tokens("/pgf/every decoration/.try"), Some("/pgf/decoration"))?;
        let s = self.st.decoration.clone();
        if s.reverse {
            parsed.objs = walker::reverse(&parsed.objs);
            // the reversed path starts where the path ended and ends where it started
            if let Some(&walker::Obj::MoveTo(p)) = parsed.objs.first() {
                parsed.first = p;
            }
            if let Some(&walker::Obj::Line { b, .. } | &walker::Obj::Curve { b, .. } | &walker::Obj::Close { b, .. }) =
                parsed.objs.iter().rev().find(|o| !matches!(o, walker::Obj::End | walker::Obj::MoveTo(_)))
            {
                parsed.last_nonmove = b;
            }
        }
        let w = Walker::new(parsed, existing);
        let mut run = Run::new(self, w, s);
        if let Some(te) = prepared {
            run.te = te;
        }
        match how {
            How::Tikz(name) => {
                let pre = run.key_text("pre length");
                let post = run.key_text("post length");
                if let Some(m) = library::resolve_meta(run.e, &name) {
                    run.run_meta(m, None)?;
                } else {
                    let dec = library::resolve(run.e, &name).unwrap_or(library::Dec::LineTo);
                    if pre != "0pt" || post != "0pt" {
                        run.run_meta(library::Meta::Internal, Some(dec))?;
                    } else {
                        let total = run.w.total;
                        run.invoke(dec, total)?;
                    }
                }
            }
            How::Meta(name) => match library::resolve_meta(run.e, &name) {
                Some(m) => run.run_meta(m, None)?,
                None => run.e.warn(format!("meta-decoration '{}' not supported: path left out", name.trim())),
            },
            How::List(list) => {
                for item in split_commas(&list) {
                    let a = super::options::split_arguments(&item, 4);
                    let name = run.e.tex.expand_to_string(a[0].clone())?;
                    let (total, rem) = (run.w.total, run.w.rem_total);
                    run.e.tex.dimens.insert(4010, sp(total));
                    run.e.tex.dimens.insert(4012, sp(rem));
                    let dist = run.e.length_of(a[1].clone())? / PT;
                    let code = |v: &Vec<Token>| if keys::trim(v).is_empty() { None } else { Some(v.clone()) };
                    match library::resolve(run.e, &name) {
                        Some(dec) => run.invoke_with(dec, dist, code(&a[2]), code(&a[3]))?,
                        None => run.e.warn(format!("decoration '{}' not supported: left out", name.trim())),
                    }
                }
            }
        }
        let out = std::mem::take(&mut run.w.out.segs);
        Ok(transform_segments(&out, &Transform::scaling(PT, PT)))
    }

    /// `\pgfdecoration{list}` and `\pgfmetadecoration{name}` (`meta`): the path built until the end
    /// is decorated, then added to the path there was.
    pub fn decoration_begin(&mut self) -> Result<(), Error> {
        let list = self.argument()?;
        let meta = self.argument_string()? == "1";
        let existing = std::mem::take(&mut self.pgf_path);
        self.pgf_path = super::basic::PgfPath { segments: Vec::new(), last: existing.last, start: existing.start, quick: false };
        self.decoration_envs.push(Env { list, meta, existing });
        self.begin_group();
        Ok(())
    }

    pub fn decoration_end(&mut self) -> Result<(), Error> {
        self.end_group();
        let Some(env) = self.decoration_envs.pop() else {
            self.warn("\\endpgfdecoration without \\pgfdecoration: ignored");
            return Ok(());
        };
        let decorated = std::mem::take(&mut self.pgf_path.segments);
        let last = env.existing.last;
        self.pgf_path = env.existing;
        if decorated.is_empty() {
            self.warn("I cannot decorate an empty path");
            return Ok(());
        }
        let how = if env.meta { How::Meta(self.tex.expand_to_string(env.list)?) } else { How::List(env.list) };
        self.begin_group();
        let r = self.decorate_how(&decorated, last, how);
        self.end_group();
        self.add_to_pgf_path(r?);
        Ok(())
    }

    /// `\pgfdecoratecurrentpath{name}`: the basic layer's path, decorated.
    pub fn decorate_current_path(&mut self) -> Result<(), Error> {
        let name = self.argument()?;
        let path = std::mem::take(&mut self.pgf_path);
        let mut list = vec![Token::Char('{', Cat::Begin)];
        list.extend(name);
        list.push(Token::Char('}', Cat::End));
        list.extend(tikra_tokens("{\\pgfdecoratedpathlength}"));
        self.begin_group();
        let r = self.decorate_how(&path.segments, None, How::List(list));
        self.end_group();
        self.add_to_pgf_path(r?);
        Ok(())
    }

    fn add_to_pgf_path(&mut self, segs: Vec<Segment>) {
        let path = &mut self.pgf_path;
        for s in segs {
            match s {
                Segment::MoveTo(q) => {
                    path.start = Some(q);
                    path.last = Some(q);
                }
                Segment::LineTo(q) | Segment::CurveTo(_, _, q) => path.last = Some(q),
                Segment::Close => path.last = path.start,
            }
            path.segments.push(s);
        }
    }

    /// `\arrow[options]{tip}` and `\arrowreversed` in a mark: the tip at the origin, pointing right.
    pub fn arrow_command(&mut self, reversed: bool) -> Result<(), Error> {
        let options = self.optional()?;
        let spec = self.argument_string()?;
        self.begin_group();
        let r = (|| {
            if let Some(o) = options {
                self.process_keys(o, Some("/tikz"))?;
                self.st.apply_pending();
            }
            let stroke = self.stroke();
            let d = Point::new(if reversed { -1.0 } else { 1.0 }, 0.0);
            let mut tips = Vec::new();
            let user = self.st.tips.clone();
            super::arrows::tip_items(self, &user, &spec, Point::default(), d, &stroke, &mut tips);
            let t = self.st.transform;
            for item in tips {
                // (tips are paths)
                let Item::Path { segments, fill, stroke } = item else { continue };
                let segments = transform_segments(&segments, &t);
                if self.counts_for_size() {
                    self.canvas.include_path(&segments);
                }
                self.canvas.push(Item::Path { segments, fill, stroke });
            }
            Ok(())
        })();
        self.end_group();
        r
    }
}
