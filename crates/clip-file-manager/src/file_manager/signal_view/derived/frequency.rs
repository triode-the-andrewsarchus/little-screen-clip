use super::super::spectra::DisplaySettings;
use super::{
    compute::{Frame, Lane},
    display::Trace,
    filtering::Filters,
};
use crate::file_manager::signal_view::{
    filter_editor::{SpectrumPane, add_buttons, reveal_frequency, spectrum_workspace},
    frequency_plot::{Display, measurement_readout, paint_measurement, settings, spectrum_rect},
    spectra::{PeakWindow, Spectra},
};
use egui::Ui;
use little_clip_core::engineering::{
    analysis::{FilterKind, SpectrumWindow},
    waveforms::display_value,
};
use std::ops::Range;

#[derive(Default)]
pub(super) struct Frequency {
    lane: Option<usize>,
    window: SpectrumWindow,
    pub(super) spectra: Spectra,
    span: Option<Range<f64>>,
    marker: Option<f64>,
    linear: bool,
    filters: Filters,
    filtered_spectrum: bool,
    reveal: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SavedFrequency {
    lane: Option<usize>,
    window: SpectrumWindow,
    spectra: DisplaySettings,
    span: Option<Range<f64>>,
    marker: Option<f64>,
    linear: bool,
    filters: std::collections::BTreeMap<usize, super::filtering::Settings>,
    filtered_spectrum: bool,
}

impl Frequency {
    pub(super) fn filter_settings(
        &self,
    ) -> &std::collections::BTreeMap<usize, super::filtering::Settings> {
        &self.filters.settings
    }

    pub(super) fn save(&self) -> SavedFrequency {
        SavedFrequency {
            lane: self.lane,
            window: self.window,
            spectra: self.spectra.settings(),
            span: self.span.clone(),
            marker: self.marker,
            linear: self.linear,
            filters: self.filters.settings.clone(),
            filtered_spectrum: self.filtered_spectrum,
        }
    }

    pub(super) fn restore(&mut self, state: SavedFrequency, lanes: usize) -> Result<(), String> {
        if state.lane.is_some_and(|index| index >= lanes)
            || state.filters.keys().any(|&key| key >= lanes)
        {
            return Err("Invalid derived spectrum lane".into());
        }
        for settings in state.filters.values() {
            settings.stages.validate()?;
            if settings
                .interval
                .as_ref()
                .is_some_and(|span| span.is_empty())
            {
                return Err("Invalid derived filter interval".into());
            }
        }
        self.lane = state.lane;
        self.window = state.window;
        self.spectra.restore_settings(state.spectra);
        self.span = state.span.filter(|span| !span.is_empty());
        self.marker = state.marker;
        self.linear = state.linear;
        self.filters.settings = state.filters;
        self.filtered_spectrum = state.filtered_spectrum;
        Ok(())
    }

    pub(super) fn update_filters(&mut self, ui: &Ui, frame: &Frame) {
        self.filters.update(ui.ctx(), frame);
    }

    pub(super) fn remap(&mut self, lanes: &[Option<usize>]) {
        self.lane = self
            .lane
            .and_then(|index| lanes.get(index).copied().flatten());
        self.spectra.clear_result();
        self.filters.remap(lanes);
    }

    pub(super) fn button(&mut self, ui: &mut Ui, trace: Trace) {
        let index = trace.index();
        let filtered = matches!(trace, Trace::Filtered(_));
        let selected = self.lane == Some(index) && self.filtered_spectrum == filtered;
        let label = if filtered { "Edit filters" } else { "Spectrum" };
        if ui
            .add(egui::Button::new(label).small().selected(selected))
            .clicked()
        {
            if !selected || !filtered {
                self.lane = (!selected).then_some(index);
                self.span = None;
                self.marker = None;
                self.filtered_spectrum = filtered;
                self.spectra.clear_result();
            }
            if filtered {
                self.spectra.only = false;
            }
            self.reveal = self.lane.is_some();
        }
    }

