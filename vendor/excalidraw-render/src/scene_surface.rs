use std::collections::TryReserveError;

use crate::diagnostic_registry::{DiagnosticKind, ImageFailure, ImageRecovery, LocalRecovery};
use crate::outcome::PendingDiagnostic;
use crate::scene::{
    finite_json_number, member_value, pointer_child, pointer_index, JsonMember, JsonValue,
};
use crate::{DiagnosticCode, DiagnosticEffect, OutputFormat, Recovery, SceneDocument};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SceneFieldDisposition {
    Supported,
    StaticExportIrrelevant,
    OpaqueMetadata,
}

/// The manifested recovery of one malformed (`invalid`) field. The manifest's
/// `missing` fallbacks and every fallback's `action` are not emitted: the
/// renderer settles missing fields from the restore-path defaults normalization
/// hardcodes, and the frozen JSON manifest keeps the data.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneFieldFallback {
    pub(crate) code: Option<&'static str>,
    pub(crate) effects: &'static [&'static str],
    pub(crate) recovery: Option<&'static str>,
    pub(crate) recipe: Option<&'static str>,
    pub(crate) format_recoveries: [Option<&'static str>; 3],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneFieldPolicy {
    pub(crate) record_kind: &'static str,
    pub(crate) path: &'static str,
    pub(crate) accepted_domain: &'static str,
    pub(crate) disposition: SceneFieldDisposition,
    pub(crate) participation: &'static str,
    pub(crate) formats: &'static [&'static str],
    pub(crate) invalid: SceneFieldFallback,
    pub(crate) ordinal: usize,
}

/// One record kind of the scene surface. The manifest's `fieldPaths` are not
/// emitted: `xtask catalog verify` checks that they own exactly the field
/// dispositions, and the renderer reads a field's owner from the field.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneRecordPolicy {
    pub(crate) identity: &'static str,
    pub(crate) inherits: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneElementPolicy {
    pub(crate) kind: &'static str,
    pub(crate) record_kind: &'static str,
    pub(crate) status: &'static str,
    pub(crate) ignored_inherited_paths: &'static [&'static str],
}

/// The notice obligations of one manifested fallback recipe. The recipe's
/// identity, glyph, paint, placement and inheritance content is not emitted:
/// `xtask catalog verify` pins the identities, the renderer hardcodes the
/// official placeholders, and the frozen JSON manifest keeps the data.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneFallbackRecipePolicy {
    pub(crate) content_kind: &'static str,
    /// The notices every artifact that paints the recipe must carry.
    pub(crate) notices: &'static [&'static str],
    /// Whether the recipe paints in `svg`, `png` and `pdf`, in that order.
    pub(crate) paints: [bool; 3],
}

impl SceneFallbackRecipePolicy {
    pub(crate) fn paints(&self, format: OutputFormat) -> bool {
        self.paints[format_slot(format)]
    }
}

/// One output-traveling notice of the manifest's notice catalog. Its source
/// identities and text digest are not emitted: `xtask catalog verify` checks
/// the digest against the text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneNoticePolicy {
    pub(crate) identity: &'static str,
    pub(crate) asset_identity: &'static str,
    pub(crate) required_formats: &'static [&'static str],
    pub(crate) text: &'static str,
}

impl SceneNoticePolicy {
    pub(crate) fn requires(&self, format: OutputFormat) -> bool {
        self.required_formats.contains(&format.as_str())
    }
}

/// The sealed revision-1 scene-surface policy. The manifest's registry
/// mappings are not emitted: `xtask catalog verify` pins their count and
/// uniqueness, and diagnostics mint their codes from the field policies.
#[derive(Debug)]
pub(crate) struct SceneSurfacePolicy {
    pub(crate) record_kinds: &'static [SceneRecordPolicy],
    pub(crate) fields: &'static [SceneFieldPolicy],
    pub(crate) element_kinds: &'static [SceneElementPolicy],
    pub(crate) fallback_recipes: &'static [SceneFallbackRecipePolicy],
    pub(crate) notices: &'static [SceneNoticePolicy],
}

impl SceneSurfacePolicy {
    /// Looks up one manifested notice by its catalog identity. Artifacts
    /// carry font notices from the asset catalog's `FontNotice` table (#151),
    /// so only the policy's own tests look a notice up by identity.
    #[cfg(test)]
    pub(crate) fn notice(&self, identity: &str) -> Option<&'static SceneNoticePolicy> {
        self.notices
            .iter()
            .find(|notice| notice.identity == identity)
    }
}

/// The index of `format` in the per-format arrays the generated policy carries
/// in `svg`, `png`, `pdf` order.
const fn format_slot(format: OutputFormat) -> usize {
    match format {
        OutputFormat::Svg => 0,
        OutputFormat::Png => 1,
        OutputFormat::Pdf => 2,
    }
}

include!("generated/scene_surface_revision_1.rs");

pub(crate) struct SurfaceDiagnostic {
    pub(crate) source_order: u64,
    pub(crate) source_position: u64,
    pub(crate) local_owner: Option<String>,
    pub(crate) diagnostic: PendingDiagnostic,
}

pub(crate) struct SurfaceClassification {
    pub(crate) diagnostics: Vec<SurfaceDiagnostic>,
    pub(crate) fallbacks: Vec<SurfaceFallback>,
}

#[derive(Clone, Debug)]
pub(crate) struct SurfaceFallback {
    pub(crate) source_index: usize,
    pub(crate) source_order: u64,
    pub(crate) element_id: Option<String>,
    pub(crate) json_pointer: String,
    pub(crate) label: String,
    pub(crate) link: Option<String>,
    pub(crate) frame_id: Option<String>,
    pub(crate) is_frame: bool,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) angle: f64,
    pub(crate) opacity: f64,
    pub(crate) paint: bool,
}

/// The source position of one top-level record member in the total diagnostic
/// order: the member index in the high half, leaving the low half for the
/// nested position that diagnostics inside the member (such as the frame
/// rendering controls under `appState.frameRendering`) carry, so a member's
/// own diagnostics and its nested ones sort together by the member.
fn member_source_position(position: usize) -> u64 {
    u64::try_from(position)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(u32::MAX) + 1)
}

