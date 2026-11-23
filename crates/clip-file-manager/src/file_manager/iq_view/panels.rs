use super::{
    Copies, MAX_EXPORT_SAMPLES, Viewer, WaterfallFrame, midpoint, plot,
    spectrum::{MAX_PEAK_WINDOWS, Measurement},
    waterfall,
};
use crate::file_manager::preview_copy;
use egui::{
    Align2, Color32, FontId, PointerButton, Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2,
};
use little_clip_core::engineering::waveforms::display_value;
use std::ops::Range;

const MEASURED: Color32 = Color32::from_rgb(247, 150, 203);

impl Viewer {
    pub(super) fn toolbar(&mut self, ui: &mut Ui, copies: &mut Copies) {
        let Some(loaded) = &self.loaded else { return };
        let metadata = loaded.recording.capture().metadata();
        let datatype = metadata.encoding.name();
        let rate = metadata.sample_rate_hz;
        let count = self.count();
        ui.horizontal_wrapped(|ui| {
            ui.strong(datatype);
            if let Some(rate) = rate {
                ui.weak(display_value(rate, "Sa/s"));
            }
            ui.weak(plot::sample_label(count, rate));
            if ui.button("Fit recording").clicked() {
                self.peak_view_before = None;
                self.view = 0..count;
                self.delay(ui);
            }
            if let Some(selection) = self.selection.as_ref() {
                let count = selection.end - selection.start;
                ui.label(format!("Selected {}", plot::sample_label(count, rate)));
                if ui
                    .add_enabled(
                        count <= MAX_EXPORT_SAMPLES && !copies.busy(),
                        egui::Button::new("Copy I/Q CSV"),
                    )
                    .on_hover_text("Copy up to 262,144 normalized I/Q sample pairs as a CSV file")
                    .clicked()
                {
                    self.copy(ui, copies);
                }
                if ui.small_button("Clear selection").clicked() {
                    self.restore_peak_view(ui);
                    self.selection = None;
                }
            }
        });
    }

