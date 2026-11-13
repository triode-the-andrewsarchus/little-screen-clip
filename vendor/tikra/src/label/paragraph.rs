//! TeX's line breaking (Knuth and Plass): the breaks of a paragraph of boxes, glue and penalties
//! with the fewest demerits, in two passes as LaTeX sets them (first without hyphenating, within
//! `\pretolerance`; then with the hyphens, within `\tolerance`, overfull if it must).

/// An item of the paragraph; widths in any unit, the same for all.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Box(f32),
    /// Glue, its stretch finite or infinite (`fil`).
    Glue {
        width: f32,
        stretch: f32,
        fil: f32,
        shrink: f32,
    },
    /// A place to break, at `penalty`; `width` is what the break adds to the line (a hyphen);
    /// `flagged`, a hyphen's; `hyphen`, one that hyphenation found, which the first pass ignores.
    Penalty {
        width: f32,
        penalty: f32,
        flagged: bool,
        hyphen: bool,
    },
}

/// Glue at the side of each line (`\leftskip`, `\rightskip`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Skip {
    pub width: f32,
    pub stretch: f32,
    pub fil: f32,
}

/// A line: the items from `start` to `end` (the break, excluded), and how much its glue stretches
/// (`ratio` > 0, of the finite stretch) or shrinks (< 0, of the shrink).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    pub ratio: f32,
    /// The line ends at a hyphen that the break adds.
    pub hyphen: bool,
}

const INF_BAD: f64 = 10000.0;
const LINE_PENALTY: f64 = 10.0;
const ADJ_DEMERITS: f64 = 10000.0;
const DOUBLE_HYPHEN_DEMERITS: f64 = 10000.0;
const FINAL_HYPHEN_DEMERITS: f64 = 5000.0;
const PRETOLERANCE: f64 = 100.0;
// `\sloppy` (with an emergency stretch of 3 em), as the `standalone` class sets it with `varwidth`,
// where TikZ pictures are often drawn;
// LaTeX's default is 200
const TOLERANCE: f64 = 9999.0;

/// TeX's badness of glue that stretches (or shrinks) by `t` where it can by `s`: about 100 (t/s)³.
fn badness(t: f64, s: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if s <= 0.0 {
        return INF_BAD;
    }
    let r = (t * 297.0 / s).floor();
    if r > 1290.0 {
        INF_BAD
    } else {
        ((r * r * r + 131072.0) / 262144.0).floor()
    }
}

#[derive(Clone, Copy)]
struct Node {
    /// the break's item; `usize::MAX` for the paragraph's start
    at: usize,
    fitness: u8,
    total: f64,
    prev: Option<usize>,
    flagged: bool,
    ratio: f32,
}

/// Breaks the items into lines `hsize` wide, between `left` and `right`; the last item must be a
/// forced break (a penalty of -10000). `emergency`: the stretch that the last pass adds to every
/// line (`\emergencystretch`, in the items' unit).
pub fn break_lines(items: &[Item], hsize: f32, left: Skip, right: Skip, emergency: f32) -> Vec<Line> {
    try_pass(items, hsize, left, right, PRETOLERANCE, Pass::First)
        .or_else(|| try_pass(items, hsize, left, right, TOLERANCE, if emergency > 0.0 { Pass::Second } else { Pass::Final(0.0) }))
        .or_else(|| if emergency > 0.0 { try_pass(items, hsize, left, right, TOLERANCE, Pass::Final(emergency)) } else { None })
        .unwrap_or_default()
}

/// TeX's passes: without hyphens; with them; with them and the emergency stretch, the last,
/// which takes overfull lines when it must.
#[derive(Clone, Copy, PartialEq)]
enum Pass {
    First,
    Second,
    Final(f32),
}

