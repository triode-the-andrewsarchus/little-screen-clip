//! `\addplot`, as `pgfplotscoordprocessing.code.tex` reads it:
//! `\addplot[+][options] [plot[behavior]] <input> <trailing path>;` with the inputs `coordinates`,
//! `table`, `expression` (`{…}`, `(x(t),y(t))`), `file`, `fill between`, and `\addplot3`.
//! The survey computes the coordinates; [`super::draw`] draws them.

use crate::tex::{self, Cat, Token};
use crate::Error;

use super::keys;
use super::table::{self, Table};
use super::{Engine, Entry};

/// A data point. Unbounded or missing values are NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub meta: f64,
    /// error bars: x minus, x plus, y minus, y plus (NaN: none)
    pub err: [f64; 4],
    /// The point meta as written (`[label]`, a table cell): an index into the plot's labels.
    pub label: u32,
    /// The sampling variable's value (expressions), and `quiver`'s (u, v, w).
    pub param: [f64; 2],
    pub uvw: [f64; 3],
    /// `coordinate style`: an index into the plot's `coord_styles` (`NO_LABEL`: none).
    pub style: u32,
    /// `visualization depends on`: an index into the plot's `deps` (`NO_LABEL`: none).
    pub deps: u32,
}

/// No label.
pub const NO_LABEL: u32 = u32::MAX;

