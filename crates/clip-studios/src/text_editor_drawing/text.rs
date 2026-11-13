//! Text drafts are measured in the existing accepted edit job, then one source edit.
use super::*;
use little_clip_core::diagram::drawing::{TextContent, TextCreation, TextStyle};

#[derive(Clone)]
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
#[derive(Clone)]
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
                Ok(None) => {
                    if self
                        .text_draft
                        .as_ref()
                        .is_some_and(|draft| matches!(draft.target, TextTarget::Existing(_)))
                    {
                        self.text_draft = None;
                    }
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
            self.text_draft = None;
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
