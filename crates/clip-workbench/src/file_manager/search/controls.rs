//! Draft-only search controls; generated commands pass through the ordinary parser.
use super::query::{AnswerType, Operator, Query, Scope, SortField};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Name,
    Regex,
    Content,
    Semantic,
    Question,
}
#[derive(Default)]
pub(super) struct Controls {
    pub visible: bool,
    source: Option<(String, bool)>,
    mode: Mode,
    filters: String,
    payload: String,
    scope: Scope,
    sort: String,
    answer_type: AnswerType,
    error: Option<String>,
}
impl Controls {
    fn load(&mut self, draft: &str, literal: bool) -> Result<(), String> {
        let query = if literal {
            Query::literal(draft)
        } else {
            Query::parse(draft)
        }?;
        let (mode, payload) = match &query.operator {
            Operator::Name => (Mode::Name, query.name.clone()),
            Operator::Regex(regex) => (
                Mode::Regex,
                regex
                    .as_str()
                    .strip_prefix(r"\A(?:")
                    .and_then(|text| text.strip_suffix(')'))
                    .unwrap_or(regex.as_str())
                    .into(),
            ),
            Operator::Content(text) => (Mode::Content, text.clone()),
            Operator::Embedding(text) => (Mode::Semantic, text.clone()),
            Operator::Answer(text) => (Mode::Question, text.clone()),
        };
        *self = Self {
            visible: self.visible,
            source: Some((draft.into(), literal)),
            mode,
            payload,
            filters: query.selector_text(),
            scope: query.scope,
            sort: query
                .sort
                .map(|sort| {
                    format!(
                        "{}:{}",
                        field_name(sort.field),
                        if sort.descending { "desc" } else { "asc" }
                    )
                })
                .unwrap_or_default(),
            answer_type: query.answer_type,
            error: None,
        };
        Ok(())
    }
    fn head(&self) -> String {
        let filters = self.filters.trim();
        let quoted = format!(
            "\"{}\"",
            self.payload.replace('\\', "\\\\").replace('"', "\\\"")
        );
        if self.mode == Mode::Name {
            format!(
                "{} {}",
                if filters.is_empty() { "*" } else { filters },
                quoted
            )
        } else {
            let operator = match self.mode {
                Mode::Regex => "regex",
                Mode::Content => "content",
                Mode::Semantic => "emb",
                Mode::Question => "q",
                Mode::Name => unreachable!(),
            };
            format!(
                "{}{operator}:{quoted}",
                if filters.is_empty() {
                    String::new()
                } else {
                    format!("{filters};")
                }
            )
        }
    }
    fn command(&self) -> Result<String, String> {
        let head = self.head();
        let mut command = format!(
            ":{head};scope:{}",
            match self.scope {
                Scope::Here => "here",
                Scope::Recursive => "recursive",
                Scope::Indexed => "indexed",
            }
        );
        if !self.sort.is_empty() {
            command.push_str(&format!(";sort:{}", self.sort));
            if self.sort.starts_with("similarity:") {
                command.push_str(";ref:selected");
            }
        }
        if self.mode == Mode::Question {
            command.push_str(&format!(
                ";type:{}",
                match self.answer_type {
                    AnswerType::Text => "text",
                    AnswerType::Number => "number",
                    AnswerType::Date => "date",
                    AnswerType::Boolean => "boolean",
                }
            ));
        }
        Query::parse(&command)?;
        Ok(command)
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, draft: &str, literal: bool) -> Option<String> {
        ui.set_max_width(440.0);
        let current = self
            .source
            .as_ref()
            .is_some_and(|(source, mode)| source == draft && *mode == literal);
        if !current && let Err(error) = self.load(draft, literal) {
            ui.label("Finish the current command before opening its controls.");
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Start a new draft").clicked() {
                return Some(String::new());
            }
            return None;
        }
        ui.label("Build a search command");
        egui::ComboBox::from_id_source("search_control_mode")
            .selected_text(mode_name(self.mode))
            .show_ui(ui, |ui| {
                for mode in [
                    Mode::Name,
                    Mode::Regex,
                    Mode::Content,
                    Mode::Semantic,
                    Mode::Question,
                ] {
                    ui.selectable_value(&mut self.mode, mode, mode_name(mode));
                }
            });
        ui.label("Search text or question");
        ui.add(
            egui::TextEdit::singleline(&mut self.payload)
                .char_limit(8192)
                .desired_width(420.0),
        );
        ui.label("Filters (comma-separated)");
        ui.add(
            egui::TextEdit::singleline(&mut self.filters)
                .char_limit(8192)
                .desired_width(420.0)
                .hint_text("image,!*.svg,10KiB..2MiB"),
        );
        self.selector_menu(ui);
        ui.horizontal(|ui| {
            ui.label("Scope");
            for (scope, name) in [
                (Scope::Here, "Folder"),
                (Scope::Recursive, "Subfolders"),
                (Scope::Indexed, "Indexed folders"),
            ] {
                if ui
                    .add(egui::Button::new(name).selected(self.scope == scope))
                    .clicked()
                {
                    self.scope = scope;
                }
            }
        });
        self.sort_ui(ui);
        if self.mode == Mode::Question {
            ui.label("Answer type (current folder only)");
            egui::ComboBox::from_id_source("search_control_answer_type")
                .selected_text(format!("{:?}", self.answer_type))
                .show_ui(ui, |ui| {
                    for (kind, label) in [
                        (AnswerType::Text, "Text"),
                        (AnswerType::Number, "Number"),
                        (AnswerType::Date, "Date"),
                        (AnswerType::Boolean, "Boolean"),
                    ] {
                        ui.selectable_value(&mut self.answer_type, kind, label);
                    }
                });
        }
        ui.small("Options edit the command above. Press Search to run it.");
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if ui.button("Apply to draft").clicked() {
            match self.command() {
                Ok(command) => return Some(command),
                Err(error) => self.error = Some(error),
            }
        }
        None
    }
    fn selector_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Insert filter…", |ui| {
            for selector in [
                "image",
                "audio",
                "video",
                "document",
                "pdf",
                "folder",
                "file",
                "*.jpg",
                "!*.svg",
                "10KiB..2MiB",
            ] {
                if ui.button(selector).clicked() {
                    self.append_filter(selector);
                    ui.close_menu();
                }
            }
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            for key in ["modified", "created"] {
                if ui.button(format!("{key} since today")).clicked() {
                    self.append_filter(&format!("{key}:>={today}"));
                    ui.close_menu();
                }
            }
        });
    }
    fn append_filter(&mut self, selector: &str) {
        if !self.filters.trim().is_empty() {
            self.filters.push(',');
        }
        self.filters.push_str(selector);
    }
    fn sort_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Sort");
            egui::ComboBox::from_id_source("search_control_sort")
                .selected_text(if self.sort.is_empty() {
                    "Relevance / existing order"
                } else {
                    &self.sort
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.sort,
                        String::new(),
                        "Relevance / existing order",
                    );
                    for field in [
                        "name",
                        "type",
                        "size",
                        "modified",
                        "created",
                        "dhash",
                        "similarity",
                    ] {
                        for direction in ["asc", "desc"] {
                            let value = format!("{field}:{direction}");
                            ui.selectable_value(&mut self.sort, value.clone(), value);
                        }
                    }
                });
        });
        if self.sort.starts_with("similarity:") {
            ui.small("Uses the selected reference image; current folder only.");
        }
    }
}
fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Name => "Filename",
        Mode::Regex => "Regex",
        Mode::Content => "Text content",
        Mode::Semantic => "Semantic",
        Mode::Question => "Answer column",
    }
}
fn field_name(field: SortField) -> &'static str {
    match field {
        SortField::Name => "name",
        SortField::Type => "type",
        SortField::Size => "size",
        SortField::Modified => "modified",
        SortField::Created => "created",
        SortField::Dhash => "dhash",
        SortField::Similarity => "similarity",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_preserve_command_constraints_quoted_payloads_and_options() {
        for command in [
            r#":10.5KB-2MB,*.jpg foo;scope:recursive;sort:size:desc"#,
            r#":image,!*.svg;emb:"a blue image; with, punctuation";scope:indexed"#,
            r#":modified:2026-01-01 - 2026-03-31,created:>=2025-01-01 foo"#,
            r#":regex:"(?i)foo.*\.jpg$""#,
            r#":*.pdf;q:"Ending balance?";type:number"#,
            r#":image;sort:similarity;ref:selected"#,
        ] {
            let original = Query::parse(command).unwrap();
            let mut controls = Controls::default();
            controls.load(command, false).unwrap();
            let generated = controls.command().unwrap();
            let parsed = Query::parse(&generated).unwrap();
            assert_eq!(
                parsed.selector_text(),
                original.selector_text(),
                "{generated}"
            );
            assert_eq!(parsed.scope, original.scope);
            assert_eq!(parsed.sort, original.sort);
            assert_eq!(parsed.name, original.name);
            assert_eq!(parsed.answer_type, original.answer_type);
            assert_eq!(
                format!("{:?}", parsed.operator),
                format!("{:?}", original.operator)
            );
        }
    }
    #[test]
    fn literal_names_with_colons_and_quotes_are_never_operators() {
        let text = ":emb:\"literal; \\filename\"";
        let mut controls = Controls::default();
        controls.load(text, true).unwrap();
        let query = Query::parse(&controls.command().unwrap()).unwrap();
        assert!(matches!(query.operator, Operator::Name));
        assert_eq!(query.name, text);
        assert_eq!(Query::literal("  spaces  ").unwrap().name, "  spaces  ");
    }
    #[test]
    fn invalid_control_combinations_and_incomplete_filters_are_rejected() {
        let mut controls = Controls::default();
        controls
            .load(":q:\"Question?\";type:boolean", false)
            .unwrap();
        controls.scope = Scope::Indexed;
        assert!(controls.command().is_err());
        controls.scope = Scope::Here;
        controls.filters = "10KB..".into();
        assert!(controls.command().is_err());
    }
}
