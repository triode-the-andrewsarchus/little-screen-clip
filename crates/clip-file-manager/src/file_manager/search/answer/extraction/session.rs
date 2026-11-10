//! Column-owned plans contain field paths and schema hashes, never answers/content.
use super::super::{Answer, Applicability, State, Value};
use super::{
    Attempt, Plan, Request, Response, call,
    review::{self, Decision},
};
use crate::file_manager::search::query::AnswerType;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(in crate::file_manager::search) type SharedPlans =
    Arc<Mutex<HashMap<String, Option<CachedPlan>>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    Unreviewed,
    Literal,
    EachFile,
}

#[derive(Clone)]
pub(in crate::file_manager::search) struct CachedPlan {
    plan: Plan,
    policy: Policy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    path: Option<Vec<String>>,
}

pub(in crate::file_manager::search) enum Input<'a> {
    Header(&'a str),
    Document(&'a super::super::Source),
}

pub(in crate::file_manager::search) fn try_answer(
    input: Input<'_>,
    question: &str,
    kind: AnswerType,
    plans: &SharedPlans,
    cancel: &AtomicBool,
    mut infer: impl FnMut(serde_json::Value) -> Result<serde_json::Value, String>,
) -> Result<Attempt, String> {
    let (source, document_location) = match input {
        Input::Header(source) if literal_question_field(question).is_some() => (source, None),
        Input::Header(_) => return Ok(Attempt::Skip),
        Input::Document(source) if source.text.len() <= super::lines::MAX_DOCUMENT => {
            (source.text.as_str(), Some(source.location.as_str()))
        }
        Input::Document(_) => return Ok(Attempt::Skip),
    };
    let request = if document_location.is_some() {
        Request::InspectDocument {
            source: source.into(),
        }
    } else {
        Request::Inspect {
            source: source.into(),
        }
    };
    let Response::Header {
        header: Some(header),
    } = call(&request, cancel)?
    else {
        return Ok(Attempt::Skip);
    };
    // Whole-document field extraction is safe to reuse only when the
    // question explicitly requests a named field without subject conditions.
    // Everything else goes through ordinary full-document answering, without
    // anchoring the model on a possibly irrelevant field value.
    if document_location.is_some()
        && !header
            .fields
            .iter()
            .any(|field| explicitly_requests_field(question, &field.path))
    {
        return Ok(Attempt::Skip);
    }
    let plan = select_plan(&header, question, kind, plans, cancel, &mut infer)?;
    let Some(plan) = plan else {
        return Ok(Attempt::Skip);
    };
    if let Plan::LabelledField { path, .. } = &plan.plan
        && !explicitly_requests_field(question, path)
    {
        return Ok(Attempt::Skip);
    }
    let response = call(
        &Request::Execute {
            source: source.into(),
            plan: plan.plan.clone(),
        },
        cancel,
    )?;
    let Some(mut answer) = field_answer(source, kind, response)? else {
        return Ok(Attempt::Skip);
    };
    if let Some(location) = document_location {
        answer.reason =
            "Extracted a literal typed field from the complete bounded text document.".into();
        for (field_location, _) in &mut answer.evidence {
            *field_location = format!("{location}: {field_location}");
        }
    }
    review_capture(
        &review::Context {
            question,
            kind,
            header: &header.raw,
            document_location,
        },
        plans,
        &plan,
        answer,
        cancel,
        &mut infer,
    )
}

fn field_answer(
    source: &str,
    kind: AnswerType,
    response: Response,
) -> Result<Option<Answer>, String> {
    let Response::Field {
        value: Some(field),
        evidence,
        location,
    } = response
    else {
        return Ok(None);
    };
    if evidence.is_empty() || evidence.len() > 4096 || !source.contains(&evidence) {
        return Err("Structured evidence does not match the source".into());
    }
    if field.kind == "unsupported" {
        return Ok(None);
    }
    let Ok(value) = Value::parse(&field.value, None, kind) else {
        return Ok(None);
    };
    Ok(Some(Answer {
        applicability: Applicability::Applicable,
        state: State::Value(value, field.value),
        reason: "Extracted a typed field with this column's metadata plan. The complete front matter was parsed; the document body was not examined.".into(),
        evidence: vec![(location, evidence)],
    }))
}

fn select_plan(
    header: &super::frontmatter::Header,
    question: &str,
    kind: AnswerType,
    plans: &SharedPlans,
    cancel: &AtomicBool,
    infer: &mut impl FnMut(serde_json::Value) -> Result<serde_json::Value, String>,
) -> Result<Option<CachedPlan>, String> {
    let document = matches!(
        header.format.as_str(),
        "labelled text document" | "literal document fields"
    );
    let (cached, may_learn) = {
        let cache = plans.lock().map_err(|_| "Column plans unavailable")?;
        (cache.get(&header.schema).cloned(), cache.len() < 8)
    };
    tracing::debug!(target: "little_clip_search_timing",
        cache_hit = cached.is_some(), may_learn, "column metadata plan");
    match cached {
        Some(plan) => Ok(plan),
        None if may_learn => {
            // Show every field, including incompatible values: hiding a broken
            // publishDate could make a different date look unambiguous.
            let fields = &header.fields;
            let plan = if !fields.iter().any(|field| {
                field.kind != "unsupported" && Value::parse(&field.value, None, kind).is_ok()
            }) {
                None
            } else {
                let mut request = serde_json::json!({
                    "op":"extraction_plan", "question":question, "type":kind, "fields":fields,
                });
                if document {
                    request["layout"] = serde_json::json!("labelled_document");
                }
                let selection: Selection =
                    serde_json::from_value(infer(request)?).map_err(|_| "Invalid metadata plan")?;
                selection
                    .path
                    .filter(|path| fields.iter().any(|field| field.path == *path))
                    .map(|path| CachedPlan {
                        plan: if document {
                            Plan::LabelledField {
                                schema: header.schema.clone(),
                                path,
                            }
                        } else {
                            Plan::FrontmatterField {
                                schema: header.schema.clone(),
                                path,
                            }
                        },
                        policy: Policy::Unreviewed,
                    })
            };
            let mut cache = plans.lock().map_err(|_| "Column plans unavailable")?;
            // Check under the same lock as invalidation: a cancelled in-flight
            // inference cannot repopulate a cache after the UI cleared it.
            if cancel.load(Ordering::Relaxed) {
                return Err("Answer cancelled".into());
            }
            cache.insert(header.schema.clone(), plan.clone());
            Ok(plan)
        }
        None => Ok(None),
    }
}

fn review_capture(
    context: &review::Context<'_>,
    plans: &SharedPlans,
    plan: &CachedPlan,
    mut answer: Answer,
    cancel: &AtomicBool,
    infer: &mut impl FnMut(serde_json::Value) -> Result<serde_json::Value, String>,
) -> Result<Attempt, String> {
    if plan.policy == Policy::Literal {
        answer
            .reason
            .push_str(" Reused a semantically reviewed literal-field mapping.");
        return Ok(Attempt::Answer(answer));
    }
    let (schema, path) = match &plan.plan {
        Plan::FrontmatterField { schema, path } | Plan::LabelledField { schema, path } => {
            (schema, path)
        }
    };
    if !context.independently_agrees(&answer, infer)? {
        set_policy(plans, schema, plan, Decision::Escalate, cancel)?;
        tracing::debug!(target: "little_clip_search_timing", action = "escalate", "column independent review");
        return Ok(Attempt::Escalate("An independent source answer did not agree with the extracted candidate. Recheck the question, negation and evidence in the document.".into()));
    }
    let State::Value(_, ref raw) = answer.state else {
        return Err("Candidate review requires a typed value".into());
    };
    // A shared layout does not prove that a question about a particular kind
    // of document applies to every file. For whole documents, bypass future
    // semantic reviews only for an unqualified request for this exact field.
    let direct_field = matches!(&plan.plan, Plan::FrontmatterField { .. })
        || explicitly_requests_field(context.question, path);
    let may_reuse = plan.policy == Policy::Unreviewed && direct_field;
    let reviewed = review::check(context.request(path, raw, may_reuse), may_reuse, infer)?;
    set_policy(plans, schema, plan, reviewed.decision, cancel)?;
    let action = match reviewed.decision {
        Decision::Reuse => "reuse",
        Decision::Accept => "accept",
        Decision::Escalate => "escalate",
    };
    tracing::debug!(target: "little_clip_search_timing", action, "column candidate review");
    if reviewed.decision == Decision::Escalate {
        return Ok(Attempt::Escalate(reviewed.reason));
    }
    answer.reason.push_str(&format!(
        " Candidate reviewed against the question: {}",
        reviewed.reason
    ));
    Ok(Attempt::Answer(answer))
}

fn normalized_question(question: &str) -> String {
    let question = question.trim().strip_suffix('?').unwrap_or(question.trim());
    question
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn explicitly_requests_field(question: &str, path: &[String]) -> bool {
    let [field] = path else { return false };
    let question = normalized_question(question);
    let field = field
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    [
        format!("what is the {field}"),
        format!("what is {field}"),
        format!("what is the stored {field} field"),
        format!("what is the value of the {field} field"),
    ]
    .contains(&question)
}

pub(super) fn literal_question_field(question: &str) -> Option<String> {
    let question = normalized_question(question);
    let phrase = question
        .strip_prefix("what is the ")
        .or_else(|| question.strip_prefix("what is "))?;
    // This controls optimization eligibility, not accepted question syntax.
    // Relations, conditions and propositions need complete-source answering.
    let words: Vec<_> = phrase.split_whitespace().collect();
    let eligible = !words.is_empty()
        && words.len() <= 6
        && phrase
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, ' ' | '_' | '-'))
        && !words.iter().any(|word| {
            matches!(
                *word,
                "if" | "when"
                    | "where"
                    | "why"
                    | "how"
                    | "this"
                    | "that"
                    | "these"
                    | "those"
                    | "on"
                    | "in"
                    | "from"
                    | "for"
                    | "of"
                    | "with"
                    | "without"
                    | "than"
                    | "before"
                    | "after"
                    | "not"
                    | "actual"
                    | "really"
            )
        });
    eligible.then(|| {
        let field = phrase.strip_prefix("stored ").unwrap_or(phrase);
        field.strip_suffix(" field").unwrap_or(field).to_owned()
    })
}

