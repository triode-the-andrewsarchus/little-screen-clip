//! The groupplots library (`tikzlibrarypgfplots.groupplots.code.tex`): a `groupplot` environment
//! holds a grid of axes, each begun by `\nextgroupplot`. Plot `cXrY` is named `group cXrY`; the
//! first of a row stands below the one above it (`at=(above.south) - (0, vertical sep)`,
//! `anchor=north`), the others right of their left neighbour (`at=(left.east) + (horizontal sep, 0)`,
//! `anchor=west`). `xlabels at`, `xticklabels at` and their friends remove the labels of inner
//! plots, as `\pgfplots@group@determine@labels` does.

use crate::tex::Token;
use crate::Error;

use super::keys;
use super::{toks, Engine};

#[derive(Debug, Clone, Default)]
pub struct Group {
    pub columns: usize,
    pub rows: usize,
    /// The plots begun, and where the last one is (1-based).
    pub current: usize,
    pub row: usize,
    pub column: usize,
    pub name: String,
    /// The environment's options that are not `group style`: every plot gets them.
    pub options: Vec<Token>,
    pub opened_picture: bool,
    pub depth: usize,
}

impl Engine {
    /// `\begin{groupplot}[options]`
    pub fn begin_groupplot(&mut self) -> Result<(), Error> {
        let o = self.optional()?.unwrap_or_default();
        if self.plot_group.is_some() || self.axis.is_some() {
            self.warn("groupplot inside a groupplot or an axis: pgfplots refuses it; drawn on its own");
        }
        let opened = self.picture_depth == 0;
        if opened {
            self.begin_picture(None)?;
        }
        self.begin_group();
        let depth = self.tex.group_depth();
        // `group style={…}` is set now, the rest goes to every plot
        let mut rest = Vec::new();
        let saved = self.st.key_path.clone();
        self.st.key_path = "/pgfplots".into();
        for item in crate::keys::split_list(&o) {
            if item.key.trim_start_matches("/pgfplots/") == "group style" {
                if let Some(v) = item.value {
                    self.process_keys(v, Some("/pgfplots/group"))?;
                }
                continue;
            }
            if !rest.is_empty() {
                rest.push(Token::other(','));
            }
            rest.extend(crate::tex::string_tokens(&item.key));
            if let Some(v) = item.value {
                rest.push(Token::other('='));
                rest.push(Token::Char('{', crate::tex::Cat::Begin));
                rest.extend(v);
                rest.push(Token::Char('}', crate::tex::Cat::End));
            }
        }
        self.st.key_path = saved;
        let number = |e: &mut Engine, k: &str| keys::number(e, &format!("/pgfplots/group/{k}")).unwrap_or(1.0).max(1.0) as usize;
        let columns = number(self, "columns");
        let rows = number(self, "rows");
        let name = keys::value(self, "/pgfplots/group/group name");
        let name = if name.is_empty() { "group".to_string() } else { name };
        self.plot_group = Some(Box::new(Group { columns, rows, current: 0, row: 1, column: 0, name, options: rest, opened_picture: opened, depth }));
        Ok(())
    }

    /// `\end{groupplot}`: the last plot is drawn.
    pub fn end_groupplot(&mut self) -> Result<(), Error> {
        let r = if self.axis.is_some() { self.end_axis() } else { Ok(()) };
        let Some(g) = self.plot_group.take() else { return r };
        while self.tex.group_depth() > g.depth {
            self.end_group();
        }
        self.end_group();
        if g.opened_picture {
            self.end_picture();
        }
        r
    }

    /// `\nextgroupplot[options]`: the plot before is drawn, the next one begins.
    pub fn next_group_plot(&mut self) -> Result<(), Error> {
        let o = self.optional()?.unwrap_or_default();
        let Some(mut g) = self.plot_group.take() else {
            self.warn("\\nextgroupplot outside a groupplot environment: ignored");
            return Ok(());
        };
        if g.current >= g.columns * g.rows {
            self.warn(format!("groupplots: more plots than the group size ({} by {}): this one is ignored", g.columns, g.rows));
            self.plot_group = Some(g);
            return Ok(());
        }
        if g.current > 0 && self.axis.is_some() {
            self.plot_group = Some(g.clone());
            self.end_axis()?;
            self.plot_group = None;
        }
        g.current += 1;
        if g.column == g.columns {
            if g.current != 1 {
                g.row += 1;
            }
            g.column = 1;
        } else {
            g.column += 1;
        }
        let (c, r) = (g.column, g.row);
        let mut list = g.options.clone();
        let mut add = |s: &str| {
            if !list.is_empty() {
                list.push(Token::other(','));
            }
            list.extend(toks(s));
        };
        add(&format!("name={{{} c{c}r{r}}},/pgfplots/group/every plot", g.name));
        add(&cell_style(self, &g, c, r));
        let hsep = keys::value(self, "/pgfplots/group/horizontal sep");
        let vsep = keys::value(self, "/pgfplots/group/vertical sep");
        if r == 1 && c > 1 {
            add(&format!("at={{($({} c{}r{r}.east)+({hsep},0)$)}},anchor=west", g.name, c - 1));
        } else if r > 1 {
            add(&format!("at={{($({} c{c}r{}.south)-(0,{vsep})$)}},anchor=north", g.name, r - 1));
        }
        if !o.is_empty() {
            list.push(Token::other(','));
            list.extend(o);
        }
        self.plot_group = Some(g);
        self.begin_axis_with("axis", list)
    }
}

/// What `xlabels at`, `ylabels at`, `xticklabels at` and `yticklabels at` take from plot `cXrY`.
fn cell_style(e: &mut Engine, g: &Group, c: usize, r: usize) -> String {
    let v = |e: &mut Engine, k: &str| keys::value(e, &format!("/pgfplots/group/{k}"));
    let mut out: Vec<&str> = Vec::new();
    let (last_row, last_col) = (r == g.rows, c == g.columns);
    match v(e, "xticklabels at").as_str() {
        "edge top" if r == 1 => out.push("xticklabel pos=upper"),
        "edge top" => out.push("/pgfplots/@empty xticklabels=true"),
        "edge bottom" if last_row => out.push("xticklabel pos=lower"),
        "edge bottom" => out.push("/pgfplots/@empty xticklabels=true"),
        _ => {}
    }
    match v(e, "yticklabels at").as_str() {
        "edge left" if c == 1 => out.push("yticklabel pos=left"),
        "edge left" => out.push("/pgfplots/@empty yticklabels=true"),
        "edge right" if last_col => out.push("yticklabel pos=right"),
        "edge right" => out.push("/pgfplots/@empty yticklabels=true"),
        _ => {}
    }
    match v(e, "xlabels at").as_str() {
        "edge bottom" if g.rows > 1 && !last_row => out.push("xlabel={}"),
        "edge top" if g.rows > 1 && r == 1 => out.push("xticklabel pos=upper"),
        "edge top" if g.rows > 1 => out.push("xlabel={}"),
        _ => {}
    }
    match v(e, "ylabels at").as_str() {
        "edge left" if g.columns > 1 && c > 1 => out.push("ylabel={}"),
        "edge right" if g.columns > 1 && last_col => out.push("yticklabel pos=upper"),
        "edge right" if g.columns > 1 => out.push("ylabel={}"),
        _ => {}
    }
    let mut s = out.join(",");
    // `group/plot cXrY/.style`, if someone set it
    let key = format!("/pgfplots/group/plot c{c}r{r}");
    if e.key_exists(&key) {
        if !s.is_empty() {
            s.push(',');
        }
        s.push_str(&key);
    }
    s
}
