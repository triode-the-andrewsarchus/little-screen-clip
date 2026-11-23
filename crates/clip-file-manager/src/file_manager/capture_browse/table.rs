use crate::ui_appearance::WorkbenchColors;
use egui::{Color32, FontId, Rect, ScrollArea, Sense, Ui, pos2, text::LayoutJob, vec2};
use std::ops::Range;

const ROW_HEIGHT: f32 = 21.0;

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
        let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(0.0);
        let heading = ui
            .allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover())
            .0;
        self.cells(ui, heading, self.headings, colors.text_muted);
        let mut area = ScrollArea::vertical()
            .id_salt((self.id, page.offset, page.generation))
            .auto_shrink([false, false])
            .max_height((page.height - ROW_HEIGHT).max(self.min_height));
        if let Some(row) = page.scroll_to {
            area = area
                .vertical_scroll_offset(row as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y));
        }
        let mut output = Response {
            visible: page.offset..page.offset,
            clicked: None,
        };
        area.show_rows(ui, ROW_HEIGHT, page.count, |ui, rows| {
            output.visible = page.offset + rows.start as u64..page.offset + rows.end as u64;
            for index in output.visible.clone() {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
                if index % 2 == 1 {
                    ui.painter().rect_filled(rect, 0.0, colors.panel);
                }
                let Some(row) = read(index) else {
                    let number = (index + 1).to_string();
                    let mut pending = [""; N];
                    for (slot, value) in pending.iter_mut().zip([number.as_str(), "…"]) {
                        *slot = value;
                    }
                    self.cells(ui, rect, pending, colors.text_muted);
                    continue;
                };
                let ordinal = ordinal(row);
                let selected = page.selected == Some(ordinal);
                let ranged = page.selection.is_some_and(|range| range.contains(&ordinal));
                if selected || ranged || response.hovered() {
                    let fill = if selected {
                        self.selected_fill
                    } else {
                        self.hover_fill
                    };
                    ui.painter()
                        .rect_filled(rect, 2.0, colors.focus.gamma_multiply(fill));
                }
                if response.clicked() {
                    output.clicked = Some((ordinal, ui.input(|input| input.modifiers.shift)));
                }
                paint(ui, rect, row);
            }
        });
        output
    }

    pub fn cells(&self, ui: &Ui, rect: Rect, values: [&str; N], color: Color32) {
        let mut left = rect.left();
        for (value, fraction) in values.into_iter().zip(self.fractions) {
            let right = left + fraction * rect.width();
            let cell = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
            let painter = ui.painter().with_clip_rect(cell);
            let inset = 3.0;
            let width = (cell.width() - 2.0 * inset).max(0.0);
            let mut job =
                LayoutJob::simple(value.to_owned(), FontId::monospace(10.5), color, width);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let text = painter.layout_job(job);
            let y = cell.center().y - text.size().y * 0.5;
            painter.galley(pos2(left + inset, y), text, color);
            left = right;
        }
    }
}
