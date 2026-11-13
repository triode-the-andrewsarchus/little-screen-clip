//! What visualizers draw: the lines and marks of `visualize as line`, `smooth line`, `scatter`…
//! (the plot handler visualizer's `render()`, with pgf's plot handlers and plot marks), the
//! rectangles of `visualize as rectangles`, and the legends: entries laid out in rows and columns
//! by the legend class, each a text with a sample of its visualizer, in a matrix.

use crate::color::Color;
use crate::scene::Point;
use crate::tex::{self, Meaning, Token};
use crate::{Error, PT};

use super::axes::pt;
use super::objects::Object;
use super::{attribute, group, key_text, macro_text, set_attribute, Engine, PathToken, Signal};
use crate::engine::tikra_tokens;

/// How a visualizer joins its points (pgf's plot handlers), by the name its styling gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handler {
    LineTo,
    CurveTo,
    ClosedCurve,
    Polygon,
    Discard,
}

fn handler_of(name: &str) -> Handler {
    match name.trim() {
        "curveto" => Handler::CurveTo,
        "closedcurve" => Handler::ClosedCurve,
        "polygon" => Handler::Polygon,
        "discard" => Handler::Discard,
        _ => Handler::LineTo,
    }
}

/// pgf's plot tension (0.5) as `\pgfsetplottension` stores it.
const TENSION: f64 = 0.5 * 0.2775;

/// A stream of points as a path with a plot handler (pt).
fn stream_path(points: &[Point], h: Handler) -> String {
    let mut out = String::new();
    if points.is_empty() {
        return out;
    }
    let p = |q: Point| pt(q);
    match h {
        Handler::LineTo | Handler::Polygon => {
            out.push_str(&p(points[0]));
            for q in &points[1..] {
                out.push_str(" -- ");
                out.push_str(&p(*q));
            }
            if h == Handler::Polygon {
                out.push_str(" -- cycle");
            }
        }
        Handler::CurveTo => {
            out.push_str(&p(points[0]));
            if points.len() == 1 {
                return out;
            }
            let mut first = points[0];
            let mut support = points[0];
            let mut second = points[1];
            for current in &points[2..] {
                let d = current.minus(first).scale(TENSION);
                let b = second.minus(d);
                out.push_str(&format!(" .. controls {} and {} .. {}", p(support), p(b), p(second)));
                first = second;
                support = second.plus(d);
                second = *current;
            }
            out.push_str(&format!(" .. controls {} and {} .. {}", p(support), p(second), p(second)));
        }
        Handler::ClosedCurve => {
            let n = points.len();
            if n < 3 {
                return stream_path(points, Handler::Polygon);
            }
            let d = |i: usize| points[(i + 1) % n].minus(points[(i + n - 1) % n]).scale(TENSION);
            out.push_str(&p(points[0]));
            for i in 0..n {
                let j = (i + 1) % n;
                out.push_str(&format!(" .. controls {} and {} .. {}", p(points[i].plus(d(i))), p(points[j].minus(d(j))), p(points[j])));
            }
            out.push_str(" -- cycle");
        }
        Handler::Discard => {}
    }
    out
}

/// xcolor's hsb (all in [0, 1]) as rgb.
fn hsb(h: f64, s: f64, b: f64) -> Color {
    let h = (h - h.floor()) * 6.0;
    let i = h.floor() as i32 % 6;
    let f = h - h.floor();
    let (p, q, t) = (b * (1.0 - s), b * (1.0 - s * f), b * (1.0 - s * (1.0 - f)));
    let (r, g, bl) = match i {
        0 => (b, t, p),
        1 => (q, b, p),
        2 => (p, b, t),
        3 => (p, q, b),
        4 => (t, p, b),
        _ => (b, p, q),
    };
    Color::rgb(r, g, bl)
}

impl Engine {
    /// The visualizer's color (`visualizer color`, a color name) around `f`: TeX's `\colorlet` is local.
    fn keeping_visualizer_color<T>(&mut self, f: impl FnOnce(&mut Engine) -> Result<T, Error>) -> Result<T, Error> {
        let saved = self.colors.get("visualizer color").copied().expect("the library sets `visualizer color=black`");
        let r = f(self);
        self.colors.insert("visualizer color".into(), saved);
        r
    }

