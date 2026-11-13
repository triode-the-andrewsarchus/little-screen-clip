//! Native whiteboard interaction, owning only derived in-memory editing work.
use crate::diagram_preview::Preview;
use egui::{Color32, Rect, Vec2};
use little_clip_core::{
    BackgroundJobs, JobError,
    diagram::{
        Format,
        drawing::{Bounds, Change, Item, Kind, Paint, Scene, Style, TOOLS},
    },
    jobs::task::Task,
    palette::AppPalette,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

mod canvas;
mod connections;
mod resize;
mod text;
use crate::diagram_preview::Camera;
use canvas::{Gesture, distance};
use connections::{Connections, Lines, group_id};
use text::{Action, TextDraft};

struct Update {
    source: String,
    scene: Arc<Scene>,
    publish: bool,
    lines: Arc<Lines>,
}
struct RetainedMove {
    gesture: Gesture,
    model: Arc<Scene>,
    lines: Arc<Lines>,
}
struct PendingEdit {
    change: Action,
    started: Instant,
}

use crate::diagram_preview::GraphicalUpdate as Poll;

#[derive(Default)]
pub(crate) struct DrawingEditor {
    source: Option<Arc<str>>,
    model: Option<Arc<Scene>>,
    pending: Option<Task<Result<Update, String>>>,
    queued: Option<PendingEdit>,
    retry: Option<Instant>,
    parse_error: Option<String>,
    edit_error: Option<String>,
    camera: Camera,
    tool: Option<Kind>,
    selected: Vec<String>,
    gesture: Option<Gesture>,
    ghost: Option<Gesture>,
    retained_move: Option<RetainedMove>,
    lines: Arc<Lines>,
    connections: Connections,
    style: Style,
    text_draft: Option<TextDraft>,
    text_selection: Option<String>,
    resize_available: bool,
    text_notice: Option<String>,
    focus: Option<egui::Id>,
}
impl DrawingEditor {
    pub(crate) fn has_focus(&self, ctx: &egui::Context) -> bool {
        self.focus
            .is_some_and(|id| ctx.memory(|memory| memory.has_focus(id)))
    }

    pub(crate) fn busy(&self) -> bool {
        self.queued.is_some()
    }
    pub(crate) fn failed(&self) -> bool {
        self.edit_error.is_some()
    }
    pub(crate) fn error(&self) -> Option<&str> {
        self.edit_error.as_deref()
    }

    pub(crate) fn suspend(&mut self) {
        self.gesture = None;
        self.connections.clear();
        // Accepted edits keep their owner and publish through Editor::poll even
        // after switching tabs. Hidden read-only model preparation can retire.
        if !self.busy() {
            self.pending = None;
        }
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context, source: &str) -> Option<Poll> {
        if self.source.as_deref().is_some_and(|old| old != source) {
            let accepted = self.busy();
            self.pending = None;
            self.queued = None;
            self.source = None;
            self.model = None;
            self.connections.clear();
            self.gesture = None;
            self.ghost = None;
            self.retained_move = None;
            self.parse_error = None;
            self.text_draft = None;
            self.text_selection = None;
            self.text_notice = None;
            return accepted.then_some(Poll::Replaced);
        }
        if self
            .queued
            .as_ref()
            .is_some_and(|edit| edit.started.elapsed() > Duration::from_secs(3))
        {
            self.pending = None;
            return self
                .task_error("The canvas edit timed out; the source was kept unchanged.".into());
        }
        if self.busy() && self.pending.is_none() {
            self.start_edit();
        }
        let result = self.pending.as_ref().and_then(Task::poll);
        if let Some(result) = result {
            self.pending = None;
            match result {
                Ok(Ok(update)) => {
                    if let Some(edit) = self.queued.take() {
                        self.accept_text(edit.change);
                    }
                    self.selected
                        .retain(|id| update.scene.items.iter().any(|item| &item.id == id));
                    if self.tool.is_none()
                        && let Some(item) = self
                            .selected
                            .first()
                            .and_then(|id| update.scene.items.iter().find(|item| &item.id == id))
                    {
                        self.style = item.style.clone();
                    }
                    self.model = Some(update.scene);
                    self.lines = update.lines;
                    self.source = Some(Arc::from(update.source.as_str()));
                    self.parse_error = None;
                    ctx.request_repaint();
                    if update.publish {
                        self.ghost = None;
                        self.edit_error = None;
                        return Some(Poll::Changed(update.source));
                    }
                }
                Err(JobError::QueueFull) => {
                    self.retry = Some(Instant::now() + Duration::from_millis(100))
                }
                Ok(Err(error)) => return self.task_error(error),
                Err(error) => return self.task_error(error.to_string()),
            }
        }
        if self.pending.is_some() || self.busy() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        None
    }

    fn task_error(&mut self, error: String) -> Option<Poll> {
        if let Some(edit) = self.queued.take() {
            self.reject_text(edit.change);
            self.ghost = None;
            self.retained_move = None;
            self.edit_error = Some(error);
            Some(Poll::Failed)
        } else {
            self.parse_error = Some(error);
            None
        }
    }

    fn start_edit(&mut self) {
        if self.retry.is_some_and(|due| Instant::now() < due) {
            return;
        }
        let (Some(model), Some(edit)) = (self.model.clone(), self.queued.as_ref()) else {
            return;
        };
        let change = edit.change.clone();
        self.pending = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(3),
            move |_| edit_source(model, change),
        ));
    }

    fn prepare_model(&mut self, source: &str) {
        if self.source.is_none() {
            self.source = Some(Arc::from(source));
        }
        if self.pending.is_some()
            || self.model.is_some()
            || self.parse_error.is_some()
            || self.retry.is_some_and(|due| Instant::now() < due)
        {
            return;
        }
        let source = self.source.clone().unwrap_or_else(|| Arc::from(source));
        self.pending = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(3),
            move |_| {
                let scene = Arc::new(Scene::parse(&source).map_err(|error| error.to_string())?);
                Ok(Update {
                    source: source.to_string(),
                    scene,
                    publish: false,
                    lines: Arc::new(little_clip_render::diagram::retained::drawing_lines(
                        &source,
                    )?),
                })
            },
        ));
    }

    fn queue(&mut self, change: impl Into<Action>) {
        if self.busy() || self.model.is_none() || self.pending.is_some() {
            return;
        }
        self.edit_error = None;
        self.queued = Some(PendingEdit {
            change: change.into(),
            started: Instant::now(),
        });
        self.start_edit();
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &str,
        preview: &mut Preview,
        palette: &AppPalette,
    ) {
        self.prepare_model(source);
        self.toolbar(ui, palette);
        let size = Vec2::new(
            ui.available_width().max(1.0),
            (ui.available_height() - 88.0).max(1.0),
        );
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        self.focus = Some(response.id);
        let frame = preview.native_frame(ui, Format::Excalidraw, source);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::WHITE);
        self.resize_available = frame.as_ref().is_some_and(|frame| frame.vector.is_some());
        if let Some(frame) = frame {
            self.camera.fit_if_ready(frame.layout, rect, frame.fresh);
            self.camera.input(ui, &response, rect);
            let image = self.camera.image_rect(frame.layout, rect);
            let tint = if frame.fresh {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(120)
            };
            let ready = frame.fresh && self.model.is_some() && !self.busy();
            if frame.fresh && !self.busy() {
                self.ghost = None;
                self.retained_move = None;
            }
            if ready {
                self.interact(ui, &response, rect);
            }
            let motion = self.vector_motion(ui.ctx(), frame.layout.origin);
            if let Some(error) = frame.paint(ui, rect, image, motion, tint) {
                self.parse_error = Some(error);
            }
            self.paint_feedback(&painter, rect, palette);
            if response.double_clicked_by(egui::PointerButton::Middle) {
                self.camera.fitted = false;
                self.camera.fit_if_ready(frame.layout, rect, frame.fresh);
            }
        }
        self.keyboard(ui, &response);
        self.text_controls(ui);
        self.footer(ui);
        egui::ScrollArea::vertical()
            .id_source("drawing_notices")
            .min_scrolled_height(0.0)
            .max_height(64.0)
            .show(ui, |ui| preview.status_ui(ui, Format::Excalidraw));
    }

    fn vector_motion(
        &mut self,
        ctx: &egui::Context,
        origin: [f64; 2],
    ) -> clip_ui::vectors::svg::Changes {
        let mut changes = clip_ui::vectors::svg::Changes::default();
        let retained = self.retained_move.as_ref();
        let gesture = self
            .gesture
            .as_ref()
            .or(self.ghost.as_ref())
            .or(retained.map(|retained| &retained.gesture));
        let model = retained
            .map(|retained| &retained.model)
            .or(self.model.as_ref());
        let lines = retained.map_or(&self.lines, |retained| &retained.lines);
        if let (Some(Gesture::Resize { id, current, .. }), Some(model)) = (gesture, model) {
            let model = model.clone();
            let lines = lines.clone();
            let id = id.clone();
            let current = *current;
            match self.resize_motion(ctx, &model, &lines, &id, current, origin) {
                Ok(changes) => return changes,
                Err(error) => {
                    self.edit_error = Some(error);
                    return changes;
                }
            }
        }
        if let (
            Some(Gesture::Move {
                start,
                current,
                ids,
            }),
            Some(model),
        ) = (gesture, model)
        {
            let delta = [current[0] - start[0], current[1] - start[1]];
            match model.movement(ids, delta) {
                Ok(movement) => {
                    let model = model.clone();
                    let lines = lines.clone();
                    changes = self.movement_artwork(ctx, &model, &lines, movement);
                }
                Err(error) => self.edit_error = Some(error.to_string()),
            }
        } else {
            self.connections.clear();
        }
        changes
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_value(&mut self.tool, None, "Select")
                .changed()
            {
                self.text_draft = None;
                self.text_selection = None;
            }
            for kind in TOOLS {
                let label = match kind {
                    Kind::Rectangle => "Box",
                    Kind::Freedraw => "Pencil",
                    _ => kind.as_str(),
                };
                if ui
                    .selectable_value(&mut self.tool, Some(kind.clone()), label)
                    .changed()
                {
                    self.selected.clear();
                    self.text_draft = None;
                    self.text_selection = None;
                }
            }
            if ui
                .selectable_value(&mut self.tool, Some(Kind::Text), "Text")
                .changed()
            {
                self.selected.clear();
                self.text_draft = None;
            }
            if ui
                .small_button("Add label")
                .on_hover_text("Add a label to a selected rectangle that has no label")
                .clicked()
                && let Some(id) = self.selected.first().cloned()
            {
                self.new_text([0.0; 2], Some(id));
            }
            if ui
                .add(
                    clip_ui::buttons::ButtonRole::Secondary
                        .button("Fit", palette)
                        .small(),
                )
                .clicked()
            {
                self.camera.fitted = false;
            }
            if self.camera.fitted {
                ui.small(format!("{:.0}%", self.camera.zoom_percent()));
            }
            ui.separator();
            for (label, color) in [
                ("Ink", "#1e1e1e"),
                ("Blue", "#1971c2"),
                ("Red", "#e03131"),
                ("Green", "#2f9e44"),
            ] {
                if ui.small_button(label).clicked() {
                    self.paint_control(Paint::Stroke(color.into()));
                }
            }
            if ui.small_button("Fill").clicked() {
                let fill = if self.style.fill == "transparent" {
                    "#d0ebff"
                } else {
                    "transparent"
                };
                self.paint_control(Paint::Fill(fill.into()));
            }
            egui::ComboBox::from_id_source("drawing_width")
                .width(36.0)
                .selected_text(format!("{} px", self.style.width))
                .show_ui(ui, |ui| {
                    for width in [1.0, 2.0, 4.0, 8.0] {
                        if ui
                            .selectable_label(self.style.width == width, format!("{width} px"))
                            .clicked()
                        {
                            self.paint_control(Paint::Width(width));
                        }
                    }
                });
        });
    }
    fn paint_control(&mut self, paint: Paint) {
        if self.busy() {
            return;
        }
        match &paint {
            Paint::Stroke(v) => self.style.stroke = v.clone(),
            Paint::Fill(v) => self.style.fill = v.clone(),
            Paint::Width(v) => self.style.width = *v,
            Paint::Roughness(v) => self.style.roughness = *v,
            Paint::Opacity(v) => self.style.opacity = *v,
        }
        if self.tool.is_none() && !self.selected.is_empty() {
            self.queue(Change::Paint {
                ids: self.selected.clone(),
                paint,
            });
        }
    }
    fn footer(&self, ui: &mut egui::Ui) {
        if let Some(error) = self
            .parse_error
            .as_ref()
            .or(self.edit_error.as_ref())
            .or(self.connections.error.as_ref())
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(notice) = &self.text_notice {
            ui.small(notice);
        }
        if let (Some(model), Some(id)) = (&self.model, self.selected.first())
            && let Some(item) = model.items.iter().find(|item| &item.id == id)
        {
            if let Err(error) = model.resize(id, item.bounds) {
                ui.small(format!("Resize: {error}"));
            } else if !self.resize_available {
                ui.small(
                    "Resize needs native vector artwork; use Source for this effect fallback.",
                );
            }
        }
        if self.busy() {
            ui.small("Applying gesture…");
        } else {
            ui.small("Click to select · Shift adds to selection · drag to draw or move · corner handles resize · wheel zooms · middle-drag pans · Delete removes");
        }
    }
}

