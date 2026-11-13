//! Matrices: `\matrix[options] (m) at (point) { cell & cell \\ cell & cell \\ };`, as pgf's matrix
//! module lays them out. Each cell is drawn once alone to measure how far it reaches left, right,
//! above and below its origin; columns and rows line the origins up with `column sep` and
//! `row sep` between the cells' borders; the matrix is then a node (drawn, named, anchored) and the
//! cells are drawn again in place. `matrix of nodes` makes each cell a node named `m-r-c`, with
//! `|[options]|` for that cell's options.

use std::collections::HashMap;
use std::rc::Rc;

use crate::scene::{Point, Transform};
use crate::tex::{self, Cat, Meaning, Token};
use crate::Error;

use super::nodes::{NodeSpec, OnPath};
use super::{tikra_tokens, Engine};

/// A cell's extent from its origin: left, right, above, below (all ≥ 0 when it has ink).
#[derive(Debug, Clone, Copy, Default)]
struct Extent {
    left: f64,
    right: f64,
    up: f64,
    down: f64,
    empty: bool,
}

/// A matrix body split into rows of cells, with the separation each `&[spec]` and `\\[spec]`
/// adds before a cell and after a row.
struct Body {
    rows: Vec<Vec<Vec<Token>>>,
    /// for each cell, the `[spec]` after the `&` before it
    cell_seps: Vec<Vec<Vec<Token>>>,
    /// for each row, the `[spec]` after its `\\`
    row_seps: Vec<Vec<Token>>,
}

