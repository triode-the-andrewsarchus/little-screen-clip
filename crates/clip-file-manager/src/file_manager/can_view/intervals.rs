//! Inter-frame intervals and the exact recorded frames at their endpoints.

mod plot;

use super::{Capture, JOBS, activity};
use crate::{
    file_manager::capture_browse::{
        Job, poll, start,
        table::{Columns, Page},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Context, Ui};
use little_clip_core::engineering::{
    can::{
        Frame, Index,
        intervals::{Endpoint, Gap, Report, Stream},
    },
    waveforms::display_value,
};
use std::{ops::Range, sync::Arc, time::Duration};

const COLUMNS: Columns<5> = Columns {
    id: "CAN intervals",
    headings: [
        "Duration",
        "Earlier frame",
        "Earlier time",
        "Later frame",
        "Later time",
    ],
    fractions: [0.19, 0.20, 0.20, 0.20, 0.21],
    min_height: 42.0,
    selected_fill: 0.52,
    hover_fill: 0.20,
};

#[derive(Default)]
pub(super) struct Panel {
    visible: bool,
    explorer: Option<Explorer>,
}

struct Explorer {
    stream: Stream,
    entry_scope: Range<i128>,
    scope: Range<i128>,
    report: Option<Result<Report, String>>,
    pending: Option<Job<Report>>,
    selected: Option<Gap>,
    scroll_to: Option<usize>,
    brush: Option<i128>,
    log: bool,
    endpoint: Option<Inspection>,
    generation: u64,
}

struct Inspection {
    endpoint: Endpoint,
    pending: Option<Job<Frame>>,
    result: Option<Result<Frame, String>>,
}

pub(super) struct Workspace<'a> {
    pub capture: &'a Arc<Capture>,
    pub index: &'a Arc<Index>,
    pub full: Range<i128>,
    pub colors: WorkbenchColors,
}

impl Panel {
    pub(super) fn visible(&self) -> bool {
        self.visible
    }

    pub(super) fn open(&mut self, stream: Stream, scope: Range<i128>) {
        if !self
            .explorer
            .as_ref()
            .is_some_and(|explorer| explorer.stream == stream && explorer.entry_scope == scope)
        {
            self.explorer = Some(Explorer {
                stream,
                entry_scope: scope.clone(),
                scope,
                report: None,
                pending: None,
                selected: None,
                scroll_to: None,
                brush: None,
                log: false,
                endpoint: None,
                generation: 0,
            });
        }
        self.visible = true;
    }

    pub(super) fn show<'a>(
        &'a mut self,
        ui: &mut Ui,
        workspace: Workspace<'_>,
    ) -> Option<&'a Frame> {
        let explorer = self.explorer.as_mut()?;
        let (close, back) = explorer.toolbar(ui, &workspace.full);
        if close {
            self.visible = false;
            explorer.pending = None;
            explorer.endpoint = None;
            ui.ctx().request_repaint();
            return None;
        }
        if back {
            explorer.endpoint = None;
        }
        if explorer.endpoint.is_some() {
            return explorer.show_endpoint(ui, &workspace);
        }
        explorer.update(ui.ctx(), &workspace);
        explorer.show_report(ui, &workspace);
        None
    }
}

impl Explorer {
    fn toolbar(&mut self, ui: &mut Ui, full: &Range<i128>) -> (bool, bool) {
        let is_frame = self.endpoint.is_some();
        ui.horizontal_wrapped(|ui| {
            let close = ui.button("Frame list").clicked();
            let back = is_frame && ui.button("Back to intervals").clicked();
            let width = if self.stream.id.extended {
                "29-bit ID"
            } else {
                "11-bit ID"
            };
            ui.strong(format!(
                "{} · {} · {width}",
                self.stream.channel, self.stream.id
            ));
            if !is_frame {
                ui.separator();
                ui.monospace(format!(
                    "{} to {}",
                    activity::time_label(self.scope.start - full.start),
                    activity::time_label(self.scope.end - 1 - full.start),
                ))
                .on_hover_text("Times relative to the beginning of the capture");
                if ui
                    .add_enabled(self.scope != *full, egui::Button::new("Whole capture"))
                    .clicked()
                {
                    self.set_scope(full.clone());
                }
                if ui
                    .add(egui::Button::new("Log duration").selected(self.log))
                    .clicked()
                {
                    self.log = !self.log;
                }
            }
            (close, back)
        })
        .inner
    }

