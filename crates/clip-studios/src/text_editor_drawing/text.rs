//! Text drafts are measured in the existing accepted edit job, then one source edit.
use super::*;
use little_clip_core::diagram::drawing::{TextContent, TextCreation, TextStyle};

#[derive(Clone, PartialEq)]
pub(super) enum TextTarget {
    Existing(String),
    New {
        id: String,
        at: [f64; 2],
        container: Option<String>,
        style: Style,
    },
}
impl TextTarget {
    fn id(&self) -> &str {
        match self {
            Self::Existing(id) | Self::New { id, .. } => id,
        }
    }
}
#[derive(Clone, PartialEq)]
pub(super) struct TextDraft {
    pub target: TextTarget,
    pub value: String,
}
#[derive(Clone)]
pub(super) enum Action {
    Change(Change),
    Text(TextDraft),
}
impl From<Change> for Action {
    fn from(change: Change) -> Self {
        Self::Change(change)
    }
}

impl Action {
    pub(super) fn resolve(self, model: &Scene) -> Result<Change, String> {
        match self {
            Self::Change(change) => Ok(change),
            Self::Text(draft) => {
                let font = match &draft.target {
                    TextTarget::Existing(id) => {
                        model
                            .text_info(id)
                            .map_err(|e| e.to_string())?
                            .ok_or("No text selected")?
                            .font
                    }
                    TextTarget::New { .. } => TextStyle::default(),
                };
                let size = little_clip_render::diagram::measure_drawing_text(
                    &draft.value,
                    font.family,
                    font.size,
                    font.line_height,
                )?;
                let content = TextContent::plain(
                    draft.value,
                    excalidraw_number(size[0])?,
                    excalidraw_number(size[1])?,
                );
                match draft.target {
                    TextTarget::Existing(id) => Ok(Change::Text { id, content }),
                    TextTarget::New {
                        id,
                        mut at,
                        container,
                        style,
                    } => {
                        if let Some(container) = &container {
                            let bounds = model
                                .items
                                .iter()
                                .find(|item| &item.id == container)
                                .ok_or("Container disappeared")?
                                .bounds;
                            at = [
                                bounds.x + (bounds.width - size[0]) / 2.0,
                                bounds.y + (bounds.height - size[1]) / 2.0,
                            ];
                        }
                        Ok(Change::CreateText(TextCreation {
                            id,
                            position: at,
                            content,
                            font,
                            style,
                            container,
                        }))
                    }
                }
            }
        }
    }
}
fn excalidraw_number(value: f64) -> Result<little_clip_core::diagram::drawing::Number, String> {
    little_clip_core::diagram::drawing::Number::from_f64(value).map_err(|error| error.to_string())
}

impl DrawingEditor {
    pub(super) fn accept_text(&mut self, action: Action) {
        if let Action::Text(draft) = action
            && self.text_draft.as_ref() == Some(&draft)
        {
            self.text_draft = None;
        }
    }

    pub(super) fn reject_text(&mut self, action: Action) {
        if let Action::Text(draft) = action
            && self.text_draft.is_none()
            && self.text_selection.as_deref() == Some(draft.target.id())
        {
            self.text_draft = Some(draft);
        }
    }

    pub(super) fn new_text(&mut self, at: [f64; 2], container: Option<String>) {
        let id = uuid::Uuid::new_v4().to_string();
        self.text_selection = Some(id.clone());
        self.text_draft = Some(TextDraft {
            target: TextTarget::New {
                id,
                at,
                container,
                style: self.style.clone(),
            },
            value: "Text".into(),
        });
    }

