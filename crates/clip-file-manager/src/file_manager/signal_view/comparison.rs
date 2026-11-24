use super::{
    Capture, Link, LinkedTimeline, Source, SourceAction, Viewer, alignment, frequency_plot,
};
use crate::file_manager::{
    preview_copy::Copies,
    scope_view::{COLLAPSED_HEIGHT, move_trace, plot, scroll_trace_drag, trace_drop},
};
use egui::{Align, Button, Rect, Response, Sense, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Trace {
    source: u64,
    channel: usize,
}

struct TraceRow {
    source_index: usize,
    trace: Trace,
    visible: bool,
    digital: bool,
    filtered: Option<bool>,
}

impl TraceRow {
    fn lane_visibility(&self) -> impl Iterator<Item = bool> {
        std::iter::once(self.visible).chain(self.filtered)
    }

    fn has_visible_lane(&self) -> bool {
        self.visible || self.filtered == Some(true)
    }
}

#[derive(Default)]
pub(super) struct Comparison {
    pub order: Vec<Trace>,
    pub active: Option<Trace>,
    pub spectrum: bool,
    pub initialized: bool,
}

impl Comparison {
    pub(super) fn reconcile(&mut self, sources: &[Source]) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let defaults = ordered_traces(sources);
        let valid: HashSet<_> = defaults.iter().copied().collect();
        let mut kept = HashSet::new();
        self.order
            .retain(|key| valid.contains(key) && kept.insert(*key));
        self.order
            .extend(defaults.into_iter().filter(|key| !kept.contains(key)));
        if self.active.is_none_or(|key| !self.order.contains(&key)) {
            self.active = self.order.first().copied();
        }
    }

    fn focus_visible(&mut self, rows: &[TraceRow]) {
        let start = self
            .active
            .and_then(|active| rows.iter().position(|row| row.trace == active))
            .unwrap_or(0);
        self.active = rows[start..]
            .iter()
            .chain(&rows[..start])
            .find(|row| row.has_visible_lane())
            .map(|row| row.trace);
    }
}

