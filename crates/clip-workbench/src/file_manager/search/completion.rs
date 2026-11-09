//! Local draft help only: no directory reads, model calls or query submission.
use super::query::{self, AnswerType};
use egui::{
    Context, Id, Key, Modifiers, Ui,
    text::{CCursor, CCursorRange},
};
use std::ops::Range;

const SELECTORS: &[(&str, &str)] = &[
    ("image", "Images"),
    ("audio", "Audio files"),
    ("video", "Videos"),
    ("document", "Supported text, office and PDF documents"),
    ("pdf", "PDF documents"),
    ("folder", "Folders"),
    ("file", "Files"),
    ("modified:", "Modification date or date range"),
    (
        "created:",
        "Filesystem creation date; unavailable dates do not match",
    ),
    ("*.md", "Filename glob; replace md with an extension"),
];
const OPERATORS: &[(&str, &str)] = &[
    ("q:\"", "Ask a question and add an answer column"),
    ("emb:\"", "Search indexed content by meaning"),
    ("content:\"", "Find literal text inside supported files"),
    (
        "regex:\"",
        "Match a regular expression from the start of the filename",
    ),
];
const OPTIONS: &[(&str, &str)] = &[
    ("scope:", "Choose the search scope"),
    ("sort:", "Choose result ordering"),
    (
        "ref:selected",
        "Use one selected image with sort:similarity",
    ),
];
const SCOPES: &[(&str, &str)] = &[
    ("here", "Current folder"),
    ("recursive", "Current folder and subfolders"),
    ("indexed", "All explicitly indexed folders"),
];
const SORTS: &[(&str, &str)] = &[
    ("name", "Natural filename order"),
    ("type", "Content family, then extension"),
    ("size", "Logical file size"),
    ("modified", "Modification date"),
    ("created", "Filesystem creation date"),
    ("dhash", "Unsigned image hash order"),
    (
        "similarity",
        "Image distance; requires ref:selected and scope:here",
    ),
];

