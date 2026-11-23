use super::super::spectra::DisplaySettings;
use super::{
    compute::{Frame, Lane},
    filtering::{Filters, PlotLane},
};
use crate::file_manager::{
    scope_view::plot,
    signal_view::{
        filter_editor::{FILTER_COLOR, filter_menu},
        frequency_plot::{Display, choose_region, choose_window, spectrum_readout, spectrum_rect},
        spectra::{ResultView, Spectra},
    },
};
use egui::{Rect, Response, Ui};
use little_clip_core::engineering::{
    analysis::{FilterKind, SpectralSurvey, SpectrumWindow},
    scope::Channel,
};
use std::ops::Range;

#[derive(Default)]
pub(super) struct Frequency {
    lane: Option<usize>,
    window: SpectrumWindow,
    spectra: Spectra,
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

    pub(super) fn button(&mut self, ui: &mut Ui, index: usize) {
        let selected = self.lane == Some(index);
        if ui
            .add(egui::Button::new("Spectrum").small().selected(selected))
            .clicked()
        {
            self.lane = (!selected).then_some(index);
            self.span = None;
            self.marker = None;
            self.filtered_spectrum = false;
            self.spectra.clear_result();
            self.reveal = !selected;
        }
    }

    pub(super) fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal)
    }

    pub(super) fn filtered_lanes(&self, frame: &Frame) -> Vec<PlotLane<'_>> {
        self.filters.plots(frame)
    }

    pub(super) fn only(&self) -> bool {
        self.lane.is_some() && self.spectra.only
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        frame: &Frame,
        selected: Option<Range<f64>>,
        visible: &Range<f64>,
    ) -> Option<Range<f64>> {
        let index = self.lane?;
        let lane = frame.lanes.get(index)?;
        let rate = frame.step.recip();
        let interval = self.measurement_controls(ui, lane, index, rate, selected, visible);
        self.lane?;
        let input = if self.filtered_spectrum {
            self.filters
                .lanes()
                .iter()
                .find(|output| output.index == index)
                .map(|output| output.lane.spectrum_input())
        } else {
            Some(lane.spectrum_input())
        };
        let expected = if self.filtered_spectrum {
            format!("{} filtered", lane.name())
        } else {
            lane.name().to_owned()
        };
        self.spectra
            .update(ui.ctx(), input, interval.clone(), self.window);
        let ready = self.spectra.ready();
        let current = ready.is_some();
        let shown = ready.or_else(|| {
            self.spectra
                .latest()
                .filter(|(input, _)| input.channel().name == expected)
        });
        let pending = if !self.filtered_spectrum {
            "Computing spectrum…"
        } else if self.filters.error().is_some() {
            "Filtered spectrum unavailable"
        } else if !self.filters.has_samples(index, frame) {
            "No samples from the filter interval in this view"
        } else {
            "Updating filtered spectrum…"
        };
        let mut add = None;
        let target = ui
            .allocate_ui(egui::vec2(ui.available_width(), 215.0), |ui| {
                let (rect, response) = spectrum_rect(ui, 65.0);
                let mut span = self.span.clone().unwrap_or(0.0..rate * 0.5);
                let color = if self.filtered_spectrum {
                    FILTER_COLOR
                } else {
                    plot::color(index + 1)
                };
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
                if matches!(lane, Lane::Analog { .. }) {
                    add = self
                        .filters
                        .edit_plot(ui, index, rect, &response, &span, rate);
                }
                measurement_readout(ui, ready, cursor, !self.linear, pending)
            })
            .inner;
        if let Some((kind, anchor)) = add {
            self.add_filter(ui, index, kind, rate, interval, anchor.or(self.marker));
        }
        if target.is_some() {
            self.spectra.only = false;
        }
        target
    }

    fn measurement_controls(
        &mut self,
        ui: &mut Ui,
        lane: &Lane,
        index: usize,
        rate: f64,
        selected: Option<Range<f64>>,
        visible: &Range<f64>,
    ) -> Option<Range<f64>> {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} spectrum", lane.name()));
            choose_region(ui, &mut self.spectra.region);
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
        let interval = self.spectra.region.interval(selected, visible);
        if matches!(lane, Lane::Analog { .. }) {
            self.filter_controls(ui, index, rate, &interval);
        }
        interval
    }

    fn filter_controls(
        &mut self,
        ui: &mut Ui,
        index: usize,
        rate: f64,
        interval: &Option<Range<f64>>,
    ) {
        ui.horizontal_wrapped(|ui| {
            let can_add = self
                .filters
                .settings
                .get(&index)
                .is_none_or(|settings| settings.stages.can_add());
            let kind = ui
                .add_enabled_ui(can_add, |ui| {
                    ui.menu_button("Add filter", filter_menu).inner.flatten()
                })
                .inner;
            if let Some(kind) = kind {
                self.add_filter(ui, index, kind, rate, interval.clone(), self.marker);
            }
            let Some(settings) = self.filters.settings.get_mut(&index) else {
                return;
            };
            let previous = settings.stages.clone();
            settings.stages.controls(ui, rate);
            if previous != settings.stages {
                ui.ctx().request_repaint();
            }
            if settings.stages.is_empty() {
                self.filters.settings.remove(&index);
                self.filtered_spectrum = false;
                return;
            }
            if ui
                .add(egui::Button::new("Filtered spectrum").selected(self.filtered_spectrum))
                .clicked()
            {
                self.filtered_spectrum = !self.filtered_spectrum;
            }
            if ui
                .add_enabled(
                    settings.interval != *interval,
                    egui::Button::new("Use spectrum interval"),
                )
                .clicked()
            {
                settings.interval = interval.clone();
                ui.ctx().request_repaint();
            }
            if let Some(error) = self.filters.error() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }

    fn add_filter(
        &mut self,
        ui: &Ui,
        index: usize,
        kind: FilterKind,
        rate: f64,
        interval: Option<Range<f64>>,
        marker: Option<f64>,
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
        settings.stages.add(kind, rate, &span, marker);
        ui.ctx().request_repaint();
    }
}

type Cursor<'a> = (&'a SpectralSurvey, &'a Channel, usize);

fn paint_measurement<'a>(
    ui: &mut Ui,
    rect: Rect,
    response: &Response,
    span: &mut Range<f64>,
    display: &mut Display<'_>,
    shown: Option<ResultView<'a>>,
) -> Option<Cursor<'a>> {
    let Some((input, Ok(Some(survey)))) = shown else {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        return None;
    };
    let channel = input.channel();
    *span = display.navigate(ui, rect, response, survey, true);
    let bin = display.paint(ui, rect, survey, span, &channel.unit)?;
    Some((survey, channel, bin))
}

fn measurement_readout(
    ui: &mut Ui,
    ready: Option<ResultView<'_>>,
    cursor: Option<Cursor<'_>>,
    db: bool,
    pending: &str,
) -> Option<Range<f64>> {
    match ready {
        Some((_, Ok(Some(_)))) => {
            let (survey, channel, bin) = cursor?;
            return spectrum_readout(ui, survey, channel, bin, db, 0.0);
        }
        Some((_, Ok(None))) => {
            ui.weak("No contiguous finite samples in this interval");
        }
        Some((_, Err(error))) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        None => {
            ui.weak(pending);
        }
    }
    None
}
