//! Arrow tips as pgf draws them (`pgfcorearrows`, the `arrows.meta`, `arrows` and `arrows.spaced`
//! libraries): reading specifications (`->`, `<->`, `-{Latex[length=3mm]}`, `|<->|`, `<<.<-`), the
//! geometry of each tip with its keys, how far the line is pulled back into the tips, and the drawing.
//!
//! Geometry is in TeX points in the tip's own frame, as pgf writes it: the tip points right along x,
//! y is to its left. Only the placement converts to the canvas (cm).

use std::collections::HashMap;

use crate::canvas::{Cap, Fill, Item, Join, Paint, Stroke};
use crate::color::Color;
use crate::scene::{Point, Segment, Transform};
use crate::PT;

/// `<start>-<end>`: the tips at each end (None for none). None if the text is not an arrow specification.
pub fn parse_arrows(spec: &str, user: &UserTips) -> Option<(Option<String>, Option<String>)> {
    let c: Vec<char> = spec.trim().chars().collect();
    let mut level = 0;
    let mut cut = None;
    for (i, ch) in c.iter().enumerate() {
        match ch {
            '{' | '[' => level += 1,
            '}' | ']' => level -= 1,
            '-' if level == 0 => {
                cut = Some(i);
                break;
            }
            _ => {}
        }
    }
    let i = cut?;
    let a: String = c[..i].iter().collect();
    let b: String = c[i + 1..].iter().collect();
    let side = |s: &str, start: bool| -> Option<Option<String>> {
        let s = s.trim();
        if s.is_empty() {
            return Some(None);
        }
        sequence(s, start, user, 0).map(|_| Some(s.to_string()))
    };
    // (with no start, the text starts with the `-`: an arrow specification either way)
    let start = side(&a, true)?;
    let end = side(&b, false)?;
    Some((start, end))
}

/// Arrow tips declared in the document (`name/.tip=…`, `>=…`): what each end name means, and the end
/// name each start name stands for. Kept in the graphic state, so a group ends them, as in pgf.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserTips {
    pub means: HashMap<String, String>,
    pub starts: HashMap<String, String>,
}

impl UserTips {
    /// `\pgfdeclarearrow{name=…, means=…}`: `name` is `start-end`, or one name for both ends.
    pub fn declare(&mut self, name: &str, means: &str) {
        let name = name.trim();
        let (start, end) = match name.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (name, name),
        };
        self.starts.insert(start.to_string(), end.to_string());
        self.means.insert(end.to_string(), strip_group(means.trim()).to_string());
    }
}

/// When tips are drawn (`tips=`): pgf's `\pgf@tips@mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TipsMode {
    Never,
    /// on stroked paths (the default)
    #[default]
    OnDraw,
    /// also on paths that are neither stroked nor filled
    True,
    /// on stroked paths that do not stay at one point
    OnProperDraw,
    Proper,
}

impl TipsMode {
    pub fn from_name(s: &str) -> Option<TipsMode> {
        Some(match s.trim() {
            "" | "true" => TipsMode::True,
            "on draw" => TipsMode::OnDraw,
            "never" | "false" => TipsMode::Never,
            "on proper draw" => TipsMode::OnProperDraw,
            "proper" => TipsMode::Proper,
            _ => return None,
        })
    }
}

// ------------------------------------------------------------------------------------------ names

/// A kind of tip with drawing code: those of `arrows.meta`, and the older ones.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    StraightBarb,
    Hooks,
    ArcBarb,
    TeeBarb,
    ClassicalRightarrow,
    CmRightarrow,
    Implies,
    Latex,
    Stealth,
    Kite,
    Square,
    Circle,
    RoundCap,
    ButtCap,
    TriangleCap,
    FastTriangle,
    FastRound,
    Rays,
    /// CircuiTikZ's `Jack Tap`: a step then half a triangle
    JackTap,
    /// `_`: only space
    Underscore,
    Old(Old),
}

/// The tips declared with `\pgfarrowsdeclare` (pgf's core and the `arrows` library).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Old {
    Stealth,
    To,
    ToReversed,
    Latex,
    Bar,
    Space,
    SquareBracket,
    Parenthesis,
    Angle(u8),
    Dot,
    Ring,
    Diamond,
    OpenDiamond,
    Square,
    OpenSquare,
    Triangle(u8),
    OpenTriangle(u8),
    OpenTriangleReversed(u8),
    LatexPrime,
    StealthPrime,
    LeftTo,
    RightTo,
    LeftToReversed,
    RightToReversed,
    LeftHook,
    RightHook,
    Hooks,
    SerifCm,
    RoundCap,
    ButtCap,
    Triangle90Cap,
    Triangle90CapReversed,
    FastCap,
    FastCapReversed,
    Implies,
    /// the circuits' `direction ee`, as long as its `length` (`\pgfsetarrowoptions`)
    DirectionEe,
    /// one the document declared with `\pgfarrowsdeclare`: its place among them
    User(u16),
}

/// A tip the document declared with `\pgfarrowsdeclare{start}{end}{extents}{code}`: its names and
/// what its code drew (pt, the tip pointing right with its origin where the line ends).
#[derive(Debug, Clone)]
pub struct DeclaredTip {
    pub names: (String, String),
    pub left: f64,
    pub right: f64,
    /// each path, whether it is filled and stroked, and its line width
    pub paths: Vec<(Vec<Segment>, bool, bool, f64)>,
}

