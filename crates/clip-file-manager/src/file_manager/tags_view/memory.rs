use super::hex;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align, Key, Layout, Modifiers, ScrollArea, Ui, vec2};
use little_clip_core::engineering::tags::{Memory, MemoryRow, RowRole};

pub(super) struct View {
    memory: Memory,
    coverage: [usize; 2],
    selected: Option<usize>,
    scroll_to: Option<usize>,
    byte: Option<usize>,
    field: Option<usize>,
    reveal_field: bool,
}

impl View {
    pub fn new(memory: Memory) -> Self {
        let selected = (!memory.rows.is_empty()).then_some(0);
        let coverage = memory.coverage();
        Self {
            memory,
            coverage,
            selected,
            scroll_to: None,
            byte: None,
            field: None,
            reveal_field: false,
        }
    }

    pub fn focus(&mut self, ordinal: u64) {
        if self.memory.rows.is_empty() {
            return;
        }
        let ordinal = ordinal.min(self.memory.rows.len() as u64 - 1) as usize;
        self.selected = Some(ordinal);
        self.scroll_to = Some(ordinal.saturating_sub(4));
        self.byte = None;
    }

    pub fn ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        self.navigate(ui);
        self.title(ui, colors);
        ui.separator();
        let width = ui.available_width();
        let height = ui.available_height().max(132.0);
        if width > 650.0 {
            ui.horizontal_top(|ui| {
                let left = (width * 0.36).max(210.0);
                ui.allocate_ui_with_layout(
                    vec2(left, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(left);
                        self.list(ui, height, colors);
                    },
                );
                ui.separator();
                ui.vertical(|ui| self.details(ui, colors));
            });
        } else {
            self.list(ui, (height * 0.42).max(86.0), colors);
            ui.separator();
            self.details(ui, colors);
        }
    }

    fn navigate(&mut self, ui: &Ui) {
        let delta = ui.input_mut(|input| {
            let down = input.consume_key(Modifiers::NONE, Key::ArrowDown);
            let up = input.consume_key(Modifiers::NONE, Key::ArrowUp);
            isize::from(down) - isize::from(up)
        });
        if delta == 0 {
            return;
        }
        if !self.memory.rows.is_empty() && self.field.is_none() {
            let index = self
                .selected
                .unwrap_or_default()
                .saturating_add_signed(delta);
            self.focus(index as u64);
        } else if !self.memory.fields.is_empty() {
            let last = self.memory.fields.len() - 1;
            let index = self
                .field
                .map_or(0, |index| index.saturating_add_signed(delta));
            self.field = Some(index.min(last));
            self.reveal_field = true;
        }
    }

    fn title(&self, ui: &mut Ui, colors: WorkbenchColors) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(&self.memory.protocol);
            if let Some(version) = self.memory.version {
                ui.weak(format!("Version {version}"));
            }
            let [known, total] = self.coverage;
            if total > 0 {
                ui.weak(format!("{known} / {total} bytes read"));
            }
            if !self.memory.identifier.is_empty() {
                ui.monospace(hex(self.memory.identifier.iter().copied()));
            }
            if let Some(rom) = self.memory.rom_check {
                let (label, color) = if rom.valid() {
                    ("ROM CRC matches", colors.success)
                } else {
                    ("ROM CRC mismatch", colors.warning)
                };
                ui.colored_label(color, label).on_hover_text(format!(
                    "Recorded {:02X} · Calculated {:02X}",
                    rom.stored, rom.calculated,
                ));
            }
        });
    }

    fn list(&mut self, ui: &mut Ui, height: f32, colors: WorkbenchColors) {
        if self.memory.rows.is_empty() {
            ui.weak("Identifier-only capture");
            return;
        }
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 5.0;
        let mut area = ScrollArea::vertical()
            .id_salt("tag memory addresses")
            .auto_shrink([false, false])
            .max_height(height);
        if let Some(scroll) = self.scroll_to.take() {
            area = area
                .vertical_scroll_offset(scroll as f32 * (row_height + ui.spacing().item_spacing.y));
        }
        area.show_rows(ui, row_height, self.memory.rows.len(), |ui, range| {
            for index in range {
                let row = &self.memory.rows[index];
                let text = format!("{} · {} / {}", row.name, row.known_bytes(), row.bytes.len());
                let known = if row.known_bytes() == 0 {
                    colors.text_muted
                } else {
                    role_color(row.role, colors)
                };
                let selected = self.selected == Some(index);
                let label = egui::RichText::new(text);
                let label = if selected { label } else { label.color(known) };
                if ui
                    .add_sized(
                        [ui.available_width(), row_height],
                        egui::Button::selectable(selected, label),
                    )
                    .on_hover_text(
                        row.sector
                            .map_or_else(|| row.name.clone(), |sector| format!("Sector {sector}")),
                    )
                    .clicked()
                {
                    self.selected = Some(index);
                    self.byte = None;
                    self.field = None;
                }
            }
        });
    }

    fn details(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        if let Some(row) = self
            .selected
            .and_then(|selected| self.memory.rows.get(selected))
        {
            row_detail(ui, row, &mut self.byte, colors);
        }
        if !self.memory.fields.is_empty() {
            ui.separator();
            ui.strong("Recorded fields");
            ScrollArea::vertical()
                .id_salt("tag recorded fields")
                .max_height(ui.available_height().max(40.0))
                .show(ui, |ui| {
                    for (index, (name, value)) in self.memory.fields.iter().enumerate() {
                        let field = ui.selectable_label(
                            self.field == Some(index),
                            egui::RichText::new(format!("{name}   {value}")).monospace(),
                        );
                        if field.clicked() {
                            self.field = Some(index);
                        }
                        if self.field == Some(index) && self.reveal_field {
                            field.scroll_to_me(None);
                            self.reveal_field = false;
                        }
                    }
                });
        }
    }
}