thread_local! {
    /// The labels of the plot being read.
    static LABELS: std::cell::RefCell<Vec<Vec<Token>>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn add_label(tokens: Vec<Token>) -> u32 {
    LABELS.with(|l| {
        let mut l = l.borrow_mut();
        l.push(tokens);
        (l.len() - 1) as u32
    })
}

impl Coord {
    pub fn new(x: f64, y: f64, z: f64) -> Coord {
        Coord { x, y, z, meta: f64::NAN, err: [f64::NAN; 4], label: NO_LABEL, param: [f64::NAN; 2], uvw: [f64::NAN; 3], style: NO_LABEL, deps: NO_LABEL }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Plot {
    /// Where the `\addplot` is in the code: it is drawn at the end of the axis.
    pub source: Option<usize>,
    /// The full option list it is drawn with.
    pub style: Vec<Token>,
    pub coords: Vec<Coord>,
    pub three_d: bool,
    /// What follows the input up to the `;` (`\closedcycle`, `-- cycle`, `node {…}`).
    pub post: Vec<Token>,
    pub forget: bool,
    /// `/tikz/@plot handler` (empty: lines or `smooth`)
    pub handler: String,
    /// The plot's number (`\plotnum`) and its number among plots of the same handler.
    pub index: usize,
    /// `stack plots=x|y`, with `stack dir=minus`
    pub stack: Option<char>,
    pub stack_minus: bool,
    /// For a lattice (`\addplot3` of an expression, `mesh/rows`): (rows, cols).
    pub lattice: Option<(usize, usize)>,
    /// `fill between[of=a and b]`
    pub fill_between: Option<Vec<Token>>,
    /// `bar shift` (or `bar shift auto`) given to the axis or the plot: stacked bars keep it.
    pub bar_shift_given: bool,
    /// `x tick=data`: the first plot's x coordinates are kept.
    pub symbolic: bool,
    /// `mesh` (`'m'`) or `surf`/`patch` (`'s'`) plots, colored by the colormap.
    pub mesh: Option<char>,
    pub patch: bool,
    /// `scatter`: marks colored by their point meta.
    pub scatter: bool,
    /// The plot has numeric point meta (`point meta=y`, `explicit`…).
    pub has_meta: bool,
    /// Where the input's scanlines end (empty lines in tables and coordinates).
    pub scanlines: Vec<usize>,
    /// The point metas as written, for `point meta=explicit symbolic` (see `Coord::label`).
    pub labels: Vec<Vec<Token>>,
    /// `mesh input=image` (`matrix plot`): the points are the centers of the cells.
    pub image: bool,
    /// `late options`: axis options the plot asks for (`matrix plot`: `y dir=reverse`).
    pub late: Vec<Token>,
    /// `boxplot`, `boxplot prepared`: the box's numbers (the points are the outliers).
    pub boxplot: Option<Box<super::boxplot::BoxStats>>,
    /// The styles of single coordinates (`coordinate style/.condition`, `.from`).
    pub coord_styles: Vec<Vec<Token>>,
    /// `visualization depends on={expr \as \macro}`: the macros' values at single coordinates.
    pub deps: Vec<Vec<(String, String)>>,
    /// The formula it was sampled from (`\addplot {x^2}`), for the figure's description.
    pub expression: String,
    /// `\addplot graphics`: the image and where it goes.
    pub graphics: Option<Box<super::graphics::Graphics>>,
}

/// Letters following in the input: a word like `coordinates`.
fn word(e: &mut Engine) -> String {
    let mut w = String::new();
    loop {
        match e.tex.get_token() {
            Some(Token::Char(c, Cat::Letter)) => w.push(c),
            Some(t) => {
                e.tex.back(t);
                break;
            }
            None => break,
        }
        if w.len() > 40 {
            break;
        }
    }
    w
}

fn peek(e: &mut Engine) -> Option<Token> {
    let t = e.tex.next_nonspace();
    if let Some(t) = &t {
        e.tex.back(t.clone());
    }
    t
}

/// `\addplot…;`
pub fn addplot(e: &mut Engine) -> Result<(), Error> {
    let mut three_d = false;
    let mut plus = false;
    if let Some(t) = e.tex.get_token() {
        if t.is_char('3') {
            three_d = true;
        } else {
            e.tex.back(t);
        }
    }
    if let Some(t) = e.tex.next_nonspace() {
        if t.is_char('+') {
            plus = true;
        } else {
            e.tex.back(t);
        }
    }
    let user = e.optional()?;
    // `plot[behavior options]`
    let mut behavior = Vec::new();
    if let Some(Token::Char('p', Cat::Letter)) = peek(e) {
        let w = word(e);
        if w == "plot" {
            if let Some(o) = e.optional()? {
                behavior = o;
            }
        } else {
            // the word given back as letters: `\addplot pizza` is the input 'pizza'
            e.tex.push_tokens(w.chars().map(|c| Token::Char(c, Cat::Letter)));
        }
    }
    let (index, cycle) = {
        let a = e.axis.as_ref().expect("in an axis");
        (a.numplots, a.numplots)
    };
    let user_options = user.clone().unwrap_or_default();
    let mut spec = Vec::new();
    if user.is_none() || plus {
        spec.extend(cycle_entry(e, cycle)?);
        if plus && !user_options.is_empty() {
            spec.push(Token::other(','));
        }
    }
    spec.extend(user_options.clone());
    if !behavior.is_empty() {
        spec.push(Token::other(','));
        spec.extend(behavior);
    }
    let mut style = super::toks(&format!("/pgfplots/every axis plot,/pgfplots/every axis plot except legend,/pgfplots/every axis plot no {index}/.try,"));
    style.extend(spec.clone());
    style.extend(super::toks(",/pgfplots/.cd,/pgfplots/every axis plot post"));

    // the survey: the plot's keys, then its input
    e.begin_group();
    let r = survey(e, &style, three_d);
    e.end_group();
    let mut plot = r?;
    // a forgotten plot has `every forget plot` instead of the styles of the plot's number
    let style = if plot.forget {
        let mut forget = super::toks("/pgfplots/every axis plot,/pgfplots/every forget plot,");
        forget.extend(spec.clone());
        forget.extend(super::toks(",/pgfplots/.cd,/pgfplots/every forget plot,/pgfplots/every axis plot post"));
        forget
    } else {
        style
    };
    plot.style = style;
    plot.index = index;
    if !plot.forget {
        // a scatter plot with classes has a legend entry per class: its mark
        let classes = if plot.scatter {
            e.begin_group();
            let r = e.process_keys(style_for_classes(&spec), Some("/pgfplots"));
            let c = scatter_classes(e);
            e.end_group();
            r?;
            c
        } else {
            Vec::new()
        };
        let a = e.axis.as_mut().expect("in an axis");
        a.numplots += 1;
        if classes.is_empty() {
            a.images.push(spec);
        } else {
            for (_, s) in classes {
                let mut image = spec.clone();
                image.extend(super::toks(",/tikz/only marks,"));
                image.extend(s);
                a.images.push(image);
            }
        }
    }
    e.axis.as_mut().expect("in an axis").entries.push(Entry::Plot(Box::new(plot)));
    Ok(())
}

/// The plot's options, for reading its `scatter/classes`.
fn style_for_classes(spec: &[Token]) -> Vec<Token> {
    let mut s = super::toks("/pgfplots/every axis plot,");
    s.extend(spec.iter().cloned());
    s
}

/// The entry of the cycle list for plot `k` (`\pgfplots@getautoplotspec`).
pub fn cycle_entry(e: &mut Engine, k: usize) -> Result<Vec<Token>, Error> {
    let name = keys::value(e, "/pgfplots/cycle list name");
    let shift = keys::number(e, "/pgfplots/cycle list shift").unwrap_or(0.0) as i64;
    let Some(list) = crate::keys::value(&e.tex, &format!("/pgfplots/@cycle lists/{name}")) else {
        e.warn(format!("cycle list '{name}' not defined"));
        return Ok(Vec::new());
    };
    let k = (k as i64 + shift).max(0) as usize;
    // `cycle multi list={a\nextlist b}`: one entry of each, the last list fastest
    if list.iter().any(|t| t.is_cs("nextlist")) {
        let lists: Vec<Vec<Vec<Token>>> = list.split(|t| t.is_cs("nextlist")).map(|part| multi_part(e, part)).filter(|l| !l.is_empty()).collect();
        let mut out = Vec::new();
        let mut rest = k;
        let mut chosen = Vec::new();
        if name.starts_with("@multiindex") {
            // `cycle multiindex list`: the same index into every list, the shorter ones padded
            // with nothing (restarted with `cycle multiindex* list`)
            let longest = lists.iter().map(Vec::len).max().unwrap_or(1);
            for l in &lists {
                let i = if name == "@multiindex*@" { k % l.len() } else { k % longest };
                chosen.push(l.get(i).cloned().unwrap_or_default());
            }
            chosen.reverse();
        } else {
            for l in lists.iter().rev() {
                chosen.push(l[rest % l.len()].clone());
                rest /= l.len();
            }
        }
        for entry in chosen.into_iter().rev().filter(|c| !c.is_empty()) {
            if !out.is_empty() {
                out.push(Token::other(','));
            }
            out.extend(entry);
        }
        return Ok(out);
    }
    let entries = split_cycle_list(&list);
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    Ok(entries[k % entries.len()].clone())
}

/// A list of `cycle multi list`: a cycle list's name (`[2 of]mark list`: its first two entries)
/// or the entries themselves.
fn multi_part(e: &mut Engine, part: &[Token]) -> Vec<Vec<Token>> {
    let text = tex::tokens_to_string(crate::keys::trim(part)).trim().to_string();
    let (limit, name) = match text.strip_prefix('[').and_then(|r| r.split_once(" of]")) {
        Some((n, rest)) => (n.trim().parse::<usize>().ok(), rest.trim().to_string()),
        None => (None, text.clone()),
    };
    if let Some(list) = crate::keys::value(&e.tex, &format!("/pgfplots/@cycle lists/{name}")) {
        let mut entries = split_cycle_list(&list);
        if let Some(n) = limit {
            entries.truncate(n.max(1));
        }
        return entries;
    }
    split_cycle_list(part)
}

/// A cycle list: entries separated by `\\`, or by commas when there is no `\\`.
pub fn split_cycle_list(list: &[Token]) -> Vec<Vec<Token>> {
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
    out.push(cur);
    out.into_iter().map(|v| tex::strip_outer_braces(crate::keys::trim(&v).to_vec())).filter(|v| !v.is_empty()).collect()
}

/// Reads the plot's input and trailing path, with its keys set (inside the plot's group).
fn survey(e: &mut Engine, style: &[Token], three_d: bool) -> Result<Plot, Error> {
    e.st.mode = Default::default();
    e.process_keys(style.to_vec(), Some("/pgfplots"))?;
    if let Some(code) = keys::value_tokens(e, "/pgfplots/execute at begin plot") {
        e.run_tokens(code)?;
    }
    let mut plot = Plot { three_d, source: e.source, ..Default::default() };
    LABELS.with(|l| l.borrow_mut().clear());
    let r = survey_input(e, &mut plot, three_d);
    plot.labels = LABELS.with(|l| std::mem::take(&mut *l.borrow_mut()));
    r?;
    Ok(plot)
}

fn survey_input(e: &mut Engine, plot: &mut Plot, three_d: bool) -> Result<(), Error> {
    plot.forget = keys::flag(e, "/pgfplots/forget plot");
    plot.mesh = match keys::value(e, "/pgfplots/@mesh").as_str() {
        "m" => Some('m'),
        "s" => Some('s'),
        _ => None,
    };
    plot.patch = keys::flag(e, "/pgfplots/@patch");
    plot.image = plot.mesh.is_some() && keys::value(e, "/pgfplots/mesh input") == "image";
    plot.late = keys::value_tokens(e, "/pgfplots/late options").unwrap_or_default();
    plot.scatter = keys::flag(e, "/pgfplots/scatter");
    plot.handler = keys::value(e, "/tikz/@plot handler");
    let axis_options = e.axis.as_ref().map(|a| tex::tokens_to_string(&a.options)).unwrap_or_default();
    plot.bar_shift_given = axis_options.contains("bar shift");
    // a prepared contour: its level is z in 3d, the third column (point meta=explicit) in 2d
    if plot.handler == "contour prepared" && keys::value(e, "/pgfplots/point meta").is_empty() {
        if three_d {
            keys::set(e, "/pgfplots/point meta", "z");
        } else if keys::value(e, "/pgfplots/@contour format") != "matlab" {
            keys::set(e, "/pgfplots/table/meta index", "2");
            keys::set(e, "/pgfplots/point meta", "explicit");
        }
    }
    plot.stack = match keys::value(e, "/pgfplots/stack plots").as_str() {
        "x" => Some('x'),
        "y" => Some('y'),
        _ => None,
    };
    plot.stack_minus = keys::value(e, "/pgfplots/stack dir") == "minus";
    let t = e.tex.next_nonspace().ok_or_else(|| Error::Figure("\\addplot without input".into()))?;
    match &t {
        Token::Char('(', _) => {
            let tokens = super::super::coords::read_coordinate_tokens(e)?;
            let parts = split_commas(&tokens);
            plot.coords = sample(e, &parts, plot)?;
        }
        Token::Char(_, Cat::Begin) => {
            let body = e.tex.read_group_body()?;
            plot.expression = tex::tokens_to_string(&body);
            let mut parts = vec![super::toks("x"), body];
            if three_d {
                parts = vec![super::toks("x"), super::toks("y"), parts.pop().unwrap_or_default()];
            }
            plot.coords = sample(e, &parts, plot)?;
        }
        Token::Char(c, Cat::Letter) => {
            e.tex.back(t.clone());
            let w = word(e);
            let _ = c;
            match w.as_str() {
                "coordinates" => {
                    let list = match e.tex.next_nonspace() {
                        Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
                        _ => return Err(Error::Figure("coordinates without a list".into())),
                    };
                    (plot.coords, plot.scanlines) = coordinates(e, list)?;
                }
                "table" => {
                    (plot.coords, plot.scanlines) = read_table(e, three_d)?;
                }
                "expression" => {
                    // `\addplot expression[domain=0:3] {…}`: options for the sampling
                    if let Some(o) = e.optional()? {
                        e.process_keys(o, Some("/pgfplots"))?;
                    }
                    let body = match e.tex.next_nonspace() {
                        Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
                        _ => return Err(Error::Figure("expression without a formula".into())),
                    };
                    plot.expression = tex::tokens_to_string(&body);
                    let parts = if three_d { vec![super::toks("x"), super::toks("y"), body] } else { vec![super::toks("x"), body] };
                    plot.coords = sample(e, &parts, plot)?;
                }
                "file" => {
                    let _ = e.optional()?;
                    let name = e.argument_string()?;
                    let search = search_path(e);
                    match table::read_file(&name, &search) {
                        Ok(text) => {
                            let t = Table::parse(&text, table::ColSep::Space, "false", 0, "");
                            plot.coords = table_coords(e, &t, three_d)?;
                            plot.scanlines = t.scanlines.clone();
                        }
                        Err(m) => e.warn(format!("\\addplot file: {m}")),
                    }
                }
                "fill" => {
                    let between = word_after_space(e);
                    if between != "between" {
                        return Err(Error::Figure(format!("\\addplot fill {between} not supported")));
                    }
                    plot.fill_between = Some(e.optional()?.unwrap_or_default());
                }
                "graphics" => super::graphics::survey(e, plot, three_d)?,
                "gnuplot" | "shell" | "function" => {
                    let _ = e.optional()?;
                    let _ = e.argument()?;
                    e.warn(format!("\\addplot {w} not supported: it needs an external program or file"));
                }
                other => return Err(Error::Figure(format!("\\addplot input '{other}' not supported"))),
            }
        }
        other => return Err(Error::Figure(format!("\\addplot input '{}' not supported", tex::show_token(other)))),
    }
    // the trailing path, up to the `;`
    let mut depth = 0usize;
    while let Some(x) = e.tex.get_token() {
        match &x {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => {
                if depth == 0 {
                    e.tex.back(x);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
        if depth == 0 && x.is_char(';') {
            break;
        }
        plot.post.push(x);
        if plot.post.len() > tex::MAX_TOKENS {
            break;
        }
    }
    data_cs(e, plot);
    // `is smithchart cs`: the input is in the unit circle's plane
    if e.axis.as_ref().is_some_and(|a| a.kind == "smithchart") && keys::flag(e, "/pgfplots/is smithchart cs") {
        for c in plot.coords.iter_mut() {
            (c.x, c.y) = super::smith::impedance(c.x, c.y);
        }
    }
    if plot.handler == "contour prepared" && keys::value(e, "/pgfplots/@contour format") == "matlab" {
        super::contour::from_matlab(plot);
        keys::set(e, "/pgfplots/point meta", "explicit");
    }
    if keys::flag(e, "/pgfplots/@quiver") {
        quiver_vectors(e, plot);
    }
    if keys::flag(e, "/pgfplots/@hist") {
        histogram(e, plot)?;
    }
    match keys::value(e, "/pgfplots/@boxplot").as_str() {
        "raw" => super::boxplot::survey(e, plot, false),
        "prepared" => super::boxplot::survey(e, plot, true),
        _ => {}
    }
    point_meta(e, plot)?;
    if plot.patch {
        patch_table(e, plot);
    }
    lattice_from_scanlines(e, plot);
    coordinate_filters(e, plot)?;
    filter(e, plot)?;
    if let Some(code) = keys::value_tokens(e, "/pgfplots/execute at end plot") {
        e.run_tokens(code)?;
    }
    Ok(())
}

/// `patch table={0 1 2\\ 1 2 3\\}` (or a file name): the patches are rows of indices into the
/// points read; `patch table with point meta` adds a color to each row, `… with individual point
/// meta` one to each of its corners. The points become the patches' corners, one after another,
/// each with its color.
fn patch_table(e: &mut Engine, plot: &mut Plot) {
    let mut found = None;
    for (key, kind) in [("patch table", 0), ("patch table with point meta", 1), ("patch table with individual point meta", 2)] {
        let v = keys::value(e, &format!("/pgfplots/{key}"));
        if !v.trim().is_empty() {
            found = Some((v, kind));
        }
    }
    let Some((text, kind)) = found else { return };
    let text = text.replace("\\\\", "\n");
    let trimmed = text.trim();
    let text = if !trimmed.contains('\n') && !trimmed.contains(' ') && trimmed.chars().any(|c| c.is_alphabetic()) {
        match table::read_file(trimmed, &search_path(e)) {
            Ok(t) => t,
            Err(m) => {
                e.warn(format!("patch table: {m}"));
                return;
            }
        }
    } else {
        text
    };
    let t = Table::parse(&text, table::ColSep::Space, "false", 0, "");
    let mut coords = Vec::new();
    for row in &t.rows {
        let v: Vec<f64> = row.iter().filter_map(|c| table::parse_number(c)).collect();
        let n = match kind {
            0 => v.len(),
            1 => v.len().saturating_sub(1),
            _ => v.len() / 2,
        };
        for (i, idx) in v[..n].iter().enumerate() {
            let Some(c) = plot.coords.get(*idx as usize) else { continue };
            let mut c = *c;
            match kind {
                1 => c.meta = v[n],
                2 => c.meta = v.get(n + i).copied().unwrap_or(f64::NAN),
                _ => {}
            }
            coords.push(c);
        }
    }
    plot.coords = coords;
    plot.scanlines.clear();
    if kind > 0 {
        plot.has_meta = true;
    }
}

fn word_after_space(e: &mut Engine) -> String {
    if let Some(t) = e.tex.next_nonspace() {
        e.tex.back(t);
    }
    word(e)
}

/// Splits tokens at top-level commas.
pub fn split_commas(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0i32;
    for t in tokens {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth -= 1,
            Token::Char('(', _) => depth += 1,
            Token::Char(')', _) => depth -= 1,
            Token::Char(',', _) if depth == 0 => {
                out.push(Vec::new());
                continue;
            }
            _ => {}
        }
        if let Some(l) = out.last_mut() {
            l.push(t.clone());
        }
    }
    out.into_iter().map(|p| tex::strip_outer_braces(crate::keys::trim(&p).to_vec())).collect()
}

/// The domain and samples of an expression plot.
fn sample_points(e: &mut Engine, domain_key: &str, samples_key: &str, log: bool) -> Result<Vec<f64>, Error> {
    let at = crate::keys::value(&e.tex, "/pgfplots/samples at").filter(|v| !crate::keys::trim(v).is_empty());
    if let Some(list) = at.filter(|_| domain_key == "/pgfplots/domain") {
        let list = e.tex.expand_fully(list)?;
        let mut v = Vec::new();
        for item in super::super::foreach_items(&list) {
            v.push(e.number_of(item)?);
        }
        return Ok(v);
    }
    let d = keys::value(e, domain_key);
    let d = if d.is_empty() { keys::value(e, "/pgfplots/domain") } else { d };
    let (a, b) = if d.is_empty() {
        e.st.domain
    } else {
        let (a, b) = d.split_once(':').ok_or_else(|| Error::Figure(format!("domain '{d}' is not a:b")))?;
        (e.number(a)?, e.number(b)?)
    };
    let n = keys::number(e, samples_key).or_else(|| keys::number(e, "/pgfplots/samples")).map_or(e.st.samples, |s| s.max(1.0) as usize).clamp(1, 10_000);
    if n == 1 {
        return Ok(vec![a]);
    }
    if !log {
        if let Some(v) = tex_samples(a, b, n) {
            return Ok(v);
        }
    } else if a > 0.0 && b > 0.0 {
        // on a log axis, the same loop between the fpu's logarithms of the ends: 1:100000 in 25
        // samples ends at ln 11.51257, not 11.51291
        if let Some(v) = tex_samples(super::fpu::ln(a), super::fpu::ln(b), n) {
            return Ok(v.into_iter().map(f64::exp).collect());
        }
    }
    let mut v = Vec::with_capacity(n);
    for k in 0..n {
        let t = k as f64 / (n - 1) as f64;
        v.push(if log && a > 0.0 && b > 0.0 { (a.ln() + t * (b.ln() - a.ln())).exp() } else { a + t * (b - a) });
    }
    Ok(v)
}

/// The samples of `domain=a:b` as pgfplots loops over them, in pgf's floating point unit
/// (`\pgfplots@domain@to@foreach` and the `a,a+h,...,b` loop of `\pgfplotsforeachungrouped`):
/// `a + (b - a)/(n - 1)` is the second sample, `h` its distance to `a`, and the loop adds `h`
/// while the sample is at most `b + h/2`. The steps keep few digits, so the last sample can fall a
/// little short of `b`, which moves the data limits and, sometimes, the ticks.
fn tex_samples(a: f64, b: f64, n: usize) -> Option<Vec<f64>> {
    use super::fpu::Num;
    if n < 2 || a == b {
        return None;
    }
    let (fa, fb) = (Num::from_f64(a)?, Num::from_f64(b)?);
    let q = fb.plus(fa.negated()).over(Num::from_f64((n - 1) as f64)?);
    let h = fa.plus(q).plus(fa.negated());
    if h.is_zero() {
        return None;
    }
    let end = b + 0.5 * h.value();
    let up = h.value() > 0.0;
    let mut v = Vec::with_capacity(n + 1);
    let mut x = fa;
    // (a guard against a step much shorter than asked: never more than twice the samples)
    while ((up && x.value() <= end) || (!up && x.value() >= end)) && v.len() <= 2 * n + 2 {
        v.push(x.value());
        x = x.plus(h);
    }
    (v.len() <= 2 * n + 2).then_some(v)
}

/// Where the points of one patch of a type lie in the unit interval or square (`sample in unit
/// cube` of the patch classes), in their order; None for a type without them.
fn patch_unit_points(kind: &str) -> Option<Vec<(f64, f64)>> {
    let (t1, t2) = (1.0 / 3.0, 2.0 / 3.0);
    Some(match kind.trim() {
        "line" => vec![(0.0, 0.0), (1.0, 0.0)],
        "quadratic spline" => vec![(0.0, 0.0), (1.0, 0.0), (0.5, 0.0)],
        "cubic spline" => vec![(0.0, 0.0), (1.0, 0.0), (t1, 0.0), (t2, 0.0)],
        "rectangle" | "bilinear" => vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
        "triangle quadr" => vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.5, 0.0), (0.5, 0.5), (0.0, 0.5), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.5, 0.5), (1.0, 0.5), (0.5, 1.0)],
        "biquadratic" => vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5), (0.5, 0.5)],
        "bicubic" => (0..16).map(|k| ([0.0, t1, t2, 1.0][k % 4], [0.0, t1, t2, 1.0][k / 4])).collect(),
        _ => return None,
    })
}