    pub(super) fn activity(&mut self, ui: &mut Ui) {
        let Some(loaded) = &self.loaded else { return };
        let count = self.count();
        let span = 0..count;
        let rate = self.rate();
        let mut target = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Activity");
            if loaded.power.sampled() {
                ui.weak("Sampled");
            }
            ui.label("Burst threshold");
            ui.add(
                egui::DragValue::new(&mut self.threshold_dbfs)
                    .range(-180.0..=30.0)
                    .speed(0.35)
                    .suffix(" dBFS"),
            );
            let bursts = loaded.power.bursts_above(self.threshold_dbfs);
            ui.menu_button(format!("Bursts ({})", bursts.len()), |ui| {
                egui::ScrollArea::vertical().max_height(180.0).show_rows(
                    ui,
                    21.0,
                    bursts.len(),
                    |ui, rows| {
                        for range in &bursts[rows] {
                            let label = format!(
                                "{} · {}",
                                plot::sample_label(range.start, rate),
                                plot::sample_label(range.end - range.start, rate),
                            );
                            if ui.button(label).clicked() {
                                target = Some(range.clone());
                                ui.close();
                            }
                        }
                    },
                );
            });
        });
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 67.0), Sense::hover());
        let rect = Rect::from_min_max(outer.min, outer.max - vec2(0.0, 15.0));
        let id = ui.make_persistent_id("iq-activity");
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let paint = ui.painter_at(outer);
        paint.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        plot::draw_activity(
            &paint,
            rect,
            &span,
            &loaded.power,
            &loaded.scale,
            Some(self.threshold_dbfs),
        );
        plot::selection(
            &paint,
            rect,
            &span,
            &self.view,
            Color32::from_white_alpha(100),
        );
        paint_selection(
            &paint,
            rect,
            &span,
            self.selection.as_ref(),
            self.cursor,
            ui,
        );
        plot::time_ticks(&paint, rect, &span, rate, ui.visuals().weak_text_color());

        if let Some(range) = target {
            self.focus_samples(range.start, range.end);
        } else if let Some(range) = drag_range(ui, &response, &span) {
            self.peak_view_before = None;
            self.selection = Some(range.clone());
            if response.drag_stopped() {
                self.focus_samples(range.start, range.end);
            }
            self.delay(ui);
        } else if response.clicked()
            && let Some(point) = response.interact_pointer_pos()
        {
            let cursor = plot::sample_at(rect, &span, point.x).min(count - 1);
            let width = (self.view.end - self.view.start)
                .min((count / 4).max(2))
                .min(count);
            self.peak_view_before = None;
            self.view = window_around(count, cursor, width);
            self.selection = None;
            self.cursor = Some(cursor);
            self.delay(ui);
        }
    }

    pub(super) fn annotation_panel(&mut self, ui: &mut Ui) {
        let Some(loaded) = &self.loaded else { return };
        let capture = loaded.recording.capture();
        let metadata = capture.metadata();
        let rate = metadata.sample_rate_hz;
        let mut focus = None;
        ui.collapsing("Recording metadata", |ui| {
            if let Some(hardware) = &metadata.hardware {
                ui.label(hardware);
            }
            if let Some(description) = &metadata.description {
                ui.weak(description);
            }
            let count = metadata.tunings.len() + metadata.annotations.len();
            egui::ScrollArea::vertical().max_height(148.0).show_rows(
                ui,
                21.0,
                count,
                |ui, rows| {
                    for index in rows {
                        if let Some(tuning) = metadata.tunings.get(index) {
                            let center = tuning.center_hz.map_or_else(
                                || "Center unknown".into(),
                                |hz| display_value(hz, "Hz"),
                            );
                            let response = ui.button(format!(
                                "{} · {center}",
                                plot::sample_label(tuning.start_sample, rate)
                            ));
                            let response = if let Some(datetime) = &tuning.datetime {
                                response.on_hover_text(datetime)
                            } else {
                                response
                            };
                            if response.clicked() {
                                let end = metadata
                                    .tunings
                                    .get(index + 1)
                                    .map_or(capture.sample_count(), |next| next.start_sample);
                                focus = Some(tuning.start_sample..end);
                            }
                        } else {
                            let annotation = &metadata.annotations[index - metadata.tunings.len()];
                            let response = ui.button(format!(
                                "{} · {}",
                                plot::sample_label(annotation.samples.start, rate),
                                annotation.label.as_deref().unwrap_or("Annotation"),
                            ));
                            let details = annotation_details(annotation);
                            let response = if details.is_empty() {
                                response
                            } else {
                                response.on_hover_text(details)
                            };
                            if response.clicked() {
                                let first = annotation.samples.start;
                                focus = Some(first..annotation.samples.end.max(first + 1));
                            }
                        }
                    }
                },
            );
        });
        if let Some(range) = focus {
            self.focus_samples(range.start, range.end);
            self.delay(ui);
        }
    }

    pub(super) fn spectral_views(&mut self, ui: &mut Ui) {
        let height = (ui.available_height() - 10.0).max(120.0);
        let width = ui.available_width();
        let side_by_side = width >= 530.0;
        let waterfall_width = width * if width < 900.0 { 0.60 } else { 0.67 };
        let sizes = if side_by_side {
            [
                vec2(waterfall_width, height),
                vec2(
                    width - waterfall_width - ui.spacing().item_spacing.x,
                    height,
                ),
            ]
        } else {
            [
                vec2(width, (height * 0.55).max(95.0)),
                vec2(width, (height * 0.45).max(95.0)),
            ]
        };
        let mut render = |ui: &mut Ui| {
            let panels: [fn(&mut Viewer, &mut Ui, f32, f32); 2] =
                [Self::waterfall_panel, Self::spectrum_panel];
            for (size, show) in sizes.into_iter().zip(panels) {
                ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                    show(self, ui, size.x, size.y);
                });
            }
        };
        if side_by_side {
            ui.horizontal_top(render);
        } else {
            render(ui);
        }
    }

    fn waterfall_panel(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let heading = ui.horizontal_wrapped(|ui| {
            ui.strong("Waterfall");
            if let Some(Ok(measurement)) = self.current_spectrum()
                && let Some((_, origin)) = peak_source(measurement, self.frequency)
            {
                ui.weak(format!(
                    "Peak window · {}",
                    sample_span(origin, self.rate()),
                ));
            } else if let Some(cursor) = self.cursor {
                ui.weak(format!(
                    "Cursor {}",
                    plot::sample_label(cursor, self.rate())
                ));
            }
        });
        let caption = heading.response.rect.height();
        let (outer, _) = ui.allocate_exact_size(
            vec2(width, (height - caption - 14.0).max(65.0)),
            Sense::hover(),
        );
        let inset = if width >= 260.0 { 64.0 } else { 0.0 };
        let rect = Rect::from_min_max(outer.min + vec2(inset, 0.0), outer.max - vec2(0.0, 18.0));
        let response = ui.interact(
            rect,
            ui.make_persistent_id("iq-waterfall"),
            Sense::click_and_drag(),
        );
        let click = self.paint_waterfall(ui, outer, rect, &response);
        self.waterfall_input(ui, rect, &response, click);
    }

    fn paint_waterfall(
        &self,
        ui: &Ui,
        outer: Rect,
        rect: Rect,
        response: &Response,
    ) -> Option<(u64, f32)> {
        let paint = ui.painter_at(outer);
        let ink = ui.visuals().weak_text_color();
        paint.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let frame = self.displays.waterfall.as_ref().filter(|frame| {
            frame.data.samples.end > self.view.start && frame.data.samples.start < self.view.end
        });
        let mut pointed = None;
        if let Some(frame) = frame {
            let image = paint_image(&paint, rect, &self.view, frame);
            if let Some(position) = response
                .hover_pos()
                .filter(|position| image.contains(*position))
            {
                let point = waterfall::point(&frame.data, image, position);
                let cursor = midpoint(&point.spectrum.samples);
                pointed = Some((cursor, point.frequency_fraction));
                let clipped = paint.with_clip_rect(rect);
                let crosshair = Stroke::new(0.6, Color32::WHITE);
                clipped.hline(rect.x_range(), position.y, crosshair);
                clipped.vline(position.x, rect.y_range(), crosshair);
                let dbfs = point.spectrum.bins_dbfs[point.bin];
                response.clone().on_hover_text(format!(
                    "{} · {} · {dbfs:.1} dBFS",
                    plot::sample_label(cursor, self.rate()),
                    plot::frequency_label(point.spectrum, point.bin, true),
                ));
            }
            waterfall_labels(&paint, rect, frame, ink);
        }
        if frame.is_none_or(|frame| frame.key.span != self.view || frame.key.fft != self.fft)
            && self.count() >= 2
        {
            paint.text(
                rect.left_top() + vec2(6.0, 6.0),
                Align2::LEFT_TOP,
                "Updating waterfall…",
                FontId::proportional(11.0),
                ink,
            );
        }
        let clipped = paint.with_clip_rect(rect);
        let time_y = |sample| waterfall_y(rect, &self.view, sample);
        if let Some(selected) = &self.selection {
            let selected = Rect::from_min_max(
                pos2(rect.left(), time_y(selected.start)),
                pos2(rect.right(), time_y(selected.end)),
            );
            let accent = ui.visuals().selection.stroke.color;
            clipped.rect_filled(selected, 0.0, accent.gamma_multiply(0.24));
            for y in [selected.top(), selected.bottom()] {
                clipped.hline(rect.x_range(), y, Stroke::new(1.0, accent));
            }
        }
        self.paint_spectrum_evidence(&clipped, rect);
        if let Some(cursor) = self.cursor.filter(|cursor| self.view.contains(cursor)) {
            clipped.hline(
                rect.x_range(),
                time_y(cursor),
                Stroke::new(1.0, Color32::WHITE),
            );
        }
        if inset_has_labels(outer, rect) {
            waterfall_time_labels(&paint, rect, &self.view, self.rate(), ink);
        }
        pointed
    }

    fn paint_spectrum_evidence(&self, painter: &egui::Painter, rect: Rect) {
        if let Some(Ok(measurement)) = self.current_spectrum() {
            paint_measured_windows(painter, rect, &self.view, measurement);
            if let Some((bin, window)) = peak_source(measurement, self.frequency) {
                paint_peak_window(
                    painter,
                    rect,
                    &self.view,
                    window,
                    plot::bin_fraction(bin, measurement.frame.bins_dbfs.len()),
                );
            }
        }
        // A held peak's frequency marker belongs to its source window, including
        // when the visible waterfall spans other tuning centers.
        if !self.peak_hold
            && let Some(frequency) = self.frequency
        {
            painter.vline(
                egui::lerp(rect.x_range(), frequency),
                rect.y_range(),
                Stroke::new(1.0, Color32::GOLD),
            );
        }
    }

    fn waterfall_input(
        &mut self,
        ui: &Ui,
        rect: Rect,
        response: &Response,
        hover: Option<(u64, f32)>,
    ) {
        let span = self.view.clone();
        let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, range) =
            preview_copy::drag_range_by(ui, response, response.id, modified, |point| {
                waterfall::time_fraction(rect, point)
            });
        if let Some(range) = sample_range(&span, range) {
            self.peak_view_before = None;
            self.cursor = Some(midpoint(&range));
            self.selection = Some(range);
            self.delay(ui);
        } else if response.clicked()
            && let Some(point) = response.interact_pointer_pos()
        {
            let (cursor, fraction) = hover.unwrap_or_else(|| {
                let sample = plot::sample_fraction(&span, waterfall::time_fraction(rect, point));
                let frequency = x_fraction(rect, point);
                (sample, frequency)
            });
            self.cursor = Some(cursor.min(self.count() - 1));
            self.frequency = Some(fraction);
            self.peak_view_before = None;
            self.selection = None;
            self.delay(ui);
        }
        let count = self.count();
        if response.dragged_by(PointerButton::Middle) {
            let delta = ui.input(|input| input.pointer.delta().y);
            let width = span.end - span.start;
            let shift = (-f64::from(delta / rect.height()) * width as f64).round() as i64;
            let start = span.start.saturating_add_signed(shift).min(count - width);
            self.view = start..start + width;
            self.delay(ui);
        } else if let Some(point) = response.hover_pos() {
            let zoom = ui.input(|input| input.smooth_scroll_delta().y);
            if zoom.abs() > 0.01 {
                self.view = zoom_at(&span, waterfall::time_fraction(rect, point), zoom, count);
                self.delay(ui);
            }
        }
    }

    fn spectrum_panel(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let first_y = ui.next_widget_position().y;
        ui.scope(|ui| {
            let spacing = ui.spacing_mut();
            spacing.interact_size = vec2(0.0, 22.0);
            spacing.button_padding = vec2(4.0, 2.0);
            spacing.item_spacing = vec2(5.0, 4.0);
            self.spectrum_controls(ui);
            self.peak_controls(ui);
            match self.current_spectrum() {
                Some(Ok(measurement)) => show_measurement(ui, measurement),
                Some(Err(error)) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                None => {
                    ui.weak(if self.count() < 2 {
                        "Needs at least two samples"
                    } else {
                        "Reading spectrum…"
                    });
                }
            }
        });
        let measurement = self.current_spectrum();
        let controls_height = ui.next_widget_position().y - first_y;
        let (outer, _) = ui.allocate_exact_size(
            vec2(width, (height - controls_height).max(58.0)),
            Sense::hover(),
        );
        let rect = Rect::from_min_max(outer.min, outer.max - vec2(0.0, 18.0));
        let response = ui.interact(rect, ui.make_persistent_id("iq-spectrum"), Sense::click());
        let paint = ui.painter_at(outer);
        paint.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let Some(Ok(measurement)) = measurement else {
            return;
        };
        let frame = &measurement.frame;
        let density = self.show_density && frame.noise_bandwidth_hz.is_some();
        let bins = &frame.bins_dbfs;
        let marker = self.frequency.or_else(|| {
            let bin = measurement.coverage.as_ref()?.strongest()?;
            Some(plot::bin_fraction(bin, bins.len()))
        });
        let pointed = response
            .hover_pos()
            .and_then(|position| plot::displayed_bin(rect, bins, x_fraction(rect, position)));
        let hover = pointed.zip(response.hover_pos()).map(|(bin, mut point)| {
            point.x = egui::lerp(
                rect.x_range(),
                plot::plotted_bin_fraction(rect, bin, bins.len()),
            );
            point
        });
        plot::spectrum(
            &paint,
            rect,
            frame,
            marker,
            hover,
            ui.visuals().weak_text_color(),
            density,
        );
        if let Some(bin) = pointed {
            let (level, unit) = if density {
                (frame.density_dbfs_at(bin), "dBFS/Hz")
            } else {
                (Some(bins[bin]), "dBFS")
            };
            if let Some(level) = level {
                response.clone().on_hover_text(format!(
                    "{} · {level:.1} {unit}",
                    plot::frequency_label(frame, bin, true),
                ));
            }
        }
        let clicked = response
            .interact_pointer_pos()
            .filter(|_| response.clicked())
            .and_then(|point| plot::displayed_bin(rect, bins, x_fraction(rect, point)));
        if let Some(bin) = clicked {
            let fraction = plot::bin_fraction(bin, bins.len());
            // With a source already open, selecting another bin follows it
            // without replacing the viewport that Back will restore.
            let focus = (self.peak_view_before.is_some() || response.double_clicked())
                .then(|| measurement.coverage.as_ref()?.source(bin).cloned())
                .flatten();
            self.frequency = Some(fraction);
            if let Some(window) = focus {
                self.focus_peak(ui, &window);
            }
            ui.ctx().request_repaint();
        }
    }

    fn peak_controls(&mut self, ui: &mut Ui) {
        let inspecting = self.peak_view_before.is_some();
        let origin = self
            .current_spectrum()
            .and_then(Result::ok)
            .and_then(|measurement| {
                let (bin, window) = peak_source(measurement, self.frequency)?;
                Some((&measurement.frame, bin, window.clone()))
            });
        let Some((frame, bin, window)) = origin else {
            return;
        };
        let prefix = if self.frequency.is_some() {
            "Marked"
        } else {
            "Strongest"
        };
        ui.strong(format!(
            "{prefix} · {} · {:.1} dBFS",
            plot::frequency_label(frame, bin, true),
            frame.bins_dbfs[bin],
        ));
        let action = if inspecting {
            "Back to waterfall view"
        } else {
            "Show peak window"
        };
        if ui.small_button(action).clicked() {
            if inspecting {
                self.restore_peak_view(ui);
            } else {
                self.focus_peak(ui, &window);
            }
        }
    }

    fn spectrum_controls(&mut self, ui: &mut Ui) {
        let has_selection = self.spectrum_selection().is_some();
        self.peak_hold &= has_selection;
        ui.strong(if self.peak_hold {
            "Selection peak hold"
        } else {
            "Window spectrum"
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("FFT");
            for size in super::FFT_SIZES {
                if ui
                    .add(
                        egui::Button::new(size.to_string())
                            .selected(self.fft == size)
                            .small(),
                    )
                    .clicked()
                    && self.fft != size
                {
                    self.restore_peak_view(ui);
                    self.fft = size;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    has_selection,
                    egui::Button::new("Peak hold over selection").selected(self.peak_hold).small(),
                )
                .on_disabled_hover_text("Brush a range of at least two samples")
                .on_hover_text(format!(
                    "Keep the strongest level at each frequency across up to {MAX_PEAK_WINDOWS} FFT windows"
                ))
                .clicked()
            {
                self.peak_hold = !self.peak_hold;
                if !self.peak_hold {
                    self.restore_peak_view(ui);
                }
            }
            if self.rate().is_some() && !self.peak_hold {
                ui.toggle_value(&mut self.show_density, "dBFS/Hz")
                    .on_hover_text("Show power per Hertz instead of coherent peak amplitude");
            }
        });
    }
}

