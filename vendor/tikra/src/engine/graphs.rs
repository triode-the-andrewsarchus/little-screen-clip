//! Graphs: `\graph [options] {a -> {b, c} -> d}`, as the `graphs` and `graphs.standard` libraries of
//! TikZ read and draw them. A graph is a group: chains of nodes and groups separated by commas or
//! semicolons, joined by `--`, `->`, `<-`, `<->` or `-!-`. Each node is a `\node` named after its
//! text and placed by the placement keys (`grow right`, `branch down`, `circular placement`…) from
//! its logical width and depth in the chains and groups around it; each join colors the nodes on
//! both sides (`source'`, `target'`) and runs an operator (`matching and star` by default,
//! `complete bipartite`, `clique`…) that makes the edges with `new ->` and friends, which are plain
//! `\path … edge …` statements.
//!
//! The parser follows `tikzlibrarygraphs.code.tex` step by step, with the same TeX groups at the
//! same places, so that the keys it keeps as pgfkeys values (the placement counters, `@operators`,
//! `@edges styling`…) are scoped as TikZ scopes them. What TikZ keeps in global macros (the color
//! classes of the nodes, the edge annotations, the edges of a `simple` graph) is kept in [`Graph`].
//! The keys are TikZ's own definitions where they are styles or values; the operators and the
//! placement arithmetic are done natively.

use std::collections::HashMap;

use crate::keys::{self, macro_name};
use crate::tex::{self, Cat, Meaning, Prim, Token};
use crate::{Error, PT};

use super::options::{split_arguments, KeyFn};
use super::{macro_from, macro_tokens, pgf_number, tikra_tokens, Engine, NATIVE_KEY_BASE};

/// The graphs being drawn (a `\graph` inside an operator's code nests) and what lives across them.
#[derive(Debug, Clone, Default)]
pub struct State {
    stack: Vec<Graph>,
    /// the node sets (`new set`, `set=`), with their nodes
    sets: HashMap<String, Vec<String>>,
    /// anonymous nodes (`/`, `""`) named so far: `tikz@f@N`
    anonymous: usize,
}

/// An edge waiting: its kind (`->`…), options and nodes.
type Edge = (String, Vec<Token>, Vec<Token>);

/// What TikZ keeps globally while it reads one graph.
#[derive(Debug, Clone, Default)]
struct Graph {
    /// the node list in force (`\tikz@lib@graph@node@list`): the operators work on it
    list: Vec<String>,
    /// the color classes of each node made so far in this graph (`\tikz@lgc@<name>`)
    colors: HashMap<String, Vec<String>>,
    /// `source edge style`/`node` (side `a`) and `target edge style`/`node` (side `b`) of a node
    annotations: HashMap<(char, String), (Vec<Token>, Vec<Token>)>,
    /// the edges of a `simple` graph, made when its group ends: (from, to) → kind, options, nodes
    simple: HashMap<(String, String), Edge>,
    /// `parse=` text, put before the group's content
    extras: Vec<Token>,
    /// the operators a node's options asked for, run once the node exists
    saved_ops: Vec<Token>,
    /// the nodes of a quick graph made so far
    quick: Vec<String>,
}

fn group(v: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(v.len() + 2);
    out.push(Token::Char('{', Cat::Begin));
    out.extend(v);
    out.push(Token::Char('}', Cat::End));
    out
}

fn text_group(s: &str) -> Vec<Token> {
    group(tex::string_tokens(s))
}

fn is_begin(t: Option<&Token>) -> bool {
    matches!(t, Some(Token::Char(_, Cat::Begin)))
}

fn trimmed(v: &[Token]) -> Vec<Token> {
    keys::trim(v).to_vec()
}

/// `{…}` around everything is taken off, as `\pgfkeys@spdef` does.
fn unbraced(v: &[Token]) -> Vec<Token> {
    tex::strip_outer_braces(trimmed(v))
}

/// A token list read element by element: braces, brackets and quotes keep what is inside whole.
struct Reader {
    t: Vec<Token>,
    i: usize,
}

impl Reader {
    fn new(t: Vec<Token>) -> Reader {
        Reader { t, i: 0 }
    }

    fn skip_spaces(&mut self) {
        while self.i < self.t.len() && (self.t[self.i].is_space() || self.t[self.i].is_cs("par")) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.t.get(self.i)
    }

    /// The next token after spaces, left in place (`\pgfutil@ifnextchar`).
    fn peek_char(&mut self, c: char) -> bool {
        self.skip_spaces();
        self.peek().is_some_and(|t| t.is_char(c))
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.t.get(self.i).cloned();
        if t.is_some() {
            self.i += 1;
        }
        t
    }