    /// A visualizer's scope (its options, `every visualizer`, its style sheets' style, its styling),
    /// after the style signal once, so that the visualizer's color is known when its options use it
    /// (TikZ sets colors after all options).
    fn begin_visualizer(&mut self, name: &str) -> Result<(), Error> {
        self.begin_group();
        set_attribute(self, "set", name);
        let old = std::mem::replace(&mut self.st.key_path, "/tikz".into());
        let styling = tikra_tokens(&format!("/tikz/data visualization/visualizers/{name}/styling/.try"));
        let r = self.emit(Signal::Style, None).and_then(|_| self.keys_command(styling, Some("/tikz"), true));
        self.st.key_path = old;
        self.end_group();
        r?;
        set_attribute(self, "set", name);
        let base = format!("/tikz/data visualization/visualizers/{name}");
        let mut open = tikra_tokens("\\begin{scope}[");
        open.extend(crate::keys::value(&self.tex, &format!("{base}/options")).unwrap_or_default());
        open.extend(tikra_tokens(&format!(",/tikz/data visualization/every visualizer/.try,{base}/signal,{base}/styling/.try]")));
        self.run_tokens(open)
    }

    fn end_visualizer(&mut self, _name: &str) -> Result<(), Error> {
        self.run_tokens(tikra_tokens("\\end{scope}"))
    }

    /// The plot handler the visualizer's styling chose.
    fn current_handler(&mut self) -> Handler {
        handler_of(&macro_text(self, "tikra@dv@handler"))
    }

    /// The path of the points with the current handler, drawn (every path is `draw` in visualizers).
    fn draw_stream(&mut self, positions: &[Option<Point>]) -> Result<(), Error> {
        let h = self.current_handler();
        let mut path = String::new();
        for stream in positions.split(|p| p.is_none()) {
            let pts: Vec<Point> = stream.iter().flatten().copied().collect();
            let s = stream_path(&pts, h);
            if !s.is_empty() {
                path.push(' ');
                path.push_str(&s);
            }
        }
        if path.is_empty() {
            return Ok(());
        }
        self.run_tokens(tikra_tokens(&format!("\\path{path};")))
    }

    /// The marks at the points (`\tikra@dv@marks`: `solid`, then `every mark`).
    fn draw_marks(&mut self, positions: &[Option<Point>]) -> Result<(), Error> {
        if self.st.plot_mark.is_none() {
            return Ok(());
        }
        self.begin_group();
        let r = (|| {
            self.keys_command(tikra_tokens("solid"), Some("/tikz"), true)?;
            let t = self.st.transform;
            let points: Vec<Point> = positions.iter().flatten().map(|p| t.apply(Point::new(p.x * PT, p.y * PT))).collect();
            let items = self.plot_marks(&points)?;
            self.canvas.items.extend(items);
            Ok(())
        })();
        self.end_group();
        r
    }

    /// A plot handler visualizer's `render()`, at the end of the visualization: for each of its
    /// handlers (the line, the marks), the visualizer's scope and what the handler draws.
    pub fn render_visualizer(&mut self, id: usize) -> Result<(), Error> {
        let Some(Object::PlotVisualizer { name, handlers, positions, .. }) = self.datavis.last().and_then(|d| d.objects.get(id)).cloned() else { return Ok(()) };
        if let Some(Object::PlotVisualizer { positions, .. }) = self.datavis.last_mut().and_then(|d| d.objects.get_mut(id)) {
            positions.clear();
        }
        self.keeping_visualizer_color(|e| {
            e.begin_group();
            e.begin_scope();
            let r = (|| {
                for h in super::axes_split(&handlers) {
                    let Some(Token::Cs(hn)) = h.iter().find(|t| !t.is_space()).cloned() else { continue };
                    e.begin_visualizer(&name)?;
                    if &*hn == "tikra@dv@marks" {
                        e.draw_marks(&positions)?;
                    } else {
                        e.draw_stream(&positions)?;
                    }
                    e.end_visualizer(&name)?;
                }
                Ok(())
            })();
            e.end_scope();
            e.end_group();
            r
        })
    }

