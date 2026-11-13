//! Native harness selection and worker-owned source transactions.
use crate::diagram_preview::{Camera, GraphicalUpdate, Preview};
use clip_ui::buttons::ButtonRole;
use egui::{Color32, Rect, Sense, Stroke};
use little_clip_core::{
    BackgroundJobs, JobError,
    diagram::{
        Format,
        wireviz::{
            self, Conductor, Endpoint, Harness, LinkKind, PinId,
            edits::{Edit, Editable},
        },
    },
    jobs::task::Task,
    palette::AppPalette,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

mod canvas;

struct Model {
    editable: Editable,
    neighbors: HashMap<Endpoint, Vec<Endpoint>>,
}
impl Model {
    fn parse(source: &str) -> Result<Self, String> {
        let editable = Editable::parse(source)?;
        let mut neighbors: HashMap<_, Vec<_>> = HashMap::new();
        for link in &editable.harness().links {
            if matches!(link.kind, LinkKind::ConnectorMating(_)) {
                continue;
            }
            let [a, b] = link.endpoints;
            neighbors.entry(a).or_default().push(b);
            neighbors.entry(b).or_default().push(a);
        }
        Ok(Self {
            editable,
            neighbors,
        })
    }
}
struct Update {
    source: Arc<str>,
    model: Arc<Model>,
    publish: bool,
}
struct Request {
    edit: Edit,
    started: Instant,
}

#[derive(Default)]
pub(crate) struct WireEditor {
    source: Option<Arc<str>>,
    model: Option<Arc<Model>>,
    pending: Option<Task<Result<Update, String>>>,
    request: Option<Request>,
    retry: Option<Instant>,
    parse_error: Option<String>,
    edit_error: Option<String>,
    canvas_notice: Option<String>,
    camera: Camera,
    selected: Option<Endpoint>,
    connected: HashSet<Endpoint>,
    label: String,
    color: String,
    show_source: bool,
}
impl WireEditor {
    pub(crate) fn busy(&self) -> bool {
        self.request.is_some()
    }
    pub(crate) fn failed(&self) -> bool {
        self.edit_error.is_some()
    }
    pub(crate) fn suspend(&mut self) {
        if !self.busy() {
            self.pending = None;
        }
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context, source: &str) -> Option<GraphicalUpdate> {
        if self.source.as_deref().is_some_and(|old| old != source) {
            let accepted = self.busy();
            self.pending = None;
            self.request = None;
            self.model = None;
            self.source = None;
            self.selected = None;
            self.connected.clear();
            self.parse_error = None;
            self.edit_error = accepted
                .then(|| "Harness source changed; the graphical edit was not applied.".into());
            self.retry = Some(Instant::now() + Duration::from_millis(120));
            return accepted.then_some(GraphicalUpdate::Replaced);
        }
        if self
            .request
            .as_ref()
            .is_some_and(|r| r.started.elapsed() > Duration::from_secs(5))
        {
            self.pending = None;
            return self.fail("Harness edit timed out; the source was kept unchanged.".into());
        }
        if self.pending.is_none() && self.busy() {
            self.start_edit();
        }
        if let Some(result) = self.pending.as_ref().and_then(Task::poll) {
            self.pending = None;
            match result {
                Ok(Ok(update)) => {
                    self.request = None;
                    self.source = Some(update.source.clone());
                    self.model = Some(update.model);
                    self.parse_error = None;
                    if update.publish {
                        self.edit_error = None;
                    }
                    self.select(self.selected);
                    ctx.request_repaint();
                    if update.publish {
                        return Some(GraphicalUpdate::Changed(update.source.to_string()));
                    }
                }
                Err(JobError::QueueFull) => {
                    self.retry = Some(Instant::now() + Duration::from_millis(100))
                }
                Ok(Err(error)) => return self.fail(error),
                Err(error) => return self.fail(error.to_string()),
            }
        }
        if self.pending.is_some() || self.busy() {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
        None
    }

    fn fail(&mut self, error: String) -> Option<GraphicalUpdate> {
        let accepted = self.request.take().is_some();
        if accepted {
            self.edit_error = Some(error);
        } else {
            self.parse_error = Some(error);
        }
        accepted.then_some(GraphicalUpdate::Failed)
    }

    fn prepare(&mut self, ui: &egui::Ui, source: &str) {
        if self.source.is_none() {
            self.source = Some(Arc::from(source));
        }
        if self.pending.is_some() || self.model.is_some() || self.parse_error.is_some() {
            return;
        }
        if self.retry.is_some_and(|at| Instant::now() < at) {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
            return;
        }
        let source = self.source.clone().unwrap_or_else(|| Arc::from(source));
        self.pending = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(3),
            move |_| {
                let model = Arc::new(Model::parse(&source)?);
                Ok(Update {
                    source,
                    model,
                    publish: false,
                })
            },
        ));
    }

    fn start_edit(&mut self) {
        if self.retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let (Some(model), Some(source), Some(request)) = (
            self.model.clone(),
            self.source.clone(),
            self.request.as_ref(),
        ) else {
            return;
        };
        let edit = request.edit.clone();
        self.pending = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(3),
            move |_| edit_source(model, source, edit),
        ));
    }

    fn queue(&mut self, edit: Edit) {
        if self.busy() || self.pending.is_some() || self.model.is_none() {
            return;
        }
        self.edit_error = None;
        self.retry = None;
        self.request = Some(Request {
            edit,
            started: Instant::now(),
        });
        self.start_edit();
    }

    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> bool {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Harness explorer");
            if ui
                .add(ButtonRole::Secondary.button("Fit", palette))
                .clicked()
            {
                self.camera.fitted = false;
            }
            if ui.checkbox(&mut self.show_source, "Source").changed() {
                self.camera.fitted = false;
            }
            ui.small(format!("{:.0}%", self.camera.zoom_percent()));
            ui.small("Click a terminal to trace its path · scroll to zoom · middle-drag to pan");
        });
        self.show_source
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &str,
        preview: &mut Preview,
        palette: &AppPalette,
    ) {
        self.prepare(ui, source);
        let frame = preview.native_frame(ui, Format::Wireviz, source);
        let ready = frame.as_ref().is_some_and(|f| f.fresh)
            && self.model.is_some()
            && !self.busy()
            && self.pending.is_none();
        let height = (ui.available_height() - 102.0).max(100.0);
        let sidebar = (ui.available_width() * 0.28)
            .clamp(160.0, 260.0)
            .min((ui.available_width() - 92.0).max(0.0));
        ui.horizontal_top(|ui| {
            let width = (ui.available_width() - sidebar - 12.0).max(1.0);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
            self.canvas(ui, rect, &response, frame.as_ref(), ready, palette);
            ui.allocate_ui_with_layout(
                egui::vec2(sidebar, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_source("harness_inspector")
                        .min_scrolled_height(0.0)
                        .max_height(height)
                        .show(ui, |ui| {
                            ui.add_enabled_ui(ready, |ui| {
                                if let Some(edit) = self.inspector(ui, palette) {
                                    self.queue(edit);
                                    response.request_focus();
                                }
                            });
                        });
                },
            );
        });
        egui::ScrollArea::vertical()
            .id_source("harness_notices")
            .min_scrolled_height(0.0)
            .max_height(64.0)
            .show(ui, |ui| {
                if self.busy() {
                    ui.spinner();
                }
                if let Some(error) = self.edit_error.as_ref().or(self.parse_error.as_ref()) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(notice) = &self.canvas_notice {
                    ui.small(notice);
                }
                preview.status_ui(ui, Format::Wireviz)
            });
    }

    fn select(&mut self, endpoint: Option<Endpoint>) {
        self.selected = endpoint;
        self.connected.clear();
        let Some(model) = &self.model else { return };
        let harness = model.editable.harness();
        match endpoint {
            Some(Endpoint::Pin { connector, pin }) => {
                self.label = harness
                    .connectors
                    .get(connector)
                    .and_then(|c| c.pins.get(pin))
                    .and_then(|p| p.label.clone())
                    .unwrap_or_default()
            }
            Some(Endpoint::Cable {
                cable,
                conductor: Conductor::Wire(wire),
            }) => {
                self.color = harness
                    .cables
                    .get(cable)
                    .and_then(|c| c.wires.get(wire))
                    .and_then(|w| w.color.clone())
                    .unwrap_or_default()
            }
            _ => (),
        }
        let mut pending: Vec<_> = endpoint.into_iter().collect();
        while let Some(endpoint) = pending.pop() {
            if self.connected.insert(endpoint)
                && let Some(neighbors) = model.neighbors.get(&endpoint)
            {
                pending.extend(neighbors);
            }
        }
    }
}

fn edit_source(model: Arc<Model>, source: Arc<str>, edit: Edit) -> Result<Update, String> {
    let source = Arc::from(model.editable.apply(&source, edit)?);
    let model = Arc::new(Model::parse(&source)?);
    Ok(Update {
        source,
        model,
        publish: true,
    })
}

#[cfg(test)]
pub(crate) fn delayed_edit(
    source: &str,
    edit: Edit,
) -> (Box<WireEditor>, std::sync::mpsc::Sender<()>) {
    let source: Arc<str> = Arc::from(source);
    let model = Arc::new(Model::parse(&source).unwrap());
    let (release, wait) = std::sync::mpsc::channel();
    let captured = source.clone();
    let prepared = model.clone();
    let change = edit.clone();
    let pending = Task::start(
        BackgroundJobs::global(),
        Duration::from_secs(3),
        move |_| {
            wait.recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            edit_source(prepared, captured, change)
        },
    );
    (
        Box::new(WireEditor {
            source: Some(source),
            model: Some(model),
            request: Some(Request {
                edit,
                started: Instant::now(),
            }),
            pending: Some(pending),
            ..Default::default()
        }),
        release,
    )
}

#[cfg(test)]
mod tests;
