//! Axes, ticks, grids and axis labels, drawn after the data, and the tick placement strategies: `step=`,
//! `about=` (`few`, `some`, `many`) with the about strategies, linear and exponential steps, and
//! `minor steps between steps`.
//!
//! Positions come from the axes' objects, as pgf computes them: an attribute is set (`goto`), the
//! data point is mapped and transformed, and the point (pt, the picture's transformation not
//! applied) is where the line, tick or label goes. A tick's direction is that of its direction
//! axis between its minimum and its maximum; the anchor of its text is the one TikZ's `auto` would
//! give for that direction.

use crate::keys::macro_name;
use crate::scene::{Point, Transform};
use crate::tex::{self, Cat, Meaning, Token};
use crate::{Error, PT};

use super::objects::Step;
use super::{group, key_text, macro_text, num, parse_value, set_attribute, Engine, PathToken, Signal};
use crate::engine::{macro_tokens, tikra_tokens};

/// A point as TikZ coordinates in pt (the picture's transformation applies to it).
pub fn pt(p: Point) -> String {
    format!("({}pt,{}pt)", num(p.x), num(p.y))
}

/// The anchors TikZ's `auto` gives a node beside a direction: on its left, and on its right.
fn auto_anchors(d: Point) -> (&'static str, &'static str) {
    let (x, y) = (d.x, d.y);
    let left = if x > 0.05 {
        if y > 0.05 {
            "south east"
        } else if y < -0.05 {
            "south west"
        } else {
            "south"
        }
    } else if x < -0.05 {
        if y > 0.05 {
            "north east"
        } else if y < -0.05 {
            "north west"
        } else {
            "north"
        }
    } else if y > 0.0 {
        "east"
    } else {
        "west"
    };
    let right = if x > 0.05 {
        if y > 0.05 {
            "north west"
        } else if y < -0.05 {
            "north east"
        } else {
            "north"
        }
    } else if x < -0.05 {
        if y > 0.05 {
            "south west"
        } else if y < -0.05 {
            "south east"
        } else {
            "south"
        }
    } else if y > 0.0 {
        "west"
    } else {
        "east"
    };
    (left, right)
}

fn normalized(p: Point) -> Point {
    let n = (p.x * p.x + p.y * p.y).sqrt();
    if n < 1e-12 {
        Point::new(0.0, 0.0)
    } else {
        p.scale(1.0 / n)
    }
}

impl Engine {
    /// `before creation` of an axis' scaling mapper: `scaling` (or its default) with its `at`
    /// values evaluated.
    pub fn prepare_scaling(&mut self, axis: &str) -> Result<(), Error> {
        let mut spec = key_text(self, &format!("{axis}/scaling")).unwrap_or_default();
        if spec.is_empty() {
            spec = key_text(self, &format!("{axis}/scaling/default")).unwrap_or_default();
        }
        let (first, second) = spec.split_once(" and ").unwrap_or((&spec, ""));
        let (a, c) = first.split_once(" at").unwrap_or((first, "0"));
        let (b, d) = second.split_once(" at").unwrap_or((second, "1"));
        let eval = |e: &mut Engine, s: &str| -> String {
            let s = s.trim();
            let s = s.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(s);
            match crate::expr::eval_length(&e.math_string(tex::string_tokens(s)).unwrap_or_default(), &e.vars()) {
                Ok((v, true)) => num(v / PT),
                Ok((v, false)) => num(v),
                Err(_) => s.to_string(),
            }
        };
        let (c, d) = (eval(self, c), eval(self, d));
        let v = format!("{} at {c} and {} at {d}", a.trim(), b.trim());
        self.tex.define_string(&macro_name(&format!("{axis}/scaling")), &v, false);
        Ok(())
    }

    /// `goto=value` for an axis: its attribute set to the value, `min`, `max`, `padded min` or
    /// `padded max` (with the padding as an offset of the scaled attribute).
    pub fn goto(&mut self, axis: &str, value: &str) -> Result<(), Error> {
        let Some(mapper) = self.object_of(&format!("{axis}/scaling mapper")) else { return Ok(()) };
        match value.trim() {
            v @ ("padded min" | "padded max") => {
                let end = &v[7..];
                self.set_in_to(mapper, end);
                let out = self.mapper_output(mapper);
                let padding = key_text(self, &format!("{axis}/padding {end}")).unwrap_or_default();
                let p = if padding.trim().is_empty() { 0.0 } else { self.length(&padding)? / PT };
                set_attribute(self, &format!("{out}/offset"), &num(p));
            }
            v => self.set_in_to(mapper, v),
        }
        Ok(())
    }

