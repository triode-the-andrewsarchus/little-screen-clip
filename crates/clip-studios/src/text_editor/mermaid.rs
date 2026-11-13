//! Native flowchart interaction over the canonical Mermaid SVG and parser facts.
use crate::diagram_preview::{Camera, GraphicalUpdate, NativeFrame, Preview};
use clip_ui::buttons::ButtonRole;
use egui::{Color32, Rect, Sense, Stroke};
use little_clip_core::{
    BackgroundJobs, JobError,
    diagram::{
        Format,
        mermaid::{self, Change, Editable, Shape, Target},
    },
    jobs::task::Task,
    palette::AppPalette,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

struct Model {
    editable: Editable,
    selections: HashMap<String, Target>,
}
impl Model {
    fn parse(source: &str) -> Result<Self, String> {
        let editable = Editable::parse(source)?;
        let mut selections = HashMap::new();
        for node in &editable.nodes {
            selections.insert(
                mermaid::selection_id("node", &node.id),
                Target::Node(node.id.clone()),
            );
        }
        for edge in &editable.edges {
            let target = Target::Edge(edge.id.clone());
            selections.insert(mermaid::selection_id("edge", &edge.id), target.clone());
            selections.insert(mermaid::selection_id("edge-label", &edge.id), target);
        }
        Ok(Self {
            editable,
            selections,
        })
    }
}
struct Update {
    source: Arc<str>,
    model: Arc<Model>,
    changed: bool,
    proof: Option<(Target, bool)>,
}
struct Accepted {
    change: Change,
    started: Instant,
}

pub(super) struct MermaidEditor {
    source: Option<Arc<str>>,
    model: Option<Arc<Model>>,
    pending: Option<Task<Result<Update, String>>>,
    accepted: Option<Accepted>,
    proof_due: Option<Target>,
    retry: Option<Instant>,
    error: Option<String>,
    parse_error: Option<String>,
    selected: Option<Target>,
    delete_proved: bool,
    connect_from: Option<String>,
    label: String,
    new_label: String,
    connection_label: String,
    shape: Shape,
    camera: Camera,
    focus: Option<egui::Id>,
    show_source: bool,
    paint_notice: Option<String>,
}
impl Default for MermaidEditor {
    fn default() -> Self {
        Self {
            source: None,
            model: None,
            pending: None,
            accepted: None,
            proof_due: None,
            retry: None,
            error: None,
            parse_error: None,
            selected: None,
            delete_proved: false,
            connect_from: None,
            label: String::new(),
            new_label: "New step".into(),
            connection_label: String::new(),
            shape: Shape::Box,
            camera: Camera::default(),
            focus: None,
            show_source: true,
            paint_notice: None,
        }
    }
}
impl MermaidEditor {
    pub(super) fn busy(&self) -> bool {
        self.accepted.is_some()
    }
    pub(super) fn failed(&self) -> bool {
        self.error.is_some()
    }
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(super) fn has_focus(&self, ctx: &egui::Context) -> bool {
        self.focus
            .is_some_and(|id| ctx.memory(|memory| memory.has_focus(id)))
    }
    pub(super) fn suspend(&mut self) {
        self.connect_from = None;
        if !self.busy() {
            self.pending = None;
        }
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context, current: &str) -> Option<GraphicalUpdate> {
        if self
            .source
            .as_deref()
            .is_some_and(|source| source != current)
        {
            let replaced = self.busy();
            self.pending = None;
            self.accepted = None;
            self.proof_due = None;
            self.model = None;
            self.source = None;
            self.selected = None;
            self.delete_proved = false;
            self.connect_from = None;
            self.parse_error = None;
            self.error = replaced
                .then(|| "Mermaid source changed; the graphical edit was not applied.".into());
            self.retry = Some(Instant::now() + Duration::from_millis(120));
            return replaced.then_some(GraphicalUpdate::Replaced);
        }
        if self
            .accepted
            .as_ref()
            .is_some_and(|request| request.started.elapsed() > Duration::from_secs(4))
        {
            self.pending = None;
            self.accepted = None;
            self.error = Some("The Mermaid edit timed out; source was kept unchanged.".into());
            return Some(GraphicalUpdate::Failed);
        }
        let Some(result) = self.pending.as_ref().and_then(Task::poll) else {
            self.start();
            if self.pending.is_some() || self.busy() {
                ctx.request_repaint_after(Duration::from_millis(20));
            }
            return None;
        };
        self.pending = None;
        match result {
            Ok(Ok(update)) => {
                let changed = update.changed;
                self.source = Some(update.source.clone());
                self.model = Some(update.model);
                self.parse_error = None;
                if let Some((target, proved)) = update.proof {
                    if self.proof_due.as_ref() == Some(&target) {
                        self.proof_due = None;
                    }
                    if self.selected.as_ref() == Some(&target) {
                        self.delete_proved = proved;
                    }
                }
                if changed {
                    self.accepted = None;
                    self.error = None;
                    self.delete_proved = false;
                    self.select(self.selected.clone());
                    ctx.request_repaint();
                    return Some(GraphicalUpdate::Changed(update.source.to_string()));
                }
            }
            Err(JobError::QueueFull) => {
                self.retry = Some(Instant::now() + Duration::from_millis(100))
            }
            failure => {
                let error = match failure {
                    Ok(Err(error)) => error,
                    Err(error) => error.to_string(),
                    _ => unreachable!(),
                };
                if self.accepted.take().is_some() {
                    self.error = Some(error);
                    return Some(GraphicalUpdate::Failed);
                }
                self.parse_error = Some(error);
            }
        }
        self.start();
        ctx.request_repaint();
        None
    }

    fn start(&mut self) {
        if self.pending.is_some() || self.retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        if let Some(accepted) = &self.accepted {
            let Some(model) = self.model.clone() else {
                return;
            };
            let change = accepted.change.clone();
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(3),
                move |_| edit_source(model, source, change),
            ));
        } else if let Some(target) = self.proof_due.clone() {
            let Some(model) = self.model.clone() else {
                return;
            };
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(3),
                move |_| {
                    let proved = model.editable.can_delete(&target);
                    Ok(Update {
                        source,
                        model,
                        changed: false,
                        proof: Some((target, proved)),
                    })
                },
            ));
        } else if self.model.is_none() && self.parse_error.is_none() {
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(3),
                move |_| {
                    let model = Arc::new(Model::parse(&source)?);
                    Ok(Update {
                        source,
                        model,
                        changed: false,
                        proof: None,
                    })
                },
            ));
        }
    }

    fn queue(&mut self, change: Change) {
        if self.busy() || self.pending.is_some() || self.model.is_none() {
            return;
        }
        self.error = None;
        self.retry = None;
        self.accepted = Some(Accepted {
            change,
            started: Instant::now(),
        });
        self.start();
    }

    fn select(&mut self, target: Option<Target>) {
        self.selected = target;
        self.proof_due = self.selected.clone();
        self.delete_proved = false;
        self.label.clear();
        if let (Some(model), Some(target)) = (&self.model, &self.selected) {
            self.label = match target {
                Target::Node(id) => model
                    .editable
                    .nodes
                    .iter()
                    .find(|node| node.id == *id)
                    .map(|node| node.label.clone()),
                Target::Edge(id) => model
                    .editable
                    .edges
                    .iter()
                    .find(|edge| edge.id == *id)
                    .map(|edge| edge.label.clone()),
            }
            .unwrap_or_default();
            self.proof_due = Some(target.clone());
        }
    }

    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> bool {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Flowchart editor");
            if ui
                .add(ButtonRole::Secondary.button("Fit", palette))
                .clicked()
            {
                self.camera.fitted = false;
            }
            ui.checkbox(&mut self.show_source, "Source");
            ui.small("Select a node · connect steps · scroll to zoom · middle-drag to pan");
        });
        self.show_source
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &str,
        preview: &mut Preview,
        palette: &AppPalette,
    ) {
        if self.source.is_none() {
            self.source = Some(Arc::from(source));
        }
        self.start();
        let frame = preview.native_frame(ui, Format::Mermaid, source);
        let current = self.source.as_deref() == Some(source)
            && self.model.is_some()
            && !self.busy()
            && self.pending.is_none();
        let fresh = current && frame.as_ref().is_some_and(|frame| frame.fresh);
        let height = (ui.available_height() - 110.0).max(100.0);
        let sidebar = (ui.available_width() * 0.32)
            .clamp(150.0, 250.0)
            .min((ui.available_width() - 100.0).max(0.0));
        ui.horizontal_top(|ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2((ui.available_width() - sidebar - 10.0).max(1.0), height),
                Sense::click_and_drag(),
            );
            self.focus = Some(response.id);
            self.canvas(ui, rect, &response, frame.as_ref(), fresh, palette);
            ui.allocate_ui_with_layout(
                egui::vec2(sidebar, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_source("mermaid_inspector")
                        .min_scrolled_height(0.0)
                        .max_height(height)
                        .show(ui, |ui| {
                            ui.add_enabled_ui(current, |ui| self.inspector(ui, palette));
                        });
                },
            );
        });
        egui::ScrollArea::vertical()
            .id_source("mermaid_notices")
            .min_scrolled_height(0.0)
            .max_height(70.0)
            .show(ui, |ui| {
                if self.pending.is_some() || self.busy() {
                    ui.spinner();
                }
                if let Some(error) = self.error.as_ref().or(self.parse_error.as_ref()) {
                    if error.starts_with("Graphical editing currently supports") {
                        ui.small(error);
                    } else {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                }
                if let Some(notice) = &self.paint_notice {
                    ui.small(notice);
                }
                preview.status_ui(ui, Format::Mermaid);
            });
    }

    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        response: &egui::Response,
        frame: Option<&NativeFrame>,
        ready: bool,
        palette: &AppPalette,
    ) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::WHITE);
        let Some(frame) = frame else { return };
        if !self.camera.fitted {
            self.camera.fit(frame.layout, rect);
        }
        self.camera.input(ui, response, rect);
        let image = self.camera.image_rect(frame.layout, rect);
        self.paint_notice = frame.paint(ui, rect, image, Default::default(), Color32::WHITE);
        if !frame.fresh {
            painter.rect_filled(rect, 4.0, Color32::from_white_alpha(135));
            painter.text(
                rect.center_top() + egui::vec2(0.0, 10.0),
                egui::Align2::CENTER_TOP,
                "Updating Mermaid…",
                egui::FontId::proportional(14.0),
                Color32::DARK_GRAY,
            );
        }
        let (Some(model), Some(geometry)) = (&self.model, &frame.geometry) else {
            return;
        };
        let hits: Vec<_> = geometry
            .groups
            .iter()
            .filter_map(|(id, bounds)| {
                let base = id.strip_prefix("graph-0-")?.rsplit_once("-part-")?.0;
                model
                    .selections
                    .get(base)
                    .map(|target| (target.clone(), *bounds, base.contains("-edge-label-")))
            })
            .collect();
        let hover = if ready {
            response.hover_pos().and_then(|position| {
                let point = self.camera.world(position, rect);
                hit(
                    &hits,
                    egui::pos2(point[0] as f32, point[1] as f32),
                    (6.0 / self.camera.scale) as f32,
                )
            })
        } else {
            None
        };
        if ready && response.clicked() {
            response.request_focus();
            if let (Some(from), Some(Target::Node(to))) = (self.connect_from.take(), hover.clone())
            {
                self.queue(Change::Connect {
                    from,
                    to,
                    label: self.connection_label.clone(),
                });
            } else {
                self.select(hover.clone());
            }
        }
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        for (target, bounds, label) in hits {
            if self.selected.as_ref() == Some(&target) || hover.as_ref() == Some(&target) {
                if matches!(target, Target::Edge(_))
                    && !label
                    && bounds.width().min(bounds.height()) > 16.0
                {
                    continue;
                }
                let bounds = self.camera.screen_rect(bounds, rect);
                painter.rect_stroke(bounds.expand(2.0), 2.0, Stroke::new(2.0_f32, accent));
            }
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let Some(model) = self.model.clone() else {
            return;
        };
        self.add_panel(ui, palette, &model);
        ui.separator();
        self.selection_panel(ui, palette, &model);
        self.relationship_panel(ui, palette, &model);
    }

    fn add_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette, model: &Model) {
        ui.strong("Add step");
        ui.add(egui::TextEdit::singleline(&mut self.new_label).desired_width(ui.available_width()));
        egui::ComboBox::from_id_source("mermaid_new_shape")
            .selected_text(self.shape.name())
            .show_ui(ui, |ui| {
                for shape in [Shape::Box, Shape::Rounded, Shape::Decision] {
                    ui.selectable_value(&mut self.shape, shape, shape.name());
                }
            });
        if ui
            .add_enabled(
                !self.new_label.trim().is_empty(),
                ButtonRole::Secondary.button("Add node", palette),
            )
            .clicked()
        {
            let id = model.editable.next_id();
            self.selected = Some(Target::Node(id.clone()));
            self.queue(Change::AddNode {
                id,
                label: self.new_label.clone(),
                shape: self.shape,
            });
        }
    }

    fn selection_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette, model: &Model) {
        if let Some(target) = self.selected.clone() {
            match &target {
                Target::Node(id) => {
                    ui.strong(id);
                    if let Some(node) = model.editable.nodes.iter().find(|node| node.id == *id) {
                        ui.small(format!("Shape: {}", node.shape));
                    }
                }
                Target::Edge(id) => {
                    if let Some(edge) = model.editable.edges.iter().find(|edge| edge.id == *id) {
                        ui.strong(format!("{} → {}", edge.from, edge.to));
                    }
                }
            }
            ui.label("Label");
            ui.add(
                egui::TextEdit::multiline(&mut self.label)
                    .desired_rows(2)
                    .desired_width(ui.available_width()),
            );
            if ui
                .add_enabled(
                    !self.label.trim().is_empty() && model.editable.label_editable(&target),
                    ButtonRole::Secondary.button("Apply label", palette),
                )
                .clicked()
            {
                self.queue(match &target {
                    Target::Node(id) => Change::NodeLabel {
                        id: id.clone(),
                        label: self.label.clone(),
                    },
                    Target::Edge(id) => Change::EdgeLabel {
                        id: id.clone(),
                        label: self.label.clone(),
                    },
                });
            }
            if !model.editable.label_editable(&target) {
                ui.small("This label uses shared or structured syntax; edit it in Source.");
            }
            if let Target::Node(id) = &target {
                ui.label("Connection label (optional)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.connection_label)
                        .desired_width(ui.available_width()),
                );
                if ui
                    .add(ButtonRole::Secondary.button("Connect to node…", palette))
                    .clicked()
                {
                    self.connect_from = Some(id.clone());
                }
            }
            if ui.add_enabled(self.delete_proved, ButtonRole::Secondary.button("Delete", palette)).on_disabled_hover_text("Delete is available for standalone declarations that can be removed without changing other items. Remove connected nodes or compound statements in Source.").clicked() {
                self.queue(Change::Delete(target));
                self.selected = None;
            }
        } else {
            ui.small("Select a node or relationship to inspect it.");
        }
    }

    fn relationship_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette, model: &Model) {
        if let Some(from) = self.connect_from.clone() {
            ui.label(format!("Connect from {from}: click a destination node"));
            if ui
                .add(ButtonRole::Secondary.button("Cancel connection", palette))
                .clicked()
            {
                self.connect_from = None;
            }
        }
        ui.separator();
        ui.strong("Nodes");
        for node in &model.editable.nodes {
            let target = Target::Node(node.id.clone());
            if ui
                .selectable_label(
                    self.selected.as_ref() == Some(&target),
                    format!("{} · {}", node.id, node.label)
                        .chars()
                        .take(64)
                        .collect::<String>(),
                )
                .clicked()
            {
                if let Some(from) = self.connect_from.take() {
                    self.queue(Change::Connect {
                        from,
                        to: node.id.clone(),
                        label: self.connection_label.clone(),
                    });
                } else {
                    self.select(Some(target));
                }
            }
        }
        ui.strong("Relationships");
        for edge in &model.editable.edges {
            let target = Target::Edge(edge.id.clone());
            if ui
                .selectable_label(
                    self.selected.as_ref() == Some(&target),
                    format!("{} → {} {}", edge.from, edge.to, edge.label)
                        .chars()
                        .take(64)
                        .collect::<String>(),
                )
                .clicked()
            {
                self.select(Some(target));
            }
        }
    }
}

