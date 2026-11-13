//! Data formats: how `data [format=…] {…}` turns its lines into data points. `table` (the default:
//! a head line naming the attributes, then one data point per line), `named` (`x=1, y={2,3}`),
//! `TeX code`, and `function` (`var x : interval [0:1]; func y = \value x^2;`, from the
//! formats.functions library). Each data point is the `/data point/` keys being set, then
//! `\pgfdatapoint`.

use crate::tex::{self, Cat, Token};
use crate::Error;

use super::{key_text, num, set_attribute, Engine};
use crate::engine::{foreach_items, tikra_tokens};

/// One statement of the function format.
#[derive(Debug, Clone)]
enum Statement {
    /// `var name : interval [a:b] samples n` or `… step s`
    Interval { name: String, from: String, to: String, steps: Steps },
    /// `var name : {1,2,3}`
    List { name: String, items: Vec<Vec<Token>> },
    /// `func name = expression`
    Func { name: String, expr: Vec<Token> },
}

#[derive(Debug, Clone)]
enum Steps {
    Samples(String),
    Step(String),
}

impl Engine {
    /// `\tikra@dv@data{options}{lines}`: one data set, read by its format.
    pub fn run_data(&mut self, options: Vec<Token>, body: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        let r = (|| {
            let mut keys = tikra_tokens("/pgf/data/.cd,/pgf/every data/.try,");
            keys.extend(options);
            self.keys_command(keys, Some("/"), true)?;
            let format = key_text(self, "/pgf/data/format").unwrap_or_default();
            match format.as_str() {
                "table" => self.table_format(&body),
                "named" => self.named_format(&body),
                "function" => self.function_format(&body),
                "TeX code" => self.run_tokens(body),
                other => {
                    self.warn(format!("unknown data format '{other}'"));
                    Ok(())
                }
            }
        })();
        self.end_group();
        r
    }

    /// `format=table`: the head line (or `headline=`) names the columns; `separator=` splits them.
    fn table_format(&mut self, body: &[Token]) -> Result<(), Error> {
        let separator = key_text(self, "/pgf/data/separator").unwrap_or_default();
        let space = separator.is_empty() || separator.trim_start_matches('\\').trim() == "space";
        let split = |line: &str| -> Vec<String> {
            if space {
                line.split_whitespace().map(str::to_string).collect()
            } else {
                line.split(separator.as_str()).map(|s| s.trim().to_string()).collect()
            }
        };
        let mut names: Vec<String> = Vec::new();
        let headline = key_text(self, "/pgf/data/headline").unwrap_or_default();
        let mut need_head = headline.is_empty();
        if !need_head {
            names = split(&headline);
        }
        for line in lines(body) {
            let text = tex::tokens_to_string(&line);
            if text.trim().is_empty() {
                continue;
            }
            let cells = split(text.trim());
            if need_head {
                names = cells;
                need_head = false;
                continue;
            }
            self.begin_group();
            for (i, v) in cells.iter().enumerate() {
                let name = names.get(i).cloned().unwrap_or_else(|| format!("attribute {}", i + 1));
                set_attribute(self, &name, v);
            }
            let r = self.data_point_now();
            self.end_group();
            r?;
        }
        Ok(())
    }

    /// `format=named`: `key=value` pairs; a value `{a,b}` gives a data point for each.
    fn named_format(&mut self, body: &[Token]) -> Result<(), Error> {
        for line in lines(body) {
            if keys_trimmed(&line).is_empty() {
                continue;
            }
            let pairs: Vec<(String, Vec<Token>)> = crate::keys::split_list(&line).into_iter().map(|i| (i.key, i.value.unwrap_or_default())).collect();
            self.named_points(&pairs)?;
        }
        Ok(())
    }

    fn named_points(&mut self, pairs: &[(String, Vec<Token>)]) -> Result<(), Error> {
        let Some(((key, value), rest)) = pairs.split_first() else { return self.data_point_now() };
        let value = self.tex.expand_fully(value.clone())?;
        for item in foreach_items(&value) {
            self.begin_group();
            let v = tex::tokens_to_string(&item);
            set_attribute(self, key, v.trim());
            let r = self.named_points(rest);
            self.end_group();
            r?;
        }
        Ok(())
    }

    /// `format=function`: the variables range over their values, the functions are evaluated, and
    /// every combination is a data point.
    fn function_format(&mut self, body: &[Token]) -> Result<(), Error> {
        let mut all = keys_value(self, "/pgf/data/vars");
        all.push(Token::Char(' ', Cat::Space));
        all.extend(body.iter().cloned().map(|t| if matches!(t, Token::Char('\n' | '\r', Cat::Other)) { Token::Char(' ', Cat::Space) } else { t }));
        all.push(Token::Char(' ', Cat::Space));
        all.extend(keys_value(self, "/pgf/data/funcs"));
        let statements = parse_functions(&all)?;
        // \value x is the attribute's value while the functions are evaluated
        let getval = tex::Macro { params: vec![tex::Param::Arg(1)], body: tex::parse_body(tikra_tokens("\\pgfkeysvalueof{/data point/#1}")), optional: None, protected: false };
        self.tex.set_meaning(&Token::cs("value"), tex::Meaning::Macro(std::rc::Rc::new(getval)), false);
        self.run_statements(&statements)
    }

