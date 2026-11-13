//! Path statements: `\path[options] <operations> ;`, read the way `tikz.code.tex` reads them, token
//! by token after expansion: coordinates, `--`, `-|`, `|-`, `.. controls ..`, `to`, `edge`,
//! `rectangle`, `circle`, `ellipse`, `arc`, `grid`, `plot`, `sin`, `cos`, `parabola`, `cycle`,
//! `let`, `node`, `coordinate`, `\foreach` and `\pgfextra` inside the path, options and `{…}` groups.

use crate::canvas::Item;
use crate::keys;
use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Cat, Meaning, Token};
use crate::Error;

use super::coords::{read_coordinate_tokens, Coord};
use super::nodes::{NodeSpec, OnPath};
use super::state::Mode;
use super::{macro_from, tikra_tokens, Engine};
use crate::describe::{Arrow, Link};

/// Largest accepted coordinate, in cm (one kilometer): beyond that it is a typo or a bad computation.
pub const MAX_COORDINATE: f64 = 100_000.0;
/// Cap on `grid` lines in each direction.
pub const MAX_GRID_LINES: usize = 10_000;

pub fn valid_point(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.x.abs() <= MAX_COORDINATE && p.y.abs() <= MAX_COORDINATE
}

pub fn valid_segment(s: &Segment) -> bool {
    match s {
        Segment::MoveTo(p) | Segment::LineTo(p) => valid_point(*p),
        Segment::CurveTo(a, b, p) => valid_point(*a) && valid_point(*b) && valid_point(*p),
        Segment::Close => true,
    }
}

/// A node written between an operation and its target (`-- node {a} (B)`), placed once the segment exists.
struct MidNode {
    options: Vec<Token>,
    name: Option<String>,
    text: Vec<Token>,
    coordinate: bool,
    /// a pic (`to … pic {type} (b)`), placed as a node is
    pic: bool,
}

/// An `edge`: its segments, options, nodes and connections.
type EdgePath = (Vec<Segment>, Vec<Token>, Vec<Item>, Vec<Link>);

struct Builder {
    segs: Vec<Segment>,
    current: Option<Coord>,
    subpath_start: Option<Point>,
    last: Option<OnPath>,
    node_items: Vec<Item>,
    mid_nodes: Vec<MidNode>,
    /// `edge` paths, drawn after the main path: (segments, options, nodes, connections)
    edges: Vec<EdgePath>,
    /// the point a `+` coordinate is relative to (set by `+`, not moved)
    relative_base: Option<Point>,
    /// where a path is before its first point: the origin of its coordinate system
    origin: Point,
    /// the operation's target was `cycle`: close the subpath once it is done
    close_after: bool,
    /// the node just made: a `to` or `edge` right after it starts there (`\tikz@to@use@last@fig@name`)
    to_start: Option<Coord>,
    /// options given after the last point changed the transformation: a node placed there moves as
    /// the point would under the new one (`(0,0) [yshift=8pt] node {…}`), from the old to the new
    node_move: Option<Transform>,
    /// the current point where each `{` of the path opened, for `current point is local`
    groups: Vec<Option<Coord>>,
    /// the lines from node to node, for the figure's description; the node the path last was at,
    /// and whether the last operation made a line (its nodes are its labels)
    links: Vec<Link>,
    from_node: Option<String>,
    fresh_link: bool,
}

impl Builder {
    fn point(&self) -> Point {
        self.current.as_ref().map_or(self.origin, |c| c.p)
    }

    /// The last point and the point the path came to it from (`\tikz@tangent`), for `turn`.
    fn tangent(&self) -> (Point, Point) {
        let last = self.point();
        let n = self.segs.len();
        // (a line after a close starts with a move: the segment before a line has an end)
        let end = |s: &Segment| if let Segment::MoveTo(p) | Segment::LineTo(p) | Segment::CurveTo(_, _, p) = s { Some(*p) } else { None };
        let from = match self.segs.last() {
            Some(Segment::CurveTo(_, c2, _)) => Some(*c2),
            Some(Segment::LineTo(_)) if n >= 2 => end(&self.segs[n - 2]),
            _ => None,
        };
        (last, from.unwrap_or(last))
    }

    fn move_to(&mut self, r: Coord) {
        self.from_node = r.node.clone();
        self.segs.push(Segment::MoveTo(r.p));
        self.subpath_start = Some(r.p);
        self.last = Some(OnPath::Point(r.p));
        self.current = Some(r);
    }
}

/// A path statement (`\draw`, `\fill`, `\path`, `\node`…): `defaults` are the command's options.
pub fn statement(e: &mut Engine, defaults: &str) -> Result<(), Error> {
    e.begin_group();
    // the state before the path's options: decorations and preactions draw with its graphics
    let outer = e.st.clone();
    e.st.mode = Mode::default();
    e.st.pending = crate::scene::Transform::default();
    reset_path_settings(e);
    // a `scope fading` of a path that drew nothing fades the next one
    let carried = std::mem::take(&mut e.carried_fadings);
    e.path_fadings.push(carried.into_iter().map(|(name, fit, t)| (name, e.canvas.items.len(), fit.is_some(), t, fit)).collect());
    e.path_fading_settings.push((e.st.fading_transform, e.st.fit_fading));
    // `transform canvas` among the path's options (or an animated `:rotate`) is for this path only;
    // a clip it makes stays for the rest of the scope
    let (opened, transforms) = (e.scopes.last().map(|s| s.ends), e.canvas_transforms);
    let r = statement_in_group(e, defaults, &outer);
    if let (Some(before), Some(s)) = (opened, e.scopes.last_mut()) {
        let mut own = e.canvas_transforms - transforms;
        while own > 0 && s.ends > before {
            s.ends -= 1;
            own -= 1;
            e.canvas.push(crate::canvas::Item::End);
        }
    }
    e.canvas_transforms = transforms;
    let fadings = e.path_fadings.pop().unwrap_or_default();
    let settings = e.path_fading_settings.pop().unwrap_or((crate::scene::Transform::default(), true));
    e.close_path_fadings(fadings, settings);
    e.end_group();
    r
}

/// `lindenmayer system [options]` and `l-system [options]`: the system's drawing, from the current
/// point.
fn lindenmayer_operation(e: &mut Engine, b: &mut Builder, depth: Option<usize>, w: &str) -> Result<(), Error> {
    let rest = if w == "l" {
        match next(e, depth)? {
            Some(t) if t.is_char('-') => match next(e, depth)? {
                Some(Token::Char(c, Cat::Letter)) => word(e, c),
                _ => String::new(),
            },
            _ => String::new(),
        }
    } else {
        match next(e, depth)? {
            Some(Token::Char(c, Cat::Letter)) => word(e, c),
            _ => String::new(),
        }
    };
    if rest != "system" {
        e.warn(format!("unrecognized path fragment: '{w}'"));
        return skip_rest(e, depth);
    }
    if let Some(t) = peek(e, depth)? {
        if t.is_char('[') {
            let _ = next(e, depth)?;
            let o = e.tex.read_until_char(']', false)?;
            e.process_keys(o, Some("/tikz"))?;
        }
    }
    let segs = e.lindenmayer_path(b.point())?;
    for s in segs {
        let (Segment::MoveTo(p) | Segment::LineTo(p)) = s else { unreachable!("an L-system moves and draws lines only") };
        if matches!(s, Segment::MoveTo(_)) {
            b.subpath_start = Some(p);
        } else {
            b.last = Some(OnPath::Segment(b.point(), None, p));
        }
        b.segs.push(s);
        b.current = Some(Coord { p, node: None });
    }
    Ok(())
}

/// `insert path={…}` in options just read: the path operations go on from there.
fn insert_path(e: &mut Engine) {
    // (the key defines the macro, empty at first)
    let Meaning::Macro(m) = e.tex.meaning_of("tikra@insertpath") else { return };
    if !m.body.is_empty() {
        let v = tex::substitute(&m.body, &[]);
        e.tex.define_string("tikra@insertpath", "", false);
        e.tex.push_tokens(v);
    }
}

/// What `\path` resets: `decorate`, `preaction`, `postaction` belong to one path.
fn reset_path_settings(e: &mut Engine) {
    e.st.decorate = false;
    e.st.preactions = Default::default();
    e.st.postactions = Default::default();
}

fn statement_in_group(e: &mut Engine, defaults: &str, outer: &super::GState) -> Result<(), Error> {
    e.process_keys(tikra_tokens("every path/.try"), Some("/tikz"))?;
    if !defaults.is_empty() {
        e.process_keys(tikra_tokens(defaults), Some("/tikz"))?;
    }
    // options right after the command, possibly several groups
    loop {
        match e.tex.get_x_token()? {
            Some(t) if t.is_space() => continue,
            Some(t) if t.is_char('[') => {
                let o = e.tex.read_until_char(']', false)?;
                e.process_keys(o, Some("/tikz"))?;
            }
            Some(t) => {
                e.tex.back(t);
                break;
            }
            None => break,
        }
    }
    e.st.apply_pending();
    insert_path(e);
    let mut b = Builder {
        segs: Vec::new(),
        current: None,
        subpath_start: None,
        last: None,
        node_items: Vec::new(),
        mid_nodes: Vec::new(),
        edges: Vec::new(),
        relative_base: None,
        origin: e.st.transform.apply(Point::default()),
        close_after: false,
        to_start: None,
        node_move: None,
        groups: Vec::new(),
        links: Vec::new(),
        from_node: None,
        fresh_link: false,
    };
    let (color, dash, start) = (e.st.stroke_color(), e.st.dash.clone(), e.canvas.items.len());
    operations(e, &mut b, None)?;
    if b.current.is_some() {
        e.last_xy = b.point();
    }
    // a node left between an operation and a target that never came
    place_mid_nodes(e, &mut b, None)?;
    // options in the middle of the path (`[dashed, red]`) are the whole path's: TeX draws its nodes
    // after it, with its final color and dashes where they have their own none
    if (e.st.stroke_color(), &e.st.dash) != (color, &dash) {
        let (to_color, to_dash) = (e.st.stroke_color(), e.st.dash.clone());
        for item in e.canvas.items[start..].iter_mut().chain(b.node_items.iter_mut()) {
            if let crate::canvas::Item::Path { stroke: Some(st), .. } = item {
                if st.color == color {
                    st.color = to_color;
                }
                if st.dash == dash {
                    st.dash = to_dash.clone();
                }
            }
        }
    }
    let mut segs = std::mem::take(&mut b.segs);
    // a move to a node waits for the next point, which tells it where on the border to start
    // (`\tikz@moveto@waiting`): alone (`\draw (a) edge (b);`), it never reaches the path
    if matches!(segs[..], [Segment::MoveTo(_)]) && b.current.as_ref().is_some_and(|c| c.node.is_some()) {
        segs.clear();
    }
    if let Some(name) = e.st.name_path.take() {
        e.named_paths.insert(name, segs.clone());
    }
    let behind = std::mem::take(&mut e.behind_path);
    e.canvas.items.extend(behind);
    // `current path bounding box`, and what a `scope fading` of the path is fitted to
    let mut bounds = crate::canvas::Canvas::default();
    bounds.include_path(&segs);
    e.st.path_box = bounds.min.zip(bounds.max);
    let m = &e.st.mode;
    if !segs.is_empty() && e.facts.mute == 0 && (m.draw || m.fill || m.shade || m.pattern) {
        e.facts.shapes += 1;
        if let Some(id) = e.source {
            *e.facts.shapes_of.entry(id).or_default() += 1;
        }
    }
    e.note_links(std::mem::take(&mut b.links));
    e.finish_path(segs, outer, "path")?;
    for (segs, options, items, links) in std::mem::take(&mut b.edges) {
        edge_path(e, segs, options, links)?;
        e.canvas.items.extend(items);
    }
    let items = std::mem::take(&mut b.node_items);
    e.canvas.items.extend(items);
    Ok(())
}

fn edge_path(e: &mut Engine, segs: Vec<Segment>, options: Vec<Token>, links: Vec<Link>) -> Result<(), Error> {
    e.begin_group();
    let outer = e.st.clone();
    e.st.mode = Mode::default();
    reset_path_settings(e);
    let r = (|| {
        e.process_keys(tikra_tokens("every edge/.try"), Some("/tikz"))?;
        e.process_keys(options, Some("/tikz"))?;
        e.note_links(links);
        e.finish_path(segs, &outer, "edge")
    })();
    e.end_group();
    r
}

/// The next token that is not a space, expanded.
fn next(e: &mut Engine, depth: Option<usize>) -> Result<Option<Token>, Error> {
    loop {
        let t = match depth {
            Some(d) => e.tex.get_x_token_until(d)?,
            None => e.tex.get_x_token()?,
        };
        match t {
            Some(t) if t.is_space() || t.is_cs("par") || t.is_cs("relax") => continue,
            other => return Ok(other),
        }
    }
}

fn peek(e: &mut Engine, depth: Option<usize>) -> Result<Option<Token>, Error> {
    let t = next(e, depth)?;
    if let Some(t) = &t {
        e.tex.back(t.clone());
    }
    Ok(t)
}

