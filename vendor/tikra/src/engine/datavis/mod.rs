//! Data visualizations: `\datavisualization [options] data {…} …;`, drawn as pgf's
//! datavisualization module and TikZ's data visualization libraries draw them.
//!
//! A data visualization is made of objects that listen to signals. A data point is the current
//! value of the keys under `/data point/`; each time a data format has read one, `\pgfdatapoint`
//! emits signals: in the survey (done first) the axes see the ranges of their attributes; in the
//! visualization their mappers scale each attribute, the transformers turn the scaled attributes
//! into a position, and the visualizers remember the positions, to draw them at the end. Then the
//! grids, ticks, axes, labels and legends are drawn from what the survey found.
//!
//! The keys are the libraries' ([`keys`], [`library`]); objects ([`objects`]), formats
//! ([`formats`]) and drawing ([`axes`], [`legend`]) are native. An object's handle is the macro
//! `\tikra@dv@obj{n}`, and `\tikra@dv@obj{n}.method(args)` calls a method, so that the documented
//! object interface (`new object`, `\pgfdvmin`…) works.

mod axes;
mod formats;
mod keys;
mod legend;
mod library;
mod numbers;
mod objects;

use crate::keys::macro_name;
use crate::scene::Point;
use crate::tex::{self, Cat, Meaning, Token};
use crate::{Error, PT};

use super::{macro_tokens, tikra_tokens, Engine, NATIVE_KEY_BASE};

pub use objects::Object;

/// The native commands of data visualizations, by their TeX name.
const COMMANDS: &[&str] = &[
    "datavisualization",
    "pgfdatapoint",
    "pgfkeysaddvalue",
    "pgfdvvisualizerfilter",
    "pgfmathapproxequalto",
    "tikzdvdeclarestylesheetcolorseries",
    "tikra@dv@obj",
    "tikra@dv@data",
    "tikra@dv@newobject",
    "tikra@dv@make",
    "tikra@dv@include",
    "tikra@dv@setrange",
    "tikra@dv@printnumber",
    "tikra@dv@colorseries",
    "tikra@dv@stylesignal",
    "tikra@dv@about",
    "tikra@dv@pi",
    "tikra@dv@finish",
    "tikra@dv@addentry",
    "tikra@dv@legendentry",
    "tikra@dv@sample",
    "tikra@dv@rectsample",
    "tikra@dv@labeldraw",
];

/// The signals of a data visualization, in the order pgf makes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Prepare,
    Map,
    Transform,
    Visualize,
    Finish,
    Survey,
    Phase,
    Path,
    Direction,
    Style,
}

const SIGNALS: usize = 10;

/// What the phase signal emits (`\pgfdvbeforesurvey`…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    BeforeSurvey,
    BeginSurvey,
    EndSurvey,
    AfterSurvey,
    BeforeVisualization,
    BeginVisualization,
    EndVisualization,
    AfterVisualization,
}

/// What a path or direction signal is about (`\pgfdvpathmovetotoken`…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathToken {
    MoveTo,
    LineTo,
    DirectionFrom,
    DirectionTo,
    DirectionAt,
}

/// The slot of an object that a signal calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Survey,
    Map,
    Transform,
    Phase,
    Protocol,
    Visualize,
    Test,
    InvokeStyle,
    Apply,
    Path,
    Direction,
}

/// The code a data visualization runs around its phases (the attributes of pgf's class).
#[derive(Debug, Clone, Default)]
pub struct Hooks {
    pub before_survey: Vec<Token>,
    pub at_start_survey: Vec<Token>,
    pub at_end_survey: Vec<Token>,
    pub after_survey: Vec<Token>,
    pub before_visualization: Vec<Token>,
    pub at_start_visualization: Vec<Token>,
    pub at_end_visualization: Vec<Token>,
    pub after_visualization: Vec<Token>,
}

/// One `\datavisualization` being run.
#[derive(Debug, Clone)]
pub struct Dv {
    /// the objects, by handle; 0 is the data visualization itself
    pub objects: Vec<Object>,
    pub signals: [Vec<(usize, Slot)>; SIGNALS],
    pub hooks: Hooks,
    /// the code that produces the data points, run once per survey and once to visualize
    pub data: Vec<Token>,
    pub visualizing: bool,
    pub survey_count: usize,
    pub number_of_surveys: usize,
    /// the picture's bounding box before the data visualization (its own starts empty)
    pub outer_box: (Option<Point>, Option<Point>),
    /// the tick placement in effect: logarithmic steps for `about=`
    pub log_steps: bool,
    /// for a polar direction: the angle and radius at its start and end
    pub polar_from: (Option<f64>, Option<f64>),
    pub polar_to: (Option<f64>, Option<f64>),
    /// the axes made so far (`all axes`), by name
    pub axes: Vec<String>,
    /// how many visualizers there are (their numbers for style sheets)
    pub visualizers: usize,
    /// the legends made so far (`new legend`), by name
    pub legends: Vec<String>,
    /// what is drawn after the data
    pub actions: Actions,
    /// `major=`, `minor=`… are ignored while tick positions are computed
    pub setters_off: bool,
}