impl Viewer {
    pub(super) fn recording_comparison(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        timeline: LinkedTimeline<'_>,
        links: &HashMap<u64, Link>,
    ) -> (Option<f64>, Option<(usize, SourceAction)>) {
        let mut action = self.comparison_source_controls(ui, timeline.preview, timeline.reference);
        let chart_height = (ui.available_height() - 85.0).max(0.0);
        let probe = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                self.comparison_lanes(ui, chart_height, links)
            })
            .inner;
        if let Some(source_action) = self.selected_trace_tools(ui, copies, links) {
            action = Some(source_action);
        }
        for source in &mut self.sources {
            if let Capture::Analog(view) = &mut source.capture {
                if self
                    .comparison
                    .active
                    .is_none_or(|trace| trace.source != source.id)
                {
                    view.suspend_comparison_spectrum();
                }
                view.finish_comparison(ui);
            } else if let Capture::Digital(view) = &mut source.capture {
                view.finish_comparison(ui);
            }
        }
        (probe, action)
    }

    fn selected_trace_tools(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        links: &HashMap<u64, Link>,
    ) -> Option<(usize, SourceAction)> {
        let trace = self.comparison.active?;
        let index = self
            .sources
            .iter()
            .position(|source| source.id == trace.source)?;
        let link = links.get(&trace.source)?;
        let source = &mut self.sources[index];
        let previous = source.capture.span().clone();
        let selection = ui
            .push_id(("selected-trace", trace.source), |ui| {
                match &mut source.capture {
                    Capture::Analog(view) => {
                        view.comparison_select(trace.channel);
                        view.comparison_overview(ui);
                        view.comparison_tools(ui, copies, link)
                    }
                    Capture::Digital(view) => {
                        view.comparison_tools(ui, copies, trace.channel, link)
                    }
                    Capture::Unavailable(_) => None,
                }
            })
            .inner;
        if *source.capture.span() != previous {
            self.span = link.clock.workspace_range(source.capture.span());
            ui.ctx().request_repaint();
        }
        selection.map(|range| (index, SourceAction::Share(range)))
    }

    pub(super) fn comparison_spectrum_dock(&mut self, ui: &mut Ui, links: &HashMap<u64, Link>) {
        let Some(trace) = self.comparison.active else {
            return;
        };
        let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.id == trace.source)
        else {
            return;
        };
        let Some(link) = links.get(&trace.source) else {
            return;
        };
        if !self.comparison.spectrum {
            if let Capture::Analog(view) = &mut source.capture {
                view.suspend_comparison_spectrum();
            }
            return;
        }
        let filename = source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let previous = link.clock.native_range(&self.span);
        *source.capture.span() = previous.clone();
        let inspecting_time = match &source.capture {
            Capture::Analog(view) => view.spectrum_time_evidence(),
            Capture::Digital(view) => view.spectrum_time_evidence(),
            Capture::Unavailable(_) => false,
        };
        let mut render = |ui: &mut Ui| {
            ui.push_id(trace.source, |ui| {
                self.comparison.spectrum = match &mut source.capture {
                    Capture::Analog(view) => {
                        view.comparison_select(trace.channel);
                        view.comparison_spectrum(ui, link, true, &filename)
                    }
                    Capture::Digital(view) => view.comparison_spectrum(ui, trace.channel, link),
                    Capture::Unavailable(_) => false,
                };
            });
        };
        if inspecting_time {
            render(ui);
        } else {
            frequency_plot::spectrum_dock(ui, "comparison-spectrum", render);
        }
        if *source.capture.span() != previous {
            self.span = link.clock.workspace_range(source.capture.span());
            ui.ctx().request_repaint();
        }
    }

    fn comparison_source_controls(
        &mut self,
        ui: &mut Ui,
        preview: Option<alignment::Suggestion>,
        reference: Option<f64>,
    ) -> Option<(usize, SourceAction)> {
        if !self.alignment.open {
            return None;
        }
        let mut action = None;
        let may_remove = self.sources.len() > 1;
        for (index, source) in self.sources.iter_mut().enumerate() {
            if matches!(source.capture, Capture::Unavailable(_)) {
                continue;
            }
            let is_reference = self
                .reference
                .as_ref()
                .is_some_and(|pin| pin.source == source.id);
            ui.push_id(("clock", source.id), |ui| {
                if let Some(request) = source.controls(
                    ui,
                    may_remove,
                    reference,
                    is_reference,
                    preview,
                    &mut self.alignment,
                ) {
                    action = Some((index, request));
                }
            });
        }
        action
    }

    pub(super) fn comparison_links(
        &mut self,
        ui: &mut Ui,
        timeline: LinkedTimeline<'_>,
    ) -> HashMap<u64, Link> {
        self.sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                (!matches!(source.capture, Capture::Unavailable(_))).then_some((index, source.id))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(index, id)| {
                let link = ui
                    .push_id(("comparison-measure", id), |ui| {
                        self.link_source(ui, index, timeline)
                    })
                    .inner;
                (id, link)
            })
            .collect()
    }

    pub(super) fn comparison_toolbar(&mut self, ui: &mut Ui) {
        if ui
            .add_enabled(
                self.comparison.spectrum || self.comparison.active.is_some(),
                Button::new("Spectrum").selected(self.comparison.spectrum),
            )
            .on_disabled_hover_text("Show a trace to inspect its spectrum")
            .clicked()
        {
            self.comparison.spectrum = !self.comparison.spectrum;
        }
    }

    fn comparison_rows(&self) -> Vec<TraceRow> {
        self.comparison
            .order
            .iter()
            .copied()
            .filter_map(|trace| {
                let source_index = self
                    .sources
                    .iter()
                    .position(|source| source.id == trace.source)?;
                let (visible, filtered, digital) = match &self.sources[source_index].capture {
                    Capture::Analog(view) => (
                        view.comparison_visible(trace.channel),
                        view.comparison_filter_visible(trace.channel),
                        false,
                    ),
                    Capture::Digital(view) => (view.comparison_visible(trace.channel), None, true),
                    Capture::Unavailable(_) => return None,
                };
                Some(TraceRow {
                    source_index,
                    trace,
                    visible,
                    filtered,
                    digital,
                })
            })
            .collect()
    }

    fn comparison_lanes(
        &mut self,
        ui: &mut Ui,
        chart_height: f32,
        links: &HashMap<u64, Link>,
    ) -> Option<f64> {
        scroll_trace_drag::<Trace>(ui);
        for source in &mut self.sources {
            let Some(link) = links.get(&source.id) else {
                continue;
            };
            let span = link.clock.native_range(&self.span);
            match &mut source.capture {
                Capture::Analog(view) => view.prepare_comparison(ui, span, link),
                Capture::Digital(view) => view.prepare_comparison(ui, link, span),
                Capture::Unavailable(_) => {}
            }
        }
        let mut rows = self.comparison_rows();
        self.comparison.focus_visible(&rows);
        const DIGITAL_HEIGHT: f32 = 86.0;
        let visible_digital = rows.iter().filter(|row| row.digital && row.visible).count();
        let visibility = rows.iter().flat_map(TraceRow::lane_visibility);
        let (total, visible) = visibility.fold((0, 0), |(total, visible), shown| {
            (total + 1, visible + usize::from(shown))
        });
        let desired = chart_height
            - (total - visible) as f32 * COLLAPSED_HEIGHT
            - visible_digital as f32 * DIGITAL_HEIGHT
            - total.saturating_sub(1) as f32 * ui.spacing().item_spacing.y;
        let height = (desired / (visible - visible_digital).max(1) as f32).max(116.0);
        let axis = (visible > 0).then(|| comparison_time_axis(ui, &self.span));
        let mut probe = None;
        let mut reorder = None;
        for row in &mut rows {
            let lane_height = if row.digital { DIGITAL_HEIGHT } else { height };
            let (_, target) = reserve_trace_row(ui, lane_height, row.visible);
            let filter = row
                .filtered
                .map(|shown| reserve_trace_row(ui, height, shown).0);
            let Some(link) = links.get(&row.trace.source) else {
                continue;
            };
            probe = self
                .comparison_row(ui, target.rect, filter, row, link)
                .or(probe);
            let row_bounds = filter.map_or(target.rect, |filtered| target.rect.union(filtered));
            reorder = trace_drop(ui, row_bounds, row.trace).or(reorder);
        }
        if let Some(axis) = axis {
            self.paint_comparison_fft_window(ui, axis, links);
        }
        self.comparison.focus_visible(&rows);
        if let Some((from, to, after)) = reorder {
            move_trace(&mut self.comparison.order, (from, to, after));
            ui.ctx().request_repaint();
        }
        probe
    }

    fn paint_comparison_fft_window(&self, ui: &Ui, axis: Rect, links: &HashMap<u64, Link>) {
        let interval = self.sources.iter().find_map(|source| {
            let Capture::Analog(view) = &source.capture else {
                return None;
            };
            let link = links.get(&source.id)?;
            Some(link.clock.workspace_range(view.inline_fft_evidence()?))
        });
        if let Some(interval) = interval {
            frequency_plot::paint_window(ui, ui.painter(), axis, &self.span, &interval);
        }
    }

    fn comparison_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        filter: Option<Rect>,
        row: &mut TraceRow,
        link: &Link,
    ) -> Option<f64> {
        let key = row.trace;
        let source = &mut self.sources[row.source_index];
        let name = source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        *source.capture.span() = link.clock.native_range(&self.span);
        let mut trace_ui = ui.new_child(egui::UiBuilder::new().id_salt(key).max_rect(rect));
        let active = self.comparison.active == Some(key);
        let (interaction, filtered) = match &mut source.capture {
            Capture::Analog(view) => {
                let interaction = view.comparison_trace(
                    &mut trace_ui,
                    rect,
                    key.channel,
                    &name,
                    active,
                    link,
                    key,
                );
                let filtered = filter
                    .map(|area| {
                        view.comparison_filtered_trace(
                            &mut trace_ui,
                            area,
                            key.channel,
                            &name,
                            link,
                        )
                    })
                    .unwrap_or_default();
                row.visible = view.comparison_visible(key.channel);
                row.filtered = view.comparison_filter_visible(key.channel);
                (interaction, filtered)
            }
            Capture::Digital(view) => {
                let interaction = view.comparison_trace(
                    &mut trace_ui,
                    rect,
                    key.channel,
                    &name,
                    active,
                    link,
                    key,
                );
                row.visible = view.comparison_visible(key.channel);
                (interaction, Default::default())
            }
            Capture::Unavailable(_) => return None,
        };
        if (interaction.selected || filtered.selected) && row.has_visible_lane() {
            self.comparison.active = Some(key);
        }
        self.comparison.spectrum |= filtered.edit_filters;
        if interaction.span_changed || filtered.span_changed {
            self.span = link.clock.workspace_range(source.capture.span());
            ui.ctx().request_repaint();
        }
        Link::reveal_trace(Some(link), ui, rect, key.channel);
        if self.reveal_source.take_if(|id| *id == source.id).is_some() {
            ui.scroll_to_rect(rect, Some(Align::TOP));
        }
        filtered
            .probe
            .or(interaction.probe)
            .map(|time| link.clock.to_workspace(time))
    }
}