/// Reads a word of letters (`rectangle`, `node`), the first one already read.
fn word(e: &mut Engine, first: char) -> String {
    let mut w = String::new();
    w.push(first);
    while let Some(t) = e.tex.get_token() {
        match t {
            Token::Char(c, Cat::Letter) => w.push(c),
            other => {
                e.tex.back(other);
                break;
            }
        }
        if w.len() > 64 {
            break;
        }
    }
    w
}

/// `(…)` read and evaluated (the `(` already read).
fn coordinate(e: &mut Engine) -> Result<Coord, Error> {
    let tokens = read_coordinate_tokens(e)?;
    let text = e.math_string(tokens)?;
    let c = e.coordinate(&text)?;
    e.facts.touched = named_node(e, &text);
    Ok(c)
}

/// The node a coordinate's text names, with or without an anchor: `a`, `a.east`, `a.30`.
fn named_node(e: &Engine, text: &str) -> Option<String> {
    let t = text.trim();
    let name = if e.lookup_node(t).is_some() { t } else { t.rsplit_once('.').map(|(n, _)| n.trim()).filter(|n| e.lookup_node(n).is_some())? };
    Some(e.node_key(name))
}

/// A node that a connection can join: a coordinate is a point the line goes through.
fn joinable(e: &Engine, name: &str) -> bool {
    e.nodes.get(name).is_some_and(|n| n.shape != "coordinate")
}

/// A line just made to `r`: a connection when it goes from a node to a node.
fn note_link(e: &mut Engine, b: &mut Builder, r: &Coord) {
    let touched = e.facts.touched.take();
    let Some(to) = r.node.clone().or(touched).filter(|n| joinable(e, n)) else { return };
    if let Some(from) = b.from_node.clone().filter(|n| joinable(e, n)) {
        b.links.push(Link { from, to: to.clone(), arrow: Arrow::None, labels: Vec::new(), source: None });
        b.fresh_link = true;
    }
    b.from_node = Some(to);
}

/// A point that must follow: skips spaces and reads `(…)`, with an optional `+`/`++`.
fn target(e: &mut Engine, b: &mut Builder, depth: Option<usize>, what: &str) -> Result<Coord, Error> {
    e.turn_from = b.tangent();
    e.subpath_start = b.subpath_start.unwrap_or_else(|| b.point());
    e.facts.touched = None;
    match next(e, depth)? {
        Some(t) if t.is_char('(') => coordinate(e),
        Some(t) if t.is_char('+') => relative(e, b, depth),
        // `cycle`: back to where the subpath started, closed once the operation is done
        Some(Token::Char('c', Cat::Letter)) => {
            let w = word(e, 'c');
            if w != "cycle" {
                e.tex.push_tokens(word_tokens(&w));
                return Err(Error::Figure(format!("{what} without a point")));
            }
            b.close_after = true;
            Ok(Coord { p: b.subpath_start.unwrap_or_else(|| b.point()), node: None })
        }
        _ => Err(Error::Figure(format!("{what} without a point"))),
    }
}

/// The point a `+(x,y)` is relative to: the last point not given with `+`.
fn relative_base(b: &Builder) -> Point {
    b.relative_base.unwrap_or_else(|| b.point())
}

