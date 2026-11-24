//! Channel lists and Icom image inspection without connecting a radio.

mod frequency;
pub(super) mod strip;

use crate::ui_appearance::WorkbenchColors;
use egui::{Sense, Ui};
use egui_extras::{Column, TableBuilder};
use little_clip_core::{
    engineering::radio::{self, BANDS, Channel, Plan, Tone, Tones},
    palette::AppPalette,
};
use std::{path::Path, sync::atomic::AtomicBool};

pub(super) struct Viewer {
    plan: Plan,
    band_counts: [usize; BANDS.len()],
    selected: Option<usize>,
    band: Option<usize>,
    search: String,
    matched: Vec<usize>,
    shown: Vec<usize>,
    frequency: frequency::View,
    scroll_to: Option<usize>,
    show_fields: bool,
    memory: bool,
    record: usize,
    order: Option<(ChannelColumn, bool)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChannelColumn {
    Number,
    Name,
    Receive,
    Transmit,
    Mode,
    Tone,
    Band,
}

impl ChannelColumn {
    const ALL: [(Self, &'static str); 7] = [
        (Self::Number, "#"),
        (Self::Name, "Channel"),
        (Self::Receive, "Receive"),
        (Self::Transmit, "Transmit"),
        (Self::Mode, "Mode"),
        (Self::Tone, "Tone mode"),
        (Self::Band, "Band"),
    ];

    fn compare(self, a: &Channel, b: &Channel, format: radio::Format) -> std::cmp::Ordering {
        match self {
            Self::Number => a.number.cmp(&b.number),
            Self::Name => super::sorting::natural_cmp(&a.name, &b.name),
            Self::Receive => a.receive_hz.cmp(&b.receive_hz),
            Self::Transmit => a.transmit_hz().cmp(&b.transmit_hz()),
            Self::Mode => a.mode.cmp(&b.mode),
            Self::Tone => Tones::for_channel(a, format)
                .mode
                .label()
                .cmp(Tones::for_channel(b, format).mode.label()),
            Self::Band => radio::band(a.receive_hz).cmp(&radio::band(b.receive_hz)),
        }
    }
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let plan = radio::read(path, cancel)?;
        let length = plan.channels.len();
        let band_counts = plan.band_counts();
        let shown: Vec<_> = (0..length).collect();
        let mut frequency = frequency::View::default();
        frequency.rebuild(&plan.channels, &shown);
        Ok(Self {
            memory: length == 0 && !plan.records.is_empty(),
            plan,
            band_counts,
            selected: (length > 0).then_some(0),
            band: None,
            search: String::new(),
            matched: shown.clone(),
            shown,
            frequency,
            scroll_to: None,
            show_fields: false,
            record: 0,
            order: None,
        })
    }

    pub fn focus_channel(&mut self, ordinal: usize) {
        if ordinal < self.plan.channels.len() {
            self.selected = Some(ordinal);
            self.scroll_to = Some(ordinal);
            self.memory = false;
        } else if self.memory && ordinal < self.plan.records.len() {
            self.record = ordinal;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        let mut shell = crate::shell::StudioShell::new(
            "Radio programming",
            crate::icons::Icon::Waves,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.header(ui);
        if self.memory {
            self.clone_records(ui);
        } else {
            self.channels(ui, WorkbenchColors::from_palette(palette));
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.plan.format.title());
            if let Some(model) = &self.plan.model {
                ui.label(model);
            }
            if !self.plan.channels.is_empty() && !self.plan.records.is_empty() {
                ui.selectable_value(&mut self.memory, false, "Channels");
                ui.selectable_value(&mut self.memory, true, "Clone records");
            }
        });
        if self.memory {
            ui.horizontal_wrapped(|ui| {
                ui.weak(format!("{} records", self.plan.records.len()));
                for (name, value) in self.plan.fields.iter().take(4) {
                    ui.weak(format!("{name}  {value}"));
                }
            });
        }
    }