fn try_pass(items: &[Item], hsize: f32, left: Skip, right: Skip, threshold: f64, pass: Pass) -> Option<Vec<Line>> {
    let second = pass != Pass::First;
    let (last_pass, background) = match pass {
        Pass::Final(e) => (true, e as f64),
        _ => (false, 0.0),
    };
    // sums of widths, stretch, fil and shrink before each item
    let n = items.len();
    let (mut w, mut st, mut fil, mut sh) = (vec![0.0f64; n + 1], vec![0.0f64; n + 1], vec![0.0f64; n + 1], vec![0.0f64; n + 1]);
    for (k, item) in items.iter().enumerate() {
        let (dw, ds, df, dk) = match *item {
            Item::Box(x) => (x, 0.0, 0.0, 0.0),
            Item::Glue { width, stretch, fil, shrink } => (width, stretch, fil, shrink),
            Item::Penalty { .. } => (0.0, 0.0, 0.0, 0.0),
        };
        w[k + 1] = w[k] + dw as f64;
        st[k + 1] = st[k] + ds as f64;
        fil[k + 1] = fil[k] + df as f64;
        sh[k + 1] = sh[k] + dk as f64;
    }
    // where the line after a break starts: its first box (glue and penalties after a break go)
    let start_after = |at: usize| -> usize {
        let mut s = if at == usize::MAX { 0 } else { at + 1 };
        while s < n && !matches!(items[s], Item::Box(_)) {
            s += 1;
        }
        s
    };
    let mut nodes = vec![Node { at: usize::MAX, fitness: 2, total: 0.0, prev: None, flagged: false, ratio: 0.0 }];
    let mut active = vec![0usize];
    for b in 0..n {
        let (pen, pen_width, flagged) = match items[b] {
            Item::Glue { .. } if b > 0 && matches!(items[b - 1], Item::Box(_)) => (0.0, 0.0, false),
            Item::Penalty { penalty, width, flagged, hyphen } if penalty < 10000.0 && (second || !hyphen) => (penalty as f64, width as f64, flagged),
            _ => continue,
        };
        let forced = pen <= -10000.0;
        let last = b == n - 1;
        // the best line to here from an active node, by fitness class
        let mut best: [Option<(f64, usize, f32)>; 4] = [None; 4];
        let mut kept = Vec::with_capacity(active.len());
        let mut dropped: Option<(f64, usize, f32)> = None;
        for (i, &a) in active.iter().enumerate() {
            let node = nodes[a];
            let s = start_after(node.at);
            let natural = if s <= b { w[b] - w[s] } else { 0.0 } + pen_width + (left.width + right.width) as f64;
            let shortfall = hsize as f64 - natural;
            let (stretch, fils, shrink) = if s <= b {
                (st[b] - st[s] + (left.stretch + right.stretch) as f64, fil[b] - fil[s] + (left.fil + right.fil) as f64, sh[b] - sh[s])
            } else {
                ((left.stretch + right.stretch) as f64, (left.fil + right.fil) as f64, 0.0)
            };
            let (bad, fitness, ratio, overfull) = if shortfall >= 0.0 {
                if fils > 0.0 {
                    (0.0, 2u8, 0.0, false)
                } else {
                    // (the emergency stretch counts for the badness, not for how the glue is set)
                    let bad = badness(shortfall, stretch + background);
                    let f = if bad > 99.0 {
                        0
                    } else if bad > 12.0 {
                        1
                    } else {
                        2
                    };
                    (bad, f, if stretch > 0.0 { (shortfall / stretch) as f32 } else { 0.0 }, false)
                }
            } else if -shortfall > shrink {
                (INF_BAD + 1.0, 3, -1.0, true)
            } else {
                let bad = badness(-shortfall, shrink);
                (bad, if bad > 12.0 { 3 } else { 2 }, (shortfall / shrink) as f32, false)
            };
            // a node from which the line is too long now stays so
            if !(overfull || forced) {
                kept.push(a);
            }
            if bad > threshold {
                // the final pass takes a line too long only from the one active node left: the
                // others before it went, and none comes after (TeX's artificial demerits)
                let alone = kept.is_empty() && i + 1 == active.len() && best.iter().all(Option::is_none);
                if last_pass && (overfull || forced) && alone {
                    dropped = Some((node.total, a, ratio));
                }
                continue;
            }
            let mut d = LINE_PENALTY + bad;
            d = if d >= 10000.0 { 1e8 } else { d * d };
            if pen > 0.0 {
                d += pen * pen;
            } else if pen > -10000.0 {
                d -= pen * pen;
            }
            if node.flagged {
                if flagged {
                    d += DOUBLE_HYPHEN_DEMERITS;
                } else if last {
                    d += FINAL_HYPHEN_DEMERITS;
                }
            }
            if (fitness as i32 - node.fitness as i32).abs() > 1 {
                d += ADJ_DEMERITS;
            }
            let total = node.total + d;
            // (of equal ones, the latest line start, as TeX keeps)
            let slot = &mut best[fitness as usize];
            if slot.is_none_or(|s| total <= s.0) {
                *slot = Some((total, a, ratio));
            }
        }
        active = kept;
        // the last resort of the final pass: when no line fits, the least bad overfull one
        if best.iter().all(Option::is_none) && active.is_empty() {
            if let Some((total, a, ratio)) = dropped {
                best[3] = Some((total, a, ratio));
            }
        }
        for (fitness, slot) in best.iter().enumerate() {
            if let Some((total, a, ratio)) = *slot {
                nodes.push(Node { at: b, fitness: fitness as u8, total, prev: Some(a), flagged, ratio });
                active.push(nodes.len() - 1);
            }
        }
        if active.is_empty() {
            return None;
        }
    }
    // the best way to the paragraph's end
    let end = active.iter().copied().filter(|&a| nodes[a].at == n - 1).min_by(|&a, &b| nodes[a].total.total_cmp(&nodes[b].total))?;
    let mut lines = Vec::new();
    let mut k = end;
    while let Some(p) = nodes[k].prev {
        let at = nodes[k].at;
        let hyphen = matches!(items[at], Item::Penalty { width, .. } if width > 0.0);
        lines.push(Line { start: start_after(nodes[p].at), end: at, ratio: nodes[k].ratio, hyphen });
        k = p;
    }
    lines.reverse();
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(ws: &[f32]) -> Vec<Item> {
        let mut items = Vec::new();
        for (k, &x) in ws.iter().enumerate() {
            if k > 0 {
                items.push(Item::Glue { width: 3.0, stretch: 0.0, fil: 0.0, shrink: 0.0 });
            }
            items.push(Item::Box(x));
        }
        items.push(Item::Penalty { width: 0.0, penalty: 10000.0, flagged: false, hyphen: false });
        items.push(Item::Glue { width: 0.0, stretch: 0.0, fil: 1.0, shrink: 0.0 });
        items.push(Item::Penalty { width: 0.0, penalty: -10000.0, flagged: false, hyphen: false });
        items
    }

    #[test]
    fn ragged_lines() {
        let items = words(&[10.0, 10.0, 10.0, 10.0]);
        let right = Skip { width: 0.0, stretch: 20.0, fil: 0.0 };
        let lines = break_lines(&items, 25.0, Skip::default(), right, 0.0);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!((lines[0].start, lines[0].end), (0, 3));
    }

    /// A word exactly as wide as the line fills it (badness 0); a break at a penalty right after
    /// a break at glue would make a line with no box, which is not taken; the line after a break
    /// starts at its first box, past the glue and penalties.
    #[test]
    fn breaks_next_to_each_other() {
        let glue = Item::Glue { width: 3.0, stretch: 0.0, fil: 0.0, shrink: 0.0 };
        let items = [
            Item::Box(10.0),
            glue,
            Item::Penalty { width: 0.0, penalty: 0.0, flagged: false, hyphen: false },
            glue,
            Item::Box(10.0),
            Item::Penalty { width: 0.0, penalty: 10000.0, flagged: false, hyphen: false },
            Item::Glue { width: 0.0, stretch: 0.0, fil: 1.0, shrink: 0.0 },
            Item::Penalty { width: 0.0, penalty: -10000.0, flagged: false, hyphen: false },
        ];
        let lines = break_lines(&items, 10.0, Skip::default(), Skip::default(), 0.0);
        let spans: Vec<(usize, usize)> = lines.iter().map(|l| (l.start, l.end)).collect();
        assert_eq!(spans, [(0, 1), (4, 7)], "{lines:?}");
        assert_eq!(lines[0].ratio, 0.0);
    }
}
