use super::Input;
use egui::{
    TextBuffer, TextEdit, Ui,
    text::{CCursor, CCursorRange},
};
use little_clip_core::engineering::analysis::{Binding, SignalGraph, SignalType};

const MAX_DRAFT_CHARS: usize = 12_000;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Equation {
    pub id: u64,
    pub text: String,
    pub name: String,
    pub kind: SignalType,
}

#[derive(Default)]
pub(super) struct Book {
    pub rows: Vec<Equation>,
    edit: Option<Draft>,
    failure: Option<(u64, String)>,
    next_id: u64,
}

struct Draft {
    target: Option<u64>,
    text: String,
    original: String,
    focus: bool,
    error: Option<String>,
}

pub(super) enum Action {
    Apply,
    Remove(u64),
}

pub(super) struct Change {
    pub program: String,
    pub requires_history: bool,
    pub lanes: Vec<Option<usize>>,
    pub digital: Vec<Option<usize>>,
    pub reveal: Option<u64>,
}

impl Book {
    pub fn restore(&mut self, rows: Vec<Equation>) -> Result<String, String> {
        let mut ids = std::collections::BTreeSet::new();
        if rows.len() > 32
            || rows.iter().any(|row| {
                !ids.insert(row.id)
                    || row.id > u64::MAX / 2
                    || row.name.chars().count() > MAX_DRAFT_CHARS
                    || row.text.chars().count() > MAX_DRAFT_CHARS
                    || row.text.lines().count() != 1
                    || row.text.trim().is_empty()
            })
        {
            return Err("Invalid saved equation rows".into());
        }
        let program = rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.next_id = ids.last().map_or(0, |id| id + 1);
        self.rows = rows;
        Ok(program)
    }
    pub fn open(&self) -> bool {
        self.edit.is_some() || !self.rows.is_empty()
    }

    pub fn add(&mut self, expression: &str) {
        if self.edit.is_some() {
            return;
        }
        let name = (1..)
            .map(|index| format!("M{index}"))
            .find(|name| {
                !self
                    .rows
                    .iter()
                    .any(|row| row.name.eq_ignore_ascii_case(name))
            })
            .expect("bounded equation list");
        let mut draft = Draft::new(None, format!("{name} = "));
        draft.text.push_str(expression);
        self.edit = Some(draft);
    }

    pub fn editing(&self) -> bool {
        self.edit.is_some()
    }

    pub fn has_draft_edits(&self) -> bool {
        self.edit
            .as_ref()
            .is_some_and(|draft| draft.text.trim() != draft.original.trim())
    }

    pub fn edit(&mut self, index: usize) {
        if self.edit.is_none() {
            let row = &self.rows[index];
            self.edit = Some(Draft::new(Some(row.id), row.text.clone()));
        }
    }

    pub fn row_error(&self, id: u64) -> Option<&str> {
        let (row, error) = self.failure.as_ref()?;
        (*row == id).then_some(error)
    }

    pub fn editor(&mut self, ui: &mut Ui, target: Option<u64>, inputs: &[Input]) -> Option<Action> {
        let draft = self.edit.as_mut().filter(|draft| draft.target == target)?;
        let id = ui.make_persistent_id(("equation-draft", target));
        let response = draft.field(ui, id);
        let (apply, cancel) = ui
            .horizontal(|ui| {
                let apply = ui.button("Apply equation").clicked();
                let cancel = ui.button("Cancel edit").clicked();
                (apply, cancel)
            })
            .inner;
        let escape = (response.has_focus() || response.lost_focus())
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cancel || escape {
            self.edit = None;
            return None;
        }
        let apply_shortcut = response.has_focus()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        if apply
            || apply_shortcut
            || (target.is_some()
                && response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            return Some(Action::Apply);
        }
        draft.inputs(ui, id, inputs, &self.rows);
        None
    }