    fn mapper_output(&mut self, mapper: usize) -> String {
        let Some(super::Object::ScalingMapper(m)) = self.datavis.last().and_then(|d| d.objects.get(mapper)) else { return String::new() };
        m.output.clone()
    }

    /// `\tikzpointandanchordirection`: the direction of the axis (from its minimum to its maximum)
    /// at the current data point, and the anchors for texts below (min) and above (max) it.
    fn direction(&mut self, mapper: usize) -> Result<(Point, &'static str, &'static str), Error> {
        let mut ends = [Point::new(0.0, 0.0); 2];
        for (i, end) in ["min", "max"].iter().enumerate() {
            self.begin_group();
            self.set_in_to(mapper, end);
            let token = if i == 0 { PathToken::DirectionFrom } else { PathToken::DirectionTo };
            let r = self.dv_generic(Signal::Direction, token);
            self.end_group();
            if let Step::Point(p) = r? {
                ends[i] = p;
            }
        }
        let d = match self.dv_generic(Signal::Direction, PathToken::DirectionAt)? {
            Step::Direction(d) => d,
            _ => normalized(ends[1].minus(ends[0])),
        };
        // the anchors look at the direction turned by -90 degrees
        let (max, min) = auto_anchors(Point::new(d.y, -d.x));
        self.tex.define_string("tikra@dv@maxanchor", max, true);
        self.tex.define_string("tikra@dv@minanchor", min, true);
        Ok((d, max, min))
    }

    /// `anchor at min`, `anchor at max`: the node anchor for a text at that end of the axis.
    pub fn anchor_at(&mut self, axis: &str, which: &str) -> Result<(), Error> {
        let Some(mapper) = self.object_of(&format!("{axis}/scaling mapper")) else { return Ok(()) };
        let (_, max, min) = self.direction(mapper)?;
        let a = if which == "min" { min } else { max };
        self.keys_command(tikra_tokens(&format!("/tikz/anchor={a}")), None, false)
    }

    /// A path step of a data visualization path (`\pgfpathdvmoveto`, `\pgfpathdvlineto`).
    fn path_step(&mut self, token: PathToken) -> Result<Step, Error> {
        self.dv_generic(Signal::Path, token)
    }

    /// Steps as a TikZ path (pt): moves, lines, and the arcs of polar axes as Bézier curves.
    pub(super) fn path_text(steps: &[(PathToken, Step)]) -> String {
        let mut out = String::new();
        let mut current = Point::new(0.0, 0.0);
        for (token, step) in steps {
            match step {
                Step::Point(p) | Step::Direction(p) => {
                    if *token == PathToken::LineTo && !out.is_empty() {
                        out.push_str(" -- ");
                    } else {
                        out.push(' ');
                    }
                    out.push_str(&pt(*p));
                    current = *p;
                }
                Step::Arc { from, to, a, b } => {
                    let e = |t: f64| {
                        let (s, c) = t.to_radians().sin_cos();
                        a.scale(c).plus(b.scale(s))
                    };
                    let d = |t: f64| {
                        let (s, c) = t.to_radians().sin_cos();
                        b.scale(c).minus(a.scale(s))
                    };
                    let center = current.minus(e(*from));
                    let n = ((to - from).abs() / 90.0).ceil().max(1.0) as usize;
                    let delta = (to - from) / n as f64;
                    for k in 0..n {
                        let t1 = from + delta * k as f64;
                        let t2 = t1 + delta;
                        let kk = 4.0 / 3.0 * (delta.to_radians() / 4.0).tan();
                        let c1 = center.plus(e(t1)).plus(d(t1).scale(kk));
                        let c2 = center.plus(e(t2)).minus(d(t2).scale(kk));
                        let p2 = center.plus(e(t2));
                        out.push_str(&format!(" .. controls {} and {} .. {}", pt(c1), pt(c2), pt(p2)));
                        current = p2;
                    }
                }
            }
        }
        out
    }

    /// The low and high ends along the axis: a move and a line, each `goto` in a group.
    fn axis_steps(&mut self, axis: &str, low: &str, high: &str) -> Result<Vec<(PathToken, Step)>, Error> {
        let mut steps = Vec::new();
        for (token, value) in [(PathToken::MoveTo, low), (PathToken::LineTo, high)] {
            self.begin_group();
            let r = self.goto(axis, value).and_then(|_| self.path_step(token));
            self.end_group();
            steps.push((token, r?));
        }
        Ok(steps)
    }