/// `+(x,y)` or `++(x,y)` (the first `+` already read).
fn relative(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<Coord, Error> {
    let double = match next(e, depth)? {
        Some(t) if t.is_char('+') => true,
        Some(t) => {
            e.tex.back(t);
            false
        }
        None => false,
    };
    match next(e, depth)? {
        Some(t) if t.is_char('(') => {}
        _ => return Err(Error::Figure("'+' without a point".into())),
    }
    let r = coordinate(e)?;
    // the offset uses only the linear part of the transformation
    let origin = e.st.transform.apply(Point::default());
    let base = relative_base(b);
    let p = base.plus(r.p.minus(origin));
    if double {
        b.relative_base = None;
    } else {
        b.relative_base = Some(base);
    }
    Ok(Coord { p, node: None })
}

/// Runs path operations until `;` (or the end of the frame above `depth`).
fn operations(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    let Some(d) = depth else { return operations_in(e, b, None) };
    // a word or a number at the end of the frame reads no further
    let floor = e.tex.seal(d);
    let r = operations_in(e, b, depth);
    e.tex.unseal(floor);
    r
}

fn operations_in(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    // the operation waiting for its target: "--", "-|", "|-", or "to" with its options
    let mut pending_op: Option<String> = None;
    let mut to_options: Vec<Token> = Vec::new();
    loop {
        if std::mem::take(&mut b.close_after) {
            b.segs.push(Segment::Close);
            if let Some(p0) = b.subpath_start {
                b.current = Some(Coord { p: p0, node: None });
            }
        }
        let Some(t) = next(e, depth)? else {
            if depth.is_none() {
                e.warn("path without ';' at the end");
            }
            return Ok(());
        };
        e.steps += 1;
        if e.steps > super::MAX_STEPS {
            return Err(Error::Figure(format!("figure too large: more than {} statements", super::MAX_STEPS)));
        }
        match &t {
            Token::Char(';', _) => return Ok(()),
            Token::Char('(', _) => {
                e.turn_from = b.tangent();
                e.subpath_start = b.subpath_start.unwrap_or_else(|| b.point());
                let r = coordinate(e)?;
                b.relative_base = None;
                // a `to` right after a node starts at that node's border
                if pending_op.as_deref() != Some("to") {
                    b.to_start = None;
                }
                b.node_move = None;
                apply_operation(e, b, pending_op.take(), std::mem::take(&mut to_options), r)?;
            }
            Token::Char('+', _) => {
                e.turn_from = b.tangent();
                e.subpath_start = b.subpath_start.unwrap_or_else(|| b.point());
                let r = relative(e, b, depth)?;
                if pending_op.as_deref() != Some("to") {
                    b.to_start = None;
                }
                b.node_move = None;
                apply_operation(e, b, pending_op.take(), std::mem::take(&mut to_options), r)?;
            }
            Token::Char('-', _) => match e.tex.get_token() {
                Some(n) if n.is_char('-') => pending_op = Some("--".into()),
                Some(n) if n.is_char('|') => pending_op = Some("-|".into()),
                other => {
                    if let Some(o) = other {
                        e.tex.back(o);
                    }
                    e.warn("unrecognized path fragment: '-'");
                    return skip_rest(e, depth);
                }
            },
            Token::Char('|', _) => match e.tex.get_token() {
                Some(n) if n.is_char('-') => pending_op = Some("|-".into()),
                other => {
                    if let Some(o) = other {
                        e.tex.back(o);
                    }
                    e.warn("unrecognized path fragment: '|'");
                    return skip_rest(e, depth);
                }
            },
            Token::Char('.', _) => {
                // .. controls (a) and (b) ..
                match e.tex.get_token() {
                    Some(n) if n.is_char('.') => {}
                    _ => {
                        e.warn("unrecognized path fragment: '.'");
                        return skip_rest(e, depth);
                    }
                }
                let mut w = match next(e, depth)? {
                    Some(Token::Char(c, Cat::Letter)) => word(e, c),
                    _ => String::new(),
                };
                // `.. coordinate (bc) controls …`: nodes on the curve, before its controls
                while w == "node" || w == "coordinate" {
                    let coordinate = w == "coordinate";
                    let (options, name, _, text) = read_node(e, depth, coordinate, b.point())?;
                    b.mid_nodes.push(MidNode { options, name, text, coordinate, pic: false });
                    w = match next(e, depth)? {
                        Some(Token::Char(c, Cat::Letter)) => word(e, c),
                        _ => String::new(),
                    };
                }
                if w != "controls" {
                    e.warn("'..' without 'controls'");
                    return skip_rest(e, depth);
                }
                let c1 = target(e, b, depth, "controls")?.p;
                let mut c2 = c1;
                // a relative second control is relative to the end point
                let mut c2_offset = None;
                if let Some(Token::Char('a', Cat::Letter)) = peek(e, depth)? {
                    let _ = next(e, depth)?;
                    let w = word(e, 'a');
                    if w == "and" {
                        if matches!(peek(e, depth)?, Some(t) if t.is_char('+')) {
                            let saved = b.relative_base;
                            let base = saved.unwrap_or_else(|| b.point());
                            c2_offset = Some(target(e, b, depth, "controls")?.p.minus(base));
                            b.relative_base = saved;
                        } else {
                            c2 = target(e, b, depth, "controls")?.p;
                        }
                    }
                }
                // the closing ..
                for _ in 0..2 {
                    match next(e, depth)? {
                        Some(t) if t.is_char('.') => {}
                        Some(t) => {
                            e.tex.back(t);
                            break;
                        }
                        None => break,
                    }
                }
                // nodes before the end point go on the curve
                let mids = read_mid_nodes(e, depth, b.point())?;
                b.mid_nodes.extend(mids);
                let r = target(e, b, depth, "controls")?;
                if let Some(o) = c2_offset {
                    c2 = r.p.plus(o);
                }
                curve_to(e, b, c1, c2, r);
                place_mid_nodes(e, b, Some(0.5))?;
            }
            Token::Char('[', _) => {
                let o = e.tex.read_until_char(']', false)?;
                let before = (e.st.color, e.st.draw_color, e.st.fill_color, e.st.text_paint_color());
                let transform = e.st.transform;
                e.process_keys(o, Some("/tikz"))?;
                // in a `{…}` group of the path, a color is only the text's: the path, and the nodes'
                // lines and fills, keep the path's (`{[red] to[V=1] (0,2)}` has its label red)
                if !b.groups.is_empty() && (e.st.color, e.st.draw_color, e.st.fill_color) != (before.0, before.1, before.2) {
                    let text = e.st.text_paint_color();
                    (e.st.color, e.st.draw_color, e.st.fill_color) = (before.0, before.1, before.2);
                    if text != before.3 {
                        e.st.text_color = Some(text);
                    }
                }
                e.st.apply_pending();
                if e.st.transform != transform {
                    let m = e.st.transform.compose(&transform.inverse());
                    b.node_move = Some(m.compose(&b.node_move.unwrap_or_default()));
                }
                insert_path(e);
            }
            Token::Char(':', _) => {
                if let Some(o) = e.read_animation()? {
                    e.process_keys(o, Some("/tikz"))?;
                    e.st.apply_pending();
                }
            }
            Token::Char(_, Cat::Begin) => {
                e.begin_group();
                e.st.current_point_local = false;
                b.groups.push(b.current.clone());
            }
            Token::Char(_, Cat::End) => {
                // `current point is local`: relative points count again from where the group began,
                // while the path goes on from where it ended
                let local = e.st.current_point_local;
                e.end_group();
                // (a `}` of the path always closes one of its `{`: the reader stops at the end of the
                // group the statement is in)
                if let Some(c) = b.groups.pop().filter(|_| local) {
                    b.current = c;
                }
            }
            Token::Char(c, Cat::Letter) => {
                let w = word(e, *c);
                match w.as_str() {
                    "to" | "edge" => {
                        let opts = match peek(e, depth)? {
                            Some(t) if t.is_char('[') => {
                                let _ = next(e, depth)?;
                                e.tex.read_until_char(']', false)?
                            }
                            _ => Vec::new(),
                        };
                        if w == "edge" && opts.is_empty() && peek_words(e, depth, &["from", "parent"])? {
                            // a tree's edge from parent: options and label nodes, drawn after the child
                            let o = match peek(e, depth)? {
                                Some(t) if t.is_char('[') => {
                                    let _ = next(e, depth)?;
                                    e.tex.read_until_char(']', false)?
                                }
                                _ => Vec::new(),
                            };
                            let mut labels = Vec::new();
                            // the nodes and coordinates on the edge
                            while let Some(Token::Char(c @ ('n' | 'c'), Cat::Letter)) = peek(e, depth)? {
                                let _ = next(e, depth)?;
                                let w = word(e, c);
                                if w != "node" && w != "coordinate" {
                                    e.tex.push_tokens(word_tokens(&w));
                                    break;
                                }
                                let (options, name, _, text) = read_node(e, depth, w == "coordinate", b.point())?;
                                labels.extend(tikra_tokens(&format!("{w}[")));
                                labels.extend(options);
                                labels.extend(tikra_tokens("]"));
                                if let Some(n) = name {
                                    labels.extend(tikra_tokens(&format!("({n})")));
                                }
                                if w == "node" {
                                    labels.push(Token::Char('{', Cat::Begin));
                                    labels.extend(text);
                                    labels.push(Token::Char('}', Cat::End));
                                }
                            }
                            e.st.tree_edge = Some((o, labels));
                            e.tree_edge_out = e.st.tree_edge.clone();
                        } else if w == "edge" {
                            // nodes can follow, then the target
                            let mids = read_mid_nodes(e, depth, b.point())?;
                            let r = target(e, b, depth, "edge")?;
                            edge(e, b, opts, r, mids)?;
                        } else {
                            to_options = opts;
                            pending_op = Some("to".into());
                        }
                    }
                    "node" | "coordinate" | "pic" if peek_words(e, depth, &["foreach"])? => node_foreach(e, b, &w)?,
                    // `node also [label=…] (a)`: more options for a node that exists
                    "node" if peek_words(e, depth, &["also"])? => {
                        let (options, name, _, _) = read_node(e, depth, true, b.point())?;
                        let items = e.late_options(name.as_deref().unwrap_or_default(), options)?;
                        b.node_items.extend(items);
                    }
                    "node" | "coordinate" => {
                        let coordinate = w == "coordinate";
                        let (options, name, at, text) = read_node(e, depth, coordinate, relative_base(b))?;
                        if at.is_some() || pending_op.is_none() {
                            let mut point = b.point();
                            let mut place = b.last.unwrap_or(OnPath::Point(point));
                            if let Some(m) = b.node_move {
                                point = m.apply(point);
                                place = place.transformed(&m);
                            }
                            let items = e.node(NodeSpec { options, name, at, text, place, point, coordinate, default_pos: None })?;
                            b.node_items.extend(items);
                            if e.last_node_name.is_none() {
                                // what follows may refer to the node (an edge, `after node path`): it gets a
                                // name, as `\tikz@fig@mustbenamed` gives one
                                if let Some(info) = e.last_node.clone() {
                                    let n = format!("tikra@fig@{}", e.steps);
                                    e.alias_last_node(&n);
                                    e.nodes.insert(n.clone(), info);
                                    e.tex.define_string("tikzlastnode", &n, false);
                                    e.last_node_name = Some(n);
                                }
                            }
                            b.to_start = e.last_node_name.clone().and_then(|n| e.nodes.get(&n).map(|info| Coord { p: info.center(), node: Some(n) }));
                            // `after node path` and the chains' joins continue the path
                            let after = std::mem::take(&mut e.after_node_path);
                            e.tex.push_tokens(after);
                        } else {
                            b.mid_nodes.push(MidNode { options, name, text, coordinate, pic: false });
                        }
                    }
                    "cycle" => {
                        if let Some(p0) = b.subpath_start {
                            if let Some(op) = pending_op.take() {
                                let start = b.segs.len();
                                // `-| cycle`: by the corner, then closed
                                if op == "-|" || op == "|-" {
                                    let a = b.point();
                                    let corner = if op == "-|" { Point::new(p0.x, a.y) } else { Point::new(a.x, p0.y) };
                                    b.segs.push(Segment::LineTo(corner));
                                    b.current = Some(Coord { p: corner, node: None });
                                }
                                let from = b.point();
                                b.segs.push(Segment::Close);
                                snake(e, b, start)?;
                                b.last = Some(OnPath::Segment(from, None, p0));
                                b.current = Some(Coord { p: p0, node: None });
                                place_mid_nodes(e, b, Some(0.5))?;
                            }
                        }
                    }
                    "rectangle" => {
                        // `rectangle node {…} (b)`: the corner comes after the nodes
                        if matches!(peek(e, depth)?, Some(Token::Char('n', Cat::Letter))) {
                            pending_op = Some("rectangle".into());
                            continue;
                        }
                        let r = target(e, b, depth, "rectangle")?;
                        let start = b.segs.len();
                        rectangle_to(e, b, r);
                        snake(e, b, start)?;
                        pending_op = None;
                        place_mid_nodes(e, b, Some(0.5))?;
                    }
                    "circle" | "ellipse" => circle(e, b, depth)?,
                    "arc" => arc(e, b, depth)?,
                    "grid" => grid(e, b, depth)?,
                    "plot" => plot(e, b, depth, pending_op.take().is_some())?,
                    "sin" | "cos" => {
                        let r = target(e, b, depth, &w)?;
                        let a = b.point();
                        let d = r.p.minus(a);
                        // \pgfpathsine and \pgfpathcosine
                        let (c1, c2) = if w == "sin" {
                            (Point::new(0.3260 * d.x, 0.5120 * d.y), Point::new(0.6380 * d.x, d.y))
                        } else {
                            (Point::new(0.3620 * d.x, 0.0), Point::new(0.6740 * d.x, 0.4880 * d.y))
                        };
                        curve_to(e, b, a.plus(c1), a.plus(c2), r);
                    }
                    "parabola" => parabola(e, b, depth)?,
                    "let" => let_operation(e, depth)?,
                    "in" => {}
                    "pic" => {
                        let (options, name, at, content) = read_node(e, depth, false, relative_base(b))?;
                        if at.is_none() && pending_op.is_some() {
                            // between an operation and its target: placed on the segment
                            b.mid_nodes.push(MidNode { options, name, text: content, coordinate: false, pic: true });
                        } else {
                            // after a segment, `pos` places it on it as a node
                            let place = if at.is_none() { Some((b.last.unwrap_or(OnPath::Point(b.point())), None)) } else { None };
                            let at = at.unwrap_or_else(|| b.point());
                            e.pic(super::pics::PicSpec { options, name, at, content, place })?;
                        }
                    }
                    "graph" => e.graph()?,
                    "child" => {
                        if b.segs.is_empty() {
                            // a path that only makes nodes: they are drawn before the children (the
                            // petri library's tokens lie on their place)
                            e.canvas.items.append(&mut b.node_items);
                        }
                        let parent = e.last_node_name.clone();
                        e.children(parent)?;
                    }
                    "decorate" => decorate_operation(e, b, depth)?,
                    "lindenmayer" | "l" => lindenmayer_operation(e, b, depth, &w)?,
                    "foreach" => path_foreach(e, b)?,
                    "svg" => svg_operation(e, b, depth)?,
                    other => {
                        e.warn(format!("unrecognized path fragment: '{other}'"));
                        return skip_rest(e, depth);
                    }
                }
            }
            Token::Cs(name) if &**name == "foreach" => path_foreach(e, b)?,
            Token::Cs(name) if &**name == "pgfextra" => {
                // \pgfextra{code}, or \pgfextra code \endpgfextra
                let code = match e.tex.next_nonspace() {
                    Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
                    Some(t) => {
                        let mut v = vec![t];
                        while let Some(x) = e.tex.get_token() {
                            if x.is_cs("endpgfextra") {
                                break;
                            }
                            v.push(x);
                        }
                        v
                    }
                    None => Vec::new(),
                };
                // a path the code builds on the basic layer (`\pgfsetpathandBB`) becomes the path
                let outer = std::mem::take(&mut e.pgf_path);
                let r = e.run_tokens(code);
                let built = std::mem::replace(&mut e.pgf_path, outer);
                r?;
                if !built.segments.is_empty() {
                    b.segs = built.segments;
                }
            }
            Token::Cs(name) if matches!(e.tex.meaning(&t), Meaning::Native(_)) && matches!(&**name, "node" | "coordinate") => {
                // `\node` inside a path reads as `node`
                e.tex.push_tokens(tikra_tokens(&format!("{name} ")));
            }
            other => {
                let s = tex::show_token(other);
                e.warn(format!("unrecognized path fragment: '{s}'"));
                return skip_rest(e, depth);
            }
        }
    }
}

/// Whether the next words are `words` (read if so, left if not).
/// A word read too far, as the letters it was read as.
fn word_tokens(w: &str) -> Vec<Token> {
    w.chars().map(|c| Token::Char(c, Cat::Letter)).collect()
}

fn peek_words(e: &mut Engine, depth: Option<usize>, words: &[&str]) -> Result<bool, Error> {
    let mut read: Vec<Token> = Vec::new();
    for w in words {
        let first = match next(e, depth)? {
            Some(t) => t,
            None => {
                e.tex.push_tokens(read);
                return Ok(false);
            }
        };
        let ok = matches!(&first, Token::Char(c, Cat::Letter) if w.starts_with(*c));
        if !ok {
            e.tex.back(first);
            e.tex.push_tokens(read);
            return Ok(false);
        }
        let Token::Char(c, _) = first else { unreachable!() };
        let got = word(e, c);
        // given back as they were read: letters (a word that is not the one may be `at`)
        read.extend(word_tokens(&got));
        read.push(Token::Char(' ', Cat::Space));
        if got != *w {
            e.tex.push_tokens(read);
            return Ok(false);
        }
    }
    Ok(true)
}

/// `decorate[options]{subpath}`: the subpath, built with the options, is replaced by its
/// decoration (`\tikz@lib@decoration`).
fn decorate_operation(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    let options = match peek(e, depth)? {
        Some(t) if t.is_char('[') => {
            let _ = next(e, depth)?;
            e.tex.read_until_char(']', false)?
        }
        _ => Vec::new(),
    };
    let body = match next(e, depth)? {
        Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
        other => {
            if let Some(t) = other {
                e.tex.back(t);
            }
            e.warn("a decoration must begin with a brace: decorate ignored");
            return Ok(());
        }
    };
    let start = b.segs.len();
    let existing = b.current.as_ref().map(|c| c.p);
    e.begin_group();
    let r = (|| {
        e.process_keys(options, Some("/tikz"))?;
        e.st.apply_pending();
        let d = e.tex.depth();
        e.tex.push_frame(body);
        let r = operations(e, b, Some(d));
        e.tex.drain_to(d);
        r?;
        let sub: Vec<Segment> = b.segs.split_off(start);
        if sub.is_empty() || !sub.iter().all(valid_segment) {
            b.segs.extend(sub);
            return Ok(());
        }
        if e.counts_for_size() {
            e.canvas.include_path(&sub);
        }
        let decorated = e.decorate_segments(&sub, if start > 0 { existing } else { None })?;
        b.segs.extend(decorated);
        Ok(())
    })();
    e.end_group();
    r
}

/// After an error inside a path: skips to its `;`.
fn skip_rest(e: &mut Engine, depth: Option<usize>) -> Result<(), Error> {
    let mut level = 0usize;
    loop {
        let t = match depth {
            Some(d) => e.tex.get_token_until(d),
            None => e.tex.get_token(),
        };
        let Some(t) = t else { return Ok(()) };
        match t {
            Token::Char(_, Cat::Begin) => level += 1,
            Token::Char(_, Cat::End) => level = level.saturating_sub(1),
            Token::Char(';', _) if level == 0 => return Ok(()),
            _ => {}
        }
    }
}

fn apply_operation(e: &mut Engine, b: &mut Builder, op: Option<String>, to_opts: Vec<Token>, r: Coord) -> Result<(), Error> {
    match op.as_deref() {
        Some("--") => {
            let start = b.segs.len();
            line_to(e, b, r);
            snake(e, b, start)?;
            place_mid_nodes(e, b, Some(0.5))?;
        }
        Some("-|") | Some("|-") => {
            let a = b.point();
            let corner = if op.as_deref() == Some("-|") { Point::new(r.p.x, a.y) } else { Point::new(a.x, r.p.y) };
            let first = b.segs.len();
            let touched = e.facts.touched.take();
            line_to(e, b, Coord { p: corner, node: None });
            e.facts.touched = touched;
            // the legs from and to the nodes' borders
            let start = match b.last {
                Some(OnPath::Segment(p, _, _)) => p,
                _ => a,
            };
            line_to(e, b, r.clone());
            // (a line from a point makes a segment)
            let Some(OnPath::Segment(_, _, end)) = b.last else { unreachable!("a line to the target after the corner") };
            // pos runs along both legs: 0.5, the default, is the corner
            b.last = Some(OnPath::Corner(start, corner, end));
            snake(e, b, first)?;
            place_mid_nodes(e, b, Some(0.5))?;
        }
        Some("to") => to(e, b, to_opts, r)?,
        Some("rectangle") => {
            let start = b.segs.len();
            rectangle_to(e, b, r);
            snake(e, b, start)?;
            place_mid_nodes(e, b, Some(0.5))?;
        }
        _ => {
            b.move_to(r);
            if b.from_node.is_none() {
                b.from_node = e.facts.touched.take();
            }
            place_mid_nodes(e, b, Some(0.5))?;
        }
    }
    Ok(())
}

/// A rectangle from the current point to the corner `r`.
fn rectangle_to(e: &mut Engine, b: &mut Builder, r: Coord) {
    b.from_node = None;
    let a = b.point();
    let t = e.st.transform;
    let inv = t.inverse();
    let (la, lb) = (inv.apply(a), inv.apply(r.p));
    b.segs.push(Segment::MoveTo(a));
    // in the order of `\tikz@rectB`: first to (a.x, b.y)
    for k in [Point::new(la.x, lb.y), lb, Point::new(lb.x, la.y)] {
        b.segs.push(Segment::LineTo(t.apply(k)));
    }
    b.segs.push(Segment::Close);
    b.subpath_start = Some(a);
    b.last = Some(OnPath::Segment(a, None, r.p));
    b.current = Some(Coord { p: r.p, node: None });
}

/// A line to `r`, stopping at the border of the nodes on both ends.
fn line_to(e: &mut Engine, b: &mut Builder, r: Coord) {
    let Some(a) = b.current.clone() else {
        b.move_to(r);
        return;
    };
    let mut pa = a.p;
    let mut pb = r.p;
    if let Some(n) = a.node.as_ref().and_then(|n| e.nodes.get(n)) {
        let target = r.node.as_ref().and_then(|m| e.nodes.get(m)).map_or(r.p, |m| m.center());
        pa = n.border_toward(target);
        // the move to a node waits for the border it leaves from: after a line that ended on
        // the node, a new subpath starts there (a coordinate is a point, its path goes on)
        if n.shape == "coordinate" {
            if let Some(Segment::MoveTo(m)) = b.segs.last_mut() {
                *m = pa;
            }
        } else {
            match b.segs.last_mut() {
                Some(Segment::MoveTo(m)) => *m = pa,
                _ => b.segs.push(Segment::MoveTo(pa)),
            }
        }
    }
    if let Some(n) = r.node.as_ref().and_then(|n| e.nodes.get(n)) {
        let from = a.node.as_ref().and_then(|m| e.nodes.get(m)).map_or(a.p, |m| m.center());
        pb = n.border_toward(from);
    }
    if !matches!(b.segs.last(), Some(Segment::MoveTo(_)) | Some(Segment::LineTo(_)) | Some(Segment::CurveTo(..))) {
        b.segs.push(Segment::MoveTo(pa));
    }
    b.segs.push(Segment::LineTo(pb));
    b.last = Some(OnPath::Segment(pa, None, pb));
    note_link(e, b, &r);
    b.current = Some(r);
}

/// `snake=…`: each straight line the path got from `start` on (a closing one too) becomes that
/// decoration of it, as the snakes library does with each line.
fn snake(e: &mut Engine, b: &mut Builder, start: usize) -> Result<(), Error> {
    let Some(name) = e.st.snake.clone() else { return Ok(()) };
    let added = b.segs.split_off(start.min(b.segs.len()));
    let end = |s: &Segment| match s {
        Segment::MoveTo(p) | Segment::LineTo(p) | Segment::CurveTo(_, _, p) => Some(*p),
        Segment::Close => None,
    };
    let mut first = b.segs.iter().rev().find_map(|s| if let Segment::MoveTo(p) = s { Some(*p) } else { None });
    let mut at = b.segs.last().and_then(end).or(first);
    e.begin_group();
    e.st.snake = None;
    e.st.decoration.name = Some(name);
    let r = (|| {
        for s in added {
            let target = match s {
                Segment::LineTo(p) => p,
                Segment::Close => first.unwrap_or_default(),
                other => {
                    if let Segment::MoveTo(p) = other {
                        first = Some(p);
                    }
                    at = end(&other).or(at);
                    b.segs.push(other);
                    continue;
                }
            };
            let from = at.unwrap_or(target);
            let decorated = e.decorate_segments(&[Segment::MoveTo(from), Segment::LineTo(target)], None)?;
            // the decoration goes on from where the path is, in the same subpath
            b.segs.extend(decorated.into_iter().skip_while(|d| matches!(d, Segment::MoveTo(p) if p.minus(from).norm() < 1e-9)));
            if matches!(s, Segment::Close) {
                b.segs.push(Segment::Close);
            }
            at = Some(target);
        }
        Ok(())
    })();
    e.end_group();
    r
}

fn curve_to(e: &mut Engine, b: &mut Builder, c1: Point, c2: Point, r: Coord) {
    let a = b.current.clone().map_or(r.p, |a| a.p);
    if b.current.is_none() {
        b.segs.push(Segment::MoveTo(a));
    }
    if let Some(n) = b.current.as_ref().and_then(|c| c.node.as_ref()).and_then(|n| e.nodes.get(n)) {
        // from the node's border toward the first control: a new piece if the path came to the
        // node (`(a) to (b) .. controls … .. (b)`)
        let pa = n.border_toward(c1);
        match b.segs.last_mut() {
            Some(Segment::MoveTo(m)) => *m = pa,
            _ => b.segs.push(Segment::MoveTo(pa)),
        }
    }
    let mut pb = r.p;
    if let Some(n) = r.node.as_ref().and_then(|n| e.nodes.get(n)) {
        pb = n.border_toward(c2);
    }
    let start = match b.segs.last() {
        Some(Segment::MoveTo(q) | Segment::LineTo(q) | Segment::CurveTo(_, _, q)) => *q,
        _ => a,
    };
    b.segs.push(Segment::CurveTo(c1, c2, pb));
    b.last = Some(OnPath::Segment(start, Some((c1, c2)), pb));
    note_link(e, b, &r);
    b.current = Some(r);
}

fn polar(angle: f64, d: f64) -> Point {
    let (s, c) = angle.to_radians().sin_cos();
    Point::new(d * c, d * s)
}

/// `\tikz@to@compute@distance@main`: the out and in distances, from the looseness and clamped by
/// `min distance` and `max distance`.
fn to_distances(e: &Engine, a: Point, b: Point) -> (f64, f64) {
    let t = &e.st.to;
    let d = 0.3915 * b.minus(a).norm();
    let clamp = |x: f64, lo: f64, hi: f64| {
        let x = if x < lo { lo } else { x };
        if x > hi {
            hi
        } else {
            x
        }
    };
    (clamp(t.out_looseness * d, t.out_min, t.out_max), clamp(t.in_looseness * d, t.in_min, t.in_max))
}

/// A curve of `to` as the topaths library computes it, from `from` to `r`: an explicit start (the
/// relative curves start on the border of the start node), the controls and an explicit end.
fn to_curve(e: &mut Engine, from: &Coord, r: &Coord) -> Result<(Option<Point>, Point, Point, Option<Point>), Error> {
    let node = |e: &Engine, c: &Coord| c.node.as_ref().and_then(|n| e.nodes.get(n)).cloned();
    let (na, nb) = (node(e, from), node(e, r));
    let (out, inn) = (e.st.to.out, e.st.to.inn);
    if !e.st.to.relative {
        // the ends are the nodes' borders at the angles out and in
        let a = na.as_ref().and_then(|n| n.anchor(&out.to_string())).unwrap_or(from.p);
        let b = nb.as_ref().and_then(|n| n.anchor(&inn.to_string())).unwrap_or(r.p);
        // the angles and the distance are in the picture's coordinate system: under `xscale` or a
        // slant, the controls go where the transformation takes them
        let t = e.st.transform;
        let inverse = t.inverse();
        let (ko, ki) = to_distances(e, inverse.apply(a), inverse.apply(b));
        // `(p)`, or `+(d)` from the end it belongs to
        let control = |e: &mut Engine, c: Option<Vec<Token>>, base: Point| -> Result<Option<Point>, Error> {
            let Some(v) = c else { return Ok(None) };
            let mut v = crate::keys::trim(&v).to_vec();
            let plus = v.iter().take_while(|t| t.is_char('+')).count();
            v.drain(..plus);
            let v = if v.first().is_some_and(|t| t.is_char('(')) && v.last().is_some_and(|t| t.is_char(')')) { v[1..v.len() - 1].to_vec() } else { v };
            let text = e.math_string(v)?;
            let p = e.coordinate(&text)?.p;
            Ok(Some(if plus > 0 { base.plus(p.minus(e.st.transform.apply(Point::default()))) } else { p }))
        };
        let c1 = control(e, e.st.to.out_control.clone(), from.p)?.unwrap_or_else(|| a.plus(t.linear(polar(out, ko))));
        let c2 = control(e, e.st.to.in_control.clone(), r.p)?.unwrap_or_else(|| b.plus(t.linear(polar(inn, ki))));
        return Ok((None, c1, c2, None));
    }
    // relative: the angles are measured from the line between the ends
    let (a0, b0) = (na.as_ref().map_or(from.p, |n| n.center()), nb.as_ref().map_or(r.p, |n| n.center()));
    let rotate = |p: Point, about: Point, angle: f64| {
        let (s, c) = angle.to_radians().sin_cos();
        let d = p.minus(about);
        about.plus(Point::new(c * d.x - s * d.y, s * d.x + c * d.y))
    };
    let first = na.as_ref().map_or(a0, |n| n.border_toward(rotate(b0, a0, out)));
    let second = nb.as_ref().map_or(b0, |n| n.border_toward(rotate(a0, b0, 180.0 + inn)));
    // the controls in the picture's coordinate system, as for absolute angles
    let t = e.st.transform;
    let (fl, sl) = (t.inverse().apply(first), t.inverse().apply(second));
    let (ko, ki) = to_distances(e, fl, sl);
    let d = sl.minus(fl);
    let u = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(1.0, 0.0) };
    let turn = |v: Point| Point::new(u.x * v.x - u.y * v.y, u.y * v.x + u.x * v.y);
    let c1 = t.apply(fl.plus(turn(polar(out, ko))));
    let c2 = t.apply(sl.plus(turn(polar(inn, ki))));
    Ok((na.map(|_| first), c1, c2, Some(second)))
}