fn role_color(role: RowRole, colors: WorkbenchColors) -> egui::Color32 {
    match role {
        RowRole::Manufacturer | RowRole::Rom => colors.link,
        RowRole::SectorTrailer => colors.warning,
        RowRole::Data => colors.text_primary,
    }
}

fn row_detail(ui: &mut Ui, row: &MemoryRow, selected: &mut Option<usize>, colors: WorkbenchColors) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(&row.name);
        if let Some(sector) = row.sector {
            ui.weak(format!("Sector {sector}"));
        }
        let kind = match row.role {
            RowRole::Manufacturer => Some("Manufacturer data"),
            RowRole::SectorTrailer => Some("Sector trailer"),
            RowRole::Rom => Some("ROM"),
            RowRole::Data => None,
        };
        if let Some(kind) = kind {
            ui.colored_label(role_color(row.role, colors), kind);
        }
    });
    for (offset, chunk) in row.bytes.chunks(16).enumerate() {
        ui.horizontal_wrapped(|ui| {
            for (index, &byte) in chunk.iter().enumerate() {
                let index = index + offset * 16;
                let label = byte.map_or_else(|| "··".to_owned(), |value| format!("{value:02X}"));
                if ui
                    .selectable_label(
                        *selected == Some(index),
                        egui::RichText::new(label).monospace(),
                    )
                    .on_hover_text(format!("Byte {index}"))
                    .clicked()
                {
                    *selected = Some(index);
                }
            }
        });
    }
    let ascii: String = row
        .bytes
        .iter()
        .map(|byte| {
            byte.filter(|value| value.is_ascii_graphic() || *value == b' ')
                .map_or('·', char::from)
        })
        .collect();
    ui.monospace(ascii);
    if let Some(index) = *selected {
        let value = row.bytes.get(index).copied().flatten().map_or_else(
            || "Unread".into(),
            |value| format!("0x{value:02X} · {value} · {value:08b}"),
        );
        ui.weak(format!("Byte {index}   {value}"));
    }
}