pub(crate) fn classify(
    scene: &SceneDocument,
    policy: &'static SceneSurfacePolicy,
    format: OutputFormat,
    frame_names: bool,
    diagnostic_limit: usize,
) -> Result<SurfaceClassification, TryReserveError> {
    let mut diagnostics = Vec::new();
    let mut fallbacks = Vec::new();
    classify_record(
        scene,
        policy,
        "scene",
        scene.root_members(),
        "",
        None,
        format,
        diagnostic_limit,
        &mut diagnostics,
    )?;
    classify_record(
        scene,
        policy,
        "app_state",
        scene.app_state_members(),
        "/appState",
        Some("appState"),
        format,
        diagnostic_limit,
        &mut diagnostics,
    )?;
    let participating_ids = ParticipatingElementIds::new(scene)?;
    for index in 0..scene.element_occurrence_count() {
        if diagnostics.len() >= diagnostic_limit {
            break;
        }
        if scene.element_is_silent(index) {
            continue;
        }
        let pointer = crate::scene::pointer_index("/elements", index)?;
        let source_order = scene.source_owner_order("elements", index);
        let Some(value) = scene.element_value(index) else {
            continue;
        };
        let JsonValue::Object(members) = value else {
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: 0,
                local_owner: None,
                diagnostic: PendingDiagnostic::new(
                    DiagnosticKind::MalformedElement(LocalRecovery::Omitted),
                    format,
                    &[DiagnosticEffect::Visual],
                    None,
                    &pointer,
                    "malformed element was omitted by the revision-1 scene-surface policy",
                )?,
            });
            continue;
        };
        let kind = match member_value(members, "type") {
            Some(JsonValue::String(kind)) if !kind.is_empty() => Some(kind.as_ref()),
            _ => None,
        };
        let element_id = match member_value(members, "id") {
            Some(JsonValue::String(id)) if !id.is_empty() => Some(id.as_ref()),
            _ => None,
        };
        let element_policy = kind.and_then(|kind| {
            policy
                .element_kinds
                .iter()
                .find(|element| element.kind == kind)
        });
        if let Some(element_policy) = element_policy {
            if element_policy.status == "static-export-irrelevant" {
                continue;
            }
            let has_unrendered_element = scene.element_at(index).is_none();
            if has_unrendered_element {
                let bounds = recoverable_bounds(members);
                diagnostics.try_reserve(1)?;
                diagnostics.push(SurfaceDiagnostic {
                    source_order,
                    source_position: 0,
                    local_owner: element_id.map(try_owned).transpose()?,
                    diagnostic: PendingDiagnostic::new(
                        DiagnosticKind::KnownElementReplaced(LocalRecovery::placeholder_if(
                            bounds.is_some(),
                        )),
                        format,
                        &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic],
                        element_id,
                        &pointer,
                        "known element unavailable to this renderer used the revision-1 fallback",
                    )?,
                });
                if let Some(bounds) = bounds {
                    let label = fallback_label("Unavailable: ", element_policy.kind)?;
                    fallbacks.try_reserve(1)?;
                    fallbacks.push(surface_fallback(
                        index,
                        source_order,
                        element_id,
                        &pointer,
                        &label,
                        fallback_link(members)?,
                        fallback_frame_id(members),
                        matches!(element_policy.kind, "frame" | "magicframe"),
                        bounds,
                    )?);
                }
            }
            classify_element_record(
                &participating_ids,
                policy,
                element_policy,
                members,
                &pointer,
                source_order,
                element_id,
                format,
                frame_names,
                has_unrendered_element,
                diagnostic_limit,
                &mut diagnostics,
            )?;
            continue;
        }
        if kind == Some("draw") {
            classify_element_record(
                &participating_ids,
                policy,
                &SceneElementPolicy {
                    kind: "draw",
                    record_kind: "element.legacy_draw",
                    status: "supported",
                    ignored_inherited_paths: &[],
                },
                members,
                &pointer,
                source_order,
                element_id,
                format,
                frame_names,
                false,
                diagnostic_limit,
                &mut diagnostics,
            )?;
            continue;
        }

        let bounds = recoverable_bounds(members);
        let recovery = LocalRecovery::placeholder_if(bounds.is_some());
        let (diagnostic_kind, effects, message, label) = if let Some(kind) = kind {
            (
                DiagnosticKind::UnsupportedElementType(recovery),
                &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic][..],
                "unsupported element type used the revision-1 element fallback",
                fallback_label("Unsupported: ", kind)?,
            )
        } else {
            // A link is a recoverable common semantic channel even when the
            // type discriminator is malformed. A bounded placeholder keeps it;
            // omission loses it in the formats that expose navigation.
            let loses_link = bounds.is_none()
                && format != OutputFormat::Png
                && fallback_link(members)?.is_some();
            (
                DiagnosticKind::MalformedElement(recovery),
                if loses_link {
                    &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic][..]
                } else {
                    &[DiagnosticEffect::Visual][..]
                },
                "malformed element used the revision-1 element fallback",
                try_owned("Malformed element")?,
            )
        };
        diagnostics.try_reserve(1)?;
        diagnostics.push(SurfaceDiagnostic {
            source_order,
            source_position: 0,
            local_owner: element_id.map(try_owned).transpose()?,
            diagnostic: PendingDiagnostic::new(
                diagnostic_kind,
                format,
                effects,
                element_id,
                &pointer,
                message,
            )?,
        });
        if format != OutputFormat::Png
            && diagnostics.len() < diagnostic_limit
            && matches!(member_value(members, "link"), Some(JsonValue::String(link)) if !safe_link(link))
        {
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: u64::try_from(
                    members
                        .iter()
                        .position(|member| &*member.key == "link")
                        .unwrap_or(usize::MAX),
                )
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::from(u32::MAX) + 1),
                local_owner: element_id.map(try_owned).transpose()?,
                diagnostic: unsafe_link_diagnostic(
                    format,
                    element_id,
                    &pointer_child(&pointer, "link")?,
                )?,
            });
        }
        if let Some(bounds) = bounds {
            fallbacks.try_reserve(1)?;
            fallbacks.push(surface_fallback(
                index,
                source_order,
                element_id,
                &pointer,
                &label,
                fallback_link(members)?,
                fallback_frame_id(members),
                false,
                bounds,
            )?);
        }
    }
    Ok(SurfaceClassification {
        diagnostics,
        fallbacks,
    })
}

#[allow(clippy::too_many_arguments)]
fn classify_record(
    scene: &SceneDocument,
    policy: &'static SceneSurfacePolicy,
    record_kind: &str,
    members: &[JsonMember],
    pointer: &str,
    owner: Option<&str>,
    format: OutputFormat,
    diagnostic_limit: usize,
    diagnostics: &mut Vec<SurfaceDiagnostic>,
) -> Result<(), TryReserveError> {
    for (position, member) in members.iter().enumerate() {
        if diagnostics.len() >= diagnostic_limit {
            break;
        }
        let semantic_path = pointer_child("", &member.key)?;
        if let Some(field) = field_policy(policy, record_kind, &semantic_path) {
            if field.disposition == SceneFieldDisposition::Supported
                && !value_matches_domain(&member.value, field.accepted_domain)
            {
                if let Some(diagnostic) = manifested_field_diagnostic(
                    &field.invalid,
                    format,
                    &pointer_child(pointer, &member.key)?,
                    None,
                )? {
                    diagnostics.try_reserve(1)?;
                    diagnostics.push(SurfaceDiagnostic {
                        source_order: scene.source_owner_order(owner.unwrap_or(&member.key), 0),
                        source_position: member_source_position(position),
                        local_owner: None,
                        diagnostic,
                    });
                }
            }
            continue;
        }
        diagnostics.try_reserve(1)?;
        diagnostics.push(SurfaceDiagnostic {
            source_order: scene.source_owner_order(owner.unwrap_or(&member.key), 0),
            source_position: member_source_position(position),
            local_owner: None,
            diagnostic: unrecognized_property_diagnostic(
                format,
                None,
                &pointer_child(pointer, &member.key)?,
                is_editor_only_property(PropertyOwner::Record(record_kind), &semantic_path),
                "unrecognized property was ignored",
            )?,
        });
    }
    Ok(())
}

fn value_matches_domain(value: &JsonValue, domain: &str) -> bool {
    let string_is = |allowed: &[&str]| matches!(value, JsonValue::String(string) if allowed.contains(&string.as_ref()));
    let integer = || match value {
        JsonValue::Number(token) => {
            !token.contains(['.', 'e', 'E']) && finite_json_number(value).is_some()
        }
        _ => false,
    };
    let number_array = |accepts: fn(f64) -> bool| match value {
        JsonValue::Array(values) => values
            .iter()
            .all(|value| finite_json_number(value).is_some_and(accepts)),
        _ => false,
    };
    let point = |value: &JsonValue| match value {
        JsonValue::Array(values) => {
            values.len() == 2
                && values
                    .iter()
                    .all(|value| finite_json_number(value).is_some())
        }
        _ => false,
    };
    match domain {
        "any_json" => true,
        "arrow_type" => string_is(&["sharp", "round", "elbow"]),
        "arrowhead" => {
            matches!(value, JsonValue::Null)
                || string_is(&[
                    "arrow",
                    "bar",
                    "dot",
                    "circle",
                    "circle_outline",
                    "triangle",
                    "triangle_outline",
                    "diamond",
                    "diamond_outline",
                    "crowfoot_one",
                    "crowfoot_many",
                    "crowfoot_one_or_many",
                ])
        }
        "boolean" => matches!(value, JsonValue::Bool(_)),
        "bound_element_array" => match value {
            JsonValue::Null => true,
            JsonValue::Array(values) => values.iter().all(|value| {
                let JsonValue::Object(members) = value else {
                    return false;
                };
                matches!(member_value(members, "id"), Some(JsonValue::String(id)) if !id.is_empty())
                    && matches!(member_value(members, "type"), Some(JsonValue::String(kind)) if matches!(&**kind, "arrow" | "text"))
            }),
            _ => false,
        },
        "bound_element_type" => string_is(&["arrow", "text"]),
        "chart_type" => string_is(&["bar", "line"]),
        "css_color" => matches!(value, JsonValue::String(color) if css_color_is_admitted(color)),
        "cursor_button" => string_is(&["up", "down"]),
        "data_url" => matches!(value, JsonValue::String(url) if url.starts_with("data:")),
        "element_type" => string_is(&[
            "selection",
            "rectangle",
            "diamond",
            "ellipse",
            "text",
            "line",
            "arrow",
            "freedraw",
            "image",
            "frame",
            "magicframe",
            "iframe",
            "embeddable",
        ]),
        "fill_style" => string_is(&[
            "hachure",
            "cross-hatch",
            "solid",
            "zigzag",
            "zigzag-line",
            "dots",
            "dashed",
        ]),
        "finite_number" => finite_json_number(value).is_some(),
        "finite_number_array" => number_array(|_| true),
        "fixed_segment_array" => matches!(value, JsonValue::Null)
            || matches!(value, JsonValue::Array(values) if values.iter().all(|value| matches!(value, JsonValue::Object(_)))),
        "font_family" => integer() && matches!(finite_json_number(value), Some(1.0 | 2.0 | 3.0 | 5.0 | 6.0 | 7.0 | 8.0 | 9.0)),
        "freedraw_points" => {
            matches!(value, JsonValue::Array(values) if values.iter().all(&point))
        }
        "hyperlink_popup" => matches!(value, JsonValue::Bool(false))
            || string_is(&["info", "editor"]),
        "image_crop" => {
            matches!(value, JsonValue::Null | JsonValue::Object(_))
        }
        "image_mime" => string_is(&[
            "image/svg+xml",
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/bmp",
            "image/x-icon",
            "image/avif",
            "image/jfif",
            "application/octet-stream",
        ]),
        "image_scale" => match value {
            JsonValue::Array(values) => {
                values.len() == 2
                    && values.iter().all(|value| {
                        matches!(finite_json_number(value), Some(-1.0 | 1.0))
                    })
            }
            _ => false,
        },
        "image_status" => string_is(&["pending", "saved", "error"]),
        "integer" => integer(),
        "nonempty_string" => {
            matches!(value, JsonValue::String(string) if !string.is_empty())
        }
        "nonnegative_integer" => {
            integer() && finite_json_number(value).is_some_and(|value| value >= 0.0)
        }
        "nonnegative_number" => finite_json_number(value).is_some_and(|value| value >= 0.0),
        "nonzero_integer" => {
            integer() && finite_json_number(value).is_some_and(|value| value != 0.0)
        }
        "nullable_boolean" => matches!(value, JsonValue::Null | JsonValue::Bool(_)),
        "nullable_font_family" => {
            matches!(value, JsonValue::Null)
                || (integer()
                    && matches!(finite_json_number(value), Some(1.0 | 2.0 | 3.0 | 5.0 | 6.0 | 7.0 | 8.0 | 9.0)))
        }
        "nullable_point" => matches!(value, JsonValue::Null) || point(value),
        "nullable_record" => matches!(value, JsonValue::Null | JsonValue::Object(_)),
        "nullable_string" => matches!(value, JsonValue::Null | JsonValue::String(_)),
        "opacity" => {
            finite_json_number(value).is_some_and(|value| (0.0..=100.0).contains(&value))
        }
        "open_menu" => matches!(value, JsonValue::Null) || string_is(&["canvas", "shape"]),
        "open_popup" => {
            matches!(value, JsonValue::Null)
                || string_is(&[
                    "canvasBackground",
                    "elementBackground",
                    "elementStroke",
                    "fontFamily",
                ])
        }
        "point" => point(value),
        "point_array" => {
            matches!(value, JsonValue::Array(values) if values.len() >= 2 && values.iter().all(point))
        }
        "point_binding" => matches!(value, JsonValue::Null | JsonValue::Object(_)),
        "pointer_type" => string_is(&["mouse", "pen", "touch"]),
        "pressure_array" => number_array(|value| (0.0..=1.0).contains(&value)),
        "roundness" => matches!(value, JsonValue::Null | JsonValue::Object(_)),
        "roundness_type" => {
            integer() && matches!(finite_json_number(value), Some(1.0 | 2.0 | 3.0))
        }
        "string" => matches!(value, JsonValue::String(_)),
        "string_array" => {
            matches!(value, JsonValue::Array(values) if values.iter().all(|value| matches!(value, JsonValue::String(_))))
        }
        "stroke_roundness" => string_is(&["round", "sharp"]),
        "stroke_style" => string_is(&["solid", "dashed", "dotted"]),
        "text_align" => string_is(&["left", "center", "right"]),
        "theme" => string_is(&["light", "dark"]),
        "vertical_align" => string_is(&["top", "middle", "bottom"]),
        "literal_excalidraw" => {
            matches!(value, JsonValue::String(value) if &**value == "excalidraw")
        }
        "literal_v2" => matches!(value, JsonValue::Number(value) if &**value == "2"),
        "scene_elements" => matches!(value, JsonValue::Array(_)),
        "positive_number" => finite_json_number(value).is_some_and(|value| value > 0.0),
        "frame_rendering" | "object" => matches!(value, JsonValue::Object(_)),
        _ => false,
    }
}

