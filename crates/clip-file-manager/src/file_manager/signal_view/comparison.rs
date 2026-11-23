use super::{Capture, Link, LinkedTimeline, Source, SourceAction, Viewer, alignment};
use crate::file_manager::{preview_copy::Copies, scope_view::plot};
use egui::{Align, Button, Rect, Response, Sense, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Trace {
    source: u64,
    channel: usize,
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
            .add(Button::selectable(self.comparison.spectrum, "Spectrum"))
            .clicked()
        {
            self.comparison.spectrum = !self.comparison.spectrum;
        }
        ui.menu_button("Traces", |ui| {
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for &trace in &self.comparison.order {
                        let Some(source) = self
                            .sources
                            .iter_mut()
                            .find(|source| source.id == trace.source)
                        else {
                            continue;
                        };
                        let name = source
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy();
                        let Capture::Analog(view) = &mut source.capture else {
                            continue;
                        };
                        let channel = &view.recording().channels()[trace.channel].name;
                        let mut visible = view.comparison_visible(trace.channel);
                        if ui
                            .checkbox(&mut visible, format!("{channel} · {name}"))
                            .changed()
                        {
                            view.comparison_set_visible(trace.channel, visible);
                        }
                    }
                });
        });
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
        let rows = self
            .comparison
            .order
            .iter()
            .copied()
            .filter_map(|key| {
                let index = self
                    .sources
                    .iter()
                    .position(|source| source.id == key.source)?;
                match &self.sources[index].capture {
                    Capture::Analog(view) if view.comparison_visible(key.channel) => {
                        Some((index, key))
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        let height = (chart_height / rows.len().max(1) as f32).max(88.0);
        let (axis, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        let axis = Rect::from_min_max(axis.min + vec2(78.0, 0.0), pos2(axis.right(), axis.top()));
        plot::x_axis_with_grid(
            ui.painter(),
            axis,
            &self.span,
            "s",
            ui.visuals().weak_text_color(),
            std::iter::empty(),
        );
        let mut probe = None;
        let mut reorder = None;
        for (index, key) in rows {
            let (rect, target) =
                ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
            let Some(link) = links.get(&key.source) else {
                continue;
            };
            probe = self.comparison_row(ui, rect, index, key, link).or(probe);
            reorder = trace_drop(ui, &target, key).or(reorder);
        }
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
        index: usize,
        key: Trace,
        link: &Link,
    ) -> Option<f64> {
        let source = &mut self.sources[index];
        let name = source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let Capture::Analog(view) = &mut source.capture else {
            return None;
        };
        *view.span() = link.clock.native_range(&self.span);
        let interaction = ui
            .push_id(key, |ui| {
                view.comparison_trace(
                    ui,
                    rect,
                    key.channel,
                    &name,
                    self.comparison.active == Some(key),
                    link,
                    key,
                )
            })
            .inner;
        if interaction.selected {
            self.comparison.active = Some(key);
        }
        if interaction.span_changed {
            self.span = link.clock.workspace_range(view.span());
            ui.ctx().request_repaint();
        }
        if self.reveal_source.take_if(|id| *id == source.id).is_some() {
            ui.scroll_to_rect(rect, Some(Align::TOP));
        }
        interaction.probe.map(|time| link.clock.to_workspace(time))
    }
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
