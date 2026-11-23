use super::{Viewer, plot};
use egui::{Align2, Color32, FontId, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    analysis::SpectrumWindow,
    waveforms::{Peak, display_value},
};

impl Viewer {
    pub(super) fn spectrum_ui(&mut self, ui: &mut Ui) {
        self.spectrum_controls(ui);
        let unit = self.overview.channels[self.active].unit.as_str();
        let spectrum_matches = self
            .measurement_key
            .as_ref()
            .is_some_and(|key| key.channel == self.active && key.fft == Some(self.spectrum_window));
        let Some(spectrum) = self
            .measurements
            .as_ref()
            .filter(|_| spectrum_matches)
            .and_then(|result| result.spectrum.as_ref())
        else {
            ui.weak("Computing spectrum…");
            return;
        };
        let length = spectrum.peak_amplitude.len();
        let last = length.saturating_sub(1);
        if last == 0 {
            return;
        }
        let hz = |bin: usize| bin as f64 * spectrum.bin_hz;
        let domain = 0.0..hz(last);
        let mut span = self.spectrum_span.clone().unwrap_or_else(|| domain.clone());
        let (area, response) = ui.allocate_exact_size(
            egui::vec2(
                ui.available_width(),
                (ui.available_height() - 24.0).max(66.0),
            ),
            Sense::click_and_drag(),
        );
        let rect = egui::Rect::from_min_max(area.min + vec2(58.0, 9.0), area.max - vec2(8.0, 18.0));
        if response.hovered() {
            let zoom = ui.input(|input| input.smooth_scroll_delta.y);
            if zoom != 0.0
                && let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
            {
                let center = plot::at(rect, &span, pointer.x);
                let factor = f64::from((-zoom * 0.012).exp()).clamp(0.1, 10.0);
                let width = ((span.end - span.start) * factor).clamp(spectrum.bin_hz, domain.end);
                let start = (center + (span.start - center) * factor)
                    .clamp(0.0, (domain.end - width).max(0.0));
                span = start..start + width;
                self.spectrum_span = Some(span.clone());
            }
        }
        let bin = |frequency: f64| {
            (frequency / spectrum.bin_hz)
                .round()
                .clamp(0.0, last as f64) as usize
        };
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.spectrum_marker = Some(bin(plot::at(rect, &span, pointer.x)));
        }
        let first = bin(span.start);
        let stop = bin(span.end).saturating_add(1);
        let values = &spectrum.peak_amplitude[first..stop];
        let stride = values.len().div_ceil((rect.width() as usize).max(8)).max(1);
        let amplitude = |value| amplitude_value(value, self.spectrum_db);
        let (peaks, bounds) = display_peaks(values, stride, self.spectrum_db);
        let painter = ui.painter_at(area);
        let ink = ui.visuals().weak_text_color();
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        plot::labelled_x_axis(&painter, rect, &span, "Hz", ink);
        amplitude_ticks(
            &painter,
            rect,
            bounds,
            if self.spectrum_db { "dB" } else { unit },
            ink,
        );
        plot::draw_raw(
            &painter.with_clip_rect(rect),
            rect,
            &span,
            hz(first),
            stride as f64 * spectrum.bin_hz,
            &peaks,
            bounds,
            plot::color(self.active),
        );
        if let Some(mark) = self.spectrum_marker.filter(|&mark| mark < length) {
            let marker = hz(mark);
            let x = plot::x_at(rect, &span, marker);
            if (rect.left()..=rect.right()).contains(&x) {
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.2, Color32::WHITE),
                );
            }
            ui.label(format!(
                "{} · {}",
                display_value(marker, "Hz"),
                if self.spectrum_db {
                    format!(
                        "{:.2} dB re 1 {unit} peak",
                        amplitude(spectrum.peak_amplitude[mark])
                    )
                } else {
                    display_value(f64::from(spectrum.peak_amplitude[mark]), unit)
                },
            ));
        } else {
            let peak = (first.max(1)..stop)
                .max_by(|&a, &b| spectrum.peak_amplitude[a].total_cmp(&spectrum.peak_amplitude[b]));
            ui.label(format!(
                "{} to {} · Peak {}",
                display_value(span.start, "Hz"),
                display_value(span.end, "Hz"),
                display_value(peak.map_or(f64::NAN, hz), "Hz")
            ));
        }
        self.draw_filter_response(ui, rect, &span);
        response.on_hover_text(
            "Scroll to zoom the frequency axis · Click a frequency to measure amplitude",
        );
    }
    fn spectrum_controls(&mut self, ui: &mut Ui) {
        let unit = self.overview.channels[self.active].unit.as_str();
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!(
                "{} spectrum",
                self.overview.channels[self.active].name
            ));
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
                .on_hover_text(format!("Level relative to 1 {unit} peak"));
            if ui
                .button("Fit")
                .on_hover_text("Fit the frequency range")
                .clicked()
            {
                self.spectrum_span = None;
            }
        });
        self.filter_controls(ui);
    }
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
