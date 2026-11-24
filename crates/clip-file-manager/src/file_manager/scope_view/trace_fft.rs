use super::{Viewer, plot, spectrum_input};
use crate::file_manager::signal_view::{
    filter_editor::filter_ink,
    frequency_plot::{Display, paint_measurement, peak_reading, time_range},
    spectra::ResultView,
};
use egui::{Button, Label, Rect, Sense, Ui, vec2};
use little_clip_core::engineering::analysis::{Clock, SpectralSurvey, SpectrumWindow};
use little_clip_core::engineering::waveforms::display_value;
use std::ops::Range;

#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub(super) enum Trace {
    Recorded(usize),
    Math,
    Filtered(usize),
}

impl Trace {
    pub fn valid(self, channels: usize) -> bool {
        match self {
            Self::Recorded(index) | Self::Filtered(index) => index < channels,
            Self::Math => true,
        }
    }

    fn color(self, ui: &Ui) -> egui::Color32 {
        match self {
            Self::Recorded(index) => plot::color(index),
            Self::Math => super::MATH_COLOR,
            Self::Filtered(_) => filter_ink(ui.visuals().dark_mode),
        }
    }
}

#[derive(Default)]
pub(super) struct TraceSpectrum {
    spectra: spectrum_input::Spectra,
    frequency: Option<Range<f64>>,
    marker: Option<f64>,
}

impl TraceSpectrum {
    pub fn suspend(&mut self) {
        self.spectra.suspend();
    }
}

impl Viewer {
    pub(super) fn trace_fft_toggle(&self, ui: &mut Ui, trace: Trace) -> bool {
        let fft = self.trace_fft.contains_key(&trace);
        ui.add(Button::new(if fft { "Waveform" } else { "FFT" }).small())
            .on_hover_text(if fft {
                "Show this trace in the time domain"
            } else {
                "Show the FFT of this trace over the selected or visible time range"
            })
            .clicked()
    }

    pub(super) fn toggle_trace_fft(&mut self, trace: Trace) {
        if self.trace_fft.remove(&trace).is_none() {
            self.trace_fft.insert(trace, TraceSpectrum::default());
        }
    }

    pub(in crate::file_manager) fn inline_fft_evidence(&self) -> Option<&Range<f64>> {
        self.inline_fft_evidence.as_ref()
    }

    pub(super) fn paint_trace_fft(
        &mut self,
        ui: &mut Ui,
        heading: &mut Ui,
        rect: Rect,
        trace: Trace,
        clock: Clock,
    ) {
        let interval = self.selection().unwrap_or_else(|| self.span.clone());
        let input = match trace {
            Trace::Recorded(index) => Some(spectrum_input::Input::recorded(
                self.recording.clone(),
                index,
            )),
            Trace::Math => self.calculated().map(|(raw, _)| {
                spectrum_input::Input::derived("Math".into(), self.math_unit(), raw.clone())
            }),
            Trace::Filtered(index) => self
                .filters
                .input(index, &self.overview.channels[index].unit),
        };
        if let Some(state) = self.trace_fft.get_mut(&trace) {
            let evidence = state.paint(
                ui,
                heading,
                rect,
                trace,
                input,
                interval,
                self.spectrum_window,
                self.spectrum_db,
                clock,
            );
            if evidence.is_some() {
                self.inline_fft_evidence = evidence;
            }
        }
    }
}

impl TraceSpectrum {
    fn paint(
        &mut self,
        ui: &mut Ui,
        heading: &mut Ui,
        rect: Rect,
        trace: Trace,
        input: Option<spectrum_input::Input>,
        interval: Range<f64>,
        window: SpectrumWindow,
        db: bool,
        clock: Clock,
    ) -> Option<Range<f64>> {
        self.spectra.update(ui.ctx(), input, Some(interval), window);
        let plot = Rect::from_min_max(rect.min, rect.max - vec2(0.0, 14.0));
        let response = ui.interact(
            plot,
            ui.make_persistent_id(("trace-fft", trace)),
            Sense::click_and_drag(),
        );
        let ready = self.spectra.ready();
        let shown = ready.or_else(|| self.spectra.latest());
        let mut span = self.frequency.clone().unwrap_or(0.0..1.0);
        let color = trace.color(ui);
        let mut display = Display {
            span: &mut self.frequency,
            marker: &mut self.marker,
            db,
            color: if ready.is_some() {
                color
            } else {
                color.gamma_multiply(0.48)
            },
        };
        let cursor = paint_measurement(ui, plot, &response, &mut span, &mut display, shown);
        match ready {
            Some(
                result @ ResultView {
                    output: Ok(Some(survey)),
                    ..
                },
            ) => peak_window_readout(heading, result, survey, cursor?, db, clock),
            Some(ResultView {
                output: Err(error), ..
            }) => {
                heading.colored_label(heading.visuals().error_fg_color, error);
                None
            }
            Some(ResultView {
                output: Ok(None), ..
            }) => {
                heading.weak("No contiguous samples");
                None
            }
            None => {
                heading.weak("Computing FFT…");
                None
            }
        }
    }
}

fn peak_window_readout(
    ui: &mut Ui,
    result: ResultView<'_>,
    survey: &SpectralSurvey,
    bin: usize,
    db: bool,
    clock: Clock,
) -> Option<Range<f64>> {
    let (peak, gain) = peak_reading(result, survey, bin, db)?;
    let channel = result.input.channel();
    let input = channel.time_at(result.rows.start)..channel.time_at(result.rows.end);
    let measured = time_range(&peak.interval, clock);
    let coverage = if survey.covered_samples < survey.requested_samples {
        " · partial coverage"
    } else {
        ""
    };
    let frequency = display_value(peak.hz, "Hz");
    let label = format!("{frequency} · {gain} · Peak window {measured}{coverage}");
    let response = ui.add(
        Label::new(&label)
            .truncate()
            .show_tooltip_when_elided(false),
    );
    if response.hovered() {
        response.on_hover_ui(|ui| {
            ui.label(label);
            ui.label(format!("FFT input {}", time_range(&input, clock)));
            ui.label(format!(
                "{} of {} samples across {} windows",
                survey.covered_samples, survey.requested_samples, survey.windows,
            ));
            if clock.scale() != 1.0 {
                ui.weak("Frequency uses the recording clock");
            }
        });
        Some(peak.interval)
    } else {
        None
    }
}