    /// A rectangle of `visualize as rectangles`, from the attributes' `min` and `max` sub keys.
    pub fn render_rectangle(&mut self, name: &str, a1: &str, a2: &str) -> Result<(), Error> {
        let get = |e: &mut Engine, k: &str| key_text(e, k).filter(|s| !s.is_empty());
        let (Some(amin), Some(amax), Some(bmin), Some(bmax)) =
            (get(self, &format!("{a1}/min")), get(self, &format!("{a1}/max")), get(self, &format!("{a2}/min")), get(self, &format!("{a2}/max")))
        else {
            return Ok(());
        };
        let (k1, k2) = (a1.trim_start_matches("/data point/").to_string(), a2.trim_start_matches("/data point/").to_string());
        self.keeping_visualizer_color(|e| {
            e.begin_group();
            let r = (|| {
                e.begin_visualizer(name)?;
                let mut steps = Vec::new();
                for (i, (x, y)) in [(&amin, &bmin), (&amin, &bmax), (&amax, &bmax), (&amax, &bmin), (&amin, &bmin)].iter().enumerate() {
                    set_attribute(e, &k1, x);
                    set_attribute(e, &k2, y);
                    let token = if i == 0 { PathToken::MoveTo } else { PathToken::LineTo };
                    // on polar axes, the sides along an angle are arcs
                    steps.push((token, e.dv_generic(Signal::Path, token)?));
                }
                e.run_tokens(tikra_tokens(&format!("\\path{} -- cycle;", Engine::path_text(&steps))))?;
                e.end_visualizer(name)
            })();
            e.end_group();
            r
        })
    }

    // ---------------------------------------------------------------- legends

    /// The legend's entries in rows and columns (`get arrangement`), cells split by `&` and rows
    /// ended by `\\`.
    pub fn legend_arrangement(&mut self, id: usize) -> Vec<Token> {
        let Some(Object::Legend { columns, ideal, max, strategy, entries }) = self.datavis.last().and_then(|d| d.objects.get(id)).cloned() else { return Vec::new() };
        let n = entries.len();
        let (ideal, max) = (ideal.max(1), max.max(1));
        // a = number of targets, b = entries per target
        let (mut a, mut b) = if n > ideal {
            if ideal * max > n {
                (ideal, (n - 1) / ideal + 1)
            } else {
                ((n - 1) / max + 1, max)
            }
        } else {
            (n, 1)
        };
        // a = rows, b = columns
        if columns {
            std::mem::swap(&mut a, &mut b);
        }
        let (rows, cols) = (a.max(1), b.max(1));
        let mut grid = vec![vec![None; cols]; rows];
        let (mut r, mut c): (i64, i64) = match strategy.as_str() {
            "up then right" | "right then up" => (rows as i64, 1),
            "left then up" | "up then left" => (rows as i64, cols as i64),
            "left then down" | "down then left" => (1, cols as i64),
            _ => (1, 1),
        };
        for (k, entry) in entries.iter().enumerate() {
            if r >= 1 && c >= 1 && (r as usize) <= rows && (c as usize) <= cols {
                grid[r as usize - 1][c as usize - 1] = Some(k);
            }
            let _ = entry;
            match strategy.as_str() {
                "right then down" => {
                    c += 1;
                    if c > cols as i64 {
                        c = 1;
                        r += 1;
                    }
                }
                "up then right" => {
                    r -= 1;
                    if r == 0 {
                        r = rows as i64;
                        c += 1;
                    }
                }
                "right then up" => {
                    c += 1;
                    if c > cols as i64 {
                        c = 1;
                        r -= 1;
                    }
                }
                "left then up" => {
                    c -= 1;
                    if c == 0 {
                        c = cols as i64;
                        r -= 1;
                    }
                }
                "up then left" => {
                    r -= 1;
                    if r == 0 {
                        r = rows as i64;
                        c -= 1;
                    }
                }
                "left then down" => {
                    c -= 1;
                    if c == 0 {
                        c = cols as i64;
                        r += 1;
                    }
                }
                "down then left" => {
                    r += 1;
                    if r > rows as i64 {
                        r = 1;
                        c -= 1;
                    }
                }
                _ => {
                    r += 1;
                    if r > rows as i64 {
                        r = 1;
                        c += 1;
                    }
                }
            }
        }
        let mut out = Vec::new();
        for row in grid {
            for (k, cell) in row.iter().enumerate() {
                if k > 0 {
                    out.push(Token::Char('&', crate::tex::Cat::Align));
                }
                if let Some(i) = cell {
                    out.extend(entries[*i].clone());
                }
            }
            out.push(Token::cs("\\"));
        }
        out
    }

