use super::{Arc, Digital, Recording, Ui};
use little_clip_core::engineering::analysis::{Cutoffs, MiddleBand, Threshold};

impl Digital {
    pub(in crate::file_manager) fn tools(&mut self, ui: &mut Ui, recording: &Recording) {
        if ui.selectable_label(self.open, "Digital lanes").clicked() {
            self.open = !self.open;
        }
        ui.menu_button("Add decoder", |ui| {
            if self.panel.decoder_buttons(ui, &self.config.lanes) {
                self.open = true;
                for index in self.panel.channels().into_iter().flatten() {
                    if let Some(shown) = self.shown.get_mut(index) {
                        *shown = true;
                    }
                }
                ui.close();
            }
        });
        if self.open {
            ui.menu_button("Thresholds", |ui| self.thresholds(ui, recording));
        }
    }

    fn thresholds(&mut self, ui: &mut Ui, recording: &Recording) {
        let mut changed = false;
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for index in 0..self.shown.len() {
                ui.push_id(index, |ui| {
                    ui.checkbox(&mut self.shown[index], &self.config.lanes[index].name)
                        .on_hover_text(format!("Use {}_D in a single-recording equation", recording.channels()[index].name));
                    let mut rule = self.config.rules[index];
                    edit_cutoffs(ui, &mut rule.cutoffs, &recording.channels()[index].unit);
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("middle")
                            .selected_text(middle_name(rule.middle))
                            .show_ui(ui, |ui| {
                                for mode in [MiddleBand::Unknown, MiddleBand::Hold, MiddleBand::Floating] {
                                    ui.selectable_value(&mut rule.middle, mode, middle_name(mode));
                                }
                            })
                            .response
                            .on_hover_text("Interpret voltages between the cutoffs. Floating marks the configured voltage band.");
                        ui.checkbox(&mut rule.invert, "Invert");
                    });
                    if rule.valid() && rule != self.config.rules[index] {
                        Arc::make_mut(&mut self.config).rules[index] = rule;
                        changed = true;
                    }
                    ui.separator();
                });
            }
        });
        if changed {
            let input = Arc::make_mut(&mut self.config);
            input.revision = input.revision.wrapping_add(1);
            self.pending = None;
            self.ready = None;
            self.cursor = None;
            ui.ctx().request_repaint();
        }
    }
}

fn edit_cutoffs(ui: &mut Ui, cutoffs: &mut Cutoffs, unit: &str) {
    let mut adaptive = matches!(*cutoffs, Cutoffs::Adaptive { .. });
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut adaptive, "Auto")
            .on_hover_text("Adapt the cutoffs to the observed range of the analyzed window")
            .changed()
        {
            *cutoffs = if adaptive {
                Cutoffs::default()
            } else {
                Cutoffs::Fixed {
                    low: 0.8,
                    high: 2.0,
                }
            };
        }
        match *cutoffs {
            Cutoffs::Fixed { mut low, mut high } => {
                cutoffs_row(ui, &mut low, &mut high, 0.01, unit, -1e9..=1e9);
                *cutoffs = Cutoffs::Fixed { low, high };
            }
            Cutoffs::Adaptive {
                low_fraction,
                high_fraction,
            } => {
                let mut low = low_fraction * 100.0;
                let mut high = high_fraction * 100.0;
                if cutoffs_row(ui, &mut low, &mut high, 1.0, "%", 0.0..=100.0) {
                    *cutoffs = Cutoffs::Adaptive {
                        low_fraction: low / 100.0,
                        high_fraction: high / 100.0,
                    };
                }
            }
        }
    });
}

fn cutoffs_row(
    ui: &mut Ui,
    low: &mut f32,
    high: &mut f32,
    speed: f64,
    unit: &str,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut changed = false;
    for (label, value) in [("Low ≤", low), ("High ≥", &mut *high)] {
        ui.label(label);
        changed |= ui
            .add(
                egui::DragValue::new(value)
                    .speed(speed)
                    .range(range.clone())
                    .suffix(format!(" {unit}"))
                    .update_while_editing(false),
            )
            .changed();
    }
    changed
}

fn middle_name(mode: MiddleBand) -> &'static str {
    match mode {
        MiddleBand::Unknown => "Unknown between",
        MiddleBand::Hold => "Hold between",
        MiddleBand::Floating => "Floating between",
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct Settings {
    rules: Vec<Threshold>,
    shown: Vec<bool>,
    open: bool,
    decoders: crate::file_manager::logic_view::protocols::SessionSettings,
}

impl Digital {
    pub(in crate::file_manager) fn save(&self) -> Settings {
        Settings {
            rules: self.config.rules.clone(),
            shown: self.shown.clone(),
            open: self.open,
            decoders: self.panel.session_settings(),
        }
    }

    pub(in crate::file_manager) fn restore(&mut self, state: Settings) -> Result<(), String> {
        let count = self.config.lanes.len();
        if state.shown.len() != count
            || state.rules.len() != count
            || state.rules.iter().any(|rule| !rule.valid())
        {
            return Err("The saved thresholds do not match the recording".into());
        }
        self.panel.restore_session(
            state.decoders,
            None,
            &std::sync::atomic::AtomicBool::new(false),
        )?;
        let input = Arc::make_mut(&mut self.config);
        input.rules = state.rules;
        input.revision = input.revision.wrapping_add(1);
        self.shown = state.shown;
        self.open = state.open;
        Ok(())
    }
}