    fn run_statements(&mut self, statements: &[Statement]) -> Result<(), Error> {
        let Some((first, rest)) = statements.split_first() else { return self.data_point_now() };
        match first {
            Statement::Func { name, expr } => {
                let v = self.number_of(expr.clone())?;
                // \pgfmathparse computes in TeX's fixed point: what it gives is a whole number of
                // sp, its products cut toward zero (sin x at its top is just below 1)
                let v = if v.abs() < 16384.0 { (v * 65536.0).trunc() / 65536.0 } else { v };
                set_attribute(self, name, &num(v));
                self.run_statements(rest)
            }
            Statement::List { name, items } => {
                for item in items {
                    self.begin_group();
                    let v = tex::tokens_to_string(item);
                    set_attribute(self, name, v.trim());
                    let r = self.run_statements(rest);
                    self.end_group();
                    r?;
                }
                Ok(())
            }
            Statement::Interval { name, from, to, steps } => {
                // in TeX's fixed point, as \pgfmathloop runs it: the end is kept if reached exactly
                let a = self.number(from)?;
                let b = self.number(to)?;
                let step = match steps {
                    Steps::Samples(n) => {
                        let n = self.number(n)?;
                        (b - a) / (n - 1.0)
                    }
                    Steps::Step(s) => self.number(s)?,
                };
                let sp = |v: f64| (v * 65536.0).trunc() as i64;
                let (a, b, step) = (sp(a), sp(b), sp(step));
                if step <= 0 {
                    return Ok(());
                }
                self.begin_group();
                let mut v = a;
                let mut n = 0;
                let r = loop {
                    if b < v || n > 10_000 {
                        break Ok(());
                    }
                    set_attribute(self, name, &num(v as f64 / 65536.0));
                    if let Err(e) = self.run_statements(rest) {
                        break Err(e);
                    }
                    v += step;
                    n += 1;
                };
                self.end_group();
                r
            }
        }
    }

    /// A data point from the current keys.
    fn data_point_now(&mut self) -> Result<(), Error> {
        self.datavis_command(&Token::cs("pgfdatapoint"))
    }
}

fn keys_value(e: &mut Engine, key: &str) -> Vec<Token> {
    crate::keys::value(&e.tex, key).unwrap_or_default()
}

fn keys_trimmed(v: &[Token]) -> &[Token] {
    crate::keys::trim(v)
}

/// The lines of inline data (its ends of lines were read as characters).
fn lines(body: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    for t in body {
        if matches!(t, Token::Char('\n' | '\r', Cat::Other)) || t.is_cs("par") {
            out.push(Vec::new());
            continue;
        }
        if let Some(l) = out.last_mut() {
            l.push(t.clone());
        }
    }
    out
}

/// `var x : interval [a:b] samples n; func y = …;` as statements.
fn parse_functions(all: &[Token]) -> Result<Vec<Statement>, Error> {
    let mut out = Vec::new();
    // statements end at top-level semicolons
    let mut parts: Vec<Vec<Token>> = vec![Vec::new()];
    let mut depth = 0usize;
    for t in all {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(';', _) if depth == 0 => {
                parts.push(Vec::new());
                continue;
            }
            _ => {}
        }
        if let Some(p) = parts.last_mut() {
            p.push(t.clone());
        }
    }
    for p in parts {
        let p = keys_trimmed(&p).to_vec();
        let text = tex::tokens_to_string(&p);
        let t = text.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("var ") {
            let (name, spec) = rest.split_once(':').ok_or_else(|| Error::Figure(format!("data function: '{t}' without ':'")))?;
            let name = name.trim().to_string();
            let spec = spec.trim();
            if spec.starts_with('{') {
                // the list, as tokens
                let i = p.iter().position(|x| matches!(x, Token::Char(_, Cat::Begin))).unwrap_or(0);
                let list = tex::strip_outer_braces(keys_trimmed(&p[i..]).to_vec());
                out.push(Statement::List { name, items: foreach_items(&list) });
            } else if spec.contains('[') {
                // `interval [a:b]` (or any word before the brackets)
                let r = spec.trim();
                let open = r.find('[').ok_or_else(|| Error::Figure(format!("data function: '{t}' without '['")))?;
                let close = r.rfind(']').ok_or_else(|| Error::Figure(format!("data function: '{t}' without ']'")))?;
                let inner = &r[open + 1..close];
                let (from, to) = inner.split_once(':').ok_or_else(|| Error::Figure(format!("data function: '{t}': interval without ':'")))?;
                let after = r[close + 1..].trim();
                let steps = if let Some(n) = after.strip_prefix("samples") {
                    Steps::Samples(n.trim().to_string())
                } else if let Some(s) = after.strip_prefix("step") {
                    Steps::Step(s.trim().to_string())
                } else {
                    Steps::Samples("25".into())
                };
                out.push(Statement::Interval { name, from: from.trim().to_string(), to: to.trim().to_string(), steps });
            } else {
                return Err(Error::Figure(format!("data function: '{t}' not understood")));
            }
        } else if t.starts_with("func ") {
            let i = p.iter().position(|x| x.is_char('=')).ok_or_else(|| Error::Figure(format!("data function: '{t}' without '='")))?;
            let name = tex::tokens_to_string(&p[..i]).trim().trim_start_matches("func").trim().to_string();
            out.push(Statement::Func { name, expr: keys_trimmed(&p[i + 1..]).to_vec() });
        } else {
            return Err(Error::Figure(format!("data function: '{t}' not understood")));
        }
    }
    Ok(out)
}
