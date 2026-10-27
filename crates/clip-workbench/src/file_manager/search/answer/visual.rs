//! Visual evidence identifies model-observed regions, never verified quotations.
use super::{Answer, AnswerType, Source};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    source: usize,
    region: [u16; 4],
    observation: String,
}

pub(in super::super) fn decode(
    mut reply: Value,
    kind: AnswerType,
    location: &str,
) -> Result<Answer, String> {
    let sources = sources(
        reply
            .get("evidence")
            .cloned()
            .ok_or("Missing visual evidence")?,
        location,
    )?;
    let evidence: Vec<_> = sources
        .iter()
        .map(|source| json!({"source":source.id,"quote":source.text}))
        .collect();
    reply["evidence"] = json!(evidence);
    Answer::decode(reply, kind, &sources, true)
}

pub(super) fn request(question: &str, kind: AnswerType, data: &str, selection: bool) -> Value {
    json!({"op":if selection {"visual_evidence"} else {"visual_answer"},
        "question":question,"type":kind,"data":data})
}

pub(super) fn sources(items: Value, location: &str) -> Result<Vec<Source>, String> {
    let items: Vec<Observation> =
        serde_json::from_value(items).map_err(|_| "Invalid visual evidence")?;
    if items.len() > 4 {
        return Err("Excessive visual evidence".into());
    }
    let mut sources = Vec::new();
    for (id, item) in items.into_iter().enumerate() {
        let [left, top, right, bottom] = item.region;
        if item.source != 0 || right > 1000 || bottom > 1000 || left >= right || top >= bottom {
            return Err("Invalid visual evidence region/source".into());
        }
        if item.observation.trim().is_empty() || item.observation.len() > 512 {
            return Err("Missing or excessive visual observation".into());
        }
        sources.push(Source {
            id,
            text: item.observation,
            location: format!(
                "{location}: region {left},{top}..{right},{bottom}/1000 (model observation)"
            ),
        });
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> Value {
        json!({"applicability":"applicable","state":"value","value":"false","unit":null,
            "evidence":[{"source":0,"region":[10,20,900,800],"observation":"A green rectangle."}],
            "reason":"The rectangle is green rather than blue."})
    }

    #[test]
    fn false_keeps_its_value_and_explicit_observational_provenance() {
        let answer = decode(reply(), AnswerType::Boolean, "image").unwrap();
        assert_eq!(answer.label(), "false");
        assert!(answer.evidence[0].0.contains("model observation"));
        assert!(answer.evidence[0].0.contains("10,20..900,800/1000"));
    }

    #[test]
    fn invalid_regions_unknown_sources_and_untyped_answers_are_rejected() {
        for evidence in [
            json!({"source":1,"region":[0,0,1000,1000],"observation":"Other image"}),
            json!({"source":0,"region":[900,0,800,1000],"observation":"Reversed"}),
            json!({"source":0,"region":[0,0,1001,1000],"observation":"Outside"}),
            json!({"source":0,"region":[0,0,1000,1000],"observation":""}),
        ] {
            let mut response = reply();
            response["evidence"] = json!([evidence]);
            assert!(decode(response, AnswerType::Boolean, "image").is_err());
        }
        let mut response = reply();
        response["value"] = json!("yes");
        assert!(decode(response, AnswerType::Boolean, "image").is_err());
    }
}