    fn set_scope(&mut self, span: Range<i128>) {
        if self.scope != span {
            self.scope = span;
            self.pending = None;
            self.report = None;
            self.selected = None;
            self.scroll_to = None;
            self.brush = None;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    fn update(&mut self, ctx: &Context, workspace: &Workspace<'_>) {
        if let Some(result) = poll(&mut self.pending) {
            self.selected = result
                .as_ref()
                .ok()
                .and_then(|report| report.longest.first().copied());
            self.report = Some(result);
        }
        if self.report.is_none() && self.pending.is_none() {
            let capture = workspace.capture.clone();
            let index = workspace.index.clone();
            let stream = self.stream.clone();
            let scope = self.scope.clone();
            self.pending = Some(start(&JOBS, ctx, Duration::from_secs(30), move |cancel| {
                capture.intervals(&index, &stream, scope, cancel)
            }));
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(24));
        }
    }

    fn show_report(&mut self, ui: &mut Ui, workspace: &Workspace<'_>) {
        let report = match &self.report {
            Some(Ok(report)) => report,
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.button("Retry interval scan").clicked() {
                    self.report = None;
                    ui.ctx().request_repaint();
                }
                return;
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Reading frame timing…");
                });
                return;
            }
        };
        coverage(ui, report);
        let view = plot::View {
            scope: &self.scope,
            origin: workspace.full.start,
            log: self.log,
            selected: self.selected,
            colors: workspace.colors,
        };
        let hit = plot::show(ui, report, view, &mut self.brush);
        if let plot::Hit::Range(span) = hit {
            self.set_scope(span);
            ui.ctx().request_repaint();
            return;
        }
        if let plot::Hit::Gap(gap) = hit {
            self.selected = Some(gap);
            self.scroll_to = report.longest.iter().position(|longest| *longest == gap);
        }
        ui.weak("Click a point or row to select a gap · Drag to narrow the time range");
        if report.longest.is_empty() {
            ui.weak("No adjacent matching frames in this range");
            return;
        }
        let action = gap_controls(ui, self.selected, None, workspace.full.start);
        if let Some(endpoint) = action {
            self.endpoint = Some(Inspection {
                endpoint,
                pending: None,
                result: None,
            });
            ui.ctx().request_repaint();
            return;
        }
        let title = format!(
            "Longest gaps · showing {} of {}",
            report.longest.len(),
            report.intervals
        );
        ui.strong(title);
        if let Some(gap) = gap_table(
            ui,
            report,
            workspace,
            self.selected,
            self.scroll_to.take(),
            self.generation,
        ) {
            self.selected = Some(gap);
            ui.ctx().request_repaint();
        }
    }

    fn show_endpoint<'a>(
        &'a mut self,
        ui: &mut Ui,
        workspace: &Workspace<'_>,
    ) -> Option<&'a Frame> {
        let active = self.endpoint.as_ref().map(|view| view.endpoint);
        if let Some(endpoint) = gap_controls(ui, self.selected, active, workspace.full.start)
            && active != Some(endpoint)
        {
            self.endpoint = Some(Inspection {
                endpoint,
                pending: None,
                result: None,
            });
        }
        let view = self.endpoint.as_mut()?;
        view.inspect(ui, workspace)
    }
}

