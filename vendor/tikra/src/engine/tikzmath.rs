//! The math library: `\tikzmath{…}` and the `evaluate={…}` key run statements ended by `;` —
//! declarations (`integer \i, \j;`, `real \x;`, `coordinate \c;`), assignments (`\x = 2*3;`,
//! `\a{\i,\j} = \i+\j;`, `\x1 = 4;`, `\c = (1,2)+(30:1);`), `let \x = text;`, `for \i in {list} {…};`,
//! `if cond then {…} else {…};` and `{code};` (TikZ code, run as it is). A variable is a macro
//! holding its value; one assigned with an index takes the index as its argument, and a coordinate
//! also sets `\cx` and `\cy` (in pt).

use crate::tex::{self, Cat, Token};
use crate::Error;

use super::{macro_tokens, tikra_tokens, Engine};

/// What a declaration made a variable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Real,
    Integer,
    Coordinate,
}

/// The statements of `body`, split at the `;` outside braces.
fn statements(body: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0usize;
    for t in body {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(';', _) if depth == 0 => {
                out.push(Vec::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().expect("a statement").push(t.clone());
    }
    out.into_iter().map(|s| crate::keys::trim(&s).to_vec()).filter(|s| !s.is_empty()).collect()
}

/// The leading word of a statement (`integer`, `for`…), and the rest.
fn first_word(v: &[Token]) -> (String, &[Token]) {
    let n = v.iter().take_while(|t| matches!(t, Token::Char(_, Cat::Letter))).count();
    (v[..n].iter().filter_map(|t| if let Token::Char(c, _) = t { Some(*c) } else { None }).collect(), &v[n..])
}

/// A braced group at the start of `v` (spaces skipped): its contents and the rest.
fn group(v: &[Token]) -> Option<(Vec<Token>, &[Token])> {
    let v = crate::keys::trim(v);
    if !matches!(v.first(), Some(Token::Char(_, Cat::Begin))) {
        return None;
    }
    // (the statements come from TeX's tokens, whose braces pair up: the group is closed)
    let mut depth = 0usize;
    let end = v.iter().position(|t| {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth -= 1,
            _ => {}
        }
        depth == 0
    })?;
    Some((v[1..end].to_vec(), &v[end + 1..]))
}

/// Where the first `=` outside braces is in `v` (`\a{x=y} = 1`: the one after the index).
fn equals(v: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    v.iter().position(|t| {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            _ => return depth == 0 && t.is_char('='),
        }
        false
    })
}

