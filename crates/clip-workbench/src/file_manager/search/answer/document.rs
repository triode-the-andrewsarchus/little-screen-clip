//! Bounded mixed text/page-image collection and shared evidence reconciliation.
use super::{Answer, AnswerType, Source, State, context, visual};
use crate::file_manager::{
    filesystem::Entry,
    search::reader::{self, Prepared, Reply, Request, SourceRoot},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(in super::super) struct Document {
    text: Vec<Source>,
    images: Vec<Prepared>,
    pages: usize,
    complete: bool,
    unsupported: Option<String>,
}
impl Document {
    pub fn needs_vision(&self) -> bool {
        self.complete && !self.images.is_empty()
    }

    pub fn complete_single_text(&self) -> Option<&Source> {
        (self.complete && self.images.is_empty() && self.text.len() == 1).then(|| &self.text[0])
    }
}

pub(in super::super) fn read_document(
    root: &SourceRoot,
    entry: &Entry,
    cancel: &AtomicBool,
) -> Result<Document, String> {
    collect(|cursor| {
        let Reply::Prepared(prepared) = reader::call(
            &Request::Extract {
                root: root.clone(),
                entry: entry.clone(),
                cursor,
                text_only: false,
            },
            cancel,
        )?
        else {
            return Err("Invalid answer source reply".into());
        };
        Ok(prepared)
    })
}

fn collect(mut load: impl FnMut(usize) -> Result<Prepared, String>) -> Result<Document, String> {
    let mut document = Document {
        complete: true,
        ..Default::default()
    };
    let mut cursor = 0;
    let mut text_bytes = 0;
    let mut image_bytes = 0;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if document.pages >= 512 || Instant::now() >= deadline {
            break;
        }
        let mut prepared = load(cursor)?;
        document.pages += 1;
        document.complete &= prepared.coverage == "complete";
        text_bytes += prepared.text.len();
        if text_bytes > 512 * 1024 {
            break;
        }
        if prepared.modality == "text" && !prepared.text.trim().is_empty() {
            document.text.push(Source {
                id: document.text.len(),
                text: std::mem::take(&mut prepared.text),
                location: prepared.location.clone(),
            });
        }
        let next = prepared.next;
        match prepared.modality.as_str() {
            "image" => {
                let Some(data) = &prepared.data else {
                    break;
                };
                image_bytes += data.len();
                if document.images.len() == 16 || image_bytes > 16 * 1024 * 1024 {
                    break;
                }
                document.images.push(prepared);
            }
            "text" => {}
            _ => {
                document.complete = false;
                document.unsupported = Some(prepared.coverage);
            }
        }
        match next {
            Some(next) if next > cursor => cursor = next,
            Some(_) => return Err("Invalid answer extraction cursor".into()),
            None => return Ok(document),
        }
    }
    document.complete = false;
    Ok(document)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    evidence: Value,
    limited: bool,
}

