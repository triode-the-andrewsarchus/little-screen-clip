use super::{MAP_COLUMNS, MAP_ROWS, Pos2, Rect, cell_boundary, map_cell, map_cell_rect};
use std::ops::Range;

#[derive(Default)]
pub(super) struct Eraser {
    pub(super) active: bool,
    pub(super) cell: Option<usize>,
}

impl Eraser {
    pub(super) fn update(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        visible: Rect,
    ) -> Option<Vec<Range<u32>>> {
        let secondary = egui::PointerButton::Secondary;
        let (pressed, down, point) = ui.input(|input| {
            (
                input.pointer.button_pressed(secondary) && response.is_pointer_button_down_on(),
                input.focused && input.pointer.button_down(secondary),
                input.pointer.latest_pos().filter(|_| input.focused),
            )
        });
        if !self.active && !pressed && !response.secondary_clicked() {
            return None;
        }

        let cell = point
            .filter(|point| visible.contains(*point))
            .map(|point| map_cell(response.rect, point));
        let previous = self.cell;
        self.active = down;
        self.cell = cell.filter(|_| down);
        Some(match cell {
            Some(cell) if previous != Some(cell) => stroke(previous.unwrap_or(cell), cell),
            _ => Vec::new(),
        })
    }
}

fn stroke(from: usize, to: usize) -> Vec<Range<u32>> {
    let start = cell_indices(from);
    let end = cell_indices(to);
    let distance = end - start;
    let steps = distance.abs().max_elem() as usize;
    (0..=steps)
        .flat_map(|step| {
            let point = start + distance * (step as f32 / steps.max(1) as f32);
            let (x, y) = (point.x.round() as usize, point.y.round() as usize);
            let columns = neighbors(x, MAP_COLUMNS);
            neighbors(y, MAP_ROWS).map(move |row| {
                let base = row * MAP_COLUMNS;
                cell_boundary(base + columns.start)..cell_boundary(base + columns.end)
            })
        })
        .collect()
}

pub(super) fn outline(rect: Rect, cell: usize) -> Rect {
    let columns = neighbors(cell % MAP_COLUMNS, MAP_COLUMNS);
    let rows = neighbors(cell / MAP_COLUMNS, MAP_ROWS);
    let first = rows.start * MAP_COLUMNS + columns.start;
    let last = (rows.end - 1) * MAP_COLUMNS + columns.end - 1;
    map_cell_rect(rect, first).union(map_cell_rect(rect, last))
}

fn cell_indices(cell: usize) -> Pos2 {
    Pos2::new((cell % MAP_COLUMNS) as f32, (cell / MAP_COLUMNS) as f32)
}

fn neighbors(index: usize, count: usize) -> Range<usize> {
    index.saturating_sub(1)..(index + 2).min(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_covers_three_cells_per_row_and_clips_at_map_edges() {
        let center = 5 * MAP_COLUMNS + 5;
        let ranges = stroke(center, center);
        assert_eq!(ranges.len(), 3);
        for (row, range) in (4..=6).zip(ranges) {
            let base = row * MAP_COLUMNS;
            assert_eq!(range, cell_boundary(base + 4)..cell_boundary(base + 7));
        }
        assert_eq!(
            stroke(0, 0),
            [
                0..cell_boundary(2),
                cell_boundary(MAP_COLUMNS)..cell_boundary(MAP_COLUMNS + 2)
            ]
        );
    }
}
