//! The objects of a data visualization (pgfmoduledatavisualization.code.tex and the polar library):
//! what each class keeps, its methods, and the slots its signals call.

use crate::keys::macro_name;
use crate::scene::Point;
use crate::tex::{self, Meaning, Token};
use crate::{Error, PT};

use super::{attribute, group, handle, key_text, num, parse_value, set_attribute, Engine, PathToken, Phase, Signal, Slot};
use crate::engine::{macro_from, macro_tokens, tikra_tokens};

/// The deforming function of a scaling (`function=`): none, `\pgfdvmathln` (logarithmic axes), or a
/// pgfmath function by name (`\pgfdvmathunaryop{…}{sqrt}{…}`).
#[derive(Debug, Clone, PartialEq)]
pub enum Function {
    Identity,
    Ln,
    Op(String),
}

impl Function {
    fn of(text: &str) -> Function {
        if text.trim().is_empty() {
            Function::Identity
        } else if text.contains("pgfdvmathln") {
            Function::Ln
        } else if let Some(i) = text.find("pgfdvmathunaryop") {
            let rest = &text[i..];
            let name = rest.split('{').nth(2).and_then(|s| s.split('}').next()).unwrap_or("").trim().to_string();
            Function::Op(name)
        } else {
            Function::Identity
        }
    }

    /// Whether `text` is a function this knows (an empty one is the identity).
    fn known(text: &str) -> bool {
        text.trim().is_empty() || text.contains("pgfdvmathln") || text.contains("pgfdvmathunaryop")
    }

    /// The warning when the function has no value at `v`.
    fn undefined(&self, input: &str, v: f64) -> String {
        match self {
            Function::Ln => format!("data visualization: logarithmic scaling of '{input}': {} has no logarithm: left out", num(v)),
            Function::Op(name) => format!("data visualization: scaling of '{input}': {name} has no value at {}: left out", num(v)),
            Function::Identity => format!("data visualization: scaling of '{input}': {} is not a finite number: left out", num(v)),
        }
    }

    fn apply(&self, e: &mut Engine, v: f64) -> Option<f64> {
        let r = match self {
            Function::Identity => v,
            Function::Ln => v.ln(),
            Function::Op(name) => e.number(&format!("{name}({})", num(v))).ok()?,
        };
        r.is_finite().then_some(r)
    }
}

/// How an interval mapper scales, once set up at the end of the survey.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scale {
    Unset,
    /// the in interval is a single value: everything maps to the middle of the out interval
    None,
    By(f64),
}

/// A scaling mapper, with its range surveyor, its in range interval and its interval mapper.
#[derive(Debug, Clone)]
pub struct Mapper {
    pub input: String,
    pub output: String,
    pub spec: String,
    pub function: Function,
    pub range: usize,
    pub scale: Scale,
    pub trans_in_min: f64,
    pub out_min: f64,
    pub out_max: f64,
    pub maps_min: bool,
    pub maps_max: bool,
}

#[derive(Debug, Clone)]
pub enum Object {
    /// the data visualization itself
    Main,
    Interval {
        min: Option<f64>,
        max: Option<f64>,
    },
    ScalingMapper(Box<Mapper>),
    LinearTransformer {
        attribute: String,
        origin: Point,
        unit: Point,
    },
    PolarTransformer {
        angle: String,
        radius: String,
        u0: Point,
        u90: Point,
        prev_angle: Option<f64>,
        prev_radius: Option<f64>,
    },
    PlotVisualizer {
        name: String,
        handlers: Vec<Token>,
        filter: Option<Vec<Token>>,
        projection: Vec<Token>,
        /// canvas positions in pt; None where an outlier breaks the line
        positions: Vec<Option<Point>>,
    },
    RectangleVisualizer {
        name: String,
        a1: String,
        a2: String,
    },
    LabelVisualizer {
        attribute: String,
        threshold: Option<f64>,
        code: Vec<Token>,
        filter: Vec<Token>,
        before: Option<Point>,
        at: Option<Point>,
    },
    StyleAttribute {
        attribute: String,
        sheet: String,
    },
    Legend {
        columns: bool,
        ideal: usize,
        max: usize,
        strategy: String,
        entries: Vec<Vec<Token>>,
    },
    Count {
        attribute: String,
        val: f64,
        max: f64,
        step: f64,
        start: f64,
        filter: Vec<Token>,
    },
    Accumulator {
        attribute: String,
        sum: f64,
    },
}

