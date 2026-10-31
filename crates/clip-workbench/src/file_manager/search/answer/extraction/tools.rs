//! Model-proposed searches run only over supplied bytes in the isolated worker.
use super::super::{AnswerType, Source, State, context};
use super::{Attempt, Request, Response};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const MAX_SPANS: usize = 8;
const MAX_WINDOW: usize = 4096;
const MAX_EVIDENCE: usize = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager::search) struct Search {
    queries: Vec<Query>,
    before: usize,
    after: usize,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    mode: Mode,
    pattern: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Literal,
    Regex,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager::search) struct Span {
    source: usize,
    start: usize,
    end: usize,
    tabular: bool,
}

pub(super) fn execute(sources: &[Source], spec: &Search) -> Result<Response, String> {
    validate_sources(sources, spec)?;
    let mut spans = BTreeSet::new();
    let mut bytes = 0;
    for query in &spec.queries {
        let regex = compile(query)?;
        for source in sources {
            for found in regex.find_iter(&source.text) {
                if found.is_empty() {
                    return Err("Empty-match searches are not supported".into());
                }
                let span = window(source, found.start(), found.end(), spec);
                if spans.contains(&span) {
                    continue;
                }
                bytes += span.end - span.start;
                if span.end - span.start > MAX_WINDOW
                    || spans.len() == MAX_SPANS
                    || bytes > MAX_EVIDENCE
                {
                    return Ok(Response::Matches {
                        spans: spans.into_iter().collect(),
                        limited: true,
                    });
                }
                spans.insert(span);
            }
        }
    }
    Ok(Response::Matches {
        spans: spans.into_iter().collect(),
        limited: false,
    })
}

fn validate_sources(sources: &[Source], spec: &Search) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    if sources.is_empty()
        || sources.len() > 512
        || sources
            .iter()
            .map(|source| source.text.len())
            .sum::<usize>()
            > 512 * 1024
        || sources
            .iter()
            .any(|source| !ids.insert(source.id) || source.location.len() > 1024)
        || spec.queries.is_empty()
        || spec.queries.len() > 4
        || spec.before > 1024
        || spec.after > 1024
    {
        return Err("Text search exceeds its source or query budget".into());
    }
    Ok(())
}