    /// At `{`: the group's body, the braces read.
    fn group(&mut self) -> Vec<Token> {
        let start = self.i + 1;
        let mut depth = 0usize;
        // the tokens of a graph are TeX's, whose braces pair up
        let end = (self.i..self.t.len())
            .find(|&j| {
                match self.t[j] {
                    Token::Char(_, Cat::Begin) => depth += 1,
                    Token::Char(_, Cat::End) => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .expect("a group's closing brace");
        self.i = end + 1;
        self.t[start..end].to_vec()
    }

    /// At `open`: what is up to the matching `close` (outside braces), both read.
    fn delimited(&mut self, open: char, close: char) -> Vec<Token> {
        let start = self.i + 1;
        let (mut braces, mut depth) = (0usize, 0usize);
        while self.i < self.t.len() {
            let t = &self.t[self.i];
            match t {
                Token::Char(_, Cat::Begin) => braces += 1,
                Token::Char(_, Cat::End) => braces = braces.saturating_sub(1),
                _ if braces == 0 && t.is_char(open) => depth += 1,
                _ if braces == 0 && t.is_char(close) => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return self.t[start..self.i - 1].to_vec();
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        self.t[start.min(self.t.len())..].to_vec()
    }

    fn rest(&mut self) -> Vec<Token> {
        let r = self.t[self.i.min(self.t.len())..].to_vec();
        self.i = self.t.len();
        r
    }
}

/// The positions in `v` where `at` holds, outside braces, brackets and quotes.
fn top_level(v: &[Token], mut at: impl FnMut(&[Token], usize) -> bool) -> Option<usize> {
    let (mut braces, mut brackets, mut quote) = (0usize, 0usize, false);
    for i in 0..v.len() {
        let t = &v[i];
        if braces == 0 && brackets == 0 && !quote && at(v, i) {
            return Some(i);
        }
        match t {
            Token::Char(_, Cat::Begin) => braces += 1,
            Token::Char(_, Cat::End) => braces = braces.saturating_sub(1),
            _ if braces > 0 => {}
            _ if t.is_char('"') => quote = !quote,
            _ if quote => {}
            _ if t.is_char('[') => brackets += 1,
            _ if t.is_char(']') => brackets = brackets.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn find_char(v: &[Token], c: char) -> Option<usize> {
    top_level(v, |v, i| v[i].is_char(c))
}

/// `v` cut at each top-level token for which `sep` holds.
fn split_top(v: &[Token], sep: impl Fn(&Token) -> bool) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut rest = v;
    while let Some(i) = top_level(rest, |v, i| sep(&v[i])) {
        out.push(rest[..i].to_vec());
        rest = &rest[i + 1..];
    }
    out.push(rest.to_vec());
    out
}

/// `"text" options`, `"text"' options` or `"text"{options}`: the text and the options (with `'`
/// first when it follows the quotes), as the quotes library reads them.
fn quoted(item: &[Token]) -> Option<(Vec<Token>, Vec<Token>)> {
    let v = trimmed(item);
    if !v.first().is_some_and(|t| t.is_char('"')) {
        return None;
    }
    let end = v[1..].iter().position(|t| t.is_char('"'))? + 1;
    let text = tex::strip_outer_braces(v[1..end].to_vec());
    let mut r = Reader::new(v[end + 1..].to_vec());
    let mut options = Vec::new();
    if r.peek_char('\'') {
        r.next();
        options.extend(tikra_tokens("',"));
    }
    r.skip_spaces();
    if is_begin(r.peek()) {
        options.extend(r.group());
    } else {
        options.extend(r.rest());
    }
    Some((text, options))
}

/// A list of options with `"text"` items as `edge node`s (`\tikz@enable@edge@quotes`).
fn edge_quotes(options: &[Token]) -> Vec<Token> {
    let mut out = Vec::new();
    for item in split_top(options, |t| t.is_char(',')) {
        if !out.is_empty() {
            out.push(Token::other(','));
        }
        match quoted(&item) {
            Some((text, opts)) => {
                let mut node = tikra_tokens("node[every edge quotes,");
                node.extend(opts);
                node.push(Token::other(']'));
                node.extend(group(text));
                out.extend(tikra_tokens("edge node="));
                out.extend(group(node));
            }
            None => out.extend(item),
        }
    }
    out
}

/// The options of a node with the source and target edge syntax: `>red` is `target edge style=red`,
/// `<"x"` is `source edge node={node[every edge quotes]{x}}` (`\tikz@lib@activate@source@target@edge@syntax`).
fn edge_syntax(options: &[Token]) -> Vec<Token> {
    let mut out = Vec::new();
    for item in split_top(options, |t| t.is_char(',')) {
        if !out.is_empty() {
            out.push(Token::other(','));
        }
        let v = trimmed(&item);
        let side = match v.first() {
            Some(t) if t.is_char('>') => "target",
            Some(t) if t.is_char('<') => "source",
            _ => {
                out.extend(item);
                continue;
            }
        };
        let rest = trimmed(&v[1..]);
        match quoted(&rest) {
            Some((text, opts)) => {
                let mut node = tikra_tokens("node[every edge quotes,");
                node.extend(opts);
                node.push(Token::other(']'));
                node.extend(group(text));
                out.extend(tikra_tokens(&format!("{side} edge node=")));
                out.extend(group(node));
            }
            None => {
                out.extend(tikra_tokens(&format!("{side} edge style=")));
                out.extend(group(rest));
            }
        }
    }
    out
}

/// A quoted node's name: its text with the characters that cannot be in a name spelled out, the way
/// the graphs library makes them active and defines each as its Unicode name.
fn quoted_name(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let name = match c {
            '$' => "DOLLAR SIGN",
            '&' => "AMPERSAND",
            '^' => "CIRCUMFLEX ACCENT",
            '~' => "TILDE",
            '_' => "LOW LINE",
            '|' => "VERTICAL LINE",
            '[' => "LEFT SQUARE BRACKET",
            ']' => "RIGHT SQUARE BRACKET",
            '(' => "LEFT PARENTHESIS",
            ')' => "RIGHT PARENTHESIS",
            '/' => "SOLIDUS",
            '.' => "FULL STOP",
            '-' => "HYPHEN MINUS",
            ',' => "COMMA",
            '+' => "PLUS SIGN",
            '*' => "ASTERISK",
            '\'' => "APOSTROPHE",
            '!' => "EXCLAMATION MARK",
            '"' => "QUOTATION MARK",
            ':' => "COLON",
            ';' => "SEMINCOLON",
            '<' => "LESS THAN SIGN",
            '=' => "EQUALS SIGN",
            '>' => "GREATER THAN SIGN",
            '?' => "QUESTION MARK",
            '`' => "GRAVE ACCENT",
            '%' => "PERCENT SIGN",
            '#' => "NUMBER SIGN",
            '\\' => "REVERSE SOLIDUS",
            '@' => "COMMERCIAL AT",
            '{' => "LEFT CURLY BRACE",
            '}' => "RIGHT CURLY BRACE",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push('@');
        out.push_str(name);
        out.push('@');
    }
    out
}

/// The six counters a `\foreach` inside a graph carries from one iteration to the next.
const LOOP_COUNTERS: [&str; 6] = ["local depth", "local width", "chain count", "element count", "width", "depth"];

impl Engine {
    // ------------------------------------------------------------ state

    fn graph_state(&mut self) -> Option<&mut Graph> {
        self.graphs.stack.last_mut()
    }

    fn list(&mut self) -> Vec<String> {
        self.graph_state().map(|g| g.list.clone()).unwrap_or_default()
    }

    fn take_list(&mut self) -> Vec<String> {
        self.graph_state().map(|g| std::mem::take(&mut g.list)).unwrap_or_default()
    }

    fn set_list(&mut self, list: Vec<String>) {
        if let Some(g) = self.graph_state() {
            g.list = list;
        }
    }

    /// `\pgfkeysvalueof{/tikz/graphs/<key>}`.
    fn gvalue(&self, key: &str) -> Vec<Token> {
        keys::value(&self.tex, &format!("/tikz/graphs/{key}")).unwrap_or_default()
    }

    fn gvalue_string(&mut self, key: &str) -> String {
        let v = self.gvalue(key);
        self.tex.expand_to_string(v).unwrap_or_default().trim().to_string()
    }

    fn set_gvalue(&mut self, key: &str, v: Vec<Token>) {
        self.tex.set_meaning(&Token::cs(&macro_name(&format!("/tikz/graphs/{key}"))), macro_tokens(v), false);
    }

    /// A placement counter (`/tikz/graphs/placement/width`…).
    fn place(&mut self, key: &str) -> f64 {
        let s = self.gvalue_string(&format!("placement/{key}"));
        s.parse().or_else(|_| self.number(&s)).unwrap_or(0.0)
    }

    fn set_place(&mut self, key: &str, v: f64) {
        self.set_gvalue(&format!("placement/{key}"), tex::string_tokens(&pgf_number(v)));
    }

    fn tex_if(&self, name: &str) -> bool {
        matches!(self.tex.meaning_of(name), Meaning::Prim(Prim::Iftrue))
    }

    fn macro_string(&mut self, name: &str) -> String {
        if !self.tex.is_defined(name) {
            return String::new();
        }
        self.tex.expand_to_string(vec![Token::cs(name)]).unwrap_or_default()
    }

    /// Keys in `/tikz/graphs` (`\tikzgraphsset`).
    fn gkeys(&mut self, list: Vec<Token>) -> Result<(), Error> {
        self.process_keys(list, Some("/tikz/graphs"))
    }

    /// `/tikz/graphs/.unknown` made the native key `key` for the current group.
    fn set_unknown(&mut self, key: &str) {
        let m = self.tex.meaning_of(&macro_name(&format!("/tikz/graphs/{key}/.@cmd")));
        self.tex.set_meaning(&Token::cs(&macro_name("/tikz/graphs/.unknown/.@cmd")), m, false);
    }

    /// `\tikzgraphinvokeoperator{keys}`: the operators the keys ask for, run now.
    fn invoke_operator(&mut self, list: &str) -> Result<(), Error> {
        self.invoke_operator_tokens(tikra_tokens(list))
    }

    fn invoke_operator_tokens(&mut self, list: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        self.set_gvalue("@pending operators", Vec::new());
        let r = self.gkeys(list);
        let ops = self.gvalue("@pending operators");
        self.end_group();
        r?;
        self.run_tokens(ops)
    }

    fn run_operators(&mut self) -> Result<(), Error> {
        let ops = self.gvalue("@pending operators");
        self.run_tokens(ops)
    }

    fn is_local(&mut self, name: &str) -> bool {
        self.graph_state().is_some_and(|g| g.colors.contains_key(name))
    }

    fn has_color(&mut self, name: &str, color: &str) -> bool {
        self.graph_state().is_some_and(|g| g.colors.get(name).is_some_and(|c| c.iter().any(|x| x == color)))
    }

    /// The node list without repetitions (`\tikz@lib@graph@pack@node@list`), restricted to a color.
    fn colored(&mut self, color: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in self.list() {
            if !out.contains(&n) && self.has_color(&n, color) {
                out.push(n);
            }
        }
        out
    }

    // ------------------------------------------------------------ the parser

    /// `graph [options] {…}` on a path, the word `graph` already read.
    pub fn graph(&mut self) -> Result<(), Error> {
        let mut options = Vec::new();
        match self.tex.next_nonspace() {
            Some(t) if t.is_char('[') => options = self.tex.read_until_char(']', false)?,
            Some(t) => self.tex.back(t),
            None => {}
        }
        let body = match self.tex.next_nonspace() {
            Some(Token::Char(_, Cat::Begin)) => self.tex.read_group_body()?,
            Some(t) => {
                self.tex.back(t);
                return Err(Error::Figure("graph without '{'".into()));
            }
            None => return Err(Error::Figure("graph without '{'".into())),
        };
        self.graphs.stack.push(Graph::default());
        self.begin_group();
        let mut o = tikra_tokens("graphs/.cd,redirect unknown to tikz,@graph drawing setup/.try,@pending operators=,every graph/.try,");
        o.extend(options);
        let r = self.open_tikz_scope(Some(o), false).and_then(|_| {
            self.begin_group();
            let r = if self.tex_if("iftikra@gr@quick") { self.quick_graph(body) } else { self.graph_main(body) };
            self.end_group();
            r
        });
        let closed = self.close_tikz_scope();
        self.end_group();
        self.graphs.stack.pop();
        r.and(closed)
    }

    /// `\tikz@lib@graphs@normal@main`: the outermost group, then the graph's own operators.
    fn graph_main(&mut self, body: Vec<Token>) -> Result<(), Error> {
        let outer = self.gvalue("@pending operators");
        self.set_list(Vec::new());
        for key in ["depth", "width", "level"] {
            self.set_place(key, 0.0);
        }
        self.start_hint_group();
        self.parse_group(body)?;
        self.run_tokens(outer)
    }

    fn start_hint_group(&mut self) {
        for key in ["local depth", "local width", "chain count", "element count"] {
            self.set_place(key, 0.0);
        }
    }

    /// The depth and width of the group about to end (`\tikz@lib@graph@end@hint@group`): none when
    /// a placement started again in it, whose `\aftergroup\tikz@lib@graph@reset@locals` clears them.
    fn group_size(&mut self) -> (f64, f64) {
        if self.macro_string("tikra@lg@placed") == self.tex.group_depth().to_string() {
            return (0.0, 0.0);
        }
        (self.place("local depth"), self.place("local width"))
    }

    /// After a group (`\tikz@lib@graph@hint@aftergroup`): it counts as one element of width `w`.
    fn hint_aftergroup(&mut self, depth: f64, width: f64) {
        let (w, lw, ld, n) = (self.place("width"), self.place("local width"), self.place("local depth"), self.place("element count"));
        self.set_place("width", width + w);
        self.set_place("local width", width + lw);
        self.set_place("local depth", depth.max(ld));
        self.set_place("element count", n + 1.0);
    }

    /// After a chain (`\tikz@lib@graph@placement@after@chain@update`): the group gets deeper.
    fn after_chain_update(&mut self, depth: f64, width: f64) {
        let (d, lw, ld, n) = (self.place("depth"), self.place("local width"), self.place("local depth"), self.place("chain count"));
        self.set_place("depth", depth + d);
        self.set_place("local depth", depth + ld);
        self.set_place("local width", width.max(lw));
        self.set_place("chain count", n + 1.0);
    }

    /// `\tikz@lib@graph@parse@group`: `[options]` first, then the chains, then the group's operators.
    fn parse_group(&mut self, content: Vec<Token>) -> Result<(), Error> {
        let mut r = Reader::new(content);
        let mut options = Vec::new();
        if r.peek_char('[') {
            options = r.delimited('[', ']');
        }
        if let Some(g) = self.graph_state() {
            g.extras.clear();
        }
        let mut o = tikra_tokens("@pending operators=,every group/.try,@extra group options,@extra group options/.style={},");
        o.extend(options);
        self.gkeys(o)?;
        let mut content = self.graph_state().map(|g| std::mem::take(&mut g.extras)).unwrap_or_default();
        content.extend(r.rest());
        // \par goes, ; is ,
        let content: Vec<Token> = content.into_iter().filter(|t| !t.is_cs("par")).collect();
        for chain in split_top(&content, |t| t.is_char(',') || t.is_char(';')) {
            self.chain(chain)?;
        }
        self.run_operators()
    }

    /// One chain, in its own group: its depth and width go to the group around it.
    fn chain(&mut self, item: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        self.set_place("local depth", 0.0);
        self.set_place("local width", 0.0);
        let outer = self.take_list();
        let r = self.chain_elements(item);
        let (depth, width) = (self.place("local depth"), self.place("local width"));
        let inner = self.take_list();
        self.end_group();
        self.set_list([outer, inner].concat());
        r?;
        self.after_chain_update(depth, width);
        Ok(())
    }

    /// The elements of a chain and the joins between them.
    fn chain_elements(&mut self, item: Vec<Token>) -> Result<(), Error> {
        let mut r = Reader::new(item);
        let mut join: Option<(String, Vec<Token>)> = None;
        loop {
            r.skip_spaces();
            let kind = if is_begin(r.peek()) {
                let body = r.group();
                self.group_element(body)?;
                if let Some((k, o)) = join.clone() {
                    self.graph_join(&k, o)?;
                }
                r.skip_spaces();
                match r.next() {
                    None => return Ok(()),
                    Some(t) if t.is_char('-') => arrow_after_dash(&mut r),
                    Some(t) if t.is_char('<') => {
                        if r.peek_char('-') {
                            r.next();
                        }
                        back_arrow(&mut r)
                    }
                    Some(t) => {
                        self.warn(format!("graph: one of the arrow types <-, --, ->, -!- or <-> expected, not '{}'", tex::show_token(&t)));
                        return Ok(());
                    }
                }
            } else {
                let (text, dash) = self.read_element(&mut r);
                let (text, back) = match find_char(&text, '<') {
                    Some(i) => (text[..i].to_vec(), true),
                    None => (text, false),
                };
                self.node_element(text)?;
                if let Some((k, o)) = join.clone() {
                    self.graph_join(&k, o)?;
                }
                if !dash {
                    return Ok(());
                }
                if back {
                    back_arrow(&mut r)
                } else {
                    arrow_after_dash(&mut r)
                }
            };
            let options = if r.peek_char('[') { r.delimited('[', ']') } else { Vec::new() };
            self.invoke_operator("recolor source by=source'',recolor target by=target'")?;
            join = Some((kind, options));
        }
    }

    /// A node of a chain, up to the `-` that starts the next arrow (whether there is one is the
    /// second value). A quoted node becomes `name/{text}`; `(name)` is kept whole.
    fn read_element(&mut self, r: &mut Reader) -> (Vec<Token>, bool) {
        r.skip_spaces();
        let mut out = Vec::new();
        if r.peek().is_some_and(|t| t.is_char('"')) {
            // "text", "" for a ", then /text or the text itself
            let mut text = Vec::new();
            while r.peek().is_some_and(|t| t.is_char('"')) {
                r.next();
                if !text.is_empty() {
                    text.push(Token::other('"'));
                }
                while let Some(t) = r.next() {
                    if t.is_char('"') {
                        break;
                    }
                    text.push(t);
                }
                r.skip_spaces();
            }
            let text = tex::strip_outer_braces(text);
            out.extend(tex::string_tokens(&quoted_name(&tex::tokens_to_string(&text))));
            if r.peek_char('/') {
                r.next();
                out.push(Token::other('/'));
            } else {
                out.push(Token::other('/'));
                out.extend(group(text));
            }
        } else if r.peek().is_some_and(|t| t.is_char('(')) {
            let inner = r.delimited('(', ')');
            let mut v = vec![Token::other('(')];
            v.extend(inner);
            v.push(Token::other(')'));
            out.extend(group(v));
        }
        let rest = r.t[r.i..].to_vec();
        match find_char(&rest, '-') {
            Some(i) => {
                out.extend(rest[..i].iter().cloned());
                r.i += i + 1;
                (out, true)
            }
            None => {
                out.extend(rest);
                r.i = r.t.len();
                (out, false)
            }
        }
    }

    /// `{…}` in a chain (`\tikz@lib@graph@scope`): a group, in its own TeX group.
    fn group_element(&mut self, body: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        let outer = self.take_list();
        self.start_hint_group();
        let r = self.parse_group(body);
        let (depth, width) = self.group_size();
        let inner = self.take_list();
        self.end_group();
        self.set_list([outer, inner].concat());
        r?;
        self.hint_aftergroup(depth, width);
        Ok(())
    }

    /// The join of two elements (`\tikz@lib@graph@joiner`): the nodes on the left are `target'`, the
    /// new ones `source'`; the operator of the options (or `default edge operator`) joins them.
    fn graph_join(&mut self, kind: &str, options: Vec<Token>) -> Result<(), Error> {
        self.invoke_operator("recolor source by=source',recolor source'' by=source")?;
        self.begin_group();
        self.set_gvalue("default edge kind", tex::string_tokens(kind));
        self.set_gvalue("@pending operators", Vec::new());
        self.set_unknown("@unknown edge option");
        let r = (|| {
            self.gkeys(edge_quotes(&options))?;
            if keys::trim(&self.gvalue("@pending operators")).is_empty() {
                let default = self.gvalue("default edge operator");
                self.gkeys(default)?;
            }
            self.run_operators()
        })();
        self.end_group();
        r?;
        self.invoke_operator("not source',not target'")
    }

    /// A node, a `\foreach`, a list of existing nodes `(a,b)` or a declared graph.
    fn node_element(&mut self, text: Vec<Token>) -> Result<(), Error> {
        let mut r = Reader::new(text);
        r.skip_spaces();
        if r.peek().is_some_and(|t| t.is_cs("foreach")) {
            r.next();
            return self.graph_foreach(r.rest());
        }
        let text = r.rest();
        if top_level(&text, |v, i| v[i].is_char('/') && v.get(i + 1).is_some_and(|t| t.is_char('/'))).is_some() {
            self.warn("graph: sublayouts (//) need the graphdrawing library: ignored");
            return Ok(());
        }
        // name/text [options]
        let (name_part, options) = match find_char(&text, '[') {
            Some(i) => {
                let mut r = Reader::new(text[i..].to_vec());
                (text[..i].to_vec(), Some(r.delimited('[', ']')))
            }
            None => (text, None),
        };
        let slash =
            find_char(&name_part, '/').map(|i| (i, 1)).or_else(|| top_level(&name_part, |v, i| v[i].is_char('_') && v.get(i + 1).is_some_and(|t| t.is_char('_'))).map(|i| (i, 2)));
        let (mut name, node_text) = match slash {
            Some((i, n)) => {
                let raw = self.tex.expand_fully(unbraced(&name_part[..i]))?;
                let mut name = tex::tokens_to_string(&raw).trim().to_string();
                if name.is_empty() {
                    self.graphs.anonymous += 1;
                    name = format!("tikz@f@{}", self.graphs.anonymous);
                }
                (name, trimmed(&name_part[i + n..]))
            }
            None => {
                let raw = self.tex.expand_fully(unbraced(&name_part))?;
                let raw = trimmed(&raw);
                (tex::tokens_to_string(&raw).trim().to_string(), raw)
            }
        };
        let path = self.macro_string("tikra@gr@path");
        if self.tex_if("iftikra@gr@numbered") && !name.is_empty() {
            let sep = self.macro_string("tikra@gr@autosep");
            if let Meaning::Count(c) = self.tex.meaning_of("tikra@gr@autonum") {
                let n = self.tex.counts.get(&c).copied().unwrap_or(0);
                name = format!("{name}{sep}{n}");
                self.tex.set_count(c, n + 1, true);
            }
        }
        if self.tex_if("iftikra@gr@fresh@node") {
            while self.is_local(&format!("{path}{name}")) {
                name.push('\'');
            }
        }
        self.tex.set_meaning(&Token::cs("tikzgraphnodeas"), macro_tokens(vec![Token::cs("tikra@gr@typesetter")]), false);
        if name.is_empty() && options.is_none() {
            return Ok(());
        }
        self.graph_node(&name, &path, node_text, options.unwrap_or_default())
    }

    /// `\tikz@lib@graph@node@opt@normal`: the node is made (or, when it exists, colored again).
    fn graph_node(&mut self, name: &str, path: &str, text: Vec<Token>, options: Vec<Token>) -> Result<(), Error> {
        // (a, b, set): nodes made outside the graph
        if let Some(inner) = name.strip_prefix('(').and_then(|n| n.strip_suffix(')')) {
            let mut used = Vec::new();
            for n in inner.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                match self.graphs.sets.get(n) {
                    Some(members) => used.extend(members.iter().cloned()),
                    None => used.push(n.to_string()),
                }
            }
            let g = self.graph_state().expect("a graph's nodes are read inside it");
            for n in &used {
                g.colors.insert(n.clone(), vec!["all".into(), "source".into(), "target".into()]);
            }
            g.list.extend(used);
            return Ok(());
        }
        let full = format!("{path}{name}");
        self.tex.define_string("tikra@gr@name", &full, false);
        if let Meaning::Macro(m) = self.tex.meaning_of(&format!("tikra@gr@def@{name}")) {
            let body = tex::substitute(&m.body, &[]);
            return self.declared_graph(body, options);
        }
        let level = self.place("level") + 1.0;
        self.set_place("level", level);
        let level = pgf_number(level);
        self.gkeys(tikra_tokens(&format!("level/.try={level},level {level}/.try")))?;
        let options = edge_syntax(&options);
        self.begin_group();
        let outer = self.take_list();
        self.set_list(vec![full.clone()]);
        self.set_gvalue("@pending operators", Vec::new());
        let r = self.make_graph_node(name, path, &full, text, options);
        self.set_list(outer);
        self.end_group();
        let created = r?;
        if created {
            self.placement_update(&full)?;
        }
        if let Some(g) = self.graph_state() {
            g.list.push(full.clone());
        }
        if self.tex_if("iftikra@gr@trie") {
            let mut k = tikra_tokens("name=");
            k.extend(text_group(name));
            self.gkeys(k)?;
        }
        Ok(())
    }

    /// The node itself, in the node's group: whether a node was made.
    fn make_graph_node(&mut self, name: &str, path: &str, full: &str, text: Vec<Token>, options: Vec<Token>) -> Result<bool, Error> {
        if self.is_local(full) {
            // late options and operators for a node already there (\tikz@lg@local@node@handle)
            self.set_unknown("@ignore");
            let mut o = tikra_tokens("source,target,");
            o.extend(options.clone());
            self.gkeys(o)?;
            self.begin_group();
            self.set_unknown("@redirect");
            let r = self.gkeys(options);
            self.end_group();
            r?;
            self.run_operators()?;
            return Ok(false);
        }
        if let Some(g) = self.graph_state() {
            g.colors.insert(full.to_string(), vec!["all".into(), "source".into(), "target".into()]);
        }
        if self.tex_if("iftikra@gr@all") {
            // use existing nodes: only the coloring
            self.set_unknown("@ignore");
            self.gkeys(options)?;
            self.run_operators()?;
            return Ok(false);
        }
        self.gkeys(tikra_tokens("placement/place"))?;
        self.tex.set_meaning(&Token::cs("tikzgraphnodetext"), macro_tokens(text), false);
        self.tex.define_string("tikzgraphnodename", name, false);
        self.tex.define_string("tikzgraphnodepath", path, false);
        self.tex.define_string("tikzgraphnodefullname", full, false);
        if let Some(g) = self.graph_state() {
            g.saved_ops.clear();
        }
        let mut node = tikra_tokens("\\node[name=");
        node.extend(text_group(full));
        node.extend(tikra_tokens(",graphs/redirect unknown to tikz,/tikz/graphs/.cd,/tikz/graphs/@node options,"));
        node.extend(options);
        node.extend(tikra_tokens(",/tikz/graphs/@save operators]{\\tikzgraphnodeas};"));
        self.run_tokens(node)?;
        let ops = self.graph_state().map(|g| std::mem::take(&mut g.saved_ops)).unwrap_or_default();
        self.run_tokens(ops)?;
        Ok(true)
    }

    /// A node adds its logical width to the chain and its depth to the chain's depth.
    fn placement_update(&mut self, full: &str) -> Result<(), Error> {
        let mut size = [0.0; 2];
        for (k, key) in ["placement/logical node depth=", "placement/logical node width="].iter().enumerate() {
            let mut o = tikra_tokens(key);
            o.extend(text_group(full));
            self.gkeys(o)?;
            let s = self.macro_string("pgfmathresult");
            size[k] = s.trim().parse().or_else(|_| self.number(&s)).unwrap_or(1.0);
        }
        let [depth, width] = size;
        let (w, lw, ld, n) = (self.place("width"), self.place("local width"), self.place("local depth"), self.place("element count"));
        self.set_place("width", width + w);
        self.set_place("local width", width + lw);
        self.set_place("local depth", depth.max(ld));
        self.set_place("element count", n + 1.0);
        Ok(())
    }

    /// A node that names a declared graph (`declare={name}{…}`): the declaration read as a group,
    /// with the node's options before its own.
    fn declared_graph(&mut self, body: Vec<Token>, options: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        let outer = self.take_list();
        let mut o = tikra_tokens("@extra group options/.style=");
        o.extend(group(options));
        let r = self.gkeys(o).and_then(|_| {
            self.start_hint_group();
            self.parse_group(body)
        });
        let (depth, width) = self.group_size();
        let inner = self.take_list();
        self.end_group();
        self.set_list([outer, inner].concat());
        r?;
        self.hint_aftergroup(depth, width);
        Ok(())
    }

    /// `\foreach \x in {list} {group}` among the nodes of a chain: the group read once per item, the
    /// placement counters going on from one iteration to the next.
    fn graph_foreach(&mut self, rest: Vec<Token>) -> Result<(), Error> {
        let mut r = Reader::new(rest);
        let mut vars = Vec::new();
        loop {
            let Some(t) = r.next() else { return Err(Error::Figure("graph: \\foreach without 'in'".into())) };
            if t.is_char('i') && r.peek().is_some_and(|n| n.is_char('n')) {
                r.next();
                break;
            }
            vars.push(t);
        }
        r.skip_spaces();
        let list = if is_begin(r.peek()) { r.group() } else { r.next().into_iter().collect() };
        let list = self.tex.expand_fully(list)?;
        let body = r.rest();
        let names: Vec<Token> = vars.into_iter().filter(|t| matches!(t, Token::Cs(_))).collect();
        self.begin_group();
        let outer = self.take_list();
        let mut saved: Vec<f64> = LOOP_COUNTERS.iter().map(|k| self.place(k)).collect();
        let mut nodes = Vec::new();
        let mut result = Ok(());
        for item in super::foreach_items(&list) {
            self.count_step()?;
            self.begin_group();
            let parts = super::split_slash(&item);
            for (i, name) in names.iter().enumerate() {
                let v = parts.get(i).or(parts.last()).cloned().unwrap_or_default();
                self.tex.set_meaning(name, macro_tokens(v), false);
            }
            self.set_list(std::mem::take(&mut nodes));
            for (k, v) in LOOP_COUNTERS.iter().zip(&saved) {
                self.set_place(k, *v);
            }
            result = self.parse_group(body.clone());
            saved = LOOP_COUNTERS.iter().map(|k| self.place(k)).collect();
            nodes = self.take_list();
            self.end_group();
            if result.is_err() {
                break;
            }
        }
        self.end_group();
        self.set_list([outer, nodes].concat());
        for (k, v) in LOOP_COUNTERS.iter().zip(&saved) {
            self.set_place(k, *v);
        }
        result
    }

    // ------------------------------------------------------------ quick graphs

    /// `graph [quick] {…}`: quoted nodes, connectors and groups with options, nothing else.
    fn quick_graph(&mut self, body: Vec<Token>) -> Result<(), Error> {
        self.tex.define_string("tikzgraphnodepath", "", false);
        self.quick_group(body)
    }

    fn quick_group(&mut self, body: Vec<Token>) -> Result<(), Error> {
        let mut r = Reader::new(body);
        let mut prev: Option<(String, String, Vec<Token>)> = None;
        loop {
            r.skip_spaces();
            let Some(t) = r.peek().cloned() else { return Ok(()) };
            if is_begin(Some(&t)) {
                let inner = r.group();
                let mut g = Reader::new(inner);
                let options = if g.peek_char('[') { g.delimited('[', ']') } else { Vec::new() };
                self.begin_group();
                let res = self.gkeys(options).and_then(|_| self.quick_group(g.rest()));
                self.end_group();
                res?;
                if r.peek_char(';') || r.peek_char(',') {
                    r.next();
                }
                prev = None;
                continue;
            }
            if !t.is_char('"') {
                r.next();
                if !(t.is_char(',') || t.is_char(';')) {
                    self.warn(format!("graph [quick]: unexpected '{}'", tex::show_token(&t)));
                }
                prev = None;
                continue;
            }
            // "name"/"text"[options]
            let name = quick_quoted(&mut r);
            let mut text = name.clone();
            if r.peek_char('/') {
                r.next();
                r.skip_spaces();
                text = quick_quoted(&mut r);
            }
            let options = if r.peek_char('[') { r.delimited('[', ']') } else { Vec::new() };
            let mut name = tex::tokens_to_string(&name);
            if self.tex_if("iftikra@gr@numbered") {
                let sep = self.macro_string("tikra@gr@autosep");
                if let Meaning::Count(c) = self.tex.meaning_of("tikra@gr@autonum") {
                    let n = self.tex.counts.get(&c).copied().unwrap_or(0);
                    name = format!("{name}{sep}{n}");
                    self.tex.set_count(c, n + 1, true);
                }
            }
            let known = self.graph_state().is_some_and(|g| g.quick.contains(&name));
            if !known {
                if let Some(g) = self.graph_state() {
                    g.quick.push(name.clone());
                }
                self.begin_group();
                self.tex.set_meaning(&Token::cs("tikzgraphnodetext"), macro_tokens(text), false);
                self.tex.define_string("tikzgraphnodename", &name, false);
                self.tex.define_string("tikzgraphnodefullname", &name, false);
                self.tex.set_meaning(&Token::cs("tikzgraphnodeas"), macro_tokens(vec![Token::cs("tikra@gr@typesetter")]), false);
                let mut node = tikra_tokens("\\node[name=");
                node.extend(text_group(&name));
                node.extend(tikra_tokens(",graphs/redirect unknown to tikz,/tikz/graphs/.cd,/tikz/graphs/@node options,"));
                node.extend(options);
                node.extend(tikra_tokens("]{\\tikzgraphnodeas};"));
                let res = self.run_tokens(node);
                self.end_group();
                res?;
            }
            if let Some((from, kind, options)) = prev.take() {
                self.begin_group();
                self.set_unknown("@unknown edge option");
                let res = self.gkeys(edge_quotes(&options)).and_then(|_| {
                    let (styling, nodes) = (self.gvalue("@edge options"), self.gvalue("@edge texts"));
                    self.new_edge(&kind, &from, &name, styling, nodes)
                });
                self.end_group();
                res?;
            }
            // what follows: , ; } or a connector
            r.skip_spaces();
            match r.peek() {
                None => return Ok(()),
                Some(t) if t.is_char(',') || t.is_char(';') => {
                    r.next();
                }
                Some(_) => {
                    let mut kind = String::new();
                    for _ in 0..2 {
                        if let Some(Token::Char(c, _)) = r.next() {
                            kind.push(c);
                        }
                    }
                    if r.peek().is_some_and(|t| t.is_char('>')) {
                        r.next();
                        kind.push('>');
                    }
                    let options = if r.peek_char('[') { r.delimited('[', ']') } else { Vec::new() };
                    prev = Some((name, kind, options));
                }
            }
        }
    }

    // ------------------------------------------------------------ edges and operators

    /// `\tikzgraphsset{new <kind>={from}{to}{options}{nodes}}`.
    fn new_edge(&mut self, kind: &str, from: &str, to: &str, options: Vec<Token>, nodes: Vec<Token>) -> Result<(), Error> {
        let mut k = tikra_tokens(&format!("new {kind}="));
        k.extend(text_group(from));
        k.extend(text_group(to));
        k.extend(group(options));
        k.extend(group(nodes));
        self.gkeys(k)
    }

    /// `\tikz@lib@graph@default@new@edge`: an edge of the default kind, with the edge styling in force
    /// and the annotations of its two nodes; in a `simple` graph it waits for the group's end.
    fn default_new_edge(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let mut styling = self.gvalue("@edge options");
        let mut nodes = self.gvalue("@edge texts");
        let mut annotated = false;
        for (side, n) in [('a', from), ('b', to)] {
            if let Some((s, nd)) = self.graph_state().and_then(|g| g.annotations.get(&(side, n.to_string())).cloned()) {
                styling.push(Token::other(','));
                styling.extend(s);
                nodes.extend(nd);
                annotated = true;
            }
        }
        if annotated {
            styling.extend(tikra_tokens(",after source and target edge/.try"));
        }
        let kind = self.gvalue_string("default edge kind");
        if self.tex_if("iftikra@gr@simple") {
            if let Some(g) = self.graph_state() {
                g.simple.remove(&(to.to_string(), from.to_string()));
                g.simple.insert((from.to_string(), to.to_string()), (kind, styling, nodes));
            }
            return Ok(());
        }
        self.new_edge(&kind, from, to, styling, nodes)
    }

    /// The operators: `@op={name}{arguments}`.
    fn graph_operator(&mut self, name: &str, args: Vec<Token>) -> Result<(), Error> {
        let arg = |e: &mut Engine, v: &[Token]| e.tex.expand_to_string(v.to_vec()).map(|s| s.trim().to_string());
        let two = split_arguments(&args, 2);
        match name {
            "colorize" | "recolor" => {
                let (old, new) = if name == "colorize" { (String::new(), arg(self, &args)?) } else { (arg(self, &two[0])?, arg(self, &two[1])?) };
                let list = self.list();
                if let Some(g) = self.graph_state() {
                    for n in list {
                        let Some(c) = g.colors.get_mut(&n) else { continue };
                        if name == "colorize" {
                            c.insert(0, new.clone());
                        } else {
                            let mut out: Vec<String> = Vec::new();
                            for x in c.drain(..) {
                                if x == old {
                                    if !new.is_empty() {
                                        out.push(new.clone());
                                    }
                                } else {
                                    out.push(x);
                                }
                            }
                            *c = out;
                        }
                    }
                }
            }
            "simple" => {
                let mut packed: Vec<String> = Vec::new();
                for n in self.list() {
                    if !packed.contains(&n) {
                        packed.push(n);
                    }
                }
                for from in &packed {
                    for to in &packed {
                        let edge = self.graph_state().and_then(|g| g.simple.remove(&(from.clone(), to.clone())));
                        if let Some((kind, options, nodes)) = edge {
                            self.new_edge(&kind, from, to, options, nodes)?;
                        }
                    }
                }
            }
            "complete bipartite" => {
                let (a, b) = (arg(self, &two[0])?, arg(self, &two[1])?);
                for x in self.colored(&a) {
                    for y in self.colored(&b) {
                        if x != y {
                            self.default_new_edge(&x, &y)?;
                        }
                    }
                }
            }
            "clique" | "independent set" => {
                let c = arg(self, &args)?;
                let l = self.colored(&c);
                let saved = self.gvalue("default edge kind");
                if name == "independent set" {
                    self.set_gvalue("default edge kind", tikra_tokens("-!-"));
                }
                for a in (1..=l.len()).rev() {
                    for b in (1..a).rev() {
                        if name == "clique" {
                            self.default_new_edge(&l[b - 1], &l[a - 1])?;
                        } else {
                            self.default_new_edge(&l[a - 1], &l[b - 1])?;
                        }
                    }
                }
                self.set_gvalue("default edge kind", saved);
            }
            "path" | "cycle" => {
                let c = arg(self, &args)?;
                let l = self.colored(&c);
                for w in l.windows(2) {
                    self.default_new_edge(&w[0], &w[1])?;
                }
                if name == "cycle" {
                    if let (Some(first), Some(last)) = (l.first(), l.last()) {
                        self.default_new_edge(last, first)?;
                    }
                }
            }
            "matching and star" | "matching" => {
                let (a, b) = (arg(self, &two[0])?, arg(self, &two[1])?);
                self.matching(&a, &b, name == "matching and star")?;
            }
            "grid" => {
                let c = arg(self, &args)?;
                let l = self.colored(&c);
                let wrap = self.wrap_after();
                for b in 1..=l.len() {
                    if (b - 1) % wrap > 0 {
                        self.default_new_edge(&l[b - 1], &l[b - 2])?;
                    }
                    if b > wrap {
                        self.default_new_edge(&l[b - wrap - 1], &l[b - 1])?;
                    }
                }
            }
            "butterfly" | "butterfly'" => {
                self.process_keys(args, Some("/tikz/graphs/butterfly"))?;
                let level = self.gvalue_string("butterfly/level").parse::<f64>().unwrap_or(1.0) as usize;
                let (from, to) = (self.gvalue_string("butterfly/from"), self.gvalue_string("butterfly/to"));
                if level > 0 {
                    let l = self.colored(&from);
                    for (k, x) in self.colored(&to).into_iter().enumerate() {
                        // the other side: k ± level, as k mod 2·level is below level or not
                        let other = if k % (2 * level) < level { k + level } else { k - level };
                        let other = (other + 1).min(l.len());
                        if other > 0 {
                            self.default_new_edge(&l[other - 1], &x)?;
                        }
                    }
                }
                if level == 0 || name == "butterfly" {
                    self.matching(&from, &to, true)?;
                }
            }
            _ => self.warn(format!("graph: operator '{name}' not supported")),
        }
        Ok(())
    }

    /// `matching` and `matching and star`: the i-th node of `a` with the i-th of `b`; with the star,
    /// the nodes left over on either side go to the last one of the other.
    fn matching(&mut self, a: &str, b: &str, star: bool) -> Result<(), Error> {
        let l = self.colored(a);
        let n = l.len();
        let mut k = 0usize;
        let mut prev = None;
        for x in self.colored(b) {
            k += 1;
            if star {
                k = k.min(n);
                if k > 0 {
                    self.default_new_edge(&l[k - 1], &x)?;
                }
            } else if k <= n {
                self.default_new_edge(&l[k - 1], &x)?;
            }
            prev = Some(x);
        }
        if star {
            if let Some(p) = prev {
                while k < n && k > 0 {
                    k += 1;
                    self.default_new_edge(&l[k - 1], &p)?;
                }
            }
        }
        Ok(())
    }

    /// `\tikzgraphpreparewrapafter`: `wrap after`, or the square root of the number of nodes.
    fn wrap_after(&mut self) -> usize {
        let w = self.gvalue_string("wrap after").parse::<f64>().unwrap_or(0.0);
        let w = if w == 0.0 { self.macro_string("tikzgraphVnum").trim().parse::<f64>().unwrap_or(1.0).sqrt().trunc() } else { w };
        (w as usize).max(1)
    }

    // ------------------------------------------------------------ placement

    /// A point of a shift key (`chain shift={(1,0)}`), in the picture's coordinates, in pt.
    fn shift_value(&mut self, key: &str) -> Result<crate::scene::Point, Error> {
        let s = self.gvalue_string(key);
        let inner = s.trim().trim_start_matches('{').trim_end_matches('}').trim().trim_start_matches('(').trim_end_matches(')').to_string();
        let p = self.coordinate(&inner)?.p;
        let p = self.st.transform.inverse().apply(p);
        Ok(crate::scene::Point::new(p.x / PT, p.y / PT))
    }

    /// `(angle:radius)` of a polar shift key: the angle in degrees, the radius in pt.
    fn polar_value(&mut self, key: &str) -> Result<(f64, f64), Error> {
        let s = self.gvalue_string(key);
        let inner = s.trim().trim_start_matches('{').trim_end_matches('}').trim().trim_start_matches('(').trim_end_matches(')').to_string();
        let (a, r) = inner.split_once(':').unwrap_or((&inner, "0"));
        let a = self.length(a)? / PT;
        let r = self.length(r)? / PT;
        Ok((a, r))
    }

    /// `placement/compute position` of the Cartesian, circular and grid placements: the shift of the
    /// next node from the counters, given to the nodes.
    fn compute_position(&mut self, how: &str) -> Result<(), Error> {
        let (width, depth) = (self.place("width"), self.place("depth"));
        let (x, y) = match how {
            "circular" => {
                let (ca, cr) = self.polar_value("chain polar shift")?;
                let (ga, gr) = self.polar_value("group polar shift")?;
                let radius = self.gvalue_string("radius");
                let radius = self.length(&radius)? / PT;
                let phase = self.gvalue_string("phase");
                let phase = self.number(&phase)?;
                let angle = ca * width + ga * depth + phase;
                let r = cr * width + gr * depth + radius;
                let (s, c) = angle.to_radians().sin_cos();
                (r * c, r * s)
            }
            _ => {
                let (c, g) = (self.shift_value("chain shift")?, self.shift_value("group shift")?);
                let (cs, gs) = if how == "grid" {
                    let wrap = self.wrap_after() as f64;
                    let t = depth.max(width);
                    (t % wrap, (t / wrap).trunc())
                } else {
                    (width, depth)
                };
                (c.x * cs + g.x * gs, c.y * cs + g.y * gs)
            }
        };
        // TikZ says shift={(x,y)}: the same translation
        self.gkeys(tikra_tokens(&format!("nodes={{xshift={}pt,yshift={}pt}}", pgf_number(x), pgf_number(y))))
    }
}

/// After the `-` of an arrow: `->`, `--` or `-!-`.
fn arrow_after_dash(r: &mut Reader) -> String {
    if r.peek_char('>') {
        r.next();
        "->".into()
    } else if r.peek_char('-') {
        r.next();
        "--".into()
    } else if r.peek_char('!') {
        r.next();
        if r.peek_char('-') {
            r.next();
        }
        "-!-".into()
    } else {
        "--".into()
    }
}

/// After `<-`: `<->` or `<-`.
fn back_arrow(r: &mut Reader) -> String {
    if r.peek_char('>') {
        r.next();
        "<->".into()
    } else {
        "<-".into()
    }
}

/// `"…"` in a quick graph (at the first quote).
fn quick_quoted(r: &mut Reader) -> Vec<Token> {
    r.skip_spaces();
    let mut out = Vec::new();
    if r.peek().is_some_and(|t| t.is_char('"')) {
        r.next();
        while let Some(t) = r.next() {
            if t.is_char('"') {
                break;
            }
            out.push(t);
        }
    }
    out
}

// -------------------------------------------------------------------- the native keys

fn value_text(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<String, Error> {
    Ok(match v {
        Some(v) => e.tex.expand_to_string(v.clone())?.trim().to_string(),
        None => String::new(),
    })
}

fn k_invoke_operator(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    e.invoke_operator_tokens(v.unwrap_or_default())
}

fn k_op(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 2);
    let name = e.tex.expand_to_string(a[0].clone())?.trim().to_string();
    e.graph_operator(&name, a[1].clone())
}

/// `\tikzgraphforeachcolorednode{color}{\macro}`.
fn k_foreach_colored(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 2);
    let color = e.tex.expand_to_string(a[0].clone())?.trim().to_string();
    for n in e.colored(&color) {
        let mut code = a[1].clone();
        code.extend(text_group(&n));
        e.run_tokens(code)?;
    }
    Ok(())
}

/// `\tikzgraphpreparecolor{color}\count{prefix}`: the count of nodes of that color, and the i-th
/// in the macro `\csname prefix i\endcsname`.
fn k_prepare_color(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 3);
    let color = e.tex.expand_to_string(a[0].clone())?.trim().to_string();
    let prefix = e.tex.expand_to_string(a[2].clone())?.trim().to_string();
    let l = e.colored(&color);
    if let Some(Meaning::Count(c)) = a[1].iter().find(|t| !t.is_space()).map(|t| e.tex.meaning(t)) {
        e.tex.set_count(c, l.len() as i64, false);
    }
    for (i, n) in l.iter().enumerate() {
        e.tex.define_string(&format!("{prefix}{}", i + 1), n, false);
    }
    Ok(())
}

/// `color class=name`: the keys `name`, `not name`, `!name` and `recolor name by`.
fn k_color_class(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let n = value_text(e, &v)?;
    let keys = format!(
        "{n}/.style={{operator={{\\tikra@gop{{colorize}}{{{n}}}}}}},not {n}/.style={{operator={{\\tikra@gop{{recolor}}{{{{{n}}}{{}}}}}}}},\
         recolor {n} by/.style={{operator={{\\tikra@gop{{recolor}}{{{{{n}}}{{#1}}}}}}}},!{n}/.style={{not {n}}}"
    );
    e.gkeys(tikra_tokens(&keys))
}

/// `source edge style`, `target edge node`…: `{side}{style}{node}` added to the current node's.
fn k_annotate(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 3);
    let side = if e.tex.expand_to_string(a[0].clone())?.trim() == "a" { 'a' } else { 'b' };
    let name = e.macro_string("tikra@gr@name");
    if let Some(g) = e.graph_state() {
        let entry = g.annotations.entry((side, name)).or_insert_with(|| (Vec::new(), Vec::new()));
        if entry.0.is_empty() && entry.1.is_empty() {
            *entry = (a[1].clone(), a[2].clone());
        } else {
            entry.0.push(Token::other(','));
            entry.0.extend(a[1].clone());
            entry.1.extend(a[2].clone());
        }
    }
    Ok(())
}

fn k_annotate_clear(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let side = if value_text(e, &v)? == "a" { 'a' } else { 'b' };
    let name = e.macro_string("tikra@gr@name");
    if let Some(g) = e.graph_state() {
        g.annotations.remove(&(side, name));
    }
    Ok(())
}

/// `put node text on incoming edges`: `{side}{options}`, the node's text as a node on its edges.
fn k_edge_node_from_text(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 2);
    let side = if e.tex.expand_to_string(a[0].clone())?.trim() == "source" { "a" } else { "b" };
    let text = super::spy::macro_body(e, "tikzgraphnodetext");
    let mut node = tikra_tokens("node[");
    // the style given no value passes \pgfkeysnovalue
    node.extend(a[1].iter().filter(|t| !t.is_cs("pgfkeysnovalue")).cloned());
    node.push(Token::other(']'));
    node.extend(group(text));
    let mut args = text_group(side);
    args.extend(group(Vec::new()));
    args.extend(group(node));
    k_annotate(e, Some(args))?;
    e.tex.set_meaning(&Token::cs("tikzgraphnodeas"), macro_tokens(Vec::new()), false);
    Ok(())
}

fn k_pos(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let how = value_text(e, &v)?;
    e.compute_position(&how)
}

/// `\tikz@lib@graph@width@sep` and `…@depth@sep`: `{x or y}{node}{sep}`, the node's size plus the
/// separation, in pt, as the logical width of the node.
fn k_sep(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 3);
    let axis = e.tex.expand_to_string(a[0].clone())?.trim().to_string();
    let name = e.tex.expand_to_string(a[1].clone())?.trim().to_string();
    let sep = e.length_of(a[2].clone())? / PT;
    let size = match e.nodes.get(&name) {
        Some(n) => {
            let inv = e.st.transform.inverse();
            let at = |a: &str| inv.apply(n.anchor(a).unwrap_or(n.center()));
            if axis == "x" {
                at("east").x - at("west").x
            } else {
                at("north").y - at("south").y
            }
        }
        None => 0.0,
    };
    e.tex.define_string("pgfmathresult", &pgf_number(sep + size / PT), false);
    Ok(())
}

/// `V={list}` and `W={list}`: the list and the number of its items.
fn set_shore(e: &mut Engine, v: Option<Vec<Token>>, name: &str) -> Result<(), Error> {
    let v = v.unwrap_or_default();
    e.tex.set_meaning(&Token::cs(&format!("tikzgraph{name}")), macro_tokens(v.clone()), false);
    let items = super::foreach_items(&e.tex.expand_fully(v)?).len();
    e.tex.set_meaning(&Token::cs(&format!("tikzgraph{name}num")), macro_from(&items.to_string()), false);
    Ok(())
}

fn k_v(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    set_shore(e, v, "V")
}

fn k_w(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    set_shore(e, v, "W")
}

fn k_parse(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    if let Some(g) = e.graph_state() {
        g.extras.extend(v.unwrap_or_default());
    }
    Ok(())
}

/// `declare={name}{graph}`: `name` used as a node reads the graph instead.
fn k_declare(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = split_arguments(&v.unwrap_or_default(), 2);
    let name = e.tex.expand_to_string(a[0].clone())?.trim().to_string();
    e.tex.set_meaning(&Token::cs(&format!("tikra@gr@def@{name}")), macro_tokens(a[1].clone()), false);
    Ok(())
}

fn current_name(e: &mut Engine) -> String {
    e.macro_string("pgfkeyscurrentname").trim().to_string()
}

/// An unknown key among the options of a join: an option of the edges (`\tikz@lib@graph@unknown@edge@option`).
fn k_unknown_edge_option(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = current_name(e);
    let mut styling = e.gvalue("@edge options");
    styling.push(Token::other(','));
    styling.extend(tex::string_tokens(&name));
    if let Some(v) = v {
        styling.push(Token::other('='));
        styling.extend(group(v));
    }
    e.set_gvalue("@edge options", styling);
    Ok(())
}

/// `redirect unknown to tikz`: unknown keys go to `/tikz`.
fn k_redirect_unknown(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    e.set_unknown("@redirect");
    Ok(())
}

fn k_redirect(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = current_name(e);
    let old = std::mem::replace(&mut e.st.key_path, "/tikz".into());
    let r = e.invoke(&format!("/tikz/{name}"), v);
    e.st.key_path = old;
    r
}

/// `placement/place` ran in this TeX group.
fn k_placed(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let depth = e.tex.group_depth().to_string();
    e.tex.define_string("tikra@lg@placed", &depth, false);
    Ok(())
}

fn k_ignore(_e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    Ok(())
}

/// At the end of a node's options: the operators they asked for, kept for when the node exists.
fn k_save_operators(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let ops = e.gvalue("@pending operators");
    if let Some(g) = e.graph_state() {
        g.saved_ops = ops;
    }
    Ok(())
}

/// `new set=name`: a set of nodes, used in a graph as `(name)`.
fn k_new_set(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = value_text(e, &v)?;
    e.graphs.sets.insert(name, Vec::new());
    Ok(())
}

/// `set=name` in a node's options: the node joins the set once it has its name.
fn k_set(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = value_text(e, &v)?;
    if !e.graphs.sets.contains_key(&name) {
        e.warn(format!("undefined set '{name}'"));
        return Ok(());
    }
    let mut sets = e.macro_string("tikra@lg@sets");
    sets.push_str(&name);
    sets.push(',');
    e.tex.define_string("tikra@lg@sets", &sets, false);
    Ok(())
}

impl Engine {
    /// The name of a node in a set that has none (`\tikz@fig@mustbenamed`).
    pub fn set_member_name(&mut self) -> Option<String> {
        if self.macro_string("tikra@lg@sets").is_empty() {
            return None;
        }
        self.graphs.anonymous += 1;
        Some(format!("tikz@f@{}", self.graphs.anonymous))
    }