struct Help {
    message: &'static str,
    replace: Range<usize>,
    choices: Vec<(String, &'static str)>,
}
impl Help {
    fn new(message: &'static str, replace: Range<usize>) -> Self {
        Self {
            message,
            replace,
            choices: Vec::new(),
        }
    }
    fn offer(&mut self, options: &[(&str, &'static str)], prefix: &str) {
        self.choices.extend(
            options
                .iter()
                .filter(|(name, _)| name.starts_with(prefix) && *name != prefix)
                .map(|(name, detail)| ((*name).into(), *detail)),
        );
    }
    fn apply(&self, draft: &mut String, selected: usize) -> usize {
        let mut insert = self.choices[selected].0.as_str();
        if insert.ends_with('"') && draft[self.replace.end..].starts_with('"') {
            insert = &insert[..insert.len() - 1];
        }
        draft.replace_range(self.replace.clone(), insert);
        draft[..self.replace.start + insert.len()].chars().count()
    }
}

fn payload_message(clause: &str) -> &'static str {
    match clause.split_once(':').map(|(name, _)| name) {
        Some("q") => {
            "Question text · write any question; close the quote, then choose an answer format with as."
        }
        Some("regex") => {
            r#"Regular expression · matched from the filename start. Use (?i) for case-insensitive matching; close the quote when finished."#
        }
        Some("content") => {
            "Content text · literal phrase inside supported files, not a regex. Close the quote when finished."
        }
        Some("emb") => {
            "Meaning · describe the content you want to find in explicitly indexed folders. Close the quote when finished."
        }
        _ => "Filename text · punctuation inside quotes is literal. Close the quote when finished.",
    }
}

fn answer_format(clause: &str, caret: usize, replace_end: usize) -> Option<Help> {
    let value = clause.strip_prefix("q:")?.trim_start();
    let (_, tail) = query::quoted_payload(value).ok()?;
    let tail = tail.trim_start();
    let start = caret - tail.len();
    let mut help = Help::new(
        "Answer format · text, number, date or boolean. Enter submits; the format belongs to this question.",
        start..replace_end,
    );
    if let Some(prefix) = tail.strip_prefix("as ") {
        help.replace.start += 3;
        let options: Vec<_> = AnswerType::OPTIONS
            .iter()
            .map(|(name, _)| (*name, "Answer type"))
            .collect();
        help.offer(&options, prefix);
    } else {
        for (name, _) in AnswerType::OPTIONS {
            let insert = format!("as {name}");
            if insert.starts_with(tail) && insert != tail {
                // When there is no separating space yet, insert it as well.
                let separator = if clause.ends_with('"') { " " } else { "" };
                help.choices
                    .push((format!("{separator}{insert}"), "Answer type"));
            }
        }
    }
    Some(help)
}

fn named_value(token: &str, start: usize, end: usize, question: bool) -> Option<Help> {
    let (key, value) = token.split_once(':')?;
    let mut help = Help::new("", start + key.len() + 1..end);
    match key {
        "scope" => {
            help.message = "Search scope · explicit commands never widen automatically. Questions use the current folder only.";
            help.offer(if question { &SCOPES[..1] } else { SCOPES }, value);
        }
        "sort" => {
            if let Some((field, direction)) = value.split_once(':') {
                help.message = "Sort direction · missing values remain last in both directions.";
                help.replace.start += field.len() + 1;
                help.offer(
                    &[
                        ("asc", "Ascending / oldest / closest first"),
                        ("desc", "Descending / newest / farthest first"),
                    ],
                    direction,
                );
            } else {
                help.message = "Sort field · add :asc or :desc to choose direction.";
                // Keep a direction already written to the right of the caret.
                help.offer(SORTS, value);
            }
        }
        "type" if question => {
            help.message = "Answer type · as date after the question is the preferred spelling; type: remains supported.";
            let options: Vec<_> = AnswerType::OPTIONS
                .iter()
                .map(|(name, _)| (*name, "Answer type"))
                .collect();
            help.offer(&options, value);
        }
        "ref" => {
            help.message = "Reference · capture exactly one selected image for sort:similarity.";
            help.offer(&[("selected", "The selected reference image")], value);
        }
        _ => return None,
    }
    Some(help)
}

fn operator_help(clause: &str, caret: usize, end: usize) -> Option<Help> {
    if let Some(format) = answer_format(clause, caret, end) {
        return Some(format);
    }
    let (name, value) = clause.split_once(':')?;
    if !["q", "regex", "content", "emb"].contains(&name) {
        return None;
    }
    let mut help = Help::new(payload_message(clause), caret..end);
    if value.is_empty() {
        help.offer(&[("\"", "Begin quoted text")], "");
    } else if query::quoted_payload(value).is_ok() {
        help.message =
            "Payload complete · Enter applies the command; add ; for scope or sorting options.";
    }
    Some(help)
}

fn at(draft: &str, cursor: usize, literal: bool) -> Help {
    let caret = draft
        .char_indices()
        .nth(cursor)
        .map_or(draft.len(), |(byte, _)| byte);
    let prefix = &draft[..caret];
    let end = caret
        + draft[caret..]
            .find(|c: char| c.is_whitespace() || ",;\"".contains(c))
            .unwrap_or(draft.len() - caret);
    let mut help = Help::new(
        "Filename text · type a name, or start with : for command suggestions. Enter applies the draft.",
        caret..end,
    );
    if literal || !prefix.starts_with(':') || prefix.starts_with("::") || draft.len() > 8192 {
        return help;
    }
    let (clauses, quoted) = query::fragments(&prefix[1..], ';');
    let clause = clauses.last().unwrap().trim_start();
    if quoted {
        help.message = payload_message(clause);
        return help;
    }
    if let Some(operator) = operator_help(clause, caret, end) {
        return operator;
    }
    let question = clauses
        .iter()
        .any(|clause| clause.trim_start().starts_with("q:"));
    let start = caret - clause.len();
    if let Some(value) = named_value(clause, start, end, question) {
        let mut value = value;
        if clause.starts_with("sort:") && !clause[5..].contains(':') {
            value.replace.end = caret + draft[caret..end].find(':').unwrap_or(end - caret);
        }
        return value;
    }
    if clauses.len() > 1 {
        help.message = "Command options · Tab inserts the highlighted suggestion; ↑/↓ choose. Enter applies the command.";
        help.replace = start..end;
        help.offer(OPTIONS, clause);
        if !clauses.iter().any(|part| {
            OPERATORS
                .iter()
                .any(|(op, _)| part.trim_start().starts_with(op.trim_end_matches('"')))
        }) {
            help.offer(OPERATORS, clause);
        }
        return help;
    }
    selector_help(clause, caret, end)
}

fn selector_help(clause: &str, caret: usize, end: usize) -> Help {
    let (parts, _) = query::fragments(clause, ',');
    let token = parts.last().unwrap().trim_start().trim_start_matches('!');
    let mut help = Help::new(
        "Filters · comma combines filters; ! excludes. Sizes accept 10KiB..2MiB. Use ; before an operator or option.",
        caret - token.len()..end,
    );
    if token.starts_with("modified:") || token.starts_with("created:") {
        help.message = "Date · YYYY-MM-DD or an offset timestamp; use >=, <= or a range such as 2026-01-01..2026-03-31. A spaced hyphen also works.";
    } else if token.starts_with(|c: char| c.is_ascii_digit() || "<>=-".contains(c)) {
        help.message = "Size · B, KB/MB/GB (decimal), KiB/MiB/GiB (binary); <, <=, =, >=, > or inclusive ranges with .., ... or -. Example: 10KiB..2MiB.";
    } else if token.contains(char::is_whitespace) || token.starts_with('"') {
        help.message = "Filename text · a literal name after the filters; exact and prefix matches rank first. Add ; to choose another clause.";
    } else {
        help.offer(SELECTORS, token);
        if parts.len() == 1 && !clause.starts_with('!') {
            help.offer(OPERATORS, token);
        }
    }
    help
}

#[derive(Default)]
pub(super) struct State {
    source: Option<(String, usize)>,
    selected: usize,
    scroll_selected: bool,
}
impl State {
    fn help(&mut self, draft: &str, cursor: Option<CCursorRange>, literal: bool) -> Option<Help> {
        let cursor = cursor
            .filter(|range| range.primary.index == range.secondary.index)?
            .primary
            .index;
        if self
            .source
            .as_ref()
            .is_none_or(|(source, caret)| source != draft || *caret != cursor)
        {
            self.selected = 0;
            self.scroll_selected = true;
            self.source = Some((draft.into(), cursor));
        }
        Some(at(draft, cursor, literal))
    }
    /// Consume completion keys before TextEdit handles focus traversal/cursor movement.
    pub fn keys(&mut self, ctx: &Context, id: Id, draft: &mut String, literal: bool) {
        if !ctx.memory(|memory| memory.has_focus(id)) {
            return;
        }
        let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
            return;
        };
        let Some(help) = self.help(draft, state.cursor.char_range(), literal) else {
            return;
        };
        if help.choices.is_empty() {
            return;
        }
        ctx.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                self.selected = (self.selected + 1) % help.choices.len();
                self.scroll_selected = true;
            }
            if input.consume_key(Modifiers::NONE, Key::ArrowUp)
                || input.consume_key(Modifiers::SHIFT, Key::Tab)
            {
                self.selected = (self.selected + help.choices.len() - 1) % help.choices.len();
                self.scroll_selected = true;
            }
            if input.consume_key(Modifiers::NONE, Key::Tab) {
                let cursor = help.apply(draft, self.selected);
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(cursor))));
            }
        });
        state.store(ctx, id);
    }
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        draft: &mut String,
        literal: bool,
        output: &egui::text_edit::TextEditOutput,
    ) {
        if !output.response.has_focus() && !output.response.lost_focus() {
            return;
        }
        let Some(help) = self.help(draft, output.state.cursor.char_range(), literal) else {
            return;
        };
        ui.small(help.message);
        if help.choices.is_empty() {
            return;
        }
        // egui decides Tab focus traversal before the next widget pass. Keep
        // it with this field only while there are actual completions to accept.
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                output.response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
        ui.small("↑/↓ choose · Tab completes · Enter applies");
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_source("search_command_suggestions")
            .max_height(106.0)
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                for (index, (insert, detail)) in help.choices.iter().enumerate() {
                    let response = ui.selectable_label(
                        self.selected == index,
                        format!("{}  ·  {detail}", insert.trim()),
                    );
                    // Keep the draft focused across pointer-down/up; otherwise
                    // the disappearing suggestions cannot receive the release.
                    if response.is_pointer_button_down_on() {
                        output.response.request_focus();
                    }
                    if self.scroll_selected && self.selected == index {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    if response.clicked() {
                        chosen = Some(index);
                    }
                }
            });
        self.scroll_selected = false;
        if let Some(index) = chosen {
            let cursor = help.apply(draft, index);
            let mut state = output.state.clone();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(cursor))));
            state.store(ui.ctx(), output.response.id);
            output.response.request_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(text: &str) -> Help {
        at(text, text.chars().count(), false)
    }
    fn values(help: &Help) -> Vec<&str> {
        help.choices
            .iter()
            .map(|(value, _)| value.as_str())
            .collect()
    }

    #[test]
    fn values_follow_the_current_clause_and_question_scope() {
        assert_eq!(values(&help(":im")), ["image"]);
        assert_eq!(values(&help(":image,!au")), ["audio"]);
        assert_eq!(
            values(&help(":image;scope:")),
            ["here", "recursive", "indexed"]
        );
        assert_eq!(values(&help(r#":q:"Question?";scope:"#)), ["here"]);
        assert_eq!(values(&help(":image;sort:size:d")), ["desc"]);
        assert_eq!(values(&help(":image;ref:s")), ["selected"]);
        assert_eq!(
            values(&help(r#":q:"Question?" as "#)),
            ["text", "number", "date", "boolean"]
        );
        assert_eq!(values(&help(r#":q:"Question?" as d"#)), ["date"]);
        assert_eq!(values(&help(r#":q:"Question?";type:b"#)), ["boolean"]);
        assert!(!values(&help(":image,")).contains(&"q:\""));
        assert!(!values(&help(r#":q:"Question?" as date;"#)).contains(&"emb:\""));
    }

    #[test]
    fn free_text_has_context_without_rewriting_quoted_punctuation() {
        for (draft, message) in [
            (
                r#":*.md;q:"Date: yyyy-mm-dd; as date, scope:here \"quoted\""#,
                "Question text",
            ),
            (r#":regex:"(?i)foo;bar,\d+"#, "Regular expression"),
            (r#":content:"literal; text"#, "Content text"),
            (r#":emb:"a red bicycle"#, "Meaning"),
            (":modified:2026-01-", "Date"),
            (":>=10Mi", "Size"),
            (":image vacation", "Filename text"),
        ] {
            let help = help(draft);
            assert!(
                help.message.starts_with(message),
                "{draft}: {}",
                help.message
            );
            assert!(help.choices.is_empty(), "{draft}");
        }
        assert!(at(":q:", 3, true).choices.is_empty());
        assert!(help("::q:").choices.is_empty());
    }

    #[test]
    fn completion_preserves_unicode_and_later_clauses_at_the_caret() {
        for (before, caret, expected) in [
            (
                ":image;scope:re|cursive;sort:name",
                "recursive",
                ":image;scope:recursive;sort:name",
            ),
            (
                r#":*.md;q:"日付; café?" as d|ate;sort:name"#,
                "date",
                r#":*.md;q:"日付; café?" as date;sort:name"#,
            ),
            (
                ":image;sort:size:d|esc;scope:here",
                "desc",
                ":image;sort:size:desc;scope:here",
            ),
            (":image;sort:siz|e:desc", "size", ":image;sort:size:desc"),
            (
                r#":q|:"Question?" as date"#,
                "q:\"",
                r#":q:"Question?" as date"#,
            ),
        ] {
            let cursor = before[..before.find('|').unwrap()].chars().count();
            let mut draft = before.replace('|', "");
            let help = at(&draft, cursor, false);
            let selected = help
                .choices
                .iter()
                .position(|(value, _)| value == caret)
                .unwrap();
            let result_cursor = help.apply(&mut draft, selected);
            assert_eq!(draft, expected);
            assert!(result_cursor <= draft.chars().count());
            query::Query::parse(&draft).unwrap();
        }
    }

    #[test]
    fn closed_question_completes_the_format_without_a_second_separator() {
        for draft in [r#":q:"Date?""#, r#":q:"Date?" "#, r#":q:"Date?" a"#] {
            let mut draft = draft.to_string();
            let help = help(&draft);
            let index = help
                .choices
                .iter()
                .position(|(value, _)| value.trim() == "as date")
                .unwrap();
            help.apply(&mut draft, index);
            assert_eq!(
                query::Query::parse(&draft).unwrap().answer_type,
                AnswerType::Date
            );
        }
    }

    #[test]
    fn tab_keeps_text_focus_and_arrow_selection_does_not_submit() {
        let ctx = Context::default();
        let id = Id::new("completion_test");
        let mut state = State::default();
        let mut draft = r#":*.md;q:"Date?" as "#.to_string();
        let mut frame = |events: Vec<egui::Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.keys(ctx, id, &mut draft, false);
                        let output = egui::TextEdit::singleline(&mut draft).id(id).show(ui);
                        state.ui(ui, &mut draft, false, &output);
                        let _ = ui.button("Another control");
                    });
                },
            );
        };
        frame(Vec::new());
        let mut edit = egui::TextEdit::load_state(&ctx, id).unwrap();
        // Cursor indices are Unicode scalar counts. Use the actual field end.
        edit.cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(
                r#":*.md;q:"Date?" as "#.chars().count(),
            ))));
        edit.store(&ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        // Establish focus through egui's normal frame boundary before keys.
        frame(Vec::new());
        frame(Vec::new());
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        frame(vec![key(Key::ArrowDown)]);
        frame(vec![key(Key::ArrowDown)]);
        frame(vec![key(Key::Tab)]);
        assert!(ctx.memory(|memory| memory.has_focus(id)));
        // Exact completed values no longer trap Tab in the search field.
        frame(Vec::new());
        frame(Vec::new());
        frame(vec![key(Key::Tab)]);
        assert!(draft.ends_with("as date"), "{draft}");
        assert!(!ctx.memory(|memory| memory.has_focus(id)));
        assert_eq!(
            query::Query::parse(&draft).unwrap().answer_type,
            AnswerType::Date
        );
    }
}