fn peak_source(measurement: &Measurement, marker: Option<f32>) -> Option<(usize, &Range<u64>)> {
    let coverage = measurement.coverage.as_ref()?;
    let bin = match marker {
        Some(fraction) => plot::bin_at_fraction(fraction, measurement.frame.bins_dbfs.len()),
        None => coverage.strongest()?,
    };
    Some((bin, coverage.source(bin)?))
}

fn paint_peak_window(
    painter: &egui::Painter,
    rect: Rect,
    view: &Range<u64>,
    window: &Range<u64>,
    frequency: f32,
) {
    let start = window.start.max(view.start);
    let end = window.end.min(view.end);
    if start >= end {
        return;
    }
    let top = waterfall_y(rect, view, start);
    let bottom = waterfall_y(rect, view, end);
    let band = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), bottom));
    painter.rect_filled(band, 0.0, Color32::GOLD.gamma_multiply(0.12));
    for y in [top, bottom] {
        painter.hline(rect.x_range(), y, Stroke::new(1.0, Color32::GOLD));
    }
    painter.vline(
        egui::lerp(rect.x_range(), frequency),
        top..=bottom,
        Stroke::new(1.3, Color32::GOLD),
    );
}

fn show_measurement(ui: &mut Ui, measurement: &Measurement) {
    let frame = &measurement.frame;
    let ink = if ui.visuals().dark_mode {
        MEASURED
    } else {
        Color32::from_rgb(152, 46, 105)
    };
    if let Some(coverage) = &measurement.coverage {
        let description = if coverage.sampled() {
            "Sampled"
        } else {
            "Complete"
        };
        ui.colored_label(
            ink,
            format!(
                "{description} · {} FFTs · {}/{} samples",
                coverage.windows.len(),
                coverage.observed_samples,
                coverage.selection.end - coverage.selection.start,
            ),
        )
        .on_hover_text(format!(
            "Selection · {}",
            sample_span(&coverage.selection, frame.sample_rate_hz)
        ));
    } else {
        ui.colored_label(
            ink,
            format!(
                "FFT window · {}",
                sample_span(&frame.samples, frame.sample_rate_hz),
            ),
        );
    }
    let tuning = if frame.crosses_tuning && frame.center_hz.is_none() {
        "Spans a retune · Relative frequencies"
    } else if frame.center_hz.zip(frame.sample_rate_hz).is_some() {
        "Absolute RF frequencies"
    } else {
        "Relative frequencies"
    };
    let has_invalid_samples = measurement.coverage.as_ref().map_or_else(
        || frame.finite_samples < frame.samples.end - frame.samples.start,
        |coverage| coverage.has_invalid_samples,
    );
    ui.weak(if has_invalid_samples {
        format!("{tuning} · Contains invalid samples")
    } else {
        tuning.to_owned()
    });
}