thread_local! {
    /// The tips the document declared, for the figure being drawn.
    static DECLARED: std::cell::RefCell<Vec<DeclaredTip>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Forgets the tips declared for another figure.
pub fn clear_declared() {
    DECLARED.with(|d| d.borrow_mut().clear());
}

/// Keeps a tip the document declared (a later one of a name wins).
pub fn declare_tip(tip: DeclaredTip) {
    DECLARED.with(|d| d.borrow_mut().push(tip));
}

fn declared(name: &str) -> Option<Kind> {
    DECLARED.with(|d| d.borrow().iter().rposition(|t| t.names.0 == name || t.names.1 == name)).map(|i| Kind::Old(Old::User(i as u16)))
}

fn code(name: &str) -> Option<Kind> {
    use Kind::*;
    let angle = |s: &str| match s {
        "90" => Some(90),
        "60" => Some(60),
        "45" => Some(45),
        _ => None,
    };
    Some(match name {
        "Straight Barb" => StraightBarb,
        "Hooks" => Hooks,
        "Arc Barb" => ArcBarb,
        "Tee Barb" => TeeBarb,
        "Classical TikZ Rightarrow" => ClassicalRightarrow,
        "Computer Modern Rightarrow" => CmRightarrow,
        "Implies" => Implies,
        "Latex" => Latex,
        "Stealth" => Stealth,
        "Kite" => Kite,
        "Square" => Square,
        "Circle" => Circle,
        "Round Cap" => RoundCap,
        "Butt Cap" => ButtCap,
        "Triangle Cap" => TriangleCap,
        "Fast Triangle" => FastTriangle,
        "Fast Round" => FastRound,
        "Rays" => Rays,
        "Jack Tap" => JackTap,
        "_" => Underscore,
        name if declared(name).is_some() => return declared(name),
        "stealth" => Old(self::Old::Stealth),
        "to" => Old(self::Old::To),
        "to reversed" => Old(self::Old::ToReversed),
        "latex" => Old(self::Old::Latex),
        "@bar" => Old(self::Old::Bar),
        "space" => Old(self::Old::Space),
        "square bracket" => Old(self::Old::SquareBracket),
        ")" => Old(self::Old::Parenthesis),
        "*" => Old(self::Old::Dot),
        "o" => Old(self::Old::Ring),
        "diamond" => Old(self::Old::Diamond),
        "open diamond" => Old(self::Old::OpenDiamond),
        "square" => Old(self::Old::Square),
        "open square" => Old(self::Old::OpenSquare),
        "latex'" => Old(self::Old::LatexPrime),
        "stealth'" => Old(self::Old::StealthPrime),
        "left to" => Old(self::Old::LeftTo),
        "right to" => Old(self::Old::RightTo),
        "left to reversed" => Old(self::Old::LeftToReversed),
        "right to reversed" => Old(self::Old::RightToReversed),
        "left hook" => Old(self::Old::LeftHook),
        "right hook" => Old(self::Old::RightHook),
        "hooks" => Old(self::Old::Hooks),
        "serif cm" => Old(self::Old::SerifCm),
        "round cap" => Old(self::Old::RoundCap),
        "butt cap" => Old(self::Old::ButtCap),
        "triangle 90 cap" => Old(self::Old::Triangle90Cap),
        "triangle 90 cap reversed" => Old(self::Old::Triangle90CapReversed),
        "fast cap" => Old(self::Old::FastCap),
        "fast cap reversed" => Old(self::Old::FastCapReversed),
        "implies" => Old(self::Old::Implies),
        "tikra direction ee" => Old(self::Old::DirectionEe),
        _ => {
            if let Some(a) = name.strip_prefix("angle ").and_then(angle) {
                Old(self::Old::Angle(a))
            } else if let Some(a) = name.strip_prefix("triangle ").and_then(angle) {
                Old(self::Old::Triangle(a))
            } else {
                let r = name.strip_prefix("open triangle ")?;
                match r.strip_suffix(" reversed") {
                    Some(a) => Old(self::Old::OpenTriangleReversed(angle(a)?)),
                    None => Old(self::Old::OpenTriangle(angle(r)?)),
                }
            }
        }
    })
}

/// Tips whose reversed form is declared with `\pgfarrowsdeclarereversed` (`stealth reversed`…).
const REVERSIBLE: [&str; 13] =
    ["stealth", "latex", "angle 90", "angle 60", "angle 45", "triangle 90", "triangle 60", "triangle 45", "latex'", "stealth'", "left hook", "right hook", "hooks"];

enum Def {
    Code(Kind),
    Means(String),
}

/// What an end name stands for: drawing code, or a shorthand for other tips.
fn definition(name: &str, user: &UserTips, depth: usize) -> Option<Def> {
    if let Some(m) = user.means.get(name) {
        return Some(Def::Means(m.clone()));
    }
    if let Some(k) = code(name) {
        return Some(Def::Code(k));
    }
    let m = match name {
        // TikZ's `>` (redefined by `>=`) and pgf's own shorthands
        ">" => "To",
        "<" => ">[reversed]",
        "|" => "Bar",
        ">|" => ">[sep=0pt].|",
        // arrows.meta's
        "To" => "Computer Modern Rightarrow",
        "Bar" => "Tee Barb[length=+0pt]",
        "LaTeX" => "Latex",
        "Triangle" => "Stealth[inset=+0pt, angle=+60:+2.7pt +3.6]",
        "Rectangle" => "Square[length=+3pt +4.5,width'=+0pt +.5]",
        "Ellipse" => "Circle[length=+3.3pt +4.95, width'=+0pt +0.5]",
        "Diamond" => "Kite[inset'=+0pt .5]",
        "Turned Square" => "Kite[length=+3pt 4,width'= +0pt 1,inset'= +0pt 0.5]",
        "Bracket" => "Tee Barb[inset'=+0pt +1,length=+0.75pt +1]",
        "Parenthesis" => "Arc Barb[arc=+120,length=+1.725pt +2.3]",
        "direction ee" => "tikra direction ee[length=4pt]",
        // the arrows library's brackets
        "]" => "square bracket",
        "[" => "][reversed]",
        "(" => ")[reversed]",
        "spaced [" => "square bracket[reversed].space",
        "spaced ]" => "square bracket[].space",
        _ => {
            if let Some(base) = name.strip_suffix(" reversed").filter(|b| REVERSIBLE.contains(b)) {
                return Some(Def::Means(format!("{base}[reversed]")));
            }
            // arrows.spaced: the tip, then a space
            if let Some(base) = name.strip_prefix("spaced ") {
                if depth < 4 && definition(base, user, depth + 1).is_some() {
                    return Some(Def::Means(format!("{base}[sep=0pt].space")));
                }
            }
            return None;
        }
    };
    Some(Def::Means(m.to_string()))
}

/// The end name a start name stands for (`<` is the start of `>`).
fn start_to_end(name: &str, user: &UserTips) -> Option<String> {
    if let Some(e) = user.starts.get(name) {
        return Some(e.clone());
    }
    let e = match name {
        "<" => ">",
        ">" => "<",
        "(" => ")",
        ")" => "(",
        "[" => "]",
        "]" => "[",
        "|<" => ">|",
        "spaced (" => "spaced )",
        "spaced )" => "spaced (",
        "spaced [" => "spaced ]",
        "spaced ]" => "spaced [",
        ">|" => return None,
        _ => return definition(name, user, 0).map(|_| name.to_string()),
    };
    Some(e.to_string())
}

// ---------------------------------------------------------------------------------------- parsing

/// An entry of a tip sequence: a tip with its option lists (its own, then those of the shorthands
/// it came from), or `.`, where the line ends.
#[derive(Debug, Clone, PartialEq)]
enum Raw {
    Tip { kind: Kind, opts: Vec<String> },
    Dot,
}

/// One level of braces around the whole text, as TeX strips them from an argument.
fn strip_group(s: &str) -> &str {
    if s.starts_with('{') && s.ends_with('}') {
        let mut level = 0;
        for (i, ch) in s.char_indices() {
            match ch {
                '{' => level += 1,
                '}' => {
                    level -= 1;
                    if level == 0 && i != s.len() - 1 {
                        return s;
                    }
                }
                _ => {}
            }
        }
        return s[1..s.len() - 1].trim();
    }
    s
}

/// The text inside the brackets that open at `c[i]`, and the index after them.
fn bracket(c: &[char], i: usize) -> Option<(String, usize)> {
    let mut level = 0;
    for (j, &ch) in c.iter().enumerate().skip(i) {
        match ch {
            '[' | '{' => level += 1,
            ']' | '}' => {
                level -= 1;
                if level == 0 {
                    return Some((c[i + 1..j].iter().collect(), j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// The tips of a specification, from the back of the arrow to its point, as pgf's parser reads them
/// (`\pgfarrows@parser`): a name up to the options is a tip or a shorthand; otherwise its first
/// character is. At the start of a path names are those of starts, and each tip goes before the
/// ones already read. None if a name is unknown.
fn sequence(spec: &str, start: bool, user: &UserTips, depth: usize) -> Option<Vec<Raw>> {
    if depth > 12 {
        return None;
    }
    let s = strip_group(spec.trim());
    // brackets alone cannot be read as names before options: pgf has them directly
    if matches!(s, "[" | "]" | "spaced [" | "spaced ]") {
        let end = if start { start_to_end(s, user)? } else { s.to_string() };
        return expand(definition(&end, user, 0)?, Vec::new(), user, depth);
    }
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut initial = None;
    if c.first() == Some(&'[') {
        let (o, j) = bracket(&c, 0)?;
        initial = Some(o).filter(|o| !o.trim().is_empty());
        i = j;
    }
    let resolve = |name: &str| -> Option<Def> {
        let end = if start { start_to_end(name, user)? } else { name.to_string() };
        definition(&end, user, 0)
    };
    let mut out: Vec<Raw> = Vec::new();
    let push = |out: &mut Vec<Raw>, block: Vec<Raw>| {
        if start {
            out.splice(0..0, block);
        } else {
            out.extend(block);
        }
    };
    while i < c.len() {
        if c[i].is_whitespace() {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut level = 0;
        while j < c.len() && !(c[j] == '[' && level == 0) {
            match c[j] {
                '{' => level += 1,
                '}' => level -= 1,
                _ => {}
            }
            j += 1;
        }
        let name: String = c[i..j].iter().collect::<String>().trim().to_string();
        let (local, next) = if j < c.len() { bracket(&c, j)? } else { (String::new(), c.len()) };
        let mut opts: Vec<String> = initial.iter().cloned().collect();
        if name == "." {
            push(&mut out, vec![Raw::Dot]);
            i = next;
            continue;
        }
        if name.is_empty() {
            i = next;
            continue;
        }
        if let Some(def) = resolve(strip_group(&name)) {
            if !local.trim().is_empty() {
                opts.push(local);
            }
            push(&mut out, expand(def, opts, user, depth)?);
            i = next;
            continue;
        }
        // a one-character tip; the rest is read again
        let first = name.chars().next()?.to_string();
        if first == "." {
            push(&mut out, vec![Raw::Dot]);
        } else {
            push(&mut out, expand(resolve(&first)?, opts, user, depth)?);
        }
        i += first.len().max(1);
    }
    Some(out)
}

/// The tips of a definition with options: a shorthand's tips get them after their own.
fn expand(def: Def, opts: Vec<String>, user: &UserTips, depth: usize) -> Option<Vec<Raw>> {
    match def {
        Def::Code(kind) => Some(vec![Raw::Tip { kind, opts }]),
        Def::Means(m) => {
            let mut seq = sequence(&m, false, user, depth + 1)?;
            for r in &mut seq {
                if let Raw::Tip { opts: o, .. } = r {
                    o.extend(opts.iter().cloned());
                }
            }
            Some(seq)
        }
    }
}

// ---------------------------------------------------------------------------------------- keys

/// Evaluates the values of arrow keys: lengths (in cm), numbers and colors.
pub trait Eval {
    fn dim(&mut self, text: &str) -> Option<f64>;
    fn num(&mut self, text: &str) -> Option<f64>;
    fn paint(&mut self, text: &str) -> Option<Color>;
}

impl Eval for super::Engine {
    fn dim(&mut self, text: &str) -> Option<f64> {
        self.length(text).ok()
    }
    fn num(&mut self, text: &str) -> Option<f64> {
        self.number(text).ok()
    }
    fn paint(&mut self, text: &str) -> Option<Color> {
        // pgf's names for the path's current colors (`fill=pgffillcolor`)
        match text.trim() {
            "pgffillcolor" => Some(self.st.fill_paint_color()),
            "pgfstrokecolor" => Some(self.st.stroke_color()),
            t => self.color(t),
        }
    }
}

/// Plain values only (`2pt`, `.5`, color names): for callers without an engine.
pub struct Plain;

impl Eval for Plain {
    fn dim(&mut self, _: &str) -> Option<f64> {
        None
    }
    fn num(&mut self, _: &str) -> Option<f64> {
        None
    }
    fn paint(&mut self, text: &str) -> Option<Color> {
        crate::color::parse(text, &HashMap::new())
    }
}

/// `3pt`, `+2mm`, `-.5cm` in points, without an expression evaluator.
fn plain_dim(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches('+');
    let units = [("pt", 1.0), ("mm", 72.27 / 25.4), ("cm", 72.27 / 2.54), ("in", 72.27), ("bp", 72.27 / 72.0), ("sp", 1.0 / 65536.0)];
    for (u, k) in units {
        if let Some(n) = s.strip_suffix(u) {
            return n.trim().parse::<f64>().ok().map(|v| v * k);
        }
    }
    None
}

fn plain_num(s: &str) -> Option<f64> {
    s.trim().trim_start_matches('+').parse().ok()
}

/// `dimension factor factor` (`+3pt 4.5 .8`) of the length keys.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Dep(f64, f64, f64);

/// An arrow key, evaluated (`/pgf/arrow keys/…`).
#[derive(Debug, Clone, PartialEq)]
enum Opt {
    Length(Dep),
    Width(Dep),
    /// `width'`: a part of the length
    WidthOfLength(Dep),
    Inset(Dep),
    InsetOfLength(Dep),
    LineWidth(Dep),
    LineWidthOfLength(Dep),
    /// `angle=A:…`: the length along the sides, and the width it gives (set late)
    Angle(f64, Dep),
    AngleWidth(f64),
    /// `angle'=A`: the width for the length
    AngleOfLength(f64),
    CapAngle(f64),
    Scale(f64),
    ScaleLength(f64),
    ScaleWidth(f64),
    Slant(f64),
    Sep(Dep),
    Swap,
    Reversed,
    Harpoon(bool),
    Color(Color),
    Open(bool),
    FillColor(Color),
    RoundCap(bool),
    RoundJoin(bool),
    Arc(f64),
    N(u32),
    Flex(Flex),
}

impl Opt {
    /// pgf runs these after all others (`\pgfarrowsaddtolateoptions`): they read the length.
    fn late(&self) -> bool {
        matches!(
            self,
            Opt::Width(_)
                | Opt::WidthOfLength(_)
                | Opt::Inset(_)
                | Opt::InsetOfLength(_)
                | Opt::LineWidth(_)
                | Opt::LineWidthOfLength(_)
                | Opt::AngleWidth(_)
                | Opt::AngleOfLength(_)
        )
    }
}

/// How a tip sits on a curved end (the bending library, which the manual loads: its
/// `arrows=[flex]` is the default here).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flex {
    /// along the tangent at the end of the path
    Quick,
    /// turned so that its visual ends lie on the path, with this rigidity
    Flex(f64),
    /// the same with its tip and back ends
    FlexPrime(f64),
    /// bent along the path: drawn flexed here
    Bend,
}

/// The values of the arrow keys for one tip, in pt.
#[derive(Debug, Clone, PartialEq)]
struct Keys {
    length: f64,
    width: f64,
    inset: f64,
    line_width: f64,
    sep: f64,
    scale_length: f64,
    scale_width: f64,
    slant: f64,
    swap: bool,
    reversed: bool,
    harpoon: bool,
    open: bool,
    round_cap: bool,
    round_join: bool,
    color: Option<Color>,
    fill: Option<Color>,
    arc: f64,
    n: u32,
    flex: Flex,
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            length: 0.0,
            width: 0.0,
            inset: 0.0,
            line_width: 0.0,
            sep: 0.0,
            scale_length: 1.0,
            scale_width: 1.0,
            slant: 0.0,
            swap: false,
            reversed: false,
            harpoon: false,
            open: false,
            round_cap: false,
            round_join: false,
            color: None,
            fill: None,
            arc: 180.0,
            n: 4,
            flex: Flex::Flex(1.0),
        }
    }
}

/// The path's line widths (pt): pgf's `\pgflinewidth` and, for `double`, `\pgfinnerlinewidth`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    pub line: f64,
    pub inner: f64,
}

impl Widths {
    /// `\pgfarrowslinewidthdependent`: a length plus a factor of the line width (of the visible
    /// line, between the inner and the outer one, for double lines).
    fn dep(&self, d: Dep) -> f64 {
        if self.inner > 0.0 {
            d.0 + d.1 * (self.line * (1.0 - d.2 / 2.0) - d.2 / 2.0 * self.inner)
        } else {
            d.0 + d.1 * self.line
        }
    }
}

impl Keys {
    fn apply(&mut self, o: &Opt, lw: Widths, late: &mut Vec<Opt>) {
        let of_length = |k: &Keys, d: Dep| d.0 + d.1 * k.length + d.2 * lw.line;
        match *o {
            Opt::Length(d) => self.length = lw.dep(d),
            Opt::Width(d) => self.width = lw.dep(d),
            Opt::WidthOfLength(d) => self.width = of_length(self, d),
            Opt::Inset(d) => self.inset = lw.dep(d),
            Opt::InsetOfLength(d) => self.inset = of_length(self, d),
            Opt::LineWidth(d) => self.line_width = lw.dep(d),
            Opt::LineWidthOfLength(d) => self.line_width = of_length(self, d),
            Opt::Angle(a, d) => {
                let x = lw.dep(d);
                let (s, c) = (a / 2.0).to_radians().sin_cos();
                self.length = x * c;
                late.push(Opt::AngleWidth(2.0 * x * s));
            }
            Opt::AngleWidth(w) => self.width = w,
            Opt::AngleOfLength(a) => self.width = 2.0 * (a / 2.0).to_radians().tan() * self.length,
            Opt::CapAngle(a) => self.length = 0.5 / (a / 2.0).to_radians().tan() * lw.line,
            Opt::Scale(s) => {
                self.scale_length = s;
                self.scale_width = s;
            }
            Opt::ScaleLength(s) => self.scale_length = s,
            Opt::ScaleWidth(s) => self.scale_width = s,
            Opt::Slant(s) => self.slant = s,
            Opt::Sep(d) => self.sep = lw.dep(d),
            Opt::Swap => self.swap = !self.swap,
            Opt::Reversed => self.reversed = !self.reversed,
            Opt::Harpoon(h) => self.harpoon = h,
            Opt::Color(c) => self.color = Some(c),
            Opt::Open(o) => self.open = o,
            Opt::FillColor(c) => {
                self.open = false;
                self.fill = Some(c);
            }
            Opt::RoundCap(r) => self.round_cap = r,
            Opt::RoundJoin(r) => self.round_join = r,
            Opt::Arc(a) => self.arc = a,
            Opt::N(n) => self.n = n.max(1),
            Opt::Flex(f) => self.flex = f,
        }
    }

    /// The keys of a tip, as `\pgfarrows@getid` runs them: the tip's defaults, the given options,
    /// the late options, then the scaling.
    fn of(kind: Kind, opts: &[Opt], lw: Widths) -> Keys {
        let mut k = Keys::default();
        let mut late = Vec::new();
        for o in defaults(kind).iter().chain(opts) {
            if o.late() {
                late.push(o.clone());
            } else {
                k.apply(o, lw, &mut late);
            }
        }
        let mut more = Vec::new();
        for o in &late {
            k.apply(o, lw, &mut more);
        }
        k.length *= k.scale_length;
        k.inset *= k.scale_length;
        k.width *= k.scale_width;
        k
    }
}

/// `defaults=` of each tip of arrows.meta.
fn defaults(kind: Kind) -> Vec<Opt> {
    use Opt::*;
    let lw = LineWidth(Dep(0.0, 1.0, 1.0));
    let round = [RoundCap(true), RoundJoin(true)];
    match kind {
        Kind::StraightBarb | Kind::ArcBarb => vec![Length(Dep(1.5, 2.0, 0.0)), WidthOfLength(Dep(0.0, 2.0, 0.0)), lw],
        Kind::Hooks => vec![Length(Dep(0.75, 1.25, 0.0)), WidthOfLength(Dep(0.0, 4.0, -1.0)), lw],
        Kind::TeeBarb => vec![Length(Dep(1.5, 2.0, 0.0)), Width(Dep(3.0, 4.0, 0.0)), InsetOfLength(Dep(0.0, 0.5, 0.0)), lw],
        Kind::ClassicalRightarrow => [vec![Length(Dep(1.05, 1.925, 0.0)), WidthOfLength(Dep(0.0, 1.9237, 0.0)), LineWidth(Dep(0.0, 0.8, 1.0))], round.to_vec()].concat(),
        Kind::CmRightarrow => [vec![Length(Dep(1.6, 2.2, 0.0)), WidthOfLength(Dep(0.0, 2.096774, 0.0)), lw], round.to_vec()].concat(),
        Kind::Latex => vec![Length(Dep(3.0, 4.5, 0.8)), WidthOfLength(Dep(0.0, 0.75, 0.0)), lw],
        Kind::Stealth => vec![Length(Dep(3.0, 4.5, 0.8)), WidthOfLength(Dep(0.0, 0.75, 0.0)), InsetOfLength(Dep(0.0, 0.325, 0.0)), lw],
        Kind::Kite => vec![Length(Dep(3.6, 5.4, 0.0)), WidthOfLength(Dep(0.0, 0.5, 0.0)), InsetOfLength(Dep(0.0, 0.25, 0.0)), lw],
        Kind::Square => vec![Length(Dep(2.12132, 2.828427, 0.0)), WidthOfLength(Dep(0.0, 1.0, 0.0)), lw],
        Kind::Circle => vec![Length(Dep(2.39365, 3.191538, 0.0)), WidthOfLength(Dep(0.0, 1.0, 0.0)), lw],
        Kind::RoundCap | Kind::ButtCap | Kind::TriangleCap => vec![Length(Dep(0.0, 0.5, 0.0))],
        Kind::FastTriangle | Kind::FastRound => vec![Length(Dep(0.0, 0.5, 0.0)), Inset(Dep(0.0, 0.5, 0.0))],
        Kind::Rays => vec![Length(Dep(3.0, 4.0, 0.0)), WidthOfLength(Dep(0.0, 1.0, 0.0)), lw],
        // 0.3cm by 0.15cm whatever the line width, open
        Kind::JackTap => vec![Length(Dep(8.53583, 0.0, 0.0)), Width(Dep(4.26791, 0.0, 0.0)), lw, Open(true)],
        Kind::Underscore => vec![Sep(Dep(0.88, 0.3, 1.0))],
        Kind::Implies | Kind::Old(_) => Vec::new(),
    }
}

/// Splits at commas outside braces and brackets.
fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut level = 0;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '{' | '[' | '(' => level += 1,
            '}' | ']' | ')' => level -= 1,
            ',' if level == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Reads arrow keys; unknown ones go to `warn`.
fn parse_opts(ev: &mut dyn Eval, text: &str, warn: &mut Vec<String>) -> Vec<Opt> {
    let mut out = Vec::new();
    let dim = |ev: &mut dyn Eval, s: &str| -> f64 { plain_dim(s).or_else(|| ev.dim(s).map(|v| v / PT)).unwrap_or(0.0) };
    let num = |ev: &mut dyn Eval, s: &str| -> f64 { plain_num(s).or_else(|| ev.num(s)).unwrap_or(0.0) };
    for item in split_list(text) {
        let (key, value) = match item.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), Some(strip_group(v.trim()).to_string())),
            None => (item.clone(), None),
        };
        let v = value.clone().unwrap_or_default();
        // `\pgfarrowsthreeparameters`: a dimension, then two factors
        let three = |ev: &mut dyn Eval, s: &str| -> Dep {
            let mut parts = s.split_whitespace();
            let d = parts.next().map(|p| dim(ev, p)).unwrap_or(0.0);
            let a = parts.next().map(|p| num(ev, p)).unwrap_or(0.0);
            let b = parts.next().map(|p| num(ev, p)).unwrap_or(0.0);
            Dep(d, a, b)
        };
        let o = match key.as_str() {
            "length" => Opt::Length(three(ev, &v)),
            "width" => Opt::Width(three(ev, &v)),
            "width'" => Opt::WidthOfLength(three(ev, &v)),
            "inset" => Opt::Inset(three(ev, &v)),
            "inset'" => Opt::InsetOfLength(three(ev, &v)),
            "line width" => Opt::LineWidth(three(ev, &v)),
            "line width'" => Opt::LineWidthOfLength(three(ev, &v)),
            "sep" => Opt::Sep(if value.is_some() { three(ev, &v) } else { Dep(0.88, 0.3, 1.0) }),
            "angle" => {
                let (a, rest) = v.split_once(':').unwrap_or((&v, ""));
                Opt::Angle(num(ev, a), three(ev, rest))
            }
            "angle'" => Opt::AngleOfLength(num(ev, &v)),
            "cap angle" => Opt::CapAngle(num(ev, &v)),
            "scale" => Opt::Scale(num(ev, &v)),
            "scale length" => Opt::ScaleLength(num(ev, &v)),
            "scale width" => Opt::ScaleWidth(num(ev, &v)),
            "slant" => Opt::Slant(num(ev, &v)),
            "swap" => Opt::Swap,
            "reversed" => Opt::Reversed,
            "harpoon" => Opt::Harpoon(v != "false"),
            "left" => Opt::Harpoon(true),
            "right" => {
                out.push(Opt::Harpoon(true));
                Opt::Swap
            }
            "open" => Opt::Open(true),
            "fill" => match v.as_str() {
                "none" => Opt::Open(true),
                "" => Opt::Open(false),
                c => match ev.paint(c) {
                    Some(c) => Opt::FillColor(c),
                    None => {
                        warn.push(format!("arrow tip fill '{c}' is not a color: ignored"));
                        continue;
                    }
                },
            },
            "color" => match ev.paint(&v) {
                Some(c) => Opt::Color(c),
                None => {
                    warn.push(format!("arrow tip color '{v}' is not a color: ignored"));
                    continue;
                }
            },
            "line cap" => Opt::RoundCap(v == "round"),
            "line join" => Opt::RoundJoin(v == "round"),
            "round" => {
                out.push(Opt::RoundCap(true));
                Opt::RoundJoin(true)
            }
            "sharp" => {
                out.push(Opt::RoundCap(false));
                Opt::RoundJoin(false)
            }
            "arc" => Opt::Arc(num(ev, &v)),
            "n" => Opt::N(num(ev, &v).max(0.0) as u32),
            // the bending library's modes
            "quick" => Opt::Flex(Flex::Quick),
            "flex" => Opt::Flex(Flex::Flex(if value.is_some() { num(ev, &v) } else { 1.0 })),
            "flex'" => Opt::Flex(Flex::FlexPrime(if value.is_some() { num(ev, &v) } else { 1.0 })),
            "bend" => Opt::Flex(Flex::Bend),
            _ => match value.is_none().then(|| ev.paint(&key)).flatten() {
                Some(c) => Opt::Color(c),
                None => {
                    warn.push(format!("arrow key '{key}' not supported: ignored"));
                    continue;
                }
            },
        };
        out.push(o);
    }
    out
}

// ---------------------------------------------------------------------------------------- geometry

/// One path of a tip's drawing, in the tip's frame (pt).
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    segs: Vec<Segment>,
    fill: bool,
    stroke: bool,
    width: f64,
    /// None: the path's
    cap: Option<Cap>,
    join: Option<Join>,
}

/// A tip as pgf instantiates it: its ends along x (tip end, back end, line end), the visual ends
/// that a flexed tip puts on the path, the convex hull that counts for the bounding box, and the
/// drawing.
#[derive(Debug, Clone, PartialEq)]
struct Geometry {
    tip: f64,
    back: f64,
    line: f64,
    visual_tip: f64,
    visual_back: f64,
    hull: Vec<Point>,
    shapes: Vec<Shape>,
}

/// A path being built in the tip's frame.
#[derive(Default)]
struct Pen(Vec<Segment>);

impl Pen {
    fn m(&mut self, x: f64, y: f64) -> &mut Self {
        self.0.push(Segment::MoveTo(Point::new(x, y)));
        self
    }
    fn l(&mut self, x: f64, y: f64) -> &mut Self {
        self.0.push(Segment::LineTo(Point::new(x, y)));
        self
    }
    fn c(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) -> &mut Self {
        self.0.push(Segment::CurveTo(Point::new(x1, y1), Point::new(x2, y2), Point::new(x, y)));
        self
    }
    fn z(&mut self) -> &mut Self {
        self.0.push(Segment::Close);
        self
    }
    /// `\pgfpatharc`: from the current point, around the center that puts it at angle `a0`.
    fn arc(&mut self, a0: f64, a1: f64, rx: f64, ry: f64) -> &mut Self {
        let at = self.current();
        let (s, c) = a0.to_radians().sin_cos();
        let center = Point::new(at.x - rx * c, at.y - ry * s);
        self.0.extend(super::path::arc_segments(center, rx, ry, a0, a1, &Transform::default()));
        self
    }
    /// `\pgfpathellipse` with axes along x and y.
    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64) -> &mut Self {
        self.m(cx + rx, cy);
        self.arc(0.0, 360.0, rx, ry);
        self.z()
    }
    fn current(&self) -> Point {
        let Some(Segment::MoveTo(p) | Segment::LineTo(p) | Segment::CurveTo(_, _, p)) = self.0.last() else { return Point::default() };
        *p
    }
    fn take(&mut self) -> Vec<Segment> {
        std::mem::take(&mut self.0)
    }
}

