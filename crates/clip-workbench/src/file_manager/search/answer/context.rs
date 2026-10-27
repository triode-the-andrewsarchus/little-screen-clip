//! Bounded window evidence selection and reconciliation for larger text sources.
use super::{Answer, AnswerType, Evidence, Source, State};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const WINDOW_BYTES: usize = 8 * 1024;
const MAX_WINDOWS: usize = 64;
const MAX_EVIDENCE: usize = 64;
const EVIDENCE_BYTES: usize = 24 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowReply {
    evidence: Vec<Evidence>,
    limited: bool,
}
fn payload(question: &str, kind: AnswerType, sources: &[Source], complete: bool) -> Value {
    json!({"question":question,"type":kind,"sources":sources,"complete":complete})
}
fn fits(request: &Value) -> bool {
    serde_json::to_vec(request).is_ok_and(|bytes| bytes.len() <= 30 * 1024)
}
fn unknown(reason: &str) -> Answer {
    Answer::status(State::Unknown, reason)
}

pub(in super::super) fn answer_document(
    sources: &[Source],
    complete: bool,
    question: &str,
    kind: AnswerType,
    cancel: &AtomicBool,
    mut infer: impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Answer, String> {
    if !complete {
        return Ok(unknown(
            "Source extraction is incomplete or exceeds the 512 KiB/512-source read budget.",
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Answer cancelled".into());
    }
    let full = payload(question, kind, sources, true);
    if sources.len() <= 64
        && sources
            .iter()
            .map(|source| source.text.len())
            .sum::<usize>()
            <= 24 * 1024
        && fits(&full)
    {
        return Answer::decode(infer(full, Duration::from_secs(90))?, kind, sources, true);
    }
    answer_selected_document(
        sources,
        Vec::new(),
        question,
        kind,
        cancel,
        Instant::now() + Duration::from_secs(180),
        infer,
    )
}

pub(super) fn answer_selected_document(
    sources: &[Source],
    mut evidence: Vec<Source>,
    question: &str,
    kind: AnswerType,
    cancel: &AtomicBool,
    deadline: Instant,
    mut infer: impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Answer, String> {
    if !evidence_fits(&evidence) {
        return Ok(unknown(
            "Selected evidence exceeds the reconciliation budget.",
        ));
    }
    for (id, source) in evidence.iter_mut().enumerate() {
        source.id = id;
    }
    let windows = windows(sources);
    if windows.len() > MAX_WINDOWS {
        return Ok(unknown("Document exceeds the 64-window answer budget."));
    }
    let mut seen = BTreeSet::new();
    for window in windows {
        let mut request = payload(question, kind, &window, false);
        request["op"] = json!("evidence");
        if !fits(&request) {
            return Ok(unknown(
                "Question/window exceeds the evidence input budget.",
            ));
        }
        let Some(timeout) = remaining(deadline, cancel)? else {
            return Ok(unknown(
                "Document answering exceeded its inference time budget.",
            ));
        };
        let reply: WindowReply = serde_json::from_value(infer(request, timeout)?)
            .map_err(|_| "Invalid window evidence reply")?;
        if !extend_evidence(&mut evidence, &mut seen, reply, &window)? {
            return Ok(unknown(
                "Evidence selection exceeds a window/context/output or reconciliation budget.",
            ));
        }
    }
    reconcile(&evidence, question, kind, cancel, deadline, infer)
}

fn reconcile(
    evidence: &[Source],
    question: &str,
    kind: AnswerType,
    cancel: &AtomicBool,
    deadline: Instant,
    mut infer: impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Answer, String> {
    if evidence.is_empty() {
        return Ok(unknown(
            "No supported answer evidence was found in the document windows.",
        ));
    }
    let mut request = payload(question, kind, evidence, true);
    request["context"] = json!("evidence");
    if !fits(&request) {
        return Ok(unknown(
            "Question/evidence exceeds the reconciliation input budget.",
        ));
    }
    let Some(timeout) = remaining(deadline, cancel)? else {
        return Ok(unknown(
            "Document answering exceeded its inference time budget.",
        ));
    };
    let reply = infer(request, timeout)?;
    if reply.get("basis").is_none() {
        return Err("Missing evidence sufficiency basis".into());
    }
    Answer::decode(reply, kind, evidence, true)
}
fn extend_evidence(
    evidence: &mut Vec<Source>,
    seen: &mut BTreeSet<(usize, String)>,
    reply: WindowReply,
    window: &[Source],
) -> Result<bool, String> {
    if reply.limited || reply.evidence.len() >= 4 {
        return Ok(false);
    }
    super::evidence(&reply.evidence, window)?;
    for item in reply.evidence {
        if item.quote.len() > 512 {
            return Err("Window evidence quotation exceeds its budget".into());
        }
        if !seen.insert((item.source, item.quote.clone())) {
            continue;
        }
        let source = window
            .iter()
            .find(|source| source.id == item.source)
            .ok_or("Unknown evidence source")?;
        evidence.push(Source {
            id: evidence.len(),
            text: item.quote,
            location: source.location.clone(),
        });
        if !evidence_fits(evidence) {
            return Ok(false);
        }
    }
    Ok(true)
}
fn evidence_fits(sources: &[Source]) -> bool {
    sources.len() <= MAX_EVIDENCE
        && sources
            .iter()
            .map(|source| source.text.len())
            .sum::<usize>()
            <= EVIDENCE_BYTES
}

pub(super) fn remaining(
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<Option<Duration>, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Answer cancelled".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Ok(None);
    }
    Ok(Some(remaining.min(Duration::from_secs(90))))
}
fn windows(sources: &[Source]) -> Vec<Vec<Source>> {
    let mut windows = Vec::new();
    let mut window: Vec<Source> = Vec::new();
    let mut bytes = 0;
    for source in sources {
        let mut start = 0;
        while start < source.text.len() {
            let mut end = (start + WINDOW_BYTES).min(source.text.len());
            while !source.text.is_char_boundary(end) {
                end -= 1;
            }
            let text = &source.text[start..end];
            if !window.is_empty()
                && (bytes + text.len() > WINDOW_BYTES
                    || window.len() == 64
                    || window.iter().any(|part| part.id == source.id))
            {
                windows.push(std::mem::take(&mut window));
                bytes = 0;
            }
            window.push(Source {
                id: source.id,
                text: text.into(),
                location: source.location.clone(),
            });
            bytes += text.len();
            if end == source.text.len() {
                break;
            }
            start = end.saturating_sub(256).max(start + 1);
            while !source.text.is_char_boundary(start) {
                start += 1;
            }
        }
    }
    if !window.is_empty() {
        windows.push(window);
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_evidence_reaches_reconciliation_with_original_location() {
        let sources = vec![Source {
            id: 7,
            text: format!(
                "{}\nRating: 18 points.",
                "Ordinary background. ".repeat(1600)
            ),
            location: "page 19".into(),
        }];
        let mut maps = 0;
        let mut reduced = false;
        let result = answer_document(&sources, true, "What rating?", AnswerType::Number,
            &AtomicBool::new(false), |request, timeout| {
                assert!(timeout <= Duration::from_secs(90));
                if request["op"] == "evidence" {
                    maps += 1;
                    let relevant = request["sources"].as_array().unwrap().iter()
                        .find(|source| source["text"].as_str().unwrap().contains("Rating: 18 points."));
                    Ok(json!({"limited":false,"evidence":relevant.map_or_else(Vec::new,
                        |_| vec![json!({"source":7,"quote":"Rating: 18 points."})])}))
                } else {
                    reduced = true;
                    assert_eq!(request["context"], "evidence");
                    assert_eq!(request["sources"][0]["location"], "page 19");
                    Ok(json!({"basis":"direct","applicability":"applicable","state":"value",
                        "value":"18","unit":"points","evidence":[{"source":0,"quote":"Rating: 18 points."}],
                        "reason":"Explicit rating."}))
                }
            }).unwrap();
        assert!(maps > 3 && reduced);
        assert_eq!(result.label(), "18 points");
        assert_eq!(result.evidence[0].0, "page 19");
    }

    #[test]
    fn saturated_evidence_abstains_without_reconciling() {
        let sources = vec![Source {
            id: 0,
            text: "x".repeat(25 * 1024),
            location: "text".into(),
        }];
        let mut calls = 0;
        let answer = answer_document(
            &sources,
            true,
            "Question?",
            AnswerType::Text,
            &AtomicBool::new(false),
            |_, _| {
                calls += 1;
                Ok(json!({"limited":true,"evidence":[]}))
            },
        )
        .unwrap();
        assert!(matches!(answer.state, State::Unknown));
        assert_eq!(calls, 1);
    }

    #[test]
    fn cancelled_or_incomplete_documents_do_not_start_inference() {
        let sources = vec![Source {
            id: 0,
            text: "x".repeat(25 * 1024),
            location: "text".into(),
        }];
        let cancel = AtomicBool::new(true);
        assert!(
            answer_document(
                &sources,
                true,
                "Question?",
                AnswerType::Text,
                &cancel,
                |_, _| panic!("cancelled request reached inference")
            )
            .is_err()
        );
        let answer = answer_document(
            &sources,
            false,
            "Question?",
            AnswerType::Text,
            &cancel,
            |_, _| panic!("incomplete request reached inference"),
        )
        .unwrap();
        assert!(matches!(answer.state, State::Unknown));
    }

    #[test]
    fn window_boundaries_preserve_unicode_and_overlap() {
        let text = "aé中🦇".repeat(4000);
        let sources = vec![Source {
            id: 3,
            text: text.clone(),
            location: "section".into(),
        }];
        let parts = windows(&sources);
        assert!(parts.len() > 1);
        let mut previous_end = 0;
        for window in parts {
            assert_eq!(window.len(), 1);
            let part = &window[0];
            assert!(part.text.len() <= WINDOW_BYTES);
            assert_eq!(part.id, 3);
            // Recover the exact reconstruction using the same bounded overlap rule.
            let start = if previous_end == 0 {
                0
            } else {
                let mut start = previous_end - 256;
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                start
            };
            assert_eq!(part.text, text[start..start + part.text.len()]);
            previous_end = start + part.text.len();
        }
        assert_eq!(previous_end, text.len());
    }

    #[test]
    fn an_expired_shared_deadline_abstains_without_starting_inference() {
        let sources = vec![Source {
            id: 0,
            text: "Rating: 18 points.".into(),
            location: "page 1".into(),
        }];
        let answer = answer_selected_document(
            &sources,
            Vec::new(),
            "What rating?",
            AnswerType::Number,
            &AtomicBool::new(false),
            Instant::now(),
            |_, _| panic!("Expired budget reached inference"),
        )
        .unwrap();
        assert!(matches!(answer.state, State::Unknown));
    }

    #[test]
    fn fabricated_quotations_are_rejected_before_reconciliation() {
        let sources = vec![Source {
            id: 0,
            text: "x".repeat(25 * 1024),
            location: "text".into(),
        }];
        let result = answer_document(
            &sources,
            true,
            "Question?",
            AnswerType::Text,
            &AtomicBool::new(false),
            |_, _| {
                Ok(json!({"limited":false,
                "evidence":[{"source":0,"quote":"Invented fact"}]}))
            },
        );
        assert!(result.unwrap_err().contains("not present"));
    }
}
