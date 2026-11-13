//! pgfplots on the engine, as `pgfplots.code.tex` works: `\begin{axis}` opens a survey phase in
//! which `\addplot` computes and stores its coordinates and TikZ statements are put aside; at
//! `\end{axis}` the limits, the scaling and the ticks are computed, then the axis, the plots (in
//! the order they came, with the TikZ statements among them), the descriptions and the legend
//! are drawn ([`draw`]).

pub mod attime;
pub mod boxplot;
pub mod colorbar;
pub mod colormap;
pub mod contour;
pub mod cube;
pub mod dateplot;
pub mod draw;
pub mod fillbetween;
pub mod fpu;
pub mod graphics;
pub mod group;
pub mod keys;
pub mod legend;
pub mod marks;
pub mod mesh;
pub mod number;
pub mod patch;
pub mod plot;
pub mod polar;
pub mod scaling;
pub mod smith;
pub mod symbolic;
pub mod table;
pub mod ternary;
pub mod ticks;
pub mod visual;

use crate::tex::{self, Cat, Meaning, Token};
use crate::Error;

pub use draw::Frame;
pub use plot::{Coord, Plot};

use super::{tikra_tokens, Engine};

/// What the survey phase of an axis collected.
#[derive(Debug, Clone, Default)]
pub struct Axis {
    /// `axis`, `semilogxaxis`, `semilogyaxis`, `loglogaxis`.
    pub kind: String,
    /// Where `\begin{axis}` is in the code: the axis is drawn at its end.
    pub source: Option<usize>,
    /// Plots and TikZ statements, in order.
    pub entries: Vec<Entry>,
    /// `\addlegendentry` texts with their options.
    pub legend: Vec<LegendEntry>,
    /// The legend images: one per plot that is not forgotten, and `\addlegendimage`s, in order.
    pub images: Vec<Vec<Token>>,
    /// Plots that count (not `forget plot`): the index into the cycle list.
    pub numplots: usize,
    /// Whether a picture was opened for this axis (an `axis` outside `tikzpicture`).
    pub opened_picture: bool,
    /// The axis' options, as given.
    pub options: Vec<Token>,
    /// The environment's group depth, to close it at `\end{axis}`.
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub enum Entry {
    Plot(Box<Plot>),
    /// A TikZ statement (`\draw …;`, `\node …;`), run once the axis is set up, and its place in
    /// the code.
    Tikz(Vec<Token>, Option<usize>),
}

#[derive(Debug, Clone)]
pub struct LegendEntry {
    pub options: Vec<Token>,
    pub text: Vec<Token>,
}

/// Ids of pgfplots' commands (`Meaning::Native`), between the basic layer's and the keys.
pub const BASE: u16 = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum C {
    Addplot,
    Addlegendentry,
    Addlegendentryexpanded,
    Legend,
    Addlegendimage,
    Pgfplotsextra,
    Nextgroupplot,
    Closedcycle,
    Pgfplotsinvokeforeach,
    Pgfplotstableread,
    Colormaptodatafile,
    Colormaptoshadingspec,
    Tikzfillbetween,
    /// the fillbetween library's commands on soft paths
    Softpath,
    Pgfplotstabletypeset,
    /// `\pgfplotstransformcoordinatex{v}`: the value in the axis' transformed units
    Transformcoordinate,
}

const COMMANDS: &[(&str, C)] = &[
    ("addplot", C::Addplot),
    ("addlegendentry", C::Addlegendentry),
    ("addlegendentryexpanded", C::Addlegendentryexpanded),
    ("legend", C::Legend),
    ("addlegendimage", C::Addlegendimage),
    ("pgfplotsextra", C::Pgfplotsextra),
    ("nextgroupplot", C::Nextgroupplot),
    ("closedcycle", C::Closedcycle),
    ("pgfplotsinvokeforeach", C::Pgfplotsinvokeforeach),
    ("pgfplotstableread", C::Pgfplotstableread),
    ("pgfplotscolormaptodatafile", C::Colormaptodatafile),
    ("pgfplotscolormaptoshadingspec", C::Colormaptoshadingspec),
    ("tikzfillbetween", C::Tikzfillbetween),
    ("pgfplotstabletypeset", C::Pgfplotstabletypeset),
    ("tikzgetnamedpath", C::Softpath),
    ("pgfplotspathaxisoutline", C::Softpath),
    ("pgfcomputeintersectionsegments", C::Softpath),
    ("pgfgetintersectionsegmentpath", C::Softpath),
    ("pgfsetpathandBB", C::Softpath),
    ("pgfaddpathandBB", C::Softpath),
    ("pgfpathreplacefirstmoveto", C::Softpath),
    ("pgfplotstransformcoordinatex", C::Transformcoordinate),
    ("pgfplotstransformcoordinatey", C::Transformcoordinate),
    ("pgfplotstransformcoordinatez", C::Transformcoordinate),
];

/// The environments that are axes.
pub fn is_axis(name: &str) -> bool {
    matches!(name, "axis" | "semilogxaxis" | "semilogyaxis" | "loglogaxis" | "polaraxis" | "smithchart" | "ternaryaxis")
}

/// Defines pgfplots' commands and keys.
pub fn define(e: &mut Engine) {
    for (i, (name, _)) in COMMANDS.iter().enumerate() {
        e.tex.define(name, Meaning::Native(BASE + i as u16));
    }
    keys::define(e);
}

pub fn is_command(id: u16) -> bool {
    (BASE..BASE + COMMANDS.len() as u16).contains(&id)
}

/// Commands a survey keeps for later (TikZ statements inside an axis).
pub fn is_deferred(name: &str) -> bool {
    matches!(name, "draw" | "fill" | "filldraw" | "path" | "node" | "coordinate" | "clip" | "shade" | "shadedraw" | "pattern" | "matrix" | "pic" | "useasboundingbox")
}

impl Engine {
    /// `\begin{axis}[options]`: the survey starts.
    pub fn begin_axis(&mut self, kind: &str) -> Result<(), Error> {
        let o = self.optional()?.unwrap_or_default();
        self.begin_axis_with(kind, o)
    }