fn sample_span(samples: &Range<u64>, rate: Option<f64>) -> String {
    let start = samples.start;
    let last = samples.end - 1;
    match rate {
        Some(rate) => format!(
            "Samples {start}–{last} · {}–{}",
            plot::sample_label(start, Some(rate)),
            plot::sample_label(last, Some(rate)),
        ),
        None => format!("Samples {start}–{last}"),
    }
}

fn annotation_details(annotation: &little_clip_core::engineering::iq::Annotation) -> String {
    let mut detail = annotation.comment.clone().unwrap_or_default();
    if let Some(frequency) = &annotation.frequency_hz {
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&format!(
            "{} to {}",
            display_value(frequency.start, "Hz"),
            display_value(frequency.end, "Hz")
        ));
    }
    detail
}

fn drag_range(ui: &Ui, response: &Response, span: &Range<u64>) -> Option<Range<u64>> {
    let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
    let (_, range) = preview_copy::drag_range(ui, response, response.id, modified);
    sample_range(span, range)
}

fn sample_range(span: &Range<u64>, range: Option<(f32, f32)>) -> Option<Range<u64>> {
    let (first, end) = range?;
    let range = plot::sample_fraction(span, first)..plot::sample_fraction(span, end);
    (!range.is_empty()).then_some(range)
}

