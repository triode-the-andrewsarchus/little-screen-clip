use super::{Copies, MAX_EXPORT_SAMPLES, Viewer, WaterfallFrame, midpoint, plot, waterfall};
use crate::file_manager::preview_copy;
use egui::{
    Align2, Color32, FontId, PointerButton, Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2,
};
use little_clip_core::engineering::{iq::Spectrum, waveforms::display_value};
use std::ops::Range;

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
                self.view = 0..count;
                self.delay(ui);
            }
            egui::ComboBox::from_id_salt("iq-window")
                .selected_text(format!("FFT {}", self.fft))
                .show_ui(ui, |ui| {
                    for size in super::FFT_SIZES {
                        ui.selectable_value(&mut self.fft, size, format!("FFT {size}"));
                    }
                });
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
                    self.selection = None;
                }
            }
            if let Some(cursor) = self.cursor {
                ui.weak(format!("Cursor {}", plot::sample_label(cursor, rate)));
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
        if width >= 530.0 {
            ui.horizontal_top(|ui| {
                let waterfall_width = width * 0.67;
                self.waterfall_panel(ui, waterfall_width, height);
                let spectrum_width =
                    (width - waterfall_width - ui.spacing().item_spacing.x).max(140.0);
                self.spectrum_panel(ui, spectrum_width, height);
            });
        } else {
            self.waterfall_panel(ui, width, (height * 0.55).max(95.0));
            self.spectrum_panel(ui, width, (height * 0.45).max(95.0));
        }
    }

    fn waterfall_panel(&mut self, ui: &mut Ui, width: f32, height: f32) {
        ui.allocate_ui(vec2(width, height), |ui| {
            ui.strong("Waterfall");
            let (outer, _) =
                ui.allocate_exact_size(vec2(width, (height - 34.0).max(65.0)), Sense::hover());
            let inset = if width >= 260.0 { 64.0 } else { 0.0 };
            let rect =
                Rect::from_min_max(outer.min + vec2(inset, 0.0), outer.max - vec2(0.0, 18.0));
            let response = ui.interact(
                rect,
                ui.make_persistent_id("iq-waterfall"),
                Sense::click_and_drag(),
            );
            let click = self.paint_waterfall(ui, outer, rect, &response);
            self.waterfall_input(ui, rect, &response, click);
        });
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
                paint.with_clip_rect(rect).hline(
                    rect.x_range(),
                    position.y,
                    Stroke::new(0.6, Color32::WHITE),
                );
                let dbfs = point.spectrum.bins_dbfs[point.bin];
                response.clone().on_hover_text(format!(
                    "{} · {} · {dbfs:.1} dBFS",
                    plot::sample_label(cursor, self.rate()),
                    plot::frequency_label(point.spectrum, point.bin),
                ));
            }
            if inset_has_labels(outer, rect) {
                waterfall_labels(&paint, rect, frame, ink);
            }
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
        paint_selection(
            &clipped,
            rect,
            &self.view,
            self.selection.as_ref(),
            self.cursor,
            ui,
        );
        if let Some(frequency) = self.frequency {
            clipped.hline(
                rect.x_range(),
                egui::lerp(rect.y_range(), 1.0 - frequency),
                Stroke::new(1.0, Color32::GOLD),
            );
        }
        plot::time_ticks(&paint, rect, &self.view, self.rate(), ink);
        pointed
    }

    fn waterfall_input(
        &mut self,
        ui: &Ui,
        rect: Rect,
        response: &Response,
        hover: Option<(u64, f32)>,
    ) {
        let span = self.view.clone();
        if let Some(range) = drag_range(ui, response, &span) {
            self.cursor = Some(midpoint(&range));
            self.selection = Some(range);
            self.delay(ui);
        } else if response.clicked()
            && let Some(point) = response.interact_pointer_pos()
        {
            let (cursor, fraction) = hover.unwrap_or_else(|| {
                let sample = plot::sample_at(rect, &span, point.x);
                let frequency = ((rect.bottom() - point.y) / rect.height()).clamp(0.0, 1.0);
                (sample, frequency)
            });
            self.cursor = Some(cursor.min(self.count() - 1));
            self.frequency = Some(fraction);
            self.selection = None;
            self.delay(ui);
        }
        let count = self.count();
        if response.dragged_by(PointerButton::Middle) {
            let delta = ui.input(|input| input.pointer.delta().x);
            let width = span.end - span.start;
            let shift = (-f64::from(delta / rect.width()) * width as f64).round() as i64;
            let start = span.start.saturating_add_signed(shift).min(count - width);
            self.view = start..start + width;
            self.delay(ui);
        } else if let Some(point) = response.hover_pos() {
            let zoom = ui.input(|input| input.smooth_scroll_delta().y);
            if zoom.abs() > 0.01 {
                self.view = zoom_at(rect, &span, point.x, zoom, count);
                self.delay(ui);
            }
        }
    }

    fn spectrum_panel(&mut self, ui: &mut Ui, width: f32, height: f32) {
        ui.allocate_ui(vec2(width, height), |ui| {
            let density_available = self.rate().is_some();
            ui.horizontal(|ui| {
                ui.strong("Selection spectrum");
                if density_available {
                    ui.toggle_value(&mut self.show_density, "dBFS/Hz")
                        .on_hover_text("Show power per Hertz instead of coherent peak amplitude");
                }
            });
            let (outer, _) =
                ui.allocate_exact_size(vec2(width, (height - 54.0).max(58.0)), Sense::hover());
            let rect = Rect::from_min_max(outer.min, outer.max - vec2(0.0, 18.0));
            let response = ui.interact(rect, ui.make_persistent_id("iq-spectrum"), Sense::click());
            let paint = ui.painter_at(outer);
            paint.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
            let Some(frame) = self.displays.spectrum.as_ref() else {
                ui.weak(if self.count() < 2 {
                    "Needs at least two samples"
                } else {
                    "Reading spectrum…"
                });
                return;
            };
            let density = self.show_density && frame.noise_bandwidth_hz.is_some();
            plot::spectrum(
                &paint,
                rect,
                frame,
                self.frequency,
                response.hover_pos(),
                ui.visuals().weak_text_color(),
                density,
            );
            let label = if frame.crosses_tuning {
                "Spans a retune"
            } else if frame.center_hz.is_none() {
                "Relative frequencies"
            } else {
                "Recorded frequencies"
            };
            ui.weak(format!(
                "{} · {}",
                plot::sample_label(frame.samples.start, self.rate()),
                label
            ));
            if let Some(position) = response.hover_pos() {
                let fraction = x_fraction(rect, position);
                let bin = ((fraction * frame.bins_dbfs.len() as f32) as usize)
                    .min(frame.bins_dbfs.len() - 1);
                let (level, unit) = if density {
                    (frame.density_dbfs_at(bin).unwrap_or(f32::NAN), "dBFS/Hz")
                } else {
                    (frame.bins_dbfs[bin], "dBFS")
                };
                if level.is_finite() {
                    response.clone().on_hover_text(format!(
                        "{} · {level:.1} {unit}",
                        plot::frequency_label(frame, bin),
                    ));
                }
            }
            if response.clicked()
                && let Some(point) = response.interact_pointer_pos()
            {
                self.frequency = Some(x_fraction(rect, point));
            }
        });
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
    let (first, end) = range?;
    let range = plot::sample_fraction(span, first)..plot::sample_fraction(span, end);
    (!range.is_empty()).then_some(range)
}