    /// A legend, as a matrix at its `at`, anchored at its `anchor`.
    pub fn show_legend(&mut self, legend: &str) -> Result<(), Error> {
        self.update_bounding_box(false);
        let Some(id) = self.object_of(&format!("/tikz/data visualization/{legend}/obj")) else { return Ok(()) };
        let body = self.legend_arrangement(id);
        let at = key_text(self, &format!("/tikz/data visualization/{legend}/at")).unwrap_or_default();
        let anchor = key_text(self, &format!("/tikz/data visualization/{legend}/anchor")).unwrap_or_default();
        let mut v = tikra_tokens(&format!("\\matrix[at={{{at}}},matrix anchor={anchor},/tikz/data visualization/{legend}/matrix node styling]"));
        v.extend(group(body));
        v.extend(tikra_tokens(";"));
        self.begin_group();
        let r = self.run_tokens(v);
        self.end_group();
        r
    }

    /// A legend entry: its text and its visualizer's sample.
    pub fn render_legend_entry(&mut self, options: Vec<Token>) -> Result<(), Error> {
        self.keeping_visualizer_color(|e| {
            e.begin_group();
            let r = e.legend_entry_in_group(options);
            e.end_group();
            r
        })
    }

    fn legend_entry_in_group(&mut self, options: Vec<Token>) -> Result<(), Error> {
        // in a cell, `reset cm` keeps the cell's place
        super::keys::define_native(self, "/tikz/reset cm", super::keys::k_cell_reset_cm);
        self.keys_command(tikra_tokens("/tikz/data visualization/.cd,every data set label"), Some("/"), true)?;
        let mut keys = tikra_tokens("/tikz/data visualization/legend entry options/.cd,/tikz/data visualization/every label in legend,");
        keys.extend(options);
        self.keys_command(keys, Some("/"), true)?;
        let text = crate::keys::value(&self.tex, "/tikz/data visualization/legend entry options/text").unwrap_or_default();
        let style = crate::keys::value(&self.tex, "/tikz/data visualization/@node styling").unwrap_or_default();
        let setup = crate::keys::value(&self.tex, "/tikz/data visualization/legend entry options/setup").unwrap_or_default();
        let styling = crate::keys::value(&self.tex, "/tikz/data visualization/legend entry options/visualizer in legend styling").unwrap_or_default();
        let sample = crate::keys::value(&self.tex, "/tikz/data visualization/legend entry options/visualizer in legend").unwrap_or_default();
        let text_only = matches!(self.tex.meaning_of("iftikra@dv@textonly"), Meaning::Prim(tex::Prim::Iftrue));
        self.begin_group();
        let r = (|| {
            self.keys_command(setup, Some("/tikz"), true)?;
            self.st.key_path = "/tikz".into();
            self.emit(Signal::Style, None)?;
            // the text has a strut of .75em above and .25em below its baseline (taken as its
            // height and depth: TeX's inline fractions fit in them, tikra's are taller)
            let mut node = tikra_tokens("\\node[/tikz/data visualization/node styling,text height=.75em,text depth=.25em,");
            node.extend(style);
            node.extend(tikra_tokens("]"));
            node.extend(group(text));
            node.extend(tikra_tokens(";"));
            self.run_tokens(node)?;
            if !text_only {
                self.begin_group();
                let r = self.keys_command(styling, Some("/tikz"), true).and_then(|_| self.run_tokens(sample));
                self.end_group();
                r?;
            }
            Ok(())
        })();
        self.end_group();
        r
    }

