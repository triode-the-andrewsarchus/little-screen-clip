//! The idempotence property: spelling the normalized rendering view back as a
//! scene document and normalizing it again.

use std::fmt::Write as _;

use crate::normalization::{
    NormalizationOutcome, NormalizedElement, NormalizedElementKind, NormalizedImageStatus,
    NormalizedRoundness,
};
use crate::{OutputFormat, SceneDocument};

use super::{normalize, Normalized, ObservedEvent};

/// Normalization is idempotent on its own rendering view: projecting the
/// normalized view for one format back into a scene document and normalizing
/// it again for that format produces no repair diagnostics and the same
/// normalized values.
///
/// One repair has no scene-document spelling and is left out of the
/// projection: a linear or freedraw element whose malformed points were
/// replaced by the known-element placeholder. Link sanitization is the pinned
/// reference sanitizer, which decodes one layer of HTML entities per pass and
/// is deliberately not a fixpoint, so links and the link-derived embeddable
/// label are compared through determinism rather than idempotence.
pub(super) fn assert_normalization_is_idempotent(
    scene: &SceneDocument,
    view: &Normalized,
    format: OutputFormat,
) {
    let projected = project_rendering_view(scene, &view.outcome);
    let reparsed = SceneDocument::parse(&projected).unwrap_or_else(|error| {
        panic!(
            "projected rendering view is not admitted: {error} at {:?}\n{}",
            error.json_pointer(),
            String::from_utf8_lossy(&projected)
        )
    });
    let again = normalize(&reparsed, format);
    assert_eq!(
        idempotence_view(&again.outcome),
        idempotence_view(&view.outcome),
        "re-normalizing the {format:?} rendering view changed it\n{}",
        String::from_utf8_lossy(&projected)
    );
    let repairs = again
        .events
        .iter()
        .filter_map(|event| match event {
            ObservedEvent::Diagnostic {
                code, json_pointer, ..
            } if code.starts_with("normalization.") => Some((*code, json_pointer.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        repairs.is_empty(),
        "re-normalizing the {format:?} rendering view repaired it again: {repairs:?}\n{}",
        String::from_utf8_lossy(&projected)
    );
}

fn representable(element: &NormalizedElement) -> bool {
    match &element.kind {
        NormalizedElementKind::Linear(linear) => linear.geometry_available,
        NormalizedElementKind::Freedraw(freedraw) => freedraw.geometry_available,
        _ => true,
    }
}

/// The normalized values that have a scene-document spelling, in paint order.
fn idempotence_view(outcome: &NormalizationOutcome) -> String {
    let mut view = String::new();
    for element in outcome
        .elements
        .iter()
        .filter(|element| representable(element))
    {
        let kind = match &element.kind {
            NormalizedElementKind::Embeddable { iframe, .. } => format!("Embeddable({iframe})"),
            kind => format!("{kind:?}"),
        };
        let _ = writeln!(
            view,
            "{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{:?}|{:?}|{:?}|{}|{:?}|{}|{kind}",
            element.source.id(),
            element.index,
            element.x,
            element.y,
            element.width,
            element.height,
            element.angle,
            element.stroke_color,
            element.background_color,
            element.fill_style,
            element.stroke_width,
            element.stroke_style,
            element.roughness,
            element.opacity,
            element.frame_id,
            element.seed,
            element.roundness,
            element.kind_name,
        );
    }
    let _ = writeln!(view, "background={}", outcome.view_background_color);
    let _ = writeln!(view, "frame_rendering={:?}", outcome.frame_rendering);
    for file in &outcome.files {
        let _ = writeln!(view, "file|{}|{}|{:?}", file.key, file.id, file.created);
    }
    view
}

/// Spells the normalized SVG rendering view as a scene document.
fn project_rendering_view(scene: &SceneDocument, outcome: &NormalizationOutcome) -> Vec<u8> {
    use serde_json::{json, Map, Value};

    let elements = outcome
        .elements
        .iter()
        .filter(|element| representable(element))
        .map(project_element)
        .collect::<Vec<_>>();

    let mut app_state = Map::new();
    app_state.insert(
        "viewBackgroundColor".to_owned(),
        Value::String(outcome.view_background_color.clone()),
    );
    app_state.insert(
        "frameRendering".to_owned(),
        json!({
            "enabled": outcome.frame_rendering.enabled,
            "clip": outcome.frame_rendering.clip,
            "name": outcome.frame_rendering.name,
            "outline": outcome.frame_rendering.outline,
        }),
    );

    // Every source file key stays present so image participation is
    // unchanged; normalized records spell their repaired identity and
    // timestamp, other keys keep a non-record value.
    let mut files = Map::new();
    for index in 0..scene.file_record_count() {
        let Some(record) = scene.file_record_at(index) else {
            continue;
        };
        let normalized = outcome.files.iter().find(|file| file.key == record.key());
        let value = match normalized {
            Some(file) => {
                let mut members = Map::new();
                members.insert("id".to_owned(), Value::String(file.id.clone()));
                members.insert(
                    "created".to_owned(),
                    file.created.map_or(json!(-1), |created| json!(created)),
                );
                Value::Object(members)
            }
            None => Value::Bool(false),
        };
        files.insert(record.key().to_owned(), value);
    }

    let document = json!({
        "type": "excalidraw",
        "version": 2,
        "elements": elements,
        "appState": app_state,
        "files": files,
    });
    serde_json::to_vec(&document).expect("projected rendering view serializes")
}

fn project_element(element: &NormalizedElement) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let number = |value: f64| {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| panic!("normalized value {value} has no JSON spelling"))
    };
    let point = |point: &[f64; 2]| Value::Array(vec![number(point[0]), number(point[1])]);

    let mut members = Map::new();
    members.insert(
        "id".to_owned(),
        Value::String(element.source.id().to_owned()),
    );
    members.insert(
        "type".to_owned(),
        Value::String(element.kind_name.to_owned()),
    );
    members.insert("index".to_owned(), Value::String(element.index.clone()));
    members.insert("x".to_owned(), number(element.x));
    members.insert("y".to_owned(), number(element.y));
    members.insert("width".to_owned(), number(element.width));
    members.insert("height".to_owned(), number(element.height));
    members.insert("angle".to_owned(), number(element.angle));
    members.insert(
        "strokeColor".to_owned(),
        Value::String(element.stroke_color.clone()),
    );
    members.insert(
        "backgroundColor".to_owned(),
        Value::String(element.background_color.clone()),
    );
    members.insert(
        "fillStyle".to_owned(),
        Value::String(element.fill_style.clone()),
    );
    members.insert("strokeWidth".to_owned(), number(element.stroke_width));
    members.insert(
        "strokeStyle".to_owned(),
        Value::String(element.stroke_style.clone()),
    );
    members.insert("roughness".to_owned(), number(element.roughness));
    members.insert("opacity".to_owned(), number(element.opacity));
    members.insert("seed".to_owned(), json!(element.seed));
    members.insert("updated".to_owned(), json!(1));
    if let Some(frame_id) = &element.frame_id {
        members.insert("frameId".to_owned(), Value::String(frame_id.clone()));
    }
    if let Some(link) = &element.link {
        members.insert("link".to_owned(), Value::String(link.clone()));
    }
    match element.roundness {
        Some(NormalizedRoundness::Proportional) => {
            members.insert("roundness".to_owned(), json!({ "type": 2 }));
        }
        Some(NormalizedRoundness::Adaptive { radius }) => {
            members.insert(
                "roundness".to_owned(),
                json!({ "type": 3, "value": number(radius) }),
            );
        }
        None => {}
    }

    match &element.kind {
        NormalizedElementKind::Rectangle
        | NormalizedElementKind::Diamond
        | NormalizedElementKind::Ellipse => {}
        NormalizedElementKind::Text(text) => {
            members.insert("fontSize".to_owned(), number(text.font_size));
            members.insert("fontFamily".to_owned(), json!(text.font_family));
            members.insert("text".to_owned(), Value::String(text.text.clone()));
            members.insert(
                "originalText".to_owned(),
                Value::String(text.original_text.clone()),
            );
            members.insert(
                "textAlign".to_owned(),
                Value::String(text.text_align.clone()),
            );
            members.insert(
                "verticalAlign".to_owned(),
                Value::String(text.vertical_align.clone()),
            );
            members.insert("lineHeight".to_owned(), number(text.line_height));
        }
        NormalizedElementKind::Linear(linear) => {
            members.insert(
                "points".to_owned(),
                Value::Array(linear.points.iter().map(point).collect()),
            );
            members.insert("elbowed".to_owned(), Value::Bool(linear.elbowed));
            for (field, binding) in [
                ("startBinding", &linear.start_binding),
                ("endBinding", &linear.end_binding),
            ] {
                let value = match binding {
                    Some(binding) => {
                        let mut record = Map::new();
                        record.insert(
                            "elementId".to_owned(),
                            Value::String(binding.element_id.clone()),
                        );
                        record.insert("focus".to_owned(), number(binding.focus));
                        record.insert("gap".to_owned(), number(binding.gap));
                        if let Some(fixed_point) = &binding.fixed_point {
                            record.insert("fixedPoint".to_owned(), point(fixed_point));
                        }
                        Value::Object(record)
                    }
                    None => Value::Null,
                };
                members.insert(field.to_owned(), value);
            }
            for (field, arrowhead) in [
                ("startArrowhead", &linear.start_arrowhead),
                ("endArrowhead", &linear.end_arrowhead),
            ] {
                members.insert(
                    field.to_owned(),
                    arrowhead
                        .as_ref()
                        .map_or(Value::Null, |arrowhead| Value::String(arrowhead.clone())),
                );
            }
        }
        NormalizedElementKind::Freedraw(freedraw) => {
            members.insert(
                "points".to_owned(),
                Value::Array(freedraw.points.iter().map(point).collect()),
            );
            if !freedraw.pressures.is_empty() {
                members.insert(
                    "pressures".to_owned(),
                    Value::Array(freedraw.pressures.iter().copied().map(number).collect()),
                );
            }
            members.insert(
                "simulatePressure".to_owned(),
                Value::Bool(freedraw.simulate_pressure),
            );
        }
        NormalizedElementKind::Image(image) => {
            members.insert(
                "fileId".to_owned(),
                image
                    .file_id
                    .as_ref()
                    .map_or(Value::Null, |file_id| Value::String(file_id.clone())),
            );
            members.insert(
                "status".to_owned(),
                Value::String(
                    match image.status {
                        NormalizedImageStatus::Pending => "pending",
                        NormalizedImageStatus::Saved => "saved",
                        NormalizedImageStatus::Error => "error",
                    }
                    .to_owned(),
                ),
            );
            members.insert("scale".to_owned(), json!([image.scale[0], image.scale[1]]));
            if let Some(crop) = &image.crop {
                members.insert(
                    "crop".to_owned(),
                    json!({
                        "x": number(crop.x),
                        "y": number(crop.y),
                        "width": number(crop.width),
                        "height": number(crop.height),
                        "naturalWidth": number(crop.natural_width),
                        "naturalHeight": number(crop.natural_height),
                    }),
                );
            }
        }
        NormalizedElementKind::Frame(frame) | NormalizedElementKind::MagicFrame(frame) => {
            members.insert(
                "name".to_owned(),
                frame
                    .name
                    .as_ref()
                    .map_or(Value::Null, |name| Value::String(name.clone())),
            );
        }
        NormalizedElementKind::Embeddable { .. } => {}
    }
    Value::Object(members)
}
