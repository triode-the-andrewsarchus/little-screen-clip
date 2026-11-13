//! Trees: `node {root} child {node {a}} child[options] {node {b} child {…}}`, as `tikz.code.tex`
//! grows them. The children of a node are collected; for each one the coordinate system moves to
//! the parent's center, then `level distance` along `grow` and its place among the siblings
//! (`sibling distance`); the child's node is named `parent-k`, its own children follow, and the
//! `edge from parent` is drawn last, with its label nodes midway.

use crate::scene::{Point, Transform};
use crate::tex::{self, Cat, Token};
use crate::Error;

use super::{macro_from, tikra_tokens, Engine};

/// One child: its options and its body, and what `child foreach` defines for it.
struct Child {
    options: Vec<Token>,
    body: Vec<Token>,
    define: Vec<Token>,
}

impl Engine {
    /// `child …` after a node on a path: the word `child` already read.
    pub fn children(&mut self, parent: Option<String>) -> Result<(), Error> {
        let Some(info) = self.last_node.clone() else {
            return Err(Error::Figure("child without a node before it".into()));
        };
        // the parent needs a name for the edges: an unnamed one gets one, as TikZ does
        let parent = match parent {
            Some(p) => p,
            None => {
                self.tree_count += 1;
                let n = format!("tikra@tree@{}", self.tree_count);
                self.alias_last_node(&n);
                self.nodes.insert(n.clone(), info);
                n
            }
        };
        let children = self.collect_children()?;
        self.begin_group();
        let r = self.grow_children(&parent, children);
        self.end_group();
        r
    }

    /// Reads `child[options]{body}` (and `child foreach \x in {…} {body}`) while they follow.
    fn collect_children(&mut self) -> Result<Vec<Child>, Error> {
        let mut out = Vec::new();
        let mut first = true;
        loop {
            if !first {
                // another `child`?
                let mut read = Vec::new();
                let mut word = String::new();
                while let Some(t) = self.tex.get_token() {
                    match &t {
                        Token::Char(c, Cat::Letter) => {
                            word.push(*c);
                            read.push(t);
                        }
                        x if x.is_space() && word.is_empty() => read.push(t),
                        _ => {
                            self.tex.back(t);
                            break;
                        }
                    }
                }
                if word != "child" {
                    self.tex.push_tokens(read);
                    return Ok(out);
                }
            }
            first = false;
            let options = match self.tex.next_nonspace() {
                Some(t) if t.is_char('[') => self.tex.read_until_char(']', false)?,
                Some(t) => {
                    self.tex.back(t);
                    Vec::new()
                }
                None => Vec::new(),
            };
            // child foreach \x in {list} {body}
            let mut peek = Vec::new();
            let mut word = String::new();
            while let Some(t) = self.tex.get_token() {
                match &t {
                    Token::Char(c, Cat::Letter) => {
                        word.push(*c);
                        peek.push(t);
                    }
                    x if x.is_space() && word.is_empty() => peek.push(t),
                    _ => {
                        self.tex.back(t);
                        break;
                    }
                }
            }
            if word == "foreach" {
                // `\x/\y [count=\i] in {list}`, as a `\foreach`
                let mut vars: Vec<Token> = Vec::new();
                let mut opts: Vec<Token> = Vec::new();
                loop {
                    match self.tex.next_nonspace() {
                        Some(t) if t.is_char('[') => opts.extend(self.tex.read_until_char(']', false)?),
                        Some(t) if t.is_char('i') => match self.tex.get_token() {
                            Some(n) if n.is_char('n') => break,
                            // not `in`: what follows is read again (the end of the input stops below)
                            n => n.into_iter().for_each(|n| self.tex.back(n)),
                        },
                        Some(t @ Token::Cs(_)) => vars.push(t),
                        Some(_) => {}
                        None => return Err(Error::Figure("child foreach without 'in'".into())),
                    }
                }
                if vars.is_empty() {
                    return Err(Error::Figure("child foreach without a variable".into()));
                }
                let list = self.tex.read_undelimited()?;
                let list = self.tex.expand_fully(list)?;
                let body = match self.tex.next_nonspace() {
                    Some(Token::Char(_, Cat::Begin)) => self.tex.read_group_body()?,
                    Some(t) => {
                        self.tex.back(t);
                        Vec::new()
                    }
                    None => Vec::new(),
                };
                let o = super::foreach_options(&opts);
                let mut memory = o.initial_memory();
                // what each iteration defines: the variables and what the options set
                let mut names = vars.clone();
                names.extend(o.count.iter().map(|c| c.0.clone()));
                names.extend(o.remember.iter().map(|r| r.1.clone()));
                names.extend(o.evaluate.iter().map(|v| v.1.clone().unwrap_or_else(|| v.0.clone())));
                for (k, item) in super::foreach_items(&list).into_iter().enumerate() {
                    self.begin_group();
                    let parts: Vec<Vec<Token>> = item.split(|t| t.is_char('/')).map(|p| crate::keys::trim(p).to_vec()).collect();
                    for (i, n) in vars.iter().enumerate() {
                        let v = parts.get(i).or(parts.last()).cloned().unwrap_or_default();
                        self.tex.set_meaning(n, super::macro_tokens(v), false);
                    }
                    let r = self.foreach_iteration(&o, k, &memory);
                    // they are defined (a \def each) before the child's options are read
                    let mut b = Vec::new();
                    for n in &names {
                        if let tex::Meaning::Macro(m) = self.tex.meaning(n) {
                            b.extend([Token::cs("def"), n.clone(), Token::Char('{', Cat::Begin)]);
                            b.extend(tex::substitute(&m.body, &[]));
                            b.push(Token::Char('}', Cat::End));
                        }
                    }
                    self.foreach_remember(&o, &mut memory);
                    self.end_group();
                    r?;
                    out.push(Child { options: options.clone(), body: body.clone(), define: b });
                }
                continue;
            }
            self.tex.push_tokens(peek);
            let body = match self.tex.next_nonspace() {
                Some(Token::Char(_, Cat::Begin)) => self.tex.read_group_body()?,
                Some(t) => {
                    self.tex.back(t);
                    Vec::new()
                }
                None => Vec::new(),
            };
            out.push(Child { options, body, define: Vec::new() });
        }
    }

