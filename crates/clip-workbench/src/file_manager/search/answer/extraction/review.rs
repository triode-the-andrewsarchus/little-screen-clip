//! Semantic review is separate from literal extraction and its OS boundary.
use super::super::{Answer, Source, State};
use crate::file_manager::search::query::AnswerType;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) struct Context<'a> {
    pub question: &'a str,
    pub kind: AnswerType,
    pub header: &'a str,
    pub document_location: Option<&'a str>,
}

impl Context<'_> {
    pub fn independently_agrees(
        &self,
        candidate: &Answer,
        infer: &mut impl FnMut(Value) -> Result<Value, String>,
    ) -> Result<bool, String> {
        let sources = [Source {
            id: 0,
            text: self.header.into(),
            location: self.document_location.unwrap_or("front matter").into(),
        }];
        // This selected evidence cannot prove body-wide absence/non-applicability.
        // Do NOT include the proposed value/path: avoid candidate anchoring.
        let reply = infer(
            json!({"op":"answer", "question":self.question, "type":self.kind,
            "sources":sources, "complete":true, "context":"evidence"}),
        )?;
        if reply["basis"] != "direct" {
            return Ok(false);
        }
        let independent = Answer::decode(reply, self.kind, &sources, true)?;
        Ok(match (&candidate.state, independent.state) {
            (State::Value(a, raw_a), State::Value(b, raw_b)) => {
                std::mem::discriminant(a) == std::mem::discriminant(&b)
                    && if self.kind == AnswerType::Text {
                        // Sorting folds case/numbers; literal identifiers must
                        // not inherit that looser notion of equality.
                        raw_a == &raw_b
                    } else {
                        a.compare(&b, false).is_eq()
                    }
            }
            _ => false,
        })
    }

    pub fn request(&self, path: &[String], value: &str, may_reuse: bool) -> Value {
        let mut request = json!({
            "op":"extraction_review", "question":self.question, "type":self.kind,
            "sources":[{"id":0,"text":self.header,"location":self.document_location.unwrap_or("front matter")}],
            "complete":false, "may_reuse":may_reuse,
            "candidate":{"path":path,"value":value},
        });
        if self.document_location.is_some() {
            request["layout"] = json!("labelled_document");
        }
        request
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Decision {
    Reuse,
    Accept,
    Escalate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Review {
    pub scope: Scope,
    pub reason: String,
    pub decision: Decision,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scope {
    Literal,
    PerFile,
    Document,
}

pub(super) fn check(
    request: Value,
    may_reuse: bool,
    infer: &mut impl FnMut(Value) -> Result<Value, String>,
) -> Result<Review, String> {
    let review: Review =
        serde_json::from_value(infer(request)?).map_err(|_| "Invalid candidate review")?;
    if review.reason.trim().is_empty() || review.reason.len() > 512 {
        return Err("Invalid candidate review reason".into());
    }
    if review.decision == Decision::Reuse && !may_reuse {
        return Err("A per-file review cannot authorize later reuse".into());
    }
    if (review.scope != Scope::Literal && review.decision == Decision::Reuse)
        || (review.scope == Scope::Document && review.decision != Decision::Escalate)
    {
        return Err("Candidate review contradicts its question scope".into());
    }
    Ok(review)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_agreement_preserves_case_instead_of_using_sort_equivalence() {
        let context = Context {
            question: "What is the exact identifier?",
            kind: AnswerType::Text,
            header: "identifier: Foo\nalternative: foo\n",
            document_location: None,
        };
        let candidate = Answer::status(
            State::Value(super::super::super::Value::Text("foo".into()), "Foo".into()),
            "Literal field",
        );
        assert!(!context.independently_agrees(&candidate, &mut |_| Ok(json!({
            "basis":"direct", "applicability":"applicable", "state":"value", "value":"foo", "unit":null,
            "reason":"The alternative has this spelling.", "evidence":[{"source":0,"quote":"alternative: foo"}],
        }))).unwrap());
    }

    #[test]
    fn per_file_review_cannot_upgrade_itself_and_invalid_decisions_fail_closed() {
        for value in [
            json!({"scope":"literal", "reason":"Looks fine", "decision":"reuse"}),
            json!({"scope":"literal", "reason":"", "decision":"accept"}),
            json!({"scope":"literal", "reason":"Looks fine", "decision":"accept", "command":"true"}),
            json!({"scope":"per_file", "reason":"A judgment", "decision":"reuse"}),
            json!({"scope":"document", "reason":"Needs context", "decision":"accept"}),
        ] {
            assert!(check(json!({}), false, &mut |_| Ok(value.clone())).is_err());
        }
    }
}
