//! Working-byte and work-unit planning for normalization: the §13.2 ledger rows
//! a normalized element will own, computed from the source before the element
//! is built so the preflight record can be observed first, and the measurement
//! of the built kind the plan must equal exactly. Value normalization lives in
//! the parent and in `kinds`; this file only prices it. A field's default is
//! contract behavior there and accounting behavior here.

use std::collections::TryReserveError;

use crate::scene::{BindingSource, ElementKind, ElementSource, SourceField};

use super::kinds::{embeddable_label, is_arrowhead, normalized_image_crop, pressures_are_valid};
use super::{
    defaulted, CommonFields, NormalizationParticipation, NormalizedBinding, NormalizedElementKind,
    NormalizedLinearKind,
};

const NORMALIZED_FIELD_COUNT: u64 = 14;
const SCALAR_REFERENCE_FIELD_COUNT: u64 = 10;
pub(super) const GENERATED_ID_BYTES: usize = "generated-".len() + 64;
pub(crate) const NORMALIZATION_WORK_UNITS: u64 = NORMALIZED_FIELD_COUNT * 8;

impl ElementKind {
    pub(super) const fn normalization_event_name(self) -> &'static str {
        match self {
            Self::Rectangle => "normalized_rectangle",
            Self::Diamond => "normalized_diamond",
            Self::Ellipse => "normalized_ellipse",
            Self::Text => "normalized_text",
            Self::Line | Self::LegacyDraw | Self::Arrow => "normalized_linear",
            Self::Freedraw => "normalized_freedraw",
            Self::Image => "normalized_image",
            Self::Frame | Self::MagicFrame => "normalized_frame",
            Self::Iframe | Self::Embeddable => "normalized_embeddable",
        }
    }

    pub(super) fn normalized_field_count(self, participation: NormalizationParticipation) -> u64 {
        match self {
            Self::Rectangle | Self::Diamond | Self::Ellipse => 0,
            Self::Text => 6,
            Self::Line | Self::LegacyDraw => 6,
            Self::Arrow => 7,
            Self::Freedraw => 4,
            Self::Image => 3 + u64::from(participation.authoritative_file),
            Self::Frame | Self::MagicFrame => participation.frame_names as u64,
            Self::Iframe | Self::Embeddable => 0,
        }
    }
}

/// The working bytes the normalized kind will own, planned before the kind is
/// built so the preflight record can be observed first. The plan must equal the
/// actual measurement exactly; where the kind owns derived text, the derivation
/// runs here too and its allocation failure is the caller's to report.
pub(super) fn planned_kind_working_bytes(
    source: &ElementSource<'_>,
    participation: NormalizationParticipation,
) -> Result<u64, TryReserveError> {
    let optional_owned_bytes =
        |value: Option<&str>| value.map_or(64, |value| owned_field_bytes(value.len()));
    Ok(match source.kind {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => 0,
        ElementKind::Iframe => owned_field_bytes("IFrame element".len()),
        ElementKind::Embeddable => owned_field_bytes(embeddable_label(source.link)?.len()),
        ElementKind::Text => {
            let text = match source.text {
                SourceField::Value(text) => text,
                SourceField::Missing | SourceField::Invalid => "",
            };
            owned_field_bytes(text.len()).saturating_add(5 * 64)
        }
        ElementKind::Line | ElementKind::LegacyDraw | ElementKind::Arrow => {
            let point_count = match source.points {
                SourceField::Value(points)
                    if points.len() >= 2
                        && (0..points.len())
                            .all(|index| matches!(points.point(index), SourceField::Value(_))) =>
                {
                    points.len()
                }
                SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => 2,
            };
            let arrow = matches!(source.kind, ElementKind::Arrow);
            let elbowed = arrow && defaulted(source.elbowed, false).unwrap_or(false);
            u64::try_from(point_count)
                .unwrap_or(u64::MAX)
                .saturating_mul(2 * 64)
                .saturating_add(3 * 64)
                .saturating_add(optional_owned_bytes(planned_arrowhead(
                    source.start_arrowhead,
                    None,
                )))
                .saturating_add(optional_owned_bytes(planned_arrowhead(
                    source.end_arrowhead,
                    arrow.then_some("arrow"),
                )))
                .saturating_add(u64::from(arrow) * 64)
                .saturating_add(planned_binding_working_bytes(source.start_binding, elbowed))
                .saturating_add(planned_binding_working_bytes(source.end_binding, elbowed))
        }
        ElementKind::Freedraw => {
            let point_count = match source.points {
                SourceField::Value(points)
                    if (0..points.len())
                        .all(|index| matches!(points.point(index), SourceField::Value(_))) =>
                {
                    points.len()
                }
                SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => 0,
            };
            let simulate_pressure = defaulted(source.simulate_pressure, true).unwrap_or(true);
            let source_point_count = match source.points {
                SourceField::Value(points) => points.len(),
                SourceField::Missing | SourceField::Invalid => point_count,
            };
            let pressure_count = match source.pressures {
                SourceField::Value(pressures)
                    if pressures_are_valid(
                        source.pressures,
                        source_point_count,
                        simulate_pressure,
                    ) =>
                {
                    pressures.len()
                }
                SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => 0,
            };
            u64::try_from(point_count)
                .unwrap_or(u64::MAX)
                .saturating_mul(2 * 64)
                .saturating_add(
                    u64::try_from(pressure_count)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(64),
                )
                .saturating_add(2 * 64)
        }
        ElementKind::Image => {
            let file_id = match source.file_id {
                SourceField::Value(Some(file_id)) => Some(file_id),
                SourceField::Missing | SourceField::Invalid | SourceField::Value(None) => None,
            };
            optional_owned_bytes(file_id)
                .saturating_add(2 * 64)
                .saturating_add(u64::from(participation.authoritative_file) * 64)
                .saturating_add(
                    u64::from(
                        participation.authoritative_file
                            && normalized_image_crop(source.image_crop).is_some(),
                    ) * 6
                        * 64,
                )
        }
        ElementKind::Frame | ElementKind::MagicFrame if participation.frame_names => {
            let name = match source.name {
                SourceField::Value(Some(name)) => Some(name),
                SourceField::Missing | SourceField::Invalid | SourceField::Value(None) => None,
            };
            optional_owned_bytes(name)
        }
        ElementKind::Frame | ElementKind::MagicFrame => 0,
    })
}