    /// An axis with these options (`\nextgroupplot` makes them).
    pub fn begin_axis_with(&mut self, kind: &str, o: Vec<Token>) -> Result<(), Error> {
        if self.axis.is_some() {
            self.warn("nested axis environments are not supported (pgfplots refuses them too): the inner one is drawn on its own");
        }
        let opened = self.picture_depth == 0;
        if opened {
            self.begin_picture(None)?;
        }
        self.begin_group();
        dateplot::reset(self);
        boxplot::reset();
        let mut axis = Axis { kind: kind.to_string(), source: self.source, opened_picture: opened, depth: self.tex.group_depth(), ..Default::default() };
        let style = match kind {
            "polaraxis" => "enlarge x limits=false,every polar axis",
            "smithchart" => "every smithchart axis",
            "ternaryaxis" => {
                "enlargelimits=false,xmin=0,xmax=1,ymin=0,ymax=1,zmin=0,zmax=1,\
                 footnotesize/.append style={every axis title shift=15pt},tiny/.append style={every axis title shift=15pt},every ternary axis"
            }
            "semilogxaxis" => "every semilogx axis,xmode=log",
            "semilogyaxis" => "every semilogy axis,ymode=log",
            "loglogaxis" => "every loglog axis,xmode=log,ymode=log",
            _ => "every linear axis",
        };
        self.st.key_path = "/pgfplots".into();
        keys::run(self, "every axis/.try")?;
        keys::run(self, style)?;
        let saved = self.st.key_path.clone();
        axis.options = o.clone();
        // pgfplots' axis types add `data cs` after the options (`\pgfplotsdefineaxistype`)
        let o = if kind == "polaraxis" {
            let mut o = o;
            o.extend(toks(",data cs=polar"));
            o
        } else {
            o
        };
        keys::set(self, "/pgfplots/@axis type", if kind == "polaraxis" { "polar" } else { "cart" });
        self.process_keys(o, Some("/pgfplots")).map_err(|err| Error::Figure(format!("axis options: {err}")))?;
        if kind == "smithchart" {
            // the ticks by size, unless given
            let width = keys::length(self, "/pgfplots/width").unwrap_or(207.0 * crate::PT);
            keys::run(self, smith::size_style(width))?;
            let given = [keys::value(self, "/pgfplots/xtick").is_empty(), keys::value(self, "/pgfplots/ytick").is_empty()];
            if given[0] {
                keys::run(self, "default smithchart xtick")?;
            }
            if given[1] {
                keys::run(self, "default smithchart ytick")?;
            }
            if given[0] && given[1] {
                keys::run(self, "default smithchart xytick")?;
            }
            keys::set(self, "/pgfplots/is smithchart cs", "false");
        }
        if kind == "ternaryaxis" {
            // percentages: whole numbers, never scaled
            if keys::flag(self, "/pgfplots/ternary limits relative") {
                keys::run(self, "scaled ticks=false,ticklabel style={/pgf/number format/precision=0}")?;
            }
            for c in ['x', 'y', 'z'] {
                if keys::value(self, &format!("/pgfplots/{c} dir")) == "reverse" {
                    self.warn(format!("ternaryaxis: '{c} dir=reverse' not supported: drawn forward"));
                }
            }
        }
        self.st.key_path = saved;
        axis.numplots = 0;
        if let Some(code) = keys::value_tokens(self, "/pgfplots/execute at begin axis") {
            self.run_tokens(code)?;
        }
        self.axis = Some(Box::new(axis));
        Ok(())
    }

