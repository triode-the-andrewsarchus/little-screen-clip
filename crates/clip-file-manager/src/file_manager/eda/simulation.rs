use super::{ViewerState, raster_task::REPAINT_INTERVAL, render_with_panels};
use crate::{file_manager::preview_job::Task, file_source::FileSource};
use egui::Ui;
use little_clip_core::{
    BackgroundJobs,
    eda::{
        Document,
        spice::{
            SpiceRawDocument,
            simulation::{check_support, simulate},
        },
    },
    palette::AppPalette,
};
use std::sync::{Arc, LazyLock, atomic::Ordering};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 1));

#[derive(Default)]
pub(super) struct View {
    pub enabled: bool,
    open: bool,
    support: Option<Result<(), String>>,
    probe: Option<Task<Result<(), String>>>,
    run: Option<Task<Result<SpiceRawDocument, String>>>,
    cancelled: bool,
    result: Option<Result<ResultView, String>>,
}

struct ResultView {
    doc: Arc<Document>,
    view: ViewerState,
}

impl View {
    pub(super) fn navigation(&mut self, ui: &mut Ui, palette: &AppPalette) {
        if !self.enabled {
            return;
        }
        let label = if self.open {
            "Back to netlist"
        } else {
            "Simulation…"
        };
        if ui
            .add(clip_ui::buttons::ButtonRole::Secondary.button(label, palette))
            .clicked()
        {
            if self.open {
                self.close();
            } else {
                self.open = true;
            }
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    fn close(&mut self) {
        self.open = false;
        if let Some(task) = &self.run {
            task.token.store(true, Ordering::Relaxed);
            self.cancelled = true;
        }
        self.probe = None;
    }

    pub(super) fn ui(&mut self, ui: &mut Ui, source: &FileSource, palette: &AppPalette) {
        self.poll();
        if self.support.is_none() && self.probe.is_none() {
            self.probe = Some(Task::start_unbounded(&JOBS, check_support));
        }
        ui.label("Use ngspice to run a local, self-contained deck you trust.");
        ui.horizontal(|ui| {
            let ready = matches!(self.support, Some(Ok(()))) && self.run.is_none();
            if ui
                .add_enabled(ready, egui::Button::new("Run Simulation"))
                .clicked()
            {
                let source = source.clone();
                self.cancelled = false;
                self.result = None;
                self.run = Some(Task::start_unbounded(&JOBS, move |cancel| {
                    simulate(&source, cancel)
                }));
            }
            if let Some(task) = &self.run {
                if ui
                    .add_enabled(!self.cancelled, egui::Button::new("Cancel"))
                    .clicked()
                {
                    self.cancelled = true;
                    task.token.store(true, Ordering::Relaxed);
                }
                ui.spinner();
                ui.label(if self.cancelled {
                    "Cancelling…"
                } else {
                    "Running ngspice…"
                });
            }
        });

        match &self.support {
            Some(Err(reason)) => {
                ui.label(reason.as_str());
            }
            None => {
                ui.label("Checking Linux sandbox support…");
            }
            _ => {}
        }
        match &mut self.result {
            Some(Ok(result)) => result.ui(ui, palette),
            Some(Err(reason)) => {
                ui.label(reason.as_str());
            }
            None => {}
        }
        if self.probe.is_some() || self.run.is_some() {
            ui.ctx().request_repaint_after(REPAINT_INTERVAL);
        }
    }

    fn poll(&mut self) {
        if let Some(result) = self.probe.as_ref().and_then(Task::poll) {
            self.probe = None;
            self.support = Some(result.map_err(|error| error.to_string()).and_then(|r| r));
        }
        if let Some(result) = self.run.as_ref().and_then(Task::poll) {
            self.run = None;
            self.result = Some(if self.cancelled {
                Err("Simulation cancelled".into())
            } else {
                result
                    .map_err(|error| error.to_string())
                    .and_then(|r| r)
                    .map(|raw| ResultView {
                        doc: Arc::new(Document::SpiceRaw(Box::new(raw))),
                        view: ViewerState::new(Vec::new()),
                    })
            });
        }
    }
}

impl ResultView {
    fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        ui.horizontal(|ui| {
            ui.strong("Simulation results");
            if ui.button("Fit view").clicked() {
                self.view.pan = egui::Vec2::ZERO;
                self.view.zoom = 1.0;
            }
        });
        render_with_panels(ui, &self.doc, &mut self.view, palette);
    }
}
