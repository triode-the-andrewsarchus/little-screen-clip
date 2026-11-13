//! `text effects along path` (`tikzlibrarydecorations.text.code.tex`): each character of the text
//! is a node, with options by its number, its word and its place in the word, placed along the
//! path at the middle of its width.

use crate::engine::nodes::{NodeSpec, OnPath};
use crate::engine::Engine;
use crate::scene::{Point, Transform};
use crate::tex::{self, Cat, Token};
use crate::{Error, PT};

use super::library::{to_cm, Run};

/// The `/pgf/decoration/text effects/…` settings kept in the graphic state.
#[derive(Debug, Clone)]
pub struct Settings {
    pub path_from_text: bool,
    pub path_from_text_angle: f64,
    pub fit: bool,
    pub scale: bool,
    pub segment_from_width: bool,
    /// None: a space
    pub word_separator: Option<String>,
    pub character_command: Option<Vec<Token>>,
    pub reverse: bool,
    pub group_letters: bool,
    pub repeat: i64,
    /// the macros named by `character count=`, `character total=`, `letter count=`…
    pub vars: [Option<String>; 6],
    /// `style characters=… with {…}`: a character's text and its options
    pub styles: Vec<(String, Vec<Token>)>,
    /// `replace characters=… with {…}`: a character's text and the code drawn instead
    pub replacements: Vec<(String, Vec<Token>)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            path_from_text: false,
            path_from_text_angle: 0.0,
            fit: false,
            scale: false,
            segment_from_width: true,
            word_separator: None,
            character_command: None,
            reverse: false,
            group_letters: false,
            repeat: 0,
            vars: Default::default(),
            styles: Vec::new(),
            replacements: Vec::new(),
        }
    }
}

/// One character (or group, or word) of the text.
#[derive(Debug, Clone, Default)]
pub struct Char {
    tokens: Vec<Token>,
    text: String,
    sep: bool,
    letter: usize,
    word: usize,
    final_letter: bool,
    letter_total: usize,
    pub pre: f64,
    pub post: f64,
}

/// What the decoration keeps while it runs.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub chars: Vec<Char>,
    word_total: usize,
    pub count: usize,
    repeat: i64,
    factor: f64,
}

