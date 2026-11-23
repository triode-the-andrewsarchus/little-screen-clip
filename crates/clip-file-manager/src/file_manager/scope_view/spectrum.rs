use super::{
    Viewer, plot,
    spectrum_input::{Input, LaneKind, Region},
};
use crate::file_manager::signal_view::timeline::{self, Link};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    analysis::{SpectralSurvey, SpectrumWindow},
    scope::Channel,
    waveforms::{Peak, display_value},
};
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
        let target =
            bin.and_then(|bin| spectrum_readout(ui, survey, channel, bin, self.spectrum_db, link));
        if self.spectra.kind == LaneKind::Recorded {
            self.draw_filter_response(ui, rect, &span);
            let mut add = None;
            response.context_menu(|ui| {
                add = ui
                    .add_enabled_ui(self.can_add_filter(), super::filters::filter_menu)
                    .inner;
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
        match self.spectra.region {
            Region::Current => self.selection().or_else(|| Some(self.span.clone())),
            Region::Visible => Some(self.span.clone()),
            Region::Whole => None,
        }
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
            egui::ComboBox::from_id_salt("spectrum_region")
                .selected_text(self.spectra.region.label())
                .show_ui(ui, |ui| {
                    for region in [Region::Current, Region::Visible, Region::Whole] {
                        ui.selectable_value(&mut self.spectra.region, region, region.label());
                    }
                });
            egui::ComboBox::from_id_salt("scope_fft_window")
                .selected_text(window_name(self.spectrum_window))
                .show_ui(ui, |ui| {
                    for window in [
                        SpectrumWindow::Hann,
                        SpectrumWindow::Rectangular,
                        SpectrumWindow::BlackmanHarris,
                    ] {
                        ui.selectable_value(&mut self.spectrum_window, window, window_name(window));
                    }
                });
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

fn spectrum_rect(ui: &mut Ui) -> (Rect, egui::Response) {
    let height = (ui.available_height() - 29.0).max(65.0);
    let (area, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    (
        Rect::from_min_max(area.min + vec2(58.0, 7.0), area.max - vec2(8.0, 15.0)),
        response,
    )
}

struct Display<'a> {
    span: &'a mut Option<Range<f64>>,
    marker: &'a mut Option<f64>,
    db: bool,
    color: Color32,
}

impl Display<'_> {
    fn navigate(
        &mut self,
        ui: &Ui,
        rect: Rect,
        response: &egui::Response,
        survey: &SpectralSurvey,
        linked: bool,
    ) -> Range<f64> {
        let spectrum = &survey.spectrum;
        let last = spectrum.peak_amplitude.len().saturating_sub(1);
        let domain = 0.0..last as f64 * spectrum.bin_hz;
        let mut span = self.span.clone().map_or_else(
            || domain.clone(),
            |range| timeline::fit(&domain, range, spectrum.bin_hz * 0.25),
        );
        if let Some(next) = timeline::navigate(ui.ctx(), rect, response, &span, linked) {
            span = timeline::fit(&domain, next, spectrum.bin_hz * 0.25);
            *self.span = Some(span.clone());
        }
        if (response.clicked() || response.secondary_clicked())
            && let Some(pointer) = response.interact_pointer_pos()
            && rect.contains(pointer)
        {
            *self.marker = Some(plot::at(rect, &span, pointer.x));
        }
        span
    }

    fn paint(
        &self,
        ui: &Ui,
        rect: Rect,
        survey: &SpectralSurvey,
        span: &Range<f64>,
        unit: &str,
    ) -> Option<usize> {
        let spectrum = &survey.spectrum;
        let last = spectrum.peak_amplitude.len().saturating_sub(1);
        let hz = |bin| bin as f64 * spectrum.bin_hz;
        let bin = |frequency: f64| {
            (frequency / spectrum.bin_hz)
                .round()
                .clamp(0.0, last as f64) as usize
        };
        let first = bin(span.start);
        let stop = bin(span.end).saturating_add(1);
        let values = &spectrum.peak_amplitude[first..stop];
        let stride = values.len().div_ceil((rect.width() as usize).max(8)).max(1);
        let (peaks, bounds) = display_peaks(values, stride, self.db);
        let painter = ui.painter();
        let clip = painter.with_clip_rect(rect.intersect(ui.clip_rect()));
        let ink = ui.visuals().weak_text_color();
        clip.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        plot::labelled_x_axis(painter, rect, span, "Hz", ink);
        amplitude_ticks(
            painter,
            rect,
            bounds,
            if self.db { "dB" } else { unit },
            ink,
        );
        plot::draw_raw(
            &clip,
            rect,
            span,
            hz(first),
            stride as f64 * spectrum.bin_hz,
            &peaks,
            bounds,
            self.color,
        );
        if let Some(marker) = *self.marker {
            let mark = bin(marker);
            let x = plot::x_at(rect, span, hz(mark));
            clip.vline(x, rect.y_range(), Stroke::new(1.2, Color32::WHITE));
            return Some(mark);
        }
        (first.max(1)..stop)
            .max_by(|&a, &b| spectrum.peak_amplitude[a].total_cmp(&spectrum.peak_amplitude[b]))
    }
}

fn spectrum_readout(
    ui: &mut Ui,
    survey: &SpectralSurvey,
    channel: &Channel,
    bin: usize,
    db: bool,
    link: Option<&Link>,
) -> Option<Range<f64>> {
    let measured = survey.peak_interval(bin)?;
    let interval = channel.time_at(measured.start)..channel.time_at(measured.end);
    let offset = link.map_or(0.0, |link| link.offset);
    let amplitude = survey.spectrum.peak_amplitude[bin];
    let gain = if db {
        format!("{:.2} dB", amplitude_value(amplitude, true))
    } else {
        display_value(f64::from(amplitude), &channel.unit)
    };
    ui.horizontal_wrapped(|ui| {
        ui.monospace(format!(
            "{} · {gain}",
            display_value(bin as f64 * survey.spectrum.bin_hz, "Hz")
        ));
        let window = format!(
            "{} to {}",
            display_value(interval.start + offset, "s"),
            display_value(interval.end + offset, "s")
        );
        let reveal = ui
            .button("Reveal peak interval")
            .on_hover_text(window)
            .clicked();
        if survey.windows > 1 || survey.covered_samples < survey.requested_samples {
            ui.weak(format!(
                "{} windows · {:.1}% coverage",
                survey.windows,
                survey.covered_samples as f64 * 100.0 / survey.requested_samples as f64
            ));
        }
        reveal.then_some(interval)
    })
    .inner
}

fn window_name(window: SpectrumWindow) -> &'static str {
    match window {
        SpectrumWindow::Hann => "Hann",
        SpectrumWindow::Rectangular => "Rectangular",
        SpectrumWindow::BlackmanHarris => "Blackman–Harris",
    }
}

fn amplitude_value(value: f32, db: bool) -> f32 {
    if db {
        20.0 * value.max(1e-9).log10()
    } else {
        value
    }
}

fn display_peaks(values: &[f32], stride: usize, db: bool) -> (Vec<f32>, Peak) {
    let mut bounds = Peak::default();
    let peaks = values
        .chunks(stride)
        .map(|chunk| {
            let peak = amplitude_value(chunk.iter().copied().fold(0.0, f32::max), db);
            bounds.add(&[peak]);
            peak
        })
        .collect();
    if bounds.min == bounds.max {
        let padding = (bounds.min.abs() * 0.02).max(if db { 1.0 } else { 1e-9 });
        bounds.min -= padding;
        bounds.max += padding;
    }
    (peaks, bounds)
}

fn amplitude_ticks(
    painter: &egui::Painter,
    rect: egui::Rect,
    bounds: Peak,
    unit: &str,
    ink: Color32,
) {
    let divisions = (rect.height() / 27.0).clamp(2.0, 6.0);
    let raw_step = f64::from(bounds.max - bounds.min) / f64::from(divisions);
    let order = 10.0_f64.powf(raw_step.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .find(|&n| n * order >= raw_step)
        .unwrap_or(10.0)
        * order;
    if !step.is_finite() || step <= 0.0 {
        return;
    }
    let mut level = (f64::from(bounds.min) / step).ceil() * step;
    for _ in 0..=6 {
        if level > f64::from(bounds.max) {
            break;
        }
        let y = plot::y_at(rect, bounds, level as f32);
        painter.hline(
            rect.x_range(),
            y,
            Stroke::new(1.0, ink.gamma_multiply(0.21)),
        );
        let label = if unit == "dB" && step >= 1.0 {
            format!("{level:.0} dB")
        } else {
            display_value(level, unit)
        };
        painter.text(
            pos2(rect.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            label,
            FontId::monospace(10.0),
            ink,
        );
        level += step;
    }
}