/// What `visualize axis`, `visualize ticks`, `visualize grid`, `visualize label` and legends ask
/// for: the axis and the options.
#[derive(Debug, Clone, Default)]
pub struct Actions {
    pub grids: Vec<(String, Vec<Token>)>,
    pub ticks: Vec<(String, Vec<Token>)>,
    pub axes: Vec<(String, Vec<Token>)>,
    pub labels: Vec<(String, Vec<Token>)>,
    pub legends: Vec<String>,
}

impl Dv {
    fn new() -> Dv {
        Dv {
            objects: vec![Object::Main],
            signals: Default::default(),
            hooks: Hooks::default(),
            data: Vec::new(),
            visualizing: false,
            survey_count: 1,
            number_of_surveys: 1,
            outer_box: (None, None),
            log_steps: false,
            polar_from: (None, None),
            polar_to: (None, None),
            axes: Vec::new(),
            visualizers: 0,
            legends: Vec::new(),
            actions: Actions::default(),
            setters_off: false,
        }
    }

    pub fn connect(&mut self, id: usize, slot: Slot, signal: Signal) {
        self.signals[signal as usize].push((id, slot));
    }
}

/// Defines the commands, the native keys and the library's keys (called once, when the engine is built).
pub fn define(e: &mut Engine) {
    let id = super::Cmd::DataVis as u16;
    for name in COMMANDS {
        e.tex.define(name, Meaning::Native(id));
    }
    for (path, f) in keys::NATIVE {
        let id = NATIVE_KEY_BASE + e.key_fns.len() as u16;
        e.key_fns.push(*f);
        e.tex.define(&macro_name(&format!("{path}/.@cmd")), Meaning::Native(id));
    }
    for (path, v) in keys::DEFAULTS {
        e.tex.set_meaning(&Token::cs(&macro_name(&format!("{path}/.@def"))), macro_tokens(tikra_tokens(v)), false);
    }
    let mut text = String::new();
    if !e.tex.is_defined(&macro_name("/utils/exec/.@cmd")) {
        text.push_str("\\pgfkeys{/utils/exec/.code={#1}}");
    }
    if !e.tex.is_defined("pgfkeyslet") {
        text.push_str("\\def\\pgfkeyslet#1#2{\\expandafter\\let\\csname pgfk@#1\\endcsname#2}");
    }
    text.push_str(numbers::NUMBER_FORMAT);
    text.push_str(&library::definitions());
    let old = std::mem::take(&mut e.st.key_path);
    e.st.key_path = "/tikz".into();
    let mut lexer = tex::Tex::new_lexer();
    lexer.push_text(&text);
    let mut tokens = Vec::new();
    while let Some(t) = lexer.get_token() {
        tokens.push(t);
    }
    e.run_tokens(tokens).expect("the data visualization definitions run");
    e.st.key_path = old;
}

/// `\bgroup\pgfkeys{/data point/.cd,#1}\pgfdatapoint\egroup`
pub fn data_point_code(options: Vec<Token>) -> Vec<Token> {
    let mut code = vec![Token::cs("bgroup"), Token::cs("pgfkeys"), Token::Char('{', Cat::Begin)];
    code.extend(tikra_tokens("/data point/.cd,"));
    code.extend(options);
    code.push(Token::Char('}', Cat::End));
    code.push(Token::cs("pgfdatapoint"));
    code.push(Token::cs("egroup"));
    code
}

// ---------------------------------------------------------------- helpers

pub fn group(v: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(v.len() + 2);
    out.push(Token::Char('{', Cat::Begin));
    out.extend(v);
    out.push(Token::Char('}', Cat::End));
    out
}

/// A macro's expansion as text (empty when undefined).
pub fn macro_text(e: &mut Engine, name: &str) -> String {
    let Meaning::Macro(m) = e.tex.meaning_of(name) else { return String::new() };
    e.tex.expand_to_string(tex::substitute(&m.body, &[])).unwrap_or_default().trim().to_string()
}

/// A key's value, expanded, as text; None when the key has no value.
pub fn key_text(e: &mut Engine, path: &str) -> Option<String> {
    let v = crate::keys::value(&e.tex, path)?;
    if v.len() == 1 && v[0].is_cs("relax") {
        return None;
    }
    Some(e.tex.expand_to_string(v).unwrap_or_default().trim().to_string())
}

pub fn set_key(e: &mut Engine, path: &str, value: &str) {
    e.tex.define_string(&macro_name(path), value, false);
}

/// A number as data visualization keys store it: plain decimal, full precision.
pub fn num(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let r = (v * 1e9).round() / 1e9;
    let s = if (r - v).abs() <= 1e-12 * v.abs().max(1.0) { format!("{r}") } else { format!("{v}") };
    s
}

/// `\pgfdvmathenter`: a number, `(expression)`, or a stored value.
pub fn parse_value(e: &mut Engine, text: &str) -> Option<f64> {
    let t = text.trim().trim_start_matches('@');
    let t = t.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(t).trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(v) = t.parse::<f64>() {
        return Some(v);
    }
    let expr = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or(t);
    e.number(expr).ok()
}