/// Samples `(x(var), y(var)[, z(var, y)])`.
fn sample(e: &mut Engine, parts: &[Vec<Token>], plot: &mut Plot) -> Result<Vec<Coord>, Error> {
    let var = e.st.variable.clone();
    let var_y = keys::value(e, "/pgfplots/variable y").trim_start_matches('\\').to_string();
    // the variables read as names in the expressions
    for (name, value) in [(var.as_str(), var.clone()), (var_y.as_str(), var_y.clone())] {
        e.tex.set_meaning(&Token::cs(name), super::super::macro_from(&value), false);
    }
    let texts: Vec<String> = parts.iter().map(|p| e.math_string(p.clone())).collect::<Result<_, _>>()?;
    let xlog = keys::value(e, "/pgfplots/xmode") == "log";
    let uniform_x = texts.first().is_some_and(|t| t.trim() == var);
    let xs = sample_points(e, "/pgfplots/domain", "/pgfplots/samples", xlog && uniform_x)?;
    let mut vars = fpu_vars(e);
    // `trig format plots`: the plot's expressions in radians (or degrees), whatever pgf's is
    match keys::value(e, "/pgfplots/trig format plots").trim() {
        "rad" => vars.trig_rad = true,
        "deg" => vars.trig_rad = false,
        _ => {}
    }
    let mut out = Vec::new();
    // a point pgfplots cannot compute is unbounded (NaN); the first error is kept, to tell it when
    // no point at all could be
    let failed: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let eval = |text: &str, vars: &crate::expr::Variables| {
        crate::expr::eval(text, vars).unwrap_or_else(|err| {
            failed.borrow_mut().get_or_insert_with(|| err.to_string());
            f64::NAN
        })
    };
    // `patch type sampling`: each interval (or cell) gives the points of one patch, at the places
    // of the patch type's points in the unit interval (square)
    let unit = if keys::flag(e, "/pgfplots/patch type sampling") { patch_unit_points(&keys::value(e, "/pgfplots/patch type")) } else { None };
    if let Some(unit) = unit {
        let at = |v: &[f64], i: usize, t: f64| v[i] + t * (v[i + 1] - v[i]);
        let two = plot.three_d && texts.len() >= 3 && unit.iter().any(|(_, v)| *v != 0.0);
        let ys = if two { sample_points(e, "/pgfplots/y domain", "/pgfplots/samples y", false)? } else { vec![0.0, 0.0] };
        for j in 0..ys.len().saturating_sub(1) {
            for i in 0..xs.len().saturating_sub(1) {
                for &(u, v) in &unit {
                    let (x, y) = (at(&xs, i, u), at(&ys, j, v));
                    vars.insert(var.clone(), x);
                    vars.insert(var_y.clone(), y);
                    let mut c = Coord::new(eval(&texts[0], &vars), texts.get(1).map_or(f64::NAN, |t| eval(t, &vars)), texts.get(2).map_or(0.0, |t| eval(t, &vars)));
                    c.param = [x, y];
                    out.push(c);
                }
            }
        }
        plot.patch = plot.mesh.is_some();
        return Ok(out);
    }
    if plot.three_d && texts.len() >= 3 {
        let ys = sample_points(e, "/pgfplots/y domain", "/pgfplots/samples y", false)?;
        let line = ys.len() < 2;
        if line {
            for &x in &xs {
                vars.insert(var.clone(), x);
                vars.insert(var_y.clone(), 0.0);
                out.push(Coord::new(eval(&texts[0], &vars), eval(&texts[1], &vars), eval(&texts[2], &vars)));
            }
        } else {
            for &y in &ys {
                for &x in &xs {
                    vars.insert(var.clone(), x);
                    vars.insert(var_y.clone(), y);
                    let mut c = Coord::new(eval(&texts[0], &vars), eval(&texts[1], &vars), eval(&texts[2], &vars));
                    c.param = [x, y];
                    out.push(c);
                }
            }
            plot.lattice = Some((ys.len(), xs.len()));
        }
    } else {
        for &x in &xs {
            vars.insert(var.clone(), x);
            let cx = eval(&texts[0], &vars);
            let cy = texts.get(1).map_or(f64::NAN, |t| eval(t, &vars));
            let cz = texts.get(2).map_or(0.0, |t| eval(t, &vars));
            let mut c = Coord::new(cx, cy, cz);
            c.param = [x, f64::NAN];
            out.push(c);
        }
    }
    if let Some(err) = failed.into_inner().filter(|_| out.iter().all(|c| !(c.x.is_finite() && c.y.is_finite()))) {
        e.warn(format!("\\addplot: {err}: no point of the plot could be computed"));
    }
    Ok(out)
}