fn map_segments(segs: &[Segment], f: impl Fn(Point) -> Point) -> Vec<Segment> {
    segs.iter()
        .map(|s| match *s {
            Segment::MoveTo(p) => Segment::MoveTo(f(p)),
            Segment::LineTo(p) => Segment::LineTo(f(p)),
            Segment::CurveTo(a, b, p) => Segment::CurveTo(f(a), f(b), f(p)),
            Segment::Close => Segment::Close,
        })
        .collect()
}

fn ratio(a: f64, b: f64) -> f64 {
    if b.abs() < 1e-9 {
        0.0
    } else {
        a / b
    }
}

/// The setup and drawing code of a tip, then its reversal (`\pgf@arrows@handle@reverse`).
fn geometry(kind: Kind, k: &Keys, lw: Widths) -> Geometry {
    let mut g = match kind {
        Kind::Old(o) => old(o, k.length, lw, k.reversed),
        _ => meta(kind, k, lw),
    };
    if k.reversed {
        let (t, b) = (g.tip, g.back);
        g.tip = -b;
        g.back = -t;
        g.line = -g.line;
        let (vt, vb) = (g.visual_tip, g.visual_back);
        g.visual_tip = -vb;
        g.visual_back = -vt;
        let flip = |p: Point| Point::new(-p.x, p.y);
        g.hull = g.hull.iter().map(|&p| flip(p)).collect();
        for s in &mut g.shapes {
            s.segs = map_segments(&s.segs, flip);
        }
    }
    g
}

