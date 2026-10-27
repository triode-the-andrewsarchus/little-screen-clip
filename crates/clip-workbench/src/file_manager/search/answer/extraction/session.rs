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

pub(in crate::file_manager::search) fn try_answer(
    source: &str,
    question: &str,
    kind: AnswerType,
    plans: &SharedPlans,
    cancel: &AtomicBool,
    mut infer: impl FnMut(serde_json::Value) -> Result<serde_json::Value, String>,
) -> Result<Attempt, String> {
    let Response::Header {
        header: Some(header),
    } = call(
        &Request::Inspect {
            source: source.into(),
        },
        cancel,
    )?
    else {
        return Ok(Attempt::Skip);
    };
    let plan = select_plan(&header, question, kind, plans, cancel, &mut infer)?;
    let Some(plan) = plan else {
        return Ok(Attempt::Skip);
    };
    let response = call(
        &Request::Execute {
            source: source.into(),
            plan: plan.plan.clone(),
        },
        cancel,
    )?;
    let Some(answer) = field_answer(source, kind, response)? else {
        return Ok(Attempt::Skip);
    };
    review_capture(
        &review::Context {
            question,
            kind,
            header: &header.raw,
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
                let selection: Selection = serde_json::from_value(infer(serde_json::json!({
                    "op":"extraction_plan", "question":question, "type":kind, "fields":fields,
                }))?)
                .map_err(|_| "Invalid metadata plan")?;
                selection
                    .path
                    .filter(|path| fields.iter().any(|field| field.path == *path))
                    .map(|path| CachedPlan {
                        plan: Plan::FrontmatterField {
                            schema: header.schema.clone(),
                            path,
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
    let Plan::FrontmatterField { schema, path } = &plan.plan;
    if !context.independently_agrees(&answer, infer)? {
        set_policy(plans, schema, plan, Decision::Escalate, cancel)?;
        tracing::debug!(target: "little_clip_search_timing", action = "escalate", "column independent review");
        return Ok(Attempt::Escalate("An independent source answer did not agree with the extracted candidate. Recheck the question, negation and evidence in the document.".into()));
    }
    let State::Value(_, ref raw) = answer.state else {
        return Err("Candidate review requires a typed value".into());
    };
    let may_reuse = plan.policy == Policy::Unreviewed;
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
    use crate::file_manager::search::answer::extraction::{execute, frontmatter};
    use serde_json::json;

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
        let response = execute(Request::Execute {
            source: source.into(),
            plan: plan.plan.clone(),
        })
        .unwrap();
        let candidate = field_answer(source, kind, response).unwrap().unwrap();
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
            let result = review_capture(&review::Context { question:"Is it NOT a draft?", kind:AnswerType::Boolean, header:&header.raw },
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
                let result = review_capture(&review::Context { question:"Publication date?", kind: AnswerType::Date, header:&header.raw }, &plans, &plan, candidate, &cancel,
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