/// A data point attribute's value (`\pgfkeysgetvalue{/data point/…}`), None if unset or empty.
pub fn attribute(e: &mut Engine, name: &str) -> Option<String> {
    key_text(e, &format!("/data point/{name}")).filter(|s| !s.is_empty())
}

pub fn set_attribute(e: &mut Engine, name: &str, value: &str) {
    set_key(e, &format!("/data point/{name}"), value);
}

impl Engine {
    /// The innermost data visualization.
    pub fn dv(&mut self) -> Result<&mut Dv, Error> {
        self.datavis.last_mut().ok_or_else(|| Error::Figure("data visualization keys outside \\datavisualization".into()))
    }

    /// The commands of [`COMMANDS`].
    pub fn datavis_command(&mut self, t: &Token) -> Result<(), Error> {
        let Token::Cs(name) = t else { return Ok(()) };
        match &**name {
            "datavisualization" => self.datavisualization(),
            "pgfdatapoint" => self.data_point(),
            "tikra@dv@obj" => {
                let id = self.argument_string()?;
                let (method, args) = self.read_method()?;
                self.call(id.parse().unwrap_or(usize::MAX), &method, args)
            }
            "tikra@dv@data" => {
                let options = self.argument()?;
                let body = self.argument()?;
                self.run_data(options, body)
            }
            "tikra@dv@newobject" => {
                let store = self.argument_string()?;
                let spec = self.argument()?;
                self.new_object(&store, spec)
            }
            "tikra@dv@make" => self.make_object(),
            "tikra@dv@include" => {
                let axis = self.argument_string()?;
                let values = self.argument()?;
                let values = self.tex.expand_fully(values)?;
                let mapper = self.object_of(&format!("{axis}/scaling mapper"));
                for item in super::foreach_items(&values) {
                    let s = tex::tokens_to_string(&item);
                    if let (Some(m), Some(v)) = (mapper, parse_value(self, &s)) {
                        self.include_value(m, v);
                    }
                }
                Ok(())
            }
            "tikra@dv@setrange" => {
                let axis = self.argument_string()?;
                let value = self.argument_string()?;
                let which = self.argument_string()?;
                if let (Some(m), Some(v)) = (self.object_of(&format!("{axis}/scaling mapper")), parse_value(self, &value)) {
                    self.set_range(m, &which, v);
                }
                Ok(())
            }
            "tikra@dv@printnumber" | "pgfmathprintnumber" => self.print_number_command(),
            "tikra@dv@colorseries" => self.color_series(),
            "tikzdvdeclarestylesheetcolorseries" => self.declare_color_series(),
            "pgfmathapproxequalto" => self.approx_equal_command(),
            "tikra@dv@stylesignal" => self.emit(Signal::Style, None),
            "pgfkeysaddvalue" => {
                let key = self.argument_string()?;
                let pre = self.argument()?;
                let post = self.argument()?;
                let old = crate::keys::value(&self.tex, &key).unwrap_or_default();
                let v = [pre, old, post].concat();
                self.tex.set_meaning(&Token::cs(&macro_name(&key)), macro_tokens(v), false);
                Ok(())
            }
            "pgfdvvisualizerfilter" => {
                let name = self.argument_string()?;
                if attribute(self, "set").unwrap_or_default() != name {
                    self.tex.set_meaning(&Token::cs("ifpgfdvfilterpassed"), Meaning::Prim(tex::Prim::Iffalse), false);
                }
                Ok(())
            }
            "tikra@dv@about" => {
                let n = self.argument_string()?;
                self.compute_about(&n)
            }
            "tikra@dv@pi" => self.pi_typesetter(),
            "tikra@dv@finish" => self.finish(),
            "tikra@dv@addentry" => {
                let legend = self.argument_string()?;
                let options = self.argument()?;
                // (`new legend entry` made the legend before the survey)
                let id = self.object_of(&format!("{}/{legend}/obj", keys::DV)).expect("the legend of an entry is made before the survey");
                let mut entry = vec![Token::cs("tikra@dv@legendentry")];
                entry.extend(group(options));
                self.call(id, "add entry", vec![entry])
            }
            "tikra@dv@legendentry" => {
                let options = self.argument()?;
                self.render_legend_entry(options)
            }
            "tikra@dv@sample" => self.legend_sample(false),
            "tikra@dv@rectsample" => self.legend_sample(true),
            // the last of the commands: `\tikra@dv@labeldraw`
            _ => {
                let name = self.argument_string()?;
                let options = self.argument()?;
                self.draw_label_in_data(&name, options)
            }
        }
    }