/// The tips of arrows.meta.
fn meta(kind: Kind, k: &Keys, lw: Widths) -> Geometry {
    let (mut l, mut w, mut ins, mut alw) = (k.length, k.width, k.inset, k.line_width);
    let (h, rev, rc, mut rj) = (k.harpoon, k.reversed, k.round_cap, k.round_join);
    let mut hull: Vec<Point> = Vec::new();
    let upper = |hull: &mut Vec<Point>, x: f64, y: f64| {
        hull.push(Point::new(x, y));
        if y > 0.0 && !h {
            hull.push(Point::new(x, -y));
        }
    };
    let hy = |alw: f64, y: f64| if h { -0.5 * alw } else { y };
    let mut p = Pen::default();
    let (tip, back, line);
    // `\pgfarrowssetvisualbackend`, where the tip sets it (the back end otherwise)
    let mut visual_back = None;
    let mut shapes = Vec::new();
    let cap = |round: bool| Some(if round { Cap::Round } else { Cap::Butt });
    let join = |round: bool| Some(if round { Join::Round } else { Join::Miter });
    // the geometric tips: open, filled, or filled and stroked
    let closed = |segs: Vec<Segment>, alw: f64, rj: bool| Shape { segs, fill: !k.open, stroke: k.open || alw > 0.0, width: alw, cap: None, join: join(rj) };
    let barb = |segs: Vec<Segment>, alw: f64, rc: bool, rj: bool| Shape { segs, fill: false, stroke: true, width: alw, cap: cap(rc), join: join(rj) };
    match kind {
        Kind::StraightBarb => {
            let q = ratio(l, w);
            let xc = 0.5 * (4.0 * q * q + 1.0).sqrt() * alw;
            let xa = q * alw;
            tip = if rj { l + 0.5 * alw } else { l + xc + if h { xa } else { 0.0 } };
            visual_back = Some(l + 0.5 * alw);
            back = -0.5 * alw;
            line = if rev { l + if h { 0.5 * alw } else { 0.0 } } else { l - 0.5 * alw };
            hull.push(Point::new(if rj { l + 0.5 * alw } else { l + xc + xa }, hy(alw, 0.0)));
            upper(&mut hull, 0.5 * alw, 0.5 * w + 0.5 * alw);
            upper(&mut hull, -0.5 * alw, 0.5 * w + 0.5 * alw);
            if h {
                hull.push(Point::new(-0.5 * alw, -0.5 * alw));
            }
            p.m(0.0, 0.5 * w).l(l, 0.0);
            if h {
                p.l(if rev { l + alw } else { l - alw }, 0.0);
            } else {
                p.l(0.0, -0.5 * w);
            }
            shapes.push(barb(p.take(), alw, rc, rj));
        }
        Kind::Hooks => {
            let arc = k.arc;
            if h {
                hull.push(Point::new(0.5 * l, -0.5 * alw));
                hull.push(Point::new(l, -0.5 * alw + 0.125 * w));
            }
            if arc < 90.1 {
                let y = 0.25 * w - if rc { 0.0 } else { 0.25 * alw };
                upper(&mut hull, 0.0, y);
                upper(&mut hull, l, y);
            } else {
                upper(&mut hull, l, 0.375 * w);
                upper(&mut hull, 0.5 * l, 0.5 * w);
                if arc > 180.0 {
                    upper(&mut hull, -0.5 * l, 0.5 * w);
                    upper(&mut hull, -l, 0.375 * w);
                    upper(&mut hull, -l, 0.25 * w);
                } else {
                    let x = if rc { -0.5 * alw } else { 0.0 };
                    upper(&mut hull, x, 0.25 * w);
                    upper(&mut hull, x, 0.5 * w);
                }
            }
            l -= 0.5 * alw;
            w -= alw;
            let mut b = if rev && rj { -0.5 * alw } else { 0.0 };
            let t;
            if arc < 90.0 {
                t = arc.to_radians().sin() * l + 0.5 * alw;
            } else if arc < 180.0 {
                t = l + 0.5 * alw;
                if rc {
                    b = -0.5 * alw;
                }
            } else if arc < 270.0 {
                t = l + 0.5 * alw;
                if arc > 180.0 {
                    b = arc.to_radians().sin() * l - 0.5 * alw;
                } else if rc {
                    b = -0.5 * alw;
                }
            } else {
                t = l + 0.5 * alw;
                b = -l - 0.5 * alw;
            }
            tip = t;
            back = b;
            line = if rev {
                0.5 * alw
            } else if h {
                0.0
            } else {
                0.25 * alw
            };
            let a = arc - 90.0;
            let (s, c) = a.to_radians().sin_cos();
            p.m(c, s + 1.0).arc(a, -90.0, 1.0, 1.0);
            if !h {
                p.arc(90.0, -a, 1.0, 1.0);
            }
            let mut segs = map_segments(&p.take(), |q| Point::new(q.x * l, q.y * 0.25 * w));
            if h && rev {
                segs.push(Segment::LineTo(Point::new(alw, 0.0)));
            }
            shapes.push(barb(segs, alw, rc, rj));
        }
        Kind::ArcBarb => {
            let ah = 0.5 * k.arc;
            tip = l;
            visual_back = Some(l);
            line = l - 0.5 * alw;
            upper(&mut hull, l, 0.25 * w);
            if h {
                hull.push(Point::new(l, -0.5 * lw.line));
            }
            let (sn, cs) = ah.to_radians().sin_cos();
            let xa = 0.5 * w;
            upper(&mut hull, cs * l, sn * xa);
            if ah > 60.0 {
                upper(&mut hull, 0.5 * l, if ah < 90.0 { sn * xa } else { 0.5 * w });
            }
            if ah > 90.0 {
                if ah < 120.0 {
                    upper(&mut hull, cs * l, 0.5 * w);
                } else {
                    upper(&mut hull, -0.5 * l, 0.5 * w);
                    if ah > 150.0 {
                        upper(&mut hull, -l, 0.25 * w);
                    }
                }
            }
            if ah < 90.0 || h {
                upper(&mut hull, cs * (l - alw), sn * (0.5 * w - alw));
            }
            let x = if rc {
                cs * (l - 0.5 * alw)
            } else if ah < 90.0 {
                cs * (l - alw)
            } else {
                cs * l
            };
            back = x - if rc { 0.5 * alw } else { 0.0 };
            l -= 0.5 * alw;
            w -= alw;
            p.m(cs, sn).arc(ah, if h { 0.0 } else { -ah }, 1.0, 1.0);
            let mut segs = map_segments(&p.take(), |q| Point::new(q.x * l, q.y * 0.5 * w));
            if h {
                segs.push(Segment::LineTo(Point::new(l + if rev { 0.5 } else { -0.5 } * alw, 0.0)));
            }
            shapes.push(barb(segs, alw, rc, rj));
        }
        Kind::TeeBarb => {
            let (mut da, mut sa) = (l - ins, false);
            if da < 0.5 * alw {
                da = 0.5 * alw;
                sa = true;
            }
            let (mut db, mut sb) = (-ins, false);
            if db > -0.5 * alw {
                db = -0.5 * alw;
                sb = true;
            }
            tip = da + if rc && !sa { 0.5 * alw } else { 0.0 };
            back = db - if rc && !sb { 0.5 * alw } else { 0.0 };
            line = if rev { 0.25 * alw } else { -0.25 * alw };
            visual_back = Some(0.5 * alw);
            upper(&mut hull, tip, 0.5 * w);
            upper(&mut hull, back, 0.5 * w);
            if h {
                hull.push(Point::new(0.5 * alw, -0.5 * alw));
            }
            let yc = lw.line;
            let top = 0.5 * w - 0.5 * alw;
            if ((da - 0.5 * alw) - (db + 0.5 * alw)).abs() < 1e-9 {
                p.m(0.0, 0.5 * w).l(0.0, if h { -0.5 * alw } else { -0.5 * w });
            } else if (da - 0.5 * alw).abs() < 1e-9 {
                p.m(db, top).l(0.0, top);
                if h {
                    p.l(0.0, -0.5 * yc);
                } else {
                    p.l(0.0, -top).l(db, -top);
                }
            } else {
                p.m(db, top).l(da, top).m(0.0, top);
                if h {
                    p.l(0.0, -0.5 * yc);
                } else {
                    p.l(0.0, -top).m(db, -top).l(da, -top);
                }
            }
            shapes.push(barb(p.take(), alw, rc, rj));
        }
        Kind::ClassicalRightarrow => {
            if h {
                rj = true;
            }
            let da = l - alw;
            let db = w - alw;
            let xc = if rj { 0.0 } else { 0.5 * (40.96 * ratio(da, db).powi(2) + 1.0).sqrt() * alw };
            tip = da + if rj { 0.5 * alw } else { xc };
            line = if rev {
                if h {
                    da + 0.5 * lw.line
                } else {
                    da - 0.25 * lw.line
                }
            } else {
                da - 0.6 * lw.line
            };
            back = -0.5 * alw;
            visual_back = Some(da + 0.5 * alw);
            hull.push(Point::new(tip, 0.5 * alw));
            hull.push(Point::new(tip, -0.5 * alw));
            upper(&mut hull, 0.5 * alw, 0.5 * db + 0.5 * alw);
            upper(&mut hull, -0.5 * alw, 0.5 * db + 0.5 * alw);
            if h {
                hull.push(Point::new(da - lw.line, -0.5 * lw.line));
                hull.push(Point::new(-0.5 * alw, 0.5 * db - 0.5 * lw.line));
            }
            // in the drawing, \pgflinewidth is the tip's
            let t = alw;
            p.m(0.0, 0.5 * db);
            if h && rev {
                p.c(0.066666 * da, 0.3125 * db, 0.8 * da, 0.03125 * db, da, 0.125 * t);
                p.m(0.0, 0.5 * db).c(0.066666 * da, 0.3125 * db, 0.8 * da, 0.03125 * db, da, -0.125 * t);
                shapes.push(barb(p.take(), alw, rc, rj));
                p.m(da + 0.6 * lw.line, 0.0).l(da, 0.0);
                shapes.push(barb(p.take(), lw.line, true, rj));
            } else if h {
                p.c(0.066666 * da, 0.3125 * db, 0.8 * da, 0.03125 * db, da, 0.0);
                p.c(0.95 * da, -0.125 * t, 0.933333 * da, -0.125 * t, 0.933333 * da, -0.125 * t);
                p.l(0.8 * da - 0.5 * t, -0.125 * t);
                shapes.push(barb(p.take(), alw, rc, rj));
            } else {
                p.c(0.066666 * da, 0.3125 * db, 0.8 * da, 0.03125 * db, da, 0.0);
                p.c(0.8 * da, -0.03125 * db, 0.066666 * da, -0.3125 * db, 0.0, -0.5 * db);
                shapes.push(barb(p.take(), alw, rc, rj));
            }
        }
        Kind::CmRightarrow => {
            let da = l - alw;
            let db = w - alw;
            let (xc, xa) = if rj {
                (0.0, 0.0)
            } else {
                let q = ratio(da, db);
                (0.5 * (49.44662 * q * q + 1.0).sqrt() * alw, 3.51591 * q * alw)
            };
            tip = if rj { 0.5 * alw } else { xc + if h { xa } else { 0.0 } };
            line = if rev && h { 0.5 * alw } else { -0.5 * alw };
            back = -da - 0.5 * alw;
            visual_back = Some(0.5 * alw);
            hull.push(Point::new(tip, 0.5 * alw));
            hull.push(Point::new(tip, -0.5 * alw));
            upper(&mut hull, -da + 0.5 * alw, 0.5 * db + 0.5 * alw);
            upper(&mut hull, -da - 0.5 * alw, 0.5 * db + 0.5 * alw);
            if h {
                hull.push(Point::new(-alw, -0.5 * alw));
                hull.push(Point::new(-da - 0.5 * alw, 0.5 * db - 0.5 * alw));
            }
            p.m(-da, 0.5 * db).c(-0.81731 * da, 0.2 * db, -0.41019 * da, 0.05833333 * db, 0.0, 0.0);
            if h {
                p.l(if rev { 0.5 * alw } else { -alw }, 0.0);
            } else {
                p.c(-0.41019 * da, -0.05833333 * db, -0.81731 * da, -0.2 * db, -da, -0.5 * db);
            }
            shapes.push(barb(p.take(), alw, rc, rj));
        }
        Kind::Implies => {
            let da = 0.25 * lw.line + 0.25 * lw.inner;
            let db = 0.5 * lw.line - 0.5 * lw.inner;
            tip = 2.06 * da + 0.5 * db;
            back = -1.36 * da + 0.5 * db;
            line = 0.0;
            upper(&mut hull, -1.4 * da, 2.8 * da);
            upper(&mut hull, 2.0 * da, 0.5 * db);
            let x = 0.06 * da;
            p.m(x - 1.4 * da, 2.65 * da).c(x - 0.75 * da, 1.25 * da, x + da, 0.05 * da, x + 2.0 * da, 0.0);
            p.c(x + da, -0.05 * da, x - 0.75 * da, -1.25 * da, x - 1.4 * da, -2.65 * da);
            shapes.push(barb(p.take(), db, true, true));
        }
        Kind::Latex => {
            alw = alw.min(0.2 * l);
            let q = ratio(l, w);
            let xc = (9.0 * q * q + 1.0).sqrt() * alw;
            let xa = q * alw;
            let da = l - 0.5 * xc - 0.5 * alw;
            back = -0.5 * alw;
            line = if rev {
                if lw.inner > 0.0 {
                    0.0
                } else {
                    da - 0.5 * lw.line
                }
            } else {
                0.0
            };
            tip = if rj { da + 0.5 * alw } else { l - 0.5 * alw + if h { 1.5 * xa } else { 0.0 } };
            // the miter at the back corners
            let v = Point::new(0.3 * l, 0.2333333 * w);
            let n = v.scale(1.0 / v.norm().max(1e-12));
            let db = 0.5 * w - 0.5 * ratio(n.y + 1.0, n.x) * alw;
            hull.push(Point::new(l - 0.5 * alw + if !rj && h { 1.5 * xa } else { 0.0 }, hy(alw, 0.0)));
            hull.push(Point::new(-0.5 * alw, 0.5 * w - if h { 0.5 * alw } else { 0.0 }));
            hull.push(Point::new(-0.5 * alw, hy(alw, -0.5 * w)));
            p.m(da, 0.0).c(0.877192 * da, 0.077922 * db, 0.337381 * da, 0.519480 * db, 0.0, db);
            if h {
                p.l(0.0, 0.0);
            } else {
                p.l(0.0, -db).c(0.337381 * da, -0.519480 * db, 0.877192 * da, -0.077922 * db, da, 0.0);
            }
            p.z();
            shapes.push(closed(p.take(), alw, rj));
        }
        Kind::Stealth => {
            alw = alw.min(0.25 * (l - ins));
            let q = ratio(l, w);
            let xc = 0.5 * (4.0 * q * q + 1.0).sqrt() * alw;
            let xa = q * alw;
            // the miters at the back corners and on top
            let half_w = 0.5 * w;
            let outer = l.atan2(half_w).to_degrees();
            let inner = ins.atan2(half_w).to_degrees();
            let half = 0.5 * (outer - inner);
            let yc = 0.5 * ratio(1.0, half.to_radians().tan()) * alw;
            let (s, c) = (inner + half).to_radians().sin_cos();
            let mut back_miter = s * yc;
            let top_miter = c * yc;
            if ins == 0.0 {
                back_miter = 0.5 * alw;
            }
            let q2 = ratio(ins, w);
            let inset_miter = 0.5 * (4.0 * q2 * q2 + 1.0).sqrt() * alw;
            let da = l - xc - back_miter;
            let db = 0.5 * w - top_miter;
            back = if rj { back_miter - 0.5 * alw } else { 0.0 };
            line = if h {
                ins + inset_miter + 0.5 * alw
            } else if rev {
                if lw.inner > 0.0 {
                    ins
                } else {
                    da + back_miter - 0.25 * alw
                }
            } else {
                ins + inset_miter - 0.25 * alw
            };
            tip = if rj { da + back_miter + 0.5 * alw } else { l + if h { xa } else { 0.0 } };
            hull.push(Point::new(l + if !rj && h { xa } else { 0.0 }, hy(alw, 0.0)));
            upper(&mut hull, 0.0, 0.5 * w);
            hull.push(Point::new(ins, hy(alw, 0.0)));
            visual_back = Some(ins);
            ins += inset_miter;
            p.m(da + back_miter, 0.0).l(back_miter, db).l(ins, 0.0);
            if !h {
                p.l(back_miter, -db);
            }
            p.z();
            shapes.push(closed(p.take(), alw, rj));
        }
        Kind::Kite => {
            alw = alw.min(0.4 * l).min(0.4 * w);
            let q = ratio(l - ins, w);
            let xc = 0.5 * (4.0 * q * q + 1.0).sqrt() * alw;
            let xa = q * alw;
            let q2 = ratio(ins, w);
            let yc = 0.5 * (4.0 * q2 * q2 + 1.0).sqrt() * alw;
            let ya = q2 * alw;
            // the miter at the side corners
            let tb = 0.5 * w;
            let front = (l - ins).atan2(tb).to_degrees();
            let rear = ins.atan2(tb).to_degrees();
            let half = 0.5 * (front + rear);
            let r = if (half - 45.0).abs() < 1e-9 { std::f64::consts::SQRT_2 } else { ratio(1.0, half.to_radians().sin()) };
            let m = 0.5 * r * alw;
            let (s, c) = (-rear - 90.0 + half).to_radians().sin_cos();
            let xb = c * m;
            let side = s * m;
            let da = l - xc;
            let db = 0.5 * w + side;
            back = if rj { ya - 0.5 * alw } else { 0.0 };
            line = if rev { da - lw.line + alw } else { yc + lw.line - alw };
            tip = if rj { da + 0.5 * alw } else { l + if h { xa } else { 0.0 } };
            hull.push(Point::new(l + if !rj && h { xa } else { 0.0 }, hy(alw, 0.0)));
            upper(&mut hull, ins, 0.5 * w);
            hull.push(Point::new(0.0, hy(alw, 0.0)));
            ins += xb;
            p.m(da, 0.0).l(ins, db).l(yc, 0.0);
            if !h {
                p.l(ins, -db);
            }
            p.z();
            shapes.push(closed(p.take(), alw, rj));
        }
        Kind::Square => {
            alw = alw.min(0.5 * l);
            line = if rev { l - 0.5 * alw } else { 0.5 * alw };
            tip = l;
            back = 0.0;
            hull.push(Point::new(l, 0.5 * w));
            hull.push(Point::new(0.0, 0.5 * w));
            hull.push(Point::new(l, hy(alw, -0.5 * w)));
            hull.push(Point::new(0.0, hy(alw, -0.5 * w)));
            let (x0, x1, y) = (0.5 * alw, l - 0.5 * alw, 0.5 * w - 0.5 * alw);
            let yb = if h { 0.0 } else { -y };
            p.m(x1, y).l(x0, y).l(x0, yb).l(x1, yb).z();
            shapes.push(closed(p.take(), alw, rj));
        }
        Kind::Circle => {
            alw = alw.min(0.5 * l);
            line = if rev { l - 0.5 * alw } else { 0.5 * alw };
            tip = l;
            back = 0.0;
            upper(&mut hull, l, 0.25 * w);
            upper(&mut hull, 0.75 * l, 0.5 * w);
            upper(&mut hull, 0.25 * l, 0.5 * w);
            upper(&mut hull, 0.0, 0.25 * w);
            if h {
                hull.push(Point::new(0.0, -0.5 * alw));
                hull.push(Point::new(l, -0.5 * alw));
            }
            let (rx, ry) = (0.5 * l - 0.5 * alw, 0.5 * w - 0.5 * alw);
            if h {
                p.m(l - 0.5 * alw, 0.0).arc(0.0, 180.0, rx, ry).z();
            } else {
                p.ellipse(0.5 * l, 0.0, rx, ry);
            }
            shapes.push(closed(p.take(), alw, rj));
        }
        Kind::RoundCap | Kind::ButtCap | Kind::TriangleCap => {
            let t = 0.5 * lw.line;
            tip = l;
            back = 0.0;
            line = if rev && kind != Kind::ButtCap { l + 0.5 * lw.line } else { -0.5 * lw.line };
            upper(&mut hull, 0.0, t);
            if kind == Kind::TriangleCap {
                hull.push(Point::new(l, 0.0));
            } else {
                upper(&mut hull, l, t);
            }
            let start = -0.75 * lw.line;
            let r = rev && kind != Kind::ButtCap;
            // reversed, the cap is drawn mirrored (x ↦ l - x) but its front part is not
            let outer = |x: f64| if r { l - x } else { x };
            p.m(outer(start), t).l(outer(0.0), t);
            if r {
                p.l(outer(l), t);
            }
            match kind {
                Kind::RoundCap => {
                    let k = 0.55228475;
                    p.c(k * l, t, l, k * t, l, 0.0).c(l, -k * t, k * l, -t, 0.0, -t);
                }
                Kind::ButtCap => {
                    p.l(l, t).l(l, -t);
                }
                _ => {
                    p.l(l, 0.0);
                    if r {
                        p.l(outer(l), -t);
                    }
                    p.l(outer(0.0), -t);
                }
            }
            p.l(outer(start), -t).z();
            shapes.push(Shape { segs: p.take(), fill: true, stroke: false, width: 0.0, cap: None, join: None });
        }
        Kind::FastTriangle | Kind::FastRound => {
            let t = 0.5 * lw.line;
            tip = l + ins;
            back = 0.0;
            line = if rev { l + ins } else { 0.0 };
            upper(&mut hull, 0.0, t);
            if kind == Kind::FastTriangle {
                upper(&mut hull, ins, t);
                hull.push(Point::new(l + ins, 0.0));
                p.m(0.0, t).l(ins, t).l(ins + l, 0.0).l(ins, -t).l(0.0, -t).l(l, 0.0).z();
            } else {
                upper(&mut hull, l + ins, t);
                let k = 0.55228475;
                p.m(0.0, t).l(ins, t);
                p.c(ins + k * l, t, ins + l, k * t, ins + l, 0.0).c(ins + l, -k * t, ins + k * l, -t, ins, -t);
                p.l(0.0, -t).c(k * l, -t, l, -k * t, l, 0.0).c(l, k * t, k * l, t, 0.0, t).z();
            }
            shapes.push(Shape { segs: p.take(), fill: true, stroke: false, width: 0.0, cap: None, join: None });
        }
        Kind::Rays => {
            let n = k.n.max(1);
            let inc = 360.0 / n as f64;
            let first = if n % 2 == 1 { 0.25 * inc } else { 0.5 * inc };
            let (s, c) = first.to_radians().sin_cos();
            let mut xa = c * 0.5 * l;
            if rc {
                xa += 0.5 * alw;
            } else {
                let v = Point::new(c * l, s * w);
                xa += v.y / v.norm().max(1e-12) * 0.5 * alw;
            }
            tip = xa;
            back = -xa;
            line = 0.0;
            let r = if rc { 0.5 * alw } else { 0.0 };
            upper(&mut hull, xa, 0.25 * w);
            upper(&mut hull, 0.25 * l, 0.5 * w + r);
            upper(&mut hull, -0.25 * l, 0.5 * w + r);
            upper(&mut hull, -xa, 0.25 * w);
            if h {
                hull.push(Point::new(-0.5 * l - r, -0.5 * alw));
                hull.push(Point::new(0.5 * l + r, -0.5 * alw));
            }
            let mut a = first;
            let end = if h { 180.0 } else { 360.0 };
            while a < end - 1e-9 {
                let (s, c) = a.to_radians().sin_cos();
                p.m(c * 0.5 * l, s * 0.5 * w).l(0.0, 0.0);
                if h && a <= inc + 1e-9 {
                    p.l(-0.5 * alw, 0.0);
                }
                a += inc;
            }
            shapes.push(Shape { segs: p.take(), fill: false, stroke: true, width: alw, cap: cap(rc), join: None });
        }
        Kind::JackTap => {
            // a tenth of the length along the line, then up the width at its middle and down to
            // the end: open, or closed and filled
            tip = 1.1 * l;
            back = 0.0;
            line = 0.0;
            hull.extend([Point::new(0.0, 0.0), Point::new(0.6 * l, w), Point::new(tip, 0.0)]);
            p.m(0.0, 0.0).l(0.1 * l, 0.0).l(0.6 * l, w).l(tip, 0.0);
            if !k.open {
                p.z();
            }
            shapes.push(Shape { segs: p.take(), fill: !k.open, stroke: true, width: alw, cap: cap(rc), join: join(rj) });
        }
        Kind::Underscore | Kind::Old(_) => {
            tip = 0.0;
            back = 0.0;
            line = 0.0;
        }
    }
    Geometry { tip, back, line, visual_tip: tip, visual_back: visual_back.unwrap_or(back), hull, shapes }
}