    /// An axis line, from `low` to `high`.
    pub fn show_axis(&mut self, axis: &str, options: Vec<Token>) -> Result<(), Error> {
        let mut open = tikra_tokens("\\begin{scope}[data visualization/axis layer,/tikz/data visualization/.cd,low=min,high=max,every axis/.try,");
        open.extend(options);
        open.extend(tikra_tokens("]"));
        self.run_tokens(open)?;
        let r = (|| {
            self.object_of(&format!("{axis}/scaling mapper")).map(|mapper| self.direction(mapper)).transpose()?;
            let low = key_text(self, "/tikz/data visualization/low").unwrap_or_default();
            let high = key_text(self, "/tikz/data visualization/high").unwrap_or_default();
            let steps = self.axis_steps(axis, &low, &high)?;
            let path = Self::path_text(&steps);
            self.run_tokens(tikra_tokens(&format!("\\path[draw,/tikz/data visualization/styling]{path};")))
        })();
        let close = self.run_tokens(tikra_tokens("\\end{scope}"));
        r.and(close)
    }

    /// The positions of an axis' major, minor and
    /// subminor ticks (or grid lines), from the placement strategy, in `axis/default … at`.
    pub fn default_action(&mut self, what: &str, axis: &str) -> Result<(), Error> {
        let mut marshal = Vec::new();
        self.begin_group();
        let r = (|| {
            self.tex.set_meaning(&Token::cs("tikra@dv@ticks"), macro_tokens(Vec::new()), false);
            let default_at = crate::keys::value(&self.tex, &format!("{axis}/ticks at")).unwrap_or_default();
            self.tex.set_meaning(&Token::cs("tikra@dv@defaultticks"), macro_tokens(default_at), false);
            let settings = crate::keys::value(&self.tex, &format!("{axis}/{what} at")).unwrap_or_default();
            if let Some(v) = crate::keys::value(&self.tex, &format!("{axis}/scaling mapper")) {
                self.tex.set_meaning(&Token::cs("tikra@dv@mapper"), macro_tokens(v), false);
            }
            // the settings' own `major=`… do not count: only what the strategy computes
            self.dv()?.setters_off = true;
            let r = self.dv_keys(settings);
            self.dv()?.setters_off = false;
            r?;
            let step = crate::keys::value(&self.tex, "/tikz/data visualization/@step").unwrap_or_default();
            if !crate::keys::trim(&step).is_empty() {
                // linear or exponential steps, or the code of one's own strategy
                let strategy = crate::keys::value(&self.tex, &format!("{axis}/tick placement strategy")).unwrap_or_default();
                match tex::tokens_to_string(&strategy).trim() {
                    "linear" | "" => self.compute_at(false)?,
                    "exponential" => self.compute_at(true)?,
                    _ => self.run_tokens(strategy)?,
                }
                for k in ["major", "minor", "subminor"] {
                    if !marshal.is_empty() {
                        marshal.push(Token::other(','));
                    }
                    marshal.extend(tikra_tokens(&format!("{k}=")));
                    marshal.extend(group(crate::keys::value(&self.tex, &format!("/tikz/data visualization/{k}")).unwrap_or_default()));
                }
            }
            Ok(())
        })();
        self.end_group();
        self.tex.set_meaning(&Token::cs(&macro_name(&format!("{axis}/default {what} at"))), macro_tokens(marshal), false);
        r
    }

    /// Keys in `/tikz/data visualization` (`\tikzdatavisualizationset`).
    fn dv_keys(&mut self, v: Vec<Token>) -> Result<(), Error> {
        self.keys_command(v, Some("/tikz/data visualization"), true)
    }

    /// `\tikra@dv@pi{x}`: the text of an angle in radians as a multiple of π, to the nearest twelfth
    /// (`0`, `\pi`, `-\pi`, or the fraction then `\pi`).
    pub fn pi_typesetter(&mut self) -> Result<(), Error> {
        let x = self.argument_string()?;
        let x = parse_value(self, &x).unwrap_or(0.0);
        let k = (x * 12.0 / std::f64::consts::PI).round() / 12.0;
        let text = if k.abs() < 1e-4 {
            "$0$".to_string()
        } else if (k - 1.0).abs() < 1e-4 {
            "$\\pi$".to_string()
        } else if (k + 1.0).abs() < 1e-4 {
            "$-\\pi$".to_string()
        } else {
            format!("\\pgfmathprintnumber{{{}}}$\\pi$", num(k))
        };
        self.tex.push_tokens(tikra_tokens(&text));
        Ok(())
    }

