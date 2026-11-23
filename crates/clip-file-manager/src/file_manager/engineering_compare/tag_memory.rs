use crate::ui_appearance::WorkbenchColors;
use egui::{Align, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use little_clip_core::engineering::tags::{
    Agreement, ComparedRow, Memory, MemoryComparison, ObservedByte,
};

const COMPARISONS: [Agreement; 4] = [
    Agreement::Different,
    Agreement::OneRead,
    Agreement::NeitherRead,
    Agreement::Equal,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Different,
    OneRead,
    All,
}

impl Filter {
    fn includes(self, row: &ComparedRow) -> bool {
        match self {
            Self::Different => row.count(Agreement::Different) != 0,
            Self::OneRead => row.count(Agreement::OneRead) != 0,
            Self::All => true,
        }
    }
}

pub(super) struct View {
    comparison: MemoryComparison,
    filter: Filter,
    visible: Vec<usize>,
    selected: usize,
    byte: usize,
    scroll_to: Option<usize>,
}

impl View {
    pub(super) fn compare([left, right]: [Option<&Memory>; 2]) -> Option<Self> {
        let comparison = MemoryComparison::new(left?, right?)?;
        let filter = if comparison.count(Agreement::Different) != 0 {
            Filter::Different
        } else if comparison.count(Agreement::OneRead) != 0 {
            Filter::OneRead
        } else {
            Filter::All
        };
        let mut view = Self {
            comparison,
            filter,
            visible: Vec::new(),
            selected: 0,
            byte: 0,
            scroll_to: None,
        };
        view.refresh();
        Some(view)
    }

    pub(super) fn ui(&mut self, ui: &mut Ui, names: [&str; 2], colors: WorkbenchColors) {
        self.filters(ui, colors);
        self.map(ui, colors);
        self.details(ui, names, colors);
        ui.separator();
        self.table(ui);
    }

    fn filters(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let before = self.filter;
        ui.horizontal_wrapped(|ui| {
            for (filter, agreement) in [
                (Filter::Different, Agreement::Different),
                (Filter::OneRead, Agreement::OneRead),
            ] {
                let count = self.comparison.count(agreement);
                let label = egui::RichText::new(format!("{} ({count})", label(agreement)));
                let label = if filter == self.filter {
                    label
                } else {
                    label.color(ink(agreement, colors))
                };
                ui.selectable_value(&mut self.filter, filter, label);
            }
            ui.selectable_value(&mut self.filter, Filter::All, "All addresses");
            ui.weak(format!(
                "{} matching bytes · {} unread on both",
                self.comparison.count(Agreement::Equal),
                self.comparison.count(Agreement::NeitherRead),
            ));
        });
        if self.filter != before {
            self.refresh();
        }
    }

    fn refresh(&mut self) {
        self.visible = self
            .comparison
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.filter.includes(row))
            .map(|(index, _)| index)
            .collect();
        if self.visible.binary_search(&self.selected).is_err()
            && let Some(&first) = self.visible.first()
        {
            self.selected = first;
        }
        self.scroll_to = self.visible.binary_search(&self.selected).ok();
        if self.scroll_to.is_some() {
            self.select(self.selected);
        }
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        let row = &self.comparison.rows[index];
        let target = if self.filter == Filter::OneRead && row.count(Agreement::OneRead) > 0 {
            Agreement::OneRead
        } else {
            row.strongest()
        };
        self.byte = row
            .bytes
            .iter()
            .position(|&pair| Agreement::between(pair) == target)
            .unwrap_or(0);
    }

    fn map(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        let map = AddressMap::new(rect, &self.comparison.rows);
        map.paint(ui, &self.comparison.rows, self.selected, colors);
        if let Some(index) = response
            .interact_pointer_pos()
            .and_then(|pointer| map.at(pointer.x))
            && response.clicked()
        {
            self.select(index);
            if !self.filter.includes(&self.comparison.rows[index]) {
                self.filter = Filter::All;
                self.refresh();
            }
            self.scroll_to = self.visible.binary_search(&index).ok();
        }
        response.on_hover_ui_at_pointer(|ui| {
            let Some(index) = ui
                .input(|input| input.pointer.hover_pos())
                .and_then(|point| map.at(point.x))
            else {
                return;
            };
            let row = &self.comparison.rows[index];
            ui.strong(format!("Address {} · 0x{:04X}", row.address, row.address));
            for agreement in COMPARISONS {
                let count = row.count(agreement);
                if count > 0 {
                    ui.label(format!("{count} · {}", label(agreement)));
                }
            }
        });
    }

    fn details(&mut self, ui: &mut Ui, names: [&str; 2], colors: WorkbenchColors) {
        if self.visible.is_empty() {
            ui.weak("No addresses match this filter.");
            return;
        }
        let row = &self.comparison.rows[self.selected];
        ui.strong(format!("Address {} · 0x{:04X}", row.address, row.address));
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 23.0;
            for (side, name) in names.into_iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    let width = ui.available_width().min(170.0);
                    ui.allocate_ui_with_layout(
                        vec2(width, 23.0),
                        egui::Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_width(width);
                            ui.add(egui::Label::new(name).truncate())
                                .on_hover_text(name);
                        },
                    );
                    for (index, &values) in row.bytes.iter().enumerate() {
                        let state = Agreement::between(values);
                        let selected = self.byte == index;
                        let text = egui::RichText::new(byte_text(values[side])).monospace();
                        let text = if selected {
                            text
                        } else {
                            text.color(ink(state, colors))
                        };
                        if ui
                            .add_sized([30.0, 23.0], egui::Button::selectable(selected, text))
                            .on_hover_text(format!("Byte {index} · {}", label(state)))
                            .clicked()
                        {
                            self.byte = index;
                        }
                    }
                });
            }
        });
        if let Some(&pair) = row.bytes.get(self.byte) {
            let agreement = Agreement::between(pair);
            let [left, right] = pair.map(byte_detail);
            ui.colored_label(
                ink(agreement, colors),
                format!(
                    "Byte {}   {left}  /  {right}   ·   {}",
                    self.byte,
                    label(agreement)
                ),
            );
        }
    }

    fn table(&mut self, ui: &mut Ui) {
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let mut target = None;
        let mut table = TableBuilder::new(ui)
            .id_salt("compared tag addresses")
            .striped(true)
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .columns(Column::remainder().clip(true), 5)
            .sense(Sense::click());
        if let Some(row) = self.scroll_to.take() {
            table = table.scroll_to_row(row, Some(Align::Center));
        }
        table
            .header(row_height, |mut row| {
                row.col(|ui| {
                    ui.strong("Address");
                });
                for agreement in COMPARISONS {
                    row.col(|ui| {
                        ui.strong(label(agreement));
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.visible.len(), |mut table_row| {
                    let index = self.visible[table_row.index()];
                    let row = &self.comparison.rows[index];
                    table_row.set_selected(index == self.selected);
                    table_row.col(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "{} · 0x{:04X}",
                                    row.address, row.address
                                ))
                                .monospace(),
                            )
                            .selectable(false),
                        );
                    });
                    for agreement in COMPARISONS {
                        table_row.col(|ui| {
                            ui.add(
                                egui::Label::new(row.count(agreement).to_string())
                                    .selectable(false),
                            );
                        });
                    }
                    if table_row.response().clicked() {
                        target = Some(index);
                    }
                });
            });
        if let Some(index) = target {
            self.select(index);
        }
    }
}