    /// `\tikra@dv@make{kind}{…}`: one of the objects the keys ask for, made at its time.
    fn make_object(&mut self) -> Result<(), Error> {
        let kind = self.argument_string()?;
        let dv = keys::DV;
        let arg = |e: &mut Engine| e.argument_string();
        let text = |e: &mut Engine, key: &str| key_text(e, key).unwrap_or_default();
        let made = |e: &mut Engine, key: &str| key_text(e, key).is_some_and(|s| !s.is_empty());
        let store = |e: &mut Engine, key: &str, id: Option<usize>| {
            if let Some(id) = id {
                e.tex.set_meaning(&Token::cs(&macro_name(key)), macro_tokens(handle(id)), false);
            }
        };
        let t = |s: &str| tex::string_tokens(s);
        match kind.as_str() {
            "mapper" => {
                let axis = arg(self)?;
                let key = format!("{axis}/scaling mapper");
                if made(self, &key) {
                    return Ok(());
                }
                self.prepare_scaling(&axis)?;
                let input = text(self, &format!("{axis}/attribute"));
                let (scaling, function) = (text(self, &format!("{axis}/scaling")), text(self, &format!("{axis}/function")));
                let id = self.create("scaling mapper", &[t(&input), t(&format!("{input}/scaled")), t(&scaling), t(&function)])?;
                store(self, &key, id);
            }
            "linear" => {
                let axis = arg(self)?;
                let key = format!("{axis}/linear transformer");
                if made(self, &key) {
                    return Ok(());
                }
                let a = text(self, &format!("{axis}/attribute"));
                let unit = text(self, &format!("{axis}/unit vector"));
                let id = self.create("linear transformer", &[t(&format!("{a}/scaled")), t(&unit)])?;
                store(self, &key, id);
            }
            "polar" => {
                let (angle, radius) = (arg(self)?, arg(self)?);
                let key = format!("{angle}/polar transformer");
                if made(self, &key) {
                    return Ok(());
                }
                let (a, r) = (text(self, &format!("{angle}/attribute")), text(self, &format!("{radius}/attribute")));
                let (u0, u90) = (text(self, &format!("{angle}/unit vector 0 degrees")), text(self, &format!("{angle}/unit vector 90 degrees")));
                let id = self.create("polar transformer", &[t(&format!("{a}/scaled")), t(&format!("{r}/scaled")), t(&u0), t(&u90)])?;
                store(self, &key, id);
            }
            "plot" => {
                let name = arg(self)?;
                let handlers = self.argument()?;
                let key = format!("{dv}/visualizers/{name}");
                if made(self, &key) {
                    return Ok(());
                }
                let id = self.create("plot handler visualizer", &[t(&name), handlers])?;
                store(self, &key, id);
            }
            "rectangle" => {
                let name = arg(self)?;
                let key = format!("{dv}/visualizers/{name}");
                if made(self, &key) {
                    return Ok(());
                }
                let id = self.create("rectangle visualizer", &[t(&name)])?;
                store(self, &key, id);
            }
            "count" => {
                let name = arg(self)?;
                let key = format!("{dv}/visualizers/{name}/count/obj");
                if made(self, &key) {
                    return Ok(());
                }
                let Some(id) = self.create("count", &[t(&format!("set/{name}/count"))])? else { unreachable!("a count is always made") };
                store(self, &key, Some(id));
                let filter = tikra_tokens(&format!("\\pgfdvvisualizerfilter{{{name}}}"));
                self.call(id, "set filter", vec![filter])?;
            }
            "label" => {
                let name = arg(self)?;
                let options = self.argument()?;
                self.make_label_in_data(&name, options)?;
            }
            "legend" => {
                let name = arg(self)?;
                // (made once: `new legend` asks for it once per data visualization)
                let base = format!("{dv}/{name}");
                let args: Vec<Vec<Token>> = ["@columns or rows", "@ideal", "max entries", "@strategy"].iter().map(|k| t(&text(self, &format!("{base}/{k}")))).collect();
                let id = self.create("legend", &args)?;
                store(self, &format!("{base}/obj"), id);
                self.dv()?.actions.legends.push(name);
            }
            other => self.warn(format!("data visualization: no object '{other}'")),
        }
        Ok(())
    }