/// The end of a `to` as `\tikztostart` and `\tikztotarget` name it: a node, or a point on the canvas.
fn to_name(c: &Coord) -> String {
    match &c.node {
        Some(n) => n.clone(),
        None => format!("tikra cs:{}pt,{}pt", c.p.x / crate::PT, c.p.y / crate::PT),
    }
}

/// `\tikztonodes`: the nodes given between `to` and its target, labels at the middle by default.
fn to_nodes(mids: Vec<MidNode>) -> Vec<Token> {
    let mut v = Vec::new();
    for m in mids {
        v.extend(tikra_tokens(if m.coordinate { "coordinate[pos=0.5," } else { "node[pos=0.5," }));
        v.extend(m.options);
        v.extend(tikra_tokens("]"));
        if let Some(n) = m.name {
            v.extend(tikra_tokens(&format!("({n})")));
        }
        if !m.coordinate {
            v.push(Token::Char('{', Cat::Begin));
            v.extend(m.text);
            v.push(Token::Char('}', Cat::End));
        }
    }
    v
}

/// The path of a `to` (or an `edge`) with its options already set, from the current point to `r`.
fn to_path(e: &mut Engine, b: &mut Builder, r: Coord) -> Result<(), Error> {
    let from = b.current.clone().unwrap_or(Coord { p: b.point(), node: None });
    let r = if e.st.to.is_loop {
        // [looseness=8,min distance=5mm] after the options, and back to the start
        let t = &mut e.st.to;
        (t.out_looseness, t.in_looseness) = (8.0, 8.0);
        (t.out_min, t.in_min) = (0.5, 0.5);
        (t.out_control, t.in_control) = (None, None);
        from.clone()
    } else {
        r
    };
    if let Some(path) = e.st.to.path.clone() {
        // the user's to path, run as path operations with the three macros set
        e.tex.define_string("tikztostart", &to_name(&from), false);
        e.tex.define_string("tikztotarget", &to_name(&r), false);
        e.tex.set_meaning(&Token::cs("tikztonodes"), super::macro_tokens(to_nodes(std::mem::take(&mut b.mid_nodes))), false);
        let mut v = path;
        v.push(Token::other(';'));
        e.tex.push_tokens(v);
        return operations(e, b, None);
    }
    if !e.st.to.curve {
        let start = b.segs.len();
        line_to(e, b, r);
        return snake(e, b, start);
    }
    e.process_keys(tikra_tokens("every curve to/.try"), Some("/tikz"))?;
    let (start, c1, c2, end) = to_curve(e, &from, &r)?;
    if let Some(s) = start {
        match b.segs.last_mut() {
            Some(Segment::MoveTo(m)) => *m = s,
            _ => b.segs.push(Segment::MoveTo(s)),
        }
        b.current = Some(Coord { p: s, node: None });
        b.subpath_start.get_or_insert(s);
    }
    match end {
        Some(p) => {
            let a = b.point();
            if b.current.is_none() {
                b.segs.push(Segment::MoveTo(a));
            }
            b.segs.push(Segment::CurveTo(c1, c2, p));
            b.last = Some(OnPath::Segment(a, Some((c1, c2)), p));
            note_link(e, b, &r);
            b.current = Some(Coord { p, node: r.node.clone() });
        }
        None => curve_to(e, b, c1, c2, r),
    }
    Ok(())
}

/// `execute at begin to` and `execute at end to`: the code they collect, run in the `to`'s group.
fn run_hook(e: &mut Engine, name: &str) -> Result<(), Error> {
    match e.tex.meaning_of(name) {
        Meaning::Macro(m) if !m.body.is_empty() => {
            let code = tex::substitute(&m.body, &[]);
            e.run_tokens(code)
        }
        _ => Ok(()),
    }
}

fn to(e: &mut Engine, b: &mut Builder, opts: Vec<Token>, r: Coord) -> Result<(), Error> {
    if let Some(s) = b.to_start.take() {
        b.move_to(s);
    }
    // what the target set for a `+(…)` after it: a user's to path, whatever its points, does not change it
    let relative_base = b.relative_base;
    e.begin_group();
    e.st.quotes_on_path = true;
    e.st.quoted.clear();
    let res = (|| {
        run_hook(e, "tikz@atbegin@to")?;
        e.process_keys(tikra_tokens("every to/.try"), Some("/tikz"))?;
        e.process_keys(opts, Some("/tikz"))?;
        for (text, options) in std::mem::take(&mut e.st.quoted) {
            b.mid_nodes.push(MidNode { options, name: None, text, coordinate: false, pic: false });
        }
        to_path(e, b, r)?;
        place_mid_nodes(e, b, Some(0.5))?;
        run_hook(e, "tikz@atend@to")
    })();
    e.end_group();
    b.relative_base = relative_base;
    res
}

