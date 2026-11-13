//! The legend (`\pgfplots@createlegend`): a TikZ matrix with `every axis legend`, one row per
//! entry, a small picture of the plot (`legend image code`) and a node with the text in each.
//! The matrix is laid out as TikZ does: the cells of a column share their origin's x, those of
//! a row their origin's y, columns and rows as wide and high as their largest cell.

use crate::canvas::Item;
use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Token};
use crate::{Error, PT};

use super::draw::Frame;
use super::keys;
use super::visual;
use super::{toks, Axis, Engine, LegendEntry};

/// A cell: what it draws (relative to its origin) and its extent.
struct Cell {
    items: Vec<Item>,
    lo: Point,
    hi: Point,
}

impl Cell {
    fn empty() -> Cell {
        Cell { items: Vec::new(), lo: Point::default(), hi: Point::default() }
    }
}

/// The legend's entries: `\addlegendentry`, `\legend` or `legend entries`.
pub fn entries(e: &Engine, axis: &Axis) -> Vec<LegendEntry> {
    if axis.legend.is_empty() {
        if let Some(list) = keys::value_tokens(e, "/pgfplots/legend entries") {
            return split(&list);
        }
    }
    axis.legend.clone()
}

pub fn legend(e: &mut Engine, axis: &Axis, f: &Frame) -> Result<(), Error> {
    // `legend to name`: kept for a `\ref` (that a single run of LaTeX leaves as ??), not drawn
    if !keys::value(e, "/pgfplots/legend to name").is_empty() {
        return Ok(());
    }
    let mut entries = entries(e, axis);
    if entries.is_empty() {
        return Ok(());
    }
    let mut images = axis.images.clone();
    if keys::flag(e, "/pgfplots/reverse legend") {
        let n = entries.len().min(images.len());
        entries.truncate(n);
        images.truncate(n);
        entries.reverse();
        images.reverse();
    }
    let plot_pos = keys::value(e, "/pgfplots/legend plot pos");
    e.begin_group();
    let r = (|| -> Result<(), Error> {
        e.st.mode = Default::default();
        e.st.pending = Transform::default();
        e.st.node = Default::default();
        // `legend style={legend pos=…}`: pgfplots applies the legend keys of the style directly,
        // and what they add to the legend style takes effect at once
        let mut direct = String::from("/pgfplots/legend style/.code={\\pgfkeysalso{#1}}");
        for key in ["legend pos", "area legend", "legend cell align", "legend plot pos", "transpose legend", "reverse legend", "legend image code", "legend image post style"] {
            direct.push_str(&format!(",/tikz/{key}/.code={{\\pgfkeysalso{{/pgfplots/{key}={{#1}}}}}}"));
        }
        e.process_keys(toks(&direct), None)?;
        e.process_keys(toks("/pgfplots/every axis legend"), Some("/tikz"))?;
        // `legend style={legend columns=-1}` is set by the style
        let columns = keys::number(e, "/pgfplots/legend columns").unwrap_or(1.0) as i64;
        let at = e.st.node.at.unwrap_or_else(|| f.desc(0.98, 0.98));
        let anchor = keys::value(e, "/tikz/anchor");
        let xsep = keys::length(e, "/pgf/inner xsep").unwrap_or(3.0 * PT);
        let ysep = keys::length(e, "/pgf/inner ysep").unwrap_or(2.0 * PT);
        let col_sep = keys::length(e, "/tikz/column sep").unwrap_or(0.0);
        let row_sep = keys::length(e, "/tikz/row sep").unwrap_or(0.0);
        let frame_mode = e.st.mode.clone();
        let fill = frame_mode.fill.then(|| super::draw::solid(e.st.fill_paint_color(), e.st.fill_opacity));
        let stroke = frame_mode.draw.then(|| e.stroke());
        let lw = e.st.line_width;
        // the cells, row by row
        let mut cells: Vec<Vec<Cell>> = Vec::new();
        let mut row: Vec<Cell> = Vec::new();
        let mut in_row = 0i64;
        let transpose = keys::flag(e, "/pgfplots/transpose legend");
        let mut items: Vec<Vec<Cell>> = Vec::new();
        for (k, entry) in entries.iter().enumerate() {
            let Some(image) = images.get(k) else { break };
            if tex::tokens_to_string(&entry.text).trim().is_empty() && entry.options.is_empty() {
                continue;
            }
            let img = image_cell(e, image)?;
            let txt = text_cell(e, entry)?;
            match plot_pos.as_str() {
                "right" => {
                    row.push(txt);
                    row.push(img);
                }
                "none" => row.push(txt),
                _ => {
                    row.push(img);
                    row.push(txt);
                }
            }
            if transpose {
                items.push(std::mem::take(&mut row));
                continue;
            }
            in_row += 1;
            if columns > 0 && in_row >= columns {
                cells.push(std::mem::take(&mut row));
                in_row = 0;
            }
        }
        if !row.is_empty() {
            cells.push(row);
        }
        // `transpose legend`: filled column by column, `legend columns` rows
        if transpose && !items.is_empty() {
            let rows = if columns > 0 { columns as usize } else { 1 };
            let cols = items.len().div_ceil(rows);
            for r in 0..rows.min(items.len()) {
                let mut line = Vec::new();
                for c in 0..cols {
                    if let Some(it) = items.get_mut(c * rows + r) {
                        line.append(it);
                    }
                }
                cells.push(line);
            }
        }
        if cells.is_empty() {
            return Ok(());
        }
        let ncols = cells.iter().map(Vec::len).max().unwrap_or(0);
        let mut left = vec![0.0f64; ncols];
        let mut right = vec![0.0f64; ncols];
        for r in &cells {
            for (j, c) in r.iter().enumerate() {
                left[j] = left[j].max(-c.lo.x);
                right[j] = right[j].max(c.hi.x);
            }
        }
        let heights: Vec<(f64, f64)> = cells.iter().map(|r| r.iter().fold((0.0f64, 0.0f64), |(u, d), c| (u.max(c.hi.y), d.max(-c.lo.y)))).collect();
        let total_w: f64 = (0..ncols).map(|j| left[j] + right[j]).sum::<f64>() + col_sep * ncols.saturating_sub(1) as f64;
        let total_h: f64 = heights.iter().map(|(u, d)| u + d).sum::<f64>() + row_sep * cells.len().saturating_sub(1) as f64;
        // the matrix node: its border is the cells' box plus the inner seps
        let (hw, hh) = (total_w / 2.0 + xsep, total_h / 2.0 + ysep);
        let outer = 0.5 * lw;
        let compass = |name: &str| -> Option<Point> {
            let (ow, oh) = (hw + outer, hh + outer);
            Some(match name.trim() {
                "center" => Point::new(0.0, 0.0),
                "north" => Point::new(0.0, oh),
                "south" => Point::new(0.0, -oh),
                "east" => Point::new(ow, 0.0),
                "west" => Point::new(-ow, 0.0),
                "north east" => Point::new(ow, oh),
                "north west" => Point::new(-ow, oh),
                "south east" => Point::new(ow, -oh),
                "south west" => Point::new(-ow, -oh),
                _ => return None,
            })
        };
        let a = compass(&anchor).unwrap_or_else(|| {
            e.warn(format!("legend anchor '{anchor}' not supported: center used"));
            Point::default()
        });
        let center = at.minus(a);
        let lo = Point::new(center.x - hw, center.y - hh);
        let hi = Point::new(center.x + hw, center.y + hh);
        if !e.canvas_frozen {
            e.canvas.include(lo.minus(Point::new(outer, outer)));
            e.canvas.include(hi.plus(Point::new(outer, outer)));
        }
        if fill.is_some() || stroke.is_some() {
            let segs = vec![Segment::MoveTo(lo), Segment::LineTo(Point::new(hi.x, lo.y)), Segment::LineTo(hi), Segment::LineTo(Point::new(lo.x, hi.y)), Segment::Close];
            e.canvas.push(Item::Path { segments: segs, fill, stroke });
        }
        // the cells in place
        let mut y = hi.y - ysep;
        for (r, row) in cells.into_iter().enumerate() {
            let (up, down) = heights[r];
            let oy = y - up;
            let mut x = lo.x + xsep;
            for (j, c) in row.into_iter().enumerate() {
                let ox = x + left[j];
                let d = Point::new(ox, oy);
                for mut it in c.items {
                    it.translate(d);
                    e.canvas.items.push(it);
                }
                x += left[j] + right[j] + col_sep;
            }
            y = oy - down - row_sep;
        }
        Ok(())
    })();
    e.end_group();
    r
}