    /// `\end{axis}`: everything is drawn.
    pub fn end_axis(&mut self) -> Result<(), Error> {
        if self.axis.is_none() {
            return Ok(());
        }
        // still in the axis: its code can add plots and use `axis cs`
        if let Some(code) = keys::value_tokens(self, "/pgfplots/execute at end axis") {
            self.run_tokens(code)?;
        }
        let Some(axis) = self.axis.take() else { return Ok(()) };
        self.mark_source(axis.source);
        let r = draw::draw_axis(self, &axis);
        // close the groups the body left open, then the axis' own
        while self.tex.group_depth() > axis.depth {
            self.end_group();
        }
        self.end_group();
        let r = match r {
            Ok(Some(cb)) => self.draw_colorbar(&cb),
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        };
        if axis.opened_picture {
            self.end_picture();
        }
        r
    }

    /// pgfplots' commands.
    pub fn pgfplots_command(&mut self, id: u16) -> Result<(), Error> {
        let cmd = COMMANDS[(id - BASE) as usize].1;
        if self.axis.is_none()
            && !matches!(
                cmd,
                C::Pgfplotsinvokeforeach
                    | C::Closedcycle
                    | C::Nextgroupplot
                    | C::Pgfplotstableread
                    | C::Colormaptodatafile
                    | C::Colormaptoshadingspec
                    | C::Tikzfillbetween
                    | C::Softpath
                    | C::Pgfplotstabletypeset
                    | C::Transformcoordinate
            )
        {
            let rest = self.skip_statement()?;
            let name = COMMANDS[(id - BASE) as usize].0;
            self.warn(format!("\\{name} outside an axis: '{}' ignored", rest.chars().take(30).collect::<String>().trim()));
            return Ok(());
        }
        match cmd {
            C::Addplot => {
                let r = plot::addplot(self);
                if let Err(err) = r {
                    if super::is_limit(&err) {
                        return Err(err);
                    }
                    self.warn(format!("\\addplot: {err}; plot ignored"));
                    let _ = self.skip_statement();
                }
                Ok(())
            }
            C::Addlegendentry | C::Addlegendentryexpanded => {
                let options = self.optional()?.unwrap_or_default();
                let mut text = self.argument()?;
                if cmd == C::Addlegendentryexpanded {
                    text = self.tex.expand_fully(text)?;
                }
                if let Some(a) = self.axis.as_mut() {
                    a.legend.push(LegendEntry { options, text });
                }
                Ok(())
            }
            C::Legend => {
                let _ = self.optional()?;
                let list = self.argument()?;
                let entries = split_legend(&list);
                if let Some(a) = self.axis.as_mut() {
                    a.legend = entries;
                }
                Ok(())
            }
            C::Addlegendimage => {
                let o = self.argument()?;
                if let Some(a) = self.axis.as_mut() {
                    a.images.push(o);
                }
                Ok(())
            }
            C::Pgfplotsextra => {
                let code = self.argument()?;
                let source = self.source;
                if let Some(a) = self.axis.as_mut() {
                    a.entries.push(Entry::Tikz(code, source));
                }
                Ok(())
            }
            C::Nextgroupplot => self.next_group_plot(),
            C::Closedcycle => Ok(()),
            C::Pgfplotstableread => plot::table_read(self),
            C::Pgfplotstabletypeset => plot::table_typeset(self),
            C::Colormaptodatafile => colormap::to_data_file(self),
            C::Colormaptoshadingspec => colormap::to_shading_spec(self),
            // `\tikzfillbetween[of=A and B, …]{style}`: the area between two named paths
            C::Tikzfillbetween => {
                let spec = self.optional()?.unwrap_or_default();
                let style = self.argument()?;
                let plot = Plot { style, ..Default::default() };
                self.begin_group();
                let r = fillbetween::draw(self, &plot, &spec);
                self.end_group();
                r
            }
            C::Softpath => fillbetween::soft_command(self, COMMANDS[(id - BASE) as usize].0),
            C::Transformcoordinate => {
                // the logarithm on a log axis, then the data scale transformation: what
                // `\pgfqpointxy` takes in the axis' units (`\pgfplots@coord@trafo@x`)
                let name = COMMANDS[(id - BASE) as usize].0;
                let a = match name.as_bytes()[name.len() - 1] {
                    b'x' => 0,
                    b'y' => 1,
                    _ => 2,
                };
                let arg = self.argument()?;
                let text = self.tex.expand_to_string(arg)?;
                let v = self.number(&text)?;
                let t = match self.frame.clone() {
                    Some(f) => {
                        let (factor, shift) = draw::data_scale_trafo(self, &f, a);
                        (f.logical(a, v) - shift) * factor
                    }
                    None => v,
                };
                self.tex.define_string("pgfmathresult", &super::pgf_number(t), false);
                Ok(())
            }
            C::Pgfplotsinvokeforeach => {
                // \pgfplotsinvokeforeach{list}{code with #1}
                let list = self.argument()?;
                let body = self.argument()?;
                let list = self.tex.expand_fully(list)?;
                let parsed = tex::parse_body(body);
                for item in super::foreach_items(&list) {
                    let code = tex::substitute(&parsed, &[item]);
                    self.run_tokens(code)?;
                }
                Ok(())
            }
        }
    }