/// What a path step becomes: a point, or an arc that a polar transformer draws instead of a line.
#[derive(Debug, Clone, Copy)]
pub enum Step {
    Point(Point),
    /// from angle to angle, on the ellipse with these axes (pt)
    Arc {
        from: f64,
        to: f64,
        a: Point,
        b: Point,
    },
    /// a direction a polar transformer computed (normalized, pt)
    Direction(Point),
}

/// `\pgfqpoint{1pt}{0pt}` (or any two lengths in braces) → the point in pt.
fn point_of(e: &mut Engine, text: &str) -> Point {
    let parts: Vec<String> = text.split('{').skip(1).map(|s| s.split('}').next().unwrap_or("").to_string()).collect();
    if parts.len() < 2 {
        return Point::new(0.0, 0.0);
    }
    let x = e.length(&parts[0]).unwrap_or(0.0) / PT;
    let y = e.length(&parts[1]).unwrap_or(0.0) / PT;
    Point::new(x, y)
}

impl Engine {
    fn object(&mut self, id: usize) -> Option<&mut Object> {
        self.datavis.last_mut()?.objects.get_mut(id)
    }

    fn add_object(&mut self, o: Object) -> Result<usize, Error> {
        let dv = self.dv()?;
        dv.objects.push(o);
        Ok(dv.objects.len() - 1)
    }

    /// `new object={class=…, store=…, arg1=…, after creation=…}`, run at its time: the object is
    /// created unless the key `store` already holds one.
    pub fn new_object(&mut self, store: &str, spec: Vec<Token>) -> Result<(), Error> {
        if !store.is_empty() && key_text(self, store).is_some_and(|s| !s.is_empty()) {
            return Ok(());
        }
        let mut class = String::new();
        let mut args: Vec<Option<Vec<Token>>> = vec![None; 8];
        let mut after = Vec::new();
        for item in crate::keys::split_list(&spec) {
            let value = item.value.unwrap_or_default();
            let (key, handler) = match item.key.split_once("/.") {
                Some((k, h)) => (k.trim().to_string(), h.trim().to_string()),
                None => (item.key.trim().to_string(), String::new()),
            };
            let value = if handler.is_empty() { value } else { self.tex.expand_fully(value)? };
            match key.as_str() {
                "class" => class = self.tex.expand_to_string(value)?.trim().to_string(),
                "before creation" => self.run_tokens(value)?,
                "after creation" => after = value,
                k => {
                    let Some(rest) = k.strip_prefix("arg") else { continue };
                    let (n, from_key) = match rest.split_once(' ') {
                        Some((n, r)) => (n, r.trim() == "from key"),
                        None => (rest, false),
                    };
                    let Some(i) = n.parse::<usize>().ok().filter(|i| (1..=8).contains(i)) else { continue };
                    args[i - 1] = Some(if from_key {
                        let key = self.tex.expand_to_string(value)?;
                        crate::keys::value(&self.tex, key.trim()).unwrap_or_default()
                    } else {
                        value
                    });
                }
            }
        }
        let args: Vec<Vec<Token>> = args.into_iter().map_while(|a| a).collect();
        let Some(id) = self.create(&class, &args)? else { return Ok(()) };
        if !store.is_empty() {
            self.tex.set_meaning(&Token::cs(&macro_name(store)), macro_tokens(handle(id)), false);
        }
        self.tex.set_meaning(&Token::cs("tikzdvobj"), macro_tokens(handle(id)), false);
        self.run_tokens(after)
    }

