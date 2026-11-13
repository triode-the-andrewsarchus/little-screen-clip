//! pgfmath expressions inside coordinates and options: `2*\R`, `sin(30)`, `sqrt(2)/2`, `\x+1`.
//! Trigonometry in degrees, as in TikZ. Units (`cm`, `pt`, `mm`) become centimeters.

use std::collections::HashMap;

use crate::Error;

/// Numeric variables (`\R`, `\x`, `x`) and declared functions (`declare function={E(\n) = (\n)^2;}`).
#[derive(Debug, Clone, Default)]
pub struct Variables {
    pub values: HashMap<String, f64>,
    /// name → (parameters without the backslash, body)
    pub functions: HashMap<String, (Vec<String>, String)>,
    /// The font size over 10pt, for `em` and `ex` (none: 10pt).
    pub font_scale: Option<f64>,
    /// `trig format=rad`: the trigonometric functions take and give radians.
    pub trig_rad: bool,
    /// lengths count in pt (as pgfmath counts them: `100/2cm` is 100/56.9), not in cm
    pub in_pt: bool,
    /// The font is sans serif (its `em` and `ex` are cmss's).
    pub sans: bool,
    /// pgf's floating point unit computes (pgfplots' expressions): its square root and `exp` are
    /// pgf's own, not pgfmath's.
    pub fpu: bool,
}

impl Variables {
    pub fn new() -> Variables {
        Variables::default()
    }
    pub fn insert(&mut self, name: String, value: f64) {
        self.values.insert(name, value);
    }
    pub fn get(&self, name: &str) -> Option<&f64> {
        self.values.get(name)
    }
}

/// Centimeters per TeX unit.
pub fn unit_cm(u: &str) -> Option<f64> {
    Some(match u {
        "cm" => 1.0,
        "mm" => 0.1,
        "pt" => 2.54 / 72.27,
        "bp" => 2.54 / 72.0,
        "in" => 2.54,
        "em" => 0.3515, // 10pt
        "ex" => 0.1515,
        "pc" => 12.0 * 2.54 / 72.27,
        _ => return None,
    })
}

/// Centimeters per unit, `em` and `ex` in the font of `vars` (an em is the font's quad, wider
/// than its size in the small designs and narrower in the large ones).
fn unit_cm_in(u: &str, vars: &Variables) -> Option<f64> {
    let f = unit_cm(u)?;
    let scale = vars.font_scale.unwrap_or(1.0);
    let f = match u {
        "em" => f * scale * crate::label::quad(10.0 * scale, vars.sans),
        "ex" => f * scale * crate::label::x_height(vars.sans) / crate::label::x_height(false),
        _ => f,
    };
    Some(if vars.in_pt { f / (2.54 / 72.27) } else { f })
}

/// Nesting cap (parentheses, signs, calls): `f(x) = f(x)` does not overflow the stack.
pub const MAX_DEPTH: usize = 200;

pub fn eval(text: &str, vars: &Variables) -> Result<f64, Error> {
    eval_at(text, vars, 0)
}

fn eval_at(text: &str, vars: &Variables, depth: usize) -> Result<f64, Error> {
    eval_units(text, vars, depth).map(|(v, _)| v)
}

/// The value, and whether a unit appeared in the expression (pgfmath's "units declared"). With
/// units, the value is in cm unless `vars.in_pt`.
fn eval_units(text: &str, vars: &Variables, depth: usize) -> Result<(f64, bool), Error> {
    let (v, units) = eval_pt(text, vars, depth)?;
    if !units || vars.in_pt {
        return Ok((v, units));
    }
    // back to cm, without the rounding error of the way through pt (`14cm` is 14, not 13.999…):
    // only a value a few ulps from a round one is moved (`120pt` stays 120pt exactly)
    let cm = v * crate::PT;
    let round = (cm * 1e9).round() / 1e9;
    Ok((if (round - cm).abs() <= 8.0 * f64::EPSILON * cm.abs() { round } else { cm }, units))
}