pub(crate) fn css_color_is_admitted(color: &str) -> bool {
    crate::color::parse_css(color).is_some()
}

/// The diagnostic a supported field's manifested `invalid` fallback declares
/// for `format`, or `None` when the fallback names no code, no recovery for
/// the format, or a binding the diagnostic registry does not have (the
/// registry tests check the sealed table against the registry).
fn manifested_field_diagnostic(
    fallback: &SceneFieldFallback,
    format: OutputFormat,
    json_pointer: &str,
    element_id: Option<&str>,
) -> Result<Option<PendingDiagnostic>, TryReserveError> {
    let Some(code) = fallback.code else {
        return Ok(None);
    };
    let recovery = fallback
        .recovery
        .or(fallback.format_recoveries[format_slot(format)]);
    let Some(recovery) = recovery.and_then(Recovery::from_manifest) else {
        return Ok(None);
    };
    let Some(kind) = DiagnosticKind::manifested(code, recovery, fallback.recipe) else {
        return Ok(None);
    };
    let mut effects = Vec::new();
    effects.try_reserve_exact(fallback.effects.len())?;
    for effect in fallback.effects {
        if let Some(effect) = DiagnosticEffect::from_manifest(effect) {
            effects.push(effect);
        }
    }
    PendingDiagnostic::new(
        kind,
        format,
        &effects,
        element_id,
        json_pointer,
        "manifested field value used its revision-1 recovery",
    )
    .map(Some)
}

#[allow(clippy::too_many_arguments)]
fn classify_element_record(
    participating_ids: &ParticipatingElementIds<'_>,
    policy: &'static SceneSurfacePolicy,
    element_policy: &SceneElementPolicy,
    members: &[JsonMember],
    pointer: &str,
    source_order: u64,
    element_id: Option<&str>,
    format: OutputFormat,
    frame_names: bool,
    validate_values: bool,
    diagnostic_limit: usize,
    diagnostics: &mut Vec<SurfaceDiagnostic>,
) -> Result<(), TryReserveError> {
    for (position, member) in members.iter().enumerate() {
        if diagnostics.len() >= diagnostic_limit {
            break;
        }
        let semantic_path = pointer_child("", &member.key)?;
        if element_policy
            .ignored_inherited_paths
            .contains(&semantic_path.as_str())
        {
            continue;
        }
        if let Some(field) = field_policy(policy, element_policy.record_kind, &semantic_path) {
            if semantic_path == "/boundElements" {
                classify_bound_elements(
                    participating_ids,
                    policy,
                    &member.value,
                    pointer,
                    position,
                    source_order,
                    element_id,
                    format,
                    diagnostic_limit,
                    diagnostics,
                )?;
                // Object records recover at their own member boundary. Only
                // a malformed array/container uses the parent substitution.
                if matches!(&member.value, JsonValue::Array(values) if values.iter().all(|v|matches!(v,JsonValue::Object(_))))
                {
                    continue;
                }
            }
            if field_participates(field, format, frame_names)
                && semantic_path == "/link"
                && matches!(&member.value, JsonValue::String(link) if !safe_link(link))
            {
                diagnostics.try_reserve(1)?;
                diagnostics.push(SurfaceDiagnostic {
                    source_order,
                    source_position: u64::try_from(position)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(u64::from(u32::MAX) + 1),
                    local_owner: element_id.map(try_owned).transpose()?,
                    diagnostic: unsafe_link_diagnostic(
                        format,
                        element_id,
                        &pointer_child(pointer, &member.key)?,
                    )?,
                });
                continue;
            }
            if validate_values
                && semantic_path == "/seed"
                && matches!(finite_json_number(&member.value), Some(0.0))
            {
                diagnostics.try_reserve(1)?;
                diagnostics.push(SurfaceDiagnostic {
                    source_order,
                    source_position: u64::try_from(position)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(u64::from(u32::MAX) + 1),
                    local_owner: element_id.map(try_owned).transpose()?,
                    diagnostic: PendingDiagnostic::new(
                        DiagnosticKind::ZeroRoughSeed,
                        format,
                        &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                        element_id,
                        &pointer_child(pointer, &member.key)?,
                        "zero Rough seed was repaired deterministically",
                    )?,
                });
                continue;
            }
            if (validate_values || surface_validates_element_field(field))
                && field.disposition != SceneFieldDisposition::OpaqueMetadata
                && field_participates(field, format, frame_names)
                && field.invalid.code.is_some()
                && !value_matches_domain(&member.value, field.accepted_domain)
            {
                if let Some(diagnostic) = manifested_field_diagnostic(
                    &field.invalid,
                    format,
                    &pointer_child(pointer, &member.key)?,
                    element_id,
                )? {
                    diagnostics.try_reserve(1)?;
                    diagnostics.push(SurfaceDiagnostic {
                        source_order,
                        source_position: u64::try_from(position)
                            .unwrap_or(u64::MAX)
                            .saturating_mul(u64::from(u32::MAX) + 1),
                        local_owner: element_id.map(try_owned).transpose()?,
                        diagnostic,
                    });
                }
            }
            continue;
        }
        diagnostics.try_reserve(1)?;
        diagnostics.push(SurfaceDiagnostic {
            source_order,
            source_position: u64::try_from(position)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::from(u32::MAX) + 1),
            local_owner: element_id.map(try_owned).transpose()?,
            diagnostic: unrecognized_property_diagnostic(
                format,
                element_id,
                &pointer_child(pointer, &member.key)?,
                is_editor_only_property(PropertyOwner::Element, &semantic_path),
                "unrecognized element property was ignored",
            )?,
        });
    }
    Ok(())
}

fn surface_validates_element_field(field: &SceneFieldPolicy) -> bool {
    matches!(
        field.accepted_domain,
        "bound_element_array" | "css_color" | "string_array"
    ) || matches!(
        field.path,
        "/link" | "/strokeSharpness" | "/boundElementIds"
    )
}

