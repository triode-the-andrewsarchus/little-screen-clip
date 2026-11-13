//! TikZ libraries that are styles on top of nodes, paths and trees: `automata` (states, initial and
//! accepting marks), `petri` (places, transitions, tokens), `er` (entities, relationships,
//! attributes), `shadows` (copies of a node's background drawn behind it) and `through` (a circle
//! node through a point), and the matrix library's delimiters. Their keys are TikZ
//! styles with the values of `tikzlibraryautomata.code.tex`, `tikzlibrarypetri.code.tex`,
//! `tikzlibraryer.code.tex` and `tikzlibraryshadows.code.tex`; what they need from TikZ itself
//! (`append after command`, `prefix after command`, `growth function`) is here too.

use std::rc::Rc;

use crate::scene::{Point, Transform};
use crate::tex::Token;
use crate::Error;

use super::nodes::NodeInfo;
use super::options::KeyFn;
use super::Engine;

macro_rules! keyfn {
    ($name:ident, |$e:ident, $v:ident| $body:expr) => {
        fn $name($e: &mut Engine, $v: Option<Vec<Token>>) -> Result<(), Error> {
            $body
        }
    };
}

// `append after command` adds path operations after the node (as `after node path`), `prefix after
// command` puts them before those already there; `insert path` puts path operations where the options are
keyfn!(k_append_after_command, |e, v| {
    e.st.after_node_path.extend(v.unwrap_or_default());
    Ok(())
});
keyfn!(k_prefix_after_command, |e, v| {
    let mut path = v.unwrap_or_default();
    path.append(&mut e.st.after_node_path);
    e.st.after_node_path = path;
    Ok(())
});
// `growth function=\macro`: how a tree places its children instead of `grow` and the distances; the
// macro transforms the coordinate system from the parent, knowing `\tikznumberofchildren` and
// `\tikznumberofcurrentchild`
keyfn!(k_growth_function, |e, v| {
    e.st.growth_function = v.filter(|v| !crate::keys::trim(v).is_empty());
    Ok(())
});

keyfn!(k_insert_path, |e, v| {
    e.st.insert_path.extend(v.unwrap_or_default());
    Ok(())
});

// the through library: `circle through=(a)` makes the node a circle around its position through
// the point
keyfn!(k_circle_through, |e, v| {
    e.process_keys(super::tikra_tokens("shape=circle,inner sep=0pt,outer sep=0pt"), Some("/tikz"))?;
    let s = e.tex.expand_to_string(v.unwrap_or_default())?;
    let inner = crate::keys::strip_braces(s.trim()).trim().trim_start_matches('(').trim_end_matches(')').to_string();
    e.st.node.through = Some(e.coordinate(&inner)?.p);
    Ok(())
});

pub const KEYS: &[(&str, KeyFn)] = &[
    ("/tikz/append after command", k_append_after_command),
    ("/tikz/prefix after command", k_prefix_after_command),
    ("/tikz/growth function", k_growth_function),
    ("/tikz/circle through", k_circle_through),
    ("/tikz/insert path", k_insert_path),
];

