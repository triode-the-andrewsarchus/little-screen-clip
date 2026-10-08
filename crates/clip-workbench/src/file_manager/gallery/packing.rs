//! Five-geometry row candidates with bounded, memoized lookahead.
use std::{collections::HashMap, sync::Arc};

const BEAM: usize = 24;
const LOOKAHEAD: usize = 2;
const SHAPES: [(usize, usize); 4] = [(2, 2), (3, 2), (4, 2), (2, 4)];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tile {
    pub item: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Layout {
    pub tiles: Vec<Tile>,
    pub height: usize,
}
#[derive(Clone, Default)]
struct Row {
    tiles: Vec<Tile>,
    next: usize,
    outgoing: u128,
    height: usize,
    crop: f32,
    cost: f32,
}

pub(super) fn crop_loss(source: f32, width: usize, height: usize) -> f32 {
    let tile = width as f32 / height as f32;
    1.0 - (source / tile).min(tile / source)
}

#[cfg(test)]
pub(super) fn pack(aspects: &[f32], width: usize) -> Layout {
    pack_chunk(aspects, width, true)
}
pub(super) fn pack_chunk(aspects: &[f32], width: usize, final_chunk: bool) -> Layout {
    pack_with_lookahead(aspects, width, LOOKAHEAD, final_chunk)
}
fn pack_with_lookahead(
    aspects: &[f32],
    width: usize,
    lookahead: usize,
    final_chunk: bool,
) -> Layout {
    let mut layout = Layout::default();
    let (mut index, mut incoming) = (0, 0);
    let mut memo = HashMap::new();
    let mut rows_cache = HashMap::new();
    while index < aspects.len() {
        let rows = cached_candidates(
            aspects,
            index,
            incoming,
            width,
            final_chunk,
            &mut rows_cache,
        );
        let Some(row) = rows.iter().min_by(|a, b| {
            let mut cost = |row: &Row| {
                row.cost
                    + future(
                        aspects,
                        row.next,
                        row.outgoing,
                        width,
                        final_chunk,
                        lookahead - 1,
                        &mut memo,
                        &mut rows_cache,
                    )
            };
            cost(a).total_cmp(&cost(b))
        }) else {
            // Width-aligned batches always have this gap-free fallback if
            // beam pruning discarded a more crop-efficient continuation.
            return square_placeholder(aspects.len(), width);
        };
        layout.tiles.extend(row.tiles.iter().map(|tile| Tile {
            y: layout.height,
            ..*tile
        }));
        layout.height += row.height;
        index = row.next;
        incoming = row.outgoing;
    }
    layout
}

fn future(
    aspects: &[f32],
    index: usize,
    incoming: u128,
    width: usize,
    final_chunk: bool,
    depth: usize,
    memo: &mut HashMap<(usize, u128, usize), f32>,
    rows_cache: &mut HashMap<(usize, u128), Arc<[Row]>>,
) -> f32 {
    if index == aspects.len() {
        return if incoming == 0 { 0.0 } else { f32::INFINITY };
    }
    if depth == 0 {
        return 0.0;
    }
    let key = (index, incoming, depth);
    if let Some(cost) = memo.get(&key) {
        return *cost;
    }
    let cost = cached_candidates(aspects, index, incoming, width, final_chunk, rows_cache)
        .iter()
        .map(|row| {
            row.cost
                + future(
                    aspects,
                    row.next,
                    row.outgoing,
                    width,
                    final_chunk,
                    depth - 1,
                    memo,
                    rows_cache,
                )
        })
        .fold(f32::INFINITY, f32::min);
    memo.insert(key, cost);
    cost
}

fn cached_candidates(
    aspects: &[f32],
    index: usize,
    incoming: u128,
    width: usize,
    final_chunk: bool,
    cache: &mut HashMap<(usize, u128), Arc<[Row]>>,
) -> Arc<[Row]> {
    cache
        .entry((index, incoming))
        .or_insert_with(|| candidates(aspects, index, incoming, width, final_chunk).into())
        .clone()
}

fn candidates(
    aspects: &[f32],
    start: usize,
    incoming: u128,
    width: usize,
    final_chunk: bool,
) -> Vec<Row> {
    if incoming == (1_u128 << width) - 1 {
        // A completely occupied continuation row is legal inside the chunk.
        // Terminal scoring still rejects it if no later input images remain.
        return vec![Row {
            next: start,
            height: 2,
            ..Default::default()
        }];
    }
    let mut buckets = vec![Vec::<Row>::new(); width + 1];
    buckets[0].push(Row {
        next: start,
        height: 2,
        ..Default::default()
    });
    let mut rows = Vec::new();
    for x in 0..=width {
        let states = prune(std::mem::take(&mut buckets[x]));
        for state in states {
            if x % 2 == 0
                && state.next > start
                && (state.next < aspects.len() || state.outgoing == 0)
            {
                rows.push(score(state.clone(), incoming, width, aspects.len()));
            }
            if x == width {
                continue;
            }
            if incoming & (1 << x) != 0 {
                buckets[x + 1].push(state);
            } else if state.next < aspects.len() {
                extend(aspects, x, incoming, width, state, &mut buckets);
            }
        }
    }
    if incoming == 0 {
        rows.extend(portrait_rows(aspects, start, width));
    }
    // Keep distinct continuations, rather than allowing equally shaped rows to
    // crowd out alternatives the next row needs for a clean ending.
    rows.retain(|row| legal_row(row, incoming, width, aspects.len(), final_chunk));
    prune(rows)
}

fn legal_row(row: &Row, incoming: u128, width: usize, count: usize, final_chunk: bool) -> bool {
    let filled =
        incoming.count_ones() as usize + row.tiles.iter().map(|tile| tile.width).sum::<usize>();
    if filled != width {
        // Only the actual folder ending can have trailing unused columns.
        return final_chunk && row.next == count && incoming == 0 && row.outgoing == 0;
    }
    let remaining = count - row.next;
    if row.outgoing == 0 {
        return final_chunk || remaining == 0 || remaining >= width.div_ceil(4);
    }
    // There must be enough images to fill every region around the descenders.
    // A following ordinary full row is always possible with 2/3/4-wide tiles.
    let mut minimum = 0;
    let mut run = 0usize;
    for x in 0..=width {
        if x < width && row.outgoing & (1 << x) == 0 {
            run += 1;
        } else {
            minimum += run.div_ceil(4);
            run = 0;
        }
    }
    let maximum = (width - row.outgoing.count_ones() as usize) / 2;
    remaining >= minimum && (remaining <= maximum || remaining >= minimum + width.div_ceil(4))
}

fn extend(
    aspects: &[f32],
    x: usize,
    incoming: u128,
    width: usize,
    state: Row,
    buckets: &mut [Vec<Row>],
) {
    for (w, h) in SHAPES {
        if x + w > width || (h == 4 && x % 2 != 0) {
            continue;
        }
        let mask = ((1_u128 << w) - 1) << x;
        if incoming & mask != 0 {
            continue;
        }
        let mut next = state.clone();
        next.tiles.push(Tile {
            item: state.next,
            x,
            y: 0,
            width: w,
            height: h,
        });
        next.crop += crop_loss(aspects[state.next], w, h);
        next.next += 1;
        if h == 4 {
            next.outgoing |= mask;
        }
        buckets[x + w].push(next);
    }
}

fn portrait_rows(aspects: &[f32], start: usize, width: usize) -> Vec<Row> {
    let mut row = Row {
        next: start,
        height: 3,
        ..Default::default()
    };
    let mut rows = Vec::new();
    for x in (0..width).step_by(2) {
        if row.next == aspects.len() {
            break;
        }
        row.tiles.push(Tile {
            item: row.next,
            x,
            y: 0,
            width: 2,
            height: 3,
        });
        row.crop += crop_loss(aspects[row.next], 2, 3);
        row.next += 1;
        rows.push(score(row.clone(), 0, width, aspects.len()));
    }
    rows
}

fn score(mut row: Row, incoming: u128, width: usize, count: usize) -> Row {
    let occupied = incoming.count_ones() as usize * 2
        + row
            .tiles
            .iter()
            .map(|tile| tile.width * tile.height.min(row.height))
            .sum::<usize>();
    let empty = (width * row.height - occupied) as f32;
    let waste_cost = if row.next == count {
        // Compare ending emptiness as a fraction, rather than charging per
        // missing square: a lone portrait must not turn landscape simply
        // because there are many empty columns in a wide viewport.
        empty / (width * row.height) as f32 * 1.4
    } else {
        empty / 4.0 * 0.65
    };
    row.cost = row.crop + waste_cost + 0.35;
    row
}

fn prune(rows: Vec<Row>) -> Vec<Row> {
    let mut best = HashMap::<(usize, u128), Row>::new();
    for row in rows {
        let key = (row.next, row.outgoing);
        let replace = best
            .get(&key)
            .is_none_or(|old| row.crop + row.cost < old.crop + old.cost);
        if replace {
            best.insert(key, row);
        }
    }
    let mut rows: Vec<_> = best.into_values().collect();
    rows.sort_by(|a, b| {
        (a.crop + a.cost)
            .total_cmp(&(b.crop + b.cost))
            .then_with(|| b.next.cmp(&a.next))
            .then_with(|| a.outgoing.cmp(&b.outgoing))
    });
    rows.truncate(BEAM);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookahead_finds_a_cleaner_ending_than_single_row_greedy() {
        let score = |layout: &Layout, aspects: &[f32]| {
            layout
                .tiles
                .iter()
                .map(|tile| crop_loss(aspects[tile.item], tile.width, tile.height))
                .sum::<f32>()
                + (layout.height * 8
                    - layout
                        .tiles
                        .iter()
                        .map(|tile| tile.width * tile.height)
                        .sum::<usize>()) as f32
                    * 0.4
        };
        let mut improved = false;
        for pattern in 0..256 {
            let aspects: Vec<_> = (0..8)
                .map(|i| if pattern & (1 << i) == 0 { 1.5 } else { 0.5 })
                .collect();
            let greedy = pack_with_lookahead(&aspects, 8, 1, true);
            let lookahead = pack(&aspects, 8);
            if score(&lookahead, &aspects) + 0.1 < score(&greedy, &aspects) {
                improved = true;
                break;
            }
        }
        assert!(improved);
    }
}

pub(super) fn square_placeholder(count: usize, width: usize) -> Layout {
    let columns = width.max(2) / 2;
    Layout {
        height: count.div_ceil(columns) * 2,
        tiles: (0..count)
            .map(|item| Tile {
                item,
                x: item % columns * 2,
                y: item / columns * 2,
                width: 2,
                height: 2,
            })
            .collect(),
    }
}