fn window_around(count: u64, sample: u64, width: u64) -> Range<u64> {
    let first = sample.saturating_sub(width / 2).min(count - width);
    first..first + width
}

fn zoom_at(span: &Range<u64>, fraction: f32, zoom: f32, count: u64) -> Range<u64> {
    let anchor = plot::sample_fraction(span, fraction);
    let factor = (-f64::from(zoom) * 0.0028).exp();
    let width = ((span.end - span.start) as f64 * factor).round() as u64;
    let width = width.clamp(2.min(count), count);
    let left = anchor
        .saturating_sub((width as f64 * f64::from(fraction)) as u64)
        .min(count - width);
    left..left + width
}

fn x_fraction(rect: Rect, position: Pos2) -> f32 {
    ((position.x - rect.left()) / rect.width()).clamp(0.0, 1.0)
}

fn paint_selection(
    paint: &egui::Painter,
    rect: Rect,
    span: &Range<u64>,
    selection: Option<&Range<u64>>,
    cursor: Option<u64>,
    ui: &Ui,
) {
    if let Some(selected) = selection {
        plot::selection(
            paint,
            rect,
            span,
            selected,
            ui.visuals().selection.stroke.color,
        );
    }
    if let Some(cursor) = cursor.filter(|cursor| span.contains(cursor)) {
        paint.vline(
            plot::x_at(rect, span, cursor),
            rect.y_range(),
            Stroke::new(1.0, Color32::WHITE),
        );
    }
}