    pub fn apply(
        &mut self,
        action: Action,
        bindings: &[Binding<'_>],
        aliases: &std::collections::HashMap<String, &str>,
    ) -> Option<Change> {
        let reveal = self
            .edit
            .as_ref()
            .map(|draft| draft.target.unwrap_or(self.next_id));
        let (replacement, new_id) = self.replacement(&action)?;
        let program = replacement
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let renamed = self.edit.as_ref().and_then(|draft| {
            self.rows
                .iter()
                .enumerate()
                .find(|(_, row)| Some(row.id) == draft.target)
        });
        let program = if let Some((index, row)) = renamed {
            SignalGraph::rename_lane_uses(&program, index, &row.name)
        } else {
            Ok(program)
        };
        let graph = program.and_then(|program| {
            let program = if aliases.is_empty() {
                program
            } else {
                SignalGraph::resolve_input_names(&program, aliases)?
            };
            SignalGraph::compile(&program, bindings, 0).map(|graph| (program, graph))
        });
        let (program, graph) = match graph {
            Ok(result) => result,
            Err(error) => {
                match action {
                    Action::Apply => self.edit.as_mut()?.error = Some(error.to_string()),
                    Action::Remove(id) => self.failure = Some((id, error.to_string())),
                }
                return None;
            }
        };
        let rows = replacement
            .into_iter()
            .map(|(id, _)| id)
            .zip(program.lines())
            .zip(graph.lanes())
            .map(|((id, text), lane)| Equation {
                id,
                text: text.to_owned(),
                name: lane.name.clone(),
                kind: lane.signal_type,
            })
            .collect::<Vec<_>>();
        let lanes = remap(self.rows.iter(), rows.iter());
        let digital = remap(
            self.rows
                .iter()
                .filter(|row| row.kind == SignalType::Digital),
            rows.iter().filter(|row| row.kind == SignalType::Digital),
        );
        self.rows = rows;
        self.next_id = new_id;
        self.edit = None;
        self.failure = None;
        Some(Change {
            program,
            requires_history: graph.requires_history(),
            lanes,
            digital,
            reveal,
        })
    }

    fn replacement(&mut self, action: &Action) -> Option<(Vec<(u64, String)>, u64)> {
        let mut items = self
            .rows
            .iter()
            .map(|row| (row.id, row.text.clone()))
            .collect::<Vec<_>>();
        let mut next_id = self.next_id;
        match *action {
            Action::Apply => {
                let draft = self.edit.as_mut()?;
                let lines = draft
                    .text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty());
                if let Some(id) = draft.target {
                    let row = items.iter_mut().find(|(key, _)| *key == id)?;
                    let mut lines = lines;
                    let Some(line) = lines.next().filter(|_| lines.next().is_none()) else {
                        draft.error = Some("Enter one equation.".into());
                        return None;
                    };
                    row.1 = line.to_owned();
                } else {
                    for line in lines {
                        items.push((next_id, line.to_owned()));
                        next_id += 1;
                    }
                    if next_id == self.next_id {
                        draft.error = Some("Enter an equation.".into());
                        return None;
                    }
                }
            }
            Action::Remove(id) => items.retain(|(key, _)| *key != id),
        }
        Some((items, next_id))
    }
}

impl Draft {
    fn new(target: Option<u64>, text: String) -> Self {
        Self {
            target,
            original: text.clone(),
            text,
            focus: true,
            error: None,
        }
    }

    fn field(&mut self, ui: &mut Ui, id: egui::Id) -> egui::Response {
        let focus = std::mem::take(&mut self.focus);
        let mut edit = if self.target.is_some() {
            TextEdit::singleline(&mut self.text)
        } else {
            TextEdit::multiline(&mut self.text).desired_rows(1)
        };
        edit = edit
            .id(id)
            .font(egui::TextStyle::Monospace)
            .desired_width(ui.available_width())
            .char_limit(MAX_DRAFT_CHARS)
            .hint_text("name = expression");
        let response = ui.add(edit);
        if focus {
            response.request_focus();
            response.scroll_to_me(Some(egui::Align::Center));
            let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(
                    self.text.chars().count(),
                ))));
            state.store(ui.ctx(), id);
        }
        if response.changed() {
            self.error = None;
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        response
    }

    fn inputs(&mut self, ui: &mut Ui, id: egui::Id, inputs: &[Input], rows: &[Equation]) {
        let target = self.target;
        ui.weak("Insert input");
        egui::ScrollArea::vertical()
            .id_salt("equation-inputs")
            .max_height(86.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for input in inputs {
                        if ui
                            .add(
                                egui::Button::new(format!("{} · {}", input.alias, input.label))
                                    .small(),
                            )
                            .clicked()
                        {
                            self.insert(ui, id, &input.alias);
                        }
                    }
                    for row in rows.iter().take_while(|row| Some(row.id) != target) {
                        if ui.small_button(&row.name).clicked() {
                            self.insert(ui, id, &row.name);
                        }
                    }
                });
            });
    }

    fn insert(&mut self, ui: &Ui, id: egui::Id, alias: &str) {
        let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let length = self.text.chars().count();
        let selection = state
            .cursor
            .char_range()
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(length)));
        let range = selection.as_sorted_char_range();
        if length - (range.end.0.min(length) - range.start.0.min(length)) + alias.chars().count()
            > MAX_DRAFT_CHARS
        {
            return;
        }
        let mut cursor = self.text.delete_selected(&selection);
        self.text
            .insert_text_at(&mut cursor, alias, MAX_DRAFT_CHARS);
        state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
        state.store(ui.ctx(), id);
        ui.memory_mut(|memory| memory.request_focus(id));
        self.error = None;
    }
}