fn hit(hits: &[(Target, Rect, bool)], point: egui::Pos2, tolerance: f32) -> Option<Target> {
    hits.iter()
        .filter(|(target, bounds, label)| {
            let thin = bounds.width().min(bounds.height()) <= 2.0 * tolerance;
            (matches!(target, Target::Node(_)) || *label || thin)
                && bounds
                    .expand(if thin { tolerance } else { 0.0 })
                    .contains(point)
        })
        .min_by(|(a, ab, al), (b, bb, bl)| {
            let rank = |target: &Target, label: bool| {
                if matches!(target, Target::Node(_)) {
                    0
                } else if label {
                    1
                } else {
                    2
                }
            };
            rank(a, *al)
                .cmp(&rank(b, *bl))
                .then_with(|| ab.area().total_cmp(&bb.area()))
        })
        .map(|(target, _, _)| target.clone())
}
fn edit_source(model: Arc<Model>, source: Arc<str>, change: Change) -> Result<Update, String> {
    let source: Arc<str> = Arc::from(model.editable.apply(&source, change)?);
    let model = Arc::new(Model::parse(&source)?);
    Ok(Update {
        source,
        model,
        changed: true,
        proof: None,
    })
}

#[cfg(test)]
pub(super) fn delayed_edit(
    source: &str,
    change: Change,
) -> (Box<MermaidEditor>, std::sync::mpsc::Sender<()>) {
    let source: Arc<str> = Arc::from(source);
    let model = Arc::new(Model::parse(&source).unwrap());
    let (release, wait) = std::sync::mpsc::channel();
    let captured = source.clone();
    let prepared = model.clone();
    let gesture = change.clone();
    let pending = Task::start(
        BackgroundJobs::global(),
        Duration::from_secs(3),
        move |_| {
            wait.recv_timeout(Duration::from_secs(2))
                .map_err(|error| error.to_string())?;
            edit_source(prepared, captured, gesture)
        },
    );
    (
        Box::new(MermaidEditor {
            source: Some(source),
            model: Some(model),
            pending: Some(pending),
            accepted: Some(Accepted {
                change,
                started: Instant::now(),
            }),
            ..Default::default()
        }),
        release,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_node_bounds_win_over_edges_and_diagonal_rectangles_do_not_capture_empty_space() {
        let node = Target::Node("A".into());
        let edge = Target::Edge("e".into());
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let node_rect = Rect::from_min_max(egui::pos2(40.0, 40.0), egui::pos2(60.0, 60.0));
        let hits = vec![
            (edge.clone(), rect, false),
            (node.clone(), node_rect, false),
        ];
        assert_eq!(hit(&hits, egui::pos2(50.0, 50.0), 4.0), Some(node));
        assert_eq!(hit(&hits, egui::pos2(20.0, 80.0), 4.0), None);
        let label = Rect::from_min_max(egui::pos2(15.0, 75.0), egui::pos2(25.0, 85.0));
        assert_eq!(
            hit(&[(edge.clone(), label, true)], egui::pos2(20.0, 80.0), 4.0),
            Some(edge)
        );
    }

    #[test]
    fn accepted_mermaid_edits_survive_hiding_and_reject_newer_source() {
        let source = "flowchart LR\nA[Old]\n";
        for replace in [false, true] {
            let (mut editor, release) = delayed_edit(
                source,
                Change::NodeLabel {
                    id: "A".into(),
                    label: "New".into(),
                },
            );
            editor.suspend();
            assert!(editor.busy());
            assert!(editor.pending.is_some());
            let ctx = egui::Context::default();
            if replace {
                assert!(matches!(
                    editor.poll(&ctx, "flowchart LR\nA[Typed]\n"),
                    Some(GraphicalUpdate::Replaced)
                ));
                assert!(!editor.busy());
                assert!(editor.pending.is_none());
                let _ = release.send(());
            } else {
                release.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(4);
                loop {
                    if let Some(GraphicalUpdate::Changed(text)) = editor.poll(&ctx, source) {
                        assert_eq!(text, "flowchart LR\nA[\"New\"]\n");
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    #[test]
    fn selection_maps_are_derived_from_parser_ids_and_removal_proofs_expire_with_source() {
        let source = "flowchart LR\nalpha[Label]\nbeta[Other]\nalpha --> beta\n";
        let model = Arc::new(Model::parse(source).unwrap());
        assert_eq!(
            model
                .selections
                .get(&mermaid::selection_id("node", "alpha")),
            Some(&Target::Node("alpha".into()))
        );
        let mut editor = MermaidEditor {
            source: Some(Arc::from(source)),
            model: Some(model),
            ..Default::default()
        };
        editor.select(Some(Target::Node("alpha".into())));
        editor.delete_proved = true;
        editor.poll(&egui::Context::default(), "flowchart LR\nalpha[Newer]\n");
        assert!(!editor.delete_proved);
        assert!(editor.selected.is_none());
    }
}
