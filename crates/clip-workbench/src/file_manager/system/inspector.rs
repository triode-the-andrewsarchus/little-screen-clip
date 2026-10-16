use super::{
    Document, Relation,
    cache::{self, RefreshPolicy},
    jobs,
};
use crate::file_manager::preview_job::Task;
use std::{path::PathBuf, time::Duration};

pub(in crate::file_manager) struct Inspector {
    pub document: Document,
    history: Vec<(Document, Option<String>)>,
    pending: Option<Task<Result<Document, String>>>,
    browsing: Option<Task<Result<BrowserTarget, String>>>,
    browser_target: Option<BrowserTarget>,
    error: Option<String>,
    refreshed: std::time::Instant,
    next_delay: Duration,
    console: super::console::Capture,
}
#[derive(Clone, Debug)]
pub(in crate::file_manager) struct BrowserTarget {
    pub folder: PathBuf,
    pub reveal: Option<PathBuf>,
    relation: Relation,
    parent: Relation,
    namespace: super::identity::Namespace,
}
impl BrowserTarget {
    pub fn new(relation: Relation) -> Result<Self, String> {
        relation.verify()?;
        let folder = if relation.directory {
            relation.target.clone()
        } else {
            relation
                .target
                .parent()
                .ok_or("Target has no parent directory")?
                .into()
        };
        Ok(Self {
            parent: Relation::new("Browser directory", folder.clone()),
            folder,
            reveal: (!relation.directory).then(|| relation.target.clone()),
            relation,
            namespace: super::identity::Namespace::current(),
        })
    }
    pub fn read_directory(
        &self,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::file_manager::filesystem::Directory, String> {
        self.verify()?;
        let directory = crate::file_manager::filesystem::read_directory(&self.folder, cancelled)?;
        self.verify()?;
        Ok(directory)
    }
    fn verify(&self) -> Result<(), String> {
        if !self.namespace.known() || self.namespace != super::identity::Namespace::current() {
            return Err("Namespace view changed; reopen the inspector".into());
        }
        self.parent.verify()?;
        self.relation.verify()
    }
}
impl Inspector {
    pub fn new(document: Document) -> Self {
        let error = document.last_error.clone();
        Self {
            document,
            history: Vec::new(),
            pending: None,
            browsing: None,
            browser_target: None,
            error,
            refreshed: std::time::Instant::now(),
            next_delay: Duration::from_secs(1),
            console: Default::default(),
        }
    }
    fn request(&mut self, path: PathBuf, policy: RefreshPolicy) {
        self.request_relation(path, None, policy);
    }
    fn request_relation(
        &mut self,
        path: PathBuf,
        relation: Option<Relation>,
        policy: RefreshPolicy,
    ) {
        self.refreshed = std::time::Instant::now();
        if path != self.document.path || relation.is_some() {
            self.console = Default::default();
        }
        self.browsing = None;
        let namespace = self.document.namespace.clone();
        self.pending = Some(
            Task::start(jobs(), Duration::from_secs(3), move |token| {
                if namespace != super::identity::Namespace::current() {
                    return Err("Namespace view changed; reopen the inspector".into());
                }
                if let Some(relation) = &relation {
                    relation.verify()?;
                }
                let document = cache::collect_for(&path, token, policy)?;
                if let Some(relation) = relation {
                    relation.verify()?;
                    if let Some(start) = relation.process_start {
                        if document
                            .observed_raw("Process start tick")
                            .and_then(|value| value.parse::<u64>().ok())
                            != Some(start)
                        {
                            return Err(
                                "Process lifetime changed; refresh the originating device".into()
                            );
                        }
                    }
                }
                Ok(document)
            })
            .with_response_deadline(super::RESPONSE_DEADLINE),
        );
    }
    fn browse(&mut self, relation: Relation) {
        self.console = Default::default();
        self.pending = None;
        let namespace = self.document.namespace.clone();
        self.browsing = Some(
            Task::start(jobs(), Duration::from_secs(3), move |_| {
                let target = BrowserTarget::new(relation)?;
                if namespace != target.namespace {
                    return Err("Namespace view changed; reopen the inspector".into());
                }
                Ok(target)
            })
            .with_response_deadline(super::RESPONSE_DEADLINE),
        );
    }
    pub fn take_browser_target(&mut self) -> Option<BrowserTarget> {
        self.browser_target.take()
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.pending.as_ref().and_then(|task| task.poll()) {
            self.pending = None;
            match result {
                Ok(Ok(mut document)) => {
                    document.inherit_retained_provenance(&self.document);
                    if document.path != self.document.path {
                        self.history
                            .push((self.document.clone(), self.error.clone()));
                        if self.history.len() > 64 {
                            self.history.remove(0);
                        }
                    }
                    if !super::identity::same_interface(&self.document, &document)
                        || self.document.namespace != document.namespace
                    {
                        self.console = Default::default();
                    }
                    self.document = document;
                    self.error = self.document.last_error.clone();
                    self.next_delay = if self.error.is_some() {
                        (self.next_delay * 2).min(Duration::from_secs(30))
                    } else {
                        Duration::from_secs(1)
                    };
                }
                error => {
                    self.error = Some(format!("{error:?}"));
                    self.next_delay = (self.next_delay * 2).min(Duration::from_secs(30));
                }
            }
        }
        if let Some(result) = self.browsing.as_ref().and_then(|task| task.poll()) {
            self.browsing = None;
            match result {
                Ok(Ok(target)) => self.browser_target = Some(target),
                error => self.error = Some(format!("{error:?}")),
            }
        }
        if self.pending.is_none()
            && self.browsing.is_none()
            && self.browser_target.is_none()
            && self.refreshed.elapsed() >= self.next_delay
        {
            self.request(self.document.path.clone(), RefreshPolicy::Inspector);
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    fn header(&self, ui: &mut egui::Ui) {
        ui.heading(&self.document.title);
        let age = super::model::now_ms().saturating_sub(self.document.observed_ms) / 1000;
        if let Some(status) = self
            .document
            .observation_status(Duration::from_secs(age), Duration::from_secs(2))
        {
            ui.label(format!("{status}; values may be stale."));
        }
        ui.label(super::super::display_path(&self.document.path));
        ui.label(format!(
            "Observed {} · {}s ago · read-only inspection",
            chrono::DateTime::from_timestamp_millis(self.document.observed_ms as i64)
                .map(|t| t
                    .with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string())
                .unwrap_or_else(|| "at unknown time".into()),
            age
        ));
        ui.label(format!(
            "Namespace view: mount {:?}, PID {:?}, network {:?}, cgroup {:?} · object generation {} · {}",
            self.document.namespace.mount,
            self.document.namespace.pid,
            self.document.namespace.network,
            self.document.namespace.cgroup,
            self.document.generation,
            if super::events::available() {
                "udev monitoring plus visible revalidation"
            } else {
                "udev unavailable; visible revalidation"
            }
        ));
        if let Some(error) = &self.error {
            ui.label(format!(
                "Observation incomplete; available values retained: {error}"
            ));
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        close: &mut bool,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        *close |= ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        self.poll(ui.ctx());
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.history.is_empty(),
                    egui::Button::new("Back in inspector"),
                )
                .clicked()
            {
                self.pending = None;
                self.browsing = None;
                self.console = Default::default();
                (self.document, self.error) = self.history.pop().unwrap();
                self.refreshed = std::time::Instant::now();
            }
            if ui.button("Refresh").clicked() {
                self.request(self.document.path.clone(), RefreshPolicy::Manual);
            }
            if self.pending.is_some() || self.browsing.is_some() {
                ui.spinner();
                ui.ctx().request_repaint_after(Duration::from_millis(40));
            }
        });
        self.header(ui);
        let action = egui::ScrollArea::vertical()
            .id_source((
                "system-inspector-document",
                &self.document.path,
                self.document.generation,
            ))
            .show(ui, |ui| {
                if let Some(data) = &self.document.telemetry {
                    super::telemetry::inspector(
                        ui,
                        data,
                        crate::ui_appearance::WorkbenchColors::from_palette(palette),
                        palette,
                    );
                }
                if let Some(cpu) = &self.document.cpu {
                    super::cpu::inspector(
                        ui,
                        cpu,
                        crate::ui_appearance::WorkbenchColors::from_palette(palette),
                        palette,
                    );
                }
                if let Some(storage) = &self.document.storage {
                    super::storage::inspector(
                        ui,
                        storage,
                        crate::ui_appearance::WorkbenchColors::from_palette(palette),
                    );
                }
                if let Some(data) = &self.document.numa {
                    super::numa::inspector(
                        ui,
                        data,
                        crate::ui_appearance::WorkbenchColors::from_palette(palette),
                        palette,
                    );
                }
                self.console.show(ui, &self.document);
                self.facts_ui(ui);
                if let Some(topology) = &self.document.topology {
                    ui.heading(&topology.heading);
                    ui.label(&topology.summary);
                    ui.label(format!("Coverage: {}", topology.coverage));
                    for node in &topology.nodes {
                        let parent = node
                            .parent
                            .and_then(|index| topology.nodes.get(index))
                            .map(|node| node.label.as_str())
                            .unwrap_or("Root");
                        ui.label(format!("{parent} → {}", node.label));
                    }
                }
                self.relations_ui(ui)
            })
            .inner;
        match action {
            Some(RelationAction::Inspect(relation)) => self.request_relation(
                relation.target.clone(),
                Some(relation),
                RefreshPolicy::Manual,
            ),
            Some(RelationAction::Browse(relation)) => self.browse(relation),
            None => {}
        }
    }
    fn facts_ui(&self, ui: &mut egui::Ui) {
        egui::Grid::new("system-facts")
            .min_col_width(0.0)
            .max_col_width((ui.available_width() - 80.0).max(48.0) / 2.0)
            .striped(true)
            .show(ui, |ui| {
                for fact in &self.document.facts {
                    if fact.value.is_none() && fact.outcome == super::model::Outcome::Missing {
                        continue;
                    }
                    ui.add(egui::Label::new(&fact.label).wrap(true));
                    let display = match &fact.value {
                        Some(value) if fact.retained => {
                            format!("{value} · {} (last observation)", fact.outcome)
                        }
                        Some(value) => value.clone(),
                        None => fact.outcome.to_string(),
                    };
                    ui.add(egui::Label::new(display).wrap(true))
                        .on_hover_text(format!(
                            "{}\n{} · {}\nObserved {}ms ago · generation {}{}",
                            fact.source.display(),
                            fact.origin,
                            fact.outcome,
                            super::model::now_ms().saturating_sub(fact.observed_ms),
                            fact.observed_generation.map_or_else(
                                || "unknown".into(),
                                |generation| generation.to_string()
                            ),
                            fact.error
                                .as_ref()
                                .map_or(String::new(), |e| format!("\n{e}"))
                        ));
                    if let Some(value) = &fact.value {
                        if ui.small_button("Copy value").clicked() {
                            ui.output_mut(|out| out.copied_text = value.clone());
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
    }
    fn relations_ui(&self, ui: &mut egui::Ui) -> Option<RelationAction> {
        ui.separator();
        ui.label("Related kernel objects");
        let plain = ui.input(|input| {
            !input.modifiers.ctrl && !input.modifiers.shift && !input.modifiers.command
        });
        let mut action = None;
        for relation in &self.document.relations {
            ui.horizontal(|ui| {
                let response = ui
                    .add_enabled_ui(relation.identity.is_some(), |ui| {
                        ui.link(format!(
                            "{} → {}",
                            relation.label,
                            super::super::display_path(&relation.target)
                        ))
                        .interact(egui::Sense::click_and_drag())
                    })
                    .inner;
                if response.drag_started() {
                    response.dnd_set_drag_payload(crate::file_manager::transfers::FileDrag {
                        paths: vec![relation.target.clone()],
                        copy_only: true,
                    });
                }
                if response.clicked() && plain {
                    action = Some(if relation.system {
                        RelationAction::Inspect(relation.clone())
                    } else {
                        RelationAction::Browse(relation.clone())
                    });
                }
                if relation.identity.is_none() {
                    response
                        .on_hover_text("Target identity is unavailable; refresh before navigating");
                }
                let label = if relation.directory {
                    "Browse folder"
                } else {
                    "Reveal in folder"
                };
                if ui
                    .add_enabled(
                        relation.identity.is_some() && relation.process_start.is_none(),
                        egui::Button::new(label),
                    )
                    .clicked()
                    && plain
                {
                    action = Some(RelationAction::Browse(relation.clone()));
                }
            });
        }
        action
    }
}

enum RelationAction {
    Inspect(Relation),
    Browse(Relation),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_completion_preserves_original_generation_of_retained_observations() {
        let previous = super::super::retained_provenance_fixture();
        let mut current = previous.clone();
        current.facts[0].retained = true;
        current.facts[0].observed_generation = None;
        current.facts[0].error = Some("refresh failed".into());
        let mut inspector = Inspector::new(previous);
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        inspector.pending = Some(Task::start(&jobs, Duration::from_secs(3), move |_| {
            Ok(current)
        }));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while inspector.pending.is_some() {
            inspector.poll(&egui::Context::default());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let fact = &inspector.document.facts[0];
        assert_eq!(fact.observed_generation, Some(40));
        assert_eq!(fact.observed_ms, 123_000);
        assert!(fact.retained);
        assert_eq!(fact.error.as_deref(), Some("refresh failed"));
    }

    #[test]
    fn relationship_drag_emits_a_reference_and_cancels_link_activation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("related-file");
        std::fs::write(&path, "reference only").unwrap();
        let mut document = super::super::worker::interface(&path).unwrap();
        document.relations = vec![Relation::new("Drag reference", path.clone())];
        let inspector = Inspector::new(document);
        let context = egui::Context::default();
        let mut action = None;
        let mut frame = |events| {
            context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = inspector.relations_ui(ui);
                    });
                },
            )
        };
        let first = frame(vec![]);
        let position = first
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.text().starts_with("Drag reference") =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("relationship link");
        frame(vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let dragged = position + egui::vec2(40.0, 0.0);
        frame(vec![egui::Event::PointerMoved(dragged)]);
        let payload =
            egui::DragAndDrop::payload::<crate::file_manager::transfers::FileDrag>(&context)
                .expect("dragged relationship reference");
        assert_eq!(payload.paths, vec![path]);
        assert!(
            payload.copy_only,
            "inspector references must never advertise a move"
        );
        frame(vec![egui::Event::PointerButton {
            pos: dragged,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(action.is_none(), "drag release must not navigate");
    }

    #[test]
    fn browser_handoff_rejects_unknown_namespace_and_replaced_parent() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let file = parent.join("file");
        std::fs::File::create(&file).unwrap();
        let target = BrowserTarget::new(Relation::new("file", file.clone())).unwrap();
        assert!(
            target
                .read_directory(&std::sync::atomic::AtomicBool::new(false))
                .is_ok()
        );
        let mut unknown = target.clone();
        unknown.namespace.mount = None;
        assert!(unknown.verify().unwrap_err().contains("Namespace"));
        // Keep the file's inode alive while replacing only its parent directory.
        let old = temp.path().join("old-parent");
        std::fs::rename(&parent, &old).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::hard_link(old.join("file"), &file).unwrap();
        assert!(
            target
                .read_directory(&std::sync::atomic::AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn rendered_back_restores_document_error_and_original_fact_provenance() {
        for (document_error, previous_error) in [
            (None, None),
            (
                Some("Attribute observation timed out"),
                Some("Attribute observation timed out"),
            ),
            (None, Some("Refresh response deadline exceeded")),
        ] {
            let previous_error = previous_error.map(str::to_owned);
            let mut previous =
                super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
            previous.last_error = document_error.map(str::to_owned);
            previous.generation = 41;
            let mut fact = super::super::model::Fact::text(
                "Previous value",
                "retained reading",
                previous.path.clone(),
            );
            fact.observed_ms = 123;
            fact.observed_generation = Some(40);
            fact.retained = true;
            previous.facts = vec![fact];
            previous.relations.clear();
            let mut current = previous.clone();
            current.path = "/proc/version".into();
            current.last_error = Some("Different object's error".into());
            let mut inspector = Inspector::new(current);
            inspector
                .history
                .push((previous.clone(), previous_error.clone()));
            let context = egui::Context::default();
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let mut close = false;
            let first = context.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| inspector.ui(ui, &mut close, &palette));
            });
            let position = first
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text)
                        if text.galley.text() == "Back in inspector" =>
                    {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("rendered inspector Back button");
            for pressed in [true, false] {
                let _ = context.run(
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(position),
                            egui::Event::PointerButton {
                                pos: position,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default()
                            .show(ctx, |ui| inspector.ui(ui, &mut close, &palette));
                    },
                );
            }
            assert_eq!(inspector.document.path, previous.path);
            assert_eq!(inspector.error, previous_error);
            assert_eq!(inspector.document.generation, 41);
            assert_eq!(inspector.document.facts[0].observed_ms, 123);
            assert_eq!(inspector.document.facts[0].observed_generation, Some(40));
            assert!(inspector.document.facts[0].retained);
            assert!(inspector.history.is_empty());
            assert!(inspector.pending.is_none());
            assert!(inspector.take_browser_target().is_none());
            assert!(!close);
        }
    }
}