/// A tip declared with `\pgfarrowsdeclare`.
fn declared_geometry(i: u16) -> Geometry {
    let tip = DECLARED.with(|d| d.borrow().get(i as usize).cloned());
    let Some(tip) = tip else { return Geometry { tip: 0.0, back: 0.0, line: 0.0, visual_tip: 0.0, visual_back: 0.0, hull: Vec::new(), shapes: Vec::new() } };
    let shapes = tip.paths.into_iter().map(|(segs, fill, stroke, width)| Shape { segs, fill, stroke, width, cap: None, join: None }).collect();
    Geometry { tip: tip.right, back: tip.left, line: 0.0, visual_tip: tip.right, visual_back: tip.left, hull: Vec::new(), shapes }
}

/// `direction ee` of `pgflibraryshapes.gates.ee`: a triangle `d` long, filled and stroked with
/// round joins, from `-d/2` to `d/2`.
fn direction_ee(d: f64, lw: Widths) -> Geometry {
    let mut p = Pen::default();
    let k = 0.38268343237 * d;
    p.m(-0.5 * d, k).l(0.5 * d, 0.0).l(-0.5 * d, -k).z();
    let shapes = vec![Shape { segs: p.take(), fill: true, stroke: true, width: lw.line, cap: None, join: Some(Join::Round) }];
    Geometry { tip: 0.5 * d, back: -0.5 * d, line: 0.0, visual_tip: 0.5 * d, visual_back: -0.5 * d, hull: Vec::new(), shapes }
}