fn window_around(count: u64, sample: u64, width: u64) -> Range<u64> {
    let first = sample.saturating_sub(width / 2).min(count - width);
    first..first + width
}

fn zoom_at(rect: Rect, span: &Range<u64>, x: f32, zoom: f32, count: u64) -> Range<u64> {
    let fraction = x_fraction(rect, pos2(x, rect.top()));
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

fn paint_image(
    paint: &egui::Painter,
    rect: Rect,
    view: &Range<u64>,
    frame: &WaterfallFrame,
) -> Rect {
    let range = &frame.data.samples;
    let frame_width = (range.end - range.start).max(1) as f64;
    let view_width = (view.end - view.start).max(1) as f64;
    let to_x = |sample| {
        let offset = i128::from(sample) - i128::from(view.start);
        rect.left() + (offset as f64 / view_width) as f32 * rect.width()
    };
    let image = Rect::from_min_max(
        pos2(to_x(range.start), rect.top()),
        pos2(to_x(range.end), rect.bottom()),
    );
    let visible = image.intersect(rect);
    let start = (view.start.max(range.start) - range.start) as f64 / frame_width;
    let end = (view.end.min(range.end) - range.start) as f64 / frame_width;
    let uv = Rect::from_min_max(pos2(start as f32, 0.0), pos2(end as f32, 1.0));
    paint.image(frame.texture.id(), visible, uv, Color32::WHITE);
    image
}

fn waterfall_labels(paint: &egui::Painter, rect: Rect, frame: &WaterfallFrame, ink: Color32) {
    let row = &frame.data.rows[0];
    let same_center = frame
        .data
        .rows
        .iter()
        .all(|row2| row2.center_hz == row.center_hz);
    for fraction in [0.0, 0.5, 1.0] {
        let bin = ((row.bins_dbfs.len() - 1) as f32 * fraction).round() as usize;
        let label = if same_center {
            plot::frequency_label(row, bin)
        } else {
            relative_label(row, bin)
        };
        paint.text(
            pos2(
                rect.left() - 4.0,
                egui::lerp(rect.y_range(), 1.0 - fraction),
            ),
            Align2::RIGHT_CENTER,
            label,
            FontId::monospace(10.0),
            ink,
        );
    }
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

fn relative_label(row: &Spectrum, bin: usize) -> String {
    row.relative_hz_at(bin).map_or_else(
        || {
            let value = bin as f64 / row.bins_dbfs.len() as f64 - 0.5;
            format!("{value:+.3} cy/smp")
        },
        |frequency| display_value(frequency, "Hz"),
    )
}

fn inset_has_labels(outer: Rect, rect: Rect) -> bool {
    rect.left() - outer.left() >= 60.0
}
