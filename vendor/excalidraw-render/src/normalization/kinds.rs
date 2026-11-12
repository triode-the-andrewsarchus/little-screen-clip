//! Per-kind value normalization: the common fields every element shares, and
//! one `normalize_*` function per element kind that diagnoses the kind's
//! malformed members and settles the values the reference renderer's restore
//! path would give them. What these values cost the ledger is priced in
//! `planning`, which reads the same predicates so the plan equals the build.

use std::collections::TryReserveError;

use crate::diagnostic_registry::{DiagnosticKind, FallbackRecipe};
use crate::font_metadata::FamilyMetrics;
use crate::outcome::PendingDiagnostic;
use crate::scene::{
    pointer_child, pointer_index, BindingSource, ElementKind, ElementSource, ImageCropSource,
    NumberArraySource, PointArraySource, SourceField,
};
use crate::scene_surface::SceneSurfacePolicy;
use crate::{DiagnosticEffect, OutputFormat, Recovery};

use super::{
    canonical_zero, defaulted, manifested_value, nested_path_ordinal, source_accepts, try_owned,
    CommonFields, ElementDiagnostic, ElementDiagnosticContext, NormalizationParticipation,
    NormalizedBinding, NormalizedElementKind, NormalizedFrame, NormalizedFreedraw, NormalizedImage,
    NormalizedImageCrop, NormalizedImageStatus, NormalizedLinear, NormalizedLinearKind,
    NormalizedRoundness, NormalizedText, SourceDiagnosticPosition,
};

pub(super) fn element_fields<'a>(
    source: &ElementSource<'a>,
    policy: &'static SceneSurfacePolicy,
) -> Option<CommonFields<'a>> {
    let ignored = |path| {
        crate::scene_surface::element_ignores_inherited_path(
            policy,
            source.kind.scene_surface_kind(),
            path,
        )
    };
    let mut x = manifested_value(source.x, 0.0, |_| true);
    let mut y = manifested_value(source.y, 0.0, |_| true);
    let mut width = manifested_value(source.width, 0.0, |_| true);
    let mut height = manifested_value(source.height, 0.0, |_| true);
    if !matches!(
        source.kind,
        ElementKind::Line | ElementKind::LegacyDraw | ElementKind::Arrow
    ) {
        if width < 0.0 {
            x += width;
        }
        width = width.abs();
        if height < 0.0 {
            y += height;
        }
        height = height.abs();
    }
    Some(CommonFields {
        x: canonical_zero(x),
        y: canonical_zero(y),
        width: canonical_zero(width),
        height: canonical_zero(height),
        angle: canonical_zero(manifested_value(source.angle, 0.0, |_| true)),
        stroke_color: if ignored("/strokeColor") {
            "#1e1e1e"
        } else {
            manifested_value(source.stroke_color, "#1e1e1e", |value| {
                crate::scene_surface::css_color_is_admitted(value)
            })
        },
        background_color: if ignored("/backgroundColor") {
            "transparent"
        } else {
            manifested_value(source.background_color, "transparent", |value| {
                crate::scene_surface::css_color_is_admitted(value)
            })
        },
        fill_style: if ignored("/fillStyle") {
            "solid"
        } else {
            // The seven fillers Rough.js 4.6.4 dispatches; the reference passes
            // the element's value through unvalidated (#137).
            manifested_value(source.fill_style, "solid", |value| {
                matches!(
                    value,
                    "hachure"
                        | "cross-hatch"
                        | "solid"
                        | "zigzag"
                        | "zigzag-line"
                        | "dots"
                        | "dashed"
                )
            })
        },
        stroke_width: if ignored("/strokeWidth") {
            2.0
        } else {
            manifested_value(source.stroke_width, 2.0, |value| value > 0.0)
        },
        stroke_style: if ignored("/strokeStyle") {
            "solid"
        } else {
            manifested_value(source.stroke_style, "solid", |value| {
                matches!(value, "solid" | "dashed" | "dotted")
            })
        },
        roughness: if ignored("/roughness") {
            1.0
        } else {
            manifested_value(source.roughness, 1.0, |value| value >= 0.0)
        },
        opacity: manifested_value(source.opacity, 100.0, |value| {
            (0.0..=100.0).contains(&value)
        }),
        source_seed: if ignored("/seed") {
            1
        } else {
            manifested_value(source.seed, 1, |_| true)
        },
    })
}

/// The compatibility-normalized `x`, `y`, `width` and `height` of one element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ElementBox {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

impl ElementBox {
    /// The box the common fields settled: the source box with negative
    /// extents folded back, which every kind but linear keeps.
    const fn of(fields: &CommonFields<'_>) -> Self {
        Self {
            x: fields.x,
            y: fields.y,
            width: fields.width,
            height: fields.height,
        }
    }
}