    /// `about=n`: a step that gives about n ticks over the range, its first
    /// digit chosen by the about strategy (or 1, 3, 6 for exponential steps).
    pub fn compute_about(&mut self, n: &str) -> Result<(), Error> {
        let Some(mapper) = self.object_in("tikra@dv@mapper") else { return Ok(()) };
        let (Some(mut lo), Some(mut hi)) = self.range_of(mapper) else { return Ok(()) };
        let log = self.dv()?.log_steps;
        if log {
            lo = lo.log10();
            hi = hi.log10();
        }
        let about = parse_value(self, n).unwrap_or(5.0);
        let around = fpu_divide(hi - lo, about);
        if around == 0.0 || !around.is_finite() {
            self.tex.define_string("tikra@dv@step", "0", false);
            return Ok(());
        }
        let e = around.abs().log10().floor() as i32;
        let m = around.abs() / 10f64.powi(e);
        // TeX's dimensions keep five decimals of the mantissa
        let m = (m * 65536.0).round() / 65536.0;
        let step = if log {
            if e < 0 {
                1.0
            } else if m < 2.5 {
                10f64.powi(e)
            } else if m < 5.1 {
                3.0 * 10f64.powi(e)
            } else {
                6.0 * 10f64.powi(e)
            }
        } else {
            let strategy = key_text(self, "/tikz/data visualization/about strategy").unwrap_or_default();
            let mut x = m;
            for pair in strategy.split(',') {
                if let Some((t, r)) = pair.split_once('/') {
                    if let (Ok(t), Ok(r)) = (t.trim().parse::<f64>(), r.trim().parse::<f64>()) {
                        if x < t {
                            x = r;
                            break;
                        }
                    }
                }
            }
            if x < 10.0 {
                x * 10f64.powi(e)
            } else {
                x / 10.0 * 10f64.powi(e + 1)
            }
        };
        self.tex.define_string("tikra@dv@step", &num(step), false);
        Ok(())
    }

    /// Linear and exponential steps: the major positions from the step and the
    /// phase, and the minor ones between them.
    pub fn compute_at(&mut self, log: bool) -> Result<(), Error> {
        self.dv()?.log_steps = log;
        self.tex.set_meaning(&Token::cs("tikra@dv@step"), Meaning::Prim(tex::Prim::Relax), false);
        let code = crate::keys::value(&self.tex, "/tikz/data visualization/@step").unwrap_or_default();
        self.run_tokens(code)?;
        if matches!(self.tex.meaning_of("tikra@dv@step"), Meaning::Prim(tex::Prim::Relax) | Meaning::Undefined) {
            return Ok(());
        }
        let step_text = macro_text(self, "tikra@dv@step");
        let Some(mapper) = self.object_in("tikra@dv@mapper") else { return Ok(()) };
        let (Some(lo), Some(hi)) = self.range_of(mapper) else { return Ok(()) };
        let phase = key_text(self, "/tikz/data visualization/phase").unwrap_or_default();
        let phase = parse_value(self, &phase).unwrap_or(0.0);
        let step = parse_value(self, &step_text).unwrap_or(0.0);
        let substeps = key_text(self, "/tikz/data visualization/minor steps between steps").filter(|s| !s.is_empty());
        let substeps = match substeps {
            Some(s) => Some(self.number(&s)?.trunc() as i64 + 1),
            None => None,
        };
        let mut majors = Vec::new();
        let mut minors = Vec::new();
        let linear_function =
            !matches!(self.datavis.last().and_then(|d| d.objects.get(mapper)), Some(super::Object::ScalingMapper(m)) if m.function != super::objects::Function::Identity);
        if !log {
            if step <= 0.0 {
                self.dv_keys(tikra_tokens(&format!("major/.expanded={{at={{{}}}}}", num(lo))))?;
                return Ok(());
            }
            let mut current = ((lo - phase) / step).floor() * step + phase;
            let epsilon = if linear_function { (hi - lo) * 0.001 } else { 0.0 };
            let (min_eps, max_eps) = (lo - epsilon, hi + epsilon);
            let mut n = 0;
            loop {
                if let Some(k) = substeps {
                    let sub = step / k as f64;
                    let mut s = current;
                    for _ in 0..k {
                        s += sub;
                        if s >= lo && s < hi {
                            minors.push(s);
                        }
                    }
                }
                if current >= min_eps {
                    majors.push(current);
                }
                current += step;
                n += 1;
                if current >= max_eps || n > 1000 {
                    break;
                }
            }
        } else {
            let (lo_l, hi_l) = (lo.log10(), hi.log10());
            if !lo_l.is_finite() || !hi_l.is_finite() {
                self.warn(format!("data visualization: logarithmic ticks for a range from {} to {}, which has no logarithm: no ticks", num(lo), num(hi)));
                return Ok(());
            }
            if step <= 0.0 {
                return Ok(());
            }
            let mut current = ((lo_l - phase) / step).floor() * step + phase;
            let epsilon = (hi_l - lo_l) * 0.001;
            let (min_eps, max_eps) = (lo_l - epsilon, hi_l + epsilon);
            let mut n = 0;
            loop {
                if let Some(k) = substeps {
                    let a = pow10(current);
                    let b = pow10(current + step);
                    let sub = (b - a) / k as f64;
                    let mut s = a;
                    for _ in 0..k {
                        s += sub;
                        if s >= lo && s < hi {
                            minors.push(s);
                        }
                    }
                }
                if current >= min_eps {
                    majors.push(pow10(current));
                }
                current += step;
                n += 1;
                if current >= max_eps || n > 1000 {
                    break;
                }
            }
        }
        let list = |v: &[f64]| v.iter().map(|x| format!(",{}", num(*x))).collect::<String>();
        let keys = format!("major/.expanded={{at={{{}}}}},minor/.expanded={{at={{{}}}}}", list(&majors), list(&minors));
        self.dv_keys(tikra_tokens(&keys))
    }