/// The tips declared with `\pgfarrowsdeclare`: their extents (left and right) and drawing; the line
/// ends at 0 and they have no hull, so they leave the bounding box alone.
fn old(o: Old, length: f64, lw: Widths, _reversed: bool) -> Geometry {
    use Old::*;
    let t = lw.line;
    let mut p = Pen::default();
    let mut shapes = Vec::new();
    let mut push = |segs: Vec<Segment>, fill: bool, stroke: bool, width: f64, cap: Option<Cap>, join: Option<Join>| shapes.push(Shape { segs, fill, stroke, width, cap, join });
    // stealth and latex grow with the visible line of a double line
    let visible = if lw.inner > 0.0 { 0.6 * t - 0.4 * lw.inner } else { t };
    let polar = |a: f64, r: f64| {
        let (s, c) = a.to_radians().sin_cos();
        (r * c, r * s)
    };
    let (back, tip) = match o {
        Stealth => {
            let a = 0.28 + 0.3 * visible;
            p.m(5.0 * a, 0.0).l(-3.0 * a, 4.0 * a).l(0.0, 0.0).l(-3.0 * a, -4.0 * a);
            push(p.take(), true, false, t, None, None);
            (-3.0 * a, 5.0 * a)
        }
        To | ToReversed => {
            let a = 0.28 + 0.3 * t;
            if o == To {
                p.m(-3.0 * a, 4.0 * a).c(-2.75 * a, 2.5 * a, 0.0, 0.25 * a, 0.75 * a, 0.0);
                p.c(0.0, -0.25 * a, -2.75 * a, -2.5 * a, -3.0 * a, -4.0 * a);
            } else {
                p.m(3.5 * a, 4.0 * a).c(3.25 * a, 2.5 * a, 0.5 * a, 0.25 * a, -0.25 * a, 0.0);
                p.c(0.5 * a, -0.25 * a, 3.25 * a, -2.5 * a, 3.5 * a, -4.0 * a);
            }
            push(p.take(), false, true, 0.8 * t, Some(Cap::Round), Some(Join::Round));
            if o == To {
                (-0.84 - 1.3 * t, 0.21 + 0.625 * t)
            } else {
                (-0.21 - 0.475 * t, 0.98 + 1.45 * t)
            }
        }
        Latex => {
            let a = 0.28 + 0.3 * visible;
            p.m(9.0 * a, 0.0).c(6.3333 * a, 0.5 * a, 2.0 * a, 2.0 * a, -a, 3.75 * a).l(-a, -3.75 * a);
            p.c(2.0 * a, -2.0 * a, 6.3333 * a, -0.5 * a, 9.0 * a, 0.0);
            push(p.take(), true, false, t, None, None);
            (-a, 9.0 * a)
        }
        Bar => {
            let a = 2.0 + 1.5 * t;
            p.m(0.25 * t, -a).l(0.25 * t, a);
            push(p.take(), false, true, t, Some(Cap::Square), None);
            (-0.25 * t, 0.75 * t)
        }
        Space => (0.0, 0.88 + 0.3 * t),
        SquareBracket => {
            let a = 2.0 + 1.5 * t;
            let b = a + t;
            p.m(-0.5 * b, -a).l(0.0, -a).l(0.0, a).l(-0.5 * b, a);
            push(p.take(), false, true, t, Some(Cap::Butt), Some(Join::Miter));
            (-(1.0 + 1.25 * t), 0.5 * t)
        }
        Parenthesis => {
            let a = 2.0 + 1.5 * t;
            p.m(-0.5 * a, -a).c(0.25 * a, -0.5 * a, 0.25 * a, 0.5 * a, -0.5 * a, a);
            push(p.take(), false, true, t, Some(Cap::Round), None);
            (-(0.5 * a + 0.5 * t), 0.0625 * a + 0.5 * t)
        }
        Angle(deg) | Triangle(deg) => {
            let a = if matches!(o, Angle(_)) { 0.3 + 0.25 * t } else { 0.5 + 0.25 * t };
            let x = 0.5 * a;
            let (ext, front) = match deg {
                90 => {
                    p.m(-5.5 * a, -6.0 * a).l(x, 0.0).l(-5.5 * a, 6.0 * a);
                    (5.5, 0.707)
                }
                60 => {
                    let (dx, dy) = polar(150.0, 9.0 * a);
                    p.m(x + dx, dy).l(x, 0.0).l(x + dx, -dy);
                    (7.29, 1.0)
                }
                _ => {
                    let (dx, dy) = polar(157.0, 10.0 * a);
                    p.m(x + dx, dy).l(x, 0.0).l(x + dx, -dy);
                    (8.705, 1.28)
                }
            };
            if matches!(o, Angle(_)) {
                push(p.take(), false, true, t, Some(Cap::Round), Some(Join::Miter));
            } else {
                p.z();
                push(p.take(), true, true, t, None, Some(Join::Miter));
            }
            (-(ext * a + 0.5 * t), 0.5 * a + front * t)
        }
        OpenTriangle(deg) | OpenTriangleReversed(deg) => {
            let a = 0.5 + 0.25 * t;
            let rev = matches!(o, OpenTriangleReversed(_));
            let ext = match (deg, rev) {
                (90, false) => {
                    p.m(0.0, -6.0 * a).l(6.0 * a, 0.0).l(0.0, 6.0 * a);
                    (-0.5 * t, 6.0 * a + 0.707 * t)
                }
                (90, true) => {
                    p.m(6.0 * a, -6.0 * a).l(0.0, 0.0).l(6.0 * a, 6.0 * a);
                    (-0.707 * t, 6.0 * a + 0.5 * t)
                }
                (60, false) | (45, false) => {
                    let (x, ang, r, f) = if deg == 60 { (7.794 * a, 150.0, 9.0 * a, 1.0) } else { (9.205 * a, 157.0, 10.0 * a, 1.28) };
                    let (dx, dy) = polar(ang, r);
                    p.m(x + dx, dy).l(x, 0.0).l(x + dx, -dy);
                    (-0.5 * t, x + f * t)
                }
                _ => {
                    let (x, ang, r, f) = if deg == 60 { (7.794 * a, 30.0, 9.0 * a, 1.0) } else { (9.205 * a, 23.0, 10.0 * a, 1.28) };
                    let (dx, dy) = polar(ang, r);
                    p.m(dx, dy).l(0.0, 0.0).l(dx, -dy);
                    (-f * t, x + 0.5 * t)
                }
            };
            p.z();
            push(p.take(), false, true, t, None, Some(Join::Miter));
            ext
        }
        Dot | Ring => {
            let a = 0.4 + 0.2 * t;
            if o == Dot {
                p.ellipse(-3.0 * a, 0.0, 4.5 * a, 4.5 * a);
                push(p.take(), true, true, t, None, None);
                (-(7.5 * a + 0.5 * t), 1.5 * a + 0.5 * t)
            } else {
                p.ellipse(4.5 * a, 0.0, 4.5 * a, 4.5 * a);
                push(p.take(), false, true, t, None, None);
                (-0.5 * t, 9.0 * a + 0.5 * t)
            }
        }
        Diamond | OpenDiamond => {
            let a = 0.4 + 0.275 * t;
            if o == Diamond {
                p.m(a, 0.0).l(-6.0 * a, 4.0 * a).l(-13.0 * a, 0.0).l(-6.0 * a, -4.0 * a).z();
                push(p.take(), true, true, t, None, Some(Join::Round));
                (-(13.0 * a + 0.5 * t), a + 0.5 * t)
            } else {
                p.m(14.0 * a, 0.0).l(7.0 * a, 4.0 * a).l(0.0, 0.0).l(7.0 * a, -4.0 * a).z();
                push(p.take(), false, true, t, None, Some(Join::Round));
                (-0.5 * t, 14.0 * a + 0.5 * t)
            }
        }
        Square | OpenSquare => {
            let a = 0.4 + 0.275 * t;
            if o == Square {
                p.m(a, 4.0 * a).l(-7.0 * a, 4.0 * a).l(-7.0 * a, -4.0 * a).l(a, -4.0 * a).z();
                push(p.take(), true, true, t, None, Some(Join::Round));
                (-7.0 * a - 0.5 * t, a + 0.5 * t)
            } else {
                p.m(8.0 * a, 4.0 * a).l(0.0, 4.0 * a).l(0.0, -4.0 * a).l(8.0 * a, -4.0 * a).z();
                push(p.take(), false, true, t, None, Some(Join::Round));
                (-0.5 * t, 8.0 * a + 0.5 * t)
            }
        }
        LatexPrime => {
            let a = 0.28 + 0.3 * t;
            p.m(6.0 * a, 0.0).c(3.5 * a, 0.5 * a, -a, 1.5 * a, -4.0 * a, 3.75 * a);
            p.c(-1.5 * a, a, -1.5 * a, -a, -4.0 * a, -3.75 * a).c(-a, -1.5 * a, 3.5 * a, -0.5 * a, 6.0 * a, 0.0).z();
            push(p.take(), true, false, t, None, None);
            (-4.0 * a, 6.0 * a)
        }
        StealthPrime => {
            let a = 0.28 + 0.3 * t;
            p.m(2.0 * a, 0.0).c(-0.5 * a, 0.5 * a, -3.0 * a, 1.5 * a, -6.0 * a, 3.25 * a);
            p.c(-3.0 * a, a, -3.0 * a, -a, -6.0 * a, -3.25 * a).c(-3.0 * a, -1.5 * a, -0.5 * a, -0.5 * a, 2.0 * a, 0.0).z();
            push(p.take(), true, true, t, None, Some(Join::Round));
            (-(6.0 * a + 0.5 * t), 2.0 * a + 0.5 * t)
        }
        LeftTo | RightTo => {
            let a = 0.28 + 0.3 * t;
            let s = if o == LeftTo { 1.0 } else { -1.0 };
            let u = 0.8 * t;
            p.m(-3.0 * a, s * 4.0 * a).c(-2.75 * a, s * 2.5 * a, 0.0, s * 0.25 * a, 0.75 * a, 0.0);
            p.c(0.55 * a, -s * 0.125 * u, 0.5 * a, -s * 0.125 * u, 0.5 * a, -s * 0.125 * u).l(0.0, -s * 0.125 * u);
            push(p.take(), false, true, u, Some(Cap::Round), Some(Join::Round));
            (-0.84 - 1.3 * t, 0.21 + 0.625 * t)
        }
        LeftToReversed | RightToReversed => {
            let a = 0.28 + 0.3 * t;
            let s = if o == LeftToReversed { 1.0 } else { -1.0 };
            p.m(0.5 * t, 0.0).l(-0.1 * t, 0.0);
            push(p.take(), false, true, t, Some(Cap::Butt), Some(Join::Round));
            let u = 0.8 * t;
            let x = 0.625 * u;
            p.m(x + 3.75 * a, s * 4.0 * a).c(x + 3.5 * a, s * 2.5 * a, x + 0.75 * a, s * 0.25 * a, x, s * 0.125 * u);
            p.m(x + 3.75 * a, s * 4.0 * a).c(x + 3.5 * a, s * 2.5 * a, x + 0.75 * a, s * 0.25 * a, x, -s * 0.125 * u);
            push(p.take(), false, true, u, Some(Cap::Round), Some(Join::Round));
            (-0.1 * t, 3.75 * a + 0.9 * t)
        }
        LeftHook | RightHook | Hooks => {
            let a = 0.4 + 0.2 * t;
            let hook = |p: &mut Pen, s: f64| {
                p.c(2.415 * a, 0.0, 3.75 * a, s * 1.665 * a, 3.75 * a, s * 3.0 * a);
                p.c(3.75 * a, s * 4.665 * a, 2.415 * a, s * 6.0 * a, 0.75 * a, s * 6.0 * a);
            };
            p.m(0.0, 0.0).l(0.75 * a, 0.0);
            hook(&mut p, if o == RightHook { -1.0 } else { 1.0 });
            if o == Hooks {
                p.m(0.75 * a, 0.0);
                hook(&mut p, -1.0);
            }
            push(p.take(), false, true, t, Some(Cap::Round), None);
            (-0.5 * t, 3.75 * a + 0.5 * t)
        }
        SerifCm => {
            let a = 0.4 + 0.45 * t;
            let x = 0.04 * t;
            p.m(x - 0.75 * a, 0.5 * t).c(x - 0.375 * a, 0.5 * t, x - 0.375 * a, 0.7 * t, x - 0.375 * a, 1.95 * a).l(x, 1.95 * a);
            p.c(x - 0.04 * t, 0.5 * a, x - 0.04 * t, -0.5 * a, x, -1.95 * a).l(x - 0.375 * a, -1.95 * a);
            p.c(x - 0.375 * a, -0.7 * t, x - 0.375 * a, -0.5 * t, x - 0.75 * a, -0.5 * t).z();
            push(p.take(), true, false, t, None, None);
            (-0.75 * a, 0.04 * t)
        }
        RoundCap => {
            p.m(0.0, 0.0).l(0.5 * t, 0.0);
            push(p.take(), false, true, t, Some(Cap::Round), None);
            (0.0, t)
        }
        ButtCap => {
            p.m(-0.1 * t, 0.0).l(0.5 * t, 0.0);
            push(p.take(), false, true, t, Some(Cap::Butt), None);
            (-0.1 * t, 0.5 * t)
        }
        Triangle90Cap => {
            p.m(-0.1 * t, 0.5 * t).l(0.5 * t, 0.5 * t).l(t, 0.0).l(0.5 * t, -0.5 * t).l(-0.1 * t, -0.5 * t).z();
            push(p.take(), true, false, t, None, None);
            (-0.1 * t, t)
        }
        Triangle90CapReversed => {
            p.m(t, 0.5 * t).l(-0.1 * t, 0.5 * t).l(-0.1 * t, -0.5 * t).l(t, -0.5 * t).l(0.5 * t, 0.0).z();
            push(p.take(), true, false, t, None, None);
            (-0.1 * t, t)
        }
        FastCap => {
            p.m(-0.1 * t, 0.5 * t).l(0.5 * t, 0.5 * t).l(t, 0.0).l(0.5 * t, -0.5 * t).l(-0.1 * t, -0.5 * t).z();
            p.m(t, 0.5 * t).l(1.5 * t, 0.5 * t).l(2.0 * t, 0.0).l(1.5 * t, -0.5 * t).l(t, -0.5 * t).l(1.5 * t, 0.0).z();
            push(p.take(), true, false, t, None, None);
            (-0.1 * t, 2.0 * t)
        }
        FastCapReversed => {
            p.m(-0.1 * t, 0.5 * t).l(t, 0.5 * t).l(0.5 * t, 0.0).l(t, -0.5 * t).l(-0.1 * t, -0.5 * t).z();
            p.m(1.5 * t, 0.5 * t).l(2.0 * t, 0.5 * t).l(1.5 * t, 0.0).l(2.0 * t, -0.5 * t).l(1.5 * t, -0.5 * t).l(t, 0.0).z();
            push(p.take(), true, false, t, None, None);
            (-0.1 * t, 2.0 * t)
        }
        Implies => {
            let a = 0.25 * t + 0.25 * lw.inner;
            let b = 0.5 * t - 0.5 * lw.inner;
            let x = 0.06 * a;
            p.m(x - 1.4 * a, 2.65 * a).c(x - 0.75 * a, 1.25 * a, x + a, 0.05 * a, x + 2.0 * a, 0.0);
            p.c(x + a, -0.05 * a, x - 0.75 * a, -1.25 * a, x - 1.4 * a, -2.65 * a);
            push(p.take(), false, true, b, Some(Cap::Round), Some(Join::Round));
            (-1.36 * a - 0.5 * b, 2.06 * a + 0.5 * b)
        }
        // `direction ee` has its length
        DirectionEe => return direction_ee(length, lw),
        User(i) => return declared_geometry(i),
    };
    Geometry { tip, back, line: 0.0, visual_tip: tip, visual_back: back, hull: Vec::new(), shapes }
}