/// A value of `coordinates`: a number, or an expression.
/// A coordinate of axis `c`: a name of `symbolic c coords`, or a number.
fn axis_value(e: &mut Engine, c: char, tokens: &[Token]) -> f64 {
    if let Some(i) = super::symbolic::index(e, c, &tex::tokens_to_string(tokens)) {
        return i;
    }
    value(e, tokens)
}

/// pgfmath's variables for pgfplots' math, which runs with the fpu on.
fn fpu_vars(e: &Engine) -> crate::expr::Variables {
    crate::expr::Variables { fpu: true, ..e.vars() }
}

fn value(e: &mut Engine, tokens: &[Token]) -> f64 {
    let s = tex::tokens_to_string(tokens);
    if let Some(v) = table::parse_number(&s) {
        return v;
    }
    let tokens = tokens.to_vec();
    e.math_string(tokens).and_then(|t| crate::expr::eval(&t, &fpu_vars(e))).unwrap_or(f64::NAN)
}

/// `coordinates {(x,y) (x,y) +- (ex,ey) [meta] …}`.
/// The points of `coordinates {…}`, and where its scanlines (ended by empty lines) end.
fn coordinates(e: &mut Engine, list: Vec<Token>) -> Result<(Vec<Coord>, Vec<usize>), Error> {
    let explicit_symbolic = keys::value(e, "/pgfplots/point meta").trim() == "explicit symbolic";
    let list = e.tex.expand_fully(list)?;
    let mut out = Vec::new();
    let mut scanlines: Vec<usize> = Vec::new();
    let mut i = 0;
    let read_paren = |i: &mut usize| -> Option<Vec<Token>> {
        let mut depth = 0i32;
        let start = *i + 1;
        while *i < list.len() {
            match &list[*i] {
                Token::Char('(', _) => depth += 1,
                Token::Char(')', _) => {
                    depth -= 1;
                    if depth == 0 {
                        let v = list[start..*i].to_vec();
                        *i += 1;
                        return Some(v);
                    }
                }
                _ => {}
            }
            *i += 1;
        }
        None
    };
    while i < list.len() {
        let t = &list[i];
        if t.is_cs("par") {
            if !out.is_empty() && scanlines.last() != Some(&out.len()) {
                scanlines.push(out.len());
            }
            i += 1;
            continue;
        }
        if t.is_char('(') {
            let Some(inside) = read_paren(&mut i) else { break };
            let parts = split_commas(&inside);
            if parts.iter().all(|p| p.is_empty()) {
                // () is a jump
                out.push(Coord::new(f64::NAN, f64::NAN, f64::NAN));
                continue;
            }
            let x = parts.first().map_or(f64::NAN, |p| axis_value(e, 'x', p));
            let y = parts.get(1).map_or(f64::NAN, |p| axis_value(e, 'y', p));
            let z = parts.get(2).map_or(0.0, |p| value(e, p));
            let mut c = Coord::new(x, y, z);
            // error bars (`+- (…)`, or `+= (…) -= (…)`), then [meta]
            let mut j = i;
            loop {
                while j < list.len() && list[j].is_space() {
                    j += 1;
                }
                if !(j + 1 < list.len() && (list[j].is_char('+') || list[j].is_char('-')) && (list[j + 1].is_char('-') || list[j + 1].is_char('='))) {
                    break;
                }
                let before = j;
                {
                    let kind = (list[j].is_char('+'), list[j + 1].is_char('-'));
                    let mut k = j + 2;
                    while k < list.len() && list[k].is_space() {
                        k += 1;
                    }
                    if k < list.len() && list[k].is_char('(') {
                        let mut kk = k;
                        if let Some(inside) = read_paren(&mut kk) {
                            let p = split_commas(&inside);
                            let ex = p.first().map_or(f64::NAN, |v| value(e, v));
                            let ey = p.get(1).map_or(f64::NAN, |v| value(e, v));
                            match kind {
                                (true, true) => c.err = [ex, ex, ey, ey],
                                (true, false) => {
                                    c.err[1] = ex;
                                    c.err[3] = ey;
                                }
                                _ => {
                                    c.err[0] = ex;
                                    c.err[2] = ey;
                                }
                            }
                            j = kk;
                        }
                    }
                }
                if j == before {
                    break;
                }
            }
            // (the loop above stopped at what is not a space)
            if j < list.len() && list[j].is_char('[') {
                let mut k = j + 1;
                let start = k;
                while k < list.len() && !list[k].is_char(']') {
                    k += 1;
                }
                let written = &list[start..k.min(list.len())];
                // `explicit symbolic`: the text as written, never a number
                if !explicit_symbolic {
                    c.meta = value(e, written);
                }
                c.label = add_label(crate::keys::trim(written).to_vec());
                j = k + 1;
            }
            i = j;
            out.push(c);
            continue;
        }
        i += 1;
    }
    Ok((out, scanlines))
}