    /// The ticks of one kind (`major`…) of an axis, and their texts.
    pub fn show_ticks(&mut self, kind: &str, axis: &str, options: Vec<Token>) -> Result<(), Error> {
        let settings = self.at_settings(axis, "ticks");
        if crate::keys::trim(&settings).is_empty() {
            return Ok(());
        }
        self.begin_group();
        let r = self.show_ticks_in_group(kind, axis, options, settings);
        self.end_group();
        r
    }

    /// `axis/default … at` and `axis/… at`, the settings for the ticks or the grid.
    fn at_settings(&mut self, axis: &str, what: &str) -> Vec<Token> {
        let default = crate::keys::value(&self.tex, &format!("{axis}/default {what} at")).unwrap_or_default();
        let own = crate::keys::value(&self.tex, &format!("{axis}/{what} at")).unwrap_or_default();
        if crate::keys::trim(&default).is_empty() {
            own
        } else {
            let mut v = default;
            v.push(Token::other(','));
            v.extend(own);
            v
        }
    }

    fn show_ticks_in_group(&mut self, kind: &str, axis: &str, options: Vec<Token>, settings: Vec<Token>) -> Result<(), Error> {
        self.tex.set_meaning(&Token::cs("tikra@dv@at"), macro_tokens(Vec::new()), false);
        self.tex.set_meaning(&Token::cs("tikra@dv@atoptions"), macro_tokens(Vec::new()), false);
        let Some(mapper) = self.object_of(&format!("{axis}/scaling mapper")) else { return Ok(()) };
        let mut open = tikra_tokens(&format!("\\begin{{scope}}[/tikz/data visualization/.cd,every ticks/.try,every {kind} ticks/.try,"));
        open.extend(options);
        open.extend(tikra_tokens("]"));
        self.run_tokens(open)?;
        let r = (|| {
            self.dv_keys(settings)?;
            let common = crate::keys::value(&self.tex, "/tikz/data visualization/common").unwrap_or_default();
            self.dv_keys(common)?;
            let own = crate::keys::value(&self.tex, &format!("/tikz/data visualization/{kind}")).unwrap_or_default();
            self.dv_keys(own)?;
            let direction_axis = key_text(self, "/tikz/data visualization/direction axis").unwrap_or_default();
            let dir_mapper = self.object_of(&format!("/tikz/data visualization/{direction_axis}/scaling mapper"));
            let list = self.at_list();
            let styles = self.style_at_list();
            let mut count = 0;
            for item in list {
                count += 1;
                self.begin_group();
                let r = self.one_tick(mapper, dir_mapper, &item, count, &styles);
                self.end_group();
                r?;
            }
            Ok(())
        })();
        let close = self.run_tokens(tikra_tokens("\\end{scope}"));
        r.and(close)
    }

