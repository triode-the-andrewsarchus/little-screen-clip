use crate::ui_appearance::WorkbenchColors;
use egui::{
    Color32, FontId, Rect, ScrollArea, Sense, Ui, pos2, scroll_area::ScrollAreaOutput, vec2,
};
use std::ops::Range;

pub(super) const ROW_HEIGHT: f32 = 21.0;

pub(super) struct TableBody<'a> {
    pub rows: Range<u64>,
    pub width: f32,
    pub selected: Option<u64>,
    pub selected_range: Option<&'a Range<u64>>,
    pub selected_fill: f32,
    pub hover_fill: f32,
}

pub(super) struct VisibleRows {
    pub visible: Range<u64>,
    pub clicked: Option<(u64, bool)>,
}

impl TableBody<'_> {
    pub fn show<T>(
        self,
        ui: &mut Ui,
        area: ScrollArea,
        colors: WorkbenchColors,
        read: impl Fn(u64) -> (Option<u64>, T),
        mut paint: impl FnMut(&Ui, Rect, u64, T) -> Option<String>,
    ) -> ScrollAreaOutput<VisibleRows> {
        let start = self.rows.start;
        let count = (self.rows.end - start) as usize;
        area.show_rows(ui, ROW_HEIGHT, count, |ui, visible| {
            let visible = start + visible.start as u64..start + visible.end as u64;
            let mut clicked = None;
            for index in visible.clone() {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(self.width, ROW_HEIGHT), Sense::hover());
                if index % 2 == 1 {
                    ui.painter().rect_filled(rect, 0.0, colors.panel);
                }
                let (source_id, row) = read(index);
                let response = source_id.map(|source_id| {
                    let id = ui.make_persistent_id(("table-row", source_id));
                    let response = ui.interact(rect, id, Sense::click());
                    let selected = self.selected == Some(source_id);
                    let ranged = self
                        .selected_range
                        .is_some_and(|range| range.contains(&source_id));
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
                        clicked = Some((source_id, ui.input(|input| input.modifiers.shift)));
                    }
                    (response, selected)
                });
                if let (Some(label), Some((response, selected))) =
                    (paint(ui, rect, index, row), response)
                {
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            ui.is_enabled(),
                            selected,
                            &label,
                        )
                    });
                }
            }
            VisibleRows { visible, clicked }
        })
    }
}

pub(super) fn paint_cell(ui: &Ui, rect: Rect, value: &str, color: Color32) {
    let painter = ui.painter().with_clip_rect(rect);
    let inset = 3.0;
    let width = (rect.width() - 2.0 * inset).max(0.0);
    let mut job =
        egui::text::LayoutJob::simple(value.to_owned(), FontId::monospace(12.0), color, width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let text = painter.layout_job(job);
    let y = rect.center().y - text.size().y * 0.5;
    painter.galley(pos2(rect.left() + inset, y), text, color);
}