    /// A node just named joins the sets of its options.
    pub fn join_sets(&mut self, name: &str) {
        let sets = self.macro_string("tikra@lg@sets");
        for s in sets.split(',').filter(|s| !s.is_empty()) {
            if let Some(members) = self.graphs.sets.get_mut(s) {
                members.push(name.to_string());
            }
        }
    }
}

/// `/tikz/edge node={node[options]{text}…}`: nodes on the edge or `to` being read, as quotes put them.
fn k_tikz_edge_node(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let mut r = Reader::new(v.unwrap_or_default());
    loop {
        r.skip_spaces();
        let mut word = String::new();
        while let Some(Token::Char(c, Cat::Letter)) = r.peek() {
            word.push(*c);
            r.next();
        }
        if word != "node" {
            return Ok(());
        }
        let options = if r.peek_char('[') { r.delimited('[', ']') } else { Vec::new() };
        r.skip_spaces();
        let text = if is_begin(r.peek()) { r.group() } else { Vec::new() };
        if e.st.quotes_on_path {
            e.st.quoted.push((text, options));
        }
    }
}

const NATIVE: &[(&str, KeyFn)] = &[
    ("/tikz/edge node", k_tikz_edge_node),
    ("/tikz/graphs/@op", k_op),
    ("/tikz/graphs/@invoke operator", k_invoke_operator),
    ("/tikz/graphs/@foreach colored", k_foreach_colored),
    ("/tikz/graphs/@prepare color", k_prepare_color),
    ("/tikz/graphs/color class", k_color_class),
    ("/tikz/graphs/@annotate", k_annotate),
    ("/tikz/graphs/@annotate clear", k_annotate_clear),
    ("/tikz/graphs/@edge node from text", k_edge_node_from_text),
    ("/tikz/graphs/@pos", k_pos),
    ("/tikz/graphs/@sep", k_sep),
    ("/tikz/graphs/V", k_v),
    ("/tikz/graphs/W", k_w),
    ("/tikz/graphs/parse", k_parse),
    ("/tikz/graphs/declare", k_declare),
    ("/tikz/graphs/@unknown edge option", k_unknown_edge_option),
    ("/tikz/graphs/redirect unknown to tikz", k_redirect_unknown),
    ("/tikz/graphs/@redirect", k_redirect),
    ("/tikz/graphs/@ignore", k_ignore),
    ("/tikz/graphs/@placed", k_placed),
    ("/tikz/graphs/@save operators", k_save_operators),
    ("/tikz/new set", k_new_set),
    ("/tikz/set", k_set),
];

/// The keys of the graphs and graphs.standard libraries that TikZ defines as styles and values,
/// as it defines them; their code, where TikZ has TeX code, calls the native keys above.
const DEFINITIONS: &str = r"
\def\tikzgraphsset{\pgfqkeys{/tikz/graphs}}
\def\tikra@gop#1#2{\pgfkeys{/tikz/graphs/@op={#1}{#2}}}
\def\tikzgraphforeachcolorednode#1#2{\pgfkeys{/tikz/graphs/@foreach colored={#1}{#2}}}
\def\tikzgraphpreparecolor#1#2#3{\pgfkeys{/tikz/graphs/@prepare color={#1}{#2}{#3}}}
\def\tikzgraphinvokeoperator#1{\pgfkeys{/tikz/graphs/@invoke operator={#1}}}
\def\tikra@gr@linear@pos{\pgfkeys{/tikz/graphs/@pos=linear}}
\def\tikra@gr@circular@pos{\pgfkeys{/tikz/graphs/@pos=circular}}
\def\tikra@gr@grid@pos{\pgfkeys{/tikz/graphs/@pos=grid}}
\def\tikra@gr@width@sep#1#2{\pgfkeys{/tikz/graphs/@sep={x}{#1}{#2}}}
\def\tikra@gr@depth@sep#1#2{\pgfkeys{/tikz/graphs/@sep={y}{#1}{#2}}}
\def\tikzgraphleftanchor{}\def\tikzgraphrightanchor{}
\def\tikra@gr@new#1#2#3#4#5#6{\path[#1,every new #2/.try] (#3\tikzgraphleftanchor) edge[#5] #6 (#4\tikzgraphrightanchor);}
\def\tikra@gr@one{\def\pgfmathresult{1}}
\newif\iftikra@gr@all\newif\iftikra@gr@simple\newif\iftikra@gr@trie
\newif\iftikra@gr@fresh@node\newif\iftikra@gr@numbered\newif\iftikra@gr@quick
\newcount\tikra@gr@autonum
\def\tikra@gr@autosep{\space}\def\tikra@gr@path{}
\def\tikra@gr@x{0}\def\tikra@gr@y{0}
\def\tikra@gr@at{\pgfkeys{/tikz/at/.expanded={(\tikra@gr@x,\tikra@gr@y)}}}
\def\tikra@gr@auto@h{center}\def\tikra@gr@auto@v{center}
\def\tikra@gr@auto@center@center{center}\def\tikra@gr@auto@west@center{west}
\def\tikra@gr@auto@east@center{east}\def\tikra@gr@auto@center@north{north}
\def\tikra@gr@auto@west@north{north west}\def\tikra@gr@auto@east@north{north east}
\def\tikra@gr@auto@center@south{south}\def\tikra@gr@auto@west@south{south west}
\def\tikra@gr@auto@east@south{south east}
\pgfkeys{/utils/exec/.code={#1}}
\tikzset{edge label/.style={/tikz/edge node={node[auto]{#1}}}, edge label'/.style={/tikz/edge node={node[auto,swap]{#1}}}}
\tikzgraphsset{
  % an edge of each kind: the tip, the kind's own style, from, to, options, edge nodes
  new ->/.code n args={4}{\tikra@gr@new{->}{->}{#1}{#2}{#3}{#4}},
  new <-/.code n args={4}{\tikra@gr@new{<-}{<-}{#1}{#2}{#3}{#4}},
  new <->/.code n args={4}{\tikra@gr@new{<->}{<->}{#1}{#2}{#3}{#4}},
  new --/.code n args={4}{\tikra@gr@new{-}{--}{#1}{#2}{#3}{#4}},
  new -!-/.code n args={4}{\relax},
  default edge kind/.initial=--, default edge operator/.initial=matching and star,
  --/.style={default edge kind=--}, ->/.style={default edge kind=->}, <-/.style={default edge kind=<-},
  <->/.style={default edge kind=<->}, -!-/.style={default edge kind=-!-},
  left anchor/.code={\def\tikz@temp{#1}\ifx\tikz@temp\pgfutil@empty\def\tikzgraphleftanchor{}\else\def\tikzgraphleftanchor{.#1}\fi},
  right anchor/.code={\def\tikz@temp{#1}\ifx\tikz@temp\pgfutil@empty\def\tikzgraphrightanchor{}\else\def\tikzgraphrightanchor{.#1}\fi},
  use existing nodes/.is if=tikra@gr@all,
  simple/.code={\iftikra@gr@simple\else\tikra@gr@simpletrue\pgfkeysalso{operator={\tikra@gop{simple}{}}}\fi},
  multi/.code={\tikra@gr@simplefalse},
  @node options/.style={},
  nodes/.style={/tikz/graphs/@node options/.append style={,#1}},
  @edge options/.initial=,
  edges/.style={/tikz/graphs/@edge options/.append={,#1}},
  edge/.style={edges={#1}},
  @edge texts/.initial=,
  edge node/.style={/tikz/graphs/@edge texts/.append={#1}},
  edge label/.style={/tikz/graphs/@edge texts/.append={node[auto]{#1}}},
  edge label'/.style={/tikz/graphs/@edge texts/.append={node[auto,swap]{#1}}},
  @pending operators/.initial=,
  operator/.style={/tikz/graphs/@pending operators/.append={#1}},
  @extra group options/.style={},
  source edge style/.code={\pgfkeys{/tikz/graphs/@annotate={a}{#1}{}}},
  source edge node/.code={\pgfkeys{/tikz/graphs/@annotate={a}{}{#1}}},
  source edge clear/.code={\pgfkeys{/tikz/graphs/@annotate clear=a}},
  target edge style/.code={\pgfkeys{/tikz/graphs/@annotate={b}{#1}{}}},
  target edge node/.code={\pgfkeys{/tikz/graphs/@annotate={b}{}{#1}}},
  target edge clear/.code={\pgfkeys{/tikz/graphs/@annotate clear=b}},
  clear >/.style={target edge clear}, clear </.style={source edge clear},
  x/.code={\def\tikra@gr@x{#1}\tikra@gr@at},
  y/.code={\def\tikra@gr@y{#1}\tikra@gr@at},
  trie/.is if=tikra@gr@trie,
  put node text on incoming edges/.style={nodes={/tikz/graphs/@edge node from text={target}{#1}}},
  put node text on outgoing edges/.style={nodes={/tikz/graphs/@edge node from text={source}{#1}}},
  edge quotes/.code={\pgfkeysalso{/tikz/every edge quotes/.style={#1}}},
  edge quotes center/.style={edge quotes=anchor=center}, edge quotes mid/.style={edge quotes=anchor=mid},
  fresh nodes/.is if=tikra@gr@fresh@node,
  number nodes/.code={\tikra@gr@numberedtrue\pgfmathsetcount\tikra@gr@autonum{#1}}, number nodes/.default=1,
  number nodes sep/.code={\def\tikra@gr@autosep{#1}},
  typeset/.code={\def\tikra@gr@typesetter{#1}}, typeset=\tikzgraphnodetext,
  math nodes/.code={\def\tikra@gr@typesetter{$\tikzgraphnodetext$}},
  empty nodes/.code={\def\tikra@gr@typesetter{}},
  as/.code={\def\tikzgraphnodeas{#1}},
  name separator/.code={\def\tikra@gr@name@separator{#1}}, name separator=\space,
  name/.code={\edef\tikra@gr@path{\tikra@gr@path#1\tikra@gr@name@separator}},
  color class=source, color class=source', color class=source'', color class=target,
  color class=target', color class=all,
  complete bipartite/.style 2 args={operator={\tikra@gop{complete bipartite}{{#1}{#2}}}},
  induced complete bipartite/.style 2 args={induced independent set={#1},induced independent set={#2},complete bipartite={#1}{#2}},
  clique/.style={operator={\tikra@gop{clique}{#1}}}, clique/.default=all,
  induced independent set/.style={operator={\tikra@gop{independent set}{#1}}}, induced independent set/.default=all,
  path/.style={operator={\tikra@gop{path}{#1}}}, path/.default=all,
  induced path/.style={induced independent set={#1},path={#1}}, induced path/.default=all,
  cycle/.style={operator={\tikra@gop{cycle}{#1}}}, cycle/.default=all,
  induced cycle/.style={induced independent set={#1},cycle={#1}}, induced cycle/.default=all,
  matching and star/.style 2 args={operator={\tikra@gop{matching and star}{{#1}{#2}}}},
  matching/.style 2 args={operator={\tikra@gop{matching}{{#1}{#2}}}},
  % the two-shore operators join the nodes on the left (target') with those on the right (source')
  complete bipartite/.default={target'}{source'}, induced complete bipartite/.default={target'}{source'},
  matching and star/.default={target'}{source'}, matching/.default={target'}{source'},
  butterfly/.style={operator={\tikra@gop{butterfly}{#1}}}, butterfly/.default=,
  butterfly/level/.initial=1, butterfly/from/.initial=target', butterfly/to/.initial=source',
  butterfly'/.style={operator={\tikra@gop{butterfly'}{#1}}}, butterfly'/.default=,
  no edges/.style={operator=\relax},
  grid/.style={operator={\tikra@gop{grid}{#1}}}, grid/.default=all,
  placement/compute position/.code=\tikra@gr@linear@pos,
  placement/place/.code={\pgfkeys{/tikz/graphs/placement/compute position,/tikz/graphs/@placed}%
    \pgfkeyssetvalue{/tikz/graphs/placement/element count}{0}\pgfkeyssetvalue{/tikz/graphs/placement/chain count}{0}%
    \pgfkeyssetvalue{/tikz/graphs/placement/depth}{0}\pgfkeyssetvalue{/tikz/graphs/placement/width}{0}%
    \pgfkeyssetvalue{/tikz/graphs/placement/local depth}{0}\pgfkeyssetvalue{/tikz/graphs/placement/local width}{0}},
  placement/element count/.initial=0, placement/chain count/.initial=0, placement/depth/.initial=0,
  placement/width/.initial=0, placement/level/.initial=0, placement/local depth/.initial=0, placement/local width/.initial=0,
  placement/logical node depth/.code=\tikra@gr@one,
  placement/logical node width/.code=\tikra@gr@one,
  Cartesian placement/.style={placement/place, placement/compute position/.code=\tikra@gr@linear@pos},
  chain shift/.initial={(1,0)}, group shift/.initial={(0,-1)},
  grow right/.style={placement/place, chain shift={(#1,0)}, @auto anchor horizontal=center, placement/logical node width/.code=\tikra@gr@one},
  grow left/.style={placement/place, chain shift={(-#1,0)}, @auto anchor horizontal=center, placement/logical node width/.code=\tikra@gr@one},
  grow up/.style={placement/place, chain shift={(0,#1)}, @auto anchor horizontal=center, placement/logical node width/.code=\tikra@gr@one},
  grow down/.style={placement/place, chain shift={(0,-#1)}, @auto anchor vertical=center, placement/logical node width/.code=\tikra@gr@one},
  branch right/.style={placement/place, group shift={(#1,0)}, @auto anchor horizontal=center, placement/logical node depth/.code=\tikra@gr@one},
  branch left/.style={placement/place, group shift={(-#1,0)}, @auto anchor horizontal=center, placement/logical node depth/.code=\tikra@gr@one},
  branch up/.style={placement/place, group shift={(0,#1)}, @auto anchor vertical=center, placement/logical node depth/.code=\tikra@gr@one},
  branch down/.style={placement/place, group shift={(0,-#1)}, @auto anchor vertical=center, placement/logical node depth/.code=\tikra@gr@one},
  grow right sep/.style={Cartesian placement, chain shift={(1pt,0)}, @auto anchor horizontal=west, placement/logical node width/.code=\tikra@gr@width@sep{##1}{#1}},
  grow left sep/.style={Cartesian placement, chain shift={(-1pt,0)}, @auto anchor horizontal=east, placement/logical node width/.code=\tikra@gr@width@sep{##1}{#1}},
  grow up sep/.style={Cartesian placement, chain shift={(0,1pt)}, @auto anchor vertical=south, placement/logical node width/.code=\tikra@gr@depth@sep{##1}{#1}},
  grow down sep/.style={Cartesian placement, chain shift={(0,-1pt)}, @auto anchor vertical=north, placement/logical node width/.code=\tikra@gr@depth@sep{##1}{#1}},
  branch right sep/.style={Cartesian placement, group shift={(1pt,0)}, @auto anchor horizontal=west, placement/logical node depth/.code=\tikra@gr@width@sep{##1}{#1}},
  branch left sep/.style={Cartesian placement, group shift={(-1pt,0)}, @auto anchor horizontal=east, placement/logical node depth/.code=\tikra@gr@width@sep{##1}{#1}},
  branch up sep/.style={Cartesian placement, group shift={(0,1pt)}, @auto anchor vertical=south, placement/logical node depth/.code=\tikra@gr@depth@sep{##1}{#1}},
  branch down sep/.style={Cartesian placement, group shift={(0,-1pt)}, @auto anchor vertical=north, placement/logical node depth/.code=\tikra@gr@depth@sep{##1}{#1}},
  % the default steps: a unit for the plain ones, 1em between nodes for the sep ones
  grow right/.default=1, grow left/.default=1, grow up/.default=1, grow down/.default=1, branch right/.default=1, branch left/.default=1, branch up/.default=1, branch down/.default=1,
  grow right sep/.default=1em, grow left sep/.default=1em, grow up sep/.default=1em, grow down sep/.default=1em, branch right sep/.default=1em, branch left sep/.default=1em, branch up sep/.default=1em, branch down sep/.default=1em,
  @auto anchor horizontal/.style={nodes={anchor=\csname tikra@gr@auto@\tikra@gr@auto@h @\tikra@gr@auto@v\endcsname}, /utils/exec=\def\tikra@gr@auto@h{#1}},
  @auto anchor vertical/.style={nodes={anchor=\csname tikra@gr@auto@\tikra@gr@auto@h @\tikra@gr@auto@v\endcsname}, /utils/exec=\def\tikra@gr@auto@v{#1}},
  no placement/.style={placement/place, placement/compute position/.code=},
  circular placement/.style={placement/place, placement/compute position/.code=\tikra@gr@circular@pos,
    placement/logical node depth/.code=\tikra@gr@one, placement/logical node width/.code=\tikra@gr@one},
  clockwise/.style={circular placement, group polar shift={(-360/#1:0)}}, clockwise/.default=\tikzgraphVnum,
  counterclockwise/.style={circular placement, group polar shift={(360/#1:0)}}, counterclockwise/.default=\tikzgraphVnum,
  chain polar shift/.initial={(0:1cm)}, group polar shift/.initial={(60:0)}, radius/.initial=1cm, phase/.initial=90,
  grid placement/.style={placement/place, placement/compute position/.code=\tikra@gr@grid@pos},
  wrap after/.initial=0,
  V={1}, W={1},
  n/.style={name shore V/.style={name=V},V={1,...,#1}}, m/.style={name shore W/.style={name=W},W={1,...,#1}},
  name shore V/.style={}, name shore W/.style={},
  quick/.is if=tikra@gr@quick,
  declare={subgraph I_n}{\foreach \tikra@gr@node@num in \tikzgraphV {\tikra@gr@node@num}},
  declare={subgraph I_nm}{subgraph I_n [name shore V] -- [no edges] subgraph I_n [name shore W, V/.expand once=\tikzgraphW]},
  declare={subgraph K_n}{[clique] subgraph I_n},
  declare={subgraph K_nm}{[color class=shore V, color class=shore W, complete bipartite={shore V}{shore W}]
    subgraph I_n [name shore V, shore V] -- [no edges] subgraph I_n [name shore W, V/.expand once=\tikzgraphW, shore W]},
  declare={subgraph C_n}{[cycle] subgraph I_n},
  declare={subgraph P_n}{[path] subgraph I_n},
  declare={subgraph Grid_n}{[grid] subgraph I_n},
}
";

/// Defines the native keys and the libraries' own definitions.
pub fn define(e: &mut Engine) {
    for (path, f) in NATIVE {
        let id = NATIVE_KEY_BASE + e.key_fns.len() as u16;
        e.key_fns.push(*f);
        e.tex.define(&macro_name(&format!("{path}/.@cmd")), Meaning::Native(id));
    }
    let mut lexer = tex::Tex::new_lexer();
    lexer.push_text(DEFINITIONS);
    let mut tokens = Vec::new();
    while let Some(t) = lexer.get_token() {
        tokens.push(t);
    }
    e.run_tokens(tokens).expect("the graphs libraries' definitions are fixed and run");
    e.tex.warnings.clear();
    e.st.key_path = "/".into();
}