/// `edge[options] node{…} (target)`: a separate path, drawn after this one; the current point stays.
fn edge(e: &mut Engine, b: &mut Builder, opts: Vec<Token>, r: Coord, mids: Vec<MidNode>) -> Result<(), Error> {
    let saved_current = b.current.clone();
    let mut sub = Builder {
        segs: Vec::new(),
        current: None,
        subpath_start: None,
        last: None,
        node_items: Vec::new(),
        mid_nodes: mids,
        edges: Vec::new(),
        relative_base: None,
        origin: e.st.transform.apply(Point::default()),
        close_after: false,
        to_start: None,
        node_move: None,
        groups: Vec::new(),
        links: Vec::new(),
        from_node: None,
        fresh_link: false,
    };
    if let Some(c) = b.to_start.clone().or_else(|| saved_current.clone()) {
        sub.move_to(c);
    }
    e.begin_group();
    e.st.quotes_on_path = true;
    e.st.quoted.clear();
    let mut opts = opts;
    let res = (|| {
        run_hook(e, "tikz@atbegin@to")?;
        e.process_keys(tikra_tokens("every edge/.try"), Some("/tikz"))?;
        e.process_keys(opts.clone(), Some("/tikz"))?;
        for (text, options) in std::mem::take(&mut e.st.quoted) {
            sub.mid_nodes.push(MidNode { options, name: None, text, coordinate: false, pic: false });
        }
        // options that start the to path are the edge path's own, as TikZ runs it in `\path[…]`
        if let Some(leading) = e.st.to.path.as_deref().and_then(leading_options) {
            opts.push(Token::other(','));
            opts.extend(leading);
        }
        to_path(e, &mut sub, r.clone())?;
        place_mid_nodes(e, &mut sub, Some(0.5))
    })();
    e.end_group();
    res?;
    b.edges.push((sub.segs, opts, sub.node_items, sub.links));
    b.current = saved_current;
    Ok(())
}

/// `[options]` at the start of a path, without the brackets.
fn leading_options(path: &[Token]) -> Option<Vec<Token>> {
    let path = crate::keys::trim(path);
    if !path.first()?.is_char('[') {
        return None;
    }
    let mut depth = 0usize;
    for (i, t) in path.iter().enumerate().skip(1) {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            t if depth == 0 && t.is_char(']') => return Some(path[1..i].to_vec()),
            _ => {}
        }
    }
    None
}

/// Nodes between `edge` and its target.
fn read_mid_nodes(e: &mut Engine, depth: Option<usize>, base: Point) -> Result<Vec<MidNode>, Error> {
    let mut v = Vec::new();
    while let Some(Token::Char('n' | 'c', Cat::Letter)) = peek(e, depth)? {
        let Some(Token::Char(c, _)) = next(e, depth)? else { break };
        let w = word(e, c);
        if w != "node" && w != "coordinate" {
            e.tex.push_tokens(word_tokens(&w));
            break;
        }
        let coordinate = w == "coordinate";
        let (options, name, _, text) = read_node(e, depth, coordinate, base)?;
        v.push(MidNode { options, name, text, coordinate, pic: false });
    }
    Ok(v)
}

fn place_mid_nodes(e: &mut Engine, b: &mut Builder, default_pos: Option<f64>) -> Result<(), Error> {
    // the nodes on a line just made from node to node are its labels
    let link = std::mem::take(&mut b.fresh_link).then(|| b.links.len() - 1);
    let first = e.facts.nodes.len();
    for m in std::mem::take(&mut b.mid_nodes) {
        let place = b.last.unwrap_or(OnPath::Point(b.point()));
        if m.pic {
            e.pic(super::pics::PicSpec { options: m.options, name: m.name, at: b.point(), content: m.text, place: Some((place, default_pos)) })?;
            continue;
        }
        let items = e.node(NodeSpec { options: m.options, name: m.name, at: None, text: m.text, place, point: b.point(), coordinate: m.coordinate, default_pos })?;
        b.node_items.extend(items);
    }
    if let Some(l) = link {
        for i in first..e.facts.nodes.len() {
            e.facts.nodes[i].on_link = true;
            b.links[l].labels.push(i);
        }
    }
    Ok(())
}

/// `node[options] (name) at (x,y) {text}`, the word already read; `base` is the point an
/// `at +(x,y)` is relative to.
#[allow(clippy::type_complexity)]
fn read_node(e: &mut Engine, depth: Option<usize>, coordinate: bool, base: Point) -> Result<(Vec<Token>, Option<String>, Option<Point>, Vec<Token>), Error> {
    let mut options = Vec::new();
    let mut name = None;
    let mut at = None;
    loop {
        match next(e, depth)? {
            // options after a coordinate's name are the path's (`\coordinate (root) [grow cyclic] child …`)
            Some(t) if t.is_char('[') && coordinate && name.is_some() => {
                e.tex.back(t);
                break;
            }
            Some(t) if t.is_char('[') => {
                if !options.is_empty() {
                    options.push(Token::other(','));
                }
                options.extend(e.tex.read_until_char(']', false)?);
                // with its contents in the options, the node ends there
                if crate::keys::split_list(&options).iter().any(|i| i.key == "node contents" || i.key == "pic type") {
                    break;
                }
            }
            // a second point after a coordinate's name is the path's next (`coordinate (a) (1,0)`)
            Some(t) if t.is_char('(') && coordinate && name.is_some() => {
                e.tex.back(t);
                break;
            }
            // a node's name; a second one replaces the first (`node (a) (b) {}` is named b)
            Some(t) if t.is_char('(') => {
                let n = read_coordinate_tokens(e)?;
                name = Some(e.tex.expand_to_string(n)?.trim().to_string());
            }
            Some(t) if t.is_char(':') => {
                if let Some(o) = e.read_animation()? {
                    if !options.is_empty() {
                        options.push(Token::other(','));
                    }
                    options.extend(o);
                }
            }
            Some(Token::Char('a', Cat::Letter)) => {
                // `at (x,y)`
                match e.tex.get_token() {
                    Some(Token::Char('t', Cat::Letter)) => match next(e, depth)? {
                        Some(t) if t.is_char('(') => at = Some(coordinate_point(e)?),
                        // `at +(x,y)` and `at ++(x,y)`: from the path's last point, which stays
                        Some(t) if t.is_char('+') => {
                            let mut t = next(e, depth)?;
                            if matches!(&t, Some(t) if t.is_char('+')) {
                                t = next(e, depth)?;
                            }
                            if !matches!(t, Some(t) if t.is_char('(')) {
                                return Err(Error::Figure("at without a point".into()));
                            }
                            let r = coordinate_point(e)?;
                            at = Some(base.plus(r.minus(e.st.transform.apply(Point::default()))));
                        }
                        _ => return Err(Error::Figure("at without a point".into())),
                    },
                    other => {
                        if let Some(o) = other {
                            e.tex.back(o);
                        }
                        e.tex.back(Token::Char('a', Cat::Letter));
                        break;
                    }
                }
            }
            Some(Token::Char(_, Cat::Begin)) if !coordinate => {
                let text = e.tex.read_group_body()?;
                return Ok((options, name, at, text));
            }
            // `\node\bgroup text\egroup` (as `execute at begin cell` writes it)
            Some(t) if t.is_cs("bgroup") && !coordinate => {
                let mut text = Vec::new();
                let mut depth = 0usize;
                loop {
                    match e.tex.get_token() {
                        None => break,
                        Some(t) if t.is_cs("egroup") && depth == 0 => break,
                        Some(t) => {
                            if t.is_cs("bgroup") || matches!(t, Token::Char(_, Cat::Begin)) {
                                depth += 1;
                            } else if t.is_cs("egroup") || matches!(t, Token::Char(_, Cat::End)) {
                                depth = depth.saturating_sub(1);
                            }
                            text.push(t);
                        }
                    }
                }
                return Ok((options, name, at, text));
            }
            Some(t) => {
                e.tex.back(t);
                break;
            }
            None => break,
        }
    }
    // `node contents=…` (`pic type=…` for a pic) stands for the text, which is then not given
    if let Some(v) = crate::keys::split_list(&options).into_iter().find(|i| i.key == "node contents" || i.key == "pic type").and_then(|i| i.value) {
        return Ok((options, name, at, tex::strip_outer_braces(v)));
    }
    if !coordinate {
        e.warn("node without text: '{…}' missing");
    }
    Ok((options, name, at, Vec::new()))
}

fn coordinate_point(e: &mut Engine) -> Result<Point, Error> {
    Ok(coordinate(e)?.p)
}

/// `circle (r)`, `circle (rx and ry)`, `circle[radius=…]`, `ellipse[x radius=…, y radius=…]`.
fn circle(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    e.begin_group();
    let res = (|| -> Result<(f64, f64, Option<Point>, crate::scene::Transform), Error> {
        // `circle[options]`, or the old `circle (radius)`: a point after the options is the next one
        let mut options = false;
        e.st.node.at = None;
        let before = e.st.pending;
        if let Some(t) = peek(e, depth)? {
            if t.is_char('[') {
                let _ = next(e, depth)?;
                let o = e.tex.read_until_char(']', false)?;
                e.process_keys(o, Some("/tikz"))?;
                options = true;
            }
        }
        let (mut rx, mut ry) = radii(e)?;
        if let Some(t) = peek(e, depth)?.filter(|_| !options) {
            if t.is_char('(') {
                let _ = next(e, depth)?;
                let tokens = read_coordinate_tokens(e)?;
                let text = e.math_string(tokens)?;
                if let Some((a, bb)) = text.split_once(" and ") {
                    rx = e.radius_length(a)?;
                    ry = e.radius_length(bb)?;
                } else {
                    rx = e.radius_length(&text)?;
                    ry = rx;
                }
            }
        }
        Ok((rx, ry, e.st.node.at.take(), before.inverse().compose(&e.st.pending)))
    })();
    e.end_group();
    let (rx, ry, at, turn) = res?;
    // `at=(c)` centers it elsewhere; the options' transformations (`rotate=30`) turn it about its
    // center
    let t = e.st.transform.compose(&turn);
    let center = at.unwrap_or_else(|| b.point());
    let mut v = arc_segments(center, rx, ry, 0.0, 360.0, &t);
    v.insert(0, Segment::MoveTo(center.plus(t.linear(Point::new(rx, 0.0)))));
    v.push(Segment::Close);
    b.segs.extend(v);
    b.subpath_start = Some(center);
    Ok(())
}

/// `radius`, `x radius`, `y radius` from the keys.
fn radii(e: &mut Engine) -> Result<(f64, f64), Error> {
    let get = |e: &mut Engine, k: &str| -> Result<Option<f64>, Error> {
        match keys::value(&e.tex, k) {
            Some(v) if !keys::trim(&v).is_empty() => {
                let s = e.math_string(v)?;
                Ok(Some(e.radius_length(&s)?))
            }
            _ => Ok(None),
        }
    };
    // (`radius` sets both)
    let rx = get(e, "/tikz/x radius")?.unwrap_or(0.0);
    let ry = get(e, "/tikz/y radius")?.unwrap_or(0.0);
    Ok((rx, ry))
}

impl Engine {
    /// The connections of a path, kept for the description when the path is drawn, with its tips.
    fn note_links(&mut self, links: Vec<Link>) {
        if links.is_empty() {
            return;
        }
        if self.facts.mute > 0 || !self.st.mode.draw {
            // not drawn: its labels are nodes like any other
            for l in links {
                for i in l.labels {
                    self.facts.nodes[i].on_link = false;
                }
            }
            return;
        }
        let tip = |a: &Option<String>| a.as_ref().is_some_and(|s| !s.trim().is_empty());
        let arrow = match (tip(&self.st.start_arrow), tip(&self.st.end_arrow)) {
            (false, false) => Arrow::None,
            (false, true) => Arrow::Forward,
            (true, false) => Arrow::Backward,
            (true, true) => Arrow::Both,
        };
        let source = self.source;
        self.facts.links.extend(links.into_iter().map(|l| Link { arrow, source, ..l }));
    }

    /// A radius: with a unit in cm, without one in the x unit (as TikZ scales circle radii).
    fn radius_length(&mut self, text: &str) -> Result<f64, Error> {
        let (v, unit) = crate::expr::eval_length(crate::keys::strip_braces(text.trim()), &self.vars())?;
        Ok(if unit { v } else { v * self.st.x.norm() })
    }
}

