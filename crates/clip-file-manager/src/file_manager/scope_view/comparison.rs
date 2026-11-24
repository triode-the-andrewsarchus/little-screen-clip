use super::{Copies, Link, PlottedChannel, ScopeTrace, Viewer, lanes::Lane, plot, timeline};
use egui::{Rect, Sense, Ui};
use little_clip_core::engineering::waveforms::Peak;

#[derive(Default)]
pub(in crate::file_manager) struct TraceInteraction {
    pub selected: bool,
    pub probe: Option<f64>,
    pub span_changed: bool,
    pub edit_filters: bool,
}

impl Viewer {
    pub(in crate::file_manager) fn comparison_visible(&self, index: usize) -> bool {
        self.visible[index]
    }

    fn comparison_set_visible(&mut self, index: usize, visible: bool) {
        self.visible[index] = visible;
        if !visible && let Some(spectrum) = self.trace_fft.get_mut(&ScopeTrace::Recorded(index)) {
            spectrum.suspend();
        }
    }

    pub(in crate::file_manager) fn comparison_select(&mut self, index: usize) {
        if index != self.active {
            self.activate_channel(index);
        }
    }

    pub(in crate::file_manager) fn prepare_comparison(
        &mut self,
        ui: &Ui,
        span: std::ops::Range<f64>,
        link: &Link,
    ) {
        self.poll(ui.ctx());
        if let Some(channel) = link.reveal_input {
            self.reveal_recorded(channel);
        }
        self.span = span;
        self.hover.time = link.probe;
        self.inline_fft_evidence = None;
    }

    pub(in crate::file_manager) fn finish_comparison(&mut self, ui: &Ui) {
        self.update_source(ui);
    }