fn edit_source(model: Arc<Scene>, change: Action) -> Result<Update, String> {
    let change = change.resolve(&model)?;
    if let Some(source) = model.apply(change).map_err(|error| error.to_string())? {
        let scene = Arc::new(Scene::parse(&source).map_err(|error| error.to_string())?);
        let lines = Arc::new(little_clip_render::diagram::retained::drawing_lines(
            &source,
        )?);
        Ok(Update {
            source,
            scene,
            publish: true,
            lines,
        })
    } else {
        Ok(Update {
            source: model.source().into(),
            lines: Arc::new(little_clip_render::diagram::retained::drawing_lines(
                model.source(),
            )?),
            scene: model,
            publish: false,
        })
    }
}

#[cfg(test)]
pub(crate) fn delayed_edit(
    source: &str,
    change: Change,
) -> (Box<DrawingEditor>, std::sync::mpsc::Sender<()>) {
    let model = Arc::new(Scene::parse(source).unwrap());
    let (release, wait) = std::sync::mpsc::channel();
    let worker_model = Arc::clone(&model);
    let worker_change = change.clone();
    let task = Task::start(
        BackgroundJobs::global(),
        Duration::from_secs(3),
        move |_| {
            wait.recv_timeout(Duration::from_secs(2))
                .map_err(|error| error.to_string())?;
            edit_source(worker_model, worker_change.into())
        },
    );
    (
        Box::new(DrawingEditor {
            source: Some(Arc::from(source)),
            model: Some(model),
            pending: Some(task),
            queued: Some(PendingEdit {
                change: change.into(),
                started: Instant::now(),
            }),
            ..Default::default()
        }),
        release,
    )
}

#[cfg(test)]
pub(crate) fn focused_fixture(id: egui::Id) -> Box<DrawingEditor> {
    Box::new(DrawingEditor {
        focus: Some(id),
        ..Default::default()
    })
}
