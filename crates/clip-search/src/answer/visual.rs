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

pub(crate) fn decode(
    mut reply: Value,
    kind: AnswerType,
    location: &str,
    text_sources: &[Source],
) -> Result<Answer, String> {
    let items: Vec<Value> = serde_json::from_value(
        reply
            .get("evidence")
            .cloned()
            .ok_or("Missing visual evidence")?,
    )
    .map_err(|_| "Invalid mixed evidence")?;
    if items.len() > 4 {
        return Err("Excessive mixed evidence".into());
    }
    let mut sources = Vec::new();
    for item in items {
        if item.get("quote").is_some() {
            let quote: super::Evidence =
                serde_json::from_value(item).map_err(|_| "Invalid text evidence")?;
            if quote.source == 0 {
                return Err("Text evidence cannot reference image source 0".into());
            }
            for (location, text) in super::evidence(&[quote], text_sources)? {
                sources.push(Source {
                    id: 0,
                    text,
                    location,
                });
            }
        } else {
            sources.extend(self::sources(json!([item]), location)?);
        }
    }
    for (id, source) in sources.iter_mut().enumerate() {
        source.id = id;
    }
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
    fn mixed_text_quotations_are_exact_and_cannot_impersonate_image_regions() {
        let text = [Source {
            id: 1,
            text: "Shape: rectangle.".into(),
            location: "page 1".into(),
        }];
        let mut response = reply();
        response["evidence"]
            .as_array_mut()
            .unwrap()
            .push(json!({"source":1,"quote":"Shape: rectangle."}));
        let answer = decode(response.clone(), AnswerType::Boolean, "page 1", &text).unwrap();
        assert!(answer.evidence[0].0.contains("model observation"));
        assert_eq!(answer.evidence[1].0, "page 1");
        for item in [
            json!({"source":0,"quote":"Shape: rectangle."}),
            json!({"source":2,"quote":"Shape: rectangle."}),
            json!({"source":1,"quote":"Shape: circle."}),
            json!({"source":1,"region":[0,0,1000,1000],"observation":"Text is not pixels."}),
        ] {
            response["evidence"] = json!([item]);
            assert!(decode(response.clone(), AnswerType::Boolean, "page 1", &text).is_err());
        }
    }

    #[test]
    fn false_keeps_its_value_and_explicit_observational_provenance() {
        let answer = decode(reply(), AnswerType::Boolean, "image", &[]).unwrap();
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
            assert!(decode(response, AnswerType::Boolean, "image", &[]).is_err());
        }
        let mut response = reply();
        response["value"] = json!("yes");
        assert!(decode(response, AnswerType::Boolean, "image", &[]).is_err());
    }
}