/// `\tikz@lib@dec@te@scancharacters`: spaces, `{groups}` and single tokens.
pub fn scan(text: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        match &text[i] {
            Token::Char(_, Cat::Begin) => {
                let mut depth = 0;
                let start = i;
                while i < text.len() {
                    match text[i] {
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
                out.push(text[start..(i + 1).min(text.len())].to_vec());
            }
            t => out.push(vec![t.clone()]),
        }
        i += 1;
    }
    out
}

fn is_space(t: &[Token]) -> bool {
    matches!(t, [Token::Char(_, Cat::Space)])
}

impl Run<'_> {
    fn is_sep(&self, t: &[Token]) -> bool {
        match &self.s.te.word_separator {
            None => is_space(t),
            Some(s) => tex::tokens_to_string(t).trim() == s.trim() && !is_space(t),
        }
    }

    /// The `setup` state: the characters, their counts and widths, and the alignment.
    pub(super) fn te_setup(&mut self) -> Result<(), Error> {
        if !(self.s.te.path_from_text && !self.te.chars.is_empty()) {
            self.te_prepare()?;
        }
        let n = self.te.chars.len();
        if n == 0 {
            return Ok(());
        }
        let l = self.w.section;
        let total: f64 = self.te.chars.iter().map(|c| c.pre + c.post).sum();
        if self.s.te.scale {
            self.te.factor = l / total.max(1e-9);
            for c in &mut self.te.chars {
                c.pre *= self.te.factor;
                c.post *= self.te.factor;
            }
        } else if self.s.te.fit {
            let (a, b) = (self.te.chars[0].post, self.te.chars[n - 1].pre);
            let k = (l - a - b) / (total - a - b).max(1e-9);
            for (i, c) in self.te.chars.iter_mut().enumerate() {
                if i > 0 {
                    c.pre *= k;
                }
                if i + 1 < n {
                    c.post *= k;
                }
            }
        } else {
            let align = self.key_text("text align/align");
            let room = (l - total).max(0.0);
            if align == "center" {
                self.te.chars[0].pre += room / 2.0;
                self.te.chars[n - 1].post += room / 2.0;
            } else if align == "right" {
                self.te.chars[0].pre += room;
            }
        }
        self.te.count = 0;
        self.te.repeat = self.s.te.repeat;
        Ok(())
    }

    /// `\tikz@lib@dec@te@preparetext`
    pub(super) fn te_prepare(&mut self) -> Result<(), Error> {
        let mut tokens = scan(&self.s.text);
        if self.s.te.reverse {
            tokens.reverse();
        }
        if self.s.te.group_letters {
            let mut grouped = Vec::new();
            let mut word: Vec<Token> = Vec::new();
            for t in tokens {
                if self.is_sep(&t) {
                    if !word.is_empty() {
                        grouped.push(std::mem::take(&mut word));
                    }
                    grouped.push(t);
                } else {
                    word.extend(t);
                }
            }
            if !word.is_empty() {
                grouped.push(word);
            }
            tokens = grouped;
        }
        let mut chars: Vec<Char> = Vec::new();
        let (mut word, mut letter) = (0usize, 0usize);
        for t in tokens {
            let sep = self.is_sep(&t);
            if sep {
                if letter > 0 {
                    if let Some(c) = chars.last_mut() {
                        c.final_letter = true;
                    }
                }
                letter = 0;
            } else {
                if letter == 0 {
                    word += 1;
                }
                letter += 1;
            }
            let text = tex::tokens_to_string(&t);
            chars.push(Char { tokens: t, text, sep, letter, word, ..Default::default() });
        }
        if letter > 0 {
            if let Some(c) = chars.last_mut() {
                c.final_letter = true;
            }
        }
        // the letters of each word
        let mut totals = std::collections::HashMap::new();
        for c in &chars {
            if !c.sep {
                totals.insert(c.word, c.letter);
            }
        }
        // as pgf has it, the total of the last word for every character
        let last = totals.get(&word).copied().unwrap_or(0);
        for c in &mut chars {
            c.letter_total = last;
        }
        self.te = State { chars, word_total: word, count: 0, repeat: 0, factor: 1.0 };
        for i in 0..self.te.chars.len() {
            let w = self.te_node(i, None)?;
            self.te.chars[i].pre = w / 2.0;
            self.te.chars[i].post = w / 2.0;
        }
        Ok(())
    }

    /// The `scan` state: the next character, or the end (or the text again with `repeat text`).
    pub(super) fn te_scan(&mut self) {
        self.te.count += 1;
        if self.te.count > self.te.chars.len() {
            if self.te.repeat == 0 {
                self.set_next_state("final");
            } else {
                self.te.repeat -= 1;
                self.te.count = 1;
            }
        }
    }

    pub(super) fn te_current(&self) -> Option<&Char> {
        self.te.count.checked_sub(1).and_then(|i| self.te.chars.get(i))
    }

    /// The node of character `i`: drawn at `t`, or only measured (its width or height, pt).
    pub(super) fn te_node(&mut self, i: usize, t: Option<&Transform>) -> Result<f64, Error> {
        let c = self.te.chars[i].clone();
        let n = self.te.chars.len();
        let te = self.s.te.clone();
        let mut opts = String::from("/pgf/decoration/text effects/.cd,");
        opts.push_str(if t.is_none() { "every character width/.try," } else { "every character/.try," });
        opts.push_str(&format!("character {}/.try,", i + 1));
        let mut extra: Vec<Token> = Vec::new();
        for (s, o) in &te.styles {
            if s.trim() == c.text.trim() {
                extra.extend(super::super::tikra_tokens(","));
                extra.extend(o.iter().cloned());
            }
        }
        if c.sep {
            opts.push_str("/pgf/decoration/text effects/every word separator/.try,");
        } else {
            // (a character that is no separator is a letter of a word: both counts start at 1)
            opts.push_str(&format!("every word/.try,word {}/.try,", c.word));
            opts.push_str(&format!("every letter/.try,every {} letter/.try,", c.letter));
            if c.letter == 1 {
                opts.push_str("every first letter/.try,");
            }
            if c.final_letter {
                opts.push_str("every final letter/.try,");
            }
        }
        let mut options = super::super::tikra_tokens(&opts);
        options.extend(extra);
        options.extend(super::super::tikra_tokens(&format!(",/tikz/scale={}", self.te.factor.max(1e-6))));
        let e = &mut *self.e;
        e.begin_group();
        let values = [(i + 1).to_string(), n.to_string(), c.letter.to_string(), c.letter_total.to_string(), c.word.to_string(), self.te.word_total.to_string()];
        for (name, v) in [
            "tikzdecorationcharactercount",
            "tikzdecorationcharactertotal",
            "tikzdecorationlettercount",
            "tikzdecorationlettertotal",
            "tikzdecorationwordcount",
            "tikzdecorationwordtotal",
        ]
        .iter()
        .zip(&values)
        {
            e.tex.define_string(name, v, false);
        }
        for (var, v) in te.vars.iter().zip(&values) {
            if let Some(var) = var {
                e.tex.define_string(var.trim_start_matches('\\'), v, false);
            }
        }
        e.tex.set_meaning(&Token::cs("tikzdecorationcharacter"), crate::engine::macro_tokens(c.tokens.clone()), false);
        let saved = (e.canvas.min, e.canvas.max);
        e.st.transform = t.map(to_cm).unwrap_or_default();
        e.st.pending = Transform::default();
        let at = e.st.transform.apply(Point::default());
        let mut text = c.tokens.clone();
        // the character's styles may set its command (`every word/.style={character command=…}`)
        e.begin_group();
        let styled = e.process_keys(options.clone(), Some("/tikz")).map(|_| e.st.decoration.te.character_command.clone());
        e.end_group();
        let command = styled.ok().flatten().or_else(|| te.character_command.clone());
        if let Some(cmd) = &command {
            let mut v = cmd.clone();
            v.push(Token::Char('{', Cat::Begin));
            v.extend(text);
            v.push(Token::Char('}', Cat::End));
            text = v;
        }
        // (a control space `\ ` is one too)
        let blank = is_space(&c.tokens) || matches!(&c.tokens[..], [Token::Cs(n)] if &**n == " ");
        if blank {
            // a space: an empty box as wide as the font's interword space
            text = super::super::tikra_tokens("$\\hspace{0.33333em}$");
        }
        if let Some((_, code)) = te.replacements.iter().find(|(s, _)| s.trim() == c.text.trim()) {
            // `replace characters`: the code in a scope with the character's options; its width
            // is that of the picture it draws
            let code = code.clone();
            let saved_items = std::mem::take(&mut e.canvas.items);
            e.canvas.min = None;
            e.canvas.max = None;
            // measured as a picture of its own: `use as bounding box` and clips around do not count
            let frozen = std::mem::replace(&mut e.canvas_frozen, false);
            let clipped: Vec<bool> = e.scopes.iter_mut().map(|s| std::mem::take(&mut s.clipped)).collect();
            e.begin_scope();
            let r = e.process_keys(options, Some("/tikz")).and_then(|_| {
                e.st.apply_pending();
                e.run_tokens(code)
            });
            e.end_scope();
            e.canvas_frozen = frozen;
            for (s, c) in e.scopes.iter_mut().zip(clipped) {
                s.clipped = c;
            }
            let drawn = std::mem::replace(&mut e.canvas.items, saved_items);
            let (lo, hi) = (e.canvas.min, e.canvas.max);
            (e.canvas.min, e.canvas.max) = saved;
            e.end_group();
            r?;
            if t.is_some() {
                self.e.canvas.items.extend(drawn);
                // the drawing counts in the picture as it was drawn
                if self.e.counts_for_size() {
                    for q in [lo, hi].into_iter().flatten() {
                        self.e.canvas.include(q);
                    }
                }
            }
            let width = lo.zip(hi).map_or(0.0, |(a, b)| b.x - a.x);
            return Ok(width / PT);
        }
        let name = "tikra@te@character".to_string();
        let r = e.node(NodeSpec { options, name: Some(name.clone()), at: None, text, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None });
        // the size of what the node draws, as pgf measures the picture it makes: its outline (no
        // outer sep), with half the line width around it if it is drawn
        let width = e.nodes.get(&name).map(|info| {
            let mut b = crate::canvas::Canvas::default();
            for p in info.geometry.paths.iter().filter(|p| p.layer == crate::engine::shapes::Layer::Background && matches!(p.usage, crate::engine::shapes::Usage::Node)) {
                b.include_path(&crate::canvas::transform_segments(&p.segments, &info.transform));
            }
            match (b.min, b.max) {
                (Some(lo), Some(hi)) if te.segment_from_width => (hi.x - lo.x, info.transform),
                (Some(lo), Some(hi)) => (hi.y - lo.y, info.transform),
                _ => (0.0, info.transform),
            }
        });
        e.nodes.remove(&name);
        e.end_group();
        let items = r?;
        let line = items.iter().find_map(|i| match i {
            crate::canvas::Item::Path { stroke: Some(s), .. } => Some(s.width),
            _ => None,
        });
        let width = width.map(|(w, _)| w + line.unwrap_or(0.0));
        if t.is_none() {
            (self.e.canvas.min, self.e.canvas.max) = saved;
        } else {
            self.e.canvas.items.extend(items);
        }
        Ok(width.unwrap_or(0.0) / PT)
    }
}