    /// The tick positions (`at=`), empty ones left out.
    fn at_list(&mut self) -> Vec<Vec<Token>> {
        let v = if let Meaning::Macro(m) = self.tex.meaning_of("tikra@dv@at") { tex::substitute(&m.body, &[]) } else { Vec::new() };
        let v = self.tex.expand_fully(v).unwrap_or_default();
        split_items(&v).into_iter().filter(|i| !crate::keys::trim(i).is_empty()).collect()
    }

    /// `options at=value as [options]`: the values and their options.
    fn style_at_list(&mut self) -> Vec<(f64, Vec<Token>)> {
        let v = if let Meaning::Macro(m) = self.tex.meaning_of("tikra@dv@atoptions") { tex::substitute(&m.body, &[]) } else { Vec::new() };
        let mut out = Vec::new();
        for item in split_items(&v) {
            let (pos, opts, _) = parse_tick(&item);
            let s = self.tex.expand_to_string(pos).unwrap_or_default();
            if let Some(x) = parse_value(self, &s) {
                out.push((x, opts));
            }
        }
        out
    }

    fn one_tick(&mut self, mapper: usize, dir_mapper: Option<usize>, item: &[Token], count: usize, styles: &[(f64, Vec<Token>)]) -> Result<(), Error> {
        let (pos, opts, text) = parse_tick(item);
        self.dv_keys(opts)?;
        let pos_text = self.tex.expand_to_string(pos)?;
        self.set_in_to(mapper, &pos_text);
        let origin = self.local_data_point()?;
        let (dir, max_anchor, min_anchor) = match dir_mapper {
            Some(m) => self.direction(m)?,
            None => (Point::new(0.0, 1.0), "south", "north"),
        };
        // the options of `options at` for this value (none for a position that is no number)
        let x = parse_value(self, &pos_text);
        let mut mine = Vec::new();
        for (v, o) in styles {
            if x.is_some_and(|x| (v - x).abs() <= 1e-9 * v.abs().max(x.abs()).max(1e-300) || (*v == 0.0 && x.abs() < 1e-12)) {
                mine.push(Token::other(','));
                mine.extend(o.clone());
            }
        }
        if !mine.is_empty() {
            self.dv_keys(mine)?;
        }
        let parity = if count % 2 == 1 { "odd" } else { "even" };
        let high = key_text(self, "/tikz/data visualization/high").unwrap_or_default();
        let low = key_text(self, "/tikz/data visualization/low").unwrap_or_default();
        let at_low = matches!(self.tex.meaning_of("iftikra@dv@low"), Meaning::Prim(tex::Prim::Iftrue));
        let at_high = matches!(self.tex.meaning_of("iftikra@dv@high"), Meaning::Prim(tex::Prim::Iftrue));
        let mut max_val = self.length(&high)? / PT;
        let mut min_val = self.length(&low)? / PT;
        if at_high {
            let p = key_text(self, &format!("/tikz/data visualization/tick text high {parity} padding")).unwrap_or_default();
            max_val += self.length(&p)? / PT;
        }
        if at_low {
            let p = key_text(self, &format!("/tikz/data visualization/tick text low {parity} padding")).unwrap_or_default();
            min_val += self.length(&p)? / PT;
        }
        let max_tick = origin.plus(dir.scale(max_val));
        let min_tick = origin.plus(dir.scale(min_val));
        let every = format!("\\tikzdatavisualizationset{{every {parity} tick/.try}}");
        self.run_tokens(tikra_tokens(&format!(
            "\\begin{{scope}}[data visualization/ticks layer]{every}\\draw [/tikz/data visualization/styling] {} -- {};\\end{{scope}}",
            pt(min_tick),
            pt(max_tick)
        )))?;
        let text = match text {
            Some(t) => t,
            None => {
                if at_low || at_high {
                    self.tick_text(&pos_text)?
                } else {
                    Vec::new()
                }
            }
        };
        if crate::keys::trim(&text).is_empty() {
            return Ok(());
        }
        for (on, at, anchor) in [(at_low, min_tick, min_anchor), (at_high, max_tick, max_anchor)] {
            if !on {
                continue;
            }
            let mut v = tikra_tokens(&format!(
                "\\begin{{scope}}[data visualization/ticks node layer]{every}\\path [/tikz/data visualization/styling] {} node[anchor={anchor},/tikz/data visualization/node styling]",
                pt(at)
            ));
            v.extend(group(text.clone()));
            v.extend(tikra_tokens(";\\end{scope}"));
            self.run_tokens(v)?;
        }
        Ok(())
    }