fn reserve_trace_row(ui: &mut Ui, height: f32, visible: bool) -> (Rect, Response) {
    let height = if visible { height } else { COLLAPSED_HEIGHT };
    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover())
}

fn comparison_time_axis(ui: &mut Ui, span: &std::ops::Range<f64>) -> Rect {
    let (axis, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let ruler = Rect::from_min_max(axis.min + vec2(78.0, 0.0), pos2(axis.right(), axis.top()));
    plot::x_axis_with_grid(
        ui.painter(),
        ruler,
        span,
        "s",
        ui.visuals().weak_text_color(),
        std::iter::empty(),
    );
    Rect::from_min_max(ruler.min, axis.max)
}

fn ordered_traces(sources: &[Source]) -> Vec<Trace> {
    let mut groups = HashMap::new();
    let mut traces = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        let names: Vec<&str> = match &source.capture {
            Capture::Analog(view) => view
                .recording()
                .channels()
                .iter()
                .map(|lane| lane.name.as_str())
                .collect(),
            Capture::Digital(view) => view
                .recording()
                .1
                .iter()
                .map(|lane| lane.name.as_str())
                .collect(),
            Capture::Unavailable(_) => continue,
        };
        for (channel, name) in names.into_iter().enumerate() {
            let group = name.to_lowercase();
            let key = (channel, source_index);
            groups
                .entry(group.clone())
                .and_modify(|first| *first = key.min(*first))
                .or_insert(key);
            traces.push((
                group,
                Trace {
                    source: source.id,
                    channel,
                },
            ));
        }
    }
    traces.sort_by_key(|(group, _)| groups[group]);
    traces.into_iter().map(|(_, key)| key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::document_format::Format;
    use std::{fs, sync::atomic::AtomicBool};

    #[test]
    fn hiding_an_active_trace_advances_to_the_next_visible_row() {
        let traces = [1, 2, 3].map(|source| Trace { source, channel: 0 });
        let mut rows = traces
            .into_iter()
            .enumerate()
            .map(|(source_index, trace)| TraceRow {
                source_index,
                trace,
                visible: true,
                digital: false,
                filtered: None,
            })
            .collect::<Vec<_>>();
        let mut comparison = Comparison {
            active: Some(traces[1]),
            ..Default::default()
        };
        rows[1].visible = false;
        comparison.focus_visible(&rows);
        assert_eq!(comparison.active, Some(traces[2]));
        rows[2].visible = false;
        comparison.focus_visible(&rows);
        assert_eq!(comparison.active, Some(traces[0]));
        rows[0].visible = false;
        comparison.focus_visible(&rows);
        assert_eq!(comparison.active, None);
        rows[1].filtered = Some(true);
        comparison.focus_visible(&rows);
        assert_eq!(comparison.active, Some(traces[1]));

        rows[1].filtered = Some(false);
        comparison.focus_visible(&rows);
        assert_eq!(comparison.active, None);
    }

    #[test]
    fn groups_channels_across_files_and_retains_the_dragged_order() {
        let directory = tempfile::tempdir().unwrap();
        let mut sources = Vec::new();
        for id in [1, 2] {
            let path = directory.path().join(format!("scope-{id}.csv"));
            fs::write(&path, "X,CH2,CH4,Start,Increment,\nSequence,Volt,Volt,0,0.001,\n0,1,2,,,\n1,2,3,,,\n2,3,4,,,\n").unwrap();
            let mut source =
                Source::load(&path, Some(Format::Oscilloscope), &AtomicBool::new(false)).unwrap();
            source.id = id;
            sources.push(source);
        }
        let trace = |source, channel| Trace { source, channel };
        let mut comparison = Comparison::default();
        comparison.reconcile(&sources);
        assert_eq!(
            comparison.order,
            [trace(1, 0), trace(2, 0), trace(1, 1), trace(2, 1)]
        );
        move_trace(&mut comparison.order, (trace(1, 0), trace(1, 1), true));
        assert_eq!(
            comparison.order,
            [trace(2, 0), trace(1, 1), trace(1, 0), trace(2, 1)]
        );
        comparison.initialized = false;
        sources.remove(1);
        comparison.reconcile(&sources);
        assert_eq!(comparison.order, [trace(1, 1), trace(1, 0)]);
    }
}