/// Normalizes one element's kind-specific state and settles its box. Every
/// kind first diagnoses the inherited fields it shares with the others; linear
/// elements then move the box to their first point and recompute its extent,
/// while every other kind keeps the common fields' box.
#[allow(clippy::too_many_arguments)]
pub(super) fn normalize_element_kind(
    source: &ElementSource<'_>,
    fields: &CommonFields<'_>,
    policy: &'static SceneSurfacePolicy,
    element_pointer: &str,
    element_id: &str,
    format: OutputFormat,
    participation: NormalizationParticipation,
    diagnostic_limit: usize,
    diagnostics: &mut Vec<ElementDiagnostic>,
) -> Result<(NormalizedElementKind, ElementBox), TryReserveError> {
    let mut context = ElementDiagnosticContext {
        diagnostics,
        source,
        element_pointer,
        element_id,
        format,
        policy,
        diagnostic_limit,
    };
    diagnose_common_fields(&mut context)?;
    let kind = match source.kind {
        ElementKind::Rectangle => NormalizedElementKind::Rectangle,
        ElementKind::Diamond => NormalizedElementKind::Diamond,
        ElementKind::Ellipse => NormalizedElementKind::Ellipse,
        ElementKind::Text => NormalizedElementKind::Text(normalize_text(fields, &mut context)?),
        ElementKind::Line | ElementKind::LegacyDraw | ElementKind::Arrow => {
            let (linear, element_box) = normalize_linear(fields, &mut context)?;
            return Ok((NormalizedElementKind::Linear(linear), element_box));
        }
        ElementKind::Freedraw => NormalizedElementKind::Freedraw(normalize_freedraw(&mut context)?),
        ElementKind::Image => {
            NormalizedElementKind::Image(normalize_image(participation, &mut context)?)
        }
        ElementKind::Frame => {
            NormalizedElementKind::Frame(normalize_frame(participation, &mut context)?)
        }
        ElementKind::MagicFrame => {
            NormalizedElementKind::MagicFrame(normalize_frame(participation, &mut context)?)
        }
        ElementKind::Iframe | ElementKind::Embeddable => normalize_embeddable(source)?,
    };
    Ok((kind, ElementBox::of(fields)))
}