    /// After the visualization: the bounding boxes, then the grids, ticks, axes, labels and legends
    /// (the grids and ticks first all their positions, then subminor, minor, major).
    fn finish(&mut self) -> Result<(), Error> {
        self.update_bounding_box(true);
        let actions = self.dv()?.actions.clone();
        for (what, list) in [("grid", &actions.grids), ("ticks", &actions.ticks)] {
            for (axis, _) in list {
                self.default_action(what, axis)?;
            }
            for kind in ["subminor", "minor", "major"] {
                for (axis, options) in list {
                    if what == "grid" {
                        self.show_grid(kind, axis, options.clone())?;
                    } else {
                        self.show_ticks(kind, axis, options.clone())?;
                    }
                }
            }
        }
        for (axis, options) in &actions.axes {
            self.show_axis(axis, options.clone())?;
        }
        for (axis, options) in &actions.labels {
            self.show_label(axis, options.clone())?;
        }
        for legend in &actions.legends {
            self.show_legend(legend)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- the \datavisualization command

    /// `\datavisualization [options] data {…} …;`: a sequence of blocks up to the semicolon, then the
    /// survey and the visualization.
    pub fn datavisualization(&mut self) -> Result<(), Error> {
        if self.datavis.len() >= 8 {
            return Err(Error::Figure("data visualizations nested too deeply".into()));
        }
        self.begin_group();
        let mut dv = Dv::new();
        // the data visualization's own bounding box starts empty
        dv.outer_box = (self.canvas.min.take(), self.canvas.max.take());
        self.datavis.push(dv);
        if self.facts.mute == 0 {
            self.facts.data_visualizations += 1;
        }
        let r = self.datavisualization_body();
        let dv = self.datavis.pop().expect("the data visualization pushed above");
        self.update_bounding_box(false);
        let (lo, hi) = dv.outer_box;
        for p in [lo, hi].into_iter().flatten() {
            self.canvas.include(p);
        }
        self.end_group();
        r
    }

    fn datavisualization_body(&mut self) -> Result<(), Error> {
        let mut handle = vec![Token::cs("tikra@dv@obj")];
        handle.extend(group(tex::string_tokens("0")));
        self.tex.set_meaning(&Token::cs("tikra@dv@main"), macro_tokens(handle), false);
        // \pgfmathprintnumber prints as pgf does, with /pgf/number format, inside data visualizations
        self.tex.set_meaning(&Token::cs("pgfmathprintnumber"), Meaning::Native(super::Cmd::DataVis as u16), false);
        let after = vec![Token::cs("tikra@dv@finish")];
        self.dv()?.hooks.after_visualization = after;
        self.parse_blocks()?;
        self.survey()?;
        self.visualize()
    }

    /// The blocks up to the semicolon.
    fn parse_blocks(&mut self) -> Result<(), Error> {
        let mut scopes = 0usize;
        loop {
            let Some(t) = self.tex.next_nonspace() else { return Err(Error::Figure("\\datavisualization: ';' missing".into())) };
            match &t {
                Token::Char(';', _) => {
                    if scopes > 0 {
                        self.warn("\\datavisualization: a scope or data group is not closed");
                    }
                    for _ in 0..scopes {
                        self.end_group();
                    }
                    return Ok(());
                }
                Token::Cs(n) if &**n == "par" => {}
                Token::Char('[', _) => {
                    let o = self.tex.read_until_char(']', false)?;
                    self.keys_command(o, Some("/tikz/data visualization"), true)?;
                }
                Token::Char(_, Cat::End) if scopes > 0 => {
                    // the end of a scope or of a data group's definition
                    self.do_add_data(vec![Token::cs("endgroup")])?;
                    self.end_group();
                    scopes -= 1;
                }
                Token::Char(c, Cat::Letter) => {
                    self.tex.back(t.clone());
                    let w = self.word();
                    match (*c, w.as_str()) {
                        (_, "data") => scopes += self.data_block()?,
                        (_, "scope") => {
                            let o = self.optional()?.unwrap_or_default();
                            self.begin_group();
                            self.keys_command(data_keys(o.clone()), Some("/"), true)?;
                            let mut code = vec![Token::cs("begingroup"), Token::cs("pgfkeys")];
                            code.extend(group(data_keys(o)));
                            self.do_add_data(code)?;
                            match self.tex.next_nonspace() {
                                Some(Token::Char(_, Cat::Begin)) => scopes += 1,
                                other => {
                                    if let Some(o) = other {
                                        self.tex.back(o);
                                    }
                                    self.warn("\\datavisualization: scope without '{'");
                                    self.end_group();
                                }
                            }
                        }
                        (_, "info") => {
                            let prime = match self.tex.get_token() {
                                Some(x) if x.is_char('\'') => true,
                                Some(x) => {
                                    self.tex.back(x);
                                    false
                                }
                                None => false,
                            };
                            let o = self.optional()?.unwrap_or_default();
                            let body = self.argument()?;
                            let mut code = tikra_tokens("{\\begin{scope}[");
                            code.extend(o);
                            code.push(Token::other(']'));
                            code.extend(body);
                            code.extend(tikra_tokens("\\end{scope}}"));
                            let dv = self.dv()?;
                            if prime {
                                dv.hooks.before_visualization.extend(code);
                            } else {
                                let old = std::mem::take(&mut dv.hooks.after_visualization);
                                dv.hooks.after_visualization = [code, old].concat();
                            }
                        }
                        _ => return Err(Error::Figure(format!("\\datavisualization: semicolon expected before '{w}'"))),
                    }
                }
                _ => return Err(Error::Figure(format!("\\datavisualization: semicolon expected before '{}'", tex::show_token(&t)))),
            }
        }
    }

    /// The letters that follow, as a word.
    fn word(&mut self) -> String {
        let mut w = String::new();
        while let Some(t) = self.tex.get_token() {
            match t {
                Token::Char(c, Cat::Letter) => w.push(c),
                other => {
                    self.tex.back(other);
                    break;
                }
            }
        }
        w
    }

    /// After `data`: `point [options]`, `group [options] {name} = {…}` (or `+=`, or a use), or a data
    /// set `[options] {…}`. Returns the scopes it opened (a group being defined).
    fn data_block(&mut self) -> Result<usize, Error> {
        let next = self.tex.next_nonspace();
        match next {
            Some(Token::Char('p', Cat::Letter)) => {
                self.tex.back(Token::Char('p', Cat::Letter));
                let w = self.word();
                if w != "point" {
                    return Err(Error::Figure(format!("\\datavisualization: 'data {w}' not understood")));
                }
                let o = self.optional()?.unwrap_or_default();
                self.do_add_data(data_point_code(o))?;
                Ok(0)
            }
            Some(Token::Char('g', Cat::Letter)) => {
                self.tex.back(Token::Char('g', Cat::Letter));
                let w = self.word();
                if w != "group" {
                    return Err(Error::Figure(format!("\\datavisualization: 'data {w}' not understood")));
                }
                let o = self.optional()?.unwrap_or_default();
                let name = self.argument_string()?;
                let mut define = false;
                match self.tex.next_nonspace() {
                    Some(x) if x.is_char('=') => {
                        // a new group
                        self.tex.set_meaning(&Token::cs(&format!("tikra@dv@group@{name}")), macro_tokens(Vec::new()), true);
                        define = true;
                    }
                    Some(x) if x.is_char('+') => {
                        // `+=`, or `+` alone: what follows it is read again
                        let y = self.tex.get_token().filter(|y| !y.is_char('='));
                        y.into_iter().for_each(|y| self.tex.back(y));
                        define = true;
                    }
                    Some(x) => self.tex.back(x),
                    None => {}
                }
                if define {
                    match self.tex.next_nonspace() {
                        Some(Token::Char(_, Cat::Begin)) => {
                            self.begin_group();
                            set_key(self, "/pgf/data/store in group", &name);
                            let mut code = vec![Token::cs("begingroup"), Token::cs("pgfkeys")];
                            code.extend(group(data_keys(o)));
                            self.do_add_data(code)?;
                            return Ok(1);
                        }
                        other => {
                            if let Some(o) = other {
                                self.tex.back(o);
                            }
                            self.warn("\\datavisualization: data group without '{'");
                            return Ok(0);
                        }
                    }
                }
                // a use: the group's code adds its data
                self.begin_group();
                let r = (|| {
                    self.keys_command(data_keys(o), Some("/"), true)?;
                    let name = format!("tikra@dv@group@{name}");
                    match self.tex.meaning_of(&name) {
                        Meaning::Macro(m) => {
                            let code = tex::substitute(&m.body, &[]);
                            self.begin_group();
                            set_key(self, "/pgf/data/store in group", "");
                            let r = self.run_tokens(code);
                            self.end_group();
                            r
                        }
                        _ => {
                            self.warn(format!("undefined data group '{}'", name.trim_start_matches("tikra@dv@group@")));
                            Ok(())
                        }
                    }
                })();
                self.end_group();
                r.map(|_| 0)
            }
            other => {
                if let Some(o) = other {
                    self.tex.back(o);
                }
                self.data_set()?;
                Ok(0)
            }
        }
    }

    /// `data [options] {…}` (`\pgfdata`): the options are read, then the inline data, line by line.
    fn data_set(&mut self) -> Result<(), Error> {
        let o = self.optional()?.unwrap_or_default();
        self.begin_group();
        let r = (|| {
            self.keys_command(data_keys(o.clone()), Some("/"), true)?;
            if let Some(group_name) = key_text(self, "/pgf/data/use group").filter(|s| !s.is_empty()) {
                let name = format!("tikra@dv@group@{group_name}");
                if let Meaning::Macro(m) = self.tex.meaning_of(&name) {
                    let code = tex::substitute(&m.body, &[]);
                    return self.run_tokens(code);
                }
                self.warn(format!("undefined data group '{group_name}'"));
                return Ok(());
            }
            if key_text(self, "/pgf/data/read from file").is_some_and(|s| !s.is_empty()) {
                self.warn("data [read from file=…]: external data files are not read");
                return Ok(());
            }
            // the lines matter: the ends of lines are read as characters
            match self.tex.next_nonspace() {
                Some(Token::Char(_, Cat::Begin)) => {}
                other => {
                    if let Some(o) = other {
                        self.tex.back(o);
                    }
                    return Err(Error::Figure("data: opening brace expected".into()));
                }
            }
            self.tex.begin_group();
            self.tex.set_catcode('\n', Cat::Other);
            self.tex.set_catcode('\r', Cat::Other);
            let body = self.tex.read_group_body();
            self.tex.end_group();
            let body = body?;
            let mut code = vec![Token::cs("tikra@dv@data")];
            code.extend(group(o));
            code.extend(group(body));
            self.do_add_data(code)
        })();
        self.end_group();
        r
    }

    /// Data for the data visualization, or for the group being defined.
    pub fn do_add_data(&mut self, code: Vec<Token>) -> Result<(), Error> {
        let mut call = vec![Token::cs("tikra@dv@main")];
        call.extend(tikra_tokens(".add data"));
        call.push(Token::other('('));
        call.extend(group(code));
        call.push(Token::other(')'));
        self.add_data_call(call)
    }

    /// The call adding data is run, or kept in the group being defined.
    pub fn add_data_call(&mut self, call: Vec<Token>) -> Result<(), Error> {
        match key_text(self, "/pgf/data/store in group").filter(|s| !s.is_empty()) {
            Some(name) => {
                let name = format!("tikra@dv@group@{name}");
                let mut old = match self.tex.meaning_of(&name) {
                    Meaning::Macro(m) => tex::substitute(&m.body, &[]),
                    _ => Vec::new(),
                };
                old.extend(call);
                self.tex.set_meaning(&Token::cs(&name), macro_tokens(old), true);
                Ok(())
            }
            None => self.run_tokens(call),
        }
    }

    // ---------------------------------------------------------------- phases

    fn run_hook(&mut self, which: fn(&Hooks) -> &Vec<Token>) -> Result<(), Error> {
        let code = which(&self.dv()?.hooks).clone();
        self.run_tokens(code)
    }

    /// The survey: the data is produced once (or more, if objects ask for it), for the ranges.
    fn survey(&mut self) -> Result<(), Error> {
        self.dv()?.visualizing = false;
        self.dv()?.survey_count = 1;
        loop {
            self.emit(Signal::Phase, Some(Phase::BeforeSurvey))?;
            self.run_hook(|h| &h.before_survey)?;
            self.emit(Signal::Phase, Some(Phase::BeginSurvey))?;
            self.run_hook(|h| &h.at_start_survey)?;
            let data = self.dv()?.data.clone();
            self.run_tokens(data)?;
            self.run_hook(|h| &h.at_end_survey)?;
            self.emit(Signal::Phase, Some(Phase::EndSurvey))?;
            self.run_hook(|h| &h.after_survey)?;
            self.emit(Signal::Phase, Some(Phase::AfterSurvey))?;
            let dv = self.dv()?;
            if dv.survey_count >= dv.number_of_surveys || dv.survey_count > 10 {
                return Ok(());
            }
            dv.survey_count += 1;
        }
    }

    fn visualize(&mut self) -> Result<(), Error> {
        self.dv()?.visualizing = true;
        self.emit(Signal::Phase, Some(Phase::BeforeVisualization))?;
        self.run_hook(|h| &h.before_visualization)?;
        self.emit(Signal::Phase, Some(Phase::BeginVisualization))?;
        self.run_hook(|h| &h.at_start_visualization)?;
        let data = self.dv()?.data.clone();
        self.run_tokens(data)?;
        self.run_hook(|h| &h.at_end_visualization)?;
        self.emit(Signal::Phase, Some(Phase::EndVisualization))?;
        self.run_hook(|h| &h.after_visualization)?;
        self.emit(Signal::Phase, Some(Phase::AfterVisualization))
    }

    /// `\pgfdatapoint`: the signals for the data point now in `/data point/`.
    fn data_point(&mut self) -> Result<(), Error> {
        self.emit(Signal::Prepare, None)?;
        if !self.dv()?.visualizing {
            self.emit(Signal::Map, None)?;
            return self.emit(Signal::Survey, None);
        }
        let p = self.local_data_point()?;
        set_attribute(self, "canvas x", &format!("{}pt", num(p.x)));
        set_attribute(self, "canvas y", &format!("{}pt", num(p.y)));
        if let Some(name) = key_text(self, "/data point/name") {
            self.run_tokens(tikra_tokens(&format!("\\pgfcoordinate{{{name}}}{{{}}}", keys::qpoint(p))))?;
        }
        self.emit(Signal::Visualize, None)?;
        self.emit(Signal::Finish, None)
    }

    /// Emits a signal: each connected object's slot, in the order they connected.
    pub fn emit(&mut self, signal: Signal, phase: Option<Phase>) -> Result<(), Error> {
        let slots = self.dv()?.signals[signal as usize].clone();
        for (id, slot) in slots {
            self.slot(id, slot, phase)?;
        }
        Ok(())
    }

    /// `\pgfpointdvlocaldatapoint`: where the current data point is, in pt, with the transformation
    /// reset (the picture's applies when it is drawn).
    pub fn local_data_point(&mut self) -> Result<Point, Error> {
        self.begin_group();
        let r = (|| {
            self.emit(Signal::Map, None)?;
            self.transformed()
        })();
        self.end_group();
        r
    }

    /// The transform signal applied to the origin.
    fn transformed(&mut self) -> Result<Point, Error> {
        let slots = self.dv()?.signals[Signal::Transform as usize].clone();
        let mut p = Point::new(0.0, 0.0);
        for (id, _) in slots {
            p = self.transform_point(id, p);
        }
        Ok(p)
    }

    /// `\pgfdv@generic`: a path or direction step, which a polar transformer may take over.
    pub fn dv_generic(&mut self, signal: Signal, token: PathToken) -> Result<objects::Step, Error> {
        self.begin_group();
        let r = (|| {
            self.emit(Signal::Map, None)?;
            let slots = self.dv()?.signals[signal as usize].clone();
            let mut handled = None;
            for (id, _) in slots {
                if let Some(s) = self.path_slot(id, token, handled.is_some())? {
                    handled = Some(s);
                }
            }
            match handled {
                Some(s) => Ok(s),
                None => Ok(objects::Step::Point(self.transformed()?)),
            }
        })();
        self.end_group();
        r
    }

    /// `.method(args)`: the method's name and its arguments, split at commas.
    fn read_method(&mut self) -> Result<(String, Vec<Vec<Token>>), Error> {
        match self.tex.next_nonspace() {
            Some(t) if t.is_char('.') => {}
            other => {
                other.into_iter().for_each(|t| self.tex.back(t));
                return Ok((String::new(), Vec::new()));
            }
        }
        let mut name = String::new();
        while let Some(t) = self.tex.get_token() {
            if t.is_char('(') {
                break;
            }
            name.push_str(&tex::tokens_to_string(&[t]));
            if name.len() > 100 {
                return Err(Error::Figure("data visualization: method call without '('".into()));
            }
        }
        let inner = self.tex.read_until_char(')', true)?;
        let mut args = Vec::new();
        let mut current = Vec::new();
        let mut depth = 0usize;
        for t in inner {
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                Token::Char(',', _) if depth == 0 => {
                    args.push(tex::strip_outer_braces(crate::keys::trim(&current).to_vec()));
                    current.clear();
                    continue;
                }
                _ => {}
            }
            current.push(t);
        }
        args.push(tex::strip_outer_braces(crate::keys::trim(&current).to_vec()));
        Ok((name.trim().to_string(), args))
    }

    /// The object whose handle a key holds (`\tikra@dv@obj{n}`).
    pub fn object_of(&mut self, key: &str) -> Option<usize> {
        let v = crate::keys::value(&self.tex, key)?;
        handle_id(&v)
    }

    /// The object a macro holds.
    pub fn object_in(&mut self, name: &str) -> Option<usize> {
        let Meaning::Macro(m) = self.tex.meaning_of(name) else { return None };
        handle_id(&tex::substitute(&m.body, &[]))
    }

    // ---------------------------------------------------------------- bounding boxes

    /// The nodes `data visualization bounding box` (what was drawn so far) and, when `freeze`, `data
    /// bounding box` (frozen after the data, before the axes).
    fn update_bounding_box(&mut self, freeze: bool) {
        // nothing drawn yet: a box of no size at the origin
        let origin = self.st.transform.apply(Point::new(0.0, 0.0));
        let (lo, hi) = match (self.canvas.min, self.canvas.max) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => (origin, origin),
        };
        let c = Point::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0);
        let mut g = super::shapes::Geometry::new(c, super::shapes::Border::Rectangle(Point::new((hi.x - lo.x) / 2.0, (hi.y - lo.y) / 2.0)));
        for (name, x, y) in [
            ("north", c.x, hi.y),
            ("south", c.x, lo.y),
            ("east", hi.x, c.y),
            ("west", lo.x, c.y),
            ("north east", hi.x, hi.y),
            ("north west", lo.x, hi.y),
            ("south east", hi.x, lo.y),
            ("south west", lo.x, lo.y),
        ] {
            g.anchor_at(name, Point::new(x, y));
        }
        let info = super::nodes::NodeInfo { shape: "rectangle".into(), geometry: std::rc::Rc::new(g), transform: crate::scene::Transform::default() };
        if freeze {
            self.nodes.insert("data bounding box".into(), info.clone());
        }
        self.nodes.insert("data visualization bounding box".into(), info);
    }

    /// `(visualization cs:x=1,y=2)`: where that data point would be.
    pub fn visualization_cs(&mut self, args: &str) -> Result<Point, Error> {
        if self.datavis.is_empty() {
            return Err(Error::Figure("visualization cs outside a data visualization".into()));
        }
        self.begin_group();
        let r = (|| {
            self.keys_command(tikra_tokens(&format!("/data point/.cd,{args}")), Some("/"), true)?;
            self.local_data_point()
        })();
        self.end_group();
        let p = r?;
        Ok(self.st.transform.apply(Point::new(p.x * PT, p.y * PT)))
    }
}