fn field_participates(field: &SceneFieldPolicy, format: OutputFormat, frame_names: bool) -> bool {
    field.formats.contains(&format.as_str())
        && (field.participation != "participating-frame-when-frame-name-rendering-is-enabled"
            || frame_names)
}

pub(crate) fn safe_link(link: &str) -> bool {
    matches!(sanitized_link(link), Ok(Some(_)))
}

pub(crate) fn sanitized_link(link: &str) -> Result<Option<String>, TryReserveError> {
    let decoded = decode_numeric_html_entities(link)?;
    let mut sanitized = String::new();
    sanitized.try_reserve_exact(decoded.len())?;
    let mut index = 0;
    while index < decoded.len() {
        let remaining = &decoded[index..];
        if remaining
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("&newline;"))
        {
            index += 9;
            continue;
        }
        if remaining
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("&tab;"))
        {
            index += 5;
            continue;
        }
        let Some(character) = remaining.chars().next() else {
            break;
        };
        if !matches!(
            u32::from(character),
            0x0000..=0x001f | 0x007f..=0x009f | 0x2000..=0x200d | 0xfeff
        ) {
            sanitized.push(character);
        }
        index += character.len_utf8();
    }
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        return Ok(None);
    }
    if matches!(sanitized.as_bytes().first(), Some(b'.' | b'/')) {
        return try_owned(sanitized).map(Some);
    }
    if sanitized.contains(':') || contains_ascii_case_insensitive(sanitized, b"&colon;") {
        let protocol = sanitized.trim_start_matches(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_'
        });
        if starts_with_ascii_case_insensitive(protocol, b"javascript")
            || starts_with_ascii_case_insensitive(protocol, b"data")
            || starts_with_ascii_case_insensitive(protocol, b"vbscript")
        {
            return Ok(None);
        }
    }
    try_owned(sanitized).map(Some)
}

fn decode_numeric_html_entities(link: &str) -> Result<String, TryReserveError> {
    let mut decoded = String::new();
    decoded.try_reserve_exact(link.len())?;
    let mut remaining = link;
    while let Some(start) = remaining.find("&#") {
        decoded.push_str(&remaining[..start]);
        let entity = &remaining[start + 2..];
        let word_bytes = entity
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        if word_bytes == 0 {
            decoded.push_str("&#");
            remaining = entity;
            continue;
        }
        let token = &entity[..word_bytes];
        let number = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .map_or_else(
                || token.parse::<f64>().unwrap_or(f64::NAN),
                |hex| {
                    hex.bytes().fold(0.0, |value, digit| {
                        value * 16.0
                            + f64::from(match digit {
                                b'0'..=b'9' => digit - b'0',
                                b'a'..=b'f' => digit - b'a' + 10,
                                b'A'..=b'F' => digit - b'A' + 10,
                                _ => 0,
                            })
                    })
                },
            );
        let value = if number.is_finite() {
            number.trunc().rem_euclid(65_536.0) as u32
        } else {
            0
        };
        decoded.push(char::from_u32(value).unwrap_or('\0'));
        let suffix = usize::from(entity.as_bytes().get(word_bytes) == Some(&b';'));
        remaining = &entity[word_bytes + suffix..];
    }
    decoded.push_str(remaining);
    Ok(decoded)
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &[u8]) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn contains_ascii_case_insensitive(value: &str, needle: &[u8]) -> bool {
    value
        .as_bytes()
        .windows(needle.len())
        .any(|candidate| candidate.eq_ignore_ascii_case(needle))
}

fn unsafe_link_diagnostic(
    format: OutputFormat,
    element_id: Option<&str>,
    json_pointer: &str,
) -> Result<PendingDiagnostic, TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::UnsafeLinkOmitted,
        format,
        &[DiagnosticEffect::Semantic],
        element_id,
        json_pointer,
        "unsafe link was omitted by the revision-1 sanitizer",
    )
}

#[allow(clippy::too_many_arguments)]
fn classify_bound_elements(
    participating_ids: &ParticipatingElementIds<'_>,
    policy: &'static SceneSurfacePolicy,
    value: &JsonValue,
    element_pointer: &str,
    member_position: usize,
    source_order: u64,
    element_id: Option<&str>,
    format: OutputFormat,
    diagnostic_limit: usize,
    diagnostics: &mut Vec<SurfaceDiagnostic>,
) -> Result<(), TryReserveError> {
    let JsonValue::Array(values) = value else {
        return Ok(());
    };
    if !values
        .iter()
        .all(|value| matches!(value, JsonValue::Object(_)))
    {
        return Ok(());
    }
    for (index, value) in values.iter().enumerate() {
        if diagnostics.len() >= diagnostic_limit {
            break;
        }
        let JsonValue::Object(members) = value else {
            continue;
        };
        let target_id = match member_value(members, "id") {
            Some(JsonValue::String(id)) if !id.is_empty() => Some(id.as_ref()),
            _ => None,
        };
        if target_id.is_some_and(|target| !participating_ids.contains(target)) {
            continue;
        }
        let record_pointer =
            pointer_index(&pointer_child(element_pointer, "boundElements")?, index)?;
        for (ordinal, key) in ["id", "type"].into_iter().enumerate() {
            if member_value(members, key).is_none() && diagnostics.len() < diagnostic_limit {
                let Some(field) =
                    field_policy(policy, "nested.bound_element", &pointer_child("", key)?)
                else {
                    continue;
                };
                if let Some(diagnostic) = manifested_field_diagnostic(
                    &field.invalid,
                    format,
                    &pointer_child(&record_pointer, key)?,
                    element_id,
                )? {
                    diagnostics.try_reserve(1)?;
                    diagnostics.push(SurfaceDiagnostic {
                        source_order,
                        source_position: (member_position as u64)
                            .saturating_mul(u64::from(u32::MAX) + 1)
                            .saturating_add(
                                ((index + 1) as u64).saturating_mul(u64::from(u16::MAX) + 1),
                            )
                            .saturating_add((members.len() + ordinal + 1) as u64),
                        local_owner: element_id.map(try_owned).transpose()?,
                        diagnostic,
                    });
                }
            }
        }
        for (nested_position, member) in members.iter().enumerate() {
            if diagnostics.len() >= diagnostic_limit {
                break;
            }
            let path = pointer_child("", &member.key)?;
            let pointer = pointer_child(&record_pointer, &member.key)?;
            let diagnostic = match field_policy(policy, "nested.bound_element", &path) {
                Some(field) if !value_matches_domain(&member.value, field.accepted_domain) => {
                    manifested_field_diagnostic(&field.invalid, format, &pointer, element_id)?
                }
                Some(_) => None,
                None => Some(unrecognized_property_diagnostic(
                    format,
                    element_id,
                    &pointer,
                    false,
                    "unrecognized bound-element property was ignored",
                )?),
            };
            let Some(diagnostic) = diagnostic else {
                continue;
            };
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: u64::try_from(member_position)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(u64::from(u32::MAX) + 1)
                    .saturating_add(
                        u64::try_from(index.saturating_add(1))
                            .unwrap_or(u64::MAX)
                            .saturating_mul(u64::from(u16::MAX) + 1),
                    )
                    .saturating_add(
                        u64::try_from(nested_position.saturating_add(1)).unwrap_or(u64::MAX),
                    ),
                local_owner: element_id.map(try_owned).transpose()?,
                diagnostic,
            });
        }
    }
    Ok(())
}

/// The source `id` of every participating (not deleted) element record,
/// sorted, so each bound-element record checks its target by binary search
/// instead of scanning the whole scene.
struct ParticipatingElementIds<'a>(Vec<&'a str>);

impl<'a> ParticipatingElementIds<'a> {
    fn new(scene: &'a SceneDocument) -> Result<Self, TryReserveError> {
        let mut ids = Vec::new();
        ids.try_reserve_exact(scene.element_occurrence_count())?;
        for index in 0..scene.element_occurrence_count() {
            if scene.element_is_silent(index) {
                continue;
            }
            let Some(JsonValue::Object(element)) = scene.element_value(index) else {
                continue;
            };
            if let Some(JsonValue::String(id)) = member_value(element, "id") {
                ids.push(&**id);
            }
        }
        ids.sort_unstable();
        Ok(Self(ids))
    }

    fn contains(&self, expected: &str) -> bool {
        self.0.binary_search(&expected).is_ok()
    }
}

/// What owns an unrecognized property, for [`is_editor_only_property`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum PropertyOwner<'a> {
    /// A root record of the scene surface: `scene` or `app_state`.
    Record(&'a str),
    /// An element of any kind.
    Element,
    /// A linear element's `startBinding` or `endBinding`.
    Binding,
}

/// Whether the unrecognized `path` under `owner` is editing state that an
/// Excalidraw release after 0.18.1 persists and that no static export reads:
/// locked multi-selections, a line's polygon toggle (the fill still follows
/// the closed path), midpoint dragging, and the binding mode and fixed point
/// of a non-elbow arrow. Ignoring one changes nothing in the artifact. Any
/// other unrecognized property may carry appearance a later release renders.
pub(crate) fn is_editor_only_property(owner: PropertyOwner<'_>, path: &str) -> bool {
    match owner {
        PropertyOwner::Record(record) => record == "app_state" && path == "/lockedMultiSelections",
        PropertyOwner::Element => matches!(path, "/polygon" | "/moveMidPointsWithElement"),
        PropertyOwner::Binding => matches!(path, "/mode" | "/fixedPoint"),
    }
}