/// Diagnoses the malformed inherited fields every kind shares — the box,
/// angle, style, seed, deletion and frame membership — and the `roundness`
/// record's members, unless the scene-surface policy ignores the path for the
/// kind. Their values are already settled in the common fields.
fn diagnose_common_fields(
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<(), TryReserveError> {
    let source = context.source;
    let policy = context.policy;
    macro_rules! diagnose_common {
        ($field:literal, $invalid:expr) => {
            if $invalid
                && !crate::scene_surface::element_ignores_inherited_path(
                    policy,
                    source.kind.scene_surface_kind(),
                    concat!("/", $field),
                )
            {
                context.unsupported($field, &[], Recovery::Substituted, None)?;
            }
        };
    }
    diagnose_common!("x", matches!(source.x, SourceField::Invalid));
    diagnose_common!("y", matches!(source.y, SourceField::Invalid));
    diagnose_common!("width", matches!(source.width, SourceField::Invalid));
    diagnose_common!("height", matches!(source.height, SourceField::Invalid));
    diagnose_common!("angle", matches!(source.angle, SourceField::Invalid));
    diagnose_common!(
        "fillStyle",
        !matches!(
            source.fill_style,
            SourceField::Missing
                | SourceField::Value(
                    "hachure"
                        | "cross-hatch"
                        | "solid"
                        | "zigzag"
                        | "zigzag-line"
                        | "dots"
                        | "dashed"
                )
        )
    );
    diagnose_common!(
        "strokeWidth",
        !source_accepts(source.stroke_width, |value| value > 0.0)
    );
    diagnose_common!(
        "strokeStyle",
        !matches!(
            source.stroke_style,
            SourceField::Missing | SourceField::Value("solid" | "dashed" | "dotted")
        )
    );
    diagnose_common!(
        "roughness",
        !source_accepts(source.roughness, |value| value >= 0.0)
    );
    diagnose_common!(
        "opacity",
        !source_accepts(source.opacity, |value| (0.0..=100.0).contains(&value))
    );
    diagnose_common!("seed", matches!(source.seed, SourceField::Invalid));
    diagnose_common!(
        "isDeleted",
        matches!(source.is_deleted, SourceField::Invalid)
    );
    diagnose_common!("frameId", matches!(source.frame_id, SourceField::Invalid));
    let roundness_ignored = crate::scene_surface::element_ignores_inherited_path(
        policy,
        source.kind.scene_surface_kind(),
        "/roundness",
    );
    if !roundness_ignored {
        match source.roundness {
            SourceField::Invalid => context.unsupported(
                "roundness",
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Substituted,
                None,
            )?,
            SourceField::Value(Some(roundness)) => {
                for (position, member) in roundness.members().iter().enumerate() {
                    let semantic_path = pointer_child("", &member.key)?;
                    if !crate::scene_surface::is_manifested_path(
                        policy,
                        "nested.roundness",
                        &semantic_path,
                    ) {
                        context.unsupported_property_nested("roundness", &member.key, position)?;
                    }
                }
                if !matches!(
                    roundness.kind(),
                    SourceField::Missing | SourceField::Value(1..=3)
                ) {
                    context.unsupported_nested(
                        "roundness",
                        "type",
                        roundness.member_position("type"),
                        None,
                        &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                        Recovery::Substituted,
                    )?;
                }
                let value_is_valid = match roundness.value() {
                    SourceField::Missing => true,
                    SourceField::Value(value) => value >= 0.0,
                    SourceField::Invalid => false,
                };
                if !value_is_valid {
                    context.unsupported_nested(
                        "roundness",
                        "value",
                        roundness.member_position("value"),
                        None,
                        &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                        Recovery::Substituted,
                    )?;
                }
            }
            SourceField::Missing | SourceField::Value(None) => {}
        }
    }
    Ok(())
}

fn normalize_text(
    fields: &CommonFields<'_>,
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<NormalizedText, TryReserveError> {
    let source = context.source;
    let format = context.format;
    if matches!(source.container_id, SourceField::Invalid) {
        context.unsupported(
            "containerId",
            &[
                DiagnosticEffect::InputRepair,
                DiagnosticEffect::Visual,
                DiagnosticEffect::Semantic,
            ],
            Recovery::Substituted,
            None,
        )?;
    }
    if !source_accepts(source.font_size, |value| value > 0.0) {
        context.unsupported(
            "fontSize",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    if !source_accepts(source.font_family, |value| {
        matches!(value, 1 | 2 | 3 | 5 | 6 | 7 | 8 | 9)
    }) {
        context.unsupported(
            "fontFamily",
            &[
                DiagnosticEffect::InputRepair,
                DiagnosticEffect::Visual,
                DiagnosticEffect::Semantic,
            ],
            Recovery::Substituted,
            None,
        )?;
    }
    if matches!(source.text, SourceField::Invalid) {
        context.unsupported(
            "text",
            &[
                DiagnosticEffect::InputRepair,
                DiagnosticEffect::Visual,
                DiagnosticEffect::Semantic,
            ],
            Recovery::Substituted,
            None,
        )?;
    }
    if !matches!(
        source.text_align,
        SourceField::Missing | SourceField::Value("left" | "center" | "right")
    ) {
        context.unsupported(
            "textAlign",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    if !matches!(
        source.vertical_align,
        SourceField::Missing | SourceField::Value("top" | "middle" | "bottom")
    ) {
        context.unsupported(
            "verticalAlign",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    if !source_accepts(source.line_height, |value| value > 0.0) {
        context.unsupported(
            "lineHeight",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    let text = match source.text {
        SourceField::Value(text) => text,
        SourceField::Missing | SourceField::Invalid => "",
    };
    let original_text = match (format, source.original_text) {
        (OutputFormat::Svg, SourceField::Value(original_text)) => original_text,
        (OutputFormat::Svg, SourceField::Missing | SourceField::Invalid) => text,
        (OutputFormat::Png | OutputFormat::Pdf, _) => "",
    };
    if matches!(source.original_text, SourceField::Invalid) {
        context.unsupported(
            "originalText",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    let source_font_size = manifested_value(source.font_size, 20.0, |value| value > 0.0);
    let source_font_family = manifested_value(source.font_family, 5, |value| {
        matches!(value, 1 | 2 | 3 | 5 | 6 | 7 | 8 | 9)
    });
    // `if ("font" in element)`: a legacy `font` string overrides both
    // modern fields whenever the element carries one. A value that is
    // not a string, or a string without a usable size, takes the
    // manifest's `/font` recovery: the independent modern fields.
    let legacy_font = match source.font {
        SourceField::Value(font) => legacy_font_migration(font),
        SourceField::Missing | SourceField::Invalid => None,
    };
    let font_is_malformed = legacy_font.is_none() && !matches!(source.font, SourceField::Missing);
    if font_is_malformed {
        context.unsupported(
            "font",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    let (font_size, font_family) = legacy_font.unwrap_or((source_font_size, source_font_family));
    // The restore derives lineHeight from the element's own fontSize
    // and fontFamily before the migration lands, so the source fields
    // decide it; only a fontSize the reference would divide NaN by
    // gives way to the normalized size.
    let line_height_font_size = manifested_value(source.font_size, font_size, |value| value > 0.0);
    let line_height = match source.line_height {
        SourceField::Value(value) if value > 0.0 => value,
        SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => {
            restored_line_height(
                fields.height,
                text,
                line_height_font_size,
                source_font_family,
            )
        }
    };
    Ok(NormalizedText {
        font_size,
        font_family,
        text: try_owned(text)?,
        original_text: try_owned(original_text)?,
        text_align: try_owned(manifested_value(source.text_align, "left", |value| {
            matches!(value, "left" | "center" | "right")
        }))?,
        vertical_align: try_owned(manifested_value(source.vertical_align, "top", |value| {
            matches!(value, "top" | "middle" | "bottom")
        }))?,
        line_height,
    })
}

/// The restored line height: what the reference renderer's restore path gives a
/// text element whose source `lineHeight` is missing or unusable (#108),
/// detected from the element's height when it has one, otherwise the source
/// family's metadata line height.
///
/// `height` and `font_size` are the compatibility-normalized values, so a
/// negative source height detects from its magnitude and a missing or invalid
/// `fontSize` detects with the normalized size (the default, or the size a
/// legacy `font` string migrated to) rather than reproducing the reference's
/// `NaN`; a zero height falls through as the reference's truthiness check does.
/// The family metadata line height is keyed on the source family, so Helvetica
/// keeps `1.15` although Liberation Sans shapes it, and a legacy `font` string
/// never changes it because the reference reads the element's own `fontFamily`.
/// Every normalized family is in the table; the Excalifont fallback mirrors the
/// reference's for a family it has not registered. A detected value that
/// overflows to infinity, which only a degenerate height or font size can
/// produce, is excluded like every other non-finite metric and takes the family
/// value too.
fn restored_line_height(height: f64, text: &str, font_size: f64, font_family: i32) -> f64 {
    let family_line_height = FamilyMetrics::for_source_family(font_family)
        .unwrap_or(FamilyMetrics::EXCALIFONT)
        .line_height;
    if height <= 0.0 {
        return family_line_height;
    }
    let detected = height / restored_line_count(text) as f64 / font_size;
    if detected.is_finite() && detected > 0.0 {
        detected
    } else {
        family_line_height
    }
}

/// The line count the reference renderer's `splitIntoLines` reports for wrapped text:
/// every `\r\n`, `\r`, and `\n` ends a line, and the text always has at least
/// one. Tab expansion happens on the same path but adds no lines.
fn restored_line_count(text: &str) -> usize {
    let mut lines = 1_usize;
    let mut bytes = text.bytes().peekable();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\n' => lines += 1,
            b'\r' => {
                lines += 1;
                if bytes.peek() == Some(&b'\n') {
                    bytes.next();
                }
            }
            _ => {}
        }
    }
    lines
}

/// The `fontSize` and `fontFamily` the reference renderer's restore path
/// migrates a legacy CSS-like `font` string to, or `None` when the string
/// yields no usable size.
///
/// `restoreElement` splits the string on single spaces, takes `parseFloat` of
/// the first token (which strips the `px` unit) and looks the second token up
/// in `FONT_FAMILY` by exact name, falling back to Excalifont for a missing or
/// unknown name. The multi-word family names can never match through a split
/// on spaces, so they take the same fallback. A size that is not a positive
/// finite number is the reference's `NaN` (or an unrenderable size), which
/// revision 1 treats as a malformed value rather than reproducing.
fn legacy_font_migration(font: &str) -> Option<(f64, i32)> {
    let mut tokens = font.split(' ');
    let font_size = ecmascript_parse_float(tokens.next().unwrap_or(""));
    if !(font_size.is_finite() && font_size > 0.0) {
        return None;
    }
    let font_family = match tokens.next() {
        Some("Virgil") => 1,
        Some("Helvetica") => 2,
        Some("Cascadia") => 3,
        Some("Excalifont") => 5,
        Some("Nunito") => 6,
        Some(_) | None => 5,
    };
    Some((font_size, font_family))
}

/// The number ECMAScript `parseFloat` reads from `text`: the longest prefix
/// after the leading white space and line terminators that is a
/// `StrDecimalLiteral`, or `NaN` when there is none.
fn ecmascript_parse_float(text: &str) -> f64 {
    fn decimal_digits(bytes: &[u8]) -> usize {
        bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    }
    let text = text.trim_start_matches(is_ecmascript_white_space);
    let bytes = text.as_bytes();
    let signed = matches!(bytes.first(), Some(b'+' | b'-'));
    let mut end = usize::from(signed);
    if text[end..].starts_with("Infinity") {
        return if bytes.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let integer_digits = decimal_digits(&bytes[end..]);
    end += integer_digits;
    let mut fraction_digits = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction_digits = decimal_digits(&bytes[end + 1..]);
        if integer_digits > 0 || fraction_digits > 0 {
            end += 1 + fraction_digits;
        }
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return f64::NAN;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent_end = end + 1;
        if matches!(bytes.get(exponent_end), Some(b'+' | b'-')) {
            exponent_end += 1;
        }
        let exponent_digits = decimal_digits(&bytes[exponent_end..]);
        if exponent_digits > 0 {
            end = exponent_end + exponent_digits;
        }
    }
    text[..end].parse().unwrap_or(f64::NAN)
}

/// ECMAScript `WhiteSpace` (including every Unicode `Zs` scalar) and
/// `LineTerminator`, which `parseFloat` trims from the start of its input.
fn is_ecmascript_white_space(character: char) -> bool {
    matches!(character, '\u{2000}'..='\u{200A}')
        || matches!(
            character,
            '\t' | '\n'
                | '\u{000B}'
                | '\u{000C}'
                | '\r'
                | ' '
                | '\u{00A0}'
                | '\u{1680}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
        )
}

fn normalize_linear(
    fields: &CommonFields<'_>,
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<(NormalizedLinear, ElementBox), TryReserveError> {
    let source = context.source;
    let policy = context.policy;
    let format = context.format;
    let element_pointer = context.element_pointer;
    let element_id = context.element_id;
    if matches!(source.points, SourceField::Invalid) {
        context.unsupported(
            "points",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    let source_points = match source.points {
        SourceField::Value(points) if points.len() >= 2 => Some(points),
        SourceField::Missing | SourceField::Value(_) | SourceField::Invalid => None,
    };
    let point_count = source_points.map_or(2, |points| points.len());
    let mut points = Vec::new();
    let mut geometry_available = true;
    points.try_reserve_exact(point_count)?;
    if let Some(source_points) = source_points {
        for index in 0..source_points.len() {
            match source_points.point(index) {
                SourceField::Value(point) => {
                    points.push(point);
                    for component in 2..source_points.component_count(index).unwrap_or(2) {
                        let pointer = pointer_index(
                            &pointer_index(&pointer_child(element_pointer, "points")?, index)?,
                            component,
                        )?;
                        context.unsupported_property_at(
                            "points",
                            nested_path_ordinal(index, component),
                            &pointer,
                        )?;
                    }
                }
                SourceField::Missing | SourceField::Invalid => {
                    geometry_available = false;
                    let mut pointer =
                        pointer_index(&pointer_child(element_pointer, "points")?, index)?;
                    if let Some(component) = source_points.invalid_component(index) {
                        pointer = pointer_index(&pointer, component)?;
                    }
                    if context.can_retain() {
                        context.diagnostics.try_reserve(1)?;
                        context.diagnostics.push((
                            SourceDiagnosticPosition::nested(
                                source.member_position("points").unwrap_or(usize::MAX),
                                nested_path_ordinal(
                                    index,
                                    source_points.invalid_component(index).unwrap_or(0),
                                ),
                            ),
                            PendingDiagnostic::new(
                                DiagnosticKind::UnsupportedPropertyValue {
                                    recovery: Recovery::Placeholder,
                                    recipe: Some(FallbackRecipe::KnownElementUnavailable),
                                },
                                format,
                                &[DiagnosticEffect::Visual],
                                Some(element_id),
                                &pointer,
                                "malformed linear point used the known-element fallback",
                            )?,
                        ));
                    }
                    points.clear();
                    points.push([0.0, 0.0]);
                    points.push([fields.width, fields.height]);
                    break;
                }
            }
        }
    } else {
        points.push([0.0, 0.0]);
        points.push([fields.width, fields.height]);
    }

    let offset = points[0];
    let mut x = fields.x;
    let mut y = fields.y;
    if offset != [0.0, 0.0] {
        x = canonical_zero(x + offset[0]);
        y = canonical_zero(y + offset[1]);
        for point in &mut points {
            point[0] = canonical_zero(point[0] - offset[0]);
            point[1] = canonical_zero(point[1] - offset[1]);
        }
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in &points {
        min_x = min_x.min(point[0]);
        min_y = min_y.min(point[1]);
        max_x = max_x.max(point[0]);
        max_y = max_y.max(point[1]);
    }
    let width = canonical_zero(max_x - min_x);
    let height = canonical_zero(max_y - min_y);
    let kind = if matches!(source.kind, ElementKind::Arrow) {
        NormalizedLinearKind::Arrow
    } else {
        NormalizedLinearKind::Line
    };
    if matches!(source.kind, ElementKind::Arrow) && matches!(source.elbowed, SourceField::Invalid) {
        context.unsupported(
            "elbowed",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    for (field, arrowhead) in [
        ("startArrowhead", source.start_arrowhead),
        ("endArrowhead", source.end_arrowhead),
    ] {
        if !arrowhead_is_valid_or_missing(arrowhead) {
            context.unsupported(
                field,
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Substituted,
                None,
            )?;
        }
    }
    let elbowed = matches!(source.kind, ElementKind::Arrow)
        && defaulted(source.elbowed, false).unwrap_or(false);
    let start_binding = normalize_binding(
        source.start_binding,
        elbowed,
        "startBinding",
        policy,
        context,
    )?;
    let end_binding =
        normalize_binding(source.end_binding, elbowed, "endBinding", policy, context)?;
    let start_arrowhead = normalized_arrowhead(source.start_arrowhead, None)?;
    let end_arrowhead = normalized_arrowhead(
        source.end_arrowhead,
        matches!(kind, NormalizedLinearKind::Arrow).then_some("arrow"),
    )?;
    Ok((
        NormalizedLinear {
            kind,
            points,
            start_binding,
            end_binding,
            last_committed_point: None,
            start_arrowhead,
            end_arrowhead,
            elbowed,
            geometry_available,
        },
        ElementBox {
            x,
            y,
            width,
            height,
        },
    ))
}

fn normalize_binding(
    binding_field: SourceField<Option<BindingSource<'_>>>,
    elbowed: bool,
    field: &str,
    policy: &SceneSurfacePolicy,
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<Option<NormalizedBinding>, TryReserveError> {
    let source = match binding_field {
        SourceField::Missing | SourceField::Value(None) => return Ok(None),
        SourceField::Invalid => {
            context.unsupported(
                field,
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Substituted,
                None,
            )?;
            return Ok(None);
        }
        SourceField::Value(Some(source)) => source,
    };
    let record_kind = if elbowed {
        "nested.fixed_point_binding"
    } else {
        "nested.point_binding"
    };
    for (position, member) in source.members().iter().enumerate() {
        let semantic_path = pointer_child("", &member.key)?;
        if !crate::scene_surface::is_manifested_path(policy, record_kind, &semantic_path) {
            context.unsupported_property_nested(field, &member.key, position)?;
        }
    }
    let fixed_point = match (elbowed, source.fixed_point()) {
        (true, SourceField::Missing) => return Ok(None),
        (true, SourceField::Value(mut point)) => {
            for component in &mut point {
                if (*component - 0.5).abs() < 1e-4 {
                    *component = 0.5001;
                }
            }
            Some(point)
        }
        (true, SourceField::Invalid) => {
            context.unsupported_nested(
                field,
                "fixedPoint",
                source.member_position("fixedPoint"),
                source.fixed_point_invalid_component(),
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Substituted,
            )?;
            Some([0.0, 0.0])
        }
        (false, _) => None,
    };
    let binding_element_id = match source.element_id() {
        SourceField::Value(binding_element_id) if !binding_element_id.is_empty() => {
            binding_element_id
        }
        SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => {
            context.unsupported_nested(
                field,
                "elementId",
                source.member_position("elementId"),
                None,
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Omitted,
            )?;
            return Ok(None);
        }
    };
    // Excalidraw's restore keeps a binding without `gap`, which later
    // releases no longer persist, as it keeps one without `focus`.
    let gap = match source.gap() {
        SourceField::Missing => 0.0,
        SourceField::Value(gap) => gap,
        SourceField::Invalid => {
            context.unsupported_nested(
                field,
                "gap",
                source.member_position("gap"),
                None,
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Omitted,
            )?;
            return Ok(None);
        }
    };
    let focus = match source.focus() {
        SourceField::Missing => 0.0,
        SourceField::Value(focus) => focus,
        SourceField::Invalid => {
            context.unsupported_nested(
                field,
                "focus",
                source.member_position("focus"),
                None,
                &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                Recovery::Omitted,
            )?;
            return Ok(None);
        }
    };
    Ok(Some(NormalizedBinding {
        element_id: try_owned(binding_element_id)?,
        focus: canonical_zero(focus),
        gap: canonical_zero(gap),
        fixed_point,
    }))
}

fn normalized_arrowhead(
    source: SourceField<Option<&str>>,
    default: Option<&str>,
) -> Result<Option<String>, TryReserveError> {
    match source {
        SourceField::Value(Some(value)) if is_arrowhead(value) => Ok(Some(try_owned(value)?)),
        SourceField::Value(None) => Ok(None),
        SourceField::Missing | SourceField::Invalid | SourceField::Value(Some(_)) => {
            default.map(try_owned).transpose()
        }
    }
}

fn arrowhead_is_valid_or_missing(source: SourceField<Option<&str>>) -> bool {
    matches!(source, SourceField::Missing | SourceField::Value(None))
        || matches!(source, SourceField::Value(Some(value)) if is_arrowhead(value))
}

pub(super) fn is_arrowhead(value: &str) -> bool {
    matches!(
        value,
        "arrow"
            | "bar"
            | "dot"
            | "circle"
            | "circle_outline"
            | "triangle"
            | "triangle_outline"
            | "diamond"
            | "diamond_outline"
            | "crowfoot_one"
            | "crowfoot_many"
            | "crowfoot_one_or_many"
    )
}

fn normalize_freedraw(
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<NormalizedFreedraw, TryReserveError> {
    let source = context.source;
    let format = context.format;
    let element_pointer = context.element_pointer;
    let element_id = context.element_id;
    if matches!(source.points, SourceField::Invalid) {
        context.unsupported(
            "points",
            &[DiagnosticEffect::Visual],
            Recovery::Placeholder,
            Some(FallbackRecipe::KnownElementUnavailable),
        )?;
    }
    if matches!(source.simulate_pressure, SourceField::Invalid) {
        context.unsupported(
            "simulatePressure",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    let (points, geometry_available) = normalized_freedraw_points(source.points)?;
    if let SourceField::Value(source_points) = source.points {
        for index in 0..source_points.len() {
            if !matches!(source_points.point(index), SourceField::Value(_)) {
                continue;
            }
            for component in 2..source_points.component_count(index).unwrap_or(2) {
                let pointer = pointer_index(
                    &pointer_index(&pointer_child(element_pointer, "points")?, index)?,
                    component,
                )?;
                context.unsupported_property_at(
                    "points",
                    nested_path_ordinal(index, component),
                    &pointer,
                )?;
            }
        }
    }
    if !geometry_available {
        if let SourceField::Value(source_points) = source.points {
            if let Some((index, component)) = source_points.first_invalid_point() {
                let mut pointer = pointer_index(&pointer_child(element_pointer, "points")?, index)?;
                if let Some(component) = component {
                    pointer = pointer_index(&pointer, component)?;
                }
                if context.can_retain() {
                    context.diagnostics.try_reserve(1)?;
                    context.diagnostics.push((
                        SourceDiagnosticPosition::nested(
                            source.member_position("points").unwrap_or(usize::MAX),
                            nested_path_ordinal(index, component.unwrap_or(0)),
                        ),
                        PendingDiagnostic::new(
                            DiagnosticKind::UnsupportedPropertyValue {
                                recovery: Recovery::Placeholder,
                                recipe: Some(FallbackRecipe::KnownElementUnavailable),
                            },
                            format,
                            &[DiagnosticEffect::Visual],
                            Some(element_id),
                            &pointer,
                            "malformed freedraw point used the known-element fallback",
                        )?,
                    ));
                }
            }
        }
    }
    let simulate_pressure = defaulted(source.simulate_pressure, true).unwrap_or(true);
    let source_point_count = match source.points {
        SourceField::Value(source_points) => source_points.len(),
        SourceField::Missing | SourceField::Invalid => points.len(),
    };
    if !pressures_are_valid(source.pressures, source_point_count, simulate_pressure) {
        context.unsupported(
            "pressures",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    let pressures = normalized_pressures(source.pressures, source_point_count, simulate_pressure)?;
    Ok(NormalizedFreedraw {
        points,
        pressures,
        simulate_pressure,
        last_committed_point: None,
        geometry_available,
    })
}

fn normalize_image(
    participation: NormalizationParticipation,
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<NormalizedImage, TryReserveError> {
    let source = context.source;
    let policy = context.policy;
    if matches!(source.file_id, SourceField::Invalid) {
        context.unsupported(
            "fileId",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    if !matches!(
        source.image_status,
        SourceField::Missing | SourceField::Value("pending" | "saved" | "error")
    ) {
        context.unsupported(
            "status",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    if !image_scale_is_valid(source.image_scale) {
        context.unsupported(
            "scale",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    if matches!(source.image_crop, SourceField::Invalid) {
        context.unsupported(
            "crop",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    } else if participation.authoritative_file {
        if let SourceField::Value(Some(crop)) = source.image_crop {
            diagnose_invalid_crop_members(context, crop)?;
            for (position, member) in crop.members().iter().enumerate() {
                let semantic_path = pointer_child("", &member.key)?;
                if !crate::scene_surface::is_manifested_path(
                    policy,
                    "nested.image_crop",
                    &semantic_path,
                ) {
                    context.unsupported_property_nested("crop", &member.key, position)?;
                }
            }
        }
    }
    let status = match source.image_status {
        SourceField::Value("saved") => NormalizedImageStatus::Saved,
        SourceField::Value("error") => NormalizedImageStatus::Error,
        SourceField::Missing
        | SourceField::Invalid
        | SourceField::Value("pending")
        | SourceField::Value(_) => NormalizedImageStatus::Pending,
    };
    let file_id = match source.file_id {
        SourceField::Value(Some(file_id)) => Some(file_id),
        SourceField::Missing | SourceField::Invalid | SourceField::Value(None) => None,
    };
    if !matches!(source.file_id, SourceField::Invalid)
        && (!participation.authoritative_file || file_id.is_none())
    {
        context.missing_image_file(file_id, status)?;
    }
    Ok(NormalizedImage {
        file_id: file_id.map(try_owned).transpose()?,
        status,
        scale: normalized_image_scale(source.image_scale),
        crop: participation
            .authoritative_file
            .then(|| normalized_image_crop(source.image_crop))
            .flatten(),
    })
}

fn diagnose_invalid_crop_members(
    context: &mut ElementDiagnosticContext<'_, '_>,
    crop: ImageCropSource<'_>,
) -> Result<(), TryReserveError> {
    let crop_pointer = pointer_child(context.element_pointer, "crop")?;
    let top_position = context
        .source
        .member_position("crop")
        .unwrap_or(usize::MAX / 2);
    for (manifest_ordinal, (field, accepts_zero)) in [
        ("x", true),
        ("y", true),
        ("width", false),
        ("height", false),
        ("naturalWidth", false),
        ("naturalHeight", false),
    ]
    .into_iter()
    .enumerate()
    {
        if !context.can_retain() {
            break;
        }
        let value = crop.number(field);
        let valid = matches!(value, SourceField::Value(value) if value > 0.0 || (accepts_zero && value == 0.0));
        if valid {
            continue;
        }
        let pointer = pointer_child(&crop_pointer, field)?;
        context.diagnostics.try_reserve(1)?;
        let position = crop.member_position(field).map_or(
            SourceDiagnosticPosition::nested(
                top_position,
                crop.len().saturating_add(manifest_ordinal),
            ),
            |nested| SourceDiagnosticPosition::nested(top_position, nested),
        );
        context.diagnostics.push((
            position,
            PendingDiagnostic::new(
                DiagnosticKind::UnsupportedPropertyValue {
                    recovery: Recovery::Omitted,
                    recipe: None,
                },
                context.format,
                &[DiagnosticEffect::Visual],
                Some(context.element_id),
                &pointer,
                "invalid image crop member disabled crop rendering",
            )?,
        ));
    }
    Ok(())
}

fn normalized_freedraw_points(
    source: SourceField<PointArraySource<'_>>,
) -> Result<(Vec<[f64; 2]>, bool), TryReserveError> {
    let SourceField::Value(source) = source else {
        return Ok((Vec::new(), !matches!(source, SourceField::Invalid)));
    };
    let mut points = Vec::new();
    points.try_reserve_exact(source.len())?;
    for index in 0..source.len() {
        let SourceField::Value(point) = source.point(index) else {
            return Ok((Vec::new(), false));
        };
        points.push([canonical_zero(point[0]), canonical_zero(point[1])]);
    }
    Ok((points, true))
}

fn normalized_pressures(
    source: SourceField<NumberArraySource<'_>>,
    point_count: usize,
    simulate_pressure: bool,
) -> Result<Vec<f64>, TryReserveError> {
    let SourceField::Value(source) = source else {
        return Ok(Vec::new());
    };
    if !simulate_pressure && source.len() != point_count {
        return Ok(Vec::new());
    }
    let mut pressures = Vec::new();
    pressures.try_reserve_exact(source.len())?;
    for index in 0..source.len() {
        let SourceField::Value(pressure) = source.number(index) else {
            return Ok(Vec::new());
        };
        if !(0.0..=1.0).contains(&pressure) {
            return Ok(Vec::new());
        }
        pressures.push(canonical_zero(pressure));
    }
    Ok(pressures)
}

pub(super) fn pressures_are_valid(
    source: SourceField<NumberArraySource<'_>>,
    point_count: usize,
    simulate_pressure: bool,
) -> bool {
    match source {
        SourceField::Missing => true,
        SourceField::Invalid => false,
        SourceField::Value(source) => {
            (simulate_pressure || source.len() == point_count)
                && (0..source.len()).all(|index| {
                    matches!(source.number(index), SourceField::Value(value) if (0.0..=1.0).contains(&value))
                })
        }
    }
}

fn normalized_optional_string(
    source: SourceField<Option<&str>>,
) -> Result<Option<String>, TryReserveError> {
    match source {
        SourceField::Value(Some(value)) => Ok(Some(try_owned(value)?)),
        SourceField::Missing | SourceField::Invalid | SourceField::Value(None) => Ok(None),
    }
}

fn normalized_image_scale(source: SourceField<NumberArraySource<'_>>) -> [i8; 2] {
    let SourceField::Value(values) = source else {
        return [1, 1];
    };
    if values.len() != 2 {
        return [1, 1];
    }
    let component = |index| match values.number(index) {
        SourceField::Value(-1.0) => Some(-1),
        SourceField::Value(1.0) => Some(1),
        SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => None,
    };
    match (component(0), component(1)) {
        (Some(x), Some(y)) => [x, y],
        _ => [1, 1],
    }
}

fn image_scale_is_valid(source: SourceField<NumberArraySource<'_>>) -> bool {
    match source {
        SourceField::Missing => true,
        SourceField::Invalid => false,
        SourceField::Value(values) => {
            values.len() == 2
                && [0, 1]
                    .into_iter()
                    .all(|index| matches!(values.number(index), SourceField::Value(-1.0 | 1.0)))
        }
    }
}

pub(super) fn normalized_image_crop(
    source: SourceField<Option<ImageCropSource<'_>>>,
) -> Option<NormalizedImageCrop> {
    let SourceField::Value(Some(source)) = source else {
        return None;
    };
    let number = |key| match source.number(key) {
        SourceField::Value(value) => Some(value),
        SourceField::Missing | SourceField::Invalid => None,
    };
    let crop = NormalizedImageCrop {
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
        natural_width: number("naturalWidth")?,
        natural_height: number("naturalHeight")?,
    };
    (crop.x >= 0.0
        && crop.y >= 0.0
        && crop.width > 0.0
        && crop.height > 0.0
        && crop.natural_width > 0.0
        && crop.natural_height > 0.0)
        .then_some(crop)
}

fn normalize_frame(
    participation: NormalizationParticipation,
    context: &mut ElementDiagnosticContext<'_, '_>,
) -> Result<NormalizedFrame, TryReserveError> {
    let source = context.source;
    if participation.frame_names && matches!(source.name, SourceField::Invalid) {
        context.unsupported(
            "name",
            &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
            Recovery::Substituted,
            None,
        )?;
    }
    Ok(NormalizedFrame {
        name: if participation.frame_names {
            normalized_optional_string(source.name)?
        } else {
            None
        },
    })
}

fn normalize_embeddable(
    source: &ElementSource<'_>,
) -> Result<NormalizedElementKind, TryReserveError> {
    let iframe = matches!(source.kind, ElementKind::Iframe);
    let label = if iframe {
        try_owned("IFrame element")?
    } else {
        embeddable_label(source.link)?
    };
    Ok(NormalizedElementKind::Embeddable { iframe, label })
}

/// The placeholder label of an embeddable element. The reference renderer
/// derives it from the element's link after its restore path has normalized
/// it, so the label is the sanitized link: an unsafe link reads `about:blank`,
/// the sanitizer's replacement, rather than exposing the rejected text, and a
/// missing or blank link reads `Empty Web-Embed`.
pub(super) fn embeddable_label(link: SourceField<Option<&str>>) -> Result<String, TryReserveError> {
    match link {
        SourceField::Value(Some(link)) if !link.trim().is_empty() => {
            match crate::scene_surface::sanitized_link(link)? {
                Some(sanitized) => Ok(sanitized),
                None => try_owned("about:blank"),
            }
        }
        SourceField::Value(Some(_))
        | SourceField::Missing
        | SourceField::Invalid
        | SourceField::Value(None) => try_owned("Empty Web-Embed"),
    }
}

pub(super) fn normalized_roundness(
    source: &ElementSource<'_>,
    policy: &'static SceneSurfacePolicy,
) -> Option<NormalizedRoundness> {
    if crate::scene_surface::element_ignores_inherited_path(
        policy,
        source.kind.scene_surface_kind(),
        "/roundness",
    ) {
        return None;
    }
    let roundness = match source.roundness {
        SourceField::Value(Some(roundness)) => roundness,
        // restoreElementWithProperties consults the legacy `strokeSharpness`
        // only when the element carries no `roundness` record: `"round"`
        // becomes ROUNDNESS.LEGACY for the adaptive-radius kinds and
        // PROPORTIONAL_RADIUS for every other kind, and getCornerRadius treats
        // both alike; `"sharp"` and any other value stay `null`.
        SourceField::Missing | SourceField::Value(None) => {
            return matches!(source.stroke_sharpness, SourceField::Value("round"))
                .then_some(NormalizedRoundness::Proportional);
        }
        // A malformed record is diagnosed at `/roundness` and, like the
        // truthy malformed value the reference keeps verbatim, rounds nothing.
        SourceField::Invalid => return None,
    };
    let SourceField::Value(kind @ 1..=3) = roundness.kind() else {
        return None;
    };
    let value = match roundness.value() {
        SourceField::Value(value) if value >= 0.0 => Some(value),
        SourceField::Missing => None,
        SourceField::Value(_) | SourceField::Invalid => return None,
    };
    match kind {
        1 | 2 => Some(NormalizedRoundness::Proportional),
        3 => Some(NormalizedRoundness::Adaptive {
            radius: value.unwrap_or(32.0),
        }),
        _ => None,
    }
}