fn compile(query: &Query) -> Result<regex::Regex, String> {
    if query.pattern.is_empty() || query.pattern.len() > 512 || query.pattern.contains('\0') {
        return Err("Invalid text search pattern".into());
    }
    let pattern = match query.mode {
        Mode::Literal => regex::escape(&query.pattern),
        Mode::Regex => query.pattern.clone(),
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .size_limit(256 * 1024)
        .dfa_size_limit(256 * 1024)
        .nest_limit(32)
        .build()
        .map_err(|_| "Unsupported or oversized non-backtracking search expression")?;
    if regex.is_match("") {
        return Err("Empty-match searches are not supported".into());
    }
    Ok(regex)
}

fn window(source: &Source, start: usize, end: usize, spec: &Search) -> Span {
    let tabular = tabular_match(&source.text, start, end);
    let mut start = start.saturating_sub(spec.before);
    let mut end = end.saturating_add(spec.after).min(source.text.len());
    while !source.text.is_char_boundary(start) {
        start -= 1;
    }
    while !source.text.is_char_boundary(end) {
        end += 1;
    }
    Span {
        source: source.id,
        start,
        end,
        tabular,
    }
}

fn tabular_match(text: &str, start: usize, end: usize) -> bool {
    let first = text[..start].rfind('\n').map_or(0, |offset| offset + 1);
    let last = text[end..]
        .find('\n')
        .map_or(text.len(), |offset| end + offset);
    text[first..last].lines().any(|line| {
        line.split(['|', '\t'])
            .filter(|cell| !cell.trim().is_empty())
            .take(2)
            .count()
            == 2
    })
}

fn evidence(sources: &[Source], spans: &[Span]) -> Result<Vec<Source>, String> {
    let mut evidence = Vec::new();
    let mut bytes = 0;
    if spans.len() > MAX_SPANS {
        return Err("Too many text search windows".into());
    }
    for span in spans {
        let source = sources
            .iter()
            .find(|source| source.id == span.source)
            .ok_or("Unknown search source")?;
        let text = source
            .text
            .get(span.start..span.end)
            .filter(|text| !text.is_empty())
            .ok_or("Invalid search offsets")?;
        bytes += text.len();
        if text.len() > MAX_WINDOW || bytes > MAX_EVIDENCE {
            return Err("Text search evidence exceeds budget".into());
        }
        evidence.push(Source {
            id: evidence.len(),
            text: text.into(),
            location: format!(
                "{}; text search UTF-8 bytes {}..{}",
                source.location, span.start, span.end
            ),
        });
    }
    Ok(evidence)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    sufficient: bool,
    reason: String,
}

pub(in crate::file_manager::search::answer) fn try_answer(
    sources: &[Source],
    question: &str,
    kind: AnswerType,
    cancel: &AtomicBool,
    deadline: Instant,
    infer: &mut impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Attempt, String> {
    let Some(timeout) = context::remaining(deadline, cancel)? else {
        return Ok(Attempt::Skip);
    };
    let proposed = infer(
        json!({"op":"text_search_plan", "question":question, "type":kind,
            "literal_field":super::session::literal_question_field(question)}),
        timeout.min(Duration::from_secs(30)),
    )?;
    let spec: Option<Search> =
        serde_json::from_value(proposed).map_err(|_| "Invalid text search plan")?;
    let Some(spec) = spec else {
        return Ok(Attempt::Skip);
    };
    let search = serde_json::to_value(&spec).map_err(|_| "Invalid search description")?;
    let Response::Matches {
        spans,
        limited: false,
    } = super::call(
        &Request::Search {
            sources: sources.to_vec(),
            spec,
        },
        cancel,
    )?
    else {
        return Ok(Attempt::Skip);
    };
    if spans.iter().any(|span| span.tabular) {
        tracing::debug!(target: "little_clip_search_timing", "column text search requires table scope");
        return Ok(Attempt::Escalate("A text search matched tabular content without a validated row-selection plan. Examine the complete table/document before choosing a cell.".into()));
    }
    let selected = evidence(sources, &spans)?;
    if selected.is_empty() {
        return Ok(Attempt::Skip);
    }
    let mut answer = context::reconcile(&selected, question, kind, cancel, deadline, &mut *infer)?;
    let State::Value(_, raw) = &answer.state else {
        return Ok(Attempt::Skip);
    };
    let reviewed = review_search(
        json!({"op":"text_search_review", "question":question,
        "type":kind, "sources":selected, "search":search, "complete":false, "value":raw}),
        cancel,
        deadline,
        infer,
    )?;
    if !reviewed.sufficient {
        tracing::debug!(target: "little_clip_search_timing", "column text search escalated");
        return Ok(Attempt::Escalate(reviewed.reason));
    }
    answer.reason.push_str(" Answered from reviewed read-only text-search windows; omitted text does not establish absence or whole-file coverage.");
    tracing::debug!(target: "little_clip_search_timing", "column text search accepted");
    Ok(Attempt::Answer(answer))
}

fn review_search(
    request: Value,
    cancel: &AtomicBool,
    deadline: Instant,
    infer: &mut impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Review, String> {
    let Some(timeout) = context::remaining(deadline, cancel)? else {
        return Ok(Review {
            sufficient: false,
            reason: "Text search review exceeded its time budget.".into(),
        });
    };
    let reviewed: Review =
        serde_json::from_value(infer(request, timeout.min(Duration::from_secs(30)))?)
            .map_err(|_| "Invalid text search review")?;
    if reviewed.reason.trim().is_empty() || reviewed.reason.len() > 512 {
        return Err("Invalid text search review reason".into());
    }
    Ok(reviewed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec(mode: &str, pattern: &str) -> Search {
        serde_json::from_value(
            json!({"queries":[{"mode":mode,"pattern":pattern}],"before":20,"after":30}),
        )
        .unwrap()
    }
    #[test]
    fn literal_regex_and_unicode_windows_retain_exact_offsets() {
        let sources = [Source {
            id: 7,
            text: format!(
                "{} Closing balance: 10.50 EUR {}",
                "é中🦇".repeat(9),
                "雪".repeat(30)
            ),
            location: "page 3".into(),
        }];
        for (mode, pattern) in [
            ("literal", "Closing balance"),
            ("literal", "closing balance"),
            ("regex", r"(?i)closing\s+balance"),
        ] {
            let Response::Matches { spans, limited } =
                execute(&sources, &spec(mode, pattern)).unwrap()
            else {
                panic!()
            };
            assert!(!limited);
            let selected = evidence(&sources, &spans).unwrap();
            assert_eq!(selected.len(), 1);
            assert!(selected[0].text.contains("Closing balance: 10.50 EUR"));
            assert!(
                selected[0]
                    .location
                    .starts_with("page 3; text search UTF-8 bytes")
            );
            assert_eq!(
                sources[0].text.get(spans[0].start..spans[0].end),
                Some(selected[0].text.as_str())
            );
        }
    }
    #[test]
    fn regex_and_protocol_limits_fail_closed_and_saturation_is_explicit() {
        for pattern in ["", ".*", "(?=secret)", r"(a)\1", &"x".repeat(513)] {
            assert!(
                compile(&spec("regex", pattern).queries[0]).is_err(),
                "{pattern}"
            );
        }
        let source = Source {
            id: 0,
            text: "needle and other text. ".repeat(100),
            location: "text".into(),
        };
        assert!(matches!(
            execute(&[source], &spec("literal", "needle")).unwrap(),
            Response::Matches { limited: true, .. }
        ));
        assert!(
            serde_json::from_value::<Search>(
                json!({"queries":[],"before":0,"after":0,"command":"rm"})
            )
            .is_err()
        );
        let sources = [Source {
            id: 0,
            text: "needle".into(),
            location: "text".into(),
        }];
        assert!(
            evidence(
                &sources,
                &[Span {
                    source: 0,
                    start: 1,
                    end: 999,
                    tabular: false
                }]
            )
            .is_err()
        );
        assert!(
            evidence(
                &sources,
                &[Span {
                    source: 8,
                    start: 0,
                    end: 1,
                    tabular: false
                }]
            )
            .is_err()
        );
        assert!(execute(&sources, &spec("literal", "missing")).is_ok_and(
            |result| matches!(result,Response::Matches {spans,limited:false} if spans.is_empty())
        ));
        assert!(execute(&sources, &spec("regex", r"\b")).is_err());
        assert!(
            execute(
                &[sources[0].clone(), sources[0].clone()],
                &spec("literal", "needle")
            )
            .is_err()
        );
        let oversized = [Source {
            id: 0,
            text: "x".repeat(512 * 1024 + 1),
            location: "text".into(),
        }];
        assert!(execute(&oversized, &spec("literal", "x")).is_err());
        assert!(serde_json::from_value::<Request>(json!({
            "operation":"search", "sources":[{"id":0,"text":"x","location":"text","path":"/etc/passwd"}],
            "spec":{"queries":[{"mode":"literal","pattern":"x"}],"before":0,"after":0},
        })).is_err());
    }

    #[test]
    fn table_matches_need_row_scope_but_adjacent_tables_do_not_block_prose_fields() {
        let source = Source {
            id: 0,
            text: "Code: ALPHA\nSKU | Quantity | Unit\nZX | 0 | units".into(),
            location: "text".into(),
        };
        for (pattern, expected) in [("Code", false), ("Quantity", true)] {
            let Response::Matches {
                spans,
                limited: false,
            } = execute(std::slice::from_ref(&source), &spec("literal", pattern)).unwrap()
            else {
                panic!()
            };
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].tabular, expected);
        }
    }
}