    fn grow_children(&mut self, parent: &str, children: Vec<Child>) -> Result<(), Error> {
        self.st.tree_level += 1;
        let level = self.st.tree_level;
        self.st.pending = Transform::default();
        self.process_keys(tikra_tokens(&format!("level/.try={level},level {level}/.try")), Some("/tikz"))?;
        self.st.apply_pending();
        let n = children.len();
        let Some(info) = self.nodes.get(parent).cloned() else { return Ok(()) };
        let anchor = self.st.growth_anchor.clone();
        let origin = info.anchor(&anchor).unwrap_or(info.center());
        let local = self.st.transform.inverse().apply(origin);
        self.st.transform = self.st.transform.compose(&Transform::translation(local.x, local.y));
        for (k, child) in children.into_iter().enumerate() {
            self.begin_group();
            let r = self.grow_child(parent, k + 1, n, child);
            self.end_group();
            r?;
        }
        Ok(())
    }

    fn grow_child(&mut self, parent: &str, k: usize, n: usize, child: Child) -> Result<(), Error> {
        self.tex.set_meaning(&Token::cs("tikzparentnode"), macro_from(parent), false);
        self.st.pending = Transform::default();
        self.st.child_missing = false;
        // the variable of `child foreach`, for the options too
        self.run_tokens(child.define)?;
        let mut options = tikra_tokens("every child/.try,");
        options.extend(child.options);
        self.process_keys(options, Some("/tikz"))?;
        self.st.apply_pending();
        if self.st.child_missing {
            return Ok(());
        }
        self.st.transform = self.grow(k, n)?;
        // the child's node, named parent-k unless it names itself, then its own children
        let mut body = keys_trimmed(&child.body);
        // `child {[level distance=5mm] child …}`: options of the rest of the child's path
        let mut path_options = Vec::new();
        if body.first().is_some_and(|t| t.is_char('[')) {
            if let Some(end) = body.iter().position(|t| t.is_char(']')) {
                path_options = body[..=end].to_vec();
                body = keys_trimmed(&body[end + 1..]);
            }
        }
        let (kind, rest) = split_first_word(&body);
        let mut statement = tikra_tokens("\\path");
        statement.extend(path_options);
        statement.extend(tikra_tokens(&format!(" {} [name={parent}-{k},every child node/.try]", if kind == "node" { "node" } else { "coordinate" })));
        if kind == "node" || kind == "coordinate" {
            statement.extend(rest);
        } else {
            statement.extend(body);
        }
        statement.extend(tikra_tokens(";"));
        // the parent's own `edge from parent[…]`, read before this child, waits for it to finish
        let outer = self.tree_edge_out.take();
        self.tree_child_names.push(None);
        let r = self.run_tokens(statement);
        let named = self.tree_child_names.pop().flatten().filter(|n| !n.is_empty());
        r?;
        let child_name = named.unwrap_or_else(|| format!("{parent}-{k}"));
        self.tex.set_meaning(&Token::cs("tikzchildnode"), macro_from(&child_name), false);
        // the edge from the parent, in the parent's coordinate system
        let (edge_options, edge_nodes) = std::mem::replace(&mut self.tree_edge_out, outer).unwrap_or_default();
        let pa = self.st.parent_anchor.clone();
        let ca = self.st.child_anchor.clone();
        self.tex.define_string("tikzparentanchor", &pa, false);
        self.tex.define_string("tikzchildanchor", &ca, false);
        let mut edge = tikra_tokens("\\path[edge from parent/.try,");
        edge.extend(edge_options);
        edge.extend(tikra_tokens("] "));
        match self.st.edge_from_parent_path.clone() {
            // a path of its own (edge from parent path={…}, the trees library's forks); labels after it
            Some(p) => {
                edge.extend(p);
                edge.extend(edge_nodes);
            }
            None => {
                edge.extend(tikra_tokens(&format!("({parent}{pa}) -- ")));
                edge.extend(edge_nodes);
                edge.extend(tikra_tokens(&format!(" ({child_name}{ca})")));
            }
        }
        edge.extend(tikra_tokens(";"));
        self.st.transform = Transform::default();
        self.run_tokens(edge)
    }
}