/// The small picture of a plot, with its origin at (0,0).
fn image_cell(e: &mut Engine, spec: &[Token]) -> Result<Cell, Error> {
    let start = e.canvas.items.len();
    let (min, max) = (e.canvas.min.take(), e.canvas.max.take());
    e.begin_group();
    let r = image(e, spec);
    e.end_group();
    let (lo, hi) = (e.canvas.min, e.canvas.max);
    e.canvas.min = min;
    e.canvas.max = max;
    let items: Vec<Item> = e.canvas.items.drain(start..).collect();
    r?;
    Ok(match (lo, hi) {
        (Some(lo), Some(hi)) => Cell { items, lo, hi },
        _ => Cell { items, ..Cell::empty() },
    })
}

fn image(e: &mut Engine, spec: &[Token]) -> Result<(), Error> {
    e.st.mode = Default::default();
    e.st.pending = Transform::default();
    e.st.fill_color = Some(crate::color::Color::BLACK);
    let mut style = toks("/tikz/draw,/pgfplots/every axis plot except legend/.code={},");
    style.extend(spec.iter().cloned());
    style.extend(toks(",/pgfplots/every legend image post"));
    e.process_keys(style.clone(), Some("/pgfplots"))?;
    e.st.apply_pending();
    let kind = keys::value(e, "/pgfplots/@legend image");
    let handler = keys::value(e, "/tikz/@plot handler");
    let em = 10.0 * PT;
    let rect = |a: Point, b: Point| vec![Segment::MoveTo(a), Segment::LineTo(Point::new(b.x, a.y)), Segment::LineTo(b), Segment::LineTo(Point::new(a.x, b.y)), Segment::Close];
    match kind.as_str() {
        "empty" => {}
        "area" => e.use_path(rect(Point::new(0.0, -0.1), Point::new(0.6, 0.1)), "legend")?,
        "ybar" | "single ybar" | "xbar" | "single xbar" => {
            let vertical = kind.ends_with("ybar");
            let single = kind.starts_with("single");
            let w = if single { 6.0 * PT } else { 3.0 * PT };
            let mut segs = Vec::new();
            let bars: Vec<(f64, f64)> = if single { vec![(0.0, 0.8 * em)] } else { vec![(0.0, 0.8 * em), (2.0 * w, 0.6 * em)] };
            for (pos, len) in bars {
                if vertical {
                    let s = -0.2 * em;
                    segs.extend(rect(Point::new(pos - w / 2.0, s), Point::new(pos + w / 2.0, s + len)));
                } else {
                    let s = if single { 0.2 * 4.3 * PT } else { -0.2 * em };
                    segs.extend(rect(Point::new(0.0, pos - w / 2.0 + s), Point::new(len, pos + w / 2.0 + s)));
                }
            }
            e.use_path(segs, "legend")?;
        }
        "ybar interval" | "xbar interval" => {
            let s = -0.2 * em;
            let mut segs = Vec::new();
            for (x0, x1, h) in [(0.0, 5.0 * PT, 0.8 * em), (5.0 * PT, 10.0 * PT, 0.6 * em)] {
                let d = x1 - x0;
                let a = x0 + d * (0.5 - 0.35);
                if kind.starts_with('y') {
                    segs.extend(rect(Point::new(a, s), Point::new(a + 0.7 * d, s + h)));
                } else {
                    segs.extend(rect(Point::new(0.0, a + s), Point::new(h, a + 0.7 * d + s)));
                }
            }
            e.use_path(segs, "legend")?;
        }
        _ => {
            let pts = [Point::new(0.0, 0.0), Point::new(0.3, 0.0), Point::new(0.6, 0.0)];
            let bar = matches!(handler.as_str(), "ybar" | "xbar" | "ybar interval" | "xbar interval");
            if bar {
                // a bar plot with a line legend: pgf's bar handler on the three points
                let w = keys::length(e, "/pgf/bar width").unwrap_or(10.0 * PT);
                let mut segs = Vec::new();
                for p in pts {
                    segs.extend(rect(Point::new(p.x - w / 2.0, 0.0), Point::new(p.x + w / 2.0, p.y)));
                }
                e.use_path(segs, "legend")?;
            } else if !e.st.only_marks {
                let segs = vec![Segment::MoveTo(pts[0]), Segment::LineTo(pts[1]), Segment::LineTo(pts[2])];
                e.use_path(segs, "legend")?;
            }
            if let Some(mark) = e.st.plot_mark.clone() {
                // the plot's mark size, not one of `every mark` (as for the plot's marks)
                let size = e.st.mark_size;
                e.st.mode = Default::default();
                e.st.pending = Transform::default();
                e.process_keys(toks("every mark"), Some("/tikz"))?;
                let local = e.st.pending;
                visual::draw_mark_set(e, &mark, size, &local, std::iter::once(pts[1]))?;
            }
        }
    }
    Ok(())
}

