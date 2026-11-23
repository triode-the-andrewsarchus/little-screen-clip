use super::{
    Viewer, plot,
    spectrum_input::{Input, LaneKind},
};
use crate::file_manager::signal_view::{
    filter_editor::{SpectrumPane, spectrum_workspace},
    frequency_plot::{Display, measurement_readout, paint_measurement, settings, spectrum_rect},
    timeline::Link,
};
use egui::{Button, Ui};
use std::ops::Range;

impl Viewer {
    pub(super) fn spectrum_ui(&mut self, ui: &mut Ui, link: Option<&Link>) {
        if self.spectra.kind == LaneKind::Filtered && !self.filters.has_lane(self.active) {
            self.set_spectrum_kind(LaneKind::Recorded);
        }
        self.spectrum_controls(ui);
        if !self.spectrum {
            self.spectra.suspend();
            return;
        }
        if link.is_some_and(|link| link.clock.scale() != 1.0) {
            ui.weak("Frequency uses the recording clock");
        }
        let input = self.spectrum_input();
        let interval = self.spectrum_interval();
        let available = input.is_some();
        self.spectra
            .update(ui.ctx(), input, interval, self.spectrum_window);
        let filterable = self.spectra.kind != LaneKind::Calculated;
        let height = if self.spectra.only {
            ui.available_height().clamp(245.0, 440.0)
        } else {
            190.0
        };
        spectrum_workspace(ui, filterable, height, |ui, part| match part {
            SpectrumPane::Chart => self.spectrum_chart(ui, link, available),
            SpectrumPane::Filters => self.filter_controls(ui),
        });
    }

    fn spectrum_chart(&mut self, ui: &mut Ui, link: Option<&Link>, available: bool) {
        let (rect, response) = spectrum_rect(ui, 145.0);
        let ready = self.spectra.ready();
        let shown = ready.or_else(|| self.spectra.latest());
        let rate = self.overview.channels[self.active].interval_seconds.recip();
        let mut span = self.spectrum_span.clone().unwrap_or(0.0..rate * 0.5);
        let color = plot::color(self.active);
        let mut display = Display {
            span: &mut self.spectrum_span,
            marker: &mut self.spectrum_marker,
            db: self.spectrum_db,
            color: if ready.is_some() {
                color
            } else {
                color.gamma_multiply(0.48)
            },
        };
        let cursor = paint_measurement(ui, rect, &response, &mut span, &mut display, shown);
        let pending = match (available, self.spectra.kind) {
            (true, _) => "Computing spectrum…",
            (false, LaneKind::Filtered) => "Updating filtered spectrum…",
            (false, _) => "Calculating the selected lane…",
        };
        let target = measurement_readout(
            ui,
            ready,
            cursor,
            self.spectrum_db,
            Link::clock(link),
            pending,
        );
        if self.spectra.kind != LaneKind::Calculated {
            self.draw_filter_response(ui, rect, &span);
        }
        if let Some(window) = target {
            let domain = link.map_or(&self.domain, |link| &link.domain);
            self.spectra.inspect(window, &mut self.span, domain, 0.0);
            ui.ctx().request_repaint();
        }
        response.on_hover_text(
            "Scroll to zoom in frequency · Middle drag to pan · Click to measure a frequency",
        );
    }

    pub(super) fn set_spectrum_kind(&mut self, kind: LaneKind) {
        if self.spectra.kind != kind {
            self.spectra.kind = kind;
            self.spectra.clear_result();
        }
    }

    pub(super) fn spectrum_interval(&self) -> Option<Range<f64>> {
        self.spectra.region.interval(self.selection(), &self.span)
    }

    fn spectrum_input(&self) -> Option<Input> {
        let channel = &self.overview.channels[self.active];
        match self.spectra.kind {
            LaneKind::Recorded => Some(Input::recorded(self.recording.clone(), self.active)),
            LaneKind::Calculated => {
                let (raw, _) = self.calculated()?;
                Some(Input::derived("Math".into(), self.math_unit(), raw.clone()))
            }
            LaneKind::Filtered => self.filters.input(self.active, &channel.unit),
        }
    }

    fn spectrum_controls(&mut self, ui: &mut Ui) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let name = if self.spectra.kind == LaneKind::Calculated {
                "Math"
            } else {
                &self.overview.channels[self.active].name
            };
            ui.strong(format!("{name} spectrum"));
            for (kind, available) in [
                (LaneKind::Recorded, true),
                (LaneKind::Calculated, self.math_state.numeric().is_some()),
                (LaneKind::Filtered, self.filters.has_lane(self.active)),
            ] {
                let button = Button::new(kind.label()).selected(self.spectra.kind == kind);
                if ui.add_enabled(available, button).clicked() {
                    self.set_spectrum_kind(kind);
                }
            }
            ui.separator();
            ui.toggle_value(&mut self.spectrum_db, "dB")
                .on_hover_text("Amplitude relative to one peak unit");
            ui.toggle_value(&mut self.spectra.only, "Frequency only");
            if ui.button("Fit frequencies").clicked() {
                self.spectrum_span = None;
            }
            if ui.button("Close").on_hover_text("Close spectrum").clicked() {
                self.spectrum = false;
            }
        });
        settings(ui, &mut self.spectra.region, &mut self.spectrum_window);
    }
}