/// The value with lengths in pt, as pgfmath computes it: a length is its number of pt, so `1/10pt`
/// is 0.1 and `3pt*2cm` is 3 times 56.9 (pt, since units appeared).
fn eval_pt(text: &str, vars: &Variables, depth: usize) -> Result<(f64, bool), Error> {
    let mut p = Parser { c: text.chars().collect(), pos: 0, vars, depth, units: false };
    p.skip_spaces();
    let v = p.ternary()?;
    p.skip_spaces();
    if p.pos < p.c.len() {
        return Err(Error::Expression(format!("leftover '{}' in '{text}'", p.c[p.pos..].iter().collect::<String>())));
    }
    Ok((v, p.units))
}

/// pgfmath's strings: `cond ? "pink" : "purple!50"` is `pink` or `purple!50`. `Some` when the value
/// the expression picks is a quoted string; `None` for one without quotes (evaluated as a number).
pub fn eval_string(text: &str, vars: &Variables) -> Result<Option<String>, Error> {
    let mut t = text.trim().to_string();
    // each `?` and each array picks one part, so a chain of them is walked in a loop, without recursion
    loop {
        if !t.contains('"') {
            return Ok(None);
        }
        // `(0 ? "red" : "green")`: parentheses around the whole of it
        if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')).filter(|i| balanced(i)) {
            t = inner.trim().to_string();
            continue;
        }
        if let Some(inner) = t.strip_prefix('"').and_then(|r| r.strip_suffix('"')).filter(|i| !i.contains('"')) {
            return Ok(Some(inner.to_string()));
        }
        // `{"red", "blue"}[1]`: an array of strings, indexed
        if t.starts_with('{') {
            let mut p = Parser { c: t.chars().collect(), pos: 0, vars, depth: 0, units: false };
            if let Some(end) = p.array_end() {
                let element = p.array_element(end)?;
                p.skip_spaces();
                if p.pos == p.c.len() {
                    t = element;
                    continue;
                }
            }
        }
        let Some((q, c)) = top_ternary(&t) else {
            return Err(Error::Expression(format!("a string where a number is expected: '{t}'")));
        };
        t = if eval(&t[..q], vars)? != 0.0 { t[q + 1..c].trim() } else { t[c + 1..].trim() }.to_string();
    }
}

