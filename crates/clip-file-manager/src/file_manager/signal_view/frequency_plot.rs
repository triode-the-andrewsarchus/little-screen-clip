use super::{
    spectra::{PeakWindow, Region, ResultView, Spectra},
    timeline,
};
use crate::file_manager::scope_view::plot;
use egui::{Align2, Button, Color32, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    analysis::{Clock, SpectralSurvey, SpectrumWindow},
    waveforms::{Peak, display_value},
};
use std::ops::Range;

pub(in crate::file_manager) fn spectrum_rect(
    ui: &mut Ui,
    min_height: f32,
) -> (Rect, egui::Response) {
    let height = (ui.available_height() - 48.0).max(min_height);
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

pub(in crate::file_manager) fn peak_reading(
    result: ResultView<'_>,
    survey: &SpectralSurvey,
    bin: usize,
    db: bool,
) -> Option<(PeakWindow, String)> {
    let channel = result.input.channel();
    let measured = survey.peak_interval(bin)?;
    let interval = channel.time_at(measured.start)..channel.time_at(measured.end);
    let hz = bin as f64 * survey.spectrum.bin_hz;
    let amplitude = survey.spectrum.peak_amplitude[bin];
    let gain = if db {
        format!("{:.2} dB", amplitude_value(amplitude, true))
    } else {
        display_value(f64::from(amplitude), &channel.unit)
    };
    Some((PeakWindow { interval, hz }, gain))
}

fn spectrum_readout(
    ui: &mut Ui,
    result: ResultView<'_>,
    survey: &SpectralSurvey,
    bin: usize,
    db: bool,
    clock: Clock,
) -> Option<PeakWindow> {
    let (window, gain) = peak_reading(result, survey, bin, db)?;
    let channel = result.input.channel();
    let requested = channel.time_at(result.rows.start)..channel.time_at(result.rows.end);
    let reveal = ui
        .horizontal_wrapped(|ui| {
            ui.monospace(format!("{} · {gain}", display_value(window.hz, "Hz")));
            ui.small_button(format!(
                "Show peak window  {}",
                time_range(&window.interval, clock)
            ))
            .on_hover_text("Inspect the source samples that produced this frequency bin")
            .clicked()
        })
        .inner;
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!("FFT input  {}", time_range(&requested, clock)));
        if survey.windows > 1 || survey.covered_samples < survey.requested_samples {
            ui.weak(format!(
                "{} windows · {:.1}% sample coverage",
                survey.windows,
                survey.covered_samples as f64 * 100.0 / survey.requested_samples as f64
            ));
        }
    });
    reveal.then_some(window)
}

pub(in crate::file_manager) fn time_range(interval: &Range<f64>, clock: Clock) -> String {
    format!(
        "{} to {}",
        display_value(clock.to_workspace(interval.start), "s"),
        display_value(clock.to_workspace(interval.end), "s"),
    )
}

pub(in crate::file_manager) fn origin_controls(
    ui: &mut Ui,
    spectra: &mut Spectra,
    view: &mut Range<f64>,
    clock: Clock,
) -> bool {
    let reveal = spectra.take_origin_reveal();
    let Some(window) = spectra.origin() else {
        return false;
    };
    let row = ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            ui.visuals().hyperlink_color,
            format!(
                "{} peak window  {}",
                display_value(window.hz, "Hz"),
                time_range(&window.interval, clock),
            ),
        );
        ui.button("Back to spectrum").clicked()
    });
    if row.inner {
        spectra.finish_inspection(view);
        ui.ctx().request_repaint();
    } else if reveal {
        row.response.scroll_to_me(Some(egui::Align::Min));
    }
    row.inner
}

pub(in crate::file_manager) fn paint_origin(
    ui: &Ui,
    painter: &egui::Painter,
    rect: Rect,
    visible: &Range<f64>,
    spectra: &Spectra,
) {
    if let Some(window) = spectra.origin() {
        paint_window(ui, painter, rect, visible, &window.interval);
    }
}

pub(in crate::file_manager) fn paint_window(
    ui: &Ui,
    painter: &egui::Painter,
    rect: Rect,
    visible: &Range<f64>,
    interval: &Range<f64>,
) {
    let color = ui.visuals().hyperlink_color;
    plot::highlight(
        painter,
        rect,
        visible,
        interval.clone(),
        color.gamma_multiply(0.18),
    );
    for edge in [interval.start, interval.end] {
        if visible.start <= edge && edge <= visible.end {
            painter.vline(
                plot::x_at(rect, visible, edge),
                rect.y_range(),
                Stroke::new(1.1, color),
            );
        }
    }
}

pub(in crate::file_manager) fn choose_window(ui: &mut Ui, window: &mut SpectrumWindow) {
    ui.weak("Window");
    for candidate in [
        SpectrumWindow::Hann,
        SpectrumWindow::Rectangular,
        SpectrumWindow::BlackmanHarris,
    ] {
        if ui
            .add(
                Button::new(window_name(candidate))
                    .small()
                    .selected(*window == candidate),
            )
            .clicked()
        {
            *window = candidate;
        }
    }
}

pub(in crate::file_manager) fn settings(
    ui: &mut Ui,
    region: &mut Region,
    window: &mut SpectrumWindow,
) {
    ui.horizontal_wrapped(|ui| {
        let spacing = ui.spacing_mut();
        spacing.interact_size.y = 24.0;
        spacing.item_spacing.x = 3.0;
        ui.weak("FFT range");
        for (candidate, label, help) in [
            (
                Region::Current,
                "Selection or view",
                "Use the selected time range, or the visible range when there is no selection",
            ),
            (Region::Visible, "Visible", "Use the visible time range"),
            (
                Region::Whole,
                "Whole lane",
                "Use the full available span of this lane",
            ),
        ] {
            if ui
                .add(Button::new(label).small().selected(*region == candidate))
                .on_hover_text(help)
                .clicked()
            {
                *region = candidate;
            }
        }
        ui.add_space(7.0);
        choose_window(ui, window);
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

pub(in crate::file_manager) fn paint_measurement(
    ui: &mut Ui,
    rect: Rect,
    response: &Response,
    span: &mut Range<f64>,
    display: &mut Display<'_>,
    shown: Option<ResultView<'_>>,
) -> Option<usize> {
    let Some(ResultView {
        input,
        output: Ok(Some(survey)),
        ..
    }) = shown
    else {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        return None;
    };
    let channel = input.channel();
    *span = display.navigate(ui, rect, response, survey, true);
    display.paint(ui, rect, survey, span, &channel.unit)
}

pub(in crate::file_manager) fn measurement_readout(
    ui: &mut Ui,
    ready: Option<ResultView<'_>>,
    cursor: Option<usize>,
    db: bool,
    clock: Clock,
    pending: &str,
) -> Option<PeakWindow> {
    ui.scope(|ui| {
        let spacing = ui.spacing_mut();
        spacing.interact_size.y = 20.0;
        spacing.item_spacing.y = 3.0;
        ui.set_min_height(48.0);
        match ready {
            Some(
                result @ ResultView {
                    output: Ok(Some(survey)),
                    ..
                },
            ) => {
                return spectrum_readout(ui, result, survey, cursor?, db, clock);
            }
            Some(ResultView {
                output: Ok(None), ..
            }) => {
                ui.weak("No contiguous finite samples in this interval");
            }
            Some(ResultView {
                output: Err(error), ..
            }) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            None => {
                ui.weak(pending);
            }
        }
        None
    })
    .inner
}
