//! Running pgfkeys: key lists, handlers (`.style`, `.code`, `.default`, `.initial`, `.store in`,
//! `.is choice`…), TikZ's `.unknown` (a color, an arrow specification, a shape name), and the keys
//! that TikZ defines with code, done natively. Keys that TikZ defines as styles are defined here the
//! same way, from TikZ's own definitions.

use std::rc::Rc;

use crate::canvas::{Cap, Join};
use crate::keys::{self, macro_name};
use crate::scene::{Point, Transform};
use crate::tex::{self, Body, Cat, Macro, Meaning, Param, Token};
use crate::{Error, PT};

use super::{macro_from, macro_tokens, tikra_tokens, Engine, EXTENSION_KEY_BASE, NATIVE_KEY_BASE};

pub type KeyFn = fn(&mut Engine, Option<Vec<Token>>) -> Result<(), Error>;

fn group(v: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(v.len() + 2);
    out.push(Token::Char('{', Cat::Begin));
    out.extend(v);
    out.push(Token::Char('}', Cat::End));
    out
}

fn is_novalue(v: &[Token]) -> bool {
    let v: Vec<&Token> = v.iter().filter(|t| !t.is_space()).collect();
    v.len() == 1 && v[0].is_cs("pgfkeysnovalue")
}

impl Engine {
    /// Runs a key list now, with `path` as the default path (or the current one).
    pub fn process_keys(&mut self, list: Vec<Token>, path: Option<&str>) -> Result<(), Error> {
        let tokens = self.key_invocations(list, path, true);
        self.run_tokens(tokens)
    }

    /// `\tikzset`, `\pgfkeys`, `\pgfkeysalso`: the list's keys, one after the other.
    pub fn keys_command(&mut self, list: Vec<Token>, path: Option<&str>, restore: bool) -> Result<(), Error> {
        let tokens = self.key_invocations(list, path, restore);
        self.run_tokens(tokens)
    }

    fn key_invocations(&mut self, list: Vec<Token>, path: Option<&str>, restore: bool) -> Vec<Token> {
        let old = self.st.key_path.clone();
        let mut out = Vec::new();
        if let Some(p) = path {
            out.push(Token::cs("tikra@setpath"));
            out.extend(group(tex::string_tokens(p)));
        }
        for item in keys::split_list(&list) {
            // a style called without a value passes `\pgfkeysnovalue` on: pgfkeys skips it
            if item.key.trim() == "\\pgfkeysnovalue" && item.value.is_none() {
                continue;
            }
            out.push(Token::cs("tikra@key"));
            out.extend(group(tikra_tokens(&item.key)));
            out.extend(group(item.value.unwrap_or_else(|| vec![Token::cs("pgfkeysnovalue")])));
        }
        if restore && path.is_some() {
            out.push(Token::cs("tikra@setpath"));
            out.extend(group(tex::string_tokens(&old)));
        }
        out
    }

    /// `\tikra@key{key}{value}`: one key.
    pub fn invoke_key_command(&mut self) -> Result<(), Error> {
        let key = self.argument_string()?;
        let value = self.argument()?;
        let value = if is_novalue(&value) { None } else { Some(value) };
        // a label's text is a node's: it keeps them with `handle active characters in nodes`
        let text = matches!(key.trim(), "label" | "pin" | "node contents");
        let value = match value {
            Some(v) if self.st.active_in_code && !(text && self.st.active_in_nodes) => Some(deactivate(v)),
            v => v,
        };
        self.invoke(&key, value)
    }

    pub fn invoke(&mut self, key: &str, value: Option<Vec<Token>>) -> Result<(), Error> {
        self.keys_run += 1;
        if self.keys_run > super::MAX_KEYS {
            if self.keys_run == super::MAX_KEYS + 1 {
                self.warn(format!("more than {} options (styles that multiply?): the rest ignored", super::MAX_KEYS));
            }
            return Ok(());
        }
        // `/handlers/first char syntax`: a key that starts with a character that has a macro there
        // is that macro, with the whole key (and its value) as argument
        // (looked up only once it is on: every key passes here)
        if let Some(c) = key.trim_start().chars().next().filter(|_| self.first_char_syntax) {
            let handler = format!("/handlers/first char syntax/the character {c}");
            if let Some(code) = keys::value(&self.tex, &handler).filter(|_| self.key_text("/handlers/first char syntax").trim() == "true") {
                let mut whole = tikra_tokens(key.trim());
                if let Some(v) = value {
                    whole.push(Token::Char('=', tex::Cat::Other));
                    whole.extend(v);
                }
                let mut run = code;
                run.extend(group(whole));
                return self.run_tokens(run);
            }
        }
        let path = keys::full_path(&self.st.key_path, key);
        if path == "/handlers/first char syntax" {
            self.first_char_syntax = true;
        }
        // a key named like a handler (`characters/.append`) is a key: pgfkeys looks for keys first
        if let Some((base, h)) = keys::handler(&path).filter(|_| !self.tex.is_defined(&macro_name(&format!("{path}/.@cmd")))) {
            let (mut base, h) = (base.to_string(), h.to_string());
            // `/handler config=full or existing`: a handler that changes a key that does not exist
            // goes to the key of the same name on the search path (pgfplots' `every mark/.append style`)
            if matches!(h.as_str(), ".append style" | ".prefix style" | ".append code" | ".prefix code" | ".append" | ".prefix" | ".add") && !self.key_exists(&base) {
                if let Some(list) = keys::value(&self.tex, &format!("{}/.@searchalso", keys::parent(&base))) {
                    let name = keys::name(&base).to_string();
                    let found = tex::tokens_to_string(&list).split(',').map(|p| format!("{}/{name}", p.trim().trim_end_matches('/'))).find(|other| self.key_exists(other));
                    if let Some(other) = found {
                        base = other;
                    }
                }
            }
            return self.handler(&base, &h, value);
        }
        if self.run_key(&path, value.clone())? {
            return Ok(());
        }
        self.unknown(&path, value)
    }

    /// Whether a key has code or a value.
    pub fn key_exists(&self, path: &str) -> bool {
        self.tex.is_defined(&macro_name(path)) || self.tex.is_defined(&macro_name(&format!("{path}/.@cmd")))
    }

    /// Runs the key at `path` if it is defined; false if it is not.
    pub fn run_key(&mut self, path: &str, value: Option<Vec<Token>>) -> Result<bool, Error> {
        let cmd = self.tex.meaning_of(&macro_name(&format!("{path}/.@cmd")));
        let value = value.or_else(|| keys::value(&self.tex, &format!("{path}/.@def")));
        match cmd {
            Meaning::Native(id) if id >= EXTENSION_KEY_BASE => {
                let f = self.extension_fns[(id - EXTENSION_KEY_BASE) as usize];
                self.current_key.clear();
                self.current_key.push_str(path);
                let v = value.filter(|v| !is_novalue(v)).map(crate::extension::Tokens);
                f(&mut crate::extension::Context { e: self }, v)?;
                Ok(true)
            }
            Meaning::Native(id) if id >= NATIVE_KEY_BASE => {
                let f = self.key_fns[(id - NATIVE_KEY_BASE) as usize];
                self.current_key.clear();
                self.current_key.push_str(path);
                let v = value.filter(|v| !is_novalue(v));
                f(self, v)?;
                Ok(true)
            }
            Meaning::Macro(m) => {
                let n = m.params.iter().filter(|p| matches!(p, Param::Arg(_))).count();
                let v = value.unwrap_or_else(|| vec![Token::cs("pgfkeysnovalue")]);
                let args = if n <= 1 { vec![v] } else { split_arguments(&v, n) };
                let code = tex::substitute(&m.body, &args);
                self.run_tokens(code)?;
                Ok(true)
            }
            _ => {
                if self.tex.is_defined(&macro_name(path)) {
                    match value {
                        Some(v) => self.tex.set_meaning(&Token::cs(&macro_name(path)), macro_tokens(v), false),
                        None => {
                            // a stored value used as a key: its value is "typeset" (inserted)
                            if let Some(v) = keys::value(&self.tex, path) {
                                self.tex.push_tokens(v);
                            }
                        }
                    }
                    return Ok(true);
                }
                Ok(false)
            }
        }
    }