/// The libraries' styles, read as TeX with `@` a letter.
pub const DEFINITIONS: &str = r"
\newcount\tikznumberofchildren \newcount\tikznumberofcurrentchild
% plain TeX's form of the scope environment
\def\scope{\begin{scope}}
\def\endscope{\end{scope}}
% a reference of a single run of LaTeX: not yet known
\def\ref#1{{\bfseries ??}}
\def\tikzaddafternodepathoption#1{#1\tikzset{prefix after command={\pgfextra{#1}}}}
\tikzset{
  every state/.style={}, every accepting by arrow/.style={}, every initial by arrow/.style={},
  state without output/.style={circle,draw,minimum size=2.5em,every state},
  state with output/.style={circle split,draw,minimum size=2.5em,every state},
  state/.style={state without output},
  accepting by double/.style={double,outer sep=.5\pgflinewidth+.3pt},
  accepting by arrow/.style={after node path={{[to path={[->,double=none,every accepting by arrow] --
    ([shift=(\tikra@au@accepting@angle:\tikra@au@accepting@distance)]\tikztostart.\tikra@au@accepting@angle)
    node[shape=rectangle,anchor=\tikra@au@accepting@anchor]{\tikra@au@accepting@text}}] edge ()}}},
  initial by arrow/.style={after node path={{[to path={[->,double=none,every initial by arrow]
    ([shift=(\tikra@au@initial@angle:\tikra@au@initial@distance)]\tikztostart.\tikra@au@initial@angle)
    node[shape=rectangle,anchor=\tikra@au@initial@anchor]{\tikra@au@initial@text} -- (\tikztostart)}] edge ()}}},
  initial by diamond/.style={shape=diamond},
  initial text/.code={\tikzaddafternodepathoption{\def\tikra@au@initial@text{#1}}},
  accepting text/.code={\tikzaddafternodepathoption{\def\tikra@au@accepting@text{#1}}},
  initial where/.code={\tikzaddafternodepathoption{\csname tikra@au@initial@where@#1\endcsname}},
  accepting where/.code={\tikzaddafternodepathoption{\csname tikra@au@accepting@where@#1\endcsname}},
  initial distance/.code={\tikzaddafternodepathoption{\def\tikra@au@initial@distance{#1}}},
  accepting distance/.code={\tikzaddafternodepathoption{\def\tikra@au@accepting@distance{#1}}},
  initial above/.style={initial by arrow,initial where=above},
  initial below/.style={initial by arrow,initial where=below},
  initial left/.style={initial by arrow,initial where=left},
  initial right/.style={initial by arrow,initial where=right},
  accepting above/.style={accepting by arrow,accepting where=above},
  accepting below/.style={accepting by arrow,accepting where=below},
  accepting left/.style={accepting by arrow,accepting where=left},
  accepting right/.style={accepting by arrow,accepting where=right},
  accepting/.style={accepting by double}, initial/.style={initial by arrow},
}
\def\tikra@au@initial@text{start}\def\tikra@au@accepting@text{}
\def\tikra@au@initial@distance{3ex}\def\tikra@au@accepting@distance{3ex}
\def\tikra@au@initial@angle{180}\def\tikra@au@initial@anchor{east}
\def\tikra@au@accepting@angle{0}\def\tikra@au@accepting@anchor{west}
\def\tikra@where#1#2#3{%
  \expandafter\def\csname tikra@au@initial@where@#1\endcsname{\def\tikra@au@initial@angle{#2}\def\tikra@au@initial@anchor{#3}}%
  \expandafter\def\csname tikra@au@accepting@where@#1\endcsname{\def\tikra@au@accepting@angle{#2}\def\tikra@au@accepting@anchor{#3}}}
\tikra@where{above}{90}{south}\tikra@where{below}{270}{north}
\tikra@where{left}{180}{east}\tikra@where{right}{0}{west}
\tikzset{
  every place/.style={}, every transition/.style={}, every token/.style={},
  place/.style={circle,draw,inner sep=0pt,minimum size=5ex,every place},
  transition/.style={rectangle,draw,inner sep=0pt,minimum size=4mm,every transition},
  pre/.style={<-,shorten <=1pt}, post/.style={->,shorten >=1pt},
  pre and post/.style={<->,shorten >=1pt,shorten <=1pt},
  token/.style={fill=black,draw=none,circle,inner sep=0.5pt,minimum size=1ex,text=white,font=\tiny,every token},
  token distance/.code={\def\tikz@token@distance{#1}},
  children are tokens/.style={growth function=\tikra@grow@tokens,edge from parent path=},
  tokens/.code={\ifnum#1>0 \tikzset{append after command={{[children are tokens]
    child foreach \tikz@dummy in {1,...,#1} {node[token]{}}}}}\fi},
  colored tokens/.code={\tikzset{append after command={{[children are tokens]
    child foreach \tikz@child@color in {#1} {node[token,\tikz@child@color]{}}}}}},
  structured tokens/.code={\tikzset{append after command={{[children are tokens]
    child foreach \tikzpetrichildname in {#1} {node[token]{\tikzpetrichildname}}}}}},
}
\def\tikz@token@distance{1.5ex}
\def\tikra@grow@tokens{\csname tikra@token@\the\tikznumberofchildren @\the\tikznumberofcurrentchild\endcsname}
\def\tikra@token#1#2#3#4{\expandafter\def\csname tikra@token@#1@#2\endcsname{%
  \pgftransformshift{\pgfpoint{#3*\tikz@token@distance}{#4*\tikz@token@distance}}}}
\tikra@token11{0}{0}
\tikra@token21{-.5}{0}\tikra@token22{.5}{0}
\tikra@token31{0}{.57}\tikra@token32{-.5}{-.306025}\tikra@token33{.5}{-.306025}
\tikra@token41{-.5}{.5}\tikra@token42{.5}{.5}\tikra@token43{-.5}{-.5}\tikra@token44{.5}{-.5}
\tikra@token51{0}{.85}\tikra@token52{-.808398}{.26266}\tikra@token53{.808398}{.26266}
\tikra@token54{-.499617}{-.687664}\tikra@token55{.499617}{-.687664}
\tikra@token61{-1}{.5}\tikra@token62{0}{.5}\tikra@token63{1}{.5}
\tikra@token64{-1}{-.5}\tikra@token65{0}{-.5}\tikra@token66{1}{-.5}
\tikra@token71{0}{1}\tikra@token72{-1}{.5}\tikra@token73{0}{0}\tikra@token74{1}{.5}
\tikra@token75{-1}{-.5}\tikra@token76{0}{-1}\tikra@token77{1}{-.5}
\tikra@token81{-.5}{1}\tikra@token82{.5}{1}\tikra@token83{-1}{0}\tikra@token84{0}{0}
\tikra@token85{1}{0}\tikra@token86{-1}{-1}\tikra@token87{0}{-1}\tikra@token88{1}{-1}
\tikra@token91{-1}{1}\tikra@token92{0}{1}\tikra@token93{1}{1}\tikra@token94{-1}{0}\tikra@token95{0}{0}
\tikra@token96{1}{0}\tikra@token97{-1}{-1}\tikra@token98{0}{-1}\tikra@token99{1}{-1}
\tikzset{
  every entity/.style={}, every relationship/.style={}, every attribute/.style={},
  entity/.style={rectangle,draw,minimum height=2\baselineskip,minimum width=4\baselineskip,every entity},
  relationship/.style={diamond,draw,minimum size=1.5\baselineskip,inner sep=1pt,every relationship},
  attribute/.style={minimum size=1.5\baselineskip,ellipse,draw,every attribute},
  key attribute/.style={font=\itshape,attribute},
}
% the shadow keys' values, as the shadow's transformation reads them
\def\tikra@shadow#1{\pgfkeysvalueof{/tikz/shadow #1}}
\tikzset{
  shadow scale/.initial=1, shadow xshift/.initial=0pt, shadow yshift/.initial=0pt,
  general shadow/.style={preaction={#1,transform canvas={
    scale around=\tikra@shadow{scale}:(current path bounding box.center),
    shift={(\tikra@shadow{xshift},\tikra@shadow{yshift})}}}},
  drop shadow/.style={general shadow={shadow scale=1,shadow xshift=.5ex,shadow yshift=-.5ex,
    opacity=.5,fill=black!50,every shadow,#1}},
  circular drop shadow/.style={general shadow={shadow scale=1.1,shadow xshift=.3ex,shadow yshift=-.3ex,
    fill=black!50,path fading={circle with fuzzy edge 15 percent},every shadow,#1}},
  circular glow/.style={general shadow={shadow scale=1.25,shadow xshift=0pt,shadow yshift=0pt,
    fill=black!50,path fading={circle with fuzzy edge 15 percent},every shadow,#1}},
  copy shadow/.style={general shadow={shadow scale=1,shadow xshift=.5ex,shadow yshift=.5ex,
    fill=\tikz@fillcolor,draw=\tikz@strokecolor,every shadow,#1}},
  double copy shadow/.style={
    general shadow={shadow scale=1,shadow xshift=.5ex,shadow yshift=.5ex,
      fill=\tikz@fillcolor,draw=\tikz@strokecolor,#1,
      shadow xshift/.expanded=2*\tikra@shadow{xshift}, shadow yshift/.expanded=2*\tikra@shadow{yshift}},
    general shadow={shadow scale=1,shadow xshift=.5ex,shadow yshift=.5ex,
      fill=\tikz@fillcolor,draw=\tikz@strokecolor,#1}},
  general shadow/.default=, drop shadow/.default=, circular drop shadow/.default=, circular glow/.default=,
  copy shadow/.default=, double copy shadow/.default=,
}
\tikzset{
  every delimiter/.style={}, every left delimiter/.style={}, every right delimiter/.style={},
  every above delimiter/.style={}, every below delimiter/.style={},
  left delimiter/.code={\tikzset{append after command={\tikra@delimiter{south east}{south west}
    {every delimiter,every left delimiter}{south}{north}{#1}{.}{y}}}},
  right delimiter/.code={\tikzset{append after command={\tikra@delimiter{south west}{south east}
    {every delimiter,every right delimiter}{south}{north}{.}{#1}{y}}}},
  above delimiter/.code={\tikzset{append after command={\tikra@delimiter{south east}{north west}
    {every delimiter,every above delimiter,rotate=-90}{west}{east}{#1}{.}{x}}}},
  below delimiter/.code={\tikzset{append after command={\tikra@delimiter{south west}{south west}
    {every delimiter,every below delimiter,rotate=-90}{west}{east}{.}{#1}{x}}}},
}
\def\tikra@delimiter#1#2#3#4#5#6#7#8{{\pgfextra{\global\let\tikra@delimited=\tikzlastnode}let \p1=($(\tikzlastnode.#5)-(\tikzlastnode.#4)$) in
  \pgfextra{\pgfmathsetmacro\tikra@size{abs(\csname #8\endcsname1)}\pgfmathsetmacro\tikra@raise{2.5-\tikra@size/2}}
  node[outer sep=0pt,inner sep=0pt,draw=none,fill=none,anchor=#1,at=(\tikzlastnode.#2),#3]
  {$\left#6\rule[\tikra@raise pt]{0pt}{\tikra@size pt}\right#7$}
  \pgfextra{\global\let\tikzlastnode=\tikra@delimited}}}
";

/// Shape keys with nothing set: every key at its default.
struct NoKeys;

impl super::shapes::Keys for NoKeys {
    fn length(&mut self, _: &str, default: f64) -> f64 {
        default
    }
    fn length_of(&mut self, _: &str, default: f64) -> f64 {
        default
    }
    fn number(&mut self, _: &str, default: f64) -> f64 {
        default
    }
    fn text(&mut self, _: &str) -> String {
        String::new()
    }
    fn flag(&mut self, _: &str) -> bool {
        false
    }
    fn point(&mut self, _: &str) -> Option<Point> {
        None
    }
}

/// A rectangle node over a box (`current bounding box`, `current path bounding box`), with its anchors.
pub fn box_node(lo: Point, hi: Point) -> NodeInfo {
    use super::shapes::{compute, BoxSize, Input};
    let parts = [("text".to_string(), BoxSize { wd: hi.x - lo.x, ht: hi.y - lo.y, dp: 0.0 })];
    let mut keys = NoKeys;
    let mut input = Input {
        parts: &parts,
        inner_xsep: 0.0,
        inner_ysep: 0.0,
        minimum_width: 0.0,
        minimum_height: 0.0,
        outer_xsep: 0.0,
        outer_ysep: 0.0,
        aspect: 1.0,
        ex: 0.0,
        line_width: 0.0,
        pointer: None,
        keys: &mut keys,
    };
    let geometry = compute("rectangle", &mut input).expect("a rectangle");
    NodeInfo { shape: "rectangle".into(), geometry: Rc::new(geometry), transform: Transform::translation(lo.x, lo.y) }
}

#[cfg(test)]
mod tests {
    use super::super::shapes::Keys;
    use super::*;

    /// The rectangle of a box node reads no key, so no input reaches these: each key is at its
    /// default, as it would be for a shape that asked.
    #[test]
    fn no_keys_are_defaults() {
        let mut k = NoKeys;
        assert_eq!(k.length("/pgf/inner xsep", 1.5), 1.5);
        assert_eq!(k.length_of("2cm", 0.25), 0.25);
        assert_eq!(k.number("/pgf/star points", 5.0), 5.0);
        assert_eq!(k.text("/pgf/cylinder end fill"), "");
        assert!(!k.flag("pgfshapeborderusesincircle"));
        assert_eq!(k.point("/pgf/at"), None);
    }
}