    /// A tick's text: its value with the tick prefix, the typesetter and the suffix.
    fn tick_text(&mut self, pos: &str) -> Result<Vec<Token>, Error> {
        let v = parse_value(self, pos).unwrap_or(0.0);
        let mut code = crate::keys::value(&self.tex, "/tikz/data visualization/tick prefix").unwrap_or_default();
        // the typesetter's code with the value, run here so that what it prints is kept
        // (a key's code: the library gives it one, `\pgfmathprintnumber{#1}`)
        let Meaning::Macro(m) = self.tex.meaning_of(&macro_name("/tikz/data visualization/tick typesetter/.@cmd")) else { unreachable!("the tick typesetter is a key's code") };
        code.extend(tex::substitute(&m.body, &[tex::string_tokens(&num(v))]));
        code.extend(crate::keys::value(&self.tex, "/tikz/data visualization/tick suffix").unwrap_or_default());
        self.tex.define_string("tikra@dv@tickvalue", pos, false);
        // TeX prints the number inside the node: its style (a number format) applies
        self.begin_group();
        let r = self.keys_command(tikra_tokens("/tikz/data visualization/styling,/tikz/data visualization/node styling"), Some("/tikz"), true).and_then(|_| self.typeset(code));
        self.end_group();
        r
    }

    /// The grid lines of one kind, across the direction axis.
    pub fn show_grid(&mut self, kind: &str, axis: &str, options: Vec<Token>) -> Result<(), Error> {
        let settings = self.at_settings(axis, "grid");
        if crate::keys::trim(&settings).is_empty() {
            return Ok(());
        }
        self.begin_group();
        let r = (|| {
            self.tex.set_meaning(&Token::cs("tikra@dv@at"), macro_tokens(Vec::new()), false);
            self.tex.set_meaning(&Token::cs("tikra@dv@atoptions"), macro_tokens(Vec::new()), false);
            self.tex.set_meaning(&Token::cs("tikra@dv@defaultticks"), macro_tokens(Vec::new()), false);
            let Some(mapper) = self.object_of(&format!("{axis}/scaling mapper")) else { return Ok(()) };
            let mut open = tikra_tokens(&format!("\\begin{{scope}}[data visualization/grid layer,/tikz/data visualization/.cd,every {kind} grid/.try,every grid,"));
            open.extend(options);
            open.extend(tikra_tokens("]"));
            self.run_tokens(open)?;
            let r = (|| {
                let ticks_at = crate::keys::value(&self.tex, &format!("{axis}/ticks at")).unwrap_or_default();
                self.tex.set_meaning(&Token::cs("tikra@dv@ticks"), macro_tokens(ticks_at), false);
                self.dv_keys(settings)?;
                let common = crate::keys::value(&self.tex, "/tikz/data visualization/common").unwrap_or_default();
                self.dv_keys(common)?;
                let own = crate::keys::value(&self.tex, &format!("/tikz/data visualization/{kind}")).unwrap_or_default();
                self.dv_keys(own)?;
                let direction_axis = key_text(self, "/tikz/data visualization/direction axis").unwrap_or_default();
                let dir_axis = format!("/tikz/data visualization/{direction_axis}");
                for item in self.at_list() {
                    self.begin_group();
                    let r = (|| {
                        let (pos, opts, _) = parse_tick(&item);
                        self.dv_keys(opts)?;
                        let pos = self.tex.expand_to_string(pos)?;
                        self.set_in_to(mapper, &pos);
                        let low = key_text(self, "/tikz/data visualization/low").unwrap_or_default();
                        let high = key_text(self, "/tikz/data visualization/high").unwrap_or_default();
                        let mut steps = Vec::new();
                        self.goto(&dir_axis, &low)?;
                        steps.push((PathToken::MoveTo, self.path_step(PathToken::MoveTo)?));
                        self.goto(&dir_axis, &high)?;
                        steps.push((PathToken::LineTo, self.path_step(PathToken::LineTo)?));
                        let path = Self::path_text(&steps);
                        self.run_tokens(tikra_tokens(&format!("\\draw[/tikz/data visualization/styling]{path};")))
                    })();
                    self.end_group();
                    r?;
                }
                Ok(())
            })();
            let close = self.run_tokens(tikra_tokens("\\end{scope}"));
            r.and(close)
        })();
        self.end_group();
        r
    }

