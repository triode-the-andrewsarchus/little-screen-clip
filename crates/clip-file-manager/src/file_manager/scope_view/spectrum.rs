use super::{
    Viewer, plot,
    spectrum_input::{Input, LaneKind},
};
use crate::file_manager::signal_view::{
    filter_editor::filter_menu,
    frequency_plot::{Display, choose_region, choose_window, spectrum_readout, spectrum_rect},
    timeline::{self, Link},
};
use egui::Ui;
use std::ops::Range;

impl Viewer {
    pub(super) fn spectrum_ui(&mut self, ui: &mut Ui, link: Option<&Link>) {
        let input = self.spectrum_input();
        self.spectrum_controls(ui, input.as_ref());
        let input = self.spectrum_input();
        let interval = self.spectrum_interval();
        let available = input.is_some();
        self.spectra
            .update(ui.ctx(), input, interval, self.spectrum_window);
        let Some((input, result)) = self.spectra.ready() else {
            ui.weak(if available {
                "Computing spectrum…"
            } else {
                "Calculating the selected lane…"
            });
            return;
        };
        let survey = match result {
            Ok(Some(value)) => value,
            Ok(None) => {
                ui.weak("No contiguous finite samples in this interval");
                return;
            }
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
        };
        let channel = input.channel();
        let (rect, response) = spectrum_rect(ui);
        let mut display = Display {
            span: &mut self.spectrum_span,
            marker: &mut self.spectrum_marker,
            db: self.spectrum_db,
            color: plot::color(self.active),
        };
        let span = display.navigate(ui, rect, &response, survey, link.is_some());
        let bin = display.paint(ui, rect, survey, &span, &channel.unit);
        let target = bin.and_then(|bin| {
            spectrum_readout(
                ui,
                survey,
                channel,
                bin,
                self.spectrum_db,
                link.map_or(0.0, |link| link.offset),
            )
        });
        if self.spectra.kind == LaneKind::Recorded {
            self.draw_filter_response(ui, rect, &span);
            let mut add = None;
            response.context_menu(|ui| {
                add = ui.add_enabled_ui(self.can_add_filter(), filter_menu).inner;
            });
            if let Some(kind) = add {
                self.add_filter(kind);
            }
        }
        if let Some(interval) = target {
            self.reveal_peak(interval, link);
        }
        response.on_hover_text(
            "Scroll to zoom in frequency · Middle drag to pan · Click to measure a frequency",
        );
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

    fn spectrum_controls(&mut self, ui: &mut Ui, input: Option<&Input>) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong(input.map_or("Spectrum", |input| input.channel().name.as_str()));
            egui::ComboBox::from_id_salt("spectrum_lane")
                .selected_text(self.spectra.kind.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.spectra.kind,
                        LaneKind::Recorded,
                        LaneKind::Recorded.label(),
                    );
                    ui.add_enabled_ui(self.math_state.numeric().is_some(), |ui| {
                        ui.selectable_value(
                            &mut self.spectra.kind,
                            LaneKind::Calculated,
                            LaneKind::Calculated.label(),
                        );
                    });
                    ui.add_enabled_ui(self.filters.has_lane(self.active), |ui| {
                        ui.selectable_value(
                            &mut self.spectra.kind,
                            LaneKind::Filtered,
                            LaneKind::Filtered.label(),
                        );
                    });
                });
            choose_region(ui, &mut self.spectra.region);
            choose_window(ui, &mut self.spectrum_window);
            ui.toggle_value(&mut self.spectrum_db, "dB")
                .on_hover_text("Amplitude relative to one peak unit");
            ui.toggle_value(&mut self.spectra.only, "Frequency only");
            if ui.button("Fit frequencies").clicked() {
                self.spectrum_span = None;
            }
        });
        if self.spectra.kind == LaneKind::Recorded {
            self.filter_controls(ui);
        }
    }

    fn reveal_peak(&mut self, interval: Range<f64>, link: Option<&Link>) {
        self.archive_mark();
        self.snaps.iter_mut().for_each(super::CursorSnap::clear);
        self.cursors = [Some(interval.start), Some(interval.end)];
        let padding = (interval.end - interval.start) * 0.08;
        let domain = link.map_or(&self.domain, |link| &link.domain);
        self.span = timeline::fit(
            domain,
            interval.start - padding..interval.end + padding,
            0.0,
        );
        self.spectra.only = false;
    }
}