fn planned_arrowhead<'a>(
    source: SourceField<Option<&'a str>>,
    default: Option<&'a str>,
) -> Option<&'a str> {
    match source {
        SourceField::Value(Some(value)) if is_arrowhead(value) => Some(value),
        SourceField::Value(None) => None,
        SourceField::Missing | SourceField::Invalid | SourceField::Value(Some(_)) => default,
    }
}

fn planned_binding_working_bytes(
    source: SourceField<Option<BindingSource<'_>>>,
    elbowed: bool,
) -> u64 {
    let SourceField::Value(Some(source)) = source else {
        return 0;
    };
    let fixed_point_bytes = if elbowed {
        match source.fixed_point() {
            SourceField::Missing => return 0,
            SourceField::Value(_) | SourceField::Invalid => 2 * 64,
        }
    } else {
        0
    };
    let SourceField::Value(element_id) = source.element_id() else {
        return 0;
    };
    if element_id.is_empty()
        || matches!(source.gap(), SourceField::Invalid)
        || matches!(source.focus(), SourceField::Invalid)
    {
        return 0;
    }
    owned_field_bytes(element_id.len())
        .saturating_add(2 * 64)
        .saturating_add(fixed_point_bytes)
}

pub(super) fn kind_working_bytes(
    kind: &NormalizedElementKind,
    participation: NormalizationParticipation,
) -> u64 {
    let optional_owned_bytes =
        |value: Option<&str>| value.map_or(64, |value| owned_field_bytes(value.len()));
    let binding_bytes = |binding: Option<&NormalizedBinding>| {
        binding.map_or(0, |binding| {
            owned_field_bytes(binding.element_id.len())
                .saturating_add(2 * 64)
                .saturating_add(u64::from(binding.fixed_point.is_some()) * 2 * 64)
        })
    };
    match kind {
        NormalizedElementKind::Rectangle
        | NormalizedElementKind::Diamond
        | NormalizedElementKind::Ellipse => 0,
        NormalizedElementKind::Embeddable { label, .. } => owned_field_bytes(label.len()),
        NormalizedElementKind::Text(text) => {
            owned_field_bytes(text.text.len()).saturating_add(5 * 64)
        }
        NormalizedElementKind::Linear(linear) => u64::try_from(linear.points.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(2 * 64)
            .saturating_add(3 * 64)
            .saturating_add(optional_owned_bytes(linear.start_arrowhead.as_deref()))
            .saturating_add(optional_owned_bytes(linear.end_arrowhead.as_deref()))
            .saturating_add(u64::from(matches!(linear.kind, NormalizedLinearKind::Arrow)) * 64)
            .saturating_add(binding_bytes(linear.start_binding.as_ref()))
            .saturating_add(binding_bytes(linear.end_binding.as_ref())),
        NormalizedElementKind::Freedraw(freedraw) => u64::try_from(freedraw.points.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(2 * 64)
            .saturating_add(
                u64::try_from(freedraw.pressures.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(64),
            )
            .saturating_add(2 * 64),
        NormalizedElementKind::Image(image) => optional_owned_bytes(image.file_id.as_deref())
            .saturating_add(2 * 64)
            .saturating_add(u64::from(participation.authoritative_file) * 64)
            .saturating_add(u64::from(image.crop.is_some()) * 6 * 64),
        NormalizedElementKind::Frame(frame) | NormalizedElementKind::MagicFrame(frame) => {
            if participation.frame_names {
                optional_owned_bytes(frame.name.as_deref())
            } else {
                0
            }
        }
    }
}

pub(super) fn normalized_working_bytes(id_bytes: usize, fields: &CommonFields<'_>) -> u64 {
    256 + SCALAR_REFERENCE_FIELD_COUNT * 64
        + owned_field_bytes(id_bytes)
        + owned_field_bytes(fields.stroke_color.len())
        + owned_field_bytes(fields.background_color.len())
        + owned_field_bytes(fields.fill_style.len())
        + owned_field_bytes(fields.stroke_style.len())
}

pub(super) fn owned_field_bytes(bytes: usize) -> u64 {
    64 + u64::try_from(bytes).unwrap_or(u64::MAX).saturating_add(63) / 64 * 64
}