    pub(super) fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal)
    }

    pub(super) fn filtered_lane(&self, index: usize) -> Option<&Lane> {
        self.filters.lane(index)
    }

    pub(super) fn filtered_status(&self, frame: &Frame, index: usize) -> &'static str {
        if self.filters.error().is_some() {
            "Filtered samples unavailable"
        } else if !self.filters.has_samples(index, frame) {
            "Outside filter interval"
        } else {
            "Filtering…"
        }
    }

    pub(super) fn origin_trace(&self) -> Option<Trace> {
        self.spectra.origin()?;
        let index = self.lane?;
        Some(if self.filtered_spectrum {
            Trace::Filtered(index)
        } else {
            Trace::Equation(index)
        })
    }

    pub(super) fn only(&self) -> bool {
        self.lane.is_some() && self.spectra.only && self.spectra.origin().is_none()
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        frame: &Frame,
        selected: Option<Range<f64>>,
        visible: &Range<f64>,
    ) -> Option<PeakWindow> {
        if self.spectra.origin().is_some() {
            return None;
        }
        let index = self.lane?;
        let lane = frame.lanes.get(index)?;
        self.filtered_spectrum &=
            matches!(lane, Lane::Analog { .. }) && self.filters.settings.contains_key(&index);
        let interval = self.measurement_controls(ui, lane, index, selected, visible);
        self.lane?;
        let rate = frame.step.recip();
        let mut added = false;
        let mut target = None;
        spectrum_workspace(
            ui,
            matches!(lane, Lane::Analog { .. }),
            215.0,
            |ui, part| match part {
                SpectrumPane::Chart => target = self.chart(ui, frame, index, interval.clone()),
                SpectrumPane::Filters => added = self.filter_controls(ui, index, rate, &interval),
            },
        );
        self.reveal |= added;
        target
    }

    fn chart(
        &mut self,
        ui: &mut Ui,
        frame: &Frame,
        index: usize,
        interval: Option<Range<f64>>,
    ) -> Option<PeakWindow> {
        let lane = frame.lanes.get(index)?;
        let rate = frame.step.recip();
        let input = if self.filtered_spectrum {
            self.filtered_lane(index).map(Lane::spectrum_input)
        } else {
            Some(lane.spectrum_input())
        };
        let expected = if self.filtered_spectrum {
            format!("{} filtered", lane.name())
        } else {
            lane.name().to_owned()
        };
        self.spectra.update(ui.ctx(), input, interval, self.window);
        let ready = self.spectra.ready();
        let current = ready.is_some();
        let shown = ready.or_else(|| {
            self.spectra
                .latest()
                .filter(|result| result.input.channel().name == expected)
        });
        let pending = if !self.filtered_spectrum {
            "Computing spectrum…"
        } else if self.filtered_lane(index).is_none() {
            self.filtered_status(frame, index)
        } else {
            "Updating filtered spectrum…"
        };
        let (rect, response) = spectrum_rect(ui, 65.0);
        let mut span = self.span.clone().unwrap_or(0.0..rate * 0.5);
        let color = Trace::Equation(index).color(ui);
        let mut display = Display {
            span: &mut self.span,
            marker: &mut self.marker,
            db: !self.linear,
            color: if current {
                color
            } else {
                color.gamma_multiply(0.48)
            },
        };
        let cursor = paint_measurement(ui, rect, &response, &mut span, &mut display, shown);
        if let Some(settings) = self.filters.settings.get_mut(&index) {
            ui.push_id(("equation-stage-plot", index), |ui| {
                settings
                    .stages
                    .draw(ui, &mut settings.editor, rect, &span, rate);
            });
        }
        measurement_readout(ui, ready, cursor, !self.linear, Default::default(), pending)
    }

    fn measurement_controls(
        &mut self,
        ui: &mut Ui,
        lane: &Lane,
        index: usize,
        selected: Option<Range<f64>>,
        visible: &Range<f64>,
    ) -> Option<Range<f64>> {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} spectrum", lane.name()));
            if self.filters.settings.contains_key(&index) {
                for (filtered, label) in [(false, "Original"), (true, "Filtered")] {
                    if ui
                        .add(egui::Button::new(label).selected(self.filtered_spectrum == filtered))
                        .clicked()
                    {
                        self.filtered_spectrum = filtered;
                    }
                }
            }
            if ui
                .button(if self.linear { "Linear" } else { "dB" })
                .clicked()
            {
                self.linear = !self.linear;
            }
            if ui.button("Fit frequencies").clicked() {
                self.span = None;
            }
            let only = self.spectra.only;
            if ui
                .add(egui::Button::new("Frequency only").selected(only))
                .clicked()
            {
                self.spectra.only = !only;
            }
            if ui.button("Close spectrum").clicked() {
                self.lane = None;
                self.spectra.suspend();
            }
        });
        settings(ui, &mut self.spectra.region, &mut self.window);
        self.spectra.region.interval(selected, visible)
    }

    fn filter_controls(
        &mut self,
        ui: &mut Ui,
        index: usize,
        rate: f64,
        interval: &Option<Range<f64>>,
    ) -> bool {
        let can_add = self
            .filters
            .settings
            .get(&index)
            .is_none_or(|settings| settings.stages.can_add());
        let added = ui.horizontal_wrapped(|ui| add_buttons(ui, can_add)).inner;
        if let Some(kind) = added {
            self.add_filter(ui, index, kind, rate, interval.clone());
        }
        let Some(settings) = self.filters.settings.get_mut(&index) else {
            return false;
        };
        let previous = settings.stages.clone();
        ui.push_id(("equation-stage-editor", index), |ui| {
            if let Some(frequency) = settings.stages.controls(ui, &mut settings.editor, rate) {
                reveal_frequency(&mut self.span, frequency, rate);
            }
        });
        if previous != settings.stages {
            ui.ctx().request_repaint();
        }
        if settings.stages.is_empty() {
            self.filters.settings.remove(&index);
            self.filtered_spectrum = false;
            return false;
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(range) = &settings.interval {
                ui.weak(format!(
                    "Filter range  {} to {}",
                    display_value(range.start, "s"),
                    display_value(range.end, "s")
                ));
            } else {
                ui.weak("Filter range  Available samples");
            }
            if ui
                .add_enabled(
                    settings.interval != *interval,
                    egui::Button::new("Use spectrum range").small(),
                )
                .clicked()
            {
                settings.interval = interval.clone();
                ui.ctx().request_repaint();
            }
        });
        if let Some(error) = self.filters.error() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        added.is_some()
    }

    fn add_filter(
        &mut self,
        ui: &Ui,
        index: usize,
        kind: FilterKind,
        rate: f64,
        interval: Option<Range<f64>>,
    ) {
        let settings =
            self.filters
                .settings
                .entry(index)
                .or_insert_with(|| super::filtering::Settings {
                    interval,
                    ..Default::default()
                });
        let span = self.span.clone().unwrap_or(0.0..rate * 0.5);
        if let Some(stage) = settings.stages.add(kind, rate, &span, self.marker) {
            settings.editor.select(stage);
        }
        ui.ctx().request_repaint();
    }
}