// ---------------------------------------------------------------------------------------- placing

/// A tip of an end, ready to be placed.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    g: Geometry,
    sep: f64,
    slant: f64,
    swap: bool,
    color: Option<Color>,
    fill: Option<Color>,
    flex: Flex,
}

/// The tips at one end of a path, as pgf computes them before drawing (`\pgf@arrow@compute@shortening`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EndTips {
    tips: Vec<Placed>,
    /// How far the path is pulled back from its end (pt), `shorten` included.
    pub shortening: f64,
    /// From the back of the first tip to the end of the path (pt), `shorten` included.
    pub total: f64,
}

impl EndTips {
    /// The tips of `spec` (none if empty) at the start or the end of a path drawn with `lw`,
    /// shortened by `shorten` (pt) more. Warnings for unknown tips and keys.
    pub fn new(ev: &mut dyn Eval, spec: &str, start: bool, user: &UserTips, lw: Widths, shorten: f64, warn: &mut Vec<String>) -> EndTips {
        let mut e = EndTips::default();
        let seq = if spec.trim().is_empty() {
            Vec::new()
        } else {
            sequence(spec, start, user, 0).unwrap_or_else(|| {
                warn.push(format!("arrow tip '{}' not supported: ignored", spec.trim()));
                Vec::new()
            })
        };
        let mut after_line_end = false;
        for r in seq {
            let (kind, texts) = match r {
                Raw::Dot => {
                    after_line_end = true;
                    continue;
                }
                Raw::Tip { kind, opts } => (kind, opts),
            };
            let opts: Vec<Opt> = texts.iter().flat_map(|t| parse_opts(ev, t, warn)).collect();
            let k = Keys::of(kind, &opts, lw);
            let g = geometry(kind, &k, lw);
            e.total += g.tip - g.back + k.sep;
            if after_line_end {
                e.shortening += g.tip + k.sep - g.back;
            } else {
                e.shortening = g.tip + k.sep - g.line;
            }
            e.tips.push(Placed { g, sep: k.sep, slant: k.slant, swap: k.swap, color: k.color, fill: k.fill, flex: k.flex });
        }
        e.shortening += shorten;
        e.total += shorten;
        e
    }

    pub fn is_empty(&self) -> bool {
        self.tips.is_empty()
    }

    /// Draws the tips for an end at `at` reached from `from` (canvas cm): pgf's
    /// `\pgf@arrow@draw@arrow` in the frame of `\pgftransformarrow`. Returns the hull points, which
    /// count for the bounding box. On a path of no length the tips point up, as pgf's normalized
    /// null vector does.
    pub fn draw(&self, at: Point, from: Point, stroke: &Stroke, fill_opacity: f64, items: &mut Vec<Item>) -> Vec<Point> {
        let d = at.minus(from);
        let u = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(0.0, 1.0) };
        let n = Point::new(-u.y, u.x);
        let canvas = |p: Point| at.plus(u.scale(p.x * PT)).plus(n.scale(p.y * PT));
        let mut hull = Vec::new();
        let mut pos = -self.total;
        for t in &self.tips {
            let off = pos - t.g.back;
            pos += t.g.tip + t.sep - t.g.back;
            t.paint(|p| canvas(Point::new(p.x + off, p.y)), stroke, fill_opacity, items, &mut hull);
        }
        hull
    }

    /// Whether the end is drawn along a curve, when it ends with one: some tip is flexed (pgf's
    /// precise shortening).
    pub fn flexed(&self) -> bool {
        self.tips.iter().any(|t| t.flex != Flex::Quick)
    }

    /// Draws the tips on a curve (`\pgf@do@draw@curved`), each turned so that its visual ends (or
    /// tip and back ends, for `flex'`) lie on it; the curve is measured from its point `along.0`,
    /// where the path ends. Returns the hull points.
    pub fn draw_on_curve(&self, along: &Curvilinear, stroke: &Stroke, fill_opacity: f64, items: &mut Vec<Item>) -> Vec<Point> {
        let mut hull = Vec::new();
        let mut xb = self.total;
        for t in &self.tips {
            xb += t.g.back;
            let origin = xb;
            xb -= t.g.tip + t.sep;
            let (start, end, rigidity) = match t.flex {
                Flex::FlexPrime(r) => (t.g.tip, t.g.back, r),
                Flex::Flex(r) => (t.g.visual_tip, t.g.visual_back, r),
                Flex::Quick | Flex::Bend => (t.g.visual_tip, t.g.visual_back, 1.0),
            };
            let end = rigidity * (end - start) + start;
            let (at, u) = if (start - end).abs() < 1e-9 {
                // no length to flex: along the tangent
                let time = along.time(origin - start);
                (along.at(time), along.toward_start(time).scale(-1.0))
            } else {
                let p = along.at(along.time(origin - start));
                let q = along.at(along.time(origin - end));
                let d = p.minus(q);
                (p, if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(0.0, 1.0) })
            };
            let n = Point::new(-u.y, u.x);
            t.paint(|p| at.plus(u.scale((p.x - start) * PT)).plus(n.scale(p.y * PT)), stroke, fill_opacity, items, &mut hull);
        }
        hull
    }
}

impl Placed {
    /// Paints the tip through `place`, from its frame (pt) after its slant and swap to the canvas.
    fn paint(&self, place: impl Fn(Point) -> Point, stroke: &Stroke, fill_opacity: f64, items: &mut Vec<Item>, hull: &mut Vec<Point>) {
        let local = |p: Point| {
            let y = if self.swap { -p.y } else { p.y };
            place(Point::new(p.x + self.slant * y, y))
        };
        let color = self.color.unwrap_or(stroke.color);
        for s in &self.g.shapes {
            let segments = map_segments(&s.segs, local);
            let fill = s.fill.then(|| Fill { paint: Paint::Solid(self.fill.unwrap_or(color)), opacity: fill_opacity, even_odd: false });
            let line = s.stroke.then(|| Stroke { color, width: s.width * PT, dash: None, cap: s.cap.unwrap_or(stroke.cap), join: s.join.unwrap_or(stroke.join), ..stroke.clone() });
            items.push(Item::Path { segments, fill, stroke: line });
        }
        hull.extend(self.g.hull.iter().map(|&p| local(p)));
    }
}

/// A cubic curve measured by length from its first point, the way pgf's curvilinear library does
/// it: lengths at four times near the start (about 1, 2, 4 and 8 pt along), straight between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Curvilinear {
    pub points: [Point; 4],
    tau: f64,
    lengths: [f64; 4],
}

impl Curvilinear {
    /// `\pgfsetcurvilinearbeziercurve` (canvas cm).
    pub fn new(points: [Point; 4]) -> Curvilinear {
        let mut c = Curvilinear { points, tau: 0.0, lengths: [0.0; 4] };
        let pt = |a: Point, b: Point| b.minus(a).norm() / PT;
        let polygon = pt(points[0], points[1]) + pt(points[1], points[2]) + pt(points[2], points[3]);
        c.tau = ratio(1.0, polygon);
        let mut first = pt(points[0], c.at(c.tau));
        if first > 1.0 {
            c.tau = ratio(c.tau, first);
            first = pt(points[0], c.at(c.tau));
        }
        let mut sum = first;
        c.lengths[0] = sum;
        for (k, f) in [2.0, 4.0, 8.0].into_iter().enumerate() {
            sum += pt(c.at(c.tau * f / 2.0), c.at(c.tau * f));
            c.lengths[k + 1] = sum;
        }
        c
    }

    /// The point at `t` (beyond 1, the cubic goes on).
    pub fn at(&self, t: f64) -> Point {
        let [a, b, c, d] = self.points;
        let s = 1.0 - t;
        a.scale(s * s * s).plus(b.scale(3.0 * s * s * t)).plus(c.scale(3.0 * s * t * t)).plus(d.scale(t * t * t))
    }

    /// The unit direction at `t` toward increasing `t`, away from the first point.
    fn toward_start(&self, t: f64) -> Point {
        let [a, b, c, d] = self.points;
        let s = 1.0 - t;
        let mut v = b.minus(a).scale(3.0 * s * s).plus(c.minus(b).scale(6.0 * s * t)).plus(d.minus(c).scale(3.0 * t * t));
        if v.norm() < 1e-12 {
            // a degenerate start: toward the first control point that differs
            v = self.points.iter().find(|p| p.minus(a).norm() > 1e-12).map(|p| p.minus(a)).unwrap_or(Point::new(0.0, -1.0));
        }
        v.scale(1.0 / v.norm())
    }

    /// `\pgfcurvilineardistancetotime`: the time at `distance` (pt) from the first point.
    pub fn time(&self, distance: f64) -> f64 {
        let [la, lb, lc, ld] = self.lengths;
        let tau = self.tau;
        // straight through (l0, t0) and (l1, t1)
        let line = |l0: f64, t0: f64, l1: f64, t1: f64| t0 + ratio(t1 - t0, l1 - l0) * (distance - l0);
        if distance < la {
            ratio(tau, la) * distance
        } else if distance < lb {
            line(la, tau, lb, 2.0 * tau)
        } else if distance < lc {
            line(lb, 2.0 * tau, lc, 4.0 * tau)
        } else if distance < ld {
            line(lc, 4.0 * tau, ld, 8.0 * tau)
        } else {
            ratio(8.0 * tau, ld) * distance
        }
    }

    /// The curve from time `t` to the end, as a cubic: what is left once the start is cut off.
    pub fn after(&self, t: f64) -> [Point; 4] {
        let [a, b, c, d] = self.points;
        let ab = a.lerp(b, t);
        let bc = b.lerp(c, t);
        let cd = c.lerp(d, t);
        let abc = ab.lerp(bc, t);
        let bcd = bc.lerp(cd, t);
        [abc.lerp(bcd, t), bcd, cd, d]
    }
}

/// A path of a tip: its segments, whether it is filled, and its stroke's caps and joins if stroked.
pub type TipPath = (Vec<Segment>, bool, Option<(Cap, Join)>);

/// The tip `spec` as `\pgfarrowdraw` draws it with lines `line_width` wide (cm), its end at the
/// origin pointing right: its total length (cm) and its paths, each with whether it is filled and
/// its stroke's caps and joins if it is stroked; its options are evaluated by `ev`. None for an
/// unknown tip.
pub fn tip_shapes(ev: &mut dyn Eval, spec: &str, line_width: f64) -> Option<(f64, Vec<TipPath>)> {
    let mut warn = Vec::new();
    let e = EndTips::new(ev, spec, false, &UserTips::default(), Widths { line: line_width / PT, inner: 0.0 }, 0.0, &mut warn);
    if !warn.is_empty() || e.is_empty() {
        return None;
    }
    let stroke = Stroke { color: Color::BLACK, width: line_width, dash: None, cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0, opacity: 1.0 };
    let mut items = Vec::new();
    e.draw(Point::default(), Point::new(-1.0, 0.0), &stroke, 1.0, &mut items);
    let paths = items
        .into_iter()
        // (tips draw paths only)
        .filter_map(|i| if let Item::Path { segments, fill, stroke } = i { Some((segments, fill.is_some(), stroke.map(|s| (s.cap, s.join)))) } else { None })
        .collect();
    Some((e.total * PT, paths))
}