/// An ignored unrecognized property: a warning that appearance, semantics or
/// metadata may differ, or, for editor-only state, an informational input
/// repair.
pub(crate) fn unrecognized_property_diagnostic(
    format: OutputFormat,
    element_id: Option<&str>,
    json_pointer: &str,
    editor_only: bool,
    message: &str,
) -> Result<PendingDiagnostic, TryReserveError> {
    if editor_only {
        return PendingDiagnostic::new(
            DiagnosticKind::EditorPropertyIgnored,
            format,
            &[DiagnosticEffect::InputRepair],
            element_id,
            json_pointer,
            "editor-only property was ignored",
        );
    }
    PendingDiagnostic::new(
        DiagnosticKind::UnsupportedProperty,
        format,
        &[
            DiagnosticEffect::Visual,
            DiagnosticEffect::Semantic,
            DiagnosticEffect::Metadata,
        ],
        element_id,
        json_pointer,
        message,
    )
}

pub(crate) fn classify_referenced_files(
    scene: &SceneDocument,
    policy: &'static SceneSurfacePolicy,
    referenced_file_ids: &[&str],
    format: OutputFormat,
    diagnostic_limit: usize,
) -> Result<Vec<SurfaceDiagnostic>, TryReserveError> {
    let mut diagnostics = Vec::new();
    for index in 0..scene.file_record_count() {
        if diagnostics.len() >= diagnostic_limit {
            break;
        }
        let Some(file) = scene.file_record_at(index) else {
            continue;
        };
        if referenced_file_ids.binary_search(&file.key()).is_err() {
            continue;
        }
        let pointer = pointer_child("/files", file.key())?;
        let source_order = scene.source_owner_order("files", index);
        let recovery = || malformed_file_recovery(scene, file.key(), format);
        if file.key().is_empty() {
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: 0,
                local_owner: Some(String::new()),
                diagnostic: malformed_file_record_diagnostic(
                    format,
                    file.key(),
                    &pointer,
                    recovery(),
                    "referenced authoritative file key was malformed",
                )?,
            });
            continue;
        }
        let Some(members) = file.members() else {
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: 0,
                local_owner: Some(try_owned(file.key())?),
                diagnostic: malformed_file_record_diagnostic(
                    format,
                    file.key(),
                    &pointer,
                    recovery(),
                    "referenced authoritative file record was malformed",
                )?,
            });
            continue;
        };
        if member_value(members, "dataURL").is_none() {
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: field_policy(policy, "file_record", "/dataURL")
                    .map_or(u64::MAX - 1, |field| {
                        u64::try_from(field.ordinal).unwrap_or(u64::MAX - 1)
                    }),
                local_owner: Some(try_owned(file.key())?),
                diagnostic: malformed_file_record_diagnostic(
                    format,
                    file.key(),
                    &pointer_child(&pointer, "dataURL")?,
                    recovery(),
                    "referenced authoritative file record had no payload",
                )?,
            });
        }
        for (position, member) in members.iter().enumerate() {
            if diagnostics.len() >= diagnostic_limit {
                break;
            }
            let semantic_path = pointer_child("", &member.key)?;
            if let Some(field) = field_policy(policy, "file_record", &semantic_path) {
                // File normalization owns both absent and invalid ID repairs.
                // Reporting present invalid IDs here as well duplicates the
                // same source-field occurrence and consumes two diagnostics.
                if semantic_path == "/id" {
                    continue;
                }
                if field.invalid.code.is_some()
                    && !value_matches_domain(&member.value, field.accepted_domain)
                {
                    let member_pointer = pointer_child(&pointer, &member.key)?;
                    let diagnostic = if semantic_path == "/dataURL"
                        && matches!(&member.value, JsonValue::String(url) if !url.starts_with("data:"))
                    {
                        forbidden_resource_diagnostic(
                            format,
                            file.key(),
                            &member_pointer,
                            referenced_image_has_safe_bounds(scene, file.key()),
                        )?
                    } else if field.invalid.code
                        == Some(DiagnosticCode::ASSET_MEDIA_TYPE_MISMATCH.as_str())
                    {
                        asset_media_type_diagnostic(
                            format,
                            file.key(),
                            &member_pointer,
                            match &member.value {
                                JsonValue::String(media_type) => Some(media_type.as_ref()),
                                _ => None,
                            },
                            None,
                        )?
                    } else if let Some(diagnostic) =
                        manifested_field_diagnostic(&field.invalid, format, &member_pointer, None)?
                    {
                        diagnostic
                    } else {
                        continue;
                    };
                    diagnostics.try_reserve(1)?;
                    diagnostics.push(SurfaceDiagnostic {
                        source_order,
                        source_position: u64::try_from(position).unwrap_or(u64::MAX),
                        local_owner: Some(try_owned(file.key())?),
                        diagnostic,
                    });
                }
                continue;
            }
            diagnostics.try_reserve(1)?;
            diagnostics.push(SurfaceDiagnostic {
                source_order,
                source_position: u64::try_from(position).unwrap_or(u64::MAX),
                local_owner: Some(try_owned(file.key())?),
                diagnostic: unrecognized_property_diagnostic(
                    format,
                    None,
                    &pointer_child(&pointer, &member.key)?,
                    false,
                    "unrecognized file property was ignored",
                )?,
            });
        }
    }
    Ok(diagnostics)
}

/// The format recovery of the image elements that refer to the malformed
/// file record `file_id`: PNG paints the official placeholder (the error one
/// when the first participating reference has status `error`), SVG and PDF omit the image.
fn malformed_file_recovery(
    scene: &SceneDocument,
    file_id: &str,
    format: OutputFormat,
) -> ImageRecovery {
    ImageRecovery::for_format(format, referenced_image_has_error_status(scene, file_id))
}

pub(crate) fn supplement_conflict(
    scene: &SceneDocument,
    file_id: &str,
    format: OutputFormat,
) -> Result<Option<SurfaceDiagnostic>, TryReserveError> {
    for index in 0..scene.file_record_count() {
        let Some(file) = scene.file_record_at(index) else {
            continue;
        };
        if file.key() != file_id {
            continue;
        }
        let pointer = pointer_child("/files", file_id)?;
        return Ok(Some(SurfaceDiagnostic {
            source_order: scene.source_owner_order("files", index),
            source_position: 0,
            local_owner: Some(try_owned(file_id)?),
            diagnostic: PendingDiagnostic::new(
                DiagnosticKind::SupplementConflict { asset_id: file_id },
                format,
                &[DiagnosticEffect::InputRepair],
                None,
                &pointer,
                "scene file remained authoritative over supplemental asset",
            )?,
        }));
    }
    Ok(None)
}

fn referenced_image_has_error_status(scene: &SceneDocument, file_id: &str) -> bool {
    (0..scene.element_occurrence_count()).find_map(|index| {
        if scene.element_is_silent(index) {
            return None;
        }
        let Some(JsonValue::Object(element)) = scene.element_value(index) else {
            return None;
        };
        (matches!(member_value(element, "type"), Some(JsonValue::String(kind)) if &**kind == "image")
            && matches!(member_value(element, "fileId"), Some(JsonValue::String(id)) if &**id == file_id))
            .then(|| matches!(member_value(element, "status"), Some(JsonValue::String(status)) if &**status == "error"))
    }).unwrap_or(false)
}

fn referenced_image_has_safe_bounds(scene: &SceneDocument, file_id: &str) -> bool {
    (0..scene.element_occurrence_count()).any(|index| {
        if scene.element_is_silent(index) { return false; }
        let Some(JsonValue::Object(element)) = scene.element_value(index) else {
            return false;
        };
        matches!(member_value(element, "type"), Some(JsonValue::String(kind)) if &**kind == "image")
            && matches!(member_value(element, "fileId"), Some(JsonValue::String(id)) if &**id == file_id)
            && ["width", "height"].into_iter().all(|field| {
                member_value(element, field)
                    .and_then(finite_json_number)
                    .is_some_and(|value| value != 0.0)
            })
    })
}

fn forbidden_resource_diagnostic(
    format: OutputFormat,
    asset_id: &str,
    json_pointer: &str,
    has_safe_bounds: bool,
) -> Result<PendingDiagnostic, TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::ForbiddenResource {
            asset_id,
            recovery: LocalRecovery::placeholder_if(has_safe_bounds),
        },
        format,
        &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic],
        None,
        json_pointer,
        "non-local image resource was rejected",
    )
}

fn malformed_file_record_diagnostic(
    format: OutputFormat,
    asset_id: &str,
    json_pointer: &str,
    recovery: ImageRecovery,
    message: &str,
) -> Result<PendingDiagnostic, TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::ImageUnavailable {
            failure: ImageFailure::MalformedFileRecord,
            asset_id,
            recovery,
        },
        format,
        &[DiagnosticEffect::Visual],
        None,
        json_pointer,
        message,
    )
}