/// Whether no `)` in `t` closes more than was opened in it (outside quotes).
fn balanced(t: &str) -> bool {
    let (mut depth, mut quoted) = (0i32, false);
    for ch in t.chars() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// The byte offsets of the first `?` outside parentheses and quotes, and of its `:`.
fn top_ternary(t: &str) -> Option<(usize, usize)> {
    let (mut depth, mut quoted, mut q, mut open) = (0i32, false, None, 0);
    for (i, ch) in t.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            '?' if depth == 0 => {
                q = q.or(Some(i));
                open += 1;
            }
            ':' if depth == 0 && q.is_some() => {
                open -= 1;
                if open == 0 {
                    return Some((q?, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Like `eval`, but a number without a unit is returned as is (the caller's default unit)
/// and one with a unit is converted to cm. Returns (value, had_unit).
pub fn eval_length(text: &str, vars: &Variables) -> Result<(f64, bool), Error> {
    let t = text.trim();
    // once: an expression with `rnd` draws a new number each time it is evaluated
    let first = eval_units(t, vars, 0);
    if first.is_ok() {
        return first;
    }
    // a unit after the whole expression (`2*\x cm`)
    for u in ["cm", "mm", "pt", "bp", "in", "em", "ex", "pc"] {
        if let Some(n) = t.strip_suffix(u) {
            if let Ok(v) = eval(n, vars) {
                return Ok((v * unit_cm_in(u, vars).unwrap(), true));
            }
        }
    }
    first
}

struct Parser<'a> {
    c: Vec<char>,
    pos: usize,
    vars: &'a Variables,
    depth: usize,
    /// a unit was glued to a number
    units: bool,
}

impl Parser<'_> {
    fn skip_spaces(&mut self) {
        while self.c.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.c.get(self.pos).copied()
    }

    /// `c ? a : b`, then `||`, `&&` and the comparisons, which give 1 or 0, as pgfmath has them.
    fn ternary(&mut self) -> Result<f64, Error> {
        let c = self.or()?;
        self.skip_spaces();
        if self.peek() != Some('?') {
            return Ok(c);
        }
        self.pos += 1;
        let a = self.ternary()?;
        self.skip_spaces();
        if self.peek() != Some(':') {
            return Err(Error::Expression("'?' without ':'".into()));
        }
        self.pos += 1;
        let b = self.ternary()?;
        Ok(if c != 0.0 { a } else { b })
    }

    fn starts(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.c.get(self.pos + i) == Some(&c))
    }

    fn or(&mut self) -> Result<f64, Error> {
        let mut v = self.and()?;
        loop {
            self.skip_spaces();
            if !self.starts("||") {
                return Ok(v);
            }
            self.pos += 2;
            let w = self.and()?;
            v = f64::from(v != 0.0 || w != 0.0);
        }
    }

    fn and(&mut self) -> Result<f64, Error> {
        let mut v = self.comparison()?;
        loop {
            self.skip_spaces();
            if !self.starts("&&") {
                return Ok(v);
            }
            self.pos += 2;
            let w = self.comparison()?;
            v = f64::from(v != 0.0 && w != 0.0);
        }
    }

    fn comparison(&mut self) -> Result<f64, Error> {
        let v = self.sum()?;
        self.skip_spaces();
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.starts(op) {
                self.pos += op.len();
                let w = self.sum()?;
                return Ok(f64::from(match op {
                    "==" => v == w,
                    "!=" => v != w,
                    "<=" => v <= w,
                    ">=" => v >= w,
                    "<" => v < w,
                    _ => v > w,
                }));
            }
        }
        Ok(v)
    }

    fn sum(&mut self) -> Result<f64, Error> {
        let mut v = self.product()?;
        loop {
            self.skip_spaces();
            match self.peek() {
                Some('+') => {
                    self.pos += 1;
                    v += self.product()?;
                }
                Some('-') => {
                    self.pos += 1;
                    v -= self.product()?;
                }
                _ => return Ok(v),
            }
        }
    }

    fn product(&mut self) -> Result<f64, Error> {
        let mut v = self.signed()?;
        loop {
            self.skip_spaces();
            match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    v *= self.signed()?;
                }
                Some('/') => {
                    self.pos += 1;
                    v /= self.signed()?;
                }
                _ => return Ok(v),
            }
        }
    }

    /// A sign binds less than `^`, as in pgfmath: `-x^2` is `-(x^2)`.
    fn signed(&mut self) -> Result<f64, Error> {
        self.skip_spaces();
        match self.peek() {
            Some('-') | Some('+') => {
                let minus = self.peek() == Some('-');
                self.pos += 1;
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(Error::Expression(format!("nesting went past {MAX_DEPTH} levels")));
                }
                let v = self.signed();
                self.depth -= 1;
                v.map(|v| if minus { -v } else { v })
            }
            _ => self.power(),
        }
    }

    /// `a^b^c` is right-associative; read in a loop so a long `2^2^…^2` does not deepen the stack.
    /// The exponents may have a sign (`2^-1`).
    fn power(&mut self) -> Result<f64, Error> {
        let mut terms = vec![self.postfix()?];
        self.skip_spaces();
        while self.peek() == Some('^') {
            self.pos += 1;
            terms.push(self.postfix()?);
            self.skip_spaces();
        }
        let last = terms.pop().unwrap_or_default();
        Ok(terms.into_iter().rev().fold(last, |e, base| base.powf(e)))
    }

    /// A factor and pgfmath's postfix `r` (radians to degrees: `sin(2 r)`).
    fn postfix(&mut self) -> Result<f64, Error> {
        let v = self.unary()?;
        self.skip_spaces();
        if self.peek() == Some('r') && !self.c.get(self.pos + 1).is_some_and(|c| c.is_ascii_alphanumeric()) {
            self.pos += 1;
            return Ok(deg_of(v));
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::Expression(format!("nesting went past {MAX_DEPTH} levels")));
        }
        let r = self.unary_unbounded();
        self.depth -= 1;
        r
    }

    fn unary_unbounded(&mut self) -> Result<f64, Error> {
        self.skip_spaces();
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some('+') => {
                self.pos += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }

    /// At a `{` that an index `[…]` follows (pgfmath's arrays, `{1, {2, 3}}[1][0]`): where its
    /// group ends.
    fn array_end(&self) -> Option<usize> {
        let mut depth = 0;
        for (k, &c) in self.c.iter().enumerate().skip(self.pos) {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let next = self.c[k + 1..].iter().position(|c| !c.is_whitespace()).map(|d| k + 1 + d)?;
                        return (self.c[next] == '[').then_some(k + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The value of `text`, a part of this expression (an array's element, a declared function's
    /// body), its lengths in pt.
    fn sub(&mut self, text: &str, vars: &Variables) -> Result<f64, Error> {
        let (v, units) = eval_pt(text, vars, self.depth + 1)?;
        self.units |= units;
        Ok(v)
    }

    /// The array from here to `end` indexed by the `[…]` after it (from 0), one array in another
    /// for each index: the value of the element.
    fn index_array(&mut self, end: usize) -> Result<f64, Error> {
        let element = self.array_element(end)?;
        self.sub(&element, self.vars)
    }

    /// The text of the element that the indices after the array (from here to `end`) pick.
    fn array_element(&mut self, end: usize) -> Result<String, Error> {
        let mut element: String = self.c[self.pos..end].iter().collect();
        self.pos = end;
        loop {
            self.skip_spaces();
            if self.peek() != Some('[') {
                break;
            }
            self.pos += 1;
            let i = self.ternary()?;
            self.skip_spaces();
            if self.peek() != Some(']') {
                return Err(Error::Expression("missing ']'".into()));
            }
            self.pos += 1;
            let inner = element.trim().strip_prefix('{').and_then(|s| s.strip_suffix('}')).ok_or_else(|| Error::Expression(format!("'{element}' is not an array")))?.to_string();
            // its elements, at the commas outside braces
            let mut items = vec![String::new()];
            let mut depth = 0;
            for c in inner.chars() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    ',' if depth == 0 => {
                        items.push(String::new());
                        continue;
                    }
                    _ => {}
                }
                items.last_mut().unwrap().push(c);
            }
            element = items.get(i.max(0.0) as usize).ok_or_else(|| Error::Expression(format!("index {i} out of the array '{inner}'")))?.trim().to_string();
        }
        Ok(element)
    }

    fn atom(&mut self) -> Result<f64, Error> {
        self.skip_spaces();
        match self.peek() {
            Some('{') if self.array_end().is_some() => {
                let end = self.array_end().unwrap_or(self.pos);
                self.index_array(end)
            }
            Some('(') | Some('{') => {
                let close = if self.peek() == Some('(') { ')' } else { '}' };
                self.pos += 1;
                let v = self.ternary()?;
                self.skip_spaces();
                if self.peek() != Some(close) {
                    return Err(Error::Expression(format!("missing '{close}'")));
                }
                self.pos += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.pos += 1;
                }
                // pgfmath scientific notation: 1e3, 2.5E-4 (not confused with the em and ex units)
                if matches!(self.peek(), Some('e' | 'E')) {
                    let sign = matches!(self.c.get(self.pos + 1), Some('+' | '-')) as usize;
                    if self.c.get(self.pos + 1 + sign).is_some_and(|c| c.is_ascii_digit()) {
                        self.pos += 1 + sign;
                        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                            self.pos += 1;
                        }
                    }
                }
                let n: String = self.c[start..self.pos].iter().collect();
                let mut v: f64 = n.parse().map_err(|_| Error::Expression(format!("invalid number: {n}")))?;
                // a unit glued to a number inside an expression: 2cm, 3pt
                let rest: String = self.c[self.pos..].iter().take(2).collect();
                if let Some(f) = unit_cm_in(&rest, self.vars) {
                    if !self.c.get(self.pos + 2).is_some_and(|c| c.is_ascii_alphabetic()) {
                        self.pos += 2;
                        v *= if self.vars.in_pt { f } else { f / crate::PT };
                        self.units = true;
                    }
                }
                Ok(v)
            }
            Some('\\') => {
                self.pos += 1;
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.pos += 1;
                }
                let name: String = self.c[start..self.pos].iter().collect();
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    _ => self.vars.get(&name).copied().ok_or(Error::Expression(format!("unknown variable: \\{name}"))),
                }
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let start = self.pos;
                // digits after the first letter: atan2, log10
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
                    self.pos += 1;
                }
                let name: String = self.c[start..self.pos].iter().collect();
                if name == "pi" {
                    return Ok(std::f64::consts::PI);
                }
                if name == "e" {
                    return Ok(std::f64::consts::E);
                }
                self.skip_spaces();
                if self.peek() != Some('(') {
                    // a variable without a backslash, like the x of pgfplots functions
                    if let Some(v) = self.vars.get(&name) {
                        return Ok(*v);
                    }
                    // a function without arguments: declared (`declare function={C=4;}`) or pgfmath's
                    if let Some(body) = self.vars.functions.get(&name).filter(|f| f.0.is_empty()).map(|f| f.1.clone()) {
                        return self.sub(&body, self.vars);
                    }
                    // `sin 30`, `cos \x`: the function of the operand after it, sign included
                    // (`sin 2*30` is sin(2) times 30, `sin 30^2` is sin(30) squared)
                    if ONE_ARGUMENT.contains(&name.as_str()) {
                        let units_before = self.units;
                        let a = self.unary()?;
                        return self.call(&name, vec![a], units_before);
                    }
                    return match name.as_str() {
                        "rand" => Ok(pseudo_random_signed()),
                        "rnd" => Ok(pseudo_random()),
                        "true" => Ok(1.0),
                        "false" => Ok(0.0),
                        // the fpu's special numbers (pgfplots' `nan` point meta, `unbounded coords`)
                        "nan" | "NaN" => Ok(f64::NAN),
                        "inf" | "Inf" => Ok(f64::INFINITY),
                        _ => Err(Error::Expression(format!("function without parentheses: {name}"))),
                    };
                }
                self.pos += 1;
                let units_before = self.units;
                self.skip_spaces();
                let mut args = Vec::new();
                // `random()` has no argument
                if !(name == "random" && self.peek() == Some(')')) {
                    args.push(self.ternary()?);
                    self.skip_spaces();
                }
                while self.peek() == Some(',') {
                    self.pos += 1;
                    args.push(self.ternary()?);
                    self.skip_spaces();
                }
                if self.peek() != Some(')') {
                    return Err(Error::Expression(format!("missing ')' in {name}(…)")));
                }
                self.pos += 1;
                self.call(&name, args, units_before)
            }
            Some(c) => Err(Error::Expression(format!("unexpected character: '{c}'"))),
            None => Err(Error::Expression("empty expression".into())),
        }
    }

    /// The function `name` of `args` (one at least, but for `random()`); `units_before`: whether
    /// units had appeared before its arguments.
    fn call(&mut self, name: &str, args: Vec<f64>, units_before: bool) -> Result<f64, Error> {
        if let Some((params, body)) = self.vars.functions.get(name).cloned() {
            let mut locals = self.vars.clone();
            for (p, v) in params.iter().zip(&args) {
                locals.insert(p.clone(), *v);
            }
            return self.sub(&body, &locals);
        }
        // pgfmath's random integers, from the generator `rnd` uses: `random(m)` in 1..m,
        // `random(a, b)` in a..b; `random()` is `rnd`
        if name == "random" {
            return Ok(match args[..] {
                [] => pseudo_random(),
                [m] => random_integer(1, m as i64),
                [a, b, ..] => random_integer(a as i64, b as i64),
            });
        }
        let rad = self.vars.trig_rad;
        // the argument of a trigonometric function, in degrees; an angle given back
        let deg = |x: f64| if rad { x.to_degrees() } else { x };
        let angle = |x: f64| if rad { x.to_radians() } else { x };
        let g = |x: f64| deg(x).to_radians();
        let a = args[0];
        if rad {
            match name {
                "sin" | "cos" | "sec" | "cosec" => {
                    return Ok(match name {
                        "sin" => sin_degrees(deg(a)),
                        "cos" => cos_degrees(deg(a)),
                        "sec" => 1.0 / sin_degrees(deg(a) + 90.0),
                        _ => 1.0 / sin_degrees(deg(a)),
                    })
                }
                "asin" | "acos" | "atan" | "atan2" => {
                    return Ok(angle(match name {
                        "asin" => a.asin().to_degrees(),
                        "acos" => a.acos().to_degrees(),
                        "atan" => a.atan().to_degrees(),
                        _ => args.get(1).map_or(0.0, |x| a.atan2(*x).to_degrees()),
                    }))
                }
                _ => {}
            }
        }
        Ok(match name {
            "sin" => sin_degrees(a),
            "cos" => cos_degrees(a),
            "tan" => g(a).tan(),
            "asin" => a.asin().to_degrees(),
            "acos" => a.acos().to_degrees(),
            "atan" => a.atan().to_degrees(),
            "atan2" => args.get(1).map_or(0.0, |x| a.atan2(*x).to_degrees()),
            "sqrt" if self.vars.fpu => crate::engine::pgfplots::fpu::sqrt(a),
            "sqrt" => a.sqrt(),
            "abs" => a.abs(),
            "exp" if self.vars.fpu => crate::engine::pgfplots::fpu::exp(a),
            "exp" => a.exp(),
            "ln" => a.ln(),
            "log10" => a.log10(),
            "floor" => a.floor(),
            "ceil" => a.ceil(),
            "round" => a.round(),
            "deg" => a.to_degrees(),
            "rad" => a.to_radians(),
            "min" => args.iter().copied().fold(f64::INFINITY, f64::min),
            "max" => args.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "pow" => a.powf(args.get(1).copied().unwrap_or(1.0)),
            "mod" => a % args.get(1).copied().unwrap_or(1.0),
            // `Mod` never negative: Mod(-7,3) = 2, where mod(-7,3) = -1
            "Mod" => a.rem_euclid(args.get(1).copied().unwrap_or(1.0)),
            // the fpu's veclen is its sqrt(a^2 + b^2)
            "veclen" if self.vars.fpu => crate::engine::pgfplots::fpu::sqrt(a * a + args.get(1).map_or(0.0, |b| b * b)),
            "veclen" => a.hypot(args.get(1).copied().unwrap_or(0.0)),
            // n! of the integer part, as pgfmath has it
            "factorial" => (1..=a.trunc().max(0.0) as u64).map(|k| k as f64).product(),
            // pgfmath's hyperbolic and other functions
            "sinh" => a.sinh(),
            "cosh" => a.cosh(),
            "tanh" => a.tanh(),
            "cot" => 1.0 / g(a).tan(),
            "sec" => 1.0 / sin_degrees(a + 90.0),
            "cosec" => 1.0 / sin_degrees(a),
            "log2" => a.log2(),
            "int" => a.trunc(),
            "frac" => a - a.trunc(),
            "real" => a,
            // its value without the units its argument declared (a ratio of lengths)
            "scalar" => {
                self.units = units_before;
                a
            }
            "neg" => -a,
            "sign" => {
                if a > 0.0 {
                    1.0
                } else if a < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
            "iseven" => f64::from(a.trunc() % 2.0 == 0.0),
            "isodd" => f64::from(a.trunc() % 2.0 != 0.0),
            other => return Err(Error::Expression(format!("unknown function: {other}"))),
        })
    }
}