/// The keys of text effects.
pub fn key_unknown(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    // unknown here: a TikZ key (`\pgfkeysalso{/tikz/…}`)
    let name = super::current_name(e);
    let path = format!("/tikz/{name}");
    e.invoke(&path, v)
}

fn text(e: &mut Engine, v: &Option<Vec<Token>>) -> String {
    v.as_ref().map(|v| e.tex.expand_to_string(v.clone()).unwrap_or_default().trim().to_string()).unwrap_or_default()
}

fn flag(e: &mut Engine, v: &Option<Vec<Token>>) -> bool {
    text(e, v) != "false"
}

macro_rules! te_key {
    ($name:ident, |$e:ident, $v:ident| $body:expr) => {
        pub fn $name($e: &mut Engine, $v: Option<Vec<Token>>) -> Result<(), Error> {
            $body;
            Ok(())
        }
    };
}

te_key!(k_path_from_text, |e, v| e.st.decoration.te.path_from_text = flag(e, &v));
te_key!(k_path_from_text_angle, |e, v| {
    let t = text(e, &v);
    e.st.decoration.te.path_from_text_angle = e.number(&t).unwrap_or(0.0)
});
te_key!(k_fit, |e, v| e.st.decoration.te.fit = flag(e, &v));
te_key!(k_scale, |e, v| e.st.decoration.te.scale = flag(e, &v));
te_key!(k_segment_from_width, |e, v| e.st.decoration.te.segment_from_width = flag(e, &v));
te_key!(k_word_separator, |e, v| {
    let t = v.as_ref().map(|v| tex::tokens_to_string(v)).unwrap_or_default();
    e.st.decoration.te.word_separator = if t.trim() == "space" { None } else { Some(t) }
});
te_key!(k_character_command, |e, v| e.st.decoration.te.character_command = v.filter(|v| !crate::keys::trim(v).is_empty()));
te_key!(k_reverse, |e, _v| e.st.decoration.te.reverse = true);
te_key!(k_group, |e, _v| e.st.decoration.te.group_letters = true);
te_key!(k_repeat, |e, v| {
    let t = text(e, &v);
    e.st.decoration.te.repeat = if t.is_empty() { -1 } else { e.number(&t).unwrap_or(0.0) as i64 }
});
fn var(e: &mut Engine, v: &Option<Vec<Token>>, k: usize) {
    let t = v.as_ref().map(|v| tex::tokens_to_string(v)).unwrap_or_default();
    e.st.decoration.te.vars[k] = if t.trim().is_empty() { None } else { Some(t.trim().to_string()) };
}
te_key!(k_character_count, |e, v| var(e, &v, 0));
te_key!(k_character_total, |e, v| var(e, &v, 1));
te_key!(k_letter_count, |e, v| var(e, &v, 2));
te_key!(k_letter_total, |e, v| var(e, &v, 3));
te_key!(k_word_count, |e, v| var(e, &v, 4));
te_key!(k_word_total, |e, v| var(e, &v, 5));
te_key!(k_style_characters, |e, v| {
    let v = v.unwrap_or_default();
    match super::split_word(&v, "with") {
        Some((chars, opts)) => {
            let opts = tex::strip_outer_braces(crate::keys::trim(&opts).to_vec());
            for c in scan(crate::keys::trim(&chars)) {
                let t = tex::tokens_to_string(&c);
                e.st.decoration.te.styles.push((t, opts.clone()));
            }
        }
        None => e.warn("style characters: '… with {…}' expected: ignored"),
    }
});
te_key!(k_replace_characters, |e, v| {
    let v = v.unwrap_or_default();
    match super::split_word(&v, "with") {
        Some((chars, code)) => {
            let code = tex::strip_outer_braces(crate::keys::trim(&code).to_vec());
            for c in scan(crate::keys::trim(&chars)) {
                let t = tex::tokens_to_string(&c);
                e.st.decoration.te.replacements.push((t, code.clone()));
            }
        }
        None => e.warn("replace characters: '… with {…}' expected: ignored"),
    }
});
te_key!(k_characters_append, |e, v| {
    let mut t = super::super::tikra_tokens("/pgf/decoration/text effects/every character/.append style=");
    t.push(Token::Char('{', Cat::Begin));
    t.extend(v.unwrap_or_default());
    t.push(Token::Char('}', Cat::End));
    e.process_keys(t, None)?
});
te_key!(k_widths_append, |e, v| {
    let mut t = super::super::tikra_tokens("/pgf/decoration/text effects/every character width/.append style=");
    t.push(Token::Char('{', Cat::Begin));
    t.extend(v.unwrap_or_default());
    t.push(Token::Char('}', Cat::End));
    e.process_keys(t, None)?
});
te_key!(k_text_effects, |e, v| e.process_keys(v.unwrap_or_default(), Some("/pgf/decoration/text effects"))?);

/// TikZ's definitions of the text effects styles.
pub const DEFINITIONS: &str = r"
\def\tikra@te@style#1#2{\pgfkeys{/pgf/decoration/text effects/every character#1/.style={#2}}}
\pgfkeys{/pgf/decoration/text effects/.cd,
  % a character's width is measured with its own style, unless the widths have one
  every character/.style={}, every character width/.style={/pgf/decoration/text effects/every character/.try},
  characters/.code={\tikra@te@style{}{#1}}, character widths/.code={\tikra@te@style{ width}{#1}},
  text along path/.style={transform shape, anchor=base, inner xsep=0pt},
}
";
