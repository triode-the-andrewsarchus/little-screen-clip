use super::{Arc, Digital, Recording, Ui};
use little_clip_core::engineering::analysis::{Cutoffs, MiddleBand, Threshold};
use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct ThresholdDraft {
    channel: usize,
    rule: Threshold,
}

impl Digital {
    pub(in crate::file_manager) fn toggle_lanes(&mut self, ui: &mut Ui) {
        if ui
            .add(egui::Button::new("Digital lanes").selected(self.open))
            .clicked()
        {
            self.open = !self.open;
        }
    }

    pub(in crate::file_manager) fn decoder_buttons(&mut self, ui: &mut Ui, span: &mut Range<f64>) {
        let mut use_sigrok = false;
        if self
            .panel
            .decoder_buttons_with(ui, &self.config.lanes, |ui| {
                if ui.button("Sigrok catalog…").clicked() {
                    use_sigrok = true;
                    ui.close();
                }
            })
        {
            self.close_sigrok(span);
            self.open = true;
            self.cursor = None;
            for index in self.panel.channels().into_iter().flatten() {
                if let Some(shown) = self.shown.get_mut(index) {
                    *shown = true;
                }
            }
        }
        if use_sigrok {
            self.panel.exit_inspection(span);
            self.panel.suspend_all();
            self.sigrok.open(ui.ctx());
            self.open = true;
        }
    }

    pub(super) fn lane_tools(&mut self, ui: &mut Ui, recording: &Recording, index: usize) {
        let channel = &recording.channels()[index];
        let editing = self
            .threshold_draft
            .is_some_and(|draft| draft.channel == index);
        let color = super::plot::color(index);
        ui.horizontal(|ui| {
            if ui
                .small_button(if self.shown[index] { "Hide" } else { "Show" })
                .clicked()
            {
                self.shown[index] = !self.shown[index];
                ui.ctx().request_repaint();
            }
            ui.colored_label(color, &self.config.lanes[index].name)
                .on_hover_text(format!(
                    "Use {}_D in a single-recording equation",
                    channel.name
                ));
            if ui
                .add(egui::Button::new("Thresholds").selected(editing).small())
                .clicked()
            {
                self.threshold_draft = (!editing).then_some(ThresholdDraft {
                    channel: index,
                    rule: self.config.rules[index],
                });
            }
        });
        if let Some(draft) = self.threshold_draft.filter(|draft| draft.channel == index) {
            self.thresholds(ui, draft, &channel.unit);
        }
    }

    fn thresholds(&mut self, ui: &mut Ui, mut draft: ThresholdDraft, unit: &str) {
        ui.group(|ui| {
            edit_cutoffs(ui, &mut draft.rule.cutoffs, unit);
            ui.horizontal_wrapped(|ui| {
                ui.label("Between cutoffs");
                for (band, label, help) in [
                    (
                        MiddleBand::Unknown,
                        "Unknown",
                        "Report an uncertain logic level",
                    ),
                    (
                        MiddleBand::Hold,
                        "Hold",
                        "Hold the previous state until the opposite cutoff is crossed",
                    ),
                    (
                        MiddleBand::Floating,
                        "Floating",
                        "Mark samples in this voltage band as floating",
                    ),
                ] {
                    ui.selectable_value(&mut draft.rule.middle, band, label)
                        .on_hover_text(help);
                }
                ui.checkbox(&mut draft.rule.invert, "Invert");
            });
            if !draft.rule.valid() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "Low must be at or below High. Decoding uses the last valid cutoffs.",
                );
            }
        });
        self.threshold_draft = Some(draft);
        if draft.rule.valid() && draft.rule != self.config.rules[draft.channel] {
            let config = Arc::make_mut(&mut self.config);
            config.rules[draft.channel] = draft.rule;
            config.revision = config.revision.wrapping_add(1);
            self.pending = None;
            self.ready = None;
            self.cursor = None;
            ui.ctx().request_repaint();
        }
    }
}

fn edit_cutoffs(ui: &mut Ui, cutoffs: &mut Cutoffs, unit: &str) {
    let mut adaptive = matches!(*cutoffs, Cutoffs::Adaptive { .. });
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut adaptive, true, "Adaptive")
            .on_hover_text(
                "Set cutoffs as percentages of the observed range in the analyzed window",
            );
        ui.selectable_value(&mut adaptive, false, "Fixed");
        if adaptive != matches!(*cutoffs, Cutoffs::Adaptive { .. }) {
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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct Settings {
    rules: Vec<Threshold>,
    shown: Vec<bool>,
    open: bool,
    decoders: crate::file_manager::logic_view::protocols::SessionSettings,
}

impl Digital {
    pub(in crate::file_manager) fn analysis_edits(&self) -> Settings {
        let mut settings = self.save();
        settings.open = false;
        settings.shown.clear();
        settings.decoders = settings.decoders.analysis_edits();
        settings
    }

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
        self.threshold_draft = None;
        Ok(())
    }
}
