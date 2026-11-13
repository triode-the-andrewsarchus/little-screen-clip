//! The xstring package's commands: parts of a text (`\StrLeft`, `\StrBefore`, `\StrCut`…), its
//! length and counts, substitutions, and tests (`\IfSubStr`, `\IfStrEq`…). As xstring does by
//! default, the texts are expanded first; a character is a token, and a `{…}` group counts as one.
//! A result goes into the macro named in brackets after the arguments (`\StrLeft{abc}{2}[\r]`),
//! locally, or is typeset where the command is.

use super::{string_tokens, Body, Cat, Macro, Meaning, Tex, Token};
use crate::Error;
use std::rc::Rc;

/// The commands, by name.
pub const COMMANDS: &[&str] = &[
    "StrLen",
    "StrLeft",
    "StrRight",
    "StrMid",
    "StrChar",
    "StrBefore",
    "StrBehind",
    "StrCut",
    "StrBetween",
    "StrSubstitute",
    "StrDel",
    "StrCount",
    "IfSubStr",
    "IfStrEq",
    "IfEq",
    "IfBeginWith",
    "IfEndWith",
];

/// The characters of a text: each token, or each `{…}` group with its braces.
fn units(v: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i + 1;
        if matches!(v[i], Token::Char(_, Cat::Begin)) {
            let mut depth = 1usize;
            while j < v.len() && depth > 0 {
                match v[j] {
                    Token::Char(_, Cat::Begin) => depth += 1,
                    Token::Char(_, Cat::End) => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
        }
        out.push(&v[i..j]);
        i = j;
    }
    out
}

/// Where `p` occurs in `s`, from the left, without overlaps.
fn occurrences(s: &[&[Token]], p: &[&[Token]]) -> Vec<usize> {
    let mut out = Vec::new();
    if p.is_empty() {
        return out;
    }
    let mut i = 0;
    while i + p.len() <= s.len() {
        if s[i..i + p.len()] == *p {
            out.push(i);
            i += p.len();
        } else {
            i += 1;
        }
    }
    out
}

fn join(v: &[&[Token]]) -> Vec<Token> {
    v.iter().flat_map(|u| u.iter().cloned()).collect()
}

impl Tex {
    /// An argument, expanded.
    fn xstring_text(&mut self) -> Result<Vec<Token>, Error> {
        let a = self.read_undelimited()?;
        self.expand_fully(a)
    }

    /// An argument read as an integer.
    fn xstring_int(&mut self, a: Vec<Token>) -> Result<i64, Error> {
        let a = self.expand_fully(a)?;
        self.int_of(a)
    }

    /// `[…]` if it comes next, its numbers separated by commas.
    fn xstring_counts(&mut self) -> Result<Vec<i64>, Error> {
        let Some(t) = self.next_nonspace() else { return Ok(Vec::new()) };
        if !t.is_char('[') {
            self.back(t);
            return Ok(Vec::new());
        }
        let o = self.read_until_char(']', false)?;
        let mut out = Vec::new();
        for part in o.split(|t| t.is_char(',')) {
            out.push(self.xstring_int(part.to_vec())?);
        }
        Ok(out)
    }

    /// The result: into the macro named in `[\name]` if that follows, typeset otherwise.
    fn xstring_result(&mut self, v: Vec<Token>) -> Result<(), Error> {
        if let Some(t) = self.next_nonspace() {
            if t.is_char('[') {
                let name = self.read_until_char(']', false)?;
                if let Some(name) = name.into_iter().find(|x| !x.is_space()) {
                    self.define_tokens(&name, v);
                }
                return Ok(());
            }
            self.back(t);
        }
        self.push_tokens(v);
        Ok(())
    }

    fn define_tokens(&mut self, name: &Token, v: Vec<Token>) {
        let m = Macro { params: Vec::new(), body: v.into_iter().map(Body::Tok).collect(), optional: None, protected: false };
        self.set_meaning(name, Meaning::Macro(Rc::new(m)), false);
    }

    /// Runs the xstring command `name`.
    pub fn xstring(&mut self, name: &str) -> Result<(), Error> {
        // the occurrence counts in brackets before the texts
        let counts =
            if matches!(name, "StrBefore" | "StrBehind" | "StrCut" | "StrBetween" | "StrSubstitute" | "StrDel" | "IfSubStr") { self.xstring_counts()? } else { Vec::new() };
        let n = counts.first().copied();
        let text = self.xstring_text()?;
        let s = units(&text);
        let result = match name {
            "StrLen" => string_tokens(&s.len().to_string()),
            "StrLeft" | "StrRight" | "StrChar" => {
                let a = self.read_undelimited()?;
                let n = self.xstring_int(a)?;
                let k = n.clamp(0, s.len() as i64) as usize;
                match name {
                    "StrLeft" => join(&s[..k]),
                    "StrRight" => join(&s[s.len() - k..]),
                    _ => usize::try_from(n - 1).ok().and_then(|i| s.get(i)).map(|u| u.to_vec()).unwrap_or_default(),
                }
            }
            "StrMid" => {
                let (a, b) = (self.read_undelimited()?, self.read_undelimited()?);
                let a = self.xstring_int(a)?.max(1) as usize;
                let b = self.xstring_int(b)?.min(s.len() as i64).max(0) as usize;
                if a <= b {
                    join(&s[a - 1..b])
                } else {
                    Vec::new()
                }
            }
            "StrSubstitute" | "StrDel" => {
                let from = self.xstring_text()?;
                let to = if name == "StrSubstitute" { self.xstring_text()? } else { Vec::new() };
                let p = units(&from);
                // a count of 0 (the default) replaces them all
                let n = n.unwrap_or(0);
                let found = occurrences(&s, &p);
                let mut out = Vec::new();
                let mut at = 0;
                for (k, &i) in found.iter().enumerate() {
                    if n > 0 && k as i64 >= n {
                        break;
                    }
                    out.extend(join(&s[at..i]));
                    out.extend(to.iter().cloned());
                    at = i + p.len();
                }
                out.extend(join(&s[at..]));
                out
            }
            "StrBetween" => {
                let (a, b) = (self.xstring_text()?, self.xstring_text()?);
                let (pa, pb) = (units(&a), units(&b));
                // the n1-th A and the n2-th B, both counted from the start
                let n1 = n.unwrap_or(1);
                let n2 = counts.get(1).copied().unwrap_or(1);
                let nth = |found: Vec<usize>, k: i64| usize::try_from(k - 1).ok().and_then(|k| found.get(k).copied());
                match (nth(occurrences(&s, &pa), n1), nth(occurrences(&s, &pb), n2)) {
                    (Some(i), Some(j)) if j >= i + pa.len() => join(&s[i + pa.len()..j]),
                    _ => Vec::new(),
                }
            }
            "StrCount" => {
                let p = self.xstring_text()?;
                string_tokens(&occurrences(&s, &units(&p)).len().to_string())
            }
            "IfSubStr" | "IfStrEq" | "IfEq" | "IfBeginWith" | "IfEndWith" => {
                let p = self.xstring_text()?;
                let p = units(&p);
                let test = match name {
                    "IfSubStr" => occurrences(&s, &p).len() as i64 >= n.unwrap_or(1),
                    "IfStrEq" => s == p,
                    // the same text, or the same number (`1` and `01.0`)
                    "IfEq" => {
                        let num = |v: &[&[Token]]| super::tokens_to_string(&join(v)).trim().parse::<f64>().ok();
                        match (num(&s), num(&p)) {
                            (Some(a), Some(b)) => a == b,
                            _ => s == p,
                        }
                    }
                    "IfBeginWith" => s.starts_with(&p),
                    _ => s.ends_with(&p),
                };
                let (yes, no) = (self.read_undelimited()?, self.read_undelimited()?);
                self.push_tokens(if test { yes } else { no });
                return Ok(());
            }
            _ => {
                // \StrBefore, \StrBehind, \StrCut: around the n-th occurrence
                let p = self.xstring_text()?;
                let p = units(&p);
                let found = occurrences(&s, &p);
                let k = n.unwrap_or(1);
                let at = if name == "StrCut" { found.get(k.max(1) as usize - 1) } else { usize::try_from(k - 1).ok().and_then(|k| found.get(k)) };
                let (before, behind) = match at {
                    Some(&i) => (join(&s[..i]), join(&s[i + p.len()..])),
                    None if name == "StrCut" => (text.clone(), Vec::new()),
                    None => (Vec::new(), Vec::new()),
                };
                match name {
                    "StrBefore" => before,
                    "StrBehind" => behind,
                    _ => {
                        // \StrCut{text}{sep}{\left}{\right}
                        let (l, r) = (self.read_undelimited()?, self.read_undelimited()?);
                        for (target, v) in [(l, before), (r, behind)] {
                            if let Some(m) = target.into_iter().find(|x| !x.is_space()) {
                                self.define_tokens(&m, v);
                            }
                        }
                        return Ok(());
                    }
                }
            }
        };
        self.xstring_result(result)
    }
}