/// pgf's separation list (`1cm,between origins`, `2mm,-1mm`): lengths add up, and whether the
/// distance is between the cells' origins or (the default) their borders.
struct Sep {
    distance: f64,
    origins: bool,
}
impl Engine {
    /// The body split at `&` (or what `ampersand replacement` or `\pgfmatrixnextcell` stand for)
    /// and at `\\` (`\pgfmatrixendrow`).
    fn split_body(&self, body: &[Token], ampersand: Option<&str>) -> Body {
        let next_cell = self.tex.meaning(&Token::cs("pgfmatrixnextcell"));
        let end_row = self.tex.meaning(&Token::cs("pgfmatrixendrow"));
        let mut out = Body { rows: vec![vec![Vec::new()]], cell_seps: vec![vec![Vec::new()]], row_seps: vec![Vec::new()] };
        let mut depth = 0usize;
        let mut i = 0;
        // `[spec]` right after a separator (spaces skipped), and where reading goes on
        let spec = |i: usize| -> (Vec<Token>, usize) {
            let mut j = i + 1;
            while j < body.len() && body[j].is_space() {
                j += 1;
            }
            if !body.get(j).is_some_and(|x| x.is_char('[')) {
                return (Vec::new(), i + 1);
            }
            let mut k = j + 1;
            let mut v = Vec::new();
            while k < body.len() && !body[k].is_char(']') {
                v.push(body[k].clone());
                k += 1;
            }
            (v, k + 1)
        };
        while i < body.len() {
            let t = &body[i];
            let cs = |name: &str| matches!(t, Token::Cs(n) if &**n == name);
            let is_cell =
                matches!(t, Token::Char(_, Cat::Align)) || matches!(t, Token::Cs(n) if Some(&**n) == ampersand) || (matches!(t, Token::Cs(_)) && self.tex.meaning(t) == next_cell);
            let is_row = cs("\\") || (matches!(t, Token::Cs(_)) && self.tex.meaning(t) == end_row);
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                _ if depth == 0 && is_cell => {
                    let (sep, next) = spec(i);
                    out.rows.last_mut().expect("a row").push(Vec::new());
                    out.cell_seps.last_mut().expect("a row").push(sep);
                    i = next;
                    continue;
                }
                _ if depth == 0 && is_row => {
                    let (sep, next) = spec(i);
                    *out.row_seps.last_mut().expect("a row") = sep;
                    out.rows.push(vec![Vec::new()]);
                    out.cell_seps.push(vec![Vec::new()]);
                    out.row_seps.push(Vec::new());
                    i = next;
                    continue;
                }
                _ => {}
            }
            out.rows.last_mut().expect("a row").last_mut().expect("a cell").push(t.clone());
            i += 1;
        }
        // a last row with only empty cells is the end of the body
        while out.rows.len() > 1 && out.rows.last().is_some_and(|r| r.iter().all(|c| crate::keys::trim(c).is_empty())) {
            out.rows.pop();
            out.cell_seps.pop();
            out.row_seps.pop();
        }
        out
    }

    /// The default separation followed by a cell's or row's own.
    fn sep(&mut self, default: &[Token], extra: &[Token]) -> Sep {
        let mut sep = Sep { distance: 0.0, origins: false };
        for list in [default, extra] {
            for item in crate::keys::split_list(list) {
                match item.key.as_str() {
                    "between origins" => sep.origins = true,
                    "between borders" => sep.origins = false,
                    "" => {}
                    k => match self.length(k) {
                        Ok(d) => sep.distance += d,
                        Err(_) => self.warn(format!("matrix separation '{k}' not understood: ignored")),
                    },
                }
            }
        }
        sep
    }

    /// `\pgfmatrix{shape}{anchor}{name}{usage}{shift}{pre code}{body}`: the basic layer's matrix,
    /// laid out as `\matrix` lays it out.
    pub fn pgf_matrix(&mut self) -> Result<(), Error> {
        let shape = self.argument_string()?;
        let anchor = self.argument_string()?;
        let name = self.argument_string()?;
        let usage = self.argument_string()?;
        let shift = self.point_arg()?;
        let pre = self.argument()?;
        let body = self.argument()?;
        self.begin_group();
        let r = (|| {
            self.run_tokens(pre)?;
            // the basic layer's node takes no TikZ style (`every node`): only its usage
            let mut options = format!("shape={shape},anchor={anchor},draw=none,fill=none");
            for (word, key) in [("stroke", "draw"), ("draw", "draw"), ("fill", "fill")] {
                if usage.contains(word) {
                    options.push_str(&format!(",{key}"));
                }
            }
            let at = self.st.transform.apply(shift);
            let body = self.expand_body_macros(body);
            let name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
            self.matrix_in_group(tikra_tokens(&options), name, Some(at), body)
        })();
        self.end_group();
        r
    }

    /// `\matrix` (the command already read) and `node[matrix]`.
    pub fn matrix(&mut self) -> Result<(), Error> {
        // options, (name), at (point), then the body in braces
        let mut options = Vec::new();
        let mut name = None;
        let mut at = None;
        loop {
            match self.tex.next_nonspace() {
                Some(t) if t.is_char('[') => {
                    if !options.is_empty() {
                        options.push(Token::other(','));
                    }
                    options.extend(self.tex.read_until_char(']', false)?);
                }
                Some(t) if t.is_char('(') => {
                    let n = super::coords::read_coordinate_tokens(self)?;
                    name = Some(self.tex.expand_to_string(n)?.trim().to_string());
                }
                Some(Token::Char('a', Cat::Letter)) => {
                    let _ = self.tex.get_token();
                    match self.tex.next_nonspace() {
                        Some(t) if t.is_char('(') => {
                            let c = super::coords::read_coordinate_tokens(self)?;
                            let text = self.math_string(c)?;
                            at = Some(self.coordinate(&text)?.p);
                        }
                        _ => return Err(Error::Figure("\\matrix: at without a point".into())),
                    }
                }
                Some(Token::Char(_, Cat::Begin)) => break,
                Some(t) => return Err(Error::Figure(format!("\\matrix: unexpected {}", tex::show_token(&t)))),
                None => return Err(Error::Figure("\\matrix without a body".into())),
            }
        }
        let body = self.tex.read_group_body()?;
        let body = self.expand_body_macros(body);
        // the statement ends with ;
        if let Some(t) = self.tex.next_nonspace() {
            if !t.is_char(';') {
                self.tex.back(t);
            }
        }
        self.begin_group();
        let r = self.matrix_in_group(options, name, at, body);
        self.end_group();
        r
    }

    fn matrix_in_group(&mut self, options: Vec<Token>, name: Option<String>, at: Option<Point>, body: Vec<Token>) -> Result<(), Error> {
        // the matrix's own options are read once to know the cells' settings; the node reads them again
        // `every node` as it is outside: the matrix's own node takes it before its options, as
        // any node does, and not the one its options set for the cells
        let every_node = Token::cs(&crate::keys::macro_name("/tikz/every node/.@cmd"));
        let outer_every_node = self.tex.meaning(&every_node);
        // what the surroundings set stays (`ampersand replacement`, the separations)
        let outer = std::mem::take(&mut self.st.matrix);
        self.st.matrix = MatrixSettings { ampersand: outer.ampersand, column_sep: outer.column_sep, row_sep: outer.row_sep, ..Default::default() };
        let saved_mode = self.st.mode.clone();
        self.process_keys(tikra_tokens("every matrix/.try"), Some("/tikz"))?;
        self.process_keys(options.clone(), Some("/tikz"))?;
        self.st.mode = saved_mode;
        // the basic layer's `\pgfmatrixemptycode`, `\pgfmatrixbegincode` and `\pgfmatrixendcode`,
        // what `execute at empty cell`, `execute at begin cell` and `execute at end cell` set
        for (code, m) in [("pgfmatrixemptycode", 0), ("pgfmatrixbegincode", 1), ("pgfmatrixendcode", 2)] {
            let target = match m {
                0 => &mut self.st.matrix.at_empty_cell,
                1 => &mut self.st.matrix.at_begin_cell,
                _ => &mut self.st.matrix.at_end_cell,
            };
            if target.is_empty() {
                if let Meaning::Macro(m) = self.tex.meaning_of(code) {
                    *target = tex::substitute(&m.body, &[]);
                }
            }
        }
        let settings = self.st.matrix.clone();
        let name = name.or_else(|| self.st.node.name.clone());
        let at = at.or(self.st.node.at).unwrap_or_else(|| self.st.transform.apply(Point::default()));
        let Body { rows, cell_seps, row_seps } = self.split_body(&body, settings.ampersand.as_deref());
        let nrows = rows.len();
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);

        // `matrix anchor=inner node.south` (or `anchor=…`): a cell's node is where the matrix goes
        let anchor = self.key_text("/tikz/anchor");
        let inner = settings.anchor.as_deref().or(Some(anchor.as_str())).and_then(|a| a.split_once('.')).map(|(n, a)| (n.trim().to_string(), a.trim().to_string()));
        if let Some((n, _)) = &inner {
            self.nodes.remove(n);
        }
        // the cell of that node and its anchor there
        let mut inner_at: Option<(usize, usize, Point)> = None;
        // first pass: measure every cell alone
        let mut extents = vec![vec![Extent { empty: true, ..Default::default() }; ncols]; nrows];
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let saved = std::mem::take(&mut self.canvas);
                let frozen = self.canvas_frozen;
                self.canvas_frozen = false;
                let clipped: Vec<bool> = self.scopes.iter_mut().map(|s| std::mem::take(&mut s.clipped)).collect();
                let res = self.draw_cell(name.as_deref(), &settings, r + 1, c + 1, cell, Transform::default());
                let drawn = std::mem::replace(&mut self.canvas, saved);
                self.canvas_frozen = frozen;
                for (s, c) in self.scopes.iter_mut().zip(clipped) {
                    s.clipped = c;
                }
                res?;
                if let (Some((n, a)), None) = (&inner, inner_at) {
                    if let Some(p) = self.nodes.get(n).and_then(|i| i.anchor(a)) {
                        inner_at = Some((r, c, p));
                    }
                }
                if let (Some(lo), Some(hi)) = (drawn.min, drawn.max) {
                    extents[r][c] = Extent { left: (-lo.x).max(0.0), right: hi.x.max(0.0), up: hi.y.max(0.0), down: (-lo.y).max(0.0), empty: false };
                }
            }
        }
        let left: Vec<f64> = (0..ncols).map(|c| extents.iter().map(|row| if row[c].empty { 0.0 } else { row[c].left }).fold(0.0, f64::max)).collect();
        let right: Vec<f64> = (0..ncols).map(|c| extents.iter().map(|row| if row[c].empty { 0.0 } else { row[c].right }).fold(0.0, f64::max)).collect();
        let up: Vec<f64> = extents.iter().map(|row| row.iter().filter(|e| !e.empty).map(|e| e.up).fold(0.0, f64::max)).collect();
        let down: Vec<f64> = extents.iter().map(|row| row.iter().filter(|e| !e.empty).map(|e| e.down).fold(0.0, f64::max)).collect();
        // origins: columns from the left, rows from the top (the first row's baseline at y = 0)
        let mut xs = Vec::with_capacity(ncols);
        for c in 0..ncols {
            if c == 0 {
                xs.push(left[0]);
                continue;
            }
            // what the first row to reach this column asks for (pgf sets a column's separation
            // when a row makes the column): between borders, or between origins
            let extra = rows.iter().zip(&cell_seps).find(|(row, _)| row.len() > c).and_then(|(_, seps)| seps.get(c).cloned()).unwrap_or_default();
            let sep = self.sep(&settings.column_sep, &extra);
            let step = if sep.origins { sep.distance } else { right[c - 1] + sep.distance + left[c] };
            xs.push(xs[c - 1] + step);
        }
        let mut ys = Vec::with_capacity(nrows);
        for r in 0..nrows {
            if r == 0 {
                ys.push(0.0);
                continue;
            }
            let sep = self.sep(&settings.row_sep, &row_seps[r - 1]);
            let step = if sep.origins { sep.distance } else { down[r - 1] + sep.distance + up[r] };
            ys.push(ys[r - 1] - step);
        }
        // the matrix's baseline is its last row's, as a \vbox's
        let last = *ys.last().expect("a matrix has a row at least");
        for y in &mut ys {
            *y -= last;
        }
        let width = if ncols == 0 { 0.0 } else { xs[ncols - 1] + right[ncols - 1] };
        let height = up.first().map_or(0.0, |u| u + ys[0]);
        let depth = down.last().copied().unwrap_or(0.0);

        // the matrix is a node around the cells
        self.forced_box = Some((width, height, depth));
        let cells_every_node = self.tex.meaning(&every_node);
        self.tex.set_meaning(&every_node, outer_every_node.clone(), false);
        let mut options = options;
        let mut at = at;
        match (&settings.anchor, inner_at) {
            (_, Some((r, c, p))) if inner.is_some() => {
                // the matrix's center where it puts that point of the cell at `at` (the box's
                // center, from its baseline's left end)
                let center = Point::new(width / 2.0, (height - depth) / 2.0);
                at = at.plus(center).minus(Point::new(xs[c], ys[r])).minus(p);
                options.extend(tikra_tokens(",anchor=center"));
            }
            (Some(a), _) if inner.is_none() => options.extend(tikra_tokens(&format!(",anchor={a}"))),
            _ => {}
        }
        let items =
            self.node(NodeSpec { options, name: name.clone(), at: Some(at), text: Vec::new(), place: OnPath::Point(at), point: at, coordinate: false, default_pos: None })?;
        self.forced_box = None;
        // what comes after the matrix's node (`append after command`, the delimiters), drawn after the cells
        let after = std::mem::take(&mut self.after_node_path);
        let matrix_node = self.last_node.clone();
        self.tex.set_meaning(&every_node, cells_every_node, false);
        self.canvas.items.extend(items);
        let placed = match &name {
            Some(n) => self.nodes.get(n).map(|i| i.transform),
            None => self.last_node.as_ref().map(|i| i.transform),
        }
        .unwrap_or_default();
        // second pass: the cells in place
        // TeX sets a cell in a box at the origin and moves the box afterwards: in the cell, a named
        // point is where it is in the cell's own coordinates. A point from outside the matrix moves
        // with the cell; one made in another cell is where it was in that cell, moved with this one.
        let mut made_in: HashMap<String, Point> = HashMap::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let t = placed.compose(&Transform::translation(xs[c], ys[r]));
                let origin = t.apply(Point::default());
                let mut moved = HashMap::with_capacity(self.nodes.len());
                for (n, info) in self.nodes.iter_mut() {
                    let d = origin.minus(made_in.get(n).copied().unwrap_or_default());
                    moved.insert(n.clone(), (Rc::clone(&info.geometry), info.transform));
                    info.transform = Transform::translation(d.x, d.y).compose(&info.transform);
                }
                let res = self.draw_cell(name.as_deref(), &settings, r + 1, c + 1, cell, t);
                // the points from before back where they were; the ones the cell made stay
                for (n, info) in self.nodes.iter_mut() {
                    match moved.remove(n) {
                        Some((g, before)) if Rc::ptr_eq(&g, &info.geometry) => info.transform = before,
                        _ => {
                            made_in.insert(n.clone(), origin);
                        }
                    }
                }
                res?;
            }
        }
        if !after.is_empty() {
            let node = match &name {
                Some(n) => n.clone(),
                None => {
                    let n = format!("tikra@fig@{}", self.steps);
                    if let Some(info) = matrix_node {
                        self.nodes.insert(n.clone(), info);
                    }
                    n
                }
            };
            self.tex.define_string("tikzlastnode", &node, false);
            // the delimiters are nodes of the picture, not cells: `nodes={…}` is not theirs
            self.tex.set_meaning(&every_node, outer_every_node, false);
            let mut path = tikra_tokens("\\path ");
            path.extend(after);
            path.extend(tikra_tokens(";"));
            self.run_tokens(path)?;
        }
        Ok(())
    }

    /// The body with the macros at its top level (`{ \matrixcontent }`) replaced by what they hold,
    /// as TeX's alignment expands them while it looks for `&` and `\\`.
    fn expand_body_macros(&self, body: Vec<Token>) -> Vec<Token> {
        let mut out = Vec::new();
        let mut todo: Vec<Token> = body.into_iter().rev().collect();
        let mut budget = 1000;
        // the separators stay, to be found by what they mean
        let separators = [self.tex.meaning(&Token::cs("pgfmatrixnextcell")), self.tex.meaning(&Token::cs("pgfmatrixendrow"))];
        while let Some(t) = todo.pop() {
            if let (Token::Cs(_), tex::Meaning::Macro(m)) = (&t, self.tex.meaning(&t)) {
                if !separators.contains(&tex::Meaning::Macro(m.clone())) && m.params.is_empty() && m.optional.is_none() && budget > 0 {
                    budget -= 1;
                    for b in m.body.iter().rev() {
                        if let tex::Body::Tok(x) = b {
                            todo.push(x.clone());
                        }
                    }
                    continue;
                }
            }
            out.push(t);
        }
        out
    }

    /// One cell, drawn with its origin at `place`.
    fn draw_cell(&mut self, matrix: Option<&str>, settings: &MatrixSettings, r: usize, c: usize, cell: &[Token], place: Transform) -> Result<(), Error> {
        let mut cell = crate::keys::trim(cell).to_vec();
        let empty = cell.is_empty();
        let empty_node = empty && settings.of_nodes && settings.nodes_in_empty_cells && settings.at_empty_cell.is_empty();
        if empty && !empty_node {
            if settings.at_empty_cell.is_empty() {
                return Ok(());
            }
            cell = settings.at_empty_cell.clone();
        }
        let empty = empty && !empty_node;
        self.begin_group();
        self.begin_scope();
        let res = (|| -> Result<(), Error> {
            self.st.transform = place;
            self.st.pending = Transform::default();
            self.tex.define_string("pgfmatrixcurrentrow", &r.to_string(), false);
            self.tex.define_string("pgfmatrixcurrentcolumn", &c.to_string(), false);
            let parity = |n: usize| if n.is_multiple_of(2) { "even" } else { "odd" };
            // `matrix of nodes` sets `cells={anchor=base}`: the nodes' styles may change it
            let mut styles = if settings.of_nodes { "anchor=base,".to_string() } else { String::new() };
            styles.push_str(&format!("every cell/.try,every {} row/.try,every {} column/.try,row {r}/.try,column {c}/.try,row {r} column {c}/.try", parity(r), parity(c)));
            self.process_keys(tikra_tokens(&styles), Some("/tikz"))?;
            self.st.apply_pending();
            // a cell that starts a path (`\node[red]{7}; \draw …`) is drawn as it is
            const PATHS: &[&str] = &["node", "draw", "path", "fill", "filldraw", "shade", "shadedraw", "clip", "coordinate", "pic", "matrix", "useasboundingbox"];
            let plain = cell.first().is_some_and(|t| PATHS.iter().any(|p| t.is_cs(p)));
            if settings.of_nodes && !empty && !plain {
                // |[options]| text
                let (opts, text) = cell_options(&cell);
                let mut statement = tikra_tokens("\\node[");
                if let Some(m) = matrix {
                    statement.extend(tikra_tokens(&format!(",name={m}-{r}-{c}")));
                }
                statement.push(Token::other(','));
                statement.extend(opts);
                statement.extend(tikra_tokens("] at (0,0) "));
                statement.push(Token::Char('{', Cat::Begin));
                if settings.math {
                    statement.push(Token::Char('$', Cat::Math));
                }
                statement.extend(text);
                if settings.math {
                    statement.push(Token::Char('$', Cat::Math));
                }
                statement.push(Token::Char('}', Cat::End));
                statement.extend(tikra_tokens(";"));
                self.run_tokens(statement)
            } else if empty {
                self.run_tokens(cell)
            } else {
                let mut v = settings.at_begin_cell.clone();
                v.extend(cell);
                v.extend(settings.at_end_cell.clone());
                self.run_tokens(v)
            }
        })();
        self.end_scope();
        self.end_group();
        res
    }
}