/// An elliptical arc from `a0` to `a1` degrees as cubic curves, cut the way `\pgfpatharc` cuts it:
/// 90° pieces while more than 115° are left, 60° pieces while more than 90° are, then the rest.
pub fn arc_segments(c: Point, rx: f64, ry: f64, a0: f64, a1: f64, t: &crate::scene::Transform) -> Vec<Segment> {
    let mut a = a0;
    let mut b = a1;
    if (b - a).abs() > 720.0 {
        // more than two turns draw the same ellipse: keep one turn and the rest
        b = a + (b - a).signum() * (360.0 + (b - a).abs() % 360.0);
    }
    let piece = |from: f64, to: f64| -> Segment {
        let delta = (to - from).abs();
        let k = if (delta - 90.0).abs() < 1e-12 { 0.55228475 } else { 4.0 / 3.0 * (delta.to_radians() / 4.0).tan() };
        let side = if to > from { 90.0 } else { -90.0 };
        let polar = |angle: f64, sx: f64, sy: f64| {
            let (s, co) = angle.to_radians().sin_cos();
            Point::new(sx * co, sy * s)
        };
        let p0 = polar(from, rx, ry);
        let p3 = polar(to, rx, ry);
        let c1 = p0.plus(polar(from + side, k * rx, k * ry));
        let c2 = p3.plus(polar(to - side, k * rx, k * ry));
        Segment::CurveTo(c.plus(t.linear(c1)), c.plus(t.linear(c2)), c.plus(t.linear(p3)))
    };
    let mut v = Vec::new();
    while (a - b).abs() > 90.0 && v.len() < 100 {
        let step = if (a - b).abs() > 115.0 { 90.0 } else { 60.0 };
        let next = if b > a { a + step } else { a - step };
        v.push(piece(a, next));
        a = next;
    }
    v.push(piece(a, b));
    v
}

/// `svg[options] {M 0 0 L 20 20 h 10 a 10 10 0 0 0 -20 0}` (or `svg "…"`): SVG's path data, its unit the pt and its
/// y axis up (pgf's `\pgfpathsvg`), in the coordinate system the options make; relative commands
/// start from the path's current point.
fn svg_operation(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    e.begin_group();
    let r = (|| -> Result<(), Error> {
        if let Some(t) = peek(e, depth)? {
            if t.is_char('[') {
                let _ = next(e, depth)?;
                let o = e.tex.read_until_char(']', false)?;
                e.process_keys(o, Some("/tikz"))?;
                e.st.apply_pending();
            }
        }
        let data = match next(e, depth)? {
            Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
            // or in quotes, as the svg.path library also takes it
            Some(t) if t.is_char('"') => e.tex.read_until_char('"', false)?,
            _ => return Err(Error::Figure("svg without a {path} or \"path\"".into())),
        };
        let text = e.tex.expand_to_string(data)?;
        svg_data(e, b, &text);
        Ok(())
    })();
    e.end_group();
    r
}

/// The path data of SVG (`M 0 0 l 20 0 …`, in pt) onto the path being built.
fn svg_data(e: &Engine, b: &mut Builder, text: &str) {
    let t = e.st.transform;
    // local pt to the canvas
    let to = |p: Point| t.apply(p.scale(crate::PT));
    let from = |c: Point| t.inverse().apply(c).scale(1.0 / crate::PT);
    let mut last = from(b.point());
    let mut start = last;
    let mut prev_control: Option<Point> = None;
    let mut prev_quad: Option<Point> = None;
    // the numbers and command letters of the data
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        let starts_number = c == '-' && !cur.is_empty() && !cur.ends_with(['e', 'E']);
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' || c.is_whitespace() || c == ',' || starts_number {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
            if c.is_ascii_alphabetic() {
                tokens.push(c.to_string());
            } else if c == '-' {
                cur.push(c);
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut i = 0;
    let mut cmd = 'M';
    let num = |i: &mut usize| -> Option<f64> {
        let v = tokens.get(*i)?.parse().ok()?;
        *i += 1;
        Some(v)
    };
    while i < tokens.len() {
        if let Some(c) = tokens[i].chars().next().filter(|c| c.is_ascii_alphabetic()) {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                b.segs.push(Segment::Close);
                last = start;
                b.current = Some(Coord { p: to(last), node: None });
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { last } else { Point::default() };
        let pt = |i: &mut usize| -> Option<Point> { Some(Point::new(num(i)?, num(i)?).plus(base)) };
        // (each command reads its numbers, or ends the data)
        match cmd.to_ascii_uppercase() {
            'M' => {
                let Some(p) = pt(&mut i) else { break };
                b.move_to(Coord { p: to(p), node: None });
                (last, start) = (p, p);
                // more pairs after a moveto are lines
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let Some(p) = pt(&mut i) else { break };
                b.segs.push(Segment::LineTo(to(p)));
                last = p;
            }
            'H' => {
                let Some(x) = num(&mut i) else { break };
                last = Point::new(if rel { last.x + x } else { x }, last.y);
                b.segs.push(Segment::LineTo(to(last)));
            }
            'V' => {
                let Some(y) = num(&mut i) else { break };
                last = Point::new(last.x, if rel { last.y + y } else { y });
                b.segs.push(Segment::LineTo(to(last)));
            }
            'C' | 'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&'S') {
                    prev_control.map_or(last, |c| last.scale(2.0).minus(c))
                } else {
                    let Some(c) = pt(&mut i) else { break };
                    c
                };
                let (Some(c2), Some(p)) = (pt(&mut i), pt(&mut i)) else { break };
                b.segs.push(Segment::CurveTo(to(c1), to(c2), to(p)));
                prev_control = Some(c2);
                last = p;
            }
            'Q' | 'T' => {
                let q = if cmd.eq_ignore_ascii_case(&'T') {
                    prev_quad.map_or(last, |c| last.scale(2.0).minus(c))
                } else {
                    let Some(c) = pt(&mut i) else { break };
                    c
                };
                let Some(p) = pt(&mut i) else { break };
                b.segs.push(Segment::CurveTo(to(last.lerp(q, 2.0 / 3.0)), to(p.lerp(q, 2.0 / 3.0)), to(p)));
                prev_quad = Some(q);
                last = p;
            }
            'A' => {
                let (Some(rx), Some(ry), Some(phi), Some(large), Some(sweep)) = (num(&mut i), num(&mut i), num(&mut i), num(&mut i), num(&mut i)) else { break };
                let Some(p) = pt(&mut i) else { break };
                // with y up, pgf turns the other way for the same sweep flag
                svg_arc(b, last, p, rx.abs(), ry.abs(), phi, large != 0.0, sweep == 0.0, &t);
                last = p;
            }
            _ => break,
        }
        if !matches!(cmd.to_ascii_uppercase(), 'C' | 'S') {
            prev_control = None;
        }
        if !matches!(cmd.to_ascii_uppercase(), 'Q' | 'T') {
            prev_quad = None;
        }
        b.current = Some(Coord { p: to(last), node: None });
        b.last = Some(OnPath::Point(to(last)));
    }
}

/// The basic layer's `\pgfpathsvg{data}`: the segments of SVG path data (in pt, transformed as the
/// basic layer's points are), from `from` (the canvas point the path is at, if any).
pub fn svg_path(e: &Engine, text: &str, from: Option<Point>) -> Vec<Segment> {
    let mut b = Builder {
        segs: Vec::new(),
        current: from.map(|p| Coord { p, node: None }),
        subpath_start: from,
        last: None,
        node_items: Vec::new(),
        mid_nodes: Vec::new(),
        edges: Vec::new(),
        relative_base: None,
        origin: e.st.transform.apply(Point::default()),
        close_after: false,
        to_start: None,
        node_move: None,
        groups: Vec::new(),
        links: Vec::new(),
        from_node: None,
        fresh_link: false,
    };
    svg_data(e, &mut b, text);
    b.segs
}

/// An SVG elliptical arc from `a` to `p` (local pt), as curves on the canvas (SVG's appendix F.6.5).
#[allow(clippy::too_many_arguments)]
fn svg_arc(b: &mut Builder, a: Point, p: Point, rx: f64, ry: f64, phi: f64, large: bool, sweep: bool, t: &crate::scene::Transform) {
    b.segs.extend(svg_arc_segments(a, p, rx, ry, phi, large, sweep, t));
}

/// The curves of an SVG elliptical arc from `a` to `p` (local pt) on the canvas; `sweep` turns it
/// counterclockwise. A radius of zero or no distance makes it a line.
#[allow(clippy::too_many_arguments)]
pub fn svg_arc_segments(a: Point, p: Point, mut rx: f64, mut ry: f64, phi: f64, large: bool, sweep: bool, t: &crate::scene::Transform) -> Vec<Segment> {
    if rx < 1e-12 || ry < 1e-12 || a.minus(p).norm() < 1e-12 {
        return vec![Segment::LineTo(t.apply(p.scale(crate::PT)))];
    }
    let (s, c) = phi.to_radians().sin_cos();
    let d = a.minus(p).scale(0.5);
    let x1 = Point::new(c * d.x + s * d.y, -s * d.x + c * d.y);
    let lambda = (x1.x * x1.x) / (rx * rx) + (x1.y * x1.y) / (ry * ry);
    if lambda > 1.0 {
        (rx, ry) = (rx * lambda.sqrt(), ry * lambda.sqrt());
    }
    let num = (rx * rx * ry * ry - rx * rx * x1.y * x1.y - ry * ry * x1.x * x1.x).max(0.0);
    let den = rx * rx * x1.y * x1.y + ry * ry * x1.x * x1.x;
    let mut k = (num / den).sqrt();
    if large == sweep {
        k = -k;
    }
    let cp = Point::new(k * rx * x1.y / ry, -k * ry * x1.x / rx);
    let m = a.lerp(p, 0.5);
    let center = Point::new(c * cp.x - s * cp.y + m.x, s * cp.x + c * cp.y + m.y);
    let angle = |u: Point| u.y.atan2(u.x).to_degrees();
    let u = Point::new((x1.x - cp.x) / rx, (x1.y - cp.y) / ry);
    let v = Point::new((-x1.x - cp.x) / rx, (-x1.y - cp.y) / ry);
    let theta = angle(u);
    let mut delta = angle(v) - theta;
    if sweep && delta < 0.0 {
        delta += 360.0;
    } else if !sweep && delta > 0.0 {
        delta -= 360.0;
    }
    let local = arc_segments(Point::default(), rx, ry, theta, theta + delta, &crate::scene::Transform::default());
    let m = t
        .compose(&crate::scene::Transform::scaling(crate::PT, crate::PT))
        .compose(&crate::scene::Transform::translation(center.x, center.y))
        .compose(&crate::scene::Transform::rotation(phi));
    crate::canvas::transform_segments(&local, &m)
}

/// `arc (start:end:radius)`, `arc (start:end:rx and ry)`, `arc[start angle=…, end angle=…, radius=…]`.
fn arc(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    e.begin_group();
    let res = (|| -> Result<(f64, f64, f64, f64), Error> {
        let (mut a0, mut a1, mut rx, mut ry) = (0.0, 90.0, 0.0, 0.0);
        match peek(e, depth)? {
            Some(t) if t.is_char('(') => {
                let _ = next(e, depth)?;
                let tokens = read_coordinate_tokens(e)?;
                let text = e.math_string(tokens)?;
                let p: Vec<&str> = text.split(':').collect();
                if p.len() == 3 {
                    a0 = e.number(p[0])?;
                    a1 = e.number(p[1])?;
                    if let Some((x, y)) = p[2].split_once(" and ") {
                        rx = e.radius_length(x)?;
                        ry = e.radius_length(y)?;
                    } else {
                        rx = e.radius_length(p[2])?;
                        ry = rx;
                    }
                }
            }
            Some(t) if t.is_char('[') => {
                let _ = next(e, depth)?;
                let o = e.tex.read_until_char(']', false)?;
                e.process_keys(o, Some("/tikz"))?;
                let (x, y) = radii(e)?;
                rx = x;
                ry = y;
                let get = |e: &mut Engine, k: &str| -> Result<Option<f64>, Error> {
                    match keys::value(&e.tex, k) {
                        Some(v) if !keys::trim(&v).is_empty() => Ok(Some(e.number_of(v)?)),
                        _ => Ok(None),
                    }
                };
                let s = get(e, "/tikz/start angle")?;
                let en = get(e, "/tikz/end angle")?;
                let d = get(e, "/tikz/delta angle")?;
                match (s, en, d) {
                    (Some(s), Some(en), _) => {
                        a0 = s;
                        a1 = en;
                    }
                    (Some(s), None, Some(d)) => {
                        a0 = s;
                        a1 = s + d;
                    }
                    (None, Some(en), Some(d)) => {
                        a0 = en - d;
                        a1 = en;
                    }
                    (Some(s), None, None) => a0 = s,
                    _ => {}
                }
            }
            _ => {
                let (x, y) = radii(e)?;
                rx = x;
                ry = y;
            }
        }
        Ok((a0, a1, rx, ry))
    })();
    e.end_group();
    let (a0, a1, rx, ry) = res?;
    let t = e.st.transform;
    let p = b.point();
    if b.current.is_none() {
        b.segs.push(Segment::MoveTo(p));
    }
    let (s0, c0) = a0.to_radians().sin_cos();
    let center = p.minus(t.linear(Point::new(rx * c0, ry * s0)));
    let segs = arc_segments(center, rx, ry, a0, a1, &t);
    let (s1, c1) = a1.to_radians().sin_cos();
    let end = center.plus(t.linear(Point::new(rx * c1, ry * s1)));
    if !segs.is_empty() {
        b.last = Some(OnPath::Arc { center, x: t.linear(Point::new(rx, 0.0)), y: t.linear(Point::new(0.0, ry)), from: a0, to: a1 });
    }
    b.segs.extend(segs);
    b.current = Some(Coord { p: end, node: None });
    // a `+(…)` after it is from the arc's end
    b.relative_base = None;
    Ok(())
}