fn search_path(e: &mut Engine) -> Vec<String> {
    let s = keys::value(e, "/pgfplots/table/@search path");
    s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

thread_local! {
    /// Tables `\pgfplotstableread` keeps, by macro name: their text and their options.
    static READ_TABLES: std::cell::RefCell<std::collections::HashMap<String, (String, Vec<Token>)>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// `[options]{file or data}` with line ends as characters (rows), and the data as text.
fn table_source(e: &mut Engine) -> Result<(Option<Vec<Token>>, String), Error> {
    let saved = (e.tex.catcode('\n'), e.tex.catcode('\r'));
    e.tex.set_catcode('\n', Cat::Other);
    e.tex.set_catcode('\r', Cat::Other);
    let r = (|| -> Result<(Option<Vec<Token>>, Vec<Token>), Error> {
        // line ends before the options or the data are spaces still
        let next = |e: &mut Engine| loop {
            match e.tex.next_nonspace() {
                Some(Token::Char('\n' | '\r', Cat::Other)) => continue,
                t => break t,
            }
        };
        if let Some(t) = next(e) {
            e.tex.back(t);
        }
        let opts = e.optional()?;
        let data = match next(e) {
            Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
            // `table [options] from \macro`: a table read with \pgfplotstableread
            Some(Token::Char('f', c)) => {
                let mut taken = vec![Token::Char('f', c)];
                for want in ['r', 'o', 'm'] {
                    match e.tex.get_token() {
                        Some(t @ Token::Char(x, _)) if x == want => taken.push(t),
                        Some(t) => {
                            taken.push(t);
                            break;
                        }
                        None => break,
                    }
                }
                if taken.len() == 4 {
                    match next(e) {
                        Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
                        Some(t) => vec![t],
                        None => Vec::new(),
                    }
                } else {
                    for t in taken.into_iter().skip(1).rev() {
                        e.tex.back(t);
                    }
                    vec![Token::Char('f', c)]
                }
            }
            Some(t) => vec![t],
            None => Vec::new(),
        };
        Ok((opts, data))
    })();
    e.tex.set_catcode('\n', saved.0);
    e.tex.set_catcode('\r', saved.1);
    let (opts, mut data) = r?;
    // a file name with macros (`plotdata/experiment\n.dat` in a \foreach) is expanded; a table
    // kept by \pgfplotstableread (one macro) and inline data are not
    let significant: Vec<&Token> = data.iter().filter(|t| !t.is_space()).collect();
    let one_line = !data.iter().any(|t| matches!(t, Token::Char('\n' | '\r', _)) || t.is_cs("\\"));
    if one_line && significant.len() > 1 && significant.iter().any(|t| matches!(t, Token::Cs(_))) {
        data = super::toks(&e.tex.expand_to_string(data)?);
    }
    let mut text = String::new();
    for t in &data {
        match t {
            Token::Char(c, _) => text.push(*c),
            // `\\` kept as written: a row's end with `row sep=\\`
            Token::Cs(n) if &**n == "\\" => text.push_str("\\\\"),
            Token::Cs(n) => {
                text.push('\\');
                text.push_str(n);
                text.push(' ');
            }
        }
    }
    Ok((opts, text))
}

/// `\pgfplotstableread[options]{file or data}\macro`: the table, kept for `table {\macro}`.
pub fn table_read(e: &mut Engine) -> Result<(), Error> {
    let (opts, text) = table_source(e)?;
    let name = match e.tex.next_nonspace() {
        Some(Token::Cs(n)) => n.to_string(),
        Some(Token::Char(_, Cat::Begin)) => {
            let body = e.tex.read_group_body()?;
            match body.iter().find(|t| matches!(t, Token::Cs(_))) {
                Some(Token::Cs(n)) => n.to_string(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    };
    if name.is_empty() {
        e.warn("\\pgfplotstableread without a macro to keep the table: ignored");
        return Ok(());
    }
    READ_TABLES.with(|t| t.borrow_mut().insert(name, (text, opts.unwrap_or_default())));
    Ok(())
}

/// `table[options]{file or data}`.
fn read_table(e: &mut Engine, three_d: bool) -> Result<(Vec<Coord>, Vec<usize>), Error> {
    let (opts, text) = table_source(e)?;
    let Some(table) = load_table(e, opts, text, "\\addplot table")? else { return Ok((Vec::new(), Vec::new())) };
    Ok((table_coords(e, &table, three_d)?, table.scanlines.clone()))
}

/// The table of a file, of inline data or of a `\pgfplotstableread` macro, read with the options
/// (under `/pgfplots/table`); none (with a warning) if it cannot be read.
fn load_table(e: &mut Engine, opts: Option<Vec<Token>>, mut text: String, what: &str) -> Result<Option<Table>, Error> {
    // a table `\pgfplotstableread` kept: its options, then the plot's
    let kept = text.trim().strip_prefix('\\').map(|n| n.trim().to_string()).and_then(|n| READ_TABLES.with(|t| t.borrow().get(&n).cloned()));
    let mut all_opts = Vec::new();
    if let Some((t, o)) = kept {
        text = t;
        all_opts.extend(o);
        all_opts.extend(super::toks(","));
    } else if let Some(name) = text.trim().strip_prefix('\\').map(str::trim).filter(|n| !n.is_empty() && !n.contains(char::is_whitespace)) {
        // any other macro holds the file's name (or the data): its text
        if e.tex.is_defined(name) {
            text = e.tex.expand_to_string(vec![Token::cs(name)])?;
        }
    }
    all_opts.extend(opts.unwrap_or_default());
    if !all_opts.is_empty() {
        let saved = e.st.key_path.clone();
        e.process_keys(all_opts, Some("/pgfplots/table"))?;
        e.st.key_path = saved;
    }
    let row_sep = keys::value(e, "/pgfplots/table/row sep");
    if row_sep.contains("\\\\") {
        // the rows end at `\\`, not at the line ends
        text = text.replace('\n', " ");
    }
    text = text.replace("\\\\", "\n");
    let sep = table::col_sep(&keys::value(e, "/pgfplots/table/col sep"));
    let header = keys::value(e, "/pgfplots/table/header");
    let skip = keys::number(e, "/pgfplots/table/skip first n").unwrap_or(0.0) as usize;
    let comment = keys::value(e, "/pgfplots/table/comment chars");
    let trimmed = text.trim();
    // (a macro left as it is was not defined: not a file's name)
    let is_file = !trimmed.starts_with('\\') && !trimmed.contains('\n') && !trimmed.contains(' ') && !trimmed.is_empty() && trimmed.chars().any(|c| c.is_alphabetic());
    // a file the document wrote (`\pgfplotscolormaptodatafile`)
    let written = format!("tikra@file@{trimmed}");
    let table = if is_file && e.tex.is_defined(&written) {
        let content = e.tex.expand_to_string(vec![Token::cs(&written)])?;
        Table::parse(&content, sep, &header, skip, &comment)
    } else if is_file {
        match table::read_file(trimmed, &search_path(e)) {
            Ok(content) => Table::parse(&content, sep, &header, skip, &comment),
            Err(m) => {
                e.warn(format!("{what}: {m}"));
                return Ok(None);
            }
        }
    } else if trimmed.starts_with('\\') {
        e.warn(format!("{what}: {trimmed} is not defined"));
        return Ok(None);
    } else {
        Table::parse(&text, sep, &header, skip, &comment)
    };
    Ok(Some(table))
}

/// `\pgfplotstabletypeset[options]{table}`: the table as text, a `tabular` of its columns
/// (`columns={a,b}`, else all) under their names, the numbers printed as `\pgfmathprintnumber`
/// prints them, each column centered.
pub fn table_typeset(e: &mut Engine) -> Result<(), Error> {
    let (opts, text) = table_source(e)?;
    // `columns` is pgfplotstable's own; the rest is for reading the table
    let mut columns: Option<Vec<String>> = None;
    let mut rest = Vec::new();
    for item in crate::keys::split_list(&opts.unwrap_or_default()) {
        if item.key.trim() == "columns" {
            let v = item.value.map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
            columns = Some(crate::keys::split_top_level(&v).iter().map(|c| crate::keys::strip_braces(c.trim()).trim().to_string()).collect());
            continue;
        }
        rest.extend(super::toks(&item.key));
        if let Some(v) = item.value {
            rest.extend(super::toks("={"));
            rest.extend(v);
            rest.extend(super::toks("}"));
        }
        rest.extend(super::toks(","));
    }
    e.begin_group();
    let table = load_table(e, Some(rest), text, "\\pgfplotstabletypeset");
    e.end_group();
    let Some(table) = table? else { return Ok(()) };
    let picked: Vec<usize> = match &columns {
        Some(names) => names.iter().filter_map(|n| table.names.iter().position(|m| m == n)).collect(),
        None => (0..table.names.len()).collect(),
    };
    let mut body = format!("\\begin{{tabular}}{{{}}}", "c".repeat(picked.len()));
    body.push_str(&picked.iter().map(|&k| table.names[k].clone()).collect::<Vec<_>>().join(" & "));
    for row in &table.rows {
        body.push_str("\\\\");
        let cells: Vec<String> = picked
            .iter()
            .map(|&k| {
                let cell = row.get(k).map_or("", |c| c.trim());
                match table::parse_number(cell) {
                    Some(v) => format!("${}$", e.st.number_format.print(v)),
                    None => cell.to_string(),
                }
            })
            .collect();
        body.push_str(&cells.join(" & "));
    }
    body.push_str("\\end{tabular}");
    e.typeset_word(body);
    Ok(())
}

/// The coordinates of a table: the `x`, `y`, `z` columns (by name, index or expression).
fn table_coords(e: &mut Engine, t: &Table, three_d: bool) -> Result<Vec<Coord>, Error> {
    #[derive(Clone)]
    enum Src {
        Col(Option<usize>),
        Expr(Vec<Token>),
        /// A column made for the plot (`create col/linear regression`).
        Values(Vec<f64>),
        None,
    }
    let pick = |e: &mut Engine, axis: &str, default: Option<usize>| -> Src {
        if let Some(expr) = keys::value_tokens(e, &format!("/pgfplots/table/{axis} expr")) {
            return Src::Expr(expr);
        }
        let name = keys::value(e, &format!("/pgfplots/table/{axis}"));
        let created = name.trim().trim_start_matches('{').trim_start();
        if let Some(options) = created.strip_prefix("create col/linear regression") {
            // `={y=…}`: the options, up to their closing brace
            let options = options.trim().trim_start_matches('=').trim();
            let options = match options.strip_prefix('{') {
                Some(inner) => {
                    let mut depth = 1;
                    let end = inner
                        .char_indices()
                        .find(|(_, c)| {
                            match c {
                                '{' => depth += 1,
                                '}' => depth -= 1,
                                _ => {}
                            }
                            depth == 0
                        })
                        .map_or(inner.len(), |(i, _)| i);
                    &inner[..end]
                }
                None => options.trim_end_matches('}'),
            };
            return Src::Values(linear_regression(e, t, options));
        }
        if !name.is_empty() {
            return Src::Col(t.column(&name));
        }
        if let Some(i) = keys::number(e, &format!("/pgfplots/table/{axis} index")) {
            return Src::Col(Some(i as usize));
        }
        match default {
            Some(d) => Src::Col(Some(d)),
            None => Src::None,
        }
    };
    let sx = pick(e, "x", Some(0));
    let sy = pick(e, "y", Some(1));
    let sz = pick(e, "z", if three_d { Some(2) } else { None });
    let symbolic = symbolic_meta(e);
    let quiver = keys::flag(e, "/pgfplots/@quiver");
    let t_table = t;
    let smeta = match crate::keys::value(&e.tex, "/pgfplots/point meta") {
        _ if symbolic.is_some() => Src::None,
        Some(v) if tex::tokens_to_string(&v).contains("\\thisrow") => Src::Expr(v),
        _ => pick(e, "meta", None),
    };
    // a tieline plot reads the tie's other end of each row too: `tie end x` (or its index),
    // else the column three after x's
    let tie: Option<[Src; 3]> = (keys::value(e, "/tikz/@plot handler") == "tieline").then(|| {
        let mut ends = [Src::None, Src::None, Src::None];
        for (k, c) in ["x", "y", "z"].iter().enumerate() {
            let own = [&sx, &sy, &sz][k];
            let name = keys::value(e, &format!("/pgfplots/table/tie end {c}"));
            ends[k] = if !name.is_empty() {
                Src::Col(t.column(&name))
            } else if let Some(i) = keys::number(e, &format!("/pgfplots/table/tie end {c} index")) {
                Src::Col(Some(i as usize))
            } else {
                match own {
                    Src::Col(Some(i)) if i + 3 < t.columns() => Src::Col(Some(i + 3)),
                    _ => Src::None,
                }
            };
        }
        ends
    });
    // (in 2d, the ends need no z)
    if tie.as_ref().is_some_and(|t| t.iter().take(if three_d { 3 } else { 2 }).any(|s| matches!(s, Src::None | Src::Col(None)))) {
        e.warn("tieline: the tie ends' columns are not found (give 'tie end x' and its friends): ties not drawn");
    }
    let sey = pick(e, "y error", None);
    let sex = pick(e, "x error", None);
    let seyp = pick(e, "y error plus", None);
    let seym = pick(e, "y error minus", None);
    let sexp = pick(e, "x error plus", None);
    let sexm = pick(e, "x error minus", None);
    let box_filter = match e.tex.meaning_of(&crate::keys::macro_name("/pgfplots/boxplot/data filter/.@cmd")) {
        tex::Meaning::Macro(m) if !m.body.is_empty() && !keys::value(e, "/pgfplots/@boxplot").is_empty() => Some(m.body.clone()),
        _ => None,
    };
    let mut out = Vec::with_capacity(t.rows.len());
    let mut rows = Vec::with_capacity(t.rows.len());
    for r in 0..t.rows.len() {
        // an expression may use the coordinates read so far: `z expr=x^2-y^2`
        let known: std::cell::RefCell<Vec<(&str, f64)>> = Default::default();
        let get = |e: &mut Engine, s: &Src| -> f64 {
            match s {
                Src::Col(Some(c)) => t.cell(r, *c).and_then(table::parse_number).unwrap_or(f64::NAN),
                Src::Values(v) => v.get(r).copied().unwrap_or(f64::NAN),
                Src::Col(None) | Src::None => f64::NAN,
                Src::Expr(tokens) => {
                    let replaced = replace_thisrow(tokens, t, r);
                    let mut vars = fpu_vars(e);
                    for (n, v) in known.borrow().iter() {
                        vars.insert((*n).into(), *v);
                    }
                    e.math_string(replaced).and_then(|s| crate::expr::eval(&s, &vars)).unwrap_or(f64::NAN)
                }
            }
        };
        // a cell of a symbolic axis (or of a histogram's symbolic data) is a name
        let sym = |e: &mut Engine, s: &Src, c: char, v: f64| match s {
            Src::Col(Some(col)) if !v.is_finite() => {
                t.cell(r, *col).and_then(|cell| super::symbolic::index(e, c, cell).or_else(|| super::symbolic::hist_index(e, cell))).unwrap_or(v)
            }
            _ => v,
        };
        let x = get(e, &sx);
        let x = sym(e, &sx, 'x', x);
        known.borrow_mut().push(("x", x));
        let y = get(e, &sy);
        let y = sym(e, &sy, 'y', y);
        known.borrow_mut().push(("y", y));
        let z = if matches!(sz, Src::None) { 0.0 } else { get(e, &sz) };
        let mut c = Coord::new(x, y, z);
        c.meta = get(e, &smeta);
        if let Src::Col(Some(col)) = &smeta {
            if let Some(cell) = t.cell(r, *col) {
                c.label = add_label(tex::string_tokens(cell));
            }
        }
        if quiver {
            for (k, name) in ["u", "v", "w"].iter().enumerate() {
                if let Some(t) = keys::value_tokens(e, &format!("/pgfplots/quiver/{name}")) {
                    let replaced = replace_thisrow(&t, t_table, r);
                    let mut vars = fpu_vars(e);
                    vars.insert("x".into(), x);
                    vars.insert("y".into(), y);
                    vars.insert("z".into(), z);
                    c.uvw[k] = e.math_string(replaced).and_then(|s| crate::expr::eval(&s, &vars)).unwrap_or(f64::NAN);
                }
            }
        }
        if let Some((tokens, mathparse)) = &symbolic {
            // a symbolic meta is text: the cells as written (`0,0,1` is a color, not a number)
            let replaced = replace_thisrow_as(tokens, t, r, true);
            let text = e.tex.expand_to_string(replaced).unwrap_or_default();
            let l = symbolic_label(e, &text, *mathparse, [x, y, z]);
            c.label = add_label(tex::string_tokens(&l));
        }
        let ey = get(e, &sey);
        let ex = get(e, &sex);
        c.err = [ex, ex, ey, ey];
        let (p, m) = (get(e, &seyp), get(e, &seym));
        if p.is_finite() {
            c.err[3] = p;
        }
        if m.is_finite() {
            c.err[2] = m;
        }
        let (p, m) = (get(e, &sexp), get(e, &sexm));
        if p.is_finite() {
            c.err[1] = p;
        }
        if m.is_finite() {
            c.err[0] = m;
        }
        let other = tie.as_ref().map(|ends| {
            let mut b = c;
            b.x = get(e, &ends[0]);
            b.y = get(e, &ends[1]);
            b.z = get(e, &ends[2]);
            b
        });
        // `boxplot/data filter/.code`: the row's value in `\pgfmathresult` (empty: left out), with
        // `\thisrow` the row's cells
        if let Some(body) = &box_filter {
            let data = keys::value(e, "/pgfplots/boxplot/data");
            let v = match data.as_str() {
                "x" => &mut c.x,
                "z" => &mut c.z,
                _ => &mut c.y,
            };
            if v.is_finite() {
                let text = super::super::pgf_number(*v);
                let code = replace_thisrow_as(&tex::substitute(body, &[tex::string_tokens(&text)]), t, r, true);
                e.begin_group();
                e.tex.define_string("pgfmathresult", &text, false);
                let res = e.run_tokens(code).and_then(|_| e.tex.expand_to_string(vec![Token::cs("pgfmathresult")]));
                e.end_group();
                *v = table::parse_number(&res?).unwrap_or(f64::NAN);
            }
        }
        out.push(c);
        rows.push(r);
        if other.is_some() {
            rows.push(r);
        }
        out.extend(other);
    }
    FILTER_ROWS.with(|f| *f.borrow_mut() = Some((t.clone(), rows)));
    Ok(out)
}

/// `create col/linear regression={y=…}`: the least squares line through the table's points (in
/// logarithms on log axes, weighted by `variance` or `variance list`), at every row's x; its slope
/// and intercept are kept in `\pgfplotstableregressiona` and `\pgfplotstableregressionb`.
fn linear_regression(e: &mut Engine, t: &Table, options: &str) -> Vec<f64> {
    let mut opt = std::collections::HashMap::new();
    for part in crate::keys::split_top_level(options) {
        if let Some((k, v)) = part.split_once('=') {
            opt.insert(k.trim().to_string(), v.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string());
        }
    }
    let column = |name: Option<&String>, fallback: &str, index: usize| -> Option<usize> {
        let n = name.cloned().unwrap_or_else(|| fallback.to_string());
        if n.is_empty() {
            Some(index)
        } else {
            t.column(&n)
        }
    };
    let plot_x = keys::value(e, "/pgfplots/table/x");
    let xs = column(opt.get("x"), &plot_x, 0);
    let ys = column(opt.get("y"), "", 1);
    let (Some(xc), Some(yc)) = (xs, ys) else {
        e.warn("create col/linear regression: column not found");
        return vec![f64::NAN; t.rows.len()];
    };
    // the modes: given, else the axis'
    let mode = |e: &mut Engine, c: char| -> Option<f64> {
        let m = opt.get(&format!("{c}mode")).cloned().unwrap_or_default();
        let log = match m.as_str() {
            "log" => true,
            "linear" => false,
            _ => keys::value(e, &format!("/pgfplots/{c}mode")) == "log",
        };
        // the logarithm's base: `log basis`, else e
        log.then(|| keys::number(e, &format!("/pgfplots/log basis {c}")).filter(|b| *b > 0.0 && *b != 1.0).unwrap_or(std::f64::consts::E))
    };
    let (xlog, ylog) = (mode(e, 'x'), mode(e, 'y'));
    let linear_variance = opt.get("variance format").cloned().unwrap_or_else(|| keys::value(e, "/pgfplots/table/create col/linear regression/variance format")) == "linear";
    let mut variances: std::collections::VecDeque<f64> = match opt.get("variance list") {
        Some(list) => list.split(',').filter_map(|v| e.number(v.trim()).ok()).collect(),
        None => match opt.get("variance").and_then(|n| t.column(n)) {
            Some(c) => (0..t.rows.len()).map(|r| t.cell(r, c).and_then(table::parse_number).unwrap_or(1.0)).collect(),
            None => Default::default(),
        },
    };
    let log_of = |v: f64, base: Option<f64>| match base {
        Some(b) => v.ln() / b.ln(),
        None => v,
    };
    let (mut s, mut sx, mut sxx, mut sy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut parsed = Vec::with_capacity(t.rows.len());
    for r in 0..t.rows.len() {
        let x = t.cell(r, xc).and_then(table::parse_number).unwrap_or(f64::NAN);
        let y = t.cell(r, yc).and_then(table::parse_number).unwrap_or(f64::NAN);
        if !(x.is_finite() && y.is_finite()) {
            parsed.push(f64::NAN);
            continue;
        }
        let (px, py) = (log_of(x, xlog), log_of(y, ylog));
        parsed.push(px);
        let var = variances.pop_front().unwrap_or(1.0);
        let mut w = 1.0 / (var * var);
        // a variance of y given for y itself: its logarithm's is dy / (y ln b)
        if let (Some(b), true) = (ylog, linear_variance) {
            w *= (y * b.ln()).powi(2);
        }
        s += w;
        sx += px * w;
        sxx += px * px * w;
        sy += py * w;
        sxy += px * py * w;
    }
    let delta = s * sxx - sx * sx;
    let a = (s * sxy - sx * sy) / delta;
    let b = (sxx * sy - sx * sxy) / delta;
    for (name, v) in [("pgfplotstableregressiona", a), ("pgfplotstableregressionb", b)] {
        e.tex.define_string(name, &format!("{v:e}"), true);
    }
    parsed
        .into_iter()
        .map(|px| {
            let v = a * px + b;
            match ylog {
                Some(base) => base.powf(v),
                None => v,
            }
        })
        .collect()
}

/// `\thisrow{name}`, `\thisrowno{i}`, `\coordindex`, `\lineno` replaced by the row's values.
fn replace_thisrow(tokens: &[Token], t: &Table, row: usize) -> Vec<Token> {
    replace_thisrow_as(tokens, t, row, false)
}

/// The same, the cells as their text (`raw`) or as numbers in parentheses.
fn replace_thisrow_as(tokens: &[Token], t: &Table, row: usize, raw: bool) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tk = &tokens[i];
        let (is_row, is_no) = (tk.is_cs("thisrow"), tk.is_cs("thisrowno"));
        if is_row || is_no {
            // (no space after the command: TeX skips those when it reads it)
            let j = i + 1;
            let (arg, end) = if j < tokens.len() && matches!(tokens[j], Token::Char(_, Cat::Begin)) {
                // the group's closing brace (or the end of an unbalanced list)
                let mut depth = 0;
                let k = (j..tokens.len())
                    .find(|&k| {
                        match &tokens[k] {
                            Token::Char(_, Cat::Begin) => depth += 1,
                            Token::Char(_, Cat::End) => depth -= 1,
                            _ => {}
                        }
                        depth == 0
                    })
                    .unwrap_or(tokens.len());
                (tex::tokens_to_string(&tokens[j + 1..k.min(tokens.len())]), k + 1)
            } else {
                (tokens.get(j).map(|t| tex::tokens_to_string(std::slice::from_ref(t))).unwrap_or_default(), j + 1)
            };
            let col = if is_no { arg.trim().parse().ok() } else { t.column(arg.trim()) };
            if raw {
                let cell = col.and_then(|c| t.cell(row, c)).unwrap_or("");
                out.extend(cell.chars().map(|c| Token::Char(c, if c.is_alphabetic() { Cat::Letter } else { Cat::Other })));
                i = end;
                continue;
            }
            let v = col.and_then(|c| t.cell(row, c)).and_then(table::parse_number).unwrap_or(f64::NAN);
            out.extend(tex::string_tokens(&format!("({})", if v.is_nan() { "nan".to_string() } else { format!("{v}") })));
            i = end;
            continue;
        }
        if tk.is_cs("coordindex") || tk.is_cs("lineno") {
            out.extend(tex::string_tokens(&row.to_string()));
            i += 1;
            continue;
        }
        out.push(tk.clone());
        i += 1;
    }
    out
}

thread_local! {
    /// The table the plot's coordinates were read from, and each coordinate's row: `\thisrow`
    /// in coordinate filters.
    static FILTER_ROWS: std::cell::RefCell<Option<(Table, Vec<usize>)>> = const { std::cell::RefCell::new(None) };
}

/// The code of a key (`x filter/.code`), if it has some: its macro.
fn filter_code(e: &Engine, path: &str) -> Option<Token> {
    let name = crate::keys::macro_name(&format!("{path}/.@cmd"));
    match e.tex.meaning_of(&name) {
        tex::Meaning::Macro(m) if !m.body.is_empty() => Some(Token::cs(&name)),
        _ => None,
    }
}

/// pgfplots' coordinate filters (`\pgfplots@coord@filter`): `x filter`, `y filter`, `z filter` as
/// code (`\pgfmathresult` in and out, empty to drop the point) and as expressions (`x`, `y`,
/// `rawx`…, `\coordindex`, `\thisrow`), then `restrict expr to domain`.
fn coordinate_filters(e: &mut Engine, plot: &mut Plot) -> Result<(), Error> {
    let rows = FILTER_ROWS.with(|f| f.borrow_mut().take()).filter(|(_, r)| r.len() == plot.coords.len());
    let dims = if plot.three_d { 3 } else { 2 };
    let mut codes: Vec<Option<Token>> = Vec::new();
    let mut exprs: Vec<Vec<Vec<Token>>> = Vec::new();
    for c in ['x', 'y', 'z'].iter().take(dims) {
        let path = format!("/pgfplots/{c} filter");
        codes.push(filter_code(e, &path));
        let list = crate::keys::value(&e.tex, &format!("{path}/@expressions")).unwrap_or_default();
        exprs.push(split_groups(&list));
    }
    let restrict = keys::value_tokens(e, "/pgfplots/@restrict expr").zip(Some(keys::value(e, "/pgfplots/@restrict expr domain")));
    let conditions = split_groups(&crate::keys::value(&e.tex, "/pgfplots/coordinate style/@conditions").unwrap_or_default());
    let from = keys::value_tokens(e, "/pgfplots/coordinate style/@from");
    let classes = if plot.scatter { scatter_classes(e) } else { Vec::new() };
    // `visualization depends on`: (expression, verbatim, macro)
    let deps: Vec<(Vec<Token>, bool, String)> = split_groups(&crate::keys::value(&e.tex, "/pgfplots/@visualization depends on").unwrap_or_default())
        .into_iter()
        .filter_map(|d| {
            let at = d.iter().position(|t| t.is_cs("as"))?;
            let name = d[at + 1..].iter().find_map(|t| if let Token::Cs(n) = t { Some(n.to_string()) } else { None })?;
            let mut expr = crate::keys::trim(&d[..at]).to_vec();
            let text = tex::tokens_to_string(&expr);
            let verbatim = text.trim_start().starts_with("value");
            if verbatim {
                expr = tex::strip_outer_braces(crate::keys::trim(&super::toks(text.trim_start().trim_start_matches("value"))).to_vec());
            }
            Some((expr, verbatim, name))
        })
        .collect();
    if codes.iter().all(Option::is_none)
        && exprs.iter().all(Vec::is_empty)
        && restrict.is_none()
        && conditions.is_empty()
        && from.is_none()
        && classes.is_empty()
        && deps.is_empty()
    {
        return Ok(());
    }
    let number = |v: f64| super::super::pgf_number(v);
    let mut drop = vec![false; plot.coords.len()];
    for k in 0..plot.coords.len() {
        let raw = plot.coords[k];
        let mut cur = [raw.x, raw.y, raw.z];
        let row = rows.as_ref().map(|(t, r)| (t, r[k]));
        let substitute = |tokens: &[Token], result: f64| -> Vec<Token> {
            let tokens = match row {
                Some((t, r)) => replace_thisrow(tokens, t, r),
                None => tokens.to_vec(),
            };
            let mut out = Vec::with_capacity(tokens.len());
            for t in tokens {
                if t.is_cs("pgfmathresult") {
                    out.extend(super::toks(&format!("({})", number(result))));
                } else if t.is_cs("coordindex") {
                    out.extend(super::toks(&k.to_string()));
                } else {
                    out.push(t);
                }
            }
            out
        };
        let eval = |e: &mut Engine, tokens: Vec<Token>, cur: [f64; 3]| -> f64 {
            let mut vars = fpu_vars(e);
            for (i, n) in ["x", "y", "z"].iter().enumerate() {
                vars.insert((*n).into(), cur[i]);
                vars.insert(format!("raw{n}"), [raw.x, raw.y, raw.z][i]);
            }
            e.math_string(tokens).and_then(|s| crate::expr::eval(&s, &vars)).unwrap_or(f64::NAN)
        };
        for a in 0..dims {
            if let Some(code) = &codes[a] {
                e.tex.define_string("pgfmathresult", &number(cur[a]), false);
                e.tex.define_string("coordindex", &k.to_string(), false);
                // the code's argument is the value too
                let mut call = vec![code.clone(), Token::Char('{', Cat::Begin)];
                call.extend(super::toks(&number(cur[a])));
                call.push(Token::Char('}', Cat::End));
                e.run_tokens(call)?;
                let out = e.tex.expand_to_string(vec![Token::cs("pgfmathresult")])?;
                let out = out.trim();
                if out.is_empty() {
                    drop[k] = true;
                    break;
                }
                cur[a] = e.number(out).unwrap_or(f64::NAN);
            }
            for x in exprs[a].clone() {
                let tokens = substitute(&x, cur[a]);
                cur[a] = eval(e, tokens, cur);
            }
        }
        if let Some((expr, domain)) = &restrict {
            if let Some((lo, hi)) = domain.split_once(':') {
                let (lo, hi) = (e.number(lo)?, e.number(hi)?);
                let v = eval(e, substitute(expr, cur[0]), cur);
                if !(lo..=hi).contains(&v) {
                    cur[0] = f64::INFINITY;
                }
            }
        }
        // the coordinate's own style: its `scatter/classes` class (by its symbolic point meta),
        // `.from` (a table cell), then the conditions that hold
        let mut style = Vec::new();
        // (the labels are still being collected)
        let label = (raw.label != NO_LABEL).then(|| LABELS.with(|l| l.borrow().get(raw.label as usize).cloned())).flatten();
        if let Some(l) = label {
            let name = tex::tokens_to_string(&l);
            if let Some((_, s)) = classes.iter().find(|(n, _)| *n == name.trim()) {
                style.extend(s.clone());
            }
        }
        if let Some(from) = &from {
            // `\thisrow{style}`: the cell's text
            let text = tex::tokens_to_string(from);
            let cell = text.trim().strip_prefix("\\thisrow").map(|n| n.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string());
            let text = match (cell, row) {
                (Some(name), Some((t, r))) => t.column(&name).and_then(|c| t.cell(r, c)).unwrap_or("").to_string(),
                _ => e.tex.expand_to_string(from.clone())?,
            };
            let text = text.trim();
            let text = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')).unwrap_or(text);
            style.extend(super::toks(text.trim()));
        }
        for pair in conditions.chunks(2) {
            let [test, s] = pair else { break };
            let v = eval(e, substitute(test, cur[0]), cur);
            if v != 0.0 && !v.is_nan() {
                if !style.is_empty() {
                    style.push(Token::other(','));
                }
                style.extend(s.clone());
            }
        }
        let mut values = Vec::new();
        for (expr, verbatim, name) in &deps {
            let tokens = substitute(expr, cur[0]);
            let v = if *verbatim { e.tex.expand_to_string(tokens)?.trim().to_string() } else { super::super::pgf_number(eval(e, tokens, cur)) };
            values.push((name.clone(), v));
        }
        if !values.is_empty() {
            plot.coords[k].deps = plot.deps.len() as u32;
            plot.deps.push(values);
        }
        let c = &mut plot.coords[k];
        (c.x, c.y, c.z) = (cur[0], cur[1], cur[2]);
        if style.iter().any(|t| !t.is_space()) {
            c.style = plot.coord_styles.len() as u32;
            plot.coord_styles.push(style);
        }
    }
    if drop.iter().any(|d| *d) {
        let mut k = 0;
        plot.coords.retain(|_| {
            k += 1;
            !drop[k - 1]
        });
    }
    Ok(())
}

/// `scatter/classes={a={mark=square*,blue},b={…}}`: each class' name and style.
pub fn scatter_classes(e: &mut Engine) -> Vec<(String, Vec<Token>)> {
    let Some(v) = keys::value_tokens(e, "/pgfplots/scatter/classes") else { return Vec::new() };
    crate::keys::split_list(&v).into_iter().map(|i| (i.key.trim().to_string(), i.value.map(tex::strip_outer_braces).unwrap_or_default())).collect()
}

/// The brace groups of a list (`{a}{b}`).
fn split_groups(list: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = Vec::new();
    for t in list {
        match t {
            Token::Char(_, Cat::Begin) => {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            }
            Token::Char(_, Cat::End) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push(std::mem::take(&mut cur));
                    continue;
                }
            }
            _ => {}
        }
        if depth > 0 {
            cur.push(t.clone());
        }
    }
    out
}

/// `restrict x to domain`, `restrict y to domain`, `unbounded coords`.
fn filter(e: &mut Engine, plot: &mut Plot) -> Result<(), Error> {
    // a restriction makes the coordinates outside unbounded, and unbounded coordinates jump
    let mut restricted = false;
    for (axis, key) in [('x', "/pgfplots/restrict x to domain"), ('y', "/pgfplots/restrict y to domain")] {
        let d = keys::value(e, key);
        if let Some((a, b)) = d.split_once(':') {
            restricted = true;
            let (a, b) = (e.number(a)?, e.number(b)?);
            for c in &mut plot.coords {
                let v = if axis == 'x' { &mut c.x } else { &mut c.y };
                if *v < a || *v > b {
                    *v = f64::NAN;
                }
            }
        }
    }
    let jump = restricted || keys::value(e, "/pgfplots/unbounded coords") == "jump";
    let log = (keys::value(e, "/pgfplots/xmode") == "log", keys::value(e, "/pgfplots/ymode") == "log");
    for c in &mut plot.coords {
        if (log.0 && c.x <= 0.0) || (log.1 && c.y <= 0.0) {
            c.x = f64::NAN;
        }
        if !(c.x.is_finite() && c.y.is_finite() && (!plot.three_d || c.z.is_finite())) {
            c.x = f64::NAN;
        }
    }
    if !jump && plot.lattice.is_none() {
        plot.coords.retain(|c| c.x.is_finite());
    }
    Ok(())
}

/// An expression with the parentheses still open at its end closed: pgfmath reads
/// `(sqrt((1)^2 + (2*x)^2)` (the manual's quiver example) as if they were.
fn balanced(text: &str) -> String {
    let open = text.chars().filter(|c| *c == '(').count();
    let close = text.chars().filter(|c| *c == ')').count();
    format!("{text}{}", ")".repeat(open.saturating_sub(close)))
}

/// `quiver`: each point's (u, v, w), from `quiver/u` and its friends (expressions of x, y, z and
/// the sampling variable), times `scale arrows`.
fn quiver_vectors(e: &mut Engine, plot: &mut Plot) {
    let var = e.st.variable.clone();
    let var_y = keys::value(e, "/pgfplots/variable y").trim_start_matches('\\').to_string();
    let texts: Vec<Option<String>> = ["u", "v", "w"].iter().map(|n| keys::value_tokens(e, &format!("/pgfplots/quiver/{n}")).and_then(|t| e.math_string(t).ok())).collect();
    let scale = keys::number(e, "/pgfplots/quiver/scale arrows").unwrap_or(1.0);
    let mut vars = fpu_vars(e);
    for c in plot.coords.iter_mut() {
        vars.insert("x".into(), c.x);
        vars.insert("y".into(), c.y);
        vars.insert("z".into(), c.z);
        if c.param[0].is_finite() {
            vars.insert(var.clone(), c.param[0]);
        }
        if c.param[1].is_finite() && !var_y.is_empty() {
            vars.insert(var_y.clone(), c.param[1]);
        }
        for (u, text) in c.uvw.iter_mut().zip(&texts) {
            if !u.is_finite() {
                *u = match text {
                    Some(t) if !t.trim().is_empty() => crate::expr::eval(&balanced(t), &vars).unwrap_or(f64::NAN),
                    _ => 0.0,
                };
            }
            *u *= scale;
        }
    }
}

/// `data cs`: the input's coordinate system (`cart`, `polar` in degrees, `polarrad`) turned into
/// the axis' (`polar` for a polar axis).
fn data_cs(e: &mut Engine, plot: &mut Plot) {
    let source = keys::value(e, "/pgfplots/data cs");
    let target = keys::value(e, "/pgfplots/@axis type");
    if source == target || source.is_empty() {
        return;
    }
    for c in plot.coords.iter_mut() {
        if source == "polarrad" {
            c.x = c.x.to_degrees();
        }
        let (x, y) = (c.x, c.y);
        match (source.as_str(), target.as_str()) {
            ("polar" | "polarrad", "cart") => {
                let (s, co) = super::polar::sin_cos(x);
                (c.x, c.y) = (y * co, y * s);
            }
            ("cart", "polar") => {
                (c.x, c.y) = (y.atan2(x).to_degrees().rem_euclid(360.0), x.hypot(y));
            }
            _ => {}
        }
    }
}

/// `hist`: the points become the bins' counts, `(start of bin k, count k)`, and a last point at
/// the end of the data range (`\pgfplotsplothandlersurveyend@hist@`).
fn histogram(e: &mut Engine, plot: &mut Plot) -> Result<(), Error> {
    let data = keys::value(e, "/pgfplots/hist/data");
    let value_expr = keys::value(e, "/pgfplots/hist/data value");
    let mut vars = fpu_vars(e);
    let mut values = Vec::new();
    for c in &plot.coords {
        let v = if !value_expr.is_empty() {
            vars.insert("x".into(), c.x);
            vars.insert("y".into(), c.y);
            vars.insert("z".into(), c.z);
            crate::expr::eval(&value_expr, &vars).unwrap_or(f64::NAN)
        } else {
            match data.as_str() {
                "x" => c.x,
                "z" => c.z,
                "y" | "" => c.y,
                other => {
                    vars.insert("x".into(), c.x);
                    vars.insert("y".into(), c.y);
                    vars.insert("z".into(), c.z);
                    crate::expr::eval(other, &vars).unwrap_or(f64::NAN)
                }
            }
        };
        if v.is_finite() {
            values.push(v);
        }
    }
    let bins = keys::number(e, "/pgfplots/hist/bins").unwrap_or(10.0).max(1.0) as usize;
    // `data min` defaults to `xmin`
    let limit = |e: &mut Engine, k: &str, axis: &str| keys::number(e, &format!("/pgfplots/hist/{k}")).or_else(|| keys::number(e, &format!("/pgfplots/{axis}")));
    let lo = limit(e, "data min", "xmin").unwrap_or_else(|| values.iter().copied().fold(f64::INFINITY, f64::min));
    let hi = limit(e, "data max", "xmax").unwrap_or_else(|| values.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    if values.is_empty() || !(lo.is_finite() && hi.is_finite()) {
        plot.coords.clear();
        return Ok(());
    }
    let h = (hi - lo) / bins as f64;
    let mut counts = vec![0.0f64; bins];
    for v in &values {
        let k = if h != 0.0 { ((v - lo) / h + 1e-4).trunc() } else { 0.0 };
        let k = if k < 0.0 { 0 } else { (k as usize).min(bins - 1) };
        counts[k] += 1.0;
    }
    let total: f64 = counts.iter().sum();
    let cumulative = keys::flag(e, "/pgfplots/hist/cumulative");
    let density = keys::flag(e, "/pgfplots/hist/density");
    if cumulative {
        let mut acc = 0.0;
        for c in counts.iter_mut() {
            acc += *c;
            *c = if density { acc / total } else { acc };
        }
    } else if density && total > 0.0 && h != 0.0 {
        for c in counts.iter_mut() {
            *c /= total * h;
        }
    }
    let mut coords: Vec<Coord> = counts.iter().enumerate().map(|(k, c)| Coord::new(lo + h * k as f64, *c, 0.0)).collect();
    if keys::flag(e, "/pgfplots/hist/intervals") {
        coords.push(Coord::new(hi, counts[bins - 1], 0.0));
    }
    plot.coords = coords;
    plot.three_d = false;
    plot.scanlines.clear();
    plot.handler = keys::value(e, "/tikz/@plot handler");
    Ok(())
}

/// `point meta={symbolic={R,G,B}}`: the text of each point's meta, and whether
/// `mesh/color input=explicit mathparse` evaluates it.
fn symbolic_meta(e: &mut Engine) -> Option<(Vec<Token>, bool)> {
    let raw = crate::keys::value(&e.tex, "/pgfplots/point meta")?;
    let items = crate::keys::split_list(crate::keys::trim(&raw));
    let item = items.first().filter(|i| i.key == "symbolic" && items.len() == 1)?;
    let mathparse = keys::value(e, "/pgfplots/mesh/color input") == "explicit mathparse";
    Some((item.value.clone().unwrap_or_default(), mathparse))
}

/// A symbolic point meta for a point: with `mathparse`, each part (`R,G,B`, `wave=…`) evaluated.
fn symbolic_label(e: &mut Engine, text: &str, mathparse: bool, xyz: [f64; 3]) -> String {
    if !mathparse {
        return text.trim().to_string();
    }
    let mut vars = fpu_vars(e);
    vars.insert("x".into(), xyz[0]);
    vars.insert("y".into(), xyz[1]);
    vars.insert("z".into(), xyz[2]);
    let (model, rest) = match text.split_once('=') {
        Some((m, r)) if m.trim().chars().all(|c| c.is_ascii_alphanumeric()) => (format!("{}=", m.trim()), r),
        _ => (String::new(), text),
    };
    let parts: Vec<String> =
        crate::keys::split_top_level(rest).iter().map(|p| crate::expr::eval(p.trim(), &vars).map(|v| format!("{v}")).unwrap_or_else(|_| p.trim().to_string())).collect();
    format!("{model}{}", parts.join(","))
}

/// `point meta`: the value each point maps to the colormap with.
fn point_meta(e: &mut Engine, plot: &mut Plot) -> Result<(), Error> {
    if let Some((tokens, mathparse)) = symbolic_meta(e) {
        // tables have set them row by row
        if plot.coords.iter().all(|c| c.label == NO_LABEL) {
            let text = e.tex.expand_to_string(tokens).unwrap_or_default();
            for i in 0..plot.coords.len() {
                let c = plot.coords[i];
                let l = symbolic_label(e, &text, mathparse, [c.x, c.y, c.z]);
                plot.coords[i].label = add_label(tex::string_tokens(&l));
            }
        }
        plot.has_meta = false;
        return Ok(());
    }
    let raw = crate::keys::value(&e.tex, "/pgfplots/point meta").unwrap_or_default();
    let source = tex::tokens_to_string(crate::keys::trim(&raw)).trim().to_string();
    match source.as_str() {
        "" | "none" => {}
        "x" => plot.coords.iter_mut().for_each(|c| c.meta = c.x),
        "y" => plot.coords.iter_mut().for_each(|c| c.meta = c.y),
        "z" => plot.coords.iter_mut().for_each(|c| c.meta = c.z),
        "f(x)" => {
            let three = plot.three_d;
            plot.coords.iter_mut().for_each(|c| c.meta = if three { c.z } else { c.y })
        }
        "explicit" | "explicit symbolic" => {}
        _ => {
            // \thisrow{…} is evaluated with the table's rows
            if !source.contains("\\thisrow") {
                // `\coordindex` is the point's number
                let per_point = source.contains("\\coordindex");
                let mut text = if per_point { String::new() } else { e.math_string(raw.clone())? };
                let mut vars = fpu_vars(e);
                for (i, c) in plot.coords.iter_mut().enumerate() {
                    if per_point {
                        e.tex.define_string("coordindex", &i.to_string(), false);
                        text = e.math_string(raw.clone())?;
                    }
                    vars.insert("x".into(), c.x);
                    vars.insert("y".into(), c.y);
                    vars.insert("z".into(), c.z);
                    c.meta = crate::expr::eval(&balanced(&text), &vars).unwrap_or(f64::NAN);
                }
            }
        }
    }
    plot.has_meta = !matches!(source.as_str(), "" | "none" | "explicit symbolic");
    Ok(())
}

/// A lattice from `mesh/rows`, `mesh/cols` or the input's scanlines.
fn lattice_from_scanlines(e: &mut Engine, plot: &mut Plot) {
    if plot.lattice.is_some() || plot.mesh.is_none() {
        return;
    }
    let n = plot.coords.len();
    let rows = keys::number(e, "/pgfplots/mesh/rows").map(|v| v as usize);
    let cols = keys::number(e, "/pgfplots/mesh/cols").map(|v| v as usize);
    let lattice = match (rows, cols) {
        (_, Some(c)) if c > 0 => Some((n / c, c)),
        (Some(r), _) if r > 0 => Some((r, n / r)),
        // patches are their own (`mesh input=patches`)
        _ if plot.patch => None,
        _ => plot.scanlines.first().filter(|c| **c > 0 && n.is_multiple_of(**c)).map(|c| (n / c, *c)).or_else(|| {
            // without scanlines a matrix is square (`matrix delinearize uses sqrt`)
            let s = (n as f64).sqrt().round() as usize;
            (plot.image && s * s == n && s > 0).then_some((s, s))
        }),
    };
    if let Some((r, c)) = lattice.filter(|(r, c)| r * c == n && *r > 1 && *c > 1) {
        plot.lattice = Some((r, c));
    }
}
