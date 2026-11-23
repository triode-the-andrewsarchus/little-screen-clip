use super::{
    spectra::{Region, ResultView},
    timeline,
};
use crate::file_manager::scope_view::plot;
use egui::{Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    analysis::{Clock, SpectralSurvey, SpectrumWindow},
    scope::Channel,
    waveforms::{Peak, display_value},
};
use std::ops::Range;

pub(in crate::file_manager) fn spectrum_rect(
    ui: &mut Ui,
    min_height: f32,
) -> (Rect, egui::Response) {
    let height = (ui.available_height() - 29.0).max(min_height);
    let (area, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    (
        Rect::from_min_max(area.min + vec2(58.0, 7.0), area.max - vec2(8.0, 15.0)),
        response,
    )
}

pub(in crate::file_manager) struct Display<'a> {
    pub span: &'a mut Option<Range<f64>>,
    pub marker: &'a mut Option<f64>,
    pub db: bool,
    pub color: Color32,
}

impl Display<'_> {
    pub fn navigate(
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

    pub fn paint(
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

pub(in crate::file_manager) fn spectrum_readout(
    ui: &mut Ui,
    survey: &SpectralSurvey,
    channel: &Channel,
    bin: usize,
    db: bool,
    clock: Clock,
) -> Option<Range<f64>> {
    let measured = survey.peak_interval(bin)?;
    let interval = channel.time_at(measured.start)..channel.time_at(measured.end);
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
            display_value(clock.to_workspace(interval.start), "s"),
            display_value(clock.to_workspace(interval.end), "s")
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

pub(in crate::file_manager) fn choose_window(ui: &mut Ui, window: &mut SpectrumWindow) {
    egui::ComboBox::from_id_salt("dsp-fft-window")
        .selected_text(window_name(*window))
        .show_ui(ui, |ui| {
            for candidate in [
                SpectrumWindow::Hann,
                SpectrumWindow::Rectangular,
                SpectrumWindow::BlackmanHarris,
            ] {
                ui.selectable_value(window, candidate, window_name(candidate));
            }
        });
}

pub(in crate::file_manager) fn choose_region(ui: &mut Ui, region: &mut Region) {
    egui::ComboBox::from_id_salt("dsp-fft-region")
        .selected_text(region.label())
        .show_ui(ui, |ui| {
            for candidate in [Region::Current, Region::Visible, Region::Whole] {
                ui.selectable_value(region, candidate, candidate.label());
            }
        });
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

pub(in crate::file_manager) fn amplitude_ticks(
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

pub(in crate::file_manager) type Cursor<'a> = (&'a SpectralSurvey, &'a Channel, usize);

pub(in crate::file_manager) fn paint_measurement<'a>(
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

pub(in crate::file_manager) fn measurement_readout(
    ui: &mut Ui,
    ready: Option<ResultView<'_>>,
    cursor: Option<Cursor<'_>>,
    db: bool,
    clock: Clock,
    pending: &str,
) -> Option<Range<f64>> {
    match ready {
        Some((_, Ok(Some(_)))) => {
            let (survey, channel, bin) = cursor?;
            return spectrum_readout(ui, survey, channel, bin, db, clock);
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