struct AddressMap {
    rect: Rect,
    first: u32,
    extent: usize,
    bins: Vec<Option<usize>>,
}

impl AddressMap {
    fn new(rect: Rect, rows: &[ComparedRow]) -> Self {
        let first = rows[0].address;
        let extent = (rows[rows.len() - 1].address - first) as usize + 1;
        let width = rect.width().ceil().clamp(1.0, 2048.0) as usize;
        let mut map = Self {
            rect,
            first,
            extent,
            bins: vec![None; width.min(extent)],
        };
        for (index, row) in rows.iter().enumerate() {
            let bin = map.bin(row.address);
            if map.bins[bin].is_none_or(|prior| row.strongest() > rows[prior].strongest()) {
                map.bins[bin] = Some(index);
            }
        }
        map
    }

    fn bin(&self, address: u32) -> usize {
        (address - self.first) as usize * self.bins.len() / self.extent
    }

    fn at(&self, x: f32) -> Option<usize> {
        let fraction = (x - self.rect.left()) / self.rect.width().max(1.0);
        let index = (fraction * self.bins.len() as f32) as usize;
        self.bins[index.min(self.bins.len() - 1)]
    }

    fn paint(&self, ui: &Ui, rows: &[ComparedRow], selected: usize, colors: WorkbenchColors) {
        let width = self.rect.width() / self.bins.len() as f32;
        let cell = |bin| {
            let left = self.rect.left() + bin as f32 * width;
            Rect::from_min_max(
                pos2(left, self.rect.top() + 3.0),
                pos2(left + width, self.rect.bottom() - 3.0),
            )
        };
        let painter = ui.painter_at(self.rect);
        painter.rect_filled(self.rect, 3.0, colors.recessed);
        let mut mesh = egui::Mesh::default();
        for (bin, row) in self.bins.iter().enumerate() {
            if let Some(index) = row {
                let state = rows[*index].strongest();
                let color = match state {
                    Agreement::Equal => colors.success.gamma_multiply(0.45),
                    Agreement::NeitherRead => colors.text_muted.gamma_multiply(0.20),
                    _ => ink(state, colors),
                };
                mesh.add_colored_rect(cell(bin), color);
            }
        }
        painter.add(mesh);
        painter.rect_stroke(
            cell(self.bin(rows[selected].address)).expand2(vec2(1.0, 2.0)),
            0.0,
            Stroke::new(1.5, colors.focus),
            StrokeKind::Inside,
        );
    }
}

fn label(state: Agreement) -> &'static str {
    match state {
        Agreement::Equal => "Match",
        Agreement::NeitherRead => "Neither read",
        Agreement::OneRead => "One read",
        Agreement::Different => "Different",
    }
}

fn ink(state: Agreement, colors: WorkbenchColors) -> Color32 {
    match state {
        Agreement::Equal => colors.text_primary,
        Agreement::NeitherRead => colors.text_muted,
        Agreement::OneRead => colors.link,
        Agreement::Different => colors.warning,
    }
}

fn byte_text(byte: ObservedByte) -> String {
    match byte {
        ObservedByte::Known(value) => format!("{value:02X}"),
        ObservedByte::Unread => "··".into(),
        ObservedByte::Absent => "—".into(),
    }
}

fn byte_detail(byte: ObservedByte) -> String {
    match byte {
        ObservedByte::Known(value) => format!("0x{value:02X} ({value:08b})"),
        ObservedByte::Unread => "Unread".into(),
        ObservedByte::Absent => "Absent".into(),
    }
}
