//! The decorations of pgf's libraries, state by state as they are declared there
//! (`pgflibrarydecorations.*.code.tex` and the module's `lineto`, `moveto`, `curveto`), and the
//! automaton that runs them (`\pgf@decorate@run`, `\pgf@metadecoration@run`).
//!
//! A state has options (its width, switches, next state, repeat, auto end and corner, persistent
//! computations) and code that draws in the coordinate system of the current position: x along
//! the path, y to its left, in pt.

use std::collections::VecDeque;

use crate::canvas::Item;
use crate::color::Color;
use crate::scene::{Point, Transform};
use crate::tex::Token;
use crate::{Error, PT};

use super::feet;
use super::walker::{Kind, Walker, EPS, SP};
use super::{Mark, Pos, Settings};
use crate::engine::Engine;

/// A decoration (`\pgfdeclaredecoration`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dec {
    LineTo,
    MoveTo,
    CurveTo,
    Zigzag,
    Saw,
    RandomSteps,
    Bent,
    Snake,
    Coil,
    Bumps,
    Ticks,
    ExpandingWaves,
    Waves,
    Border,
    Brace,
    ShowPathConstruction,
    Markings,
    TextAlongPath,
    Triangles,
    Crosses,
    ShapeBackgrounds,
    Footprints,
    Koch1,
    Koch2,
    KochSnowflake,
    Cantor,
    TextEffects,
    /// the mindmap library's
    CircleConnectionBar,
    /// declared in the document with `\pgfdeclaredecoration`, run as TeX
    User(&'static str),
}

/// A meta-decoration (`\pgfdeclaremetadecoration`): `straight zigzag`, and TikZ's `tikz@internal`
/// that puts `pre` and `post` around the decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meta {
    StraightZigzag,
    Internal,
    /// declared with `\pgfdeclaremetadecoration`
    User(&'static str),
}

/// A name kept for the rest of the run (state and decoration names of the document are few).
pub fn intern(s: &str) -> &'static str {
    thread_local! {
        static NAMES: std::cell::RefCell<std::collections::HashSet<&'static str>> = Default::default();
    }
    NAMES.with(|n| {
        let mut n = n.borrow_mut();
        if let Some(x) = n.get(s) {
            return *x;
        }
        let x: &'static str = Box::leak(s.to_string().into_boxed_str());
        n.insert(x);
        x
    })
}

/// The body of a macro, if defined.
pub(super) fn macro_body(e: &Engine, name: &str) -> Option<Vec<Token>> {
    match e.tex.meaning_of(name) {
        crate::tex::Meaning::Macro(m) => Some(crate::tex::substitute(&m.body, &[])),
        _ => None,
    }
}

/// A decoration by name: one declared in the document, else one of pgf's libraries.
pub fn resolve(e: &Engine, name: &str) -> Option<Dec> {
    let name = name.trim();
    if e.tex.is_defined(&format!("pgf@decorate@@{name}@initial")) {
        return Some(Dec::User(intern(name)));
    }
    lookup(name)
}

pub fn resolve_meta(e: &Engine, name: &str) -> Option<Meta> {
    let name = name.trim();
    if e.tex.is_defined(&format!("pgf@metadecoration@@{name}@initial")) {
        return Some(Meta::User(intern(name)));
    }
    lookup_meta(name)
}

pub fn lookup(name: &str) -> Option<Dec> {
    Some(match name.trim() {
        "lineto" => Dec::LineTo,
        "moveto" => Dec::MoveTo,
        "curveto" => Dec::CurveTo,
        "zigzag" => Dec::Zigzag,
        "saw" => Dec::Saw,
        "random steps" => Dec::RandomSteps,
        "bent" => Dec::Bent,
        "snake" => Dec::Snake,
        "coil" => Dec::Coil,
        "bumps" => Dec::Bumps,
        "ticks" => Dec::Ticks,
        "expanding waves" => Dec::ExpandingWaves,
        "waves" => Dec::Waves,
        "border" => Dec::Border,
        "brace" => Dec::Brace,
        "show path construction" => Dec::ShowPathConstruction,
        "markings" => Dec::Markings,
        "text along path" => Dec::TextAlongPath,
        "triangles" => Dec::Triangles,
        "crosses" => Dec::Crosses,
        "shape backgrounds" => Dec::ShapeBackgrounds,
        "footprints" => Dec::Footprints,
        "Koch curve type 1" => Dec::Koch1,
        "Koch curve type 2" => Dec::Koch2,
        "Koch snowflake" => Dec::KochSnowflake,
        "Cantor set" => Dec::Cantor,
        "text effects along path" => Dec::TextEffects,
        "circle connection bar" => Dec::CircleConnectionBar,
        _ => return None,
    })
}

pub fn lookup_meta(name: &str) -> Option<Meta> {
    match name.trim() {
        "straight zigzag" => Some(Meta::StraightZigzag),
        _ => None,
    }
}

fn initial(d: Dec) -> &'static str {
    match d {
        // (a declared decoration names its own: `run` reads it)
        Dec::LineTo | Dec::MoveTo | Dec::CurveTo | Dec::Saw | Dec::Snake | Dec::Bumps | Dec::ExpandingWaves | Dec::TextAlongPath | Dec::CircleConnectionBar | Dec::User(_) => {
            "initial"
        }
        Dec::Zigzag => "up from center",
        Dec::RandomSteps => "start",
        Dec::Bent => "bent",
        Dec::Coil => "coil",
        Dec::Ticks => "ticks",
        Dec::Waves => "wave",
        Dec::Border => "tick",
        Dec::Brace => "brace",
        Dec::ShowPathConstruction => "check for moveto",
        Dec::Markings | Dec::Triangles | Dec::Crosses | Dec::Koch1 | Dec::Koch2 | Dec::KochSnowflake | Dec::Cantor => "init",
        Dec::ShapeBackgrounds => "initialise",
        Dec::TextEffects => "setup",
        Dec::Footprints => "left",
    }
}

