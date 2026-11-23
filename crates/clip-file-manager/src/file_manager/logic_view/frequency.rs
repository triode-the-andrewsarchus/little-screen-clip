use super::Viewer;
use crate::file_manager::{
    scope_view::plot,
    signal_view::{
        frequency_plot::{Display, choose_region, choose_window, spectrum_readout, spectrum_rect},
        spectra::{Input, Spectra},
        timeline::Link,
    },
};
use egui::Ui;
use little_clip_core::engineering::{analysis::SpectrumWindow, waveforms::display_value};
use std::{ops::Range, time::Duration};

#[derive(Default)]
pub(super) struct Frequency {
    pub visible: bool,
    spectra: Spectra,
    window: SpectrumWindow,
    span: Option<Range<f64>>,
    marker: Option<f64>,
    selected_lane: usize,
    sampled_rate: Option<f64>,
}

impl Viewer {
    pub(super) fn frequency(&mut self, ui: &mut Ui, link: Option<&Link>) {
        let index = self.active;
        let lane = &self.overview.lanes[index];
        let default_rate = (32_768.0 / (self.span.end - self.span.start)).clamp(1.0, 1e12);
        let panel = &mut self.frequency;
        panel.controls(ui, index, lane, default_rate);
        let rate = lane
            .sample_rate
            .or(panel.sampled_rate)
            .unwrap_or(default_rate);
        let domain = lane.start_seconds..lane.end_seconds.max(lane.start_seconds + rate.recip());
        let domain = if lane.sample_rate.is_none() {
            self.overview.span.clone()
        } else {
            domain
        };
        let input = Input::logic(self.recording.clone(), index, &lane.name, domain, rate);
        let interval = panel
            .spectra
            .region
            .interval(self.selected.clone(), &self.span);
        panel
            .spectra
            .update(ui.ctx(), Some(input), interval, panel.window);
        if let Some(interval) = panel.plot(ui, link) {
            self.frequency.visible = false;
            self.focus_time(interval.start, interval.end);
        }
    }
}

impl Frequency {
    fn controls(
        &mut self,
        ui: &mut Ui,
        index: usize,
        lane: &little_clip_core::engineering::logic::Lane,
        default_rate: f64,
    ) {
        if index != self.selected_lane {
            self.selected_lane = index;
            self.span = None;
            self.marker = None;
            self.sampled_rate = None;
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} spectrum", lane.name));
            choose_region(ui, &mut self.spectra.region);
            choose_window(ui, &mut self.window);
            if lane.sample_rate.is_none() {
                let rate = self.sampled_rate.get_or_insert(default_rate);
                let speed = (*rate * 0.005).max(1.0);
                ui.label("Analysis rate");
                ui.add(
                    egui::DragValue::new(rate)
                        .range(1e-6..=1e15)
                        .speed(speed)
                        .suffix(" Hz"),
                );
            } else {
                ui.weak(display_value(lane.sample_rate.unwrap_or_default(), "Hz"));
            }
            if ui.button("Fit frequencies").clicked() {
                self.span = None;
            }
        });
    }

    fn plot(&mut self, ui: &mut Ui, link: Option<&Link>) -> Option<Range<f64>> {
        let Some((input, output)) = self.spectra.ready() else {
            ui.weak("Computing spectrum…");
            if self.spectra.busy() {
                ui.ctx().request_repaint_after(Duration::from_millis(33));
            }
            return None;
        };
        let survey = match output {
            Ok(Some(survey)) => survey,
            Ok(None) => {
                ui.weak("No contiguous known levels in this interval");
                return None;
            }
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return None;
            }
        };
        let (rect, response) = spectrum_rect(ui);
        let mut plot = Display {
            span: &mut self.span,
            marker: &mut self.marker,
            db: true,
            color: plot::color(2),
        };
        let span = plot.navigate(ui, rect, &response, survey, link.is_some());
        let bin = plot.paint(ui, rect, survey, &span, "")?;
        spectrum_readout(
            ui,
            survey,
            input.channel(),
            bin,
            true,
            link.map_or(0.0, |link| link.offset),
        )
    }
}