/// Where the word `word` is at the top level of `v`.
fn find_word(v: &[Token], word: &str) -> Option<usize> {
    let chars: Vec<char> = word.chars().collect();
    let mut depth = 0usize;
    for i in 0..v.len() {
        match &v[i] {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            _ if depth == 0 && i + chars.len() <= v.len() => {
                let at_word_start = i == 0 || !matches!(v[i - 1], Token::Char(_, Cat::Letter));
                let matches = chars.iter().enumerate().all(|(k, c)| v[i + k].is_char(*c));
                let ends = !matches!(v.get(i + chars.len()), Some(Token::Char(_, Cat::Letter)));
                if at_word_start && matches && ends {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

impl Engine {
    /// `\tikzmath{statements}`.
    pub fn tikzmath(&mut self) -> Result<(), Error> {
        let body = self.argument()?;
        self.math_statements(&body)
    }

    pub fn math_statements(&mut self, body: &[Token]) -> Result<(), Error> {
        for s in statements(body) {
            self.math_statement(&s)?;
        }
        Ok(())
    }

    fn math_statement(&mut self, s: &[Token]) -> Result<(), Error> {
        // `{ \draw …; };`: TeX (TikZ) code, run as it is
        if let Some((code, after)) = group(s) {
            if crate::keys::trim(after).is_empty() {
                return self.run_tokens(code);
            }
        }
        let (word, rest) = first_word(s);
        match word.as_str() {
            "int" | "integer" | "real" | "coordinate" => {
                let kind = match word.as_str() {
                    "real" => Kind::Real,
                    "coordinate" => Kind::Coordinate,
                    _ => Kind::Integer,
                };
                for t in rest {
                    if let Token::Cs(n) = t {
                        self.math_kinds.insert(n.to_string(), kind);
                    }
                }
                Ok(())
            }
            "let" => {
                let Some(eq) = equals(rest) else { return Err(Error::Figure("\\tikzmath: let without '='".into())) };
                // the expanded tokens themselves: `ab\relax c` stays three letters around a \relax
                let value = self.tex.expand_fully(crate::keys::trim(&rest[eq + 1..]).to_vec())?;
                self.math_set(&rest[..eq], crate::keys::trim(&value).to_vec(), None)
            }
            "for" => {
                let Some(i) = find_word(rest, "in") else { return Err(Error::Figure("\\tikzmath: for without 'in'".into())) };
                let var = rest[..i].iter().find(|t| matches!(t, Token::Cs(_))).cloned().ok_or_else(|| Error::Figure("\\tikzmath: for without a variable".into()))?;
                let Some((list, after)) = group(&rest[i + 2..]) else { return Err(Error::Figure("\\tikzmath: for without a list".into())) };
                let Some((body, _)) = group(after) else { return Err(Error::Figure("\\tikzmath: for without a body".into())) };
                let list = self.tex.expand_fully(list)?;
                for item in super::foreach_items(&list) {
                    self.tex.set_meaning(&var, super::macro_tokens(item), false);
                    self.math_statements(&body)?;
                }
                Ok(())
            }
            "if" => {
                let Some(then) = find_word(rest, "then") else { return Err(Error::Figure("\\tikzmath: if without 'then'".into())) };
                let text = self.math_string(rest[..then].to_vec())?;
                let condition = crate::expr::eval(&text, &self.vars())? != 0.0;
                let Some((yes, after)) = group(&rest[then + 4..]) else { return Err(Error::Figure("\\tikzmath: if without a block".into())) };
                let no = match find_word(after, "else") {
                    Some(k) => group(&after[k + 4..]).map(|(b, _)| b),
                    None => None,
                };
                match (condition, no) {
                    (true, _) => self.math_statements(&yes),
                    (false, Some(no)) => self.math_statements(&no),
                    (false, None) => Ok(()),
                }
            }
            // what print typesets is text outside a picture; functions are not supported yet
            "print" => Ok(()),
            "function" => {
                self.warn("\\tikzmath: function not supported: ignored");
                Ok(())
            }
            _ => {
                let Some(eq) = equals(s) else { return Err(Error::Figure(format!("\\tikzmath: '{}' is not a statement", tex::tokens_to_string(s)))) };
                self.math_assign(&s[..eq], &s[eq + 1..])
            }
        }
    }

    /// `\x = expr`, `\a{i} = expr`, `\x1 = expr`.
    fn math_assign(&mut self, target: &[Token], expr: &[Token]) -> Result<(), Error> {
        let Some(Token::Cs(name)) = target.iter().find(|t| !t.is_space()) else { return Err(Error::Figure("\\tikzmath: an assignment needs a variable".into())) };
        let kind = self.math_kinds.get(&**name).copied().unwrap_or(Kind::Real);
        let text = self.math_string(crate::keys::trim(expr).to_vec())?;
        if kind == Kind::Coordinate {
            let t = text.trim();
            let p = if t.starts_with('(') && t.ends_with(')') && t[1..t.len() - 1].find(['(', ')']).is_none() {
                self.coordinate(&t[1..t.len() - 1])?.p
            } else {
                self.coordinate(&format!("${t}$"))?.p
            };
            let (x, y) = (p.x / crate::PT, p.y / crate::PT);
            self.math_set(target, tex::string_tokens(&format!("tikra cs:{x}pt,{y}pt")), Some((x, y)))
        } else {
            let (v, unit) = crate::expr::eval_length(&text, &self.vars())?;
            let v = if unit { v / crate::PT } else { v };
            let v = if kind == Kind::Integer { v.trunc() } else { v };
            self.math_set(target, tex::string_tokens(&super::pgf_number(v)), None)
        }
    }

    /// The variable of `target` (with its index, if any) set to `value`; a coordinate's x and y too.
    fn math_set(&mut self, target: &[Token], value: Vec<Token>, xy: Option<(f64, f64)>) -> Result<(), Error> {
        let target = crate::keys::trim(target);
        let Some(Token::Cs(name)) = target.first() else { return Err(Error::Figure("\\tikzmath: an assignment needs a variable".into())) };
        let name = name.to_string();
        let index = &target[1..];
        let mut values = vec![(name.clone(), value)];
        if let Some((x, y)) = xy {
            values.push((format!("{name}x"), tex::string_tokens(&format!("{x}pt"))));
            values.push((format!("{name}y"), tex::string_tokens(&format!("{y}pt"))));
        }
        if index.is_empty() {
            for (n, v) in values {
                self.tex.set_meaning(&Token::cs(&n), macro_tokens(v), false);
            }
            return Ok(());
        }
        // \a{\i,\j} or \x1: the variable takes the index as its argument
        let index = match group(index) {
            Some((g, _)) => g,
            None => index.to_vec(),
        };
        let key = self.tex.expand_to_string(index)?.replace(' ', "");
        for (n, v) in values {
            self.tex.set_meaning(&Token::cs(&format!("tikra@math@{n}@{key}")), macro_tokens(v), false);
            self.run_tokens(tikra_tokens(&format!("\\def\\{n}#1{{\\csname tikra@math@{n}@#1\\endcsname}}")))?;
        }
        Ok(())
    }
}