    pub(in crate::file_manager) fn comparison_trace<T: Copy + Send + Sync + 'static>(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        index: usize,
        source: &str,
        active: bool,
        link: &Link,
        key: T,
    ) -> TraceInteraction {
        let visible = self.comparison_visible(index);
        let lane = Lane::comparison(area, visible);
        if !ui.is_rect_visible(lane.frame) {
            if let Some(spectrum) = self.trace_fft.get_mut(&ScopeTrace::Recorded(index)) {
                spectrum.suspend();
            }
            return TraceInteraction::default();
        }
        let painter = ui.painter_at(area);
        let color = plot::color(index);
        lane.background(ui, &painter);
        let mut heading = lane.heading(ui, "trace-heading");
        let handle = heading.add(egui::Label::new("↕").sense(Sense::drag()));
        handle.dnd_set_drag_payload(key);
        handle.on_hover_text("Drag to reorder traces");
        let label = egui::RichText::new(self.channel_label(index));
        let label = if active { label } else { label.color(color) };
        let selected = heading
            .add(egui::Button::selectable(active, label).small())
            .on_hover_text("Use this trace for analysis and export")
            .clicked();
        let source_width = (heading.available_width() - 128.0).clamp(48.0, 224.0);
        heading
            .add_sized(
                [source_width, heading.spacing().interact_size.y],
                egui::Label::new(egui::RichText::new(source).weak()).truncate(),
            )
            .on_hover_text(source);
        let action = if visible { "Hide" } else { "Show" };
        if heading
            .small_button(action)
            .on_hover_text(format!(
                "{action} {} · {source}",
                self.overview.channels[index].name,
            ))
            .clicked()
        {
            self.comparison_set_visible(index, !visible);
            ui.ctx().request_repaint();
        }
        let trace = ScopeTrace::Recorded(index);
        let toggle = visible && self.trace_fft_toggle(&mut heading, trace);
        if !visible {
            return TraceInteraction {
                selected,
                ..TraceInteraction::default()
            };
        }
        let bounds = (!self.trace_fft.contains_key(&trace)).then(|| self.channel_bounds(index));
        let mut interaction = if let Some(bounds) = bounds {
            let interaction = self.comparison_timeline(ui, lane.plot, index, link, trace);
            self.comparison_waveform(ui, lane, index, link, bounds);
            interaction
        } else {
            self.paint_trace_fft(ui, &mut heading, lane.plot, trace, link.clock);
            TraceInteraction::default()
        };
        interaction.selected |= selected || toggle;
        if toggle {
            self.toggle_trace_fft(trace);
            ui.ctx().request_repaint();
        }
        interaction
    }

    pub(super) fn comparison_timeline(
        &mut self,
        ui: &Ui,
        rect: Rect,
        index: usize,
        link: &Link,
        trace: ScopeTrace,
    ) -> TraceInteraction {
        let mut interaction = TraceInteraction::default();
        let response = ui.interact(
            rect,
            ui.make_persistent_id(("trace-time", trace)),
            Sense::click_and_drag(),
        );
        if let Some(span) = timeline::navigate(ui.ctx(), rect, &response, &self.span, true) {
            self.span = timeline::fit(
                &link.domain,
                span,
                self.overview.channels[index].interval_seconds,
            );
            interaction.span_changed = true;
        }
        if response.clicked()
            || response.drag_started_by(egui::PointerButton::Primary)
            || response.secondary_clicked()
        {
            self.comparison_select(index);
            interaction.selected = true;
        }
        if index == self.active {
            self.move_cursors(ui, rect, &response, trace);
        }
        interaction.probe = response
            .hover_pos()
            .filter(|point| rect.contains(*point))
            .map(|point| plot::at(rect, &self.span, point.x));
        if interaction.probe.is_some() {
            self.hover.time = interaction.probe;
        }
        interaction
    }

    fn comparison_waveform(&self, ui: &Ui, lane: Lane, index: usize, link: &Link, raw: Peak) {
        let rect = lane.plot;
        let painter = ui.painter_at(lane.frame);
        let color = plot::color(index);
        let bounds = raw;
        let channel = &self.overview.channels[index];
        lane.scale(&painter, bounds, &channel.unit, color);
        paint_time_grid(ui, &painter, rect);
        self.paint_channel(&painter, index, rect, bounds, color);
        Link::paint_trace_evidence(Some(link), &painter, rect, &self.span, index);
        if index == self.active {
            self.math_state.paint(
                &painter.with_clip_rect(rect.intersect(painter.clip_rect())),
                rect,
                &self.span,
                channel,
                self.math_envelope(),
            );
        }
        self.paint_time_markers(ui, &painter, rect, Some(link), index == self.active);
        self.paint_lane_probe(
            &painter,
            PlottedChannel {
                index,
                plot: rect,
                bounds,
                display_offset: 0.0,
            },
        );
    }

    pub(in crate::file_manager) fn comparison_tools(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        link: &Link,
    ) -> Option<std::ops::Range<f64>> {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 22.0;
            ui.spacing_mut().button_padding.y = 2.0;
            ui.horizontal_wrapped(|ui| self.copy_selection(ui, copies));
            self.cursor_readout(ui, Some(link))
        })
        .inner
    }

    pub(in crate::file_manager) fn comparison_spectrum(
        &mut self,
        ui: &mut Ui,
        link: &Link,
        show: bool,
        source: &str,
    ) -> bool {
        if !show {
            self.spectra.suspend();
            return false;
        }
        self.spectrum = true;
        self.spectra.only = false;
        if self.spectra.origin().is_some() {
            crate::file_manager::signal_view::frequency_plot::origin_controls(
                ui,
                &mut self.spectra,
                &mut self.span,
                link.clock,
            );
        } else {
            self.spectrum_ui(ui, Some(link), false, Some(source));
        }
        self.spectrum
    }

    pub(in crate::file_manager) fn suspend_comparison_spectrum(&mut self) {
        self.spectra.suspend();
    }

    pub(in crate::file_manager) fn comparison_overview(&mut self, ui: &mut Ui) {
        self.overview_ui(ui);
    }
}

pub(super) fn paint_time_grid(ui: &Ui, painter: &egui::Painter, rect: Rect) {
    let ink = ui.visuals().weak_text_color();
    let divisions = plot::x_divisions(rect);
    for tick in 0..=divisions {
        let x = egui::lerp(rect.x_range(), tick as f32 / divisions as f32);
        painter.vline(
            x,
            rect.y_range(),
            egui::Stroke::new(1.0, ink.gamma_multiply(0.21)),
        );
    }
}