/// Draws the tip `spec` at `p`, pointing along `d`, its options evaluated by `ev` and the tips
/// declared in the document from `user`. Returns how far to pull the line back, in cm.
pub fn tip_items(ev: &mut dyn Eval, user: &UserTips, spec: &str, p: Point, d: Point, stroke: &Stroke, items: &mut Vec<Item>) -> f64 {
    let mut warn = Vec::new();
    let e = EndTips::new(ev, spec, false, user, Widths { line: stroke.width / PT, inner: 0.0 }, 0.0, &mut warn);
    e.draw(p, p.minus(d), stroke, stroke.opacity, items);
    e.shortening * PT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(spec: &str, start: bool) -> Vec<String> {
        sequence(spec, start, &UserTips::default(), 0)
            .unwrap()
            .iter()
            .map(|r| match r {
                Raw::Tip { kind, opts } => format!("{kind:?}{opts:?}"),
                Raw::Dot => ".".into(),
            })
            .collect()
    }

    fn end(spec: &str, lw: f64) -> EndTips {
        let mut w = Vec::new();
        let e = EndTips::new(&mut Plain, spec, false, &UserTips::default(), Widths { line: lw, inner: 0.0 }, 0.0, &mut w);
        assert!(w.is_empty(), "{w:?}");
        e
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn specifications() {
        let u = UserTips::default();
        assert_eq!(parse_arrows("->", &u), Some((None, Some(">".into()))));
        assert_eq!(parse_arrows("<->", &u), Some((Some("<".into()), Some(">".into()))));
        assert_eq!(parse_arrows("-{Latex[length=3mm]}", &u), Some((None, Some("{Latex[length=3mm]}".into()))));
        assert_eq!(parse_arrows("|-|", &u), Some((Some("|".into()), Some("|".into()))));
        assert_eq!(parse_arrows("-", &u), Some((None, None)));
        assert_eq!(parse_arrows("<<.<->_>", &u), Some((Some("<<.<".into()), Some(">_>".into()))));
        assert_eq!(parse_arrows("red", &u), None);
        assert_eq!(parse_arrows("north east", &u), None);
        assert_eq!(parse_arrows("x-y", &u), None);
        assert_eq!(parse_arrows("-foo", &u), None);
        let mut user = UserTips::default();
        user.declare("foo", "Stealth[red]");
        assert!(parse_arrows("-foo", &user).is_some());
    }

    #[test]
    fn sequences_from_the_back_to_the_point() {
        // `>` is To, Computer Modern Rightarrow
        assert_eq!(names(">", false), ["CmRightarrow[]"]);
        assert_eq!(names(">>", false).len(), 2);
        // `<` at the end is `>` reversed; at the start it is `>`
        assert_eq!(names("<", false), ["CmRightarrow[\"reversed\"]"]);
        assert_eq!(names("<", true), ["CmRightarrow[]"]);
        // a shorthand's options come after the tip's own
        assert_eq!(names("Bar[red]", false), ["TeeBarb[\"length=+0pt\", \"red\"]"]);
        // the start reverses the order: the first tip read is at the point
        assert_eq!(names("Latex[] Stealth[]", false), ["Latex[]", "Stealth[]"]);
        assert_eq!(names("{Latex[] Stealth[]}", true), ["Stealth[]", "Latex[]"]);
        assert_eq!(names("|<", true), ["CmRightarrow[\"sep=0pt\"]", ".", "TeeBarb[\"length=+0pt\"]"]);
        // initial options for every tip
        assert_eq!(names("[red]>>", false), ["CmRightarrow[\"red\"]", "CmRightarrow[\"red\"]"]);
        assert_eq!(names("spaced to", false), ["Old(To)[\"sep=0pt\"]", ".", "Old(Space)[]"]);
        assert_eq!(names("]", false), ["Old(SquareBracket)[]"]);
        assert_eq!(names("[", true), ["Old(SquareBracket)[]"]);
        assert_eq!(names("stealth reversed", false), ["Old(Stealth)[\"reversed\"]"]);
        assert!(sequence("Nothing", false, &UserTips::default(), 0).is_none());
    }

    #[test]
    fn keys_in_pgf_order() {
        let lw = Widths { line: 0.4, inner: 0.0 };
        // width' is late: it reads the length given after it
        let k = Keys::of(Kind::Latex, &[Opt::WidthOfLength(Dep(0.0, 1.0, 0.0)), Opt::Length(Dep(10.0, 0.0, 0.0))], lw);
        assert!(close(k.length, 10.0) && close(k.width, 10.0));
        let k = Keys::of(Kind::Latex, &[Opt::Scale(2.0)], lw);
        assert!(close(k.length, 2.0 * (3.0 + 4.5 * 0.4)) && close(k.width, 0.75 * k.length));
        // a 60° angle: equal sides
        let k = Keys::of(Kind::Stealth, &[Opt::Angle(60.0, Dep(2.0, 0.0, 0.0))], lw);
        assert!(close(k.width, 2.0) && close(k.length, 3f64.sqrt()));
        let k = Keys::of(Kind::Latex, &[Opt::Reversed, Opt::Reversed, Opt::Swap], lw);
        assert!(!k.reversed && k.swap);
        // the line width of a double line's visible part
        let d = Widths { line: 2.0, inner: 1.0 };
        assert!(close(d.dep(Dep(0.0, 1.0, 1.0)), 0.5));
    }

    #[test]
    fn options_are_read() {
        let mut w = Vec::new();
        let o = parse_opts(&mut Plain, "length=3pt 2 1, width'=0pt 1, red, open, sep, harpoon, right, angle=60:2pt, n=3", &mut w);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(o[0], Opt::Length(Dep(3.0, 2.0, 1.0)));
        assert_eq!(o[1], Opt::WidthOfLength(Dep(0.0, 1.0, 0.0)));
        assert!(matches!(o[2], Opt::Color(_)));
        assert_eq!(o[4], Opt::Sep(Dep(0.88, 0.3, 1.0)));
        assert!(o.contains(&Opt::Swap) && o.contains(&Opt::Angle(60.0, Dep(2.0, 0.0, 0.0))) && o.contains(&Opt::N(3)));
        parse_opts(&mut Plain, "no such key=1", &mut w);
        assert_eq!(w.len(), 1);
        // without an engine, an expression is not a value: it counts as 0
        let o = parse_opts(&mut Plain, "length=2*3pt, n=1+2", &mut w);
        assert_eq!(o, [Opt::Length(Dep(0.0, 0.0, 0.0)), Opt::N(0)]);
    }

    #[test]
    fn default_tip_is_computer_modern_rightarrow() {
        // length 1.6pt + 2.2 lw, line ends half a line width behind the point
        let e = end(">", 0.4);
        let l = 1.6 + 2.2 * 0.4;
        assert!(close(e.total, 0.2 + (l - 0.4) + 0.2), "{e:?}");
        assert!(close(e.shortening, 0.2 + 0.2));
        // the legacy `to`: the line stops at its back
        let e = end("to", 0.4);
        assert!(close(e.shortening, 0.21 + 0.625 * 0.4) && close(e.total, 0.21 + 0.625 * 0.4 + 0.84 + 1.3 * 0.4));
    }

    #[test]
    fn several_tips_and_the_line_end() {
        let one = end("Latex", 0.4);
        let two = end("Latex[] Latex[]", 0.4);
        assert!(close(two.total, 2.0 * one.total));
        // the line runs to the last tip
        assert!(close(two.shortening, one.shortening));
        // after `.` the tips add to the shortening
        let dot = end("Latex[] . Latex[]", 0.4);
        assert!(close(dot.shortening, one.shortening + one.total));
        let sep = end("Latex[sep=1pt]", 0.4);
        assert!(close(sep.total, one.total + 1.0) && close(sep.shortening, one.shortening + 1.0));
    }

    #[test]
    fn latex_geometry() {
        let e = end("Latex", 0.4);
        let g = &e.tips[0].g;
        let l = 3.0 + 4.5 * 0.4;
        // the point a miter short of the length, the line stopping at the back
        assert!(close(g.tip, l - 0.2) && close(g.back, -0.2) && close(g.line, 0.0), "{g:?}");
        assert!(g.shapes[0].fill && g.shapes[0].stroke);
        assert_eq!(g.hull.len(), 3);
        // reversed: the ends swap sides
        let r = end("Latex[reversed]", 0.4);
        let rg = &r.tips[0].g;
        assert!(close(rg.tip, 0.2) && close(rg.back, -(l - 0.2)));
        let open = end("Latex[open]", 0.4);
        assert!(!open.tips[0].g.shapes[0].fill);
    }

    #[test]
    fn stealth_and_kite_and_bar() {
        let e = end("Stealth", 0.0);
        let g = &e.tips[0].g;
        let l = 3.0;
        assert!(close(g.tip, l) && close(g.back, 0.0) && close(g.line, 0.325 * l), "{g:?}");
        let b = end("|", 0.4);
        let g = &b.tips[0].g;
        assert!(close(g.tip, 0.2) && close(g.back, -0.2) && close(g.line, -0.1));
        let k = end("Kite", 0.0);
        assert!(close(k.tips[0].g.tip, 3.6));
        // a harpoon has only the upper half
        let h = end("Straight Barb[harpoon]", 0.4);
        assert_eq!(h.tips[0].g.shapes[0].segs.len(), 3);
    }

    #[test]
    fn jack_tap() {
        // measured on CircuiTikZ 1.8.7: 0.3cm long and 0.15cm high whatever the line width, the
        // line pulled back a tenth more
        let l = 8.53583;
        for lw in [0.4, 2.0] {
            let e = end("Jack Tap", lw);
            let g = &e.tips[0].g;
            assert!(close(g.tip, 1.1 * l) && close(g.back, 0.0) && close(g.line, 0.0), "{g:?}");
            let s = &g.shapes[0];
            assert!(!s.fill && s.stroke && close(s.width, lw));
            assert_eq!(s.segs[2], Segment::LineTo(Point::new(0.6 * l, 4.26791)));
        }
        let wide = end("Jack Tap[width=.3cm,fill]", 0.4);
        let s = &wide.tips[0].g.shapes[0];
        assert!(s.fill && matches!(s.segs.last(), Some(Segment::Close)));
        assert!(end("Jack Tap[swap]", 0.4).tips[0].swap);
    }

    #[test]
    fn every_kind_draws() {
        for spec in [
            "Straight Barb",
            "Hooks",
            "Hooks[arc=270]",
            "Hooks[arc=60]",
            "Arc Barb",
            "Tee Barb",
            "Classical TikZ Rightarrow",
            "Classical TikZ Rightarrow[harpoon,reversed]",
            "Computer Modern Rightarrow[sharp]",
            "Implies",
            "Latex[harpoon]",
            "Stealth[round]",
            "Kite[left]",
            "Square",
            "Circle[harpoon]",
            "Round Cap",
            "Round Cap[reversed]",
            "Butt Cap",
            "Triangle Cap[reversed]",
            "Fast Triangle",
            "Fast Round",
            "Rays[n=5]",
            "Jack Tap[reversed,fill=red]",
            "Triangle",
            "Rectangle",
            "Ellipse",
            "Diamond",
            "Turned Square",
            "Bracket",
            "Parenthesis",
            "LaTeX",
            "_",
            "stealth",
            "to reversed",
            "latex reversed",
            "[",
            "]",
            "(",
            ")",
            "angle 90",
            "angle 60",
            "angle 45 reversed",
            "*",
            "o",
            "diamond",
            "open diamond",
            "square",
            "open square",
            "triangle 60",
            "open triangle 90 reversed",
            "open triangle 45",
            "latex'",
            "stealth' reversed",
            "left to",
            "right to reversed",
            "hooks",
            "left hook",
            "serif cm",
            "round cap",
            "butt cap",
            "triangle 90 cap reversed",
            "fast cap",
            "fast cap reversed",
            "implies",
            "spaced latex",
            "spaced ]",
        ] {
            let e = end(spec, 0.4);
            assert!(!e.tips.is_empty(), "{spec}");
            let mut items = Vec::new();
            let s = Stroke { color: Color::BLACK, width: 0.4 * PT, dash: Some((vec![1.0], 0.0)), cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0, opacity: 1.0 };
            e.draw(Point::new(1.0, 0.0), Point::new(0.0, 0.0), &s, 1.0, &mut items);
            assert!(items.iter().all(|i| matches!(i, Item::Path { stroke, .. } if stroke.as_ref().is_none_or(|s| s.dash.is_none()))), "{spec}");
        }
    }

    #[test]
    fn curves_measured_from_their_start() {
        // a straight cubic 10 pt long, evenly parametrized: time is distance over length
        let p = |x: f64| Point::new(x * PT, 0.0);
        let c = Curvilinear::new([p(0.0), p(10.0 / 3.0), p(20.0 / 3.0), p(10.0)]);
        for d in [0.5, 1.0, 3.0, 7.0, 12.0] {
            assert!((c.time(d) - d / 10.0).abs() < 1e-6, "{d}: {}", c.time(d));
        }
        let [a, _, _, d] = c.after(0.5);
        assert!((a.x - 5.0 * PT).abs() < 1e-9 && d == p(10.0));
        // tips laid on a quarter circle stay near it
        let r = 1.0;
        let k = 0.5523 * r;
        let arc = Curvilinear::new([Point::new(r, 0.0), Point::new(r, k), Point::new(k, r), Point::new(0.0, r)]);
        let e = end("Latex[length=10pt]", 0.0);
        let mut items = Vec::new();
        let s = Stroke { color: Color::BLACK, width: 0.0, dash: None, cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0, opacity: 1.0 };
        let hull = e.draw_on_curve(&arc, &s, 1.0, &mut items);
        // the point at the end, the back corners about 10 pt down the arc, both sides of it
        assert!(hull[0].minus(Point::new(r, 0.0)).norm() < 1e-6, "{hull:?}");
        let back = hull[1].lerp(hull[2], 0.5);
        assert!((back.norm() - r).abs() < 0.005 && back.y > 0.0, "{back:?}");
    }

    #[test]
    fn placement_along_the_path() {
        // pointing up: the tip of a Latex ends at the end point (lw 0)
        let e = end("Latex", 0.0);
        let mut items = Vec::new();
        let s = Stroke { color: Color::BLACK, width: 0.0, dash: None, cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0, opacity: 1.0 };
        let hull = e.draw(Point::new(0.0, 1.0), Point::new(0.0, 0.0), &s, 1.0, &mut items);
        let top = hull.iter().map(|p| p.y).fold(f64::MIN, f64::max);
        assert!((top - 1.0).abs() < 1e-9, "{hull:?}");
        let low = hull.iter().map(|p| p.y).fold(f64::MAX, f64::min);
        assert!((low - (1.0 - 3.0 * PT)).abs() < 1e-9);
    }
}
