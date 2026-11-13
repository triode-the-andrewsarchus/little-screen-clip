//! Lindenmayer systems (`pgflibrarylindenmayersystems`, `tikzlibrarylindenmayersystems`).
//!
//! `\pgfdeclarelindenmayersystem{name}{\symbol{X}{code} \rule{F -> F-F++F-F}}` declares a system:
//! rules rewrite a symbol (one character) into a string, symbols say what a character draws. The
//! axiom is rewritten `order` times, characters without a rule staying as they are, and the result
//! is drawn by a turtle: by default `F` draws `step` forward, `f` moves, `+` and `-` turn by the
//! left and right angles, `[` and `]` save and restore where the turtle is (restoring moves the
//! path there). The turtle starts at the origin of the current coordinate system, facing x.
//!
//! In TikZ, `lindenmayer system` (or `l-system`) is a path operation, with the keys of
//! `l-system={name, axiom=…, order=…, step=…, angle=…, rule set={…}, anchor=…}`; `rule set`
//! declares a system on the spot, and `anchor` places the drawing like a node at the current point
//! instead. The basic layer's `\pgflindenmayersystem{name}{axiom}{order}` draws on the pgf path.

use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Meaning, Token};
use crate::{Error, PT};

use super::Engine;

/// Ids of the commands (`Meaning::Native`), after the mindmap library's.
pub const BASE: u16 = 895;

const COMMANDS: &[&str] = &["pgfdeclarelindenmayersystem", "pgflindenmayersystem", "tikra@lssymbol", "tikra@lsrule", "tikra@lsystem"];

pub fn define(e: &mut Engine) {
    for (i, name) in COMMANDS.iter().enumerate() {
        e.tex.define(name, Meaning::Native(BASE + i as u16));
    }
}

pub fn is_lsystem(id: u16) -> bool {
    (BASE..BASE + COMMANDS.len() as u16).contains(&id)
}

