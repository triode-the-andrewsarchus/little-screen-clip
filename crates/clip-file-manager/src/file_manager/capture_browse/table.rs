use crate::{
    file_manager::table_body::{ROW_HEIGHT, TableBody, paint_cell},
    ui_appearance::WorkbenchColors,
};
use egui::{Color32, Rect, ScrollArea, Sense, Ui, pos2, vec2};
use std::{cmp::Ordering, ops::Range};

pub(in crate::file_manager) struct Page<'a> {
    pub offset: u64,
    pub count: usize,
    pub selected: Option<u64>,
    pub selection: Option<&'a Range<u64>>,
    pub scroll_to: Option<usize>,
    pub generation: u64,
    pub height: f32,
}

pub(in crate::file_manager) struct Response {
    pub visible: Range<u64>,
    pub clicked: Option<(u64, bool)>,
    pub sort: Option<Sort>,
    pub scroll_offset: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::file_manager) struct Sort {
    pub column: usize,
    pub descending: bool,
}

impl Sort {
    pub fn compare<T: Ord>(self, left: T, right: T) -> Ordering {
        self.direction(left.cmp(&right))
    }

    pub fn known_first<T: Ord>(self, left: Option<T>, right: Option<T>) -> Ordering {
        match (left, right) {
            (Some(left), Some(right)) => self.compare(left, right),
            (left, right) => right.is_some().cmp(&left.is_some()),
        }
    }

    fn direction(self, order: Ordering) -> Ordering {
        if self.descending {
            order.reverse()
        } else {
            order
        }
    }
}

#[derive(Default)]
pub(in crate::file_manager) struct RowOrder {
    pub sort: Option<Sort>,
    positions: Vec<usize>,
}

impl RowOrder {
    pub fn rebuild<T>(&mut self, source: &[T], compare: impl Fn(Sort, &T, &T) -> Ordering) {
        self.positions.clear();
        if let Some(sort) = self.sort {
            self.positions.extend(0..source.len());
            // Original positions break ties in both directions.
            self.positions.sort_unstable_by(|&left, &right| {
                compare(sort, &source[left], &source[right]).then(left.cmp(&right))
            });
        }
    }

    pub fn source_index(&self, display_index: usize) -> usize {
        self.positions
            .get(display_index)
            .copied()
            .unwrap_or(display_index)
    }

    pub fn display_index(&self, source_index: usize) -> usize {
        self.positions
            .iter()
            .position(|&index| index == source_index)
            .unwrap_or(source_index)
    }
}

#[derive(Default)]
pub(in crate::file_manager) struct TableOptions {
    pub sortable_columns: usize,
    pub sort: Option<Sort>,
    pub scroll_offset: Option<f32>,
}

pub(in crate::file_manager) struct Columns<const N: usize> {
    pub id: &'static str,
    pub headings: [&'static str; N],
    pub fractions: [f32; N],
    pub min_height: f32,
    pub selected_fill: f32,
    pub hover_fill: f32,
}

impl<const N: usize> Columns<N> {
    pub fn show<T: Copy>(
        &self,
        ui: &mut Ui,
        page: Page<'_>,
        colors: WorkbenchColors,
        read: impl Fn(u64) -> Option<T>,
        ordinal: impl Fn(T) -> u64,
        paint: impl Fn(&Ui, Rect, T),
    ) -> Response {
        self.show_with_options(
            ui,
            (page, TableOptions::default()),
            colors,
            read,
            ordinal,
            paint,
        )
    }

    pub fn show_with_options<T: Copy>(
        &self,
        ui: &mut Ui,
        (page, options): (Page<'_>, TableOptions),
        colors: WorkbenchColors,
        read: impl Fn(u64) -> Option<T>,
        ordinal: impl Fn(T) -> u64,
        paint: impl Fn(&Ui, Rect, T),
    ) -> Response {
        let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(0.0);
        let heading = ui
            .allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover())
            .0;
        let sort = if options.sortable_columns > 0 {
            self.sort_headers(ui, heading, options.sort, options.sortable_columns)
        } else {
            self.cells(ui, heading, self.headings, colors.text_muted);
            None
        };
        let mut area = ScrollArea::vertical()
            .id_salt((self.id, page.offset, page.generation))
            .auto_shrink([false, false])
            .max_height((page.height - ROW_HEIGHT).max(self.min_height));
        if let Some(offset) = options.scroll_offset {
            area = area.vertical_scroll_offset(offset);
        } else if let Some(row) = page.scroll_to {
            area = area
                .vertical_scroll_offset(row as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y));
        }
        let scroll = TableBody {
            rows: page.offset..page.offset + page.count as u64,
            width,
            selected: page.selected,
            selected_range: page.selection,
            selected_fill: self.selected_fill,
            hover_fill: self.hover_fill,
        }
        .show(
            ui,
            area,
            colors,
            |index| {
                let row = read(index);
                (row.map(&ordinal), row)
            },
            |ui, rect, index, row| {
                if let Some(row) = row {
                    paint(ui, rect, row);
                } else {
                    let number = (index + 1).to_string();
                    let mut pending = [""; N];
                    for (slot, value) in pending.iter_mut().zip([number.as_str(), "…"]) {
                        *slot = value;
                    }
                    self.cells(ui, rect, pending, colors.text_muted);
                }
                None
            },
        );
        Response {
            visible: scroll.inner.visible,
            clicked: scroll.inner.clicked,
            sort,
            scroll_offset: scroll.state.clamped_offset().y,
        }
    }

    fn sort_headers(
        &self,
        ui: &mut Ui,
        rect: Rect,
        active: Option<Sort>,
        sortable_columns: usize,
    ) -> Option<Sort> {
        let mut left = rect.left();
        let mut next = None;
        for (column, (label, fraction)) in self.headings.into_iter().zip(self.fractions).enumerate()
        {
            let right = left + fraction * rect.width();
            let cell = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
            left = right;
            if column >= sortable_columns {
                ui.put(
                    cell,
                    egui::Label::new(egui::RichText::new(label).monospace().weak()),
                );
                continue;
            }
            let active = active.filter(|sort| sort.column == column);
            let name = active.map_or_else(
                || label.to_owned(),
                |sort| {
                    let arrow = if sort.descending { "↓" } else { "↑" };
                    format!("{label} {arrow}")
                },
            );
            let response = ui.put(
                cell.shrink2(vec2(1.0, 0.0)),
                egui::Button::new(name)
                    .small()
                    .truncate()
                    .selected(active.is_some()),
            );
            if response.on_hover_text(format!("Sort by {label}")).clicked() {
                next = Some(Sort {
                    column,
                    descending: active.is_some_and(|sort| !sort.descending),
                });
            }
        }
        next
    }

    pub fn cells(&self, ui: &Ui, rect: Rect, values: [&str; N], color: Color32) {
        let mut left = rect.left();
        for (value, fraction) in values.into_iter().zip(self.fractions) {
            let right = left + fraction * rect.width();
            let cell = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
            paint_cell(ui, cell, value, color);
            left = right;
        }
    }
}