/// `/pgf/data/.cd,/pgf/every data/.try,options`
fn data_keys(o: Vec<Token>) -> Vec<Token> {
    let mut v = tikra_tokens("/pgf/data/.cd,/pgf/every data/.try,");
    v.extend(o);
    v
}

/// The id in `\tikra@dv@obj{n}`.
fn handle_id(v: &[Token]) -> Option<usize> {
    let i = v.iter().position(|t| t.is_cs("tikra@dv@obj"))?;
    let s = tex::tokens_to_string(&v[i + 1..]);
    s.trim().trim_start_matches('{').split('}').next()?.trim().parse().ok()
}

/// `\tikra@dv@obj{n}` as tokens.
pub fn handle(id: usize) -> Vec<Token> {
    let mut v = vec![Token::cs("tikra@dv@obj")];
    v.extend(group(tex::string_tokens(&id.to_string())));
    v
}

/// Items at commas outside braces and parentheses (`(-2em,0),(0,0)`), as `\foreach` reads points.
pub fn point_split(v: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0i32;
    for t in v {
        match t {
            Token::Char(_, Cat::Begin) | Token::Char('(' | '[', _) => depth += 1,
            Token::Char(_, Cat::End) | Token::Char(')' | ']', _) => depth -= 1,
            Token::Char(',', _) if depth == 0 => {
                out.push(Vec::new());
                continue;
            }
            _ => {}
        }
        if let Some(l) = out.last_mut() {
            l.push(t.clone());
        }
    }
    out
}

/// Items at top-level commas (a list of handlers).
pub fn axes_split(v: &[Token]) -> Vec<Vec<Token>> {
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
        if let Some(l) = out.last_mut() {
            l.push(t.clone());
        }
    }
    out
}