    fn unknown(&mut self, path: &str, value: Option<Vec<Token>>) -> Result<(), Error> {
        let parent = keys::parent(path).to_string();
        let name = keys::name(path).to_string();
        // the parent's .unknown handler (TikZ's: a color, an arrow specification, a shape)
        let handler = macro_name(&format!("{parent}/.unknown/.@cmd"));
        if self.tex.is_defined(&handler) {
            self.tex.define_string("pgfkeyscurrentname", &name, false);
            self.tex.define_string("pgfkeyscurrentkey", path, false);
            self.tex.set_meaning(&Token::cs("pgfkeyscurrentvalue"), macro_tokens(value.clone().unwrap_or_else(|| vec![Token::cs("pgfkeysnovalue")])), false);
            if self.run_key(&format!("{parent}/.unknown"), value.clone())? {
                return Ok(());
            }
        }
        // .search also
        if let Some(list) = keys::value(&self.tex, &format!("{parent}/.@searchalso")) {
            let text = tex::tokens_to_string(&list);
            for p in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let other = format!("{}/{name}", p.trim_end_matches('/'));
                if self.run_key(&other, value.clone())? {
                    return Ok(());
                }
            }
        }
        self.unknown_warning(&name, path);
        Ok(())
    }

    fn unknown_warning(&mut self, name: &str, path: &str) {
        let _ = path;
        self.warn(format!("option '{name}' not supported: ignored"));
    }

    /// `/tikz/.unknown`: a color, an arrow specification or a shape name; otherwise a `/pgf/` key.
    fn tikz_unknown(&mut self, value: Option<Vec<Token>>) -> Result<(), Error> {
        // `\pgfkeyscurrentname`, as `unknown` defines it before it runs the handler
        let name = if let Meaning::Macro(m) = self.tex.meaning_of("pgfkeyscurrentname") { tex::tokens_to_string(&tex::substitute(&m.body, &[])) } else { String::new() };
        if name.starts_with('"') {
            return self.quotes(&name, value);
        }
        // `myself:rotate={…}`, `:fill={…}`: an animated attribute
        if let (Some(v), true) = (&value, name.contains(':')) {
            if let Some(o) = self.animation_option(&name, v.clone())? {
                self.process_keys(o, Some("/tikz"))?;
            }
            return Ok(());
        }
        if value.is_none() {
            if let Some(c) = self.color(&name) {
                self.st.color = c;
                self.st.draw_color = None;
                self.st.fill_color = None;
                self.st.text_color = None;
                return Ok(());
            }
            if let Some((a, b)) = super::arrows::parse_arrows(&name, &self.st.tips) {
                self.st.start_arrow = a;
                self.st.end_arrow = b;
                return Ok(());
            }
            if super::nodes::is_shape(&name) || self.user_shapes.contains_key(&name) {
                self.tex.define_string(&macro_name("/tikz/shape"), &name, false);
                return Ok(());
            }
        }
        let pgf = format!("/pgf/{name}");
        if self.run_key(&pgf, value)? {
            return Ok(());
        }
        self.unknown_warning(&name, &format!("/tikz/{name}"));
        Ok(())
    }

    /// The quotes library: `"text" options` makes a label in node options, and a node on the path in
    /// `to`/`edge` options; `'` after the quotes means `swap`.
    fn quotes(&mut self, key: &str, value: Option<Vec<Token>>) -> Result<(), Error> {
        // pgfkeys split `"x=1"` at its `=`: put it back together
        let mut whole = key.to_string();
        if let Some(v) = value {
            whole.push('=');
            whole.push_str(&tex::tokens_to_string(&v));
        }
        let Some(end) = whole[1..].find('"').map(|i| i + 1) else {
            self.warn(format!("quotes: closing '\"' missing in {whole}"));
            return Ok(());
        };
        let text = whole[1..end].to_string();
        let mut options = whole[end + 1..].trim().to_string();
        if let Some(rest) = options.strip_prefix('\'') {
            options = format!("swap,{}", rest.trim());
        }
        // `"text" above, red` is one key in a list only up to the comma: the rest are options
        let options = options.split_whitespace().collect::<Vec<_>>().join(" ");
        if self.st.quotes_in_pic {
            self.st.pic_text = Some((tikra_tokens(&text), tikra_tokens(&options)));
        } else if self.st.quotes_on_path {
            let mut o = tikra_tokens("every edge quotes/.try,");
            o.extend(tikra_tokens(&options));
            self.st.quoted.push((tikra_tokens(&text), o));
        } else {
            // `quotes mean pin`: pins instead of labels
            let pin = matches!(self.tex.meaning_of("tikra@quotes@pin"), Meaning::Macro(m) if tex::tokens_to_string(&tex::substitute(&m.body, &[])).trim() == "1");
            let spec = format!("[direction shorthands,every {} quotes/.try,{options}]{text}", if pin { "pin" } else { "label" });
            self.st.node.labels.push((tikra_tokens(&spec), pin));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- handlers

    fn define_code(&mut self, path: &str, n: usize, body: Vec<Token>) {
        let params = (1..=n as u8).map(Param::Arg).collect();
        let m = Macro { params, body: tex::parse_body(body), optional: None, protected: false };
        self.tex.set_meaning(&Token::cs(&macro_name(&format!("{path}/.@cmd"))), Meaning::Macro(Rc::new(m)), false);
    }

    /// The code of a key, as tokens with `#1`… in place, if it is a macro.
    fn code_tokens(&self, path: &str) -> Option<(usize, Vec<Token>)> {
        match self.tex.meaning_of(&macro_name(&format!("{path}/.@cmd"))) {
            Meaning::Macro(m) => {
                let n = m.params.len();
                let mut v = Vec::new();
                for b in &m.body {
                    match b {
                        Body::Tok(t) => {
                            if matches!(t, Token::Char(_, Cat::Param)) {
                                // a ## inside: keep it doubled so the code stays the same once defined again
                                v.push(t.clone());
                            }
                            v.push(t.clone());
                        }
                        Body::Arg(k) => {
                            v.push(Token::Char('#', Cat::Param));
                            v.push(Token::other(char::from(b'0' + k)));
                        }
                    }
                }
                Some((n, v))
            }
            _ => None,
        }
    }

    fn style_body(value: Vec<Token>) -> Vec<Token> {
        let mut b = vec![Token::cs("pgfkeysalso")];
        b.extend(group(value));
        b
    }

    fn handler(&mut self, base: &str, h: &str, value: Option<Vec<Token>>) -> Result<(), Error> {
        let v = value.clone().unwrap_or_default();
        match h {
            ".cd" => self.st.key_path = base.to_string(),
            ".style" | ".estyle" => {
                let v = if h == ".estyle" { self.tex.expand_fully(v)? } else { v };
                self.define_code(base, 1, Self::style_body(v));
            }
            ".append style" | ".prefix style" => {
                let (n, old) = self.code_tokens(base).unwrap_or((1, Vec::new()));
                let new = Self::style_body(v);
                let body = if h == ".append style" { [old, new].concat() } else { [new, old].concat() };
                self.define_code(base, n.max(1), body);
            }
            ".code" | ".ecode" => {
                let v = if h == ".ecode" { self.tex.expand_fully(v)? } else { v };
                self.define_code(base, 1, v);
                // a pgfplots coordinate filter given as code drops its expressions
                if base.ends_with(" filter") {
                    self.tex.set_meaning(&Token::cs(&macro_name(&format!("{base}/@expressions"))), macro_tokens(Vec::new()), false);
                }
            }
            // pgfplots' `coordinate style/.condition={test}{style}` and `coordinate style/.from={style}`
            ".condition" => {
                let path = format!("{base}/@conditions");
                let mut list = keys::value(&self.tex, &path).unwrap_or_default();
                list.extend(v);
                self.tex.set_meaning(&Token::cs(&macro_name(&path)), macro_tokens(list), false);
            }
            ".from" => {
                self.tex.set_meaning(&Token::cs(&macro_name(&format!("{base}/@from"))), macro_tokens(v), false);
            }
            // pgfplots' `x filter/.expression={…}`: the filter's value is the expression's (after
            // the filter's code, `.append expression` after the expressions before)
            ".expression" | ".append expression" => {
                let path = format!("{base}/@expressions");
                let mut list = if h == ".expression" {
                    self.define_code(base, 1, Vec::new());
                    Vec::new()
                } else {
                    keys::value(&self.tex, &path).unwrap_or_default()
                };
                list.extend(group(v));
                self.tex.set_meaning(&Token::cs(&macro_name(&path)), macro_tokens(list), false);
            }
            ".append code" | ".prefix code" => {
                let (n, old) = self.code_tokens(base).unwrap_or((1, Vec::new()));
                let body = if h == ".append code" { [old, v].concat() } else { [v, old].concat() };
                self.define_code(base, n.max(1), body);
            }
            ".code 2 args" | ".style 2 args" | ".code n args" | ".style n args" | ".ecode 2 args" | ".estyle 2 args" => {
                let (n, body) = if h.contains("n args") {
                    let args = split_arguments(&v, 2);
                    let n: usize = tex::tokens_to_string(&args[0]).trim().parse().unwrap_or(1);
                    (n, args[1].clone())
                } else {
                    (2, v)
                };
                let body = if h.contains("style") { Self::style_body(body) } else { body };
                self.define_code(base, n, body);
            }
            ".code args" | ".style args" | ".ecode args" | ".estyle args" => {
                // as pgf: a macro with the parameter text, called with the value and \pgfeov
                let args = split_arguments(&v, 2);
                let body = if h.contains("style") { Self::style_body(args[1].clone()) } else { args[1].clone() };
                let aux = Token::cs(&macro_name(&format!("{base}/.@args")));
                let mut def = vec![Token::cs("def"), aux.clone()];
                def.extend(args[0].clone());
                def.push(Token::cs("pgfeov"));
                def.extend(group(body));
                self.run_tokens(def)?;
                self.define_code(base, 1, vec![aux, Token::Char('#', Cat::Param), Token::other('1'), Token::cs("pgfeov")]);
            }
            ".tip" => {
                // `name/.tip=spec`: \pgfdeclarearrow{name=name, means=spec}, the name being the last part of the path
                let name = base.rsplit('/').next().unwrap_or(base).trim().to_string();
                let means = tex::tokens_to_string(&v);
                std::rc::Rc::make_mut(&mut self.st.tips).declare(&name, &means);
            }
            ".default" => self.tex.set_meaning(&Token::cs(&macro_name(&format!("{base}/.@def"))), macro_tokens(v), false),
            ".initial" => self.tex.set_meaning(&Token::cs(&macro_name(base)), macro_tokens(v), false),
            // `/K/.is family`: the key `/K` makes `/K` the default path
            ".is family" => self.define_code(base, 0, tikra_tokens(&format!("\\pgfkeysalso{{{base}/.cd}}"))),
            ".value required" | ".value forbidden" | ".show code" | ".show value" | ".activate family" | ".deactivate family" | ".belongs to family" => {}
            ".store in" | ".estore in" => {
                let target: Vec<Token> = v.into_iter().filter(|t| !t.is_space()).collect();
                let mut body = vec![Token::cs(if h == ".store in" { "def" } else { "edef" })];
                body.extend(target);
                body.extend(group(vec![Token::Char('#', Cat::Param), Token::other('1')]));
                self.define_code(base, 1, body);
            }
            ".is if" => {
                let name = tex::tokens_to_string(&v).trim().to_string();
                // key=true runs \nametrue
                let body = vec![Token::cs("csname"), Token::other(' ')].into_iter().take(1).collect::<Vec<_>>();
                let mut body = body;
                body.extend(tex::string_tokens(&name));
                body.push(Token::Char('#', Cat::Param));
                body.push(Token::other('1'));
                body.push(Token::cs("endcsname"));
                self.define_code(base, 1, body);
                self.tex.set_meaning(&Token::cs(&macro_name(&format!("{base}/.@def"))), macro_from("true"), false);
            }
            ".is choice" => {
                // base=choice runs base/choice
                let mut body = vec![Token::cs("pgfkeysalso")];
                let mut inner = tex::string_tokens(&format!("{base}/"));
                inner.push(Token::Char('#', Cat::Param));
                inner.push(Token::other('1'));
                body.extend(group(inner));
                self.define_code(base, 1, body);
            }
            ".search also" => {
                let s = tex::tokens_to_string(&v);
                self.tex.define_string(&macro_name(&format!("{base}/.@searchalso")), &s, false);
            }
            ".try" | ".retry" | ".lastretry" => {
                let _ = self.run_key(base, value)?;
            }
            ".expand once" | ".expand twice" | ".expanded" => {
                let v = self.tex.expand_fully(v)?;
                if !self.run_key(base, Some(v.clone()))? {
                    self.unknown(base, Some(v))?;
                }
            }
            ".get" => {
                if let Some(target) = v.into_iter().find(|t| !t.is_space()) {
                    let val = keys::value(&self.tex, base).unwrap_or_default();
                    self.tex.set_meaning(&target, macro_tokens(val), false);
                }
            }
            ".add" => {
                let args = split_arguments(&v, 2);
                let old = keys::value(&self.tex, base).unwrap_or_default();
                let new = [args[0].clone(), old, args[1].clone()].concat();
                self.tex.set_meaning(&Token::cs(&macro_name(base)), macro_tokens(new), false);
            }
            ".append" | ".prefix" => {
                let old = keys::value(&self.tex, base).unwrap_or_default();
                let new = if h == ".append" { [old, v].concat() } else { [v, old].concat() };
                self.tex.set_meaning(&Token::cs(&macro_name(base)), macro_tokens(new), false);
            }
            ".forward to" => {
                let target = tex::tokens_to_string(&v).trim().to_string();
                let mut inner = tex::string_tokens(&format!("{target}="));
                inner.extend(group(vec![Token::Char('#', Cat::Param), Token::other('1')]));
                self.define_code(base, 1, Self::style_body(inner));
            }
            ".list" => {
                for item in super::foreach_items(&v) {
                    if !self.run_key(base, Some(item.clone()))? {
                        self.unknown(base, Some(item))?;
                    }
                }
            }
            ".pic" => {
                // name/.pic={code}: the pic type `name`, under /tikz/pics
                let name = base.trim_start_matches("/tikz/").trim_start_matches('/');
                let mut body = tikra_tokens("code=");
                body.extend(group(v));
                self.define_code(&format!("/tikz/pics/{name}"), 1, Self::style_body(body));
            }
            ".unknown" => {
                // `/foo/.unknown/.code={…}` reaches here as a handler of the path /foo/.unknown
                self.define_code(&format!("{base}/.unknown"), 1, v);
            }
            other => {
                // a user handler: /handlers/<name>/.code
                let user = format!("/handlers/{}", other);
                self.tex.define_string("pgfkeyscurrentpath", base, false);
                if !self.run_key(&user, value)? {
                    self.warn(format!("key handler '{other}' not supported (in '{base}/{other}'): ignored"));
                }
            }
        }
        Ok(())
    }
}

/// `{a}{b}` (or `ab`) as `n` arguments, TeX-style.
pub fn split_arguments(v: &[Token], n: usize) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < n {
        while i < v.len() && v[i].is_space() {
            i += 1;
        }
        if i >= v.len() {
            out.push(Vec::new());
            continue;
        }
        if matches!(v[i], Token::Char(_, Cat::Begin)) {
            let mut depth = 0usize;
            let start = i;
            while i < v.len() {
                match v[i] {
                    Token::Char(_, Cat::Begin) => depth += 1,
                    Token::Char(_, Cat::End) => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            out.push(v[start + 1..i.min(v.len())].to_vec());
            i += 1;
        } else {
            out.push(vec![v[i].clone()]);
            i += 1;
        }
    }
    out
}

// -------------------------------------------------------------------- the native keys

fn text(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<String, Error> {
    Ok(match v {
        Some(v) => e.tex.expand_to_string(v.clone())?.trim().to_string(),
        None => String::new(),
    })
}

fn length(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<Option<f64>, Error> {
    match v {
        Some(v) => Ok(Some(e.length_of(v.clone())?)),
        None => Ok(None),
    }
}

fn number(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<Option<f64>, Error> {
    match v {
        Some(v) => Ok(Some(e.number_of(v.clone())?)),
        None => Ok(None),
    }
}

fn set_value(e: &mut Engine, path: &str, s: &str) {
    e.tex.define_string(&macro_name(path), s, false);
}

/// A transformation added to those of the options (`\tikz@addtransform`).
fn add_transform(e: &mut Engine, t: Transform) {
    e.st.pending = e.st.pending.compose(&t);
}

/// A point for `shift=`: in the coordinate system the shift happens in.
fn shift_vector(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<Point, Error> {
    // read as a coordinate on a path is: a length register (`\newdimen\R`) is its value
    let s = e.math_string(v.clone().unwrap_or_default())?;
    let inner = s.trim().trim_start_matches('{').trim_end_matches('}').trim().trim_start_matches('(').trim_end_matches(')').to_string();
    let p = e.coordinate(&inner)?.p;
    // in the coordinate system as it is: shifts given before (still pending) do not change it
    Ok(e.st.transform.inverse().apply(p))
}

fn dash_from(e: &mut Engine, spec: &str) -> Result<Option<Vec<f64>>, Error> {
    // on 3pt off 2pt on \pgflinewidth off 2pt; a length that `\the` gives keeps no space before
    // what follows (`on \the\pgflinewidth off 2pt` is `0.4ptoff`): TeX's length ends at its unit
    let mut spec = spec.to_string();
    for unit in ["pt", "cm", "mm", "bp", "em", "ex", "in", "pc"] {
        spec = spec.replace(&format!("{unit}on"), &format!("{unit} on")).replace(&format!("{unit}off"), &format!("{unit} off"));
    }
    // nor does TeX need a space after `on` and `off` (`on5pt off3pt`)
    let mut words: Vec<&str> = Vec::new();
    for w in spec.split_whitespace() {
        match ["off", "on"].iter().find(|k| w.len() > k.len() && w.starts_with(*k)) {
            Some(k) => words.extend([&w[..k.len()], &w[k.len()..]]),
            None => words.push(w),
        }
    }
    let mut v = Vec::new();
    let mut i = 0;
    while i + 1 < words.len() {
        match words[i] {
            "on" | "off" => {
                let w = words[i + 1];
                let len = if w.contains("pgflinewidth") {
                    let f: f64 = w.replace("\\pgflinewidth", "").trim_end_matches('*').parse().unwrap_or(1.0);
                    f * e.st.line_width
                } else {
                    e.length(w)?
                };
                v.push(len);
                i += 2;
            }
            _ => i += 1,
        }
    }
    Ok(if v.is_empty() { None } else { Some(v) })
}

macro_rules! keyfn {
    ($name:ident, |$e:ident, $v:ident| $body:expr) => {
        fn $name($e: &mut Engine, $v: Option<Vec<Token>>) -> Result<(), Error> {
            $body
        }
    };
}

keyfn!(k_draw, |e, v| {
    let s = text(e, &v)?;
    match s.as_str() {
        "" => e.st.mode.draw = true,
        "none" => e.st.mode.draw = false,
        c => {
            e.st.mode.draw = true;
            match e.color(c) {
                Some(c) => e.st.draw_color = Some(c),
                None => e.warn(format!("unknown color '{c}'")),
            }
        }
    }
    Ok(())
});

keyfn!(k_fill, |e, v| {
    let s = text(e, &v)?;
    match s.as_str() {
        "" => e.st.mode.fill = true,
        "none" => e.st.mode.fill = false,
        c => {
            e.st.mode.fill = true;
            match e.color(c) {
                Some(c) => e.st.fill_color = Some(c),
                None => e.warn(format!("unknown color '{c}'")),
            }
        }
    }
    Ok(())
});

keyfn!(k_color, |e, v| {
    let s = text(e, &v)?;
    match e.color(&s) {
        Some(c) => {
            e.st.color = c;
            e.st.draw_color = None;
            e.st.fill_color = None;
            e.st.text_color = None;
        }
        None => e.warn(format!("unknown color '{s}'")),
    }
    Ok(())
});

keyfn!(k_text, |e, v| {
    let s = text(e, &v)?;
    match e.color(&s) {
        Some(c) => e.st.text_color = Some(c),
        None => e.warn(format!("unknown color '{s}'")),
    }
    Ok(())
});

keyfn!(k_clip, |e, _v| {
    e.st.mode.clip = true;
    Ok(())
});

keyfn!(k_bounding_box, |e, _v| {
    e.st.mode.bounding_box = true;
    Ok(())
});

keyfn!(k_shade, |e, _v| {
    e.st.mode.shade = true;
    Ok(())
});

keyfn!(k_pattern, |e, v| {
    let s = text(e, &v)?;
    e.st.mode.pattern = true;
    e.st.pattern_name = Some(if s.is_empty() { "north east lines".into() } else { s });
    Ok(())
});

keyfn!(k_pattern_color, |e, v| {
    let s = text(e, &v)?;
    e.st.pattern_color = e.color(&s);
    Ok(())
});

keyfn!(k_line_width, |e, v| {
    if let Some(w) = length(e, &v)? {
        e.st.line_width = w;
        // \pgflinewidth follows
        let sp = (w / PT * 65536.0).round() as i64;
        if let Meaning::Dimen(r) = e.tex.meaning_of("pgflinewidth") {
            e.tex.set_dimen(r, sp, false);
        }
    }
    Ok(())
});

keyfn!(k_dash_pattern, |e, v| {
    let s = text(e, &v)?;
    let d = dash_from(e, &s)?;
    let phase = e.st.dash.as_ref().map_or(0.0, |d| d.1);
    e.st.dash = d.map(|d| (d, phase));
    Ok(())
});

keyfn!(k_dash_phase, |e, v| {
    let p = length(e, &v)?.unwrap_or(0.0);
    if let Some(d) = &mut e.st.dash {
        d.1 = p;
    } else {
        e.st.dash = Some((Vec::new(), p));
    }
    Ok(())
});

keyfn!(k_dash, |e, v| {
    // dash=on 2pt off 1pt phase 1pt
    let s = text(e, &v)?;
    if s == "solid" {
        e.st.dash = None;
        return Ok(());
    }
    let (pattern, phase) = match s.split_once("phase") {
        Some((a, b)) => (a.to_string(), e.length(b.trim())?),
        None => (s.clone(), 0.0),
    };
    e.st.dash = dash_from(e, &pattern)?.map(|d| (d, phase));
    Ok(())
});

keyfn!(k_solid, |e, _v| {
    e.st.dash = None;
    Ok(())
});

keyfn!(k_line_cap, |e, v| {
    e.st.cap = match text(e, &v)?.as_str() {
        "round" => Cap::Round,
        "rect" => Cap::Square,
        _ => Cap::Butt,
    };
    Ok(())
});

keyfn!(k_line_join, |e, v| {
    e.st.join = match text(e, &v)?.as_str() {
        "round" => Join::Round,
        "bevel" => Join::Bevel,
        _ => Join::Miter,
    };
    Ok(())
});

keyfn!(k_miter_limit, |e, v| {
    if let Some(m) = number(e, &v)? {
        e.st.miter_limit = m;
    }
    Ok(())
});

keyfn!(k_opacity, |e, v| {
    let o = number(e, &v)?.unwrap_or(1.0);
    e.st.draw_opacity = o;
    e.st.fill_opacity = o;
    e.st.text_opacity = o;
    Ok(())
});

keyfn!(k_draw_opacity, |e, v| {
    e.st.draw_opacity = number(e, &v)?.unwrap_or(1.0);
    Ok(())
});

keyfn!(k_fill_opacity, |e, v| {
    e.st.fill_opacity = number(e, &v)?.unwrap_or(1.0);
    Ok(())
});

keyfn!(k_text_opacity, |e, v| {
    e.st.text_opacity = number(e, &v)?.unwrap_or(1.0);
    Ok(())
});

keyfn!(k_even_odd, |e, _v| {
    e.st.even_odd = true;
    Ok(())
});

keyfn!(k_nonzero, |e, _v| {
    e.st.even_odd = false;
    Ok(())
});

keyfn!(k_rounded, |e, v| {
    e.st.rounded = length(e, &v)?.unwrap_or(4.0 * PT);
    e.st.rounded_out = e.st.rounded;
    Ok(())
});

keyfn!(k_sharp, |e, _v| {
    e.st.rounded = 0.0;
    e.st.rounded_out = 0.0;
    Ok(())
});

keyfn!(k_arrows, |e, v| {
    let s = text(e, &v)?;
    match super::arrows::parse_arrows(&s, &e.st.tips) {
        Some((a, b)) => {
            e.st.start_arrow = a;
            e.st.end_arrow = b;
        }
        None => e.warn(format!("arrow specification '{s}' not supported: ignored")),
    }
    Ok(())
});

keyfn!(k_default_arrow, |e, v| {
    // without a value it declares nothing: a library can give it a meaning (`bare arrow`)
    if v.is_none() {
        return e.process_keys(tikra_tokens("bare arrow/.try"), Some("/tikz"));
    }
    // TikZ's `>`: `\pgfdeclarearrow{name=<->, means=…}`
    let s = text(e, &v)?;
    std::rc::Rc::make_mut(&mut e.st.tips).declare("<->", &s);
    Ok(())
});

keyfn!(k_tips, |e, v| {
    let s = text(e, &v)?;
    match super::arrows::TipsMode::from_name(&s) {
        Some(m) => e.st.tips_mode = m,
        None => e.warn(format!("tips={s} not supported: ignored")),
    }
    Ok(())
});

keyfn!(k_shorten_end, |e, v| {
    e.st.shorten_end = length(e, &v)?.unwrap_or(0.0);
    Ok(())
});

keyfn!(k_shorten_start, |e, v| {
    e.st.shorten_start = length(e, &v)?.unwrap_or(0.0);
    Ok(())
});

keyfn!(k_double, |e, v| {
    let s = text(e, &v)?;
    if s == "none" {
        e.st.double = None;
        return Ok(());
    }
    e.st.double = Some(if s.is_empty() { crate::color::Color::WHITE } else { e.color(&s).unwrap_or(crate::color::Color::WHITE) });
    Ok(())
});

keyfn!(k_double_distance, |e, v| {
    e.st.double_distance = length(e, &v)?.unwrap_or(0.6 * PT);
    if e.st.double.is_none() {
        e.st.double = Some(crate::color::Color::WHITE);
    }
    Ok(())
});

// the distance between the two lines' centers: the gap is that less one line width
keyfn!(k_double_distance_centers, |e, v| {
    e.st.double_distance = (length(e, &v)?.unwrap_or(0.6 * PT) - e.st.line_width).max(0.0);
    if e.st.double.is_none() {
        e.st.double = Some(crate::color::Color::WHITE);
    }
    Ok(())
});
// `shift only`: the transformation keeps only where it moves the origin
keyfn!(k_shift_only, |e, _v| {
    let full = e.st.transform.compose(&e.st.pending);
    e.st.pending = e.st.transform.inverse().compose(&Transform::translation(full.e, full.f));
    Ok(())
});
keyfn!(k_allow_upside_down, |e, v| {
    e.st.allow_upside_down = text(e, &v)? != "false";
    Ok(())
});
keyfn!(k_nodes_in_empty_cells, |e, v| {
    e.st.matrix.nodes_in_empty_cells = text(e, &v)? != "false";
    Ok(())
});
keyfn!(k_matrix_anchor, |e, v| {
    e.st.matrix.anchor = Some(text(e, &v)?);
    Ok(())
});
keyfn!(k_shift, |e, v| {
    let p = shift_vector(e, &v)?;
    add_transform(e, Transform::translation(p.x, p.y));
    Ok(())
});

keyfn!(k_xshift, |e, v| {
    let d = length(e, &v)?.unwrap_or(0.0);
    add_transform(e, Transform::translation(d, 0.0));
    Ok(())
});

keyfn!(k_yshift, |e, v| {
    let d = length(e, &v)?.unwrap_or(0.0);
    add_transform(e, Transform::translation(0.0, d));
    Ok(())
});

keyfn!(k_scale, |e, v| {
    let s = number(e, &v)?.unwrap_or(1.0);
    add_transform(e, Transform::scaling(s, s));
    Ok(())
});

keyfn!(k_xscale, |e, v| {
    let s = number(e, &v)?.unwrap_or(1.0);
    add_transform(e, Transform::scaling(s, 1.0));
    Ok(())
});

keyfn!(k_yscale, |e, v| {
    let s = number(e, &v)?.unwrap_or(1.0);
    add_transform(e, Transform::scaling(1.0, s));
    Ok(())
});

keyfn!(k_rotate, |e, v| {
    let a = number(e, &v)?.unwrap_or(0.0);
    add_transform(e, Transform::rotation(a));
    Ok(())
});

keyfn!(k_xslant, |e, v| {
    let s = number(e, &v)?.unwrap_or(0.0);
    add_transform(e, Transform { c: s, ..Transform::default() });
    Ok(())
});

keyfn!(k_yslant, |e, v| {
    let s = number(e, &v)?.unwrap_or(0.0);
    add_transform(e, Transform { b: s, ..Transform::default() });
    Ok(())
});

keyfn!(k_rotate_around, |e, v| {
    // rotate around={angle:(point)}
    let s = text(e, &v)?;
    let Some((a, p)) = s.split_once(':') else { return Ok(()) };
    let angle = e.number(a)?;
    let tv = Some(tikra_tokens(p.trim()));
    let c = shift_vector(e, &tv)?;
    add_transform(e, Transform::translation(c.x, c.y).compose(&Transform::rotation(angle)).compose(&Transform::translation(-c.x, -c.y)));
    Ok(())
});

keyfn!(k_scale_around, |e, v| {
    // scale around={factor:(point)}
    let s = text(e, &v)?;
    let Some((f, p)) = s.split_once(':') else { return Ok(()) };
    let factor = e.number(f)?;
    let tv = Some(tikra_tokens(p.trim()));
    let c = shift_vector(e, &tv)?;
    add_transform(e, Transform::translation(c.x, c.y).compose(&Transform::scaling(factor, factor)).compose(&Transform::translation(-c.x, -c.y)));
    Ok(())
});

keyfn!(k_cm, |e, v| {
    // cm={a,b,c,d,(tx,ty)}
    let s = text(e, &v)?;
    let parts = super::coords::split_commas(&s);
    if parts.len() >= 5 {
        let n: Vec<f64> = parts[..4].iter().map(|x| e.number(x).unwrap_or(0.0)).collect();
        let tv = Some(tikra_tokens(parts[4..].join(",").trim()));
        let t = shift_vector(e, &tv).unwrap_or_default();
        add_transform(e, Transform { a: n[0], b: n[1], c: n[2], d: n[3], e: t.x, f: t.y });
    }
    Ok(())
});

keyfn!(k_reset_cm, |e, _v| {
    e.st.transform = Transform::default();
    e.st.pending = Transform::default();
    Ok(())
});

fn unit_vector(e: &mut Engine, v: &Option<Vec<Token>>, axis: usize) -> Result<Point, Error> {
    let s = text(e, v)?;
    let t = s.trim();
    if t.starts_with('(') || t.starts_with('{') {
        let inner = crate::keys::strip_braces(t).trim().trim_start_matches('(').trim_end_matches(')').to_string();
        // a point, in canvas units without the current transformation
        let saved = e.st.transform;
        e.st.transform = Transform::default();
        let r = e.coordinate(&inner);
        e.st.transform = saved;
        // a node's point as the transformation makes it (`x=(a.east)` in a shifted scope is the
        // vector to the anchor from the scope's origin)
        let name = inner.split_once('.').map_or(inner.as_str(), |(n, _)| n).trim();
        if e.lookup_node(name).is_some() {
            return Ok(saved.inverse().apply(r?.p));
        }
        return Ok(r?.p);
    }
    let m = e.length(t)?;
    Ok(match axis {
        0 => Point::new(m, 0.0),
        1 => Point::new(0.0, m),
        // `z=1cm` is (1cm,1cm), as TikZ takes a length for the z vector
        _ => Point::new(m, m),
    })
}

/// `rotate around x=θ` (and `y`, `z`): the other two unit vectors turned about that axis, in the
/// xyz system.
fn rotate_around(e: &mut Engine, v: &Option<Vec<Token>>, axis: usize) -> Result<(), Error> {
    let a = number(e, v)?.unwrap_or(0.0);
    let (s, c) = a.to_radians().sin_cos();
    let (x, y, z) = (e.st.x, e.st.y, e.st.z);
    match axis {
        0 => (e.st.y, e.st.z) = (y.scale(c).plus(z.scale(s)), z.scale(c).minus(y.scale(s))),
        1 => (e.st.x, e.st.z) = (x.scale(c).minus(z.scale(s)), z.scale(c).plus(x.scale(s))),
        _ => (e.st.x, e.st.y) = (x.scale(c).plus(y.scale(s)), y.scale(c).minus(x.scale(s))),
    }
    Ok(())
}
keyfn!(k_rotate_around_x, |e, v| rotate_around(e, &v, 0));
keyfn!(k_rotate_around_y, |e, v| rotate_around(e, &v, 1));
keyfn!(k_rotate_around_z, |e, v| rotate_around(e, &v, 2));

keyfn!(k_x, |e, v| {
    e.st.x = unit_vector(e, &v, 0)?;
    Ok(())
});
keyfn!(k_y, |e, v| {
    e.st.y = unit_vector(e, &v, 1)?;
    Ok(())
});
keyfn!(k_z, |e, v| {
    e.st.z = unit_vector(e, &v, 2)?;
    Ok(())
});

keyfn!(k_inner_sep, |e, v| {
    let s = text(e, &v)?;
    set_value(e, "/pgf/inner xsep", &s);
    set_value(e, "/pgf/inner ysep", &s);
    Ok(())
});

keyfn!(k_outer_sep, |e, v| {
    let s = text(e, &v)?;
    let s = if s == "auto" { "0.5\\pgflinewidth".to_string() } else { s };
    set_value(e, "/pgf/outer xsep", &s);
    set_value(e, "/pgf/outer ysep", &s);
    Ok(())
});

keyfn!(k_minimum_size, |e, v| {
    let s = text(e, &v)?;
    set_value(e, "/pgf/minimum width", &s);
    set_value(e, "/pgf/minimum height", &s);
    Ok(())
});

keyfn!(k_font, |e, v| {
    let s = text(e, &v)?;
    // `font=` replaces the font of the nodes (`\tikz@textfont`), and the fonts selected around
    // the picture stay: what came before the last `font=` is kept, the rest replaced
    const MARK: &str = "\\tikz@textfont ";
    let around = e.st.font.find(MARK).map_or(e.st.font.as_str(), |i| &e.st.font[..i]).to_string();
    e.st.font = format!("{around}{MARK}{s}");
    e.st.font_size = 10.0 * crate::label::font_size(&e.st.font);
    Ok(())
});

// `node font`: also the font of the node's dimensions (`minimum height=3em`)
keyfn!(k_node_font, |e, v| {
    k_font(e, v)?;
    e.st.ambient_size = e.st.font_size;
    Ok(())
});

keyfn!(k_anchor, |e, v| {
    let s = text(e, &v)?;
    set_value(e, "/tikz/anchor", &s);
    Ok(())
});

/// `above`, `below left=2mm`, `below=1cm of a`, `right=of a.north`: an anchor and an offset or a reference.
fn placement(e: &mut Engine, dir: &str, v: Option<Vec<Token>>) -> Result<(), Error> {
    let anchor = match dir {
        "above" => "south",
        "below" => "north",
        "left" => "east",
        "right" => "west",
        "above left" => "south east",
        "above right" => "south west",
        "below left" => "north east",
        "base left" => "base east",
        "base right" => "base west",
        "mid left" => "mid east",
        "mid right" => "mid west",
        _ => "north west",
    };
    set_value(e, "/tikz/anchor", anchor);
    let (sy, sx) = (
        if dir.contains("above") {
            1.0
        } else if dir.contains("below") {
            -1.0
        } else {
            0.0
        },
        if dir.contains("left") {
            -1.0
        } else if dir.contains("right") {
            1.0
        } else {
            0.0
        },
    );
    let Some(v) = v else { return Ok(()) };
    let s = e.tex.expand_to_string(v)?.trim().to_string();
    if s.is_empty() {
        return Ok(());
    }
    // positioning: `<vertical> and <horizontal> of <node>`; without `of`, an offset from the position
    let (dist, target) = match s.split_once("of ").filter(|_| s.contains("of ")) {
        Some((d, t)) => (d.trim().to_string(), Some(t.trim().to_string())),
        None => (s.clone(), None),
    };
    let (dv, dh) = node_distance(e)?;
    let (v_, h_) = if dist.is_empty() {
        (dv, dh)
    } else if let Some((a, b)) = dist.split_once(" and ") {
        (place_length(e, a, 1)?, place_length(e, b, 0)?)
    } else if target.is_none() && !e.st.positioning {
        // TikZ's own `left=30`: a length, a bare number in points (the positioning library reads it
        // in the vectors' units)
        let s = e.math_string(tikra_tokens(dist.trim()))?;
        let (v, unit) = crate::expr::eval_length(&s, &e.vars())?;
        let m = if unit { v } else { v * crate::PT };
        (m, m)
    } else {
        let m = place_length(e, &dist, 1)?;
        (m, if target.is_some() { m } else { place_length(e, &dist, 0)? })
    };
    // a single distance (given, or the node distance) along a diagonal is measured along it
    let single = if dist.is_empty() { !node_distance_text(e).contains(" and ") } else { !dist.contains(" and ") };
    let diagonal = sx != 0.0 && sy != 0.0 && single;
    let offset = if diagonal && target.is_some() {
        // a single distance along a diagonal is measured along it
        Point::new(sx * h_ * std::f64::consts::FRAC_1_SQRT_2, sy * v_ * std::f64::consts::FRAC_1_SQRT_2)
    } else {
        Point::new(sx * h_, sy * v_)
    };
    let Some(target) = target else {
        add_transform(e, Transform::translation(offset.x, offset.y));
        return Ok(());
    };
    // the distance is between borders; the reference is the target's matching anchor
    let target_anchor = match dir {
        "above" => "north",
        "below" => "south",
        "left" => "west",
        "right" => "east",
        "above left" => "north west",
        "above right" => "north east",
        "below left" => "south west",
        "base left" => "base west",
        "base right" => "base east",
        "mid left" => "mid west",
        "mid right" => "mid east",
        _ => "south east",
    };
    let (name, explicit) = match target.split_once('.') {
        Some((n, a)) if e.nodes.contains_key(n.trim()) => (n.trim().to_string(), Some(a.trim().to_string())),
        _ => (target.trim_start_matches('(').trim_end_matches(')').to_string(), None),
    };
    // `on grid`: from center to center
    let on_grid = e.tex.meaning_of("iftikra@ignore@size") == e.tex.meaning_of("iftrue");
    let base = match e.nodes.get(&name) {
        Some(n) if on_grid => n.center(),
        Some(n) => n.anchor(explicit.as_deref().unwrap_or(target_anchor)).unwrap_or(n.center()),
        None => e.coordinate(&name)?.p,
    };
    if on_grid && e.nodes.contains_key(&name) {
        set_value(e, "/tikz/anchor", "center");
    }
    e.st_node().at = Some(base.plus(offset));
    Ok(())
}

/// The active characters of `v` as the characters they are.
pub fn deactivate(v: Vec<Token>) -> Vec<Token> {
    v.into_iter().map(|t| if let Token::Char(c, tex::Cat::Active) = t { Token::other(c) } else { t }).collect()
}

/// A positioning distance: a length, or a number in units of the x or y vector.
fn place_length(e: &mut Engine, text: &str, axis: usize) -> Result<f64, Error> {
    let s = e.math_string(tikra_tokens(text.trim()))?;
    let (v, unit) = crate::expr::eval_length(&s, &e.vars())?;
    Ok(if unit {
        v
    } else if axis == 0 {
        v * e.st.x.norm()
    } else {
        v * e.st.y.norm()
    })
}

fn node_distance_text(e: &Engine) -> String {
    keys::value(&e.tex, "/tikz/node distance").map(|v| tex::tokens_to_string(&v)).unwrap_or_else(|| "1cm".into())
}

/// The deprecated `right of=a` and its kin: at the node's center, shifted by the node distance
/// (its first part) in the direction, anchored at the center (`\tikz@of`).
fn old_of(e: &mut Engine, v: Option<Vec<Token>>, angle: f64) -> Result<(), Error> {
    let name = text(e, &v)?;
    let Some(node) = e.lookup_node(name.trim()) else { return Err(Error::Figure(format!("{name}: no such node"))) };
    set_value(e, "/tikz/anchor", "center");
    e.st_node().at = Some(node.center());
    // the directions add up (`below of=a, right of=a` is diagonal); the distance is the node
    // distance the node ends up with, set before or after
    let (s, c) = angle.to_radians().sin_cos();
    let (v, h) = node_distance(e)?;
    add_transform(e, Transform::translation(h * c, v * s));
    // a `node distance` given after it still counts: the node redoes the shift (`\tikz@of`
    // reads the distance when the node is made)
    let pending = e.st.pending;
    e.st_node().old_of.push((Point::new(c, s), (v, h), pending));
    Ok(())
}
keyfn!(k_above_of, |e, v| old_of(e, v, 90.0));
keyfn!(k_below_of, |e, v| old_of(e, v, -90.0));
keyfn!(k_left_of, |e, v| old_of(e, v, 180.0));
keyfn!(k_right_of, |e, v| old_of(e, v, 0.0));
keyfn!(k_above_left_of, |e, v| old_of(e, v, 135.0));
keyfn!(k_below_left_of, |e, v| old_of(e, v, -135.0));
keyfn!(k_above_right_of, |e, v| old_of(e, v, 45.0));
keyfn!(k_below_right_of, |e, v| old_of(e, v, -45.0));
keyfn!(k_save_path, |e, v| {
    e.st.save_path = v.and_then(|v| v.into_iter().find(|t| matches!(t, Token::Cs(_))));
    Ok(())
});
keyfn!(k_use_path, |e, v| {
    e.st.use_saved_path = v.and_then(|v| v.into_iter().find(|t| matches!(t, Token::Cs(_))));
    Ok(())
});

pub(super) fn node_distance(e: &mut Engine) -> Result<(f64, f64), Error> {
    let s = node_distance_text(e);
    let read = |e: &mut Engine, t: &str| match place_length(e, t, 1) {
        Ok(v) => v,
        Err(err) => {
            e.warn(format!("node distance '{}': {err}; 1cm used", t.trim()));
            1.0
        }
    };
    Ok(match s.split_once(" and ") {
        Some((a, b)) => (read(e, a), read(e, b)),
        None => {
            let m = read(e, &s);
            (m, m)
        }
    })
}

keyfn!(k_above, |e, v| placement(e, "above", v));
keyfn!(k_below, |e, v| placement(e, "below", v));
keyfn!(k_left, |e, v| placement(e, "left", v));
keyfn!(k_right, |e, v| placement(e, "right", v));
keyfn!(k_above_left, |e, v| placement(e, "above left", v));
keyfn!(k_above_right, |e, v| placement(e, "above right", v));
keyfn!(k_below_left, |e, v| placement(e, "below left", v));
keyfn!(k_below_right, |e, v| placement(e, "below right", v));
keyfn!(k_base_left, |e, v| placement(e, "base left", v));
keyfn!(k_base_right, |e, v| placement(e, "base right", v));
keyfn!(k_mid_left, |e, v| placement(e, "mid left", v));
keyfn!(k_mid_right, |e, v| placement(e, "mid right", v));

keyfn!(k_pos, |e, v| {
    // `pos=` (empty): not on the path
    let v = v.filter(|v| !keys::trim(v).is_empty());
    e.st_node().pos = number(e, &v)?;
    Ok(())
});
keyfn!(k_midway, |e, _v| {
    e.st_node().pos = Some(0.5);
    Ok(())
});
keyfn!(k_near_start, |e, _v| {
    e.st_node().pos = Some(0.25);
    Ok(())
});
keyfn!(k_near_end, |e, _v| {
    e.st_node().pos = Some(0.75);
    Ok(())
});
keyfn!(k_very_near_start, |e, _v| {
    e.st_node().pos = Some(0.125);
    Ok(())
});
keyfn!(k_very_near_end, |e, _v| {
    e.st_node().pos = Some(0.875);
    Ok(())
});
keyfn!(k_at_start, |e, _v| {
    e.st_node().pos = Some(0.0);
    Ok(())
});
keyfn!(k_at_end, |e, _v| {
    e.st_node().pos = Some(1.0);
    Ok(())
});
keyfn!(k_sloped, |e, _v| {
    e.st_node().sloped = true;
    Ok(())
});
keyfn!(k_transform_shape, |e, _v| {
    e.st_node().transform_shape = true;
    Ok(())
});
keyfn!(k_transform_shape_nonlinear, |e, v| {
    e.st_node().transform_shape_nonlinear = text(e, &v)?.trim() != "false";
    Ok(())
});
keyfn!(k_name, |e, v| {
    let s = text(e, &v)?;
    e.st_node().name = Some(s);
    Ok(())
});
keyfn!(k_at, |e, v| {
    let s = text(e, &v)?;
    let inner = s.trim().trim_start_matches('(').trim_end_matches(')').to_string();
    let p = e.coordinate(&inner)?.p;
    e.st_node().at = Some(p);
    Ok(())
});
keyfn!(k_late_options, |e, v| {
    // `late options={name=a, label=above:world}`: the other options for the node `a`
    let mut name = String::new();
    let mut rest = Vec::new();
    for item in crate::keys::split_list(&v.unwrap_or_default()) {
        if item.key == "name" {
            name = tex::tokens_to_string(&item.value.unwrap_or_default()).trim().to_string();
            continue;
        }
        if !rest.is_empty() {
            rest.push(Token::other(','));
        }
        rest.extend(tikra_tokens(&item.key));
        if let Some(value) = item.value {
            rest.push(Token::other('='));
            rest.push(Token::Char('{', Cat::Begin));
            rest.extend(value);
            rest.push(Token::Char('}', Cat::End));
        }
    }
    let items = e.late_options(&name, rest)?;
    e.canvas.items.extend(items);
    Ok(())
});
keyfn!(k_label, |e, v| {
    if let Some(v) = v {
        e.st_node().labels.push((v, false));
    }
    Ok(())
});
keyfn!(k_pin, |e, v| {
    if let Some(v) = v {
        e.st_node().labels.push((v, true));
    }
    Ok(())
});

keyfn!(k_out, |e, v| {
    if let Some(x) = number(e, &v)? {
        e.st.to.out = x;
    }
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_in, |e, v| {
    if let Some(x) = number(e, &v)? {
        e.st.to.inn = x;
    }
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_relative, |e, v| {
    e.st.to.relative = text(e, &v)? != "false";
    Ok(())
});
keyfn!(k_bend_angle, |e, v| {
    if let Some(x) = number(e, &v)? {
        e.st.to.bend = x;
    }
    Ok(())
});
/// `bend left[=angle]` and `bend right`: `out` the angle off the line, `in` its mirror, relative.
fn bend(e: &mut Engine, v: Option<Vec<Token>>, sign: f64) -> Result<(), Error> {
    let v = v.filter(|v| !crate::keys::trim(v).is_empty());
    if let Some(x) = number(e, &v)? {
        e.st.to.bend = x;
    }
    e.st.to.out = sign * e.st.to.bend;
    e.st.to.inn = 180.0 - e.st.to.out;
    e.st.to.curve = true;
    e.st.to.relative = true;
    Ok(())
}
keyfn!(k_bend_left, |e, v| bend(e, v, 1.0));
keyfn!(k_bend_right, |e, v| bend(e, v, -1.0));
keyfn!(k_looseness, |e, v| {
    let x = number(e, &v)?.unwrap_or(1.0);
    e.st.to.out_looseness = x;
    e.st.to.in_looseness = x;
    e.st.to.out_control = None;
    e.st.to.in_control = None;
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_out_looseness, |e, v| {
    e.st.to.out_looseness = number(e, &v)?.unwrap_or(1.0);
    e.st.to.out_control = None;
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_in_looseness, |e, v| {
    e.st.to.in_looseness = number(e, &v)?.unwrap_or(1.0);
    e.st.to.in_control = None;
    e.st.to.curve = true;
    Ok(())
});
/// `\tikz@to@set@distances{in min}{in max}{out min}{out max}`: which of them a key sets.
fn distances(e: &mut Engine, v: Option<Vec<Token>>, which: [bool; 4]) -> Result<(), Error> {
    let Some(d) = length(e, &v)? else { return Ok(()) };
    let t = &mut e.st.to;
    for (k, set) in which.iter().enumerate() {
        if *set {
            *[&mut t.in_min, &mut t.in_max, &mut t.out_min, &mut t.out_max][k] = d;
        }
    }
    if which[0] || which[1] {
        t.in_control = None;
    }
    if which[2] || which[3] {
        t.out_control = None;
    }
    t.curve = true;
    Ok(())
}
keyfn!(k_in_min_distance, |e, v| distances(e, v, [true, false, false, false]));
keyfn!(k_in_max_distance, |e, v| distances(e, v, [false, true, false, false]));
keyfn!(k_in_distance, |e, v| distances(e, v, [true, true, false, false]));
keyfn!(k_out_min_distance, |e, v| distances(e, v, [false, false, true, false]));
keyfn!(k_out_max_distance, |e, v| distances(e, v, [false, false, false, true]));
keyfn!(k_out_distance, |e, v| distances(e, v, [false, false, true, true]));
keyfn!(k_min_distance, |e, v| distances(e, v, [true, false, true, false]));
keyfn!(k_max_distance, |e, v| distances(e, v, [false, true, false, true]));
keyfn!(k_distance, |e, v| distances(e, v, [true, true, true, true]));
keyfn!(k_in_control, |e, v| {
    e.st.to.in_control = v;
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_out_control, |e, v| {
    e.st.to.out_control = v;
    e.st.to.curve = true;
    Ok(())
});
// `controls=(a) and (b)`: the out control, then the in control.
keyfn!(k_controls, |e, v| {
    let v = v.unwrap_or_default();
    let mut split = None;
    for i in 0..v.len().saturating_sub(2) {
        if v[i].is_char('a') && v[i + 1].is_char('n') && v[i + 2].is_char('d') {
            split = Some(i);
        }
    }
    let Some(i) = split else {
        e.warn("controls without 'and': ignored");
        return Ok(());
    };
    e.st.to.out_control = Some(crate::keys::trim(&v[..i]).to_vec());
    e.st.to.in_control = Some(crate::keys::trim(&v[i + 3..]).to_vec());
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_ampersand_replacement, |e, v| {
    e.st.matrix.ampersand = v.unwrap_or_default().into_iter().find_map(|t| match t {
        Token::Cs(n) => Some(n.to_string()),
        _ => None,
    });
    Ok(())
});
keyfn!(k_at_begin_cell, |e, v| {
    e.st.matrix.at_begin_cell.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_at_end_cell, |e, v| {
    e.st.matrix.at_end_cell.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_at_empty_cell, |e, v| {
    e.st.matrix.at_empty_cell = v.unwrap_or_default();
    Ok(())
});
// `turn`: the coordinate system at the path's last point, its x axis along the path there
// `transform canvas={…}`: the transformations of the options applied to what the scope draws, as a
// whole (line widths and text too)
keyfn!(k_transform_canvas, |e, v| {
    let saved = std::mem::take(&mut e.st.pending);
    // (read with the transformations reset, as TikZ does: a node's point there is the canvas's)
    let transform = std::mem::take(&mut e.st.transform);
    let r = e.process_keys(v.unwrap_or_default(), Some("/tikz"));
    e.st.transform = transform;
    let t = std::mem::replace(&mut e.st.pending, saved);
    r?;
    // the transformations act on the canvas around the current origin, as pgf's low level does
    e.open_in_scope(crate::canvas::Item::Transform { transform: t });
    e.canvas_transforms += 1;
    Ok(())
});
// `view={(a) (b) at (c) (d)}` (animations library): what the scope draws in the box from a to b,
// shown in the box from c to d, as large as fits and centered, and clipped to it (the view as it is
// before any animation of it)
keyfn!(k_view, |e, v| {
    let s = text(e, &v)?;
    let Some((from, to)) = s.split_once(" at ") else { return Ok(()) };
    let mut corners = Vec::new();
    for part in [from, to] {
        for c in part.split(')').map(|c| c.trim().trim_start_matches('(').trim()).filter(|c| !c.is_empty()) {
            corners.push(e.coordinate(c)?.p);
        }
    }
    let [a, b, c, d] = corners[..] else { return Ok(()) };
    let (w, h) = ((b.x - a.x).abs().max(1e-9), (b.y - a.y).abs().max(1e-9));
    let k = ((d.x - c.x).abs() / w).min((d.y - c.y).abs() / h);
    let (from_mid, to_mid) = (a.lerp(b, 0.5), c.lerp(d, 0.5));
    let t = Transform::translation(to_mid.x, to_mid.y).compose(&Transform::scaling(k, k)).compose(&Transform::translation(-from_mid.x, -from_mid.y));
    let p = Point::new;
    use crate::scene::Segment;
    let clip = vec![Segment::MoveTo(c), Segment::LineTo(p(d.x, c.y)), Segment::LineTo(d), Segment::LineTo(p(c.x, d.y)), Segment::Close];
    e.open_in_scope(crate::canvas::Item::Clip { segments: clip, even_odd: false });
    if let Some(s) = e.scopes.last_mut() {
        s.clipped = true;
    }
    e.open_in_scope(crate::canvas::Item::Transform { transform: t });
    Ok(())
});
keyfn!(k_snapshot, |e, v| {
    let s = text(e, &v)?;
    e.set_snapshot(&s);
    Ok(())
});
// `make snapshot after`: a hair after the time, when animations that end then are over
keyfn!(k_snapshot_after, |e, v| {
    let s = text(e, &v)?;
    e.set_snapshot(&s);
    e.st.snapshot = e.st.snapshot.map(|t| t + 1e-6);
    Ok(())
});
keyfn!(k_trig_format, |e, v| {
    e.st.trig_rad = text(e, &v)?.trim() == "rad";
    Ok(())
});
keyfn!(k_current_point_local, |e, v| {
    e.st.current_point_local = text(e, &v)?.trim() != "false";
    Ok(())
});
keyfn!(k_animate, |e, v| e.animate_key(v.unwrap_or_default()));
// the math library's `evaluate={…}`: \tikzmath statements
keyfn!(k_evaluate, |e, v| e.math_statements(&v.unwrap_or_default()));
keyfn!(k_trim_left, |e, v| {
    e.st.trim_left = Some(text(e, &v)?);
    Ok(())
});
keyfn!(k_trim_right, |e, v| {
    e.st.trim_right = Some(text(e, &v)?);
    Ok(())
});
keyfn!(k_path_picture, |e, v| {
    e.st.mode.picture = v;
    Ok(())
});
keyfn!(k_turn, |e, _v| {
    let (last, from) = e.turn_from;
    let d = last.minus(from);
    let d = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(1.0, 0.0) };
    let t = Transform { a: d.x, b: d.y, c: -d.y, d: d.x, e: last.x, f: last.y };
    e.st.pending = e.st.pending.compose(&t);
    Ok(())
});
keyfn!(k_start_chain, |e, v| {
    let s = text(e, &v)?;
    e.start_chain(&s);
    Ok(())
});
keyfn!(k_continue_chain, |e, v| {
    let s = text(e, &v)?;
    e.continue_chain(&s);
    Ok(())
});
keyfn!(k_start_branch, |e, v| {
    let s = text(e, &v)?;
    e.start_branch(&s);
    Ok(())
});
keyfn!(k_continue_branch, |e, v| {
    let s = text(e, &v)?;
    e.continue_branch(&s);
    Ok(())
});
keyfn!(k_chain_default_direction, |e, v| {
    let s = text(e, &v)?;
    e.chain_default_direction(&s);
    Ok(())
});
keyfn!(k_on_chain, |e, v| {
    let s = text(e, &v)?;
    e.on_chain(&s)
});
keyfn!(k_join, |e, v| {
    let s = text(e, &v)?;
    // `join=round`: the line join; anything else is the chains' join
    if matches!(s.trim(), "round" | "bevel" | "miter") {
        return e.process_keys(tikra_tokens(&format!("line join={}", s.trim())), Some("/tikz"));
    }
    e.join(&s);
    Ok(())
});
keyfn!(k_after_node_path, |e, v| {
    e.st.after_node_path.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_alias, |e, v| {
    let s = text(e, &v)?;
    e.st.aliases.push(s);
    Ok(())
});
keyfn!(k_to_path, |e, v| {
    e.st.to.path = v;
    Ok(())
});
// `line to`, `move to` and `curve to`: the topaths library's own paths.
keyfn!(k_line_to, |e, _v| {
    e.st.to.path = None;
    e.st.to.curve = false;
    Ok(())
});
keyfn!(k_curve_to, |e, _v| {
    e.st.to.path = None;
    e.st.to.curve = true;
    Ok(())
});
keyfn!(k_loop, |e, _v| {
    e.st.to.path = None;
    e.st.to.is_loop = true;
    e.st.to.curve = true;
    e.process_keys(tikra_tokens("every loop/.try"), Some("/tikz"))
});

/// A grid step (cm): a length as it is, a number in the x or y unit.
fn grid_step(e: &mut Engine, v: &Option<Vec<Token>>, axis: usize) -> Result<f64, Error> {
    let s = text(e, v)?;
    let (d, unit) = crate::expr::eval_length(&e.math_string(tikra_tokens(&s))?, &e.vars())?;
    Ok(if unit { d } else { d * if axis == 0 { e.st.x.norm() } else { e.st.y.norm() } })
}

keyfn!(k_step, |e, v| {
    let s = text(e, &v)?;
    if s.starts_with('(') {
        let p = e.coordinate(s.trim_start_matches('(').trim_end_matches(')'))?.p;
        let inv = e.st.transform.inverse();
        let d = inv.apply(p);
        e.st.grid_step = (d.x.abs(), d.y.abs());
    } else {
        e.st.grid_step = (grid_step(e, &v, 0)?, grid_step(e, &v, 1)?);
    }
    Ok(())
});
keyfn!(k_xstep, |e, v| {
    e.st.grid_step.0 = grid_step(e, &v, 0)?;
    Ok(())
});
keyfn!(k_ystep, |e, v| {
    e.st.grid_step.1 = grid_step(e, &v, 1)?;
    Ok(())
});

keyfn!(k_domain, |e, v| {
    let s = text(e, &v)?;
    if let Some((a, b)) = s.split_once(':') {
        e.st.domain = (e.number(a)?, e.number(b)?);
    }
    // `domain` and `samples` replace a `samples at` given before them, as in TeX
    e.st.samples_at = None;
    Ok(())
});
keyfn!(k_samples, |e, v| {
    e.st.samples = number(e, &v)?.map_or(25, |s| s.max(2.0) as usize).min(2000);
    e.st.samples_at = None;
    Ok(())
});
// `samples at={…}`: a `\foreach` list of the values, `...` ranges included
keyfn!(k_samples_at, |e, v| {
    let list = e.tex.expand_fully(v.unwrap_or_default())?;
    let mut out = Vec::new();
    for item in super::foreach_items(&list) {
        let s = e.tex.expand_to_string(item)?;
        out.push(e.number(&s)?);
    }
    e.st.samples_at = Some(out);
    Ok(())
});
keyfn!(k_variable, |e, v| {
    let s = text(e, &v)?;
    e.st.variable = s.trim().trim_start_matches('\\').to_string();
    Ok(())
});
// the plot handlers named by their key (pgfplots' keys set the others)
fn plot_handler(e: &mut Engine, name: &str) -> Result<(), Error> {
    e.st.plot_handler = name.into();
    Ok(())
}
keyfn!(k_smooth_cycle, |e, _v| plot_handler(e, "smooth cycle"));
keyfn!(k_sharp_cycle, |e, _v| plot_handler(e, "sharp cycle"));
keyfn!(k_tension, |e, v| {
    e.st.tension = number(e, &v)?.unwrap_or(0.55);
    Ok(())
});
keyfn!(k_mark, |e, v| {
    let s = text(e, &v)?;
    e.st.plot_mark = if s == "none" || s.is_empty() { None } else { Some(s) };
    Ok(())
});
keyfn!(k_mark_size, |e, v| {
    if let Some(d) = length(e, &v)? {
        e.st.mark_size = d;
    }
    Ok(())
});
keyfn!(k_mark_repeat, |e, v| {
    e.st.mark_repeat = number(e, &v)?.unwrap_or(1.0).max(1.0) as usize;
    Ok(())
});
keyfn!(k_mark_phase, |e, v| {
    e.st.mark_phase = number(e, &v)?.unwrap_or(1.0).max(0.0) as usize;
    Ok(())
});
keyfn!(k_mark_indices, |e, v| {
    let list = e.tex.expand_fully(v.unwrap_or_default())?;
    let mut out = Vec::new();
    for item in super::foreach_items(&list) {
        if let Ok(n) = e.tex.expand_to_string(item)?.trim().parse::<f64>() {
            out.push(n as usize);
        }
    }
    e.st.mark_indices = if out.is_empty() { None } else { Some(out) };
    Ok(())
});
keyfn!(k_mark_color, |e, v| {
    let s = text(e, &v)?;
    e.st.mark_color = if s.is_empty() { None } else { Some(s) };
    Ok(())
});
keyfn!(k_only_marks, |e, _v| {
    e.st.only_marks = true;
    Ok(())
});

keyfn!(k_declare_function, |e, v| {
    let s = text(e, &v)?;
    for def in s.split(';') {
        let Some((head, body)) = def.split_once('=') else { continue };
        // a constant (`C=4;`) has no parameters
        let (name, params) = head.trim().split_once('(').unwrap_or((head.trim(), ""));
        let params = params.trim_end_matches(')').split(',').map(|p| p.trim().trim_start_matches('\\').to_string()).filter(|p| !p.is_empty()).collect();
        Rc::make_mut(&mut e.st.functions).insert(name.trim().to_string(), (params, body.trim().to_string()));
    }
    Ok(())
});

keyfn!(k_tikz_unknown, |e, v| e.tikz_unknown(v));

keyfn!(k_ignore, |_e, _v| Ok(()));
keyfn!(k_overlay, |e, v| {
    e.st.overlay = v.is_none_or(|v| tex::tokens_to_string(&v).trim() != "false");
    e.canvas.overlay = e.st.overlay;
    Ok(())
});

// `fit=(a)(b)(1,2)`: the node covers the points (a node by its west, east, north and south).
keyfn!(k_fit, |e, v| {
    let s = text(e, &v)?;
    let (mut lo, mut hi) = (Point::new(f64::INFINITY, f64::INFINITY), Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
    let mut grow = |p: Point| {
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    };
    // with `rotate fit`, the box is the points' in a coordinate system turned that much
    let turn = Transform::rotation(e.st.node.fit_rotate);
    let back = turn.inverse();
    let mut rest = s.as_str();
    while let Some(i) = rest.find('(') {
        let Some(j) = rest[i..].find(')') else { break };
        let inner = rest[i + 1..i + j].trim().to_string();
        rest = &rest[i + j + 1..];
        match e.nodes.get(&inner) {
            Some(n) => {
                for a in ["west", "east", "north", "south"] {
                    if let Some(p) = n.anchor(a) {
                        grow(back.apply(p));
                    }
                }
            }
            None => grow(back.apply(e.coordinate(&inner)?.p)),
        }
    }
    if lo.x > hi.x {
        return Ok(());
    }
    let center = turn.apply(lo.lerp(hi, 0.5));
    e.st_node().at = Some(center);
    e.st_node().fit = Some((hi.x - lo.x, hi.y - lo.y));
    set_value(e, "/tikz/anchor", "center");
    set_value(e, "/tikz/align", "center");
    e.process_keys(tikra_tokens("every fit/.try"), None)?;
    Ok(())
});

// `rotate fit=30`: the fitted box is turned, and the node with it
keyfn!(k_rotate_fit, |e, v| {
    let a = number(e, &v)?.unwrap_or(0.0);
    e.st_node().fit_rotate = a;
    e.process_keys(tikra_tokens(&format!("rotate={a}")), Some("/tikz"))
});
keyfn!(k_name_path, |e, v| {
    let s = text(e, &v)?;
    e.st.name_path = Some(s);
    Ok(())
});

keyfn!(k_name_intersections, |e, v| super::intersections::name_intersections(e, v));

keyfn!(k_at_begin_scope, |e, v| {
    e.st.at_begin_scope.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_at_end_scope, |e, v| {
    e.st.at_end_scope.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_at_begin_picture, |e, v| {
    e.st.at_begin_picture.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_at_end_picture, |e, v| {
    e.st.at_end_picture.extend(v.unwrap_or_default());
    Ok(())
});

keyfn!(k_pic_actions, |e, _v| {
    let p = e.st.pic_mode.clone();
    let m = &mut e.st.mode;
    m.draw |= p.draw;
    m.fill |= p.fill;
    m.pattern |= p.pattern;
    m.shade |= p.shade;
    m.clip |= p.clip;
    Ok(())
});

keyfn!(k_level_distance, |e, v| {
    e.st.level_distance = length(e, &v)?.unwrap_or(1.5);
    super::trees::set_distance_registers(e);
    Ok(())
});
keyfn!(k_sibling_distance, |e, v| {
    e.st.sibling_distance = length(e, &v)?.unwrap_or(1.5);
    super::trees::set_distance_registers(e);
    Ok(())
});

/// `grow=down`, `grow=30`, `grow'=…`: the direction of the tree; set in a child, it is that child's.
fn set_grow(e: &mut Engine, v: &Option<Vec<Token>>, swapped: bool) -> Result<(), Error> {
    let s = text(e, v)?;
    e.st.grow_angle = match s.as_str() {
        "down" | "south" => -90.0,
        "up" | "north" => 90.0,
        "left" | "west" => 180.0,
        "right" | "east" => 0.0,
        "north east" => 45.0,
        "north west" => 135.0,
        "south east" => -45.0,
        "south west" => -135.0,
        other => e.number(other)?,
    };
    e.st.grow_swapped = swapped;
    e.st.growth_function = None;
    e.st.special_level = Some(e.st.tree_level);
    // the growth function is the direction's again (not `grow cyclic`'s)
    e.tex.define_string("tikra@growth", "", false);
    Ok(())
}
keyfn!(k_grow, |e, v| set_grow(e, &v, false));
keyfn!(k_grow_swapped, |e, v| set_grow(e, &v, true));
keyfn!(k_missing, |e, v| {
    e.st.child_missing = text(e, &v)? != "false";
    Ok(())
});
keyfn!(k_growth_parent_anchor, |e, v| {
    e.st.growth_anchor = text(e, &v)?;
    Ok(())
});
keyfn!(k_parent_anchor, |e, v| {
    let s = text(e, &v)?;
    e.st.parent_anchor = if s == "border" || s.is_empty() { String::new() } else { format!(".{s}") };
    Ok(())
});
keyfn!(k_child_anchor, |e, v| {
    let s = text(e, &v)?;
    e.st.child_anchor = if s == "border" || s.is_empty() { String::new() } else { format!(".{s}") };
    Ok(())
});
keyfn!(k_edge_from_parent_path, |e, v| {
    e.st.edge_from_parent_path = v;
    Ok(())
});

keyfn!(k_column_sep, |e, v| {
    e.st.matrix.column_sep = e.tex.expand_fully(v.unwrap_or_default())?;
    Ok(())
});
keyfn!(k_row_sep, |e, v| {
    e.st.matrix.row_sep = e.tex.expand_fully(v.unwrap_or_default())?;
    Ok(())
});
keyfn!(k_matrix_of_nodes, |e, _v| {
    e.st.matrix.of_nodes = true;
    Ok(())
});
keyfn!(k_matrix_of_math_nodes, |e, _v| {
    e.st.matrix.of_nodes = true;
    e.st.matrix.math = true;
    Ok(())
});
// `nodes={…}`: more of every node (a matrix's own node took every node before its options)
keyfn!(k_matrix_nodes, |e, v| {
    let Some(v) = v else { return Ok(()) };
    let mut keys = tikra_tokens("every node/.append style=");
    keys.push(Token::Char('{', crate::tex::Cat::Begin));
    keys.extend(v);
    keys.push(Token::Char('}', crate::tex::Cat::End));
    e.process_keys(keys, Some("/tikz"))
});

fn shade_color(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<crate::color::Color, Error> {
    let s = text(e, v)?;
    e.color(&s).ok_or_else(|| Error::Figure(format!("unknown color '{s}'")))
}
fn mix(a: crate::color::Color, b: crate::color::Color) -> crate::color::Color {
    crate::color::Color::rgb((a.r + b.r) / 2.0, (a.g + b.g) / 2.0, (a.b + b.b) / 2.0)
}
keyfn!(k_shading, |e, v| {
    e.st.shading = text(e, &v)?;
    e.st.mode.shade = true;
    Ok(())
});
/// The shadings library's `upper left=red`…: the corner's color of the bilinear interpolation, which
/// becomes the shading.
fn corner_color(e: &mut Engine, corner: &str, v: Option<Vec<Token>>) -> Result<(), Error> {
    let s = text(e, &v)?;
    match e.color(&s) {
        Some(c) => {
            e.colors.insert(corner.to_string(), c);
        }
        None => e.warn(format!("{corner}: unknown color '{s}'")),
    }
    e.st.shading = "bilinear interpolation".into();
    e.st.mode.shade = true;
    Ok(())
}
keyfn!(k_upper_left, |e, v| corner_color(e, "upper left", v));
keyfn!(k_upper_right, |e, v| corner_color(e, "upper right", v));
keyfn!(k_lower_left, |e, v| corner_color(e, "lower left", v));
keyfn!(k_lower_right, |e, v| corner_color(e, "lower right", v));
keyfn!(k_shading_angle, |e, v| {
    e.st.shade_angle = number(e, &v)?.unwrap_or(0.0);
    e.st.mode.shade = true;
    Ok(())
});
/// `top color`, `bottom color` (angle 0), `left color`, `right color` (angle 90): the middle becomes their mix.
fn axis_color(e: &mut Engine, v: &Option<Vec<Token>>, top: bool, angle: f64) -> Result<(), Error> {
    let c = shade_color(e, v)?;
    if top {
        e.st.axis_top = c;
    } else {
        e.st.axis_bottom = c;
    }
    e.st.axis_middle = mix(e.st.axis_top, e.st.axis_bottom);
    e.st.shading = "axis".into();
    e.st.shade_angle = angle;
    e.st.mode.shade = true;
    Ok(())
}
keyfn!(k_top_color, |e, v| axis_color(e, &v, true, 0.0));
keyfn!(k_bottom_color, |e, v| axis_color(e, &v, false, 0.0));
keyfn!(k_left_color, |e, v| axis_color(e, &v, true, 90.0));
keyfn!(k_right_color, |e, v| axis_color(e, &v, false, 90.0));
keyfn!(k_middle_color, |e, v| {
    e.st.axis_middle = shade_color(e, &v)?;
    e.st.shading = "axis".into();
    e.st.mode.shade = true;
    Ok(())
});
keyfn!(k_ball_color, |e, v| {
    e.st.ball = shade_color(e, &v)?;
    e.st.shading = "ball".into();
    e.st.mode.shade = true;
    Ok(())
});
keyfn!(k_inner_color, |e, v| {
    e.st.radial_inner = shade_color(e, &v)?;
    e.st.shading = "radial".into();
    e.st.mode.shade = true;
    Ok(())
});
keyfn!(k_outer_color, |e, v| {
    e.st.radial_outer = shade_color(e, &v)?;
    e.st.shading = "radial".into();
    e.st.mode.shade = true;
    Ok(())
});

// `path fading=name` names the fading and fades this path (not the paths of a scope it is given
// to); `path fading` alone fades it with the fading named last
keyfn!(k_path_fading, |e, v| {
    let s = text(e, &v)?;
    if s == "none" {
        e.st.mode.fade = false;
    } else {
        if !s.is_empty() {
            e.st.path_fading = Some(s);
        }
        e.st.mode.fade = true;
    }
    Ok(())
});
keyfn!(k_fading_angle, |e, v| {
    let s = text(e, &v)?;
    e.st.fading_transform = Transform::rotation(e.number(&s)?);
    note_fading_settings(e);
    Ok(())
});
// `fading transform={rotate=90}`: the transformations of the options, for the path fading
keyfn!(k_fading_transform, |e, v| {
    let saved = std::mem::take(&mut e.st.pending);
    let r = e.process_keys(v.unwrap_or_default(), Some("/tikz"));
    e.st.fading_transform = std::mem::replace(&mut e.st.pending, saved);
    note_fading_settings(e);
    r
});

/// The fading settings just set, for the `scope fading`s of the path being read.
fn note_fading_settings(e: &mut Engine) {
    if let Some(s) = e.path_fading_settings.last_mut() {
        *s = (e.st.fading_transform, e.st.fit_fading);
    }
}
keyfn!(k_local_bounding_box, |e, v| {
    let s = text(e, &v)?;
    e.open_local_box(s.trim().to_string());
    Ok(())
});
keyfn!(k_meet, |e, v| {
    let s = text(e, &v)?;
    e.view_key(&s, false)
});
keyfn!(k_slice, |e, v| {
    let s = text(e, &v)?;
    e.view_key(&s, true)
});
keyfn!(k_active_in_code, |e, _v| {
    e.st.active_in_code = true;
    Ok(())
});
keyfn!(k_active_in_nodes, |e, _v| {
    e.st.active_in_nodes = true;
    Ok(())
});
keyfn!(k_fit_fading, |e, v| {
    e.st.fit_fading = text(e, &v)? != "false";
    note_fading_settings(e);
    Ok(())
});
// `scope fading=name`: what the scope draws is faded, the fading at the scope's origin
keyfn!(k_scope_fading, |e, v| {
    let s = text(e, &v)?;
    // in a path's options: what the path draws, the fading fitted to it when it is done
    let t = e.st.transform.compose(&e.st.pending);
    let (start, fit) = (e.canvas.items.len(), e.st.fit_fading);
    if let Some(list) = e.path_fadings.last_mut() {
        list.push((s, start, fit, t, None));
        return Ok(());
    }
    e.open_fading(&s, t);
    Ok(())
});
keyfn!(k_transparency_group, |e, _v| {
    e.st.transparency_group = true;
    Ok(())
});
// `blend mode=screen`: the paths drawn from here on blend with what is under them
keyfn!(k_blend_mode, |e, v| {
    let s = text(e, &v)?;
    e.set_blend_mode(&s);
    Ok(())
});
// `blend group=screen`: a transparency group whose paths blend so
keyfn!(k_blend_group, |e, v| {
    let s = text(e, &v)?;
    e.st.transparency_group = true;
    e.set_blend_mode(&s);
    Ok(())
});

keyfn!(k_auto, |e, v| {
    let s = text(e, &v)?;
    e.st.auto = match s.as_str() {
        "false" => None,
        "right" => Some('r'),
        _ => Some('l'),
    };
    Ok(())
});

keyfn!(k_swap, |e, _v| {
    e.st.swap = !e.st.swap;
    Ok(())
});

keyfn!(k_baseline, |e, v| {
    e.st.baseline = Some(text(e, &v)?);
    Ok(())
});

// `alt={…}`, as LaTeX's tagging gives a picture its alternative text: the author's words, told
// instead of the description tikra writes
keyfn!(k_alt, |e, v| {
    let t = text(e, &v)?;
    if e.facts.author_alt.trim().is_empty() {
        e.facts.author_alt = t;
    }
    Ok(())
});

/// Keys done natively: the path and its function.
const NATIVE: &[(&str, KeyFn)] = &[
    ("/tikz/.unknown", k_tikz_unknown),
    ("/tikz/draw", k_draw),
    ("/tikz/fill", k_fill),
    ("/tikz/color", k_color),
    ("/tikz/text", k_text),
    ("/tikz/clip", k_clip),
    ("/tikz/use as bounding box", k_bounding_box),
    ("/tikz/shade", k_shade),
    ("/tikz/pattern", k_pattern),
    ("/tikz/pattern color", k_pattern_color),
    ("/tikz/line width", k_line_width),
    ("/tikz/dash pattern", k_dash_pattern),
    ("/tikz/dash phase", k_dash_phase),
    ("/tikz/dash", k_dash),
    ("/tikz/solid", k_solid),
    ("/tikz/line cap", k_line_cap),
    ("/tikz/line join", k_line_join),
    ("/tikz/miter limit", k_miter_limit),
    ("/tikz/opacity", k_opacity),
    ("/tikz/draw opacity", k_draw_opacity),
    ("/tikz/fill opacity", k_fill_opacity),
    ("/tikz/text opacity", k_text_opacity),
    ("/tikz/even odd rule", k_even_odd),
    ("/tikz/nonzero rule", k_nonzero),
    ("/tikz/rounded corners", k_rounded),
    ("/tikz/sharp corners", k_sharp),
    ("/tikz/arrows", k_arrows),
    ("/tikz/>", k_default_arrow),
    ("/pgf/tips", k_tips),
    ("/tikz/shorten >", k_shorten_end),
    ("/tikz/shorten <", k_shorten_start),
    ("/tikz/double", k_double),
    ("/tikz/double distance", k_double_distance),
    ("/tikz/double distance between line centers", k_double_distance_centers),
    ("/tikz/shift only", k_shift_only),
    ("/tikz/allow upside down", k_allow_upside_down),
    ("/tikz/nodes in empty cells", k_nodes_in_empty_cells),
    ("/tikz/matrix anchor", k_matrix_anchor),
    ("/tikz/shift", k_shift),
    ("/tikz/xshift", k_xshift),
    ("/tikz/yshift", k_yshift),
    ("/tikz/scale", k_scale),
    ("/tikz/xscale", k_xscale),
    ("/tikz/yscale", k_yscale),
    ("/tikz/rotate", k_rotate),
    ("/tikz/xslant", k_xslant),
    ("/tikz/yslant", k_yslant),
    ("/tikz/rotate around", k_rotate_around),
    ("/tikz/scale around", k_scale_around),
    ("/tikz/cm", k_cm),
    ("/tikz/reset cm", k_reset_cm),
    ("/tikz/x", k_x),
    ("/tikz/y", k_y),
    ("/tikz/z", k_z),
    ("/tikz/rotate around x", k_rotate_around_x),
    ("/tikz/rotate around y", k_rotate_around_y),
    ("/tikz/rotate around z", k_rotate_around_z),
    ("/pgf/inner sep", k_inner_sep),
    ("/pgf/outer sep", k_outer_sep),
    ("/pgf/minimum size", k_minimum_size),
    ("/tikz/font", k_font),
    ("/tikz/node font", k_node_font),
    ("/tikz/anchor", k_anchor),
    ("/tikz/above", k_above),
    ("/tikz/below", k_below),
    ("/tikz/left", k_left),
    ("/tikz/right", k_right),
    ("/tikz/above left", k_above_left),
    ("/tikz/base left", k_base_left),
    ("/tikz/base right", k_base_right),
    ("/tikz/mid left", k_mid_left),
    ("/tikz/mid right", k_mid_right),
    ("/tikz/above right", k_above_right),
    ("/tikz/below left", k_below_left),
    ("/tikz/below right", k_below_right),
    ("/tikz/pos", k_pos),
    ("/tikz/midway", k_midway),
    ("/tikz/near start", k_near_start),
    ("/tikz/near end", k_near_end),
    ("/tikz/very near start", k_very_near_start),
    ("/tikz/very near end", k_very_near_end),
    ("/tikz/at start", k_at_start),
    ("/tikz/at end", k_at_end),
    ("/tikz/sloped", k_sloped),
    ("/tikz/transform shape", k_transform_shape),
    ("/tikz/transform shape nonlinear", k_transform_shape_nonlinear),
    ("/tikz/name", k_name),
    ("/tikz/at", k_at),
    ("/tikz/label", k_label),
    ("/tikz/pin", k_pin),
    ("/tikz/out", k_out),
    ("/tikz/in", k_in),
    ("/tikz/bend left", k_bend_left),
    ("/tikz/bend right", k_bend_right),
    ("/tikz/looseness", k_looseness),
    ("/tikz/out looseness", k_out_looseness),
    ("/tikz/in looseness", k_in_looseness),
    ("/tikz/bend angle", k_bend_angle),
    ("/tikz/relative", k_relative),
    ("/tikz/in min distance", k_in_min_distance),
    ("/tikz/in max distance", k_in_max_distance),
    ("/tikz/in distance", k_in_distance),
    ("/tikz/out min distance", k_out_min_distance),
    ("/tikz/out max distance", k_out_max_distance),
    ("/tikz/out distance", k_out_distance),
    ("/tikz/min distance", k_min_distance),
    ("/tikz/max distance", k_max_distance),
    ("/tikz/distance", k_distance),
    ("/tikz/in control", k_in_control),
    ("/tikz/out control", k_out_control),
    ("/tikz/controls", k_controls),
    ("/tikz/to path", k_to_path),
    ("/tikz/start chain", k_start_chain),
    ("/tikz/turn", k_turn),
    ("/tikz/path picture", k_path_picture),
    ("/tikz/above of", k_above_of),
    ("/tikz/below of", k_below_of),
    ("/tikz/left of", k_left_of),
    ("/tikz/right of", k_right_of),
    ("/tikz/above left of", k_above_left_of),
    ("/tikz/below left of", k_below_left_of),
    ("/tikz/above right of", k_above_right_of),
    ("/tikz/below right of", k_below_right_of),
    ("/tikz/save path", k_save_path),
    ("/tikz/use path", k_use_path),
    ("/tikz/trim left", k_trim_left),
    ("/tikz/evaluate", k_evaluate),
    ("/tikz/make snapshot of", k_snapshot),
    ("/tikz/make snapshot after", k_snapshot_after),
    ("/tikz/make snapshot if necessary", k_ignore),
    ("/tikz/current point is local", k_current_point_local),
    ("/tikz/trig format", k_trig_format),
    ("/pgf/trig format", k_trig_format),
    ("/tikz/animate", k_animate),
    ("/tikz/transform canvas", k_transform_canvas),
    ("/tikz/view", k_view),
    ("/tikz/trim right", k_trim_right),
    ("/tikz/remember picture", k_ignore),
    ("/tikz/continue chain", k_continue_chain),
    ("/tikz/start branch", k_start_branch),
    ("/tikz/continue branch", k_continue_branch),
    ("/tikz/chain default direction", k_chain_default_direction),
    ("/tikz/on chain", k_on_chain),
    ("/tikz/join", k_join),
    ("/tikz/after node path", k_after_node_path),
    ("/tikz/alias", k_alias),
    ("/tikz/execute at begin cell", k_at_begin_cell),
    ("/tikz/ampersand replacement", k_ampersand_replacement),
    ("/tikz/execute at end cell", k_at_end_cell),
    ("/tikz/execute at empty cell", k_at_empty_cell),
    ("/tikz/line to", k_line_to),
    ("/tikz/curve to", k_curve_to),
    ("/tikz/loop", k_loop),
    ("/tikz/step", k_step),
    ("/tikz/xstep", k_xstep),
    ("/tikz/ystep", k_ystep),
    ("/tikz/domain", k_domain),
    ("/tikz/samples", k_samples),
    ("/tikz/samples at", k_samples_at),
    ("/tikz/variable", k_variable),
    ("/tikz/smooth cycle", k_smooth_cycle),
    ("/tikz/sharp cycle", k_sharp_cycle),
    ("/tikz/tension", k_tension),
    ("/tikz/mark", k_mark),
    ("/tikz/only marks", k_only_marks),
    ("/tikz/mark size", k_mark_size),
    ("/tikz/mark repeat", k_mark_repeat),
    ("/tikz/mark phase", k_mark_phase),
    ("/tikz/mark indices", k_mark_indices),
    ("/pgf/mark color", k_mark_color),
    ("/pgf/declare function", k_declare_function),
    ("/tikz/declare function", k_declare_function),
    ("/tikz/overlay", k_overlay),
    ("/tikz/inner frame sep", k_ignore),
    ("/tikz/baseline", k_baseline),
    ("/tikz/alt", k_alt),
    ("/tikz/auto", k_auto),
    ("/tikz/path fading", k_path_fading),
    ("/tikz/fit fading", k_fit_fading),
    ("/tikz/handle active characters in code", k_active_in_code),
    ("/tikz/handle active characters in nodes", k_active_in_nodes),
    ("/tikz/meet", k_meet),
    ("/tikz/slice", k_slice),
    ("/tikz/local bounding box", k_local_bounding_box),
    ("/tikz/fading angle", k_fading_angle),
    ("/tikz/fading transform", k_fading_transform),
    ("/tikz/scope fading", k_scope_fading),
    ("/tikz/transparency group", k_transparency_group),
    ("/tikz/blend mode", k_blend_mode),
    ("/tikz/upper left", k_upper_left),
    ("/tikz/upper right", k_upper_right),
    ("/tikz/lower left", k_lower_left),
    ("/tikz/lower right", k_lower_right),
    ("/tikz/blend group", k_blend_group),
    ("/tikz/shading", k_shading),
    ("/tikz/shading angle", k_shading_angle),
    ("/tikz/top color", k_top_color),
    ("/tikz/bottom color", k_bottom_color),
    ("/tikz/left color", k_left_color),
    ("/tikz/right color", k_right_color),
    ("/tikz/middle color", k_middle_color),
    ("/tikz/ball color", k_ball_color),
    ("/tikz/inner color", k_inner_color),
    ("/tikz/outer color", k_outer_color),
    ("/tikz/column sep", k_column_sep),
    ("/tikz/row sep", k_row_sep),
    ("/tikz/matrix of nodes", k_matrix_of_nodes),
    ("/tikz/matrix of math nodes", k_matrix_of_math_nodes),
    ("/tikz/nodes", k_matrix_nodes),
    ("/tikz/matrix", k_ignore),
    ("/tikz/level distance", k_level_distance),
    ("/tikz/sibling distance", k_sibling_distance),
    ("/tikz/grow", k_grow),
    ("/tikz/grow'", k_grow_swapped),
    ("/tikz/missing", k_missing),
    ("/tikz/growth parent anchor", k_growth_parent_anchor),
    ("/tikz/parent anchor", k_parent_anchor),
    ("/tikz/child anchor", k_child_anchor),
    ("/tikz/edge from parent path", k_edge_from_parent_path),
    ("/tikz/pic actions", k_pic_actions),
    ("/tikz/execute at begin scope", k_at_begin_scope),
    ("/tikz/execute at end scope", k_at_end_scope),
    ("/tikz/execute at begin picture", k_at_begin_picture),
    ("/tikz/execute at end picture", k_at_end_picture),
    ("/tikz/fit", k_fit),
    ("/tikz/rotate fit", k_rotate_fit),
    ("/tikz/name path", k_name_path),
    ("/tikz/late options", k_late_options),
    ("/tikz/name path global", k_name_path),
    ("/tikz/name intersections", k_name_intersections),
    ("/tikz/swap", k_swap),
    ("/tikz/'", k_swap),
];

/// TikZ's own definitions of the keys it defines as styles or values (tikz.code.tex, pgfmoduleshapes).
const DEFINITIONS: &str = r"
\tikzset{
  every picture/.style={}, every scope/.style={}, every path/.style={}, every node/.style={},
  every label/.style={draw=none,fill=none}, every pin/.style={draw=none,fill=none}, every pin edge/.style={help lines}, pin edge/.code={\gdef\tikra@pinedge{#1}},
  every edge/.style={draw}, every to/.style={}, centered/.style={anchor=center}, id/.initial=, prefix/.initial=, parametric/.code={}, raw gnuplot/.code={}, every plot/.style={}, double equal sign distance/.style={double distance between line centers=0.45ex}, /pgf/bar width/.initial=10pt, /handlers/first char syntax/.initial=false, /pgf/bar shift/.initial=0pt, every mark/.style={}, mark options/.style={every mark/.style={#1}},
  /pgf/text mark/.initial=p, /pgf/text mark style/.initial=, /pgf/text mark as node/.initial=false,
  text mark/.forward to=/pgf/text mark, text mark style/.forward to=/pgf/text mark style, text mark as node/.forward to=/pgf/text mark as node,
  no marks/.style={mark=none}, no markers/.style={mark=none}, every curve to/.style={},
  every loop/.style={->,shorten >=1pt}, loop right/.style={right,out=15,in=-15,loop},
  loop above/.style={above,out=105,in=75,loop}, loop left/.style={left,out=195,in=165,loop},
  loop below/.style={below,out=285,in=255,loop}, move to/.style={to path={(\tikztotarget) \tikztonodes}}, every circle node/.style={}, every rectangle node/.style={},
  every coordinate node/.style={}, every ellipse node/.style={}, every diamond node/.style={},
  ultra thin/.style={line width=0.1pt}, very thin/.style={line width=0.2pt}, thin/.style={line width=0.4pt},
  semithick/.style={line width=0.6pt}, thick/.style={line width=0.8pt}, very thick/.style={line width=1.2pt},
  ultra thick/.style={line width=1.6pt},
  help lines/.style={color=gray,line width=0.2pt},
  dotted/.style={dash pattern=on \pgflinewidth off 2pt},
  densely dotted/.style={dash pattern=on \pgflinewidth off 1pt},
  loosely dotted/.style={dash pattern=on \pgflinewidth off 4pt},
  dashed/.style={dash pattern=on 3pt off 3pt},
  densely dashed/.style={dash pattern=on 3pt off 2pt},
  loosely dashed/.style={dash pattern=on 3pt off 6pt},
  dash dot/.style={dash pattern=on 3pt off 2pt on \pgflinewidth off 2pt},
  densely dash dot/.style={dash pattern=on 3pt off 1pt on \pgflinewidth off 1pt},
  loosely dash dot/.style={dash pattern=on 3pt off 4pt on \pgflinewidth off 4pt},
  dash dot dot/.style={dash pattern=on 3pt off 2pt on \pgflinewidth off 2pt on \pgflinewidth off 2pt},
  densely dash dot dot/.style={dash pattern=on 3pt off 1pt on \pgflinewidth off 1pt on \pgflinewidth off 1pt},
  loosely dash dot dot/.style={dash pattern=on 3pt off 4pt on \pgflinewidth off 4pt on \pgflinewidth off 4pt},
  transparent/.style={opacity=0}, ultra nearly transparent/.style={opacity=0.05},
  very nearly transparent/.style={opacity=0.1}, nearly transparent/.style={opacity=0.25},
  semitransparent/.style={opacity=0.5}, nearly opaque/.style={opacity=0.75},
  very nearly opaque/.style={opacity=0.9}, ultra nearly opaque/.style={opacity=0.95}, opaque/.style={opacity=1},
  rounded corners/.default=4pt, double/.default=white, cap/.style={line cap=#1},
  draw opacity/.default=1, help lines/.append style={},
  text centered/.style={align=center}, text ragged/.style={align=left}, text badly ragged/.style={align=flush left},
  text justified/.style={align=justify}, text ragged left/.style={align=right}, text badly ragged left/.style={align=flush right}, text badly centered/.style={align=flush center},
  shape/.initial=rectangle, anchor/.initial=center, node distance/.initial=1cm and 1cm,
  align/.initial=, text width/.initial=, text height/.initial=, text depth/.initial=,
  label distance/.initial=0pt, pin distance/.initial=3ex, label position/.initial=above,
  /pgf/inner xsep/.initial=.3333em, /pgf/inner ysep/.initial=.3333em,
  /pgf/outer xsep/.initial=.5\pgflinewidth, /pgf/outer ysep/.initial=.5\pgflinewidth,
  /pgf/minimum width/.initial=1pt, /pgf/minimum height/.initial=1pt,
  /pgf/aspect/.initial=1, /pgf/shape aspect/.forward to=/pgf/aspect,
  shape aspect/.forward to=/pgf/aspect,
  inner xsep/.forward to=/pgf/inner xsep, inner ysep/.forward to=/pgf/inner ysep,
  inner sep/.forward to=/pgf/inner sep, outer sep/.forward to=/pgf/outer sep,
  outer xsep/.forward to=/pgf/outer xsep, outer ysep/.forward to=/pgf/outer ysep,
  minimum width/.forward to=/pgf/minimum width, minimum height/.forward to=/pgf/minimum height,
  minimum size/.forward to=/pgf/minimum size, aspect/.forward to=/pgf/aspect,
  coordinate/.style={shape=coordinate},
  circle/.style={shape=circle}, rectangle/.style={shape=rectangle}, ellipse/.style={shape=ellipse},
  diamond/.style={shape=diamond},
  behind path/.code={\def\tikra@behindpath{1}}, in front of path/.code={\def\tikra@behindpath{0}},
  execute at begin to/.code={\expandafter\def\expandafter\tikz@atbegin@to\expandafter{\tikz@atbegin@to#1}},
  execute at end to/.code={\expandafter\def\expandafter\tikz@atend@to\expandafter{\tikz@atend@to#1}},
  style/.code={\pgfkeysalso{#1}},
  every shadow/.style={}, cap/.style={line cap=#1},
  % as TikZ, `radius` is code setting both radii: \pgfkeysvalueof{/tikz/radius} is not it
  /tikz/radius/.code={\pgfkeyssetvalue{/tikz/x radius}{#1}\pgfkeyssetvalue{/tikz/y radius}{#1}},
  /tikz/x radius/.initial=, /tikz/y radius/.initial=,
  /tikz/start angle/.initial=, /tikz/end angle/.initial=, /tikz/delta angle/.initial=,
  /pgfplots/table/search path/.code={}, /pgfplots/compat/.code={},
  /tikz/pics/setup code/.initial=, /tikz/pics/code/.initial=, /tikz/pics/background code/.initial=,
  /tikz/pics/foreground code/.initial=, angle radius/.initial=5mm, angle eccentricity/.initial=.6,
  every pic/.style={}, every matrix/.style={}, every cell/.style={}, every child/.style={}, every child node/.style={}, edge from parent/.style={draw}, name prefix/.code={\tikra@nameprefix{#1}},
  every edge quotes/.style={auto}, every label quotes/.style={}, every pin quotes/.style={}, quotes mean pin/.code={\def\tikra@quotes@pin{1}}, quotes mean label/.code={\def\tikra@quotes@pin{0}}, every node quotes/.style={}, every fit/.style={},
  absolute/.code={\def\tikra@absolute{#1}}, absolute/.default=true, pic type/.code={}, pic anchor/.code={\def\tikra@picanchor{#1}},
  pic text/.code={\def\tikzpictext{#1}}, pic text options/.code={\def\tikzpictextoptions{#1}},
}
\def\tikzoption#1{\@ifnextchar[{\tikra@optiondefault{#1}}{\tikra@option{#1}}}\def\tikra@option#1#2{\tikzset{#1/.code={#2}}}\def\tikra@optiondefault#1[#2]#3{\tikzset{#1/.code={#3},#1/.default={#2}}}\let\tikz@textfont\pgfutil@empty\let\tikz@atbegin@to\pgfutil@empty\let\tikz@atend@to\pgfutil@empty
% PGF 1.x's `set style={{name}=[options]}` defines a style, `{name}+=[options]` adds to one
\def\tikra@setstyle#1#2=[#3]#4\tikra@stop{\def\tikra@temp{#2}\ifx\tikra@temp\pgfutil@empty\pgfkeysalso{#1/.style={#3}}\else\pgfkeysalso{#1/.append style={#3}}\fi}
\tikzset{set style/.code={\tikra@setstyle#1\tikra@stop}}
\def\pgfmathsetseed#1{\pgfmathparse{#1}\tikra@setseed{\pgfmathresult}}
\def\pgfversion{3.1.12}
\def\pgfmathrand{\pgfmathparse{rand}}\def\pgfmathrnd{\pgfmathparse{rnd}}
\def\pgfmathrandominteger#1#2#3{\pgfmathtruncatemacro#1{random(#2,#3)}}
\def\pgfmathrandomitem#1#2{\pgfmathtruncatemacro\tikra@rl{random(\csname pgfmath@randomlist@#2\endcsname)}\edef#1{\expandafter\noexpand\csname pgfmath@randomlist@#2@\tikra@rl\endcsname}}
\newif\iftikra@ignore@size
\tikzset{on grid/.is if=tikra@ignore@size}
\def\pgfsetmatrixcolumnsep#1{\tikzset{column sep={#1}}}
\def\pgfsetmatrixrowsep#1{\tikzset{row sep={#1}}}
\def\pgfmatrixnextcell{\tikra@nextcell}
\def\pgfmatrixendrow{\tikra@endrow}
\tikzset{
  on background layer/.style={execute at begin scope={\pgfonlayer{background}\tikzset{every on background layer/.try,#1}},
    execute at end scope={\endpgfonlayer}},
  background rectangle/.style={draw}, background top/.style={draw}, background bottom/.style={draw},
  background left/.style={draw}, background right/.style={draw}, background grid/.style={help lines,draw},
  inner frame sep/.code={\def\tikz@framexsep{#1}\def\tikz@frameysep{#1}},
  inner frame xsep/.code={\def\tikz@framexsep{#1}}, inner frame ysep/.code={\def\tikz@frameysep{#1}},
  outer frame sep/.code={\def\tikz@outerframexsep{#1}\def\tikz@outerframeysep{#1}},
  outer frame xsep/.code={\def\tikz@outerframexsep{#1}}, outer frame ysep/.code={\def\tikz@outerframeysep{#1}},
  loose background/.style={inner frame sep=2ex}, tight background/.style={inner frame sep=0ex},
  framed/.style={show background rectangle}, gridded/.style={show background grid},
  show background rectangle/.style={execute at end picture={\tikra@frame\tikra@onbg{\path[style=background rectangle] (\tikra@xa,\tikra@ya) rectangle (\tikra@xb,\tikra@yb);}}},
  show background top/.style={execute at end picture={\tikra@frame\tikra@onbg{\path[style=background top] (\tikra@oxa,\tikra@yb) -- (\tikra@oxb,\tikra@yb);}}},
  show background bottom/.style={execute at end picture={\tikra@frame\tikra@onbg{\path[style=background bottom] (\tikra@oxa,\tikra@ya) -- (\tikra@oxb,\tikra@ya);}}},
  show background left/.style={execute at end picture={\tikra@frame\tikra@onbg{\path[style=background left] (\tikra@xa,\tikra@oya) -- (\tikra@xa,\tikra@oyb);}}},
  show background right/.style={execute at end picture={\tikra@frame\tikra@onbg{\path[style=background right] (\tikra@xb,\tikra@oya) -- (\tikra@xb,\tikra@oyb);}}},
  show background grid/.style={execute at end picture={\tikra@frame\tikra@onbg{\path[style=background grid] (\tikra@xa,\tikra@ya) grid (\tikra@xb,\tikra@yb);}}},
}
\def\tikz@framexsep{1ex}\def\tikz@frameysep{1ex}\def\tikz@outerframexsep{0ex}\def\tikz@outerframeysep{0ex}
\def\tikra@onbg#1{\pgfonlayer{background}\pgftransformreset#1\endpgfonlayer}
\def\tikz@picture@start{\tikzset{every picture/.try}}
\def\tikz@scope@options{\tikzset{every scope/.try}}
\newif\iftikz@isdimension\let\tikz@textfont\pgfutil@empty
";

/// Defines every native key and TikZ's own styles.
pub fn define_keys(e: &mut Engine) {
    e.tex.define("pgflinewidth", Meaning::Dimen(4000));
    e.tex.dimens.insert(4000, (0.4 * 65536.0) as i64);
    super::trees::define(e);
    super::decorations::define(e);
    for (path, f) in NATIVE.iter().chain(super::decorations::KEYS).chain(super::libraries::KEYS).chain(super::three_d::KEYS) {
        let id = NATIVE_KEY_BASE + e.key_fns.len() as u16;
        e.key_fns.push(*f);
        e.tex.define(&macro_name(&format!("{path}/.@cmd")), Meaning::Native(id));
    }
    e.register_keys(&super::circuits::LIBRARY);
    e.st.key_path = "/tikz".into();
    for (path, value) in super::shapes::INITIAL {
        e.tex.set_meaning(&Token::cs(&macro_name(path)), macro_from(value), false);
    }
    // the definitions run as TeX, with @ a letter
    let mut lexer = crate::tex::Tex::new_lexer();
    lexer.push_text(DEFINITIONS);
    lexer.push_text(super::basic::DEFINITIONS);
    lexer.push_text(super::decorations::DEFINITIONS);
    lexer.push_text(super::decorations::DECLARATIONS);
    lexer.push_text(super::decorations::textfx::DEFINITIONS);
    lexer.push_text(super::shapes::DEFINITIONS);
    lexer.push_text(super::calendar::DEFINITIONS);
    lexer.push_text(&super::folding::definitions());
    lexer.push_text(super::mindmap::DEFINITIONS);
    lexer.push_text(super::trees::DEFINITIONS);
    lexer.push_text(super::turtle::DEFINITIONS);
    lexer.push_text(super::lsystem::DEFINITIONS);
    let circuits = (super::circuits::LIBRARY.definitions)();
    lexer.push_text(&circuits);
    lexer.push_text(super::libraries::DEFINITIONS);
    lexer.push_text(super::spy::DEFINITIONS);
    let mut tokens = Vec::new();
    while let Some(t) = lexer.get_token() {
        tokens.push(t);
    }
    e.run_tokens(tokens).expect("TikZ's own definitions run without errors");
    e.tex.warnings.clear();
    e.st.key_path = "/".into();
}
