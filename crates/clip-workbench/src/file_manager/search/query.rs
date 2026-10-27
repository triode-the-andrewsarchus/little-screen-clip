//! One parser for typed browser commands and search controls. Never shell syntax.
use super::range::{self, Interval};
use crate::file_manager::{filesystem::Entry, sorting};
use regex::{Regex, RegexBuilder};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scope {
    #[default]
    Here,
    Recursive,
    Indexed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnswerType {
    #[default]
    Text,
    Number,
    Date,
    Boolean,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SortField {
    Name,
    Type,
    Size,
    Modified,
    Created,
    Dhash,
    Similarity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sort {
    pub field: SortField,
    pub descending: bool,
}
#[derive(Clone, Debug, Default)]
pub(crate) enum Operator {
    #[default]
    Name,
    Regex(Regex),
    Content(String),
    Embedding(String),
    Answer(String),
}
#[derive(Clone, Debug)]
enum Constraint {
    Size(Interval),
    Modified(Interval),
    Created(Interval),
    Glob(Regex),
    Family(Family),
}
#[derive(Clone, Copy, Debug)]
enum Family {
    Image,
    Audio,
    Video,
    Document,
    Pdf,
    Folder,
    File,
}
#[derive(Clone, Debug)]
struct Selector {
    source: String,
    constraint: Constraint,
    negated: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Query {
    selectors: Vec<Selector>,
    pub name: String,
    pub operator: Operator,
    pub scope: Scope,
    pub sort: Option<Sort>,
    pub answer_type: AnswerType,
    pub selected_reference: bool,
    /// Internal navigation mode; never enabled by an explicit command.
    pub progressive: bool,
}
impl Query {
    pub fn literal(input: &str) -> Result<Self, String> {
        if input.len() > 8192 {
            return Err("Search commands are limited to 8 KiB".into());
        }
        Ok(Self {
            name: input.into(),
            ..Self::default()
        })
    }
    pub(super) fn selector_text(&self) -> String {
        self.selectors
            .iter()
            .map(|selector| selector.source.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
    pub fn navigation(name: String) -> Self {
        Self {
            name,
            progressive: true,
            ..Self::default()
        }
    }

    pub fn resolve_navigation_scope(&mut self, directory: &super::Directory, hidden: bool) {
        if self.progressive
            && !self.name.is_empty()
            && !directory
                .entries
                .iter()
                .any(|entry| (hidden || !entry.hidden) && self.local_rank(entry).is_some())
        {
            self.scope = Scope::Recursive;
        }
    }

    pub fn parse(input: &str) -> Result<Self, String> {
        if input.len() > 8192 {
            return Err("Search commands are limited to 8 KiB".into());
        }
        let input = input.trim();
        let mut query = Self::default();
        if let Some(literal) = input.strip_prefix("::") {
            query.name = format!(":{literal}");
            return Ok(query);
        }
        let Some(command) = input.strip_prefix(':') else {
            query.name = input.to_owned();
            return Ok(query);
        };
        if command.is_empty() {
            return Err("Enter a selector, filename, or search operator after :".into());
        }
        query.command(command)?;
        Ok(query)
    }

    fn command(&mut self, command: &str) -> Result<(), String> {
        let parts = split(command, ';')?;
        let mut have_operator = false;
        if is_operator(parts[0]) {
            self.operator = operator(parts[0])?;
            have_operator = true;
        } else if !parts[0].trim().is_empty() {
            self.head(parts[0])?;
        }
        let mut options = std::collections::HashSet::new();
        for part in parts.iter().skip(1) {
            let part = part.trim();
            if is_operator(part) {
                if have_operator {
                    return Err("Use only one search operator".into());
                }
                self.operator = operator(part)?;
                have_operator = true;
                continue;
            }
            let (key, value) = part
                .split_once(':')
                .ok_or("Expected an operator or named option after ;")?;
            if !options.insert(key) {
                return Err(format!("Repeated option: {key}"));
            }
            self.option(key, value)?;
        }
        self.validate(&options)
    }

    fn option(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "scope" => {
                self.scope = match value {
                    "here" => Scope::Here,
                    "recursive" => Scope::Recursive,
                    "indexed" => Scope::Indexed,
                    _ => return Err("Scope must be here, recursive, or indexed".into()),
                }
            }
            "sort" => self.sort = Some(parse_sort(value)?),
            "type" => {
                self.answer_type = match value {
                    "text" => AnswerType::Text,
                    "number" => AnswerType::Number,
                    "date" => AnswerType::Date,
                    "boolean" => AnswerType::Boolean,
                    _ => {
                        return Err("Answer type must be text, number, date, or boolean".into());
                    }
                }
            }
            "ref" if value == "selected" => self.selected_reference = true,
            _ => return Err(format!("Unknown search option: {key}:{value}")),
        }
        Ok(())
    }

    fn head(&mut self, head: &str) -> Result<(), String> {
        let parts = split(head, ',')?;
        for (position, part) in parts.iter().enumerate() {
            let part = part.trim();
            let (token, trailing) = selector_token(part);
            if token.is_empty() {
                return Err("Empty metadata selector".into());
            }
            self.selectors.push(selector(token)?);
            if !trailing.is_empty() {
                if position + 1 != parts.len() {
                    return Err("Filename text belongs after all selectors".into());
                }
                self.name = payload(trailing.trim(), false)?;
            }
        }
        Ok(())
    }
    fn validate(&self, options: &std::collections::HashSet<&str>) -> Result<(), String> {
        if options.contains("type") && !matches!(self.operator, Operator::Answer(_)) {
            return Err("type belongs only to a q answer-column command".into());
        }
        if !matches!(self.operator, Operator::Name) && !self.name.is_empty() {
            return Err("Use an operator payload or literal filename text, not both".into());
        }
        if matches!(self.operator, Operator::Answer(_)) && self.scope != Scope::Here {
            return Err("Answer columns belong to the current folder only".into());
        }
        let similarity = self
            .sort
            .is_some_and(|sort| sort.field == SortField::Similarity);
        if similarity != self.selected_reference {
            return Err(
                "sort:similarity requires ref:selected; ref belongs only to similarity sorting"
                    .into(),
            );
        }
        if self.scope != Scope::Here
            && self
                .sort
                .is_some_and(|sort| matches!(sort.field, SortField::Dhash | SortField::Similarity))
        {
            return Err("Image hash sorting currently belongs to scope:here".into());
        }
        Ok(())
    }

    pub(super) fn has_selectors(&self) -> bool {
        !self.selectors.is_empty()
    }
    pub fn metadata_matches(&self, entry: &Entry) -> bool {
        self.selectors
            .iter()
            .all(|selector| selector.matches(entry))
    }
    pub fn local_rank(&self, entry: &Entry) -> Option<(u8, usize)> {
        if !self.metadata_matches(entry) {
            return None;
        }
        match &self.operator {
            Operator::Name if self.name.is_empty() => Some((0, 0)),
            Operator::Name if self.progressive => {
                crate::file_manager::type_navigation::navigation_rank(
                    &entry.name,
                    &self.name.to_lowercase(),
                )
            }
            Operator::Name => crate::file_manager::type_navigation::name_rank(
                &entry.name,
                &self.name.to_lowercase(),
            )
            .map(|rank| (rank, 0)),
            Operator::Regex(regex) => regex.is_match(&entry.name).then_some((0, 0)),
            _ => Some((0, 0)),
        }
    }
    pub fn browser_sort(&self) -> Option<sorting::Sort> {
        let sort = self.sort?;
        let column = match sort.field {
            SortField::Name => sorting::Column::Name,
            SortField::Type => sorting::Column::Type,
            SortField::Size => sorting::Column::Size,
            SortField::Modified => sorting::Column::Modified,
            SortField::Created => sorting::Column::Created,
            _ => return None,
        };
        Some(sorting::Sort {
            column,
            descending: sort.descending,
        })
    }
}

impl Selector {
    fn matches(&self, entry: &Entry) -> bool {
        let value = match &self.constraint {
            Constraint::Size(interval) => (!entry.directory && entry.warning.is_none())
                .then(|| interval.contains(i128::from(entry.size_bytes) * range::BYTE_SCALE)),
            Constraint::Modified(interval) => entry
                .modified_time
                .map(|v| interval.contains(range::timestamp(v))),
            Constraint::Created(interval) => entry
                .created_time
                .map(|v| interval.contains(range::timestamp(v))),
            Constraint::Glob(glob) => Some(glob.is_match(&entry.name)),
            Constraint::Family(family) => Some(family.matches(entry)),
        };
        value.is_some_and(|value| value != self.negated)
    }
}
impl Family {
    fn matches(self, entry: &Entry) -> bool {
        match self {
            Self::Folder => entry.directory,
            Self::File => entry.regular_file,
            Self::Image => entry.is_photo(),
            Self::Video => entry.is_video(),
            Self::Audio => {
                entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path)
            }
            Self::Pdf => entry.regular_file && extension(entry) == "pdf",
            Self::Document => {
                entry.regular_file
                    && (crate::file_manager::text_strip::known_text(&entry.path)
                        || matches!(
                            extension(entry).as_str(),
                            "pdf" | "rtf" | "docx" | "odt" | "pptx" | "xlsx"
                        ))
            }
        }
    }
}
fn extension(entry: &Entry) -> String {
    entry
        .path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}
fn selector(text: &str) -> Result<Selector, String> {
    let (negated, text) = text.strip_prefix('!').map_or((false, text), |v| (true, v));
    let constraint = if let Some(value) = text.strip_prefix("modified:") {
        Constraint::Modified(range::date(value)?)
    } else if let Some(value) = text.strip_prefix("created:") {
        Constraint::Created(range::date(value)?)
    } else if text.starts_with(|c: char| c.is_ascii_digit() || "<>=-".contains(c)) {
        Constraint::Size(range::size(text)?)
    } else if text.contains(['*', '?']) {
        let mut pattern = String::from("\\A(?:");
        for c in text.chars() {
            match c {
                '*' => pattern.push_str(".*"),
                '?' => pattern.push('.'),
                _ => pattern.push_str(&regex::escape(&c.to_string())),
            }
        }
        pattern.push_str(")\\z");
        Constraint::Glob(
            RegexBuilder::new(&pattern)
                .case_insensitive(true)
                .size_limit(256 * 1024)
                .build()
                .map_err(|e| e.to_string())?,
        )
    } else {
        Constraint::Family(match text {
            "image" => Family::Image,
            "audio" => Family::Audio,
            "video" => Family::Video,
            "document" => Family::Document,
            "pdf" => Family::Pdf,
            "folder" => Family::Folder,
            "file" => Family::File,
            _ => return Err(format!("Unknown metadata selector: {text}")),
        })
    };
    Ok(Selector {
        source: format!("{}{text}", if negated { "!" } else { "" }),
        constraint,
        negated,
    })
}
fn is_operator(text: &str) -> bool {
    ["regex:", "content:", "emb:", "q:"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}
fn operator(text: &str) -> Result<Operator, String> {
    let (kind, value) = text.split_once(':').ok_or("Missing operator payload")?;
    if kind == "regex" && !value.starts_with('"') && value.chars().any(char::is_whitespace) {
        return Err("Quote a regex containing whitespace".into());
    }
    let value = payload(value, kind != "regex")?;
    if value.is_empty() {
        return Err("Search operator payload cannot be empty".into());
    }
    Ok(match kind {
        "regex" => {
            if value.len() > 4096 {
                return Err("Regex patterns are limited to 4 KiB".into());
            }
            Operator::Regex(
                RegexBuilder::new(&format!("\\A(?:{value})"))
                    .size_limit(1024 * 1024)
                    .build()
                    .map_err(|e| format!("Invalid regular expression: {e}"))?,
            )
        }
        "content" => Operator::Content(value),
        "emb" => Operator::Embedding(value),
        "q" => Operator::Answer(value),
        _ => return Err("Unknown search operator".into()),
    })
}
fn parse_sort(text: &str) -> Result<Sort, String> {
    let (field, direction) = text.split_once(':').unwrap_or((text, "asc"));
    let field = match field {
        "name" => SortField::Name,
        "type" => SortField::Type,
        "size" => SortField::Size,
        "modified" => SortField::Modified,
        "created" => SortField::Created,
        "dhash" => SortField::Dhash,
        "similarity" => SortField::Similarity,
        _ => return Err(format!("Unknown sort field: {field}")),
    };
    let descending = match direction {
        "asc" => false,
        "desc" => true,
        _ => return Err("Sort direction must be asc or desc".into()),
    };
    Ok(Sort { field, descending })
}

/// Split only outside double quotes; preserve regex backslashes for payload decoding.
fn split(text: &str, delimiter: char) -> Result<Vec<&str>, String> {
    let (mut quoted, mut escaped, mut start) = (false, false, 0);
    let mut parts = Vec::new();
    for (index, c) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quoted {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if c == delimiter && !quoted {
            parts.push(&text[start..index]);
            start = index + c.len_utf8();
        }
    }
    if quoted {
        return Err("Unterminated quoted payload".into());
    }
    parts.push(&text[start..]);
    Ok(parts)
}
fn payload(text: &str, require_quotes: bool) -> Result<String, String> {
    let text = text.trim();
    if !text.starts_with('"') {
        if require_quotes || text.contains('"') {
            return Err("Use a double-quoted payload".into());
        }
        return Ok(text.into());
    }
    let mut output = String::new();
    let mut chars = text[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                return if chars.as_str().trim().is_empty() {
                    Ok(output)
                } else {
                    Err("Unexpected text after quoted payload".into())
                };
            }
            '\\' => match chars.next() {
                Some(next @ ('"' | '\\')) => output.push(next),
                Some(next) => {
                    output.push('\\');
                    output.push(next);
                }
                None => return Err("Incomplete quoted escape".into()),
            },
            _ => output.push(c),
        }
    }
    Err("Unterminated quoted payload".into())
}
fn selector_token(text: &str) -> (&str, &str) {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let first = &text[..end];
    // A spaced hyphen belongs to a date range, not trailing filename text.
    let unnegated = first.trim_start_matches('!');
    if unnegated.starts_with("modified:") || unnegated.starts_with("created:") {
        let rest = text[end..].trim_start();
        if let Some(after) = rest.strip_prefix("- ") {
            let after = after.trim_start();
            let bound_end = after.find(char::is_whitespace).unwrap_or(after.len());
            let total = text.len() - after.len() + bound_end;
            return (&text[..total], &text[total..]);
        }
    }
    (first, &text[end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planned_commands_share_one_unambiguous_parser() {
        for input in [
            "foo",
            "::foo",
            ":10kb..2MB foo",
            ":10kb...2MB,*.jpg foo",
            ":10kb-2MB,image foo",
            ":image,!*.svg",
            ":created:>=2026-01-01;sort:created:desc",
            ":modified:2026-01-01 - 2026-03-31 foo",
            ":regex:foo.*",
            r#":regex:"(?i)foo.*\.jpg$""#,
            r#":content:"invoice number""#,
            r#":10kb..2MB,*.pdf;emb:"2019 tax document";scope:indexed"#,
            r#":q:"End balance";type:number"#,
            ":image;sort:similarity;ref:selected",
        ] {
            Query::parse(input).unwrap_or_else(|error| panic!("{input}: {error}"));
        }
        assert_eq!(Query::parse("::foo").unwrap().name, ":foo");
    }
    #[test]
    fn invalid_options_and_ambiguous_modes_never_fall_back_to_names() {
        for input in [
            ":",
            ":unknown",
            ":image,",
            ":image;sort:size:sideways",
            ":image;type:number",
            ":image;ref:selected",
            ":image;sort:similarity",
            ":q:\"x\";scope:recursive",
            ":emb:tax",
            ":regex:(?=bad)",
            ":image;scope:here;scope:indexed",
            ":content:\"x\";emb:\"y\"",
            ":created:2026-02-30",
            ":emb:\"x\"junk",
        ] {
            assert!(Query::parse(input).is_err(), "{input}");
        }
    }
    #[test]
    fn quoted_separators_and_regex_escapes_are_preserved() {
        let Operator::Embedding(value) = Query::parse(r#":emb:"tax, 2019; \"quoted\"""#)
            .unwrap()
            .operator
        else {
            panic!()
        };
        assert!(value.contains("tax, 2019;"));
        let Operator::Regex(regex) = Query::parse(r#":regex:"\d+.*\.jpg$""#).unwrap().operator
        else {
            panic!()
        };
        assert!(regex.is_match("123-foo.jpg"));
        assert!(!regex.is_match("a123-foo.jpg"));
    }
}