fn remap<'a>(
    old: impl Iterator<Item = &'a Equation>,
    new: impl Iterator<Item = &'a Equation> + Clone,
) -> Vec<Option<usize>> {
    old.map(|old| {
        new.clone()
            .position(|row| row.id == old.id && row.kind == old.kind)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUTS: &[Binding<'_>] = &[Binding {
        name: "input",
        signal_type: SignalType::Analog(
            little_clip_core::engineering::analysis::Dimension::VOLTAGE,
        ),
    }];

    #[test]
    fn resolves_channel_names_to_stable_recording_inputs() {
        let signal_type = INPUTS[0].signal_type;
        let bindings = [
            Binding {
                name: "s1_c1",
                signal_type,
            },
            Binding {
                name: "s1_c2",
                signal_type,
            },
        ];
        let aliases = [("ch1".into(), "s1_c1"), ("ch2".into(), "s1_c2")].into();
        let mut book = Book::default();
        book.add("");
        book.edit.as_mut().unwrap().text = "M1 = CH1 - ch2".into();
        let change = book.apply(Action::Apply, &bindings, &aliases).unwrap();
        assert_eq!(change.program, "M1 = s1_c1 - s1_c2");
    }

    #[test]
    fn adding_and_removing_lanes_keeps_surviving_channel_identities() {
        let mut book = Book::default();
        book.add("");
        book.edit.as_mut().unwrap().text = "A = 1V\nB = 1V > 2V\nC = 2V".into();
        book.apply(Action::Apply, INPUTS, &Default::default())
            .unwrap();
        let ids: Vec<_> = book.rows.iter().map(|row| row.id).collect();
        let change = book
            .apply(Action::Remove(ids[0]), INPUTS, &Default::default())
            .unwrap();
        assert_eq!(change.lanes, [None, Some(0), Some(1)]);
        assert_eq!(change.digital, [Some(0)]);
        assert_eq!(
            book.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            ids[1..]
        );
    }

    #[test]
    fn rejected_edit_keeps_the_last_applied_graph_and_draft() {
        let mut book = Book::default();
        book.add("");
        book.edit.as_mut().unwrap().text = "A = 1V\nB = A * 2".into();
        book.apply(Action::Apply, INPUTS, &Default::default())
            .unwrap();
        book.edit(0);
        assert!(!book.has_draft_edits());
        book.edit.as_mut().unwrap().text = "A = missing".into();
        assert!(
            book.apply(Action::Apply, INPUTS, &Default::default())
                .is_none()
        );
        assert_eq!(book.rows[0].text, "A = 1V");
        assert!(book.has_draft_edits());
        let draft = book.edit.as_ref().unwrap();
        assert_eq!(draft.text, "A = missing");
        assert!(draft.error.as_ref().unwrap().contains("missing"));
    }

    #[test]
    fn renaming_an_equation_rewrites_bindings_without_changing_its_identity() {
        let mut book = Book::default();
        book.add("");
        book.edit.as_mut().unwrap().text = "signal = input\nscaled = signal * 2".into();
        book.apply(Action::Apply, INPUTS, &Default::default())
            .unwrap();
        let identity = book.rows[0].id;
        book.edit(0);
        book.edit.as_mut().unwrap().text = "reference = input".into();
        let change = book
            .apply(Action::Apply, INPUTS, &Default::default())
            .unwrap();
        assert_eq!(change.lanes, [Some(0), Some(1)]);
        assert_eq!(book.rows[0].id, identity);
        assert_eq!(book.rows[1].text, "scaled = reference * 2");
    }
}