pub(in super::super) fn answer_prepared(
    document: Document,
    question: &str,
    kind: AnswerType,
    cancel: &AtomicBool,
    infer: impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Answer, String> {
    if document.text.is_empty()
        && document.images.is_empty()
        && let Some(reason) = document.unsupported
    {
        return Ok(Answer::status(State::Unsupported, reason));
    }
    if !document.complete {
        return Ok(Answer::status(
            State::Unknown,
            "Source extraction is incomplete or exceeds 512 pages/sources/512 KiB text/16 images/16 MiB prepared image data/90 seconds.",
        ));
    }
    if document.images.is_empty() {
        if document.text.is_empty() {
            return Ok(Answer::status(
                State::Unsupported,
                "No usable document content was extracted.",
            ));
        }
        return context::answer_document(&document.text, true, question, kind, cancel, infer);
    }
    answer_images(document, question, kind, cancel, infer)
}

fn answer_images(
    document: Document,
    question: &str,
    kind: AnswerType,
    cancel: &AtomicBool,
    mut infer: impl FnMut(Value, Duration) -> Result<Value, String>,
) -> Result<Answer, String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    let selection = document.pages > 1;
    let mut selected_sources = Vec::new();
    for image in document.images {
        let Some(timeout) = context::remaining(deadline, cancel)? else {
            return Ok(Answer::status(
                State::Unknown,
                "Document answering exceeded its inference time budget.",
            ));
        };
        let request = visual::request(
            question,
            kind,
            image.data.as_deref().ok_or("Missing visual source data")?,
            selection,
        );
        let response = infer(request, timeout)?;
        if !selection {
            return visual::decode(response, kind, &image.location);
        }
        let selected: Selection =
            serde_json::from_value(response).map_err(|_| "Invalid visual selection reply")?;
        let observations = visual::sources(selected.evidence, &image.location)?;
        if selected.limited || observations.len() == 4 {
            return Ok(Answer::status(
                State::Unknown,
                format!(
                    "{}: visual evidence is unreadable, saturated or budget-limited.",
                    image.location
                ),
            ));
        }
        selected_sources.extend(observations);
    }
    context::answer_selected_document(
        &document.text,
        selected_sources,
        question,
        kind,
        cancel,
        deadline,
        infer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labelled_fast_path_requires_one_complete_text_source_without_images() {
        let source = Source {
            id: 0,
            text: "Date: 2021-11-18".into(),
            location: "page 1".into(),
        };
        let mut document = Document {
            text: vec![source.clone()],
            complete: true,
            ..Default::default()
        };
        assert!(document.complete_single_text().is_some());
        document.complete = false;
        assert!(document.complete_single_text().is_none());
        document.complete = true;
        document.text.push(source);
        assert!(document.complete_single_text().is_none());
        document.text.pop();
        document.images.push(Prepared::default());
        assert!(document.complete_single_text().is_none());
    }

    fn page(location: &str, next: Option<usize>) -> Prepared {
        Prepared {
            modality: "image".into(),
            data: Some(location.into()),
            location: location.into(),
            coverage: "complete".into(),
            next,
            ..Default::default()
        }
    }
    fn visual_item(observation: &str) -> Value {
        json!({"source":0,"region":[0,0,1000,1000],"observation":observation})
    }

    #[test]
    fn a_short_single_pdf_page_uses_full_visual_answering() {
        let document = collect(|_| {
            let mut page = page("page 1", None);
            page.text = "Rating: 18 points.".into();
            Ok(page)
        })
        .unwrap();
        assert!(document.complete && document.needs_vision());
        let answer = answer_prepared(document, "What rating?", AnswerType::Number,
            &AtomicBool::new(false), |request, _| {
                assert_eq!(request["op"], "visual_answer");
                Ok(json!({"applicability":"applicable","state":"value","value":"18","unit":"points",
                    "evidence":[visual_item("Rating: 18 points.")],"reason":"The page shows a rating."}))
            }).unwrap();
        assert_eq!(answer.label(), "18 points");
        assert!(answer.evidence[0].0.starts_with("page 1: region"));
    }

    #[test]
    fn competing_visual_pages_reconcile_with_both_page_locations() {
        let document = collect(|cursor| {
            Ok(page(
                &format!("page {}", cursor + 1),
                (cursor == 0).then_some(1),
            ))
        })
        .unwrap();
        let mut image_calls = 0;
        let answer = answer_prepared(document, "What rating?", AnswerType::Number,
            &AtomicBool::new(false), |request, _| {
                if request["op"] == "visual_evidence" {
                    image_calls += 1;
                    return Ok(json!({"limited":false,"evidence":[visual_item(
                        if request["data"] == "page 1" { "Rating: 18 points." } else { "Rating: 22 points." })]}));
                }
                assert_ne!(request["op"], "evidence", "Selected visual facts must not be selected a second time");
                assert_eq!(request["context"], "evidence");
                Ok(json!({"basis":"direct","applicability":"applicable","state":"ambiguous","value":null,"unit":null,
                    "evidence":[{"source":0,"quote":"Rating: 18 points."},{"source":1,"quote":"Rating: 22 points."}],
                    "reason":"Two competing ratings with no preference."}))
            }).unwrap();
        assert_eq!(image_calls, 2);
        assert!(matches!(answer.state, State::Ambiguous));
        assert!(answer.evidence[0].0.starts_with("page 1:"));
        assert!(answer.evidence[1].0.starts_with("page 2:"));
    }

    #[test]
    fn mixed_text_and_visual_pages_share_reconciliation_and_keep_provenance() {
        let document = collect(|cursor| {
            if cursor == 0 {
                Ok(Prepared {
                    modality: "text".into(),
                    text: "Rating: 14 points.".into(),
                    location: "page 1".into(),
                    next: Some(1),
                    coverage: "complete".into(),
                    ..Default::default()
                })
            } else {
                Ok(page("page 2", None))
            }
        })
        .unwrap();
        let mut text_windows = 0;
        let answer = answer_prepared(document, "What rating?", AnswerType::Number,
            &AtomicBool::new(false), |request, _| {
                if request["op"] == "visual_evidence" {
                    return Ok(json!({"limited":false,"evidence":[visual_item("Rating: 18 points.")]}));
                }
                if request["op"] == "evidence" {
                    text_windows += 1;
                    assert_eq!(request["sources"].as_array().unwrap().len(), 1);
                    assert_eq!(request["sources"][0]["location"], "page 1");
                    return Ok(json!({"limited":false,"evidence":[{"source":0,"quote":"Rating: 14 points."}]}));
                }
                assert_eq!(request["sources"][0]["id"], 0);
                assert_eq!(request["sources"][1]["id"], 1);
                Ok(json!({"basis":"direct","applicability":"applicable","state":"ambiguous","value":null,"unit":null,
                    "evidence":[{"source":0,"quote":"Rating: 18 points."},{"source":1,"quote":"Rating: 14 points."}],
                    "reason":"Two competing page ratings."}))
            }).unwrap();
        assert_eq!(text_windows, 1);
        assert!(matches!(answer.state, State::Ambiguous));
        assert!(
            answer.evidence[0].0.contains("page 2:")
                && answer.evidence[0].0.contains("model observation")
        );
        assert_eq!(answer.evidence[1].0, "page 1");
    }

    #[test]
    fn excessive_images_or_text_never_reach_inference() {
        for document in [
            collect(|cursor| Ok(page("page", Some(cursor + 1)))).unwrap(),
            collect(|_| {
                Ok(Prepared {
                    modality: "text".into(),
                    text: "x".repeat(512 * 1024 + 1),
                    coverage: "complete".into(),
                    ..Default::default()
                })
            })
            .unwrap(),
        ] {
            assert!(!document.complete);
            let answer = answer_prepared(
                document,
                "What rating?",
                AnswerType::Number,
                &AtomicBool::new(false),
                |_, _| panic!("Incomplete collection reached inference"),
            )
            .unwrap();
            assert!(matches!(answer.state, State::Unknown));
        }
        assert!(collect(|_| Ok(page("page", Some(0)))).is_err());
    }

    #[test]
    fn a_name_only_extractor_remains_unsupported_with_its_reason() {
        let document = collect(|_| {
            Ok(Prepared {
                coverage: "name-only: extractor unavailable".into(),
                ..Default::default()
            })
        })
        .unwrap();
        let answer = answer_prepared(
            document,
            "Question?",
            AnswerType::Text,
            &AtomicBool::new(false),
            |_, _| panic!("Unsupported file reached inference"),
        )
        .unwrap();
        assert!(matches!(answer.state, State::Unsupported));
        assert!(answer.reason.contains("extractor unavailable"));
    }

    #[test]
    fn saturated_visual_evidence_cannot_return_a_partial_answer() {
        let document = collect(|cursor| Ok(page("page", (cursor == 0).then_some(1)))).unwrap();
        let answer = answer_prepared(
            document,
            "What rating?",
            AnswerType::Number,
            &AtomicBool::new(false),
            |request, _| {
                assert_eq!(request["op"], "visual_evidence");
                Ok(json!({"limited":true,"evidence":[visual_item("Rating: 18 points.")]}))
            },
        )
        .unwrap();
        assert!(matches!(answer.state, State::Unknown));
    }
}