impl Engine {
    /// Where the `k`-th of `n` children goes: the growth function's transformation, applied to
    /// the parent's coordinate system.
    fn grow(&mut self, k: usize, n: usize) -> Result<Transform, Error> {
        let polar = |angle: f64, d: f64| {
            let (s, c) = angle.to_radians().sin_cos();
            Point::new(d * c, d * s)
        };
        let shift = |t: &Transform, p: Point| t.compose(&Transform::translation(p.x, p.y));
        let (g, ld, sd) = (self.st.grow_angle, self.st.level_distance, self.st.sibling_distance);
        let (k, n) = (k as f64, n as f64);
        let growth = self.tex.expand_to_string(vec![Token::cs("tikra@growth")])?;
        let sibling_angle = |e: &mut Engine| -> Result<f64, Error> {
            let v = crate::keys::value(&e.tex, "/tikz/sibling angle").unwrap_or_default();
            e.number_of(v)
        };
        let t = self.st.transform;
        if let Some(f) = self.st.growth_function.clone() {
            // a growth function of the options (`children are tokens`): it moves the coordinate system itself
            for (name, v) in [("tikznumberofchildren", n), ("tikznumberofcurrentchild", k)] {
                if let tex::Meaning::Count(r) = self.tex.meaning_of(name) {
                    self.tex.set_count(r, v as i64, false);
                }
            }
            self.run_tokens(f)?;
            return Ok(self.st.transform);
        }
        let (kind, arg) = growth.trim().split_once(' ').unwrap_or((growth.trim(), ""));
        Ok(match kind {
            // grow cyclic: turned by the sibling angle around the parent, then out by the level
            // distance; the turn stays for the child's own children
            "cyclic" => {
                let a = sibling_angle(self)? * (-0.5 - 0.5 * n + k);
                t.compose(&Transform::rotation(a)).compose(&Transform::translation(ld, 0.0))
            }
            "ccw" | "cw" => {
                let start = self.number(arg)?;
                let step = sibling_angle(self)? * (k - 1.0);
                shift(&t, polar(if kind == "ccw" { start + step } else { start - step }, ld))
            }
            // grow via three points: one child at a, the first of two at b, the second at c
            "three" => {
                let points: Vec<&str> = arg.split('|').collect();
                let mut p = Vec::new();
                for q in points.iter().take(3) {
                    let c = self.coordinate(q)?;
                    p.push(t.inverse().apply(c.p));
                }
                let [a, b, c] = p[..] else { return Ok(t) };
                let (b, c) = (b.minus(a), c.minus(b));
                shift(&t, Point::new(a.x + (n - 1.0) * b.x + (k - 1.0) * c.x, a.y + (n - 1.0) * b.y + (k - 1.0) * c.y))
            }
            // the default: level distance along the direction, then the place among the siblings
            _ => {
                let mut t = shift(&t, polar(g, ld));
                if self.st.special_level != Some(self.st.tree_level) {
                    let (left, right) = if self.st.grow_swapped { (g + 90.0, g - 90.0) } else { (g - 90.0, g + 90.0) };
                    t = shift(&shift(&t, polar(left, (n + 1.0) * sd / 2.0)), polar(right, k * sd));
                }
                t
            }
        })
    }
}

