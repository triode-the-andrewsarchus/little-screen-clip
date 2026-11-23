//! Channel lists and Icom image inspection without connecting a radio.

pub(super) mod strip;

use egui::{RichText, Ui};
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
    }

    fn channel_table(&mut self, ui: &mut Ui, height: f32) {
        ui.horizontal(|ui| {
            ui.add_sized([56.0, 24.0], egui::Label::new("#"));
            ui.add_sized([190.0, 24.0], egui::Label::new("Channel"));
            ui.add_sized([126.0, 24.0], egui::Label::new("Receive"));
            ui.label("Mode");
        });
        egui::ScrollArea::vertical()
            .id_salt("radio_channel_list")
            .max_height(height)
            .show_rows(ui, 24.0, self.shown.len(), |ui, rows| {
                for row in rows {
                    let index = self.shown[row];
                    let channel = &self.plan.channels[index];
                    ui.horizontal(|ui| {
                        let number = ui.add_sized(
                            [56.0, 24.0],
                            egui::Button::selectable(
                                self.selected == Some(index),
                                channel.number.to_string(),
                            ),
                        );
                        let name = ui.add_sized(
                            [182.0, 24.0],
                            egui::Label::new(&channel.name)
                                .truncate()
                                .sense(egui::Sense::click()),
                        );
                        let rx = ui.add_sized(
                            [110.0, 24.0],
                            egui::Label::new(radio::frequency_label(channel.receive_hz))
                                .sense(egui::Sense::click()),
                        );
                        let mode =
                            ui.add(egui::Label::new(&channel.mode).sense(egui::Sense::click()));
                        if number.clicked() || name.clicked() || rx.clicked() || mode.clicked() {
                            self.selected = Some(index);
                        }
                        if let Some(selected) = self.selected
                            && selected == index
                        {
                            let band = BANDS[radio::band(channel.receive_hz)];
                            ui.weak(band);
                        }
                    });
                }
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
        egui::ScrollArea::vertical().max_height(height).show_rows(
            ui,
            23.0,
            self.plan.records.len(),
            |ui, rows| {
                for index in rows {
                    let record = &self.plan.records[index];
                    let label = format!("{:08X}     {} bytes", record.address, record.bytes.len());
                    if ui
                        .selectable_label(self.record == index, RichText::new(label).monospace())
                        .clicked()
                    {
                        self.record = index;
                    }
                }
            },
        );
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