pub(crate) fn asset_media_type_diagnostic(
    format: OutputFormat,
    asset_id: &str,
    json_pointer: &str,
    claimed_media_type: Option<&str>,
    validated_media_type: Option<&str>,
) -> Result<PendingDiagnostic, TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::MediaTypeMismatch {
            asset_id,
            claimed_media_type,
            validated_media_type,
        },
        format,
        &[DiagnosticEffect::InputRepair],
        None,
        json_pointer,
        if validated_media_type.is_some() {
            "asset media type did not match the validated payload type"
        } else {
            "asset media type was outside the revision-1 admitted domain"
        },
    )
}

#[derive(Clone, Copy)]
struct RecoverableBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
    opacity: f64,
}

fn recoverable_bounds(members: &[JsonMember]) -> Option<RecoverableBounds> {
    let optional_coordinate = |field| match member_value(members, field) {
        Some(value) => finite_json_number(value),
        None => Some(0.0),
    };
    let mut x = optional_coordinate("x")?;
    let mut y = optional_coordinate("y")?;
    let source_width = member_value(members, "width").and_then(finite_json_number)?;
    let source_height = member_value(members, "height").and_then(finite_json_number)?;
    if source_width == 0.0 || source_height == 0.0 {
        return None;
    }
    if source_width < 0.0 {
        x += source_width;
    }
    if source_height < 0.0 {
        y += source_height;
    }
    let angle = member_value(members, "angle")
        .and_then(finite_json_number)
        .unwrap_or(0.0);
    let opacity = member_value(members, "opacity")
        .and_then(finite_json_number)
        .filter(|opacity| (0.0..=100.0).contains(opacity))
        .unwrap_or(100.0);
    Some(RecoverableBounds {
        x: canonical_zero(x),
        y: canonical_zero(y),
        width: canonical_zero(source_width.abs()),
        height: canonical_zero(source_height.abs()),
        angle: canonical_zero(angle),
        opacity: canonical_zero(opacity),
    })
}

#[allow(clippy::too_many_arguments)]
fn surface_fallback(
    source_index: usize,
    source_order: u64,
    element_id: Option<&str>,
    json_pointer: &str,
    label: &str,
    link: Option<String>,
    frame_id: Option<&str>,
    is_frame: bool,
    bounds: RecoverableBounds,
) -> Result<SurfaceFallback, TryReserveError> {
    Ok(SurfaceFallback {
        source_index,
        source_order,
        element_id: element_id.map(try_owned).transpose()?,
        json_pointer: try_owned(json_pointer)?,
        label: try_owned(label)?,
        link,
        frame_id: frame_id.map(try_owned).transpose()?,
        is_frame,
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
        angle: bounds.angle,
        opacity: bounds.opacity,
        paint: true,
    })
}

fn fallback_link(members: &[JsonMember]) -> Result<Option<String>, TryReserveError> {
    match member_value(members, "link") {
        Some(JsonValue::String(link)) => sanitized_link(link),
        _ => Ok(None),
    }
}

fn fallback_frame_id(members: &[JsonMember]) -> Option<&str> {
    match member_value(members, "frameId") {
        Some(JsonValue::String(frame_id)) if !frame_id.is_empty() => Some(frame_id),
        _ => None,
    }
}

fn fallback_label(prefix: &str, kind: &str) -> Result<String, TryReserveError> {
    let mut label = String::new();
    label.try_reserve_exact(prefix.len().saturating_add(kind.len()))?;
    label.push_str(prefix);
    label.push_str(kind);
    Ok(label)
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

fn try_owned(value: &str) -> Result<String, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

/// The manifest's pseudo-field for a file record's map key (`file_record`
/// `/$key`). It describes the key of the `files` map entry, which the
/// classifier reports through its own asset-shaped constructors, and never a
/// member of the record: a member literally named `$key` is an unknown member
/// like any other (#156).
const FILE_RECORD_KEY_PSEUDO_FIELD: &str = "/$key";

fn field_policy<'a>(
    policy: &'a SceneSurfacePolicy,
    record_kind: &str,
    path: &str,
) -> Option<&'a SceneFieldPolicy> {
    if path == FILE_RECORD_KEY_PSEUDO_FIELD {
        return None;
    }
    let record = policy
        .record_kinds
        .iter()
        .find(|record| record.identity == record_kind)?;
    policy
        .fields
        .iter()
        .find(|field| field.record_kind == record_kind && field.path == path)
        .or_else(|| {
            record
                .inherits
                .iter()
                .find_map(|inherited| field_policy(policy, inherited, path))
        })
}

pub(crate) fn manifested_element_field<'a>(
    policy: &'a SceneSurfacePolicy,
    kind: &str,
    path: &str,
) -> Option<&'a SceneFieldPolicy> {
    let record_kind = if kind == "draw" {
        "element.legacy_draw"
    } else {
        policy
            .element_kinds
            .iter()
            .find(|element| element.kind == kind)?
            .record_kind
    };
    field_policy(policy, record_kind, path)
}

pub(crate) fn is_manifested_path(
    policy: &SceneSurfacePolicy,
    record_kind: &str,
    path: &str,
) -> bool {
    field_policy(policy, record_kind, path).is_some()
}

