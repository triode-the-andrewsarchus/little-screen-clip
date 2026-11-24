use super::Selection;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, Color32, FontId, Painter, Pos2, ScrollArea, Ui, vec2};
use little_clip_core::engineering::{
    flowgraph::{Direction, Graph, RateMeaning, RateSetting, Rates, StreamRate},
    waveforms::display_value,
};

impl super::Viewer {
    pub(super) fn rate_controls(&mut self, ui: &mut Ui) {
        ui.separator();
        clip_ui::buttons::toggle(ui, &mut self.show_rates, "Sample rates")
            .on_hover_text("Trace nominal rates and rate changes from the saved settings");
        if !self.rates.issues.is_empty()
            && ui
                .add(
                    egui::Button::new(format!("Rate checks ({})", self.rates.issues.len()))
                        .selected(self.show_rate_checks),
                )
                .on_hover_text("Show the checks in the inspector")
                .clicked()
        {
            self.show_rate_checks = !self.show_rate_checks;
            self.show_rates |= self.show_rate_checks;
            if !self.show_rate_checks {
                self.rate_check = None;
            }
        }
    }

    pub(super) fn rate_link_label(&self, name: String, edge: usize) -> String {
        if self.show_rates
            && let Some(rate) = self.rates.wire(edge)
        {
            format!("{name} · {}", rate.text)
        } else {
            name
        }
    }
}

pub(super) struct Overlay {
    rates: Rates,
    labels: Vec<Option<WireLabel>>,
    issues: Vec<(usize, &'static str)>,
}

pub(super) struct WireLabel {
    pub text: String,
    warning: Option<&'static str>,
    nominal: bool,
}

impl WireLabel {
    pub fn color(&self, colors: WorkbenchColors) -> Option<Color32> {
        if self.warning.is_some() {
            Some(colors.warning)
        } else {
            self.nominal.then_some(colors.success)
        }
    }

    pub fn paint(&self, painter: &Painter, source: Pos2, sink: Pos2, color: Color32) {
        if !self.nominal && self.warning.is_none()
            || (source.x - sink.x).abs() <= self.text.len() as f32 * 5.0
        {
            return;
        }
        painter.text(
            source.lerp(sink, 0.5) - vec2(0.0, 3.0),
            Align2::CENTER_BOTTOM,
            &self.text,
            FontId::monospace(9.5),
            color,
        );
    }
}

impl Overlay {
    pub fn new(graph: &Graph) -> Self {
        let rates = Rates::inspect(graph);
        let labels = graph
            .connections
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let rate = rates.wire(index)?;
                Some(WireLabel {
                    text: rate_text(rate),
                    warning: wire_issue(&rates, link.sink.block, rate),
                    nominal: matches!(rate, StreamRate::Nominal(_)),
                })
            })
            .collect();
        let issues = graph
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                issue(&rates, index, rates.block(index)?.input)
                    .map(|description| (index, description))
            })
            .collect();
        Self {
            rates,
            labels,
            issues,
        }
    }

    pub fn wire(&self, index: usize) -> Option<&WireLabel> {
        self.labels.get(index)?.as_ref()
    }

    pub fn issue_list(
        &self,
        ui: &mut Ui,
        graph: &Graph,
        current: Option<Selection>,
        height: f32,
    ) -> Option<Selection> {
        let mut selected = None;
        let row_height = ui.spacing().interact_size.y;
        ScrollArea::vertical()
            .id_salt("flowgraph-rate-checks")
            .max_height((height * 0.28).clamp(38.0, 136.0))
            .show_rows(ui, row_height, self.issues.len(), |ui, rows| {
                for row in rows {
                    let (block, description) = self.issues[row];
                    let suspect = graph
                        .adjacent(block, Direction::Upstream)
                        .iter()
                        .copied()
                        .filter(|&edge| self.wire(edge).is_some_and(|wire| wire.warning.is_some()));
                    let mut suspect = suspect.take(2);
                    let target = match (suspect.next(), suspect.next()) {
                        (Some(edge), None) => Selection::Wire(edge),
                        _ => Selection::Block(block),
                    };
                    let label = format!("{} · {description}", graph.blocks[block].name);
                    let button = egui::Button::new(&label)
                        .selected(current == Some(target))
                        .truncate();
                    if ui
                        .add_sized(vec2(ui.available_width(), row_height), button)
                        .on_hover_text(label)
                        .clicked()
                    {
                        selected = Some(target);
                    }
                }
            });
        selected
    }

    pub fn connection(
        &self,
        ui: &mut Ui,
        graph: &Graph,
        edge: usize,
        select: &mut Option<Selection>,
    ) {
        let Some(rate) = self.rates.wire(edge) else {
            ui.weak("Nominal rate unavailable");
            return;
        };
        let wire = &graph.connections[edge];
        let source = wire.source.block;
        let warning = self.wire(edge).and_then(|wire| wire.warning);
        if let Some(warning) = warning {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
        }
        clock_row(ui, graph, source, "Nominal rate", rate, select);
        if warning.is_some()
            && let Some(setting) = self
                .rates
                .block(wire.sink.block)
                .and_then(|block| block.setting)
        {
            rate_setting_row(ui, setting);
        }
        if let Some(ratio) = self.rates.block(source).and_then(|block| block.ratio)
            && ratio != 1.0
        {
            let mut streams = graph
                .adjacent(source, Direction::Upstream)
                .iter()
                .copied()
                .filter_map(|edge| Some((edge, self.rates.wire(edge)?)));
            if let (Some((input, rate)), None) = (streams.next(), streams.next()) {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button(format!("Input · {}", rate_text(rate)))
                        .clicked()
                    {
                        *select = Some(Selection::Wire(input));
                    }
                    ratio_label(ui, ratio);
                });
            }
        }
    }

    pub fn inspector(
        &self,
        ui: &mut Ui,
        graph: &Graph,
        index: usize,
        select: &mut Option<Selection>,
    ) {
        let Some(block) = self.rates.block(index) else {
            return;
        };
        let has_stream = |direction| {
            graph
                .adjacent(index, direction)
                .iter()
                .any(|&wire| self.wire(wire).is_some())
        };
        let has_input = has_stream(Direction::Upstream);
        let has_output = has_stream(Direction::Downstream);
        if !has_input && !has_output && block.setting.is_none() {
            return;
        }
        ui.separator();
        ui.strong("Sample rates");
        if has_input {
            clock_row(ui, graph, index, "Input", block.input, select);
        }
        if has_output {
            clock_row(ui, graph, index, "Output", block.output, select);
        }
        if let Some(ratio) = block.ratio {
            ratio_label(ui, ratio);
        }
        if let Some(setting) = block.setting {
            rate_setting_row(ui, setting);
        }
        if let Some(description) = issue(&self.rates, index, block.input) {
            ui.colored_label(ui.visuals().warn_fg_color, description);
        }
    }
}

