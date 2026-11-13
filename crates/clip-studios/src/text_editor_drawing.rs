//! Native whiteboard interaction, owning only derived in-memory editing work.
use crate::diagram_preview::Preview;
use egui::{Color32, Pos2, Rect, Vec2};
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
use canvas::{Camera, Gesture};

struct Update {
    source: String,
    scene: Arc<Scene>,
    publish: bool,
}
struct PendingEdit {
    change: Change,
    started: Instant,
}

pub(crate) enum Poll {
    Changed(String),
    Replaced,
    Failed,
}

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
    style: Style,
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
            self.gesture = None;
            self.ghost = None;
            self.parse_error = None;
            return accepted.then_some(Poll::Replaced);
        }
        if self
            .queued
            .as_ref()
            .is_some_and(|edit| edit.started.elapsed() > Duration::from_secs(3))
        {
            self.pending = None;
            self.queued = None;
            self.edit_error =
                Some("The canvas edit timed out; the source was kept unchanged.".into());
            return Some(Poll::Failed);
        }
        if self.busy() && self.pending.is_none() {
            self.start_edit();
        }
        let result = self.pending.as_ref().and_then(Task::poll);
        if let Some(result) = result {
            self.pending = None;
            match result {
                Ok(Ok(update)) => {
                    self.queued = None;
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
        if self.busy() {
            self.queued = None;
            self.ghost = None;
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
                })
            },
        ));
    }

    fn queue(&mut self, change: Change) {
        if self.busy() || self.model.is_none() || self.pending.is_some() {
            return;
        }
        self.edit_error = None;
        self.queued = Some(PendingEdit {
            change,
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
        let frame = preview.drawing_frame(ui, source);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::WHITE);
        if let Some(frame) = frame {
            if !self.camera.fitted {
                self.camera.fit(frame.layout, rect);
            }
            self.camera.input(ui, &response, rect);
            let image = self.camera.image_rect(frame.layout, rect);
            let tint = if frame.fresh {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(120)
            };
            painter.image(
                frame.texture,
                image,
                Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                tint,
            );
            let ready = frame.fresh && self.model.is_some() && !self.busy();
            if frame.fresh && !self.busy() {
                self.ghost = None;
            }
            if ready {
                self.interact(ui, &response, rect);
            }
            self.paint_feedback(&painter, rect, palette);
            if response.double_clicked_by(egui::PointerButton::Middle) {
                self.camera.fit(frame.layout, rect);
            }
        }
        self.footer(ui);
        egui::ScrollArea::vertical()
            .id_source("drawing_notices")
            .min_scrolled_height(0.0)
            .max_height(64.0)
            .show(ui, |ui| preview.status_ui(ui, Format::Excalidraw));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tool, None, "Select");
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
                }
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
        if let Some(error) = &self.parse_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.busy() {
            ui.small("Applying gesture…");
        } else {
            ui.small("Click to select · Shift adds to selection · drag to draw or move · wheel zooms · middle-drag pans · Delete removes");
        }
    }
}

fn edit_source(model: Arc<Scene>, change: Change) -> Result<Update, String> {
    if let Some(source) = model.apply(change).map_err(|error| error.to_string())? {
        let scene = Arc::new(Scene::parse(&source).map_err(|error| error.to_string())?);
        Ok(Update {
            source,
            scene,
            publish: true,
        })
    } else {
        Ok(Update {
            source: model.source().into(),
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
            edit_source(worker_model, worker_change)
        },
    );
    (
        Box::new(DrawingEditor {
            source: Some(Arc::from(source)),
            model: Some(model),
            pending: Some(task),
            queued: Some(PendingEdit {
                change,
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
