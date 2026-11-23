use super::{Capture, Link, LinkedTimeline, Source, SourceAction, Viewer, alignment};
use crate::file_manager::{
    preview_copy::Copies,
    scope_view::{COLLAPSED_HEIGHT, plot},
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
            .find(|row| row.visible)
            .map(|row| row.trace);
    }

    fn place(&mut self, from: Trace, to: Trace, after: bool) {
        if from == to {
            return;
        }
        let Some(index) = self.order.iter().position(|&trace| trace == from) else {
            return;
        };
        self.order.remove(index);
        if let Some(target) = self.order.iter().position(|&trace| trace == to) {
            self.order.insert(target + usize::from(after), from);
        } else {
            self.order.insert(index, from);
        }
    }
}

impl Viewer {
    pub(super) fn analog_comparison(
        &mut self,
        ui: &mut Ui,
        copies: &mut Copies,
        timeline: LinkedTimeline<'_>,
    ) -> (Option<f64>, Option<(usize, SourceAction)>) {
        let mut action = self.comparison_source_controls(ui, timeline.preview, timeline.reference);
        let links = self.comparison_links(ui, timeline);
        let available = ui.available_height().max(330.0);
        let chart_height = if self.comparison.spectrum {
            (available - 290.0).max(0.0) * 0.76
        } else {
            available - 164.0
        };
        let probe = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                self.comparison_lanes(ui, chart_height, &links)
            })
            .inner;
        if let Some(trace) = self.comparison.active
            && let Some(index) = self
                .sources
                .iter()
                .position(|source| source.id == trace.source)
            && let Some(link) = links.get(&trace.source)
        {
            let Capture::Analog(view) = &mut self.sources[index].capture else {
                return (probe, action);
            };
            let previous = view.span().clone();
            view.comparison_select(trace.channel);
            view.comparison_overview(ui);
            if let Some(range) = view.comparison_tools(ui, copies, link) {
                action = Some((index, SourceAction::Share(range)));
            }
            ui.push_id(trace.source, |ui| {
                self.comparison.spectrum =
                    view.comparison_spectrum(ui, link, self.comparison.spectrum);
            });
            if *view.span() != previous {
                self.span = link.clock.workspace_range(view.span());
                ui.ctx().request_repaint();
            }
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
            }
        }
        (probe, action)
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
            if !matches!(source.capture, Capture::Analog(_)) {
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

    fn comparison_links(
        &mut self,
        ui: &mut Ui,
        timeline: LinkedTimeline<'_>,
    ) -> HashMap<u64, Link> {
        self.sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                matches!(source.capture, Capture::Analog(_)).then_some((index, source.id))
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
                self.comparison.active.is_some(),
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
                let Capture::Analog(view) = &self.sources[source_index].capture else {
                    return None;
                };
                Some(TraceRow {
                    source_index,
                    trace,
                    visible: view.comparison_visible(trace.channel),
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
        for source in &mut self.sources {
            if let Capture::Analog(view) = &mut source.capture
                && let Some(link) = links.get(&source.id)
            {
                view.prepare_comparison(ui, link.clock.native_range(&self.span), link);
            }
        }
        let mut rows = self.comparison_rows();
        self.comparison.focus_visible(&rows);
        let visible = rows.iter().filter(|row| row.visible).count();
        let desired = chart_height
            - (rows.len() - visible) as f32 * COLLAPSED_HEIGHT
            - rows.len().saturating_sub(1) as f32 * ui.spacing().item_spacing.y;
        let height = (desired / visible.max(1) as f32).max(88.0);
        if visible > 0 {
            comparison_time_axis(ui, &self.span);
        }
        let mut probe = None;
        let mut reorder = None;
        for row in &mut rows {
            let height = if row.visible {
                height
            } else {
                COLLAPSED_HEIGHT
            };
            let (rect, target) =
                ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
            let Some(link) = links.get(&row.trace.source) else {
                continue;
            };
            probe = self.comparison_row(ui, rect, row, link).or(probe);
            reorder = trace_drop(ui, &target, row.trace).or(reorder);
        }
        self.comparison.focus_visible(&rows);
        if let Some((from, to, after)) = reorder {
            self.comparison.place(from, to, after);
            ui.ctx().request_repaint();
        }
        probe
    }
    fn comparison_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
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
        let Capture::Analog(view) = &mut source.capture else {
            return None;
        };
        *view.span() = link.clock.native_range(&self.span);
        // The parent already reserved the row. Its child owns the widget IDs
        // without claiming a second vertical slot.
        let mut trace_ui = ui.new_child(egui::UiBuilder::new().id_salt(key).max_rect(rect));
        let interaction = view.comparison_trace(
            &mut trace_ui,
            rect,
            key.channel,
            &name,
            self.comparison.active == Some(key),
            link,
            key,
        );
        row.visible = view.comparison_visible(key.channel);
        if interaction.selected && row.visible {
            self.comparison.active = Some(key);
        }
        if interaction.span_changed {
            self.span = link.clock.workspace_range(view.span());
            ui.ctx().request_repaint();
        }
        Link::reveal_trace(Some(link), ui, rect, key.channel);
        if self.reveal_source.take_if(|id| *id == source.id).is_some() {
            ui.scroll_to_rect(rect, Some(Align::TOP));
        }
        interaction.probe.map(|time| link.clock.to_workspace(time))
    }
}

fn comparison_time_axis(ui: &mut Ui, span: &std::ops::Range<f64>) {
    let (axis, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let axis = Rect::from_min_max(axis.min + vec2(78.0, 0.0), pos2(axis.right(), axis.top()));
    plot::x_axis_with_grid(
        ui.painter(),
        axis,
        span,
        "s",
        ui.visuals().weak_text_color(),
        std::iter::empty(),
    );
}

fn trace_drop(ui: &Ui, target: &Response, key: Trace) -> Option<(Trace, Trace, bool)> {
    target
        .dnd_hover_payload::<Trace>()
        .filter(|dragged| **dragged != key)?;
    let pointer = ui.input(|input| input.pointer.latest_pos())?;
    let rect = target.rect;
    let after = pointer.y >= rect.center().y;
    let y = if after { rect.bottom() } else { rect.top() };
    ui.painter()
        .hline(rect.x_range(), y, ui.visuals().selection.stroke);
    let dragged = target.dnd_release_payload::<Trace>()?;
    Some((*dragged, key, after))
}

fn ordered_traces(sources: &[Source]) -> Vec<Trace> {
    let mut groups = HashMap::new();
    let mut traces = Vec::new();
    for source in sources {
        if let Capture::Analog(view) = &source.capture {
            for (channel, data) in view.recording().channels().iter().enumerate() {
                let count = groups.len();
                let group = *groups.entry(data.name.to_lowercase()).or_insert(count);
                traces.push((
                    group,
                    Trace {
                        source: source.id,
                        channel,
                    },
                ));
            }
        }
    }
    traces.sort_by_key(|(group, _)| *group);
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
        rows[1].visible = true;
        comparison.focus_visible(&rows);
        assert_eq!(comparison.active, Some(traces[1]));
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
        comparison.place(trace(1, 0), trace(1, 1), true);
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
