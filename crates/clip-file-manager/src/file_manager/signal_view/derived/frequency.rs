use super::compute::Frame;
use crate::file_manager::{
    scope_view::plot,
    signal_view::{
        frequency_plot::{Display, choose_window, spectrum_readout, spectrum_rect},
        spectra::Spectra,
    },
};
use egui::Ui;
use little_clip_core::engineering::analysis::SpectrumWindow;
use std::ops::Range;

#[derive(Default)]
pub(super) struct Frequency {
    lane: Option<usize>,
    window: SpectrumWindow,
    spectra: Spectra,
    span: Option<Range<f64>>,
    marker: Option<f64>,
    linear: bool,
}

impl Frequency {
    pub(super) fn controls(&mut self, ui: &mut Ui, frame: &Frame) {
        ui.menu_button("Spectrum", |ui| {
            if ui.selectable_value(&mut self.lane, None, "Off").clicked() {
                self.spectra.suspend();
                ui.close();
            }
            for (index, lane) in frame.lanes.iter().enumerate() {
                if ui
                    .selectable_value(&mut self.lane, Some(index), lane.name())
                    .clicked()
                {
                    self.span = None;
                    self.marker = None;
                    ui.close();
                }
            }
        });
        if self.lane.is_some() {
            ui.toggle_value(&mut self.spectra.only, "Frequency only");
        }
    }

    pub(super) fn only(&self) -> bool {
        self.lane.is_some() && self.spectra.only
    }

    pub(super) fn content(&mut self, ui: &mut Ui, frame: &Frame) -> Option<Range<f64>> {
        let index = self.lane?;
        let lane = frame.lanes.get(index)?;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} spectrum", lane.name()));
            choose_window(ui, &mut self.window);
            if ui
                .button(if self.linear { "Linear" } else { "dB" })
                .clicked()
            {
                self.linear = !self.linear;
            }
            if ui.button("Fit frequencies").clicked() {
                self.span = None;
            }
        });
        self.spectra
            .update(ui.ctx(), Some(lane.spectrum_input()), None, self.window);
        let Some((input, result)) = self.spectra.ready() else {
            ui.weak("Computing spectrum…");
            if self.spectra.busy() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(33));
            }
            return None;
        };
        let survey = match result {
            Ok(Some(survey)) => survey,
            Ok(None) => {
                ui.weak("No contiguous finite samples in this lane");
                return None;
            }
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return None;
            }
        };
        let size = egui::vec2(ui.available_width(), 215.0);
        let channel = input.channel();
        let target = ui
            .allocate_ui(size, |ui| {
                let (rect, response) = spectrum_rect(ui);
                let mut plot = Display {
                    span: &mut self.span,
                    marker: &mut self.marker,
                    db: !self.linear,
                    color: plot::color(index + 1),
                };
                let span = plot.navigate(ui, rect, &response, survey, true);
                let bin = plot.paint(ui, rect, survey, &span, &channel.unit)?;
                spectrum_readout(ui, survey, channel, bin, !self.linear, 0.0)
            })
            .inner;
        if target.is_some() {
            self.spectra.only = false;
        }
        target
    }
}
