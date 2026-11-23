//! Channel lists and Icom image inspection without connecting a radio.

pub(super) mod strip;

use egui::{Sense, Ui};
use egui_extras::{Column, TableBuilder};
use little_clip_core::{
    engineering::radio::{self, BANDS, Channel, Plan},
    palette::AppPalette,
};
use std::{path::Path, sync::atomic::AtomicBool};

pub(super) struct Viewer {
    plan: Plan,
    band_counts: [usize; BANDS.len()],
    selected: Option<usize>,
    band: Option<usize>,
    search: String,
    shown: Vec<usize>,
    memory: bool,
    record: usize,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let plan = radio::read(path, cancel)?;
        let length = plan.channels.len();
        let band_counts = plan.band_counts();
        Ok(Self {
            memory: length == 0 && !plan.records.is_empty(),
            plan,
            band_counts,
            selected: (length > 0).then_some(0),
            band: None,
            search: String::new(),
            shown: (0..length).collect(),
            record: 0,
        })
    }

    pub fn focus_channel(&mut self, ordinal: usize) {
        if ordinal < self.plan.channels.len() {
            self.selected = Some(ordinal);
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
            self.channels(ui);
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

    fn channels(&mut self, ui: &mut Ui) {
        self.channel_filters(ui);
        let footer = 183.0;
        let body_height = (ui.available_height() - footer).max(84.0);
        self.channel_table(ui, body_height);
        self.channel_info(ui);
    }

    fn channel_filters(&mut self, ui: &mut Ui) {
        let mut changed = false;
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
                    .changed();
            }
            copy = ui
                .add_enabled(!self.shown.is_empty(), egui::Button::new("Copy channels"))
                .on_hover_text("Copy the filtered channels as tab-separated text")
                .clicked();
        });
        if changed {
            let query = self.search.trim().to_lowercase();
            self.shown = self
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
            self.selected = self
                .selected
                .filter(|index| self.shown.binary_search(index).is_ok())
                .or_else(|| self.shown.first().copied());
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

    fn channel_table(&mut self, ui: &mut Ui, height: f32) {
        ui.spacing_mut().item_spacing.y = 1.0;
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 3.0;
        let name_width = (ui.available_width() * 0.28).clamp(72.0, 250.0);
        TableBuilder::new(ui)
            .id_salt("radio_channel_list")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .max_scroll_height(height)
            .column(Column::exact(48.0).clip(true))
            .column(Column::exact(name_width).clip(true))
            .column(Column::exact(125.0).clip(true))
            .column(Column::exact(64.0).clip(true))
            .column(Column::remainder().clip(true))
            .header(row_height, |mut row| {
                for title in ["#", "Channel", "Receive", "Mode", "Band"] {
                    row.col(|ui| {
                        ui.strong(title);
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
                        ui.label(&channel.mode);
                    });
                    row.col(|ui| {
                        ui.label(BANDS[radio::band(channel.receive_hz)]);
                    });
                    if row.response().clicked() {
                        self.selected = Some(index);
                    }
                });
            });
    }

    fn channel_info(&self, ui: &mut Ui) {
        let Some(channel) = self.selected.map(|index| &self.plan.channels[index]) else {
            return;
        };
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{}  {}", channel.number, channel.name));
            ui.label(format!("RX {}", radio::frequency_label(channel.receive_hz)));
            let tx = channel
                .transmit_hz()
                .map(|hz| radio::frequency_label(hz))
                .unwrap_or_else(|| {
                    if channel.duplex == "off" {
                        "disabled".into()
                    } else {
                        "—".into()
                    }
                });
            ui.label(format!("TX {tx}"));
            if ui.button("Copy channel").clicked() {
                let mut report = format!(
                    "Location\t{}\nName\t{}\nRX (Hz)\t{}\n",
                    channel.number, channel.name, channel.receive_hz
                );
                for (field, value) in &channel.fields {
                    if !matches!(
                        field.as_str(),
                        "Location" | "Name" | "Frequency" | "shortName"
                    ) {
                        report.push_str(&format!("{field}\t{value}\n"));
                    }
                }
                ui.ctx().copy_text(report);
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("radio_channel_info")
            .max_height(ui.available_height().min(141.0))
            .show(ui, |ui| {
                egui::Grid::new("radio_channel_fields")
                    .striped(true)
                    .show(ui, |ui| {
                        for (name, value) in &channel.fields {
                            if !matches!(
                                name.as_str(),
                                "Location" | "Name" | "Frequency" | "shortName"
                            ) {
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

fn matches_query(channel: &Channel, query: &str) -> bool {
    channel.name.to_lowercase().contains(query)
        || channel.mode.to_lowercase().contains(query)
        || channel.number.to_string() == query
        || radio::frequency_label(channel.receive_hz)
            .to_lowercase()
            .contains(query)
}