fn paint_measured_windows(
    paint: &egui::Painter,
    rect: Rect,
    view: &Range<u64>,
    measurement: &Measurement,
) {
    for window in measurement.measured_windows() {
        let start = window.start.max(view.start);
        let end = window.end.min(view.end);
        if start >= end {
            continue;
        }
        let top = waterfall_y(rect, view, start);
        let bottom = waterfall_y(rect, view, end);
        let mark = Rect::from_min_max(
            pos2(rect.left(), top),
            pos2(
                rect.left() + 4.0,
                (top + 2.0).max(bottom).min(rect.bottom()),
            ),
        );
        paint.rect_filled(mark, 0.0, MEASURED);
        if measurement.coverage.is_none() {
            let band = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), bottom));
            paint.rect_filled(band, 0.0, MEASURED.gamma_multiply(0.14));
            for y in [top, bottom] {
                paint.hline(rect.x_range(), y, Stroke::new(1.0, MEASURED));
            }
        }
    }
}

fn paint_image(
    paint: &egui::Painter,
    rect: Rect,
    view: &Range<u64>,
    frame: &WaterfallFrame,
) -> Rect {
    let range = &frame.data.samples;
    let frame_length = (range.end - range.start).max(1) as f64;
    let image = Rect::from_min_max(
        pos2(rect.left(), waterfall_y(rect, view, range.start)),
        pos2(rect.right(), waterfall_y(rect, view, range.end)),
    );
    let visible = image.intersect(rect);
    let start = (view.start.max(range.start) - range.start) as f64 / frame_length;
    let end = (view.end.min(range.end) - range.start) as f64 / frame_length;
    let uv = Rect::from_min_max(pos2(0.0, start as f32), pos2(1.0, end as f32));
    paint.image(frame.texture.id(), visible, uv, Color32::WHITE);
    image
}

