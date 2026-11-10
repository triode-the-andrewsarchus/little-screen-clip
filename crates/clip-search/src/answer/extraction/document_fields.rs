//! Literal field discovery in a complete bounded document, including prose.
//!
//! Model planning selects an offered field; it cannot generate executable code.
//! Non-field text is part of the template identity and the semantic review.
use super::{frontmatter, lines};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub(super) fn inspect(source: &str) -> Result<Option<frontmatter::Header>, String> {
    if source.len() > lines::MAX_DOCUMENT || source.contains('\0') {
        return Ok(None);
    }
    let mut fields = Vec::new();
    let mut labels = HashSet::new();
    let mut template = Sha256::new();
    for line in source.lines() {
        let pair = pair(line);
        if pair.is_some_and(|(_, value)| value.is_empty()) {
            return Ok(None);
        }
        let field = pair.and_then(|(label, value)| lines::field(label, value));
        let shape = if let Some(field) = field {
            if !labels.insert(field.path[0].to_lowercase()) || fields.len() == 128 {
                return Ok(None);
            }
            // Serialize rather than concatenate labels and kinds: untrusted
            // delimiters cannot impersonate a different template component.
            let shape = serde_json::to_vec(&(&field.path, &field.kind))
                .map_err(|_| "Invalid document-field shape")?;
            fields.push(field);
            shape
        } else {
            serde_json::to_vec(line).map_err(|_| "Invalid document text")?
        };
        template.update(shape.len().to_le_bytes());
        template.update(shape);
    }
    if fields.is_empty() {
        return Ok(None);
    }
    let mut header = frontmatter::header("literal document fields", fields, source)?;
    if let Some(header) = &mut header {
        // Changing surrounding prose requires a new review even when every
        // label/type is identical. Only changing captured values may reuse it.
        header.schema = format!("{}:{:x}", header.schema, template.finalize());
    }
    Ok(header)
}

fn pair(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if let Some(row) = line
        .strip_prefix('|')
        .and_then(|line| line.strip_suffix('|'))
    {
        let mut cells = row.split('|').map(str::trim);
        let pair = (cells.next()?, cells.next()?);
        return cells.next().is_none().then_some(pair);
    }
    if line.contains('\t') {
        let mut cells = line.split('\t').map(str::trim);
        let pair = (cells.next()?, cells.next()?);
        return cells.next().is_none().then_some(pair);
    }
    let (label, value) = line.split_once([':', '='])?;
    Some((label.trim(), value.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_assignments_and_two_column_tables_preserve_literal_values() {
        for source in [
            "Survey report\nThe following data was recorded.\nRecorded date = 2020-02-03\nCount = 0\nEnd of report.",
            "Survey report\n| Recorded date | 2020-02-03 |\n| Count | 0 |\nEnd of report.",
            "Survey report\nRecorded date\t2020-02-03\nCount\t0\nEnd of report.",
        ] {
            let header = inspect(source).unwrap().unwrap();
            assert_eq!(header.fields.len(), 2);
            assert_eq!(header.fields[0].value, "2020-02-03");
            assert_eq!(header.fields[1].value, "0");
            assert_eq!(header.raw, source);
        }
    }

    #[test]
    fn only_captured_values_can_change_without_reviewing_the_template_again() {
        let source = "Report\nRecorded date = 2020-02-03\nThe date was verified.\n";
        let first = inspect(source).unwrap().unwrap();
        let next = inspect(&source.replace("2020-02-03", "2023-04-05"))
            .unwrap()
            .unwrap();
        assert_eq!(first.schema, next.schema);
        for changed in [
            source.replace("verified", "corrupt"),
            source.replace("2020-02-03", "unknown"),
            source.replace("Recorded date", "Other date"),
        ] {
            assert_ne!(first.schema, inspect(&changed).unwrap().unwrap().schema);
        }
    }

    #[test]
    fn ambiguous_or_oversized_documents_decline_capture() {
        for source in [
            "Report\nRecorded date: first\nrecorded date = second",
            "| Recorded date | first |\nRecorded date\tsecond",
            "Recorded date: first\nRecorded date:",
            "Prose without a labeled value.",
            "| a | b | c |",
            "a\tb\tc",
            "Date: 2020-02-03\0",
        ] {
            assert!(inspect(source).unwrap().is_none(), "{source:?}");
        }
        assert!(
            inspect(&"a".repeat(lines::MAX_DOCUMENT + 1))
                .unwrap()
                .is_none()
        );
    }
}
