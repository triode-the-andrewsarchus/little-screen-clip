//! The turtle library (`tikzlibraryturtle`): `[turtle={home, forward, right, forward=2cm, left=30}]`
//! in a path's options inserts path operations as a turtle walks. The turtle has a direction
//! (90, up, at `home`, kept from one path to the next); `forward` (`fd`) adds a `to` of `distance`
//! in that direction, with the `how` options (`how/.style={bend left}`), `back` (`bk`) one the
//! other way, and `left` (`lt`) and `right` (`rt`) turn it, by 90 degrees unless told.
//!
//! The steps go in with `insert path`, so the direction is read when the key runs.

/// The keys, as tikra's TikZ styles.
pub const DEFINITIONS: &str = r"
\def\tikra@turtle@dir{90}
\def\tikra@insertpath{}
\tikzset{
  insert path/.code={\expandafter\def\expandafter\tikra@insertpath\expandafter{\tikra@insertpath #1}},
  turtle/.code={\pgfqkeys{/tikz/turtle}{#1}},
  turtle/direction/.code={\gdef\tikra@turtle@dir{#1}},
  turtle/distance/.initial=1cm,
  turtle/how/.style={},
  turtle/home/.code={\tikzset{insert path={(0,0)}}\gdef\tikra@turtle@dir{90}},
  turtle/forward/.default=\pgfkeysvalueof{/tikz/turtle/distance},
  turtle/forward/.code={\edef\tikra@temp{\noexpand\tikzset{insert path={to[/tikz/turtle/how] ++(\tikra@turtle@dir:{#1})}}}\tikra@temp},
  turtle/back/.default=\pgfkeysvalueof{/tikz/turtle/distance},
  turtle/back/.style={/tikz/turtle/forward={-(#1)}},
  turtle/left/.default=90,
  turtle/left/.code={\pgfmathparse{(#1)+(\tikra@turtle@dir)}\ifdim\pgfmathresult pt>360pt\relax\pgfmathparse{\pgfmathresult-360}\fi
    \global\let\tikra@turtle@dir\pgfmathresult},
  turtle/right/.default=90,
  turtle/right/.code={\pgfmathparse{(\tikra@turtle@dir)-(#1)}\ifdim\pgfmathresult pt<0pt\relax\pgfmathparse{\pgfmathresult+360}\fi
    \global\let\tikra@turtle@dir\pgfmathresult},
  turtle/fd/.style={/tikz/turtle/forward={#1}}, turtle/fd/.default=\pgfkeysvalueof{/tikz/turtle/distance},
  turtle/bk/.style={/tikz/turtle/back={#1}}, turtle/bk/.default=\pgfkeysvalueof{/tikz/turtle/distance},
  turtle/lt/.style={/tikz/turtle/left={#1}}, turtle/lt/.default=90,
  turtle/rt/.style={/tikz/turtle/right={#1}}, turtle/rt/.default=90,
}
";

#[cfg(test)]
mod tests {
    #[test]
    fn the_turtle_walks() {
        let (canvas, warnings) = super::super::run_document(r"\tikz[turtle/distance=2mm] \draw [turtle={home,forward,right,forward,left,fd,lt,fd}];").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let (lo, hi) = (canvas.min.unwrap(), canvas.max.unwrap());
        // up, right, up, left: 0.2 cm wide, 0.4 cm high (and the line's width)
        assert!((hi.x - lo.x - 0.2).abs() < 0.05 && (hi.y - lo.y - 0.4).abs() < 0.05, "{lo:?} {hi:?}");
    }
}