    /// An axis' label: a node at the data point the
    /// options go to.
    pub fn show_label(&mut self, axis: &str, options: Vec<Token>) -> Result<(), Error> {
        let label = crate::keys::value(&self.tex, &format!("{axis}/label")).unwrap_or_default();
        if crate::keys::trim(&label).is_empty() {
            return Ok(());
        }
        self.update_bounding_box(false);
        let mut open = tikra_tokens("\\begin{scope}[anchor=center,/tikz/data visualization/.cd,every axis label/.try,");
        open.extend(options);
        open.extend(tikra_tokens("]"));
        self.run_tokens(open)?;
        let r = (|| {
            let label = crate::keys::trim(&label).to_vec();
            let (opts, text) = match label.first() {
                Some(t) if t.is_char('[') => {
                    let mut depth = 0usize;
                    let mut end = label.len();
                    for (i, t) in label.iter().enumerate().skip(1) {
                        match t {
                            Token::Char(_, Cat::Begin) => depth += 1,
                            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                            x if depth == 0 && x.is_char(']') => {
                                end = i;
                                break;
                            }
                            _ => {}
                        }
                    }
                    (label[1..end].to_vec(), label[(end + 1).min(label.len())..].to_vec())
                }
                _ => (Vec::new(), label.clone()),
            };
            self.dv_keys(opts)?;
            let p = self.local_data_point()?;
            self.st.transform = self.st.transform.compose(&Transform::translation(p.x * PT, p.y * PT));
            let mut v = tikra_tokens("\\node [/tikz/data visualization/node styling]");
            v.extend(group(text));
            v.extend(tikra_tokens(";"));
            self.run_tokens(v)
        })();
        let close = self.run_tokens(tikra_tokens("\\end{scope}"));
        r.and(close)
    }
}

fn pow10(x: f64) -> f64 {
    // whole exponents exactly, as \pgfdvmathpowten does
    if (x - x.round()).abs() <= 0.0002 {
        10f64.powi(x.round() as i32)
    } else {
        10f64.powf(x)
    }
}

/// Items at top-level commas, one pair of braces removed from each.
fn split_items(v: &[Token]) -> Vec<Vec<Token>> {
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
    out.into_iter().map(|i| tex::strip_outer_braces(crate::keys::trim(&i).to_vec())).collect()
}

/// `value as [options] text`: the value, the options, and the text if given.
fn parse_tick(item: &[Token]) -> (Vec<Token>, Vec<Token>, Option<Vec<Token>>) {
    // `as`, at the top level
    let mut depth = 0usize;
    let mut at = None;
    for i in 0..item.len() {
        match &item[i] {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char('a', _) if depth == 0 && matches!(item.get(i + 1), Some(Token::Char('s', _))) => {
                at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(i) = at else { return (item.to_vec(), Vec::new(), None) };
    let pos = item[..i].to_vec();
    let mut rest = crate::keys::trim(&item[i + 2..]).to_vec();
    let mut opts = Vec::new();
    if rest.first().is_some_and(|t| t.is_char('[')) {
        let mut depth = 0usize;
        let mut end = rest.len();
        for (k, t) in rest.iter().enumerate().skip(1) {
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                x if depth == 0 && x.is_char(']') => {
                    end = k;
                    break;
                }
                _ => {}
            }
        }
        // delimited by `]`, TeX drops a pair of braces around all of it
        opts = tex::strip_outer_braces(rest[1..end].to_vec());
        rest = crate::keys::trim(&rest[(end + 1).min(rest.len())..]).to_vec();
    }
    let text = if rest.is_empty() { None } else { Some(rest) };
    (pos, opts, text)
}

/// `a / b` as pgf's FPU divides: the quotient of the two mantissas kept to five decimals, so
/// 1.99998 / 5 is 0.4 (and a step of 0.5, not 0.25, for a sine sampled to TeX's precision).
fn fpu_divide(a: f64, b: f64) -> f64 {
    if a == 0.0 || b == 0.0 || !a.is_finite() || !b.is_finite() {
        return a / b;
    }
    let (ea, eb) = (a.abs().log10().floor() as i32, b.abs().log10().floor() as i32);
    let q = (a / 10f64.powi(ea)) / (b / 10f64.powi(eb));
    (q * 1e5).round() / 1e5 * 10f64.powi(ea - eb)
}