fn clock_row(
    ui: &mut Ui,
    graph: &Graph,
    current: usize,
    label: &str,
    rate: StreamRate,
    select: &mut Option<Selection>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{label} · {}", rate_text(rate)));
        if let StreamRate::Nominal(clock) = rate
            && let Some(origin) = clock.origin.filter(|origin| origin.block != current)
        {
            let source = &graph.blocks[origin.block].name;
            let description = match origin.meaning {
                RateMeaning::Pacing => "Inferred from the configured throttle target",
                _ => "Trace to the configured source rate",
            };
            if ui
                .small_button(format!("From {source}"))
                .on_hover_text(description)
                .clicked()
            {
                *select = Some(Selection::Block(origin.block));
            }
        }
    });
}

fn ratio_label(ui: &mut Ui, ratio: f64) {
    let factor = format!("{ratio:.6}");
    ui.weak(format!(
        "Output / input · ×{}",
        factor.trim_end_matches('0').trim_end_matches('.'),
    ));
}

fn rate_setting_row(ui: &mut Ui, setting: RateSetting) {
    let (name, unit) = match setting.meaning {
        RateMeaning::Signal => ("Signal setting", "S/s"),
        RateMeaning::Hardware => ("Hardware setting", "S/s"),
        RateMeaning::Pacing => ("Throttle target", "items/s"),
        RateMeaning::Display => (
            "Display scale",
            if setting.key == "bw" { "Hz" } else { "S/s" },
        ),
    };
    ui.label(format!("{name} · {}", display_value(setting.rate, unit)))
        .on_hover_text(setting.key);
}

fn wire_issue(rates: &Rates, sink: usize, rate: StreamRate) -> Option<&'static str> {
    if rate == StreamRate::Unknown {
        return None;
    }
    let input = rates.block(sink)?.input;
    issue(
        rates,
        sink,
        if input == StreamRate::Conflicting {
            input
        } else {
            rate
        },
    )
}

fn rate_text(rate: StreamRate) -> String {
    match rate {
        StreamRate::Unknown => "Unknown".into(),
        StreamRate::Nominal(clock) => display_value(clock.rate, "S/s"),
        StreamRate::Conflicting => "Conflicting rates".into(),
    }
}

fn issue(rates: &Rates, block: usize, input: StreamRate) -> Option<&'static str> {
    match input {
        StreamRate::Unknown => None,
        StreamRate::Conflicting => Some("Upstream rates disagree"),
        StreamRate::Nominal(clock) => {
            let setting = rates.block(block)?.setting?;
            if !setting.differs(clock) {
                return None;
            }
            match setting.meaning {
                RateMeaning::Display => Some("Display scale differs from input"),
                RateMeaning::Hardware => Some("Hardware setting differs from input"),
                RateMeaning::Pacing => Some("Throttle target differs from input"),
                RateMeaning::Signal => None,
            }
        }
    }
}