/// The options of a state (`/pgf/decoration automaton/…`), in their order.
#[derive(Debug, Clone, Copy)]
enum Opt {
    Width(f64),
    Switch(f64, &'static str),
    SwitchInput(f64, &'static str),
    Next(&'static str),
    AutoEnd(f64),
    AutoCorner(f64),
    /// `persistent precomputation`, `persistent postcomputation`
    Pre,
    Post,
    Repeat(i64),
}

/// TeX code to run, if any.
type Code = Option<Vec<Token>>;

/// A path command of a foot: moveto, lineto, curveto, closepath (pt).
#[derive(Debug, Clone, Copy)]
pub enum FootOp {
    M(f64, f64),
    L(f64, f64),
    C(f64, f64, f64, f64, f64, f64),
    Z,
}

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn polar(angle: f64, r: f64) -> Point {
    let (s, c) = angle.to_radians().sin_cos();
    p(r * c, r * s)
}

fn shifted(t: &Transform, x: f64, y: f64) -> Transform {
    t.compose(&Transform::translation(x, y))
}

/// Canvas pt → cm for a transformation from local pt.
pub fn to_cm(t: &Transform) -> Transform {
    Transform { e: t.e * PT, f: t.f * PT, ..*t }
}

/// What text along path keeps between states.
#[derive(Debug, Clone, Default)]
struct TextState {
    rest: VecDeque<Token>,
    format: String,
    math: bool,
    indent_left: f64,
    char_shift: f64,
    space_shift: f64,
    fit: bool,
    stretch: bool,
    is_space: bool,
    /// the character's box: its text, format, width (pt); None at the end of the text
    current: Option<(String, String, f64)>,
    shift: f64,
}

/// One run of decorations along one path.
pub struct Run<'a> {
    pub e: &'a mut Engine,
    pub w: Walker,
    pub s: Settings,
    /// `\pgf@decorate@additionaltransform` (raise, mirror, transform), pt
    pub extra: Transform,
    has_corners: bool,
    state: &'static str,
    next_state: &'static str,
    repeat: i64,
    width: f64,
    jump: bool,
    // triangles and crosses
    ssw: f64,
    ssh: f64,
    // markings
    marks: VecDeque<Mark>,
    mark_at: f64,
    mark_action: Vec<Token>,
    mark_next: &'static str,
    seq: i64,
    // shape backgrounds
    shape: ShapeRun,
    text: TextState,
    pub(super) te: super::textfx::State,
    // declared decorations: the persistent computations of the state
    user_pre: Vec<Token>,
    user_post: Vec<Token>,
}

#[derive(Debug, Clone, Default)]
struct ShapeRun {
    initialise: f64,
    before: f64,
    after: f64,
    start_width: f64,
    width: f64,
    width_change: f64,
    start_height: f64,
    height: f64,
    height_change: f64,
    sep: f64,
    between_borders: bool,
    spread: bool,
    special_width: f64,
}

impl<'a> Run<'a> {
    pub fn new(e: &'a mut Engine, w: Walker, s: Settings) -> Run<'a> {
        let extra = s.extra_transform();
        let has_corners = s.has_corners;
        // \pgfdecorationsegmentlength and \pgfdecorationsegmentamplitude, which code may set
        e.tex.dimens.insert(4016, (s.segment_length * 65536.0).round() as i64);
        e.tex.dimens.insert(4017, (s.amplitude * 65536.0).round() as i64);
        Run {
            e,
            w,
            s,
            extra,
            has_corners,
            state: "",
            next_state: "",
            repeat: -1,
            width: 0.0,
            jump: false,
            ssw: 0.0,
            ssh: 0.0,
            marks: VecDeque::new(),
            mark_at: 0.0,
            mark_action: Vec::new(),
            mark_next: "final",
            seq: 0,
            shape: ShapeRun::default(),
            text: TextState::default(),
            user_pre: Vec::new(),
            user_post: Vec::new(),
            te: Default::default(),
        }
    }

    // ------------------------------------------------------------ values

    fn amp(&self) -> f64 {
        self.s.amplitude
    }
    fn len(&self) -> f64 {
        self.s.segment_length
    }

    /// A length stored in a key (`/pgf/decoration/pre length`…), in pt.
    pub fn key_pt(&mut self, key: &str, default: f64) -> f64 {
        match crate::keys::value(&self.e.tex, &format!("/pgf/decoration/{key}")) {
            Some(v) if !crate::keys::trim(&v).is_empty() => match self.e.length_of(v) {
                Ok(l) => l / PT,
                Err(err) => {
                    self.e.warn(format!("decoration: {key}: {err}"));
                    default
                }
            },
            _ => default,
        }
    }

    pub fn key_text(&mut self, key: &str) -> String {
        let v = crate::keys::value(&self.e.tex, &format!("/pgf/decoration/{key}")).unwrap_or_default();
        self.e.tex.expand_to_string(v).unwrap_or_default().trim().to_string()
    }

    fn key_number(&mut self, key: &str, default: f64) -> f64 {
        let t = self.key_text(key);
        if t.is_empty() {
            return default;
        }
        self.e.number(&t).unwrap_or(default)
    }

    /// pgf's `rand`, from the one generator pgfmath has (`\pgfmathsetseed` sets it).
    fn rand(&mut self) -> f64 {
        crate::expr::pseudo_random_signed()
    }

    // ------------------------------------------------------------ path commands

    /// The transformation of the code: the input segment's, then the additional one.
    fn t(&self) -> Transform {
        self.w.transform().compose(&self.extra)
    }
    fn mv(&mut self, t: &Transform, x: f64, y: f64) {
        self.w.out.move_to(t.apply(p(x, y)));
    }
    fn ln(&mut self, t: &Transform, x: f64, y: f64) {
        self.w.out.line_to(t.apply(p(x, y)));
    }
    #[allow(clippy::too_many_arguments)]
    fn cv(&mut self, t: &Transform, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        self.w.out.curve_to(t.apply(p(x1, y1)), t.apply(p(x2, y2)), t.apply(p(x, y)));
    }
    /// `\pgfpathsine` and `\pgfpathcosine`: from the current point, by `(x, y)`.
    fn wave(&mut self, t: &Transform, x: f64, y: f64, sine: bool) {
        let last = self.w.out.last;
        let q = |a: f64, b: f64| last.plus(t.linear(p(a, b)));
        let (c1, c2) = if sine { (q(0.3260 * x, 0.5120 * y), q(0.6380 * x, y)) } else { (q(0.3620 * x, 0.0), q(0.6740 * x, 0.4880 * y)) };
        self.w.out.curve_to(c1, c2, q(x, y));
    }
    /// `\pgfpatharc{a0}{a1}{r}` from the current point.
    fn arc(&mut self, t: &Transform, a0: f64, a1: f64, r: f64) {
        let c = self.w.out.last.minus(t.linear(polar(a0, r)));
        for s in crate::engine::path::arc_segments(c, r, r, a0, a1, t) {
            if let crate::scene::Segment::CurveTo(a, b, q) = s {
                self.w.out.curve_to(a, b, q);
            }
        }
    }

    // ------------------------------------------------------------ the automata

    /// `\pgf@decorate@invoke`: decorates the next `distance` of the path with `dec`.
    pub fn invoke(&mut self, dec: Dec, distance: f64) -> Result<(), Error> {
        self.invoke_with(dec, distance, None, None)
    }

    /// `\pgf@decorate@invoke` with the code to run before and after (`\pgfdecoration`'s list).
    pub fn invoke_with(&mut self, dec: Dec, distance: f64, before: Option<Vec<Token>>, after: Option<Vec<Token>>) -> Result<(), Error> {
        let w = &mut self.w;
        let len = super::walker::trunc_sp(distance).min(w.rem_total);
        w.section = len;
        w.rem_total -= len;
        w.remaining = len;
        w.completed = 0.0;
        if let Some(code) = before {
            self.tex_code(code, &Transform::default(), false, true)?;
            self.after_user_code();
        }
        self.run(dec)?;
        let w = &mut self.w;
        w.path_last = if w.rem_total < 1.0 { w.total_last } else { w.point_ahead(w.remaining) };
        let t = self.t();
        self.final_code(dec, &t)?;
        // the next stretch starts where this one ends, not where its automaton stopped
        let rem = self.w.remaining;
        self.w.move_along_path(rem);
        if let Some(code) = after {
            self.tex_code(code, &Transform::default(), false, true)?;
        }
        self.extra = Transform::default();
        self.w.path_first = self.w.path_last;
        Ok(())
    }

    /// What code of the document may have changed: the segment length and amplitude registers,
    /// and `\pgfsetdecorationsegmenttransformation`.
    fn after_user_code(&mut self) {
        let get = |e: &Engine, r: u32| e.tex.dimens.get(&r).map(|v| *v as f64 / 65536.0);
        if let Some(v) = get(self.e, 4016) {
            self.s.segment_length = v;
        }
        if let Some(v) = get(self.e, 4017) {
            self.s.amplitude = v;
        }
        // (defined, empty, by the decorations' definitions)
        let code = macro_body(self.e, "pgf@decorate@additionaltransform").unwrap_or_default();
        if !crate::keys::trim(&code).is_empty() {
            let e = &mut *self.e;
            e.begin_group();
            e.st.transform = Transform::default();
            let r = e.run_tokens(code);
            let t = e.st.transform;
            e.end_group();
            match r {
                Ok(()) => self.extra = Transform { e: t.e / PT, f: t.f / PT, ..t },
                Err(err) => self.e.warn(format!("decoration segment transformation: {err}")),
            }
        }
    }

    /// Defines what decoration code reads: the registers, `\pgfpointdecoratedpathfirst`… (in the
    /// coordinates of `t`), `\pgfmetadecorated…`.
    fn install_points(&mut self, t: &Transform) {
        super::set_registers(self.e, &self.w);
        let inv = t.inverse();
        let w = &self.w;
        let points = [
            ("pgfpointdecoratedpathfirst", w.path_first),
            ("pgfpointdecoratedpathlast", w.path_last),
            ("pgfpointmetadecoratedpathfirst", w.path_first),
            ("pgfpointmetadecoratedpathlast", w.path_last),
            ("pgfpointdecoratedinputsegmentfirst", w.seg.first),
            ("pgfpointdecoratedinputsegmentlast", w.seg.last),
        ];
        let defs: Vec<(String, String)> = points
            .iter()
            .map(|(n, q)| {
                let l = inv.apply(*q);
                (n.to_string(), format!("\\pgfqpoint{{{}pt}}{{{}pt}}", l.x, l.y))
            })
            .collect();
        let total = w.total;
        let rem = w.rem_total;
        for (n, v) in defs {
            let tokens = crate::engine::tikra_tokens(&v);
            self.e.tex.set_meaning(&Token::cs(&n), crate::engine::macro_tokens(tokens), false);
        }
        self.e.tex.define_string("pgfmetadecoratedpathlength", &format!("{total}pt"), false);
        let (ml, ma) = (self.s.meta_segment_length, self.s.meta_amplitude);
        self.e.tex.define_string("pgfmetadecorationsegmentlength", &format!("{ml}pt"), false);
        self.e.tex.define_string("pgfmetadecorationsegmentamplitude", &format!("{ma}pt"), false);
        self.e.tex.define_string("pgfmetadecoratedremainingdistance", &format!("{rem}pt"), false);
        self.e.tex.define_string("pgfmetadecoratedcompleteddistance", &format!("{}pt", total - rem), false);
    }

    /// Runs decoration code written in TeX: `t` (pt) is the transformation, the basic layer's
    /// path is the decoration's (all of it when `full`, else what the code adds).
    pub fn tex_code(&mut self, code: Vec<Token>, t: &Transform, grouped: bool, full: bool) -> Result<(), Error> {
        self.install_points(t);
        let out = &mut self.w.out;
        let scale = Transform::scaling(PT, PT);
        let mut path = super::super::basic::PgfPath { segments: Vec::new(), last: Some(out.last.scale(PT)), start: Some(out.start.scale(PT)), quick: false };
        let had = !out.segs.is_empty();
        if full {
            path.segments = crate::canvas::transform_segments(&std::mem::take(&mut out.segs), &scale);
        }
        let e = &mut *self.e;
        let saved = std::mem::replace(&mut e.pgf_path, path);
        let saved_t = (e.st.transform, e.st.pending);
        if grouped {
            e.begin_group();
        }
        e.st.transform = to_cm(t);
        e.st.pending = Transform::default();
        let r = e.run_tokens(code);
        if grouped {
            e.end_group();
        } else {
            (e.st.transform, e.st.pending) = saved_t;
        }
        let path = std::mem::replace(&mut e.pgf_path, saved);
        let mut segs = crate::canvas::transform_segments(&path.segments, &Transform::scaling(1.0 / PT, 1.0 / PT));
        let out = &mut self.w.out;
        if !full && had {
            // the basic layer starts a path with a moveto to its current point: the decoration's path goes on
            if let Some(crate::scene::Segment::MoveTo(q)) = segs.first() {
                if q.minus(out.last).norm() < 1e-6 {
                    segs.remove(0);
                }
            }
        }
        out.segs.extend(segs);
        if let Some(l) = path.last {
            out.last = l.scale(1.0 / PT);
        }
        if let Some(st) = path.start {
            out.start = st.scale(1.0 / PT);
        }
        r
    }

    /// The options of a declared state, read as `/pgf/decoration automaton` keys.
    fn user_options(&mut self, name: &str, state: &str) -> Vec<Opt> {
        let Some(tokens) = macro_body(self.e, &format!("pgf@decorate@@{name}@{state}@options")) else { return Vec::new() };
        super::set_registers(self.e, &self.w);
        let mut out = Vec::new();
        self.user_opts(&tokens, &mut out);
        out
    }

    fn user_opts(&mut self, tokens: &[Token], out: &mut Vec<Opt>) {
        for item in crate::keys::split_list(tokens) {
            let v = item.value.unwrap_or_default();
            let len = |r: &mut Run, v: Vec<Token>| match r.e.length_of(v) {
                Ok(l) => l / PT,
                Err(err) => {
                    r.e.warn(format!("decoration state option '{}': {err}", item.key));
                    0.0
                }
            };
            let target = |r: &mut Run, v: &[Token]| -> Option<(f64, &'static str)> {
                let (x, s) = super::split_word(v, "to")?;
                let x = len(r, x);
                Some((x, intern(crate::tex::tokens_to_string(&s).trim())))
            };
            match item.key.as_str() {
                "width" => out.push(Opt::Width(len(self, v))),
                "next state" => out.push(Opt::Next(intern(&self.e.tex.expand_to_string(v).unwrap_or_default()))),
                "switch if less than" => {
                    if let Some((x, s)) = target(self, &v) {
                        out.push(Opt::Switch(x, s));
                    }
                }
                "switch if input segment less than" => {
                    if let Some((x, s)) = target(self, &v) {
                        out.push(Opt::SwitchInput(x, s));
                    }
                }
                "repeat state" => out.push(Opt::Repeat(self.e.number_of(v).unwrap_or(0.0) as i64)),
                "persistent precomputation" => {
                    self.user_pre = v;
                    out.push(Opt::Pre);
                }
                "persistent postcomputation" => {
                    self.user_post = v;
                    out.push(Opt::Post);
                }
                "auto end on length" => out.push(Opt::AutoEnd(len(self, v))),
                "auto corner on length" => out.push(Opt::AutoCorner(len(self, v))),
                "if input segment is closepath" => {
                    if self.w.is_close {
                        self.user_opts(&v, out);
                    }
                }
                other => self.e.warn(format!("decoration state option '{other}' not supported: ignored")),
            }
        }
    }

    /// A declared meta-decoration (`\pgf@metadecoration@run`).
    fn run_user_meta(&mut self, name: &'static str) -> Result<(), Error> {
        let e = &mut *self.e;
        let initial = macro_body(e, &format!("pgf@metadecoration@@{name}@initial")).map(|v| crate::tex::tokens_to_string(&v)).unwrap_or_default();
        let mut state = intern(initial.trim());
        let defs =
            r"\def\decoration#1{\edef\pgf@decorate@tempname{#1}}\def\beforedecoration#1{\def\pgf@decorate@tempbefore{#1}}\def\afterdecoration#1{\def\pgf@decorate@tempafter{#1}}";
        self.e.run_tokens(crate::engine::tikra_tokens(defs))?;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > crate::engine::MAX_STEPS {
                self.e.warn("meta-decoration: too many steps, stopped");
                break;
            }
            if state == "final" {
                break;
            }
            self.install_points(&Transform::default());
            let mut width = 0.0;
            let mut next = state;
            let mut jump = false;
            // (every state declared has its options, maybe none)
            let opts = macro_body(self.e, &format!("pgf@metadecoration@@{name}@{state}@options")).unwrap_or_default();
            for item in crate::keys::split_list(&opts) {
                let v = item.value.unwrap_or_default();
                let mut switch = |r: &mut Run, v: &[Token], input: bool| -> Result<(), Error> {
                    if jump {
                        return Ok(());
                    }
                    let Some((x, s)) = super::split_word(v, "to") else { return Ok(()) };
                    let x = r.e.length_of(x)? / PT;
                    let d = if input { r.w.seg_remaining } else { r.w.rem_total };
                    if d < x - EPS {
                        state = intern(crate::tex::tokens_to_string(&s).trim());
                        jump = true;
                    }
                    Ok(())
                };
                match item.key.as_str() {
                    "width" => {
                        width = self.e.length_of(v.clone())? / PT;
                        let mut t = v;
                        t.extend(crate::engine::tikra_tokens(" to final"));
                        switch(self, &t, false)?;
                    }
                    "switch if less than" => switch(self, &v, false)?,
                    "switch if input segment less than" => switch(self, &v, true)?,
                    "next state" => next = intern(&self.e.tex.expand_to_string(v)?),
                    other => self.e.warn(format!("meta-decoration state option '{other}' not supported: ignored")),
                }
            }
            if jump {
                continue;
            }
            let (dec, before, after) = self.meta_code(name, state)?;
            self.invoke_with(dec, width, before, after)?;
            state = next;
        }
        let (dec, before, after) = self.meta_code(name, "final")?;
        let rem = self.w.rem_total;
        self.invoke_with(dec, rem, before, after)
    }

    /// Runs a meta state's code: the decoration it names and its code before and after.
    fn meta_code(&mut self, name: &str, state: &str) -> Result<(Dec, Code, Code), Error> {
        for m in ["pgf@decorate@tempname", "pgf@decorate@tempbefore", "pgf@decorate@tempafter"] {
            self.e.tex.define_string(m, "", false);
        }
        if let Some(code) = macro_body(self.e, &format!("pgf@metadecoration@@{name}@{state}@code")) {
            self.tex_code(code, &Transform::default(), false, true)?;
            // what the code set (`\pgfset{/pgf/decoration/segment length=4pt}`) holds for the
            // decorations from here on
            self.s = self.e.st.decoration.clone();
        }
        let dname = macro_body(self.e, "pgf@decorate@tempname").map(|v| crate::tex::tokens_to_string(&v)).unwrap_or_default();
        let dname = if dname.trim().is_empty() { "moveto".to_string() } else { dname };
        let dec = resolve(self.e, &dname).unwrap_or_else(|| {
            self.e.warn(format!("decoration '{}' not supported: moved over", dname.trim()));
            Dec::MoveTo
        });
        let code = |r: &mut Run, n: &str| macro_body(r.e, n).filter(|v| !crate::keys::trim(v).is_empty());
        let before = code(self, "pgf@decorate@tempbefore");
        let after = code(self, "pgf@decorate@tempafter");
        Ok((dec, before, after))
    }

    /// `\pgfmetadecoration`: runs the meta-automaton, each state decorating a stretch.
    pub fn run_meta(&mut self, meta: Meta, main: Option<Dec>) -> Result<(), Error> {
        if let Meta::User(name) = meta {
            return self.run_user_meta(name);
        }
        let mut state = "initial";
        let main_name = main.unwrap_or(Dec::LineTo);
        let pre = self.key_text("pre");
        let post = self.key_text("post");
        let named = |e: &mut Engine, n: &str| -> Dec {
            resolve(e, n).unwrap_or_else(|| {
                e.warn(format!("decoration '{n}' not supported: drawn as lineto"));
                Dec::LineTo
            })
        };
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > crate::engine::MAX_STEPS {
                self.e.warn("decoration: too many steps, stopped");
                break;
            }
            let (width, next, dec) = match (meta, state) {
                (Meta::StraightZigzag, "initial") => (self.s.meta_segment_length, "zigzag", Dec::CurveTo),
                (Meta::StraightZigzag, _) => (self.s.meta_segment_length, "initial", Dec::Zigzag),
                (Meta::Internal, "initial") => (self.key_pt("pre length", 0.0), "main", named(self.e, &pre)),
                // (a declared meta-decoration is run above)
                (Meta::Internal | Meta::User(_), _) => (self.w.rem_total - self.key_pt("post length", 0.0), "final", main_name),
            };
            if self.w.rem_total < width - EPS {
                break;
            }
            if meta == Meta::Internal {
                self.extra = self.s.extra_transform();
            }
            self.invoke(dec, width)?;
            state = next;
            if state == "final" {
                break;
            }
        }
        let dec = match meta {
            Meta::StraightZigzag => Dec::CurveTo,
            Meta::Internal | Meta::User(_) => {
                self.extra = self.s.extra_transform();
                named(self.e, &post)
            }
        };
        let rem = self.w.rem_total;
        self.invoke(dec, rem)
    }

    /// `\pgf@decorate@run`: states until `final`.
    fn run(&mut self, dec: Dec) -> Result<(), Error> {
        self.state = match dec {
            Dec::User(n) => intern(macro_body(self.e, &format!("pgf@decorate@@{n}@initial")).map(|v| crate::tex::tokens_to_string(&v)).unwrap_or_default().trim()),
            _ => initial(dec),
        };
        self.repeat = -1;
        let mut guard = 0usize;
        loop {
            if self.state == "final" {
                return Ok(());
            }
            guard += 1;
            if guard > crate::engine::MAX_STEPS {
                self.e.warn("decoration: too many steps (segment length too small?): stopped");
                return Ok(());
            }
            let state = self.state;
            self.next_state = state;
            self.jump = false;
            let opts = self.options(dec, state);
            let (mut pre, mut post) = (false, false);
            for o in &opts {
                self.option(*o, &mut pre, &mut post);
            }
            if self.jump {
                continue;
            }
            let (user_pre, user_post) = (std::mem::take(&mut self.user_pre), std::mem::take(&mut self.user_post));
            if pre {
                if matches!(dec, Dec::User(_)) {
                    self.tex_code(user_pre, &Transform::default(), false, false)?;
                } else {
                    self.precompute(dec, state)?;
                }
            }
            let t = self.t();
            self.code(dec, state, &t)?;
            if post {
                self.tex_code(user_post, &Transform::default(), false, false)?;
            }
            // the width is read when the automaton moves (after the precomputation)
            if let Some(w) = self.options(dec, state).iter().rev().find_map(|o| if let Opt::Width(w) = o { Some(*w) } else { None }) {
                self.width = w;
            }
            let w = self.width;
            self.w.move_along_path(w);
            if self.repeat > 0 {
                self.repeat -= 1;
            } else {
                self.repeat = -1;
                self.state = self.next_state;
            }
        }
    }

    pub(super) fn set_next_state(&mut self, state: &'static str) {
        self.next_state = state;
    }

    fn switch_to(&mut self, state: &'static str) {
        self.state = state;
        self.repeat = -1;
        self.jump = true;
    }

    fn option(&mut self, o: Opt, pre: &mut bool, post: &mut bool) {
        match o {
            Opt::Width(w) => {
                self.width = w;
                self.switch_if(w, "final");
            }
            Opt::Switch(x, s) => self.switch_if(x, s),
            Opt::SwitchInput(x, s) => {
                if !self.jump && self.w.seg_remaining < x - EPS {
                    self.switch_to(s);
                }
            }
            Opt::Next(s) => self.next_state = s,
            Opt::Pre => *pre = true,
            Opt::Post => *post = true,
            Opt::Repeat(n) => {
                if self.repeat == -1 {
                    self.repeat = n;
                }
            }
            Opt::AutoEnd(x) => {
                if self.jump {
                    return;
                }
                if self.w.is_close {
                    if self.w.remaining > x + EPS {
                        if self.w.seg_remaining <= x + EPS {
                            self.w.out.close();
                            let d = self.w.seg_remaining;
                            self.w.move_along_path(d);
                            self.repeat = -1;
                            self.jump = true;
                        }
                    } else {
                        self.w.out.close();
                        self.width = self.w.remaining;
                        self.switch_to("final");
                    }
                } else if self.w.remaining <= x + EPS {
                    let t = self.t();
                    let r = self.w.remaining;
                    self.ln(&t, r, 0.0);
                    self.width = r;
                    self.switch_to("final");
                }
            }
            Opt::AutoCorner(x) => {
                if self.has_corners && !self.jump && self.w.seg_remaining <= x + EPS {
                    let t = self.t();
                    let r = self.w.seg_remaining;
                    self.ln(&t, r, 0.0);
                    self.w.move_along_path(r);
                    self.repeat = -1;
                    self.jump = true;
                }
            }
        }
    }

    fn switch_if(&mut self, x: f64, state: &'static str) {
        if self.jump {
            return;
        }
        if self.w.remaining.abs() < EPS || self.w.remaining < x - EPS {
            self.switch_to(state);
        }
    }

    fn options(&mut self, dec: Dec, state: &str) -> Vec<Opt> {
        use Opt::*;
        let (a, l) = (self.amp(), self.len());
        let rem = self.w.remaining;
        let seg_rem = self.w.seg_remaining;
        let aspect = self.s.aspect;
        match (dec, state) {
            (Dec::User(n), s) => self.user_options(n, s),
            (Dec::LineTo, "initial") => vec![SwitchInput(rem, "input segment"), Width(rem)],
            (Dec::LineTo, _) => vec![Width(self.w.seg.len), Next("initial")],
            (Dec::MoveTo, _) => vec![Width(rem)],
            (Dec::CurveTo, _) => vec![Width(self.w.seg.len / 100.0)],
            (Dec::Zigzag, "up from center") => vec![Width(0.5 * l), Next("big down")],
            (Dec::Zigzag, "big down") => vec![Switch(0.5 * l, "center finish"), Width(0.5 * l), Next("big up")],
            (Dec::Zigzag, "big up") => vec![Switch(0.5 * l, "center finish"), Width(0.5 * l), Next("big down")],
            (Dec::Zigzag, _) => vec![Width(0.0), Next("final")],
            (Dec::Saw, _) => vec![AutoEnd(l), AutoCorner(l), Width(l)],
            (Dec::RandomSteps, "start") => vec![Width(0.0), Next("step"), Pre],
            (Dec::RandomSteps, _) => vec![AutoEnd(1.5 * l), AutoCorner(1.5 * l), Width(l)],
            (Dec::Bent, _) => vec![Width(seg_rem)],
            (Dec::Snake, "initial") => vec![Switch(0.625 * l, "final"), Width(0.3125 * l), Next("down")],
            (Dec::Snake, "down") => vec![Switch(0.8125 * l, "end down"), Width(0.5 * l), Next("up")],
            (Dec::Snake, "up") => vec![Switch(0.8125 * l, "end up"), Width(0.5 * l), Next("down")],
            (Dec::Snake, _) => vec![Width(0.3125 * l), Next("final")],
            (Dec::Coil, "coil") => vec![Switch(1.5 * l + 2.0 * aspect * a, "last"), Width(l)],
            (Dec::Coil, _) => vec![Width(0.5 * l + 2.0 * aspect * a), Next("final")],
            (Dec::Bumps, _) => vec![AutoEnd(0.51 * l), AutoCorner(0.51 * l), Width(0.5 * l)],
            (Dec::Ticks, _) => vec![Width(l)],
            (Dec::ExpandingWaves, "initial") => vec![Width(l), Next("wave")],
            (Dec::ExpandingWaves, "wave") => vec![Switch(l, "last"), Width(l)],
            (Dec::ExpandingWaves, _) => vec![Width(0.0), Next("final")],
            (Dec::Waves, _) => vec![Width(l)],
            (Dec::Border, "tick") => vec![Switch(l, "last"), Width(l)],
            (Dec::Border, _) => vec![Width(a), Next("final")],
            (Dec::Brace, _) => vec![Width(rem), Next("final")],
            (Dec::ShowPathConstruction, "check for moveto") => vec![Width(0.0), Next("segment"), Pre],
            (Dec::ShowPathConstruction, _) => vec![Width(seg_rem), Next("check for moveto"), Pre],
            (Dec::Markings, "init") => vec![Width(0.0), Pre, Next("pre")],
            (Dec::Markings, "pre") => vec![Width(0.0), Pre, Next("skipper")],
            (Dec::Markings, "skipper") => vec![Width(self.mark_at - self.w.completed - SP), Next(self.mark_next)],
            (Dec::Markings, _) => vec![Width(SP), Next("pre"), Pre],
            (Dec::Triangles | Dec::Crosses, "init") => vec![Width(0.0), Next(if dec == Dec::Triangles { "triangle" } else { "crosses" }), Pre],
            (Dec::Triangles, "triangle") | (Dec::Crosses, "crosses") => vec![Switch(l, "last"), Width(l)],
            (Dec::Triangles, "last") => vec![Switch(self.ssw, "skip"), Width(rem), Next("final")],
            (Dec::Triangles | Dec::Crosses, _) => vec![Width(rem)],
            (Dec::Koch1 | Dec::Koch2 | Dec::KochSnowflake | Dec::Cantor, _) => vec![Width(seg_rem)],
            (Dec::CircleConnectionBar, "initial") => vec![Width(0.0), Next("bar")],
            (Dec::CircleConnectionBar, "bar") => vec![Width(0.0), Next("end")],
            (Dec::CircleConnectionBar, _) => vec![Width(0.0), Next("final")],
            (Dec::Footprints, "left") => {
                let w = self.key_pt("stride length", 30.0) / 2.0;
                vec![Width(w), Next("right")]
            }
            (Dec::Footprints, _) => {
                let w = self.key_pt("stride length", 30.0) / 2.0;
                vec![Width(w), Next("left")]
            }
            (Dec::ShapeBackgrounds, "initialise") => vec![Width(self.shape.initialise), Next("shape"), Pre],
            (Dec::ShapeBackgrounds, "before shape") => vec![Width(self.shape.before - SP), Next("shape"), Pre],
            (Dec::ShapeBackgrounds, "shape") => vec![Width(0.0), Next("after shape")],
            (Dec::ShapeBackgrounds, "after shape") => vec![Width(self.shape.after - SP), Next("sep"), Pre],
            (Dec::ShapeBackgrounds, _) => vec![Width(self.shape.sep), Next("before shape"), Pre],
            (Dec::TextEffects, "setup") => vec![Width(0.0), Next("scan"), Pre],
            (Dec::TextEffects, "scan") => vec![Width(0.0), Next("pre token"), Pre],
            (Dec::TextEffects, "pre token") => vec![Width(self.te_current().map_or(0.0, |c| c.pre)), Next("token")],
            (Dec::TextEffects, "token") => vec![Width(0.0), Next("post token")],
            (Dec::TextEffects, _) => vec![Width(self.te_current().map_or(0.0, |c| c.post)), Next("scan")],
            (Dec::TextAlongPath, "initial") => vec![Width(0.0), Next("left indent"), Pre],
            (Dec::TextAlongPath, "left indent") => vec![Width(self.text.indent_left), Next("scan")],
            (Dec::TextAlongPath, "scan") => vec![Width(0.0), Next("before typeset"), Pre],
            (Dec::TextAlongPath, "before typeset") => vec![Width(0.5 * self.text_width()), Next("typeset")],
            (Dec::TextAlongPath, "typeset") => vec![Width(0.0), Next("after typeset")],
            (Dec::TextAlongPath, "after typeset") => vec![Width(0.5 * self.text_width()), Next("shift"), Pre],
            (Dec::TextAlongPath, _) => vec![Width(self.text.shift), Next("scan")],
        }
    }

    /// The persistent precomputation of a state.
    fn precompute(&mut self, dec: Dec, state: &str) -> Result<(), Error> {
        match (dec, state) {
            (Dec::RandomSteps, _) => self.has_corners = true,
            (Dec::Triangles, _) => {
                self.ssw = self.key_pt("shape start width", 2.5);
                self.ssh = self.key_pt("shape start height", 2.5) / 2.0;
            }
            (Dec::Crosses, _) => {
                self.ssw = self.key_pt("shape start width", 2.5) / 2.0;
                self.ssh = self.key_pt("shape start height", 2.5) / 2.0;
            }
            (Dec::ShowPathConstruction, "check for moveto") => {
                if self.w.prev_is_moveto {
                    self.segment_code(0)?;
                }
            }
            (Dec::ShowPathConstruction, _) => self.segment_code_current()?,
            (Dec::Markings, "init") => {
                self.seq = 0;
                self.marks = self.s.marks.iter().cloned().collect();
                self.set_mark_info()?;
            }
            (Dec::Markings, "pre") => self.next_mark()?,
            (Dec::Markings, _) => {
                self.seq += 1;
                self.set_mark_info()?;
            }
            (Dec::ShapeBackgrounds, s) => self.shape_precompute(s)?,
            (Dec::TextEffects, "setup") => self.te_setup()?,
            (Dec::TextEffects, _) => self.te_scan(),
            (Dec::TextAlongPath, "initial") => self.text_initial()?,
            (Dec::TextAlongPath, "scan") => self.text_scan()?,
            // text along path (the only other decoration with a precomputation)
            _ => {
                let tx = &mut self.text;
                tx.shift = if tx.fit {
                    if tx.stretch {
                        if tx.is_space {
                            tx.space_shift
                        } else {
                            0.0
                        }
                    } else {
                        tx.char_shift
                    }
                } else {
                    0.0
                };
            }
        }
        Ok(())
    }

    /// The code of a state, drawn with `t`.
    fn code(&mut self, dec: Dec, state: &str, t: &Transform) -> Result<(), Error> {
        let (a, l) = (self.amp(), self.len());
        match (dec, state) {
            (Dec::User(n), s) => {
                macro_body(self.e, &format!("pgf@decorate@@{n}@{s}@code")).map(|code| self.tex_code(code, t, true, false)).transpose()?;
            }
            (Dec::LineTo, "input segment") => {
                let q = self.w.seg.last;
                self.w.out.line_to(q);
            }
            (Dec::CurveTo, _) => self.ln(t, 0.0, 0.0),
            (Dec::Zigzag, "up from center") | (Dec::Zigzag, "big up") => self.ln(t, 0.25 * l, a),
            (Dec::Zigzag, "big down") => self.ln(t, 0.25 * l, -a),
            (Dec::Zigzag, "center finish") => self.ln(t, 0.0, 0.0),
            (Dec::Saw, _) => {
                self.ln(t, l, a);
                self.ln(t, l, 0.0);
            }
            (Dec::RandomSteps, "step") => {
                let x = self.rand() * a;
                let y = self.rand() * a;
                self.ln(t, l + x, y);
            }
            (Dec::Bent, _) => {
                let r = self.w.seg_remaining;
                let k = self.s.aspect * r;
                self.cv(t, k, a, r - k, a, r, 0.0);
            }
            (Dec::Snake, "initial") => self.cv(t, 0.125 * l, 0.0, 0.1875 * l, a, 0.3125 * l, a),
            (Dec::Snake, "down") | (Dec::Snake, "up") => {
                let y = if state == "down" { -a } else { a };
                self.wave(t, 0.25 * l, y, false);
                self.wave(t, 0.25 * l, y, true);
            }
            (Dec::Snake, "end down") => self.cv(t, 0.125 * l, a, 0.1875 * l, 0.0, 0.3125 * l, 0.0),
            (Dec::Snake, "end up") => self.cv(t, 0.125 * l, -a, 0.1875 * l, 0.0, 0.3125 * l, 0.0),
            (Dec::Coil, _) => {
                let asp = self.s.aspect;
                let on = |x: f64, y: f64, k: f64| p(x * a * asp + k * 0.083333333333 * l, y * a);
                let half = [[(0.0, 0.555, 1.0), (0.445, 1.0, 2.0), (1.0, 1.0, 3.0)], [(1.555, 1.0, 4.0), (2.0, 0.555, 5.0), (2.0, 0.0, 6.0)]];
                let rest = [[(2.0, -0.555, 7.0), (1.555, -1.0, 8.0), (1.0, -1.0, 9.0)], [(0.445, -1.0, 10.0), (0.0, -0.555, 11.0), (0.0, 0.0, 12.0)]];
                let n = if state == "coil" { 4 } else { 2 };
                for c in half.iter().chain(rest.iter()).take(n) {
                    let q: Vec<Point> = c.iter().map(|&(x, y, k)| t.apply(on(x, y, k))).collect();
                    self.w.out.curve_to(q[0], q[1], q[2]);
                }
            }
            (Dec::Bumps, _) => {
                self.cv(t, 0.0, 0.555 * a, 0.11125 * l, a, 0.25 * l, a);
                self.cv(t, 0.38875 * l, a, 0.5 * l, 0.5 * a, 0.5 * l, 0.0);
            }
            (Dec::Ticks, _) => {
                self.mv(t, 0.0, a);
                self.ln(t, 0.0, -a);
            }
            (Dec::ExpandingWaves, "wave") | (Dec::ExpandingWaves, "last") => {
                let c = self.w.completed;
                let ang = self.s.angle;
                let q = p(-c, 0.0).plus(polar(ang, c));
                self.mv(t, q.x, q.y);
                self.arc(t, ang, -ang, c);
            }
            (Dec::Waves, _) => {
                let t = shifted(t, l, 0.0);
                let r = self.key_pt("start radius", 2.5);
                let ang = self.s.angle;
                let q = p(-r, 0.0).plus(polar(ang, r));
                self.mv(&t, q.x, q.y);
                self.arc(&t, ang, -ang, r);
            }
            (Dec::Border, _) => {
                self.mv(t, 0.0, 0.0);
                let q = polar(self.s.angle, a);
                self.ln(t, q.x, q.y);
            }
            (Dec::Brace, _) => {
                let r = self.w.remaining;
                let asp = self.s.aspect;
                let mut yc = asp * r;
                yc = if 2.0 * a > yc { 0.5 * yc } else { a };
                let mut xc = asp * r - r;
                xc = if -2.0 * a < xc { -0.5 * xc } else { a };
                self.mv(t, 0.0, 0.0);
                self.cv(t, 0.15 * yc, 0.3 * a, 0.5 * yc, 0.5 * a, yc, 0.5 * a);
                let t2 = shifted(t, asp * r, 0.0);
                self.ln(&t2, -yc, 0.5 * a);
                self.cv(&t2, -0.5 * yc, 0.5 * a, -0.15 * yc, 0.7 * a, 0.0, a);
                self.cv(&t2, 0.15 * xc, 0.7 * a, 0.5 * xc, 0.5 * a, xc, 0.5 * a);
                let t3 = shifted(t, r, 0.0);
                self.ln(&t3, -xc, 0.5 * a);
                self.cv(&t3, -0.5 * xc, 0.5 * a, -0.15 * xc, 0.3 * a, 0.0, 0.0);
            }
            (Dec::Markings, "main") => self.mark(t)?,
            (Dec::Triangles, "triangle") | (Dec::Triangles, "last") => {
                let (w, h) = (self.ssw, self.ssh);
                self.mv(t, 0.0, h);
                self.ln(t, w, 0.0);
                self.ln(t, 0.0, -h);
                self.w.out.close();
            }
            (Dec::Crosses, "crosses") | (Dec::Crosses, "last") => self.cross(t),
            (Dec::CircleConnectionBar, state) => {
                // the start cap, the band, the end cap: all from the start, as their widths are 0
                let (r1, r2) = (self.key_pt("start radius", 2.5), self.key_pt("end radius", 2.5));
                let (ang, rem) = (self.s.angle, self.w.remaining);
                let all = crate::engine::mindmap::bar(r1, r2, a, ang, rem, t);
                let mut pieces = all.split_inclusive(|s| matches!(s, crate::scene::Segment::Close));
                let piece = match state {
                    "initial" => pieces.next(),
                    "bar" => pieces.nth(1),
                    _ => pieces.nth(2),
                };
                for seg in piece.unwrap_or_default() {
                    match *seg {
                        crate::scene::Segment::MoveTo(q) => self.w.out.move_to(q),
                        crate::scene::Segment::LineTo(q) => self.w.out.line_to(q),
                        crate::scene::Segment::CurveTo(c1, c2, q) => self.w.out.curve_to(c1, c2, q),
                        crate::scene::Segment::Close => self.w.out.close(),
                    }
                }
            }
            (Dec::Koch1 | Dec::Koch2 | Dec::KochSnowflake | Dec::Cantor, _) => {
                let r = self.w.seg_remaining;
                let pts: &[(f64, f64, bool)] = match dec {
                    Dec::Koch1 => &[(0.33333, 0.0, true), (0.33333, 0.33333, true), (0.66666, 0.33333, true), (0.66666, 0.0, true), (1.0, 0.0, true)],
                    Dec::Koch2 => {
                        &[(0.25, 0.0, true), (0.25, 0.25, true), (0.5, 0.25, true), (0.5, 0.0, true), (0.5, -0.25, true), (0.75, -0.25, true), (0.75, 0.0, true), (1.0, 0.0, true)]
                    }
                    Dec::KochSnowflake => &[(0.3333, 0.0, true), (0.5, 0.2886751347, true), (0.6666, 0.0, true), (1.0, 0.0, true)],
                    _ => &[(0.3333, 0.0, true), (0.6666, 0.0, false), (1.0, 0.0, true)],
                };
                for &(x, y, line) in pts {
                    if line {
                        self.ln(t, x * r, y * r);
                    } else {
                        self.mv(t, x * r, y * r);
                    }
                }
            }
            (Dec::Footprints, s) => {
                let t = if s == "right" { t.compose(&Transform::scaling(1.0, -1.0)) } else { *t };
                let t = shifted(&t, 0.0, self.key_pt("foot sep", 4.0) / 2.0);
                let size = self.key_pt("foot length", 10.0);
                let t = t.compose(&Transform::scaling(size, size)).compose(&Transform::rotation(self.key_number("foot angle", 10.0)));
                self.foot(&t);
            }
            (Dec::ShapeBackgrounds, "shape") => self.shape_code(t),
            (Dec::TextAlongPath, "typeset") => self.text_typeset(t),
            (Dec::TextEffects, "token") => {
                self.te.count.checked_sub(1).filter(|i| *i < self.te.chars.len()).map(|i| self.te_node(i, Some(t))).transpose()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn final_code(&mut self, dec: Dec, t: &Transform) -> Result<(), Error> {
        let last = self.w.path_last;
        match dec {
            Dec::LineTo | Dec::CurveTo | Dec::Zigzag | Dec::Snake | Dec::Coil | Dec::Bumps => self.w.out.line_to(last),
            Dec::MoveTo | Dec::ExpandingWaves | Dec::Waves | Dec::Border | Dec::Triangles | Dec::Crosses | Dec::ShapeBackgrounds => self.w.out.move_to(last),
            Dec::Ticks => {
                let a = self.amp();
                self.mv(t, 0.0, a);
                self.ln(t, 0.0, -a);
                self.w.out.move_to(last);
            }
            Dec::Markings => {
                if self.s.connection_node.is_empty() {
                    self.w.out.move_to(last);
                } else {
                    self.w.out.line_to(last);
                }
            }
            Dec::ShowPathConstruction => self.segment_code_current()?,
            Dec::User(n) => {
                if let Some(code) = macro_body(self.e, &format!("pgf@decorate@@{n}@final@code")) {
                    self.tex_code(code, t, true, false)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn cross(&mut self, t: &Transform) {
        let (w, h) = (self.ssw, self.ssh);
        self.mv(t, -w, h);
        self.ln(t, w, -h);
        self.mv(t, -w, -h);
        self.ln(t, w, h);
    }

    fn foot(&mut self, t: &Transform) {
        let foot = match self.key_text("foot of").as_str() {
            "gnome" => &feet::GNOME,
            "bird" => &feet::BIRD,
            "felis silvestris" => &feet::FELIS_SILVESTRIS,
            "human" => &feet::HUMAN,
            other => {
                self.e.warn(format!("footprints of '{other}' not known: drawn as human"));
                &feet::HUMAN
            }
        };
        let [a, b, c, d, e, f] = foot.cm;
        let t = t.compose(&Transform { a, b, c, d, e, f }).compose(&Transform::scaling(foot.scale, foot.scale));
        for op in foot.ops {
            match *op {
                FootOp::M(x, y) => self.mv(&t, x, y),
                FootOp::L(x, y) => self.ln(&t, x, y),
                FootOp::C(x1, y1, x2, y2, x, y) => self.cv(&t, x1, y1, x2, y2, x, y),
                FootOp::Z => self.w.out.close(),
            }
        }
    }

    // ------------------------------------------------------------ show path construction

    fn segment_code_current(&mut self) -> Result<(), Error> {
        // (the walker moves past a moveto at once: it is never the current segment)
        let k = match self.w.seg.kind {
            Kind::LineTo => 1,
            Kind::CurveTo => 2,
            Kind::ClosePath => 3,
            Kind::MoveTo | Kind::Last => return Ok(()),
        };
        self.segment_code(k)
    }

    /// Runs `moveto code`, `lineto code`… with `\tikzinputsegmentfirst`… set, in canvas coordinates.
    fn segment_code(&mut self, k: usize) -> Result<(), Error> {
        let Some(code) = self.s.segment_code[k].clone() else { return Ok(()) };
        let s = self.w.seg;
        let e = &mut *self.e;
        e.begin_group();
        for (name, q) in [("tikzinputsegmentfirst", s.first), ("tikzinputsegmentlast", s.last), ("tikzinputsegmentsupporta", s.sa), ("tikzinputsegmentsupportb", s.sb)] {
            e.tex.define_string(name, &format!("{}pt,{}pt", q.x, q.y), false);
        }
        e.st.transform = Transform::default();
        e.st.pending = Transform::default();
        let r = e.run_tokens(code);
        e.end_group();
        r
    }

    // ------------------------------------------------------------ markings

    fn set_mark_info(&mut self) -> Result<(), Error> {
        let seq = self.seq;
        // a dimension, as `\the` writes it (`120.0pt`)
        let at = if seq == 0 { String::new() } else { crate::tex::dimen::show((self.mark_at * 65536.0).round() as i64) };
        let e = &mut *self.e;
        e.tex.define_string(&crate::keys::macro_name("/pgf/decoration/mark info/sequence number"), &seq.to_string(), false);
        e.tex.define_string(&crate::keys::macro_name("/pgf/decoration/mark info/distance from start"), &at, false);
        Ok(())
    }

    /// `\pgf@lib@dec@parsenum`: a position in pt (a fraction of the path without a unit; from the
    /// end if negative).
    fn position(&mut self, pos: &Pos) -> Result<f64, Error> {
        let l = self.w.section;
        match pos {
            Pos::Pt(v) => Ok(if *v < 0.0 { l + v } else { *v }),
            Pos::Tokens(t) => {
                super::set_registers(self.e, &self.w);
                let s = self.e.math_string(t.clone())?;
                let (v, unit) = crate::expr::eval_length(&s, &self.e.vars())?;
                Ok(if unit {
                    let v = v / PT;
                    if v < 0.0 {
                        l + v
                    } else {
                        v
                    }
                } else if v < 0.0 {
                    l + v * l
                } else {
                    v * l
                })
            }
        }
    }

    /// The `pre` state: the next mark of the list, or the end.
    fn next_mark(&mut self) -> Result<(), Error> {
        self.mark_at = 0.0;
        self.mark_next = "final";
        while let Some(m) = self.marks.pop_front() {
            match m {
                Mark::At { pos, action } => {
                    self.mark_at = self.position(&pos)?;
                    self.mark_action = action;
                    self.mark_next = "main";
                    return Ok(());
                }
                Mark::Between { start, end, step, action } => {
                    let a = self.position(&start)?;
                    let b = self.position(&end)?;
                    if b < a - EPS {
                        continue;
                    }
                    let st = self.position(&step)?;
                    if st <= 0.0 {
                        self.e.warn("mark between positions: the step must be positive: mark ignored");
                        continue;
                    }
                    self.marks.push_front(Mark::Between { start: Pos::Pt(a + st), end: Pos::Pt(b), step: Pos::Pt(st), action: action.clone() });
                    self.mark_at = a;
                    self.mark_action = action;
                    self.mark_next = "main";
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// The `main` state: the mark's code, in a scope at the mark.
    fn mark(&mut self, t: &Transform) -> Result<(), Error> {
        let action = self.mark_action.clone();
        super::set_registers(self.e, &self.w);
        let e = &mut *self.e;
        e.begin_group();
        e.begin_scope();
        e.st.transform = to_cm(t);
        e.st.pending = Transform::default();
        e.st.decoration.marks = Default::default();
        e.st.decorate = false;
        e.tex.set_meaning(&Token::cs("arrow"), e.tex.meaning_of("tikra@arrow"), false);
        e.tex.set_meaning(&Token::cs("arrowreversed"), e.tex.meaning_of("tikra@arrowreversed"), false);
        let r = e.run_tokens(action);
        e.end_scope();
        e.end_group();
        r?;
        let node = self.s.connection_node.clone();
        if !node.is_empty() {
            match self.e.nodes.get(&node).cloned() {
                Some(n) => {
                    let tc = to_cm(t);
                    let a = n.border_toward(tc.apply(p(-PT, 0.0)));
                    let b = n.border_toward(tc.apply(p(PT, 0.0)));
                    self.w.out.line_to(a.scale(1.0 / PT));
                    self.w.out.move_to(b.scale(1.0 / PT));
                }
                None => self.e.warn(format!("mark connection node: no node named '{node}'")),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ shape backgrounds

    fn shape_precompute(&mut self, state: &str) -> Result<(), Error> {
        match state {
            "initialise" => {
                let sh = &mut self.shape;
                *sh = ShapeRun::default();
                let sw = self.key_pt("shape start width", 2.5);
                let ew = self.key_pt("shape end width", 2.5);
                let hs = self.key_pt("shape start height", 2.5);
                let he = self.key_pt("shape end height", 2.5);
                let scaled = self.s.shape_scaled;
                let sep_text = self.s.shape_sep.clone();
                let spread_text = self.s.shape_spread.clone();
                let rem = self.w.remaining;
                let mut sh = ShapeRun { start_width: sw, width: sw, width_change: ew - sw, start_height: hs, height: hs, height_change: he - hs, ..Default::default() };
                if spread_text.trim().is_empty() {
                    let (amount, keyword) = split_keyword(&sep_text);
                    sh.between_borders = keyword == "between borders";
                    sh.sep = self.e.length(&amount)? / PT;
                } else {
                    sh.spread = true;
                    let (amount, keyword) = split_keyword(&spread_text);
                    sh.between_borders = keyword == "by borders";
                    let mut n = self.e.number(&amount)?.trunc() as i64;
                    let mut x;
                    if sh.between_borders {
                        if n > 1 {
                            n -= 1;
                            let ab = if scaled { sw + ew } else { 2.0 * sw };
                            x = ((rem - n as f64 * 0.5 * ab) / n as f64) * 0.9999;
                            if scaled {
                                sh.special_width = sh.width_change / n as f64;
                            }
                        } else {
                            sh.between_borders = false;
                            x = rem;
                            if n == 1 {
                                sh.initialise = 0.5 * x;
                            } else {
                                x += 5.0;
                                sh.initialise = x;
                            }
                        }
                    } else {
                        x = rem;
                        if n > 1 {
                            x /= (n - 1) as f64;
                        } else if n == 1 {
                            sh.initialise = 0.5 * x;
                        } else {
                            x += 5.0;
                            sh.initialise = x;
                        }
                    }
                    sh.sep = x;
                }
                self.shape = sh;
            }
            "before shape" => {
                let c = self.w.completed;
                let r = self.w.remaining;
                let sh = &mut self.shape;
                if self.s.shape_scaled {
                    let ratio = if sh.between_borders {
                        if !sh.spread {
                            (c + sh.start_width * 0.5) / (c + r - 0.5 * sh.width_change)
                        } else {
                            0.0
                        }
                    } else {
                        c / (c + r)
                    };
                    let ratio = if sh.spread && sh.between_borders {
                        let w = sh.width + sh.special_width;
                        (w - sh.start_width) / sh.width_change
                    } else {
                        ratio
                    };
                    sh.width = if sh.spread && sh.between_borders { sh.width + sh.special_width } else { sh.width_change * ratio + sh.start_width };
                    sh.height = sh.height_change * ratio + sh.start_height;
                }
                sh.before = if sh.between_borders { 0.5 * sh.width } else { 0.0 };
            }
            "after shape" => {
                let sh = &mut self.shape;
                sh.after = if sh.between_borders { 0.5 * sh.width } else { 0.0 };
            }
            _ => self.shape.before = 0.0,
        }
        Ok(())
    }

    /// The shape's background path, scaled to the shape's width and height, at its anchor.
    fn shape_code(&mut self, t: &Transform) {
        let mut t = *t;
        if !self.s.shape_sloped {
            t = t.compose(&Transform::rotation(-self.w.angle));
        }
        let (w, h) = (self.shape.width / 2.0, self.shape.height / 2.0);
        let name = self.key_text("shape");
        let anchor = self.key_text("anchor");
        let off = match anchor.as_str() {
            "north" => p(0.0, h),
            "south" => p(0.0, -h),
            "east" => p(w, 0.0),
            "west" => p(-w, 0.0),
            "north east" => p(w, h),
            "north west" => p(-w, h),
            "south east" => p(w, -h),
            "south west" => p(-w, -h),
            _ => p(0.0, 0.0),
        };
        let base = t;
        let t = shifted(&t, -off.x, -off.y);
        match name.as_str() {
            "circle" | "ellipse" => {
                let k = 0.5523;
                self.mv(&t, w, 0.0);
                self.cv(&t, w, h * k, w * k, h, 0.0, h);
                self.cv(&t, -w * k, h, -w, h * k, -w, 0.0);
                self.cv(&t, -w, -h * k, -w * k, -h, 0.0, -h);
                self.cv(&t, w * k, -h, w, -h * k, w, 0.0);
                self.w.out.close();
            }
            "diamond" => {
                self.mv(&t, 0.0, -h);
                self.ln(&t, w, 0.0);
                self.ln(&t, 0.0, h);
                self.ln(&t, -w, 0.0);
                self.w.out.close();
            }
            other if other != "rectangle" => {
                // any other shape: its background path at the size, from the node shapes, at its anchor
                let boxes = [("text".to_string(), crate::engine::shapes::BoxSize::default())];
                let (line_width, ex) = (self.e.st.line_width, 4.3 * PT);
                let mut keys = crate::engine::nodes::EngineKeys(self.e);
                let mut input = crate::engine::shapes::Input {
                    parts: &boxes,
                    // pgf's shape of an arbitrary size, then stretched to the decoration's
                    inner_xsep: 50.0 * PT,
                    inner_ysep: 50.0 * PT,
                    minimum_width: PT,
                    minimum_height: PT,
                    outer_xsep: 0.0,
                    outer_ysep: 0.0,
                    aspect: 1.0,
                    ex,
                    line_width,
                    pointer: None,
                    keys: &mut keys,
                };
                let Some(g) = crate::engine::shapes::compute(other, &mut input) else {
                    self.e.warn(format!("shape backgrounds: shape '{other}' not supported: drawn as a rectangle"));
                    self.rectangle(&t, w, h);
                    return;
                };
                // the path stretched to be as wide and high as the shape's size, as pgf scales the
                // background it measured
                let mut bounds = crate::canvas::Canvas::default();
                for path in g.paths.iter().filter(|p| matches!(p.layer, crate::engine::shapes::Layer::Background) && matches!(p.usage, crate::engine::shapes::Usage::Node)) {
                    bounds.include_path(&path.segments);
                }
                let stretch = match (bounds.min, bounds.max) {
                    (Some(lo), Some(hi)) if hi.x - lo.x > 1e-9 && hi.y - lo.y > 1e-9 => {
                        let c = g.center;
                        Transform::translation(c.x, c.y)
                            .compose(&Transform::scaling(2.0 * w * PT / (hi.x - lo.x), 2.0 * h * PT / (hi.y - lo.y)))
                            .compose(&Transform::translation(-c.x, -c.y))
                    }
                    _ => Transform::default(),
                };
                let at = stretch.apply(g.anchor(&anchor).unwrap_or(g.center));
                // node coordinates are cm, the decoration's pt
                let m = base.compose(&Transform::scaling(1.0 / PT, 1.0 / PT)).compose(&Transform::translation(-at.x, -at.y)).compose(&stretch);
                for path in g.paths.iter().filter(|p| matches!(p.layer, crate::engine::shapes::Layer::Background) && matches!(p.usage, crate::engine::shapes::Usage::Node)) {
                    for seg in &path.segments {
                        match *seg {
                            crate::scene::Segment::MoveTo(q) => self.w.out.move_to(m.apply(q)),
                            crate::scene::Segment::LineTo(q) => self.w.out.line_to(m.apply(q)),
                            crate::scene::Segment::CurveTo(a, b, q) => self.w.out.curve_to(m.apply(a), m.apply(b), m.apply(q)),
                            crate::scene::Segment::Close => self.w.out.close(),
                        }
                    }
                }
            }
            _ => self.rectangle(&t, w, h),
        }
    }

    fn rectangle(&mut self, t: &Transform, w: f64, h: f64) {
        self.mv(t, -w, -h);
        self.ln(t, w, -h);
        self.ln(t, w, h);
        self.ln(t, -w, h);
        self.w.out.close();
    }

    // ------------------------------------------------------------ text along path

    fn text_width(&self) -> f64 {
        // no more text: a box 16383pt wide ends the decoration
        self.text.current.as_ref().map_or(16383.0, |c| c.2)
    }

    fn text_initial(&mut self) -> Result<(), Error> {
        let left = self.key_pt("text align/left indent", 0.0);
        let right = self.key_pt("text align/right indent", 0.0);
        let align = self.key_text("text align/align");
        self.w.remaining = self.w.section - right;
        self.w.section = self.w.remaining;
        // the width of the whole text, character by character
        let mut probe = TextState { rest: self.s.text.iter().cloned().collect(), ..Default::default() };
        let (mut width, mut chars, mut spaces) = (0.0, 0i64, 0i64);
        loop {
            self.scan_char(&mut probe)?;
            let Some(c) = &probe.current else { break };
            width += c.2;
            chars += 1;
            if probe.is_space {
                spaces += 1;
            }
            if chars > 100_000 {
                break;
            }
        }
        let mut tx = TextState { rest: self.s.text.iter().cloned().collect(), indent_left: left, fit: self.s.text_fit, stretch: self.s.text_stretch, ..Default::default() };
        let mut y = self.w.remaining;
        if tx.fit {
            y -= left + width;
            if tx.stretch {
                y /= spaces.max(1) as f64;
                tx.space_shift = y;
            } else {
                y /= (chars - 1).max(1) as f64;
                tx.char_shift = y;
            }
            if y < 0.0 {
                tx.fit = false;
                tx.stretch = false;
                tx.char_shift = 0.0;
                tx.space_shift = 0.0;
            }
        } else if align == "right" {
            tx.indent_left = y - width;
        } else if align == "center" {
            tx.indent_left = 0.5 * (y - width - left) + left;
        }
        self.text = tx;
        Ok(())
    }

    fn text_scan(&mut self) -> Result<(), Error> {
        let mut tx = std::mem::take(&mut self.text);
        let r = self.scan_char(&mut tx);
        self.text = tx;
        r
    }

    /// `\pgf@lib@dec@text@scanchar`: the next character's box.
    fn scan_char(&mut self, tx: &mut TextState) -> Result<(), Error> {
        tx.is_space = false;
        tx.current = None;
        let mut guard = 0;
        while let Some(t) = tx.rest.pop_front() {
            guard += 1;
            if guard > 10_000 {
                break;
            }
            match &t {
                Token::Char(_, crate::tex::Cat::Space) => {
                    tx.is_space = true;
                    tx.current = Some((" ".into(), tx.format.clone(), self.char_width(" ", &tx.format, tx.math)));
                    return Ok(());
                }
                Token::Char(_, crate::tex::Cat::Begin) => {
                    let mut depth = 1;
                    let mut inner = Vec::new();
                    while let Some(u) = tx.rest.pop_front() {
                        match u {
                            Token::Char(_, crate::tex::Cat::Begin) => depth += 1,
                            Token::Char(_, crate::tex::Cat::End) => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        inner.push(u);
                    }
                    let s = crate::tex::tokens_to_string(&inner);
                    let w = self.char_width(&s, &tx.format, tx.math);
                    tx.current = Some((s, tx.format.clone(), w));
                    return Ok(());
                }
                Token::Char('$', _) => tx.math = !tx.math,
                Token::Char(c, _) if *c == self.s.text_format.0 => {
                    // |format| or |+format| up to the closing delimiter
                    let close = self.s.text_format.1.unwrap_or(self.s.text_format.0);
                    let mut f = Vec::new();
                    while let Some(u) = tx.rest.pop_front() {
                        if matches!(u, Token::Char(c, _) if c == close) {
                            break;
                        }
                        f.push(u);
                    }
                    let s = crate::tex::tokens_to_string(&f);
                    match s.strip_prefix('+') {
                        Some(add) => tx.format.push_str(add),
                        None => tx.format = s,
                    }
                }
                Token::Char(c, _) => {
                    let s = c.to_string();
                    let w = self.char_width(&s, &tx.format, tx.math);
                    tx.current = Some((s, tx.format.clone(), w));
                    return Ok(());
                }
                Token::Cs(name) if &**name == " " || &**name == "space" => {
                    tx.current = Some((" ".into(), tx.format.clone(), self.char_width(" ", &tx.format, tx.math)));
                    return Ok(());
                }
                Token::Cs(_) => {
                    let m = self.e.tex.meaning(&t);
                    if m.is_expandable() {
                        // expanded once, then scanned again
                        let depth = self.e.tex.depth();
                        self.e.tex.push_frame(vec![t.clone()]);
                        let mut out = Vec::new();
                        let x = self.e.tex.get_token_until(depth).expect("the token pushed just before");
                        self.e.tex.expand(&x, &m)?;
                        while let Some(x) = self.e.tex.get_token_until(depth) {
                            out.push(x);
                        }
                        self.e.tex.drain_to(depth);
                        for x in out.into_iter().rev() {
                            tx.rest.push_front(x);
                        }
                    } else {
                        let s = crate::tex::show_token(&t);
                        let w = self.char_width(&s, &tx.format, tx.math);
                        tx.current = Some((s, tx.format.clone(), w));
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// The label of one character, with the format's color (`\color{…}`) taken out.
    fn char_label(&mut self, text: &str, format: &str, math: bool) -> crate::scene::Label {
        let (font, (lets, color)) = split_color(format);
        // the format's `\colorlet`s hold for its `\color`, and only there
        let saved = (!lets.is_empty()).then(|| self.e.colors.clone());
        for (name, spec) in lets {
            let spec = self.expand_spec(spec);
            if let Some(c) = self.e.color(&spec) {
                self.e.colors.insert(name, c);
            }
        }
        let color = color.map(|(model, spec)| (model, self.expand_spec(spec)));
        let color = color
            .and_then(|(model, spec)| model.map_or_else(|| self.e.color(&spec), |m| crate::color::define(&m, &spec)))
            .or_else(|| self.e.color(&self.s.text_color.clone()))
            .unwrap_or(Color::BLACK);
        if let Some(colors) = saved {
            self.e.colors = colors;
        }
        let body = if math { format!("${text}$") } else { text.to_string() };
        let mut warnings = Vec::new();
        let label = crate::label::compose_label(&body, &font, None, 'l', color, &mut warnings);
        for w in warnings {
            self.e.warn(w);
        }
        label
    }

    /// A color spec with its macros expanded, as xcolor reads it (`\color\mycolor`).
    fn expand_spec(&mut self, spec: String) -> String {
        if !spec.contains('\\') {
            return spec;
        }
        match self.e.tex.expand_fully(crate::engine::tikra_tokens(&spec)) {
            Ok(t) => crate::tex::tokens_to_string(&t).trim().to_string(),
            Err(_) => spec,
        }
    }

    fn char_width(&mut self, text: &str, format: &str, math: bool) -> f64 {
        if text == " " {
            // an interword space of the 10pt font
            return 3.33333 * crate::label::font_size(format);
        }
        self.char_label(text, format, math).width() / PT
    }

    fn text_typeset(&mut self, t: &Transform) {
        let Some((s, format, w)) = self.text.current.clone() else { return };
        if s == " " {
            return;
        }
        let math = self.text.math;
        let label = self.char_label(&s, &format, math);
        let place = to_cm(&shifted(t, -0.5 * w, 0.0));
        // as \pgfqboxsynced, the box does not count in the picture's size
        self.e.canvas.push(Item::Text { label, place, opacity: self.e.st.text_opacity });
    }
}

/// `.25cm, between centers` → ("0.25cm", "between centers").
fn split_keyword(s: &str) -> (String, String) {
    match s.split_once(',') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (s.trim().to_string(), String::new()),
    }
}

/// The colors a text format sets.
type FormatColors = (Vec<(String, String)>, Option<(Option<String>, String)>);

/// A text format without its `\colorlet{name}{…}` and `\color[model]{…}`, then the colors the
/// `\colorlet`s define (name and spec, in order), and the color: its model (if given) and spec.
fn split_color(format: &str) -> (String, FormatColors) {
    let mut font = String::new();
    let mut lets = Vec::new();
    let mut color = None;
    let mut rest = format;
    while let Some(i) = rest.find("\\color") {
        font.push_str(&rest[..i]);
        rest = &rest[i..];
        let after = &rest[6..];
        let letters = after.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(after.len());
        let word = &after[..letters];
        if !matches!(word, "" | "let") {
            // `\colorbox`, `\colorlets`…: another command
            font.push_str(&rest[..6 + letters]);
            rest = &after[letters..];
            continue;
        }
        let mut after = after[letters..].trim_start();
        let mut model = None;
        if let Some((m, b)) = after.strip_prefix('[').and_then(|a| a.split_once(']')) {
            model = Some(m.trim().to_string());
            after = b;
        }
        if word == "let" {
            let Some((name, b)) = format_argument(after) else { break };
            let Some((spec, b)) = format_argument(b) else { break };
            lets.push((name.trim().to_string(), spec.to_string()));
            rest = b;
            continue;
        }
        let Some((spec, b)) = format_argument(after) else { break };
        color = Some((model, spec.to_string()));
        rest = b;
    }
    // what is left after a `\color` missing its argument is dropped with it
    if !rest.starts_with("\\color") {
        font.push_str(rest);
    }
    (font, (lets, color))
}

/// A command's argument in a text format and what follows it: a braced group, or else one token
/// (`\color\mycolor`, a control word or a character).
fn format_argument(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if let Some(b) = s.strip_prefix('{') {
        let j = b.find('}')?;
        return Some((&b[..j], &b[j + 1..]));
    }
    let first = s.chars().next()?;
    let n = if first == '\\' {
        let letters = s[1..].find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(s.len() - 1);
        1 + if letters > 0 { letters } else { s[1..].chars().next().map_or(0, char::len_utf8) }
    } else {
        first.len_utf8()
    };
    Some((&s[..n], &s[n..]))
}