    fn channels(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        self.channel_filters(ui);
        match self
            .frequency
            .show(ui, &self.plan.channels, self.selected, colors)
        {
            Some(frequency::Action::Select(index)) => {
                self.apply_brush();
                self.selected = Some(index);
                self.scroll_to = Some(index);
                ui.ctx().request_repaint();
            }
            Some(frequency::Action::Filter) => {
                self.apply_brush();
                self.scroll_to = self.selected;
                ui.ctx().request_repaint();
            }
            None => {}
        }
        let footer = if self.show_fields { 224.0 } else { 125.0 };
        let body_height = (ui.available_height() - footer).max(84.0);
        ui.scope(|ui| self.channel_table(ui, body_height));
        self.channel_info(ui);
    }

    fn channel_filters(&mut self, ui: &mut Ui) {
        let mut changed = false;
        let mut cleared_range = false;
        let band = self.band;
        let mut copy = false;
        ui.horizontal_wrapped(|ui| {
            changed |= ui
                .add(egui::TextEdit::singleline(&mut self.search).hint_text("Find channels"))
                .changed();
            changed |= ui
                .selectable_value(
                    &mut self.band,
                    None,
                    format!("All {}", self.plan.channels.len()),
                )
                .changed();
            for (index, name) in BANDS.iter().enumerate() {
                let count = self.band_counts[index];
                if count == 0 {
                    continue;
                }
                changed |= ui
                    .selectable_value(&mut self.band, Some(index), format!("{name}  {count}"))
                    .on_hover_text("Filter by receive band")
                    .changed();
            }
            ui.toggle_value(&mut self.frequency.open, "Frequency map");
            if self.frequency.open && ui.small_button("Fit map").clicked() {
                self.frequency.fit();
            }
            if let Some(brush) = self.frequency.brush.as_ref() {
                let label = format!(
                    "{} to {}  ×",
                    radio::frequency_label(*brush.start()),
                    radio::frequency_label(*brush.end()),
                );
                if ui
                    .button(label)
                    .on_hover_text("Clear the receive/transmit range filter")
                    .clicked()
                {
                    self.frequency.brush = None;
                    cleared_range = true;
                }
            }
            copy = ui
                .add_enabled(!self.shown.is_empty(), egui::Button::new("Copy channels"))
                .on_hover_text("Copy the filtered channels as tab-separated text")
                .clicked();
        });
        if changed {
            if band != self.band {
                self.frequency.brush = None;
            }
            self.apply_channel_filters();
        } else if cleared_range {
            self.apply_brush();
        }
        if copy {
            use std::fmt::Write;
            let mut table =
                String::from("Channel\tName\tReceive (Hz)\tTransmit (Hz)\tMode\tBand\n");
            for &index in &self.shown {
                let channel = &self.plan.channels[index];
                let name = channel.name.replace(['\t', '\r', '\n'], " ");
                let tx = channel
                    .transmit_hz()
                    .map_or_else(String::new, |hz| hz.to_string());
                let band = BANDS[radio::band(channel.receive_hz)];
                let _ = writeln!(
                    table,
                    "{}\t{name}\t{}\t{tx}\t{}\t{band}",
                    channel.number, channel.receive_hz, channel.mode
                );
            }
            ui.ctx().copy_text(table);
        }
    }

    fn apply_channel_filters(&mut self) {
        let query = self.search.trim().to_lowercase();
        self.matched = self
            .plan
            .channels
            .iter()
            .enumerate()
            .filter(|(_, channel)| {
                self.band
                    .is_none_or(|band| band == radio::band(channel.receive_hz))
                    && (query.is_empty() || matches_query(channel, &query))
            })
            .map(|(index, _)| index)
            .collect();
        self.frequency.rebuild(&self.plan.channels, &self.matched);
        self.apply_brush();
        self.scroll_to = self.selected;
    }

    fn apply_brush(&mut self) {
        self.shown = self
            .matched
            .iter()
            .copied()
            .filter(|&index| self.frequency.includes(&self.plan.channels[index]))
            .collect();
        self.sort_channels();
        self.selected = self
            .selected
            .filter(|index| self.shown.contains(index))
            .or_else(|| self.shown.first().copied());
    }