fn waterfall_y(rect: Rect, view: &Range<u64>, sample: u64) -> f32 {
    let offset = i128::from(sample) - i128::from(view.start);
    let length = (view.end - view.start).max(1) as f64;
    rect.top() + (offset as f64 / length) as f32 * rect.height()
}

fn waterfall_time_labels(
    paint: &egui::Painter,
    rect: Rect,
    span: &Range<u64>,
    rate: Option<f64>,
    ink: Color32,
) {
    for (fraction, align) in [
        (0.0, Align2::RIGHT_TOP),
        (0.5, Align2::RIGHT_CENTER),
        (1.0, Align2::RIGHT_BOTTOM),
    ] {
        paint.text(
            pos2(rect.left() - 4.0, egui::lerp(rect.y_range(), fraction)),
            align,
            plot::sample_label(plot::sample_fraction(span, fraction), rate),
            FontId::monospace(10.0),
            ink,
        );
    }
}

fn waterfall_labels(paint: &egui::Painter, rect: Rect, frame: &WaterfallFrame, ink: Color32) {
    plot::frequency_ticks(
        paint,
        rect,
        &frame.data.rows[0],
        frame.has_absolute_axis,
        ink,
    );
    let label = format!(
        "{:.0} to {:.0} dBFS",
        frame.color_span.0, frame.color_span.1
    );
    paint.with_clip_rect(rect).text(
        rect.right_top() + vec2(-5.0, 6.0),
        Align2::RIGHT_TOP,
        label,
        FontId::monospace(10.0),
        Color32::from_white_alpha(172),
    );
}

fn inset_has_labels(outer: Rect, rect: Rect) -> bool {
    rect.left() - outer.left() >= 60.0
}