/// pgfmath's functions of one argument, which may also be written without parentheses (`sin 30`).
const ONE_ARGUMENT: &[&str] = &[
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "sqrt",
    "abs",
    "exp",
    "ln",
    "log10",
    "log2",
    "floor",
    "ceil",
    "round",
    "deg",
    "rad",
    "factorial",
    "sinh",
    "cosh",
    "tanh",
    "cot",
    "sec",
    "cosec",
    "int",
    "frac",
    "real",
    "scalar",
    "neg",
    "sign",
    "iseven",
    "isodd",
];

/// An integer of `a..=b` from pgf's generator: its number modulo the count, from `a`.
fn random_integer(a: i64, b: i64) -> f64 {
    let (lo, hi) = (a.min(b), a.max(b));
    (lo + next_random() % (hi - lo + 1)) as f64
}

#[cfg(test)]
mod tests {
    #[test]
    fn factorial_of_the_integer_part() {
        let v = Variables::new();
        assert_eq!(eval("factorial(5)", &v).unwrap(), 120.0);
        assert_eq!(eval("factorial(0)", &v).unwrap(), 1.0);
        assert_eq!(eval("factorial(4.7)", &v).unwrap(), 24.0);
    }

    use super::*;

    #[test]
    fn pgfmath_expressions() {
        let mut v = Variables::new();
        v.insert("R".into(), 0.4);
        assert!((eval("2*\\R", &v).unwrap() - 0.8).abs() < 1e-9);
        assert!((eval("sin(30)", &v).unwrap() - 0.5).abs() < 1e-9);
        assert!((eval("-\\R + 1", &v).unwrap() - 0.6).abs() < 1e-9);
        assert!((eval("2^3", &v).unwrap() - 8.0).abs() < 1e-9);
        assert!((eval_length("10pt", &v).unwrap().0 - 0.3515).abs() < 1e-3);
        // a sign binds less than ^ (pgfmath's -x^2 is -(x^2)); an exponent may have one
        assert_eq!(eval("-2^2", &v).unwrap(), -4.0);
        assert_eq!(eval("1-2^2", &v).unwrap(), -3.0);
        assert_eq!(eval("2^-1", &v).unwrap(), 0.5);
        assert_eq!(eval("3*-2^2", &v).unwrap(), -12.0);
    }
}