fn set_policy(
    plans: &SharedPlans,
    schema: &str,
    expected: &CachedPlan,
    decision: Decision,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut cache = plans.lock().map_err(|_| "Column plans unavailable")?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Answer cancelled".into());
    }
    let Some(Some(current)) = cache.get_mut(schema) else {
        return Err("Column plan was invalidated during review".into());
    };
    if current.plan != expected.plan || current.policy != expected.policy {
        return Err("Column plan changed during review".into());
    }
    current.policy = if decision == Decision::Reuse {
        Policy::Literal
    } else {
        Policy::EachFile
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_search_labels_share_field_question_eligibility() {
        for question in [
            "What is the quantity?",
            "What is the stored quantity field?",
        ] {
            assert_eq!(
                literal_question_field(question).as_deref(),
                Some("quantity")
            );
        }
        assert!(literal_question_field("What is the quantity on this invoice?").is_none());
    }
    use crate::file_manager::search::answer::extraction::{execute, frontmatter};
    use serde_json::json;

    fn execute_candidate(source: &str, kind: AnswerType, plan: &CachedPlan) -> Answer {
        let response = execute(Request::Execute {
            source: source.into(),
            plan: plan.plan.clone(),
        })
        .unwrap();
        field_answer(source, kind, response).unwrap().unwrap()
    }

    fn capture(
        source: &str,
        question: &str,
        kind: AnswerType,
        field: &str,
        plans: &SharedPlans,
        cancel: &AtomicBool,
    ) -> (frontmatter::Header, CachedPlan, Answer) {
        let header = frontmatter::inspect(source).unwrap().unwrap();
        let plan = select_plan(&header, question, kind, plans, cancel, &mut |_| {
            Ok(json!({"path":[field]}))
        })
        .unwrap()
        .unwrap();
        let candidate = execute_candidate(source, kind, &plan);
        (header, plan, candidate)
    }

    #[test]
    fn same_schema_reuses_only_the_path_and_executes_each_files_current_value() {
        let plans = SharedPlans::default();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        for date in ["2021-11-18", "2025-01-04"] {
            let source = format!("---\ndate: {date}\ndraft: false\n---\nBody has 2030-01-01.");
            let header = frontmatter::inspect(&source).unwrap().unwrap();
            let plan = select_plan(
                &header,
                "Publication date?",
                AnswerType::Date,
                &plans,
                &cancel,
                &mut |_| {
                    calls += 1;
                    Ok(json!({"path":["date"]}))
                },
            )
            .unwrap()
            .unwrap();
            let Response::Field {
                value: Some(value),
                evidence,
                ..
            } = execute(Request::Execute {
                source,
                plan: plan.plan,
            })
            .unwrap()
            else {
                panic!()
            };
            assert_eq!(value.value, date);
            assert!(!evidence.contains("2030"));
        }
        assert_eq!(calls, 1);
        assert_eq!(plans.lock().unwrap().len(), 1);
    }

    #[test]
    fn labelled_documents_reuse_reviewed_fields_with_current_value_and_source_evidence() {
        let plans = SharedPlans::default();
        let cancel = AtomicBool::new(false);
        let mut plan_calls = 0;
        let mut review_calls = 0;
        for date in ["2021-11-18", "2025-01-04"] {
            let source = format!("Account: Savings\nStatement date: {date}\nBalance: 10.50\n");
            let header = super::super::lines::inspect(&source).unwrap().unwrap();
            let plan = select_plan(
                &header,
                "What is the statement date?",
                AnswerType::Date,
                &plans,
                &cancel,
                &mut |request| {
                    plan_calls += 1;
                    assert_eq!(request["layout"], "labelled_document");
                    Ok(json!({"path":["Statement date"]}))
                },
            )
            .unwrap()
            .unwrap();
            let candidate = execute_candidate(&source, AnswerType::Date, &plan);
            let result = review_capture(&review::Context { question:"What is the statement date?", kind:AnswerType::Date,
                header:&header.raw, document_location:Some("page 1") }, &plans, &plan, candidate, &cancel,
                &mut |request| {
                    review_calls += 1;
                    if request["op"] == "answer" {
                        assert!(request.get("candidate").is_none());
                        return Ok(json!({"basis":"direct", "applicability":"applicable", "state":"value", "value":date,
                            "unit":null, "reason":"The document states the date.",
                            "evidence":[{"source":0,"quote":format!("Statement date: {date}")}]}));
                    }
                    assert_eq!(request["layout"], "labelled_document");
                    assert_eq!(request["sources"][0]["location"], "page 1");
                    Ok(json!({"scope":"literal", "decision":"reuse", "reason":"Direct stored date."}))
                }).unwrap();
            let Attempt::Answer(answer) = result else {
                panic!("Expected reviewed value")
            };
            assert_eq!(answer.label(), date);
            assert!(answer.evidence[0].1.contains(date));
        }
        assert_eq!(plan_calls, 1);
        assert_eq!(review_calls, 2);
    }

    #[test]
    fn labelled_false_and_zero_are_values_and_type_mismatches_fall_back() {
        let source = "Enabled: false\nCount: 0\n";
        let header = super::super::lines::inspect(source).unwrap().unwrap();
        let capture = |field: &str, kind| {
            let response = execute(Request::Execute {
                source: source.into(),
                plan: Plan::LabelledField {
                    schema: header.schema.clone(),
                    path: vec![field.into()],
                },
            })
            .unwrap();
            field_answer(source, kind, response).unwrap()
        };
        assert_eq!(
            capture("Enabled", AnswerType::Boolean).unwrap().label(),
            "false"
        );
        assert_eq!(capture("Count", AnswerType::Number).unwrap().label(), "0");
        assert!(capture("Enabled", AnswerType::Date).is_none());
    }

    #[test]
    fn only_unqualified_exact_field_questions_allow_document_rule_reuse() {
        let path = vec!["Recorded date".into()];
        for question in [
            "What is the recorded date?",
            "What is the stored recorded date field?",
            "What is the value of the recorded date field?",
        ] {
            assert!(explicitly_requests_field(question, &path), "{question}");
        }
        for question in [
            "What is the recorded date on this statement?",
            "What is NOT the recorded date?",
            "Does this rental agreement permit pets?",
            "What is the stored recorded date field if valid?",
        ] {
            assert!(!explicitly_requests_field(question, &path), "{question}");
        }
    }

    #[test]
    fn header_shortcut_declines_contextual_and_interpretive_questions() {
        for question in [
            "What is the publication date?",
            "What is the stored positive flag?",
            "What is the last modified date?",
        ] {
            assert!(literal_question_field(question).is_some(), "{question}");
        }
        for question in [
            "Does this rental agreement permit pets?",
            "How old is the child?",
            "Does the summary sound positive?",
            "What is the date on this statement?",
            "What is the actual publication date?",
        ] {
            assert!(literal_question_field(question).is_none(), "{question}");
        }
    }

    #[test]
    fn same_layout_cannot_reuse_contextual_applicability_between_documents() {
        let plans = SharedPlans::default();
        let cancel = AtomicBool::new(false);
        let question = "Does this rental agreement permit pets?";
        let mut plans_generated = 0;
        let mut reviews = 0;
        for (document, applicable) in [("Rental agreement", true), ("Recipe", false)] {
            let source = format!("Document: {document}\nPets permitted: true\nTenant: Alice\n");
            let header = super::super::lines::inspect(&source).unwrap().unwrap();
            let plan = select_plan(
                &header,
                question,
                AnswerType::Boolean,
                &plans,
                &cancel,
                &mut |_| {
                    plans_generated += 1;
                    Ok(json!({"path":["Pets permitted"]}))
                },
            )
            .unwrap()
            .unwrap();
            let candidate = execute_candidate(&source, AnswerType::Boolean, &plan);
            let result = review_capture(&review::Context { question, kind:AnswerType::Boolean, header:&header.raw,
                document_location:Some("text") }, &plans, &plan, candidate, &cancel, &mut |request| {
                if request["op"] == "answer" {
                    return Ok(if applicable {
                        json!({"basis":"direct", "applicability":"applicable", "state":"value", "value":"true", "unit":null,
                            "reason":"The agreement permits pets.", "evidence":[{"source":0,"quote":"Pets permitted: true"}]})
                    } else {
                        json!({"basis":"direct", "applicability":"not_applicable", "state":"not_applicable", "value":null, "unit":null,
                            "reason":"This is a recipe.", "evidence":[{"source":0,"quote":"Document: Recipe"}]})
                    });
                }
                reviews += 1;
                assert_eq!(request["may_reuse"], false);
                Ok(json!({"scope":"per_file", "decision":"accept", "reason":"This file is an agreement."}))
            }).unwrap();
            assert_eq!(matches!(result, Attempt::Answer(_)), applicable);
            assert_eq!(matches!(result, Attempt::Escalate(_)), !applicable);
        }
        assert_eq!(plans_generated, 1);
        assert_eq!(reviews, 1);
    }

    #[test]
    fn unsupported_captures_fall_back_while_empty_literal_text_remains_a_value() {
        for (source, field, expected_value) in [
            (
                "---\npublishDate: null\ndate: 2021-11-18\n---\n",
                "publishDate",
                false,
            ),
            (
                "+++\namount = 12.1234567890123456789\n+++\n",
                "amount",
                false,
            ),
            ("---\ntitle: ''\n---\n", "title", true),
        ] {
            let header = frontmatter::inspect(source).unwrap().unwrap();
            let response = execute(Request::Execute {
                source: source.into(),
                plan: Plan::FrontmatterField {
                    schema: header.schema,
                    path: vec![field.into()],
                },
            })
            .unwrap();
            let answer = field_answer(source, AnswerType::Text, response).unwrap();
            assert_eq!(answer.is_some(), expected_value);
            if let Some(answer) = answer {
                assert!(matches!(answer.state, State::Value(_, ref value) if value.is_empty()));
            }
        }
    }

    #[test]
    fn independent_disagreement_escalates_without_asking_to_approve_the_wrong_boolean() {
        let plans = SharedPlans::default();
        let cancel = AtomicBool::new(false);
        for draft in [false, true] {
            let source = format!("---\ndraft: {draft}\n---\n");
            let (header, plan, candidate) = capture(
                &source,
                "Is it NOT a draft?",
                AnswerType::Boolean,
                "draft",
                &plans,
                &cancel,
            );
            let result = review_capture(&review::Context { question:"Is it NOT a draft?", kind:AnswerType::Boolean, header:&header.raw, document_location:None },
                &plans, &plan, candidate, &cancel, &mut |request| {
                    assert_eq!(request["op"], "answer", "A disagreement must not reach an anchoring approval prompt");
                    assert!(request.get("candidate").is_none());
                    Ok(json!({"basis":"direct", "reason":"The flag is inverted by the question.", "applicability":"applicable",
                        "state":"value", "value":(!draft).to_string(), "unit":null,
                        "evidence":[{"source":0,"quote":format!("draft: {draft}")}]}))
                }).unwrap();
            assert!(matches!(result, Attempt::Escalate(_)));
            assert!(
                plans
                    .lock()
                    .unwrap()
                    .values()
                    .flatten()
                    .all(|plan| plan.policy == Policy::EachFile)
            );
        }
    }

    #[test]
    fn literal_reviews_reuse_but_interpretation_and_escalation_review_every_file() {
        for decision in ["reuse", "accept", "escalate"] {
            let plans = SharedPlans::default();
            let cancel = AtomicBool::new(false);
            let mut reviews = 0;
            let mut blind_calls = 0;
            for date in ["2021-11-18", "2025-01-04"] {
                let source = format!("---\ndate: {date}\n---\n");
                let (header, plan, candidate) = capture(
                    &source,
                    "Publication date?",
                    AnswerType::Date,
                    "date",
                    &plans,
                    &cancel,
                );
                let result = review_capture(&review::Context { question:"Publication date?", kind: AnswerType::Date, header:&header.raw, document_location:None }, &plans, &plan, candidate, &cancel,
                    &mut |request| {
                        if request["op"] == "answer" {
                            blind_calls += 1;
                            assert!(request.get("candidate").is_none());
                            return Ok(json!({"basis":"direct", "applicability":"applicable", "state":"value", "value":date, "unit":null,
                                "reason":"The source contains the date.", "evidence":[{"source":0,"quote":date}]}));
                        }
                        reviews += 1;
                        assert_eq!(request["op"], "extraction_review");
                        assert_eq!(request["candidate"]["value"], date);
                        assert!(request["sources"][0]["text"].as_str().unwrap().contains(date));
                        assert_eq!(request["may_reuse"], reviews == 1);
                        Ok(json!({"scope":if decision == "reuse" { "literal" } else { "per_file" }, "decision":decision,"reason":"Reviewed the actual candidate and question."}))
                    }).unwrap();
                assert_eq!(
                    matches!(result, Attempt::Escalate(_)),
                    decision == "escalate"
                );
            }
            assert_eq!(reviews, if decision == "reuse" { 1 } else { 2 });
            assert_eq!(blind_calls, reviews);
        }
    }

    #[test]
    fn cancellation_during_inference_cannot_restore_invalidated_plans() {
        let plans = SharedPlans::default();
        let cancel = AtomicBool::new(false);
        let header = frontmatter::inspect("---\ndate: 2021-11-18\n---\n")
            .unwrap()
            .unwrap();
        let result = select_plan(
            &header,
            "Publication date?",
            AnswerType::Date,
            &plans,
            &cancel,
            &mut |_| {
                cancel.store(true, Ordering::Relaxed);
                plans.lock().unwrap().clear();
                Ok(json!({"path":["date"]}))
            },
        );
        assert!(result.is_err());
        assert!(plans.lock().unwrap().is_empty());
        cancel.store(false, Ordering::Relaxed);
        let (header, plan, _) = capture(
            "---\ndate: 2021-11-18\n---\n",
            "Publication date?",
            AnswerType::Date,
            "date",
            &plans,
            &cancel,
        );
        plans.lock().unwrap().clear();
        assert!(set_policy(&plans, &header.schema, &plan, Decision::Reuse, &cancel).is_err());
        assert!(plans.lock().unwrap().is_empty());
    }

    #[test]
    fn incompatible_fields_remain_visible_and_declined_plans_are_bounded() {
        let plans = SharedPlans::default();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        for index in 0..10 {
            let source =
                format!("---\ndate: 2021-11-18\npublishDate: unknown\nextra{index}: false\n---\n");
            let header = frontmatter::inspect(&source).unwrap().unwrap();
            let mut infer = |request: serde_json::Value| {
                calls += 1;
                assert!(
                    request["fields"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|field| field["path"] == json!(["publishDate"]))
                );
                Ok(json!({"path":null}))
            };
            for _ in 0..2 {
                assert!(
                    select_plan(
                        &header,
                        "Publication date?",
                        AnswerType::Date,
                        &plans,
                        &cancel,
                        &mut infer
                    )
                    .unwrap()
                    .is_none()
                );
            }
        }
        assert_eq!(calls, 8);
        assert_eq!(plans.lock().unwrap().len(), 8);
    }
}