    /// A TikZ statement inside an axis, kept until the axis is drawn: the command and its tokens up
    /// to the `;`.
    pub fn defer_statement(&mut self, t: &Token) -> Result<(), Error> {
        let mut out = vec![t.clone()];
        let mut depth = 0usize;
        while let Some(x) = self.tex.get_token() {
            match x {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => {
                    if depth == 0 {
                        self.tex.back(x);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            let end = depth == 0 && x.is_char(';');
            out.push(x);
            if end || out.len() > tex::MAX_TOKENS {
                break;
            }
        }
        let source = self.source;
        if let Some(a) = self.axis.as_mut() {
            a.entries.push(Entry::Tikz(out, source));
        }
        Ok(())
    }
}

/// `\legend{a,b,{c,d}}`: entries split at top-level commas (or `\\`).
fn split_legend(list: &[Token]) -> Vec<LegendEntry> {
    let backslashes = list.iter().any(|t| t.is_cs("\\"));
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut depth = 0usize;
    for t in list {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            _ => {}
        }
        let sep = depth == 0 && if backslashes { t.is_cs("\\") } else { t.is_char(',') };
        if sep {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(t.clone());
    }
    if !crate::keys::trim(&cur).is_empty() || (!backslashes && !out.is_empty()) {
        out.push(cur);
    }
    out.into_iter()
        .map(|e| {
            let e = crate::keys::trim(&e).to_vec();
            let (options, text) = split_options(e);
            LegendEntry { options, text: tex::strip_outer_braces(text) }
        })
        .collect()
}

/// `[options]text` → (options, text).
pub fn split_options(v: Vec<Token>) -> (Vec<Token>, Vec<Token>) {
    if v.first().is_some_and(|t| t.is_char('[')) {
        let mut depth = 0usize;
        for (i, t) in v.iter().enumerate() {
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                Token::Char(']', _) if depth == 0 => return (v[1..i].to_vec(), crate::keys::trim(&v[i + 1..]).to_vec()),
                _ => {}
            }
        }
    }
    (Vec::new(), v)
}

/// Tokens of `text`, read as TeX with `@` a letter.
pub fn toks(text: &str) -> Vec<Token> {
    tikra_tokens(text)
}