/// The sine of degrees as pgfmath computes it (`\pgfmathsin`): in TeX's fixed point, between the
/// cosines of the whole degrees around (rounded to five decimals), so `sin(90)` is 1 and the top of a
/// sine between two whole degrees just below.
fn sin_degrees(a: f64) -> f64 {
    pgf_cos_sp(sp_of(a) - 90 * 65536) as f64 / 65536.0
}

fn cos_degrees(a: f64) -> f64 {
    pgf_cos_sp(sp_of(a)) as f64 / 65536.0
}

/// A number as TeX reads it into a dimension: the nearest sp.
fn sp_of(v: f64) -> i64 {
    (v * 65536.0).round() as i64
}

/// `<factor><dimen>` as TeX computes it: the factor's whole part times the dimension, plus its
/// fraction (in 2^-16) times the dimension cut toward zero.
fn tex_scale(f: f64, d: i64) -> i64 {
    let negative = (f < 0.0) != (d < 0);
    let a = f.abs();
    let whole = a.trunc() as i64;
    let fraction = ((a - a.trunc()) * 65536.0).round() as i64;
    let d = d.abs();
    let r = whole * d + d * fraction / 65536;
    if negative {
        -r
    } else {
        r
    }
}

/// pgfmath's table: the cosine of a whole degree, to five decimals.
fn cos_table(n: i64) -> f64 {
    ((n as f64).to_radians().cos() * 1e5).round() / 1e5
}