/// `\tikzleveldistance` and `\tikzsiblingdistance`, the registers TikZ keeps the distances in.
const LEVEL_DISTANCE: u32 = 4030;
const SIBLING_DISTANCE: u32 = 4031;

pub fn define(e: &mut Engine) {
    e.tex.define("tikzleveldistance", crate::tex::Meaning::Dimen(LEVEL_DISTANCE));
    e.tex.define("tikzsiblingdistance", crate::tex::Meaning::Dimen(SIBLING_DISTANCE));
    set_distance_registers(e);
}

/// The registers follow `level distance` and `sibling distance`.
pub fn set_distance_registers(e: &mut Engine) {
    let sp = |cm: f64| (cm / crate::PT * 65536.0).round() as i64;
    let (l, s) = (sp(e.st.level_distance), sp(e.st.sibling_distance));
    e.tex.set_dimen(LEVEL_DISTANCE, l, false);
    e.tex.set_dimen(SIBLING_DISTANCE, s, false);
}

/// The trees library: other growth functions (`\tikra@growth`), the forks.
pub const DEFINITIONS: &str = r"
\def\tikra@growth{}
\def\tikra@growthree one child at#1(#2)#3and two children at#4(#5)#6and#7(#8)\relax{\def\tikra@growth{three #2|#5|#8}}
\tikzset{
  grow cyclic/.style={edge from parent/.append style={reset cm},tikra@growth=cyclic},
  tikra@growth/.code={\def\tikra@growth{#1}},
  sibling angle/.initial=20,
  counterclockwise from/.code={\def\tikra@growth{ccw #1}},
  clockwise from/.code={\def\tikra@growth{cw #1}},
  grow via three points/.code={\tikra@growthree#1\relax},
  grow siblings on line/.code={\def\tikra@growth{}},
  edge from parent fork down/.style={edge from parent path={(\tikzparentnode\tikzparentanchor) -- +(0pt,-.5\tikzleveldistance) -| (\tikzchildnode\tikzchildanchor)}},
  edge from parent fork up/.style={edge from parent path={(\tikzparentnode\tikzparentanchor) -- +(0pt,.5\tikzleveldistance) -| (\tikzchildnode\tikzchildanchor)}},
  edge from parent fork left/.style={edge from parent path={(\tikzparentnode\tikzparentanchor) -- +(-.5\tikzleveldistance,0pt) |- (\tikzchildnode\tikzchildanchor)}},
  edge from parent fork right/.style={edge from parent path={(\tikzparentnode\tikzparentanchor) -- +(.5\tikzleveldistance,0pt) |- (\tikzchildnode\tikzchildanchor)}},
}
";

fn keys_trimmed(v: &[Token]) -> Vec<Token> {
    crate::keys::trim(v).to_vec()
}

/// `node[…] {…} child …` → (`node`, the rest).
fn split_first_word(v: &[Token]) -> (String, Vec<Token>) {
    let mut word = String::new();
    let mut i = 0;
    while i < v.len() {
        match &v[i] {
            Token::Char(c, Cat::Letter) => word.push(*c),
            _ => break,
        }
        i += 1;
    }
    (word, v[i..].to_vec())
}