    fn sort_channels(&mut self) {
        let Some((column, descending)) = self.order else {
            return;
        };
        self.shown.sort_by(|&a, &b| {
            let compare = column.compare(
                &self.plan.channels[a],
                &self.plan.channels[b],
                self.plan.format,
            );
            let compare = if descending {
                compare.reverse()
            } else {
                compare
            };
            compare.then(a.cmp(&b))
        });
    }

    fn channel_table(&mut self, ui: &mut Ui, height: f32) {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.style_mut().interaction.selectable_labels = false;
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 3.0;
        let header_height = row_height.max(ui.spacing().interact_size.y) + 4.0;
        let name_width = (ui.available_width() * 0.28).clamp(72.0, 250.0);
        let mut table = TableBuilder::new(ui)
            .id_salt("radio_channel_list")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, true])
            .max_scroll_height(height)
            .column(Column::exact(48.0).clip(true))
            .column(Column::exact(name_width).clip(true))
            .column(Column::exact(125.0).clip(true))
            .column(Column::exact(125.0).clip(true))
            .column(Column::exact(64.0).clip(true))
            .column(Column::exact(81.0).clip(true))
            .column(Column::remainder().clip(true));
        if let Some(row) = self
            .scroll_to
            .take()
            .and_then(|index| self.shown.iter().position(|&other| other == index))
        {
            table = table.scroll_to_row(row, Some(egui::Align::Center));
        }
        let mut sorted = None;
        table
            .header(header_height, |mut row| {
                for (column, title) in ChannelColumn::ALL {
                    row.col(|ui| {
                        let direction = self
                            .order
                            .filter(|(sorted, _)| *sorted == column)
                            .map(|(_, descending)| descending);
                        let caption = match direction {
                            Some(true) => format!("{title} ↓"),
                            Some(false) => format!("{title} ↑"),
                            None => title.to_owned(),
                        };
                        if ui
                            .add(egui::Button::new(caption).frame(false).truncate())
                            .on_hover_text("Sort the filtered channels")
                            .clicked()
                        {
                            sorted = Some((column, direction == Some(false)));
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.shown.len(), |mut row| {
                    let index = self.shown[row.index()];
                    let channel = &self.plan.channels[index];
                    row.set_selected(self.selected == Some(index));
                    row.col(|ui| {
                        ui.monospace(channel.number.to_string());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&channel.name).truncate());
                    });
                    row.col(|ui| {
                        ui.monospace(radio::frequency_label(channel.receive_hz));
                    });
                    row.col(|ui| {
                        ui.monospace(transmit_label(channel));
                    });
                    row.col(|ui| {
                        ui.label(&channel.mode);
                    });
                    row.col(|ui| {
                        ui.label(Tones::for_channel(channel, self.plan.format).mode.label());
                    });
                    row.col(|ui| {
                        ui.label(BANDS[radio::band(channel.receive_hz)]);
                    });
                    if row.response().clicked() {
                        self.selected = Some(index);
                        row.response().ctx.request_repaint();
                    }
                });
            });
        if let Some(order) = sorted {
            self.order = Some(order);
            self.sort_channels();
            self.scroll_to = self.selected;
            ui.ctx().request_repaint();
        }
    }

