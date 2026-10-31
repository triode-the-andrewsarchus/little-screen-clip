//! Small complete documents made exclusively of literal labelled fields.
use super::frontmatter::{self, Field, Header};
use crate::file_manager::search::{answer::Value as TypedValue, query::AnswerType};
use std::collections::HashSet;

pub(super) const MAX_DOCUMENT: usize = 4096;
const MAX_FIELDS: usize = 128;

pub(super) fn inspect(source: &str) -> Result<Option<Header>, String> {
    if source.len() > MAX_DOCUMENT || source.contains('\0') {
        return Ok(None);
    }
    let mut fields = Vec::new();
    let mut labels = HashSet::new();
    for line in source.strip_prefix('\u{feff}').unwrap_or(source).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((label, value)) = line.split_once(':') else {
            return Ok(None);
        };
        let label = label.trim();
        let value = value.trim();
        let Some(field) = field(label, value) else {
            return Ok(None);
        };
        if !labels.insert(label.to_lowercase()) || fields.len() == MAX_FIELDS {
            return Ok(None);
        }
        fields.push(field);
    }
    if fields.is_empty() {
        return Ok(None);
    }
    frontmatter::header("labelled text document", fields, source)
}

pub(super) fn field(label: &str, value: &str) -> Option<Field> {
    if !valid_label(label) || value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let kind = if TypedValue::parse(value, None, AnswerType::Date).is_ok() {
        "date"
    } else if matches!(value, "true" | "false") {
        "boolean"
    } else if value.parse::<i64>().is_ok() {
        "integer"
    } else {
        "text"
    };
    Some(Field {
        path: vec![label.into()],
        value: value.into(),
        kind: kind.into(),
    })
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 128
        && label.chars().any(char::is_alphabetic)
        && label
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, ' ' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values_and_layout_survive_independent_files() {
        let first = inspect("Account: Savings\nStatement date: 2021-11-18\nBalance: 10.50\n")
            .unwrap()
            .unwrap();
        let second = inspect("Account: Checking\nStatement date: 2025-01-04\nBalance: 0.00\n")
            .unwrap()
            .unwrap();
        assert_eq!(first.schema, second.schema);
        assert_eq!(first.fields[2].value, "10.50");
        assert_eq!(second.fields[2].value, "0.00");
        assert_eq!(first.fields[1].kind, "date");
        assert_ne!(
            first.schema,
            inspect("Account: Savings\nStatement date: unknown\nBalance: 10.50\n")
                .unwrap()
                .unwrap()
                .schema
        );
        assert_ne!(
            first.schema,
            inspect("Account: Savings\nBalance: 10.50\nStatement date: 2021-11-18\n")
                .unwrap()
                .unwrap()
                .schema
        );
    }

    #[test]
    fn ambiguity_prose_and_budget_fail_closed() {
        for source in [
            "Date: first\ndate: second",
            "Date: first\nA competing fact in prose.",
            "Date: first\nDate:",
            "Bad/path: value",
            "Date: first\nKey: val\0ue",
        ] {
            assert!(inspect(source).unwrap().is_none(), "{source:?}");
        }
        assert!(
            inspect(&format!("Date: {}", "a".repeat(MAX_DOCUMENT)))
                .unwrap()
                .is_none()
        );
        assert!(
            inspect(
                &(0..=MAX_FIELDS)
                    .map(|n| format!("key{n}: value\n"))
                    .collect::<String>()
            )
            .unwrap()
            .is_none()
        );
    }
}