/// Multiples of `step` between `start` and `end`. None if the step is unusable.
fn ticks(start: f64, end: f64, step: f64) -> Option<Vec<f64>> {
    // a step of zero: no lines that way (`ystep=0`)
    if step == 0.0 {
        return Some(Vec::new());
    }
    if !(step > 0.0 && step.is_finite() && start.is_finite() && end.is_finite()) {
        return None;
    }
    let first = (start / step - 1e-9).ceil();
    let count = ((end + 1e-9) / step).floor() - first + 1.0;
    if count > MAX_GRID_LINES as f64 {
        return None;
    }
    Some((0..count.max(0.0) as usize).map(|k| (first + k as f64) * step).collect())
}

fn grid(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    e.begin_group();
    let res = (|| -> Result<(f64, f64), Error> {
        if let Some(t) = peek(e, depth)? {
            if t.is_char('[') {
                let _ = next(e, depth)?;
                let o = e.tex.read_until_char(']', false)?;
                e.process_keys(o, Some("/tikz"))?;
            }
        }
        Ok(e.st.grid_step)
    })();
    e.end_group();
    let step = res?;
    if step == (0.0, 0.0) {
        e.warn("grid: step 0 × 0 draws no lines");
    }
    let r = target(e, b, depth, "grid")?;
    let t = e.st.transform;
    let inv = t.inverse();
    let (la, lb) = (inv.apply(b.point()), inv.apply(r.p));
    let (x0, x1) = (la.x.min(lb.x), la.x.max(lb.x));
    let (y0, y1) = (la.y.min(lb.y), la.y.max(lb.y));
    match (ticks(x0, x1, step.0), ticks(y0, y1, step.1)) {
        (Some(xs), Some(ys)) => {
            for x in xs {
                b.segs.push(Segment::MoveTo(t.apply(Point::new(x, y0))));
                b.segs.push(Segment::LineTo(t.apply(Point::new(x, y1))));
            }
            for y in ys {
                b.segs.push(Segment::MoveTo(t.apply(Point::new(x0, y))));
                b.segs.push(Segment::LineTo(t.apply(Point::new(x1, y))));
            }
        }
        _ => e.warn(format!("grid: step {} × {} is invalid or too fine (more than {MAX_GRID_LINES} lines): grid ignored", step.0, step.1)),
    }
    b.last = Some(OnPath::Point(r.p));
    b.current = Some(r);
    Ok(())
}

/// `plot[options] coordinates {…}` and `plot[options] (expression)`.
fn plot(e: &mut Engine, b: &mut Builder, depth: Option<usize>, continues: bool) -> Result<(), Error> {
    e.begin_group();
    type Plot = (Vec<Point>, Vec<Segment>, Vec<crate::canvas::Item>, bool);
    let res = (|| -> Result<Plot, Error> {
        e.process_keys(tikra_tokens("every plot/.try"), Some("/tikz"))?;
        if let Some(t) = peek(e, depth)? {
            if t.is_char('[') {
                let _ = next(e, depth)?;
                let o = e.tex.read_until_char(']', false)?;
                e.process_keys(o, Some("/tikz"))?;
            }
        }
        let (d0, d1) = e.st.domain;
        let n = e.st.samples.max(2);
        let var = e.st.variable.clone();
        let mut points = Vec::new();
        match next(e, depth)? {
            Some(Token::Char('c', Cat::Letter)) => {
                let w = word(e, 'c');
                if w != "coordinates" {
                    return Err(Error::Figure(format!("plot {w} not supported")));
                }
                let list = match next(e, depth)? {
                    Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
                    _ => return Err(Error::Figure("plot coordinates without a list".into())),
                };
                let depth0 = e.tex.depth();
                e.tex.push_frame(list);
                while let Some(t) = e.tex.get_x_token_until(depth0)? {
                    if t.is_char('(') {
                        let r = coordinate(e)?;
                        points.push(r.p);
                    }
                }
            }
            Some(Token::Char('f', Cat::Letter)) => {
                // `plot file {name}`, and `plot function {…}`, which reads the table gnuplot wrote
                // for it (`prefix` and `id` name it), as TeX does when it does not call gnuplot
                let w = word(e, 'f');
                let arg = e.argument()?;
                let name = match w.as_str() {
                    "file" => e.tex.expand_to_string(arg)?.trim().to_string(),
                    "function" => {
                        let key = |e: &mut Engine, k: &str| crate::keys::value(&e.tex, k).map(|v| e.tex.expand_to_string(v).unwrap_or_default()).unwrap_or_default();
                        let (prefix, id) = (key(e, "/tikz/prefix"), key(e, "/tikz/id"));
                        format!("{prefix}{}.table", if id.is_empty() { "plot" } else { id.trim() })
                    }
                    _ => return Err(Error::Figure(format!("plot {w} not supported"))),
                };
                let text = super::pgfplots::table::read_file(&name, &[]).map_err(|m| Error::Figure(format!("plot {w}: {m}")))?;
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') || line.starts_with('%') {
                        continue;
                    }
                    let f: Vec<&str> = line.split_whitespace().collect();
                    let (Some(x), Some(y)) = (f.first().and_then(|v| v.parse::<f64>().ok()), f.get(1).and_then(|v| v.parse::<f64>().ok())) else { continue };
                    // `u`: undefined, the line breaks there
                    if f.get(2) == Some(&"u") {
                        points.push(Point::new(f64::NAN, f64::NAN));
                        continue;
                    }
                    let r = e.coordinate(&format!("{},{}", super::pgf_number(x), super::pgf_number(y)))?;
                    points.push(r.p);
                }
            }
            Some(t) if t.is_char('(') => {
                let tokens = read_coordinate_tokens(e)?;
                let name = Token::cs(&var);
                let values = e.st.samples_at.clone().unwrap_or_else(|| (0..n).map(|k| d0 + (d1 - d0) * k as f64 / (n - 1) as f64).collect());
                for v in values {
                    e.tex.set_meaning(&name, macro_from(&super::pgf_number(v)), false);
                    let text = e.math_string(tokens.clone())?;
                    match e.coordinate(&text) {
                        Ok(r) if r.p.x.is_finite() && r.p.y.is_finite() => points.push(r.p),
                        Ok(_) => {}
                        Err(err) => {
                            e.warn(format!("plot: {err}"));
                            break;
                        }
                    }
                }
            }
            _ => return Err(Error::Figure("plot without an expression".into())),
        }
        // the marks, drawn after the path, in the plot's own settings
        let marks = e.plot_marks(&points)?;
        let segs = plot_segments(e, &points)?;
        Ok((points, segs, marks, e.st.only_marks))
    })();
    e.end_group();
    let (points, mut segs, marks, only_marks) = res?;
    b.node_items.extend(marks);
    if points.is_empty() || only_marks {
        if let Some(p) = points.last() {
            b.current = Some(Coord { p: *p, node: None });
        }
        return Ok(());
    }
    // `--plot` continues the path: the plot's first move is a line
    if continues && b.current.is_some() {
        if let Some(Segment::MoveTo(p)) = segs.first().copied() {
            segs[0] = Segment::LineTo(p);
        }
    }
    if let Some(Segment::MoveTo(p)) = segs.first() {
        b.subpath_start = Some(*p);
    }
    b.segs.extend(segs);
    let points: Vec<Point> = points.into_iter().filter(|p| p.x.is_finite()).collect();
    let n = points.len();
    if n == 0 {
        return Ok(());
    }
    b.last = Some(OnPath::Segment(points[n.saturating_sub(2)], None, points[n - 1]));
    b.current = Some(Coord { p: points[n - 1], node: None });
    Ok(())
}

/// The path of a plot's points (canvas coordinates) as its handler makes it, in the plot's own
/// coordinate system (where the zero level of combs and bars is, and bar widths are measured).
pub fn plot_segments(e: &mut Engine, points: &[Point]) -> Result<Vec<Segment>, Error> {
    // an undefined point breaks the plot: each run is plotted on its own
    if points.iter().any(|p| !p.x.is_finite()) {
        let mut out = Vec::new();
        for run in points.split(|p| !p.x.is_finite()).filter(|r| !r.is_empty()) {
            out.extend(plot_segments(e, run)?);
        }
        return Ok(out);
    }
    let t = e.st.transform;
    let inv = t.inverse();
    let q: Vec<Point> = points.iter().map(|p| inv.apply(*p)).collect();
    let n = q.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let p = Point::new;
    let mut v = Vec::new();
    let handler = e.st.plot_handler.clone();
    let k = 0.2775 * e.st.tension;
    match handler.as_str() {
        "smooth" if n > 2 => {
            // \pgfplothandlercurveto: each inner point's supports along its neighbours' chord
            v.push(Segment::MoveTo(q[0]));
            let mut support = q[0];
            for i in 1..n - 1 {
                let d = q[i + 1].minus(q[i - 1]).scale(k);
                v.push(Segment::CurveTo(support, q[i].minus(d), q[i]));
                support = q[i].plus(d);
            }
            v.push(Segment::CurveTo(support, q[n - 1], q[n - 1]));
        }
        "smooth cycle" if n > 2 => {
            // \pgfplothandlerclosedcurve: from the second point round to it again
            let d = |i: usize| q[(i + 1) % n].minus(q[(i + n - 1) % n]).scale(k);
            v.push(Segment::MoveTo(q[1]));
            for step in 1..=n {
                let (i, j) = (step % n, (step + 1) % n);
                v.push(Segment::CurveTo(q[i].plus(d(i)), q[j].minus(d(j)), q[j]));
            }
            v.push(Segment::Close);
        }
        "xcomb" | "ycomb" | "polar comb" => {
            for a in &q {
                let base = match handler.as_str() {
                    "xcomb" => p(0.0, a.y),
                    "ycomb" => p(a.x, 0.0),
                    _ => p(0.0, 0.0),
                };
                v.extend([Segment::MoveTo(base), Segment::LineTo(*a)]);
            }
        }
        "xbar" | "ybar" => {
            let (w, sh) = (e.key_text("/pgf/bar width"), e.key_text("/pgf/bar shift"));
            let (width, shift) = (e.length(&w)?, e.length(&sh)?);
            for a in &q {
                // \pgfpathrectangle from the zero level to the point, bar width across
                let (lo, size) = if handler == "ybar" { (p(a.x - width / 2.0 + shift, 0.0), p(width, a.y)) } else { (p(0.0, a.y - width / 2.0 + shift), p(a.x, width)) };
                v.extend([
                    Segment::MoveTo(lo),
                    Segment::LineTo(p(lo.x + size.x, lo.y)),
                    Segment::LineTo(p(lo.x + size.x, lo.y + size.y)),
                    Segment::LineTo(p(lo.x, lo.y + size.y)),
                    Segment::Close,
                ]);
            }
        }
        "ybar interval" | "xbar interval" => {
            // a bar for each interval between two points, its height the first's
            let num = |e: &mut Engine, k: &str, d: f64| -> f64 {
                let t = e.key_text(k);
                if t.trim().is_empty() {
                    d
                } else {
                    e.number(&t).unwrap_or(d)
                }
            };
            let (width, shift) = (num(e, "/pgf/bar interval width", 1.0), num(e, "/pgf/bar interval shift", 0.5));
            for w in q.windows(2) {
                let (a, b) = (w[0], w[1]);
                let (lo, size) = if handler == "ybar interval" {
                    let d = b.x - a.x;
                    (p(a.x + shift * d - 0.5 * width * d, 0.0), p(width * d, a.y))
                } else {
                    let d = b.y - a.y;
                    (p(0.0, a.y + shift * d - 0.5 * width * d), p(a.x, width * d))
                };
                v.extend([
                    Segment::MoveTo(lo),
                    Segment::LineTo(p(lo.x + size.x, lo.y)),
                    Segment::LineTo(p(lo.x + size.x, lo.y + size.y)),
                    Segment::LineTo(p(lo.x, lo.y + size.y)),
                    Segment::Close,
                ]);
            }
        }
        "gap lineto" | "gap cycle" => {
            // lines from point to point, stopping `gap around stream point` short of each
            let g = e.key_text("/pgf/gap around stream point");
            let gap = if g.trim().is_empty() { 1.5 * crate::PT } else { e.length(&g)? };
            let mut pts = q.clone();
            if handler == "gap cycle" && n > 1 {
                pts.push(q[0]);
            }
            for w in pts.windows(2) {
                let d = w[1].minus(w[0]);
                let u = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { p(0.0, 0.0) };
                v.extend([Segment::MoveTo(w[0].plus(u.scale(gap))), Segment::LineTo(w[1].minus(u.scale(gap)))]);
            }
        }
        h if h.starts_with("const plot") || h.starts_with("jump mark") => {
            v.push(Segment::MoveTo(q[0]));
            for w in q.windows(2) {
                let (a, b) = (w[0], w[1]);
                let mid = 0.5 * a.x + 0.5 * b.x;
                match h {
                    "const plot mark right" => v.extend([Segment::LineTo(p(a.x, b.y)), Segment::LineTo(b)]),
                    "const plot mark mid" => v.extend([Segment::LineTo(p(mid, a.y)), Segment::LineTo(p(mid, b.y)), Segment::LineTo(b)]),
                    "jump mark left" => v.extend([Segment::LineTo(p(b.x, a.y)), Segment::MoveTo(b)]),
                    "jump mark right" => v.extend([Segment::MoveTo(p(a.x, b.y)), Segment::LineTo(b)]),
                    "jump mark mid" => v.extend([Segment::LineTo(p(mid, a.y)), Segment::MoveTo(p(mid, b.y)), Segment::LineTo(b)]),
                    _ => v.extend([Segment::LineTo(p(b.x, a.y)), Segment::LineTo(b)]),
                }
            }
        }
        _ => {
            v.push(Segment::MoveTo(q[0]));
            v.extend(q[1..].iter().map(|a| Segment::LineTo(*a)));
            if handler == "sharp cycle" {
                v.push(Segment::Close);
            }
        }
    }
    Ok(crate::canvas::transform_segments(&v, &t))
}