    pub(super) fn text_controls(&mut self, ui: &mut egui::Ui) {
        self.text_notice = None;
        if self.tool.is_none()
            && self.selected.is_empty()
            && self
                .text_draft
                .as_ref()
                .is_some_and(|draft| matches!(draft.target, TextTarget::Existing(_)))
        {
            self.text_draft = None;
            self.text_selection = None;
        }
        if self.tool.is_none()
            && !self.busy()
            && self.text_draft.is_none()
            && let (Some(model), Some(id)) = (&self.model, self.selected.first())
        {
            match model.text_info(id) {
                Ok(Some(info)) if self.text_selection.as_deref() != Some(info.id.as_str()) => {
                    self.text_selection = Some(info.id.clone());
                    self.text_draft = Some(TextDraft {
                        target: TextTarget::Existing(info.id),
                        value: info.text,
                    });
                }
                Err(error) => {
                    self.text_notice = Some(error.to_string());
                }
                _ => {}
            }
        }
        let Some(mut draft) = self.text_draft.take() else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        egui::Window::new("Drawing text").id(egui::Id::new("drawing_text_inspector")).open(&mut open).collapsible(false).resizable(false).default_width(320.0).default_pos(ui.min_rect().right_top() + egui::vec2(-340.0, 40.0)).show(ui.ctx(), |ui| {
            ui.add_enabled_ui(!self.busy(), |ui| {
                ui.add(egui::TextEdit::multiline(&mut draft.value).desired_width(300.0).desired_rows(3));
                if draft.value.chars().count() > 4096 { ui.colored_label(ui.visuals().error_fg_color, "Text is limited to 4096 characters"); }
                apply = ui.add_enabled(draft.value.chars().count() <= 4096, egui::Button::new("Apply text")).clicked();
                ui.small("Uses the drawing's bundled font. Wrapped or unsupported bindings stay in Source.");
            });
        });
        if apply {
            let id = draft.target.id().to_owned();
            self.selected = vec![id];
            self.queue(Action::Text(draft.clone()));
            self.tool = None;
            self.text_draft = Some(draft);
        } else if open {
            self.text_draft = Some(draft);
        } else {
            self.tool = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");

    fn draft(value: &str) -> TextDraft {
        TextDraft {
            target: TextTarget::Existing("capture-label".into()),
            value: value.into(),
        }
    }

    fn editor(draft: &TextDraft) -> DrawingEditor {
        DrawingEditor {
            source: Some(Arc::from(SOURCE)),
            model: Some(Arc::new(Scene::parse(SOURCE).unwrap())),
            selected: vec![draft.target.id().into()],
            text_selection: Some(draft.target.id().into()),
            text_draft: Some(draft.clone()),
            ..Default::default()
        }
    }

    fn complete(editor: &mut DrawingEditor) -> Option<Poll> {
        let context = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        while editor.busy() {
            let result = editor.poll(&context, SOURCE);
            if result.is_some() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "accepted drawing edit did not settle"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn failed_validation_and_measurement_retain_exact_submitted_text() {
        for value in [
            "This label is much too wide for its rectangular container",
            "bad\0text",
        ] {
            let submitted = draft(value);
            let mut editor = editor(&submitted);
            editor.queue(Action::Text(submitted.clone()));
            assert!(matches!(complete(&mut editor), Some(Poll::Failed)));
            assert!(editor.text_draft.as_ref() == Some(&submitted));
            assert!(editor.edit_error.is_some());
            assert!(!editor.busy());
            assert_eq!(editor.source.as_deref(), Some(SOURCE));
            assert_eq!(editor.model.as_ref().unwrap().source(), SOURCE);
        }
    }

    #[test]
    fn timeout_recovers_dismissed_text_but_never_replaces_a_newer_draft() {
        let submitted = draft("Retain this exact replacement\nwith two lines");
        for newer in [None, Some(draft("A newer unsubmitted replacement"))] {
            let mut editor = editor(&submitted);
            editor.queued = Some(PendingEdit {
                change: Action::Text(submitted.clone()),
                started: Instant::now() - Duration::from_secs(4),
            });
            editor.text_draft = newer.clone();
            assert!(matches!(
                editor.poll(&egui::Context::default(), SOURCE),
                Some(Poll::Failed)
            ));
            assert!(editor.text_draft.as_ref() == Some(newer.as_ref().unwrap_or(&submitted)));
            assert!(!editor.busy());
            assert_eq!(editor.model.as_ref().unwrap().source(), SOURCE);
        }
    }

    #[test]
    fn failed_text_does_not_reopen_over_a_newer_selection_or_source_revision() {
        let submitted = draft("Submitted replacement");
        let mut editor = editor(&submitted);
        editor.queued = Some(PendingEdit {
            change: Action::Text(submitted),
            started: Instant::now(),
        });
        editor.text_draft = None;
        editor.text_selection = Some("a-newer-dismissed-draft".into());
        assert!(matches!(
            editor.task_error("validation failed".into()),
            Some(Poll::Failed)
        ));
        assert!(editor.text_draft.is_none());

        let submitted = draft("Must not reach a different source");
        let mut editor = self::editor(&submitted);
        editor.queued = Some(PendingEdit {
            change: Action::Text(submitted),
            started: Instant::now() - Duration::from_secs(4),
        });
        assert!(matches!(
            editor.poll(&egui::Context::default(), "a newer source"),
            Some(Poll::Replaced)
        ));
        assert!(editor.text_draft.is_none());
        assert!(editor.queued.is_none());
        assert!(editor.pending.is_none());
    }

    #[test]
    fn successful_no_op_closes_only_its_matching_draft_without_source_changes() {
        for newer in [None, Some(draft("Keep this newer input"))] {
            let submitted = draft("Capture");
            let mut editor = editor(&submitted);
            editor.queue(Action::Text(submitted));
            if let Some(newer) = &newer {
                editor.text_draft = Some(newer.clone());
            }
            assert!(complete(&mut editor).is_none());
            assert!(editor.text_draft.as_ref() == newer.as_ref());
            assert!(editor.edit_error.is_none());
            assert!(!editor.busy());
            assert_eq!(editor.source.as_deref(), Some(SOURCE));
            assert_eq!(editor.model.as_ref().unwrap().source(), SOURCE);
        }
    }

    #[test]
    fn form_refresh_cannot_replace_a_newer_draft_with_selected_source_text() {
        let mut editor = editor(&draft("Submitted"));
        let newer = TextDraft {
            target: TextTarget::Existing("caption".into()),
            value: "Newer exact input\nfor a different target".into(),
        };
        editor.text_selection = Some("caption".into());
        editor.text_draft = Some(newer.clone());
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| editor.text_controls(ui));
        });
        assert!(editor.text_draft.as_ref() == Some(&newer));
    }

    #[test]
    fn text_requests_measure_inside_the_existing_edit_job_and_keep_the_shared_model_owner() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let model = Scene::parse(source).unwrap();
        let action = Action::Text(TextDraft {
            target: TextTarget::Existing("capture-label".into()),
            value: "Label".into(),
        });
        let change = action.resolve(&model).unwrap();
        let Change::Text { id, content } = change else {
            panic!("expected text edit");
        };
        assert_eq!(id, "capture-label");
        let expected =
            little_clip_render::diagram::measure_drawing_text("Label", 5, 24.0, 1.25).unwrap();
        assert_eq!(content.width.as_f64().unwrap(), expected[0]);
        assert!(model.apply(Change::Text { id, content }).unwrap().is_some());
        assert_eq!(model.source(), source);
    }
}