/// `|(name) [red]| text` → (`name=name,red`, `text`).
fn cell_options(cell: &[Token]) -> (Vec<Token>, Vec<Token>) {
    let t = crate::keys::trim(cell);
    if t.first().is_some_and(|x| x.is_char('|')) {
        if let Some(end) = t.iter().skip(1).position(|x| x.is_char('|')) {
            let inner = crate::keys::trim(&t[1..end + 1]);
            let mut options = Vec::new();
            let mut i = 0;
            while i < inner.len() {
                let close = if inner[i].is_char('[') {
                    ']'
                } else if inner[i].is_char('(') {
                    ')'
                } else {
                    i += 1;
                    continue;
                };
                let Some(len) = inner[i + 1..].iter().position(|x| x.is_char(close)) else { break };
                if !options.is_empty() {
                    options.push(Token::other(','));
                }
                if close == ')' {
                    options.extend(tikra_tokens("name="));
                    options.push(Token::Char('{', Cat::Begin));
                    options.extend(inner[i + 1..i + 1 + len].iter().cloned());
                    options.push(Token::Char('}', Cat::End));
                } else {
                    options.extend(inner[i + 1..i + 1 + len].iter().cloned());
                }
                i += len + 2;
            }
            return (options, crate::keys::trim(&t[end + 2..]).to_vec());
        }
    }
    (Vec::new(), t.to_vec())
}

/// What the matrix's options say about its cells.
#[derive(Debug, Clone, Default)]
pub struct MatrixSettings {
    /// `column sep` and `row sep` as pgf's separation lists
    pub column_sep: Vec<Token>,
    pub row_sep: Vec<Token>,
    pub of_nodes: bool,
    pub math: bool,
    /// `execute at begin cell`, `execute at end cell`, `execute at empty cell`
    pub at_begin_cell: Vec<Token>,
    pub at_end_cell: Vec<Token>,
    pub at_empty_cell: Vec<Token>,
    pub ampersand: Option<String>,
    /// `nodes in empty cells`, and `matrix anchor` (the matrix node's own anchor)
    pub nodes_in_empty_cells: bool,
    pub anchor: Option<String>,
}