    fn channel_info(&mut self, ui: &mut Ui) {
        let Some(channel) = self.selected.map(|index| &self.plan.channels[index]) else {
            return;
        };
        ui.separator();
        let tones = Tones::for_channel(channel, self.plan.format);
        let has_extra = channel
            .fields
            .iter()
            .any(|(name, value)| !value.is_empty() && extra_field(name));
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{}  {}", channel.number, channel.name));
            ui.weak(&channel.mode);
            if let Some(group) = channel.field("Group") {
                ui.weak(format!("Group {group}"));
            } else if let Some(bank) = channel.field("Bank") {
                ui.weak(format!("Bank {bank}"));
            }
            if let Some(scan) = scan_label(channel) {
                ui.weak(scan);
            }
            if ui.button("Copy channel").clicked() {
                copy_channel(ui, channel);
            }
            if has_extra {
                ui.toggle_value(&mut self.show_fields, "Stored fields");
            }
        });
        channel_frequencies(ui, channel, tones);
        if !has_extra || !self.show_fields {
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("radio_channel_info")
            .max_height(ui.available_height().min(141.0))
            .show(ui, |ui| {
                egui::Grid::new("radio_channel_fields")
                    .striped(true)
                    .show(ui, |ui| {
                        for (name, value) in &channel.fields {
                            if !value.is_empty() && extra_field(name) {
                                ui.weak(name);
                                ui.label(value);
                                ui.end_row();
                            }
                        }
                    });
            });
    }

    fn clone_records(&mut self, ui: &mut Ui) {
        if self.plan.records.is_empty() {
            return;
        }
        let height = (ui.available_height() - 107.0).max(80.0);
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
        TableBuilder::new(ui)
            .id_salt("radio_clone_records")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .max_scroll_height(height)
            .column(Column::exact(112.0).clip(true))
            .column(Column::remainder().clip(true))
            .header(row_height, |mut row| {
                row.col(|ui| {
                    ui.strong("Address");
                });
                row.col(|ui| {
                    ui.strong("Bytes");
                });
            })
            .body(|body| {
                body.rows(row_height, self.plan.records.len(), |mut row| {
                    let index = row.index();
                    let record = &self.plan.records[index];
                    row.set_selected(self.record == index);
                    row.col(|ui| {
                        ui.monospace(format!("{:08X}", record.address));
                    });
                    row.col(|ui| {
                        ui.monospace(record.bytes.len().to_string());
                    });
                    if row.response().clicked() {
                        self.record = index;
                    }
                });
            });
        let record = &self.plan.records[self.record];
        ui.separator();
        ui.strong(format!("Address 0x{:08X}", record.address));
        let hex = record
            .bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        ui.monospace(hex);
    }
}

fn copy_channel(ui: &Ui, channel: &Channel) {
    let mut report = format!(
        "Location\t{}\nName\t{}\nRX (Hz)\t{}\n",
        channel.number, channel.name, channel.receive_hz
    );
    for (field, value) in &channel.fields {
        if extra_field(field) {
            report.push_str(&format!("{field}\t{value}\n"));
        }
    }
    ui.ctx().copy_text(report);
}

fn channel_frequencies(ui: &mut Ui, channel: &Channel, tones: Tones) {
    ui.horizontal_wrapped(|ui| {
        direction(
            ui,
            "RX",
            radio::frequency_label(channel.receive_hz),
            tones.receive,
        );
        ui.separator();
        direction(ui, "TX", transmit_label(channel), tones.transmit);
        if let Some(tx) = channel.transmit_hz()
            && tx != channel.receive_hz
        {
            let offset = i128::from(tx) - i128::from(channel.receive_hz);
            let sign = if offset < 0 { '−' } else { '+' };
            ui.weak(format!(
                "Offset {sign}{}",
                radio::frequency_label(offset.unsigned_abs() as u64),
            ));
        }
    });
}

fn direction(ui: &mut Ui, title: &str, frequency: String, tone: Tone) {
    ui.monospace(format!("{title} {frequency}"));
    if tone == Tone::Disabled {
        return;
    }
    let label = ui.weak(tone.label(title == "RX"));
    if tone.device_specific() {
        label.on_hover_text(
            "Both values are stored. The target radio model determines which field is used.",
        );
    }
}

fn transmit_label(channel: &Channel) -> String {
    match channel.transmit_hz() {
        Some(hz) => radio::frequency_label(hz),
        None if channel.duplex == "off" => "disabled".into(),
        None => "—".into(),
    }
}

fn scan_label(channel: &Channel) -> Option<&str> {
    channel.field("Scan").or_else(|| {
        match channel.field("Skip").or_else(|| channel.field("skip")) {
            Some("S") => Some("Scan skipped"),
            Some("P") => Some("Scan priority"),
            _ => None,
        }
    })
}

fn extra_field(name: &str) -> bool {
    !matches!(
        name,
        "Location" | "Name" | "Frequency" | "shortName" | "frequency"
    )
}

fn matches_query(channel: &Channel, query: &str) -> bool {
    channel.name.to_lowercase().contains(query)
        || channel.mode.to_lowercase().contains(query)
        || channel.number.to_string() == query
        || radio::frequency_label(channel.receive_hz)
            .to_lowercase()
            .contains(query)
}