    /// An object of a class, from its constructor's arguments, connected to its signals.
    pub fn create(&mut self, class: &str, args: &[Vec<Token>]) -> Result<Option<usize>, Error> {
        let text = |e: &mut Engine, i: usize| -> String { args.get(i).map(|v| e.tex.expand_to_string(v.clone()).unwrap_or_default().trim().to_string()).unwrap_or_default() };
        let id = match class {
            "scaling mapper" => {
                let range = self.add_object(Object::Interval { min: None, max: None })?;
                let m = Mapper {
                    input: text(self, 0),
                    output: text(self, 1),
                    spec: text(self, 2),
                    function: {
                        let f = text(self, 3);
                        if !Function::known(&f) {
                            self.warn(format!("data visualization: scaling function '{f}' not supported: the identity instead"));
                        }
                        Function::of(&f)
                    },
                    range,
                    scale: Scale::Unset,
                    trans_in_min: 0.0,
                    out_min: 0.0,
                    out_max: 0.0,
                    maps_min: false,
                    maps_max: false,
                };
                let id = self.add_object(Object::ScalingMapper(Box::new(m)))?;
                let dv = self.dv()?;
                dv.connect(id, Slot::Survey, Signal::Survey);
                dv.connect(id, Slot::Map, Signal::Map);
                dv.connect(id, Slot::Phase, Signal::Phase);
                id
            }
            "linear transformer" => {
                let attribute = text(self, 0);
                let unit = text(self, 1);
                let unit = point_of(self, &unit);
                let id = self.add_object(Object::LinearTransformer { attribute, origin: Point::new(0.0, 0.0), unit })?;
                self.dv()?.connect(id, Slot::Transform, Signal::Transform);
                id
            }
            "polar transformer" => {
                let angle = text(self, 0);
                let radius = text(self, 1);
                let (a, b) = (text(self, 2), text(self, 3));
                let (u0, u90) = (point_of(self, &a), point_of(self, &b));
                let id = self.add_object(Object::PolarTransformer { angle, radius, u0, u90, prev_angle: None, prev_radius: None })?;
                let dv = self.dv()?;
                dv.connect(id, Slot::Transform, Signal::Transform);
                dv.connect(id, Slot::Path, Signal::Path);
                dv.connect(id, Slot::Direction, Signal::Direction);
                id
            }
            "plot handler visualizer" => {
                let name = text(self, 0);
                let handlers = args.get(1).cloned().unwrap_or_default();
                let id = self.add_object(Object::PlotVisualizer { name, handlers, filter: None, projection: Vec::new(), positions: Vec::new() })?;
                let dv = self.dv()?;
                dv.connect(id, Slot::Protocol, Signal::Visualize);
                dv.connect(id, Slot::Phase, Signal::Phase);
                id
            }
            "rectangle visualizer" => {
                let name = text(self, 0);
                let id = self.add_object(Object::RectangleVisualizer { name, a1: String::new(), a2: String::new() })?;
                let dv = self.dv()?;
                dv.connect(id, Slot::Visualize, Signal::Visualize);
                dv.connect(id, Slot::Phase, Signal::Phase);
                id
            }
            "label visualizer" => {
                let attribute = text(self, 0);
                let threshold = text(self, 1);
                let threshold = parse_value(self, &threshold);
                let code = args.get(2).cloned().unwrap_or_default();
                let filter = args.get(3).cloned().unwrap_or_default();
                let id = self.add_object(Object::LabelVisualizer { attribute, threshold, code, filter, before: None, at: None })?;
                let dv = self.dv()?;
                dv.connect(id, Slot::Test, Signal::Visualize);
                dv.connect(id, Slot::Phase, Signal::Phase);
                id
            }
            "count" => {
                let attribute = text(self, 0);
                let id = self.add_object(Object::Count { attribute, val: 0.0, max: 0.0, step: 1.0, start: 0.0, filter: Vec::new() })?;
                let dv = self.dv()?;
                dv.connect(id, Slot::Apply, Signal::Prepare);
                dv.connect(id, Slot::Phase, Signal::Phase);
                id
            }
            "accumulator" => {
                let attribute = text(self, 0);
                let id = self.add_object(Object::Accumulator { attribute: attribute.clone(), sum: 0.0 })?;
                set_attribute(self, &format!("{attribute}/sum"), "0");
                set_attribute(self, &format!("{attribute}/prev sum"), "0");
                let dv = self.dv()?;
                dv.connect(id, Slot::Apply, Signal::Prepare);
                dv.connect(id, Slot::Phase, Signal::Phase);
                id
            }
            "legend" => {
                let columns = text(self, 0) == "columns";
                let ideal = text(self, 1).parse().unwrap_or(1);
                let max = text(self, 2).parse().unwrap_or(100);
                let strategy = text(self, 3);
                self.add_object(Object::Legend { columns, ideal, max, strategy, entries: Vec::new() })?
            }
            "interval" => self.add_object(Object::Interval { min: None, max: None })?,
            "" => return Ok(None),
            other => {
                self.warn(format!("data visualization: class '{other}' not supported"));
                return Ok(None);
            }
        };
        Ok(Some(id))
    }