impl Inspection {
    fn inspect<'a>(&'a mut self, ui: &mut Ui, workspace: &Workspace<'_>) -> Option<&'a Frame> {
        if let Some(result) = poll(&mut self.pending) {
            self.result = Some(result);
        }
        if self.result.is_none() && self.pending.is_none() {
            let capture = workspace.capture.clone();
            let index = workspace.index.clone();
            let expected = self.endpoint;
            self.pending = Some(start(
                &JOBS,
                ui.ctx(),
                Duration::from_secs(15),
                move |cancel| {
                    let mut rows = capture.rows(&index, expected.row..expected.row + 1, cancel)?;
                    rows.pop()
                        .filter(|frame| {
                            frame.row == expected.row && frame.timestamp_ns == expected.time_ns
                        })
                        .ok_or_else(|| "The interval endpoint has changed.".into())
                },
            ));
        }
        match &self.result {
            Some(Ok(frame)) => Some(frame),
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                None
            }
            None => {
                ui.weak(format!("Reading frame {}…", self.endpoint.row + 1));
                ui.ctx().request_repaint_after(Duration::from_millis(24));
                None
            }
        }
    }
}

fn coverage(ui: &mut Ui, report: &Report) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} observed frames · {} intervals",
            report.observations, report.intervals
        ));
        ui.weak(format!(
            "Duplicate timestamps {} · Reversals {}",
            report.duplicates, report.reversals
        ));
        ui.weak(format!("{} source rows inspected", report.frames_read));
    });
    if report.boundaries > 0 {
        ui.weak("The time range crosses skipped index blocks. Pairs stay on the same side of each break.");
    }
    if report.truncated {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Scan limit reached. Narrow the time range to inspect fewer rows.",
        );
    }
}

fn gap_controls(
    ui: &mut Ui,
    gap: Option<Gap>,
    active: Option<Endpoint>,
    origin: i128,
) -> Option<Endpoint> {
    let gap = gap?;
    ui.horizontal_wrapped(|ui| {
        ui.strong(duration(gap.duration_ns()))
            .on_hover_text(format!("{} ns", gap.duration_ns()));
        let mut open = None;
        for (label, endpoint) in [("Earlier frame", gap.earlier), ("Later frame", gap.later)] {
            let button = ui.add(
                egui::Button::new(format!("{label} {}", endpoint.row + 1))
                    .selected(active == Some(endpoint)),
            );
            if button
                .on_hover_text(format!(
                    "{} · source timestamp {} ns",
                    activity::time_label(endpoint.time_ns - origin),
                    endpoint.time_ns
                ))
                .clicked()
            {
                open = Some(endpoint);
            }
        }
        open
    })
    .inner
}

fn gap_table(
    ui: &mut Ui,
    report: &Report,
    workspace: &Workspace<'_>,
    selected: Option<Gap>,
    scroll_to: Option<usize>,
    generation: u64,
) -> Option<Gap> {
    let page = Page {
        offset: 0,
        count: report.longest.len(),
        selected: selected.map(|gap| gap.later.row),
        selection: None,
        scroll_to,
        generation,
        height: ui.available_height(),
    };
    let origin = workspace.full.start;
    let result = COLUMNS.show(
        ui,
        page,
        workspace.colors,
        |position| report.longest.get(position as usize),
        |gap| gap.later.row,
        |ui, rect, gap| {
            let duration = duration(gap.duration_ns());
            let early_row = (gap.earlier.row + 1).to_string();
            let early_time = activity::time_label(gap.earlier.time_ns - origin);
            let late_row = (gap.later.row + 1).to_string();
            let late_time = activity::time_label(gap.later.time_ns - origin);
            COLUMNS.cells(
                ui,
                rect,
                [&duration, &early_row, &early_time, &late_row, &late_time],
                workspace.colors.text_primary,
            );
        },
    );
    let (row, _) = result.clicked?;
    report
        .longest
        .iter()
        .find(|gap| gap.later.row == row)
        .copied()
}

fn duration(ns: u128) -> String {
    display_value(ns as f64 * 1e-9, "s")
}