    /// A legend entry's sample (a line through the legend's line coordinates, marks at
    /// its mark coordinates) or `…@as@rectangle`.
    pub fn legend_sample(&mut self, rectangle: bool) -> Result<(), Error> {
        let name = attribute(self, "set").unwrap_or_default();
        let coords = |e: &mut Engine, key: &str| -> Result<Vec<Option<Point>>, Error> {
            let v = crate::keys::value(&e.tex, &format!("/tikz/data visualization/legend entry options/{key}")).unwrap_or_default();
            let v = e.tex.expand_fully(v)?;
            let mut out = Vec::new();
            for item in super::point_split(&v) {
                if crate::keys::trim(&item).is_empty() {
                    continue;
                }
                out.push(Some(super::keys::scan_point(e, item)?));
            }
            Ok(out)
        };
        if rectangle {
            let pts = coords(self, "label in legend rectangle coordinates")?;
            self.begin_visualizer(&name)?;
            let p: Vec<Point> = pts.iter().flatten().copied().collect();
            let path = stream_path(&p, Handler::Polygon);
            if !path.is_empty() {
                self.run_tokens(tikra_tokens(&format!("\\path {path};")))?;
            }
            return self.end_visualizer(&name);
        }
        let line = coords(self, "label in legend line coordinates")?;
        self.begin_visualizer(&name)?;
        self.draw_stream(&line)?;
        self.end_visualizer(&name)?;
        let marks = coords(self, "label in legend mark coordinates")?;
        self.begin_visualizer(&name)?;
        self.draw_marks(&marks)?;
        self.end_visualizer(&name)
    }

    /// `\tikzdvdeclarestylesheetcolorseries{name}{model}{start}{step}`: a style sheet whose n-th
    /// style is the n-th color of the series (`start` + n `step`, each part modulo 1).
    pub fn declare_color_series(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        let model = self.argument_string()?;
        let start = self.argument_string()?;
        let step = self.argument_string()?;
        let code = format!(
            "\\pgfdvdeclarestylesheet{{{name}}}{{default style/.style={{/utils/exec=\\tikra@dv@colorseries{{{model}}}{{{start}}}{{{step}}}{{#1}},visualizer color=tikra@dv@seriescolor}}}}"
        );
        self.run_tokens(tikra_tokens(&code))
    }

    /// `\tikra@dv@colorseries{model}{start}{step}{n}`: the color `tikra@dv@seriescolor`.
    pub fn color_series(&mut self) -> Result<(), Error> {
        let model = self.argument_string()?;
        let start = self.argument_string()?;
        let step = self.argument_string()?;
        let n = self.argument_string()?;
        let n = super::parse_value(self, &n).unwrap_or(0.0);
        let parse = |s: &str| -> Vec<f64> { s.split(',').map(|x| x.trim().parse().unwrap_or(0.0)).collect() };
        let (a, d) = (parse(&start), parse(&step));
        let part = |i: usize| -> f64 {
            let v = a.get(i).copied().unwrap_or(0.0) + n * d.get(i).copied().unwrap_or(0.0);
            if (0.0..=1.0).contains(&v) {
                v
            } else {
                v - v.floor()
            }
        };
        let c = match model.as_str() {
            "hsb" => hsb(part(0), part(1), part(2)),
            "rgb" => Color::rgb(part(0), part(1), part(2)),
            "gray" => Color::rgb(part(0), part(0), part(0)),
            "cmy" => Color::rgb(1.0 - part(0), 1.0 - part(1), 1.0 - part(2)),
            "cmyk" => Color::cmyk(part(0), part(1), part(2), part(3)),
            _ => {
                self.warn(format!("color series in the color model '{model}' not supported: black"));
                Color::BLACK
            }
        };
        self.colors.insert("tikra@dv@seriescolor".into(), c);
        Ok(())
    }