/// The text of an entry: a node with anchor `center` (from `cells`) at the origin.
fn text_cell(e: &mut Engine, entry: &LegendEntry) -> Result<Cell, Error> {
    let mut options = toks("/tikz/every cell,");
    options.extend(entry.options.iter().cloned());
    let text = super::draw::print_numbers(e, &entry.text)?;
    let start = e.canvas.items.len();
    let (min, max) = (e.canvas.min.take(), e.canvas.max.take());
    let o = Point::default();
    let r = e.node(super::super::nodes::NodeSpec {
        options,
        name: None,
        at: Some(o),
        text,
        place: super::super::nodes::OnPath::Point(o),
        point: o,
        coordinate: false,
        default_pos: None,
    });
    let (lo, hi) = (e.canvas.min, e.canvas.max);
    e.canvas.min = min;
    e.canvas.max = max;
    let mut items: Vec<Item> = e.canvas.items.drain(start..).collect();
    items.extend(r?);
    Ok(match (lo, hi) {
        (Some(lo), Some(hi)) => Cell { items, lo, hi },
        _ => Cell { items, ..Cell::empty() },
    })
}

/// `legend entries={a,b,{c,d}}`; `{0,...,20}` as `\foreach` reads it.
fn split(list: &[Token]) -> Vec<LegendEntry> {
    if tex::tokens_to_string(list).contains("...") {
        return super::super::foreach_items(list).into_iter().map(|v| LegendEntry { options: Vec::new(), text: tex::strip_outer_braces(v) }).collect();
    }
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut depth = 0usize;
    for t in list {
        match t {
            Token::Char(_, tex::Cat::Begin) => depth += 1,
            Token::Char(_, tex::Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(',', _) if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(t.clone());
    }
    out.push(cur);
    out.into_iter()
        .map(|v| {
            // `{[text width=25pt]Neg. Sign:}`: the braces of the list item go first
            let (options, text) = super::split_options(tex::strip_outer_braces(crate::keys::trim(&v).to_vec()));
            LegendEntry { options, text: tex::strip_outer_braces(text) }
        })
        .collect()
}