/// `parabola (b)`, `parabola bend (c) (b)`, `parabola[bend at end] (b)`.
fn parabola(e: &mut Engine, b: &mut Builder, depth: Option<usize>) -> Result<(), Error> {
    // `bend pos=f`: a relative bend is from the point that far from the start to the end (the
    // start by default); `parabola height=h`: the bend is h above the middle; `bend at end`
    let (mut factor, mut height, mut bend_at_end) = (0.0, None, false);
    if let Some(t) = peek(e, depth)? {
        if t.is_char('[') {
            let _ = next(e, depth)?;
            let o = e.tex.read_until_char(']', false)?;
            for item in crate::keys::split_list(&o) {
                let value = item.value.map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
                match item.key.as_str() {
                    "bend at end" => bend_at_end = true,
                    "bend at start" => factor = 0.0,
                    "bend pos" => factor = e.number(&value)?,
                    "parabola height" => {
                        factor = 0.5;
                        height = Some(e.length(&value)?);
                    }
                    _ => {}
                }
            }
        }
    }
    let mut bend = None;
    if let Some(Token::Char('b', Cat::Letter)) = peek(e, depth)? {
        let _ = next(e, depth)?;
        let w = word(e, 'b');
        if w == "bend" {
            let relative = matches!(peek(e, depth)?, Some(t) if t.is_char('+'));
            bend = Some((target(e, b, depth, "parabola bend")?.p, relative));
        }
    }
    let r = target(e, b, depth, "parabola")?;
    let a = b.point();
    if b.current.is_none() {
        b.segs.push(Segment::MoveTo(a));
    }
    let saved = a.lerp(r.p, factor);
    let bend = match bend {
        Some((p, true)) => Some(p.minus(a).plus(saved)),
        Some((p, false)) => Some(p),
        // the height is a y vector of the picture: `scale` and `yscale` stretch it too
        None => height.map(|h| saved.plus(e.st.transform.linear(Point::new(0.0, h)))),
    };
    // \pgfpathparabola: to the bend with its vertex there, then on to the end with its vertex at the bend
    let (bend_vector, end_vector) = match bend {
        Some(v) => (v.minus(a), r.p.minus(v)),
        None if bend_at_end => (r.p.minus(a), Point::default()),
        None => (Point::default(), r.p.minus(a)),
    };
    let mut from = a;
    if bend_vector.norm() > 1e-12 {
        let c = bend_vector;
        b.segs.push(Segment::CurveTo(from.plus(Point::new(0.1125 * c.x, 0.225 * c.y)), from.plus(Point::new(0.5 * c.x, c.y)), from.plus(c)));
        from = from.plus(c);
    }
    if end_vector.norm() > 1e-12 {
        let c = end_vector;
        b.segs.push(Segment::CurveTo(from.plus(Point::new(0.5 * c.x, 0.0)), from.plus(Point::new(0.8875 * c.x, 0.775 * c.y)), from.plus(c)));
    }
    b.current = Some(r);
    Ok(())
}

/// `let \p1 = (a), \n1 = {expr} in`: defines `\p1`, `\x1`, `\y1`, `\n1` for the rest of the path.
fn let_operation(e: &mut Engine, depth: Option<usize>) -> Result<(), Error> {
    loop {
        // \p and \n are read as they are: they are macros once a first one is defined
        let t = loop {
            let t = match depth {
                Some(d) => e.tex.get_token_until(d),
                None => e.tex.get_token(),
            };
            match t {
                Some(t) if t.is_space() => continue,
                other => break other,
            }
        };
        let Some(t) = t else { return Ok(()) };
        let name = match &t {
            Token::Cs(n) => n.to_string(),
            Token::Char('i', Cat::Letter) => {
                let w = word(e, 'i');
                if w == "in" {
                    return Ok(());
                }
                return Err(Error::Figure(format!("let: '{w}' unexpected")));
            }
            Token::Char(',', _) => continue,
            other => return Err(Error::Figure(format!("let: '{}' unexpected", tex::show_token(other)))),
        };
        // the name after \p, \n: an argument, \p1, \p{name} or \p\i (expanded, as a \csname makes it)
        let index = match e.tex.get_token() {
            Some(Token::Char(_, Cat::Begin)) => {
                let name = e.tex.read_group_body()?;
                e.tex.expand_to_string(name)?
            }
            Some(Token::Char(c, _)) => c.to_string(),
            Some(t) => e.tex.expand_to_string(vec![t])?,
            None => String::new(),
        };
        match next(e, depth)? {
            Some(t) if t.is_char('=') => {}
            _ => return Err(Error::Figure("let: '=' missing".into())),
        }
        match name.as_str() {
            "p" => {
                let p = match next(e, depth)? {
                    Some(t) if t.is_char('(') => coordinate(e)?.p,
                    _ => return Err(Error::Figure("let \\p: a point is missing".into())),
                };
                let (x, y) = (p.x / crate::PT, p.y / crate::PT);
                // \x and \y are in the coordinates of the picture's transformation (a point read in a
                // shifted scope gives what it was given there), so `(\x1,0)` comes back to the point
                let l = e.st.transform.inverse().apply(p);
                let (lx, ly) = (l.x / crate::PT, l.y / crate::PT);
                // \p1, \p{name}: \p takes the name as its argument, as TikZ defines it
                for (letter, value) in [("p", format!("tikra cs:{x}pt,{y}pt")), ("x", format!("{lx}pt")), ("y", format!("{ly}pt"))] {
                    e.tex.set_meaning(&Token::cs(&format!("tikra@let@{letter}@{index}")), macro_from(&value), false);
                    let_macro(e, letter)?;
                }
            }
            "n" => {
                let v = match next(e, depth)? {
                    Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
                    Some(t) => vec![t],
                    None => Vec::new(),
                };
                let text = e.math_string(v)?;
                let (val, unit) = crate::expr::eval_length(&text, &e.vars())?;
                let s = if unit { format!("{}pt", super::pgf_number(val / crate::PT)) } else { super::pgf_number(val) };
                e.tex.set_meaning(&Token::cs(&format!("tikra@let@n@{index}")), macro_from(&s), false);
                let_macro(e, "n")?;
            }
            other => return Err(Error::Figure(format!("let: \\{other} is not \\p or \\n"))),
        }
    }
}

/// `\p` (`\x`, `\y`, `\n`) as `let` defines it: one argument, the name of what it holds.
fn let_macro(e: &mut Engine, letter: &str) -> Result<(), Error> {
    e.run_tokens(tikra_tokens(&format!("\\def\\{letter}#1{{\\csname tikra@let@{letter}@#1\\endcsname}}")))
}

/// `\foreach` inside a path: its body is more path operations.
fn path_foreach(e: &mut Engine, b: &mut Builder) -> Result<(), Error> {
    let (names, list, options) = foreach_header(e)?;
    let body = match e.tex.next_nonspace() {
        Some(Token::Char(_, Cat::Begin)) => e.tex.read_group_body()?,
        Some(t) => vec![t],
        None => Vec::new(),
    };
    foreach_run(e, b, names, list, options, body)
}

/// `\x/\y in {list}` of a `foreach` (the word already read): the variables and the list.
fn foreach_header(e: &mut Engine) -> Result<(Vec<Token>, Vec<Token>, super::ForeachOptions), Error> {
    let mut vars: Vec<Token> = Vec::new();
    let mut opts: Vec<Token> = Vec::new();
    loop {
        let Some(t) = e.tex.get_token() else { return Err(Error::Figure("\\foreach without 'in'".into())) };
        if t.is_char('[') {
            opts.extend(e.tex.read_until_char(']', false)?);
            continue;
        }
        if t.is_char('i') {
            if let Some(n) = e.tex.get_token() {
                if n.is_char('n') {
                    break;
                }
                e.tex.back(n);
            }
        }
        vars.push(t);
    }
    let names: Vec<Token> = vars.iter().filter(|t| matches!(t, Token::Cs(_))).cloned().collect();
    let list = e.foreach_list()?;
    Ok((names, list, super::foreach_options(&opts)))
}

/// `node foreach \x in {list} [options] at (…) {text}`: the node once for each item.
fn node_foreach(e: &mut Engine, b: &mut Builder, kind: &str) -> Result<(), Error> {
    let (names, list, options) = foreach_header(e)?;
    // the rest of the node as it is written, up to its text
    let mut spec = tikra_tokens(&format!("{kind} "));
    // the letters of the last word, spaces after it skipped
    let last_word = |spec: &[Token]| -> String {
        let s: Vec<char> =
            spec.iter().rev().skip_while(|t| matches!(t, Token::Char(_, Cat::Space))).map_while(|t| if let Token::Char(c, Cat::Letter) = t { Some(*c) } else { None }).collect();
        s.into_iter().rev().collect()
    };
    // inside another `foreach \y in {list}` (the nodes for each pair), before its `in`
    let mut header = false;
    while let Some(t) = e.tex.get_token() {
        if !matches!(t, Token::Char(_, Cat::Letter)) {
            match last_word(&spec).as_str() {
                "foreach" => header = true,
                "in" => header = false,
                _ => {}
            }
        }
        match &t {
            Token::Cs(_) | Token::Char('/', _) if header => spec.push(t),
            Token::Char(_, Cat::Begin) if last_word(&spec) == "in" => {
                header = false;
                spec.push(t);
                spec.extend(e.tex.read_group_body()?);
                spec.push(Token::Char('}', Cat::End));
            }
            Token::Char(_, Cat::Begin) if kind != "coordinate" => {
                spec.push(t);
                spec.extend(e.tex.read_group_body()?);
                spec.push(Token::Char('}', Cat::End));
                break;
            }
            Token::Char('[', _) | Token::Char('(', _) => {
                let close = if t.is_char('[') { ']' } else { ')' };
                spec.push(t);
                spec.extend(e.tex.read_until_char(close, false)?);
                spec.push(Token::other(close));
            }
            // `at +(x,y)`
            Token::Char(_, Cat::Letter) | Token::Char(_, Cat::Space) | Token::Char('+', _) => spec.push(t),
            _ => {
                e.tex.back(t);
                break;
            }
        }
    }
    foreach_run(e, b, names, list, options, spec)
}

/// The path operations `body` once for each item of `list`, with the variables set.
fn foreach_run(e: &mut Engine, b: &mut Builder, names: Vec<Token>, list: Vec<Token>, options: super::ForeachOptions, body: Vec<Token>) -> Result<(), Error> {
    let mut memory = options.initial_memory();
    for (k, item) in super::foreach_items(&list).into_iter().enumerate() {
        e.begin_group();
        let parts: Vec<Vec<Token>> = item.split(|t| t.is_char('/')).map(|p| keys::trim(p).to_vec()).collect();
        for (i, n) in names.iter().enumerate() {
            let v = parts.get(i).or(parts.last()).cloned().unwrap_or_default();
            e.tex.set_meaning(n, super::macro_tokens(v), false);
        }
        e.foreach_iteration(&options, k, &memory)?;
        let depth = e.tex.depth();
        e.tex.push_frame(body.clone());
        let r = operations(e, b, Some(depth));
        e.tex.drain_to(depth);
        e.foreach_remember(&options, &mut memory);
        // the path's settings stay; only the loop variables end with the iteration
        let st = e.st.clone();
        e.end_group();
        e.st = st;
        r?;
    }
    Ok(())
}