/// The keys, kept in macros (`\tikra@ls@…`).
pub const DEFINITIONS: &str = r"
\def\tikra@ls@name{}\def\tikra@ls@axiom{}\def\tikra@ls@order{0}\def\tikra@ls@anchor{}
\def\tikra@ls@step{5pt}\def\tikra@ls@left{90}\def\tikra@ls@right{90}\def\tikra@ls@rstep{0}\def\tikra@ls@rangle{0}
\pgfkeys{/pgf/lindenmayer system/.cd,
  name/.code={\def\tikra@ls@name{#1}}, axiom/.code={\def\tikra@ls@axiom{#1}}, order/.code={\def\tikra@ls@order{#1}},
  anchor/.code={\def\tikra@ls@anchor{#1}}, rule set/.code={\tikra@lsystem{rule set}{#1}},
  step/.code={\def\tikra@ls@step{#1}}, left angle/.code={\def\tikra@ls@left{#1}}, right angle/.code={\def\tikra@ls@right{#1}},
  angle/.code={\def\tikra@ls@left{#1}\def\tikra@ls@right{#1}},
  randomize step percent/.code={\def\tikra@ls@rstep{#1}}, randomize angle percent/.code={\def\tikra@ls@rangle{#1}},
}
\tikzset{lindenmayer system/.code={\tikra@lsystem{keys}{#1}}, l-system/.code={\tikra@lsystem{keys}{#1}}}
";

/// What a character makes the turtle do.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Forward,
    Move,
    Left,
    Right,
    Save,
    Restore,
}

fn default_op(c: char) -> Option<Op> {
    Some(match c {
        'F' => Op::Forward,
        'f' => Op::Move,
        '+' => Op::Left,
        '-' => Op::Right,
        '[' => Op::Save,
        ']' => Op::Restore,
        _ => return None,
    })
}

/// The longest string a system may grow to.
const MAX_LENGTH: usize = 2_000_000;

/// `axiom` rewritten `order` times by `rule`.
fn rewrite(axiom: &str, order: usize, rule: impl Fn(char) -> Option<String>) -> String {
    let mut s = axiom.to_string();
    for _ in 0..order {
        let mut next = String::with_capacity(s.len() * 2);
        for c in s.chars() {
            match rule(c) {
                Some(r) => next.push_str(&r),
                None => next.push(c),
            }
            if next.len() > MAX_LENGTH {
                return next;
            }
        }
        s = next;
    }
    s
}

impl Engine {
    pub fn lsystem_command(&mut self, id: u16) -> Result<(), Error> {
        match COMMANDS[(id - BASE) as usize] {
            "pgfdeclarelindenmayersystem" => {
                let name = self.argument_string()?;
                let body = self.argument()?;
                self.declare_lsystem(&name, body)
            }
            "pgflindenmayersystem" => {
                let name = self.argument_string()?;
                let axiom = self.argument_string()?;
                let order = self.argument_string()?;
                let segments = self.lsystem_segments(&name, &axiom, &order, self.st.transform)?;
                for s in segments {
                    self.pgf_add(s);
                }
                Ok(())
            }
            "tikra@lssymbol" => {
                let c = self.argument_string()?;
                let code = self.argument()?;
                let name = self.ls_macro_string("tikra@ls@declaring");
                self.tex.set_meaning(&Token::cs(&format!("tikra@ls@{name}@symbol@{c}")), super::macro_tokens(code), true);
                Ok(())
            }
            "tikra@lsrule" => {
                let rule = self.argument_string()?;
                let name = self.ls_macro_string("tikra@ls@declaring");
                self.lsystem_rule(&name, &rule)
            }
            _ => {
                let what = self.argument_string()?;
                let value = self.argument()?;
                if what == "rule set" {
                    // a system declared on the spot, from rules separated by commas
                    self.tex.define_string("tikra@ls@declaring", "tikz@temp", false);
                    let text = self.tex.expand_to_string(value)?;
                    for rule in text.split(',').filter(|r| !r.trim().is_empty()) {
                        self.lsystem_rule("tikz@temp", rule)?;
                    }
                    self.tex.set_meaning(&Token::cs("tikra@ls@tikz@temp"), super::macro_from(""), true);
                    self.tex.define_string("tikra@ls@name", "tikz@temp", false);
                    return Ok(());
                }
                // the keys of `l-system={…}`: a key that names a system chooses it
                for item in crate::keys::split_list(&value) {
                    let key = item.key.trim().to_string();
                    if item.value.is_none() && self.tex.is_defined(&format!("tikra@ls@{key}")) {
                        self.tex.define_string("tikra@ls@name", &key, false);
                        continue;
                    }
                    let mut v = tex::string_tokens(&key);
                    if let Some(x) = item.value {
                        v.push(Token::other('='));
                        v.push(Token::Char('{', tex::Cat::Begin));
                        v.extend(x);
                        v.push(Token::Char('}', tex::Cat::End));
                    }
                    self.process_keys(v, Some("/pgf/lindenmayer system"))?;
                }
                Ok(())
            }
        }
    }

    fn ls_macro_string(&mut self, name: &str) -> String {
        self.tex.expand_to_string(vec![Token::cs(name)]).map(|s| s.trim().to_string()).unwrap_or_default()
    }

    /// `\pgfdeclarelindenmayersystem`: the body runs with `\symbol` and `\rule` defining the system.
    fn declare_lsystem(&mut self, name: &str, body: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        self.tex.define_string("tikra@ls@declaring", name, false);
        let symbol = self.tex.meaning_of("tikra@lssymbol");
        let rule = self.tex.meaning_of("tikra@lsrule");
        self.tex.set_meaning(&Token::cs("symbol"), symbol, false);
        self.tex.set_meaning(&Token::cs("rule"), rule, false);
        let r = self.run_tokens(body);
        self.end_group();
        self.tex.set_meaning(&Token::cs(&format!("tikra@ls@{name}")), super::macro_from(""), true);
        r
    }

    /// `F -> F-F++F-F`: the head (one character) and what it becomes, spaces left out.
    fn lsystem_rule(&mut self, name: &str, rule: &str) -> Result<(), Error> {
        let Some((head, body)) = rule.split_once("->") else {
            return Err(Error::Figure(format!("Lindenmayer rule '{rule}' has no '->'")));
        };
        let Some(c) = head.trim().chars().next() else { return Err(Error::Figure(format!("Lindenmayer rule '{rule}' has no symbol"))) };
        let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        self.tex.set_meaning(&Token::cs(&format!("tikra@ls@{name}@rule@{c}")), super::macro_from(&body), true);
        Ok(())
    }

    /// The path a system draws, its turtle starting at the origin of `start`.
    fn lsystem_segments(&mut self, name: &str, axiom: &str, order: &str, start: Transform) -> Result<Vec<Segment>, Error> {
        let order = self.number(order)?.trunc().max(0.0) as usize;
        let mut rules = std::collections::HashMap::new();
        let mut ops = std::collections::HashMap::new();
        // the rules and symbols of the characters that appear
        let mut seen: Vec<char> = axiom.chars().collect();
        let mut i = 0;
        while i < seen.len() {
            let c = seen[i];
            i += 1;
            if rules.contains_key(&c) || ops.contains_key(&c) {
                continue;
            }
            let rule_name = format!("tikra@ls@{name}@rule@{c}");
            if self.tex.is_defined(&rule_name) {
                let r = self.ls_macro_string(&rule_name);
                seen.extend(r.chars());
                rules.insert(c, r);
            }
            let symbol = format!("tikra@ls@{name}@symbol@{c}");
            let op = if self.tex.is_defined(&symbol) {
                // (`\symbol` defines it as a macro)
                let code = super::spy::macro_body(self, &symbol);
                let code: Vec<&Token> = code.iter().filter(|t| !t.is_space()).collect();
                match code.as_slice() {
                    [Token::Cs(n)] => match &**n {
                        "pgflsystemdrawforward" => Some(Op::Forward),
                        "pgflsystemmoveforward" => Some(Op::Move),
                        "pgflsystemturnleft" => Some(Op::Left),
                        "pgflsystemturnright" => Some(Op::Right),
                        "pgflsystemsavestate" => Some(Op::Save),
                        "pgflsystemrestorestate" => Some(Op::Restore),
                        other => {
                            self.warn(format!("Lindenmayer symbol {c}: \\{other} not supported: ignored"));
                            None
                        }
                    },
                    [] => None,
                    _ => {
                        self.warn(format!("Lindenmayer symbol {c}: only one of the \\pgflsystem… commands is supported: ignored"));
                        None
                    }
                }
            } else {
                default_op(c)
            };
            ops.insert(c, op);
        }
        let s = rewrite(axiom, order, |c| rules.get(&c).cloned());
        let step = self.ls_macro_string("tikra@ls@step");
        let step = self.length(&step)? / PT;
        let (left, right) = (self.ls_macro_string("tikra@ls@left"), self.ls_macro_string("tikra@ls@right"));
        let (left, right) = (self.number(&left)?, self.number(&right)?);
        let (rstep, rangle) = (self.ls_macro_string("tikra@ls@rstep"), self.ls_macro_string("tikra@ls@rangle"));
        let (rstep, rangle) = (self.number(&rstep)?, self.number(&rangle)?);
        // pgf's random numbers, in [-1, 1]
        let mut seed: i64 = 1;
        let mut rand = || {
            let (m, a, q, r) = (2_147_483_647i64, 69_621i64, 30_845i64, 23_902i64);
            let mut next = a * (seed - (seed / q) * q) - r * (seed / q);
            if next < 0 {
                next += m;
            }
            seed = next;
            (next - (next / 200_001) * 200_001 - 100_000) as f64 / 100_000.0
        };
        let mut t = start;
        let mut saved = Vec::new();
        let mut out = Vec::new();
        for c in s.chars() {
            let Some(Some(op)) = ops.get(&c) else { continue };
            match op {
                Op::Forward | Op::Move => {
                    // pgf adds the percentage over 20, in pt, times a random number
                    let d = if rstep != 0.0 { step + rand() * rstep / 20.0 } else { step };
                    t = t.compose(&Transform::translation(d * PT, 0.0));
                    let p = t.apply(Point::new(0.0, 0.0));
                    out.push(if *op == Op::Forward { Segment::LineTo(p) } else { Segment::MoveTo(p) });
                }
                Op::Left | Op::Right => {
                    let a = if *op == Op::Left { left } else { -right };
                    let a = if rangle != 0.0 { a + a.signum() * rand() * rangle / 20.0 } else { a };
                    t = t.compose(&Transform::rotation(a));
                }
                Op::Save => saved.push(t),
                Op::Restore => {
                    if let Some(x) = saved.pop() {
                        t = x;
                    }
                    out.push(Segment::MoveTo(t.apply(Point::new(0.0, 0.0))));
                }
            }
        }
        Ok(out)
    }

    /// The `lindenmayer system` path operation from `at`: the segments to add to the path.
    pub fn lindenmayer_path(&mut self, at: Point) -> Result<Vec<Segment>, Error> {
        let name = self.ls_macro_string("tikra@ls@name");
        if name.is_empty() || !self.tex.is_defined(&format!("tikra@ls@{name}")) {
            return Err(Error::Figure(format!("no Lindenmayer system named '{name}'")));
        }
        let (axiom, order) = (self.ls_macro_string("tikra@ls@axiom"), self.ls_macro_string("tikra@ls@order"));
        let anchor = self.ls_macro_string("tikra@ls@anchor");
        if anchor.is_empty() {
            // the turtle starts at the current point, with the picture's transformation
            let o = self.st.transform.apply(Point::new(0.0, 0.0));
            let t = Transform::translation(at.x - o.x, at.y - o.y).compose(&self.st.transform);
            let mut v = vec![Segment::MoveTo(at)];
            v.extend(self.lsystem_segments(&name, &axiom, &order, t)?);
            return Ok(v);
        }
        // drawn on its own, then placed by the anchor of its box at the current point
        let mut v = vec![Segment::MoveTo(Point::new(0.0, 0.0))];
        v.extend(self.lsystem_segments(&name, &axiom, &order, Transform::default())?);
        let mut c = crate::canvas::Canvas::default();
        c.include_path(&v);
        let (lo, hi) = (c.min.unwrap_or_default(), c.max.unwrap_or_default());
        let x = if anchor.contains("west") {
            lo.x
        } else if anchor.contains("east") {
            hi.x
        } else {
            (lo.x + hi.x) / 2.0
        };
        let y = if anchor.contains("south") || anchor.contains("base") {
            lo.y
        } else if anchor.contains("north") {
            hi.y
        } else {
            (lo.y + hi.y) / 2.0
        };
        Ok(crate::canvas::transform_segments(&v, &Transform::translation(at.x - x, at.y - y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_rewrite() {
        let r = |c: char| if c == 'F' { Some("F+F".to_string()) } else { None };
        assert_eq!(rewrite("F-", 2, r), "F+F+F+F-");
    }

    #[test]
    fn a_koch_curve() {
        let (canvas, warnings) = super::super::run_document(
            r"\begin{tikzpicture}\pgfdeclarelindenmayersystem{Koch curve}{\rule{F -> F-F++F-F}}
\draw [l-system={Koch curve, step=2pt, angle=60, axiom=F++F++F, order=2}] lindenmayer system -- cycle;\end{tikzpicture}",
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let crate::canvas::Item::Path { segments, .. } = &canvas.items[0] else { panic!() };
        // 3 sides of 16 segments each, and the cycle
        assert_eq!(segments.iter().filter(|s| matches!(s, Segment::LineTo(_))).count(), 48);
    }
}