    // ---------------------------------------------------------------- labels in data

    /// The label visualizer of `label in data={options}`: the options say which attribute it
    /// watches and when to stop (its count, by default), it remembers the last two places.
    pub fn make_label_in_data(&mut self, name: &str, options: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        let r = (|| {
            self.tex.define_string("tikra@dv@visualizer", name, false);
            self.tex.define_string("tikra@dv@labelattr", "", false);
            self.tex.define_string("tikra@dv@labelat", "", false);
            self.keys_command(options.clone(), Some("/tikz/data visualization/visualizer label options"), true)?;
            Ok((macro_text(self, "tikra@dv@labelattr"), macro_text(self, "tikra@dv@labelat")))
        })();
        self.end_group();
        let (attr, at) = r?;
        let mut code = vec![Token::cs("tikra@dv@labeldraw")];
        code.extend(group(tex::string_tokens(name)));
        code.extend(group(options));
        let filter = tikra_tokens(&format!("\\pgfdvvisualizerfilter{{{name}}}"));
        self.create("label visualizer", &[tex::string_tokens(&attr), tex::string_tokens(&at), code, filter])?;
        Ok(())
    }

    /// A label in data, at the end of the visualization: a node beside the line from the place
    /// before to the place where the label goes, in the visualizer's style.
    pub fn draw_label_in_data(&mut self, name: &str, options: Vec<Token>) -> Result<(), Error> {
        let mut open = tikra_tokens(&format!("\\begin{{scope}}[/utils/exec={{\\def\\tikra@dv@visualizer{{{name}}}}},/tikz/data visualization/visualizer label options/.cd,"));
        open.extend(options);
        open.extend(tikra_tokens("]\\tikzdatavisualizationset{every data set label/.try}"));
        self.run_tokens(open)?;
        let text = crate::keys::value(&self.tex, "/tikz/data visualization/visualizer label options/text").unwrap_or_default();
        let option = |e: &mut Engine, key: &str| e.key_text(&format!("/tikz/data visualization/visualizer label options/{key}")).trim().to_string();
        // `pin in data`: the label moved a pin's distance off the line, to its left (right with
        // `text'`) or at `pin angle`, with a pin's edge to the place
        let mut shift = String::new();
        let mut pin_edge = String::new();
        if !option(self, "tikra pin").is_empty() {
            let length = option(self, "pin length");
            let sign = if option(self, "tikra pin sign") == "-" { -1.0 } else { 1.0 };
            let distance = self.length(&length)? * sign;
            // the label visualizer defines both coordinates before it draws its label
            let at = self.lookup_node("label visualizer coordinate").expect("the label's place").center();
            let before = self.lookup_node("label visualizer coordinate'").expect("the place before it").center();
            let angle = option(self, "pin angle");
            let d = if !angle.is_empty() {
                let a = self.number(&angle)?.to_radians();
                Some(Point::new(a.cos(), a.sin()))
            } else {
                let v = before.minus(at);
                let n = v.norm();
                (n > 1e-9).then(|| Point::new(v.y / n, -v.x / n))
            };
            if let Some(d) = d {
                shift = format!(",shift={{({}cm,{}cm)}},name=tikra@dv@pin", d.x * distance, d.y * distance);
                pin_edge = "\\path (tikra@dv@pin) edge[solid,every pin edge] (label visualizer coordinate);".into();
            }
        }
        let mut path = tikra_tokens(&format!(
            "\\path[/data point/set={{{name}}},/utils/exec=\\tikra@dv@stylesignal,/tikz/data visualization/.cd,every label in data/.try] (label visualizer coordinate') -- (label visualizer coordinate) node[auto,at end,/tikz/data visualization/node styling{shift}]"
        ));
        path.extend(group(text));
        path.extend(tikra_tokens(&format!(";{pin_edge}\\end{{scope}}")));
        self.run_tokens(path)
    }
}