    /// A method call on object `id`.
    pub fn call(&mut self, id: usize, method: &str, args: Vec<Vec<Token>>) -> Result<(), Error> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let target = |v: &[Token]| v.iter().find(|t| !t.is_space()).cloned();
        let Some(o) = self.object(id).cloned() else {
            self.warn(format!("data visualization: no object {id} for .{method}()"));
            return Ok(());
        };
        match (o, method) {
            (_, "default connects") => {}
            (Object::Main, "add data") => {
                let code = tex::strip_outer_braces(crate::keys::trim(&arg(0)).to_vec());
                self.dv()?.data.extend(code);
            }
            (Object::Main, m) if m.starts_with("before ") || m.starts_with("after ") || m.starts_with("at ") => {
                let code = arg(0);
                let h = &mut self.dv()?.hooks;
                match m {
                    "before survey" => h.before_survey.extend(code),
                    "at start survey" => h.at_start_survey.extend(code),
                    "before visualization" => h.before_visualization.extend(code),
                    "at start visualization" => h.at_start_visualization.extend(code),
                    "at end survey" => h.at_end_survey = [code, std::mem::take(&mut h.at_end_survey)].concat(),
                    "after survey" => h.after_survey = [code, std::mem::take(&mut h.after_survey)].concat(),
                    "at end visualization" => h.at_end_visualization = [code, std::mem::take(&mut h.at_end_visualization)].concat(),
                    "after visualization" => h.after_visualization = [code, std::mem::take(&mut h.after_visualization)].concat(),
                    _ => {}
                }
            }
            (Object::Main, "get survey count") => {
                let n = self.dv()?.survey_count;
                if let Some(t) = target(&arg(0)) {
                    self.tex.set_meaning(&t, macro_from(&n.to_string()), false);
                }
            }
            (Object::Main, "do an additional survey") => self.dv()?.number_of_surveys += 1,
            (Object::Main, _) => {}
            (Object::ScalingMapper(m), "get in range interval") => {
                self.tex.set_meaning(&Token::cs("pgfdvinrangeinterval"), macro_tokens(handle(m.range)), false);
            }
            (Object::ScalingMapper(m), "get out") => self.tex.define_string("pgfdvout", &m.output, false),
            (Object::ScalingMapper(m), "get function") => {
                if let Some(t) = target(&arg(0)) {
                    let v = if m.function == Function::Identity { String::new() } else { "\\pgfdvmathln".into() };
                    self.tex.set_meaning(&t, macro_from(&v), false);
                }
            }
            (Object::ScalingMapper(_), "include in value") => {
                let s = self.tex.expand_to_string(arg(0))?;
                if let Some(v) = parse_value(self, &s) {
                    self.include_value(id, v);
                }
            }
            (Object::ScalingMapper(_), "set in to") => {
                let s = self.tex.expand_to_string(arg(0))?;
                self.set_in_to(id, &s);
            }
            (Object::Interval { min, max }, "get min and max") => {
                self.tex.define_string("pgfdvmin", &min.map(num).unwrap_or_default(), false);
                self.tex.define_string("pgfdvmax", &max.map(num).unwrap_or_default(), false);
            }
            (Object::Interval { .. }, m @ ("set min" | "set max" | "let min" | "let max")) => {
                let s = self.tex.expand_to_string(arg(0))?;
                let v = parse_value(self, &s);
                let Some(Object::Interval { min, max }) = self.object(id) else { unreachable!("object {id} is the interval just called") };
                if m.ends_with("min") {
                    *min = v;
                } else {
                    *max = v;
                }
            }
            (Object::Count { max, .. }, "get max") => {
                if let Some(t) = target(&arg(0)) {
                    self.tex.set_meaning(&t, macro_from(&num(max)), false);
                }
            }
            (Object::Count { .. }, m @ ("set filter" | "set value" | "set start value" | "set step")) => {
                let code = arg(0);
                let v = parse_value(self, &tex::tokens_to_string(&code)).unwrap_or(0.0);
                let Some(Object::Count { filter, val, start, step, max, .. }) = self.object(id) else { unreachable!("object {id} is the count just called") };
                match m {
                    "set filter" => *filter = code,
                    "set value" => *val = v,
                    "set start value" => {
                        *start = v;
                        *max = v;
                    }
                    _ => *step = v,
                }
            }
            (Object::Legend { .. }, "add entry") => {
                if let Some(Object::Legend { entries, .. }) = self.object(id) {
                    entries.push(arg(0));
                }
            }
            (Object::Legend { .. }, "get arrangement") => {
                let v = self.legend_arrangement(id);
                if let Some(t) = target(&arg(0)) {
                    self.tex.set_meaning(&t, macro_tokens(v), false);
                }
            }
            (Object::PlotVisualizer { .. }, m @ ("set filter" | "set projection code")) => {
                let code = arg(0);
                let Some(Object::PlotVisualizer { filter, projection, .. }) = self.object(id) else { unreachable!("object {id} is the visualizer just called") };
                if m == "set filter" {
                    *filter = Some(code);
                } else {
                    *projection = code;
                }
            }
            (Object::LinearTransformer { .. }, "set origin") => {
                let s = self.tex.expand_to_string(arg(0))?;
                let p = point_of(self, &s);
                if let Some(Object::LinearTransformer { origin, .. }) = self.object(id) {
                    *origin = p;
                }
            }
            (Object::PolarTransformer { angle, radius, .. }, m @ ("get angle attribute" | "get radius attribute")) => {
                if let Some(t) = target(&arg(0)) {
                    self.tex.set_meaning(&t, macro_from(if m.contains("angle") { &angle } else { &radius }), false);
                }
            }
            (Object::Accumulator { attribute, .. }, "reset") => self.reset_accumulator(id, &attribute),
            (_, m) => self.warn(format!("data visualization: method '{m}' not supported")),
        }
        Ok(())
    }

    // ---------------------------------------------------------------- scaling mappers

    fn mapper(&mut self, id: usize) -> Option<Mapper> {
        let Some(Object::ScalingMapper(m)) = self.object(id) else { return None };
        Some((**m).clone())
    }

    /// The range an axis' survey found.
    pub fn range_of(&mut self, mapper: usize) -> (Option<f64>, Option<f64>) {
        let Some(m) = self.mapper(mapper) else { return (None, None) };
        let Some(Object::Interval { min, max }) = self.object(m.range) else { unreachable!("a mapper's range is an interval") };
        (*min, *max)
    }

    pub fn include_value(&mut self, mapper: usize, v: f64) {
        let Some(m) = self.mapper(mapper) else { return };
        let Some(Object::Interval { min, max }) = self.object(m.range) else { unreachable!("a mapper's range is an interval") };
        *min = Some(min.map_or(v, |x| x.min(v)));
        *max = Some(max.map_or(v, |x| x.max(v)));
    }

    /// `min value=…`, `max value=…`: the end of the range, set.
    pub fn set_range(&mut self, mapper: usize, which: &str, v: f64) {
        let Some(m) = self.mapper(mapper) else { return };
        let Some(Object::Interval { min, max }) = self.object(m.range) else { unreachable!("a mapper's range is an interval") };
        if which == "min" {
            *min = Some(v);
        } else {
            *max = Some(v);
        }
    }

    /// `set in to(value)`: the axis' attribute set to a value, or to the range's `min` or `max`.
    pub fn set_in_to(&mut self, mapper: usize, value: &str) {
        let Some(m) = self.mapper(mapper) else { return };
        let (lo, hi) = self.range_of(mapper);
        let v = match value.trim() {
            "min" => {
                if m.maps_min {
                    set_attribute(self, &format!("{}/is min or max", m.input), "min");
                }
                lo
            }
            "max" => {
                // pgf tests `spec maps min` here too
                if m.maps_min {
                    set_attribute(self, &format!("{}/is min or max", m.input), "max");
                }
                hi
            }
            other => parse_value(self, other),
        };
        set_attribute(self, &m.input, &v.map(num).unwrap_or_default());
    }

    /// The end of the survey: the interval mapper is set up from the range and the scaling.
    fn setup_mapper(&mut self, id: usize) {
        let Some(m) = self.mapper(id) else { return };
        let (Some(lo), Some(hi)) = self.range_of(id) else { return };
        // #1 at#2and #3 at#4
        let spec = m.spec.clone();
        let (first, second) = spec.split_once(" and ").unwrap_or((&spec, ""));
        let (a, c) = first.split_once(" at").unwrap_or((first, "0"));
        let (b, d) = second.split_once(" at").unwrap_or((second, "1"));
        let value = |e: &mut Engine, s: &str| {
            parse_value(e, s).unwrap_or_else(|| {
                e.warn(format!("data visualization: '{}' in the scaling '{spec}' is not a number or a length: 0 instead", s.trim()));
                0.0
            })
        };
        let (maps_min, a) = if a.trim() == "min" { (true, lo) } else { (false, value(self, a)) };
        let (maps_max, b) = if b.trim() == "max" { (true, hi) } else { (false, value(self, b)) };
        let (c, d) = (value(self, c), value(self, d));
        let f = m.function.clone();
        let (fa, fb) = (f.apply(self, a), f.apply(self, b));
        for (v, fv) in [(a, fa), (b, fb)] {
            if fv.is_none() {
                let w = f.undefined(&m.input, v);
                self.warn(w);
            }
        }
        let (scale, trans) = match (a < b, fa, fb) {
            (true, Some(fa), Some(fb)) if fb != fa => (Scale::By((d - c) / (fb - fa)), fa),
            (true, _, _) => (Scale::Unset, 0.0),
            (false, _, _) => (Scale::None, 0.0),
        };
        if let Some(Object::ScalingMapper(m)) = self.object(id) {
            m.out_min = c;
            m.out_max = d;
            m.maps_min = maps_min;
            m.maps_max = maps_max;
            m.scale = scale;
            m.trans_in_min = trans;
        }
    }

    /// The map slot: the in attribute, scaled, into the out attribute.
    fn map_slot(&mut self, id: usize) {
        let Some(m) = self.mapper(id) else { return };
        if m.scale == Scale::Unset {
            return;
        }
        let offset = attribute(self, &format!("{}/offset", m.output)).and_then(|s| parse_value(self, &s)).unwrap_or(0.0);
        let is = attribute(self, &format!("{}/is min or max", m.input)).unwrap_or_default();
        let out = if is == "min" || is == "max" {
            set_attribute(self, &format!("{}/is min or max", m.output), &is);
            Some(if is == "min" { m.out_min } else { m.out_max } + offset)
        } else if m.scale == Scale::None {
            Some((m.out_min + m.out_max) / 2.0)
        } else {
            let v = attribute(self, &format!("{}/const", m.input)).or_else(|| attribute(self, &m.input));
            match v {
                Some(v) => {
                    let Scale::By(scale) = m.scale else { return };
                    let x = parse_value(self, &v);
                    let fx = x.and_then(|x| m.function.apply(self, x));
                    if let (Some(x), None) = (x, fx) {
                        let w = m.function.undefined(&m.input, x);
                        self.warn(w);
                    }
                    fx.map(|fx| (fx - m.trans_in_min) * scale + m.out_min + offset)
                }
                None => match attribute(self, &format!("{}/out pos", m.input)) {
                    Some(p) => {
                        let p = parse_value(self, &p).unwrap_or(0.0);
                        Some((1.0 - p) * m.out_min + p * m.out_max)
                    }
                    None => None,
                },
            }
        };
        if let Some(v) = out {
            set_attribute(self, &m.output, &num(v));
        }
    }

    /// The survey slot: the attribute (and its sub attributes) widen the range.
    fn survey_slot(&mut self, id: usize) {
        let Some(m) = self.mapper(id) else { return };
        let mut subs = vec![String::new()];
        if let Some(s) = key_text(self, &format!("/data point/{}/sub attributes", m.input)) {
            for part in s.split("\\pgfdvsub").skip(1) {
                subs.push(part.trim().trim_start_matches('{').split('}').next().unwrap_or("").to_string());
            }
        }
        for sub in subs {
            if let Some(v) = attribute(self, &format!("{}{sub}", m.input)) {
                if let Some(x) = parse_value(self, &v) {
                    self.include_value(id, x);
                }
            }
        }
    }

    // ---------------------------------------------------------------- transformers

    /// The transform slot of a transformer: `p` shifted for the current data point.
    pub fn transform_point(&mut self, id: usize, p: Point) -> Point {
        let o = self.object(id).cloned();
        if let Some(Object::LinearTransformer { attribute: a, origin, unit }) = o {
            let Some(v) = attribute(self, &a).and_then(|s| parse_value(self, &s)) else { return p };
            let p = p.plus(origin);
            // values below 10^-3 are not shifted by (pgf's TeX dimensions would overflow)
            if v != 0.0 && v.abs() < 1e-3 {
                return p;
            }
            return p.plus(unit.scale(v));
        }
        let Some(Object::PolarTransformer { angle, radius, u0, u90, .. }) = o else { unreachable!("only transformers are connected to the transform signal") };
        let get = |e: &mut Engine, a: &str| attribute(e, &format!("{a}/const")).or_else(|| attribute(e, a)).and_then(|s| parse_value(e, &s));
        let (Some(a), Some(r)) = (get(self, &angle), get(self, &radius)) else { return p };
        let (s, c) = a.to_radians().sin_cos();
        p.plus(u0.scale(c * r)).plus(u90.scale(s * r))
    }

    /// The path and direction slots of a polar transformer; Some when it handles the step.
    pub fn path_slot(&mut self, id: usize, token: PathToken, handled: bool) -> Result<Option<Step>, Error> {
        let Some(Object::PolarTransformer { angle, radius, u0, u90, prev_angle, prev_radius }) = self.object(id).cloned() else { return Ok(None) };
        let a = attribute(self, &angle).and_then(|s| parse_value(self, &s));
        let r = attribute(self, &radius).and_then(|s| parse_value(self, &s));
        let same = |x: Option<f64>, y: Option<f64>| x.zip(y).is_some_and(|(x, y)| (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0));
        let mut step = None;
        match token {
            PathToken::MoveTo | PathToken::LineTo => {
                if !handled && token == PathToken::LineTo && same(r, prev_radius) && !same(a, prev_angle) {
                    if let (Some(from), Some(to), Some(r)) = (prev_angle, a, r) {
                        step = Some(Step::Arc { from, to, a: u0.scale(r), b: u90.scale(r) });
                    }
                }
                if let Some(Object::PolarTransformer { prev_angle, prev_radius, .. }) = self.object(id) {
                    *prev_angle = a;
                    *prev_radius = r;
                }
            }
            PathToken::DirectionFrom => self.dv()?.polar_from = (a, r),
            PathToken::DirectionTo => self.dv()?.polar_to = (a, r),
            PathToken::DirectionAt => {
                let (from, to) = (self.dv()?.polar_from, self.dv()?.polar_to);
                if same(from.1, to.1) {
                    if let Some(a) = a {
                        let sign = if from.0.unwrap_or(0.0) < to.0.unwrap_or(0.0) { 90.0 } else { -90.0 };
                        let (s, c) = (a + sign).to_radians().sin_cos();
                        step = Some(Step::Direction(Point::new(c, s)));
                    }
                }
            }
        }
        Ok(step)
    }

    // ---------------------------------------------------------------- slots

    /// Runs filter code (`\pgfdvfilterpassedtrue`, the code, then `\ifpgfdvfilterpassed`).
    fn filter_passes(&mut self, code: &[Token]) -> Result<bool, Error> {
        if code.is_empty() {
            return Ok(true);
        }
        self.tex.set_meaning(&Token::cs("ifpgfdvfilterpassed"), Meaning::Prim(tex::Prim::Iftrue), false);
        self.run_tokens(code.to_vec())?;
        Ok(matches!(self.tex.meaning_of("ifpgfdvfilterpassed"), Meaning::Prim(tex::Prim::Iftrue)))
    }

    pub fn slot(&mut self, id: usize, slot: Slot, phase: Option<Phase>) -> Result<(), Error> {
        let Some(o) = self.object(id).cloned() else { return Ok(()) };
        match (o, slot) {
            (Object::ScalingMapper(_), Slot::Survey) => self.survey_slot(id),
            (Object::ScalingMapper(_), Slot::Map) => self.map_slot(id),
            (Object::ScalingMapper(_), Slot::Phase) => {
                if phase == Some(Phase::EndSurvey) {
                    self.setup_mapper(id);
                }
            }
            (Object::PlotVisualizer { name, filter, projection, .. }, Slot::Protocol) => {
                let passes = match filter {
                    Some(code) => self.filter_passes(&code)?,
                    None => attribute(self, "set").unwrap_or_default() == name,
                };
                if passes {
                    let p = if attribute(self, "outlier").is_some() {
                        None
                    } else {
                        self.begin_group();
                        let r = self.run_tokens(projection);
                        let x = attribute(self, "canvas x").map(|s| self.length(&s).unwrap_or(0.0) / PT);
                        let y = attribute(self, "canvas y").map(|s| self.length(&s).unwrap_or(0.0) / PT);
                        self.end_group();
                        r?;
                        Some(Point::new(x.unwrap_or(0.0), y.unwrap_or(0.0)))
                    };
                    if let Some(Object::PlotVisualizer { positions, .. }) = self.object(id) {
                        positions.push(p);
                    }
                }
            }
            (Object::PlotVisualizer { .. }, Slot::Phase) => {
                if phase == Some(Phase::EndVisualization) {
                    self.render_visualizer(id)?;
                }
            }
            (Object::RectangleVisualizer { name, .. }, Slot::Phase) => {
                if phase == Some(Phase::BeginSurvey) {
                    let a1 = format!("/data point/{}", key_text(self, &format!("/data point/{name}/attribute 1")).unwrap_or_default());
                    let a2 = format!("/data point/{}", key_text(self, &format!("/data point/{name}/attribute 2")).unwrap_or_default());
                    for a in [&a1, &a2] {
                        self.tex.set_meaning(&Token::cs(&macro_name(&format!("{a}/sub attributes"))), macro_tokens(tikra_tokens("\\pgfdvsub{/min}\\pgfdvsub{/max}")), false);
                    }
                    if let Some(Object::RectangleVisualizer { a1: x, a2: y, .. }) = self.object(id) {
                        *x = a1;
                        *y = a2;
                    }
                }
            }
            (Object::RectangleVisualizer { name, a1, a2 }, Slot::Visualize) => {
                if attribute(self, "set").unwrap_or_default() == name {
                    self.render_rectangle(&name, &a1, &a2)?;
                }
            }
            (Object::LabelVisualizer { attribute: a, threshold, filter, at, .. }, Slot::Test) => {
                if !a.is_empty() && self.filter_passes(&filter)? {
                    let x = attribute(self, "canvas x").map(|s| self.length(&s).unwrap_or(0.0) / PT).unwrap_or(0.0);
                    let y = attribute(self, "canvas y").map(|s| self.length(&s).unwrap_or(0.0) / PT).unwrap_or(0.0);
                    let stop = threshold.is_some_and(|t| attribute(self, &a).and_then(|s| parse_value(self, &s)).is_some_and(|v| v >= t));
                    let Some(Object::LabelVisualizer { before, at: here, attribute, .. }) = self.object(id) else { unreachable!("object {id} is the label visualizer") };
                    *before = at;
                    *here = Some(Point::new(x, y));
                    if stop {
                        attribute.clear();
                    }
                }
            }
            (Object::LabelVisualizer { code, before, at, .. }, Slot::Phase) => {
                if phase == Some(Phase::EndVisualization) {
                    let (Some(at), before) = (at, before) else { return Ok(()) };
                    let before = before.unwrap_or(at);
                    let mut v = tikra_tokens("\\begin{scope}");
                    v.extend(tikra_tokens(&format!("\\pgfcoordinate{{label visualizer coordinate}}{{{}}}", super::keys::qpoint(at))));
                    v.extend(tikra_tokens(&format!("\\pgfcoordinate{{label visualizer coordinate'}}{{{}}}", super::keys::qpoint(before))));
                    v.extend(code);
                    v.extend(tikra_tokens("\\end{scope}"));
                    self.run_tokens(v)?;
                }
            }
            (Object::StyleAttribute { attribute: a, sheet }, Slot::InvokeStyle) => self.invoke_style(&a, &sheet)?,
            (Object::Count { attribute: a, val, max, step, filter, .. }, Slot::Apply) => {
                if self.filter_passes(&filter)? {
                    let v = val + step;
                    let m = if v > max { v } else { max };
                    set_attribute(self, &a, &num(v));
                    set_attribute(self, &format!("{a}/max"), &num(m));
                    if let Some(Object::Count { val, max, .. }) = self.object(id) {
                        *val = v;
                        *max = m;
                    }
                }
            }
            (Object::Count { start, .. }, Slot::Phase) => {
                if matches!(phase, Some(Phase::BeginSurvey | Phase::BeginVisualization)) {
                    if let Some(Object::Count { val, .. }) = self.object(id) {
                        *val = start;
                    }
                }
            }
            (Object::Accumulator { attribute: a, sum }, Slot::Apply) => {
                let prev = attribute(self, &format!("{a}/sum")).unwrap_or_else(|| "0".into());
                set_attribute(self, &format!("{a}/prev sum"), &prev);
                let v = attribute(self, &a).and_then(|s| parse_value(self, &s)).unwrap_or(0.0);
                let s = sum + v;
                set_attribute(self, &format!("{a}/sum"), &num(s));
                if let Some(Object::Accumulator { sum, .. }) = self.object(id) {
                    *sum = s;
                }
            }
            // the last: an accumulator's phase (the transformers' slots are called directly, not
            // by a signal, and no other object connects to a slot it does not have)
            (o, _) => {
                let Object::Accumulator { attribute: a, .. } = o else { unreachable!("object {id} has no slot {slot:?}") };
                if matches!(phase, Some(Phase::BeginSurvey | Phase::BeginVisualization)) {
                    self.reset_accumulator(id, &a);
                }
            }
        }
        Ok(())
    }

    fn reset_accumulator(&mut self, id: usize, a: &str) {
        if let Some(Object::Accumulator { sum, .. }) = self.object(id) {
            *sum = 0.0;
        }
        set_attribute(self, &format!("{a}/sum"), "0");
        set_attribute(self, &format!("{a}/prev sum"), "0");
    }

    /// A style attribute: the style sheet's style for the attribute's value (or its default style).
    fn invoke_style(&mut self, attr: &str, sheet: &str) -> Result<(), Error> {
        let Some(mut value) = key_text(self, attr).filter(|s| !s.is_empty()) else { return Ok(()) };
        if let Some(redirect) = key_text(self, &format!("{attr}/{value}")) {
            value = redirect;
        }
        let base = format!("/pgf/data visualization/style sheets/{sheet}");
        let key = format!("{base}/{value}");
        let assignable = self.tex.is_defined(&macro_name(&format!("{key}/.@cmd"))) || self.tex.is_defined(&macro_name(&key));
        let default = format!("{base}/default style");
        let mut list = tex::string_tokens(if assignable { &key } else { &default });
        if !assignable {
            list.push(Token::other('='));
            list.extend(group(tex::string_tokens(&value)));
        }
        self.keys_command(list, None, false)
    }
}