/// pgfmath's cosine of `x` sp degrees, in sp.
fn pgf_cos_sp(x: i64) -> i64 {
    let mut x = x;
    let turns = (x / 65536) / 360 * -360;
    x += turns * 65536;
    if x < 0 {
        x = -x;
    }
    if x >= 180 * 65536 {
        x = 360 * 65536 - x;
    }
    let (a, fraction) = (x / 65536, x % 65536);
    if fraction == 0 {
        return sp_of(cos_table(a));
    }
    tex_scale(cos_table(a), 65536 - fraction) + tex_scale(cos_table((a + 1).min(180)), fraction)
}

/// Radians to degrees as pgfmath's postfix `r` (times 57.29577, in TeX's fixed point).
fn deg_of(v: f64) -> f64 {
    if v.abs() > 285.0 {
        return v.to_degrees();
    }
    tex_scale(57.29577, sp_of(v)) as f64 / 65536.0
}

thread_local! {
    static SEED: std::cell::Cell<i64> = const { std::cell::Cell::new(1) };
}

/// pgf's pseudo-random numbers (`\pgfmathgeneratepseudorandomnumber`): the Park–Miller "minimal
/// standard" generator with Schrage's factoring, a = 69621, m = 2^31 - 1, so that `\pgfmathsetseed`
/// gives TeX's sequence.
pub fn next_random() -> i64 {
    SEED.with(|s| {
        let (a, q, r, m) = (69_621i64, 30_845i64, 23_902i64, 2_147_483_647i64);
        let z = s.get();
        let mut n = a * (z % q) - r * (z / q);
        if n < 0 {
            n += m;
        }
        s.set(n);
        n
    })
}

/// `\pgfmathsetseed{n}`.
pub fn set_seed(seed: i64) {
    SEED.with(|s| s.set(seed));
}

/// pgfmath's `rnd`: [0, 1] in steps of 1/100000, as pgf scales the generator's number.
fn pseudo_random() -> f64 {
    (next_random() % 100_001) as f64 / 100_000.0
}

/// pgfmath's `rand`: [-1, 1].
pub fn pseudo_random_signed() -> f64 {
    (next_random() % 200_001 - 100_000) as f64 / 100_000.0
}

/// Starts `rnd` and `rand` over: each figure draws the same numbers.
pub fn reset_random() {
    // pgf seeds with \time * \year; a document here always starts from the same seed
    set_seed(1);
}