pub(crate) fn element_ignores_inherited_path(
    policy: &SceneSurfacePolicy,
    kind: &str,
    path: &str,
) -> bool {
    policy
        .element_kinds
        .iter()
        .find(|element| element.kind == kind)
        .is_some_and(|element| element.ignored_inherited_paths.contains(&path))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{css_color_is_admitted, SceneFieldDisposition, SCENE_SURFACE_POLICY};
    use crate::normalization::{
        codes_and_pointers, pointers, reported_diagnostics, NormalizedElementKind,
    };
    use crate::{
        CompatibilityDiagnostic, DiagnosticCode, DiagnosticDetails, DiagnosticEffect, OutputFormat,
        Recovery, SceneDocument, SupplementalAssets,
    };

    #[test]
    fn css_color_domain_rejects_malformed_functions_and_accepts_case_insensitively() {
        assert!(css_color_is_admitted("RGB(30 30 30)"));
        assert!(!css_color_is_admitted("rgb(future)"));
    }

    /// The generated table's shape: the manifest's counts, one field policy
    /// per `(record kind, path)` in manifest order, and the disposition
    /// split. Field-path ownership and registry-mapping uniqueness are the
    /// manifest's own invariants, checked by `xtask catalog verify`.
    #[test]
    fn generated_policy_has_the_manifest_shape() {
        let policy = &SCENE_SURFACE_POLICY;
        assert_eq!(policy.record_kinds.len(), 29);
        assert_eq!(policy.fields.len(), 202);
        assert_eq!(policy.element_kinds.len(), 13);

        let fields = policy
            .fields
            .iter()
            .enumerate()
            .map(|(ordinal, field)| {
                assert_eq!(field.ordinal, ordinal);
                (field.record_kind, field.path)
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(fields.len(), policy.fields.len());
        assert_eq!(
            policy
                .fields
                .iter()
                .filter(|field| field.disposition == SceneFieldDisposition::Supported)
                .count(),
            88
        );
        assert_eq!(
            policy
                .fields
                .iter()
                .filter(|field| {
                    field.disposition == SceneFieldDisposition::StaticExportIrrelevant
                })
                .count(),
            75
        );
        assert_eq!(
            policy
                .fields
                .iter()
                .filter(|field| field.disposition == SceneFieldDisposition::OpaqueMetadata)
                .count(),
            39
        );
    }

    /// The notice tables agree with each other and with the identities the
    /// asset catalog seals: every recipe owes only catalogued notices, every
    /// notice is required by at least one format, and the two tables list
    /// exactly the manifested recipes and notices.
    #[test]
    fn generated_notice_tables_close_over_the_manifest() {
        let policy = &SCENE_SURFACE_POLICY;
        let assets = crate::fonts::revision_1();
        assert_eq!(
            policy.fallback_recipes.len(),
            assets.fallback_recipe_identities.len()
        );
        assert_eq!(
            policy
                .notices
                .iter()
                .map(|notice| notice.identity)
                .collect::<Vec<_>>(),
            assets.notice_identities
        );
        for recipe in policy.fallback_recipes {
            assert!(!recipe.notices.is_empty(), "{}", recipe.content_kind);
            for identity in recipe.notices {
                assert!(policy.notice(identity).is_some(), "{identity}");
            }
            assert!(
                [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf]
                    .into_iter()
                    .any(|format| recipe.paints(format)),
                "{}",
                recipe.content_kind
            );
        }
        for notice in policy.notices {
            assert!(
                [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf]
                    .into_iter()
                    .any(|format| notice.requires(format)),
                "{}",
                notice.identity
            );
            assert!(
                !notice.text.ends_with(char::is_whitespace),
                "{}",
                notice.identity
            );
        }
        assert!(policy.notice("liberation-sans.ofl-full").is_some());
        assert!(policy.notice("liberation-sans").is_none());
    }

    // The revision-1 scene-surface policy, observed through normalization:
    // which unknown or malformed members produce which diagnostics, with
    // which recovery, in the order the report lists them. Nothing here
    // renders; the diagnostics are the ones the normalization observer
    // carries, ordered as the session orders them.

    /// The diagnostics a default SVG render of `source` would report from
    /// normalization, in report order.
    fn surface_diagnostics(source: &str) -> Vec<CompatibilityDiagnostic> {
        surface_diagnostics_for(source, OutputFormat::Svg)
    }

    /// [`surface_diagnostics`] for `format`.
    fn surface_diagnostics_for(source: &str, format: OutputFormat) -> Vec<CompatibilityDiagnostic> {
        let scene = SceneDocument::parse(source.as_bytes()).expect("policy vector is admitted");
        reported_diagnostics(&scene, format, true, &SupplementalAssets::default()).0
    }

    /// The recovery the diagnostic's compatibility or asset details declare.
    fn recovery_of(diagnostic: &CompatibilityDiagnostic) -> Recovery {
        match diagnostic.details() {
            Some(DiagnosticDetails::Compatibility { recovery, .. })
            | Some(DiagnosticDetails::Asset { recovery, .. }) => *recovery,
            details => panic!("unexpected details: {details:?}"),
        }
    }

    #[test]
    fn revision_1_global_policy_is_exact_and_suppresses_unknown_descendants() {
        let diagnostics = surface_diagnostics_for(
            r#"{
                "type":"excalidraw",
                "version":2,
                "scrollToContent":{"future":{"deep":true}},
                "libraryItems":{"future":{"deep":true}},
                "unknownRoot":{"first":1,"nested":{"second":2}},
                "elements":[],
                "appState":{
                    "activeTool":{"future":{"deep":true}},
                    "gridModeEnabled":"preserved but irrelevant",
                    "unknownAppState":{"first":1,"nested":{"second":2}}
                },
                "files":{}
            }"#,
            OutputFormat::Svg,
        );
        assert_eq!(
            codes_and_pointers(&diagnostics),
            [
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                    "/unknownRoot",
                ),
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                    "/appState/unknownAppState",
                ),
            ]
        );
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.effects()
                == [
                    DiagnosticEffect::Visual,
                    DiagnosticEffect::Semantic,
                    DiagnosticEffect::Metadata,
                ]
        }));
    }

    #[test]
    fn revision_1_element_policy_uses_exact_kind_and_record_boundaries() {
        // A deleted or selection element is silent whatever it carries; an
        // unknown kind and an empty kind take a placeholder; a non-object
        // element is omitted.
        let diagnostics = surface_diagnostics_for(
            r#"{
                "type":"excalidraw",
                "version":2,
                "elements":[
                    {"type":"future","isDeleted":true,"unknown":{"deep":true}},
                    {"type":"selection","unknown":{"deep":true}},
                    {"id":"unknown","type":"future","x":0,"y":0,"width":20,"height":10,"unknown":{"deep":true}},
                    {"id":"malformed","type":"","x":30,"y":0,"width":20,"height":10,"unknown":{"deep":true}},
                    null
                ],
                "appState":{},
                "files":{}
            }"#,
            OutputFormat::Svg,
        );
        assert_eq!(
            codes_and_pointers(&diagnostics),
            [
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_ELEMENT_TYPE,
                    "/elements/2",
                ),
                (
                    DiagnosticCode::COMPATIBILITY_MALFORMED_ELEMENT,
                    "/elements/3",
                ),
                (
                    DiagnosticCode::COMPATIBILITY_MALFORMED_ELEMENT,
                    "/elements/4",
                ),
            ]
        );
        assert_eq!(
            diagnostics.iter().map(recovery_of).collect::<Vec<_>>(),
            [
                Recovery::Placeholder,
                Recovery::Placeholder,
                Recovery::Omitted
            ]
        );
    }

    #[test]
    fn revision_1_file_participation_requires_a_manifested_image_reference() {
        // A rectangle's `fileId` is an unmatched property, not an image
        // reference, so the file record it names never participates and its
        // malformed members stay silent.
        assert_eq!(
            codes_and_pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw",
                    "version":2,
                    "elements":[{
                        "id":"rectangle",
                        "type":"rectangle",
                        "width":20,
                        "height":10,
                        "fileId":"asset",
                        "seed":1,
                        "updated":1
                    }],
                    "appState":{},
                    "files":{
                        "asset":{"id":3,"created":"invalid","unknown":{"deep":true}},
                        "unreferenced":false
                    }
                }"#
            )),
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                "/elements/0/fileId",
            )]
        );
    }

    #[test]
    fn revision_1_referenced_file_classifies_known_and_unknown_children() {
        // The key is escaped per RFC 6901 in every pointer; an unreferenced
        // record is silent however malformed.
        assert_eq!(
            codes_and_pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw",
                    "version":2,
                    "elements":[{
                        "id":"image",
                        "type":"image",
                        "width":20,
                        "height":10,
                        "fileId":"a/b~c",
                        "updated":1
                    }],
                    "appState":{},
                    "files":{
                        "a/b~c":{"not":"used"},
                        "unreferenced":false
                    }
                }"#
            )),
            [
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                    "/files/a~1b~0c/id",
                ),
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                    "/files/a~1b~0c/not",
                ),
                (
                    DiagnosticCode::ASSET_MALFORMED_FILE_RECORD,
                    "/files/a~1b~0c/dataURL",
                ),
                (
                    DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
                    "/files/a~1b~0c/created",
                ),
            ]
        );
    }

    #[test]
    fn revision_1_wholly_malformed_referenced_file_suppresses_descendants() {
        let diagnostics = surface_diagnostics_for(
            r#"{
                "type":"excalidraw","version":2,
                "elements":[{"id":"image","type":"image","width":20,"height":10,"fileId":"asset","updated":1}],
                "appState":{},"files":{"asset":false,"unreferenced":{"unknown":{"deep":true}}}
            }"#,
            OutputFormat::Svg,
        );
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("the malformed authoritative record is one occurrence: {diagnostics:?}");
        };
        assert_eq!(
            diagnostic.code(),
            DiagnosticCode::ASSET_MALFORMED_FILE_RECORD
        );
        assert_eq!(diagnostic.json_pointer(), "/files/asset");
        assert!(matches!(
            diagnostic.details(),
            Some(DiagnosticDetails::Asset {
                asset_id,
                recovery: Recovery::Omitted,
                ..
            }) if asset_id == "asset"
        ));
    }

    #[test]
    fn revision_1_empty_referenced_file_key_is_malformed() {
        assert_eq!(
            codes_and_pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw","version":2,
                    "elements":[{"id":"image","type":"image","width":20,"height":10,"fileId":"","updated":1}],
                    "appState":{},"files":{"":{"dataURL":"data:image/png;base64,AA=="}}
                }"#
            )),
            [(DiagnosticCode::ASSET_MALFORMED_FILE_RECORD, "/files/",)]
        );
    }

    #[test]
    fn revision_1_file_record_key_pseudo_field_never_matches_a_member() {
        // The manifest describes the `files` map key as the pseudo-field
        // `/$key`; a member literally named `$key` is an unknown member of the
        // record, ignored with `compatibility.unsupported-property`, not the
        // malformed key the pseudo-field's fallback describes (#156).
        let diagnostics = surface_diagnostics_for(
            r#"{
                "type":"excalidraw","version":2,
                "elements":[{"id":"image","type":"image","x":0,"y":0,"width":20,"height":10,"fileId":"asset","status":"saved","updated":1}],
                "appState":{},
                "files":{"asset":{"id":"asset","created":1,"mimeType":"image/png","$key":1,"dataURL":"data:image/png;base64,AA=="}}
            }"#,
            OutputFormat::Svg,
        );
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("the `$key` member is the one occurrence: {diagnostics:?}");
        };
        assert_eq!(
            diagnostic.code(),
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY
        );
        assert_eq!(diagnostic.json_pointer(), "/files/asset/$key");
        assert_eq!(
            diagnostic.details(),
            Some(&DiagnosticDetails::Compatibility {
                recovery: Recovery::Ignored,
                recipe_id: None,
            })
        );
        assert!(
            super::field_policy(&SCENE_SURFACE_POLICY, "file_record", "/$key").is_none(),
            "the pseudo-field is not a member path"
        );
        assert!(
            SCENE_SURFACE_POLICY
                .fields
                .iter()
                .any(|field| field.record_kind == "file_record" && field.path == "/$key"),
            "the generated table still carries the pseudo-field the manifest declares"
        );
    }

    #[test]
    fn revision_1_malformed_error_image_selects_the_error_recipe_in_png() {
        // PNG paints the official placeholder for a payload-less record, and
        // an image whose status is `error` selects the error recipe; SVG and
        // PDF omit the image instead.
        let source = r#"{
            "type":"excalidraw","version":2,
            "elements":[{"id":"image","type":"image","width":20,"height":10,"fileId":"asset","status":"error","updated":1}],
            "appState":{},"files":{"asset":false}
        }"#;
        let diagnostics = surface_diagnostics_for(source, OutputFormat::Png);
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("the malformed authoritative record is one occurrence: {diagnostics:?}");
        };
        assert!(matches!(
            diagnostic.details(),
            Some(DiagnosticDetails::Asset {
                recovery: Recovery::Placeholder,
                recipe_id: Some(recipe),
                ..
            }) if recipe.starts_with("official.image-error@")
        ));

        let diagnostics = surface_diagnostics_for(source, OutputFormat::Svg);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(recovery_of(&diagnostics[0]), Recovery::Omitted);
    }

    #[test]
    fn revision_1_file_values_use_asset_specific_manifest_recoveries() {
        let diagnostics = surface_diagnostics_for(
            r#"{
                "type":"excalidraw","version":2,
                "elements":[{"id":"image","type":"image","width":20,"height":10,"fileId":"asset","status":"saved","updated":1}],
                "appState":{},
                "files":{"asset":{"id":"asset","created":1,"mimeType":"image/future","dataURL":"https://example.com/image.png"}}
            }"#,
            OutputFormat::Svg,
        );
        assert_eq!(
            codes_and_pointers(&diagnostics),
            [
                (
                    DiagnosticCode::ASSET_MEDIA_TYPE_MISMATCH,
                    "/files/asset/mimeType",
                ),
                (
                    DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE,
                    "/files/asset/dataURL",
                ),
            ]
        );
        assert!(diagnostics.iter().all(|diagnostic| {
            matches!(
                diagnostic.details(),
                Some(DiagnosticDetails::Asset { asset_id, .. }) if asset_id == "asset"
            )
        }));
    }

    #[test]
    fn revision_1_frame_rendering_keeps_unknown_children_separate() {
        assert_eq!(
            pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw",
                    "version":2,
                    "elements":[],
                    "appState":{"frameRendering":{
                        "enabled":true,
                        "firstUnknown":{"deep":true},
                        "secondUnknown":{"deep":true}
                    }},
                    "files":{}
                }"#
            )),
            [
                "/appState/frameRendering/firstUnknown",
                "/appState/frameRendering/secondUnknown",
            ]
        );
    }

    #[test]
    fn revision_1_frame_children_precede_later_app_state_members() {
        assert_eq!(
            pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw","version":2,"elements":[],
                    "appState":{
                        "frameRendering":{"unknown":{"deep":true}},
                        "laterUnknown":{"deep":true}
                    },
                    "files":{}
                }"#
            )),
            ["/appState/frameRendering/unknown", "/appState/laterUnknown"]
        );
    }

    #[test]
    fn revision_1_linear_nested_records_classify_the_narrowest_unknown_child() {
        assert_eq!(
            codes_and_pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw",
                    "version":2,
                    "elements":[
                        {
                            "id":"line",
                            "type":"line",
                            "width":10,
                            "height":10,
                            "points":[[0,0,99],[10,10]],
                            "startBinding":{"elementId":"target","focus":0,"gap":0,"unknown":{"deep":true}},
                            "seed":1,
                            "updated":1
                        },
                        {"id":"target","type":"rectangle","width":10,"height":10,"seed":1,"updated":1}
                    ],
                    "appState":{},
                    "files":{}
                }"#
            )),
            [
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                    "/elements/0/points/0/2",
                ),
                (
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                    "/elements/0/startBinding/unknown",
                ),
            ]
        );
    }

    #[test]
    fn revision_1_participating_bound_record_classifies_unknown_children() {
        assert_eq!(
            codes_and_pointers(&surface_diagnostics(
                r#"{
                    "type":"excalidraw","version":2,
                    "elements":[
                        {"id":"owner","type":"rectangle","width":20,"height":10,"boundElements":[{"id":"target","type":"text","unknown":{"deep":true}}],"seed":1,"updated":1},
                        {"id":"target","type":"text","updated":1}
                    ],
                    "appState":{},"files":{}
                }"#
            )),
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY,
                "/elements/0/boundElements/0/unknown",
            )]
        );
    }

    #[test]
    fn revision_1_supported_value_uses_its_manifested_recovery() {
        // A `strokeWidth` outside its domain is substituted by the default
        // the manifest declares, so the element normalizes as if it had said
        // `2`, and the substitution is a visible input repair.
        let scene = SceneDocument::parse(
            br##"{
                "type":"excalidraw","version":2,
                "elements":[{"id":"rectangle","type":"rectangle","width":20,"height":10,"strokeWidth":0,"seed":1,"updated":1}],
                "appState":{},"files":{}
            }"##,
        )
        .expect("policy vector is admitted");
        let (diagnostics, normalized) = reported_diagnostics(
            &scene,
            OutputFormat::Svg,
            true,
            &SupplementalAssets::default(),
        );
        assert_eq!(normalized.elements[0].stroke_width, 2.0);
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("invalid supported value has one manifested recovery: {diagnostics:?}");
        };
        assert_eq!(
            diagnostic.code(),
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE
        );
        assert_eq!(diagnostic.json_pointer(), "/elements/0/strokeWidth");
        assert_eq!(
            diagnostic.effects(),
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual]
        );
    }

    #[test]
    fn revision_1_invalid_css_color_uses_the_manifested_color_recovery() {
        let scene = SceneDocument::parse(
            br##"{
                "type":"excalidraw","version":2,
                "elements":[{"id":"rectangle","type":"rectangle","width":20,"height":10,"strokeColor":"not-a-color","seed":1,"updated":1}],
                "appState":{},"files":{}
            }"##,
        )
        .expect("policy vector is admitted");
        let (diagnostics, normalized) = reported_diagnostics(
            &scene,
            OutputFormat::Svg,
            true,
            &SupplementalAssets::default(),
        );
        assert_eq!(normalized.elements[0].stroke_color, "#1e1e1e");
        assert_eq!(
            codes_and_pointers(&diagnostics),
            [(DiagnosticCode::COLOR_INVALID, "/elements/0/strokeColor")]
        );
    }

    #[test]
    fn malformed_colors_use_field_specific_substitutions_in_source_order() {
        let scene = SceneDocument::parse(
            br##"{
                "type":"excalidraw",
                "version":2,
                "elements":[{
                    "id":"invalid-colors",
                    "type":"rectangle",
                    "width":20,
                    "height":10,
                    "strokeColor":"rgb(1, 2 3)",
                    "backgroundColor":"color(invalid 1 1 1)",
                    "seed":1,
                    "updated":1
                }],
                "appState":{"viewBackgroundColor":"rgb(future)"},
                "files":{}
            }"##,
        )
        .expect("policy vector is admitted");
        let (diagnostics, normalized) = reported_diagnostics(
            &scene,
            OutputFormat::Svg,
            true,
            &SupplementalAssets::default(),
        );
        assert_eq!(normalized.elements[0].stroke_color, "#1e1e1e");
        assert_eq!(normalized.elements[0].background_color, "transparent");
        assert_eq!(normalized.view_background_color, "#ffffff");
        assert_eq!(
            codes_and_pointers(&diagnostics),
            [
                (DiagnosticCode::COLOR_INVALID, "/elements/0/strokeColor"),
                (DiagnosticCode::COLOR_INVALID, "/elements/0/backgroundColor"),
                (
                    DiagnosticCode::COLOR_INVALID,
                    "/appState/viewBackgroundColor"
                ),
            ]
        );
    }

    #[test]
    fn revision_1_known_unavailable_kind_still_applies_exact_field_policy() {
        // A text element with no text is unavailable to the renderer, but its
        // malformed `fontSize` is still diagnosed by the field policy.
        let scene = SceneDocument::parse(
            br#"{
                "type":"excalidraw","version":2,
                "elements":[{"id":"text","type":"text","fontSize":0,"updated":1}],
                "appState":{},"files":{}
            }"#,
        )
        .expect("policy vector is admitted");
        let (diagnostics, normalized) = reported_diagnostics(
            &scene,
            OutputFormat::Svg,
            true,
            &SupplementalAssets::default(),
        );
        let NormalizedElementKind::Text(text) = &normalized.elements[0].kind else {
            panic!("the element retains its normalized text kind");
        };
        assert_eq!(text.font_size, 20.0);
        assert_eq!(
            codes_and_pointers(&diagnostics),
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/fontSize",
            )]
        );
    }

    #[test]
    fn revision_1_value_rules_are_format_and_kind_aware() {
        // An unsafe link is omitted, and diagnosed, only where a link can
        // reach the artifact: SVG carries links, PNG does not. Every
        // diagnostic names the format it was produced for.
        let source = r#"{
            "type":"excalidraw","version":2,
            "elements":[{"id":"diamond","type":"diamond","width":20,"height":10,"seed":0,"link":"javascript:alert(1)","updated":1}],
            "appState":{},"files":{}
        }"#;
        for (format, expected_codes) in [
            (
                OutputFormat::Svg,
                vec![
                    DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
                    DiagnosticCode::LINK_UNSAFE_OMITTED,
                ],
            ),
            (
                OutputFormat::Png,
                vec![DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED],
            ),
            (
                OutputFormat::Pdf,
                vec![
                    DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
                    DiagnosticCode::LINK_UNSAFE_OMITTED,
                ],
            ),
        ] {
            let diagnostics = surface_diagnostics_for(source, format);
            assert_eq!(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code())
                    .collect::<Vec<_>>(),
                expected_codes,
                "{format:?}"
            );
            assert!(
                diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.format() == format),
                "{format:?}"
            );
        }
    }
}
