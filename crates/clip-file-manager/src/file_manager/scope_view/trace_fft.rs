use super::{Viewer, plot, spectrum_input};
use crate::file_manager::signal_view::frequency_plot::{Display, paint_measurement};
use egui::{Align2, Button, FontId, Rect, Sense, Ui, vec2};
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

    fn color(self) -> egui::Color32 {
        match self {
            Self::Recorded(index) => plot::color(index),
            Self::Math => super::MATH_COLOR,
            Self::Filtered(_) => super::filters::FILTER_COLOR,
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

    pub(super) fn paint_trace_fft(&mut self, ui: &mut Ui, rect: Rect, trace: Trace) {
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
            state.paint(
                ui,
                rect,
                trace,
                input,
                interval,
                self.spectrum_window,
                self.spectrum_db,
            );
        }
    }
}

impl TraceSpectrum {
    fn paint(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        trace: Trace,
        input: Option<spectrum_input::Input>,
        interval: Range<f64>,
        window: little_clip_core::engineering::analysis::SpectrumWindow,
        db: bool,
    ) {
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
        let color = trace.color();
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
        let label = match ready.map(|result| result.output) {
            Some(Err(error)) => error.to_owned(),
            Some(Ok(None)) => "No contiguous samples".into(),
            Some(Ok(Some(survey))) => cursor.map_or_else(String::new, |bin| {
                super::display_value(bin as f64 * survey.spectrum.bin_hz, "Hz")
            }),
            None => "Computing FFT…".into(),
        };
        if !label.is_empty() {
            ui.painter_at(plot).text(
                plot.right_top() + vec2(-6.0, 3.0),
                Align2::RIGHT_TOP,
                label,
                FontId::monospace(11.0),
                ui.visuals().text_color(),
            );
        }
    }
}
